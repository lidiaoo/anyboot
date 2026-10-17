//! 远程管理 · 「电脑」页：AnyBoot 服务端档案。
//!
//! 一条记录 = **一台 AnyBoot 电脑**（不是一个 IP）：多网卡候选地址列表 + token，
//! 语义对齐手机端 `ServerProfile`/`ServerStore`/`HealthProbe`：
//! - 探测协议：`GET http://addr/health`（免鉴权），响应 `{ok,service,machineId,machineName}`；
//! - 同一台电脑的多个地址靠 `machineId` 归并（本轮先存不消费，字段对齐）；
//! - `connect_all` = 手机端「启动时连接全部服务端」的全局开关。
//!
//! 与 hub（SSH 服务器）完全解耦：配置目录下自己的 JSON（`remote-servers.json`），
//! 命令只服务 hub 窗口的「电脑」页，不碰 UiState。

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// 档案文件名（放配置目录，随 `ANYBOOT_CONFIG_DIR` 走）
pub(crate) const REMOTE_SERVERS_FILE: &str = "remote-servers.json";

/// 电脑端「远程控制」的默认端口（地址不带端口时探活兜底，对齐手机 DEFAULT_PORT）
pub(crate) const DEFAULT_PORT: u16 = 12300;

/// 单次探活的连接+响应超时
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RemoteServer {
    pub id: String,
    /// 分组名（区分不同的电脑）；空 = 未分组
    #[serde(default)]
    pub group: String,
    /// 服务端名；空 = 用地址兜底
    #[serde(default)]
    pub name: String,
    /// 候选地址（有序，第 0 个是首选）。`host:port` 或整条带 token 的 URL（保存时归一）
    #[serde(default)]
    pub addresses: Vec<String>,
    /// 访问令牌；空 = 服务端未设 token
    #[serde(default)]
    pub token: String,
    /// `/health` 给的机器标识（同一台电脑多地址归并用）；未知为空
    #[serde(default)]
    pub machine_id: String,
    /// `/health` 给的机器名；未知为空
    #[serde(default)]
    pub machine_name: String,
    /// 多网卡探测：parallel（默认，并发取最快）/ serial（按顺序取第一个通）
    #[serde(default)]
    pub probe_mode: String,
    /// 经由哪台跳板机（hub SSH 服务器 id）；本轮仅存储校验，转发链路后续接入
    #[serde(default)]
    pub jump_id: String,
    #[serde(default)]
    pub created_at: u64,
}

/// 落盘结构：档案列表 + 全局开关
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct RemoteStore {
    #[serde(default)]
    pub connect_all: bool,
    #[serde(default)]
    pub servers: Vec<RemoteServer>,
}

/// remote_test 的结果（按探测模式选中的那一条；全挂时 ok=false 带首个错误）
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RemoteTestResult {
    pub ok: bool,
    pub addr: String,
    pub ms: u64,
    pub machine_id: String,
    pub machine_name: String,
    pub error: String,
}

impl RemoteServer {
    pub(crate) fn normalize(mut self) -> Self {
        self.id = self.id.trim().to_string();
        self.group = self.group.trim().to_string();
        self.name = self.name.trim().to_string();
        self.token = self.token.trim().to_string();
        self.machine_id = self.machine_id.trim().to_string();
        self.machine_name = self.machine_name.trim().to_string();
        self.jump_id = self.jump_id.trim().to_string();
        self.probe_mode = match self.probe_mode.as_str() {
            "serial" => "serial".to_string(),
            _ => "parallel".to_string(),
        };
        // 地址逐条归一：挖 URL 形态里的 token 与裸地址；trim、去空、去重（保序）
        let mut token_from_url: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        let mut addresses = Vec::new();
        for raw in &self.addresses {
            let (address, token) = split_address(raw);
            if address.is_empty() || !seen.insert(address.clone()) {
                continue;
            }
            if token.is_some() && self.token.is_empty() && token_from_url.is_none() {
                token_from_url = token;
            }
            addresses.push(address);
        }
        if let Some(token) = token_from_url {
            self.token = token;
        }
        self.addresses = addresses;
        self
    }
}

