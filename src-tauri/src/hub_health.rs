//! hub 系列独立窗的「页面还活着吗」判据（心跳）。
//!
//! ## 为什么需要
//!
//! `open_term_window` / `open_files_window` 这些函数的复用分支过去只看
//! `get_webview_window(label)` 有没有返回 —— 有就 show + 发事件。但
//! **窗口对象还在 ≠ 页面还活着**：
//!
//! - WebView2 的渲染进程崩掉（GPU/renderer crash）后，窗口壳仍挂在 Tauri 的
//!   窗口注册表里，内容区是纯白；
//! - 首帧就没加载出来时同理，窗口壳有、webview 永远停在白底。
//!
//! 于是之后每一次点击都只 show 那个白框、往一个死页面 emit 事件（`let _ =`
//! 把错误吞掉），页面永远不会重新加载。用户看到的就是：点「终端」/「文件」
//! 白框无响应，点多少次都一样；关掉重开程序也没用（关窗只是收进托盘，
//! 进程还在，那个僵尸窗口就一直在）。只有重启系统把进程全清掉才恢复。
//!
//! ## 判据
//!
//! 每个 hub 页面每 3s 发一次 `hub_page_ping`，页面能执行 JS 就说明它是活的。
//! 窗口存在但心跳停了（且过了首帧宽限期）= 僵尸窗口 → 销毁重建。
//! 建窗后再挂一个一次性看门狗：12s 还没就绪就记日志 + 隐藏，让下一次点击
//! 走重建，而不是继续把白框 show 给用户。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, WebviewWindow};

/// 前端心跳间隔（各页面的 setInterval 用的就是这个值）。
pub(crate) const PING_INTERVAL_MS: u64 = 3000;
/// 心跳停多久算死：约 5 个周期，容忍偶发卡顿，又不至于让白框挂太久。
const STALE_AFTER: Duration = Duration::from_secs(15);
/// 首帧宽限：窗口刚建好、页面 JS 还没跑起来的这段时间不能判死
/// （首次冷启动 WebView2 要初始化 profile，比后续慢得多）。
const FIRST_FRAME_GRACE: Duration = Duration::from_secs(25);
/// 看门狗等首帧的时间。
const FIRST_FRAME_WATCH: Duration = Duration::from_secs(12);

/// 页面心跳表（按窗口 label）。
#[derive(Default)]
pub struct HubPageHealth {
    beats: Mutex<HashMap<String, Beat>>,
}

struct Beat {
    /// 窗口建出来的时刻（后端登记，不依赖页面，页面自己起不来也有据可查）
    created: Instant,
    /// 最后一次收到心跳的时刻
    last: Instant,
    /// 页面是否已经报过 ready
    ready: bool,
    /// 累计心跳次数（排错时看「到底一次都没到」还是「跑着跑着断了」）
    pings: u64,
}

/// 页面心跳 / 就绪上报。前端 DOMContentLoaded 后先发一次 `ready=true`，
/// 之后每 [`PING_INTERVAL_MS`] 再发一次（ready 一直带 true）。
///
/// 刻意保持同步：函数体只有一次 `Mutex<HashMap>` 写入，不碰 IO、不起子进程，
/// 不会占住 WebView2 的 IPC 回调线程（见 `blocking_guard`）。
#[tauri::command]
pub fn hub_page_ping(state: tauri::State<'_, HubPageHealth>, window: WebviewWindow, ready: bool) {
    let label = window.label().to_string();
    let Ok(mut beats) = state.beats.lock() else {
        return;
    };
    let now = Instant::now();
    let entry = beats.entry(label).or_insert(Beat {
        created: now,
        last: now,
        ready: false,
        pings: 0,
    });
    entry.last = now;
    entry.pings = entry.pings.saturating_add(1);
    if ready {
        entry.ready = true;
    }
}

/// 建窗成功后立刻登记 —— `created` 必须由后端记，页面没跑起来时也要有起点。
pub(crate) fn note_created(app: &AppHandle, label: &str) {
    let Some(health) = app.try_state::<HubPageHealth>() else {
        return;
    };
    let Ok(mut beats) = health.beats.lock() else {
        return;
    };
    let now = Instant::now();
    beats.insert(
        label.to_string(),
        Beat {
            created: now,
            last: now,
            ready: false,
            pings: 0,
        },
    );
}

/// 该页面是否已报过 ready。
fn is_ready(app: &AppHandle, label: &str) -> bool {
    let Some(health) = app.try_state::<HubPageHealth>() else {
        return false;
    };
    let Ok(beats) = health.inner().beats.lock() else {
        return false;
    };
    beats.get(label).map(|beat| beat.ready).unwrap_or(false)
}

/// 该页面的心跳是否还能证明它活着。
///
/// 没有记录按「不活」处理（窗口在、后端却没登记 = 不是这条路径建的，
/// 重新建一次比继续用一个来路不明的窗口安全）。
pub(crate) fn page_alive(app: &AppHandle, label: &str) -> bool {
    let Some(health) = app.try_state::<HubPageHealth>() else {
        return false;
    };
    let Ok(beats) = health.inner().beats.lock() else {
        return false;
    };
    match beats.get(label) {
        None => false,
        Some(beat) => beat_alive(beat.ready, beat.last.elapsed(), beat.created.elapsed()),
    }
}

