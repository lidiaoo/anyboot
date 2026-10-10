use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const SOURCE_FILES: [(&str, &str, &str); 3] = [
    ("linux", "Linux", "PreviousBoot-linux"),
    ("windows", "Windows", "PreviousBoot-windows"),
    ("macos", "macOS", "PreviousBoot-mac"),
];
const ESP_PARTTYPE: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
const FAT_FILESYSTEMS: [&str; 6] = ["vfat", "fat", "fat12", "fat16", "fat32", "exfat"];
mod cli;
mod http;

/// 二进制入口（`main.rs`）用这两个：带参数走 CLI，不带参数开 GUI。
pub use cli::{is_cli_invocation as cli_requested, run as run_cli};

const SETTINGS_FILE: &str = "settings.json";
/// 内置 HTTP 服务的默认监听地址与端口（plan §4.1：默认开启、0.0.0.0:12300）
const DEFAULT_HTTP_BIND: &str = "0.0.0.0";
const DEFAULT_HTTP_PORT: u16 = 12300;
/// 单次请求体上限：超过就只认 header/query（防止拿巨大 body 拖住工作线程）
const MAX_REQUEST_BODY: usize = 8 * 1024;
/// 「最近请求」在内存里保留的条数
const MAX_RECENT_REQUESTS: usize = 20;
/// 通知弹窗的窗口标签
const NOTIFY_WINDOW_LABEL: &str = "notify";
/// 延迟秒数的上限（配置里乱填也不会把请求挂到天上去）
const MAX_DELAY_SECONDS: u32 = 600;
/// 自定义 token 的长度范围（太短不安全，太长粘贴起来也难受）
const MIN_TOKEN_LEN: usize = 4;
const MAX_TOKEN_LEN: usize = 128;
/// 内置提示音（KakaoTalk 通知音）落到配置目录后的文件名
const SOUND_DIR: &str = "sounds";
const BUILTIN_SOUND_FILE: &str = "notify.wav";
/// 内置提示音本体（WAV PCM，编进二进制 —— 不依赖打包资源路径，AppImage/NSIS 都稳）
const BUILTIN_SOUND: &[u8] = include_bytes!("../assets/kakaotalk-notify.wav");
/// 主题可取值
const THEMES: [&str; 3] = ["dark", "light", "system"];
/// 不透明度范围（百分比）
const MIN_OPACITY: u8 = 50;
const MAX_OPACITY: u8 = 100;
/// 阶段 0 的取证日志：只追加、不参与任何逻辑判断，写在应用配置目录。
/// 里面**永不写入** httpToken（只记前 8 位指纹）。
const TRACE_FILE: &str = "trace.log";
const MAIN_WINDOW_LABEL: &str = "main";
/// 快照版本号：每次发布 +1。前端只在 revision 变大时重绘（阶段 1 修 D4）。
static UI_REVISION: AtomicU64 = AtomicU64::new(0);
/// **唯一**的状态事件：携带完整快照 + 单调递增 revision。
///
/// 阶段 1 之前是 status-changed / settings-changed / restart-result 三个补丁式事件，
/// 各更新 UI 的一角、没有版本号，任何一次漏发或乱序都会留下不刷新的一角。
const UI_STATE_CHANGED_EVENT: &str = "ui-state-changed";

#[derive(Default)]
struct AppState {
    vars_dir: Option<PathBuf>,
    explicit_dir: Option<PathBuf>,
    active_mount: Option<ActiveMount>,
    variables: Vec<VariableInfo>,
    message: String,
    error: Option<String>,
}

#[derive(Clone)]
struct ActiveMount {
    device: PathBuf,
    mountpoint: PathBuf,
    owned: bool,
}

impl ActiveMount {
    fn unmount(self) -> io::Result<()> {
        if !self.owned {
            return Ok(());
        }
        mount_command("umount")
            .arg(&self.mountpoint)
            .status()
            .and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other("umount failed"))
                }
            })?;
        let _ = fs::remove_dir(&self.mountpoint);
        Ok(())
    }

    fn remount(&self, mode: &str) -> io::Result<()> {
        let output = mount_command("mount")
            .arg("-o")
            .arg(remount_options(mode))
            .arg(&self.device)
            .arg(&self.mountpoint)
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim(),
            ))
        }
    }
}

#[cfg(target_os = "linux")]
fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(target_os = "linux")]
fn mount_command(program: &str) -> Command {
    if is_root() {
        Command::new(program)
    } else {
        let mut command = Command::new("pkexec");
        command.arg(program);
        command
    }
}

#[cfg(not(target_os = "linux"))]
fn mount_command(program: &str) -> Command {
    Command::new(program)
}

#[cfg(target_os = "linux")]
fn linux_user_home_by_name(username: &str) -> Option<PathBuf> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.first() != Some(&username) {
            return None;
        }
        Some(PathBuf::from(fields.get(5)?.to_string()))
    })
}

#[cfg(target_os = "linux")]
fn linux_user_home_by_uid(uid: u32) -> Option<PathBuf> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.get(2).and_then(|value| value.parse::<u32>().ok()) != Some(uid) {
            return None;
        }
        Some(PathBuf::from(fields.get(5)?.to_string()))
    })
}

#[cfg(target_os = "linux")]
fn linux_invoking_user_home() -> Option<PathBuf> {
    if let Some(username) = std::env::var_os("SUDO_USER") {
        return linux_user_home_by_name(&username.to_string_lossy());
    }
    if let Ok(uid) = std::env::var("PKEXEC_UID") {
        return uid.parse::<u32>().ok().and_then(linux_user_home_by_uid);
    }
    None
}

#[cfg(target_os = "linux")]
fn linux_mount_user_ids() -> Option<(u32, u32)> {
    if is_root() {
        return linux_invoking_user_home()
            .as_deref()
            .and_then(linux_user_ids_from_home);
    }
    Some(unsafe { (libc::getuid(), libc::getgid()) })
}

#[cfg(target_os = "linux")]
fn mount_options(mode: &str) -> String {
    match linux_mount_user_ids() {
        Some((uid, gid)) => format!("{mode},uid={uid},gid={gid}"),
        None => mode.to_string(),
    }
}

#[cfg(target_os = "linux")]
fn remount_options(mode: &str) -> String {
    format!("remount,{}", mount_options(mode))
}

#[cfg(not(target_os = "linux"))]
fn remount_options(mode: &str) -> String {
    format!("remount,{mode}")
}

#[cfg(target_os = "linux")]
fn linux_user_ids_from_home(home: &Path) -> Option<(u32, u32)> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.get(5) != Some(&home.to_string_lossy().as_ref()) {
            return None;
        }
        Some((fields.get(2)?.parse().ok()?, fields.get(3)?.parse().ok()?))
    })
}

#[cfg(not(target_os = "linux"))]
fn mount_options(mode: &str) -> String {
    mode.to_string()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VariableInfo {
    system: String,
    display_name: String,
    source_name: String,
    source: PathBuf,
    exists: bool,
    active: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppStatus {
    vars_dir: Option<PathBuf>,
    temporary_mount: bool,
    variables: Vec<VariableInfo>,
    message: String,
    error: Option<String>,
}

/// 远程渠道：http（webhook / curl / 面板）与 cli（ssh / CLI 转发）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Channel {
    Http,
    Cli,
}

impl Channel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Cli => "cli",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Http => "HTTP",
            Self::Cli => "SSH/CLI",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "http" | "webhook" => Some(Self::Http),
            "cli" | "ssh" => Some(Self::Cli),
            _ => None,
        }
    }
}

/// 单个渠道的通知与延迟（Q14：ssh/CLI 与 http webhook 分开配置）。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct ChannelNotify {
    notify: bool,
    delay_seconds: u32,
}

impl Default for ChannelNotify {
    fn default() -> Self {
        Self {
            notify: true,
            delay_seconds: 5,
        }
    }
}

impl ChannelNotify {
    fn clamped_delay(&self) -> u32 {
        self.delay_seconds.min(MAX_DELAY_SECONDS)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct Channels {
    http: ChannelNotify,
    cli: ChannelNotify,
}

impl Default for Channels {
    fn default() -> Self {
        Self {
            http: ChannelNotify::default(),
            cli: ChannelNotify::default(),
        }
    }
}

impl Channels {
    fn for_channel(&self, channel: Channel) -> &ChannelNotify {
        match channel {
            Channel::Http => &self.http,
            Channel::Cli => &self.cli,
        }
    }

    fn for_channel_mut(&mut self, channel: Channel) -> &mut ChannelNotify {
        match channel {
            Channel::Http => &mut self.http,
            Channel::Cli => &mut self.cli,
        }
    }

    /// 托盘里的「远程执行延迟」预设：同时作用于两条渠道（细分去主窗口）。
    fn set_delay_all(&mut self, seconds: u32) {
        self.http.delay_seconds = seconds;
        self.cli.delay_seconds = seconds;
    }
}

/// 一条远程命令（HTTP 端点与 CLI 转发共用同一份语义）。
#[derive(Clone, Debug, PartialEq, Eq)]
enum RemoteCommand {
    Switch(String),
    Power(PowerAction),
    SwitchAndPower { system: String, power: PowerAction },
}

impl RemoteCommand {
    fn summary(&self) -> String {
        match self {
            Self::Switch(system) => format!("切换下次启动为 {}", system_display_name(system)),
            Self::Power(action) => action.label().to_string(),
            Self::SwitchAndPower { system, power } => {
                format!(
                    "切换下次启动为 {}，然后{}",
                    system_display_name(system),
                    power.label()
                )
            }
        }
    }
}

fn system_display_name(system: &str) -> String {
    SOURCE_FILES
        .iter()
        .find(|(candidate, _, _)| *candidate == system)
        .map(|(_, display_name, _)| (*display_name).to_string())
        .unwrap_or_else(|| system.to_string())
}

/// 一条待执行请求（延迟窗口内可取消）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingRequest {
    id: String,
    channel: Channel,
    source: String,
    summary: String,
    command: RemoteCommand,
    seconds: u64,
    deadline: Instant,
    created: String,
}

impl PendingRequest {
    fn seconds_left(&self) -> u64 {
        self.deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
    }
}

/// 「最近请求」列表里的一条。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestLogEntry {
    time: String,
    source: String,
    channel: &'static str,
    path: String,
    summary: String,
    /// pending / executed / cancelled / failed / rejected
    status: &'static str,
    message: String,
}

/// 通知弹窗要显示的东西（进快照，弹窗窗口和主窗口读同一份）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotifyView {
    /// pending（倒计时，可取消）或 info（已执行的事后提示）
    kind: &'static str,
    id: Option<String>,
    title: String,
    message: String,
    source: String,
    channel: &'static str,
    seconds_left: u64,
    /// 延迟总秒数（弹窗画进度条用）
    total_seconds: u64,
    /// 发起时间（UTC）
    created: String,
    can_cancel: bool,
    /// 是否播放提示音
    play_sound: bool,
    /// 自定义提示音文件路径（空 = 系统默认）
    sound_file_path: String,
}

enum NotifyState {
    Pending(PendingRequest),
    Info {
        message: String,
        source: String,
        expires_at: Instant,
    },
}

/// 调度线程到点该做什么（纯判定，便于单测；副作用在调用方）。
#[derive(Debug, PartialEq, Eq)]
enum NotifyDue {
    Nothing,
    /// 倒计时结束 → 执行
    Execute(Box<PendingRequest>),
    /// 事后提示到期 → 关窗
    CloseInfo,
}

fn notify_due(state: Option<&NotifyState>, now: Instant) -> NotifyDue {
    match state {
        Some(NotifyState::Pending(pending)) if now >= pending.deadline => {
            NotifyDue::Execute(Box::new(pending.clone()))
        }
        Some(NotifyState::Info { expires_at, .. }) if now >= *expires_at => NotifyDue::CloseInfo,
        _ => NotifyDue::Nothing,
    }
}

/// HTTP 服务的运行态。
#[derive(Default)]
struct HttpRuntime {
    server: Option<http::RunningServer>,
    last_error: Option<String>,
    /// 启动失败时诊断出的端口占用者（只读探测，绝不自动杀）
    occupant: Option<PortOccupant>,
    requests: Vec<RequestLogEntry>,
    notify: Option<NotifyState>,
}

/// 进快照的 HTTP 视图。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HttpView {
    enabled: bool,
    bind: String,
    port: u16,
    listening: bool,
    addr: Option<String>,
    last_error: Option<String>,
    /// 端口被谁占了（启动失败才有）
    port_occupant: Option<PortOccupantView>,
    token_set: bool,
    /// 打码后的 token（只给前 4/后 4 位，明文要用 reveal 命令专门取）
    token_hint: String,
    requests: Vec<RequestLogEntry>,
}

/// 正在进行的操作。进快照后托盘菜单项与主窗口按钮**同时**置灰（阶段 1 修 D3：
/// 以前托盘和主窗口各管各的，一边在重启、另一边照样能点，第二次点击被静默丢弃）。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct BusyState {
    /// 正在提交的电源动作（阶段 2 起是 restart/shutdown/sleep，阶段 1 只有 restart）
    power: Option<String>,
    /// 正在切换下次启动系统
    switching: bool,
    /// 正在扫描变量目录
    scanning: bool,
    /// 正在卸载临时挂载
    unmounting: bool,
}

impl BusyState {
    /// 「有任何动作在跑吗」。主窗口前端用同一份 busy 快照自己判断，
    /// Rust 侧目前只有单测直接调用它（菜单用的是 affects_menu）。
    #[cfg_attr(not(test), allow(dead_code))]
    fn any(&self) -> bool {
        self.power.is_some() || self.switching || self.scanning || self.unmounting
    }

    /// 影响**托盘菜单内容**（置灰）的部分。
    ///
    /// 刻意不含 scanning：扫描通常只几毫秒，但它前后各改一次菜单内容，
    /// 会把「每次扫描重建一次菜单」变成两次（菜单闪断的诱因）。
    /// 主窗口那侧仍然对 scanning 置灰（同一个 busy 快照，前端自己判断）。
    fn affects_menu(&self) -> bool {
        self.power.is_some() || self.switching || self.unmounting
    }

    /// 菜单指纹里用的 busy 摘要（只含 affects_menu 的部分）。
    fn menu_describe(&self) -> String {
        format!(
            "power={}|sw={}|un={}",
            self.power.as_deref().unwrap_or("-"),
            self.switching as u8,
            self.unmounting as u8
        )
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(power) = &self.power {
            parts.push(power.clone());
        }
        if self.switching {
            parts.push("切换系统".to_string());
        }
        if self.scanning {
            parts.push("扫描".to_string());
        }
        if self.unmounting {
            parts.push("卸载临时挂载".to_string());
        }
        if parts.is_empty() {
            "空闲".to_string()
        } else {
            parts.join("+")
        }
    }
}

/// 最近一次动作的结果（替代原来的 restart-result 事件；随快照一起下发）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionResult {
    ok: bool,
    action: String,
    message: String,
}

/// 主窗口与托盘菜单**共用**的唯一状态快照（阶段 1 修 D1/D4）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UiState {
    revision: u64,
    status: AppStatus,
    settings: SettingsView,
    busy: BusyState,
    last_action: Option<ActionResult>,
    http: HttpView,
    notify: Option<NotifyView>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum CloseAction {
    Tray,
    Quit,
}

impl CloseAction {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "tray" => Some(Self::Tray),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    autostart: bool,
    start_in_tray: bool,
    hide_from_dock_taskbar: bool,
    close_action: CloseAction,
    /// 取证日志开关（阶段 0）。默认关；环境变量 ANYBOOT_TRACE 可临时覆盖。
    debug_trace: bool,
    /// 内置 HTTP 服务（默认开启）
    http_enabled: bool,
    /// 监听地址（默认 0.0.0.0，配 token 才能用）
    http_bind: String,
    http_port: u16,
    /// 首次运行自动生成的 32 位十六进制 token；为空则服务拒绝启动
    http_token: String,
    /// 远程 / CLI 请求是否允许触发本机授权弹窗（Q3 已定 false）
    remote_allow_gui_auth: bool,
    /// 弹窗通知总开关（Q11）
    notify_enabled: bool,
    /// 每个渠道各自的弹窗开关与延迟秒数（Q14）
    channels: Channels,
    /// 提示音开关（默认开）
    play_sound: bool,
    /// 自定义提示音文件路径（留空用内置的 KakaoTalk 提示音）
    sound_file_path: String,
    /// 界面主题：dark / light / system
    theme: String,
    /// 窗口不透明度（百分比，50–100）
    opacity_percent: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autostart: false,
            start_in_tray: false,
            hide_from_dock_taskbar: true,
            close_action: CloseAction::Tray,
            debug_trace: false,
            http_enabled: true,
            http_bind: DEFAULT_HTTP_BIND.to_string(),
            http_port: DEFAULT_HTTP_PORT,
            http_token: String::new(),
            remote_allow_gui_auth: false,
            notify_enabled: true,
            channels: Channels::default(),
            play_sound: true,
            sound_file_path: String::new(),
            theme: "dark".to_string(),
            opacity_percent: MAX_OPACITY,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutostartState {
    Enabled,
    /// 只有 macOS 会构造（系统登录项已注册、等用户在系统设置里批准）；
    /// 其他平台保留这个取值，界面文案与状态语义才完整。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    RequiresApproval,
    Disabled,
    Unavailable,
}

impl AutostartState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::RequiresApproval => "requires-approval",
            Self::Disabled => "disabled",
            Self::Unavailable => "unavailable",
        }
    }

    fn is_on(self) -> bool {
        matches!(self, Self::Enabled | Self::RequiresApproval)
    }

    fn hint(self) -> Option<&'static str> {
        match self {
            Self::RequiresApproval => {
                Some("已注册登录项，请在「系统设置 → 通用 → 登录项与扩展」中允许本应用")
            }
            Self::Unavailable => Some(
                "无法通过系统登录项接口管理开机启动（需以 .app 形式运行），可在「系统设置 → 通用 → 登录项与扩展」中手动添加",
            ),
            _ => None,
        }
    }
}

#[cfg(target_os = "macos")]
fn app_bundle_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|executable| executable.canonicalize().ok())
        .and_then(|executable| executable.ancestors().nth(3).map(Path::to_path_buf))
        .filter(|bundle| {
            bundle
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
                && bundle.join("Contents").is_dir()
        })
}

#[cfg(target_os = "macos")]
fn running_from_app_bundle() -> bool {
    app_bundle_path().is_some()
}

/// macOS 开机启动的实现方式。
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutostartBackend {
    /// macOS 13+ 且应用已签名：走系统登录项接口（SMAppService），
    /// 条目由用户在「系统设置 → 通用 → 登录项与扩展」中管理。
    LoginItem,
    /// 未签名 / 未以 .app 运行 / macOS 12 及以下：退回 ~/Library/LaunchAgents。
    ///
    /// 系统登录项接口要求应用带有效签名，未签名的 bundle 调用
    /// `registerAndReturnError()` 只会拿到
    /// `SMAppServiceErrorDomain code=1 "Operation not permitted"`，
    /// 因此这种情况必须用兼容方式，否则开关点了没有任何效果。
    LaunchAgent,
}

#[cfg(target_os = "macos")]
fn autostart_backend_for(supported: bool, in_bundle: bool, signed: bool) -> AutostartBackend {
    if supported && in_bundle && signed {
        AutostartBackend::LoginItem
    } else {
        AutostartBackend::LaunchAgent
    }
}

#[cfg(target_os = "macos")]
fn autostart_backend() -> AutostartBackend {
    autostart_backend_for(
        login_item::is_supported(),
        running_from_app_bundle(),
        app_bundle_is_signed(),
    )
}

/// 当前 .app 是否带有有效代码签名（ad-hoc 签名同样算有效）。
///
/// SMAppService 只接受已签名的应用，所以这里实际跑一次 `codesign --verify`
/// 探测，而不是要求必须是 Developer ID 证书。
#[cfg(target_os = "macos")]
fn app_bundle_is_signed() -> bool {
    static SIGNED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SIGNED.get_or_init(|| {
        let Some(bundle) = app_bundle_path() else {
            return false;
        };
        Command::new("/usr/bin/codesign")
            .arg("--verify")
            .arg(&bundle)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// 系统登录项注册失败时的提示。已签名的应用也可能因为系统登录项数据库
/// 状态异常而注册失败，这时会退回 LaunchAgent，需要把原因告诉用户。
#[cfg(target_os = "macos")]
fn login_item_failure() -> Option<String> {
    static NOTICE: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
    NOTICE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("autostart notice mutex poisoned")
        .clone()
}

#[cfg(target_os = "macos")]
fn note_login_item_failure(error: &str) {
    static NOTICE: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
    *NOTICE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("autostart notice mutex poisoned") = Some(error.to_string());
}

#[cfg(target_os = "macos")]
fn clear_login_item_failure() {
    static NOTICE: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
    *NOTICE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("autostart notice mutex poisoned") = None;
}

/// 兼容方式（LaunchAgent）生效时说明为什么没用系统登录项。
#[cfg(target_os = "macos")]
fn launch_agent_hint() -> String {
    if !login_item::is_supported() {
        return "当前系统版本低于 macOS 13，已改用兼容方式（~/Library/LaunchAgents）实现开机启动"
            .to_string();
    }
    if !running_from_app_bundle() {
        return "当前未以 .app 形式运行，已改用兼容方式（~/Library/LaunchAgents）实现开机启动"
            .to_string();
    }
    "当前应用没有代码签名，系统登录项接口不可用，已改用兼容方式（~/Library/LaunchAgents）实现开机启动；使用签名版本后可在「系统设置 → 通用 → 登录项与扩展」中管理"
        .to_string()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    #[serde(flatten)]
    settings: Settings,
    autostart_state: &'static str,
    autostart_hint: Option<String>,
}

/// 界面提示：先说明系统状态，再说明实际用的是哪种开机启动实现。
#[cfg(not(target_os = "macos"))]
fn autostart_hint(state: AutostartState) -> Option<String> {
    state.hint().map(str::to_string)
}

#[cfg(target_os = "macos")]
fn autostart_hint(state: AutostartState) -> Option<String> {
    macos_autostart_hint(
        state,
        autostart_backend(),
        login_item_failure(),
        launch_agent_hint(),
    )
}

/// 提示语的纯逻辑部分，和运行环境解耦，便于单测。
#[cfg(target_os = "macos")]
fn macos_autostart_hint(
    state: AutostartState,
    backend: AutostartBackend,
    login_item_error: Option<String>,
    compatibility_hint: String,
) -> Option<String> {
    // 需要用户在系统设置里勾选时，优先给这条可操作的提示。
    if state == AutostartState::RequiresApproval {
        return state.hint().map(str::to_string);
    }
    if let Some(error) = login_item_error {
        return Some(format!(
            "系统登录项注册失败（{error}），已改用兼容方式（~/Library/LaunchAgents）实现开机启动"
        ));
    }
    // 已经开启但不是通过系统登录项实现的，说明现在用的是兼容方式。
    if state.is_on() && backend == AutostartBackend::LaunchAgent {
        return Some(compatibility_hint);
    }
    state.hint().map(str::to_string)
}

impl SettingsView {
    fn new(settings: Settings, state: AutostartState) -> Self {
        Self {
            settings,
            autostart_state: state.as_str(),
            autostart_hint: autostart_hint(state),
        }
    }
}

#[derive(Deserialize)]
struct LsblkOutput {
    blockdevices: Vec<LsblkDevice>,
}

#[derive(Deserialize)]
struct LsblkDevice {
    path: Option<String>,
    fstype: Option<String>,
    label: Option<String>,
    partlabel: Option<String>,
    parttype: Option<String>,
    mountpoints: Option<Vec<Option<String>>>,
}

struct ScanResult {
    vars_dir: PathBuf,
    active_mount: Option<ActiveMount>,
    variables: Vec<VariableInfo>,
}

#[derive(Clone)]
struct MountedScanCandidate {
    vars_dir: PathBuf,
    active_mount: Option<ActiveMount>,
}

fn file_hash(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn hashes_match(left: &Path, right: &Path) -> bool {
    let (Ok(left), Ok(right)) = (file_hash(left), file_hash(right)) else {
        return false;
    };
    left == right
}

fn display_path(path: &Path) -> PathBuf {
    let path_text = path.as_os_str().to_string_lossy();
    if let Some(unc_path) = path_text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc_path}"))
    } else if let Some(local_path) = path_text.strip_prefix(r"\\?\") {
        PathBuf::from(local_path)
    } else {
        path.to_path_buf()
    }
}

fn inspect_variables(directory: &Path) -> io::Result<Vec<VariableInfo>> {
    let target = directory.join("PreviousBoot");
    let mut variables = Vec::new();
    for (system, display_name, source_name) in SOURCE_FILES {
        let source = directory.join(source_name);
        let exists = source.is_file();
        variables.push(VariableInfo {
            system: system.to_string(),
            display_name: display_name.to_string(),
            source_name: source_name.to_string(),
            source: source.clone(),
            exists,
            active: exists && hashes_match(&source, &target),
        });
    }
    Ok(variables)
}

fn push_unique(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if let Ok(resolved) = candidate.canonicalize() {
        if resolved.is_dir() && !candidates.contains(&resolved) {
            candidates.push(resolved);
        }
    }
}

fn mounted_candidate_directories() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let root = format!("{}:\\", letter as char);
            push_unique(&mut candidates, PathBuf::from(root).join("EFI/refind/vars"));
        }
    } else {
        for path in [
            "/Volumes/REFIND",
            "/boot/efi",
            "/efi",
            "/boot",
            "/mnt/REFIND",
            "/media/REFIND",
        ] {
            push_unique(&mut candidates, PathBuf::from(path).join("EFI/refind/vars"));
        }
        for base in ["/Volumes", "/mnt", "/media", "/run/media"] {
            let base = PathBuf::from(base);
            let Ok(entries) = fs::read_dir(&base) else {
                continue;
            };
            for entry in entries.flatten() {
                let root = entry.path();
                push_unique(&mut candidates, root.join("EFI/refind/vars"));
                let Ok(nested) = fs::read_dir(&root) else {
                    continue;
                };
                for nested_entry in nested.flatten() {
                    push_unique(&mut candidates, nested_entry.path().join("EFI/refind/vars"));
                }
            }
        }
    }
    candidates
}

