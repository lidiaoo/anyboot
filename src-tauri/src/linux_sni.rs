#![cfg(target_os = "linux")]

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
