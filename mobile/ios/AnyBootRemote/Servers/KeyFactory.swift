import Foundation
import Crypto
import Citadel

/// 密钥的导入 / 本机生成（对齐 `ServersActivity.kt:1654` generateKey / `:1702` importKey）。
enum KeyFactory {

    enum KeyError: LocalizedError {
        case notPem
        case tooBig
        case unsupported

        var errorDescription: String? {
            switch self {
            case .notPem: return "不像 PEM 格式的私钥（应以 -----BEGIN 开头）"
            case .tooBig: return "文件太大，不像私钥"
            case .unsupported: return "这个私钥格式暂不支持"
            }
        }
    }

    /// 导入一个私钥文件（PEM）。私钥进 Keychain，公钥 / 指纹一起落库。
    static func importPem(at url: URL, fallbackName: String) throws -> SshKeyEntry {
        let secured = url.startAccessingSecurityScopedResource()
        defer { if secured { url.stopAccessingSecurityScopedResource() } }
        let data = try Data(contentsOf: url)
        if data.count > 256 * 1024 { throw KeyError.tooBig }
        let pem = String(decoding: data, as: UTF8.self).trimmedAnyBoot
        guard pem.hasPrefix("-----BEGIN") else { throw KeyError.notPem }

        var entry = SshKeyEntry()
        entry.name = url.lastPathComponent.isEmpty ? fallbackName : url.lastPathComponent
        entry.privateRef = "key-\(entry.id)"
        KeyVault.save(pem, account: entry.privateRef)
        if pem.contains("ssh-ed25519") {
            entry.algorithm = "ssh-ed25519"
            let privateKey = try Curve25519.Signing.PrivateKey(sshEd25519: pem)
            let publicData = privateKey.publicKey.rawRepresentation
            entry.fingerprint = KeyVault.sha256Fingerprint(Data(publicData))
            entry.publicKey = "ssh-ed25519 \(Data(publicData).base64EncodedString())"
        } else {
            entry.algorithm = "ssh-rsa"
            let privateKey = try Insecure.RSA.PrivateKey(sshRsa: pem)
            // CryptoKit 不暴露 n/e，本地拼不了 ssh-rsa wire 公钥；
            // 指纹取 DER rawRepresentation（App 内一致性用途），authorized_keys 行留空
            entry.fingerprint = KeyVault.sha256Fingerprint(privateKey.publicKey.rawRepresentation)
            entry.publicKey = ""
        }
        return entry
    }

    /// 本机生成一对密钥。
    static func generate(name: String, ed25519: Bool) throws -> SshKeyEntry {
        guard ed25519 else {
            // RSA 生成的 OpenSSH 导出依赖 Citadel 的序列化细节，一期只支持 Ed25519 生成（推荐）；
            // RSA 走「导入私钥文件」——与 Android 端「本机只生成不了 Ed25519、只给 RSA」正好相反，
            // 两边各自保留能跑通的那一种。
            throw KeyError.unsupported
        }
        let privateKey = Curve25519.Signing.PrivateKey()
        var entry = SshKeyEntry()
        entry.name = name
        entry.privateRef = "key-\(entry.id)"
        entry.algorithm = "ssh-ed25519"
        let openSSH = privateKey.ed25519OpenSSHPrivateKey(comment: name)
        entry.publicKey = privateKey.ed25519OpenSSHPublicKey(comment: name)
        entry.fingerprint = privateKey.ed25519Fingerprint
        KeyVault.save(openSSH, account: entry.privateRef)
        return entry
    }
}
