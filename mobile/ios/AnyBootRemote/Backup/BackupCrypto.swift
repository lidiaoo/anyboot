import Foundation
import CryptoKit
import CommonCrypto

// MARK: - 备份加密（docs/backup-design.md §3.2）
//
// PBKDF2-HMAC-SHA256（310000 轮，16B 盐）派生 256 位密钥
// → AES-256-GCM（12B IV）加密整份明文 JSON。
//
// 不用 Keystore/Keychain：口令派生密钥必须跨设备可解（换机迁移语义）。
// 口令不落盘、不进备份文件；GCM 校验失败统一报「口令错误或文件已损坏」。
//
// **三端逐字节对齐契约**（Android `BackupCrypto.java` / 桌面 `backup_crypto.rs` 同参）：
// - PBKDF2：HMAC-SHA256，rounds=310000，salt=16B 随机，dkLen=32
// - AES-256-GCM：IV 12B 随机，tag 16B 追加在密文尾部（CryptoKit combined 默认即此）
// - AAD = `BackupEncryption.aadSeed` 确定性字符串（防篡改 version/rounds/salt/iv）

enum BackupCryptoError: LocalizedError {
    case badBase64
    case decryptFailed      // 口令错误或文件已损坏（安全语义：不区分两种情况）

    var errorDescription: String? {
        switch self {
        case .badBase64: return "备份文件已损坏"
        case .decryptFailed: return "口令错误或文件已损坏"
        }
    }
}

enum BackupCrypto {

    /// 加密：明文 JSON → envelope.payload（base64(ciphertext+tag)），并填好 encryption 字段。
    static func encrypt(plain: Data, password: String) throws -> (payload: String, encryption: BackupEncryption) {
        var salt = Data(count: 16)
        var iv = Data(count: 12)
        let saltStatus = salt.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, 16, $0.baseAddress!) }
        let ivStatus = iv.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, 12, $0.baseAddress!) }
        guard saltStatus == errSecSuccess, ivStatus == errSecSuccess else {
            throw BackupCryptoError.decryptFailed
        }
        let saltB64 = salt.base64EncodedString()
        let ivB64 = iv.base64EncodedString()
        let enc = BackupEncryption(salt: saltB64, iv: ivB64, rounds: BackupConstants.pbkdf2Rounds)

        let key = try deriveKey(password: password, salt: salt, rounds: enc.rounds)
        let symmetric = SymmetricKey(data: key)
        let sealed = try AES.GCM.seal(plain, using: symmetric, authenticating: Data(enc.aadSeed.utf8))
        guard let combined = sealed.combined else { throw BackupCryptoError.decryptFailed }
        return (combined.base64EncodedString(), enc)
    }

    /// 解密：envelope.payload → 明文 JSON。口令错/tag 坏/base64 坏统一抛 `decryptFailed`
    /// （调用方文案固定「口令错误或文件已损坏」，不区分两种情况 —— 安全语义）。
    static func decrypt(payloadB64: String, encryption: BackupEncryption, password: String) throws -> Data {
        guard let combined = Data(base64Encoded: payloadB64),
              let salt = Data(base64Encoded: encryption.salt),
              let iv = Data(base64Encoded: encryption.iv), iv.count == 12 else {
            throw BackupCryptoError.badBase64
        }
        let key = try deriveKey(password: password, salt: salt, rounds: encryption.rounds)
        let box = try? AES.GCM.SealedBox(combined: combined)
        guard let box else { throw BackupCryptoError.decryptFailed }
        do {
            let plain = try AES.GCM.open(box, using: SymmetricKey(data: key),
                                         authenticating: Data(encryption.aadSeed.utf8))
            return plain
        } catch {
            // tag 校验失败 = 口令错或文件被改 —— 统一口径
            throw BackupCryptoError.decryptFailed
        }
    }

    /// PBKDF2-HMAC-SHA256 → 32 字节
    static func deriveKey(password: String, salt: Data, rounds: Int) throws -> Data {
        var derived = Data(count: 32)
        let passwordData = Data(password.utf8)
        let status = derived.withUnsafeMutableBytes { derivedPtr in
            salt.withUnsafeBytes { saltPtr in
                passwordData.withUnsafeBytes { passwordPtr in
                    CCKeyDerivationPBKDF(
                        CCPBKDFAlgorithm(kCCPBKDF2),
                        passwordPtr.baseAddress!.assumingMemoryBound(to: Int8.self),
                        passwordData.count,
                        saltPtr.baseAddress!.assumingMemoryBound(to: UInt8.self),
                        salt.count,
                        CCPseudoRandomAlgorithm(kCCPRFHmacAlgSHA256),
                        UInt32(rounds),
                        derivedPtr.baseAddress!.assumingMemoryBound(to: UInt8.self),
                        32
                    )
                }
            }
        }
        guard status == kCCSuccess else { throw BackupCryptoError.decryptFailed }
        return derived
    }
}