// ==================== 纯函数（单测覆盖） ====================

/// 地址归一：接受 `host:port`、裸 `host`（探活时补默认端口）或整条
/// `http://host:port/?token=x` URL —— 剥出裸 `host:port` 与 token。
/// token 只在显式带时返回；没有就是 None。
fn split_address(raw: &str) -> (String, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    // 非URL 形态原样（trim 过）
    let lower = raw.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return (raw.to_string(), None);
    }
    let rest = &raw[if lower.starts_with("https://") { 8 } else { 7 }..];
    let (authority, query) = match rest.find(['?', '/']) {
        Some(index) => {
            let tail = &rest[index..];
            let query = match tail.find('?') {
                Some(q) => &tail[q + 1..],
                None => "",
            };
            (&rest[..index], query)
        }
        None => (rest, ""),
    };
    let authority = authority.trim().to_string();
    // 挖 token=（query 里第一个 token 参数；大小写不敏感，对齐 http.rs 的宽松风格）
    let mut token = None;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if key == "token" {
            let value = parts.next().unwrap_or("").trim().to_string();
            if !value.is_empty() {
                token = Some(value);
            }
            break;
        }
    }
    (authority, token)
}

/// 探活地址兜底默认端口：不带 `:` 的补 `:12300`（对齐手机 DEFAULT_PORT）。
/// 只看最后一个冒号后面是否全数字，避免误伤 IPv6 字面量（本地局域网场景 v4 为主）。
fn with_default_port(addr: &str) -> String {
    if addr.contains(':') {
        let port = addr.rsplit(':').next().unwrap_or("");
        if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
            return addr.to_string();
        }
    }
    format!("{addr}:{DEFAULT_PORT}")
}

/// 从 `/health` 响应体里取 machineId/machineName（解析失败按空串，探活不因它失败）
fn parse_health(body: &str) -> (String, String) {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    (text("machineId"), text("machineName"))
}

/// 对一台电脑的一个地址做 `GET /health` 探活：成功返回 (耗时毫秒, machineId, machineName)。
fn probe_health(addr: &str, timeout: Duration) -> Result<(u64, String, String), String> {
    let target = with_default_port(addr);
    let started = Instant::now();
    let mut stream = TcpStream::connect(&target)
        .map_err(|error| format!("连接 {target} 失败: {error}"))
        .and_then(|stream| {
            stream
                .set_read_timeout(Some(timeout))
                .map_err(|error| format!("设置读超时失败: {error}"))?;
            stream
                .set_write_timeout(Some(timeout))
                .map_err(|error| format!("设置写超时失败: {error}"))?;
            Ok(stream)
        })?;
    let request = format!("GET /health HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("发送请求失败: {error}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("读取响应失败: {error}"))?;
    let text = String::from_utf8_lossy(&raw);
    let body = match text.find("\r\n\r\n") {
        Some(index) => &text[index + 4..],
        None => "",
    };
    let (machine_id, machine_name) = parse_health(body);
    let ms = started.elapsed().as_millis() as u64;
    Ok((ms, machine_id, machine_name))
}

// ==================== 存储 ====================

fn store_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::config_dir(app)?.join(REMOTE_SERVERS_FILE))
}

fn load_store_from(path: &Path) -> RemoteStore {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return RemoteStore::default(), // 不存在 = 还没存过
    };
    serde_json::from_str(&content).unwrap_or_else(|error| {
        eprintln!("remote-servers.json 解析失败（按空表处理）: {error}");
        RemoteStore::default()
    })
}

