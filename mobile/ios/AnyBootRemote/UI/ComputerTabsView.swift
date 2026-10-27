import SwiftUI

/// 电脑页：多标签 + 保活 WebView + 电源悬浮球 + 完整错误面板。
struct ComputerTabsView: View {

    @ObservedObject private var center = ComputerTabCenter.shared
    @ObservedObject private var store = ServerStore.shared
    @ObservedObject private var nav = ABNavigator.shared
    @ObservedObject private var jumps = JumpStore.shared
    @ObservedObject private var heartbeat = Heartbeat.shared

    @State private var editorTarget: ComputerEditorTarget?
    @State private var powerExpanded = false
    @State private var powerBusy = false
    @State private var helpOpen = false
    @State private var failReloadOpen = false
    @State private var transportOpen = false
    @State private var chipTick = 0
    /// ⋮ →「电脑列表」弹的服务端选择对话框（对齐 Android ServerListDialog）
    @State private var serverListOpen = false
    /// 「连不上时自动重载」的计时任务
    @State private var failReloadTask: Task<Void, Never>?
    /// 首次启动：冷启动直接弹「连接 AnyBoot 服务端」（Android firstRunPending）
    @State private var firstRunPending = UserDefaults.standard.object(forKey: "firstRunDone") == nil

    private var activeProfile: ServerProfile? { store.profile(center.activeId) }
    private var activeTab: ComputerTab? {
        center.tabs.first { $0.profileId == center.activeId }
    }

    var body: some View {
        page
            .abPage()
            .sheet(item: $editorTarget, content: editorSheet)
            .onAppear { onFirstAppear() }
            .onChange(of: nav.activeProfileId, perform: activateIfNeeded)
            .onChange(of: statusTick, perform: { _ in scheduleFailReload(failed: activeTab?.status == .failed) })
            .overlay { overlays }
    }

    private var page: some View {
        VStack(spacing: 0) {
            topBar
            content
        }
    }

    private var statusTick: ABLoadStatus { activeTab?.status ?? .unknown }

    private func activateIfNeeded(_ value: String) {
        if !value.isEmpty { center.activate(value) }
    }

    private var overlays: some View {
        ZStack {
            powerLayer
            helpOverlay
            failReloadOverlay
            transportOverlay
            serverListDialog
        }
    }

    private func editorSheet(_ target: ComputerEditorTarget) -> some View {
        ComputerEditorView(profile: target.profile, firstRun: target.firstRun) { result in
            if case .save(let profile) = result {
                store.upsert(profile)
            } else if case .delete(let id) = result {
                store.remove(id)
                center.close(id)
            }
        }
        .id(target.id)   // 每次呈现都是新身份 → init 的回显一定生效
    }

    private func onFirstAppear() {
        center.syncProfiles()
        if center.activeId.isEmpty {
            let target = nav.activeProfileId.isEmpty ? store.activeId : nav.activeProfileId
            if !target.isEmpty { center.activate(target) }
            else if let first = store.servers.first { center.activate(first.id) }
        }
        if store.connectAll { center.connectAll() }
        // 首启：先弹「连接 AnyBoot 服务端」再谈别的（MainActivity 365-369）
        if firstRunPending {
            firstRunPending = false
            UserDefaults.standard.set(true, forKey: "firstRunDone")
            if store.servers.isEmpty {
                editorTarget = ComputerEditorTarget(profile: nil, firstRun: true)
            }
        }
        heartbeat.onMachineInfo = handleMachineInfo
    }

    /// 心跳带回来的机器标识 / 系统族回写档案
    private func handleMachineInfo(_ id: String, _ machineId: String,
                                   _ machineName: String, _ os: String) {
        guard var target = store.profile(id) else { return }
        var changed = false
        if !machineId.isEmpty && target.machineId != machineId {
            target.machineId = machineId
            changed = true
        }
        if !machineName.isEmpty && target.machineName != machineName {
            target.machineName = machineName
            changed = true
        }
        let normalized = ServerProfile.normalizeMachineOs(os)
        if !normalized.isEmpty && target.machineOs != normalized {
            target.machineOs = normalized
            changed = true
        }
        if changed { store.updateInPlace(target) }
    }

