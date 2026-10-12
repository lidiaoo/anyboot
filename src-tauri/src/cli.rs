//! 无 GUI 的 CLI 子命令（阶段 5）：`ssh user@host 'anyboot --switch windows --action reboot'`。
//!
//! 两条路径：
//! 1. 应用正在运行 → 把命令**转发**给它（走 `/api/*`，于是行为与 webhook 完全一致：
//!    渠道用 `cli`，通知与延迟都按 `channels.cli`）；
//! 2. 应用没在运行 → **降级**自己执行（复用同一批核心函数，没有第二份实现），
//!    没有 UI 也就没有弹窗，相当于「立即执行」。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use super::{
    atomic_copy, display_path, grub_scan, grub_write_env, hashes_match, power_dry_run,
    run_power_action, scan_vars_directory, sleep_probe, write_with_mount, PowerAction, ScanResult,
    Settings, SETTINGS_FILE, SOURCE_FILES, TRACE_FILE,
};

/// 与 `tauri.conf.json` 的 `identifier` 必须一致（有单测盯着这件事）。
const APP_IDENTIFIER: &str = "com.example.anyboot";

const EXIT_OK: i32 = 0;
const EXIT_FAILED: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_NEEDS_AUTH: i32 = 3;
const EXIT_NO_VARS_DIR: i32 = 4;

/// 命令行里出现这些参数就走 CLI 分支（不带参数照旧启动 GUI）。
pub fn is_cli_invocation(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "--status"
                | "--switch"
                | "--action"
                | "--json"
                | "--port"
                | "--help"
                | "-h"
                | "--version"
                | "-V"
                | "--grub-list"
                | "--switch-grub"
        )
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct CliArgs {
    status: bool,
    switch: Option<String>,
    action: Option<PowerAction>,
    json: bool,
    port: Option<u16>,
    help: bool,
    version: bool,
    /// 列出 ESP 上 grub.cfg 的启动项（GRUB 细粒度切换那条链路）
    grub_list: bool,
    /// 切换 GRUB 启动项：值是 grub.cfg 里解析出的 selector 原文
    switch_grub: Option<String>,
}

fn parse_args(args: &[String]) -> Result<CliArgs, String> {
    let mut parsed = CliArgs::default();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "--status" => parsed.status = true,
            "--json" => parsed.json = true,
            "--help" | "-h" => parsed.help = true,
            "--version" | "-V" => parsed.version = true,
            "--switch" => {
                let value = args
                    .get(index + 1)
                    .ok_or("--switch 需要一个系统名（linux/windows/macos）")?;
                if !SOURCE_FILES.iter().any(|(system, _, _)| system == value) {
                    return Err(format!("未知系统: {value}（可用 linux/windows/macos）"));
                }
                parsed.switch = Some(value.clone());
                index += 1;
            }
            "--action" => {
                let value = args
                    .get(index + 1)
                    .ok_or("--action 需要一个动作（reboot/shutdown/sleep）")?;
                parsed.action = Some(
                    PowerAction::parse(value)
                        .ok_or(format!("未知动作: {value}（可用 reboot/shutdown/sleep）"))?,
                );
                index += 1;
            }
            "--port" => {
                let value = args.get(index + 1).ok_or("--port 需要一个端口号")?;
                let port = value
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or(format!("端口无效: {value}"))?;
                parsed.port = Some(port);
                index += 1;
            }
            "--grub-list" => parsed.grub_list = true,
            "--switch-grub" => {
                let value = args
                    .get(index + 1)
                    .ok_or("--switch-grub 需要一个 GRUB 启动项的 selector（见 --grub-list）")?;
                if value.trim().is_empty() {
                    return Err("--switch-grub 的 selector 不能为空".to_string());
                }
                parsed.switch_grub = Some(value.clone());
                index += 1;
            }
            other => return Err(format!("未知参数: {other}")),
        }
        index += 1;
    }
    if !parsed.help
        && !parsed.version
        && !parsed.status
        && parsed.switch.is_none()
        && parsed.action.is_none()
        && !parsed.grub_list
        && parsed.switch_grub.is_none()
    {
        return Err(
            "至少要有一个动作：--status / --switch / --grub-list / --switch-grub / --action"
                .to_string(),
        );
    }
    Ok(parsed)
}

