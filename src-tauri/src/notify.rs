//! 提示音 + 通知窗 + 调度器

use super::*;

/// 通知弹窗的窗口标签
pub(crate) const NOTIFY_WINDOW_LABEL: &str = "notify";

/// 通知弹窗宽度（逻辑像素）
pub(crate) const NOTIFY_WIDTH: f64 = 440.0;

/// 通知弹窗的**标称**高度：建窗口时用它，前端量不出真实内容高度时也用它兜底。
/// 正常路径下前端会量出「刚好兜住文字」的高度再报上来（见 present_notify_window）。
pub(crate) const NOTIFY_HEIGHT: f64 = 240.0;

/// 前端报上来的高度会被夹到这个区间：
/// 太低会把文字裁掉，太高就又变成「底下空一大片」——正是这次要修的东西。
pub(crate) const NOTIFY_MIN_HEIGHT: f64 = 150.0;

pub(crate) const NOTIFY_MAX_HEIGHT: f64 = 460.0;

/// 通知弹窗离工作区右下角的边距（逻辑像素）。
/// 注意用的是**工作区**（已排除任务栏），所以这个边距是「离任务栏 24px」而不是「离屏幕 24px」。
pub(crate) const NOTIFY_MARGIN: f64 = 24.0;

pub(crate) fn set_notify_state(app: &AppHandle, state: Option<NotifyState>) {
    let show = state.is_some();
    http_lock().notify = state;
    if show {
        if let Err(error) = show_notify_window(app) {
            trace(app, "notify", &format!("弹窗失败（不影响执行）: {error}"));
        }
    } else {
        close_notify_window(app);
    }
    publish_ui(app);
}

pub(crate) fn notify_view(app: &AppHandle) -> Option<NotifyView> {
    let runtime = http_lock();
    match runtime.notify.as_ref()? {
        NotifyState::Pending(pending) => {
            let settings = stored_settings(app);
            Some(NotifyView {
                kind: "pending",
                id: Some(pending.id.clone()),
                title: format!("远程请求（{}）", pending.channel.label()),
                message: pending.summary.clone(),
                source: pending.source.clone(),
                channel: pending.channel.as_str(),
                seconds_left: pending.seconds_left(),
                total_seconds: pending.seconds,
                created: pending.created.clone(),
                can_cancel: true,
                play_sound: settings.play_sound,
                sound_file_path: settings.sound_file_path.clone(),
            })
        }
        NotifyState::Info {
            message,
            source,
            expires_at,
        } => Some(NotifyView {
            kind: "info",
            id: None,
            title: "远程请求已执行".to_string(),
            message: message.clone(),
            source: source.clone(),
            channel: Channel::Http.as_str(),
            seconds_left: expires_at
                .saturating_duration_since(Instant::now())
                .as_secs(),
            total_seconds: 3,
            created: utc_timestamp(now_epoch_seconds()),
            can_cancel: false,
            play_sound: true,
            sound_file_path: String::new(),
        }),
    }
}

/// 显示置顶小窗（右下角）。失败不影响执行，只记 trace。
/// 内置提示音落到配置目录后的路径（内容不符就重写一遍，便于升级后自动更新）。
pub(crate) fn builtin_sound_path(app: &AppHandle) -> Option<PathBuf> {
    let dir = config_dir(app).ok()?.join(SOUND_DIR);
    if let Err(error) = std::fs::create_dir_all(&dir) {
        trace(app, "sound", &format!("创建提示音目录失败: {error}"));
        return None;
    }
    let path = dir.join(BUILTIN_SOUND_FILE);
    let needs_write = match std::fs::read(&path) {
        Ok(existing) => existing != BUILTIN_SOUND,
        Err(_) => true,
    };
    if needs_write {
        if let Err(error) = std::fs::write(&path, BUILTIN_SOUND) {
            trace(app, "sound", &format!("写入内置提示音失败: {error}"));
            return None;
        }
    }
    Some(path)
}

