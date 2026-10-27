//! 备份与恢复 + WebDAV 多端同步（docs/backup-design.md v4 / backup-ui-design.md 桌面 D1-D7）。
//!
//! 信封契约与 iOS / Android 逐字节对齐：
//! - 顶层 `format="anyboot-backup"` / `version=1`；加密时 `data` 置空、`payload` 装
//!   base64(AES-256-GCM 密文+tag)，`encryption` 带 PBKDF2 参数；
//! - AAD = `anyboot-backup|v1|<scheme>|<salt>|<iv>|<rounds>`（防篡改 version/rounds/salt/iv）；
//! - canonical 键 camelCase；secrets 明文表 `key:<id>:private` / `jump:<id>:password`；
//! - 桌面端 vault 密钥是本地文件（hub-vault.key 可拷贝），所以导出侧能把密码解成明文；
//!   反向导入用 hub_vault::encrypt 重新落密文。
//!
//! 桌面特有口径（§2.1 审计）：
//! - 电脑档案 = remote-servers.json；SSH 服务器 = hub-servers.json；密钥 = keys/ 目录文件；
//! - settings.json **主体不迁移**（machine_id 不可迁移）、hub-tunnels（local_port）、
//!   transfer-queue、tsnet state 一律排除；
//! - 私钥口令「只出不进」：带口令的钥匙解不出 PEM（ssh-keygen -y 需要口令），导出跳过并在
//!   报告里记失败原因；导入方向一律写成无口令私钥文件。
//!
//! 同步（§9）：整文件拉取 + 按条目 LWW（updatedAt ISO 串直接比）→ 推回；
//! rev + If-Match 乐观锁；桌面档案不落 updatedAt（struct 无该字段），时间戳存
//! sidecar `backup-item-times.json`，collect/merge 时挂回 canonical 条目。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::{json, Value};
use std::io::Read as _;
use tauri_plugin_dialog::DialogExt;

pub(crate) const FORMAT: &str = "anyboot-backup";
pub(crate) const VERSION: u32 = 1;
pub(crate) const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const PBKDF2_ROUNDS: u32 = 310_000;
pub(crate) const SCHEME: &str = "PBKDF2-SHA256-310000 + AES-256-GCM";

pub(crate) const CATEGORIES: [&str; 5] = ["servers", "jumps", "keys", "terminal", "history"];

// ==================== 加密（与移动端 BackupCrypto 同参） ====================

fn aad_seed(salt_b64: &str, iv_b64: &str, rounds: u32) -> Vec<u8> {
    format!("{FORMAT}|v1|{SCHEME}|{salt_b64}|{iv_b64}|{rounds}").into_bytes()
}

/// PBKDF2-HMAC-SHA256（手写：hmac 0.12 + sha2，不引入 pbkdf2 crate）
fn pbkdf2_key(password: &str, salt: &[u8], rounds: u32) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(password.as_bytes())
        .expect("HMAC 接受任意长度密钥");
    let block = [1u8, 0, 0, 0];
    let mut salt_block = salt.to_vec();
    salt_block.extend_from_slice(&block);
    mac.update(&salt_block);
    let mut u = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&u);
    for _ in 1..rounds {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(password.as_bytes())
            .expect("HMAC 接受任意长度密钥");
        mac.update(&u);
        u = mac.finalize().into_bytes();
        for (o, b) in out.iter_mut().zip(u.iter()) {
            *o ^= b;
        }
    }
    out
}

fn aes_gcm_seal(plain: &[u8], key: &[u8; 32], nonce: &[u8; 12], aad: &[u8]) -> Result<Vec<u8>, String> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(key)
        .map_err(|error| format!("备份密钥长度异常: {error}"))?;
    cipher
        .encrypt(
            aes_gcm::Nonce::from_slice(nonce),
            Payload { msg: plain, aad },
        )
        .map_err(|error| format!("备份加密失败: {error}"))
}

fn aes_gcm_open(sealed: &[u8], key: &[u8; 32], nonce: &[u8; 12], aad: &[u8]) -> Result<Vec<u8>, String> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(key)
        .map_err(|error| format!("备份密钥长度异常: {error}"))?;
    cipher
        .decrypt(aes_gcm::Nonce::from_slice(nonce), Payload { msg: sealed, aad })
        .map_err(|_| "口令错误或文件已损坏".to_string())
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).expect("系统熵源不可用");
    bytes
}

/// 明文信封 + 口令 → 加密信封字节
pub(crate) fn serialize(envelope: &Value, password: Option<&str>) -> Result<Vec<u8>, String> {
    let plain = serde_json::to_vec(envelope).map_err(|error| format!("序列化失败: {error}"))?;
    let password = match password {
        Some(p) if !p.is_empty() => p,
        _ => return Ok(plain),
    };
    let salt = random_bytes(16);
    let nonce = random_bytes(12);
    let salt_b64 = B64.encode(&salt);
    let iv_b64 = B64.encode(&nonce);
    let key = pbkdf2_key(password, &salt, PBKDF2_ROUNDS);
    let ciphertext = aes_gcm_seal(&plain, &key, nonce.as_slice().try_into().unwrap(),
        &aad_seed(&salt_b64, &iv_b64, PBKDF2_ROUNDS))?;

    let mut boxed = envelope.clone();
    boxed.as_object_mut().map(|o| o.remove("data"));
    boxed["encryption"] = json!({
        "scheme": SCHEME,
        "salt": salt_b64,
        "iv": iv_b64,
        "rounds": PBKDF2_ROUNDS,
    });
    boxed["payload"] = Value::String(B64.encode(&ciphertext));
    Ok(serde_json::to_vec(&boxed).map_err(|error| format!("序列化失败: {error}"))?)
}

fn require_format(envelope: &Value) -> Result<(), String> {
    if envelope["format"].as_str() != Some(FORMAT) {
        return Err("不是有效的 AnyBoot 备份文件".to_string());
    }
    Ok(())
}

fn check_version(envelope: &Value) -> Result<(), String> {
    if envelope["version"].as_u64().unwrap_or(1) > VERSION as u64 {
        return Err("备份来自更新版本，请先升级 App".to_string());
    }
    Ok(())
}

/// 原始字节 → 明文信封。加密文件须带口令。
pub(crate) fn open_envelope(raw: &[u8], password: Option<&str>) -> Result<Value, String> {
    if raw.len() > MAX_FILE_BYTES {
        return Err("文件超过 10MB，已拒绝".to_string());
    }
    let envelope: Value = serde_json::from_slice(raw)
        .map_err(|_| "不是有效的 AnyBoot 备份文件".to_string())?;
    require_format(&envelope)?;
    if envelope["payload"].is_null() || envelope["encryption"].is_null() {
        check_version(&envelope)?;
        return Ok(envelope);
    }
    let password = password.filter(|p| !p.is_empty()).ok_or("此备份已加密，请输入口令")?;
    let encryption = &envelope["encryption"];
    let salt = B64.decode(encryption["salt"].as_str().unwrap_or(""))
        .map_err(|_| "备份文件已损坏".to_string())?;
    let nonce = B64.decode(encryption["iv"].as_str().unwrap_or(""))
        .map_err(|_| "备份文件已损坏".to_string())?;
    if nonce.len() != 12 {
        return Err("备份文件已损坏".to_string());
    }
    let rounds = encryption["rounds"].as_u64().unwrap_or(PBKDF2_ROUNDS as u64) as u32;
    let sealed = B64.decode(envelope["payload"].as_str().unwrap_or(""))
        .map_err(|_| "备份文件已损坏".to_string())?;
    let key = pbkdf2_key(password, &salt, rounds);
    let plain = aes_gcm_open(&sealed, &key, nonce.as_slice().try_into().unwrap(),
        &aad_seed(encryption["salt"].as_str().unwrap_or(""),
            encryption["iv"].as_str().unwrap_or(""), rounds))?;
    let inner: Value = serde_json::from_slice(&plain)
        .map_err(|_| "备份文件已损坏".to_string())?;
    require_format(&inner)?;
    check_version(&inner)?;
    let mut inner = inner;
    if let Some(sync) = envelope.get("sync") {
        inner["sync"] = sync.clone();
    }
    Ok(inner)
}

fn is_encrypted_envelope(raw: &[u8]) -> Result<bool, String> {
    if raw.len() > MAX_FILE_BYTES {
        return Err("文件超过 10MB，已拒绝".to_string());
    }
    let envelope: Value = serde_json::from_slice(raw)
        .map_err(|_| "不是有效的 AnyBoot 备份文件".to_string())?;
    require_format(&envelope)?;
    Ok(!envelope["payload"].is_null() && !envelope["encryption"].is_null())
}

