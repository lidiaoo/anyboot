//! server-hub M4：SSH 隧道（本地转发 / SOCKS5 动态转发）。
//!
//! 认证定则与终端同源：**hub 不碰凭据**，只把凭据挂到 ssh 进程上
//! （`apply_auth`）：
//! - 密钥 → `-i`；
//! - 存档密码 → `SSH_ASKPASS` 指向本程序自身（提示语里的 host 决定取哪份密码）；
//! - Unix 额外用 ControlPath 复用终端建好的 ControlMaster（没有就先拨一次）；
//! - 非 Unix（Windows）**没有多路复用**（Win32-OpenSSH 实测不支持），每次自己认证。
//! 没存凭据时给明确报错，而不是无声挂死。

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::hub::HubServer;
use crate::HUB_WINDOW_LABEL;

/// 隧道档案文件名
pub(crate) const HUB_TUNNELS_FILE: &str = "hub-tunnels.json";

/// 事件：隧道进程退出
const EVENT_STOPPED: &str = "hub-tunnel-stopped";

// ==================== 档案 ====================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HubTunnel {
    pub id: String,
    /// 关联的服务器（hub-servers.json 里的 id）
    pub server_id: String,
    pub name: String,
    /// "local" = 本地转发 | "dynamic" = SOCKS5 动态转发
    pub kind: String,
    /// 本地监听端口
    pub local_port: u16,
    /// local 转发的目标主机（如 127.0.0.1 / 内网 IP）
    #[serde(default)]
    pub target_host: String,
    /// local 转发的目标端口
    #[serde(default)]
    pub target_port: u16,
    #[serde(default)]
    pub created_at: u64,
}

impl HubTunnel {
    pub(crate) fn normalize(mut self) -> Self {
        self.id = self.id.trim().to_string();
        self.server_id = self.server_id.trim().to_string();
        self.name = self.name.trim().to_string();
        self.kind = match self.kind.as_str() {
            "dynamic" => "dynamic".to_string(),
            _ => "local".to_string(),
        };
        self.target_host = self.target_host.trim().to_string();
        self
    }

    fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("名称不能为空".to_string());
        }
        if self.server_id.is_empty() {
            return Err("必须关联一台服务器".to_string());
        }
        if self.local_port == 0 {
            return Err("本地端口不能为空".to_string());
        }
        if self.kind == "local" {
            if self.target_host.is_empty() {
                return Err("目标主机不能为空".to_string());
            }
            if self.target_port == 0 {
                return Err("目标端口不能为空".to_string());
            }
        }
        Ok(())
    }

    /// ssh 的转发参数（-L / -D）
    pub(crate) fn forward_arg(&self) -> String {
        match self.kind.as_str() {
            "dynamic" => format!("127.0.0.1:{}", self.local_port),
            _ => format!(
                "127.0.0.1:{}:{}:{}",
                self.local_port, self.target_host, self.target_port
            ),
        }
    }
}

fn tunnels_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::config_dir(app)?.join(HUB_TUNNELS_FILE))
}

fn load_tunnels_from(path: &std::path::Path) -> Vec<HubTunnel> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&content).unwrap_or_else(|error| {
        eprintln!("hub-tunnels.json 解析失败（按空表处理）: {error}");
        Vec::new()
    })
}

fn save_tunnels_to(path: &std::path::Path, tunnels: &[HubTunnel]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建配置目录: {error}"))?;
    }
    let temporary = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(tunnels)
        .map_err(|error| format!("序列化失败: {error}"))?;
    std::fs::write(&temporary, content.as_bytes())
        .map_err(|error| format!("无法写入 {temporary:?}: {error}"))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("无法落盘 {path:?}: {error}"))?;
    Ok(())
}

fn load_tunnels(app: &AppHandle) -> Result<Vec<HubTunnel>, String> {
    Ok(load_tunnels_from(&tunnels_path(app)?))
}

fn save_tunnels(app: &AppHandle, tunnels: &[HubTunnel]) -> Result<(), String> {
    save_tunnels_to(&tunnels_path(app)?, tunnels)
}

fn new_tunnel_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("tun-{nanos:016x}")
}

