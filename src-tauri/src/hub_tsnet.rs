//! AnyBoot 内置 Tailscale 引擎（tsnet sidecar）——免装 Tailscale 客户端的
//! Funnel 公网通道（gateway 页「Tailscale (内置)」标签的数据源）。
//!
//! 架构（2026-10-24 定稿，见 design/tsnet-p1-todo.md）：
//!   - Go sidecar `anyboot-tsnet.exe`（源码 src-tauri/sidecar-tsnet/，clean-room
//!     手写，只依赖 tailscale.com/tsnet 公开 API）：tsnet 起 Funnel（FunnelOnly）
//!     并把流量反代到本机 http 服务。stdout 逐行 JSON 事件（hello/state/bye），
//!     stdin 首行收可选 authkey 配置、之后收 quit 命令；父进程死 = stdin EOF = 自杀。
//!   - 本模块只管子进程生命周期与状态聚合：spawn（CREATE_NO_WINDOW）/ stdout
//!     读线程更新共享状态 / enable 带 45s 就绪等待 / disable 走 stdin quit 再兜底
//!     kill；退出钩子 kill_all 兜底收尸（对齐 hub_gateway::kill_all 语义）。
//!   - AuthKey 存 Settings.tsnet_authkey（敏感度对齐 gateway_named_token），
//!     经 stdin 管道传给 sidecar，不进进程命令行（任务管理器看不见）。
//!
//! 五态机（与 UI 对齐）：off / starting / needs_auth / ready / error。
//! needs_auth 是合法的驻留态：用户在浏览器完成登录后 sidecar 自己会转 ready。

use std::collections::VecDeque;
use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tauri::{AppHandle, Manager};

/// 内置 Tailscale 引擎状态（gateway 第 4 标签页的数据源）。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TsnetStatus {
    /// off / starting / needs_auth / ready / error
    pub state: String,
    /// Funnel 公网地址 https://<机器名>.<tailnet>.ts.net（ready 时非空）
    pub url: String,
    /// 官方登录页（needs_auth 时非空；只认 login.tailscale.com 白名单域）
    pub login_url: String,
    /// 面向用户的简短说明 / 错误详情
    pub message: String,
    /// sidecar 子进程是否还活着（排障展示：error 但进程还在 ≠ 假死）
    pub sidecar_running: bool,
}

impl TsnetStatus {
    fn new(state: &str) -> Self {
        Self {
            state: state.into(),
            url: String::new(),
            login_url: String::new(),
            message: String::new(),
            sidecar_running: false,
        }
    }
}

impl Default for TsnetStatus {
    fn default() -> Self {
        Self::new("off")
    }
}

// ==================== 共享运行时 ====================

/// 全局共享态：当前状态 + 子进程句柄（stdin 留着发 quit，stdout 已移交读线程）。
#[derive(Default)]
struct TsnetShared {
    status: TsnetStatus,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    /// 运行日志环形缓冲（时间戳 = sidecar 事件自带 t 字段 / Rust 侧相对秒数）。
    logs: VecDeque<String>,
    /// 本次引擎进程的启动时刻（Rust 侧自打日志的前缀基准）。
    started_at: Option<Instant>,
}

/// 日志缓冲上限（超出丢最老的；UI 全量展示，排障足够）。
const LOG_CAP: usize = 200;

/// 锁内追加一条日志（截超长行，FIFO 淘汰）。
fn push_log(guard: &mut TsnetShared, line: &str) {
    let line = line.trim_end();
    if line.is_empty() {
        return;
    }
    let mut owned = line.to_string();
    if owned.chars().count() > 500 {
        owned = owned.chars().take(500).collect();
    }
    if guard.logs.len() >= LOG_CAP {
        guard.logs.pop_front();
    }
    guard.logs.push_back(owned);
}

/// 相对时间戳前缀（Rust 侧自打日志用；sidecar 的 t 字段事件直接透传不做二次前缀）。
fn elapsed_prefix(started_at: Option<Instant>) -> String {
    match started_at {
        Some(t) => format!("[+{:.1}s] ", t.elapsed().as_secs_f32()),
        None => String::new(),
    }
}

static TSNET: OnceLock<Mutex<TsnetShared>> = OnceLock::new();

fn shared() -> &'static Mutex<TsnetShared> {
    TSNET.get_or_init(|| Mutex::new(TsnetShared::default()))
}