fn save_store_to(path: &Path, store: &RemoteStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录: {error}"))?;
    }
    // 临时文件 + 改名：写一半被杀也不会留下半截 JSON
    let temporary = path.with_extension("json.tmp");
    let content =
        serde_json::to_string_pretty(store).map_err(|error| format!("序列化失败: {error}"))?;
    std::fs::write(&temporary, content.as_bytes())
        .map_err(|error| format!("无法写入 {temporary:?}: {error}"))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("无法落盘 {path:?}: {error}"))?;
    Ok(())
}

fn load_store(app: &AppHandle) -> Result<RemoteStore, String> {
    Ok(load_store_from(&store_path(app)?))
}

fn save_store(app: &AppHandle, store: &RemoteStore) -> Result<(), String> {
    save_store_to(&store_path(app)?, store)
}

/// 后续「启动时连接全部 / 心跳」复用（跨模块只读）
pub(crate) fn load_store_pub(app: &AppHandle) -> Result<RemoteStore, String> {
    load_store(app)
}

fn new_remote_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("rmt-{nanos:016x}")
}

// ==================== 命令 ====================

/// 「电脑」页 iframe 的本机入口：本机 AnyBoot 服务端的 http 端口与明文 token。
/// 本机标签与远端标签同一路径（`http://127.0.0.1:port/?token=…`），页面行为
/// 与手机 WebView 加载的完全一致；仅 hub 窗口本地使用，不出本机。
#[tauri::command]
pub fn local_http_endpoint(app: AppHandle) -> Result<serde_json::Value, String> {
    let settings = crate::read_settings_file(&app);
    Ok(serde_json::json!({
        "port": settings.http_port,
        "token": settings.http_token,
        "enabled": settings.http_enabled,
    }))
}

/// 电脑页初始化数据：全部档案 + 全局开关。token 明文回传（iframe 拼 URL 要用，对齐手机）。
#[tauri::command]
pub fn remote_list(app: AppHandle) -> Result<RemoteStore, String> {
    load_store(&app)
}

/// 新建 / 编辑。id 空 = 新建；否则按 id 更新（created_at 保留）。
#[tauri::command]
pub fn remote_save(app: AppHandle, server: RemoteServer) -> Result<RemoteServer, String> {
    let mut server = server.normalize();
    if server.addresses.is_empty() {
        return Err("至少要填一个服务端地址".to_string());
    }
    // 跳板机只存引用：必须是已存在的 SSH 服务器（转发链路后续接入，先保证引用有效）
    if !server.jump_id.is_empty() {
        let hub_servers = crate::hub::load_servers_pub(&app)?;
        match hub_servers.iter().find(|s| s.id == server.jump_id) {
            Some(hub) if hub.protocol == "ssh" => {}
            Some(_) => return Err("经由跳板机只支持 SSH 服务器".to_string()),
            None => return Err("所选跳板机不存在，请重新选择".to_string()),
        }
    }
    let mut store = load_store(&app)?;
    if server.id.is_empty() {
        server.id = new_remote_id();
        server.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        store.servers.push(server.clone());
    } else if let Some(existing) = store.servers.iter_mut().find(|s| s.id == server.id) {
        // 编辑态 token 留空 = 保持已存 token（前端不回显 token，同 hub 密码语义）；
        // 新建态空 token 合法（服务端可能没设 token）
        if server.token.is_empty() {
            server.token = existing.token.clone();
        }
        // 地址没变才保留探测学到的机器标识（变了可能已是另一台电脑，作废）
        let same_machine_set = existing.addresses == server.addresses;
        let machine = (existing.machine_id.clone(), existing.machine_name.clone());
        let created_at = existing.created_at;
        *existing = server.clone();
        existing.created_at = created_at;
        if same_machine_set {
            existing.machine_id = machine.0;
            existing.machine_name = machine.1;
        }
    } else {
        return Err("服务端不存在（可能已被删除）".to_string());
    }
    save_store(&app, &store)?;
    Ok(server)
}