/// 终端 / 隧道共用的 ControlMaster 套接字路径（按服务器 id 一把锁）。
///
/// **只有 Unix 有多路复用**：Win32-OpenSSH 明确不支持（实测带
/// `ControlMaster=yes + ControlPath=…` 直接 `getsockname failed: Not a socket`，
/// 套接字文件根本不落地）。所以这套机制整个按 cfg 门控，Windows 走「每次自己认证」。
#[cfg(unix)]
pub(crate) fn control_path(app: &AppHandle, server_id: &str) -> Result<PathBuf, String> {
    let path = crate::config_dir(app)?.join(control_socket_name(server_id));
    // Unix domain socket 的 sun_path 上限：macOS 104 / Linux 108 字节，ssh 绑定时
    // 还会临时加 `.XXXXXXXXXXXXXXXX` 随机后缀（实测 17 字节）。超限就让 ssh 在
    // 深处报一句 `unix_listener: path "…" too long`（macOS 实测），这里提前按
    // 同一余量拦下，把「配置目录太深」这个根因写进报错。
    const SUN_PATH_MAX: usize = if cfg!(target_os = "macos") { 104 } else { 108 };
    const MKTEMP_SUFFIX: usize = 17;
    let rendered = path.display().to_string();
    if rendered.len() + MKTEMP_SUFFIX > SUN_PATH_MAX {
        return Err(format!(
            "连接复用套接字路径过长（{rendered}）：配置目录太深，无法建立 ControlMaster"
        ));
    }
    Ok(path)
}

/// 套接字文件名：server id 的短哈希（固定种子的 SipHash，同 id 恒同名）。
/// **12 个 hex（48 位）就够**：macOS 配置目录本身 ~62 字节，16 hex 会把
/// control_path 顶过 sun_path 上限；12 hex 留出 ssh 临时后缀后还有余量。
#[cfg(unix)]
fn control_socket_name(server_id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    server_id.hash(&mut hasher);
    // 取低 48 位（12 hex）：u64 全宽有 16 位，会把文件名顶过预算
    format!("hub-cm-{:012x}", hasher.finish() & 0xffff_ffff_ffff)
}

/// `-o` 用的 `ControlPath=…` 选项串：路径**必须双引号包住**。
/// macOS 配置目录是 `~/Library/Application Support/…`，带空格 —— 裸拼的话
/// ssh 把空格后的部分当成多余参数，直接拒连：
/// `command-line line 0: keyword controlpath extra arguments at end of line`
/// （Linux 的 `~/.config/anyboot` 没空格，所以一直没暴露）。
/// ssh 配置行支持双引号包带空格的参数（实测 `ssh -G` 解析正常）。
#[cfg(unix)]
pub(crate) fn control_path_option(control: &std::path::Path) -> String {
    format!("ControlPath=\"{}\"", control.display())
}

// ==================== 认证 ====================

/// 把「凭据」挂到一条 ssh / sftp 命令上（只挂凭据：密钥或 askpass；不含跳板、不含复用）。
/// 两者都没有时什么都不挂 —— 交互认证交给调用方的 PTY。
fn apply_credentials(
    app: &AppHandle,
    server: &HubServer,
    command: &mut std::process::Command,
) -> Result<(), String> {
    if !server.key_id.is_empty() {
        let key = crate::hub_keys::private_key_path(app, &server.key_id)?;
        command
            .args(["-o", "IdentitiesOnly=yes"])
            .arg("-i")
            .arg(key.display().to_string());
    } else if !server.password_enc.is_empty() {
        // askpass 指向**本程序自身**：ssh 把提示语当 argv[1] 传回来，
        // server_id 走环境变量；跳板机的提示语由同一个程序按 host 认领。
        crate::hub_vault::apply_askpass(command, &server.id)?;
    }
    Ok(())
}

/// 跳板机 ProxyCommand（目标机与跳板机共用一段形态）。
/// 密码跳板不在这里拼 askpass：ProxyCommand 子进程继承外层 ssh 的
/// `SSH_ASKPASS` / `SSH_ASKPASS_REQUIRE`，由 askpass 按提示语里的 host 认领。
fn apply_jump_proxy(
    app: &AppHandle,
    server: &HubServer,
    command: &mut std::process::Command,
) -> Result<(), String> {
    if server.jump_id.is_empty() {
        return Ok(());
    }
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
    if jump.key_id.is_empty() && jump.password_enc.is_empty() {
        return Err(format!(
            "跳板机「{}」没有保存凭据，无法自动认证：先对它开一次终端完成认证",
            jump.name
        ));
    }
    let key = if jump.key_id.is_empty() {
        None
    } else {
        Some(crate::hub_keys::private_key_path(app, &jump.key_id)?)
    };
    command.args([
        "-o",
        &format!(
            "ProxyCommand={}",
            crate::hub_term::jump_proxy_command(&jump, key.as_deref())
        ),
    ]);
    Ok(())
}

