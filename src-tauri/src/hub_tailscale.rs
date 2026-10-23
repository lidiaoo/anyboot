//! Tailscale 直连支持：探测本机 tailscale CLI，给远程接入页提供「虚拟局域网地址」。
//! 语义与 gateway 对齐：cloudflared 是「暴露公网」，Tailscale 是「组虚拟局域网」——
//! 地址 100.x.y.z（CGNAT 段 100.64.0.0/10）长期固定、不出公网，配对串 tunnel 直接填
//! `http://<ip>:<port>`。地址解析、探测选路、配对鉴权全链路零新增（它就是普通 remote_addr）。
//!
//! CLI 探测顺序（`status --json` 三端输出结构一致）：
//! - Windows: `%ProgramFiles%\Tailscale\tailscale.exe`
//! - macOS:   App 版 `/Applications/Tailscale.app/Contents/MacOS/Tailscale`，
//!            standalone/brew 版 `/usr/local/bin/tailscale`、`/opt/homebrew/bin/tailscale`
//! - Linux:   `/usr/bin/tailscale`、`/usr/local/bin/tailscale`、`/snap/bin/tailscale`
//! - 兜底：PATH 逐目录查找（Windows 补 .exe）
//! status --json 解析失败时退回 `tailscale ip -4` 纯文本（老版本字段名变动的兜底）。
//!
//! 下载器对齐 hub_gateway 的约定：纯线程模型——std::thread 里跑阻塞下载，
//! 进度/结果用事件推前端（`tailscale-progress` / `tailscale-done`），AtomicBool 防重入。

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

/// 本机 Tailscale 状态（远程接入页「Tailscale 直连」卡的数据源）。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TailscaleStatus {
    /// 找到了 tailscale CLI。
    pub installed: bool,
    /// running / needs_login / stopped / unknown。
    pub state: String,
    /// 本机 Tailscale IPv4（100.x.y.z；未运行时为空）。
    pub ip: String,
    /// MagicDNS 全名（已去尾点；可能为空）。
    pub dns_name: String,
    /// CLI 版本（展示用）。
    pub version: String,
    /// 探测到的 CLI 路径（排障展示用；未找到为空）。
    pub cli_path: String,
}

impl Default for TailscaleStatus {
    fn default() -> Self {
        Self {
            installed: false,
            state: "unknown".into(),
            ip: String::new(),
            dns_name: String::new(),
            version: String::new(),
            cli_path: String::new(),
        }
    }
}

// ==================== CLI 探测 ====================

fn cli_candidates() -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    if cfg!(target_os = "windows") {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            out.push(std::path::PathBuf::from(pf).join("Tailscale").join("tailscale.exe"));
        }
        out.push(std::path::PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe"));
    } else if cfg!(target_os = "macos") {
        out.push(std::path::PathBuf::from(
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ));
        out.push(std::path::PathBuf::from("/usr/local/bin/tailscale"));
        out.push(std::path::PathBuf::from("/opt/homebrew/bin/tailscale"));
    } else {
        out.push(std::path::PathBuf::from("/usr/bin/tailscale"));
        out.push(std::path::PathBuf::from("/usr/local/bin/tailscale"));
        out.push(std::path::PathBuf::from("/snap/bin/tailscale"));
    }
    // PATH 兜底（Windows 上 tailscale.exe，其余平台 tailscale）
    if let Some(path) = std::env::var_os("PATH") {
        let exe = if cfg!(windows) { "tailscale.exe" } else { "tailscale" };
        for dir in std::env::split_paths(&path) {
            out.push(dir.join(exe));
        }
    }
    out
}

fn find_cli() -> Option<std::path::PathBuf> {
    cli_candidates().into_iter().find(|p| p.is_file())
}

fn run_cli(cli: &std::path::Path, args: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new(cli);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    Some(text)
}

// status --json 只取需要的字段；字段名大小写按实测（BackendState / Self.TailscaleIPs 混 camel）。
#[derive(Deserialize)]
struct StatusJson {
    #[serde(default, rename = "BackendState")]
    backend_state: String,
    #[serde(default, rename = "Version")]
    version: String,
    #[serde(default, rename = "Self")]
    self_peer: Option<SelfPeer>,
}

#[derive(Deserialize)]
struct SelfPeer {
    #[serde(default, rename = "DNSName")]
    dns_name: String,
    #[serde(default, rename = "TailscaleIPs")]
    ips: Vec<String>,
}

