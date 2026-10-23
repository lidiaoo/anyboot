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

use crate::hub::{normalize_mac, normalize_route_target, PowerRoute};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

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
    /// 每个地址对应的网卡 MAC（局域网电源包要用）：地址 → MAC 列表（小写、冒号分隔）。
    /// 只影响电源 / 唤醒，不参与连接。字段语义与手机端 `ServerProfile.macs` 一致。
    #[serde(default)]
    pub macs: std::collections::BTreeMap<String, Vec<String>>,
    /// 局域网电源路由（唤醒 / 重启 / 关机 / 睡眠 各发到哪个广播地址、哪个端口、
    /// 带哪块网卡的 MAC、发几个包、重试几次）。空 = 没配，走原来的老路。
    #[serde(default)]
    pub power_routes: Vec<PowerRoute>,
    /// 远程接入地址（单槽位）：`https://xxx.trycloudflare.com` 这类公网隧道地址。
    /// 局域网候选全都不通时才用它兜底（探测/HTTP/iframe 同一套语义）。
    #[serde(default)]
    pub remote_addr: String,
    /// 远程接入令牌：对方电脑给我们这台签发的「网关客户端令牌」，与它的局域网
    /// `token` 是两套（那边校验时按 ab_token 比对已配对客户端表）。
    #[serde(default)]
    pub remote_token: String,
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

/// 单个地址的探测结果（results 列表用：每个候选地址一条，成败与耗时都在）
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ProbeOutcome {
    pub address: String,
    pub ok: bool,
    pub ms: u64,
    pub error: String,
    /// true = 这一行是远程接入地址（https 隧道），不是局域网候选
    #[serde(default)]
    pub remote: bool,
}

/// remote_test 的结果：addr/ms 是选中的赢家；results 是全部候选地址的逐行结果
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct RemoteTestResult {
    pub ok: bool,
    pub addr: String,
    pub ms: u64,
    pub machine_id: String,
    pub machine_name: String,
    pub error: String,
    #[serde(default)]
    pub results: Vec<ProbeOutcome>,
    /// true = 赢家是远程接入地址（前端要用 https origin + ab_token 拼 URL）
    #[serde(default)]
    pub remote: bool,
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
            "parallel_all" => "parallel_all".to_string(),
            // parallel_first 是默认；旧值 parallel 与空值都归到它（兼容老配置）
            _ => "parallel_first".to_string(),
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
        // MAC：只留还存在的地址（地址删了 MAC 跟着丢），MAC 统一小写冒号、去重
        let mut macs: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
        for (address, values) in std::mem::take(&mut self.macs) {
            let (address, _) = split_address(&address);
            if !self.addresses.contains(&address) {
                continue;
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
        // 电源路由：动作认成四个之一、端口 0 补 9、广播地址去掉 scheme/路径、
        // MAC 归一去重；burst/retry 保留 0 = 「按动作默认」的语义
        let mut routes: Vec<PowerRoute> = Vec::new();
        for raw in std::mem::take(&mut self.power_routes) {
            let mut list: Vec<String> = Vec::new();
            for value in &raw.macs {
                if let Some(mac) = normalize_mac(value) {
                    if !list.contains(&mac) {
                        list.push(mac);
                    }
                }
            }
            routes.push(PowerRoute {
                action: raw.action_or_wake().to_string(),
                target: normalize_route_target(&raw.target),
                port: raw.effective_port(),
                macs: list,
                enabled: raw.enabled,
                burst: if raw.burst == 0 { 0 } else { raw.effective_burst() },
                retry: if raw.retry == 0 { 0 } else { raw.effective_retry() },
                // mode 只认四个合法值；http_fallback 按 mode 派生（与 hub.rs 同一套）
                mode: match raw.mode.as_str() {
                    "auto" | "http_first" | "http_only" | "packet_only" => raw.mode.clone(),
                    _ => String::new(),
                },
                http_fallback: matches!(raw.effective_mode(), "auto" | "http_first"),
            });
        }
        self.power_routes = routes;
        // 远程接入：地址 trim；整条 URL 里挖 ab_token（用户粘贴配对串时经常连
        // 令牌一起带进来）；令牌没单独填就用 URL 里那个
        self.remote_addr = self.remote_addr.trim().to_string();
        self.remote_token = self.remote_token.trim().to_string();
        if !self.remote_addr.is_empty() {
            let (origin, token_from_url) = split_remote_address(&self.remote_addr);
            if origin.is_empty() {
                self.remote_addr = String::new();
                self.remote_token = String::new();
            } else {
                self.remote_addr = origin;
                if self.remote_token.is_empty() {
                    self.remote_token = token_from_url.unwrap_or_default();
                }
            }
        } else {
            self.remote_token = String::new();
        }
        self
    }

    /// 这台电脑配过的所有 MAC（去重、按地址顺序）—— 电源包要用
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
    // 挖 token= / ab_token=（query 里第一个命中；大小写不敏感，对齐 http.rs 的宽松风格）
    let mut token = None;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if key == "token" || key == "ab_token" {
            let value = parts.next().unwrap_or("").trim().to_string();
            if !value.is_empty() {
                token = Some(value);
            }
            break;
        }
    }
    (authority, token)
}

/// 远程接入地址归一：`https://xxx.trycloudflare.com/?ab_token=t` 这类整条 URL、
/// 裸 `xxx.trycloudflare.com` 都吃。返回 (origin, token)：
/// - origin = `scheme://authority`（无 scheme 默认 `https://`；隧道与 ts.net 都是 https；
///   路径剥掉 —— 隧道地址没有路径语义）；
/// - token 从 query 的 `ab_token=` / `token=` 挖出（没有为 None）。
fn split_remote_address(raw: &str) -> (String, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    // 配对串形态：anyboot://pair?gateway=…&tunnel=https://…&token=… ——
    // 拆出 tunnel 与 token（值是 encode_query 转义过的，先解码），tunnel 再走一遍归一
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("anyboot://pair?") {
        let query = &raw[15..];
        let mut tunnel = None;
        let mut token = None;
        for pair in query.split('&') {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
            let value = percent_decode(parts.next().unwrap_or("").trim());
            match key.as_str() {
                "tunnel" if !value.is_empty() => tunnel = Some(value),
                "token" | "ab_token" if !value.is_empty() => token = Some(value),
                _ => {}
            }
        }
        return match tunnel {
            Some(t) => {
                let (origin, token_in_url) = split_remote_address(&t);
                (origin, token.or(token_in_url))
            }
            None => (String::new(), token),
        };
    }
    let lower = raw.to_ascii_lowercase();
    let (scheme, rest) = if lower.starts_with("https://") {
        ("https://", &raw[8..])
    } else if lower.starts_with("http://") {
        ("http://", &raw[7..])
    } else {
        ("https://", raw)
    };
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = rest[..end].trim();
    if authority.is_empty() {
        return (String::new(), None);
    }
    let query = match rest.find('?') {
        Some(index) => &rest[index + 1..],
        None => "",
    };
    let mut token = None;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if key == "token" || key == "ab_token" {
            let value = parts.next().unwrap_or("").trim().to_string();
            if !value.is_empty() {
                token = Some(value);
            }
            break;
        }
    }
    (format!("{scheme}{authority}"), token)
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

/// 远程地址（公网隧道 / ts.net）的探测与请求都走 ureq：它们是 https，
/// 手写 TCP 客户端做不了 TLS（本地局域网场景才用 probe_health）。
/// 超时放宽到 8s —— 公网 RTT 大，按局域网 3s 判会大量误报。
const REMOTE_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// ureq GET：返回 (状态码, 响应体)。4xx/5xx 也带回状态码由调用方解读。
fn ureq_get(url: &str, timeout: Duration) -> Result<(u16, String), String> {
    let response = ureq::get(url)
        .timeout(timeout)
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(code, response) => {
                let body = response.into_string().unwrap_or_default();
                format!("HTTP {code}: {}", body.trim().chars().take(200).collect::<String>())
            }
            other => format!("{other}"),
        })?;
    let status = response.status();
    let body = response.into_string().map_err(|error| format!("读响应失败: {error}"))?;
    Ok((status, body))
}

/// 对远程地址做 `GET {origin}/health`（免鉴权，与局域网探测同一协议）。
/// 成功返回 (耗时毫秒, machineId, machineName)。
fn probe_remote_health(origin: &str, timeout: Duration) -> Result<(u64, String, String), String> {
    let url = format!("{origin}/health");
    let started = Instant::now();
    let (_status, body) = ureq_get(&url, timeout)?;
    let (machine_id, machine_name) = parse_health(&body);
    let ms = started.elapsed().as_millis() as u64;
    Ok((ms, machine_id, machine_name))
}

/// 远程 HTTP 请求（带 ab_token 的完整 GET，走 ureq）：电源兜底 / 引导项读取
/// 出门时的那一条路。`path_with_query` 形如 `/api/power?action=…&token=…`
/// —— ab_token 由这里统一追加，调用方不用管。
fn remote_https_get(origin: &str, ab_token: &str, path_with_query: &str) -> Result<(u16, String), String> {
    let mut query = path_with_query.to_string();
    let ab_token = ab_token.trim();
    if !ab_token.is_empty() {
        query.push_str(if query.contains('?') { "&" } else { "?" });
        query.push_str("ab_token=");
        query.push_str(&encode_query(ab_token));
    }
    ureq_get(&format!("{origin}{query}"), Duration::from_secs(12))
}

// ==================== 局域网电源（路由 sol + HTTP 兜底） ====================

/// URL 查询串转义：保留字母数字和 -_.~，其余转 %XX（token 里出现 & %= 都不怕）
pub(crate) fn encode_query_pub(value: &str) -> String {
    encode_query(value)
}

