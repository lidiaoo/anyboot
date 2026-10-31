//! Windows 显示环境变化监听（分辨率 / 缩放比例）→ 触发「重置窗口大小」。
//!
//! 用户定案 2026-10-31：改分辨率 / 缩放后，主窗口按托盘右键「重置窗口大小」
//! 同一条路径重算尺寸。之前的 fit-to-work-area 方案（3e39171）只在窗口收到
//! Moved/Resized 时才夹取 —— 运行中改分辨率**不会改变窗口自身尺寸**，WM 不发
//! 那两条事件，夹取根本不跑，用户看到的就是「窗口一点没变」。
//!
//! 实现：MESSAGEONLY 隐形窗口（HWND_MESSAGE，不进任务栏不闪框）挂在独立线程
//! 的消息循环上，收两条广播：
//! - `WM_DISPLAYCHANGE`：分辨率 / 色深 / 方向变化；
//! - `WM_SETTINGCHANGE` 且 lParam = "WindowMetrics"：DPI 缩放比例改动（改缩放
//!   系统主要广播这个；MESSAGEONLY 窗口收不到 WM_DPICHANGED，那只发给
//!   top-level 窗口）。
//!
//! 收到后去抖 1s（显示切换常连发多条，重置落两拍几何也不便宜），期间有新
//! 广播就顺延到最后一拍，然后 `run_on_main_thread` 调
//! `window_geometry::reset_window_size_inner` —— 与托盘菜单「重置窗口大小」
//! 同一实现：按当前布局档位宽度 + 当前屏幕整屏 2/3 高度落尺寸（自带夹取、
//! 落盘、重居中）。改大改小都重置：屏幕变了，标称尺寸就该跟着屏幕重算。

use super::*;

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        TranslateMessage, MSG, WNDCLASSW, HWND_MESSAGE,
    };

    /// 广播到达后等多久再动手（ms）：显示切换常连发多条，只认最后一拍。
    const DEBOUNCE_MS: u64 = 1000;

    static APP: OnceLock<AppHandle> = OnceLock::new();
    /// 世代号：每条广播 +1；延迟线程醒来时世代已变 = 有更新的广播，让位。
    static EPOCH: AtomicU64 = AtomicU64::new(0);

    const WM_SETTINGCHANGE: u32 = 0x001A;
    const WM_DISPLAYCHANGE: u32 = 0x007E;

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

    pub(super) fn spawn(app: AppHandle) {
        let _ = APP.set(app);
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

/// 非 Windows：macOS 用 NSWindowDidChangeScreenNotification 一类再说，暂不实现。
#[cfg(not(target_os = "windows"))]
pub(crate) fn spawn(_app: AppHandle) {}
