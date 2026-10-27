import SwiftUI

// MARK: - S2 WebDAV 与同步内容（L8 保存 / L9 测试连接 / L10 加密开关）

struct BackupWebDavView: View {

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var sync = SyncStore.shared

    // 表单（进入时从 config 拷贝，保存才落库）
    @State private var serverURL = ""
    @State private var username = ""
    @State private var password = ""
    @State private var showPassword = false
    @State private var directory = "AnyBoot"
    @State private var periodMinutes = 0
    @State private var syncOnChange = false
    @State private var categories: Set<BackupCategory> = [.servers, .jumps, .keys, .terminal]
    @State private var encrypt = false
    @State private var encryptPassword = ""
    @State private var encryptPasswordAgain = ""

    // 交互状态
    @State private var testing = false
    @State private var testResult: String?
    @State private var testOK = false
    @State private var confirmCreateDir = false
    @State private var loaded = false

    var body: some View {
        VStack(spacing: 0) {
            navBar
            ABHairline()
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    serverCard
                    typeCard
                    encryptCard
                    testButton
                        .padding(.top, ABMetric.px(16))
                }
                .padding(.horizontal, ABMetric.px(16))
                .padding(.bottom, ABMetric.px(32))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(ABColor.base)
        .onAppear {
            guard !loaded else { return }
            loaded = true
            let c = sync.config
            serverURL = c.serverURL
            username = c.username
            password = KeyVault.load(account: c.passwordRef)
            directory = c.directory
            periodMinutes = c.periodMinutes
            syncOnChange = c.syncOnChange
            categories = c.categories
            encrypt = c.encrypt
            encryptPassword = KeyVault.load(account: c.encryptPasswordRef)
            encryptPasswordAgain = encryptPassword
        }
        // S6c 自动创建目录
        .confirmationDialog("目录不存在", isPresented: $confirmCreateDir, titleVisibility: .visible) {
            Button("创建") { createDirectory() }
            Button("取消", role: .cancel) { }
        } message: {
            Text("远端目录不存在，是否自动创建？")
        }
    }

    private var navBar: some View {
        HStack(spacing: 0) {
            ABBarIcon(text: "‹", description: "返回") { dismiss() }
            Text("WebDAV 与同步内容")
                .font(.system(size: 16, weight: .bold))
                .foregroundColor(ABColor.fg)
            Spacer(minLength: 0)
            Button {
                save()
            } label: {
                Text("保存")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(saveEnabled ? ABColor.accent : ABColor.fgFaint)
                    .padding(.horizontal, ABMetric.px(12))
                    .frame(height: ABMetric.px(44))
            }
            .buttonStyle(.plain)
            .disabled(!saveEnabled)
        }
        .padding(.trailing, ABMetric.px(4))
        .frame(height: ABMetric.px(44))
    }

    // MARK: - 服务器卡片

