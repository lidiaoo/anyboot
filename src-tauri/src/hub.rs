//! server-hub（远程管理独立窗口）的后端：服务器档案存储 + CRUD + 测试连接。
//!
//! 刻意与主窗口的任何状态解耦（主窗口零改动原则）：
//! - 存储是配置目录下自己的一个 JSON 文件（`hub-servers.json`）；
//! - 命令只服务 hub 窗口，语义自足，不碰 UiState。
//!
//! 密码可存可存（M6）：AES-256-GCM 密文落盘（hub_vault.rs），ssh 经
//! SSH_ASKPASS 免输喂入；不存则保持 M3 的 PTY 内交互认证。认证细节交给
//! ssh 本尊，hub 只管「连到哪 + 带什么凭据」。

use serde::{Deserialize, Serialize};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::AppHandle;

/// 服务器档案文件名（放配置目录，随 `ANYBOOT_CONFIG_DIR` 走）
pub(crate) const HUB_SERVERS_FILE: &str = "hub-servers.json";

/// 测试连接的 TCP 超时
const TEST_TIMEOUT: Duration = Duration::from_secs(3);

/// 四个电源动作（前后端 + 移动端唯一的一套字符串）
pub(crate) const POWER_WAKE: &str = "wake";
pub(crate) const POWER_REBOOT: &str = "reboot";
pub(crate) const POWER_SHUTDOWN: &str = "shutdown";
pub(crate) const POWER_SLEEP: &str = "sleep";

/// 把各种写法（英文 / 中文 / 旧别名）归一成四个动作之一；不认识返回 None。
///
/// <p>为什么容错：这些值会落进 `hub-servers.json`，用户手工改过、或者从旧版
/// 备份里恢复过来的都可能有别名。宁可认出来，也别把一条配好的路由悄悄丢掉。
pub(crate) fn normalize_power_action(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "wake" | "wol" | "wakeup" | "唤醒" | "开机" => Some(POWER_WAKE),
        "reboot" | "restart" | "重启" => Some(POWER_REBOOT),
        "shutdown" | "poweroff" | "power-off" | "关机" => Some(POWER_SHUTDOWN),
        "sleep" | "suspend" | "sleep-hybrid" | "睡眠" => Some(POWER_SLEEP),
        _ => None,
    }
}

/// 一条「局域网电源路由」：把**动作 → 发到哪 → 哪个端口 → 带哪些 MAC** 绑起来。
///
/// <p>为什么要用户自己配，而不是写死端口：目标机装的是 sol，它在局域网里
/// 按 UDP 端口区分要干什么（9 唤醒、别的端口重启/关机/睡眠……每个人给自己的
/// 机器分的端口都不一样）。这里配的就是「发到哪个广播地址、哪个端口、带哪块网卡的 MAC」。
///
/// <p>为空 = 没配，电源操作照旧：重启/关机/睡眠走 SSH（要求 sudo 免密），
/// 唤醒走标准 WOL（9 + 7 端口，本机或跳板机广播）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PowerRoute {
    /// wake / reboot / shutdown / sleep（空 = wake）
    #[serde(default)]
    pub action: String,
    /// 目标地址：广播地址（192.168.0.255 / 255.255.255.255）或单播 IP / 主机名。
    /// 空 = 自动（全局广播 + 各访问地址所在 /24 的广播）
    #[serde(default)]
    pub target: String,
    /// UDP 端口（0 = 9）。不同端口 = 不同动作，由目标机上的 sol 监听
    #[serde(default)]
    pub port: u16,
    /// 用哪些网卡的 MAC（空 = 这台服务器配过的全部 MAC）
    #[serde(default)]
    pub macs: Vec<String>,
    /// 关掉但不想删（留着自己记）
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 这条路由每次发几个包（0 = 按动作默认：唤醒 10、其余 1）。
    /// 每个动作都能单独设 —— 重启/关机/睡眠发多个包等于让 sol 执行多次，改之前想清楚。
    #[serde(default)]
    pub burst: u32,
    /// 一轮没全发出去时的重试次数（0 = 按动作默认：唤醒 10、其余 1）。每个动作都能设。
    #[serde(default)]
    pub retry: u32,
    /// sol 包没发出去时，改调这台电脑自带的 /api/power 接口兜底。唤醒也能勾：
    /// HTTP 能送达就说明机器其实醒着（服务端回 WakeAck，不做任何动作）。
    /// 所有动作默认勾上。
    #[serde(default = "default_true")]
    pub http_fallback: bool,
}

fn default_true() -> bool {
    true
}

/// 唤醒包默认发几个（每条路由、每块网卡）
pub(crate) const DEFAULT_WAKE_BURST: u32 = 10;

/// 唤醒包默认重试几次（一轮有包没发出去才重试，间隔 200ms）
pub(crate) const DEFAULT_WAKE_RETRY: u32 = 10;

/// 重启 / 关机 / 睡眠默认只发 1 个包 —— 同一个包到两次，sol 就执行两次
pub(crate) const DEFAULT_POWER_BURST: u32 = 1;

/// 重启 / 关机 / 睡眠默认重试 1 次（只在真没发出去时才补发）
pub(crate) const DEFAULT_POWER_RETRY: u32 = 1;

