//! 显示环境变化监听（分辨率 / 缩放比例）→ 触发「重置窗口大小」。三端：
//! Windows 走广播快速路径 + 主线程外轮询；macOS / Linux 纯轮询（采样经
//! 主线程）。行为一致：显示环境变了就按标称尺寸重置，改大改小都重置。
//!
//! 用户定案 2026-10-31：改分辨率 / 缩放后，主窗口按托盘右键「重置窗口大小」
//! 同一条路径重算尺寸。之前的 fit-to-work-area 方案（3e39171）只在窗口收到
//! Moved/Resized 时才夹取 —— 运行中改分辨率**不会改变窗口自身尺寸**，WM 不发
//! 那两条事件，夹取根本不跑，用户看到的就是「窗口一点没变」。
//!
//! 实现（两层）：
//! 1. MESSAGEONLY 隐形窗口（HWND_MESSAGE，不进任务栏不闪框）挂在独立线程
//!    的消息循环上，收 `WM_DISPLAYCHANGE`：分辨率 / 色深 / 方向变化。
//! 2. **轮询兜底**（用户实测 2026-10-31：改 DPI 缩放 300→175 时
//!    `WM_SETTINGCHANGE(WindowMetrics)` 根本没到——这条广播不可靠）：
//!    独立线程每 1.5s 枚举全部显示器，采样「物理分辨率 + EFFECTIVE_DPI」
//!    签名，和上次比变了就触发。注意签名必须用每显示器的有效 DPI——
//!    `GetDpiForSystem` 的「系统 DPI」登录时定格、运行中改缩放不动
//!    （第一版轮询靠它，用户实测同样没反应的根因）。
//!
//! 收到后去抖 1s（显示切换常连发多条，重置落两拍几何也不便宜），然后
//! `run_on_main_thread` 调 `window_geometry::reset_window_size_inner` —— 与
//! 托盘菜单「重置窗口大小」同一实现：按当前布局档位宽度 + 当前屏幕整屏
//! 2/3 高度落尺寸（自带夹取、落盘、重居中）。改大改小都重置：屏幕变了，
//! 标称尺寸就该跟着屏幕重算。

