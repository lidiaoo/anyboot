//! 共享结构与共享 static（AppState / Settings / BusyState / Channels /
//! UI_REVISION 等）。拆分方案第 1 步落地后冻结：任何刀不得再改动这里的条目，
//! 需要新共享字段时先在这里加 pub(crate)。

use super::*;

/// 快照版本号：每次发布 +1。前端只在 revision 变大时重绘（阶段 1 修 D4）。
///
/// **起点必须跟「时间」挂钩，不能从 0 数起。** 网页端 / 手机端在
/// `applySnapshot` 里用 `state.revision <= lastRevision` 丢弃过期快照，而
/// revision 是**进程内**计数器：进程一重启又从 0 数起，客户端手里那个「旧进程的
/// 高水位」就把新进程的快照**全部**判成旧的丢掉 → 页面永久冻在重启前那份数据上。
///
/// 实测（真实 WebKit 客户端 + WebDriver）：旧进程 revision 涨到 7，客户端记下
/// lastRevision=7；杀掉进程重启后新进程才到 5 → `5 <= 7` → 客户端每 2 秒都收到
/// HTTP 200、`fetch` 全是 ok，却**一个字段都不更新**（主题、系统列表全停在旧值），
/// 而且症状是「越用越不同步」——用户根本不会联想到「我重启过 AnyBoot」。
/// 旧进程跑得越久（revision 越大），新进程要涨过那个高水位就越难，实际等于「不刷新
/// 页面就永远不同步」。
///
/// 用启动时刻（Unix 毫秒）做种子：新进程启动必然更晚 → 起点必然更大 →
/// 客户端只要继续比大小就能自然接受。真机时钟被大幅往回调（NTP 纠正）才会再现，
/// 那种情况刷新一次即可。
pub(crate) static UI_REVISION: AtomicU64 = AtomicU64::new(0);

/// revision 的起始值（纯函数便于单测）：启动时刻的 Unix 毫秒，且不为 0。
pub(crate) fn ui_revision_seed(now_millis: u64) -> u64 {
    now_millis.max(1)
}

