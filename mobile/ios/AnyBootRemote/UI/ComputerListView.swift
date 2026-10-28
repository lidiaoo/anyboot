import SwiftUI

/// **电脑列表 = App 首页**（连接策略机制的展示主题）。
///
/// 一比一移植 `ComputerListActivity.kt`（3286 行）的 HostCard 全部信息层：
/// 选中态 / OS 徽标 / 状态七态 / 多地址 + 每块网卡的探测与 MAC / 远程配对逐条平铺 /
/// 两个引导项（rEFInd · GRUB）/ 待生效条 / 电源四键（带两行 hint）/ ⋮ 菜单 /
/// 顶栏（☰ + 标题副标题 + 刷新 + 帮助 + ⋮）/ 搜索 / 四段统计筛选条。
///
/// 布局全部相对约束：卡片列表是 `ABGrid`（iPhone 1 列 / iPad 2 列），卡片内的
/// 行用 `ABFlow` 自动折行，没有一处写死宽度。
struct ComputerListView: View {

    // MARK: 常量（照抄 Android）

    private enum Reach {
        static let online = "online"
        static let offline = "offline"
        static let waking = "waking"
        static let shutting = "shutting"
        static let rebooting = "rebooting"
        static let sleeping = "sleeping"
        static let unknown = "unknown"
    }

    private enum Filter: String {
        case all, online, offline, pending
    }

    private enum Kind {
        static let refind = BootKind.refind
        static let grub = BootKind.grub
        static let help = "help"
        /// 三引导器通道弹层（sdboot / opencore / clover），具体通道在 SheetRequest.channel 里
        static let channel = "channel"
    }

    @ObservedObject private var store = ServerStore.shared
    @ObservedObject private var nav = ABNavigator.shared
    @ObservedObject private var theme = ABThemeStore.shared

    // 数据
    @State private var boots: [String: BootInfo] = [:]
    @State private var busy: [String: String] = [:]
    @State private var transients: [String: String] = [:]

    // 界面
    @State private var query = ""
    @State private var filter: Filter = .all
    @State private var selectedId = ""
    @State private var sheet: SheetRequest?
    @State private var editorTarget: ComputerEditorTarget?
    @State private var deleteTarget: ServerProfile?
    /// ⋮ →「电脑列表」弹的责任端清单（对齐 Android `MoreServerListDialog`）
    @State private var serverListOpen = false
    @State private var refreshing = false
    @State private var toasts: [ToastItem] = []
    @State private var probeTask: Task<Void, Never>?

    private struct SheetRequest: Identifiable {
        let id = UUID()
        let rid: String
        let kind: String
        /// kind == Kind.channel 时携带的通道
        var channel: ChannelGroup? = nil
    }

    private struct ToastItem: Identifiable {
        let id = UUID()
        let text: String
    }

    // MARK: - Body

    var body: some View {
        VStack(spacing: 0) {
            topBar
            content
        }
        .abPage()
        .overlay(alignment: .bottom) { toastStack }
        .overlay { pickSheetOverlay }
        .overlay { deleteDialog }
        .overlay { serverListDialog }
        .overlay { failReloadDialog }
        .overlay { transportDialog }
        .sheet(item: $editorTarget) { target in
            ComputerEditorView(profile: target.profile) { result in
                switch result {
                case .save(let profile):
                    store.upsert(profile)
                    reloadBoot(profile)
                case .delete(let id):
                    doDelete(id)
                case .cancel:
                    break
                }
            }
            .id(target.id)   // 每次呈现都是新身份 → init 的回显一定生效
        }
        .onAppear {
            if selectedId.isEmpty { selectedId = store.activeId }
            refreshAll(silent: boots.isEmpty ? false : true)
            startAutoProbe()
        }
        .onDisappear { probeTask?.cancel() }
        .onChange(of: store.servers.count) { _, _ in
            if boots.isEmpty { refreshAll() }
        }
    }

    // MARK: - 顶栏（TopBar 329-405）

    private var topBar: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                barIcon("侧边栏") {
                    DrawnIcon.menu(ABColor.fg, size: ABMetric.px(20))
                } action: {
                    nav.openDrawer()
                }
                .padding(.leading, ABMetric.px(2))

                VStack(alignment: .leading, spacing: 0) {
                    Text("电脑列表")
                        .font(.system(size: 16.5, weight: .bold))
                        .foregroundColor(ABColor.fg)
                        .lineLimit(1)
                    Text("远程管理 - 电脑 · 点卡片即可发电源指令或切引导项")
                        .font(.system(size: 11))
                        .foregroundColor(ABColor.fgFaint)
                        .lineLimit(1)
                }
                .padding(.leading, ABMetric.px(6))
                .frame(maxWidth: .infinity, alignment: .leading)

