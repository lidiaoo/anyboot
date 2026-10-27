import Foundation

/// 服务端列表的持久化（UserDefaults + JSON）。
///
/// 1.0 只存了单个 `host` / `token`，升级到这里时会把那一条搬成列表里的第一个
/// 服务端（再顺手写进一个分组，方便用户后面继续加同一台电脑的其它 IP）。
final class ServerStore: ObservableObject {

    static let shared = ServerStore()

    static let transportAuto = "auto"
    static let transportPush = "ws"
    static let transportPoll = "http"
    /// 三个可选值（菜单/对话框的顺序就是这个）
    static let transports = [transportAuto, transportPush, transportPoll]

    private let keyServers = "servers"
    private let keyActive = "active_id"
    private let keyConnectAll = "connect_all"
    private let keyTransport = "transport"
    private let keyMigrated = "migrated_multi_server"
    private let legacyHost = "host"
    private let legacyToken = "token"

    @Published private(set) var servers: [ServerProfile] = []
    @Published private(set) var activeId: String = ""
    /// 启动时是否把所有服务端一起连上（多个 WKWebView 同时在线）
    @Published private(set) var connectAll: Bool = false
    /// 传输方式偏好：`auto` / `ws` / `http`（加载页面时以 `?transport=` 交给网页决定）
    @Published private(set) var transport: String = ServerStore.transportAuto

    private let defaults = UserDefaults.standard

    private init() {
        load()
    }

    // MARK: - 读写

    func load() {
        var list: [ServerProfile] = []
        if let data = defaults.data(forKey: keyServers),
           let decoded = try? JSONDecoder().decode([ServerProfile].self, from: data) {
            list = decoded.filter { !$0.addresses.isEmpty }
        }
        migrateLegacy(into: &list)
        sort(&list)
        servers = list
        activeId = defaults.string(forKey: keyActive) ?? ""
        connectAll = defaults.bool(forKey: keyConnectAll)
        transport = Self.cleanTransport(defaults.string(forKey: keyTransport))
        if !servers.contains(where: { $0.id == activeId }) {
            activeId = servers.first?.id ?? ""
        }
    }

    private func migrateLegacy(into list: inout [ServerProfile]) {
        guard !defaults.bool(forKey: keyMigrated) else { return }
        let oldHost = defaults.string(forKey: legacyHost) ?? ""
        let oldToken = defaults.string(forKey: legacyToken) ?? ""
        if let host = ServerProfile.normalizeHost(oldHost) {
            var profile = ServerProfile()
            profile.addAddress(host)
            profile.name = host
            profile.token = oldToken
            list.append(profile)
        }
        defaults.set(true, forKey: keyMigrated)
        defaults.removeObject(forKey: legacyHost)
        defaults.removeObject(forKey: legacyToken)
        if !list.isEmpty {
            servers = list
            save()
        }
    }

    func save() {
        if let data = try? JSONEncoder().encode(servers) {
            defaults.set(data, forKey: keyServers)
        }
    }

    func persistActive() {
        defaults.set(activeId, forKey: keyActive)
    }

    func setConnectAll(_ value: Bool) {
        connectAll = value
        defaults.set(value, forKey: keyConnectAll)
    }

    /// 选传输方式：`auto`（推送优先，连不上退回轮询）/ `ws`（只用推送）/ `http`（只用轮询）。
    ///
    /// 存在 App 侧而不是只存网页端：网页端的 localStorage 按**来源站点**存，
    /// 这台电脑换网卡就会换地址（= 换来源），偏好会跟着丢。加载页面时以
    /// `?transport=` 追加到 URL 上交给网页决定走哪条通道。
    func setTransport(_ value: String) {
        transport = Self.cleanTransport(value)
        defaults.set(transport, forKey: keyTransport)
    }

    /// 只认这三个值，其余（含 nil / 空串 / 拼错）一律回落到 `auto`
    static func cleanTransport(_ raw: String?) -> String {
        let value = (raw ?? "").trimmedAnyBoot.lowercased()
        return transports.contains(value) ? value : transportAuto
    }

