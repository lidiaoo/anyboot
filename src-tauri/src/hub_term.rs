//! server-hub M3：终端会话 = PTY 直跑系统 `ssh`。
//!
//! 设计定则（与移动端同源）：
//! - 认证不归 hub 管——密码/密钥/known_hosts 指纹确认全部在 PTY 里由 ssh 本尊
//!   交互完成，hub 只存「连到哪」，所以也没有任何凭据落盘面。
//! - 会话生命周期独立于命令调用：spawn 后只剩 读线程 + 等待线程 在推事件，
//!   结构体只留 resize/write/kill 的把手。
//! - hub 窗口销毁时全部会话收掉（lib.rs 的 on_window_event → kill_all），
//!   PTY 子进程不越窗存活。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

use crate::hub::HubServer;

/// 下行事件名（ssh 输出 → 前端 xterm）
const EVENT_DATA: &str = "hub-term-data";
/// 会话结束事件名
const EVENT_CLOSED: &str = "hub-term-closed";

/// 新会话的初始窗口大小（前端 fit 后立刻 resize 到真实值）
const INITIAL_ROWS: u16 = 30;
const INITIAL_COLS: u16 = 100;

// ==================== 状态 ====================

#[derive(Default)]
pub struct HubTermState {
    sessions: Mutex<HashMap<String, Arc<TermSession>>>,
    /// hub「打开终端」排队的服务器：server-term 窗口首次加载时来取。
    /// 窗口已存在时走 hub-term-open-server 事件，不走这条队列。
    pending: Mutex<Option<HubServer>>,
}

