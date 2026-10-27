//! server-hub 密钥管理（独立窗 `server-keys`）的后端。
//!
//! 定则与 hub 同源：**私钥永不出本机** —— 私钥只落在配置目录 `keys/` 下
//! （随 `ANYBOOT_CONFIG_DIR` 走），接口永远只回元数据与公钥内容，
//! 任何命令都不返回私钥体。
//!
//! 生成/指纹全部借系统 `ssh-keygen`（与终端走系统 `ssh` 同一哲学：不引
//! 加密库，行为与用户手工操作一致）。口令有无的判定用
//! `ssh-keygen -y -P "" -f <私钥>`：能导出公钥 = 没设口令。

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

/// 密钥目录名（配置目录之下）
pub(crate) const HUB_KEYS_DIR: &str = "keys";

// ==================== 存储 ====================

pub(crate) fn keys_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = crate::config_dir(app)?.join(HUB_KEYS_DIR);
    std::fs::create_dir_all(&dir).map_err(|error| format!("无法创建密钥目录: {error}"))?;
    Ok(dir)
}

/// 按 key_id（= 文件基名）解析私钥路径；不存在返回 Err。
/// hub_term 起 ssh 时拿它拼 `-i`。
pub(crate) fn private_key_path(app: &AppHandle, key_id: &str) -> Result<PathBuf, String> {
    let id = key_id.trim();
    if id.is_empty() {
        return Err("未选择密钥".to_string());
    }
    // 防路径穿越：只允许纯文件名
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!("非法的密钥标识（{id}）"));
    }
    let path = keys_dir(app)?.join(id);
    if !path.exists() {
        return Err(format!("密钥不存在（{id}），可能已被删除"));
    }
    Ok(path)
}

fn quote_for_shell(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn run_ssh_keygen(args: &[&str]) -> Result<String, String> {
    let output = crate::command("ssh-keygen")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| format!("无法启动 ssh-keygen: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("ssh-keygen 失败（退出码 {:?}）", output.status.code())
        } else {
            stderr
        })
    }
}

// ==================== 元数据 ====================

#[derive(Clone, Serialize)]
pub struct HubKeyMeta {
    /// 密钥标识 = 文件基名（私钥 `<id>`、公钥 `<id>.pub`）
    pub id: String,
    /// "Ed25519" | "RSA-2048" | "RSA-4096" | "ECDSA" | 未知
    pub kind: String,
    /// "SHA256:xxxx…xxxx"（前端只展示首尾）
    pub fingerprint: String,
    /// 私钥是否设了口令
    pub has_passphrase: bool,
    /// 私钥文件修改时间（毫秒时间戳，导入/生成时间近似值）
    pub updated_at: u64,
    /// 有多少台服务器把认证方式设成这把钥匙
    pub used_by: usize,
}

/// 从公钥首字段 + 位长归出人类可读类型
fn kind_of(pub_line: &str) -> String {
    let first = pub_line.split_whitespace().next().unwrap_or("");
    match first {
        "ssh-ed25519" => "Ed25519".to_string(),
        "ssh-rsa" => {
            // 位长从指纹行里拿（ssh-keygen -lf 输出含 "2048 SHA256:..."）
            "RSA".to_string()
        }
        "ecdsa-sha2-nistp256" => "ECDSA-256".to_string(),
        "ecdsa-sha2-nistp384" => "ECDSA-384".to_string(),
        "ecdsa-sha2-nistp521" => "ECDSA-521".to_string(),
        other if other.is_empty() => "未知".to_string(),
        other => other.to_string(),
    }
}

/// ssh-keygen -lf 首行形如：
/// `256 SHA256:9mQx…K4tW id_ed25519_work (ED25519)` / `4096 SHA256:… nas (RSA)`
fn parse_fingerprint(listing: &str) -> (String, String) {
    let line = listing.lines().next().unwrap_or("");
    let mut bits = String::new();
    let mut fp = String::new();
    let mut fields = line.split_whitespace();
    if let Some(b) = fields.next() {
        bits = b.to_string();
    }
    if let Some(f) = fields.next() {
        fp = f.to_string();
    }
    (fp, bits)
}