use super::*;

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::GetDpiForMonitor;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        TranslateMessage, MSG, WNDCLASSW, HWND_MESSAGE, WM_DISPLAYCHANGE, WM_SETTINGCHANGE,
    };

    static APP: OnceLock<AppHandle> = OnceLock::new();
    /// 世代号：每条广播 +1；延迟线程醒来时世代已变 = 有更新的广播，让位。
    static EPOCH: AtomicU64 = AtomicU64::new(0);

    /// 广播/轮询命中后等多久再动手（ms）：显示切换常连发多条，只认最后一拍。
    const DEBOUNCE_MS: u64 = 1000;
    /// 轮询间隔（ms）：读 DPI + 主屏分辨率，每拍一次。轻量调用，1.5s 足够跟手。
    const POLL_INTERVAL_MS: u64 = 1500;

    /// 请求一次重置：世代 +1，起延迟线程，醒来时仍是最新世代才真正落尺寸。
    fn request_reset(reason: &str) {
        let my = EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
        let Some(app) = APP.get() else { return };
        let app = app.clone();
        let reason = reason.to_string();
        std::thread::Builder::new()
            .name("display-change-reset".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(DEBOUNCE_MS));
                if EPOCH.load(Ordering::SeqCst) != my {
                    return; // 期间又来了广播，让最后一拍去干
                }
                let app2 = app.clone();
                let reason2 = reason.clone();
                let result = app.run_on_main_thread(move || {
                    match crate::window_geometry::reset_window_size_inner(&app2) {
                        Ok(_) => {
                            trace(&app2, "display-watch", &format!("显示环境变化（{reason2}），已按标称尺寸重置窗口"));
                        }
                        Err(error) => {
                            trace(&app2, "display-watch", &format!("显示环境变化（{reason2}）重置窗口失败: {error}"));
                        }
                    }
                });
                if let Err(error) = result {
                    trace(&app, "display-watch", &format!("重置任务排不进主线程: {error}"));
                }
            })
            .ok();
    }

    /// 读 lParam 指向的 UTF-16 字符串（最多 64 个单元，够 "WindowMetrics"）。
    fn lparam_string(lparam: LPARAM) -> String {
        let ptr = lparam as usize as *const u16;
        if ptr.is_null() {
            return String::new();
        }
        let mut units = Vec::new();
        let mut i = 0usize;
        while i < 64 {
            let unit = unsafe { *ptr.add(i) };
            if unit == 0 {
                break;
            }
            units.push(unit);
            i += 1;
        }
        String::from_utf16_lossy(&units)
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            // 分辨率 / 色深 / 屏幕方向变化
            WM_DISPLAYCHANGE => {
                request_reset("分辨率变化");
                0
            }
            // DPI 缩放比例改动：系统广播 WM_SETTINGCHANGE，lparam = "WindowMetrics"。
            // 其它设置变化（主题 / 时区…）同消息不同参数，不重置（太吵）。
            WM_SETTINGCHANGE => {
                if lparam_string(lparam).eq_ignore_ascii_case("WindowMetrics") {
                    request_reset("缩放变化");
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    /// 显示器签名条目：物理宽、物理高、有效 DPI x/y（每台显示器一条）。
    type MonitorSig = Vec<(i32, i32, u32, u32)>;

    /// EnumDisplayMonitors 回调：收一台显示器的物理 rect + 有效 DPI。
    unsafe extern "system" fn monitor_enum_proc(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let list = &mut *(lparam as usize as *mut MonitorSig);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmonitor, &mut info) != 0 {
            let mut dpi_x: u32 = 0;
            let mut dpi_y: u32 = 0;
            // MDT_EFFECTIVE_DPI = 该显示器当前缩放比例的体现，改缩放立即变。
            // 查询失败（感知级别不配合）也不拦：物理 rect 的变化仍能抓到分辨率档。
            let _ = GetDpiForMonitor(hmonitor, 0 /* MDT_EFFECTIVE_DPI */, &mut dpi_x, &mut dpi_y);
            list.push((
                info.rcMonitor.right - info.rcMonitor.left,
                info.rcMonitor.bottom - info.rcMonitor.top,
                dpi_x,
                dpi_y,
            ));
        }
        1 // TRUE：继续枚举
    }

    /// 当前显示环境签名：全部显示器的（物理宽、高、有效 DPI），排序后比较。
    /// 任何显示器改缩放（任意值）或改分辨率，签名都会变。
    fn display_signature() -> MonitorSig {
        let mut list: MonitorSig = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(monitor_enum_proc),
                &mut list as *mut _ as LPARAM,
            );
        }
        list.sort_unstable();
        list
    }

    /// 轮询兜底线程：每 1.5s 采样一次显示签名，变了就走重置。
    /// 广播（WM_SETTINGCHANGE）在部分机器改缩放时不发/参数对不上，这条路
    /// 不依赖任何广播 —— 任何缩放值（100→125→150→300 任意）都会被抓到。
    fn spawn_poll() {
        std::thread::Builder::new()
            .name("display-dpi-poll".into())
            .spawn(move || {
                // 首拍基线：显示器枚举签名 + 主窗 scale_factor（tao 收到
                // WM_DPICHANGED 后更新，双保险，不依赖进程感知级别的假设）。
                let mut last = display_signature();
                let mut last_scale = window_scale();
                loop {
                    std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                    let now = display_signature();
                    let now_scale = window_scale();
                    if now != last || now_scale != last_scale {
                        last = now;
                        last_scale = now_scale;
                        request_reset("缩放/分辨率变化（轮询）");
                    }
                }
            })
            .ok();
    }

    /// 主窗当前 scale_factor（拿不到窗口按 0.0，首轮之后窗口在就没问题）。
    fn window_scale() -> f64 {
        APP.get()
            .and_then(|app| app.get_webview_window("main"))
            .and_then(|window| window.scale_factor().ok())
            .unwrap_or(0.0)
    }

    pub(super) fn spawn(app: AppHandle) {
        let _ = APP.set(app);
        spawn_poll();
        std::thread::Builder::new()
            .name("display-change-watch".into())
            .spawn(move || unsafe {
                let hinstance = GetModuleHandleW(std::ptr::null());
                let class_name: Vec<u16> = "AnyBootDisplayWatch\0".encode_utf16().collect();
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    lpszClassName: class_name.as_ptr(),
                    hInstance: hinstance,
                    ..std::mem::zeroed()
                };
                if RegisterClassW(&wc) == 0 {
                    return;
                }
                // HWND_MESSAGE = 消息专用窗口：不可见、无任务栏项，只收广播
                let hwnd = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    hinstance,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return;
                }
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, hwnd, 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })
            .ok();
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn spawn(app: AppHandle) {
    imp::spawn(app);
}