/// encode_query 的逆：%XX 解回字节；坏转义原样保留。
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = bytes.get(index + 1..index + 3);
            match hex.and_then(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|text| u8::from_str_radix(text, 16).ok())
            }) {
                Some(byte) => {
                    out.push(byte);
                    index += 3;
                }
                None => {
                    out.push(bytes[index]);
                    index += 1;
                }
            }
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 探活地址兜底默认端口（跨模块复用：网关在线拉取要对 peer 地址补端口）。
pub(crate) fn with_default_port_pub(addr: &str) -> String {
    with_default_port(addr)
}

fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 调这台电脑自带的 AnyBoot 接口（`GET /api/power?action=..&token=..`）。
///
/// 前提是这台电脑还在线 —— 睡了 / 关了它的 HTTP 服务也没了。对「唤醒」这反而是
/// 有效的探测：请求能送达就说明机器醒着（服务端回 WakeAck，不做任何动作）。
/// 局域网地址全连不上时自动兜底远程接入地址（https 隧道 + ab_token）——
/// 出门在外时电源指令照样能发。
fn remote_http_power(server: &RemoteServer, action: &str) -> Result<String, String> {
    match remote_http_power_lan(server, action) {
        Ok(message) => Ok(message),
        Err(lan_error) => {
            let origin = server.remote_addr.trim();
            if origin.is_empty() {
                return Err(lan_error);
            }
            match remote_https_get(origin, &server.remote_token, &format!("/api/power?action={action}")) {
                Ok((status, body)) => parse_power_reply(status, &body)
                    .map(|message| format!("（经远程通道）{message}")),
                Err(remote_error) => Err(format!("{lan_error}；远程通道也不通：{remote_error}")),
            }
        }
    }
}

/// 电源接口回执解析（LAN / 远程共用）：401/4xx/非 JSON 的分支都收在这。
fn parse_power_reply(status: u16, body: &str) -> Result<String, String> {
    if status == 401 {
        return Err("这台电脑不认这个 token（401）—— 到「编辑」里核对访问令牌".to_string());
    }
    if status >= 400 {
        return Err(format!("这台电脑返回 {status}：{}", body.trim()));
    }
    if status == 0 {
        return Err("这台电脑的响应不是有效的 HTTP".to_string());
    }
    // 优先 message，其次 error，都不是 JSON 就把原文带回去
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body.trim()) {
        for key in ["message", "error"] {
            if let Some(text) = value.get(key).and_then(|item| item.as_str()) {
                if !text.trim().is_empty() {
                    return Ok(text.to_string());
                }
            }
        }
    }
    let fallback = body.trim();
    Ok(if fallback.is_empty() {
        "已提交".to_string()
    } else {
        fallback.to_string()
    })
}

/// 电源接口的局域网那一路（原 remote_http_power 主体，手写 TCP）。
fn remote_http_power_lan(server: &RemoteServer, action: &str) -> Result<String, String> {
    let address = server
        .addresses
        .iter()
        .map(|raw| raw.trim())
        .find(|raw| !raw.is_empty())
        .ok_or_else(|| "这台电脑还没有可用地址".to_string())?;
    let target = with_default_port(address);
    let mut query = format!("action={action}");
    let token = server.token.trim();
    if !token.is_empty() {
        query.push_str("&token=");
        query.push_str(&encode_query(token));
    }
    let mut stream = TcpStream::connect(&target)
        .map_err(|error| format!("连接 {target} 失败: {error}"))
        .and_then(|stream| {
            stream
                .set_read_timeout(Some(Duration::from_secs(15)))
                .map_err(|error| format!("设置读超时失败: {error}"))?;
            stream
                .set_write_timeout(Some(Duration::from_secs(4)))
                .map_err(|error| format!("设置写超时失败: {error}"))?;
            Ok(stream)
        })?;
    let request =
        format!("GET /api/power?{query} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("发送请求失败: {error}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("读取响应失败: {error}"))?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = match text.find("\r\n\r\n") {
        Some(index) => text[index + 4..].to_string(),
        None => String::new(),
    };
    parse_power_reply(status, &body)
}

/// 电脑档案的电源动作：按「局域网电源路由」发 sol 包；唤醒没配路由就按默认
/// （每块网卡 10 包 / 重试 10 次）广播魔术包，其余动作没配路由就调这台电脑
/// 自带的 AnyBoot 接口；配了但一个包都没发出去、且路由勾了「HTTP 兜底」
/// → 也改走它的接口（唤醒时 HTTP 能通 = 机器本来就醒着）。语义与手机端
/// `runPowerAction` 一致。
#[tauri::command]
pub async fn remote_power(
    app: AppHandle,
    server_id: String,
    action: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = load_store(&app)
            .ok()
            .and_then(|store| {
                store
                    .servers
                    .iter()
                    .find(|server| server.id == server_id)
                    .map(|server| server.name.clone())
            })
            .unwrap_or_else(|| "?".to_string());
        crate::hub_power::power_log(
            &app,
            &format!("remote_power 入口：电脑「{name}」({server_id}) 动作 {action}"),
        );
        let result = remote_power_blocking(&app, &server_id, &action);
        match &result {
            Ok(message) => crate::hub_power::power_log(&app, &format!("remote_power 完成：{message}")),
            Err(message) => crate::hub_power::power_log(&app, &format!("remote_power 失败：{message}")),
        }
        result
    })
    .await
    .map_err(|error| format!("电源操作任务失败：{error}"))?
}