fn print_usage() {
    println!(
        "AnyBoot CLI\n\
         \n\
         用法:\n\
         \x20 anyboot --status [--json]\n\
         \x20 anyboot --switch <linux|windows|macos> [--action <reboot|shutdown|sleep>] [--json]\n\
         \x20 anyboot --action <reboot|shutdown|sleep> [--json]\n\
         \x20 anyboot --grub-list [--json]\n\
         \x20 anyboot --switch-grub <selector> [--json]\n\
         \n\
         说明:\n\
         \x20 · 不带参数运行 = 打开图形界面（托盘常驻），与 CLI 是同一个可执行文件\n\
         \x20 · --switch X --action Y = 先切换下次启动为 X，成功后立刻执行 Y（缺省 reboot）\n\
         \x20 · --grub-list / --switch-grub = GRUB 细粒度那条链路（读 ESP 上的 grub.cfg，\n\
         \x20   只写 grubenv，不重启）；selector 就是列表里那一行，含空格请加引号\n\
         \x20 · 应用正在运行时：命令转发给应用执行（走 cli 渠道：通知与延迟按该渠道的配置）\n\
         \x20 · 应用没在运行：CLI 自己执行（无界面、无弹窗，相当于立即执行）\n\
         \x20 · 调试用环境变量：ANYBOOT_POWER_DRY_RUN=1 只打印命令不执行；\n\
         \x20   ANYBOOT_TRACE=1 记日志；ANYBOOT_VARS_DIR=<目录> 指定变量目录\n\
         \n\
         退出码: 0 成功 / 1 动作失败 / 2 参数错误 / 3 需要本机授权 / 4 变量目录未就绪"
    );
}

/// release 版是 GUI 子系统（`windows_subsystem = "windows"`），ssh 会话里默认没有 std 句柄。
///
/// AttachConsole 的坑（本机探针 gui-stdio 实测）：
/// - cmd 直接跑：`GetStdHandle` 返回非空句柄（指向父控制台），`AttachConsole` 会把它们**换掉**——
///   如果 stdout/stderr 原本指向 cmd 重定向出的**文件/管道**，重定向就丢了（输出 0 字节文件）。
/// - cmd 重定向跑：`GetStartupInfoW` 带 `STARTF_USESTDHANDLES`，句柄就是重定向目标，绝不能动。
/// - 真正没有句柄的场景（ssh 无终端、资源管理器双击）：句柄是 NULL/INVALID，这时才需要接管。
///
/// 所以策略：**句柄已经有效就什么都不做**；只有本来就缺失时才 AttachConsole + SetStdHandle，
/// 接管失败就关闭句柄（`eprintln!` 会安静失败，动作结果仍有 trace.log 兜底）。
#[cfg(windows)]
fn attach_console() {
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const STARTF_USESTDHANDLES: u32 = 0x0000_0100;

    #[repr(C)]
    struct StartupInfoW {
        cb: u32,
        lp_reserved: *const u16,
        lp_desktop: *const u16,
        lp_title: *const u16,
        dw_x: u32,
        dw_y: u32,
        dw_x_size: u32,
        dw_y_size: u32,
        dw_x_count_chars: u32,
        dw_y_count_rows: u32,
        dw_fill_attribute: u32,
        dw_flags: u32,
        w_show_window: u16,
        cb_reserved2: u16,
        lp_reserved2: *const u8,
        h_std_input: isize,
        h_std_output: isize,
        h_std_error: isize,
    }

    unsafe extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
        fn GetStdHandle(n_std_handle: u32) -> isize;
        fn SetStdHandle(n_std_handle: u32, h_handle: isize) -> i32;
        fn GetStartupInfoW(lp_startup_info: *mut StartupInfoW);
        fn GetConsoleWindow() -> isize;
    }

    unsafe {
        // 两条 stdout/stderr 都有效（cmd 直跑、重定向、管道、重定向到文件都算）就没我们的事。
        let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
        let stderr = GetStdHandle(STD_ERROR_HANDLE);
        let missing = |handle: isize| handle == 0 || handle == -1;
        if !missing(stdout) && !missing(stderr) {
            return;
        }

        // 先看父进程给没给 STARTF_USESTDHANDLES：给了就说明句柄由调用方安排，
        // 我们不做任何接管（防止把别人的重定向换成控制台）。
        let mut info: StartupInfoW = std::mem::zeroed();
        info.cb = std::mem::size_of::<StartupInfoW>() as u32;
        GetStartupInfoW(&mut info);
        if info.dw_flags & STARTF_USESTDHANDLES != 0 {
            return;
        }

        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        // GUI 子系统启动时 std 句柄缓存着旧值（NULL/INVALID），必须显式换新
        if missing(stdout) {
            let handle = GetStdHandle(STD_OUTPUT_HANDLE);
            if !missing(handle) {
                let _ = SetStdHandle(STD_OUTPUT_HANDLE, handle);
            }
        }
        if missing(stderr) {
            let handle = GetStdHandle(STD_ERROR_HANDLE);
            if !missing(handle) {
                let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
            }
        }
        // AttachConsole 成功但拿不到有效句柄：把残留句柄关掉，
        // Rust 的 println!/eprintln! 遇到坏句柄会 panic——这里直接关掉让它们静默失败。
        if missing(GetStdHandle(STD_OUTPUT_HANDLE)) || GetConsoleWindow() == 0 {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, 0);
        }
        if missing(GetStdHandle(STD_ERROR_HANDLE)) || GetConsoleWindow() == 0 {
            let _ = SetStdHandle(STD_ERROR_HANDLE, 0);
        }
    }
}