/// 非 Windows（macOS / Linux）：同一「轮询 + 签名比对」策略（用户定案
/// 2026-10-31：三端行为一致）。差别只在采样层 —— AppKit / GTK 的显示器
/// 查询不是线程安全的，采样必须落在主线程，轮询线程只负责节拍：
/// sleep → run_on_main_thread 采样 → channel 取回 → 比对 → 变了就主线程重置。
///
/// 签名 = 全部显示器的（位置、物理宽高、scale_factor）：
/// - macOS：改缩放 → backingScaleFactor 变；改分辨率 → size 变；
/// - Linux：改缩放/分辨率同理（Wayland 下各屏 position 可能全 0，
///   靠 size + scale_factor 兜住信号）。
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod imp {
    use super::*;
    use std::time::Duration;

    /// 轮询间隔（ms），与 Windows 端同一节奏。
    const POLL_INTERVAL_MS: u64 = 1500;

    /// 显示器签名条目：x、y、物理宽、物理高、scale_factor 的位模式
    /// （f64 没有 Eq，比较前转 bits —— `f64::to_bits()` 是 u64，别写成 u32，
    /// 否则 Windows 上永远编不到这个 cfg 分支、Linux 一编才炸）。
    type MonitorSig = Vec<(i32, i32, u32, u32, u64)>;

    /// 采样当前全部显示器签名。**只能在主线程调用**。
    fn sample(app: &AppHandle) -> MonitorSig {
        let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
            return Vec::new();
        };
        let Ok(monitors) = main.available_monitors() else {
            return Vec::new();
        };
        let mut sig: MonitorSig = monitors
            .iter()
            .map(|m| {
                let pos = m.position();
                let size = m.size();
                (
                    pos.x,
                    pos.y,
                    size.width,
                    size.height,
                    m.scale_factor().to_bits(),
                )
            })
            .collect();
        sig.sort_unstable();
        sig
    }

    pub(super) fn spawn(app: AppHandle) {
        std::thread::Builder::new()
            .name("display-change-watch".into())
            .spawn(move || {
                let mut last: Option<MonitorSig> = None;
                loop {
                    std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                    // 采样必须主线程：排队过去，channel 取回
                    let (tx, rx) = std::sync::mpsc::channel::<MonitorSig>();
                    let app2 = app.clone();
                    if app.run_on_main_thread(move || {
                        let _ = tx.send(sample(&app2));
                    })
                    .is_err()
                    {
                        continue; // 事件循环已退出，应用在退出中
                    }
                    let Ok(sig) = rx.recv_timeout(Duration::from_secs(2)) else {
                        continue; // 主线程忙到 2s 都没排上，下一拍再说
                    };
                    let Some(last_sig) = &last else {
                        // 首拍只记基线，不触发（窗口还没起来时采到空也算基线）
                        if !sig.is_empty() {
                            last = Some(sig);
                        }
                        continue;
                    };
                    if sig.is_empty() || &sig == last_sig {
                        continue;
                    }
                    last = Some(sig.clone());
                    let app3 = app.clone();
                    let result = app.run_on_main_thread(move || {
                        match crate::window_geometry::reset_window_size_inner(&app3) {
                            Ok(_) => trace(
                                &app3,
                                "display-watch",
                                "显示环境变化（轮询），已按标称尺寸重置窗口",
                            ),
                            Err(error) => trace(
                                &app3,
                                "display-watch",
                                &format!("显示环境变化（轮询）重置窗口失败: {error}"),
                            ),
                        }
                    });
                    if let Err(error) = result {
                        trace(&app, "display-watch", &format!("重置任务排不进主线程: {error}"));
                    }
                }
            })
            .ok();
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn spawn(app: AppHandle) {
    imp::spawn(app);
}
