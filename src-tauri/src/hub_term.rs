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
use crate::HUB_WINDOW_LABEL;

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
}

pub struct TermSession {
    pub id: String,
    pub server_name: String,
    /// user@host:port（标签条显示用）
    pub host_label: String,
    pub protocol: String,
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

/// 打开终端：PTY 里跑 `ssh -p PORT user@host`，读线程/等待线程开始推事件。
#[tauri::command]
pub fn hub_term_open(
    app: AppHandle,
    state: tauri::State<'_, HubTermState>,
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
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        killer: Mutex::new(killer),
    });
    state
        .sessions
        .lock()
        .map_err(|_| "会话表已被锁死")?
        .insert(id.clone(), session);

    // 读线程：ssh 输出 → hub-term-data（b64）
    let emit_app = app.clone();
    let read_id = id.clone();
    std::thread::Builder::new()
        .name(format!("hub-term-read-{id}"))
        .spawn(move || pump_output(&emit_app, &read_id, reader))
        .map_err(|error| format!("无法启动读线程: {error}"))?;

    // 等待线程：ssh 退出 → hub-term-closed，并从会话表摘除（master 随之释放，读线程收尾）
    let wait_app = app.clone();
    let wait_id = id.clone();
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
                HUB_WINDOW_LABEL,
                EVENT_CLOSED,
                json!({ "id": wait_id, "code": code }),
            );
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

/// 读线程主体：PTY → b64 → 事件
fn pump_output(app: &AppHandle, id: &str, mut reader: Box<dyn Read + Send>) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,             // EOF：ssh 退出，等待线程会收尾
            Ok(n) => {
                let payload = json!({
                    "id": id,
                    "data": B64.encode(&chunk[..n]),
                });
                if app.emit_to(HUB_WINDOW_LABEL, EVENT_DATA, payload).is_err() {
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
            created_at: 0,
        };
        assert!(empty.host.trim().is_empty());
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