#[cfg(not(windows))]
fn attach_console() {}

fn config_dir() -> Option<PathBuf> {
    // 与 GUI 侧同名的覆盖变量：CLI 与 GUI 可以指向同一个沙箱目录
    if let Ok(directory) = std::env::var("ANYBOOT_CONFIG_DIR") {
        let directory = directory.trim();
        if !directory.is_empty() {
            return Some(PathBuf::from(directory));
        }
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|dir| dir.join(APP_IDENTIFIER))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(PathBuf::from).map(|home| {
            home.join("Library")
                .join("Application Support")
                .join(APP_IDENTIFIER)
        })
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|base| base.join(APP_IDENTIFIER))
    }
}

struct CliConfig {
    dir: Option<PathBuf>,
    settings: Settings,
}

impl CliConfig {
    fn load() -> Self {
        let dir = config_dir();
        let settings = dir
            .as_deref()
            .and_then(|dir| std::fs::read_to_string(dir.join(SETTINGS_FILE)).ok())
            .and_then(|raw| serde_json::from_str::<Settings>(&raw).ok())
            .unwrap_or_default();
        Self { dir, settings }
    }

    fn trace(&self, category: &str, message: &str) {
        let enabled = super::env_flag_from(std::env::var("ANYBOOT_TRACE").ok().as_deref())
            .unwrap_or(self.settings.debug_trace);
        if !enabled {
            return;
        }
        let line = format!(
            "{} [cli/{category}] {message}\n",
            super::utc_timestamp(super::now_epoch_seconds())
        );
        eprint!("trace {line}");
        if let Some(dir) = self.dir.as_deref() {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(TRACE_FILE))
                .and_then(|mut file| file.write_all(line.as_bytes()));
        }
    }
}

pub fn run(args: &[String]) -> i32 {
    attach_console();

    let parsed = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("参数错误: {error}");
            print_usage();
            return EXIT_USAGE;
        }
    };
    if parsed.help {
        print_usage();
        return EXIT_OK;
    }
    if parsed.version {
        println!("AnyBoot {}", env!("CARGO_PKG_VERSION"));
        return EXIT_OK;
    }

    let config = CliConfig::load();
    config.trace("argv", &format!("{args:?}"));
    // 端口优先级：--port > ANYBOOT_HTTP_PORT > 设置文件（与 GUI 侧同一个环境变量）
    let port = parsed
        .port
        .or_else(|| {
            std::env::var("ANYBOOT_HTTP_PORT")
                .ok()
                .and_then(|value| value.trim().parse::<u16>().ok())
                .filter(|port| *port != 0)
        })
        .unwrap_or(config.settings.http_port);
    let token = config.settings.http_token.trim().to_string();

    // 1) 应用在跑？转发给它，行为与 webhook 一致（cli 渠道）
    if !token.is_empty() && health_ok(port) {
        config.trace(
            "forward",
            &format!("应用在 127.0.0.1:{port} 上运行，转发命令"),
        );
        return forward(&config, port, &token, &parsed);
    }
    config.trace("local", "应用没在运行（或 token 为空），CLI 自己执行");

    // 2) 降级：自己干
    run_locally(&config, &parsed)
}

fn health_ok(port: u16) -> bool {
    matches!(http_request(port, "GET", "/health", ""), Ok((200, _)))
}

/// 极简 HTTP 客户端（只连 127.0.0.1，够用就不引额外依赖）。
fn http_request(
    port: u16,
    method: &str,
    path_and_query: &str,
    token: &str,
) -> Result<(u16, String), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(800))
        .map_err(|error| format!("连接 127.0.0.1:{port} 失败: {error}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let request = format!(
        "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Auth-Token: {token}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("发送请求失败: {error}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("读取响应失败: {error}"))?;
    parse_http_response(&String::from_utf8_lossy(&raw))
}

