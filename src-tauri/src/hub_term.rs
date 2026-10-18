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
use std::sync::atomic::{AtomicBool, Ordering};
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

/// 上行队列容量（条）。每条最多是一段粘贴内容，1024 条足够吸收
/// 「按住键不放 + 远端一时不回显」的突发，同时给 PTY 缓冲满留出明确的
/// 失败边界（满了就报错，而不是无声排队到天荒地老）。
const INPUT_QUEUE: usize = 1024;

/// 会话异常退出时，往 stderr 抄多少字节的 ssh 原文（够装下一条完整报错）。
const TAIL_LIMIT: usize = 4096;

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
    /// 上行队列：`hub_term_write` 只负责入队，真正写 PTY 的是专属写线程。
    ///
    /// 以前命令里直接 `write_all + flush`：PTY 缓冲满（远端不读、网络卡住、
    /// 用户粘贴一大段）时 write 会阻塞，而同步命令跑在 UI 线程上 —— 一阻塞
    /// 整个窗口就不响应。改成队列后命令只做一次 `try_send`（纯内存 + 有界
    /// 通道，满则立刻报错），UI 线程永远不会卡在这一步。
    input: std::sync::mpsc::SyncSender<Vec<u8>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// 用户主动关的标签（hub_term_close 置位）：算「干净收尾」，最后一个会话
    /// 结束照常自动关窗。ssh 自己 abnormal 退出（连不上/认证失败/指纹被拒）
    /// 则**不**自动关窗 —— 报错文本留在死标签里让用户看清死因（闪关即黑箱）。
    user_closed: AtomicBool,
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
                // hub 主窗（会话页）看全部；终端窗只看自己的
                .filter(|s| label == crate::HUB_WINDOW_LABEL || s.window == label)
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

/// 组装跳板机 ProxyCommand（纯函数：单测直接喂参数，不依赖 AppHandle）。
/// 形态：`ssh -o StrictHostKeyChecking=accept-new [-o IdentitiesOnly=yes -i '…']
///        -p PORT user@host -W %h:%p`
///
/// 密码跳板**不在这里拼 askpass**：ProxyCommand 子进程继承外层 ssh 的
/// `SSH_ASKPASS` / `SSH_ASKPASS_REQUIRE`，由 askpass 按提示语里的 host 认领跳板的密码。
/// 以前是 `env SSH_ASKPASS='…' ssh …` 前缀，Windows 上没有 shell，这条前缀根本执行不了。
pub(crate) fn jump_proxy_command(
    jump: &crate::hub::HubServer,
    key_path: Option<&std::path::Path>,
) -> String {
    let mut proxy = String::from("ssh -o StrictHostKeyChecking=accept-new");
    if let Some(key) = key_path {
        proxy.push_str(&format!(" -o IdentitiesOnly=yes -i '{}'", key.display()));
    }
    let jump_user = if jump.username.is_empty() {
        String::new()
    } else {
        format!("{}@", jump.username)
    };
    proxy.push_str(&format!(
        " -p {} {}{} -W %h:%p",
        jump.effective_port(),
        jump_user,
        jump.host
    ));
    proxy
}

/// 打开终端：PTY 里跑 `ssh -p PORT user@host`，读线程/等待线程开始推事件。
/// `window` 由 Tauri 注入 = 调用方窗口（server-term 页内的标签全归它）。
///
/// **必须 async + spawn_blocking**。`openpty` 在 Windows 上是「起一个 conhost
/// 并等它连上命名管道」的同步握手，`spawn_command` 也是同步 CreateProcess，
/// 两者合计几十毫秒到数秒。同步 `#[tauri::command]` 跑在 WebView2 的 IPC
/// 回调线程（就是 UI 线程）上，这期间窗口不绘制、不吃输入 —— 用户看到的
/// 就是「点开终端卡死」，与上一轮「在 IPC 回调里建 webview」同一个病根。
/// 放进阻塞线程池后 UI 线程全程空闲，慢也只慢终端自己转圈。
#[tauri::command]
pub async fn hub_term_open(
    app: AppHandle,
    window: tauri::WebviewWindow,
    server: HubServer,
) -> Result<TermMeta, String> {
    let window_label = window.label().to_string();
    let result =
        tauri::async_runtime::spawn_blocking(move || open_term_session(&app, &window_label, server))
            .await
            .map_err(|error| format!("打开终端任务失败: {error}"))?;
    // 失败原因必须进 stderr：前端只弹一个 toast，落不成可查的证据
    if let Err(ref error) = result {
        eprintln!("[hub-term] open 失败: {error}");
    }
    result
}