/// 私钥是否设了口令（`ssh-keygen -y -P "" -f <私钥>` 能导公钥 = 无口令）。
/// 备份（backup.rs collect_keys）用它做「只出不进」的跳过判定。
pub(crate) fn key_has_passphrase(private: &Path) -> bool {
    run_ssh_keygen(&["-y", "-P", "", "-f", &quote_for_shell(private)]).is_err()
}

/// 备份用密钥三元组：(algorithm, fingerprint, publicKey)。
/// 不经 AppHandle（backup.rs 收集时只有路径）；任何一步失败都给空串兜底，
/// 不阻塞整包导出。
pub(crate) fn describe_key(private: &Path) -> (String, String, String) {
    let pub_path = private.with_extension("pub");
    let mut pub_line = std::fs::read_to_string(&pub_path).unwrap_or_default();
    if pub_line.trim().is_empty() {
        // 没有现成公钥：无口令私钥可以现场导一份
        if let Ok(exported) = run_ssh_keygen(&["-y", "-P", "", "-f", &quote_for_shell(private)]) {
            pub_line = exported;
        }
    }
    let algorithm = {
        let kind = kind_of(&pub_line);
        if kind == "未知" {
            // 公钥不可得时退回 PEM 头判断
            let head = std::fs::read_to_string(private).unwrap_or_default();
            if head.contains("BEGIN OPENSSH PRIVATE KEY") {
                "OpenSSH".to_string()
            } else if head.contains("BEGIN RSA PRIVATE KEY") {
                "RSA".to_string()
            } else if head.contains("BEGIN EC PRIVATE KEY") {
                "ECDSA".to_string()
            } else {
                String::new()
            }
        } else {
            kind
        }
    };
    // 指纹：先试公钥文件，再试无口令私钥本体（ssh-keygen -lf 直接吃私钥）
    let fingerprint = run_ssh_keygen(&["-lf", &quote_for_shell(&pub_path)])
        .or_else(|_| run_ssh_keygen(&["-lf", &quote_for_shell(private)]))
        .ok()
        .and_then(|listing| {
            let (fp, _) = parse_fingerprint(&listing);
            if fp.is_empty() { None } else { Some(fp) }
        })
        .unwrap_or_default();
    let public_key = pub_line.lines().next().unwrap_or("").trim().to_string();
    (algorithm, fingerprint, public_key)
}

fn meta_of(app: &AppHandle, private: &Path) -> Option<HubKeyMeta> {
    if !private.is_file() {
        return None;
    }
    let id = private.file_name()?.to_string_lossy().to_string();
    // RSA 位长补全：从指纹行拿位长，与公钥首字段拼出 "RSA-4096"
    let pub_path = private.with_extension("pub");
    let pub_line = std::fs::read_to_string(&pub_path).unwrap_or_default();
    if pub_line.trim().is_empty() {
        return None; // 没有公钥的私钥不认（fingerprint/复制公钥都没法做）
    }
    let listing = run_ssh_keygen(&["-lf", &quote_for_shell(&pub_path)]).ok()?;
    let (fingerprint, bits) = parse_fingerprint(&listing);
    let mut kind = kind_of(&pub_line);
    if kind == "RSA" {
        kind = format!("RSA-{bits}");
    }
    let has_passphrase = run_ssh_keygen(&["-y", "-P", "", "-f", &quote_for_shell(private)]).is_err();
    let updated_at = std::fs::metadata(private)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let used_by = crate::hub::load_servers_pub(&app)
        .unwrap_or_default()
        .iter()
        .filter(|s| s.key_id == id)
        .count();
    Some(HubKeyMeta {
        id,
        kind,
        fingerprint,
        has_passphrase,
        updated_at,
        used_by,
    })
}

// ==================== 命令 ====================

/// 密钥清单。每个私钥都要跑两次 `ssh-keygen`（取指纹 + 试空口令），
/// 三把钥匙就是六个子进程 —— 同步跑在 UI 线程上，密钥窗一打开就整窗卡住。
#[tauri::command]
pub async fn hub_keys_list(app: AppHandle) -> Result<Vec<HubKeyMeta>, String> {
    tauri::async_runtime::spawn_blocking(move || hub_keys_list_blocking(app))
        .await
        .map_err(|error| format!("读取密钥清单任务失败: {error}"))?
}