    private var serverCard: some View {
        VStack(alignment: .leading, spacing: 0) {
            ABFieldLabel("服务器地址")
            ABDialogInput(placeholder: "https://dav.jianguoyun.com/dav/", text: $serverURL)
                .keyboardType(.URL)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
            ABFieldLabel("用户名")
            ABDialogInput(placeholder: "账号 / 邮箱", text: $username)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
            ABFieldLabel("密码")
            HStack(spacing: ABMetric.px(8)) {
                Group {
                    if showPassword {
                        TextField("WebDAV 密码", text: $password)
                    } else {
                        SecureField("WebDAV 密码", text: $password)
                    }
                }
                .font(.system(size: 13))
                .foregroundColor(ABColor.fg)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .padding(.horizontal, ABMetric.px(12))
                .padding(.vertical, ABMetric.px(10))
                .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                    .fill(ABColor.base))
                .overlay(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                    .strokeBorder(ABColor.lineStrong, lineWidth: ABMetric.hairline))
                Button {
                    showPassword.toggle()
                } label: {
                    Text(showPassword ? "隐藏" : "显示")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.accent)
                        .frame(minHeight: ABMetric.px(44))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("切换密码明文")
            }
            ABFieldLabel("远端同步目录")
            ABDialogInput(placeholder: "AnyBoot", text: $directory)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
            ABFieldLabel("同步周期")
            Picker("同步周期", selection: $periodMinutes) {
                Text("手动").tag(0)
                Text("15 分钟").tag(15)
                Text("30 分钟").tag(30)
                Text("60 分钟").tag(60)
            }
            .pickerStyle(.segmented)
            .padding(.top, ABMetric.px(2))
            Toggle(isOn: $syncOnChange) {
                VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                    Text("记录变化时就同步")
                        .font(.system(size: 14))
                        .foregroundColor(ABColor.fg)
                    Text("建议只在自有 WebDAV 服务器时开启，防止快速占用服务商限额。")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.fgFaint)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.top, ABMetric.px(12))
            // L9 测试结果行
            if let testResult {
                HStack(spacing: ABMetric.px(6)) {
                    Text(testOK ? "✓" : "✕")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundColor(testOK ? ABColor.ok : ABColor.bad)
                    Text(testResult)
                        .font(.system(size: 12))
                        .foregroundColor(testOK ? ABColor.ok : ABColor.bad)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
                .padding(.top, ABMetric.px(10))
            }
        }
        .padding(ABMetric.px(12))
        .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
            .fill(ABColor.card))
        .padding(.top, ABMetric.px(12))
    }

    // MARK: - 数据类型卡片

    private var typeCard: some View {
        VStack(alignment: .leading, spacing: 0) {
            ABFieldLabel("同步的数据类型")
            ForEach(BackupCategory.allCases) { cat in
                Button {
                    withAnimation(.easeInOut(duration: 0.15)) {
                        if categories.contains(cat) {
                            categories.remove(cat)
                        } else {
                            categories.insert(cat)
                        }
                    }
                } label: {
                    HStack(spacing: ABMetric.px(10)) {
                        Image(systemName: categories.contains(cat) ? "checkmark.square.fill" : "square")
                            .font(.system(size: 18))
                            .foregroundColor(categories.contains(cat) ? ABColor.accent : ABColor.fgFaint)
                        Text(cat.label)
                            .font(.system(size: 14))
                            .foregroundColor(ABColor.fg)
                        Spacer(minLength: 0)
                    }
                    .frame(minHeight: ABMetric.px(44))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            if categories.contains(.keys) && !encrypt {
                HStack(spacing: ABMetric.px(6)) {
                    Text("⚠").font(.system(size: 12, weight: .bold)).foregroundColor(ABColor.warn)
                    Text("建议开启加密，私钥将以明文存放于 WebDAV 服务器")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.warn)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
                .padding(ABMetric.px(8))
                .background(RoundedRectangle(cornerRadius: ABMetric.px(8), style: .continuous)
                    .fill(ABColor.warn.opacity(0.12)))
                .padding(.top, ABMetric.px(6))
            }
        }
        .padding(ABMetric.px(12))
        .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
            .fill(ABColor.card))
        .padding(.top, ABMetric.px(12))
    }

    // MARK: - 加密卡片（L10）

    private var encryptCard: some View {
        VStack(alignment: .leading, spacing: 0) {
            Toggle(isOn: $encrypt) {
                Text("同步文件加密")
                    .font(.system(size: 14))
                    .foregroundColor(ABColor.fg)
            }
            .onChange(of: encrypt) { _, on in
                // L10：关→清空口令（明示即可，无二次确认）
                if !on {
                    encryptPassword = ""
                    encryptPasswordAgain = ""
                }
            }
            if encrypt {
                ABFieldLabel("同步口令（≥6 位）")
                SecureField("同步口令", text: $encryptPassword)
                    .font(.system(size: 13))
                    .foregroundColor(ABColor.fg)
                    .padding(.horizontal, ABMetric.px(12))
                    .padding(.vertical, ABMetric.px(10))
                    .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(ABColor.base))
                    .overlay(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .strokeBorder(ABColor.lineStrong, lineWidth: ABMetric.hairline))
                SecureField("再次输入同步口令", text: $encryptPasswordAgain)
                    .font(.system(size: 13))
                    .foregroundColor(ABColor.fg)
                    .padding(.horizontal, ABMetric.px(12))
                    .padding(.vertical, ABMetric.px(10))
                    .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(ABColor.base))
                    .overlay(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .strokeBorder(encryptMismatch ? ABColor.bad : ABColor.lineStrong, lineWidth: ABMetric.hairline))
                if encryptMismatch {
                    Text("两次输入不一致")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.bad)
                }
            }
        }
        .padding(ABMetric.px(12))
        .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
            .fill(ABColor.card))
        .padding(.top, ABMetric.px(12))
    }

    private var encryptMismatch: Bool {
        !encryptPasswordAgain.isEmpty && encryptPassword != encryptPasswordAgain
    }

    // MARK: - 测试连接按钮（底部固定次级）

    private var testButton: some View {
        Button {
            test()
        } label: {
            HStack(spacing: ABMetric.px(8)) {
                if testing { ProgressView().tint(ABColor.accent) }
                Text(testing ? "测试中…" : "测试连接")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(formValid ? ABColor.accent : ABColor.fgFaint)
            }
            .frame(maxWidth: .infinity)
            .frame(height: ABMetric.px(48))
            .background(RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .strokeBorder(formValid ? ABColor.accent : ABColor.lineStrong, lineWidth: 1.5))
        }
        .buttonStyle(.plain)
        .disabled(!formValid || testing)
    }

    // MARK: - 校验与动作

    private var formValid: Bool {
        WebDavConfig(serverURL: serverURL, username: username,
                     passwordRef: "", directory: directory).isValid
    }

    /// L8：地址非空且含 scheme 才可保存；口令区展开但不完整时仍可保存（未启加密时忽略）
    private var saveEnabled: Bool {
        formValid && (!encrypt || (encryptPassword.count >= 6 && !encryptMismatch))
    }

    private func applyToConfig() {
        var c = sync.config
        c.serverURL = serverURL.trimmingCharacters(in: .whitespacesAndNewlines)
        c.username = username.trimmingCharacters(in: .whitespacesAndNewlines)
        if !password.isEmpty {
            let ref = c.passwordRef.isEmpty ? "webdav-password" : c.passwordRef
            if KeyVault.save(password, account: ref) { c.passwordRef = ref }
        }
        c.directory = directory.trimmingCharacters(in: .whitespacesAndNewlines)
        c.periodMinutes = periodMinutes
        c.syncOnChange = syncOnChange
        c.categories = categories
        c.encrypt = encrypt
        if encrypt {
            let ref = c.encryptPasswordRef.isEmpty ? "sync-encrypt-password" : c.encryptPasswordRef
            if KeyVault.save(encryptPassword, account: ref) { c.encryptPasswordRef = ref }
        } else {
            if !c.encryptPasswordRef.isEmpty { KeyVault.delete(account: c.encryptPasswordRef) }
            c.encryptPasswordRef = ""
        }
        sync.config = c
    }

    private func save() {
        applyToConfig()
        SyncEngine.shared.restartPeriodic()
        ABToastBus.shared.show("已保存")
        dismiss()
    }

    /// L9 三态：成功 / 404 → 确认创建 / 失败带原因。测试不写任何配置。
    private func test() {
        testing = true
        testResult = nil
        Task { @MainActor in
            // 用当前表单临时组客户端（不落库）
            var c = sync.config
            c.serverURL = serverURL.trimmingCharacters(in: .whitespacesAndNewlines)
            c.username = username.trimmingCharacters(in: .whitespacesAndNewlines)
            c.directory = directory.trimmingCharacters(in: .whitespacesAndNewlines)
            let client = WebDavClient(config: c.webDav)
            switch await client.testConnection() {
            case .success:
                testOK = true
                testResult = "连接成功"
            case .failure(.notFound):
                testing = false
                confirmCreateDir = true
                return
            case .failure(let e):
                testOK = false
                testResult = e.errorDescription
            }
            testing = false
        }
    }

    private func createDirectory() {
        testing = true
        Task { @MainActor in
            var c = sync.config
            c.serverURL = serverURL.trimmingCharacters(in: .whitespacesAndNewlines)
            c.username = username.trimmingCharacters(in: .whitespacesAndNewlines)
            c.directory = directory.trimmingCharacters(in: .whitespacesAndNewlines)
            let client = WebDavClient(config: c.webDav)
            switch await client.createDirectory() {
            case .success:
                testOK = true
                testResult = "目录已创建，连接成功"
            case .failure(let e):
                testOK = false
                testResult = e.errorDescription ?? "创建失败"
            }
            testing = false
        }
    }
}