/// 就绪等待上限（sidecar 首次要连协调服务器 + 配证书，国内网络放宽到 45s；
/// 超时后 UI 仍可重试，login_needed 驻留态不受此限制——它本身就是合法返回态）。
const READY_WAIT_SECS: u64 = 45;
/// disable 时优雅退出（stdin quit）的宽限秒数，超时直接 kill。
const QUIT_GRACE_SECS: u64 = 3;

/// 子进程是否存活（锁内调用；try_wait 收尸后判断）。
fn child_alive(guard: &mut TsnetShared) -> bool {
    match &mut guard.child {
        Some(child) => match child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        },
        None => false,
    }
}

// ==================== stdout 事件解析 ====================

/// sidecar stdout 事件（与 sidecar-tsnet/main.go 的 event 结构对齐）。
#[derive(Debug, serde::Deserialize)]
struct SidecarEvent {
    #[serde(default)]
    event: String,
    #[serde(default)]
    state: String,
    #[serde(default, rename = "url")]
    funnel_url: String,
    #[serde(default, rename = "loginUrl")]
    login_url: String,
    #[serde(default)]
    detail: String,
    /// sidecar 相对进程启动的秒数（日志前缀；hello 事件不带）。
    #[serde(default)]
    t: f64,
}

impl SidecarEvent {
    /// 日志行内容（None = 不进日志，如 hello/starting 这类噪音事件——
    /// Rust 侧已自打对应文案）。state 事件翻成人话；ready 带上地址。
    fn log_text(&self) -> Option<String> {
        match self.event.as_str() {
            "log" => Some(self.detail.clone()),
            "bye" => Some("sidecar 正常退出".into()),
            "hello" => None,
            "state" => match self.state.as_str() {
                "starting" => None,
                "login_needed" => Some(self.login_url.clone()),
                "ready" => Some(if self.funnel_url.is_empty() {
                    "状态: ready".into()
                } else {
                    format!("状态: ready，Funnel 地址 {}", self.funnel_url)
                }),
                _ => Some(if self.detail.is_empty() {
                    format!("状态: {}", self.state)
                } else {
                    self.detail.clone()
                }),
            },
            _ => Some(format!("未知事件: {self:?}")),
        }
    }

    /// 应用到共享状态。返回是否是终态（ready/needs_auth/error —— enable 的
    /// 45s 等待遇到终态就返回；starting 只是流转中间值）。
    /// sidecar 的状态名是 login_needed，本模块/UI 的规范名是 needs_auth ——
    /// 在这里归一（曾因两套名字对不上，登录链接事件被当中间态吞掉，UI
    /// 永远停在「启动中」出不来登录按钮）。
    fn apply(&self, status: &mut TsnetStatus) -> bool {
        match self.event.as_str() {
            "log" => false, // 日志行由调用方写环形缓冲，不影响状态机
            "state" => {
                let normalized = if self.state == "login_needed" {
                    "needs_auth".to_string()
                } else {
                    self.state.clone()
                };
                status.state = normalized;
                if !self.funnel_url.is_empty() {
                    status.url = self.funnel_url.clone();
                }
                if !self.login_url.is_empty() {
                    status.login_url = self.login_url.clone();
                }
                status.message = self.detail.clone();
                matches!(self.state.as_str(), "login_needed" | "ready" | "needs_auth" | "error")
            }
            "bye" => {
                // 优雅退出：保持 off 语义交给退出处理（读线程循环结束后统一收）
                false
            }
            _ => false,
        }
    }
}