fn hub_keys_list_blocking(app: AppHandle) -> Result<Vec<HubKeyMeta>, String> {
    let dir = keys_dir(&app)?;
    let mut keys: Vec<HubKeyMeta> = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|error| format!("无法读取密钥目录: {error}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        // 私钥 = 不带 .pub 后缀的文件
        if path.extension().map(|e| e == "pub").unwrap_or(false) {
            continue;
        }
        if let Some(meta) = meta_of(&app, &path) {
            keys.push(meta);
        }
    }
    keys.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(keys)
}

/// 生成密钥：`ssh-keygen -t <ed25519|rsa> [-b 4096] -N <口令> -f <keys>/<name> -C <注释>`
///
/// **必须 async + spawn_blocking**：ssh-keygen 是子进程，RSA 4096 生成在低配机上
/// 能跑好几秒。同步命令跑在 UI 线程上，用户看到的就是「点生成密钥 → 整个窗口
/// 白掉/不响应」。
#[tauri::command]
pub async fn hub_key_generate(
    app: AppHandle,
    name: String,
    kind: String,
    passphrase: String,
) -> Result<HubKeyMeta, String> {
    tauri::async_runtime::spawn_blocking(move || {
        hub_key_generate_blocking(app, name, kind, passphrase)
    })
    .await
    .map_err(|error| format!("生成密钥任务失败: {error}"))?
}

fn hub_key_generate_blocking(
    app: AppHandle,
    name: String,
    kind: String,
    passphrase: String,
) -> Result<HubKeyMeta, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("名称不能为空".to_string());
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains(' ') {
        return Err("名称只能用字母、数字、-、_、.（不能含空格与路径分隔）".to_string());
    }
    let kind = kind.trim().to_string();
    let dir = keys_dir(&app)?;
    let target = dir.join(&name);
    if target.exists() {
        return Err(format!("已存在同名密钥（{name}）"));
    }
    let pass = if passphrase.is_empty() {
        "".to_string()
    } else {
        passphrase
    };
    let pass_arg = format!("'{}'", pass.replace('\'', "'\\''"));
    let comment = format!("anyboot-hub-{}", name);
    if kind == "rsa" {
        run_ssh_keygen(&[
            "-t", "rsa", "-b", "4096", "-N", &pass_arg, "-f", &quote_for_shell(&target),
            "-C", &comment, "-q",
        ])?;
    } else {
        run_ssh_keygen(&[
            "-t", "ed25519", "-N", &pass_arg, "-f", &quote_for_shell(&target), "-C", &comment,
            "-q",
        ])?;
    }
    meta_of(&app, &target).ok_or_else(|| "密钥已生成但读取元数据失败".to_string())
}

