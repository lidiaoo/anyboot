import Foundation
import Security

/// 敏感字段存取：iOS Keychain（kSecClassGenericPassword）。
///
/// SSH 密码 / 私钥 PEM / 密钥口令都放这里 —— Keychain 自带加密与设备保护，
/// 不进 UserDefaults、不进 iCloud 备份。存取以 `account` 字符串为键。
enum KeyVault {

    private static let service = "com.anyboot.remote.vault"

    static func save(_ secret: String, account: String) {
        guard let data = secret.data(using: .utf8) else { return }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(query as CFDictionary)   // upsert 语义：先删旧的
        var attributes = query
        attributes[kSecValueData as String] = data
        attributes[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemAdd(attributes as CFDictionary, nil)
    }

    static func load(account: String) -> String {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
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