fn remote_power_blocking(app: &AppHandle, id: &str, action: &str) -> Result<String, String> {
    let action = crate::hub::normalize_power_action(action)
        .ok_or_else(|| format!("未知电源动作：{action}"))?;
    let label = crate::hub_power::action_label(action);
    // 本机行：直连 IPC —— 电源动作直接走托盘/主窗口同一套进程内执行（spawn_power，
    // 忙闸/埋点/结果推送全都在），不回环 HTTP。唤醒对发号施令的这台没有意义，直接说明。
    if id == "local" {
        return match action {
            crate::hub::POWER_WAKE => Ok("本机正在运行，无需唤醒".to_string()),
            other => {
                let parsed = crate::power::PowerAction::parse(other)
                    .ok_or_else(|| format!("未知电源动作：{other}"))?;
                crate::power::spawn_power(app, parsed, true);
                Ok(format!("{}（已提交）", parsed.label()))
            }
        };
    }
    let server = load_store(app)?
        .servers
        .into_iter()
        .find(|candidate| candidate.id == id)
        .ok_or_else(|| format!("这台电脑不存在（可能已被删除）：{id}"))?;
    let jump = if server.jump_id.is_empty() {
        None
    } else {
        Some(
            crate::hub::load_servers_pub(app)?
                .into_iter()
                .find(|candidate| candidate.id == server.jump_id)
                .ok_or_else(|| "跳板机不存在（可能已被删除），请到服务器列表检查".to_string())?,
        )
    };
    let all_macs = server.all_macs();
    let routes: Vec<PowerRoute> = server
        .power_routes
        .iter()
        .filter(|route| {
            route.enabled && crate::hub::normalize_power_action(&route.action) == Some(action)
        })
        .cloned()
        .collect();
    if routes.is_empty() {
        if action == crate::hub::POWER_WAKE {
            // 没配唤醒路由：不调 HTTP（机器关着接口是死的），按默认广播魔术包 ——
            // 每块网卡 10 个 / 重试 10 次，目标 = 各地址的 /24 广播 + 全局广播
            // （与手机端 runPowerAction 的回落一致）。
            let default_route = PowerRoute {
                action: crate::hub::POWER_WAKE.to_string(),
                target: String::new(),
                port: 9,
                macs: Vec::new(),
                enabled: true,
                burst: 0,
                retry: 0,
                mode: String::new(),
                http_fallback: false,
            };
            let plan = crate::hub_power::plan_routes(
                crate::hub::POWER_WAKE,
                &server.addresses,
                &all_macs,
                std::slice::from_ref(&default_route),
            )?;
            let (_sent, where_from) = crate::hub_power::send_plan(app, jump.as_ref(), &plan)?;
            return Ok(format!(
                "（这台电脑没配「唤醒」路由，按默认每块网卡 10 个魔术包 / 重试 10 次广播）\
                 已从{where_from}发送：{}",
                plan.described.join(" · ")
            ));
        }
        let message = remote_http_power(&server, action)?;
        return Ok(format!(
            "（这台电脑没配「{label}」路由，走的是它自己的 AnyBoot 接口）{message}"
        ));
    }

    let mode = crate::hub::routes_mode(&routes);

    // 仅 HTTP：一个广播包都不发，直接调它自己的接口
    if mode == "http_only" {
        let message = remote_http_power(&server, action)?;
        return Ok(format!("（仅 HTTP 模式）已调它自己的 AnyBoot 接口：{message}"));
    }

    // 优先 HTTP：先调接口，通了就完事（一个广播包都不发）；没通退回广播包
    let mut http_first_error: Option<String> = None;
    if mode == "http_first" {
        match remote_http_power(&server, action) {
            Ok(message) => {
                return Ok(format!("（优先 HTTP 模式）已调它自己的 AnyBoot 接口：{message}"));
            }
            Err(http_error) => {
                http_first_error = Some(http_error);
            }
        }
    }

    // 混合：HTTP 接口与广播包**同时**发 —— HTTP 起独立线程，广播包走当前线程，
    // 互不等待；最后把两边结果合并汇报（哪边成了哪边败都说清楚）。
    if mode == "hybrid" {
        let plan = crate::hub_power::plan_routes(action, &server.addresses, &all_macs, &routes)?;
        let http_server = server.clone();
        let http_action = action.to_string();
        let http = std::thread::spawn(move || remote_http_power(&http_server, &http_action));
        let send_result = crate::hub_power::send_plan(app, jump.as_ref(), &plan);
        let http_result =
            http.join()
                .unwrap_or_else(|_| Err("HTTP 线程异常退出".to_string()));
        let packet_text = match &send_result {
            Ok((_sent, where_from)) => format!(
                "已从{where_from}按路由发送{label}：{}",
                plan.described.join(" · ")
            ),
            Err(error) => format!("广播包没发出去（{error}）"),
        };
        return match (&send_result, &http_result) {
            (Ok(_), Ok(http_message)) => Ok(format!(
                "（混合模式）HTTP 与广播包已同时发送 —— HTTP：{http_message}；广播包：{packet_text}"
            )),
            (Ok(_), Err(http_error)) => Ok(format!(
                "（混合模式）广播包已发出：{packet_text}；HTTP 接口失败：{http_error}"
            )),
            (Err(_), Ok(http_message)) => Ok(format!(
                "（混合模式）HTTP 接口已执行：{http_message}；{packet_text}"
            )),
            (Err(_), Err(http_error)) => Err(format!(
                "（混合模式）两边都失败 —— {packet_text}；HTTP：{http_error}"
            )),
        };
    }

    let plan = crate::hub_power::plan_routes(action, &server.addresses, &all_macs, &routes)?;
    match crate::hub_power::send_plan(app, jump.as_ref(), &plan) {
        Ok((_sent, where_from)) => {
            let mut message =
                format!("已从{where_from}按路由发送{label}：{}", plan.described.join(" · "));
            if let Some(http_error) = &http_first_error {
                message =
                    format!("（优先 HTTP 模式）接口没通（{http_error}），改发广播包 —— {message}");
            }
            if action != crate::hub::POWER_WAKE {
                message.push_str("。这个动作靠目标机上的 sol 监听该端口执行 —— 没装 sol 不会有反应");
            }
            if !plan.problems.is_empty() {
                message.push_str(&format!(
                    "（另有 {} 条路由被跳过：{}）",
                    plan.problems.len(),
                    plan.problems.join("；")
                ));
            }
            Ok(message)
        }
        Err(send_error) => {
            // 兜底条件：只有 auto（优先广播包）才回头调 HTTP。packet_only 明确不要；
            // http_first 开头已经试过一次 HTTP 了，不再重复，把两边错误一起报。
            let prefix = match &http_first_error {
                Some(http_error) => format!("（优先 HTTP 模式）接口没通（{http_error}）；"),
                None => String::new(),
            };
            if mode != "auto" {
                return Err(format!("{prefix}{send_error}"));
            }
            match remote_http_power(&server, action) {
                Ok(message) => Ok(format!(
                    "sol 包没发出去（{send_error}），已改走它自己的 AnyBoot 接口：{message}"
                )),
                Err(http_error) => {
                    Err(format!("sol 包没发出去（{send_error}）；HTTP 兜底也失败：{http_error}"))
                }
            }
        }
    }
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
    // listening / error：本机标签要能区分「服务没开」和「开了但端口绑不上」
    // （Windows 上 12300 随时可能落进 Hyper-V/WSL 的动态保留段，此时 bind 直接 WSAEACCES）
    let (listening, addr, error) = {
        let runtime = crate::http_server::http_lock();
        let listening = runtime.server.is_some();
        let addr = runtime
            .server
            .as_ref()
            .map(|server| server.addr.clone())
            .unwrap_or_default();
        let error = runtime.last_error.clone().unwrap_or_default();
        (listening, addr, error)
    };
    Ok(serde_json::json!({
        "port": settings.http_port,
        "token": settings.http_token,
        "enabled": settings.http_enabled,
        "listening": listening,
        "addr": addr,
        "error": error,
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
        // 远程令牌同语义：编辑弹窗不回显令牌，留空 = 保持已存的网关客户端令牌
        if server.remote_token.is_empty() {
            server.remote_token = existing.remote_token.clone();
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

/// 删除服务端（顺带收掉它的隧道子进程）。
/// **必须 async + spawn_blocking**：`kill_forward` 要 kill 再 wait 子进程。
#[tauri::command]
pub async fn remote_delete(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || remote_delete_blocking(app, id))
        .await
        .map_err(|error| format!("删除服务端任务失败: {error}"))?
}

/// 把探测成功的地址提到候选列表最前并落盘（下次 iframe / 页面加载优先用它）。
/// 地址本来就在首位时是空操作。
#[tauri::command]
pub fn remote_promote_address(
    app: AppHandle,
    server_id: String,
    address: String,
) -> Result<(), String> {
    let mut store = load_store(&app)?;
    let Some(server) = store.servers.iter_mut().find(|s| s.id == server_id) else {
        return Err("服务端不存在（可能已被删除）".to_string());
    };
    let Some(index) = server.addresses.iter().position(|a| *a == address) else {
        return Err(format!("地址 {address} 不在这台服务端的候选列表里"));
    };
    if index > 0 {
        let picked = server.addresses.remove(index);
        server.addresses.insert(0, picked);
        save_store(&app, &store)?;
    }
    Ok(())
}

fn remote_delete_blocking(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<RemoteJumpState>();
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

/// 探测模式归一：serial（按顺序）/ parallel_all（并发等全部）/ parallel_first
/// （并发取首个成功；旧值 parallel 与空值都算它）
pub(crate) fn normalize_probe_mode(mode: &str) -> &'static str {
    match mode.trim() {
        "serial" => "serial",
        "parallel_all" => "parallel_all",
        _ => "parallel_first",
    }
}

/// 探测一台电脑的所有候选地址。三种模式：
/// - **并发（首个）**：每个地址一个线程，**任一响应立即胜出**（done 旗标），
///   其余线程探完作废 —— 用户要的是「最快哪个能用」；
/// - **并发（全部）**：等全部线程结束，取最快的成功者，逐行结果随 results 返回；
/// - **串行**：按顺序逐个试，第一个成功就停。
/// results 里每个候选地址一条（成败 + 耗时），前端逐行展示。
/// `GET /health` 免鉴权。**必须 async + spawn_blocking**：要 join 并发线程 / 等 TCP 超时。
///
/// `remote_addr`（可选，单槽位远程接入地址）：**局域网优先、远程兜底** ——
/// 局域网有赢家就不探远程（results 里补一行「未探测」说明）；局域网全挂才探
/// 远程（8s 超时），探通则赢家就是远程地址（结果带 `remote: true`，前端按
/// https + ab_token 拼 iframe）。没有远程地址时行为与老版本完全一致。
#[tauri::command]
pub async fn remote_test(
    addresses: Vec<String>,
    probe_mode: String,
    remote_addr: Option<String>,
) -> RemoteTestResult {
    tauri::async_runtime::spawn_blocking(move || {
        remote_test_blocking(addresses, probe_mode, remote_addr)
    })
    .await
    .unwrap_or_else(|error| RemoteTestResult {
        error: format!("探测任务失败: {error}"),
        ..Default::default()
    })
}

fn remote_test_blocking(
    addresses: Vec<String>,
    probe_mode: String,
    remote_addr: Option<String>,
) -> RemoteTestResult {
    let mode = normalize_probe_mode(&probe_mode);
    let remote_origin = remote_addr
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .map(|raw| split_remote_address(raw).0)
        .filter(|origin| !origin.is_empty());
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
    if candidates.is_empty() && remote_origin.is_none() {
        return RemoteTestResult {
            error: "没有可探测的地址".to_string(),
            ..Default::default()
        };
    }

    let mut results: Vec<ProbeOutcome> = Vec::new();
    let mut best: Option<RemoteTestResult> = None;
    if mode == "serial" {
        // 串行：按顺序逐个试，第一个成功就停（剩下的地址不探）
        for addr in &candidates {
            match probe_health(addr, PROBE_TIMEOUT) {
                Ok((ms, machine_id, machine_name)) => {
                    results.push(ProbeOutcome {
                        address: addr.clone(),
                        ok: true,
                        ms,
                        error: String::new(),
                        remote: false,
                    });
                    best = Some(RemoteTestResult {
                        ok: true,
                        addr: addr.clone(),
                        ms,
                        machine_id,
                        machine_name,
                        ..Default::default()
                    });
                    break;
                }
                Err(error) => results.push(ProbeOutcome {
                    address: addr.clone(),
                    ok: false,
                    ms: 0,
                    error,
                    remote: false,
                }),
            }
        }
    } else {
        // 并发：每个地址一个线程。首个 = 谁先成功谁赢（done 旗标，后到的成功作废）；
        // 全部 = 等所有线程结束，取最快的成功者。
        let first_only = mode == "parallel_first";
        let done = Arc::new(AtomicBool::new(false));
        let winner: Arc<Mutex<Option<RemoteTestResult>>> = Arc::new(Mutex::new(None));
        let outcomes: Arc<Mutex<Vec<ProbeOutcome>>> = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for addr in &candidates {
            let addr = addr.clone();
            let (done, winner, outcomes) =
                (Arc::clone(&done), Arc::clone(&winner), Arc::clone(&outcomes));
            handles.push(std::thread::spawn(move || {
                if first_only && done.load(Ordering::Relaxed) {
                    return; // 已经有赢家：不再白探（在途的等它自己超时收尾）
                }
                let outcome = match probe_health(&addr, PROBE_TIMEOUT) {
                    Ok((ms, machine_id, machine_name)) => {
                        if first_only {
                            if !done.swap(true, Ordering::SeqCst) {
                                *winner.lock().unwrap() = Some(RemoteTestResult {
                                    ok: true,
                                    addr: addr.clone(),
                                    ms,
                                    machine_id,
                                    machine_name,
                                    ..Default::default()
                                });
                            }
                        } else {
                            let mut guard = winner.lock().unwrap();
                            let better = match guard.as_ref() {
                                Some(current) => ms < current.ms,
                                None => true,
                            };
                            if better {
                                *guard = Some(RemoteTestResult {
                                    ok: true,
                                    addr: addr.clone(),
                                    ms,
                                    machine_id,
                                    machine_name,
                                    ..Default::default()
                                });
                            }
                        }
                        ProbeOutcome {
                            address: addr.clone(),
                            ok: true,
                            ms,
                            error: String::new(),
                            remote: false,
                        }
                    }
                    Err(error) => ProbeOutcome {
                        address: addr.clone(),
                        ok: false,
                        ms: 0,
                        error,
                        remote: false,
                    },
                };
                outcomes.lock().unwrap().push(outcome);
            }));
        }
        for handle in handles {
            let _ = handle.join();
        }
        results = outcomes.lock().unwrap().clone();
        // 按候选顺序排回，方便用户逐行对照地址表
        results.sort_by_key(|o| {
            candidates
                .iter()
                .position(|a| *a == o.address)
                .unwrap_or(usize::MAX)
        });
        best = winner.lock().unwrap().clone();
    }
    let mut final_result = best.unwrap_or(RemoteTestResult {
        error: results
            .iter()
            .find(|o| !o.error.is_empty())
            .map(|o| o.error.clone())
            .unwrap_or_else(|| "全部地址都连不上".to_string()),
        ..Default::default()
    });
    // 局域网有赢家：远程不探（省一次 8s 级公网等待），results 里补一行说明
    if let Some(origin) = &remote_origin {
        if final_result.ok {
            results.push(ProbeOutcome {
                address: origin.clone(),
                ok: false,
                ms: 0,
                error: "局域网可用，远程地址未探测".to_string(),
                remote: true,
            });
        } else {
            match probe_remote_health(origin, REMOTE_PROBE_TIMEOUT) {
                Ok((ms, machine_id, machine_name)) => {
                    results.push(ProbeOutcome {
                        address: origin.clone(),
                        ok: true,
                        ms,
                        error: String::new(),
                        remote: true,
                    });
                    final_result = RemoteTestResult {
                        ok: true,
                        addr: origin.clone(),
                        ms,
                        machine_id,
                        machine_name,
                        remote: true,
                        ..Default::default()
                    };
                }
                Err(error) => results.push(ProbeOutcome {
                    address: origin.clone(),
                    ok: false,
                    ms: 0,
                    error,
                    remote: true,
                }),
            }
        }
    }
    // 逐行结果无条件带回：前端并发（全部）模式要逐行展示每个地址的成败
    final_result.results = results;
    final_result
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
/// **必须 async + spawn_blocking**：没现成转发时要起 ssh 子进程并等它就绪。
#[tauri::command]
pub async fn remote_endpoint(app: AppHandle, server_id: String) -> Result<RemoteEndpoint, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<RemoteJumpState>();
        remote_endpoint_inner(&app, state.inner(), &server_id)
    })
    .await
    .map_err(|error| format!("获取端点任务失败: {error}"))?
}

/// 经跳板机探测：先确保本地转发，再对 127.0.0.1:port 做 /health 探测。
#[tauri::command]
pub async fn remote_test_via(app: AppHandle, server_id: String) -> RemoteTestResult {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<RemoteJumpState>();
        remote_test_via_blocking(&app, state.inner(), &server_id)
    })
    .await
    .unwrap_or_else(|error| RemoteTestResult {
        error: format!("探测任务失败: {error}"),
        ..Default::default()
    })
}

