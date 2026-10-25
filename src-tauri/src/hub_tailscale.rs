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
    /// tailscaled 系统服务状态：1=运行中 / 0=已停止 / -1=查不了。
    /// 两级开关的副开关数据源；state=unknown 且 installed 时多半也是服务没跑。
    /// Windows 走 sc query；Linux 走 systemctl is-active tailscaled（is-active 是
    /// 只读查询，非 root 可用）；macOS 恒 -1（App 常驻，没有独立服务这一层）。
    #[serde(default)]
    pub service_running: i8,
    /// 是否已保存 AuthKey（仅告知前端「已配置」，密钥本身不回传）。
    #[serde(default)]
    pub authkey_set: bool,
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
            authkey_set: false,
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

/// 提权路径超时：pkexec 会等 polkit 授权框里的人工确认，远超普通 CLI 耗时。
const ELEVATED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// up/down 主开关专用超时：needs_login 时 `tailscale up` 会阻塞等浏览器
/// 授权完成，通用 5s 必然误杀（实测被误报成 "failed to spawn tailscale"）。
/// 给足 120s 完成授权；超时时部分输出（含登录链接）会被带回用于提示。
const TOGGLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

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
    run_command_with_timeout_for(command, CLI_TIMEOUT)
}

/// 子进程执行结果：Done = 正常退出（含非零码）；TimedOut = 超时被杀，但
/// 已捕获的部分输出一并带回（`tailscale up` 等浏览器授权时会把登录链接
/// 打到 stderr，超时杀掉前这些内容对用户有用——不能像以前那样直接丢弃）。
enum CliOutcome {
    Done(bool, String, String),
    TimedOut(String, String),
}

/// 流式输出的行回调（Arc 跨两个读线程共享；ts_log 自带锁，线程安全）。
type LineSink = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// 起一个读线程：逐行 lossy 读（lines() 遇非法 UTF-8 会中断，丢后续输出；
/// 原实现 read_to_end + from_utf8_lossy 的容错语义要保持），每行过一遍
/// 回调后累积进字符串，join 时整段带回。
fn read_pipe(
    pipe: impl std::io::Read + Send + 'static,
    on_line: &Option<LineSink>,
) -> std::thread::JoinHandle<String> {
    let on_line = on_line.clone();
    std::thread::spawn(move || {
        use std::io::{BufReader, BufRead};
        let mut text = String::new();
        let mut reader = BufReader::new(pipe);
        let mut raw = Vec::new();
        loop {
            raw.clear();
            match reader.read_until(b'\n', &mut raw) {
                Ok(0) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&raw);
                    let line = line.trim_end_matches(['\n', '\r']);
                    if let Some(sink) = &on_line {
                        sink(line);
                    }
                    text.push_str(line);
                    text.push('\n');
                }
                Err(_) => break,
            }
        }
        text
    })
}

/// spawn → 并发读 → 超时兜底杀进程的通用执行器（提权路径复用，超时可调）。
/// `on_line` 传 Some 时每捕获一行实时回调（开关路径用：up 等浏览器授权
/// 长达两分钟，结束时才一次性带回会让 UI 全程静默、登录链接也出不来）。
/// spawn 失败 = Err；区分超时与 spawn 失败的教训见 run_cli_full_timeout。
fn run_command_outcome_impl(
    mut command: std::process::Command,
    timeout: std::time::Duration,
    on_line: Option<LineSink>,
) -> Result<CliOutcome, std::io::Error> {
    command.stdin(std::process::Stdio::null());
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        std::io::Error::other("tailscale CLI stdout 管道缺失")
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        std::io::Error::other("tailscale CLI stderr 管道缺失")
    })?;
    let out_reader = read_pipe(stdout, &on_line);
    let err_reader = read_pipe(stderr, &on_line);
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = out_reader.join().unwrap_or_default();
                let err = err_reader.join().unwrap_or_default();
                return Ok(CliOutcome::Done(status.success(), out, err));
            }
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let out = out_reader.join().unwrap_or_default();
                let err = err_reader.join().unwrap_or_default();
                return Ok(CliOutcome::TimedOut(out, err));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(e) => return Err(e),
        }
    }
}

fn run_command_outcome(
    command: std::process::Command,
    timeout: std::time::Duration,
) -> Result<CliOutcome, std::io::Error> {
    run_command_outcome_impl(command, timeout, None)
}

