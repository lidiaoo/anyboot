import SwiftUI
import UniformTypeIdentifiers

// MARK: - BackupData 分类存在性（S3 默认勾选依据）

extension BackupData {
    func has(_ cat: BackupCategory) -> Bool {
        switch cat {
        case .servers: return servers != nil
        case .jumps: return jumps != nil
        case .keys: return keys != nil
        case .terminal: return terminal != nil
        case .history: return history != nil
        }
    }
}

// MARK: - 备份文件 UTType（.anyboot / .json 兜底 .data）

extension UTType {
    static let anyboot = UTType(filenameExtension: "anyboot") ?? .data
}

// MARK: - S5 备份口令弹窗（L15：错 3 次追加「请确认文件来源」）

/// verify 的结果（避免 String 拼 Result 的协议问题）
enum BackupEnvelopeVerify {
    case ok(BackupEnvelope)
    case bad(String)
}

struct BackupPasswordSheet: View {

    let title: String
    /// 返回 .bad(原因) 则留在弹窗内显示错误
    let verify: (String) -> BackupEnvelopeVerify
    let onSuccess: (BackupEnvelope) -> Void

    @State private var password = ""
    @State private var showPassword = false
    @State private var error: String?
    @State private var attempts = 0
    @State private var verifying = false
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: ABMetric.px(12)) {
            Text(title)
                .font(.system(size: 16, weight: .bold))
                .foregroundColor(ABColor.fg)
                .padding(.top, ABMetric.px(20))
            HStack(spacing: ABMetric.px(8)) {
                Group {
                    if showPassword {
                        TextField("口令", text: $password)
                    } else {
                        SecureField("口令", text: $password)
                    }
                }
                .focused($focused)
                .font(.system(size: 13))
                .foregroundColor(ABColor.fg)
                .padding(.horizontal, ABMetric.px(12))
                .padding(.vertical, ABMetric.px(11))
                .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                    .fill(ABColor.base))
                .overlay(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                    .strokeBorder(error != nil ? ABColor.bad : ABColor.lineStrong, lineWidth: ABMetric.hairline))
                .submitLabel(.go)
                .onSubmit(confirm)
                Button {
                    showPassword.toggle()
                } label: {
                    Text(showPassword ? "隐藏" : "显示")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.accent)
                        .frame(minWidth: ABMetric.px(32), minHeight: ABMetric.px(44))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("切换口令明文")
            }
            if let error {
                Text(error + (attempts >= 3 ? "，请确认文件来源" : ""))
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.bad)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: ABMetric.px(8)) {
                Button {
                    password = ""
                    error = nil
                    dismiss()
                } label: {
                    Text("取消")
                        .font(.system(size: 14))
                        .foregroundColor(ABColor.fgDim)
                        .frame(maxWidth: .infinity)
                        .frame(height: ABMetric.px(44))
                        .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .fill(ABColor.elevate))
                }
                .buttonStyle(.plain)
                Button {
                    confirm()
                } label: {
                    HStack(spacing: ABMetric.px(6)) {
                        if verifying { ProgressView().tint(ABColor.onAccent) }
                        Text("确认")
                            .font(.system(size: 14, weight: .semibold))
                    }
                    .foregroundColor(password.isEmpty ? ABColor.fgFaint : ABColor.onAccent)
                    .frame(maxWidth: .infinity)
                    .frame(height: ABMetric.px(44))
                    .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(password.isEmpty ? ABColor.elevate : ABColor.accent))
                }
                .buttonStyle(.plain)
                .disabled(password.isEmpty || verifying)
            }
        }
        .padding(.horizontal, ABMetric.px(20))
        .padding(.bottom, ABMetric.px(20))
        .frame(maxWidth: .infinity)
        .background(ABColor.card)
        .onAppear { focused = true }
    }

    private func confirm() {
        guard !password.isEmpty, !verifying else { return }
        verifying = true
        let pwd = password
        Task { @MainActor in
            // PBKDF2 31 万轮在低端机上可能超过 300ms：最少给一次转圈
            try? await Task.sleep(nanoseconds: 350_000_000)
            switch verify(pwd) {
            case .ok(let envelope):
                onSuccess(envelope)
            case .bad(let reason):
                attempts += 1
                error = reason
            }
            verifying = false
        }
    }

    @Environment(\.dismiss) private var dismiss
}