impl PowerRoute {
    /// 归一化后的动作（空 / 不认识一律按唤醒处理，别让一条路由失效）
    pub fn action_or_wake(&self) -> &'static str {
        normalize_power_action(&self.action).unwrap_or(POWER_WAKE)
    }

    /// 归一化后的端口
    pub fn effective_port(&self) -> u16 {
        if self.port == 0 {
            9
        } else {
            self.port
        }
    }

    /// 每次发几个包（0 = 按动作默认；上限 50 —— 再多人也接不住，纯属广播风暴）
    pub fn effective_burst(&self) -> u32 {
        if self.burst == 0 {
            self.default_burst()
        } else {
            self.burst.clamp(1, 50)
        }
    }

    /// 失败重试次数（0 = 按动作默认；上限 20）
    pub fn effective_retry(&self) -> u32 {
        if self.retry == 0 {
            self.default_retry()
        } else {
            self.retry.clamp(1, 20)
        }
    }

    /// 没填发包数时按动作取默认：唤醒 10（唤醒包丢了就是叫不醒），其余 1（重复会重复执行）
    pub fn default_burst(&self) -> u32 {
        if self.action_or_wake() == POWER_WAKE {
            DEFAULT_WAKE_BURST
        } else {
            DEFAULT_POWER_BURST
        }
    }

    /// 没填重试次数时按动作取默认：唤醒 10，其余 1
    pub fn default_retry(&self) -> u32 {
        if self.action_or_wake() == POWER_WAKE {
            DEFAULT_WAKE_RETRY
        } else {
            DEFAULT_POWER_RETRY
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HubServer {
    pub id: String,
    pub name: String,
    /// "ssh" | "telnet"（M3 终端先通 SSH；telnet 字段先留着）
    pub protocol: String,
    pub host: String,
    pub port: u16,
    /// 全部访问地址（`host:port`，有序）。
    ///
    /// <p>`addresses[0]` 恒等于 `host:port` —— 老字段继续是「当前连接用的地址」，
    /// 所有既有的 ssh / sftp / 隧道代码一个字都不用改；多出来的地址用于
    /// 「测试连接时都探一遍」「唤醒时按地址匹配 MAC」。
    ///
    /// <p>为空时由 `host` + `port` 反推（老档案没有这个字段）。
    #[serde(default)]
    pub addresses: Vec<String>,
    /// 每个地址对应的网卡 MAC（唤醒用）：地址 → MAC 列表（小写、冒号分隔）。
    /// 只影响唤醒，不参与连接。
    #[serde(default)]
    pub macs: std::collections::BTreeMap<String, Vec<String>>,
    /// 局域网电源路由（唤醒 / 重启 / 关机 / 睡眠 各发到哪个广播地址、哪个端口、
    /// 带哪块网卡的 MAC）。空 = 没配，走原来的 SSH / 标准 WOL。
    #[serde(default)]
    pub power_routes: Vec<PowerRoute>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub notes: String,
    /// 认证方式（三选一，编辑表单里分段）：
    /// - 交互：三个字段全空，密码/首次指纹在 ssh 里输（凭据不落盘）
    /// - 密码：password_enc 非空（AES-GCM 密文，hub_vault），ssh 经 askpass 免输
    /// - 密钥：key_id 非空（hub_keys 文件基名），起 ssh 拼 `-i`；带口令的钥匙仍在终端解锁
    #[serde(default)]
    pub key_id: String,
    /// 存的密码（AES-256-GCM 密文）。空 = 未存。
    #[serde(default)]
    pub password_enc: String,
    /// 经由哪台跳板机（另一台 HubServer.id）；空 = 直连，行为与现在完全一致。
    /// 一个 SSH 服务器可以借助另一台 SSH 服务器连上：终端的 ssh 走 ProxyCommand
    /// 先登录跳板机再转发；文件 / 隧道复用终端的 ControlMaster，自动跟着走。
    #[serde(default)]
    pub jump_id: String,
    /// 终端启动路径（可选，每台服务器各自配）：非空时登录后先 `cd` 过去。
    /// 目录不存在/无权限时把报错打进终端，然后**照常**落进远端默认目录
    /// （用户定案：不因填错路径把自己锁在外面）。空 = 远端家目录，零干预。
    #[serde(default)]
    pub term_path: String,
    /// 终端 shell（可选，每台服务器各自配）：`/bin/bash` `/bin/sh` `/bin/zsh`
    /// `/bin/fish` 等预设，或任意自定义命令原样 exec。空 = 远端账号的登录 shell。
    #[serde(default)]
    pub term_shell: String,
    /// 前端提交的新密码明文。只作为 hub_save_server 的入参字段：
    /// `skip_serializing` 保证它永不落盘、也永不回传给前端；
    /// hub_save_server 里消化成 password_enc（None = 不动已存密码，空串 = 清除）。
    #[serde(default, skip_serializing)]
    pub password_new: Option<String>,
    #[serde(default)]
    pub created_at: u64,
}

impl HubServer {
    /// 按协议补默认端口（0 = 用户没填）
    pub fn effective_port(&self) -> u16 {
        if self.port != 0 {
            return self.port;
        }
        match self.protocol.as_str() {
            "telnet" => 23,
            _ => 22,
        }
    }

    pub(crate) fn normalize(mut self) -> Self {
        self.id = self.id.trim().to_string();
        self.name = self.name.trim().to_string();
        self.protocol = match self.protocol.as_str() {
            "telnet" => "telnet".to_string(),
            _ => "ssh".to_string(),
        };
        self.host = self.host.trim().to_string();
        self.username = self.username.trim().to_string();
        self.key_id = self.key_id.trim().to_string();
        self.jump_id = self.jump_id.trim().to_string();
        self.term_path = self.term_path.trim().to_string();
        self.term_shell = self.term_shell.trim().to_string();
        // 多地址：清理 + 用 addresses[0] 回写 host/port（老档案没有 addresses，从 host/port 反推）
        self.normalize_addresses();
        self.normalize_power_routes();
        self
    }

    /// 地址 / MAC 归一化：地址去 scheme、补默认端口、去重保序；MAC 只留仍存在地址的。
    /// 末尾把 `addresses[0]` 回写到 `host` / `port` —— 老字段永远是「首选地址」。
    fn normalize_addresses(&mut self) {
        let default_port = self.effective_port();
        let mut cleaned: Vec<String> = Vec::new();
        if self.addresses.is_empty() {
            if !self.host.is_empty() {
                if let Some(address) = normalize_address(&self.host, default_port) {
                    cleaned.push(address);
                }
            }
        } else {
            for raw in &self.addresses {
                if let Some(address) = normalize_address(raw, default_port) {
                    if !cleaned.contains(&address) {
                        cleaned.push(address);
                    }
                }
            }
        }
        self.addresses = cleaned;
        if let Some(first) = self.addresses.first().cloned() {
            if let Some((host, port)) = split_address(&first) {
                self.host = host;
                self.port = port;
            }
        }

        let mut macs = std::collections::BTreeMap::new();
        for (raw_host, values) in std::mem::take(&mut self.macs) {
            let Some(address) = normalize_address(&raw_host, default_port) else {
                continue;
            };
            if !self.addresses.contains(&address) {
                continue; // 地址被删了，MAC 跟着丢
            }
            let mut list: Vec<String> = Vec::new();
            for value in values {
                if let Some(mac) = normalize_mac(&value) {
                    if !list.contains(&mac) {
                        list.push(mac);
                    }
                }
            }
            if !list.is_empty() {
                macs.insert(address, list);
            }
        }
        self.macs = macs;
    }

    /// 这台服务器配过的所有 MAC（去重、按地址顺序）—— 唤醒时全发一遍
    pub fn all_macs(&self) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        for address in &self.addresses {
            if let Some(values) = self.macs.get(address) {
                for mac in values {
                    if !result.contains(mac) {
                        result.push(mac.clone());
                    }
                }
            }
        }
        for values in self.macs.values() {
            for mac in values {
                if !result.contains(mac) {
                    result.push(mac.clone());
                }
            }
        }
        result
    }

    /// 电源路由归一化：动作认成四个之一、端口 0 补 9、地址去掉 scheme/路径、
    /// MAC 归一化去重。**不删「已不在地址列表里的 MAC」** —— 路由是用户自己
    /// 手写的意图，删地址不等于想改唤醒目标。
    fn normalize_power_routes(&mut self) {
        let mut cleaned: Vec<PowerRoute> = Vec::new();
        for raw in std::mem::take(&mut self.power_routes) {
            let action = raw.action_or_wake().to_string();
            let mut macs: Vec<String> = Vec::new();
            for value in &raw.macs {
                if let Some(mac) = normalize_mac(value) {
                    if !macs.contains(&mac) {
                        macs.push(mac);
                    }
                }
            }
            cleaned.push(PowerRoute {
                action,
                target: normalize_route_target(&raw.target),
                port: raw.effective_port(),
                macs,
                enabled: raw.enabled,
                burst: if raw.burst == 0 { 0 } else { raw.effective_burst() },
                retry: if raw.retry == 0 { 0 } else { raw.effective_retry() },
                http_fallback: raw.http_fallback,
            });
        }
        self.power_routes = cleaned;
    }

    /// 某个动作配了哪些（启用中的）路由
    pub fn routes_for(&self, action: &str) -> Vec<PowerRoute> {
        let wanted = normalize_power_action(action).unwrap_or(POWER_WAKE);
        self.power_routes
            .iter()
            .filter(|route| route.enabled && route.action_or_wake() == wanted)
            .cloned()
            .collect()
    }

    /// 这条路由实际要发的 MAC：路由里选了就用选的，没选 = 这台服务器配过的全部
    pub fn route_macs(&self, route: &PowerRoute) -> Vec<String> {
        if route.macs.is_empty() {
            self.all_macs()
        } else {
            route.macs.clone()
        }
    }
}

/// 路由目标归一化：去 `udp://` 之类 scheme、去路径尾巴、去空白。
/// 空串 = 自动（全局广播 + 各地址的 /24 广播），保持空串。
pub(crate) fn normalize_route_target(raw: &str) -> String {
    let mut value = raw.trim();
    if let Some(index) = value.find("://") {
        value = &value[index + 3..];
    }
    if let Some(index) = value.find('/') {
        value = &value[..index];
    }
    value.trim().to_string()
}

/// 地址归一化：去 `scheme://`、路径、查询串，没写端口就补默认端口。
/// 空串 / 全是空白返回 None。
pub(crate) fn normalize_address(raw: &str, default_port: u16) -> Option<String> {
    let mut value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(index) = value.find("://") {
        value = &value[index + 3..];
    }
    if let Some(index) = value.find('/') {
        value = &value[..index];
    }
    if let Some(index) = value.find('?') {
        value = &value[..index];
    }
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let has_port = value
        .rfind(':')
        .is_some_and(|index| index + 1 < value.len() && value[index + 1..].chars().all(|c| c.is_ascii_digit()));
    Some(if has_port {
        value.to_string()
    } else {
        format!("{value}:{default_port}")
    })
}

/// `host:port` 拆开（IPv6 字面量不支持，与既有实现一致）
pub(crate) fn split_address(address: &str) -> Option<(String, u16)> {
    let (host, port) = address.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port))
}