#[derive(Default)]
pub(crate) struct AppState {
    pub(crate) vars_dir: Option<PathBuf>,
    pub(crate) explicit_dir: Option<PathBuf>,
    pub(crate) active_mount: Option<ActiveMount>,
    pub(crate) variables: Vec<VariableInfo>,
    pub(crate) message: String,
    pub(crate) error: Option<String>,
    /// GRUB 细粒度切换（与上面 rEFInd 那套并列）
    pub(crate) grub: GrubRuntime,
    /// 安装 / 卸载 09_anyboot 的执行过程日志（最近一次）
    pub(crate) script_log: ScriptLogView,
    /// 贴边自动隐藏的运行态：是否已停靠、贴在哪条边、贴住时的坐标。
    /// dock_pos 用来在 Moved 事件里区分「我们自己 set_position 触发的」和
    /// 「用户手拖的」——前者坐标一致（容差内），后者必然偏离。
    pub(crate) auto_hide_docked: bool,
    pub(crate) auto_hide_edge: Option<DockEdge>,
    /// 上一轮贴的边：undock 不清（与 auto_hide_edge 分开）。重贴边时只要没有
    /// 另一条边「严格更近」就沿用 —— 展开的窗口很高，贴左缘时离底缘往往也是
    /// 0px，纯几何重判会在开合几轮后翻成底边贴边（见 redock_edge 的注释）。
    pub(crate) auto_hide_last_edge: Option<DockEdge>,
    pub(crate) auto_hide_dock_pos: (i32, i32),
    /// 本次停靠以来「白条保障」自纠过几次：见 AUTO_HIDE_RESNAP_LIMIT。
    pub(crate) auto_hide_dock_resnaps: u8,
    /// 贴边前的窗口尺寸（物理像素）：贴边时窗口被缩成一条白条厚度
    /// （否则窗口大部分面积压在任务栏/Dock 上——窗口置顶，网页内容会把
    /// 任务栏整个盖住）。展开时按这个尺寸原样恢复。
    pub(crate) auto_hide_restore_size: Option<(u32, u32)>,
    /// 托盘呼出后的宽限期（Unix 毫秒）：呼出瞬间鼠标必然停在托盘上（窗口外），
    /// 没有宽限的话窗口刚弹出来就被 watcher 立刻贴边藏回去 —— 表现就是
    /// 「点了托盘主界面不显示」。宽限期内不累计鼠标离窗时钟。
    pub(crate) auto_hide_grace_until_ms: u64,
    /// 托盘呼出后的「等待进入」标志：呼出展开后鼠标还没进过窗口之前，
    /// watcher 一律不自动贴回。没有它就是死循环：呼出 → 800ms 宽限期一过、
    /// 鼠标还在托盘 → 贴回 → 用户再点托盘 → 再展开 → 再贴回……每轮都有
    /// show/hide + set_decorations 样式切换，两个窗口互相闪烁直到卡死。
    /// 鼠标进过一次窗口（用户真的用过界面）后清除，恢复正常的离窗贴边。
    pub(crate) auto_hide_pending_enter: bool,
    /// undock 重锚计划：展开请求落点 + 已补发次数。GTK 在隐藏态恢复装饰后
    /// frame extents 缓存错拍，首个 set_position 的实际落点会偏低（2x 缩放
    /// 下正好一个标题栏 64px）——watcher 随后几拍读实况、偏了就趁缓存新鲜
    /// 补发一次 set_position（见 undock_reanchor_action）。收敛/放弃/重新
    /// dock 都清掉。
    pub(crate) auto_hide_reanchor: Option<AutoHideReanchor>,
    /// 跨轮记住的展开请求修正量（语义同 `AutoHideReanchor.offset`）：由重锚
    /// 收敛时写回，下一次 undock 的**首次**落位就带修正 —— 不用每轮都先偏
    /// 64px 再弹回来。WM 语义变化（换主题/改缩放）时重锚会重新学。
    pub(crate) auto_hide_undock_offset: (i32, i32),
    /// 标题栏高度（物理像素，窗框顶 − 客户区顶，dock 时窗口还带着装饰、
    /// 可见，读数三端都准）：顶边贴边展开时给标题栏让位用（见 undock_target）。
    /// undock 时现场实测的边距在 Linux 上可能读到 0（GDK frame extents
    /// 缓存未刷新），此时退回这个缓存值。
    pub(crate) auto_hide_caption: i32,
}

/// 主窗口贴边自动隐藏时贴向的屏幕边。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DockEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl DockEdge {
    /// 转成前端用的小写字符串（与 CSS 类名、白条方位一一对应）。
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            DockEdge::Top => "top",
            DockEdge::Bottom => "bottom",
            DockEdge::Left => "left",
            DockEdge::Right => "right",
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VariableInfo {
    pub(crate) system: String,
    pub(crate) display_name: String,
    pub(crate) source_name: String,
    pub(crate) source: PathBuf,
    pub(crate) exists: bool,
    pub(crate) active: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppStatus {
    pub(crate) vars_dir: Option<PathBuf>,
    pub(crate) temporary_mount: bool,
    pub(crate) variables: Vec<VariableInfo>,
    pub(crate) message: String,
    pub(crate) error: Option<String>,
}

/// 主窗口「GRUB 启动项」区块的展示数据。
///
/// 这一块和上面的三个系统项是**并列**的两套机制：上面走 rEFInd 的 `PreviousBoot`，
/// 这里走 GRUB 自己的环境块。用户设了 ESP 的分区 UUID 才会出现。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GrubView {
    /// 用户填写的分区 UUID 原文（空 = 未配置）
    pub(crate) partuuid: String,
    /// 用户填写的 ESP 目录原文（空 = 没填，只靠 fs-uuid 找）
    pub(crate) dir: String,
    /// 识别出来的 ESP 分区（是不是 FAT32 / fs-uuid / 挂载状态）
    pub(crate) esp: Option<EspInfo>,
    /// 「保持挂载」勾选状态
    pub(crate) keep_mounted: bool,
    /// 本端能不能按分区 UUID 挂载（Windows = false，勾选框要置灰）
    pub(crate) mount_supported: bool,
    /// 本端解析出的 ESP 目录（Windows 是盘符、Linux 是挂载点）
    pub(crate) efi_dir: Option<String>,
    /// 环境块路径（本端）
    pub(crate) env_path: Option<String>,
    /// grub.cfg 副本路径（本端）
    pub(crate) cfg_path: Option<String>,
    /// ESP 上是否已经有环境块文件（没有的话 GRUB 会报 invalid environment block）
    pub(crate) env_exists: bool,
    /// 09_anyboot 是否已安装；非 Linux 恒为 None（安装/卸载按钮只在 Linux 展示）
    pub(crate) installed: Option<bool>,
    /// 上次同步时间（本地时间串）
    pub(crate) synced_at: Option<String>,
    /// 安装 / 卸载 09_anyboot 的执行过程（最近一次，含命令与退出码）
    pub(crate) script_log: ScriptLogView,
    /// 解析出来的启动项（第一版**不过滤**，os-prober 的 Windows/OpenCore 也在内）
    pub(crate) entries: Vec<grub::GrubEntry>,
    /// submenu 分组（id + 标题）：前端据此把子条目挂到组头下面做树形层级
    pub(crate) groups: Vec<grub::GrubGroup>,
    /// 列表展示格式（raw/minimal/simple/medium/detailed）：渲染分支由前端挑
    pub(crate) list_format: String,
    /// 从环境块读回的「上次启动 / 下次启动」
    pub(crate) saved_entry: Option<String>,
    pub(crate) next_entry: Option<String>,
    pub(crate) error: Option<String>,
    /// 给用户的下一步提示
    pub(crate) hint: Option<String>,
}

