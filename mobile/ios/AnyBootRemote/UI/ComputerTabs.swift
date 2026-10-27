import SwiftUI

// MARK: - 电脑页（多标签，对齐 MainActivity.kt 2074 行）
//
// 每台电脑一个**保活** WebView：切走隐藏、切回不重载。顶部标签条可横向滚动，
// 每个 chip = 状态点 + 名称 + 系统徽标 + 连接标注；长按编辑。

@MainActor
final class ComputerTab: ObservableObject, Identifiable {

    let profileId: String
    var profile: ServerProfile
    let box = ABWebViewBox()

    @Published var status: ABLoadStatus = .unknown
    @Published var addressIndex = 0
    @Published var started = false
    @Published var errorDetail = ""

    var tried: Set<String> = []
    var triedRemote = false
    var triedJump = false
    var usingRemote = false
    var effectiveAddress = ""
    var mainFailed = false

    /// 当前 tab 才弹连接过程 toast（对齐 Android `if (profile.id == activeId) toast(...)`）
    private var isActiveTab: Bool { ComputerTabCenter.shared.activeId == profileId }
    private func activeToast(_ text: String) {
        guard isActiveTab else { return }
        ABToastBus.shared.show(text)
    }

    init(profile: ServerProfile) {
        self.profileId = profile.id
        self.profile = profile
        box.isCurrent = { [weak self] url in self?.isCurrentAddress(url) ?? true }
        box.onStart = { [weak self] in
            guard let self else { return }
            self.mainFailed = false
            self.status = .loading
        }
        box.onFinish = { [weak self] in
            guard let self else { return }
            if self.status != .failed {
                self.status = .ok
                self.rememberWorkingAddress()
            }
        }
        box.onFail = { [weak self] message in
            guard let self else { return }
            self.mainFailed = true
            if self.advanceAddress() { return }
            self.status = .failed
            self.errorDetail = self.composeErrorDetail(message)
        }
        box.onReloadCommand = { [weak self] in
            self?.connect(forceReload: true)
        }
    }

    var webView: UIView { box.webView }

    // MARK: 错误明细（MainActivity.showErrorFor 1795-1814）

    /// 「主名（另有 N 个地址）」—— Android ServerProfile.addressSummary 同式
    private func addressSummary() -> String {
        let primary = ServerProfile.normalizeHost(profile.host) ?? profile.host
        let spare = profile.addressCount - 1
        return spare > 0 ? "\(primary)（另有 \(spare) 个地址）" : primary
    }

    /// 失败面板的明细：服务端/地址头 + 描述 + 多地址清单 + 「请确认」两条
    private func composeErrorDetail(_ description: String) -> String {
        var detail = "服务端：\(profile.label)\n地址：\(addressSummary())\n\n\(description)"
        let count = profile.addressCount
        if count > 1 {
            let list = (0..<count).map { "  " + profile.hostAt($0) }.joined(separator: "\n")
            detail += "\n\n这台电脑的 \(count) 个地址都试过了：\n\(list)"
        }
        detail += "\n\n请确认：\n"
            + "· 电脑上 AnyBoot 的「远程控制」已打开，手机和电脑在同一局域网\n"
            + "· 地址填的是电脑上的 IP 和端口（例如 192.168.0.120:12300）"
        return detail
    }

    // MARK: 连接

    func connect(forceReload: Bool) {
        if !forceReload && started {
            // 已经连过就保留后台页面，切回来直接显示，不重新拉
            return
        }
        addressIndex = 0
        tried = []
        triedRemote = false
        triedJump = false
        loadCurrentAddress()
    }

    /// 没探出赢家时的初始路径：优先级表里「直连」与「跳板机」谁在前走谁
    private var defaultLocalKind: String {
        for candidate in profile.routeCandidates() {
            if candidate.kind == ServerProfile.routeLan || candidate.kind == ServerProfile.routeJump {
                return candidate.kind
            }
        }
        let jumpUsable = !profile.jumpServerId.trimmingCharacters(in: .whitespaces).isEmpty
            && !profile.routeDisabled.contains(ServerProfile.routeJump)
        return jumpUsable ? ServerProfile.routeJump : ServerProfile.routeLan
    }