// ==================== 时间工具 ====================

fn iso_now() -> String {
    iso_ms(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0))
}

fn iso_ms(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    // 简易 UTC 格式化（无 chrono 依赖）： civil-from-days 算法
    let days = (ms / 86_400_000) as i64;
    let rem = ms % 86_400_000;
    let (h, m, s) = (rem / 3_600_000, (rem / 60_000) % 60, (rem / 1000) % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn ms_iso(value: &str) -> u64 {
    let value = value.trim();
    if value.is_empty() {
        return 0;
    }
    if value.chars().all(|c| c.is_ascii_digit()) {
        return value.parse().unwrap_or(0);
    }
    // 只需要「能比较大小」的精度：解析 yyyy-MM-ddTHH:mm:ss
    let bytes = value.as_bytes();
    if bytes.len() < 19 {
        return 0;
    }
    let digits: Vec<u32> = bytes[..19]
        .iter()
        .filter(|b| b.is_ascii_digit())
        .map(|b| (b - b'0') as u32)
        .collect();
    // yyyy-MM-ddTHH:mm:ss 里数字共 14 位（4+2+2+2+2+2），其余是分隔符
    if digits.len() < 14 {
        return 0;
    }
    // days_from_civil
    let (y, mo, d) = (digits[0] * 1000 + digits[1] * 100 + digits[2] * 10 + digits[3],
        digits[4] * 10 + digits[5], digits[6] * 10 + digits[7]);
    let (hh, mm, ss) = (digits[8] * 10 + digits[9], digits[10] * 10 + digits[11],
        digits[12] * 10 + digits[13]);
    let y_adj = if mo <= 2 { y - 1 } else { y } as i64;
    let era = y_adj.div_euclid(400);
    let yoe = y_adj - era * 400;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days as u64) * 86_400_000 + hh as u64 * 3_600_000 + mm as u64 * 60_000 + ss as u64 * 1000
}

// ==================== 键名转换（桌面 snake ⇄ canonical camel） ====================

fn rename(object: &mut Value, from: &str, to: &str) {
    if let Some(map) = object.as_object_mut() {
        if let Some(value) = map.remove(from) {
            map.insert(to.to_string(), value);
        }
    }
}

/// remote-servers.json 原生条目 → canonical（snake → camel；配对键一起改）
fn canonicalize_server(item: &Value) -> Value {
    let mut v = item.clone();
    rename(&mut v, "machine_id", "machineId");
    rename(&mut v, "machine_name", "machineName");
    rename(&mut v, "machine_os", "machineOs");
    rename(&mut v, "probe_mode", "probeMode");
    rename(&mut v, "route_probe_mode", "routeProbeMode");
    rename(&mut v, "probe_timeout_ms", "probeTimeoutMs");
    rename(&mut v, "route_timeouts_ms", "routeTimeoutsMs");
    rename(&mut v, "block_probe_mode", "blockProbeMode");
    rename(&mut v, "block_timeouts_ms", "blockTimeoutsMs");
    rename(&mut v, "jump_id", "jumpServerId");
    rename(&mut v, "power_routes", "powerRoutes");
    rename(&mut v, "route_order", "routeOrder");
    rename(&mut v, "route_disabled", "routeDisabled");
    rename(&mut v, "conn_mode", "connMode");
    if let Some(pairs) = v["pairs"].as_array_mut() {
        for pair in pairs {
            rename(pair, "probe_timeout_ms", "probeTimeoutMs");
            rename(pair, "tunnel_id", "tunnelId");
            rename(pair, "created_at", "createdAt");
            if pair["createdAt"].is_string() {
                let ms = ms_iso(pair["createdAt"].as_str().unwrap_or(""));
                if ms > 0 {
                    pair["createdAt"] = json!(ms);
                }
            }
        }
    }
    if let Some(routes) = v["powerRoutes"].as_array_mut() {
        for route in routes {
            rename(route, "http_fallback", "httpFallback");
        }
    }
    v
}

/// canonical → remote-servers.json 原生（camel → snake）
fn native_server(item: &Value) -> Value {
    let mut v = item.clone();
    rename(&mut v, "machineId", "machine_id");
    rename(&mut v, "machineName", "machine_name");
    rename(&mut v, "machineOs", "machine_os");
    rename(&mut v, "probeMode", "probe_mode");
    rename(&mut v, "routeProbeMode", "route_probe_mode");
    rename(&mut v, "probeTimeoutMs", "probe_timeout_ms");
    rename(&mut v, "routeTimeoutsMs", "route_timeouts_ms");
    rename(&mut v, "blockProbeMode", "block_probe_mode");
    rename(&mut v, "blockTimeoutsMs", "block_timeouts_ms");
    rename(&mut v, "jumpServerId", "jump_id");
    rename(&mut v, "powerRoutes", "power_routes");
    rename(&mut v, "routeOrder", "route_order");
    rename(&mut v, "routeDisabled", "route_disabled");
    rename(&mut v, "connMode", "conn_mode");
    if let Some(pairs) = v["pairs"].as_array_mut() {
        for pair in pairs {
            rename(pair, "probeTimeoutMs", "probe_timeout_ms");
            rename(pair, "tunnelId", "tunnel_id");
            rename(pair, "createdAt", "created_at");
            if pair["created_at"].is_string() {
                let ms = ms_iso(pair["created_at"].as_str().unwrap_or(""));
                if ms > 0 {
                    pair["created_at"] = json!(ms);
                }
            }
        }
    }
    if let Some(routes) = v["power_routes"].as_array_mut() {
        for route in routes {
            rename(route, "httpFallback", "http_fallback");
        }
    }
    v
}

/// canonical jump → hub-servers.json 原生（HubServer snake 字段）
fn native_jump(item: &Value, secrets: &Value) -> Result<Value, String> {
    let id = item["id"].as_str().unwrap_or("").trim().to_string();
    if id.is_empty() {
        return Err("档案字段不合法".to_string());
    }
    let mut host = item["host"].as_str().unwrap_or("").trim().to_string();
    if host.is_empty() {
        if let Some(first) = item["addresses"].as_array().and_then(|a| a.first()) {
            host = first.as_str().unwrap_or("").trim().to_string();
        }
    }
    if host.is_empty() {
        return Err("档案字段不合法".to_string());
    }
    // host:port → host + port
    let (hostname, port) = match host.rfind(':') {
        Some(index) if index > 0 && index < host.len() - 1 => (
            host[..index].to_string(),
            host[index + 1..].parse::<u16>().unwrap_or(22),
        ),
        _ => (host.clone(), 22),
    };
    let password_plain = secrets["jump:".to_string() + &id + ":password"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok(json!({
        "id": id,
        "name": item["name"].as_str().unwrap_or(""),
        "protocol": if item["protocol"].as_str() == Some("telnet") { "telnet" } else { "ssh" },
        "host": hostname,
        "port": port,
        "addresses": [host],
        "username": item["username"].as_str().unwrap_or(""),
        "notes": item["note"].as_str().unwrap_or(""),
        "key_id": item["keyId"].as_str().unwrap_or(""),
        "password_enc": "",   // 由调用方经 vault 加密后回填
        "jump_id": item["jumpServerId"].as_str().unwrap_or(""),
        "term_path": item["termPath"].as_str().unwrap_or(""),
        "term_shell": item["termShell"].as_str().unwrap_or(""),
        "created_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "_password_plain": password_plain,
    }))
}

// ==================== 收集（导出组包） ====================

fn times_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(crate::config_dir(app)?.join("backup-item-times.json"))
}

/// sidecar 时间戳表 {"servers|<id>": iso, ...}
fn load_item_times(app: &tauri::AppHandle) -> Value {
    std::fs::read_to_string(times_path(app).unwrap_or_default())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_item_times(app: &tauri::AppHandle, times: &Value) -> Result<(), String> {
    let path = times_path(app)?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = serde_json::to_string_pretty(times).map_err(|error| format!("序列化失败: {error}"))?;
    std::fs::write(&path, content.as_bytes()).map_err(|error| format!("无法写时间戳表: {error}"))
}

fn collect_servers(app: &tauri::AppHandle, times: &Value) -> Result<(Value, bool), String> {
    let path = crate::config_dir(app)?.join(crate::remote_servers::REMOTE_SERVERS_FILE);
    let store: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_else(|| json!({"servers": []}));
    let list = store["servers"].as_array().cloned().unwrap_or_default();
    let mut items = Vec::new();
    let mut touched = false;
    let now = iso_now();
    for item in &list {
        let mut canonical = canonicalize_server(item);
        let id = canonical["id"].as_str().unwrap_or("").to_string();
        let key = format!("servers|{id}");
        if canonical["updatedAt"].is_null() {
            let stamp = times[&key].as_str().unwrap_or(&now).to_string();
            canonical["updatedAt"] = json!(stamp);
            touched = true;
        }
        items.push(canonical);
    }
    let active = store["active_id"].as_str()
        .or(store["activeId"].as_str())
        .unwrap_or("");
    Ok((json!({"items": items, "activeId": active}), touched))
}

fn collect_jumps(app: &tauri::AppHandle, times: &Value)
                 -> Result<(Value, Value, Vec<(String, String)>), String> {
    let path = crate::config_dir(app)?.join(crate::hub::HUB_SERVERS_FILE);
    let list: Vec<Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default();
    let vault = crate::hub_vault::load_or_create_key(&crate::config_dir(app)?)?;
    let now = iso_now();
    let mut items = Vec::new();
    let mut secrets = json!({});
    let mut times_updates: Vec<(String, String)> = Vec::new();
    for server in &list {
        let id = server["id"].as_str().unwrap_or("").to_string();
        let mut canonical = json!({
            "id": id,
            "name": server["name"].as_str().unwrap_or(""),
            "protocol": server["protocol"].as_str().unwrap_or("ssh"),
            "host": format!("{}:{}",
                server["host"].as_str().unwrap_or(""),
                server["port"].as_u64().unwrap_or(22)),
            "addresses": server["addresses"].as_array().cloned().unwrap_or_default(),
            "username": server["username"].as_str().unwrap_or(""),
            "authType": if !server["key_id"].as_str().unwrap_or("").is_empty() {
                "key"
            } else {
                "password"
            },
            "keyId": server["key_id"].as_str().unwrap_or(""),
            "jumpServerId": server["jump_id"].as_str().unwrap_or(""),
            "note": server["notes"].as_str().unwrap_or(""),
            "termPath": server["term_path"].as_str().unwrap_or(""),
            "termShell": server["term_shell"].as_str().unwrap_or(""),
        });
        // 密码：vault 密文本机可解 → 明文进 secrets
        let password_enc = server["password_enc"].as_str().unwrap_or("");
        if !password_enc.is_empty() {
            if let Ok(plain) = crate::hub_vault::decrypt_with(&vault, password_enc) {
                if !plain.is_empty() {
                    secrets["jump:".to_string() + &id + ":password"] = json!(plain);
                }
            }
        }
        let key = format!("jumps|{id}");
        if let Some(stamp) = times[&key].as_str() {
            canonical["updatedAt"] = json!(stamp);
        } else {
            canonical["updatedAt"] = json!(now.clone());
            times_updates.push((key, now.clone()));
        }
        items.push(canonical);
    }
    Ok((json!({"items": items}), secrets, times_updates))
}

fn collect_keys(app: &tauri::AppHandle, times: &Value) -> Result<(Value, Vec<String>), String> {
    let dir = crate::hub_keys::keys_dir(app)?;
    let mut items = Vec::new();
    let mut secrets = json!({});
    let mut skipped = Vec::new();
    let now = iso_now();
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|read| {
            read.filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_file()
                    && path.extension().map(|ext| ext != "pub").unwrap_or(true))
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    for private in entries {
        let Some(id) = private.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let public = private.with_extension("pub");
        let (algorithm, fingerprint, public_key) = crate::hub_keys::describe_key(&private);
        let pem = std::fs::read_to_string(&private).unwrap_or_default();
        if pem.starts_with("-----BEGIN ENCRYPTED")
            || crate::hub_keys::key_has_passphrase(&private) {
            skipped.push(format!("密钥「{id}」带口令，桌面端只出不进：已跳过私钥材料"));
            continue;
        }
        let mut canonical = json!({
            "id": id,
            "name": id,
            "algorithm": algorithm,
            "fingerprint": fingerprint,
            "publicKey": public_key,
            "createdAt": now,
        });
        if !pem.trim().is_empty() {
            secrets["key:".to_string() + &id + ":private"] = json!(pem);
        }
        let key = format!("keys|{id}");
        if let Some(stamp) = times[&key].as_str() {
            canonical["updatedAt"] = json!(stamp);
        } else {
            canonical["updatedAt"] = json!(now.clone());
        }
        let _ = public; // publicKey 已由 describe_key 读出
        items.push(canonical);
    }
    Ok((json!({"items": items, "secrets": secrets}), skipped))
}

/// 组明文信封；sync 为 None = 手工导出
pub(crate) fn collect(app: &tauri::AppHandle, categories: &[String],
                      sync: Option<Value>) -> Result<Value, String> {
    let times = load_item_times(app);
    let mut times_dirty = false;
    let mut data = json!({});
    let mut key_skip_notes: Vec<String> = Vec::new();

    for cat in CATEGORIES {
        if !categories.iter().any(|c| c == cat) {
            continue;
        }
        match cat {
            "servers" => {
                let (payload, touched) = collect_servers(app, &times)?;
                times_dirty |= touched;
                data["servers"] = payload;
            }
            "jumps" => {
                let (payload, secrets, updates) = collect_jumps(app, &times)?;
                if !updates.is_empty() {
                    times_dirty = true;
                    let mut times_mut = times.clone();
                    if let Some(map) = times_mut.as_object_mut() {
                        for (key, stamp) in updates {
                            map.insert(key, Value::String(stamp));
                        }
                    }
                    data["jumps"] = payload;
                    data["_secrets"] = secrets;
                    continue;
                }
                data["jumps"] = payload;
                data["_secrets"] = secrets;
            }
            "keys" => {
                let (payload, skipped) = collect_keys(app, &times)?;
                key_skip_notes = skipped;
                // keys 的 secrets 并进 _secrets
                if let Some(secrets) = payload.get("secrets").cloned() {
                    if let Some(existing) = data.get_mut("_secrets") {
                        if let (Some(map), Some(add)) =
                            (existing.as_object_mut(), secrets.as_object()) {
                            for (k, v) in add {
                                map.insert(k.clone(), v.clone());
                            }
                        }
                    } else {
                        data["_secrets"] = secrets;
                    }
                }
                data["keys"] = json!({
                    "items": payload["items"].clone(),
                    "secrets": payload["secrets"].clone(),
                });
            }
            "terminal" => {
                // 桌面 settings.json 主体不迁移（§2.1）：只导空壳占位，导入端按共同子集消费
                data["terminal"] = json!({});
            }
            "history" => {
                // TOFU 指纹由系统 ssh known_hosts 管：桌面导空，导入方向忽略
                data["history"] = json!({"sessions": [], "trustedHosts": []});
            }
            _ => {}
        }
    }

    // secrets 收进 data.keys.secrets（信封契约位）；没有 keys 分类时挂临时字段再摘掉
    let secrets = data.get("_secrets").cloned().unwrap_or(json!({}));
    data.as_object_mut().map(|o| o.remove("_secrets"));
    if !secrets.as_object().map(|m| m.is_empty()).unwrap_or(true) {
        if data["keys"].is_null() {
            data["keys"] = json!({"items": [], "secrets": secrets});
        } else {
            data["keys"]["secrets"] = secrets;
        }
    }

    if times_dirty {
        save_item_times(app, &times)?;
    }
    let _ = key_skip_notes; // 跳过说明由 summarize/导出 toast 拼接（见 backup_export）

    let includes: Vec<&str> = CATEGORIES
        .iter()
        .filter(|c| categories.iter().any(|x| x == **c))
        .copied()
        .collect();
    let mut envelope = json!({
        "format": FORMAT,
        "version": VERSION,
        "app": env!("CARGO_PKG_VERSION"),
        "platform": "desktop",
        "exportedAt": iso_now(),
        "includes": includes,
        "data": data,
    });
    if let Some(sync) = sync {
        envelope["sync"] = sync;
    }
    Ok(envelope)
}

// ==================== 摘要（D5 预览） ====================

pub(crate) fn summarize(app: &tauri::AppHandle, envelope: &Value) -> Result<Value, String> {
    let data = envelope["data"].as_object().ok_or("备份没有数据")?;
    let mut rows = json!({});

    fn row_of(payload: Option<&Value>, local_ids: Vec<String>, kind: &str) -> Value {
        let items = payload.map(|p| p["items"].clone()).unwrap_or(json!([]));
        let mut conflicts = Vec::new();
        if let Some(list) = items.as_array() {
            for (index, item) in list.iter().enumerate() {
                let id = item["id"].as_str().unwrap_or("");
                if local_ids.iter().any(|x| x == id) {
                    conflicts.push(json!(format!("{kind} {}", index + 1)));
                }
            }
        }
        json!({
            "backup": items.as_array().map(|a| a.len()).unwrap_or(0),
            "local": local_ids.len(),
            "conflicts": conflicts,
        })
    }

    fn server_ids(store: &Value) -> Vec<String> {
        store["servers"].as_array().map(|a| {
            a.iter().filter_map(|s| s["id"].as_str().map(String::from)).collect()
        }).unwrap_or_default()
    }
    fn jump_ids(list: &[Value]) -> Vec<String> {
        list.iter().filter_map(|s| s["id"].as_str().map(String::from)).collect()
    }
    fn key_ids(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir).map(|read| {
            read.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| !n.ends_with(".pub")).collect()
        }).unwrap_or_default()
    }

    if let Some(payload) = data.get("servers") {
        let path = crate::config_dir(app)?.join(crate::remote_servers::REMOTE_SERVERS_FILE);
        let store: Value = std::fs::read_to_string(&path).ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_else(|| json!({}));
        rows["servers"] = row_of(Some(payload), server_ids(&store), "电脑");
    }
    if let Some(payload) = data.get("jumps") {
        let path = crate::config_dir(app)?.join(crate::hub::HUB_SERVERS_FILE);
        let list: Vec<Value> = std::fs::read_to_string(&path).ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default();
        rows["jumps"] = row_of(Some(payload), jump_ids(&list), "服务器");
    }
    if let Some(payload) = data.get("keys") {
        let dir = crate::config_dir(app)?.join(crate::hub_keys::HUB_KEYS_DIR);
        rows["keys"] = row_of(Some(payload), key_ids(&dir), "密钥");
    }
    if let Some(_prefs) = data.get("terminal") {
        rows["terminal"] = json!({"backup": 1, "local": 1, "conflicts": []});
    }
    if let Some(history) = data.get("history") {
        rows["history"] = json!({
            "backup": history["sessions"].as_array().map(|a| a.len()).unwrap_or(0),
            "local": 0,
            "conflicts": [],
        });
    }
    // 引用完整性
    let jump_ids_in_backup: Vec<String> = data["jumps"]["items"].as_array()
        .map(|a| a.iter().filter_map(|j| j["id"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    let mut missing = 0;
    for server in data["servers"]["items"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let reference = server["jumpServerId"].as_str().unwrap_or("");
        if !reference.is_empty() && !jump_ids_in_backup.iter().any(|x| x == reference) {
            missing += 1;
        }
    }
    Ok(json!({
        "rows": rows,
        "missingJumpRefs": missing,
        "platform": envelope["platform"].as_str().unwrap_or(""),
        "exportedAt": envelope["exportedAt"].as_str().unwrap_or(""),
        "app": envelope["app"].as_str().unwrap_or(""),
    }))
}

// ==================== 落地（导入 apply / 同步合并共用） ====================

pub(crate) fn apply(app: &tauri::AppHandle, envelope: &Value, categories: &[String],
                   strategy: &str, tombstones: &Vec<String>) -> Result<Value, String> {
    let data = envelope["data"].as_object().ok_or("备份没有数据")?;
    let secrets = data.get("keys").and_then(|k| k.get("secrets")).cloned().unwrap_or(json!({}));
    let mut report = json!({});

    // ---- 电脑列表 ----
    if categories.iter().any(|c| c == "servers") {
        if let Some(payload) = data.get("servers") {
            report["servers"] = apply_servers(app, payload, strategy, tombstones)?;
        }
    }
    // ---- SSH 服务器 ----
    if categories.iter().any(|c| c == "jumps") {
        if let Some(payload) = data.get("jumps") {
            report["jumps"] = apply_jumps(app, payload, strategy, tombstones, &secrets)?;
        }
    }
    // ---- SSH 密钥 ----
    if categories.iter().any(|c| c == "keys") {
        if let Some(payload) = data.get("keys") {
            report["keys"] = apply_keys(app, payload, strategy, tombstones, &secrets)?;
        }
    }
    // ---- 偏好 / 历史：桌面按共同子集消费（当前无共同键，记 skipped） ----
    if categories.iter().any(|c| c == "terminal") && data.contains_key("terminal") {
        report["terminal"] = json!({"category": "terminal", "added": 0, "updated": 0,
            "skipped": 1, "failed": []});
    }
    if categories.iter().any(|c| c == "history") {
        if let Some(history) = data.get("history") {
            // TOFU 指纹桌面由系统 ssh 管：只记跳过
            let count = history["trustedHosts"].as_array().map(|a| a.len()).unwrap_or(0);
            report["history"] = json!({"category": "history", "added": 0, "updated": 0,
                "skipped": count, "failed": []});
        }
    }
    Ok(report)
}

fn row_shell(cat: &str) -> Value {
    json!({"category": cat, "added": 0, "updated": 0, "skipped": 0, "failed": []})
}

fn push_failed(row: &mut Value, message: &str) {
    if let Some(list) = row["failed"].as_array_mut() {
        list.push(json!(message));
    }
}

fn apply_servers(app: &tauri::AppHandle, payload: &Value, strategy: &str,
                 tombstones: &Vec<String>) -> Result<Value, String> {
    let mut row = row_shell("servers");
    let path = crate::config_dir(app)?.join(crate::remote_servers::REMOTE_SERVERS_FILE);
    let mut store: Value = std::fs::read_to_string(&path).ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_else(|| json!({"servers": []}));
    let mut list = store["servers"].as_array().cloned().unwrap_or_default();
    let items = payload["items"].as_array().cloned().unwrap_or_default();

    let overwrite = strategy == "overwrite";
    if overwrite {
        list.clear();
    }
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut skipped = 0usize;
    for (index, item) in items.iter().enumerate() {
        let id = item["id"].as_str().unwrap_or("").trim().to_string();
        let has_address = item["addresses"].as_array()
            .map(|a| a.iter().any(|x| !x.as_str().unwrap_or("").trim().is_empty()))
            .unwrap_or(false);
        if id.is_empty() || !has_address {
            push_failed(&mut row, &format!("第 {} 台电脑：档案字段不合法", index + 1));
            continue;
        }
        if tombstones.iter().any(|t| t == &format!("servers|{id}")) {
            list.retain(|s| s["id"].as_str() != Some(id.as_str()));
            skipped += 1;
            continue;
        }
        let native = native_server(item);
        let existing = list.iter().position(|s| s["id"].as_str() == Some(id.as_str()));
        match existing {
            Some(_) if !overwrite => {
                skipped += 1; // 桌面导入策略 skipExisting
            }
            Some(position) => {
                list[position] = native;
                updated += 1;
            }
            None => {
                list.push(native);
                added += 1;
            }
        }
    }
    store["servers"] = json!(list);
    write_json(&path, &store)?;
    row["added"] = json!(added);
    row["updated"] = json!(updated);
    row["skipped"] = json!(skipped);
    Ok(row)
}

fn apply_jumps(app: &tauri::AppHandle, payload: &Value, strategy: &str,
               tombstones: &Vec<String>, secrets: &Value) -> Result<Value, String> {
    let mut row = row_shell("jumps");
    let path = crate::config_dir(app)?.join(crate::hub::HUB_SERVERS_FILE);
    let mut list: Vec<Value> = std::fs::read_to_string(&path).ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    let items = payload["items"].as_array().cloned().unwrap_or_default();
    let overwrite = strategy == "overwrite";
    if overwrite {
        list.clear();
    }
    let vault_key = crate::hub_vault::load_or_create_key(&crate::config_dir(app)?)?;
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut skipped = 0usize;
    for (index, item) in items.iter().enumerate() {
        let mut native = match native_jump(item, secrets) {
            Ok(native) => native,
            Err(_) => {
                push_failed(&mut row, &format!("第 {} 台服务器：档案字段不合法", index + 1));
                continue;
            }
        };
        let id = native["id"].as_str().unwrap_or("").to_string();
        if tombstones.iter().any(|t| t == &format!("jumps|{id}")) {
            list.retain(|s| s["id"].as_str() != Some(id.as_str()));
            skipped += 1;
            continue;
        }
        // 密码：明文 → 本端 vault 密文
        let plain = native["_password_plain"].as_str().unwrap_or("").to_string();
        native.as_object_mut().map(|o| o.remove("_password_plain"));
        if !plain.is_empty() {
            match crate::hub_vault::encrypt_with(&vault_key, &plain) {
                Ok(sealed) => native["password_enc"] = json!(sealed),
                Err(error) => push_failed(&mut row,
                    &format!("服务器「{id}」：密码加密失败（{error}）")),
            }
        }
        let existing = list.iter().position(|s| s["id"].as_str() == Some(id.as_str()));
        match existing {
            Some(_) if !overwrite => skipped += 1,
            Some(position) => {
                list[position] = native;
                updated += 1;
            }
            None => {
                list.push(native);
                added += 1;
            }
        }
    }
    write_json(&path, &json!(list))?;
    row["added"] = json!(added);
    row["updated"] = json!(updated);
    row["skipped"] = json!(skipped);
    Ok(row)
}

fn apply_keys(app: &tauri::AppHandle, payload: &Value, strategy: &str,
              tombstones: &Vec<String>, secrets: &Value) -> Result<Value, String> {
    let mut row = row_shell("keys");
    let dir = crate::config_dir(app)?.join(crate::hub_keys::HUB_KEYS_DIR);
    std::fs::create_dir_all(&dir).map_err(|error| format!("无法创建密钥目录: {error}"))?;
    let items = payload["items"].as_array().cloned().unwrap_or_default();
    let overwrite = strategy == "overwrite";
    if overwrite {
        let keep: Vec<String> = items.iter()
            .filter_map(|k| k["id"].as_str().map(String::from)).collect();
        if let Ok(read) = std::fs::read_dir(&dir) {
            for entry in read.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                let base = name.trim_end_matches(".pub").to_string();
                if !keep.iter().any(|k| k == &base) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut skipped = 0usize;
    for (index, item) in items.iter().enumerate() {
        let id = item["id"].as_str().unwrap_or("").trim().to_string();
        if id.is_empty() || id.contains('/') || id.contains("..") {
            push_failed(&mut row, &format!("第 {} 把密钥：元数据字段不合法", index + 1));
            continue;
        }
        if tombstones.iter().any(|t| t == &format!("keys|{id}")) {
            let _ = std::fs::remove_file(dir.join(&id));
            let _ = std::fs::remove_file(dir.join(format!("{id}.pub")));
            skipped += 1;
            continue;
        }
        let exists = dir.join(&id).exists();
        if exists && strategy != "overwrite" {
            skipped += 1;
            continue;
        }
        // 私钥 PEM：桌面「只出不进」的反向即「导入即无口令文件」
        if let Some(pem) = secrets["key:".to_string() + &id + ":private"].as_str() {
            std::fs::write(dir.join(&id), pem.as_bytes())
                .map_err(|error| format!("无法写入私钥 {id}: {error}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir.join(&id),
                    std::fs::Permissions::from_mode(0o600));
            }
        } else if !exists {
            push_failed(&mut row, &format!("密钥「{id}」：备份中无私钥材料（明文备份？）"));
            continue;
        }
        if let Some(public) = item["publicKey"].as_str() {
            if !public.trim().is_empty() {
                let _ = std::fs::write(dir.join(format!("{id}.pub")), public.as_bytes());
            }
        }
        // LWW 时间戳挂到 sidecar
        if let Some(stamp) = item["updatedAt"].as_str() {
            let mut times = load_item_times(app);
            times["keys|".to_string() + &id] = json!(stamp);
            let _ = save_item_times(app, &times);
        }
        if exists { updated += 1 } else { added += 1 }
    }
    row["added"] = json!(added);
    row["updated"] = json!(updated);
    row["skipped"] = json!(skipped);
    Ok(row)
}

fn write_json(path: &std::path::Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录: {error}"))?;
    }
    let temporary = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(value).map_err(|error| format!("序列化失败: {error}"))?;
    std::fs::write(&temporary, content.as_bytes())
        .map_err(|error| format!("无法写入 {temporary:?}: {error}"))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("无法落盘 {path:?}: {error}"))
}

// ==================== Tauri 命令 ====================

#[tauri::command]
pub async fn backup_export(app: tauri::AppHandle, path: String, categories: Vec<String>,
                           password: Option<String>) -> Result<String, String> {
    // 密钥加密等 IO 放阻塞线程（命令本身 async，别占 IPC 线程）
    blocking_task(move || {
        let envelope = collect(&app, &categories, None)?;
        let mut envelope = envelope;
        // 导出说明：密钥口令跳过清单
        let skipped = collect_key_skip_notes(&app)?;
        if !skipped.is_empty() {
            envelope["notes"] = json!(skipped);
        }
        let bytes = serialize(&envelope, password.as_deref())?;
        std::fs::write(&path, bytes).map_err(|error| format!("无法写入 {path}: {error}"))?;
        Ok(format!("已导出 {} 类数据", categories.len()))
    }).await
}

/// 密钥口令跳过说明（收集阶段不做加密判定，导出前补一次）
fn collect_key_skip_notes(app: &tauri::AppHandle) -> Result<Vec<String>, String> {
    let dir = crate::config_dir(app)?.join(crate::hub_keys::HUB_KEYS_DIR);
    let mut notes = Vec::new();
    if let Ok(read) = std::fs::read_dir(&dir) {
        for entry in read.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file() || path.extension().map(|e| e == "pub").unwrap_or(false) {
                continue;
            }
            if crate::hub_keys::key_has_passphrase(&path) {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                notes.push(format!("密钥「{name}」带口令，已跳过私钥材料"));
            }
        }
    }
    Ok(notes)
}

#[tauri::command]
pub async fn backup_preview(app: tauri::AppHandle, path: String,
                            password: Option<String>) -> Result<Value, String> {
    blocking_task(move || {
        let raw = std::fs::read(&path).map_err(|error| format!("读取失败: {error}"))?;
        if is_encrypted_envelope(&raw)? && password.as_ref().map(|p| p.is_empty()).unwrap_or(true) {
            return Err("__NEED_PASSWORD__".to_string());
        }
        let envelope = open_envelope(&raw, password.as_deref())?;
        summarize(&app, &envelope)
    }).await
}

#[tauri::command]
pub async fn backup_import(app: tauri::AppHandle, path: String, password: Option<String>,
                           categories: Vec<String>, strategy: String) -> Result<Value, String> {
    blocking_task(move || {
        let raw = std::fs::read(&path).map_err(|error| format!("读取失败: {error}"))?;
        let envelope = open_envelope(&raw, password.as_deref())?;
        apply(&app, &envelope, &categories, &strategy, &Vec::new())
    }).await
}

/// 阻塞任务搬到工作线程（hub_keys_list 注释的同一理由；走 tauri 自带
/// async_runtime，不直接依赖 tokio —— 项目 IQ6 定稿：不引 async 运行时）
async fn blocking_task<T, F>(job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| format!("任务线程异常: {error}"))?
}

// ==================== WebDAV 同步（§9，ureq 阻塞客户端） ====================

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct SyncState {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub username: String,
    /// hub-vault 加密落盘的 WebDAV 密码
    #[serde(default)]
    pub password_enc: String,
    #[serde(default = "default_directory")]
    pub directory: String,
    #[serde(default)]
    pub period_minutes: u32,
    #[serde(default)]
    pub sync_on_change: bool,
    #[serde(default = "default_categories")]
    pub categories: Vec<String>,
    #[serde(default)]
    pub encrypt: bool,
    /// hub-vault 加密落盘的同步口令
    #[serde(default)]
    pub encrypt_password_enc: String,
    #[serde(default)]
    pub rev: u64,
    #[serde(default)]
    pub etag: String,
    #[serde(default)]
    pub tombstones: Vec<Value>,
    #[serde(default)]
    pub last_sync_ms: u64,
    #[serde(default)]
    pub last_error: String,
}

fn default_directory() -> String { "AnyBoot".to_string() }
fn default_categories() -> Vec<String> {
    vec!["servers".into(), "jumps".into(), "keys".into(), "terminal".into()]
}

impl SyncState {
    fn load(app: &tauri::AppHandle) -> Self {
        std::fs::read_to_string(state_path(app).unwrap_or_default())
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or(Self {
                enabled: false,
                server_url: String::new(),
                username: String::new(),
                password_enc: String::new(),
                directory: default_directory(),
                period_minutes: 0,
                sync_on_change: false,
                categories: default_categories(),
                encrypt: false,
                encrypt_password_enc: String::new(),
                rev: 0,
                etag: String::new(),
                tombstones: Vec::new(),
                last_sync_ms: 0,
                last_error: String::new(),
            })
    }

    fn is_configured(&self) -> bool {
        self.server_url.trim().starts_with("http://")
            || self.server_url.trim().starts_with("https://")
    }

    fn file_url(&self) -> String {
        let mut base = self.server_url.trim().trim_end_matches('/').to_string();
        let dir = self.directory.trim().trim_matches('/');
        if !dir.is_empty() {
            base = format!("{base}/{dir}");
        }
        format!("{base}/anyboot-sync.json")
    }
}

fn state_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(crate::config_dir(app)?.join("backup-sync-state.json"))
}