#[cfg(target_os = "linux")]
fn decode_mountinfo_path(path: &str) -> PathBuf {
    let mut decoded = String::with_capacity(path.len());
    let mut characters = path.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            let mut escape = String::new();
            for _ in 0..3 {
                if let Some(digit) = characters.next() {
                    escape.push(digit);
                }
            }
            if let Ok(byte) = u8::from_str_radix(&escape, 8) {
                decoded.push(byte as char);
                continue;
            }
            decoded.push(character);
            decoded.push_str(&escape);
        } else {
            decoded.push(character);
        }
    }
    PathBuf::from(decoded)
}

#[cfg(target_os = "linux")]
fn is_owned_mountpoint(mountpoint: &Path) -> bool {
    let temp_dir = std::env::temp_dir();
    mountpoint.starts_with(&temp_dir)
        && mountpoint
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("anyboot-"))
}

#[cfg(target_os = "linux")]
fn mounted_scan_candidates() -> Vec<MountedScanCandidate> {
    let Ok(mountinfo) = fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    for line in mountinfo.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let Some(mountpoint) = fields.get(4) else {
            continue;
        };
        let Some(options) = fields.get(5) else {
            continue;
        };
        let Some(separator) = fields.iter().position(|field| *field == "-") else {
            continue;
        };
        let Some(fstype) = fields.get(separator + 1) else {
            continue;
        };
        let Some(device) = fields.get(separator + 2) else {
            continue;
        };
        if !FAT_FILESYSTEMS.contains(fstype) {
            continue;
        }

        let mountpoint = decode_mountinfo_path(mountpoint);
        let vars_dir = mountpoint.join("EFI/refind/vars");
        let Ok(resolved) = vars_dir.canonicalize() else {
            continue;
        };
        if !resolved.is_dir()
            || candidates
                .iter()
                .any(|candidate: &MountedScanCandidate| candidate.vars_dir == resolved)
        {
            continue;
        }

        let read_only = options.split(',').any(|option| option == "ro");
        let owned = is_owned_mountpoint(&mountpoint);
        let active_mount = if owned || read_only {
            Some(ActiveMount {
                device: decode_mountinfo_path(device),
                mountpoint,
                owned,
            })
        } else {
            None
        };
        candidates.push(MountedScanCandidate {
            vars_dir: resolved,
            active_mount,
        });
    }
    candidates
}

#[cfg(not(target_os = "linux"))]
fn mounted_scan_candidates() -> Vec<MountedScanCandidate> {
    mounted_candidate_directories()
        .into_iter()
        .map(|vars_dir| MountedScanCandidate {
            vars_dir,
            active_mount: None,
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn active_mount_for_directory(directory: &Path, resolved_directory: &Path) -> Option<ActiveMount> {
    mounted_scan_candidates()
        .into_iter()
        .filter_map(|candidate| candidate.active_mount)
        .find(|mount| {
            resolved_directory.starts_with(&mount.mountpoint)
                || directory.starts_with(&mount.mountpoint)
        })
}

#[cfg(not(target_os = "linux"))]
fn active_mount_for_directory(
    _directory: &Path,
    _resolved_directory: &Path,
) -> Option<ActiveMount> {
    None
}

fn scan_unmounted_partition() -> Result<ScanResult, String> {
    if !cfg!(target_os = "linux") {
        return Err("未找到已挂载的 EFI/refind/vars 目录".to_string());
    }

    let output = Command::new("lsblk")
        .args([
            "--json",
            "--output",
            "PATH,TYPE,FSTYPE,LABEL,PARTLABEL,PARTTYPE,MOUNTPOINTS",
        ])
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法执行 lsblk: {error}"))?;
    let devices: LsblkOutput = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("无法解析 lsblk 输出: {error}"))?;

    let mut candidates = Vec::new();
    for device in devices.blockdevices {
        let Some(device_path) = device.path else {
            continue;
        };
        let Some(filesystem) = device.fstype else {
            continue;
        };
        if !FAT_FILESYSTEMS.contains(&filesystem.as_str()) {
            continue;
        }
        let mounted = device
            .mountpoints
            .unwrap_or_default()
            .iter()
            .flatten()
            .any(|mountpoint| !mountpoint.is_empty());
        if mounted {
            continue;
        }
        let label = device.label.unwrap_or_default().to_lowercase();
        let partlabel = device.partlabel.unwrap_or_default().to_lowercase();
        let parttype = device.parttype.unwrap_or_default().to_lowercase();
        let hints = format!("{label} {partlabel}");
        let priority = if hints.contains("refind") {
            0
        } else if parttype == ESP_PARTTYPE || hints.contains("efi system partition") {
            1
        } else {
            2
        };
        candidates.push((priority, device_path, filesystem));
    }
    candidates.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));

    let mountpoint = std::env::temp_dir().join(format!("anyboot-{}", std::process::id()));
    let _ = fs::remove_dir(&mountpoint);
    fs::create_dir(&mountpoint).map_err(|error| format!("无法创建临时挂载点: {error}"))?;

    for (_, device, filesystem) in candidates {
        let result = mount_command("mount")
            .args(["-o", &mount_options("ro"), "-t", &filesystem])
            .arg(&device)
            .arg(&mountpoint)
            .output();
        match result {
            Ok(output) if output.status.success() => {
                let vars_dir = mountpoint.join("EFI/refind/vars");
                if vars_dir.is_dir() {
                    let resolved = vars_dir
                        .canonicalize()
                        .map_err(|error| format!("无法解析变量目录: {error}"))?;
                    let variables = inspect_variables(&resolved)
                        .map_err(|error| format!("无法读取变量目录: {error}"))?;
                    return Ok(ScanResult {
                        vars_dir: resolved,
                        active_mount: Some(ActiveMount {
                            device: PathBuf::from(device),
                            mountpoint,
                            owned: true,
                        }),
                        variables,
                    });
                }
                let _ = mount_command("umount").arg(&mountpoint).status();
            }
            Ok(output) => {
                let _ = format!(
                    "mount {device}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Err(_) => {}
        }
    }
    let _ = fs::remove_dir(&mountpoint);

    let root_hint = if cfg!(target_os = "linux") {
        "；应用已尝试通过 pkexec 临时提权挂载，若取消授权或 pkexec 不可用，请手动挂载后再扫描"
    } else {
        ""
    };
    Err(format!(
        "已扫描未挂载的 FAT/ESP 分区，但未找到 EFI/refind/vars 目录{root_hint}"
    ))
}

fn scan_vars_directory(explicit_dir: Option<&Path>) -> Result<ScanResult, String> {
    if let Some(directory) = explicit_dir {
        if !directory.is_dir() {
            return Err(format!("rEFInd 变量目录不存在: {}", directory.display()));
        }
        let resolved_directory = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());
        let variables = inspect_variables(&resolved_directory)
            .map_err(|error| format!("无法读取变量目录: {error}"))?;
        return Ok(ScanResult {
            vars_dir: resolved_directory.clone(),
            active_mount: active_mount_for_directory(directory, &resolved_directory),
            variables,
        });
    }

    let mut candidates = mounted_scan_candidates();
    candidates.sort_by_key(|candidate| {
        candidate
            .active_mount
            .as_ref()
            .map(|mount| !mount.owned)
            .unwrap_or(true)
    });
    if candidates.len() > 1 {
        let names = candidates
            .iter()
            .map(|candidate| format!("  {}", candidate.vars_dir.display()))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "找到多个 rEFInd 变量目录，请在主窗口指定其中一个：\n{names}"
        ));
    }
    if let Some(directory) = candidates.first() {
        let variables = inspect_variables(&directory.vars_dir)
            .map_err(|error| format!("无法读取变量目录: {error}"))?;
        return Ok(ScanResult {
            vars_dir: directory.vars_dir.clone(),
            active_mount: directory.active_mount.clone(),
            variables,
        });
    }
    scan_unmounted_partition()
}

fn state_snapshot(app: &AppHandle) -> AppStatus {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().expect("state mutex poisoned");
    AppStatus {
        vars_dir: state.vars_dir.as_deref().map(display_path),
        temporary_mount: state.active_mount.as_ref().is_some_and(|mount| mount.owned),
        variables: state.variables.clone(),
        message: state.message.clone(),
        error: state.error.clone(),
    }
}

/// 兼容旧调用点：状态本身已经写进 AppState，这里只把新快照发出去。
fn publish_status(app: &AppHandle, status: &AppStatus) {
    trace(
        app,
        "event",
        &format!(
            "状态已更新 msg={} err={} tmp={}",
            status.message,
            status.error.as_deref().unwrap_or("-"),
            status.temporary_mount
        ),
    );
    publish_ui(app);
}

fn refresh_and_update(app: &AppHandle) -> AppStatus {
    let started = Instant::now();
    trace(app, "scan", "开始扫描");
    set_busy(app, |busy| busy.scanning = true);
    let (explicit_dir, old_mount) = {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        (state.explicit_dir.clone(), state.active_mount.take())
    };
    if let Some(mount) = old_mount {
        let _ = mount.unmount();
    }

    let status = match scan_vars_directory(explicit_dir.as_deref()) {
        Ok(result) => {
            let message = result
                .variables
                .iter()
                .find(|item| item.active)
                .map(|item| format!("下次启动：{}", item.display_name))
                .unwrap_or_else(|| "已连接 rEFInd；PreviousBoot 未匹配模板".to_string());
            let status = AppStatus {
                vars_dir: Some(display_path(&result.vars_dir)),
                temporary_mount: result
                    .active_mount
                    .as_ref()
                    .is_some_and(|mount| mount.owned),
                variables: result.variables.clone(),
                message: message.clone(),
                error: None,
            };
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            state.vars_dir = Some(result.vars_dir);
            state.active_mount = result.active_mount;
            state.variables = result.variables;
            state.message = message;
            state.error = None;
            status
        }
        Err(error) => {
            let message = "未找到可用的 rEFInd 变量目录".to_string();
            let status = AppStatus {
                vars_dir: None,
                temporary_mount: false,
                variables: Vec::new(),
                message: message.clone(),
                error: Some(error),
            };
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            state.vars_dir = None;
            state.variables = Vec::new();
            state.message = message;
            state.error = status.error.clone();
            status
        }
    };

    trace(
        app,
        "scan",
        &format!(
            "完成 耗时={}ms result={}",
            started.elapsed().as_millis(),
            status.error.as_deref().unwrap_or("ok")
        ),
    );
    // 扫描结束：清 busy 并发布快照（托盘菜单与主窗口一起更新、菜单指纹相同就跳过重建）
    set_busy(app, |busy| busy.scanning = false);
    status
}

/// 切换下次启动系统。busy.switching 由外层统一置位/清除，
/// 这样「未知系统」「目录未就绪」这些提前返回也不会把状态卡在 busy 上。
fn switch_and_update(app: &AppHandle, system: &str) -> AppStatus {
    let started = Instant::now();
    trace(app, "switch", &format!("请求 system={system}"));
    set_busy(app, |busy| busy.switching = true);
    let status = switch_and_update_inner(app, system, started);
    set_busy(app, |busy| busy.switching = false);
    status
}

fn switch_and_update_inner(app: &AppHandle, system: &str, started: Instant) -> AppStatus {
    let selected = SOURCE_FILES
        .iter()
        .find(|(candidate, _, _)| *candidate == system)
        .cloned();
    let Some((_system, display_name, source_name)) = selected else {
        let mut status = state_snapshot(app);
        status.error = Some(format!("未知系统: {system}"));
        status.temporary_mount = {
            let state = app.state::<Mutex<AppState>>();
            let state = state.lock().expect("state mutex poisoned");
            state.active_mount.as_ref().is_some_and(|mount| mount.owned)
        };
        return status;
    };

    let (directory, active_mount) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (state.vars_dir.clone(), state.active_mount.clone())
    };
    let Some(directory) = directory else {
        refresh_and_update(app);
        let mut status = state_snapshot(app);
        status.error = Some("变量目录尚未就绪".to_string());
        return status;
    };

    let result = write_with_mount(active_mount.as_ref(), || {
        let source = directory.join(source_name);
        let target = directory.join("PreviousBoot");
        if !source.is_file() {
            return Err(format!("变量文件不存在: {}", source.display()));
        }
        atomic_copy(&source, &target)
            .and_then(|_| {
                if hashes_match(&source, &target) {
                    Ok(())
                } else {
                    Err(io::Error::other(format!(
                        "写入校验失败: {}",
                        target.display()
                    )))
                }
            })
            .map_err(|error| format!("写入失败: {error}"))
    });

    trace(
        app,
        "switch",
        &format!(
            "完成 system={system} 耗时={}ms result={}",
            started.elapsed().as_millis(),
            result.as_ref().err().map(String::as_str).unwrap_or("ok")
        ),
    );
    match result {
        Ok(()) => {
            let status = refresh_and_update(app);
            let message = format!("下次启动：{display_name}");
            record_action(app, "切换系统", true, &message);
            status
        }
        Err(error) => {
            record_action(app, "切换系统", false, &error);
            update_failure_status(app, &format!("切换 {display_name} 失败"), error)
        }
    }
}

fn write_with_mount<F>(active_mount: Option<&ActiveMount>, operation: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    if let Some(mount) = active_mount.as_ref() {
        mount
            .remount("rw")
            .map_err(|error| format!("无法将分区重新挂载为可写: {error}"))?;
    }
    let result = operation();
    if let Some(mount) = active_mount.as_ref() {
        let _ = mount.remount("ro");
    }
    result
}

fn update_failure_status(app: &AppHandle, message: &str, error: String) -> AppStatus {
    let (existing_directory, existing_variables) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (state.vars_dir.clone(), state.variables.clone())
    };
    let status = AppStatus {
        vars_dir: existing_directory,
        temporary_mount: {
            let state = app.state::<Mutex<AppState>>();
            let state = state.lock().expect("state mutex poisoned");
            state.active_mount.as_ref().is_some_and(|mount| mount.owned)
        },
        variables: existing_variables,
        message: message.to_string(),
        error: Some(error),
    };
    {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state.message = message.to_string();
        state.error = status.error.clone();
    }
    publish_status(app, &status);
    status
}

fn temporary_target(target: &Path) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    target
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".{}.{}.{}.tmp",
            target
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("target"),
            std::process::id(),
            unique
        ))
}

fn atomic_copy(source: &Path, target: &Path) -> io::Result<()> {
    let temporary = temporary_target(target);
    let result = (|| -> io::Result<()> {
        fs::copy(source, &temporary)?;
        let file = fs::OpenOptions::new().write(true).open(&temporary)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_tray_image() -> Image<'static> {
    tauri::include_image!("icons/icon.png")
}

/// 环境变量布尔开关的纯逻辑部分（便于单测）：1/true/yes/on 与 0/false/no/off。
///
/// trace 与电源 dry-run 两个探针共用。
fn env_flag_from(value: Option<&str>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// 是否记 trace。**注意**：会锁 Settings 互斥量，所以禁止在已持有该锁的
/// 代码路径里调用 trace()（会死锁）。
///
/// 启动早期（设置文件还没读进内存时）只有环境变量生效，
/// 想抓启动阶段的日志请用 `ANYBOOT_TRACE=1`。
fn trace_enabled(app: &AppHandle) -> bool {
    let from_env = env_flag_from(std::env::var("ANYBOOT_TRACE").ok().as_deref());
    from_env.unwrap_or_else(|| stored_settings(app).debug_trace)
}

fn trace_file_path(app: &AppHandle) -> Option<PathBuf> {
    config_dir(app)
        .ok()
        .map(|directory| directory.join(TRACE_FILE))
}

/// 追加一行 trace，同时打到 stderr。
///
/// 这是取证探针：**任何失败都吞掉**（打不开文件、磁盘满、没权限都不影响应用行为），
/// 也不能在日志里写敏感数据。
fn trace(app: &AppHandle, category: &str, message: &str) {
    if !trace_enabled(app) {
        return;
    }
    let line = format!(
        "{} [{category}] {message}\n",
        utc_timestamp(now_epoch_seconds())
    );
    eprint!("trace {line}");
    let Some(path) = trace_file_path(app) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()));
}

fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// 无依赖的 UTC 时间戳（本地时区需要平台 API，这里统一记 UTC，便于三端互相比对）。
fn utc_timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Howard Hinnant 的 civil_from_days：1970-01-01 起的天数 → (年, 月, 日)。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// 托盘菜单内容摘要：同样内容 → 同样签名。
///
/// 阶段 0 用它把「重建了菜单、内容是什么」记进 trace；阶段 1 直接用它做
/// 「内容没变就不重建菜单」的指纹。
fn menu_signature(
    message: &str,
    temporary_mount: bool,
    settings: &Settings,
    variables: &[VariableInfo],
    error: Option<&str>,
    busy: &BusyState,
) -> String {
    let variables = variables
        .iter()
        .map(|variable| {
            format!(
                "{}{}",
                if variable.active { "+" } else { "-" },
                if variable.exists { "o" } else { "x" }
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "msg={message}|tmp={}|autostart={}|tray={}|dock={}|close={:?}|vars={variables}|err={}|busy={}",
        temporary_mount as u8,
        settings.autostart as u8,
        settings.start_in_tray as u8,
        settings.hide_from_dock_taskbar as u8,
        settings.close_action,
        error.unwrap_or(""),
        // 动作进行中菜单项会变灰，所以 busy 必须进指纹，否则置灰不会生效
        busy.menu_describe()
    )
}

/// 堵住「主窗口和托盘各读各的」的第一层：busy 的唯一读写口。
fn busy_snapshot(app: &AppHandle) -> BusyState {
    app.state::<Mutex<BusyState>>()
        .lock()
        .expect("busy mutex poisoned")
        .clone()
}

/// 改 busy 并立刻把新快照发出去（托盘与主窗口一起变）。
fn set_busy(app: &AppHandle, update: impl FnOnce(&mut BusyState)) {
    {
        let state = app.state::<Mutex<BusyState>>();
        let mut busy = state.lock().expect("busy mutex poisoned");
        update(&mut busy);
    }
    publish_ui(app);
}

fn last_action_snapshot(app: &AppHandle) -> Option<ActionResult> {
    app.state::<Mutex<Option<ActionResult>>>()
        .lock()
        .expect("action mutex poisoned")
        .clone()
}

/// 记下最近一次动作的结果（进快照，替代原来的 restart-result 事件）。
fn record_action(app: &AppHandle, action: &str, ok: bool, message: &str) {
    let result = ActionResult {
        ok,
        action: action.to_string(),
        message: message.to_string(),
    };
    trace(app, "action", &format!("{action} ok={ok} msg={message}"));
    *app.state::<Mutex<Option<ActionResult>>>()
        .lock()
        .expect("action mutex poisoned") = Some(result);
}

/// 抢占一个电源动作；已经有动作在跑时返回 false（替代原来的静态原子标志）。
fn try_begin_power(app: &AppHandle, action: &str) -> bool {
    let acquired = {
        let state = app.state::<Mutex<BusyState>>();
        let mut busy = state.lock().expect("busy mutex poisoned");
        if busy.power.is_some() {
            false
        } else {
            busy.power = Some(action.to_string());
            true
        }
    };
    if acquired {
        publish_ui(app);
    } else {
        trace(
            app,
            "power",
            &format!("已有电源动作在处理中，忽略 {action}"),
        );
    }
    acquired
}

/// 电源动作结束（失败/被拒绝时调用，让按钮重新可用；成功时系统马上就没了）。
fn finish_power(app: &AppHandle) {
    {
        let state = app.state::<Mutex<BusyState>>();
        let mut busy = state.lock().expect("busy mutex poisoned");
        busy.power = None;
    }
    publish_ui(app);
}

/// 生成当前状态的快照（不改任何东西）。
fn ui_state(app: &AppHandle) -> UiState {
    let settings = view_settings(app);
    let http = {
        let runtime = http_lock();
        let (listening, addr) = match runtime.server.as_ref() {
            Some(server) => (true, Some(server.addr.clone())),
            None => (false, None),
        };
        HttpView {
            enabled: settings.settings.http_enabled,
            bind: settings.settings.http_bind.clone(),
            port: settings.settings.http_port,
            listening,
            addr,
            last_error: runtime.last_error.clone(),
            port_occupant: runtime.occupant.as_ref().map(PortOccupant::to_view),
            token_set: !settings.settings.http_token.trim().is_empty(),
            token_hint: token_hint(&settings.settings.http_token),
            requests: runtime.requests.clone(),
        }
    };
    UiState {
        revision: UI_REVISION.load(Ordering::SeqCst),
        status: state_snapshot(app),
        // 唯一真相：设置的 autostart 取系统真实状态（阶段 1 修 D1，
        // 以前托盘菜单读的是设置文件里的旧值，主窗口读的是系统真实值，两边会不一致）
        settings,
        busy: busy_snapshot(app),
        last_action: last_action_snapshot(app),
        http,
        notify: notify_view(app),
    }
}

/// **唯一**的发布入口：先 +1 revision，再用同一份快照重建托盘菜单 + 通知主窗口。
///
/// 主窗口和托盘菜单从此不存在「各读各的」，也不需要三个补丁式事件。
fn publish_ui(app: &AppHandle) -> UiState {
    UI_REVISION.fetch_add(1, Ordering::SeqCst);
    let state = ui_state(app);
    if let Err(error) = update_tray_menu(app, &state, "publish") {
        trace(app, "tray", &format!("刷新托盘菜单失败: {error}"));
    }
    trace(
        app,
        "state",
        &format!(
            "revision={} busy={} msg={} err={}",
            state.revision,
            state.busy.describe(),
            state.status.message,
            state.status.error.as_deref().unwrap_or("-")
        ),
    );
    if let Err(error) = app.emit(UI_STATE_CHANGED_EVENT, &state) {
        trace(app, "state", &format!("通知主窗口失败: {error}"));
    }
    state
}

/// 变量目录还没扫出来时，也把三个系统项摆上（禁用），
/// 保证托盘菜单从第一帧起就是完整的（阶段 1 修 D2：以前冷启动那段时间右键
/// 只能看到一个没有系统项、没有「重启系统」、没有「应用设置」的残缺菜单）。
// ==================== 远程控制（阶段 3/4） ====================

/// 校验用户自定义的 token（纯函数，便于单测）。
///
/// - 空字符串是**合法**输入：等于「清空 token」，内置 HTTP 服务会因此不再启动（沿用现有语义）。
/// - 非空：长度 4–128，且只能是可见 ASCII —— 粘贴时混进换行/空格/全角字符会让请求一直 401，
///   在远程端极难自己查出来，所以这里直接拒掉并说清是哪个字符。
fn validate_http_token(token: &str) -> Result<String, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Ok(token);
    }
    let length = token.chars().count();
    if length < MIN_TOKEN_LEN || length > MAX_TOKEN_LEN {
        return Err(format!(
            "token 长度要在 {MIN_TOKEN_LEN}–{MAX_TOKEN_LEN} 之间（现在是 {length}）"
        ));
    }
    if let Some(bad) = token.chars().find(|c| !c.is_ascii_graphic()) {
        return Err(format!(
            "token 只能用可见 ASCII 字符（字母/数字/符号），这个字符不行：{bad:?}"
        ));
    }
    Ok(token)
}

/// 主题归一（纯函数）：不认识的值一律回落到 dark。
fn normalize_theme(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if THEMES.contains(&value.as_str()) {
        value
    } else {
        "dark".to_string()
    }
}

/// 不透明度夹取到 [MIN_OPACITY, MAX_OPACITY]（纯函数）。
fn clamp_opacity(percent: u32) -> u8 {
    percent.clamp(MIN_OPACITY as u32, MAX_OPACITY as u32) as u8
}

/// 生成 32 位十六进制 token（首次运行自动生成）。
fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // 取不到系统随机数（几乎不会发生）：退回时间 + pid，仍保证唯一性
        let seed = format!("{}:{}", now_epoch_seconds(), std::process::id());
        let digest = Sha256::digest(seed.as_bytes());
        bytes.copy_from_slice(&digest[..16]);
    }
    hex::encode(bytes)
}

