import SwiftUI
import Citadel
import Crypto

/// 服务器模块（跳板机管理）：服务器列表 + 模块内底部导航四页（会话/终端/密钥/文件）。
/// 导航作用域只在模块内部 —— 侧边栏「电脑」的主列表不受影响（client-todo F7）。
struct ServerModuleView: View {

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var jumps = JumpStore.shared

    enum Page { case sessions, terminal, keys, files }
    @State private var page: Page = .terminal
    @State private var search = ""
    @State private var expanded: Set<String> = []
    @State private var editing: JumpServer?
    @State private var showEditor = false
    @State private var notice: String?
    @State private var tofuAlert: TofuPrompt?

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                tabBar
                Divider()
                content
                Divider()
                bottomNav
            }
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("完成") { dismiss() }
                }
                ToolbarItem(placement: .principal) {
                    Text("服务器（SSH / Telnet 跳板机）").font(.headline)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        editing = JumpServer()
                        showEditor = true
                    } label: {
                        Image(systemName: "plus")
                    }
                }
            }
            .sheet(isPresented: $showEditor) {
                JumpEditorSheet(jump: editing) { result in
                    switch result {
                    case .save(let jump):
                        jumps.upsert(jump)
                    case .delete(let id):
                        SshSessionManager.shared.shutdown(jumpId: id)
                        TerminalSessionsRegistry.get(id)?.close("服务器已删除")
                        TerminalSessionsRegistry.remove(id)
                        jumps.remove(id)
                    case .test(let jump):
                        jumps.upsert(jump)
                        Task { await testConnection(jump) }
                    }
                }
                .onAppear {
                    SshSessionManager.shared.confirmHandler = { title, message in
                        await withCheckedContinuation { continuation in
                            tofuAlert = TofuPrompt(title: title, message: message,
                                                   continuation: continuation)
                        }
                    }
                }
            }
            .alert("连接结果", isPresented: Binding(
                get: { notice != nil },
                set: { if !$0 { notice = nil } })) {
                Button("好") { notice = nil }
            } message: {
                Text(notice ?? "")
            }
            .confirmationDialog("确定删除服务器吗？", isPresented: Binding(
                get: { deleting != nil },
                set: { if !$0 { deleting = nil } }),
                titleVisibility: .visible) {
                Button("删除", role: .destructive) {
                    if let jump = deleting {
                        let via = jumps.computersVia(jump.id)
                        SshSessionManager.shared.shutdown(jumpId: jump.id)
                        TerminalSessionsRegistry.get(jump.id)?.close("服务器已删除")
                        TerminalSessionsRegistry.remove(jump.id)
                        jumps.remove(jump.id)
                        notice = via > 0
                            ? "已删除。注意：有 \(via) 台电脑配置了经由它连接，会退回直连。"
                            : nil
                    }
                    deleting = nil
                }
                Button("取消", role: .cancel) { deleting = nil }
            } message: {
                Text(deleting.map { "「\($0.displayName)」"
                        + (jumps.computersVia($0.id) > 0
                            ? "（有 \(jumps.computersVia($0.id)) 台电脑经由它连接）" : "") } ?? "")
            }
            .sheet(isPresented: $showTerminal) {
                if let id = terminalJumpId, let session = TerminalSessionsRegistry.get(id) {
                    TerminalSheet(session: session)
                }
            }
            .alert(tofuAlert?.title ?? "", isPresented: Binding(
                get: { tofuAlert != nil },
                set: { if !$0 { tofuAlert?.resume(false) } })) {
                Button("信任并连接") { tofuAlert?.resume(true) }
                Button("取消", role: .cancel) { tofuAlert?.resume(false) }
            } message: {
                Text(tofuAlert?.message ?? "")
            }
            .onAppear {
                TerminalSessionsRegistry.setObserver { }
            }
        }
    }

    // MARK: - 底部导航（会话 / 终端 / 密钥 / 文件）

    private var bottomNav: some View {
        HStack {
            navButton("会话", page: .sessions)
            navButton("终端", page: .terminal)
            navButton("密钥", page: .keys)
            navButton("文件", page: .files)
        }
        .padding(.vertical, 4)
    }

    private func navButton(_ title: String, page target: Page) -> some View {
        Button {
            page = target
        } label: {
            Text(title)
                .font(.subheadline)
                .fontWeight(page == target ? .bold : .regular)
                .foregroundColor(page == target ? Color.accentColor : .secondary)
                .frame(maxWidth: .infinity)
        }
    }

    @ViewBuilder
    private var content: some View {
        switch page {
        case .sessions: sessionsPage
        case .terminal: serverListPage
        case .keys: keysPage
        case .files: FilesPage()
        }
    }

    private var tabBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                if page == .terminal {
                    TextField("搜索名称 / 地址", text: $search)
                        .textFieldStyle(.roundedBorder)
                        .frame(maxWidth: 260)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
    }

    // MARK: - 会话页

    private var sessionsPage: some View {
        List {
            Section("进行中") {
                let live = TerminalSessionsRegistry.all
                if live.isEmpty {
                    Text("没有进行中的会话。到「终端」页点「▶ 打开终端」开始。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                ForEach(live) { session in
                    HStack(spacing: 10) {
                        Circle().fill(Color.green).frame(width: 8, height: 8)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(session.server.displayName)
                            Text("已连接 \(Int(Date().timeIntervalSince(session.startedAt))) 秒")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("结束", role: .destructive) {
                            TerminalSessionsRegistry.kill(session)
                        }
                        .buttonStyle(.bordered)
                    }
                    .onTapGesture {
                        openTerminal(session.server)
                    }
                }
            }
            Section("历史") {
                let history = jumps.history.filter { $0.endedAt > $0.startedAt }
                if history.isEmpty {
                    Text("还没有会话历史。").font(.footnote).foregroundStyle(.secondary)
                }
                ForEach(history.prefix(20)) { entry in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(entry.serverName.isEmpty ? "服务器" : entry.serverName)
                        Text("\(entry.startedAt.formatted(date: .abbreviated, time: .shortened))"
                             + " · 时长 \(Int(entry.endedAt.timeIntervalSince(entry.startedAt) / 60)) 分钟")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
                Button("清空历史", role: .destructive) { jumps.clearHistory() }
            }
        }
    }

    // MARK: - 终端页（服务器列表）

    private var serverListPage: some View {
        List {
            if jumps.jumps.isEmpty {
                Text("还没有服务器。\n\nSSH 跳板机：外网连家里的必经之路 —— 手机先 SSH 登录它，"
                     + "再由它访问局域网内的 anyboot 电脑。\nTelnet：只有实验性终端。")
                    .font(.footnote).foregroundStyle(.secondary)
            }
            ForEach(jumps.jumps.filter { matches($0) }) { jump in
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 8) {
                        Text(String(jump.displayName.prefix(1)).uppercased())
                            .font(.headline).foregroundColor(.white)
                            .frame(width: 34, height: 34)
                            .background(Circle().fill(Color.accentColor))
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) {
                                Text(jump.displayName).font(.body)
                                Text(jump.protocolLabel)
                                    .font(.caption2)
                                    .foregroundColor(jump.isSsh ? Color.accentColor : .orange)
                                    .padding(.horizontal, 6)
                                    .padding(.vertical, 1)
                                    .background(Capsule().fill(jump.isSsh
                                            ? Color.accentColor.opacity(0.12)
                                            : Color.orange.opacity(0.12)))
                                if jump.lastRttMs >= 0 {
                                    Text("\(Int(jump.lastRttMs))ms")
                                        .font(.caption).foregroundColor(.green)
                                }
                            }
                            Text(jump.addressLabel).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if jump.isSsh {
                            Button("▶ 终端") { openTerminal(jump) }
                                .buttonStyle(.bordered)
                                .tint(.green)
                        }
                        Image(systemName: expanded.contains(jump.id) ? "chevron.down" : "chevron.right")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    .contentShape(Rectangle())
                    .onTapGesture {
                        if expanded.contains(jump.id) {
                            expanded.remove(jump.id)
                        } else {
                            expanded.insert(jump.id)
                        }
                    }
                    if expanded.contains(jump.id) {
                        VStack(alignment: .leading, spacing: 8) {
                            let userLabel = jump.username.trimmedAnyBoot.isEmpty
                                ? "（未填，连接时用 root）" : jump.username.trimmedAnyBoot
                            Text("端口 \(jump.port) · 用户 \(userLabel)"
                                 + " · 认证 \(jump.authType == JumpServer.authKey ? "密钥" : "密码")")
                                .font(.caption).foregroundStyle(.secondary)
                            if !jump.note.trimmedAnyBoot.isEmpty {
                                Text("备注：\(jump.note)").font(.caption).foregroundStyle(.secondary)
                            }
                            if jump.isSsh {
                                Text("经由此服务器的电脑：\(jumps.computersVia(jump.id)) 台")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            if SshSessionManager.shared.isUp(jump.id) {
                                Text("SSH 会话在线（隧道与终端共享）")
                                    .font(.caption).foregroundStyle(.green)
                            }
                            HStack {
                                if jump.isSsh {
                                    Button("▶ 打开终端") { openTerminal(jump) }
                                        .buttonStyle(.borderedProminent)
                                    Button("浏览文件") { page = .files }
                                        .buttonStyle(.bordered)
                                }
                                Button("测试连接") { Task { await testConnection(jump) } }
                                    .buttonStyle(.bordered)
                                Button("编辑") {
                                    editing = jump
                                    showEditor = true
                                }
                                .buttonStyle(.bordered)
                                Button("删除", role: .destructive) { confirmDelete(jump) }
                                    .buttonStyle(.bordered)
                            }
                            .font(.footnote)
                        }
                        .padding(.leading, 42)
                    }
                }
            }
        }
    }

    private func matches(_ jump: JumpServer) -> Bool {
        let query = search.trimmedAnyBoot.lowercased()
        guard !query.isEmpty else { return true }
        return jump.displayName.lowercased().contains(query)
            || jump.host.lowercased().contains(query)
    }

    private func openTerminal(_ jump: JumpServer) {
        if TerminalSessionsRegistry.get(jump.id) == nil {
            let session = TerminalSessionRef(server: jump)
            TerminalSessionsRegistry.put(session)
            Task {
                do {
                    if jump.isSsh {
                        try await session.startSsh()
                    } else {
                        try session.startTelnet()
                    }
                } catch {
                    session.close("连接失败：\(error.localizedDescription)")
                    notice = "连接失败：\(error.localizedDescription)"
                }
            }
        }
        terminalJumpId = jump.id
        showTerminal = true
    }

    @State private var terminalJumpId: String?
    @State private var showTerminal = false

    private func testConnection(_ jump: JumpServer) async {
        let started = Date()
        do {
            if jump.isSsh {
                _ = try await SshSessionManager.shared.client(for: jump)
            } else {
                // Telnet 只测 TCP 通：起会话 → 稍等 → 关
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                    let probe = TelnetSession()
                    probe.onClosed = { _ in }
                    DispatchQueue.global().async {
                        do {
                            try probe.start(jump: jump)
                            Thread.sleep(forTimeInterval: 0.6)
                            probe.close("测试完成")
                            continuation.resume()
                        } catch {
                            continuation.resume(throwing: error)
                        }
                    }
                }
            }
            var updated = jump
            updated.lastRttMs = Date().timeIntervalSince(started) * 1000
            updated.lastSeen = Date()
            jumps.upsert(updated)
            notice = "连接成功（\(Int(updated.lastRttMs))ms）"
        } catch {
            notice = "连接失败：\(error.localizedDescription)"
        }
    }

    @State private var deleting: JumpServer?

    private func confirmDelete(_ jump: JumpServer) {
        deleting = jump
    }

    private var deleteDialog: some View {
        Group { EmptyView() }
    }

    // MARK: - 密钥页

    private var keysPage: some View {
        List {
            Section("密钥（私钥只存本机 Keychain）") {
                if jumps.keys.isEmpty {
                    Text("还没有密钥。导入已有的私钥，或直接在本机生成一对 —— "
                         + "把公钥放到服务器上（authorized_keys）就能免密登录。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                ForEach(jumps.keys) { key in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(key.name.isEmpty ? "未命名密钥" : key.name)
                        Text("\(key.algorithm) · \(key.fingerprint)"
                             + " · 关联 \(jumps.keysInUse(key.id)) 台服务器")
                            .font(.caption).foregroundStyle(.secondary)
                        HStack {
                            Button("复制公钥") {
                                UIPasteboard.general.string = key.publicKey
                            }
                            .font(.footnote)
                            Button("删除", role: .destructive) {
                                if jumps.keysInUse(key.id) > 0 {
                                    notice = "有服务器在用这把密钥，先改它们的认证方式"
                                    return
                                }
                                KeyVault.delete(account: key.privateRef)
                                jumps.removeKey(key.id)
                            }
                            .font(.footnote)
                        }
                    }
                }
            }
            Section {
                Button("导入私钥文件") { showImporter = true }
                Button("在本机生成密钥对") { showGenerator = true }
            }
        }
        .fileImporter(isPresented: $showImporter, allowedContentTypes: [.data]) { result in
            importKey(result)
        }
        .alert("生成密钥对", isPresented: $showGenerator) {
            TextField("名称，如 home-ed25519", text: $generatedKeyName)
            Button("生成（Ed25519）") { generateKey(ed25519: true) }
            Button("生成（RSA 4096）") { generateKey(ed25519: false) }
            Button("取消", role: .cancel) { }
        }
    }

    @State private var showImporter = false
    @State private var showGenerator = false
    @State private var generatedKeyName = ""

    private func importKey(_ result: Result<URL, Error>) {
        guard case .success(let url) = result else { return }
        let secured = url.startAccessingSecurityScopedResource()
        defer { if secured { url.stopAccessingSecurityScopedResource() } }
        do {
            let pem = try String(contentsOf: url, encoding: .utf8).trimmedAnyBoot
            guard pem.hasPrefix("-----BEGIN") else {
                notice = "不像 PEM 格式的私钥（应以 -----BEGIN 开头）"
                return
            }
            let entry = SshKeyEntry()
            entry.name = url.lastPathComponent
            entry.privateRef = "key-\(entry.id)"
            KeyVault.save(pem, account: entry.privateRef)
            if pem.contains("ssh-ed25519") {
                entry.algorithm = "ssh-ed25519"
                let privateKey = try Curve25519.Signing.PrivateKey(sshEd25519: pem)
                let publicData = privateKey.publicKey.rawRepresentation
                entry.fingerprint = KeyVault.sha256Fingerprint(publicData)
                entry.publicKey = "ssh-ed25519 \(Data(publicData).base64EncodedString())"
            } else {
                entry.algorithm = "ssh-rsa"
                let privateKey = try Insecure.RSA.PrivateKey(sshRsa: pem)
                let sshPublic = privateKey.sshPublicKeyData   // Citadel 提供
                entry.fingerprint = KeyVault.sha256Fingerprint(sshPublic)
                entry.publicKey = "ssh-rsa \(sshPublic.base64EncodedString())"
            }
            jumps.upsert(entry)
            notice = "已导入 \(entry.algorithm) 密钥"
        } catch {
            notice = "导入失败：\(error.localizedDescription)"
        }
    }

    private func generateKey(ed25519: Bool) {
        guard ed25519 else {
            // RSA 生成的 OpenSSH 导出依赖 Citadel 的序列化细节，一期只支持
            // Ed25519 生成（推荐）；RSA 走「导入私钥文件」。
            notice = "RSA 请用「导入私钥文件」；本机生成只支持推荐的 Ed25519"
            return
        }
        let name = generatedKeyName.trimmedAnyBoot.isEmpty
            ? "key-\(jumps.keys.count)" : generatedKeyName
        let privateKey = Curve25519.Signing.PrivateKey()
        let entry = SshKeyEntry()
        entry.name = name
        entry.privateRef = "key-\(entry.id)"
        entry.algorithm = "ssh-ed25519"
        // OpenSSH 双格式导出（seed 32B → 私钥 PEM；公钥 → 一行 authorized_keys 格式）
        let openSSH = privateKey.ed25519OpenSSHPrivateKey(comment: name)
        entry.publicKey = privateKey.ed25519OpenSSHPublicKey(comment: name)
        entry.fingerprint = privateKey.ed25519Fingerprint
        KeyVault.save(openSSH, account: entry.privateRef)
        jumps.upsert(entry)
        generatedKeyName = ""
        notice = "已生成 \(entry.algorithm) 密钥「\(name)」—— 复制公钥放到服务器的 authorized_keys"
    }
}

/// 终端多会话宿主：标签条 + 单个终端视图切换（会话 sheet / 会话页共用）
struct TerminalSessionHost: View {

    var initialId: String = ""
    @State private var selected = ""

    var body: some View {
        VStack(spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(TerminalSessionsRegistry.all) { session in
                        Text(session.server.displayName)
                            .font(.caption)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 5)
                            .background(Capsule().fill(session.server.id == selected
                                    ? Color.accentColor : Color(.tertiarySystemFill)))
                            .foregroundColor(session.server.id == selected ? .white : .primary)
                            .onTapGesture { selected = session.server.id }
                            .onLongPressGesture { TerminalSessionsRegistry.kill(session) }
                    }
                }
                .padding(8)
            }
            if let session = TerminalSessionsRegistry.get(selected) {
                TerminalView(session: session)
            } else {
                Spacer()
            }
        }
        .onAppear {
            if selected.isEmpty {
                selected = initialId.isEmpty
                    ? TerminalSessionsRegistry.all.first?.server.id ?? "" : initialId
            }
        }
    }
}