fn save_state(app: &tauri::AppHandle, state: &SyncState) -> Result<(), String> {
    let path = state_path(app)?;
    write_json(&path, &serde_json::to_value(state).map_err(|e| e.to_string())?)
}

fn vault_encrypt(app: &tauri::AppHandle, plain: &str) -> Result<String, String> {
    let key = crate::hub_vault::load_or_create_key(&crate::config_dir(app)?)?;
    crate::hub_vault::encrypt_with(&key, plain)
}

fn vault_decrypt(app: &tauri::AppHandle, sealed: &str) -> Result<String, String> {
    if sealed.is_empty() {
        return Ok(String::new());
    }
    let key = crate::hub_vault::load_or_create_key(&crate::config_dir(app)?)?;
    crate::hub_vault::decrypt_with(&key, sealed)
}

fn dav_request(state: &SyncState, url: &str, method: &str, body: Option<&[u8]>,
               if_match: Option<&str>, password: &str) -> Result<(u16, Vec<u8>, String), String> {
    let credentials = format!("{}:{password}", state.username);
    let auth = format!("Basic {}", B64.encode(credentials.as_bytes()));
    let mut request = ureq::request(method, url)
        .timeout(std::time::Duration::from_secs(30))
        .set("Authorization", &auth);
    if method == "PROPFIND" {
        request = request.set("Depth", "0");
    }
    if let Some(etag) = if_match {
        request = request.set("If-Match", &format!("\"{etag}\""));
    }
    let response = match body {
        Some(bytes) => request
            .set("Content-Type", "application/json")
            .send_bytes(bytes),
        None => request.call(),
    };
    match response {
        Ok(response) => {
            let code = response.status();
            let etag = response.header("ETag").unwrap_or("").replace('"', "");
            let mut body = Vec::new();
            let _ = response.into_reader().take(MAX_FILE_BYTES as u64).read_to_end(&mut body);
            Ok((code, body, etag))
        }
        Err(ureq::Error::Status(code, response)) => {
            let mut body = Vec::new();
            let _ = response.into_reader().take(1 << 20).read_to_end(&mut body);
            Ok((code, body, String::new()))
        }
        Err(error) => Err(format!("连接失败：{error}")),
    }
}