/// 给「非交互 ssh / sftp」命令挂齐认证：凭据 + 跳板 + 复用策略。
/// 文件浏览、上传下载、远端复制、隧道全走这里，免得每处各写一套。
///
/// - 凭据：密钥（-i）或存档密码（askpass），都没有 → 原样抛出 `no_cred_message`；
/// - 跳板：统一 ProxyCommand；
/// - 复用：Unix 走 ControlMaster（终端建过就直接白用，没建就先拨一次，随后
///   `BatchMode=yes` 快进快出）；非 Unix 没有复用，只有密钥才敢开 `BatchMode`
///   （`BatchMode=yes` 会把 askpass 一起禁掉，密码认证必须留 `BatchMode=no`）。
pub(crate) fn apply_auth(
    app: &AppHandle,
    server: &HubServer,
    command: &mut std::process::Command,
    no_cred_message: &str,
) -> Result<(), String> {
    if server.key_id.is_empty() && server.password_enc.is_empty() {
        return Err(no_cred_message.to_string());
    }
    apply_credentials(app, server, command)?;
    apply_jump_proxy(app, server, command)?;

    #[cfg(unix)]
    {
        let control = control_path(app, &server.id)?;
        ensure_control(app, server, &control, no_cred_message)?;
        command
            .args(["-o", "BatchMode=yes"])
            .arg("-o")
            .arg(control_path_option(&control));
    }
    #[cfg(not(unix))]
    {
        if !server.key_id.is_empty() {
            command.args(["-o", "BatchMode=yes"]);
        } else {
            // 密码认证**必须显式**关掉 BatchMode：默认值虽然是 no，但 `sftp -b`
            // 会在内部把它打开，那时 askpass 一起被禁 → 只剩
            // `No more authentication methods to try`（Windows 实测）。
            command.args(["-o", "BatchMode=no"]);
        }
    }
    Ok(())
}

// ==================== 认证连接自建（仅 Unix：Windows 没有多路复用） ====================

/// master 不在时自动拨通认证连接（文件浏览 / 隧道 / 跳板转发共用，免先开
/// 终端）：与终端（hub_term_open）同款的认证参数跑一次 `ssh … exit`，认证
/// 成功后 `ControlMaster=auto + ControlPersist=600` 把 master 留在后台，
/// 后续 ssh/sftp 子进程照旧 BatchMode 复用 —— 每台服务器至多拨一次。
/// `no_cred_message`：没存凭据（只在终端里手输过密码）确实无法非交互
/// 认证，把调用方原有的报错文案原样抛出。
#[cfg(unix)]
pub(crate) fn ensure_control(
    app: &AppHandle,
    server: &HubServer,
    control: &std::path::Path,
    no_cred_message: &str,
) -> Result<(), String> {
    if control.exists() {
        return Ok(());
    }
    if server.key_id.is_empty() && server.password_enc.is_empty() {
        return Err(no_cred_message.to_string());
    }
    let target = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };
    let mut command = crate::command("ssh");
    command
        .args(["-o", "ControlMaster=auto"])
        .arg("-o")
        .arg(control_path_option(control))
        .args(["-o", "ControlPersist=600"])
        .args(["-o", "StrictHostKeyChecking=accept-new"])
        .args(["-o", "ConnectTimeout=10"]);
    apply_credentials(app, server, &mut command)?;
    apply_jump_proxy(app, server, &mut command)?;
    command
        .arg("-p")
        .arg(server.effective_port().to_string())
        .arg(&target)
        .arg("exit"); // 远端跑 exit：拨通即走，master 由 ControlPersist 留下
    command.env("LC_ALL", "C.UTF-8");
    let output = command
        .output()
        .map_err(|error| format!("无法启动 ssh: {error}"))?;
    if output.status.success() {
        eprintln!("[hub-cm] master established for {}", server.id);
        Ok(())
    } else {
        let tail = crate::hub_sftp::stderr_tail(&output.stderr);
        Err(if tail.is_empty() {
            format!(
                "自动认证失败（退出码 {:?}），可先开一次终端手动认证",
                output.status.code()
            )
        } else {
            format!("自动认证失败：{tail}")
        })
    }
}

// ==================== 运行时 ====================

