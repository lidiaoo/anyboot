//! set_main_* 命令面

use super::*;

#[tauri::command]
pub(crate) fn set_main_auto_hide(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_always_on_top(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_auto_hide_edge_distance(app: AppHandle, distance: i32) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_dock_strip_thickness(app: AppHandle, value: i32) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_dock_strip_length(app: AppHandle, value: i32) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_dock_strip_opacity(app: AppHandle, value: i32) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_dock_strip_edge_gap(app: AppHandle, value: i32) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_dock_strip_hover_ms(app: AppHandle, value: u64) -> Result<SettingsView, String> {
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
pub(crate) fn normalize_hex_color(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("#{}", hex.to_ascii_lowercase()))
}

/// 贴边白条颜色：前端 CSS 渲染（--dock-strip-color），后端只存值。
#[tauri::command]
pub(crate) fn set_main_dock_strip_color(app: AppHandle, color: String) -> Result<SettingsView, String> {
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
pub(crate) fn state_auto_hide_docked(app: &AppHandle) -> bool {
    app.state::<Mutex<AppState>>()
        .lock()
        .map(|guard| guard.auto_hide_docked)
        .unwrap_or(false)
}

/// 贴边热区厚度（逻辑像素 1–100）：停靠态从工作区边往内这一段是鼠标移入
/// 触发呼出的热区。判定纯几何，与窗口槽位厚度无关；可视化由前端 CSS 渲染。
#[tauri::command]
pub(crate) fn set_main_hotzone_thickness(app: AppHandle, value: i32) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_hotzone_color(app: AppHandle, color: String) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_hotzone_opacity(app: AppHandle, value: i32) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_hotzone_border(app: AppHandle, enabled: bool) -> Result<SettingsView, String> {
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
pub(crate) fn set_main_hotzone_border_color(app: AppHandle, color: String) -> Result<SettingsView, String> {
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

#[tauri::command]
pub(crate) fn set_main_window_position(app: AppHandle, mode: String) -> Result<SettingsView, String> {
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

/// 前端白条渲染取证：停靠态下前端把 #dock-strip 的渲染矩形（CSS px）+
/// 视口尺寸 + DPR 报上来，与后端 `dock done` 的窗口物理矩形对拍 —— 白条
/// 的物理落点 = 窗口客户区原点 + rect×DPR，一眼算出「画在哪」。
#[tauri::command]
pub(crate) fn report_dock_strip_rect(
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
pub(crate) fn set_main_fixed_monitor(app: AppHandle, name: String) -> Result<SettingsView, String> {
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