fn dav_message(code: u16) -> String {
    match code {
        401 => "401 用户名或密码错误".to_string(),
        404 => "404 远端目录不存在".to_string(),
        412 => "云端已被其他设备更新，请重试".to_string(),
        507 => "WebDAV 空间不足".to_string(),
        other => format!("服务器无响应（{other}），请稍后重试"),
    }
}

#[tauri::command]
pub async fn backup_sync_get_state(app: tauri::AppHandle) -> Result<Value, String> {
    let mut state = SyncState::load(&app);
    // 密码/口令不回传明文，只回「是否已存」
    let has_password = !state.password_enc.is_empty();
    let has_encrypt_password = !state.encrypt_password_enc.is_empty();
    state.password_enc.clear();
    state.encrypt_password_enc.clear();
    let mut value = serde_json::to_value(&state).map_err(|e| e.to_string())?;
    value["hasPassword"] = json!(has_password);
    value["hasEncryptPassword"] = json!(has_encrypt_password);
    value["isConfigured"] = json!(state.is_configured());
    Ok(value)
}

#[tauri::command]
pub async fn backup_sync_save_config(app: tauri::AppHandle, config: Value) -> Result<(), String> {
    let mut state = SyncState::load(&app);
    if let Some(url) = config["serverUrl"].as_str() { state.server_url = url.trim().to_string(); }
    if let Some(username) = config["username"].as_str() { state.username = username.trim().to_string(); }
    if let Some(password) = config["password"].as_str() {
        if !password.is_empty() {
            state.password_enc = vault_encrypt(&app, password)?;
        }
    }
    if let Some(directory) = config["directory"].as_str() {
        state.directory = if directory.trim().is_empty() { "AnyBoot".into() } else { directory.trim().to_string() };
    }
    if let Some(period) = config["periodMinutes"].as_u64() { state.period_minutes = period as u32; }
    if let Some(on_change) = config["syncOnChange"].as_bool() { state.sync_on_change = on_change; }
    if let Some(categories) = config["categories"].as_array() {
        state.categories = categories.iter()
            .filter_map(|c| c.as_str().map(String::from)).collect();
    }
    if let Some(encrypt) = config["encrypt"].as_bool() {
        state.encrypt = encrypt;
        if !encrypt {
            state.encrypt_password_enc.clear();
        }
    }
    if let Some(password) = config["encryptPassword"].as_str() {
        if !password.is_empty() {
            state.encrypt_password_enc = vault_encrypt(&app, password)?;
        }
    }
    state.enabled = config["enabled"].as_bool().unwrap_or(state.enabled);
    save_state(&app, &state)
}