/// 是不是 MAC：支持 `aa:bb:cc:dd:ee:ff` / `aa-bb-...` / `aabb.ccdd.eeff` / 12 位连写。
/// 先排除 IPv4 —— `192.168.100.100` 去掉点正好是 12 位十六进制。
pub(crate) fn is_mac(raw: &str) -> bool {
    let value = raw.trim();
    if value.is_empty() {
        return false;
    }
    let pieces: Vec<&str> = value.split('.').collect();
    if pieces.len() == 4
        && pieces
            .iter()
            .all(|piece| !piece.is_empty() && piece.chars().all(|c| c.is_ascii_digit()))
    {
        return false;
    }
    normalize_mac(value).is_some()
}

/// 归一化成小写冒号分隔；不是 MAC 返回 None
pub(crate) fn normalize_mac(raw: &str) -> Option<String> {
    let hex: String = raw
        .trim()
        .chars()
        .filter(|c| *c != ':' && *c != '-' && *c != '.')
        .collect();
    if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let lower = hex.to_ascii_lowercase();
    let mut parts: Vec<String> = Vec::with_capacity(6);
    for index in (0..12).step_by(2) {
        parts.push(lower[index..index + 2].to_string());
    }
    Some(parts.join(":"))
}

// ==================== 存储 ====================