#[tauri::command]
pub fn remote_delete(
    app: AppHandle,
    state: tauri::State<'_, RemoteJumpState>,
    id: String,
) -> Result<(), String> {
    kill_forward(state.inner(), &id);
    let mut store = load_store(&app)?;
    let before = store.servers.len();
    store.servers.retain(|s| s.id != id);
    if store.servers.len() == before {
        return Err("服务端不存在（可能已被删除）".to_string());
    }
    save_store(&app, &store)
}

/// 「✓ 启动时连接全部服务端」全局开关（对齐手机 setConnectAll）
#[tauri::command]
pub fn remote_set_connect_all(app: AppHandle, on: bool) -> Result<(), String> {
    let mut store = load_store(&app)?;
    store.connect_all = on;
    save_store(&app, &store)
}

/// 探测一台电脑：并发模式 = 所有地址同时探取最快成功；串行 = 按顺序第一个成功。
/// `GET /health` 免鉴权，token 参数仅保证签名对齐（后续健康检查可能带）。
#[tauri::command]
pub fn remote_test(addresses: Vec<String>, probe_mode: String) -> RemoteTestResult {
    let candidates: Vec<String> = addresses
        .iter()
        .filter_map(|raw| {
            let (address, _) = split_address(raw);
            if address.is_empty() {
                None
            } else {
                Some(address)
            }
        })
        .collect();
    if candidates.is_empty() {
        return RemoteTestResult {
            ok: false,
            addr: String::new(),
            ms: 0,
            machine_id: String::new(),
            machine_name: String::new(),
            error: "没有可探测的地址".to_string(),
        };
    }
    let serial = probe_mode == "serial";
    let mut best: Option<RemoteTestResult> = None;
    let mut first_error = String::new();
    if serial {
        for addr in &candidates {
            match probe_health(addr, PROBE_TIMEOUT) {
                Ok((ms, machine_id, machine_name)) => {
                    best = Some(RemoteTestResult {
                        ok: true,
                        addr: addr.clone(),
                        ms,
                        machine_id,
                        machine_name,
                        error: String::new(),
                    });
                    break;
                }
                Err(error) => {
                    if first_error.is_empty() {
                        first_error = error;
                    }
                }
            }
        }
    } else {
        // 并发探全部地址，取最快成功（对齐手机「并发探测（推荐）」）
        let handles: Vec<_> = candidates
            .iter()
            .map(|addr| {
                let addr = addr.clone();
                std::thread::spawn(move || (addr.clone(), probe_health(&addr, PROBE_TIMEOUT)))
            })
            .collect();
        let mut results = Vec::new();
        for handle in handles {
            if let Ok((addr, result)) = handle.join() {
                results.push((addr, result));
            }
        }
        for (addr, result) in results {
            match result {
                Ok((ms, machine_id, machine_name)) => {
                    let candidate = RemoteTestResult {
                        ok: true,
                        addr,
                        ms,
                        machine_id,
                        machine_name,
                        error: String::new(),
                    };
                    let better = match &best {
                        Some(current) => candidate.ms < current.ms,
                        None => true,
                    };
                    if better {
                        best = Some(candidate);
                    }
                }
                Err(error) => {
                    if first_error.is_empty() {
                        first_error = error;
                    }
                }
            }
        }
    }
    best.unwrap_or(RemoteTestResult {
        ok: false,
        addr: String::new(),
        ms: 0,
        machine_id: String::new(),
        machine_name: String::new(),
        error: if first_error.is_empty() {
            "全部地址都连不上".to_string()
        } else {
            first_error
        },
    })
}

// ==================== 经跳板机连接远端 AnyBoot ====================

/// 远端端点：直连 = 目标地址原样；经跳板 = 复用跳板机 ControlMaster 的本地转发端口。
#[derive(Clone, Debug, Serialize)]
pub struct RemoteEndpoint {
    pub host: String,
    pub port: u16,
    pub via_jump: bool,
}

/// 经跳板机的本地转发子进程（按远端档案 id 记，应用退出统一收掉）。
pub struct JumpForward {
    pub child: std::process::Child,
    pub local_port: u16,
    pub target: String,
}