/// stdout 读线程主体：逐行解析事件 → 更新共享状态；每一行（事件或裸文本）都
/// 进日志环形缓冲；循环结束（EOF）说明 sidecar 退出了 —— starting 中途死亡 =
/// 意外退出（报 error），其余保持末态。
fn stdout_worker(stdout: std::process::ChildStdout) {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(stdout);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut guard = match shared().lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        match serde_json::from_str::<SidecarEvent>(trimmed) {
            Ok(ev) => {
                // 事件行带 sidecar 的相对秒数 t，直接作前缀；Rust 侧不重复计时
                if let Some(text) = ev.log_text() {
                    let stamp = if ev.event == "hello" {
                        String::new()
                    } else {
                        format!("[+{:.1}s] ", ev.t)
                    };
                    push_log(&mut guard, &format!("{stamp}{text}"));
                }
                let was_starting = guard.status.state == "starting";
                let done = ev.apply(&mut guard.status);
                // starting 中途把最新日志透出（UI「启动中」文案跟着日志走）
                if was_starting
                    && guard.status.state == "starting"
                    && ev.event == "log"
                    && !ev.detail.is_empty()
                {
                    guard.status.message =
                        ev.detail.chars().take(80).collect();
                }
                capture_login_url(&mut guard, trimmed);
                drop(guard);
                let _ = done;
            }
            Err(_) => {
                // 非 JSON 行：原样进日志（sidecar 理论上只发 JSON，兜底别丢线索）
                push_log(&mut guard, trimmed);
                capture_login_url(&mut guard, trimmed);
            }
        }
    }
    // 进程退出收尾：child 交还 None（句柄不再有意义），中途死亡标 error。
    if let Ok(mut guard) = shared().lock() {
        let was_starting = guard.status.state == "starting";
        if let Some(mut child) = guard.child.take() {
            let code = child.try_wait().ok().and_then(|s| s.map(|s| s.code())).flatten();
            let prefix = elapsed_prefix(guard.started_at);
            if was_starting {
                guard.status.state = "error".into();
                guard.status.message = match code {
                    Some(c) => format!("内置引擎意外退出（exit {c}）"),
                    None => "内置引擎意外退出".into(),
                };
            }
            push_log(
                &mut guard,
                &format!(
                    "{prefix}sidecar 进程退出（exit {}）",
                    code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                ),
            );
        }
        guard.stdin = None;
        guard.status.sidecar_running = false;
        if guard.status.state == "starting" {
            guard.status.state = "error".into();
        }
    }
}

/// 从任意 stdout 行里摘登录链接（双保险：不依赖 sidecar 的 login_needed 事件
/// ——tsnet 的 printAuthURLLoop 每 5 秒把链接打进日志流，日志行一定能到本进程，
/// 直接从行文本自捕，事件丢了也不怕）。ready/error 之后的行不再改状态。
/// 锁内调用。
fn capture_login_url(guard: &mut TsnetShared, line: &str) {
    if matches!(guard.status.state.as_str(), "ready" | "error" | "off") {
        return;
    }
    let Some(url) = extract_login_url(line) else { return };
    if guard.status.login_url == url {
        return;
    }
    guard.status.login_url = url;
    if guard.status.state == "starting" {
        guard.status.state = "needs_auth".into();
    }
}

/// 摘取行内的 login.tailscale.com 链接（无 regex 依赖：定位前缀后取合法字符）。
fn extract_login_url(line: &str) -> Option<String> {
    const MARK: &str = "https://login.tailscale.com/";
    let start = line.find(MARK)? + MARK.len();
    let tail: String = line[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '?' | '&' | '=' | '.' | '/' | '-' | '_' | '%'))
        .collect();
    if tail.is_empty() {
        None
    } else {
        Some(format!("{MARK}{tail}"))
    }
}

// ==================== sidecar 路径 ====================

/// sidecar 可执行文件三候选（按序找第一个存在的）：
/// 1. exe 目录下 binaries/（tauri bundle.resources 的安装布局，Windows 资源
///    目录 = exe 目录，保留相对子路径）
/// 2. exe 同目录（开发 cargo run 时手动拷 / 旧版手工部署）
/// 3. app_data/tsnet/（用户手动放置位）
fn sidecar_candidates(app: &AppHandle) -> Vec<std::path::PathBuf> {
    let name = if cfg!(windows) { "anyboot-tsnet.exe" } else { "anyboot-tsnet" };
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("binaries").join(name));
            out.push(dir.join(name));
        }
    }
    if let Ok(dir) = app.path().app_data_dir() {
        out.push(dir.join("tsnet").join(name));
    }
    out
}

fn find_sidecar(app: &AppHandle) -> Option<std::path::PathBuf> {
    sidecar_candidates(app).into_iter().find(|p| p.is_file())
}

// ==================== enable / disable / kill_all ====================

