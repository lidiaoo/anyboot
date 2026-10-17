use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender, TrySendError},
        Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{image::Image, AppHandle, Emitter, Manager};
// 菜单/托盘是桌面专属（tauri v2 里 menu/tray 模块与几乎全部窗口几何 API
// 都挂在 #[cfg(desktop)] 下），安卓/iOS 目标编译不过 —— 移动端没有托盘。
// 原生菜单已经三端全下线：托盘菜单都是同一扇自绘窗（tray-menu），原生点完必关，③ 无从谈起。
// 托盘图标 classic 方式下三端都用 tauri 的实现（原生菜单挂在托盘上）；
// unified 方式下只有 Windows / macOS 还用 tauri 的实现，Linux 那颗是自己按 SNI 导出的。
// 2026-10-15 定案：Linux 两种方式都不再用 tauri 托盘（muda 0.20 GTK3 子菜单渲染回归，
// 两形态统一走自研 linux_sni），这些类型只剩 Win/mac 用。
#[cfg(all(desktop, not(target_os = "linux")))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
// classic 原生菜单的装配类型（三端自绘之前那套，见 build_tray_menu）；
// Linux 已不走 tauri 托盘（classic 由 linux_sni 承载），这套类型只剩 Win/mac，
// 但 MenuItem 等仍有 Linux 使用点，allow 而非 cfg 掉。
#[cfg(desktop)]
#[cfg_attr(target_os = "linux", allow(unused_imports))]
use tauri::menu::{CheckMenuItem, IconMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
// autostart 插件只支持桌面三端（移动端没有「开机自启」概念）
#[cfg(desktop)]
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_dialog::DialogExt;
// `MessageDialogKind` 只有非 macOS 的「已经启动」提示在用（macOS 走 osascript，见
// `announce_already_running`）：按 cfg 导，免得 macOS 上白挨一条 unused_imports。
#[cfg(not(target_os = "macos"))]
use tauri_plugin_dialog::MessageDialogKind;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// 起子进程的**唯一**入口。
///
/// GUI 程序（`windows_subsystem = "windows"`）spawn 控制台程序时，Windows 会为子进程
/// 新建一个控制台窗口 —— 用户在倒计时弹窗旁看到的那个蓝色 PowerShell 窗口就是这么来的
/// （提示音走 powershell 播放；网口诊断、电源命令也都一样会闪一下）。
/// 统一带上 CREATE_NO_WINDOW，子进程就在后台静默跑完。
#[allow(unused_mut)]
fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        /// CREATE_NO_WINDOW：不为子进程创建控制台窗口
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

const SOURCE_FILES: [(&str, &str, &str); 3] = [
    ("linux", "Linux", "PreviousBoot-linux"),
    ("windows", "Windows", "PreviousBoot-windows"),
    ("macos", "macOS", "PreviousBoot-mac"),
];
const ESP_PARTTYPE: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
const FAT_FILESYSTEMS: [&str; 6] = ["vfat", "fat", "fat12", "fat16", "fat32", "exfat"];
mod cli;
mod grub;
mod hub;
mod hub_keys;
// pub(crate)：async 命令的 __cmd__ 宏可见性跟着模块走（private mod 会报
// "macro import is private"，sync 命令没这问题）
pub(crate) mod hub_sftp;
mod hub_term;
mod hub_tunnel;
mod hub_vault;
mod http;
mod remote_servers;

/// Linux 侧数据收集脚本（把 grubenv 迁到 ESP），装到 `/etc/grub.d/09_anyboot`。
///
/// 内容与 `linux/09_anyboot` 一致，只有 `@@ESP_UUID@@` 是占位符 —— 安装时替换成
/// 用户填的 fs-uuid，其余一字不改（用户定案 1）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const GRUB_SCRIPT_TEMPLATE: &str = r##"#!/bin/sh
exec tail -n +3 "$0"
# ============================================================================
#  AnyBoot · /etc/grub.d/09_anyboot
#  安装位置： /etc/grub.d/09_anyboot        权限： 0755（必须可执行）
#
#  ★ 开头两行 `#!/bin/sh` 与 `exec tail -n +3 "$0"` 是 shell 包装。
#    grub-mkconfig 会「执行」本文件并读取它的 stdout；包装行负责把下面这段
#    GRUB 脚本原样打印出去。删掉它们，grub-mkconfig 就会拿 sh 去跑 GRUB 命令，
#    于是报 `search: 未找到命令` / `未预期的记号 "("`。**不要删！**
#
#  作用：把 GRUB 的"环境块"(grubenv) 从 ext4 的 /boot/grub/ 迁到 ESP(FAT32)：
#          <ESP>/EFI/grubenv
#  这样 Windows / macOS 无需 ext4 支持、无需提权，即可读写"上次/下次启动项"。
#  AnyBoot 同步时也会把 grub.cfg 副本放到 <ESP>/EFI/grub.cfg（同一个父目录）。
#
#  ── 本机取值（安装时由 AnyBoot 填入）──────────────────────────────────────
#          ESP 的 fs-uuid  = @@ESP_UUID@@
#          ESP 上的环境块  = /EFI/grubenv
#
#  排序：09 —— 在 00_header 之后、10_linux 之前。GRUB 在渲染菜单前会把整份
#    grub.cfg 执行一遍，所以这里重设 default、重定义 savedefault 会覆盖
#    00_header 里指向 ext4 的旧逻辑（后写入者胜出）。
#
#  模块依赖：search / load_env / save_env 已内置在 grubx64.efi 里；fat 未内置
#    （生成的 grub.cfg 要到 40_custom 那段才 insmod fat）→ 读 ESP 前必须显式
#    insmod fat，否则 search --fs-uuid 探测不到 ESP。
# ============================================================================

insmod part_gpt
insmod part_msdos
insmod fat

if search --no-floppy --fs-uuid --set=anyboot_esp @@ESP_UUID@@ ; then
  set anyboot_env="($anyboot_esp)/EFI/grubenv"
  export anyboot_esp
  export anyboot_env

  # 1) 从 ESP 上的环境块读取状态（saved_entry / next_entry / 任意自定义键）
  if [ -s "${anyboot_env}" ] ; then
    load_env -f "${anyboot_env}"
  fi

  # 2) 用 ESP 的状态决定 default（覆盖 00_header 基于内建 grubenv 的结论）。
  #    若 00_header 已取到一次性项（Linux 侧 grub-reboot / grub-editenv），
  #    它会置 boot_once=true —— 此时不再覆盖，尊重那一次的选择。
  if [ -z "${boot_once}" ] ; then
    if [ "${next_entry}" ] ; then
      set default="${next_entry}"
      set next_entry=
      save_env -f "${anyboot_env}" next_entry
      set boot_once=true
    elif [ "${saved_entry}" ] ; then
      set default="${saved_entry}"
    fi
  fi
fi

# 3) 覆盖 00_header 的 savedefault：把"记住上次选择"写回 ESP。
#    定义在最外层（不嵌在 if 里），ESP 若找不到会自动降级回内建 grubenv。
function savedefault {
  if [ -z "${boot_once}" ] ; then
    saved_entry="${chosen}"
    if [ "${anyboot_env}" ] ; then
      save_env -f "${anyboot_env}" saved_entry
    else
      save_env saved_entry
    fi
  fi
}
"##;

/// 09_anyboot 的安装路径（仅 Linux 有意义）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const GRUB_SCRIPT_PATH: &str = "/etc/grub.d/09_anyboot";

/// 二进制入口（`main.rs`）用这两个：带参数走 CLI，不带参数开 GUI。
pub use cli::{is_cli_invocation as cli_requested, run as run_cli};

const SETTINGS_FILE: &str = "settings.json";
/// 单实例锁文件（放配置目录里，因此 `ANYBOOT_CONFIG_DIR` 也生效 —— 沙箱/多套配置互不干扰）
const INSTANCE_LOCK_FILE: &str = "anyboot.lock";
/// 内置 HTTP 服务的默认监听地址与端口（plan §4.1：默认开启、0.0.0.0:12300）
const DEFAULT_HTTP_BIND: &str = "0.0.0.0";
const DEFAULT_HTTP_PORT: u16 = 12300;
/// 单次请求体上限：超过就只认 header/query（防止拿巨大 body 拖住工作线程）
const MAX_REQUEST_BODY: usize = 8 * 1024;
/// 「最近请求」在内存里保留的条数
const MAX_RECENT_REQUESTS: usize = 20;
/// 通知弹窗的窗口标签
const NOTIFY_WINDOW_LABEL: &str = "notify";
/// 通知弹窗宽度（逻辑像素）
const NOTIFY_WIDTH: f64 = 440.0;
/// 通知弹窗的**标称**高度：建窗口时用它，前端量不出真实内容高度时也用它兜底。
/// 正常路径下前端会量出「刚好兜住文字」的高度再报上来（见 present_notify_window）。
const NOTIFY_HEIGHT: f64 = 240.0;
/// 前端报上来的高度会被夹到这个区间：
/// 太低会把文字裁掉，太高就又变成「底下空一大片」——正是这次要修的东西。
const NOTIFY_MIN_HEIGHT: f64 = 150.0;
const NOTIFY_MAX_HEIGHT: f64 = 460.0;
/// 通知弹窗离工作区右下角的边距（逻辑像素）。
/// 注意用的是**工作区**（已排除任务栏），所以这个边距是「离任务栏 24px」而不是「离屏幕 24px」。
const NOTIFY_MARGIN: f64 = 24.0;
/// **自绘**托盘菜单的窗口标签。
///
/// 为什么托盘菜单要由我们自己画一扇窗：Linux 上菜单原本由宿主按我们导出的 dbusmenu
/// 重新渲染（GtkMenu），点任何一项它都会把菜单收掉 —— 而「点选择框不关菜单」是产品
/// 要求，dbusmenu 协议里又没有任何开关能改这个行为（2026-10-14 实测）。Windows 的
/// TrackPopupMenu / macOS 的 NSMenu 同理。所以菜单生命周期只能归我们所有。
const TRAY_MENU_WINDOW_LABEL: &str = "tray-menu";
/// 远程管理（server-hub）独立窗口标签。主窗口零改动：hub 是一扇常规窗口，
/// 托盘菜单「远程管理…」是它目前唯一的入口（点开常驻，再次点击只是唤起）。
pub(crate) const HUB_WINDOW_LABEL: &str = "server-hub";
/// hub 的三个子窗口：终端 / 文件浏览（SFTP）/ 密钥管理。
/// 每个标签都必须进 capabilities/default.json 的 windows 列表，否则该窗口
/// 所有 invoke/listen 被静默拒（ACL 教训见 capabilities 文件头注释）。
pub(crate) const HUB_TERM_WINDOW_LABEL: &str = "server-term";
pub(crate) const HUB_FILES_WINDOW_LABEL: &str = "server-files";
pub(crate) const HUB_KEYS_WINDOW_LABEL: &str = "server-keys";
/// 菜单宽度（逻辑像素）：固定宽度，三端长得一样，也只有这样「一组里恰好一个 ◉」才好扫。
const TRAY_MENU_WIDTH: f64 = 320.0;
/// 前端报上来的高度夹到这个区间（太低会裁掉文字，太高会把托盘图标顶出屏幕）。
const TRAY_MENU_MIN_HEIGHT: f64 = 40.0;
const TRAY_MENU_MAX_HEIGHT: f64 = 720.0;
/// 前端量不出内容高度时的标称高度（正常路径下前端总会量出来，这只是兜底）
const TRAY_MENU_NOMINAL_HEIGHT: f64 = 420.0;
/// 菜单与托盘图标之间的缝（逻辑像素）。两种方式的手算落位都用
/// （unified 曾交给 positioner 的 TrayCenter，2026-10-15 起两种都自己算坐标）。
const TRAY_MENU_GAP: f64 = 6.0;
/// 刚弹出来这一小段时间里收到的「失焦」是假的（WM 还没把焦点交过来就报了一次），
/// 滤掉它，免得菜单一闪就没。
const TRAY_MENU_BLUR_GRACE_MS: u64 = 250;
/// 鼠标离开主窗口后多久贴边隐藏（太灵敏会误触，太迟钝像卡住）
const AUTO_HIDE_DELAY_MS: u64 = 350;
/// 托盘右键菜单的锚点窗口标签（Windows 专用、classic 方式用，见 `popup_tray_menu`）
#[cfg(target_os = "windows")]
const MENU_ANCHOR_WINDOW_LABEL: &str = "menu-anchor";
/// 锚点窗口尺寸：只是个坐标原点，多大都行（不可见、无边框）
#[cfg(target_os = "windows")]
const MENU_ANCHOR_SIZE: f64 = 8.0;
/// 绿勾位图边长（classic 原生菜单的「当前项」标记，见 check_mark_image）
#[cfg_attr(target_os = "linux", allow(dead_code))]
const CHECK_MARK_SIZE: u32 = 16;
/// 贴边守护线程的轮询周期：光标在不在窗口内、贴没贴边，都靠它扫
const AUTO_HIDE_TICK_MS: u64 = 120;
/// 主窗「还在动」的静默期：距最近一条主窗 Moved 不超过它时，watcher 一律不
/// 累计「离窗时钟」（也就不会贴边）。用户拖窗口、拖白条、WM 自己摆窗口都会
/// 连发 Moved，这期间读到的「光标在窗外」是**拖动过程的中间态**，不是
/// 「用户离开了」。实测（poke-1 复现台：贴边成条 → 抓住白条往工作区里拖）：
/// 松手瞬间窗口从条子展开成 460x900，**176ms 后** watcher 就按拖动途中的
/// 「光标在窗外」把它缩回条子 —— 主窗闪一下（用户报的「主界面会变长」）、
/// GRUB 独立窗 129ms 内翻两次（用户报的「乱闪乱跳」）。
const AUTO_HIDE_MOVE_QUIET_MS: u64 = 300;
/// 位置夹取的静默期：拖动过程中 Moved 连发，期间绝不 set_position（会跟
/// Windows 的拖动循环互相拉扯 = 窗口乱跳）；拖完 250ms 没动静再夹一次。
const CLAMP_DELAY_MS: u64 = 250;
/// 「屏幕中间」模式拖动回中的静默期（毫秒）：拖动全程 Moved 连发，每条都刷新
/// 世代号；最后一条之后静默这么久才真的把窗口收回居中位 —— 拖动期间我们一个
/// `set_position` 都不发（与 `schedule_clamp` 同一套哲学：不去和用户的拖动循环
/// 拉扯，窗口就不抖）。**用户报的「拖着拖着窗口突然弹回中间、像点了按钮」**就是
/// 旧实现「每条 Moved 都当场回中 + 盖上 700ms 回声窗」这条组合拳的观感。
const CENTER_SNAP_DELAY_MS: u64 = 250;
/// 判定「窗口还停在居中位」的容差（物理像素）。Moved 载荷是**窗框**坐标，与
/// `set_position` 请求（客户区语义）恒定差一个标题栏（Muffin 实测 37px）——
/// 取 80px 一次吃掉标题栏错拍与 WM 抖动，于是不再需要「居中后 700ms 一律忽略」
/// 的时间窗：那个时间窗会把用户在这 700ms 里的**真拖动**一起吃掉，松手后既不
/// 回中也不提示（用户报的另一半）。
const CENTER_HOLD_TOLERANCE: i32 = 80;
/// Moved 事件里判定「这是用户在拖、不是我们贴的那次」的容差（物理像素）：
/// 我们自己的 set_position 会在同一坐标上触发 Moved，用户手拖必然超过这个量。
const AUTO_HIDE_TOLERANCE: i32 = 24;
/// WM 重装饰微调的吸收阈值（物理像素）：Muffin 实测每次重装饰把窗口掰 3px
/// （client y 97 → 100）。≤ 此值视为微调、沿上一轮停靠位（dock_axis_position）；
/// 与「自己回声」哨兵的 ±8px 同源。用户拖动必然超过它，不受影响。
const AUTO_HIDE_WM_NUDGE_PX: i32 = 8;
/// undock 重锚的补发次数上限：展开后连续 3 拍（~360ms）没收敛就放弃，
/// 防 WM 不配合时变成永久 set_position 抖动（与 AUTO_HIDE_RESNAP_LIMIT 同思路）。
const AUTO_HIDE_REANCHOR_TRIES: u8 = 6;
/// 贴边后窗口缩成的「缝」的厚度（逻辑像素）：窗口只缩到这条缝，白条是
/// **独立渲染**的白色条（#dock-strip，长度/厚度/透明度全由前端 CSS 控制），
/// 缝要足够容纳白条最大厚度（40px），所以固定取 40 —— 窗口几何与白条
/// 参数彻底解耦，改白条参数不用动窗口。
const DOCK_STRIP_SLOT_PX: f64 = 40.0;
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
/// 不透明度范围（百分比）。0 = 完全透明（窗口还在、还能操作，只是看不见），100 = 不透明。
const MIN_OPACITY: u8 = 0;
const MAX_OPACITY: u8 = 100;
/// 出厂默认窗口不透明度（百分比）：用户定案 90% —— 略微透出桌面，但不影响阅读。
const DEFAULT_OPACITY_PERCENT: u8 = 90;
/// 出厂默认主题：用户定案亮色（暗色改为用户自己选）。
const DEFAULT_THEME: &str = "light";
/// 出厂默认窗口落位：屏幕（工作区）正中间。选 center 而非 last：默认就应该在
/// 视线正前方，而不是「上次拖到哪算哪」。
const DEFAULT_WINDOW_POSITION: &str = "center";
/// 托盘菜单方式出厂默认：classic = 三端各自方式（锚点由宿主/托盘事件给出，
/// 落位按所在显示器工作区手算夹取）；unified = 全局统一自绘窗（落位同为手算，
/// 优先贴图标正下方、放不下翻正上方）。默认选 classic：这是长期实测稳定的形态。
const DEFAULT_TRAY_MENU_STYLE: &str = "classic";
/// 阶段 0 的取证日志：只追加、不参与任何逻辑判断，写在应用配置目录。
/// 里面**永不写入** httpToken（只记前 8 位指纹）。
const TRACE_FILE: &str = "trace.log";
const MAIN_WINDOW_LABEL: &str = "main";
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
static UI_REVISION: AtomicU64 = AtomicU64::new(0);

/// revision 的起始值（纯函数便于单测）：启动时刻的 Unix 毫秒，且不为 0。
fn ui_revision_seed(now_millis: u64) -> u64 {
    now_millis.max(1)
}
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
    /// GRUB 细粒度切换（与上面 rEFInd 那套并列）
    grub: GrubRuntime,
    /// 安装 / 卸载 09_anyboot 的执行过程日志（最近一次）
    script_log: ScriptLogView,
    /// 贴边自动隐藏的运行态：是否已停靠、贴在哪条边、贴住时的坐标。
    /// dock_pos 用来在 Moved 事件里区分「我们自己 set_position 触发的」和
    /// 「用户手拖的」——前者坐标一致（容差内），后者必然偏离。
    auto_hide_docked: bool,
    auto_hide_edge: Option<DockEdge>,
    /// 上一轮贴的边：undock 不清（与 auto_hide_edge 分开）。重贴边时只要没有
    /// 另一条边「严格更近」就沿用 —— 展开的窗口很高，贴左缘时离底缘往往也是
    /// 0px，纯几何重判会在开合几轮后翻成底边贴边（见 redock_edge 的注释）。
    auto_hide_last_edge: Option<DockEdge>,
    auto_hide_dock_pos: (i32, i32),
    /// 本次停靠以来「白条保障」自纠过几次：见 AUTO_HIDE_RESNAP_LIMIT。
    auto_hide_dock_resnaps: u8,
    /// 贴边前的窗口尺寸（物理像素）：贴边时窗口被缩成一条白条厚度
    /// （否则窗口大部分面积压在任务栏/Dock 上——窗口置顶，网页内容会把
    /// 任务栏整个盖住）。展开时按这个尺寸原样恢复。
    auto_hide_restore_size: Option<(u32, u32)>,
    /// 托盘呼出后的宽限期（Unix 毫秒）：呼出瞬间鼠标必然停在托盘上（窗口外），
    /// 没有宽限的话窗口刚弹出来就被 watcher 立刻贴边藏回去 —— 表现就是
    /// 「点了托盘主界面不显示」。宽限期内不累计鼠标离窗时钟。
    auto_hide_grace_until_ms: u64,
    /// 托盘呼出后的「等待进入」标志：呼出展开后鼠标还没进过窗口之前，
    /// watcher 一律不自动贴回。没有它就是死循环：呼出 → 800ms 宽限期一过、
    /// 鼠标还在托盘 → 贴回 → 用户再点托盘 → 再展开 → 再贴回……每轮都有
    /// show/hide + set_decorations 样式切换，两个窗口互相闪烁直到卡死。
    /// 鼠标进过一次窗口（用户真的用过界面）后清除，恢复正常的离窗贴边。
    auto_hide_pending_enter: bool,
    /// undock 重锚计划：展开请求落点 + 已补发次数。GTK 在隐藏态恢复装饰后
    /// frame extents 缓存错拍，首个 set_position 的实际落点会偏低（2x 缩放
    /// 下正好一个标题栏 64px）——watcher 随后几拍读实况、偏了就趁缓存新鲜
    /// 补发一次 set_position（见 undock_reanchor_action）。收敛/放弃/重新
    /// dock 都清掉。
    auto_hide_reanchor: Option<AutoHideReanchor>,
    /// 跨轮记住的展开请求修正量（语义同 `AutoHideReanchor.offset`）：由重锚
    /// 收敛时写回，下一次 undock 的**首次**落位就带修正 —— 不用每轮都先偏
    /// 64px 再弹回来。WM 语义变化（换主题/改缩放）时重锚会重新学。
    auto_hide_undock_offset: (i32, i32),
    /// 标题栏高度（物理像素，窗框顶 − 客户区顶，dock 时窗口还带着装饰、
    /// 可见，读数三端都准）：顶边贴边展开时给标题栏让位用（见 undock_target）。
    /// undock 时现场实测的边距在 Linux 上可能读到 0（GDK frame extents
    /// 缓存未刷新），此时退回这个缓存值。
    auto_hide_caption: i32,
}

/// 主窗口贴边自动隐藏时贴向的屏幕边。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DockEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl DockEdge {
    /// 转成前端用的小写字符串（与 CSS 类名、白条方位一一对应）。
    fn as_str(&self) -> &'static str {
        match self {
            DockEdge::Top => "top",
            DockEdge::Bottom => "bottom",
            DockEdge::Left => "left",
            DockEdge::Right => "right",
        }
    }
}

/// 按窗口与工作区四条边的距离取最近的一条（纯函数，便于单测）。
fn nearest_edge(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    work_x: i32,
    work_y: i32,
    work_w: i32,
    work_h: i32,
) -> DockEdge {
    let distances = [
        (DockEdge::Top, (y - work_y).abs()),
        (DockEdge::Bottom, ((work_y + work_h) - (y + h)).abs()),
        (DockEdge::Left, (x - work_x).abs()),
        (DockEdge::Right, ((work_x + work_w) - (x + w)).abs()),
    ];
    distances
        .into_iter()
        .min_by_key(|(_, distance)| *distance)
        .map(|(edge, _)| edge)
        .unwrap_or(DockEdge::Top)
}

/// 把窗口位置夹回工作区，保证**整个窗口**完整可见（纯函数，便于单测）。
/// 窗口比工作区还大时钉在左上角（max 兜底，至少别越顶）。
fn clamp_to_work(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    work_x: i32,
    work_y: i32,
    work_w: i32,
    work_h: i32,
) -> (i32, i32) {
    let max_x = (work_x + work_w - w).max(work_x);
    let max_y = (work_y + work_h - h).max(work_y);
    (x.clamp(work_x, max_x), y.clamp(work_y, max_y))
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
        // macOS 用 diskutil 卸（它自己挂的，也只能它自己卸干净）；其余用 umount
        #[cfg(target_os = "macos")]
        let result = {
            let mut cmd = command("diskutil");
            cmd.arg("unmount");
            if self.device.as_os_str().is_empty() {
                cmd.arg(&self.mountpoint);
            } else {
                cmd.arg(&self.device);
            }
            cmd.status()
        };
        #[cfg(not(target_os = "macos"))]
        let result = mount_command("umount").arg(&self.mountpoint).status();
        result.and_then(|status| {
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
        command(program)
    } else {
        let mut escalated = command("pkexec");
        escalated.arg(program);
        escalated
    }
}

#[cfg(not(target_os = "linux"))]
fn mount_command(program: &str) -> Command {
    command(program)
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

/// 主窗口「GRUB 启动项」区块的展示数据。
///
/// 这一块和上面的三个系统项是**并列**的两套机制：上面走 rEFInd 的 `PreviousBoot`，
/// 这里走 GRUB 自己的环境块。用户设了 ESP 的分区 UUID 才会出现。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrubView {
    /// 用户填写的分区 UUID 原文（空 = 未配置）
    partuuid: String,
    /// 用户填写的 ESP 目录原文（空 = 没填，只靠 fs-uuid 找）
    dir: String,
    /// 识别出来的 ESP 分区（是不是 FAT32 / fs-uuid / 挂载状态）
    esp: Option<EspInfo>,
    /// 「保持挂载」勾选状态
    keep_mounted: bool,
    /// 本端能不能按分区 UUID 挂载（Windows = false，勾选框要置灰）
    mount_supported: bool,
    /// 本端解析出的 ESP 目录（Windows 是盘符、Linux 是挂载点）
    efi_dir: Option<String>,
    /// 环境块路径（本端）
    env_path: Option<String>,
    /// grub.cfg 副本路径（本端）
    cfg_path: Option<String>,
    /// ESP 上是否已经有环境块文件（没有的话 GRUB 会报 invalid environment block）
    env_exists: bool,
    /// 09_anyboot 是否已安装；非 Linux 恒为 None（安装/卸载按钮只在 Linux 展示）
    installed: Option<bool>,
    /// 上次同步时间（本地时间串）
    synced_at: Option<String>,
    /// 安装 / 卸载 09_anyboot 的执行过程（最近一次，含命令与退出码）
    script_log: ScriptLogView,
    /// 解析出来的启动项（第一版**不过滤**，os-prober 的 Windows/OpenCore 也在内）
    entries: Vec<grub::GrubEntry>,
    /// submenu 分组（id + 标题）：前端据此把子条目挂到组头下面做树形层级
    groups: Vec<grub::GrubGroup>,
    /// 列表展示格式（raw/minimal/simple/medium/detailed）：渲染分支由前端挑
    list_format: String,
    /// 从环境块读回的「上次启动 / 下次启动」
    saved_entry: Option<String>,
    next_entry: Option<String>,
    error: Option<String>,
    /// 给用户的下一步提示
    hint: Option<String>,
}

/// GRUB 功能的**本端运行时状态**。与 rEFInd 那套（`AppState` 的
/// `vars_dir` / `active_mount` / `variables`）完全并列，互不影响。
#[derive(Clone, Default)]
struct GrubRuntime {
    /// 本端解析出的 ESP 根目录
    efi_dir: Option<PathBuf>,
    /// ESP 分区的识别结果（是不是 FAT32 / fs-uuid 从哪来的）
    esp: Option<EspInfo>,
    /// ESP 未挂载时由我们挂上去的挂载记录
    mount: Option<ActiveMount>,
    entries: Vec<grub::GrubEntry>,
    /// submenu 分组（树形展示的组头）
    groups: Vec<grub::GrubGroup>,
    env_path: Option<PathBuf>,
    cfg_path: Option<PathBuf>,
    env_exists: bool,
    saved_entry: Option<String>,
    next_entry: Option<String>,
    installed: Option<bool>,
    synced_at: Option<String>,
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
    SwitchAndPower {
        system: String,
        power: PowerAction,
    },
    /// GRUB 细粒度条目（第二条链路）：selector 是 grub.cfg 里解析出的原文，
    /// submenu 内为 `submenu_id>entry_id`。只写环境块、不重启。
    SwitchGrub(String),
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
            Self::SwitchGrub(selector) => format!("切换 GRUB 启动项为 {selector}"),
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
    /// 监听状态的一句话：主窗口 `#http-state` 与托盘「远程控制」子菜单**共用**同一份文案，
    /// 免得两边各拼各的、说着说着就不一致了。
    state_text: String,
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
    /// GRUB 细粒度切换（与 status 里 rEFInd 那三个系统项并列的第二块）
    grub: GrubView,
    /// 当前生效的 09_anyboot 正文（供界面「查看 / 修改」用）
    grub_script: String,
    /// 主窗口当前是否处于「贴边隐藏」态（前端据此在窗口边界画一条白条）
    docked: bool,
    /// 贴向的屏幕边（top/bottom/left/right）；未贴边时为 null
    dock_edge: Option<String>,
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
    /// 本机标识（32 位十六进制，首次运行生成后持久化）。手机 / 网页客户端靠它认出
    /// 「这几条 IP 其实是同一台电脑」—— 一台电脑可能有多块网卡、多次 DHCP 拿到
    /// 多个 IP，客户端据此把它们归并成一条记录、一个页面，而不是一个 IP 一个标签。
    /// 通过 `GET /health` **免鉴权**返回：客户端还没填 token 时也要能归并。
    machine_id: String,
    /// 远程 / CLI 请求是否允许触发本机授权弹窗（默认开：用户定案「远程可触发
    /// 本机框」—— 远程动作要不要真的提权，由本机这台机器说了算）
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
    /// GRUB 细粒度切换用的 ESP 识别键：**分区 UUID**（GPT PARTUUID，如
    /// `F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3`）。它写在 GPT 分区表里，三端
    /// （Linux lsblk / macOS diskutil / Windows Get-Partition）报的都是同一个
    /// 值。空 = 未配置，界面不展示 GRUB 区块。
    /// 注意与 fs-uuid（FAT 卷序列号，GRUB `search --fs-uuid` 用的 8 位串）是
    /// 两套键：GRUB 脚本里需要的 fs-uuid 在 Linux 上由分区 UUID 现场解析。
    grub_efi_partuuid: String,
    /// 用户在界面上指定的 ESP 目录（形如 `H:\` / `/boot/efi` / `/Volumes/ARCH-EFI`）。
    /// 只在「识别」时用来反查 fs-uuid，并把结果归一化成分区根目录存下来。
    /// fs-uuid 才是权威键，这个只是入口和展示。
    grub_efi_dir: String,
    /// 勾上「挂载 ESP」后保持挂载：每次扫描不再自动卸掉自己挂的那份。
    grub_keep_mounted: bool,
    /// 用户在界面上改过的 09_anyboot 正文（空 = 用内置模板）。
    /// 安装时只替换 `@@ESP_UUID@@`，其余按用户改的来。
    grub_script_override: String,
    /// GRUB 区块的展示方式（四种，全在同一个主窗口里）：
    /// inline（内嵌）/ wide（加宽窗口展示）/ list（列表独立到右栏）/
    /// pane（GRUB 全部内容在右栏）。老配置里的 window 语义由 pane 承接。
    grub_list_view: String,
    /// GRUB 列表的展示格式（前端渲染偏好，纯展示不改数据）：
    /// raw（原始）/ minimal（极简）/ simple（简易）/ medium（中等）/
    /// detailed（复杂）。落盘存归一化后的小写值。
    grub_list_format: String,
    /// 主窗口贴边自动隐藏：鼠标离开后窗口贴到最近的屏幕边缘、只露一条边，
    /// 悬停边条再展开。关掉时窗口保持完整可见。
    main_auto_hide: bool,
    /// 主窗口始终置顶（默认开）：任何情况下都不被别的窗口盖住。关掉后窗口
    /// 回到普通层级，可被覆盖。Windows 上是 HWND_TOPMOST（稳），Linux/macOS
    /// 上 hide→重装饰→show 一圈会把置顶层级丢掉 —— 所以每个 show/落位点都要补钉。
    main_always_on_top: bool,
    /// 贴边自动隐藏的触发距离（逻辑像素）：主窗口离工作区边框不超过这个距离、
    /// 且鼠标离开主窗口/独立窗口超过延迟，才贴边隐藏。窗口在屏幕中央时
    /// 鼠标离开不隐藏（用户明确要求「正中央不触发」+ 距离可自定义）。
    main_auto_hide_edge_distance: i32,
    /// 贴边白条距屏幕（工作区）边界的距离（逻辑像素 0–100，默认 0 = 紧贴）。
    /// 停靠落点沿法线方向离边留这条缝，白条不再压死在屏幕边上。
    main_dock_strip_edge_gap: i32,
    /// 悬浮白条多少毫秒才呼出主窗（0–2000，默认 0 = 当拍立即呼出，既有行为）。
    main_dock_strip_hover_ms: u64,
    /// 贴边白条厚度（逻辑像素，2–40，默认 6）：贴边隐藏时窗口缩成这条的厚度
    main_dock_strip_thickness: i32,
    /// 贴边白条长度（占窗口边长的百分比 10–100，默认 100 = 整条边）
    main_dock_strip_length: i32,
    /// 贴边白条不透明度（百分比 10–100，默认 100）
    main_dock_strip_opacity: i32,
    /// 贴边白条颜色（#rrggbb，默认白色）：调色板选的色调，纯前端 CSS 渲染
    main_dock_strip_color: String,
    /// 贴边热区厚度（逻辑像素 1–100，默认 40 = 既有槽位缝厚，行为不变）：
    /// 停靠态从工作区边往内这一段是「鼠标移入触发呼出」的热区，厚度独立于
    /// 白条视觉厚度 —— 判定是后端按几何算的虚拟矩形，不必是窗口那么厚。
    main_hotzone_thickness: i32,
    /// 热区调试底色（#rrggbb）：配合透明度可视化热区边界，方便调试
    main_hotzone_color: String,
    /// 热区不透明度（百分比 0–100，默认 0 = 全透明 = 默认无颜色，看不见）
    main_hotzone_opacity: i32,
    /// 热区是否画边框（默认关 = 无边框）
    main_hotzone_border: bool,
    /// 热区边框颜色（#rrggbb）
    main_hotzone_border_color: String,
    /// 主题强调色（#rrggbb，默认内置蓝 #2f7fe0）：调色板选的色调，
    /// 前端据此派生按钮/链接/悬停色
    accent_color: String,
    /// 主窗口固定显示位置模式：free=用户自由拖动（默认，保持现状）；
    /// center=每次显示都居中；last=记住上次手动摆放的位置。
    main_window_position: String,
    /// 托盘菜单方式：classic=三端各用系统原生托盘菜单（三端自绘之前的原始方案，
    /// 默认）；unified=全局统一 —— 三端右键都开**同一扇自绘菜单窗**（Win/mac 托盘
    /// 事件直开；Linux 自研 SNI 按 WM 分流：GNOME 导出空 dbusmenu、宿主弹窗打开
    /// 瞬间弹自绘窗，Cinnamon/KDE 导出全量 dbusmenu 由宿主菜单渲染，2026-10-15
    /// 定案）。两种形态的托盘接线在启动时二选一，切换后重启应用生效。
    tray_menu_style: String,
    /// 「上次位置」模式记忆的落点（物理像素；未设定为 None，回退到居中）。
    main_window_last: Option<(i32, i32)>,
    /// 主窗口固定显示器：`auto`（不固定，默认）或某个显示器的名字（tauri
    /// Monitor::name()，如 `DP-1` / `\\.\DISPLAY1` / 「内建视网膜显示屏」）。
    /// 固定后主窗只在这台显示器里活动：居中/呼出/落位/贴边都落在它的工作区，
    /// 拖到别的显示器会被夹回来。显示器被拔掉时回落既有挑屏逻辑（窗口总得
    /// 有地方显示），设置值保留 —— 插回来继续生效。
    main_fixed_monitor: String,
    /// 锁定窗口大小：锁定后窗口不可拖拽缩放、不可最大化，前端宽高滑条同步停用。
    window_size_locked: bool,
    /// 记忆的窗口尺寸（逻辑像素）。**0 = 还没记过**（serde 缺字段 / 全新配置），
    /// 此时启动按展示方式给标称宽度（460/932）、高度按**整屏高度的 2/3**
    /// （见 default_window_height）。
    /// 任何一次尺寸落位都会写成真实值（clamp 下限 360/560，0 不会存活）。
    /// 不能把默认填成 460/900：serde(default) 会给**所有老配置**伪造一个
    /// 「已记忆 460x900」，用户拖过的尺寸在重启后被悄悄拽回去。
    window_width: u32,
    window_height: u32,
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
            accent_color: "#2f7fe0".to_string(),
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
        command("/usr/bin/codesign")
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
    /// 当前可用的显示器名列表：设置页「固定显示器」下拉与托盘同名子菜单
    /// 都用它当选项（与 Settings.mainFixedMonitor 的取值同一来源）。
    main_monitor_options: Vec<String>,
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
            // 选项列表依赖 AppHandle，由 view_settings 填（见那边）
            main_monitor_options: Vec::new(),
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
    /// 文件系统 UUID —— GRUB `search --fs-uuid` 比的那串（**不是**识别键）。
    /// 老代码的 lsblk 调用没要这一列，`Option` 让缺失列也能反序列化。
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    uuid: Option<String>,
    /// 分区 UUID（GPT PARTUUID）—— 识别 ESP 的权威键，三端同值。
    /// 只有 Linux 的定位函数读它（lsblk 输出列），其余平台缺列也能反序列化。
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    partuuid: Option<String>,
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

    let output = command("lsblk")
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
    // GRUB 那条链路跟着一起刷新：Linux 端顺带把 /boot/grub 的两份数据同步到 ESP，
    // 其它端就是重新解析 ESP 上的副本。用户要的「每次启动软件自动同步一次」在这里。
    // **硬性条件（用户定案）**：ESP 必须已经由当前系统挂载在 /boot 下（fstab
    // 统一方案）—— 没挂就整条跳过：不自动挂载、不 pkexec、不写盘。手动链路
    // （识别/挂载/同步按钮）不受此限。
    refresh_grub(app);
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

/// 本机主动切换系统（主窗口按钮 / 托盘菜单）的统一入口。
///
/// 先作废「挂着的远程请求」再切换：网页 / 手机端选系统会形成一条 pending（要等
/// 倒计时到点才执行），本机用户在这段时间里自己点了另一个系统 —— 本机的操作是更
/// 明确、更权威的意图，绝不能被那条到点的请求盖回去（否则用户看到的是「我明明
/// 选了 A，几秒后自己变回 B」）。**只在本机入口做这件事**：远程请求到点执行时也
/// 会走 switch_and_update，从那里清理就等于自己把自己取消掉。
fn switch_system_locally(app: &AppHandle, system: &str) -> AppStatus {
    if let Some(pending) = pending_snapshot() {
        trace(
            app,
            "remote",
            &format!(
                "本机切换到 {system}：挂着的远程请求 {}（{}）作废",
                pending.id, pending.summary
            ),
        );
        if let Err(error) = cancel_pending_now(app) {
            // 作废失败不该拦住切换本身（切换才是用户真正要的事）
            trace(app, "remote", &format!("作废挂起请求失败（不影响切换）: {error}"));
        }
    }
    switch_and_update(app, system)
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

// ==================== classic（各自方式）的原生菜单素材 ====================
//
// 托盘菜单的「classic」形态 = 三端各用系统原生菜单（三端自绘之前的原始方案）。
// 原生菜单改不了文字颜色/字重，所以「哪个系统是选中的」只能用图形表达：
// 选了的那一行带绿勾、其余行不带 —— 勾占的是菜单**独立的位图列**，
// 不占用文字，系统的名字因此天然左对齐（以前用「● 」前缀，名字会被顶偏一格）。

/// 绿勾位图（classic 原生菜单的列表类条目用）。
///
/// 尺寸固定 16×16：muda 在 Windows 上就是把图标按 16×16 画进位图列的。
/// Linux 已不走 tauri 托盘（classic 也由 linux_sni 承载），只剩 Win/mac 用；
/// 单测仍跑，故 allow 而非 cfg 掉。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn check_mark_image() -> Image<'static> {
    use std::sync::OnceLock;
    static IMAGE: OnceLock<Image<'static>> = OnceLock::new();
    IMAGE.get_or_init(build_check_mark_image).clone()
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn build_check_mark_image() -> Image<'static> {
    Image::new_owned(check_mark_rgba(), CHECK_MARK_SIZE, CHECK_MARK_SIZE)
}

/// 绿勾的像素（纯函数，便于单测；tauri 的 Image 借不出来，所以拆开）。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn check_mark_rgba() -> Vec<u8> {
    /// 与主界面 --ok 同一个绿（#46c46e）
    const COLOR: [u8; 3] = [0x46, 0xc4, 0x6e];
    /// 笔画宽度
    const STROKE: f32 = 2.2;
    /// 对勾折线：左下 → 谷底 → 右上
    const LINE: [(f32, f32); 3] = [(3.2, 8.4), (6.6, 11.8), (12.8, 4.4)];

    let size = CHECK_MARK_SIZE;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let distance = LINE
                .windows(2)
                .map(|segment| distance_to_segment(px, py, segment[0], segment[1]))
                .fold(f32::MAX, f32::min);
            // 半个像素的过渡带：边缘不平滑的话 16px 的勾会全是锯齿
            let coverage = (STROKE / 2.0 + 0.5 - distance).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let offset = ((y * size + x) * 4) as usize;
            rgba[offset..offset + 3].copy_from_slice(&COLOR);
            rgba[offset + 3] = (coverage * 255.0).round() as u8;
        }
    }
    rgba
}

/// 点到线段的距离（画对勾用，纯函数）。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn distance_to_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (ax, ay) = a;
    let (dx, dy) = (b.0 - ax, b.1 - ay);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0)
    };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
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

// ==================== WebKitGTK × NVIDIA：窗口一片空白 ====================
//
// 上游已知故障（tauri-apps/tauri#9394 / #10702、WebKitGTK Bug 280210）：WebKitGTK 的
// **DMA-BUF 渲染器**跟 NVIDIA 专有驱动谈不拢缓冲格式，构造不出完整 framebuffer 就
// 静默放弃 —— 窗口照常打开、托盘照常有、日志一行不报，就是内容一片白。
// Intel/Mesa 走同一条 DMA-BUF 路径没问题，所以核显模式（或纯集显的机器）看到的是好的，
// 「独显才白」这个现象就出在这里。
//
// 修法不是一个开关通吃，按会话类型分几种：
//   X11                      → 让 WebKit 把帧走**共享内存**交给窗口系统
//                              （WEBKIT_DMABUF_RENDERER_FORCE_SHM=1；为什么是它、
//                              以及试错过程见 X11_GPU_SWITCHES 的注释）
//   Wayland(Hyprland)        → 关 DMA-BUF 渲染器（缺 acquire point，合成器直接掐连接）
//   Wayland(KWin 一类严格同步) → 关 NVIDIA 显式同步（否则应用可能起不来）
//   Wayland(其它 + egl-wayland2) → 不用动（驱动 ≥560 自带，这条路径本来是好的）
//
// 一个必须知道的背景：WebKitGTK **2.46 起换了渲染架构**（Skia 取代 Cairo 做 2D）、
// **2.54 起连合成器也换成 Skia，并把「只合成损伤区域」打开成默认**。老一代靠
// 「关掉某个加速开关」的招数到新版上会失效、甚至反过来有害，改这一块之前先去
// 查上面那几个上游 issue，别照着旧帖子抄。

/// 一次会话里 GDK / WebKitGTK 实际选中的显示后端。
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LinuxDisplayBackend {
    X11,
    Wayland,
    /// 线索全无（既没有 `GDK_BACKEND` 也没有会话变量）：按 X11 处理，理由见选择表。
    Unknown,
}

/// 用户已经自己配过的 WebKit 开关：设过任何一个就完全不插手。
///
/// 也包含显式设成 `0` 的情况 —— 那种用户多半跟这个问题搏斗过，
/// 覆盖掉他自己的配置比什么都不做更糟。
#[cfg(target_os = "linux")]
const WEBKIT_ENV_SWITCHES: [&str; 4] = [
    "WEBKIT_DISABLE_DMABUF_RENDERER",
    "__NV_DISABLE_EXPLICIT_SYNC",
    "WEBKIT_DISABLE_COMPOSITING_MODE",
    "WEBKIT_DMABUF_RENDERER_FORCE_SHM",
];

/// 从环境变量推断后端（纯函数，便于单测）。
///
/// 顺序照抄 GDK 自己的选择逻辑：`GDK_BACKEND` 是硬指定、优先于一切，它是个逗号分隔的
/// 候选表，第一个认得出的 x11/wayland 算数（`broadway` 这类认不出的项跳过）；
/// 没设才依次看 `XDG_SESSION_TYPE`、`WAYLAND_DISPLAY`、`DISPLAY`。
/// 注意 XWayland 下 `DISPLAY` 也在，所以 Wayland 的判定必须排在它前面。
#[cfg(target_os = "linux")]
fn linux_display_backend(
    gdk_backend: Option<&str>,
    session_type: Option<&str>,
    wayland_display: Option<&str>,
    display: Option<&str>,
) -> LinuxDisplayBackend {
    if let Some(candidates) = gdk_backend {
        for candidate in candidates.split(',').map(str::trim) {
            match candidate.to_ascii_lowercase().as_str() {
                "x11" => return LinuxDisplayBackend::X11,
                "wayland" => return LinuxDisplayBackend::Wayland,
                _ => continue,
            }
        }
    }
    match session_type.map(|value| value.trim().to_ascii_lowercase()) {
        Some(kind) if kind == "x11" => return LinuxDisplayBackend::X11,
        Some(kind) if kind == "wayland" => return LinuxDisplayBackend::Wayland,
        _ => {}
    }
    if wayland_display.is_some_and(|value| !value.is_empty()) {
        return LinuxDisplayBackend::Wayland;
    }
    if display.is_some_and(|value| !value.is_empty()) {
        return LinuxDisplayBackend::X11;
    }
    LinuxDisplayBackend::Unknown
}

/// 从 `/proc/driver/nvidia/version` 的内容里取驱动主版本号（纯函数，便于单测）。
///
/// 文件内容形如：
/// ```text
/// NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.71.09  Release Build  (root@arch)
/// GCC version:  gcc 16.2.1 20260810 (GCC)
/// ```
/// 「第一个带点的数字词」就是版本号，要的是它的主版本（`egl-wayland2` 看 ≥560）。
/// **必须锚定 `NVRM version:` 这一行**：下面那行的 `gcc 16.2.1` 也是带点的数字词，
/// 不锚定就会把编译器版本当成驱动版本。
#[cfg(target_os = "linux")]
fn nvidia_major_from_version_text(text: &str) -> Option<u32> {
    let banner = text
        .lines()
        .find(|line| line.trim_start().starts_with("NVRM version:"))?;
    banner.split_whitespace().find_map(|word| {
        let (major, rest) = word.split_once('.')?;
        // 至少得是「主.次」两段数字，`x86_64` 这种词不算
        if rest.is_empty() || !rest.starts_with(|character: char| character.is_ascii_digit()) {
            return None;
        }
        major.parse::<u32>().ok()
    })
}

/// NVIDIA 专有驱动的主版本号；没装 / 没加载（纯核显模式、nouveau）返回 `None`。
///
/// `/proc/driver/nvidia/version` 只有 NVIDIA 自己的驱动会建（nouveau 与 Mesa 都不建），
/// 所以「这文件在」≈「这次会话的渲染有 NVIDIA 参与」，也正是要加开关的时刻；
/// 反过来，切到纯核显模式后这个文件就没了，开关自动不加，性能也不会白白吃亏。
#[cfg(target_os = "linux")]
fn nvidia_driver_major() -> Option<u32> {
    let text = std::fs::read_to_string("/proc/driver/nvidia/version").ok()?;
    nvidia_major_from_version_text(&text)
}

/// 让 WebKit 的 DMA-BUF 渲染器彻底不用 DMA-BUF：Wayland(Hyprland) 上缺 acquire point
/// 会被合成器当协议错误掐断连接，那里必须把这条路堵死。
///
/// **只给 Hyprland 用**。2.52 起这个变量**不再回退到共享内存**，换别的会话用会把传输
/// 模式清空（X11 上正是它把「全白」变成「只有鼠标扫过的地方才显示」的，见
/// `X11_GPU_SWITCHES` 的注释）。Hyprland 若也反馈空白/崩溃，同样该换 `FORCE_SHM`。
#[cfg(target_os = "linux")]
const DMABUF_RENDERER_OFF: &[(&str, &str)] = &[("WEBKIT_DISABLE_DMABUF_RENDERER", "1")];

/// 关掉 NVIDIA 显式同步：严格的 Wayland 合成器上会让应用根本起不来。
#[cfg(target_os = "linux")]
const NVIDIA_EXPLICIT_SYNC_OFF: &[(&str, &str)] = &[("__NV_DISABLE_EXPLICIT_SYNC", "1")];

/// X11（以及后端认不出来的情况）上给 WebKit 的开关：**只有这一个**。
///
/// 语义：让 WebKit 把合成好的帧以**共享内存**交给窗口系统，而不是 DMA-BUF / GPU 缓冲。
/// NVIDIA 专有驱动的 X11/EGL 路径收不下这些 GPU 缓冲，WebKit 就画不出来 ——
/// 表现可以是全白，也可以是「内容能加载、但只有鼠标扫过的地方才显示」。
///
/// 换过两轮才认准，弯路记在这儿（别再回头试）：
/// - `WEBKIT_DISABLE_DMABUF_RENDERER=1`：**2.52 起不再回退到共享内存**，只会把传输模式
///   清空。本机实测（2.54.1 + 615 驱动）表现就是「只有鼠标扫过才显示」；上游还报告它会让
///   `AcceleratedBackingStore::create()` 返回 null 而崩。
/// - `WEBKIT_DISABLE_COMPOSITING_MODE=1`：**2.54 已失效** —— 非合成渲染器在 2.54 同样走
///   AcceleratedBackingStore，加它症状一字不变（本机实测）。
///
/// 依据：WebKit Bug 324551（同一台 NVIDIA 机器上，唯一能把页面画出来的是 `FORCE_SHM`，
/// 且明说 `DISABLE_COMPOSITING_MODE` 在 2.54 已不避开该问题）、WebKit PR #20869（变量由来）、
/// Buzz 桌面端 #3271/#3654（检测到 NVIDIA 就自动设它，并明确不要用 `DISABLE_DMABUF_RENDERER`）、
/// A2Tools DPS Meter 的 Linux 说明（同症状、同修法）。
#[cfg(target_os = "linux")]
const X11_GPU_SWITCHES: &[(&str, &str)] = &[("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1")];

/// 一条都不用加（例如 Wayland 上驱动自带好用的 egl-wayland2）。
#[cfg(target_os = "linux")]
const NO_GPU_SWITCHES: &[(&str, &str)] = &[];

/// 这次会话该给 WebKit 加哪些开关（纯函数，便于单测）：可能一个都不加。
#[cfg(target_os = "linux")]
fn linux_gpu_env_overrides(
    backend: LinuxDisplayBackend,
    desktop: Option<&str>,
    driver_major: u32,
) -> &'static [(&'static str, &'static str)] {
    let desktop = desktop.unwrap_or("").to_ascii_lowercase();
    match backend {
        // 白窗 / 内容看不见都出在这条路上。后端认不出来时也按 X11 处理：
        // 画不出来比性能掉一点点严重得多。
        LinuxDisplayBackend::X11 | LinuxDisplayBackend::Unknown => X11_GPU_SWITCHES,
        // 缺 acquire point 会被合成器当协议错误掐断连接，DMA-BUF 必须关
        LinuxDisplayBackend::Wayland if desktop.contains("hyprland") => DMABUF_RENDERER_OFF,
        // KWin 一类严格同步的合成器：关掉 NVIDIA 显式同步就没事了
        LinuxDisplayBackend::Wayland if desktop.contains("kde") || desktop.contains("plasma") => {
            NVIDIA_EXPLICIT_SYNC_OFF
        }
        // 驱动 ≥560 自带能用的 egl-wayland2，这条路径本来是好的，别动它
        LinuxDisplayBackend::Wayland if driver_major >= 560 => NO_GPU_SWITCHES,
        // 更老的 egl-wayland：不关显式同步可能连不上
        LinuxDisplayBackend::Wayland => NVIDIA_EXPLICIT_SYNC_OFF,
    }
}

/// 用户是否已经自己配过 WebKit 的这些开关（纯函数，便于单测）。
#[cfg(target_os = "linux")]
fn webkit_env_already_configured(probe: impl Fn(&str) -> bool) -> bool {
    WEBKIT_ENV_SWITCHES.iter().any(|name| probe(name))
}

/// 按会话类型给 WebKit 塞进与 NVIDIA 相容的开关。
///
/// **必须在 GTK / WebKit 起来之前调用**：WebKit 是在建 WebView 的那一刻读这些变量的，
/// 之后再设就没用了。所以调用点在 `run()` 的最前面，早于 `tauri::Builder`。
#[cfg(target_os = "linux")]
fn apply_linux_gpu_workarounds() {
    if webkit_env_already_configured(|name| std::env::var_os(name).is_some()) {
        return;
    }
    // 没有 NVIDIA 驱动（核显模式 / nouveau / Mesa）：这条路径本来就没问题，什么都别动
    let Some(driver_major) = nvidia_driver_major() else {
        return;
    };
    let backend = linux_display_backend(
        std::env::var("GDK_BACKEND").ok().as_deref(),
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("DISPLAY").ok().as_deref(),
    );
    let overrides = linux_gpu_env_overrides(
        backend,
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
        driver_major,
    );
    if overrides.is_empty() {
        return;
    }
    for (name, value) in overrides {
        std::env::set_var(name, value);
    }
    // 白窗这类问题现场很难复现，ANYBOOT_TRACE=1 时留一行痕
    // （启动期还没有日志文件可写，所以走 stderr，和 trace 的口径一致）
    if env_flag_from(std::env::var("ANYBOOT_TRACE").ok().as_deref()) == Some(true) {
        let applied = overrides
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        eprintln!("anyboot: NVIDIA 驱动 {driver_major} / {backend:?}，已设置 {applied}");
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
    let line = format!("{}ms [{}] {message}\n", unix_millis(), category);
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

/// 「已提交电源动作」之后忙闸最多挂多久（毫秒）。
///
/// 忙闸的用意是「别重复提交」，但**提交成功 ≠ 动作真的发生**：`systemctl suspend`
/// 在 logind 那边只是**排队**，命令立刻返回 0；随后被 inhibitor 拦下、或内核
/// suspend 失败立刻 resume，机器照样活着 —— 而闸一直挂着，三个电源项就永远置灰、
/// 点了毫无反应（用户报的「三个电源按钮都没用」就是这么来的：点过睡眠/被拦下的重启
/// 之后，后面每一次点击都被静默吞掉，只能重启应用才恢复）。
/// 真关机/重启了的话没人会看菜单，放掉闸没有任何代价。
const POWER_RELEASE_MS: u64 = 15_000;

/// 放闸的世代号：新动作一来就把先前排队的放闸作废（与 schedule_clamp /
/// schedule_center_snap 同一套「世代号 + 静默期」写法）。
static POWER_RELEASE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// 抢占一个电源动作；已经有动作在跑时返回 false（替代原来的静态原子标志）。
fn try_begin_power(app: &AppHandle, action: &str) -> bool {
    let acquired = {
        let state = app.state::<Mutex<BusyState>>();
        let mut busy = state.lock().expect("busy mutex poisoned");
        if busy.power.is_some() {
            false
        } else {
            busy.power = Some(action.to_string());
            // 新动作一来就把先前排队的放闸作废。**必须在这把锁里推高**：放闸那边
            // 也是在同一把锁里比对世代号（见 schedule_power_release），两边同锁才
            // 不会出现「新动作刚置上 busy、世代号还没换，旧放闸把它的闸清掉」的缝。
            POWER_RELEASE_GENERATION.fetch_add(1, Ordering::SeqCst);
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
        // 被忙闸挡掉的点击**也必须留痕**：从前这里只写 trace，而 trace 默认是关的，
        // 于是「点了没反应」在现场等于彻底无声 —— 正是用户这次报障最难查的地方。
        record_action(
            app,
            "电源操作",
            false,
            "上一个电源动作还没结束，这次点击已忽略；若菜单里的电源项一直是灰的，就是它卡住了",
        );
        publish_ui(app);
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

/// 提交成功之后**延时放闸**（见 POWER_RELEASE_MS）。
///
/// 到点时若世代号还是最新、且忙闸确实还挂着，就放掉 —— 说明「命令返回成功」之后
/// 系统并没有真的关机/重启/睡着，别让三个电源项永远灰着。
/// 线程睡眠走 CLOCK_MONOTONIC（不含 suspend 时间），所以真睡了一觉回来之后，
/// 剩下的静默期照样会走完、闸照样放掉 —— 正是我们要的。
fn schedule_power_release(app: &AppHandle) {
    let generation = POWER_RELEASE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(POWER_RELEASE_MS));
        // 「世代号还是不是最新」与「清掉 busy」必须在**同一把锁**里做：
        // 否则新动作刚在 try_begin_power 里置上 busy、世代号还没换的那一刻，
        // 这里会把它的闸放掉（新动作就没人看了）。
        {
            let state = app.state::<Mutex<BusyState>>();
            let mut busy = state.lock().expect("busy mutex poisoned");
            if POWER_RELEASE_GENERATION.load(Ordering::SeqCst) != generation
                || busy.power.is_none()
            {
                return;
            }
            busy.power = None;
        }
        trace(&app, "power", "提交后系统仍在：放掉忙闸，电源项恢复可用");
        record_action(
            &app,
            "电源操作",
            false,
            "已提交的动作在 15 秒内没有关机/重启/睡眠，电源项已恢复可用（真在关机中就不用管）",
        );
        publish_ui(&app);
    });
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
        let last_error = runtime.last_error.clone();
        let token_set = !settings.settings.http_token.trim().is_empty();
        HttpView {
            enabled: settings.settings.http_enabled,
            bind: settings.settings.http_bind.clone(),
            port: settings.settings.http_port,
            listening,
            state_text: http_state_text(
                listening,
                addr.as_deref(),
                last_error.as_deref(),
                token_set,
            ),
            addr,
            last_error,
            port_occupant: runtime.occupant.as_ref().map(PortOccupant::to_view),
            token_set,
            token_hint: token_hint(&settings.settings.http_token),
            requests: runtime.requests.clone(),
        }
    };
    // GRUB 那块：先算好（下面 `settings` 会被整体移进快照，不能再借）
    let grub = grub_snapshot(app, &settings.settings);
    // 09_anyboot 模板里嵌的是 fs-uuid（GRUB 的 search --fs-uuid 只认它）。
    // fs-uuid 不再存设置 —— 用最近一次扫描探到的（Linux 上 findmnt/引导扇区
    // 都报得出来）；探不到就留占位符，安装时（grub_install_script）会现场解析。
    let grub_script = grub_script_content(
        grub_template(&settings.settings),
        &grub
            .esp
            .as_ref()
            .and_then(|esp| esp.uuid.clone())
            .unwrap_or_else(|| "@@ESP_UUID@@".to_string()),
    );
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
        grub,
        grub_script,
        docked: {
            let state = app.state::<Mutex<AppState>>();
            state
                .lock()
                .map(|state| state.auto_hide_docked)
                .unwrap_or(false)
        },
        dock_edge: {
            let state = app.state::<Mutex<AppState>>();
            state
                .lock()
                .ok()
                .and_then(|state| state.auto_hide_edge.map(|edge| edge.as_str().to_string()))
        },
    }
}

/// **唯一**的发布入口：先 +1 revision，再用同一份快照重建托盘菜单 + 通知主窗口。
///
/// 主窗口和托盘菜单从此不存在「各读各的」，也不需要三个补丁式事件。
fn publish_ui(app: &AppHandle) -> UiState {
    UI_REVISION.fetch_add(1, Ordering::SeqCst);
    let state = ui_state(app);
    // 移动端没有托盘/菜单，跳过重建（快照本身照发）
    #[cfg(desktop)]
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
    // 网页端 / 手机端的推送通道：没人连就没有任何开销（见 websocket_publish）
    websocket_publish(&state);
    state
}

// ==================== WebSocket 推送（网页端 / 手机端的 /api/ws） ====================
//
// 传输与握手在 http 模块，这里只管「谁订阅了」和「推什么」。推的内容与
// `/api/ui-state` 完全同源（都是 get_ui_state 的那份快照），所以两条路不会分叉。

/// 已连上的推送客户端：id → 发送端。
///
/// 为什么中间隔一条通道、而不是直接把 socket 存在这儿：连接由 http 层的线程抱着
/// （它的生命周期、读写、关闭都在那边），两边只交接「快照字符串」。发送端一 drop，
/// 对面 `recv_timeout` 立刻拿到 `Disconnected` 收尾退出 —— 收服务、换 token 时就
/// 靠这个把它们请走（见 http::RunningServer::stop）。
static WS_CLIENTS: Mutex<Vec<(u64, SyncSender<String>)>> = Mutex::new(Vec::new());
/// 推送客户端的发号器，注销时靠它找到自己那一条。
static WS_NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// 每个客户端的待发队列容量。
///
/// 故意很小：快照是**完整状态**而不是增量，中间几拍丢了不影响正确性（下一拍补齐）。
/// 队列满说明这个客户端收得比我们发得慢，这时**丢掉这一拍**，绝不阻塞 `publish_ui`
/// —— 后者会把主窗口自己的重绘一起拖住。
const WS_QUEUE: usize = 8;

/// 握手前的一次性鉴权 + 首帧内容（当前完整快照）。失败时回一个普通 HTTP 错误。
pub(crate) fn websocket_hello(app: &AppHandle, token: Option<&str>) -> Result<String, RemoteReply> {
    let settings = stored_settings(app);
    if !token_matches(&settings.http_token, token.unwrap_or_default()) {
        return Err(RemoteReply::error(401, "invalid_token", "token 缺失或无效"));
    }
    Ok(ui_state_json(app))
}

/// 当前快照的 JSON 串（`/api/ui-state`、WS 首帧、WS 广播共用同一份）。
fn ui_state_json(app: &AppHandle) -> String {
    serde_json::to_string(&get_ui_state(app.clone())).unwrap_or_else(|_| "{}".to_string())
}

/// 登记一个推送客户端，返回 (id, 收件口)。
pub(crate) fn websocket_subscribe() -> (u64, Receiver<String>) {
    let (sender, receiver) = sync_channel(WS_QUEUE);
    let id = WS_NEXT_ID.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut clients) = WS_CLIENTS.lock() {
        clients.push((id, sender));
    }
    (id, receiver)
}

/// 当前推送客户端数量（只有单测要用；正常路径靠 `websocket_publish` 里的
/// `is_empty()` 短路，不必单独查一次）。
#[cfg(test)]
fn websocket_client_count() -> usize {
    WS_CLIENTS.lock().map(|clients| clients.len()).unwrap_or(0)
}

/// 注销一个推送客户端（连接线程退出时调）。
pub(crate) fn websocket_unsubscribe(id: u64) {
    if let Ok(mut clients) = WS_CLIENTS.lock() {
        clients.retain(|(client, _)| *client != id);
    }
}

/// 请所有推送客户端离开（收服务 / 换 token 时调）。返回后对面线程很快会自己退出。
pub(crate) fn websocket_disconnect_all() {
    if let Ok(mut clients) = WS_CLIENTS.lock() {
        clients.clear();
    }
}

/// 把新快照推给所有客户端。**没人订阅时连序列化都不做**。
fn websocket_publish<T: Serialize>(state: &T) {
    let Ok(mut clients) = WS_CLIENTS.lock() else {
        return;
    };
    if clients.is_empty() {
        return;
    }
    let Ok(payload) = serde_json::to_string(state) else {
        return;
    };
    clients.retain(|(_, sender)| match sender.try_send(payload.clone()) {
        Ok(()) => true,
        // 队列满（对端收得慢）：丢掉这一拍，连接照留
        Err(TrySendError::Full(_)) => true,
        // 对面线程已经退了：顺手清掉
        Err(TrySendError::Disconnected(_)) => false,
    });
}

/// 推送通道**连上时**记一条请求日志 —— 「基本设置 → 远程控制」的最近请求里
/// 一眼能看出推送到底有没有连上（排查手机端为什么在轮询时最有用）。
///
/// **断开只写 trace.log，不写最近请求**：断开是「对方关页面 / 切网」造成的，
/// 时刻随机，摆进最近请求里只会让人以为「刚刚有人动了什么」；链路抖动时更糟 ——
/// 一次抖动就是两条，20 条的历史马上被这种噪音顶光，真正要紧的切换/电源记录
/// 反而被挤出去。排查时看 trace.log（`[http-access] /api/ws -> closed`）就够。
pub(crate) fn record_websocket_event(app: &AppHandle, source: &str, connected: bool) {
    if !connected {
        trace(app, "http-access", &format!("{source} /api/ws -> closed 客户端离开"));
        return;
    }
    record_request_log(
        app,
        Channel::Http,
        source,
        "/api/ws",
        "推送通道已连接",
        "executed",
        "WebSocket（快照推送）",
    );
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

/// 主窗口固定位置模式归一（纯函数）：free / center / last，其余回落 free。
fn normalize_window_position(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "center" => "center",
        "last" => "last",
        _ => "free",
    }
}

/// 托盘菜单方式归一（纯函数）：不认识的值一律回 classic（出厂默认形态）。
fn normalize_tray_menu_style(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "unified" => "unified",
        _ => "classic",
    }
}

/// 固定显示器设置归一（纯函数）：空 / 空白 = auto（不固定）。
fn normalize_fixed_monitor(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "auto".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 在候选显示器名里挑「固定显示器」的下标（纯函数）：名字精确匹配；
/// 找不到（显示器被拔了）返回 None，调用方回落既有挑屏逻辑 —— 窗口
/// 总得有地方显示，绝不能因为配置指向了一块不存在的屏就没了落脚点。
fn fixed_monitor_index(names: &[&str], fixed: &str) -> Option<usize> {
    let fixed = normalize_fixed_monitor(fixed);
    if fixed == "auto" {
        return None;
    }
    names.iter().position(|name| *name == fixed)
}

/// GRUB 区块展示方式归一（纯函数）：inline / wide / list / pane，
/// 其余一律回落 inline。老配置里的 `window`（独立窗口）已经演进成
/// 同一窗口里的「独立区域」，语义由 `pane` 承接 —— 老用户无感迁移。
fn normalize_grub_list_view(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "wide" => "wide",
        "list" => "list",
        "window" | "pane" => "pane",
        _ => "inline",
    }
}

/// GRUB 列表展示格式归一（纯函数）：raw（原始）/ minimal（极简）/
/// simple（简易）/ medium（中等）/ detailed（复杂），其余一律回落 minimal
/// （用户定案：默认极简）。
fn normalize_grub_list_format(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "raw" => "raw",
        "simple" => "simple",
        "medium" => "medium",
        "detailed" => "detailed",
        _ => "minimal",
    }
}

/// 展示方式对应的窗口宽度（逻辑像素）：内嵌 = 单栏 460；
/// 加宽 / 列表独立 / GRUB 独立 = 932 = 左栏 460 + 中缝 12 + 右栏 460
/// （和以前「主窗 + GRUB 独立窗」并排占的总宽度一致，前端两栏等宽）。
fn grub_layout_width(mode: &str) -> f64 {
    if mode == "inline" {
        460.0
    } else {
        932.0
    }
}

/// 窗口尺寸的合法范围（逻辑像素）：下限是前端 CSS 的 min-width + 内容最低可用，
/// 上限防手滑把窗口拖到比屏幕还大。滑条范围与它一致。
const WINDOW_MIN_SIZE: (u32, u32) = (360, 560);
const WINDOW_MAX_SIZE: (u32, u32) = (1600, 1200);
/// 拿不到显示器时的兜底高度；正常路径是「整屏高度的 2/3」（见 default_window_height）。
const DEFAULT_WINDOW_HEIGHT: u32 = 900;

/// 默认窗口高度 = **整屏**高度的 2/3（纯函数，便于单测）。
///
/// 用户定案 2026-10-13：由「占屏幕一半」改为「占屏幕高度的 2/3」，且基准是
/// **整屏**（monitor.size）不是工作区 —— 基准用工作区会在有任务栏的机器上
/// 少一截（1920x1080 + 底栏 → 652 而不是 720），窗口「已经在重置目标上」，
/// 重置按钮点起来就像失灵（实测踩过）。高度超工作区没关系：居中落位走
/// clamp_to_work，窗口仍会被摆进工作区。夹回合法区间 —— 1080p 屏上 2/3 是
/// 720；特别矮的屏（如 800）算出 533 会落到最小高度 560。
fn default_window_height_from_screen(screen_height: u32) -> u32 {
    (screen_height * 2 / 3).clamp(WINDOW_MIN_SIZE.1, WINDOW_MAX_SIZE.1)
}

/// 当前显示器的默认窗口高度：**整屏**高度（逻辑像素）的 2/3。
///
/// 窗口还没映射时 `current_monitor()` 可能给不出，退到主显示器；两个都拿不到
/// 才回落到兜底常量。基准用整屏（monitor.size）而非工作区 —— 见
/// `default_window_height_from_screen` 的注释（任务栏那条交给落位去夹，
/// 不从高度里扣）。
fn default_window_height(app: &AppHandle) -> u32 {
    // 挑屏走 main_window_monitor：current_monitor() 在窗口不可见时回落主屏
    // （tao 实锤），多屏下「重置窗口大小」的高度就会按错屏的 2/3 算。
    let monitor = main_window_monitor(app).or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return DEFAULT_WINDOW_HEIGHT;
    };
    // scale_factor 理论上不会是 0，真遇上就按 1:1 处理，别让除零变成 inf→饱和
    let scale = monitor.scale_factor();
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let screen_height = (monitor.size().height as f64 / scale).round() as u32;
    default_window_height_from_screen(screen_height)
}

/// 夹回合法窗口尺寸（纯函数）。
fn clamp_window_size(width: u32, height: u32) -> (u32, u32) {
    (
        width.clamp(WINDOW_MIN_SIZE.0, WINDOW_MAX_SIZE.0),
        height.clamp(WINDOW_MIN_SIZE.1, WINDOW_MAX_SIZE.1),
    )
}

/// 落一个新窗口尺寸：夹取 → 记忆 → 应用。锁定状态下要先把 max 放开再落尺寸
/// （max_size 对 set_size 也生效，不放开 932 落不进一个 max=460 的窗口），
/// 落完再按新值收紧。所有改窗口尺寸的入口（滑条 / 切展示方式 / 重置）都走这里。
/// 居中的尺寸基准一律取「设置值 × 显示器缩放」（见 center_main_window），
/// 不读窗口实况 —— 2x 屏上未映射/过渡期的读数是逻辑值，会算错落点。
fn set_main_window_size(app: &AppHandle, width: u32, height: u32) -> Result<(), String> {
    let (width, height) = clamp_window_size(width, height);
    let mut settings = stored_settings(app);
    settings.window_width = width;
    settings.window_height = height;
    save_settings(app, &settings)?;
    let main = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "主窗口不存在".to_string())?;
    if settings.window_size_locked {
        let _ = main.set_max_size(None::<tauri::LogicalSize<f64>>);
    }
    main.set_size(tauri::LogicalSize::new(width as f64, height as f64))
        .map_err(|error| format!("无法调整窗口大小: {error}"))?;
    if settings.window_size_locked {
        let _ = main.set_max_size(Some(tauri::LogicalSize::new(
            width as f64,
            height as f64,
        )));
    }
    // 改完尺寸要重新落位（仅「固定屏幕中间」，窗口已显示时）：见
    // recenter_after_window_size。启动期 setup 调进来时窗口还没 show，
    // 那条路径有一套「等 WM 映射完再落位」的自愈流程，抢在它前面
    // set_position 会被 WM 忽略或覆盖，所以不插手。
    let visible = main.is_visible().unwrap_or(false);
    if visible {
        recenter_after_window_size(app, width, height);
    }
    Ok(())
}

/// 改完窗口尺寸后重新落位（只在「固定屏幕中间」模式下真正生效）。
///
/// 为什么必须补这一下：`set_size` 是**以左上角为锚**改尺寸的，只改高度时
/// 窗口往下长出去、底边越过工作区底部 → `schedule_clamp`（Moved 后 250ms）
/// 把它「往上推回工作区内」→ 于是原本居中的窗口被一路推到屏幕上沿，而且
/// **永远留在那儿**（1920x1080 实测：高 560→900→1000→560 得到
/// y=260→180→80→**80**，窗口中心从 540 漂到 360）。用户拖一次高度滑条就
/// 中招，「固定屏幕中间」名不副实。
///
/// 落位本身交给 `apply_main_window_position`（它自己判 center/last/free 与
/// 贴边停靠态），这里只解决「**什么时候**落」：`set_size` 是异步派发的，
/// 紧接着读 `outer_size()` 还是旧值，拿旧高度算出的落点会让窗口「刚居中又
/// 被撑歪」。所以先轮询等新尺寸真正落地（通常几十毫秒）再落位 —— 与呼出
/// 路径那段「等窗口映射完再落位」的自愈线程同一套路，上界 ≈600ms。
fn recenter_after_window_size(app: &AppHandle, width: u32, height: u32) {
    let app = app.clone();
    std::thread::spawn(move || {
        for _ in 0..40 {
            let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
                return;
            };
            // 尺寸落地前窗口被藏了/关了就放弃：这条落位只在「用户正看着窗口
            // 拖滑条」的语境下有意义，隐藏窗口的几何另有主（show 流程自己落位）。
            if !main.is_visible().unwrap_or(false) {
                return;
            }
            let scale = main.scale_factor().unwrap_or(1.0);
            let scale = if scale > 0.0 { scale } else { 1.0 };
            let want_w = (width as f64 * scale).round();
            let want_h = (height as f64 * scale).round();
            let landed = main
                .inner_size()
                .map(|size| {
                    (size.width as f64 - want_w).abs() <= 2.0
                        && (size.height as f64 - want_h).abs() <= 2.0
                })
                .unwrap_or(false);
            if landed {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        // 等待期间用户可能已经把落位模式改成别的（滑条和下拉框都在同一屏），
        // 那就按新模式来，别硬居中。
        if normalize_window_position(&stored_settings(&app).main_window_position) != "center" {
            return;
        }
        // 落位前先立几何抑制期：set_position 的回声是一条 Moved，抑制期内
        // 不拿它做贴边判定（同 dock/undock 的处理）。
        mark_geometry_busy(AUTO_HIDE_GEOMETRY_BUSY_MS);
        apply_main_window_position(&app);
    });
}

/// 把「锁定窗口大小」落到窗口属性上：锁定 = 不可拖拽缩放 + 不可最大化 +
/// 尺寸上限钉死在记忆值（上限同时挡住 WM 的最大化，双保险）。
fn apply_window_size_lock(app: &AppHandle) {
    let settings = stored_settings(app);
    let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };
    let _ = main.set_resizable(!settings.window_size_locked);
    let _ = main.set_maximizable(!settings.window_size_locked);
    if settings.window_size_locked {
        let (width, height) = clamp_window_size(settings.window_width, settings.window_height);
        let _ = main.set_max_size(Some(tauri::LogicalSize::new(
            width as f64,
            height as f64,
        )));
    } else {
        let _ = main.set_max_size(None::<tauri::LogicalSize<f64>>);
    }
}

/// 按当前展示方式把主窗口宽度落到位（高度不变）。
/// 触发点只有两处：启动 setup、用户切换展示方式 —— 切换时 UI 一定可见
/// （贴边成条时界面是藏着的，点不到下拉框），直接 set_size 不会跟贴边打架。
/// 按展示方式把主窗口宽度落到位（高度用记忆值不变）。
/// `force_nominal_width`：
/// · **true**（用户切换展示方式）= 用户明确要「这个模式的标准宽度」，覆盖记忆值
///   —— **锁定窗口大小也不例外**（用户定案 2026-10-13：锁定只管「不许手拖
///   缩放」，展示方式的落宽是明确操作、允许改尺寸）；
/// · **false**（启动）= 尊重记忆的宽度 —— 用户上次拖成 700 就还是 700，
///   只有老配置还没记过尺寸（window_width == 0，serde(default) 的初值）才按模式给标称。
/// 触发点只有两处：启动 setup、用户切换展示方式 —— 切换时 UI 一定可见
/// （贴边成条时界面是藏着的，点不到下拉框），直接 set_size 不会跟贴边打架。
fn apply_grub_layout_size(app: &AppHandle, force_nominal_width: bool) {
    let settings = stored_settings(app);
    let mode = normalize_grub_list_view(&settings.grub_list_view);
    let nominal = grub_layout_width(mode) as u32;
    let width = grub_layout_target_width(nominal, settings.window_width, force_nominal_width);
    let height = if settings.window_height == 0 {
        // 还没记过尺寸（新装 / 清过配置）：默认占整屏高度的 2/3。
        // 落完 set_main_window_size 会把它记进设置，下次就按记忆值来。
        default_window_height(app)
    } else {
        settings.window_height
    };
    // 尺寸入口统一走 set_main_window_size：记忆值同步更新，锁定上限跟着放宽/收紧
    if let Err(error) = set_main_window_size(app, width, height) {
        trace(app, "layout", &format!("按展示方式调窗口尺寸失败: {error}"));
    }
}

/// 展示方式对应的落宽（纯函数）：切模式（force）用模式标称覆盖记忆值 ——
/// 锁定窗口大小不拦这条（锁定只禁手拖缩放，见用户定案 2026-10-13）；
/// 启动路径（force=false）尊重记忆值，未记过尺寸（0）才用模式标称。
fn grub_layout_target_width(nominal: u32, remembered: u32, force: bool) -> u32 {
    if force || remembered == 0 {
        nominal
    } else {
        remembered
    }
}

/// 把主窗口摆到当前显示器工作区的中心（物理像素，夹回工作区）。返回落点。
/// 挑「主窗口此刻所在」的那块显示器。
///
/// 为什么不直接用 `current_monitor()`：tao 在 X11 上的实现是
/// 「`gtk_window.get_window()`（窗口不可见/未映射时返回 `None`）→
/// `monitor_at_window`，`None` 就**回落主显示器**」（tao 源码实锤）。托盘里
/// 点「重置窗口位置」时窗口多半藏着 → 永远居中到**主屏**，而不是窗口原来
/// 待的那块屏 —— 多屏下「居中不在该屏正中间」的直接来源。
///
/// 这里的选法按本项目定则来：判位只用**客户区坐标**（真实 configure，唯一
/// 可信来源）——窗口中心点落在哪块 `available_monitors()` 的矩形里就是哪块；
/// 都不命中（坐标陈旧/未映射过）退 `current_monitor()`，最后退第一块。
fn main_window_monitor(app: &AppHandle) -> Option<tauri::Monitor> {
    let main = app.get_webview_window(MAIN_WINDOW_LABEL)?;
    let monitors = main.available_monitors().ok()?;
    if monitors.is_empty() {
        return main.current_monitor().ok().flatten();
    }
    // 固定显示器优先：设了名字就按名字挑 —— 不依赖窗口几何，启动落位时
    // 窗口还没映射也照样成立（这正是固定屏要解决的场景：居中/呼出/重置
    // 必须落到用户指定的那块屏，而不是「窗口现在在哪块屏」）。显示器被
    // 拔了就往下走既有逻辑。
    let fixed = stored_settings(app).main_fixed_monitor;
    let names: Vec<&str> = monitors.iter().map(|m| m.name().map(|s| s.as_str()).unwrap_or("")).collect();
    if let Some(index) = fixed_monitor_index(&names, &fixed) {
        return monitors.into_iter().nth(index);
    }
    let center = window_client_position(&main)
        .zip(usable_window_size_basis(
            main.outer_size().ok().map(|s| (s.width, s.height)),
            main.inner_size().ok().map(|s| (s.width, s.height)),
        ))
        .map(|((x, y), (w, h))| (x + w as i32 / 2, y + h as i32 / 2));
    if let Some((cx, cy)) = center {
        for monitor in &monitors {
            let origin = monitor.position();
            let size = monitor.size();
            let (mx, my) = (origin.x, origin.y);
            let (mw, mh) = (size.width as i32, size.height as i32);
            if cx >= mx && cx < mx + mw && cy >= my && cy < my + mh {
                return Some(monitor.clone());
            }
        }
    }
    main.current_monitor()
        .ok()
        .flatten()
        .or_else(|| monitors.into_iter().next())
}

fn center_main_window(app: &AppHandle) -> Option<(i32, i32)> {
    let main = app.get_webview_window(MAIN_WINDOW_LABEL)?;
    // 贴边停靠态：白条不是「窗口」，居中它没有意义 —— 先完整展开，再把
    // 物理像素的居中目标交给重锚牛顿步收敛（展开过渡期亲手发 set_position
    // 会被 WM 重摆吞掉、窗口还会被 near_edge 抢跑贴回，见 center_docked_window）。
    let docked = app
        .state::<Mutex<AppState>>()
        .lock()
        .map(|state| state.auto_hide_docked)
        .unwrap_or(false);
    if docked {
        return center_docked_window(app, &main);
    }
    // 尺寸基准 = 设置里记的**逻辑**尺寸 × 显示器缩放 = 物理像素。
    // 为什么不读窗口实况：本机（2x 屏）实测，未映射 / 刚过 dock-undock 过渡 /
    // GDK frame extents 陈旧时，tao 的读数返回**逻辑值**（932x720，真窗是
    // 1864x1440 物理），拿它居中 = 落点偏出正中 ~466px，底边越界 → near_edge
    // → 自动贴边（10:46~11:07 日志反复实锤：启动首屏、「屏幕中间」拖动回弹、
    // 重置大小后的重居中全按 932x720 算）。设置值是唯一「用户想要多大」的
    // 事实源 —— 手拖缩放也实时回写设置（前端 Resized → set_window_size），
    // × 缩放后永远与工作区同一单位，不存在滞后。
    let settings = stored_settings(app);
    let monitor = main_window_monitor(app)?;
    let work = monitor.work_area();
    let scale = monitor.scale_factor();
    let (lw, lh) = clamp_window_size(settings.window_width, settings.window_height);
    let (client_w, client_h) = (
        (lw as f64 * scale).round() as i32,
        (lh as f64 * scale).round() as i32,
    );
    // 外框（客户区+标题栏）在工作区居中（见 docked_center_target）。请求按
    // 「实况边距」反补偿：set_position 由 GDK 按它手头那份 extents 缓存定位，
    // 现场量出的 inner−outer 就是它将要应用的偏移 → 客户区必然落到 target
    // （三端统一：Win=窗口矩形语义、mac=窗框语义、Linux= extents 缓存语义，
    // 各自的缓存值自洽）。Linux 缓存陈旧读到 0 时退回 dock 时缓存的标题栏。
    let frame_offset = match (main.inner_position(), main.outer_position()) {
        (Ok(inner), Ok(outer)) => (inner.x - outer.x, inner.y - outer.y),
        _ => (0, 0),
    };
    let caption = if frame_offset.1 > 0 {
        frame_offset.1
    } else {
        app.state::<Mutex<AppState>>()
            .lock()
            .map(|state| state.auto_hide_caption)
            .unwrap_or(0)
            .max(0)
    };
    let target = docked_center_target(
        work.position.x,
        work.position.y,
        work.size.width as i32,
        work.size.height as i32,
        client_w,
        client_h,
        caption,
    );
    let (cx, cy) = clamp_to_work(
        target.0 - frame_offset.0,
        target.1 - frame_offset.1,
        client_w,
        client_h + caption,
        work.position.x,
        work.position.y,
        work.size.width as i32,
        work.size.height as i32,
    );
    // 居中是低频用户动作，取证常开（同 dock/undock）：挑了哪块屏、工作区是
    // 哪个矩形、窗口基准多大 —— 「居中不到正中间」下次一眼定位。
    let work_desc = {
        let work = monitor.work_area();
        format!(
            "monitor={:?} monitor_rect=({},{},{},{}) work=({},{},{},{})(物理px)",
            monitor.name(),
            monitor.position().x,
            monitor.position().y,
            monitor.size().width,
            monitor.size().height,
            work.position.x,
            work.position.y,
            work.size.width,
            work.size.height
        )
    };
    startup_debug(
        app,
        &format!(
            "center main -> ({cx},{cy}) target=({},{}) 客户区={client_w}x{client_h} 边距={frame_offset:?} 标题栏={caption} {work_desc}",
            target.0, target.1
        ),
    );
    let _ = main.set_position(tauri::PhysicalPosition::new(cx, cy));
    // 记落点：「屏幕中间」模式下 handle_main_moved 靠它区分「我们自己居中的
    // 回声 / WM 抖动」与「用户硬拖」（见 CENTER_HOLD_TOLERANCE）。记的是
    // **请求值**：Moved 载荷是窗框坐标，与它恒定差一个标题栏，80px 容差罩得住。
    CENTER_LAST_X.store(cx, Ordering::SeqCst);
    CENTER_LAST_Y.store(cy, Ordering::SeqCst);
    CENTER_HAS_POS.store(true, Ordering::SeqCst);
    Some((cx, cy))
}

/// 停靠态「重置位置」的展开过渡等待（毫秒）：undock 的重装饰/改尺寸落地
/// 期间发出的 set_position 会被 WM 重摆吞掉（实测见 center_docked_window），
/// 等它落地再把居中交给重锚牛顿步；抑制期同步立起，防 near_edge 抢跑贴回。
const DOCKED_CENTER_SETTLE_MS: u64 = 600;

/// 停靠态居中的目标**客户区**落点（纯函数，物理像素）：外框（客户区+标题栏）
/// 在工作区居中 —— 与非停靠路径的外框基准同观感；重锚的收敛判据是
/// 「客户区实况 == target」，所以 y 要加回标题栏。
fn docked_center_target(
    work_x: i32,
    work_y: i32,
    work_w: i32,
    work_h: i32,
    client_w: i32,
    client_h: i32,
    caption: i32,
) -> (i32, i32) {
    let outer_h = client_h + caption;
    (
        work_x + (work_w - client_w) / 2,
        work_y + (work_h - outer_h) / 2 + caption,
    )
}

/// 停靠态的「重置窗口位置」：先走完整展开链路（还原装饰/尺寸/置顶），再把
/// 物理像素的居中目标交给重锚牛顿步收敛。返回居中目标（客户区坐标）。
///
/// 两个实测教训（10:46 日志，2x 屏 3840x2160，底边贴边）：
/// 1. **居中不能在过渡期亲手发**：undock 的 hide→show/重装饰/改尺寸过渡要
///    几百 ms，期间发出的 set_position 会被尺寸落地时的 WM 重摆吞掉 ——
///    undock 后 2ms 发 center -> (1454,666)，708ms 后窗口仍停在 undock 落点
///    (1454,484)，随后 near_edge=true 自动贴回底边（用户看到的就是「点了没
///    反应」）。所以立 600ms 抑制期（防贴回抢跑）等过渡落地，再由重锚牛顿步
///    补发 —— 它每拍读实况、偏了就修，吞一拍吞不掉整个收敛。
/// 2. **基准不能用设置里记的尺寸**：那是**逻辑**尺寸（set_size(LogicalSize)），
///    居中的对面是**物理**工作区，2x 屏差一倍（实测按 932x720 算出
///    (1454,666)，偏出正中 ~466px）。基准用贴边前的客户区尺寸（物理，
///    undock 刚拿它展开）。
fn center_docked_window(app: &AppHandle, main: &tauri::WebviewWindow) -> Option<(i32, i32)> {
    let (restore, caption) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (state.auto_hide_restore_size, state.auto_hide_caption)
    };
    undock_main_window(app);
    let monitor = main_window_monitor(app)?;
    let work = monitor.work_area();
    let (w, h) = restore.unwrap_or_else(|| {
        main.inner_size()
            .map(|s| (s.width.max(1), s.height.max(1)))
            .unwrap_or((460, 900))
    });
    let cap = caption.max(0);
    let target = docked_center_target(
        work.position.x,
        work.position.y,
        work.size.width as i32,
        work.size.height as i32,
        w as i32,
        h as i32,
        cap,
    );
    // 抑制期必须在过渡落地前立起来：窗口此刻停在贴边落点（near_edge=true），
    // 不压住的话贴边轮询几拍内就把它收回底边，居中还没开始就输了。牛顿步
    // 每次补发会各自续 400ms，收敛前贴回一直被压住。
    mark_geometry_busy(DOCKED_CENTER_SETTLE_MS);
    let app_for_thread = app.clone();
    std::thread::spawn(move || {
        // 等过渡落地（期间的 set_position 会被吞，实测见函数注释），再把
        // 居中计划交给 watcher 的牛顿步。预置修正量 = live − target：牛顿步
        // 第一拍的请求 = target + (offset + target − live) = target，
        // 首发即准、不产生垃圾中间跳。
        std::thread::sleep(std::time::Duration::from_millis(DOCKED_CENTER_SETTLE_MS));
        let live = main_client_position(&app_for_thread).unwrap_or(target);
        if let Ok(mut state) = app_for_thread.state::<Mutex<AppState>>().lock() {
            state.auto_hide_reanchor = Some(AutoHideReanchor {
                target,
                offset: (live.0 - target.0, live.1 - target.1),
                tries: 0,
                last_live: None,
                noops: 0,
            });
        }
    });
    // 记落点：牛顿步补发产生的 Moved 都落在 target ± 标题栏内，居中模式的
    // 「还停在居中位」判据（±CENTER_HOLD_TOLERANCE）天然罩得住。
    CENTER_LAST_X.store(target.0, Ordering::SeqCst);
    CENTER_LAST_Y.store(target.1, Ordering::SeqCst);
    CENTER_HAS_POS.store(true, Ordering::SeqCst);
    startup_debug(
        app,
        &format!(
            "center docked -> ({},{}) 客户区={}x{} 标题栏={} 等待={DOCKED_CENTER_SETTLE_MS}ms",
            target.0, target.1, w, h, cap
        ),
    );
    Some(target)
}

/// 按固定位置模式落位主窗口（贴边停靠态不干预）。center=居中；last=落到
/// 记忆的位置（无记忆则居中）；free=不管。
fn apply_main_window_position(app: &AppHandle) -> Option<(i32, i32)> {
    let docked = app
        .state::<Mutex<AppState>>()
        .lock()
        .map(|state| state.auto_hide_docked)
        .unwrap_or(false);
    if docked {
        return None;
    }
    let mode = normalize_window_position(&stored_settings(app).main_window_position).to_string();
    let pos = match mode.as_str() {
        "center" => center_main_window(app),
        "last" => match stored_settings(app).main_window_last {
            Some(p) => {
                if auto_hide_diag_enabled() {
                    startup_debug(app, &format!("apply last -> ({},{})", p.0, p.1));
                }
                if let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                    let _ = main.set_position(tauri::PhysicalPosition::new(p.0, p.1));
                }
                Some(p)
            }
            None => center_main_window(app),
        },
        _ => None,
    };
    // 固定显示器：center 已由 center_main_window 走 main_window_monitor
    // （固定屏优先）天然落在固定屏上；free/last 的落点可能指向别的屏
    // （记忆点在屏 2、用户把窗口拖去过屏 2），这里统一约束回固定屏。
    enforce_fixed_monitor(app);
    pos
}

/// 固定显示器约束：窗口不在固定显示器上时，把客户区夹进它的工作区。
/// auto / 显示器已不在 → 不动。判位与落位跟 clamp_main_window 同款
/// （客户区判位 + 窗框语义请求，见那边注释），只是挑屏换成固定名。
fn enforce_fixed_monitor(app: &AppHandle) {
    if normalize_fixed_monitor(&stored_settings(app).main_fixed_monitor) == "auto" {
        return;
    }
    let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };
    if !main.is_visible().unwrap_or(false) {
        return;
    }
    // main_window_monitor 已按固定名挑屏：它返回的若不是配置里那块屏，
    // 说明显示器不在了（回落逻辑），此时不动窗口。
    let Some(monitor) = main_window_monitor(app) else {
        return;
    };
    if monitor.name().map(|name| name.as_str())
        != Some(normalize_fixed_monitor(&stored_settings(app).main_fixed_monitor).as_str())
    {
        return;
    }
    let (Ok(client), Ok(client_size), Ok(outer)) = (
        main.inner_position(),
        main.inner_size(),
        main.outer_position(),
    ) else {
        return;
    };
    let (w, h) = (client_size.width as i32, client_size.height as i32);
    // 客户区中心已在固定屏内 = 正在固定屏上显示，free 模式的既有落位不动它。
    let origin = monitor.position();
    let (mw, mh) = (monitor.size().width as i32, monitor.size().height as i32);
    let (cx, cy) = (client.x + w / 2, client.y + h / 2);
    if cx >= origin.x && cx < origin.x + mw && cy >= origin.y && cy < origin.y + mh {
        return;
    }
    let work = monitor.work_area();
    let (x, y) = clamp_to_work(
        client.x,
        client.y,
        w,
        h,
        work.position.x,
        work.position.y,
        work.size.width as i32,
        work.size.height as i32,
    );
    let request = (x - (client.x - outer.x), y - (client.y - outer.y));
    let _ = main.set_position(tauri::PhysicalPosition::new(request.0, request.1));
    pin_main_topmost(app);
    startup_debug(
        app,
        &format!(
            "fixed monitor -> client ({x},{y}) monitor={:?}",
            monitor.name()
        ),
    );
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

/// 本机名称（给客户端一眼认出「这是哪台电脑」）。取不到返回空串。
///
/// 只读环境变量与几个约定路径，不开子进程 —— 它在 `/health` 里每次请求都会走，
/// 不能有进程开销。
fn machine_name() -> String {
    // Windows 是 COMPUTERNAME；Linux / macOS 的 shell 里是 HOSTNAME
    let from_env = ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .map(|key| std::env::var(key).unwrap_or_default());
    // Linux 兜底；其它系统这两个文件不存在，read_to_string 会失败并被跳过
    let from_file = ["/etc/hostname", "/proc/sys/kernel/hostname"]
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap_or_default());
    first_nonempty(from_env.chain(from_file))
}

/// 从一组候选里挑第一个「去掉首尾空白后非空」的值（纯函数，便于单测）。
fn first_nonempty<T: IntoIterator<Item = String>>(values: T) -> String {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

/// `GET /health` 的响应体（纯函数，便于单测）。
///
/// `ok`/`service` 是既有的探活字段；`machineId`/`machineName` 是给客户端认机器用的
/// —— 同一台电脑多块网卡会有多个 IP，客户端靠 `machineId` 把它们归并成一份页面。
/// 这里**不含任何机密**，所以和探活一样免鉴权。
fn health_json(machine_id: &str, machine_name: &str) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "service": "anyboot",
        "machineId": machine_id.trim(),
        "machineName": machine_name.trim(),
    })
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

/// 监听状态的一句话（纯函数）。主窗口与托盘菜单都读它，同一份真相。
fn http_state_text(
    listening: bool,
    addr: Option<&str>,
    last_error: Option<&str>,
    token_set: bool,
) -> String {
    if listening {
        return format!("正在监听 {}", addr.unwrap_or("-"));
    }
    if let Some(error) = last_error {
        return format!("未监听：{error}");
    }
    if token_set {
        "未监听（服务已关闭）".to_string()
    } else {
        "未监听（token 为空，服务不会启动）".to_string()
    }
}

/// 菜单里的一行状态文案：过长的错误信息截断，免得把菜单撑到满屏。
fn menu_status_text(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
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

    /// 界面文案：自己占着 / 同程序的另一个实例 / 完全外部，三种说法不能混。
    ///
    /// 以前「自己占着」也被算进 same_app，于是显示成「本应用的另一个实例」——
    /// 实际上是**我们自己**上一轮监听的 socket 没放开，用户会去找一个根本不存在的进程。
    fn display(&self) -> String {
        if self.is_self {
            format!(
                "端口还被本程序自己占着（PID {}）：上一轮监听的 socket 没放开。点「重试监听」，仍不行就重启本程序",
                self.pid
            )
        } else if self.same_app {
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

    /// 给用户复制即用的结束命令（Windows 优先 taskkill，非 Windows 给通用提示）。
    ///
    /// 占端口的是**当前进程**时不给：那是「结束自己」，后端也会直接拒绝，摆出来只会误导。
    fn kill_hint(&self) -> Option<String> {
        if self.pid == 0 || self.is_self {
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
        command("wmic")
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
        command("ps")
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
    let output = command("netstat").args(["-ano"]).output().ok()?;
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
        command("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .map_err(|error| format!("无法执行 taskkill：{error}"))?
    } else {
        command("kill")
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
    if settings.machine_id.trim().is_empty() {
        // 本机标识：跨重启稳定（存在 settings.json 里），客户端靠它归并同一台
        // 电脑的多个网卡 IP。跟 token 用同一套随机来源，但没有鉴权含义。
        settings.machine_id = generate_token();
        changed = true;
        trace(app, "http", "首次运行：已生成机器标识 machineId");
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

/// 确定性收掉当前 HTTP 监听：`stop()` 返回时端口一定已经放开（tiny_http 的 accept
/// 线程是异步退场的，等待逻辑在 http::RunningServer::stop 里），所以调用方紧接着
/// 重绑端口是安全的。
///
/// 两个调用方：改配置时的「停旧的再起新的」（apply_http_config），以及「重启程序」
/// 之前先把端口让出来（restart_app）。
fn stop_http_server(app: &AppHandle) {
    let previous = http_lock().server.take();
    if let Some(server) = previous {
        let addr = server.addr.clone();
        match server.stop() {
            Ok(()) => trace(app, "http", &format!("已停止旧服务 {addr}，端口已放开")),
            Err(error) => trace(app, "http", &format!("已停止旧服务 {addr}，但{error}")),
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

    // 先确定性收掉旧服务：stop() 返回时端口一定已经放开（tiny_http 的 accept 线程
    // 是异步退场的，等待逻辑在 http::RunningServer::stop 里），所以下面立刻重绑是安全的。
    stop_http_server(app);

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
    /// 未经 pairs 解析的原始请求体：JSON 端点（/api/ui-command）要用，
    /// pairs 解析会把 JSON 撕碎。
    pub(crate) raw_body: String,
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
    /// 原始响应体（网页控制台静态资源）：Some((content_type, 字节)) 时忽略 json。
    pub(crate) raw: Option<(&'static str, Vec<u8>)>,
}

impl RemoteReply {
    fn ok(status: u16, json: serde_json::Value) -> Self {
        Self {
            status,
            json,
            raw: None,
        }
    }

    fn error(status: u16, code: &str, message: &str) -> Self {
        Self {
            status,
            json: serde_json::json!({
                "ok": false,
                "error": code,
                "message": message,
            }),
            raw: None,
        }
    }

    /// 静态资源响应（非 JSON）。
    fn raw_page(status: u16, content_type: &'static str, bytes: Vec<u8>) -> Self {
        Self {
            status,
            json: serde_json::Value::Null,
            raw: Some((content_type, bytes)),
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
        "/api/grub-switch" => {
            // selector 里可能有空格、`>`、引号 —— 一律要求调用方先做 URL 编码，
            // `RemoteRequest::param` 会解码回来（见 http::percent_decode）。
            let Some(entry) = request
                .param("entry")
                .or_else(|| request.param("selector"))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
            else {
                return Err(RemoteReply::error(
                    400,
                    "missing_entry",
                    "缺少 entry 参数（GRUB 启动项的 selector，可用 --grub-list / GET /api/grub 查）",
                ));
            };
            Ok(RemoteCommand::SwitchGrub(entry))
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

/// GRUB 列表的 JSON 视图（HTTP `/api/grub` 与 CLI `--grub-list` 共用）。
///
/// 只读当前快照，不主动重扫 —— 与 `/api/status` 同一套语义：要刷新由本机点同步。
fn remote_grub_json(app: &AppHandle) -> serde_json::Value {
    let settings = stored_settings(app);
    let view = grub_snapshot(app, &settings);
    serde_json::json!({
        "ok": true,
        "partuuid": view.partuuid,
        "efiDir": view.efi_dir,
        "envExists": view.env_exists,
        "installed": view.installed,
        "syncedAt": view.synced_at,
        "savedEntry": view.saved_entry,
        "nextEntry": view.next_entry,
        "error": view.error,
        "entries": view
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
    })
}

/// 网页控制台静态资源（编译期嵌入）：内置 HTTP 服务直接把主窗口那套前端
/// 托管给局域网/外网浏览器，设置改动通过 /api/ui-command 真正作用于本机应用。
const UI_INDEX_HTML: &str = include_str!("../../ui/index.html");
const UI_APP_JS: &str = include_str!("../../ui/app.js");
const UI_STYLES_CSS: &str = include_str!("../../ui/styles.css");
const UI_GRUB_LIST_VIEW_JS: &str = include_str!("../../ui/grub-list-view.js");

/// 网页控制台静态资源路由。
///
/// 页面本身**不要求 token**：浏览器得先拿到页面才能让用户输入 token；
/// 真正的数据与命令全部在 /api/* 后面，那里全程 token 鉴权。页面里没有任何
/// 机密（token 只在设置快照接口返回，且那需要 token 本身）。
fn static_ui_reply(path: &str) -> Option<RemoteReply> {
    let (content_type, body): (&'static str, &str) = match path {
        // `/index.html` 也得挂上：用户/书签很容易直接打这个（浏览器里输
        // `192.168.1.5:12300/index.html` 是很自然的动作）。漏了它就落到 token
        // 闸门 → 无 token 时回 401「token 缺失或无效」、带 token 时回 404，
        // 两种都让人以为是自己 token 的问题，其实是路由没接。
        "/" | "/index.html" | "/ui" | "/ui/" | "/ui/index.html" => {
            ("text/html; charset=utf-8", UI_INDEX_HTML)
        }
        // index.html 里引用的是 ./styles.css 等相对路径，页面在根路径时
        // 解析成 /styles.css —— 根路径与 /ui/ 前缀两套都挂上
        "/app.js" | "/ui/app.js" => ("text/javascript; charset=utf-8", UI_APP_JS),
        "/grub-list-view.js" | "/ui/grub-list-view.js" => {
            ("text/javascript; charset=utf-8", UI_GRUB_LIST_VIEW_JS)
        }
        "/styles.css" | "/ui/styles.css" => ("text/css; charset=utf-8", UI_STYLES_CSS),
        _ => return None,
    };
    Some(RemoteReply::raw_page(
        200,
        content_type,
        body.as_bytes().to_vec(),
    ))
}

/// POST /api/ui-command：网页控制台的命令通道（真正改设置的入口）。
///
/// body：`{"cmd":"set_theme","args":{"theme":"dark"}}` → `{ok:true,cmd,result}`。
/// 与 Tauri 主窗口共用**同一批命令函数**（同进程直接调用），所以行为、
/// 校验、落盘、广播完全一致；全程 token 鉴权（handle_remote_request 前置）。
fn handle_ui_command_request(app: &AppHandle, request: &RemoteRequest) -> RemoteReply {
    let parsed = match serde_json::from_str::<serde_json::Value>(request.raw_body.trim()) {
        Ok(value) => value,
        Err(error) => {
            return RemoteReply::error(400, "bad_json", &format!("请求体不是合法 JSON：{error}"))
        }
    };
    let Some(cmd) = parsed
        .get("cmd")
        .and_then(|value| value.as_str())
        .map(str::to_string)
    else {
        return RemoteReply::error(400, "missing_cmd", "缺少 cmd 字段");
    };
    let args = parsed.get("args").cloned().unwrap_or(serde_json::Value::Null);

    // GUI 交互类命令：要弹**本机**对话框/文件管理器，远程网页没有意义
    // （「重启程序 / 退出程序」操作的是电脑上那份应用，同理只能在本机点：
    // 远程端一个误触就把服务关了，谁都连不上）。
    if matches!(
        cmd.as_str(),
        "pick_grub_dir"
            | "open_config_dir"
            | "open_vars_dir"
            | "open_grub_dir"
            | "open_login_items_settings"
            | "restart_app"
            | "quit_app"
    ) {
        return RemoteReply::error(
            400,
            "gui_only",
            "该操作需要在本机应用内执行（远程网页不支持）",
        );
    }

    // 「选择系统」从网页 / 手机客户端发起时，走**远程请求**那条链路
    // （与 POST /api/switch 完全一致）：本机弹授权窗、显示来源客户端，由本机
    // 用户确认或取消 —— 而不是借道网页绕过本机直接改引导项。
    // 本机主窗口点按钮不走这条 HTTP 路径（那是 Tauri IPC 直连命令），
    // 所以本机自己的操作依然是即时的，不受影响。
    // 「立即执行」只能在本机点：授权窗的意义就是本机用户点头，远程端能自己确认
    // 等于自己给自己盖章，弹窗就成了摆设。「取消」放行 —— 撤回自己的请求是安全
    // 方向的操作，随时能反悔也是授权模型该有的样子（见 ui_command_is_local_only）。
    if ui_command_is_local_only(&cmd) {
        record_request_log(
            app,
            remote_channel(request),
            &request.source,
            &request.path,
            &cmd,
            "rejected",
            "只能在本机点「立即执行」",
        );
        return RemoteReply::error(
            403,
            "local_only",
            "「立即执行」需要在电脑本机上点（远程只能取消这条请求）",
        );
    }

    if ui_command_requires_local_auth(&cmd) {
        let system = match ui_str(&args, "system") {
            Ok(system) => system,
            Err(message) => return RemoteReply::error(400, "bad_args", &message),
        };
        let reply = start_remote_command(
            app,
            remote_channel(request),
            &request.source,
            RemoteCommand::Switch(system),
            &request.path,
        );
        // 网页端的 invoke 只取 `result`（见 ui/app.js），所以把远程请求的回执
        // **整块塞进 result** —— 前端要据 `result.pending` 告诉用户
        // 「已发送到本机，等待确认」，否则这条提示永远不会出现。
        // 被拒（409 已有待执行请求等）原样返回错误，让前端照常报错。
        if reply.status >= 400 {
            return reply;
        }
        return RemoteReply::ok(
            200,
            serde_json::json!({ "ok": true, "cmd": cmd, "result": reply.json }),
        );
    }

    let started = std::time::Instant::now();
    let reply = match remote_ui_command(app, &cmd, &args) {
        Ok(result) => RemoteReply::ok(
            200,
            serde_json::json!({ "ok": true, "cmd": cmd, "result": result }),
        ),
        Err(message) => RemoteReply::error(400, "command_failed", &message),
    };
    record_request_log(
        app,
        remote_channel(request),
        &request.source,
        &request.path,
        &cmd,
        if reply.status == 200 { "ok" } else { "failed" },
        &format!("{}ms", started.elapsed().as_millis()),
    );
    reply
}

/// 网页 / 手机端发来的哪些命令**必须先经本机授权**（转成远程请求 → 本机弹窗确认）。
///
/// 目前只有「选择系统」：它会改下一次的引导项，属于必须让本机用户点头的事。
/// 本机主窗口不走这条 HTTP 路径（走 IPC 直连命令），所以不受影响。
fn ui_command_requires_local_auth(cmd: &str) -> bool {
    cmd == "switch_system"
}

/// 哪些网页 / 手机命令**只能在本机点**（远程调用直接拒绝，而不是转成待确认）。
///
/// 目前只有「立即执行」：授权窗的意义就是让本机用户点头，远程端要是能自己确认，
/// 就等于自己给自己盖章、弹窗成了摆设。「取消」（撤回自己刚发的那条请求）是安全
/// 方向的操作，远程端保留 —— 随时能反悔也是授权模型该有的样子。
fn ui_command_is_local_only(cmd: &str) -> bool {
    cmd == "confirm_pending"
}

// ---------- /api/ui-command 参数提取辅助 ----------

fn ui_str(args: &serde_json::Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("缺少或非法参数 {key}（应为字符串）"))
}

fn ui_bool(args: &serde_json::Value, key: &str) -> Result<bool, String> {
    args.get(key)
        .and_then(|value| value.as_bool())
        .ok_or_else(|| format!("缺少或非法参数 {key}（应为布尔）"))
}

fn ui_u32(args: &serde_json::Value, key: &str) -> Result<u32, String> {
    args.get(key)
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("缺少或非法参数 {key}（应为非负整数）"))
}

fn ui_i32(args: &serde_json::Value, key: &str) -> Result<i32, String> {
    args.get(key)
        .and_then(|value| value.as_i64())
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| format!("缺少或非法参数 {key}（应为整数）"))
}

/// 可缺省的字符串参数（JSON null / 缺字段 → None）。
fn ui_opt_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn status_value(status: AppStatus) -> Result<serde_json::Value, String> {
    serde_json::to_value(status).map_err(|error| format!("序列化失败: {error}"))
}

fn settings_value(view: Result<SettingsView, String>) -> Result<serde_json::Value, String> {
    let view = view?;
    serde_json::to_value(view).map_err(|error| format!("序列化失败: {error}"))
}

fn res_unit(result: Result<(), String>) -> Result<serde_json::Value, String> {
    result?;
    Ok(serde_json::Value::Null)
}

/// 网页控制台命令 → 同进程命令函数的白名单分发。
fn remote_ui_command(
    app: &AppHandle,
    cmd: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    // 改 HTTP 服务配置的命令会在执行里**同步重启监听**（apply_http_config →
    // server.stop() 会 join 全部 worker）—— 而这条请求本身就是 worker 在处理，
    // 同步跑就是自我死锁。转后台线程执行并立即回执，结果通过轮询快照反映。
    if matches!(
        cmd,
        "set_http_enabled"
            | "set_http_bind"
            | "set_http_port"
            | "set_http_token"
            | "regenerate_http_token"
            | "retry_http"
    ) {
        let handle = app.clone();
        let cmd = cmd.to_string();
        let args = args.clone();
        std::thread::spawn(move || {
            if let Err(error) = remote_ui_command_inner(&handle, &cmd, &args) {
                trace(&handle, "http", &format!("网页命令 {cmd} 后台执行失败: {error}"));
            }
        });
        return Ok(serde_json::json!({ "deferred": true }));
    }
    remote_ui_command_inner(app, cmd, args)
}

fn remote_ui_command_inner(
    app: &AppHandle,
    cmd: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    match cmd {
        "get_ui_state" => serde_json::to_value(get_ui_state(app.clone()))
            .map_err(|error| format!("序列化失败: {error}")),
        // ---- 引导切换 / 扫描 ----
        "refresh" => status_value(refresh(app.clone())),
        // 注意：网页 / 手机端的 switch_system 在 handle_ui_command_request 里
        // 就被拦下改走本机授权流程了（见那里的注释），这里的分支留给非 HTTP 的
        // 内部调用者，语义是「立即切换」。
        "switch_system" => {
            let system = ui_str(args, "system")?;
            status_value(switch_system(app.clone(), system))
        }
        "set_vars_dir" => status_value(set_vars_dir(app.clone(), ui_opt_str(args, "path"))),
        "unmount_temporary" => status_value(unmount_temporary(app.clone())),
        "power_action" => res_unit(power_action(app.clone(), ui_opt_str(args, "action"))),
        // confirm_pending 在 handle_ui_command_request 里就被拦下了（本机专属，
        // 见 ui_command_is_local_only）；这里的分支留给非 HTTP 的内部调用者。
        "confirm_pending" => res_unit(confirm_pending(app.clone())),
        "cancel_pending" => res_unit(cancel_pending(app.clone())),
        // ---- GRUB ----
        "switch_grub" => {
            let selector = ui_str(args, "selector")?;
            status_value(switch_grub(app.clone(), selector))
        }
        "detect_esp" => {
            let path = ui_str(args, "path")?;
            detect_esp(app.clone(), path).map(serde_json::Value::String)
        }
        "detect_grub_by_partuuid" => {
            detect_grub_by_partuuid(app.clone()).map(serde_json::Value::String)
        }
        "set_grub_dir" => res_unit(set_grub_dir(app.clone(), ui_str(args, "dir")?)),
        "set_grub_partuuid" => {
            res_unit(set_grub_partuuid(app.clone(), ui_str(args, "partuuid")?))
        }
        "set_grub_script" => res_unit(set_grub_script(app.clone(), ui_str(args, "script")?)),
        "set_grub_mount" => res_unit(set_grub_mount(app.clone(), ui_bool(args, "enabled")?)),
        "sync_grub" => res_unit(sync_grub(app.clone())),
        "install_grub_script" => res_unit(install_grub_script(app.clone())),
        "uninstall_grub_script" => res_unit(uninstall_grub_script(app.clone())),
        "set_grub_list_view" => {
            settings_value(set_grub_list_view(app.clone(), ui_str(args, "mode")?))
        }
        "set_grub_list_format" => {
            settings_value(set_grub_list_format(app.clone(), ui_str(args, "format")?))
        }
        // ---- 主窗行为 / 贴边 / 白条 / 热区（Result<SettingsView,_> 一批）----
        "set_main_auto_hide" => settings_value(set_main_auto_hide(app.clone(), ui_bool(args, "enabled")?)),
        "set_main_always_on_top" => {
            settings_value(set_main_always_on_top(app.clone(), ui_bool(args, "enabled")?))
        }
        "set_main_auto_hide_edge_distance" => settings_value(set_main_auto_hide_edge_distance(
            app.clone(),
            ui_i32(args, "distance")?,
        )),
        "set_main_dock_strip_thickness" => {
            settings_value(set_main_dock_strip_thickness(app.clone(), ui_i32(args, "value")?))
        }
        "set_main_dock_strip_length" => {
            settings_value(set_main_dock_strip_length(app.clone(), ui_i32(args, "value")?))
        }
        "set_main_dock_strip_opacity" => {
            settings_value(set_main_dock_strip_opacity(app.clone(), ui_i32(args, "value")?))
        }
        "set_main_dock_strip_edge_gap" => {
            settings_value(set_main_dock_strip_edge_gap(app.clone(), ui_i32(args, "value")?))
        }
        "set_main_dock_strip_hover_ms" => settings_value(set_main_dock_strip_hover_ms(
            app.clone(),
            args.get("value")
                .and_then(|value| value.as_u64())
                .ok_or_else(|| "缺少或非法参数 value（应为非负整数）".to_string())?,
        )),
        "set_main_dock_strip_color" => {
            settings_value(set_main_dock_strip_color(app.clone(), ui_str(args, "color")?))
        }
        "set_main_hotzone_thickness" => {
            settings_value(set_main_hotzone_thickness(app.clone(), ui_i32(args, "value")?))
        }
        "set_main_hotzone_color" => {
            settings_value(set_main_hotzone_color(app.clone(), ui_str(args, "color")?))
        }
        "set_main_hotzone_opacity" => {
            settings_value(set_main_hotzone_opacity(app.clone(), ui_i32(args, "value")?))
        }
        "set_main_hotzone_border" => {
            settings_value(set_main_hotzone_border(app.clone(), ui_bool(args, "enabled")?))
        }
        "set_main_hotzone_border_color" => {
            settings_value(set_main_hotzone_border_color(app.clone(), ui_str(args, "color")?))
        }
        "set_accent_color" => settings_value(set_accent_color(app.clone(), ui_str(args, "color")?)),
        "set_main_window_position" => {
            settings_value(set_main_window_position(app.clone(), ui_str(args, "mode")?))
        }
        "set_tray_menu_style" => {
            settings_value(set_tray_menu_style(app.clone(), ui_str(args, "mode")?))
        }
        "reset_main_window_position" => settings_value(reset_main_window_position(app.clone())),
        "set_window_size" => settings_value(set_window_size(
            app.clone(),
            ui_u32(args, "width")?,
            ui_u32(args, "height")?,
        )),
        "set_window_size_locked" => {
            settings_value(set_window_size_locked(app.clone(), ui_bool(args, "locked")?))
        }
        "set_hide_from_dock_taskbar" => {
            settings_value(set_hide_from_dock_taskbar(app.clone(), ui_bool(args, "enabled")?))
        }
        "set_close_action" => {
            settings_value(set_close_action(app.clone(), ui_str(args, "action")?))
        }
        "set_autostart" => settings_value(set_autostart(app.clone(), ui_bool(args, "enabled")?)),
        "set_start_in_tray" => {
            settings_value(set_start_in_tray(app.clone(), ui_bool(args, "enabled")?))
        }
        // ---- 外观 ----
        "set_theme" => res_unit(set_theme(app.clone(), ui_str(args, "theme")?)),
        "set_opacity" => res_unit(set_opacity(app.clone(), ui_u32(args, "percent")?)),
        // ---- 通知 / 提示音 ----
        "set_notify_enabled" => {
            settings_value(set_notify_enabled(app.clone(), ui_bool(args, "enabled")?))
        }
        "set_remote_allow_gui_auth" => {
            settings_value(set_remote_allow_gui_auth(app.clone(), ui_bool(args, "enabled")?))
        }
        "set_channel_notify" => settings_value(set_channel_notify(
            app.clone(),
            ui_str(args, "channel")?,
            ui_bool(args, "notify")?,
        )),
        "set_channel_delay" => settings_value(set_channel_delay(
            app.clone(),
            ui_str(args, "channel")?,
            ui_u32(args, "seconds")?,
        )),
        "set_play_sound" => res_unit(set_play_sound(app.clone(), ui_bool(args, "enabled")?)),
        "set_sound_file_path" => {
            res_unit(set_sound_file_path(app.clone(), ui_str(args, "path")?))
        }
        // ---- HTTP 服务配置（外层已转后台线程执行）----
        "set_http_enabled" => {
            settings_value(set_http_enabled(app.clone(), ui_bool(args, "enabled")?))
        }
        "set_http_bind" => settings_value(set_http_bind(app.clone(), ui_str(args, "bind")?)),
        "set_http_port" => {
            let port = ui_u32(args, "port")?;
            let port = u16::try_from(port).map_err(|_| "端口超出范围（1–65535）".to_string())?;
            settings_value(set_http_port(app.clone(), port))
        }
        "set_http_token" => res_unit(set_http_token(app.clone(), ui_str(args, "token")?)),
        "regenerate_http_token" => {
            regenerate_http_token(app.clone()).map(serde_json::Value::String)
        }
        "retry_http" => retry_http(app.clone()).map(serde_json::Value::String),
        "kill_port_occupant" => kill_port_occupant(app.clone()).map(serde_json::Value::String),
        "reveal_http_token" => Ok(serde_json::Value::String(reveal_http_token(app.clone()))),
        "http_access_hint" => Ok(serde_json::Value::String(http_access_hint(app.clone()))),
        _ => Err(format!("未知或不受支持的命令: {cmd}")),
    }
}

/// HTTP / CLI 转发共用的唯一入口：鉴权 → 路由 → 立即执行或转成待执行请求。
pub(crate) fn handle_remote_request(app: &AppHandle, request: &RemoteRequest) -> RemoteReply {
    if !matches!(request.method.as_str(), "GET" | "POST") {
        return RemoteReply::error(405, "method_not_allowed", "只支持 GET / POST");
    }
    // 静态页面在鉴权之前（见 static_ui_reply 注释）
    if request.method == "GET" {
        if let Some(reply) = static_ui_reply(&request.path) {
            return reply;
        }
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
            // 免鉴权探活 + 机器身份：客户端（手机 App / 网页）靠 machineId 认出
            // 「这几条 IP 是同一台电脑」→ 只保留一份页面。不含机密。
            let settings = stored_settings(app);
            return RemoteReply::ok(200, health_json(&settings.machine_id, &machine_name()));
        }
        "/api/status" => return RemoteReply::ok(200, remote_status_json(app)),
        "/api/requests" => return RemoteReply::ok(200, remote_requests_json()),
        "/api/grub" => return RemoteReply::ok(200, remote_grub_json(app)),
        // 网页控制台：完整快照（轮询用）+ 命令通道（真正改设置）
        "/api/ui-state" => {
            let value = serde_json::to_value(get_ui_state(app.clone()))
                .unwrap_or_else(|_| serde_json::json!({}));
            return RemoteReply::ok(200, value);
        }
        "/api/ui-command" => {
            if request.method != "POST" {
                return RemoteReply::error(405, "method_not_allowed", "ui-command 只支持 POST");
            }
            return handle_ui_command_request(app, request);
        }
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
/// `allow_gui_auth` 用 `remoteAllowGuiAuth`（默认 true：远程请求可以弹本机授权框）。
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
    // 这次请求（连同它触发的 GRUB 扫描 / 写 ESP）全程不许弹授权框
    let _no_gui_auth = RemoteNoGuiAuth::enter(allow_gui_auth);
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
        RemoteCommand::SwitchGrub(selector) => {
            let status = switch_grub_and_update(app, selector);
            match status.error {
                Some(error) => (false, error),
                None => (true, format!("下次启动（GRUB）：{selector}")),
            }
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
        match command(&program)
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

/// 前端报上来的高度夹到 [NOTIFY_MIN_HEIGHT, NOTIFY_MAX_HEIGHT]（纯函数，便于单测）。
/// `None`（前端量不出真实内容高度）退回标称高度。
fn notify_height(raw: Option<f64>) -> f64 {
    match raw {
        Some(value) if value.is_finite() && value > 0.0 => {
            value.clamp(NOTIFY_MIN_HEIGHT, NOTIFY_MAX_HEIGHT)
        }
        _ => NOTIFY_HEIGHT,
    }
}

/// 打开/唤起远程管理（server-hub）窗口：已存在就 unminimize + 聚焦，不存在才建。
/// 普通常规窗口（系统标题栏、可缩放、进任务栏），与主窗口的贴边/置顶语义完全无关。
fn open_hub_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(HUB_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        app,
        HUB_WINDOW_LABEL,
        tauri::WebviewUrl::App("hub.html".into()),
    )
    .title("AnyBoot 远程管理")
    .inner_size(1080.0, 720.0)
    .min_inner_size(860.0, 560.0)
    .resizable(true)
    .center()
    .build()
    .map_err(|error| format!("无法创建远程管理窗口: {error}"))?;
    trace(app, "hub", "远程管理窗口已创建");
    Ok(())
}

/// 打开/唤起密钥管理窗（server-keys）：无上下文参数，已存在就聚焦。
pub(crate) fn open_keys_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(HUB_KEYS_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        app,
        HUB_KEYS_WINDOW_LABEL,
        tauri::WebviewUrl::App("hub-keys.html".into()),
    )
    .title("AnyBoot 密钥管理")
    .inner_size(720.0, 560.0)
    .min_inner_size(560.0, 420.0)
    .resizable(true)
    .center()
    .build()
    .map_err(|error| format!("无法创建密钥管理窗口: {error}"))?;
    trace(app, "hub", "密钥管理窗口已创建");
    Ok(())
}

/// 前端按钮入口：hub 工具栏「密钥」
#[tauri::command]
fn hub_open_keys_window(app: AppHandle) -> Result<(), String> {
    open_keys_window(&app)
}

/// 前端按钮入口：hub 详情「打开终端」（按 server_id 查档案 → 开/聚焦终端窗）
#[tauri::command]
fn hub_open_term_window(app: AppHandle, server_id: String) -> Result<(), String> {
    let server = hub::load_servers_pub(&app)?
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("服务器不存在（{server_id}）"))?;
    hub_term::open_term_window(&app, server)
}

/// 前端按钮入口：hub 详情「浏览文件」
#[tauri::command]
fn hub_open_files_window(app: AppHandle, server_id: String) -> Result<(), String> {
    let server = hub::load_servers_pub(&app)?
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("服务器不存在（{server_id}）"))?;
    hub_sftp::open_files_window(&app, server)
}

fn show_notify_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW_LABEL) {
        let _ = window.show();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        app,
        NOTIFY_WINDOW_LABEL,
        tauri::WebviewUrl::App("notify.html".into()),
    )
    .title("远程请求")
    .inner_size(NOTIFY_WIDTH, NOTIFY_HEIGHT)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    // 先建出来但**不显示**：等前端把内容画好、量出真实高度，调 `present_notify_window`
    // 时才定尺寸、定位置、显示。
    //
    // 老实现是「建出来（默认可见）→ set_position」，于是窗口先用系统默认位置画一帧空白
    // （屏幕左上角），再跳到右下角 —— 就是用户看到的「先渲染空白窗口随后才跳过去」。
    // 后来改成「page-load → 定位 → 显示」，但那时的尺寸还是写死的 320 高，
    // 内容只有 200 出头，底下就一直空着一大块。现在尺寸由前端按内容量出来。
    .visible(false)
    .build()
    .map_err(|error| format!("无法创建通知窗口: {error}"))?;

    // 兜底：前端脚本万一没跑起来（弹窗不能就这么消失），800ms 后按标称高度自己露出来。
    let fallback = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(800));
        if let Some(window) = fallback.get_webview_window(NOTIFY_WINDOW_LABEL) {
            if !window.is_visible().unwrap_or(true) {
                trace(&fallback, "notify", "前端 800ms 未量出高度，按标称兜底显示");
                place_notify_window(&fallback, &window, NOTIFY_HEIGHT);
                let _ = window.show();
            }
        }
    });
    Ok(())
}

/// 前端量好高度后「把弹窗摆出来」：定尺寸 → 摆到工作区右下角 → 显示。
///
/// 之所以由前端来触发，是因为只有它知道内容有多高（原生菜单/窗口没有「自适应内容」这回事）。
/// `height` 传 `None` 表示量不出来，用标称高度。
#[tauri::command]
fn present_notify_window(app: AppHandle, height: Option<f64>) -> Result<(), String> {
    let Some(window) = app.get_webview_window(NOTIFY_WINDOW_LABEL) else {
        return Ok(());
    };
    let height = notify_height(height);
    trace(&app, "notify", &format!("present height={height}"));
    let _ = window.set_size(tauri::LogicalSize::new(NOTIFY_WIDTH, height));
    place_notify_window(&app, &window, height);
    let _ = window.show();
    Ok(())
}

/// 把通知窗口摆到「**工作区**右下角、留 NOTIFY_MARGIN 边距」。
///
/// 两个关键点：
/// 1. 用 `work_area()` 而不是 `size()`：任务栏占的那一条不属于工作区，
///    用屏幕尺寸摆就会像以前那样压住任务栏（用户截图里就是这个）。
/// 2. 尺寸用「我们正要设成的逻辑高度」换算，而不是 `outer_size()` 读回来的 ——
///    `set_size` 是投递到主线程的异步操作，设完立刻读回拿到的还是旧尺寸，位置会偏几十像素。
///
/// 刚建出来的窗口 `current_monitor()` 在 Linux 上可能还是 None，回退到主显示器；
/// 都拿不到（极罕见）就保持系统默认位置，不去猜。
fn place_notify_window(app: &AppHandle, window: &tauri::WebviewWindow, logical_height: f64) {
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let width = (NOTIFY_WIDTH * scale).round() as i32;
    let height = (logical_height * scale).round() as i32;
    let margin = (NOTIFY_MARGIN * scale).round() as i32;
    let work = monitor.work_area();
    let _ = window.set_position(tauri::PhysicalPosition::new(
        work.position.x + work.size.width as i32 - width - margin,
        work.position.y + work.size.height as i32 - height - margin,
    ));
}

fn close_notify_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW_LABEL) {
        let _ = window.close();
    }
}

// ——— 自绘托盘菜单（窗口 + 命令）———
//
// 菜单内容由 `build_menu_nodes` 生成（纯数据），这扇窗把它画出来。三端同一份内容、
// 同一个渲染器 ⇒ 同一套外观、同一套行为（用户诉求④）。
// 起这扇窗的直接原因是③：只要菜单由宿主/系统渲染（Linux 的 GtkMenu、Windows 的
// TrackPopupMenu、macOS 的 NSMenu），点任何一项它都会把菜单收掉，且没有任何开关能改。

/// 自绘托盘菜单的运行时状态。
#[derive(Default)]
struct TrayMenuRuntime {
    /// 本次打开的锚点 = **托盘图标的原点**（物理像素）。
    /// Linux 上来自 SNI 的 `Activate(x, y)` —— 实测它给的是图标原点而不是点击点
    /// （点 1776 收到 1742），正好拿来给菜单定位。
    anchor: Option<(f64, f64)>,
    /// 已经呈现出去的高度（逻辑像素）
    presented: Option<f64>,
    /// 本次弹出的时刻（见 `TRAY_MENU_BLUR_GRACE_MS`）
    opened_at: Option<Instant>,
    /// 每次「开菜单」自增，随内容一起发给前端 —— 前端据此知道「这是新开的一次」，
    /// 哪怕高度与上次一样也要重新呈现（不然重开菜单时窗口永远不显示）
    seq: u64,
    /// 弹出后是否**真的**拿到过焦点（失焦看门狗用，见 `spawn_tray_menu_watchdog`）：
    /// 拿到过又没了 = 真失焦，但事件可能被假失焦宽限吞掉或干脆没送达。
    focus_seen: bool,
    /// 连续无焦点的起点（看门狗去抖：稳定无焦点才收，别跟点击本窗抢焦点的竞态打架）
    unfocused_since: Option<Instant>,
    /// 「始终没拿到焦点」时是否已重试过一次 set_focus
    focus_retried: bool,
}

/// 摸一下菜单运行时状态。
///
/// 不返回 guard：`app.state::<T>()` 给的是一个临时值，`.lock()` 借它，出了函数就没了
/// （E0515）。所以这里收一个闭包代替返回 guard —— 与项目里其它几处 `state.lock()` 同款做法。
fn with_tray_menu_runtime<R>(app: &AppHandle, use_it: impl FnOnce(&mut TrayMenuRuntime) -> R) -> R {
    let state = app.state::<Mutex<TrayMenuRuntime>>();
    let mut guard = state.lock().expect("tray menu mutex poisoned");
    use_it(&mut guard)
}

/// 发给前端的一整份菜单内容。
#[derive(Clone, Serialize)]
struct TrayMenuPayload {
    /// light / dark（与主界面同一套取值）
    theme: String,
    /// 本次「开菜单」的序号（见 `TrayMenuRuntime::seq`）
    seq: u64,
    nodes: Vec<MenuNode>,
}

/// 前端报上来的高度夹到区间里（纯函数，便于单测）：量不出来就用标称高度兜底。
fn tray_menu_height(raw: Option<f64>) -> f64 {
    match raw {
        Some(value) if value.is_finite() && value > 0.0 => {
            value.clamp(TRAY_MENU_MIN_HEIGHT, TRAY_MENU_MAX_HEIGHT)
        }
        _ => TRAY_MENU_NOMINAL_HEIGHT,
    }
}

/// 菜单窗高度上限（逻辑像素）：锚点所在显示器**工作区**高减去上下各一个间隙。
/// 工作区已扣掉任务栏/面板，所以这个上限保证窗口（连同上下的摆放余量）完整落在可视区内。
/// 锚点还没记（窗口没开过）或显示器拿不到时回 None —— 不封顶，交给既有逻辑。
fn tray_menu_max_height(app: &AppHandle) -> Option<f64> {
    let (anchor_x, anchor_y) = with_tray_menu_runtime(app, |runtime| runtime.anchor)?;
    let monitor = app
        .monitor_from_point(anchor_x, anchor_y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten())?;
    let scale = monitor.scale_factor();
    let work = monitor.work_area();
    let margin = (TRAY_MENU_GAP * 2.0 * scale).round() as i32;
    let height = (work.size.height as i32 - margin).max(0);
    Some(height as f64 / scale)
}

/// 内容高度按上限截断（纯函数，便于单测）：上限缺位或非法就不截。
fn clamp_tray_menu_height(height: f64, max: Option<f64>) -> f64 {
    match max {
        Some(max) if max.is_finite() && max > 0.0 => height.min(max),
        _ => height,
    }
}

/// 打开托盘菜单：记下锚点、把最新内容推给菜单窗（窗口不在就现建一个，建完前端会来拉）。
///
/// `x` / `y` 是**托盘图标的原点**（物理像素）。
fn open_tray_menu(app: &AppHandle, x: f64, y: f64) {
    with_tray_menu_runtime(app, |runtime| {
        runtime.anchor = Some((x, y));
        runtime.presented = None;
        runtime.seq = runtime.seq.wrapping_add(1);
        // 新开的一次：焦点账本清零（看门狗字段，见 spawn_tray_menu_watchdog）
        runtime.focus_seen = false;
        runtime.unfocused_since = None;
        runtime.focus_retried = false;
    });
    trace(app, "tray-menu", &format!("打开菜单 锚点=({x:.0},{y:.0})"));
    if let Some(window) = app.get_webview_window(TRAY_MENU_WINDOW_LABEL) {
        trace(app, "tray-menu", "复用已有菜单窗，推最新内容");
        push_tray_menu_nodes(app, &window);
        return;
    }
    match build_tray_menu_window(app) {
        Ok(()) => trace(app, "tray-menu", "菜单窗已建（隐藏态），等前端量高"),
        Err(error) => trace(app, "tray-menu", &format!("菜单窗创建失败：{error}")),
    }
}

/// 建菜单窗（隐藏态）：透明、无边框、置顶、不进任务栏。
/// 先不显示 —— 等前端量出真实内容高度回调 `tray_menu_present` 时才定尺寸、定位置、显示
/// （与 `notify` 那扇窗同一套约定，理由一样：只有前端知道内容有多高）。
fn build_tray_menu_window(app: &AppHandle) -> tauri::Result<()> {
    tauri::WebviewWindowBuilder::new(
        app,
        TRAY_MENU_WINDOW_LABEL,
        tauri::WebviewUrl::App("tray-menu.html".into()),
    )
    .title("AnyBoot")
    .inner_size(TRAY_MENU_WIDTH, TRAY_MENU_NOMINAL_HEIGHT)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    // 页面加载事件只用来取证：「开菜单没反应」时看这行在不在，
    // 能把「页面没加载」和「页面加载了但 JS 没跑/没报高度」分开。
    .on_page_load(|window, payload| {
        trace(
            window.app_handle(),
            "tray-menu",
            &format!("页面加载事件 {:?} uri={}", payload.event(), payload.url()),
        );
    })
    .build()?;
    Ok(())
}

/// 把当前菜单内容推给菜单窗（前端收到就重画；高度变了会再回调 `tray_menu_present`）。
fn push_tray_menu_nodes(app: &AppHandle, window: &tauri::WebviewWindow) {
    let state = ui_state(app);
    emit_tray_menu_nodes(app, window, &state);
}

/// 同上，但用调用方手里那份快照（`update_tray_menu` 已经有一份，不必再取一次）。
fn emit_tray_menu_nodes(app: &AppHandle, window: &tauri::WebviewWindow, state: &UiState) {
    let payload = TrayMenuPayload {
        theme: normalize_theme(&state.settings.settings.theme),
        seq: with_tray_menu_runtime(app, |runtime| runtime.seq),
        nodes: build_menu_nodes(state),
    };
    if let Err(error) = window.emit("tray-menu-nodes", payload) {
        trace(app, "tray-menu", &format!("内容推送失败：{error}"));
    }
}

/// 收起菜单。窗口**不销毁**（下次打开直接用，省掉重新加载页面那一下）。
fn close_tray_menu(app: &AppHandle) {
    let Some(window) = app.get_webview_window(TRAY_MENU_WINDOW_LABEL) else {
        return;
    };
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    startup_debug(app, "托盘菜单收起");
    with_tray_menu_runtime(app, |runtime| {
        runtime.presented = None;
        runtime.opened_at = None;
        runtime.focus_seen = false;
        runtime.unfocused_since = None;
        runtime.focus_retried = false;
    });
    let _ = window.hide();
}

/// 这次失焦是不是「刚弹出来那一下」的假失焦（WM 还没把焦点交过来就先报了一次）。
fn tray_menu_blur_is_fake(app: &AppHandle) -> bool {
    with_tray_menu_runtime(app, |runtime| match runtime.opened_at {
        Some(at) => at.elapsed() < Duration::from_millis(TRAY_MENU_BLUR_GRACE_MS),
        None => false,
    })
}

/// 托盘图标尺寸拿不到实值时的标称（物理像素）：菜单落位用它估「图标下沿/上沿」，
/// 与 `feed_positioner` 喂给 positioner 的标称矩形同一套近似。
const TRAY_MENU_ICON_NOMINAL: f64 = 32.0;

/// 自绘托盘菜单的**失焦看门狗**：收起菜单的主路径是 `Focused(false)` 事件，但它有两个
/// 已知漏点 ——①刚弹出的假失焦被宽限吞掉时焦点其实已经丢了，后面**不会再有**失焦事件；
/// ②`set_focus` 偶尔没成功，窗从头到尾没焦点，失焦事件自然一次都不来。两种都表现为
/// 「点外面菜单挂在那里怎么都不消失」（用户实机偶发）。所以菜单开着时每 250ms 摸一次
/// 焦点状态兜底：
/// * 拿到过焦点又没了 → 稳定 400ms 仍无焦点就收（去抖是给「点击本窗重新拿焦点」的
///   竞态留余量 —— GNOME 点击必聚焦，用户点在菜单上时这里不会误判）；
/// * 从头没拿到焦点 → 宽限期后重试一次 `set_focus`；再过 4 秒还没拿到也收 ——
///   还是那个道理：用户点在菜单上窗口必然有焦点，持续没焦点说明人已点去别处。
#[cfg(desktop)]
fn spawn_tray_menu_watchdog(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));
        let Some(window) = app.get_webview_window(TRAY_MENU_WINDOW_LABEL) else {
            continue;
        };
        let Ok(visible) = window.is_visible() else {
            continue;
        };
        if !visible {
            continue;
        }
        let focused = window.is_focused().unwrap_or(true);
        let verdict = with_tray_menu_runtime(&app, |runtime| {
            if runtime.opened_at.is_none() {
                return None;
            }
            if focused {
                runtime.focus_seen = true;
                runtime.unfocused_since = None;
                return None;
            }
            let since = *runtime.unfocused_since.get_or_insert(Instant::now());
            if runtime.focus_seen {
                return (since.elapsed() >= Duration::from_millis(400))
                    .then_some("拿到过焦点后失焦");
            }
            let opened = runtime.opened_at.expect("opened_at 刚判过非空");
            if !runtime.focus_retried
                && opened.elapsed() >= Duration::from_millis(TRAY_MENU_BLUR_GRACE_MS)
            {
                runtime.focus_retried = true;
                let _ = window.set_focus();
                return None;
            }
            (opened.elapsed() >= Duration::from_secs(4)).then_some("始终没拿到焦点")
        });
        if let Some(reason) = verdict {
            startup_debug(&app, &format!("托盘菜单看门狗收起（{reason}）"));
            close_tray_menu(&app);
        }
    });
}

/// 托盘菜单方式是不是 unified（每次读设置，切了即刻生效于下一次弹菜单）。
fn tray_menu_style_is_unified(app: &AppHandle) -> bool {
    normalize_tray_menu_style(&stored_settings(app).tray_menu_style) == "unified"
}

/// 托盘菜单的落位：把菜单摆到托盘图标旁边（两种方式共用同一套几何代码）。
///
/// 竖着：
/// * classic（各自方式）：图标上方放得下就**往上摆**（面板在屏幕底部时走的就是这条，
///   菜单下沿贴着图标上沿）；放不下才往下。
/// * unified（全局统一）：反过来 —— **优先贴图标正下方**（菜单上沿贴图标下沿+缝）；
///   图标在屏幕底部放不下 → 翻到**正上方**（菜单下沿贴图标上沿-缝）。两种都**不盖图标**。
/// 横着：classic 菜单左边对齐图标左边；unified 对图标**水平居中** —— 再都夹进工作区
/// （用工作区而不是屏幕，免得菜单被任务栏/面板盖掉一截）。
///
/// 图标矩形拿不到实值，用 `TRAY_MENU_ICON_NOMINAL` 标称（与 feed_positioner 同一套近似）；
/// 尺寸用「我们正要设成的逻辑高度」换算，不读回 outer_size：set_size 是投递到主线程的
/// 异步操作，设完立刻读回还是旧尺寸，位置会偏出去（place_notify_window 同款坑）。
fn place_tray_menu(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    logical_height: f64,
    unified: bool,
) {
    let Some((anchor_x, anchor_y)) = with_tray_menu_runtime(app, |runtime| runtime.anchor) else {
        return;
    };
    let monitor = app
        .monitor_from_point(anchor_x, anchor_y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    // 尺寸用「我们正要设成的逻辑高度」换算，不读回 outer_size：set_size 是投递到主线程
    // 的异步操作，设完立刻读回还是旧尺寸，位置会偏出去（place_notify_window 同款坑）。
    let scale = monitor.scale_factor();
    let width = (TRAY_MENU_WIDTH * scale).round() as i32;
    let height = (logical_height * scale).round() as i32;
    let gap = (TRAY_MENU_GAP * scale).round() as i32;
    let work = monitor.work_area();
    let left = work.position.x;
    let top = work.position.y;
    let right = left + work.size.width as i32;
    let bottom = top + work.size.height as i32;
    let anchor_x = anchor_x.round() as i32;
    let anchor_y = anchor_y.round() as i32;
    let icon = TRAY_MENU_ICON_NOMINAL as i32;
    // 竖着：unified 优先「图标下沿+缝」往下摆，放不下翻到「图标上沿-缝」上面去；
    // classic 沿用「先上后下」。两条路径都保证菜单与图标矩形不相交（不盖托盘）。
    let y = if unified {
        let below = anchor_y + icon + gap;
        if below + height <= bottom {
            below
        } else {
            let above = anchor_y - gap - height;
            if above >= top {
                above
            } else {
                below.min(bottom - height).max(top)
            }
        }
    } else {
        let above = anchor_y - gap - height;
        if above >= top {
            above
        } else {
            (anchor_y + icon + gap).min(bottom - height).max(top)
        }
    };
    // 横着：unified 对图标水平居中，classic 左边对齐图标左边 —— 都夹进工作区
    let x = if unified {
        anchor_x + icon / 2 - width / 2
    } else {
        anchor_x
    }
    .clamp(left, (right - width).max(left));
    // 定位全程留痕（常开通道）：「菜单离图标很远」这类报告，靠这一行就能分清是
    // 锚点坐标空间不对（宿主给逻辑坐标）、显示器/工作区解析错、还是换算错。
    startup_debug(
        app,
        &format!(
            "菜单定位({}) 锚点=({anchor_x},{anchor_y}) 显示器={:?} rect=({},{},{},{}) 工作区=({left},{top})-({right},{bottom}) scale={scale} 菜单={width}x{height} → ({x},{y})",
            if unified { "unified" } else { "classic" },
            monitor.name().map(String::as_str).unwrap_or(""),
            monitor.position().x,
            monitor.position().y,
            monitor.size().width,
            monitor.size().height,
        ),
    );
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

/// 前端量好内容高度后「把菜单摆出来」：定尺寸 → 定位（方式由 trayMenuStyle 决定）→ 显示 → 拿焦点。
#[tauri::command]
fn tray_menu_present(app: AppHandle, height: Option<f64>) -> Result<(), String> {
    let Some(window) = app.get_webview_window(TRAY_MENU_WINDOW_LABEL) else {
        trace(&app, "tray-menu", "呈现被忽略：菜单窗不在");
        return Ok(());
    };
    trace(&app, "tray-menu", &format!("前端报高 {height:?}"));
    let height = tray_menu_height(height);
    // 封顶：内容再长也不能把窗口开得比工作区还高 —— 窗口 = 内容高时没有滚动可言，
    // 超出的部分直接顶出屏幕。封顶后多出的内容由 `.menu` 自己的滚动条看
    // （tray-menu.css 本来就是 height:100% + overflow-y:auto，滚轮直接可用）。
    let height = clamp_tray_menu_height(height, tray_menu_max_height(&app));
    let needs_presenting = with_tray_menu_runtime(&app, |runtime| {
        // 已经按这个高度摆着、而且就在眼前 —— 不必再折腾一次（内容没变时的重复回调）
        if runtime.presented == Some(height) && window.is_visible().unwrap_or(false) {
            return false;
        }
        runtime.presented = Some(height);
        runtime.opened_at = Some(Instant::now());
        true
    });
    if !needs_presenting {
        return Ok(());
    }
    trace(&app, "tray-menu", &format!("呈现 height={height}"));
    // GTK3 定则：`set_resizable(false)` 会把窗口尺寸**钉在子控件最小尺寸**上
    // （WebKitWebView ≈200×200 逻辑像素），之后 set_size 会被夹回来 —— 菜单矮于 200px 时
    // 就会多出一大块空白。所以要缩就先临时放开，**回锁排在几何请求之后**。
    let _ = window.set_resizable(true);
    let _ = window.set_size(tauri::LogicalSize::new(TRAY_MENU_WIDTH, height));
    let _ = window.set_resizable(false);
    if tray_menu_style_is_unified(&app) {
        // unified：贴托盘图标**正下方**（水平对图标居中）；图标在屏幕底部放不下
        // → 翻到**正上方**。不再交给 positioner 的 TrayCenter —— 它固定往上摆、
        // Linux 不翻面，与「优先下方 + 不盖托盘」的要求相反（用户定案）。
        // positioner 插件与喂点（feed_positioner / on_tray_icon_event）保留不动，
        // 只是落位不再走它。
        place_tray_menu(&app, &window, height, true);
    } else {
        // classic（默认）：三端各自方式 —— 锚点已由 open_tray_menu 记进 runtime，
        // 落位按所在显示器工作区手算夹取，不依赖 positioner。
        place_tray_menu(&app, &window, height, false);
    }
    let _ = window.show();
    let _ = window.set_focus();
    Ok(())
}

/// 前端点了一条：分发动作，并按模型决定关不关窗。
///
/// 分发走的是与原生菜单**完全相同**的 `handle_menu_event`（id 语义一字不变）。
#[tauri::command]
fn tray_menu_click(app: AppHandle, id: String) {
    let keep_open = menu_node_keeps_open(&build_menu_nodes(&ui_state(&app)), &id);
    handle_menu_event(&app, &id);
    if !keep_open {
        close_tray_menu(&app);
        return;
    }
    // 留着菜单：内容会由这个动作引发的 publish 推新（见 update_tray_menu）。
    // 这里不主动推 —— 动作大多在工作线程上落盘，此刻推上去的还是旧状态，只会闪一下。
}

/// 前端按 Esc：收起。
#[tauri::command]
fn tray_menu_close(app: AppHandle) {
    close_tray_menu(&app);
}

/// 前端加载完（或菜单窗刚建出来）主动拉一次：拿当前内容与主题。
#[tauri::command]
fn tray_menu_nodes(app: AppHandle) -> TrayMenuPayload {
    // 这是菜单窗前端**跑起来了**的唯一确证（页面加载 → JS 执行 → 调这个命令）。
    // 「开菜单没反应」时先看这行在不在，能把「窗没建出来」和「前端没跑/没报高度」分开。
    trace(&app, "tray-menu", "前端已加载，拉取菜单内容");
    let state = ui_state(&app);
    TrayMenuPayload {
        theme: normalize_theme(&state.settings.settings.theme),
        seq: with_tray_menu_runtime(&app, |runtime| runtime.seq),
        nodes: build_menu_nodes(&state),
    }
}

/// Windows 把最小化窗口挪到屏幕外 (−32000,−32000)，按这个特征识别最小化态。
fn is_minimized_position(pos: tauri::PhysicalPosition<i32>) -> bool {
    pos.x < -20000 || pos.y < -20000
}

/// 挑一个**可用**的窗口尺寸读数（纯函数）：优先外框，读不出来退到内尺寸。
///
/// 未映射的窗口 `outer_size()` 读到 `0x0`（tao 的初值取的是 `root_origin()`，
/// 是个坐标）—— 实测 `main_outer=0x0 main_inner=Ok("460x900")`。内尺寸在未映射时
/// 就可读（= builder 请求值），退到它只差一个标题栏（本机实测 37px）。
/// 两个用途都踩过这个坑，所以放在一处：
/// * GRUB 独立窗的初始尺寸 —— 拿 0 当基准会算出 `440x320`（正确 `460x937`），
///   窗口 show 出来要再过 ~76ms 才被对齐线程纠正 = 「独立窗先闪一下」；
/// * 主窗**居中**的基准 —— 拿 0 当基准等于「把 0 尺寸的矩形居中」，落点
///   `(800,500)` 对 460x937 的窗口是屏幕外，WM 一夹就贴到底边 → 启动约 1s 后
///   自己贴边成条（见 `center_main_window`）。
fn usable_window_size_basis(
    main_outer: Option<(u32, u32)>,
    main_inner: Option<(u32, u32)>,
) -> Option<(u32, u32)> {
    fn usable(size: &(u32, u32)) -> bool {
        size.0 > 0 && size.1 > 0
    }
    main_outer
        .filter(usable)
        .or_else(|| main_inner.filter(usable))
}

/// 几何稳定抑制期（Unix 毫秒）：dock（缩条）/ undock（展开）会连续改窗口
/// 尺寸与位置，这段过渡期里读到的坐标/尺寸都是中间态，拿它做判定就是在
/// 跟自己的几何编程打架（贴边成条→立刻被弹回的互闪就是这么来的）。
static GEOMETRY_BUSY_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

/// 几何稳定抑制期的长度（毫秒）。选 400：配置事件来回通常 < 30ms，留足
/// WM 处理去装饰/重设边框的余量，又短到用户察觉不到。
const AUTO_HIDE_GEOMETRY_BUSY_MS: u64 = 400;

/// 每次停靠最多自纠几次。自纠 = 把窗口重新钉回 dock_pos，正常平台一次就够
/// （Windows 上根本不会触发）；万一某个 WM 就是不肯把窗口放在指定坐标，
/// 也不能让 120ms 的轮询把它变成永久的 set_position 抖动。
const AUTO_HIDE_RESNAP_LIMIT: u8 = 3;

/// 「窗口几何还没落定，先别贴边」的宽限（毫秒）。`show_main_window` 的两个
/// 来源都需要它，理由各不相同：
/// · 呼出：点击瞬间鼠标必然停在托盘/菜单上（窗口外）。
/// · 启动首次显示：窗口要经过**两轮**落位 —— ① 我们在映射前按客户区尺寸居中
///   （那时 `outer_size()` 还是 `0x0`，只能拿内尺寸当基准），② WM 按自己的记录
///   摆一次、随后我们按**真实窗框**重新居中。实测（reg3-nudge/reg3-ledge）：
///   ① 落点 y=50、真实窗框 460x937 → 窗底 987，离工作区底(1000)**只剩 13px**
///   ≤ 触发距离 24 → `near_edge=true`；② 的重新居中在 **+1.2s** 才到，而旧值
///   800ms 的宽限在 +800ms 就过期 → **启动约 1s 后自己贴边成条**
///   （`dock begin edge=Bottom from=(570,50)`，用户看到的就是「一打开就变成一条」）。
/// 取 1500 盖住 ② 的实测到达时间并留余量。启动**不挂**「等待进入」标志，
/// 否则用户在碰过窗口之前永远等不到贴边（哑火）。
const AUTO_HIDE_SETTLE_MS: u64 = 1500;

/// 是否处于几何抑制期内：刚编程过窗口几何（dock / undock / 白条自纠），
/// 这段时间里读到的窗口坐标、收到的 Moved 都可能是「还没落到位的中间态」，
/// 不能拿来做判断。
///
/// Windows 上「去装饰」与窗口坐标是两套独立的东西，dock_pos 的 24px 容差
/// 足够认出自家的回声；**Linux(X11) 上 `set_decorations(false)` 会让 GDK 的
/// frame extents 与 WM 的实际边框状态错一拍** —— 去装饰请求和随后的
/// set_position 几乎同时发出，GDK 仍按「带标题栏」的偏移去算 move 与
/// root_origin，于是这次 set_position 的回声坐标能偏出一整个标题栏（36px+，
/// 用户机上是 Cinnamon/Muffin + WhiteSur 主题），超出 24px 容差 →
/// `handle_main_moved` 把它当成「用户把白条拖离了」→ 贴边刚成条就被弹回来
/// （实测 startup-debug.log：`docked=true` 之后 7ms 就 `docked=false`）。
/// 抑制期过完 GDK/WM 已经一致，那时的位置比对与自纠才是准的。
fn geometry_busy() -> bool {
    unix_millis() < GEOMETRY_BUSY_UNTIL_MS.load(Ordering::SeqCst)
}

/// 进入几何抑制期。必须在**发出**几何请求之前调用：请求是同步发出去的、
/// 配置事件是异步回来的，立晚了那批回声就落在抑制期外。
fn mark_geometry_busy(duration_ms: u64) {
    GEOMETRY_BUSY_UNTIL_MS.store(unix_millis() + duration_ms, Ordering::SeqCst);
}

/// 最近一条**主窗** Moved 的时间戳（Unix 毫秒，0 = 还没有过）。每次进
/// `handle_main_moved` 都盖一次。判据见 `main_moved_recently`。
static LAST_MAIN_MOVED_MS: AtomicU64 = AtomicU64::new(0);

/// 「屏幕中间」落位模式的拖动拦截（用户定案 2026-10-13）：居中模式下窗口
/// 位置固定、不支持拖动。CENTER_LAST_X/Y 记最近一次居中落点的**请求值**；
/// Moved 载荷落在它的 `CENTER_HOLD_TOLERANCE` 内就是「还停在居中位」（我们自己
/// 的回声、WM 抖动、去装饰后的标题栏错拍），超出才是用户把它拖走了 → 交给
/// `schedule_center_snap` 在拖动静默后收回。这里刻意**不用**「居中后 N 毫秒
/// 一律忽略」的时间窗：那会把用户在这段窗口期里的真拖动一起吃下去，松手既不
/// 回中也不提示（见 CENTER_HOLD_TOLERANCE 的注释）。
static CENTER_LAST_X: AtomicI32 = AtomicI32::new(0);
static CENTER_LAST_Y: AtomicI32 = AtomicI32::new(0);
/// 我们是否已经居中过至少一次：没有的话 Moved 一律不管（启动早期 WM 自己
/// 摆窗口的 Moved 不能当成「用户硬拖」去弹提示）。
static CENTER_HAS_POS: AtomicBool = AtomicBool::new(false);
/// 拖动提示的节流：拖动全程 Moved 连发，提示最多 4 秒一条。
static LAST_CENTER_HINT_MS: AtomicU64 = AtomicU64::new(0);

/// 主窗是不是「刚刚还在动」（距最近一条 Moved < `AUTO_HIDE_MOVE_QUIET_MS`）。
///
/// 用来挡住「拖动途中就贴边」：用户拖着窗口/拖着白条时 Moved 连发，这一拍
/// watcher 读到的「光标在窗外」是拖动的中间态，不是用户离开了窗口。
/// 实测（poke-1）：抓白条往工作区里拖 → 松手瞬间窗口展开回 460x900 →
/// watcher 176ms 后就把它缩回条子（主窗闪一下 + GRUB 窗 129ms 内翻两次）。
fn main_moved_recently() -> bool {
    let last = LAST_MAIN_MOVED_MS.load(Ordering::SeqCst);
    last != 0 && unix_millis().saturating_sub(last) < AUTO_HIDE_MOVE_QUIET_MS
}

fn main_auto_hide_enabled(app: &AppHandle) -> bool {
    stored_settings(app).main_auto_hide
}

/// 贴边自动隐藏在什么条件下才真正生效：开关开着，且落位方式**不是**「屏幕中间」。
///
/// 两者互斥（用户定案 2026-10-14）：居中要求窗口钉在屏幕正中，贴边要求窗口缩到
/// 屏幕边上 —— 叠在一起就是「贴边 watcher 把窗口搬到边上 → Moved 触发居中回弹
/// → 回到正中后 watcher 又觉得该贴边」的互闪，用户在贴边位与居中位之间来回抖动。
/// 设置值**不改**（用户在别的方式下选的开关保留着），只在居中模式下整体停用；
/// 切回其他落位方式立刻恢复。
///
/// 拦截点只有一个：`dock_main_window` 开头的闸（所有贴边路径的唯一入口）。
/// watcher 线程本身照常活着 —— 它同时承担 undock 落点的重锚收敛，而「停靠态切
/// 到居中」这条路径正需要它把窗口从贴边落点收敛到屏幕正中（见 center_docked_window）。
fn auto_hide_should_run(auto_hide_enabled: bool, position_mode: &str) -> bool {
    auto_hide_enabled && normalize_window_position(position_mode) != "center"
}

fn auto_hide_active(app: &AppHandle) -> bool {
    let settings = stored_settings(app);
    auto_hide_should_run(settings.main_auto_hide, &settings.main_window_position)
}

/// 按需拉起贴边 watcher：居中模式（互斥）下不起 —— 起了也只会在 `dock_main_window`
/// 的闸前空转。切回其他落位方式时由 `set_main_window_position` 再调一次。
fn ensure_auto_hide_watcher(app: &AppHandle) {
    if auto_hide_active(app) {
        start_auto_hide_watcher(app);
    }
}

/// 算贴边缩条后的窗口尺寸与位置（物理像素）。
/// 白条 = 主窗口贴边后露出的那条**窗口边界**（不是屏幕边）：贴上下边时
/// 露出主窗顶/底边（水平，长 = 主窗 client 宽），贴左右边时露出主窗
/// 左/右缘（垂直，长 = 主窗 client 高）。白条长度百分比由前端 CSS 按这条
/// 窗口边长控制。这里只把窗口垂直贴边方向缩成 strip 厚度、沿边方向保持
/// 主窗原边长与原位置。返回 (new_size, target)：new_size 是 inner 尺寸
/// （去装饰后 ≈ 外框）。
/// 展开（undock）时窗口该落到哪 —— 一律 **客户区**坐标（纯函数，便于单测）。
///
/// 「贴回工作区边缘」= 让整窗贴住工作区的那条边：顶边客户区 y 落在工作区顶
/// （标题栏留在工作区外，展开后不压内容）；底边客户区底边落在工作区底；
/// 左右同理。另一轴保持客户区当前坐标不动。
///
/// 必须按**客户区**算：三端 `set_position` 带装饰时都落「窗框原点」（客户区
/// = 请求 + 边距），`outer_position()` 也是窗框坐标，两者差一个标题栏
/// （Linux 实测 37px）。原来这套算式按窗框算，于是展开后整窗偏高一个标题栏
/// （实测客户区落到 63，本该 100）。undock 落位时按实测边距（inner−outer）
/// 扣掉这段差值，重锚牛顿步兜底校准残差。
///
/// **顶边必须给标题栏让位**（`frame_top` = 窗框顶 − 客户区顶）：顶边贴边
/// 展开时若客户区顶直接贴工作区顶，标题栏（客户区上方）就悬在工作区外
/// —— Windows/macOS 上标题栏在屏幕顶之外**完全看不见**、窗口拖不动
/// （三端一致修的 bug）。让位后窗框顶 = 工作区顶，标题栏可见可拖；
/// 客户区往下让出一个标题栏高度，其余三边不用让（窗框顶必然在工作区内）。
fn undock_target(
    edge: DockEdge,
    work_x: i32,
    work_y: i32,
    work_w: i32,
    work_h: i32,
    client_x: i32,
    client_y: i32,
    client_w: i32,
    client_h: i32,
    frame_top: i32,
) -> (i32, i32) {
    match edge {
        DockEdge::Top => (client_x, work_y + frame_top.max(0)),
        DockEdge::Bottom => (client_x, work_y + work_h - client_h),
        DockEdge::Left => (work_x, client_y),
        DockEdge::Right => (work_x + work_w - client_w, client_y),
    }
}

/// 重贴边的选边：上一轮贴的边只要**没有被另一条边严格更近**就沿用。
///
/// 为什么不直接 nearest_edge：展开后的窗口很高（900），贴着左缘时离底缘往往
/// 也只剩个位数像素 —— 实测（dock-cycle 复现台）：Left undock 后 Muffin 重装饰
/// 时把 client y 97 掰到 100，窗框 63+937=1000 恰好贴死工作区底缘，Left/Bottom
/// 距离打平，nearest_edge 的数组序让 Bottom 赢 → 左右贴边开合几轮后翻成底边
/// 贴边（用户实测「反复打开最后都变成底面贴边」）。距离打平 = 没有换边的理由，
/// 沿用上一轮；用户真把窗口拖向另一条边（严格更近）才跟着换。
/// 坐标用**客户区**（本项目判位唯一可信来源）；首次贴边（None）退回
/// nearest_edge 纯几何。纯函数，坐标无关语义。
fn redock_edge(
    last: Option<DockEdge>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    work_x: i32,
    work_y: i32,
    work_w: i32,
    work_h: i32,
) -> DockEdge {
    let Some(last) = last else {
        return nearest_edge(x, y, w, h, work_x, work_y, work_w, work_h);
    };
    let distance =
        |edge: DockEdge| -> i32 {
            match edge {
                DockEdge::Top => y - work_y,
                DockEdge::Bottom => (work_y + work_h) - (y + h),
                DockEdge::Left => x - work_x,
                DockEdge::Right => (work_x + work_w) - (x + w),
            }
        };
    let nearest = nearest_edge(x, y, w, h, work_x, work_y, work_w, work_h);
    if nearest == last || distance(nearest).abs() >= distance(last).abs() {
        last
    } else {
        nearest
    }
}

/// 沿边轴的停靠坐标：当前读数与上一轮停靠位差 ≤ tolerance 时视为 WM 重装饰
/// 微调（Muffin 实测每次重装饰 +3px），沿用上一轮的值 —— 不吸收的话微调每轮
/// 进 dock_pos、undock 再放出来，左右贴边每开合一次窗口就下移一点（无界爬移）。
/// 差得多 = 用户真挪过，尊重当前读数。纯函数，便于单测。
fn dock_axis_position(live: i32, remembered: i32, tolerance: i32) -> i32 {
    if (live - remembered).abs() <= tolerance {
        remembered
    } else {
        live
    }
}

/// undock 重锚计划：展开的期望客户区落点 + 当前请求修正量 + 已补发次数。
///
/// 请求修正量的由来：装饰态窗口上 `set_position` 的坐标是**窗框**原点
/// （GTK gravity 语义），客户区实际落在 请求 + 标题栏（真机 2x 缩放 64px）；
/// 停靠态窗口去装饰后窗框=客户区，不受影响。修正量 = 请求里预先扣掉的
/// 那部分，由重锚的牛顿步自校准学出（见 undock_reanchor_action）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AutoHideReanchor {
    target: (i32, i32),
    offset: (i32, i32),
    tries: u8,
    /// 上一拍读到的实况。牛顿步的防失控保险丝：连续两拍实况纹丝不动 =
    /// 补发的 set_position 被 WM 钳制/忽略，再叠修正量只会让请求越飞越远
    /// （多屏桌面上会真落到远处的屏上，实测 offset 一路加到 11520、请求
    /// 飞到 15416）—— 见 undock_reanchor_action 的 GiveUp 分支。
    last_live: Option<(i32, i32)>,
    /// 实况连续没动的拍数（≥2 放弃）。
    noops: u8,
}

/// 重锚一拍的裁决（纯函数，便于单测）。
#[derive(Debug, PartialEq, Eq)]
enum UndockReanchor {
    /// 实况偏离期望落点超过 WM 微调容差、还有补发额度 → 按牛顿步更新
    /// 修正量并补发一次 set_position（坐标 = target + 新修正量）。
    Set(AutoHideReanchor, (i32, i32)),
    /// 已落到位（≤ AUTO_HIDE_WM_NUDGE_PX）→ 清计划（修正量已持久化）。
    Settled,
    /// 补发额度用完仍不收敛 → 清计划，位置交给用户/白条保障接管。
    GiveUp,
}

/// undock 重锚决策（牛顿式自校准）：不假设 WM 的落位语义（窗框/客户区、
/// 缓存错拍、钳制……），只认「请求 R 一次 → 实况 O」的线性响应：实测偏差
/// `O − (target+offset)` 全额反扣进修正量。真机（Cinnamon 2x）：请求 428
/// 实况 492 → 修正 −64 → 请求 364 → 实况 428 = 期望，一步收敛。复现台
/// （标题栏 0px）偏差 +3 在容差内 → 整块零动作。Windows 落点精确 → 首拍
/// 即收敛，行为不变。
fn undock_reanchor_action(plan: AutoHideReanchor, live_x: i32, live_y: i32) -> UndockReanchor {
    // 收敛判据是**期望客户区落点**（target），不是请求值（请求 = target +
    // offset 是窗框坐标，天然差一个标题栏）。实况 == target 就是落准了。
    let off_x = (live_x - plan.target.0).abs();
    let off_y = (live_y - plan.target.1).abs();
    if off_x <= AUTO_HIDE_WM_NUDGE_PX && off_y <= AUTO_HIDE_WM_NUDGE_PX {
        return UndockReanchor::Settled;
    }
    if plan.tries >= AUTO_HIDE_REANCHOR_TRIES {
        return UndockReanchor::GiveUp;
    }
    // 防失控保险丝：上一拍已经补发过一次、这一拍实况还停在原地 = 请求被
    // WM 钳制/忽略（典型：target 落在窗口够不到的区域）。此时**不再叠**
    // 修正量（叠了就是把请求往更远处推），照发同一请求再等一拍；连续两拍
    // 没动静就放弃 —— 牛顿步只负责校准「WM 肯配合」的偏差，追不动就止损。
    let noop = plan.last_live == Some((live_x, live_y));
    let noops = if noop { plan.noops + 1 } else { 0 };
    if noops >= 2 {
        return UndockReanchor::GiveUp;
    }
    let offset = if noop {
        plan.offset
    } else {
        (
            plan.offset.0 + (plan.target.0 - live_x),
            plan.offset.1 + (plan.target.1 - live_y),
        )
    };
    UndockReanchor::Set(
        AutoHideReanchor {
            target: plan.target,
            offset,
            tries: plan.tries + 1,
            last_live: Some((live_x, live_y)),
            noops,
        },
        (plan.target.0 + offset.0, plan.target.1 + offset.1),
    )
}

/// 贴边缩条时的几何：窗口缩成 `strip` 厚的细缝，贴到工作区最近的边；
/// `gap` = 白条沿法线方向离这条边的距离（0 = 紧贴，既有行为）。
/// 返回的落点是 **客户区**坐标 —— 和 `set_position` 的语义一致（见
/// `undock_target` 的说明），所以「另一轴保持原位」用的也必须是**客户区**
/// 当前位置：拿窗框坐标当客户区用，左/右侧贴边会把条子放高一个标题栏，
/// 而且每贴一次就再偏一点（展开时又按客户区锚回，净效果是累积漂移）。
fn strip_geometry(
    edge: DockEdge,
    work_x: i32,
    work_y: i32,
    work_w: i32,
    work_h: i32,
    client_w: i32,
    client_h: i32,
    client_x: i32,
    client_y: i32,
    strip: i32,
    gap: i32,
) -> (tauri::PhysicalSize<u32>, (i32, i32)) {
    match edge {
        // Bottom/Top：窗口保持主窗宽、高缩成 strip；x 保持主窗客户区原位
        DockEdge::Bottom => (
            tauri::PhysicalSize::new(client_w.max(strip) as u32, strip as u32),
            (client_x, work_y + work_h - strip - gap),
        ),
        DockEdge::Top => (
            tauri::PhysicalSize::new(client_w.max(strip) as u32, strip as u32),
            (client_x, work_y + gap),
        ),
        // Left/Right：窗口保持主窗高、宽缩成 strip；y 保持主窗客户区原位
        DockEdge::Left => (
            tauri::PhysicalSize::new(strip as u32, client_h.max(strip) as u32),
            (work_x + gap, client_y),
        ),
        DockEdge::Right => (
            tauri::PhysicalSize::new(strip as u32, client_h.max(strip) as u32),
            (work_x + work_w - strip - gap, client_y),
        ),
    }
}

/// 贴边热区判定（纯函数，物理像素）：热区 = 从**工作区贴边一侧**往工作区内
/// 延伸 `hotzone` 厚的带状区域，沿边轴与停靠窗口同范围。光标落在带内 = 触发呼出。
///
/// 判定不依赖窗口矩形：热区厚度是独立设置，可以比窗口槽位（40px）更厚 ——
/// 那段在窗口外，鼠标照样触发（后端几何判定，不需要真实窗口去接事件）。
fn hotzone_hit(
    edge: DockEdge,
    work_x: i32,
    work_y: i32,
    work_w: i32,
    work_h: i32,
    win_x: i32,
    win_y: i32,
    win_w: i32,
    win_h: i32,
    hotzone: i32,
    cx: i32,
    cy: i32,
) -> bool {
    let hz = hotzone.max(0);
    match edge {
        // 顶边：热区 [work_y, work_y+hz)，沿边轴取窗口 x 范围
        DockEdge::Top => {
            cx >= win_x && cx < win_x + win_w && cy >= work_y && cy < work_y + hz
        }
        // 底边：热区 (work_y+work_h-hz, work_y+work_h]
        DockEdge::Bottom => {
            cx >= win_x && cx < win_x + win_w && cy >= work_y + work_h - hz && cy < work_y + work_h
        }
        // 左缘：热区 [work_x, work_x+hz)，沿边轴取窗口 y 范围
        DockEdge::Left => {
            cy >= win_y && cy < win_y + win_h && cx >= work_x && cx < work_x + hz
        }
        // 右缘：热区 (work_x+work_w-hz, work_x+work_w]
        DockEdge::Right => {
            cy >= win_y && cy < win_y + win_h && cx >= work_x + work_w - hz && cx < work_x + work_w
        }
    }
}

/// 贴边隐藏：按当前窗口位置选最近的屏幕边，把窗口缩成一条白条——
/// 白条沿屏幕边方向拉伸（长度 = 屏幕边长 × 长度百分比）、垂直方向缩成
/// 厚度，居中贴边。白条就是窗口本身，任务栏/Dock 永远不被遮挡。
fn dock_main_window(app: &AppHandle) {
    // 唯一入口闸：开关关着、或落位方式是「屏幕中间」（互斥）都不贴边。
    if !auto_hide_active(app) {
        return;
    }
    let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };
    // 恢复尺寸存去装饰前的 client 内尺寸（set_size 设的是内尺寸，展开时
    // set_size(内尺寸) 才能精确还原外框；存外框每轮会加一圈边框 = 越变越长）
    let pre_client = main.inner_size().ok();
    let (Ok(pos), Ok(size)) = (main.outer_position(), main.outer_size()) else {
        return;
    };
    // 贴到哪块屏的边上：固定显示器优先（选了屏 1，白条就贴屏 1 的边，
    // 哪怕窗口此刻还停在屏 2 —— set_position 会把它搬过去）；没设才跟
    // 窗口当前所在的屏走。
    let Some(monitor) = main_window_monitor(app)
        .or_else(|| main.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())
    else {
        return;
    };
    let work = monitor.work_area();
    // 上一轮贴边状态：选边惯性 + WM 微调吸收都要用（见 redock_edge /
    // dock_axis_position 的注释）。dock_pos 在 undock 后仍保留，正是这两处的基准。
    let (last_edge, prev_dock_pos) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (state.auto_hide_last_edge, state.auto_hide_dock_pos)
    };
    // 窗口缩成细缝：厚度 = max(槽位 40, 热区厚度)。白条参数由前端 CSS 独立
    // 控制；热区可视化也画在这条缝里 —— 窗口必须装得下热区矩形的可见部分，
    // 否则「渲染的热区」和「判定的热区」就成了两个不同的矩形。
    let dock_settings = stored_settings(&app);
    let slot = DOCK_STRIP_SLOT_PX.max(dock_settings.main_hotzone_thickness as f64);
    let strip = (slot * monitor.scale_factor()).round() as i32;
    // 白条距边界的缝隙：设置存逻辑像素，落点用物理像素（与触发距离同款换算）
    let gap = (dock_settings.main_dock_strip_edge_gap as f64 * monitor.scale_factor()).round()
        as i32;
    // 主窗 client 边长（白条沿这条窗口边延伸，长度百分比由前端 CSS 控制）
    let (client_w, client_h) = match &pre_client {
        Some(c) => (c.width as i32, c.height as i32),
        None => (size.width as i32, size.height as i32),
    };
    // 「另一轴保持原位」要的是**客户区**原位（见 strip_geometry 的说明）。
    // 读的是客户区当前位置 —— 贴边逻辑里判位/落位的唯一可信来源。
    let (client_x, client_y) = window_client_position(&main).unwrap_or((pos.x, pos.y));
    // 选边：上一轮的边没有被严格更近的边挑战就沿用（纯几何重判会在
    // 「贴左缘同时也几乎贴底缘」的窗口上翻边）。输入用客户区矩形。
    let edge = redock_edge(
        last_edge,
        client_x,
        client_y,
        client_w,
        client_h,
        work.position.x,
        work.position.y,
        work.size.width as i32,
        work.size.height as i32,
    );
    // 沿边轴吸收 WM 重装饰微调（只在沿用上一轮边时吸收；换边说明用户主动拖过，
    // 旧 dock_pos 对新边毫无意义）。
    let (client_x, client_y) = if last_edge == Some(edge) {
        match edge {
            DockEdge::Left | DockEdge::Right => (
                client_x,
                dock_axis_position(client_y, prev_dock_pos.1, AUTO_HIDE_WM_NUDGE_PX),
            ),
            DockEdge::Top | DockEdge::Bottom => (
                dock_axis_position(client_x, prev_dock_pos.0, AUTO_HIDE_WM_NUDGE_PX),
                client_y,
            ),
        }
    } else {
        (client_x, client_y)
    };
    let (new_size, target) = strip_geometry(
        edge,
        work.position.x,
        work.position.y,
        work.size.width as i32,
        work.size.height as i32,
        client_w,
        client_h,
        client_x,
        client_y,
        strip,
        gap,
    );
    {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state.auto_hide_docked = true;
        state.auto_hide_edge = Some(edge);
        state.auto_hide_last_edge = Some(edge);
        state.auto_hide_dock_pos = target;
        state.auto_hide_dock_resnaps = 0;
        // 重新停靠 = 展开落点话题终结：未收敛的重锚计划作废，别再跟停靠
        // 几何抢窗口。
        state.auto_hide_reanchor = None;
        state.auto_hide_restore_size = Some(match pre_client {
            Some(client) => (client.width.max(1), client.height.max(1)),
            None => (size.width.max(1), size.height.max(1)),
        });
    }
    // 抑制期必须在发几何请求**之前**立起来（请求同步发出、配置事件异步回来），
    // 否则这批回声落在抑制期外 —— Linux 上那就是「刚缩成条就被弹回」。
    // 缩条/展开都会连续改几何，抑制期也顺带防住 place↔follow 乒乓。
    mark_geometry_busy(AUTO_HIDE_GEOMETRY_BUSY_MS);
    // 在**隐藏态**做完「改尺寸 + 落位」再 show。可见窗口上做这一套时，前面那句
    // `set_decorations(false)` 触发的重父化会吃掉同批发出的 set_position：
    // 实测（Muffin/X11）窗口缩成条后停在旧窗框原点（client y=63）约 420ms，
    // 直到 watcher 的「回钉」兜底才挪到边条上 —— 用户看到的就是「白条先在屏幕
    // 上方闪一下，再跳到边条」。隐藏态做完再 show 没有这个中间态，和
    // undock_main_window 是同一个套路（那边注释见「标题跳一下才出来」）。
    // 注意：monitor / target 都必须在这之前算完 —— tao 在窗口不可见时
    // `current_monitor()` 会退化成主显示器，多显示器下会把边条吸到错的屏上。
    let was_visible = main.is_visible().unwrap_or(false);
    // 缓存标题栏高度（窗框顶 − 客户区顶）：此刻窗口还带着装饰，三端读数
    // 都准。顶边贴边展开时客户区要给标题栏让位（否则标题栏悬在屏幕外
    // 看不见、窗口拖不动），undock 时的现场实测在 Linux 上可能读到 0
    // （GDK frame extents 缓存），那时退回这个缓存值。
    if let (Ok(inner), Ok(outer)) = (main.inner_position(), main.outer_position()) {
        let caption = inner.y - outer.y;
        if caption > 0 {
            let state = app.state::<Mutex<AppState>>();
            state.lock().expect("state mutex poisoned").auto_hide_caption = caption;
        }
    }
    if was_visible {
        // 「清屏」：趁窗口还可见，让 WebView 把**最后一帧**刷成全透明。
        // hide→show 后窗口服务器先摆出的就是这帧缓存（macOS 上 WebView
        // 新帧要下一拍才到），缓存里若是主界面切片，用户就看到「残影子
        // 在热区里闪一下再消失」。全透明帧看不出任何东西；因为原生背景
        // 也是透明的，这 ~80ms 里全尺寸窗口在屏幕上等于不存在。
        // busy 闸已在上面立起（docked 分支遇 busy 会 Hold），这段等待
        // 不会被 watcher 的停靠态轮询抢走窗口。
        let _ = main.eval("document.documentElement.classList.add('dock-pending');");
        std::thread::sleep(std::time::Duration::from_millis(80));
        let _ = main.hide();
    }
    // 去装饰 / 关投影 / 压原生背景：全部放到 hide **之后**。在可见窗口上做，
    // 标题栏先当场消失一帧、内容重排一帧，用户看到的就是「主界面闪一下才藏」；
    // 隐藏态做这些，屏幕上只有「整窗消失」一个动作。所有几何计算（monitor /
    // client 位置）都在上面取完了，这里纯属窗口属性设置。
    // 去装饰：带标题栏的窗口有最小高度钳制，缩不成条；无边框窗口无此钳制
    let _ = main.set_decorations(false);
    // 关掉 DWM 投影：窗口默认带 drop shadow，贴边缩成透明细缝后阴影仍围
    // 着细缝绘制，用户看到的就是「半透明矩形包着一条白条」。展开时还原。
    let _ = main.set_shadow(false);
    // 窗口/WebView 的**原生背景**也要按到透明：CSS 里贴边态的 body/html
    // 背景早已是 transparent，但三端的 WebView 原生层默认仍是白（macOS
    // NSWindow/Windows WebView2/GTK 的默认底色），白条外面那圈「白色底框」
    // 就是它。macOS 上这整套透明能力由 macos-private-api feature 门控。
    let _ = main.set_background_color(Some(tauri::utils::config::Color(0, 0, 0, 0)));
    // 贴边期间**临时解锁 resizable**：GTK3 对「不可缩放」窗口的尺寸处理是把
    // 尺寸钉在**子控件最小尺寸**上（WebKitWebView 实测 ≈ 200x200 逻辑px），
    // 之后 set_size 一律被夹进 [200, max] —— 贴边要缩的 40 逻辑px 细缝根本
    // 缩不动，窗口停在 200 逻辑px，白条（贴在窗口朝屏幕边那一侧）就画在离
    // 屏幕边 200px 处；多屏上直接落到隔壁屏（真机复现台 Xvfb+mutter
    // 3840x2160/2x：贴顶请求 1864x80 物理 → 实测 1864x400 物理，
    // WM_NORMAL_HINTS min=max=1864x400）。用户设置里 windowSizeLocked=true
    // 走的就是 set_resizable(false)（apply_window_size_lock），所以「锁定窗口
    // 大小」开着时必现。展开时由 apply_window_size_lock 重新上锁（探针验证：
    // 解锁→缩 40x720 ✓，展开→回锁 → 尺寸保持且锁仍生效）。
    let _ = main.set_resizable(true);
    let _ = main.set_size(new_size);
    let _ = main.set_position(tauri::PhysicalPosition::new(target.0, target.1));
    if was_visible {
        let _ = main.show();
    }
    startup_debug(
        app,
        &format!(
            "dock begin edge={edge:?} from=({},{}) size={}x{} target=({},{})",
            pos.x, pos.y, size.width, size.height, target.0, target.1
        ),
    );
    trace(
        app,
        "autohide",
        &format!("dock edge={edge:?} target=({},{})", target.0, target.1),
    );
    pin_main_topmost(app);
    // 停靠落定取证（常开，低频动作）：set_position/set_size 是异步落地的，
    // 发请求那一刻读数必陈旧 —— 等 200ms 再读回窗口实际矩形。多屏下
    // 「白条画在哪」要靠这条与前端渲染取证（strip rect css=...）对拍：
    // 窗口物理矩形 × 前端白条 CSS 矩形 × DPR 一乘，白条的物理落点一清
    // 二楚，肉眼描述猜不出根因。
    {
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
                return;
            };
            let docked = app
                .state::<Mutex<AppState>>()
                .lock()
                .map(|state| state.auto_hide_docked)
                .unwrap_or(false);
            if !docked {
                return; // 这 200ms 里用户呼出了/换屏了，读数没有意义
            }
            let (Ok(pos), Ok(size)) = (main.outer_position(), main.outer_size()) else {
                return;
            };
            let client_pos = main.inner_position().ok().map(|p| (p.x, p.y));
            let client_size = main.inner_size().ok().map(|s| (s.width, s.height));
            startup_debug(
                &app,
                &format!(
                    "dock done pos=({},{}) outer={}x{} client_pos={client_pos:?} client_size={client_size:?}",
                    pos.x, pos.y, size.width, size.height
                ),
            );
        });
    }
    // 广播 docked 状态：前端据此在窗口边界画白条（ui-state-changed 顺带同步）
    publish_ui(app);
}

/// 展开贴边的窗口：恢复贴边前的尺寸（贴边时被缩成一条白条厚度），
/// 贴回工作区边缘（不是回到贴边前的原位置 —— 简单且可预期）。
fn undock_main_window(app: &AppHandle) {
    let (edge, restore, dock_pos) = {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state.auto_hide_docked = false;
        (
            state.auto_hide_edge.take(),
            state.auto_hide_restore_size.take(),
            state.auto_hide_dock_pos,
        )
    };
    let Some(edge) = edge else {
        return;
    };
    let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };
    // 整个展开过程在隐藏状态下完成：先恢复装饰（标题栏）、量边距、摆几何，
    // 最后再 show。如果带着可见窗口做 set_decorations → set_size →
    // set_position，标题栏会先在「细缝」上长出来、再跳到最终位置 —— 用户
    // 看到的就是「标题跳一下才出来」。隐藏期间 watcher 侧最多有一拍读不到
    // 窗口，此时 docked 已经是 false，不会触发贴回。
    let was_visible = main.is_visible().unwrap_or(false);
    let t0 = unix_millis();
    if was_visible {
        let _ = main.hide();
    }
    trace(
        app,
        "autohide",
        &format!("undock#hide t={} vis={was_visible}", unix_millis() - t0),
    );
    // 先把系统装饰（标题栏）还原再恢复尺寸：贴边时 set_decorations(false)
    // 去掉了标题栏，不还原的话展开后窗口没有标题栏、拖不动。
    let _ = main.set_decorations(true);
    // 展开到哪块屏的工作区：固定显示器优先。白条停在 A 右缘时 tao 的
    // current_monitor 可能判成相邻的那块屏（多屏边界窗口的双归属歧义，
    // 真机实测：白条在 A [0,3840] 内 3752 处，current_monitor 却给了
    // B [3840,5760]）→ 按它算 Right 边展开落点 = 3840+1920−1864 = 3896，
    // **越过 A 的右边界**，整窗落到 B 上 —— 用户看到的就是「白条一呼出，
    // 窗口跑到另一块屏」。固定名优先后 target 永远落在固定屏工作区内。
    let Some(monitor) = main_window_monitor(app)
        .or_else(|| main.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())
    else {
        if was_visible {
            let _ = main.show();
        }
        return;
    };
    let work = monitor.work_area();
    // 恢复贴边前的尺寸（没记到就用当前尺寸兜底）。存的/用的都是 **client 内尺寸**
    // ——`set_size` 设的就是内尺寸，存外框每轮会加一圈边框 = 越变越长。
    let (win_w, win_h) = restore.unwrap_or_else(|| {
        main.inner_size()
            .map(|s| (s.width, s.height))
            .unwrap_or((460, 900))
    });
    // 展开的**沿边轴锚点 = dock_pos**（上一轮停靠时 set_position 写下的白条
    // 客户区坐标；停靠态无装饰，落点即客户区，三端语义一致）。
    //
    // 为什么不现场读：「刚 set_decorations(true) 的隐藏窗口」上读窗框坐标
    // （outer_position）再拿缓存的标题栏偏移换算，三端各有各的缓存错拍 ——
    // macOS/Windows 直接多出一个标题栏，Linux 取决于 GDK frame extents 刷新
    // 到没刷新。偏差本身不可怕，可怕的是它进了闭环：undock 按偏了的 target
    // 落位（重锚牛顿步只保证「实况 == target」，target 偏了照单全收）→ 下一轮
    // dock 把这个偏位记进 dock_pos → 再 undock 再偏一个标题栏 —— 左右贴边
    // 开合几轮，白条从屏幕顶一路爬到屏幕底（三端实测，真机日志 target
    // 428→492→556→588）。锚定 dock_pos 后闭环进料口被掐死：undock 落点 ==
    // 白条原位，下一轮 dock 读到的实况 == dock_pos，零累积。
    // 落位补偿的边距（窗框→客户区）：三端 `set_position` 在**带装饰**窗口上
    // 都是「窗框原点」语义 —— Windows（SetWindowPos）/macOS（setFrameOrigin）
    // 天然如此，Linux(GTK) 的 tao 带装饰时同样落窗框（客户区 = 请求 + 标题栏，
    // 真机 2x = 64px）。target 是**客户区**期望落点，请求必须预先扣掉这段
    // 边距，否则 Windows 上客户区先偏 (border≈8, 标题栏≈31) 落下去，300ms
    // 后重锚牛顿步再把它扳回来 —— 用户看到的就是「展开后窗口跳/抖一下」。
    // 边距在 set_decorations(true) 之后现场实测（inner−outer）：Windows 的
    // 样式切换同步生效、隐藏窗口读数即准 → **首发即落准**；Linux 的 GDK
    // frame extents 是缓存、此刻多半还没刷新（读到 0）→ 退化成原行为，
    // 由重锚牛顿步兜底学习。学习值（auto_hide_undock_offset）叠加在补偿
    // 之上：实况 = 请求 + 边距 = target + 学习值，牛顿步语义不变。
    let frame_offset = match (main.inner_position(), main.outer_position()) {
        (Ok(inner), Ok(outer)) => (inner.x - outer.x, inner.y - outer.y),
        _ => (0, 0),
    };
    // 顶边让位的标题栏高度：优先现场实测（Windows/macOS 准），实测读不到
    // （Linux 缓存 0）退回 dock 时缓存的值；都没有就 0（= 旧行为，标题栏
    // 出屏，但至少不会更糟）。
    let cached_caption = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        state.auto_hide_caption
    };
    let frame_top = if frame_offset.1 > 0 { frame_offset.1 } else { cached_caption };
    let target = undock_target(
        edge,
        work.position.x,
        work.position.y,
        work.size.width as i32,
        work.size.height as i32,
        dock_pos.0,
        dock_pos.1,
        win_w as i32,
        win_h as i32,
        frame_top,
    );
    let _ = main.set_size(tauri::PhysicalSize::new(win_w, win_h));
    let (offset, request) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        let offset = state.auto_hide_undock_offset;
        (
            offset,
            (
                target.0 - frame_offset.0 + offset.0,
                target.1 - frame_offset.1 + offset.1,
            ),
        )
    };
    let _ = main.set_position(tauri::PhysicalPosition::new(request.0, request.1));
    // 武装重锚：这一拍 set_position 的落位语义没把握（窗框/客户区、缓存
    // 错拍、WM 钳制都可能出现）——watcher 随后几拍读实况、偏了就按牛顿步
    // 修修正量再补发（见 undock_reanchor_action）。
    {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state.auto_hide_reanchor = Some(AutoHideReanchor {
            target,
            offset,
            tries: 0,
            last_live: None,
            noops: 0,
        });
    }
    trace(
        app,
        "autohide",
        &format!(
            "undock#geo t={} target=({},{}) request=({},{})",
            unix_millis() - t0,
            target.0,
            target.1,
            request.0,
            request.1
        ),
    );
    // 还原贴边时关掉的 DWM 投影（dock_main_window 里 set_shadow(false)）
    let _ = main.set_shadow(true);
    // 展开（undock）首帧掩底：见 undock_mask_color 的注释。必须在 show
    // **之前**设 —— show 后的第一帧就靠它兜着，NSWindow 背景是主线程
    // 原生属性、立即生效，不等 WebView 进程醒来。
    let _ = main.set_background_color(Some(undock_mask_color(app, &main)));
    // 几何全部就位后才亮出来：标题栏直接在最终位置出现，不跳
    if was_visible {
        // 撤掉 dock 前的清屏态（show 后第一帧旧缓存本来就是近乎全透明的
        // 白条缝，拉伸不出内容；撤类的新帧随后自然到位）
        let _ = main.eval("document.documentElement.classList.remove('dock-pending');");
        let _ = main.show();
        // show 之后必须补钉置顶：hide → set_decorations(true) → show 这一圈
        // 在 Linux（EWMH keep-above 随重映射丢失）和 macOS（NSWindow.level 被
        // orderOut/orderIn 重置）上会把置顶层级丢掉 —— 用户看到的就是「悬浮
        // 呼出后窗口被别的窗口盖住」，Windows 的 HWND_TOPMOST 能扛住所以以前
        // 只有那边正常。设置关掉时 pin_main_topmost 自己会跳过。
        pin_main_topmost(app);
    }
    // 贴边期间临时解锁过 resizable（见 dock_main_window 的注释）：展开落定后
    // 按设置重新上锁，保住「锁定窗口大小 = 不能手拖缩放」的语义。放在 show
    // 之后 —— 此刻尺寸请求已经发出，重新上锁是拿「已请求的尺寸」去夹，不会
    // 把窗口夹回贴边时的 40px。未开启锁定时这个函数是「解锁 + 清上限」，无害。
    apply_window_size_lock(app);
    trace(
        app,
        "autohide",
        &format!("undock#show t={}", unix_millis() - t0),
    );
    // 重设一次抑制期（show 之后还会回来一批配置事件：重设边框、重设尺寸都要
    // WM 重新摆一次窗口），期间 watcher 不拉回、handle_main_moved 不夹取。
    mark_geometry_busy(AUTO_HIDE_GEOMETRY_BUSY_MS);
    startup_debug(
        app,
        &format!(
            "undock edge={edge:?} restore={win_w}x{win_h} target=({},{}) request=({},{})",
            target.0, target.1, request.0, request.1
        ),
    );
    trace(
        app,
        "autohide",
        &format!("undock edge={edge:?} restore=({win_w},{win_h})"),
    );
    publish_ui(app);
}

/// 展开首帧的「掩底色」：隐藏期间 WebView 停止合成，show 出来那一拍只有
/// 原生标题栏（系统即时绘制）+ 上一帧缓存（近全透明的白条缝）——白条缝
/// 拉伸后整个窗口就是个透明洞，用户看到「标题栏先出、卡一下内容才出」。
/// NSWindow 背景是主线程原生属性、立即生效、不等 WebView 进程：按主题
/// 底色（alpha 跟随窗口不透明度设置，与 body 的 rgba(--bg-rgb, --ui-alpha)
/// 同源）填上，观感 = 正常应用开窗。不透明度 100%（默认）时 body 本来就
/// 不透明，这层背景完全被盖住、零影响；半透明用户会略有叠透（可接受）。
/// 下一轮 dock 会把它按回全透明（dock_main_window 的 set_background_color）。
fn undock_mask_color(
    app: &AppHandle,
    main: &tauri::WebviewWindow,
) -> tauri::utils::config::Color {
    let settings = stored_settings(app);
    let alpha = (settings.opacity_percent.min(100) as u32 * 255 / 100) as u8;
    let (r, g, b) = match settings.theme.as_str() {
        "light" => (244, 246, 250),
        "dark" => (20, 25, 34),
        // system：按系统当前主题解析（解析不了按暗色兜底，与前端默认一致）
        _ => match main.theme() {
            Ok(tauri::Theme::Light) => (244, 246, 250),
            _ => (20, 25, 34),
        },
    };
    tauri::utils::config::Color(r, g, b, alpha)
}

/// 解除贴边（含用户抓着白条直接拖离）时恢复贴边前的窗口尺寸。
/// undock_main_window 自己取恢复尺寸做定位，不走这里。
fn restore_main_dock_size(app: &AppHandle) {
    let restore = {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state.auto_hide_restore_size.take()
    };
    let Some((w, h)) = restore else {
        return;
    };
    if let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        // 拖离停靠位 = 展开窗口：系统装饰（标题栏）也要一并还原，否则
        // 窗口恢复完整尺寸了却没有标题栏，拖不动、关不掉。
        // 同 dock/undock：重设边框 + 尺寸会回来一批中间态事件，先立抑制期。
        mark_geometry_busy(AUTO_HIDE_GEOMETRY_BUSY_MS);
        if auto_hide_diag_enabled() {
            startup_debug(
                app,
                &format!(
                    "restore dock size -> {w}x{h} (inner 当前={:?})",
                    main.inner_size()
                ),
            );
        }
        let _ = main.set_decorations(true);
        let _ = main.set_size(tauri::PhysicalSize::new(w, h));
        // 同 undock：贴边期间临时解锁过 resizable（否则 GTK 把尺寸钉在子控件
        // 最小尺寸 200 逻辑px 上），恢复完整尺寸后重新上锁。
        apply_window_size_lock(app);
    }
}

/// 主窗口**始终**置顶（受「窗口始终置顶」设置控制）：Windows 上 `set_position`
/// （夹取 / 贴边重定位）偶发会把 topmost 样式丢掉，于是窗口被别的程序盖住。
/// 每次挪完位置再钉一次 topmost，保证「无论如何都在最上层」。关掉设置时
/// 什么都不做（取消置顶由 setter 负责，这里不反向抢）。
fn pin_main_topmost(app: &AppHandle) {
    if !stored_settings(app).main_always_on_top {
        return;
    }
    if let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = main.set_always_on_top(true);
    }
}

/// 当前 Unix 毫秒时间戳（贴边宽限期用）。
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 【临时诊断】托盘启动时独立窗口仍闪现的问题排查：无条件追加写
/// startup-debug.log（配置目录下），定位完成后整段删除。
fn startup_debug(app: &AppHandle, message: &str) {
    let Ok(mut path) = config_dir(app) else {
        return;
    };
    path.push("startup-debug.log");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let line = format!("{} [{}] {}\n", unix_millis(), std::process::id(), message);
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()));
}

/// 贴边 watcher 的逐拍取证开关：环境变量 `ANYBOOT_AUTOHIDE_DIAG=1` 打开时，
/// watcher 每秒往 startup-debug.log 打一行它实际看到的几何与判定，用来定位
/// 「为什么没贴边」。默认关 —— 那是每秒一行，常开会把日志写爆。
fn auto_hide_diag_enabled() -> bool {
    std::env::var("ANYBOOT_AUTOHIDE_DIAG").is_ok_and(|value| !value.is_empty() && value != "0")
}

/// 鼠标位置轮询守护线程：这个 Tauri 版本的 `WindowEvent` 没有 CursorEntered /
/// CursorLeft 事件，所以「鼠标在不在窗口里」用 `AppHandle::cursor_position()`
/// 每 120ms 扫一次 —— 没贴边且光标在外超过 350ms 就贴边；贴着边且光标压到
/// 露出的那条上就展开。开关关掉时线程自己退出。
fn start_auto_hide_watcher(app: &AppHandle) {
    static WATCHER_RUNNING: AtomicBool = AtomicBool::new(false);
    if WATCHER_RUNNING.swap(true, Ordering::SeqCst) {
        return; // 已经有一个在跑
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let dock_delay_ticks = (AUTO_HIDE_DELAY_MS / AUTO_HIDE_TICK_MS).max(1) as u32;
        let mut outside_ticks: u32 = 0;
        // 光标已连续压在白条上的拍数（悬浮呼出延时用，见 docked_poll_action）
        let mut hover_ticks: u32 = 0;
        // 上一拍光标是否在窗口内：只为「一次离窗打一条判定日志」用。
        let mut last_inside = false;
        // 逐拍取证开关（ANYBOOT_AUTOHIDE_DIAG=1）与下一次心跳的时间点。
        let diag = auto_hide_diag_enabled();
        let mut diag_next_at: u64 = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(AUTO_HIDE_TICK_MS));
            if !main_auto_hide_enabled(&app) {
                WATCHER_RUNNING.store(false, Ordering::SeqCst);
                return;
            }
            let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
                continue;
            };
            if !main.is_visible().unwrap_or(false) {
                outside_ticks = 0;
                continue;
            }
            // 是否处于「贴边缩成条」状态：此时主窗口只露一条边，GRUB 独立
            // 窗口应跟随隐藏，深度绑定只在「完整显示」状态下维持。
            let (docked, dock_pos, pending_enter, grace_until, docked_edge) = {
                let state = app.state::<Mutex<AppState>>();
                // 不用 expect：锁中毒会 panic 掉整个 watcher 线程，而
                // WATCHER_RUNNING 还卡在 true，自动隐藏从此永久失效。
                let Ok(state) = state.lock() else {
                    WATCHER_RUNNING.store(false, Ordering::SeqCst);
                    return;
                };
                (
                    state.auto_hide_docked,
                    state.auto_hide_dock_pos,
                    state.auto_hide_pending_enter,
                    state.auto_hide_grace_until_ms,
                    state.auto_hide_edge,
                )
            };
            let (Ok(pos), Ok(size)) = (main.outer_position(), main.outer_size()) else {
                continue;
            };
            let Ok(cursor) = app.cursor_position() else {
                continue;
            };
            let cx = cursor.x as i32;
            let cy = cursor.y as i32;
            // 「在窗口里」= 主窗口或独立窗口的矩形（各自外扩一圈）。
            // 外扩是为了盖住两窗之间的缝隙：鼠标从主窗口滑到独立窗口的
            // 半路上不算「离开」，不能累计隐藏时钟。
            let monitor = main
                .current_monitor()
                .ok()
                .flatten()
                .or_else(|| app.primary_monitor().ok().flatten());
            // 悬停判定的外扩余量（逻辑像素）：给「窗口边缘 ±12px」的容差，
            // 鼠标在窗口边缘打转时不至于反复进出。
            let pad = monitor
                .as_ref()
                .map(|m| (12.0_f64 * m.scale_factor()).round() as i32)
                .unwrap_or(12);
            let in_box = |x: i32, y: i32, w: i32, h: i32| {
                cx >= x - pad && cx < x + w + pad && cy >= y - pad && cy < y + h + pad
            };
            // 不带外扩的严格判定：贴边停靠态下判断「光标压在白条上」用它。
            // 外扩是给「主窗↔GRUB 独立窗」之间那道缝留的（见上），贴边态下
            // 独立窗是藏着的、白条窗口本身也完全落在工作区内 —— 再用外扩
            // 就会把工作区外边那一条（任务栏/面板所在处）也算成「在窗口里」，
            // 光标停在面板上就展开 → 贴边成条↔展开互闪。
            let in_rect =
                |x: i32, y: i32, w: i32, h: i32| cx >= x && cx < x + w && cy >= y && cy < y + h;
            let inside = in_box(pos.x, pos.y, size.width as i32, size.height as i32);
            // 本拍与上一拍的「在窗口里」对比：只在 inside→outside 这一次跳变上
            // 打日志（见下面 else 分支），避免 8 拍/秒刷屏。
            let inside_prev = std::mem::replace(&mut last_inside, inside);
            // 距离门槛（用户自定义）：主窗口**或 GRUB 独立窗口**贴近工作区
            // 某条边（不超过触发距离）时，鼠标离开就整组隐藏 —— 两窗绑定，
            // 用户拖的是 GRUB 也要一样触发（GRUB 窗口同款特性）。
            // 提到分支之前算：贴边判定和下面的逐拍取证日志共用同一份结果。
            let settings_now = stored_settings(&app);
            let edge_distance = settings_now.main_auto_hide_edge_distance;
            // 悬浮呼出延时（毫秒→拍数）：0 = 当拍立即呼出（既有行为）
            let hover_need = hover_delay_ticks(settings_now.main_dock_strip_hover_ms);
            let threshold = monitor
                .as_ref()
                .map(|m| (edge_distance as f64 * m.scale_factor()).round() as i32)
                .unwrap_or(0);
            let rect_near = |m: &tauri::Monitor, x: i32, y: i32, w: i32, h: i32, threshold: i32| {
                let work = m.work_area();
                auto_hide_near_edge(
                    x,
                    y,
                    w,
                    h,
                    work.position.x,
                    work.position.y,
                    work.size.width as i32,
                    work.size.height as i32,
                    threshold,
                )
            };
            let near_edge = monitor.as_ref().is_some_and(|m| {
                rect_near(
                    m,
                    pos.x,
                    pos.y,
                    size.width as i32,
                    size.height as i32,
                    threshold,
                )
            });
            // 逐拍取证（ANYBOOT_AUTOHIDE_DIAG=1）：每秒一行，把这一拍 watcher
            // 实际看到的几何与判定全打出来。真机上「为什么没贴边」看它。
            if diag && unix_millis() >= diag_next_at {
                diag_next_at = unix_millis() + 1000;
                startup_debug(
                    &app,
                    &format!(
                        "autohide diag: docked={} pos=({},{}) size={}x{} inner=({},{}) cursor=({},{}) inside={} near_edge={} outside_ticks={} grace_left_ms={} pending_enter={}",
                        docked,
                        pos.x,
                        pos.y,
                        size.width,
                        size.height,
                        main.inner_position().map(|p| p.x).unwrap_or(-1),
                        main.inner_position().map(|p| p.y).unwrap_or(-1),
                        cx,
                        cy,
                        inside,
                        near_edge,
                        outside_ticks,
                        grace_until.saturating_sub(unix_millis()),
                        pending_enter
                    ),
                );
            }
            // undock 重锚：展开落点的「现实校准」。装饰态窗口上 set_position
            // 的坐标是**窗框**原点（GTK gravity 语义），客户区实际落在
            // 请求 + 标题栏（真机 Cinnamon 2x = 64px）——不纠正的话下一轮
            // dock 把错位坐标当实况记进 dock_pos（超出 ±8px 微调容差），每
            // 开合一轮 +64 爬移、最后被工作区底缘钳住翻成底边贴边（真机日志
            // 铁证：target 428→492→556→588 后恒定）。修法是牛顿式自校准：
            // 实测偏差全额反扣进修正量、按修正后的请求补发；收敛时把修正量
            // 写回 AppState，下次 undock **首发**即准。用户正在拖窗
            // （moved_recently）不抢、3 拍不收敛放弃。复现台（标题栏 0px）
            // 与 Windows（客户区语义、落点精确）偏差在容差内 → 整块零动作。
            let reanchor_plan = {
                let state = app.state::<Mutex<AppState>>();
                state.lock().ok().and_then(|guard| guard.auto_hide_reanchor)
            };
            if let Some(plan) = reanchor_plan {
                if !main_moved_recently() {
                    if let Some(live) = main_client_position(&app) {
                        match undock_reanchor_action(plan, live.0, live.1) {
                            UndockReanchor::Set(new_plan, request) => {
                                // 取证常开：牛顿步的补发只进 trace（要开
                                // debugTrace 才有），而 extents 刷新/语义翻转
                                // 这类时序问题恰恰要看「每拍发了什么、落在哪」。
                                startup_debug(
                                    &app,
                                    &format!(
                                        "undock#reanchor try={} live=({},{}) -> ({},{}) offset=({},{})",
                                        new_plan.tries, live.0, live.1, request.0, request.1,
                                        new_plan.offset.0, new_plan.offset.1
                                    ),
                                );
                                trace(
                                    &app,
                                    "autohide",
                                    &format!(
                                        "undock#reanchor try={} live=({},{}) -> ({},{}) offset=({},{})",
                                        new_plan.tries, live.0, live.1, request.0, request.1,
                                        new_plan.offset.0, new_plan.offset.1
                                    ),
                                );
                                {
                                    if let Ok(mut guard) =
                                        app.state::<Mutex<AppState>>().lock()
                                    {
                                        guard.auto_hide_reanchor = Some(new_plan);
                                    }
                                }
                                mark_geometry_busy(AUTO_HIDE_GEOMETRY_BUSY_MS);
                                let _ = main.set_position(tauri::PhysicalPosition::new(
                                    request.0, request.1,
                                ));
                            }
                            UndockReanchor::Settled => {
                                startup_debug(&app, "undock#reanchor settled");
                                trace(&app, "autohide", "undock#reanchor settled");
                                if let Ok(mut guard) = app.state::<Mutex<AppState>>().lock() {
                                    // 学到的修正量写回：下一次 undock 首发即准
                                    //（会话内 WM 语义不变，这是常量）。
                                    guard.auto_hide_undock_offset = plan.offset;
                                    guard.auto_hide_reanchor = None;
                                }
                            }
                            UndockReanchor::GiveUp => {
                                startup_debug(
                                    &app,
                                    &format!(
                                        "undock reanchor gave up: live=({},{}) target=({},{}) offset=({},{})",
                                        live.0, live.1, plan.target.0, plan.target.1,
                                        plan.offset.0, plan.offset.1
                                    ),
                                );
                                if let Ok(mut guard) = app.state::<Mutex<AppState>>().lock() {
                                    guard.auto_hide_reanchor = None;
                                }
                            }
                        }
                    }
                }
            }
            if docked {
                outside_ticks = 0;
                // 托盘呼出进行中（等待进入标志立着）：undock 正在托盘线程
                // 恢复窗口尺寸/位置，此刻白条保障读到「偏离停靠位」是恢复
                // 过程的正常中间态 —— 拉回就是跟展开互相拉扯（互闪）。放手。
                if pending_enter {
                    hover_ticks = 0;
                    continue;
                }
                // 白条保障 + 悬停展开，两者都要看「几何是不是已经落位」：
                // 抑制期内一律放手 —— dock/undock 刚发出去的请求还没落地，
                // 这一拍读到的坐标是中间态，拉回或展开都是在跟自己的几何
                // 编程打架（Linux 上就是贴边成条→立刻被拉回/展开的互闪）。
                // 漂移判定用**客户区**绝对坐标：`dock_pos` 是 set_position 的落点
                // （客户区）。而 `outer_position()` 与 `Moved` 载荷都是 tao 的
                // `root_origin()` = 窗框坐标；停靠态已经去过装饰，GDK 那份 frame
                // extents 缓存不再刷新（WM 的 _NET_FRAME_EXTENTS 也停在旧值），
                // 恒多出一个标题栏（Muffin 实测 outer=923 / inner=960 / 目标=960）
                // → 每一拍都被误判成「白条被挪走了」，3 次回钉额度全被空转吃光，
                // 真出了位移反而没人修。停靠态无装饰，inner 就是屏幕坐标。
                let docked_pos = main_client_position(&app).unwrap_or((pos.x, pos.y));
                // 复用 handle_main_moved 的同一条判据（同一个纯函数、同一个容差，
                // 现在连坐标来源也统一成客户区）。
                let drifted = docked_move_action(docked_pos, dock_pos, AUTO_HIDE_TOLERANCE)
                    == DockedMove::Release;
                let resnaps = {
                    let state = app.state::<Mutex<AppState>>();
                    let locked = state.lock();
                    match locked {
                        Ok(guard) => guard.auto_hide_dock_resnaps,
                        Err(_) => AUTO_HIDE_RESNAP_LIMIT,
                    }
                };
                // 悬停判定用**热区矩形**（纯几何，不必是窗口那么厚）：
                // 从工作区贴边边往内 main_hotzone_thickness 厚、沿边轴与窗口
                // 同范围。厚度可设得比槽位缝更厚 —— 那段在窗口外，鼠标照样
                // 触发（后端几何判定，不需要真实窗口接事件）。edge/monitor
                // 状态缺失（不该发生）时退回窗口矩形 —— 几何上唯一已知的事实。
                let hovering = match (docked_edge, monitor.as_ref()) {
                    (Some(edge), Some(m)) => {
                        let work = m.work_area();
                        let hotzone_phys = (settings_now.main_hotzone_thickness as f64
                            * m.scale_factor())
                        .round() as i32;
                        hotzone_hit(
                            edge,
                            work.position.x,
                            work.position.y,
                            work.size.width as i32,
                            work.size.height as i32,
                            pos.x,
                            pos.y,
                            size.width as i32,
                            size.height as i32,
                            hotzone_phys,
                            cx,
                            cy,
                        )
                    }
                    _ => in_rect(pos.x, pos.y, size.width as i32, size.height as i32),
                };
                // 悬浮计时：连续压住才累计，光标一离开就清零（见 hover_delay_ticks）
                if hovering {
                    hover_ticks = hover_ticks.saturating_add(1);
                } else {
                    hover_ticks = 0;
                }
                match docked_poll_action(
                    geometry_busy(),
                    drifted,
                    hovering,
                    resnaps,
                    hover_ticks,
                    hover_need,
                ) {
                    DockedPoll::Hold | DockedPoll::Rest => {}
                    DockedPoll::SnapBack => {
                        // 位置被别的东西挪走了（夹取回声、DPI 取整抖动、Linux 上
                        // 去装饰瞬间 frame extents 错拍）→ 重新钉回停靠位。
                        // 钉完重新进抑制期：否则这次 set_position 的回声又会被
                        // handle_main_moved 当成「用户拖离白条」。
                        startup_debug(
                            &app,
                            &format!(
                                "docked: snap back ({},{}) -> ({},{})",
                                docked_pos.0, docked_pos.1, dock_pos.0, dock_pos.1
                            ),
                        );
                        trace(&app, "autohide", "docked: snap back");
                        {
                            let state = app.state::<Mutex<AppState>>();
                            let locked = state.lock();
                            if let Ok(mut guard) = locked {
                                guard.auto_hide_dock_resnaps =
                                    guard.auto_hide_dock_resnaps.saturating_add(1);
                            }
                        }
                        mark_geometry_busy(AUTO_HIDE_GEOMETRY_BUSY_MS);
                        let _ =
                            main.set_position(tauri::PhysicalPosition::new(dock_pos.0, dock_pos.1));
                    }
                    DockedPoll::Release => {
                        // 光标压到露出的边条上（也就是窗口可见的那一小块）→ 展开
                        startup_debug(&app, "docked: cursor on strip -> undock");
                        trace(&app, "autohide", "docked: cursor on strip");
                        undock_main_window(&app);
                    }
                }
            } else if inside {
                hover_ticks = 0;
                outside_ticks = 0;
                // 鼠标进过窗口了：清除「等待进入」，恢复正常的离窗贴边
                if !docked {
                    if let Ok(mut state) = app.state::<Mutex<AppState>>().lock() {
                        state.auto_hide_pending_enter = false;
                    }
                }
            } else {
                hover_ticks = 0;
                // 宽限期 / 「等待进入」标志都取自本拍开头那一次加锁（见上）：
                // 同一拍内再锁一次没有意义，只是多一处锁中毒分支要维护。
                // 距离门槛 / near_edge 也在上面统一算过（贴边判定与取证日志共用）。
                // 「一次离窗」只记一条：把这一拍的判定依据全打出来。真机上
                // 「为什么没贴边」看这一行就够（宽限期/等待进入/根本不够贴边）。
                if inside_prev {
                    startup_debug(
                        &app,
                        &format!(
                            "undocked: left window pos=({},{}) size={}x{} near_edge={} grace_left_ms={} pending_enter={} geo_busy={} move_quiet={}",
                            pos.x,
                            pos.y,
                            size.width,
                            size.height,
                            near_edge,
                            grace_until.saturating_sub(unix_millis()),
                            pending_enter,
                            geometry_busy(),
                            main_moved_recently()
                        ),
                    );
                }
                // 托盘呼出宽限期内不累计离窗时钟（呼出瞬间鼠标必然在托盘上）
                if unix_millis() < grace_until {
                    outside_ticks = 0;
                    continue;
                }
                // 三个否决条件一次问清（纯函数，语义见其定义与单测）
                if !undocked_counts_ticks(
                    near_edge,
                    unix_millis() < grace_until,
                    pending_enter,
                    geometry_busy(),
                    main_moved_recently(),
                ) {
                    outside_ticks = 0;
                    continue;
                }
                outside_ticks += 1;
                if outside_ticks >= dock_delay_ticks {
                    outside_ticks = 0;
                    dock_main_window(&app);
                }
            }
        }
    });
}

/// 非停靠态这一拍该不该开始/继续累计「离窗时钟」（也就是准不准它贴边）。
/// 五个否决条件：
/// 1. 窗口不够贴边（离工作区各边都超过触发距离）—— 用户明确要求「正中央不触发」；
/// 2. 正处于托盘呼出宽限期（呼出瞬间鼠标必然停在托盘上，那是窗口外）；
/// 3. 呼出之后鼠标还没进过窗口（用户还没真用上界面）；
/// 4. 我们自己刚编程过窗口几何（dock / undock / 展开 / 夹取），请求还没落位 ——
///    此刻读到的「光标在窗外」是中间态。与停靠态 `docked_poll_action` 的第一条
///    （`busy → Hold`）同义：那半边早就挡着了，这半边原先漏了 ——
///    于是「展开 → 176ms 后又被缩回条子」就在这条缝里发生。
/// 5. 主窗刚刚还在动（用户正在拖 / WM 正在摆）—— 拖动途中的「在窗外」同理。
/// 2、3 合起来就是「呼出保护」：少了它就是「呼出 → 立刻贴回 → 再呼出」的
/// 互闪死循环（每轮都带一次 show/hide + set_decorations 样式切换）。
/// 4、5 是「拖动/落位保护」：少了它，抓着白条拖离停靠位 → 窗口展开 →
/// 下一拍就按拖动途中的「光标在窗外」缩回条子 = 主窗闪一下 + GRUB 独立窗
/// 129ms 内翻两次（用户报的「主界面会变长 + grub 窗口乱闪乱跳」）。
/// 纯函数便于单测 —— 这几条防线的语义值得被测试钉住。
fn undocked_counts_ticks(
    near_edge: bool,
    grace_active: bool,
    pending_enter: bool,
    geometry_in_flight: bool,
    move_quiet: bool,
) -> bool {
    near_edge && !grace_active && !pending_enter && !geometry_in_flight && !move_quiet
}

/// 窗口是否贴近工作区的任意一条边（间隙 ≤ threshold）。纯函数便于单测。
/// 各边间隙：left = x - work_x，top = y - work_y，
/// right = work_x + work_w - (x + w)，bottom = work_y + work_h - (y + h)。
/// 间隙为负（窗口已被夹进边缘或略微越界）也算「贴边」。
fn auto_hide_near_edge(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    work_x: i32,
    work_y: i32,
    work_w: i32,
    work_h: i32,
    threshold: i32,
) -> bool {
    let gaps = [
        x - work_x,
        y - work_y,
        work_x + work_w - (x + w),
        work_y + work_h - (y + h),
    ];
    gaps.iter().any(|gap| *gap <= threshold)
}

/// 贴边停靠态下收到主窗 `Moved` 事件的处理结论。
#[derive(Debug, PartialEq, Eq)]
enum DockedMove {
    /// 坐标与停靠位差得远 = 用户抓着白条把它拖离了 → 解除停靠
    Release,
    /// 坐标就在停靠位（容差内）= 贴边后的正常状态，什么都不做
    Rest,
}

/// 停靠态收到 Moved 时该做什么（调用方负责先挡掉几何抑制期内的回声）。
/// 纯函数，便于单测。
fn docked_move_action(pos: (i32, i32), dock_pos: (i32, i32), tolerance: i32) -> DockedMove {
    if (pos.0 - dock_pos.0).abs() <= tolerance && (pos.1 - dock_pos.1).abs() <= tolerance {
        return DockedMove::Rest;
    }
    DockedMove::Release
}

/// 「屏幕中间」模式下收到主窗 `Moved` 的处置结论。
#[derive(Debug, PartialEq, Eq)]
enum CenterMove {
    /// 载荷仍在最近的居中落点附近（我们自己的回声 / WM 抖动 / 标题栏错拍）
    Rest,
    /// 偏离居中位 = 用户把窗口拖走了（或 WM 把它摆走了）→ 收回居中位
    Snap,
}

/// 居中模式收到 Moved 时该做什么。纯函数，便于单测。
/// `payload` = Moved 载荷（窗框坐标），`last` = 最近一次居中落点的请求值。
fn center_move_action(payload: (i32, i32), last: (i32, i32), tolerance: i32) -> CenterMove {
    if (payload.0 - last.0).abs() <= tolerance && (payload.1 - last.1).abs() <= tolerance {
        return CenterMove::Rest;
    }
    CenterMove::Snap
}

/// 贴边停靠态下 watcher 每一拍的处理结论。
#[derive(Debug, PartialEq, Eq)]
enum DockedPoll {
    /// 几何抑制期内：位置还在往停靠位落，放手（不拉回、也不展开）
    Hold,
    /// 位置偏离停靠位（被别的东西挪走）→ 重新钉回白条位置
    SnapBack,
    /// 光标压到露出的白条上 → 展开
    Release,
    /// 一切正常
    Rest,
}

/// 停靠态每拍该做什么。纯函数，便于单测。
/// `hover_ticks` = 光标已连续压在白条上的拍数，`hover_need` = 设置要求的拍数
/// （`hover_delay_ticks` 换算）；差着拍数就继续 Rest 等着，不展开。
fn docked_poll_action(
    busy: bool,
    drifted: bool,
    hovering: bool,
    resnaps: u8,
    hover_ticks: u32,
    hover_need: u32,
) -> DockedPoll {
    if busy {
        return DockedPoll::Hold;
    }
    if drifted && resnaps < AUTO_HIDE_RESNAP_LIMIT {
        return DockedPoll::SnapBack;
    }
    if hovering && hover_ticks >= hover_need {
        return DockedPoll::Release;
    }
    DockedPoll::Rest
}

/// 悬浮呼出延时（毫秒）换算成 120ms 轮询的拍数（向上取整）。
/// 0 = 当拍立即呼出（既有行为）。纯函数，便于单测。
fn hover_delay_ticks(ms: u64) -> u32 {
    ((ms + AUTO_HIDE_TICK_MS - 1) / AUTO_HIDE_TICK_MS) as u32
}

/// 窗口的**客户区**绝对位置 —— 贴边逻辑里唯一允许用来判位/落位的坐标。
///
/// 为什么不能直接用 `Moved` 载荷或 `outer_position()`：这几个 API 在 Linux 上
/// 是**两套坐标系** ——
/// * `set_position` / `set_size`（tao→`gtk_window_move`/`resize`）落的是**客户区**；
/// * `outer_position()` 和 `Moved` 载荷都来自 tao 的 `window.root_origin()`，
///   是**窗框**坐标（客户区减去缓存的标题栏高度）。
/// 去装饰后 GDK 的 frame extents 缓存再也不刷新，窗框坐标恒定偏一个标题栏
/// （Muffin 实测 37px），于是「拿载荷比 dock_pos」永远是「差得远」。Windows 上
/// 两者同一个数（`SetWindowPos` 就是窗口矩形），所以那边看不出问题。
/// `inner_position()` 取自真实 configure 事件，去装饰后依旧准确。
fn window_client_position(window: &tauri::WebviewWindow) -> Option<(i32, i32)> {
    let pos = window.inner_position().ok()?;
    Some((pos.x, pos.y))
}

/// 主窗口的客户区绝对位置（`window_client_position` 的主窗快捷入口）。
fn main_client_position(app: &AppHandle) -> Option<(i32, i32)> {
    window_client_position(&app.get_webview_window(MAIN_WINDOW_LABEL)?)
}

/// 诊断钩子：`ANYBOOT_AUTOHIDE_DIAG=1` 且 `ANYBOOT_AUTOHIDE_DIAG_RECALL_MS=<毫秒>`
/// 时，启动 N 毫秒后模拟一次「托盘呼出」（走的是同一条 `ShowOrigin::Recall`）。
///
/// 为什么需要它：呼出保护（800ms 宽限 + 「等待进入」）只有**真的点托盘**才会被走到，
/// 而没有托盘宿主的环境（容器/CI）里这条路径没有任何自动化覆盖 —— 它一旦退化就是
/// 「呼出 → 立刻贴回 → 再呼出」的互闪死循环（用户能看见的那种）。有了这个钩子，
/// 复现台就能跑通「贴着边 → 呼出展开 → 光标仍在窗外不许贴回 → 光标进过窗口再离开
/// → 重新贴边」整条链路。**不设这两个环境变量时它直接 return**，产品路径不变。
fn spawn_diag_recall(app: &AppHandle) {
    if !auto_hide_diag_enabled() {
        return;
    }
    let Ok(raw) = std::env::var("ANYBOOT_AUTOHIDE_DIAG_RECALL_MS") else {
        return;
    };
    let Ok(delay) = raw.trim().parse::<u64>() else {
        return;
    };
    if delay == 0 {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay));
        startup_debug(&app, &format!("diag: 模拟呼叫主窗口（{delay}ms）"));
        show_main_window(&app, ShowOrigin::Recall);
    });
}

/// 诊断钩子：`ANYBOOT_MENU_DIAG=<菜单项 id>`（可选 `ANYBOOT_MENU_DIAG_MS=<毫秒>`，
/// 默认 4000）时，启动 N 毫秒后按这个 id 走一次**完整的菜单派发**
/// （`handle_menu_event`，与真实点击同一条路径）。
///
/// 为什么需要它：无头/自动化环境里点不了托盘菜单（本项目文档里早就记着这个
/// 盲区），于是「菜单 id → MenuAction → 动作」这一段**从来没有自动覆盖**——
/// 而它恰好是「点了没反应」这类报障的第一嫌疑段。配上 `ANYBOOT_POWER_DRY_RUN=1`
/// 就能在不真的关机的前提下验证电源项：
/// `ANYBOOT_MENU_DIAG=power:reboot ANYBOOT_POWER_DRY_RUN=1 anyboot`。
/// **不设就 return**，产品路径不变。
fn spawn_diag_menu(app: &AppHandle) {
    let Ok(id) = std::env::var("ANYBOOT_MENU_DIAG") else {
        return;
    };
    let id = id.trim().to_string();
    if id.is_empty() {
        return;
    }
    let delay = std::env::var("ANYBOOT_MENU_DIAG_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|delay| *delay > 0)
        .unwrap_or(4000);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay));
        startup_debug(&app, &format!("diag: 模拟菜单点击 id={id}（{delay}ms）"));
        handle_menu_event(&app, &id);
    });
}

/// 诊断钩子：`ANYBOOT_AUTOHIDE_DIAG=1` 且 `ANYBOOT_AUTOHIDE_DIAG_FORCE_DOCK_MS=<毫秒>`
/// 时，启动 N 毫秒后直接调一次 `dock_main_window`（跳过「鼠标离窗 + 贴边」
/// 的自然触发条件 —— 自动化截图里没法真的去挪鼠标）。**不设就 return**，
/// 产品路径不变。
fn spawn_diag_force_dock(app: &AppHandle) {
    if !auto_hide_diag_enabled() {
        return;
    }
    let Ok(raw) = std::env::var("ANYBOOT_AUTOHIDE_DIAG_FORCE_DOCK_MS") else {
        return;
    };
    let Ok(delay) = raw.trim().parse::<u64>() else {
        return;
    };
    if delay == 0 {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay));
        startup_debug(&app, &format!("diag: 强制停靠主窗口（{delay}ms）"));
        dock_main_window(&app);
    });
}

/// 主窗口 Moved 事件的统一处理：GRUB 独立窗口跟随 + 位置夹取 + 贴边判定。
fn handle_main_moved(app: &AppHandle, position: tauri::PhysicalPosition<i32>) {
    // 「主窗刚刚还在动」的时间戳：watcher 靠它挡掉「拖动途中就贴边」
    // （见 main_moved_recently）。放在最前面 —— 无论是用户拖动的 Moved、
    // WM 自己摆窗口的 Moved，还是我们自己编程几何的回声，都算「窗口在动」。
    LAST_MAIN_MOVED_MS.store(unix_millis(), Ordering::SeqCst);
    // 最小化时 Windows 会把窗口挪到屏幕外 (−32000,−32000) 并触发 Moved：
    // 不能照常夹取（会跟最小化打架）。
    if is_minimized_position(position) {
        return;
    }
    // 诊断模式下把**每一条** Moved 都记下来：排查「窗口自己挪了一下」时
    // 关键是分清「事件是谁发的」—— 应用侧自己发的（dock / 夹取 / 落位）在日志里
    // 都有自己的行，Moved 出现而前面没有对应行 = WM 动的。默认关，不污染日志。
    if auto_hide_diag_enabled() {
        let (outer, inner) = match app.get_webview_window(MAIN_WINDOW_LABEL) {
            Some(main) => (
                main.outer_position()
                    .map(|p| format!("({},{})", p.x, p.y))
                    .unwrap_or_else(|_| "?".to_string()),
                main.inner_position()
                    .map(|p| format!("({},{})", p.x, p.y))
                    .unwrap_or_else(|_| "?".to_string()),
            ),
            None => ("-".to_string(), "-".to_string()),
        };
        startup_debug(
            app,
            &format!(
                "moved diag: event=({},{}) outer={outer} inner={inner}",
                position.x, position.y
            ),
        );
    }
    let (docked, dock_pos) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (state.auto_hide_docked, state.auto_hide_dock_pos)
    };
    // 我们自己刚编程过窗口几何（dock / undock / 白条自纠）：随后回来的这批
    // Moved 是自己的回声，而抑制期内读到的坐标还可能是中间的（Linux 上
    // set_decorations 翻转会让 GDK 的 frame extents 与 WM 状态错一拍，
    // 回声能偏出一整个标题栏 —— 见 geometry_busy 的注释）。
    // 只挡**停靠态**：只有「刚缩成条」这一种状态会因为回声被判成「用户拖离
    // 白条」而立刻弹回来；undock 之后（docked=false）的夹取路径保持原样
    // 不动（那条路 Windows 上是对的，不去碰）。
    if docked && geometry_busy() {
        startup_debug(
            app,
            &format!(
                "moved: docked + geometry busy -> hold ({},{})",
                position.x, position.y
            ),
        );
        return;
    }
    if main_auto_hide_enabled(app) && docked {
        // 判据两边必须同一个坐标系：`dock_pos` 是 `set_position` 的落点 =
        // **客户区**坐标，而 `position`（Moved 载荷）来自 tao 的
        // `window.root_origin()` = **窗框**坐标，两者恒定差一个标题栏
        // （Muffin 实测 37px）。拿载荷去比 → 「容差内 = 自己的回声」这条
        // 永远不成立：停靠态下**任何**一条 Moved（WM 重发 configure、点白条
        // 引发的 raise/重父化…）都会被判成「用户把白条拖走了」→ 立刻展开、
        // 160ms 后又被 watcher 贴回去 = 白条闪一下（实测：客户区 580,960 只偏
        // 10px，载荷报 580,923，判 release）。所以改读 `inner_position()`：
        // 客户区、来自真实 configure，去装饰后依旧准确 —— 与 watcher 轮询路径
        // 同一个来源、同一个纯函数。Windows 上 outer==inner，那边看不出问题。
        let current = main_client_position(app).unwrap_or((position.x, position.y));
        if docked_move_action(current, dock_pos, AUTO_HIDE_TOLERANCE) == DockedMove::Rest {
            return;
        }
        startup_debug(
            app,
            &format!(
                "moved: docked but off dock_pos ({},{}) vs ({},{}) -> release",
                current.0, current.1, dock_pos.0, dock_pos.1
            ),
        );
        // 用户把停靠的窗口拖离：解除停靠，并广播一次让前端撤掉白条
        // （只在过渡那一刻发，避免拖动过程中 Moved 连发狂刷托盘菜单）。
        let was_docked = {
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            let prev = state.auto_hide_docked;
            state.auto_hide_docked = false;
            state.auto_hide_edge = None;
            prev
        };
        if was_docked {
            trace(app, "autohide", "handle_main_moved: drag-off dock");
            // 用户抓着白条把窗口拖离停靠位：先把贴边时缩掉的尺寸恢复回来，
            // 再走正常夹取 —— 否则窗口会一直保持一条的形状。
            restore_main_dock_size(app);
            publish_ui(app);
        }
    }
    // 「屏幕中间」模式：窗口位置固定，不支持拖动（用户定案 2026-10-13）。
    // 判据只有一条 —— 载荷还在最近落点附近就是「没动」（我们自己的回声、WM
    // 抖动、去装饰后的标题栏错拍），超出才是真拖动；真拖动交给静默期后的
    // `schedule_center_snap` 收回，**拖动全程零 set_position**。
    // 该分支替代 schedule_clamp：居中落点天然在工作区内，不需要再夹取。
    if normalize_window_position(&stored_settings(app).main_window_position) == "center" {
        if !CENTER_HAS_POS.load(Ordering::SeqCst) {
            return; // 还没轮到我们居中过（启动早期 WM 自己摆窗），不动也不提示
        }
        let action = center_move_action(
            (position.x, position.y),
            (
                CENTER_LAST_X.load(Ordering::SeqCst),
                CENTER_LAST_Y.load(Ordering::SeqCst),
            ),
            CENTER_HOLD_TOLERANCE,
        );
        if action == CenterMove::Rest {
            return; // 还停在居中位
        }
        schedule_center_snap(app);
        return;
    }
    // 主窗口必须完整显示：夹回工作区。但不能在拖动过程中夹 —— Moved 连发期间
    // 每次 set_position 都会跟 Windows 的拖动循环互相拉扯（窗口乱跳），
    // 所以只安排一次静默期后的夹取：拖完 250ms 没动静才真正落位。
    schedule_clamp(app);
}

/// 拖动停止后再夹取：Moved 每来一次就刷新一次世代号，静默 250ms 的那个
/// 才真正执行。拖动全程我们一个 set_position 都不发，跟用户零冲突。
fn schedule_clamp(app: &AppHandle) {
    static CLAMP_GENERATION: AtomicU64 = AtomicU64::new(0);
    let generation = CLAMP_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(CLAMP_DELAY_MS));
        if CLAMP_GENERATION.load(Ordering::SeqCst) != generation {
            return; // 拖动还在继续，有更新的自己来
        }
        let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
            return;
        };
        if !main.is_visible().unwrap_or(false) {
            return;
        }
        let Ok(pos) = main.outer_position() else {
            return;
        };
        if is_minimized_position(pos) {
            return;
        }
        // 贴边停靠的位置本来就在工作区外，是刻意为之，别给夹回来
        let docked = {
            let state = app.state::<Mutex<AppState>>();
            let state = state.lock().expect("state mutex poisoned");
            state.auto_hide_docked
        };
        if docked {
            return;
        }
        // undock 重锚还在校准落点时让路：重锚按「客户区 == 期望落点」收敛，
        // clamp 按「窗框在工作区内」判界，两套判据的目标差一个窗框边距
        // （Windows 上 border≈8px 恰好超过微调容差 8px 的边界）—— 叠在一起
        // 就是「展开后窗口左右抖几下、最后停住」（重锚 3 拍放弃后 clamp 定格）。
        // 重锚收敛/放弃后若真越界，后续 Moved 还会再排 clamp，不会漏夹。
        let reanchoring = app
            .state::<Mutex<AppState>>()
            .lock()
            .ok()
            .map(|guard| guard.auto_hide_reanchor.is_some())
            .unwrap_or(false);
        if reanchoring {
            return;
        }
        clamp_main_window(&app, &main);
        // 「上次位置」模式：拖动落点记下来，下次显示还停这儿（用夹取后的真实落点）
        if normalize_window_position(&stored_settings(&app).main_window_position) == "last" {
            if let Ok(final_pos) = main.outer_position() {
                if !is_minimized_position(final_pos) {
                    let mut settings = stored_settings(&app);
                    settings.main_window_last = Some((final_pos.x, final_pos.y));
                    let _ = save_settings(&app, &settings);
                }
            }
        }
    });
}

/// 居中模式下「拖完把窗口收回居中位」：与 `schedule_clamp` 同一套世代号机制 ——
/// 拖动期间 Moved 连发，每条都刷新世代号，只有「静默 `CENTER_SNAP_DELAY_MS`
/// 的那一条」才真的执行。于是**拖动全程零 `set_position`**，窗口平滑跟手、不抖。
///
/// 旧实现是「每条 Moved 当场回中 + 盖上 700ms 回声窗」：拖动中窗口被反复拽回
/// （抖），松手那下又常被自己刚盖的 700ms 回声窗吃掉（停在拖到的位置不回中、
/// 也不提示）——用户报的「弹回中间像点了按钮」和「拖了却不回中」就是这两个
/// 半截凑出来的观感。判据改纯落点比对后，两个症状一起消失。
fn schedule_center_snap(app: &AppHandle) {
    static SNAP_GENERATION: AtomicU64 = AtomicU64::new(0);
    let generation = SNAP_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(CENTER_SNAP_DELAY_MS));
        if SNAP_GENERATION.load(Ordering::SeqCst) != generation {
            return; // 拖动还在继续，有更新的自己来
        }
        let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
            return;
        };
        if !main.is_visible().unwrap_or(false) {
            return; // 隐藏窗口的几何没有意义（视口 0x0 那套）
        }
        if geometry_busy() {
            return; // 别的几何过渡（dock/undock/重锚）正在落位，让路
        }
        let docked = app
            .state::<Mutex<AppState>>()
            .lock()
            .map(|state| state.auto_hide_docked)
            .unwrap_or(false);
        if docked {
            return; // 停靠态的位置归贴边管（拖白条 = 由 docked 分支解除停靠）
        }
        center_main_window(&app);
        // 提示一次「想自己摆就换落位方式」——拖动全程最多 4 秒一条
        let now = unix_millis();
        if now.saturating_sub(LAST_CENTER_HINT_MS.load(Ordering::SeqCst)) >= 4000 {
            LAST_CENTER_HINT_MS.store(now, Ordering::SeqCst);
            record_action(
                &app,
                "window-position",
                false,
                "「屏幕中间」模式下窗口位置固定，不能拖动；想自己摆放请把固定显示位置改成其他选项",
            );
            publish_ui(&app);
        }
    });
}

/// 把主窗口的**客户区**夹到「相交面积最大的显示器」的工作区内。
///
/// 判界和落位都用客户区（本项目判位唯一可信来源）：窗框在 Windows 上含
/// DWM 不可见边框（左右各约 7-8px），按窗框判界会把「客户区贴边」的窗口
/// 误判成越界、往里推 8px —— 与 undock 重锚（客户区语义）打架，就是用户
/// 看到的「展开后窗口左右抖动几下」。落位请求 = 客户区期望落点 − 实测
/// 「窗框→客户区」边距（三端 set_position 带装饰时都是窗框语义，见
/// undock_main_window 的注释）。
fn clamp_main_window(app: &AppHandle, main: &tauri::WebviewWindow) {
    let Ok(client) = main.inner_position() else {
        return;
    };
    let Ok(client_size) = main.inner_size() else {
        return;
    };
    let Ok(outer) = main.outer_position() else {
        return;
    };
    let win_x = client.x;
    let win_y = client.y;
    let win_w = client_size.width as i32;
    let win_h = client_size.height as i32;
    let monitors = match app.available_monitors() {
        Ok(monitors) if !monitors.is_empty() => monitors,
        _ => match main.current_monitor().ok().flatten() {
            Some(monitor) => vec![monitor],
            None => return,
        },
    };
    // 固定显示器优先：选了固定屏就把窗口夹回它的工作区（用户定案：选了
    // 显示器 1 就不许待在显示器 2 —— 拖出去松手后夹回）。没设或那块屏
    // 不在了才按「相交面积最大」自动挑屏（跨屏拖动时自动落到目标屏上，
    // 不会用「重心所在屏」把窗口往回拽）。
    let monitor = {
        let names: Vec<&str> = monitors.iter().map(|m| m.name().map(|s| s.as_str()).unwrap_or("")).collect();
        match fixed_monitor_index(&names, &stored_settings(app).main_fixed_monitor) {
            Some(index) => monitors.into_iter().nth(index),
            None => monitors
                .into_iter()
                .map(|monitor| {
                    let work = monitor.work_area();
                    let overlap_w = (win_x + win_w).min(work.position.x + work.size.width as i32)
                        - win_x.max(work.position.x);
                    let overlap_h = (win_y + win_h).min(work.position.y + work.size.height as i32)
                        - win_y.max(work.position.y);
                    (monitor, overlap_w.max(0) * overlap_h.max(0))
                })
                .max_by_key(|(_, overlap)| *overlap)
                .map(|(monitor, _)| monitor),
        }
    };
    let Some(monitor) = monitor else {
        return;
    };
    let work = monitor.work_area();
    let (x, y) = clamp_to_work(
        win_x,
        win_y,
        win_w,
        win_h,
        work.position.x,
        work.position.y,
        work.size.width as i32,
        work.size.height as i32,
    );
    if x != win_x || y != win_y {
        if auto_hide_diag_enabled() {
            startup_debug(
                app,
                &format!("clamp: client ({win_x},{win_y}) {win_w}x{win_h} -> ({x},{y})"),
            );
        }
        // 客户区期望落点 → 窗框语义请求：扣掉实测「窗框→客户区」边距，
        // 让客户区（而不是窗框）精确落到夹取后的位置。
        let request = (
            x - (client.x - outer.x),
            y - (client.y - outer.y),
        );
        let _ = main.set_position(tauri::PhysicalPosition::new(request.0, request.1));
        pin_main_topmost(app);
    }
}

#[tauri::command]
fn set_main_auto_hide(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_main_auto_hide enabled={enabled}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_auto_hide = enabled;
    save_settings(&app, &settings)?;
    if enabled {
        // 守护线程接管：光标离窗一会儿就贴边（先能看到窗口，别一勾就消失）。
        // 居中模式下它不会起（互斥），见 ensure_auto_hide_watcher。
        ensure_auto_hide_watcher(&app);
    } else {
        undock_main_window(&app);
    }
    Ok(publish_settings(&app))
}

/// 主窗口始终置顶开关（默认勾选）：勾选立即钉到最上层，取消立即放回普通
/// 层级 —— 三端的置顶机制不同（见 Settings.main_always_on_top 注释），这里
/// 是唯一显式「放下」的入口；其余路径（dock/落位/焦点/呼出）只补钉不反向抢。
#[tauri::command]
fn set_main_always_on_top(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_main_always_on_top enabled={enabled}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_always_on_top = enabled;
    save_settings(&app, &settings)?;
    if let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = main.set_always_on_top(enabled);
    }
    Ok(publish_settings(&app))
}

/// 设置贴边隐藏的触发距离（逻辑像素）。0 = 只有真正贴到边才隐藏；
/// 值越大越灵敏。夹在 [0, 200]。
#[tauri::command]
fn set_main_auto_hide_edge_distance(app: AppHandle, distance: i32) -> Result<SettingsView, String> {
    let distance = distance.clamp(0, 200);
    trace(
        &app,
        "command",
        &format!("set_main_auto_hide_edge_distance distance={distance}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_auto_hide_edge_distance = distance;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 贴边白条厚度（逻辑像素，2–40）。停靠时窗口整体缩到这个厚度，
/// 白条从工作区边缘露出，任务栏/Dock 永不被遮挡。
#[tauri::command]
fn set_main_dock_strip_thickness(app: AppHandle, value: i32) -> Result<SettingsView, String> {
    let value = value.clamp(2, 40);
    trace(
        &app,
        "command",
        &format!("set_main_dock_strip_thickness value={value}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_dock_strip_thickness = value;
    save_settings(&app, &settings)?;
    // 白条厚度由前端 CSS 独立渲染（窗口只缩到固定细缝），后端只存值
    Ok(publish_settings(&app))
}

/// 贴边白条长度（沿边方向占屏幕宽/高的百分比，10–100），由前端 CSS 居中截取。
#[tauri::command]
fn set_main_dock_strip_length(app: AppHandle, value: i32) -> Result<SettingsView, String> {
    let value = value.clamp(10, 100);
    trace(
        &app,
        "command",
        &format!("set_main_dock_strip_length value={value}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_dock_strip_length = value;
    save_settings(&app, &settings)?;
    // 长度是白条占「主窗那条边界」的百分比，纯前端 CSS 控制，后端不参与
    Ok(publish_settings(&app))
}

/// 贴边白条不透明度（百分比，10–100）。
#[tauri::command]
fn set_main_dock_strip_opacity(app: AppHandle, value: i32) -> Result<SettingsView, String> {
    let value = value.clamp(10, 100);
    trace(
        &app,
        "command",
        &format!("set_main_dock_strip_opacity value={value}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_dock_strip_opacity = value;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 白条距屏幕边界的距离（逻辑像素，0–100）。0 = 紧贴（既有行为）；
/// 停靠落点沿法线方向离边这条缝。
#[tauri::command]
fn set_main_dock_strip_edge_gap(app: AppHandle, value: i32) -> Result<SettingsView, String> {
    let value = value.clamp(0, 100);
    trace(
        &app,
        "command",
        &format!("set_main_dock_strip_edge_gap value={value}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_dock_strip_edge_gap = value;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 悬浮白条呼出延时（毫秒，0–2000）。0 = 当拍立即呼出（既有行为）。
#[tauri::command]
fn set_main_dock_strip_hover_ms(app: AppHandle, value: u64) -> Result<SettingsView, String> {
    let value = value.clamp(0, 2000);
    trace(
        &app,
        "command",
        &format!("set_main_dock_strip_hover_ms value={value}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_dock_strip_hover_ms = value;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 校验/归一化调色板颜色（#rrggbb，大小写都收，统一转小写返回）。
/// 非法输入返回 None，由调用方决定回退值 —— 绝不把乱码存进设置。
fn normalize_hex_color(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", hex.to_ascii_lowercase()))
}

/// 贴边白条颜色：前端 CSS 渲染（--dock-strip-color），后端只存值。
#[tauri::command]
fn set_main_dock_strip_color(app: AppHandle, color: String) -> Result<SettingsView, String> {
    let color = normalize_hex_color(&color).ok_or_else(|| format!("颜色格式无效: {color}"))?;
    trace(
        &app,
        "command",
        &format!("set_main_dock_strip_color color={color}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_dock_strip_color = color;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 主窗当前是否处于「贴边缩成条」状态（锁失败按未停靠处理，别 panic 掉命令线程）。
fn state_auto_hide_docked(app: &AppHandle) -> bool {
    app.state::<Mutex<AppState>>()
        .lock()
        .map(|guard| guard.auto_hide_docked)
        .unwrap_or(false)
}

/// 贴边热区厚度（逻辑像素 1–100）：停靠态从工作区边往内这一段是鼠标移入
/// 触发呼出的热区。判定纯几何，与窗口槽位厚度无关；可视化由前端 CSS 渲染。
#[tauri::command]
fn set_main_hotzone_thickness(app: AppHandle, value: i32) -> Result<SettingsView, String> {
    let value = value.clamp(1, 100);
    trace(
        &app,
        "command",
        &format!("set_main_hotzone_thickness value={value}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_hotzone_thickness = value;
    save_settings(&app, &settings)?;
    // 热区厚度决定停靠窗口的缝厚（max(槽位, 热区)）：停靠中改动必须重贴一次，
    // 否则可视化层被旧窗口裁着，「渲染的热区」和「判定的热区」又变两个矩形
    if state_auto_hide_docked(&app) {
        dock_main_window(&app);
    }
    Ok(publish_settings(&app))
}

/// 热区调试底色：配合透明度可视化热区边界，前端 CSS 渲染，后端只存值。
#[tauri::command]
fn set_main_hotzone_color(app: AppHandle, color: String) -> Result<SettingsView, String> {
    let color = normalize_hex_color(&color).ok_or_else(|| format!("颜色格式无效: {color}"))?;
    trace(
        &app,
        "command",
        &format!("set_main_hotzone_color color={color}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_hotzone_color = color;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 热区不透明度（0–100，默认 0 = 全透明 = 看不见）。
#[tauri::command]
fn set_main_hotzone_opacity(app: AppHandle, value: i32) -> Result<SettingsView, String> {
    let value = value.clamp(0, 100);
    trace(
        &app,
        "command",
        &format!("set_main_hotzone_opacity value={value}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_hotzone_opacity = value;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 热区是否画边框（默认关）。
#[tauri::command]
fn set_main_hotzone_border(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_main_hotzone_border enabled={enabled}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_hotzone_border = enabled;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 热区边框颜色。
#[tauri::command]
fn set_main_hotzone_border_color(app: AppHandle, color: String) -> Result<SettingsView, String> {
    let color = normalize_hex_color(&color).ok_or_else(|| format!("颜色格式无效: {color}"))?;
    trace(
        &app,
        "command",
        &format!("set_main_hotzone_border_color color={color}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_hotzone_border_color = color;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 主题强调色：前端据此派生按钮/链接色（--accent 系列），后端只存值。
#[tauri::command]
fn set_accent_color(app: AppHandle, color: String) -> Result<SettingsView, String> {
    let color = normalize_hex_color(&color).ok_or_else(|| format!("颜色格式无效: {color}"))?;
    trace(&app, "command", &format!("set_accent_color color={color}"));
    let mut settings = stored_settings(&app);
    settings.accent_color = color;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

#[tauri::command]
fn set_main_window_position(app: AppHandle, mode: String) -> Result<SettingsView, String> {
    let mode = normalize_window_position(&mode).to_string();
    trace(
        &app,
        "command",
        &format!("set_main_window_position mode={mode}"),
    );
    let mut settings = stored_settings(&app);
    settings.main_window_position = mode.clone();
    save_settings(&app, &settings)?;
    // 立刻按新模式落位（free 模式不动，center 立刻居中，last 立刻回到记忆点）
    apply_main_window_position(&app);
    // 「屏幕中间」与贴边自动隐藏互斥（见 auto_hide_should_run）：切进居中模式
    // 要停掉 watcher（它在下一拍自己退出），切出去要按需重新拉起 ——
    // 否则从居中切回自由摆放后贴边就永久失效了。开关值本身不动。
    ensure_auto_hide_watcher(&app);
    if mode == "center" && settings.main_auto_hide {
        record_action(
            &app,
            "window-position",
            false,
            "「屏幕中间」与贴边自动隐藏互斥：居中期间贴边已停用，切回其他落位方式后自动恢复",
        );
    }
    Ok(publish_settings(&app))
}

/// 托盘菜单方式：classic（三端各自方式，默认）/ unified（positioner 全局统一）。
/// 落位在下次弹菜单时按新值走（tray_menu_present 每次现读设置）；菜单形态
/// （SNI 的 Menu 属性）由宿主在注册时缓存，GNOME 上切完重启应用才完全生效。
#[tauri::command]
fn set_tray_menu_style(app: AppHandle, mode: String) -> Result<SettingsView, String> {
    let mode = normalize_tray_menu_style(&mode).to_string();
    trace(
        &app,
        "command",
        &format!("set_tray_menu_style mode={mode}"),
    );
    let mut settings = stored_settings(&app);
    if settings.tray_menu_style != mode {
        settings.tray_menu_style = mode.clone();
        save_settings(&app, &settings)?;
        // 用户可见动作留痕（常开通道）：两种形态在启动时二选一，切换后重启生效
        startup_debug(&app, &format!("托盘菜单方式 → {mode}（重启应用后生效）"));
    }
    Ok(publish_settings(&app))
}

/// 前端白条渲染取证：停靠态下前端把 #dock-strip 的渲染矩形（CSS px）+
/// 视口尺寸 + DPR 报上来，与后端 `dock done` 的窗口物理矩形对拍 —— 白条
/// 的物理落点 = 窗口客户区原点 + rect×DPR，一眼算出「画在哪」。
#[tauri::command]
fn report_dock_strip_rect(
    app: AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    viewport_width: f64,
    viewport_height: f64,
    dpr: f64,
) {
    startup_debug(
        &app,
        &format!(
            "strip rect css=({x},{y} {width}x{height}) viewport={viewport_width}x{viewport_height} dpr={dpr}"
        ),
    );
}

/// 固定显示器（auto / 显示器名，与设置页下拉、托盘子菜单同源）：
/// 落盘 + 立即把主窗约束到新显示器。停靠态先展开再按新屏落位、
/// 然后重新贴边 —— 白条会贴到新屏的边上。
#[tauri::command]
fn set_main_fixed_monitor(app: AppHandle, name: String) -> Result<SettingsView, String> {
    let name = normalize_fixed_monitor(&name);
    trace(&app, "command", &format!("set_main_fixed_monitor name={name}"));
    let docked = app
        .state::<Mutex<AppState>>()
        .lock()
        .map(|state| state.auto_hide_docked)
        .unwrap_or(false);
    let mut settings = stored_settings(&app);
    settings.main_fixed_monitor = name;
    save_settings(&app, &settings)?;
    if docked {
        // 停靠态换固定屏：展开+显示到新屏（show_main_window 自带
        // undock → apply 落位），白条什么时候贴回交给 watcher 的常规节奏
        // （光标离开窗口后）。这里**不能**手动补 dock：undock 的几何是
        // 异步落地的，紧跟着的 dock 读到搬运前的旧坐标，实测把白条贴到
        // 了另一块屏上（target 混了两块屏的工作区）。
        show_main_window(&app, ShowOrigin::Recall);
        // show → undock 武装的重锚瞄准**旧屏**的停靠位；enforce 已把窗口
        // 搬到新屏，留着它会把窗口往旧屏拽 —— 清掉（同 center_main_window
        // 停靠态分支的处理）。
        if let Ok(mut state) = app.state::<Mutex<AppState>>().lock() {
            state.auto_hide_reanchor = None;
        }
    } else {
        // center 模式直接按新屏重居中；其他模式把窗口约束进新屏
        apply_main_window_position(&app);
    }
    Ok(publish_settings(&app))
}

/// 切换 GRUB 区块的展示方式（内嵌 / 加宽 / 列表独立 / GRUB 独立）：
/// 落盘 + 按新模式调窗口宽度（460 ⇄ 932）。布局搬移由前端按快照里的
/// grubListView 自己做 —— 后端只管设置与窗口尺寸这两件它该管的事。
#[tauri::command]
fn set_grub_list_view(app: AppHandle, mode: String) -> Result<SettingsView, String> {
    let mode = normalize_grub_list_view(&mode).to_string();
    trace(&app, "command", &format!("set_grub_list_view mode={mode}"));
    let mut settings = stored_settings(&app);
    settings.grub_list_view = mode;
    save_settings(&app, &settings)?;
    apply_grub_layout_size(&app, true);
    Ok(publish_settings(&app))
}

/// GRUB 列表展示格式（原始/极简/简易/中等/复杂）：纯展示偏好，落盘 + 广播，
/// 渲染完全由前端按快照里的 grubListFormat 做 —— 后端不碰数据。
#[tauri::command]
fn set_grub_list_format(app: AppHandle, format: String) -> Result<SettingsView, String> {
    let format = normalize_grub_list_format(&format).to_string();
    trace(&app, "command", &format!("set_grub_list_format format={format}"));
    let mut settings = stored_settings(&app);
    settings.grub_list_format = format;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 设置窗口尺寸（滑条拖动 / 用户拖窗口边框都走这里，双向实时）。
#[tauri::command]
fn set_window_size(app: AppHandle, width: u32, height: u32) -> Result<SettingsView, String> {
    trace(
        &app,
        "command",
        &format!("set_window_size {width}x{height}"),
    );
    set_main_window_size(&app, width, height)?;
    Ok(publish_settings(&app))
}

/// 锁定 / 解锁窗口大小：锁定 = 不可拖拽缩放 + 不可最大化 + 滑条停用（前端）。
#[tauri::command]
fn set_window_size_locked(app: AppHandle, locked: bool) -> Result<SettingsView, String> {
    trace(&app, "command", &format!("set_window_size_locked {locked}"));
    let mut settings = stored_settings(&app);
    settings.window_size_locked = locked;
    save_settings(&app, &settings)?;
    apply_window_size_lock(&app);
    Ok(publish_settings(&app))
}

/// 重置窗口大小到当前展示方式的标称尺寸（托盘菜单与设置页共用）。
/// 宽随模式（460 ⇄ 932）、高回到「整屏高度的 2/3」（默认高度）。
fn reset_window_size_inner(app: &AppHandle) -> Result<SettingsView, String> {
    let mode = normalize_grub_list_view(&stored_settings(app).grub_list_view);
    let width = grub_layout_width(mode) as u32;
    set_main_window_size(app, width, default_window_height(app))?;
    kick_webview_repaint(app);
    Ok(publish_settings(app))
}

/// Linux/NVIDIA：改几何（尤其跨屏落位）后 WebKit 的损伤合成偶尔不交新帧
/// —— 窗口内容一片空白。借 dock/undock 同款招数：清屏一拍再放开，强制
/// WebKit 提交完整新帧（用户实测「重置窗口大小/位置后内容变空白」）。
/// 只挂 Linux；Windows/macOS 的 resize 重绘一直正常，不折腾。
#[cfg(target_os = "linux")]
fn kick_webview_repaint(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        // 等尺寸/落位这两拍异步几何先落地再做清屏-放开，否则清的是旧帧
        std::thread::sleep(std::time::Duration::from_millis(450));
        let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
            return;
        };
        let _ = main.eval("document.documentElement.classList.add('dock-pending');");
        std::thread::sleep(std::time::Duration::from_millis(80));
        let _ = main.eval("document.documentElement.classList.remove('dock-pending');");
    });
}

/// 非 Linux：重绘一直正常，无操作。
#[cfg(not(target_os = "linux"))]
fn kick_webview_repaint(_app: &AppHandle) {}

#[tauri::command]
fn reset_window_size(app: AppHandle) -> Result<SettingsView, String> {
    trace(&app, "command", "reset_window_size");
    reset_window_size_inner(&app)
}

#[tauri::command]
fn reset_main_window_position(app: AppHandle) -> Result<SettingsView, String> {
    trace(&app, "command", "reset_main_window_position");
    let center = center_main_window(&app);
    // 「上次位置」模式下，把居中点记成新的 last，下次显示仍停这儿
    if normalize_window_position(&stored_settings(&app).main_window_position) == "last" {
        if let Some(c) = center {
            let mut settings = stored_settings(&app);
            settings.main_window_last = Some(c);
            save_settings(&app, &settings)?;
        }
    }
    // 跨屏落位后 WebKit 可能不交新帧（内容空白），踢一脚强制重绘
    kick_webview_repaint(&app);
    Ok(publish_settings(&app))
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
        "用户取消",
    );
    trace(app, "remote", &format!("待执行请求 {} 已取消", pending.id));
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

/// URL 查询值的最小百分号编码：保留 RFC 3986 unreserved（字母数字 - _ . ~），
/// 其余逐字节 %XX —— selector 里的空格、中文、括号都能安全进 query。
fn percent_encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn http_access_hint_text(app: &AppHandle) -> String {
    let settings = stored_settings(app);
    let port = settings.http_port;
    let token = settings.http_token.trim();
    let ip = local_ipv4();
    // GRUB 示例里的 selector 用**实际解析出的第一条**（实时替换，含百分号编码）；
    // 一条都没解析出来就用占位符，别给用户一个复制了必 404 的假例子。
    let grub_entry_example = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        state
            .grub
            .entries
            .first()
            .map(|entry| percent_encode_component(&entry.selector))
            .unwrap_or_else(|| "<启动项selector>".to_string())
    };
    format!(
        "# 局域网/远程（把 IP 换成目标机地址）\n\
         curl -s -X POST \"http://{ip}:{port}/api/power?action=reboot\" -H \"X-Auth-Token: {token}\"\n\
         # 本机 / ssh 隧道\n\
         curl -s \"http://127.0.0.1:{port}/api/status?token={token}\"\n\
         # 切换下次启动并重启（一条命令搞定）\n\
         curl -s -X POST \"http://{ip}:{port}/api/switch-and-power?system=windows&action=reboot\" -H \"X-Auth-Token: {token}\"\n\
         # 只切换、不重启\n\
         curl -s -X POST \"http://{ip}:{port}/api/switch?system=linux\" -H \"X-Auth-Token: {token}\"\n\
         # GRUB 启动项：列出 ESP 上 grub.cfg 里的条目（selector 原文）\n\
         curl -s \"http://127.0.0.1:{port}/api/grub?token={token}\"\n\
         # 切换 GRUB 启动项（selector 含空格等要 URL 编码，空格 = %20）\n\
         curl -s -X POST \"http://127.0.0.1:{port}/api/grub-switch?entry={grub_entry_example}\" -H \"X-Auth-Token: {token}\""
    )
}

/// Web 控制页地址（纯函数，便于单测）。
///
/// host 跟着监听地址走：绑 `0.0.0.0` 时换成本机局域网 IP（本机和同网段都能开），
/// 绑回环就用回环，其余按用户填的地址原样用。token 走百分号编码 ——
/// 服务端的 `find_pair` 会解码，这样含 `&`、`+`、`#` 的自定义 token 也不会散架。
fn web_page_url(host: &str, port: u16, token: &str) -> String {
    let host = host.trim();
    let host = if host.is_empty() || host == "0.0.0.0" {
        local_ipv4()
    } else if host == "::" || host == "[::]" {
        "127.0.0.1".to_string()
    } else {
        host.to_string()
    };
    // IPv6 字面量在 URL 里必须加方括号
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host
    };
    format!(
        "http://{host}:{port}/?token={}",
        percent_encode_component(token.trim())
    )
}

/// 拼出「此刻真的能打开」的 Web 页地址：没 token / 没在监听就直接说明原因，
/// 免得给用户开一个必然打不开的页面（打开浏览器失败很难自查）。
fn current_web_page_url(app: &AppHandle) -> Result<String, String> {
    let settings = stored_settings(app);
    let token = settings.http_token.trim().to_string();
    if token.is_empty() {
        return Err(
            "远程控制还没有访问令牌：token 为空时 HTTP 服务不会启动，先去「基本设置 ▸ 远程控制」生成一个"
                .to_string(),
        );
    }
    let (listening, addr) = {
        let runtime = http_lock();
        match runtime.server.as_ref() {
            Some(server) => (true, Some(server.addr.clone())),
            None => (false, None),
        }
    };
    if !listening {
        return Err(
            "HTTP 服务当前没有在监听：先在「基本设置 ▸ 远程控制」里打开它（或看是不是端口被占了）"
                .to_string(),
        );
    }
    // 以**实际监听到的端口**为准：配置改了但还没重启时会不一致
    let port = addr
        .as_deref()
        .and_then(|addr| addr.rsplit(':').next())
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or(settings.http_port);
    Ok(web_page_url(&settings.http_bind, port, &token))
}

/// 用系统默认浏览器打开 Web 控制页，返回真正打开的地址（给界面回显用）。
fn open_web_page_now(app: &AppHandle) -> Result<String, String> {
    let url = current_web_page_url(app)?;
    open_url_in_browser(&url)?;
    Ok(url)
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

/// 改 token 专用：保存 → **关掉旧监听、按新 token 重新绑定端口** → 拿重启后的实况当状态文案。
///
/// 为什么不复用 `update_settings_and_publish`：那个函数的文案在重启**之前**就定死了，
/// 用户根本看不到「到底有没有重新监听、重新绑在哪个地址上」——而这正是改 token 之后
/// 最该看见的一句话。旧实现的文案写着「监听已按新 token 重启」，但那是**假设**，不是事实。
fn update_settings_and_restart_http(
    app: &AppHandle,
    action: &str,
    update: impl FnOnce(&mut Settings),
) -> Result<String, String> {
    let mut settings = stored_settings(app);
    update(&mut settings);
    match save_settings(app, &settings) {
        Ok(_) => {
            // 这里就是「关旧端口 → 开新端口」那一步（apply_http_config 内部先 stop 再 start）
            apply_http_config(app);
            let outcome = http_listener_summary();
            record_action(app, action, true, &outcome);
            publish_settings(app);
            Ok(outcome)
        }
        Err(error) => {
            record_action(app, action, false, &error);
            publish_settings(app);
            Err(error)
        }
    }
}

/// 监听重启后的实况（正在监听哪个地址 / 为什么没听起来）。纯读，不改任何状态。
fn http_listener_summary() -> String {
    let runtime = http_lock();
    match runtime.server.as_ref() {
        Some(server) => format!("监听已重启：{}", server.addr),
        None => runtime
            .last_error
            .clone()
            .unwrap_or_else(|| "监听未运行（HTTP 服务已关闭）".to_string()),
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

/// 延迟子菜单标题带上当前值：两渠道相同时显示一个数，不同时分别列出 ——
/// 否则值不同时所有勾选项都不勾，用户在托盘里无从知道现在延迟是多少。
fn delay_submenu_title(settings: &Settings) -> String {
    let describe = |seconds: u32| {
        if seconds == 0 {
            "立即执行".to_string()
        } else {
            format!("{seconds}秒")
        }
    };
    let http = settings.channels.http.delay_seconds;
    let cli = settings.channels.cli.delay_seconds;
    if http == cli {
        format!("远程执行延迟（{}）", describe(http))
    } else {
        format!(
            "远程执行延迟（HTTP {} / CLI {}）",
            describe(http),
            describe(cli)
        )
    }
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

/// 选择项的「勾」用**纯文本**画在标签里，不用原生勾选控件。
///
/// 为什么（2026-10-14 实测，见 `.workbuddy/memory/2026-10-14.md`）：托盘菜单在
/// Linux 上并不是我们画的 —— 应用把菜单导出成 dbusmenu，宿主（Cinnamon 面板的
/// xapp-status applet + xapp-sn-watcher 的桥）按那份导出**自己重新渲染**，勾选态
/// 同样走 label 里的文字标记（下方 build_menu_snapshot：不导出原生 toggle 属性）。
/// 原生勾选项在这条链路上必然出错：
///
/// ① **勾被取消**：点一下勾选项，GTK 会**先自己把控件翻转**再通知我们。若这次点击
///    没有改变设置（在单选组里再点一次已选中的那个），菜单指纹不变 → 我们不重建
///    → 导出里那一项永久停在「未勾选」，宿主照单画出来，整个单选组一个勾都不剩。
/// ② **勾跑到别的行上**：每次重建菜单，dbusmenu 的 item id 会整片重新分配
///    （实测「开机启动」从 119 变成 171），宿主手里那份还在用旧 id，
///    勾选态就会落到相邻的另一行上（用户报的「点启动时隐藏，误勾选开机启动」）。
///
/// 把状态放进**标签文字**，就与控件无关、与宿主无关：`☑`/`☐`（开关）、`◉`/`○`
/// （单选）。单选组「必须有且只有一个选中」由我们算标签时保证，不靠任何工具包。
/// 同一份规则在 GTK / Win32 / AppKit 三种菜单上表现一致 —— 这就是「三端统一」。
const TOGGLE_ON: &str = "☑";
const TOGGLE_OFF: &str = "☐";
const RADIO_ON: &str = "◉";
const RADIO_OFF: &str = "○";

/// 开关项标签：`☑ 开机启动` / `☐ 开机启动`。
fn toggle_label(selected: bool, text: &str) -> String {
    format!("{} {text}", if selected { TOGGLE_ON } else { TOGGLE_OFF })
}

/// 单选项标签：`◉ 屏幕中间` / `○ 自由摆放`。
fn radio_label(selected: bool, text: &str) -> String {
    format!("{} {text}", if selected { RADIO_ON } else { RADIO_OFF })
}

/// GRUB 条目的展示文字：随列表格式变（与 `ui/grub-list-view.js` 的 `rowLabel`
/// **同一条规则** —— 极简/简易 = 去前缀内核名；中等 = 完整内核名；其余 = 菜单标题）。
fn grub_entry_label(entry: &grub::GrubEntry, list_format: &str) -> String {
    // JS 的 `(entry.kernels && entry.kernels[0]) || entry.name`：空内核名也回落到菜单标题
    let kernel = entry
        .kernels
        .first()
        .filter(|kernel| !kernel.is_empty())
        .map(String::as_str)
        .unwrap_or(&entry.name);
    match list_format {
        "minimal" | "simple" => {
            let stripped = kernel
                .strip_prefix("vmlinuz-")
                .or_else(|| kernel.strip_prefix("linux-"))
                .unwrap_or(kernel);
            stripped.to_string()
        }
        "medium" => kernel.to_string(),
        _ => entry.name.clone(),
    }
}

/// 「下次启动」的级联文字：rEFInd 层 → GRUB 层。
///
/// 与主窗口顶部那行大字（`ui/app.js` 的 `nextBootText`）**同一条规则**：
/// rEFInd 选中的不是 Linux（Windows/macOS 是直接引导），或者 GRUB 侧还没选过
/// → 只显示 rEFInd 这一层；选中 Linux 且能手拿把掐找到那个 GRUB 条目 →
/// 「Linux › 组名 › 条目文字」。托盘顶部、主窗口大字说的是同一个东西，
/// **改这里的规则请同步改 `ui/app.js`**（两处一字不差是用户的要求）。
fn next_boot_cascade(variables: &[VariableInfo], grub: &GrubView) -> String {
    let Some(active) = variables.iter().find(|variable| variable.active) else {
        return "未知（没有选中的系统）".to_string();
    };
    let mut text = active.display_name.clone();
    if active.system != "linux" {
        return text;
    }
    let selector = grub
        .next_entry
        .as_deref()
        .or(grub.saved_entry.as_deref())
        .unwrap_or("");
    let Some(entry) = grub.entries.iter().find(|entry| entry.selector == selector) else {
        return text;
    };
    if let Some(submenu) = &entry.submenu {
        let group = grub
            .groups
            .iter()
            .find(|group| &group.id == submenu)
            .map(|group| group.name.as_str())
            .unwrap_or(submenu.as_str());
        text.push_str(&format!(" › {group}"));
    }
    text.push_str(&format!(" › {}", grub_entry_label(entry, &grub.list_format)));
    text
}

/// 托盘顶部那行状态文案：只在**和「下次启动：X」不一样**的时候才显示。
///
/// 扫描成功之后 `status.message` 本身就是「下次启动：X」，和顶部那行一字不差；
/// 老实现照单再摆一行，于是菜单里出现两条「下次启动：…」（用户报的重复）。
/// 只有错误提示、临时操作结果这类「额外信息」才值得单独一行 —— 抽成纯函数方便单测。
fn menu_status_line<'a>(message: &'a str, next_boot_label: &str) -> Option<&'a str> {
    let message = message.trim();
    if message.is_empty() || message == next_boot_label {
        return None;
    }
    Some(message)
}

/// —— 托盘菜单的**数据模型**（三端统一的地基）——
///
/// 菜单内容只在这里生成一次，而且是**纯数据**：不碰 GTK / Win32 / AppKit 任何控件。
/// 三端各自把这份模型渲染出来 —— 因此「同一份菜单」在三端必然一致。
/// 顺带解决一个老隐患：构造原生菜单会真的去创建工具包控件，而重建菜单的调用点
/// 可能在非主线程（HTTP 线程拿着菜单点击 → publish → 重建），GTK 那边并不欢迎。
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
struct MenuNode {
    /// 点击时回传给应用的动作 id；分隔线与表头没有 id
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    /// 纯文本。开关/单选的勾已经画在里面（见 toggle_label / radio_label）
    label: String,
    enabled: bool,
    /// 结构：`item`（可点）/ `header`（分区标题）/ `sep`（分隔线）/ `sub`（子菜单）
    kind: &'static str,
    /// 列表类条目的「当前项」标记（系统列表、GRUB 条目、顶部「下次启动」那行）；
    /// 不是这类条目时是 None（勾在 label 里，不重复表达）
    #[serde(skip_serializing_if = "Option::is_none")]
    checked: Option<bool>,
    /// 点完这一项**要不要留着菜单**（用户诉求③：涉及选择框的都不要关）。
    ///
    /// 这条规则**只在这里算一次**，随模型一起发给前端，前端照字段办事、不重算一遍 ——
    /// 否则「哪些算选择框」就有了两份实现，迟早漂移（第一版就是这么写的，已改掉）。
    #[serde(rename = "keepsOpen")]
    keeps_open: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<MenuNode>,
}

/// 动作条目（扫描 / 显示主窗口 / 电源 / 退出…）：点完关菜单。
fn menu_item(id: impl Into<String>, label: impl Into<String>, enabled: bool) -> MenuNode {
    MenuNode {
        id: Some(id.into()),
        label: label.into(),
        enabled,
        kind: "item",
        checked: None,
        keeps_open: false,
        children: Vec::new(),
    }
}

/// **选择条目**：开关（☑/☐）或单选（◉/○）。点完**不关菜单** —— 可以连着勾几个。
///
/// 勾/单选标记必须已经画在 label 里（toggle_label / radio_label），这是本项成立的前提：
/// 标记是文字，与工具包无关，谁渲染都一样。
fn menu_choice_item(id: impl Into<String>, label: impl Into<String>, enabled: bool) -> MenuNode {
    MenuNode {
        keeps_open: true,
        ..menu_item(id, label, enabled)
    }
}

/// 列表类**动作**条目：带「当前项」标记，但点它是动作（顶部「下次启动」→ 打开主窗口）。
fn menu_marked_item(
    id: impl Into<String>,
    label: impl Into<String>,
    enabled: bool,
    marked: bool,
) -> MenuNode {
    MenuNode {
        checked: Some(marked),
        ..menu_item(id, label, enabled)
    }
}

/// 列表类**选择**条目：带「当前项」标记，点它就是把「下次启动」换成它 —— 属单选组，点完留着菜单。
fn menu_marked_choice_item(
    id: impl Into<String>,
    label: impl Into<String>,
    enabled: bool,
    marked: bool,
) -> MenuNode {
    MenuNode {
        keeps_open: true,
        ..menu_marked_item(id, label, enabled, marked)
    }
}

/// 分区标题：禁用项，配合上下分隔线把菜单切成独立的几块（原生菜单唯一的表达能力）。
fn menu_header(id: impl Into<String>, label: impl Into<String>) -> MenuNode {
    MenuNode {
        id: Some(id.into()),
        label: label.into(),
        enabled: false,
        kind: "header",
        checked: None,
        keeps_open: false,
        children: Vec::new(),
    }
}

/// 分隔线。
fn menu_separator() -> MenuNode {
    MenuNode {
        id: None,
        label: String::new(),
        enabled: false,
        kind: "sep",
        checked: None,
        keeps_open: false,
        children: Vec::new(),
    }
}

/// 子菜单。
fn menu_submenu(
    id: impl Into<String>,
    label: impl Into<String>,
    children: Vec<MenuNode>,
) -> MenuNode {
    MenuNode {
        id: Some(id.into()),
        label: label.into(),
        enabled: true,
        kind: "sub",
        checked: None,
        keeps_open: false,
        children,
    }
}

/// GRUB 启动项那一块（进顶层菜单 = 平铺展开；进子菜单 = 收起）。
///
/// 与主窗 GRUB 区**同一套列表格式逻辑**（`ui/grub-list-view.js` 的 renderGrubEntries）：
/// * 当前选中项置顶（原位照常保留 —— 同一条出现两次是主窗定案的行为）；
/// * 组头 = ▸ + 系统图标 + 组名 +（N 项[ · ESP 位置]），位置按格式带不带（极简/原始不带）；
/// * 条目文字随 `grub.listFormat` 变（极简/简易 = 去前缀内核名、中等 = 完整内核名、
///   原始/复杂 = 完整标题），非原始格式按渲染结果折叠同名条目（首个出面、其余并成
///   「· N 个变体」）；点击写 grubenv（`grub-switch:<selector>`），与主窗完全同源。
fn grub_menu_nodes(grub: &GrubView, busy: bool) -> Vec<MenuNode> {
    let mut items: Vec<MenuNode> = Vec::new();
    if grub.entries.is_empty() {
        items.push(menu_item(
            "grub-empty",
            if grub.partuuid.is_empty() && grub.dir.is_empty() {
                "GRUB：还没配置 ESP"
            } else {
                "GRUB：没解析到启动项"
            },
            false,
        ));
        return items;
    }
    let format = normalize_grub_list_format(&grub.list_format);
    let active = grub
        .next_entry
        .as_deref()
        .or(grub.saved_entry.as_deref())
        .unwrap_or("");
    let group_titles: std::collections::HashMap<&str, &str> = grub
        .groups
        .iter()
        .map(|group| (group.id.as_str(), group.name.as_str()))
        .collect();
    let mut children_of: std::collections::HashMap<&str, Vec<&grub::GrubEntry>> =
        std::collections::HashMap::new();
    for entry in &grub.entries {
        if let Some(submenu) = &entry.submenu {
            children_of.entry(submenu.as_str()).or_default().push(entry);
        }
    }
    // 组头附加的 ESP 位置（简易/中等/复杂带，极简/原始不带 —— 与主窗同一规则）
    let location = if format == "minimal" || format == "raw" {
        String::new()
    } else {
        let mut parts: Vec<String> = Vec::new();
        if let Some(dir) = &grub.efi_dir {
            parts.push(format!("位于{dir}"));
        }
        let uuid = (!grub.partuuid.is_empty())
            .then(|| format!("UUID={}", grub.partuuid))
            .or_else(|| {
                grub.esp
                    .as_ref()
                    .and_then(|esp| esp.partuuid.clone().or_else(|| esp.uuid.clone()))
                    .map(|uuid| format!("UUID={uuid}"))
            });
        if let Some(uuid) = uuid {
            parts.push(uuid);
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" · {}", parts.join(" "))
        }
    };

    // 置顶当前选中项（组内子项补「组名 › 」前缀，与主窗置顶行同一规则）
    if let Some(pinned) = grub.entries.iter().find(|entry| entry.selector == active) {
        let prefix = pinned
            .submenu
            .as_deref()
            .and_then(|id| group_titles.get(id))
            .map(|title| format!("{title} › "))
            .unwrap_or_default();
        grub_push_entry(
            &mut items,
            format,
            pinned,
            true,
            &prefix,
            &std::collections::HashMap::new(),
            &mut std::collections::HashSet::new(),
            busy,
            active,
        );
    }

    // 主体：顶层一套折叠状态，各组独立一套（与主窗一致）；原始格式不折叠
    let mut emitted_groups = std::collections::HashSet::new();
    let top_counts = grub_collapse(
        format,
        grub.entries
            .iter()
            .filter(|entry| entry.submenu.is_none()),
    );
    let mut top_emitted = std::collections::HashSet::new();
    for entry in &grub.entries {
        let Some(submenu_id) = &entry.submenu else {
            grub_push_entry(
                &mut items,
                format,
                entry,
                true,
                "",
                &top_counts,
                &mut top_emitted,
                busy,
                active,
            );
            continue;
        };
        if !emitted_groups.insert(submenu_id.as_str()) {
            continue;
        }
        let title = group_titles
            .get(submenu_id.as_str())
            .copied()
            .unwrap_or(submenu_id.as_str());
        let count = children_of
            .get(submenu_id.as_str())
            .map(Vec::len)
            .unwrap_or(0);
        items.push(menu_header(
            format!("grub-group:{submenu_id}"),
            format!(
                "▸ {}{title}（{count} 项{}）",
                grub_os_icon(title),
                location
            ),
        ));
        let counts = grub_collapse(
            format,
            children_of
                .get(submenu_id.as_str())
                .into_iter()
                .flatten()
                .copied(),
        );
        let mut emitted = std::collections::HashSet::new();
        for child in children_of.get(submenu_id.as_str()).into_iter().flatten() {
            grub_push_entry(
                &mut items,
                format,
                child,
                false,
                "",
                &counts,
                &mut emitted,
                busy,
                active,
            );
        }
    }
    items
}

/// 条目/组名的系统图标：windows → 🪟，mac/apple → 🍎，linux → 🐧，都不中就不加
/// （与主窗 grubOsIcon 同一规则，不猜）。
fn grub_os_icon(text: &str) -> &'static str {
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("windows") {
        "🪟 "
    } else if lowered.contains("mac") || lowered.contains("apple") {
        "🍎 "
    } else if lowered.contains("linux") {
        "🐧 "
    } else {
        ""
    }
}

/// 条目的内核名：优先 `linux` 行解析出的内核文件名，取不到退回标题原文。
fn grub_kernel_of(entry: &grub::GrubEntry) -> String {
    entry
        .kernels
        .first()
        .cloned()
        .unwrap_or_else(|| entry.name.clone())
}

/// 去内核名前缀：先去 `vmlinuz-`，再去 `linux-`（与主窗 grubStripKernel 一致）。
fn grub_strip_kernel(name: &str) -> String {
    let stripped = name.strip_prefix("vmlinuz-").unwrap_or(name);
    stripped.strip_prefix("linux-").unwrap_or(stripped).to_string()
}

/// 行文字随 `grub.listFormat` 变（与主窗 rowLabel 一致）。
fn grub_row_label(format: &str, entry: &grub::GrubEntry) -> String {
    match format {
        "minimal" | "simple" => grub_strip_kernel(&grub_kernel_of(entry)),
        "medium" => grub_kernel_of(entry),
        _ => entry.name.clone(),
    }
}

/// 按渲染结果数一遍变体数（非原始格式用；原始格式不折叠，回空表 —— 变体数
/// 已在解析期并进 `entry.variants`）。
fn grub_collapse<'a>(
    format: &str,
    entries: impl Iterator<Item = &'a grub::GrubEntry>,
) -> std::collections::HashMap<String, usize> {
    let mut counts = std::collections::HashMap::new();
    if format == "raw" {
        return counts;
    }
    for entry in entries {
        *counts.entry(grub_row_label(format, entry)).or_insert(0) += 1;
    }
    counts
}

/// 出一行 GRUB 条目：非原始格式先按渲染结果折叠（重复的只出面一次，与主窗同一规则），
/// 行文字 = **缩进箭头 + 系统图标** +（组前缀）+ 格式化文字 + 行尾附注（变体数/内核）。
/// 箭头在图标的**前面** —— 与主窗 `makeRow`（`arrow + icon + text`）逐字对齐，
/// 组头那行也是「▸ 图标 组名」同一顺序。
/// 置顶行带 `group_prefix`（组名 › ）、不做缩进箭头；原位行顶层无箭头、组内 ↳。
fn grub_push_entry(
    items: &mut Vec<MenuNode>,
    format: &str,
    entry: &grub::GrubEntry,
    top: bool,
    group_prefix: &str,
    counts: &std::collections::HashMap<String, usize>,
    emitted: &mut std::collections::HashSet<String>,
    busy: bool,
    active: &str,
) {
    let label = grub_row_label(format, entry);
    let variants = if format == "raw" {
        entry.variants.max(1)
    } else if !emitted.insert(label.clone()) {
        return;
    } else {
        counts.get(&label).copied().unwrap_or(1)
    };
    let mut parts: Vec<String> = Vec::new();
    if variants > 1 {
        parts.push(format!("{variants} 个变体"));
    }
    if format == "raw" || format == "detailed" {
        let tag = if !entry.kernels.is_empty() {
            entry.kernels[0].clone()
        } else if entry.chainloader {
            "转交引导".to_string()
        } else {
            entry.class.clone().unwrap_or_default()
        };
        if !tag.is_empty() {
            parts.push(tag);
        }
    }
    let suffix = if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    };
    let icon = grub_os_icon(&format!("{group_prefix}{}", entry.name));
    let arrow = if top { "" } else { "↳ " };
    items.push(menu_marked_choice_item(
        format!("grub-switch:{}", entry.selector),
        format!("{arrow}{icon}{group_prefix}{label}{suffix}"),
        !busy,
        entry.selector == active,
    ));
}

/// 在模型里按 id 找那一条，回答「点完要不要留着菜单」（找不到 = 动作项，关）。
///
/// 前端也会照模型里的 `keepsOpen` 做即时反馈，但**关不关窗由后端说了算** ——
/// 免得菜单窗在「前端认为该关、后端认为不该关」时僵在那里。
fn menu_node_keeps_open(nodes: &[MenuNode], id: &str) -> bool {
    fn find(nodes: &[MenuNode], id: &str) -> Option<bool> {
        for node in nodes {
            if node.id.as_deref() == Some(id) {
                return Some(node.keeps_open);
            }
            if let Some(found) = find(&node.children, id) {
                return Some(found);
            }
        }
        None
    }
    find(nodes, id).unwrap_or(false)
}

/// 托盘菜单的全部内容。**纯数据**，与任何工具包无关。
fn build_menu_nodes(state: &UiState) -> Vec<MenuNode> {
    let busy = state.busy.affects_menu();
    let message = &state.status.message;
    let settings = &state.settings.settings;
    let variables = menu_system_rows(&state.status.variables);
    let next_boot = variables.iter().find(|variable| variable.active);
    // 扫描成功后 `status.message` 就是这一行的样子（rEFInd 那一层）—— 去重拿它比对，
    // 与顶部那行改成什么排版无关（见 menu_status_line）。
    let next_boot_scan_line = next_boot.map_or_else(
        || "下次启动：未知".to_string(),
        |variable| format!("下次启动：{}", variable.display_name),
    );
    // 顶部那行：**与主窗口那行大字同一份文字**（级联到 GRUB 条目），并按用户定案
    // 排成两行「下次启动 / Linux › OpenCore (macOS)」。级联规则见 next_boot_cascade。
    let next_boot_label = format!(
        "下次启动\n{}",
        next_boot_cascade(&state.status.variables, &state.grub)
    );

    let mut nodes: Vec<MenuNode> = Vec::new();
    // 第一行就把「下次启动哪个系统」摆出来。原生菜单改不了字号/字重，所以靠**位置**
    // 和标记让它够醒目；点它 = 打开主窗口去改。
    nodes.push(menu_marked_item(
        "next-boot",
        next_boot_label,
        true,
        next_boot.is_some(),
    ));
    // 状态行只在**跟上面那行不一样**的时候才出现（见 menu_status_line）：
    // 扫描成功后 message 本身就是「下次启动：X」，再摆一行就是「下次启动重复了」，
    // 只有错误提示/临时操作结果才需要单独一行。
    if let Some(line) = menu_status_line(message, &next_boot_scan_line) {
        nodes.push(menu_item("status", line, false));
    }
    nodes.push(menu_separator());

    // rEFInd 扫到了系统 = 「展开」平铺（现状不变）；没扫到 = 收进子菜单不展开，
    // 同时 GRUB 区平铺展开顶上 —— 保证总有一块能直接点到。反过来 rEFInd 在时
    // GRUB 收进子菜单（默认展开 rEFInd，用户定案）。
    let refind_alive = variables.iter().any(|variable| variable.exists);
    // —— 分区：系统列表（选中项带标记）——
    if refind_alive {
        for variable in &variables {
            nodes.push(menu_marked_choice_item(
                variable.system.clone(),
                variable.display_name.clone(),
                variable.exists && !busy,
                variable.active,
            ));
        }
    } else {
        let children = variables
            .iter()
            .map(|variable| {
                menu_marked_choice_item(
                    variable.system.clone(),
                    variable.display_name.clone(),
                    variable.exists && !busy,
                    variable.active,
                )
            })
            .collect();
        nodes.push(menu_submenu("refind-systems", "rEFInd 系统列表", children));
    }
    nodes.push(menu_separator());
    // —— 分区：GRUB 启动项（与 rEFInd 系统列表并列的第二块）——
    if refind_alive {
        nodes.push(menu_submenu(
            "grub-entries",
            "GRUB 启动项",
            grub_menu_nodes(&state.grub, busy),
        ));
    } else {
        nodes.extend(grub_menu_nodes(&state.grub, busy));
    }
    nodes.push(menu_separator());
    if state.status.temporary_mount {
        nodes.push(menu_item("unmount-temporary", "卸载临时挂载", !busy));
    }
    nodes.push(menu_item("show", "显示主窗口", true));
    // 远程管理（server-hub）独立窗口的入口：托盘是唯一新增入口，主窗口零改动。
    nodes.push(menu_item("open-hub", "远程管理…", true));

    // —— 分区：应用设置 ——
    nodes.push(menu_separator());
    nodes.push(menu_header("settings-label", "应用设置"));
    nodes.push(menu_choice_item(
        "setting-autostart",
        toggle_label(settings.autostart, "开机启动"),
        true,
    ));
    // 开关项的 id 带「点下去要变成的状态」。原生勾选控件会先把勾翻转再通知我们，
    // 所以分发时必须知道用户想要哪个值；现在是纯文本标记（见 toggle_label），
    // 这个后缀同时也让「重复点同一项」是幂等的（不会来回翻）。
    nodes.push(menu_choice_item(
        toggle_menu_id("setting-start-in-tray", !settings.start_in_tray),
        toggle_label(settings.start_in_tray, "启动时隐藏主界面"),
        true,
    ));
    nodes.push(menu_choice_item(
        toggle_menu_id(
            "setting-hide-from-dock-taskbar",
            !settings.hide_from_dock_taskbar,
        ),
        toggle_label(settings.hide_from_dock_taskbar, "隐藏程序（任务栏/Dock）"),
        true,
    ));
    // 贴边自动隐藏是高频功能开关，托盘给个快捷入口（与 HTTP 服务/远程请求通知同级）。
    // 「屏幕中间」模式下两者互斥，这里整体置灰（勾选态保持设置原值，
    // 切回其他落位方式即可用）。
    nodes.push(menu_choice_item(
        toggle_menu_id("setting-main-auto-hide", !settings.main_auto_hide),
        toggle_label(settings.main_auto_hide, "贴边自动隐藏"),
        normalize_window_position(&settings.main_window_position) != "center",
    ));
    // 窗口大小在设置页能随意调（滑条 / 拖边框），托盘里放一个「重置」入口：
    // 窗口被拖到怪尺寸（或锁定态想恢复标称）时，不用打开主窗口去找
    nodes.push(menu_item("reset-window-size", "重置窗口大小", !busy));
    // 与设置页「重置位置」同源：窗口被拖丢/飞出屏幕时一键回正中
    nodes.push(menu_item("reset-window-position", "重置窗口位置", !busy));
    // 固定显示位置（与设置页「固定显示位置」下拉同源）：三种模式各一个单选项
    let position_now = normalize_window_position(&settings.main_window_position);
    nodes.push(menu_submenu(
        "setting-window-position",
        "固定显示位置",
        [("free", "自由摆放"), ("center", "屏幕中间"), ("last", "上次位置")]
            .into_iter()
            .map(|(mode, label)| {
                menu_choice_item(
                    format!("setting-main-window-position:{mode}"),
                    radio_label(position_now == mode, label),
                    true,
                )
            })
            .collect(),
    ));
    // 固定显示器（与设置页「固定显示器」下拉同源）：auto + 当前所有显示器。
    // 列表来自快照里的 mainMonitorOptions（与主菜单同源），显示器热插拔后
    // 下一次快照刷新菜单就会跟着变。
    let fixed_now = normalize_fixed_monitor(&settings.main_fixed_monitor);
    let mut monitors = vec![menu_choice_item(
        "setting-main-fixed-monitor:auto",
        radio_label(fixed_now == "auto", "自动（不固定）"),
        true,
    )];
    monitors.extend(state.settings.main_monitor_options.iter().map(|name| {
        menu_choice_item(
            format!("setting-main-fixed-monitor:{name}"),
            radio_label(fixed_now == *name, name),
            true,
        )
    }));
    nodes.push(menu_submenu(
        "setting-main-fixed-monitor",
        "固定显示器",
        monitors,
    ));
    nodes.push(menu_submenu(
        "setting-close-action",
        "点击窗口 ❌ 时",
        [(CloseAction::Tray, "最小化到托盘"), (CloseAction::Quit, "退出程序")]
            .into_iter()
            .map(|(action, label)| {
                menu_choice_item(
                    close_action_menu_id(action),
                    radio_label(settings.close_action == action, label),
                    true,
                )
            })
            .collect(),
    ));

    // —— 分区：扫描 ——
    nodes.push(menu_separator());
    nodes.push(menu_item("refresh", "扫描", !busy));

    // —— 分区：电源操作 ——
    // 铺平在主菜单里（不折叠成子菜单：关机/重启是高频动作，多一层展开纯属添堵），
    // 但上下各有一条分隔线 + 一个标题，这样它是一块独立分区，而不是粘在「扫描」下面。
    nodes.push(menu_separator());
    nodes.push(menu_header("power-label", "电源操作"));
    for action in [
        PowerAction::Reboot,
        PowerAction::Shutdown,
        PowerAction::Sleep,
    ] {
        nodes.push(menu_item(power_menu_id(action), action.label(), !busy));
    }

    // —— 分区：远程控制 ——
    // 同样铺平在主菜单里；监听状态是这一区的**第一行**，不展开任何子菜单就能看到在不在听。
    nodes.push(menu_separator());
    nodes.push(menu_header("remote-label", "远程控制"));
    nodes.push(menu_item(
        "remote-status",
        format!("监听状态：{}", menu_status_text(&state.http.state_text, 48)),
        false,
    ));
    nodes.push(menu_choice_item(
        toggle_menu_id("setting-http-enabled", !settings.http_enabled),
        toggle_label(settings.http_enabled, "HTTP 服务"),
        true,
    ));
    nodes.push(menu_choice_item(
        toggle_menu_id("setting-notify-enabled", !settings.notify_enabled),
        toggle_label(settings.notify_enabled, "远程请求通知"),
        true,
    ));
    let delay_now = settings.channels.http.delay_seconds;
    let delay_same = delay_now == settings.channels.cli.delay_seconds;
    nodes.push(menu_submenu(
        "setting-delay",
        delay_submenu_title(settings),
        [0u32, 3, 5, 10, 30]
            .into_iter()
            .map(|seconds| {
                let label = if seconds == 0 {
                    "立即执行（0 秒）".to_string()
                } else {
                    format!("{seconds} 秒")
                };
                menu_choice_item(
                    format!("setting-delay:{seconds}"),
                    radio_label(delay_same && delay_now == seconds, &label),
                    true,
                )
            })
            .collect(),
    ));
    nodes.push(menu_item("show-remote", "远程控制设置…", true));
    // 用系统浏览器打开 Web 控制页（带 token，点开即用）。
    // 没在监听时置灰：没有页面可开，免得点了弹一个必然失败的错误。
    nodes.push(menu_item("open-web", "打开 Web 页面", state.http.listening));

    nodes.push(menu_separator());
    nodes.push(menu_item("quit", "退出程序", true));
    nodes
}

// ==================== classic（各自方式）：三端原生托盘菜单 ====================
//
// 这是「三端自绘」之前的原始方案（6b7ef6c 之前的形态）：托盘图标由 tauri 托管，
// 菜单是**原生菜单**——Windows 现建现弹（锚点窗）、macOS 挂在托盘上由系统弹、
// Linux 挂在托盘上由宿主按 dbusmenu 渲染（libappindicator 那条路）。自绘窗方案
// 作为「全局统一」方式保留，两种形态按 trayMenuStyle 在启动时二选一。

/// 菜单里的「小节标题」：灰色禁用项，纯视觉分区用。
/// 只被手写 build_tray_menu（Win/mac）使用；单测与 Linux 编译下 allow。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn menu_section_header(
    app: &AppHandle,
    id: &str,
    label: &str,
) -> tauri::Result<MenuItem<tauri::Wry>> {
    MenuItem::with_id(app, id, label, false, None::<&str>)
}

/// 托盘菜单（原生形态）——**手写构建**（昨天中午前验证过的原始实现）。
///
/// 只负责构建菜单，不碰托盘图标。setup 的第一帧和后续重建走同一份实现。
/// 注意：不要换回「MenuNode 模型 → native_menu 转换层」——转换层在 GNOME
/// 宿主渲染下子菜单展开为空（GRUB 启动项/固定显示器等），手写版没有这个问题。
/// Linux 已不走 tauri 托盘（classic 也由 linux_sni 承载），本函数只剩 Win/mac。
#[cfg(all(desktop, not(target_os = "linux")))]
fn build_tray_menu(app: &AppHandle, state: &UiState) -> tauri::Result<Menu<tauri::Wry>> {
    let busy = state.busy.affects_menu();
    let message = &state.status.message;
    let settings = &state.settings.settings;
    let variables = menu_system_rows(&state.status.variables);
    let next_boot = variables.iter().find(|variable| variable.active);
    let next_boot_label = next_boot.map_or_else(
        || "下次启动：未知".to_string(),
        |variable| format!("下次启动：{}", variable.display_name),
    );
    let check = check_mark_image();

    let menu = Menu::new(app)?;
    // 第一行就把「下次启动哪个系统」摆出来。原生菜单改不了字号/字重，所以靠**位置**
    // 和绿勾让它够醒目；点它 = 打开主窗口去改。
    let next_boot_item = IconMenuItem::with_id(
        app,
        "next-boot",
        &next_boot_label,
        true,
        next_boot.map(|_| check.clone()),
        None::<&str>,
    )?;
    menu.append(&next_boot_item)?;
    // 状态行只在**跟上面那行不一样**的时候才出现（见 menu_status_line）：
    // 扫描成功后 message 本身就是「下次启动：X」，再摆一行就是「下次启动重复了」，
    // 只有错误提示/临时操作结果才需要单独一行。
    if let Some(line) = menu_status_line(message, &next_boot_label) {
        let status = MenuItem::with_id(app, "status", line, false, None::<&str>)?;
        menu.append(&status)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    // —— 分区：GRUB 启动项菜单项的构造（进顶层菜单 = 平铺展开；进子菜单 = 收起）——
    // **内容来自共享菜单模型**（`grub_menu_nodes`，与主窗 GRUB 区、统一方式同一份函数）：
    // 列表格式随 `grub.listFormat`、当前项置顶、组头带（N 项 · ESP 位置）、变体折叠。
    // 这里只把纯数据节点翻译成原生控件，不再自己算一遍 —— 以前那套是第二份实现，
    // 于是「各自方式」的托盘 GRUB 区跟主窗/统一方式长期对不上。
    // GRUB 节点只有 item / header 两类、不含子菜单，因此不涉及上方注释里
    // 「转换层在 GNOME 宿主下子菜单展开为空」那个坑（那条说的是 sub 节点）。
    fn grub_menu_items(
        app: &AppHandle,
        grub: &GrubView,
        busy: bool,
        check: &Image<'static>,
    ) -> tauri::Result<Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>>> {
        let nodes = grub_menu_nodes(grub, busy);
        // 常开通道留一行痕：原生菜单这一块现在只有一份来源（上面的 nodes），
        // 出问题时能直接跟主窗 GRUB 区对行。只在菜单真的重建时打（指纹拦着）。
        startup_debug(
            app,
            &format!(
                "托盘菜单(classic) GRUB 区 {} 行: {}",
                nodes.len(),
                nodes
                    .iter()
                    .map(|node| node.label.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
        );
        let mut items: Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>> = Vec::new();
        for node in nodes {
            let id = node.id.unwrap_or_default();
            // 组头（`▸ 🐧 组名（N 项 · 位于…）`）——原生菜单里就是一条禁用条目
            if node.kind == "header" {
                items.push(Box::new(menu_section_header(app, &id, &node.label)?));
                continue;
            }
            // 「当前项」在原生菜单里用绿勾位图表达（与系统列表同一套做法）；
            // 条目的 ◉/○ 是给文字型宿主（dbusmenu / 自绘窗）的，这里不复用。
            let icon = match node.checked {
                Some(true) => Some(check.clone()),
                _ => None,
            };
            items.push(Box::new(IconMenuItem::with_id(
                app,
                id,
                node.label,
                node.enabled,
                icon,
                None::<&str>,
            )?));
        }
        Ok(items)
    }

    // rEFInd 扫到了系统 = 「展开」平铺（现状不变）；没扫到 = 收进子菜单不展开，
    // 同时 GRUB 区平铺展开顶上 —— 保证总有一块能直接点到。反过来 rEFInd 在时
    // GRUB 收进子菜单（默认展开 rEFInd，用户定案）。
    let refind_alive = variables.iter().any(|variable| variable.exists);
    // —— 分区：系统列表（选中项带绿勾，名字靠位图列天然左对齐）——
    if refind_alive {
        for variable in &variables {
            // 选中项带绿勾、没选中的不带：勾占的是菜单**独立的位图列**，不挤占文字，
            // 三个系统的名字因此天然左对齐（以前用「● 」前缀，选中的那个会被顶偏一格）。
            let item = IconMenuItem::with_id(
                app,
                variable.system.clone(),
                variable.display_name.clone(),
                variable.exists && !busy,
                if variable.active {
                    Some(check.clone())
                } else {
                    None
                },
                None::<&str>,
            )?;
            menu.append(&item)?;
        }
    } else {
        let systems_menu = Submenu::with_id(app, "refind-systems", "rEFInd 系统列表", true)?;
        for variable in &variables {
            let item = IconMenuItem::with_id(
                app,
                variable.system.clone(),
                variable.display_name.clone(),
                variable.exists && !busy,
                if variable.active {
                    Some(check.clone())
                } else {
                    None
                },
                None::<&str>,
            )?;
            systems_menu.append(&item)?;
        }
        menu.append(&systems_menu)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    // —— 分区：GRUB 启动项（与 rEFInd 系统列表并列的第二块）——
    if refind_alive {
        let grub_menu = Submenu::with_id(app, "grub-entries", "GRUB 启动项", true)?;
        for item in grub_menu_items(app, &state.grub, busy, &check)? {
            grub_menu.append(item.as_ref())?;
        }
        menu.append(&grub_menu)?;
    } else {
        for item in grub_menu_items(app, &state.grub, busy, &check)? {
            menu.append(item.as_ref())?;
        }
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
    menu.append(&MenuItem::with_id(
        app,
        "show",
        "显示主窗口",
        true,
        None::<&str>,
    )?)?;

    // —— 分区：应用设置 ——
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&menu_section_header(app, "settings-label", "应用设置")?)?;
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
    // 贴边自动隐藏是高频功能开关，托盘给个快捷入口（与 HTTP 服务/远程请求通知同级）
    menu.append(&CheckMenuItem::with_id(
        app,
        toggle_menu_id("setting-main-auto-hide", !settings.main_auto_hide),
        "贴边自动隐藏",
        true,
        settings.main_auto_hide,
        None::<&str>,
    )?)?;
    // 窗口大小在设置页能随意调（滑条 / 拖边框），托盘里放一个「重置」入口：
    // 窗口被拖到怪尺寸（或锁定态想恢复标称）时，不用打开主窗口去找
    menu.append(&MenuItem::with_id(
        app,
        "reset-window-size",
        "重置窗口大小",
        !busy,
        None::<&str>,
    )?)?;
    // 与设置页「重置位置」同源：窗口被拖丢/飞出屏幕时一键回正中
    menu.append(&MenuItem::with_id(
        app,
        "reset-window-position",
        "重置窗口位置",
        !busy,
        None::<&str>,
    )?)?;
    // 固定显示位置（与设置页「固定显示位置」下拉同源）：三种模式各一个勾选项
    let position_menu = Submenu::with_id(app, "setting-window-position", "固定显示位置", true)?;
    let position_now = normalize_window_position(&settings.main_window_position);
    for (mode, label) in [
        ("free", "自由摆放"),
        ("center", "屏幕中间"),
        ("last", "上次位置"),
    ] {
        position_menu.append(&CheckMenuItem::with_id(
            app,
            format!("setting-main-window-position:{mode}"),
            label,
            true,
            position_now == mode,
            None::<&str>,
        )?)?;
    }
    menu.append(&position_menu)?;
    // 固定显示器（与设置页「固定显示器」下拉同源）：auto + 当前所有显示器。
    // 列表来自快照里的 mainMonitorOptions（与主菜单同源），显示器热插拔后
    // 下一次快照刷新菜单就会跟着变。
    let monitor_menu = Submenu::with_id(app, "setting-main-fixed-monitor", "固定显示器", true)?;
    let fixed_now = normalize_fixed_monitor(&settings.main_fixed_monitor);
    monitor_menu.append(&CheckMenuItem::with_id(
        app,
        "setting-main-fixed-monitor:auto",
        "自动（不固定）",
        true,
        fixed_now == "auto",
        None::<&str>,
    )?)?;
    for name in &state.settings.main_monitor_options {
        monitor_menu.append(&CheckMenuItem::with_id(
            app,
            format!("setting-main-fixed-monitor:{name}"),
            name.clone(),
            true,
            fixed_now == *name,
            None::<&str>,
        )?)?;
    }
    menu.append(&monitor_menu)?;
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

    // —— 分区：扫描 ——
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "refresh",
        "扫描",
        !busy,
        None::<&str>,
    )?)?;

    // —— 分区：电源操作 ——
    // 铺平在主菜单里（不折叠成子菜单：关机/重启是高频动作，多一层展开纯属添堵），
    // 但上下各有一条分隔线 + 一个标题，这样它是一块独立分区，而不是粘在「扫描」下面。
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&menu_section_header(app, "power-label", "电源操作")?)?;
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
        menu.append(&item)?;
    }

    // —— 分区：远程控制 ——
    // 同样铺平在主菜单里；监听状态是这一区的**第一行**，不展开任何子菜单就能看到在不在听。
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&menu_section_header(app, "remote-label", "远程控制")?)?;
    let listening = MenuItem::with_id(
        app,
        "remote-status",
        format!("监听状态：{}", menu_status_text(&state.http.state_text, 48)),
        false,
        None::<&str>,
    )?;
    menu.append(&listening)?;
    menu.append(&CheckMenuItem::with_id(
        app,
        toggle_menu_id("setting-http-enabled", !settings.http_enabled),
        "HTTP 服务",
        true,
        settings.http_enabled,
        None::<&str>,
    )?)?;
    menu.append(&CheckMenuItem::with_id(
        app,
        toggle_menu_id("setting-notify-enabled", !settings.notify_enabled),
        "远程请求通知",
        true,
        settings.notify_enabled,
        None::<&str>,
    )?)?;
    let delay_menu = Submenu::with_id(app, "setting-delay", &delay_submenu_title(settings), true)?;
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
    menu.append(&delay_menu)?;
    menu.append(&MenuItem::with_id(
        app,
        "show-remote",
        "远程控制设置…",
        true,
        None::<&str>,
    )?)?;
    // 用系统浏览器打开 Web 控制页（带 token，点开即用）。
    // 没在监听时置灰：没有页面可开，免得点了弹一个必然失败的错误。
    menu.append(&MenuItem::with_id(
        app,
        "open-web",
        "打开 Web 页面",
        state.http.listening,
        None::<&str>,
    )?)?;

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "退出程序", true, None::<&str>)?)?;
    Ok(menu)
}

/// 托盘右键（classic 形态）。
///
/// Windows：菜单由我们自己弹（锚点在工作区下沿，见 `popup_tray_menu`），
/// 所以托盘上**没有**挂菜单，右键只有这个事件可走。
/// 其他平台：菜单挂在托盘上，什么时候弹、弹在哪由面板/系统决定，这里无事可做。
/// Linux 已不建 tauri 托盘（classic 也走 linux_sni），本函数只剩 Win/mac 用。
#[cfg(not(target_os = "linux"))]
fn on_tray_right_click(app: &AppHandle, cursor: tauri::PhysicalPosition<f64>) {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, cursor);
    }
    #[cfg(target_os = "windows")]
    popup_tray_menu(app, cursor);
}

/// Windows 上托盘右键菜单**由我们自己弹**，锚点落在工作区下沿（任务栏上沿）。
///
/// 为什么不能交给原生实现：tray-icon 的锚点是**光标**，而托盘图标的光标落在任务栏里；
/// Windows 的 TrackPopupMenu 只保证菜单不越出**屏幕**，不保证不越出**工作区**。
/// 本机实测（2194×1234、任务栏 40px）：锚在光标上菜单底部 = 1214，把任务栏压住 20px；
/// 锚在工作区底边（1194）则压住 0px。用 `-flags TPM_BOTTOMALIGN|TPM_LEFTALIGN` 也一样，
/// 所以只能换锚点，不能换标志位。
///
/// 锚点怎么给：`Menu::popup_at` 会把坐标当**锚点窗口的客户区坐标**再 `ClientToScreen` 一次，
/// 而锚点窗口是无边框的、创建时就停在 (0,0) —— 客户区原点就是屏幕原点，
/// 于是「屏幕坐标 - 窗口原点」正好是要传的值（窗口原点万一不是 (0,0) 也照样算对）。
#[cfg(target_os = "windows")]
fn popup_tray_menu(app: &AppHandle, cursor: tauri::PhysicalPosition<f64>) {
    // `popup_at` 定义在公开 trait 上，只有 Windows 这条路用到它
    use tauri::menu::ContextMenu;

    let Some(anchor) = app.get_webview_window(MENU_ANCHOR_WINDOW_LABEL) else {
        trace(app, "tray", "锚点窗口不在，右键菜单没有弹出");
        return;
    };
    let monitor = app
        .monitor_from_point(cursor.x, cursor.y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        trace(app, "tray", "拿不到显示器信息，右键菜单没有弹出");
        return;
    };
    let work = monitor.work_area();
    let target = tauri::PhysicalPosition::new(
        cursor.x.round() as i32,
        work.position.y + work.size.height as i32,
    );
    let origin = anchor
        .outer_position()
        .unwrap_or(tauri::PhysicalPosition::new(0, 0));
    let position = tauri::PhysicalPosition::new(target.x - origin.x, target.y - origin.y);

    // 菜单按**此刻**的快照现建：这样永远是最新状态，也省掉了「状态变了要记得重建」的坑
    let menu = match build_tray_menu(app, &ui_state(app)) {
        Ok(menu) => menu,
        Err(error) => {
            trace(app, "tray", &format!("右键菜单构建失败：{error}"));
            return;
        }
    };
    if let Err(error) = menu.popup_at(anchor.as_ref().window(), position) {
        trace(app, "tray", &format!("右键菜单弹出失败：{error}"));
    }
}

// ==================== 菜单内容指纹（classic 原生菜单的「变了才重建」） ====================

/// 托盘菜单内容摘要：同样内容 → 同样签名。
///
/// 用它做「内容没变就不重建菜单」的指纹（原生菜单重建 + 重挂会闪断）；
/// unified 方式下同一个指纹用来决定要不要给 dbusmenu 发 LayoutUpdated。
fn menu_signature(
    message: &str,
    temporary_mount: bool,
    settings: &Settings,
    variables: &[VariableInfo],
    error: Option<&str>,
    busy: &BusyState,
    http_state: &str,
) -> String {
    // 系统行上每个可见字段都进指纹：系统名 / 显示名 / 选中 / 存在。
    // 少一个就会出现「数据变了但菜单没重建」的假状态。
    let variables = variables
        .iter()
        .map(|variable| {
            format!(
                "{}:{}:{}{}",
                variable.system,
                variable.display_name,
                if variable.active { "+" } else { "-" },
                if variable.exists { "o" } else { "x" }
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "msg={message}|tmp={}|autostart={}|tray={}|dock={}|close={:?}|http={}|notify={}|autohide={}|delay={},{}|vars={variables}|err={}|busy={}|httpState={http_state}",
        temporary_mount as u8,
        settings.autostart as u8,
        settings.start_in_tray as u8,
        settings.hide_from_dock_taskbar as u8,
        settings.close_action,
        // 远程控制子菜单里的勾选与「监听状态」行都从设置/运行态来，
        // 不进指纹的话主窗口一改，托盘那边就停在旧样子
        settings.http_enabled as u8,
        settings.notify_enabled as u8,
        // 贴边自动隐藏的托盘勾选态也要进指纹
        settings.main_auto_hide as u8,
        settings.channels.http.delay_seconds,
        settings.channels.cli.delay_seconds,
        error.unwrap_or(""),
        // 动作进行中菜单项会变灰，所以 busy 必须进指纹，否则置灰不会生效
        busy.menu_describe()
    )
}

fn menu_signature_for(state: &UiState) -> String {
    let base = menu_signature(
        &state.status.message,
        state.status.temporary_mount,
        &state.settings.settings,
        &menu_system_rows(&state.status.variables),
        state.status.error.as_deref(),
        &state.busy,
        &state.http.state_text,
    );
    // 固定显示位置的模式进指纹（托盘子菜单三个勾选项跟着变）
    let position = normalize_window_position(&state.settings.settings.main_window_position);
    // 固定显示器及其选项列表也进指纹：勾选态跟着变，显示器热插拔后
    // 选项集变了也要重建菜单
    let fixed_monitor = normalize_fixed_monitor(&state.settings.settings.main_fixed_monitor);
    let monitor_options = state.settings.main_monitor_options.join(",");
    // GRUB 区进指纹：**直接用托盘菜单真正会显示的那份 GRUB 节点**（grub_menu_nodes，
    // 与主窗 GRUB 区、统一方式同一份渲染逻辑）—— 列表格式、当前项置顶、组头
    // （N 项 · ESP 位置）、变体折叠、哪一条是当前项，全在里面。
    // 以前是自己再数一遍「条目名 + 分组名 + 列表格式」，等于把同一份内容算了两遍：
    // 改渲染规则时极易漏掉指纹，症状就是「显示的名字变了但托盘没重建」的假状态。
    let grub_menu = serde_json::to_string(&grub_menu_nodes(
        &state.grub,
        state.busy.affects_menu(),
    ))
    .expect("GRUB 菜单节点一定能序列化");
    let next_boot = next_boot_cascade(&state.status.variables, &state.grub);
    format!(
        "{base}|wpos={position}|fmon={fixed_monitor}|fmons={monitor_options}|grubmenu={grub_menu}|nb={next_boot}"
    )
}

/// 记住当前菜单内容，让后续相同内容的刷新直接跳过重建。
/// Linux 已不建 tauri 托盘（classic 也走 linux_sni），预热只剩 Win/mac 用。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn remember_menu_signature(app: &AppHandle, signature: String) {
    *app.state::<Mutex<Option<String>>>()
        .lock()
        .expect("menu signature mutex poisoned") = Some(signature);
}

/// 作废指纹记忆：下一次 publish 一定重建菜单（哪怕内容算出来没变）。
///
/// 托盘菜单的实时性全靠「重建 + 重挂」：勾选标记、置灰、状态行都是重建那一刻才刷新的。
/// 而「用户点了菜单」这件事本身就是一个「显示可能与真实状态不符」的信号 ——
/// 比如单选项点的是已选中的那个（设置没变、指纹没变），但宿主那边的勾选态很可能
/// 已经被工具包翻过了。所以每次菜单点击都廉价地作废一次指纹，让界面马上回到真实状态。
fn invalidate_menu_signature(app: &AppHandle) {
    *app.state::<Mutex<Option<String>>>()
        .lock()
        .expect("menu signature mutex poisoned") = None;
}

/// 把最新状态推给托盘（classic 原生菜单 / unified 自绘窗+dbusmenu 两种形态）。
fn update_tray_menu(app: &AppHandle, state: &UiState, trigger: &str) -> tauri::Result<()> {
    let signature = menu_signature_for(state);
    if tray_menu_style_is_unified(app) {
        // unified：自绘窗开着就推一份纯数据（Linux 的菜单面就是这扇窗），状态一变
        // 就要推新内容重画；同时把 dbusmenu 置脏 —— SNI 线程广播 LayoutUpdated，
        // Cinnamon/KDE 的宿主菜单重新拉取全量内容（GNOME 空根置脏无感）。
        if let Some(window) = app.get_webview_window(TRAY_MENU_WINDOW_LABEL) {
            if window.is_visible().unwrap_or(false) {
                trace(app, "tray", &format!("菜单窗开着，推送最新内容 trigger={trigger}"));
                emit_tray_menu_nodes(app, &window, state);
            }
        }
        #[cfg(target_os = "linux")]
        linux_sni::menu_layout_changed(menu_signature_changed(app, &signature));
        // Linux 的托盘提示是 SNI 的静态 ToolTip（状态在菜单第一行里），不用跟着刷；
        // Win/mac 的 tooltip 承担同样的「一眼看状态」职责，跟着状态走。
        #[cfg(not(target_os = "linux"))]
        update_tray_tooltip(app, state)?;
        return Ok(());
    }

    // classic：原生菜单。**内容没变就不重建**（指纹相同直接返回）——
    // 重建 + 重挂正是「扫描期间菜单闪断 / 要点两次」的诱因。
    if !menu_signature_changed(app, &signature) {
        trace(
            app,
            "tray",
            &format!("菜单内容未变，跳过重建 trigger={trigger} sig={signature}"),
        );
        return Ok(());
    }
    trace(
        app,
        "tray",
        &format!("菜单内容已变 trigger={trigger} sig={signature}"),
    );
    #[cfg(target_os = "linux")]
    {
        // Linux classic：菜单由 linux_sni 的 dbusmenu 承载（setup 不建 tauri 托盘，
        // tray_by_id 恒 None —— 不在这里早退的话，内容变化永远到不了置脏广播，
        // dbusmenu 会停在首次拉取的快照）。走到这里说明指纹已判定「变了」，
        // 直接置脏 —— SNI 线程广播 LayoutUpdated，宿主重拉全量新内容。
        linux_sni::menu_layout_changed(true);
        return Ok(());
    }
    // Linux classic 在上方 cfg 块已 return；以下只剩 Win/mac 的 tauri 托盘路径。
    #[cfg(not(target_os = "linux"))]
    {
        let Some(tray) = app.tray_by_id("main") else {
            trace(app, "tray", "未找到托盘图标 main，菜单没有挂上");
            return Ok(());
        };
        let tooltip = state
            .status
            .error
            .clone()
            .unwrap_or_else(|| format!("AnyBoot：{}", state.status.message));
        // Windows 上菜单**不挂到托盘**：右键那一刻现建现弹（见 popup_tray_menu），
        // 这里的指纹就只剩「别白刷一遍 tooltip」这一个用处。
        #[cfg(not(target_os = "windows"))]
        {
            tray.set_menu(Some(build_tray_menu(app, state)?))?;
            trace(app, "tray", "菜单已重建并挂上托盘");
        }
        #[cfg(target_os = "windows")]
        trace(app, "tray", "菜单不挂托盘（Windows 右键时现建现弹）");
        tray.set_tooltip(Some(tooltip))?;
        Ok(())
    }
}

/// 指纹比对 + 记忆。返回「这次和上次比内容变了吗」。
///
/// 统一给两种形态用：classic 变了重建原生菜单，unified 变了给 dbusmenu 发更新信号。
fn menu_signature_changed(app: &AppHandle, signature: &str) -> bool {
    let cache = app.state::<Mutex<Option<String>>>();
    let mut cache = cache.lock().expect("menu signature mutex poisoned");
    if cache.as_deref() == Some(signature) {
        return false;
    }
    *cache = Some(signature.to_string());
    true
}

/// unified 方式下 Win/mac 的 tooltip 跟状态走（Linux 是 SNI 静态 ToolTip，不在此列）。
#[cfg(not(target_os = "linux"))]
fn update_tray_tooltip(app: &AppHandle, state: &UiState) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id("main") else {
        trace(app, "tray", "未找到托盘图标 main，tooltip 没有刷新");
        return Ok(());
    };
    let tooltip = state
        .status
        .error
        .clone()
        .unwrap_or_else(|| format!("AnyBoot：{}", state.status.message));
    tray.set_tooltip(Some(tooltip))?;
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
    /// 贴边自动隐藏总开关（关掉时立即展开）
    SetMainAutoHide(bool),
    /// 远程执行延迟（同时作用于两条渠道）
    SetDelayAll(u32),
    /// 打开主窗口并定位到「远程控制」区
    ShowRemote,
    /// 用系统默认浏览器打开 Web 控制页（带 token）
    OpenWeb,
    /// 打开/唤起远程管理（server-hub）独立窗口
    OpenHub,
    SetAutostart,
    SetStartInTray(bool),
    SetHideFromDockTaskbar(bool),
    SetCloseAction(CloseAction),
    ResetWindowSize,
    /// 主窗口摆回屏幕正中（与设置页「重置位置」同源）
    ResetWindowPosition,
    /// 固定显示位置模式（free / center / last，与设置页下拉同源）
    SetMainWindowPosition(String),
    /// 固定显示器（auto / 显示器名，与设置页下拉同源）
    SetMainFixedMonitor(String),
    /// 切换 GRUB 启动项（写 grubenv 的 selector，与主窗 GRUB 区同源）
    SwitchGrub(String),
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
        if let Some(rest) = id.strip_prefix("setting-main-auto-hide:") {
            return parse_menu_bool(rest).map(Self::SetMainAutoHide);
        }
        if let Some(rest) = id.strip_prefix("setting-delay:") {
            return rest.trim().parse::<u32>().ok().map(Self::SetDelayAll);
        }
        if let Some(rest) = id.strip_prefix("setting-main-window-position:") {
            let mode = rest.trim().to_string();
            // 归一兜底：脏值会被 normalize 打回 free，不会落进设置
            return Some(Self::SetMainWindowPosition(mode));
        }
        if let Some(rest) = id.strip_prefix("setting-main-fixed-monitor:") {
            return Some(Self::SetMainFixedMonitor(rest.to_string()));
        }
        if let Some(rest) = id.strip_prefix("grub-switch:") {
            return Some(Self::SwitchGrub(rest.to_string()));
        }
        match id {
            "show" => Some(Self::Show),
            // 顶部那行「下次启动：X」：点它就是把主窗口叫出来改
            "next-boot" => Some(Self::Show),
            "refresh" => Some(Self::Refresh),
            "unmount-temporary" => Some(Self::UnmountTemporary),
            // 旧版 id：只有一个「重启系统」项时的写法，保留兼容
            "restart" => Some(Self::Power(PowerAction::Reboot)),
            "setting-autostart" => Some(Self::SetAutostart),
            "show-remote" => Some(Self::ShowRemote),
            "open-web" => Some(Self::OpenWeb),
            "open-hub" => Some(Self::OpenHub),
            "reset-window-size" => Some(Self::ResetWindowSize),
            "reset-window-position" => Some(Self::ResetWindowPosition),
            "quit" => Some(Self::Quit),
            "linux" | "windows" | "macos" => Some(Self::Switch(id.to_string())),
            _ => None,
        }
    }
}

/// 托盘菜单事件入口：原始 id → 解析 → 分发，每一步都写 trace（现场取证）。
///
/// 另外**无条件**往 startup-debug.log 记一行「收到某个菜单 id」：trace 默认是关的，
/// 而「点了菜单没反应」这类报障只有两个可能 —— 事件根本没到应用，或到了但被
/// 某个闸吞了。这一行让两种情形在用户机器上一眼可辨（托盘点击是低频事件，
/// 一行日志的代价可以忽略）。
fn handle_menu_event(app: &AppHandle, id: &str) {
    startup_debug(app, &format!("菜单点击 id={id}"));
    trace(app, "menu-action", &format!("id={id}"));
    let Some(action) = MenuAction::from_id(id) else {
        trace(app, "menu-action", &format!("id={id} 未识别，已忽略"));
        return;
    };
    trace(app, "menu-action", &format!("id={id} → {action:?}"));
    // 用户交互过菜单：先作废指纹（见 invalidate_menu_signature），
    // 让这次动作引发的 publish 一定刷新菜单（classic 重建原生菜单 / unified 发
    // LayoutUpdated），把勾选标记刷成真实状态。
    invalidate_menu_signature(app);
    // 窗口操作留在事件线程上执行；其余动作放工作线程（可能耗时或要授权）。
    if action == MenuAction::Show {
        show_main_window(app, ShowOrigin::Recall);
        return;
    }
    spawn_menu_action(app, action);
}

fn spawn_menu_action(app: &AppHandle, action: MenuAction) {
    let app = app.clone();
    std::thread::spawn(move || match action {
        MenuAction::Show => show_main_window(&app, ShowOrigin::Recall),
        MenuAction::Refresh => {
            refresh_and_update(&app);
        }
        MenuAction::UnmountTemporary => {
            unmount_temporary(app);
        }
        MenuAction::Switch(system) => {
            switch_system_locally(&app, &system);
        }
        MenuAction::Power(action) => spawn_power(&app, action, true),
        MenuAction::SetHttpEnabled(enabled) => set_http_enabled_setting(&app, enabled),
        MenuAction::SetNotifyEnabled(enabled) => set_notify_enabled_setting(&app, enabled),
        MenuAction::SetMainAutoHide(enabled) => match set_main_auto_hide(app.clone(), enabled) {
            Ok(_) => record_action(
                &app,
                "贴边自动隐藏",
                true,
                if enabled {
                    "已开启：鼠标离窗后贴边缩成白条"
                } else {
                    "已关闭：窗口立即展开"
                },
            ),
            Err(error) => record_action(&app, "贴边自动隐藏", false, &error),
        },
        MenuAction::SetDelayAll(seconds) => set_delay_all_setting(&app, seconds),
        MenuAction::ShowRemote => {
            show_main_window(&app, ShowOrigin::Recall);
            if let Err(error) = app.emit("focus-remote", ()) {
                trace(
                    &app,
                    "tray",
                    &format!("通知主窗口定位远程控制失败: {error}"),
                );
            }
        }
        MenuAction::OpenWeb => match open_web_page_now(&app) {
            Ok(url) => record_action(&app, "打开 Web 页面", true, &format!("已打开 {url}")),
            Err(error) => record_action(&app, "打开 Web 页面", false, &error),
        },
        MenuAction::OpenHub => match open_hub_window(&app) {
            Ok(_) => record_action(&app, "打开远程管理", true, "远程管理窗口已打开"),
            Err(error) => record_action(&app, "打开远程管理", false, &error),
        },
        MenuAction::SetAutostart => toggle_autostart_setting(&app),
        MenuAction::SetStartInTray(enabled) => {
            update_tray_setting(&app, TraySetting::StartInTray, enabled)
        }
        MenuAction::SetHideFromDockTaskbar(enabled) => {
            update_tray_setting(&app, TraySetting::HideFromDockTaskbar, enabled)
        }
        MenuAction::SetCloseAction(close_action) => update_close_action_setting(&app, close_action),
        MenuAction::ResetWindowSize => match reset_window_size_inner(&app) {
            Ok(_) => record_action(&app, "重置窗口大小", true, "窗口大小已重置为标称尺寸"),
            Err(error) => record_action(&app, "重置窗口大小", false, &error),
        },
        MenuAction::ResetWindowPosition => {
            // 复用设置页「重置位置」的同一实现：摆回屏幕正中（last 模式顺带记为新落点）
            match reset_main_window_position(app.clone()) {
                Ok(_) => record_action(&app, "重置窗口位置", true, "窗口已摆回屏幕正中"),
                Err(error) => record_action(&app, "重置窗口位置", false, &error),
            }
        }
        MenuAction::SetMainWindowPosition(mode) => {
            // 复用设置页同一条链路：落盘 + 立即按新模式落位
            match set_main_window_position(app.clone(), mode) {
                Ok(_) => record_action(&app, "固定显示位置", true, "显示位置已更新"),
                Err(error) => record_action(&app, "固定显示位置", false, &error),
            }
        }
        MenuAction::SetMainFixedMonitor(name) => {
            // 复用设置页同一条链路：落盘 + 立即把主窗约束到所选显示器
            match set_main_fixed_monitor(app.clone(), name.clone()) {
                Ok(_) => record_action(&app, "固定显示器", true, &format!("固定显示器已设为 {name}")),
                Err(error) => record_action(&app, "固定显示器", false, &error),
            }
        }
        MenuAction::SwitchGrub(selector) => {
            // 与主窗 GRUB 区同一条链路：写 grubenv → 重扫 → 广播
            switch_grub_and_update(&app, &selector);
        }
        MenuAction::Quit => app.exit(0),
    });
}

/// 本进程持有的单实例锁。拿到之后一直拿着（不主动放）：`File::try_lock` 用的是内核的
/// 文件锁（Unix flock / Windows LockFileEx），进程一结束 —— 哪怕是崩溃、被 kill ——
/// 内核就把锁放了。不会出现 pid 文件方案那种「进程早没了、文件还在，于是谁都起不来」
/// 的假死状态，也不需要「探测旧 pid 是否还活着」那套。
static INSTANCE_LOCK: OnceLock<fs::File> = OnceLock::new();

/// 抢单实例锁的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
enum InstanceLockError {
    /// 已经有实例在跑（锁被别的进程拿着）。
    AlreadyRunning,
    /// 锁本身用不了（配置目录建不出来 / 文件打不开 / 锁定出错）。
    Io(String),
}

/// 抢锁的核心（只吃路径，单测能直接验）：`Ok(file)` = 本进程就是唯一实例。
fn try_lock_instance_file(path: &Path) -> Result<fs::File, InstanceLockError> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|error| InstanceLockError::Io(format!("无法打开 {}: {error}", path.display())))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err(InstanceLockError::AlreadyRunning),
        Err(error) => Err(InstanceLockError::Io(format!(
            "无法锁定 {}: {error}",
            path.display()
        ))),
    }
}

/// 试着成为「唯一实例」。`Ok(())` = 本进程拿到了锁。
fn acquire_instance_lock(app: &AppHandle) -> Result<(), InstanceLockError> {
    let directory = config_dir(app).map_err(InstanceLockError::Io)?;
    fs::create_dir_all(&directory).map_err(|error| {
        InstanceLockError::Io(format!("无法创建配置目录 {}: {error}", directory.display()))
    })?;
    let file = try_lock_instance_file(&directory.join(INSTANCE_LOCK_FILE))?;
    let _ = INSTANCE_LOCK.set(file);
    Ok(())
}

/// 放锁。**只有「重启程序」用得上**：tauri 的 `restart()` 是「先拉起新进程、再退出本进程」，
/// 不放锁的话新进程第一眼就看见锁还在，会把自己当第二个实例提示「已经启动」然后退出 ——
/// 用户看到的就是「点重启，程序没了」。
fn release_instance_lock() {
    if let Some(file) = INSTANCE_LOCK.get() {
        let _ = file.unlock();
    }
}

/// 告诉用户「已经有一个 AnyBoot 在跑了」。**两类形态都会让本进程随后退出**：
/// macOS 这条同步阻塞到用户关掉提示之后自己 `exit`，不返回；其余两端把退出交给
/// 异步对话框的回调（所以那里函数会立刻返回，进程过一会儿才走）。
///
/// **macOS 为什么不用 tauri 的对话框插件**：没有 parent 时 rfd 走
/// `CFUserNotificationDisplayAlert`。实测（macOS 15）这条 API **只登记、不显示** ——
/// UserNotificationCenter 里多出一个 4×3、alpha=0 的空窗口，调用方一直阻塞
/// （默认 30 秒后才返回），用户屏幕上什么都没有。改用 osascript 的
/// `display dialog`：系统脚本引擎自己弹 NSAlert（实测 378×120、alpha=1，可见），
/// 与本应用已经用着的电源动作同一条「osascript 子进程」路子，不需要任何额外权限、
/// 也不碰 TCC。另外两端 rfd 的后端是 GTK / MessageBox，没这个毛病，继续用插件。
#[cfg(target_os = "macos")]
fn announce_already_running(app: &AppHandle) {
    // `giving up after` 是兜底：用户不理它的话 60 秒后自己关掉、进程跟着退，
    // 不至于留下一个「看不见又退不掉」的进程。
    const SCRIPT: &str = "display dialog \"AnyBoot 已经启动，只能运行一个实例。\" \
                          with title \"AnyBoot\" buttons {\"好\"} default button 1 \
                          with icon note giving up after 60";
    match Command::new("osascript").arg("-e").arg(SCRIPT).status() {
        Ok(status) => startup_debug(
            app,
            &format!("已提示「已经启动」（osascript {status}）→ 退出"),
        ),
        Err(error) => startup_debug(app, &format!("提示「已经启动」失败（照常退出）: {error}")),
    }
    std::process::exit(0);
}

#[cfg(not(target_os = "macos"))]
fn announce_already_running(app: &AppHandle) {
    app.dialog()
        .message("AnyBoot 已经启动，只能运行一个实例。")
        .title("AnyBoot")
        .kind(MessageDialogKind::Info)
        .show(|_| std::process::exit(0));
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
        let _ = command("launchctl")
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
    let mut view = SettingsView::new(settings, state);
    // 显示器名列表：设置页「固定显示器」下拉的选项（托盘子菜单同源）
    view.main_monitor_options = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| monitor.name().cloned().unwrap_or_default())
        .filter(|name| !name.is_empty())
        .collect();
    view
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

/// 这一次「显示主窗口」是谁触发的。裸 bool 在 6 个调用点上太容易传反，
/// 用枚举把意图直接写在调用处。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShowOrigin {
    /// 启动时的首次显示：不挂呼出保护，贴边隐藏在窗口可见后第一拍就可触发。
    Startup,
    /// 呼出（托盘点击 / 托盘菜单 / 远程 / 动作失败回显）：挂 800ms 宽限 +
    /// 「等待进入」标志 —— 理由见 `undocked_counts_ticks`。
    Recall,
}

/// 显示主窗口。`origin` 见 `ShowOrigin`：启动首次显示与呼出的贴边语义完全不同，
/// 不能共用一条路径。
fn show_main_window(app: &AppHandle, origin: ShowOrigin) {
    let recalled = origin == ShowOrigin::Recall;
    // 呼出保护的第一件事：置「等待进入」标志，必须在 undock 之前 —— undock
    // 在托盘线程恢复窗口的这几百毫秒里，watcher 的贴边分支还会读到
    // docked=true，白条保障发现主窗「偏离停靠位」就把窗口拉回 6px 条的位置，
    // 跟展开互相拉扯 = 两个窗口互相闪烁。标志先立起来，watcher 全程放手。
    if recalled {
        if let Ok(mut guard) = app.state::<Mutex<AppState>>().lock() {
            guard.auto_hide_pending_enter = true;
        }
    }
    #[cfg(target_os = "macos")]
    if !stored_settings(app).hide_from_dock_taskbar {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    // 呼出主界面 = 用户要操作窗口。若此刻处于贴边停靠态（窗口被缩成一条白条），
    // 先解除贴边把完整尺寸恢复回来，再走 show 流程；否则 apply_main_window_position
    // 会按「一条」形状落位，窗口展不开。
    {
        let docked = match app.state::<Mutex<AppState>>().lock() {
            Ok(state) => state.auto_hide_docked,
            Err(_) => false,
        };
        if docked {
            undock_main_window(app);
        }
    }
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        // 顺序不能反：先 show 再 unminimize。skipTaskbar 的窗口最小化后
        // （Windows 把它挪到 −32000），反过来先 unminimize 会被吞掉，
        // 窗口"可见"却仍停在屏幕外 —— 用户看到的就是只剩 GRUB 独立窗口。
        let was_minimized = window.is_minimized().unwrap_or(false);
        let _ = window.show();
        let _ = window.unminimize();
        // 呼出即置顶（受「窗口始终置顶」设置控制）：Linux 下 WM 的
        // focus-stealing-prevention 会让 set_focus 无效，于是窗口被别的窗口
        // 盖住（用户看到的就是「点了托盘但窗口没跳出来」）。设置关掉时窗口
        // 按普通层级呼出，不再强制压到最上层。
        if stored_settings(app).main_always_on_top {
            let _ = window.set_always_on_top(true);
        }
        let _ = window.set_focus();
        // 固定显示位置（free 不动；center 居中；last 回到记忆点）：窗口没被
        // 最小化过（只是藏着）时立刻落位，避免恢复完成后再跳一下；被最小化过
        // 则交给下面的自愈线程在「窗口真正恢复后」再落位，否则 Windows 上
        // unminimize 的异步恢复会把我们的 set_position 覆盖掉（用户看到的就是
        // 「选了居中重开却还是上次的位置」）。
        if !was_minimized {
            apply_main_window_position(app);
        }
        // 自愈 + 落位：show/unminimize 都是异步派发。先等窗口真正脱离最小化
        // （位置离开 −32000），这期间 OS 的恢复会先把窗口摆回「上次位置」，
        // 必须等它稳定之后再 set_position，否则会被覆盖。窗口恢复后，按固定
        // 位置模式落位并兜底夹回工作区 —— 保证窗口绝不会盖住任务栏（mac/
        // linux/win 共用 work_area，落点一定在任务栏上方）。这层兜底针对
        // skipTaskbar + transparent 窗口在 Windows 上恢复路径的不可靠行为。
        {
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(40));
                let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
                    return;
                };
                if !main.is_visible().unwrap_or(false) {
                    return;
                }
                // 轮询等到「窗口不再最小化」**且**「窗框尺寸读数可用」，再落位。
                // · 最小化：OS 的恢复会先把窗口摆回「上次位置」，先 set_position 会被覆盖。
                // · 尺寸：窗口刚 show、WM 还没映射时 `outer_size()` 读到 `0x0`
                //   （tao 的初值取的是坐标）。拿它算「居中」= 按 0 尺寸居中 →
                //   实测落点 y=50，而真实窗框是 460x937 → 窗底离工作区底只剩
                //   **13px** ≤ 触发距离 24 → `near_edge=true` → **启动约 1s 后自己
                //   贴边成条**（reg3-nudge / reg3-ledge 实测 `dock begin edge=Bottom
                //   from=(570,50)`）。等读数可用再落位，这一次就够（原来在 +41ms
                //   读到的还是 0x0，于是落位按 0 算；真正补上居中的那次要等到
                //   +1.2s，宽限期早过了）。
                // 上界 60×16ms≈1s：WM 映射窗口通常在几十毫秒内。
                let outer_usable = |window: &tauri::WebviewWindow| {
                    window
                        .outer_size()
                        .map(|size| size.width > 0 && size.height > 0)
                        .unwrap_or(false)
                };
                for _ in 0..60 {
                    let minimized = main.is_minimized().unwrap_or(false)
                        || is_minimized_position(main.outer_position().unwrap_or_default());
                    if !minimized && outer_usable(&main) {
                        break;
                    }
                    if minimized {
                        let _ = main.unminimize();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
                // 贴边停靠态不碰位置（它本就在工作区外、由 dock 逻辑自己管）；
                // 其余情况按固定位置模式落位，并兜底夹回工作区。
                let docked = match app.state::<Mutex<AppState>>().lock() {
                    Ok(state) => state.auto_hide_docked,
                    Err(_) => false,
                };
                if !docked {
                    apply_main_window_position(&app);
                    if let Ok(pos) = main.outer_position() {
                        if !is_minimized_position(pos) {
                            clamp_main_window(&app, &main);
                        }
                    }
                }
            });
        }
        // 贴边自动隐藏开着：守护线程会在光标离窗 350ms 后把窗口缩回边条。
        // 但**两种来源此刻的几何都还没落定**，所以都要先给一段宽限，
        // 区别只在于呼出还要额外挂「等待进入」（见 ShowOrigin）：
        // · 呼出：鼠标必然停在托盘/菜单上（窗口外）。只有宽限没有「等待进入」
        //   就是死循环 —— 呼出展开（还在屏幕边缘）→ 宽限期一过鼠标仍在托盘 →
        //   贴回 → 用户再点托盘 → 再展开……每轮 show/hide + set_decorations
        //   样式切换，两个窗口互相闪烁直到卡死。
        // · 启动首次显示：WM 会自己挑一个初始位置，而且经常直接忽略我们的首次
        //   set_position（GTK 文档：多数 WM 忽略初始位置请求），要靠自愈线程
        //   把窗口挪到设置要求的位置。这中间若让 watcher 拿 WM 那份临时位置去
        //   判定「贴边了 + 鼠标在外」，窗口会在启动 200ms 内自己缩成条
        //   （实测 Muffin：初始 800,63 底边正好齐平 → 245ms 就 dock 了）。
        {
            let state = app.state::<Mutex<AppState>>();
            let locked = state.lock();
            if let Ok(mut state) = locked {
                state.auto_hide_grace_until_ms = unix_millis() + AUTO_HIDE_SETTLE_MS;
                if recalled {
                    state.auto_hide_pending_enter = true;
                }
            }
        }
        // 居中模式（与贴边互斥）下这里不起线程
        ensure_auto_hide_watcher(app);
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
    /// 命令在，但**这条候选在本机用不了**（典型：`sudo -n` 没配免密 sudoers、
    /// macOS 的「自动化」授权被拒）。
    ///
    /// 必须和 `Missing` 一样按「换下一条候选」处理：它只是「这条方式行不通」，
    /// 不是「用户拒绝了这次操作」。若归进 `Failed`，macOS 上会直接 Stop
    /// （`escalate_power` 在 macOS 恒返回 None）→ 后面那条管理员授权候选永远跑不到。
    Unavailable(String),
    /// 命令存在但执行失败，通常是权限不足或用户取消了授权弹窗。
    Failed(String),
}

impl PowerAttemptOutcome {
    fn describe(&self) -> String {
        match self {
            Self::Success => "已提交".to_string(),
            Self::Missing(reason) | Self::Unavailable(reason) | Self::Failed(reason) => {
                reason.clone()
            }
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
        // 「没有这个命令」和「这条候选在本机用不了」都是换下一条，不是停手。
        PowerAttemptOutcome::Missing(_) | PowerAttemptOutcome::Unavailable(_) => PowerStep::Next,
        PowerAttemptOutcome::Failed(_) if can_escalate => PowerStep::Escalate,
        PowerAttemptOutcome::Failed(_) => PowerStep::Stop,
    }
}

/// 各平台的候选命令，按顺序尝试（见 plan §3.2）。
///
/// macOS 先走 **AppleEvent → System Events**：这是苹果菜单里「重新启动/关机/睡眠」
/// 的同一条链路（`loginwindow` 的 `aevt`/`rst`/`sdn` 事件），**任何用户都不需要
/// 管理员密码**，也不需要改任何配置文件；代价只是首次弹一次「自动化」授权
/// （「AnyBoot」想要控制「System Events」），点允许后写进该用户的 TCC 库，此后永久免框。
/// 用不了（系统里没授权、缺 osascript）才退到免密 sudo，最后才是管理员授权框。
/// Linux 先试免密的 systemctl（本地活动会话下 logind 允许），失败再走 pkexec；
/// Windows 用自带的 shutdown / rundll32。
#[cfg(target_os = "macos")]
fn power_attempts(action: PowerAction) -> Vec<PowerAttempt> {
    /// 走系统管理员授权框执行一条 shell 命令。
    fn admin_script(command: &str) -> PowerAttempt {
        let script = format!("do shell script \"{command}\" with administrator privileges");
        attempt("osascript", &["-e", &script], true)
    }

    /// 免密 sudo：`-n` 保证**绝不问密码** —— 配了免密 sudoers 就直接执行成功（不弹框），
    /// 没配就立刻失败并被判为 `Unavailable` → 换下一条（管理员授权框）。
    /// 也就是「配了免密才免框，没配的机器行为与以前完全一样」。
    fn sudo_nopasswd(arguments: &[&'static str]) -> PowerAttempt {
        let mut args = vec!["-n"];
        args.extend(arguments);
        attempt("sudo", &args, false)
    }

    /// AppleEvent 到 System Events（= 苹果菜单的同一条路，免 root、免配置）。
    ///
    /// `needs_gui_auth` 记 false：授权是一次性的**状态**（TCC 库），拿到之后
    /// 远程/CLI 调也一样免框；只有从未授权过的那一次会弹本机授权框
    /// （与睡眠那条候选同一约定，见 `docs/remote-api-plan.md` §3.2）。
    /// macOS 10.14+ 要求 app 声明 `NSAppleEventsUsageDescription`
    /// （`src-tauri/Info.plist`），否则**连授权框都不弹**、直接报 -1743。
    fn apple_event(verb: &str) -> PowerAttempt {
        let script = format!("tell application \"System Events\" to {verb}");
        attempt("osascript", &["-e", &script], false)
    }

    match action {
        PowerAction::Reboot => vec![
            apple_event("restart"),
            sudo_nopasswd(&["/sbin/shutdown", "-r", "now"]),
            admin_script("/sbin/shutdown -r now"),
        ],
        PowerAction::Shutdown => vec![
            apple_event("shut down"),
            sudo_nopasswd(&["/sbin/shutdown", "-h", "now"]),
            admin_script("/sbin/shutdown -h now"),
        ],
        PowerAction::Sleep => vec![
            apple_event("sleep"),
            // 配了免密 sudo 的机器：这条就成功，不用再弹框
            sudo_nopasswd(&["/usr/bin/pmset", "sleepnow"]),
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
/// `sudo -n`（免密模式）在**没有配免密 sudoers** 时的失败文案。
///
/// 这几种都不是「用户取消了授权框」，而是「这条候选在本机用不了」——
/// `-n` 的语义就是「绝不问密码」，所以它失败时机器上根本没人被问过。
/// 判定做成纯函数，单测能钉死文案（sudo 的原文随版本略有差别，多认几种）。
fn sudo_without_password(program: &str, args: &[String], stderr: &str) -> bool {
    program == "sudo"
        && args.iter().any(|argument| argument == "-n")
        && [
            "a password is required",
            "no tty present",
            "a terminal is required",
        ]
        .iter()
        .any(|needle| stderr.contains(needle))
}

/// AppleEvents（「自动化」授权）被拒时 osascript 的失败文案。
///
/// 和 `sudo -n` 一样属于「这条候选在本机用不了」，不算「用户当场取消了授权框」：
/// TCC 的拒绝是一个**持久状态**（拒了就一直是拒的，不会这次拒下次又问），
/// 所以归 `Unavailable` 换下一条候选，而不是 `Failed` 停手 —— 停手等于把
/// 整台机器的重启/关机按钮打死，用户连输密码那条路都走不到。
/// 判定做成纯函数，单测能钉死文案（osascript 的报错会随系统语言变，按错误码为主）。
fn apple_events_denied(program: &str, args: &[String], stderr: &str) -> bool {
    program == "osascript"
        && args.iter().any(|argument| argument.contains("System Events"))
        && [
            "-1743", // errAEEventNotPermitted
            "Not authorized to send Apple events",
            "not allowed to send Apple events",
        ]
        .iter()
        .any(|needle| stderr.contains(needle))
}

fn run_power_command(program: &str, args: &[String]) -> PowerAttemptOutcome {
    let display = render_command(program, args);
    match command(program).args(args).output() {
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
            // 免密 sudo 没配 → 换下一条候选（管理员授权框），别当作用户取消授权。
            if sudo_without_password(program, args, &detail) {
                return PowerAttemptOutcome::Unavailable(format!("{display}：{detail}"));
            }
            // 「自动化」没授权 → 同上，换下一条候选。
            if apple_events_denied(program, args, &detail) {
                return PowerAttemptOutcome::Unavailable(format!("{display}：{detail}"));
            }
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
/// **只有「命令不存在」和「这条候选在本机用不了」才换下一条候选**，
/// 命令能跑但失败时要么提权重试、要么停手（用户当场取消了授权框就绝不再换方式执行）。
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
                match command("powercfg").arg("/a").output() {
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
                    // 但「命令返回成功」不等于动作真的发生（见 POWER_RELEASE_MS），
                    // 所以排一次延时放闸，别让电源项永远灰着。
                    schedule_power_release(&app);
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
                if let Err(error) = app
                    .run_on_main_thread(move || show_main_window(&window_app, ShowOrigin::Recall))
                {
                    trace(&app, "power", &format!("显示主窗口失败: {error}"));
                }
            }
        },
    );
}

fn apply_dock_taskbar_visibility(app: &AppHandle, hide: bool) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        // Windows 的 tao 走 ITaskbarList::DeleteTab/AddTab：AddTab 立即生效，
        // 但 DeleteTab 对**可见**的窗口不可靠 —— 按钮当下还在，shell 还会在
        // 窗口被激活时把标签加回来（实测：设置里勾了「隐藏程序」，任务栏
        // 图标纹丝不动）。可靠的顺序是 隐藏 → 切换 → 再显示（代价是闪一下）。
        // Linux 走 GTK 的 skip-taskbar hint（映射中也会发 _NET_WM_STATE），
        // macOS 走 activation policy，两者都天然实时，不需要这一套。
        #[cfg(target_os = "windows")]
        {
            let was_visible = window.is_visible().unwrap_or(false);
            let was_maximized = window.is_maximized().unwrap_or(false);
            if was_visible {
                let _ = window.hide();
            }
            window
                .set_skip_taskbar(hide)
                .map_err(|error| format!("无法更新任务栏显示状态: {error}"))?;
            if was_visible {
                let _ = window.show();
                if was_maximized {
                    let _ = window.maximize();
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
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

// ============================================================================
// GRUB 细粒度切换（Linux / Windows / macOS 三端共用）
// ============================================================================
//
// 这是与上面 rEFInd 那套**完全并列**的第二条链路：
//
//   数据源   ESP(FAT32) 上的 `<ESP>/EFI/grub.cfg` 与 `<ESP>/EFI/grubenv`
//   取值     `grub` 模块（移植自 system-switch.sh 的 grub_self_check）
//   收集     **仅 Linux 端**把 /boot/grub 的 grub.cfg 与 grubenv 搬进 ESP
//
// 为什么不直接读 ext4：Windows / macOS 读不了 ext4（要管理员或第三方驱动），
// 而 ESP 是三端唯一都原生可读写的落点。所以让 Linux 端搬运一次，之后就三端
// 都只看 ESP —— 这也正是 `09_anyboot` 把环境块改指向 ESP 的原因。

/// 把 ESP 上的相对路径拼到本端 ESP 根目录下。
///
/// 不能直接用 `Path::join`：`ENV_REL_PATH` 以 `/` 开头，Windows 会把它当绝对
/// 路径从而丢掉盘符（`H:\`.join("/EFI/grubenv") 会变成「当前盘」的 `\EFI\grubenv`）。
fn grub_rel(dir: &Path, rel: &str) -> PathBuf {
    dir.join(rel.trim_start_matches('/'))
}

/// 原子写字节：先写同目录临时文件再改名（与 `atomic_copy` 同一套思路）。
fn atomic_write_bytes(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = temporary_target(target);
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

/// 09_anyboot 正文：只把占位符换成真值，其余一字不改（用户定案 1）。
///
/// `template` 正常就是内置的 `GRUB_SCRIPT_TEMPLATE`；用户在界面上改过脚本，就用
/// 他改的那份（设置项 `grub_script_override`）—— 「可展示 + 可修改」是明确要求。
fn grub_script_content(template: &str, uuid: &str) -> String {
    template.replace("@@ESP_UUID@@", uuid)
}

/// 当前生效的 09_anyboot 模板（内置 / 用户改过的）。
fn grub_template(settings: &Settings) -> &str {
    if settings.grub_script_override.trim().is_empty() {
        GRUB_SCRIPT_TEMPLATE
    } else {
        settings.grub_script_override.as_str()
    }
}

/// 09_anyboot 是否已安装。这个功能只在 Linux 有意义，所以非 Linux 返回 None
/// —— 界面据此**只在 Linux 展示**安装/卸载按钮。
#[cfg(target_os = "linux")]
fn grub_installed() -> Option<bool> {
    Some(Path::new(GRUB_SCRIPT_PATH).is_file())
}

#[cfg(not(target_os = "linux"))]
fn grub_installed() -> Option<bool> {
    None
}

/// 「上次同步」用的时间串：沿用项目既有的 UTC 记法（`utc_timestamp`），
/// 三端互相比对时不会因为本地时区而错位。
fn grub_synced_stamp() -> String {
    utc_timestamp(now_epoch_seconds())
}

/// 安装 / 卸载 09_anyboot 的**执行过程日志**：每跑一条外部命令就记一行，
/// 连同 stdout / stderr 与退出码一起。用户要知道「你到底做了什么」，光给一句
/// 「已安装」是不够的（一个 pkexec 弹窗背后的 `install` + `grub-mkconfig` 全看不见）。
///
/// 容量固定：这是给人看的最近一次操作的流水，不是审计日志，超过就从头丢。
const SCRIPT_LOG_LIMIT: usize = 200;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScriptLogView {
    /// 最近一次操作的名字（安装 09_anyboot / 卸载 09_anyboot）
    title: Option<String>,
    /// 逐行记录：命令行、stdout/stderr、退出码
    lines: Vec<String>,
    /// 这次操作整体成没成；None = 还没跑过
    ok: Option<bool>,
}

/// 往日志里追一行；超出上限就丢掉最老的。
fn script_log_push(lines: &mut Vec<String>, line: &str) {
    for part in line.lines() {
        lines.push(part.to_string());
    }
    if lines.len() > SCRIPT_LOG_LIMIT {
        let excess = lines.len() - SCRIPT_LOG_LIMIT;
        lines.drain(..excess);
    }
}

/// 把一次命令执行的结果按「退出码 → stdout → stderr」记进日志。
///
/// 只有 Linux 的安装 / 卸载会真正跑到它；其余平台留着是为了同一份代码不按平台分叉。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn script_log_output(lines: &mut Vec<String>, label: &str, output: &Output) {
    script_log_push(lines, &format!("[{label}] {}", output.status));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();
    if !stdout.is_empty() {
        script_log_push(lines, stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        script_log_push(lines, stderr);
    }
}

/// 记一条「命令根本没跑起来」的失败。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn script_log_error(lines: &mut Vec<String>, label: &str, error: &io::Error) {
    script_log_push(lines, &format!("[{label}] 无法执行: {error}"));
}

// ---------------------------------------------------------------------------
// ESP 分区识别：判断它是不是 FAT32，并取出**分区 UUID**（识别键）与 fs-uuid（展示）
// ---------------------------------------------------------------------------
//
// 分区 UUID（GPT PARTUUID）是识别 ESP 的权威键 —— 它写在分区表里，三端报同一个值：
//   Windows → Get-Partition 的 Guid（PowerShell 枚举）
//   Linux   → lsblk 的 PARTUUID 列（udev 数据库）
//   macOS   → diskutil info 的 Disk / Partition UUID
// fs-uuid（FAT 卷序列号，GRUB search --fs-uuid 用）只作展示/Linux 脚本取值：
//   Windows → GetVolumeInformationW 的卷序列号（就是 FAT 的 BS_VolID，与 GRUB 同源）
//   Linux   → findmnt 报的 UUID / 引导扇区（util-linux 读的就是那几个字节）
//   macOS   → 用户态拿不到（Volume UUID 是 MD5 派生值，反推不回序列号）—— 如实留空
// 任何一端都不去改分区，只读。

/// 一块分区的识别结果（取不到的字段就是 None，绝不编）。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct EspInfo {
    /// 用户给的路径（原样回显）
    path: String,
    /// 规范化后的分区根目录（挂载点 / 盘符根）
    root: Option<String>,
    /// 设备节点（Linux `/dev/sda1`、macOS `/dev/disk0s1`、Windows `\\.\H:`）
    device: Option<String>,
    /// 文件系统类型原文（FAT32 / vfat / msdos / NTFS / APFS …）
    fstype: Option<String>,
    /// 是不是 FAT 家族（FAT12/16/32）—— ESP 规范要求用它
    fat: bool,
    /// 是不是正好 FAT32
    fat32: bool,
    /// fs-uuid（GRUB `search --fs-uuid` 那一串，形如 `E537-7F31`）。
    /// 注意这是**文件系统**的标识（FAT 卷序列号），不是识别键 —— macOS
    /// 用户态取不到它（diskutil 报的是 MD5 派生的 Volume UUID），所以 None 很正常。
    uuid: Option<String>,
    /// **分区 UUID**（GPT PARTUUID，三端同值）—— 识别 ESP 的权威键。
    /// Linux 取自 lsblk 的 PARTUUID 列，macOS 取自 diskutil 的
    /// `Disk / Partition UUID`，Windows 取自 Get-Partition 的 Guid。
    partuuid: Option<String>,
    /// 当前是否已挂载 / 可访问
    mounted: bool,
    /// 这个挂载点是不是本程序挂上的（只有自己挂上的才允许自己卸）
    owned_mount: bool,
    /// fs-uuid 的来路，或者取不到时的原话（展示给人看）
    note: Option<String>,
    /// 这块分区**当前挂载在哪里**（用户可能自己先挂上了，或系统自动挂的）。
    ///
    /// Windows 上没盘符的分区 user mode 摸不到 → 这里就是 None，
    /// 界面照实说「没有盘符」，而不是装作认得。
    mount_point: Option<String>,
    /// 这个 mount_point 是不是系统/用户已有的挂载（不是本程序挂的）。
    /// 是的话「打开文件管理器」直接跳过去，也不需要再挂一次。
    preexisting: bool,
}

/// Linux：ESP 挂到哪儿。放 `$XDG_RUNTIME_DIR`（没有就 /tmp）下的**固定名字**，
/// 这样「打开文件管理器」看到的路径每次一样，用户还能把它加进书签。
#[cfg(target_os = "linux")]
fn esp_mountpoint() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(std::env::temp_dir)
        .join("anyboot-esp")
}

/// 本端能不能「按 fs-uuid 自己挂上来」。
///
/// Windows 不行：盘符是系统分配的，没盘符的分区用户态根本摸不到 ——
/// 所以界面把勾选框置灰，并提示去「磁盘管理」里分配盘符。
#[cfg(target_os = "windows")]
fn esp_mount_supported() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
fn esp_mount_supported() -> bool {
    true
}

/// 从 FAT 引导扇区里读出 `(FAT 类型, 卷序列号)` —— 纯函数，好单测。
///
/// 类型按簇数判定（<4085 = FAT12、<65525 = FAT16、否则 FAT32），不认那 8 字节的
/// 文件系统标识串（FAT16 卷上它常常写的是 `MSDOS5.0`）。序列号就是 GRUB 的
/// fs-uuid：FAT12/16 在偏移 39，FAT32 在偏移 67。
///
/// 只有 Linux 会用它去补 blkid 分不出来的 FAT16/FAT32（要走单测，所以不按平台裁掉）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn fat_boot_sector(sector: &[u8]) -> Option<(String, String)> {
    if sector.len() < 512 {
        return None;
    }
    let bytes_per_sector = u16::from_le_bytes([sector[11], sector[12]]) as u64;
    if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096) {
        return None;
    }
    let sectors_per_cluster = sector[13] as u64;
    let reserved = u16::from_le_bytes([sector[14], sector[15]]) as u64;
    let fat_count = sector[16] as u64;
    let root_entries = u16::from_le_bytes([sector[17], sector[18]]) as u64;
    let total16 = u16::from_le_bytes([sector[19], sector[20]]) as u64;
    let fat_size16 = u16::from_le_bytes([sector[22], sector[23]]) as u64;
    let total32 = u32::from_le_bytes([sector[32], sector[33], sector[34], sector[35]]) as u64;
    let fat_size32 = u32::from_le_bytes([sector[36], sector[37], sector[38], sector[39]]) as u64;
    let total = if total16 != 0 { total16 } else { total32 };
    let fat_size = if fat_size16 != 0 {
        fat_size16
    } else {
        fat_size32
    };
    if total == 0 || fat_size == 0 || fat_count == 0 || sectors_per_cluster == 0 {
        return None;
    }
    let root_dir_sectors = (root_entries * 32).div_ceil(bytes_per_sector);
    let overhead = reserved
        .saturating_add(fat_count.saturating_mul(fat_size))
        .saturating_add(root_dir_sectors);
    let clusters = total.saturating_sub(overhead) / sectors_per_cluster;
    let (kind, serial_at) = if clusters < 4085 {
        ("FAT12", 39usize)
    } else if clusters < 65525 {
        ("FAT16", 39usize)
    } else {
        ("FAT32", 67usize)
    };
    if sector.len() < serial_at + 4 {
        return None;
    }
    let serial = u32::from_le_bytes([
        sector[serial_at],
        sector[serial_at + 1],
        sector[serial_at + 2],
        sector[serial_at + 3],
    ]);
    Some((kind.to_string(), format!("{serial:08X}")))
}

/// 读设备的前 512 字节（Linux 上普通用户读 `/dev/sdX1` 多半没权限 ——
/// 那是预期的失败，调用方退回到 blkid 报的信息，这里不报错）。
#[cfg(target_os = "linux")]
fn read_first_sector(device: &Path) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = fs::File::open(device)?;
    let mut buffer = vec![0u8; 512];
    file.read_exact(&mut buffer)?;
    Ok(buffer)
}

/// Windows：盘符路径 → 卷信息。
#[cfg(target_os = "windows")]
fn esp_probe(path: &Path) -> Result<EspInfo, String> {
    let text = path.to_string_lossy().to_string();
    let letter = text
        .trim()
        .chars()
        .next()
        .filter(char::is_ascii_alphabetic)
        .map(|letter| letter.to_ascii_uppercase())
        .ok_or_else(|| format!("请填盘符路径，例如 H:\\（当前填的是 {text}）"))?;
    let (filesystem, serial) = windows_volume(letter).ok_or_else(|| {
        format!("读不到 {letter}: 的卷信息：盘符不存在，或该分区还没在「磁盘管理」里分配盘符")
    })?;
    let name = filesystem.to_ascii_uppercase();
    let fat32 = name == "FAT32";
    let root = format!("{letter}:\\");
    // 识别键 = 分区 UUID（Get-Partition 的 Guid），一次 PowerShell 枚举全量再查表
    let partuuid = windows_partition_guids()
        .into_iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(&letter))
        .map(|(_, guid)| guid)
        .filter(|guid| grub::partuuid_digits(guid).is_some());
    Ok(EspInfo {
        path: text,
        root: Some(root.clone()),
        device: Some(format!("\\\\.\\{letter}:")),
        fstype: Some(filesystem),
        fat: fat32 || name == "FAT" || name == "FAT16",
        fat32,
        uuid: Some(grub::uuid_pretty(&serial)),
        partuuid,
        mounted: true,
        owned_mount: false,
        note: Some(
            "分区 UUID 取自 Get-Partition 的 Guid（识别键，三端同值）；fs-uuid 取自 \
             Windows 卷序列号 —— GRUB 的 search --fs-uuid 用它"
                .to_string(),
        ),
        // 有盘符就是「挂着的」，而且是系统给的挂载（不是我们挂的）
        mount_point: Some(root),
        preexisting: true,
    })
}

#[cfg(target_os = "linux")]
#[derive(Deserialize)]
struct FindmntOutput {
    #[serde(default)]
    filesystems: Vec<FindmntEntry>,
}

#[cfg(target_os = "linux")]
#[derive(Deserialize)]
struct FindmntEntry {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    fstype: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

/// Linux：路径 → 它所在的那块分区。
///
/// `findmnt --target` 取的是「包含这个路径的那一次挂载」，所以用户填
/// `/boot/efi/EFI/` 还是 `/run/media/me/ARCH-EFI/EFI/grub.cfg`，都会落到同一块 FAT 上。
#[cfg(target_os = "linux")]
fn esp_probe(path: &Path) -> Result<EspInfo, String> {
    let root = fs::canonicalize(path)
        .map_err(|error| format!("路径打不开：{}（{error}）", path.display()))?;
    let output = command("findmnt")
        .args([
            "--json",
            "--output",
            "SOURCE,FSTYPE,UUID,TARGET",
            "--target",
        ])
        .arg(&root)
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法执行 findmnt: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{} 不在任何已挂载的分区上（ESP 还没挂上来？）",
            root.display()
        ));
    }
    let parsed: FindmntOutput = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("无法解析 findmnt 输出: {error}"))?;
    let entry = parsed
        .filesystems
        .into_iter()
        .next()
        .ok_or_else(|| format!("{} 不在任何已挂载的分区上", root.display()))?;

    let mut fstype = entry.fstype.clone().unwrap_or_else(|| "未知".to_string());
    let mut uuid = entry
        .uuid
        .clone()
        .filter(|value| !value.trim().is_empty())
        .map(|value| match grub::uuid_digits(&value) {
            Some(digits) if digits.len() == 8 => grub::uuid_pretty(&digits),
            _ => value,
        });
    let mut note = "fs-uuid 取自 findmnt 报的 UUID（与 GRUB 同源）".to_string();
    // 能直接读引导扇区就自己算一遍：blkid 的 `vfat` 分不出 FAT16/FAT32，
    // 引导扇区里的簇数分得出来，序列号也读得到。读不了（普通用户没权限）就算了。
    if let Some(device) = entry.source.as_deref() {
        if let Ok(sector) = read_first_sector(Path::new(device)) {
            if let Some((kind, serial)) = fat_boot_sector(&sector) {
                fstype = kind;
                uuid = Some(grub::uuid_pretty(&serial));
                note = format!("fs-uuid 取自 {device} 引导扇区里的 BS_VolID（最权威）");
            }
        }
    }
    let name = fstype.to_ascii_uppercase();
    let fat32 = name == "FAT32";
    // Linux 内核里这文件系统就叫 vfat（fat32 只是 mount(8) 的别名），
    // blkid/findmnt/lsblk 报的都是 vfat —— 引导扇区读不到（没权限）时
    // 走到这里，别把它漏出 FAT 家族。
    let fat = fat32
        || matches!(
            name.as_str(),
            "FAT" | "FAT12" | "FAT16" | "VFAT" | "MSDOS"
        );
    if matches!(name.as_str(), "VFAT" | "MSDOS") {
        note = format!("Linux 把 FAT12/16/32 都报成 {fstype}；ESP 一般是 FAT32（{note}）");
    }
    // 识别键 = 分区 UUID：对 findmnt 报的 SOURCE 设备查 lsblk 的 PARTUUID 列
    //（udev 数据库，普通用户可读）。查不到就如实留空，note 里说去哪补。
    let partuuid = entry.source.as_deref().and_then(|device| {
        let output = command("lsblk")
            .args(["--noheadings", "--output", "PARTUUID"])
            .arg(device)
            .stderr(Stdio::null())
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    let partuuid = partuuid.filter(|value| grub::partuuid_digits(value).is_some());
    Ok(EspInfo {
        path: path.to_string_lossy().to_string(),
        root: Some(root.display().to_string()),
        device: entry.source,
        fstype: Some(fstype),
        fat,
        fat32,
        uuid,
        partuuid,
        mounted: true,
        owned_mount: false,
        note: Some(note),
        // findmnt --target 报的 TARGET 就是这块分区的**挂载点本身**
        mount_point: entry.target,
        preexisting: true,
    })
}

/// macOS：`diskutil info` 里我们要的那几行。
#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Default)]
struct DiskInfo {
    node: Option<String>,
    identifier: Option<String>,
    personality: Option<String>,
    uuid: Option<String>,
    mount_point: Option<String>,
    /// 分区 UUID（GPT PARTGUID，diskutil 报作 `Disk / Partition UUID`）。
    /// 与 Linux blkid/lsblk 的 PARTUUID、Windows Get-Partition 的 Guid 同值，
    /// 是识别 ESP 的权威键。
    partition_uuid: Option<String>,
}

#[cfg(target_os = "macos")]
fn macos_read_line(info: &mut DiskInfo, line: &str) {
    let Some((key, value)) = line.split_once(':') else {
        return;
    };
    let value = value.trim();
    match key.trim() {
        "Device Node" => info.node = Some(value.to_string()),
        "Device Identifier" => info.identifier = Some(value.to_string()),
        "File System Personality" => info.personality = Some(value.to_string()),
        "Volume UUID" => info.uuid = Some(value.to_string()),
        "Disk / Partition UUID" => info.partition_uuid = Some(value.to_string()),
        // 未挂载时 diskutil 打的就是 "Mount Point: None"
        "Mount Point" if value != "None" => info.mount_point = Some(value.to_string()),
        _ => {}
    }
}

#[cfg(target_os = "macos")]
fn macos_disk_info(path: &Path) -> Option<DiskInfo> {
    let output = command("diskutil").arg("info").arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut info = DiskInfo::default();
    for line in text.lines() {
        macos_read_line(&mut info, line);
    }
    Some(info)
}

/// `diskutil info -all`：每块盘一段，段之间是一整行星号。未挂载的卷也能从这里看到。
#[cfg(target_os = "macos")]
fn macos_all_disks() -> Vec<DiskInfo> {
    let Ok(output) = command("diskutil").arg("info").arg("-all").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut disks = Vec::new();
    let mut current = DiskInfo::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.len() >= 8 && trimmed.chars().all(|character| character == '*') {
            if current.node.is_some() || current.identifier.is_some() {
                disks.push(std::mem::take(&mut current));
            }
            continue;
        }
        macos_read_line(&mut current, line);
    }
    if current.node.is_some() || current.identifier.is_some() {
        disks.push(current);
    }
    disks
}

/// macOS：路径 → 它所在的那块分区。
#[cfg(target_os = "macos")]
fn esp_probe(path: &Path) -> Result<EspInfo, String> {
    let root = fs::canonicalize(path)
        .map_err(|error| format!("路径打不开：{}（{error}）", path.display()))?;
    let info = macos_disk_info(&root)
        .ok_or_else(|| format!("diskutil 读不到 {}（分区已卸载？）", root.display()))?;
    let personality = info
        .personality
        .clone()
        .unwrap_or_else(|| "未知".to_string());
    let upper = personality.to_ascii_uppercase();
    let fat32 = upper.contains("FAT32");
    let fat = fat32 || upper.contains("FAT");
    // 识别键 = 分区 UUID（diskutil 的 `Disk / Partition UUID`，GPT PARTGUID）
    let partuuid = info
        .partition_uuid
        .clone()
        .filter(|value| grub::partuuid_digits(value).is_some());
    let note = match (&info.uuid, partuuid.is_some()) {
        (_, true) => {
            "分区 UUID 取自 diskutil 报的 Disk / Partition UUID（识别键，三端同值）".to_string()
        }
        (Some(raw), false) => format!(
            "diskutil 报的 Volume UUID {raw} 是 MD5 派生值（FAT 卷没有原生 UUID），\
             且没报出分区 UUID —— 请在 Linux（lsblk -o PARTUUID）或 Windows \
             （Get-Partition）里读同一分区的分区 UUID"
        ),
        (None, false) => "diskutil 没报分区 UUID —— 请在 Linux（lsblk -o PARTUUID）\
             或 Windows（Get-Partition）里读同一分区的分区 UUID"
            .to_string(),
    };
    Ok(EspInfo {
        path: path.to_string_lossy().to_string(),
        root: Some(
            info.mount_point
                .clone()
                .unwrap_or_else(|| root.display().to_string()),
        ),
        device: info
            .node
            .clone()
            .or_else(|| info.identifier.as_deref().map(|id| format!("/dev/{id}"))),
        fstype: Some(personality),
        fat,
        fat32,
        // fs-uuid：macOS 用户态拿不到（引导扇区要特权、Volume UUID 是派生值
        // 反推不回序列号）—— 如实留空，note 里说清楚。识别键用上面的 partuuid。
        uuid: None,
        partuuid,
        mounted: info.mount_point.is_some(),
        owned_mount: false,
        note: Some(note),
        mount_point: info.mount_point.clone(),
        preexisting: info.mount_point.is_some(),
    })
}

/// macOS：按分区 UUID 找到那块盘并 `diskutil mount`。
#[cfg(target_os = "macos")]
fn esp_mount(partuuid: &str) -> Result<Option<ActiveMount>, String> {
    // 已经挂着了（能在 /Volumes 下按分区 UUID 认出来）就不用动，owned=false 表示不归我们管
    if let Ok((dir, _)) = grub_esp_dir(partuuid) {
        return Ok(Some(ActiveMount {
            device: PathBuf::new(),
            mountpoint: dir,
            owned: false,
        }));
    }
    let node = macos_all_disks()
        .into_iter()
        .find(|info| {
            info.partition_uuid
                .as_deref()
                .is_some_and(|value| grub::uuid_matches(value, partuuid))
        })
        .and_then(|info| {
            info.node
                .or_else(|| info.identifier.map(|id| format!("/dev/{id}")))
        })
        .ok_or_else(|| {
            format!("没找到分区 UUID 为 {partuuid} 的分区（diskutil info -all 里没有对得上的）")
        })?;
    let output = command("diskutil")
        .arg("mount")
        .arg(&node)
        .output()
        .map_err(|error| format!("无法执行 diskutil mount: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "diskutil mount {node} 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mountpoint = macos_disk_info(Path::new(&node))
        .and_then(|info| info.mount_point)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{node} 挂上了，但 diskutil 没报出挂载点"))?;
    Ok(Some(ActiveMount {
        device: PathBuf::from(node),
        mountpoint,
        owned: true,
    }))
}

// ---------------------------------------------------------------------------
// 本端按分区 UUID（GPT PARTUUID，三端同值）定位 ESP
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn GetLogicalDrives() -> u32;
    fn GetVolumeInformationW(
        root: *const u16,
        volume_name: *mut u16,
        volume_name_size: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        filesystem_name: *mut u16,
        filesystem_name_size: u32,
    ) -> i32;
}

/// 盘符 → `(文件系统名, 卷序列号十六进制)`；盘符不存在或没有介质时返回 None。
///
/// 卷序列号是**文件系统**的 fs-uuid（GRUB `search --fs-uuid` 比的那串，
/// `E5377F31` 形态）—— 仅作展示；识别键是分区 UUID，走下面的 PowerShell。
#[cfg(target_os = "windows")]
fn windows_volume(letter: char) -> Option<(String, String)> {
    let root: Vec<u16> = format!("{}:\\", letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut serial: u32 = 0;
    let mut filesystem = vec![0u16; 32];
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    let end = filesystem
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(filesystem.len());
    Some((
        String::from_utf16_lossy(&filesystem[..end]),
        format!("{serial:08X}"),
    ))
}

/// 一次 PowerShell 调用枚举所有带盘符分区的 GUID：`盘符 → 分区 UUID`。
///
/// GPT 分区 GUID 与 Linux lsblk 的 PARTUUID、macOS diskutil 的
/// `Disk / Partition UUID` 是同一个值（UEFI 标准渲染，都是 8-4-4-4-12）。
/// Guid 属性经 ConvertTo-Json 序列化成 `"{…}"` 带花括号的串，要剥掉。
/// MBR 盘没有 GUID（报告全零），比不上就如实命不中。
#[cfg(target_os = "windows")]
fn windows_partition_guids() -> Vec<(char, String)> {
    let output = match command("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Partition | Where-Object DriveLetter | Select-Object DriveLetter,Guid | ConvertTo-Json -Compress",
        ])
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    let parsed: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    // 单个分区时 ConvertTo-Json 出对象而不是数组 —— 统一当数组处理
    let rows = match parsed {
        serde_json::Value::Array(rows) => rows,
        row @ serde_json::Value::Object(_) => vec![row],
        _ => return Vec::new(),
    };
    rows.into_iter()
        .filter_map(|row| {
            let letter = row.get("DriveLetter")?.as_str()?.chars().next()?;
            let guid = row.get("Guid")?.as_str()?;
            Some((letter, guid.trim_matches(['{', '}']).to_string()))
        })
        .collect()
}

/// Windows：按分区 UUID 认盘（有盘符 = 可访问，没有就如实说）。
#[cfg(target_os = "windows")]
fn grub_esp_dir(partuuid: &str) -> Result<(PathBuf, Option<ActiveMount>), String> {
    let guids = windows_partition_guids();
    if guids.is_empty() {
        return Err(
            "读不到分区的 GUID（PowerShell Get-Partition 失败）：确认这是 GPT 磁盘"
                .to_string(),
        );
    }
    for (letter, guid) in guids {
        if grub::uuid_matches(&guid, partuuid) {
            return Ok((PathBuf::from(format!("{letter}:\\")), None));
        }
    }
    Err(format!(
        "没找到分区 UUID 为 {partuuid} 的分区：确认它已在「磁盘管理」里分配了盘符"
    ))
}

/// Windows 上「已挂载 = 有盘符」，所以 `grub_esp_dir` 本身就是挂载扫描 —— 不用再来一遍。
#[cfg(target_os = "windows")]
fn find_mounted_by_partuuid(partuuid: &str) -> Option<PathBuf> {
    grub_esp_dir(partuuid).ok().map(|(dir, _)| dir)
}

/// Linux：`lsblk --json` 里按**分区 UUID**（PARTUUID 列）认盘。已挂载就用现成
/// 挂载点；没挂载才由我们用 `mount`（必要时 pkexec 提权）以 **rw** 挂上。
#[cfg(target_os = "linux")]
fn grub_esp_dir(partuuid: &str) -> Result<(PathBuf, Option<ActiveMount>), String> {
    let output = command("lsblk")
        .args(["--json", "--output", "PATH,FSTYPE,UUID,PARTUUID,MOUNTPOINTS"])
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法执行 lsblk: {error}"))?;
    let devices: LsblkOutput = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("无法解析 lsblk 输出: {error}"))?;

    for device in devices.blockdevices {
        let (Some(path), Some(candidate)) = (device.path.clone(), device.partuuid.clone()) else {
            continue;
        };
        if !grub::uuid_matches(&candidate, partuuid) {
            continue;
        }
        // 已经挂载：直接用用户的挂载点，绝不擅自改动别人的挂载
        if let Some(mountpoint) = device
            .mountpoints
            .clone()
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .find(|mountpoint| !mountpoint.is_empty())
        {
            return Ok((PathBuf::from(mountpoint), None));
        }

        let mountpoint = esp_mountpoint();
        // 只建目录、不先 remove_dir：这个路径可能是上一次自己挂上的活挂载点，
        // 删不掉也没关系 —— mount(8) 允许挂到非空目录。
        if !mountpoint.is_dir() {
            fs::create_dir_all(&mountpoint)
                .map_err(|error| format!("无法创建临时挂载点: {error}"))?;
        }
        let filesystem = device.fstype.clone().unwrap_or_else(|| "vfat".to_string());
        match mount_command("mount")
            .args(["-o", &mount_options("rw"), "-t", &filesystem])
            .arg(&path)
            .arg(&mountpoint)
            .output()
        {
            Ok(result) if result.status.success() => {
                return Ok((
                    mountpoint.clone(),
                    Some(ActiveMount {
                        device: PathBuf::from(path),
                        mountpoint,
                        owned: true,
                    }),
                ));
            }
            Ok(result) => {
                let _ = fs::remove_dir(&mountpoint);
                return Err(format!(
                    "挂载 ESP 失败: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ));
            }
            Err(error) => {
                let _ = fs::remove_dir(&mountpoint);
                return Err(format!("无法执行 mount: {error}"));
            }
        }
    }
    Err(format!("没找到分区 UUID 为 {partuuid} 的分区（lsblk 里没有对得上的 PARTUUID）"))
}

/// Linux：**用户已经挂上的**那块分区，按分区 UUID 找它的挂载点。
///
/// 和 `grub_esp_dir` 的区别：这个**绝不挂载**，只用 `lsblk` 已经报出来的
/// MOUNTPOINTS —— 用户自己挂好之后，我们直接把那个路径拿来用。
#[cfg(target_os = "linux")]
fn find_mounted_by_partuuid(partuuid: &str) -> Option<PathBuf> {
    let output = command("lsblk")
        .args(["--json", "--output", "PARTUUID,MOUNTPOINTS"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let devices: LsblkOutput = serde_json::from_slice(&output.stdout).ok()?;
    for device in devices.blockdevices {
        let Some(candidate) = device.partuuid.as_deref() else {
            continue;
        };
        if !grub::uuid_matches(candidate, partuuid) {
            continue;
        }
        let mountpoints: Vec<String> = device
            .mountpoints
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .filter(|mountpoint| !mountpoint.is_empty())
            .collect();
        if mountpoints.is_empty() {
            continue;
        }
        // 一块分区可以同时挂在多处（fstab 的 /boot/efi + 桌面的自动挂载）。
        // 优先取 /boot 下的挂载点 —— 那是 fstab 方案里「系统已经挂好的引导
        // 数据」；/media 之类的自动挂载只是顺手，不该被当成首选。
        return mountpoints
            .iter()
            .find(|mountpoint| under_boot(mountpoint))
            .or_else(|| mountpoints.first())
            .map(PathBuf::from);
    }
    None
}

/// 挂载点是否在 `/boot` 下（fstab 统一方案的约定位置）。纯函数，便于单测。
#[cfg(target_os = "linux")]
fn under_boot(mountpoint: &str) -> bool {
    mountpoint == "/boot" || mountpoint.starts_with("/boot/")
}

/// 「同步」按钮的硬性条件（用户定案）：当前系统把这个分区挂在了 **`/boot` 或
/// `/boot/efi`** —— 两个位置都算数。挂别处（/media 之类）或没挂都不同步。
/// 纯函数，便于单测。
#[cfg(target_os = "linux")]
fn sync_mount_ok(mountpoint: &str) -> bool {
    mountpoint == "/boot" || mountpoint == "/boot/efi"
}

/// 启动/刷新链路的同步硬性条件（用户定案）：ESP 必须已经由**当前系统**
/// 挂载好（fstab 挂在 /boot 下），启动同步才会进行；否则**一个字节都不动**
/// —— 不自动挂载、不 pkexec、不写盘。判据：
/// * 填了分区 UUID → `lsblk` 里它有挂载点且在 /boot 下；
/// * 只填了目录 → `findmnt --target` 报的挂载点在 /boot 下（目录没挂上时
///   报的是根挂载 `/`，直接不通过 —— 防止把 grub.cfg 写进根文件系统的
///   普通目录）。
#[cfg(target_os = "linux")]
fn grub_esp_ready(settings: &Settings) -> bool {
    let uuid = settings.grub_efi_partuuid.trim();
    let dir = settings.grub_efi_dir.trim();
    if !uuid.is_empty() {
        return find_mounted_by_partuuid(uuid)
            .is_some_and(|mount| under_boot(&mount.display().to_string()));
    }
    if !dir.is_empty() {
        let path = Path::new(dir);
        if !path.is_dir() {
            return false;
        }
        return esp_probe(path)
            .ok()
            .and_then(|info| info.mount_point)
            .is_some_and(|target| under_boot(&target));
    }
    false
}

/// 非 Linux：`grub_collect` 是空操作，启动同步没有副作用，硬性条件恒满足。
#[cfg(not(target_os = "linux"))]
fn grub_esp_ready(_settings: &Settings) -> bool {
    true
}

/// 启动/刷新链路的「启动识别」（用户定案 2026-10-13）：**打开软件就调用一次
/// 识别**，UUID 优先、目录兜底。识别只解析不写盘（sync=false），解析出的挂载点
/// 经 `discovered` 回填设置里的「ESP 目录」并显示到目录框。
///
/// * UUID 填了就直接走 `grub_scan` —— 与「识别」按钮同一条链路：先看已挂载的，
///   没挂载按需挂载（Linux 上 mount 失败/pkexec 被取消 → Err，回落目录分支）。
///   这取代了早先「启动绝不挂载」的定案：UUID 的意义就在于不用管它挂在哪。
/// * 目录兜底：清掉 UUID 再扫（识别链路不许再触发第二次按需挂载），分区
///   UUID 取到了回填（仅在 UUID 本来为空时 —— 用户填了的 UUID 不被覆盖）。
/// 两个都无效 → 返回 None，调用方维持现状。
/// 返回 `(scan, 落选时待归还的挂载具柄)`。
fn startup_identify_scan(
    app: &AppHandle,
    settings: &Settings,
    reusable: Option<ActiveMount>,
) -> (Option<GrubScan>, Option<ActiveMount>) {
    let uuid = settings.grub_efi_partuuid.trim();
    let dir = settings.grub_efi_dir.trim();

    // ① UUID 优先：grub_locate 先看已挂载的，没挂载按需挂载（= 识别按钮行为）
    if !uuid.is_empty() {
        match grub_scan(settings, reusable, false) {
            Ok(scan) => {
                trace(app, "grub", "startup identify: via partuuid");
                return (scan, None);
            }
            // 定位失败（没这分区 / 挂载被取消）：reusable 已被 grub_scan 收尾，
            // 回落目录分支再试一次 —— 目录可能指向手动挂载的位置。
            Err(error) => {
                trace(app, "grub", &format!("startup identify: partuuid failed: {error}"));
                return fall_back_to_dir(app, settings, None, uuid, dir);
            }
        }
    }

    fall_back_to_dir(app, settings, reusable, uuid, dir)
}

/// 目录兜底分支（UUID 缺失或定位失败时）：清掉 UUID 再扫，杜绝按需挂载。
fn fall_back_to_dir(
    app: &AppHandle,
    settings: &Settings,
    reusable: Option<ActiveMount>,
    uuid: &str,
    dir: &str,
) -> (Option<GrubScan>, Option<ActiveMount>) {
    if !dir.is_empty() && Path::new(dir).is_dir() {
        let mut scan_settings = settings.clone();
        scan_settings.grub_efi_partuuid = String::new();
        match grub_scan(&scan_settings, reusable, false) {
            Ok(Some(scan)) => {
                if !scan.esp.fat {
                    // 目录不是 FAT32：指错了地方，识别作废；具柄按 merge 结果归还
                    trace(app, "grub", "startup identify: dir is not FAT32, abort");
                    return (None, scan.mount);
                }
                trace(app, "grub", "startup identify: via dir");
                if uuid.is_empty() {
                    if let Some(partuuid) = scan
                        .esp
                        .partuuid
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let mut new_settings = settings.clone();
                        new_settings.grub_efi_partuuid = partuuid.to_string();
                        let _ = save_settings(app, &new_settings);
                    }
                }
                return (Some(scan), None);
            }
            // Ok(None) 到不了（dir 非空）；Err 时 grub_scan 已把具柄收尾
            _ => return (None, None),
        }
    }

    (None, reusable)
}

/// macOS：遍历已挂载的卷，按**分区 UUID**（`Disk / Partition UUID`）定位。
///
/// 键就是 GPT 分区表里的 GUID，Linux/Windows 报的是同一个值 —— 直接归一化
/// 比对即可，不需要任何派生换算。命不中就如实报错并提示先手动挂载。
#[cfg(target_os = "macos")]
fn grub_esp_dir(partuuid: &str) -> Result<(PathBuf, Option<ActiveMount>), String> {
    let entries =
        fs::read_dir("/Volumes").map_err(|error| format!("无法读取 /Volumes: {error}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(info) = macos_disk_info(&path) else {
            continue;
        };
        if info
            .partition_uuid
            .as_deref()
            .is_some_and(|value| grub::uuid_matches(value, partuuid))
        {
            return Ok((path, None));
        }
    }
    Err(format!(
        "没在 /Volumes 下找到分区 UUID 为 {partuuid} 的 ESP；请先在 macOS 上手动挂载该分区"
    ))
}

/// macOS：已挂载的卷里按分区 UUID 找（和 `grub_esp_dir` 同一套比对）。
#[cfg(target_os = "macos")]
fn find_mounted_by_partuuid(partuuid: &str) -> Option<PathBuf> {
    grub_esp_dir(partuuid).ok().map(|(dir, _)| dir)
}

// ---------------------------------------------------------------------------
// 往 ESP 里写：自己写得动就自己写，写不动就交给 root（pkexec）
// ---------------------------------------------------------------------------
//
// 标准发行版上，这两个动作**只有 root 做得到**，跟用户的挂载配置无关：
//
//   读源 `/boot/grub/grub.cfg` —— /usr/bin/grub-mkconfig 自己 `umask 077` 写它，
//                                 是刻意的 0600（GRUB 怕里面嵌了密码）；
//   写目标 `<ESP>/EFI/…`      —— 被系统（fstab）挂在 /boot/efi 的 vfat 属主是 root，
//                                 `dmask=0022` → 普通用户建不了文件。
//
// 而「程序自己按 fs-uuid 挂上来的」那份是 `-o rw,uid=<登录用户>`，两端都通 ——
// 于是出现反直觉的结果：ESP **没挂**时同步能成，ESP 被系统挂好了反而失败。
// 所以这里不猜挂载来自谁，直接看**实际权限**：能自己干就自己干，干不了就让 root
// 用 `install` 落盘（与安装 09_anyboot 同一条路，见 `grub_install_script`）。

/// 远程/CLI 请求正在执行：这段时间内**不弹**本机授权框（Q3 定稿 —— 远程请求不能
/// 挂在那里等人去点确认）。需要 root 的步骤直接失败并说清该怎么办。
///
/// 为什么用全局量而不是逐层传参：策略入口只有 `execute_remote_command` 一个，但它
/// 触发的一次扫描要穿过 rescan_grub → grub_scan → grub_collect 三层，中间还夹着
/// 本机动作共用的 `refresh_and_update` —— 传参会把本机那条路也污染一遍。
static REMOTE_NO_GUI_AUTH: AtomicBool = AtomicBool::new(false);

/// 作用域守卫：把一次远程请求圈起来（panic 也不会漏关）。
struct RemoteNoGuiAuth;

impl RemoteNoGuiAuth {
    /// 只有「本次请求不允许本机授权」时才需要圈。
    fn enter(allow_gui_auth: bool) -> Option<Self> {
        if allow_gui_auth {
            return None;
        }
        REMOTE_NO_GUI_AUTH.store(true, Ordering::SeqCst);
        Some(Self)
    }
}

impl Drop for RemoteNoGuiAuth {
    fn drop(&mut self) {
        REMOTE_NO_GUI_AUTH.store(false, Ordering::SeqCst);
    }
}

/// 需要 root 但当前策略不许弹授权框时的统一说法（远程改不动就明说改不动）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn gui_auth_forbidden() -> Option<String> {
    REMOTE_NO_GUI_AUTH.load(Ordering::SeqCst).then(|| {
        "需要本机授权（远程请求默认不弹授权框）：请在目标机主窗口点一次「识别」，\
         或打开 remoteAllowGuiAuth，或把 ESP 挂成当前用户可写（见 README 权限说明）"
            .to_string()
    })
}

/// 当前用户对 `path` 有没有 `mode` 权限（Linux 的 `access(2)`；`libc::R_OK` / `W_OK`）。
#[cfg(target_os = "linux")]
fn user_can(path: &Path, mode: libc::c_int) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(raw) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(raw.as_ptr(), mode) == 0 }
}

/// 这次写 ESP 要不要 root：目标目录写不进去（系统挂的 ESP），或源文件读不了（0600）。
#[cfg(target_os = "linux")]
fn esp_write_needs_root(source: Option<&Path>, target: &Path) -> bool {
    let dir_writable = target.parent().is_some_and(|dir| user_can(dir, libc::W_OK));
    let source_readable = source.is_none_or(|path| user_can(path, libc::R_OK));
    !dir_writable || !source_readable
}

/// ESP 上的副本是不是已经是源的这一份 —— 只看 size/mtime，**不读内容**
/// （源 0600，普通用户根本读不了）。
///
/// 为什么要判：每次写都要 root、每次 root 都要弹一次授权框，而 `/boot/grub/grub.cfg`
/// 只在用户跑 grub-mkconfig 时才变。ESP 是 FAT，mtime 只有 **2 秒**精度
/// （`install -p` 落盘时秒以下会被截掉），所以留 2 秒容差。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn same_size_and_time(source: &Path, target: &Path) -> bool {
    let (Ok(source_meta), Ok(target_meta)) = (fs::metadata(source), fs::metadata(target)) else {
        return false;
    };
    if source_meta.len() != target_meta.len() {
        return false;
    }
    let Ok(source_time) = source_meta.modified() else {
        return false;
    };
    let Ok(target_time) = target_meta.modified() else {
        return false;
    };
    let delta = match source_time.duration_since(target_time) {
        Ok(delta) => delta,
        Err(error) => error.duration(),
    };
    delta <= Duration::from_secs(2)
}

/// 以 root 把 `source` 写到 `target`：`pkexec install -p -m 0644`（本来就是 root 就直连）。
///
/// * 读源和写目标必须是**同一条命令**：源 0600，普通用户连读都读不到，没法先在用户态
///   复制一份再提权写。
/// * `install -p` 把源的 mtime 一起带过去 —— 下一次 `same_size_and_time` 才认得出
///   「已同步」，否则每次扫描都要再授权一次。
#[cfg(target_os = "linux")]
fn privileged_install(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(error) = gui_auth_forbidden() {
        return Err(error);
    }
    let output = mount_command("install")
        .args(["-p", "-m", "0644"])
        .arg(source)
        .arg(target)
        .output()
        .map_err(|error| format!("无法执行 install（经 pkexec 提权）: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    Err(format!(
        "root 侧没写完：{}",
        if stderr.is_empty() {
            "install 退出码非 0".to_string()
        } else {
            format!("{stderr}（授权被取消？本机没有 polkit 授权代理？）")
        }
    ))
}

/// `$TMPDIR` 下一个本次调用专用的路径（与 `temporary_target` 同一套去重思路）。
#[cfg(target_os = "linux")]
fn unique_temp_path(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("{prefix}-{}-{unique}", std::process::id()))
}

/// 把内存里的字节落进 ESP：先写用户态暂存文件，再让 root `install` 过去。
///
/// 必须暂存：`pkexec` 只跑得动一条命令，而内容是我们现算出来的（ESP 上的环境块就是
/// 这样）。暂存文件 0600 —— 里面没有秘密，但没必要给它更宽的权限。
#[cfg(target_os = "linux")]
fn stage_and_install(bytes: &[u8], target: &Path) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let staging = unique_temp_path("anyboot-esp-stage");
    let staged = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = staged {
        let _ = fs::remove_file(&staging);
        return Err(format!("无法创建暂存文件 {}: {error}", staging.display()));
    }
    let result = privileged_install(&staging, target);
    let _ = fs::remove_file(&staging);
    result
}

/// 需要 root 才能把这份文件写进 ESP 时返回 `Some(结果)`；自己写得动就是 `None`。
#[cfg(target_os = "linux")]
fn esp_file_via_root(source: &Path, target: &Path) -> Option<Result<(), String>> {
    if !esp_write_needs_root(Some(source), target) {
        return None;
    }
    Some(privileged_install(source, target).and_then(|()| {
        // 提权这条路读不了源（0600），核对只能看 `install -p` 带过来的 size/mtime
        if same_size_and_time(source, target) {
            Ok(())
        } else {
            Err("写入后核对不一致（副本的 size/mtime 与源对不上）".to_string())
        }
    }))
}

#[cfg(not(target_os = "linux"))]
fn esp_file_via_root(_source: &Path, _target: &Path) -> Option<Result<(), String>> {
    None
}

/// 同上，但内容在内存里（先落暂存文件，见 `stage_and_install`）。
#[cfg(target_os = "linux")]
fn esp_bytes_via_root(target: &Path, bytes: &[u8]) -> Option<Result<(), String>> {
    esp_write_needs_root(None, target).then(|| stage_and_install(bytes, target))
}

#[cfg(not(target_os = "linux"))]
fn esp_bytes_via_root(_target: &Path, _bytes: &[u8]) -> Option<Result<(), String>> {
    None
}

/// 把一份**用户态已经算好的字节**写进 ESP（ESP 上的环境块走这条）。
fn write_esp_bytes(target: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(result) = esp_bytes_via_root(target, bytes) {
        return result;
    }
    atomic_write_bytes(target, bytes).map_err(|error| error.to_string())
}

/// 把系统里另一个文件同步进 ESP（`/boot/grub/grub.cfg` 走这条）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn copy_into_esp(source: &Path, target: &Path) -> Result<(), String> {
    // 已经是源的这一份就别再写：写一次就要一次 root 授权
    if same_size_and_time(source, target) {
        return Ok(());
    }
    if let Some(result) = esp_file_via_root(source, target) {
        return result;
    }
    atomic_copy(source, target).map_err(|error| error.to_string())?;
    // 副本带上源的 mtime：下一次 `same_size_and_time` 才认得出「已同步」
    if let Ok(modified) = fs::metadata(source).and_then(|meta| meta.modified()) {
        let _ = fs::OpenOptions::new()
            .write(true)
            .open(target)
            .and_then(|file| file.set_modified(modified));
    }
    if !hashes_match(source, target) {
        return Err("写入后校验不一致".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 收集（仅 Linux）与读取
// ---------------------------------------------------------------------------

/// Linux：把 `/boot/grub` 的两份数据搬进 ESP。
///
/// * `grub.cfg` **每次都覆盖** —— 它就是 `update-grub` 的产物，内容必须跟得上。
/// * `grubenv` **只在 ESP 上还没有时才搬**（首次迁移）。装上 09_anyboot 之后
///   GRUB 只写 ESP 那一份，ext4 那份是旧的；每次覆盖会把 GRUB 刚记住的选择冲掉。
#[cfg(target_os = "linux")]
fn grub_collect(efi_dir: &Path) -> Result<(), String> {
    let source = ["/boot/grub/grub.cfg", "/boot/grub2/grub.cfg"]
        .iter()
        .map(Path::new)
        .find(|path| path.is_file())
        .ok_or_else(|| "没找到 /boot/grub/grub.cfg（或 /boot/grub2/grub.cfg）".to_string())?;
    let target = grub_rel(efi_dir, grub::CFG_REL_PATH);
    copy_into_esp(source, &target)
        .map_err(|error| format!("同步 grub.cfg 到 ESP 失败: {error}"))?;

    let env_target = grub_rel(efi_dir, grub::ENV_REL_PATH);
    if !env_target.is_file() {
        let existing = ["/boot/grub/grubenv", "/boot/grub2/grubenv"]
            .iter()
            .map(Path::new)
            .find(|path| path.is_file());
        let block = match existing.and_then(|path| fs::read(path).ok()) {
            // ext4 上那份是合法环境块就原样迁过去（保住当前的 saved_entry）
            Some(bytes) if grub::parse_env_block(&bytes).is_ok() => bytes,
            // 否则造一块干净的空块 —— GRUB 自己不会创建这个文件
            _ => grub::empty_env_block(),
        };
        write_esp_bytes(&env_target, &block)
            .map_err(|error| format!("在 ESP 上创建环境块失败: {error}"))?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn grub_collect(_efi_dir: &Path) -> Result<(), String> {
    // Windows / macOS 没有 /boot/grub 可搬：ESP 上的副本由 Linux 端负责更新
    Ok(())
}

/// GRUB 数据的一次读取结果（**不碰 AppState**）：GUI 与 CLI 共用同一份实现。
struct GrubScan {
    efi_dir: PathBuf,
    /// ESP 分区识别结果（是不是 FAT32 / fs-uuid 从哪来）
    esp: EspInfo,
    /// 我们自己挂上的挂载（用完要 `unmount`；用户已有的挂载是 None）
    mount: Option<ActiveMount>,
    entries: Vec<grub::GrubEntry>,
    /// submenu 分组的 (id, 标题) —— 前端做树形层级用
    groups: Vec<grub::GrubGroup>,
    env_path: PathBuf,
    cfg_path: PathBuf,
    env_exists: bool,
    saved_entry: Option<String>,
    next_entry: Option<String>,
    /// 这个目录是**按 fs-uuid 现找出来的**（不是用户填的那一行）——
    /// 调用方据此把找到的挂载点回填进设置，界面上「ESP 目录」那一行就会跟着变。
    discovered: bool,
}

impl GrubScan {
    fn release(self) {
        release_mount(self.mount);
    }
}

/// 把一次挂载交回去卸掉（`None` 就是什么都不做）。
fn release_mount(mount: Option<ActiveMount>) {
    if let Some(mount) = mount {
        let _ = mount.unmount();
    }
}

/// 决定这次该拿哪个挂载具柄：新挂的优先；否则复用上一次自己挂的那份
/// （**挂载点必须还是同一个**），其余的卸掉 —— 绝不能留下没人管得着的挂载。
fn merge_mount(
    fresh: Option<ActiveMount>,
    reusable: Option<ActiveMount>,
    dir: &Path,
) -> Option<ActiveMount> {
    match (fresh, reusable) {
        (Some(fresh), other) => {
            release_mount(other);
            Some(fresh)
        }
        (None, Some(old)) if old.mountpoint.as_path() == dir => Some(old),
        (None, other) => {
            release_mount(other);
            None
        }
    }
}

/// 按 fs-uuid 定位 ESP。
///
/// 顺序刻意是「**先看已经挂上的**，再考虑自己挂」：
/// 用户可能已经在系统里（或上次）把 ESP 挂好了 —— 那种情况下直接用它现成的挂载点，
/// 既不重复挂载，也顺手把这个路径回填到「ESP 目录」那一行给人看。
///
/// Linux 的 `grub_esp_dir` 本身就负责「没挂就挂上」；macOS 的只认已挂载的卷，
/// 所以这里给 macOS 补一次 `diskutil mount`（只在勾了「保持挂载」时做 ——
/// 用户没说要挂，就不该悄悄动他的磁盘）。
fn grub_locate(partuuid: &str, allow_mount: bool) -> Result<(PathBuf, Option<ActiveMount>), String> {
    // ① 已经挂着的：直接用（owned = false，不归我们管，也不会被我们卸掉）
    if let Some(dir) = find_mounted_by_partuuid(partuuid) {
        return Ok((dir, None));
    }
    // ② 没挂上：Linux 走 grub_esp_dir（会按需 mount），macOS 按需 diskutil mount
    match grub_esp_dir(partuuid) {
        Ok(found) => Ok(found),
        Err(error) => {
            if cfg!(target_os = "macos") && allow_mount {
                #[cfg(target_os = "macos")]
                match esp_mount(partuuid) {
                    Ok(Some(mount)) => return Ok((mount.mountpoint.clone(), Some(mount))),
                    Ok(None) => {}
                    Err(mount_error) => return Err(format!("{error}；{mount_error}")),
                }
            }
            Err(error)
        }
    }
}

/// 只读 ESP 上的 grub.cfg 与 grubenv。
///
/// * `Ok(Some(scan))` —— 定位到了 ESP（按分区 UUID，或按用户填的目录）
/// * `Ok(None)` —— 既没填分区 UUID 也没填目录，什么都不做
///
/// `reusable` 是上一次自己挂上的那份（勾了「保持挂载」才会传进来）。
/// `sync` 才会把 /boot/grub 的数据搬进 ESP（`grub_collect`）——**识别只解析，
/// 不写盘**（用户定案：同步只发生在启动自动链路和「同步」按钮，见 rescan_grub）。
fn grub_scan(
    settings: &Settings,
    reusable: Option<ActiveMount>,
    sync: bool,
) -> Result<Option<GrubScan>, String> {
    let uuid = settings.grub_efi_partuuid.trim().to_string();
    let dir = settings.grub_efi_dir.trim().to_string();
    if uuid.is_empty() && dir.is_empty() {
        release_mount(reusable);
        return Ok(None);
    }

    // ① 定位：填了分区 UUID 就按它找（先看已挂载的，再按需挂载）；只有目录就直接用目录
    let efi_dir_from_uuid = !uuid.is_empty();
    let located = if uuid.is_empty() {
        None
    } else {
        let Some(digits) = grub::partuuid_digits(&uuid) else {
            release_mount(reusable);
            return Err(format!(
                "分区 UUID 格式不对：{uuid}（GPT 分区 GUID，32 位标准 UUID，\
                 如 F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3）"
            ));
        };
        match grub_locate(&digits, settings.grub_keep_mounted) {
            Ok(found) => Some(found),
            Err(error) => {
                release_mount(reusable);
                return Err(error);
            }
        }
    };
    let (efi_dir, mount) = match located {
        Some((efi_dir, fresh)) => {
            let mount = merge_mount(fresh, reusable, &efi_dir);
            (efi_dir, mount)
        }
        None => {
            // 目录分支同样要按「同目录才复用，否则释放」收好手上的具柄 ——
            // 直接丢掉的话，勾了「保持挂载」的旧挂载会从此没人管得着。
            let mount = merge_mount(None, reusable, &PathBuf::from(&dir));
            (PathBuf::from(dir), mount)
        }
    };
    let discovered = efi_dir_from_uuid;

    // ② 挂上之后任何一步失败都要把挂载收掉，否则挂载点会越堆越多
    if sync {
        if let Err(error) = grub_collect(&efi_dir) {
            release_mount(mount);
            return Err(error);
        }
    }

    let cfg_path = grub_rel(&efi_dir, grub::CFG_REL_PATH);
    let env_path = grub_rel(&efi_dir, grub::ENV_REL_PATH);
    // grub.cfg 还没同步过来不算致命：先把状态摆出来，界面上会提示去点同步
    let cfg_text = fs::read_to_string(&cfg_path).ok();
    let raw_entries = cfg_text
        .as_deref()
        .map(grub::parse_grub_cfg)
        .unwrap_or_default();
    // 展示前折叠同名条目：os-prober 的 memtest86+（.bin/.efi）、31_efi_bootnext 的
    // `UEFI OS (EFI BootNext)`（0009/000A）标题逐字相同，铺两行只会让人以为重复了。
    let entries = grub::dedupe_display_entries(&raw_entries);
    let groups = cfg_text
        .as_deref()
        .map(grub::parse_submenus)
        .unwrap_or_default();
    let (env_exists, saved_entry, next_entry) = match fs::read(&env_path) {
        Ok(bytes) => (
            true,
            grub::env_value(&bytes, "saved_entry").filter(|value| !value.is_empty()),
            grub::env_value(&bytes, "next_entry").filter(|value| !value.is_empty()),
        ),
        Err(_) => (false, None, None),
    };

    // ③ 顺手把分区认一遍（探测失败**不算**致命：条目照样能用，只是分区信息缺一块）
    let mut esp = esp_probe(&efi_dir).unwrap_or_else(|error| EspInfo {
        path: efi_dir.display().to_string(),
        root: Some(efi_dir.display().to_string()),
        note: Some(error),
        ..EspInfo::default()
    });
    esp.owned_mount = mount.as_ref().is_some_and(|mount| mount.owned);

    Ok(Some(GrubScan {
        efi_dir,
        esp,
        mount,
        entries,
        groups,
        env_path,
        cfg_path,
        env_exists,
        saved_entry,
        next_entry,
        discovered,
    }))
}

/// 唯一的 GRUB 数据读取入口：只读 ESP 上的 grub.cfg 与 grubenv。
///
/// `sync=true` 才把 /boot/grub 的数据搬进 ESP。三条链路的分工（用户定案）：
/// * 启动/刷新/切换后（`refresh_grub`）：`(false, true)` —— 硬性条件满足才同步；
///   不满足时仍做一次「启动识别」（只解析 + 回填，UUID 优先、绝不挂载，
///   见 `startup_identify_scan`）；
/// * 识别（`detect_esp` / `detect_grub_by_partuuid`）与挂载勾选：`(true, false)`
///   —— 只解析，不写盘；
/// * 「同步」按钮（`sync_grub`）：`(true, true)` —— 自己先验挂载条件。
fn rescan_grub(app: &AppHandle, allow_mount: bool, sync: bool) -> Result<(), String> {
    let settings = stored_settings(app);
    // 上一次自己挂的那份：勾了「保持挂载」就留着复用（否则每次扫描都要重新
    // mount/umount 一遍，挂载点还会越堆越多）。
    let previous = {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state.grub.mount.take()
    };
    let reusable = if settings.grub_keep_mounted {
        previous
    } else {
        release_mount(previous);
        None
    };

    // 启动/刷新链路（`refresh_grub`，allow_mount=false）的硬性条件（用户定案）：
    // ESP 必须已经由**当前系统**挂载在 /boot 下，**同步**才进行 —— 否则不挂载、
    // 不写盘，绝不在后台偷偷 pkexec。但硬性条件不满足 ≠ 连解析都不做：填了
    // UUID 或目录就仍做一次「启动识别」（只解析 + 双向回填，UUID 优先，
    // 见 startup_identify_scan）。
    let scan = if !allow_mount && !grub_esp_ready(&settings) {
        match startup_identify_scan(app, &settings, reusable) {
            (Some(scan), _) => Some(scan),
            (None, reusable) => {
                if settings.grub_keep_mounted {
                    let state = app.state::<Mutex<AppState>>();
                    let mut state = state.lock().expect("state mutex poisoned");
                    state.grub.mount = reusable;
                }
                return Ok(());
            }
        }
    } else {
        grub_scan(&settings, reusable, sync)?
    };

    // 这次是「按 fs-uuid 现找出来的挂载点」——把它回填到设置里的 ESP 目录，
    // 界面上那一行就会显示真实位置（用户填了 fs-uuid 却看不到任何反馈，就是这个问题）。
    if let Some(scan) = &scan {
        if scan.discovered {
            let root = scan
                .esp
                .mount_point
                .clone()
                .or_else(|| scan.esp.root.clone())
                .unwrap_or_else(|| display_path(&scan.efi_dir).display().to_string());
            if settings.grub_efi_dir.trim() != root.trim() {
                let mut settings = stored_settings(app);
                settings.grub_efi_dir = root;
                let _ = save_settings(app, &settings);
            }
        }
    }

    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().expect("state mutex poisoned");
    state.grub = match scan {
        Some(scan) => GrubRuntime {
            efi_dir: Some(scan.efi_dir),
            esp: Some(scan.esp),
            mount: scan.mount,
            entries: scan.entries,
            groups: scan.groups,
            env_path: Some(scan.env_path),
            cfg_path: Some(scan.cfg_path),
            env_exists: scan.env_exists,
            saved_entry: scan.saved_entry,
            next_entry: scan.next_entry,
            installed: grub_installed(),
            synced_at: Some(grub_synced_stamp()),
            error: None,
        },
        None => GrubRuntime {
            installed: grub_installed(),
            ..GrubRuntime::default()
        },
    };
    Ok(())
}

/// 把一条 GRUB 侧的错误记进快照（保留「装没装 09_anyboot」这个仍可判定的信息）。
///
/// 注意别把挂载具柄顺手丢了：`GrubRuntime` 一被整体替换，旧的 `mount` 就没人管
/// 得着了（`ActiveMount` 没有 Drop），挂载点会一直留在那里。
fn apply_grub_error(app: &AppHandle, error: &str) {
    let keep = stored_settings(app).grub_keep_mounted;
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().expect("state mutex poisoned");
    let mount = state.grub.mount.take();
    let mount = if keep {
        mount
    } else {
        release_mount(mount);
        None
    };
    state.grub = GrubRuntime {
        installed: grub_installed(),
        mount,
        error: Some(error.to_string()),
        ..GrubRuntime::default()
    };
}

/// 重新解析 GRUB 数据，失败只记进快照（不打断 rEFInd 那条链路）。
///
/// **不发布**快照：调用它的 `refresh_and_update` 末尾会统一发布。
fn refresh_grub(app: &AppHandle) {
    // 自动链路：硬性条件（/boot 下已挂）满足才同步；识别链路不同步，见 rescan_grub。
    if let Err(error) = rescan_grub(app, false, true) {
        apply_grub_error(app, &error);
    }
}

/// 组装主窗口要的 GRUB 视图。
fn grub_snapshot(app: &AppHandle, settings: &Settings) -> GrubView {
    let (runtime, script_log) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (state.grub.clone(), state.script_log.clone())
    };
    let uuid = settings.grub_efi_partuuid.trim().to_string();
    let hint = if uuid.is_empty() {
        Some(
            "填 ESP 目录后点「识别」（自动判断是不是 FAT32 并取出分区 UUID），\
             或者直接手工填分区 UUID（三端都报同一个值：Linux lsblk -o PARTUUID / \
             macOS diskutil 的 Disk / Partition UUID / Windows Get-Partition 的 Guid）。\
             目录和 UUID 填一个即可，识别后二者双向同步"
                .to_string(),
        )
    } else if cfg!(target_os = "linux") {
        Some(
            "执行过 update-grub / grub-mkconfig 之后，点一次「同步」把新 grub.cfg \
             搬进 ESP（需已把该分区挂载到 /boot 或 /boot/efi）"
                .to_string(),
        )
    } else {
        Some("列表读的是 ESP 上的 grub.cfg 副本；Linux 端更新过 GRUB 后需重新同步".to_string())
    };
    GrubView {
        partuuid: uuid,
        dir: settings.grub_efi_dir.trim().to_string(),
        esp: runtime.esp.clone(),
        keep_mounted: settings.grub_keep_mounted,
        mount_supported: esp_mount_supported(),
        efi_dir: runtime
            .efi_dir
            .as_deref()
            .map(|path| display_path(path).display().to_string()),
        env_path: runtime
            .env_path
            .as_deref()
            .map(|path| display_path(path).display().to_string()),
        cfg_path: runtime
            .cfg_path
            .as_deref()
            .map(|path| display_path(path).display().to_string()),
        env_exists: runtime.env_exists,
        installed: runtime.installed,
        synced_at: runtime.synced_at,
        script_log,
        entries: runtime.entries,
        groups: runtime.groups,
        list_format: settings.grub_list_format.clone(),
        saved_entry: runtime.saved_entry,
        next_entry: runtime.next_entry,
        error: runtime.error,
        hint,
    }
}

/// 把 selector 写进指定的环境块文件（**不碰 AppState**，GUI 与 CLI 共用）。
///
/// 内容规则完全沿用原脚本：写 `saved_entry`（永久默认）**和** `next_entry`
/// （一次性），值取 grub.cfg 里解析出来的原文 —— **绝不自己拼 id**。
fn grub_write_env(env_path: &Path, selector: &str) -> Result<(), String> {
    let existing = fs::read(env_path).unwrap_or_else(|_| grub::empty_env_block());
    let updates = vec![
        ("saved_entry".to_string(), selector.to_string()),
        ("next_entry".to_string(), selector.to_string()),
    ];
    let block = grub::set_env_vars(&existing, &updates)?;
    write_esp_bytes(env_path, &block).map_err(|error| format!("写入环境块失败: {error}"))?;
    // 回读校验：写不进去必须当场发现，而不是等到开机才报 invalid environment block
    let written = fs::read(env_path).map_err(|error| format!("回读环境块失败: {error}"))?;
    if grub::env_value(&written, "next_entry").as_deref() != Some(selector) {
        return Err("环境块写入校验失败（回读的 next_entry 与预期不一致）".to_string());
    }
    Ok(())
}

/// 把用户点选的条目写进 ESP 上的环境块（先校验它在当前 grub.cfg 里确实存在）。
fn write_grub_entry(app: &AppHandle, selector: &str) -> Result<(), String> {
    let env_path = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        let env_path = state
            .grub
            .env_path
            .clone()
            .ok_or_else(|| "GRUB 环境块路径尚未解析出来".to_string())?;
        if !state
            .grub
            .entries
            .iter()
            .any(|entry| entry.selector == selector)
        {
            return Err(format!("grub.cfg 里没有这一项：{selector}"));
        }
        env_path
    };
    grub_write_env(&env_path, selector)
}

/// 切换下次启动的 GRUB 条目。只写环境块，**不重启**（与 rEFInd 那条链路一致）。
fn switch_grub_and_update(app: &AppHandle, selector: &str) -> AppStatus {
    trace(app, "grub", &format!("切换 entry={selector}"));
    set_busy(app, |busy| busy.switching = true);
    let outcome = write_grub_entry(app, selector);
    set_busy(app, |busy| busy.switching = false);

    match outcome {
        Ok(()) => {
            refresh_grub(app);
            let message = format!("下次启动：{selector}");
            record_action(app, "切换 GRUB 启动项", true, &message);
            publish_ui(app);
            state_snapshot(app)
        }
        Err(error) => {
            record_action(app, "切换 GRUB 启动项", false, &error);
            update_failure_status(app, "切换 GRUB 启动项失败", error)
        }
    }
}

// ---------------------------------------------------------------------------
// Linux 专属：安装 / 卸载 09_anyboot
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn binary_path(program: &str) -> Option<PathBuf> {
    ["/usr/bin", "/usr/sbin", "/bin", "/sbin"]
        .iter()
        .map(|dir| Path::new(dir).join(program))
        .find(|path| path.is_file())
}

/// 重新生成 grub.cfg：发行版之间命令名与输出路径都不一样（用户定案 8）。
#[cfg(target_os = "linux")]
fn grub_run_mkconfig(log: &mut Vec<String>) -> Result<(), String> {
    let candidates = [
        ("grub-mkconfig", "/boot/grub/grub.cfg"),
        ("grub2-mkconfig", "/boot/grub2/grub.cfg"),
    ];
    let mut last_error = "没找到 grub-mkconfig / grub2-mkconfig".to_string();
    for (program, output) in candidates {
        if binary_path(program).is_none() {
            script_log_push(log, &format!("跳过 {program}：系统里没有这个命令"));
            continue;
        }
        script_log_push(
            log,
            &format!("运行 {program} -o {output}（非 root 会经 pkexec 提权）"),
        );
        let result = mount_command(program).args(["-o", output]).output();
        match &result {
            Ok(result) if result.status.success() => {
                script_log_output(log, program, result);
                script_log_push(log, &format!("→ 已重新生成 {output}"));
                return Ok(());
            }
            Ok(result) => {
                script_log_output(log, program, result);
                last_error = format!(
                    "{program} 失败: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                )
            }
            Err(error) => {
                script_log_error(log, program, error);
                last_error = format!("无法执行 {program}: {error}");
            }
        }
    }
    Err(last_error)
}

/// Linux：按分区 UUID 查那块分区的 fs-uuid（lsblk 的 UUID 列，udev 数据库，
/// 普通用户可读）。GRUB 的 `search --fs-uuid` 只认 fs-uuid，所以 09_anyboot
/// 写盘前要从识别键（分区 UUID）现场解析一次。
#[cfg(target_os = "linux")]
fn linux_fsuuid_of_partuuid(partuuid: &str) -> Option<String> {
    let output = command("lsblk")
        .args(["--json", "--output", "UUID,PARTUUID"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let devices: LsblkOutput = serde_json::from_slice(&output.stdout).ok()?;
    devices
        .blockdevices
        .into_iter()
        .find(|device| {
            device
                .partuuid
                .as_deref()
                .is_some_and(|value| grub::uuid_matches(value, partuuid))
        })?
        .uuid
        .filter(|value| grub::uuid_digits(value).is_some())
}

#[cfg(target_os = "linux")]
fn grub_install_script(app: &AppHandle, log: &mut Vec<String>) -> Result<String, String> {
    let settings = stored_settings(app);
    let Some(partuuid) = grub::partuuid_digits(&settings.grub_efi_partuuid) else {
        script_log_push(log, "缺少 ESP 的分区 UUID，安装中止");
        return Err("请先填写并保存 ESP 的分区 UUID".to_string());
    };
    let Some(fsuuid) = linux_fsuuid_of_partuuid(&partuuid) else {
        script_log_push(log, "从分区 UUID 解析不出 fs-uuid（lsblk 没报这块分区的 UUID），安装中止");
        return Err(
            "从分区 UUID 解析不出 fs-uuid —— GRUB 的 search --fs-uuid 只认 fs-uuid；\
             确认这块分区还在（lsblk 里 PARTUUID 对得上且 UUID 列有值）"
                .to_string(),
        );
    };
    let content = grub_script_content(grub_template(&settings), &grub::uuid_pretty(&fsuuid));
    script_log_push(
        log,
        &format!(
            "准备安装 {GRUB_SCRIPT_PATH}（把 @@ESP_UUID@@ 换成 {}，\
             由分区 UUID {} 解析而来）",
            grub::uuid_pretty(&fsuuid),
            grub::uuid_pretty(&partuuid)
        ),
    );

    // 用 install(1) 落盘：/etc/grub.d 需要 root，非 root 时 mount_command 会套 pkexec
    let staging = std::env::temp_dir().join("anyboot-09_anyboot");
    if let Err(error) = fs::write(&staging, content.as_bytes()) {
        script_log_push(log, &format!("暂存脚本失败: {error}"));
        return Err(format!("暂存脚本失败: {error}"));
    }
    script_log_push(log, &format!("写入临时文件 {}", staging.display()));
    script_log_push(
        log,
        &format!("运行 install -m 755 <暂存文件> {GRUB_SCRIPT_PATH}（非 root 会经 pkexec 提权）"),
    );
    let result = mount_command("install")
        .args(["-m", "755"])
        .arg(&staging)
        .arg(GRUB_SCRIPT_PATH)
        .output();
    match &result {
        Ok(output) => script_log_output(log, "install", output),
        Err(error) => script_log_error(log, "install", error),
    }
    let _ = fs::remove_file(&staging);
    match result {
        Ok(output) if output.status.success() => {
            script_log_push(log, &format!("→ 已写入 {GRUB_SCRIPT_PATH}"));
        }
        Ok(_) => {
            return Err("安装 09_anyboot 失败：见上方执行过程".to_string());
        }
        Err(error) => return Err(format!("无法执行 install: {error}")),
    }
    grub_run_mkconfig(log)?;
    Ok(format!(
        "已安装 {GRUB_SCRIPT_PATH} 并重新生成 grub.cfg（fs-uuid = {}）",
        grub::uuid_pretty(&fsuuid)
    ))
}

#[cfg(target_os = "linux")]
fn grub_uninstall_script(log: &mut Vec<String>) -> Result<String, String> {
    let mut removed = false;
    if Path::new(GRUB_SCRIPT_PATH).exists() {
        script_log_push(
            log,
            &format!("运行 rm -f {GRUB_SCRIPT_PATH}（非 root 会经 pkexec 提权）"),
        );
        let output = mount_command("rm").args(["-f", GRUB_SCRIPT_PATH]).output();
        match &output {
            Ok(result) => script_log_output(log, "rm", result),
            Err(error) => script_log_error(log, "rm", error),
        }
        let output = output.map_err(|error| format!("无法执行 rm: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "删除 09_anyboot 失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        script_log_push(log, &format!("→ 已删除 {GRUB_SCRIPT_PATH}"));
        removed = true;
    } else {
        script_log_push(log, &format!("{GRUB_SCRIPT_PATH} 不存在，跳过删除"));
    }
    grub_run_mkconfig(log)?;
    Ok(if removed {
        format!("已删除 {GRUB_SCRIPT_PATH} 并重新生成 grub.cfg")
    } else {
        "09_anyboot 本来就没装；已重新生成 grub.cfg".to_string()
    })
}

/// 非 Linux 上也保留同样的签名（`log` 只用来记一句「不支持」）。
#[cfg(not(target_os = "linux"))]
fn grub_install_script(_app: &AppHandle, log: &mut Vec<String>) -> Result<String, String> {
    script_log_push(log, "安装 09_anyboot 只能在 Linux 上进行");
    Err("安装 09_anyboot 只能在 Linux 上进行".to_string())
}

#[cfg(not(target_os = "linux"))]
fn grub_uninstall_script(log: &mut Vec<String>) -> Result<String, String> {
    script_log_push(log, "卸载 09_anyboot 只能在 Linux 上进行");
    Err("卸载 09_anyboot 只能在 Linux 上进行".to_string())
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
    // 本机入口：会先作废挂着的远程请求，别让它到点把本机选择盖回去
    switch_system_locally(&app, &system)
}

// ---- GRUB 细粒度切换的命令面 ----

/// 保存 ESP 的**分区 UUID**。填了它，主窗口才会出现「GRUB 启动项」区块。
#[tauri::command]
fn set_grub_partuuid(app: AppHandle, partuuid: String) -> Result<(), String> {
    trace(&app, "command", &format!("set_grub_partuuid partuuid={partuuid}"));
    let trimmed = partuuid.trim().to_string();
    if !trimmed.is_empty() && grub::partuuid_digits(&trimmed).is_none() {
        return Err(format!(
            "分区 UUID 不像有效值：{trimmed}（GPT 分区 GUID，32 位标准 UUID，\
             如 F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3）"
        ));
    }
    let mut settings = stored_settings(&app);
    settings.grub_efi_partuuid = trimmed;
    save_settings(&app, &settings)?;
    refresh_grub(&app);
    publish_ui(&app);
    Ok(())
}

/// 保存用户改过的 09_anyboot 正文；传空串 = 恢复内置模板。
/// 保存用户填的 ESP 目录**原文**；传空串 = 清空（分区 UUID 不动，它才是权威键）。
///
/// 与「识别」（detect_esp）的区别：这里不做 FAT32 校验、不取分区 UUID，
/// 纯落盘 —— 用户就是想改目录原文（包括清成空），识别才是校验+取键的入口。
#[tauri::command]
fn set_grub_dir(app: AppHandle, dir: String) -> Result<(), String> {
    trace(&app, "command", &format!("set_grub_dir dir={dir}"));
    let mut settings = stored_settings(&app);
    settings.grub_efi_dir = dir.trim().to_string();
    save_settings(&app, &settings)?;
    refresh_grub(&app);
    publish_ui(&app);
    Ok(())
}

#[tauri::command]
fn set_grub_script(app: AppHandle, script: String) -> Result<(), String> {
    trace(&app, "command", "set_grub_script");
    let mut settings = stored_settings(&app);
    settings.grub_script_override = script;
    save_settings(&app, &settings)?;
    publish_ui(&app);
    Ok(())
}

/// 呼出**系统原生文件夹选择框**，让用户挑 ESP 分区里的目录去识别。
///
/// 返回选中的路径（取消返回 None）；识别本身走同一条 `detect_esp` ——
/// 成功后分区 UUID 与 ESP 目录一起落盘，「选文件夹」和「填分区 UUID」两个
/// 入口自动对上（分区 UUID 是权威键，目录只是入口）。
///
/// 走 tauri-plugin-dialog 的 Rust API（macOS NSOpenPanel / Windows
/// 文件夹对话框 / Linux GTK 选择器），不暴露前端 JS 插件层，所以也不需要
/// capabilities 权限配置。回调是异步回来的：阻塞等结果的活儿丢进
/// spawn_blocking，别占 async runtime 的工作线程。
#[tauri::command]
async fn pick_grub_dir(app: AppHandle) -> Result<Option<String>, String> {
    trace(&app, "command", "pick_grub_dir");
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        app.dialog()
            .file()
            .set_title("选择 ESP 分区里的文件夹")
            .pick_folder(move |path| {
                let picked = path.and_then(|p| p.into_path().ok()).map(|p| {
                    display_path(&p).display().to_string()
                });
                let _ = tx.send(picked);
            });
        rx.recv().map_err(|_| "文件夹选择框被关闭".to_string())
    })
    .await
    .map_err(|error| format!("文件夹选择框执行失败: {error}"))?
}

/// 识别 ESP 分区：判断是不是 FAT32，并把分区 UUID（识别键）填进设置。
///
/// 填目录是**可选的入口**：分区 UUID 才是权威键。识别成功后存的是**分区根目录**，
/// 所以用户填 `H:\EFI` 或 `/boot/efi/EFI/grub.cfg` 都会归一化成 `H:\` / `/boot/efi`。
#[tauri::command]
fn detect_esp(app: AppHandle, path: String) -> Result<String, String> {
    let raw = path.trim().to_string();
    trace(&app, "command", &format!("detect_esp path={raw}"));
    if raw.is_empty() {
        return Err("先填 ESP 分区里的任意路径（例如 H:\\ 或 /boot/efi）".to_string());
    }
    let info = esp_probe(Path::new(&raw))?;
    let fstype = info.fstype.clone().unwrap_or_else(|| "未知".to_string());
    // 只认 FAT 家族：ESP 按规范就是 FAT32，别让人指到数据分区上 ——
    // 一旦指错，同步会把 /EFI/grub.cfg 写到那块分区里去。
    if !info.fat {
        return Err(format!(
            "{raw} 是 {fstype}，不是 FAT32 —— ESP 必须是 FAT32，请指到 EFI 系统分区上"
        ));
    }
    let root = info.root.clone().unwrap_or_else(|| raw.clone());
    let mut settings = stored_settings(&app);
    settings.grub_efi_dir = root.clone();
    let message = match &info.partuuid {
        Some(partuuid) => {
            settings.grub_efi_partuuid = partuuid.clone();
            match &info.uuid {
                Some(fsuuid) => format!(
                    "{root} 是 {fstype}，分区 UUID = {partuuid}，fs-uuid = {fsuuid}（已填好）"
                ),
                None => format!("{root} 是 {fstype}，分区 UUID = {partuuid}（已填好）"),
            }
        }
        None => format!(
            "{root} 是 {fstype}，但没取出分区 UUID —— {}",
            info.note
                .clone()
                .unwrap_or_else(|| "请手工填写".to_string())
        ),
    };
    save_settings(&app, &settings)?;

    // 识别只解析、不写盘（用户定案）：同步只属于「同步」按钮和启动自动链路。
    // 解析顺带把「目录下有没有 grub.cfg」反馈给用户，别让人对空列表发懵。
    if let Err(error) = rescan_grub(&app, true, false) {
        apply_grub_error(&app, &error);
    }
    let message = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        let parsed = state
            .grub
            .cfg_path
            .as_ref()
            .is_some_and(|path| path.is_file());
        if parsed {
            format!(
                "{message}；已解析 grub.cfg（{} 个启动项）。目录和 UUID 填一个即可，二者已双向同步",
                state.grub.entries.len()
            )
        } else {
            format!(
                "{message}；该分区上还没有 grub.cfg —— 点「同步」可从当前系统 /boot/grub 搬一份过来。\
                 目录和 UUID 填一个即可，二者已双向同步"
            )
        }
    };
    record_action(&app, "识别 ESP 分区", true, &message);
    publish_ui(&app);
    Ok(message)
}

/// 按分区 UUID 识别（UUID 行的「识别」按钮）：没挂载就先按需挂载 ——
/// 手动按钮，pkexec 授权是明说的行为（`grub_locate`：先看已挂的，没挂自己挂）。
/// 解析结果（挂载点）经由 `discovered` 回填进设置里的「ESP 目录」并保存；
/// 与目录识别同一条解析链路（sync=false），**识别不同步**。
#[tauri::command]
fn detect_grub_by_partuuid(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "detect_grub_by_partuuid");
    let mut settings = stored_settings(&app);
    let uuid = settings.grub_efi_partuuid.trim().to_string();
    if uuid.is_empty() {
        return Err("先填 ESP 的分区 UUID（或填目录点「识别」自动取）".to_string());
    }
    // 格式校验用归一化 digits，落盘存**用户填的原文**（trim 过）—— 否则识别
    // 一次就把 B3FBE669-D3B9-… 改写成 32 位无连字符全大写，看着像被改了值。
    // 匹配环节（uuid_matches / partuuid_digits）两边都归一化，原格式照样命中。
    if grub::partuuid_digits(&uuid).is_none() {
        return Err(format!(
            "分区 UUID 格式不对：{uuid}（GPT 分区 GUID，32 位标准 UUID，\
             如 F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3）"
        ));
    }
    settings.grub_efi_partuuid = uuid;
    save_settings(&app, &settings)?;

    if let Err(error) = rescan_grub(&app, true, false) {
        apply_grub_error(&app, &error);
        record_action(&app, "识别 ESP 分区", false, &error);
        publish_ui(&app);
        return Err(error);
    }
    let message = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        let dir = state
            .grub
            .efi_dir
            .as_ref()
            .map(|path| display_path(path).display().to_string())
            .unwrap_or_default();
        let parsed = state
            .grub
            .cfg_path
            .as_ref()
            .is_some_and(|path| path.is_file());
        if parsed {
            format!(
                "{dir}：已解析 grub.cfg（{} 个启动项），挂载点已回填到「ESP 目录」。\
                 目录和 UUID 填一个即可，二者已双向同步",
                state.grub.entries.len()
            )
        } else if dir.is_empty() {
            "按分区 UUID 识别完成，但还没定位到挂载点".to_string()
        } else {
            format!(
                "{dir}：分区上还没有 grub.cfg —— 点「同步」可从当前系统 /boot/grub 搬一份过来。\
                 目录和 UUID 填一个即可，二者已双向同步"
            )
        }
    };
    record_action(&app, "识别 ESP 分区", true, &message);
    publish_ui(&app);
    Ok(message)
}

/// 勾选「挂载 ESP」：勾上就按分区 UUID 挂上来并保持，取消就卸掉**自己挂的那份**。
///
/// 用户自己挂的（`owned = false`）绝不去卸 —— 那是别人的挂载。
#[tauri::command]
fn set_grub_mount(app: AppHandle, enabled: bool) -> Result<(), String> {
    trace(
        &app,
        "command",
        &format!("set_grub_mount enabled={enabled}"),
    );
    if enabled && !esp_mount_supported() {
        return Err(
            "本端不能按分区 UUID 挂载：请先在「磁盘管理」里给这个分区分配盘符，再填盘符路径"
                .to_string(),
        );
    }
    let mut settings = stored_settings(&app);
    if enabled && grub::partuuid_digits(&settings.grub_efi_partuuid).is_none() {
        return Err("先填好 ESP 的分区 UUID（或在上面填目录点「识别」自动取）".to_string());
    }
    settings.grub_keep_mounted = enabled;
    save_settings(&app, &settings)?;

    if !enabled {
        let mount = {
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            state.grub.mount.take()
        };
        release_mount(mount);
    }

    // 勾上 = 走一次扫描（里面会按分区 UUID 挂载并把具柄留下）；取消 = 上面已经卸了
    if enabled {
        set_busy(&app, |busy| busy.scanning = true);
        // 挂载勾选只保证「挂上 + 解析」，不同步 —— 同步是「同步」按钮的职责。
        let outcome = rescan_grub(&app, true, false);
        set_busy(&app, |busy| busy.scanning = false);
        if let Err(error) = outcome {
            // 挂不上（比如 pkexec 被取消）—— 把勾选退回未勾，别让界面骗人
            let mut settings = stored_settings(&app);
            settings.grub_keep_mounted = false;
            let _ = save_settings(&app, &settings);
            apply_grub_error(&app, &error);
            record_action(&app, "挂载 ESP", false, &error);
            publish_ui(&app);
            return Err(error);
        }
    }

    let message = if enabled {
        "ESP 已挂载，可以直接读写（取消勾选即卸载）"
    } else {
        "已卸载本程序挂上的 ESP"
    };
    record_action(&app, "挂载 ESP", true, message);
    publish_ui(&app);
    Ok(())
}

/// 用系统文件管理器打开 ESP 目录（三端都走同一条 `open_directory_in_explorer`）。
///
/// 解析顺序：内存里刚扫出来的目录 → 用户填的分区 UUID 现找（**含已挂载的**）→
/// 用户填的目录原文。都不行才报错，并把「该怎么办」说清楚。
#[tauri::command]
fn open_grub_dir(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "open_grub_dir");
    let resolved = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        state.grub.efi_dir.clone()
    };
    let settings = stored_settings(&app);

    // 已经解析出来、而且真的能打开，就直接用
    if let Some(dir) = resolved.filter(|dir| dir.is_dir()) {
        open_directory_in_explorer(&dir)?;
        return Ok(display_path(&dir).display().to_string());
    }

    // 内存里没有：按用户填的分区 UUID 现找一次（已挂载的优先）
    if let Some(digits) = grub::partuuid_digits(&settings.grub_efi_partuuid) {
        if let Ok((dir, mount)) = grub_locate(&digits, settings.grub_keep_mounted) {
            let result = open_directory_in_explorer(&dir);
            release_mount(mount);
            result?;
            return Ok(display_path(&dir).display().to_string());
        }
    }

    // 退到用户填的目录原文
    let raw = settings.grub_efi_dir.trim();
    if !raw.is_empty() {
        let dir = PathBuf::from(raw);
        if dir.is_dir() {
            open_directory_in_explorer(&dir)?;
            return Ok(display_path(&dir).display().to_string());
        }
        return Err(format!(
            "{raw} 现在打不开（ESP 没挂上？先填分区 UUID 或在上面填目录后点「识别」）"
        ));
    }

    Err("还不知道 ESP 在哪儿：先填分区 UUID，或在上面填目录后点「识别」".to_string())
}

/// 「同步」按钮：把当前系统 /boot/grub（或 /boot/grub2）的 grub.cfg 覆盖搬进
/// ESP，grubenv 没有才搬。硬性条件：该分区已挂载到 /boot 或 /boot/efi，
/// 否则不动（不自动挂载、不 pkexec、不写盘）。
#[tauri::command]
fn sync_grub(app: AppHandle) -> Result<(), String> {
    trace(&app, "command", "sync_grub");
    let settings = stored_settings(&app);
    let uuid = settings.grub_efi_partuuid.trim();
    let dir = settings.grub_efi_dir.trim();
    if uuid.is_empty() && dir.is_empty() {
        return Err("先识别 ESP：填好分区 UUID 或目录，点「识别」".to_string());
    }
    // 硬性条件（用户定案）：当前系统把这个分区挂载到了 /boot 或 /boot/efi 才同步；
    // 否则什么都不动 —— 不自动挂载、不 pkexec、不写盘。填了 UUID 就按 UUID 找
    // 挂载点（lsblk），只有目录就按 findmnt 报的挂载点判。
    #[cfg(target_os = "linux")]
    {
        let mounted_ok = if !uuid.is_empty() {
            let Some(digits) = grub::partuuid_digits(uuid) else {
                return Err(format!(
                    "分区 UUID 格式不对：{uuid}（GPT 分区 GUID，32 位标准 UUID）"
                ));
            };
            find_mounted_by_partuuid(&digits)
                .and_then(|mount| mount.into_os_string().into_string().ok())
                .is_some_and(|mount| sync_mount_ok(&mount))
        } else {
            esp_probe(Path::new(dir))
                .ok()
                .and_then(|info| info.mount_point)
                .is_some_and(|mount| sync_mount_ok(&mount))
        };
        if !mounted_ok {
            let error = "当前系统没有把该 ESP 分区挂载到 /boot 或 /boot/efi，不进行同步 \
                         —— 请先挂载或配置 fstab（同步不会自动挂载）";
            record_action(&app, "同步 GRUB 数据", false, error);
            publish_ui(&app);
            return Err(error.to_string());
        }
    }
    set_busy(&app, |busy| busy.scanning = true);
    let outcome = rescan_grub(&app, true, true);
    if let Err(error) = &outcome {
        apply_grub_error(&app, error);
    }
    set_busy(&app, |busy| busy.scanning = false);
    match outcome {
        Ok(()) => {
            let (count, stamp) = {
                let state = app.state::<Mutex<AppState>>();
                let state = state.lock().expect("state mutex poisoned");
                (state.grub.entries.len(), state.grub.synced_at.clone())
            };
            let message = match stamp {
                Some(stamp) => format!("已同步 {count} 个启动项（{stamp}）"),
                None => format!("已同步 {count} 个启动项"),
            };
            record_action(&app, "同步 GRUB 数据", true, &message);
            publish_ui(&app);
            Ok(())
        }
        Err(error) => {
            record_action(&app, "同步 GRUB 数据", false, &error);
            publish_ui(&app);
            Err(error)
        }
    }
}

/// 切换下次启动的 GRUB 条目（只写环境块，不重启）。
#[tauri::command]
fn switch_grub(app: AppHandle, selector: String) -> AppStatus {
    trace(&app, "command", &format!("switch_grub selector={selector}"));
    switch_grub_and_update(&app, &selector)
}

/// 安装 09_anyboot + 重新生成 grub.cfg（仅 Linux）。
#[tauri::command]
fn install_grub_script(app: AppHandle) -> Result<(), String> {
    trace(&app, "command", "install_grub_script");
    let mut log = Vec::new();
    let outcome = grub_install_script(&app, &mut log);
    let title = "安装 09_anyboot";
    let result = match &outcome {
        Ok(message) => {
            record_action(&app, title, true, message);
            Ok(())
        }
        Err(error) => {
            record_action(&app, title, false, error);
            Err(error.clone())
        }
    };
    finish_script_log(&app, title, log, result.is_ok());
    if result.is_ok() {
        refresh_grub(&app);
    }
    publish_ui(&app);
    result
}

/// 卸载 09_anyboot + 重新生成 grub.cfg（仅 Linux）。
/// 按用户定案：只删脚本、不把环境块迁回 ext4。
#[tauri::command]
fn uninstall_grub_script(app: AppHandle) -> Result<(), String> {
    trace(&app, "command", "uninstall_grub_script");
    let mut log = Vec::new();
    let outcome = grub_uninstall_script(&mut log);
    let title = "卸载 09_anyboot";
    let result = match &outcome {
        Ok(message) => {
            record_action(&app, title, true, message);
            Ok(())
        }
        Err(error) => {
            record_action(&app, title, false, error);
            Err(error.clone())
        }
    };
    finish_script_log(&app, title, log, result.is_ok());
    if result.is_ok() {
        refresh_grub(&app);
    }
    publish_ui(&app);
    result
}

/// 把一次安装 / 卸载的执行过程落进快照（`ok = None` 表示还没跑过）。
fn finish_script_log(app: &AppHandle, title: &str, lines: Vec<String>, ok: bool) {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().expect("state mutex poisoned");
    state.script_log = ScriptLogView {
        title: Some(title.to_string()),
        lines,
        ok: Some(ok),
    };
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

/// 重新生成 token：旧 token 立即失效（**关掉旧监听、按新 token 重建**）。
#[tauri::command]
fn regenerate_http_token(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "regenerate_http_token（主窗口）");
    let token = generate_token();
    let replace = token.clone();
    update_settings_and_restart_http(&app, "访问令牌", move |settings| {
        settings.http_token = replace;
    })?;
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

/// 主窗口「打开 Web 页面」：用系统默认浏览器打开带令牌的控制页。
/// 返回真正的地址，界面直接回显，省得用户猜开到了哪。
#[tauri::command]
fn open_web_page(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "open_web_page（系统浏览器）");
    open_web_page_now(&app)
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

/// 设置访问令牌（自定义 token）：**关掉旧监听 → 按新 token 重新绑定端口**，新 token 立即生效。
#[tauri::command]
fn set_http_token(app: AppHandle, token: String) -> Result<(), String> {
    trace(&app, "command", "set_http_token（主窗口）");
    let token = validate_http_token(&token)?;
    // 状态文案由重启后的实况决定（「监听已重启：0.0.0.0:12300」/ 失败原因），
    // 而不是提前写一句「已重启」的漂亮话。
    update_settings_and_restart_http(&app, "访问令牌", |settings| settings.http_token = token)?;
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

/// 设置窗口不透明度（百分比，0–100；0 = 全透明但窗口还在）
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

/// 「重启程序」（主窗口顶部标题栏按钮）：拉起一份新实例后本进程退出。
///
/// 为什么先停 HTTP 服务再重启：`AppHandle::restart` 是「先 spawn 新进程、再
/// exit(0)」，新实例起来的那一刻旧进程可能还攥着监听端口 → 新实例第一屏就是
/// 「端口被本应用的另一个实例占用」的红字。`stop_http_server` 返回时端口一定
/// 已放开，所以先让出来再拉起。
///
/// 单实例锁（`release_instance_lock`）同理必须先放：不然新进程会把自己当成
/// 「第二个实例」，提示一句「已经启动」就退出 —— 点一次重启应用直接没了。
///
/// AppImage 下 tauri 用 `APPIMAGE` 环境变量指回 **.AppImage 文件本身**（而不是挂载
/// 点里的那份二进制）：否则旧进程一退出、挂载点被卸掉，新进程就成了无源之水
/// （见 tauri::process::current_binary）。
#[tauri::command]
fn restart_app(app: AppHandle) -> Result<(), String> {
    trace(&app, "command", "restart_app（主窗口）");
    startup_debug(
        &app,
        "用户点「重启程序」：先停 HTTP 监听（放开端口）并放掉单实例锁，再拉起新实例并退出本进程",
    );
    stop_http_server(&app);
    // 单实例锁必须在拉起新进程**之前**放掉：tauri 是先 spawn 再 exit(0)，
    // 新进程会立刻来抢这把锁（与放开端口同一个道理，见上方注释）。
    release_instance_lock();
    app.restart();
}

/// 「退出程序」（主窗口顶部标题栏按钮）：与托盘菜单里的「退出程序」同一条路。
#[tauri::command]
fn quit_app(app: AppHandle) -> Result<(), String> {
    trace(&app, "command", "quit_app（主窗口）");
    startup_debug(&app, "用户点「退出程序」（主窗口）");
    app.exit(0);
    Ok(())
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
    let mut opener = command(if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    opener.arg(dir);
    // explorer.exe 打开一个目录后**正常也会返回 1**（这是它的老毛病，不是失败），
    // 所以只看「进程有没有起来」；起不来才算真失败。
    let status = opener
        .status()
        .map_err(|error| format!("打开文件管理器失败：{error}"))?;
    if cfg!(target_os = "windows") {
        return Ok(());
    }
    if !status.success() {
        return Err(format!("打开文件管理器失败（exit code {status}）"));
    }
    Ok(())
}

/// 跨平台：用系统默认浏览器打开 URL。
///
/// Windows 走 `ShellExecuteW` —— 这是系统「用默认程序打开某物」的标准入口
/// （资源管理器双击、Office 打开链接都走它），对 URL 会直接交给默认浏览器。
/// 不用 `explorer.exe <url>`：它会把带 query 的地址当文件/文件夹路径解析，
/// 常常什么都不打开；而且一旦弹出错误对话框，explorer 进程会一直不退出，
/// 用 `.status()` 同步等它就把整个命令挂死（按钮停在禁用态，看着就是「没反应」）。
/// `ShellExecuteW` 是即发即返回，不阻塞。其余平台用 xdg-open / open，spawn 后不等。
pub(crate) fn open_url_in_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        fn wide(value: &str) -> Vec<u16> {
            std::ffi::OsStr::new(value)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        }

        let operation = wide("open");
        let file = wide(url);
        // SAFETY: 两个宽字符串由上面的 Vec 持有、以 NUL 结尾，在本调用期间一直存活；
        // 其余参数按 API 约定传 null / 默认显示方式均合法。
        let handle = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecuteW 的返回值不是真句柄：<= 32 表示失败（0/2/3/5/... 各有含义），
        // > 32 才算成功。所以不能用「非空」判断。
        let code = handle as isize;
        if code <= 32 {
            return Err(format!(
                "打开默认浏览器失败：ShellExecute 返回 {code}（常见 2=找不到文件，5=被安全软件/策略拒绝）"
            ));
        }
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let program = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        // spawn 而非 status：浏览器进程可能长期驻留，等它退出没有意义。
        command(program)
            .arg(url)
            .spawn()
            .map_err(|error| format!("打开浏览器失败：{error}"))?;
        Ok(())
    }
}

#[tauri::command]
fn confirm_pending(app: AppHandle) -> Result<(), String> {
    trace(&app, "command", "confirm_pending（本机立即执行）");
    confirm_pending_now(&app)
}

#[tauri::command]
fn cancel_pending(app: AppHandle) -> Result<(), String> {
    trace(&app, "command", "cancel_pending（取消待执行请求）");
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
    // 第一件事：给快照版本号取「跟时间挂钩」的起点。必须早于任何 publish_ui ——
    // 否则第一份快照还是从 0 数起，客户端跨进程重启就会永久冻在旧快照上
    // （原因见 UI_REVISION 的注释）。
    UI_REVISION.store(ui_revision_seed(unix_millis()), Ordering::SeqCst);
    // 必须在建 WebView 之前：WebKit 是在那一刻读这些环境变量的（NVIDIA 上白窗的修法）
    #[cfg(target_os = "linux")]
    apply_linux_gpu_workarounds();

    let builder = tauri::Builder::default()
        .manage(Mutex::<AppState>::default())
        .manage(Mutex::<Settings>::default())
        // busy / 最近动作 / 菜单内容指纹：都只由 publish_ui 与托盘动作读写
        .manage(Mutex::<BusyState>::default())
        .manage(Mutex::<Option<ActionResult>>::default())
        // 自绘托盘菜单的锚点/已呈现高度/弹出时刻
        .manage(Mutex::<TrayMenuRuntime>::default())
        // 菜单内容指纹（classic 原生菜单「变了才重建」/ unified dbusmenu「变了才发信号」）
        .manage(Mutex::<Option<String>>::default())
        // server-hub 的终端会话表（PTY + 系统 ssh，窗口销毁时全收，见 hub_term::kill_all）
        .manage(hub_term::HubTermState::default())
        // server-hub 的文件浏览窗状态（pending 服务器 + 传输子进程表，见 hub_sftp.rs）
        .manage(hub_sftp::HubSftpState::default())
        // server-hub 的隧道进程表（应用退出时全收，见 hub_tunnel::kill_all）
        .manage(hub_tunnel::HubTunnelState::default())
        .manage(remote_servers::RemoteJumpState::default());
    // macOS 13+ 走系统登录项（见 login_item），这里的 LaunchAgent 方式仅作为
    // 旧版 macOS 以及 Windows / Linux 的实现。移动端没有「开机自启」，不注册。
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_autostart::init(
        MacosLauncher::LaunchAgent,
        None,
    ));
    // 托盘菜单窗的定位交给 positioner：Tray* 位置由插件内部的托盘矩形状态解析
    // （SNI/托盘事件喂给它，见 feed_positioner 与 on_tray_icon_event），我们不算坐标。
    // 移动端没有托盘，插件本体也没有 mobile 构建。
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_positioner::init());
    builder
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            // 置顶补钉与移动夹取都是桌面概念：移动端窗口不可移动/置顶，
            // 这些 API（set_always_on_top / 位置处理）在移动目标上不存在。
            #[cfg(desktop)]
            {
                // server-hub 窗口销毁（用户关窗 / 应用退出）：收掉它名下的所有终端
                // 会话 —— PTY 子进程不越窗存活，不然 ssh 会变成无主的孤儿进程。
                // hub 窗口销毁：什么都不收 —— 终端会话已归属独立的 server-term 窗
                //（见下面那条分支），关管理窗不该连带弄死正在用的终端。
                // 应用退出路径统一 kill_all（见 setup 里的 RunEvent::ExitRequested）。
                if window.label() == HUB_WINDOW_LABEL {
                    return;
                }
                // 终端窗销毁：只收归它管的会话（别的终端窗不受牵连）
                if window.label() == HUB_TERM_WINDOW_LABEL {
                    if let tauri::WindowEvent::Destroyed = event {
                        hub_term::kill_window(window.app_handle(), window.label());
                    }
                    return;
                }
                // 文件浏览窗销毁：收掉还在跑的传输子进程
                if window.label() == HUB_FILES_WINDOW_LABEL {
                    if let tauri::WindowEvent::Destroyed = event {
                        hub_sftp::kill_window(window.app_handle(), window.label());
                    }
                    return;
                }
                // 自绘托盘菜单：失焦就收起、别想真关掉（下次打开还要用这扇窗）。
                // 必须拦在下面主窗那条 `Focused` 分支**之前** —— 那条对任何窗口都 `return`。
                if window.label() == TRAY_MENU_WINDOW_LABEL {
                    match event {
                        // 拿到焦点也留一行痕：配合失焦行能看出「点外面没关」到底断在哪
                        tauri::WindowEvent::Focused(true) => {
                            startup_debug(window.app_handle(), "托盘菜单拿到焦点");
                        }
                        tauri::WindowEvent::Focused(false) => {
                            let app = window.app_handle().clone();
                            // 刚弹出那一拍会先来一次「假失焦」（WM 还没把焦点交过来），
                            // 不滤掉的话菜单一闪就没。
                            if tray_menu_blur_is_fake(&app) {
                                startup_debug(&app, "托盘菜单失焦：刚弹出那一下的假失焦，跳过");
                            } else {
                                // 用户可见动作（点外面菜单该消失）没生效时，这行就是取证起点
                                startup_debug(&app, "托盘菜单失焦 → 收起");
                                close_tray_menu(&app);
                            }
                        }
                        // Alt+F4 / 前端 close() 之类：只收起，不销毁
                        tauri::WindowEvent::CloseRequested { api, .. } => {
                            api.prevent_close();
                            let app = window.app_handle().clone();
                            startup_debug(&app, "托盘菜单收到关闭请求 → 收起");
                            close_tray_menu(&app);
                        }
                        _ => {}
                    }
                    return;
                }
                // 主窗口**常驻最顶层**（受「窗口始终置顶」设置控制）：呼出 / 拿到
                // 焦点时再确认一次置顶；失焦不再取消置顶 —— 否则点别处时窗口会被
                // 别的窗口盖住。Linux/macOS 的置顶层级会被 hide/show 一圈丢掉，
                // 这里是拿到焦点后的最后一道补钉；关掉设置时不再强制。
                if let tauri::WindowEvent::Focused(focused) = event {
                    if window.label() == MAIN_WINDOW_LABEL && *focused {
                        if stored_settings(window.app_handle()).main_always_on_top {
                            // 回到焦点：回到最上层（常态就是置顶，这里再确认）
                            let _ = window.set_always_on_top(true);
                        }
                    }
                    // 失焦：保持现状（设置开着就是置顶，关了就是普通层级）。
                    return;
                }
                // 主窗口移动：位置夹取（完整显示）+ 贴边判定。
                if let tauri::WindowEvent::Moved(position) = event {
                    if window.label() == MAIN_WINDOW_LABEL {
                        let app = window.app_handle().clone();
                        handle_main_moved(&app, *position);
                    }
                    return;
                }
            }
            #[cfg(not(desktop))]
            let _ = (window, event);
            // 这个 Tauri 版本的 WindowEvent 没有 Cursor 事件；「鼠标进窗展开 /
            // 离窗贴边」由 start_auto_hide_watcher 的轮询线程负责。
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
            // —— 单实例守卫（放在最前面）——
            // 拿不到锁 = 已经有实例在跑：说一句就退出。**后面一行都不做** ——
            // 建托盘会多出第二颗图标、起监听会跟第一例抢端口、显示主窗口会凭空
            // 多出一扇窗，那些都是「另一个实例」在用户眼里的样子。
            match acquire_instance_lock(app.handle()) {
                Ok(()) => {}
                Err(InstanceLockError::AlreadyRunning) => {
                    startup_debug(app.handle(), "检测到已有实例在运行 → 提示「已经启动」后退出");
                    // 提示完就退（macOS 上这个函数阻塞到用户关掉提示才返回，
                    // 另外两端是异步弹窗、回调里退）。
                    announce_already_running(app.handle());
                    return Ok(());
                }
                Err(InstanceLockError::Io(detail)) => {
                    // 锁文件本身用不了（配置目录只读之类）：不因此拒绝启动，
                    // 但留一行痕 —— 这时「单实例」这条保证是**没有**的。
                    startup_debug(app.handle(), &format!("单实例锁不可用（照常启动）: {detail}"));
                }
            }
            let settings = read_settings_file(app.handle());
            {
                let state = app.state::<Mutex<Settings>>();
                *state.lock().expect("settings mutex poisoned") = settings;
            }
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            // 「开机自启」同步是桌面专属（autostart 插件）
            #[cfg(desktop)]
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
            // —— 以下整段是桌面窗口管理（任务栏隐藏 / 贴边 / 尺寸锁定 / 托盘），
            // 移动端窗口是系统托管的 Activity/View，这些概念都不存在 ——
            #[cfg(desktop)]
            {
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
                // 启动即按已保存的展示方式与记忆尺寸落窗口（老配置没记过尺寸
                // 才回落到模式标称 460/932）。托盘启动也照做：呼出时窗口直接
                // 就是对的尺寸，不用再补。
                apply_grub_layout_size(app.handle(), false);
                // 锁定窗口大小的话，启动就把窗口钉成不可缩放/不可最大化
                apply_window_size_lock(app.handle());
                // 置顶状态按设置摆正：配置里存了 false（用户关过）就在启动时放下
                // —— tauri.conf 里 alwaysOnTop=true 只是默认值，设置才是事实来源。
                if !stored_settings(app.handle()).main_always_on_top {
                    if let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                        let _ = main.set_always_on_top(false);
                    }
                }
                if start_in_tray {
                    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
                        let _ = window.hide();
                    }
                } else {
                    // 启动首次显示：recalled=false —— 不挂「呼出保护」，
                    // 贴边隐藏从第一拍起就是可用的（与 Windows 行为一致）。
                    show_main_window(app.handle(), ShowOrigin::Startup);
                }
                // 贴边自动隐藏的守护线程：托盘启动也要有（呼出窗口后光标一离就贴边）；
                // 居中模式下不起（互斥）
                ensure_auto_hide_watcher(app.handle());
                // 诊断钩子：模拟一次「托盘呼出」（见 spawn_diag_recall）。不设环境变量时
                // 这里什么都不做，产品路径一行不变。
                spawn_diag_recall(app.handle());
                spawn_diag_force_dock(app.handle());
                // 诊断钩子：按 id 走一次完整的菜单派发（点不了托盘菜单时用，见 spawn_diag_menu）
                spawn_diag_menu(app.handle());
            }

            // 状态行先占位，随后的首次扫描会用真实结果覆盖
            {
                // 状态行先占位，和以前写死的「正在扫描…」一致；随后的首次扫描会覆盖它
                let state = app.state::<Mutex<AppState>>();
                let mut state = state.lock().expect("state mutex poisoned");
                state.message = "正在扫描…".to_string();
            }
            // —— 托盘与菜单整段桌面专属（移动端没有托盘/原生菜单）——
            #[cfg(desktop)]
            {
                // 两种形态在启动时二选一（切换 trayMenuStyle 后重启生效）：
                // classic（默认）= 三端各用系统原生托盘菜单（三端自绘之前的原始方案）；
                // unified = 全局统一 —— 右键都开同一扇自绘菜单窗（Win/mac 托盘事件直开；
                // Linux 按 WM 分流：GNOME 单条+opened 弹自绘窗，其余全量）。
                // Linux 例外（2026-10-15 定案）：**两种形态都走自研 linux_sni** ——
                // muda 0.20 的 GTK3 菜单子菜单渲染有回归（展开变宽但条目全空，GNOME
                // 实机反复复现），tauri 托盘那条路不再可靠；差别只剩分流（见
                // linux_sni::UNIFIED_STYLE）：unified 在 GNOME 弹自绘窗，classic 全量
                // dbusmenu 由宿主渲染（GtkMenu 自带滚动）。Win/mac 不受影响。
                #[cfg(target_os = "linux")]
                linux_sni::spawn(app.handle().clone());
                if tray_menu_style_is_unified(app.handle()) {
                    // Windows / macOS：托盘只留**图标**，不挂任何原生菜单 —— 菜单是
                    // 同一扇自绘窗（tray-menu）。左键=显示主窗口，右键=自绘菜单。
                    // （tray-icon 在这两端的 Click 事件**不依赖菜单**：macOS 是 rightMouseDown
                    // 无条件发事件、空菜单只 highlight；Windows 是 WM_RBUTTONUP 直发事件。）
                    #[cfg(not(target_os = "linux"))]
                    let _tray = TrayIconBuilder::with_id("main")
                        .icon(create_tray_image())
                        .show_menu_on_left_click(false)
                        .on_tray_icon_event(|tray, event| {
                            // 把托盘矩形喂给 positioner（Enter/Move 也带 rect，顺手都收；
                            // 插件内部会自己过滤只认 Click/Enter/Leave/Move）
                            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
                            let TrayIconEvent::Click {
                                button,
                                button_state: MouseButtonState::Up,
                                position,
                                ..
                            } = event
                            else {
                                return;
                            };
                            let app = tray.app_handle();
                            match button {
                                MouseButton::Left => show_main_window(app, ShowOrigin::Recall),
                                // position = 光标的物理屏幕坐标（左上原点），正好当菜单锚点
                                MouseButton::Right => open_tray_menu(app, position.x, position.y),
                                _ => {}
                            }
                        })
                        .build(app.handle())?;
                } else {
                    // classic：Win/mac 原生托盘菜单（三端自绘之前的原始方案，原样恢复）；
                    // Linux 已在上方交给 linux_sni（见开头注释），不再建 tauri 托盘。
                    #[cfg(not(target_os = "linux"))]
                    {
                        let initial_state = ui_state(app.handle());
                        // 预热菜单指纹：这一帧就是当前内容，别让紧接着的首次扫描白重建一次菜单
                        remember_menu_signature(app.handle(), menu_signature_for(&initial_state));

                        // Windows：右键菜单由我们自己弹（见 popup_tray_menu），先从托盘上**摘掉**菜单，
                        // 否则原生实现会抢在事件前面用「光标锚点」把它弹出来，压住任务栏。
                        // 托盘不挂菜单这一点对 tray-icon 是合法的：右键仍会照常发 Click 事件给我们。
                        // 菜单锚点窗口 —— 8×8、无边框、不可见，创建时就停在 (0,0)。
                        // `popup_at` 会把坐标当它的**客户区坐标**再 ClientToScreen 一次，停在原点坐标才好算。
                        //
                        // 为什么是「带 webview 的窗口」而不是更轻的 `WindowBuilder`：`AppHandle::get_window`
                        // 被 tauri 的 `unstable` feature 挡着（`windows()` 同理），只有 `get_webview_window`
                        // 是公开 API；而这里在 setup 里建一次、之后每次右键按 label 取回来，是最省事也最稳的路子。
                        // 多出来的这个 webview 从头到尾不可见、不参与交互，就是一块坐标基准。
                        #[cfg(not(target_os = "windows"))]
                        let initial_menu = build_tray_menu(app.handle(), &initial_state)?;
                        #[cfg(target_os = "windows")]
                        tauri::WebviewWindowBuilder::new(
                            app.handle(),
                            MENU_ANCHOR_WINDOW_LABEL,
                            tauri::WebviewUrl::App("menu-anchor.html".into()),
                        )
                        .inner_size(MENU_ANCHOR_SIZE, MENU_ANCHOR_SIZE)
                        .position(0.0, 0.0)
                        .decorations(false)
                        .resizable(false)
                        .skip_taskbar(true)
                        .visible(false)
                        .focused(false)
                        .build()?;

                        // Windows 上托盘不挂菜单，`mut` 只有其他平台用得上
                        #[allow(unused_mut)]
                        let mut tray_builder = TrayIconBuilder::with_id("main")
                            .icon(create_tray_image())
                            .show_menu_on_left_click(false)
                            .on_tray_icon_event(|tray, event| {
                                let TrayIconEvent::Click {
                                    button,
                                    button_state: MouseButtonState::Up,
                                    position,
                                    ..
                                } = event
                                else {
                                    return;
                                };
                                let app = tray.app_handle();
                                match button {
                                    MouseButton::Left => show_main_window(app, ShowOrigin::Recall),
                                    MouseButton::Right => on_tray_right_click(app, position),
                                    _ => {}
                                }
                            })
                            .on_menu_event(|app, event| {
                                handle_menu_event(app, event.id().as_ref());
                            });
                        #[cfg(not(target_os = "windows"))]
                        {
                            tray_builder = tray_builder.menu(&initial_menu);
                        }
                        let _tray = tray_builder.build(app.handle())?;
                    }
                }

                // 自绘菜单窗的失焦看门狗：Focused(false) 事件偶发丢失/被宽限吞掉时，
                // 「点外面不消失」就没了兜底 —— 见 spawn_tray_menu_watchdog。
                #[cfg(desktop)]
                spawn_tray_menu_watchdog(app.handle().clone());
            }

            // HTTP 服务与待执行请求的调度线程（托盘已就绪，publish_ui 能刷菜单）
            apply_http_config(app.handle());
            spawn_notify_scheduler(app.handle());

            let app_handle = app.handle().clone();
            std::thread::spawn(move || refresh_and_update(&app_handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            remote_servers::remote_list,
            remote_servers::remote_save,
            remote_servers::remote_delete,
            remote_servers::remote_set_connect_all,
            remote_servers::remote_test,
            remote_servers::remote_endpoint,
            remote_servers::remote_test_via,
            remote_servers::local_http_endpoint,
            remote_servers::hub_about_open,
            hub::hub_list_servers,
            hub::hub_save_server,
            hub::hub_delete_server,
            hub::hub_test_server,
            hub_open_keys_window,
            hub_open_term_window,
            hub_open_files_window,
            hub_term::hub_term_open,
            hub_term::hub_term_write,
            hub_term::hub_term_resize,
            hub_term::hub_term_close,
            hub_term::hub_term_take_pending,
            hub_term::hub_term_sessions,
            hub_sftp::hub_sftp_take_pending,
            hub_sftp::hub_sftp_home,
            hub_sftp::hub_sftp_list,
            hub_sftp::hub_sftp_mkdir,
            hub_sftp::hub_sftp_remove,
            hub_sftp::hub_sftp_transfer_ctl,
            hub_sftp::hub_focus_window,
            hub_sftp::hub_sftp_rename,
            hub_sftp::hub_sftp_download,
            hub_sftp::hub_sftp_upload,
            hub_keys::hub_keys_list,
            hub_keys::hub_key_generate,
            hub_keys::hub_key_import,
            hub_keys::hub_key_delete,
            hub_keys::hub_key_public_key,
            hub_tunnel::hub_list_tunnels,
            hub_tunnel::hub_save_tunnel,
            hub_tunnel::hub_delete_tunnel,
            hub_tunnel::hub_tunnel_start,
            hub_tunnel::hub_tunnel_stop,
            get_ui_state,
            current_status,
            refresh,
            tray_menu_nodes,
            tray_menu_present,
            tray_menu_click,
            tray_menu_close,
            set_vars_dir,
            switch_system,
            set_grub_partuuid,
            set_grub_dir,
            set_grub_script,
            detect_esp,
            detect_grub_by_partuuid,
            pick_grub_dir,
            set_grub_mount,
            open_grub_dir,
            sync_grub,
            switch_grub,
            set_main_auto_hide,
            set_main_always_on_top,
            set_main_auto_hide_edge_distance,
            set_main_dock_strip_edge_gap,
            set_main_dock_strip_hover_ms,
            set_main_dock_strip_thickness,
            set_main_dock_strip_length,
            set_main_dock_strip_opacity,
            set_main_dock_strip_color,
            set_main_hotzone_thickness,
            set_main_hotzone_color,
            set_main_hotzone_opacity,
            set_main_hotzone_border,
            set_main_hotzone_border_color,
            set_accent_color,
            set_main_window_position,
            set_tray_menu_style,
            set_main_fixed_monitor,
            report_dock_strip_rect,
            set_grub_list_view,
            set_grub_list_format,
            set_window_size,
            set_window_size_locked,
            reset_window_size,
            reset_main_window_position,
            install_grub_script,
            uninstall_grub_script,
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
            open_web_page,
            set_play_sound,
            set_sound_file_path,
            set_http_token,
            set_theme,
            set_opacity,
            open_config_dir,
            open_vars_dir,
            restart_app,
            quit_app,
            retry_http,
            kill_port_occupant,
            confirm_pending,
            cancel_pending,
            present_notify_window
        ])
        .build(tauri::generate_context!())
        .expect("failed to build AnyBoot")
        .run(|app, event| {
            // 应用退出：收掉 server-hub 名下的终端会话与隧道进程，
            // 别让 ssh 子进程被过继给 init 变成孤儿。
            if let tauri::RunEvent::Exit = event {
                hub_term::kill_all(app);
                hub_sftp::kill_all(app);
                hub_tunnel::kill_all(app);
                // 电脑页「经跳板机」的本地转发子进程
                if let Ok(mut state) = app.state::<remote_servers::RemoteJumpState>().forwards.lock() {
                    for (_, f) in state.iter_mut() {
                        let _ = f.child.kill();
                        let _ = f.child.wait();
                    }
                }
            }
        });
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
    fn macos_power_table_starts_with_apple_events_not_a_password_prompt() {
        // 重启：第一条是 AppleEvent → System Events（苹果菜单同一条路，免密免配置），
        // 用不了才退免密 sudo，最后才是管理员授权框。
        let reboot = power_attempts(PowerAction::Reboot);
        assert_eq!(reboot.len(), 3, "macOS 重启：AppleEvent + 免密 sudo + 授权框兜底");
        assert_eq!(reboot[0].program, "osascript");
        assert!(!reboot[0].needs_gui_auth, "AppleEvent 不弹管理员密码框");
        assert_eq!(
            reboot[0].args.join(" "),
            "-e tell application \"System Events\" to restart"
        );
        assert_eq!(reboot[1].command(), "sudo -n /sbin/shutdown -r now");
        assert!(!reboot[1].needs_gui_auth, "sudo -n 不会弹密码框");

        assert_eq!(reboot[2].program, "osascript");
        assert!(reboot[2].needs_gui_auth);
        let script = reboot[2].args.join(" ");
        assert!(
            script.contains("administrator privileges"),
            "实际脚本: {script}"
        );
        assert!(
            script.contains("/sbin/shutdown -r now"),
            "实际脚本: {script}"
        );
        // 候选命令本身就带管理员授权，不需要再提权一层。
        assert!(escalate_power(&reboot[2], true).is_none());

        // 关机同理：AppleScript 的动词是 shut down。
        let shutdown = power_attempts(PowerAction::Shutdown);
        assert_eq!(
            shutdown[0].args.join(" "),
            "-e tell application \"System Events\" to shut down"
        );
        assert!(!shutdown[0].needs_gui_auth);
        assert_eq!(shutdown[1].command(), "sudo -n /sbin/shutdown -h now");
        assert!(shutdown[2].args.join(" ").contains("/sbin/shutdown -h now"));

        // 睡眠：三条都是免 root 的 AppleEvent 优先，再免密 sudo，最后授权框兜底。
        let sleep = power_attempts(PowerAction::Sleep);
        assert_eq!(sleep.len(), 3);
        assert_eq!(
            sleep[0].args.join(" "),
            "-e tell application \"System Events\" to sleep"
        );
        assert!(
            !sleep[0].needs_gui_auth && sleep[0].args.join(" ").contains("System Events"),
            "第一条不该弹管理员授权框"
        );
        assert_eq!(sleep[1].command(), "sudo -n /usr/bin/pmset sleepnow");
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

    #[test]
    fn instance_lock_is_exclusive_until_released() {
        // 单实例的机制全在一个函数里：拿到锁 = 本进程是唯一实例，拿不到 =
        // 已经有实例在跑。文件锁是内核级的，所以同进程再开一个 fd 也会撞上 ——
        // 这点让它可以在单测里验证（不必真的起第二个进程）。
        let dir = std::env::temp_dir().join(format!("anyboot-lock-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join(INSTANCE_LOCK_FILE);
        let _ = fs::remove_file(&path);

        let first = try_lock_instance_file(&path).expect("第一个实例应该拿到锁");
        assert_eq!(
            try_lock_instance_file(&path).unwrap_err(),
            InstanceLockError::AlreadyRunning,
            "第二个必须被判为「已经有实例在跑」"
        );

        // 放锁（只有「重启程序」需要放，见 release_instance_lock）之后新进程才抢得到
        first.unlock().expect("放锁");
        let second = try_lock_instance_file(&path).expect("放锁后应该能再拿到");
        drop(second);
        // 锁文件留在原地不影响下次启动：不读里面的内容、不等 pid（没有过期概念）
        assert!(try_lock_instance_file(&path).is_ok());

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }
}

// ——— Linux 托盘：自己实现 StatusNotifierItem ———
//
// 为什么要自己写：tauri 用的 tray-icon 在 GTK/X11 上**不发任何点击事件**（连 activate
// 信号都没接、`rect()` 恒 None），应用根本不知道托盘图标被点过。
//
// SNI 这条路两头都拿得到：宿主把点击坐标交给我们 —— `Activate(x, y)`=左键、
// `ContextMenu`=右键（实测坐标是**图标原点**而不是点击点：点 1777 收到 1744）。
// 点击语义三端一致：左键=显示主窗口，右键（和中键）=弹自绘菜单窗（open_tray_menu）。
//
// **dbusmenu 按宿主分流**（2026-10-15 四轮定案）：
// GNOME 的 Shell 弹窗**不滚动、不封顶、dbusmenu 子菜单渲染为空**（classic 的
// libappindicator 与我们自研服务两套实现同样症状，总线层数据都完整 —— 宿主渲染
// 这条路在 GNOME 上彻底没戏），所以 GNOME 的菜单面只能是**自绘窗**。抓手：
// 宿主弹窗**打开瞬间**扩展必向根发 `Event(0, "opened")`（dbusMenu.js
// _onMenuOpenStateChanged 实证）—— 但**空菜单连弹窗都开不了、opened 不来**
// （12:29 实机实测），所以 dbusmenu 导出**一条「显示主窗口」**（非空弹窗必开必发
// opened）：右键 → 弹窗瞬间我们取光标、原位弹自绘菜单窗（封顶/滚轮/单选窗内展开/
// 点选不关），**一步直达、无二阶段点击**；那条「显示主窗口」只是兜底可点（万一
// opened 不来，右键也不至于毫无反应）。弹出的空壳弹窗带着输入 grab，会吃掉自绘窗
// 的第一次点击 —— 弹出后补一个**合成 Esc**（XTEST）把它关掉，grab 释放。
// Cinnamon/KDE（以及查不到 WM 的兜底）：导出 build_menu_nodes **全量内容** ——
// 这两家的宿主菜单（GtkMenu/QMenu）自带超长滚动、子菜单渲染正常；"opened" 忽略，
// "clicked" 照旧按快照派发动作。内容变化走 LayoutUpdated + AboutToShow 脏标记。
//
// 本模块只在 **unified（全局统一）方式**下启动。
// classic（各自方式）不走这里：托盘图标由 tauri 托管、菜单是原生菜单。
#[cfg(target_os = "linux")]
mod linux_sni {
    use super::*;
    use dbus::arg::messageitem::{MessageItem, MessageItemArray, MessageItemDict};
    use dbus::ffidisp::{MsgHandler, MsgHandlerResult, MsgHandlerType};
    use dbus::strings::{ErrorName, Path as DbusPath};
    use dbus::{Message, MessageType, Signature};
    use std::ffi::CStr;
    use std::sync::atomic::{AtomicBool, AtomicU32};

    /// 我们导出的对象路径。注册给 watcher 的也是它，宿主便按「调用方唯一名 + 这个路径」找我们。
    const ITEM_PATH: &str = "/StatusNotifierItem";
    const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
    const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
    const INTROSPECT_IFACE: &str = "org.freedesktop.DBus.Introspectable";
    const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
    const WATCHER_PATH: &str = "/StatusNotifierWatcher";
    /// dbusmenu 的对象路径与接口（内容按宿主分流：GNOME 空根 / 其余全量）。
    const MENU_PATH: &str = "/StatusNotifierMenu";
    const MENU_IFACE: &str = "com.canonical.dbusmenu";

    /// 宿主是不是 GNOME Shell（serve() 启动时经 X11 查 WM 名一次性判定）。
    /// 「GNOME + unified」走「单条 + opened 时机弹自绘窗」，其余组合走全量 dbusmenu。
    static GNOME_HOST: AtomicBool = AtomicBool::new(false);
    fn gnome_host() -> bool {
        GNOME_HOST.load(Ordering::SeqCst)
    }

    /// 当前是不是 unified 方式（serve() 启动时读 trayMenuStyle 一次性判定）。
    ///
    /// 2026-10-15 定案：classic 在 Linux 也走本模块（muda 0.20 的 GTK3 菜单
    /// 子菜单渲染回归——展开变宽但条目全空，用户 GNOME 实机反复复现）。
    /// 分流收窄为「GNOME 且 unified」才弹自绘窗；classic 一律全量 dbusmenu
    /// （GNOME 扩展用 GtkMenu 渲染，自带滚动与封顶，AboutToShow 脏标记 /
    /// children-display 修复后子菜单可正常展开）。
    static UNIFIED_STYLE: AtomicBool = AtomicBool::new(false);
    fn unified_style() -> bool {
        UNIFIED_STYLE.load(Ordering::SeqCst)
    }

    /// dbusmenu 内容代次：内容每变一次 +1（GetLayout 回执与 LayoutUpdated 信号都用它）。
    static MENU_REVISION: AtomicU32 = AtomicU32::new(1);
    /// 内容待宿主重拉：`menu_layout_changed` 置位，**只在 GetLayout 应答时清零**。
    /// 初始 true —— 扩展客户端只有 AboutToShow 回 true（或收到 LayoutUpdated）才会
    /// 发起第一次布局拉取，开机得先脏一回。
    ///
    /// 为什么不能"随手清"：AboutToShow 与 LayoutUpdated 是**两条并列的重拉促发路径**，
    /// 读的是同一位 —— SNI 线程发信号时清了，AboutToShow 就可能漏报。各走各的，
    /// 清零统一交给 GetLayout（宿主拿到新内容的那一刻才算"不脏"）。
    /// 注意它不分条目：快照本来就是整树现建，全局一位足够。
    static MENU_DIRTY: AtomicBool = AtomicBool::new(true);
    /// 已广播过 LayoutUpdated 的代次：防重发（脏标记可能持续为 true，直到宿主来拉；
    /// 代次没变就不必再广播一遍）。
    static MENU_SIGNAL_REVISION: AtomicU32 = AtomicU32::new(0);
    /// 最近一次 GetLayout 建立的快照：dbusmenu 数字 id ↔ 动作 id 的对应表。
    /// 宿主先 GetLayout 拿树、再按数字 id 发 Event —— 两边都读这一份才对得上。
    static MENU_SNAPSHOT: Mutex<Option<MenuSnapshot>> = Mutex::new(None);

    /// unified 菜单内容变化入口（update_tray_menu 在主线程调）。
    ///
    /// 指纹变了 → 代次 +1 + 置脏；SNI 线程下一拍发出 LayoutUpdated，宿主收到后重新
    /// GetLayout 拿到全量新内容。Win/mac 上没有 SNI 线程，置的脏没人消费，无副作用。
    /// GNOME 空根永远不变，置脏对它无感（重拉一次还是空根）。
    pub(crate) fn menu_layout_changed(changed: bool) {
        if !changed {
            return;
        }
        MENU_REVISION.fetch_add(1, Ordering::SeqCst);
        MENU_DIRTY.store(true, Ordering::SeqCst);
    }

    /// 起一条专用线程跑 SNI 服务。
    ///
    /// 连接在**线程内部**建：`dbus::ffidisp::Connection` 内部是 `Rc`（不是 Send），
    /// 建好再往线程里搬过不了编译 —— 建在里面、用在里面，就不必和类型系统较劲。
    pub fn spawn(app: AppHandle) {
        std::thread::spawn(move || {
            if let Err(error) = serve(app.clone()) {
                startup_debug(&app, &format!("托盘 SNI 服务退出: {error}"));
            }
        });
    }

    fn serve(app: AppHandle) -> Result<(), String> {
        // 方式判定（一次性）：classic/unified 都走本模块（Linux），差别只在分流——
        // 「GNOME 且 unified」单条+opened 弹自绘窗；classic 永远全量 dbusmenu。
        let unified = tray_menu_style_is_unified(&app);
        UNIFIED_STYLE.store(unified, Ordering::SeqCst);
        // 宿主判定（一次性）：GNOME Shell 的宿主弹窗不滚动不封顶，unified 的全量菜单
        // 装不下 —— 走「单条 + opened 时机弹自绘窗」；classic 与其余宿主走全量。
        let wm_name = detect_wm_name();
        let gnome = wm_name
            .as_deref()
            .map(|name| name.contains("GNOME"))
            .unwrap_or(false);
        GNOME_HOST.store(gnome, Ordering::SeqCst);
        startup_debug(
            &app,
            &format!(
                "托盘 SNI 启动：方式={} WM={} → {}",
                if unified { "unified" } else { "classic" },
                wm_name.as_deref().unwrap_or("(查不到)"),
                if gnome && unified {
                    "GNOME 模式（dbusmenu 单条，opened 时弹自绘窗）"
                } else {
                    "全量 dbusmenu"
                }
            ),
        );
        let conn = dbus::ffidisp::Connection::new_session()
            .map_err(|error| format!("连不上会话总线: {error}"))?;
        conn.register_object_path(ITEM_PATH)
            .map_err(|error| format!("注册对象路径 {ITEM_PATH} 失败: {error}"))?;
        conn.register_object_path(MENU_PATH)
            .map_err(|error| format!("注册对象路径 {MENU_PATH} 失败: {error}"))?;
        conn.add_handler(SniHandler { app: app.clone() });
        conn.add_handler(MenuHandler { app: app.clone() });
        // 名字带 pid：多实例互不打架（SNI 惯例，ayatana / KDE 都这么取）
        let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        conn.register_name(&name, 0)
            .map_err(|error| format!("请求名字 {name} 失败: {error}"))?;
        startup_debug(
            &app,
            &format!(
                "托盘 SNI 服务就绪，开始向 watcher 注册：{name}{ITEM_PATH}（总线 env={:?} 连接唯一名={}）",
                std::env::var("DBUS_SESSION_BUS_ADDRESS")
                    .unwrap_or_else(|_| "(未设置)".to_string()),
                conn.unique_name(),
            ),
        );
        // 第 1 次失败时顺带列出总线上所有带 Notifier 的名字：区分「应用连错了总线
        // （终端没继承 DBUS_SESSION_BUS_ADDRESS 时 libdbus 会 autolaunch 一条新的）」
        // 和「总线没错、面板就是没跑 watcher」这两个完全不同方向的问题。
        let notifier_names = |conn: &dbus::ffidisp::Connection| -> Vec<String> {
            let Ok(call) = Message::new_method_call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "ListNames",
            ) else {
                return vec!["(ListNames 构造失败)".to_string()];
            };
            match conn.send_with_reply_and_block(call, 2000) {
                Ok(reply) => match reply.get1::<MessageItem>() {
                    Some(MessageItem::Array(names)) => names
                        .iter()
                        .filter_map(|item| match item {
                            MessageItem::Str(name) if name.contains("StatusNotifier") => {
                                Some(name.clone())
                            }
                            _ => None,
                        })
                        .collect(),
                    _ => vec!["(ListNames 回执不是字符串数组)".to_string()],
                },
                Err(error) => vec![format!("(ListNames 失败: {error})")],
            }
        };
        // 告诉 watcher「托盘图标在这儿」。传对象路径（与 libappindicator 同款做法）。
        //
        // **注册要重试**：开机自启动的应用常常比面板 / watcher 先起来，第一次注册几乎
        // 必然扑空（ServiceUnknown）—— 只试一次的话托盘图标就永远不出现，直到重启应用。
        // 所以注册放进主循环：没注册上就每 3 秒再发一次，成功一次就收手。
        // 取证走常开通道（startup-debug.log），失败只记第一次和之后每 30 秒一次，别刷屏。
        let mut registered = false;
        let mut failures = 0u32;
        let mut next_try = Instant::now();
        // 消息循环：`iter` 内部会把方法调用交给上面 add_handler 注册的处理器，处理器返回的
        // 回复由 dbus crate 直接发出去（ffidisp::msghandler_process）。超时时 yield Nothing
        // —— 正好用来轮转注册重试，不需要额外的定时器。
        let mut items = conn.iter(200);
        loop {
            match items.next() {
                Some(dbus::ffidisp::ConnectionItem::Nothing) => {}
                Some(_) => {}
                None => return Err("会话总线断了".to_string()),
            }
            // 菜单内容变了（update_tray_menu 置脏）：两路回显。
            //
            // ① **ItemsPropertiesUpdated**：把变了的属性（勾选标记挪动 = label 文字变了）
            //    主动推给宿主，开着的菜单**就地刷新** —— 这是「双向绑定的回程」。
            //    只靠 ② 不行：KDE/Qt 宿主对 LayoutUpdated 的响应不可靠（2026-10-15 实测
            //    菜单开着时不重拉，勾选回显要重开菜单才刷新）。
            // ② **LayoutUpdated**：兜底信号，宿主收到会重新 GetLayout —— 结构性变化
            //    （增删条目、子菜单折叠）靠它整树重拉。
            //
            // **这里不清脏标记**（AboutToShow 同读这一位，清零统一在 GetLayout）；
            // 靠「已广播代次」防重发。
            if MENU_DIRTY.load(Ordering::SeqCst) {
                let revision = MENU_REVISION.load(Ordering::SeqCst);
                if MENU_SIGNAL_REVISION.swap(revision, Ordering::SeqCst) != revision {
                    // **信号必须从 Menu 对象路径发**（MENU_PATH，即 Item.Menu 指向的那
                    // 个）。dbusmenu 规范里 com.canonical.dbusmenu 的信号挂在菜单对象
                    // 上，宿主按路径匹配订阅 —— 从 ITEM_PATH 发的信号会被所有宿主
                    // 静默丢弃（2026-10-15 实测：LayoutUpdated 根本到不了探针/宿主，
                    // 勾选回显不刷新的老病根之一）。静态字面量，构造失败只可能是代码写错。
                    let path = DbusPath::new(MENU_PATH).expect("静态路径合法");
                    let iface = dbus::strings::Interface::new(MENU_IFACE).expect("静态接口名合法");
                    // ① 现建新快照与旧快照求差分，换上新快照后**按结果分流**：
                    //    只属性变 → 只推 ItemsPropertiesUpdated（就地刷新回显）；
                    //    只结构变 → 只发 LayoutUpdated（宿主整树重拉）；
                    //    两者都没变 → 什么都不发（无操作点击不许有更新风暴）。
                    //    宿主还没拉过快照（没有开着的菜单）时无回显可言，只发兜底。
                    let (structural, updates) = {
                        let mut guard = MENU_SNAPSHOT
                            .lock()
                            .expect("dbusmenu snapshot mutex poisoned");
                        match guard.as_ref() {
                            Some(old) => {
                                let fresh =
                                    build_menu_snapshot(&menu_nodes_for_host(&ui_state(&app)));
                                let diff = menu_property_diff(old, &fresh);
                                *guard = Some(fresh);
                                diff
                            }
                            None => (true, Vec::new()),
                        }
                    };
                    if !updates.is_empty() {
                        let updated: Vec<MessageItem> = updates
                            .into_iter()
                            .map(|(id, changed)| {
                                MessageItem::Struct(vec![
                                    MessageItem::Int32(id),
                                    menu_props_dict(changed),
                                ])
                            })
                            .collect();
                        // 信号体：a(ia{sv})a(ias) —— 后半是「被移除的属性」，我们只有
                        // 属性更新，恒空数组。**第二个签名必须是 a(ias) 而不是 a(iias)**：
                        // 规范/各家实现（Chromium `OpenArray("(ias)")`、libdbusmenu-qt、
                        // Go systray）都是 (id, 属性名数组) 这个结构；Qt 的
                        // QDBusConnection::connect 按**签名精确匹配**，签名不符的信号
                        // 直接被丢掉（2026-10-15 真 KDE 客户端复现：a(iias) 时
                        // ItemsPropertiesUpdated 发了没人收，勾选回显不刷新；
                        // GNOME/Cinnamon 走 GDBus 按名字匹配，所以一直没暴露）。
                        let updated = MessageItem::Array(
                            MessageItemArray::new(
                                updated,
                                Signature::new("a(ia{sv})").expect("静态签名"),
                            )
                            .expect("元素都是 (ia{sv}) 结构"),
                        );
                        let removed = MessageItem::Array(
                            MessageItemArray::new(
                                Vec::new(),
                                Signature::new("a(ias)").expect("静态签名"),
                            )
                            .expect("空数组构造不会失败"),
                        );
                        let member = dbus::strings::Member::new("ItemsPropertiesUpdated")
                            .expect("静态成员名合法");
                        let mut signal = Message::signal(&path, &iface, &member);
                        signal.append_items(&[updated, removed]);
                        if let Err(()) = conn.send(signal) {
                            startup_debug(&app, "ItemsPropertiesUpdated 信号发送失败");
                        }
                    }
                    // ② 结构兜底：只有结构真变了才发 LayoutUpdated —— 无变化时不许白发
                    //    （宿主整树重拉会销毁展开中的子菜单，见 menu_property_diff 注释）。
                    if structural {
                        let member = dbus::strings::Member::new("LayoutUpdated")
                            .expect("静态成员名合法");
                        let mut signal = Message::signal(&path, &iface, &member);
                        signal.append_items(&[MessageItem::UInt32(revision), MessageItem::Int32(0)]);
                        if let Err(()) = conn.send(signal) {
                            startup_debug(&app, "LayoutUpdated 信号发送失败");
                        }
                    }
                }
            }
            if registered || next_try > Instant::now() {
                continue;
            }
            let Ok(mut register) = Message::new_method_call(
                WATCHER_NAME,
                WATCHER_PATH,
                WATCHER_NAME,
                "RegisterStatusNotifierItem",
            ) else {
                return Err("构造注册消息失败".to_string());
            };
            register.append_items(&[MessageItem::Str(ITEM_PATH.to_string())]);
            match conn.send_with_reply_and_block(register, 2000) {
                Ok(_) => {
                    registered = true;
                    startup_debug(&app, &format!("托盘图标已注册到 StatusNotifierWatcher（第 {failures} 次失败后成功）"));
                }
                Err(error) => {
                    failures += 1;
                    if failures == 1 || failures % 10 == 0 {
                        startup_debug(
                            &app,
                            &format!(
                                "托盘图标注册失败（第 {failures} 次，3 秒后重试）: {error}；总线上带 StatusNotifier 的名字={:?}",
                                notifier_names(&conn)
                            ),
                        );
                    }
                    next_try = Instant::now() + Duration::from_secs(3);
                }
            }
        }
    }

    /// 把托盘锚点喂给 positioner：合成一条托盘 Click 事件（插件只读 rect）。
    /// 宿主没给图标尺寸，用 32×32 标称。落位已改手算（不再用 TrayCenter），
    /// 这一步只剩「保持插件状态有人喂」的意义 —— 插件仍注册着，别让它拿到过期矩形。
    fn feed_positioner(app: &AppHandle, x: f64, y: f64) {
        // 本模块只在 unified 方式下启动（classic 用原生菜单，不经过这里）；
        // Win/mac 的入口在 on_tray_icon_event 里直接喂原生事件。
        use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent, TrayIconId};
        let event = TrayIconEvent::Click {
            id: TrayIconId::new("main"),
            position: tauri::PhysicalPosition::new(x, y),
            rect: tauri::Rect {
                position: tauri::PhysicalPosition::new(x, y).into(),
                size: tauri::PhysicalSize::new(32.0, 32.0).into(),
            },
            button: MouseButton::Right,
            button_state: MouseButtonState::Up,
        };
        tauri_plugin_positioner::on_tray_event(app, &event);
    }

    /// 读窗口管理器名（X11：根窗口 `_NET_SUPPORTING_WM_CHECK` → 那个窗口的
    /// `_NET_WM_NAME`）。GNOME Shell 报 "GNOME Shell"、KDE 报 "KWin"、Cinnamon 报
    /// "Muffin"。查不到（Wayland / 打不开 Display / 属性缺失）回 None —— 兜底走全量
    /// 导出（全量在 Cinnamon/KDE 是正解，GNOME 查不到说明根本不是 GNOME 的 X11 会话）。
    fn detect_wm_name() -> Option<String> {
        let Ok(xlib) = x11_dl::xlib::Xlib::open() else {
            return None;
        };
        unsafe {
            let display = (xlib.XOpenDisplay)(std::ptr::null());
            if display.is_null() {
                return None;
            }
            let name = (|| {
                let atom_name = |s: &str| {
                    std::ffi::CString::new(s).expect("原子名不含内嵌 0")
                };
                let root = (xlib.XDefaultRootWindow)(display);
                let check = (xlib.XInternAtom)(
                    display,
                    atom_name("_NET_SUPPORTING_WM_CHECK").as_ptr(),
                    0,
                );
                if check == 0 {
                    return None;
                }
                let mut actual_type: u64 = 0;
                let mut actual_format = 0i32;
                let mut nitems: u64 = 0;
                let mut bytes_after: u64 = 0;
                let mut prop: *mut u8 = std::ptr::null_mut();
                // 33 = XA_WINDOW
                let status = (xlib.XGetWindowProperty)(
                    display,
                    root,
                    check,
                    0,
                    1,
                    0,
                    33,
                    &mut actual_type,
                    &mut actual_format,
                    &mut nitems,
                    &mut bytes_after,
                    &mut prop,
                );
                if status != 0 || nitems < 1 {
                    return None;
                }
                let wm_window = *(prop as *const u64);
                (xlib.XFree)(prop.cast());
                let name_atom =
                    (xlib.XInternAtom)(display, atom_name("_NET_WM_NAME").as_ptr(), 0);
                let utf8_atom = (xlib.XInternAtom)(display, atom_name("UTF8_STRING").as_ptr(), 0);
                if name_atom == 0 || utf8_atom == 0 {
                    return None;
                }
                prop = std::ptr::null_mut();
                let status = (xlib.XGetWindowProperty)(
                    display,
                    wm_window,
                    name_atom,
                    0,
                    256,
                    0,
                    utf8_atom,
                    &mut actual_type,
                    &mut actual_format,
                    &mut nitems,
                    &mut bytes_after,
                    &mut prop,
                );
                if status != 0 || nitems < 1 {
                    return None;
                }
                let bytes = std::slice::from_raw_parts(prop, nitems as usize);
                let name = String::from_utf8_lossy(bytes)
                    .trim_end_matches('\0')
                    .to_string();
                (xlib.XFree)(prop.cast());
                Some(name)
            })();
            (xlib.XCloseDisplay)(display);
            name
        }
    }

    /// 取当前光标的屏幕物理坐标（X11 根窗口坐标，与 Activate 给的原点同空间）。
    ///
    /// dbusmenu 的 `Event(id, event, data, timestamp)` 不带坐标，而 GNOME 扩展在宿主
    /// 弹窗打开瞬间必发 `opened`（dbusMenu.js 实证）—— 那一刻光标正停在托盘图标上，
    /// 拿它当自绘菜单窗的锚点最贴切。X11 查询失败（Wayland / 打不开 Display）回 None，
    /// 调用方退回 `fallback_anchor`。
    fn cursor_position() -> Option<(f64, f64)> {
        let xlib = x11_dl::xlib::Xlib::open().ok()?;
        unsafe {
            let display = (xlib.XOpenDisplay)(std::ptr::null());
            if display.is_null() {
                return None;
            }
            let mut root_return = 0u64;
            let mut child = 0u64;
            let mut root_x = 0i32;
            let mut root_y = 0i32;
            let mut win_x = 0i32;
            let mut win_y = 0i32;
            let mut mask = 0u32;
            let root = (xlib.XDefaultRootWindow)(display);
            let on_screen = (xlib.XQueryPointer)(
                display,
                root,
                &mut root_return,
                &mut child,
                &mut root_x,
                &mut root_y,
                &mut win_x,
                &mut win_y,
                &mut mask,
            );
            (xlib.XCloseDisplay)(display);
            if on_screen != 0 {
                Some((root_x as f64, root_y as f64))
            } else {
                None
            }
        }
    }

    /// 给 X 输入队列注入一次 Esc 按下+抬起（XTEST，合成事件等同真实键盘）。
    ///
    /// 用途：GNOME 上宿主弹窗（壳）开着时持有输入 grab，自绘菜单窗的第一次点击会被
    /// 它吃掉；壳弹出后注入 Esc，Shell 的菜单 grab 收到 Esc 就收起菜单、释放 grab。
    /// keycode 9 = evdev 布局的 Esc（Xorg 默认）。注入失败（无 XTEST/非 X11）静默
    /// 放弃 —— 最坏情况只是壳多留一会儿、第一次点击被吃。
    fn send_escape_key() {
        let Ok(xlib) = x11_dl::xlib::Xlib::open() else {
            return;
        };
        // x11-dl 的 xtest 绑定沿用 link 宏名，结构体叫 Xf86vmode（上游如此，非笔误）
        let Ok(xtest) = x11_dl::xtest::Xf86vmode::open() else {
            return;
        };
        unsafe {
            let display = (xlib.XOpenDisplay)(std::ptr::null());
            if display.is_null() {
                return;
            }
            const KEY_ESC: u32 = 9;
            let pressed = (xtest.XTestFakeKeyEvent)(display, KEY_ESC, 1, 0);
            let released = (xtest.XTestFakeKeyEvent)(display, KEY_ESC, 0, 0);
            (xlib.XSync)(display, 0);
            (xlib.XCloseDisplay)(display);
            if pressed == 0 || released == 0 {
                // 注入失败没法在这条线程里留痕（拿不到 app 句柄），XSync 错误也无需处理
            }
        }
    }

    /// 光标查不到时的兜底锚点：主显示器偏下的位置（大多数托盘在屏幕角落，
    /// 这里宁可退而求其次也别把菜单开出屏幕）。
    fn fallback_anchor(app: &AppHandle) -> (f64, f64) {
        app.primary_monitor()
            .ok()
            .flatten()
            .map(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                (
                    (position.x + size.width as i32 / 2) as f64,
                    (position.y + size.height as i32 * 3 / 4) as f64,
                )
            })
            .unwrap_or((0.0, 0.0))
    }

    /// 宿主（面板 / watcher）对我们这个 SNI 对象的全部调用。
    ///
    /// `MsgHandlerType` 只能按消息类型过滤、**不能按路径**，所以路径得在这儿自己判。
    struct SniHandler {
        app: AppHandle,
    }

    impl MsgHandler for SniHandler {
        fn handler_type(&self) -> MsgHandlerType {
            MsgHandlerType::MsgType(MessageType::MethodCall)
        }

        fn handle_msg(&mut self, msg: &Message) -> Option<MsgHandlerResult> {
            // 不是发给我们这个对象的：不认领（别家路径交给 libdbus 自己的处理）
            if msg.path().as_deref() != Some(ITEM_PATH) {
                return None;
            }
            let interface = msg.interface().map(|i| i.to_string()).unwrap_or_default();
            let member = msg.member().map(|m| m.to_string()).unwrap_or_default();
            let reply = match (interface.as_str(), member.as_str()) {
                (PROPS_IFACE, "Get") => properties_get(msg),
                (PROPS_IFACE, "GetAll") => properties_get_all(msg),
                (INTROSPECT_IFACE, "Introspect") => introspect(msg),
                (ITEM_IFACE, "Activate") => self.activate(msg, "左键", false),
                (ITEM_IFACE, "SecondaryActivate") => self.activate(msg, "中键", true),
                (ITEM_IFACE, "ContextMenu") => self.activate(msg, "右键", true),
                (ITEM_IFACE, "Scroll") => msg.method_return(),
                _ => unknown_method(msg),
            };
            Some(MsgHandlerResult {
                handled: true,
                done: false,
                reply: vec![reply],
            })
        }
    }

    impl SniHandler {
        /// 宿主点了托盘图标。点击语义三端一致：左键=显示主窗口，右键/中键=自绘菜单。
        fn activate(&self, msg: &Message, what: &str, menu: bool) -> Message {
            let (x, y) = msg.read2::<i32, i32>().unwrap_or((0, 0));
            // 用户点了托盘 —— 这是用户可见动作，走常开通道留痕（trace 默认关，等于没有）
            startup_debug(&self.app, &format!("托盘{what}点击 ({x}, {y}) → {}", if menu { "弹菜单" } else { "主窗口" }));
            if menu {
                feed_positioner(&self.app, x as f64, y as f64);
                open_tray_menu(&self.app, x as f64, y as f64);
            } else {
                show_main_window(&self.app, ShowOrigin::Recall);
            }
            msg.method_return()
        }
    }

    /// dbusmenu 宿主端实现（`com.canonical.dbusmenu`）—— **按宿主分流**。
    ///
    /// unified 方式下三家的右键都走这里：GNOME 的 appindicators 扩展点击托盘时**只按
    /// dbusmenu 渲染宿主菜单**、从不回调 `ContextMenu`（`Menu=""` 时它按空路径拉菜单，
    /// 什么都弹不出来 —— 用户实测）；Cinnamon（xapp）与 KDE 看到有效的 Menu 也会自建
    /// 宿主菜单消费右键。
    ///
    /// - **GNOME**：导出**空根**（Shell 弹窗不滚动不封顶，全量装不下）；右键弹出瞬间
    ///   扩展会向根发 `Event(0, "opened")`（dbusMenu.js 实证）—— 抓这个时机弹自绘窗。
    /// - **Cinnamon/KDE**：导出 build_menu_nodes 的**全量内容**，宿主菜单自带滚动，
    ///   "clicked" 按快照派发动作（与自绘窗同一份 handle_menu_event）。
    ///
    /// 内容是**动态**的：每次 GetLayout 都按当前 UiState 现建快照；内容变化（指纹变了）
    /// 走 LayoutUpdated 信号 + AboutToShow 提醒宿主重新拉取。
    struct MenuHandler {
        app: AppHandle,
    }

    /// 一次 GetLayout 现建的 dbusmenu 快照。
    ///
    /// dbusmenu 的 id 是它自己的 i32 编号（1 起顺序分配，0 恒为根）；
    /// `node_id` 是点击时要派发的动作 id（与自绘窗同一套，分隔线/表头没有）。
    /// 宿主先 GetLayout 拿树、再按数字 id 发 Event —— 两边都读 MENU_SNAPSHOT 这一份才对得上。
    struct MenuSnapshot {
        items: Vec<SnapshotNode>,
        root_children: Vec<i32>,
    }

    struct SnapshotNode {
        dbus_id: i32,
        node_id: Option<String>,
        /// dbusmenu 规范的标准属性（type/label/enabled/visible/toggle-*）。
        properties: Vec<(&'static str, MessageItem)>,
        children: Vec<i32>,
    }

    /// 按宿主与方式取要导出的菜单模型。「GNOME + unified」留两条「显示主窗口 / 退出程序」：
    /// **非空菜单宿主弹窗才开得出来**（空菜单实测连弹窗都不开、opened 事件也不发），
    /// 弹出瞬间 opened 触发自绘窗一步直达；这两条本身只是兜底可点（万一 opened 不来，
    /// 右键也不至于毫无反应，还能就地退出）。
    /// classic（含 GNOME）与其余宿主一律全量导出（见 UNIFIED_STYLE 注释）。
    fn menu_nodes_for_host(state: &UiState) -> Vec<MenuNode> {
        if gnome_host() && unified_style() {
            vec![
                menu_item("show", "显示主窗口", true),
                menu_item("quit", "退出程序", true),
            ]
        } else {
            build_menu_nodes(state)
        }
    }

    /// 把菜单模型装成 dbusmenu 快照：深度优先，id 顺序分配。
    fn build_menu_snapshot(nodes: &[MenuNode]) -> MenuSnapshot {
        let mut next_id = 1i32;
        fn walk(
            nodes: &[MenuNode],
            snapshot: &mut MenuSnapshot,
            next_id: &mut i32,
        ) -> Vec<i32> {
            let mut ids = Vec::new();
            for node in nodes {
                let dbus_id = *next_id;
                *next_id += 1;
                let properties = match node.kind {
                    "sep" => vec![
                        ("type", MessageItem::Str("separator".to_string())),
                        ("visible", MessageItem::Bool(true)),
                    ],
                    _ => {
                        // 列表类条目的「当前项」标记**画进 label 文字**（radio_label 的
                        // ◉/○），**不导出 toggle-type/toggle-state** —— 与
                        // toggle_label/radio_label 的定则一致：标记进文字，与工具包/宿主无关。
                        //
                        // 为什么（2026-10-15 用户实测 KDE，各自方式与全局统一都中招）：
                        // 宿主（Plasma 的 Qt 菜单）对 checkable 条目会**先本地翻转再通知
                        // 我们**（QMenu 点 checkable 动作的标准行为），而 dbusmenu 协议没有
                        // 「单选组」概念，宿主不知道哪些行互斥 —— 点几个就勾几个（用户报的
                        // 「GRUB 选择框变成多选」）；本地翻转还把视觉搅乱，我们靠
                        // LayoutUpdated 推权威状态也追不回来（「单选框点不动」）。不交出
                        // 勾选机制，宿主只能照纯文本渲染，标记永远以我们算出的文字为准。
                        let label = match node.checked {
                            Some(checked) if node.keeps_open => {
                                radio_label(checked, &node.label)
                            }
                            // 顶部「下次启动」这类动作行：文字本身已说明当前项，不补标记
                            _ => node.label.clone(),
                        };
                        let mut properties = vec![
                            ("type", MessageItem::Str("standard".to_string())),
                            ("label", MessageItem::Str(label)),
                            ("enabled", MessageItem::Bool(node.enabled)),
                            ("visible", MessageItem::Bool(true)),
                        ];
                        // 子菜单标记必须进快照属性表：GNOME 扩展拿到 GetGroupProperties
                        // 回应后会 resetProperties() 清空再逐个 set —— 只在 GetLayout
                        // 序列化时补的话，这枚标记会被那次清洗抹掉，条目被定型为普通项，
                        // 下拉列表展不开（用户实测）。判定在扩展侧读的是缓存的属性。
                        if node.kind == "sub" {
                            properties.push((
                                "children-display",
                                MessageItem::Str("submenu".to_string()),
                            ));
                        }
                        properties
                    }
                };
                // 先序入列：父节点先占坑，子节点随后 —— items 的顺序与 id 一致，好排查
                let index = snapshot.items.len();
                snapshot.items.push(SnapshotNode {
                    dbus_id,
                    node_id: node.id.clone(),
                    properties,
                    children: Vec::new(),
                });
                ids.push(dbus_id);
                if node.kind == "sub" {
                    let children = walk(&node.children, snapshot, next_id);
                    snapshot.items[index].children = children;
                }
            }
            ids
        }
        let mut snapshot = MenuSnapshot {
            items: Vec::new(),
            root_children: Vec::new(),
        };
        snapshot.root_children = walk(nodes, &mut snapshot, &mut next_id);
        snapshot
    }

    /// 新旧快照差分。返回 `(结构是否变了, 属性更新列表)`：
    ///
    /// - 结构没变：回每个条目上**变了的那几枚属性** —— `ItemsPropertiesUpdated`
    ///   让开着的菜单就地刷新勾选回显；
    /// - 结构变了（增删/搬移会让先序 id 整片洗牌）：属性列表为空，交给
    ///   LayoutUpdated 让宿主整树重拉（按 id 求差分只会对上错行的旧属性）。
    ///
    /// **调用方必须按结果分流**：只属性变 → 只推 ItemsPropertiesUpdated；只结构变 →
    /// 只发 LayoutUpdated；两者都没变 → 什么都不发。GNOME 客户端 propertySet 不去重、
    /// 收到 LayoutUpdated 会整树重拉 —— 无变化时白发信号就是更新风暴，会当场销毁
    /// 展开中的子菜单（cea8908 之后的 H 步探针抓过：无操作点击也广播 LayoutUpdated）。
    fn menu_property_diff(
        old: &MenuSnapshot,
        new: &MenuSnapshot,
    ) -> (bool, Vec<(i32, Vec<(&'static str, MessageItem)>)>) {
        let structural = old.root_children != new.root_children
            || old.items.len() != new.items.len()
            || old
                .items
                .iter()
                .zip(&new.items)
                .any(|(a, b)| a.dbus_id != b.dbus_id || a.children != b.children);
        if structural {
            return (true, Vec::new());
        }
        let updates = old
            .items
            .iter()
            .zip(&new.items)
            .filter_map(|(a, b)| {
                let changed: Vec<(&'static str, MessageItem)> = b
                    .properties
                    .iter()
                    .filter(|(key, value)| {
                        a.properties
                            .iter()
                            .all(|(old_key, old_value)| old_key != key || old_value != value)
                    })
                    .cloned()
                    .collect();
                (!changed.is_empty()).then_some((b.dbus_id, changed))
            })
            .collect();
        (false, updates)
    }

    /// 取当前快照；宿主没先 GetLayout 就来查属性/发事件时现建一份兜底。
    fn with_menu_snapshot<R>(
        app: &AppHandle,
        use_it: impl FnOnce(&MenuSnapshot) -> R,
    ) -> R {
        let mut guard = MENU_SNAPSHOT
            .lock()
            .expect("dbusmenu snapshot mutex poisoned");
        if guard.is_none() {
            let state = ui_state(app);
            *guard = Some(build_menu_snapshot(&menu_nodes_for_host(&state)));
        }
        let snapshot = guard.as_ref().expect("快照刚补过，必有");
        use_it(snapshot)
    }

    /// `a{sv}` 小助手：给 GetLayout / GetGroupProperties 造属性字典。
    fn menu_props_dict(properties: Vec<(&'static str, MessageItem)>) -> MessageItem {
        let entries = properties
            .into_iter()
            .map(|(name, value)| {
                (
                    MessageItem::Str(name.to_string()),
                    MessageItem::Variant(Box::new(value)),
                )
            })
            .collect();
        let dict = MessageItemDict::new(
            entries,
            Signature::new("s").expect("静态签名"),
            Signature::new("v").expect("静态签名"),
        )
        .expect("键值签名是写死的 s/v，构造不会失败");
        MessageItem::Dict(dict)
    }

    /// 一个 dbusmenu 节点：`(i id, a{sv} properties, av children)`。
    fn menu_node(
        id: i32,
        properties: Vec<(&'static str, MessageItem)>,
        children: Vec<MessageItem>,
    ) -> MessageItem {
        MessageItem::Struct(vec![
            MessageItem::Int32(id),
            menu_props_dict(properties),
            MessageItem::Array(
                MessageItemArray::new(children, Signature::new("av").expect("静态签名"))
                    .expect("元素都是 v"),
            ),
        ])
    }

    impl MsgHandler for MenuHandler {
        fn handler_type(&self) -> MsgHandlerType {
            MsgHandlerType::MsgType(MessageType::MethodCall)
        }

        fn handle_msg(&mut self, msg: &Message) -> Option<MsgHandlerResult> {
            if msg.path().as_deref() != Some(MENU_PATH) {
                return None;
            }
            let interface = msg.interface().map(|i| i.to_string()).unwrap_or_default();
            let member = msg.member().map(|m| m.to_string()).unwrap_or_default();
            let reply = match (interface.as_str(), member.as_str()) {
                (PROPS_IFACE, "Get") => menu_property_get(msg),
                (PROPS_IFACE, "GetAll") => menu_properties_get_all(msg),
                (INTROSPECT_IFACE, "Introspect") => menu_introspect(msg),
                (MENU_IFACE, "GetLayout") => self.menu_get_layout(msg),
                // 宿主拿到布局后可能再按 id 批量查属性；从快照里按 id 查。
                (MENU_IFACE, "GetGroupProperties") => self.menu_group_properties(msg),
                (MENU_IFACE, "GetProperty") => self.menu_get_property(msg),
                (MENU_IFACE, "AboutToShow") => menu_about_to_show(msg),
                (MENU_IFACE, "AboutToShowGroup") => menu_about_to_show_group(msg),
                (MENU_IFACE, "Event") => self.menu_event(msg),
                (MENU_IFACE, "EventGroup") => self.menu_event_group(msg),
                _ => unknown_method(msg),
            };
            Some(MsgHandlerResult {
                handled: true,
                done: false,
                reply: vec![reply],
            })
        }
    }

    impl MenuHandler {
        /// 宿主发来的菜单事件：`Event(id, eventId, data, timestamp)`。
        ///
        /// - `"clicked"`：用户点了条目 —— 按快照把数字 id 换回动作 id，派发与自绘窗
        ///   **同一份** handle_menu_event（动作可能碰窗口，必须回主线程执行）。
        /// - `"opened"`：宿主弹窗打开瞬间（GNOME 扩展开根菜单必发，dbusMenu.js 实证）
        ///   —— **仅 GNOME 模式**响应：此刻光标就停在托盘图标上，取它当锚点弹自绘菜单
        ///   窗，右键一步直达。Cinnamon/KDE 也可能发 opened，但它们的全量宿主菜单就是
        ///   菜单面，绝不能在这里再弹一扇。
        fn menu_event(&self, msg: &Message) -> Message {
            let items = msg.get_items();
            // id 按规范是 i32，但有的宿主（含我们自己的探针）发 u32 —— 两种都认。
            let id = match items.first() {
                Some(MessageItem::Int32(id)) => Some(*id),
                Some(MessageItem::UInt32(id)) => Some(*id as i32),
                _ => None,
            };
            let event_name = match items.get(1) {
                Some(MessageItem::Str(event)) => Some(event.as_str()),
                _ => None,
            };
            if event_name == Some("opened") && gnome_host() && unified_style() {
                // 仅「GNOME + unified」响应 opened（单条壳弹出 → 弹自绘窗）。
                // classic 的全量菜单就是宿主渲染的最终形态，opened 只当普通事件忽略。
                let (x, y) = cursor_position().unwrap_or_else(|| fallback_anchor(&self.app));
                startup_debug(
                    &self.app,
                    &format!("宿主菜单打开（opened id={id:?}）→ 弹自绘菜单窗 锚点=({x:.0},{y:.0})"),
                );
                let app = self.app.clone();
                let dispatch_app = app.clone();
                if let Err(error) = app.run_on_main_thread(move || {
                    feed_positioner(&dispatch_app, x, y);
                    open_tray_menu(&dispatch_app, x, y);
                    // 宿主弹窗（壳）开着时持有输入 grab，自绘窗的第一次点击会被它吃掉。
                    // 稍候补一个合成 Esc（XTEST 注入，走 grab 通道）把壳收掉、grab 释放，
                    // 之后自绘窗的点击一次到位。延迟留给壳完成弹出动画。
                    std::thread::spawn(|| {
                        std::thread::sleep(Duration::from_millis(150));
                        send_escape_key();
                    });
                }) {
                    startup_debug(
                        &self.app,
                        &format!("派发开菜单回主线程失败: {error}"),
                    );
                }
                return msg.method_return();
            }
            if let (Some(id), Some("clicked")) = (id, event_name) {
                self.dispatch_menu_clicked(id);
            }
            msg.method_return()
        }

        /// 用户点了菜单条目：把 dbusmenu 数字 id 换回动作 id，派发到主线程执行。
        ///
        /// `Event`（单条）与 `EventGroup`（批量）两条通道共用这一段 —— 不同宿主家族
        /// 走的方法不同，语义一模一样（动作可能碰窗口，必须回主线程）。
        fn dispatch_menu_clicked(&self, id: i32) {
            let node_id = with_menu_snapshot(&self.app, |snapshot| {
                snapshot
                    .items
                    .iter()
                    .find(|entry| entry.dbus_id == id)
                    .and_then(|entry| entry.node_id.clone())
            });
            match node_id {
                Some(node_id) => {
                    startup_debug(
                        &self.app,
                        &format!("托盘菜单点击（dbusmenu id={id}）→ 动作 {node_id}"),
                    );
                    let app = self.app.clone();
                    let dispatch_app = app.clone();
                    if let Err(error) = app
                        .run_on_main_thread(move || handle_menu_event(&dispatch_app, &node_id))
                    {
                        startup_debug(
                            &self.app,
                            &format!("派发菜单动作回主线程失败: {error}"),
                        );
                    }
                }
                None => {
                    startup_debug(
                        &self.app,
                        &format!("托盘菜单点击（dbusmenu id={id}）快照里没有，已忽略"),
                    );
                }
            }
        }

        /// `EventGroup(events: a(isvu))` → `ai idErrors`。
        ///
        /// **这是 libdbusmenu 家族点条目的正式通道**：Cinnamon 的 xapp-sn-watcher
        /// （libdbusmenu-gtk3）、MATE 的 mate-xapp-status-applet，以及一切 indicator 系
        /// 宿主，点菜单项发的都是本方法 —— **不是 `Event`**。只有 GNOME 的 appindicators
        /// 扩展（dbusMenu.js）与 KDE 的 libdbusmenu-qt5 用 `Event`。
        ///
        /// 线上签名是 **`a(isvu)`**：每项 `(i 条目id, s 事件名, v 数据, u 时间戳)`。
        /// （本文件早先的内省 XML 把它写成 `a(iisvu)` —— 多了一个 i，错误签名。）
        ///
        /// 2026-10-15 用户实测（Cinnamon，各自方式与全局统一都一样）：菜单能弹、内容也对，
        /// 但**点任何条目都毫无反应**。根因就在这里 —— 老实现只回一个空 idErrors、把批里
        /// 的事件全丢了（注释还写着「批量事件宿主实际不会发」）。复现台
        /// `~/.cache/ab-cin-repro/`（libdbusmenu-gtk3 客户端，xapp-sn-watcher 同款调用
        /// `dbusmenu_gtkmenu_new`）抓到的正是 `EventGroup{ int32 109, "clicked" }`，
        /// 应用侧一条派发日志都没有。
        fn menu_event_group(&self, msg: &Message) -> Message {
            for id in event_group_clicks(msg) {
                self.dispatch_menu_clicked(id);
            }
            let mut reply = msg.method_return();
            let empty_ids = MessageItemArray::new(Vec::new(), Signature::new("ai").expect("静态签名"))
                .expect("元素都是 i");
            reply.append_items(&[MessageItem::Array(empty_ids)]);
            reply
        }

        /// `GetLayout(parentId, depth, propertyNames)` → `(u revision, (ia{sv}av) node)`。
        ///
        /// 每次都按**当前** UiState 现建快照（内容永远新鲜）；parentId 给子树查询用
        /// （宿主展开子菜单时会按子菜单 id 再拉一次），0 = 根。GNOME 模式下模型是
        /// 空的，根自然没有子节点。
        fn menu_get_layout(&self, msg: &Message) -> Message {
            let parent = msg
                .get_items()
                .first()
                .and_then(|item| match item {
                    MessageItem::Int32(id) => Some(*id),
                    MessageItem::UInt32(id) => Some(*id as i32),
                    _ => None,
                })
                .unwrap_or(0);
            let state = ui_state(&self.app);
            let snapshot = build_menu_snapshot(&menu_nodes_for_host(&state));
            let revision = MENU_REVISION.load(Ordering::SeqCst);
            // 序列化：把快照里以 parent 为根的子树整体写成 (ia{sv}av)。
            // 根（0）没有对应条目，属性固定 children-display=submenu。
            fn serialize(snapshot: &MenuSnapshot, dbus_id: i32) -> MessageItem {
                let (properties, children_ids) = match snapshot
                    .items
                    .iter()
                    .find(|entry| entry.dbus_id == dbus_id)
                {
                    None if dbus_id == 0 => (
                        vec![(
                            "children-display",
                            MessageItem::Str("submenu".to_string()),
                        )],
                        snapshot.root_children.clone(),
                    ),
                    Some(entry) => (entry.properties.clone(), entry.children.clone()),
                    None => (Vec::new(), Vec::new()),
                };
                let children = children_ids
                    .iter()
                    .filter_map(|id| {
                        snapshot
                            .items
                            .iter()
                            .find(|entry| entry.dbus_id == *id)
                            .map(|entry| MessageItem::Variant(Box::new(serialize(snapshot, entry.dbus_id))))
                    })
                    .collect();
                menu_node(dbus_id, properties, children)
            }
        let node = serialize(&snapshot, parent);
        *MENU_SNAPSHOT
            .lock()
            .expect("dbusmenu snapshot mutex poisoned") = Some(snapshot);
        // 宿主拿到新内容的那一刻才算"不脏"——清零只在这里（AboutToShow/广播同读这一位）。
        // 漏掉这一步的后果（2026-10-15 实测）：脏标记永真 → 扩展每次展开子菜单
        // AboutToShow(子id) 恒 true → 整树重拉 → _replaceSelf 把刚展开的子菜单行
        // 当场销毁重建 → 折叠展开永远失效。
        MENU_DIRTY.store(false, Ordering::SeqCst);
        let mut reply = msg.method_return();
        reply.append_items(&[MessageItem::UInt32(revision), node]);
        reply
        }

        /// `GetGroupProperties(ids, propertyNames)` → `a(ia{sv})`：按快照逐个查。
        fn menu_group_properties(&self, msg: &Message) -> Message {
            let ids: Vec<i32> = msg
                .get_items()
                .first()
                .and_then(|item| match item {
                    MessageItem::Array(ids) => Some(
                        ids.iter()
                            .filter_map(|id| match id {
                                MessageItem::Int32(id) => Some(*id),
                                MessageItem::UInt32(id) => Some(*id as i32),
                                _ => None,
                            })
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default();
            let entries = with_menu_snapshot(&self.app, |snapshot| {
                ids.iter()
                    .filter_map(|id| {
                        snapshot
                            .items
                            .iter()
                            .find(|entry| entry.dbus_id == *id)
                            .map(|entry| {
                                MessageItem::Struct(vec![
                                    MessageItem::Int32(entry.dbus_id),
                                    menu_props_dict(entry.properties.clone()),
                                ])
                            })
                    })
                    .collect::<Vec<_>>()
            });
            let mut reply = msg.method_return();
            reply.append_items(&[MessageItem::Array(
                MessageItemArray::new(entries, Signature::new("a(ia{sv})").expect("静态签名"))
                    .expect("元素都是 (ia{sv})"),
            )]);
            reply
        }

        /// `GetProperty(id, name)` → `v`：按快照查单个属性。
        fn menu_get_property(&self, msg: &Message) -> Message {
            let (id, property) = msg.read2::<i32, String>().unwrap_or((0, String::new()));
            let value = with_menu_snapshot(&self.app, |snapshot| {
                snapshot
                    .items
                    .iter()
                    .find(|entry| entry.dbus_id == id)
                    .and_then(|entry| {
                        entry
                            .properties
                            .iter()
                            .find(|(name, _)| *name == property)
                            .map(|(_, value)| value.clone())
                    })
                    .unwrap_or_else(|| MessageItem::Str(String::new()))
            });
            let mut reply = msg.method_return();
            reply.append_items(&[MessageItem::Variant(Box::new(value))]);
            reply
        }
    }

    /// `Menu` 属性 `Get`：只认 Version / Status，其余回 UnknownProperty 之外的空变体。
    fn menu_property_get(msg: &Message) -> Message {
        let property = msg
            .read2::<String, String>()
            .map(|(_, property)| property)
            .unwrap_or_default();
        let value = match property.as_str() {
            "Version" => MessageItem::UInt32(3),
            "Status" => MessageItem::Str("normal".to_string()),
            _ => MessageItem::Str(String::new()),
        };
        let mut reply = msg.method_return();
        reply.append_items(&[MessageItem::Variant(Box::new(value))]);
        reply
    }

    /// `Menu` 属性 `GetAll`。
    fn menu_properties_get_all(msg: &Message) -> Message {
        let mut reply = msg.method_return();
        reply.append_items(&[menu_props_dict(vec![
            ("Version", MessageItem::UInt32(3)),
            ("Status", MessageItem::Str("normal".to_string())),
        ])]);
        reply
    }

    /// `AboutToShow(id)` → 是否需要宿主重拉布局：回当前脏标记。
    ///
    /// **不能恒 true**：扩展客户端每次展开子菜单都会先调 AboutToShow(子菜单id)，
    /// 回 true 会促发整树重拉重建，刚展开的子菜单瞬间被销毁又重建 —— 表现就是
    /// 「子菜单展不开」（GNOME 扩展 dbusMenu.js sendAboutToShow → _requestLayoutUpdate
    /// 实锤，2026-10-15）。平时子菜单随便开；内容真变了走脏标记让宿主刷新。
    fn menu_about_to_show(msg: &Message) -> Message {
        let mut reply = msg.method_return();
        reply.append_items(&[MessageItem::Bool(MENU_DIRTY.load(Ordering::SeqCst))]);
        reply
    }

    /// `AboutToShowGroup(ids)` → `(b updateNeeded, ai idErrors)`：语义同上，全局一位。
    fn menu_about_to_show_group(msg: &Message) -> Message {
        let mut reply = msg.method_return();
        let empty_ids = MessageItemArray::new(Vec::new(), Signature::new("ai").expect("静态签名"))
            .expect("元素都是 i");
        reply.append_items(&[
            MessageItem::Bool(MENU_DIRTY.load(Ordering::SeqCst)),
            MessageItem::Array(empty_ids),
        ]);
        reply
    }

    /// 从 `EventGroup` 的入参里取出「点了哪些条目」。
    ///
    /// 线上签名是 **`a(isvu)`**（见 `MenuHandler::menu_event_group` 的注释）。
    /// 拆出来成独立函数是为了能单测 —— 这条通道丢了事件的后果是「点了完全没反应」，
    /// 而它只在这几家的宿主上才走得到，真机上很难靠肉眼发现。
    fn event_group_clicks(msg: &Message) -> Vec<i32> {
        let items = msg.get_items();
        let Some(MessageItem::Array(events)) = items.first() else {
            return Vec::new();
        };
        events
            .iter()
            .filter_map(|event| {
                let MessageItem::Struct(fields) = event else {
                    return None;
                };
                let clicked = matches!(
                    fields.get(1),
                    Some(MessageItem::Str(name)) if name.as_str() == "clicked"
                );
                if !clicked {
                    return None;
                }
                match fields.first() {
                    Some(MessageItem::Int32(id)) => Some(*id),
                    // 有的宿主（含我们自己的探针）把 id 发成 u32 —— 照认
                    Some(MessageItem::UInt32(id)) => Some(*id as i32),
                    _ => None,
                }
            })
            .collect()
    }

    fn menu_introspect(msg: &Message) -> Message {
        let xml = "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \
             \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\"><node>\
             <interface name=\"com.canonical.dbusmenu\">\
             <method name=\"GetLayout\"><arg direction=\"in\" type=\"i\"/><arg direction=\"in\" type=\"i\"/><arg direction=\"in\" type=\"as\"/>\
             <arg direction=\"out\" type=\"u\"/><arg direction=\"out\" type=\"(ia{sv}av)\"/></method>\
             <method name=\"GetGroupProperties\"><arg direction=\"in\" type=\"ai\"/><arg direction=\"in\" type=\"as\"/>\
             <arg direction=\"out\" type=\"a(ia{sv})\"/></method>\
             <method name=\"GetProperty\"><arg direction=\"in\" type=\"i\"/><arg direction=\"in\" type=\"s\"/>\
             <arg direction=\"out\" type=\"v\"/></method>\
             <method name=\"Event\"><arg direction=\"in\" type=\"i\"/><arg direction=\"in\" type=\"s\"/><arg direction=\"in\" type=\"v\"/><arg direction=\"in\" type=\"u\"/></method>\
             <method name=\"EventGroup\"><arg direction=\"in\" type=\"a(isvu)\"/><arg direction=\"out\" type=\"ai\"/></method>\
             <method name=\"AboutToShow\"><arg direction=\"in\" type=\"i\"/><arg direction=\"out\" type=\"b\"/></method>\
             <method name=\"AboutToShowGroup\"><arg direction=\"in\" type=\"ai\"/><arg direction=\"out\" type=\"b\"/><arg direction=\"out\" type=\"ai\"/></method>\
             <property name=\"Version\" type=\"u\" access=\"read\"/>\
             <property name=\"Status\" type=\"s\" access=\"read\"/>\
             </interface></node>";
        let mut reply = msg.method_return();
        reply.append_items(&[MessageItem::Str(xml.to_string())]);
        reply
    }

    /// 属性表（`org.kde.StatusNotifierItem` 的全部属性）。
    /// 取值与类型都照实测可行的探针来（见模块头注释）——尤其 `ItemIsMenu = false`：
    /// 它决定 Cinnamon/KDE 走 `ContextMenu` 回调。`Menu` 恒指向全量 dbusmenu
    /// （本模块只在 unified 方式下启动；类型必须是 object path——严格校验的宿主
    /// 按 GIO 规范读，类型不符会静默丢属性）。
    fn item_properties() -> Vec<(&'static str, MessageItem)> {
        vec![
            ("Id", MessageItem::Str("anyboot".to_string())),
            ("Category", MessageItem::Str("ApplicationStatus".to_string())),
            ("Title", MessageItem::Str("AnyBoot".to_string())),
            ("Status", MessageItem::Str("Active".to_string())),
            ("IconName", MessageItem::Str(String::new())),
            ("IconThemePath", MessageItem::Str(String::new())),
            ("IconPixmap", icon_pixmap()),
            ("AttentionIconName", MessageItem::Str(String::new())),
            ("AttentionIconPixmap", empty_pixmaps()),
            ("AttentionMovieName", MessageItem::Str(String::new())),
            ("OverlayIconName", MessageItem::Str(String::new())),
            ("OverlayIconPixmap", empty_pixmaps()),
            ("ToolTip", tooltip()),
            ("ItemIsMenu", MessageItem::Bool(false)),
            (
                "Menu",
                MessageItem::ObjectPath(DbusPath::new(MENU_PATH).expect("静态路径合法")),
            ),
            ("WindowId", MessageItem::Int32(0)),
        ]
    }

    /// 托盘提示气泡：`(图标名, 图标像素, 标题, 说明)`。
    /// 它是**静态**的 —— 状态（在不在扫描、下次启动是哪个）都在菜单第一行里。
    fn tooltip() -> MessageItem {
        MessageItem::Struct(vec![
            MessageItem::Str(String::new()),
            empty_pixmaps(),
            MessageItem::Str("AnyBoot".to_string()),
            MessageItem::Str("选择下次启动的系统".to_string()),
        ])
    }

    /// `IconPixmap`：应用图标，**按原尺寸发**（512×512）。
    ///
    /// 面板自己会把它缩到二十几像素，那边的缩放器比我们手写的更靠谱；代价只是注册那一次
    /// 一条 1MB 的消息（一次性开销，之后这个属性不再传）。
    fn icon_pixmap() -> MessageItem {
        let image = create_tray_image();
        let pixels = argb32_network(image.rgba());
        MessageItem::Array(pixmaps(vec![(image.width(), image.height(), pixels)]))
    }

    /// 空像素表（Attention / Overlay 两个属性用）。
    fn empty_pixmaps() -> MessageItem {
        MessageItem::Array(pixmaps(Vec::new()))
    }

    /// `a(iiay)`：宽、高、像素三段一组的数组。
    ///
    /// 坑：`MessageItemArray::new` 要的是**整条数组签名**（带前导 `a`），不是元素签名 ——
    /// 它自己会砍掉首个字节去比元素（dbus 0.9 `messageitem.rs:78`，传 `"y"` 直接
    /// `Err(InvalidSignature)`）。上一版就是在 `expect` 上崩了，SNI 线程当场死掉、
    /// 面板拿不到属性、图标不出现（表现为「注册成功但没图标」，见当日日志）。
    fn pixmaps(items: Vec<(u32, u32, Vec<u8>)>) -> MessageItemArray {
        let entries = items
            .into_iter()
            .map(|(width, height, pixels)| {
                MessageItem::Struct(vec![
                    MessageItem::Int32(width as i32),
                    MessageItem::Int32(height as i32),
                    MessageItem::Array(
                        MessageItemArray::new(
                            pixels.into_iter().map(MessageItem::Byte).collect(),
                            Signature::new("ay").expect("静态签名"),
                        )
                        .expect("元素都是 byte"),
                    ),
                ])
            })
            .collect();
        MessageItemArray::new(entries, Signature::new("a(iiay)").expect("静态签名"))
            .expect("元素都是 (iiay)")
    }

    /// RGBA（tauri 的排布）→ SNI 要的 **ARGB32 网络字节序**（每像素 A, R, G, B）。
    fn argb32_network(rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(rgba.len());
        for pixel in rgba.chunks_exact(4) {
            out.push(pixel[3]);
            out.push(pixel[0]);
            out.push(pixel[1]);
            out.push(pixel[2]);
        }
        out
    }

    /// 单个属性的值（`Get` 的返回体；外面还要套一层 variant）。
    ///
    /// 不认识的属性回空串（照探针的做法；回错误会让某些宿主整段属性读失败）。
    fn property_value(property: &str) -> MessageItem {
        item_properties()
            .into_iter()
            .find(|(name, _)| *name == property)
            .map(|(_, value)| value)
            .unwrap_or_else(|| MessageItem::Str(String::new()))
    }

    /// 整张属性表 → `a{sv}`（`GetAll` 的返回体）。抽出来是为了能在单测里直接按线格式写一遍
    /// —— `method_return()` 要求原消息带真 serial，单测造不出合法的回复消息（libdbus 会
    /// `reply_serial != 0` 断言失败），所以「可编码」这一条只能测返回体本身。
    fn properties_dict() -> MessageItem {
        let entries = item_properties()
            .into_iter()
            .map(|(name, value)| {
                (
                    MessageItem::Str(name.to_string()),
                    MessageItem::Variant(Box::new(value)),
                )
            })
            .collect();
        let dict = MessageItemDict::new(
            entries,
            Signature::new("s").expect("静态签名"),
            Signature::new("v").expect("静态签名"),
        )
        .expect("键值签名是写死的 s/v，构造不会失败");
        MessageItem::Dict(dict)
    }

    fn properties_get(msg: &Message) -> Message {
        let property = msg
            .read2::<String, String>()
            .map(|(_, property)| property)
            .unwrap_or_default();
        let mut reply = msg.method_return();
        reply.append_items(&[MessageItem::Variant(Box::new(property_value(&property)))]);
        reply
    }

    fn properties_get_all(msg: &Message) -> Message {
        let mut reply = msg.method_return();
        reply.append_items(&[properties_dict()]);
        reply
    }

    fn introspect(msg: &Message) -> Message {
        // Menu 的类型必须是 o（object path）——严格校验的宿主（GNOME 扩展按 GIO
        // 规范读）类型不符会静默丢属性。
        let xml = format!(
            "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \
             \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\"><node>\
             <interface name=\"{ITEM_IFACE}\">\
             <method name=\"Activate\"><arg direction=\"in\" type=\"i\"/><arg direction=\"in\" type=\"i\"/></method>\
             <method name=\"SecondaryActivate\"><arg direction=\"in\" type=\"i\"/><arg direction=\"in\" type=\"i\"/></method>\
             <method name=\"ContextMenu\"><arg direction=\"in\" type=\"i\"/><arg direction=\"in\" type=\"i\"/></method>\
             <method name=\"Scroll\"><arg direction=\"in\" type=\"i\"/><arg direction=\"in\" type=\"s\"/></method>\
             <property name=\"ItemIsMenu\" type=\"b\" access=\"read\"/>\
             <property name=\"Menu\" type=\"o\" access=\"read\"/>\
             </interface></node>"
        );
        let mut reply = msg.method_return();
        reply.append_items(&[MessageItem::Str(xml)]);
        reply
    }

    /// 不认识的方法：回一个标准的 UnknownMethod 错误。
    ///
    /// **必须回**：我们的对象路径注册了 vtable，libdbus 就不再自动回「没有这个对象/方法」，
    /// 不回的话调用方会一直等到超时。
    fn unknown_method(msg: &Message) -> Message {
        let name = ErrorName::new("org.freedesktop.DBus.Error.UnknownMethod")
            .expect("这是个合法的错误名");
        let text = CStr::from_bytes_with_nul(b"AnyBoot: unknown method\0")
            .expect("静态字面量，一定带 \\0");
        msg.error(&name, text)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn pixels_are_reordered_to_argb_network_order() {
            // 两个像素：不透明红、半透明蓝
            let rgba = [255, 0, 0, 255, 0, 0, 255, 128];
            assert_eq!(argb32_network(&rgba), vec![255, 255, 0, 0, 128, 0, 0, 255]);
        }

        #[test]
        fn trailing_partial_pixel_is_ignored() {
            // 末尾多出来的半个像素不参与（chunks_exact 的行为，靠这条钉住）
            assert_eq!(argb32_network(&[1, 2, 3, 4, 9]), vec![4, 1, 2, 3]);
        }

        /// 踩过坑：`MessageItemArray::new` 收的是**整条数组签名**（带前导 `a`），
        /// 传元素签名（`"y"` / `"(iiay)"`）会被它判成 `InvalidSignature`。
        /// 上一版就在 `.expect()` 上崩了 —— 而且是**宿主来读属性时**才崩，
        /// SNI 线程当场死掉，表现为「注册成功但没有图标」，极难回头找。
        /// 这条把两个签名就地钉住，别再退回去。
        #[test]
        fn pixmap_arrays_carry_the_full_array_signature() {
            let groups = pixmaps(vec![(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8])]);
            assert_eq!(groups.signature().to_string(), "a(iiay)");
            let MessageItem::Struct(fields) = &groups[0] else {
                panic!("a(iiay) 的元素应是 struct");
            };
            assert_eq!(fields.len(), 3, "元素是 (宽, 高, 像素)");
            assert_eq!(fields[0].signature().to_string(), "i");
            assert_eq!(fields[1].signature().to_string(), "i");
            let MessageItem::Array(bytes) = &fields[2] else {
                panic!("第三段应是 ay");
            };
            // 这里正是崩点：签名若不是 ay，构造阶段就已经 panic 了
            assert_eq!(bytes.signature().to_string(), "ay");
            assert_eq!(bytes.len(), 8);

            // 空表（Attention / Overlay 两个属性用）也要构造得出来
            let MessageItem::Array(empty) = empty_pixmaps() else {
                panic!("空像素表应是 a(iiay)");
            };
            assert_eq!(empty.len(), 0);
            assert_eq!(empty.signature().to_string(), "a(iiay)");
        }

        /// ③（点选择项不关菜单）成立的前提在 `ItemIsMenu=false`：Cinnamon/KDE 据此把
        /// 点击坐标回调给我们（Activate/ContextMenu）。`Menu` 恒指向全量 dbusmenu
        /// （GNOME 的 appindicators 扩展点击只认 dbusmenu、从不回调 ContextMenu）。
        /// 这两条取值一旦变动，各端的行为都会变 —— 必须钉住。
        #[test]
        fn item_properties_keep_the_activate_contract() {
            let props = item_properties();
            let value = |name: &str| {
                props
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_else(|| panic!("缺属性 {name}"))
            };
            assert_eq!(value("ItemIsMenu"), MessageItem::Bool(false));
            assert_eq!(
                value("Menu"),
                MessageItem::ObjectPath(DbusPath::new(MENU_PATH).expect("静态路径合法"))
            );
            assert_eq!(value("Status"), MessageItem::Str("Active".to_string()));
            assert_eq!(
                value("Category"),
                MessageItem::Str("ApplicationStatus".to_string())
            );

            // 图标像素真带着，尺寸就是应用图标本身（面板靠它画图标）
            let image = create_tray_image();
            let MessageItem::Array(groups) = value("IconPixmap") else {
                panic!("IconPixmap 应是 a(iiay)");
            };
            assert_eq!(groups.len(), 1, "只发一张应用图标");
            let MessageItem::Struct(fields) = &groups[0] else {
                panic!("元素应是 (iiay)");
            };
            let (MessageItem::Int32(width), MessageItem::Int32(height)) = (&fields[0], &fields[1])
            else {
                panic!("前两段应是 i");
            };
            assert_eq!((*width as u32, *height as u32), (image.width(), image.height()));
            let MessageItem::Array(bytes) = &fields[2] else {
                panic!("第三段应是 ay");
            };
            assert_eq!(bytes.len(), (image.width() * image.height() * 4) as usize);

            // ToolTip 是四段 (图标名, 图标像素, 标题, 说明)
            let MessageItem::Struct(tip) = value("ToolTip") else {
                panic!("ToolTip 应是 struct");
            };
            assert_eq!(tip.len(), 4);
        }

        /// 属性表能按 dbus 线格式写进一条消息、又能原样读回来 = 宿主一定读得动。
        /// （签名不合法的话，这条在 append / 读回阶段就炸，不必等真机。）
        #[test]
        fn property_bodies_are_wire_encodable() {
            // GetAll 的返回体：a{sv}
            let mut msg =
                Message::new_method_call(":1.1", ITEM_PATH, PROPS_IFACE, "GetAll").expect("构造调用");
            msg.append_items(&[properties_dict()]);
            let Some(MessageItem::Dict(entries)) = msg.get1::<MessageItem>() else {
                panic!("GetAll 应回一个 a{{sv}}");
            };
            assert_eq!(entries.signature().to_string(), "a{sv}");
            assert_eq!(entries.len(), item_properties().len());
            // 键都是 s、值都是 v（宿主按签名取属性，错一个整张表就读不成）
            for (key, value) in entries.iter() {
                assert_eq!(key.signature().to_string(), "s");
                assert_eq!(value.signature().to_string(), "v");
            }

            // Get 的返回体是套一层的 variant
            let mut msg =
                Message::new_method_call(":1.1", ITEM_PATH, PROPS_IFACE, "Get").expect("构造调用");
            msg.append_items(&[MessageItem::Variant(Box::new(property_value("ItemIsMenu")))]);
            assert_eq!(
                msg.get1::<MessageItem>(),
                Some(MessageItem::Variant(Box::new(MessageItem::Bool(false))))
            );
            assert_eq!(
                property_value("Menu"),
                MessageItem::ObjectPath(DbusPath::new(MENU_PATH).expect("静态路径合法"))
            );
            // 不认识的属性回空串
            assert_eq!(
                property_value("NoSuchProperty"),
                MessageItem::Str(String::new())
            );
        }

        /// GetLayout 回执 `(u(ia{sv}av))` 的线格式必须能编码 —— dbusmenu 节点是三层嵌套
        /// （struct 套 dict 套 av），签名传错宿主会整段解析失败（IconPixmap 同款坑钉过一次）。
        #[test]
        fn menu_layout_encodes_to_the_dbusmenu_signature() {
            let layout = menu_node(
                0,
                vec![("children-display", MessageItem::Str("submenu".to_string()))],
                Vec::new(),
            );
            assert_eq!(layout.signature().to_string(), "(ia{sv}av)");

            let dict = menu_props_dict(vec![
                ("type", MessageItem::Str("standard".to_string())),
                ("label", MessageItem::Str("示例".to_string())),
            ]);
            assert_eq!(dict.signature().to_string(), "a{sv}");
        }

        /// 快照构建：模型树 → dbusmenu 编号顺序分配、动作 id 对得上、
        /// 勾选/分隔线/子菜单的属性表各就位。Event(id) ↔ node_id 的换算靠它。
        #[test]
        fn menu_snapshot_maps_ids_and_properties() {
            let nodes = vec![
                menu_item("show-main", "显示主窗口", true),
                menu_header("sys-head", "系统"),
                MenuNode {
                    keeps_open: true,
                    ..menu_marked_choice_item("switch:arch", "Arch", true, true)
                },
                MenuNode {
                    keeps_open: true,
                    ..menu_marked_choice_item("switch:mint", "Mint", true, false)
                },
                menu_separator(),
                menu_submenu(
                    "sub",
                    "子菜单",
                    vec![menu_item("sub-action", "子项", true)],
                ),
            ];
            let snapshot = build_menu_snapshot(&nodes);
            // 先序编号：1..=7（子菜单的子项夹在中间），0 留给根
            let ids: Vec<i32> = snapshot.items.iter().map(|entry| entry.dbus_id).collect();
            assert_eq!(ids, vec![1, 2, 3, 4, 5, 6, 7]);
            // 根只挂 6 个顶层节点（子项在子菜单里）
            assert_eq!(snapshot.root_children, vec![1, 2, 3, 4, 5, 6]);
            let node = |id: i32| {
                snapshot
                    .items
                    .iter()
                    .find(|entry| entry.dbus_id == id)
                    .unwrap_or_else(|| panic!("缺节点 {id}"))
            };
            let property = |entry: &SnapshotNode, name: &str| {
                entry
                    .properties
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_else(|| panic!("节点 {} 缺属性 {name}", entry.dbus_id))
            };
            // 动作 id 直通（Event 派发的依据）
            assert_eq!(node(1).node_id.as_deref(), Some("show-main"));
            assert_eq!(node(2).node_id.as_deref(), Some("sys-head"));
            // 选中项：标记画进 label 文字（radio_label），不导出 toggle 属性 ——
            // 宿主（KDE/Qt）对 checkable 条目先本地翻转再通知，协议又没有单选组
            // 概念，原生勾选在这条链路上必然出错（2026-10-15 实测「点不动/变多选」）。
            assert_eq!(
                property(node(3), "label"),
                MessageItem::Str("◉ Arch".to_string())
            );
            assert_eq!(
                property(node(4), "label"),
                MessageItem::Str("○ Mint".to_string())
            );
            assert!(node(3).properties.iter().all(|(key, _)| !key.starts_with("toggle-")));
            assert!(node(4).properties.iter().all(|(key, _)| !key.starts_with("toggle-")));
            // 分隔线：type=separator、没有动作 id
            assert_eq!(property(node(5), "type"), MessageItem::Str("separator".to_string()));
            assert_eq!(node(5).node_id, None);
            // 子菜单：children 挂上，且属性表自带 children-display=submenu ——
            // GNOME 扩展用 GetGroupProperties 的回应洗一遍属性（resetProperties），
            // 这枚标记不在快照里就会被洗掉，下拉列表展不开（真机实测）。
            assert_eq!(node(6).children, vec![7]);
            assert_eq!(
                property(node(6), "children-display"),
                MessageItem::Str("submenu".to_string())
            );
            // 非子菜单条目不该带这枚标记
            assert!(node(1).properties.iter().all(|(key, _)| *key != "children-display"));
            assert_eq!(node(7).node_id.as_deref(), Some("sub-action"));
        }

        /// GNOME 模式（menu_nodes_for_host 回空）下导出的就是空根：宿主弹窗没有
        /// 内容可渲染，靠 opened 事件弹自绘窗。
        #[test]
        fn menu_snapshot_empty_nodes_yields_empty_root() {
            let snapshot = build_menu_snapshot(&[]);
            assert!(snapshot.items.is_empty());
            assert!(snapshot.root_children.is_empty());
        }

        /// **`EventGroup` 里的事件必须被认出来。**
        ///
        /// libdbusmenu 家族（Cinnamon 的 xapp-sn-watcher / MATE / indicator 系）点菜单项
        /// 发的就是这条 `a(isvu)`；老实现把它整个丢掉 → 菜单能弹、点条目毫无反应
        /// （2026-10-15 用户实测，各自方式与全局统一都一样）。
        ///
        /// 顺便钉住线上签名：写成 `a(iisvu)`（多一个 i）会构造失败 —— 这正是老代码
        /// 内省 XML 里的错签名，也是「当初以为宿主不发批量事件」的来源。
        #[test]
        fn event_group_clicks_reads_the_wire_signature() {
            fn event(id: i32, name: &str) -> MessageItem {
                MessageItem::Struct(vec![
                    MessageItem::Int32(id),
                    MessageItem::Str(name.to_string()),
                    MessageItem::Variant(Box::new(MessageItem::Int32(0))),
                    MessageItem::UInt32(0),
                ])
            }
            let group = |entries: Vec<MessageItem>| {
                let mut msg = Message::new_method_call(":1.1", MENU_PATH, MENU_IFACE, "EventGroup")
                    .expect("构造调用");
                msg.append_items(&[MessageItem::Array(
                    MessageItemArray::new(entries, Signature::new("a(isvu)").expect("静态签名"))
                        .expect("元素都是 (isvu)"),
                )]);
                msg
            };

            // 单个点击：id 原样取出
            assert_eq!(event_group_clicks(&group(vec![event(109, "clicked")])), vec![109]);
            // 一批：只取 clicked，非 clicked 的同批事件不派发
            assert_eq!(
                event_group_clicks(&group(vec![
                    event(3, "hovered"),
                    event(109, "clicked"),
                    event(0, "opened"),
                ])),
                vec![109]
            );
            // 空批 / 不是数组：不 panic，什么都不派发
            assert!(event_group_clicks(&group(Vec::new())).is_empty());
            let empty = Message::new_method_call(":1.1", MENU_PATH, MENU_IFACE, "EventGroup")
                .expect("构造调用");
            assert!(event_group_clicks(&empty).is_empty());
            // 错签名 `a(iisvu)` 构造即失败 —— 老 XML 里的写法，钉住别再回去
            assert!(MessageItemArray::new(
                vec![MessageItem::Struct(vec![
                    MessageItem::Int32(1),
                    MessageItem::Int32(1),
                    MessageItem::Str("clicked".to_string()),
                    MessageItem::Variant(Box::new(MessageItem::Int32(0))),
                    MessageItem::UInt32(0),
                ])],
                Signature::new("a(isvu)").expect("静态签名")
            )
            .is_err());
        }

        /// 差分分流：勾选标记挪动 = 属性变（只推 ItemsPropertiesUpdated，结构位 false）；
        /// 结构变了（增删导致先序 id 洗牌）= 结构变 + 属性列表空（只发 LayoutUpdated）；
        /// 完全一致 = 两者都空（无操作点击不许白发信号）。
        #[test]
        fn menu_property_diff_splits_structure_from_labels() {
            let nodes = vec![
                menu_marked_choice_item("switch:arch", "Arch", true, true),
                menu_marked_choice_item("switch:mint", "Mint", true, false),
            ];
            let before = build_menu_snapshot(&nodes);
            // 树没变：勾选从 Arch 挪到 Mint
            let flipped = build_menu_snapshot(&[
                menu_marked_choice_item("switch:arch", "Arch", true, false),
                menu_marked_choice_item("switch:mint", "Mint", true, true),
            ]);
            let (structural, updates) = menu_property_diff(&before, &flipped);
            assert!(!structural);
            assert_eq!(updates.len(), 2);
            assert_eq!(updates[0].0, 1);
            assert_eq!(
                updates[0].1,
                vec![("label", MessageItem::Str("○ Arch".to_string()))]
            );
            assert_eq!(updates[1].0, 2);
            assert_eq!(
                updates[1].1,
                vec![("label", MessageItem::Str("◉ Mint".to_string()))]
            );
            // 完全一致：结构位 false、无更新 —— 什么都不发
            let (structural, updates) = menu_property_diff(&before, &before);
            assert!(!structural);
            assert!(updates.is_empty());
            // 结构变了（多了一条）：结构位 true、属性列表空
            let grown = build_menu_snapshot(&[
                nodes[0].clone(),
                nodes[1].clone(),
                menu_item("show", "显示主窗口", true),
            ]);
            let (structural, updates) = menu_property_diff(&before, &grown);
            assert!(structural);
            assert!(updates.is_empty());
        }

        /// ItemsPropertiesUpdated 信号体的线格式必须能编码：`a(ia{sv})a(ias)`
        /// （后半「被移除的属性」恒空数组；**必须是 a(ias)** —— Qt 宿主按签名精确
        /// 匹配，写成 a(iias) 的信号会被静默丢弃，勾选回显不刷新）。
        #[test]
        fn menu_property_update_bodies_are_wire_encodable() {
            let updated = MessageItem::Array(
                MessageItemArray::new(
                    vec![MessageItem::Struct(vec![
                        MessageItem::Int32(3),
                        menu_props_dict(vec![("label", MessageItem::Str("◉ Mint".to_string()))]),
                    ])],
                    Signature::new("a(ia{sv})").expect("静态签名"),
                )
                .expect("元素都是 (ia{sv})"),
            );
            assert_eq!(updated.signature().to_string(), "a(ia{sv})");
            let removed = MessageItem::Array(
                MessageItemArray::new(Vec::new(), Signature::new("a(ias)").expect("静态签名"))
                    .expect("空数组"),
            );
            assert_eq!(removed.signature().to_string(), "a(ias)");
        }
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
    fn auto_hide_near_edge_matches_user_semantics() {
        // 工作区 0,0 → 1920×1040
        let work = (0, 0, 1920, 1040);
        // 贴左边（间隙 10 ≤ 24）：可隐藏
        assert!(auto_hide_near_edge(
            10, 100, 400, 600, work.0, work.1, work.2, work.3, 24
        ));
        // 屏幕正中央（四边间隙都远大于 24）：不可隐藏
        assert!(!auto_hide_near_edge(
            760, 220, 400, 600, work.0, work.1, work.2, work.3, 24
        ));
        // 间隙 30 > 24：不可隐藏；阈值放大到 32 就可以
        assert!(!auto_hide_near_edge(
            30, 100, 400, 600, work.0, work.1, work.2, work.3, 24
        ));
        assert!(auto_hide_near_edge(
            30, 100, 400, 600, work.0, work.1, work.2, work.3, 32
        ));
        // 间隙为负（已经压在边上/越界）：算贴边
        assert!(auto_hide_near_edge(
            -5, 100, 400, 600, work.0, work.1, work.2, work.3, 0
        ));
        // 阈值 0：只有真正贴到（间隙 ≤ 0）才隐藏
        assert!(!auto_hide_near_edge(
            1, 100, 400, 600, work.0, work.1, work.2, work.3, 0
        ));
        // 底边贴边：y+h = 1040
        assert!(auto_hide_near_edge(
            100, 440, 400, 600, work.0, work.1, work.2, work.3, 24
        ));
    }

    #[test]
    fn strip_geometry_keeps_window_edge_length_and_shrinks_thickness() {
        // 工作区 0,0 → 1920×1040；主窗 client 460×900，原**客户区**位置 (100, 40)，厚度 6
        // （这里的 x/y 必须是客户区坐标：调用方传窗框坐标的话，左/右侧贴边会偏一个标题栏，
        //  见 strip_geometry 的说明。）
        let (size, target) =
            strip_geometry(DockEdge::Bottom, 0, 0, 1920, 1040, 460, 900, 100, 40, 6, 0);
        // 贴底边：窗口保持主窗宽 460、高缩成 6；x 保持主窗客户区原位，y 贴底
        assert_eq!(size.width, 460);
        assert_eq!(size.height, 6);
        assert_eq!(target, (100, 1034));

        // 贴顶边：y = 工作区顶
        let (_, target) = strip_geometry(DockEdge::Top, 0, 0, 1920, 1040, 460, 900, 100, 40, 6, 0);
        assert_eq!(target, (100, 0));

        // 贴左缘：窗口保持主窗高 900、宽缩成 6；y 保持主窗客户区原位，x 贴左
        let (size, target) =
            strip_geometry(DockEdge::Left, 0, 0, 1920, 1040, 460, 900, 100, 40, 6, 0);
        assert_eq!(size.width, 6);
        assert_eq!(size.height, 900);
        assert_eq!(target, (0, 40));

        // 贴右缘：x = 1920 - 6
        let (_, target) = strip_geometry(DockEdge::Right, 0, 0, 1920, 1040, 460, 900, 100, 40, 6, 0);
        assert_eq!(target, (1914, 40));

        // 工作区带偏移（副屏/面板）：贴的是工作区边，不是屏幕边
        let (_, target) =
            strip_geometry(DockEdge::Left, 1600, 40, 1280, 960, 460, 900, 1800, 90, 6, 0);
        assert_eq!(target, (1600, 90));
        let (_, target) =
            strip_geometry(DockEdge::Top, 1600, 40, 1280, 960, 460, 900, 1800, 90, 6, 0);
        assert_eq!(target, (1800, 40));
    }

    /// 启动同步的挂载点判据：/boot 本身或 /boot 下任意目录都算（fstab 统一
    /// 方案），/media 自动挂载、/ 根挂载（= 目录没挂上）都不算。
    #[cfg(target_os = "linux")]
    #[test]
    fn under_boot_accepts_only_boot_mountpoints() {
        assert!(under_boot("/boot"));
        assert!(under_boot("/boot/efi"));
        assert!(under_boot("/boot/efi2"));
        assert!(under_boot("/boot/efi/cachyos"));
        assert!(!under_boot("/"));
        assert!(!under_boot("/bootloader"));
        assert!(!under_boot("/media/user/ESP"));
        assert!(!under_boot("/mnt/esp"));
        assert!(!under_boot("/efi"));
    }

    /// 「同步」按钮的挂载条件（用户定案）：分区挂在 /boot 或 /boot/efi 才同步；
    /// /boot 下的其他子目录、/media、没挂都不算。
    #[cfg(target_os = "linux")]
    #[test]
    fn sync_mount_ok_accepts_only_boot_and_boot_efi() {
        assert!(sync_mount_ok("/boot"));
        assert!(sync_mount_ok("/boot/efi"));
        // /boot 下的其他位置、自动挂载、别的位置都不同步
        assert!(!sync_mount_ok("/boot/efi2"));
        assert!(!sync_mount_ok("/boot/efi/cachyos"));
        assert!(!sync_mount_ok("/media/user/ESP"));
        assert!(!sync_mount_ok("/mnt/esp"));
        assert!(!sync_mount_ok("/efi"));
    }

    /// 白条距边界的缝隙（gap）：停靠落点沿法线方向离边这条缝，另一轴照旧。
    #[test]
    fn strip_geometry_offsets_by_gap_along_edge_normal() {
        // 左缘 gap=10：x = 工作区左 + 10，y 保持客户区原位
        let (_, target) =
            strip_geometry(DockEdge::Left, 0, 0, 1920, 1040, 460, 900, 100, 40, 6, 10);
        assert_eq!(target, (10, 40));
        // 右缘 gap=10：x = 工作区右 - 厚度 - 10（白条右沿离右边 10）
        let (_, target) =
            strip_geometry(DockEdge::Right, 0, 0, 1920, 1040, 460, 900, 100, 40, 6, 10);
        assert_eq!(target, (1904, 40));
        // 顶边 gap=8：y = 工作区顶 + 8
        let (_, target) = strip_geometry(DockEdge::Top, 0, 0, 1920, 1040, 460, 900, 100, 40, 6, 8);
        assert_eq!(target, (100, 8));
        // 底边 gap=8：y = 工作区底 - 厚度 - 8（白条下沿离底边 8）
        let (_, target) =
            strip_geometry(DockEdge::Bottom, 0, 0, 1920, 1040, 460, 900, 100, 40, 6, 8);
        assert_eq!(target, (100, 1026));
    }

    #[test]
    fn redock_edge_keeps_last_edge_on_distance_tie() {
        // dock-cycle 复现台实测：Left undock 后 Muffin 重装饰把 client y 97 掰到
        // 100，460x900 客户区在 1600x1000 工作区里离左缘 0、离底缘 0 —— 打平。
        // 打平不是换边的理由：沿用上一轮，左右贴边绝不因此翻成底边贴边。
        assert_eq!(
            redock_edge(Some(DockEdge::Left), 0, 100, 460, 900, 0, 0, 1600, 1000),
            DockEdge::Left
        );
    }

    #[test]
    fn redock_edge_switches_only_when_strictly_closer() {
        // 用户把窗口拖到右缘（y=50，离底还有距离）：右缘 0 严格更近 → 跟着换
        assert_eq!(
            redock_edge(Some(DockEdge::Left), 1140, 50, 460, 900, 0, 0, 1600, 1000),
            DockEdge::Right
        );
        // 拖到中下部贴底：底缘 0 严格更近 → 换底
        assert_eq!(
            redock_edge(Some(DockEdge::Left), 600, 100, 460, 900, 0, 0, 1600, 1000),
            DockEdge::Bottom
        );
        // 没贴过边 → 纯几何，与 nearest_edge 一致
        assert_eq!(
            redock_edge(None, 700, 500, 460, 900, 0, 0, 1600, 1000),
            nearest_edge(700, 500, 460, 900, 0, 0, 1600, 1000)
        );
    }

    #[test]
    fn hotzone_hit_matches_band_geometry() {
        // 工作区 1600x1000 @ (0,0)，窗口贴顶边（x=100 宽 800，y=0 高 40 槽位），
        // 热区厚 40（默认 = 槽位，行为与旧的「窗口矩形判定」一致）。
        let hit = |edge, hz, cx, cy| {
            hotzone_hit(edge, 0, 0, 1600, 1000, 100, 0, 800, 40, hz, cx, cy)
        };
        // Top：带内 y ∈ [0, 40)，沿边轴 x ∈ [100, 900)
        assert!(hit(DockEdge::Top, 40, 500, 0));
        assert!(hit(DockEdge::Top, 40, 500, 39));
        assert!(!hit(DockEdge::Top, 40, 500, 40)); // 带外（恰好在边界上）
        assert!(!hit(DockEdge::Top, 40, 99, 10)); // 沿边轴窗口范围外
        assert!(!hit(DockEdge::Top, 40, 900, 10)); // 半开区间右端不含
        // 厚度独立于窗口：热区 100 > 槽位 40，鼠标在窗口外（y=80）照样命中
        assert!(hit(DockEdge::Top, 100, 500, 80));
        // Bottom：带 y ∈ (1000-40, 1000]，窗口贴底 y=960 高 40
        assert!(hotzone_hit(
            DockEdge::Bottom, 0, 0, 1600, 1000, 100, 960, 800, 40, 40, 500, 999
        ));
        assert!(!hotzone_hit(
            DockEdge::Bottom, 0, 0, 1600, 1000, 100, 960, 800, 40, 40, 500, 959
        ));
        // Left：带 x ∈ [0, 40)，窗口贴左 y=200 高 600
        assert!(hotzone_hit(
            DockEdge::Left, 0, 0, 1600, 1000, 0, 200, 40, 600, 40, 20, 500
        ));
        assert!(!hotzone_hit(
            DockEdge::Left, 0, 0, 1600, 1000, 0, 200, 40, 600, 40, 50, 500
        ));
        // Right：带 x ∈ [1600-40, 1600)，窗口贴右 x=1560（半开区间与其它边一致）
        assert!(hotzone_hit(
            DockEdge::Right, 0, 0, 1600, 1000, 1560, 200, 40, 600, 40, 1560, 500
        ));
        assert!(!hotzone_hit(
            DockEdge::Right, 0, 0, 1600, 1000, 1560, 200, 40, 600, 40, 1559, 500
        ));
    }

    #[test]
    fn dock_axis_position_absorbs_wm_nudge_only() {
        // Muffin 重装饰实测 +3px（client y 97 → 100）：≤ 阈值视为微调，沿上一轮
        // 停靠位 —— 不吸收的话微调每轮进 dock_pos，每开合一次窗口就下移一点。
        assert_eq!(
            dock_axis_position(100, 97, AUTO_HIDE_WM_NUDGE_PX),
            97
        );
        // 用户真拖了 15px：尊重当前读数
        assert_eq!(
            dock_axis_position(112, 97, AUTO_HIDE_WM_NUDGE_PX),
            112
        );
    }

    /// undock 重锚（牛顿式自校准）：装饰态窗口上 set_position 的坐标是窗框
    /// 原点，客户区 = 请求 + 标题栏（真机 Cinnamon 2x：请求 428 → 实况 492，
    /// 每轮 +64 爬移）。实测偏差全额反扣进修正量，一步收敛；修正量写回后
    /// 下一轮首发即准。容差内（复现台 +3px）直接收敛；额度用完放弃。
    #[test]
    fn undock_reanchor_newton_corrects_frame_origin_semantics() {
        let plan = AutoHideReanchor {
            target: (0, 428),
            offset: (0, 0),
            tries: 0,
            last_live: None,
            noops: 0,
        };
        // 真机首轮：请求 428、实况 492（+64 标题栏）→ 修正 −64，补发请求 364
        let (plan2, request) = match undock_reanchor_action(plan, 0, 492) {
            UndockReanchor::Set(plan, request) => (plan, request),
            other => panic!("expected Set, got {other:?}"),
        };
        assert_eq!(plan2.offset, (0, -64));
        assert_eq!(plan2.tries, 1);
        assert_eq!(request, (0, 364));
        // 补发请求 364 → 客户区 = 364 + 64 = 428 = 期望 → 收敛（修正量写回）
        assert_eq!(
            undock_reanchor_action(plan2, 0, 428),
            UndockReanchor::Settled
        );
        // 带修正量的下一轮：请求 364 → 实况 428 = 期望 → 首拍即收敛
        let armed = AutoHideReanchor {
            target: (0, 428),
            offset: (0, -64),
            tries: 0,
            last_live: None,
            noops: 0,
        };
        assert_eq!(
            undock_reanchor_action(armed, 0, 428),
            UndockReanchor::Settled
        );
        // 容差内照旧直接收敛（复现台 +3px、Windows 落点精确）
        assert_eq!(
            undock_reanchor_action(plan, 0, 428 + AUTO_HIDE_WM_NUDGE_PX),
            UndockReanchor::Settled
        );
        // 额度用完仍偏 → 放弃（防 WM 不配合时永久抖动；钳制类非线性偏差
        // 也不写回修正量）
        let exhausted = AutoHideReanchor {
            target: (0, 428),
            offset: (0, 0),
            tries: AUTO_HIDE_REANCHOR_TRIES,
            last_live: None,
            noops: 0,
        };
        assert_eq!(
            undock_reanchor_action(exhausted, 0, 492),
            UndockReanchor::GiveUp
        );
    }

    /// 牛顿步的防失控保险丝（多屏实测翻车：target 落在窗口够不到的区域，
    /// 每拍 offset += target−live 一路累加到 11520、请求飞到 15416 ——
    /// 多屏桌面上那些请求会真落到远处的屏上）。实况纹丝不动时**不叠**
    /// 修正量、照发同一请求再等一拍；连续两拍没动静就止损放弃。
    #[test]
    fn undock_reanchor_stops_chasing_an_unreachable_target() {
        let plan = AutoHideReanchor {
            target: (3896, 64),
            offset: (0, 0),
            tries: 0,
            last_live: None,
            noops: 0,
        };
        // 第一拍：live 停在 1976（WM 钳制），正常牛顿步叠修正量 +1920
        let (plan2, request) = match undock_reanchor_action(plan, 1976, 128) {
            UndockReanchor::Set(plan, request) => (plan, request),
            other => panic!("expected Set, got {other:?}"),
        };
        assert_eq!(plan2.offset, (1920, -64));
        assert_eq!(plan2.last_live, Some((1976, 128)));
        assert_eq!(request, (5816, 0));
        // 第二拍：实况纹丝不动 = 请求被忽略 → **不叠**修正量、照发同一请求
        let (plan3, request2) = match undock_reanchor_action(plan2, 1976, 128) {
            UndockReanchor::Set(plan, request) => (plan, request),
            other => panic!("expected Set, got {other:?}"),
        };
        assert_eq!(plan3.offset, (1920, -64), "no-op 拍不许叠修正量");
        assert_eq!(plan3.noops, 1);
        assert_eq!(request2, (5816, 0), "no-op 拍照发同一请求");
        // 第三拍：还是没动 → 止损放弃，不再产生更远的请求
        assert_eq!(
            undock_reanchor_action(plan3, 1976, 128),
            UndockReanchor::GiveUp
        );
        // 实况动了（哪怕还没到 target）→ 计数清零，牛顿步继续工作
        let (plan4, request3) = match undock_reanchor_action(plan2, 2500, 100) {
            UndockReanchor::Set(plan, request) => (plan, request),
            other => panic!("expected Set, got {other:?}"),
        };
        assert_eq!(plan4.noops, 0, "动了的拍清零 noops");
        // offset += target − live = (1920+1396, −64−36)
        assert_eq!(plan4.offset, (3316, -100));
        assert_eq!(request3, (3896 + 3316, 64 - 100));
    }

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

        // 展示方式四值归一：老配置的 window 由 pane 承接，脏数据回落 inline
        assert_eq!(normalize_grub_list_view("inline"), "inline");
        assert_eq!(normalize_grub_list_view(" WIDE "), "wide");
        assert_eq!(normalize_grub_list_view("list"), "list");
        assert_eq!(normalize_grub_list_view("pane"), "pane");
        assert_eq!(normalize_grub_list_view("Window"), "pane");
        assert_eq!(normalize_grub_list_view(""), "inline");
        assert_eq!(normalize_grub_list_view("floating"), "inline");

        // 列表展示格式归一：五个值 + 脏值/空值回落 minimal（默认极简）
        assert_eq!(normalize_grub_list_format("raw"), "raw");
        assert_eq!(normalize_grub_list_format(" MINIMAL "), "minimal");
        assert_eq!(normalize_grub_list_format("simple"), "simple");
        assert_eq!(normalize_grub_list_format("Medium"), "medium");
        assert_eq!(normalize_grub_list_format("detailed"), "detailed");
        assert_eq!(normalize_grub_list_format(""), "minimal");
        assert_eq!(normalize_grub_list_format("fancy"), "minimal");

        // 远程示例的 selector 百分号编码：空格/中文/括号逐字节 %XX，
        // unreserved 字符原样保留
        assert_eq!(
            percent_encode_component("Arch Linux (kernel 6.18)"),
            "Arch%20Linux%20%28kernel%206.18%29"
        );
        assert_eq!(percent_encode_component("中文 条目"), "%E4%B8%AD%E6%96%87%20%E6%9D%A1%E7%9B%AE");
        assert_eq!(percent_encode_component("a-B_c.d~e"), "a-B_c.d~e");
        // 宽度跟着模式走：内嵌 460，其余 932
        assert_eq!(grub_layout_width("inline"), 460.0);
        assert_eq!(grub_layout_width("wide"), 932.0);
        assert_eq!(grub_layout_width("list"), 932.0);
        assert_eq!(grub_layout_width("pane"), 932.0);

        // 展示方式的落宽：未记过尺寸用标称；记过尊重记忆；切模式覆盖记忆
        // —— 锁定窗口大小不拦切模式的落宽（用户定案 2026-10-13）
        assert_eq!(grub_layout_target_width(932, 0, false), 932);
        assert_eq!(grub_layout_target_width(932, 700, false), 700);
        assert_eq!(grub_layout_target_width(932, 700, true), 932);

        // 窗口尺寸夹取：滑条/用户拖拽/远程传入的脏值都收进合法范围
        assert_eq!(clamp_window_size(460, 900), (460, 900));
        assert_eq!(clamp_window_size(100, 900), (360, 900));
        assert_eq!(clamp_window_size(460, 100), (460, 560));
        assert_eq!(clamp_window_size(9999, 9999), (1600, 1200));
        // 托盘菜单的「重置窗口大小」入口
        assert_eq!(
            MenuAction::from_id("reset-window-size"),
            Some(MenuAction::ResetWindowSize)
        );

        // 贴边隐藏：窗口在各方位时都选最近的边
        assert_eq!(
            nearest_edge(100, 0, 460, 900, 0, 0, 1920, 1040),
            DockEdge::Top
        );
        assert_eq!(
            nearest_edge(100, 140, 460, 900, 0, 0, 1920, 1040),
            DockEdge::Bottom
        );
        assert_eq!(
            nearest_edge(0, 70, 460, 900, 0, 0, 1920, 1040),
            DockEdge::Left
        );
        assert_eq!(
            nearest_edge(1460, 70, 460, 900, 0, 0, 1920, 1040),
            DockEdge::Right
        );

        // 位置夹取：越界拉回工作区，界内原样返回
        assert_eq!(clamp_to_work(-100, -50, 460, 900, 0, 0, 1920, 1040), (0, 0));
        assert_eq!(
            clamp_to_work(1600, 800, 460, 900, 0, 0, 1920, 1040),
            (1460, 140)
        );
        assert_eq!(clamp_to_work(10, 20, 460, 900, 0, 0, 1920, 1040), (10, 20));
        // 窗口比工作区还大：钉在左上角，不产生负坐标
        assert_eq!(clamp_to_work(5, 5, 2000, 1200, 0, 0, 1920, 1040), (0, 0));

        assert_eq!(clamp_opacity(100), 100);
        assert_eq!(clamp_opacity(50), 50);
        assert_eq!(clamp_opacity(80), 80);
        // 0 是合法值：全透明（窗口还在、还能从托盘操作），用户要的就是这个下限
        assert_eq!(clamp_opacity(0), MIN_OPACITY);
        assert_eq!(clamp_opacity(1), 1);
        assert_eq!(clamp_opacity(999), MAX_OPACITY);
    }

    #[test]
    fn notify_height_is_clamped_and_falls_back_to_nominal() {
        // 正常量出来的高度原样用
        assert_eq!(notify_height(Some(222.0)), 222.0);
        // 太低会把文字裁掉 / 太高又变成「底下空一大片」，两头都夹
        assert_eq!(notify_height(Some(10.0)), NOTIFY_MIN_HEIGHT);
        assert_eq!(notify_height(Some(9000.0)), NOTIFY_MAX_HEIGHT);
        assert_eq!(notify_height(Some(NOTIFY_MIN_HEIGHT)), NOTIFY_MIN_HEIGHT);
        assert_eq!(notify_height(Some(NOTIFY_MAX_HEIGHT)), NOTIFY_MAX_HEIGHT);
        // 量不出来（前端拿不到 layout）时退回标称高度：宁可不紧凑，也不能裁内容
        assert_eq!(notify_height(None), NOTIFY_HEIGHT);
        assert_eq!(notify_height(Some(0.0)), NOTIFY_HEIGHT);
        assert_eq!(notify_height(Some(-5.0)), NOTIFY_HEIGHT);
        assert_eq!(notify_height(Some(f64::NAN)), NOTIFY_HEIGHT);
        assert_eq!(notify_height(Some(f64::INFINITY)), NOTIFY_HEIGHT);
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


    /// 出厂默认：亮色主题、90% 不透明度、窗口居中、远程可弹本机授权框。
    /// 这四条是用户定案的「默认设置」，动它们等于动产品行为 —— 钉住。
    #[test]
    fn settings_defaults_match_the_agreed_product_defaults() {
        let settings = Settings::default();
        assert_eq!(settings.theme, DEFAULT_THEME);
        assert_eq!(settings.theme, "light");
        assert_eq!(settings.opacity_percent, DEFAULT_OPACITY_PERCENT);
        assert_eq!(settings.opacity_percent, 90);
        assert_eq!(settings.main_window_position, DEFAULT_WINDOW_POSITION);
        assert_eq!(settings.main_window_position, "center");
        // 托盘菜单方式默认 classic（三端各自方式，positioner 引入前的形态）
        assert_eq!(settings.tray_menu_style, DEFAULT_TRAY_MENU_STYLE);
        assert_eq!(settings.tray_menu_style, "classic");
        // 归一：认识的照收，不认识/空白一律回 classic
        assert_eq!(normalize_tray_menu_style("unified"), "unified");
        assert_eq!(normalize_tray_menu_style(" UNIFIED "), "unified");
        assert_eq!(normalize_tray_menu_style("classic"), "classic");
        assert_eq!(normalize_tray_menu_style(""), "classic");
        assert_eq!(normalize_tray_menu_style("whatever"), "classic");
        // 默认允许远程请求弹本机授权框（用户定案「远程可触发本机框」）
        assert!(settings.remote_allow_gui_auth);
        // 尺寸默认「还没记过」→ 启动时按整屏高度的 2/3 算，且不锁定
        assert_eq!(settings.window_height, 0);
        assert!(!settings.window_size_locked);
        // 固定显示器默认 auto（不固定），产品行为不变 —— 用户明确要求默认不限制
        assert_eq!(settings.main_fixed_monitor, "auto");
    }

    /// 固定显示器的归一与按名挑屏（纯逻辑，钉住）：
    /// 空/空白 = auto；auto 永远挑不出；名字精确匹配；显示器被拔（名字
    /// 不在候选里）回落 None —— 调用方走既有挑屏逻辑，窗口不能消失。
    #[test]
    fn fixed_monitor_normalize_and_pick() {
        assert_eq!(normalize_fixed_monitor(""), "auto");
        assert_eq!(normalize_fixed_monitor("   "), "auto");
        assert_eq!(normalize_fixed_monitor(" DP-1 "), "DP-1");
        let names = ["DP-1", "HDMI-0", "eDP-1"];
        // auto = 不固定，永远不挑
        assert_eq!(fixed_monitor_index(&names, "auto"), None);
        assert_eq!(fixed_monitor_index(&names, ""), None);
        // 精确匹配，大小写敏感（显示器名是系统标识不是用户输入）
        assert_eq!(fixed_monitor_index(&names, "HDMI-0"), Some(1));
        assert_eq!(fixed_monitor_index(&names, "hdmi-0"), None);
        // 拔掉了：回落 None
        assert_eq!(fixed_monitor_index(&names, "DP-9"), None);
    }

    /// `/health` 是客户端认「这台电脑是谁」的唯一入口：手机端靠 `machineId`
    /// 把同一台电脑的多块网卡 IP 归并成一份页面。字段名就是契约，钉住。
    #[test]
    fn health_payload_exposes_the_machine_identity() {
        let value = health_json("  abc123  ", "  desk  ");
        assert_eq!(value["ok"], serde_json::json!(true));
        assert_eq!(value["service"], serde_json::json!("anyboot"));
        assert_eq!(value["machineId"], serde_json::json!("abc123"));
        assert_eq!(value["machineName"], serde_json::json!("desk"));
        // 取不到标识也不许报错，只给空串 —— 客户端按「空 = 未知」退回分组名判断
        let unknown = health_json("", "");
        assert_eq!(unknown["machineId"], serde_json::json!(""));
        assert_eq!(unknown["machineName"], serde_json::json!(""));
        assert_eq!(unknown["ok"], serde_json::json!(true), "探活语义不能被身份字段影响");
    }

    /// 机器标识是新增设置项：老配置（没有这个键）必须能读，且默认是空串
    /// —— 由 ensure_settings_complete 在首次运行时补上。
    #[test]
    fn machine_id_defaults_to_empty_and_is_backwards_compatible() {
        assert!(Settings::default().machine_id.is_empty());
        let loaded: Settings = serde_json::from_str(r#"{"httpPort":12300,"theme":"light"}"#)
            .expect("老配置（缺 machineId）必须能反序列化");
        assert!(loaded.machine_id.is_empty());
        // 反序列化后再序列化要带上这个键，客户端下次才读得到同一个值
        let text = serde_json::to_string(&loaded).expect("序列化");
        assert!(text.contains("\"machineId\""), "落盘必须包含 machineId: {text}");
    }

    /// 主机名取第一个非空候选（纯函数）。空白串要跳过，不能拿空格当名字。
    #[test]
    fn first_nonempty_skips_blank_candidates() {
        assert_eq!(first_nonempty(Vec::<String>::new()), "");
        assert_eq!(
            first_nonempty(vec!["".to_string(), "   ".to_string()]),
            "",
            "全是空白就当取不到"
        );
        assert_eq!(
            first_nonempty(vec!["".to_string(), "  desk  ".to_string(), "other".to_string()]),
            "desk",
            "取到就 trim，且不再往后看"
        );
        assert_eq!(
            first_nonempty(["".to_string(), "second".to_string(), "third".to_string()]),
            "second"
        );
    }

    /// 推送订阅表是全局 static，而测试是并行跑的：凡碰它的用例都先拿这把锁串起来
    /// （否则一条测试的 `disconnect_all()` 会把另一条正在用的客户端清掉 → 偶发红）。
    fn ws_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 推送订阅表：登记 → 广播 → 注销 → 收服务清空。
    #[test]
    fn websocket_hub_registers_broadcasts_and_drops_clients() {
        let _serial = ws_test_guard();
        // 干净开场，免得受别的用例影响
        websocket_disconnect_all();
        assert_eq!(websocket_client_count(), 0);

        let (first, first_rx) = websocket_subscribe();
        let (_second, second_rx) = websocket_subscribe();
        assert_eq!(websocket_client_count(), 2);

        // 广播：两个客户端各收一份，内容就是推的那个快照
        websocket_publish(&serde_json::json!({ "revision": 7 }));
        for receiver in [&first_rx, &second_rx] {
            let payload = receiver.try_recv().expect("两个客户端都该收到这一拍");
            assert!(payload.contains("\"revision\":7"), "推的是快照本体：{payload}");
        }

        // 注销一个：人数减一，而且它再也收不到（发送端已 drop）
        websocket_unsubscribe(first);
        assert_eq!(websocket_client_count(), 1);
        websocket_publish(&serde_json::json!({ "revision": 8 }));
        assert!(first_rx.try_recv().is_err(), "注销后不该再收到东西");
        assert!(second_rx.try_recv().is_ok(), "另一个照常收");

        // 只推一次、却没人订阅时不做任何事（连序列化都跳过）；有人订阅才推
        websocket_disconnect_all();
        assert_eq!(websocket_client_count(), 0);
        websocket_publish(&serde_json::json!({ "revision": 9 }));
        assert!(second_rx.try_recv().is_err(), "清空之后谁都不该收到");
    }

    /// 对端收得比我们发得慢（队列塞满）时：**丢帧而不是阻塞**。
    ///
    /// 这条钉住的是「一个卡住的手机不能把主机拖住」：快照是完整状态不是增量，
    /// 丢几拍不影响正确性，但 `publish_ui` 一旦被阻塞，主窗口自己的重绘会一起卡。
    #[test]
    fn websocket_publish_drops_frames_instead_of_blocking_a_stalled_client() {
        let _serial = ws_test_guard();
        websocket_disconnect_all();
        let (id, receiver) = websocket_subscribe();

        // 推得比队列容量多得多：不该阻塞、不该 panic、也不该把连接丢掉
        for revision in 0..(WS_QUEUE * 3) {
            websocket_publish(&serde_json::json!({ "revision": revision }));
        }
        assert_eq!(
            websocket_client_count(),
            1,
            "队列满了也该留着连接，只是丢帧"
        );

        let mut received = 0;
        while receiver.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, WS_QUEUE, "最多只留得下队列容量那么多帧");

        websocket_unsubscribe(id);
        websocket_disconnect_all();
    }

    /// revision 的起点必须跟时间挂钩。**这条是跨进程重启后客户端还能自愈的前提**：
    /// 客户端按 `revision <= lastRevision` 丢弃过期快照，起点从 0 数会让新进程的
    /// 全部快照被判成旧的 → 页面永久冻在重启前的数据上。
    #[test]
    fn ui_revision_seed_must_not_restart_from_zero() {
        assert!(ui_revision_seed(1) >= 1, "种子不能是 0");
        // 启动越晚，种子越大 —— 客户端靠比大小接受新进程的快照
        assert!(ui_revision_seed(1_700_000_000_000) > ui_revision_seed(1_600_000_000_000));
        // 旧进程即使跑很久，新进程的起点也一定更大（这就是自愈的机制）
        let old_process_high_water_mark: u64 = 100_000;
        assert!(ui_revision_seed(1_700_000_000_000) > old_process_high_water_mark);
    }

    /// 静态资源路由：用户可能用任何一种自然写法打开网页端。
    /// `/index.html` 曾经漏掉 —— 无 token 时被闸门回 401「token 缺失或无效」，
    /// 带着 token 又回 404，两种都会让人误以为是自己 token 的问题。
    #[test]
    fn static_ui_routes_cover_the_natural_spellings() {
        let html_paths = ["/", "/index.html", "/ui", "/ui/", "/ui/index.html"];
        for path in html_paths {
            let reply = static_ui_reply(path).unwrap_or_else(|| panic!("{path} 应该能直接拿到页面"));
            assert_eq!(reply.status, 200);
            let (content_type, body) = reply.raw.expect("静态资源必须走 raw 响应体");
            assert_eq!(content_type, "text/html; charset=utf-8", "{path}");
            assert!(!body.is_empty(), "{path} 的正文不该是空的");
        }

        // JS / CSS 同理（index.html 里用的是 ./app.js、./styles.css 这种相对路径）
        for (path, expected) in [
            ("/app.js", "text/javascript; charset=utf-8"),
            ("/ui/app.js", "text/javascript; charset=utf-8"),
            ("/grub-list-view.js", "text/javascript; charset=utf-8"),
            ("/styles.css", "text/css; charset=utf-8"),
            ("/ui/styles.css", "text/css; charset=utf-8"),
        ] {
            let reply = static_ui_reply(path).unwrap_or_else(|| panic!("{path} 应该能拿到"));
            let (content_type, body) = reply.raw.expect("静态资源必须走 raw 响应体");
            assert_eq!(content_type, expected, "{path}");
            assert!(!body.is_empty(), "{path}");
        }

        // 不认识的路径必须返回 None（继续走鉴权 / 404），别把什么都当页面发出去
        for path in ["/nope.js", "/index.htm", "/../etc/passwd", "/ui/app.js.map"] {
            assert!(static_ui_reply(path).is_none(), "{path} 不该走静态资源");
        }
    }

    /// 「立即执行」不允许远程点 —— 授权窗的意义就是本机用户点头，远程能自己确认
    /// 就等于自己给自己盖章。「取消」（撤回自己的请求）放行，随时能反悔。
    #[test]
    fn confirming_a_pending_request_needs_a_local_click() {
        assert!(ui_command_is_local_only("confirm_pending"));
        assert!(!ui_command_is_local_only("cancel_pending"));
        // 其余命令不受影响
        assert!(!ui_command_is_local_only("refresh"));
        assert!(!ui_command_is_local_only("switch_system"));
    }

    /// 网页 / 手机端点「选择系统」必须先经本机授权（远程请求 → 本机弹窗确认）。
    /// 这是产品决策，钉住：别哪天顺手把它改回「网页直接生效」。
    #[test]
    fn selecting_a_system_from_the_web_needs_local_confirmation() {
        assert!(ui_command_requires_local_auth("switch_system"));
        // 其余命令保持原样（改设置、刷新、GRUB 切换…）
        assert!(!ui_command_requires_local_auth("refresh"));
        assert!(!ui_command_requires_local_auth("set_theme"));
        assert!(!ui_command_requires_local_auth("switch_grub"));
    }

    /// 默认窗口高度 = 整屏高度的 2/3，夹回合法区间（用户定案 2026-10-13）。
    /// 基准必须是整屏不是工作区：工作区基准在有任务栏的机器上算出 652，
    /// 窗口「已经在重置目标上」，重置按钮看起来失灵（实测踩过）。
    #[test]
    fn default_window_height_is_two_thirds_of_the_screen() {
        // 常规 1080p：2/3 = 720，在区间内
        assert_eq!(default_window_height_from_screen(1080), 720);
        // 1440p：960 在区间内
        assert_eq!(default_window_height_from_screen(1440), 960);
        // 超大屏也夹回上限（4000*2/3=2666 → 上限 1200）
        assert_eq!(default_window_height_from_screen(4000), 1200);
        // 极小屏别低于下限（600*2/3=400 → 最小 560）
        assert_eq!(default_window_height_from_screen(600), 560);
        // 奇数高度向下取整（2161*2/3=1440），再夹回滑条上限 1200
        assert_eq!(default_window_height_from_screen(2161), 1200);
    }

    /// 停靠态居中的目标落点：外框（客户区+标题栏）在工作区居中，target 是
    /// **客户区**语义（重锚收敛判据），y 加回标题栏。
    /// 回归：2x 屏 3840x2160、工作区 (0,64,3840,1924)、贴边前客户区
    /// 1864x1440、标题栏 64 → 客户区目标 (988,338)，外框 [274,1778] 恰好
    /// 居中（外框中点 1026 = 工作区中点）。此前按逻辑尺寸 932x720 算出
    /// (1454,666)，偏出正中 ~466px（10:46 日志铁证）。
    #[test]
    fn docked_center_target_centers_the_outer_frame() {
        assert_eq!(
            docked_center_target(0, 64, 3840, 1924, 1864, 1440, 64),
            (988, 338)
        );
        // 没缓存到标题栏（0）：退化为客户区自身居中，至少不出工作区
        assert_eq!(
            docked_center_target(0, 64, 3840, 1924, 1864, 1440, 0),
            (988, 306)
        );
        // 1x 小屏：1920x1080、工作区 (0,37,1920,1043)、客户区 932x720、标题栏 37
        assert_eq!(
            docked_center_target(0, 37, 1920, 1043, 932, 720, 37),
            (494, 217)
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
        use PowerAttemptOutcome::{Failed, Missing, Success, Unavailable};

        // 命令执行成功：结束。
        assert_eq!(next_power_step(&Success, true), PowerStep::Done);
        // 系统里没有这个命令：换下一个候选（例如没有 systemd 的发行版）。
        assert_eq!(
            next_power_step(&Missing("没有 systemctl".to_string()), true),
            PowerStep::Next
        );
        // 这条候选在本机用不了（没配免密 sudo）：也换下一条，
        // 否则 macOS 上管理员授权候选永远轮不到。
        assert_eq!(
            next_power_step(&Unavailable("sudo -n：a password is required".to_string()), false),
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
    fn sudo_without_password_is_a_missing_candidate_not_a_refusal() {
        let nopasswd_args = ["-n", "/sbin/shutdown", "-r", "now"]
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();

        // sudo 的原文（各版本措辞略有差别，都要认）。
        for reason in [
            "sudo: a password is required",
            "sudo: no tty present and no askpass program specified",
            "sudo: a terminal is required to read the password",
        ] {
            assert!(
                sudo_without_password("sudo", &nopasswd_args, reason),
                "应判为「这条候选用不了」: {reason}"
            );
        }

        // 没带 -n 的 sudo 可能真的弹了密码框，那是用户的决定，不能当「用不了」。
        let plain_sudo = ["/sbin/shutdown", "-r", "now"]
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        assert!(!sudo_without_password(
            "sudo",
            &plain_sudo,
            "sudo: a password is required"
        ));
        // 别的程序 / 别的失败原因照旧按 Failed 处理。
        assert!(!sudo_without_password(
            "osascript",
            &nopasswd_args,
            "a password is required"
        ));
        assert!(!sudo_without_password(
            "sudo",
            &nopasswd_args,
            "Request dismissed"
        ));
    }

    #[test]
    fn apple_events_denial_is_a_missing_candidate_not_a_refusal() {
        let system_events_args = ["-e", "tell application \"System Events\" to restart"]
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();

        // 未授权 / 被拒（osascript 原文，中英文都认；错误码优先）。
        for reason in [
            "execution error: System Events got an error: Not authorized to send Apple events to System Events. (-1743)",
            "System Events got an error: osascript is not allowed to send Apple events to System Events. (-1743)",
            "execution error: (-1743)",
        ] {
            assert!(
                apple_events_denied("osascript", &system_events_args, reason),
                "应判为「这条候选用不了」: {reason}"
            );
        }

        // 管理员授权框（也是 osascript，但脚本里没有 System Events）失败 = 用户取消，
        // 必须照旧按 Failed 停手。
        let admin_args = [
            "-e",
            "do shell script \"/sbin/shutdown -r now\" with administrator privileges",
        ]
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
        assert!(!apple_events_denied(
            "osascript",
            &admin_args,
            "execution error: User canceled. (-128)"
        ));
        // 系统里根本没装 osascript 之类，不归这条。
        assert!(!apple_events_denied(
            "sudo",
            &system_events_args,
            "Not authorized"
        ));
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
            // 顶部「下次启动：X」那一行的 id
            ("next-boot".to_string(), MenuAction::Show),
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
            ("show-remote".to_string(), MenuAction::ShowRemote),
            ("open-web".to_string(), MenuAction::OpenWeb),
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
        // 扫描不算「影响菜单」的忙态（菜单已无重建一说的自绘窗，照样保留这条语义边界）
        let scanning = BusyState {
            scanning: true,
            ..Default::default()
        };
        assert!(scanning.any());
        assert!(!scanning.affects_menu());
        let unmounting = BusyState {
            unmounting: true,
            ..Default::default()
        };
        assert!(unmounting.affects_menu());
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
            raw_body: String::new(),
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
        // 通知默认开、两条渠道默认各 5 秒、远程默认**可以**弹本机授权框
        let settings = Settings::default();
        assert!(settings.http_enabled);
        assert_eq!(settings.http_bind, DEFAULT_HTTP_BIND);
        assert_eq!(settings.http_port, DEFAULT_HTTP_PORT);
        assert_eq!(settings.http_port, 12300);
        assert_eq!(settings.http_bind, "0.0.0.0");
        assert!(settings.http_token.is_empty(), "token 由首次运行生成");
        assert!(settings.notify_enabled);
        assert!(settings.remote_allow_gui_auth, "远程默认可弹本机授权框");
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

        // /api/grub-switch：entry 是 grub.cfg 里的 selector 原文，
        // submenu 内的项形如 `submenu_id>entry_id`，空格与 `>` 都原样保留
        assert_eq!(
            route_remote_command(&sample_request(
                "/api/grub-switch",
                &[("entry", "Arch Linux")],
                true
            )),
            Ok(RemoteCommand::SwitchGrub("Arch Linux".to_string()))
        );
        assert_eq!(
            route_remote_command(&sample_request(
                "/api/grub-switch",
                &[("entry", "gnulinux-advanced-84d0>gnulinux-linux-advanced")],
                true
            )),
            Ok(RemoteCommand::SwitchGrub(
                "gnulinux-advanced-84d0>gnulinux-linux-advanced".to_string()
            ))
        );
        // selector 别名也能用
        assert_eq!(
            route_remote_command(&sample_request(
                "/api/grub-switch",
                &[("selector", "windows")],
                true
            )),
            Ok(RemoteCommand::SwitchGrub("windows".to_string()))
        );
        for query in [&[][..], &[("entry", "")][..], &[("entry", "   ")][..]] {
            let reply = route_remote_command(&sample_request("/api/grub-switch", query, true))
                .expect_err("缺 entry");
            assert_eq!(reply.status, 400);
        }

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

        // 占着端口的是**我们自己**：文案不能再说是「另一个实例」（用户会去找一个
        // 不存在的进程），也不能给出「结束自己」的命令。
        let itself = PortOccupant {
            pid: 34792,
            name: "anyboot.exe".to_string(),
            is_self: true,
            same_app: true,
        };
        let message = itself.display();
        assert!(message.contains("本程序自己占着") && message.contains("34792"));
        assert!(!message.contains("另一个实例"));
        assert_eq!(itself.kill_hint(), None);
        assert_eq!(itself.to_view().kill_hint, None);

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
    fn tray_status_line_does_not_repeat_the_next_boot_line() {
        // 扫描成功后 message == 「下次启动：X」→ 不再单独摆一行（用户报的重复）
        assert_eq!(
            menu_status_line("下次启动：Windows", "下次启动：Windows"),
            None
        );
        assert_eq!(
            menu_status_line("  下次启动：Windows  ", "下次启动：Windows"),
            None
        );
        // 空文案不摆行
        assert_eq!(menu_status_line("", "下次启动：Windows"), None);
        assert_eq!(menu_status_line("   ", "下次启动：Windows"), None);
        // 额外信息（错误 / 临时操作结果）才单独一行，且去掉首尾空白
        assert_eq!(
            menu_status_line("无法读取 PreviousBoot", "下次启动：Windows"),
            Some("无法读取 PreviousBoot")
        );
        assert_eq!(
            menu_status_line("  监听已重启：0.0.0.0:12300 ", "下次启动：Windows"),
            Some("监听已重启：0.0.0.0:12300")
        );
        // 顶部是「未知」时同理
        assert_eq!(menu_status_line("下次启动：未知", "下次启动：未知"), None);
    }

    fn next_boot_variable(system: &str, display_name: &str, active: bool, exists: bool) -> VariableInfo {
        VariableInfo {
            system: system.to_string(),
            display_name: display_name.to_string(),
            source_name: format!("PreviousBoot-{system}"),
            source: std::path::PathBuf::from("/tmp/PreviousBoot"),
            exists,
            active,
        }
    }

    fn next_boot_entry(
        selector: &str,
        name: &str,
        submenu: Option<&str>,
        kernel: Option<&str>,
    ) -> grub::GrubEntry {
        grub::GrubEntry {
            selector: selector.to_string(),
            id: selector.to_string(),
            name: name.to_string(),
            submenu: submenu.map(str::to_string),
            class: None,
            chainloader: kernel.is_none(),
            kernels: kernel.map(|kernel| vec![kernel.to_string()]).unwrap_or_default(),
            variants: 1,
        }
    }

    fn next_boot_grub_view(
        entries: Vec<grub::GrubEntry>,
        groups: Vec<grub::GrubGroup>,
        next_entry: Option<&str>,
        list_format: &str,
    ) -> GrubView {
        GrubView {
            partuuid: String::new(),
            dir: String::new(),
            esp: None,
            keep_mounted: false,
            mount_supported: true,
            efi_dir: None,
            env_path: None,
            cfg_path: None,
            env_exists: true,
            installed: None,
            synced_at: None,
            script_log: ScriptLogView::default(),
            entries,
            groups,
            list_format: list_format.to_string(),
            saved_entry: None,
            next_entry: next_entry.map(str::to_string),
            error: None,
            hint: None,
        }
    }

    /// 托盘顶部那行「下次启动」用的是**主窗口那行大字的同一份级联文字**
    /// （用户报：托盘只显示 rEFInd 一层，主窗口却是「Linux › OpenCore (macOS)」）。
    #[test]
    fn tray_next_boot_line_mirrors_the_main_window_cascade() {
        let linux = vec![next_boot_variable("linux", "Linux", true, true)];
        let none = next_boot_grub_view(vec![], vec![], None, "minimal");

        // rEFInd 选的是 Windows（直接引导）→ 只有 rEFInd 这一层，不级联
        assert_eq!(
            next_boot_cascade(&[next_boot_variable("windows", "Windows", true, true)], &none),
            "Windows"
        );
        // 没有选中的系统
        assert_eq!(next_boot_cascade(&[], &none), "未知（没有选中的系统）");
        // 选中 Linux 但 GRUB 侧还没选过 / 找不到那个条目 → 也只显示这一层
        assert_eq!(next_boot_cascade(&linux, &none), "Linux");

        // 用户报的那条：GRUB 顶层条目、没有内核 → 用菜单标题
        let top = vec![next_boot_entry("opencore", "OpenCore (macOS)", None, None)];
        assert_eq!(
            next_boot_cascade(
                &linux,
                &next_boot_grub_view(top.clone(), vec![], Some("opencore"), "minimal")
            ),
            "Linux › OpenCore (macOS)"
        );
        // 条目文字随列表格式变（与 GRUB 列表的 rowLabel 同规则）
        let kernel = vec![next_boot_entry("arch", "Arch Linux", None, Some("vmlinuz-linux"))];
        assert_eq!(
            next_boot_cascade(
                &linux,
                &next_boot_grub_view(kernel.clone(), vec![], Some("arch"), "minimal")
            ),
            "Linux › linux",
            "极简/简易：去掉 vmlinuz-/linux- 前缀"
        );
        assert_eq!(
            next_boot_cascade(
                &linux,
                &next_boot_grub_view(kernel.clone(), vec![], Some("arch"), "simple")
            ),
            "Linux › linux"
        );
        assert_eq!(
            next_boot_cascade(
                &linux,
                &next_boot_grub_view(kernel.clone(), vec![], Some("arch"), "medium")
            ),
            "Linux › vmlinuz-linux",
            "中等：完整内核名"
        );
        assert_eq!(
            next_boot_cascade(&linux, &next_boot_grub_view(kernel, vec![], Some("arch"), "detailed")),
            "Linux › Arch Linux",
            "其余格式：菜单标题"
        );

        // submenu 子条目带上父级组名：「Linux › 组名 › 条目」
        let nested = vec![next_boot_entry("macos", "OpenCore", Some("g-macos"), None)];
        let groups = vec![grub::GrubGroup {
            id: "g-macos".to_string(),
            name: "macOS".to_string(),
        }];
        assert_eq!(
            next_boot_cascade(
                &linux,
                &next_boot_grub_view(nested, groups, Some("macos"), "minimal")
            ),
            "Linux › macOS › OpenCore"
        );

        // 找不到那个 selector 的条目（比如条目名变了）→ 退回单层，绝不显示半截
        let entries = vec![next_boot_entry("a", "A", None, None)];
        assert_eq!(
            next_boot_cascade(
                &linux,
                &next_boot_grub_view(entries, vec![], Some("missing"), "minimal")
            ),
            "Linux"
        );
    }

    /// 托盘 GRUB 区与主窗 GRUB 列表**同一套格式逻辑**：置顶当前项（组内子项带
    /// 「组名 › 」前缀）、组头带系统图标与条目数、极简格式去内核前缀、
    /// 同名条目折叠成「N 个变体」。
    #[test]
    fn tray_grub_nodes_follow_the_main_window_list_logic() {
        let entries = vec![
            next_boot_entry("w", "Windows 11", None, None),
            next_boot_entry("l1", "CachyOS", Some("linux"), Some("vmlinuz-linux-cachyos")),
            next_boot_entry("l2", "CachyOS", Some("linux"), Some("vmlinuz-linux-cachyos")),
        ];
        let groups = vec![grub::GrubGroup {
            id: "linux".to_string(),
            name: "Linux".to_string(),
        }];
        let view = next_boot_grub_view(entries, groups, Some("l1"), "minimal");
        let nodes = grub_menu_nodes(&view, false);
        let labels: Vec<String> = nodes.iter().map(|node| node.label.clone()).collect();
        // 置顶当前项在最前：组内子项带「组名 › 」前缀 + 去前缀内核名 + 系统图标
        // （图标取自「组名 › 条目名」整串 —— 组名里有 linux → 🐧，与主窗同一规则）
        assert_eq!(labels[0], "🐧 Linux › cachyos", "labels={labels:?}");
        // 顶层 Windows 原位照常出行（原始顺序）
        assert_eq!(labels[1], "🪟 Windows 11", "labels={labels:?}");
        // 组头带图标与条目数（极简格式不带 ESP 位置）
        assert_eq!(labels[2], "▸ 🐧 Linux（2 项）", "labels={labels:?}");
        // 同名折叠：组内只出一行，标 2 个变体（条目名 CachyOS 不含系统关键字 → 无图标）
        assert_eq!(labels[3], "↳ cachyos · 2 个变体", "labels={labels:?}");

        // 这两条是「各端渲染器」共同依赖的契约（classic 原生菜单也照它翻译）：
        // * 组头 kind=header（原生菜单据此渲染成禁用标题而不是可点条目），
        //   其余 kind=item 且带动作 id；
        // * 「当前项」是数据字段 checked（原生菜单拿它决定挂不挂绿勾），
        //   不在 label 文字里 —— 把 ◉/○ 画进文字是文字型宿主（dbusmenu/自绘窗）的事。
        assert_eq!(nodes[2].kind, "header");
        assert_eq!(nodes[2].id.as_deref(), Some("grub-group:linux"));
        assert_eq!(nodes[3].kind, "item");
        assert_eq!(nodes[3].id.as_deref(), Some("grub-switch:l1"));
        assert_eq!(nodes[0].checked, Some(true), "置顶的当前项要带标记");
        assert_eq!(nodes[1].checked, Some(false));
    }

    /// 组内子项的行文字 = 「↳ 图标 文字」—— **箭头在图标的后面**，与主窗 `makeRow`
    /// （`arrow + icon + text`）和组头（`▸ 图标 组名`）同一个顺序。
    /// 这里曾经是「图标 箭头」，于是同一行在托盘是「🐧 ↳ linux」、在主窗是
    /// 「↳ 🐧 linux」，两处对不上。
    #[test]
    fn tray_grub_child_rows_put_the_arrow_before_the_icon() {
        let entries = vec![
            next_boot_entry("w", "Windows 11", None, None),
            next_boot_entry("a", "Arch Linux", Some("linux"), Some("vmlinuz-linux")),
        ];
        let groups = vec![grub::GrubGroup {
            id: "linux".to_string(),
            name: "Linux".to_string(),
        }];
        // 当前项是 Windows → 组内那条原位出行（带 ↳ 缩进），置顶的是 Windows
        let view = next_boot_grub_view(entries, groups, Some("w"), "minimal");
        let labels: Vec<String> = grub_menu_nodes(&view, false)
            .into_iter()
            .map(|node| node.label)
            .collect();
        assert_eq!(labels[2], "▸ 🐧 Linux（1 项）", "labels={labels:?}");
        assert_eq!(labels[3], "↳ 🐧 linux", "labels={labels:?}");
    }

    /// 原始格式：完整标题、不做折叠，变体数用解析期合并的 `entry.variants`，
    /// 行尾附内核（与主窗 raw 的右侧小字同一份信息）。
    #[test]
    fn tray_grub_nodes_raw_keeps_full_titles_and_parsed_variants() {
        let mut entry = next_boot_entry("m", "Memtest86+", None, Some("vmlinuz-memtest"));
        entry.variants = 3;
        let view = next_boot_grub_view(vec![entry], vec![], Some("m"), "raw");
        let labels: Vec<String> = grub_menu_nodes(&view, false)
            .into_iter()
            .map(|node| node.label)
            .collect();
        // 置顶行 + 原位行（置顶只复制、不从原位删除 —— 主窗定案），文字一致；
        // 顶层条目没有 ↳ 缩进箭头（那是组内子项的标记）
        assert_eq!(labels[0], "Memtest86+ · 3 个变体 · vmlinuz-memtest");
        assert_eq!(labels[1], "Memtest86+ · 3 个变体 · vmlinuz-memtest");
    }

    /// 系统图标规则与主窗 grubOsIcon 一致：windows/mac/linux 关键字，都不中不加。
    #[test]
    fn grub_os_icon_matches_the_frontend_rule() {
        assert_eq!(grub_os_icon("Windows 11"), "🪟 ");
        assert_eq!(grub_os_icon("Mac OS X"), "🍎 ");
        assert_eq!(grub_os_icon("apple silicon"), "🍎 ");
        assert_eq!(grub_os_icon("CachyOS Linux"), "🐧 ");
        assert_eq!(grub_os_icon("Memtest86+"), "");
    }

    /// 选择项的勾是**标签文字**，不是原生勾选控件（本轮 ①②）。
    ///
    /// 原生勾选控件在「应用把菜单导出成 dbusmenu、宿主重新渲染」这条链路上会两头出问题：
    /// 勾被工具包自己翻掉、以及重建菜单后 item id 整片重排导致勾跑到别的行。
    /// 把状态放进标签，这两件事就不可能发生 —— 这里把这条定则钉住。
    #[test]
    fn choice_marks_live_in_the_label_text() {
        // 开关：☑ / ☐
        assert_eq!(toggle_label(true, "开机启动"), "☑ 开机启动");
        assert_eq!(toggle_label(false, "开机启动"), "☐ 开机启动");
        // 单选：◉ / ○
        assert_eq!(radio_label(true, "屏幕中间"), "◉ 屏幕中间");
        assert_eq!(radio_label(false, "屏幕中间"), "○ 屏幕中间");
        // 两种标记互不串台（开关项不该长成单选的样子，反之亦然）
        assert!(!toggle_label(true, "x").starts_with(RADIO_ON));
        assert!(!radio_label(true, "x").starts_with(TOGGLE_ON));
        // 标记在**最前**（对齐靠它，一眼扫下来成列）
        assert!(toggle_label(false, "x").starts_with(TOGGLE_OFF));
        assert!(radio_label(false, "x").starts_with(RADIO_OFF));
    }

    /// 单选组「必须有且只有一个选中」：三种落位方式轮一遍，每组恰好一个 ◉。
    #[test]
    fn radio_groups_always_have_exactly_one_marked() {
        let group = ["free", "center", "last"];
        for current in group {
            let labels: Vec<String> = group
                .iter()
                .map(|mode| radio_label(*mode == current, mode))
                .collect();
            let marked = labels
                .iter()
                .filter(|label| label.starts_with(RADIO_ON))
                .count();
            assert_eq!(marked, 1, "落位方式={current} 时勾的数量：{labels:?}");
        }
        // 显示器那组：「自动（不固定）」+ 若干显示器名，也恰好一个
        let monitors = ["auto", "HDMI-A-0", "eDP-1"];
        for current in monitors {
            let marked = monitors
                .iter()
                .filter(|name| radio_label(**name == current, name).starts_with(RADIO_ON))
                .count();
            assert_eq!(marked, 1, "固定显示器={current}");
        }
        // 延迟那组：只有两条渠道一致时才有一个勾（不一致 = 一个勾都没有，见 delay_same）
        let marked = [0u32, 3, 5]
            .iter()
            .filter(|seconds| radio_label(false && **seconds == 5, &seconds.to_string()).starts_with(RADIO_ON))
            .count();
        assert_eq!(marked, 0, "两条渠道延迟不一致时不该有勾");
    }

    /// ③「涉及选择框的点完别关菜单」—— 规则只算一次，落在模型的 `keepsOpen` 上。
    ///
    /// 这里把「哪些算选择项」钉死：开关/单选（勾在标签文字里）、列表类的当前项（带 ◉）
    /// 都留着菜单；动作项关。顶部的「下次启动」虽然也带标记，但点它是打开主窗口，属动作。
    #[test]
    fn keeps_open_follows_the_node_kind_not_the_label_shape() {
        let nodes = vec![
            menu_item("refresh", "扫描", true),
            menu_choice_item("setting-autostart", toggle_label(true, "开机启动"), true),
            menu_marked_choice_item("linux", "Linux", true, true),
            menu_marked_item("next-boot", "下次启动\nLinux › OpenCore", true, true),
            menu_submenu(
                "setting-window-position",
                "固定显示位置",
                vec![menu_choice_item(
                    "setting-main-window-position:center",
                    radio_label(true, "屏幕中间"),
                    true,
                )],
            ),
        ];
        assert!(!menu_node_keeps_open(&nodes, "refresh"), "扫描是动作");
        assert!(menu_node_keeps_open(&nodes, "setting-autostart"), "开关留着菜单");
        assert!(menu_node_keeps_open(&nodes, "linux"), "列表里的当前项留着菜单");
        assert!(
            !menu_node_keeps_open(&nodes, "next-boot"),
            "顶部「下次启动」点它是打开主窗口，属动作"
        );
        assert!(
            menu_node_keeps_open(&nodes, "setting-main-window-position:center"),
            "子菜单里的选择项也要查得到（递归）"
        );
        assert!(!menu_node_keeps_open(&nodes, "不存在的 id"), "找不到按动作算");
    }

    /// 菜单高度：夹在区间里，量不出来（None / 0 / NaN）用标称高度。
    #[test]
    fn tray_menu_height_is_clamped_with_a_nominal_fallback() {
        assert_eq!(tray_menu_height(None), TRAY_MENU_NOMINAL_HEIGHT);
        assert_eq!(tray_menu_height(Some(0.0)), TRAY_MENU_NOMINAL_HEIGHT);
        assert_eq!(tray_menu_height(Some(f64::NAN)), TRAY_MENU_NOMINAL_HEIGHT);
        assert_eq!(tray_menu_height(Some(5.0)), TRAY_MENU_MIN_HEIGHT);
        assert_eq!(tray_menu_height(Some(400.0)), 400.0);
        assert_eq!(tray_menu_height(Some(9999.0)), TRAY_MENU_MAX_HEIGHT);
    }

    /// 工作区封顶：上限缺位/非法不截，超了就截到上限 —— 窗口绝不比工作区高。
    #[test]
    fn tray_menu_height_respects_the_workarea_cap() {
        assert_eq!(clamp_tray_menu_height(400.0, None), 400.0);
        assert_eq!(clamp_tray_menu_height(400.0, Some(0.0)), 400.0);
        assert_eq!(clamp_tray_menu_height(400.0, Some(f64::NAN)), 400.0);
        assert_eq!(clamp_tray_menu_height(400.0, Some(1000.0)), 400.0);
        assert_eq!(clamp_tray_menu_height(1200.0, Some(900.0)), 900.0);
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

        // GRUB 条目：selector 原样带上（可能是名字，也可能是 submenu>id）
        assert_eq!(
            RemoteCommand::SwitchGrub("Arch Linux".to_string()).summary(),
            "切换 GRUB 启动项为 Arch Linux"
        );
    }

    /// 手动验证用（`cargo test -- --ignored shell_execute_can_open_url`）：
    /// 真去唤起默认浏览器。
    ///
    /// 判据不是「函数返回 Ok」——ShellExecuteW 成功只代表系统接受了请求，
    /// 真正打开与否要看浏览器有没有发请求。所以配套起一个本地探针：
    /// `python -m http.server 8899 --bind 127.0.0.1`，跑完看它的访问日志里
    /// 有没有 `GET /probe-shell-execute`。
    #[test]
    #[ignore = "会真的弹出默认浏览器，需要手动运行并观察探针日志"]
    fn shell_execute_can_open_url() {
        open_url_in_browser("http://127.0.0.1:8899/probe-shell-execute")
            .expect("ShellExecuteW 打开 URL 应被系统接受");
    }

    #[test]
    fn web_page_url_follows_bind_address() {
        // 绑通配地址 → 换成本机局域网 IP（IP 不固定，这里只断言形状）
        let lan = web_page_url("0.0.0.0", 12300, "abc123");
        assert!(lan.starts_with("http://"), "{lan}");
        assert!(lan.ends_with(":12300/?token=abc123"), "{lan}");
        assert!(
            !lan.contains("0.0.0.0"),
            "通配地址不该原样出现在 URL 里（浏览器打开必失败）: {lan}"
        );
        // 回环绑定就老老实实给回环
        assert_eq!(
            web_page_url("127.0.0.1", 12300, "abc123"),
            "http://127.0.0.1:12300/?token=abc123"
        );
        // 自定义 token 里的 `&`、`+` 必须编码：服务端 find_pair 会解码，
        // 不编码的话 `+` 会被解成空格、`&` 会被当成参数分隔符。
        assert_eq!(
            web_page_url("192.168.1.5", 8080, "a&b+c"),
            "http://192.168.1.5:8080/?token=a%26b%2Bc"
        );
        // IPv6 字面量要加方括号
        assert_eq!(web_page_url("::1", 12300, "t"), "http://[::1]:12300/?token=t");
        assert_eq!(
            web_page_url("::", 12300, "t"),
            "http://127.0.0.1:12300/?token=t"
        );
    }

    #[test]
    fn http_state_text_covers_every_listening_outcome() {
        assert_eq!(
            http_state_text(true, Some("0.0.0.0:12300"), None, true),
            "正在监听 0.0.0.0:12300"
        );
        // 有错误优先说错误：用户最需要知道的是「为什么没起来」
        assert_eq!(
            http_state_text(false, None, Some("端口被占用"), true),
            "未监听：端口被占用"
        );
        assert_eq!(
            http_state_text(false, None, None, true),
            "未监听（服务已关闭）"
        );
        assert_eq!(
            http_state_text(false, None, None, false),
            "未监听（token 为空，服务不会启动）"
        );
    }

    #[test]
    fn menu_status_text_truncates_long_errors() {
        assert_eq!(
            menu_status_text("正在监听 0.0.0.0:12300", 32),
            "正在监听 0.0.0.0:12300"
        );
        // 短于上限的按原样显示（按字符数算，不是字节数：中文一个字算一个）
        assert_eq!(menu_status_text("未监听", 3), "未监听");
        assert_eq!(menu_status_text("未监听", 2), "未监…");
        assert_eq!(menu_status_text("", 4), "");
    }

    /// 菜单指纹必须覆盖每一个可见字段：少一个就是「内容变了但不重建/不发更新」的假状态。
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
        // 监听状态是「远程控制」子菜单里的一个可见字段，这里固定住它，
        // 让下面的断言只针对各自那一项变化。
        let http = "正在监听 0.0.0.0:12300";
        let signature = |message: &str,
                         temporary_mount: bool,
                         settings: &Settings,
                         variables: &[VariableInfo],
                         error: Option<&str>,
                         busy: &BusyState| {
            menu_signature(message, temporary_mount, settings, variables, error, busy, http)
        };
        let base = signature("下次启动：Windows", false, &settings, &variables, None, &idle);
        // 同内容同签名
        assert_eq!(
            base,
            signature(
                "下次启动：Windows",
                false,
                &settings,
                &variables,
                None,
                &idle
            )
        );
        // 状态文案 / 临时挂载 / 错误 / 设置 / 变量选中与存在 / busy / 监听状态，
        // 任一变化都必须换签名，否则「内容没变就不重建菜单」会漏掉真实变更。
        assert_ne!(
            base,
            signature("已连接 rEFInd", false, &settings, &variables, None, &idle)
        );
        assert_ne!(
            base,
            signature("下次启动：Windows", true, &settings, &variables, None, &idle)
        );
        assert_ne!(
            base,
            signature(
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
            signature(
                "下次启动：Windows",
                false,
                &toggled,
                &variables,
                None,
                &idle
            )
        );
        // 主窗口里改「HTTP 服务 / 远程请求通知 / 远程执行延迟」，
        // 托盘「远程控制」子菜单上的勾也必须跟着动
        let mut http_toggled = settings.clone();
        http_toggled.http_enabled = !http_toggled.http_enabled;
        assert_ne!(
            base,
            signature(
                "下次启动：Windows",
                false,
                &http_toggled,
                &variables,
                None,
                &idle
            )
        );
        let mut notify_toggled = settings.clone();
        notify_toggled.notify_enabled = !notify_toggled.notify_enabled;
        assert_ne!(
            base,
            signature(
                "下次启动：Windows",
                false,
                &notify_toggled,
                &variables,
                None,
                &idle
            )
        );
        // 主窗口里改「贴边自动隐藏」开关，托盘里对应的勾也必须跟着动
        let mut auto_hide_toggled = settings.clone();
        auto_hide_toggled.main_auto_hide = !auto_hide_toggled.main_auto_hide;
        assert_ne!(
            base,
            signature(
                "下次启动：Windows",
                false,
                &auto_hide_toggled,
                &variables,
                None,
                &idle
            )
        );
        let mut delay_changed = settings.clone();
        delay_changed.channels.http.delay_seconds += 3;
        assert_ne!(
            base,
            signature(
                "下次启动：Windows",
                false,
                &delay_changed,
                &variables,
                None,
                &idle
            )
        );
        // 监听状态本身变了（服务起来/停了）也要换签名
        assert_ne!(
            base,
            menu_signature(
                "下次启动：Windows",
                false,
                &settings,
                &variables,
                None,
                &idle,
                "未监听（服务已关闭）"
            )
        );
        let mut active = variables.clone();
        active[0].active = true;
        assert_ne!(
            base,
            signature("下次启动：Windows", false, &settings, &active, None, &idle)
        );
        let mut missing = variables.clone();
        missing[0].exists = false;
        assert_ne!(
            base,
            signature("下次启动：Windows", false, &settings, &missing, None, &idle)
        );
        // 长动作进行中菜单项要变灰 → 必须进指纹，否则置灰不会生效
        let power = BusyState {
            power: Some("restart".to_string()),
            ..Default::default()
        };
        assert_ne!(
            base,
            signature(
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
            signature(
                "下次启动：Windows",
                false,
                &settings,
                &variables,
                None,
                &scanning
            )
        );
    }

    /// busy 的菜单摘要只含 affects_menu 的部分（scanning 刻意不在内，见 menu_signature）。
    #[test]
    fn busy_menu_describe_covers_only_menu_affecting_parts() {
        let idle = BusyState::default();
        assert_eq!(idle.menu_describe(), "power=-|sw=0|un=0");
        let power = BusyState {
            power: Some("restart".to_string()),
            ..Default::default()
        };
        assert_eq!(power.menu_describe(), "power=restart|sw=0|un=0");
        let unmounting = BusyState {
            unmounting: true,
            ..Default::default()
        };
        assert_eq!(unmounting.menu_describe(), "power=-|sw=0|un=1");
    }

    /// 绿勾位图：有实心像素（勾画出来了）、有透明像素（不是色块）、颜色是 --ok 那个绿。
    #[test]
    fn check_mark_image_is_an_opaque_green_glyph() {
        let rgba = check_mark_rgba();
        assert_eq!(rgba.len(), (CHECK_MARK_SIZE * CHECK_MARK_SIZE * 4) as usize);
        let opaque = rgba.chunks_exact(4).filter(|pixel| pixel[3] > 200).count();
        let transparent = rgba.chunks_exact(4).filter(|pixel| pixel[3] == 0).count();
        assert!(opaque > 20, "绿色像素太少: {opaque}");
        assert!(transparent > 100, "透明像素太少: {transparent}");
        let first = rgba
            .chunks_exact(4)
            .find(|pixel| pixel[3] > 200)
            .expect("应有实心像素");
        assert_eq!(&first[..3], &[0x46, 0xc4, 0x6e]);
    }

    fn set16(sector: &mut [u8], offset: usize, value: u16) {
        sector[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn set32(sector: &mut [u8], offset: usize, value: u32) {
        sector[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn fat_boot_sector_reads_type_and_serial() {
        // ① FAT32：root_entries = 0 且 fat_size16 = 0，序列号在偏移 67
        let mut sector = vec![0u8; 512];
        set16(&mut sector, 11, 512); // 每扇区字节数
        sector[13] = 8; // 每簇扇区数
        set16(&mut sector, 14, 32); // 保留扇区
        sector[16] = 2; // FAT 份数
        set32(&mut sector, 32, 1_048_576); // 总扇区数（FAT32 用 32 位这个）
        set32(&mut sector, 36, 1_000); // 每份 FAT 的扇区数
        set32(&mut sector, 67, 0xE537_7F31);
        assert_eq!(
            fat_boot_sector(&sector),
            Some(("FAT32".to_string(), "E5377F31".to_string()))
        );

        // ② FAT16：序列号在偏移 39（blkid 只说 vfat，靠这个才分得出来）
        let mut sector = vec![0u8; 512];
        set16(&mut sector, 11, 512);
        sector[13] = 4;
        set16(&mut sector, 14, 1);
        sector[16] = 2;
        set16(&mut sector, 17, 512); // 根目录项数
        set16(&mut sector, 19, 40_000);
        set16(&mut sector, 22, 100);
        set32(&mut sector, 39, 0x1234_5678);
        assert_eq!(
            fat_boot_sector(&sector),
            Some(("FAT16".to_string(), "12345678".to_string()))
        );

        // ③ FAT12：簇数 < 4085
        let mut sector = vec![0u8; 512];
        set16(&mut sector, 11, 512);
        sector[13] = 1;
        set16(&mut sector, 14, 1);
        sector[16] = 2;
        set16(&mut sector, 17, 224);
        set16(&mut sector, 19, 2_000);
        set16(&mut sector, 22, 4);
        set32(&mut sector, 39, 0x0000_0001);
        assert_eq!(
            fat_boot_sector(&sector),
            Some(("FAT12".to_string(), "00000001".to_string()))
        );

        // ④ 不是引导扇区 / 太短：宁可返回 None，也不能瞎猜
        assert_eq!(fat_boot_sector(&vec![0u8; 512]), None);
        assert_eq!(fat_boot_sector(&[0u8; 64]), None);
    }

    #[test]
    fn partuuid_digits_requires_32_hex() {
        // 标准 GPT GUID（带连字符），归一化成 32 位大写
        assert_eq!(
            grub::partuuid_digits("F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3"),
            Some("F69D7BF9E8A93135A5CB4BD0CA7344E3".to_string())
        );
        // 8 位 FAT 卷序列号是 fs-uuid 不是分区 UUID，必须拒 —— 两个键空间别混
        assert_eq!(grub::partuuid_digits("E537-7F31"), None);
        assert_eq!(grub::partuuid_digits(""), None);
        assert_eq!(grub::partuuid_digits("1234567890abcdefg"), None);
    }

    #[test]
    fn esp_info_serializes_the_keys_the_window_reads() {
        let info = EspInfo {
            path: "H:\\".to_string(),
            root: Some("H:\\".to_string()),
            fstype: Some("FAT32".to_string()),
            fat: true,
            fat32: true,
            uuid: Some("E537-7F31".to_string()),
            partuuid: Some("F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3".to_string()),
            mounted: true,
            owned_mount: false,
            ..EspInfo::default()
        };
        let json = serde_json::to_value(&info).expect("应能序列化");
        assert_eq!(json["fat32"], serde_json::json!(true));
        assert_eq!(json["fat"], serde_json::json!(true));
        assert_eq!(json["uuid"], serde_json::json!("E537-7F31"));
        assert_eq!(
            json["partuuid"],
            serde_json::json!("F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3")
        );
        assert_eq!(json["ownedMount"], serde_json::json!(false));
        assert_eq!(json["fstype"], serde_json::json!("FAT32"));
    }

    /// Windows：不是「盘符路径」的输入必须当场拒绝，而不是拿首字母去猜个盘符。
    #[test]
    #[cfg(target_os = "windows")]
    fn windows_esp_probe_rejects_paths_without_a_drive_letter() {
        for input in ["", "1:\\", "\\\\?\\nonsense"] {
            let error = esp_probe(Path::new(input)).expect_err("应当拒绝");
            assert!(
                error.contains("盘符路径"),
                "错误信息应说明要填盘符路径，实际是：{error}"
            );
        }
    }

    #[test]
    fn grub_efi_dir_and_keep_mounted_round_trip() {
        let mut settings = Settings::default();
        assert!(settings.grub_efi_dir.is_empty());
        // 默认勾选「保持挂载」（用户定案）：识别挂上的分区留在系统里，
        // 下次扫描直接复用，不用每次重新 mount/umount。
        assert!(settings.grub_keep_mounted);

        settings.grub_efi_dir = "H:\\".to_string();
        settings.grub_keep_mounted = true;
        let json = serde_json::to_string(&settings).expect("应能序列化");
        let parsed: Settings = serde_json::from_str(&json).expect("应能反序列化");
        assert_eq!(parsed.grub_efi_dir, "H:\\");
        assert!(parsed.grub_keep_mounted);

        // 老配置文件里没有这两个键 → 必须走默认值（`serde(default)` 那条老坑）
        let legacy: Settings =
            serde_json::from_str(r#"{"autostart":true}"#).expect("缺字段也要能读");
        assert_eq!(legacy.grub_efi_dir, "");
        // 老配置缺键 → serde default → 现在默认勾选「保持挂载」
        assert!(legacy.grub_keep_mounted);
    }

    // ---- 写 ESP：自己写得动就自己写，写不动交给 root ----

    /// 测试用的临时目录（放系统临时目录里，跑完不删 —— 重启就没了）。
    fn scratch_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "anyboot-test-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("创建测试目录");
        dir
    }

    fn set_mtime(path: &Path, modified: SystemTime) {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("打开待改 mtime 的文件")
            .set_modified(modified)
            .expect("设置 mtime");
    }

    /// 源文件 0600 读不了，判定「已同步」只能靠 size + mtime：这一条把判据钉住。
    #[test]
    fn esp_sync_matches_by_size_and_time_only() {
        let dir = scratch_dir("size-time");
        let source = dir.join("grub.cfg");
        let target = dir.join("grub.cfg.copy");
        fs::write(&source, b"AAA").expect("写源");
        fs::write(&target, b"BBB").expect("写副本");
        let source_time = fs::metadata(&source).expect("源元数据").modified().unwrap();

        // 大小一样但 mtime 差得远 → 不算同一份
        set_mtime(&target, source_time - Duration::from_secs(30));
        assert!(!same_size_and_time(&source, &target));

        // 差 2 秒以内 → 算同一份（ESP 是 FAT，mtime 只有 2 秒精度）
        set_mtime(&target, source_time - Duration::from_secs(2));
        assert!(same_size_and_time(&source, &target));
        set_mtime(&target, source_time);
        assert!(same_size_and_time(&source, &target));

        // 大小不同一律不算
        fs::write(&target, b"BBBB").expect("写副本");
        set_mtime(&target, source_time);
        assert!(!same_size_and_time(&source, &target));

        // 副本不存在也不算（「已同步」必须眼见为实）
        assert!(!same_size_and_time(&source, &dir.join("missing")));
    }

    /// 已经同步过就**不再写** —— 否则每次扫描都要白弹一次授权框。
    #[test]
    fn esp_copy_leaves_an_already_synced_copy_alone() {
        let dir = scratch_dir("skip");
        let source = dir.join("grub.cfg");
        let target = dir.join("grub.cfg.copy");
        fs::write(&source, b"AAA").expect("写源");
        fs::write(&target, b"BBB").expect("写副本");
        set_mtime(
            &target,
            fs::metadata(&source).expect("源元数据").modified().unwrap(),
        );

        copy_into_esp(&source, &target).expect("判定已同步时不该报错");
        // 内容刻意不同：这条断言要证明的就是「没动它」
        assert_eq!(fs::read(&target).expect("读副本"), b"BBB");
    }

    /// 副本过期就自己写（目标目录可写时走原子写这条路），并把源的 mtime 带过去。
    #[test]
    fn esp_copy_writes_a_stale_copy_and_carries_the_source_mtime() {
        let dir = scratch_dir("stale");
        let source = dir.join("grub.cfg");
        let target = dir.join("EFI").join("grub.cfg");
        fs::create_dir_all(target.parent().expect("父目录")).expect("建 EFI 目录");
        fs::write(&source, b"menuentry 'Arch' {").expect("写源");

        copy_into_esp(&source, &target).expect("可写目录里应当直接写成");
        assert_eq!(fs::read(&target).expect("读副本"), b"menuentry 'Arch' {");
        assert!(
            same_size_and_time(&source, &target),
            "副本要带上源的 mtime，下次扫描才认得出「已同步」"
        );
    }

    /// 内存里的字节（ESP 上的环境块）同样走同一条写入路径。
    #[test]
    fn esp_bytes_write_lands_in_a_writable_directory() {
        let dir = scratch_dir("bytes");
        let target = dir.join("grubenv");
        let block = grub::empty_env_block();

        write_esp_bytes(&target, &block).expect("可写目录里应当直接写成");
        assert_eq!(fs::read(&target).expect("读回"), block);
        assert_eq!(block.len(), grub::ENV_BLOCK_SIZE);
    }

    /// `user_can` 必须反映**真实**权限（root 天生全能，那部分跳过不测）。
    #[test]
    #[cfg(target_os = "linux")]
    fn user_can_reads_real_permissions() {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let dir = scratch_dir("access");
        assert!(user_can(&dir, libc::W_OK));

        let locked = dir.join("locked");
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&locked)
            .expect("建 0600 文件");
        assert!(user_can(&locked, libc::R_OK), "0600 的自己应当读得到");

        if unsafe { libc::geteuid() } == 0 {
            // root 面前没有权限位这回事，下面这条测不了
            return;
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("改成 000");
        assert!(!user_can(&locked, libc::R_OK), "000 的文件不该读得到");
        assert!(
            !user_can(&locked, libc::W_OK),
            "000 的文件不该写得到 —— ESP 上那些 root 属主的文件就是这样"
        );
    }

    /// 贴边停靠态下 Moved 事件的判据：容差内是自己 set_position 的回声（不动），
    /// 超出容差才是「用户把白条拖离」。
    #[test]
    fn docked_move_within_tolerance_is_our_own_echo() {
        let dock_pos = (1400, 1000);
        assert_eq!(
            docked_move_action((1400, 1000), dock_pos, AUTO_HIDE_TOLERANCE),
            DockedMove::Rest
        );
        // 容差边界：正好 24 算回声（<=），25 就是拖离
        assert_eq!(
            docked_move_action((1400 + 24, 1000), dock_pos, AUTO_HIDE_TOLERANCE),
            DockedMove::Rest
        );
        assert_eq!(
            docked_move_action((1400 + 25, 1000), dock_pos, AUTO_HIDE_TOLERANCE),
            DockedMove::Release
        );
        // Linux 上去装饰瞬间的 frame extents 错拍 = 整个标题栏（36px），
        // 这类回声只能靠抑制期挡（见 geometry_busy），不能指望容差
        assert_eq!(
            docked_move_action((1400, 1000 - 36), dock_pos, AUTO_HIDE_TOLERANCE),
            DockedMove::Release
        );
    }

    /// 居中模式 Moved 判据：载荷落在最近落点容差内 = 我们自己的回声/WM 抖动
    /// （不动），超出 = 用户把窗口拖走了 → 收回居中位。
    #[test]
    fn center_move_within_tolerance_is_our_own_echo() {
        let last = (1400, 1000);
        assert_eq!(
            center_move_action((1400, 1000), last, CENTER_HOLD_TOLERANCE),
            CenterMove::Rest
        );
        // 去装饰/重装饰瞬间的标题栏错拍（Muffin 实测 37px）仍算「还在居中位」
        // —— 正是这条让「居中后 700ms 一律忽略」的时间窗可以整段删掉
        assert_eq!(
            center_move_action((1400, 1000 - 37), last, CENTER_HOLD_TOLERANCE),
            CenterMove::Rest
        );
        // 容差边界：正好 80 算没动，81 就是拖动
        assert_eq!(
            center_move_action((1400 + 80, 1000), last, CENTER_HOLD_TOLERANCE),
            CenterMove::Rest
        );
        assert_eq!(
            center_move_action((1400 + 81, 1000), last, CENTER_HOLD_TOLERANCE),
            CenterMove::Snap
        );
        // 用户拖走半个屏 → 必须收回（旧实现这条会被 700ms 回声窗吃掉）
        assert_eq!(
            center_move_action((2400, 1000), last, CENTER_HOLD_TOLERANCE),
            CenterMove::Snap
        );
        assert_eq!(
            center_move_action((1400 - 200, 1000 - 200), last, CENTER_HOLD_TOLERANCE),
            CenterMove::Snap
        );
    }

    /// 居中的静默期必须够长（拖动中 Moved 连发，短了会在拖动途中抢位），
    /// 容差必须够宽（吃得下一个标题栏的错拍，否则我们自己的回声被判成拖动）。
    #[test]
    fn center_snap_timing_and_tolerance_are_sane() {
        assert!(
            CENTER_HOLD_TOLERANCE > 40,
            "容差必须吃得下标题栏错拍（Muffin 实测 37px）"
        );
        assert!(
            CENTER_SNAP_DELAY_MS >= CLAMP_DELAY_MS,
            "静默期不该短于位置夹取那套（同一类拖动事件流）"
        );
    }

    /// 电源忙闸的「延时放闸」窗口必须够长也够短：太短会在真关机/重启途中把闸放掉
    /// （用户还能再点一次），太长则「命令返回成功但机器没动」期间电源项一直灰着。
    /// 另外失败路径必须**立刻**放闸，不能也走延时 —— 这条由 spawn_power 里
    /// `finish_power` 直接调用保证，这里钉住量级。
    #[test]
    fn power_latch_release_window_is_sane() {
        assert!(
            POWER_RELEASE_MS >= 5_000,
            "短于 5 秒：真关机途中就可能放闸"
        );
        assert!(
            POWER_RELEASE_MS <= 60_000,
            "长于 60 秒：一次没生效的动作会白等太久"
        );
    }

    /// 停靠态每拍的判据：抑制期 > 位置偏离 > 悬停 > 静默。
    #[test]
    fn docked_poll_priority_is_busy_then_drift_then_hover() {
        // 抑制期内：位置还没落位、光标也压着，都不能动
        assert_eq!(docked_poll_action(true, true, true, 0, 0, 0), DockedPoll::Hold);
        // 位置偏离 → 先钉回，不管光标在哪
        assert_eq!(
            docked_poll_action(false, true, false, 0, 0, 0),
            DockedPoll::SnapBack
        );
        // 自纠用完就不再钉（防 WM 不配合时变成永久 set_position 抖动）
        assert_eq!(
            docked_poll_action(false, true, false, AUTO_HIDE_RESNAP_LIMIT, 0, 0),
            DockedPoll::Rest
        );
        assert_eq!(
            docked_poll_action(
                false,
                true,
                true,
                AUTO_HIDE_RESNAP_LIMIT,
                99,
                0
            ),
            DockedPoll::Release
        );
        // 位置正常 + 光标压到白条上 → 展开
        assert_eq!(
            docked_poll_action(false, false, true, 0, 1, 0),
            DockedPoll::Release
        );
        assert_eq!(
            docked_poll_action(false, false, false, 0, 0, 0),
            DockedPoll::Rest
        );
    }

    /// 悬浮呼出延时：压住白条的拍数没攒够不许展开；攒够（或延时为 0）才放行。
    #[test]
    fn docked_poll_hover_requires_enough_ticks() {
        // 延时 0：当拍（第 1 拍）就放行 —— 既有行为
        assert_eq!(
            docked_poll_action(false, false, true, 0, 1, 0),
            DockedPoll::Release
        );
        // 延时 2 拍：第 1 拍压着不动，第 2 拍放行
        assert_eq!(
            docked_poll_action(false, false, true, 0, 1, 2),
            DockedPoll::Rest
        );
        assert_eq!(
            docked_poll_action(false, false, true, 0, 2, 2),
            DockedPoll::Release
        );
        // 没压住白条：攒了多少拍都不展开
        assert_eq!(
            docked_poll_action(false, false, false, 0, 99, 0),
            DockedPoll::Rest
        );
    }

    /// 悬浮呼出延时毫秒 → 120ms 拍数：向上取整，0 = 立即。
    #[test]
    fn hover_delay_ticks_rounds_up() {
        assert_eq!(hover_delay_ticks(0), 0);
        assert_eq!(hover_delay_ticks(1), 1);
        assert_eq!(hover_delay_ticks(120), 1);
        assert_eq!(hover_delay_ticks(121), 2);
        assert_eq!(hover_delay_ticks(2000), 17);
    }

    /// 「呼出保护 + 拖动/落位保护」的语义防线：宽限期内、或呼出后鼠标还没进过
    /// 窗口，都不准开始累计离窗时钟 —— 丢了这两条就是「呼出 → 立刻贴回 → 再呼出」
    /// 的互闪死循环；不够贴边时同样不累计（「正中央不触发」）。
    /// 另两条（几何在飞、主窗刚在动）是「拖动途中不许贴边」：丢了它们就是
    /// 「抓白条拖离 → 展开 → 176ms 后缩回条子」的闪跳（poke-1 实测）。
    #[test]
    fn undocked_ticks_require_near_edge_and_no_recall_guard() {
        assert!(undocked_counts_ticks(true, false, false, false, false));
        // 不够贴边
        assert!(!undocked_counts_ticks(false, false, false, false, false));
        // 呼出 800ms 宽限期内（鼠标还在托盘上）
        assert!(!undocked_counts_ticks(true, true, false, false, false));
        // 呼出后鼠标还没进过窗口（用户还没真用上界面）
        assert!(!undocked_counts_ticks(true, false, true, false, false));
        assert!(!undocked_counts_ticks(true, true, true, false, false));
        // 我们自己刚发出去的几何请求还没落位（dock/undock/展开/夹取）
        assert!(!undocked_counts_ticks(true, false, false, true, false));
        // 主窗刚刚还在动（用户正在拖 / WM 正在摆）
        assert!(!undocked_counts_ticks(true, false, false, false, true));
        assert!(!undocked_counts_ticks(true, false, false, true, true));
    }

    /// 「屏幕中间」与贴边自动隐藏互斥（用户定案 2026-10-14）：居中要求窗口钉在
    /// 屏幕正中、贴边要求窗口缩到屏幕边上，两者同时生效就是互相拉扯 —— watcher
    /// 把窗口搬到边上，居中的 Moved 回弹又把它拉回正中，再贴边再回弹，窗口在
    /// 贴边位与居中位之间来回抖动（用户报的现象）。
    /// 开关值本身保留（切回其他落位方式立刻恢复），只在居中模式下整体停用。
    #[test]
    fn auto_hide_is_mutually_exclusive_with_center_position() {
        // 开关开着 + 居中 → 不生效
        assert!(!auto_hide_should_run(true, "center"));
        // 开关开着 + 其他落位方式 → 生效（自由摆放 / 上次位置都能贴边）
        assert!(auto_hide_should_run(true, "free"));
        assert!(auto_hide_should_run(true, "last"));
        // 开关关着 → 任何落位方式都不生效
        assert!(!auto_hide_should_run(false, "free"));
        assert!(!auto_hide_should_run(false, "center"));
        // 判据走 normalize：大小写/空白一样算居中；脏值归一为 free，照旧生效
        assert!(!auto_hide_should_run(true, "  CENTER "));
        assert!(auto_hide_should_run(true, "乱写"));
    }

    /// 展开落点按**客户区**算：客户区的边正好贴回工作区那条边，另一轴保持
    /// 客户区当前坐标。用 1600x1000 工作区 + 460x900 客户区钉死数字：
    /// 底边 = 1000−900 = 100（按窗框算会得到 63 = 1000−937，偏高一个标题栏）。
    /// 顶边例外：客户区顶要**让出一个标题栏**（frame_top）——不然标题栏悬在
    /// 工作区外，Windows/macOS 上完全看不见、窗口拖不动（三端一致修的 bug）。
    #[test]
    fn undock_target_sticks_client_area_to_work_edge() {
        assert_eq!(
            undock_target(DockEdge::Bottom, 0, 0, 1600, 1000, 570, 960, 460, 900, 31),
            (570, 100)
        );
        // 顶边：窗框顶贴工作区顶 = 客户区顶落在 工作区顶 + 标题栏
        assert_eq!(
            undock_target(DockEdge::Top, 0, 0, 1600, 1000, 570, 63, 460, 900, 31),
            (570, 31)
        );
        // 顶边让位量被钳到非负（实测读到 0 时等于旧行为）
        assert_eq!(
            undock_target(DockEdge::Top, 0, 0, 1600, 1000, 570, 63, 460, 900, 0),
            (570, 0)
        );
        // 左/右边：纵向保持客户区当前 y（不是窗框 y）
        assert_eq!(
            undock_target(DockEdge::Left, 0, 0, 1600, 1000, 10, 80, 460, 900, 31),
            (0, 80)
        );
        assert_eq!(
            undock_target(DockEdge::Right, 0, 0, 1600, 1000, 1000, 80, 460, 900, 31),
            (1140, 80)
        );
        // 工作区带偏移（副屏 / 面板）时贴的是工作区边，不是屏幕边
        assert_eq!(
            undock_target(DockEdge::Bottom, 1600, 40, 1280, 960, 1700, 800, 460, 900, 31),
            (1700, 100)
        );
    }


    /// 尺寸基准：未映射的窗口 `outer_size()` 是 0x0，必须退到内尺寸。
    ///
    /// 实测（改前）：`grub 首同步: main_outer=0x0 main_inner=Ok("460x900")
    /// 初值=440x320` —— 初值 440x320 就是拿那个 0 算出来的（正确值 460x937），
    /// 窗口 show 出来后要再过 ~76ms 才被对齐线程纠正 = 「独立窗先闪一下」。
    /// 同一个 0x0 后来又在**主窗居中**那里咬了第二口（落点成屏幕外 → WM 夹到
    /// 底边 → 启动约 1s 后自己贴边），所以它现在是共用的纯函数。
    #[test]
    fn basis_skips_unmapped_outer_reading() {
        // 未映射：外框 0x0 不可用 → 退到内尺寸（460x900，只差一个标题栏）。
        assert_eq!(
            usable_window_size_basis(Some((0, 0)), Some((460, 900))),
            Some((460, 900))
        );
        // 已映射：外框可用，优先用它（对齐目标是「外框 = 主窗外框」）。
        assert_eq!(
            usable_window_size_basis(Some((460, 937)), Some((460, 900))),
            Some((460, 937))
        );
        // 只有一个轴为 0 也算不可用（重父化 / 改装饰中间态）。
        assert_eq!(
            usable_window_size_basis(Some((460, 0)), Some((460, 900))),
            Some((460, 900))
        );
        // 两个都读不出来 → None，调用方回退默认尺寸。
        assert_eq!(usable_window_size_basis(Some((0, 0)), Some((0, 0))), None);
        assert_eq!(usable_window_size_basis(None, None), None);
        assert_eq!(
            usable_window_size_basis(None, Some((460, 900))),
            Some((460, 900))
        );
    }
}

/// NVIDIA 白窗相关的用例：只在真的会加这些开关的平台上跑。
#[cfg(all(test, target_os = "linux"))]
mod linux_gpu_tests {
    use super::*;

    #[test]
    fn display_backend_follows_gdk_then_session_vars() {
        use LinuxDisplayBackend::{Unknown, Wayland, X11};
        // GDK_BACKEND 是硬指定，压过其它一切线索
        assert_eq!(
            linux_display_backend(Some("wayland"), Some("x11"), None, Some(":0")),
            Wayland
        );
        assert_eq!(
            linux_display_backend(Some("x11"), Some("wayland"), Some("wayland-0"), None),
            X11
        );
        // 逗号列表：第一个认得出的算数，认不出的（broadway）跳过
        assert_eq!(
            linux_display_backend(Some("broadway,x11"), None, None, None),
            X11
        );
        assert_eq!(
            linux_display_backend(
                Some("broadway"),
                Some("wayland"),
                Some("wayland-1"),
                Some(":0")
            ),
            Wayland
        );
        // 没设 GDK_BACKEND 才轮到 XDG_SESSION_TYPE
        assert_eq!(
            linux_display_backend(None, Some("wayland"), Some("wayland-0"), Some(":0")),
            Wayland
        );
        // 都没有就用变量兜底；XWayland 下 DISPLAY 也在，Wayland 的判定必须排在它前面
        assert_eq!(
            linux_display_backend(None, None, Some("wayland-0"), Some(":0")),
            Wayland
        );
        assert_eq!(linux_display_backend(None, None, None, Some(":0")), X11);
        assert_eq!(linux_display_backend(None, None, None, None), Unknown);
    }

    #[test]
    fn nvidia_major_comes_from_the_nvrm_banner() {
        // 本机（615.71.09 开源内核模块）：文件不止一行，下一行的 gcc 版本不能被当成驱动版本
        assert_eq!(
            nvidia_major_from_version_text(
                "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.71.09  Release Build  (root@arch)\n\
                 GCC version:  gcc 16.2.1 20260810 (GCC)"
            ),
            Some(615)
        );
        // 闭源模块的排法（版本号前面还有别的词）
        assert_eq!(
            nvidia_major_from_version_text(
                "NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.107.02  Tue Jul 30 01:02:03 UTC 2024"
            ),
            Some(550)
        );
        assert_eq!(nvidia_major_from_version_text(""), None);
        // 没有 NVRM 那一行就不认（宁可不动开关，也别被别的版本号带偏）
        assert_eq!(
            nvidia_major_from_version_text("GCC version:  gcc 16.2.1"),
            None
        );
        assert_eq!(
            nvidia_major_from_version_text("NVRM version: 无版本号"),
            None
        );
    }

    #[test]
    fn gpu_override_depends_on_backend_and_compositor() {
        use LinuxDisplayBackend::{Unknown, Wayland, X11};

        // X11 的开关集钉死成**这一个**：让帧走共享内存。这是 NVIDIA 上唯一能把页面
        // 画出来的开关（WebKit Bug 324551 实测）；换成 DISABLE_DMABUF_RENDERER 会退回
        // 「只有鼠标扫过才显示」，换成 DISABLE_COMPOSITING_MODE 在 2.54 上则完全是空壳。
        assert_eq!(
            X11_GPU_SWITCHES.to_vec(),
            vec![("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1")]
        );
        assert_eq!(
            linux_gpu_env_overrides(X11, Some("X-Cinnamon"), 615),
            X11_GPU_SWITCHES
        );
        // X11 上不看出身，KDE 也一样（别把 Wayland 的修法串到 X11 上）
        assert_eq!(
            linux_gpu_env_overrides(X11, Some("KDE"), 615),
            X11_GPU_SWITCHES
        );
        // 后端认不出来也按 X11 处理，不赌
        assert_eq!(
            linux_gpu_env_overrides(Unknown, None, 615),
            X11_GPU_SWITCHES
        );
        // 上一版试过的两个开关已确认无效/有害，不许再加回 X11 这一支：
        //   DISABLE_DMABUF_RENDERER → 2.52 起会清空传输模式（表现「只有鼠标扫过才显示」）
        //   DISABLE_COMPOSITING_MODE → 2.54 起完全不生效（空壳）
        for stale in [
            "WEBKIT_DISABLE_DMABUF_RENDERER",
            "WEBKIT_DISABLE_COMPOSITING_MODE",
        ] {
            assert!(
                !X11_GPU_SWITCHES.iter().any(|(name, _)| *name == stale),
                "X11 上不该再用 {stale}"
            );
        }

        // Hyprland：缺 acquire point 会被掐连接，DMA-BUF 必须关
        assert_eq!(
            linux_gpu_env_overrides(Wayland, Some("Hyprland"), 615),
            DMABUF_RENDERER_OFF
        );
        // KWin 一类严格同步的合成器：关 NVIDIA 显式同步
        assert_eq!(
            linux_gpu_env_overrides(Wayland, Some("KDE"), 615),
            NVIDIA_EXPLICIT_SYNC_OFF
        );
        assert_eq!(
            linux_gpu_env_overrides(Wayland, Some("Plasma"), 615),
            NVIDIA_EXPLICIT_SYNC_OFF
        );
        // 其它 Wayland + 驱动 ≥560（自带 egl-wayland2）：本来是好的，别动
        assert_eq!(
            linux_gpu_env_overrides(Wayland, Some("GNOME"), 615),
            NO_GPU_SWITCHES
        );
        assert_eq!(linux_gpu_env_overrides(Wayland, None, 560), NO_GPU_SWITCHES);
        // 老驱动没有 egl-wayland2，不关显式同步可能连不上
        assert_eq!(
            linux_gpu_env_overrides(Wayland, Some("GNOME"), 470),
            NVIDIA_EXPLICIT_SYNC_OFF
        );
        // X11 专属的「帧走共享内存」开关不许串到 Wayland 各支上：那边的问题是
        // acquire point / 显式同步，用错了开关反而会引出别的毛病
        for (desktop, driver) in [
            (Some("Hyprland"), 615),
            (Some("KDE"), 615),
            (Some("GNOME"), 615),
            (Some("GNOME"), 470),
        ] {
            let picked = linux_gpu_env_overrides(Wayland, desktop, driver);
            assert!(
                !picked
                    .iter()
                    .any(|(name, _)| *name == "WEBKIT_DMABUF_RENDERER_FORCE_SHM"),
                "Wayland({desktop:?}) 不该用 X11 的共享内存开关"
            );
        }
    }

    #[test]
    fn user_configured_switches_are_left_alone() {
        // 四个开关一个都没设：才轮到我们动手
        assert!(!webkit_env_already_configured(|_| false));
        // 任何一个被设过（哪怕显式设成 0）都算「用户自己管了」，全部放行
        for name in WEBKIT_ENV_SWITCHES {
            assert!(webkit_env_already_configured(|probe| probe == name));
        }
        assert!(webkit_env_already_configured(
            |name| name == "__NV_DISABLE_EXPLICIT_SYNC"
        ));
    }
}
