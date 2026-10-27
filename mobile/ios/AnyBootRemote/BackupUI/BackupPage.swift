import SwiftUI
import UniformTypeIdentifiers

// MARK: - 系统分享面板（L3 导出完成后弹出）

struct ShareSheet: UIViewControllerRepresentable {
    let items: [Any]
    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }
    func updateUIViewController(_ vc: UIActivityViewController, context: Context) {}
}

// MARK: - S1 备份与恢复主页（docs/backup-ui-design.md §2 S1，L1-L7）
//
// 三个分区：导出（分类勾选 + 口令 + 主按钮）/ 多端同步 / 导入入口。
// S2 WebDAV 配置与 S3 导入预览以 fullScreenCover 挂在本页之上。

struct BackupPage: View {

    @ObservedObject private var nav = ABNavigator.shared
    @ObservedObject private var sync = SyncStore.shared

    // 导出区状态
    @State private var selected: Set<BackupCategory> = [.servers, .jumps, .terminal]
    @State private var password = ""
    @State private var passwordAgain = ""
    @State private var showPassword = false
    @State private var exporting = false
    @State private var shareURL: URL?

    // 导入区状态
    @State private var showImporter = false
    @State private var importRaw: Data?
    @State private var importNeedsPassword = false

    // 同步区状态
    @State private var syncing = false
    @State private var confirmSyncOff = false

    var body: some View {
        VStack(spacing: 0) {
            navBar
            ABHairline()
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    exportSection
                    syncSection
                    importSection
                }
                .padding(.horizontal, ABMetric.px(16))
                .padding(.bottom, ABMetric.px(32))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(ABColor.base)
        // S2 WebDAV 配置
        .fullScreenCover(isPresented: $showWebDav) { BackupWebDavView() }
        // S3 导入预览（解密成功后进入）
        .fullScreenCover(isPresented: $showPreview) {
            if let raw = importRaw, let envelope = previewEnvelope {
                BackupImportPreview(raw: raw, envelope: envelope)
            }
        }
        // L15 加密文件口令页（独立模态，不可点外关闭）
        .sheet(isPresented: $importNeedsPassword) {
            BackupPasswordSheet(
                title: "输入备份口令",
                verify: { pwd in
                    guard let raw = importRaw else { return .bad("文件已失效，请重新选择") }
                    do {
                        return .ok(try BackupStore.shared.decrypt(raw: raw, password: pwd))
                    } catch {
                        return .bad((error as? BackupError)?.errorDescription ?? "口令错误或文件已损坏")
                    }
                },
                onSuccess: { envelope in
                    previewEnvelope = envelope
                    importNeedsPassword = false
                    showPreview = true
                })
            .interactiveDismissDisabled()
        }
        // L3 系统分享面板
        .sheet(isPresented: $showShare) {
            if let url = shareURL {
                ShareSheet(items: [url])
            }
        }
        // L4 关闭同步确认（S6a）
        .confirmationDialog("关闭多端同步？", isPresented: $confirmSyncOff, titleVisibility: .visible) {
            Button("关闭同步", role: .destructive) {
                sync.enabled = false
                sync.resetTombstones()
                SyncEngine.shared.stopAll()
                ABToastBus.shared.show("已关闭同步，云端文件已保留")
            }
            Button("取消", role: .cancel) { }
        } message: {
            Text("关闭后本设备将停止自动同步，已上传的云文件保留。")
        }
        // L7 文件选择器（.anyboot / .json，data 兜底）
        .fileImporter(isPresented: $showImporter,
                      allowedContentTypes: [UTType.anyboot, .json, .data]) { result in
            handlePicked(result)
        }
    }

    @State private var showWebDav = false
    @State private var showPreview = false
    @State private var showShare = false
    @State private var previewEnvelope: BackupEnvelope?

    // MARK: - 导航栏（44pt，‹ 44×44 命中区）

    private var navBar: some View {
        HStack(spacing: 0) {
            ABBarIcon(text: "‹", description: "返回") { nav.back() }
            Text("备份与恢复")
                .font(.system(size: 16, weight: .bold))
                .foregroundColor(ABColor.fg)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, ABMetric.px(8))
        .frame(height: ABMetric.px(44))
    }

    // MARK: - 导出区