#[tauri::command]
pub async fn backup_sync_set_enabled(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let mut state = SyncState::load(&app);
    state.enabled = enabled;
    save_state(&app, &state)
}

#[tauri::command]
pub async fn backup_webdav_test(app: tauri::AppHandle, config: Value) -> Result<String, String> {
    let mut state = SyncState::load(&app);
    if let Some(url) = config["serverUrl"].as_str() { state.server_url = url.trim().to_string(); }
    if let Some(username) = config["username"].as_str() { state.username = username.trim().to_string(); }
    if let Some(directory) = config["directory"].as_str() {
        state.directory = if directory.trim().is_empty() { "AnyBoot".into() } else { directory.trim().to_string() };
    }
    let password = if let Some(plain) = config["password"].as_str() {
        if !plain.is_empty() { plain.to_string() } else { vault_decrypt(&app, &state.password_enc)? }
    } else {
        vault_decrypt(&app, &state.password_enc)?
    };
    let url = state.file_url();
    let dir_url = url[..url.rfind('/').unwrap_or(0)].to_string();
    blocking_task(move || {
        let (code, _, _) = dav_request(&state, &dir_url, "PROPFIND", None, None, &password)?;
        match code {
            207 | 200 => Ok("ok".to_string()),
            404 => Ok("notFound".to_string()),
            other => Err(dav_message(other)),
        }
    }).await
}

