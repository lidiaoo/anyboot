//! 远程命令路由、延迟确认（pending 四件套）与 Web 面板静态资源。

use super::*;

pub(crate) fn record_request_log(
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
pub(crate) fn push_recent(requests: &mut Vec<RequestLogEntry>, entry: RequestLogEntry) {
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
    pub(crate) fn param(&self, key: &str) -> Option<String> {
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
    pub(crate) fn ok(status: u16, json: serde_json::Value) -> Self {
        Self {
            status,
            json,
            raw: None,
        }
    }

    pub(crate) fn error(status: u16, code: &str, message: &str) -> Self {
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
    pub(crate) fn raw_page(status: u16, content_type: &'static str, bytes: Vec<u8>) -> Self {
        Self {
            status,
            json: serde_json::Value::Null,
            raw: Some((content_type, bytes)),
        }
    }
}

/// 渠道判定：`channel=cli` 只在请求确实来自 loopback 时生效，
/// 远端就算伪造这个参数也一律按 http 渠道的配置执行（plan §4.7）。
pub(crate) fn remote_channel(request: &RemoteRequest) -> Channel {
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
pub(crate) fn route_remote_command(request: &RemoteRequest) -> Result<RemoteCommand, RemoteReply> {
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
            // wake 特殊处理：这条 HTTP 能送达就说明机器本来就醒着，什么都不用做
            // （给「唤醒路由勾了 HTTP 兜底」一条说得通的回执，而不是 400）
            if request
                .param("action")
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| value.eq_ignore_ascii_case("wake"))
            {
                return Ok(RemoteCommand::WakeAck);
            }
            let action = match request.param("action").as_deref().map(str::trim) {
                None | Some("") => PowerAction::default(),
                Some(value) => PowerAction::parse(value).ok_or_else(|| {
                    RemoteReply::error(
                        400,
                        "unknown_action",
                        &format!("未知的电源动作: {value}（可用 wake/reboot/shutdown/sleep）"),
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

pub(crate) fn remote_status_json(app: &AppHandle) -> serde_json::Value {
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

pub(crate) fn remote_requests_json() -> serde_json::Value {
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
pub(crate) fn remote_grub_json(app: &AppHandle) -> serde_json::Value {
    let settings = stored_settings(app);
    let view = grub_snapshot(app, &settings);
    serde_json::json!({
        "ok": true,
        "partuuid": view.partuuid,
        "efiDir": view.efi_dir,
        "envExists": view.env_exists,
        "installed": view.installed,
        "syncedAt": view.synced_at,
        // 这台机器自己的「GRUB 列表格式」：电脑列表据此渲染条目文字
        // （与它本机 GRUB 列表页看到的完全一致）
        "listFormat": settings.grub_list_format,
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

/// 网页控制台静态资源路由。
///
/// 页面本身**不要求 token**：浏览器得先拿到页面才能让用户输入 token；
/// 真正的数据与命令全部在 /api/* 后面，那里全程 token 鉴权。页面里没有任何
/// 机密（token 只在设置快照接口返回，且那需要 token 本身）。
pub(crate) fn static_ui_reply(path: &str) -> Option<RemoteReply> {
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
pub(crate) fn handle_ui_command_request(app: &AppHandle, request: &RemoteRequest) -> RemoteReply {
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
            // SOL 面板里要弹**被控机**对话框 / 终端 / 授权框的一批：
            // 远程网页点了也只会在这台电脑上弹，操作的人根本看不见。
            | "sol_pick_dir"
            | "sol_pick_archive"
            | "sol_open_dir"
            | "sol_project_open"
            | "sol_status_privileged"
            | "sol_one_click_install_terminal"
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
pub(crate) fn ui_command_requires_local_auth(cmd: &str) -> bool {
    cmd == "switch_system"
}

/// 哪些网页 / 手机命令**只能在本机点**（远程调用直接拒绝，而不是转成待确认）。
///
/// 目前只有「立即执行」：授权窗的意义就是让本机用户点头，远程端要是能自己确认，
/// 就等于自己给自己盖章、弹窗成了摆设。「取消」（撤回自己刚发的那条请求）是安全
/// 方向的操作，远程端保留 —— 随时能反悔也是授权模型该有的样子。
pub(crate) fn ui_command_is_local_only(cmd: &str) -> bool {
    cmd == "confirm_pending"
}

pub(crate) fn ui_str(args: &serde_json::Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("缺少或非法参数 {key}（应为字符串）"))
}

pub(crate) fn ui_bool(args: &serde_json::Value, key: &str) -> Result<bool, String> {
    args.get(key)
        .and_then(|value| value.as_bool())
        .ok_or_else(|| format!("缺少或非法参数 {key}（应为布尔）"))
}

pub(crate) fn ui_u32(args: &serde_json::Value, key: &str) -> Result<u32, String> {
    args.get(key)
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("缺少或非法参数 {key}（应为非负整数）"))
}

pub(crate) fn ui_i32(args: &serde_json::Value, key: &str) -> Result<i32, String> {
    args.get(key)
        .and_then(|value| value.as_i64())
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| format!("缺少或非法参数 {key}（应为整数）"))
}

/// 可缺省的字符串参数（JSON null / 缺字段 → None）。
pub(crate) fn ui_opt_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

pub(crate) fn status_value(status: AppStatus) -> Result<serde_json::Value, String> {
    serde_json::to_value(status).map_err(|error| format!("序列化失败: {error}"))
}

pub(crate) fn settings_value(view: Result<SettingsView, String>) -> Result<serde_json::Value, String> {
    let view = view?;
    serde_json::to_value(view).map_err(|error| format!("序列化失败: {error}"))
}

pub(crate) fn res_unit(result: Result<(), String>) -> Result<serde_json::Value, String> {
    result?;
    Ok(serde_json::Value::Null)
}

/// 网页控制台命令 → 同进程命令函数的白名单分发。
pub(crate) fn remote_ui_command(
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

pub(crate) fn remote_ui_command_inner(
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
            detect_esp_blocking(app.clone(), path).map(serde_json::Value::String)
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
        "sync_grub" => res_unit(sync_grub_blocking(app.clone())),
        "install_grub_script" => res_unit(install_grub_script_blocking(app.clone())),
        "uninstall_grub_script" => res_unit(uninstall_grub_script_blocking(app.clone())),
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
        // ---- SOL 面板（独立服务，不进 UI 快照；网页端按需拉取 / 直调）----
        // sol 那批命令多数是 async fn（内部 spawn_blocking），这里在 tiny_http 的
        // std 线程上 block_on 同步等结果 —— 长任务（下载/安装）会占住一个 worker，
        // 与 detect_esp / sync_grub_blocking 同一模式，可接受。
        "sol_status" => serde_json::to_value(tauri::async_runtime::block_on(sol_panel::sol_status(
            app.clone(),
        )))
        .map_err(|error| format!("序列化失败: {error}")),
        "sol_logs" => Ok(serde_json::Value::Array(
            sol_panel::sol_logs(app.clone())
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        )),
        "sol_config_read" => {
            let config = tauri::async_runtime::block_on(sol_panel::sol_config_read(ui_opt_str(
                args, "path",
            )))?;
            serde_json::to_value(config).map_err(|error| format!("序列化失败: {error}"))
        }
        "sol_config_write" => {
            let path = ui_str(args, "path")?;
            let content = ui_str(args, "content")?;
            tauri::async_runtime::block_on(sol_panel::sol_config_write(path, content))
                .map(serde_json::Value::String)
        }
        "sol_http_get" => {
            let http = sol_panel::sol_http_get(app.clone(), ui_opt_str(args, "path"))?;
            serde_json::to_value(http).map_err(|error| format!("序列化失败: {error}"))
        }
        "sol_http_set" => {
            let path = ui_opt_str(args, "path");
            let option = ui_str(args, "option")?;
            let value = ui_str(args, "value")?;
            tauri::async_runtime::block_on(sol_panel::sol_http_set(app.clone(), path, option, value))
                .map(serde_json::Value::String)
        }
        "sol_token_generate" => Ok(serde_json::Value::String(sol_panel::sol_token_generate())),
        "sol_default_dir" => Ok(serde_json::Value::String(sol_panel::sol_default_dir())),
        "sol_local_package" => {
            let local = tauri::async_runtime::block_on(sol_panel::sol_local_package(
                app.clone(),
                ui_opt_str(args, "dir"),
            ))?;
            serde_json::to_value(local).map_err(|error| format!("序列化失败: {error}"))
        }
        "sol_install_download" => {
            tauri::async_runtime::block_on(sol_panel::sol_install_download(
                app.clone(),
                ui_opt_str(args, "dir"),
            ))
            .map(serde_json::Value::String)
        }
        "sol_extract" => {
            let archive = ui_str(args, "archive")?;
            let dir = ui_opt_str(args, "dir");
            tauri::async_runtime::block_on(sol_panel::sol_extract(app.clone(), archive, dir))
                .map(serde_json::Value::String)
        }
        "sol_one_click_install" => {
            let dir = ui_opt_str(args, "dir");
            let archive = ui_opt_str(args, "archive");
            tauri::async_runtime::block_on(sol_panel::sol_one_click_install(
                app.clone(), dir, archive,
            ))
            .map(serde_json::Value::String)
        }
        "sol_service_ctl" => {
            let action = ui_str(args, "action")?;
            tauri::async_runtime::block_on(sol_panel::sol_service_ctl(app.clone(), action))
                .map(serde_json::Value::String)
        }
        "sol_log_client_error" => {
            sol_panel::sol_log_client_error(
                app.clone(),
                ui_str(args, "kind")?,
                ui_str(args, "detail")?,
            );
            Ok(serde_json::Value::Null)
        }
        _ => Err(format!("未知或不受支持的命令: {cmd}")),
    }
}

/// 立即执行还是先放进待执行槽位（延迟 > 0）。
pub(crate) fn start_remote_command(
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
pub(crate) fn execute_remote_command(
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
        RemoteCommand::WakeAck => (
            true,
            "机器在线（这条 HTTP 能送达就说明它醒着），不需要唤醒".to_string(),
        ),
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

pub(crate) fn pending_snapshot() -> Option<PendingRequest> {
    match http_lock().notify.as_ref() {
        Some(NotifyState::Pending(pending)) => Some(pending.clone()),
        _ => None,
    }
}

/// 立即执行当前待执行请求（弹窗/主窗口里的「立即执行」）。
pub(crate) fn confirm_pending_now(app: &AppHandle) -> Result<(), String> {
    let pending = pending_snapshot().ok_or_else(|| "没有待执行请求".to_string())?;
    run_pending_now(app, pending);
    Ok(())
}

/// 取消当前待执行请求（关掉弹窗也走这里）。
pub(crate) fn cancel_pending_now(app: &AppHandle) -> Result<(), String> {
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

pub(crate) fn run_pending_now(app: &AppHandle, pending: PendingRequest) {
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

/// 网页控制台静态资源（编译期嵌入）：内置 HTTP 服务直接把主窗口那套前端
/// 托管给局域网/外网浏览器，设置改动通过 /api/ui-command 真正作用于本机应用。
pub(crate) const UI_INDEX_HTML: &str = include_str!("../../ui/index.html");

pub(crate) const UI_APP_JS: &str = include_str!("../../ui/app.js");

pub(crate) const UI_STYLES_CSS: &str = include_str!("../../ui/styles.css");

pub(crate) const UI_GRUB_LIST_VIEW_JS: &str = include_str!("../../ui/grub-list-view.js");
