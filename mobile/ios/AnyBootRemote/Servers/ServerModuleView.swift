import SwiftUI

/// 服务器模块（对齐 Android `ServersActivity`）：顶部栏 + 五个页面 + 底部四项导航。
///
/// 「服务器 / 密钥 / 文件」是底部导航的三页，「终端」是动作项（跳最近活会话），
/// 会话页与传输页在顶部 ⋮ 里 —— 与设计稿一致，别改成五项导航。
struct ServerModuleView: View {

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var jumps = JumpStore.shared
    @ObservedObject private var center = TransferCenter.shared

    @State private var moreOpen = false
    @State private var fabMenuOpen = false

    private var menusOpen: Bool { moreOpen || fabMenuOpen }

    private func closeMenus() {
        moreOpen = false
        fabMenuOpen = false
    }

    var body: some View {
        ZStack(alignment: .bottom) {
            ABColor.base.ignoresSafeArea()

            VStack(spacing: 0) {
                // ⚠️ 顶栏必须抬到页面内容之上：⋮ / ＋ 的下拉菜单是用 .overlay 挂在
                // 按钮上的，而 VStack 按**书写顺序**绘制 —— 不设 zIndex 的话，后画的
                // 页面内容（搜索框、服务器卡片）会把菜单整个盖住，只在顶栏下沿
                // 露出一条十几点的白边，看起来就是「三个点点了没反应 / 被挡住」。
                topBar
                    .zIndex(10)

                Group {
                    switch store.page {
                    case .servers: ServersListPage()
                    case .keys: KeysPage()
                    case .files: FilesPage()
                    case .sessions: SessionsPage()
                    case .transfers: TransfersPage()
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                // 菜单开着时，点空白处收起（对齐 Android Popup 的 outside tap 关闭）。
                // 只盖页面区，不盖顶栏，否则 ⋮ 自己也被挡。
                .overlay {
                    if menusOpen {
                        Color.clear
                            .contentShape(Rectangle())
                            .onTapGesture { closeMenus() }
                    }
                }

                if store.page == .files { transferStatusBar }
                bottomNav
            }
            // 抽屉由 RootView 统一挂在根上（三页共用同一份），这里不能再挂：
            // 之前这里读的是 store.drawerOpen，而 ABDrawerOverlay 读的是
            // ABNavigator.drawerOpen，点 ☰ 只会渲染出一个空的 GeometryReader。
        }
        // 终端页全屏盖上来（会话保活由 Registry 负责，页面重建不丢）
        .fullScreenCover(isPresented: Binding(
            get: { store.terminalJumpId != nil },
            set: { if !$0 { store.closeTerminal() } })) {
            TerminalPage(jumpId: store.terminalJumpId ?? "", cdPath: store.terminalCdPath)
        }
        .overlay { moduleDialogs }
        .onChange(of: store.page) { _, _ in closeMenus() }
        .onAppear {
            TerminalSessionsRegistry.setObserver { store.markSessionsChanged() }
            center.onChanged = { store.markTransfersChanged() }
        }
    }

    // MARK: - 顶部栏（`ServersActivity.kt:256`）

    private var topBar: some View {
        HStack(spacing: 0) {
            ABBarIcon(text: "☰", description: "侧边栏") { ABNavigator.shared.openDrawer() }

            Text(title)
                .font(.system(size: 20, weight: .bold))
                .foregroundColor(ABColor.fg)

            Spacer().frame(width: ABMetric.px(10))
            pagePill
            Spacer(minLength: 0)

            if store.page == .servers {
                Button {
                    store.terminalSettingsOpen = true
                } label: {
                    DrawnIcon.gear(ABColor.fgDim, size: 20)
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("终端设置")
            }

            // ⚠️ 命中区 44×44（HIG 最小）：此前是 Text("⋮") + padding，
            // 实测 accessibility frame 只有 7×20pt，手指几乎点不到。
            // 图标改用设计系统里手绘的三点（与 Android Icons.kt 同源），不依赖字形。
            Button {
                fabMenuOpen = false
                moreOpen.toggle()
            } label: {
                DrawnIcon.more(ABColor.fgDim, size: 18)
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("更多")
            .overlay(alignment: .topTrailing) {
                if moreOpen {
                    VStack(alignment: .leading, spacing: 0) {
                        menuItem("会话（进行中 / 历史）") { store.page = .sessions }
                        menuItem("传输管理") { store.page = .transfers }
                        menuItem("终端设置") { store.terminalSettingsOpen = true }
                    }
                    .frame(width: ABMetric.px(190))
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .fill(ABColor.elevate))
                    .overlay(
                        RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
                    .shadow(color: .black.opacity(0.18), radius: 12, y: 6)
                    .offset(y: ABMetric.px(40))
                }
            }
            .zIndex(3)

            if store.page == .servers || store.page == .keys {
                Button {
                    if store.page == .servers {
                        closeMenus()
                        store.editTarget = nil
                        store.editOpen = true
                    } else {
                        moreOpen = false
                        fabMenuOpen.toggle()
                    }
                } label: {
                    ZStack {
                        Circle().fill(ABColor.accent)
                        DrawnIcon.plus(ABColor.onAccent, size: 18)
                    }
                    .frame(width: ABMetric.px(36), height: ABMetric.px(36))
                    .frame(width: 44, height: 44)      // 命中区补到 44（可见圆仍是 36）
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(store.page == .servers ? "添加服务器" : "添加密钥")
                .padding(.leading, ABMetric.px(8))
                .overlay(alignment: .topTrailing) {
                    if fabMenuOpen {
                        VStack(alignment: .leading, spacing: 0) {
                            menuItem("生成密钥对") { store.generateKeyOpen = true }
                            menuItem("导入私钥文件") { store.keyImportOpen = true }
                        }
                        .frame(width: ABMetric.px(170))
                        .background(
                            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                                .fill(ABColor.elevate))
                        .overlay(
                            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                                .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
                        .shadow(color: .black.opacity(0.18), radius: 12, y: 6)
                        .offset(y: ABMetric.px(40))
                    }
                }
                .zIndex(2)
            }
        }
        .padding(.leading, ABMetric.px(4))
        .padding(.top, ABMetric.px(10))
        .padding(.trailing, ABMetric.px(20))
        .padding(.bottom, ABMetric.px(8))
    }

    private func menuItem(_ title: String, action: @escaping () -> Void) -> some View {
        Button {
            moreOpen = false
            fabMenuOpen = false
            action()
        } label: {
            Text(title)
                .font(.system(size: 13))
                .foregroundColor(ABColor.fg)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, ABMetric.px(14))
                .padding(.vertical, ABMetric.px(11))
        }
        .buttonStyle(.plain)
    }

    private var title: String {
        switch store.page {
        case .keys: return "密钥"
        case .files: return "文件"
        case .sessions: return "会话"
        case .transfers: return "传输"
        default: return "服务器"
        }
    }

    // MARK: - 标题旁状态 pill（`:347`）

    @ViewBuilder
    private var pagePill: some View {
        switch store.page {
        case .servers:
            let online = jumps.jumps.filter { $0.isSsh && SshSessionManager.shared.isUp($0.id) }.count
            pillText("\(online) 在线")
        case .keys:
            pillText("\(jumps.keys.count) 把")
        case .files:
            let up = FilesStore.shared.isUp
            HStack(spacing: ABMetric.px(5)) {
                Circle()
                    .fill(up ? ABColor.accent : ABColor.fgFaint)
                    .frame(width: ABMetric.px(6), height: ABMetric.px(6))
                Text(up ? "SFTP 就绪" : "未连接")
                    .font(.system(size: 10))
                    .foregroundColor(up ? ABColor.accent : ABColor.fgDim)
            }
            .padding(.horizontal, ABMetric.px(8))
            .padding(.vertical, ABMetric.px(3))
            .background(
                Capsule().fill(up ? ABColor.accent.opacity(0.1) : ABColor.elevate))
        default:
            EmptyView()
        }
    }

    private func pillText(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 10, design: .monospaced))
            .foregroundColor(ABColor.fgDim)
            .padding(.horizontal, ABMetric.px(8))
            .padding(.vertical, ABMetric.px(3))
            .background(Capsule().fill(ABColor.elevate))
            .overlay(Capsule().strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
    }

    // MARK: - 文件页底部传输状态条（`:470`）

    @ViewBuilder
    private var transferStatusBar: some View {
        if let task = center.active.first {
            let paused = task.state == .paused
            HStack(spacing: ABMetric.px(10)) {
                ProgressView()
                    .scaleEffect(0.6)
                    .frame(width: ABMetric.px(14), height: ABMetric.px(14))
                VStack(alignment: .leading, spacing: ABMetric.px(4)) {
                    Text(task.name)
                        .font(.system(size: 11))
                        .foregroundColor(ABColor.fg)
                        .lineLimit(1)
                    GeometryReader { geo in
                        ZStack(alignment: .leading) {
                            RoundedRectangle(cornerRadius: ABMetric.px(3), style: .continuous)
                                .fill(ABColor.elevate)
                            RoundedRectangle(cornerRadius: ABMetric.px(3), style: .continuous)
                                .fill(paused ? ABColor.warn : ABColor.accent)
                                .frame(width: geo.size.width
                                       * min(1, max(paused ? 0.01 : 0.02, task.fraction)))
                        }
                    }
                    .frame(height: ABMetric.px(6))
                }
                Text(paused ? "暂停"
                     : (task.total <= 0 ? "…" : "\(Int(task.fraction * 100))%"))
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(paused ? ABColor.warn : ABColor.accent)
            }
            .padding(.horizontal, ABMetric.px(20))
            .padding(.vertical, ABMetric.px(10))
            .background(ABColor.card)
            .overlay(alignment: .top) {
                Rectangle().fill(ABColor.line).frame(height: ABMetric.hairline)
            }
            .contentShape(Rectangle())
            .onTapGesture { store.page = .transfers }
        }
    }

    // MARK: - 底部导航（`:400` 设计稿四项：服务器 / 终端 / 密钥 / 文件）

    private var bottomNav: some View {
        HStack(spacing: 0) {
            navItem("服务器", active: store.page == .servers, icon: { tint in
                DrawnIcon.serverStack(tint, size: 20)
            }) { store.page = .servers }

            // 「终端」是动作项：跳最近一个活会话，不是本模块里的一页
            navItem("终端", active: false, icon: { tint in
                DrawnIcon.term(tint, size: 20)
            }) {
                if let id = store.openLatestSession() { store.openTerminal(jumpId: id) }
            }

            navItem("密钥", active: store.page == .keys, icon: { tint in
                DrawnIcon.key(tint, size: 20)
            }) { store.page = .keys }

            navItem("文件", active: store.page == .files, icon: { tint in
                DrawnIcon.folder(tint, size: 20)
            }) { store.page = .files }
        }
        .padding(.top, ABMetric.px(8))
        .padding(.bottom, ABMetric.px(6))
        .background(ABColor.card)
        .overlay(alignment: .top) {
            Rectangle().fill(ABColor.line).frame(height: ABMetric.hairline)
        }
    }

    private func navItem(_ label: String, active: Bool,
                         icon: @escaping (Color) -> some View,
                         onClick: @escaping () -> Void) -> some View {
        let tint = active ? ABColor.accent : ABColor.fgFaint
        return Button(action: onClick) {
            VStack(spacing: ABMetric.px(3)) {
                icon(tint)
                Text(label)
                    .font(.system(size: 10, weight: active ? .medium : .regular))
                    .foregroundColor(tint)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, ABMetric.px(2))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - 模块级弹层

    @ViewBuilder
    private var moduleDialogs: some View {
        JumpEditSheet()
        GenerateKeyDialog()
        DeleteJumpDialog()
        ManualPasswordDialog()
        TerminalSettingsSheet()
    }
}

// MARK: - 删除服务器确认（`ServersActivity.kt:942`）

struct DeleteJumpDialog: View {

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var jumps = JumpStore.shared

    var body: some View {
        ABDialogHost(isPresented: store.deleteTarget != nil,
                     onDismiss: { store.deleteTarget = nil }) {
            ABDialogCard("删除服务器") {
                if let server = store.deleteTarget {
                    let via = jumps.computersVia(server.id)
                    let viaServers = jumps.jumps.filter {
                        $0.id != server.id && $0.jumpServerId == server.id
                    }.count
                    ABDialogNote("确定删除「\(server.displayName)」吗？"
                                 + (via > 0 ? "\n\n注意：有 \(via) 台电脑配置了经由它连接，删除后这些电脑会退回直连。" : "")
                                 + (viaServers > 0 ? "\n\n另有 \(viaServers) 台服务器经由它中转，删除后退回直连。" : ""),
                                 color: ABColor.fg)
                    ABDialogButtons(confirmTitle: "删除", confirmColor: ABColor.bad,
                                    onConfirm: {
                                        store.deleteTarget = nil
                                        SshSessionManager.shared.shutdown(jumpId: server.id)
                                        if let session = TerminalSessionsRegistry.get(server.id) {
                                            TerminalSessionsRegistry.kill(session)
                                        }
                                        // 经由它中转的其他服务器退回直连（与电脑经由跳板机删除时同口径）
                                        for other in jumps.jumps where other.jumpServerId == server.id {
                                            var copy = other
                                            copy.jumpServerId = ""
                                            jumps.upsert(copy)
                                        }
                                        jumps.remove(server.id)
                                        store.markSessionsChanged()
                                    },
                                    onDismiss: { store.deleteTarget = nil })
                }
            }
        }
    }
}

// MARK: - 应急手输 SSH 密码（`:3166`）

/// 服务器页入口（测试连接等）：弹层本身与终端页共用 `ManualPasswordBody`
struct ManualPasswordDialog: View {

    @ObservedObject private var store = ServerModuleStore.shared

    var body: some View {
        ABDialogHost(isPresented: store.manualPassword != nil,
                     onDismiss: { store.manualPassword = nil }) {
            ABDialogCard("输入 SSH 密码") {
                if let request = store.manualPassword {
                    ManualPasswordBody(request: request,
                                       onDone: { store.manualPassword = nil })
                        .id(request.id)
                }
            }
        }
    }
}