fn parse_http_response(raw: &str) -> Result<(u16, String), String> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "响应格式异常（缺少头部结束）".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| "无法解析状态行".to_string())?;
    let chunked = head.lines().any(|line| {
        line.to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
    });
    let body = if chunked {
        decode_chunked(body)
    } else {
        body.to_string()
    };
    Ok((status, body))
}

/// tiny_http 通常会带 Content-Length，但分块响应也要能读。
fn decode_chunked(body: &str) -> String {
    let mut decoded = String::new();
    let mut rest = body;
    loop {
        let Some((size_line, tail)) = rest.split_once("\r\n") else {
            break;
        };
        let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
            .unwrap_or(0);
        if size == 0 {
            break;
        }
        if tail.len() < size {
            break;
        }
        decoded.push_str(&tail[..size]);
        rest = tail[size..].strip_prefix("\r\n").unwrap_or("");
    }
    decoded
}

/// 极简 percent-encode（只给 query 值用）：GRUB 的 selector 里有空格、`>`、
/// 引号、`$`，不编码就会被 HTTP 请求行拆断或当成转义。
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

fn forward(config: &CliConfig, port: u16, token: &str, parsed: &CliArgs) -> i32 {
    // GRUB 那条链路优先：--grub-list 是只读、--switch-grub 写 grubenv
    let (method, path) = if parsed.grub_list {
        ("GET", "/api/grub?channel=cli".to_string())
    } else if let Some(selector) = parsed.switch_grub.as_deref() {
        (
            "POST",
            format!(
                "/api/grub-switch?entry={}&channel=cli",
                percent_encode(selector)
            ),
        )
    } else {
        match (&parsed.switch, parsed.action) {
            (Some(system), Some(action)) => (
                "POST",
                format!(
                    "/api/switch-and-power?system={system}&action={}&channel=cli",
                    action.as_str()
                ),
            ),
            (Some(system), None) => ("POST", format!("/api/switch?system={system}&channel=cli")),
            (None, Some(action)) => (
                "POST",
                format!("/api/power?action={}&channel=cli", action.as_str()),
            ),
            (None, None) => ("GET", "/api/status?channel=cli".to_string()),
        }
    };

    match http_request(port, method, &path, token) {
        Ok((status, body)) => {
            config.trace("forward-result", &format!("status={status} body={body}"));
            if parsed.json {
                println!("{body}");
            } else if parsed.grub_list {
                // 列表要按人读的格式打印；顺带把「读不到」变成非零退出码
                if !print_grub_list(&body) {
                    return EXIT_FAILED;
                }
            } else {
                println!("{}", human_message(&body, status));
            }
            match status {
                200 | 202 => EXIT_OK,
                401 => {
                    eprintln!(
                        "提示：token 不匹配（应用里可能刚重新生成过令牌，重开一个 shell 再试）"
                    );
                    EXIT_FAILED
                }
                _ => EXIT_FAILED,
            }
        }
        Err(error) => {
            config.trace("forward-failed", &error);
            eprintln!("转发失败: {error}");
            EXIT_FAILED
        }
    }
}

/// 人类可读的 GRUB 列表。返回 false = 这份数据其实是个错误（调用方据此给非零退出码）。
fn print_grub_list(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        println!("{body}");
        return true;
    };
    if let Some(error) = value.get("error").and_then(|value| value.as_str()) {
        eprintln!("读取 GRUB 数据失败: {error}");
        return false;
    }

    match value.get("uuid").and_then(|value| value.as_str()) {
        Some(uuid) if !uuid.is_empty() => println!("ESP fs-uuid: {uuid}"),
        _ => {
            eprintln!("还没设置 ESP 的 fs-uuid：请先在应用主窗口的「GRUB 启动项」里填写并保存");
            return false;
        }
    }
    for key in ["efiDir", "syncedAt"] {
        if let Some(text) = value.get(key).and_then(|value| value.as_str()) {
            println!("{key}: {text}");
        }
    }
    let active = value
        .get("nextEntry")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            value
                .get("savedEntry")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or("");

    let Some(entries) = value.get("entries").and_then(|value| value.as_array()) else {
        return true;
    };
    println!("GRUB 启动项（{} 个）:", entries.len());
    for entry in entries {
        let selector = entry
            .get("selector")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let name = entry
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let nested = entry.get("submenu").map(|value| !value.is_null()) == Some(true);
        println!(
            "  {} {}{}",
            if selector == active { "*" } else { " " },
            if nested { "  ↳ " } else { "" },
            if name.is_empty() { selector } else { name }
        );
        println!("      selector: {selector}");
    }
    true
}