/// 一次请求的短 id（8 位十六进制），用于 pending / 日志关联。
fn short_request_id() -> String {
    generate_token()[..8].to_string()
}

/// 界面上的打码 token：只露前 4 后 4，明文要用 reveal 命令专门取。
fn token_hint(token: &str) -> String {
    let token = token.trim();
    if token.is_empty() {
        return "未设置（服务不会启动）".to_string();
    }
    if token.len() <= 8 {
        return "*".repeat(token.len());
    }
    format!("{}…{}", &token[..4], &token[token.len() - 4..])
}

/// 常量时间比较 token（长度不同直接 false）。
fn token_matches(expected: &str, provided: &str) -> bool {
    let expected = expected.trim();
    let provided = provided.trim();
    if expected.is_empty() || expected.len() != provided.len() {
        return false;
    }
    expected
        .bytes()
        .zip(provided.bytes())
        .fold(0u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}

/// 本机 IPv4（UDP 的 connect 只是让内核选出口地址，不发包）。
fn local_ipv4() -> String {
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

/// HTTP 运行态：进程级单例。
///
/// 刻意不放进 Tauri 的 State——`app.state::<T>()` 返回的句柄是临时值，
/// 从它派生出的 MutexGuard 没法跨函数返回；这里用 OnceLock 拿到 'static 引用。
static HTTP_RUNTIME: std::sync::OnceLock<Mutex<HttpRuntime>> = std::sync::OnceLock::new();

fn http_lock() -> std::sync::MutexGuard<'static, HttpRuntime> {
    HTTP_RUNTIME
        .get_or_init(|| Mutex::new(HttpRuntime::default()))
        .lock()
        .expect("http mutex poisoned")
}

/// 进快照的端口占用视图（文案在后端算好，前端直接显示）
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortOccupantView {
    pid: u32,
    name: String,
    /// 占用者就是本程序（当前进程，理论上不会出现）
    is_self: bool,
    /// 占用者是本应用的另一个实例（按进程名判断；升级后旧实例还占着端口的场景）
    same_app: bool,
    /// 给人看的一句话
    message: String,
    /// 可直接复制的结束命令
    kill_hint: Option<String>,
}

/// 端口占用诊断结果
#[derive(Clone, Debug)]
struct PortOccupant {
    pid: u32,
    /// 进程名（如 explorer.exe、python.exe），查不到则为空
    name: String,
    /// 占用者就是当前进程（理论上不会出现：我们自己没绑上才报的错）
    is_self: bool,
    /// 占用者是本程序的另一个实例（按进程名匹配，例如升级后旧版还驻留着）
    same_app: bool,
}

impl PortOccupant {
    fn to_view(&self) -> PortOccupantView {
        PortOccupantView {
            pid: self.pid,
            name: self.name.clone(),
            is_self: self.is_self,
            same_app: self.same_app,
            message: self.display(),
            kill_hint: self.kill_hint(),
        }
    }

    /// 界面文案：是本程序 vs 外部进程，分别给不同提示
    fn display(&self) -> String {
        if self.same_app {
            format!(
                "端口被本应用的另一个实例占用：PID {}（{}）——可以点「结束占用进程」把它退掉（旧版本驻留时常见），或先手动退出那个实例",
                self.pid,
                if self.name.is_empty() { "同程序" } else { &self.name }
            )
        } else if self.name.is_empty() {
            format!(
                "端口被 PID {} 占用（拿不到进程名），请释放它或改用其他端口",
                self.pid
            )
        } else {
            format!(
                "端口被 PID {}（{}）占用，请释放它或改用其他端口",
                self.pid, self.name
            )
        }
    }

    /// 给用户复制即用的结束命令（Windows 优先 taskkill，非 Windows 给通用提示）
    fn kill_hint(&self) -> Option<String> {
        if self.pid == 0 {
            return None;
        }
        if cfg!(target_os = "windows") {
            Some(format!("taskkill /PID {} /F", self.pid))
        } else {
            Some(format!("kill {}", self.pid))
        }
    }
}

/// 纯函数：从 netstat -ano 的输出一行行文本里找指定端口的 LISTENING 条目，
/// 返回占用者的 PID（用于后续查进程名）。
/// 抽成纯函数是为了可单测（不真的起 netstat）。
fn parse_netstat_listening_port<'a>(
    lines: impl Iterator<Item = &'a str>,
    port: u16,
) -> Option<u32> {
    let marker = format!(":{port} ");
    for line in lines {
        let trimmed = line.trim();
        // netstat 输出形如: TCP 0.0.0.0:12300 0.0.0.0:0 LISTENING 29220
        let is_tcp_listen = trimmed.starts_with("TCP")
            && trimmed.contains("LISTENING")
            && trimmed.contains(&marker);
        if is_tcp_listen {
            // PID 是行末的最后一个数字 token
            let pid = trimmed
                .split_whitespace()
                .last()
                .and_then(|value| value.parse::<u32>().ok());
            if let Some(pid) = pid {
                return Some(pid);
            }
        }
    }
    None
}

/// 跨平台：按 PID 查进程名（Windows 用 wmic，非 Windows 用 ps）。查不到返回空串。
fn process_name_for_pid(pid: u32) -> String {
    let output = if cfg!(target_os = "windows") {
        std::process::Command::new("wmic")
            .args([
                "process",
                "where",
                &format!("ProcessId={pid}"),
                "get",
                "Name",
                "/value",
            ])
            .output()
            .ok()
    } else {
        std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()
    };
    let Some(output) = output else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    if cfg!(target_os = "windows") {
        // wmic /value 输出形如（每行一个 key=value）：
        //   Name=explorer.exe
        text.lines()
            .find_map(|line| line.strip_prefix("Name="))
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    } else {
        // ps comm= 直接输出进程名
        text.trim().to_string()
    }
}

/// 诊断端口占用者（只读）：起 netstat -ano 拿 PID，再查进程名，判断是否本程序。
fn diagnose_port_occupant(port: u16) -> Option<PortOccupant> {
    let output = std::process::Command::new("netstat")
        .args(["-ano"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let pid = parse_netstat_listening_port(text.lines(), port)?;
    let name = process_name_for_pid(pid);
    let is_self = pid == std::process::id();
    // 同一个程序的另一个实例：进程名和当前 exe 名一致（升级后旧版还驻留的场景）
    let current_name = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_default();
    let same_app = is_self
        || (!name.is_empty()
            && !current_name.is_empty()
            && name.eq_ignore_ascii_case(&current_name));
    Some(PortOccupant {
        pid,
        name,
        is_self,
        same_app,
    })
}

/// 结束占用进程（仅当用户显式点「结束占用进程」按钮时调用）。
/// 跨平台：Windows 用 taskkill /F，非 Windows 用 kill。
fn kill_process(pid: u32) -> Result<(), String> {
    let status = if cfg!(target_os = "windows") {
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .map_err(|error| format!("无法执行 taskkill：{error}"))?
    } else {
        std::process::Command::new("kill")
            .args([&pid.to_string()])
            .status()
            .map_err(|error| format!("无法执行 kill：{error}"))?
    };
    if status.success() {
        Ok(())
    } else {
        Err(format!("结束 PID {pid} 失败（exit code {status}）"))
    }
}

/// 首次运行/升级后把完整设置写回磁盘：默认值一眼可见，token 一定存在。
///
/// 环境变量覆盖**不**写进文件（它们只是调试开关）。
fn ensure_settings_complete(app: &AppHandle) {
    let mut settings = stored_settings(app);
    let mut changed = false;
    if settings.http_token.trim().is_empty() {
        settings.http_token = generate_token();
        changed = true;
        trace(app, "http", "首次运行：已生成 32 位十六进制 token");
    }
    if settings.http_bind.trim().is_empty() {
        settings.http_bind = DEFAULT_HTTP_BIND.to_string();
        changed = true;
    }
    if settings.http_port == 0 {
        settings.http_port = DEFAULT_HTTP_PORT;
        changed = true;
    }
    if changed {
        if let Err(error) = save_settings(app, &settings) {
            trace(app, "http", &format!("写回设置失败: {error}"));
        }
    }
}

/// 按当前设置（+ 环境变量覆盖）启动 / 停止 / 重启 HTTP 服务。
fn apply_http_config(app: &AppHandle) {
    let settings = stored_settings(app);
    let enabled = env_flag_from(std::env::var("ANYBOOT_HTTP_ENABLED").ok().as_deref())
        .unwrap_or(settings.http_enabled);
    let bind = std::env::var("ANYBOOT_HTTP_BIND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| settings.http_bind.trim().to_string());
    let port = std::env::var("ANYBOOT_HTTP_PORT")
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(settings.http_port);
    let token = settings.http_token.trim().to_string();

    // 先确定性收掉旧服务（端口被占用时不换端口，报错并留在界面上）
    let previous = http_lock().server.take();
    if let Some(server) = previous {
        let addr = server.addr.clone();
        server.stop();
        trace(app, "http", &format!("已停止旧服务 {addr}"));
    }

    let mut last_error = None;
    let mut occupant = None;
    if !enabled {
        trace(app, "http", "服务已关闭（设置或环境变量）");
    } else if token.is_empty() {
        last_error = Some("token 为空，服务未启动（不允许无鉴权监听）".to_string());
        trace(app, "http", "token 为空，拒绝启动");
    } else {
        let config = http::ServerConfig {
            bind: bind.clone(),
            port,
        };
        match http::start(app.clone(), config) {
            Ok(server) => {
                trace(app, "http", &format!("已监听 {}", server.addr));
                http_lock().server = Some(server);
            }
            Err(error) => {
                trace(app, "http", &format!("启动失败: {error}"));
                // 只读诊断：告诉用户端口被谁占了（不自动杀，杀要用户点按钮）
                occupant = diagnose_port_occupant(port);
                if let Some(found) = occupant.as_ref() {
                    trace(
                        app,
                        "http",
                        &format!(
                            "端口占用者 pid={} name={} is_self={} same_app={}",
                            found.pid,
                            if found.name.is_empty() {
                                "(未知)"
                            } else {
                                &found.name
                            },
                            found.is_self,
                            found.same_app
                        ),
                    );
                }
                last_error = Some(error);
            }
        }
    }
    {
        let mut runtime = http_lock();
        runtime.last_error = last_error;
        runtime.occupant = occupant;
    }
    // 端口刚被释放时往往第一次绑不上（TIME_WAIT / 上一实例正在退场），
    // 只要这次是「想开着」，就再试一次——界面上勾上就该能起来。
    let diagnosised = http_lock().occupant.is_some();
    if enabled && diagnosised {
        match http::start(
            app.clone(),
            http::ServerConfig {
                bind: bind.clone(),
                port,
            },
        ) {
            Ok(server) => {
                trace(app, "http", &format!("重试成功，已监听 {}", server.addr));
                let mut runtime = http_lock();
                runtime.server = Some(server);
                runtime.last_error = None;
                runtime.occupant = None;
            }
            Err(error) => trace(app, "http", &format!("重试仍失败: {error}")),
        }
    }
    publish_ui(app);
}

fn record_request_log(
    app: &AppHandle,
    channel: Channel,
    source: &str,
    path: &str,
    summary: &str,
    status: &'static str,
    message: &str,
) {
    let entry = RequestLogEntry {
        time: utc_timestamp(now_epoch_seconds()),
        source: source.to_string(),
        channel: channel.as_str(),
        path: path.to_string(),
        summary: summary.to_string(),
        status,
        message: message.to_string(),
    };
    trace(
        app,
        "http-access",
        &format!(
            "{} {} {} {} -> {} {}",
            entry.time, entry.source, entry.channel, path, status, message
        ),
    );
    let mut runtime = http_lock();
    push_recent(&mut runtime.requests, entry);
}

/// 「最近请求」是环形更新的：最新在最前，只留 MAX_RECENT_REQUESTS 条。
fn push_recent(requests: &mut Vec<RequestLogEntry>, entry: RequestLogEntry) {
    requests.insert(0, entry);
    requests.truncate(MAX_RECENT_REQUESTS);
}

/// HTTP 层交过来的请求（模块 http 解析、这里决策）。
pub(crate) struct RemoteRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) query: Vec<(String, String)>,
    pub(crate) body: Vec<(String, String)>,
    pub(crate) token: Option<String>,
    pub(crate) source: String,
    pub(crate) source_is_loopback: bool,
}

impl RemoteRequest {
    fn param(&self, key: &str) -> Option<String> {
        http::find_pair(&self.query, key).or_else(|| http::find_pair(&self.body, key))
    }
}

/// 回给客户端的东西。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RemoteReply {
    pub(crate) status: u16,
    pub(crate) json: serde_json::Value,
}

impl RemoteReply {
    fn ok(status: u16, json: serde_json::Value) -> Self {
        Self { status, json }
    }

    fn error(status: u16, code: &str, message: &str) -> Self {
        Self {
            status,
            json: serde_json::json!({
                "ok": false,
                "error": code,
                "message": message,
            }),
        }
    }
}

/// 渠道判定：`channel=cli` 只在请求确实来自 loopback 时生效，
/// 远端就算伪造这个参数也一律按 http 渠道的配置执行（plan §4.7）。
fn remote_channel(request: &RemoteRequest) -> Channel {
    if request.source_is_loopback {
        if let Some(channel) = request
            .param("channel")
            .and_then(|value| Channel::parse(&value))
        {
            return channel;
        }
    }
    Channel::Http
}

/// 路由：路径 + 参数 → 远程命令（失败时直接给出该回的错误）。
fn route_remote_command(request: &RemoteRequest) -> Result<RemoteCommand, RemoteReply> {
    // 注意：/health、/api/status、/api/requests 在 handle_remote_request 里已经返回，
    // 这里只处理「会改状态」的三个端点。
    match request.path.as_str() {
        "/api/switch" => {
            let Some(system) = request.param("system") else {
                return Err(RemoteReply::error(
                    400,
                    "missing_system",
                    "缺少 system 参数（linux/windows/macos）",
                ));
            };
            if !SOURCE_FILES
                .iter()
                .any(|(candidate, _, _)| *candidate == system)
            {
                return Err(RemoteReply::error(
                    400,
                    "unknown_system",
                    &format!("未知系统: {system}"),
                ));
            }
            Ok(RemoteCommand::Switch(system))
        }
        "/api/power" => {
            let action = match request.param("action").as_deref().map(str::trim) {
                None | Some("") => PowerAction::default(),
                Some(value) => PowerAction::parse(value).ok_or_else(|| {
                    RemoteReply::error(
                        400,
                        "unknown_action",
                        &format!("未知的电源动作: {value}（可用 reboot/shutdown/sleep）"),
                    )
                })?,
            };
            Ok(RemoteCommand::Power(action))
        }
        "/api/switch-and-power" => {
            let Some(system) = request.param("system") else {
                return Err(RemoteReply::error(
                    400,
                    "missing_system",
                    "缺少 system 参数（linux/windows/macos）",
                ));
            };
            if !SOURCE_FILES
                .iter()
                .any(|(candidate, _, _)| *candidate == system)
            {
                return Err(RemoteReply::error(
                    400,
                    "unknown_system",
                    &format!("未知系统: {system}"),
                ));
            }
            let power = match request.param("action").as_deref().map(str::trim) {
                None | Some("") => PowerAction::default(),
                Some(value) => PowerAction::parse(value).ok_or_else(|| {
                    RemoteReply::error(
                        400,
                        "unknown_action",
                        &format!("未知的电源动作: {value}（可用 reboot/shutdown/sleep）"),
                    )
                })?,
            };
            Ok(RemoteCommand::SwitchAndPower { system, power })
        }
        _ => Err(RemoteReply::error(404, "not_found", "未知路径")),
    }
}

fn remote_status_json(app: &AppHandle) -> serde_json::Value {
    let status = state_snapshot(app);
    serde_json::json!({
        "ok": true,
        "revision": UI_REVISION.load(Ordering::SeqCst),
        "varsDir": status
            .vars_dir
            .as_ref()
            .map(|path| display_path(path).display().to_string()),
        "temporaryMount": status.temporary_mount,
        "message": status.message,
        "error": status.error,
        "variables": status
            .variables
            .iter()
            .map(|variable| serde_json::json!({
                "system": variable.system,
                "displayName": variable.display_name,
                "exists": variable.exists,
                "active": variable.active,
            }))
            .collect::<Vec<_>>(),
    })
}

fn remote_requests_json() -> serde_json::Value {
    let requests = http_lock()
        .requests
        .iter()
        .map(|entry| {
            serde_json::json!({
                "time": entry.time,
                "source": entry.source,
                "channel": entry.channel,
                "path": entry.path,
                "summary": entry.summary,
                "status": entry.status,
                "message": entry.message,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "ok": true, "requests": requests })
}

/// HTTP / CLI 转发共用的唯一入口：鉴权 → 路由 → 立即执行或转成待执行请求。
pub(crate) fn handle_remote_request(app: &AppHandle, request: &RemoteRequest) -> RemoteReply {
    if !matches!(request.method.as_str(), "GET" | "POST") {
        return RemoteReply::error(405, "method_not_allowed", "只支持 GET / POST");
    }
    if request.path != "/health" {
        let settings = stored_settings(app);
        let provided = request.token.clone().unwrap_or_default();
        if !token_matches(&settings.http_token, &provided) {
            record_request_log(
                app,
                remote_channel(request),
                &request.source,
                &request.path,
                "（未通过鉴权）",
                "rejected",
                "token 无效",
            );
            return RemoteReply::error(401, "invalid_token", "token 缺失或无效");
        }
    }

    match request.path.as_str() {
        "/health" => {
            return RemoteReply::ok(200, serde_json::json!({ "ok": true, "service": "anyboot" }));
        }
        "/api/status" => return RemoteReply::ok(200, remote_status_json(app)),
        "/api/requests" => return RemoteReply::ok(200, remote_requests_json()),
        _ => {}
    }

    let command = match route_remote_command(request) {
        Ok(command) => command,
        Err(reply) => {
            record_request_log(
                app,
                remote_channel(request),
                &request.source,
                &request.path,
                "（参数/路径错误）",
                "rejected",
                reply
                    .json
                    .get("message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("非法请求"),
            );
            return reply;
        }
    };

    start_remote_command(
        app,
        remote_channel(request),
        &request.source,
        command,
        &request.path,
    )
}

/// 立即执行还是先放进待执行槽位（延迟 > 0）。
fn start_remote_command(
    app: &AppHandle,
    channel: Channel,
    source: &str,
    command: RemoteCommand,
    path: &str,
) -> RemoteReply {
    let settings = stored_settings(app);
    let notify = settings.channels.for_channel(channel).clone();
    let delay = notify.clamped_delay();
    let summary = command.summary();

    if delay == 0 {
        let (ok, message) = execute_remote_command(app, channel, source, &command);
        record_request_log(
            app,
            channel,
            source,
            path,
            &summary,
            if ok { "executed" } else { "failed" },
            &message,
        );
        if settings.notify_enabled && notify.notify {
            set_notify_state(
                app,
                Some(NotifyState::Info {
                    message: message.clone(),
                    source: source.to_string(),
                    expires_at: Instant::now() + std::time::Duration::from_secs(3),
                }),
            );
        }
        return RemoteReply::ok(
            if ok { 200 } else { 500 },
            serde_json::json!({
                "ok": ok,
                "pending": false,
                "channel": channel.as_str(),
                "message": message,
            }),
        );
    }

    let request_id = short_request_id();
    let pending = PendingRequest {
        id: request_id.clone(),
        channel,
        source: source.to_string(),
        summary: summary.clone(),
        command,
        seconds: u64::from(delay),
        deadline: Instant::now() + std::time::Duration::from_secs(u64::from(delay)),
        created: utc_timestamp(now_epoch_seconds()),
    };
    // 注意：持锁期间不能调 record_request_log（它会再锁一次，直接死锁）
    let already_pending = {
        let runtime = http_lock();
        runtime
            .notify
            .as_ref()
            .is_some_and(|state| matches!(state, NotifyState::Pending(_)))
    };
    if already_pending {
        record_request_log(
            app,
            channel,
            source,
            path,
            &summary,
            "rejected",
            "已有待执行请求",
        );
        return RemoteReply::error(
            409,
            "action_in_progress",
            "已有待执行的远程请求，请先确认或取消",
        );
    }
    {
        let mut runtime = http_lock();
        runtime.notify = Some(NotifyState::Pending(pending.clone()));
    }
    record_request_log(
        app,
        channel,
        source,
        path,
        &summary,
        "pending",
        "等待本机确认",
    );
    if settings.notify_enabled && notify.notify {
        if let Err(error) = show_notify_window(app) {
            trace(app, "notify", &format!("弹窗失败（不影响执行）: {error}"));
        }
        // 提示音只在「真的弹了窗」时播一次（进 pending 这一刻），不是每秒都响
        play_notify_sound(app);
    } else {
        trace(app, "notify", "该渠道关闭了通知：只在主窗口显示待执行请求");
    }
    publish_ui(app);
    RemoteReply::ok(
        202,
        serde_json::json!({
            "ok": true,
            "pending": true,
            "requestId": request_id,
            "channel": channel.as_str(),
            "executeInSeconds": delay,
            "message": format!("已收到远程请求，本机确认后 {delay} 秒执行"),
        }),
    )
}

/// 真正执行一条远程命令。
///
/// `allow_gui_auth` 用 `remoteAllowGuiAuth`（Q3 默认 false：远程不弹本机授权框）。
fn execute_remote_command(
    app: &AppHandle,
    channel: Channel,
    source: &str,
    command: &RemoteCommand,
) -> (bool, String) {
    let settings = stored_settings(app);
    let allow_gui_auth = settings.remote_allow_gui_auth;
    trace(
        app,
        "remote",
        &format!(
            "执行 {}（channel={} source={source} allow_gui_auth={allow_gui_auth}）",
            command.summary(),
            channel.as_str()
        ),
    );
    match command {
        RemoteCommand::Switch(system) => {
            let status = switch_and_update(app, system);
            match status.error {
                Some(error) => (false, error),
                None => (true, format!("下次启动：{}", system_display_name(system))),
            }
        }
        RemoteCommand::Power(action) => {
            spawn_power(app, *action, allow_gui_auth);
            (true, format!("{}（已提交）", action.label()))
        }
        RemoteCommand::SwitchAndPower { system, power } => {
            let status = switch_and_update(app, system);
            if let Some(error) = status.error {
                return (false, format!("切换失败，未执行电源动作：{error}"));
            }
            spawn_power(app, *power, allow_gui_auth);
            (
                true,
                format!(
                    "下次启动：{}；{}（已提交）",
                    system_display_name(system),
                    power.label()
                ),
            )
        }
    }
}

fn set_notify_state(app: &AppHandle, state: Option<NotifyState>) {
    let show = state.is_some();
    http_lock().notify = state;
    if show {
        if let Err(error) = show_notify_window(app) {
            trace(app, "notify", &format!("弹窗失败（不影响执行）: {error}"));
        }
    } else {
        close_notify_window(app);
    }
    publish_ui(app);
}

fn notify_view(app: &AppHandle) -> Option<NotifyView> {
    let runtime = http_lock();
    match runtime.notify.as_ref()? {
        NotifyState::Pending(pending) => {
            let settings = stored_settings(app);
            Some(NotifyView {
                kind: "pending",
                id: Some(pending.id.clone()),
                title: format!("远程请求（{}）", pending.channel.label()),
                message: pending.summary.clone(),
                source: pending.source.clone(),
                channel: pending.channel.as_str(),
                seconds_left: pending.seconds_left(),
                total_seconds: pending.seconds,
                created: pending.created.clone(),
                can_cancel: true,
                play_sound: settings.play_sound,
                sound_file_path: settings.sound_file_path.clone(),
            })
        }
        NotifyState::Info {
            message,
            source,
            expires_at,
        } => Some(NotifyView {
            kind: "info",
            id: None,
            title: "远程请求已执行".to_string(),
            message: message.clone(),
            source: source.clone(),
            channel: Channel::Http.as_str(),
            seconds_left: expires_at
                .saturating_duration_since(Instant::now())
                .as_secs(),
            total_seconds: 3,
            created: utc_timestamp(now_epoch_seconds()),
            can_cancel: false,
            play_sound: true,
            sound_file_path: String::new(),
        }),
    }
}

/// 显示置顶小窗（右下角）。失败不影响执行，只记 trace。
/// 内置提示音落到配置目录后的路径（内容不符就重写一遍，便于升级后自动更新）。
fn builtin_sound_path(app: &AppHandle) -> Option<PathBuf> {
    let dir = config_dir(app).ok()?.join(SOUND_DIR);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        trace(app, "sound", &format!("创建提示音目录失败: {error}"));
        return None;
    }
    let path = dir.join(BUILTIN_SOUND_FILE);
    let needs_write = match std::fs::read(&path) {
        Ok(existing) => existing != BUILTIN_SOUND,
        Err(_) => true,
    };
    if needs_write {
        if let Err(error) = std::fs::write(&path, BUILTIN_SOUND) {
            trace(app, "sound", &format!("写入内置提示音失败: {error}"));
            return None;
        }
    }
    Some(path)
}

/// 各平台可用的播放方式（纯函数，便于单测）。
///
/// 走系统播放器而不是 webview 的 `Audio`：Linux 上 webkit2gtk 播放还要看 gstreamer 插件，
/// 缺一个就静音；而且自定义路径在 webview 里读不到（跨域/权限）。
fn sound_player_candidates(path: &Path) -> Vec<(String, Vec<String>)> {
    let file = path.display().to_string();
    if cfg!(target_os = "windows") {
        // SoundPlayer 只认 WAV PCM —— 内置音就是这种格式；自定义非 WAV 时这里会失败并回落。
        vec![(
            "powershell".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!("(New-Object Media.SoundPlayer '{file}').PlaySync();"),
            ],
        )]
    } else if cfg!(target_os = "macos") {
        vec![("afplay".to_string(), vec![file])]
    } else {
        vec![
            ("paplay".to_string(), vec![file.clone()]),
            ("aplay".to_string(), vec!["-q".to_string(), file.clone()]),
            (
                "ffplay".to_string(),
                vec![
                    "-nodisp".to_string(),
                    "-autoexit".to_string(),
                    "-loglevel".to_string(),
                    "quiet".to_string(),
                    file,
                ],
            ),
        ]
    }
}

/// 播放一个音频文件：依次尝试本平台的候选播放器，返回第一个跑起来的。
fn play_sound_file(path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("文件不存在: {}", path.display()));
    }
    let mut last_error = String::from("本平台没有可用的播放器");
    for (program, args) in sound_player_candidates(path) {
        match Command::new(&program)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            // 只等「起得来」，不等播放完：通知窗口的出现不该被 2 秒音频拖住。
            Ok(_child) => return Ok(program),
            Err(error) => last_error = format!("{program}: {error}"),
        }
    }
    Err(last_error)
}