#[tauri::command]
pub async fn backup_webdav_mkdir(app: tauri::AppHandle, config: Value) -> Result<(), String> {
    let mut state = SyncState::load(&app);
    if let Some(url) = config["serverUrl"].as_str() { state.server_url = url.trim().to_string(); }
    if let Some(username) = config["username"].as_str() { state.username = username.trim().to_string(); }
    if let Some(directory) = config["directory"].as_str() {
        state.directory = if directory.trim().is_empty() { "AnyBoot".into() } else { directory.trim().to_string() };
    }
    let password = vault_decrypt(&app, &state.password_enc)?;
    let base = state.server_url.trim().trim_end_matches('/').to_string();
    blocking_task(move || {
        let mut current = base;
        for part in state.directory.trim().trim_matches('/').split('/') {
            if part.is_empty() { continue; }
            current = format!("{current}/{part}");
            let (code, _, _) = dav_request(&state, &current, "MKCOL", Some(b""), None, &password)?;
            if !(code == 201 || code == 200 || code == 204 || code == 405) {
                return Err(dav_message(code));
            }
        }
        Ok(())
    }).await
}

#[tauri::command]
pub async fn backup_sync_now(app: tauri::AppHandle) -> Result<String, String> {
    blocking_task(move || sync_once(&app)).await
}

// ==================== 系统对话框（导入选文件 / 导出存盘） ====================

/// 选一个备份文件（导入）。取消返回 None（前端当「什么都不发生」，不弹错误）。
#[tauri::command]
pub async fn backup_pick_import_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let dialog_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        dialog_app
            .dialog()
            .file()
            .set_title("选择备份文件")
            .pick_file(move |path| {
                let _ = tx.send(path.and_then(|p| p.into_path().ok()));
            });
        rx.recv().map_err(|_| "文件选择框被关闭".to_string())
    })
    .await
    .map_err(|error| format!("文件选择框执行失败: {error}"))?
    .map(|picked| picked.map(|path| path.display().to_string()))
}