    private func loadCurrentAddress() {
        let address = profile.hostAt(addressIndex)
        if address.isEmpty { return }

        // 通道选择听电脑列表的机制：心跳赢家优先，没探过按优先级
        let winner = Heartbeat.shared.state(profile.id).kind
        let useJump: Bool
        switch winner {
        case ServerProfile.routeLan: useJump = false
        case ServerProfile.routeJump: useJump = true
        default: useJump = defaultLocalKind == ServerProfile.routeJump
        }

        usingRemote = false
        tried.remove(address)     // 正在试的这个不算「试过了」
        mainFailed = false
        started = true
        status = .loading

        if useJump && !profile.jumpServerId.trimmingCharacters(in: .whitespaces).isEmpty {
            connectThroughTunnel(address)
            return
        }
        effectiveAddress = ""
        guard let url = profile.url(at: addressIndex, transport: ServerStore.shared.transport) else {
            status = .failed
            errorDetail = composeErrorDetail("地址还没就绪")
            return
        }
        box.load(url)
    }

    /// 走跳板机：开（或复用）SSH 本地转发，然后加载 127.0.0.1:端口
    private func connectThroughTunnel(_ destAddress: String) {
        Task {
            do {
                let local = try await SshSessionManager.shared.tunnelAddress(for: profile,
                                                                            destination: destAddress)
                guard let url = profile.url(for: local, transport: ServerStore.shared.transport) else {
                    status = .failed
                    errorDetail = composeErrorDetail("跳板机隧道地址不可用")
                    return
                }
                effectiveAddress = local
                box.load(url)
            } catch {
                if !advanceAddress() {
                    mainFailed = true
                    status = .failed
                    errorDetail = composeErrorDetail("经由跳板机连接失败：\(error.localizedDescription)")
                }
            }
        }
    }

    /// 换到下一个还没试过的地址；全试完就走跳板机兜底，再兜底远程通道
    private func advanceAddress() -> Bool {
        if usingRemote {
            // 远程优先但隧道挂了：退回局域网链路继续试
            guard profile.routeCandidates().first?.isRemote == true else { return false }
            usingRemote = false
            tried = []
        }
        tried.insert(profile.hostAt(addressIndex))
        for index in 0..<profile.addressCount {
            let address = profile.hostAt(index)
            if tried.contains(address) { continue }
            addressIndex = index
            status = .loading
            // Android advanceAddress 1496
            activeToast("这个地址不通，换一个地址重试：\(address)")
            loadCurrentAddress()
            return true
        }
        if connectJump() { return true }
        return connectRemote()
    }

    /// 局域网候选全挂 → 跳板机兜底（本轮只兜一次）
    private func connectJump() -> Bool {
        if triedJump { return false }
        if profile.jumpServerId.trimmingCharacters(in: .whitespaces).isEmpty { return false }
        if profile.routeDisabled.contains(ServerProfile.routeJump) { return false }
        if profile.addresses.isEmpty { return false }
        triedJump = true
        tried = []
        mainFailed = false
        started = true
        status = .loading
        // Android connectJump 1526
        activeToast("局域网不通，改走跳板机…")
        connectThroughTunnel(profile.hostAt(0))
        return true
    }

    /// 局域网候选全挂 → 远程接进兜底（本轮只试一次）
    private func connectRemote() -> Bool {
        if triedRemote { return false }
        guard let winner = profile.routeCandidates().first(where: { $0.isRemote }) else { return false }
        triedRemote = true
        usingRemote = true
        tried = []
        mainFailed = false
        started = true
        status = .loading
        effectiveAddress = ""
        // Android connectRemote 1567
        activeToast("局域网不通，改走\(ServerProfile.routeLabel(winner.kind))…")
        let secret = winner.token.trimmingCharacters(in: .whitespaces)
        var raw = winner.origin
        if !secret.isEmpty {
            raw += (raw.contains("?") ? "&" : "/?") + "ab_token="
                + (secret.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? secret)
        }
        guard let url = URL(string: raw) else {
            status = .failed
            errorDetail = composeErrorDetail("远程地址不合法")
            return false
        }
        box.load(url)
        return true
    }