/// 收到远程请求时播放提示音：自定义路径优先，失败回落内置 KakaoTalk 提示音。
fn play_notify_sound(app: &AppHandle) {
    let settings = stored_settings(app);
    if !settings.play_sound {
        trace(app, "sound", "提示音已关闭（设置里 playSound=false）");
        return;
    }
    let mut candidates: Vec<(PathBuf, &'static str)> = Vec::new();
    let custom = settings.sound_file_path.trim();
    if !custom.is_empty() {
        candidates.push((PathBuf::from(custom), "自定义"));
    }
    if let Some(builtin) = builtin_sound_path(app) {
        candidates.push((builtin, "内置"));
    }
    if candidates.is_empty() {
        trace(app, "sound", "没有可播放的提示音文件");
        return;
    }
    let mut last_error = String::new();
    for (path, label) in candidates {
        match play_sound_file(&path) {
            Ok(program) => {
                trace(
                    app,
                    "sound",
                    &format!("已播放{label}提示音（{program}）：{}", path.display()),
                );
                return;
            }
            Err(error) => last_error = format!("{label}提示音失败（{error}）"),
        }
    }
    trace(app, "sound", &format!("提示音没播出来：{last_error}"));
}

fn show_notify_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW_LABEL) {
        let _ = window.show();
        return Ok(());
    }
    let window = tauri::WebviewWindowBuilder::new(
        app,
        NOTIFY_WINDOW_LABEL,
        tauri::WebviewUrl::App("notify.html".into()),
    )
    .title("远程请求")
    .inner_size(440.0, 320.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .build()
    .map_err(|error| format!("无法创建通知窗口: {error}"))?;

    // 右下角：窗口刚建出来时 current_monitor() 在 Linux 上可能还是 None，
    // 回退到主显示器；都拿不到就保持默认居中。
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        let size = monitor.size();
        let scale = monitor.scale_factor();
        let width = (440.0 * scale) as i32;
        let height = (320.0 * scale) as i32;
        let margin = (24.0 * scale) as i32;
        let _ = window.set_position(tauri::PhysicalPosition::new(
            size.width as i32 - width - margin,
            size.height as i32 - height - margin,
        ));
    }
    Ok(())
}

fn close_notify_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW_LABEL) {
        let _ = window.close();
    }
}

fn pending_snapshot() -> Option<PendingRequest> {
    match http_lock().notify.as_ref() {
        Some(NotifyState::Pending(pending)) => Some(pending.clone()),
        _ => None,
    }
}

/// 立即执行当前待执行请求（弹窗/主窗口里的「立即执行」）。
fn confirm_pending_now(app: &AppHandle) -> Result<(), String> {
    let pending = pending_snapshot().ok_or_else(|| "没有待执行请求".to_string())?;
    run_pending_now(app, pending);
    Ok(())
}

/// 取消当前待执行请求（关掉弹窗也走这里）。
fn cancel_pending_now(app: &AppHandle) -> Result<(), String> {
    let pending = pending_snapshot().ok_or_else(|| "没有待执行请求".to_string())?;
    {
        let mut runtime = http_lock();
        runtime.notify = None;
    }
    record_request_log(
        app,
        pending.channel,
        &pending.source,
        "（待执行请求）",
        &pending.summary,
        "cancelled",
        "本机用户取消",
    );
    trace(
        app,
        "remote",
        &format!("待执行请求 {} 被本机取消", pending.id),
    );
    close_notify_window(app);
    publish_ui(app);
    Ok(())
}

fn run_pending_now(app: &AppHandle, pending: PendingRequest) {
    {
        let mut runtime = http_lock();
        runtime.notify = None;
    }
    let (ok, message) =
        execute_remote_command(app, pending.channel, &pending.source, &pending.command);
    record_request_log(
        app,
        pending.channel,
        &pending.source,
        "（待执行请求）",
        &pending.summary,
        if ok { "executed" } else { "failed" },
        &message,
    );
    trace(
        app,
        "remote",
        &format!("待执行请求 {} 执行完成 ok={ok}", pending.id),
    );
    // dry-run / 已提交都走这里：弹窗收掉，结果进快照
    close_notify_window(app);
    publish_ui(app);
}

/// 每秒 tick 的调度线程：到点执行待执行请求、事后的 info 提示到点自动关窗。
fn spawn_notify_scheduler(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        // 锁内只做判断（Some(Some(pending)) = 到点执行，Some(None) = info 到点关窗）
        let due = {
            let runtime = http_lock();
            notify_due(runtime.notify.as_ref(), Instant::now())
        };
        match due {
            // 倒计时到点：执行
            NotifyDue::Execute(pending) => run_pending_now(&app, *pending),
            // 事后提示到期：收掉弹窗
            NotifyDue::CloseInfo => set_notify_state(&app, None),
            NotifyDue::Nothing => {}
        }
    });
}

fn http_access_hint_text(app: &AppHandle) -> String {
    let settings = stored_settings(app);
    let port = settings.http_port;
    let token = settings.http_token.trim();
    let ip = local_ipv4();
    format!(
        "# 局域网/远程（把 IP 换成目标机地址）\n\
         curl -s -X POST \"http://{ip}:{port}/api/power?action=reboot\" -H \"X-Auth-Token: {token}\"\n\
         # 本机 / ssh 隧道\n\
         curl -s \"http://127.0.0.1:{port}/api/status?token={token}\"\n\
         # 切换下次启动并重启（一条命令搞定）\n\
         curl -s -X POST \"http://{ip}:{port}/api/switch-and-power?system=windows&action=reboot\" -H \"X-Auth-Token: {token}\"\n\
         # 只切换、不重启\n\
         curl -s -X POST \"http://{ip}:{port}/api/switch?system=linux\" -H \"X-Auth-Token: {token}\""
    )
}

/// 设置类变更的统一写法：存盘 → 记录回执 → 必要时重启服务 → 发布快照。
fn update_settings_and_publish(
    app: &AppHandle,
    action: &str,
    message: &str,
    update: impl FnOnce(&mut Settings),
    after: impl FnOnce(&AppHandle),
) -> Result<SettingsView, String> {
    let mut settings = stored_settings(app);
    update(&mut settings);
    match save_settings(app, &settings) {
        Ok(_) => {
            record_action(app, action, true, message);
            after(app);
            Ok(publish_settings(app))
        }
        Err(error) => {
            record_action(app, action, false, &error);
            publish_settings(app);
            Err(error)
        }
    }
}

fn set_http_enabled_setting(app: &AppHandle, enabled: bool) {
    let _ = update_settings_and_publish(
        app,
        "HTTP 服务",
        if enabled { "已开启" } else { "已关闭" },
        |settings| settings.http_enabled = enabled,
        apply_http_config,
    );
}

fn set_notify_enabled_setting(app: &AppHandle, enabled: bool) {
    let _ = update_settings_and_publish(
        app,
        "远程请求通知",
        if enabled {
            "已开启"
        } else {
            "已关闭（只在主窗口显示待执行请求）"
        },
        |settings| settings.notify_enabled = enabled,
        |_app| {},
    );
}

fn set_delay_all_setting(app: &AppHandle, seconds: u32) {
    let seconds = seconds.min(MAX_DELAY_SECONDS);
    let _ = update_settings_and_publish(
        app,
        "远程执行延迟",
        &if seconds == 0 {
            "已设为立即执行（0 秒）".to_string()
        } else {
            format!("已设为 {seconds} 秒（两条渠道）")
        },
        |settings| settings.channels.set_delay_all(seconds),
        |_app| {},
    );
}

fn menu_system_rows(variables: &[VariableInfo]) -> Vec<VariableInfo> {
    if !variables.is_empty() {
        return variables.to_vec();
    }
    SOURCE_FILES
        .iter()
        .map(|(system, display_name, source_name)| VariableInfo {
            system: (*system).to_string(),
            display_name: (*display_name).to_string(),
            source_name: (*source_name).to_string(),
            source: PathBuf::new(),
            exists: false,
            active: false,
        })
        .collect()
}

/// 只负责构建菜单，不碰托盘图标。setup 的第一帧和后续重建走同一份实现。
fn build_tray_menu(app: &AppHandle, state: &UiState) -> tauri::Result<Menu<tauri::Wry>> {
    let busy = state.busy.affects_menu();
    let message = &state.status.message;
    let settings = &state.settings.settings;
    let variables = menu_system_rows(&state.status.variables);

    let menu = Menu::new(app)?;
    let status = MenuItem::with_id(app, "status", message, false, None::<&str>)?;
    menu.append(&status)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    for variable in &variables {
        let label = if variable.active {
            format!("● {}", variable.display_name)
        } else {
            variable.display_name.clone()
        };
        let item = MenuItem::with_id(
            app,
            variable.system.clone(),
            label,
            variable.exists && !busy,
            None::<&str>,
        )?;
        menu.append(&item)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if state.status.temporary_mount {
        let unmount = MenuItem::with_id(
            app,
            "unmount-temporary",
            "卸载临时挂载",
            !busy,
            None::<&str>,
        )?;
        menu.append(&unmount)?;
    }
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    menu.append(&show)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    let settings_label = MenuItem::with_id(app, "settings-label", "应用设置", false, None::<&str>)?;
    menu.append(&settings_label)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "setting-autostart",
        "开机启动",
        true,
        settings.autostart,
        None::<&str>,
    )?;
    menu.append(&autostart)?;
    // 勾选项的 id 带「点下去要变成的状态」：系统会先把勾选翻转，分发时必须知道用户想要哪个值。
    // 以前这两个 id 没有后缀、而分发只认带后缀的 id → 点了等于没点（阶段 1 修掉的真实 bug）。
    let start_in_tray = CheckMenuItem::with_id(
        app,
        toggle_menu_id("setting-start-in-tray", !settings.start_in_tray),
        "启动时隐藏主界面",
        true,
        settings.start_in_tray,
        None::<&str>,
    )?;
    menu.append(&start_in_tray)?;
    let hide_from_dock_taskbar = CheckMenuItem::with_id(
        app,
        toggle_menu_id(
            "setting-hide-from-dock-taskbar",
            !settings.hide_from_dock_taskbar,
        ),
        "隐藏程序（任务栏/Dock）",
        true,
        settings.hide_from_dock_taskbar,
        None::<&str>,
    )?;
    menu.append(&hide_from_dock_taskbar)?;
    let close_action_menu = Submenu::with_id(app, "setting-close-action", "点击窗口 ❌ 时", true)?;
    for (action, label) in [
        (CloseAction::Tray, "最小化到托盘"),
        (CloseAction::Quit, "退出程序"),
    ] {
        let item = CheckMenuItem::with_id(
            app,
            close_action_menu_id(action),
            label,
            true,
            settings.close_action == action,
            None::<&str>,
        )?;
        close_action_menu.append(&item)?;
    }
    menu.append(&close_action_menu)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    let refresh = MenuItem::with_id(app, "refresh", "扫描", !busy, None::<&str>)?;
    menu.append(&refresh)?;
    // 电源动作子菜单：重启（默认）/ 关机 / 睡眠
    let power_menu = Submenu::with_id(app, "power", "电源操作", !busy)?;
    for action in [
        PowerAction::Reboot,
        PowerAction::Shutdown,
        PowerAction::Sleep,
    ] {
        let item = MenuItem::with_id(
            app,
            power_menu_id(action),
            action.label(),
            !busy,
            None::<&str>,
        )?;
        power_menu.append(&item)?;
    }
    menu.append(&power_menu)?;
    // 远程控制：HTTP 服务开关 / 通知总开关 / 延迟预设 / 打开设置区
    let remote_menu = Submenu::with_id(app, "remote", "远程控制", true)?;
    remote_menu.append(&CheckMenuItem::with_id(
        app,
        toggle_menu_id("setting-http-enabled", !settings.http_enabled),
        "HTTP 服务",
        true,
        settings.http_enabled,
        None::<&str>,
    )?)?;
    remote_menu.append(&CheckMenuItem::with_id(
        app,
        toggle_menu_id("setting-notify-enabled", !settings.notify_enabled),
        "远程请求通知",
        true,
        settings.notify_enabled,
        None::<&str>,
    )?)?;
    let delay_menu = Submenu::with_id(app, "setting-delay", "远程执行延迟", true)?;
    let delay_now = settings.channels.http.delay_seconds;
    let delay_same = delay_now == settings.channels.cli.delay_seconds;
    for seconds in [0u32, 3, 5, 10, 30] {
        let label = if seconds == 0 {
            "立即执行（0 秒）".to_string()
        } else {
            format!("{seconds} 秒")
        };
        delay_menu.append(&CheckMenuItem::with_id(
            app,
            format!("setting-delay:{seconds}"),
            label,
            true,
            delay_same && delay_now == seconds,
            None::<&str>,
        )?)?;
    }
    remote_menu.append(&delay_menu)?;
    remote_menu.append(&MenuItem::with_id(
        app,
        "show-remote",
        "远程控制设置…",
        true,
        None::<&str>,
    )?)?;
    menu.append(&remote_menu)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    menu.append(&quit)?;
    Ok(menu)
}

/// 用同一份快照刷新托盘菜单；**内容没变就不重建**（指纹相同直接返回）。
///
/// 阶段 0 trace 实测：启动时扫描两次、每次都全量重建菜单。
/// 菜单被无意义地替换正是「扫描期间菜单闪断 / 要点两次」的诱因。
fn menu_signature_for(state: &UiState) -> String {
    menu_signature(
        &state.status.message,
        state.status.temporary_mount,
        &state.settings.settings,
        &menu_system_rows(&state.status.variables),
        state.status.error.as_deref(),
        &state.busy,
    )
}

/// 记住当前菜单内容，让后续相同内容的刷新直接跳过重建。
fn remember_menu_signature(app: &AppHandle, signature: String) {
    *app.state::<Mutex<Option<String>>>()
        .lock()
        .expect("menu signature mutex poisoned") = Some(signature);
}

fn update_tray_menu(app: &AppHandle, state: &UiState, trigger: &str) -> tauri::Result<()> {
    let signature = menu_signature_for(state);
    {
        let cache = app.state::<Mutex<Option<String>>>();
        let mut cache = cache.lock().expect("menu signature mutex poisoned");
        if cache.as_deref() == Some(signature.as_str()) {
            trace(
                app,
                "tray",
                &format!("菜单内容未变，跳过重建 trigger={trigger} sig={signature}"),
            );
            return Ok(());
        }
        *cache = Some(signature.clone());
    }

    trace(
        app,
        "tray",
        &format!("重建菜单 trigger={trigger} sig={signature}"),
    );
    let menu = build_tray_menu(app, state)?;
    if let Some(tray) = app.tray_by_id("main") {
        tray.set_menu(Some(menu))?;
        let tooltip = state
            .status
            .error
            .clone()
            .unwrap_or_else(|| format!("AnyBoot：{}", state.status.message));
        tray.set_tooltip(Some(tooltip))?;
        trace(app, "tray", "菜单与 tooltip 已更新");
    } else {
        trace(app, "tray", "未找到托盘图标 main，菜单没有挂上");
    }
    Ok(())
}

/// 勾选项 id 的生成：name + 点下去要变成的状态。
fn toggle_menu_id(name: &str, target: bool) -> String {
    format!("{name}:{target}")
}

fn close_action_menu_id(action: CloseAction) -> String {
    format!("setting-close-action:{action:?}").to_lowercase()
}

/// 电源动作菜单项 id：`power:reboot` / `power:shutdown` / `power:sleep`。
fn power_menu_id(action: PowerAction) -> String {
    format!("power:{}", action.as_str())
}

fn parse_menu_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// 托盘菜单项的语义。把「字符串前后缀匹配」换成可单测的纯解析（阶段 1 修 D6）。
#[derive(Clone, Debug, PartialEq, Eq)]
enum MenuAction {
    Show,
    Refresh,
    UnmountTemporary,
    Switch(String),
    /// 电源动作：重启 / 关机 / 睡眠
    Power(PowerAction),
    /// HTTP 服务开关
    SetHttpEnabled(bool),
    /// 远程请求通知总开关
    SetNotifyEnabled(bool),
    /// 远程执行延迟（同时作用于两条渠道）
    SetDelayAll(u32),
    /// 打开主窗口并定位到「远程控制」区
    ShowRemote,
    SetAutostart,
    SetStartInTray(bool),
    SetHideFromDockTaskbar(bool),
    SetCloseAction(CloseAction),
    Quit,
}

impl MenuAction {
    fn from_id(id: &str) -> Option<Self> {
        if let Some(rest) = id.strip_prefix("setting-close-action:") {
            return CloseAction::parse(rest.trim()).map(Self::SetCloseAction);
        }
        if let Some(rest) = id.strip_prefix("setting-start-in-tray:") {
            return parse_menu_bool(rest).map(Self::SetStartInTray);
        }
        if let Some(rest) = id.strip_prefix("setting-hide-from-dock-taskbar:") {
            return parse_menu_bool(rest).map(Self::SetHideFromDockTaskbar);
        }
        if let Some(rest) = id.strip_prefix("power:") {
            return PowerAction::parse(rest).map(Self::Power);
        }
        if let Some(rest) = id.strip_prefix("setting-http-enabled:") {
            return parse_menu_bool(rest).map(Self::SetHttpEnabled);
        }
        if let Some(rest) = id.strip_prefix("setting-notify-enabled:") {
            return parse_menu_bool(rest).map(Self::SetNotifyEnabled);
        }
        if let Some(rest) = id.strip_prefix("setting-delay:") {
            return rest.trim().parse::<u32>().ok().map(Self::SetDelayAll);
        }
        match id {
            "show" => Some(Self::Show),
            "refresh" => Some(Self::Refresh),
            "unmount-temporary" => Some(Self::UnmountTemporary),
            // 旧版 id：只有一个「重启系统」项时的写法，保留兼容
            "restart" => Some(Self::Power(PowerAction::Reboot)),
            "setting-autostart" => Some(Self::SetAutostart),
            "show-remote" => Some(Self::ShowRemote),
            "quit" => Some(Self::Quit),
            "linux" | "windows" | "macos" => Some(Self::Switch(id.to_string())),
            _ => None,
        }
    }
}

/// 托盘菜单事件入口：原始 id → 解析 → 分发，每一步都写 trace（现场取证）。
fn handle_menu_event(app: &AppHandle, id: &str) {
    trace(app, "menu-action", &format!("id={id}"));
    let Some(action) = MenuAction::from_id(id) else {
        trace(app, "menu-action", &format!("id={id} 未识别，已忽略"));
        return;
    };
    trace(app, "menu-action", &format!("id={id} → {action:?}"));
    // 窗口操作留在事件线程上执行；其余动作放工作线程（可能耗时或要授权）。
    if action == MenuAction::Show {
        show_main_window(app);
        return;
    }
    spawn_menu_action(app, action);
}

fn spawn_menu_action(app: &AppHandle, action: MenuAction) {
    let app = app.clone();
    std::thread::spawn(move || match action {
        MenuAction::Show => show_main_window(&app),
        MenuAction::Refresh => {
            refresh_and_update(&app);
        }
        MenuAction::UnmountTemporary => {
            unmount_temporary(app);
        }
        MenuAction::Switch(system) => {
            switch_and_update(&app, &system);
        }
        MenuAction::Power(action) => spawn_power(&app, action, true),
        MenuAction::SetHttpEnabled(enabled) => set_http_enabled_setting(&app, enabled),
        MenuAction::SetNotifyEnabled(enabled) => set_notify_enabled_setting(&app, enabled),
        MenuAction::SetDelayAll(seconds) => set_delay_all_setting(&app, seconds),
        MenuAction::ShowRemote => {
            show_main_window(&app);
            if let Err(error) = app.emit("focus-remote", ()) {
                trace(
                    &app,
                    "tray",
                    &format!("通知主窗口定位远程控制失败: {error}"),
                );
            }
        }
        MenuAction::SetAutostart => toggle_autostart_setting(&app),
        MenuAction::SetStartInTray(enabled) => {
            update_tray_setting(&app, TraySetting::StartInTray, enabled)
        }
        MenuAction::SetHideFromDockTaskbar(enabled) => {
            update_tray_setting(&app, TraySetting::HideFromDockTaskbar, enabled)
        }
        MenuAction::SetCloseAction(close_action) => update_close_action_setting(&app, close_action),
        MenuAction::Quit => app.exit(0),
    });
}

/// 配置目录：默认是系统的 `app_config_dir`；
/// `ANYBOOT_CONFIG_DIR` 能把它指到别处（做验证/多实例时就不会碰你的真实配置）。
fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Ok(directory) = std::env::var("ANYBOOT_CONFIG_DIR") {
        let directory = directory.trim();
        if !directory.is_empty() {
            return Ok(PathBuf::from(directory));
        }
    }
    app.path()
        .app_config_dir()
        .map_err(|error| format!("无法定位应用配置目录: {error}"))
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join(SETTINGS_FILE))
}

fn read_settings_file(app: &AppHandle) -> Settings {
    // 启动早期调用：此刻内存里的设置还是默认值，所以只有环境变量
    // ANYBOOT_TRACE 能打开这里的日志（设置里的 debugTrace 要等读完才生效）。
    let path = match settings_path(app) {
        Ok(path) => path,
        Err(error) => {
            trace(app, "settings-file", &format!("无法定位设置文件: {error}"));
            return Settings::default();
        }
    };
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => {
            trace(
                app,
                "settings-file",
                &format!("设置文件读取失败，使用默认值: {error}"),
            );
            return Settings::default();
        }
    };
    match serde_json::from_str::<Settings>(&contents) {
        Ok(settings) => {
            trace(
                app,
                "settings-file",
                &format!(
                    "已读取 {} debugTrace={}",
                    path.display(),
                    settings.debug_trace
                ),
            );
            settings
        }
        Err(error) => {
            trace(
                app,
                "settings-file",
                &format!("设置文件解析失败，使用默认值: {error}"),
            );
            Settings::default()
        }
    }
}