fn remote_test_via_blocking(
    app: &AppHandle,
    state: &RemoteJumpState,
    server_id: &str,
) -> RemoteTestResult {
    let endpoint = match remote_endpoint_inner(app, state, server_id) {
        Ok(ep) => ep,
        Err(error) => {
            return RemoteTestResult {
                error,
                ..Default::default()
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
            ..Default::default()
        },
        Err(error) => RemoteTestResult {
            error,
            ..Default::default()
        },
    }
}

// ==================== 引导项（rEFInd / GRUB 远程读取与切换） ====================

/// 一个可选的引导项。`value` 是回传给服务端的标识：
/// rEFInd 用 `system`（linux/windows/macos），GRUB 用 `selector`（可能含空格/`>`）。
#[derive(Clone, Debug, Serialize, Default, PartialEq)]
pub struct BootOption {
    pub value: String,
    pub label: String,
    /// 图标分类：mac / win / linux / auto（前端取色点用）
    pub class: String,
    /// 组头行（`value` 恒空、不可选）：GRUB 下拉的「▸ 组名（N 项…）」，
    /// 与托盘菜单的组头同一规则 —— 前端渲染成小标题而不是选项。
    #[serde(default)]
    pub header: bool,
}

/// 一台电脑上的一种引导管理器（rEFInd 或 GRUB）的现状 + 可选项。
#[derive(Clone, Debug, Serialize, Default, PartialEq)]
pub struct BootGroup {
    /// refind / grub
    pub kind: String,
    /// 界面上显示的名字：rEFInd / GRUB
    pub title: String,
    /// 这台电脑上是否真的部署了它（false = 显示「未检测到」）
    pub available: bool,
    /// 下次启动会进哪个（rEFInd = active 的 system；GRUB = nextEntry，缺省回 savedEntry）
    pub current: String,
    /// 下次启动的值（GRUB 独有：与入口菜单里那个「已保存」区分开，用来判断待生效）
    pub next: String,
    /// 当前生效/上次生效的值
    pub saved: String,
    /// 不可用或读取失败时的说明（前端挂在 title 上）
    pub note: String,
    pub options: Vec<BootOption>,
}

/// 电脑页「引导项」列的数据：一次请求拿齐 rEFInd + GRUB。
#[derive(Clone, Debug, Serialize, Default, PartialEq)]
pub struct RemoteBootInfo {
    pub ok: bool,
    /// 这台电脑当前探得通吗（离线时两个引导项一律不可改）
    pub online: bool,
    /// 探活赢家地址（展示用）
    pub addr: String,
    pub error: String,
    pub refind: BootGroup,
    pub grub: BootGroup,
}

/// GRUB 条目名 → 图标分类（纯函数，便于单测）。
fn grub_class(name: &str, class: &str, chainloader: bool) -> String {
    let haystack = format!("{name} {class}").to_ascii_lowercase();
    if haystack.contains("windows") || haystack.contains("win") {
        return "win".to_string();
    }
    if haystack.contains("osx") || haystack.contains("mac") || haystack.contains("apple") {
        return "mac".to_string();
    }
    if chainloader && haystack.contains("efi") {
        return "auto".to_string();
    }
    "linux".to_string()
}

/// rEFInd 的 system → 图标分类
fn refind_class(system: &str) -> String {
    match system {
        "windows" => "win".to_string(),
        "macos" => "mac".to_string(),
        _ => "linux".to_string(),
    }
}

/// 这台电脑的 HTTP 端点（直连取首个非空地址；经跳板机复用本地转发）。
fn server_endpoint(app: &AppHandle, server: &RemoteServer) -> Result<String, String> {
    if server.jump_id.trim().is_empty() {
        let address = server
            .addresses
            .iter()
            .map(|raw| raw.trim())
            .find(|raw| !raw.is_empty())
            .ok_or_else(|| "这台电脑还没有可用地址".to_string())?;
        return Ok(with_default_port(address));
    }
    let state = app.state::<RemoteJumpState>();
    let endpoint = remote_endpoint_inner(app, state.inner(), &server.id)?;
    Ok(format!("{}:{}", endpoint.host, endpoint.port))
}

/// 对这台电脑发一条 `GET {path}?token=…`，返回解析后的 JSON。
///
/// 与 `remote_http_power` 同一套手写 HTTP（不引 reqwest）：局域网场景只需要
/// 一条短请求，多一个依赖不划算。**必须跑在 spawn_blocking 里** —— 会阻塞在 TCP 上。
/// 局域网失败时兜底远程接入地址（https + ab_token）：出门时引导项读取/切换照常可用。
fn remote_http_json(
    app: &AppHandle,
    server: &RemoteServer,
    path: &str,
) -> Result<serde_json::Value, String> {
    match remote_http_json_lan(app, server, path) {
        Ok(value) => Ok(value),
        Err(lan_error) => {
            let origin = server.remote_addr.trim();
            if origin.is_empty() {
                return Err(lan_error);
            }
            match remote_https_get(origin, &server.remote_token, path) {
                Ok((status, body)) => parse_remote_json_reply(status, &body),
                Err(remote_error) => Err(format!("{lan_error}；远程通道也不通：{remote_error}")),
            }
        }
    }
}

/// 远程 JSON 回执解析（401 文案按「远程令牌」口径说，与局域网区分开）。
fn parse_remote_json_reply(status: u16, body: &str) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = serde_json::from_str(body.trim())
        .map_err(|error| format!("这台电脑返回的不是合法 JSON（{status}）：{error}"))?;
    if status == 401 {
        return Err("远程通道令牌无效（401）—— 请重新配对或到「编辑」里核对远程接入令牌".to_string());
    }
    if status >= 400 {
        let message = value
            .get("message")
            .and_then(|item| item.as_str())
            .unwrap_or("请求被拒绝");
        return Err(message.to_string());
    }
    Ok(value)
}