/// 阻塞版开启（spawn_blocking 里跑）：spawn sidecar → 接管 stdout/stdin →
/// 等 45s 内到终态（ready/needs_auth/error）。
pub(crate) fn enable_blocking(app: &AppHandle) -> Result<TsnetStatus, String> {
    let Some(sidecar) = find_sidecar(app) else {
        return Err(format!(
            "内置 Tailscale 引擎缺失（{}）—— 重新安装 AnyBoot，或把文件放到数据目录 tsnet/ 下",
            sidecar_candidates(app)
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "anyboot-tsnet.exe".into())
        ));
    };

    let mut guard = shared().lock().map_err(|_| "内置引擎状态锁损坏".to_string())?;
    if child_alive(&mut guard) {
        // 已在跑：直接回当前状态（幂等）
        return Ok(guard.status.clone());
    }
    guard.child = None;
    guard.stdin = None;

    let state_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("定位数据目录失败: {error}"))?
        .join("tsnet")
        .join("state");
    std::fs::create_dir_all(&state_dir).map_err(|error| format!("建状态目录失败: {error}"))?;
    let port = crate::stored_settings(app).http_port;
    let authkey = crate::stored_settings(app).tsnet_authkey;

    let mut command = Command::new(&sidecar);
    command
        .arg("--state")
        .arg(&state_dir)
        .arg("--hostname")
        .arg("anyboot")
        .arg("--upstream")
        .arg(format!("http://127.0.0.1:{port}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动内置引擎失败: {error}"))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let pid = child.id();

    guard.child = Some(child);
    guard.status = TsnetStatus::new("starting");
    guard.status.sidecar_running = true;
    guard.status.message = format!("引擎启动中（pid {pid}）");
    // 新一轮启动：日志缓冲重置（旧日志随旧进程作废）
    guard.logs.clear();
    guard.started_at = Some(Instant::now());
    push_log(&mut guard, &format!("引擎进程已启动（pid {pid}）"));
    push_log(&mut guard, &format!("sidecar: {}", sidecar.display()));
    push_log(&mut guard, &format!("状态目录: {}", state_dir.display()));
    push_log(&mut guard, &format!("反代目标: http://127.0.0.1:{port}"));
    if !authkey.is_empty() {
        push_log(&mut guard, "AuthKey 已从配置行传入（内容不落日志）");
    } else {
        push_log(&mut guard, "未配置 AuthKey：等 sidecar 发登录链接");
    }

    // 配置行走 stdin（authkey 不进命令行）；写失败不致命——sidecar 会走
    // 无 AuthKey 流程（状态目录有凭据时直接连上，没有则发登录链接）。
    if let Some(mut pipe) = stdin {
        let config = serde_json::json!({ "authkey": authkey }).to_string();
        let _ = writeln!(pipe, "{config}");
        let _ = pipe.flush();
        guard.stdin = Some(pipe);
    }

    if let Some(stdout) = stdout {
        std::thread::spawn(move || stdout_worker(stdout));
    }
    drop(guard); // 45s 等待期间放开锁，读线程要更新状态

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(READY_WAIT_SECS);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let mut guard = shared().lock().map_err(|_| "内置引擎状态锁损坏".to_string())?;
        if !child_alive(&mut guard) {
            // 读线程已把死亡翻译成 error 末态
            return Ok(guard.status.clone());
        }
        if matches!(guard.status.state.as_str(), "ready" | "needs_auth" | "error") {
            return Ok(guard.status.clone());
        }
        if std::time::Instant::now() >= deadline {
            let _ = kill_locked(&mut guard);
            guard.status = TsnetStatus::new("error");
            guard.status.message = "启动超时：45 秒内没有等到就绪/登录信号（网络连不上 Tailscale 协调服务器？）".into();
            return Ok(guard.status.clone());
        }
    }
}