// MARK: - S3 导入摘要预览（L11-L14）

struct BackupImportPreview: View {

    let raw: Data
    let envelope: BackupEnvelope

    @Environment(\.dismiss) private var dismiss
    @State private var selected: Set<BackupCategory> = []
    @State private var strategy: BackupStrategy = BackupStrategy(
        rawValue: UserDefaults.standard.string(forKey: "backup_import_strategy") ?? "") ?? .skipExisting
    @State private var expanded: Set<BackupCategory> = []
    @State private var importing = false
    @State private var report: ImportReport?
    @State private var confirmOverwrite = false
    @State private var currentCategory: BackupCategory?

    var body: some View {
        let sum = BackupStore.shared.summarize(envelope)
        VStack(spacing: 0) {
            navBar
            ABHairline()
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    sourceRow(sum)
                    categoryList(sum)
                    strategyCard
                    warningBar
                }
                .padding(.horizontal, ABMetric.px(16))
                .padding(.bottom, ABMetric.px(140))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(ABColor.base)
        .overlay(alignment: .bottom) { bottomBar(sum) }
        // S6b 覆盖导入确认
        .confirmationDialog("覆盖导入", isPresented: $confirmOverwrite, titleVisibility: .visible) {
            Button("覆盖导入", role: .destructive) { runImport() }
            Button("取消", role: .cancel) { }
        } message: {
            Text(overwriteMessage(sum))
        }
        // S4 结果报告
        .fullScreenCover(isPresented: .init(get: { report != nil }, set: { if !$0 { report = nil } })) {
            if let report {
                BackupImportReport(report: report)
            }
        }
        .onAppear {
            // 默认勾选 = 备份里带了数据的分类
            if selected.isEmpty {
                for cat in BackupCategory.allCases where envelope.data?.has(cat) == true {
                    selected.insert(cat)
                }
            }
        }
    }

    private var navBar: some View {
        HStack(spacing: 0) {
            ABBarIcon(text: "‹", description: "取消") { dismiss() }
            Text("导入预览")
                .font(.system(size: 16, weight: .bold))
                .foregroundColor(ABColor.fg)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, ABMetric.px(8))
        .frame(height: ABMetric.px(44))
    }

