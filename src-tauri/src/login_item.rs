//! 开机自启：三平台副本（macOS 登录项 / LaunchAgent / Linux XDG autostart）
//! 成组维护；内含 macOS SMAppService 的内联 mod login_item，路径别改。

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutostartState {
    Enabled,
    /// 只有 macOS 会构造（系统登录项已注册、等用户在系统设置里批准）；
    /// 其他平台保留这个取值，界面文案与状态语义才完整。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    RequiresApproval,
    Disabled,
    Unavailable,
}

impl AutostartState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::RequiresApproval => "requires-approval",
            Self::Disabled => "disabled",
            Self::Unavailable => "unavailable",
        }
    }

    pub(crate) fn is_on(self) -> bool {
        matches!(self, Self::Enabled | Self::RequiresApproval)
    }

    pub(crate) fn hint(self) -> Option<&'static str> {
        match self {
            Self::RequiresApproval => {
                Some("已注册登录项，请在「系统设置 → 通用 → 登录项与扩展」中允许本应用")
            }
            Self::Unavailable => Some(
                "无法通过系统登录项接口管理开机启动（需以 .app 形式运行），可在「系统设置 → 通用 → 登录项与扩展」中手动添加",
            ),
            _ => None,
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn app_bundle_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|executable| executable.canonicalize().ok())
        .and_then(|executable| executable.ancestors().nth(3).map(Path::to_path_buf))
        .filter(|bundle| {
            bundle
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
                && bundle.join("Contents").is_dir()
        })
}

#[cfg(target_os = "macos")]
pub(crate) fn running_from_app_bundle() -> bool {
    app_bundle_path().is_some()
}

/// macOS 开机启动的实现方式。
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutostartBackend {
    /// macOS 13+ 且应用已签名：走系统登录项接口（SMAppService），
    /// 条目由用户在「系统设置 → 通用 → 登录项与扩展」中管理。
    LoginItem,
    /// 未签名 / 未以 .app 运行 / macOS 12 及以下：退回 ~/Library/LaunchAgents。
    ///
    /// 系统登录项接口要求应用带有效签名，未签名的 bundle 调用
    /// `registerAndReturnError()` 只会拿到
    /// `SMAppServiceErrorDomain code=1 "Operation not permitted"`，
    /// 因此这种情况必须用兼容方式，否则开关点了没有任何效果。
    LaunchAgent,
}

