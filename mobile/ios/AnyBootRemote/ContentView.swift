import SwiftUI

/// iOS 端主界面：顶部服务端标签栏 + 每个标签一个独立 WKWebView。
///
/// 标签文本是「组名 · 服务端名」，左侧小圆点是该服务端的在线状态；
/// 一台电脑有多块网卡 / 多次 DHCP 拿到多个 IP 时，把这几条放进同一个分组，
/// 标签描边同色，切换时不会搞混。
struct ContentView: View {

    @StateObject private var store = ServerStore.shared

    @State private var statuses: [String: LoadStatus] = [:]
    @State private var reloadTokens: [String: Int] = [:]
    @State private var errors: [String: String] = [:]

    @State private var editing: ServerProfile?
    @State private var showEditor = false
    @State private var showList = false
    @State private var firstRun = false

    var body: some View {
        VStack(spacing: 0) {
            tabBar
            Divider()
            content
        }
        .toolbar {
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
                } label: {
                    Label("菜单", systemImage: "ellipsis.circle")
                }
            }
        }
        .sheet(isPresented: $showEditor) {
            ServerEditorSheet(profile: editing, firstRun: firstRun, knownGroups: store.knownGroups) { result in
                switch result {
                case .save(let profile):
                    store.upsert(profile)
                    store.select(profile.id)
                    // 地址/token 可能变了，按新配置重连这个标签（其它标签不受影响）
                    reload(profile.id)
                case .delete(let id):
                    store.remove(id)
                    statuses[id] = nil
                    errors[id] = nil
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
            if store.servers.isEmpty {
                startAdd(first: true)
            } else if store.connectAll {
                connectAll()
            } else {
                reloadActive()
            }
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
        let border = profile.group.trimmedAnyBoot.isEmpty
            ? Color(.systemGray4)
            : profile.groupColor
        return HStack(spacing: 6) {
            Circle().fill(status.color).frame(width: 8, height: 8)
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
                if let url = profile.url {
                    WebView(url: url,
                            reloadToken: reloadTokens[profile.id] ?? 0,
                            onStatus: { status in statuses[profile.id] = status })
                        .opacity(profile.id == store.activeId ? 1 : 0)
                        .allowsHitTesting(profile.id == store.activeId)
                }
            }
            if let message = errors[store.activeId], let profile = store.active {
                errorPanel(profile, message)
            }
        }
    }

    private func errorPanel(_ profile: ServerProfile, _ message: String) -> some View {
        VStack(spacing: 14) {
            Image(systemName: "wifi.exclamationmark").font(.system(size: 44))
            Text("连不上服务端").font(.title3.bold())
            Text("服务端：\(profile.label)\n地址：\(profile.host)\n\n\(message)")
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

    // MARK: - 动作

    private func reload(_ id: String) {
        statuses[id] = .loading
        errors[id] = nil
        reloadTokens[id, default: 0] += 1
    }

    private func reloadActive() {
        guard let profile = store.active else { return }
        reload(profile.id)
    }

    /// 全部重载 = 把没连上的也连上，并把在线的一起刷新
    private func reloadAll() {
        for profile in store.servers {
            reload(profile.id)
        }
    }

    /// 「启动时连接全部」：让每个服务端各起一个 WebView，同时在线
    private func connectAll() {
        for profile in store.servers where reloadTokens[profile.id] == nil {
            reload(profile.id)
        }
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

    @State private var group = ""
    @State private var name = ""
    @State private var host = ""
    @State private var token = ""
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
                    TextField("192.168.0.120:12300 或整条带 token 的地址", text: $host)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section("访问令牌 token") {
                    TextField("可留空，页面上也能填", text: $token)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section {
                    Text("在电脑端 AnyBoot 的「远程控制」里能看到地址和 token；手机要和电脑在同一个局域网。同一台电脑的多个 IP 填同一个分组，标签会带同色描边。")
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
                host = profile?.host ?? ""
                token = profile?.token ?? ""
            }
        }
        .interactiveDismissDisabled(firstRun)
    }

    private func save() {
        guard let normalized = ServerProfile.normalizeHost(host) else { return }
        var resolved = token.trimmedAnyBoot
        if resolved.isEmpty { resolved = ServerProfile.token(fromURL: host) }
        var result = profile ?? ServerProfile()
        result.group = group.trimmedAnyBoot
        result.name = name.trimmedAnyBoot.isEmpty ? normalized : name.trimmedAnyBoot
        result.host = normalized
        result.token = resolved
        onFinish(.save(result))
        dismiss()
    }
}

/// 服务端列表：切换 / 编辑 / 删除
struct ServerListSheet: View {

    @EnvironmentObject private var store: ServerStore
    @Environment(\.dismiss) private var dismiss

    @State private var editing: ServerProfile?
    @State private var showEditor = false

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
                            Text(profile.host).font(.caption).foregroundStyle(.secondary)
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
                        Button("删除", role: .destructive) { store.remove(profile.id) }
                        Button("编辑") {
                            editing = profile
                            showEditor = true
                        }
                    }
                }
            }
            .navigationTitle("服务端列表")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成") { dismiss() }
                }
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
