import SwiftUI
import Crypto
import Citadel

// MARK: - OpenSSH 格式（Ed25519 生成用）

extension Curve25519.Signing.PrivateKey {

    /// 公钥 SSH wire blob：string("ssh-ed25519") + string(pk)
    var ed25519PublicBlob: Data {
        var blob = Data()
        appendSSHString(&blob, Data("ssh-ed25519".utf8))
        appendSSHString(&blob, publicKey.rawRepresentation)
        return blob
    }

    /// authorized_keys 一行：ssh-ed25519 AAAA… comment
    func ed25519OpenSSHPublicKey(comment: String) -> String {
        "ssh-ed25519 \(ed25519PublicBlob.base64EncodedString())\(comment.isEmpty ? "" : " \(comment)")"
    }

    /// OpenSSH 口径指纹：SHA256(blob)
    var ed25519Fingerprint: String {
        KeyVault.sha256Fingerprint(ed25519PublicBlob)
    }

    /// 未加密的 OpenSSH 私钥 PEM（openssh-key-v1 / cipher none / kdf none）。
    /// 布局见 PROTOCOL.key：magic + none/none + 1 把公钥 + 私有段
    /// （checkint×2 + keytype + pub + priv(seed‖pub) + comment + padding）。
    func ed25519OpenSSHPrivateKey(comment: String) -> String {
        let pub = publicKey.rawRepresentation
        let seed = rawRepresentation   // 32B seed
        let check = Data((0..<4).map { _ in UInt8.random(in: 0...255) })

        var privateSection = Data()
        privateSection.append(check)
        privateSection.append(check)
        appendSSHString(&privateSection, Data("ssh-ed25519".utf8))
        appendSSHString(&privateSection, pub)
        appendSSHString(&privateSection, seed + pub)   // priv = seed ‖ pub（64B）
        appendSSHString(&privateSection, Data(comment.utf8))
        // 补位：1,2,3… 直到整段对齐 8
        var padding: UInt8 = 1
        while privateSection.count % 8 != 0 {
            privateSection.append(padding)
            padding += 1
        }

        var blob = Data("openssh-key-v1\0".utf8)
        appendSSHString(&blob, Data())                       // cipher = none
        appendSSHString(&blob, Data())                       // kdf = none
        appendSSHString(&blob, Data())                       // kdfoptions = 空
        appendUInt32(&blob, 1)                               // 1 把钥匙
        appendSSHString(&blob, ed25519PublicBlob)            // 公钥
        appendSSHString(&blob, privateSection)               // 私有段整块

        let base64 = blob.base64EncodedString()
        let lines = stride(from: 0, to: base64.count, by: 70).map {
            String(base64.dropFirst($0).prefix(70))
        }
        return "-----BEGIN OPENSSH PRIVATE KEY-----\n"
            + lines.joined(separator: "\n")
            + "\n-----END OPENSSH PRIVATE KEY-----"
    }

    private func appendSSHString(_ data: inout Data, _ value: Data) {
        appendUInt32(&data, UInt32(value.count))
        data.append(value)
    }

    private func appendUInt32(_ data: inout Data, _ value: UInt32) {
        withUnsafeBytes(of: value.bigEndian) { data.append(contentsOf: $0) }
    }
}

// MARK: - 终端 sheet（单会话：标签条 + 终端）

/// 一个已打开的终端会话页：TerminalSessionHost（标签条 + xterm）
struct TerminalSheet: View {

