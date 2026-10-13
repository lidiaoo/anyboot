import SwiftUI

extension String {
    /// 去掉首尾空白（分组名/服务端名/token/机器标识输入统一走这里）
    var trimmedAnyBoot: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}

/// 一条记录 = **一台电脑**（不是一个 IP）。
///
/// 一台电脑可能有多块网卡、或者 DHCP 每次给不同的 IP —— 这些地址其实是同一台机器，
/// 所以这里存的是一个**有序候选地址列表** `addresses`：
///
/// - 第 0 个是首选地址（上次连上的那个会被自动提到最前，下次直连）；
/// - 加载失败就换下一个地址，但**还是同一个标签、同一个 WKWebView** ——
///   一台电脑永远只占一份页面，不会「一个网卡一个标签」；
/// - 候选地址全试完才认为这台电脑连不上。
///
/// 「是不是同一台电脑」优先用电脑端 `GET /health` 给的 `machineId` 判断
/// （见 `sameMachine(_:)`）；拿不到标识时退回「分组名相同」这个用户可见的约定。
struct ServerProfile: Identifiable, Hashable {

    var id: String = UUID().uuidString
    /// 分组名（区分不同的电脑）；空串 = 未分组
    var group: String = ""
    /// 服务端名；空串 = 用地址兜底
    var name: String = ""
    /// 候选地址（有序，第 0 个是首选）：**唯一事实来源**，存储时才拆成 `host` 兜底
    var addresses: [String] = []
    /// 访问令牌，可空
    var token: String = ""
    /// 电脑端 `/health` 给的机器标识（同一台电脑的多个 IP 靠它归并）；未知为空
    var machineId: String = ""
    /// 电脑端 `/health` 给的机器名（「这是哪台电脑」）；未知为空
    var machineName: String = ""

    static let defaultPort = 12300

    init() {
    }

    // MARK: - 序列化（自己写：老数据只有单个 host，要能读进来）

    private enum CodingKeys: String, CodingKey {
        case id, group, name, addresses, host, token, machineId, machineName
    }