fn normalize_state(raw: &str) -> String {
    match raw {
        "Running" => "running".to_string(),
        "NeedsLogin" => "needs_login".to_string(),
        "Stopped" => "stopped".to_string(),
        "" => "unknown".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

/// 从 TailscaleIPs 里挑 IPv4（实测混着 fd7a:… 的 v6）。
fn pick_v4(ips: &[String]) -> String {
    ips.iter()
        .find(|ip| ip.contains('.') && !ip.contains(':'))
        .cloned()
        .unwrap_or_default()
}

/// 核心探测：跑 `status --json`；JSON 解析失败退 `ip -4` 纯文本兜底。
fn probe(cli: &std::path::Path) -> TailscaleStatus {
    let mut status = TailscaleStatus {
        installed: true,
        cli_path: cli.display().to_string(),
        ..Default::default()
    };
    if let Some(text) = run_cli(cli, &["status", "--json"]) {
        if let Ok(parsed) = serde_json::from_str::<StatusJson>(&text) {
            status.state = normalize_state(&parsed.backend_state);
            status.version = parsed.version;
            if let Some(peer) = parsed.self_peer {
                status.ip = pick_v4(&peer.ips);
                status.dns_name = peer.dns_name.trim_end_matches('.').to_string();
            }
            if status.state == "running" && status.ip.is_empty() {
                // Running 但没拿到 v4（少见）：退 ip -4 补一把
                if let Some(ip) = run_cli(cli, &["ip", "-4"]) {
                    status.ip = ip.trim().lines().next().unwrap_or("").trim().to_string();
                }
            }
            return status;
        }
        // JSON 解析失败：老版本字段变动兜底，只拿 ip（state 判不了就 unknown）
        if let Some(ip) = run_cli(cli, &["ip", "-4"]) {
            let ip = ip.trim().lines().next().unwrap_or("").trim().to_string();
            if !ip.is_empty() {
                status.state = "running".into();
                status.ip = ip;
                return status;
            }
        }
    }
    status
}

/// 阻塞版探测（spawn_blocking 里跑；CLI 子进程约几十 ms）。
pub(crate) fn probe_blocking() -> TailscaleStatus {
    match find_cli() {
        Some(cli) => probe(&cli),
        None => TailscaleStatus::default(),
    }
}

// ==================== 下载安装包 ====================

const EVENT_TS_PROGRESS: &str = "tailscale-progress";
const EVENT_TS_DONE: &str = "tailscale-done";

static TS_DL_IN_FLIGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Debug, serde::Serialize)]
struct TsDownloadProgress {
    received: u64,
    total: u64,
    percent: u32,
}

#[derive(Clone, Debug, serde::Serialize)]
struct TsDownloadDone {
    ok: bool,
    error: String,
    /// 下载好的安装包路径（ok 时非空；前端提示「安装器即将弹出」）。
    path: String,
}

/// 各平台 stable 安装包直链。Linux 没有通用安装包 → None（提示走包管理器）。
fn installer_url() -> Option<(String, &'static str)> {
    if cfg!(target_os = "windows") {
        Some((
            "https://pkgs.tailscale.com/stable/tailscale-setup-latest-amd64.exe".into(),
            "tailscale-setup.exe",
        ))
    } else if cfg!(target_os = "macos") {
        Some((
            "https://pkgs.tailscale.com/stable/Tailscale-latest-macos.pkg".into(),
            "Tailscale.pkg",
        ))
    } else {
        None
    }
}

fn ts_cache_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("定位数据目录失败: {error}"))?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("建数据目录失败: {error}"))?;
    Ok(dir)
}

fn finish_ts_download(app: &AppHandle, result: Result<std::path::PathBuf, String>) {
    TS_DL_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
    let (ok, error, path) = match result {
        Ok(path) => (true, String::new(), path.display().to_string()),
        Err(error) => (false, error, String::new()),
    };
    let done = TsDownloadDone { ok, error, path };
    let _ = app.emit(EVENT_TS_DONE, done);
}

fn ts_download_worker(app: AppHandle) {
    let (url, filename) = match installer_url() {
        Some(pair) => pair,
        None => {
            finish_ts_download(
                &app,
                Err("当前平台没有通用安装包，请在终端用系统包管理器安装 Tailscale（如 apt/brew）".into()),
            );
            return;
        }
    };
    let tmp = match ts_cache_dir(&app) {
        Ok(dir) => dir.join(filename),
        Err(error) => {
            finish_ts_download(&app, Err(error));
            return;
        }
    };
    let _ = std::fs::remove_file(&tmp);
    match crate::hub_gateway::download_to_tmp(&url, &tmp) {
        Ok((received, total)) => {
            let _ = app.emit(
                EVENT_TS_PROGRESS,
                TsDownloadProgress { received, total, percent: 100 },
            );
            finish_ts_download(&app, Ok(tmp));
        }
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            finish_ts_download(&app, Err(error));
        }
    }
}

// ==================== Tauri 命令 ====================

