import Foundation
import Security

/// 敏感字段存取：iOS Keychain（kSecClassGenericPassword）。
///
/// SSH 密码 / 私钥 PEM / 密钥口令都放这里 —— Keychain 自带加密与设备保护，
/// 不进 UserDefaults、不进 iCloud 备份。存取以 `account` 字符串为键。
enum KeyVault {

    private static let service = "com.anyboot.remote.vault"

    /// 写入（upsert）。返回是否真的写进去了 —— 调用方别再把失败当成功：
    /// Android 那边「保存密码永远不生效」就是被静默 catch 吞出来的（6786caf）。
    /// 传空串 = 删掉这条（对齐 `KeyVault.encrypt("")` 不落盘的语义）。
    @discardableResult
    static func save(_ secret: String, account: String) -> Bool {
        guard !account.isEmpty else {
            NSLog("[ABVAULT] save 跳过：account 为空（secret \(secret.count) 字符）")
            return false
        }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(query as CFDictionary)   // upsert 语义：先删旧的
        guard !secret.isEmpty else { return true }
        guard let data = secret.data(using: .utf8) else { return false }
        var attributes = query
        attributes[kSecValueData as String] = data
        attributes[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(attributes as CFDictionary, nil)
        if status != errSecSuccess {
            NSLog("[ABVAULT] save 失败 account=\(account) status=\(status)")
        }
        return status == errSecSuccess
    }

    static func load(account: String) -> String {
        guard !account.isEmpty else { return "" }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status != errSecSuccess && status != errSecItemNotFound {
            NSLog("[ABVAULT] load 失败 account=\(account) status=\(status)")
        }
        guard status == errSecSuccess, let data = result as? Data else { return "" }
        return String(data: data, encoding: .utf8) ?? ""
    }

    static func delete(account: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(query as CFDictionary)
    }

    /// SHA-256 指纹（OpenSSH 口径）：SHA256:base64（无填充）
    static func sha256Fingerprint(_ data: Data) -> String {
        let digest = Data(SHA256Hasher.hash(data: data))
        return "SHA256:" + digest.base64EncodedString()
            .trimmingCharacters(in: CharacterSet(charactersIn: "="))
    }
}

/// CryptoKit 的 SHA256 起个别名，避免和 Foundation 撞名
import Crypto
private enum SHA256Hasher {
    static func hash(data: Data) -> [UInt8] {
        Array(SHA256.hash(data: data))
    }
}