#[cfg(target_os = "macos")]
pub(crate) fn autostart_backend_for(supported: bool, in_bundle: bool, signed: bool) -> AutostartBackend {
    if supported && in_bundle && signed {
        AutostartBackend::LoginItem
    } else {
        AutostartBackend::LaunchAgent
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn autostart_backend() -> AutostartBackend {
    autostart_backend_for(
        login_item::is_supported(),
        running_from_app_bundle(),
        app_bundle_is_signed(),
    )
}

/// 当前 .app 是否带有有效代码签名（ad-hoc 签名同样算有效）。
///
/// SMAppService 只接受已签名的应用，所以这里实际跑一次 `codesign --verify`
/// 探测，而不是要求必须是 Developer ID 证书。
#[cfg(target_os = "macos")]
pub(crate) fn app_bundle_is_signed() -> bool {
    static SIGNED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SIGNED.get_or_init(|| {
        let Some(bundle) = app_bundle_path() else {
            return false;
        };
        command("/usr/bin/codesign")
            .arg("--verify")
            .arg(&bundle)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// 系统登录项注册失败时的提示。已签名的应用也可能因为系统登录项数据库
/// 状态异常而注册失败，这时会退回 LaunchAgent，需要把原因告诉用户。
#[cfg(target_os = "macos")]
pub(crate) fn login_item_failure() -> Option<String> {
    static NOTICE: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
    NOTICE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("autostart notice mutex poisoned")
        .clone()
}

#[cfg(target_os = "macos")]
pub(crate) fn note_login_item_failure(error: &str) {
    static NOTICE: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
    *NOTICE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("autostart notice mutex poisoned") = Some(error.to_string());
}

#[cfg(target_os = "macos")]
pub(crate) fn clear_login_item_failure() {
    static NOTICE: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
    *NOTICE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("autostart notice mutex poisoned") = None;
}

/// 兼容方式（LaunchAgent）生效时说明为什么没用系统登录项。
#[cfg(target_os = "macos")]
pub(crate) fn launch_agent_hint() -> String {
    if !login_item::is_supported() {
        return "当前系统版本低于 macOS 13，已改用兼容方式（~/Library/LaunchAgents）实现开机启动"
            .to_string();
    }
    if !running_from_app_bundle() {
        return "当前未以 .app 形式运行，已改用兼容方式（~/Library/LaunchAgents）实现开机启动"
            .to_string();
    }
    "当前应用没有代码签名，系统登录项接口不可用，已改用兼容方式（~/Library/LaunchAgents）实现开机启动；使用签名版本后可在「系统设置 → 通用 → 登录项与扩展」中管理"
        .to_string()
}

/// 界面提示：先说明系统状态，再说明实际用的是哪种开机启动实现。
#[cfg(not(target_os = "macos"))]
pub(crate) fn autostart_hint(state: AutostartState) -> Option<String> {
    state.hint().map(str::to_string)
}

#[cfg(target_os = "macos")]
pub(crate) fn autostart_hint(state: AutostartState) -> Option<String> {
    macos_autostart_hint(
        state,
        autostart_backend(),
        login_item_failure(),
        launch_agent_hint(),
    )
}

/// 提示语的纯逻辑部分，和运行环境解耦，便于单测。
#[cfg(target_os = "macos")]
pub(crate) fn macos_autostart_hint(
    state: AutostartState,
    backend: AutostartBackend,
    login_item_error: Option<String>,
    compatibility_hint: String,
) -> Option<String> {
    // 需要用户在系统设置里勾选时，优先给这条可操作的提示。
    if state == AutostartState::RequiresApproval {
        return state.hint().map(str::to_string);
    }
    if let Some(error) = login_item_error {
        return Some(format!(
            "系统登录项注册失败（{error}），已改用兼容方式（~/Library/LaunchAgents）实现开机启动"
        ));
    }
    // 已经开启但不是通过系统登录项实现的，说明现在用的是兼容方式。
    if state.is_on() && backend == AutostartBackend::LaunchAgent {
        return Some(compatibility_hint);
    }
    state.hint().map(str::to_string)
}

/// macOS 13+ 使用系统登录项（SMAppService），开机启动会出现在
/// 「系统设置 → 通用 → 登录项与扩展 → 登入时打开」里。
#[cfg(target_os = "macos")]
pub(crate) mod login_item {
    use objc2::rc::Retained;
    use objc2::runtime::AnyClass;
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    use super::AutostartState;

    pub fn is_supported() -> bool {
        AnyClass::get(c"SMAppService").is_some()
    }

    fn main_app() -> Result<Retained<SMAppService>, String> {
        if !is_supported() {
            return Err("当前系统版本不支持登录项（需要 macOS 13 及以上）".to_string());
        }
        Ok(unsafe { SMAppService::mainAppService() })
    }

    pub fn state() -> AutostartState {
        let Ok(service) = main_app() else {
            eprintln!("SMAppService 类不可用（macOS 低于 13）");
            return AutostartState::Unavailable;
        };
        let status = unsafe { service.status() };
        // SMAppServiceStatus 枚举: 0=NotRegistered, 1=Enabled, 2=RequiresApproval,
        // 3=NotFound。应用被更新或移动后，旧注册会短暂返回 NotFound(3)；
        // 不能把它当作 Unavailable（会误导为“未以 .app 运行”），
        // 应视为未注册，让注册流程/启动自愈重新注册。
        if status == SMAppServiceStatus::Enabled {
            AutostartState::Enabled
        } else if status == SMAppServiceStatus::RequiresApproval {
            AutostartState::RequiresApproval
        } else if status == SMAppServiceStatus::NotRegistered
            || status == SMAppServiceStatus::NotFound
        {
            AutostartState::Disabled
        } else {
            AutostartState::Unavailable
        }
    }

    pub fn set(enabled: bool) -> Result<AutostartState, String> {
        let service = main_app()?;
        let mut result = unsafe {
            if enabled {
                service.registerAndReturnError()
            } else {
                service.unregisterAndReturnError()
            }
        };
        // 应用被更新/替换后，旧注册可能处于失效状态，直接注册会失败；
        // 先注销失效的旧注册再重试一次（Apple 推荐的恢复方式）。
        if result.is_err() && enabled {
            let _ = unsafe { service.unregisterAndReturnError() };
            result = unsafe { service.registerAndReturnError() };
        }
        let state = state();
        match result {
            Ok(()) => Ok(state),
            Err(error) => {
                let already_there = if enabled {
                    state.is_on()
                } else {
                    state == AutostartState::Disabled
                };
                if already_there {
                    return Ok(state);
                }
                let action = if enabled { "开启" } else { "关闭" };
                Err(format!(
                    "无法{action}开机启动: {} (code={})",
                    error.localizedDescription(),
                    error.code()
                ))
            }
        }
    }

    pub fn open_system_settings() -> Result<(), String> {
        main_app()?;
        unsafe { SMAppService::openSystemSettingsLoginItems() };
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_autostart_state(app: &AppHandle) -> AutostartState {
    match autostart_backend() {
        AutostartBackend::LoginItem => login_item::state(),
        AutostartBackend::LaunchAgent => legacy_autostart_state(app),
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    match autostart_backend() {
        AutostartBackend::LoginItem => match login_item::set(enabled) {
            Ok(state) => {
                clear_login_item_failure();
                Ok(state)
            }
            // 已签名的应用也可能因为系统登录项数据库状态异常而注册失败，
            // 这时退回 LaunchAgent，保证开机启动开关依然生效。
            Err(error) => {
                eprintln!("系统登录项接口调用失败，改用兼容方式: {error}");
                note_login_item_failure(&error);
                legacy_autostart_set(app, enabled)
            }
        },
        AutostartBackend::LaunchAgent => legacy_autostart_set(app, enabled),
    }
}

pub(crate) fn legacy_autostart_state(app: &AppHandle) -> AutostartState {
    match app.autolaunch().is_enabled() {
        Ok(true) => AutostartState::Enabled,
        Ok(false) => AutostartState::Disabled,
        Err(_) => AutostartState::Unavailable,
    }
}

pub(crate) fn legacy_autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    let manager = app.autolaunch();
    let action = if enabled { "开启" } else { "关闭" };
    if enabled {
        manager
            .enable()
            .map_err(|error| format!("无法{action}开机启动: {error}"))?;
    } else {
        manager
            .disable()
            .map_err(|error| format!("无法{action}开机启动: {error}"))?;
    }
    Ok(legacy_autostart_state(app))
}

#[cfg(target_os = "macos")]
pub(crate) fn autostart_state(app: &AppHandle) -> AutostartState {
    macos_autostart_state(app)
}

#[cfg(target_os = "linux")]
pub(crate) fn autostart_state(app: &AppHandle) -> AutostartState {
    linux_autostart_state(app)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn autostart_state(app: &AppHandle) -> AutostartState {
    legacy_autostart_state(app)
}

#[cfg(target_os = "macos")]
pub(crate) fn autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    macos_autostart_set(app, enabled)
}

#[cfg(target_os = "linux")]
pub(crate) fn autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    linux_autostart_set(app, enabled)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    legacy_autostart_set(app, enabled)
}

#[cfg(target_os = "linux")]
pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_autostart_file(app: &AppHandle) -> Result<PathBuf, String> {
    let invoking_user = if is_root() {
        linux_invoking_user_home()
    } else {
        Some(
            app.path()
                .home_dir()
                .map_err(|error| format!("无法定位用户主目录: {error}"))?,
        )
    };
    let home = invoking_user.ok_or_else(|| "无法识别当前登录用户".to_string())?;
    let directory = home.join(".config").join("autostart");
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建自启动目录: {error}"))?;
    let name = app.package_info().name.replace([' ', '/'], "-");
    Ok(directory.join(format!("{name}.desktop")))
}

/// 早期版本在 macOS 上写入 ~/Library/LaunchAgents 实现开机启动，
/// 改用系统登录项后需要清理，否则登录时会被重复拉起。
#[cfg(target_os = "macos")]
pub(crate) fn remove_legacy_launch_agents(app: &AppHandle) {
    let Ok(home) = app.path().home_dir() else {
        return;
    };
    let directory = home.join("Library").join("LaunchAgents");
    let name = app.package_info().name.clone();
    let identifier = app.config().identifier.clone();
    let candidates = [
        format!("{name}.plist"),
        format!("{}.plist", name.replace(' ', "-")),
        format!("{identifier}.plist"),
    ];
    for file in candidates {
        let path = directory.join(&file);
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        if !contents.contains("anyboot") && !contents.contains(&name) {
            continue;
        }
        let _ = command("launchctl")
            .arg("unload")
            .arg("--")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = fs::remove_file(&path);
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn cleanup_legacy_autostart(app: &AppHandle) {
    // 只有系统登录项真正生效时才清理 LaunchAgent 文件，否则会把兼容方式
    // （未签名 / 未以 .app 运行）下正在使用的自启动项一起删掉。
    if autostart_backend() == AutostartBackend::LoginItem
        && login_item::state() == AutostartState::Enabled
    {
        remove_legacy_launch_agents(app);
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn cleanup_legacy_autostart(_app: &AppHandle) {}

/// 应用被更新或移动后，登录项可能失效；启动时按已保存的设置自愈一次。
pub(crate) fn startup_autostart_sync(app: &AppHandle) {
    cleanup_legacy_autostart(app);
    let desired = stored_settings(app).autostart;
    let state = autostart_state(app);
    eprintln!(
        "启动时检查开机启动：设置={desired}，系统状态={}",
        state.as_str()
    );
    if desired && state == AutostartState::Disabled {
        match autostart_set(app, true) {
            Ok(new_state) => eprintln!("已重新注册开机启动登录项，状态: {}", new_state.as_str()),
            Err(error) => eprintln!("重新注册开机启动登录项失败: {error}"),
        }
    }
}

pub(crate) fn toggle_autostart_setting(app: &AppHandle) {
    let enabled = !stored_settings(app).autostart;
    trace(
        app,
        "tray-setting",
        &format!("setting-autostart → enabled={enabled}"),
    );
    match autostart_set(app, enabled).and_then(|state| {
        let mut settings = stored_settings(app);
        settings.autostart = state.is_on();
        save_settings(app, &settings)
    }) {
        Ok(_) => {
            record_action(
                app,
                "开机启动",
                true,
                if enabled {
                    "已开启开机启动"
                } else {
                    "已关闭开机启动"
                },
            );
            publish_settings(app);
        }
        Err(error) => {
            // 失败时也要同步一次：让主窗口看到系统里的真实状态和提示。
            record_action(app, "开机启动", false, &error);
            publish_settings(app);
            trace(app, "tray-setting", &format!("切换开机启动失败: {error}"));
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_autostart_state(app: &AppHandle) -> AutostartState {
    match linux_autostart_file(app) {
        Ok(path) if path.exists() => AutostartState::Enabled,
        Ok(_) => AutostartState::Disabled,
        Err(_) => AutostartState::Unavailable,
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_autostart_set(app: &AppHandle, enabled: bool) -> Result<AutostartState, String> {
    let path = linux_autostart_file(app)?;
    if path.exists() {
        let stale = fs::read_to_string(&path)
            .map(|contents| !contents.contains("AnyBoot") && !contents.contains("anyboot"))
            .unwrap_or(false);
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
    if !enabled {
        if path.exists() {
            fs::remove_file(&path).map_err(|error| format!("无法关闭开机启动: {error}"))?;
        }
        return Ok(AutostartState::Disabled);
    }

    let executable = std::env::current_exe()
        .map_err(|error| format!("无法定位当前程序: {error}"))?
        .canonicalize()
        .map_err(|error| format!("无法解析当前程序路径: {error}"))?;
    let appimage = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    let launch_path = appimage.unwrap_or(executable);
    let content = format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName={}\nComment=AnyBoot\nExec={} --from-autostart\nStartupNotify=false\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        app.package_info().name,
        shell_quote(&launch_path.to_string_lossy())
    );
    fs::write(&path, content).map_err(|error| format!("无法写入用户自启动项: {error}"))?;
    Ok(AutostartState::Enabled)
}
