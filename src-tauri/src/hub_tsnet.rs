//! AnyBoot 内置 Tailscale 引擎（tsnet sidecar）——免装 Tailscale 客户端的
//! Funnel 公网通道（gateway 页「Tailscale (内置)」标签的数据源）。
//!
//! 架构（2026-10-24 定稿，见 design/tsnet-p1-todo.md）：
//!   - Go sidecar `anyboot-tsnet`（源码 src-tauri/sidecar-tsnet/，clean-room
//!     手写，只依赖 tailscale.com/tsnet 公开 API；构建见 scripts/build-sidecar.sh，
//!     unix 产物名 anyboot-tsnet-{linux|darwin}-{amd64|arm64}，Windows = .exe）：tsnet 起 Funnel（FunnelOnly）
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
//!
//! 登录链接（2026-10-24 重做，对齐 tsnet 官方语义）：sidecar 在 Server.UserLogf
//! 回调里用正则抠 `https://login.tailscale.com/...`，做官方域白名单校验后发独立
//! 的 `login` 事件；链接轮换（过期换新）时再发一条。本模块收到即转 needs_auth，
//! 并把链接挂在 TsnetStatus.login_url 上（序列化成 loginUrl 给前端）。

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
    /// 官方 Funnel 开通/修复页（error 时可能非空：权限没开 / 证书没开）。
    /// 序列化成 setupUrl，前端「去开通 Funnel」按钮消费。
    pub setup_url: String,
    /// 错误码（error 时非空：funnel_permission_required / funnel_https_required ...）
    /// 序列化成 errorCode，前端据此改按钮文案（开 Funnel 还是开 HTTPS 证书）。
    pub error_code: String,
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
            setup_url: String::new(),
            error_code: String::new(),
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
    /// 错误码（error 事件；funnel_permission_required / funnel_https_required / ...）
    #[serde(default)]
    code: String,
    /// 官方开通/修复页（error 事件可能带；sidecar 已做官方域白名单）
    #[serde(default, rename = "setupUrl")]
    setup_url: String,
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
            "log" => {
                // 双保险：sidecar 新版已在源头过滤底层网络噪音（bootstrapDNS
                // 刷屏），这里再拦一次，兜住旧 sidecar 二进制还在跑的场景
                if self.detail.contains("trying bootstrapDNS") {
                    return None;
                }
                Some(self.detail.clone())
            }
            "bye" => Some("sidecar 正常退出".into()),
            "hello" => None,
            // 独立 login 事件（链接首次出现 / 轮换时各一条）：UI 上已有链接原文，
            // 这里只留一条里程碑，方便日志排障时看出「几点拿到的」
            "login" => Some(if self.login_url.is_empty() {
                "拿到登录链接但内容为空".into()
            } else {
                format!(
                    "登录链接就绪（浏览器打开完成授权）: {} —— 授权后引擎要初始化环境资源（数分钟），就绪后会打印公网地址",
                    self.login_url
                )
            }),
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
            // login：sidecar 从 Server.UserLogf 里抠出来的官方登录链接（权威通道）。
            // 链接会轮换（过期后 control 下发新的），所以每次都更新，不能只认第一条。
            // needs_auth 是驻留态：sidecar 继续等浏览器授权，登录完成后自己转 ready。
            "login" => {
                if !self.login_url.is_empty() {
                    status.login_url = self.login_url.clone();
                }
                if status.state != "ready" && status.state != "error" {
                    status.state = "needs_auth".into();
                    status.message = String::new();
                }
                true
            }
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
                if !self.setup_url.is_empty() {
                    status.setup_url = self.setup_url.clone();
                }
                if !self.code.is_empty() {
                    status.error_code = self.code.clone();
                }
                status.message = if self.detail.is_empty() && self.state == "error" && !self.code.is_empty()
                {
                    format!("内置引擎启动失败（{}）", self.code)
                } else {
                    self.detail.clone()
                };
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

