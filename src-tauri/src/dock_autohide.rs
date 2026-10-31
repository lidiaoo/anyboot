//! dock / undock / 自动隐藏 / hotzone / watcher。与 window_geometry 共用
//! 几何护栏（geometry_busy）。

use super::*;

/// 几何稳定抑制期（Unix 毫秒）：dock（缩条）/ undock（展开）会连续改窗口
/// 尺寸与位置，这段过渡期里读到的坐标/尺寸都是中间态，拿它做判定就是在
/// 跟自己的几何编程打架（贴边成条→立刻被弹回的互闪就是这么来的）。
pub(crate) static GEOMETRY_BUSY_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

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
pub(crate) fn geometry_busy() -> bool {
    unix_millis() < GEOMETRY_BUSY_UNTIL_MS.load(Ordering::SeqCst)
}

/// 进入几何抑制期。必须在**发出**几何请求之前调用：请求是同步发出去的、
/// 配置事件是异步回来的，立晚了那批回声就落在抑制期外。
pub(crate) fn mark_geometry_busy(duration_ms: u64) {
    GEOMETRY_BUSY_UNTIL_MS.store(unix_millis() + duration_ms, Ordering::SeqCst);
}

/// 最近一条**主窗** Moved 的时间戳（Unix 毫秒，0 = 还没有过）。每次进
/// `handle_main_moved` 都盖一次。判据见 `main_moved_recently`。
pub(crate) static LAST_MAIN_MOVED_MS: AtomicU64 = AtomicU64::new(0);

/// 「屏幕中间」落位模式的拖动拦截（用户定案 2026-10-13）：居中模式下窗口
/// 位置固定、不支持拖动。CENTER_LAST_X/Y 记最近一次居中落点的**请求值**；
/// Moved 载荷落在它的 `CENTER_HOLD_TOLERANCE` 内就是「还停在居中位」（我们自己
/// 的回声、WM 抖动、去装饰后的标题栏错拍），超出才是用户把它拖走了 → 交给
/// `schedule_center_snap` 在拖动静默后收回。这里刻意**不用**「居中后 N 毫秒
/// 一律忽略」的时间窗：那会把用户在这段窗口期里的真拖动一起吃下去，松手既不
/// 回中也不提示（见 CENTER_HOLD_TOLERANCE 的注释）。
pub(crate) static CENTER_LAST_X: AtomicI32 = AtomicI32::new(0);

pub(crate) static CENTER_LAST_Y: AtomicI32 = AtomicI32::new(0);

/// 我们是否已经居中过至少一次：没有的话 Moved 一律不管（启动早期 WM 自己
/// 摆窗口的 Moved 不能当成「用户硬拖」去弹提示）。
pub(crate) static CENTER_HAS_POS: AtomicBool = AtomicBool::new(false);

/// 拖动提示的节流：拖动全程 Moved 连发，提示最多 4 秒一条。
pub(crate) static LAST_CENTER_HINT_MS: AtomicU64 = AtomicU64::new(0);

/// 主窗是不是「刚刚还在动」（距最近一条 Moved < `AUTO_HIDE_MOVE_QUIET_MS`）。
///
/// 用来挡住「拖动途中就贴边」：用户拖着窗口/拖着白条时 Moved 连发，这一拍
/// watcher 读到的「光标在窗外」是拖动的中间态，不是用户离开了窗口。
/// 实测（poke-1）：抓白条往工作区里拖 → 松手瞬间窗口展开回 460x900 →
/// watcher 176ms 后就把它缩回条子（主窗闪一下 + GRUB 窗 129ms 内翻两次）。
pub(crate) fn main_moved_recently() -> bool {
    let last = LAST_MAIN_MOVED_MS.load(Ordering::SeqCst);
    last != 0 && unix_millis().saturating_sub(last) < AUTO_HIDE_MOVE_QUIET_MS
}