    /// 旧版（1.1 及更早）一个条目只有一个 `host` —— 读的时候当成唯一的候选地址。
    /// 老配置里的键缺失也不能解码失败，所以全部用 `try?` + 兜底。
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = (try? container.decode(String.self, forKey: .id)) ?? UUID().uuidString
        group = (try? container.decode(String.self, forKey: .group)) ?? ""
        name = (try? container.decode(String.self, forKey: .name)) ?? ""
        token = (try? container.decode(String.self, forKey: .token)) ?? ""
        machineId = (try? container.decode(String.self, forKey: .machineId)) ?? ""
        machineName = (try? container.decode(String.self, forKey: .machineName)) ?? ""
        let list = (try? container.decode([String].self, forKey: .addresses)) ?? []
        let legacy = (try? container.decode(String.self, forKey: .host)) ?? ""
        addresses = Self.clean(legacy.isEmpty ? list : list + [legacy])
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(group, forKey: .group)
        try container.encode(name, forKey: .name)
        try container.encode(addresses, forKey: .addresses)
        // host = 首选地址：1.1 及更早的客户端读的是这个键，继续写出去，旧版仍读得懂
        try container.encode(host, forKey: .host)
        try container.encode(token, forKey: .token)
        try container.encode(machineId, forKey: .machineId)
        try container.encode(machineName, forKey: .machineName)
    }

    // MARK: - 地址

    /// 首选地址（没有地址时是空串）
    var host: String { addresses.first ?? "" }

    /// 第 index 个候选地址（越界给空串，调用方不用自己判边界）
    func host(at index: Int) -> String {
        addresses.indices.contains(index) ? addresses[index] : ""
    }

    /// 追加一个地址（归一化 + 去重）。返回是否真的加进去了。
    @discardableResult
    mutating func addAddress(_ raw: String) -> Bool {
        guard let value = Self.normalizeHost(raw), !addresses.contains(value) else { return false }
        addresses.append(value)
        return true
    }

    /// 用输入替换地址列表（编辑入口：以输入的顺序为准）
    mutating func replaceAddresses(_ raw: [String]) {
        addresses = Self.clean(raw)
    }

    /// 把这个地址提到最前（下次优先用它 —— 刚连通的那个地址才是「这台电脑现在的地址」）。
    /// 本来就是首选时返回 false。
    @discardableResult
    mutating func promoteAddress(_ index: Int) -> Bool {
        guard index > 0, addresses.indices.contains(index) else { return false }
        addresses.insert(addresses.remove(at: index), at: 0)
        return true
    }

    /// 并入另一条记录的地址（去重，本条已有的顺序不变），返回新增个数
    @discardableResult
    mutating func mergeAddresses(from other: ServerProfile) -> Int {
        var added = 0
        for address in other.addresses where addAddress(address) {
            added += 1
        }
        return added
    }

    /// 编辑框内容：一行一个地址
    var addressText: String { addresses.joined(separator: "\n") }

    /// 列表 / 错误面板里显示的地址摘要：首选地址（还有备用时补一句）
    var addressSummary: String {
        guard addresses.count > 1 else { return host }
        return "\(host)（另有 \(addresses.count - 1) 个地址）"
    }

    /// 解析地址输入框：**一行一个地址**（也兼容逗号 / 分号 / 空格分隔），
    /// 同一台电脑的多块网卡可以一次全填进来。归一化 + 去重，保持输入顺序。
    static func parseAddresses(_ raw: String) -> [String] {
        let separators = CharacterSet(charactersIn: "\r\n,;\u{FF0C}\u{FF1B}").union(.whitespaces)
        return clean(raw.components(separatedBy: separators))
    }

    /// 归一化 + 去掉空项 + 去重（保持顺序）
    static func clean(_ raw: [String]) -> [String] {
        var result: [String] = []
        for item in raw {
            guard let value = normalizeHost(item) else { continue }
            if !result.contains(value) {
                result.append(value)
            }
        }
        return result
    }

    // MARK: - 展示

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

    /// 带 token 的访问地址（首选地址）
    var url: URL? { url(at: 0) }

    /// 第 index 个候选地址的访问地址
    func url(at index: Int) -> URL? {
        url(at: index, transport: "")
    }

    /// 第 index 个候选地址的访问地址，并附带传输方式偏好（空 / `auto` 时不加）。
    ///
    /// 为什么这个参数要由 App 从 URL 上带过去：网页端的 localStorage 按**来源站点**
    /// 存，一台电脑有多块网卡时会换地址（换地址 = 换来源），偏好就跟着丢了。
    /// App 每次加载都带上，换地址也不会丢。
    func url(at index: Int, transport: String) -> URL? {
        let value = host(at: index)
        guard !value.isEmpty else { return nil }
        var components = URLComponents()
        components.scheme = "http"
        let parts = value.split(separator: ":")
        components.host = parts.first.map(String.init)
        if parts.count > 1, let port = Int(parts[1]) {
            components.port = port
        }
        var items: [URLQueryItem] = []
        let secret = token.trimmedAnyBoot
        if !secret.isEmpty {
            // 前端支持 ?token=，验证通过后会写进 cookie，省掉手工输入
            items.append(URLQueryItem(name: "token", value: secret))
        }
        let mode = transport.trimmedAnyBoot
        if !mode.isEmpty && mode != ServerStore.transportAuto {
            items.append(URLQueryItem(name: "transport", value: mode))
        }
        if !items.isEmpty {
            components.queryItems = items
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

    // MARK: - 「是不是同一台电脑」

    /// 是不是同一台电脑（可以合并成一条记录）。
    ///
    /// 优先用电脑端的机器标识比对：两边都拿到了 `machineId` 时以标识为准
    /// （标识不同就是不同电脑，哪怕分组名写一样）。任一边拿不到标识时，退回
    /// 「分组名相同」这个用户可见的约定 —— 分组本来就是「一台电脑」的意思。
    func sameMachine(_ other: ServerProfile) -> Bool {
        guard other.id != id else { return false }
        let mine = machineId.trimmedAnyBoot
        let theirs = other.machineId.trimmedAnyBoot
        if !mine.isEmpty && !theirs.isEmpty {
            return mine.caseInsensitiveCompare(theirs) == .orderedSame
        }
        let myGroup = group.trimmedAnyBoot
        return !myGroup.isEmpty
            && myGroup.caseInsensitiveCompare(other.group.trimmedAnyBoot) == .orderedSame
    }

    // MARK: - 解析

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
