import Foundation
import Crypto
import Citadel

// MARK: - OpenSSH 格式（Ed25519 生成用）

extension Curve25519.Signing.PrivateKey {

    /// 公钥 SSH wire blob：string("ssh-ed25519") + string(pk)
    var ed25519PublicBlob: Data {
        var blob = Data()
        appendSSHString(&blob, Data("ssh-ed25519".utf8))
        appendSSHString(&blob, publicKey.rawRepresentation)
        return blob
    }

    /// authorized_keys 一行：ssh-ed25519 AAAA… comment
    func ed25519OpenSSHPublicKey(comment: String) -> String {
        "ssh-ed25519 \(ed25519PublicBlob.base64EncodedString())\(comment.isEmpty ? "" : " \(comment)")"
    }

    /// OpenSSH 口径指纹：SHA256(blob)
    var ed25519Fingerprint: String {
        KeyVault.sha256Fingerprint(ed25519PublicBlob)
    }

    /// 未加密的 OpenSSH 私钥 PEM（openssh-key-v1 / cipher none / kdf none）。
    /// 布局见 PROTOCOL.key：magic + none/none + 1 把公钥 + 私有段
    /// （checkint×2 + keytype + pub + priv(seed‖pub) + comment + padding）。
    func ed25519OpenSSHPrivateKey(comment: String) -> String {
        let pub = publicKey.rawRepresentation
        let seed = rawRepresentation   // 32B seed
        let check = Data((0..<4).map { _ in UInt8.random(in: 0...255) })

        var privateSection = Data()
        privateSection.append(check)
        privateSection.append(check)
        appendSSHString(&privateSection, Data("ssh-ed25519".utf8))
        appendSSHString(&privateSection, pub)
        appendSSHString(&privateSection, seed + pub)   // priv = seed ‖ pub（64B）
        appendSSHString(&privateSection, Data(comment.utf8))
        // 补位：1,2,3… 直到整段对齐 8
        var padding: UInt8 = 1
        while privateSection.count % 8 != 0 {
            privateSection.append(padding)
            padding += 1
        }

        var blob = Data("openssh-key-v1\0".utf8)
        appendSSHString(&blob, Data())                       // cipher = none
        appendSSHString(&blob, Data())                       // kdf = none
        appendSSHString(&blob, Data())                       // kdfoptions = 空
        appendUInt32(&blob, 1)                               // 1 把钥匙
        appendSSHString(&blob, ed25519PublicBlob)            // 公钥
        appendSSHString(&blob, privateSection)               // 私有段整块

        let base64 = blob.base64EncodedString()
        let lines = stride(from: 0, to: base64.count, by: 70).map {
            String(base64.dropFirst($0).prefix(70))
        }
        return "-----BEGIN OPENSSH PRIVATE KEY-----\n"
            + lines.joined(separator: "\n")
            + "\n-----END OPENSSH PRIVATE KEY-----"
    }

    private func appendSSHString(_ data: inout Data, _ value: Data) {
        appendUInt32(&data, UInt32(value.count))
        data.append(value)
    }

    private func appendUInt32(_ data: inout Data, _ value: UInt32) {
        withUnsafeBytes(of: value.bigEndian) { data.append(contentsOf: $0) }
    }
}