fn human_message(body: &str, status: u16) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    if let Some(value) = parsed.as_ref() {
        if let Some(message) = value.get("message").and_then(|value| value.as_str()) {
            if status == 202 {
                return format!("已受理（本机确认后执行）：{message}");
            }
            return message.to_string();
        }
        if value.get("ok").and_then(|value| value.as_bool()) == Some(true) {
            return "完成".to_string();
        }
    }
    format!("HTTP {status}: {body}")
}

fn exit_code_for_error(message: &str) -> i32 {
    if message.contains("授权") || message.contains("pkexec") {
        EXIT_NEEDS_AUTH
    } else {
        EXIT_FAILED
    }
}

/// 变量目录：命令行环境变量优先（验证/沙箱用），否则让核心扫描逻辑去找。
fn explicit_vars_dir() -> Option<PathBuf> {
    std::env::var("ANYBOOT_VARS_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// 失败统一收口：人类可读走 stderr，`--json` 走 stdout（与 HTTP 响应同结构）。
fn finish_error(config: &CliConfig, parsed: &CliArgs, code: &str, message: &str, exit: i32) -> i32 {
    config.trace("error", &format!("{code}: {message}"));
    if parsed.json {
        println!("{}", json_error(code, message));
    } else {
        eprintln!("{message}");
    }
    exit
}

fn json_error(code: &str, message: &str) -> String {
    serde_json::json!({ "ok": false, "error": code, "message": message }).to_string()
}

/// 本地执行的结果 JSON：与转发时的 HTTP 响应保持同一套字段名。
fn json_local_result(scan: &ScanResult, lines: &[String], commands: &[String]) -> String {
    let active = scan
        .variables
        .iter()
        .find(|variable| variable.active)
        .map(|variable| variable.display_name.clone());
    let mut value = serde_json::json!({
        "ok": true,
        "pending": false,
        "channel": "cli",
        "varsDir": display_path(&scan.vars_dir).display().to_string(),
        "active": active,
        "message": lines.join("；"),
        "variables": scan
            .variables
            .iter()
            .map(|variable| serde_json::json!({
                "system": variable.system,
                "displayName": variable.display_name,
                "exists": variable.exists,
                "active": variable.active,
            }))
            .collect::<Vec<_>>(),
    });
    if !commands.is_empty() {
        value["dryRunCommands"] = serde_json::json!(commands);
    }
    value.to_string()
}

/// GRUB 那条链路（--grub-list / --switch-grub）的本地执行。
///
/// 与 rEFInd 那条链路**完全独立**：读的是 ESP 上的 grub.cfg / grubenv，
/// 不需要变量目录。`Err(exit_code)` = 已打印错误、直接按该退出码结束。
fn run_grub_locally(
    config: &CliConfig,
    parsed: &CliArgs,
    lines: &mut Vec<String>,
    grub_json: &mut Option<serde_json::Value>,
) -> Result<(), i32> {
    let scan = match grub_scan(&config.settings, None) {
        Ok(Some(scan)) => scan,
        Ok(None) => {
            return Err(finish_error(
                config,
                parsed,
                "grub_uuid_missing",
                "还没设置 ESP 的 fs-uuid / 目录：请先在应用主窗口的「GRUB 启动项」里填好并保存",
                EXIT_FAILED,
            ));
        }
        Err(error) => {
            return Err(finish_error(
                config,
                parsed,
                "grub_scan_failed",
                &error,
                exit_code_for_error(&error),
            ));
        }
    };

    let active = scan
        .next_entry
        .clone()
        .filter(|value| !value.is_empty())
        .or_else(|| scan.saved_entry.clone().filter(|value| !value.is_empty()))
        .unwrap_or_default();
    grub_json.replace(serde_json::json!({
        "ok": true,
        "channel": "cli",
        "partuuid": config.settings.grub_efi_partuuid.trim(),
        "efiDir": display_path(&scan.efi_dir).display().to_string(),
        "envExists": scan.env_exists,
        "installed": super::grub_installed(),
        "savedEntry": scan.saved_entry,
        "nextEntry": scan.next_entry,
        "entries": scan
            .entries
            .iter()
            .map(|entry| serde_json::json!({
                "selector": entry.selector,
                "id": entry.id,
                "name": entry.name,
                "submenu": entry.submenu,
                "class": entry.class,
                "chainloader": entry.chainloader,
                "kernels": entry.kernels,
            }))
            .collect::<Vec<_>>(),
    }));

    if parsed.grub_list {
        if parsed.json {
            lines.push(format!("共 {} 个 GRUB 启动项", scan.entries.len()));
        } else {
            println!("ESP 分区 UUID: {}", config.settings.grub_efi_partuuid.trim());
            println!("EFI 目录: {}", display_path(&scan.efi_dir).display());
            println!("GRUB 启动项（{} 个）:", scan.entries.len());
            for entry in &scan.entries {
                println!(
                    "  {} {}{}",
                    if entry.selector == active { "*" } else { " " },
                    if entry.submenu.is_some() {
                        "  ↳ "
                    } else {
                        ""
                    },
                    if entry.name.is_empty() {
                        entry.selector.as_str()
                    } else {
                        entry.name.as_str()
                    }
                );
                println!("      selector: {}", entry.selector);
            }
        }
    }

    if let Some(selector) = parsed.switch_grub.as_deref() {
        if !scan.entries.iter().any(|entry| entry.selector == selector) {
            let message = format!("grub.cfg 里没有这一项：{selector}（先用 --grub-list 看可选项）");
            scan.release();
            return Err(finish_error(
                config,
                parsed,
                "grub_entry_missing",
                &message,
                EXIT_FAILED,
            ));
        }
        let result = grub_write_env(&scan.env_path, selector);
        scan.release();
        if let Err(error) = result {
            return Err(finish_error(
                config,
                parsed,
                "grub_switch_failed",
                &format!("切换失败: {error}"),
                exit_code_for_error(&error),
            ));
        }
        lines.push(format!("已切换：下次启动（GRUB）{selector}"));
    }

    Ok(())
}

fn run_locally(config: &CliConfig, parsed: &CliArgs) -> i32 {
    // 人类可读的每一行都先收起来：`--json` 时它们变成 message 字段
    let mut lines: Vec<String> = Vec::new();
    let mut grub_json: Option<serde_json::Value> = None;

    // 1) GRUB 细粒度那条链路（与变量目录无关，先跑）
    if parsed.grub_list || parsed.switch_grub.is_some() {
        if let Err(code) = run_grub_locally(config, parsed, &mut lines, &mut grub_json) {
            return code;
        }
    }

    // 2) rEFInd 那条链路：只有 --status / --switch 才需要变量目录
    let scan = if parsed.status || parsed.switch.is_some() {
        match scan_vars_directory(explicit_vars_dir().as_deref()) {
            Ok(scan) => {
                config.trace(
                    "scan-ok",
                    &format!("vars_dir={}", display_path(&scan.vars_dir).display()),
                );
                Some(scan)
            }
            Err(error) => {
                config.trace("scan-failed", &error);
                return finish_error(
                    config,
                    parsed,
                    "vars_dir_not_found",
                    &error,
                    EXIT_NO_VARS_DIR,
                );
            }
        }
    } else {
        None
    };

    if let Some(scan) = scan.as_ref() {
        if parsed.status {
            if parsed.json {
                let name = scan
                    .variables
                    .iter()
                    .find(|variable| variable.active)
                    .map(|variable| variable.display_name.clone())
                    .unwrap_or_else(|| "（PreviousBoot 未匹配任何模板）".to_string());
                lines.push(format!("下次启动：{name}"));
            } else {
                print_status(scan);
            }
        }

        if let Some(system) = parsed.switch.as_deref() {
            let source_name = SOURCE_FILES
                .iter()
                .find(|(candidate, _, _)| *candidate == system)
                .map(|(_, _, source_name)| *source_name)
                .unwrap_or_default();
            let source = scan.vars_dir.join(source_name);
            let target = scan.vars_dir.join("PreviousBoot");
            if !source.is_file() {
                return finish_error(
                    config,
                    parsed,
                    "source_missing",
                    &format!("变量文件不存在: {}", display_path(&source).display()),
                    EXIT_FAILED,
                );
            }
            let result = write_with_mount(scan.active_mount.as_ref(), || {
                atomic_copy(&source, &target)
                    .map_err(|error| format!("写入失败: {error}"))
                    .and_then(|_| {
                        if hashes_match(&source, &target) {
                            Ok(())
                        } else {
                            Err("写入校验失败".to_string())
                        }
                    })
            });
            if let Err(error) = result {
                config.trace("switch-failed", &error);
                return finish_error(
                    config,
                    parsed,
                    "switch_failed",
                    &format!("切换失败: {error}"),
                    exit_code_for_error(&error),
                );
            }
            let name = SOURCE_FILES
                .iter()
                .find(|(candidate, _, _)| *candidate == system)
                .map(|(_, display_name, _)| *display_name)
                .unwrap_or(system);
            lines.push(format!("已切换：下次启动 {name}"));
        }
    }

    // 3) 电源动作
    let mut dry_run_commands: Vec<String> = Vec::new();
    if let Some(action) = parsed.action {
        let (note, evidence) = if action == PowerAction::Sleep {
            sleep_probe()
        } else {
            (None, Vec::new())
        };
        for line in &evidence {
            config.trace("sleep-probe", line);
        }
        match run_power_action(action, false, power_dry_run()) {
            Ok(report) => {
                if report.dry_run {
                    dry_run_commands = report.commands.clone();
                    lines.push(format!(
                        "dry-run（未真的执行）：将按顺序尝试 {}",
                        report.commands.join(" → ")
                    ));
                } else if let Some(note) = note {
                    lines.push(format!("{}（{note}）", action.submitted_message()));
                } else {
                    lines.push(action.submitted_message().to_string());
                }
            }
            Err(error) => {
                config.trace("power-failed", &error);
                return finish_error(
                    config,
                    parsed,
                    "power_failed",
                    &error,
                    exit_code_for_error(&error),
                );
            }
        }
    }

    if parsed.json {
        if let Some(scan) = scan.as_ref() {
            println!("{}", json_local_result(scan, &lines, &dry_run_commands));
        } else {
            // 纯 GRUB 调用：没有变量目录结果，用 GRUB 自己的 JSON（字段名与
            // HTTP `/api/grub` 对齐）再加一行 message / dryRun
            let mut value = grub_json.unwrap_or_else(|| serde_json::json!({ "ok": true }));
            value["message"] = serde_json::json!(lines.join("；"));
            if !dry_run_commands.is_empty() {
                value["dryRunCommands"] = serde_json::json!(dry_run_commands);
            }
            println!("{value}");
        }
    } else {
        for line in &lines {
            println!("{line}");
        }
    }
    EXIT_OK
}

fn print_status(scan: &ScanResult) {
    println!("变量目录: {}", display_path(&scan.vars_dir).display());
    let active = scan
        .variables
        .iter()
        .find(|variable| variable.active)
        .map(|variable| variable.display_name.clone());
    match active {
        Some(name) => println!("下次启动: {name}"),
        None => println!("下次启动: （PreviousBoot 未匹配任何模板）"),
    }
    for variable in &scan.variables {
        println!(
            "  {:<8} {}{}",
            variable.display_name,
            if variable.exists {
                "存在"
            } else {
                "不存在"
            },
            if variable.active {
                "（当前选中）"
            } else {
                ""
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn cli_invocation_requires_a_flag() {
        assert!(!is_cli_invocation(&[]));
        assert!(is_cli_invocation(&args(&["--status"])));
        assert!(is_cli_invocation(&args(&["--help"])));
        assert!(is_cli_invocation(&args(&["--grub-list"])));
        assert!(is_cli_invocation(&args(&["--switch-grub", "Arch Linux"])));
        // 只有一个值、没有旗标：当普通启动处理
        assert!(!is_cli_invocation(&args(&["windows"])));
    }

    #[test]
    fn parse_args_handles_all_forms() {
        let parsed = parse_args(&args(&["--status", "--json"])).expect("应能解析");
        assert!(parsed.status && parsed.json);

        let parsed = parse_args(&args(&["--switch", "linux"])).expect("应能解析");
        assert_eq!(parsed.switch.as_deref(), Some("linux"));
        assert_eq!(parsed.action, None, "不带 --action 就只切换");

        let parsed = parse_args(&args(&["--action", "SLEEP"])).expect("应能解析");
        assert_eq!(parsed.action, Some(PowerAction::Sleep));
        assert_eq!(parsed.switch, None);

        // --switch + --action = 切换后立刻执行（缺省 reboot 由后端补）
        let parsed = parse_args(&args(&[
            "--switch", "windows", "--action", "reboot", "--port", "12399",
        ]))
        .expect("应能解析");
        assert_eq!(parsed.switch.as_deref(), Some("windows"));
        assert_eq!(parsed.action, Some(PowerAction::Reboot));
        assert_eq!(parsed.port, Some(12399));

        // 别名也能用
        assert_eq!(
            parse_args(&args(&["--action", "poweroff"]))
                .expect("应能解析")
                .action,
            Some(PowerAction::Shutdown)
        );

        // GRUB 细粒度那条链路
        let parsed = parse_args(&args(&["--grub-list"])).expect("应能解析");
        assert!(parsed.grub_list && parsed.switch_grub.is_none());

        // selector 里有空格也照收（含 submenu 的 `>` 形态）
        let parsed = parse_args(&args(&["--switch-grub", "Arch Linux, with Linux linux"]))
            .expect("应能解析");
        assert_eq!(
            parsed.switch_grub.as_deref(),
            Some("Arch Linux, with Linux linux")
        );
        let parsed = parse_args(&args(&["--switch-grub", "a-advanced>x-linux"])).expect("应能解析");
        assert_eq!(parsed.switch_grub.as_deref(), Some("a-advanced>x-linux"));
    }

    #[test]
    fn parse_args_rejects_bad_input() {
        assert!(parse_args(&args(&[])).is_err(), "什么都不干应该报错");
        assert!(parse_args(&args(&["--switch"])).is_err(), "缺参数");
        assert!(
            parse_args(&args(&["--switch", "solaris"])).is_err(),
            "未知系统"
        );
        assert!(
            parse_args(&args(&["--action", "hibernate"])).is_err(),
            "未知动作"
        );
        assert!(
            parse_args(&args(&["--port", "0", "--status"])).is_err(),
            "端口 0 不行"
        );
        assert!(
            parse_args(&args(&["--switch-grub"])).is_err(),
            "缺 selector"
        );
        assert!(
            parse_args(&args(&["--switch-grub", "   "])).is_err(),
            "空白 selector 等于没给"
        );
        assert!(parse_args(&args(&["--nope"])).is_err(), "未知旗标");
    }

    #[test]
    fn percent_encode_escapes_what_breaks_a_request_line() {
        // 空格 / CR / LF / `&` / `#` / `%` 都会把 query 拆断，必须转义
        assert_eq!(percent_encode("Arch Linux"), "Arch%20Linux");
        assert_eq!(percent_encode("a>b$c"), "a%3Eb%24c");
        assert_eq!(percent_encode("x&y=z#w"), "x%26y%3Dz%23w");
        assert_eq!(percent_encode("100%"), "100%25");
        assert_eq!(percent_encode("line\r\nbreak"), "line%0D%0Abreak");
        // 未保留字符保持原样（否则读起来没法看）
        assert_eq!(percent_encode("AZaz09-_.~"), "AZaz09-_.~");
        // 中文按 UTF-8 逐字节转义
        assert_eq!(percent_encode("中文"), "%E4%B8%AD%E6%96%87");
        // 转义后必须能被 lib.rs 那边的 percent_decode 原样读回来
        assert_eq!(
            super::super::http::percent_decode(&percent_encode("Arch Linux, with Linux linux")),
            "Arch Linux, with Linux linux"
        );
    }

    #[test]
    fn app_identifier_is_anyboot() {
        // 品牌改名后：确保 identifier 不会误回退成旧名
        assert_eq!(APP_IDENTIFIER, "com.example.anyboot");
    }

    #[test]
    fn http_response_parsing_handles_content_length_and_chunked() {
        // 带 Content-Length（tiny_http 的常规响应）
        let body_text = "{\"ok\":true}";
        let (status, body) = parse_http_response(&format!(
            "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body_text.len(),
            body_text
        ))
        .expect("应能解析");
        assert_eq!(status, 202);
        assert_eq!(body, body_text);

        // 分块响应：7 是错的、6 才对（{"ok": 有 6 字节）——用真实分块写法盯住解码逻辑
        let (status, body) = parse_http_response(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\n{\"ok\":\r\n5\r\ntrue}\r\n0\r\n\r\n",
        )
        .expect("应能解析分块响应");
        assert_eq!(status, 200);
        assert_eq!(body, "{\"ok\":true}");
    }

    #[test]
    fn app_identifier_matches_tauri_config() {
        // CLI 要按 identifier 找配置目录，和 tauri.conf.json 漂移了就会读错地方
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"),
        )
        .expect("应能读到 tauri.conf.json");
        let config: serde_json::Value = serde_json::from_str(&raw).expect("应是合法 JSON");
        assert_eq!(
            config.get("identifier").and_then(|value| value.as_str()),
            Some(APP_IDENTIFIER)
        );
    }
}