    private var exportSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            ABSectionTitle("导出")
            VStack(spacing: 0) {
                ForEach(BackupCategory.allCases) { cat in
                    categoryRow(cat)
                    if cat != BackupCategory.allCases.last {
                        ABListDivider()
                    }
                }
                infoBar
                if selected.contains(.keys) {
                    passwordArea
                }
                exportButton
                    .padding(ABMetric.px(12))
            }
            .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.card))
        }
    }

    /// L1 分类行：点整行切换勾选；密钥行 ⚠ +「高」徽标
    private func categoryRow(_ cat: BackupCategory) -> some View {
        let checked = selected.contains(cat)
        return Button {
            toggle(cat)
        } label: {
            HStack(spacing: ABMetric.px(10)) {
                checkbox(checked)
                VStack(alignment: .leading, spacing: ABMetric.px(1)) {
                    HStack(spacing: ABMetric.px(6)) {
                        Text(cat.label)
                            .font(.system(size: 14))
                            .foregroundColor(ABColor.fg)
                        if cat == .keys {
                            Text("⚠ 高风险")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundColor(ABColor.warn)
                        }
                    }
                    Text(subtitle(cat))
                        .font(.system(size: 11))
                        .foregroundColor(ABColor.fgFaint)
                }
                Spacer(minLength: 0)
                Text("（\(count(cat))）")
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.fgDim)
            }
            .padding(.horizontal, ABMetric.px(12))
            .frame(minHeight: ABMetric.px(48))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(cat.label)，\(checked ? "已勾选" : "未勾选")")
    }

    private func toggle(_ cat: BackupCategory) {
        withAnimation(.easeInOut(duration: 0.2)) {
            if selected.contains(cat) {
                selected.remove(cat)
            } else {
                selected.insert(cat)
            }
        }
    }

    private func subtitle(_ cat: BackupCategory) -> String {
        switch cat {
        case .servers: return "电脑档案、配对通道、电源路由"
        case .jumps: return "地址、账号；登录密码不在本文件内" + (selected.contains(.keys) ? "" : "")
        case .keys: return "私钥将以明文写入备份，务必设置口令"
        case .terminal: return "终端外观、传输方式等本机偏好"
        case .history: return "会话记录与已信任的主机指纹"
        }
    }

    private func count(_ cat: BackupCategory) -> Int {
        switch cat {
        case .servers: return ServerStore.shared.servers.count
        case .jumps: return JumpStore.shared.jumps.count
        case .keys: return JumpStore.shared.keys.count
        case .terminal: return 1
        case .history: return JumpStore.shared.history.count
        }
    }

    private func checkbox(_ checked: Bool) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: ABMetric.px(6), style: .continuous)
                .fill(checked ? ABColor.accent : ABColor.base)
                .frame(width: ABMetric.px(22), height: ABMetric.px(22))
                .overlay(RoundedRectangle(cornerRadius: ABMetric.px(6), style: .continuous)
                    .strokeBorder(checked ? ABColor.accent : ABColor.lineStrong, lineWidth: 1.5))
            if checked {
                DrawnIcon.check(ABColor.onAccent, size: ABMetric.px(14))
            }
        }
    }

    /// L1 info 条：跳板机引用缺失提示；勾密钥且无口令 → 黄条 + 主按钮禁用
    @ViewBuilder
    private var infoBar: some View {
        let missing = missingJumpRefCount
        let needPassword = selected.contains(.keys) && !passwordValid
        if missing > 0 || needPassword {
            let text = needPassword
                ? "含私钥的备份必须设置口令加密"
                : "\(missing) 台电脑关联的跳板机未包含在本次导出"
            HStack(spacing: ABMetric.px(6)) {
                Text(needPassword ? "⚠" : "ℹ")
                    .font(.system(size: 12, weight: .bold))
                    .foregroundColor(needPassword ? ABColor.warn : ABColor.fgDim)
                Text(text)
                    .font(.system(size: 12))
                    .foregroundColor(needPassword ? ABColor.warn : ABColor.fgDim)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, ABMetric.px(12))
            .padding(.vertical, ABMetric.px(8))
            .background(RoundedRectangle(cornerRadius: ABMetric.px(8), style: .continuous)
                .fill(needPassword ? ABColor.warn.opacity(0.12) : ABColor.elevate))
            .padding(.horizontal, ABMetric.px(12))
            .padding(.top, ABMetric.px(8))
        }
    }

    private var missingJumpRefCount: Int {
        let jumpIds = Set(JumpStore.shared.jumps.map(\.id))
        return ServerStore.shared.servers.filter {
            !$0.jumpServerId.isEmpty && !jumpIds.contains($0.jumpServerId)
        }.count
    }

    // ---- L2 口令区 ----

    private var passwordValid: Bool {
        password.count >= 6 && password == passwordAgain
    }

    private var passwordArea: some View {
        VStack(alignment: .leading, spacing: ABMetric.px(6)) {
            HStack {
                ABFieldLabel("备份口令（≥6 位）")
                Spacer()
                Button {
                    showPassword.toggle()
                } label: {
                    Text(showPassword ? "隐藏" : "显示")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.accent)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("切换口令明文")
            }
            Group {
                if showPassword {
                    TextField("输入口令", text: $password)
                } else {
                    SecureField("输入口令", text: $password)
                }
            }
            .font(.system(size: 13))
            .foregroundColor(ABColor.fg)
            .padding(.horizontal, ABMetric.px(12))
            .padding(.vertical, ABMetric.px(11))
            .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.base))
            .overlay(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .strokeBorder(mismatch ? ABColor.bad : ABColor.lineStrong, lineWidth: ABMetric.hairline))
            Group {
                if showPassword {
                    TextField("再次输入口令", text: $passwordAgain)
                } else {
                    SecureField("再次输入口令", text: $passwordAgain)
                }
            }
            .font(.system(size: 13))
            .foregroundColor(ABColor.fg)
            .padding(.horizontal, ABMetric.px(12))
            .padding(.vertical, ABMetric.px(11))
            .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.base))
            .overlay(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .strokeBorder(mismatch ? ABColor.bad : ABColor.lineStrong, lineWidth: ABMetric.hairline))
            if mismatch {
                Text("两次输入不一致")
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.bad)
            }
        }
        .padding(.horizontal, ABMetric.px(12))
        .padding(.top, ABMetric.px(10))
    }

    private var mismatch: Bool {
        !passwordAgain.isEmpty && password != passwordAgain
    }

    // ---- L3 主按钮 ----

    private var exportDisabled: Bool {
        selected.isEmpty || (selected.contains(.keys) && !passwordValid) || exporting
    }

    private var exportButton: some View {
        Button {
            exportNow()
        } label: {
            HStack(spacing: ABMetric.px(8)) {
                if exporting { ProgressView().tint(ABColor.onAccent) }
                Text("导出到文件")
                    .font(.system(size: 15, weight: .semibold))
            }
            .foregroundColor(exportDisabled ? ABColor.fgFaint : ABColor.onAccent)
            .frame(maxWidth: .infinity)
            .frame(height: ABMetric.px(48))
            .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(exportDisabled ? ABColor.elevate : ABColor.accent))
        }
        .buttonStyle(.plain)
        .disabled(exportDisabled)
        .accessibilityLabel("导出到文件")
    }

    private func exportNow() {
        exporting = true
        let cats = selected
        let pwd = selected.contains(.keys) ? password : nil
        Task { @MainActor in
            defer { exporting = false }
            do {
                let envelope = try BackupStore.shared.collect(categories: cats)
                // 密钥材料读取失败计数（导出报告 L3）
                var keySkipped = 0
                if cats.contains(.keys), let keys = envelope.data?.keys {
                    keySkipped = max(0, keys.items.count - keys.secrets.count)
                }
                let data = try BackupStore.shared.serialize(envelope, password: pwd)
                let stamp = Self.fileStamp()
                let url = FileManager.default.temporaryDirectory
                    .appendingPathComponent("anyboot-backup-\(stamp).anyboot")
                try data.write(to: url)
                shareURL = url
                showShare = true
                ABToastBus.shared.show("已导出 \(cats.count) 类数据\(keySkipped > 0 ? "（密钥 \(keySkipped) 把读取失败已跳过）" : "")")
            } catch {
                ABToastBus.shared.show("导出失败：\((error as? BackupError)?.errorDescription ?? (error as NSError).localizedDescription)")
            }
        }
    }

    private static func fileStamp() -> String {
        let f = DateFormatter()
        f.dateFormat = "yyyyMMdd-HHmmss"
        return f.string(from: Date())
    }

    // MARK: - 多端同步区

    private var syncSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            ABSectionTitle("多端同步")
            VStack(spacing: 0) {
                // L4 启用开关
                Button {
                    if sync.enabled {
                        confirmSyncOff = true
                    } else if !sync.config.isConfigured {
                        showWebDav = true
                        ABToastBus.shared.show("请先配置 WebDAV 服务器和同步内容")
                    } else {
                        sync.enabled = true
                        SyncEngine.shared.restartPeriodic()
                        ABToastBus.shared.show("已开启多端同步")
                    }
                } label: {
                    HStack {
                        VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                            Text("启用多端同步")
                                .font(.system(size: 14))
                                .foregroundColor(ABColor.fg)
                            Text("通过 WebDAV 在多台设备间保持数据一致")
                                .font(.system(size: 12))
                                .foregroundColor(ABColor.fgFaint)
                        }
                        Spacer(minLength: 0)
                        Toggle("", isOn: .init(get: { sync.enabled }, set: { _ in }))
                            .labelsHidden()
                            .disabled(true)   // 点击由整行接管（含确认弹窗逻辑）
                    }
                    .padding(.horizontal, ABMetric.px(12))
                    .frame(minHeight: ABMetric.px(52))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                ABListDivider()
                // L5 WebDAV 配置行
                Button {
                    showWebDav = true
                } label: {
                    HStack {
                        VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                            Text("WebDAV 与同步内容")
                                .font(.system(size: 14))
                                .foregroundColor(ABColor.fg)
                            Text(sync.config.isConfigured ? Self.hostLabel(sync.config.serverURL) : "未配置")
                                .font(.system(size: 12))
                                .foregroundColor(ABColor.fgFaint)
                        }
                        Spacer(minLength: 0)
                        if sync.config.isConfigured {
                            Circle().fill(ABColor.ok).frame(width: 7, height: 7)
                        }
                        DrawnIcon.chevronRight(ABColor.fgFaint, size: ABMetric.px(16))
                    }
                    .padding(.horizontal, ABMetric.px(12))
                    .frame(minHeight: ABMetric.px(52))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                ABListDivider()
                // L6 立即同步行
                Button {
                    syncNowTapped()
                } label: {
                    HStack {
                        Text("立即同步")
                            .font(.system(size: 14))
                            .foregroundColor(sync.config.isConfigured ? ABColor.fg : ABColor.fgFaint)
                        Spacer(minLength: 0)
                        if syncing {
                            ProgressView().tint(ABColor.accent)
                            Text("同步中…")
                                .font(.system(size: 12))
                                .foregroundColor(ABColor.fgDim)
                        } else {
                            Text(statusLabel)
                                .font(.system(size: 12))
                                .foregroundColor(statusColor)
                        }
                    }
                    .padding(.horizontal, ABMetric.px(12))
                    .frame(minHeight: ABMetric.px(52))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(!sync.config.isConfigured || syncing)
                .opacity(sync.config.isConfigured ? 1 : 0.4)
                .accessibilityHint(sync.config.isConfigured ? "" : "请先配置 WebDAV 服务器和同步内容")
            }
            .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.card))
        }
    }

    private var syncingNow: Bool {
        if case .running = sync.status { return true }
        return false
    }

    private var statusLabel: String {
        if syncingNow { return "同步中…" }
        switch sync.status {
        case .idle:
            if let last = sync.lastSyncAt { return "上次同步 \(SyncStatus.timeText(last))" }
            return "未同步过"
        case .running: return "同步中…"
        case .success(let date): return "上次同步 \(SyncStatus.timeText(date)) · 成功"
        case .failed(let reason): return "同步失败：\(reason)"
        }
    }

    private var statusColor: Color {
        switch sync.status {
        case .success: return ABColor.ok
        case .failed: return ABColor.bad
        default: return ABColor.fgDim
        }
    }

    private func syncNowTapped() {
        syncing = true
        Task { @MainActor in
            let reason = await SyncEngine.shared.syncNow()
            syncing = false
            if let reason {
                ABToastBus.shared.show("同步失败：\(reason)", seconds: 3.5)
            }
        }
    }

    private static func hostLabel(_ url: String) -> String {
        URL(string: url)?.host ?? url
    }

    // MARK: - 导入区

    private var importSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            ABSectionTitle("导入")
            VStack(spacing: ABMetric.px(10)) {
                Button {
                    showImporter = true
                } label: {
                    Text("从文件导入")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(ABColor.accent)
                        .frame(maxWidth: .infinity)
                        .frame(height: ABMetric.px(48))
                        .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .strokeBorder(ABColor.accent, lineWidth: 1.5))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("从文件导入备份")
                Text("支持 .anyboot 备份文件")
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.fgFaint)
                    .frame(maxWidth: .infinity)
            }
            .padding(ABMetric.px(12))
            .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.card))
        }
    }

    private func handlePicked(_ result: Result<URL, Error>) {
        guard case .success(let url) = result else { return }
        let secured = url.startAccessingSecurityScopedResource()
        defer { if secured { url.stopAccessingSecurityScopedResource() } }
        do {
            importRaw = try Data(contentsOf: url)
            if BackupStore.shared.peekEncryption(raw: importRaw!) != nil {
                importNeedsPassword = true
            } else {
                previewEnvelope = try BackupStore.shared.parse(raw: importRaw!)
                showPreview = true
            }
        } catch {
            ABToastBus.shared.show((error as? BackupError)?.errorDescription ?? "读取文件失败")
        }
    }
}