/// 查询本机 Tailscale 状态（找不到 CLI → installed:false）。
/// CLI 子进程有几十 ms 阻塞，走 async + spawn_blocking（blocking_guard 约定）。
#[tauri::command]
pub(crate) async fn gateway_tailscale_status() -> TailscaleStatus {
    tauri::async_runtime::spawn_blocking(probe_blocking)
        .await
        .unwrap_or_default()
}

/// 发起后台下载（立即返回；进度/结果走 tailscale-progress / tailscale-done 事件）。
/// 下载完自动弹出平台安装器（Windows 静默参数由安装器自带 UAC；macOS 打开 .pkg）。
#[tauri::command]
pub(crate) fn gateway_tailscale_download(app: AppHandle) -> Result<(), String> {
    if installer_url().is_none() {
        return Err("当前平台没有通用安装包，请用系统包管理器安装 Tailscale".to_string());
    }
    if TS_DL_IN_FLIGHT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("下载已在进行中".to_string());
    }
    let worker_app = app.clone();
    std::thread::spawn(move || ts_download_worker(worker_app));
    Ok(())
}

/// 打开下载好的安装器（下载完成事件里前端自动调；用户 UAC 确认后安装）。
/// 起子进程会阻塞 → async + spawn_blocking（blocking_guard 约定）。
#[tauri::command]
pub(crate) async fn gateway_tailscale_install(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = std::path::PathBuf::from(&path);
        if !path.is_file() {
            return Err("安装包不存在，请重新下载".to_string());
        }
        launch_installer(&path)
    })
    .await
    .map_err(|error| format!("安装任务失败: {error}"))?
}

#[cfg(windows)]
fn launch_installer(path: &std::path::Path) -> Result<(), String> {
    // 静默安装：tailscale-setup.exe 支持 /quiet /norestart；仍会触发 UAC 提权确认。
    let mut command = std::process::Command::new(path);
    command.args(["/quiet", "/norestart"]);
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW（UAC 弹窗不受影响）
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("启动安装器失败: {error}"))
}

#[cfg(target_os = "macos")]
fn launch_installer(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("启动安装器失败: {error}"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn launch_installer(_path: &std::path::Path) -> Result<(), String> {
    Err("当前平台没有通用安装包，请用系统包管理器安装 Tailscale".to_string())
}

/// 本机服务自检：TCP 握手 127.0.0.1:http_port。
/// Tailscale 地址行下展示「本机服务自检通过/未监听」——防端口被保留段占用静默失败甩锅网络。
/// TCP connect 会阻塞 → async + spawn_blocking（blocking_guard 约定）。
#[tauri::command]
pub(crate) async fn local_service_health(app: AppHandle) -> Result<bool, String> {
    let port = crate::stored_settings(&app).http_port;
    tauri::async_runtime::spawn_blocking(move || {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        // std::net::TcpStream::connect 对本机回环握手 <1ms，阻塞 connect 无实际风险
        // （connect_timeout 需要非阻塞套接字的样板，不值得）
        std::net::TcpStream::connect(addr).map(|_| ()).map_err(|error| {
            format!("127.0.0.1:{port} 未监听：{error}")
        })
    })
    .await
    .map_err(|error| format!("自检任务失败: {error}"))?
    .map(|_| true)
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_state_maps_known_values() {
        assert_eq!(normalize_state("Running"), "running");
        assert_eq!(normalize_state("NeedsLogin"), "needs_login");
        assert_eq!(normalize_state("Stopped"), "stopped");
        assert_eq!(normalize_state(""), "unknown");
        assert_eq!(normalize_state("Weird"), "weird");
    }

    #[test]
    fn pick_v4_skips_ipv6() {
        let ips = vec![
            "fd7a:115c:a1e0::b7a1:9b7b:eb36:3737".to_string(),
            "100.119.234.95".to_string(),
        ];
        assert_eq!(pick_v4(&ips), "100.119.234.95");
        assert_eq!(pick_v4(&["fd7a::1".to_string()]), "");
    }

    #[test]
    fn status_json_parses_real_shape() {
        // 字段名按实测（Self.DNSName 带尾点、TailscaleIPs 混 v6）
        let text = r#"{
            "BackendState": "Running",
            "Version": "1.104.2",
            "Self": {
                "DNSName": "win-pc.tailXXXX.ts.net.",
                "TailscaleIPs": ["fd7a:115c:a1e0::b7a1", "100.119.234.95"]
            }
        }"#;
        let parsed: StatusJson = serde_json::from_str(text).unwrap();
        assert_eq!(parsed.backend_state, "Running");
        let peer = parsed.self_peer.unwrap();
        assert_eq!(peer.dns_name.trim_end_matches('.'), "win-pc.tailXXXX.ts.net");
        assert_eq!(pick_v4(&peer.ips), "100.119.234.95");
    }
}