/// GRUB 功能的**本端运行时状态**。与 rEFInd 那套（`AppState` 的
/// `vars_dir` / `active_mount` / `variables`）完全并列，互不影响。
#[derive(Clone, Default)]
pub(crate) struct GrubRuntime {
    /// 本端解析出的 ESP 根目录
    pub(crate) efi_dir: Option<PathBuf>,
    /// ESP 分区的识别结果（是不是 FAT32 / fs-uuid 从哪来的）
    pub(crate) esp: Option<EspInfo>,
    /// ESP 未挂载时由我们挂上去的挂载记录
    pub(crate) mount: Option<ActiveMount>,
    pub(crate) entries: Vec<grub::GrubEntry>,
    /// submenu 分组（树形展示的组头）
    pub(crate) groups: Vec<grub::GrubGroup>,
    pub(crate) env_path: Option<PathBuf>,
    pub(crate) cfg_path: Option<PathBuf>,
    pub(crate) env_exists: bool,
    pub(crate) saved_entry: Option<String>,
    pub(crate) next_entry: Option<String>,
    pub(crate) installed: Option<bool>,
    pub(crate) synced_at: Option<String>,
    pub(crate) error: Option<String>,
}

/// 远程渠道：http（webhook / curl / 面板）与 cli（ssh / CLI 转发）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Channel {
    Http,
    Cli,
}

impl Channel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Cli => "cli",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Http => "HTTP",
            Self::Cli => "SSH/CLI",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
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
pub(crate) struct ChannelNotify {
    pub(crate) notify: bool,
    pub(crate) delay_seconds: u32,
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
    pub(crate) fn clamped_delay(&self) -> u32 {
        self.delay_seconds.min(MAX_DELAY_SECONDS)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Channels {
    pub(crate) http: ChannelNotify,
    pub(crate) cli: ChannelNotify,
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
    pub(crate) fn for_channel(&self, channel: Channel) -> &ChannelNotify {
        match channel {
            Channel::Http => &self.http,
            Channel::Cli => &self.cli,
        }
    }

    pub(crate) fn for_channel_mut(&mut self, channel: Channel) -> &mut ChannelNotify {
        match channel {
            Channel::Http => &mut self.http,
            Channel::Cli => &mut self.cli,
        }
    }

    /// 托盘里的「远程执行延迟」预设：同时作用于两条渠道（细分去主窗口）。
    pub(crate) fn set_delay_all(&mut self, seconds: u32) {
        self.http.delay_seconds = seconds;
        self.cli.delay_seconds = seconds;
    }
}

/// 一条远程命令（HTTP 端点与 CLI 转发共用同一份语义）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteCommand {
    Switch(String),
    Power(PowerAction),
    SwitchAndPower {
        system: String,
        power: PowerAction,
    },
    /// GRUB 细粒度条目（第二条链路）：selector 是 grub.cfg 里解析出的原文，
    /// submenu 内为 `submenu_id>entry_id`。只写环境块、不重启。
    SwitchGrub(String),
    /// /api/power?action=wake 的应答：这条 HTTP 能送达就说明机器本来就醒着，
    /// 什么都不用做 —— 给「唤醒路由勾了 HTTP 兜底」一条说得通的回执。
    WakeAck,
}

impl RemoteCommand {
    pub(crate) fn summary(&self) -> String {
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
            Self::SwitchGrub(selector) => format!("切换 GRUB 启动项为 {selector}"),
            Self::WakeAck => "唤醒（本机在线，无需动作）".to_string(),
        }
    }
}