#[derive(Default)]
pub struct HubTunnelState {
    /// key = tunnel id
    pub processes: Mutex<HashMap<String, Arc<TunnelProc>>>,
}

pub struct TunnelProc {
    pub id: String,
    /// 轮询 try_wait 与 kill 共用一把锁（各自只锁一瞬）
    child: Mutex<std::process::Child>,
    /// ssh 的 stderr 尾巴（失败时报给前端）
    stderr_tail: Arc<Mutex<String>>,
}

#[derive(Clone, Serialize)]
pub struct TunnelStatus {
    pub id: String,
    pub pid: u32,
}

fn stderr_tail_of(tail: &Arc<Mutex<String>>) -> String {
    tail.lock().map(|t| t.clone()).unwrap_or_default()
}

fn watch_stderr(mut stderr: std::process::ChildStderr, tail: Arc<Mutex<String>>) {
    let mut buffer = [0u8; 512];
    let mut recent: Vec<u8> = Vec::new();
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                recent.extend_from_slice(&buffer[..n]);
                if recent.len() > 800 {
                    let cut = recent.len() - 400;
                    recent.drain(..cut);
                }
            }
        }
    }
    if let Ok(mut t) = tail.lock() {
        *t = String::from_utf8_lossy(&recent).trim().to_string();
    }
}

#[tauri::command]
pub fn hub_list_tunnels(app: AppHandle) -> Result<Vec<HubTunnel>, String> {
    let mut tunnels = load_tunnels(&app)?;
    tunnels.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(tunnels)
}

/// 新增或更新隧道档案（按 id 区分）
#[tauri::command]
pub fn hub_save_tunnel(app: AppHandle, tunnel: HubTunnel) -> Result<HubTunnel, String> {
    let mut tunnel = tunnel.normalize();
    tunnel.validate()?;
    let mut tunnels = load_tunnels(&app)?;
    if tunnel.id.is_empty() {
        tunnel.id = new_tunnel_id();
        tunnel.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        tunnels.push(tunnel.clone());
    } else if let Some(existing) = tunnels.iter_mut().find(|t| t.id == tunnel.id) {
        let created_at = existing.created_at;
        *existing = tunnel.clone();
        existing.created_at = created_at;
    } else {
        return Err(format!("要编辑的隧道不存在（{}）", tunnel.id));
    }
    save_tunnels(&app, &tunnels)?;
    Ok(tunnel)
}

#[tauri::command]
pub fn hub_delete_tunnel(app: AppHandle, state: tauri::State<'_, HubTunnelState>, id: String) -> Result<(), String> {
    // 还在跑就先停
    if let Some(process) = state
        .processes
        .lock()
        .map_err(|_| "隧道进程表已被锁死")?
        .get(&id)
        .cloned()
    {
        if let Ok(mut child) = process.child.lock() {
            let _ = child.kill();
        }
    }
    let mut tunnels = load_tunnels(&app)?;
    let before = tunnels.len();
    tunnels.retain(|t| t.id != id);
    if tunnels.len() == before {
        return Err(format!("要删除的隧道不存在（{id}）"));
    }
    save_tunnels(&app, &tunnels)
}

/// 启动隧道：复用该服务器的 ControlMaster；进程活过 1.2s 视为已建立
///
/// **必须 async + spawn_blocking**：这里要起 ssh 子进程，还要**故意等 1.2 秒**
/// 观察它有没有立刻死掉。同步命令跑在 UI 线程上，等于用户每点一次「启动隧道」
/// 界面就硬化一秒多（ssh 握手慢时更久）。
#[tauri::command]
pub async fn hub_tunnel_start(app: AppHandle, tunnel_id: String) -> Result<TunnelStatus, String> {
    tauri::async_runtime::spawn_blocking(move || tunnel_start_blocking(&app, &tunnel_id))
        .await
        .map_err(|error| format!("启动隧道任务失败: {error}"))?
}

fn tunnel_start_blocking(app: &AppHandle, tunnel_id: &str) -> Result<TunnelStatus, String> {
    let state = app.state::<HubTunnelState>();
    {
        let map = state.processes.lock().map_err(|_| "隧道进程表已被锁死")?;
        if map.contains_key(tunnel_id) {
            return Err("该隧道已在运行".to_string());
        }
    }
    let tunnels = load_tunnels(app)?;
    let tunnel = tunnels
        .iter()
        .find(|t| t.id == tunnel_id)
        .ok_or_else(|| format!("隧道不存在（{tunnel_id}）"))?
        .clone();
    let servers = crate::hub::load_servers_for_tunnel(app)?;
    let server = servers
        .iter()
        .find(|s| s.id == tunnel.server_id)
        .ok_or_else(|| "关联的服务器已被删除，请重新选择".to_string())?
        .clone();
    spawn_tunnel(app.clone(), state.inner(), &server, tunnel)
}