/// 各平台可用的播放方式（纯函数，便于单测）。
///
/// 走系统播放器而不是 webview 的 `Audio`：Linux 上 webkit2gtk 播放还要看 gstreamer 插件，
/// 缺一个就静音；而且自定义路径在 webview 里读不到（跨域/权限）。
pub(crate) fn sound_player_candidates(path: &Path) -> Vec<(String, Vec<String>)> {
    let file = path.display().to_string();
    if cfg!(target_os = "windows") {
        // SoundPlayer 只认 WAV PCM —— 内置音就是这种格式；自定义非 WAV 时这里会失败并回落。
        vec![(
            "powershell".to_string(),
            vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!("(New-Object Media.SoundPlayer '{file}').PlaySync();"),
            ],
        )]
    } else if cfg!(target_os = "macos") {
        vec![("afplay".to_string(), vec![file])]
    } else {
        vec![
            ("paplay".to_string(), vec![file.clone()]),
            ("aplay".to_string(), vec!["-q".to_string(), file.clone()]),
            (
                "ffplay".to_string(),
                vec![
                    "-nodisp".to_string(),
                    "-autoexit".to_string(),
                    "-loglevel".to_string(),
                    "quiet".to_string(),
                    file,
                ],
            ),
        ]
    }
}

/// 播放一个音频文件：依次尝试本平台的候选播放器，返回第一个跑起来的。
pub(crate) fn play_sound_file(path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("文件不存在: {}", path.display()));
    }
    let mut last_error = String::from("本平台没有可用的播放器");
    for (program, args) in sound_player_candidates(path) {
        match command(&program)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            // 只等「起得来」，不等播放完：通知窗口的出现不该被 2 秒音频拖住。
            Ok(_child) => return Ok(program),
            Err(error) => last_error = format!("{program}: {error}"),
        }
    }
    Err(last_error)
}

/// 收到远程请求时播放提示音：自定义路径优先，失败回落内置 KakaoTalk 提示音。
pub(crate) fn play_notify_sound(app: &AppHandle) {
    let settings = stored_settings(app);
    if !settings.play_sound {
        trace(app, "sound", "提示音已关闭（设置里 playSound=false）");
        return;
    }
    let mut candidates: Vec<(PathBuf, &'static str)> = Vec::new();
    let custom = settings.sound_file_path.trim();
    if !custom.is_empty() {
        candidates.push((PathBuf::from(custom), "自定义"));
    }
    if let Some(builtin) = builtin_sound_path(app) {
        candidates.push((builtin, "内置"));
    }
    if candidates.is_empty() {
        trace(app, "sound", "没有可播放的提示音文件");
        return;
    }
    let mut last_error = String::new();
    for (path, label) in candidates {
        match play_sound_file(&path) {
            Ok(program) => {
                trace(
                    app,
                    "sound",
                    &format!("已播放{label}提示音（{program}）：{}", path.display()),
                );
                return;
            }
            Err(error) => last_error = format!("{label}提示音失败（{error}）"),
        }
    }
    trace(app, "sound", &format!("提示音没播出来：{last_error}"));
}

/// 前端报上来的高度夹到 [NOTIFY_MIN_HEIGHT, NOTIFY_MAX_HEIGHT]（纯函数，便于单测）。
/// `None`（前端量不出真实内容高度）退回标称高度。
pub(crate) fn notify_height(raw: Option<f64>) -> f64 {
    match raw {
        Some(value) if value.is_finite() && value > 0.0 => {
            value.clamp(NOTIFY_MIN_HEIGHT, NOTIFY_MAX_HEIGHT)
        }
        _ => NOTIFY_HEIGHT,
    }
}

pub(crate) fn show_notify_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW_LABEL) {
        let _ = window.show();
        return Ok(());
    }
    apply_browser_args(tauri::WebviewWindowBuilder::new(
        app,
        NOTIFY_WINDOW_LABEL,
        tauri::WebviewUrl::App("notify.html".into()),
    ))
    .title("远程请求")
    .inner_size(NOTIFY_WIDTH, NOTIFY_HEIGHT)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    // 先建出来但**不显示**：等前端把内容画好、量出真实高度，调 `present_notify_window`
    // 时才定尺寸、定位置、显示。
    //
    // 老实现是「建出来（默认可见）→ set_position」，于是窗口先用系统默认位置画一帧空白
    // （屏幕左上角），再跳到右下角 —— 就是用户看到的「先渲染空白窗口随后才跳过去」。
    // 后来改成「page-load → 定位 → 显示」，但那时的尺寸还是写死的 320 高，
    // 内容只有 200 出头，底下就一直空着一大块。现在尺寸由前端按内容量出来。
    .visible(false)
    .build()
    .map_err(|error| format!("无法创建通知窗口: {error}"))?;

    // 兜底：前端脚本万一没跑起来（弹窗不能就这么消失），800ms 后按标称高度自己露出来。
    let fallback = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(800));
        if let Some(window) = fallback.get_webview_window(NOTIFY_WINDOW_LABEL) {
            if !window.is_visible().unwrap_or(true) {
                trace(&fallback, "notify", "前端 800ms 未量出高度，按标称兜底显示");
                place_notify_window(&fallback, &window, NOTIFY_HEIGHT);
                let _ = window.show();
            }
        }
    });
    Ok(())
}