fn run_command_with_timeout_for(
    command: std::process::Command,
    timeout: std::time::Duration,
) -> Option<(bool, String, String)> {
    match run_command_outcome(command, timeout) {
        Ok(CliOutcome::Done(ok, stdout, stderr)) => Some((ok, stdout, stderr)),
        _ => None, // 兼容旧语义：超时与 spawn 失败都收敛成 None
    }
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
/// Windows= sc query；Linux= systemctl is-active（只读查询非 root 可用）；
/// macOS 恒 -1（App 常驻、Linux 之外的差异不值得多维护一层）。
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

#[cfg(target_os = "linux")]
fn service_running_state_impl() -> i8 {
    let Ok(output) = std::process::Command::new("systemctl")
        .args(["is-active", "tailscaled"])
        .output()
    else {
        return -1;
    };
    match String::from_utf8_lossy(&output.stdout).trim() {
        "active" => 1,
        "inactive" | "failed" => 0,
        // "activating"/"unknown" 或非 systemd 系统（is-active 会打印报错并 exit 非 0）
        _ => -1,
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn service_running_state_impl() -> i8 {
    -1
}

/// `tailscale up` 报「changing settings via 'tailscale up' requires mentioning
/// all」= 机器上已有非默认偏好（比如之前手动跑过带旗标的 up），此时必须把
/// 全部非默认旗标带上才能改设置。报错里会列出所需旗标（固定模板：4 空格
/// 缩进的 `--flag` 行）—— 解析出来自动补上重试，保留用户既有设置。
fn is_up_flags_mismatch(stderr: &str) -> bool {
    stderr.contains("changing settings via 'tailscale up' requires mentioning all")
}

fn parse_up_required_flags(stderr: &str) -> Vec<String> {
    let mut flags = Vec::new();
    for line in stderr.lines() {
        // 只认 4 空格缩进的旗标行（官方错误模板固定格式），避免误抓正文里的 --word
        if let Some(rest) = line.strip_prefix("    --") {
            let tok = format!("--{}", rest.split_whitespace().next().unwrap_or(""));
            if tok.len() > 2 && !flags.contains(&tok) {
                flags.push(tok);
            }
        }
    }
    flags
}

/// 阻塞版主开关：`tailscale up / down`（登录级，不动系统服务）。
/// NeedsLogin 时 `up` 会拉起浏览器登录流程（这正是想要的语义）——用
/// TOGGLE_TIMEOUT 长超时等授权，不能用 5s 通用档（实测误报 spawn 失败）。
/// prefs 变更是本地权限闸门管的操作 → 走提权入口（Linux 被拒时 pkexec 弹框）。
pub(crate) fn ts_toggle_blocking(app: &AppHandle, up: bool) -> Result<TailscaleStatus, String> {
    let cli = find_cli().ok_or_else(|| "没检测到 Tailscale，请先安装".to_string())?;
    ts_log(if up { "[开关] tailscale up…" } else { "[开关] tailscale down…" });
    let mut args: Vec<String> = vec![if up { "up" } else { "down" }.to_string()];
    // AuthKey（可选）：走 0600 密钥文件 + `--auth-key=file:` 形式，密钥不进 argv
    // （ps 可见）。填了就等于免浏览器登录，与内置引擎的 AuthKey 同语义。
    if up {
        if let Some(reference) = ts_authkey_reference(app) {
            args.push(format!("--auth-key={reference}"));
            ts_log("[开关] 使用已保存的 AuthKey 连接（免浏览器登录）");
        }
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    // 实时行回调：up 等浏览器授权最长 120s，结束时才带回输出 = UI 全程静默、
    // 登录链接出不来（实机踩过）。CLI 每行即时上屏，但登录相关行（"To
    // authenticate, visit:" 和链接本体）不进日志——链接由前端登录卡片独占
    // 展示（与内置引擎一致），日志里刷链接反而干扰。同一 URL 只 emit 一次。
    let last_url = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let sink: LineSink = {
        let last_url = last_url.clone();
        let app = app.clone();
        std::sync::Arc::new(move |line: &str| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return;
            }
            let is_login_url = trimmed.starts_with("https://login.tailscale.com/");
            let is_login_hint = trimmed.contains("To authenticate");
            if !is_login_url && !is_login_hint {
                ts_log(&format!("[CLI] {trimmed}"));
            }
            if is_login_url {
                let is_new = {
                    let mut last = last_url
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if *last != trimmed {
                        *last = trimmed.to_string();
                        true
                    } else {
                        false
                    }
                };
                if is_new {
                    let _ = app.emit(
                        EVENT_TS_LOGIN_URL,
                        serde_json::json!({ "url": trimmed }),
                    );
                }
            }
        })
    };
    let (mut ok, mut stdout, mut stderr) =
        run_cli_privileged_sink(&cli, &refs, TOGGLE_TIMEOUT, Some(sink.clone()));
    // 既有偏好的旗标清单没带全 → 按报错提示自动补齐再试一次
    if up && !ok && is_up_flags_mismatch(&stderr) {
        let flags = parse_up_required_flags(&stderr);
        if !flags.is_empty() {
            ts_log(&format!(
                "[开关] 检测到既有偏好设置，按提示自动补旗标重试：{}",
                flags.join(" ")
            ));
            args.extend(flags);
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let retried = run_cli_privileged_sink(&cli, &refs, TOGGLE_TIMEOUT, Some(sink.clone()));
            ok = retried.0;
            stdout = retried.1;
            stderr = retried.2;
        }
    }
    if !ok {
        let detail = stderr.trim().lines().next().unwrap_or("").trim();
        let fallback = if up { "tailscale up 失败" } else { "tailscale down 失败" };
        let mut detail = if detail.is_empty() && stdout.trim().is_empty() {
            fallback.to_string()
        } else if detail.is_empty() {
            stdout.trim().lines().next().unwrap_or(fallback).trim().to_string()
        } else {
            detail.to_string()
        };
        // up 已把浏览器登录链接打出来了 → 这是「等授权」不是普通失败：
        // 整段保留（链接可点），别只取第一行把 URL 掐掉（实机踩过：只剩
        // "To authenticate, visit:"，用户以为提权挂了）
        let combined = format!("{stderr}\n{stdout}");
        if combined.contains("To authenticate, visit") {
            detail = format!("{} —— 在浏览器完成授权后再点一次开关即可", combined.trim());
        } else if stderr.contains("[等待超时") {
            // 等浏览器授权超时：标注在 stderr 末尾，前面通常还有 CLI 打出的登录链接，
            // 整段给用户（链接可直接点）
            detail = format!(
                "{} —— 若在等浏览器登录授权，完成授权后再点一次开关",
                stderr.trim()
            );
        }
        // tailscaled 服务没跑是 Linux 上最常见的坑（尤其容器/最小安装没 enable）：
        // CLI 原始报错对用户没有可操作性，翻译成下一步动作
        if detail.contains("failed to connect to local tailscaled") {
            detail = "tailscaled 服务未运行 —— 先点上方副开关启动它，或终端执行 \
                      sudo systemctl enable --now tailscaled"
                .to_string();
        }
        // 提权没走通（无 pkexec / 用户取消授权框）：把手动兜底命令一并给用户。
        // 取消场景 pkexec 报 "Not authorized" 不含 "access denied"，
        // 匹配不上闸门文案 → 靠 elevated_retry 打的「提权未完成」标记补位
        if is_local_access_denied(&stdout, &stderr) || stderr.contains("提权未完成") {
            detail.push('；');
            detail.push_str(&local_privilege_hint());
        }
        ts_log(&format!("[开关] 失败：{detail}"));
        return Err(detail);
    }
    ts_log("[开关] 完成");
    Ok(probe(&cli))
}

/// 阻塞版服务控制：action = start / stop / restart。
/// 返回控制后的服务状态（1/0/-1）。启停需要特权（Windows 管理员 / Linux polkit
/// 授权），被拒时原样回显系统错误。
pub(crate) fn ts_service_blocking(action: &str) -> Result<i8, String> {
    service_action_blocking(action)
}

#[cfg(windows)]
fn service_action_blocking(action: &str) -> Result<i8, String> {
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

/// Linux 服务级启停走 systemctl（桌面会话里 polkit 会弹授权框，确认即可；
/// 无桌面/被拒时把错误翻译成人话）。is-active 回读确认。
#[cfg(target_os = "linux")]
fn service_action_blocking(action: &str) -> Result<i8, String> {
    let args: Vec<&str> = match action {
        "start" => vec!["start", "tailscaled"],
        "stop" => vec!["stop", "tailscaled"],
        "restart" => vec!["restart", "tailscaled"],
        _ => return Err(format!("未知的服务操作：{action}")),
    };
    let output = std::process::Command::new("systemctl")
        .args(&args)
        .output()
        .map_err(|error| format!("执行 systemctl 失败: {error}"))?;
    if !output.status.success() {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let detail = text.trim().lines().next().unwrap_or("").trim();
        if detail.contains("Interactive authentication") || detail.contains("认证") {
            return Err(
                "需要系统授权：请在桌面弹出的授权框确认，或终端执行 sudo systemctl \
                 start tailscaled"
                    .to_string(),
            );
        }
        if detail.contains("not been booted with systemd") {
            return Err("此系统未使用 systemd，无法在应用内管理 tailscaled 服务".to_string());
        }
        return Err(if detail.is_empty() {
            "服务操作失败".to_string()
        } else {
            detail.to_string()
        });
    }
    Ok(service_running_state())
}

/// macOS：Tailscale.app 常驻代管 tailscaled，没有独立服务层可启停。
#[cfg(target_os = "macos")]
fn service_action_blocking(_action: &str) -> Result<i8, String> {
    Err("macOS 由 Tailscale.app 常驻管理服务，无需（也无法）在应用内启停".to_string())
}

// ==================== 下载安装包 ====================

const EVENT_TS_PROGRESS: &str = "tailscale-progress";
const EVENT_TS_DONE: &str = "tailscale-done";
/// 直连开关等待授权时提取到的登录链接（hub.html 引导行显示可点击 URL）。
const EVENT_TS_LOGIN_URL: &str = "tailscale-login-url";

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

// ==================== 运行日志环 ====================
// 对齐 gateway 通道日志的形态：内存环形缓冲，「日志」弹窗轮询消费。
// 记录下载 / 安装 / 开关 / Funnel 的里程碑与关键输出（含提权重试过程），
// 排障时不用再让用户复现操作来抓 toast——弹窗里全程可回看。

const TS_LOG_CAP: usize = 400;

static TS_LOG: std::sync::OnceLock<std::sync::Mutex<std::collections::VecDeque<String>>> =
    std::sync::OnceLock::new();

fn ts_log_ring() -> &'static std::sync::Mutex<std::collections::VecDeque<String>> {
    TS_LOG.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::new()))
}

