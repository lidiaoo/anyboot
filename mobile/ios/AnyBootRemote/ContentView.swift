import SwiftUI

/// iOS 端主界面：顶部电脑标签栏 + 每台电脑一个 WKWebView。
///
/// 标签文本是「组名 · 服务端名」，左侧小圆点是该电脑的在线状态。
/// **一台电脑只有一个标签**：多块网卡 / 多次 DHCP 拿到的多个 IP 是同一台机器的
/// 候选地址（见 `ServerProfile.addresses`），首选地址不通就自动换下一个，
/// 始终是同一个标签、同一个 WKWebView。
struct ContentView: View {

    @StateObject private var store = ServerStore.shared
    /// 10 秒心跳：顶部标签状态点的事实来源（在线绿 / 离线灰）
    @ObservedObject private var heartbeat = Heartbeat.shared

    @State private var statuses: [String: LoadStatus] = [:]
    @State private var reloadTokens: [String: Int] = [:]
    @State private var errors: [String: String] = [:]
    /// 每台电脑当前用第几个候选地址
    @State private var addressIndex: [String: Int] = [:]
    /// 本轮连接里已经试过、失败的地址（换地址时跳过）
    @State private var triedAddresses: [String: [String]] = [:]

    @State private var editing: ServerProfile?
    @State private var showEditor = false
    @State private var showList = false
    @State private var firstRun = false
    @State private var notice: String?
    /// 抽屉侧边栏（电脑 / 服务器 / 关于）
    @State private var showSidebar = false
    @State private var showServerModule = false
    @State private var showAbout = false
    /// 「经由跳板机」的电脑：隧道解析出的本地加载地址（127.0.0.1:端口）
    @State private var tunnelUrls: [String: URL] = [:]
    /// TOFU 主机指纹确认（SSH 首连）：continuation 存状态里等按钮回填
    @State private var tofuAlert: TofuPrompt?