/// 经跳板转发进程表
#[derive(Default)]
pub struct RemoteJumpState {
    pub forwards: std::sync::Mutex<std::collections::HashMap<String, JumpForward>>,
}

/// 地址 → 转发目标 (host, port)：URL / host:port / 裸 host 都吃，缺省端口 12300。
fn remote_target(raw: &str) -> (String, u16) {
    let (host, port) = split_address(raw);
    if let Some(p) = port.and_then(|x| x.parse::<u16>().ok()) {
        return (host, p);
    }
    // 非 URL 且自带端口（split_address 对 host:port 原样返回）
    if let Some(idx) = host.rfind(':') {
        if let Ok(p) = host[idx + 1..].parse::<u16>() {
            return (host[..idx].to_string(), p);
        }
    }
    (host, DEFAULT_PORT)
}

/// `-L` 转发参数（纯函数，单测）
fn remote_forward_arg(local_port: u16, host: &str, port: u16) -> String {
    format!("127.0.0.1:{local_port}:{host}:{port}")
}

fn kill_forward(state: &RemoteJumpState, id: &str) {
    if let Ok(mut map) = state.forwards.lock() {
        if let Some(mut f) = map.remove(id) {
            let _ = f.child.kill();
            let _ = f.child.wait();
        }
    }
}

/// 端点解析（命令体）：直连返回首个地址；经跳板确保本地转发活着并返回 127.0.0.1:port。
fn remote_endpoint_inner(
    app: &AppHandle,
    state: &RemoteJumpState,
    server_id: &str,
) -> Result<RemoteEndpoint, String> {
    let store = load_store(app)?;
    let server = store
        .servers
        .iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("服务端不存在（{server_id}）"))?
        .clone();
    let raw = server
        .addresses
        .iter()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .ok_or_else(|| "该服务端没有可连的地址".to_string())?
        .to_string();
    let (host, port) = remote_target(&raw);
    if server.jump_id.is_empty() {
        return Ok(RemoteEndpoint { host, port, via_jump: false });
    }

    // 已有活转发且目标一致 → 直接复用（地址改了就走重建）
    let target = remote_forward_arg(0, &host, port);
    if let Ok(mut map) = state.forwards.lock() {
        if let Some(f) = map.get_mut(server_id) {
            if f.target == target && f.child.try_wait().ok().flatten().is_none() {
                return Ok(RemoteEndpoint {
                    host: "127.0.0.1".into(),
                    port: f.local_port,
                    via_jump: true,
                });
            }
        }
    }
    kill_forward(state, server_id);

    let jump = crate::hub::load_servers_pub(app)?
        .into_iter()
        .find(|s| s.id == server.jump_id)
        .ok_or_else(|| "跳板机已被删除，请重新选择".to_string())?;
    if jump.protocol != "ssh" {
        return Err("跳板机必须是 SSH 服务器".to_string());
    }
    // 找一个空闲本地端口
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("拿不到空闲本地端口: {e}"))?;
    let local_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);

    let jump_user = if jump.username.is_empty() {
        String::new()
    } else {
        format!("{}@", jump.username)
    };
    let mut command = crate::command("ssh");
    command
        .arg("-N")
        .arg("-T")
        .arg("-o")
        .arg("ExitOnForwardFailure=yes");
    // 认证统一走 apply_auth：Unix 复用 ControlMaster（终端建过就直接用），
    // Windows 没有多路复用，改由每次自己认证（密钥 / askpass）。
    crate::hub_tunnel::apply_auth(
        app,
        &jump,
        &mut command,
        "跳板机还没有保存凭据，无法自动建立转发：请改用密钥或保存密码",
    )?;
    command
        .arg("-L")
        .arg(remote_forward_arg(local_port, &host, port))
        .arg("-p")
        .arg(jump.effective_port().to_string())
        .arg(format!("{jump_user}{}", jump.host));
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 ssh: {error}"))?;
    // 1.2s 内就退出 = 转发没建立（跳板连接断了等）
    std::thread::sleep(Duration::from_millis(1200));
    if let Ok(Some(status)) = child.try_wait() {
        let _ = child.wait();
        return Err(format!("跳板转发建立失败（exit {status}）"));
    }
    if let Ok(mut map) = state.forwards.lock() {
        map.insert(
            server_id.to_string(),
            JumpForward { child, local_port, target },
        );
    }
    Ok(RemoteEndpoint { host: "127.0.0.1".into(), port: local_port, via_jump: true })
}