    // MARK: - 增删改查

    var active: ServerProfile? {
        servers.first { $0.id == activeId }
    }

    func profile(_ id: String) -> ServerProfile? {
        servers.first { $0.id == id }
    }

    func select(_ id: String) {
        guard servers.contains(where: { $0.id == id }) else { return }
        activeId = id
        persistActive()
    }

    func upsert(_ profile: ServerProfile) {
        var profile = profile
        profile.updatedAt = Date()
        if let index = servers.firstIndex(where: { $0.id == profile.id }) {
            servers[index] = profile
        } else {
            servers.append(profile)
        }
        sort(&servers)
        save()
        Task { @MainActor in SyncEngine.shared.notifyChanged() }
    }

    func remove(_ id: String) {
        if servers.contains(where: { $0.id == id }) {
            Task { @MainActor in SyncStore.shared.recordDeletion(cat: "servers", id: id) }
        }
        servers.removeAll { $0.id == id }
        if activeId == id {
            activeId = servers.first?.id ?? ""
            persistActive()
        }
        save()
        Task { @MainActor in SyncEngine.shared.notifyChanged() }
    }

    /// 只更新这一条（**不重排**）：给「记住哪个地址是通的」这类运行期小改动用，
    /// 免得标签顺序突然变一下。
    func updateInPlace(_ profile: ServerProfile) {
        guard let index = servers.firstIndex(where: { $0.id == profile.id }) else { return }
        var updated = profile
        updated.updatedAt = Date()
        servers[index] = updated
        save()
    }

    /// 整表替换（覆盖导入 / 同步合并后的全量落地）
    func replaceAll(_ list: [ServerProfile], activeId newActiveId: String) {
        var list = list
        sort(&list)
        servers = list
        activeId = list.contains(where: { $0.id == newActiveId }) ? newActiveId : (list.first?.id ?? "")
        save()
        persistActive()
    }

    /// 把「其实是同一台电脑」的多条记录并成一条 —— 给老配置用：以前每个 IP 一条
    /// 记录的老用户，点一下就能收成「一台电脑一份页面」。
    ///
    /// @return 合并掉的条数
    @discardableResult
    func mergeSameMachine() -> Int {
        var removed = 0
        var index = 0
        while index < servers.count {
            var keep = servers[index]
            var scan = servers.count - 1
            while scan > index {
                var drop = servers[scan]
                guard keep.sameMachine(drop) else {
                    scan -= 1
                    continue
                }
                if drop.id == activeId {
                    // 正在用的那条留下来（页面不跳走）：和 keep 换个位置
                    servers.swapAt(index, scan)
                    swap(&keep, &drop)
                }
                keep.mergeAddresses(from: drop)
                if keep.machineId.trimmedAnyBoot.isEmpty { keep.machineId = drop.machineId }
                if keep.machineName.trimmedAnyBoot.isEmpty { keep.machineName = drop.machineName }
                if keep.group.trimmedAnyBoot.isEmpty { keep.group = drop.group }
                if keep.name.trimmedAnyBoot.isEmpty { keep.name = drop.name }
                if keep.token.trimmedAnyBoot.isEmpty { keep.token = drop.token }
                servers.remove(at: scan)
                removed += 1
                scan -= 1
            }
            servers[index] = keep
            index += 1
        }
        if removed > 0 {
            sort(&servers)
            if !servers.contains(where: { $0.id == activeId }) {
                activeId = servers.first?.id ?? ""
                persistActive()
            }
            save()
        }
        return removed
    }

    func sort(_ list: inout [ServerProfile]) {
        list.sort { $0.sortKey.localizedCaseInsensitiveCompare($1.sortKey) == .orderedAscending }
    }

    /// 已有的分组名（给「分组」输入框做候选）
    var knownGroups: [String] {
        var groups: [String] = []
        for profile in servers {
            let name = profile.group.trimmedAnyBoot
            if !name.isEmpty && !groups.contains(name) {
                groups.append(name)
            }
        }
        return groups.sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }
    }
}
