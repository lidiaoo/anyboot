//! NVIDIA / WebKitGTK 兼容环境变量


/// 环境变量布尔开关的纯逻辑部分（便于单测）：1/true/yes/on 与 0/false/no/off。
///
/// trace 与电源 dry-run 两个探针共用。
pub(crate) fn env_flag_from(value: Option<&str>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// 一次会话里 GDK / WebKitGTK 实际选中的显示后端。
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LinuxDisplayBackend {
    X11,
    Wayland,
    /// 线索全无（既没有 `GDK_BACKEND` 也没有会话变量）：按 X11 处理，理由见选择表。
    Unknown,
}

/// 从环境变量推断后端（纯函数，便于单测）。
///
/// 顺序照抄 GDK 自己的选择逻辑：`GDK_BACKEND` 是硬指定、优先于一切，它是个逗号分隔的
/// 候选表，第一个认得出的 x11/wayland 算数（`broadway` 这类认不出的项跳过）；
/// 没设才依次看 `XDG_SESSION_TYPE`、`WAYLAND_DISPLAY`、`DISPLAY`。
/// 注意 XWayland 下 `DISPLAY` 也在，所以 Wayland 的判定必须排在它前面。
#[cfg(target_os = "linux")]
pub(crate) fn linux_display_backend(
    gdk_backend: Option<&str>,
    session_type: Option<&str>,
    wayland_display: Option<&str>,
    display: Option<&str>,
) -> LinuxDisplayBackend {
    if let Some(candidates) = gdk_backend {
        for candidate in candidates.split(',').map(str::trim) {
            match candidate.to_ascii_lowercase().as_str() {
                "x11" => return LinuxDisplayBackend::X11,
                "wayland" => return LinuxDisplayBackend::Wayland,
                _ => continue,
            }
        }
    }
    match session_type.map(|value| value.trim().to_ascii_lowercase()) {
        Some(kind) if kind == "x11" => return LinuxDisplayBackend::X11,
        Some(kind) if kind == "wayland" => return LinuxDisplayBackend::Wayland,
        _ => {}
    }
    if wayland_display.is_some_and(|value| !value.is_empty()) {
        return LinuxDisplayBackend::Wayland;
    }
    if display.is_some_and(|value| !value.is_empty()) {
        return LinuxDisplayBackend::X11;
    }
    LinuxDisplayBackend::Unknown
}

/// 从 `/proc/driver/nvidia/version` 的内容里取驱动主版本号（纯函数，便于单测）。
///
/// 文件内容形如：
/// ```text
/// NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.71.09  Release Build  (root@arch)
/// GCC version:  gcc 16.2.1 20260810 (GCC)
/// ```
/// 「第一个带点的数字词」就是版本号，要的是它的主版本（`egl-wayland2` 看 ≥560）。
/// **必须锚定 `NVRM version:` 这一行**：下面那行的 `gcc 16.2.1` 也是带点的数字词，
/// 不锚定就会把编译器版本当成驱动版本。
#[cfg(target_os = "linux")]
pub(crate) fn nvidia_major_from_version_text(text: &str) -> Option<u32> {
    let banner = text
        .lines()
        .find(|line| line.trim_start().starts_with("NVRM version:"))?;
    banner.split_whitespace().find_map(|word| {
        let (major, rest) = word.split_once('.')?;
        // 至少得是「主.次」两段数字，`x86_64` 这种词不算
        if rest.is_empty() || !rest.starts_with(|character: char| character.is_ascii_digit()) {
            return None;
        }
        major.parse::<u32>().ok()
    })
}

/// NVIDIA 专有驱动的主版本号；没装 / 没加载（纯核显模式、nouveau）返回 `None`。
///
/// `/proc/driver/nvidia/version` 只有 NVIDIA 自己的驱动会建（nouveau 与 Mesa 都不建），
/// 所以「这文件在」≈「这次会话的渲染有 NVIDIA 参与」，也正是要加开关的时刻；
/// 反过来，切到纯核显模式后这个文件就没了，开关自动不加，性能也不会白白吃亏。
#[cfg(target_os = "linux")]
pub(crate) fn nvidia_driver_major() -> Option<u32> {
    let text = std::fs::read_to_string("/proc/driver/nvidia/version").ok()?;
    nvidia_major_from_version_text(&text)
}

/// 这次会话该给 WebKit 加哪些开关（纯函数，便于单测）：可能一个都不加。
#[cfg(target_os = "linux")]
pub(crate) fn linux_gpu_env_overrides(
    backend: LinuxDisplayBackend,
    desktop: Option<&str>,
    driver_major: u32,
) -> &'static [(&'static str, &'static str)] {
    let desktop = desktop.unwrap_or("").to_ascii_lowercase();
    match backend {
        // 白窗 / 内容看不见都出在这条路上。后端认不出来时也按 X11 处理：
        // 画不出来比性能掉一点点严重得多。
        LinuxDisplayBackend::X11 | LinuxDisplayBackend::Unknown => X11_GPU_SWITCHES,
        // 缺 acquire point 会被合成器当协议错误掐断连接，DMA-BUF 必须关
        LinuxDisplayBackend::Wayland if desktop.contains("hyprland") => DMABUF_RENDERER_OFF,
        // KWin 一类严格同步的合成器：关掉 NVIDIA 显式同步就没事了
        LinuxDisplayBackend::Wayland if desktop.contains("kde") || desktop.contains("plasma") => {
            NVIDIA_EXPLICIT_SYNC_OFF
        }
        // 驱动 ≥560 自带能用的 egl-wayland2，这条路径本来是好的，别动它
        LinuxDisplayBackend::Wayland if driver_major >= 560 => NO_GPU_SWITCHES,
        // 更老的 egl-wayland：不关显式同步可能连不上
        LinuxDisplayBackend::Wayland => NVIDIA_EXPLICIT_SYNC_OFF,
    }
}

