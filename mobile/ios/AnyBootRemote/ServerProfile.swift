import SwiftUI

extension String {
    /// 去掉首尾空白（分组名/服务端名/token 输入统一走这里）
    var trimmedAnyBoot: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}

/// 一个服务端条目 = 一台电脑上的一个 AnyBoot 实例（一个 IP:端口）。
///
/// 同一个分组里可以放**多块网卡 / 多次 DHCP 拿到多个 IP** 的同一台电脑：
/// 标签上显示「组名 · 服务端名」，一眼就知道这几个标签其实是同一台机器。
struct ServerProfile: Identifiable, Codable, Hashable {
    var id: String = UUID().uuidString
    /// 分组名（区分不同的电脑）；空串 = 未分组
    var group: String = ""
    /// 服务端名；空串 = 用地址兜底
    var name: String = ""
    /// 归一化后的 host:port
    var host: String = ""
    /// 访问令牌，可空
    var token: String = ""

    static let defaultPort = 12300

    /// 标签上显示的服务端名
    var displayName: String {
        let value = name.trimmedAnyBoot
        return value.isEmpty ? host : value
    }

    /// 标签文本：组名 + 服务端名（未分组时只显示服务端名）
    var label: String {
        let groupName = group.trimmedAnyBoot
        return groupName.isEmpty ? displayName : "\(groupName) · \(displayName)"
    }

    /// 带 token 的访问地址
    var url: URL? {
        guard !host.isEmpty else { return nil }
        var components = URLComponents()
        components.scheme = "http"
        let parts = host.split(separator: ":")
        components.host = parts.first.map(String.init)
        if parts.count > 1, let port = Int(parts[1]) {
            components.port = port
        }
        let value = token.trimmedAnyBoot
        if !value.isEmpty {
            // 前端支持 ?token=，验证通过后会写进 cookie，省掉手工输入
            components.queryItems = [URLQueryItem(name: "token", value: value)]
        }
        return components.url
    }

    /// 排序键：先按分组（未分组排最后），再按显示名
    var sortKey: String {
        let groupName = group.trimmedAnyBoot
        let prefix = groupName.isEmpty ? "\u{FFFF}" : groupName
        return "\(prefix)\u{0}\(displayName)\u{0}\(host)"
    }

    /// 分组配色：同一个分组恒定同一个颜色，扫一眼标签栏就知道哪几个是同一台电脑
    var groupColor: Color {
        let groupName = group.trimmedAnyBoot
        guard !groupName.isEmpty else { return Color(.systemGray3) }
        var hash: UInt64 = 5381
        for byte in groupName.utf8 {
            hash = hash &* 33 &+ UInt64(byte)
        }
        let hue = Double(hash % 360) / 360.0
        return Color(hue: hue, saturation: 0.45, brightness: 0.78)
    }

    /// 用户可能粘贴整条 URL，也可能只写 IP，统一归一成 host:port
    static func normalizeHost(_ raw: String) -> String? {
        var value = raw.trimmedAnyBoot
        guard !value.isEmpty else { return nil }
        if let range = value.range(of: "://") {
            value = String(value[range.upperBound...])
        }
        for separator in ["/", "?"] {
            if let range = value.range(of: separator) {
                value = String(value[..<range.lowerBound])
            }
        }
        value = value.trimmedAnyBoot
        guard !value.isEmpty else { return nil }
        if !value.contains(":") {
            value += ":\(defaultPort)"
        }
        return value
    }

    /// 从粘贴的 URL 里顺手把 token 挖出来
    static func token(fromURL raw: String) -> String {
        guard raw.contains("token=") else { return "" }
        if let components = URLComponents(string: raw.trimmedAnyBoot),
           let value = components.queryItems?.first(where: { $0.name == "token" })?.value {
            return value.trimmedAnyBoot
        }
        guard let range = raw.range(of: "token=") else { return "" }
        let tail = String(raw[range.upperBound...])
        return tail.split(separator: "&").first.map(String.init)?.trimmedAnyBoot ?? ""
    }
}
