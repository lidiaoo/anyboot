//! 自绘托盘菜单窗 runtime（unified 形态两窗共用）。

use super::*;

/// **自绘**托盘菜单的窗口标签。
///
/// 为什么托盘菜单要由我们自己画一扇窗：Linux 上菜单原本由宿主按我们导出的 dbusmenu
/// 重新渲染（GtkMenu），点任何一项它都会把菜单收掉 —— 而「点选择框不关菜单」是产品
/// 要求，dbusmenu 协议里又没有任何开关能改这个行为（2026-10-14 实测）。Windows 的
/// TrackPopupMenu / macOS 的 NSMenu 同理。所以菜单生命周期只能归我们所有。
pub(crate) const TRAY_MENU_WINDOW_LABEL: &str = "tray-menu";

/// 菜单宽度（逻辑像素）：固定宽度，三端长得一样，也只有这样「一组里恰好一个 ◉」才好扫。
pub(crate) const TRAY_MENU_WIDTH: f64 = 320.0;

/// 前端报上来的高度夹到这个区间（太低会裁掉文字，太高会把托盘图标顶出屏幕）。
pub(crate) const TRAY_MENU_MIN_HEIGHT: f64 = 40.0;

pub(crate) const TRAY_MENU_MAX_HEIGHT: f64 = 720.0;

/// 前端量不出内容高度时的标称高度（正常路径下前端总会量出来，这只是兜底）
pub(crate) const TRAY_MENU_NOMINAL_HEIGHT: f64 = 420.0;

/// 菜单与托盘图标之间的缝（逻辑像素）。两种方式的手算落位都用
/// （unified 曾交给 positioner 的 TrayCenter，2026-10-15 起两种都自己算坐标）。
pub(crate) const TRAY_MENU_GAP: f64 = 6.0;

/// 刚弹出来这一小段时间里收到的「失焦」是假的（WM 还没把焦点交过来就报了一次），
/// 滤掉它，免得菜单一闪就没。
pub(crate) const TRAY_MENU_BLUR_GRACE_MS: u64 = 250;

/// 自绘托盘菜单的运行时状态。
#[derive(Default)]
pub(crate) struct TrayMenuRuntime {
    /// 本次打开的锚点 = **托盘图标的原点**（物理像素）。
    /// Linux 上来自 SNI 的 `Activate(x, y)` —— 实测它给的是图标原点而不是点击点
    /// （点 1776 收到 1742），正好拿来给菜单定位。
    pub(crate) anchor: Option<(f64, f64)>,
    /// 已经呈现出去的高度（逻辑像素）
    pub(crate) presented: Option<f64>,
    /// 本次弹出的时刻（见 `TRAY_MENU_BLUR_GRACE_MS`）
    pub(crate) opened_at: Option<Instant>,
    /// 每次「开菜单」自增，随内容一起发给前端 —— 前端据此知道「这是新开的一次」，
    /// 哪怕高度与上次一样也要重新呈现（不然重开菜单时窗口永远不显示）
    pub(crate) seq: u64,
    /// 弹出后是否**真的**拿到过焦点（失焦看门狗用，见 `spawn_tray_menu_watchdog`）：
    /// 拿到过又没了 = 真失焦，但事件可能被假失焦宽限吞掉或干脆没送达。
    pub(crate) focus_seen: bool,
    /// 连续无焦点的起点（看门狗去抖：稳定无焦点才收，别跟点击本窗抢焦点的竞态打架）
    pub(crate) unfocused_since: Option<Instant>,
    /// 「始终没拿到焦点」时是否已重试过一次 set_focus
    pub(crate) focus_retried: bool,
}

/// 摸一下菜单运行时状态。
///
/// 不返回 guard：`app.state::<T>()` 给的是一个临时值，`.lock()` 借它，出了函数就没了
/// （E0515）。所以这里收一个闭包代替返回 guard —— 与项目里其它几处 `state.lock()` 同款做法。
pub(crate) fn with_tray_menu_runtime<R>(app: &AppHandle, use_it: impl FnOnce(&mut TrayMenuRuntime) -> R) -> R {
    let state = app.state::<Mutex<TrayMenuRuntime>>();
    let mut guard = state.lock().expect("tray menu mutex poisoned");
    use_it(&mut guard)
}

/// 发给前端的一整份菜单内容。
#[derive(Clone, Serialize)]
pub(crate) struct TrayMenuPayload {
    /// light / dark（与主界面同一套取值）
    pub(crate) theme: String,
    /// 本次「开菜单」的序号（见 `TrayMenuRuntime::seq`）
    pub(crate) seq: u64,
    pub(crate) nodes: Vec<MenuNode>,
}

/// 前端报上来的高度夹到区间里（纯函数，便于单测）：量不出来就用标称高度兜底。
pub(crate) fn tray_menu_height(raw: Option<f64>) -> f64 {
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
pub(crate) fn tray_menu_max_height(app: &AppHandle) -> Option<f64> {
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
pub(crate) fn clamp_tray_menu_height(height: f64, max: Option<f64>) -> f64 {
    match max {
        Some(max) if max.is_finite() && max > 0.0 => height.min(max),
        _ => height,
    }
}

/// 打开托盘菜单：记下锚点、把最新内容推给菜单窗（窗口不在就现建一个，建完前端会来拉）。
///
/// `x` / `y` 是**托盘图标的原点**（物理像素）。
pub(crate) fn open_tray_menu(app: &AppHandle, x: f64, y: f64) {
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
pub(crate) fn build_tray_menu_window(app: &AppHandle) -> tauri::Result<()> {
    apply_browser_args(tauri::WebviewWindowBuilder::new(
        app,
        TRAY_MENU_WINDOW_LABEL,
        tauri::WebviewUrl::App("tray-menu.html".into()),
    ))
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
pub(crate) fn push_tray_menu_nodes(app: &AppHandle, window: &tauri::WebviewWindow) {
    let state = ui_state(app);
    emit_tray_menu_nodes(app, window, &state);
}

/// 同上，但用调用方手里那份快照（`update_tray_menu` 已经有一份，不必再取一次）。
pub(crate) fn emit_tray_menu_nodes(app: &AppHandle, window: &tauri::WebviewWindow, state: &UiState) {
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
pub(crate) fn close_tray_menu(app: &AppHandle) {
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
pub(crate) fn tray_menu_blur_is_fake(app: &AppHandle) -> bool {
    with_tray_menu_runtime(app, |runtime| match runtime.opened_at {
        Some(at) => at.elapsed() < Duration::from_millis(TRAY_MENU_BLUR_GRACE_MS),
        None => false,
    })
}

/// 托盘图标尺寸拿不到实值时的标称（物理像素）：菜单落位用它估「图标下沿/上沿」，
/// 与 `feed_positioner` 喂给 positioner 的标称矩形同一套近似。
pub(crate) const TRAY_MENU_ICON_NOMINAL: f64 = 32.0;

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
pub(crate) fn spawn_tray_menu_watchdog(app: AppHandle) {
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
pub(crate) fn tray_menu_style_is_unified(app: &AppHandle) -> bool {
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
pub(crate) fn place_tray_menu(
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
pub(crate) fn tray_menu_present(app: AppHandle, height: Option<f64>) -> Result<(), String> {
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
pub(crate) fn tray_menu_click(app: AppHandle, id: String) {
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
pub(crate) fn tray_menu_close(app: AppHandle) {
    close_tray_menu(&app);
}

/// 前端加载完（或菜单窗刚建出来）主动拉一次：拿当前内容与主题。
#[tauri::command]
pub(crate) fn tray_menu_nodes(app: AppHandle) -> TrayMenuPayload {
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