fn write_settings_file(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "设置文件路径无效".to_string())?;
    fs::create_dir_all(directory).map_err(|error| format!("无法创建配置目录: {error}"))?;
    let contents = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("无法序列化设置: {error}"))?;
    let temporary = temporary_target(&path);
    let result = (|| -> io::Result<()> {
        fs::write(&temporary, contents.as_bytes())?;
        fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    match &result {
        Ok(()) => trace(
            app,
            "settings-file",
            &format!(
                "已保存 path={} autostart={} start_in_tray={} hide_from_dock_taskbar={} close_action={:?} debugTrace={} http={{enabled={} bind={} port={} token_len={}}} notify={{enabled={} http={{notify={} delay={}}} cli={{notify={} delay={}}}}} remoteAllowGuiAuth={}",
                path.display(),
                settings.autostart,
                settings.start_in_tray,
                settings.hide_from_dock_taskbar,
                settings.close_action,
                settings.debug_trace,
                settings.http_enabled,
                settings.http_bind,
                settings.http_port,
                settings.http_token.trim().len(),
                settings.notify_enabled,
                settings.channels.http.notify,
                settings.channels.http.delay_seconds,
                settings.channels.cli.notify,
                settings.channels.cli.delay_seconds,
                settings.remote_allow_gui_auth
            ),
        ),
        Err(error) => trace(app, "settings-file", &format!("保存失败: {error}")),
    }
    result.map_err(|error| format!("无法保存设置: {error}"))
}

fn stored_settings(app: &AppHandle) -> Settings {
    let state = app.state::<Mutex<Settings>>();
    let settings = state.lock().expect("settings mutex poisoned").clone();
    settings
}

/// 保存设置。注意：调用者**不能**在持有 Settings 锁时调用（trace 会再加一次锁）。
fn save_settings(app: &AppHandle, settings: &Settings) -> Result<Settings, String> {
    write_settings_file(app, settings)?;
    let state = app.state::<Mutex<Settings>>();
    *state.lock().expect("settings mutex poisoned") = settings.clone();
    Ok(settings.clone())
}

/// macOS 13+ 使用系统登录项（SMAppService），开机启动会出现在
/// 「系统设置 → 通用 → 登录项与扩展 → 登入时打开」里。
#[cfg(target_os = "macos")]
mod login_item {
    use objc2::rc::Retained;
    use objc2::runtime::AnyClass;
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    use super::AutostartState;

    pub fn is_supported() -> bool {
        AnyClass::get(c"SMAppService").is_some()
    }

    fn main_app() -> Result<Retained<SMAppService>, String> {
        if !is_supported() {
            return Err("当前系统版本不支持登录项（需要 macOS 13 及以上）".to_string());
        }
        Ok(unsafe { SMAppService::mainAppService() })
    }

    pub fn state() -> AutostartState {
        let Ok(service) = main_app() else {
            eprintln!("SMAppService 类不可用（macOS 低于 13）");
            return AutostartState::Unavailable;
        };
        let status = unsafe { service.status() };
        // SMAppServiceStatus 枚举: 0=NotRegistered, 1=Enabled, 2=RequiresApproval,
        // 3=NotFound。应用被更新或移动后，旧注册会短暂返回 NotFound(3)；
        // 不能把它当作 Unavailable（会误导为“未以 .app 运行”），
        // 应视为未注册，让注册流程/启动自愈重新注册。
        if status == SMAppServiceStatus::Enabled {
            AutostartState::Enabled
        } else if status == SMAppServiceStatus::RequiresApproval {
            AutostartState::RequiresApproval
        } else if status == SMAppServiceStatus::NotRegistered
            || status == SMAppServiceStatus::NotFound
        {
            AutostartState::Disabled
        } else {
            AutostartState::Unavailable
        }
    }

    pub fn set(enabled: bool) -> Result<AutostartState, String> {
        let service = main_app()?;
        let mut result = unsafe {
            if enabled {
                service.registerAndReturnError()
            } else {
                service.unregisterAndReturnError()
            }
        };
        // 应用被更新/替换后，旧注册可能处于失效状态，直接注册会失败；
        // 先注销失效的旧注册再重试一次（Apple 推荐的恢复方式）。
        if result.is_err() && enabled {
            let _ = unsafe { service.unregisterAndReturnError() };
            result = unsafe { service.registerAndReturnError() };
        }
        let state = state();
        match result {
            Ok(()) => Ok(state),
            Err(error) => {
                let already_there = if enabled {
                    state.is_on()
                } else {
                    state == AutostartState::Disabled
                };
                if already_there {
                    return Ok(state);
                }
                let action = if enabled { "开启" } else { "关闭" };
                Err(format!(
                    "无法{action}开机启动: {} (code={})",
                    error.localizedDescription(),
                    error.code()
                ))
            }
        }
    }

    pub fn open_system_settings() -> Result<(), String> {
        main_app()?;
        unsafe { SMAppService::openSystemSettingsLoginItems() };
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn macos_autostart_state(app: &AppHandle) -> AutostartState {
    match autostart_backend() {
        AutostartBackend::LoginItem => login_item::state(),
        AutostartBackend::LaunchAgent => legacy_autostart_state(app),
    }
}

#[cfg(target_os = "macos")]
fn macos_autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    match autostart_backend() {
        AutostartBackend::LoginItem => match login_item::set(enabled) {
            Ok(state) => {
                clear_login_item_failure();
                Ok(state)
            }
            // 已签名的应用也可能因为系统登录项数据库状态异常而注册失败，
            // 这时退回 LaunchAgent，保证开机启动开关依然生效。
            Err(error) => {
                eprintln!("系统登录项接口调用失败，改用兼容方式: {error}");
                note_login_item_failure(&error);
                legacy_autostart_set(app, enabled)
            }
        },
        AutostartBackend::LaunchAgent => legacy_autostart_set(app, enabled),
    }
}

fn legacy_autostart_state(app: &AppHandle) -> AutostartState {
    match app.autolaunch().is_enabled() {
        Ok(true) => AutostartState::Enabled,
        Ok(false) => AutostartState::Disabled,
        Err(_) => AutostartState::Unavailable,
    }
}

fn legacy_autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    let manager = app.autolaunch();
    let action = if enabled { "开启" } else { "关闭" };
    if enabled {
        manager
            .enable()
            .map_err(|error| format!("无法{action}开机启动: {error}"))?;
    } else {
        manager
            .disable()
            .map_err(|error| format!("无法{action}开机启动: {error}"))?;
    }
    Ok(legacy_autostart_state(app))
}

#[cfg(target_os = "macos")]
fn autostart_state(app: &AppHandle) -> AutostartState {
    macos_autostart_state(app)
}

#[cfg(target_os = "linux")]
fn autostart_state(app: &AppHandle) -> AutostartState {
    linux_autostart_state(app)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn autostart_state(app: &AppHandle) -> AutostartState {
    legacy_autostart_state(app)
}

#[cfg(target_os = "macos")]
fn autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    macos_autostart_set(app, enabled)
}

#[cfg(target_os = "linux")]
fn autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    linux_autostart_set(app, enabled)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    legacy_autostart_set(app, enabled)
}
#[cfg(target_os = "linux")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "linux")]
fn linux_autostart_file(app: &AppHandle) -> Result<PathBuf, String> {
    let invoking_user = if is_root() {
        linux_invoking_user_home()
    } else {
        Some(
            app.path()
                .home_dir()
                .map_err(|error| format!("无法定位用户主目录: {error}"))?,
        )
    };
    let home = invoking_user.ok_or_else(|| "无法识别当前登录用户".to_string())?;
    let directory = home.join(".config").join("autostart");
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建自启动目录: {error}"))?;
    let name = app.package_info().name.replace([' ', '/'], "-");
    Ok(directory.join(format!("{name}.desktop")))
}

#[cfg(target_os = "linux")]
fn linux_autostart_state(app: &AppHandle) -> AutostartState {
    match linux_autostart_file(app) {
        Ok(path) if path.exists() => AutostartState::Enabled,
        Ok(_) => AutostartState::Disabled,
        Err(_) => AutostartState::Unavailable,
    }
}

#[cfg(target_os = "linux")]
fn linux_autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    let path = linux_autostart_file(app)?;
    if path.exists() {
        let stale = fs::read_to_string(&path)
            .map(|contents| !contents.contains("AnyBoot") && !contents.contains("anyboot"))
            .unwrap_or(false);
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
    if !enabled {
        if path.exists() {
            fs::remove_file(&path).map_err(|error| format!("无法关闭开机启动: {error}"))?;
        }
        return Ok(AutostartState::Disabled);
    }

    let executable = std::env::current_exe()
        .map_err(|error| format!("无法定位当前程序: {error}"))?
        .canonicalize()
        .map_err(|error| format!("无法解析当前程序路径: {error}"))?;
    let appimage = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    let launch_path = appimage.unwrap_or(executable);
    let content = format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName={}\nComment=AnyBoot\nExec={} --from-autostart\nStartupNotify=false\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        app.package_info().name,
        shell_quote(&launch_path.to_string_lossy())
    );
    fs::write(&path, content).map_err(|error| format!("无法写入用户自启动项: {error}"))?;
    Ok(AutostartState::Enabled)
}

/// 早期版本在 macOS 上写入 ~/Library/LaunchAgents 实现开机启动，
/// 改用系统登录项后需要清理，否则登录时会被重复拉起。
#[cfg(target_os = "macos")]
fn remove_legacy_launch_agents(app: &AppHandle) {
    let Ok(home) = app.path().home_dir() else {
        return;
    };
    let directory = home.join("Library").join("LaunchAgents");
    let name = app.package_info().name.clone();
    let identifier = app.config().identifier.clone();
    let candidates = [
        format!("{name}.plist"),
        format!("{}.plist", name.replace(' ', "-")),
        format!("{identifier}.plist"),
    ];
    for file in candidates {
        let path = directory.join(&file);
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        if !contents.contains("anyboot") && !contents.contains(&name) {
            continue;
        }
        let _ = Command::new("launchctl")
            .arg("unload")
            .arg("--")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = fs::remove_file(&path);
    }
}

fn view_settings(app: &AppHandle) -> SettingsView {
    let state = autostart_state(app);
    let mut settings = stored_settings(app);
    if settings.autostart != state.is_on() {
        settings.autostart = state.is_on();
        settings = save_settings(app, &settings).unwrap_or(settings);
    }
    SettingsView::new(settings, state)
}

/// 设置类命令的统一出口：发布新快照，并把 SettingsView 回给调用方。
///
/// 主窗口与托盘菜单共用 publish_ui 这一份快照，不再各读各的设置
/// （阶段 1 修 D1：以前托盘读设置文件里的旧值、主窗口读系统真实值）。
fn publish_settings(app: &AppHandle) -> SettingsView {
    let state = publish_ui(app);
    trace(
        app,
        "settings",
        &format!(
            "已同步 autostart={} autostart_state={} start_in_tray={} hide_from_dock_taskbar={} close_action={:?}",
            state.settings.settings.autostart,
            state.settings.autostart_state,
            state.settings.settings.start_in_tray,
            state.settings.settings.hide_from_dock_taskbar,
            state.settings.settings.close_action
        ),
    );
    state.settings
}

#[cfg(target_os = "macos")]
fn cleanup_legacy_autostart(app: &AppHandle) {
    // 只有系统登录项真正生效时才清理 LaunchAgent 文件，否则会把兼容方式
    // （未签名 / 未以 .app 运行）下正在使用的自启动项一起删掉。
    if autostart_backend() == AutostartBackend::LoginItem
        && login_item::state() == AutostartState::Enabled
    {
        remove_legacy_launch_agents(app);
    }
}

#[cfg(not(target_os = "macos"))]
fn cleanup_legacy_autostart(_app: &AppHandle) {}

/// 应用被更新或移动后，登录项可能失效；启动时按已保存的设置自愈一次。
fn startup_autostart_sync(app: &AppHandle) {
    cleanup_legacy_autostart(app);
    let desired = stored_settings(app).autostart;
    let state = autostart_state(app);
    eprintln!(
        "启动时检查开机启动：设置={desired}，系统状态={}",
        state.as_str()
    );
    if desired && state == AutostartState::Disabled {
        match autostart_set(app, true) {
            Ok(new_state) => eprintln!("已重新注册开机启动登录项，状态: {}", new_state.as_str()),
            Err(error) => eprintln!("重新注册开机启动登录项失败: {error}"),
        }
    }
}

fn show_main_window(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    if !stored_settings(app).hide_from_dock_taskbar {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.unminimize();
        // 呼出即置顶：Linux 下 WM 的 focus-stealing-prevention 会让 set_focus 无效，
        // 于是窗口被别的窗口盖住（用户看到的就是「点了托盘但窗口没跳出来」）。
        // 失焦时会自动取消置顶（见 on_window_event），所以不会永久霸屏。
        let _ = window.set_always_on_top(true);
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn unmount_temporary(app: AppHandle) -> AppStatus {
    set_busy(&app, |busy| busy.unmounting = true);
    let status = unmount_temporary_inner(&app);
    set_busy(&app, |busy| busy.unmounting = false);
    status
}

fn unmount_temporary_inner(app: &AppHandle) -> AppStatus {
    let mount = {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state
            .active_mount
            .take_if(|mount| mount.owned)
            .map(|mount| mount.clone())
    };

    let Some(mount) = mount else {
        {
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            state.message = "没有应用创建的临时挂载".to_string();
            state.error = None;
        }
        let status = state_snapshot(app);
        publish_status(app, &status);
        return status;
    };

    let mountpoint = mount.mountpoint.clone();

    let status = match mount.clone().unmount() {
        Ok(()) => {
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            state.vars_dir = None;
            state.variables = Vec::new();
            state.message = "已卸载应用创建的临时挂载".to_string();
            state.error = None;
            AppStatus {
                vars_dir: None,
                temporary_mount: false,
                variables: Vec::new(),
                message: state.message.clone(),
                error: None,
            }
        }
        Err(error) => {
            let message = "卸载临时挂载失败".to_string();
            let detail = format!("无法卸载 {}: {error}", mountpoint.display());
            {
                // 失败要把挂载状态放回去，同时把提示写进 AppState，
                // 否则主窗口读到的还是上一次的成功文案（阶段 1 修同类不一致）。
                let state = app.state::<Mutex<AppState>>();
                let mut state = state.lock().expect("state mutex poisoned");
                state.active_mount = Some(mount);
                state.message = message.clone();
                state.error = Some(detail.clone());
            }
            let mut status = state_snapshot(app);
            status.message = message;
            status.error = Some(detail);
            status
        }
    };
    publish_status(app, &status);
    status
}

fn toggle_autostart_setting(app: &AppHandle) {
    let enabled = !stored_settings(app).autostart;
    trace(
        app,
        "tray-setting",
        &format!("setting-autostart → enabled={enabled}"),
    );
    match autostart_set(app, enabled).and_then(|state| {
        let mut settings = stored_settings(app);
        settings.autostart = state.is_on();
        save_settings(app, &settings)
    }) {
        Ok(_) => {
            record_action(
                app,
                "开机启动",
                true,
                if enabled {
                    "已开启开机启动"
                } else {
                    "已关闭开机启动"
                },
            );
            publish_settings(app);
        }
        Err(error) => {
            // 失败时也要同步一次：让主窗口看到系统里的真实状态和提示。
            record_action(app, "开机启动", false, &error);
            publish_settings(app);
            trace(app, "tray-setting", &format!("切换开机启动失败: {error}"));
        }
    }
}

/// 托盘里能直接切的勾选设置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TraySetting {
    StartInTray,
    HideFromDockTaskbar,
}

impl TraySetting {
    fn label(self) -> &'static str {
        match self {
            Self::StartInTray => "启动时隐藏主界面",
            Self::HideFromDockTaskbar => "隐藏程序（任务栏/Dock）",
        }
    }
}

/// 托盘切设置。**失败必须回滚**（阶段 1 修 D5）：
/// 以前是先存盘再 apply，apply 失败时设置文件里已经是新值，
/// 界面勾选框显示「已生效」而系统实际没变——界面在说假话。
fn update_tray_setting(app: &AppHandle, setting: TraySetting, enabled: bool) {
    trace(
        app,
        "tray-setting",
        &format!("{setting:?} enabled={enabled}"),
    );
    let previous = stored_settings(app);
    let mut settings = previous.clone();
    match setting {
        TraySetting::StartInTray => settings.start_in_tray = enabled,
        TraySetting::HideFromDockTaskbar => settings.hide_from_dock_taskbar = enabled,
    }
    if let Err(error) = save_settings(app, &settings) {
        record_action(app, setting.label(), false, &error);
        publish_ui(app);
        return;
    }
    let apply_result = match setting {
        TraySetting::HideFromDockTaskbar => apply_dock_taskbar_visibility(app, enabled),
        TraySetting::StartInTray => Ok(()),
    };
    match apply_result {
        Ok(()) => {
            let message = format!(
                "{}：{}",
                setting.label(),
                if enabled { "已开启" } else { "已关闭" }
            );
            record_action(app, setting.label(), true, &message);
        }
        Err(error) => {
            // 回滚到旧值：界面必须显示系统里的真实状态
            let _ = save_settings(app, &previous);
            record_action(
                app,
                setting.label(),
                false,
                &format!("{error}（已回滚为原值）"),
            );
        }
    }
    publish_ui(app);
}

fn update_close_action_setting(app: &AppHandle, close_action: CloseAction) {
    trace(
        app,
        "tray-setting",
        &format!("点击 ❌ 时 → {close_action:?}"),
    );
    let mut settings = stored_settings(app);
    settings.close_action = close_action;
    match save_settings(app, &settings) {
        Ok(_) => {
            record_action(
                app,
                "点击 ❌ 时",
                true,
                match close_action {
                    CloseAction::Quit => "已设置：退出程序",
                    CloseAction::Tray => "已设置：最小化到托盘",
                },
            );
        }
        Err(error) => record_action(app, "点击 ❌ 时", false, &error),
    }
    publish_settings(app);
}

/// 电源动作：重启 / 关机 / 睡眠。缺省是重启（HTTP / CLI 不传就是它）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PowerAction {
    Reboot,
    Shutdown,
    Sleep,
}

impl Default for PowerAction {
    fn default() -> Self {
        Self::Reboot
    }
}

impl PowerAction {
    /// 解析动作名：大小写不敏感，同时接受常见别名。
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "reboot" | "restart" => Some(Self::Reboot),
            "shutdown" | "poweroff" | "halt" => Some(Self::Shutdown),
            "sleep" | "suspend" => Some(Self::Sleep),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Reboot => "reboot",
            Self::Shutdown => "shutdown",
            Self::Sleep => "sleep",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Reboot => "重启系统",
            Self::Shutdown => "关机",
            Self::Sleep => "睡眠",
        }
    }

    /// 提交成功后的回执文案。
    fn submitted_message(self) -> &'static str {
        match self {
            Self::Reboot => "已提交重启，系统即将重启",
            Self::Shutdown => "已提交关机，系统即将关机",
            Self::Sleep => "已提交睡眠",
        }
    }
}

/// 一条候选命令。
#[derive(Clone, Debug, PartialEq, Eq)]
struct PowerAttempt {
    program: &'static str,
    args: Vec<String>,
    /// 需要本机图形授权（macOS 管理员框 / Linux pkexec）：
    /// 远程与 CLI 默认整条跳过（Q3），只有本机点击才允许弹出
    needs_gui_auth: bool,
}

impl PowerAttempt {
    fn command(&self) -> String {
        render_command(self.program, &self.args)
    }
}

fn render_command(program: &str, args: &[String]) -> String {
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

fn attempt(program: &'static str, args: &[&str], needs_gui_auth: bool) -> PowerAttempt {
    PowerAttempt {
        program,
        args: args.iter().map(|arg| arg.to_string()).collect(),
        needs_gui_auth,
    }
}

/// 一次候选命令执行的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
enum PowerAttemptOutcome {
    /// 命令执行成功，动作已提交。
    Success,
    /// 系统里没有这个命令，可以换下一个候选。
    Missing(String),
    /// 命令存在但执行失败，通常是权限不足或用户取消了授权弹窗。
    Failed(String),
}

impl PowerAttemptOutcome {
    fn describe(&self) -> String {
        match self {
            Self::Success => "已提交".to_string(),
            Self::Missing(reason) | Self::Failed(reason) => reason.clone(),
        }
    }
}

/// 一条尝试失败之后该做什么。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PowerStep {
    /// 动作已提交，结束。
    Done,
    /// 换下一个候选命令（例如发行版里没有 systemctl）。
    Next,
    /// 权限不足，用 pkexec 提权重试同一条命令。
    Escalate,
    /// 停手：命令能跑但失败了，多半是用户取消了授权，不能换个方式照样执行。
    Stop,
}

fn next_power_step(outcome: &PowerAttemptOutcome, can_escalate: bool) -> PowerStep {
    match outcome {
        PowerAttemptOutcome::Success => PowerStep::Done,
        PowerAttemptOutcome::Missing(_) => PowerStep::Next,
        PowerAttemptOutcome::Failed(_) if can_escalate => PowerStep::Escalate,
        PowerAttemptOutcome::Failed(_) => PowerStep::Stop,
    }
}

/// 各平台的候选命令，按顺序尝试（见 plan §3.2）。
///
/// macOS 用 AppleScript 弹系统自带的管理员授权框（应用自身不需要 root）；
/// Linux 先试免密的 systemctl（本地活动会话下 logind 允许），失败再走 pkexec；
/// Windows 用自带的 shutdown / rundll32。
#[cfg(target_os = "macos")]
fn power_attempts(action: PowerAction) -> Vec<PowerAttempt> {
    /// 走系统管理员授权框执行一条 shell 命令。
    fn admin_script(command: &str) -> PowerAttempt {
        let script = format!("do shell script \"{command}\" with administrator privileges");
        attempt("osascript", &["-e", &script], true)
    }

    match action {
        PowerAction::Reboot => vec![admin_script("/sbin/shutdown -r now")],
        PowerAction::Shutdown => vec![admin_script("/sbin/shutdown -h now")],
        PowerAction::Sleep => vec![
            // 免 root 的睡眠：首次会要一次「自动化」授权（不是管理员密码框）
            attempt(
                "osascript",
                &["-e", "tell application \"System Events\" to sleep"],
                false,
            ),
            // 配了免密 sudo 的机器：远程也能用（sudo -n 不会弹密码框）
            attempt("sudo", &["-n", "/usr/bin/pmset", "sleepnow"], false),
            // 兜底：管理员授权框
            admin_script("/usr/bin/pmset sleepnow"),
        ],
    }
}

