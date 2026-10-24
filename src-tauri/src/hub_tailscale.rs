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
    /// tailscaled 系统服务状态：1=运行中 / 0=已停止 / -1=查不了（非 Windows 或 sc 失败）。
    /// 两级开关的副开关数据源；state=unknown 且 installed 时多半也是服务没跑。
    #[serde(default)]
    pub service_running: i8,
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
            service_running: -1,
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

/// CLI 单次调用超时。macOS App 版 CLI 在异常场景会挂死（实测残留过两条
/// status --json 僵进程），无超时会拖死探测线程、状态卡永远转圈。
const CLI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 构造 CLI 子进程命令（统一入口）。
///
/// macOS App 版 CLI 用 `SHLVL`（只有 shell 会设）判断「是不是 shell 在调我」：
/// Rust 直接 spawn 的子进程没有它 → CLI 误判为 GUI 启动场景 → 试图拉起
/// Tailscale.app → 失败时 exit 0 + stdout 全是「Tailscale GUI failed to start
/// (Tailscale.CLIError error 3.)」报错文本（GUI 起没起都一样，实机踩过：
/// 错误文本被当成 ip 拼进直连地址）。补 SHLVL 强制走 CLI 分支，GUI 不在跑也正常；
/// standalone/brew 版 CLI 不读它，设了无副作用。
fn cli_command(cli: &std::path::Path) -> std::process::Command {
    let mut command = std::process::Command::new(cli);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(target_os = "macos")]
    command.env("SHLVL", "1");
    command
}

/// CLI 调用统一实现：超时 + stdout/stderr 并发读（等退出再读会在输出超管道
/// 缓冲时死锁），返回 (exit 是否成功, stdout, stderr)；spawn 失败 / 超时被杀 → None。
fn run_cli_raw(cli: &std::path::Path, args: &[&str]) -> Option<(bool, String, String)> {
    let mut command = cli_command(cli);
    command.args(args);
    command.stdin(std::process::Stdio::null());
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let out_reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).to_string()
    });
    let err_reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).to_string()
    });
    let deadline = std::time::Instant::now() + CLI_TIMEOUT;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(_) => break None,
        }
    };
    let status = exit?;
    Some((status.success(), out_reader.join().ok()?, err_reader.join().ok()?))
}

/// 跑一次 CLI 取 stdout（exit 非 0 → None）。
fn run_cli(cli: &std::path::Path, args: &[&str]) -> Option<String> {
    let (ok, stdout, _) = run_cli_raw(cli, args)?;
    if !ok {
        return None;
    }
    Some(stdout)
}

/// 严格的 IPv4 形状校验（4 段 0-255 纯数字）。macOS App 版 CLI 在 GUI 未启动时
/// exit 0 且把「Tailscale GUI failed to start…」报错文本打到 stdout —— 不能当 ip 采信，
/// 否则错误文本会一路拼进 `http://<ip>:<port>` 直连地址（实机踩过）。
fn plausible_ipv4(text: &str) -> bool {
    let octets: Vec<&str> = text.trim().split('.').collect();
    octets.len() == 4
        && octets.into_iter().all(|o| {
            !o.is_empty()
                && o.len() <= 3
                && o.bytes().all(|b| b.is_ascii_digit())
                && o.parse::<u16>().map(|n| n <= 255).unwrap_or(false)
        })
}

/// macOS App 版 CLI「GUI 没起来」的报错特征（exit 0、错误文本在 stdout）。
/// 主修是 cli_command 补 SHLVL 强制 CLI 模式，正常不会再走到这里；
/// 留作 CLI 行为再变时的兜底状态（前端给「打开 Tailscale.app」自救按钮）。
fn is_gui_failed_output(text: &str) -> bool {
    text.contains("Tailscale GUI failed to start") || text.contains("Tailscale.CLIError")
}