/// 追加一行日志（满了丢最老的；锁坏了静默丢弃——日志绝不能反过来卡主流程）。
fn ts_log(line: &str) {
    if let Ok(mut log) = ts_log_ring().lock() {
        if log.len() >= TS_LOG_CAP {
            log.pop_front();
        }
        log.push_back(line.to_string());
    }
}

/// 拉取日志快照（UI「日志」弹窗消费；纯内存读）。
#[tauri::command]
pub(crate) fn gateway_tailscale_logs() -> Vec<String> {
    ts_log_ring()
        .lock()
        .map(|log| log.iter().cloned().collect())
        .unwrap_or_default()
}

/// 清空日志（UI「清空」按钮：源头清掉，屏幕同步清屏，下次拉取不再恢复）。
#[tauri::command]
pub(crate) fn gateway_tailscale_logs_clear() {
    if let Ok(mut log) = ts_log_ring().lock() {
        log.clear();
    }
}

/// 平台是否有应用内安装渠道（Linux = 官方静态 tarball + systemd 注册，
/// 见 `launch_installer` 的 unix 分支）。
fn platform_has_installer() -> bool {
    cfg!(any(target_os = "windows", target_os = "macos", target_os = "linux"))
}

/// Rust arch 名 → 官方 Tarballs 的架构段名（manifest["Tarballs"] 的 key）。
/// 覆盖官方全部主流段：amd64/arm64/386/arm/riscv64（mips 系与 geode 太边缘不认）。
fn linux_tarball_arch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("amd64"),
        "aarch64" => Some("arm64"),
        "x86" => Some("386"),
        "arm" => Some("arm"),
        "riscv64" => Some("riscv64"),
        _ => None,
    }
}

