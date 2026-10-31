//! 窗口尺寸 / 居中 / 固定显示器。默认值常量钉在方案定则（90% 高度等）。

use super::*;

/// 默认窗口高度 = **整屏**高度的 2/3（纯函数，便于单测）。
///
/// 用户定案 2026-10-13：由「占屏幕一半」改为「占屏幕高度的 2/3」，且基准是
/// **整屏**（monitor.size）不是工作区 —— 基准用工作区会在有任务栏的机器上
/// 少一截（1920x1080 + 底栏 → 652 而不是 720），窗口「已经在重置目标上」，
/// 重置按钮点起来就像失灵（实测踩过）。高度超工作区没关系：居中落位走
/// clamp_to_work，窗口仍会被摆进工作区。夹回合法区间 —— 1080p 屏上 2/3 是
/// 720；特别矮的屏（如 800）算出 533 会落到最小高度 560。
pub(crate) fn default_window_height_from_screen(screen_height: u32) -> u32 {
    (screen_height * 2 / 3).clamp(WINDOW_MIN_SIZE.1, WINDOW_MAX_SIZE.1)
}

/// 当前显示器的默认窗口高度：**整屏**高度（逻辑像素）的 2/3。
///
/// 窗口还没映射时 `current_monitor()` 可能给不出，退到主显示器；两个都拿不到
/// 才回落到兜底常量。基准用整屏（monitor.size）而非工作区 —— 见
/// `default_window_height_from_screen` 的注释（任务栏那条交给落位去夹，
/// 不从高度里扣）。
pub(crate) fn default_window_height(app: &AppHandle) -> u32 {
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
pub(crate) fn clamp_window_size(width: u32, height: u32) -> (u32, u32) {
    (
        width.clamp(WINDOW_MIN_SIZE.0, WINDOW_MAX_SIZE.0),
        height.clamp(WINDOW_MIN_SIZE.1, WINDOW_MAX_SIZE.1),
    )
}

/// 把目标逻辑尺寸夹进「窗口当前所在显示器」的工作区（逻辑 px）。
///
/// 分辨率调低 / 缩放调大 / 拖去小屏之后，记忆尺寸可能比新工作区还大 ——
/// 位置夹取（clamp_to_work）只能保证窗口不越界，管不了「窗口本身放不下」
/// （那时整个窗口被钉在工作区左上角，底下大片悬在屏外，用户看到的就是
/// 「调了分辨率窗口一点没变」）。所有落尺寸的入口都汇在
/// `set_main_window_size`，在这里统一夹进当前屏幕。拿不到显示器就原样
/// 返回（与旧行为一致，别让夹取变成新的失败点）。
fn fit_to_current_work_area(app: &AppHandle, width: u32, height: u32) -> (u32, u32) {
    let Some(monitor) = main_window_monitor(app) else {
        return (width, height);
    };
    fit_size_to_work_area(
        width,
        height,
        monitor.scale_factor(),
        monitor.work_area().size.width,
        monitor.work_area().size.height,
    )
}

/// 纯函数版（便于单测）：物理工作区 ÷ 缩放 = 逻辑工作区，目标尺寸逐边取 min，
/// 再过一遍 clamp_window_size 保证不跌破最小尺寸。缩放 ≤ 0 / 工作区为 0
/// 都视为「拿不到屏幕」，原样返回。特别矮的屏上最小高度 560 仍可能超工作区
/// —— 与 `default_window_height_from_screen` 同一取舍：完整性交给落位去夹。
pub(crate) fn fit_size_to_work_area(
    width: u32,
    height: u32,
    scale: f64,
    work_w_px: u32,
    work_h_px: u32,
) -> (u32, u32) {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    if work_w_px == 0 || work_h_px == 0 {
        return (width, height);
    }
    let work_w = (work_w_px as f64 / scale).floor() as u32;
    let work_h = (work_h_px as f64 / scale).floor() as u32;
    clamp_window_size(width.min(work_w), height.min(work_h))
}

/// 落一个新窗口尺寸：夹取 → 记忆 → 应用。锁定状态下要先把 max 放开再落尺寸
/// （max_size 对 set_size 也生效，不放开 932 落不进一个 max=460 的窗口），
/// 落完再按新值收紧。所有改窗口尺寸的入口（滑条 / 切展示方式 / 重置）都走这里。
/// 居中的尺寸基准一律取「设置值 × 显示器缩放」（见 center_main_window），
/// 不读窗口实况 —— 2x 屏上未映射/过渡期的读数是逻辑值，会算错落点。
pub(crate) fn set_main_window_size(app: &AppHandle, width: u32, height: u32) -> Result<(), String> {
    let (width, height) = clamp_window_size(width, height);
    // 分辨率/缩放调整后记忆尺寸可能放不进新屏：落尺寸前先夹进当前工作区。
    let (width, height) = fit_to_current_work_area(app, width, height);
    let mut settings = stored_settings(app);
    settings.window_width = width;
    settings.window_height = height;
    save_settings(app, &settings)?;
    let main = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "主窗口不存在".to_string())?;
    // **同值跳过**：GTK 上 set_size 即使值没变也会发 configure → 前端 Resized
    // 回声 → 「Resized→500ms 回写 set_window_size」再喂回来 = 自激环
    // （Linux 4K 屏实测 2026-10-31：每 ~530ms set_size + 重居中一轮，窗口
    // 不停闪、页面不停回流、滚动位置被打回顶部；Windows 同值不发 WM_SIZE，
    // 所以那边测不出来）。±2px 容差罩住物理/逻辑换算的舍入差；设置照常
    // 落盘，只是不再动窗口 —— 环在源头断掉。
    {
        let scale = main.scale_factor().unwrap_or(1.0);
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let want_w = (width as f64 * scale).round();
        let want_h = (height as f64 * scale).round();
        if let Ok(current) = main.inner_size() {
            if (current.width as f64 - want_w).abs() <= 2.0
                && (current.height as f64 - want_h).abs() <= 2.0
            {
                return Ok(());
            }
        }
    }
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
pub(crate) fn recenter_after_window_size(app: &AppHandle, width: u32, height: u32) {
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
pub(crate) fn apply_window_size_lock(app: &AppHandle) {
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
pub(crate) fn apply_grub_layout_size(app: &AppHandle, force_nominal_width: bool) {
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
pub(crate) fn grub_layout_target_width(nominal: u32, remembered: u32, force: bool) -> u32 {
    if force || remembered == 0 {
        nominal
    } else {
        remembered
    }
}

/// 「引导高级设置 → 布局」的窗口落宽（2026-10-30 用户定案：加宽 / 左右布局
/// 抄袭原 GRUB 独立区域的窗口加宽能力，展示方式下拉随之移除）。
/// · mode = "wide"（加宽）| "lr"（左右布局）= 窗口至少 932 逻辑宽
///   （两栏标称 460 + 12 + 460；已经 ≥932 就不动 —— 用户手动拉得更宽的尊重
///   现值）；高度用记忆值，没记过按默认屏高 2/3。锁定窗口大小不拦这条
///   （锁定只禁手拖缩放，明确落宽操作放行，用户定案 2026-10-13）。
/// · mode = "default"（切回默认）= 回内嵌标称宽 **460**。这里**不再问已废弃的
///   `grub_list_view`** —— 老配置里可能还存着 "wide"（旧「加宽窗口展示」），
///   照它落宽会让「默认」也变成 932，用户看到的就是「点了默认窗口不缩回正常
///   宽度」。展示方式下拉已移除，布局的唯一事实源是前端 advState.layout。
#[tauri::command]
pub(crate) fn set_boot_layout(app: AppHandle, mode: String) -> Result<(), String> {
    trace(&app, "command", &format!("set_boot_layout mode={mode}"));
    if mode != "wide" && mode != "lr" {
        let settings = stored_settings(&app);
        let height = if settings.window_height == 0 {
            // 还没记过尺寸（新装 / 清过配置）：默认占整屏高度的 2/3
            default_window_height(&app)
        } else {
            settings.window_height
        };
        let width = crate::settings_commands::grub_layout_width("inline") as u32;
        return set_main_window_size(&app, width, height);
    }
    let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return Ok(());
    };
    let target_width = crate::settings_commands::grub_layout_width("wide") as u32;
    // inner_size 是物理像素，除以缩放才是逻辑宽（与滑条回写同一套换算）
    let scale = main.scale_factor().unwrap_or(1.0);
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let logical_width = main
        .inner_size()
        .map(|size| size.width as f64 / scale)
        .unwrap_or(0.0);
    if logical_width + 0.5 >= target_width as f64 {
        return Ok(());
    }
    let settings = stored_settings(&app);
    let height = if settings.window_height == 0 {
        default_window_height(&app)
    } else {
        settings.window_height
    };
    set_main_window_size(&app, target_width, height)
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
pub(crate) fn main_window_monitor(app: &AppHandle) -> Option<tauri::Monitor> {
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

pub(crate) fn center_main_window(app: &AppHandle) -> Option<(i32, i32)> {
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

/// 停靠态居中的目标**客户区**落点（纯函数，物理像素）：外框（客户区+标题栏）
/// 在工作区居中 —— 与非停靠路径的外框基准同观感；重锚的收敛判据是
/// 「客户区实况 == target」，所以 y 要加回标题栏。
pub(crate) fn docked_center_target(
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

/// 按固定位置模式落位主窗口（贴边停靠态不干预）。center=居中；last=落到
/// 记忆的位置（无记忆则居中）；free=不管。
pub(crate) fn apply_main_window_position(app: &AppHandle) -> Option<(i32, i32)> {
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
pub(crate) fn enforce_fixed_monitor(app: &AppHandle) {
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
pub(crate) fn clamp_opacity(percent: u32) -> u8 {
    percent.clamp(MIN_OPACITY as u32, MAX_OPACITY as u32) as u8
}

/// 设置窗口尺寸（滑条拖动 / 用户拖窗口边框都走这里，双向实时）。
#[tauri::command]
pub(crate) fn set_window_size(app: AppHandle, width: u32, height: u32) -> Result<SettingsView, String> {
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
pub(crate) fn set_window_size_locked(app: AppHandle, locked: bool) -> Result<SettingsView, String> {
    trace(&app, "command", &format!("set_window_size_locked {locked}"));
    let mut settings = stored_settings(&app);
    settings.window_size_locked = locked;
    save_settings(&app, &settings)?;
    apply_window_size_lock(&app);
    Ok(publish_settings(&app))
}

/// 重置窗口大小到当前展示方式的标称尺寸（托盘菜单与设置页共用）。
/// 宽随模式（460 ⇄ 932）、高回到「整屏高度的 2/3」（默认高度）。
pub(crate) fn reset_window_size_inner(app: &AppHandle) -> Result<SettingsView, String> {
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
pub(crate) fn kick_webview_repaint(app: &AppHandle) {
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
pub(crate) fn kick_webview_repaint(_app: &AppHandle) {}

#[tauri::command]
pub(crate) fn reset_window_size(app: AppHandle) -> Result<SettingsView, String> {
    trace(&app, "command", "reset_window_size");
    reset_window_size_inner(&app)
}

#[tauri::command]
pub(crate) fn reset_main_window_position(app: AppHandle) -> Result<SettingsView, String> {
    // blocking-ok: kick_webview_repaint 的 thread::sleep 在 thread::spawn 子线程闭包里，
    // 本命令在 UI 线程上不等它，不会阻塞（blocking_guard 逐字节扫描不看线程边界，属误报）。
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

/// 窗口尺寸的合法范围（逻辑像素）：下限是前端 CSS 的 min-width + 内容最低可用，
/// 上限防手滑把窗口拖到比屏幕还大。滑条范围与它一致。
pub(crate) const WINDOW_MIN_SIZE: (u32, u32) = (360, 560);

pub(crate) const WINDOW_MAX_SIZE: (u32, u32) = (1600, 1200);

/// 拿不到显示器时的兜底高度；正常路径是「整屏高度的 2/3」（见 default_window_height）。
pub(crate) const DEFAULT_WINDOW_HEIGHT: u32 = 900;

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
pub(crate) fn center_docked_window(app: &AppHandle, main: &tauri::WebviewWindow) -> Option<(i32, i32)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_size_shrinks_into_the_logical_work_area() {
        // 1x 屏 1920x1080：宽 1600 超 960 → 收到 960，高 720 放得下原样保留
        // （别整对齐收缩，哪边超收哪边）
        assert_eq!(fit_size_to_work_area(1600, 720, 1.0, 960, 1040), (960, 720));
        // 2x 屏工作区 1920x1040 物理 = 960x520 逻辑：高 900 收进 520 —— 但
        // 低于最小高度 560，clamp_window_size 兜底回 560（与
        // default_window_height_from_screen 同一取舍：完整性交给落位去夹）
        assert_eq!(fit_size_to_work_area(932, 900, 2.0, 1920, 1040), (932, 560));
    }

    #[test]
    fn fit_size_keeps_sizes_that_already_fit() {
        // 1x 屏 1920x1080：932x720 原样保留（别动用户记住的尺寸）
        assert_eq!(fit_size_to_work_area(932, 720, 1.0, 1920, 1040), (932, 720));
    }

    #[test]
    fn fit_size_never_drops_below_the_minimum() {
        // 特别矮的屏（800x500 逻辑工作区）：宽收到 800（仍 ≥ 最小宽），
        // 高跌破最小高度 560 → 兜底回 560，与 default_window_height_from_screen 同取舍
        let (w, h) = fit_size_to_work_area(932, 900, 1.0, 800, 500);
        assert_eq!((w, h), (800, WINDOW_MIN_SIZE.1));
    }
    #[test]
    fn fit_size_passes_through_when_screen_is_unknown() {
        // 拿不到显示器 / 缩放异常：原样返回，别让夹取变成新的失败点
        assert_eq!(fit_size_to_work_area(932, 900, 0.0, 1920, 1040), (932, 900));
        assert_eq!(fit_size_to_work_area(932, 900, 1.0, 0, 0), (932, 900));
    }
}
