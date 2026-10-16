//! server-hub（远程管理独立窗口）的后端：服务器档案存储 + CRUD + 测试连接。
//!
//! 刻意与主窗口的任何状态解耦（主窗口零改动原则）：
//! - 存储是配置目录下自己的一个 JSON 文件（`hub-servers.json`）；
//! - 命令只服务 hub 窗口，语义自足，不碰 UiState。
//!
//! 密码刻意不落盘：PC 端走系统 `ssh`（M3 起，PTY 内交互认证），
//! 认证细节交给 ssh 本尊，hub 只管「连到哪」。

use serde::{Deserialize, Serialize};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::AppHandle;

/// 服务器档案文件名（放配置目录，随 `ANYBOOT_CONFIG_DIR` 走）
pub(crate) const HUB_SERVERS_FILE: &str = "hub-servers.json";

/// 测试连接的 TCP 超时
const TEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HubServer {
    pub id: String,
    pub name: String,
    /// "ssh" | "telnet"（M3 终端先通 SSH；telnet 字段先留着）
    pub protocol: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub notes: String,
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
        self
    }
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
    if server.name.is_empty() {
        return Err("名称不能为空".to_string());
    }
    if server.host.is_empty() {
        return Err("主机地址不能为空".to_string());
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
#[tauri::command]
pub fn hub_test_server(server: HubServer) -> Result<String, String> {
    let server = server.normalize();
    if server.host.is_empty() {
        return Err("主机地址不能为空".to_string());
    }
    let port = server.effective_port();
    let target = format!("{}:{}", server.host, port);
    let addresses: std::net::SocketAddr = target
        .to_socket_addrs()
        .map_err(|error| format!("无法解析 {target}: {error}"))?
        .next()
        .ok_or_else(|| format!("无法解析 {target}"))?;
    match TcpStream::connect_timeout(&addresses, TEST_TIMEOUT) {
        Ok(_) => Ok(format!(
            "已连通 {}:{}（{}）",
            server.host,
            port,
            addresses.ip()
        )),
        Err(error) => Err(format!("连不上 {target}: {error}")),
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
            username: "root".to_string(),
            notes: String::new(),
            created_at: 0,
        }
    }

    #[test]
    fn effective_port_defaults_by_protocol() {
        assert_eq!(sample("a").effective_port(), 22);
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
        };
        assert!(hub_test_server(server).is_err());
    }
}