    let session: TerminalSessionRef
    @Environment(\.dismiss) private var dismiss
    @State private var ctrlArmed = false
    @State private var fontSize: CGFloat = 14
    @State private var dark = JumpStore.shared.terminalDark
    // 字体族编辑（终端设置的一部分：改动落盘 + 通知已开页面热更）
    @State private var fontEditorOpen = false
    @State private var fontFamilyDraft = ""

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                TerminalSessionHost(initialId: session.server.id)
            }
            .background(dark
                ? Color(red: 0.12, green: 0.12, blue: 0.12)
                : Color.white)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Text(session.server.displayName).font(.headline)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button(dark ? "切到亮色主题" : "切到深色主题") {
                            dark.toggle()
                            JumpStore.shared.setTerminalDark(dark)
                        }
                        Button("终端字体族…") {
                            fontFamilyDraft = JumpStore.shared.terminalFontFamily
                            fontEditorOpen = true
                        }
                        Button("粘贴") {
                            if let text = UIPasteboard.general.string {
                                session.write(Data(text.utf8))
                            }
                        }
                        Button("结束会话", role: .destructive) {
                            TerminalSessionsRegistry.kill(session)
                            dismiss()
                        }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                }
            }
            .safeAreaInset(edge: .bottom) { keyBar }
            .alert("终端字体族", isPresented: $fontEditorOpen) {
                TextField("留空 = 内置等宽", text: $fontFamilyDraft)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                Button("应用") {
                    JumpStore.shared.setTerminalFontFamily(fontFamilyDraft)
                    // 已开的终端页热更；新开的页面从 JumpStore 里读
                    NotificationCenter.default.post(
                        name: .init("anyboot-term-font"),
                        object: JumpStore.shared.terminalFontFamily)
                }
                Button("取消", role: .cancel) { }
            } message: {
                Text("填 CSS font-family（如 JetBrains Mono / Menlo）；字体缺失自动回落等宽。")
            }
        }
    }

    /// 移动端刚需按键栏：Esc / Tab / Ctrl / 方向键 / Enter
    private var keyBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                key("Esc") { session.write(Data([0x1b])) }
                key("Tab") { session.write(Data([0x09])) }
                key("Ctrl", highlighted: ctrlArmed) {
                    ctrlArmed.toggle()
                    if ctrlArmed {
                        // 提示：Ctrl 臂亮起后，下一个字母走软键盘会把 Ctrl 组合发出去
                    }
                }
                key("↑") { session.write(Data([0x1b, 0x5b, 0x41])) }
                key("↓") { session.write(Data([0x1b, 0x5b, 0x42])) }
                key("←") { session.write(Data([0x1b, 0x5b, 0x44])) }
                key("→") { session.write(Data([0x1b, 0x5b, 0x43])) }
                key("Enter") { session.write(Data([0x0d])) }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
        }
        .background(Color(.secondarySystemBackground))
    }

    private func key(_ label: String, highlighted: Bool = false,
                     action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.footnote)
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(RoundedRectangle(cornerRadius: 8)
                    .fill(highlighted ? Color.accentColor : Color(.tertiarySystemFill)))
                .foregroundColor(highlighted ? .white : .primary)
        }
    }
}

// MARK: - 添加 / 编辑服务器

struct JumpEditorSheet: View {

    enum Result {
        case save(JumpServer)
        case delete(String)
        case test(JumpServer)
    }

    let jump: JumpServer?
    let onFinish: (Result) -> Void

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var store = JumpStore.shared

    @State private var name = ""
    @State private var isTelnet = false
    @State private var host = ""
    @State private var username = ""
    @State private var useKey = false
    @State private var password = ""
    @State private var savePassword = true
    @State private var keyId = ""
    @State private var passphrase = ""
    @State private var note = ""
    // 终端启动路径 / 默认 shell（每台服务器各自配，留空 = 远端家目录 + 登录 shell）
    @State private var termPath = ""
    @State private var termShell = ""

    private var editing: Bool { jump != nil && store.jump(jump?.id ?? "") != nil }