/// 电脑页 iframe / 健康检查要连的端点。
#[tauri::command]
pub fn remote_endpoint(
    app: AppHandle,
    state: tauri::State<'_, RemoteJumpState>,
    server_id: String,
) -> Result<RemoteEndpoint, String> {
    remote_endpoint_inner(&app, state.inner(), &server_id)
}

/// 经跳板机探测：先确保本地转发，再对 127.0.0.1:port 做 /health 探测。
#[tauri::command]
pub fn remote_test_via(
    app: AppHandle,
    state: tauri::State<'_, RemoteJumpState>,
    server_id: String,
) -> RemoteTestResult {
    let endpoint = match remote_endpoint_inner(&app, state.inner(), &server_id) {
        Ok(ep) => ep,
        Err(error) => {
            return RemoteTestResult {
                ok: false,
                addr: String::new(),
                ms: 0,
                machine_id: String::new(),
                machine_name: String::new(),
                error,
            }
        }
    };
    match probe_health(&format!("{}:{}", endpoint.host, endpoint.port), PROBE_TIMEOUT) {
        Ok((ms, machine_id, machine_name)) => RemoteTestResult {
            ok: true,
            addr: format!("127.0.0.1:{}（经跳板机）", endpoint.port),
            ms,
            machine_id,
            machine_name,
            error: String::new(),
        },
        Err(error) => RemoteTestResult {
            ok: false,
            addr: String::new(),
            ms: 0,
            machine_id: String::new(),
            machine_name: String::new(),
            error,
        },
    }
}