    /// 来源行
    private func sourceRow(_ sum: BackupSummary) -> some View {
        HStack(spacing: ABMetric.px(10)) {
            DrawnIcon.docFile(ABColor.accent, size: ABMetric.px(22))
            VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                Text("来自 \(Self.platformName(sum.platform))")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(ABColor.fg)
                Text("\(Self.dateText(sum.exportedAt)) · AnyBoot \(sum.app.isEmpty ? "?" : sum.app)")
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.fgDim)
            }
            Spacer(minLength: 0)
        }
        .padding(ABMetric.px(12))
        .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
            .fill(ABColor.card))
        .padding(.top, ABMetric.px(12))
    }

    /// 分类清单（L11 勾选 / L12 明细展开）
    private func categoryList(_ sum: BackupSummary) -> some View {
        VStack(spacing: 0) {
            ForEach(BackupCategory.allCases.filter { envelope.data?.has($0) == true }) { cat in
                if let row = sum.rows[cat] {
                    rowView(cat, row)
                    ABListDivider()
                }
            }
        }
        .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
            .fill(ABColor.card))
        .padding(.top, ABMetric.px(12))
    }

    private func rowView(_ cat: BackupCategory, _ row: BackupSummary.CategorySummary) -> some View {
        VStack(spacing: 0) {
            Button {
                toggle(cat)
            } label: {
                HStack(spacing: ABMetric.px(10)) {
                    Image(systemName: selected.contains(cat) ? "checkmark.square.fill" : "square")
                        .font(.system(size: 18))
                        .foregroundColor(selected.contains(cat) ? ABColor.accent : ABColor.fgFaint)
                    Text(cat.label)
                        .font(.system(size: 14))
                        .foregroundColor(ABColor.fg)
                    Spacer(minLength: 0)
                    Text("备份 \(row.backupCount) 条 · 本机 \(row.localCount) 条")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.fgDim)
                    if !row.conflicts.isEmpty {
                        DrawnIcon.chevronDown(expanded.contains(cat) ? ABColor.fgDim : ABColor.fgFaint,
                                              size: ABMetric.px(14))
                            .rotationEffect(.degrees(expanded.contains(cat) ? 0 : -90))
                    }
                }
                .padding(.horizontal, ABMetric.px(12))
                .frame(minHeight: ABMetric.px(56))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if expanded.contains(cat), !row.conflicts.isEmpty {
                let shown = row.conflicts.prefix(20)
                VStack(alignment: .leading, spacing: ABMetric.px(3)) {
                    Text("新增 \(row.backupCount - row.conflicts.count) · 覆盖 \(row.conflicts.count)："
                         + shown.joined(separator: "、")
                         + (row.conflicts.count > 20 ? " 等 \(row.conflicts.count) 条" : ""))
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.fgDim)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, ABMetric.px(40))
                .padding(.bottom, ABMetric.px(10))
            }
            if cat == .terminal {
                Text("将覆盖本机当前设置")
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.leading, ABMetric.px(40))
                    .padding(.bottom, ABMetric.px(8))
            }
        }
    }

    private func toggle(_ cat: BackupCategory) {
        withAnimation(.easeInOut(duration: 0.15)) {
            if selected.contains(cat) {
                selected.remove(cat)
            } else {
                selected.insert(cat)
            }
        }
    }

    /// L13 合并策略
    private var strategyCard: some View {
        VStack(alignment: .leading, spacing: ABMetric.px(8)) {
            ABFieldLabel("合并策略")
            ForEach([BackupStrategy.skipExisting, .overwrite]) { s in
                Button {
                    strategy = s
                    UserDefaults.standard.set(s.rawValue, forKey: "backup_import_strategy")
                } label: {
                    HStack(spacing: ABMetric.px(10)) {
                        Circle()
                            .fill(strategy == s ? ABColor.accent : ABColor.base)
                            .frame(width: ABMetric.px(16), height: ABMetric.px(16))
                            .overlay(Circle().strokeBorder(strategy == s ? ABColor.accent : ABColor.lineStrong, lineWidth: 1.5))
                            .overlay {
                                if strategy == s {
                                    Circle().fill(ABColor.onAccent).frame(width: 5, height: 5)
                                }
                            }
                        Text(s.label)
                            .font(.system(size: 14))
                            .foregroundColor(ABColor.fg)
                        Spacer(minLength: 0)
                    }
                    .frame(minHeight: ABMetric.px(40))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("合并策略：\(s.label)")
            }
            if strategy == .overwrite {
                Text("本机所选分类现有数据将被替换")
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.bad)
            }
        }
        .padding(ABMetric.px(12))
        .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
            .fill(ABColor.card))
        .padding(.top, ABMetric.px(12))
    }

    /// 警示条：跳板机引用缺失
    @ViewBuilder
    private var warningBar: some View {
        let sum = BackupStore.shared.summarize(envelope)
        if sum.missingJumpRefs > 0 {
            HStack(spacing: ABMetric.px(6)) {
                Text("⚠").font(.system(size: 12, weight: .bold)).foregroundColor(ABColor.warn)
                Text("\(sum.missingJumpRefs) 台电脑关联的跳板机不在本备份中")
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.warn)
                Spacer(minLength: 0)
            }
            .padding(ABMetric.px(10))
            .background(RoundedRectangle(cornerRadius: ABMetric.px(8), style: .continuous)
                .fill(ABColor.warn.opacity(0.12)))
            .padding(.top, ABMetric.px(12))
        }
    }

    /// 底部固定双钮（L14）
    private func bottomBar(_ sum: BackupSummary) -> some View {
        VStack(spacing: 0) {
            if importing, let current = currentCategory {
                Text("正在导入 \(current.label)…")
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.fgDim)
                    .frame(maxWidth: .infinity)
                    .padding(.bottom, ABMetric.px(6))
                ProgressView()
                    .progressViewStyle(.linear)
                    .tint(ABColor.accent)
                    .padding(.horizontal, ABMetric.px(16))
            }
            HStack(spacing: ABMetric.px(10)) {
                Button {
                    dismiss()
                } label: {
                    Text("取消")
                        .font(.system(size: 15))
                        .foregroundColor(ABColor.fgDim)
                        .frame(maxWidth: .infinity)
                        .frame(height: ABMetric.px(48))
                        .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .fill(ABColor.elevate))
                }
                .buttonStyle(.plain)
                .disabled(importing)
                Button {
                    if strategy == .overwrite { confirmOverwrite = true } else { runImport() }
                } label: {
                    HStack(spacing: ABMetric.px(6)) {
                        if importing { ProgressView().tint(ABColor.onAccent) }
                        Text("导入所选（\(selected.count) 类）")
                            .font(.system(size: 15, weight: .semibold))
                    }
                    .foregroundColor(selected.isEmpty || importing ? ABColor.fgFaint : ABColor.onAccent)
                    .frame(maxWidth: .infinity)
                    .frame(height: ABMetric.px(48))
                    .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(selected.isEmpty || importing ? ABColor.elevate : ABColor.accent))
                }
                .buttonStyle(.plain)
                .disabled(selected.isEmpty || importing)
            }
            .padding(ABMetric.px(16))
        }
        .background(ABColor.card.opacity(0.98))
    }

    private func overwriteMessage(_ sum: BackupSummary) -> String {
        var parts: [String] = []
        if selected.contains(.servers), let r = sum.rows[.servers] {
            parts.append("\(r.localCount) 台电脑")
        }
        if selected.contains(.jumps), let r = sum.rows[.jumps] {
            parts.append("\(r.localCount) 台服务器")
        }
        if selected.contains(.keys), let r = sum.rows[.keys] {
            parts.append("\(r.localCount) 把密钥")
        }
        let who = parts.isEmpty ? "所选分类" : "本机 " + parts.joined(separator: "、")
        return "\(who)将被替换为备份内容。"
    }

    /// L14 执行导入：逐类推进展示；单条失败不中断
    private func runImport() {
        importing = true
        Task { @MainActor in
            var final: ImportReport?
            // 逐类推进只为展示「正在导入 …」；BackupStore.apply 本身已是逐条容错
            for cat in BackupCategory.allCases where selected.contains(cat) {
                currentCategory = cat
                try? await Task.sleep(nanoseconds: 60_000_000)   // 给进度条至少一帧
            }
            final = BackupStore.shared.apply(envelope, categories: selected, strategy: strategy)
            importing = false
            report = final
        }
    }

    // ---- 展示工具 ----

    static func platformName(_ raw: String) -> String {
        switch raw {
        case "ios": return "iOS"
        case "android": return "Android"
        case "desktop", "macos", "windows", "linux": return "桌面端"
        default: return raw.isEmpty ? "未知设备" : raw
        }
    }

    static func dateText(_ iso: String) -> String {
        guard let date = BackupDate.date(iso) else { return iso }
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd HH:mm"
        return f.string(from: date)
    }
}