/// unix 平台的 sidecar 文件名（带 OS+arch 后缀：mac/linux 包里同时放两种架构，
/// 且 macOS 与 Linux 的同名架构不能混——文件名里必须带 OS）。Windows 沿用历史单文件。
fn sidecar_filename() -> String {
    if cfg!(windows) {
        "anyboot-tsnet.exe".to_string()
    } else {
        let os = if cfg!(target_os = "macos") { "darwin" } else { "linux" };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        };
        format!("anyboot-tsnet-{os}-{arch}")
    }
}

/// sidecar 可执行文件四候选（按序找第一个存在的）：
/// 1. 资源目录 binaries/（tauri bundle.resources 的标准落位：Windows = exe 目录、
///    macOS = .app/Contents/Resources、Linux = /usr/lib/<identifier>，相对子路径保留）
/// 2. exe 目录下 binaries/（NSIS 便携布局 / 开发环境手动拷）
/// 3. exe 同目录（开发 cargo run 时手动拷 / 旧版手工部署）
/// 4. app_data/tsnet/（用户手动放置位）
fn sidecar_candidates(app: &AppHandle) -> Vec<std::path::PathBuf> {
    let name = sidecar_filename();
    let mut out = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        out.push(dir.join("binaries").join(&name));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("binaries").join(&name));
            out.push(dir.join(&name));
        }
    }
    if let Ok(dir) = app.path().app_data_dir() {
        out.push(dir.join("tsnet").join(name));
    }
    out
}

/// 打包/解压链路可能丢掉执行位（macOS zip、deb 复制等），spawn 前兜底补 0o755。
#[cfg(unix)]
fn ensure_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.permissions().mode() & 0o111 != 0o111 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
        }
    }
}
#[cfg(not(unix))]
fn ensure_executable(_path: &std::path::Path) {}

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
                .unwrap_or_else(|| sidecar_filename())
        ));
    };
    ensure_executable(&sidecar);

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

/// 启动时的内置引擎自启：勾了「启动软件时自动开启」就跑一次 enable。
/// 凭据在状态目录里，多数情况几秒就 ready；没凭据则停在 needs_auth 等用户登录，
/// 与手点「引擎」开关同一套流程（失败原因进日志环形缓冲，不弹窗打扰）。
pub(crate) fn autostart_engine(app: AppHandle) {
    if !crate::stored_settings(&app).tsnet_autostart {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let _ = tsnet_enable(app).await;
    });
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

/// 清空运行日志（UI「清空」按钮：源头清掉 + 屏幕清屏，下次拉取不再恢复）。
#[tauri::command]
pub(crate) fn tsnet_logs_clear() {
    if let Ok(mut guard) = shared().lock() {
        guard.logs.clear();
    }
}

/// 卸载内置引擎。引擎本体随 AnyBoot 安装包自带（恒属「通过 AnyBoot 安装」），
/// 卸载 = 停引擎 + 删状态目录（登录凭据）+ 删数据目录里手动放置的 sidecar 副本；
/// 资源目录里的引擎是 AnyBoot 本体的一部分不动——重新点「引擎」开关即恢复可用。
/// 子进程与删目录阻塞 → async + spawn_blocking（blocking_guard 约定）。
///
/// `keep_data`：是否保留本机数据（状态目录里的节点凭据 + AuthKey + 自启开关）。
/// 默认 **false = 一起清掉**；勾上则只停引擎、清引擎副本，登录态留着（重开即 ready）。
#[tauri::command]
pub(crate) async fn tsnet_uninstall(
    app: AppHandle,
    keep_data: Option<bool>,
) -> Result<TsnetStatus, String> {
    let keep_data = keep_data.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || tsnet_uninstall_blocking(&app, keep_data))
        .await
        .map_err(|error| format!("卸载任务失败: {error}"))?
}

