//! server-hub 密码认证的凭据保管（M6）。
//!
//! 定则：**明文密码只在内存里活一瞬** ——
//! - 落盘：AES-256-GCM 密文，写在 hub-servers.json 的 `password_enc` 字段；
//!   密钥是 `hub-vault.key`（32B 随机，0600，首次用密时生成）。
//! - 使用：ssh 进程要密码时走 `SSH_ASKPASS_REQUIRE=force`，askpass = 本程序
//!   自身（`--hub-askpass <server_id>`，见 cli.rs）：内存解密 → stdout 喂给
//!   ssh 的管道 → 进程退出，明文不碰磁盘、不进环境变量。
//!
//! 与密钥认证互斥：编辑表单三选一（交互 / 密码 / 密钥），起 ssh 时只带其一。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

/// 密钥文件名（挂在配置目录下，与 hub-servers.json 同级）
pub(crate) const VAULT_KEY_FILE: &str = "hub-vault.key";

// ==================== 纯函数（可单测；cli.rs 的 askpass 分支也走这里） ====================

/// 加密：base64( nonce[12] || AES-256-GCM 密文 )
pub(crate) fn encrypt_with(key: &[u8; 32], plain: &str) -> Result<String, String> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(key)
        .map_err(|error| format!("vault 密钥长度异常: {error}"))?;
    let nonce_bytes = random_12();
    let nonce = aes_gcm::Nonce::from_slice(&nonce_bytes);
    let sealed = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plain.as_bytes(),
                aad: b"anyboot-hub-password",
            },
        )
        .map_err(|error| format!("密码加密失败: {error}"))?;
    let mut packed = Vec::with_capacity(12 + sealed.len());
    packed.extend_from_slice(&nonce_bytes);
    packed.extend_from_slice(&sealed);
    Ok(B64.encode(packed))
}

/// 解密（`encrypt_with` 的逆操作）
pub(crate) fn decrypt_with(key: &[u8; 32], sealed: &str) -> Result<String, String> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    let packed = B64
        .decode(sealed.trim())
        .map_err(|_| "密码密文不是合法 base64（可能被手改过）".to_string())?;
    if packed.len() < 12 + 16 {
        return Err("密码密文长度异常（可能被截断）".to_string());
    }
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(key)
        .map_err(|error| format!("vault 密钥长度异常: {error}"))?;
    let plain = cipher
        .decrypt(
            aes_gcm::Nonce::from_slice(&packed[..12]),
            Payload {
                msg: &packed[12..],
                aad: b"anyboot-hub-password",
            },
        )
        .map_err(|_| "密码解密失败：vault 密钥与加密时不一致".to_string())?;
    String::from_utf8(plain).map_err(|_| "密码密文解出了非法 UTF-8".to_string())
}

fn random_12() -> [u8; 12] {
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).expect("系统熵源不可用");
    nonce
}

// ==================== 配置目录视角 ====================

/// 读 vault 密钥，没有就生成（父目录已由调用方保证存在时才会被走到；
/// 这里也兜底建一次）。文件权限 0600 —— 只许本用户读写。
pub(crate) fn load_or_create_key(config: &std::path::Path) -> Result<[u8; 32], String> {
    use std::io::Write;
    let path = config.join(VAULT_KEY_FILE);
    if let Ok(raw) = std::fs::read(&path) {
        let bytes: [u8; 32] = raw
            .try_into()
            .map_err(|_| format!("{} 长度异常（应为 32 字节）", path.display()))?;
        return Ok(bytes);
    }
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|error| format!("生成 vault 密钥失败: {error}"))?;
    std::fs::create_dir_all(config).map_err(|error| format!("无法创建配置目录: {error}"))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode_600(&path)
        .open(&path)
        .map_err(|error| format!("无法创建 {}: {error}", path.display()))?;
    file.write_all(&key)
        .map_err(|error| format!("无法写入 {}: {error}", path.display()))?;
    Ok(key)
}

/// unix 0600 的 OpenOptions 扩展（Windows 上无权限位，直接落盘）
trait Mode600 {
    fn mode_600(&mut self, path: &std::path::Path) -> &mut Self;
}

#[cfg(unix)]
impl Mode600 for std::fs::OpenOptions {
    fn mode_600(&mut self, path: &std::path::Path) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600);
        let _ = path; // 权限随 create_new 落地（mode 只在 create 时生效）
        self
    }
}

#[cfg(not(unix))]
impl Mode600 for std::fs::OpenOptions {
    fn mode_600(&mut self, _path: &std::path::Path) -> &mut Self {
        self
    }
}

/// AppHandle 视角的配置目录 + 加解密（hub.rs 保存档案时用）
pub(crate) fn encrypt(app: &tauri::AppHandle, plain: &str) -> Result<String, String> {
    let config = crate::config_dir(app)?;
    let key = load_or_create_key(&config)?;
    encrypt_with(&key, plain)
}

/// askpass 脚本：ssh 通过 `SSH_ASKPASS` 调它，它 exec 回本程序的 CLI 分支。
/// server_id 写死在脚本里（ssh 只会以 `<askpass> <提示语>` 的形式调用，没法传参）。
pub(crate) fn askpass_script_path(config: &std::path::Path) -> std::path::PathBuf {
    config.join("hub-askpass.sh")
}

/// （重）写 askpass 脚本并补执行位。每次起 ssh 都重写：服务器 id / 程序路径变了也不怕。
pub(crate) fn write_askpass_script(
    config: &std::path::Path,
    server_id: &str,
) -> Result<std::path::PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let exe = std::env::current_exe().map_err(|error| format!("定位自身可执行文件失败: {error}"))?;
    let exe = exe.display().to_string().replace('\'', "'\\''");
    let path = askpass_script_path(config);
    let script = format!("#!/bin/sh\nexec '{exe}' --hub-askpass '{server_id}'\n");
    std::fs::write(&path, script).map_err(|error| format!("无法写 {}: {error}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("无法设 {} 执行位: {error}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_aad_binding() {
        let key = [7u8; 32];
        let sealed = encrypt_with(&key, "p@ss w0rd 密码").unwrap();
        assert_ne!(sealed, "p@ss w0rd 密码");
        assert_eq!(decrypt_with(&key, &sealed).unwrap(), "p@ss w0rd 密码");
        // 同一明文两次加密密文不同（随机 nonce）
        assert_ne!(encrypt_with(&key, "x").unwrap(), encrypt_with(&key, "x").unwrap());
    }

    #[test]
    fn wrong_key_and_tamper_fail() {
        let sealed = encrypt_with(&[1u8; 32], "secret").unwrap();
        assert!(decrypt_with(&[2u8; 32], &sealed).is_err());
        // 篡改密文一个字节 → GCM 校验失败
        let mut raw = B64.decode(&sealed).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        assert!(decrypt_with(&[1u8; 32], &B64.encode(&raw)).is_err());
        // 非法 base64 / 截断输入
        assert!(decrypt_with(&[1u8; 32], "not-base64!!").is_err());
        assert!(decrypt_with(&[1u8; 32], &B64.encode([0u8; 10])).is_err());
    }
}