/// 一条待执行请求（延迟窗口内可取消）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingRequest {
    pub(crate) id: String,
    pub(crate) channel: Channel,
    pub(crate) source: String,
    pub(crate) summary: String,
    pub(crate) command: RemoteCommand,
    pub(crate) seconds: u64,
    pub(crate) deadline: Instant,
    pub(crate) created: String,
}

impl PendingRequest {
    pub(crate) fn seconds_left(&self) -> u64 {
        self.deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
    }
}

/// 「最近请求」列表里的一条。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestLogEntry {
    pub(crate) time: String,
    pub(crate) source: String,
    pub(crate) channel: &'static str,
    pub(crate) path: String,
    pub(crate) summary: String,
    /// pending / executed / cancelled / failed / rejected
    pub(crate) status: &'static str,
    pub(crate) message: String,
}

/// 通知弹窗要显示的东西（进快照，弹窗窗口和主窗口读同一份）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotifyView {
    /// pending（倒计时，可取消）或 info（已执行的事后提示）
    pub(crate) kind: &'static str,
    pub(crate) id: Option<String>,
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) source: String,
    pub(crate) channel: &'static str,
    pub(crate) seconds_left: u64,
    /// 延迟总秒数（弹窗画进度条用）
    pub(crate) total_seconds: u64,
    /// 发起时间（UTC）
    pub(crate) created: String,
    pub(crate) can_cancel: bool,
    /// 是否播放提示音
    pub(crate) play_sound: bool,
    /// 自定义提示音文件路径（空 = 系统默认）
    pub(crate) sound_file_path: String,
}

pub(crate) enum NotifyState {
    Pending(PendingRequest),
    Info {
        message: String,
        source: String,
        expires_at: Instant,
    },
}

/// 调度线程到点该做什么（纯判定，便于单测；副作用在调用方）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotifyDue {
    Nothing,
    /// 倒计时结束 → 执行
    Execute(Box<PendingRequest>),
    /// 事后提示到期 → 关窗
    CloseInfo,
}

pub(crate) fn notify_due(state: Option<&NotifyState>, now: Instant) -> NotifyDue {
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
pub(crate) struct HttpRuntime {
    pub(crate) server: Option<http::RunningServer>,
    pub(crate) last_error: Option<String>,
    /// 启动失败时诊断出的端口占用者（只读探测，绝不自动杀）
    pub(crate) occupant: Option<PortOccupant>,
    pub(crate) requests: Vec<RequestLogEntry>,
    pub(crate) notify: Option<NotifyState>,
}

/// 进快照的 HTTP 视图。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HttpView {
    pub(crate) enabled: bool,
    pub(crate) bind: String,
    pub(crate) port: u16,
    pub(crate) listening: bool,
    pub(crate) addr: Option<String>,
    pub(crate) last_error: Option<String>,
    /// 端口被谁占了（启动失败才有）
    pub(crate) port_occupant: Option<PortOccupantView>,
    pub(crate) token_set: bool,
    /// 打码后的 token（只给前 4/后 4 位，明文要用 reveal 命令专门取）
    pub(crate) token_hint: String,
    /// 监听状态的一句话：主窗口 `#http-state` 与托盘「远程控制」子菜单**共用**同一份文案，
    /// 免得两边各拼各的、说着说着就不一致了。
    pub(crate) state_text: String,
    pub(crate) requests: Vec<RequestLogEntry>,
}