/// 导出存盘位置（系统保存对话框，默认文件名 anyboot-backup-<日期>.json）。
/// 取消返回 None。
#[tauri::command]
pub async fn backup_pick_export_path(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let suggested = format!("anyboot-backup-{}.json", iso_now()[..10].to_string());
    let dialog_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        dialog_app
            .dialog()
            .file()
            .set_title("导出备份")
            .set_file_name(&suggested)
            .pick_file(move |path| {
                let _ = tx.send(path.and_then(|p| p.into_path().ok()));
            });
        rx.recv().map_err(|_| "存盘对话框被关闭".to_string())
    })
    .await
    .map_err(|error| format!("存盘对话框执行失败: {error}"))?
    .map(|picked| picked.map(|path| path.display().to_string()))
}

fn sync_once(app: &tauri::AppHandle) -> Result<String, String> {
    let mut state = SyncState::load(app);
    if !state.is_configured() {
        return Err("请先配置 WebDAV 服务器和同步内容".to_string());
    }
    let webdav_password = vault_decrypt(app, &state.password_enc)?;
    let url = state.file_url();

    // ---- pull ----
    let (code, body, etag) = dav_request(&state, &url, "GET", None, None, &webdav_password)?;
    if code != 200 && code != 404 {
        let message = dav_message(code);
        state.last_error = message.clone();
        save_state(app, &state)?;
        return Err(message);
    }
    let mut remote_rev = state.rev;
    if code == 200 {
        let envelope = open_remote_envelope(app, &state, &body)?;
        if let Some(sync) = envelope.get("sync") {
            remote_rev = sync["rev"].as_u64().unwrap_or(state.rev);
        }
        let changed = merge_into_local(app, &envelope, &state)?;
        if !changed {
            state.rev = remote_rev;
            state.etag = etag;
            state.last_sync_ms = now_ms();
            state.last_error.clear();
            save_state(app, &state)?;
            return Ok("已是最新".to_string());
        }
    }

    // ---- push ----
    let mut envelope = collect(app, &state.categories.clone(), Some(json!({
        "deviceId": device_id(app)?,
        "rev": remote_rev + 1,
        "tombstones": state.tombstones.clone(),
    })))?;
    let password = if state.encrypt {
        let plain = vault_decrypt(app, &state.encrypt_password_enc)?;
        if plain.is_empty() {
            let message = "已开启同步加密但未设置口令".to_string();
            state.last_error = message.clone();
            save_state(app, &state)?;
            return Err(message);
        }
        Some(plain)
    } else {
        None
    };
    let payload = serialize(&envelope, password.as_deref())?;
    let if_match = state.etag.clone();
    let (code, _, new_etag) =
        dav_request(&state, &url, "PUT", Some(&payload),
            if if_match.is_empty() { None } else { Some(&if_match) }, &webdav_password)?;
    let final_etag = if code == 412 {
        // 冲突：再拉一轮合并后重推一次
        let (code, body, _) = dav_request(&state, &url, "GET", None, None, &webdav_password)?;
        if code == 200 {
            let envelope = open_remote_envelope(app, &state, &body)?;
            if let Some(sync) = envelope.get("sync") {
                remote_rev = sync["rev"].as_u64().unwrap_or(state.rev);
            }
            let _ = merge_into_local(app, &envelope, &state)?;
        }
        let retry_envelope = collect(app, &state.categories.clone(), Some(json!({
            "deviceId": device_id(app)?,
            "rev": remote_rev + 1,
            "tombstones": state.tombstones.clone(),
        })))?;
        envelope = retry_envelope;
        let payload = serialize(&envelope, password.as_deref())?;
        let (retry_code, _, retry_etag) =
            dav_request(&state, &url, "PUT", Some(&payload), None, &webdav_password)?;
        if !(retry_code == 200 || retry_code == 201 || retry_code == 204) {
            let message = dav_message(retry_code);
            state.last_error = message.clone();
            save_state(app, &state)?;
            return Err(message);
        }
        retry_etag
    } else if !(code == 200 || code == 201 || code == 204) {
        let message = dav_message(code);
        state.last_error = message.clone();
        save_state(app, &state)?;
        return Err(message);
    } else {
        new_etag
    };
    state.rev = remote_rev + 1;
    state.etag = final_etag;
    state.last_sync_ms = now_ms();
    state.last_error.clear();
    prune_tombstones(app, &mut state)?;
    save_state(app, &state)?;
    Ok("同步完成".to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn device_id(app: &tauri::AppHandle) -> Result<String, String> {
    let path = crate::config_dir(app)?.join("backup-device-id.txt");
    if let Ok(id) = std::fs::read_to_string(&path) {
        if !id.trim().is_empty() {
            return Ok(id.trim().to_string());
        }
    }
    let id = format!("desk-{:016x}", now_ms());
    std::fs::write(&path, &id).map_err(|error| format!("无法写设备标识: {error}"))?;
    Ok(id)
}

fn open_remote_envelope(app: &tauri::AppHandle, state: &SyncState,
                        body: &[u8]) -> Result<Value, String> {
    if is_encrypted_envelope(body)? {
        let plain = vault_decrypt(app, &state.encrypt_password_enc)?;
        if plain.is_empty() {
            return Err("云端备份已加密，请先在本机配置同步口令".to_string());
        }
        open_envelope(body, Some(&plain))
    } else {
        open_envelope(body, None)
    }
}

fn prune_tombstones(app: &tauri::AppHandle, state: &mut SyncState) -> Result<(), String> {
    if state.tombstones.len() > 500 {
        let half = state.tombstones.len() / 2;
        state.tombstones.drain(..half);
        save_state(app, state)?;
    }
    Ok(())
}

/// LWW 合并：canonical 条目按 updatedAt（ISO 串比）新者进本端
fn merge_into_local(app: &tauri::AppHandle, envelope: &Value, state: &SyncState) -> Result<bool, String> {
    let data = envelope["data"].as_object().ok_or("备份没有数据")?;
    let secrets = data.get("keys").and_then(|k| k.get("secrets")).cloned().unwrap_or(json!({}));
    let tombstones: Vec<String> = envelope["sync"]["tombstones"].as_array()
        .map(|a| a.iter().filter_map(|t| {
            Some(format!("{}|{}", t["cat"].as_str()?, t["id"].as_str()?))
        }).collect())
        .unwrap_or_default();
    let mut changed = false;
    let mut times = load_item_times(app);

    // servers：比 updatedAt，新者覆写
    if state.categories.iter().any(|c| c == "servers") {
        if let Some(payload) = data.get("servers") {
            let path = crate::config_dir(app)?.join(crate::remote_servers::REMOTE_SERVERS_FILE);
            let mut store: Value = std::fs::read_to_string(&path).ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_else(|| json!({"servers": []}));
            let mut list = store["servers"].as_array().cloned().unwrap_or_default();
            if let Some(items) = payload["items"].as_array() {
                for item in items {
                    let id = item["id"].as_str().unwrap_or("").to_string();
                    if id.is_empty() { continue; }
                    if tombstones.iter().any(|t| t == &format!("servers|{id}")) {
                        let before = list.len();
                        list.retain(|s| s["id"].as_str() != Some(id.as_str()));
                        if list.len() != before { changed = true; }
                        continue;
                    }
                    let stamp = item["updatedAt"].as_str().unwrap_or("");
                    let existing = list.iter()
                        .position(|s| s["id"].as_str() == Some(id.as_str()));
                    let key = format!("servers|{id}");
                    let local_stamp = times[&key].as_str().unwrap_or("");
                    let incoming_newer = existing.is_none() || ms_iso(stamp) > ms_iso(local_stamp);
                    if incoming_newer {
                        let native = native_server(item);
                        match existing {
                            Some(position) => list[position] = native,
                            None => list.push(native),
                        }
                        times[&key] = json!(stamp);
                        changed = true;
                    }
                }
            }
            if changed {
                store["servers"] = json!(list);
                write_json(&path, &store)?;
            }
        }
    }
    // jumps
    if state.categories.iter().any(|c| c == "jumps") {
        if let Some(payload) = data.get("jumps") {
            let path = crate::config_dir(app)?.join(crate::hub::HUB_SERVERS_FILE);
            let mut list: Vec<Value> = std::fs::read_to_string(&path).ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_default();
            let mut local_changed = false;
            if let Some(items) = payload["items"].as_array() {
                for item in items {
                    let id = item["id"].as_str().unwrap_or("").to_string();
                    if id.is_empty() { continue; }
                    if tombstones.iter().any(|t| t == &format!("jumps|{id}")) {
                        let before = list.len();
                        list.retain(|s| s["id"].as_str() != Some(id.as_str()));
                        if list.len() != before { local_changed = true; }
                        continue;
                    }
                    let stamp = item["updatedAt"].as_str().unwrap_or("");
                    let key = format!("jumps|{id}");
                    let local_stamp = times[&key].as_str().unwrap_or("");
                    let existing = list.iter()
                        .position(|s| s["id"].as_str() == Some(id.as_str()));
                    let incoming_newer = existing.is_none() || ms_iso(stamp) > ms_iso(local_stamp);
                    if !incoming_newer { continue; }
                    let mut native = match native_jump(item, &secrets) {
                        Ok(native) => native,
                        Err(_) => continue,
                    };
                    let plain = native["_password_plain"].as_str().unwrap_or("").to_string();
                    native.as_object_mut().map(|o| o.remove("_password_plain"));
                    if !plain.is_empty() {
                        if let Ok(sealed) = vault_encrypt(app, &plain) {
                            native["password_enc"] = json!(sealed);
                        }
                    }
                    match existing {
                        Some(position) => list[position] = native,
                        None => list.push(native),
                    }
                    times[&key] = json!(stamp);
                    local_changed = true;
                }
            }
            if local_changed {
                write_json(&path, &json!(list))?;
                changed = true;
            }
        }
    }
    // keys：合并与 apply_keys 同路径（策略=覆盖最新）
    if state.categories.iter().any(|c| c == "keys") {
        if let Some(payload) = data.get("keys") {
            // 只在「远端有比本地新的条目」时走一次落盘（复用 apply_keys 的 skip 语义不合适，
            // 这里直接按 id 逐个写；文件级幂等）
            let dir = crate::config_dir(app)?.join(crate::hub_keys::HUB_KEYS_DIR);
            let _ = std::fs::create_dir_all(&dir);
            if let Some(items) = payload["items"].as_array() {
                for item in items {
                    let id = item["id"].as_str().unwrap_or("").to_string();
                    if id.is_empty() || id.contains("..") || id.contains('/') { continue; }
                    if tombstones.iter().any(|t| t == &format!("keys|{id}")) {
                        let _ = std::fs::remove_file(dir.join(&id));
                        let _ = std::fs::remove_file(dir.join(format!("{id}.pub")));
                        changed = true;
                        continue;
                    }
                    let stamp = item["updatedAt"].as_str().unwrap_or("");
                    let key = format!("keys|{id}");
                    let local_stamp = times[&key].as_str().unwrap_or("");
                    let exists = dir.join(&id).exists();
                    let incoming_newer = !exists || ms_iso(stamp) > ms_iso(local_stamp);
                    if !incoming_newer { continue; }
                    if let Some(pem) = secrets["key:".to_string() + &id + ":private"].as_str() {
                        std::fs::write(dir.join(&id), pem.as_bytes())
                            .map_err(|error| format!("无法写入私钥: {error}"))?;
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let _ = std::fs::set_permissions(dir.join(&id),
                                std::fs::Permissions::from_mode(0o600));
                        }
                    }
                    if let Some(public) = item["publicKey"].as_str() {
                        if !public.trim().is_empty() {
                            let _ = std::fs::write(dir.join(format!("{id}.pub")), public.as_bytes());
                        }
                    }
                    times[&key] = json!(stamp);
                    changed = true;
                }
            }
        }
    }
    // terminal / history：桌面无共同子集，跳过
    if changed {
        save_item_times(app, &times)?;
    }
    Ok(changed)
}

// ==================== 往返探针（单测） ====================
//
// 不经 AppHandle 的纯函数面：信封序列化/加密/解密、canonical 键名转换、
// 时间戳互转。与移动端的「逐字节对齐」契约没有跨端 CI，这里至少把
// 自往返 + AAD 防篡改钉住（改坏参数会先在这里炸）。

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_envelope() -> Value {
        json!({
            "format": FORMAT,
            "version": VERSION,
            "app": "test",
            "platform": "desktop",
            "exportedAt": "2026-10-27T12:00:00Z",
            "includes": ["servers", "keys"],
            "data": {
                "servers": {"items": [{"id": "pc1", "machineName": "工作机"}]},
                "keys": {"items": [], "secrets": {"key:k1:private": "PEM"}},
            },
        })
    }

    #[test]
    fn plaintext_roundtrip() {
        let raw = serialize(&sample_envelope(), None).unwrap();
        let back = open_envelope(&raw, None).unwrap();
        assert_eq!(back["format"], FORMAT);
        assert_eq!(back["data"]["servers"]["items"][0]["id"], "pc1");
        assert_eq!(back["data"]["keys"]["secrets"]["key:k1:private"], "PEM");
    }

    #[test]
    fn encrypted_roundtrip_and_wrong_password() {
        let raw = serialize(&sample_envelope(), Some("口令1234")).unwrap();
        assert!(is_encrypted_envelope(&raw).unwrap());
        // 加密信封里 data 必须已被摘掉
        let envelope: Value = serde_json::from_slice(&raw).unwrap();
        assert!(envelope["data"].is_null());
        assert!(!envelope["payload"].as_str().unwrap().is_empty());
        // 错口令 → 拒绝（GCM tag 校验失败）
        assert!(open_envelope(&raw, Some("wrong-pass")).is_err());
        // 正口令 → 数据回来
        let back = open_envelope(&raw, Some("口令1234")).unwrap();
        assert_eq!(back["data"]["servers"]["items"][0]["machineName"], "工作机");
        // 缺口令 → 明确要求口令
        assert_eq!(open_envelope(&raw, None).unwrap_err(), "此备份已加密，请输入口令");
    }

    #[test]
    fn rejects_foreign_and_future_formats() {
        assert!(open_envelope(b"{}", None).is_err());
        assert!(open_envelope(b"not json", None).is_err());
        let mut future = sample_envelope();
        future["version"] = json!(99);
        let raw = serde_json::to_vec(&future).unwrap();
        assert_eq!(open_envelope(&raw, None).unwrap_err(), "备份来自更新版本，请先升级 App");
    }

    #[test]
    fn iso_time_roundtrip() {
        let cases = [
            (0u64, ""),
            (1_000_u64, "1970-01-01T00:00:01Z"),
            (1_793_188_800_000_u64, "2026-10-28T12:00:00Z"),
        ];
        for (ms, iso) in cases {
            assert_eq!(iso_ms(ms), iso);
            if ms > 0 {
                assert_eq!(ms_iso(iso), ms);
            }
        }
        // Android 毫秒数字串直接识别
        assert_eq!(ms_iso("1793188800000"), 1_793_188_800_000);
        assert_eq!(ms_iso(""), 0);
    }

    #[test]
    fn server_key_roundtrip() {
        let native = json!({
            "id": "pc1",
            "machine_name": "工作机",
            "jump_id": "j1",
            "probe_timeout_ms": 1200,
            "pairs": [{"probe_timeout_ms": 300, "created_at": 1_793_188_800_000u64}],
        });
        let canonical = canonicalize_server(&native);
        assert_eq!(canonical["machineName"], "工作机");
        assert_eq!(canonical["jumpServerId"], "j1");
        assert_eq!(canonical["probeTimeoutMs"], 1200);
        assert_eq!(canonical["pairs"][0]["probeTimeoutMs"], 300);
        assert_eq!(canonical["pairs"][0]["createdAt"], 1_793_188_800_000u64);
        let back = native_server(&canonical);
        assert_eq!(back["machine_name"], "工作机");
        assert_eq!(back["pairs"][0]["created_at"], 1_793_188_800_000u64);
    }

    #[test]
    fn native_jump_shape() {
        let canonical = json!({
            "id": "j1", "name": "家里跳板", "protocol": "ssh",
            "host": "192.168.1.10:2222", "username": "root",
            "note": "n", "keyId": "", "jumpServerId": "",
            "termPath": "", "termShell": "",
        });
        let secrets = json!({"jump:j1:password": "pw"});
        let native = native_jump(&canonical, &secrets).unwrap();
        assert_eq!(native["host"], "192.168.1.10");
        assert_eq!(native["port"], 2222);
        assert_eq!(native["_password_plain"], "pw");
        assert_eq!(native["protocol"], "ssh");
        // 无 host 且无 addresses → 拒绝
        let mut bad = canonical.clone();
        bad["host"] = json!("");
        assert!(native_jump(&bad, &secrets).is_err());
    }
}