pub(crate) fn tsnet_uninstall_blocking(
    app: &AppHandle,
    keep_data: bool,
) -> Result<TsnetStatus, String> {
    // 停引擎；不保留数据时连状态目录（登录凭据）一起删（logout_blocking 的既有语义）
    let mut status = if keep_data {
        disable_blocking()
    } else {
        logout_blocking(app)?
    };
    // 手动放置位的 sidecar 副本一并清掉（resource_dir 里的属于 AnyBoot 本体，不动）
    if let Ok(dir) = app.path().app_data_dir() {
        let ts_dir = dir.join("tsnet");
        if let Ok(entries) = std::fs::read_dir(&ts_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("anyboot-tsnet") && entry.path().is_file() {
                    let outcome = match std::fs::remove_file(entry.path()) {
                        Ok(()) => format!("[卸载] 已删手动放置的引擎副本: {name}"),
                        Err(error) => format!("[卸载] 引擎副本 {name} 删除失败: {error}"),
                    };
                    if let Ok(mut guard) = shared().lock() {
                        push_log(&mut guard, &outcome);
                    }
                }
            }
        }
    }
    if keep_data {
        if let Ok(mut guard) = shared().lock() {
            push_log(&mut guard, "[卸载] 按「保留本机数据」卸载：状态目录与登录凭据保留");
        }
        status.message =
            "内置引擎已卸载（按「保留本机数据」：状态目录与登录凭据保留，重开即恢复）。引擎本体随 AnyBoot 自带"
                .into();
        return Ok(status);
    }
    // AuthKey 也是凭据；自启开关留着会在下次启动空跑一次
    let _ = crate::settings_commands::update_settings_and_publish(
        app,
        "tsnet_uninstall",
        "内置引擎：清理 AuthKey 与自启",
        |settings| {
            settings.tsnet_authkey.clear();
            settings.tsnet_autostart = false;
        },
        |_| {},
    );
    status.message = "内置引擎已卸载：状态与登录凭据已删除。引擎本体随 AnyBoot 自带，重新开启开关即可恢复".into();
    Ok(status)
}

/// 阻塞版清除登录凭据（spawn_blocking 里跑）：停 sidecar → 删整个 tsnet
/// 状态目录（tailscaled.state 里的节点凭据就在其中）→ 重建空目录 → 状态归 off。
/// 下次开启引擎等于全新节点，要走浏览器登录或新 AuthKey。
pub(crate) fn logout_blocking(app: &AppHandle) -> Result<TsnetStatus, String> {
    let state_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("定位数据目录失败: {error}"))?
        .join("tsnet")
        .join("state");

    {
        let mut guard = shared().lock().map_err(|_| "内置引擎状态锁损坏".to_string())?;
        let _ = kill_locked(&mut guard);
        guard.logs.clear();
        guard.started_at = None;
    }

    if state_dir.exists() {
        std::fs::remove_dir_all(&state_dir)
            .map_err(|error| format!("删除凭据目录失败（文件被占用？稍后重试）: {error}"))?;
    }
    std::fs::create_dir_all(&state_dir).map_err(|error| format!("重建状态目录失败: {error}"))?;

    let mut guard = shared().lock().map_err(|_| "内置引擎状态锁损坏".to_string())?;
    guard.status = TsnetStatus::new("off");
    guard.status.message = "登录凭据已清除，下次开启需重新授权".into();
    push_log(&mut guard, "登录凭据已清除（状态目录已删并重建）");
    Ok(guard.status.clone())
}

