//! 切换状态流：状态快照、发布、切换命令面。读写 AppState（state.rs）。

use super::*;

pub(crate) fn state_snapshot(app: &AppHandle) -> AppStatus {
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().expect("state mutex poisoned");
    AppStatus {
        vars_dir: state.vars_dir.as_deref().map(display_path),
        temporary_mount: state.active_mount.as_ref().is_some_and(|mount| mount.owned),
        variables: state.variables.clone(),
        message: state.message.clone(),
        error: state.error.clone(),
    }
}

/// 兼容旧调用点：状态本身已经写进 AppState，这里只把新快照发出去。
pub(crate) fn publish_status(app: &AppHandle, status: &AppStatus) {
    trace(
        app,
        "event",
        &format!(
            "状态已更新 msg={} err={} tmp={}",
            status.message,
            status.error.as_deref().unwrap_or("-"),
            status.temporary_mount
        ),
    );
    publish_ui(app);
}

pub(crate) fn refresh_and_update(app: &AppHandle) -> AppStatus {
    let started = Instant::now();
    // 每轮清空埋点表：多轮混在一起就看不出单轮账了
    perf::begin();
    trace(app, "scan", "开始扫描");
    set_busy(app, |busy| busy.scanning = true);
    let (explicit_dir, old_mount) = {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        (state.explicit_dir.clone(), state.active_mount.take())
    };
    if let Some(mount) = old_mount {
        let _ = mount.unmount();
    }

    // rEFInd 变量目录扫描：Linux 上可能要临时挂盘，Windows/macOS 是纯文件读。
    // 计时用显式 drop 收尾（不套 `{}`），免得整段 match 多缩进一层、diff 变噪音
    let vars_span = perf::span("vars.scan");
    let status = match scan_vars_directory(explicit_dir.as_deref()) {
        Ok(result) => {
            let message = result
                .variables
                .iter()
                .find(|item| item.active)
                .map(|item| format!("下次启动：{}", item.display_name))
                .unwrap_or_else(|| "已连接 rEFInd；PreviousBoot 未匹配模板".to_string());
            let status = AppStatus {
                vars_dir: Some(display_path(&result.vars_dir)),
                temporary_mount: result
                    .active_mount
                    .as_ref()
                    .is_some_and(|mount| mount.owned),
                variables: result.variables.clone(),
                message: message.clone(),
                error: None,
            };
            // 模拟引导项扫描（纯读）：与上面那份「手动获取」的 vars 快照并列，
            // 按 rEFInd 自己的规则复算菜单。失败不影响手动链路 —— 它只是展示。
            let sim = {
                let _span = perf::span("refind.sim");
                channels::refind_menu::preview(&result.vars_dir)
            };
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            state.refind_sim = Some(sim);
            state.vars_dir = Some(result.vars_dir);
            state.active_mount = result.active_mount;
            state.variables = result.variables;
            state.message = message;
            state.error = None;
            status
        }
        Err(error) => {
            let message = "未找到可用的 rEFInd 变量目录".to_string();
            let status = AppStatus {
                vars_dir: None,
                temporary_mount: false,
                variables: Vec::new(),
                message: message.clone(),
                error: Some(error),
            };
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            state.vars_dir = None;
            state.variables = Vec::new();
            state.refind_sim = None;
            state.message = message;
            state.error = status.error.clone();
            status
        }
    };
    drop(vars_span);

    trace(
        app,
        "scan",
        &format!(
            "完成 耗时={}ms result={}",
            started.elapsed().as_millis(),
            status.error.as_deref().unwrap_or("ok")
        ),
    );
    // GRUB 那条链路跟着一起刷新：Linux 端顺带把 /boot/grub 的两份数据同步到 ESP，
    // 其它端就是重新解析 ESP 上的副本。用户要的「每次启动软件自动同步一次」在这里。
    // **硬性条件（用户定案）**：ESP 必须已经由当前系统挂载在 /boot 下（fstab
    // 统一方案）—— 没挂就整条跳过：不自动挂载、不 pkexec、不写盘。手动链路
    // （识别/挂载/同步按钮）不受此限。
    {
        let _span = perf::span("grub.refresh");
        refresh_grub(app);
    }
    // 三引导器通道（systemd-boot / OC / Clover）跟着刷：只做文件/变量读取，
    // ESP 尽量复用 GRUB 链路解析好的那份
    {
        let _span = perf::span("channels.refresh");
        channels::refresh_channels(app);
    }
    // 埋点汇总：必须在两条链路都跑完之后，否则看不到它们各占多少
    perf::flush(app, started.elapsed().as_millis());
    // 扫描结束：清 busy 并发布快照（托盘菜单与主窗口一起更新、菜单指纹相同就跳过重建）
    set_busy(app, |busy| busy.scanning = false);
    status
}

/// 切换下次启动系统。busy.switching 由外层统一置位/清除，
/// 这样「未知系统」「目录未就绪」这些提前返回也不会把状态卡在 busy 上。
pub(crate) fn switch_and_update(app: &AppHandle, system: &str) -> AppStatus {
    let started = Instant::now();
    trace(app, "switch", &format!("请求 system={system}"));
    set_busy(app, |busy| busy.switching = true);
    let status = switch_and_update_inner(app, system, started);
    set_busy(app, |busy| busy.switching = false);
    status
}

