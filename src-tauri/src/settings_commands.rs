//! settings 归一化 / 发布 / 更新命令面

use super::*;

/// 主题归一（纯函数）：不认识的值一律回落到 dark。
pub(crate) fn normalize_theme(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if THEMES.contains(&value.as_str()) {
        value
    } else {
        "dark".to_string()
    }
}

/// 主窗口固定位置模式归一（纯函数）：free / center / last，其余回落 free。
pub(crate) fn normalize_window_position(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "center" => "center",
        "last" => "last",
        _ => "free",
    }
}

/// 托盘菜单方式归一（纯函数）：不认识的值一律回 classic（出厂默认形态）。
pub(crate) fn normalize_tray_menu_style(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "unified" => "unified",
        _ => "classic",
    }
}

/// 固定显示器设置归一（纯函数）：空 / 空白 = auto（不固定）。
pub(crate) fn normalize_fixed_monitor(value: &str) -> String {
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
pub(crate) fn fixed_monitor_index(names: &[&str], fixed: &str) -> Option<usize> {
    let fixed = normalize_fixed_monitor(fixed);
    if fixed == "auto" {
        return None;
    }
    names.iter().position(|name| *name == fixed)
}

/// GRUB 区块展示方式归一（纯函数）：inline / wide / list / pane，
/// 其余一律回落 inline。老配置里的 `window`（独立窗口）已经演进成
/// 同一窗口里的「独立区域」，语义由 `pane` 承接 —— 老用户无感迁移。
pub(crate) fn normalize_grub_list_view(value: &str) -> &'static str {
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
pub(crate) fn normalize_grub_list_format(value: &str) -> &'static str {
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
pub(crate) fn grub_layout_width(mode: &str) -> f64 {
    if mode == "inline" {
        460.0
    } else {
        932.0
    }
}

/// 主题强调色：前端据此派生按钮/链接色（--accent 系列），后端只存值。
#[tauri::command]
pub(crate) fn set_accent_color(app: AppHandle, color: String) -> Result<SettingsView, String> {
    let color = normalize_hex_color(&color).ok_or_else(|| format!("颜色格式无效: {color}"))?;
    trace(&app, "command", &format!("set_accent_color color={color}"));
    let mut settings = stored_settings(&app);
    settings.accent_color = color;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 托盘菜单方式：classic（三端各自方式，默认）/ unified（positioner 全局统一）。
/// 落位在下次弹菜单时按新值走（tray_menu_present 每次现读设置）；菜单形态
/// （SNI 的 Menu 属性）由宿主在注册时缓存，GNOME 上切完重启应用才完全生效。
#[tauri::command]
pub(crate) fn set_tray_menu_style(app: AppHandle, mode: String) -> Result<SettingsView, String> {
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

/// URL 查询值的最小百分号编码：保留 RFC 3986 unreserved（字母数字 - _ . ~），
/// 其余逐字节 %XX —— selector 里的空格、中文、括号都能安全进 query。
pub(crate) fn percent_encode_component(value: &str) -> String {
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

pub(crate) fn http_access_hint_text(app: &AppHandle) -> String {
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
pub(crate) fn web_page_url(host: &str, port: u16, token: &str) -> String {
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
pub(crate) fn current_web_page_url(app: &AppHandle) -> Result<String, String> {
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
pub(crate) fn open_web_page_now(app: &AppHandle) -> Result<String, String> {
    let url = current_web_page_url(app)?;
    open_url_in_browser(&url)?;
    Ok(url)
}

/// 设置类变更的统一写法：存盘 → 记录回执 → 必要时重启服务 → 发布快照。
pub(crate) fn update_settings_and_publish(
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
pub(crate) fn update_settings_and_restart_http(
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

pub(crate) fn set_http_enabled_setting(app: &AppHandle, enabled: bool) {
    let _ = update_settings_and_publish(
        app,
        "HTTP 服务",
        if enabled { "已开启" } else { "已关闭" },
        |settings| settings.http_enabled = enabled,
        apply_http_config,
    );
}

pub(crate) fn set_notify_enabled_setting(app: &AppHandle, enabled: bool) {
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

pub(crate) fn set_delay_all_setting(app: &AppHandle, seconds: u32) {
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
pub(crate) fn delay_submenu_title(settings: &Settings) -> String {
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

pub(crate) fn view_settings(app: &AppHandle) -> SettingsView {
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
pub(crate) fn publish_settings(app: &AppHandle) -> SettingsView {
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

/// 托盘里能直接切的勾选设置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TraySetting {
    StartInTray,
    HideFromDockTaskbar,
}

impl TraySetting {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::StartInTray => "启动时隐藏主界面",
            Self::HideFromDockTaskbar => "隐藏程序（任务栏/Dock）",
        }
    }
}

/// 托盘切设置。**失败必须回滚**（阶段 1 修 D5）：
/// 以前是先存盘再 apply，apply 失败时设置文件里已经是新值，
/// 界面勾选框显示「已生效」而系统实际没变——界面在说假话。
pub(crate) fn update_tray_setting(app: &AppHandle, setting: TraySetting, enabled: bool) {
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

pub(crate) fn update_close_action_setting(app: &AppHandle, close_action: CloseAction) {
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