/// 正在进行的操作。进快照后托盘菜单项与主窗口按钮**同时**置灰（阶段 1 修 D3：
/// 以前托盘和主窗口各管各的，一边在重启、另一边照样能点，第二次点击被静默丢弃）。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BusyState {
    /// 正在提交的电源动作（阶段 2 起是 restart/shutdown/sleep，阶段 1 只有 restart）
    pub(crate) power: Option<String>,
    /// 正在切换下次启动系统
    pub(crate) switching: bool,
    /// 正在扫描变量目录
    pub(crate) scanning: bool,
    /// 正在卸载临时挂载
    pub(crate) unmounting: bool,
}

impl BusyState {
    /// 「有任何动作在跑吗」。主窗口前端用同一份 busy 快照自己判断，
    /// Rust 侧目前只有单测直接调用它（菜单用的是 affects_menu）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn any(&self) -> bool {
        self.power.is_some() || self.switching || self.scanning || self.unmounting
    }

    /// 影响**托盘菜单内容**（置灰）的部分。
    ///
    /// 刻意不含 scanning：扫描通常只几毫秒，但它前后各改一次菜单内容，
    /// 会把「每次扫描重建一次菜单」变成两次（菜单闪断的诱因）。
    /// 主窗口那侧仍然对 scanning 置灰（同一个 busy 快照，前端自己判断）。
    pub(crate) fn affects_menu(&self) -> bool {
        self.power.is_some() || self.switching || self.unmounting
    }

    /// 菜单指纹里用的 busy 摘要（只含 affects_menu 的部分）。
    pub(crate) fn menu_describe(&self) -> String {
        format!(
            "power={}|sw={}|un={}",
            self.power.as_deref().unwrap_or("-"),
            self.switching as u8,
            self.unmounting as u8
        )
    }

    pub(crate) fn describe(&self) -> String {
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
pub(crate) struct ActionResult {
    pub(crate) ok: bool,
    pub(crate) action: String,
    pub(crate) message: String,
}

/// 主窗口与托盘菜单**共用**的唯一状态快照（阶段 1 修 D1/D4）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiState {
    pub(crate) revision: u64,
    pub(crate) status: AppStatus,
    pub(crate) settings: SettingsView,
    pub(crate) busy: BusyState,
    pub(crate) last_action: Option<ActionResult>,
    pub(crate) http: HttpView,
    pub(crate) notify: Option<NotifyView>,
    /// GRUB 细粒度切换（与 status 里 rEFInd 那三个系统项并列的第二块）
    pub(crate) grub: GrubView,
    /// 当前生效的 09_anyboot 正文（供界面「查看 / 修改」用）
    pub(crate) grub_script: String,
    /// 主窗口当前是否处于「贴边隐藏」态（前端据此在窗口边界画一条白条）
    pub(crate) docked: bool,
    /// 贴向的屏幕边（top/bottom/left/right）；未贴边时为 null
    pub(crate) dock_edge: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CloseAction {
    Tray,
    Quit,
}

impl CloseAction {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "tray" => Some(Self::Tray),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Settings {
    pub(crate) autostart: bool,
    pub(crate) start_in_tray: bool,
    pub(crate) hide_from_dock_taskbar: bool,
    pub(crate) close_action: CloseAction,
    /// 取证日志开关（阶段 0）。默认关；环境变量 ANYBOOT_TRACE 可临时覆盖。
    pub(crate) debug_trace: bool,
    /// 内置 HTTP 服务（默认开启）
    pub(crate) http_enabled: bool,
    /// 监听地址（默认 0.0.0.0，配 token 才能用）
    pub(crate) http_bind: String,
    pub(crate) http_port: u16,
    /// 首次运行自动生成的 32 位十六进制 token；为空则服务拒绝启动
    pub(crate) http_token: String,
    /// 本机标识（32 位十六进制，首次运行生成后持久化）。手机 / 网页客户端靠它认出
    /// 「这几条 IP 其实是同一台电脑」—— 一台电脑可能有多块网卡、多次 DHCP 拿到
    /// 多个 IP，客户端据此把它们归并成一条记录、一个页面，而不是一个 IP 一个标签。
    /// 通过 `GET /health` **免鉴权**返回：客户端还没填 token 时也要能归并。
    pub(crate) machine_id: String,
    /// 远程 / CLI 请求是否允许触发本机授权弹窗（默认开：用户定案「远程可触发
    /// 本机框」—— 远程动作要不要真的提权，由本机这台机器说了算）
    pub(crate) remote_allow_gui_auth: bool,
    /// 弹窗通知总开关（Q11）
    pub(crate) notify_enabled: bool,
    /// 每个渠道各自的弹窗开关与延迟秒数（Q14）
    pub(crate) channels: Channels,
    /// 提示音开关（默认开）
    pub(crate) play_sound: bool,
    /// 自定义提示音文件路径（留空用内置的 KakaoTalk 提示音）
    pub(crate) sound_file_path: String,
    /// 界面主题：dark / light / system
    pub(crate) theme: String,
    /// 窗口不透明度（百分比，50–100）
    pub(crate) opacity_percent: u8,
    /// GRUB 细粒度切换用的 ESP 识别键：**分区 UUID**（GPT PARTUUID，如
    /// `F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3`）。它写在 GPT 分区表里，三端
    /// （Linux lsblk / macOS diskutil / Windows Get-Partition）报的都是同一个
    /// 值。空 = 未配置，界面不展示 GRUB 区块。
    /// 注意与 fs-uuid（FAT 卷序列号，GRUB `search --fs-uuid` 用的 8 位串）是
    /// 两套键：GRUB 脚本里需要的 fs-uuid 在 Linux 上由分区 UUID 现场解析。
    pub(crate) grub_efi_partuuid: String,
    /// 用户在界面上指定的 ESP 目录（形如 `H:\` / `/boot/efi` / `/Volumes/ARCH-EFI`）。
    /// 只在「识别」时用来反查 fs-uuid，并把结果归一化成分区根目录存下来。
    /// fs-uuid 才是权威键，这个只是入口和展示。
    pub(crate) grub_efi_dir: String,
    /// 勾上「挂载 ESP」后保持挂载：每次扫描不再自动卸掉自己挂的那份。
    pub(crate) grub_keep_mounted: bool,
    /// 用户在界面上改过的 09_anyboot 正文（空 = 用内置模板）。
    /// 安装时只替换 `@@ESP_UUID@@`，其余按用户改的来。
    pub(crate) grub_script_override: String,
    /// GRUB 区块的展示方式（四种，全在同一个主窗口里）：
    /// inline（内嵌）/ wide（加宽窗口展示）/ list（列表独立到右栏）/
    /// pane（GRUB 全部内容在右栏）。老配置里的 window 语义由 pane 承接。
    pub(crate) grub_list_view: String,
    /// GRUB 列表的展示格式（前端渲染偏好，纯展示不改数据）：
    /// raw（原始）/ minimal（极简）/ simple（简易）/ medium（中等）/
    /// detailed（复杂）。落盘存归一化后的小写值。
    pub(crate) grub_list_format: String,
    /// 主窗口贴边自动隐藏：鼠标离开后窗口贴到最近的屏幕边缘、只露一条边，
    /// 悬停边条再展开。关掉时窗口保持完整可见。
    pub(crate) main_auto_hide: bool,
    /// 主窗口始终置顶（默认开）：任何情况下都不被别的窗口盖住。关掉后窗口
    /// 回到普通层级，可被覆盖。Windows 上是 HWND_TOPMOST（稳），Linux/macOS
    /// 上 hide→重装饰→show 一圈会把置顶层级丢掉 —— 所以每个 show/落位点都要补钉。
    pub(crate) main_always_on_top: bool,
    /// 贴边自动隐藏的触发距离（逻辑像素）：主窗口离工作区边框不超过这个距离、
    /// 且鼠标离开主窗口/独立窗口超过延迟，才贴边隐藏。窗口在屏幕中央时
    /// 鼠标离开不隐藏（用户明确要求「正中央不触发」+ 距离可自定义）。
    pub(crate) main_auto_hide_edge_distance: i32,
    /// 贴边白条距屏幕（工作区）边界的距离（逻辑像素 0–100，默认 0 = 紧贴）。
    /// 停靠落点沿法线方向离边留这条缝，白条不再压死在屏幕边上。
    pub(crate) main_dock_strip_edge_gap: i32,
    /// 悬浮白条多少毫秒才呼出主窗（0–2000，默认 0 = 当拍立即呼出，既有行为）。
    pub(crate) main_dock_strip_hover_ms: u64,
    /// 贴边白条厚度（逻辑像素，2–40，默认 6）：贴边隐藏时窗口缩成这条的厚度
    pub(crate) main_dock_strip_thickness: i32,
    /// 贴边白条长度（占窗口边长的百分比 10–100，默认 100 = 整条边）
    pub(crate) main_dock_strip_length: i32,
    /// 贴边白条不透明度（百分比 10–100，默认 100）
    pub(crate) main_dock_strip_opacity: i32,
    /// 贴边白条颜色（#rrggbb，默认白色）：调色板选的色调，纯前端 CSS 渲染
    pub(crate) main_dock_strip_color: String,
    /// 贴边热区厚度（逻辑像素 1–100，默认 40 = 既有槽位缝厚，行为不变）：
    /// 停靠态从工作区边往内这一段是「鼠标移入触发呼出」的热区，厚度独立于
    /// 白条视觉厚度 —— 判定是后端按几何算的虚拟矩形，不必是窗口那么厚。
    pub(crate) main_hotzone_thickness: i32,
    /// 热区调试底色（#rrggbb）：配合透明度可视化热区边界，方便调试
    pub(crate) main_hotzone_color: String,
    /// 热区不透明度（百分比 0–100，默认 0 = 全透明 = 默认无颜色，看不见）
    pub(crate) main_hotzone_opacity: i32,
    /// 热区是否画边框（默认关 = 无边框）
    pub(crate) main_hotzone_border: bool,
    /// 热区边框颜色（#rrggbb）
    pub(crate) main_hotzone_border_color: String,
    /// 主题强调色（#rrggbb，默认内置靛蓝 #6366f1，与远程管理同色）：调色板选的色调，
    /// 前端据此派生按钮/链接/悬停色
    pub(crate) accent_color: String,
    /// 主窗口固定显示位置模式：free=用户自由拖动（默认，保持现状）；
    /// center=每次显示都居中；last=记住上次手动摆放的位置。
    pub(crate) main_window_position: String,
    /// 托盘菜单方式：classic=三端各用系统原生托盘菜单（三端自绘之前的原始方案，
    /// 默认）；unified=全局统一 —— 三端右键都开**同一扇自绘菜单窗**（Win/mac 托盘
    /// 事件直开；Linux 自研 SNI 按 WM 分流：GNOME 导出空 dbusmenu、宿主弹窗打开
    /// 瞬间弹自绘窗，Cinnamon/KDE 导出全量 dbusmenu 由宿主菜单渲染，2026-10-15
    /// 定案）。两种形态的托盘接线在启动时二选一，切换后重启应用生效。
    pub(crate) tray_menu_style: String,
    /// 「上次位置」模式记忆的落点（物理像素；未设定为 None，回退到居中）。
    pub(crate) main_window_last: Option<(i32, i32)>,
    /// 主窗口固定显示器：`auto`（不固定，默认）或某个显示器的名字（tauri
    /// Monitor::name()，如 `DP-1` / `\\.\DISPLAY1` / 「内建视网膜显示屏」）。
    /// 固定后主窗只在这台显示器里活动：居中/呼出/落位/贴边都落在它的工作区，
    /// 拖到别的显示器会被夹回来。显示器被拔掉时回落既有挑屏逻辑（窗口总得
    /// 有地方显示），设置值保留 —— 插回来继续生效。
    pub(crate) main_fixed_monitor: String,
    /// 锁定窗口大小：锁定后窗口不可拖拽缩放、不可最大化，前端宽高滑条同步停用。
    pub(crate) window_size_locked: bool,
    /// 记忆的窗口尺寸（逻辑像素）。**0 = 还没记过**（serde 缺字段 / 全新配置），
    /// 此时启动按展示方式给标称宽度（460/932）、高度按**整屏高度的 2/3**
    /// （见 default_window_height）。
    /// 任何一次尺寸落位都会写成真实值（clamp 下限 360/560，0 不会存活）。
    /// 不能把默认填成 460/900：serde(default) 会给**所有老配置**伪造一个
    /// 「已记忆 460x900」，用户拖过的尺寸在重启后被悄悄拽回去。
    pub(crate) window_width: u32,
    pub(crate) window_height: u32,
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
            machine_id: String::new(),
            remote_allow_gui_auth: true,
            notify_enabled: true,
            channels: Channels::default(),
            play_sound: true,
            sound_file_path: String::new(),
            theme: DEFAULT_THEME.to_string(),
            opacity_percent: DEFAULT_OPACITY_PERCENT,
            grub_efi_partuuid: String::new(),
            grub_efi_dir: String::new(),
            grub_keep_mounted: true,
            grub_script_override: String::new(),
            grub_list_view: "inline".to_string(),
            grub_list_format: "minimal".to_string(),
            main_auto_hide: false,
            main_always_on_top: true,
            main_auto_hide_edge_distance: 24,
            main_dock_strip_edge_gap: 0,
            main_dock_strip_hover_ms: 0,
            main_dock_strip_thickness: 6,
            main_dock_strip_length: 100,
            main_dock_strip_opacity: 100,
            main_dock_strip_color: "#ffffff".to_string(),
            main_hotzone_thickness: 40,
            main_hotzone_color: "#ff0000".to_string(),
            main_hotzone_opacity: 0,
            main_hotzone_border: false,
            main_hotzone_border_color: "#ff0000".to_string(),
            accent_color: "#6366f1".to_string(),
            main_window_position: DEFAULT_WINDOW_POSITION.to_string(),
            tray_menu_style: DEFAULT_TRAY_MENU_STYLE.to_string(),
            main_window_last: None,
            main_fixed_monitor: "auto".to_string(),
            window_size_locked: false,
            window_width: 0,
            window_height: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsView {
    #[serde(flatten)]
    pub(crate) settings: Settings,
    pub(crate) autostart_state: &'static str,
    pub(crate) autostart_hint: Option<String>,
    /// 当前可用的显示器名列表：设置页「固定显示器」下拉与托盘同名子菜单
    /// 都用它当选项（与 Settings.mainFixedMonitor 的取值同一来源）。
    pub(crate) main_monitor_options: Vec<String>,
}

impl SettingsView {
    pub(crate) fn new(settings: Settings, state: AutostartState) -> Self {
        Self {
            settings,
            autostart_state: state.as_str(),
            autostart_hint: autostart_hint(state),
            // 选项列表依赖 AppHandle，由 view_settings 填（见那边）
            main_monitor_options: Vec::new(),
        }
    }
}

/// 堵住「主窗口和托盘各读各的」的第一层：busy 的唯一读写口。
pub(crate) fn busy_snapshot(app: &AppHandle) -> BusyState {
    app.state::<Mutex<BusyState>>()
        .lock()
        .expect("busy mutex poisoned")
        .clone()
}

/// 改 busy 并立刻把新快照发出去（托盘与主窗口一起变）。
pub(crate) fn set_busy(app: &AppHandle, update: impl FnOnce(&mut BusyState)) {
    {
        let state = app.state::<Mutex<BusyState>>();
        let mut busy = state.lock().expect("busy mutex poisoned");
        update(&mut busy);
    }
    publish_ui(app);
}

pub(crate) fn last_action_snapshot(app: &AppHandle) -> Option<ActionResult> {
    app.state::<Mutex<Option<ActionResult>>>()
        .lock()
        .expect("action mutex poisoned")
        .clone()
}

/// 记下最近一次动作的结果（进快照，替代原来的 restart-result 事件）。
pub(crate) fn record_action(app: &AppHandle, action: &str, ok: bool, message: &str) {
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