pub(crate) fn main_auto_hide_enabled(app: &AppHandle) -> bool {
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
pub(crate) fn auto_hide_should_run(auto_hide_enabled: bool, position_mode: &str) -> bool {
    auto_hide_enabled && normalize_window_position(position_mode) != "center"
}

pub(crate) fn auto_hide_active(app: &AppHandle) -> bool {
    let settings = stored_settings(app);
    auto_hide_should_run(settings.main_auto_hide, &settings.main_window_position)
}

/// 按需拉起贴边 watcher：居中模式（互斥）下不起 —— 起了也只会在 `dock_main_window`
/// 的闸前空转。切回其他落位方式时由 `set_main_window_position` 再调一次。
pub(crate) fn ensure_auto_hide_watcher(app: &AppHandle) {
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
pub(crate) fn undock_target(
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
pub(crate) fn redock_edge(
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
pub(crate) fn dock_axis_position(live: i32, remembered: i32, tolerance: i32) -> i32 {
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
pub(crate) struct AutoHideReanchor {
    pub(crate) target: (i32, i32),
    pub(crate) offset: (i32, i32),
    pub(crate) tries: u8,
    /// 上一拍读到的实况。牛顿步的防失控保险丝：连续两拍实况纹丝不动 =
    /// 补发的 set_position 被 WM 钳制/忽略，再叠修正量只会让请求越飞越远
    /// （多屏桌面上会真落到远处的屏上，实测 offset 一路加到 11520、请求
    /// 飞到 15416）—— 见 undock_reanchor_action 的 GiveUp 分支。
    pub(crate) last_live: Option<(i32, i32)>,
    /// 实况连续没动的拍数（≥2 放弃）。
    pub(crate) noops: u8,
}

/// 重锚一拍的裁决（纯函数，便于单测）。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UndockReanchor {
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
pub(crate) fn undock_reanchor_action(plan: AutoHideReanchor, live_x: i32, live_y: i32) -> UndockReanchor {
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
pub(crate) fn strip_geometry(
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
pub(crate) fn hotzone_hit(
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
pub(crate) fn dock_main_window(app: &AppHandle) {
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
pub(crate) fn undock_main_window(app: &AppHandle) {
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
pub(crate) fn undock_mask_color(
    app: &AppHandle,
    main: &tauri::WebviewWindow,
) -> tauri::utils::config::Color {
    let settings = stored_settings(app);
    let alpha = (settings.opacity_percent.min(100) as u32 * 255 / 100) as u8;
    let (r, g, b) = match settings.theme.as_str() {
        "light" => (244, 246, 250),
        "dark" => (2, 6, 23),
        // system：按系统当前主题解析（解析不了按暗色兜底，与前端默认一致）
        _ => match main.theme() {
            Ok(tauri::Theme::Light) => (244, 246, 250),
            _ => (2, 6, 23),
        },
    };
    tauri::utils::config::Color(r, g, b, alpha)
}

/// 解除贴边（含用户抓着白条直接拖离）时恢复贴边前的窗口尺寸。
/// undock_main_window 自己取恢复尺寸做定位，不走这里。
pub(crate) fn restore_main_dock_size(app: &AppHandle) {
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
pub(crate) fn pin_main_topmost(app: &AppHandle) {
    if !stored_settings(app).main_always_on_top {
        return;
    }
    if let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = main.set_always_on_top(true);
    }
}

/// 当前 Unix 毫秒时间戳（贴边宽限期用）。
pub(crate) fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 各窗统一走的 WebView2 additional_browser_args（见 [`apply_browser_args`]）。
#[cfg(windows)]
pub(crate) fn webview_browser_args() -> Option<String> {
    let port = std::env::var("ANYBOOT_CDP_PORT").ok()?;
    let port = port.trim();
    if port.is_empty() {
        return None;
    }
    Some(format!(
        // wry 的默认参数原样保留（整串替换语义，缺了默认项行为会漂移），
        // 再追加 CDP 调试端口 —— WebView2 155 实测已不再应用
        // WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS 环境变量，只能这样显式传。
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port={port}"
    ))
}

/// 贴边 watcher 的逐拍取证开关：环境变量 `ANYBOOT_AUTOHIDE_DIAG=1` 打开时，
/// watcher 每秒往 startup-debug.log 打一行它实际看到的几何与判定，用来定位
/// 「为什么没贴边」。默认关 —— 那是每秒一行，常开会把日志写爆。
pub(crate) fn auto_hide_diag_enabled() -> bool {
    std::env::var("ANYBOOT_AUTOHIDE_DIAG").is_ok_and(|value| !value.is_empty() && value != "0")
}

/// 鼠标位置轮询守护线程：这个 Tauri 版本的 `WindowEvent` 没有 CursorEntered /
/// CursorLeft 事件，所以「鼠标在不在窗口里」用 `AppHandle::cursor_position()`
/// 每 120ms 扫一次 —— 没贴边且光标在外超过 350ms 就贴边；贴着边且光标压到
/// 露出的那条上就展开。开关关掉时线程自己退出。
pub(crate) fn start_auto_hide_watcher(app: &AppHandle) {
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
pub(crate) fn undocked_counts_ticks(
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
pub(crate) fn auto_hide_near_edge(
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
pub(crate) enum DockedMove {
    /// 坐标与停靠位差得远 = 用户抓着白条把它拖离了 → 解除停靠
    Release,
    /// 坐标就在停靠位（容差内）= 贴边后的正常状态，什么都不做
    Rest,
}

/// 停靠态收到 Moved 时该做什么（调用方负责先挡掉几何抑制期内的回声）。
/// 纯函数，便于单测。
pub(crate) fn docked_move_action(pos: (i32, i32), dock_pos: (i32, i32), tolerance: i32) -> DockedMove {
    if (pos.0 - dock_pos.0).abs() <= tolerance && (pos.1 - dock_pos.1).abs() <= tolerance {
        return DockedMove::Rest;
    }
    DockedMove::Release
}

/// 「屏幕中间」模式下收到主窗 `Moved` 的处置结论。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CenterMove {
    /// 载荷仍在最近的居中落点附近（我们自己的回声 / WM 抖动 / 标题栏错拍）
    Rest,
    /// 偏离居中位 = 用户把窗口拖走了（或 WM 把它摆走了）→ 收回居中位
    Snap,
}

/// 居中模式收到 Moved 时该做什么。纯函数，便于单测。
/// `payload` = Moved 载荷（窗框坐标），`last` = 最近一次居中落点的请求值。
pub(crate) fn center_move_action(payload: (i32, i32), last: (i32, i32), tolerance: i32) -> CenterMove {
    if (payload.0 - last.0).abs() <= tolerance && (payload.1 - last.1).abs() <= tolerance {
        return CenterMove::Rest;
    }
    CenterMove::Snap
}

/// 居中回声的最终裁判（纯函数，便于单测）：Moved 载荷说「动了」之后，
/// 再看窗口**真实几何读数**——窗框坐标与客户区坐标任一还贴着最近落点，
/// 就认定没动（载荷的坐标语义在 Muffin 上窗框/客户区来回错拍，两个口径
/// 各差一个标题栏、叠加可达 128px，单看载荷必然把自己的回声当拖动）。
/// 两个读数都拿不到（窗口没了）时保守返回 false，走原 snap 流程。
pub(crate) fn center_echo_still_held(
    outer: Option<(i32, i32)>,
    inner: Option<(i32, i32)>,
    last: (i32, i32),
    tolerance: i32,
) -> bool {
    outer
        .map(|p| center_move_action(p, last, tolerance) == CenterMove::Rest)
        .unwrap_or(false)
        || inner
            .map(|p| center_move_action(p, last, tolerance) == CenterMove::Rest)
            .unwrap_or(false)
}

/// 贴边停靠态下 watcher 每一拍的处理结论。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DockedPoll {
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
pub(crate) fn docked_poll_action(
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
pub(crate) fn hover_delay_ticks(ms: u64) -> u32 {
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
pub(crate) fn window_client_position(window: &tauri::WebviewWindow) -> Option<(i32, i32)> {
    let pos = window.inner_position().ok()?;
    Some((pos.x, pos.y))
}

/// 主窗口的客户区绝对位置（`window_client_position` 的主窗快捷入口）。
pub(crate) fn main_client_position(app: &AppHandle) -> Option<(i32, i32)> {
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
pub(crate) fn spawn_diag_recall(app: &AppHandle) {
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
pub(crate) fn spawn_diag_menu(app: &AppHandle) {
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
pub(crate) fn spawn_diag_force_dock(app: &AppHandle) {
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
pub(crate) fn handle_main_moved(app: &AppHandle, position: tauri::PhysicalPosition<i32>) {
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
        // 载荷口径在 Muffin/GTK 上不可信：窗框/客户区坐标语义会错拍
        // （CSD/SSD 切换、set_decorations 翻转），差值能到一整个标题栏
        // （64px），两重口径叠加可达 128px > CENTER_HOLD_TOLERANCE(80) ——
        // **我们自己居中的回声**会被判成「用户拖动」→ snap → 再居中 →
        // 再回声 = ~530ms 无限居中环（startup-debug.log 实测 2026-10-31：
        // `center main -> (1460,274)` 每半秒一条，窗口狂闪）。裁判改用
        // 真实几何读数：窗框**或**客户区任一还贴着落点，就是没动；
        // 真拖动两个读数都远离落点，照常收回。
        if center_echo_still_held(
            app.get_webview_window(MAIN_WINDOW_LABEL)
                .and_then(|main| main.outer_position().ok().map(|p| (p.x, p.y))),
            app.get_webview_window(MAIN_WINDOW_LABEL)
                .and_then(|main| main.inner_position().ok().map(|p| (p.x, p.y))),
            (
                CENTER_LAST_X.load(Ordering::SeqCst),
                CENTER_LAST_Y.load(Ordering::SeqCst),
            ),
            CENTER_HOLD_TOLERANCE,
        ) {
            return;
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
pub(crate) fn schedule_clamp(app: &AppHandle) {
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
pub(crate) fn schedule_center_snap(app: &AppHandle) {
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
pub(crate) fn clamp_main_window(app: &AppHandle, main: &tauri::WebviewWindow) {
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
    // 分辨率调小 / 拖去小屏后窗口可能比工作区还大：位置夹取只管「摆进来」，
    // 管不了「本身放不下」（WM 会把窗口裁剪在屏内，看起来就是「调了分辨率
    // 窗口一点没变」）。这里把尺寸也收进工作区 —— set_main_window_size 统一
    // 走夹取 + 落盘 + 重居中。正常拖动不会触发（窗口拖不出工作区外变大），
    // 只有跨屏拖到更小的屏、或运行中改了分辨率才进这条分支。
    if win_w > work.size.width as i32 || win_h > work.size.height as i32 {
        let settings = stored_settings(app);
        startup_debug(
            app,
            &format!(
                "clamp: client {}x{} 超出工作区 {}x{}，按记忆逻辑尺寸 {:?}x{:?} 收缩",
                win_w,
                win_h,
                work.size.width,
                work.size.height,
                settings.window_width,
                settings.window_height
            ),
        );
        if let Err(error) = crate::window_geometry::set_main_window_size(
            app,
            settings.window_width,
            settings.window_height,
        ) {
            trace(app, "autohide", &format!("clamp 收缩窗口尺寸失败: {error}"));
        }
        return; // set_size 自带重落位（recenter_after_window_size），别再抢
    }
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

pub(crate) fn apply_dock_taskbar_visibility(app: &AppHandle, hide: bool) -> Result<(), String> {
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

/// 鼠标离开主窗口后多久贴边隐藏（太灵敏会误触，太迟钝像卡住）
pub(crate) const AUTO_HIDE_DELAY_MS: u64 = 350;

/// 贴边守护线程的轮询周期：光标在不在窗口内、贴没贴边，都靠它扫
pub(crate) const AUTO_HIDE_TICK_MS: u64 = 120;

/// 主窗「还在动」的静默期：距最近一条主窗 Moved 不超过它时，watcher 一律不
/// 累计「离窗时钟」（也就不会贴边）。用户拖窗口、拖白条、WM 自己摆窗口都会
/// 连发 Moved，这期间读到的「光标在窗外」是**拖动过程的中间态**，不是
/// 「用户离开了」。实测（poke-1 复现台：贴边成条 → 抓住白条往工作区里拖）：
/// 松手瞬间窗口从条子展开成 460x900，**176ms 后** watcher 就按拖动途中的
/// 「光标在窗外」把它缩回条子 —— 主窗闪一下（用户报的「主界面会变长」）、
/// GRUB 独立窗 129ms 内翻两次（用户报的「乱闪乱跳」）。
pub(crate) const AUTO_HIDE_MOVE_QUIET_MS: u64 = 300;

/// 贴边后窗口缩成的「缝」的厚度（逻辑像素）：窗口只缩到这条缝，白条是
/// **独立渲染**的白色条（#dock-strip，长度/厚度/透明度全由前端 CSS 控制），
/// 缝要足够容纳白条最大厚度（40px），所以固定取 40 —— 窗口几何与白条
/// 参数彻底解耦，改白条参数不用动窗口。
pub(crate) const DOCK_STRIP_SLOT_PX: f64 = 40.0;