    var body: some View {
        ZStack {
            VStack(spacing: 0) {
                tabBar
                Divider()
                content
            }
            if showSidebar {
                sidebar
            }
        }
        .alert("同一台电脑", isPresented: Binding(
            get: { notice != nil },
            set: { if !$0 { notice = nil } })) {
            Button("好") { notice = nil }
        } message: {
            Text(notice ?? "")
        }
        .alert(tofuAlert?.title ?? "", isPresented: Binding(
            get: { tofuAlert != nil },
            set: { if !$0 { tofuAlert?.resume(false) } })) {
            Button("信任并连接") { tofuAlert?.resume(true) }
            Button("取消", role: .cancel) { tofuAlert?.resume(false) }
        } message: {
            Text(tofuAlert?.message ?? "")
        }
        .sheet(isPresented: $showServerModule) {
            ServerModuleView()
        }
        .sheet(isPresented: $showAbout) {
            AboutView()
        }
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button {
                    showSidebar = true
                } label: {
                    Label("侧边栏", systemImage: "line.3.horizontal")
                }
            }
            ToolbarItem(placement: .topBarLeading) {
                Button {
                    reloadActive()
                } label: {
                    Label("重载", systemImage: "arrow.clockwise")
                }
                .disabled(store.servers.isEmpty)
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("添加服务端") { startAdd() }
                    Button("编辑当前服务端") { startEdit(store.active) }
                        .disabled(store.active == nil)
                    Button("服务端列表") { showList = true }
                    Divider()
                    Button("重载当前页面") { reloadActive() }
                        .disabled(store.servers.isEmpty)
                    Button("全部重载") { reloadAll() }
                        .disabled(store.servers.isEmpty)
                    Divider()
                    Toggle("启动时连接全部服务端", isOn: Binding(
                        get: { store.connectAll },
                        set: { value in
                            store.setConnectAll(value)
                            if value { connectAll() }
                        }))
                    Menu {
                        ForEach(ServerStore.transports, id: \.self) { mode in
                            Button {
                                selectTransport(mode)
                            } label: {
                                if store.transport == mode {
                                    Label(transportTitle(mode), systemImage: "checkmark")
                                } else {
                                    Text(transportTitle(mode))
                                }
                            }
                        }
                    } label: {
                        Text("传输方式（\(transportLabel(store.transport))）")
                    }
                } label: {
                    Label("菜单", systemImage: "ellipsis.circle")
                }
            }
        }
        .sheet(isPresented: $showEditor) {
            ServerEditorSheet(profile: editing, firstRun: firstRun, knownGroups: store.knownGroups) { result in
                switch result {
                case .save(let profile):
                    save(profile)
                case .delete(let id):
                    store.remove(id)
                    forgetRuntimeState(id)
                    if store.servers.isEmpty {
                        startAdd(first: true)
                    }
                case .cancel:
                    if store.servers.isEmpty { startAdd(first: true) }
                }
            }
        }
        .sheet(isPresented: $showList) {
            ServerListSheet()
                .environmentObject(store)
        }
        .onAppear {
            // 心跳上线自愈：还停在错误面板上的那台，一探通就自动重连（不用手点重试）
            heartbeat.onWentOnline = { id in
                if statuses[id] == .failed {
                    if let profile = store.profile(id),
                       !profile.jumpServerId.trimmedAnyBoot.isEmpty {
                        resolveTunnel(profile)   // 走跳板机：隧道通了再重连
                    } else {
                        reloadFromFirstAddress(id)
                    }
                }
            }
            // 心跳走隧道（经由跳板机的电脑，探测也从 SSH 通道里发）
            heartbeat.tunnelResolver = { profile in
                try await SshSessionManager.shared.tunnelAddress(for: profile)
            }
            // TOFU 主机指纹确认（SSH 首连）：转成系统弹窗
            SshSessionManager.shared.confirmHandler = { title, message in
                await withCheckedContinuation { continuation in
                    tofuAlert = TofuPrompt(title: title, message: message,
                                           continuation: continuation)
                }
            }
            heartbeat.refreshNow()
            if store.servers.isEmpty {
                startAdd(first: true)
            } else if store.connectAll {
                connectAll()
            } else {
                reloadActive()
            }
        }
    }

    // MARK: - 侧边栏

    private var sidebar: some View {
        ZStack(alignment: .leading) {
            Color.black.opacity(0.33)
                .ignoresSafeArea()
                .onTapGesture { showSidebar = false }
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 12) {
                    Text("AB")
                        .font(.system(size: 17, weight: .bold))
                        .foregroundColor(.white)
                        .frame(width: 40, height: 40)
                        .background(Circle().fill(Color.white.opacity(0.2)))
                    Text("AnyBoot")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundColor(.white)
                }
                .padding(20)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.accentColor)

                sidebarItem("🖥  电脑", "回到电脑列表") { showSidebar = false }
                sidebarItem("🖧  服务器（\(JumpStore.shared.jumps.count)）",
                            "SSH / Telnet 跳板机管理") {
                    showSidebar = false
                    showServerModule = true
                }
                sidebarItem("ⓘ  关于", "项目信息与 Star") {
                    showSidebar = false
                    showAbout = true
                }
                Spacer()
                Text("外网连家里的电脑：先把家里能公网访问的机器加成 SSH 服务器，"
                     + "再在电脑的「经由跳板机」里选它。")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .padding(16)
            }
            .frame(width: 272)
            .background(Color(.systemBackground))
        }
    }

    private func sidebarItem(_ title: String, _ subtitle: String,
                             action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.body).foregroundColor(.primary)
                Text(subtitle).font(.caption2).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 20)
            .padding(.vertical, 14)
        }
    }

    // MARK: - 标签栏

    private var tabBar: some View {
        HStack(spacing: 4) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(store.servers) { profile in
                        chip(profile)
                    }
                    if store.servers.isEmpty {
                        Text("还没有服务端，点右边 ＋ 添加")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .padding(.vertical, 8)
                    }
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
            }
            Button { startAdd() } label: {
                Image(systemName: "plus").font(.system(size: 17, weight: .semibold))
            }
            .padding(.horizontal, 6)
            Button { showList = true } label: {
                Image(systemName: "list.bullet").font(.system(size: 17, weight: .semibold))
            }
            .padding(.horizontal, 8)
        }
        .background(Color(.secondarySystemBackground))
    }

    private func chip(_ profile: ServerProfile) -> some View {
        let active = profile.id == store.activeId
        let status = statuses[profile.id] ?? .unknown
        // 状态点：WebView 加载中黄点优先，其余由 10 秒心跳说了算
        //（在线绿 / 离线灰）—— 「下线了及时变、上线了也及时变」
        let dot = heartbeat.dotColor(profile.id, loading: status == .loading)
        let border = profile.group.trimmedAnyBoot.isEmpty
            ? Color(.systemGray4)
            : profile.groupColor
        return HStack(spacing: 6) {
            Circle().fill(dot).frame(width: 8, height: 8)
            Text(profile.label)
                .font(.system(size: 13))
                .lineLimit(1)
                .foregroundStyle(active ? Color.white : Color.primary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .background(
            RoundedRectangle(cornerRadius: 16)
                .fill(active ? Color.accentColor : Color(.tertiarySystemFill))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 16)
                .stroke(active ? Color.accentColor : border, lineWidth: 1)
        )
        .onTapGesture { store.select(profile.id) }
        .onLongPressGesture { startEdit(profile) }
    }

    // MARK: - 页面区

    @ViewBuilder
    private var content: some View {
        ZStack {
            ForEach(store.servers) { profile in
                // 每台电脑一个 WKWebView：多块网卡只是换 URL，不换实例；
                // 经由跳板机的电脑加载的是隧道地址（127.0.0.1:端口）
                if let url = loadURL(for: profile) {
                    WebView(url: url,
                            reloadToken: reloadTokens[profile.id] ?? 0,
                            onStatus: { status in handle(status, for: profile.id) })
                        .opacity(profile.id == store.activeId ? 1 : 0)
                        .allowsHitTesting(profile.id == store.activeId)
                } else if !profile.jumpServerId.trimmedAnyBoot.isEmpty {
                    tunnelConnecting(profile)
                }
            }
            if let message = errors[store.activeId], let profile = store.active {
                errorPanel(profile, message)
            }
        }
    }

    /// 电脑的加载地址：直连用候选地址；走跳板机用隧道解析出的本地地址
    private func loadURL(for profile: ServerProfile) -> URL? {
        if !profile.jumpServerId.trimmedAnyBoot.isEmpty {
            return tunnelUrls[profile.id]
        }
        return profile.url(at: addressIndex[profile.id] ?? 0, transport: store.transport)
    }

    private func tunnelConnecting(_ profile: ServerProfile) -> some View {
        VStack(spacing: 12) {
            ProgressView()
            Text("正在经由跳板机建立隧道 …")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// 走跳板机的电脑：解析（或复用）本地端口转发，然后照常加载 localhost
    private func resolveTunnel(_ profile: ServerProfile) {
        Task {
            do {
                let local = try await SshSessionManager.shared.tunnelAddress(for: profile)
                guard let url = profile.url(for: local, transport: store.transport) else { return }
                let fresh = tunnelUrls[profile.id] != url
                tunnelUrls[profile.id] = url
                if fresh || statuses[profile.id] == .failed {
                    reloadFromFirstAddress(profile.id)
                }
            } catch {
                errors[profile.id] = "经由跳板机连接失败：\(error.localizedDescription)"
                statuses[profile.id] = .failed
            }
        }
    }

    private func errorPanel(_ profile: ServerProfile, _ message: String) -> some View {
        VStack(spacing: 14) {
            Image(systemName: "wifi.exclamationmark").font(.system(size: 44))
            Text("连不上服务端").font(.title3.bold())
            Text(errorDetail(profile, message))
                .font(.footnote)
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
            HStack(spacing: 12) {
                Button("重试") { reloadActive() }
                    .buttonStyle(.borderedProminent)
                Button("修改这个服务端") { startEdit(profile) }
                    .buttonStyle(.bordered)
            }
        }
        .padding(28)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(.systemBackground))
    }

    /// 错误面板正文：地址摘要 + 失败原因（这台电脑有多个地址时全列出来）
    private func errorDetail(_ profile: ServerProfile, _ message: String) -> String {
        var text = "服务端：\(profile.label)\n地址：\(profile.addressSummary)\n\n\(message)"
        if profile.addresses.count > 1 {
            text += "\n\n这台电脑的 \(profile.addresses.count) 个地址都试过了：\n"
                + profile.addresses.map { "  \($0)" }.joined(separator: "\n")
        }
        return text
    }

    // MARK: - 动作

    /// 加载状态回调：连上就记住这个地址，不通就换下一个候选地址
    private func handle(_ status: LoadStatus, for id: String) {
        statuses[id] = status
        switch status {
        case .ok:
            rememberWorkingAddress(id)
        case .failed:
            advanceAddress(id)
        case .loading, .unknown:
            break
        }
    }

    /// 手工重载：从头再试一遍（首选地址优先；走跳板机的先解析隧道）
    private func reloadFromFirstAddress(_ id: String) {
        guard let profile = store.profile(id), !profile.addresses.isEmpty else { return }
        if !profile.jumpServerId.trimmedAnyBoot.isEmpty {
            statuses[id] = .loading
            errors[id] = nil
            resolveTunnel(profile)
            return
        }
        addressIndex[id] = 0
        triedAddresses[id] = []
        errors[id] = nil
        reload(id)
    }

    private func reload(_ id: String) {
        statuses[id] = .loading
        errors[id] = nil
        reloadTokens[id, default: 0] += 1
    }

    private func reloadActive() {
        guard let profile = store.active else { return }
        reloadFromFirstAddress(profile.id)
    }

    /// 全部重载 = 把没连上的也连上，并把在线的一起刷新（一律从首选地址重新试）
    private func reloadAll() {
        for profile in store.servers {
            reloadFromFirstAddress(profile.id)
        }
    }

    /// 「启动时连接全部」：让每台电脑各起一个 WebView，同时在线
    private func connectAll() {
        for profile in store.servers where reloadTokens[profile.id] == nil {
            reloadFromFirstAddress(profile.id)
        }
    }

    // MARK: - 传输方式

    /// 菜单上那一条「传输方式（…）」给人看的短说法
    private func transportLabel(_ mode: String) -> String {
        switch mode {
        case ServerStore.transportPush: return "推送"
        case ServerStore.transportPoll: return "轮询"
        default: return "自动"
        }
    }

    /// 选项行里的完整说法
    private func transportTitle(_ mode: String) -> String {
        switch mode {
        case ServerStore.transportPush: return "推送（WebSocket）"
        case ServerStore.transportPoll: return "轮询（HTTP 每 2s）"
        default: return "自动（推送优先，连不上退回轮询）"
        }
    }

    /// 选传输方式：只记偏好，**不立刻重载页面** —— 下一次加载才带上（菜单标题即时
    /// 反映当前选择）。当前页面上用户也能在横幅的下拉里即切即用，那是同一个偏好。
    private func selectTransport(_ mode: String) {
        store.setTransport(mode)
    }

    /// 这个地址不通就换下一个（同一台电脑的另一块网卡）。
    ///
    /// **换地址不换页面**：还是同一个标签、同一个 WKWebView，用户看到的始终是
    /// 「这台电脑」的界面。全试完了就什么都不做 —— 状态留在 `.failed`，错误面板会显示。
    private func advanceAddress(_ id: String) {
        guard let profile = store.profile(id) else { return }
        var tried = triedAddresses[id] ?? []
        let current = profile.host(at: addressIndex[id] ?? 0)
        if !current.isEmpty && !tried.contains(current) {
            tried.append(current)
        }
        triedAddresses[id] = tried
        guard let next = profile.addresses.indices.first(where: { !tried.contains(profile.host(at: $0)) })
        else {
            return
        }
        addressIndex[id] = next
        errors[id] = nil
        reload(id)
    }

    /// 连上了：把当前地址提到最前，下次开机直连它
    /// （多块网卡 / DHCP 换过 IP 都能自己找回来）。
    private func rememberWorkingAddress(_ id: String) {
        let index = addressIndex[id] ?? 0
        guard index != 0, let profile = store.profile(id) else { return }
        var updated = profile
        guard updated.promoteAddress(index) else { return }
        addressIndex[id] = 0
        store.updateInPlace(updated)   // 不重排：标签位置不许因为换网卡而跳
    }

    private func forgetRuntimeState(_ id: String) {
        statuses[id] = nil
        reloadTokens[id] = nil
        errors[id] = nil
        addressIndex[id] = nil
        triedAddresses[id] = nil
    }

    // MARK: - 保存 / 合并同一台电脑

    /// 保存一条记录：新增时如果这台电脑已经有记录（同一个分组名），把地址**并进去**
    /// —— 一台电脑只留一条记录、一份页面。
    private func save(_ profile: ServerProfile) {
        let isNew = store.profile(profile.id) == nil
        let group = profile.group.trimmedAnyBoot
        if isNew, !group.isEmpty,
           let existing = store.servers.first(where: {
               group.caseInsensitiveCompare($0.group.trimmedAnyBoot) == .orderedSame
           }) {
            var keep = existing
            keep.mergeAddresses(from: profile)
            if keep.token.trimmedAnyBoot.isEmpty { keep.token = profile.token }
            store.upsert(keep)
            store.select(keep.id)
            notice = "已并入同一台电脑：\(keep.label)"
            reloadFromFirstAddress(keep.id)
            probeMachine(keep)
            return
        }
        store.upsert(profile)
        store.select(profile.id)
        // 地址/token 可能变了，按新配置重连这台电脑（其它标签不受影响）
        reloadFromFirstAddress(profile.id)
        // 顺手探一次 /health：知道「这台电脑」的机器标识后，才认得出它是不是
        // 已经有另一块网卡的记录 —— 有就自动并成一条
        probeMachine(profile)
    }

    /// 后台探一次电脑端的 `/health`，拿到机器标识
    private func probeMachine(_ profile: ServerProfile) {
        HealthProbe.fetch(host: profile.host) { machine in
            guard let machine else { return }
            applyMachineIdentity(machine, to: profile.id)
        }
    }

    /// 探到了机器身份：记下来；如果这台电脑已经有别的记录，就把两条并成一条
    private func applyMachineIdentity(_ machine: HealthProbe.Machine, to id: String) {
        guard var target = store.profile(id) else { return }
        if !machine.id.isEmpty { target.machineId = machine.id }
        if !machine.name.isEmpty { target.machineName = machine.name }
        guard let sibling = store.servers.first(where: { target.sameMachine($0) }) else {
            store.updateInPlace(target)   // 只把机器标识记下来
            return
        }
        // 同一台电脑的两条记录 → 并成一条。正在看的那条留下，免得页面跳走。
        let keep: ServerProfile
        let drop: ServerProfile
        if store.activeId == sibling.id {
            keep = sibling
            drop = target
        } else {
            keep = target
            drop = sibling
        }
        var merged = keep
        if merged.machineName.trimmedAnyBoot.isEmpty { merged.machineName = drop.machineName }
        merged.mergeAddresses(from: drop)
        if merged.machineId.trimmedAnyBoot.isEmpty { merged.machineId = drop.machineId }
        if merged.group.trimmedAnyBoot.isEmpty { merged.group = drop.group }
        if merged.name.trimmedAnyBoot.isEmpty { merged.name = drop.name }
        if merged.token.trimmedAnyBoot.isEmpty { merged.token = drop.token }
        let keepWasActive = store.activeId == merged.id
        store.upsert(merged)
        store.remove(drop.id)
        forgetRuntimeState(drop.id)
        if keepWasActive { store.select(merged.id) }
        notice = "已把同一台电脑的地址合并到一个标签：\(merged.label)"
            + (machine.name.isEmpty ? "" : "（\(machine.name)）")
    }

    private func startAdd(first: Bool = false) {
        editing = ServerProfile()
        firstRun = first
        showEditor = true
    }

    private func startEdit(_ profile: ServerProfile?) {
        guard let profile else { return }
        editing = profile
        firstRun = false
        showEditor = true
    }
}

/// 新增 / 编辑一个服务端
struct ServerEditorSheet: View {

    enum Result {
        case save(ServerProfile)
        case delete(String)
        case cancel
    }

    let profile: ServerProfile?
    let firstRun: Bool
    let knownGroups: [String]
    let onFinish: (Result) -> Void

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var jumps = JumpStore.shared

    @State private var group = ""
    @State private var name = ""
    @State private var host = ""
    @State private var token = ""
    /// 多网卡探测模式（一台电脑多个地址时怎么探）：并发（默认）/ 串行
    @State private var probeMode = ServerProfile.probeParallel
    /// 经由哪台跳板机（空 = 直连）
    @State private var jumpServerId = ""
    @State private var showDeleteConfirm = false

    private var editing: Bool { profile.map { ServerStore.shared.profile($0.id) != nil } ?? false }

    var body: some View {
        NavigationStack {
            Form {
                Section("分组（用来区分不同的电脑）") {
                    TextField("例如：书房台式机", text: $group)
                        .autocorrectionDisabled()
                    if !knownGroups.isEmpty {
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack(spacing: 8) {
                                ForEach(knownGroups, id: \.self) { item in
                                    Button(item) { group = item }
                                        .font(.footnote)
                                        .buttonStyle(.bordered)
                                }
                            }
                        }
                    }
                }
                Section("服务端名（同一台电脑的不同网卡/地址）") {
                    TextField("例如：有线网卡 / 5G WiFi", text: $name)
                        .autocorrectionDisabled()
                }
                Section("服务端地址") {
                    // 一台电脑的多个地址一次填完：多行输入（每行一个）
                    TextField("192.168.0.120:12300 或整条带 token 的地址", text: $host, axis: .vertical)
                        .lineLimit(1...4)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    // 新增说明（原有文案一字未动）：多行 = 同一台电脑的多个网卡地址
                    Text("同一台电脑有多块网卡就填多行，每行一个 —— 只占一个标签，哪个通就用哪个")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                Section("访问令牌 token") {
                    TextField("可留空，页面上也能填", text: $token)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section("经由跳板机（外网连家里）") {
                    Picker("跳板机", selection: $jumpServerId) {
                        Text("直连（默认）").tag("")
                        ForEach(jumps.jumps.filter { $0.isSsh }) { jump in
                            Text("经由：\(jump.displayName)").tag(jump.id)
                        }
                    }
                    Text("先把家里能公网访问的机器加成 SSH 服务器（侧边栏 → 服务器）。"
                         + "选定后，上面填的地址就变成「跳板机视角的内网地址」，"
                         + "页面和所有请求都经 SSH 加密通道 → 跳板机 → 内网。")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                Section("多网卡探测（有多个地址时怎么探）") {
                    Picker("探测模式", selection: $probeMode) {
                        Text("并发探测（推荐）").tag(ServerProfile.probeParallel)
                        Text("串行探测").tag(ServerProfile.probeSerial)
                    }
                    .pickerStyle(.segmented)
                    Text(probeMode == ServerProfile.probeSerial
                         ? "按地址顺序逐个探，第一个通的用之 —— 适合有明确优先级（比如优先走内网 IP）"
                         : "同时探所有地址，用延迟最低的那个；断线直接切到已探活的地址，不用等超时")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                Section {
                    Text("在电脑端 AnyBoot 的「远程控制」里能看到地址和 token；手机要和电脑在同一个局域网。同一台电脑的多个 IP 填同一个分组，只占一个标签（首选地址不通会自动换下一个）。")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                if editing {
                    Section {
                        Button("删除这个服务端", role: .destructive) { showDeleteConfirm = true }
                    }
                }
            }
            .navigationTitle(editing ? "编辑服务端" : (firstRun ? "连接 AnyBoot 服务端" : "添加服务端"))
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("保存") { save() }
                        .disabled(ServerProfile.parseAddresses(host).isEmpty)
                }
                if !firstRun {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("取消") {
                            onFinish(.cancel)
                            dismiss()
                        }
                    }
                }
            }
            .confirmationDialog("确定删除这个服务端吗？", isPresented: $showDeleteConfirm, titleVisibility: .visible) {
                Button("删除", role: .destructive) {
                    if let id = profile?.id {
                        onFinish(.delete(id))
                    }
                    dismiss()
                }
                Button("取消", role: .cancel) {}
            }
            .onAppear {
                group = profile?.group ?? ""
                name = profile?.name ?? ""
                host = profile?.addressText ?? ""
                token = profile?.token ?? ""
                probeMode = ServerProfile.cleanProbeMode(profile?.probeMode)
                jumpServerId = profile?.jumpServerId ?? ""
            }
        }
        .interactiveDismissDisabled(firstRun)
    }

    private func save() {
        let addresses = ServerProfile.parseAddresses(host)
        guard !addresses.isEmpty else { return }
        var resolved = token.trimmedAnyBoot
        if resolved.isEmpty { resolved = ServerProfile.token(fromURL: host) }
        var result = profile ?? ServerProfile()
        result.group = group.trimmedAnyBoot
        result.name = name.trimmedAnyBoot.isEmpty
            ? (addresses.first ?? "")
            : name.trimmedAnyBoot
        result.replaceAddresses(addresses)
        result.token = resolved
        result.probeMode = probeMode
        result.jumpServerId = jumpServerId
        onFinish(.save(result))
        dismiss()
    }
}

/// 服务端列表：切换 / 编辑 / 删除 / 合并同一台电脑的多条记录
struct ServerListSheet: View {

    @EnvironmentObject private var store: ServerStore
    @Environment(\.dismiss) private var dismiss

    @State private var editing: ServerProfile?
    @State private var showEditor = false
    @State private var mergeNotice: String?

    var body: some View {
        NavigationStack {
            List {
                ForEach(store.servers) { profile in
                    HStack(spacing: 10) {
                        Circle()
                            .fill(profile.group.trimmedAnyBoot.isEmpty ? Color(.systemGray3) : profile.groupColor)
                            .frame(width: 10, height: 10)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(profile.label).font(.body)
                            Text(profile.addressSummary).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if profile.id == store.activeId {
                            Image(systemName: "checkmark").foregroundStyle(Color.accentColor)
                        }
                    }
                    .contentShape(Rectangle())
                    .onTapGesture {
                        store.select(profile.id)
                        dismiss()
                    }
                    .swipeActions {
                        Button("删除", role: .destructive) {
                            store.remove(profile.id)
                        }
                        Button("编辑") {
                            editing = profile
                            showEditor = true
                        }
                    }
                }
            }
            .navigationTitle("服务端列表")
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("合并同机地址") {
                        let removed = store.mergeSameMachine()
                        mergeNotice = removed > 0
                            ? "已合并 \(removed) 条重复记录 —— 同一台电脑只留一个标签"
                            : "没有需要合并的：同一台电脑要么已经只有一条，要么分组名不同"
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成") { dismiss() }
                }
            }
            .alert("合并同一台电脑的地址", isPresented: Binding(
                get: { mergeNotice != nil },
                set: { if !$0 { mergeNotice = nil } })) {
                Button("好") { mergeNotice = nil }
            } message: {
                Text(mergeNotice ?? "")
            }
            .sheet(isPresented: $showEditor) {
                ServerEditorSheet(profile: editing,
                                  firstRun: false,
                                  knownGroups: store.knownGroups) { result in
                    if case .save(let updated) = result { store.upsert(updated) }
                    if case .delete(let id) = result { store.remove(id) }
                }
            }
        }
    }
}

#Preview {
    ContentView()
}

/// TOFU 主机指纹确认（SSH 首连）：continuation 挂在 State 里等按钮回填
struct TofuPrompt: Identifiable {
    let id = UUID()
    let title: String
    let message: String
    let continuation: CheckedContinuation<Bool, Never>

    func resume(_ accepted: Bool) {
        continuation.resume(returning: accepted)
    }
}