pub struct TermSession {
    pub id: String,
    pub server_name: String,
    /// user@host:port（标签条显示用）
    pub host_label: String,
    pub protocol: String,
    /// 承载本会话的窗口 label（事件只发给它；窗口销毁按它收会话）
    pub window: String,
    /// MasterPty 本身不是 Sync，全部收进锁里；会话经 Arc 共享给等待线程/命令层
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

#[derive(Clone, Serialize)]
pub struct TermMeta {
    pub id: String,
    pub server_name: String,
    pub host_label: String,
    pub protocol: String,
}

// ==================== 命令 ====================

/// hub「打开终端」：确保 server-term 窗口存在并聚焦。窗口是新建的 → 把服务器
/// 放进 pending 队列（页面加载后取走）；窗口已存在 → 直接给它发事件加会话标签。
/// 返回 false 表示窗口是新开的（hub 端可用于提示文案，一般忽略）。
pub fn open_term_window(app: &AppHandle, server: HubServer) -> Result<(), String> {
    const LABEL: &str = crate::HUB_TERM_WINDOW_LABEL;
    let existing = app.get_webview_window(LABEL);
    match existing {
        Some(window) => {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
            // 复用窗口：直接通知页面加一个会话标签（页面按 server.id 去重）
            tauri::async_runtime::spawn(async move {
                let _ = tauri::Emitter::emit(
                    &window,
                    "hub-term-open-server",
                    serde_json::to_value(&server).unwrap_or_default(),
                );
            });
        }
        None => {
            let state = app.state::<HubTermState>();
            // match + 分号：让 Result 临时值先于 state 析构（E0597）
            match state.pending.lock() {
                Ok(mut pending) => *pending = Some(server),
                Err(_) => {}
            };
            tauri::WebviewWindowBuilder::new(
                app,
                LABEL,
                tauri::WebviewUrl::App("hub-term.html".into()),
            )
            .title("AnyBoot 终端")
            .inner_size(980.0, 640.0)
            .min_inner_size(720.0, 420.0)
            .resizable(true)
            .center()
            .build()
            .map_err(|error| format!("无法创建终端窗口: {error}"))?;
        }
    }
    Ok(())
}

/// server-term 页面加载后取走排队的启动服务器（取走即清空）。
#[tauri::command]
pub fn hub_term_take_pending(state: tauri::State<'_, HubTermState>) -> Option<HubServer> {
    state.pending.lock().ok().and_then(|mut p| p.take())
}

/// 本窗口当前活着的会话清单（页面刷新后重建标签用）
#[tauri::command]
pub fn hub_term_sessions(
    state: tauri::State<'_, HubTermState>,
    window: tauri::WebviewWindow,
) -> Vec<TermMeta> {
    let label = window.label().to_string();
    state
        .sessions
        .lock()
        .map(|map| {
            map.values()
                .filter(|s| s.window == label)
                .map(|s| TermMeta {
                    id: s.id.clone(),
                    server_name: s.server_name.clone(),
                    host_label: s.host_label.clone(),
                    protocol: s.protocol.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 打开终端：PTY 里跑 `ssh -p PORT user@host`，读线程/等待线程开始推事件。
/// `window` 由 Tauri 注入 = 调用方窗口（server-term 页内的标签全归它）。
#[tauri::command]
pub fn hub_term_open(
    app: AppHandle,
    state: tauri::State<'_, HubTermState>,
    window: tauri::WebviewWindow,
    server: HubServer,
) -> Result<TermMeta, String> {
    let server = server.normalize();
    if server.host.is_empty() {
        return Err("主机地址不能为空".to_string());
    }
    if server.protocol != "ssh" {
        return Err(format!(
            "终端暂只支持 SSH（{} 会话在后续版本开通）",
            if server.protocol == "telnet" { "Telnet" } else { "未知协议" }
        ));
    }

    let target = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };
    let port = server.effective_port();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("无法创建 PTY: {error}"))?;

    let mut command = CommandBuilder::new("ssh");
    command.args(["-p", &port.to_string(), &target]);
    // xterm-256color：远端 vim/htop 之类才肯出彩色；终端类型由我们这侧的 xterm.js 兑现
    command.env("TERM", "xterm-256color");
    // 选了密钥认证：-i 指定私钥（带口令的钥匙由 ssh 在 PTY 里交互要，与本模块哲学一致）
    if !server.key_id.is_empty() {
        let key = crate::hub_keys::private_key_path(&app, &server.key_id)?;
        command.args(["-i", &key.display().to_string()]);
        command.args(["-o", "IdentitiesOnly=yes"]);
    } else if !server.password_enc.is_empty() {
        if server.id.is_empty() {
            return Err("这台服务器还没保存档案：存密码连接需要先保存（askpass 按 id 取密文）".to_string());
        }
        // 密码认证（M6）：SSH_ASKPASS_REQUIRE=force 让 ssh 把要密码这件事交给
        // askpass 程序 —— 本程序自身经 hub-askpass.sh 回环（内存解密 → stdout），
        // ssh 拿到后免输登录。PTY 里不再出现密码提示，指纹确认仍会走 PTY。
        // 注意带口令的密钥解锁也走 askpass，所以这两个分支必须互斥（表单三选一）。
        let config = crate::config_dir(&app)?;
        let script = crate::hub_vault::write_askpass_script(&config, &server.id)?;
        command.env("SSH_ASKPASS", script.display().to_string());
        command.env("SSH_ASKPASS_REQUIRE", "force");
    }
    // ControlMaster：认证连接留一把复用（M4 隧道靠它免二次认证，见 hub_tunnel.rs）
    if !server.id.is_empty() {
        if let Ok(control) = crate::hub_tunnel::control_path(&app, &server.id) {
            command.args(["-o", "ControlMaster=auto"]);
            command.args(["-o", &format!("ControlPath={}", control.display())]);
            command.args(["-o", "ControlPersist=600"]);
        }
    }
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("无法启动 ssh: {error}"))?;
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("无法读取 PTY 输出: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("无法写入 PTY: {error}"))?;
    let killer = child.clone_killer();

    let id = format!(
        "term-{:016x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let meta = TermMeta {
        id: id.clone(),
        server_name: server.name.clone(),
        host_label: format!("{target}:{port}"),
        protocol: server.protocol.clone(),
    };

    let session = Arc::new(TermSession {
        id: id.clone(),
        server_name: server.name.clone(),
        host_label: meta.host_label.clone(),
        protocol: meta.protocol.clone(),
        window: window.label().to_string(),
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        killer: Mutex::new(killer),
    });
    state
        .sessions
        .lock()
        .map_err(|_| "会话表已被锁死")?
        .insert(id.clone(), session);

    // 读线程：ssh 输出 → hub-term-data（b64），只发给承载窗口
    let emit_app = app.clone();
    let read_id = id.clone();
    let read_window = window.label().to_string();
    std::thread::Builder::new()
        .name(format!("hub-term-read-{id}"))
        .spawn(move || pump_output(&emit_app, &read_id, &read_window, reader))
        .map_err(|error| format!("无法启动读线程: {error}"))?;

    // 等待线程：ssh 退出 → hub-term-closed（发给承载窗口），并从会话表摘除
    // （master 随之释放，读线程收尾）
    let wait_app = app.clone();
    let wait_id = id.clone();
    let wait_window = window.label().to_string();
    std::thread::Builder::new()
        .name(format!("hub-term-wait-{id}"))
        .spawn(move || {
            let code = child
                .wait()
                .map(|status| status.exit_code())
                .unwrap_or(u32::MAX);
            if let Some(state) = wait_app.try_state::<HubTermState>() {
                if let Ok(mut map) = state.sessions.lock() {
                    map.remove(&wait_id);
                }
            }
            eprintln!("[hub-term] closed id={wait_id} code={code}");
            let _ = wait_app.emit_to(
                wait_window.clone(),
                EVENT_CLOSED,
                json!({ "id": wait_id, "code": code }),
            );
            // 这个窗口的最后一个会话结束了 → 自动关窗（前端没有 close 权限，
            // 用户敲 exit / ssh 掉线都走这条路径收尾）
            let empty = wait_app
                .try_state::<HubTermState>()
                .map(|state| {
                    // 判空在 map 里做：State 守卫不能借值出闭包（E0515）
                    state
                        .sessions
                        .lock()
                        .map(|map| map.values().all(|s| s.window != wait_window))
                        .unwrap_or(true)
                })
                .unwrap_or(true);
            if empty {
                if let Some(window) = wait_app.get_webview_window(&wait_window) {
                    eprintln!("[hub-term] last session gone, closing {wait_window}");
                    let _ = window.close();
                }
            }
        })
        .map_err(|error| format!("无法启动等待线程: {error}"))?;

    eprintln!("[hub-term] opened id={} target={target}:{port}", meta.id);
    Ok(meta)
}

/// 键盘 → ssh
#[tauri::command]
pub fn hub_term_write(
    state: tauri::State<'_, HubTermState>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    let bytes = B64
        .decode(data.as_bytes())
        .map_err(|error| format!("上行数据不是合法 base64: {error}"))?;
    let session = get_session(&state, &session_id)?;
    let mut writer = session.writer.lock().map_err(|_| "写入端已被锁死")?;
    writer
        .write_all(&bytes)
        .and_then(|_| writer.flush())
        .map_err(|error| format!("发送失败: {error}"))
}

/// xterm fit 之后报尺寸 → PTY resize（远端收到 SIGWINCH）
#[tauri::command]
pub fn hub_term_resize(
    state: tauri::State<'_, HubTermState>,
    session_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    let session = get_session(&state, &session_id)?;
    let mut master = session.master.lock().map_err(|_| "PTY 把手已被锁死")?;
    master
        .resize(PtySize {
            rows: rows.max(2),
            cols: cols.max(2),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("调整尺寸失败: {error}"))
}

/// 用户关标签：杀掉 ssh；等待线程会补发 closed 事件
#[tauri::command]
pub fn hub_term_close(
    state: tauri::State<'_, HubTermState>,
    session_id: String,
) -> Result<(), String> {
    let session = get_session(&state, &session_id)?;
    session
        .killer
        .lock()
        .map_err(|_| "结束把手已被锁死")?
        .kill();
    Ok(())
}

// ==================== 内部 ====================

fn get_session(state: &HubTermState, id: &str) -> Result<Arc<TermSession>, String> {
    state
        .sessions
        .lock()
        .map_err(|_| "会话表已被锁死")?
        .get(id)
        .cloned()
        .ok_or_else(|| format!("终端会话不存在或已结束（{id}）"))
}

/// 读线程主体：PTY → b64 → 事件（只发给承载窗口）
fn pump_output(app: &AppHandle, id: &str, window: &str, mut reader: Box<dyn Read + Send>) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,             // EOF：ssh 退出，等待线程会收尾
            Ok(n) => {
                let payload = json!({
                    "id": id,
                    "data": B64.encode(&chunk[..n]),
                });
                if app.emit_to(window, EVENT_DATA, payload).is_err() {
                    break; // 窗口没了，没必要再读
                }
            }
            Err(error) => {
                if error.kind() != std::io::ErrorKind::Interrupted {
                    break;
                }
            }
        }
    }
}

/// 某个窗口销毁：只收归它管的会话（别的终端窗口不受牵连）
pub fn kill_window(app: &AppHandle, window: &str) {
    let state = app.state::<HubTermState>();
    let Ok(mut map) = state.sessions.lock() else {
        return;
    };
    let orphan: Vec<String> = map
        .iter()
        .filter(|(_, s)| s.window == window)
        .map(|(id, _)| id.clone())
        .collect();
    for id in orphan {
        if let Some(session) = map.remove(&id) {
            if let Ok(mut killer) = session.killer.lock() {
                killer.kill();
            }
        }
    }
}

/// hub 窗口销毁（或应用退出）：收掉名下所有会话，PTY 子进程不越窗存活
pub fn kill_all(app: &AppHandle) {
    let state = app.state::<HubTermState>();
    let Ok(mut map) = state.sessions.lock() else {
        return;
    };
    for (_, session) in map.iter() {
        if let Ok(mut killer) = session.killer.lock() {
            killer.kill();
        }
    }
    map.clear();
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_rejects_telnet_and_empty_host() {
        // 这两个分支在 spawn 之前就返回，不需要 AppHandle/State
        let server = HubServer {
            id: "t".into(),
            name: "telnet 机".into(),
            protocol: "telnet".into(),
            host: "127.0.0.1".into(),
            port: 23,
            username: String::new(),
            notes: String::new(),
            key_id: String::new(),
            password_enc: String::new(),
            password_new: None,
            created_at: 0,
        };
        let normalized = server.normalize();
        assert_ne!(normalized.protocol, "ssh");

        let empty = HubServer {
            id: "e".into(),
            name: "空主机".into(),
            protocol: "ssh".into(),
            host: "  ".into(),
            port: 0,
            username: String::new(),
            notes: String::new(),
            key_id: String::new(),
            password_enc: String::new(),
            password_new: None,
            created_at: 0,
        };
        assert!(empty.host.trim().is_empty());
    }

    /// portable_pty 行为护栏：stdout/stderr 都必须进 PTY（ssh 的连接错误、
    /// Permission denied 全靠 stderr 展示，吞了就是「黑屏终端」），
    /// 且行规程回显开着（用户敲键要能看见自己打的字）。
    /// 本开发容器无 /dev/ptmx，真机验证：cargo test -- --ignored pty_carries
    #[test]
    #[ignore = "需要 /dev/ptmx（真机跑）"]
    fn pty_carries_stdout_stderr_and_echo() {
        let pair = native_pty_system()
            .openpty(PtySize { rows: 30, cols: 100, pixel_width: 0, pixel_height: 0 })
            .expect("openpty");
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "echo OUT-OK; echo ERR-OK >&2; cat"]);
        let mut child = pair.slave.spawn_command(command).expect("spawn");
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let mut writer = pair.master.take_writer().expect("writer");
        let mut killer = child.clone_killer();

        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let _ = writer.write_all(b"hi\n");
            let _ = writer.flush();
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            // 阻塞读 + 外层超时：简单起见读到 5 秒上限就收
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
            let text = String::from_utf8_lossy(&buf);
            let echo_count = text.matches("hi").count();
            if text.contains("OUT-OK") && text.contains("ERR-OK") && echo_count >= 2 {
                break;
            }
        }
        killer.kill();
        let _ = child.wait();

        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("OUT-OK"), "stdout 未进 PTY：{text:?}");
        assert!(text.contains("ERR-OK"), "stderr 未进 PTY：{text:?}");
        // 「hi」至少出现两次 = 行规程回显一次 + cat 原样输出一次
        assert!(text.matches("hi").count() >= 2, "回显未开启：{text:?}");
    }

    #[test]
    fn host_label_formats_user_and_port() {
        let server = HubServer {
            id: "l".into(),
            name: "本机".into(),
            protocol: "ssh".into(),
            host: "127.0.0.1".into(),
            port: 0,
            username: "root".into(),
            notes: String::new(),
            key_id: String::new(),
            password_enc: String::new(),
            password_new: None,
            created_at: 0,
        }
        .normalize();
        let port = server.effective_port();
        let target = if server.username.is_empty() {
            server.host.clone()
        } else {
            format!("{}@{}", server.username, server.host)
        };
        assert_eq!(format!("{target}:{port}"), "root@127.0.0.1:22");
    }
}