fn spawn_tunnel(
    app: AppHandle,
    state: &HubTunnelState,
    server: &HubServer,
    tunnel: HubTunnel,
) -> Result<TunnelStatus, String> {
    let target = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };
    let mut command = crate::command("ssh");
    command.arg("-N").arg("-T").arg("-o").arg("ExitOnForwardFailure=yes");
    // 认证（含跳板与 Unix 的 ControlMaster 复用）统一走 apply_auth：
    // Unix 上能白用终端建好的 master；Windows 上每次自己认证。
    apply_auth(
        &app,
        server,
        &mut command,
        "这台服务器还没有保存凭据，隧道无法自动认证：请改用密钥或保存密码，或先打开一次终端完成认证",
    )?;
    if tunnel.kind == "dynamic" {
        command.arg("-D").arg(tunnel.forward_arg());
    } else {
        command.arg("-L").arg(tunnel.forward_arg());
    }
    command
        .arg("-p")
        .arg(server.effective_port().to_string())
        .arg(&target);

    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 ssh: {error}"))?;
    let pid = child.id();
    let tail = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = child.stderr.take() {
        let tail_clone = tail.clone();
        std::thread::Builder::new()
            .name(format!("hub-tunnel-err-{pid}"))
            .spawn(move || watch_stderr(stderr, tail_clone))
            .ok();
    }

    // 1.2s 内就退出 = 转发没建立（端口占用 / master 断了），把 stderr 尾巴报出来
    std::thread::sleep(Duration::from_millis(1200));
    let exited_early = child.try_wait().ok().flatten().is_some();
    if exited_early {
        let message = stderr_tail_of(&tail);
        return Err(if message.is_empty() {
            format!("隧道启动失败（ssh 立即退出，pid {pid}）")
        } else {
            format!("隧道启动失败：{message}")
        });
    }

    let process = Arc::new(TunnelProc {
        id: tunnel.id.clone(),
        child: Mutex::new(child),
        stderr_tail: tail,
    });
    state
        .processes
        .lock()
        .map_err(|_| "隧道进程表已被锁死")?
        .insert(tunnel.id.clone(), process.clone());

    // 看护线程：退出即摘表 + 发事件
    let watch_app = app.clone();
    let watch_id = tunnel.id.clone();
    let watch_proc = process.clone();
    std::thread::Builder::new()
        .name(format!("hub-tunnel-wait-{pid}"))
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(300));
            let status = {
                let Ok(mut guard) = watch_proc.child.lock() else {
                    continue;
                };
                guard.try_wait().ok().flatten()
            };
            if let Some(status) = status {
                if let Some(state) = watch_app.try_state::<HubTunnelState>() {
                    if let Ok(mut map) = state.processes.lock() {
                        map.remove(&watch_id);
                    }
                }
                let message = stderr_tail_of(&watch_proc.stderr_tail);
                eprintln!("[hub-tunnel] stopped id={watch_id} code={:?} tail={message}", status.code());
                let _ = watch_app.emit_to(
                    HUB_WINDOW_LABEL,
                    EVENT_STOPPED,
                    json!({ "id": watch_id, "tail": message }),
                );
                break;
            }
        })
        .map_err(|error| format!("无法启动看护线程: {error}"))?;

    eprintln!("[hub-tunnel] started id={} pid={pid}", tunnel.id);
    Ok(TunnelStatus { id: tunnel.id, pid })
}

/// 停止隧道（进程退出后看护线程补发 stopped 事件）
#[tauri::command]
pub fn hub_tunnel_stop(
    state: tauri::State<'_, HubTunnelState>,
    tunnel_id: String,
) -> Result<(), String> {
    let process = state
        .processes
        .lock()
        .map_err(|_| "隧道进程表已被锁死")?
        .get(&tunnel_id)
        .cloned()
        .ok_or_else(|| format!("隧道未在运行（{tunnel_id}）"))?;
    if let Ok(mut child) = process.child.lock() {
        let _ = child.kill();
    }
    Ok(())
}