/// remote_http_json 的局域网那一路（手写 TCP，原主体）。
fn remote_http_json_lan(
    app: &AppHandle,
    server: &RemoteServer,
    path: &str,
) -> Result<serde_json::Value, String> {
    let target = server_endpoint(app, server)?;
    let token = server.token.trim();
    let mut query = String::new();
    if !token.is_empty() {
        query.push_str(if path.contains('?') { "&" } else { "?" });
        query.push_str("token=");
        query.push_str(&encode_query(token));
    }
    let mut stream = TcpStream::connect(&target)
        .map_err(|error| format!("连接 {target} 失败: {error}"))
        .and_then(|stream| {
            stream
                .set_read_timeout(Some(Duration::from_secs(8)))
                .map_err(|error| format!("设置读超时失败: {error}"))?;
            stream
                .set_write_timeout(Some(Duration::from_secs(4)))
                .map_err(|error| format!("设置写超时失败: {error}"))?;
            Ok(stream)
        })?;
    let request =
        format!("GET {path}{query} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("发送请求失败: {error}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("读取响应失败: {error}"))?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = match text.find("\r\n\r\n") {
        Some(index) => text[index + 4..].to_string(),
        None => String::new(),
    };
    if status == 401 {
        return Err("这台电脑不认这个 token（401）—— 到「编辑」里核对访问令牌".to_string());
    }
    if status == 0 {
        return Err("这台电脑的响应不是有效的 HTTP".to_string());
    }
    let value: serde_json::Value = serde_json::from_str(body.trim())
        .map_err(|error| format!("这台电脑返回的不是合法 JSON（{status}）：{error}"))?;
    if status >= 400 {
        let message = value
            .get("message")
            .and_then(|item| item.as_str())
            .unwrap_or("请求被拒绝");
        return Err(message.to_string());
    }
    Ok(value)
}

/// 命令通道（/api/grub-switch、/api/power…）的回执解析：{ok, pending, channel, message}。
fn command_reply_message(reply: &serde_json::Value, fallback: &str) -> Result<String, String> {
    if let Some(message) = reply
        .get("message")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Ok(message.to_string());
    }
    let ok = reply.get("ok").and_then(|item| item.as_bool()).unwrap_or(true);
    if !ok {
        return Err(reply
            .get("error")
            .and_then(|item| item.as_str())
            .unwrap_or(fallback)
            .to_string());
    }
    Ok(fallback.to_string())
}

/// 把 `/api/status` 的 variables 收成 rEFInd 的可选组。
fn refind_from_status(value: &serde_json::Value) -> BootGroup {
    let mut options = Vec::new();
    let mut current = String::new();
    for variable in value
        .get("variables")
        .and_then(|item| item.as_array())
        .map(|list| list.as_slice())
        .unwrap_or_default()
    {
        let exists = variable
            .get("exists")
            .and_then(|item| item.as_bool())
            .unwrap_or(false);
        if !exists {
            continue;
        }
        let system = variable
            .get("system")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if system.is_empty() {
            continue;
        }
        let label = variable
            .get("displayName")
            .and_then(|item| item.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or(&system)
            .to_string();
        let active = variable
            .get("active")
            .and_then(|item| item.as_bool())
            .unwrap_or(false);
        if active {
            current = system.clone();
        }
        options.push(BootOption {
            value: system.clone(),
            label,
            class: refind_class(&system),
            header: false,
        });
    }
    BootGroup {
        kind: "refind".to_string(),
        title: "rEFInd".to_string(),
        available: !options.is_empty(),
        current: current.clone(),
        next: current.clone(),
        saved: current,
        note: if options.is_empty() {
            "这台电脑没部署 rEFInd（或还没设置过下次启动项）".to_string()
        } else {
            String::new()
        },
        options,
    }
}

/// 出一条 GRUB 可选项 —— 与托盘菜单的 `menu_model::grub_push_entry` **同一条拼装
/// 规则**，只是产物从菜单节点换成 BootOption：
/// * 文字随格式变（`grub_row_label`：极简/简易=去前缀内核名、中等=完整内核名、
///   原始/复杂=标题原文）；内核为空时退回标题原文（不猜）；
/// * 非原始格式按渲染结果折叠同名条目（首个出面、其余并成「· N 个变体」）；
///   原始格式不折叠，变体数用解析期并进的 `entry.variants`；
/// * 原始/复杂在行尾附内核名（或「转交引导」/class）；
/// * 行文字 = 箭头（子项 ↳）+ 系统图标 +（组前缀）+ 格式化文字 + 附注。
#[allow(clippy::too_many_arguments)]
fn push_grub_option(
    options: &mut Vec<BootOption>,
    format: &str,
    entry: &crate::grub::GrubEntry,
    top: bool,
    group_prefix: &str,
    counts: &std::collections::HashMap<String, usize>,
    emitted: &mut std::collections::HashSet<String>,
) {
    let label = crate::menu_model::grub_row_label(format, entry);
    let mut parts: Vec<String> = Vec::new();
    if format == "raw" {
        // 原始格式不折叠；变体数在解析期并进了 entry.variants
        let variants = entry.variants.max(1);
        if variants > 1 {
            parts.push(format!("{variants} 个变体"));
        }
    } else {
        if !emitted.insert(label.clone()) {
            return;
        }
        let variants = counts.get(&label).copied().unwrap_or(1);
        if variants > 1 {
            parts.push(format!("{variants} 个变体"));
        }
    }
    if format == "raw" || format == "detailed" {
        let tag = if !entry.kernels.is_empty() {
            entry.kernels[0].clone()
        } else if entry.chainloader {
            "转交引导".to_string()
        } else {
            entry.class.clone().unwrap_or_default()
        };
        if !tag.is_empty() {
            parts.push(tag);
        }
    }
    let suffix = if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    };
    let icon = crate::menu_model::grub_os_icon(&format!("{group_prefix}{}", entry.name));
    let arrow = if top { "" } else { "↳ " };
    options.push(BootOption {
        value: entry.selector.clone(),
        label: format!("{arrow}{icon}{group_prefix}{label}{suffix}"),
        class: grub_class(
            &entry.name,
            entry.class.as_deref().unwrap_or(""),
            entry.chainloader,
        ),
        header: false,
    });
}

/// GRUB 子菜单的 id → 组名。
///
/// `/api/grub` 的 `submenu` 字段是 `gnulinux-advanced-9f1bae08-…` 这种内部 id，
/// 真正的组名（「Advanced options for …」）在 `groups` 里。GRUB 列表页与托盘菜单
/// 都会把 id 翻成组名 —— 电脑列表这里也必须翻，否则下拉里一排 uuid 前缀。
fn grub_submenu_titles(value: &serde_json::Value) -> std::collections::HashMap<String, String> {
    let mut titles = std::collections::HashMap::new();
    for group in value
        .get("groups")
        .and_then(|item| item.as_array())
        .map(|list| list.as_slice())
        .unwrap_or_default()
    {
        let id = group
            .get("id")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim();
        let name = group
            .get("name")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim();
        if !id.is_empty() || !name.is_empty() {
            titles.insert(id.to_string(), name.to_string());
        }
    }
    titles
}