/// 锁内收尸：stdin quit → 最多 QUIT_GRACE_SECS → kill。
fn kill_locked(guard: &mut TsnetShared) -> Result<(), String> {
    let Some(mut child) = guard.child.take() else {
        return Ok(());
    };
    if let Some(mut pipe) = guard.stdin.take() {
        let _ = writeln!(pipe, "quit");
        let _ = pipe.flush();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(QUIT_GRACE_SECS);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
                _ => break,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// 阻塞版关闭（spawn_blocking 里跑）：幂等，没收着进程也不报错。
pub(crate) fn disable_blocking() -> TsnetStatus {
    match shared().lock() {
        Ok(mut guard) => {
            let _ = kill_locked(&mut guard);
            guard.status = TsnetStatus::new("off");
            guard.status.message = String::new();
            guard.status.clone()
        }
        Err(_) => TsnetStatus::new("error"),
    }
}

/// 应用退出钩子（lib.rs RunEvent::ExitRequested 里与 hub_gateway::kill_all 并排）：
/// 只收进程，状态无所谓——进程都要退了。
pub(crate) fn kill_all() {
    if let Ok(mut guard) = shared().lock() {
        let _ = kill_locked(&mut guard);
    }
}

// ==================== Tauri 命令 ====================

/// 查询内置引擎状态（纯内存读，无阻塞调用）。
#[tauri::command]
pub(crate) fn tsnet_status() -> TsnetStatus {
    match shared().lock() {
        Ok(mut guard) => {
            guard.status.sidecar_running = child_alive(&mut guard);
            // 进程没了但状态还挂在 starting（读线程还没轮到收尾）：当场纠正
            if !guard.status.sidecar_running && guard.status.state == "starting" {
                guard.status.state = "error".into();
                guard.status.message = "内置引擎已退出".into();
            }
            guard.status.clone()
        }
        Err(_) => TsnetStatus::default(),
    }
}

/// 开启内置引擎（幂等：已在跑直接回当前态）。
/// 子进程 spawn + 45s 等待 → async + spawn_blocking（blocking_guard 约定）。
#[tauri::command]
pub(crate) async fn tsnet_enable(app: AppHandle) -> Result<TsnetStatus, String> {
    tauri::async_runtime::spawn_blocking(move || enable_blocking(&app))
        .await
        .map_err(|error| format!("内置引擎任务失败: {error}"))?
}

/// 关闭内置引擎（幂等）。
#[tauri::command]
pub(crate) async fn tsnet_disable() -> Result<TsnetStatus, String> {
    tauri::async_runtime::spawn_blocking(disable_blocking)
        .await
        .map_err(|error| format!("内置引擎任务失败: {error}"))
}

/// 拉取运行日志（环形缓冲快照，UI「运行日志」框消费；纯内存读）。
#[tauri::command]
pub(crate) fn tsnet_logs() -> Vec<String> {
    match shared().lock() {
        Ok(guard) => guard.logs.iter().cloned().collect(),
        Err(_) => Vec::new(),
    }
}

/// 保存 AuthKey（管理控制台 → Settings → Keys 签发的设备授权密钥）。
/// 留空 = 清除。敏感度对齐 gateway_named_token，只落本机 settings.json。
#[tauri::command]
pub(crate) fn tsnet_set_authkey(app: AppHandle, key: String) -> Result<(), String> {
    let mut settings = crate::stored_settings(&app);
    settings.tsnet_authkey = key.trim().to_string();
    crate::save_settings(&app, &settings).map(|_| ())
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> TsnetStatus {
        TsnetStatus::new("starting")
    }

    #[test]
    fn state_event_ready_sets_url() {
        let mut st = status();
        let done = SidecarEvent {
            event: "state".into(),
            state: "ready".into(),
            funnel_url: "https://anyboot.tail-scale.ts.net".into(),
            login_url: String::new(),
            detail: String::new(),
            t: 0.0,
        }
        .apply(&mut st);
        assert!(done);
        assert_eq!(st.state, "ready");
        assert_eq!(st.url, "https://anyboot.tail-scale.ts.net");
    }

    #[test]
    fn state_event_login_needed_keeps_waiting_process_alive() {
        let mut st = status();
        let done = SidecarEvent {
            event: "state".into(),
            state: "needs_auth".into(),
            funnel_url: String::new(),
            login_url: "https://login.tailscale.com/a/abc123".into(),
            detail: String::new(),
            t: 0.0,
        }
        .apply(&mut st);
        assert!(done); // enable 的等待到此返回，但 sidecar 继续驻留等登录
        assert_eq!(st.state, "needs_auth");
        assert_eq!(st.login_url, "https://login.tailscale.com/a/abc123");
    }

    #[test]
    fn starting_event_is_not_terminal() {
        let mut st = status();
        let done = SidecarEvent {
            event: "state".into(),
            state: "starting".into(),
            funnel_url: String::new(),
            login_url: String::new(),
            detail: String::new(),
            t: 0.0,
        }
        .apply(&mut st);
        assert!(!done);
        assert_eq!(st.state, "starting");
    }

    #[test]
    fn error_event_carries_detail() {
        let mut st = status();
        let done = SidecarEvent {
            event: "state".into(),
            state: "error".into(),
            funnel_url: String::new(),
            login_url: String::new(),
            detail: "开启 Funnel 失败: denied".into(),
            t: 0.0,
        }
        .apply(&mut st);
        assert!(done);
        assert_eq!(st.state, "error");
        assert_eq!(st.message, "开启 Funnel 失败: denied");
    }

    #[test]
    fn event_parses_real_sidecar_line_shape() {
        // 与 sidecar-tsnet/main.go emit 的 JSON 键名逐字对齐（含 camelCase 的 loginUrl）
        let ev: SidecarEvent = serde_json::from_str(
            r#"{"event":"state","state":"ready","url":"https://anyboot.x.ts.net"}"#,
        )
        .unwrap();
        assert_eq!(ev.funnel_url, "https://anyboot.x.ts.net");
        let ev: SidecarEvent = serde_json::from_str(
            r#"{"event":"state","state":"login_needed","loginUrl":"https://login.tailscale.com/a/xyz"}"#,
        )
        .unwrap();
        assert_eq!(ev.login_url, "https://login.tailscale.com/a/xyz");
        let ev: SidecarEvent =
            serde_json::from_str(r#"{"event":"hello","proto":1,"sidecar":"anyboot-tsnet/1.0.0"}"#)
                .unwrap();
        assert_eq!(ev.event, "hello");
    }

    #[test]
    fn default_status_is_off() {
        let st = TsnetStatus::default();
        assert_eq!(st.state, "off");
        assert!(!st.sidecar_running);
        assert!(st.url.is_empty());
    }

    #[test]
    fn log_event_does_not_change_state_and_stamps_prefix() {
        let mut st = status();
        let raw = r#"{"event":"log","t":3.2,"detail":"tsnet 上线成功（ipn.Running）"}"#;
        let ev: SidecarEvent = serde_json::from_str(raw).unwrap();
        assert!((ev.t - 3.2).abs() < 1e-9);
        let done = ev.apply(&mut st);
        assert!(!done);
        assert_eq!(st.state, "starting"); // 状态机不动
        assert_eq!(ev.log_text().as_deref(), Some("tsnet 上线成功（ipn.Running）"));
    }

    #[test]
    fn login_needed_from_real_sidecar_normalizes_to_needs_auth() {
        // 回归：sidecar 实际发的状态名是 login_needed，归一成 needs_auth 并视作
        // 终态（否则 45s 等待超时把引擎 quit 掉，UI 永远不出登录按钮）。
        let mut st = status();
        let ev: SidecarEvent = serde_json::from_str(
            r#"{"event":"state","state":"login_needed","t":0.3,"loginUrl":"https://login.tailscale.com/a/1dbb9bb401"}"#,
        )
        .unwrap();
        assert!(ev.apply(&mut st));
        assert_eq!(st.state, "needs_auth");
        assert_eq!(st.login_url, "https://login.tailscale.com/a/1dbb9bb401");
        // 日志文案 = 登录链接本身（needs_auth 态展示用）
        assert_eq!(ev.log_text().as_deref(), Some("https://login.tailscale.com/a/1dbb9bb401"));
    }

    #[test]
    fn extract_login_url_from_plain_log_line() {
        // 真实 tsnet 日志行（printAuthURLLoop 每 5s 打一遍）——双保险自捕用
        let line = "To start this tsnet server, restart with TS_AUTHKEY set, or go to: https://login.tailscale.com/a/f38e86d0112a0";
        assert_eq!(
            extract_login_url(line).as_deref(),
            Some("https://login.tailscale.com/a/f38e86d0112a0")
        );
        assert_eq!(extract_login_url("no url here"), None);
        assert_eq!(
            extract_login_url("https://login.tailscale.com/"),
            None // 前缀后面没内容不算
        );
    }

    #[test]
    fn push_log_caps_ring_buffer() {
        let mut guard = TsnetShared::default();
        for i in 0..(LOG_CAP + 20) {
            push_log(&mut guard, &format!("line-{i}"));
        }
        assert_eq!(guard.logs.len(), LOG_CAP);
        assert_eq!(guard.logs.front().unwrap(), &format!("line-{}", LOG_CAP + 19 - LOG_CAP + 1));
        assert_eq!(guard.logs.back().unwrap(), &format!("line-{}", LOG_CAP + 19));
    }
}