/// 官方 stable 元数据（https://pkgs.tailscale.com/stable/?mode=json）里挑当前平台安装包文件名。
/// Windows 取 Exes 里的标准 setup exe（支持 /quiet 静默参数；跳过 -full 变体，MSI 直启带不了参）。
/// macOS 取 MacZips 的通用 .pkg。Linux 取 Tarballs 的静态 tgz（tailscale + tailscaled）。
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
        manifest["Tarballs"][linux_tarball_arch()?]
            .as_str()
            .map(str::to_string)
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
    } else if let Some(name) = fetch_stable_manifest().and_then(|manifest| {
        pick_manifest_filename(&manifest)
    }) {
        // Linux：静态 tarball 只有版本化直链，没有 latest 别名可兜底——
        // 清单拉不到就空列表，调用方提示检查网络
        candidates.push((format!("{BASE}/{name}"), "tailscale-linux.tgz"));
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
    // Linux 前置检测不过就直接失败（fail-fast，不浪费 30MB 下载）
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Err(message) = linux_install_preflight() {
        ts_log(&format!("[下载] 前置检测未通过：{message}"));
        finish_ts_download(&app, Err(message));
        return;
    }
    ts_log("[下载] 开始获取版本清单（pkgs.tailscale.com/stable）…");
    let candidates = download_candidates();
    if candidates.is_empty() {
        let message = if platform_has_installer() {
            // 三端都有安装渠道还拿空列表 = 版本清单拉取失败
            "获取 Tailscale 版本清单失败（pkgs.tailscale.com 不可达？），请检查网络后重试".into()
        } else {
            "当前平台没有通用安装包，请在终端用系统包管理器安装 Tailscale（如 apt/brew）".into()
        };
        ts_log("[下载] 失败：拿不到可用直链（清单拉取失败或平台不支持）");
        finish_ts_download(&app, Err(message));
        return;
    }
    let dir = match ts_cache_dir(&app) {
        Ok(dir) => dir,
        Err(error) => {
            ts_log(&format!("[下载] 失败：{error}"));
            finish_ts_download(&app, Err(error));
            return;
        }
    };
    let mut last_error = String::new();
    for (url, filename) in candidates {
        let tmp = dir.join(filename);
        ts_log(&format!("[下载] {url}"));
        let _ = std::fs::remove_file(&tmp);
        match crate::hub_gateway::download_to_tmp(&url, &tmp) {
            Ok((received, total)) => {
                ts_log(&format!(
                    "[下载] 完成：{} MB → {}",
                    received / 1048576,
                    tmp.display()
                ));
                let _ = app.emit(
                    EVENT_TS_PROGRESS,
                    TsDownloadProgress { received, total, percent: 100 },
                );
                finish_ts_download(&app, Ok(tmp));
                return;
            }
            Err(error) => {
                ts_log(&format!("[下载] 失败：{error}，换下一个候选…"));
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
    run_cli_full_timeout(cli, args, CLI_TIMEOUT)
}

/// 超时可调的完整执行：把「超时」与「起不来」分开翻成人话 —— 超时时把
/// 已捕获的部分输出原样带回（`up` 等浏览器授权时会把登录链接打到 stderr），
/// 并追加超时标注。（教训：needs_login 的 `up` 阻塞等授权被 5s 通用超时
/// 杀掉后，None 被误报成 "failed to spawn tailscale"，用户无从下手。）
fn run_cli_full_timeout(
    cli: &std::path::Path,
    args: &[&str],
    timeout: std::time::Duration,
) -> (bool, String, String) {
    run_cli_full_timeout_sink(cli, args, timeout, None)
}

/// 同上，带实时行回调（开关路径用，见 run_command_outcome_impl）。
fn run_cli_full_timeout_sink(
    cli: &std::path::Path,
    args: &[&str],
    timeout: std::time::Duration,
    on_line: Option<LineSink>,
) -> (bool, String, String) {
    let mut command = cli_command(cli);
    command.args(args);
    match run_command_outcome_impl(command, timeout, on_line) {
        Ok(CliOutcome::Done(ok, stdout, stderr)) => (ok, stdout, stderr),
        Ok(CliOutcome::TimedOut(out, mut err)) => {
            if !err.trim().is_empty() {
                err.push('\n');
            }
            err.push_str(&format!("[等待超时 {} 秒]", timeout.as_secs()));
            (false, out, err)
        }
        Err(e) => (false, String::new(), format!("无法启动 tailscale CLI（{e}）")),
    }
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

/// 当前桌面用户名（operator 提示用；Linux 取 USER，Windows 兜底 USERNAME）。
fn current_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}

/// tailscaled 的本地权限闸门：prefs / serve 配置变更只允许 root / operator，
/// 非特权连接报「Access denied: prefs write access denied」/「serve config
/// denied」——文案看着像 tailnet 策略拒绝，其实是本机权限不够（tailnet 策略的
/// nodeAttrs 是另一道独立的门）。实机踩过：同一策略下 sudo 一跑就过。
fn is_local_access_denied(stdout: &str, stderr: &str) -> bool {
    format!("{stdout}\n{stderr}")
        .to_ascii_lowercase()
        .contains("access denied")
}

/// 提权没走通时的手动兜底提示（含精确命令，用户名自动探测）。
fn local_privilege_hint() -> String {
    let user = current_username();
    if cfg!(windows) {
        "本操作需要本机特权：请以管理员身份运行 AnyBoot 后重试".to_string()
    } else if user.is_empty() {
        "本操作需要本机特权：终端执行一次 sudo tailscale set --operator=$(whoami) \
         即可（授权后 AnyBoot 免提权管理 Tailscale/Funnel）"
            .to_string()
    } else {
        format!(
            "本操作需要本机特权：终端执行一次 sudo tailscale set --operator={user} \
             即可（授权后 AnyBoot 免提权管理 Tailscale/Funnel）"
        )
    }
}

/// Linux 提权重试：本地权限闸门拒绝时用 pkexec（polkit 弹桌面授权框）。
/// 首选一次性把当前用户设为 operator——授权一次终身有效，之后所有变更类
/// CLI 调用都免提权；operator 设不上（无 polkit agent / 用户取消）则用
/// pkexec 直接跑原命令。pkexec 不存在等 spawn 失败 → None，调用方维持
/// 原报错并展示手动兜底提示。超时放宽：授权框等人确认可能远超 5s。
#[cfg(target_os = "linux")]
fn elevated_retry(
    cli: &std::path::Path,
    args: &[&str],
    timeout: std::time::Duration,
    on_line: Option<LineSink>,
) -> Option<(bool, String, String)> {
    let user = current_username();
    if !user.is_empty() {
        ts_log(&format!(
            "[提权] 本地权限被拒 → pkexec 设置 operator={user}（授权一次终身免提权）…"
        ));
        let mut operator = std::process::Command::new("pkexec");
        operator.arg(cli).arg("set").arg(format!("--operator={user}"));
        if run_command_with_timeout_for(operator, ELEVATED_TIMEOUT)
            .map(|(ok, _, _)| ok)
            .unwrap_or(false)
        {
            ts_log("[提权] operator 已设置，重试原命令…");
            ts_log("[提权] 若拉起浏览器登录，完成授权后会自动继续，不用再点开关");
            // 重试沿用调用方的超时档（up 等浏览器授权要长超时，不能用 5s 档——
            // 超时会落进下面的 pkexec 直跑分支，再弹一次授权框，平白添乱）；
            // 流式回调继续带上——登录链接要实时上屏
            let retried = run_cli_full_timeout_sink(cli, args, timeout, on_line);
            return Some(retried);
        } else {
            ts_log("[提权] operator 设置未成功（取消 / 无 polkit agent），回退直接提权跑原命令");
        }
    }
    let mut direct = std::process::Command::new("pkexec");
    direct.arg(cli).args(args);
    let (ok, stdout, mut stderr) = run_command_with_timeout_for(direct, ELEVATED_TIMEOUT)?;
    if !ok {
        // 用户取消授权框 / 桌面没有 polkit agent 时，pkexec 自己报
        // "Not authorized"（或干脆无输出）——不含 "access denied"，
        // 调用方的闸门文案匹配不上 → 打标记，让调用方追加手动兜底命令
        let text = format!("{stdout}\n{stderr}").to_ascii_lowercase();
        if text.contains("not authorized")
            || text.contains("authentication agent")
            || text.trim().is_empty()
        {
            stderr.push_str("\n[提权未完成：授权框被取消或桌面无 polkit agent]");
        }
    }
    Some((ok, stdout, stderr))
}

/// 变更类 CLI 调用（up/down/funnel）统一入口：先按当前用户跑，被本地权限
/// 闸门拒绝时 Linux 上自动提权重试。成功路径零额外开销；失败回执多一次
/// 快速往返（CLI 几十 ms）才决定要不要弹授权框。
fn run_cli_privileged(
    cli: &std::path::Path,
    args: &[&str],
    timeout: std::time::Duration,
) -> (bool, String, String) {
    run_cli_privileged_sink(cli, args, timeout, None)
}

/// 同上，带实时行回调（开关路径：登录链接等输出要即时上屏）。
fn run_cli_privileged_sink(
    cli: &std::path::Path,
    args: &[&str],
    timeout: std::time::Duration,
    on_line: Option<LineSink>,
) -> (bool, String, String) {
    let result = run_cli_full_timeout_sink(cli, args, timeout, on_line.clone());
    if result.0 {
        return result;
    }
    #[cfg(target_os = "linux")]
    {
        if is_local_access_denied(&result.1, &result.2) {
            if let Some(retried) = elevated_retry(cli, args, timeout, on_line) {
                return retried;
            }
        }
    }
    result
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
    let port_arg = port.to_string();
    ts_log(&format!("[Funnel] tailscale funnel --bg {port}…"));
    let (ok, stdout, stderr) = run_cli_privileged(&cli, &["funnel", "--bg", &port_arg], CLI_TIMEOUT);
    if !ok {
        let (state, code, setup_url) = classify_funnel_error(&stderr, &stdout);
        let mut status = TailscaleFunnelStatus::new(&state);
        status.error_code = code;
        status.setup_url = setup_url;
        let detail = stderr.trim().lines().next().unwrap_or("").trim();
        status.message = if format!("{stderr}\n{stdout}").contains("serve config denied") {
            // 本地权限闸门（root/operator）且提权没走通——给手动兜底命令
            local_privilege_hint()
        } else if detail.is_empty() {
            "Funnel 开启失败".into()
        } else {
            format!("Funnel 开启失败：{detail}")
        };
        ts_log(&format!("[Funnel] 开启失败：{}", status.message));
        return status;
    }
    // CLI 成功后再确认一次配置真的落上了
    if funnel_configured(&cli) {
        ts_log("[Funnel] 已开启");
        funnel_probe_blocking()
    } else {
        ts_log("[Funnel] 命令成功但配置未生效");
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
    ts_log("[Funnel] tailscale funnel reset…");
    let _ = run_cli_privileged(&cli, &["funnel", "reset"], CLI_TIMEOUT);
    if funnel_configured(&cli) {
        ts_log("[Funnel] 关闭失败：配置仍在");
        let mut status = TailscaleFunnelStatus::new("error");
        status.error_code = "funnel_stop_failed".into();
        status.message = "Funnel 关闭失败，请重试".into();
        return status;
    }
    ts_log("[Funnel] 已关闭");
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
    let app = app.clone();
    std::thread::spawn(move || {
        let before = probe_blocking();
        if !before.installed || before.state == "running" {
            return;
        }
        // 服务没跑（Windows tailscaled）先拉起来：up 是对着服务说话的，
        // 服务停着怎么 up 都没用。没管理员权限这里会失败，静默跳过即可。
        if before.service_running == 0 {
            let _ = ts_service_blocking("start");
        }
        let _ = ts_toggle_blocking(&app, true);
    });
}

/// 查询本机 Tailscale 状态（找不到 CLI → installed:false）。
/// CLI 子进程有几十 ms 阻塞，走 async + spawn_blocking（blocking_guard 约定）。
#[tauri::command]
pub(crate) async fn gateway_tailscale_status(app: AppHandle) -> TailscaleStatus {
    let key_set = !crate::stored_settings(&app).tailscale_authkey.trim().is_empty();
    let mut status = tauri::async_runtime::spawn_blocking(probe_blocking)
        .await
        .unwrap_or_default();
    status.authkey_set = key_set;
    status
}

static TS_TOGGLE_IN_FLIGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 主开关：tailscale up / down（登录级；NeedsLogin 时 up 拉起浏览器登录）。
/// CLI 子进程阻塞 → async + spawn_blocking；AtomicBool 防连点重入。
#[tauri::command]
pub(crate) async fn gateway_tailscale_toggle(
    app: tauri::AppHandle,
    up: bool,
) -> Result<TailscaleStatus, String> {
    if TS_TOGGLE_IN_FLIGHT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("上一次开关操作还在执行，稍等一下".to_string());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let status = ts_toggle_blocking(&app, up);
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
/// 下载完自动走安装流程（Windows 静默 exe 自带 UAC；macOS 打开 .pkg；
/// Linux pkexec 提权解压静态二进制并注册 systemd 服务）。
#[tauri::command]
pub(crate) fn gateway_tailscale_download(app: AppHandle) -> Result<(), String> {
    if !platform_has_installer() {
        return Err("当前平台不支持应用内安装，请用系统包管理器安装 Tailscale".to_string());
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
pub(crate) async fn gateway_tailscale_install(app: AppHandle, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = std::path::PathBuf::from(&path);
        if !path.is_file() {
            return Err("安装包不存在，请重新下载".to_string());
        }
        launch_installer(&path)?;
        // 安装记录：卸载功能据此判定「是通过 AnyBoot 装的」才允许应用内卸载
        write_install_marker(&app)
    })
    .await
    .map_err(|error| format!("安装任务失败: {error}"))?
}

/// 安装记录文件（数据目录下）。AnyBoot 的安装流程跑完才写；没有它就说明
/// Tailscale 是用户自己装的，应用内卸载会拒绝并给手动卸载指引。
fn ts_install_marker(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("定位数据目录失败: {error}"))?
        .join("tailscale.anyboot-install"))
}

/// AuthKey 密钥文件（数据目录，0600）。AuthKey 是敏感凭据，不走命令行参数。
fn ts_authkey_file(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("定位数据目录失败: {error}"))?
        .join("tailscale-authkey"))
}

/// 把设置里存的 AuthKey 落成 0600 文件，返回 CLI 的 `file:<路径>` 引用；
/// 没配置返回 None（走浏览器登录）。密钥不进 argv，ps 看不到。
fn ts_authkey_reference(app: &AppHandle) -> Option<String> {
    let key = crate::stored_settings(app).tailscale_authkey.trim().to_string();
    if key.is_empty() {
        return None;
    }
    let path = ts_authkey_file(app).ok()?;
    if std::fs::write(&path, &key).is_err() {
        ts_log("[开关] AuthKey 写入密钥文件失败，回退浏览器登录");
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Some(format!("file:{}", path.display()))
}

/// 保存 / 清除系统 Tailscale 的 AuthKey（空串 = 清除）。
/// 与内置引擎的 AuthKey 同语义：填了连接时免浏览器登录。
#[tauri::command]
pub(crate) fn gateway_tailscale_set_authkey(
    app: AppHandle,
    key: String,
) -> Result<(), String> {
    let key = key.trim().to_string();
    let clearing = key.is_empty();
    crate::settings_commands::update_settings_and_publish(
        &app,
        "tailscale_set_authkey",
        if clearing {
            "Tailscale 直连：清除 AuthKey"
        } else {
            "Tailscale 直连：保存 AuthKey"
        },
        move |settings| settings.tailscale_authkey = key.clone(),
        |_| {},
    )?;
    // 同步密钥文件：有则写 0600，清空则删掉，避免残留旧密钥
    if let Ok(path) = ts_authkey_file(&app) {
        if clearing {
            let _ = std::fs::remove_file(&path);
        } else if let Some(reference) = ts_authkey_reference(&app) {
            ts_log(&format!("[凭据] AuthKey 已保存（{}）", reference));
        }
    }
    Ok(())
}

/// 清除登录凭据：`tailscale logout`（断开并清本机凭据，下次连接需重新授权）。
/// 与内置引擎的「清除登录凭据」同语义；管理台里的设备记录需用户自行移除。
#[tauri::command]
pub(crate) async fn gateway_tailscale_logout(
    app: AppHandle,
) -> Result<TailscaleStatus, String> {
    let key_set = !crate::stored_settings(&app).tailscale_authkey.trim().is_empty();
    tauri::async_runtime::spawn_blocking(move || {
        let cli = find_cli().ok_or_else(|| "没检测到 Tailscale，请先安装".to_string())?;
        ts_log("[凭据] tailscale logout…");
        let (ok, stdout, stderr) =
            run_cli_privileged(&cli, &["logout"], std::time::Duration::from_secs(30));
        if !ok {
            let detail = format!("{stdout}\n{stderr}");
            let detail = detail.trim();
            return Err(if detail.is_empty() {
                "清除登录凭据失败（用户取消授权？）".to_string()
            } else {
                format!("清除登录凭据失败：{detail}")
            });
        }
        ts_log("[凭据] 已清除本机登录凭据 —— 下次连接需重新授权");
        let mut status = probe(&cli);
        status.authkey_set = key_set;
        Ok(status)
    })
    .await
    .map_err(|error| format!("清除凭据任务失败: {error}"))?
}

fn write_install_marker(app: &AppHandle) -> Result<(), String> {
    let marker = ts_install_marker(app)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::fs::write(&marker, format!("installed-by=anyboot\nat={stamp}\n"))
        .map_err(|error| format!("写安装记录失败: {error}"))
}

/// 「不是 AnyBoot 装的」拒绝文案：按平台给手动卸载指引。
fn manual_uninstall_hint() -> String {
    let why = "Tailscale 不是通过 AnyBoot 安装的（没有安装记录）——AnyBoot 不动它，请自行卸载：";
    if cfg!(target_os = "windows") {
        format!("{why}设置 → 应用 → 已安装的应用 里卸载 Tailscale")
    } else if cfg!(target_os = "macos") {
        format!("{why}把 /Applications/Tailscale.app 拖进废纸篓")
    } else {
        format!("{why}用系统包管理器卸载（如 sudo pacman -R tailscale、sudo apt remove tailscale）")
    }
}

/// Linux 安装指纹：`/usr/local/bin` 双二进制 + ExecStart 指向它的自家 unit +
/// 系统无发行版 unit。三者同时成立 = 只有 AnyBoot 的 tarball 安装脚本会造出
/// 这个形态（发行版包装的 tailscaled 不在 /usr/local/bin，unit 也在 /usr/lib）。
/// 与卸载脚本里的判定条件保持一致。
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_install_fingerprint_matches() -> bool {
    linux_install_fingerprint_at(
        std::path::Path::new("/usr/local/bin/tailscale"),
        std::path::Path::new("/usr/local/bin/tailscaled"),
        std::path::Path::new("/etc/systemd/system/tailscaled.service"),
        std::path::Path::new("/usr/lib/systemd/system/tailscaled.service"),
    )
}

fn linux_install_fingerprint_at(
    bin_cli: &std::path::Path,
    bin_daemon: &std::path::Path,
    unit: &std::path::Path,
    distro_unit: &std::path::Path,
) -> bool {
    let bins = bin_cli.is_file() && bin_daemon.is_file();
    let own_unit = unit.is_file()
        && std::fs::read_to_string(unit)
            .map(|text| text.contains("/usr/local/bin/tailscaled"))
            .unwrap_or(false);
    let no_distro_unit = !distro_unit.exists();
    bins && own_unit && no_distro_unit
}

static TS_UNINSTALL_IN_FLIGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 卸载 Tailscale（仅限 AnyBoot 安装记录存在的情况）。
/// Windows=MSI 注册表卸载串静默跑（UAC 由 msiexec 弹）；macOS=退出 App 后
/// 提权删 .app；Linux=pkexec 脚本（停服务 → 删 /usr/local/bin 二进制 →
/// 只删 AnyBoot 写的 unit——指纹是 ExecStart 指向 /usr/local/bin 且系统没有
/// 发行版 unit，动不了发行版包的东西）。
#[tauri::command]
pub(crate) async fn gateway_tailscale_uninstall(app: AppHandle) -> Result<String, String> {
    if TS_UNINSTALL_IN_FLIGHT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("卸载操作还在执行，稍等一下".to_string());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let result = ts_uninstall_blocking(&app);
        TS_UNINSTALL_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
        result
    })
    .await;
    TS_UNINSTALL_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
    result.map_err(|error| format!("卸载任务失败: {error}"))?
}