/// 取多行文本的首行（trim 后）。`ip -4` 正常单行输出；报错文本也只看首行。
fn first_line(text: &str) -> String {
    text.trim().lines().next().unwrap_or("").trim().to_string()
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
/// 顺手查一次系统服务状态（副开关数据源；Windows = sc query，其它平台 -1）。
fn probe(cli: &std::path::Path) -> TailscaleStatus {
    let mut status = TailscaleStatus {
        installed: true,
        cli_path: cli.display().to_string(),
        service_running: service_running_state(),
        ..Default::default()
    };
    if let Some(text) = run_cli(cli, &["status", "--json"]) {
        if let Ok(parsed) = serde_json::from_str::<StatusJson>(&text) {
            status.state = normalize_state(&parsed.backend_state);
            status.version = parsed.version;
            if let Some(peer) = parsed.self_peer {
                let ip = pick_v4(&peer.ips);
                if plausible_ipv4(&ip) {
                    status.ip = ip;
                }
                status.dns_name = peer.dns_name.trim_end_matches('.').to_string();
            }
            if status.state == "running" && status.ip.is_empty() {
                // Running 但没拿到 v4（少见）：退 ip -4 补一把
                if let Some(ip) = run_cli(cli, &["ip", "-4"]) {
                    let ip = first_line(&ip);
                    if plausible_ipv4(&ip) {
                        status.ip = ip;
                    }
                }
            }
            return status;
        }
        // JSON 解析失败：老版本字段变动兜底，只拿 ip（state 判不了就 unknown）
        if let Some(text) = run_cli(cli, &["ip", "-4"]) {
            let ip = first_line(&text);
            if plausible_ipv4(&ip) {
                status.state = "running".into();
                status.ip = ip;
                return status;
            }
            // macOS App 版 CLI 的 GUI 未启动报错（exit 0 + stdout 全是错误文本）：
            // 可操作状态，前端给「打开 Tailscale.app」一键自救
            if is_gui_failed_output(&text) {
                status.state = "needs_gui".into();
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

// ==================== 主开关（up/down）+ 服务控制 ====================

/// Windows 服务名（tailscaled 安装器注册的名字就是 "Tailscale"）。
/// 只在 cfg!(windows) 分支里真的执行；非 Windows 编译进来但不会被调。
const SERVICE_NAME: &str = "Tailscale";

/// 查 tailscaled 系统服务状态：1=RUNNING / 0=STOPPED / -1=查不了。
/// 非 Windows 直接 -1（macOS 是 App 常驻、Linux 各发行版差异大，副开关先只对 Windows 亮）。
fn service_running_state() -> i8 {
    service_running_state_impl()
}

/// `sc` 服务控制命令。**必须带 CREATE_NO_WINDOW**：服务状态查询每次渲染远程接入页
/// 都会跑（Tailscale 卡的状态点），不带的话 Windows 上每查一次就闪一个黑框；
/// start/stop/restart 同理。
#[cfg(windows)]
fn sc_command(args: &[&str]) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new("sc");
    command.args(args).creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command
}

#[cfg(not(windows))]
fn sc_command(args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new("sc");
    command.args(args);
    command
}

#[cfg(windows)]
fn service_running_state_impl() -> i8 {
    let output = match sc_command(&["query", SERVICE_NAME]).output()
    {
        Ok(output) => output,
        Err(_) => return -1,
    };
    let text = String::from_utf8_lossy(&output.stdout);
    if text.contains("RUNNING") {
        1
    } else if text.contains("STOPPED") {
        0
    } else {
        -1
    }
}

#[cfg(not(windows))]
fn service_running_state_impl() -> i8 {
    -1
}

/// 阻塞版主开关：`tailscale up / down`（登录级，不动系统服务）。
/// NeedsLogin 时 `up` 会拉起浏览器登录流程（这正是想要的语义）。
pub(crate) fn ts_toggle_blocking(up: bool) -> Result<TailscaleStatus, String> {
    let cli = find_cli().ok_or_else(|| "没检测到 Tailscale，请先安装".to_string())?;
    let (ok, stdout, stderr) = run_cli_full(&cli, if up { &["up"] } else { &["down"] });
    if !ok {
        let detail = stderr.trim().lines().next().unwrap_or("").trim();
        let fallback = if up { "tailscale up 失败" } else { "tailscale down 失败" };
        return Err(if detail.is_empty() && stdout.trim().is_empty() {
            fallback.to_string()
        } else if detail.is_empty() {
            stdout.trim().lines().next().unwrap_or(fallback).trim().to_string()
        } else {
            detail.to_string()
        });
    }
    Ok(probe(&cli))
}

/// 阻塞版服务控制：action = start / stop / restart。
/// 返回控制后的服务状态（1/0/-1）。启停需要管理员权限，被拒时原样回显系统错误。
pub(crate) fn ts_service_blocking(action: &str) -> Result<i8, String> {
    if !cfg!(windows) {
        return Err("当前平台暂不支持服务级启停（副开关仅 Windows 可用）".to_string());
    }
    let args: Vec<&str> = match action {
        "start" => vec!["start", SERVICE_NAME],
        "stop" => vec!["stop", SERVICE_NAME],
        "restart" => vec!["stop", SERVICE_NAME],
        _ => return Err(format!("未知的服务操作：{action}")),
    };
    let sc = sc_command(&args).output();
    let output = match sc {
        Ok(output) => output,
        Err(error) => return Err(format!("执行 sc 失败: {error}")),
    };
    if !output.status.success() {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let detail = text.trim().lines().next().unwrap_or("").trim();
        // 非管理员跑 sc start/stop 会被拒（错误码 5），把话说明白
        if detail.contains("拒绝访问") || detail.contains("Access is denied") || detail.contains("(5)") {
            return Err("需要管理员权限：请右键 AnyBoot 以管理员身份运行后再操作服务级开关".to_string());
        }
        return Err(if detail.is_empty() {
            "服务操作失败".to_string()
        } else {
            detail.to_string()
        });
    }
    if action == "restart" {
        // restart = stop 后再 start；stop 可能要几秒才落地，轮询最多 5 秒
        let mut waited = 0u32;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(300));
            waited += 300;
            if service_running_state() == 0 || waited >= 5000 {
                break;
            }
        }
        let start = sc_command(&["start", SERVICE_NAME]).output();
        if let Ok(output) = start {
            if !output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout).to_string();
                let detail = text.trim().lines().next().unwrap_or("").trim();
                return Err(if detail.is_empty() {
                    "服务重新启动失败".to_string()
                } else {
                    detail.to_string()
                });
            }
        }
    }
    Ok(service_running_state())
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

/// 平台是否有通用安装包（Linux 没有 → false，提示走包管理器）。
fn platform_has_installer() -> bool {
    cfg!(any(target_os = "windows", target_os = "macos"))
}

/// 官方 stable 元数据（https://pkgs.tailscale.com/stable/?mode=json）里挑当前平台安装包文件名。
/// Windows 取 Exes 里的标准 setup exe（支持 /quiet 静默参数；跳过 -full 变体，MSI 直启带不了参）。
/// macOS 取 MacZips 的通用 .pkg。
fn pick_manifest_filename(manifest: &serde_json::Value) -> Option<String> {
    if cfg!(target_os = "windows") {
        manifest["Exes"]
            .as_array()?
            .iter()
            .filter_map(|item| item.as_str())
            .find(|name| {
                name.starts_with("tailscale-setup-")
                    && !name.contains("full")
                    && name.ends_with(".exe")
            })
            .map(str::to_string)
    } else if cfg!(target_os = "macos") {
        manifest["MacZips"]["universal-package"]
            .as_str()
            .map(str::to_string)
    } else {
        None
    }
}

/// 拉取 stable 元数据（失败返回 None，调用方落回 latest 别名兜底）。
fn fetch_stable_manifest() -> Option<serde_json::Value> {
    let body = ureq::get("https://pkgs.tailscale.com/stable/?mode=json")
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .ok()?
        .into_string()
        .ok()?;
    serde_json::from_str(&body).ok()
}

/// 下载直链候选（按序尝试）：官方 latest 别名已被撤下（实测 404），首选
/// ?mode=json 解析出的版本化直链；latest 别名留作兜底（官方若恢复别名可直连）。
fn download_candidates() -> Vec<(String, &'static str)> {
    const BASE: &str = "https://pkgs.tailscale.com/stable";
    let mut candidates: Vec<(String, &'static str)> = Vec::new();
    if cfg!(target_os = "windows") {
        if let Some(name) =
            fetch_stable_manifest().and_then(|manifest| pick_manifest_filename(&manifest))
        {
            candidates.push((format!("{BASE}/{name}"), "tailscale-setup.exe"));
        }
        candidates.push((
            format!("{BASE}/tailscale-setup-latest-amd64.exe"),
            "tailscale-setup.exe",
        ));
    } else if cfg!(target_os = "macos") {
        if let Some(name) =
            fetch_stable_manifest().and_then(|manifest| pick_manifest_filename(&manifest))
        {
            candidates.push((format!("{BASE}/{name}"), "Tailscale.pkg"));
        }
        candidates.push((format!("{BASE}/Tailscale-latest-macos.pkg"), "Tailscale.pkg"));
    }
    candidates
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
    let candidates = download_candidates();
    if candidates.is_empty() {
        finish_ts_download(
            &app,
            Err("当前平台没有通用安装包，请在终端用系统包管理器安装 Tailscale（如 apt/brew）".into()),
        );
        return;
    }
    let dir = match ts_cache_dir(&app) {
        Ok(dir) => dir,
        Err(error) => {
            finish_ts_download(&app, Err(error));
            return;
        }
    };
    let mut last_error = String::new();
    for (url, filename) in candidates {
        let tmp = dir.join(filename);
        let _ = std::fs::remove_file(&tmp);
        match crate::hub_gateway::download_to_tmp(&url, &tmp) {
            Ok((received, total)) => {
                let _ = app.emit(
                    EVENT_TS_PROGRESS,
                    TsDownloadProgress { received, total, percent: 100 },
                );
                finish_ts_download(&app, Ok(tmp));
                return;
            }
            Err(error) => {
                let _ = std::fs::remove_file(&tmp);
                last_error = error;
            }
        }
    }
    finish_ts_download(&app, Err(last_error));
}

// ==================== Funnel 公网通道 ====================
// 借鉴 dsh-mobile 的「Funnel 通道 + 状态机 + 授权引导」设计，实现走官方 CLI：
// `tailscale funnel --bg <port>` 的 TLS 与监听都在 tailscaled 系统服务内，
// 不产生子进程、强杀 anyboot 也不留残留（对齐残留通道管理的「沿用」语义：
// 启动时 funnel status 非空即视为已开启）。

static TS_FUNNEL_IN_FLIGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Funnel 通道状态（tsCard 区块 B 的数据源）。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TailscaleFunnelStatus {
    /// unavailable / off / starting / needs_authorize / ready / error
    pub state: String,
    /// 公网地址 https://机器名.tail名.ts.net（ready 时非空）
    pub url: String,
    /// 错误码（needs_authorize 也带：no_funnel / https_required / needs_login）
    pub error_code: String,
    /// 官方授权页（needs_authorize 时非空；只可能是官方白名单 URL）
    pub setup_url: String,
    /// 面向用户的简短说明
    pub message: String,
}

impl TailscaleFunnelStatus {
    fn new(state: &str) -> Self {
        Self {
            state: state.into(),
            url: String::new(),
            error_code: String::new(),
            setup_url: String::new(),
            message: String::new(),
        }
    }
}

fn funnel_unavailable(message: &str) -> TailscaleFunnelStatus {
    let mut status = TailscaleFunnelStatus::new("unavailable");
    status.message = message.into();
    status
}

/// 带完整输出的 CLI 调用（enable 的错误分类要看 stderr）。同样带超时。
fn run_cli_full(cli: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    run_cli_raw(cli, args).unwrap_or((false, String::new(), "failed to spawn tailscale".into()))
}

/// `funnel status --json` 没配置时是 `{}`；配置过返回非空对象。
/// 不依赖具体字段结构（版本间有差异），只判断是否非空。
fn funnel_configured(cli: &std::path::Path) -> bool {
    let (ok, text, _) = run_cli_full(cli, &["funnel", "status", "--json"]);
    if !ok {
        return false;
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => value.as_object().map(|map| !map.is_empty()).unwrap_or(false),
        // 解析不动但确实输出了内容：当已配置（保守取 ready，UI 可见即可对账）
        Err(_) => true,
    }
}

/// 官方授权页白名单（错误消息里捞到的 URL 必须严格匹配才回给前端）。
const FUNNEL_SETUP_URLS: [&str; 2] = [
    "https://tailscale.com/s/no-funnel",
    "https://tailscale.com/s/https",
];

/// 从 CLI 报错里分类：没开 Funnel 权限 / 没开 HTTPS 证书 → 官方授权引导。
fn classify_funnel_error(stderr: &str, stdout: &str) -> (String, String, String) {
    let haystack = format!("{stderr}\n{stdout}");
    for url in FUNNEL_SETUP_URLS {
        if haystack.contains(url) {
            let code = if url.ends_with("no-funnel") { "no_funnel" } else { "https_required" };
            return ("needs_authorize".into(), code.into(), url.into());
        }
    }
    ("error".into(), "funnel_start_failed".into(), String::new())
}

/// Funnel 未就绪的前置检查（探不到 CLI / 没登录 / 已停止）。
fn funnel_precheck() -> Result<std::path::PathBuf, TailscaleFunnelStatus> {
    let Some(cli) = find_cli() else {
        return Err(funnel_unavailable("未检测到 Tailscale"));
    };
    let base = probe_blocking();
    match base.state.as_str() {
        "needs_login" => Err(funnel_unavailable("Tailscale 未登录 —— 先在上方完成登录")),
        "stopped" => Err(funnel_unavailable("Tailscale 已停止 —— 先在上方把它连上")),
        "running" => Ok(cli),
        _ => Err(funnel_unavailable("Tailscale 状态未知")),
    }
}

/// 阻塞版 funnel 状态探测（spawn_blocking 里跑；CLI 子进程几十 ms）。
pub(crate) fn funnel_probe_blocking() -> TailscaleFunnelStatus {
    let cli = match funnel_precheck() {
        Ok(cli) => cli,
        Err(status) => return status,
    };
    let base = probe_blocking();
    if funnel_configured(&cli) {
        let mut status = TailscaleFunnelStatus::new("ready");
        if !base.dns_name.is_empty() {
            status.url = format!("https://{}", base.dns_name);
        } else {
            status.message = "Funnel 已开启但未取得 DNS 名".into();
        }
        return status;
    }
    TailscaleFunnelStatus::new("off")
}

/// 阻塞版开启 funnel：`tailscale funnel --bg <http_port>`（默认 https 模式上 443）。
/// 报错含官方引导 URL → needs_authorize；否则 error。
pub(crate) fn funnel_enable_blocking(app: &AppHandle) -> TailscaleFunnelStatus {
    let cli = match funnel_precheck() {
        Ok(cli) => cli,
        Err(status) => return status,
    };
    if funnel_configured(&cli) {
        return funnel_probe_blocking();
    }
    let port = crate::stored_settings(app).http_port;
    let (ok, stdout, stderr) = run_cli_full(&cli, &["funnel", "--bg", &port.to_string()]);
    if !ok {
        let (state, code, setup_url) = classify_funnel_error(&stderr, &stdout);
        let mut status = TailscaleFunnelStatus::new(&state);
        status.error_code = code;
        status.setup_url = setup_url;
        let detail = stderr.trim().lines().next().unwrap_or("").trim();
        status.message = if detail.is_empty() {
            "Funnel 开启失败".into()
        } else {
            format!("Funnel 开启失败：{detail}")
        };
        return status;
    }
    // CLI 成功后再确认一次配置真的落上了
    if funnel_configured(&cli) {
        funnel_probe_blocking()
    } else {
        let mut status = TailscaleFunnelStatus::new("error");
        status.error_code = "funnel_start_failed".into();
        status.message = "Funnel 命令成功但配置未生效，请重试".into();
        status
    }
}

/// 阻塞版关闭 funnel：`tailscale funnel reset`（只清 funnel，不动 serve/VPN）。
pub(crate) fn funnel_disable_blocking() -> TailscaleFunnelStatus {
    let cli = match funnel_precheck() {
        Ok(cli) => cli,
        Err(status) => return status,
    };
    let _ = run_cli_full(&cli, &["funnel", "reset"]);
    if funnel_configured(&cli) {
        let mut status = TailscaleFunnelStatus::new("error");
        status.error_code = "funnel_stop_failed".into();
        status.message = "Funnel 关闭失败，请重试".into();
        return status;
    }
    TailscaleFunnelStatus::new("off")
}

// ==================== Tauri 命令 ====================

/// 启动时的 Tailscale 自启：勾了「启动软件时自动连接」就 up 一次。
/// 已连接 / 没装都不动（up 幂等，但没必要白跑一次 CLI）；登录态缺失时
/// up 会把 NeedsLogin 拉起来，和用户手点「连接」完全等价。
pub(crate) fn autostart_connect(app: &tauri::AppHandle) {
    if !crate::stored_settings(app).tailscale_autostart {
        return;
    }
    std::thread::spawn(|| {
        let before = probe_blocking();
        if !before.installed || before.state == "running" {
            return;
        }
        // 服务没跑（Windows tailscaled）先拉起来：up 是对着服务说话的，
        // 服务停着怎么 up 都没用。没管理员权限这里会失败，静默跳过即可。
        if before.service_running == 0 {
            let _ = ts_service_blocking("start");
        }
        let _ = ts_toggle_blocking(true);
    });
}

/// 查询本机 Tailscale 状态（找不到 CLI → installed:false）。
/// CLI 子进程有几十 ms 阻塞，走 async + spawn_blocking（blocking_guard 约定）。
#[tauri::command]
pub(crate) async fn gateway_tailscale_status() -> TailscaleStatus {
    tauri::async_runtime::spawn_blocking(probe_blocking)
        .await
        .unwrap_or_default()
}

static TS_TOGGLE_IN_FLIGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 主开关：tailscale up / down（登录级；NeedsLogin 时 up 拉起浏览器登录）。
/// CLI 子进程阻塞 → async + spawn_blocking；AtomicBool 防连点重入。
#[tauri::command]
pub(crate) async fn gateway_tailscale_toggle(up: bool) -> Result<TailscaleStatus, String> {
    if TS_TOGGLE_IN_FLIGHT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("上一次开关操作还在执行，稍等一下".to_string());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let status = ts_toggle_blocking(up);
        TS_TOGGLE_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
        status
    })
    .await;
    TS_TOGGLE_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
    result.map_err(|error| format!("Tailscale 开关任务失败: {error}"))?
}