/// 用户是否已经自己配过 WebKit 的这些开关（纯函数，便于单测）。
#[cfg(target_os = "linux")]
pub(crate) fn webkit_env_already_configured(probe: impl Fn(&str) -> bool) -> bool {
    WEBKIT_ENV_SWITCHES.iter().any(|name| probe(name))
}

/// 按会话类型给 WebKit 塞进与 NVIDIA 相容的开关。
///
/// **必须在 GTK / WebKit 起来之前调用**：WebKit 是在建 WebView 的那一刻读这些变量的，
/// 之后再设就没用了。所以调用点在 `run()` 的最前面，早于 `tauri::Builder`。
#[cfg(target_os = "linux")]
pub(crate) fn apply_linux_gpu_workarounds() {
    if webkit_env_already_configured(|name| std::env::var_os(name).is_some()) {
        return;
    }
    // 没有 NVIDIA 驱动（核显模式 / nouveau / Mesa）：这条路径本来就没问题，什么都别动
    let Some(driver_major) = nvidia_driver_major() else {
        return;
    };
    let backend = linux_display_backend(
        std::env::var("GDK_BACKEND").ok().as_deref(),
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("DISPLAY").ok().as_deref(),
    );
    let overrides = linux_gpu_env_overrides(
        backend,
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
        driver_major,
    );
    if overrides.is_empty() {
        return;
    }
    for (name, value) in overrides {
        std::env::set_var(name, value);
    }
    // 白窗这类问题现场很难复现，ANYBOOT_TRACE=1 时留一行痕
    // （启动期还没有日志文件可写，所以走 stderr，和 trace 的口径一致）
    if env_flag_from(std::env::var("ANYBOOT_TRACE").ok().as_deref()) == Some(true) {
        let applied = overrides
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        eprintln!("anyboot: NVIDIA 驱动 {driver_major} / {backend:?}，已设置 {applied}");
    }
}