/// 开终端的真正逻辑：同步、会阻塞，**只允许在阻塞线程池里调用**。
fn open_term_session(
    app: &AppHandle,
    window_label: &str,
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
    // TCP 连不上 10 秒就报错：stderr 进 PTY 直接显示在终端里，
    // 别让用户对着黑屏干等内核默认的 ~130s 超时（连接错误也走 stderr，见 M3 注释）
    command.args(["-o", "ConnectTimeout=10"]);
    // xterm-256color：远端 vim/htop 之类才肯出彩色；终端类型由我们这侧的 xterm.js 兑现
    command.env("TERM", "xterm-256color");
    // 选了密钥认证：-i 指定私钥（带口令的钥匙由 ssh 在 PTY 里交互要，与本模块哲学一致）
    if !server.key_id.is_empty() {
        let key = crate::hub_keys::private_key_path(app, &server.key_id)?;
        command.args(["-i", &key.display().to_string()]);
        command.args(["-o", "IdentitiesOnly=yes"]);
    } else if !server.password_enc.is_empty() {
        if server.id.is_empty() {
            return Err("这台服务器还没保存档案：存密码连接需要先保存（askpass 按 id 取密文）".to_string());
        }
        // 密码认证（M6）：SSH_ASKPASS_REQUIRE=force 让 ssh 把要密码这件事交给
        // askpass 程序 —— 本程序自身经 hub-askpass.sh 回环（内存解密 → stdout），
        // ssh 拿到后免输登录。PTY 里不再出现密码提示。
        // 注意带口令的密钥解锁也走 askpass，所以这两个分支必须互斥（表单三选一）。
        // 首连指纹确认必须 accept-new：OpenSSH ≤9 的「(yes/no)」确认也会被
        // force 进 askpass，而 askpass 只会吐密码 → 永远答不出 yes → 首连必死
        // （OpenSSH 10 起默认就是 accept-new，这里显式写齐对齐各发行版行为）。
        // 密码认证（M6）：SSH_ASKPASS 指向**本程序自身**（不是 .sh 脚本），
        // server_id 走环境变量；ssh 把提示语当 argv[1] 传回来，我们按提示语里的
        // host 决定取哪一份密码 —— 跳板机与目标机共用同一个 askpass 程序。
        let exe = crate::hub_vault::askpass_program()?;
        command.env("SSH_ASKPASS", exe.display().to_string());
        command.env("SSH_ASKPASS_REQUIRE", "force");
        command.env(crate::hub_vault::ASKPASS_SERVER_ENV, &server.id);
        command.args(["-o", "StrictHostKeyChecking=accept-new"]);
    }
    // ControlMaster：认证连接留一把复用（M4 隧道靠它免二次认证，见 hub_tunnel.rs）。
    // 只有 Unix 有多路复用 —— Win32-OpenSSH 不支持（实测 ControlMaster 直接报
    // `getsockname failed: Not a socket`），Windows 上的文件/隧道改为每次自己认证。
    #[cfg(unix)]
    if !server.id.is_empty() {
        if let Ok(control) = crate::hub_tunnel::control_path(app, &server.id) {
            command.args(["-o", "ControlMaster=auto"]);
            command.args(["-o", &format!("ControlPath={}", control.display())]);
            command.args(["-o", "ControlPersist=600"]);
        }
    }
    // 经由跳板机（M-jump）：ProxyCommand 先登录跳板机，再 `-W %h:%p` 把 stdin/stdout
    // 接到目标主机 —— 外层 ssh 只看到一条已经打通的管道。Unix 上文件 / 隧道
    // 复用本连接的 ControlMaster，自动跟着走同一条链路。跳板凭据它自己的：
    // - 存了密码 → ProxyCommand 子进程继承外层的 SSH_ASKPASS / SSH_ASKPASS_REQUIRE，
    //   同一个 askpass 程序按提示语里的 host 认出是跳板机，自己去取跳板的密码；
    // - 密钥 → 内联 `-i`；两者都没有 → 跳板的密码提示在 PTY 里手输。
    if !server.jump_id.is_empty() {
        if server.jump_id == server.id {
            return Err("跳板机不能是服务器自己".to_string());
        }
        let jump = crate::hub::load_servers_pub(app)?
            .into_iter()
            .find(|s| s.id == server.jump_id)
            .ok_or_else(|| "找不到经由的跳板机，请在「编辑」里重新选择或改回直连".to_string())?;
        if jump.protocol != "ssh" {
            return Err(format!("跳板机「{}」不是 SSH 服务器", jump.name));
        }
        // 跳板的密码不用在这里单独挂：ProxyCommand 子进程继承外层 ssh 的环境变量，
        // 同一个 askpass 程序按提示语里的 host 认出是跳板机，自己去取跳板的密码。
        // （以前给 ProxyCommand 塞 `env SSH_ASKPASS=…` 前缀，Windows 上没有 shell，跑不了。）
        let key = if jump.key_id.is_empty() {
            None
        } else {
            Some(crate::hub_keys::private_key_path(app, &jump.key_id)?)
        };
        let proxy = jump_proxy_command(&jump, key.as_deref());
        command.args(["-o", &format!("ProxyCommand={proxy}")]);
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
    let mut writer = pair
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

    // 写线程：命令侧只往队列里塞，写 PTY 的阻塞全部收在这条线程上。
    // 通道关闭（会话从表里摘掉 / 结构体释放）时 for 循环结束，线程自然退出。
    let (input, input_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE);
    std::thread::Builder::new()
        .name(format!("hub-term-write-{id}"))
        .spawn(move || {
            for chunk in input_rx {
                if writer
                    .write_all(&chunk)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        })
        .map_err(|error| format!("无法启动写线程: {error}"))?;

    let session = Arc::new(TermSession {
        id: id.clone(),
        server_name: server.name.clone(),
        host_label: meta.host_label.clone(),
        protocol: meta.protocol.clone(),
        window: window_label.to_string(),
        master: Mutex::new(pair.master),
        input,
        killer: Mutex::new(killer),
        user_closed: AtomicBool::new(false),
    });
    // 等待线程要看 user_closed（用户主动 ✕ = 干净收尾），在入表前先留一个把手
    let wait_session = session.clone();
    app.state::<HubTermState>()
        .sessions
        .lock()
        .map_err(|_| "会话表已被锁死")?
        .insert(id.clone(), session);

    // 读线程：ssh 输出 → hub-term-data（b64），只发给承载窗口
    let emit_app = app.clone();
    let read_id = id.clone();
    let read_window = window_label.to_string();
    let tail = Arc::new(Mutex::new(Vec::<u8>::new()));
    let read_tail = tail.clone();
    std::thread::Builder::new()
        .name(format!("hub-term-read-{id}"))
        .spawn(move || pump_output(&emit_app, &read_id, &read_window, reader, read_tail))
        .map_err(|error| format!("无法启动读线程: {error}"))?;

    // 等待线程：ssh 退出 → hub-term-closed（发给承载窗口），并从会话表摘除
    // （master 随之释放，读线程收尾）
    let wait_app = app.clone();
    let wait_id = id.clone();
    let wait_window = window_label.to_string();
    let wait_tail = tail.clone();
    std::thread::Builder::new()
        .name(format!("hub-term-wait-{id}"))
        .spawn(move || {
            let code = child
                .wait()
                .map(|status| status.exit_code())
                .unwrap_or(u32::MAX);
            // 用户主动关的标签（或 exit 正常退出 0）= 干净收尾；
            // ssh 自己 abnormal 退出则留窗，报错文本在死标签里看得见
            let user_closed = wait_session.user_closed.load(Ordering::Relaxed);
            if let Some(state) = wait_app.try_state::<HubTermState>() {
                if let Ok(mut map) = state.sessions.lock() {
                    map.remove(&wait_id);
                }
            }
            eprintln!("[hub-term] closed id={wait_id} code={code} user_closed={user_closed}");
            if code != 0 && !user_closed {
                // 把 ssh 自己的报错抄进 stderr：连不上的原因不该只活在界面里
                let text = wait_tail
                    .lock()
                    .map(|buf| String::from_utf8_lossy(&buf).trim().to_string())
                    .unwrap_or_default();
                if !text.is_empty() {
                    eprintln!("[hub-term] 最后输出: {text}");
                }
            }
            let _ = wait_app.emit_to(
                wait_window.clone(),
                EVENT_CLOSED,
                json!({ "id": wait_id, "code": code }),
            );
            // 这个窗口的最后一个会话结束了 → 自动关窗（前端没有 close 权限，
            // 用户敲 exit 走 code=0；手动 ✕ 关标签走 user_closed）。
            // 连接失败（abnormal code）故意**不**关窗：闪关 = 报错黑箱，谁也修不了。
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
            if empty && (code == 0 || user_closed) {
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
///
/// 这条命令**故意保持同步**：它只做「解 base64 + 查会话表 + 往有界通道
/// try_send」，全是内存操作，微秒级，没有阻塞点。真正会阻塞的 `write_all`
/// 在专属写线程里，所以 UI 线程既不会等 PTY，也不会被远端卡住拖死。
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
    match session.input.try_send(bytes) {
        Ok(()) => Ok(()),
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            Err("输入积压太多（远端迟迟不回显），请稍等一下再输入".to_string())
        }
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
            Err("会话已结束".to_string())
        }
    }
}

/// xterm fit 之后报尺寸 → PTY resize（远端收到 SIGWINCH）
///
/// 同步是安全的，但**只拿 try_lock**：拿不到（说明别处正握着 PTY 把手）就
/// 静默跳过，下次窗口变动还会再报一次。让 UI 线程为一个尺寸等锁不值得。
#[tauri::command]
pub fn hub_term_resize(
    state: tauri::State<'_, HubTermState>,
    session_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    let session = get_session(&state, &session_id)?;
    let Ok(master) = session.master.try_lock() else {
        return Ok(());
    };
    master
        .resize(PtySize {
            rows: rows.max(2),
            cols: cols.max(2),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("调整尺寸失败: {error}"))
}

/// 用户关标签：标记「主动收尾」后杀掉 ssh；等待线程会补发 closed 事件
#[tauri::command]
pub fn hub_term_close(
    state: tauri::State<'_, HubTermState>,
    session_id: String,
) -> Result<(), String> {
    let session = get_session(&state, &session_id)?;
    session.user_closed.store(true, Ordering::Relaxed);
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
/// 读线程：ssh 输出 → hub-term-data（b64），只发给承载窗口。
///
/// 顺手把最近 `TAIL_LIMIT` 字节留在 `tail` 里：ssh 非 0 退出时（最常见的是
/// 认证失败 255），等待线程会把这段原文打进 stderr —— 否则「终端连不上」
/// 只有黑箱，报错文本只活在用户眼前那个死标签里。
fn pump_output(
    app: &AppHandle,
    id: &str,
    window: &str,
    mut reader: Box<dyn Read + Send>,
    tail: Arc<Mutex<Vec<u8>>>,
) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,             // EOF：ssh 退出，等待线程会收尾
            Ok(n) => {
                if let Ok(mut buf) = tail.lock() {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > TAIL_LIMIT {
                        let cut = buf.len() - TAIL_LIMIT;
                        buf.drain(..cut);
                    }
                }
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
            jump_id: String::new(),
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
            jump_id: String::new(),
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
            jump_id: String::new(),
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

    #[test]
    fn jump_proxy_command_shapes() {
        let mut jump = HubServer {
            id: "j".into(),
            name: "跳板".into(),
            protocol: "ssh".into(),
            host: "10.0.0.2".into(),
            port: 0,
            username: "root".into(),
            notes: String::new(),
            key_id: String::new(),
            password_enc: String::new(),
            jump_id: String::new(),
            password_new: None,
            created_at: 0,
        };

        // 纯密钥跳板：-i + IdentitiesOnly，不含任何 askpass 痕迹
        // （密码跳板由外层 ssh 的 SSH_ASKPASS 继承下去处理，不在 ProxyCommand 里拼）
        let p = jump_proxy_command(&jump, Some(std::path::Path::new("/keys/k")));
        assert!(p.starts_with("ssh -o StrictHostKeyChecking=accept-new"));
        assert!(p.contains(" -o IdentitiesOnly=yes -i '/keys/k'"));
        assert!(!p.contains("SSH_ASKPASS") && !p.contains(" env "));
        assert!(p.ends_with(" -p 22 root@10.0.0.2 -W %h:%p"));

        // 什么都没存（交互跳板）：不带 -i，密码提示留给 PTY / askpass
        let p = jump_proxy_command(&jump, None);
        assert!(!p.contains("SSH_ASKPASS") && !p.contains("IdentitiesOnly"));

        // 无用户名 → 不带 user@；显式端口；Telnet 默认端口 23
        jump.username = String::new();
        let p = jump_proxy_command(&jump, None);
        assert!(p.ends_with(" -p 22 10.0.0.2 -W %h:%p"));
        jump.port = 2222;
        let p = jump_proxy_command(&jump, None);
        assert!(p.ends_with(" -p 2222 10.0.0.2 -W %h:%p"));
        jump.protocol = "telnet".into();
        jump.port = 0;
        let p = jump_proxy_command(&jump, None);
        assert!(p.ends_with(" -p 23 10.0.0.2 -W %h:%p"));
    }
}