/// 副开关（服务级）：start / stop / restart tailscaled 系统服务。
/// 返回控制后的服务状态（1=运行 / 0=停止 / -1=查不了）。
/// 需要管理员权限，被拒时错误里已说明。CLI/子进程阻塞 → async + spawn_blocking。
#[tauri::command]
pub(crate) async fn gateway_tailscale_service(action: String) -> Result<i8, String> {
    tauri::async_runtime::spawn_blocking(move || ts_service_blocking(&action))
        .await
        .map_err(|error| format!("服务控制任务失败: {error}"))?
}

/// 发起后台下载（立即返回；进度/结果走 tailscale-progress / tailscale-done 事件）。
/// 下载完自动弹出平台安装器（Windows 静默参数由安装器自带 UAC；macOS 打开 .pkg）。
#[tauri::command]
pub(crate) fn gateway_tailscale_download(app: AppHandle) -> Result<(), String> {
    if !platform_has_installer() {
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

/// 打开 Tailscale GUI（macOS 专属：App 版 CLI 连不上 GUI 时的一键自救）。
/// 起子进程会阻塞 → async + spawn_blocking（blocking_guard 约定）。
#[tauri::command]
pub(crate) async fn gateway_tailscale_launch_gui() -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("该操作仅 macOS 需要（其它平台没有 GUI 常驻这一层）".to_string());
    }
    tauri::async_runtime::spawn_blocking(|| {
        std::process::Command::new("open")
            .args(["-a", "Tailscale"])
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("打开 Tailscale.app 失败: {error}"))
    })
    .await
    .map_err(|error| format!("任务失败: {error}"))?
}