// MARK: - S4 导入结果报告（L16）

struct BackupImportReport: View {

    let report: ImportReport
    @Environment(\.dismiss) private var dismiss

    /// 三态：✅ 全成功 / ⚠ 部分失败 / ❌ 全失败
    private var outcome: (icon: String, color: Color, title: String) {
        if report.totalFailed == 0 {
            return ("✓", ABColor.ok, "导入完成")
        }
        if report.totalAdded + report.totalUpdated > 0 {
            return ("⚠", ABColor.warn, "导入完成（有失败项）")
        }
        return ("✕", ABColor.bad, "导入失败")
    }

    var body: some View {
        VStack(spacing: 0) {
            Spacer(minLength: ABMetric.px(60))
            ZStack {
                Circle().fill(outcome.color.opacity(0.12)).frame(width: ABMetric.px(88), height: ABMetric.px(88))
                Text(outcome.icon)
                    .font(.system(size: 40, weight: .bold))
                    .foregroundColor(outcome.color)
            }
            Text(outcome.title)
                .font(.system(size: 20, weight: .bold))
                .foregroundColor(ABColor.fg)
                .padding(.top, ABMetric.px(16))
            summaryText
                .font(.system(size: 13))
                .foregroundColor(ABColor.fgDim)
                .multilineTextAlignment(.center)
                .padding(.top, ABMetric.px(8))
                .padding(.horizontal, ABMetric.px(24))
            if !report.isEmpty {
                Text("失败不影响已导入的部分")
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
                    .padding(.top, ABMetric.px(4))
            }
            if report.totalFailed > 0 {
                failureList
            }
            Spacer()
            Button {
                dismiss()
            } label: {
                Text(report.totalAdded + report.totalUpdated > 0 ? "完成" : "返回")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(ABColor.onAccent)
                    .frame(maxWidth: .infinity)
                    .frame(height: ABMetric.px(48))
                    .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(ABColor.accent))
            }
            .buttonStyle(.plain)
            .padding(ABMetric.px(16))
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(ABColor.base)
    }