    var body: some View {
        NavigationStack {
            Form {
                Section("名称") {
                    TextField("例如：pve / 家里路由器", text: $name)
                        .autocorrectionDisabled()
                }
                Section("协议") {
                    Picker("协议", selection: $isTelnet) {
                        Text("SSH（推荐：隧道+终端+文件）").tag(false)
                        Text("Telnet（实验性，仅终端）").tag(true)
                    }
                    .pickerStyle(.inline)
                    .labelsHidden()
                }
                Section("地址（host:port）") {
                    TextField("例如 1.2.3.4:22", text: $host)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section("用户名") {
                    TextField("root", text: $username)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section("认证方式") {
                    Picker("认证", selection: $useKey) {
                        Text("密码").tag(false)
                        Text("密钥").tag(true)
                    }
                    .pickerStyle(.segmented)
                    if useKey {
                        Picker("密钥", selection: $keyId) {
                            Text("（选一把）").tag("")
                            ForEach(store.keys) { key in
                                Text("\(key.name)（\(key.algorithm)）").tag(key.id)
                            }
                        }
                        SecureField("密钥口令（可空）", text: $passphrase)
                        if store.keys.isEmpty {
                            Text("还没有密钥 —— 先到「密钥」页导入 / 生成。")
                                .font(.footnote).foregroundStyle(.secondary)
                        }
                    } else {
                        SecureField(editing ? "留空保持原密码不变" : "密码", text: $password)
                        // 「保存密码」勾选（对齐设计稿）：默认勾上；不勾 = 清掉已存密码
                        Toggle("保存密码（钥匙串 · 连接免输）", isOn: $savePassword)
                        if !savePassword {
                            Text("不保存：清除已存密码，连接需改用密钥")
                                .font(.footnote).foregroundStyle(.secondary)
                        }
                    }
                }
                Section("终端") {
                    TextField("启动路径（如 /data/logs，留空 = 家目录）", text: $termPath)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                    TextField("shell（如 /bin/zsh，留空 = 登录 shell）", text: $termShell)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                    Text("路径打不通时终端里报错、照常落家目录；预设 shell 自动带 -l，也可填自定义命令原样执行。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                Section("备注") {
                    TextField("可选", text: $note)
                }
                Section {
                    Text("SSH：支持隧道（外网连家里的电脑）+ 终端 + 文件；"
                         + "Telnet：只有终端，明文传输，仅实验。")
                        .font(.footnote).foregroundStyle(.secondary)
                    Button(editing ? "保存并测试连接" : "测试连接") {
                        save()
                        if var current = jump {
                            apply(to: &current)
                            onFinish(.test(current))
                        }
                    }
                    .disabled(host.trimmedAnyBoot.isEmpty)
                }
                if editing {
                    Section {
                        Button("删除这台服务器", role: .destructive) {
                            if let id = jump?.id {
                                onFinish(.delete(id))
                            }
                            dismiss()
                        }
                    }
                }
            }
            .navigationTitle(editing ? "编辑服务器" : "添加服务器")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("保存") {
                        save()
                        dismiss()
                    }
                    .disabled(host.trimmedAnyBoot.isEmpty)
                }
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                }
            }
            .onAppear {
                if let jump {
                    name = jump.name
                    isTelnet = !jump.isSsh
                    host = jump.host
                    username = jump.username
                    useKey = jump.authType == JumpServer.authKey
                    keyId = jump.keyId
                    note = jump.note
                    termPath = jump.termPath ?? ""
                    termShell = jump.termShell ?? ""
                    // 已存密码回显，用户能确认到底存没存；读不出来 = 没存上
                    if !useKey && !jump.passwordRef.isEmpty {
                        let saved = KeyVault.load(account: jump.passwordRef)
                        if !saved.isEmpty {
                            password = saved
                        }
                    }
                }
            }
        }
    }

    private func apply(to target: inout JumpServer) {
        target.name = name.trimmedAnyBoot
        target.protocolRaw = isTelnet ? JumpServer.telnet : JumpServer.ssh
        target.applyHostInput(host)
        target.username = username.trimmedAnyBoot
        target.authType = useKey ? JumpServer.authKey : JumpServer.authPassword
        target.keyId = useKey ? keyId : ""
        target.note = note.trimmedAnyBoot
        target.termPath = termPath.trimmedAnyBoot.isEmpty ? nil : termPath.trimmedAnyBoot
        target.termShell = termShell.trimmedAnyBoot.isEmpty ? nil : termShell.trimmedAnyBoot
        if !useKey {
            if savePassword {
                if !password.trimmedAnyBoot.isEmpty {
                    let ref = target.passwordRef.isEmpty ? "pw-\(target.id)" : target.passwordRef
                    KeyVault.save(password, account: ref)
                    target.passwordRef = ref
                    password = ""
                }
            } else if !target.passwordRef.isEmpty {
                KeyVault.delete(account: target.passwordRef)
                target.passwordRef = ""
            }
        }
        if useKey && !passphrase.trimmedAnyBoot.isEmpty {
            let ref = target.keyPassphraseRef.isEmpty
                ? "kp-\(target.id)" : target.keyPassphraseRef
            KeyVault.save(passphrase, account: ref)
            target.keyPassphraseRef = ref
            passphrase = ""
        }
    }

    private func save() {
        var target = jump ?? JumpServer()
        apply(to: &target)
        onFinish(.save(target))
    }
}