                if refreshing {
                    ProgressView()
                        .scaleEffect(0.7)
                        .frame(width: ABMetric.px(38), height: ABMetric.px(38))
                } else {
                    barIcon("刷新") {
                        DrawnIcon.refresh(ABColor.fgDim, size: ABMetric.px(19))
                    } action: {
                        refreshAll()
                    }
                }
                barIcon("操作说明") {
                    DrawnIcon.help(ABColor.fgDim, size: ABMetric.px(19))
                } action: {
                    sheet = SheetRequest(rid: "", kind: Kind.help)
                }
                overflowMenu
            }
            .frame(height: ABMetric.px(54))
            .padding(.leading, ABMetric.px(6))
            .padding(.trailing, ABMetric.px(8))

            VStack(spacing: 0) {
                searchBox
                statChips
            }
            .padding(.leading, ABMetric.px(12))
            .padding(.trailing, ABMetric.px(12))
            .padding(.bottom, ABMetric.px(10))

            Rectangle().fill(ABColor.line).frame(height: 1)
        }
        .background(ABColor.card)
    }

    /// 设计稿的 38×38 方角按钮：透明底，图标居中
    private func barIcon<Content: View>(_ description: String,
                                        @ViewBuilder content: () -> Content,
                                        action: @escaping () -> Void) -> some View {
        Button(action: action) {
            content()
                .frame(width: ABMetric.px(38), height: ABMetric.px(38))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(description)
    }

    private var overflowMenu: some View {
        Menu {
            Button("添加电脑") { openAdd() }
            Button("编辑当前电脑") {
                if let profile = store.profile(store.activeId) {
                    openEdit(profile)
                } else {
                    toast("电脑页还没有当前电脑 —— 卡片右侧 ⋮ 也能直接编辑")
                }
            }
            Button("电脑列表") { serverListOpen = true }
            Button("重载当前电脑") {
                let id = selectedId.isEmpty ? store.activeId : selectedId
                if let profile = store.profile(id) {
                    reloadBoot(profile)
                } else {
                    toast("先点一下卡片选中要重载的电脑")
                }
            }
            Button("全部重载") { refreshAll() }
            Button(store.connectAll ? "✓ 启动时连接全部服务端" : "启动时连接全部服务端") {
                let next = !store.connectAll
                store.setConnectAll(next)
                toast(next ? "已开启：启动时把所有服务端一起连上" : "已关闭：只连当前标签")
            }
            Button(failReloadSec > 0
                   ? "连不上时自动重载（每 \(failReloadSec) 秒）"
                   : "连不上时自动重载（已关闭）") {
                failReloadOpen = true
            }
            Button("传输方式（\(transportLabel)）") {
                transportOpen = true
            }
        } label: {
            Text("⋮")
                .font(.system(size: 19))
                .foregroundColor(ABColor.fgDim)
                .frame(width: ABMetric.px(38), height: ABMetric.px(38))
                .contentShape(Rectangle())
        }
    }

    // MARK: - 搜索框（SearchBox 684-740）

    private var searchBox: some View {
        HStack(spacing: ABMetric.px(8)) {
            DrawnIcon.search(ABColor.fgFaint, size: ABMetric.px(15))
            ZStack(alignment: .leading) {
                if query.isEmpty {
                    Text("搜索电脑名称、IP 或 MAC")
                        .font(.system(size: 13))
                        .foregroundColor(ABColor.fgFaint)
                        .lineLimit(1)
                }
                TextField("", text: $query)
                    .font(.system(size: 13))
                    .foregroundColor(ABColor.fg)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if !query.isEmpty {
                Button {
                    query = ""
                } label: {
                    DrawnIcon.close(ABColor.fgFaint, size: ABMetric.px(13))
                        .frame(width: ABMetric.px(22), height: ABMetric.px(22))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.leading, ABMetric.px(11))
        .padding(.trailing, ABMetric.px(8))
        .frame(height: ABMetric.px(38))
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                .fill(ABColor.elevate))
        .overlay(
            RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                .strokeBorder(ABColor.line, lineWidth: 1))
    }

    // MARK: - 统计筛选条（384-401）

    private var statChips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: ABMetric.px(7)) {
                let rows = allRows
                let online = rows.filter { $0.state == Reach.online }.count
                let pending = rows.filter { grubPending($0.grub) }.count
                statChip("全部", rows.count, filter == .all, false) { filter = .all }
                statChip("在线", online, filter == .online, false) { filter = .online }
                statChip("离线", rows.count - online, filter == .offline, false) { filter = .offline }
                statChip("待生效", pending, filter == .pending, true) { filter = .pending }
            }
            .padding(.top, ABMetric.px(9))
        }
    }

    private func statChip(_ label: String, _ count: Int,
                          _ selected: Bool, _ tone: Bool,
                          _ action: @escaping () -> Void) -> some View {
        let accent = tone ? ABColor.warn : ABColor.accent
        return Button(action: action) {
            VStack(alignment: .leading, spacing: ABMetric.px(1)) {
                Text("\(count)")
                    .font(.system(size: 15, weight: .semibold, design: .monospaced))
                    .foregroundColor(selected ? ABColor.onAccent : (tone ? ABColor.warn : ABColor.fg))
                    .lineLimit(1)
                Text(label)
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundColor(selected ? ABColor.onAccent.opacity(0.78) : ABColor.fgFaint)
                    .lineLimit(1)
            }
            .padding(.horizontal, ABMetric.px(13))
            .frame(height: ABMetric.px(44))
            .background(
                RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                    .fill(selected ? accent : ABColor.card))
            .overlay(
                RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                    .strokeBorder(selected ? accent : ABColor.line, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }

    // MARK: - 内容

    @ViewBuilder
    private var content: some View {
        if store.servers.isEmpty {
            emptyState(noServers: true)
        } else if visibleRows.isEmpty {
            // 有电脑但被搜索 / 筛选滤空（对齐 Android EmptyState(false)）
            emptyState(noServers: false)
        } else {
            ScrollView {
                ABGrid(visibleRows, minItemWidth: ABMetric.px(330), spacing: ABMetric.px(10)) { row in
                    card(row)
                }
                .padding(.horizontal, ABMetric.px(12))
                .padding(.top, ABMetric.px(12))
                .padding(.bottom, ABMetric.px(24))
                // 列表末尾说明（HostList 799-810）
                Text("电源指令通过魔术包发送，无需目标机在线\n引导项变更在下次重启时生效")
                    .font(.system(size: 11))
                    .lineSpacing(ABMetric.px(7))
                    .foregroundColor(ABColor.fgFaint)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
                    .padding(.horizontal, ABMetric.px(16))
                    .padding(.top, ABMetric.px(4))
            }
            .refreshable { refreshAll() }
        }
    }

    /// 空态（EmptyState 814-836）：DesktopIcon 26 + 居中两行文案，无按钮
    private func emptyState(noServers: Bool) -> some View {
        VStack(spacing: 0) {
            DrawnIcon.desktop(ABColor.fgFaint.opacity(0.5), size: ABMetric.px(26))
            Spacer().frame(height: ABMetric.px(8))
            Text(noServers
                 ? "还没有添加任何电脑\n到「服务器」里添加一台，这里就会出现它的电源与引导项"
                 : "没有符合条件的电脑")
                .font(.system(size: 13))
                .lineSpacing(ABMetric.px(7))
                .foregroundColor(ABColor.fgDim)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.horizontal, ABMetric.px(32))
    }

    // MARK: - 行模型（rowsOf 2114-2239）

    private struct PairRowView: Identifiable {
        let id: String
        let channel: String
        let addr: String
        let url: String
        let lane: String
        let source: String
        let state: String
        let ok: Bool
        let enabled: Bool
    }

    private struct CqRow: Identifiable {
        var id: String { profile.id }
        let profile: ServerProfile
        let name: String
        let pairs: [PairRowView]
        let tunnelNote: String
        let ip: String
        let hits: [ProbeEngine.Hit]
        let refind: BootGroup
        let grub: BootGroup
        let channels: [ChannelGroup]
        let state: String
        let conn: String
        let connRemote: Bool
    }

    private var allRows: [CqRow] { store.servers.map(rowOf) }

    private var visibleRows: [CqRow] {
        allRows.filter { row in
            let byState: Bool
            switch filter {
            case .all: byState = true
            case .online: byState = row.state == Reach.online
            case .offline: byState = row.state != Reach.online
            case .pending: byState = grubPending(row.grub)
            }
            if !byState { return false }
            let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
            if needle.isEmpty { return true }
            var extras = ""
            for index in 0..<row.profile.addressCount {
                let addr = row.profile.hostAt(index)
                extras += " " + addr
                for mac in row.profile.macsForHost(addr) { extras += " " + mac }
            }
            let haystack = ([row.name, row.ip]
                + row.pairs.map { $0.addr + " " + $0.channel + " " + $0.source })
                .joined(separator: " ") + extras
            return haystack.lowercased().contains(needle)
        }
    }

    private func rowOf(_ profile: ServerProfile) -> CqRow {
        let info = boots[profile.id]
        let state: String
        if let transient = transients[profile.id] {
            state = transient
        } else if info == nil {
            state = Reach.unknown
        } else if info?.online != true {
            state = Reach.offline
        } else {
            state = Reach.online
        }

        // 当前连接：探测赢家确实落在某条配对上 → 按**那一条**标通道
        let hitPair: RemotePair? = {
            guard let info, info.online else { return nil }
            return profile.flatPairs.first { pair in
                let origin = ServerProfile.normalizeRemoteOrigin(pair.addr)
                guard !origin.isEmpty else { return false }
                var normalized = origin
                if normalized.hasPrefix("https://") { normalized.removeFirst(8) }
                else if normalized.hasPrefix("http://") { normalized.removeFirst(7) }
                return normalized == info.addr
            }
        }()
        let usingRemote = hitPair != nil
        let jumpId = profile.jumpServerId.trimmingCharacters(in: .whitespaces)

        // **没在线的不能写「直连」**：只有这一轮确实探通才描述走的哪条通道
        let conn: String
        if state != Reach.online {
            conn = "未连接"
        } else if info?.winnerKind == ServerProfile.routeLan {
            conn = "直连"
        } else if info?.winnerKind == ServerProfile.routeJump {
            if let jump = JumpStore.shared.jump(jumpId) {
                let name = jump.name.trimmingCharacters(in: .whitespaces)
                conn = "跳板机 · " + (name.isEmpty ? jump.host : name)
            } else {
                conn = "跳板机 · 已删除"
            }
        } else if hitPair?.routeKind == ServerProfile.routeTSNet {
            conn = "内置 Funnel"
        } else if hitPair?.routeKind == ServerProfile.routeFunnel {
            conn = "TS Funnel"
        } else if hitPair?.routeKind == ServerProfile.routeTS {
            conn = "TS 直连"
        } else if hitPair != nil {
            conn = "远程隧道"
        } else {
            conn = "直连"
        }

        let hits = info?.hits ?? []
        var rows: [PairRowView] = []

        // 跳板机通道行：配了且启用才亮出来（用户 2026-10-28：没开启的不上列表页）
        if !jumpId.isEmpty {
            let jump = JumpStore.shared.jump(jumpId)
            let jumpHit = hits.first { $0.kind == ServerProfile.routeJump }
            let jumpEnabled = !profile.routeDisabled.contains(ServerProfile.routeJump)
            if jumpEnabled {
                let jumpInUse = state == Reach.online && info?.winnerKind == ServerProfile.routeJump
                let display: String
                if let jump {
                    let name = jump.name.trimmingCharacters(in: .whitespaces)
                    display = name.isEmpty ? jump.host : name
                } else {
                    display = "已删除的跳板机"
                }
                rows.append(PairRowView(
                    id: "jump",
                    channel: "跳板机",
                    addr: display,
                    url: "",   // 转发端点只在本机会话有效，浏览器开不了
                    lane: "",
                    source: "",
                    state: jumpInUse && jumpHit?.ok == true ? "使用中 \(jumpHit?.ms ?? 0)ms"
                        : (jumpInUse ? "使用中" : probeStateText(jumpHit)),
                    ok: jumpHit?.ok == true,
                    enabled: true))
            }
        }

        for block in profile.pairBlockOrder() {
            // 已停用的配对不上列表页（用户 2026-10-28：连接策略没开启的就不展示）
            for pair in profile.pairsOfBlock(block, enabledOnly: true) {
                let hit = hits.first { !$0.pairId.isEmpty && $0.pairId == pair.id }
                let inUse = pair.id == hitPair?.id
                let stateText: String
                if inUse && hit?.ok == true {
                    stateText = "使用中 \(hit?.ms ?? 0)ms"
                } else if inUse {
                    stateText = "使用中"
                } else {
                    stateText = probeStateText(hit)
                }
                let origin = ServerProfile.normalizeRemoteOrigin(pair.addr)
                let secret = pair.token.trimmingCharacters(in: .whitespaces)
                var openUrl = ""
                if !origin.isEmpty {
                    openUrl = secret.isEmpty
                        ? origin
                        : origin + "/?ab_token=" + (secret.addingPercentEncoding(
                            withAllowedCharacters: .alphanumerics) ?? secret)
                }
                var addrText = origin
                if addrText.hasPrefix("https://") { addrText.removeFirst(8) }
                else if addrText.hasPrefix("http://") { addrText.removeFirst(7) }
                rows.append(PairRowView(
                    id: pair.id,
                    channel: ServerProfile.routeLabel(pair.routeKind),
                    addr: addrText,
                    url: openUrl,
                    lane: pair.routeKind == ServerProfile.routeFunnel ? "Funnel" : "",
                    source: pair.sourceLabel,
                    state: stateText,
                    ok: hit?.ok == true,
                    enabled: true))
            }
        }

        return CqRow(
            profile: profile,
            name: profile.displayName.isEmpty ? "(未命名)" : profile.displayName,
            pairs: rows,
            tunnelNote: tunnelShareNote(profile),
            ip: displayAddress(profile),
            hits: hits,
            refind: info?.refind ?? emptyGroup(Kind.refind),
            grub: info?.grub ?? emptyGroup(Kind.grub),
            channels: info?.channels ?? [],
            state: state,
            conn: conn,
            connRemote: usingRemote)
    }

    private func emptyGroup(_ kind: String) -> BootGroup {
        BootGroup(kind: kind, title: kind == Kind.refind ? "rEFInd" : "GRUB",
                  available: false, current: "", next: "", saved: "", note: "", options: [])
    }

    /// 「待生效」的唯一判据：next 与 saved 都有值且不同
    private func grubPending(_ group: BootGroup) -> Bool {
        group.available && !group.next.isEmpty && !group.saved.isEmpty && group.next != group.saved
    }

    private func probeStateText(_ hit: ProbeEngine.Hit?) -> String {
        guard let hit else { return "未探测" }
        if hit.ok { return "通 \(hit.ms)ms" }
        if hit.error.contains("未测出") || hit.error.contains("未探测") { return "未测出" }
        return "不通"
    }

    /// 「同一隧道被多套系统共用」提示（docs/remote-pairs.md §6 S5）
    private func tunnelShareNote(_ profile: ServerProfile) -> String {
        var seen: [String: Int] = [:]
        for pair in profile.flatPairs {
            let id = pair.tunnelId.trimmingCharacters(in: .whitespaces)
            if id.isEmpty { continue }
            seen[id, default: 0] += 1
        }
        for (id, count) in seen where count > 1 {
            let short = id.count > 8 ? String(id.prefix(8)) : id
            return "隧道 \(short) 被 \(count) 条配对共用 —— 同一隧道被多套系统共用，请求会随机落到其中一个"
        }
        return ""
    }

    private func displayAddress(_ profile: ServerProfile) -> String {
        guard let value = ServerProfile.normalizeHost(profile.host) else { return "(未填地址)" }
        return stripDefaultPort(value)
    }

    private func displayAddrOf(_ raw: String) -> String {
        guard let value = ServerProfile.normalizeHost(raw) else { return raw }
        return stripDefaultPort(value)
    }

    private func stripDefaultPort(_ value: String) -> String {
        let suffix = ":\(ServerProfile.defaultPort)"
        return value.hasSuffix(suffix) ? String(value.dropLast(suffix.count)) : value
    }

    // MARK: - 卡片（HostCard 839-1198）

    private func card(_ row: CqRow) -> some View {
        let selected = row.profile.id == selectedId
        let stripe = stateColor(row.state)
        let shape = RoundedRectangle(cornerRadius: ABMetric.px(15), style: .continuous)

        return VStack(alignment: .leading, spacing: 0) {
            header(row, selected: selected)
            addressLines(row)
            pairsBlock(row)
            bootButtons(row)
            if grubPending(row.grub) { pendingBar(row) }
            dashedDivider
            powerButtons(row)
        }
        .padding(.leading, ABMetric.px(13))
        .padding(.trailing, ABMetric.px(13))
        .padding(.top, ABMetric.px(12))
        .padding(.bottom, ABMetric.px(13))
        .background(shape.fill(selected ? ABColor.accent.opacity(0.07) : ABColor.card))
        .overlay(alignment: .leading) {
            // 左侧状态竖条：上下各让 14
            GeometryReader { geo in
                let inset = ABMetric.px(14)
                let height = geo.size.height - inset * 2
                if height > 0 {
                    UnevenRoundedRectangle(
                        topLeadingRadius: 0, bottomLeadingRadius: ABMetric.px(3),
                        bottomTrailingRadius: 0, topTrailingRadius: ABMetric.px(3),
                        style: .continuous)
                        .fill(stripe)
                        .frame(width: ABMetric.px(3), height: height)
                        .offset(y: inset)
                }
            }
        }
        .clipShape(shape)
        .overlay(shape.strokeBorder(selected ? ABColor.accent : ABColor.line,
                                    lineWidth: selected ? 1.5 : 1))
        .contentShape(Rectangle())
        .onTapGesture { selectComputer(row.profile.id) }
    }

    private func header(_ row: CqRow, selected: Bool) -> some View {
        HStack(spacing: 0) {
            Text(row.name)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(ABColor.fg)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)

            if selected {
                Text("当前")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundColor(ABColor.accent)
                    .lineLimit(1)
                    .padding(.horizontal, ABMetric.px(5))
                    .padding(.vertical, ABMetric.px(1))
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(5), style: .continuous)
                            .fill(ABColor.accent.opacity(0.16)))
                    .overlay(
                        RoundedRectangle(cornerRadius: ABMetric.px(5), style: .continuous)
                            .strokeBorder(ABColor.accent.opacity(0.3), lineWidth: 1))
                    .padding(.leading, ABMetric.px(7))
            }

            if !ABFormat.osShowName(row.profile.machineOs).isEmpty {
                ABOsBadge(os: row.profile.machineOs) {
                    SystemInfoCenter.shared.show(row.profile)
                }
                .padding(.leading, ABMetric.px(7))
            }

            Text(row.conn)
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(row.connRemote ? ABColor.accent : ABColor.fgFaint)
                .lineLimit(1)
                .truncationMode(.tail)
                .padding(.horizontal, ABMetric.px(5))
                .padding(.vertical, ABMetric.px(1))
                .background(
                    RoundedRectangle(cornerRadius: ABMetric.px(5), style: .continuous)
                        .fill((row.connRemote ? ABColor.accent : ABColor.fgFaint).opacity(0.12)))
                .overlay(
                    RoundedRectangle(cornerRadius: ABMetric.px(5), style: .continuous)
                        .strokeBorder((row.connRemote ? ABColor.accent : ABColor.fgFaint).opacity(0.28),
                                      lineWidth: 1))
                .frame(maxWidth: ABMetric.px(110), alignment: .trailing)
                .padding(.leading, ABMetric.px(7))

            cardMenu(row)
        }
        .padding(.leading, ABMetric.px(4))
    }

    /// 状态 + 全部配置的地址（第一行跟着状态，其余地址逐行补）
    private func addressLines(_ row: CqRow) -> some View {
        let count = row.profile.addressCount
        return VStack(alignment: .leading, spacing: ABMetric.px(3)) {
            HStack(spacing: 0) {
                statusLine(row.state)
                if count > 0 {
                    let addr = row.profile.hostAt(0)
                    dot()
                    Text(displayAddrOf(addr))
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundColor(ABColor.fgFaint)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    lanHitBadge(row, addr)
                    ForEach(row.profile.macsForHost(addr), id: \.self) { mac in
                        dot()
                        Text(mac.uppercased())
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundColor(ABColor.fgFaint)
                            .lineLimit(1)
                    }
                } else {
                    dot()
                    Text("(未配地址)")
                        .font(.system(size: 11))
                        .foregroundColor(ABColor.fgFaint)
                }
                Spacer(minLength: 0)
            }
            .padding(.leading, ABMetric.px(4))

            ForEach(1..<max(1, count), id: \.self) { index in
                if index < count {
                    let addr = row.profile.hostAt(index)
                    HStack(spacing: 0) {
                        Text(displayAddrOf(addr))
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundColor(ABColor.fgFaint)
                            .lineLimit(1)
                            .truncationMode(.tail)
                        lanHitBadge(row, addr)
                        ForEach(row.profile.macsForHost(addr), id: \.self) { mac in
                            dot()
                            Text(mac.uppercased())
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundColor(ABColor.fgFaint)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.leading, ABMetric.px(4))
                }
            }
        }
        .padding(.top, ABMetric.px(5))
    }

    /// 地址行里的「·」分隔点
    private func dot() -> some View {
        Text("·")
            .font(.system(size: 11))
            .foregroundColor(ABColor.fgFaint)
            .padding(.horizontal, ABMetric.px(6))
    }

    /// 状态行（StatusLine 1200-1220）
    private func statusLine(_ state: String) -> some View {
        let color: Color
        let label: String
        switch state {
        case Reach.online: color = ABColor.toneOnline; label = "在线"
        case Reach.offline: color = ABColor.toneOfflineText; label = "离线"
        case Reach.waking: color = ABColor.toneBusy; label = "唤醒中"
        case Reach.shutting: color = ABColor.toneBusy; label = "关机中"
        case Reach.rebooting: color = ABColor.toneBusy; label = "重启中"
        case Reach.sleeping: color = ABColor.toneSleep; label = "已睡眠"
        default: color = ABColor.fgFaint; label = "检测中…"
        }
        return HStack(spacing: ABMetric.px(4)) {
            Circle().fill(color).frame(width: ABMetric.px(5), height: ABMetric.px(5))
            Text(label)
                .font(.system(size: 11, weight: .semibold))
                .foregroundColor(color)
                .lineLimit(1)
        }
    }

    private func stateColor(_ state: String) -> Color {
        switch state {
        case Reach.online: return ABColor.toneOnline
        case Reach.offline: return ABColor.toneOffline
        case Reach.waking, Reach.shutting, Reach.rebooting: return ABColor.toneBusy
        case Reach.sleeping: return ABColor.toneSleep
        default: return ABColor.toneOffline
        }
    }

    /// 这块网卡在这轮心跳里的结果（通 = 延迟，不通 = 标出）
    @ViewBuilder
    private func lanHitBadge(_ row: CqRow, _ addr: String) -> some View {
        if let host = ServerProfile.normalizeHost(addr),
           let hit = row.hits.first(where: {
               $0.kind == ServerProfile.routeLan && ServerProfile.normalizeHost($0.origin) == host
           }) {
            Text(hit.ok ? "通 \(hit.ms)ms" : "不通")
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(hit.ok ? ABColor.ok : ABColor.bad)
                .lineLimit(1)
                .padding(.leading, ABMetric.px(6))
        }
    }

    /// 远程配对逐条平铺（三个区块一条一行，与 PC 端三区块列表同形）
    @ViewBuilder
    private func pairsBlock(_ row: CqRow) -> some View {
        if !row.pairs.isEmpty {
            VStack(alignment: .leading, spacing: ABMetric.px(3)) {
                ForEach(row.pairs) { pair in
                    HStack(spacing: 0) {
                        Text(pair.state)
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundColor(pairStateColor(pair))
                            .lineLimit(1)
                            .frame(width: ABMetric.px(72), alignment: .leading)

                        if !pair.channel.isEmpty {
                            Text(pair.channel)
                                .font(.system(size: 10, weight: .semibold))
                                .foregroundColor(ABColor.fgDim)
                                .lineLimit(1)
                                // Compose 的 Row 里非 weight 子项先量、weight 子项吃剩余空间；
                                // SwiftUI 默认按比例压缩，得显式抬高这些尾巴的优先级，
                                // 让地址那一格吸收截断（Android 就是地址先省略）
                                .layoutPriority(1)
                                .padding(.trailing, ABMetric.px(5))
                        }

                        Text(pair.addr)
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundColor(pair.enabled ? ABColor.fg : ABColor.fgFaint)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .frame(maxWidth: .infinity, alignment: .leading)

                        if !pair.lane.isEmpty {
                            Text(pair.lane)
                                .font(.system(size: 10))
                                .foregroundColor(ABColor.fgFaint)
                                .lineLimit(1)
                                .layoutPriority(1)
                                .padding(.leading, ABMetric.px(5))
                        }
                        if !pair.source.isEmpty {
                            Text(pair.source)
                                .font(.system(size: 10))
                                .foregroundColor(ABColor.fgFaint)
                                .lineLimit(1)
                                .truncationMode(.tail)
                                .layoutPriority(1)
                                .padding(.leading, ABMetric.px(5))
                        }
                        if !pair.url.isEmpty && pair.enabled {
                            Button("复制") {
                                ABClipboard.copyRemoteSlot(pair.channel + "地址", pair.url)
                            }
                            .buttonStyle(.plain)
                            .font(.system(size: 10))
                            .foregroundColor(ABColor.fgFaint)
                            .layoutPriority(2)
                            .padding(.leading, ABMetric.px(5))
                            Button("打开") {
                                if let url = URL(string: pair.url) {
                                    UIApplication.shared.open(url)
                                } else {
                                    toast("没有能打开网页的应用")
                                }
                            }
                            .buttonStyle(.plain)
                            .font(.system(size: 10))
                            .foregroundColor(ABColor.fgFaint)
                            .layoutPriority(2)
                            .padding(.leading, ABMetric.px(5))
                        }
                    }
                    .padding(.leading, ABMetric.px(4))
                }

                if !row.tunnelNote.isEmpty {
                    HStack(spacing: ABMetric.px(5)) {
                        DrawnIcon.info(ABColor.warn, size: ABMetric.px(11))
                        Text(row.tunnelNote)
                            .font(.system(size: 10))
                            .foregroundColor(ABColor.warn)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .padding(.top, ABMetric.px(7))
        }
    }

    private func pairStateColor(_ pair: PairRowView) -> Color {
        if !pair.enabled { return ABColor.fgFaint }
        if pair.state.hasPrefix("使用中") { return ABColor.accent }
        if pair.ok { return ABColor.ok }
        if pair.state == "不通" { return ABColor.bad }
        return ABColor.fgFaint
    }

    // MARK: - 引导项按钮（BootButton 1592-1724）

    private func bootButtons(_ row: CqRow) -> some View {
        let detected = row.channels.filter { $0.detected }
        let missing = row.channels.filter { !$0.detected }
        return VStack(spacing: ABMetric.px(6)) {
            bootButton(row, kind: Kind.refind, group: row.refind)
            bootButton(row, kind: Kind.grub, group: row.grub)
            // 三引导器通道：检测到的展开成按钮（与 PC 网页端 boot-list-view.js 同一套数据）
            ForEach(detected) { group in
                channelButton(row, group: group)
            }
            // 未检测到的收进一行折叠提示（PC 端「OpenCore（未检测到）」同款语义）
            if !missing.isEmpty {
                Text(missing.map { $0.label }.joined(separator: "、") + "（未检测到）")
                    .font(.system(size: 10))
                    .foregroundColor(ABColor.fgFaint)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.leading, ABMetric.px(11))
                    .padding(.top, ABMetric.px(1))
            }
        }
        .padding(.top, ABMetric.px(11))
    }

    /// 三引导器通道按钮：外观与 rEFInd/GRUB 一致，值区显示当前默认 / 首个动作。
    /// 注意可用性看 actionCount（entries + OC 预演行）：OC 的条目全在 menuPreview，
    /// 只看顶层 entries 会把 OC 误判成「没有可用动作」、点不开弹层。
    private func channelButton(_ row: CqRow, group: ChannelGroup) -> some View {
        let work = busy[row.profile.id]
        let working = work == "boot:ch-\(group.id)"
        let usable = group.actionCount > 0 && row.state == Reach.online && work == nil
        var valueLabel: String
        var dotKind = ""
        if group.entries.isEmpty {
            // 顶层空 → 动作可能全在 OC 预演行里：值区显示预演里的默认项 / 首行
            let previewRows = group.menuPreview?.rows ?? []
            if previewRows.isEmpty {
                if row.state == Reach.unknown { valueLabel = "读取中…" }
                else if row.state == Reach.offline { valueLabel = "离线不可改" }
                else { valueLabel = "没有可用动作" }
            } else if let defaultRow = previewRows.first(where: { $0.isDefault && !$0.spec.isEmpty }) {
                valueLabel = defaultRow.title
                dotKind = defaultRow.kind
            } else {
                valueLabel = previewRows[0].title
                dotKind = previewRows[0].kind
            }
        } else {
            let current = group.currentDefault.trimmingCharacters(in: .whitespaces)
            if current.isEmpty, let first = group.entries.first {
                valueLabel = first.title
                dotKind = first.icon
            } else {
                valueLabel = current
                dotKind = "menu"
            }
        }

        let background = !usable ? ABColor.toneSoftDisabled : ABColor.toneSoft
        let border = !usable ? ABColor.toneDashed : ABColor.line

        return Button {
            if group.actionCount == 0 {
                toast("该设备没有可用的 \(group.label) 动作")
            } else if row.state != Reach.online {
                toast("设备不在线，无法修改引导项")
            } else {
                sheet = SheetRequest(rid: row.profile.id, kind: Kind.channel, channel: group)
            }
        } label: {
            HStack(spacing: 0) {
                Text(group.label)
                    .font(.system(size: 10.5, weight: .semibold, design: .monospaced))
                    .foregroundColor(usable ? ABColor.fgDim : ABColor.fgFaint.opacity(0.75))
                    .lineLimit(1)
                    .frame(width: ABMetric.px(46), alignment: .leading)

                if working {
                    ProgressView()
                        .scaleEffect(0.6)
                        .frame(width: ABMetric.px(13), height: ABMetric.px(13))
                    Spacer().frame(width: ABMetric.px(7))
                } else if !dotKind.isEmpty {
                    bootDot(iconKind(dotKind))
                    Spacer().frame(width: ABMetric.px(7))
                }

                Text(valueLabel)
                    .font(.system(size: 12.5, weight: usable ? .medium : .regular))
                    .foregroundColor(usable ? ABColor.fg : ABColor.fgFaint.opacity(0.85))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)

                if group.permission == "nvram" && usable {
                    Text("⤴")
                        .font(.system(size: 10.5, weight: .bold))
                        .foregroundColor(ABColor.fgFaint)
                        .padding(.leading, ABMetric.px(5))
                }
                if usable {
                    DrawnIcon.chevronDown(ABColor.fgFaint, size: ABMetric.px(14))
                        .padding(.leading, ABMetric.px(7))
                }
            }
            .padding(.leading, ABMetric.px(11))
            .padding(.trailing, ABMetric.px(10))
            .frame(maxWidth: .infinity)
            .frame(height: ABMetric.px(42))
            .background(
                RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                    .fill(background))
            .overlay {
                if usable {
                    RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                        .strokeBorder(border, lineWidth: 1)
                } else {
                    RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                        .strokeBorder(border, style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                }
            }
        }
        .buttonStyle(.plain)
    }

    /// 后端 icon 值（linux/windows/macos/menu）→ 圆点色（win 蓝 / mac 灰 / linux 橙 / menu 空心）
    private func iconKind(_ icon: String) -> String {
        switch icon {
        case "windows": return "win"
        case "macos": return "mac"
        case "linux": return "linux"
        default: return "auto"
        }
    }

    private func bootButton(_ row: CqRow, kind: String, group: BootGroup) -> some View {
        let title = kind == Kind.refind ? "rEFInd" : "GRUB"
        let work = busy[row.profile.id]
        let working = work != nil && work!.hasPrefix("boot:")
        let pending = kind == Kind.grub && grubPending(group)
        let usable = group.available && row.state == Reach.online && work == nil

        var valueLabel: String
        var dotKind = ""
        if !group.available {
            if row.state == Reach.unknown { valueLabel = "读取中…" }
            else if row.state == Reach.offline { valueLabel = "离线不可改" }
            else { valueLabel = "未检测到" }
        } else if kind == Kind.refind {
            let hit = group.options.first { $0.value == group.current }
            valueLabel = hit?.label ?? (group.current.isEmpty ? "未设置" : group.current)
            dotKind = hit?.kind ?? "linux"
        } else {
            let rows = GrubFormat.displayRows(group, selectedFormat: BootFormatStore.shared.value)
            let hit = rows.first { $0.option?.value == group.current && !$0.isGroup }
            if let hit {
                valueLabel = hit.text
                dotKind = hit.option?.kind ?? "linux"
            } else {
                valueLabel = group.current.isEmpty ? "未设置" : group.current
                dotKind = "linux"
            }
        }

        let background = !usable ? ABColor.toneSoftDisabled
            : (pending ? ABColor.tonePendBg : ABColor.toneSoft)
        let border = !usable ? ABColor.toneDashed
            : (pending ? ABColor.tonePendBorder : ABColor.line)

        return Button {
            if group.options.isEmpty {
                toast("该设备没有可选的 \(title) 启动项")
            } else if row.state != Reach.online {
                toast("设备不在线，无法修改引导项")
            } else {
                sheet = SheetRequest(rid: row.profile.id, kind: kind)
            }
        } label: {
            HStack(spacing: 0) {
                Text(title)
                    .font(.system(size: 10.5, weight: .semibold, design: .monospaced))
                    .foregroundColor(usable ? ABColor.fgDim : ABColor.fgFaint.opacity(0.75))
                    .lineLimit(1)
                    .frame(width: ABMetric.px(46), alignment: .leading)

                if working {
                    ProgressView()
                        .scaleEffect(0.6)
                        .frame(width: ABMetric.px(13), height: ABMetric.px(13))
                    Spacer().frame(width: ABMetric.px(7))
                } else if !dotKind.isEmpty {
                    bootDot(dotKind)
                    Spacer().frame(width: ABMetric.px(7))
                }

                Text(valueLabel)
                    .font(.system(size: 12.5, weight: usable ? .medium : .regular))
                    .foregroundColor(usable ? ABColor.fg : ABColor.fgFaint.opacity(0.85))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)

                if pending {
                    Text("待重启")
                        .font(.system(size: 9.5, weight: .bold))
                        .foregroundColor(ABColor.tonePendText)
                        .lineLimit(1)
                        .padding(.horizontal, ABMetric.px(5))
                        .padding(.vertical, ABMetric.px(1))
                        .background(
                            RoundedRectangle(cornerRadius: ABMetric.px(4), style: .continuous)
                                .fill(ABColor.tonePendBorder.opacity(0.32)))
                        .padding(.leading, ABMetric.px(6))
                }
                if usable {
                    DrawnIcon.chevronDown(ABColor.fgFaint, size: ABMetric.px(14))
                        .padding(.leading, ABMetric.px(7))
                }
            }
            .padding(.leading, ABMetric.px(11))
            .padding(.trailing, ABMetric.px(10))
            .frame(maxWidth: .infinity)
            .frame(height: ABMetric.px(42))
            .background(
                RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                    .fill(background))
            .overlay {
                if usable {
                    RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                        .strokeBorder(border, lineWidth: 1)
                } else {
                    RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                        .strokeBorder(border, style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                }
            }
        }
        .buttonStyle(.plain)
    }

    /// 启动项分类小圆点：win 蓝 / mac 灰 / linux 橙 / auto 空心环
    private func bootDot(_ kind: String) -> some View {
        let color = ABColor.bootDot(kind)
        return Group {
            if kind == "auto" {
                Circle().strokeBorder(color, lineWidth: 1.6)
                    .frame(width: ABMetric.px(7), height: ABMetric.px(7))
            } else {
                Circle().fill(color)
                    .frame(width: ABMetric.px(7), height: ABMetric.px(7))
            }
        }
    }

    /// 待生效条
    private func pendingBar(_ row: CqRow) -> some View {
        HStack(spacing: ABMetric.px(6)) {
            DrawnIcon.info(ABColor.tonePendText, size: ABMetric.px(12))
            Text("引导项已修改，重启后生效")
                .font(.system(size: 11))
                .foregroundColor(ABColor.tonePendText)
            Spacer(minLength: 0)
            Button("撤销") {
                switchBoot(row.profile, kind: Kind.grub, value: row.grub.saved, verb: "撤销")
            }
            .buttonStyle(.plain)
            .font(.system(size: 11, weight: .bold))
            .foregroundColor(ABColor.accent)
        }
        .padding(.horizontal, ABMetric.px(10))
        .padding(.vertical, ABMetric.px(7))
        .frame(maxWidth: .infinity)
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(9), style: .continuous)
                .fill(ABColor.tonePendBg))
        .overlay(
            RoundedRectangle(cornerRadius: ABMetric.px(9), style: .continuous)
                .strokeBorder(ABColor.tonePendBorder, lineWidth: 1))
        .padding(.top, ABMetric.px(8))
    }

    private var dashedDivider: some View {
        DashedLine()
            .stroke(ABColor.line, style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
            .frame(height: 1)
            .padding(.top, ABMetric.px(11))
    }

    // MARK: - 电源四键（1728-1809）

    private func powerButtons(_ row: CqRow) -> some View {
        // 安卓恒定一行四键（HostCard 1170-1196：Row + 每键 weight(1f)），任何宽度都不折行
        HStack(spacing: ABMetric.px(6)) {
            powerButton(row, action: PowerRoute.actionWake, label: "唤醒",
                        hint: PowerHint.text(row.profile, action: PowerRoute.actionWake,
                                             fallback: row.profile.hasMacs ? "MAC 魔术包" : "未配 MAC",
                                             remoteActive: row.connRemote))
            powerButton(row, action: PowerRoute.actionSleep, label: "睡眠",
                        hint: PowerHint.text(row.profile, action: PowerRoute.actionSleep,
                                             fallback: "挂起到内存",
                                             remoteActive: row.connRemote))
            powerButton(row, action: PowerRoute.actionReboot, label: "重启",
                        hint: PowerHint.text(row.profile, action: PowerRoute.actionReboot,
                                             remoteActive: row.connRemote))
            powerButton(row, action: PowerRoute.actionShutdown, label: "关机",
                        hint: PowerHint.text(row.profile, action: PowerRoute.actionShutdown,
                                             remoteActive: row.connRemote))
        }
        .padding(.top, ABMetric.px(11))
    }

    private func powerButton(_ row: CqRow, action: String, label: String, hint: String) -> some View {
        let working = busy[row.profile.id] == "power:\(action)"
        // 四个电源键**任何时候都能按**；离线卡片上「唤醒」给成实心强调色（只是高亮）
        let solid = row.state != Reach.online && action == PowerRoute.actionWake
        let accent: Color = {
            switch action {
            case PowerRoute.actionWake: return ABColor.toneOnline
            case PowerRoute.actionSleep: return ABColor.accent
            case PowerRoute.actionReboot: return ABColor.powerReboot
            default: return ABColor.bad
            }
        }()
        let contentColor = solid ? Color.white : ABColor.fg
        let hintColor = solid ? Color.white.opacity(0.75) : ABColor.fgFaint

        return Button {
            performPower(row.profile, action: action)
        } label: {
            Group {
                if working {
                    ProgressView()
                        .scaleEffect(0.6)
                        .tint(accent)
                } else {
                    VStack(spacing: 0) {
                        HStack(spacing: ABMetric.px(5)) {
                            icon(for: action, tint: contentColor)
                            Text(label)
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundColor(contentColor)
                                .lineLimit(1)
                        }
                        if !hint.isEmpty {
                            // 两行：每个按钮只有 1/4 卡片宽，单行必然截断
                            Text(hint)
                                .font(.system(size: 8.5))
                                .lineSpacing(0)
                                .foregroundColor(hintColor)
                                .lineLimit(2)
                                .multilineTextAlignment(.center)
                                .padding(.top, ABMetric.px(1))
                                .padding(.horizontal, ABMetric.px(2))
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity)
            .frame(height: ABMetric.px(54))
            .background(
                RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                    .fill(solid ? ABColor.toneOnline : ABColor.card))
            .overlay {
                if !solid {
                    RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                        .strokeBorder(ABColor.line, lineWidth: 1)
                }
            }
        }
        .buttonStyle(.plain)
    }

    private func icon(for action: String, tint: Color) -> some View {
        switch action {
        case PowerRoute.actionWake:
            return AnyView(DrawnIcon.power(tint, size: ABMetric.px(15)))
        case PowerRoute.actionSleep:
            return AnyView(DrawnIcon.moon(tint, size: ABMetric.px(15)))
        case PowerRoute.actionReboot:
            return AnyView(DrawnIcon.restart(tint, size: ABMetric.px(15)))
        default:
            return AnyView(DrawnIcon.powerOff(tint, size: ABMetric.px(15)))
        }
    }

    // MARK: - 卡片 ⋮ 菜单（CardMenu 1256-1310）

    private func cardMenu(_ row: CqRow) -> some View {
        Menu {
            Button {
                openComputerPage(row.profile.id)
            } label: {
                Label("打开页面", systemImage: "rectangle.on.rectangle")
            }
            Button {
                openInBrowser(row.profile)
            } label: {
                Label("在浏览器打开", systemImage: "safari")
            }
            Divider()
            Button {
                reloadBoot(row.profile)
            } label: {
                Label("重新探测这台电脑", systemImage: "arrow.clockwise")
            }
            Button {
                openEdit(row.profile)
            } label: {
                Label("编辑这台电脑", systemImage: "pencil")
            }
            Button {
                if let host = ServerProfile.normalizeHost(row.profile.host) {
                    ABClipboard.copyRemoteSlot("地址", host)
                }
            } label: {
                Label("复制地址", systemImage: "doc.on.doc")
            }
            Button {
                SystemInfoCenter.shared.show(row.profile)
            } label: {
                Label("系统信息", systemImage: "info.circle")
            }
            Divider()
            Button(role: .destructive) {
                deleteTarget = row.profile
            } label: {
                Label("删除这台电脑", systemImage: "trash")
            }
        } label: {
            DrawnIcon.more(ABColor.fgFaint, size: ABMetric.px(17))
                .frame(width: ABMetric.px(30), height: ABMetric.px(30))
                .contentShape(Rectangle())
        }
        .padding(.leading, ABMetric.px(4))
    }

    // MARK: - 底部选择器（PickSheet 1893-2056）

    @ViewBuilder
    private var pickSheetOverlay: some View {
        if let request = sheet {
            let help = request.kind == Kind.help
            let profile = store.profile(request.rid)
            let group: BootGroup? = {
                guard !help, request.kind != Kind.channel else { return nil }
                let info = boots[request.rid]
                return request.kind == Kind.refind ? info?.refind : info?.grub
            }()

            ZStack(alignment: .bottom) {
                Color.black.opacity(0.42)
                    .ignoresSafeArea()
                    .contentShape(Rectangle())
                    .onTapGesture { sheet = nil }

                VStack(spacing: 0) {
                    sheetHandle
                    // 标题行
                    HStack(spacing: 0) {
                        Text(help ? "操作说明"
                             : request.kind == Kind.channel
                                ? "\(request.channel?.label ?? "通道") 启动项"
                                : (request.kind == Kind.refind ? "rEFInd 启动项" : "GRUB 启动项"))
                            .font(.system(size: 14.5, weight: .bold))
                            .foregroundColor(ABColor.fg)
                            .lineLimit(1)
                        Spacer(minLength: ABMetric.px(8))
                        if !help, let profile {
                            Text(profile.displayName)
                                .font(.system(size: 11))
                                .foregroundColor(ABColor.fgFaint)
                                .lineLimit(1)
                                .truncationMode(.tail)
                        }
                        Button {
                            sheet = nil
                        } label: {
                            DrawnIcon.close(ABColor.fgDim, size: ABMetric.px(14))
                                .frame(width: ABMetric.px(30), height: ABMetric.px(30))
                                .background(Circle().fill(ABColor.elevate))
                        }
                        .buttonStyle(.plain)
                        .padding(.leading, ABMetric.px(8))
                    }
                    .padding(.leading, ABMetric.px(18))
                    .padding(.trailing, ABMetric.px(8))
                    .padding(.bottom, ABMetric.px(10))

                    ScrollView {
                        VStack(alignment: .leading, spacing: 0) {
                            if help {
                                helpContent
                            } else if request.kind == Kind.channel,
                                      let channel = request.channel, let profile {
                                channelPickerContent(profile, channel: channel)
                            } else if let group, !group.options.isEmpty, let profile {
                                bootPickerContent(profile, kind: request.kind, group: group)
                            } else {
                                Text(group?.note.isEmpty == false ? group!.note
                                     : (request.channel?.error.isEmpty == false
                                        ? request.channel!.error : "没有可选的启动项"))
                                    .font(.system(size: 12.5))
                                    .lineSpacing(ABMetric.px(9))
                                    .foregroundColor(ABColor.fgDim)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, ABMetric.px(18))
                    }
                    .frame(maxHeight: ABMetric.px(460))
                    Spacer(minLength: ABMetric.px(12))
                }
                .frame(maxWidth: .infinity)
                .background(
                    UnevenRoundedRectangle(topLeadingRadius: ABMetric.px(20),
                                           bottomLeadingRadius: 0,
                                           bottomTrailingRadius: 0,
                                           topTrailingRadius: ABMetric.px(20),
                                           style: .continuous)
                        .fill(ABColor.card))
            }
            .transition(.opacity)
        }
    }

    private var sheetHandle: some View {
        Capsule()
            .fill(ABColor.lineStrong)
            .frame(width: ABMetric.px(36), height: ABMetric.px(4))
            .padding(.top, ABMetric.px(8))
            .padding(.bottom, ABMetric.px(8))
    }

    private var helpContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(Self.helpLines.enumerated()), id: \.offset) { _, line in
                HStack(alignment: .top, spacing: ABMetric.px(6)) {
                    Text("·")
                        .font(.system(size: 12.5))
                        .foregroundColor(ABColor.fgDim)
                    Text(line)
                        .font(.system(size: 12.5))
                        .lineSpacing(ABMetric.px(9.5))
                        .foregroundColor(ABColor.fgDim)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.vertical, ABMetric.px(4))
            }
            Text("电源操作不会弹出确认框")
                .font(.system(size: 11))
                .foregroundColor(ABColor.fgFaint)
                .padding(.top, ABMetric.px(8))
        }
    }

    private static let helpLines = [
        "唤醒：发标准魔术包（UDP 9）。睡眠 / 重启 / 关机按每台电脑「电源路由」里配的通道发送，"
            + "可以是局域网广播包，也可以走它自己的 AnyBoot 接口",
        "四个电源键任何时候都能按，不会因为在线/离线变灰；关机与重启立即执行，不会二次确认，请先保存工作",
        "引导项（rEFInd / GRUB）在下次重启时生效，可点「撤销」恢复成原来那项",
        "systemd-boot / OpenCore / Clover 的动作卡同样在下次重启时生效；带 ⤴ 的必经 NVRAM 写",
        "电脑离线时读不到引导项",
        "显示「未检测到」= 这台电脑没部署对应的引导管理器（rEFInd / GRUB / OpenCore / Clover / systemd-boot），不是读取失败",
    ]

    @ViewBuilder
    private func bootPickerContent(_ profile: ServerProfile,
                                   kind: String,
                                   group: BootGroup) -> some View {
        if kind == Kind.grub {
            grubFormatChips(group)
                .padding(.bottom, ABMetric.px(6))
        }

        let rows: [GrubRow] = kind == Kind.refind
            ? group.options.map { GrubRow(option: $0, text: $0.label, isGroup: false) }
            : GrubFormat.displayRows(group, selectedFormat: BootFormatStore.shared.value)

        ForEach(rows) { row in
            if row.isGroup {
                Text(row.text)
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
                    .padding(.top, ABMetric.px(7))
                    .padding(.bottom, ABMetric.px(2))
                    .padding(.leading, ABMetric.px(2))
            } else if let option = row.option {
                let selected = option.value == group.current
                Button {
                    sheet = nil
                    switchBoot(profile, kind: kind, value: option.value, verb: "切换")
                } label: {
                    HStack(spacing: ABMetric.px(8)) {
                        bootDot(option.kind)
                        Text(row.text)
                            .font(.system(size: 14))
                            .foregroundColor(selected ? ABColor.accent : ABColor.fg)
                            .lineLimit(2)
                            .truncationMode(.tail)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if selected {
                            DrawnIcon.check(ABColor.accent, size: ABMetric.px(16))
                        }
                    }
                    .padding(.horizontal, ABMetric.px(11))
                    .padding(.vertical, ABMetric.px(12))
                    .frame(maxWidth: .infinity)
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                            .fill(selected ? ABColor.accent.opacity(0.14) : .clear))
                    .padding(.bottom, ABMetric.px(4))
                }
                .buttonStyle(.plain)
            }
        }

        Text("选择后将在下次重启时生效")
            .font(.system(size: 11))
            .foregroundColor(ABColor.fgFaint)
            .padding(.top, ABMetric.px(8))
    }

    /// 三引导器通道的动作列表（对齐 PC 网页端 makeBootChannelCard 的条目区）
    @ViewBuilder
    private func channelPickerContent(_ profile: ServerProfile, channel: ChannelGroup) -> some View {
        // 说明与健康检查：收进感叹号（用户 2026-10-28：不要占位置），点了才展开
        let ocNote = channel.id == "opencore"
            ? "「设为默认 ⤴」等价于 OC 菜单里的 Ctrl+Enter：写 OCBt0080 / OCBtOrder，不碰被 macOS 挡住的 Boot* 变量；电脑端弹授权时批准即可"
            : ""
        let nvramNote = channel.permission == "nvram"
            ? "⤴ 切换必经 NVRAM 写（电脑端可能弹管理员授权）"
            : ""
        ChannelNoteDisclosure(
            detail: channel.detail.trimmingCharacters(in: .whitespaces),
            issues: channel.health,
            extra: [nvramNote, ocNote].filter { !$0.isEmpty }.joined(separator: "\n"))
        // OC 菜单预演：只有 OpenCore 通道有，动作行从这里出（顶层 entries 对 OC 是空的）
        let previewRows: [ChannelMenuRow] = channel.id == "opencore"
            ? (channel.menuPreview?.rows ?? [])
            : []
        if !previewRows.isEmpty {
            ForEach(previewRows) { row in
                ocMenuRowView(profile, channel: channel, row: row)
            }
        }
        if channel.entries.isEmpty && previewRows.isEmpty {
            Text(channel.error.isEmpty ? "没有可执行的动作" : channel.error)
                .font(.system(size: 12.5))
                .foregroundColor(ABColor.fgDim)
        } else {
            ForEach(channel.entries) { entry in
                Button {
                    sheet = nil
                    switchChannelAction(profile, channel: channel, entry: entry, verb: "切换")
                } label: {
                    HStack(alignment: .top, spacing: ABMetric.px(8)) {
                        bootDot(iconKind(entry.icon))
                        VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                            HStack(spacing: ABMetric.px(5)) {
                                Text(entry.title)
                                    .font(.system(size: 14))
                                    .foregroundColor(ABColor.fg)
                                    .lineLimit(2)
                                if entry.requiresElevation {
                                    Text("⤴")
                                        .font(.system(size: 10.5, weight: .bold))
                                        .foregroundColor(ABColor.fgFaint)
                                }
                            }
                            if !entry.detail.isEmpty {
                                Text(entry.detail)
                                    .font(.system(size: 11))
                                    .lineSpacing(ABMetric.px(7))
                                    .foregroundColor(ABColor.fgFaint)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(.horizontal, ABMetric.px(11))
                    .padding(.vertical, ABMetric.px(10))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                            .fill(ABColor.elevate.opacity(0.5)))
                    .padding(.bottom, ABMetric.px(6))
                }
                .buttonStyle(.plain)
            }
            Text("动作在下次重启时生效")
                .font(.system(size: 11))
                .foregroundColor(ABColor.fgFaint)
                .padding(.top, ABMetric.px(4))
        }
    }

    /// GRUB 条目文字格式六档（跟随电脑 + 极简/简易/中等/原始/复杂）
    private func grubFormatChips(_ group: BootGroup) -> some View {
        let effective = GrubFormat.effective(selected: BootFormatStore.shared.value, group: group)
        let selectedValue = BootFormatStore.shared.value
        return VStack(alignment: .leading, spacing: 0) {
            Text("条目文字格式（当前：\(selectedValue.isEmpty ? "跟随电脑 · \(effective)" : effective)）")
                .font(.system(size: 11))
                .foregroundColor(ABColor.fgFaint)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: ABMetric.px(6)) {
                    ForEach(GrubFormat.all, id: \.self) { value in
                        let selected = selectedValue == value
                        Button {
                            BootFormatStore.shared.value = value
                        } label: {
                            Text(GrubFormat.label(value))
                                .font(.system(size: 11.5, weight: selected ? .bold : .regular))
                                .foregroundColor(selected ? .white : ABColor.fgDim)
                                .padding(.horizontal, ABMetric.px(9))
                                .padding(.vertical, ABMetric.px(5))
                                .background(
                                    RoundedRectangle(cornerRadius: ABMetric.px(8), style: .continuous)
                                        .fill(selected ? ABColor.accent : ABColor.elevate))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.top, ABMetric.px(6))
                .padding(.bottom, ABMetric.px(4))
            }
        }
    }

    // MARK: - 删除确认

    @ViewBuilder
    private var deleteDialog: some View {
        ABDialogHost(isPresented: deleteTarget != nil, onDismiss: { deleteTarget = nil }) {
            ABDialogCard("删除这台电脑") {
                Text("确定删除「\(deleteTarget?.displayName ?? "")」？\n\n"
                     + "只删掉手机上的这条档案，不会动目标电脑上的 AnyBoot。")
                    .font(.system(size: 13))
                    .lineSpacing(ABMetric.px(7))
                    .foregroundColor(ABColor.fgDim)
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: ABMetric.px(8)) {
                    Spacer(minLength: 0)
                    ABButton(title: "取消", kind: .ghost, compact: true) { deleteTarget = nil }
                    ABButton(title: "删除", kind: .destructive, compact: true) {
                        if let target = deleteTarget { doDelete(target.id) }
                        deleteTarget = nil
                    }
                }
            }
        }
    }

    /// ⋮ →「电脑列表」：列出所有服务端 + 各自的地址，点一台直接进编辑。
    /// 一比一照 `ComputerListActivity.MoreServerListDialog`（492-556）：标题、每台把所有
    /// 配置好的 IP 都亮出来（不用「另有 N 个地址」摘要）、底部「添加 / 关闭」。
    private var serverListDialog: some View {
        ABDialogHost(isPresented: serverListOpen,
                     onDismiss: { serverListOpen = false }) {
            ABDialogCard("服务端列表（点一台进行编辑）") {
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        if store.servers.isEmpty {
                            Text("还没有电脑 —— 点「添加服务端」建一条。")
                                .font(.system(size: 13))
                                .foregroundColor(ABColor.fgDim)
                        }
                        ForEach(store.servers) { profile in
                            Button {
                                serverListOpen = false
                                openEdit(profile)
                            } label: {
                                VStack(alignment: .leading, spacing: 0) {
                                    Text("· " + profile.displayName)
                                        .font(.system(size: 14))
                                        .foregroundColor(ABColor.fg)
                                    if profile.addresses.isEmpty {
                                        Text("　(未配地址)")
                                            .font(.system(size: 11))
                                            .foregroundColor(ABColor.fgDim)
                                    } else {
                                        ForEach(Array(profile.addresses.enumerated()), id: \.offset) { index, _ in
                                            Text("　" + profile.hostAt(index))
                                                .font(.system(size: 11, design: .monospaced))
                                                .foregroundColor(ABColor.fgDim)
                                        }
                                    }
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.vertical, ABMetric.px(10))
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                .frame(maxHeight: ABMetric.px(300))

                HStack(spacing: ABMetric.px(8)) {
                    Spacer(minLength: 0)
                    ABButton(title: "添加", kind: .primary, compact: true) {
                        serverListOpen = false
                        openAdd()
                    }
                    ABButton(title: "关闭", kind: .ghost, compact: true) {
                        serverListOpen = false
                    }
                }
            }
        }
    }

    // MARK: - Toast 堆叠（1813-1836）
    @ViewBuilder
    private var toastStack: some View {
        VStack(spacing: ABMetric.px(6)) {
            ForEach(toasts) { item in
                HStack(spacing: ABMetric.px(7)) {
                    DrawnIcon.check(Color(hex: 0x6EE7B7), size: ABMetric.px(14))
                    Text(item.text)
                        .font(.system(size: 12))
                        .foregroundColor(.white)
                        .lineLimit(3)
                }
                .padding(.horizontal, ABMetric.px(14))
                .padding(.vertical, ABMetric.px(9))
                .background(
                    RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                        .fill(Color(hex: 0x1E1B4B)))
                .transition(.opacity)
            }
        }
        .padding(.horizontal, ABMetric.px(20))
        .padding(.bottom, ABMetric.px(24))
        .animation(.easeInOut(duration: 0.2), value: toasts.count)
        .allowsHitTesting(false)
    }

    // MARK: - 动作

    private func toast(_ text: String) {
        let item = ToastItem(text: text)
        toasts.append(item)
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 2_200_000_000)
            toasts.removeAll { $0.id == item.id }
        }
    }

    private func selectComputer(_ id: String) {
        selectedId = id
        store.select(id)
    }

    private func openAdd() {
        editorTarget = ComputerEditorTarget(profile: nil)
    }

    private func openEdit(_ profile: ServerProfile) {
        editorTarget = ComputerEditorTarget(profile: profile)
    }

    private func openComputerPage(_ id: String) {
        store.select(id)
        nav.activeProfileId = id
        nav.go(.computers)
    }

    /// 浏览器打开默认走**直连**（用户 2026-10-28）：URL 用档案里的局域网地址 + 令牌，
    /// 跳板机只影响 App 内的连接通道 —— 浏览器侧连不上转发端点，但直连地址仍然有效。
    private func openInBrowser(_ profile: ServerProfile) {
        guard let host = ServerProfile.normalizeHost(profile.host) else {
            toast("这台电脑还没有可用地址")
            return
        }
        guard let url = profile.url(for: host, transport: "") else {
            toast("地址还没就绪")
            return
        }
        UIApplication.shared.open(url)
    }

    private func doDelete(_ id: String) {
        store.remove(id)
        boots.removeValue(forKey: id)
        transients.removeValue(forKey: id)
        busy.removeValue(forKey: id)
        if selectedId == id { selectedId = "" }
        toast("已删除")
    }

    // MARK: - 网络：探测 + 读引导项

    private func reloadBoot(_ profile: ServerProfile) {
        if busy[profile.id] != nil { return }
        busy[profile.id] = "boot:load"
        Task {
            let info = await BootService.fetchBootInfo(profile)
            await MainActor.run {
                boots[profile.id] = info
                busy.removeValue(forKey: profile.id)
                learnMachineOs(profile, info.machineOs)
            }
        }
    }

    private func refreshAll(silent: Bool = false) {
        if refreshing { return }
        let snapshot = store.servers
        if snapshot.isEmpty { return }
        refreshing = true
        if !silent { toast("正在探测全部电脑…") }
        Task {
            for profile in snapshot {
                let info = await BootService.fetchBootInfo(profile)
                await MainActor.run {
                    boots[profile.id] = info
                    learnMachineOs(profile, info.machineOs)
                }
            }
            await MainActor.run { refreshing = false }
        }
    }

    /// 探测顺手学到的系统族记进档案，卡片上的徽标下次开界面就能直接画
    private func learnMachineOs(_ profile: ServerProfile, _ os: String) {
        let normalized = ServerProfile.normalizeMachineOs(os)
        guard !normalized.isEmpty, profile.machineOs != normalized else { return }
        guard var target = store.profile(profile.id) else { return }
        target.machineOs = normalized
        store.updateInPlace(target)
    }

    private func startAutoProbe() {
        probeTask?.cancel()
        probeTask = Task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 10_000_000_000)
                guard !Task.isCancelled else { return }
                await MainActor.run { refreshAll(silent: true) }
            }
        }
    }

    /// 三引导器通道切换（busy 键前缀 boot:ch-<id>，通道按钮据此转圈）
    private func switchChannelAction(_ profile: ServerProfile,
                                     channel: ChannelGroup,
                                     entry: ChannelEntryView,
                                     verb: String) {
        if busy[profile.id] != nil { return }
        if entry.id.trimmingCharacters(in: .whitespaces).isEmpty {
            toast("没有可执行的动作")
            return
        }
        busy[profile.id] = "boot:ch-\(channel.id)"
        Task {
            do {
                let message = try await BootService.switchChannel(profile, group: channel, entry: entry)
                await MainActor.run {
                    busy.removeValue(forKey: profile.id)
                    toast(message)
                }
                reloadBoot(profile)
            } catch {
                await MainActor.run {
                    busy.removeValue(forKey: profile.id)
                    let message = (error as? RemoteHttpError)?.message ?? error.localizedDescription
                    toast("\(verb) 失败：\(message)")
                }
            }
        }
    }

    /// OC 菜单预演的一行：序号 + 标题（含系统图标）+ 卷·路径 + 备注，
    /// 行尾「✓ 已是默认」或「设为默认 ⤴」（对齐 PC 网页端 ocMenuRow 的语义：
    /// 默认行也占位渲染，不让人误以为选不了）
    private func ocMenuRowView(_ profile: ServerProfile,
                               channel: ChannelGroup,
                               row: ChannelMenuRow) -> some View {
        let pathText = !row.volume.isEmpty && row.volume != "—"
            ? "\(row.volume) · \(row.path)"
            : row.path
        return HStack(alignment: .top, spacing: ABMetric.px(8)) {
            VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                HStack(spacing: ABMetric.px(5)) {
                    Text(ocMenuIcon(row.kind) + row.title)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(ABColor.fg)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    if row.isDefault {
                        Text("✓ 已是默认")
                            .font(.system(size: 10.5, weight: .bold))
                            .foregroundColor(ABColor.accent)
                    }
                    if row.auxiliary {
                        Text("辅助")
                            .font(.system(size: 10))
                            .foregroundColor(ABColor.fgFaint)
                    }
                }
                Text(pathText)
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if !row.note.isEmpty {
                    Text(row.note)
                        .font(.system(size: 10.5))
                        .lineSpacing(ABMetric.px(7))
                        .foregroundColor(ABColor.fgFaint)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if !row.isDefault && !row.spec.trimmingCharacters(in: .whitespaces).isEmpty {
                Button {
                    sheet = nil
                    switchPreviewAction(profile, channel: channel, row: row)
                } label: {
                    Text("设为默认 ⤴")
                        .font(.system(size: 11.5, weight: .semibold))
                        .foregroundColor(.white)
                        .padding(.horizontal, ABMetric.px(10))
                        .padding(.vertical, ABMetric.px(7))
                        .background(
                            RoundedRectangle(cornerRadius: ABMetric.px(9), style: .continuous)
                                .fill(ABColor.accent))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, ABMetric.px(11))
        .padding(.vertical, ABMetric.px(10))
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(11), style: .continuous)
                .fill(ABColor.elevate.opacity(0.5)))
        .padding(.bottom, ABMetric.px(6))
    }

    /// OC 预演行「设为默认 ⤴」的动作（busy 键与通道切换共用，按钮据此转圈）
    private func switchPreviewAction(_ profile: ServerProfile,
                                     channel: ChannelGroup,
                                     row: ChannelMenuRow) {
        if busy[profile.id] != nil { return }
        if row.spec.trimmingCharacters(in: .whitespaces).isEmpty {
            toast("这一项不支持设为默认")
            return
        }
        busy[profile.id] = "boot:ch-\(channel.id)"
        Task {
            do {
                let message = try await BootService.switchPreviewRow(profile, row: row)
                await MainActor.run {
                    busy.removeValue(forKey: profile.id)
                    toast(message)
                }
                reloadBoot(profile)
            } catch {
                await MainActor.run {
                    busy.removeValue(forKey: profile.id)
                    let message = (error as? RemoteHttpError)?.message ?? error.localizedDescription
                    toast("设为默认失败：\(message)")
                }
            }
        }
    }

    /// OC 菜单条目的系统图标（与 PC 网页端 OC_MENU_ICONS 同语义）
    private func ocMenuIcon(_ kind: String) -> String {
        switch kind {
        case "macos": return "🍎 "
        case "windows": return "🪟 "
        case "linux": return "🐧 "
        case "tool": return "⚙️ "
        default: return ""
        }
    }

    private func switchBoot(_ profile: ServerProfile, kind: String, value: String, verb: String) {
        if busy[profile.id] != nil { return }
        if value.trimmingCharacters(in: .whitespaces).isEmpty {
            toast("没有可切换的目标启动项")
            return
        }
        busy[profile.id] = "boot:\(kind)"
        Task {
            do {
                let message = try await BootService.switchBoot(profile, kind: kind, value: value)
                await MainActor.run {
                    busy.removeValue(forKey: profile.id)
                    toast(message)
                }
                reloadBoot(profile)
            } catch {
                await MainActor.run {
                    busy.removeValue(forKey: profile.id)
                    let message = (error as? RemoteHttpError)?.message ?? error.localizedDescription
                    toast("\(verb) 失败：\(message)")
                }
            }
        }
    }

    private func performPower(_ profile: ServerProfile, action: String) {
        // 不做拦截：四个电源键任何时候都能按，连按就是连发
        busy[profile.id] = "power:\(action)"
        transients[profile.id] = {
            switch action {
            case PowerRoute.actionWake: return Reach.waking
            case PowerRoute.actionSleep: return Reach.sleeping
            case PowerRoute.actionReboot: return Reach.rebooting
            default: return Reach.shutting
            }
        }()
        Task {
            do {
                let message = try await PowerOps.perform(profile, action: action)
                await MainActor.run {
                    if busy[profile.id] == "power:\(action)" { busy.removeValue(forKey: profile.id) }
                    toast(message)
                }
            } catch {
                await MainActor.run {
                    if busy[profile.id] == "power:\(action)" { busy.removeValue(forKey: profile.id) }
                    transients.removeValue(forKey: profile.id)
                    let text = (error as? RemoteHttpError)?.message ?? error.localizedDescription
                    toast("\(ServerProfile.powerActionLabel(action))没发出去：\(text)")
                }
                return
            }
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            await MainActor.run { transients.removeValue(forKey: profile.id) }
            reloadBoot(profile)
        }
    }

    // MARK: - 传输方式 / 自动重载（⋮ 菜单里三个弹窗）

    @State private var failReloadOpen = false
    @State private var transportOpen = false
    @State private var customSec = ""

    private var failReloadSec: Int { FailReloadPref.sec }

    private func setFailReloadSec(_ sec: Int) {
        FailReloadPref.sec = sec
        // Android saveMoreFailReloadSec（627-633）
        toast(sec > 0 ? "连不上时每 \(sec) 秒自动重载" : "已关闭自动重载")
    }

    private var transportLabel: String {
        switch store.transport {
        case ServerStore.transportPush: return "推送"
        case ServerStore.transportPoll: return "轮询"
        default: return "自动"
        }
    }

    /// 连不上时自动重载（MoreFailReloadDialog 559-625）：5 档单选（选中不关弹窗）+
    /// 自定义秒数输入 +「应用」；确认键「完成」。
    private var failReloadDialog: some View {
        ABDialogHost(isPresented: failReloadOpen,
                     onDismiss: { failReloadOpen = false }) {
            ABDialogCard("连不上时自动重载") {
                VStack(alignment: .leading, spacing: 0) {
                    Text("连接失败时每隔一段时间自动重载当前页面（重新探测全部地址）。")
                        .font(.system(size: 13))
                        .foregroundColor(ABColor.fgDim)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer().frame(height: ABMetric.px(10))
                    ForEach([5, 10, 30, 60, 0], id: \.self) { sec in
                        radioRow(sec == failReloadSec, "永不（关闭自动重载）", "每 \(sec) 秒") {
                            setFailReloadSec(sec)
                        }
                    }
                    HStack(spacing: ABMetric.px(8)) {
                        radioRow(![5, 10, 30, 60, 0].contains(failReloadSec), "", "") {}
                        TextField("自定义秒数", text: $customSec)
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fg)
                            .keyboardType(.numberPad)
                            .onChange(of: customSec) { _, value in
                                customSec = value.filter { $0.isNumber }
                            }
                            .padding(.horizontal, ABMetric.px(11))
                            .frame(height: ABMetric.px(42))
                            .background(
                                RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                                    .fill(ABColor.base))
                            .overlay(
                                RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                                    .strokeBorder(ABColor.lineStrong, lineWidth: 1))
                        Button("应用") {
                            guard let parsed = Int(customSec), parsed >= 0 else {
                                toast("间隔要是不小于 0 的整数秒")
                                return
                            }
                            setFailReloadSec(parsed)
                            failReloadOpen = false
                        }
                        .buttonStyle(.plain)
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(ABColor.accent)
                    }
                    .padding(.top, ABMetric.px(2))
                }
                HStack(spacing: ABMetric.px(8)) {
                    Spacer(minLength: 0)
                    ABButton(title: "完成", kind: .primary, compact: true) { failReloadOpen = false }
                }
            }
        }
        .onAppear { customSec = "" }
    }

    /// 传输方式（MoreTransportDialog 636-681）：三档单选，点选即存即关 + toast；「取消」收尾。
    private var transportDialog: some View {
        ABDialogHost(isPresented: transportOpen,
                     onDismiss: { transportOpen = false }) {
            ABDialogCard("传输方式") {
                VStack(alignment: .leading, spacing: 0) {
                    radioRow(store.transport == ServerStore.transportAuto,
                             "自动（推送优先，连不上退回轮询）", "") {
                        pickTransport(ServerStore.transportAuto)
                    }
                    radioRow(store.transport == ServerStore.transportPush,
                             "推送（WebSocket）", "") {
                        pickTransport(ServerStore.transportPush)
                    }
                    radioRow(store.transport == ServerStore.transportPoll,
                             "轮询（HTTP 每 2s）", "") {
                        pickTransport(ServerStore.transportPoll)
                    }
                }
                HStack(spacing: ABMetric.px(8)) {
                    Spacer(minLength: 0)
                    ABButton(title: "取消", kind: .ghost, compact: true) { transportOpen = false }
                }
            }
        }
    }

    private func pickTransport(_ value: String) {
        store.setTransport(value)
        transportOpen = false
        toast("已选择「\(transportLabel)」：电脑页重载后生效")
    }

    /// M3 单选行（RadioButton 20dp 圆 + 文案 13sp fg）：主文案非空才显示
    private func radioRow(_ selected: Bool, _ label: String, _ fallback: String,
                          _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: ABMetric.px(10)) {
                ZStack {
                    Circle()
                        .strokeBorder(selected ? ABColor.accent : ABColor.fgDim, lineWidth: 2)
                        .frame(width: ABMetric.px(18), height: ABMetric.px(18))
                    if selected {
                        Circle().fill(ABColor.accent)
                            .frame(width: ABMetric.px(9), height: ABMetric.px(9))
                    }
                }
                if !label.isEmpty {
                    Text(label)
                        .font(.system(size: 13))
                        .foregroundColor(ABColor.fg)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(.vertical, ABMetric.px(5))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - 小工具

/// 通道的说明 + 健康检查：默认收进一枚 ⚠/ⓘ 小圆标（用户 2026-10-28：不要占位置）。
/// ⚠ = 有健康检查条目，ⓘ = 纯说明；展开后 detail / issues / extra 依次铺开。
/// 与 PC 网页端 .channel-note-toggle / .channel-note 同一套交互。
private struct ChannelNoteDisclosure: View {
    let detail: String
    let issues: [String]
    let extra: String

    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                expanded.toggle()
            } label: {
                Text(issues.isEmpty ? "ⓘ" : "⚠")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(issues.isEmpty ? ABColor.fgFaint : ABColor.warn)
                    .frame(width: ABMetric.px(22), height: ABMetric.px(22))
                    .background(
                        Circle().strokeBorder(ABColor.fgFaint.opacity(0.4), lineWidth: 1))
            }
            .buttonStyle(.plain)
            .padding(.bottom, ABMetric.px(6))

            if expanded {
                if !detail.isEmpty {
                    Text(detail)
                        .font(.system(size: 11.5))
                        .lineSpacing(ABMetric.px(8))
                        .foregroundColor(ABColor.fgFaint)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.bottom, ABMetric.px(6))
                }
                ForEach(issues, id: \.self) { issue in
                    Text("· " + issue)
                        .font(.system(size: 11))
                        .lineSpacing(ABMetric.px(7))
                        .foregroundColor(ABColor.warn)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.bottom, ABMetric.px(2))
                }
                if !extra.isEmpty {
                    Text(extra)
                        .font(.system(size: 10.5))
                        .lineSpacing(ABMetric.px(7))
                        .foregroundColor(ABColor.fgFaint)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.top, ABMetric.px(2))
                        .padding(.bottom, ABMetric.px(4))
                }
            }
        }
    }
}

/// 虚线
private struct DashedLine: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.move(to: CGPoint(x: rect.minX, y: rect.midY))
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
        return path
    }
}

/// GRUB 条目文字格式偏好（落盘 key = grubListFormat，与 Android 同 key）
@MainActor
final class BootFormatStore: ObservableObject {

    static let shared = BootFormatStore()

    @Published var value: String {
        didSet { UserDefaults.standard.set(value, forKey: BootService.formatKey) }
    }

    private init() {
        value = UserDefaults.standard.string(forKey: BootService.formatKey) ?? ""
    }
}