fn servers_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::config_dir(app)?.join(HUB_SERVERS_FILE))
}

fn load_servers_from(path: &Path) -> Vec<HubServer> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return Vec::new(), // 不存在 = 还没存过
    };
    serde_json::from_str(&content).unwrap_or_else(|error| {
        eprintln!("hub-servers.json 解析失败（按空表处理）: {error}");
        Vec::new()
    })
}

fn save_servers_to(path: &Path, servers: &[HubServer]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建配置目录: {error}"))?;
    }
    // 临时文件 + 改名：写一半被杀也不会留下半截 JSON
    let temporary = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(servers)
        .map_err(|error| format!("序列化失败: {error}"))?;
    std::fs::write(&temporary, content.as_bytes())
        .map_err(|error| format!("无法写入 {temporary:?}: {error}"))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("无法落盘 {path:?}: {error}"))?;
    Ok(())
}

fn load_servers(app: &AppHandle) -> Result<Vec<HubServer>, String> {
    Ok(load_servers_from(&servers_path(app)?))
}

/// hub_keys 统计「被 N 台服务器使用」要用（跨模块只读）
pub(crate) fn load_servers_pub(app: &AppHandle) -> Result<Vec<HubServer>, String> {
    load_servers(app)
}

/// hub_tunnel / hub_keys 按 id 查服务器（只读快照语义）
pub(crate) fn load_servers_for_tunnel(app: &AppHandle) -> Result<Vec<HubServer>, String> {
    load_servers(app)
}

fn save_servers(app: &AppHandle, servers: &[HubServer]) -> Result<(), String> {
    save_servers_to(&servers_path(app)?, servers)
}

fn new_server_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("srv-{nanos:016x}")
}

/// askpass 分支的核心（cli.rs `--hub-askpass <id>` 调用，单测也走这里）：
/// 按 id 找服务器 → 内存解密出密码。config 目录由调用方给（CLI 没有 AppHandle）。
pub(crate) fn askpass_password(config: &Path, server_id: &str) -> Result<String, String> {
    let servers = load_servers_from(&config.join(HUB_SERVERS_FILE));
    let server = servers
        .iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("找不到服务器（{server_id}）"))?;
    if server.password_enc.is_empty() {
        return Err("这台服务器没有存密码".to_string());
    }
    let key = crate::hub_vault::load_or_create_key(config)?;
    crate::hub_vault::decrypt_with(&key, &server.password_enc)
}

/// askpass 分支：优先按提示语里的 host 找服务器，找不到再退回环境变量给的 id。
///
/// 为什么要看提示语：跳板机和目标机共用**同一个** askpass 程序（ssh 内部起的
/// ProxyCommand 子进程只会继承外层进程的环境变量），而 Windows 上没有 shell，
/// 没法像以前那样给 ProxyCommand 塞 `env SSH_ASKPASS=…` 前缀。所以改成
/// 「谁问的密码，就看提示语里的 `user@host` 是谁」，两条连接各取各的密码。
pub(crate) fn askpass_password_for_prompt(
    config: &Path,
    preferred_id: &str,
    prompt: &str,
) -> Result<String, String> {
    let mut id = preferred_id.to_string();
    if let Some((user, host)) = host_from_askpass_prompt(prompt) {
        let servers = load_servers_from(&config.join(HUB_SERVERS_FILE));
        let preferred_matches = servers
            .iter()
            .find(|server| server.id == preferred_id)
            .is_some_and(|server| host_matches(&server.host, &host));
        if !preferred_matches {
            let hit = servers.iter().find(|server| {
                host_matches(&server.host, &host)
                    && (user.is_empty()
                        || server.username.is_empty()
                        || server.username.eq_ignore_ascii_case(&user))
            });
            if let Some(server) = hit {
                id = server.id.clone();
            }
        }
    }
    askpass_password(config, &id)
}

/// 从 ssh 的 askpass 提示语里抠出 `(user, host)`。
/// 形态是固定的英文：`user@host's password: `（没有 user 时是 `host's password: `）。
/// 抠不出来（比如 `Enter passphrase for key '…':`）就返回 None —— 那是密钥口令，
/// 不该拿存档密码去答。
pub(crate) fn host_from_askpass_prompt(prompt: &str) -> Option<(String, String)> {
    let text = prompt.trim();
    let head = text.split("'s password").next()?;
    if head.len() == text.len() {
        return None;
    }
    let (user, host) = match head.rsplit_once('@') {
        Some((user, host)) => (user.trim().to_string(), host.trim().to_string()),
        None => (String::new(), head.trim().to_string()),
    };
    if host.is_empty() {
        None
    } else {
        Some((user, host))
    }
}