/// 前端量好高度后「把弹窗摆出来」：定尺寸 → 摆到工作区右下角 → 显示。
///
/// 之所以由前端来触发，是因为只有它知道内容有多高（原生菜单/窗口没有「自适应内容」这回事）。
/// `height` 传 `None` 表示量不出来，用标称高度。
#[tauri::command]
pub(crate) fn present_notify_window(app: AppHandle, height: Option<f64>) -> Result<(), String> {
    let Some(window) = app.get_webview_window(NOTIFY_WINDOW_LABEL) else {
        return Ok(());
    };
    let height = notify_height(height);
    trace(&app, "notify", &format!("present height={height}"));
    let _ = window.set_size(tauri::LogicalSize::new(NOTIFY_WIDTH, height));
    place_notify_window(&app, &window, height);
    let _ = window.show();
    Ok(())
}

/// 把通知窗口摆到「**工作区**右下角、留 NOTIFY_MARGIN 边距」。
///
/// 两个关键点：
/// 1. 用 `work_area()` 而不是 `size()`：任务栏占的那一条不属于工作区，
///    用屏幕尺寸摆就会像以前那样压住任务栏（用户截图里就是这个）。
/// 2. 尺寸用「我们正要设成的逻辑高度」换算，而不是 `outer_size()` 读回来的 ——
///    `set_size` 是投递到主线程的异步操作，设完立刻读回拿到的还是旧尺寸，位置会偏几十像素。
///
/// 刚建出来的窗口 `current_monitor()` 在 Linux 上可能还是 None，回退到主显示器；
/// 都拿不到（极罕见）就保持系统默认位置，不去猜。
pub(crate) fn place_notify_window(app: &AppHandle, window: &tauri::WebviewWindow, logical_height: f64) {
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let width = (NOTIFY_WIDTH * scale).round() as i32;
    let height = (logical_height * scale).round() as i32;
    let margin = (NOTIFY_MARGIN * scale).round() as i32;
    let work = monitor.work_area();
    let _ = window.set_position(tauri::PhysicalPosition::new(
        work.position.x + work.size.width as i32 - width - margin,
        work.position.y + work.size.height as i32 - height - margin,
    ));
}

pub(crate) fn close_notify_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(NOTIFY_WINDOW_LABEL) {
        let _ = window.close();
    }
}

/// 每秒 tick 的调度线程：到点执行待执行请求、事后的 info 提示到点自动关窗。
pub(crate) fn spawn_notify_scheduler(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        // 锁内只做判断（Some(Some(pending)) = 到点执行，Some(None) = info 到点关窗）
        let due = {
            let runtime = http_lock();
            notify_due(runtime.notify.as_ref(), Instant::now())
        };
        match due {
            // 倒计时到点：执行
            NotifyDue::Execute(pending) => run_pending_now(&app, *pending),
            // 事后提示到期：收掉弹窗
            NotifyDue::CloseInfo => set_notify_state(&app, None),
            NotifyDue::Nothing => {}
        }
    });
}