/// 本机主动切换系统（主窗口按钮 / 托盘菜单）的统一入口。
///
/// 先作废「挂着的远程请求」再切换：网页 / 手机端选系统会形成一条 pending（要等
/// 倒计时到点才执行），本机用户在这段时间里自己点了另一个系统 —— 本机的操作是更
/// 明确、更权威的意图，绝不能被那条到点的请求盖回去（否则用户看到的是「我明明
/// 选了 A，几秒后自己变回 B」）。**只在本机入口做这件事**：远程请求到点执行时也
/// 会走 switch_and_update，从那里清理就等于自己把自己取消掉。
pub(crate) fn switch_system_locally(app: &AppHandle, system: &str) -> AppStatus {
    if let Some(pending) = pending_snapshot() {
        trace(
            app,
            "remote",
            &format!(
                "本机切换到 {system}：挂着的远程请求 {}（{}）作废",
                pending.id, pending.summary
            ),
        );
        if let Err(error) = cancel_pending_now(app) {
            // 作废失败不该拦住切换本身（切换才是用户真正要的事）
            trace(app, "remote", &format!("作废挂起请求失败（不影响切换）: {error}"));
        }
    }
    switch_and_update(app, system)
}

pub(crate) fn switch_and_update_inner(app: &AppHandle, system: &str, started: Instant) -> AppStatus {
    // 三引导器通道的值域（sdboot:<id> / opencore / …）分流到 channels 链路；
    // 原有 linux/windows/macos（rEFInd 模板复制）零变化
    if channels::SwitchSpec::is_channel_spec(system) {
        let status = channels::switch_channel_and_update(app, system);
        trace(
            app,
            "switch",
            &format!("通道切换完成 system={system} 耗时={}ms", started.elapsed().as_millis()),
        );
        return status;
    }
    let selected = SOURCE_FILES
        .iter()
        .find(|(candidate, _, _)| *candidate == system)
        .cloned();
    let Some((_system, display_name, source_name)) = selected else {
        let mut status = state_snapshot(app);
        status.error = Some(format!("未知系统: {system}"));
        status.temporary_mount = {
            let state = app.state::<Mutex<AppState>>();
            let state = state.lock().expect("state mutex poisoned");
            state.active_mount.as_ref().is_some_and(|mount| mount.owned)
        };
        return status;
    };

    let (directory, active_mount) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (state.vars_dir.clone(), state.active_mount.clone())
    };
    let Some(directory) = directory else {
        refresh_and_update(app);
        let mut status = state_snapshot(app);
        status.error = Some("变量目录尚未就绪".to_string());
        return status;
    };

    let result = write_with_mount(active_mount.as_ref(), || {
        let source = directory.join(source_name);
        let target = directory.join("PreviousBoot");
        if !source.is_file() {
            return Err(format!("变量文件不存在: {}", source.display()));
        }
        atomic_copy(&source, &target)
            .and_then(|_| {
                if hashes_match(&source, &target) {
                    Ok(())
                } else {
                    Err(io::Error::other(format!(
                        "写入校验失败: {}",
                        target.display()
                    )))
                }
            })
            .map_err(|error| format!("写入失败: {error}"))
    });

    trace(
        app,
        "switch",
        &format!(
            "完成 system={system} 耗时={}ms result={}",
            started.elapsed().as_millis(),
            result.as_ref().err().map(String::as_str).unwrap_or("ok")
        ),
    );
    match result {
        Ok(()) => {
            let status = refresh_and_update(app);
            let message = format!("下次启动：{display_name}");
            record_action(app, "切换系统", true, &message);
            status
        }
        Err(error) => {
            record_action(app, "切换系统", false, &error);
            update_failure_status(app, &format!("切换 {display_name} 失败"), error)
        }
    }
}

pub(crate) fn update_failure_status(app: &AppHandle, message: &str, error: String) -> AppStatus {
    let (existing_directory, existing_variables) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (state.vars_dir.clone(), state.variables.clone())
    };
    let status = AppStatus {
        vars_dir: existing_directory,
        temporary_mount: {
            let state = app.state::<Mutex<AppState>>();
            let state = state.lock().expect("state mutex poisoned");
            state.active_mount.as_ref().is_some_and(|mount| mount.owned)
        },
        variables: existing_variables,
        message: message.to_string(),
        error: Some(error),
    };
    {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state.message = message.to_string();
        state.error = status.error.clone();
    }
    publish_status(app, &status);
    status
}

#[tauri::command]
pub(crate) fn current_status(app: AppHandle) -> AppStatus {
    state_snapshot(&app)
}

#[tauri::command]
pub(crate) fn refresh(app: AppHandle) -> AppStatus {
    trace(&app, "command", "refresh（主窗口）");
    refresh_and_update(&app)
}

#[tauri::command]
pub(crate) fn set_vars_dir(app: AppHandle, path: Option<String>) -> AppStatus {
    trace(&app, "command", &format!("set_vars_dir path={:?}", path));
    let directory = path
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from);
    {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state.explicit_dir = directory;
    }
    refresh_and_update(&app)
}

#[tauri::command]
pub(crate) fn switch_system(app: AppHandle, system: String) -> AppStatus {
    trace(
        &app,
        "command",
        &format!("switch_system system={system}（主窗口）"),
    );
    // 本机入口：会先作废挂着的远程请求，别让它到点把本机选择盖回去
    switch_system_locally(&app, &system)
}
