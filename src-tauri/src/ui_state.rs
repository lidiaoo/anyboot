//! UI 快照发布（UI_STATE_CHANGED_EVENT）+ WebSocket 广播（WS_CLIENTS）。

use super::*;

/// 生成当前状态的快照（不改任何东西）。
pub(crate) fn ui_state(app: &AppHandle) -> UiState {
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

/// **唯一**的发布入口：先取号（fetch_add 原子地拿走自己的 revision），再用同一份
/// 快照重建托盘菜单 + 通知主窗口。
///
/// 主窗口和托盘菜单从此不存在「各读各的」，也不需要三个补丁式事件。
///
/// 为什么用 fetch_add 的**返回值**而不是发布时再 load：
/// 两个线程（启动扫描线程 + 主窗 refresh 命令）并发 publish 时，A add 完、B 又 add，
/// A 再 load 会读到 B 的号 —— 两个**内容不同**的快照带同一个 revision。
/// 前端按 revision 去重，后到的内容可能被当「过期」丢掉（主窗卡在「正在扫描…」
/// 的实测根因之一）。fetch_add 返回的号是唯一归属，永不重复。
pub(crate) fn publish_ui(app: &AppHandle) -> UiState {
    let revision = UI_REVISION.fetch_add(1, Ordering::SeqCst) + 1;
    let mut state = ui_state(app);
    state.revision = revision;
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

/// 已连上的推送客户端：id → 发送端。
///
/// 为什么中间隔一条通道、而不是直接把 socket 存在这儿：连接由 http 层的线程抱着
/// （它的生命周期、读写、关闭都在那边），两边只交接「快照字符串」。发送端一 drop，
/// 对面 `recv_timeout` 立刻拿到 `Disconnected` 收尾退出 —— 收服务、换 token 时就
/// 靠这个把它们请走（见 http::RunningServer::stop）。
pub(crate) static WS_CLIENTS: Mutex<Vec<(u64, SyncSender<String>)>> = Mutex::new(Vec::new());

/// 推送客户端的发号器，注销时靠它找到自己那一条。
pub(crate) static WS_NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// 当前快照的 JSON 串（`/api/ui-state`、WS 首帧、WS 广播共用同一份）。
pub(crate) fn ui_state_json(app: &AppHandle) -> String {
    serde_json::to_string(&get_ui_state(app.clone())).unwrap_or_else(|_| "{}".to_string())
}

/// 当前推送客户端数量（只有单测要用；正常路径靠 `websocket_publish` 里的
/// `is_empty()` 短路，不必单独查一次）。
#[cfg(test)]
pub(crate) fn websocket_client_count() -> usize {
    WS_CLIENTS.lock().map(|clients| clients.len()).unwrap_or(0)
}

/// 把新快照推给所有客户端。**没人订阅时连序列化都不做**。
pub(crate) fn websocket_publish<T: Serialize>(state: &T) {
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

/// 主窗口首次加载用：拿一份完整快照（含 revision）。
///
/// 这里读的是 UI_REVISION 的**当前值**（不加号）：它可能落后于最后一次 publish
/// 刚发的快照，但绝不可能超前 —— 前端按 revision 去重，「等于上一拍」的快照
/// 会被丢，所以轮询兜底拿到与事件同一拍的快照时不会重复渲染。
#[tauri::command]
pub(crate) fn get_ui_state(app: AppHandle) -> UiState {
    trace(&app, "command", "get_ui_state（主窗口）");
    ui_state(&app)
}