/// 清除远程登录认证信息（管理控制台里对应设备的授权；tailnet 后台的设备
/// 记录不受影响，需自行在管理台移除）。
/// **顺带清 AuthKey**：只删状态目录而留着密钥的话，下次开启引擎又会拿它自动
/// 注册一个新节点，用户点完「清除登录凭据」却看见自己又上线了 —— 与 Tailscale
/// 直连的 logout 同一口径。
#[tauri::command]
pub(crate) async fn tsnet_logout(app: AppHandle) -> Result<TsnetStatus, String> {
    let settings_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut status = logout_blocking(&app)?;
        let had_key = !crate::stored_settings(&settings_app).tsnet_authkey.trim().is_empty();
        if had_key {
            let _ = crate::settings_commands::update_settings_and_publish(
                &settings_app,
                "tsnet_logout",
                "内置引擎：清除 AuthKey",
                |settings| settings.tsnet_authkey.clear(),
                |_| {},
            );
            if let Ok(mut guard) = shared().lock() {
                push_log(&mut guard, "已一并清除保存的 AuthKey（否则下次开启会自动重登）");
            }
            status.message = "登录凭据与 AuthKey 已清除，下次开启需重新授权".into();
        }
        Ok(status)
    })
    .await
    .map_err(|error| format!("内置引擎任务失败: {error}"))?
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

    /// 单测统一走「真实 JSON 行」：既验状态机，也验 sidecar 与 Rust 的字段名对齐
    /// （camelCase 的 loginUrl / setupUrl 曾因两边对不上而静默丢字段）。
    fn ev(line: &str) -> SidecarEvent {
        serde_json::from_str(line).expect("sidecar 事件行必须能解析")
    }

    #[test]
    fn state_event_ready_sets_url() {
        let mut st = status();
        let event = ev(r#"{"event":"state","state":"ready","t":12.4,"url":"https://anyboot.tail-scale.ts.net"}"#);
        assert!(event.apply(&mut st));
        assert_eq!(st.state, "ready");
        assert_eq!(st.url, "https://anyboot.tail-scale.ts.net");
        assert_eq!(
            event.log_text().as_deref(),
            Some("状态: ready，Funnel 地址 https://anyboot.tail-scale.ts.net")
        );
    }

    #[test]
    fn login_event_enters_needs_auth_with_link() {
        // sidecar 从 Server.UserLogf 抠出链接后发的独立 login 事件
        let mut st = status();
        let event = ev(r#"{"event":"login","loginUrl":"https://login.tailscale.com/a/1dbb9bb401","t":0.9}"#);
        assert!(event.apply(&mut st)); // 终态：enable 的等待到此返回，sidecar 继续驻留
        assert_eq!(st.state, "needs_auth");
        assert_eq!(st.login_url, "https://login.tailscale.com/a/1dbb9bb401");
        assert!(event.log_text().unwrap().contains("登录链接就绪"));
    }

    #[test]
    fn login_event_rotates_link() {
        // 链接过期后 control 下发新的：必须跟着换（曾只认第一条，UI 永远停在废链上）
        let mut st = status();
        let first = ev(r#"{"event":"login","loginUrl":"https://login.tailscale.com/a/old","t":1.0}"#);
        first.apply(&mut st);
        assert_eq!(st.login_url, "https://login.tailscale.com/a/old");
        let second = ev(r#"{"event":"login","loginUrl":"https://login.tailscale.com/a/new","t":640.0}"#);
        second.apply(&mut st);
        assert_eq!(st.state, "needs_auth");
        assert_eq!(st.login_url, "https://login.tailscale.com/a/new");
    }

    #[test]
    fn late_login_event_does_not_downgrade_ready() {
        // 极端时序：ready 之后又飘来一条 login（不该把状态打回 needs_auth）
        let mut st = status();
        ev(r#"{"event":"state","state":"ready","url":"https://anyboot.x.ts.net"}"#).apply(&mut st);
        ev(r#"{"event":"login","loginUrl":"https://login.tailscale.com/a/zzz"}"#).apply(&mut st);
        assert_eq!(st.state, "ready");
        assert_eq!(st.url, "https://anyboot.x.ts.net");
    }

    #[test]
    fn legacy_login_needed_state_event_still_normalizes() {
        // 回归：老协议形态（state=login_needed 带 loginUrl）仍要认，归一成 needs_auth
        let mut st = status();
        let event = ev(r#"{"event":"state","state":"login_needed","t":0.3,"loginUrl":"https://login.tailscale.com/a/1dbb9bb401"}"#);
        assert!(event.apply(&mut st));
        assert_eq!(st.state, "needs_auth");
        assert_eq!(st.login_url, "https://login.tailscale.com/a/1dbb9bb401");
    }

    #[test]
    fn starting_event_is_not_terminal() {
        let mut st = status();
        let event = ev(r#"{"event":"state","state":"starting","t":0.0}"#);
        assert!(!event.apply(&mut st));
        assert_eq!(st.state, "starting");
    }

    #[test]
    fn error_event_carries_detail() {
        let mut st = status();
        let event = ev(r#"{"event":"state","state":"error","detail":"tsnet 上线失败: context canceled","t":45.2}"#);
        assert!(event.apply(&mut st));
        assert_eq!(st.state, "error");
        assert_eq!(st.message, "tsnet 上线失败: context canceled");
        assert!(st.setup_url.is_empty());
    }

    #[test]
    fn funnel_permission_error_carries_setup_url() {
        // Funnel 权限没开：sidecar 给错误码 + 官方开通页，UI 出「去开通 Funnel」
        let mut st = status();
        let event = ev(r#"{"event":"state","state":"error","code":"funnel_permission_required","setupUrl":"https://tailscale.com/s/no-funnel","detail":"这个 tailnet 还没开启 Funnel（Funnel not available; https://tailscale.com/s/no-funnel）","t":6.0}"#);
        assert!(event.apply(&mut st));
        assert_eq!(st.state, "error");
        assert_eq!(st.setup_url, "https://tailscale.com/s/no-funnel");
        assert_eq!(st.error_code, "funnel_permission_required");
        assert!(st.message.contains("Funnel"));
        assert_eq!(event.log_text().as_deref(), Some(event.detail.as_str()));
    }

    #[test]
    fn error_event_without_detail_falls_back_to_code() {
        let mut st = status();
        ev(r#"{"event":"state","state":"error","code":"tailscale_dns_missing"}"#).apply(&mut st);
        assert_eq!(st.message, "内置引擎启动失败（tailscale_dns_missing）");
    }

    #[test]
    fn event_parses_real_sidecar_line_shape() {
        // 与 sidecar-tsnet/main.go emit 的 JSON 键名逐字对齐
        let event = ev(r#"{"event":"state","state":"ready","url":"https://anyboot.x.ts.net"}"#);
        assert_eq!(event.funnel_url, "https://anyboot.x.ts.net");
        let event = ev(r#"{"event":"login","loginUrl":"https://login.tailscale.com/a/xyz","t":2.5}"#);
        assert_eq!(event.login_url, "https://login.tailscale.com/a/xyz");
        assert!((event.t - 2.5).abs() < 1e-9);
        let event = ev(r#"{"event":"hello","proto":1,"sidecar":"anyboot-tsnet/1.0.0"}"#);
        assert_eq!(event.event, "hello");
        assert!(event.log_text().is_none());
    }

    #[test]
    fn serialized_status_uses_camel_case_for_ui() {
        // 前端读的就是 s.loginUrl / s.setupUrl（camelCase）—— 这一条防止再改回
        // snake_case 让链接在页面上静默变空。
        let mut st = status();
        st.login_url = "https://login.tailscale.com/a/1".into();
        st.setup_url = "https://tailscale.com/s/https".into();
        st.error_code = "funnel_https_required".into();
        let json = serde_json::to_string(&st).unwrap();
        assert!(json.contains("\"loginUrl\":"));
        assert!(json.contains("\"setupUrl\":"));
        assert!(json.contains("\"errorCode\":"));
        assert!(!json.contains("login_url"));
    }

    #[test]
    fn default_status_is_off() {
        let st = TsnetStatus::default();
        assert_eq!(st.state, "off");
        assert!(!st.sidecar_running);
        assert!(st.url.is_empty());
        assert!(st.login_url.is_empty());
        assert!(st.setup_url.is_empty());
    }

    #[test]
    fn log_event_does_not_change_state_and_stamps_prefix() {
        let mut st = status();
        let event = ev(r#"{"event":"log","t":3.2,"detail":"tsnet 上线成功（ipn.Running）"}"#);
        assert!((event.t - 3.2).abs() < 1e-9);
        assert!(!event.apply(&mut st));
        assert_eq!(st.state, "starting"); // 状态机不动
        assert_eq!(event.log_text().as_deref(), Some("tsnet 上线成功（ipn.Running）"));
    }

    #[test]
    fn log_event_filters_bootstrap_dns_noise() {
        // 底层网络重试噪音（每条内容都不同，重复抑制抓不住）：Rust 侧双保险
        // 过滤（sidecar 新版已源头丢弃，这里兜旧二进制还在跑的场景）
        let noise = ev(
            r#"{"event":"log","t":41.0,"detail":"trying bootstrapDNS(\"derp8c.tailscale.com\", \"206.189.16.32\") for \"log.tailscale.io\""}"#,
        );
        assert_eq!(noise.log_text(), None);
        // 正常日志不受影响
        let keep = ev(r#"{"event":"log","t":42.0,"detail":"Funnel :443 监听已建立"}"#);
        assert!(keep.log_text().is_some());
    }

    #[test]
    fn login_event_log_text_carries_init_hint() {
        // 登录里程碑要带「初始化需几分钟」的预期管理（用户曾误以为卡死）
        let event = ev(r#"{"event":"login","loginUrl":"https://login.tailscale.com/a/xyz","t":2.5}"#);
        let text = event.log_text().expect("login 应产出日志文本");
        assert!(text.contains("登录链接就绪"));
        assert!(text.contains("初始化环境资源"));
        assert!(text.contains("公网地址"));
    }

    #[test]
    fn extract_login_url_from_plain_log_line() {
        // 兜底自捕：sidecar 若把链接混在普通日志里，也要能从行文本里捞出来
        let line = "To start this tsnet server, restart with TS_AUTHKEY set, or go to: https://login.tailscale.com/a/f38e86d0112a0";
        assert_eq!(
            extract_login_url(line).as_deref(),
            Some("https://login.tailscale.com/a/f38e86d0112a0")
        );
        assert_eq!(extract_login_url("no url here"), None);
        assert_eq!(extract_login_url("https://login.tailscale.com/"), None);
        // 非官方域一律不认（防止把别处的链接当登录页打开）
        assert_eq!(extract_login_url("https://evil.example/https://login.tailscale.com/"), None);
    }

    #[test]
    fn capture_login_url_flips_starting_to_needs_auth() {
        let mut guard = TsnetShared::default();
        guard.status = status();
        capture_login_url(
            &mut guard,
            r#"{"event":"log","detail":"go to: https://login.tailscale.com/a/abc123"}"#,
        );
        assert_eq!(guard.status.state, "needs_auth");
        assert_eq!(guard.status.login_url, "https://login.tailscale.com/a/abc123");
        // ready 之后的行不再改状态
        guard.status.state = "ready".into();
        capture_login_url(&mut guard, "https://login.tailscale.com/a/late");
        assert_eq!(guard.status.login_url, "https://login.tailscale.com/a/abc123");
    }

    #[test]
    fn push_log_caps_ring_buffer() {
        let mut guard = TsnetShared::default();
        for i in 0..(LOG_CAP + 20) {
            push_log(&mut guard, &format!("line-{i}"));
        }
        assert_eq!(guard.logs.len(), LOG_CAP);
        assert_eq!(guard.logs.front().unwrap(), "line-20");
        assert_eq!(guard.logs.back().unwrap(), &format!("line-{}", LOG_CAP + 19));
    }
}