    private var summaryText: Text {
        var parts: [String] = []
        for r in report.categories {
            var seg: [String] = []
            if r.added > 0 { seg.append("新增 \(r.added)") }
            if r.updated > 0 { seg.append("覆盖 \(r.updated)") }
            if r.skipped > 0 { seg.append("跳过 \(r.skipped)") }
            if !seg.isEmpty { parts.append("\(r.category.label)：\(seg.joined(separator: " · "))") }
        }
        if parts.isEmpty { parts = ["没有可导入的内容"] }
        var text = Text(parts.joined(separator: "\n"))
        let fails = report.totalFailed
        if fails > 0 {
            text = text + Text("\n失败 \(fails) 项")
        }
        return text
    }

    private var failureList: some View {
        VStack(alignment: .leading, spacing: ABMetric.px(6)) {
            ForEach(Array(report.categories.flatMap(\.failed).enumerated()), id: \.offset) { _, msg in
                HStack(alignment: .top, spacing: ABMetric.px(6)) {
                    Text("✕").font(.system(size: 12, weight: .bold)).foregroundColor(ABColor.bad)
                    Text(msg)
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.fg)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
            }
            ForEach(Array(report.categories.flatMap(\.missingRefs).enumerated()), id: \.offset) { _, msg in
                HStack(alignment: .top, spacing: ABMetric.px(6)) {
                    Text("⚠").font(.system(size: 12, weight: .bold)).foregroundColor(ABColor.warn)
                    Text(msg)
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.fgDim)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
            }
        }
        .padding(ABMetric.px(12))
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
            .strokeBorder(ABColor.bad.opacity(0.4), lineWidth: 1))
        .padding(.horizontal, ABMetric.px(16))
        .padding(.top, ABMetric.px(16))
    }
}
