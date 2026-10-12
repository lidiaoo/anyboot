import Foundation

/// 服务端列表的持久化（UserDefaults + JSON）。
///
/// 1.0 只存了单个 `host` / `token`，升级到这里时会把那一条搬成列表里的第一个
/// 服务端（再顺手写进一个分组，方便用户后面继续加同一台电脑的其它 IP）。
final class ServerStore: ObservableObject {

    static let shared = ServerStore()

    private let keyServers = "servers"
    private let keyActive = "active_id"
    private let keyConnectAll = "connect_all"
    private let keyMigrated = "migrated_multi_server"
    private let legacyHost = "host"
    private let legacyToken = "token"

    @Published private(set) var servers: [ServerProfile] = []
    @Published private(set) var activeId: String = ""
    /// 启动时是否把所有服务端一起连上（多个 WKWebView 同时在线）
    @Published private(set) var connectAll: Bool = false

    private let defaults = UserDefaults.standard

    private init() {
        load()
    }

    // MARK: - 读写

    func load() {
        var list: [ServerProfile] = []
        if let data = defaults.data(forKey: keyServers),
           let decoded = try? JSONDecoder().decode([ServerProfile].self, from: data) {
            list = decoded.filter { !$0.host.isEmpty }
        }
        migrateLegacy(into: &list)
        sort(&list)
        servers = list
        activeId = defaults.string(forKey: keyActive) ?? ""
        connectAll = defaults.bool(forKey: keyConnectAll)
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
            profile.host = host
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
        if let index = servers.firstIndex(where: { $0.id == profile.id }) {
            servers[index] = profile
        } else {
            servers.append(profile)
        }
        sort(&servers)
        save()
    }

    func remove(_ id: String) {
        servers.removeAll { $0.id == id }
        if activeId == id {
            activeId = servers.first?.id ?? ""
            persistActive()
        }
        save()
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