/// host 比对：忽略大小写与首尾点，`[::1]` 这种带方括号的拆掉再比。
fn host_matches(configured: &str, prompted: &str) -> bool {
    fn norm(value: &str) -> String {
        value
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_end_matches('.')
            .to_ascii_lowercase()
    }
    norm(configured) == norm(prompted)
}

// ==================== 命令 ====================

#[tauri::command]
pub fn hub_list_servers(app: AppHandle) -> Result<Vec<HubServer>, String> {
    let mut servers = load_servers(&app)?;
    servers.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(servers)
}

/// 新增或更新（按 id 区分）。新条目在这里拿 id 与 created_at。
#[tauri::command]
pub fn hub_save_server(app: AppHandle, server: HubServer) -> Result<HubServer, String> {
    let mut server = server.normalize();
    // 密码三态：None = 前端没碰密码框（保留已存）；Some("") = 清除；Some(p) = 换新密文。
    // 交互/密钥两种认证方式下前端一律提交空串，切换认证方式即顺手清掉存着的密码。
    let keep_existing_password = server.password_new.is_none();
    if let Some(plain) = server.password_new.take() {
        server.password_enc = if plain.is_empty() {
            String::new()
        } else {
            crate::hub_vault::encrypt(&app, &plain)?
        };
    }
    server.password_enc = server.password_enc.trim().to_string();
    if server.name.is_empty() {
        return Err("名称不能为空".to_string());
    }
    if server.host.is_empty() {
        return Err("主机地址不能为空".to_string());
    }
    // 跳板机只对 SSH 有意义：Telnet 没有转发能力；自引用直接拦下
    if !server.jump_id.is_empty() {
        if server.protocol != "ssh" {
            return Err("经由跳板机只支持 SSH 服务器（Telnet 没有转发能力）".to_string());
        }
        if server.jump_id == server.id {
            return Err("跳板机不能是服务器自己".to_string());
        }
    }
    let mut servers = load_servers(&app)?;
    if server.id.is_empty() {
        server.id = new_server_id();
        server.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        servers.push(server.clone());
    } else if let Some(existing) = servers.iter_mut().find(|s| s.id == server.id) {
        let created_at = existing.created_at;
        // 前端对象不回传 password_enc（密文不进表单往返）：password_new == None
        // 表示用户没碰密码框，此时以磁盘上的旧密文为准，别被入参里的空串冲掉。
        if keep_existing_password {
            server.password_enc = existing.password_enc.clone();
        }
        *existing = server.clone();
        existing.created_at = created_at;
    } else {
        return Err(format!("要编辑的服务器不存在（{}）", server.id));
    }
    save_servers(&app, &servers)?;
    Ok(server)
}

#[tauri::command]
pub fn hub_delete_server(app: AppHandle, id: String) -> Result<(), String> {
    let mut servers = load_servers(&app)?;
    let before = servers.len();
    servers.retain(|s| s.id != id);
    if servers.len() == before {
        return Err(format!("要删除的服务器不存在（{id}）"));
    }
    save_servers(&app, &servers)
}

/// 测试连接：TCP 能不能握上手（不认证，认证在 M3 终端里由 ssh 本尊管）。
/// 经由跳板机时，目标主机往往只有跳板机可达，直测 TCP 没有意义 ——
/// 改测「跳板机通不通」，端到端连通性在终端里由 ssh 验证。
///
/// **必须 async + spawn_blocking**：`connect_timeout` 最长要等满整个
/// `TEST_TIMEOUT`，而终端页每 10 秒调一次做延迟探测。同步命令跑在 UI 线程上，
/// 一次连不通就是一次整窗卡顿（用户视角：界面一抽一抽）。
#[tauri::command]
pub async fn hub_test_server(app: AppHandle, server: HubServer) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || test_server_blocking(&app, server))
        .await
        .map_err(|error| format!("测试连接任务失败: {error}"))?
}

fn test_server_blocking(app: &AppHandle, server: HubServer) -> Result<String, String> {
    let server = server.normalize();
    check_test_target(&server)?;
    if server.jump_id.is_empty() {
        test_direct_tcp(server)
    } else {
        test_via_jump(app, server)
    }
}

/// 测试前的公共校验（独立成纯函数：单测不依赖 AppHandle）
fn check_test_target(server: &HubServer) -> Result<(), String> {
    if server.host.is_empty() {
        return Err("主机地址不能为空".to_string());
    }
    if !server.jump_id.is_empty() && server.jump_id == server.id {
        return Err("跳板机不能是服务器自己".to_string());
    }
    Ok(())
}

