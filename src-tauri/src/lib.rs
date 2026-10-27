//! # lib.rs —— 装配与命令注册表（2026-10-18 拆分后）
//!
//! 本文件只保留：`run()` 装配、子进程辅助 `command()`、settings/实例锁/配置目录
//! IO、命令注册表（`generate_handler!`）与命令面（set_* / 远程命令同款入口）、
//! hub 开窗命令、`ShowOrigin`/`show_main_window`、测试模块（不搬，见方案规则 8）。
//!
//! ## 分区地图（谁在哪）
//!
//! | 文件 | 职责 |
//! |---|---|
//! | state.rs | 共享结构与共享 static（AppState/Settings/BusyState/Channels/UI_REVISION），冻结勿动 |
//! | scan.rs | lsblk / 变量目录扫描 |
//! | switch_flow.rs | 切换状态流（state_snapshot/发布/切换命令） |
//! | esp.rs | ESP 分区识别（三平台 probe） |
//! | mount.rs | 挂载辅助（ActiveMount / 临时挂载 / 原子写） |
//! | login_item.rs | 开机自启（三平台副本成组；内含 macOS SMAppService 内联 mod） |
//! | power.rs | 电源动作 + 并发护栏（POWER_RELEASE_*） |
//! | grub_esp.rs | GRUB/ESP 定位 / 提权写入 / 扫描切换 + 命令面 |
//! | grub_install.rs | mkconfig / 09_anyboot 安装卸载脚本 |
//! | gpu_compat.rs | NVIDIA / WebKitGTK 兼容环境变量 |
//! | trace.rs | trace 日志与时间工具 |
//! | ui_state.rs | UI 快照发布 + WebSocket |
//! | http_server.rs | HTTP 服务：token / 健康检查 / 端口诊断 |
//! | remote_routing.rs | 远程命令路由 + 延迟确认 + 静态资源 |
//! | settings_commands.rs | settings 归一化 / 发布 / 更新命令面 |
//! | window_geometry.rs | 窗口尺寸 / 居中 / 固定显示器 |
//! | dock_autohide.rs | dock / undock / 自动隐藏 / hotzone / watcher |
//! | dock_commands.rs | set_main_* 命令面 |
//! | tray_classic.rs | classic 原生托盘菜单 + 托盘位图 |
//! | menu_model.rs | MenuNode 模型 / 菜单构建 / 指纹 |
//! | tray_menu_window.rs | 自绘托盘菜单窗 runtime |
//! | notify.rs | 提示音 + 通知窗 + 调度器 |
//! | linux_sni.rs | Linux 托盘 SNI/dbusmenu（cfg(linux)） |
//! | cli.rs / grub.rs / http.rs / hub*.rs / remote_servers.rs / blocking_guard.rs | 既有模块，未动 |
//!
//! 共享定位规则：lib.rs 对每个分区 `pub(crate) use <mod>::*;`，子模块 `use super::*;`
//! 顺着 glob 链能看到全 crate；跨分区条目一律 pub(crate)，私有的只在本文件用。

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
///
/// `pub(crate)`：hub 系列（ssh / sftp / ssh-keygen）也必须走这里 ——
/// 之前它们直接 `std::process::Command::new(...)`，于是文件列表、上传下载、
/// 远端复制每次操作都在桌面上闪一个黑窗口。
#[allow(unused_mut)]
pub(crate) fn command(program: &str) -> Command {
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
const FAT_FILESYSTEMS: [&str; 6] = ["vfat", "fat", "fat12", "fat16", "fat32", "exfat"];
mod cli;
mod grub;
mod hub;
// 只在测试里编译的守卫：同步 `#[tauri::command]` 不许出现阻塞调用
// （两次「点了就卡死」都是这个病根，见模块注释）。
#[cfg(test)]
mod blocking_guard;
mod hub_health;
mod hub_keys;
mod hub_power;// pub(crate)：async 命令的 __cmd__ 宏可见性跟着模块走（private mod 会报
// "macro import is private"，sync 命令没这问题）
pub(crate) mod hub_sftp;
mod hub_term;
mod hub_tunnel;
mod hub_gateway;
pub(crate) mod hub_tailscale;
mod hub_tsnet;
mod hub_vault;
// backup 的命令全是 async：__cmd__ 宏可见性跟着模块走（同 hub_sftp 的理由）
pub(crate) mod backup;
mod http;
mod probe_pool;
mod sysinfo_lite;
mod remote_servers;
mod scan;
mod switch_flow;
mod gpu_compat;
mod trace;
mod ui_state;

pub(crate) use scan::*;
pub(crate) use switch_flow::*;
pub(crate) use gpu_compat::*;
pub(crate) use trace::*;
pub(crate) use ui_state::*;

mod window_geometry;
mod dock_autohide;
mod dock_commands;
mod tray_classic;
mod menu_model;
mod tray_menu_window;
mod notify;

pub(crate) use window_geometry::*;
pub(crate) use dock_autohide::*;
pub(crate) use dock_commands::*;
pub(crate) use tray_classic::*;
pub(crate) use menu_model::*;
pub(crate) use tray_menu_window::*;
pub(crate) use notify::*;

mod http_server;
mod remote_routing;
mod settings_commands;

pub(crate) use http_server::*;
pub(crate) use remote_routing::*;
pub(crate) use settings_commands::*;

mod esp;
mod grub_esp;
mod grub_install;

pub(crate) use esp::*;
pub(crate) use grub_esp::*;
pub(crate) use grub_install::*;

/// 三引导器通道（systemd-boot / OpenCore / Clover）：async 命令宏可见性同 backup
/// 的约定，走 pub(crate) mod + pub(crate) use。
pub(crate) mod channels;
pub(crate) use channels::*;

mod login_item;
mod power;
mod mount;

pub(crate) use login_item::*;
pub(crate) use power::*;
pub(crate) use mount::*;

mod state;
pub(crate) use state::*;

/// SOL 远程唤醒标签页（sol 接收器管理：状态 / 下载 / 解压 / 安装 / 卸载 /
/// 配置 / 打开目录）。命令经 `sol_panel::` 全路径注册（generate_handler!），
/// 不走 glob —— 面板类型只在本模块与前端之间流转。
mod sol_panel;

/// 09_anyboot 的安装路径（仅 Linux 有意义）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const GRUB_SCRIPT_PATH: &str = "/etc/grub.d/09_anyboot";

/// 二进制入口（`main.rs`）用这三个：askpass 先答密码，带参数走 CLI，不带参数开 GUI。
pub use cli::{
    is_cli_invocation as cli_requested, run as run_cli,
    run_askpass_if_invoked as cli_askpass,
};

const SETTINGS_FILE: &str = "settings.json";
/// 单实例锁文件（放配置目录里，因此 `ANYBOOT_CONFIG_DIR` 也生效 —— 沙箱/多套配置互不干扰）
const INSTANCE_LOCK_FILE: &str = "anyboot.lock";
/// 上一个实例的 pid（**单独一个文件，不能写进锁文件里**：Windows 的 LockFileEx
/// 是整段独占字节锁，别的进程连读都读不到，写进去等于没写）
const INSTANCE_PID_FILE: &str = "anyboot.pid";
/// 接管旧实例的节奏：先礼 —— 等 3 秒让它自己走（走 RunEvent::Exit 才会收子进程）；
/// 再不退才兵 —— 强杀，连带把它的子孙进程一起收掉。
const INSTANCE_GRACE_POLLS: u32 = 30;
const INSTANCE_KILL_POLLS: u32 = 20;
/// 每次试锁的间隔
const INSTANCE_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// 内置 HTTP 服务的默认监听地址与端口（plan §4.1：默认开启、0.0.0.0:12300）
const DEFAULT_HTTP_BIND: &str = "0.0.0.0";
const DEFAULT_HTTP_PORT: u16 = 12300;
/// 单次请求体上限：超过就只认 header/query（防止拿巨大 body 拖住工作线程）
const MAX_REQUEST_BODY: usize = 8 * 1024;
/// 「最近请求」在内存里保留的条数
const MAX_RECENT_REQUESTS: usize = 20;
/// 远程管理（server-hub）独立窗口标签。主窗口零改动：hub 是一扇常规窗口，
/// 托盘菜单「远程管理…」是它目前唯一的入口（点开常驻，再次点击只是唤起）。
pub(crate) const HUB_WINDOW_LABEL: &str = "server-hub";
/// hub 的三个子窗口：终端 / 文件浏览（SFTP）/ 密钥管理。
/// 每个标签都必须进 capabilities/default.json 的 windows 列表，否则该窗口
/// 所有 invoke/listen 被静默拒（ACL 教训见 capabilities 文件头注释）。
pub(crate) const HUB_TERM_WINDOW_LABEL: &str = "server-term";
pub(crate) const HUB_FILES_WINDOW_LABEL: &str = "server-files";
pub(crate) const HUB_KEYS_WINDOW_LABEL: &str = "server-keys";
pub(crate) const HUB_BACKUP_WINDOW_LABEL: &str = "server-backup";
/// 托盘右键菜单的锚点窗口标签（Windows 专用、classic 方式用，见 `popup_tray_menu`）
#[cfg(target_os = "windows")]
const MENU_ANCHOR_WINDOW_LABEL: &str = "menu-anchor";
/// 锚点窗口尺寸：只是个坐标原点，多大都行（不可见、无边框）
#[cfg(target_os = "windows")]
const MENU_ANCHOR_SIZE: f64 = 8.0;
/// 绿勾位图边长（classic 原生菜单的「当前项」标记，见 check_mark_image）
#[cfg_attr(target_os = "linux", allow(dead_code))]
const CHECK_MARK_SIZE: u32 = 16;
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
/// 延迟秒数的上限（配置里乱填也不会把请求挂到天上去）
const MAX_DELAY_SECONDS: u32 = 600;
const MAX_TOKEN_LEN: usize = 128;
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
const MAIN_WINDOW_LABEL: &str = "main";

/// **唯一**的状态事件：携带完整快照 + 单调递增 revision。
///
/// 阶段 1 之前是 status-changed / settings-changed / restart-result 三个补丁式事件，
/// 各更新 UI 的一角、没有版本号，任何一次漏发或乱序都会留下不刷新的一角。
const UI_STATE_CHANGED_EVENT: &str = "ui-state-changed";

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

#[cfg(target_os = "linux")]
fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

// ==================== classic（各自方式）的原生菜单素材 ====================
//
// 托盘菜单的「classic」形态 = 三端各用系统原生菜单（三端自绘之前的原始方案）。
// 原生菜单改不了文字颜色/字重，所以「哪个系统是选中的」只能用图形表达：
// 选了的那一行带绿勾、其余行不带 —— 勾占的是菜单**独立的位图列**，
// 不占用文字，系统的名字因此天然左对齐（以前用「● 」前缀，名字会被顶偏一格）。

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

/// 追加一行 trace，同时打到 stderr。
///
/// 这是取证探针：**任何失败都吞掉**（打不开文件、磁盘满、没权限都不影响应用行为），
/// 也不能在日志里写敏感数据。
pub(crate) fn trace(app: &AppHandle, category: &str, message: &str) {
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

/// 前端异常上报。
///
/// webview 里没有可用的控制台（macOS 上也没法 attach devtools），脚本一崩用户
/// 只看到「卡住」，日志里一片干净 —— 排障全靠猜。这条命令只写 trace，别的
/// 什么都不做；去重由前端负责，超长一律截断，防止一条畸形消息刷爆日志。
#[tauri::command]
fn hub_report_error(app: AppHandle, source: String, message: String, stack: String) {
    let clip = |text: &str, max: usize| -> String {
        let flattened = text.replace(['\r', '\n'], " ");
        let trimmed = flattened.trim();
        if trimmed.chars().count() <= max {
            trimmed.to_string()
        } else {
            trimmed.chars().take(max).collect::<String>() + "…"
        }
    };
    let mut line = format!("{}: {}", clip(&source, 40), clip(&message, 300));
    if !stack.trim().is_empty() {
        line.push_str(&format!(" | {}", clip(&stack, 400)));
    }
    trace(&app, "js", &line);
}

// ==================== WebSocket 推送（网页端 / 手机端的 /api/ws） ====================
//
// 传输与握手在 http 模块，这里只管「谁订阅了」和「推什么」。推的内容与
// `/api/ui-state` 完全同源（都是 get_ui_state 的那份快照），所以两条路不会分叉。

/// 每个客户端的待发队列容量。
///
/// 故意很小：快照是**完整状态**而不是增量，中间几拍丢了不影响正确性（下一拍补齐）。
/// 队列满说明这个客户端收得比我们发得慢，这时**丢掉这一拍**，绝不阻塞 `publish_ui`
/// —— 后者会把主窗口自己的重绘一起拖住。
const WS_QUEUE: usize = 8;

/// 握手前的一次性鉴权 + 首帧内容（当前完整快照）。失败时回一个普通 HTTP 错误。
pub(crate) fn websocket_hello(app: &AppHandle, token: Option<&str>) -> Result<String, RemoteReply> {
    let settings = stored_settings(app);
    let provided = token.unwrap_or_default();
    // 网关每客户端令牌与本机 http_token 等价（与 handle_remote_request 同一闸门）
    if !token_matches(&settings.http_token, provided)
        && !hub_gateway::gateway_token_valid(app, provided)
    {
        return Err(RemoteReply::error(401, "invalid_token", "token 缺失或无效"));
    }
    Ok(ui_state_json(app))
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

/// 停靠态「重置位置」的展开过渡等待（毫秒）：undock 的重装饰/改尺寸落地
/// 期间发出的 set_position 会被 WM 重摆吞掉（实测见 center_docked_window），
/// 等它落地再把居中交给重锚牛顿步；抑制期同步立起，防 near_edge 抢跑贴回。
const DOCKED_CENTER_SETTLE_MS: u64 = 600;

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

// ---------- /api/ui-command 参数提取辅助 ----------

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
    // 远程接入配对握手在鉴权之前：A 还没有 B 的令牌，这正是配对的目的
    // （响应不含机密、nonce 单次消费、B 端人工确认，见 hub_gateway）。
    if request.path == "/api/gateway-pair-request" {
        return hub_gateway::pair_request_reply(app, request);
    }
    if request.path == "/api/gateway-pair-poll" {
        return hub_gateway::pair_poll_reply(request);
    }
    if request.path != "/health" {
        let settings = stored_settings(app);
        let provided = request.token.clone().unwrap_or_default();
        // 两类等价凭据：本机 http_token，或远程接入网关签发的每客户端令牌
        // （配对签发、可逐个吊销，见 hub_gateway）。
        if !token_matches(&settings.http_token, &provided)
            && !hub_gateway::gateway_token_valid(app, &provided)
        {
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
        // 系统信息（鉴权后）：客户端「系统信息」弹层的数据源。
        // 只有 OS / 版本 / CPU / 内存这类排障信息，不含路径与令牌。
        "/api/system" => {
            let info = remote_servers::local_system_info(app);
            let value = serde_json::to_value(info).unwrap_or_else(|_| serde_json::json!({}));
            return RemoteReply::ok(200, value);
        }
        "/api/requests" => return RemoteReply::ok(200, remote_requests_json()),
        "/api/grub" => return RemoteReply::ok(200, remote_grub_json(app)),
        // 三引导器通道视图（鉴权后）：网页端 / 手机端消费同一份
        "/api/boot-channels" => {
            let value = serde_json::to_value(boot_channels(app.clone()))
                .unwrap_or_else(|_| serde_json::json!([]));
            return RemoteReply::ok(200, value);
        }
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

/// 打开/唤起远程管理（server-hub）窗口：已存在就 unminimize + 聚焦，不存在才建。
/// 普通常规窗口（系统标题栏、可缩放、进任务栏），与主窗口的贴边/置顶语义完全无关。
fn open_hub_window(app: &AppHandle) -> Result<(), String> {
    // 复用前先体检：窗口对象在 ≠ 页面活着。白框（webview 崩了 / 首帧没加载）
    // 时只 show 会把那个白框一直 show 给用户，点多少次都一样。
    if let Some(window) = hub_health::reuse_or_clear(app, HUB_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    // 缩放分辨率铁律：init/min 都要按显示器夹（150% 缩放下 720 逻辑高 > 屏幕可用高度）
    let ((iw, ih), (mw, mh)) = fit_monitor_size(
        app.primary_monitor().ok().flatten(),
        (1080.0, 720.0),
        (860.0, 560.0),
    );
    apply_browser_args(tauri::WebviewWindowBuilder::new(
        app,
        HUB_WINDOW_LABEL,
        tauri::WebviewUrl::App("hub.html".into()),
    ))
    .title("AnyBoot 远程管理")
    .inner_size(iw, ih)
    .min_inner_size(mw, mh)
    .resizable(true)
    .center()
    .build()
    .map_err(|error| format!("无法创建远程管理窗口: {error}"))?;
    hub_health::note_created(app, HUB_WINDOW_LABEL);
    hub_health::watch_first_frame(app, HUB_WINDOW_LABEL);
    trace(app, "hub", "远程管理窗口已创建");
    Ok(())
}

/// 打开/唤起密钥管理窗（server-keys）：无上下文参数，已存在就聚焦。
pub(crate) fn open_keys_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = hub_health::reuse_or_clear(app, HUB_KEYS_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    let ((iw, ih), (mw, mh)) = fit_monitor_size(
        app.primary_monitor().ok().flatten(),
        (720.0, 560.0),
        (560.0, 420.0),
    );
    apply_browser_args(tauri::WebviewWindowBuilder::new(
        app,
        HUB_KEYS_WINDOW_LABEL,
        tauri::WebviewUrl::App("hub-keys.html".into()),
    ))
    .title("AnyBoot 密钥管理")
    .inner_size(iw, ih)
    .min_inner_size(mw, mh)
    .resizable(true)
    .center()
    .build()
    .map_err(|error| format!("无法创建密钥管理窗口: {error}"))?;
    hub_health::note_created(app, HUB_KEYS_WINDOW_LABEL);
    hub_health::watch_first_frame(app, HUB_KEYS_WINDOW_LABEL);
    trace(app, "hub", "密钥管理窗口已创建");
    Ok(())
}

/// 前端按钮入口：任意窗口都能唤起「远程管理」主窗。
///
/// 主窗口标题栏、终端 / 文件 / 密钥三个独立窗顶栏都挂这个 —— 统一入口，
/// 用户不必退回主窗或托盘。async 的理由同 `hub_open_keys_window`。
#[tauri::command]
async fn hub_open_window(app: AppHandle) -> Result<(), String> {
    open_hub_window(&app)
}

/// 前端按钮入口：hub 工具栏「密钥」
///
/// **必须是 async**：同步命令跑在 WebView2 的 IPC 回调里（UI 线程），在那里
/// 再建一个 webview 属于「在 webview 回调里重入 WebView2 消息循环」——
/// 窗口建得出来、但新页面永远停在白底首帧（不导航、不执行 JS），
/// 用户看到的就是「点开空白窗口卡死」。async 命令改在 tokio 工作线程上跑，
/// 开窗时不再重入，页面正常加载（与启动钩子那条后台线程路径一致）。
#[tauri::command]
async fn hub_open_keys_window(app: AppHandle) -> Result<(), String> {
    open_keys_window(&app)
}

/// 打开/唤起备份与恢复窗（server-backup）：无上下文参数，已存在就聚焦。
pub(crate) fn open_backup_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = hub_health::reuse_or_clear(app, HUB_BACKUP_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    let ((iw, ih), (mw, mh)) = fit_monitor_size(
        app.primary_monitor().ok().flatten(),
        (760.0, 640.0),
        (600.0, 480.0),
    );
    apply_browser_args(tauri::WebviewWindowBuilder::new(
        app,
        HUB_BACKUP_WINDOW_LABEL,
        tauri::WebviewUrl::App("hub-backup.html".into()),
    ))
    .title("AnyBoot 备份与恢复")
    .inner_size(iw, ih)
    .min_inner_size(mw, mh)
    .resizable(true)
    .center()
    .build()
    .map_err(|error| format!("无法创建备份窗口: {error}"))?;
    hub_health::note_created(app, HUB_BACKUP_WINDOW_LABEL);
    hub_health::watch_first_frame(app, HUB_BACKUP_WINDOW_LABEL);
    trace(app, "hub", "备份与恢复窗口已创建");
    Ok(())
}

/// 前端按钮入口：hub 工具栏「备份」（async 理由同 `hub_open_keys_window`）
#[tauri::command]
async fn hub_open_backup_window(app: AppHandle) -> Result<(), String> {
    open_backup_window(&app)
}

/// 前端按钮入口：hub 详情「打开终端」（按 server_id 查档案 → 开/聚焦终端窗）
///
/// async 的理由同 `hub_open_keys_window`：别在 IPC 回调里建 webview。
#[tauri::command]
async fn hub_open_term_window(app: AppHandle, server_id: String) -> Result<(), String> {
    let server = hub::load_servers_pub(&app)?
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("服务器不存在（{server_id}）"))?;
    hub_term::open_term_window(&app, server)
}

/// 前端按钮入口：hub 详情「浏览文件」
///
/// async 的理由同 `hub_open_keys_window`：别在 IPC 回调里建 webview。
#[tauri::command]
async fn hub_open_files_window(app: AppHandle, server_id: String) -> Result<(), String> {
    let server = hub::load_servers_pub(&app)?
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("服务器不存在（{server_id}）"))?;
    hub_sftp::open_files_window(&app, server)
}

// ——— 自绘托盘菜单（窗口 + 命令）———
//
// 菜单内容由 `build_menu_nodes` 生成（纯数据），这扇窗把它画出来。三端同一份内容、
// 同一个渲染器 ⇒ 同一套外观、同一套行为（用户诉求④）。
// 起这扇窗的直接原因是③：只要菜单由宿主/系统渲染（Linux 的 GtkMenu、Windows 的
// TrackPopupMenu、macOS 的 NSMenu），点任何一项它都会把菜单收掉，且没有任何开关能改。

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

/// 【临时诊断】托盘启动时独立窗口仍闪现的问题排查：无条件追加写
/// startup-debug.log（配置目录下），定位完成后整段删除。
pub(crate) fn startup_debug(app: &AppHandle, message: &str) {
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

/// 按显示器实际可用区域夹窗口的逻辑尺寸，返回 (init, min)。
///
/// **为什么必须有这一步**：Windows 显示缩放（125% / 150% / 175%）下，逻辑视口
/// = 物理分辨率 ÷ 缩放倍数。写死的 `inner_size(1080, 720)` 是逻辑像素，150% 的
/// 1080p 屏可用高度只有约 693 逻辑 px —— 窗口一开就比屏幕高，底部出屏（被任务
/// 栏挡住），用户看到的就是「远程接入页样式没自适应、看不全」。`min_inner_size`
/// 同样要夹：min 比屏幕还大时窗口永远缩不进屏，连手动自救都不行。
///
/// Monitor::size() 是物理像素，除以 scale_factor 换回逻辑再夹；留 8% 余量给
/// 任务栏 / 窗口边框 / DPI 取整误差，并给绝对下限防止极小屏把窗夹成 0。
pub(crate) fn fit_monitor_size(
    monitor: Option<tauri::Monitor>,
    init: (f64, f64),
    min: (f64, f64),
) -> ((f64, f64), (f64, f64)) {
    let Some(m) = monitor else { return (init, min); };
    let scale = m.scale_factor();
    if !(scale.is_finite()) || scale <= 0.0 {
        return (init, min);
    }
    let avail_w = m.size().width as f64 / scale;
    let avail_h = m.size().height as f64 / scale;
    let cap_w = (avail_w * 0.92).max(380.0);
    let cap_h = (avail_h * 0.92).max(320.0);
    let clamp = |(w, h): (f64, f64)| (w.min(cap_w).max(320.0), h.min(cap_h).max(260.0));
    (clamp(init), clamp(min))
}

/// 建窗统一入口包装：给 `WebviewWindowBuilder::new(...)` 的返回值套这一层。
///
/// **为什么所有窗都要过这里**：同一 user data folder 下的所有 webview 的
/// 环境参数（含 additional_browser_args）必须完全一致，否则 WebView2 直接报
/// 0x8007139F（ERROR_INVALID_STATE），一个窗都开不出来 —— 见 tauri#11144。
/// 所以「要不要带 CDP 端口」不能按窗决定，只能全局决定：设了
/// `ANYBOOT_CDP_PORT` 所有窗统一带，没设所有窗统一走 wry 默认。
pub(crate) fn apply_browser_args<R: tauri::Runtime, M: tauri::Manager<R>>(
    builder: tauri::WebviewWindowBuilder<'_, R, M>,
) -> tauri::WebviewWindowBuilder<'_, R, M> {
    #[cfg(windows)]
    if let Some(args) = webview_browser_args() {
        return builder.additional_browser_args(&args);
    }
    builder
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

// ==================== classic（各自方式）：三端原生托盘菜单 ====================
//
// 这是「三端自绘」之前的原始方案（6b7ef6c 之前的形态）：托盘图标由 tauri 托管，
// 菜单是**原生菜单**——Windows 现建现弹（锚点窗）、macOS 挂在托盘上由系统弹、
// Linux 挂在托盘上由宿主按 dbusmenu 渲染（libappindicator 那条路）。自绘窗方案
// 作为「全局统一」方式保留，两种形态按 trayMenuStyle 在启动时二选一。

// ==================== 菜单内容指纹（classic 原生菜单的「变了才重建」） ====================

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

// ============ 单实例：抢锁 + 接管上一个实例 ============
//
// 别处的进程辅助（sol_panel / hub_gateway）只做「找/杀」，这里要更小心一件事：
// **误杀**。所以按「锁 → pid 文件 → 确认是同程序的进程」三段走，任何一段对不上
// 就退回老行为（提示「已经启动」然后退出），绝不按名字乱杀。

/// 上个实例的 pid 文件路径（与锁文件同目录）
fn instance_pid_path(directory: &Path) -> PathBuf {
    directory.join(INSTANCE_PID_FILE)
}

/// 拿到锁之后登记自己的 pid（供下一个进程接管时定位）。
/// best-effort：写不进去只是「下次接管不成」，不该影响启动。
fn write_instance_pid(path: &Path) {
    let _ = fs::write(path, format!("{}\n", std::process::id()));
}

/// 读上个实例留下的 pid。空文件/乱码/0 一律当「没有」。
fn read_instance_pid(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    match text.trim().parse::<u32>() {
        Ok(0) | Err(_) => None,
        Ok(pid) => Some(pid),
    }
}

/// 某个 pid 的**可执行文件名**（只取 basename：AppImage 每次挂载路径都不同，
/// 比全路径会永远对不上）。拿不到（进程已退 / 没权限）返回 None。
#[cfg(target_os = "linux")]
fn process_image_name(pid: u32) -> Option<String> {
    let target = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    Some(target.file_name()?.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn process_image_name(pid: u32) -> Option<String> {
    // ucomm = 被执行的名字（不带路径、不带参数），与 Linux 的 /proc/pid/exe 对齐
    let output = command("ps")
        .args(["-p", &pid.to_string(), "-o", "ucomm="])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[cfg(target_os = "windows")]
fn process_image_name(pid: u32) -> Option<String> {
    // tasklist 的 IMAGENAME（"anyboot.exe"）；未命中时输出一行本地化提示语，
    // 解析自然落空 —— 与 sol_panel::sol_process_pids 同一套写法。
    let output = command("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&output.stdout);
    let first = line.lines().next()?.trim();
    let name = first.split("\",\"").next()?.trim_matches('"').trim();
    if name.is_empty() || name.parse::<u32>().is_ok() {
        None
    } else {
        Some(name.to_string())
    }
}

/// pid 是不是「本程序」的另一个进程（防误杀的核心闸门：PID 会被系统回收，
/// 光凭 pid 文件里的数字去 kill，撞上别的程序就是事故）。
fn is_same_app_process(pid: u32) -> bool {
    let Some(ours) = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
    else {
        return false;
    };
    match process_image_name(pid) {
        Some(name) => name.eq_ignore_ascii_case(&ours),
        None => false,
    }
}

/// 某个进程的全部子孙 pid。接管时就要先抓一份快照：真到强杀那一步，
/// 光杀父进程会把 ssh / cloudflared / tsnet 过继给 init 变成孤儿 ——
/// 用户看到的是「程序关了，隧道还在跑」。
#[cfg(target_os = "linux")]
fn descendant_pids(pid: u32) -> Vec<u32> {
    // 永远别把「自己」列进去：重启路径万一没放锁，旧实例就是我们的父进程
    let me = std::process::id();
    let mut found: Vec<u32> = Vec::new();
    let mut queue = vec![pid];
    while let Some(parent) = queue.pop() {
        // 子进程可能由任意线程 fork 出来，所以要把每个 task 的 children 都读一遍
        let Ok(entries) = fs::read_dir(format!("/proc/{parent}/task")) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(text) = fs::read_to_string(entry.path().join("children")) else {
                continue;
            };
            for token in text.split_whitespace() {
                let Ok(child) = token.parse::<u32>() else { continue };
                if child != pid && child != me && !found.contains(&child) {
                    found.push(child);
                    queue.push(child);
                }
            }
        }
    }
    found
}

#[cfg(target_os = "macos")]
fn descendant_pids(pid: u32) -> Vec<u32> {
    // 永远别把「自己」列进去：重启路径万一没放锁，旧实例就是我们的父进程
    let me = std::process::id();
    let mut found: Vec<u32> = Vec::new();
    let mut queue = vec![pid];
    while let Some(parent) = queue.pop() {
        let Ok(output) = command("pgrep").args(["-P", &parent.to_string()]).output() else {
            continue;
        };
        for token in String::from_utf8_lossy(&output.stdout).split_whitespace() {
            let Ok(child) = token.parse::<u32>() else { continue };
            if child != pid && child != me && !found.contains(&child) {
                found.push(child);
                queue.push(child);
            }
        }
    }
    found
}

#[cfg(target_os = "windows")]
fn descendant_pids(_pid: u32) -> Vec<u32> {
    // Windows 侧交给 `taskkill /T`（杀进程树），这里不需要自己列举
    Vec::new()
}

/// 给进程发信号 / 结束它。`force = false` 是「先礼」：unix 只发 SIGTERM，
/// 让它自己走退出流程；Windows 没有可用的优雅退出（关窗默认是「隐藏到托盘」），
/// 直接 `taskkill /T /F` —— 少了 `/T` 会把 WebView2 子进程留成孤儿。
#[cfg(target_os = "linux")]
fn signal_process(pid: u32, force: bool) {
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    // SAFETY: kill(2) 只读两个整数参数，失败返回 -1，不会破坏内存
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

#[cfg(target_os = "macos")]
fn signal_process(pid: u32, force: bool) {
    let _ = command("kill")
        .args([if force { "-KILL" } else { "-TERM" }, &pid.to_string()])
        .output();
}

#[cfg(target_os = "windows")]
fn signal_process(pid: u32, _force: bool) {
    let _ = command("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}

/// 等锁：每 `INSTANCE_POLL_INTERVAL` 试一次，最多 `attempts` 次。
fn wait_for_instance_lock(path: &Path, attempts: u32) -> Option<fs::File> {
    for _ in 0..attempts {
        std::thread::sleep(INSTANCE_POLL_INTERVAL);
        if let Ok(file) = try_lock_instance_file(path) {
            return Some(file);
        }
    }
    None
}

/// 某个进程的父 pid（用来判断「它是不是已经成了孤儿」）。拿不到返回 None。
#[cfg(target_os = "linux")]
fn process_parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // 格式：`pid (comm) state ppid ...`，comm 里可能带空格和括号 → 从最后一个 ')' 往后切
    let tail = stat.rsplit_once(')')?.1;
    let mut fields = tail.split_whitespace();
    fields.next()?; // state
    fields.next()?.parse::<u32>().ok()
}

#[cfg(target_os = "macos")]
fn process_parent_pid(pid: u32) -> Option<u32> {
    let output = command("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse::<u32>().ok()
}

/// 补收「上一个实例的子进程」。
///
/// 为什么先礼之后还要干这件事：Tauri 没装 SIGTERM 处理器，unix 上 SIGTERM 是
/// **直接死**（走不到 `RunEvent::Exit`），于是旧实例名下的 ssh / cloudflared /
/// tsnet 会被过继给 init —— 新实例起来后它们还在跑，用户看到的是「程序关了，
/// 隧道/终端还在」。这里把接管前抓到的那批 pid 补一刀。
///
/// **只杀已经变成孤儿的**（ppid 不再是旧实例）：万一旧实例其实还活着（接管失败
/// 的路径不会走到这），或者 pid 在这几百毫秒里被别人回收了，ppid 对不上就跳过 ——
/// 宁可漏杀一个残留子进程，不能杀错别人的进程。
#[cfg(not(target_os = "windows"))]
fn reap_orphaned_children(children: &[u32]) -> usize {
    let me = std::process::id();
    let mut reaped = 0;
    for pid in children {
        if *pid == me {
            continue;
        }
        let orphaned = matches!(process_parent_pid(*pid), Some(0) | Some(1));
        if orphaned {
            signal_process(*pid, true);
            reaped += 1;
        }
    }
    reaped
}

#[cfg(target_os = "windows")]
fn reap_orphaned_children(_children: &[u32]) -> usize {
    // `taskkill /T /F` 已经把整棵树带走，没有需要补收的
    0
}

/// 抢锁失败时接管旧实例：杀掉 → 等它放手 → 由本进程继续启动。
///
/// 三道闸门缺一不可：① 锁被占（不然根本不会走到这）② pid 文件里有个 believable
/// 的 pid ③ 那个 pid 确实是本程序。③ 挡的是「pid 被系统回收给了别的程序」——
/// 拿 pid 文件里的旧数字直接 kill 是这类功能最常见的事故来源。
///
/// `log` 传的是「怎么记一行」，不直接吃 AppHandle —— 这样单测能拿个空日志
/// 真跑一遍接管流程（起一个同程序的假实例占着锁，再把它接过手）。
fn takeover_previous_instance(
    log: &dyn Fn(&str),
    lock_path: &Path,
    pid_path: &Path,
) -> Result<fs::File, InstanceLockError> {
    let Some(pid) = read_instance_pid(pid_path) else {
        log("锁被占用，但没有 pid 记录（旧版本/写入失败）→ 不接管，提示「已经启动」");
        return Err(InstanceLockError::AlreadyRunning);
    };
    if pid == std::process::id() || !is_same_app_process(pid) {
        log(&format!("锁被 pid={pid} 占用，但它不是本程序的进程 → 不接管"));
        return Err(InstanceLockError::AlreadyRunning);
    }

    log(&format!("检测到上一个实例 pid={pid} → 先结束它再启动"));
    let children = descendant_pids(pid);
    signal_process(pid, false);
    if let Some(file) = wait_for_instance_lock(lock_path, INSTANCE_GRACE_POLLS) {
        // 它可能是被 SIGTERM 直接带走的（走不到 RunEvent::Exit），子进程要补收
        let reaped = reap_orphaned_children(&children);
        log(&format!(
            "上一个实例已退出 → 拿到单实例锁（补收 {reaped} 个残留子进程），继续启动"
        ));
        return Ok(file);
    }

    log(&format!(
        "上一个实例 3 秒内没退出 → 强制结束（连带 {} 个子进程）",
        children.len()
    ));
    signal_process(pid, true);
    for child in &children {
        signal_process(*child, true);
    }
    match wait_for_instance_lock(lock_path, INSTANCE_KILL_POLLS) {
        Some(file) => {
            log("强制结束之后拿到锁 → 继续启动");
            Ok(file)
        }
        None => {
            log("强制结束之后锁仍被占 → 放弃接管，提示「已经启动」");
            Err(InstanceLockError::AlreadyRunning)
        }
    }
}

/// 试着成为「唯一实例」。`Ok(())` = 本进程拿到了锁（必要时已经接管掉上一个实例）。
fn acquire_instance_lock(app: &AppHandle) -> Result<(), InstanceLockError> {
    let directory = config_dir(app).map_err(InstanceLockError::Io)?;
    fs::create_dir_all(&directory).map_err(|error| {
        InstanceLockError::Io(format!("无法创建配置目录 {}: {error}", directory.display()))
    })?;
    let lock_path = directory.join(INSTANCE_LOCK_FILE);
    let pid_path = instance_pid_path(&directory);
    let file = match try_lock_instance_file(&lock_path) {
        Ok(file) => file,
        Err(InstanceLockError::AlreadyRunning) => takeover_previous_instance(
            &|message| startup_debug(app, message),
            &lock_path,
            &pid_path,
        )?,
        Err(error) => return Err(error),
    };
    // 锁一到手就登记 pid：下一个进程靠它认出「谁在跑」，晚一步就可能读到上一轮的旧值
    write_instance_pid(&pid_path);
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

pub(crate) fn stored_settings(app: &AppHandle) -> Settings {
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
        // show **之前**先铺一层主题底色：主窗是 transparent 窗体，而页面在首份
        // 快照渲染完成前 body 是 opacity:0（防「闪旧页面」），这段时间窗口是空的
        // —— 用户看到的「闪一下」就是这一帧透出桌面/白色。NSWindow 背景色是主线程
        // 原生属性、立即生效，不等 WebView 进程，正好兜住这一帧。
        // 页面渲染完（data-ready）后 body 自己铺满实色，这层底色被完全盖住。
        let _ = window.set_background_color(Some(dock_autohide::undock_mask_color(app, &window)));
        // **先落位再显示**：窗口隐藏期间 WKWebView 不合成，此时 set_position 只是
        // 记下目标坐标，OS 不会画出来 —— show 的第一帧就在最终位置，不再「先按
        // 上次位置出现、再跳到目标位置」（用户反馈的历史遗留「位置跳一下」）。
        // 只处理未被最小化的常规呼出；最小化的窗口位置交给下方自愈线程
        //（OS 的异步恢复会覆盖我们提前设的坐标）。
        if !was_minimized {
            apply_main_window_position(app);
        }
        let _ = window.show();
        let _ = window.unminimize();
        // 呼出即唤醒页面：主窗隐藏期间 WKWebView 的 JS 会被 macOS 深度冻结
        //（实测：startInTray 启动即 hide 后，get_ui_state/listen/setInterval
        // 全部没跑完就停了，show 出来也不会自动恢复 —— 三个栏目「有框架没数据」
        // 100% 复现，而 hub 等常显窗口正常）。show 这一拍 eval 前端唤醒钩子：
        // 页面渲染过 = 补拉一拍快照；没渲染过 = 整页重载（此刻窗口可见，重载的
        // 初始化会正常执行）。随后再 publish 一份最新快照给已活过来的监听器。
        // 延迟 250ms 再唤醒：立即 eval 可能撞上「页面首次加载还没完成」。
        let wake = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let _ = wake.eval("window.__ANYBOOT_REVIVE__ && window.__ANYBOOT_REVIVE__()");
        });
        publish_ui(app);
        // 呼出即置顶（受「窗口始终置顶」设置控制）：Linux 下 WM 的
        // focus-stealing-prevention 会让 set_focus 无效，于是窗口被别的窗口
        // 盖住（用户看到的就是「点了托盘但窗口没跳出来」）。设置关掉时窗口
        // 按普通层级呼出，不再强制压到最上层。
        if stored_settings(app).main_always_on_top {
            let _ = window.set_always_on_top(true);
        }
        let _ = window.set_focus();
        // 固定显示位置：**未被最小化**的常规呼出已在 show 之前落好位（见上），
        // 这里不再重复调用 —— 重复 set_position 会让 macOS 合成两帧、看起来
        // 就是「位置跳一下」。被最小化过的窗口仍交给下方自愈线程在真正恢复后落位
        //（OS 的 unminimize 异步恢复会覆盖提前设的坐标）。
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
                    // 常规呼出（未最小化）：show 之前已落好位，这里**只在位置确实
                    // 不对时**纠正（比如 OS 把窗口摆到了屏幕外），避免重复 set_position
                    // 造成「位置跳一下」的第二帧。最小化恢复：必须重新落位。
                    if was_minimized {
                        apply_main_window_position(&app);
                    }
                    // 位置合法性已由 apply_main_window_position 内部的
                    // enforce_fixed_monitor / clamp 保证；常规呼出时不再额外 set_position
                    //（多做一次就会看到窗口跳一下）。
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

fn attempt(program: &'static str, args: &[&str], needs_gui_auth: bool) -> PowerAttempt {
    PowerAttempt {
        program,
        args: args.iter().map(|arg| arg.to_string()).collect(),
        needs_gui_auth,
    }
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

// ---------------------------------------------------------------------------
// 收集（仅 Linux）与读取
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Linux 专属：安装 / 卸载 09_anyboot
// ---------------------------------------------------------------------------

// ---- GRUB 细粒度切换的命令面 ----

#[tauri::command]
fn set_grub_script(app: AppHandle, script: String) -> Result<(), String> {
    trace(&app, "command", "set_grub_script");
    let mut settings = stored_settings(&app);
    settings.grub_script_override = script;
    save_settings(&app, &settings)?;
    publish_ui(&app);
    Ok(())
}

/// 原实现：同步跑 lsblk/mount/grub-mkconfig 等子进程，**只在阻塞池里调用**。
fn detect_esp_blocking(app: AppHandle, path: String) -> Result<String, String> {

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

/// 原实现：同步跑 lsblk/mount/grub-mkconfig 等子进程，**只在阻塞池里调用**。
fn sync_grub_blocking(app: AppHandle) -> Result<(), String> {

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

/// 安装 09_anyboot + 重新生成 grub.cfg（仅 Linux）。
#[tauri::command]
async fn install_grub_script(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || install_grub_script_blocking(app))
        .await
        .map_err(|error| format!("install_grub_script 任务失败: {error}"))?
}

/// 原实现：同步跑 lsblk/mount/grub-mkconfig 等子进程，**只在阻塞池里调用**。
fn install_grub_script_blocking(app: AppHandle) -> Result<(), String> {

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
async fn uninstall_grub_script(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || uninstall_grub_script_blocking(app))
        .await
        .map_err(|error| format!("uninstall_grub_script 任务失败: {error}"))?
}

/// 原实现：同步跑 lsblk/mount/grub-mkconfig 等子进程，**只在阻塞池里调用**。
fn uninstall_grub_script_blocking(app: AppHandle) -> Result<(), String> {

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
        crate::login_item::login_item::open_system_settings()
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
///
/// 主题只有一个真相源（settings.theme），主窗口、远程管理（server-hub / server-term /
/// server-files / server-keys）、托盘菜单全都读它。任何一处入口改主题，落盘后这里
/// 广播 `theme-changed`，其它窗口立即跟随 —— 前端各入口只调 set_theme + 监听本事件。
#[tauri::command]
fn set_theme(app: AppHandle, theme: String) -> Result<(), String> {
    trace(&app, "command", "set_theme（主窗口）");
    let theme = normalize_theme(&theme);
    update_settings_and_publish(
        &app,
        "界面主题",
        &format!("已切换为 {theme}"),
        |settings| settings.theme = theme.clone(),
        |app| {
            // 广播给所有窗口（hub 系列 / 托盘菜单 / 弹窗），没开的窗口收不到就算了
            const THEME_CHANGED_EVENT: &str = "theme-changed";
            for label in [
                HUB_WINDOW_LABEL,
                HUB_TERM_WINDOW_LABEL,
                HUB_FILES_WINDOW_LABEL,
                HUB_KEYS_WINDOW_LABEL,
                TRAY_MENU_WINDOW_LABEL,
                NOTIFY_WINDOW_LABEL,
            ] {
                let _ = app.emit_to(label, THEME_CHANGED_EVENT, &theme);
            }
        },
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
        // 远程接入网关：cloudflared 通道进程 + 日志环 + 快照（退出时 kill_all）
        .manage(hub_gateway::GatewayTunnelState::default())
        // hub 独立窗的页面心跳：窗口对象还在 ≠ 页面还活着（白框判据，见 hub_health.rs）
        .manage(hub_health::HubPageHealth::default())
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
                        // **主窗拿到焦点 = 任何让它变可见的路径**（托盘「显示」、
                        // 拖拽、Dock 点击、Cmd+Tab…）。只挂在 show_main_window 上
                        // 覆盖不全：用户直接拖着贴边白条展开窗口时根本不会走那条路，
                        // 而 macOS 上 startInTray 隐藏过的主窗 WKWebView 可能整页
                        // 没加载（实测：启动后主窗窗口在、贴边事件有，但零 IPC、
                        // 零 render 心跳 —— 三个栏目只剩 HTML 框架）。这里统一唤醒：
                        // 页面活着就补拉一拍，没加载就 reload —— 与 show_main_window
                        // 同一钩子。前端钩子幂等（渲染过只 pull），不会反复重载。
                        // 这里是 Window 事件（不带 webview 方法），按 label 取回 webview 再 eval。
                        // 延迟 250ms：立即 eval 会撞上「页面首次加载还没完成」，钩子不存
                        // 在就只能重载，反而闪一下。钩子幂等，已渲染过只补拉一拍。
                        if let Some(webview) =
                            window.app_handle().get_webview_window(MAIN_WINDOW_LABEL)
                        {
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(250));
                                let _ = webview.eval(
                                    "window.__ANYBOOT_REVIVE__ && window.__ANYBOOT_REVIVE__()",
                                );
                            });
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
            // 抢不到锁时**先接管**：认出上一个实例的 pid、确认是本程序，然后结束它
            // （详见 takeover_previous_instance），接管成功就照常往下启动。
            // 只有「接管不了」才会走到已经启动提示：认不出 pid、pid 已经不是本程序、
            // 或者强杀之后锁还没放手。**后面一行都不做** —— 建托盘会多出第二颗图标、
            // 起监听会跟第一例抢端口、显示主窗口会凭空多出一扇窗，那些都是
            // 「另一个实例」在用户眼里的样子。
            match acquire_instance_lock(app.handle()) {
                Ok(()) => {}
                Err(InstanceLockError::AlreadyRunning) => {
                    startup_debug(
                        app.handle(),
                        "上一个实例接管不了（认不出/杀不掉）→ 提示「已经启动」后退出",
                    );
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
            // —— 主窗从 config 挪到代码里建 ——
            // 理由：CDP 调试端口。WebView2 运行时（155.0.4283.60 实测）不再把
            // WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS 拼进浏览器进程命令行 —— wry 经
            // API 传的 additional_browser_args 优先生效，环境变量路线静默失效。
            // 主窗是第一个 webview（浏览器进程参数由它决定），在代码里建才能在
            // 需要时显式带上 --remote-debugging-port。未设 ANYBOOT_CDP_PORT 时，
            // 这里的参数与原 config「windows」逐项一致，生产行为不变。
            {
                let builder = apply_browser_args(tauri::WebviewWindowBuilder::new(
                    app.handle(),
                    MAIN_WINDOW_LABEL,
                    tauri::WebviewUrl::default(),
                ))
                .title("AnyBoot")
                .inner_size(460.0, 900.0)
                .resizable(true)
                .transparent(true)
                .skip_taskbar(true)
                .always_on_top(true)
                .visible(false);
                if let Err(error) = builder.build() {
                    startup_debug(app.handle(), &format!("主窗口创建失败: {error}"));
                    return Err(error.into());
                }
            }
            let settings = read_settings_file(app.handle());
            // 测试钩子（默认不生效）：WebView2 不接受注入的鼠标点击（原生菜单可以、
            // 网页里不行），所以「远程管理 → 打开终端」这条链没法靠脚本点出来。
            // 设 ANYBOOT_TEST_OPEN_HUB=1 → 启动后自动打开远程管理窗；
            // 再加 ANYBOOT_TEST_OPEN_TERM / _FILES / _KEYS = 1 → 顺带用第一台服务器
            // 开终端窗 / 文件窗（密钥窗不需要服务器）。
            // 只影响「开哪个窗口」，不改变任何业务路径 —— 验证的就是用户点出来的那条链。
            if std::env::var("ANYBOOT_TEST_OPEN_HUB").is_ok() {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    // 钩子失败必须留痕：`let _ =` 吞掉错误会让「窗口没开」变成悬案
                    match open_hub_window(&handle) {
                        Ok(()) => eprintln!("[hook] 远程管理窗口已打开"),
                        Err(error) => eprintln!("[hook] 打开远程管理窗口失败: {error}"),
                    }
                    let want_term = std::env::var("ANYBOOT_TEST_OPEN_TERM").is_ok();
                    let want_files = std::env::var("ANYBOOT_TEST_OPEN_FILES").is_ok();
                    let want_keys = std::env::var("ANYBOOT_TEST_OPEN_KEYS").is_ok();
                    if !(want_term || want_files || want_keys) {
                        return;
                    }
                    if want_keys {
                        match open_keys_window(&handle) {
                            Ok(()) => eprintln!("[hook] 密钥窗口已打开"),
                            Err(error) => eprintln!("[hook] 打开密钥窗口失败: {error}"),
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    match hub::load_servers_pub(&handle) {
                        Ok(list) => match list.into_iter().next() {
                            Some(server) => {
                                if want_term {
                                    match hub_term::open_term_window(&handle, server.clone()) {
                                        Ok(()) => eprintln!("[hook] 终端窗口已打开 server={}", server.name),
                                        Err(error) => eprintln!("[hook] 打开终端窗口失败: {error}"),
                                    }
                                }
                                if want_files {
                                    match hub_sftp::open_files_window(&handle, server) {
                                        Ok(()) => eprintln!("[hook] 文件窗口已打开"),
                                        Err(error) => eprintln!("[hook] 打开文件窗口失败: {error}"),
                                    }
                                }
                            }
                            None => eprintln!("[hook] hub-servers.json 里没有服务器"),
                        },
                        Err(error) => eprintln!("[hook] 读取服务器列表失败: {error}"),
                    }
                });
            }
            {
                let state = app.state::<Mutex<Settings>>();
                *state.lock().expect("settings mutex poisoned") = settings;
            }
            // 断点续传清单：应用重启后把暂停/中断的传输读回内存（传输页能看见、能续传）
            hub_sftp::load_queue_at_startup(app.handle());
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
                        apply_browser_args(tauri::WebviewWindowBuilder::new(
                            app.handle(),
                            MENU_ANCHOR_WINDOW_LABEL,
                            tauri::WebviewUrl::App("menu-anchor.html".into()),
                        ))
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
            // 远程接入网关：named 模式勾了自启就在这里拉起（quick 不自启，地址会变）
            hub_gateway::autostart_tunnel(app.handle());
            // 另两张通道卡的自启（各看自己的开关）：系统 Tailscale / 内置引擎
            hub_tailscale::autostart_connect(app.handle());
            hub_tsnet::autostart_engine(app.handle().clone());

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
            remote_servers::remote_promote_address,
            remote_servers::remote_note_machine,
            remote_servers::remote_system_info,
            remote_servers::remote_power,
            remote_servers::remote_endpoint,
            remote_servers::remote_test_via,
            remote_servers::remote_boot_info,
            remote_servers::remote_boot_switch,
            remote_servers::local_http_endpoint,
            remote_servers::remote_open_page,
            remote_servers::hub_about_open,
            hub::hub_list_servers,
            // hub 独立窗的页面心跳（白框判据，见 hub_health.rs）
            hub_health::hub_page_ping,
            // 前端异常上报（webview 无控制台，只写 trace.log）
            hub_report_error,
            hub::hub_save_server,
            hub::hub_delete_server,
            hub::hub_test_server,
            hub_open_keys_window,
            hub_open_term_window,
            hub_open_files_window,
            hub_open_window,
            hub_open_backup_window,
            backup::backup_export,
            backup::backup_preview,
            backup::backup_import,
            backup::backup_sync_get_state,
            backup::backup_sync_save_config,
            backup::backup_sync_set_enabled,
            backup::backup_webdav_test,
            backup::backup_webdav_mkdir,
            backup::backup_sync_now,
            backup::backup_pick_import_file,
            backup::backup_pick_export_path,
            channels::boot_channels,
            channels::switch_boot_channel,
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
            hub_sftp::hub_sftp_transfer_list,
            hub_sftp::hub_focus_window,
            hub_sftp::hub_sftp_rename,
            hub_sftp::hub_sftp_copy,
            hub_sftp::hub_sftp_readlink,
            hub_sftp::hub_sftp_download,
            hub_sftp::hub_sftp_upload,
            hub_sftp::hub_download_dir_default,
            hub_sftp::hub_pick_directory,
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
            hub_gateway::gateway_binary_status,
            hub_gateway::gateway_binary_download,
            hub_gateway::gateway_binary_set_manual,
            hub_gateway::gateway_binary_uninstall,
            hub_gateway::gateway_clear_credentials,
            hub_tailscale::gateway_tailscale_status,
            hub_tailscale::gateway_tailscale_toggle,
            hub_tailscale::gateway_tailscale_service,
            hub_tailscale::gateway_tailscale_download,
            hub_tailscale::gateway_tailscale_install,
            hub_tailscale::gateway_tailscale_launch_gui,
            hub_tailscale::gateway_tailscale_open_extension_settings,
            hub_tailscale::gateway_tailscale_funnel_status,
            hub_tailscale::gateway_tailscale_funnel_enable,
            hub_tailscale::gateway_tailscale_funnel_disable,
            hub_tailscale::gateway_tailscale_uninstall,
            hub_tailscale::gateway_tailscale_logs,
            hub_tailscale::gateway_tailscale_logs_clear,
            hub_tailscale::gateway_tailscale_set_authkey,
            hub_tailscale::gateway_tailscale_logout,
            hub_tsnet::tsnet_status,
            hub_tsnet::tsnet_logs,
            hub_tsnet::tsnet_logs_clear,
            hub_tsnet::tsnet_uninstall,
            hub_tsnet::tsnet_logout,
            hub_tsnet::tsnet_enable,
            hub_tsnet::tsnet_disable,
            hub_tsnet::tsnet_set_authkey,
            hub_tailscale::local_service_health,
            hub_gateway::gateway_tunnel_start,
            hub_gateway::gateway_tunnel_stop,
            hub_gateway::gateway_tunnel_status,
            hub_gateway::gateway_tunnel_logs,
            hub_gateway::gateway_tunnel_logs_clear,
            hub_gateway::gateway_tunnel_orphans,
            hub_gateway::gateway_tunnel_adopt,
            hub_gateway::gateway_tunnel_kill_orphan,
            hub_gateway::gateway_pair_issue,
            hub_gateway::gateway_pair_reissue,
            hub_gateway::gateway_pair_list,
            hub_gateway::gateway_pair_revoke,
            hub_gateway::gateway_pair_delete,
            hub_gateway::gateway_pair_pending,
            hub_gateway::gateway_pair_decide,
            hub_gateway::gateway_pair_fetch,
            hub_gateway::gateway_config,
            hub_gateway::gateway_set_autostart,
            hub_gateway::gateway_save_config,
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
            sol_panel::sol_status,
            sol_panel::sol_config_read,
            sol_panel::sol_config_write,
            sol_panel::sol_install_download,
            sol_panel::sol_extract,
            sol_panel::sol_pick_dir,
            sol_panel::sol_default_dir,
            sol_panel::sol_pick_archive,
            sol_panel::sol_http_get,
            sol_panel::sol_http_set,
            sol_panel::sol_token_generate,
            sol_panel::sol_logs,
            sol_panel::sol_status_privileged,
            sol_panel::sol_local_package,
            sol_panel::sol_project_info,
            sol_panel::sol_project_open,
            sol_panel::sol_log_client_error,
            sol_panel::sol_one_click_install,
            sol_panel::sol_one_click_install_terminal,
            sol_panel::sol_install_terminal,
            sol_panel::sol_install_silent,
            sol_panel::sol_uninstall_terminal,
            sol_panel::sol_uninstall_silent,
            sol_panel::sol_service_ctl,
            sol_panel::sol_open_dir,
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
            // macOS Dock 图标点击 / `open -a`（应用已运行时）触发 Reopen：
            // 与托盘「显示」同一条路 —— show_main_window（内含唤醒 eval + publish）。
            // 没有这条，隐藏期间被冻结的主窗页面永远等不到初始化机会。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main_window(app, ShowOrigin::Recall);
            }
            // 应用退出：收掉 server-hub 名下的终端会话与隧道进程，
            // 别让 ssh 子进程被过继给 init 变成孤儿。
            if let tauri::RunEvent::Exit = event {
                hub_term::kill_all(app);
                hub_sftp::kill_all(app);
                hub_tunnel::kill_all(app);
                hub_gateway::kill_all(app);
                // 内置 Tailscale 引擎（tsnet sidecar）子进程一并收掉：
                // 正常退出走 stdin quit；兜底 kill（stdin 已随锁销毁时）
                hub_tsnet::kill_all();
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
mod linux_sni;

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
        // wake：HTTP 能送达 = 机器醒着，直接回执不做动作（唤醒路由的 HTTP 兜底靠它）
        assert_eq!(
            route_remote_command(&sample_request("/api/power", &[("action", "wake")], true)),
            Ok(RemoteCommand::WakeAck)
        );
        assert_eq!(
            route_remote_command(&sample_request("/api/power", &[("action", "WAKE")], true)),
            Ok(RemoteCommand::WakeAck)
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

    #[test]
    fn instance_pid_file_roundtrip_and_garbage() {
        // 接管靠这个文件认出「谁在跑」，所以它对乱码必须**零容忍**：
        // 读不出合法 pid 就返回 None（宁可不接管，也不能拿垃圾数字去 kill）。
        let dir = std::env::temp_dir().join(format!("anyboot-pid-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("建临时目录");
        let path = instance_pid_path(&dir);

        write_instance_pid(&path);
        assert_eq!(
            read_instance_pid(&path),
            Some(std::process::id()),
            "pid 必须能原样读回"
        );

        fs::write(&path, "not-a-pid\n").expect("写乱码");
        assert_eq!(read_instance_pid(&path), None, "乱码当「没有」");
        fs::write(&path, "0\n").expect("写 0");
        assert_eq!(read_instance_pid(&path), None, "0 不是合法 pid");
        fs::write(&path, " 4242 \n").expect("写带空白的 pid");
        assert_eq!(read_instance_pid(&path), Some(4242), "两侧空白要吃掉");
        fs::write(&path, "").expect("写空文件");
        assert_eq!(read_instance_pid(&path), None, "空文件当「没有」");

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn wait_for_instance_lock_polls_until_the_holder_lets_go() {
        // 接管的等待循环：旧实例还活着时不能提前返回句柄（否则两个实例同时跑）
        let dir = std::env::temp_dir().join(format!("anyboot-wait-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join(INSTANCE_LOCK_FILE);
        let _ = fs::remove_file(&path);

        let holder = try_lock_instance_file(&path).expect("先占住锁");
        assert!(
            wait_for_instance_lock(&path, 2).is_none(),
            "锁还被占着就不能返回 handle"
        );
        holder.unlock().expect("放锁");
        assert!(
            wait_for_instance_lock(&path, 5).is_some(),
            "旧实例放手之后必须能等到锁"
        );

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn descendant_pids_covers_children_so_force_kill_leaves_no_orphans() {
        // 强杀旧实例时不连带收子进程，ssh / cloudflared / tsnet 会变成孤儿
        let mut child = Command::new("sh")
            .args(["-c", "sleep 30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("起子进程");
        let children = descendant_pids(std::process::id());
        assert!(
            children.contains(&child.id()),
            "子进程必须在清单里: {children:?}"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    /// 假实例：抢住锁、写好 pid，然后等着被杀。**只由下面的接管用例 spawn**，
    /// 单独跑会白等 30 秒，所以标 ignore。
    #[ignore = "由 takeover_previous_instance_kills_it_and_takes_the_lock 按需 spawn"]
    #[test]
    fn instance_holder_helper() {
        let Ok(dir) = std::env::var("ANYBOOT_HOLD_LOCK_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let file = try_lock_instance_file(&dir.join(INSTANCE_LOCK_FILE)).expect("helper 拿锁");
        write_instance_pid(&instance_pid_path(&dir));
        std::thread::sleep(Duration::from_secs(30));
        drop(file);
    }

    /// 接管全链路：起一个**同程序的**假实例占住锁 → 认出它 → 结束它 → 本进程拿到锁。
    /// 这是「打开软件自动杀掉上一个进程」这条需求的唯一真验证（GUI 起不来也能跑）。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn takeover_previous_instance_kills_it_and_takes_the_lock() {
        let dir = std::env::temp_dir().join(format!("anyboot-takeover-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("建临时目录");
        let lock_path = dir.join(INSTANCE_LOCK_FILE);
        let pid_path = instance_pid_path(&dir);
        let _ = fs::remove_file(&lock_path);
        let _ = fs::remove_file(&pid_path);

        let exe = std::env::current_exe().expect("测试二进制路径");
        let mut holder = Command::new(&exe)
            .args([
                "core_tests::instance_holder_helper",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env("ANYBOOT_HOLD_LOCK_DIR", &dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("起假实例");

        // 等假实例真把锁和 pid 落下去（起进程是异步的）
        let mut ready = false;
        for _ in 0..50 {
            if read_instance_pid(&pid_path) == Some(holder.id()) {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(ready, "假实例没能在 5 秒内占住锁");

        let logs: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let log = |message: &str| logs.lock().unwrap().push(message.to_string());
        let file = takeover_previous_instance(&log, &lock_path, &pid_path)
            .expect("应该能把上一个实例接管掉");

        // 拿到的锁必须真独占（同进程再开 fd 也会被挡）
        assert!(
            matches!(
                try_lock_instance_file(&lock_path),
                Err(InstanceLockError::AlreadyRunning)
            ),
            "接管之后锁必须被本进程独占: {:?}",
            logs.lock().unwrap()
        );

        // 旧实例必须真的被结束了
        let mut exited = false;
        for _ in 0..30 {
            if matches!(holder.try_wait(), Ok(Some(_))) {
                exited = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(exited, "旧实例没被结束: {:?}", logs.lock().unwrap());
        let _ = holder.kill();
        let _ = holder.wait();

        drop(file);
        let _ = fs::remove_file(&lock_path);
        let _ = fs::remove_file(&pid_path);
        let _ = fs::remove_dir(&dir);
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