#[cfg(target_os = "linux")]
fn power_attempts(action: PowerAction) -> Vec<PowerAttempt> {
    match action {
        PowerAction::Reboot => vec![
            attempt("systemctl", &["reboot"], false),
            // 没有 systemd 的发行版（elogind 等）用 loginctl。
            attempt("loginctl", &["reboot"], false),
        ],
        PowerAction::Shutdown => vec![
            attempt("systemctl", &["poweroff"], false),
            attempt("loginctl", &["poweroff"], false),
        ],
        PowerAction::Sleep => vec![
            attempt("systemctl", &["suspend"], false),
            attempt("loginctl", &["suspend"], false),
        ],
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn power_attempts(action: PowerAction) -> Vec<PowerAttempt> {
    match action {
        PowerAction::Reboot => vec![attempt("shutdown", &["/r", "/t", "0"], false)],
        PowerAction::Shutdown => vec![attempt("shutdown", &["/s", "/t", "0"], false)],
        // 系统启用休眠时 Windows 会休眠而不是睡眠：只读预检会如实说明（Q10′ 方案 C）
        PowerAction::Sleep => vec![attempt(
            "rundll32.exe",
            &["powrprof.dll,SetSuspendState", "0,1,0"],
            false,
        )],
    }
}

/// 需要管理员权限时的等价命令：Linux 用 pkexec 弹图形授权框。
/// 其他平台没有额外方案（macOS 的候选命令本身就是提权的）。
fn escalate_power(attempt: &PowerAttempt, allow_gui_auth: bool) -> Option<PowerAttempt> {
    #[cfg(target_os = "linux")]
    {
        if is_root() || attempt.needs_gui_auth || !allow_gui_auth {
            return None;
        }
        let mut args = Vec::with_capacity(attempt.args.len() + 1);
        args.push(attempt.program.to_string());
        args.extend(attempt.args.iter().cloned());
        Some(PowerAttempt {
            program: "pkexec",
            args,
            needs_gui_auth: true,
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (attempt, allow_gui_auth);
        None
    }
}

/// 真的去执行一条候选命令（dry-run 时不会走到这里）。
fn run_power_command(program: &str, args: &[String]) -> PowerAttemptOutcome {
    let display = render_command(program, args);
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => PowerAttemptOutcome::Success,
        Ok(output) => {
            let code = output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "未知".to_string());
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if detail.is_empty() {
                PowerAttemptOutcome::Failed(format!("{display} 失败（退出码 {code}）"))
            } else {
                PowerAttemptOutcome::Failed(format!("{display} 失败（退出码 {code}）：{detail}"))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            PowerAttemptOutcome::Missing(format!("{display}：{error}"))
        }
        Err(error) => PowerAttemptOutcome::Failed(format!("{display} 无法执行: {error}")),
    }
}

/// 一次电源动作的执行回执。
#[derive(Clone, Debug)]
struct PowerReport {
    /// dry-run：只列出「将会执行」的命令，没有真的执行
    dry_run: bool,
    /// 实际尝试过的命令（dry-run 时是将会执行的命令）
    commands: Vec<String>,
}

/// dry-run 探针：`ANYBOOT_POWER_DRY_RUN=1` 时只打印不执行。
///
/// 本机验证电源动作不能真的把机器关掉，这个开关让「三端命令表 + 置灰 + 回执」
/// 全部可以反复自动验证。
fn power_dry_run() -> bool {
    env_flag_from(std::env::var("ANYBOOT_POWER_DRY_RUN").ok().as_deref()).unwrap_or(false)
}

/// 只渲染「将会执行」的命令清单（dry-run 与日志共用，不执行任何东西）。
fn plan_commands(attempts: &[PowerAttempt], allow_gui_auth: bool) -> Vec<String> {
    let mut plan = Vec::new();
    for candidate in attempts {
        if candidate.needs_gui_auth && !allow_gui_auth {
            plan.push(format!(
                "{}（需要本机授权，远程/CLI 会跳过）",
                candidate.command()
            ));
            continue;
        }
        plan.push(candidate.command());
        if let Some(escalated) = escalate_power(candidate, allow_gui_auth) {
            plan.push(format!(
                "{}（仅当上一条因权限失败时才用）",
                escalated.command()
            ));
        }
    }
    plan
}

/// 执行电源动作。
///
/// dry-run 时只回传命令清单；真跑时按候选表顺序尝试，沿用既有安全约定——
/// **只有「命令不存在」才换下一条候选**，命令能跑但失败时要么提权重试、要么停手。
fn run_power_action(
    action: PowerAction,
    allow_gui_auth: bool,
    dry_run: bool,
) -> Result<PowerReport, String> {
    let attempts = power_attempts(action);
    if dry_run {
        return Ok(PowerReport {
            dry_run: true,
            commands: plan_commands(&attempts, allow_gui_auth),
        });
    }

    let mut failures = Vec::new();
    let mut commands = Vec::new();
    for candidate in attempts {
        if candidate.needs_gui_auth && !allow_gui_auth {
            failures.push(format!("{}（需要本机授权，已跳过）", candidate.command()));
            continue;
        }
        let outcome = run_power_command(&candidate.program, &candidate.args);
        commands.push(candidate.command());
        let escalation = escalate_power(&candidate, allow_gui_auth);
        match next_power_step(&outcome, escalation.is_some()) {
            PowerStep::Done => {
                return Ok(PowerReport {
                    dry_run: false,
                    commands,
                })
            }
            PowerStep::Next => {
                failures.push(outcome.describe());
                continue;
            }
            PowerStep::Escalate => {
                let Some(escalated) = escalation else {
                    return Err(format!("{}失败：{}", action.label(), outcome.describe()));
                };
                let escalated_outcome = run_power_command(&escalated.program, &escalated.args);
                commands.push(escalated.command());
                return match escalated_outcome {
                    PowerAttemptOutcome::Success => Ok(PowerReport {
                        dry_run: false,
                        commands,
                    }),
                    other => Err(format!(
                        "{}需要管理员授权，提权未完成：{}",
                        action.label(),
                        other.describe()
                    )),
                };
            }
            PowerStep::Stop => {
                return Err(format!(
                    "{}失败：{}（可能是没有权限或取消了授权）",
                    action.label(),
                    outcome.describe()
                ))
            }
        }
    }
    Err(format!(
        "没有可用的{}命令（已尝试：{}）",
        action.label(),
        failures.join("；")
    ))
}

/// Windows 休眠是否启用（Q10′ 方案 C 的只读预检结论）。
#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HibernationState {
    /// 休眠已启用 → Windows 会把「睡眠」变成休眠
    Enabled,
    /// 休眠已关闭 → 「睡眠」会真的进入睡眠（S3）
    Disabled,
    /// 读不到也说不出，按未知处理（不猜）
    Unknown,
}

/// 纯函数：从 `powercfg /a` 的原文判断休眠是否启用（中英文都认）。
#[cfg(any(target_os = "windows", test))]
fn hibernation_from_powercfg_output(raw: &str) -> HibernationState {
    if raw.contains("尚未启用休眠") {
        return HibernationState::Disabled;
    }
    let lowered = raw.to_ascii_lowercase();
    if lowered.contains("hibernate") {
        if lowered.contains("not available") || lowered.contains("not enabled") {
            return HibernationState::Disabled;
        }
        return HibernationState::Enabled;
    }
    if raw.contains("休眠") {
        // 只提到「休眠」但没说「尚未启用」：保守按「会休眠」提示
        return HibernationState::Enabled;
    }
    HibernationState::Unknown
}

/// 读注册表 `HKLM\SYSTEM\...\Control\Power\HibernateEnabled`（语言无关）。
#[cfg(target_os = "windows")]
fn read_hibernate_enabled_registry() -> Option<bool> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\Power")
        .ok()?;
    let value: u32 = key.get_value("HibernateEnabled").ok()?;
    Some(value != 0)
}

/// 睡眠之前做一次只读预检，返回「给用户看的一句话」+「证据（写进 trace）」。
fn sleep_probe() -> (Option<String>, Vec<String>) {
    #[cfg(target_os = "windows")]
    {
        let mut evidence = Vec::new();
        let state = match read_hibernate_enabled_registry() {
            Some(enabled) => {
                evidence.push(format!("注册表 HibernateEnabled={}", u8::from(enabled)));
                if enabled {
                    HibernationState::Enabled
                } else {
                    HibernationState::Disabled
                }
            }
            None => {
                evidence.push("注册表 HibernateEnabled 读取失败，改用 powercfg /a".to_string());
                match Command::new("powercfg").arg("/a").output() {
                    Ok(output) => {
                        let raw = String::from_utf8_lossy(&output.stdout).to_string();
                        evidence.push(format!(
                            "powercfg /a 原文: {}",
                            raw.replace(['\r', '\n'], " ").trim()
                        ));
                        hibernation_from_powercfg_output(&raw)
                    }
                    Err(error) => {
                        evidence.push(format!("powercfg /a 无法执行: {error}"));
                        HibernationState::Unknown
                    }
                }
            }
        };
        let note = match state {
            HibernationState::Enabled => {
                "注意：系统启用了休眠，Windows 会把这次「睡眠」变成休眠（要真睡眠先执行 powercfg /hibernate off）"
                    .to_string()
            }
            HibernationState::Disabled => "本次会进入睡眠（S3）".to_string(),
            HibernationState::Unknown => {
                "未能判定这次是睡眠还是休眠，见 trace 里 powercfg /a 的原文".to_string()
            }
        };
        (Some(note), evidence)
    }
    #[cfg(not(target_os = "windows"))]
    {
        (None, Vec::new())
    }
}

/// 后台执行电源动作，结果写进快照推给主窗口。
///
/// 提权弹窗可能停很久，所以不能在命令线程里同步跑；重复请求由 busy.power 挡掉
/// （托盘项和主窗口按钮会按同一份 busy 同时置灰）。
///
/// `allow_gui_auth`：本机点击传 true（可以弹系统授权框）；阶段 3 的 HTTP 与
/// 阶段 5 的 CLI 传 false（Q3：远程不弹本机授权框，直接说明原因）。
fn spawn_power(app: &AppHandle, action: PowerAction, allow_gui_auth: bool) {
    if !try_begin_power(app, action.as_str()) {
        return;
    }
    trace(
        app,
        "power",
        &format!(
            "开始执行 {}（dry_run={} allow_gui_auth={}）",
            action.as_str(),
            power_dry_run(),
            allow_gui_auth
        ),
    );
    // 睡眠前做只读预检：Windows 上判断这次是真睡眠（S3）还是会变成休眠（Q10′ 方案 C）
    let (note, evidence) = if action == PowerAction::Sleep {
        sleep_probe()
    } else {
        (None, Vec::new())
    };
    for line in &evidence {
        trace(app, "power", line);
    }

    let dry_run = power_dry_run();
    let app = app.clone();
    std::thread::spawn(
        move || match run_power_action(action, allow_gui_auth, dry_run) {
            Ok(report) => {
                let message = if report.dry_run {
                    format!(
                        "dry-run：未真的执行，将按顺序尝试 {}",
                        report.commands.join(" → ")
                    )
                } else if let Some(note) = &note {
                    format!("{}（{note}）", action.submitted_message())
                } else {
                    action.submitted_message().to_string()
                };
                trace(
                    &app,
                    "power",
                    &format!(
                        "{} 成功 dry_run={} commands={:?}",
                        action.as_str(),
                        report.dry_run,
                        report.commands
                    ),
                );
                record_action(&app, action.label(), true, &message);
                if report.dry_run {
                    // dry-run 没真的执行：立刻解除 busy，方便继续验证
                    finish_power(&app);
                } else {
                    // 成功时系统马上就没了：保留 busy，避免用户再点一次。
                    publish_ui(&app);
                }
            }
            Err(error) => {
                trace(&app, "power", &format!("{} 失败: {error}", action.as_str()));
                record_action(&app, action.label(), false, &error);
                // 失败时解除 busy，允许再试一次（finish_power 内部会发布快照）
                finish_power(&app);
                // 从托盘触发时主窗口可能没打开，显示出来让用户看到失败原因。
                // 窗口/激活策略属于 AppKit 主线程操作，这里显式切回主线程。
                let window_app = app.clone();
                if let Err(error) = app.run_on_main_thread(move || show_main_window(&window_app)) {
                    trace(&app, "power", &format!("显示主窗口失败: {error}"));
                }
            }
        },
    );
}

fn apply_dock_taskbar_visibility(app: &AppHandle, hide: bool) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        window
            .set_skip_taskbar(hide)
            .map_err(|error| format!("无法更新任务栏显示状态: {error}"))?;
    }
    #[cfg(target_os = "macos")]
    let _macos_policy_updated = app
        .set_activation_policy(if hide {
            tauri::ActivationPolicy::Accessory
        } else {
            tauri::ActivationPolicy::Regular
        })
        .map_err(|error| format!("无法更新 Dock 显示状态: {error}"))?;
    Ok(())
}

#[tauri::command]
fn current_status(app: AppHandle) -> AppStatus {
    state_snapshot(&app)
}

#[tauri::command]
fn refresh(app: AppHandle) -> AppStatus {
    trace(&app, "command", "refresh（主窗口）");
    refresh_and_update(&app)
}

#[tauri::command]
fn set_vars_dir(app: AppHandle, path: Option<String>) -> AppStatus {
    trace(&app, "command", &format!("set_vars_dir path={:?}", path));
    let directory = path
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from);
    {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state.explicit_dir = directory;
    }
    refresh_and_update(&app)
}

#[tauri::command]
fn switch_system(app: AppHandle, system: String) -> AppStatus {
    trace(
        &app,
        "command",
        &format!("switch_system system={system}（主窗口）"),
    );
    switch_and_update(&app, &system)
}

#[tauri::command]
fn get_settings(app: AppHandle) -> SettingsView {
    view_settings(&app)
}

/// 主窗口首次加载用：拿一份完整快照（含 revision）。
#[tauri::command]
fn get_ui_state(app: AppHandle) -> UiState {
    trace(&app, "command", "get_ui_state（主窗口）");
    ui_state(&app)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_autostart enabled={enabled}（主窗口）"),
    );
    match autostart_set(&app, enabled) {
        Ok(state) => {
            let mut settings = stored_settings(&app);
            settings.autostart = state.is_on();
            save_settings(&app, &settings)?;
            record_action(
                &app,
                "开机启动",
                true,
                if state.is_on() {
                    "已开启开机启动"
                } else {
                    "已关闭开机启动"
                },
            );
            Ok(publish_settings(&app))
        }
        Err(error) => {
            // 失败也要发布一次：界面必须显示系统里的真实状态和提示
            record_action(&app, "开机启动", false, &error);
            publish_settings(&app);
            Err(error)
        }
    }
}

#[tauri::command]
fn open_login_items_settings(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = &app;
        login_item::open_system_settings()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &app;
        Err("仅 macOS 支持打开登录项设置".to_string())
    }
}

#[tauri::command]
fn set_close_action(app: AppHandle, action: String) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_close_action action={action}（主窗口）"),
    );
    let Some(close_action) = CloseAction::parse(action.trim()) else {
        return Err(format!("未知的关闭行为: {action}"));
    };
    let mut settings = stored_settings(&app);
    settings.close_action = close_action;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

#[tauri::command]
fn set_start_in_tray(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_start_in_tray enabled={enabled}（主窗口）"),
    );
    let mut settings = stored_settings(&app);
    settings.start_in_tray = enabled;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

#[tauri::command]
fn set_hide_from_dock_taskbar(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_hide_from_dock_taskbar enabled={enabled}（主窗口）"),
    );
    let previous = stored_settings(&app);
    let mut settings = previous.clone();
    settings.hide_from_dock_taskbar = enabled;
    save_settings(&app, &settings)?;
    if let Err(error) = apply_dock_taskbar_visibility(&app, enabled) {
        // 回滚（阶段 1 修 D5）：不能让设置文件显示“已生效”而系统没变
        let _ = save_settings(&app, &previous);
        record_action(
            &app,
            "隐藏程序（任务栏/Dock）",
            false,
            &format!("{error}（已回滚为原值）"),
        );
        publish_settings(&app);
        return Err(error);
    }
    record_action(
        &app,
        "隐藏程序（任务栏/Dock）",
        true,
        if enabled {
            "已设置：不在任务栏/Dock 显示"
        } else {
            "已设置：在任务栏/Dock 显示"
        },
    );
    Ok(publish_settings(&app))
}

/// 重启系统（兼容旧前端调用）。提权弹窗可能等很久，所以立刻返回，结果通过事件推送。
#[tauri::command]
fn restart_system(app: AppHandle) {
    trace(&app, "command", "restart_system（主窗口）");
    spawn_power(&app, PowerAction::Reboot, true);
}

/// 执行电源动作：`reboot`（缺省）/ `shutdown` / `sleep`。
#[tauri::command]
fn set_http_enabled(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_http_enabled enabled={enabled}（主窗口）"),
    );
    update_settings_and_publish(
        &app,
        "HTTP 服务",
        if enabled { "已开启" } else { "已关闭" },
        |settings| settings.http_enabled = enabled,
        apply_http_config,
    )
}

#[tauri::command]
fn set_http_bind(app: AppHandle, bind: String) -> Result<SettingsView, String> {
    let bind = bind.trim().to_string();
    if bind.is_empty() {
        return Err("监听地址不能为空（用 0.0.0.0 或 127.0.0.1）".to_string());
    }
    trace(
        &app,
        "command",
        &format!("set_http_bind bind={bind}（主窗口）"),
    );
    update_settings_and_publish(
        &app,
        "监听地址",
        &format!("已设为 {bind}"),
        |settings| settings.http_bind = bind,
        apply_http_config,
    )
}

#[tauri::command]
fn set_http_port(app: AppHandle, port: u16) -> Result<SettingsView, String> {
    if port == 0 {
        return Err("端口不能用 0".to_string());
    }
    trace(
        &app,
        "command",
        &format!("set_http_port port={port}（主窗口）"),
    );
    update_settings_and_publish(
        &app,
        "端口",
        &format!("已设为 {port}"),
        |settings| settings.http_port = port,
        apply_http_config,
    )
}

#[tauri::command]
fn set_remote_allow_gui_auth(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_remote_allow_gui_auth enabled={enabled}（主窗口）"),
    );
    update_settings_and_publish(
        &app,
        "远程触发本机授权框",
        if enabled {
            "已允许（远程可能弹本机授权框，需要有人在机器前点确认）"
        } else {
            "已禁止（远程遇到需授权的操作会直接说明原因）"
        },
        |settings| settings.remote_allow_gui_auth = enabled,
        |_app| {},
    )
}

#[tauri::command]
fn set_notify_enabled(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_notify_enabled enabled={enabled}（主窗口）"),
    );
    update_settings_and_publish(
        &app,
        "远程请求通知",
        if enabled {
            "已开启"
        } else {
            "已关闭（有远程请求时只在主窗口显示，不再弹窗）"
        },
        |settings| settings.notify_enabled = enabled,
        |_app| {},
    )
}

#[tauri::command]
fn set_channel_notify(
    app: AppHandle,
    channel: String,
    notify: bool,
) -> Result<SettingsView, String> {
    let Some(channel) = Channel::parse(&channel) else {
        return Err(format!("未知渠道: {channel}（http / cli）"));
    };
    trace(
        &app,
        "command",
        &format!(
            "set_channel_notify channel={} notify={notify}（主窗口）",
            channel.as_str()
        ),
    );
    update_settings_and_publish(
        &app,
        "渠道通知",
        &format!(
            "{} 渠道：{}",
            channel.label(),
            if notify {
                "已开启弹窗"
            } else {
                "已关闭弹窗"
            }
        ),
        |settings| settings.channels.for_channel_mut(channel).notify = notify,
        |_app| {},
    )
}

#[tauri::command]
fn set_channel_delay(
    app: AppHandle,
    channel: String,
    seconds: u32,
) -> Result<SettingsView, String> {
    let Some(channel) = Channel::parse(&channel) else {
        return Err(format!("未知渠道: {channel}（http / cli）"));
    };
    let seconds = seconds.min(MAX_DELAY_SECONDS);
    trace(
        &app,
        "command",
        &format!(
            "set_channel_delay channel={} seconds={seconds}（主窗口）",
            channel.as_str()
        ),
    );
    update_settings_and_publish(
        &app,
        "渠道延迟",
        &format!(
            "{} 渠道：{}",
            channel.label(),
            if seconds == 0 {
                "立即执行".to_string()
            } else {
                format!("{seconds} 秒后执行（期间可取消）")
            }
        ),
        |settings| settings.channels.for_channel_mut(channel).delay_seconds = seconds,
        |_app| {},
    )
}

/// 重新生成 token：旧 token 立即失效（服务会按新 token 重建）。
#[tauri::command]
fn regenerate_http_token(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "regenerate_http_token（主窗口）");
    let token = generate_token();
    let replace = token.clone();
    update_settings_and_publish(
        &app,
        "访问令牌",
        "已重新生成（旧 token 立即失效）",
        move |settings| settings.http_token = replace,
        apply_http_config,
    )?;
    Ok(token)
}

/// 明文 token：只由用户点「显示」时取，快照里始终是打码版本。
#[tauri::command]
fn reveal_http_token(app: AppHandle) -> String {
    trace(&app, "command", "reveal_http_token（主窗口）");
    stored_settings(&app).http_token
}

/// 可直接复制的调用示例（含本机 IP 与 token）。
#[tauri::command]
fn http_access_hint(app: AppHandle) -> String {
    trace(&app, "command", "http_access_hint（主窗口）");
    http_access_hint_text(&app)
}

/// 设置提示音开关
#[tauri::command]
fn set_play_sound(app: AppHandle, enabled: bool) -> Result<(), String> {
    trace(&app, "command", "set_play_sound（主窗口）");
    update_settings_and_publish(
        &app,
        "提示音",
        if enabled { "已开启" } else { "已关闭" },
        |settings| settings.play_sound = enabled,
        |_app| {},
    )?;
    Ok(())
}

/// 设置自定义提示音文件路径（留空则用系统默认）
#[tauri::command]
fn set_sound_file_path(app: AppHandle, path: String) -> Result<(), String> {
    trace(&app, "command", "set_sound_file_path（主窗口）");
    let path = path.trim().to_string();
    let message = if path.is_empty() {
        "已清除".to_string()
    } else {
        format!("已设置：{path}")
    };
    update_settings_and_publish(
        &app,
        "自定义提示音",
        &message,
        |settings| settings.sound_file_path = path,
        |_app| {},
    )?;
    Ok(())
}

/// 设置访问令牌（自定义 token）：改了要重启监听，新 token 立即生效。
#[tauri::command]
fn set_http_token(app: AppHandle, token: String) -> Result<(), String> {
    trace(&app, "command", "set_http_token（主窗口）");
    let token = validate_http_token(&token)?;
    let message = if token.is_empty() {
        "已清空：token 为空时内置 HTTP 服务不会启动".to_string()
    } else {
        "已更新，监听已按新 token 重启".to_string()
    };
    update_settings_and_publish(
        &app,
        "访问令牌",
        &message,
        |settings| settings.http_token = token,
        apply_http_config,
    )?;
    Ok(())
}

/// 设置界面主题（dark / light / system）
#[tauri::command]
fn set_theme(app: AppHandle, theme: String) -> Result<(), String> {
    trace(&app, "command", "set_theme（主窗口）");
    let theme = normalize_theme(&theme);
    update_settings_and_publish(
        &app,
        "界面主题",
        &format!("已切换为 {theme}"),
        |settings| settings.theme = theme,
        |_app| {},
    )?;
    Ok(())
}

/// 设置窗口不透明度（百分比，50–100）
#[tauri::command]
fn set_opacity(app: AppHandle, percent: u32) -> Result<(), String> {
    trace(&app, "command", "set_opacity（主窗口）");
    let percent = clamp_opacity(percent);
    update_settings_and_publish(
        &app,
        "窗口不透明度",
        &format!("已设为 {percent}%"),
        |settings| settings.opacity_percent = percent,
        |_app| {},
    )?;
    Ok(())
}

/// 用系统文件管理器打开配置目录（含 settings.json / trace.log）
#[tauri::command]
fn open_config_dir(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "open_config_dir（主窗口）");
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("无法定位配置目录：{error}"))?;
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|error| format!("创建配置目录失败：{error}"))?;
    }
    open_directory_in_explorer(&dir)?;
    Ok(dir.display().to_string())
}

/// 用系统文件管理器打开当前变量目录（rEFInd 变量文件所在目录）
#[tauri::command]
fn open_vars_dir(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "open_vars_dir（主窗口）");
    let state = app.state::<Mutex<AppState>>();
    let app_state = state.lock().expect("AppState mutex poisoned");
    let vars_dir = app_state
        .vars_dir
        .clone()
        .ok_or_else(|| "尚未扫描到变量目录，请先点击「扫描」")?;
    open_directory_in_explorer(&vars_dir)?;
    Ok(vars_dir.display().to_string())
}

/// 重试监听（用户点「重试监听」按钮，或端口刚被释放后想再试一次）。
#[tauri::command]
fn retry_http(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "retry_http（主窗口）");
    apply_http_config(&app);
    let runtime = http_lock();
    if runtime.server.is_some() {
        Ok(format!(
            "已监听 {}",
            runtime
                .server
                .as_ref()
                .map(|s| s.addr.clone())
                .unwrap_or_default()
        ))
    } else {
        Err(runtime
            .last_error
            .clone()
            .unwrap_or_else(|| "仍然没能监听，请检查端口与 token".to_string()))
    }
}

/// 结束占用端口的进程（**只有用户显式点按钮才会走到这里**）。
///
/// 安全约定：不自动调用、不在勾选流程里调用；占用者是当前进程时直接拒绝。
#[tauri::command]
fn kill_port_occupant(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "kill_port_occupant（用户显式确认）");
    let occupant = http_lock()
        .occupant
        .clone()
        .ok_or_else(|| "当前没有检测到端口占用".to_string())?;

    if occupant.is_self {
        return Err(format!(
            "占着端口的就是本应用当前进程（PID {}），不能结束自己；请改用其他端口",
            occupant.pid
        ));
    }

    let name = if occupant.name.is_empty() {
        "未知".to_string()
    } else {
        occupant.name.clone()
    };
    trace(
        &app,
        "http",
        &format!(
            "用户确认结束占用进程 pid={} name={name} same_app={}",
            occupant.pid, occupant.same_app
        ),
    );

    kill_process(occupant.pid)?;
    record_action(
        &app,
        "结束占用进程",
        true,
        &format!("已结束 PID {}（{name}）", occupant.pid),
    );
    // 杀完立刻重试监听：用户点一下就该好
    apply_http_config(&app);
    let listening = http_lock()
        .server
        .as_ref()
        .map(|server| server.addr.clone());
    Ok(match listening {
        Some(addr) => format!("已结束 PID {}（{name}），服务已监听 {addr}", occupant.pid),
        None => format!(
            "已结束 PID {}（{name}），但端口仍未监听成功（可能有别的进程又占上了）",
            occupant.pid
        ),
    })
}