    /// 连上了：把当前地址提到最前（下次优先用它）—— 多网卡 / DHCP 换 IP 能自愈的原因
    private func rememberWorkingAddress() {
        guard addressIndex != 0, var updated = ServerStore.shared.profile(profileId) else { return }
        guard updated.promoteAddress(addressIndex) else { return }
        addressIndex = 0
        ServerStore.shared.updateInPlace(updated)
        profile = updated
    }

    /// 这个回调说的是「当前正在用的那个地址」吗？
    private func isCurrentAddress(_ url: String) -> Bool {
        guard let parsed = URL(string: url) else { return false }
        if usingRemote {
            let origin = profile.flatPairs.first.map { ServerProfile.normalizeRemoteOrigin($0.addr) } ?? ""
            return !origin.isEmpty && url.hasPrefix(origin)
        }
        var expected: String
        if !effectiveAddress.isEmpty {
            expected = effectiveAddress
        } else {
            expected = ServerProfile.normalizeHost(profile.hostAt(addressIndex)) ?? ""
        }
        if expected.hasPrefix("http://") { expected.removeFirst(7) }
        else if expected.hasPrefix("https://") { expected.removeFirst(8) }
        let actual = (parsed.host ?? "") + (parsed.port.map { ":\($0)" } ?? "")
        return expected == actual
    }

    func reload() {
        connect(forceReload: true)
    }
}

// MARK: - 标签中心

@MainActor
final class ComputerTabCenter: ObservableObject {

    static let shared = ComputerTabCenter()

    @Published private(set) var tabs: [ComputerTab] = []
    @Published var activeId: String = ""

    private init() {}

    func tab(for profile: ServerProfile) -> ComputerTab {
        if let existing = tabs.first(where: { $0.profileId == profile.id }) {
            existing.profile = profile
            return existing
        }
        let created = ComputerTab(profile: profile)
        tabs.append(created)
        return created
    }

    func activate(_ id: String) {
        guard let profile = ServerStore.shared.profile(id) else { return }
        activeId = id
        ServerStore.shared.select(id)
        let tab = tab(for: profile)
        tab.connect(forceReload: false)
    }

    func connectAll() {
        for profile in ServerStore.shared.servers {
            tab(for: profile).connect(forceReload: false)
        }
    }

    func reloadActive() {
        guard let tab = tabs.first(where: { $0.profileId == activeId }) else { return }
        tab.reload()
    }

    func reloadAll() {
        for tab in tabs { tab.reload() }
    }

    func close(_ id: String) {
        tabs.removeAll { $0.profileId == id }
        if activeId == id {
            activeId = tabs.first?.profileId ?? ""
        }
    }

    func syncProfiles() {
        // 档案删掉了，标签也跟着走
        let alive = Set(ServerStore.shared.servers.map(\.id))
        tabs.removeAll { !alive.contains($0.profileId) }
        for tab in tabs {
            if let fresh = ServerStore.shared.profile(tab.profileId) { tab.profile = fresh }
        }
        if !alive.contains(activeId) {
            activeId = tabs.first?.profileId ?? ""
        }
    }
}

/// 连不上时自动重载的间隔（秒；0 = 永不）。
/// 默认 5 秒 —— 与安卓 MainActivity:174 相同（之前 iOS 未设置时读到 0，等于默认关闭）。
@MainActor
enum FailReloadPref {
    static var sec: Int {
        get {
            UserDefaults.standard.object(forKey: "failReloadSec") == nil
                ? 5 : UserDefaults.standard.integer(forKey: "failReloadSec")
        }
        set { UserDefaults.standard.set(newValue, forKey: "failReloadSec") }
    }
}