fn test_direct_tcp(server: HubServer) -> Result<String, String> {
    // 多地址：按顺序试，第一个通的就用它（与移动端「串行探测」同一语义）。
    // 连接本身仍用 host/port（= addresses[0]），这里只是把「哪一个地址是活的」告诉用户。
    let total = server.addresses.len();
    let mut errors: Vec<String> = Vec::new();
    for address in &server.addresses {
        let Some((host, port)) = split_address(address) else {
            continue;
        };
        match connect_tcp(&host, port) {
            Ok(ip) => {
                return Ok(if total > 1 {
                    format!("已连通 {address}（{ip}）· 共 {total} 个地址里这个最快")
                } else {
                    format!("已连通 {host}:{port}（{ip}）")
                });
            }
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        return Err("没有可用地址".to_string());
    }
    Err(errors.join("；"))
}

/// 单地址 TCP 握手探测（纯函数，方便复用与阅读）
fn connect_tcp(host: &str, port: u16) -> Result<std::net::IpAddr, String> {
    let target = format!("{host}:{port}");
    let address = target
        .to_socket_addrs()
        .map_err(|error| format!("无法解析 {target}: {error}"))?
        .next()
        .ok_or_else(|| format!("无法解析 {target}"))?;
    match TcpStream::connect_timeout(&address, TEST_TIMEOUT) {
        Ok(_) => Ok(address.ip()),
        Err(error) => Err(format!("连不上 {target}: {error}")),
    }
}

fn test_via_jump(app: &AppHandle, server: HubServer) -> Result<String, String> {
    let target = format!("{}:{}", server.host, server.effective_port());
    let jump = load_servers(app)?
        .into_iter()
        .find(|s| s.id == server.jump_id)
        .ok_or_else(|| "找不到经由的跳板机，请重新选择或改回直连".to_string())?;
    if jump.protocol != "ssh" {
        return Err(format!("跳板机「{}」不是 SSH 服务器", jump.name));
    }
    let jump_port = jump.effective_port();
    let jump_target = format!("{}:{}", jump.host, jump_port);
    let addresses: std::net::SocketAddr = jump_target
        .to_socket_addrs()
        .map_err(|error| format!("无法解析 {jump_target}: {error}"))?
        .next()
        .ok_or_else(|| format!("无法解析 {jump_target}"))?;
    match TcpStream::connect_timeout(&addresses, TEST_TIMEOUT) {
        Ok(_) => Ok(format!(
            "跳板机「{}」已连通 {}:{}（{}）—— 目标 {target} 将经由它中转，端到端连通性请打开终端验证",
            jump.name,
            jump.host,
            jump_port,
            addresses.ip()
        )),
        Err(error) => Err(format!(
            "连不上跳板机「{}」（{jump_target}）: {error}",
            jump.name
        )),
    }
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ab-hub-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn sample(name: &str) -> HubServer {
        HubServer {
            id: String::new(),
            name: name.to_string(),
            protocol: "ssh".to_string(),
            host: "10.0.0.2".to_string(),
            port: 0,
            addresses: Vec::new(),
            macs: std::collections::BTreeMap::new(),
            power_routes: Vec::new(),
            username: "root".to_string(),
            notes: String::new(),
            key_id: String::new(),
            password_enc: String::new(),
            jump_id: String::new(),
            term_path: String::new(),
            term_shell: String::new(),
            password_new: None,
            created_at: 0,
        }
    }

    #[test]
    fn mac_parsing_survives_ipv4_lookalikes() {
        // 三种常见写法都要认
        assert!(is_mac("AA:BB:CC:DD:EE:FF"));
        assert!(is_mac("aa-bb-cc-dd-ee-ff"));
        assert!(is_mac("aabb.ccdd.eeff"));
        assert!(is_mac("aabbccddeeff"));
        // 192.168.100.100 去掉分隔符正好 12 位十六进制 —— 必须仍算 IP
        assert!(!is_mac("192.168.100.100"));
        assert!(!is_mac("10.0.0.2"));
        assert_eq!(
            normalize_mac("AA:BB:CC:DD:EE:FF").as_deref(),
            Some("aa:bb:cc:dd:ee:ff")
        );
        assert_eq!(
            normalize_address("http://192.168.0.120:2222/x?token=t", 22).as_deref(),
            Some("192.168.0.120:2222")
        );
        assert_eq!(
            normalize_address("192.168.0.120", 22).as_deref(),
            Some("192.168.0.120:22")
        );
    }

    #[test]
    fn power_routes_normalize_and_match_action() {
        let mut macmap = std::collections::BTreeMap::new();
        macmap.insert(
            "192.168.0.120:12300".to_string(),
            vec!["aa:bb:cc:dd:ee:ff".to_string()],
        );
        let server = HubServer {
            addresses: vec!["192.168.0.120:12300".to_string()],
            macs: macmap,
            power_routes: vec![
                PowerRoute {
                    action: "唤醒".to_string(),
                    target: " udp://192.168.0.255/ ".to_string(),
                    port: 0,
                    macs: vec!["AA-BB-CC-DD-EE-FF".to_string(), "aa:bb:cc:dd:ee:ff".to_string()],
                    enabled: true,
                    burst: 0,
                    retry: 0,
                    http_fallback: false,
                },
                PowerRoute {
                    action: "restart".to_string(),
                    target: String::new(),
                    port: 9200,
                    macs: Vec::new(),
                    enabled: true,
                    burst: 0,
                    retry: 0,
                    http_fallback: false,
                },
                PowerRoute {
                    action: "关机".to_string(),
                    target: String::new(),
                    port: 9201,
                    macs: Vec::new(),
                    enabled: false,
                    burst: 0,
                    retry: 0,
                    http_fallback: false,
                },
            ],
            ..sample("routes")
        }
        .normalize();

        assert_eq!(server.power_routes.len(), 3);
        // 中文动作 / 别名认出来；scheme 和路径尾巴去掉；端口 0 → 9；MAC 归一化去重
        assert_eq!(server.power_routes[0].action, POWER_WAKE);
        assert_eq!(server.power_routes[0].target, "192.168.0.255");
        assert_eq!(server.power_routes[0].port, 9);
        assert_eq!(server.power_routes[0].macs, vec!["aa:bb:cc:dd:ee:ff".to_string()]);
        assert_eq!(server.power_routes[1].action, POWER_REBOOT);
        assert_eq!(server.power_routes[2].action, POWER_SHUTDOWN);

        assert_eq!(server.routes_for("wake").len(), 1);
        assert_eq!(server.routes_for("reboot").len(), 1);
        // 关掉的那条不参与
        assert!(server.routes_for("shutdown").is_empty());
        // 没选 MAC → 用这台服务器的全部
        assert_eq!(
            server.route_macs(&server.power_routes[1]),
            vec!["aa:bb:cc:dd:ee:ff".to_string()]
        );
    }

    #[test]
    fn power_route_burst_and_retry_default_to_ten() {
        let route = PowerRoute {
            action: POWER_WAKE.to_string(),
            target: String::new(),
            port: 0,
            macs: Vec::new(),
            enabled: true,
            burst: 0,
            retry: 0,
            http_fallback: false,
        };
        assert_eq!(route.effective_burst(), 10);
        assert_eq!(route.effective_retry(), 10);
        assert_eq!(route.effective_port(), 9);

        // 上限保护：狂暴值别把局域网刷穿
        let wild = PowerRoute { burst: 999, retry: 999, ..route.clone() };
        assert_eq!(wild.effective_burst(), 50);
        assert_eq!(wild.effective_retry(), 20);

        // 重启 / 关机 / 睡眠：没填就默认 1 包 / 1 重试（重复发包 = sol 重复执行），
        // 但每个动作都能自己设 —— 不是只有唤醒才有这两个数字
        for action in [POWER_REBOOT, POWER_SHUTDOWN, POWER_SLEEP] {
            let mut other = route.clone();
            other.action = action.to_string();
            assert_eq!(other.effective_burst(), DEFAULT_POWER_BURST);
            assert_eq!(other.effective_retry(), DEFAULT_POWER_RETRY);
            other.burst = 3;
            other.retry = 4;
            assert_eq!(other.effective_burst(), 3);
            assert_eq!(other.effective_retry(), 4);
        }

        // 0 保持「跟随动作默认」：归一化不能把它烤成固定数字
        let normalized = HubServer {
            power_routes: vec![PowerRoute {
                action: POWER_REBOOT.to_string(),
                target: String::new(),
                port: 0,
                macs: Vec::new(),
                enabled: true,
                burst: 0,
                retry: 0,
                http_fallback: false,
            }],
            ..sample("burst")
        }
        .normalize();
        assert_eq!(normalized.power_routes[0].burst, 0);
        assert_eq!(normalized.power_routes[0].retry, 0);
        assert_eq!(normalized.power_routes[0].effective_burst(), 1);
    }

    #[test]
    fn legacy_json_without_power_routes_still_loads() {
        let json = r#"{"id":"a","name":"n","protocol":"ssh","host":"10.0.0.2","port":22}"#;
        let server: HubServer = serde_json::from_str(json).expect("老档案要能读");
        assert!(server.power_routes.is_empty());
        assert!(server.routes_for("wake").is_empty());
    }

    #[test]
    fn server_addresses_drive_host_port_and_macs() {
        let server = HubServer {
            addresses: vec![
                "192.168.0.120:2222".to_string(),
                "192.168.0.121".to_string(),
            ],
            macs: {
                let mut map = std::collections::BTreeMap::new();
                map.insert(
                    "192.168.0.120:2222".to_string(),
                    vec!["AA-BB-CC-DD-EE-FF".to_string(), "11:22:33:44:55:66".to_string()],
                );
                map.insert(
                    "192.168.0.121:22".to_string(),
                    vec!["de:ad:be:ef:00:01".to_string()],
                );
                map
            },
            ..sample("multi")
        }
        .normalize();

        // 首地址回写成老字段（连接逻辑全用它）
        assert_eq!(server.host, "192.168.0.120");
        assert_eq!(server.port, 2222);
        assert_eq!(server.addresses.len(), 2);
        // 第二个地址补了默认端口
        assert_eq!(server.addresses[1], "192.168.0.121:22");
        // MAC 归一化 + 按地址分组；all_macs 去重合并
        assert_eq!(
            server.macs.get("192.168.0.120:2222").map(|v| v.len()),
            Some(2)
        );
        assert_eq!(server.all_macs().len(), 3);
    }

    #[test]
    fn legacy_server_without_addresses_still_works() {
        // 老档案只有 host/port：normalize 后要能自己补出 addresses，别把连接搞坏
        let server = sample("legacy").normalize();
        assert_eq!(server.addresses, vec!["10.0.0.2:22".to_string()]);
        assert_eq!(server.host, "10.0.0.2");
        assert_eq!(server.port, 22);
    }

    #[test]
    fn effective_port_defaults_by_protocol() {        assert_eq!(sample("a").effective_port(), 22);
        let telnet = HubServer {
            protocol: "telnet".to_string(),
            ..sample("b")
        };
        assert_eq!(telnet.effective_port(), 23);
        let explicit = HubServer {
            port: 2222,
            ..sample("c")
        };
        assert_eq!(explicit.effective_port(), 2222);
    }

    #[test]
    fn normalize_clamps_protocol_and_fields() {
        let server = HubServer {
            name: "  名字  ".to_string(),
            protocol: "gopher".to_string(),
            ..sample("d")
        }
        .normalize();
        assert_eq!(server.name, "名字");
        assert_eq!(server.protocol, "ssh");
    }

    #[test]
    fn store_roundtrip_and_crud() {
        let dir = temp_dir("crud");
        let path = dir.join(HUB_SERVERS_FILE);
        // 空表
        assert!(load_servers_from(&path).is_empty());

        // 新增（save_servers_to 只管落盘；id/created_at 由命令层发）
        let mut a = sample("alpha");
        a.id = new_server_id();
        a.created_at = 111;
        let mut b = sample("beta");
        b.id = new_server_id();
        b.created_at = 222;
        save_servers_to(&path, &[a.clone(), b.clone()]).expect("落盘");

        let loaded = load_servers_from(&path);
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().any(|s| s.id == a.id && s.name == "alpha"));

        // 更新：同 id 改名，created_at 不许被冲掉
        let mut edited = a.clone();
        edited.name = "alpha2".to_string();
        let mut servers = loaded;
        if let Some(existing) = servers.iter_mut().find(|s| s.id == edited.id) {
            let created_at = existing.created_at;
            *existing = edited.clone();
            existing.created_at = created_at;
        }
        save_servers_to(&path, &servers).expect("落盘");
        let loaded = load_servers_from(&path);
        let found = loaded.iter().find(|s| s.id == a.id).expect("还在");
        assert_eq!(found.name, "alpha2");
        assert_eq!(found.created_at, 111);

        // 删除
        servers.retain(|s| s.id != b.id);
        save_servers_to(&path, &servers).expect("落盘");
        assert_eq!(load_servers_from(&path).len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_store_file_reads_as_empty() {
        let dir = temp_dir("broken");
        let path = dir.join(HUB_SERVERS_FILE);
        std::fs::write(&path, "{not json").unwrap();
        assert!(load_servers_from(&path).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_server_rejects_empty_host() {
        let server = HubServer {
            host: String::new(),
            ..sample("e")
        }
        .normalize();
        assert!(check_test_target(&server).is_err());
        // 自引用（跳板机 = 自己）同样拦下
        let selfref = HubServer {
            id: "srv-self".to_string(),
            jump_id: "srv-self".to_string(),
            ..sample("self")
        };
        assert!(check_test_target(&selfref).is_err());
        let fine = HubServer {
            jump_id: "srv-other".to_string(),
            ..sample("ok")
        };
        assert!(check_test_target(&fine).is_ok());
    }

    #[test]
    fn askpass_decrypts_stored_password() {
        let dir = temp_dir("askpass");
        let key = crate::hub_vault::load_or_create_key(&dir).unwrap();
        let mut stored = sample("p");
        stored.id = "srv-askpass".to_string();
        stored.password_enc = crate::hub_vault::encrypt_with(&key, "s3cret 密码").unwrap();
        std::fs::write(
            dir.join(HUB_SERVERS_FILE),
            serde_json::to_string(&vec![stored]).unwrap(),
        )
        .unwrap();
        assert_eq!(askpass_password(&dir, "srv-askpass").unwrap(), "s3cret 密码");
        assert!(askpass_password(&dir, "srv-none").is_err());
    }

    #[test]
    fn askpass_prompt_parsing() {
        // 标准形态：user@host's password:
        assert_eq!(
            host_from_askpass_prompt("root@10.0.0.2's password: "),
            Some(("root".to_string(), "10.0.0.2".to_string()))
        );
        // 没有 user 前缀
        assert_eq!(
            host_from_askpass_prompt("10.0.0.2's password:"),
            Some((String::new(), "10.0.0.2".to_string()))
        );
        // IPv6：方括号在后面比对时剥
        assert_eq!(
            host_from_askpass_prompt("me@[fe80::1]'s password: "),
            Some(("me".to_string(), "[fe80::1]".to_string()))
        );
        // 密钥口令：不是密码提示，不能拿存档密码去答
        assert_eq!(host_from_askpass_prompt("Enter passphrase for key '/k':"), None);
        assert_eq!(host_from_askpass_prompt(""), None);
        assert_eq!(host_from_askpass_prompt("Password:"), None);
    }

    #[test]
    fn askpass_resolves_jump_by_prompt_host() {
        // 跳板机与目标机共用同一个 askpass 程序：只有提示语里的 host 能区分，
        // 这是 Windows 上不能给 ProxyCommand 塞 `env` 前缀之后的替代方案。
        let dir = temp_dir("askpass-prompt");
        let key = crate::hub_vault::load_or_create_key(&dir).unwrap();
        let mut jump = sample("jump");
        jump.id = "srv-jump".to_string();
        jump.host = "10.0.0.2".to_string();
        jump.password_enc = crate::hub_vault::encrypt_with(&key, "jump-pw").unwrap();
        let mut target = sample("target");
        target.id = "srv-target".to_string();
        target.host = "192.168.1.5".to_string();
        target.username = "me".to_string();
        target.password_enc = crate::hub_vault::encrypt_with(&key, "target-pw").unwrap();
        std::fs::write(
            dir.join(HUB_SERVERS_FILE),
            serde_json::to_string(&vec![target, jump.clone()]).unwrap(),
        )
        .unwrap();

        // 跳板的提示语 → 取跳板的密码（preferred 是目标机也不影响）
        assert_eq!(
            askpass_password_for_prompt(&dir, "srv-target", "root@10.0.0.2's password: ").unwrap(),
            "jump-pw"
        );
        // 目标的提示语 → 取目标的密码
        assert_eq!(
            askpass_password_for_prompt(&dir, "srv-target", "me@192.168.1.5's password: ").unwrap(),
            "target-pw"
        );
        // 认不出来的（密钥口令）→ 回落 preferred
        assert_eq!(
            askpass_password_for_prompt(&dir, "srv-target", "Enter passphrase for key 'x':")
                .unwrap(),
            "target-pw"
        );
        // host 大小写 / 结尾点不影响
        assert_eq!(
            askpass_password_for_prompt(&dir, "srv-target", "root@10.0.0.2.'s password: ").unwrap(),
            "jump-pw"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn password_new_none_keeps_and_empty_clears() {
        // 只验证 serde 语义：password_new 带 skip_serializing，绝不落盘/回传
        let mut stored = sample("q");
        stored.password_enc = "sealed".to_string();
        let json = serde_json::to_string(&stored).unwrap();
        assert!(!json.contains("password_new"));
        assert!(json.contains("\"password_enc\":\"sealed\""));
        // None 反序列化回来 = 不动已存密码
        let back: HubServer = serde_json::from_str(&json).unwrap();
        assert_eq!(back.password_new, None);
        assert_eq!(back.password_enc, "sealed");
    }
}