/// 把 `/api/grub` 的 entries 收成 GRUB 的可选组 —— **结构与拼装规则和托盘菜单
/// （`menu_model::grub_menu_nodes`）逐条对齐**：
/// * 当前选中项置顶（组内子项带「组名 › 」前缀），原位照常保留；
/// * 每个 submenu 一行组头「▸ 图标 组名（N 项[ · ESP 位置]）」，位置按格式带不带
///   （极简/原始不带）；组名翻自 `groups`（老服务端没有 groups 时退回内部 id）；
/// * 子项「↳ 」缩进；顶层与各组各有一套折叠状态（非原始格式折叠同名变体）；
/// * 条目文字随格式变（`push_grub_option`，与 `grub_row_label` 同一规则）。
fn grub_from_json(value: &serde_json::Value, format: &str) -> BootGroup {
    let next = value
        .get("nextEntry")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let saved = value
        .get("savedEntry")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let installed = value
        .get("installed")
        .and_then(|item| item.as_bool())
        .unwrap_or(false);
    let format = crate::settings_commands::normalize_grub_list_format(format);
    let active = if next.is_empty() {
        saved.clone()
    } else {
        next.clone()
    };

    // 条目直接反序列化成 GrubEntry —— 拼装助手（grub_row_label / grub_collapse）
    // 只吃这个类型，反序列化回来就能与托盘菜单共用同一套代码。老服务端不发
    // variants（甚至 kernels），serde(default) 兜住；坏条目跳过。
    // 内核名里的空白项清掉 —— grub_kernel_of 只对「空 Vec」回落标题，
    // 空串内核会渲染出一行空白。
    let mut entries: Vec<crate::grub::GrubEntry> = Vec::new();
    if let Some(list) = value.get("entries").and_then(|item| item.as_array()) {
        for item in list {
            let Ok(mut entry) = serde_json::from_value::<crate::grub::GrubEntry>(item.clone())
            else {
                continue;
            };
            if entry.selector.trim().is_empty() {
                continue;
            }
            entry.kernels.retain(|kernel| !kernel.trim().is_empty());
            entries.push(entry);
        }
    }

    let group_titles = grub_submenu_titles(value);
    let mut options: Vec<BootOption> = Vec::new();

    // 置顶当前选中项（组内子项补「组名 › 」前缀 —— 与托盘/主窗置顶行同一规则）
    if let Some(pinned) = entries.iter().find(|entry| entry.selector == active) {
        let prefix = pinned
            .submenu
            .as_deref()
            .and_then(|id| group_titles.get(id))
            .map(|title| format!("{title} › "))
            .unwrap_or_default();
        push_grub_option(
            &mut options,
            format,
            pinned,
            true,
            &prefix,
            &std::collections::HashMap::new(),
            &mut std::collections::HashSet::new(),
        );
    }

    // 组内子项索引（与托盘同一份 children_of）；空串 submenu（老数据/手写 cfg）
    // 按「没有父级」算 —— 真服务端顶层条目的 submenu 是 null。
    let mut children_of: std::collections::HashMap<&str, Vec<&crate::grub::GrubEntry>> =
        std::collections::HashMap::new();
    for entry in &entries {
        if let Some(submenu) = entry.submenu.as_deref().filter(|id| !id.is_empty()) {
            children_of.entry(submenu).or_default().push(entry);
        }
    }

    // 组头附加的 ESP 位置（简易/中等/复杂带，极简/原始不带 —— 与托盘同一规则）
    let location = if format == "minimal" || format == "raw" {
        String::new()
    } else {
        let mut parts: Vec<String> = Vec::new();
        let dir = value
            .get("efiDir")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim();
        if !dir.is_empty() {
            parts.push(format!("位于{dir}"));
        }
        let uuid = value
            .get("partuuid")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim();
        if !uuid.is_empty() {
            parts.push(format!("UUID={uuid}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" · {}", parts.join(" "))
        }
    };

    // 主体：顶层一套折叠状态，各组独立一套（与托盘一致）；原始格式不折叠
    let mut emitted_groups = std::collections::HashSet::new();
    let top_counts = crate::menu_model::grub_collapse(
        format,
        entries.iter().filter(|entry| entry.submenu.is_none()),
    );
    let mut top_emitted = std::collections::HashSet::new();
    for entry in &entries {
        match entry.submenu.as_deref().filter(|id| !id.is_empty()) {
            None => {
                push_grub_option(
                    &mut options,
                    format,
                    entry,
                    true,
                    "",
                    &top_counts,
                    &mut top_emitted,
                );
            }
            Some(submenu_id) => {
                if !emitted_groups.insert(submenu_id) {
                    continue;
                }
                let title = group_titles
                    .get(submenu_id)
                    .cloned()
                    .unwrap_or_else(|| submenu_id.to_string());
                let count = children_of.get(submenu_id).map(Vec::len).unwrap_or(0);
                options.push(BootOption {
                    value: String::new(),
                    label: format!(
                        "▸ {}{title}（{count} 项{location}）",
                        crate::menu_model::grub_os_icon(&title)
                    ),
                    class: String::new(),
                    header: true,
                });
                let counts = crate::menu_model::grub_collapse(
                    format,
                    children_of.get(submenu_id).into_iter().flatten().copied(),
                );
                let mut emitted = std::collections::HashSet::new();
                for child in children_of.get(submenu_id).into_iter().flatten() {
                    push_grub_option(&mut options, format, child, false, "", &counts, &mut emitted);
                }
            }
        }
    }
    let note = if options.is_empty() {
        let raw = value
            .get("error")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim();
        if raw.is_empty() {
            "这台电脑没部署 GRUB（或还没同步 grub.cfg）".to_string()
        } else {
            raw.to_string()
        }
    } else {
        String::new()
    };
    BootGroup {
        kind: "grub".to_string(),
        title: "GRUB".to_string(),
        available: installed || !options.is_empty(),
        current: if next.is_empty() {
            saved.clone()
        } else {
            next.clone()
        },
        next,
        saved,
        note,
        options,
    }
}

/// 电脑页「引导项」列：读这台电脑的 rEFInd 与 GRUB 现状。
///
/// **必须 async + spawn_blocking**：探活 + 两次 HTTP 都要等网络。
#[tauri::command]
pub async fn remote_boot_info(
    app: AppHandle,
    server_id: String,
) -> Result<RemoteBootInfo, String> {
    tauri::async_runtime::spawn_blocking(move || remote_boot_info_blocking(&app, &server_id))
        .await
        .map_err(|error| format!("读取引导项任务失败：{error}"))?
}

fn remote_boot_info_blocking(app: &AppHandle, server_id: &str) -> Result<RemoteBootInfo, String> {
    // 本机行（电脑列表的「本机」固定行）：它就在本进程里，直连 IPC ——
    // 直接调用 /api/status、/api/grub 的进程内生成函数（remote_status_json /
    // remote_grub_json），不走回环 HTTP：服务端没起（端口被保留段盖住）也照读不误，
    // 解析逻辑（refind_from_status / grub_from_json）原样复用。
    if server_id == "local" {
        let status = crate::remote_routing::remote_status_json(app);
        let grub_json = crate::remote_routing::remote_grub_json(app);
        let format = crate::read_settings_file(app).grub_list_format;
        let mut info = RemoteBootInfo {
            refind: refind_from_status(&status),
            grub: grub_from_json(&grub_json, &format),
            ..Default::default()
        };
        info.ok = true;
        info.online = true;
        info.addr = "127.0.0.1".to_string();
        return Ok(info);
    }
    let server = load_store(app)?
        .servers
        .into_iter()
        .find(|candidate| candidate.id == server_id)
        .ok_or_else(|| format!("这台电脑不存在（可能已被删除）：{server_id}"))?;
    let mut info = RemoteBootInfo {
        refind: BootGroup {
            kind: "refind".to_string(),
            title: "rEFInd".to_string(),
            ..Default::default()
        },
        grub: BootGroup {
            kind: "grub".to_string(),
            title: "GRUB".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    // 先探活：离线时没必要再发两条注定超时的请求（等待体验差 3 倍）
    // 局域网探不通时兜底远程接入地址（出门场景：引导项照常可读）。
    let probe_addr = server
        .addresses
        .iter()
        .map(|raw| raw.trim())
        .find(|raw| !raw.is_empty())
        .unwrap_or("")
        .to_string();
    let lan_probe = if probe_addr.is_empty() {
        Err("这台电脑还没有可用地址".to_string())
    } else if server.jump_id.trim().is_empty() {
        probe_health(&probe_addr, PROBE_TIMEOUT)
    } else {
        let state = app.state::<RemoteJumpState>();
        let endpoint = remote_endpoint_inner(app, state.inner(), server_id)?;
        probe_health(&format!("{}:{}", endpoint.host, endpoint.port), PROBE_TIMEOUT)
    };
    let (probe, winner_addr) = match lan_probe {
        Ok(result) => (Ok(result), probe_addr),
        Err(lan_error) => {
            let origin = server.remote_addr.trim();
            if origin.is_empty() {
                (Err(lan_error), probe_addr)
            } else {
                match probe_remote_health(origin, REMOTE_PROBE_TIMEOUT) {
                    Ok(result) => (Ok(result), origin.to_string()),
                    Err(remote_error) => (
                        Err(format!("{lan_error}；远程通道也不通：{remote_error}")),
                        probe_addr,
                    ),
                }
            }
        }
    };
    match probe {
        Ok(_) => {
            info.online = true;
            info.addr = winner_addr;
        }
        Err(error) => {
            info.online = false;
            info.error = error;
            info.refind.note = "电脑离线，无法读取引导项".to_string();
            info.grub.note = "电脑离线，无法读取引导项".to_string();
            return Ok(info);
        }
    }
    // 两个接口各自失败互不影响：一边能读就先把那边显示出来
    let mut errors: Vec<String> = Vec::new();
    match remote_http_json(app, &server, "/api/status") {
        Ok(value) => info.refind = refind_from_status(&value),
        Err(error) => {
            info.refind.note = error.clone();
            errors.push(format!("rEFInd：{error}"));
        }
    }
    match remote_http_json(app, &server, "/api/grub") {
        // 条目文字按本机设置里的「GRUB 列表格式」生成（与 GRUB 列表页/托盘菜单一致）
        Ok(value) => {
            let format = crate::read_settings_file(app).grub_list_format;
            info.grub = grub_from_json(&value, &format);
            // 排查用（受 debugTrace 开关控制）：这次按哪档生成、前三条长什么样
            crate::trace(
                app,
                "boot",
                &format!(
                    "grub format={format} server={server_id} labels={:?}",
                    info.grub
                        .options
                        .iter()
                        .take(3)
                        .map(|option| option.label.as_str())
                        .collect::<Vec<_>>()
                ),
            );
        }
        Err(error) => {
            info.grub.note = error.clone();
            errors.push(format!("GRUB：{error}"));
        }
    }
    info.ok = errors.len() < 2;
    if !errors.is_empty() {
        info.error = errors.join("；");
    }
    Ok(info)
}

/// 切换引导项：rEFInd 走 `/api/switch?system=`，GRUB 走 `/api/grub-switch?entry=`。
///
/// **必须 async + spawn_blocking**：同上（等网络）。
#[tauri::command]
pub async fn remote_boot_switch(
    app: AppHandle,
    server_id: String,
    kind: String,
    value: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        remote_boot_switch_blocking(&app, &server_id, &kind, &value)
    })
    .await
    .map_err(|error| format!("切换引导项任务失败：{error}"))?
}

fn remote_boot_switch_blocking(
    app: &AppHandle,
    server_id: &str,
    kind: &str,
    value: &str,
) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("没有选择引导项".to_string());
    }
    let kind = kind.trim();
    // 本机行：直连 IPC —— 直接调远程命令的进程内执行入口（与 CLI/HTTP 走的
    // 是同一个 execute_remote_command，落到 switch_and_update / switch_grub_and_update）
    if server_id == "local" {
        let command = match kind {
            "refind" => {
                if !crate::SOURCE_FILES
                    .iter()
                    .any(|(system, _, _)| *system == value)
                {
                    return Err(format!("未知的系统：{value}"));
                }
                crate::state::RemoteCommand::Switch(value.to_string())
            }
            "grub" => crate::state::RemoteCommand::SwitchGrub(value.to_string()),
            other => return Err(format!("未知的引导管理器：{other}")),
        };
        let (ok, message) = crate::remote_routing::execute_remote_command(
            app,
            crate::state::Channel::Http,
            "hub-电脑列表-本机行",
            &command,
        );
        return if ok { Ok(message) } else { Err(message) };
    }
    let path = match kind {
        // system 只有 linux/windows/macos 三个值，不需要转义
        "refind" => {
            if !crate::SOURCE_FILES
                .iter()
                .any(|(system, _, _)| *system == value)
            {
                return Err(format!("未知的系统：{value}"));
            }
            format!("/api/switch?system={value}")
        }
        // selector 里可能有空格、`>`、引号 —— 必须转义
        "grub" => format!("/api/grub-switch?entry={}", encode_query(value)),
        other => return Err(format!("未知的引导管理器：{other}")),
    };
    let server = load_store(app)?
        .servers
        .into_iter()
        .find(|candidate| candidate.id == server_id)
        .ok_or_else(|| format!("这台电脑不存在（可能已被删除）：{server_id}"))?;
    let reply = remote_http_json(app, &server, &path)?;
    command_reply_message(&reply, &format!("已提交切换：{value}"))
}

