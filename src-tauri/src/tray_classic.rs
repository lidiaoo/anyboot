//! classic 原生托盘菜单 + 托盘位图

use super::*;

pub(crate) fn create_tray_image() -> Image<'static> {
    tauri::include_image!("icons/icon.png")
}

/// 绿勾位图（classic 原生菜单的列表类条目用）。
///
/// 尺寸固定 16×16：muda 在 Windows 上就是把图标按 16×16 画进位图列的。
/// Linux 已不走 tauri 托盘（classic 也由 linux_sni 承载），只剩 Win/mac 用；
/// 单测仍跑，故 allow 而非 cfg 掉。
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn check_mark_image() -> Image<'static> {
    use std::sync::OnceLock;
    static IMAGE: OnceLock<Image<'static>> = OnceLock::new();
    IMAGE.get_or_init(build_check_mark_image).clone()
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn build_check_mark_image() -> Image<'static> {
    Image::new_owned(check_mark_rgba(), CHECK_MARK_SIZE, CHECK_MARK_SIZE)
}

/// 绿勾的像素（纯函数，便于单测；tauri 的 Image 借不出来，所以拆开）。
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn check_mark_rgba() -> Vec<u8> {
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
pub(crate) fn distance_to_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
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

/// 托盘菜单（原生形态）——**手写构建**（昨天中午前验证过的原始实现）。
///
/// 只负责构建菜单，不碰托盘图标。setup 的第一帧和后续重建走同一份实现。
/// 注意：不要换回「MenuNode 模型 → native_menu 转换层」——转换层在 GNOME
/// 宿主渲染下子菜单展开为空（GRUB 启动项/固定显示器等），手写版没有这个问题。
/// Linux 已不走 tauri 托盘（classic 也由 linux_sni 承载），本函数只剩 Win/mac。
#[cfg(all(desktop, not(target_os = "linux")))]
pub(crate) fn build_tray_menu(app: &AppHandle, state: &UiState) -> tauri::Result<Menu<tauri::Wry>> {
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
    // 远程管理（server-hub）独立窗口：classic 形态的原生菜单是**手写**的另一套
    // 实现（不走 grub_menu_nodes），入口要单独补 —— 否则用 classic 时托盘上
    // 根本没有打开远程管理的入口。动作 id 与自绘菜单保持一致（open-hub）。
    menu.append(&MenuItem::with_id(
        app,
        "open-hub",
        "远程管理…",
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
pub(crate) fn on_tray_right_click(app: &AppHandle, cursor: tauri::PhysicalPosition<f64>) {
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
pub(crate) fn popup_tray_menu(app: &AppHandle, cursor: tauri::PhysicalPosition<f64>) {
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

/// 把最新状态推给托盘（classic 原生菜单 / unified 自绘窗+dbusmenu 两种形态）。
pub(crate) fn update_tray_menu(app: &AppHandle, state: &UiState, trigger: &str) -> tauri::Result<()> {
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