/// 关于页：打开项目 GitHub 页面（白名单限定本项目域名，不开放任意 URL）。
#[tauri::command]
pub fn hub_about_open(page: String) -> Result<(), String> {
    let url = match page.as_str() {
        "repo" => "https://github.com/lidiaoo/anyboot",
        "releases" => "https://github.com/lidiaoo/anyboot/releases",
        _ => return Err(format!("未知页面: {page}")),
    };
    crate::open_url_in_browser(url)
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_address_forms() {
        // 裸 host:port 原样
        assert_eq!(split_address("192.168.0.120:12300"), ("192.168.0.120:12300".into(), None));
        // 裸 host 无端口（探活时补默认端口）
        assert_eq!(split_address("192.168.0.120"), ("192.168.0.120".into(), None));
        // 整条 URL：剥 scheme/路径，挖 token
        assert_eq!(
            split_address("http://192.168.0.120:12300/?token=abc123"),
            ("192.168.0.120:12300".into(), Some("abc123".into()))
        );
        // https + 带路径
        assert_eq!(
            split_address("https://host.example.com:12300/remote?token=t+x"),
            ("host.example.com:12300".into(), Some("t+x".into()))
        );
        // URL 无 token
        assert_eq!(
            split_address("http://192.168.0.120:12300"),
            ("192.168.0.120:12300".into(), None)
        );
        // 空 / 空白
        assert_eq!(split_address("  "), (String::new(), None));
    }

    #[test]
    fn with_default_port_rules() {
        assert_eq!(with_default_port("192.168.0.120"), "192.168.0.120:12300");
        assert_eq!(with_default_port("192.168.0.120:12300"), "192.168.0.120:12300");
        assert_eq!(with_default_port("localhost:8080"), "localhost:8080");
    }

    #[test]
    fn parse_health_fields() {
        let body = r#"{"ok":true,"service":"anyboot","machineId":" m-1 ","machineName":"书房台式机"}"#;
        assert_eq!(parse_health(body), ("m-1".into(), "书房台式机".into()));
        // 非法 JSON / 缺字段 → 空串，不报错
        assert_eq!(parse_health("not json"), (String::new(), String::new()));
        assert_eq!(parse_health("{}"), (String::new(), String::new()));
    }

    #[test]
    fn normalize_trims_and_merges_token_from_url() {
        let server = RemoteServer {
            id: String::new(),
            group: " 书房 ".into(),
            name: " 台式机 ".into(),
            addresses: vec![
                " 192.168.0.120:12300 ".into(),
                "http://192.168.0.121:12300/?token= abc ".into(),
                "192.168.0.120:12300".into(), // 重复，去掉
                " ".into(),                   // 空，去掉
            ],
            token: String::new(),
            machine_id: String::new(),
            machine_name: String::new(),
            probe_mode: " other ".into(),
            jump_id: " j1 ".into(),
            created_at: 0,
        };
        let normalized = server.normalize();
        assert_eq!(normalized.group, "书房");
        assert_eq!(normalized.name, "台式机");
        assert_eq!(normalized.addresses, vec!["192.168.0.120:12300", "192.168.0.121:12300"]);
        // URL 里的 token 被挖出补进 token（原 token 为空时）
        assert_eq!(normalized.token, "abc");
        assert_eq!(normalized.probe_mode, "parallel");
        assert_eq!(normalized.jump_id, "j1");
    }

    #[test]
    fn normalize_keeps_explicit_token_over_url() {
        let server = RemoteServer {
            id: String::new(),
            group: String::new(),
            name: String::new(),
            addresses: vec!["http://192.168.0.120:12300/?token=urltoken".into()],
            token: " explicit ".into(),
            machine_id: String::new(),
            machine_name: String::new(),
            probe_mode: "serial".into(),
            jump_id: String::new(),
            created_at: 0,
        };
        let normalized = server.normalize();
        assert_eq!(normalized.token, "explicit");
        assert_eq!(normalized.probe_mode, "serial");
    }

    #[test]
    fn store_roundtrip_defaults() {
        let path = std::env::temp_dir().join(format!("ab-remote-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        // 不存在 = 默认空表
        assert!(load_store_from(&path).servers.is_empty());
        assert!(!load_store_from(&path).connect_all);
        // 写入再读回
        let store = RemoteStore {
            connect_all: true,
            servers: vec![RemoteServer {
                id: "rmt-1".into(),
                group: "g".into(),
                name: "n".into(),
                addresses: vec!["127.0.0.1:12300".into()],
                token: "t".into(),
                machine_id: String::new(),
                machine_name: String::new(),
                probe_mode: "parallel".into(),
                jump_id: String::new(),
                created_at: 42,
            }],
        };
        save_store_to(&path, &store).expect("落盘");
        let loaded = load_store_from(&path);
        assert_eq!(loaded, store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remote_target_forms() {
        // 裸 host → 默认端口
        assert_eq!(remote_target("192.168.0.120"), ("192.168.0.120".into(), 12300));
        // host:port
        assert_eq!(remote_target("10.0.0.8:12400"), ("10.0.0.8".into(), 12400));
        // URL 形态（split_address 拆出 authority 与端口）
        assert_eq!(remote_target("http://192.168.0.120:12300/"), ("192.168.0.120".into(), 12300));
        assert_eq!(remote_target("  http://home.example.com  "), ("home.example.com".into(), 12300));
    }

    #[test]
    fn remote_forward_arg_shape() {
        assert_eq!(remote_forward_arg(15300, "192.168.0.120", 12300), "127.0.0.1:15300:192.168.0.120:12300");
    }

    #[test]
    fn remote_test_reports_missing_addresses() {
        let result = remote_test(vec![" ".into()], "parallel".into());
        assert!(!result.ok);
        assert_eq!(result.error, "没有可探测的地址");
    }
}