    /// 失败面板挂着就按间隔重试（0 = 永不）；重载会清掉失败态，本任务随之作废 ——
    /// 失败重现时 onChange 再启一轮（MainActivity LaunchedEffect 379-388）
    private func scheduleFailReload(failed: Bool) {
        failReloadTask?.cancel()
        failReloadTask = nil
        guard failed, FailReloadPref.sec > 0 else { return }
        failReloadTask = Task { @MainActor in
            try? await Task.sleep(nanoseconds: UInt64(FailReloadPref.sec) * 1_000_000_000)
            guard !Task.isCancelled, activeTab?.status == .failed else { return }
            ABToastBus.shared.show("连接失败，自动重载中…")
            center.reloadActive()
        }
    }

    // MARK: - 顶栏（TopBar 403-495）

    private var topBar: some View {
        HStack(spacing: 0) {
            drawerButton
            chipStrip
            addButton
            overflowMenu
        }
        .padding(.leading, ABMetric.px(4))
        .padding(.trailing, ABMetric.px(4))
        .padding(.vertical, ABMetric.px(6))
        .background(ABColor.card)
    }

    private var drawerButton: some View {
        Button {
            nav.openDrawer()
        } label: {
            DrawnIcon.menu(ABColor.fgDim, size: ABMetric.px(20))
                // 44×44 命中区（之前 38，实测可点范围只有 22×22）
                .frame(width: ABMetric.px(44), height: ABMetric.px(44))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("侧边栏")
    }

    private var chipStrip: some View {
        ScrollViewReader { proxy in
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: ABMetric.px(6)) {
                    ForEach(store.servers) { profile in
                        chip(profile)
                            .id(profile.id)
                    }
                }
            }
            .onChange(of: center.activeId) { _, value in
                withAnimation(.easeInOut(duration: 0.2)) {
                    proxy.scrollTo(value, anchor: .center)
                }
            }
        }
    }

    private var addButton: some View {
        Button {
            editorTarget = ComputerEditorTarget(profile: nil)
        } label: {
            Text("＋")
                .font(.system(size: 17))
                .foregroundColor(ABColor.fgDim)
                .frame(width: ABMetric.px(38), height: ABMetric.px(38))
                // 不加这句可点范围只有字形本身（实测 17×20）
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("添加电脑")
    }

    private func chip(_ profile: ServerProfile) -> some View {
        let tab = center.tabs.first { $0.profileId == profile.id }
        let active = profile.id == center.activeId
        let state = heartbeat.state(profile.id)
        let statusColor: Color = {
            if let tab, tab.status == .loading { return ABColor.warn }
            if state.reach == .online { return ABColor.ok }
            if state.reach == .offline { return ABColor.fgFaint }
            if let tab, tab.status == .ok { return ABColor.ok }
            if let tab, tab.status == .failed { return ABColor.bad }
            return ABColor.fgFaint
        }()

        return Button {
            center.activate(profile.id)
            chipTick += 1
        } label: {
            HStack(spacing: 0) {
                Circle()
                    .fill(statusColor)
                    .frame(width: ABMetric.px(8), height: ABMetric.px(8))
                Text(profile.label)
                    .font(.system(size: 13))
                    .foregroundColor(active ? .white : ABColor.fg)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: ABMetric.px(132), alignment: .leading)
                    .padding(.leading, ABMetric.px(6))
                if !ABFormat.osShowName(profile.machineOs).isEmpty {
                    ABOsBadge(os: profile.machineOs, onDark: active) {
                        SystemInfoCenter.shared.show(profile)
                    }
                    .padding(.leading, ABMetric.px(5))
                }
                Text(connLabel(profile, tab: tab))
                    .font(.system(size: 9))
                    .foregroundColor(active ? .white.opacity(0.75) : ABColor.fgFaint)
                    .lineLimit(1)
                    .padding(.leading, ABMetric.px(5))
            }
            .padding(.leading, ABMetric.px(10))
            .padding(.trailing, ABMetric.px(12))
            .padding(.vertical, ABMetric.px(5))
            .frame(height: ABMetric.px(22))
            .background(
                RoundedRectangle(cornerRadius: ABMetric.px(16), style: .continuous)
                    .fill(active ? ABColor.accent : ABColor.elevate))
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button("编辑这台电脑") {
                editorTarget = ComputerEditorTarget(profile: profile)
            }
            Button("系统信息") { SystemInfoCenter.shared.show(profile) }
        }
    }

    /// 标签上的连接标注：**没连上就写「未连接」**，不许默认「直连」
    private func connLabel(_ profile: ServerProfile, tab: ComputerTab?) -> String {
        let state = heartbeat.state(profile.id)
        let online = state.reach == .online
        if !online && tab?.status != .ok { return "未连接" }
        if online && !state.kind.isEmpty {
            return ServerProfile.routeLabel(state.kind)
        }
        if tab?.usingRemote == true { return "远程" }
        if !profile.jumpServerId.trimmingCharacters(in: .whitespaces).isEmpty
            && !profile.routeDisabled.contains(ServerProfile.routeJump) {
            return "跳板机"
        }
        return "直连"
    }

    private var overflowMenu: some View {
        Menu {
            Button("添加电脑") {
                editorTarget = ComputerEditorTarget(profile: nil)
            }
            Button("编辑当前电脑") {
                if let current = activeProfile {
                    editorTarget = ComputerEditorTarget(profile: current)
                } else {
                    ABToastBus.shared.show("还没有当前电脑 —— 点标签或卡片右侧 ⋮ 编辑某一台")
                }
            }
            Button("电脑列表") { serverListOpen = true }
            Button("当前电脑的系统信息") {
                if let current = activeProfile {
                    SystemInfoCenter.shared.show(current)
                } else {
                    ABToastBus.shared.show("还没有当前电脑 —— 先点一个标签")
                }
            }
            Button("重载当前页面") { center.reloadActive() }
            Button("全部重载") { center.reloadAll() }
            Button(store.connectAll ? "✓ 启动时连接全部服务端" : "启动时连接全部服务端") {
                let next = !store.connectAll
                store.setConnectAll(next)
                if next { center.connectAll() }
                ABToastBus.shared.show(next ? "已开启：启动时把所有服务端一起连上" : "已关闭：只连当前标签")
            }
            Button(failReloadSec > 0
                   ? "连不上时自动重载（每 \(failReloadSec) 秒）"
                   : "连不上时自动重载（已关闭）") {
                failReloadOpen = true
            }
            Button("传输方式（\(transportLabel)）") { transportOpen = true }
        } label: {
            Text("⋮")
                .font(.system(size: 19))
                .foregroundColor(ABColor.fgDim)
                .frame(width: ABMetric.px(38), height: ABMetric.px(38))
        }
    }

    // MARK: - 内容

    @ViewBuilder
    private var content: some View {
        if store.servers.isEmpty {
            ABEmptyState(icon: "desktopcomputer", title: "还没有电脑",
                         message: "先到「电脑列表」里添加一台。")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if center.tabs.isEmpty {
            ABEmptyState(icon: "rectangle.on.rectangle", title: "还没打开标签",
                         message: "点上面的电脑标签开始连接。")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ZStack {
                // 全部 WebView 都在树上，只显示当前那个 —— 切走不销毁、不重载
                ForEach(center.tabs) { tab in
                    ABWebViewHost(view: tab.webView)
                        .opacity(tab.profileId == center.activeId && tab.status != .failed ? 1 : 0)
                        .allowsHitTesting(tab.profileId == center.activeId)
                }
                if let tab = activeTab, tab.status == .failed {
                    errorPanel(tab)
                }
                // 顶部加载条（MainActivity 322-326：3dp LinearProgressIndicator）
                if activeTab?.status == .loading {
                    VStack {
                        ProgressView()
                            .tint(ABColor.accent)
                            .scaleEffect(y: 0.4)
                            .frame(height: 3)
                            .clipped()
                        Spacer(minLength: 0)
                    }
                    .allowsHitTesting(false)
                }
            }
            .overlay(alignment: .bottomTrailing) { powerFab }
            .overlay(alignment: .bottomLeading) { refreshFab }
        }
    }

    /// 连接失败面板（ErrorPanel 622-669）
    private func errorPanel(_ tab: ComputerTab) -> some View {
        ScrollView {
            VStack(spacing: ABMetric.px(12)) {
                Text("连不上服务端")
                    .font(.system(size: 20, weight: .bold))
                    .foregroundColor(ABColor.fg)
                if !tab.errorDetail.isEmpty {
                    Text(tab.errorDetail)
                        .font(.system(size: 14))
                        .foregroundColor(ABColor.fgDim)
                        .multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Text("顺手检查这几点：\n"
                     + "· 目标电脑的 AnyBoot 是否在跑，设置里「HTTP 服务」是否显示正在监听；\n"
                     + "· 这里填的地址端口，和目标机设置里的端口是否一致；\n"
                     + "· 那个端口是否被别的程序占用 —— 被占用时目标机的服务根本起不来；\n"
                     + "· 目标机防火墙是否放行了这个端口。")
                    .font(.system(size: 12))
                    .lineSpacing(ABMetric.px(5))
                    .foregroundColor(ABColor.fgFaint)
                    .fixedSize(horizontal: false, vertical: true)
                Text(failReloadSec > 0
                     ? "每 \(failReloadSec) 秒自动重试（⋮ 里可改）"
                     : "自动重试已关闭（⋮ 里可改）")
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
                ABButton(title: "重试", kind: .primary) { tab.reload() }
                Button("修改这个服务端") {
                    editorTarget = ComputerEditorTarget(profile: tab.profile)
                }
                .buttonStyle(.plain)
                .font(.system(size: 14))
                .foregroundColor(ABColor.accent)
            }
            .padding(ABMetric.px(24))
            .frame(maxWidth: 560)
            .frame(maxWidth: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(ABColor.card)
    }

    // MARK: - 电源悬浮球（664-816）

    @ViewBuilder
    private var powerLayer: some View {
        if powerExpanded, let profile = activeProfile {
            // 只做点击遮罩收起，不拦截内容
            Color.black.opacity(0.001)
                .ignoresSafeArea()
                .onTapGesture { powerExpanded = false }
        }
    }

    private var powerFab: some View {
        VStack(alignment: .trailing, spacing: ABMetric.px(8)) {
            if powerExpanded, let profile = activeProfile {
                Button {
                    helpOpen = true
                } label: {
                    HStack(spacing: ABMetric.px(4)) {
                        Text("电源走局域网 · 需 sol")
                            .font(.system(size: 10))
                            .foregroundColor(ABColor.fgFaint)
                        Text("?")
                            .font(.system(size: 10))
                            .foregroundColor(ABColor.fgDim)
                            .frame(width: ABMetric.px(18), height: ABMetric.px(18))
                            .background(Circle().fill(ABColor.elevate))
                    }
                    .padding(.leading, ABMetric.px(10))
                    .padding(.trailing, ABMetric.px(5))
                    .padding(.vertical, ABMetric.px(5))
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(10), style: .continuous)
                            .fill(ABColor.card))
                }
                .buttonStyle(.plain)

                powerChip(profile, action: PowerRoute.actionWake, label: "唤醒",
                          hint: PowerHint.text(profile, action: PowerRoute.actionWake,
                                               fallback: profile.hasMacs ? "MAC 魔术包" : "未配 MAC",
                                               remoteActive: activeTab?.usingRemote == true))
                powerChip(profile, action: PowerRoute.actionSleep, label: "睡眠",
                          hint: PowerHint.text(profile, action: PowerRoute.actionSleep,
                                               fallback: "挂起到内存",
                                               remoteActive: activeTab?.usingRemote == true))
                powerChip(profile, action: PowerRoute.actionShutdown, label: "关机",
                          hint: PowerHint.text(profile, action: PowerRoute.actionShutdown,
                                               remoteActive: activeTab?.usingRemote == true))
                powerChip(profile, action: PowerRoute.actionReboot, label: "重启",
                          hint: PowerHint.text(profile, action: PowerRoute.actionReboot,
                                               remoteActive: activeTab?.usingRemote == true))
            }

            Button {
                guard !powerBusy else { return }
                powerExpanded.toggle()
            } label: {
                ZStack {
                    Circle()
                        .fill(powerBusy ? ABColor.fgFaint : ABColor.accent)
                        .frame(width: ABMetric.px(52), height: ABMetric.px(52))
                    if powerBusy {
                        ProgressView().tint(.white).scaleEffect(0.8)
                    } else if powerExpanded {
                        DrawnIcon.close(.white, size: ABMetric.px(18))
                    } else {
                        DrawnIcon.power(.white, size: ABMetric.px(22))
                    }
                }
            }
            .buttonStyle(.plain)
        }
        .padding(.trailing, ABMetric.px(14))
        .padding(.bottom, ABMetric.px(48))
    }

    private func powerChip(_ profile: ServerProfile, action: String,
                           label: String, hint: String) -> some View {
        Button {
            powerExpanded = false
            runPower(profile, action: action)
        } label: {
            VStack(alignment: .leading, spacing: 0) {
                Text(label)
                    .font(.system(size: 14))
                    .foregroundColor(ABColor.fg)
                if !hint.isEmpty {
                    Text(hint)
                        .font(.system(size: 10))
                        .foregroundColor(ABColor.fgFaint)
                        .lineLimit(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.leading, ABMetric.px(14))
            .padding(.trailing, ABMetric.px(14))
            .padding(.vertical, ABMetric.px(7))
            .frame(minWidth: ABMetric.px(210), alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                    .fill(ABColor.card))
        }
        .buttonStyle(.plain)
    }

    private var refreshFab: some View {
        Button {
            center.reloadActive()
        } label: {
            DrawnIcon.refresh(ABColor.fg, size: ABMetric.px(20))
                .frame(width: ABMetric.px(44), height: ABMetric.px(44))
                .background(Circle().fill(ABColor.elevate))
                .overlay(Circle().strokeBorder(ABColor.line, lineWidth: 1))
                .opacity(0.5)
        }
        .buttonStyle(.plain)
        .padding(.leading, ABMetric.px(16))
        .padding(.bottom, ABMetric.px(48))
    }

    private func runPower(_ profile: ServerProfile, action: String) {
        if powerBusy { return }
        powerBusy = true
        Task {
            do {
                let message = try await PowerOps.perform(profile, action: action)
                await MainActor.run {
                    powerBusy = false
                    ABToastBus.shared.show(message)
                }
            } catch {
                await MainActor.run {
                    powerBusy = false
                    let text = (error as? RemoteHttpError)?.message ?? error.localizedDescription
                    ABToastBus.shared.show("\(ServerProfile.powerActionLabel(action))没发出去：\(text)")
                }
            }
        }
    }

    // MARK: - 电源帮助（SleepHelpDialog 1064-1124，逐段照抄）

    @ViewBuilder
    private var helpOverlay: some View {
        ABDialogHost(isPresented: helpOpen, onDismiss: { helpOpen = false }) {
            ABDialogCard("电源 · 睡眠与唤醒") {
                ScrollView {
                    VStack(alignment: .leading, spacing: ABMetric.px(10)) {
                        Text("电源按钮点了**立即执行**，不再弹确认框 —— 各动作的后果都在这里：")
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fg)
                            .fixedSize(horizontal: false, vertical: true)
                        Text("· 睡眠 = 挂起到内存：几乎立刻能醒，但网络服务全停 —— 这个标签会显示离线、页面打不开，这是预期的。叫醒请点「唤醒」（需配好 MAC 并开启网络唤醒）；\n"
                             + "· 关机：电脑立刻关机，网页服务随之中断 —— 之后只能用「唤醒」开机（需配好 MAC），或到机器旁按电源键；\n"
                             + "· 重启：电脑立刻重启，网页会话断开 —— 重启完成后心跳会自动恢复在线；\n"
                             + "· 唤醒：发魔术包让电脑开机，需配好 MAC 并在主板开启网络唤醒。")
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fgDim)
                            .fixedSize(horizontal: false, vertical: true)
                        Text("这些按钮怎么执行（局域网 / sol）")
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fg)
                        Text("给这台电脑配了「局域网电源路由」的动作，会往路由里的**广播地址:端口**发一个 UDP 包，"
                             + "由目标机上装着的 **sol** 按端口执行 —— 这样机器睡着、网页服务没起来也能操作。"
                             + "没装 sol，包发得出去但没人执行。\n"
                             + "没配路由的动作走老路：唤醒发标准 WOL（魔术包），重启/关机/睡眠调电脑端的网页接口。")
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fgDim)
                            .fixedSize(horizontal: false, vertical: true)
                        Text("唤醒包从哪发：")
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fg)
                        Text("· 配了「唤醒」路由 → 按路由里的地址和端口发（MAC 可只挑几块网卡）；\n"
                             + "· 配了 SSH 跳板机 → 由跳板机在内网广播（在外网也能用）；\n"
                             + "· 都没有 → 手机自己往局域网广播（手机必须和电脑同一网段）；\n"
                             + "· 只有「远程接入」可用（出门经公网隧道） → 唤醒发不出去："
                             + "魔术包是局域网 UDP 广播，走不了隧道。重启/关机/睡眠调电脑网页接口，隧道照常可用。")
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fgDim)
                            .fixedSize(horizontal: false, vertical: true)
                        Text("点了没反应，按顺序检查：")
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fg)
                        Text("① 主板 BIOS 里开启了 Wake on LAN / 网络唤醒（只影响唤醒）；\n"
                             + "② 用的是**有线网卡**的 MAC —— 绝大多数无线网卡不支持关机唤醒；\n"
                             + "③ MAC 填在「编辑服务端」里对应地址那一行（如 192.168.0.120:12300 AA:BB:CC:DD:EE:FF）；\n"
                             + "④ 路由里的广播地址和端口对不对（要和 sol 侧监听的一致）；\n"
                             + "⑤ 主机没被拔电源、插排没断电（关机后网卡要靠待机供电收包）。")
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fgDim)
                            .fixedSize(horizontal: false, vertical: true)
                        Text("Windows 还要在「设备管理器 → 网卡 → 电源管理」勾上「允许此设备唤醒计算机」，"
                             + "并关掉「快速启动」（快速启动是休眠，会跳过 WOL）。")
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fgDim)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .frame(maxHeight: ABMetric.px(420))
                HStack {
                    Spacer()
                    ABButton(title: "知道了", kind: .ghost, compact: true) { helpOpen = false }
                }
            }
        }
    }

    // MARK: - 自动重载间隔 / 传输方式 / 服务端列表

    private var failReloadSec: Int { FailReloadPref.sec }

    private func setFailReloadSec(_ sec: Int) {
        FailReloadPref.sec = sec
        // Android saveFailReloadSec（1055-1061）
        ABToastBus.shared.show(sec > 0 ? "连不上时每 \(sec) 秒自动重载" : "已关闭自动重载")
    }

    private var transportLabel: String {
        switch store.transport {
        case ServerStore.transportPush: return "推送"
        case ServerStore.transportPoll: return "轮询"
        default: return "自动"
        }
    }

    /// 自动重载间隔（FailReloadDialog 1005-1053）：5 档单选（选中不关弹窗）+
    /// 自定义秒数输入 +「应用」；确认键「完成」。
    @ViewBuilder
    private var failReloadOverlay: some View {
        ABDialogHost(isPresented: failReloadOpen, onDismiss: { failReloadOpen = false }) {
            ABDialogCard("连不上时自动重载") {
                Text("连接失败时每隔一段时间自动重载当前页面（重新探测全部地址）。")
                    .font(.system(size: 13))
                    .foregroundColor(ABColor.fgDim)
                    .fixedSize(horizontal: false, vertical: true)
                ForEach([5, 10, 30, 60, 0], id: \.self) { sec in
                    radioRow(selected: failReloadSec == sec,
                             label: sec == 0 ? "永不（关闭自动重载）" : "每 \(sec) 秒") {
                        setFailReloadSec(sec)
                    }
                }
                HStack(spacing: ABMetric.px(8)) {
                    radioRow(selected: ![5, 10, 30, 60, 0].contains(failReloadSec), label: "") {}
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
                            ABToastBus.shared.show("间隔要是不小于 0 的整数秒")
                            return
                        }
                        setFailReloadSec(parsed)
                        failReloadOpen = false
                    }
                    .buttonStyle(.plain)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(ABColor.accent)
                }
                HStack {
                    Spacer()
                    ABButton(title: "完成", kind: .ghost, compact: true) { failReloadOpen = false }
                }
            }
        }
        .onAppear { customSec = "" }
    }

    /// 传输方式（TransportDialog 1255-1299）：无说明段，三档点选即存即关 + toast；「取消」
    @ViewBuilder
    private var transportOverlay: some View {
        ABDialogHost(isPresented: transportOpen, onDismiss: { transportOpen = false }) {
            ABDialogCard("传输方式") {
                ForEach(ServerStore.transports, id: \.self) { value in
                    radioRow(selected: store.transport == value,
                             label: label(for: value)) {
                        store.setTransport(value)
                        transportOpen = false
                        ABToastBus.shared.show(
                            "已选择「\(transportLabel)」：重载页面后生效（页面上也能即切）")
                    }
                }
                HStack {
                    Spacer()
                    ABButton(title: "取消", kind: .ghost, compact: true) { transportOpen = false }
                }
            }
        }
    }

    private func label(for transport: String) -> String {
        switch transport {
        case ServerStore.transportPush: return "推送（WebSocket）"
        case ServerStore.transportPoll: return "轮询（HTTP 每 2s）"
        default: return "自动（推送优先，连不上退回轮询）"
        }
    }

    /// M3 单选行（RadioButton 20dp 圆 + 文案）
    private func radioRow(selected: Bool, label: String,
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

    @State private var customSec = ""

    /// ⋮ →「电脑列表」：切换服务端对话框（ServerListDialog 1207-1253）。
    /// ● 选中标记 + 名称 + 地址摘要；「添加」开编辑；「合并同一台电脑的地址」。
    @ViewBuilder
    private var serverListDialog: some View {
        ABDialogHost(isPresented: serverListOpen, onDismiss: { serverListOpen = false }) {
            ABDialogCard("切换服务端（长按标签也能编辑）") {
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        if store.servers.isEmpty {
                            Text("还没有服务端。")
                                .font(.system(size: 13))
                                .foregroundColor(ABColor.fgDim)
                        }
                        ForEach(store.servers) { profile in
                            Button {
                                serverListOpen = false
                                center.activate(profile.id)
                            } label: {
                                Text((profile.id == center.activeId ? "● " : "　")
                                     + profile.label + "　" + addressSummary(profile))
                                    .font(.system(size: 14))
                                    .foregroundColor(ABColor.fg)
                                    .lineLimit(1)
                                    .truncationMode(.tail)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.vertical, ABMetric.px(12))
                                    .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                .frame(maxHeight: ABMetric.px(320))
                HStack(spacing: ABMetric.px(8)) {
                    ABButton(title: "合并同一台电脑的地址", kind: .ghost, compact: true) {
                        serverListOpen = false
                        let removed = store.mergeSameMachine()
                        ABToastBus.shared.show(
                            removed > 0
                                ? "已合并 \(removed) 条重复记录 —— 同一台电脑只留一个标签"
                                : "没有需要合并的：同一台电脑要么已经只有一条，要么分组名不同")
                    }
                    Spacer(minLength: 0)
                    ABButton(title: "添加", kind: .primary, compact: true) {
                        serverListOpen = false
                        editorTarget = ComputerEditorTarget(profile: nil)
                    }
                }
            }
        }
    }

    /// 「主名（另有 N 个地址）」—— Android ServerProfile.addressSummary 同式
    private func addressSummary(_ profile: ServerProfile) -> String {
        let primary = ServerProfile.normalizeHost(profile.host) ?? profile.host
        let spare = profile.addressCount - 1
        return spare > 0 ? "\(primary)（另有 \(spare) 个地址）" : primary
    }
}