/// 应用退出：收掉所有隧道进程
pub fn kill_all(app: &AppHandle) {
    let Some(state) = app.try_state::<HubTunnelState>() else {
        return;
    };
    let Ok(map) = state.processes.lock() else {
        return;
    };
    for (_, process) in map.iter() {
        if let Ok(mut child) = process.child.lock() {
            let _ = child.kill();
        }
    }
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> HubTunnel {
        HubTunnel {
            id: String::new(),
            server_id: "srv-1".to_string(),
            name: name.to_string(),
            kind: "local".to_string(),
            local_port: 8080,
            target_host: "10.0.0.2".to_string(),
            target_port: 80,
            created_at: 0,
        }
    }

    #[test]
    fn forward_arg_by_kind() {
        assert_eq!(sample("a").forward_arg(), "127.0.0.1:8080:10.0.0.2:80");
        let dynamic = HubTunnel {
            kind: "dynamic".to_string(),
            ..sample("b")
        }
        .normalize();
        assert_eq!(dynamic.forward_arg(), "127.0.0.1:8080");
    }

    /// macOS 配置目录 `~/Library/Application Support/…` 带空格：裸拼进
    /// `ControlPath=` 会被 ssh 拆成多个参数，报
    /// `keyword controlpath extra arguments at end of line`（实测）。
    /// 双引号包住后 `ssh -G` 能整段解析（macOS 实测）。
    #[cfg(unix)]
    #[test]
    fn control_path_option_quotes_spaces() {
        let option = control_path_option(std::path::Path::new(
            "/Users/x/Library/Application Support/com.example.anyboot/hub-cm-srv-1",
        ));
        assert_eq!(
            option,
            "ControlPath=\"/Users/x/Library/Application Support/com.example.anyboot/hub-cm-srv-1\""
        );
    }

    /// 套接字名必须短：macOS 配置目录 ~62 字节 + ssh 临时后缀 17 字节，
    /// 完整 server id / 16 hex 拼上去实测顶破 104 字节（unix_listener too long，
    /// 或被 control_path 的提前拦截跳过复用）。12 hex 定长，同一 id 恒同名
    /// （终端 / 隧道 / 文件三处各自推导要能对上）。
    #[cfg(unix)]
    #[test]
    fn control_socket_name_is_short_and_stable() {
        let first = control_socket_name("srv-18df82cee170ddb8");
        let second = control_socket_name("srv-18df82cee170ddb8");
        assert_eq!(first, second);
        assert_eq!(first.len(), "hub-cm-".len() + 12);
        assert_ne!(control_socket_name("srv-other"), first);
    }

    #[test]
    fn validate_rejects_incomplete() {
        assert!(sample("a").validate().is_ok());
        assert!(HubTunnel {
            name: "  ".to_string(),
            ..sample("b")
        }
        .normalize()
        .validate()
        .is_err());
        assert!(HubTunnel {
            local_port: 0,
            ..sample("c")
        }
        .normalize()
        .validate()
        .is_err());
        assert!(HubTunnel {
            target_host: String::new(),
            ..sample("d")
        }
        .normalize()
        .validate()
        .is_err());
        // dynamic 不需要 target
        let dynamic = HubTunnel {
            kind: "dynamic".to_string(),
            target_host: String::new(),
            target_port: 0,
            ..sample("e")
        }
        .normalize();
        assert!(dynamic.validate().is_ok());
    }

    #[test]
    fn store_roundtrip_and_crud() {
        let dir = std::env::temp_dir().join(format!(
            "ab-tunnel-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join(HUB_TUNNELS_FILE);
        assert!(load_tunnels_from(&path).is_empty());

        let mut a = sample("alpha");
        a.id = new_tunnel_id();
        let mut b = sample("beta");
        b.id = new_tunnel_id();
        save_tunnels_to(&path, &[a.clone(), b.clone()]).expect("落盘");
        assert_eq!(load_tunnels_from(&path).len(), 2);

        let mut tunnels = load_tunnels_from(&path);
        tunnels.retain(|t| t.id != b.id);
        save_tunnels_to(&path, &tunnels).expect("落盘");
        assert_eq!(load_tunnels_from(&path).len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_store_file_reads_as_empty() {
        let dir = std::env::temp_dir().join(format!(
            "ab-tunnel-broken-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let path = dir.join(HUB_TUNNELS_FILE);
        std::fs::write(&path, "{not json").unwrap();
        assert!(load_tunnels_from(&path).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