/// Funnel 通道状态（区块 B 轮询；CLI 子进程阻塞 → async + spawn_blocking）。
#[tauri::command]
pub(crate) async fn gateway_tailscale_funnel_status() -> TailscaleFunnelStatus {
    tauri::async_runtime::spawn_blocking(funnel_probe_blocking)
        .await
        .unwrap_or_else(|_| TailscaleFunnelStatus::new("error"))
}

/// 开启 Funnel（幂等：已开启直接回 ready；失败时 needs_authorize 带 setupUrl 引导）。
/// CLI 子进程阻塞 → async + spawn_blocking；AtomicBool 防连点重入。
#[tauri::command]
pub(crate) async fn gateway_tailscale_funnel_enable(
    app: AppHandle,
) -> Result<TailscaleFunnelStatus, String> {
    if TS_FUNNEL_IN_FLIGHT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Ok(TailscaleFunnelStatus::new("starting"));
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let status = funnel_enable_blocking(&app);
        TS_FUNNEL_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
        status
    })
    .await;
    TS_FUNNEL_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
    result.map_err(|error| format!("Funnel 任务失败: {error}"))
}

/// 关闭 Funnel（幂等；`funnel reset` 只清 funnel 不动 serve/VPN）。
#[tauri::command]
pub(crate) async fn gateway_tailscale_funnel_disable() -> Result<TailscaleFunnelStatus, String> {
    tauri::async_runtime::spawn_blocking(funnel_disable_blocking)
        .await
        .map_err(|error| format!("Funnel 任务失败: {error}"))
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
    fn plausible_ipv4_rejects_non_ip_text() {
        // 实机踩过的注入：CLI 报错文本被当 ip 拼进直连地址
        assert!(!plausible_ipv4(
            "The Tailscale GUI failed to start: The operation couldn't be completed."
        ));
        assert!(!plausible_ipv4(""));
        assert!(!plausible_ipv4("100.119.234")); // 3 段
        assert!(!plausible_ipv4("100.119.234.95.1")); // 5 段
        assert!(!plausible_ipv4("a.b.c.d"));
        assert!(!plausible_ipv4("999.1.1.1")); // 超 255
        assert!(!plausible_ipv4("100.119.234.-1")); // 负号
        assert!(!plausible_ipv4("100.119.2349.95")); // 段超 3 位
    }

    #[test]
    fn plausible_ipv4_accepts_real_shapes() {
        assert!(plausible_ipv4("100.119.234.95"));
        assert!(plausible_ipv4(" 100.64.0.1\n")); // 带空白也行（外层已 trim 过一遍，双保险）
        assert!(plausible_ipv4("0.0.0.0"));
        assert!(plausible_ipv4("255.255.255.255"));
    }

    #[test]
    fn gui_failed_output_detected() {
        assert!(is_gui_failed_output(
            "The Tailscale GUI failed to start: The operation couldn't be completed. (Tailscale.CLIError error 3.)\n"
        ));
        assert!(is_gui_failed_output("xxx (Tailscale.CLIError error 3.)"));
        assert!(!is_gui_failed_output("100.119.234.95"));
        assert!(!is_gui_failed_output(""));
    }

    #[test]
    fn first_line_takes_first_nonempty_trimmed() {
        assert_eq!(first_line("100.1.2.3\nother\n"), "100.1.2.3");
        assert_eq!(first_line("  10.0.0.1  "), "10.0.0.1");
        assert_eq!(first_line(""), "");
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

    /// 按 pkgs.tailscale.com/stable/?mode=json 实测结构造的样例（1.104.2）。
    fn manifest_fixture() -> serde_json::Value {
        serde_json::json!({
            "Exes": [
                "tailscale-setup-1.104.2.exe",
                "tailscale-setup-full-1.104.2.exe"
            ],
            "MSIs": {
                "amd64": "tailscale-setup-1.104.2-amd64.msi",
                "arm64": "tailscale-setup-1.104.2-arm64.msi",
                "x86": "tailscale-setup-1.104.2-x86.msi"
            },
            "MacZips": {
                "universal": "Tailscale-1.104.2-macos.zip",
                "universal-package": "Tailscale-1.104.2-macos.pkg"
            },
            "Version": "1.104.2"
        })
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn manifest_picks_plain_setup_exe_on_windows() {
        let name = pick_manifest_filename(&manifest_fixture()).unwrap();
        assert_eq!(name, "tailscale-setup-1.104.2.exe");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn manifest_picks_universal_pkg_on_macos() {
        let name = pick_manifest_filename(&manifest_fixture()).unwrap();
        assert_eq!(name, "Tailscale-1.104.2-macos.pkg");
    }

    #[test]
    fn manifest_missing_fields_falls_back_to_none() {
        assert_eq!(pick_manifest_filename(&serde_json::json!({})), None);
        // 只有 -full 变体时也视为不可用（full 安装器不走 /quiet 语义）
        let only_full = serde_json::json!({ "Exes": ["tailscale-setup-full-1.104.2.exe"] });
        assert_eq!(pick_manifest_filename(&only_full), None);
    }
}