/// 用系统浏览器打开这台电脑的 AnyBoot 页面。
///
/// 为什么不让前端 `window.open`：hub 是 Tauri webview，`window.open` 只会开出
/// 一个新的 webview（还带着 tauri:// 的源），行为随平台不一致；电源页那种
/// 「用系统浏览器打开」的语义必须落到 `open_url_in_browser` 上。
/// 只放行 http/https —— 别让任意 scheme 穿过命令边界。
#[tauri::command]
pub fn remote_open_page(url: String) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("地址是空的".to_string());
    }
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!("只支持 http/https 地址：{url}"));
    }
    // 空白/控制字符会把 URL 拼坏（也会让 ShellExecuteW 收到半截命令）
    if url.chars().any(|item| item.is_whitespace() || item.is_control()) {
        return Err("地址里不能有空白或控制字符".to_string());
    }
    crate::open_url_in_browser(url)
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
            macs: Default::default(),
            power_routes: vec![],
            remote_addr: "  https://test-tunnel.trycloudflare.com/?ab_token= gw-tok  ".into(),
            remote_token: String::new(),
            created_at: 0,
        };
        let normalized = server.normalize();
        assert_eq!(normalized.group, "书房");
        assert_eq!(normalized.name, "台式机");
        assert_eq!(normalized.addresses, vec!["192.168.0.120:12300", "192.168.0.121:12300"]);
        // URL 里的 token 被挖出补进 token（原 token 为空时）
        assert_eq!(normalized.token, "abc");
        // 远程地址：剥路径/query 留 origin；URL 里的 ab_token 被挖出补进 remote_token
        assert_eq!(normalized.remote_addr, "https://test-tunnel.trycloudflare.com");
        assert_eq!(normalized.remote_token, "gw-tok");
        // 旧值 / 未知 probe_mode 一律归一为并发（首个）
        assert_eq!(normalized.probe_mode, "parallel_first");
        assert_eq!(normalized.jump_id, "j1");
    }

    #[test]
    fn split_remote_address_forms() {
        // 整条配对串：拆 tunnel + token
        assert_eq!(
            split_remote_address(
                "anyboot://pair?gateway=%E4%B9%A6%E6%88%BF&tunnel=https%3A%2F%2Fabc.trycloudflare.com&token=gw1"
            ),
            ("https://abc.trycloudflare.com".into(), Some("gw1".into()))
        );
        // 配对串 tunnel 未编码的普通形态也认
        assert_eq!(
            split_remote_address("anyboot://pair?tunnel=https://abc.trycloudflare.com&token=gw2"),
            ("https://abc.trycloudflare.com".into(), Some("gw2".into()))
        );
        // 整条 URL + ab_token
        assert_eq!(
            split_remote_address("https://abc-def.trycloudflare.com/?ab_token=t1"),
            ("https://abc-def.trycloudflare.com".into(), Some("t1".into()))
        );
        // 带 token= 旧式也认
        assert_eq!(
            split_remote_address("https://host.ts.net?token=t2"),
            ("https://host.ts.net".into(), Some("t2".into()))
        );
        // 裸 host 默认补 https
        assert_eq!(
            split_remote_address(" my-tunnel.example.com "),
            ("https://my-tunnel.example.com".into(), None)
        );
        // http 保留原 scheme
        assert_eq!(
            split_remote_address("http://1.2.3.4:8080/x"),
            ("http://1.2.3.4:8080".into(), None)
        );
        // 空 / 全是路径 → 空
        assert_eq!(split_remote_address("  "), (String::new(), None));
        assert_eq!(split_remote_address("https:///path"), (String::new(), None));
    }

    /// 三种探测模式的语义：并发首个 = 最快胜出；并发全部 = 逐行全报；
    /// 串行 = 按顺序第一个通的胜出。用一个真 /health 监听 + 一个死端口验证。
    #[test]
    fn probe_modes_pick_live_address_and_report_every_candidate() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("监听回环");
        let port = listener.local_addr().expect("本地地址").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buffer = [0u8; 512];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let body = "{\"ok\":true,\"machineId\":\"m1\",\"machineName\":\"测试机\"}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        let live = format!("127.0.0.1:{port}");
        let dead = "127.0.0.1:1".to_string(); // 端口 1 上不会有服务

        // 并发（全部）：逐行两条都在，赢家是活的那个
        let all = remote_test_blocking(vec![dead.clone(), live.clone()], "parallel_all".into(), None);
        assert!(all.ok, "并发全部应探到活的地址");
        assert_eq!(all.addr, live);
        assert_eq!(all.results.len(), 2);
        assert!(all.results.iter().any(|o| o.address == dead && !o.ok));
        assert!(all.results.iter().any(|o| o.address == live && o.ok));

        // 并发（首个）：赢家也是活的（死的那个直接失败出局）
        let first = remote_test_blocking(vec![dead.clone(), live.clone()], "parallel_first".into(), None);
        assert!(first.ok && first.addr == live);

        // 串行：先死后活，第一个成功的胜出
        let serial = remote_test_blocking(vec![dead, live.clone()], "serial".into(), None);
        assert!(serial.ok && serial.addr == live);
    }

    /// 远程兜底语义：局域网全挂 → 探远程（赢家带 remote 标记）；
    /// 局域网有赢家 → 远程不探（results 里补一行「未探测」）。
    #[test]
    fn remote_test_lan_first_remote_fallback() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("监听回环");
        let port = listener.local_addr().expect("本地地址").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buffer = [0u8; 512];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let body = "{\"ok\":true,\"machineId\":\"m1\",\"machineName\":\"测试机\"}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        // 本地 http 监听充当「远程」（ureq 对 http origin 照常工作；线上远程是 https）
        let origin = format!("http://127.0.0.1:{port}");
        let live = format!("127.0.0.1:{port}");

        // 局域网全挂 → 远程兜底，赢家 = 远程 origin 且 remote: true
        let fallback = remote_test_blocking(
            vec!["127.0.0.1:1".into()],
            "parallel_first".into(),
            Some(origin.clone()),
        );
        assert!(fallback.ok, "局域网全挂时应兜底远程");
        assert!(fallback.remote, "赢家应带远程标记");
        assert_eq!(fallback.addr, origin);
        assert_eq!(fallback.machine_id, "m1");
        assert!(fallback.results.iter().any(|o| o.remote && o.ok));

        // 局域网有赢家 → 远程不探，results 里带一行「未探测」
        let lan_up = remote_test_blocking(vec![live], "parallel_first".into(), Some(origin));
        assert!(lan_up.ok && !lan_up.remote, "局域网赢家优先");
        assert!(
            lan_up.results.iter().any(|o| o.remote && !o.ok && o.error.contains("未探测")),
            "远程未探测要有说明行"
        );
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
            macs: Default::default(),
            power_routes: vec![],
            remote_addr: String::new(),
            remote_token: String::new(),
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
                macs: Default::default(),
                power_routes: vec![],
                remote_addr: String::new(),
                remote_token: String::new(),
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
        let result = remote_test_blocking(vec![" ".into()], "parallel".into(), None);
        assert!(!result.ok);
        assert_eq!(result.error, "没有可探测的地址");
    }

    #[test]
    fn grub_class_prefers_windows_then_mac() {
        assert_eq!(grub_class("Windows Boot Manager", "windows", true), "win");
        // Ubuntu 里带 win 字样的自定义条目（"Windows 11 (on /dev/nvme0n1p1)"）
        assert_eq!(grub_class("Windows 11 (on /dev/nvme0n1p1)", "", false), "win");
        assert_eq!(grub_class("macOS Sequoia", "osx", true), "mac");
        assert_eq!(grub_class("Apple Recovery", "", false), "mac");
        assert_eq!(grub_class("Ubuntu", "ubuntu", false), "linux");
        assert_eq!(grub_class("UEFI Firmware Settings", "", true), "auto");
    }

    #[test]
    fn refind_from_status_picks_active_and_skips_missing() {
        let value = serde_json::json!({
            "ok": true,
            "variables": [
                {"system": "linux", "displayName": "Ubuntu 24.04 LTS", "exists": true, "active": false},
                {"system": "windows", "displayName": "", "exists": true, "active": true},
                // exists=false 的变量不该出现在下拉里（那台系统没装）
                {"system": "macos", "displayName": "macOS Sequoia", "exists": false, "active": false}
            ]
        });
        let group = refind_from_status(&value);
        assert!(group.available);
        assert_eq!(group.kind, "refind");
        assert_eq!(group.current, "windows");
        assert_eq!(group.options.len(), 2);
        // displayName 为空时退回 system，不能出空标签
        assert_eq!(group.options[1].label, "windows");
        assert_eq!(group.options[1].class, "win");
        assert!(group.note.is_empty());
    }

    #[test]
    fn refind_from_status_all_missing_is_unavailable() {
        let group = refind_from_status(&serde_json::json!({"variables": [
            {"system": "linux", "exists": false, "active": false}
        ]}));
        assert!(!group.available);
        assert!(group.options.is_empty());
        assert!(group.note.contains("没部署"));
    }

    #[test]
    fn grub_from_json_escapes_nothing_but_keeps_selector_verbatim() {
        let value = serde_json::json!({
            "installed": true,
            "savedEntry": "0>1",
            "nextEntry": "",
            "entries": [
                {"selector": "0", "name": "Ubuntu", "submenu": "", "class": "ubuntu", "chainloader": false},
                {"selector": "0>1", "name": "Advanced options for Ubuntu", "submenu": "", "class": "", "chainloader": false},
                {"selector": "2", "name": "Windows Boot Manager", "submenu": "", "class": "windows", "chainloader": true}
            ]
        });
        let group = grub_from_json(&value, "raw");
        assert!(group.available);
        // nextEntry 为空 → 用 savedEntry 当「下次启动」
        assert_eq!(group.current, "0>1");
        assert_eq!(group.next, "");
        assert_eq!(group.saved, "0>1");
        // 当前选中项置顶（第一条），原位照常保留 —— 与托盘同一行为
        assert_eq!(group.options[0].value, "0>1");
        assert_eq!(group.options[1].value, "0");
        assert_eq!(group.options[2].value, "0>1");
        assert_eq!(group.options[3].class, "win");
        // selector 原样保留（含 `>`，切换时要转义但不能被改写）
        assert_eq!(group.options[0].value, "0>1");
    }

    #[test]
    fn grub_from_json_submenu_prefixes_label_once() {
        let value = serde_json::json!({
            "installed": true,
            "entries": [
                {"selector": "1>0", "name": "Ubuntu, with Linux 6.8.0", "submenu": "Advanced options for Ubuntu", "class": "", "chainloader": false},
                {"selector": "1>1", "name": "Advanced options for Ubuntu (recovery mode)", "submenu": "Advanced options for Ubuntu", "class": "", "chainloader": false}
            ]
        });
        let group = grub_from_json(&value, "raw");
        // 与托盘同一结构：一行组头（计数）+ 子项 ↳ 缩进
        assert!(group.options[0].header);
        assert!(group.options[0].label.contains("（2 项）"), "{}", group.options[0].label);
        assert!(!group.options[1].header);
        assert_eq!(group.options[1].label, "↳ 🐧 Ubuntu, with Linux 6.8.0");
        assert_eq!(group.options[2].label, "↳ Advanced options for Ubuntu (recovery mode)");
    }

    /// 子菜单的 `submenu` 是内部 id（`gnulinux-advanced-<uuid>`），下拉里必须换成
    /// `groups` 里的组名 —— GRUB 列表页与托盘菜单都这么翻，电脑列表以前漏了这步。
    #[test]
    fn grub_from_json_maps_submenu_id_to_group_name() {
        let value = serde_json::json!({
            "installed": true,
            "groups": [
                {"id": "gnulinux-advanced-9f1bae08-f61b-4e7a-9277-8e00b3970c81",
                 "name": "Advanced options for CachyOS Linux"}
            ],
            "entries": [
                {"selector": "gnulinux-advanced-9f1bae08-f61b-4e7a-9277-8e00b3970c81>0",
                 "name": "CachyOS Linux, with Linux linux-cachyos",
                 "submenu": "gnulinux-advanced-9f1bae08-f61b-4e7a-9277-8e00b3970c81",
                 "class": "cachyos", "chainloader": false, "kernels": ["vmlinuz-linux-cachyos"]}
            ]
        });
        let group = grub_from_json(&value, "minimal");
        assert_eq!(
            group.options[0].label,
            "▸ 🐧 Advanced options for CachyOS Linux（1 项）"
        );
        assert_eq!(group.options[1].label, "↳ 🐧 cachyos");
        assert!(
            !group.options.iter().any(|option| option.label.contains("gnulinux-advanced")),
            "下拉文字里不该出现内部 id"
        );
    }

    /// 内核名是空串时不能出一行空白（要退回菜单标题）。
    #[test]
    fn grub_row_label_falls_back_to_name_when_kernel_is_blank() {
        let entry = crate::grub::GrubEntry {
            selector: "0".to_string(),
            id: "0".to_string(),
            name: "CachyOS Linux".to_string(),
            submenu: None,
            class: None,
            chainloader: false,
            kernels: vec!["   ".to_string()],
            variants: 1,
        };
        assert_eq!(crate::menu_model::grub_row_label("minimal", &entry), "CachyOS Linux");
        assert_eq!(crate::menu_model::grub_row_label("medium", &entry), "CachyOS Linux");
    }

    #[test]
    fn grub_labels_follow_the_configured_list_format() {
        let value = serde_json::json!({
            "installed": true,
            "entries": [
                {"selector": "0", "name": "CachyOS Linux", "submenu": "", "class": "cachyos",
                 "chainloader": false, "kernels": ["vmlinuz-linux-cachyos"]},
                {"selector": "1", "name": "Ubuntu", "submenu": "", "class": "ubuntu",
                 "chainloader": false, "kernels": ["vmlinuz-6.8.0-45-generic"]}
            ]
        });
        // 极简 / 简易：去掉 vmlinuz- 与 linux- 前缀（行首是系统图标，按标题判）
        assert_eq!(grub_from_json(&value, "minimal").options[0].label, "🐧 cachyos");
        assert_eq!(grub_from_json(&value, "simple").options[1].label, "6.8.0-45-generic");
        // 中等：完整内核名
        assert_eq!(
            grub_from_json(&value, "medium").options[1].label,
            "vmlinuz-6.8.0-45-generic"
        );
        // 原始 / 复杂：标题原文 + 行尾附内核名
        assert_eq!(
            grub_from_json(&value, "raw").options[0].label,
            "🐧 CachyOS Linux · vmlinuz-linux-cachyos"
        );
        assert_eq!(
            grub_from_json(&value, "detailed").options[0].label,
            "🐧 CachyOS Linux · vmlinuz-linux-cachyos"
        );
        // 没有 kernels 时退回标题（不猜）
        let plain = serde_json::json!({
            "installed": true,
            "entries": [
                {"selector": "0", "name": "Windows Boot Manager", "class": "windows", "chainloader": true}
            ]
        });
        assert_eq!(
            grub_from_json(&plain, "minimal").options[0].label,
            "🪟 Windows Boot Manager"
        );
    }

    /// 与托盘菜单同款的整体结构：置顶当前项（组名 › 前缀）+ 组头（计数与 ESP
    /// 位置）+ 子项缩进 + 同名折叠。
    #[test]
    fn grub_from_json_matches_tray_composition() {
        let value = serde_json::json!({
            "installed": true,
            "savedEntry": "g>1",
            "efiDir": "H:\\",
            "partuuid": "abcd-1234",
            "groups": [{"id": "g", "name": "Advanced options for CachyOS"}],
            "entries": [
                {"selector": "0", "name": "CachyOS Linux", "class": "cachyos",
                 "chainloader": false, "kernels": ["vmlinuz-linux-cachyos"]},
                {"selector": "g>0", "name": "CachyOS Linux, with Linux linux-cachyos",
                 "submenu": "g", "class": "cachyos", "chainloader": false,
                 "kernels": ["vmlinuz-linux-cachyos"]},
                {"selector": "g>1", "name": "CachyOS Linux, with Linux linux-cachyos (fallback)",
                 "submenu": "g", "class": "cachyos", "chainloader": false,
                 "kernels": ["vmlinuz-linux-cachyos-fallback"]}
            ]
        });
        let group = grub_from_json(&value, "simple");
        // 置顶行在最前，带「组名 › 」前缀
        assert_eq!(group.options[0].value, "g>1");
        assert!(
            group.options[0].label.contains("Advanced options for CachyOS › cachyos-fallback"),
            "{}",
            group.options[0].label
        );
        // 顶层条目照常在原位出现（置顶行之外它还在）
        assert_eq!(group.options[1].value, "0");
        // 组头带计数与 ESP 位置（simple 档带，极简/原始不带）
        assert!(group.options[2].header);
        assert!(group.options[2].label.contains("（2 项"), "{}", group.options[2].label);
        assert!(group.options[2].label.contains("位于H:\\ UUID=abcd-1234"));
        // 子项缩进
        assert!(group.options[3].label.starts_with("↳ "));
        // 极简档组头不带位置
        let minimal = grub_from_json(&value, "minimal");
        assert!(minimal.options[2].label.contains("（2 项）"));
        assert!(!minimal.options[2].label.contains("位于"));

        // 同名条目折叠成一条并标变体数（非原始格式；原始格式不折叠）
        let dup = serde_json::json!({
            "installed": true,
            "entries": [
                {"selector": "m1", "name": "Memory test", "class": "memtest", "chainloader": false},
                {"selector": "m2", "name": "Memory test", "class": "memtest", "chainloader": false}
            ]
        });
        let folded = grub_from_json(&dup, "minimal");
        assert_eq!(folded.options.len(), 1);
        assert!(folded.options[0].label.contains("2 个变体"), "{}", folded.options[0].label);
        let unfolded = grub_from_json(&dup, "raw");
        assert_eq!(unfolded.options.len(), 2);
    }

    #[test]
    fn grub_from_json_without_entries_reports_reason() {
        let group = grub_from_json(
            &serde_json::json!({"installed": false, "error": "没找到 grub.cfg"}),
            "raw",
        );
        assert!(!group.available);
        assert_eq!(group.note, "没找到 grub.cfg");
    }

    #[test]
    fn boot_option_value_is_url_escaped_on_switch() {        // GRUB selector 里的 `>` 与空格必须转义，否则服务端 query 解析会截断
        assert_eq!(encode_query("0>1"), "0%3E1");
        assert_eq!(encode_query("Advanced options"), "Advanced%20options");
    }
}
