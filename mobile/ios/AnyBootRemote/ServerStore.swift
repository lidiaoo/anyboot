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
    private let keySortMode = "server_sort_mode"
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
        sortMode = Self.cleanSortMode(defaults.string(forKey: keySortMode))
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
        switch sortMode {
        case "recent": sortRecent(&list)
        case "name": sortName(&list)
        default: sortCustom(&list)
        }
    }

    // MARK: 排序（用户 2026-11-01，与 PC / 安卓同一套语义）

    /// 排序模式（@Published：菜单勾选跟着变）："custom" / "name" / "recent"
    @Published var sortMode: String = "custom" {
        didSet {
            guard sortMode != oldValue else { return }
            defaults.set(sortMode, forKey: keySortMode)
            sort(&servers)
        }
    }

    /// 切排序模式（菜单调用；didSet 里统一重排）
    func setSortMode(_ mode: String) {
        sortMode = Self.cleanSortMode(mode)
    }

    private static func cleanSortMode(_ raw: String?) -> String {
        switch raw ?? "" {
        case "name", "recent": return raw!
        default: return "custom"
        }
    }

    /// 自定义顺序：有 sortIndex 的按序号升序，没排过的按名称接在后面（与 PC sort_servers_custom 一致）
    private func sortCustom(_ list: inout [ServerProfile]) {
        list.sort { left, right in
            if left.sortIndex >= 0 || right.sortIndex >= 0 {
                if left.sortIndex < 0 { return false }
                if right.sortIndex < 0 { return true }
                if left.sortIndex != right.sortIndex { return left.sortIndex < right.sortIndex }
            }
            return nameThenId(left, right)
        }
    }

    /// 显示名 A→Z
    private func sortName(_ list: inout [ServerProfile]) {
        list.sort { nameThenId($0, $1) }
    }

    /// 最近连接优先：刚连过的排前面，没用过/久没用的按名称排后面
    private func sortRecent(_ list: inout [ServerProfile]) {
        list.sort { left, right in
            if left.lastUsedAt != right.lastUsedAt { return left.lastUsedAt > right.lastUsedAt }
            return nameThenId(left, right)
        }
    }

    private func nameThenId(_ left: ServerProfile, _ right: ServerProfile) -> Bool {
        let byName = left.sortKey.localizedCaseInsensitiveCompare(right.sortKey)
        if byName != .orderedSame { return byName == .orderedAscending }
        return left.id.localizedCaseInsensitiveCompare(right.id) == .orderedAscending
    }

    /// 「最近连接」排序：记一次成功连接（页加载完成才算）。
    /// 不走 upsert —— updatedAt 是云同步 LWW 的依据，记「用过」不该把它顶上去。
    func touchUsed(_ id: String) {
        guard let index = servers.firstIndex(where: { $0.id == id }) else { return }
        servers[index].lastUsedAt = Date().timeIntervalSince1970 * 1000
        sort(&servers)
        save()
    }

    /// 自定义排序里挪一台（▴▾）：先把当前顺序冻结成 sortIndex 0..n-1
    /// （老档案没排过，直接交换会跳回名称序），再移到目标位。
    func move(_ id: String, delta: Int) {
        var list = servers
        guard let index = list.firstIndex(where: { $0.id == id }) else { return }
        let target = index + delta
        guard target >= 0, target < list.count else { return }
        for i in list.indices { list[i].sortIndex = i }
        let profile = list.remove(at: index)
        list.insert(profile, at: target)
        for i in list.indices { list[i].sortIndex = i }
        sort(&list)
        servers = list
        save()
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