fn ts_uninstall_blocking(app: &AppHandle) -> Result<String, String> {
    let marker = ts_install_marker(app)?;
    if !marker.exists() {
        // 安装记录是后期加的：早期 AnyBoot 安装没有留下 marker。Linux 上用安装
        // 指纹兜底——指纹成立即确凿是 AnyBoot 装的，补写记录后照常卸载；不匹配
        // 才按「用户自装」拒绝。Win/mac 没有等价指纹，维持记录一票否决。
        #[cfg(all(unix, not(target_os = "macos")))]
        if linux_install_fingerprint_matches() {
            ts_log("[卸载] 无安装记录，但安装指纹匹配（早期 AnyBoot 安装）→ 补写记录后继续");
            write_install_marker(app)?;
        } else {
            return Err(manual_uninstall_hint());
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        return Err(manual_uninstall_hint());
    }
    if !probe_blocking().installed {
        // 装过但已经不在了（用户自己删过 / 安装没成功）：清掉残留记录
        let _ = std::fs::remove_file(&marker);
        return Err("Tailscale 未安装，无需卸载（残留的安装记录已清掉）".to_string());
    }
    ts_log("[卸载] 开始卸载 AnyBoot 安装的 Tailscale…");
    #[cfg(target_os = "windows")]
    let message = ts_uninstall_windows()?;
    #[cfg(target_os = "macos")]
    let message = ts_uninstall_macos()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    let message = ts_uninstall_linux()?;
    let _ = std::fs::remove_file(&marker);
    ts_log("[卸载] 完成");
    Ok(message)
}

/// Windows：注册表 Uninstall 里找 Tailscale 的 MSI 卸载串（`MsiExec.exe /X{GUID}`），
/// 解出 GUID 后 `msiexec /x {GUID} /quiet /norestart` 静默卸载（UAC 由 msiexec 弹）。
#[cfg(target_os = "windows")]
fn ts_uninstall_windows() -> Result<String, String> {
    let (ok, stdout, _) = run_command_with_timeout_for(
        {
            let mut c = std::process::Command::new("reg");
            c.args([
                "query",
                r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Tailscale",
                "/v",
                "UninstallString",
            ]);
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW（GUI 进程不闪黑框）
            c
        },
        CLI_TIMEOUT,
    )
    .ok_or_else(|| "查询卸载信息失败——请在 设置 → 应用 里手动卸载 Tailscale".to_string())?;
    if !ok {
        return Err("注册表里找不到 Tailscale 的卸载信息——请在 设置 → 应用 里手动卸载".to_string());
    }
    let text = stdout.trim();
    let guid = text
        .split('{')
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .map(|inner| format!("{{{inner}}}"))
        .ok_or_else(|| "卸载信息格式异常——请在 设置 → 应用 里手动卸载 Tailscale".to_string())?;
    ts_log(&format!("[卸载] msiexec /x {guid} /quiet（UAC 确认后执行）…"));
    let mut command = std::process::Command::new("msiexec");
    command.args(["/x", &guid, "/quiet", "/norestart"]);
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW（UAC 弹窗不受影响）
    let (ok, _, stderr) = run_command_with_timeout_for(command, ELEVATED_TIMEOUT)
        .ok_or_else(|| "msiexec 执行超时或被取消——卸载可能未完成，请到 设置 → 应用 确认".to_string())?;
    if !ok {
        let detail = stderr.trim().lines().next().unwrap_or("").trim();
        return Err(if detail.is_empty() {
            "卸载失败（用户取消或权限被拒？）——可到 设置 → 应用 里手动卸载".to_string()
        } else {
            format!("卸载失败：{detail}")
        });
    }
    Ok("Tailscale 已卸载".to_string())
}

/// macOS：退出 Tailscale.app 后提权删除应用本体（pkg 安装位 /Applications/Tailscale.app）。
#[cfg(target_os = "macos")]
fn ts_uninstall_macos() -> Result<String, String> {
    const APP: &str = "/Applications/Tailscale.app";
    if !std::path::Path::new(APP).exists() {
        return Err("找不到 /Applications/Tailscale.app——请手动确认安装位置后自行卸载".to_string());
    }
    // 退出 App（没在跑也没关系）
    let _ = std::process::Command::new("osascript")
        .args(["-e", r#"quit app "Tailscale""#])
        .output();
    ts_log("[卸载] 提权删除 /Applications/Tailscale.app（授权框确认后执行）…");
    let script = format!(r#"do shell script "rm -rf '{APP}'" with administrator privileges"#);
    let (ok, _, stderr) = run_command_with_timeout_for(
        {
            let mut c = std::process::Command::new("osascript");
            c.args(["-e", &script]);
            c
        },
        ELEVATED_TIMEOUT,
    )
    .ok_or_else(|| "卸载超时或被取消——请手动把 Tailscale.app 拖进废纸篓".to_string())?;
    if !ok {
        let detail = stderr.trim().lines().next().unwrap_or("").trim();
        return Err(if detail.is_empty() {
            "卸载失败（用户取消授权？）——可手动把 Tailscale.app 拖进废纸篓".to_string()
        } else {
            format!("卸载失败：{detail}")
        });
    }
    Ok("Tailscale 已卸载".to_string())
}

/// Linux：pkexec 脚本停服务 → 删 /usr/local/bin 二进制 → 只删 AnyBoot 写的 unit
/// 及其 `.d` drop-in 目录（指纹 = ExecStart 指向 /usr/local/bin/tailscaled 且系统
/// 没有发行版 unit，动不了发行版包的东西）。drop-in 不清会在重装时盖掉新 ExecStart。
#[cfg(all(unix, not(target_os = "macos")))]
fn ts_uninstall_linux() -> Result<String, String> {
    let script = "\
set +e
systemctl disable --now tailscaled >/dev/null 2>&1
rm -f /usr/local/bin/tailscale /usr/local/bin/tailscaled
if [ -f /etc/systemd/system/tailscaled.service ] && \
   grep -q '/usr/local/bin/tailscaled' /etc/systemd/system/tailscaled.service && \
   [ ! -f /usr/lib/systemd/system/tailscaled.service ]; then
  rm -f /etc/systemd/system/tailscaled.service
  rm -rf /etc/systemd/system/tailscaled.service.d
  systemctl daemon-reload
fi
echo uninstall-done
";
    let dir = std::env::temp_dir().join("anyboot-ts-uninstall.sh");
    std::fs::write(&dir, script).map_err(|error| format!("写卸载脚本失败: {error}"))?;
    ts_log("[卸载] pkexec 提权执行（停服务 → 删二进制 → 清理 AnyBoot 的 unit）…");
    let mut command = std::process::Command::new("pkexec");
    command.arg("bash").arg(&dir);
    let result = run_command_with_timeout_for(command, ELEVATED_TIMEOUT);
    let _ = std::fs::remove_file(&dir);
    let (ok, stdout, stderr) = result
        .ok_or_else(|| "无法启动 pkexec（未安装 polkit？）——请用系统包管理器手动卸载".to_string())?;
    for line in format!("{stdout}\n{stderr}").trim().lines() {
        let line = line.trim();
        if !line.is_empty() && line != "uninstall-done" {
            ts_log(&format!("[卸载] {line}"));
        }
    }
    if !ok {
        let detail = stderr.trim().lines().next().unwrap_or("").trim();
        let lowered = format!("{stdout}\n{stderr}").to_ascii_lowercase();
        return Err(if detail.is_empty() || lowered.contains("not authorized") {
            "卸载失败：授权框被取消或未通过 —— 重新操作并在弹出的授权框里确认，\
             或改用系统包管理器卸载"
                .to_string()
        } else {
            format!("卸载失败：{detail}")
        });
    }
    Ok("Tailscale 已卸载（服务已停、/usr/local/bin 二进制已删；发行版包装的部分不受影响）".to_string())
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
    ts_log(&format!("[安装] 启动静默安装器（UAC 确认）：{}", path.display()));
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
    ts_log(&format!("[安装] 打开 .pkg 安装器：{}", path.display()));
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("启动安装器失败: {error}"))
}

/// Linux 安装前置检测（下载前 fail-fast，别等 30MB 下完、脚本跑一半才报错）：
/// ① systemd 在跑；② pkexec 可用；③ 未被发行版包装机（混装会导致 CLI/daemon
/// 版本漂移，且卸载边界分不清）。返回 Err 时 message 已是给用户的人话指引。
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_install_preflight() -> Result<(), String> {
    use std::path::Path;
    if !Path::new("/run/systemd/system").is_dir() {
        return Err("此系统未使用 systemd，无法在应用内安装 Tailscale——请用发行版包管理器安装（如 apk add tailscale、xbps-install tailscale）".into());
    }
    let pkexec_ok = std::process::Command::new("pkexec")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !pkexec_ok {
        return Err("未检测到 pkexec（polkit）——请先安装 polkit，或手动安装 Tailscale".into());
    }
    if linux_distro_package_present() {
        return Err("检测到发行版包安装的 Tailscale（/usr/lib 的 unit 或 /usr/bin 的二进制）——请用系统包管理器统一管理，AnyBoot 不与其混装".into());
    }
    Ok(())
}

/// 发行版包装机信号：/usr/lib 的 tailscaled unit，或 /usr/bin 下的 tailscale
/// 二进制（发行版包的落点；AnyBoot 自己只装 /usr/local/bin，两者不重叠）。
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_distro_package_present() -> bool {
    use std::path::Path;
    Path::new("/usr/lib/systemd/system/tailscaled.service").is_file()
        || Path::new("/usr/bin/tailscale").exists()
        || Path::new("/usr/bin/tailscaled").exists()
        || Path::new("/usr/sbin/tailscaled").exists()
}

/// Linux 安装：官方静态 tarball（tailscale + tailscaled 双二进制）。
/// pkexec 提权执行安装脚本：解压 → `/usr/local/bin` → 注册 systemd 服务
/// （仅当系统还没有 tailscaled unit，发行版包装过的配置不覆盖）→ enable --now。
/// 脚本和解压暂存目录都放 tarball 同目录（app_data，大盘）——本机类环境 /tmp
/// 可能是 10MB tmpfs，放不下 30MB 的 tailscaled。
#[cfg(all(unix, not(target_os = "macos")))]
fn launch_installer(tarball: &std::path::Path) -> Result<(), String> {
    use std::fmt::Write as _;

    let dir = tarball
        .parent()
        .ok_or_else(|| "安装包路径异常：没有父目录".to_string())?;
    let stage = dir.join("ts-install-stage");
    let script_path = dir.join("ts-install.sh");
    let tarball_s = tarball.display().to_string();
    let stage_s = stage.display().to_string();

    let mut script = String::new();
    let _ = writeln!(script, "set -e");
    let _ = writeln!(script, "rm -rf '{stage_s}'");
    let _ = writeln!(script, "mkdir -p '{stage_s}'");
    let _ = writeln!(script, "tar -xzf '{tarball_s}' -C '{stage_s}'");
    // glob 段必须在引号外才能展开："{stage}"/tailscale_*/tailscale
    let _ = writeln!(
        script,
        "install -m 0755 \"{stage_s}\"/tailscale_*/tailscale \"{stage_s}\"/tailscale_*/tailscaled /usr/local/bin/"
    );
    // 系统尚无 tailscaled unit 才注册（覆盖发行版包的配置太霸道）
    let _ = writeln!(script, "if ! systemctl cat tailscaled >/dev/null 2>&1; then");
    // 无主 unit 但可能残留历史 drop-in（如旧修复脚本的 override）——不清会在
    // daemon-reload 后盖掉刚写的 ExecStart（指向已删除的旧路径 → 203/EXEC）
    let _ = writeln!(script, "  rm -rf /etc/systemd/system/tailscaled.service.d");
    let _ = writeln!(script, "cat > /etc/systemd/system/tailscaled.service <<'UNIT'");
    let _ = writeln!(script, "[Unit]");
    let _ = writeln!(script, "Description=Tailscale node agent");
    let _ = writeln!(script, "Documentation=https://tailscale.com/docs/");
    let _ = writeln!(script, "Wants=network-pre.target");
    let _ = writeln!(script, "After=network-pre.target NetworkManager.service systemd-resolved.service");
    let _ = writeln!(script, "");
    let _ = writeln!(script, "[Service]");
    let _ = writeln!(
        script,
        "ExecStart=/usr/local/bin/tailscaled --state=/var/lib/tailscale/tailscaled.state --socket=/run/tailscale/tailscaled.sock"
    );
    let _ = writeln!(script, "ExecStopPost=/usr/local/bin/tailscaled --cleanup");
    let _ = writeln!(script, "Restart=on-failure");
    let _ = writeln!(script, "Type=notify");
    let _ = writeln!(script, "RuntimeDirectory=tailscale");
    let _ = writeln!(script, "RuntimeDirectoryMode=0755");
    let _ = writeln!(script, "StateDirectory=tailscale");
    let _ = writeln!(script, "StateDirectoryMode=0700");
    let _ = writeln!(script, "");
    let _ = writeln!(script, "[Install]");
    let _ = writeln!(script, "WantedBy=multi-user.target");
    let _ = writeln!(script, "UNIT");
    let _ = writeln!(script, "systemctl daemon-reload");
    let _ = writeln!(script, "else");
    // unit 已在且是 AnyBoot 形态 → 同样清历史 drop-in（发行版 unit 的不动）
    let _ = writeln!(
        script,
        "  if grep -q '/usr/local/bin/tailscaled' /etc/systemd/system/tailscaled.service; then"
    );
    let _ = writeln!(script, "    rm -rf /etc/systemd/system/tailscaled.service.d");
    let _ = writeln!(script, "    echo '已清理历史 drop-in override'");
    let _ = writeln!(script, "    systemctl daemon-reload");
    let _ = writeln!(script, "  fi");
    let _ = writeln!(script, "fi");
    let _ = writeln!(script, "systemctl enable --now tailscaled");

    std::fs::write(&script_path, script)
        .map_err(|error| format!("写安装脚本失败: {error}"))?;
    ts_log(&format!("[安装] 安装脚本已生成：{}", script_path.display()));
    ts_log("[安装] pkexec 提权执行（解压 → /usr/local/bin → 注册并启动服务）…");

    let mut command = std::process::Command::new("pkexec");
    command.arg("bash").arg(&script_path);
    let (ok, stdout, stderr) = run_command_with_timeout_for(command, ELEVATED_TIMEOUT)
        .ok_or_else(|| {
            ts_log("[安装] 失败：无法启动 pkexec（未安装 polkit？）");
            "无法启动 pkexec（未安装 polkit？）——可手动解压安装后重试".to_string()
        })?;
    // 暂存目录归了 root，用户侧删不掉就算了（下次安装会被 root 侧重建覆盖）
    let _ = std::fs::remove_dir_all(&stage);
    for line in format!("{stdout}\n{stderr}").trim().lines() {
        let line = line.trim();
        if !line.is_empty() {
            ts_log(&format!("[安装] {line}"));
        }
    }
    if !ok {
        let text = format!("{stdout}{stderr}");
        let detail = text.trim().lines().next().unwrap_or("").trim();
        ts_log("[安装] 失败（pkexec 非零退出）");
        return Err(if detail.is_empty() || text.to_ascii_lowercase().contains("not authorized") {
            "安装失败：授权框被取消或未通过 —— 重新操作并在弹出的授权框里确认".to_string()
        } else {
            format!("安装失败：{detail}")
        });
    }
    // enable --now 返回后服务可能还在 activating，轮询确认（最多 10s）
    ts_log("[安装] 脚本执行完成，等待 tailscaled 服务进入运行态…");
    let mut running = false;
    for _ in 0..20 {
        if service_running_state() == 1 {
            running = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if running {
        ts_log("[安装] 完成：tailscaled 已运行");
        Ok(())
    } else {
        ts_log("[安装] 完成：但 tailscaled 服务未运行（journalctl -u tailscaled 可查）");
        // 常见根因先提示：容器/LXC 里常没有 /dev/net/tun
        let tun_hint = if !std::path::Path::new("/dev/net/tun").exists() {
            "；本机缺 /dev/net/tun（容器环境常见），需加载 tun 模块或改用用户态网络（TS_USERSPACE=1）"
        } else {
            ""
        };
        Err(format!(
            "安装完成但 tailscaled 服务未运行：请执行 journalctl -u tailscaled 查看日志{tun_hint}"
        ))
    }
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
    fn local_access_denied_detected() {
        // 实机踩过的两条本地权限闸门报错（区别于 tailnet 策略/连接类报错）
        assert!(is_local_access_denied(
            "",
            "Access denied: prefs write access denied"
        ));
        assert!(is_local_access_denied(
            "sending serve config: Access denied: serve config denied\n",
            ""
        ));
        assert!(!is_local_access_denied(
            "",
            "failed to connect to local tailscaled"
        ));
        assert!(!is_local_access_denied("", ""));
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
            "Tarballs": {
                "amd64": "tailscale_1.104.2_amd64.tgz",
                "arm64": "tailscale_1.104.2_arm64.tgz"
            },
            "Version": "1.104.2"
        })
    }

    #[test]
    fn linux_tarball_arch_maps_rust_arch() {
        // 纯映射函数，任何 target 都可跑；Rust arch 名与官方 tarball 段名对应
        let expected = match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            "x86" => "386",
            "arm" => "arm",
            "riscv64" => "riscv64",
            _ => return,
        };
        assert_eq!(linux_tarball_arch(), Some(expected));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn manifest_picks_tarball_on_linux() {
        let name = pick_manifest_filename(&manifest_fixture()).unwrap();
        assert_eq!(name, "tailscale_1.104.2_amd64.tgz");
    }

    #[test]
    fn platform_has_installer_on_all_supported() {
        // 三端都应有应用内安装渠道
        assert!(platform_has_installer());
    }

    #[test]
    fn parse_up_required_flags_from_official_error() {
        // 官方报错模板（2026-10-25 实机捕获）：建议旗标固定为 4 空格缩进行
        let stderr = "Error: changing settings via 'tailscale up' requires mentioning all\n\
                      non-default flags. To avoid breaking your installation, you'll need to\n\
                      pass the following non-default flags:\n\n    --accept-dns=false\n";
        assert!(is_up_flags_mismatch(stderr));
        assert_eq!(parse_up_required_flags(stderr), vec!["--accept-dns=false"]);
        // 多旗标去重 + 不误抓正文里的 --word
        let multi = "pass the following non-default flags:\n\n    --accept-dns=false\n    --hostname=box\n    --hostname=box\n\nsome --inline-mention text\n";
        assert_eq!(
            parse_up_required_flags(multi),
            vec!["--accept-dns=false", "--hostname=box"]
        );
        // 别的错误不触发
        assert!(!is_up_flags_mismatch("Error: access denied"));
        assert!(parse_up_required_flags("no flags here").is_empty());
    }

    #[test]
    fn linux_install_fingerprint_distinguishes_anyboot_from_distro() {
        // 造一个 AnyBoot 形态：双二进制 + ExecStart 指 /usr/local/bin 的 unit + 无发行版 unit
        let root = std::env::temp_dir().join(format!("anyboot-ts-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("etc")).unwrap();
        std::fs::write(root.join("bin/tailscale"), b"cli").unwrap();
        std::fs::write(root.join("bin/tailscaled"), b"daemon").unwrap();
        std::fs::write(
            root.join("etc/tailscaled.service"),
            "ExecStart=/usr/local/bin/tailscaled --state=...\n",
        )
        .unwrap();
        let matches = linux_install_fingerprint_at(
            &root.join("bin/tailscale"),
            &root.join("bin/tailscaled"),
            &root.join("etc/tailscaled.service"),
            &root.join("etc/nope.service"),
        );
        assert!(matches);
        // 发行版形态一：unit 指向 /usr/sbin（Debian 系）→ 指纹不成立
        std::fs::write(
            root.join("etc/tailscaled.service"),
            "ExecStart=/usr/sbin/tailscaled --state=...\n",
        )
        .unwrap();
        assert!(!linux_install_fingerprint_at(
            &root.join("bin/tailscale"),
            &root.join("bin/tailscaled"),
            &root.join("etc/tailscaled.service"),
            &root.join("etc/nope.service"),
        ));
        // 发行版形态二：unit 内容虽指 /usr/local/bin，但 /usr/lib 还有发行版 unit → 不成立
        std::fs::write(
            root.join("etc/tailscaled.service"),
            "ExecStart=/usr/local/bin/tailscaled --state=...\n",
        )
        .unwrap();
        std::fs::write(root.join("etc/nope.service"), b"distro unit").unwrap();
        assert!(!linux_install_fingerprint_at(
            &root.join("bin/tailscale"),
            &root.join("bin/tailscaled"),
            &root.join("etc/tailscaled.service"),
            &root.join("etc/nope.service"),
        ));
        // 缺二进制 → 不成立
        assert!(!linux_install_fingerprint_at(
            &root.join("bin/missing"),
            &root.join("bin/tailscaled"),
            &root.join("etc/tailscaled.service"),
            &root.join("etc/nope.service"),
        ));
        let _ = std::fs::remove_dir_all(&root);
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