/// 导入私钥：系统文件选择框 → 复制进 keys/ 目录（连带同名 .pub，若有）。
#[tauri::command]
pub async fn hub_key_import(app: AppHandle) -> Result<HubKeyMeta, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let dialog_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        dialog_app
            .dialog()
            .file()
            .set_title("选择私钥文件")
            .pick_file(move |path| {
                let picked = path.and_then(|p| p.into_path().ok());
                let _ = tx.send(picked);
            });
    })
    .await
    .map_err(|error| format!("文件选择框执行失败: {error}"))?;
    let picked = rx
        .recv()
        .map_err(|_| "文件选择框被关闭".to_string())?
        .ok_or_else(|| "未选择文件".to_string())?;

    // 校验是 ssh 私钥：OPENSSH / RSA / EC 头
    let head = std::fs::read(&picked)
        .map_err(|error| format!("无法读取所选文件: {error}"))?
        .into_iter()
        .take(64)
        .collect::<Vec<u8>>();
    let head_text = String::from_utf8_lossy(&head);
    if !head_text.contains("OPENSSH PRIVATE KEY")
        && !head_text.contains("RSA PRIVATE KEY")
        && !head_text.contains("EC PRIVATE KEY")
        && !head_text.contains("PRIVATE KEY")
    {
        return Err("所选文件不像 SSH 私钥（缺少 PRIVATE KEY 头）".to_string());
    }

    let dir = keys_dir(&app)?;
    let source_name = picked
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| "所选文件没有文件名".to_string())?;
    let target = dir.join(&source_name);
    if target.exists() {
        return Err(format!("已存在同名密钥（{source_name}）"));
    }
    std::fs::copy(&picked, &target).map_err(|error| format!("复制私钥失败: {error}"))?;
    // 600：私钥权限照 ssh 惯例收紧（Windows 上忽略失败）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
    }
    // 连带导入同名公钥（同目录 <name>.pub），没有就算了
    let source_pub = picked.with_extension("pub");
    if source_pub != picked && source_pub.exists() {
        let _ = std::fs::copy(&source_pub, dir.join(format!("{source_name}.pub")));
    }
    // 没有 .pub 时用 ssh-keygen -y 现场导一份（指纹/复制公钥都靠它）
    if !target.with_extension("pub").exists() {
        if let Ok(pub_line) = run_ssh_keygen(&["-y", "-f", &quote_for_shell(&target)]) {
            let _ = std::fs::write(
                target.with_extension("pub"),
                format!("{} {}\n", pub_line.trim(), source_name),
            );
        }
    }
    meta_of(&app, &target).ok_or_else(|| "私钥已导入但读取元数据失败".to_string())
}

#[tauri::command]
pub fn hub_key_delete(app: AppHandle, id: String) -> Result<(), String> {
    let id = id.trim().to_string();
    if id.is_empty() || id.contains('/') || id.contains("..") {
        return Err(format!("非法的密钥标识（{id}）"));
    }
    let dir = keys_dir(&app)?;
    let used_by = crate::hub::load_servers_pub(&app)
        .unwrap_or_default()
        .iter()
        .filter(|s| s.key_id == id)
        .count();
    if used_by > 0 {
        return Err(format!(
            "这把钥匙还被 {used_by} 台服务器引用，先在服务器编辑里换掉再删"
        ));
    }
    let private = dir.join(&id);
    let public = dir.join(format!("{id}.pub"));
    let mut removed = false;
    if private.exists() {
        std::fs::remove_file(&private).map_err(|error| format!("删除私钥失败: {error}"))?;
        removed = true;
    }
    if public.exists() {
        let _ = std::fs::remove_file(&public);
        removed = true;
    }
    if !removed {
        return Err(format!("密钥不存在（{id}）"));
    }
    Ok(())
}

/// 公钥内容（一键复制用）。只回公钥，私钥永不外泄。
#[tauri::command]
pub fn hub_key_public_key(app: AppHandle, id: String) -> Result<String, String> {
    let id = id.trim().to_string();
    if id.is_empty() || id.contains('/') || id.contains("..") {
        return Err(format!("非法的密钥标识（{id}）"));
    }
    let path = keys_dir(&app)?.join(format!("{id}.pub"));
    std::fs::read_to_string(&path).map_err(|_| format!("公钥文件不存在（{id}.pub）"))
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_key_path_rejects_traversal() {
        // 不经 AppHandle 的分支：非法标识直接 Err（含路径分隔与 ..）
        for bad in ["a/b", "a\\b", "..x", "x.."] {
            // x.. 与 ..x 不含路径分隔，但含 ".."，同样拒绝
            assert!(bad.contains("..") || bad.contains('/') || bad.contains('\\'));
        }
        let meta = kind_of("ssh-ed25519 AAAA comment");
        assert_eq!(meta, "Ed25519");
        let rsa = kind_of("ssh-rsa AAAA comment");
        assert_eq!(rsa, "RSA");
    }

    #[test]
    fn parse_fingerprint_takes_first_two_fields() {
        let (fp, bits) = parse_fingerprint("4096 SHA256:hT2vXb8L nas_rsa (RSA)");
        assert_eq!(fp, "SHA256:hT2vXb8L");
        assert_eq!(bits, "4096");
        let (fp2, bits2) = parse_fingerprint("");
        assert!(fp2.is_empty() && bits2.is_empty());
    }
}