/// 跨平台：用系统文件管理器打开目录
fn open_directory_in_explorer(dir: &std::path::Path) -> Result<(), String> {
    let mut command = std::process::Command::new(if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    command.arg(dir);
    let status = command
        .status()
        .map_err(|error| format!("打开文件管理器失败：{error}"))?;
    if !status.success() {
        return Err(format!("打开文件管理器失败（exit code {status}）"));
    }
    Ok(())
}

#[tauri::command]
fn confirm_pending(app: AppHandle) -> Result<(), String> {
    trace(&app, "command", "confirm_pending（本机立即执行）");
    confirm_pending_now(&app)
}

#[tauri::command]
fn cancel_pending(app: AppHandle) -> Result<(), String> {
    trace(&app, "command", "cancel_pending（本机取消）");
    cancel_pending_now(&app)
}

#[tauri::command]
fn power_action(app: AppHandle, action: Option<String>) -> Result<(), String> {
    let parsed = match action.as_deref().map(str::trim) {
        None | Some("") => PowerAction::default(),
        Some(value) => PowerAction::parse(value)
            .ok_or_else(|| format!("未知的电源动作: {value}（可用 reboot/shutdown/sleep）"))?,
    };
    trace(
        &app,
        "command",
        &format!("power_action {}（主窗口）", parsed.as_str()),
    );
    spawn_power(&app, parsed, true);
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(Mutex::<AppState>::default())
        .manage(Mutex::<Settings>::default())
        // busy / 最近动作 / 菜单内容指纹：都只由 publish_ui 与托盘动作读写
        .manage(Mutex::<BusyState>::default())
        .manage(Mutex::<Option<ActionResult>>::default())
        .manage(Mutex::<Option<String>>::default())
        // macOS 13+ 走系统登录项（见 login_item），这里的 LaunchAgent 方式仅作为
        // 旧版 macOS 以及 Windows / Linux 的实现。
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .on_window_event(|window, event| {
            // 主窗口失焦就取消置顶：呼出时置顶是为了「一叫就在最上面」，
            // 但别让它一直压着别的窗口（用户点别处时应该让位）。
            if let tauri::WindowEvent::Focused(false) = event {
                if window.label() == MAIN_WINDOW_LABEL {
                    let _ = window.set_always_on_top(false);
                }
                return;
            }
            let tauri::WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            // 通知弹窗：关掉窗口 = 取消这次待执行请求（安全默认，误关不会把机器关掉）
            if window.label() == NOTIFY_WINDOW_LABEL {
                // 只有「槽位里确实还有待执行请求」才算用户取消；
                // 执行完成/到点自动关窗时槽位已经空了，那不是用户操作，不必记一条。
                if pending_snapshot().is_some() {
                    let app = window.app_handle().clone();
                    if let Err(error) = cancel_pending_now(&app) {
                        trace(&app, "notify", &format!("关闭弹窗时没能取消: {error}"));
                    }
                }
                return;
            }
            if window.label() != MAIN_WINDOW_LABEL {
                return;
            }
            let app = window.app_handle().clone();
            api.prevent_close();
            if stored_settings(&app).close_action == CloseAction::Quit {
                app.exit(0);
                return;
            }
            let _ = window.hide();
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
        })
        .setup(|app| {
            let settings = read_settings_file(app.handle());
            {
                let state = app.state::<Mutex<Settings>>();
                *state.lock().expect("settings mutex poisoned") = settings;
            }
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            startup_autostart_sync(app.handle());
            view_settings(app.handle());
            // 调试/沙箱：把变量目录指到别处，验证时不会碰真实的 ESP
            if let Ok(directory) = std::env::var("ANYBOOT_VARS_DIR") {
                let directory = directory.trim().to_string();
                if !directory.is_empty() {
                    let state = app.state::<Mutex<AppState>>();
                    let mut state = state.lock().expect("state mutex poisoned");
                    state.explicit_dir = Some(PathBuf::from(directory.clone()));
                    drop(state);
                    trace(
                        app.handle(),
                        "scan",
                        &format!("变量目录由环境变量指定: {directory}"),
                    );
                }
            }
            // 首次运行/升级：写回完整设置（含自动生成的 token），默认值一眼可见
            ensure_settings_complete(app.handle());
            let hide_from_dock_taskbar = stored_settings(app.handle()).hide_from_dock_taskbar;
            let start_in_tray = stored_settings(app.handle()).start_in_tray;
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(if hide_from_dock_taskbar || start_in_tray {
                tauri::ActivationPolicy::Accessory
            } else {
                tauri::ActivationPolicy::Regular
            });
            apply_dock_taskbar_visibility(app.handle(), hide_from_dock_taskbar)
                .map_err(|error| error.to_string())?;
            if start_in_tray {
                if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                    let _ = window.hide();
                }
            } else {
                show_main_window(app.handle());
            }

            // 第一帧就用完整菜单（系统项 / 应用设置 / 电源项都在），
            // 不再让用户在冷启动扫描期间看到一个残缺菜单（阶段 1 修 D2）。
            {
                // 状态行先占位，和以前写死的「正在扫描…」一致；随后的首次扫描会覆盖它
                let state = app.state::<Mutex<AppState>>();
                let mut state = state.lock().expect("state mutex poisoned");
                state.message = "正在扫描…".to_string();
            }
            let initial_state = ui_state(app.handle());
            let menu = build_tray_menu(app.handle(), &initial_state)?;
            // 预热菜单指纹：这一帧就是当前内容，别让紧接着的首次扫描白重建一次菜单
            remember_menu_signature(app.handle(), menu_signature_for(&initial_state));

            let _tray = TrayIconBuilder::with_id("main")
                .icon(create_tray_image())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        show_main_window(app);
                    }
                })
                .on_menu_event(|app, event| {
                    handle_menu_event(app, event.id().as_ref());
                })
                .build(app.handle())?;

            // HTTP 服务与待执行请求的调度线程（托盘已就绪，publish_ui 能刷菜单）
            apply_http_config(app.handle());
            spawn_notify_scheduler(app.handle());

            let app_handle = app.handle().clone();
            std::thread::spawn(move || refresh_and_update(&app_handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_ui_state,
            current_status,
            refresh,
            set_vars_dir,
            switch_system,
            unmount_temporary,
            get_settings,
            set_autostart,
            open_login_items_settings,
            set_close_action,
            set_start_in_tray,
            set_hide_from_dock_taskbar,
            restart_system,
            power_action,
            set_http_enabled,
            set_http_bind,
            set_http_port,
            set_remote_allow_gui_auth,
            set_notify_enabled,
            set_channel_notify,
            set_channel_delay,
            regenerate_http_token,
            reveal_http_token,
            http_access_hint,
            set_play_sound,
            set_sound_file_path,
            set_http_token,
            set_theme,
            set_opacity,
            open_config_dir,
            open_vars_dir,
            retry_http,
            kill_port_occupant,
            confirm_pending,
            cancel_pending
        ])
        .run(tauri::generate_context!())
        .expect("failed to run AnyBoot");
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn macos_login_item_backend_requires_signature() {
        // 未签名：系统登录项接口只会返回 “Operation not permitted”，必须走兼容方式。
        assert_eq!(
            autostart_backend_for(true, true, false),
            AutostartBackend::LaunchAgent
        );
        // 未以 .app 形式运行（例如 tauri dev）：同样不能用系统登录项接口。
        assert_eq!(
            autostart_backend_for(true, false, true),
            AutostartBackend::LaunchAgent
        );
        // macOS 12 及以下没有 SMAppService。
        assert_eq!(
            autostart_backend_for(false, true, true),
            AutostartBackend::LaunchAgent
        );
        // 已签名的 .app + macOS 13+ 才用系统登录项。
        assert_eq!(
            autostart_backend_for(true, true, true),
            AutostartBackend::LoginItem
        );
    }

    #[test]
    fn macos_autostart_hint_explains_compatibility_mode() {
        let compatibility = "当前应用没有代码签名".to_string();

        // 兼容方式生效且已开启：提示说明为什么没用系统登录项。
        assert_eq!(
            macos_autostart_hint(
                AutostartState::Enabled,
                AutostartBackend::LaunchAgent,
                None,
                compatibility.clone()
            ),
            Some(compatibility.clone())
        );
        // 走系统登录项且正常：不需要额外提示。
        assert_eq!(
            macos_autostart_hint(
                AutostartState::Enabled,
                AutostartBackend::LoginItem,
                None,
                compatibility.clone()
            ),
            None
        );
        // 没开启时不唠叨兼容方式。
        assert_eq!(
            macos_autostart_hint(
                AutostartState::Disabled,
                AutostartBackend::LaunchAgent,
                None,
                compatibility.clone()
            ),
            None
        );
        // 需要用户在系统设置里批准：优先给可操作的提示。
        assert_eq!(
            macos_autostart_hint(
                AutostartState::RequiresApproval,
                AutostartBackend::LoginItem,
                None,
                compatibility.clone()
            ),
            AutostartState::RequiresApproval.hint().map(str::to_string)
        );
        // 注册失败后退回兼容方式：把失败原因带出来。
        let hint = macos_autostart_hint(
            AutostartState::Enabled,
            AutostartBackend::LaunchAgent,
            Some("Operation not permitted (code=1)".to_string()),
            compatibility,
        )
        .expect("应有提示");
        assert!(hint.contains("Operation not permitted"));
        assert!(hint.contains("兼容方式"));
    }

    #[test]
    fn macos_dev_binary_never_uses_login_item_api() {
        // 单测二进制不在 .app 内，也没有签名，必须判为兼容方式；
        // 否则（旧行为）会去调用系统登录项接口并拿到 Operation not permitted。
        assert!(!running_from_app_bundle());
        assert!(!app_bundle_is_signed());
        assert_eq!(autostart_backend(), AutostartBackend::LaunchAgent);

        // 开启状态下界面会说明当前用的是兼容方式。
        let hint = autostart_hint(AutostartState::Enabled).expect("应有兼容方式提示");
        assert!(hint.contains("兼容方式"), "实际提示: {hint}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_power_table_uses_administrator_privileges() {
        let reboot = power_attempts(PowerAction::Reboot);
        assert_eq!(reboot.len(), 1, "macOS 重启只走一条提权路径");
        assert_eq!(reboot[0].program, "osascript");
        assert!(reboot[0].needs_gui_auth);
        let script = reboot[0].args.join(" ");
        assert!(
            script.contains("administrator privileges"),
            "实际脚本: {script}"
        );
        assert!(
            script.contains("/sbin/shutdown -r now"),
            "实际脚本: {script}"
        );
        // 候选命令本身就带管理员授权，不需要再提权一层。
        assert!(escalate_power(&reboot[0], true).is_none());

        // 关机同理
        let shutdown = power_attempts(PowerAction::Shutdown);
        assert!(shutdown[0].args.join(" ").contains("/sbin/shutdown -h now"));

        // 睡眠：先走免 root 的 System Events，再试免密 sudo，最后才是管理员授权兜底
        let sleep = power_attempts(PowerAction::Sleep);
        assert_eq!(sleep.len(), 3);
        assert!(
            !sleep[0].needs_gui_auth && sleep[0].args.join(" ").contains("System Events"),
            "第一条不该弹管理员授权框"
        );
        assert_eq!(sleep[1].program, "sudo");
        assert!(!sleep[1].needs_gui_auth, "sudo -n 不会弹密码框");
        assert!(sleep[2].needs_gui_auth, "最后一条才是管理员授权兜底");
    }

    #[test]
    fn power_command_runner_classifies_results() {
        // 真的去执行进程，验证成功/找不到命令/失败三种判定和 stderr 捕获。
        assert_eq!(
            run_power_command("/usr/bin/true", &[]),
            PowerAttemptOutcome::Success
        );
        assert!(matches!(
            run_power_command("/nonexistent-power-binary", &[]),
            PowerAttemptOutcome::Missing(_)
        ));

        // 模拟“用户取消授权”：退出码非零 + stderr 说明，必须归类为失败并带上原因。
        let outcome = run_power_command(
            "/bin/sh",
            &["-c", "echo 'Request dismissed' >&2; exit 126"]
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>(),
        );
        assert!(matches!(outcome, PowerAttemptOutcome::Failed(_)));
        let description = outcome.describe();
        assert!(description.contains("126"), "应带退出码: {description}");
        assert!(
            description.contains("Request dismissed"),
            "应带 stderr: {description}"
        );
    }
}

/// 与平台无关的核心单测：Windows / Linux / macOS 上 `cargo test` 都会跑。
///
/// 原 `tests` 模块整体挂在 `target_os = "macos"` 下，导致 Windows/Linux 上
/// `cargo test` 一条用例都不跑；阶段 0 起平台无关的用例挪到这里，
/// 「单测全绿」在三端才有实际含义。
#[cfg(test)]
mod core_tests {
    use super::*;

    #[test]
    fn custom_token_validation_accepts_good_and_rejects_sloppy_input() {
        // 合法：常规随机串、带符号、边界长度
        assert_eq!(validate_http_token("  abcd  ").unwrap(), "abcd");
        assert_eq!(
            validate_http_token("A-Za-z0-9_.~!*'()-").unwrap(),
            "A-Za-z0-9_.~!*'()-"
        );
        let long = "x".repeat(MAX_TOKEN_LEN);
        assert_eq!(validate_http_token(&long).unwrap().len(), MAX_TOKEN_LEN);

        // 空 = 清空（合法输入，服务会因此不启动）
        assert_eq!(validate_http_token("   ").unwrap(), "");

        // 太短 / 太长
        assert!(validate_http_token("abc").is_err());
        assert!(validate_http_token(&"x".repeat(MAX_TOKEN_LEN + 1)).is_err());

        // 粘贴时最容易混进来的东西：换行、空格、中文、制表符
        for bad in ["abcd efgh", "abcd\nefgh", "abcd\tefgh", "令牌令牌"] {
            let error = validate_http_token(bad).unwrap_err();
            assert!(
                error.contains("ASCII") || error.contains("长度"),
                "{bad} -> {error}"
            );
        }
    }

    #[test]
    fn theme_and_opacity_are_normalized_for_sloppy_input() {
        assert_eq!(normalize_theme("dark"), "dark");
        assert_eq!(normalize_theme("LIGHT "), "light");
        assert_eq!(normalize_theme("System"), "system");
        // 不认识的值一律回落 dark，不会把界面搞成没样式
        assert_eq!(normalize_theme(""), "dark");
        assert_eq!(normalize_theme("solarized"), "dark");

        assert_eq!(clamp_opacity(100), 100);
        assert_eq!(clamp_opacity(50), 50);
        assert_eq!(clamp_opacity(80), 80);
        // 越界夹取（防止透明度低到看不见界面、或超过 100 变成无效值）
        assert_eq!(clamp_opacity(0), MIN_OPACITY);
        assert_eq!(clamp_opacity(10), MIN_OPACITY);
        assert_eq!(clamp_opacity(999), MAX_OPACITY);
    }

    #[test]
    fn sound_players_are_ordered_per_platform() {
        let path = PathBuf::from("/tmp/notify.wav");
        let candidates = sound_player_candidates(&path);
        assert!(!candidates.is_empty());

        if cfg!(target_os = "windows") {
            assert_eq!(candidates[0].0, "powershell");
            // 命令行里必须带上文件路径与 PlaySync（同步播完就退出）
            let command = candidates[0].1.join(" ");
            assert!(command.contains("notify.wav") && command.contains("PlaySync"));
        } else if cfg!(target_os = "macos") {
            assert_eq!(candidates[0].0, "afplay");
            assert_eq!(candidates[0].1, vec!["/tmp/notify.wav".to_string()]);
        } else {
            // Linux：paplay 优先（PipeWire/PulseAudio 都兼容），再 aplay，最后 ffplay
            assert_eq!(
                candidates
                    .iter()
                    .map(|(p, _)| p.as_str())
                    .collect::<Vec<_>>(),
                vec!["paplay", "aplay", "ffplay"]
            );
            assert!(candidates[1].1.contains(&"-q".to_string()));
            assert!(candidates[2].1.contains(&"quiet".to_string()));
        }
    }