/// 判据本体（纯函数：单测直接喂时间差，不依赖 AppHandle 与真实时钟）。
///
/// 两个分支对应两种白框：
/// - 页面曾 ready 过、后来心跳停了 → 页面/渲染进程死了（`since_last` 超阈值）；
/// - 页面从没 ready → 首帧就没加载出来，只能靠建窗时长兜底。
fn beat_alive(ready: bool, since_last: Duration, since_created: Duration) -> bool {
    if ready {
        since_last < STALE_AFTER
    } else {
        since_created < FIRST_FRAME_GRACE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_beat_is_alive() {
        // 正常页面：3s 前刚发过心跳
        assert!(beat_alive(
            true,
            Duration::from_secs(3),
            Duration::from_secs(600)
        ));
    }

    #[test]
    fn stopped_heartbeat_is_dead() {
        // 白框：页面曾 ready，之后心跳断了 20s（渲染进程崩 / JS 卡死）
        assert!(!beat_alive(
            true,
            Duration::from_secs(20),
            Duration::from_secs(600)
        ));
    }

    #[test]
    fn slow_first_frame_stays_alive() {
        // 冷启动 WebView2 初始化慢：还没 ready，但在宽限期内，不能判死
        assert!(beat_alive(
            false,
            Duration::from_secs(10),
            Duration::from_secs(10)
        ));
    }

    #[test]
    fn never_ready_after_grace_is_dead() {
        // 首帧始终没出来 → 超过宽限期就按白框处理，下次点击重建
        assert!(!beat_alive(
            false,
            Duration::from_secs(40),
            Duration::from_secs(40)
        ));
    }

    #[test]
    fn heartbeat_interval_is_the_documented_one() {
        // 前端四个页面写死的 setInterval 就是 3000ms；改这里要同步改页面
        assert_eq!(PING_INTERVAL_MS, 3000);
        // 判死阈值必须显著大于心跳间隔，否则正常页面也会被误判
        assert!(STALE_AFTER.as_millis() as u64 > PING_INTERVAL_MS * 2);
    }
}

/// 白框诊断必须无条件落盘：`trace` 只在用户开着「调试日志」时才写，
/// 而这类问题恰恰发生在普通使用中（用户早就把开关关了）。所以同时写
/// `startup-debug.log`（永远写）和 `trace.log`（开了开关才有）。
fn note(app: &AppHandle, message: &str) {
    crate::startup_debug(app, &format!("[hub] {message}"));
    crate::trace(app, "hub", message);
}

/// 排错用：把某个窗口的心跳情况写成一行日志。
fn describe(app: &AppHandle, label: &str) -> String {
    let Some(health) = app.try_state::<HubPageHealth>() else {
        return "无心跳表".to_string();
    };
    let Ok(beats) = health.inner().beats.lock() else {
        return "心跳表被锁死".to_string();
    };
    match beats.get(label) {
        None => "无记录（后端从未登记该窗口）".to_string(),
        Some(beat) => format!(
            "ready={} pings={} 建窗后={}ms 最后心跳={}ms 前",
            beat.ready,
            beat.pings,
            beat.created.elapsed().as_millis(),
            beat.last.elapsed().as_millis()
        ),
    }
}

/// 忘掉某个窗口的心跳（判定为病态、准备重建时用）。
fn forget(app: &AppHandle, label: &str) {
    let Some(health) = app.try_state::<HubPageHealth>() else {
        return;
    };
    if let Ok(mut beats) = health.inner().beats.lock() {
        beats.remove(label);
    }
}

/// 复用窗口前先体检。
///
/// - 窗口健康 → `Some(window)`，调用方照旧 unminimize / show / set_focus / emit。
/// - 没有窗口 → `None`，调用方走建窗流程。
/// - **窗口在但页面已死** → 记日志、销毁窗口壳、等 label 从注册表摘掉，返回
///   `None` 让调用方重建。这是修「白框永远点不出来」的关键：过去这里只
///   `return Ok(())`，白框就永远不会被替换。
pub(crate) fn reuse_or_clear(app: &AppHandle, label: &str) -> Option<WebviewWindow> {
    let window = app.get_webview_window(label)?;
    if page_alive(app, label) {
        return Some(window);
    }
    let detail = describe(app, label);
    note(
        app,
        &format!("{label}: 窗口在但页面已死（{detail}）→ 销毁重建"),
    );
    let _ = window.destroy();
    forget(app, label);
    // 等 label 从注册表里摘掉：destroy 是派发到主线程的，紧跟其后的
    // build 若撞上旧 label 会报「label 已存在」。最多等 600ms。
    for _ in 0..40 {
        if app.get_webview_window(label).is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    None
}

/// 建窗之后挂一个一次性看门狗。
///
/// [`FIRST_FRAME_WATCH`] 秒后页面还没 ready（白框的典型形态）：
/// 记一条带诊断的日志、把心跳记录摘掉、隐藏窗口 —— 下一次点击就会走
/// 「销毁重建」而不是继续 show 这个白框。用户不必重启程序，更不必重启系统。
pub(crate) fn watch_first_frame(app: &AppHandle, label: &str) {
    let watch_app = app.clone();
    let watch_label = label.to_string();
    let label = watch_label.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("hub-frame-watch-{label}"))
        .spawn(move || {
            std::thread::sleep(FIRST_FRAME_WATCH);
            if is_ready(&watch_app, &label) {
                return;
            }
            let detail = describe(&watch_app, &label);
            note(
                &watch_app,
                &format!(
                    "{label}: 建窗 {}s 页面仍未就绪，判定白框并隐藏（{detail}）；下次打开会重建",
                    FIRST_FRAME_WATCH.as_secs()
                ),
            );
            // 摘记录 = 标记病态，让 reuse_or_clear 下次直接销毁重建
            forget(&watch_app, &label);
            if let Some(window) = watch_app.get_webview_window(&label) {
                let _ = window.hide();
            }
        });
    if let Err(error) = spawned {
        note(app, &format!("{watch_label}: 无法启动首帧看门狗: {error}"));
    }
}