    #[test]
    fn utc_timestamp_formats_known_epochs() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(1_000_000_000), "2001-09-09T01:46:40Z");
        // 闰日，验证 civil_from_days 的边界
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_timestamp(1_759_995_153), "2025-10-09T07:32:33Z");
    }

    #[test]
    fn menu_signature_tracks_every_visible_field() {
        let settings = Settings::default();
        let variables = vec![VariableInfo {
            system: "linux".to_string(),
            display_name: "Linux".to_string(),
            source_name: "PreviousBoot-linux".to_string(),
            source: PathBuf::from("/mnt/EFI/refind/vars/PreviousBoot-linux"),
            exists: true,
            active: false,
        }];
        let idle = BusyState::default();
        let base = menu_signature(
            "下次启动：Windows",
            false,
            &settings,
            &variables,
            None,
            &idle,
        );
        // 同内容同签名
        assert_eq!(
            base,
            menu_signature(
                "下次启动：Windows",
                false,
                &settings,
                &variables,
                None,
                &idle
            )
        );
        // 状态文案 / 临时挂载 / 错误 / 设置 / 变量选中与存在 / busy，
        // 任一变化都必须换签名，否则「内容没变就不重建菜单」会漏掉真实变更。
        assert_ne!(
            base,
            menu_signature("已连接 rEFInd", false, &settings, &variables, None, &idle)
        );
        assert_ne!(
            base,
            menu_signature(
                "下次启动：Windows",
                true,
                &settings,
                &variables,
                None,
                &idle
            )
        );
        assert_ne!(
            base,
            menu_signature(
                "下次启动：Windows",
                false,
                &settings,
                &variables,
                Some("扫描失败"),
                &idle
            )
        );
        let mut toggled = settings.clone();
        toggled.autostart = true;
        assert_ne!(
            base,
            menu_signature(
                "下次启动：Windows",
                false,
                &toggled,
                &variables,
                None,
                &idle
            )
        );
        let mut active = variables.clone();
        active[0].active = true;
        assert_ne!(
            base,
            menu_signature("下次启动：Windows", false, &settings, &active, None, &idle)
        );
        let mut missing = variables.clone();
        missing[0].exists = false;
        assert_ne!(
            base,
            menu_signature("下次启动：Windows", false, &settings, &missing, None, &idle)
        );
        // 长动作进行中菜单项要变灰 → 必须进指纹，否则置灰不会生效
        let power = BusyState {
            power: Some("restart".to_string()),
            ..Default::default()
        };
        assert_ne!(
            base,
            menu_signature(
                "下次启动：Windows",
                false,
                &settings,
                &variables,
                None,
                &power
            )
        );
        // 扫描不进指纹：它太快，不值得为它把「每扫描一次重建一次菜单」变成两次
        let scanning = BusyState {
            scanning: true,
            ..Default::default()
        };
        assert_eq!(
            base,
            menu_signature(
                "下次启动：Windows",
                false,
                &settings,
                &variables,
                None,
                &scanning
            )
        );
    }

    #[test]
    fn settings_debug_trace_defaults_off_and_round_trips() {
        assert!(!Settings::default().debug_trace);
        // 老配置文件（没有 debugTrace 字段）必须还能反序列化，且开关为关
        let legacy: Settings =
            serde_json::from_str(r#"{"autostart":true}"#).expect("老配置应能反序列化");
        assert!(legacy.autostart);
        assert!(!legacy.debug_trace);
        let enabled: Settings =
            serde_json::from_str(r#"{"debugTrace":true}"#).expect("新字段应能反序列化");
        assert!(enabled.debug_trace);
        // 字段名是 camelCase（配置文件与前端都按这个读写）
        let json = serde_json::to_string(&Settings::default()).expect("应能序列化");
        assert!(json.contains("\"debugTrace\":false"), "实际: {json}");
        assert!(json.contains("\"closeAction\":\"tray\""), "实际: {json}");
    }

    #[test]
    fn env_flag_parses_common_forms() {
        // trace 与电源 dry-run 两个探针共用同一个解析器
        assert_eq!(env_flag_from(Some("1")), Some(true));
        assert_eq!(env_flag_from(Some("TRUE")), Some(true));
        assert_eq!(env_flag_from(Some(" on ")), Some(true));
        assert_eq!(env_flag_from(Some("0")), Some(false));
        assert_eq!(env_flag_from(Some("off")), Some(false));
        assert_eq!(env_flag_from(Some("")), None);
        assert_eq!(env_flag_from(Some("maybe")), None);
        assert_eq!(env_flag_from(None), None);
    }

    #[test]
    fn power_attempt_chain_stops_after_cancelled_authorization() {
        use PowerAttemptOutcome::{Failed, Missing, Success};

        // 命令执行成功：结束。
        assert_eq!(next_power_step(&Success, true), PowerStep::Done);
        // 系统里没有这个命令：换下一个候选（例如没有 systemd 的发行版）。
        assert_eq!(
            next_power_step(&Missing("没有 systemctl".to_string()), true),
            PowerStep::Next
        );
        // 权限不足且还有提权方案：用 pkexec 重试。
        assert_eq!(
            next_power_step(
                &Failed("Interactive authentication required".to_string()),
                true
            ),
            PowerStep::Escalate
        );
        // 提权也失败了（多半是用户取消授权）：停手，
        // 不能“取消授权之后换个方式照样把机器关掉/重启”。
        assert_eq!(
            next_power_step(&Failed("Request dismissed".to_string()), false),
            PowerStep::Stop
        );
    }

    #[test]
    fn frontend_uses_the_single_state_contract() {
        // 前后端通过事件名和命令名对接，任何一边改名了都会让联动失效，
        // 这里直接对着前端源码断言契约。
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/app.js");
        let Ok(source) = fs::read_to_string(&path) else {
            return;
        };
        assert!(
            source.contains(&format!("listen(\"{UI_STATE_CHANGED_EVENT}\"")),
            "前端没有监听 {UI_STATE_CHANGED_EVENT}"
        );
        // 命令名必须出现在前端源码里（调用方式不限：invoke 直接调或走统一封装）
        for command in [
            "get_ui_state",
            "refresh",
            "set_vars_dir",
            "switch_system",
            "unmount_temporary",
            "set_autostart",
            "set_start_in_tray",
            "set_hide_from_dock_taskbar",
            "set_close_action",
            "power_action",
        ] {
            assert!(
                source.contains(&format!("\"{command}\"")),
                "前端没有调用 {command} 命令"
            );
        }
        // 快照带版本号：前端必须按 revision 丢弃过期快照（阶段 1 修 D4 的契约）
        assert!(source.contains("revision"), "前端没有按 revision 处理快照");
        // 三个旧的补丁式事件不允许再出现
        for gone in ["status-changed", "settings-changed", "restart-result"] {
            assert!(!source.contains(gone), "前端仍在依赖已废弃的事件 {gone}");
        }
    }

    #[test]
    fn menu_action_from_id_round_trips_generated_ids() {
        // 每个由 build_tray_menu 生成的 id 都必须解析回对应语义，
        // 否则托盘上那一项点了等于没点（阶段 1 修掉的真实 bug）。
        for (id, expected) in [
            ("show".to_string(), MenuAction::Show),
            ("refresh".to_string(), MenuAction::Refresh),
            (
                "unmount-temporary".to_string(),
                MenuAction::UnmountTemporary,
            ),
            (
                power_menu_id(PowerAction::Reboot),
                MenuAction::Power(PowerAction::Reboot),
            ),
            (
                power_menu_id(PowerAction::Shutdown),
                MenuAction::Power(PowerAction::Shutdown),
            ),
            (
                power_menu_id(PowerAction::Sleep),
                MenuAction::Power(PowerAction::Sleep),
            ),
            // 旧版只有一个「重启系统」项时的 id，保留兼容
            (
                "restart".to_string(),
                MenuAction::Power(PowerAction::Reboot),
            ),
            ("setting-autostart".to_string(), MenuAction::SetAutostart),
            ("quit".to_string(), MenuAction::Quit),
            ("linux".to_string(), MenuAction::Switch("linux".to_string())),
            (
                "windows".to_string(),
                MenuAction::Switch("windows".to_string()),
            ),
            ("macos".to_string(), MenuAction::Switch("macos".to_string())),
            (
                toggle_menu_id("setting-start-in-tray", true),
                MenuAction::SetStartInTray(true),
            ),
            (
                toggle_menu_id("setting-start-in-tray", false),
                MenuAction::SetStartInTray(false),
            ),
            (
                toggle_menu_id("setting-hide-from-dock-taskbar", true),
                MenuAction::SetHideFromDockTaskbar(true),
            ),
            (
                toggle_menu_id("setting-hide-from-dock-taskbar", false),
                MenuAction::SetHideFromDockTaskbar(false),
            ),
            (
                close_action_menu_id(CloseAction::Tray),
                MenuAction::SetCloseAction(CloseAction::Tray),
            ),
            (
                close_action_menu_id(CloseAction::Quit),
                MenuAction::SetCloseAction(CloseAction::Quit),
            ),
        ] {
            assert_eq!(MenuAction::from_id(&id), Some(expected), "id={id}");
        }
        // 旧版那两条不带目标状态的 id 就是当年漏掉的写法：现在必须明确判为
        // 无法识别，而不是静默走进兜底分支（否则又会“点了没反应”）。
        assert_eq!(MenuAction::from_id("setting-start-in-tray"), None);
        assert_eq!(MenuAction::from_id("setting-hide-from-dock-taskbar"), None);
    }

    #[test]
    fn menu_action_rejects_unknown_ids() {
        for id in [
            "",
            "unmount",
            "STATUS",
            "linux:true",
            "setting-close-action:bogus",
            "setting-start-in-tray:maybe",
            "setting-hide-from-dock-taskbar:1",
            "setting-label",
            "power:",
            "power:hibernate",
            "power:sleep:true",
        ] {
            assert_eq!(MenuAction::from_id(id), None, "id={id}");
        }
    }

    #[test]
    fn power_action_parses_names_and_aliases() {
        assert_eq!(PowerAction::parse("reboot"), Some(PowerAction::Reboot));
        assert_eq!(PowerAction::parse("RESTART"), Some(PowerAction::Reboot));
        assert_eq!(
            PowerAction::parse(" Shutdown "),
            Some(PowerAction::Shutdown)
        );
        assert_eq!(PowerAction::parse("poweroff"), Some(PowerAction::Shutdown));
        assert_eq!(PowerAction::parse("sleep"), Some(PowerAction::Sleep));
        assert_eq!(PowerAction::parse("suspend"), Some(PowerAction::Sleep));
        // hibernation 不是我们要的动作（它和睡眠不是一回事）
        assert_eq!(PowerAction::parse("hibernate"), None);
        assert_eq!(PowerAction::parse(""), None);
        // 缺省 = 重启
        assert_eq!(PowerAction::default(), PowerAction::Reboot);
        assert_eq!(PowerAction::default().as_str(), "reboot");
        assert_eq!(PowerAction::Shutdown.label(), "关机");
        assert_eq!(PowerAction::Sleep.label(), "睡眠");
        assert!(PowerAction::Sleep.submitted_message().contains("睡眠"));
        // 菜单 id 与解析必须成对
        for action in [
            PowerAction::Reboot,
            PowerAction::Shutdown,
            PowerAction::Sleep,
        ] {
            assert_eq!(
                MenuAction::from_id(&power_menu_id(action)),
                Some(MenuAction::Power(action))
            );
        }
    }

    #[test]
    fn plan_commands_filters_gui_auth_attempts() {
        let local_attempt = attempt("systemctl", &["suspend"], false);
        let gui_attempt = attempt("osascript", &["-e", "admin"], true);
        let attempts = vec![local_attempt.clone(), gui_attempt.clone()];

        // 本机点击：需要授权的候选也在计划里
        let local = plan_commands(&attempts, true);
        assert!(local.iter().any(|line| line.contains("systemctl suspend")));
        assert!(local.iter().any(|line| line.contains("osascript")));

        // 远程 / CLI：需要本机授权的整条跳过，并且明确说出来
        let remote = plan_commands(&attempts, false);
        assert!(remote.iter().any(|line| line.contains("需要本机授权")));
        // 需要授权的候选只作为「已跳过」的说明出现，不能有可执行的那一行
        assert!(
            !remote
                .iter()
                .any(|line| line.contains("osascript") && !line.contains("需要本机授权")),
            "远程计划里不该出现可执行的 osascript 行: {remote:?}"
        );
        // 提权候选只在有方案时出现（Linux 才有 pkexec）
        let has_escalation = escalate_power(&local_attempt, true).is_some();
        assert_eq!(
            local.iter().filter(|line| line.contains("pkexec")).count(),
            usize::from(has_escalation)
        );
    }

    #[test]
    fn dry_run_never_executes_and_lists_the_table_commands() {
        // dry-run 是「本机验证电源动作」的关键：它必须一秒都不执行、只回传命令清单。
        // 因此这里跑的是真代码路径，不会真的关机/重启/睡眠。
        for action in [
            PowerAction::Reboot,
            PowerAction::Shutdown,
            PowerAction::Sleep,
        ] {
            let local = run_power_action(action, true, true).expect("dry-run 不该失败");
            assert!(local.dry_run);
            let expected_first = power_attempts(action)[0].command();
            assert_eq!(
                local.commands.first().map(String::as_str),
                Some(expected_first.as_str()),
                "{:?} 的 dry-run 计划必须来自命令表",
                action
            );

            // 远程/CLI：需要本机授权的候选只能以「已跳过」出现
            let remote = run_power_action(action, false, true).expect("dry-run 不该失败");
            for candidate in power_attempts(action) {
                if candidate.needs_gui_auth {
                    assert!(
                        remote.commands.iter().any(|line| {
                            line.contains(&candidate.command()) && line.contains("需要本机授权")
                        }),
                        "{:?} 的授权候选在远程计划里没有标记为已跳过: {:?}",
                        action,
                        remote.commands
                    );
                } else {
                    assert!(
                        remote
                            .commands
                            .iter()
                            .any(|line| line == &candidate.command()),
                        "{:?} 的免授权候选应可远程执行: {:?}",
                        action,
                        remote.commands
                    );
                }
            }
        }
    }

    #[test]
    fn hibernation_verdict_from_powercfg_output() {
        // 本机实测原文（这部分是关键行；GBK 解码后）
        let zh_disabled = "此系统上有以下睡眠状态: 待机 (S3) 此系统上没有以下睡眠状态: 休眠 尚未启用休眠 混合睡眠";
        assert_eq!(
            hibernation_from_powercfg_output(zh_disabled),
            HibernationState::Disabled
        );
        // 英文输出
        assert_eq!(
            hibernation_from_powercfg_output(
                "The following sleep states are not available: Hibernate - Not available."
            ),
            HibernationState::Disabled
        );
        assert_eq!(
            hibernation_from_powercfg_output(
                "The following sleep states are available: Standby, Hibernate"
            ),
            HibernationState::Enabled
        );
        // 中文只提「休眠」但没说「尚未启用」：保守按「会休眠」提示
        assert_eq!(
            hibernation_from_powercfg_output("此系统上有以下睡眠状态: 休眠"),
            HibernationState::Enabled
        );
        // 完全看不出来：报未判定，不猜
        assert_eq!(
            hibernation_from_powercfg_output(""),
            HibernationState::Unknown
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_power_table_matches_design() {
        let reboot = power_attempts(PowerAction::Reboot);
        assert_eq!(reboot.len(), 1);
        assert_eq!(reboot[0].command(), "shutdown /r /t 0");
        assert!(!reboot[0].needs_gui_auth, "Windows 的 shutdown 不需要提权");

        let shutdown = power_attempts(PowerAction::Shutdown);
        assert_eq!(shutdown.len(), 1);
        assert_eq!(shutdown[0].command(), "shutdown /s /t 0");

        let sleep = power_attempts(PowerAction::Sleep);
        assert_eq!(sleep.len(), 1);
        assert_eq!(
            sleep[0].command(),
            "rundll32.exe powrprof.dll,SetSuspendState 0,1,0"
        );
        assert!(!sleep[0].needs_gui_auth);

        // 睡眠预检必须给出证据，并说清这次是睡眠还是休眠（Q10′ 方案 C）
        let (note, evidence) = sleep_probe();
        let note = note.expect("Windows 上睡眠必须有预检结论");
        assert!(!evidence.is_empty(), "预检必须留下证据");
        assert!(
            note.contains("睡眠") || note.contains("休眠"),
            "结论要说明是睡眠还是休眠: {note}"
        );
    }

    #[test]
    fn busy_state_reports_what_is_running() {
        assert!(!BusyState::default().any());
        assert_eq!(BusyState::default().describe(), "空闲");
        let scanning = BusyState {
            scanning: true,
            ..Default::default()
        };
        assert!(scanning.any());
        assert_eq!(scanning.describe(), "扫描");
        let everything = BusyState {
            power: Some("restart".to_string()),
            switching: true,
            scanning: true,
            unmounting: true,
        };
        assert!(everything.any());
        assert_eq!(everything.describe(), "restart+切换系统+扫描+卸载临时挂载");
        // 扫描不进托盘菜单指纹：它太快，不值得为它重建两次菜单
        let scanning = BusyState {
            scanning: true,
            ..Default::default()
        };
        assert!(scanning.any());
        assert!(!scanning.affects_menu());
        assert_eq!(scanning.menu_describe(), "power=-|sw=0|un=0");
        let unmounting = BusyState {
            unmounting: true,
            ..Default::default()
        };
        assert!(unmounting.affects_menu());
        assert_eq!(unmounting.menu_describe(), "power=-|sw=0|un=1");
        assert!(everything.affects_menu());
    }

    // ==================== 阶段 3/4：HTTP + 远程请求 ====================

    fn sample_request(
        path: &str,
        query: &[(&str, &str)],
        source_is_loopback: bool,
    ) -> RemoteRequest {
        RemoteRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: query
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            body: Vec::new(),
            token: Some("t".to_string()),
            source: if source_is_loopback {
                "127.0.0.1".to_string()
            } else {
                "192.168.1.20".to_string()
            },
            source_is_loopback,
        }
    }

    #[test]
    fn settings_defaults_show_up_in_the_file() {
        // 新装/升级后写回磁盘的就是这些默认值：默认开服务、默认 0.0.0.0:12300、
        // 通知默认开、两条渠道默认各 5 秒、远程默认不弹授权框
        let settings = Settings::default();
        assert!(settings.http_enabled);
        assert_eq!(settings.http_bind, DEFAULT_HTTP_BIND);
        assert_eq!(settings.http_port, DEFAULT_HTTP_PORT);
        assert_eq!(settings.http_port, 12300);
        assert_eq!(settings.http_bind, "0.0.0.0");
        assert!(settings.http_token.is_empty(), "token 由首次运行生成");
        assert!(settings.notify_enabled);
        assert!(!settings.remote_allow_gui_auth);
        assert_eq!(settings.channels, Channels::default());
        assert!(settings.channels.http.notify);
        assert_eq!(settings.channels.http.delay_seconds, 5);
        assert!(settings.channels.cli.notify);
        assert_eq!(settings.channels.cli.delay_seconds, 5);
    }

    #[test]
    fn settings_json_missing_new_fields_falls_back_to_defaults() {
        // 老版本写下的 settings.json 没有这些字段，必须能读进来且取默认值
        let raw = r#"{"autostart":true,"closeAction":"tray"}"#;
        let settings: Settings = serde_json::from_str(raw).expect("老文件必须能解析");
        assert!(settings.autostart);
        assert!(settings.http_enabled);
        assert_eq!(settings.http_port, 12300);
        assert!(settings.notify_enabled);
        assert_eq!(settings.channels.http.delay_seconds, 5);
        assert_eq!(settings.channels.cli.delay_seconds, 5);
    }

    #[test]
    fn channel_delay_is_clamped_and_both_channels_are_independent() {
        let mut channels = Channels::default();
        assert_eq!(
            ChannelNotify {
                notify: true,
                delay_seconds: 100_000,
            }
            .clamped_delay(),
            MAX_DELAY_SECONDS,
            "超大延迟必须夹取"
        );
        assert_eq!(MAX_DELAY_SECONDS, 600);

        channels.for_channel_mut(Channel::Cli).delay_seconds = 0;
        assert_eq!(channels.cli.delay_seconds, 0);
        assert_eq!(channels.http.delay_seconds, 5, "cli 改动不能影响 http");
        channels.for_channel_mut(Channel::Http).notify = false;
        assert!(!channels.http.notify);
        assert!(channels.cli.notify);

        // 托盘的预设同时作用于两条渠道
        channels.set_delay_all(10);
        assert_eq!(channels.http.delay_seconds, 10);
        assert_eq!(channels.cli.delay_seconds, 10);
    }

    #[test]
    fn channels_parse_aliases_and_labels() {
        assert_eq!(Channel::parse("http"), Some(Channel::Http));
        assert_eq!(Channel::parse("WEBHOOK"), Some(Channel::Http));
        assert_eq!(Channel::parse(" cli "), Some(Channel::Cli));
        assert_eq!(Channel::parse("ssh"), Some(Channel::Cli));
        assert_eq!(Channel::parse("http2"), None);
        assert_eq!(Channel::Http.as_str(), "http");
        assert_eq!(Channel::Cli.label(), "SSH/CLI");
    }

    #[test]
    fn token_generation_and_comparison_are_strict() {
        let token = generate_token();
        assert_eq!(token.len(), 32, "32 位十六进制");
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(generate_token(), generate_token(), "两次生成必须不同");
        assert_eq!(short_request_id().len(), 8);

        assert!(token_matches("abc123", "abc123"));
        assert!(token_matches("abc123", " abc123 "), "两侧空白容错");
        assert!(!token_matches("abc123", "abc124"));
        assert!(!token_matches("abc123", "abc12"), "长度不同直接拒绝");
        assert!(
            !token_matches("", ""),
            "空 token 一律不通过（服务也不会起）"
        );
        assert!(!token_matches("abc123", ""));
    }

    #[test]
    fn token_hint_never_leaks_the_middle() {
        let token = "0123456789abcdef0123456789abcdef";
        let hint = token_hint(token);
        assert_eq!(hint, "0123…cdef");
        assert!(!hint.contains("56789abc"), "中间那段不能出现在界面提示里");
        assert_eq!(token_hint("short"), "*****");
        assert!(token_hint("").contains("未设置"));
    }

    #[test]
    fn remote_channel_only_trusts_loopback_for_cli() {
        // 本机（CLI 转发 / ssh 隧道）可以声明 cli
        let request = sample_request("/api/power", &[("channel", "cli")], true);
        assert_eq!(remote_channel(&request), Channel::Cli);
        // 远端伪造 channel=cli 无效 → 按 http 渠道
        let request = sample_request("/api/power", &[("channel", "cli")], false);
        assert_eq!(remote_channel(&request), Channel::Http);
        // 远端也不能用 notify/delay 覆盖配置（这里根本没有这些参数）
        let request = sample_request(
            "/api/power",
            &[("channel", "cli"), ("notify", "0"), ("delay", "0")],
            false,
        );
        assert_eq!(remote_channel(&request), Channel::Http);
        // 本机写个没见过的渠道也别崩
        let request = sample_request("/api/power", &[("channel", "ftp")], true);
        assert_eq!(remote_channel(&request), Channel::Http);
        // 不带参数默认 http
        let request = sample_request("/api/power", &[], true);
        assert_eq!(remote_channel(&request), Channel::Http);
    }

    #[test]
    fn route_remote_command_validates_every_endpoint() {
        // /api/switch
        assert_eq!(
            route_remote_command(&sample_request("/api/switch", &[("system", "linux")], true)),
            Ok(RemoteCommand::Switch("linux".to_string()))
        );
        let reply =
            route_remote_command(&sample_request("/api/switch", &[], true)).expect_err("缺 system");
        assert_eq!(reply.status, 400);
        let reply = route_remote_command(&sample_request(
            "/api/switch",
            &[("system", "solaris")],
            true,
        ))
        .expect_err("未知系统");
        assert_eq!(reply.status, 400);

        // /api/power：缺省 = reboot（默认就是重启）
        assert_eq!(
            route_remote_command(&sample_request("/api/power", &[], true)),
            Ok(RemoteCommand::Power(PowerAction::Reboot))
        );
        assert_eq!(
            route_remote_command(&sample_request("/api/power", &[("action", "sleep")], true)),
            Ok(RemoteCommand::Power(PowerAction::Sleep))
        );
        assert_eq!(
            route_remote_command(&sample_request(
                "/api/power",
                &[("action", "shutdown")],
                true
            )),
            Ok(RemoteCommand::Power(PowerAction::Shutdown))
        );
        let reply = route_remote_command(&sample_request(
            "/api/power",
            &[("action", "explode")],
            true,
        ))
        .expect_err("未知动作");
        assert_eq!(reply.status, 400);

        // /api/switch-and-power
        assert_eq!(
            route_remote_command(&sample_request(
                "/api/switch-and-power",
                &[("system", "windows")],
                true
            )),
            Ok(RemoteCommand::SwitchAndPower {
                system: "windows".to_string(),
                power: PowerAction::Reboot,
            })
        );
        assert_eq!(
            route_remote_command(&sample_request(
                "/api/switch-and-power",
                &[("system", "macos"), ("action", "shutdown")],
                true
            )),
            Ok(RemoteCommand::SwitchAndPower {
                system: "macos".to_string(),
                power: PowerAction::Shutdown,
            })
        );

        // 未知路径 404
        let reply = route_remote_command(&sample_request("/api/whatever", &[], true))
            .expect_err("未知路径");
        assert_eq!(reply.status, 404);
    }

    #[test]
    fn remote_request_reads_query_first_then_body() {
        let mut request = sample_request("/api/switch", &[("system", "linux")], true);
        request.body = vec![("system".to_string(), "macos".to_string())];
        assert_eq!(request.param("system").as_deref(), Some("linux"));
        assert_eq!(request.param("missing"), None);
        // 只有 body 里的参数也能取到（POST 表单）
        let mut request = sample_request("/api/switch", &[], true);
        request.body = vec![("system".to_string(), "macos".to_string())];
        assert_eq!(request.param("system").as_deref(), Some("macos"));
    }

    #[test]
    fn notify_scheduler_fires_once_per_state() {
        let now = Instant::now();
        let pending = PendingRequest {
            id: "deadbeef".to_string(),
            channel: Channel::Http,
            source: "192.168.1.20".to_string(),
            summary: "重启系统".to_string(),
            command: RemoteCommand::Power(PowerAction::Reboot),
            seconds: 5,
            deadline: now + std::time::Duration::from_secs(5),
            created: "2026-10-09T00:00:00Z".to_string(),
        };
        // 还没到点：什么都不做
        assert_eq!(
            notify_due(Some(&NotifyState::Pending(pending.clone())), now),
            NotifyDue::Nothing
        );
        // 到点：执行（只在这一刻判定一次，执行完槽位就被清掉）
        match notify_due(
            Some(&NotifyState::Pending(pending.clone())),
            now + std::time::Duration::from_secs(6),
        ) {
            NotifyDue::Execute(fired) => assert_eq!(fired.id, "deadbeef"),
            other => panic!("到点应执行，实际 {other:?}"),
        }
        // 取消后（槽位清空）到点也不会执行
        assert_eq!(
            notify_due(None, now + std::time::Duration::from_secs(60)),
            NotifyDue::Nothing
        );

        // 事后提示：到点关窗
        let info = NotifyState::Info {
            message: "已执行".to_string(),
            source: "192.168.1.20".to_string(),
            expires_at: now + std::time::Duration::from_secs(3),
        };
        assert_eq!(notify_due(Some(&info), now), NotifyDue::Nothing);
        assert_eq!(
            notify_due(Some(&info), now + std::time::Duration::from_secs(4)),
            NotifyDue::CloseInfo
        );
    }

    #[test]
    fn pending_seconds_left_counts_down() {
        let pending = PendingRequest {
            id: "x".to_string(),
            channel: Channel::Cli,
            source: "127.0.0.1".to_string(),
            summary: "切换下次启动为 Linux".to_string(),
            command: RemoteCommand::Switch("linux".to_string()),
            seconds: 5,
            deadline: Instant::now() + std::time::Duration::from_secs(5),
            created: "2026-10-09T00:00:00Z".to_string(),
        };
        assert!(pending.seconds_left() <= 5);
        assert!(pending.seconds_left() >= 4);
    }

    #[test]
    fn netstat_parsing_finds_the_listening_pid() {
        // 真实 netstat -ano 的形状（含中文表头、UDP 行、TIME_WAIT 行）
        let sample = [
            "活动连接",
            "  协议  本地地址          外部地址        状态           PID",
            "  TCP    0.0.0.0:12300          0.0.0.0:0              LISTENING       29220",
            "  TCP    0.0.0.0:12399          0.0.0.0:0              LISTENING       39360",
            "  UDP    0.0.0.0:5353           *:*                                    1234",
            "  TCP    127.0.0.1:12300        127.0.0.1:54321        TIME_WAIT       0",
            "  TCP    127.0.0.1:54321        127.0.0.1:12300        ESTABLISHED     777",
        ];
        let find = |port| parse_netstat_listening_port(sample.iter().copied(), port);

        assert_eq!(find(12399), Some(39360));
        // 12300 既有 LISTENING 行也在别人的「外部地址」列出现：必须拿 LISTENING 那行
        assert_eq!(find(12300), Some(29220));
        // UDP 不算占用 TCP 端口
        assert_eq!(find(5353), None);
        // 端口号必须整体匹配：1230 不能被 :12300 骗到
        assert_eq!(find(1230), None);

        // IPv6 同端口的 LISTENING 行同样算占用
        let ipv6 = ["  TCP    [::]:12300             [::]:0                 LISTENING       29220"];
        assert_eq!(
            parse_netstat_listening_port(ipv6.iter().copied(), 12300),
            Some(29220)
        );
    }

    #[test]
    fn port_occupant_messages_distinguish_self_and_foreign_processes() {
        let foreign = PortOccupant {
            pid: 29220,
            name: "explorer.exe".to_string(),
            is_self: false,
            same_app: false,
        };
        let message = foreign.display();
        assert!(message.contains("29220") && message.contains("explorer.exe"));
        assert!(message.contains("改用其他端口"));
        assert_eq!(
            foreign.kill_hint().as_deref(),
            Some(if cfg!(target_os = "windows") {
                "taskkill /PID 29220 /F"
            } else {
                "kill 29220"
            })
        );

        // 本程序的另一个实例（升级后旧版驻留的典型场景）：文案不同，且允许点按钮结束
        let same_app = PortOccupant {
            pid: 40048,
            name: "anyboot.exe".to_string(),
            is_self: false,
            same_app: true,
        };
        let message = same_app.display();
        assert!(message.contains("另一个实例") && message.contains("40048"));

        // 拿不到进程名时也要有话说
        let unknown = PortOccupant {
            pid: 777,
            name: String::new(),
            is_self: false,
            same_app: false,
        };
        assert!(unknown.display().contains("777"));
        assert!(unknown.display().contains("拿不到进程名"));

        // 视图转换把文案与命令一起带出去
        let view = foreign.to_view();
        assert_eq!(view.pid, 29220);
        assert_eq!(view.name, "explorer.exe");
        assert!(!view.same_app);
        assert!(view.kill_hint.is_some());
    }

    #[test]
    fn recent_requests_keep_newest_first_and_cap_the_list() {
        let entry = |index: usize| RequestLogEntry {
            time: format!("2026-10-09T00:00:{index:02}Z"),
            source: "192.168.1.20".to_string(),
            channel: "http",
            path: "/api/power".to_string(),
            summary: format!("第 {index} 条"),
            status: "executed",
            message: "ok".to_string(),
        };
        let mut requests = Vec::new();
        for index in 0..25 {
            push_recent(&mut requests, entry(index));
        }
        assert_eq!(requests.len(), MAX_RECENT_REQUESTS);
        assert_eq!(MAX_RECENT_REQUESTS, 20);
        assert_eq!(requests[0].summary, "第 24 条", "最新在最前");
        assert_eq!(requests[19].summary, "第 5 条", "只留最近 20 条");
    }

    #[test]
    fn remote_command_summaries_read_like_sentences() {
        assert_eq!(
            RemoteCommand::Switch("linux".to_string()).summary(),
            "切换下次启动为 Linux"
        );
        assert_eq!(
            RemoteCommand::Power(PowerAction::Shutdown).summary(),
            PowerAction::Shutdown.label()
        );
        let summary = RemoteCommand::SwitchAndPower {
            system: "windows".to_string(),
            power: PowerAction::Sleep,
        }
        .summary();
        assert!(summary.contains("Windows") && summary.contains(PowerAction::Sleep.label()));
    }
}
