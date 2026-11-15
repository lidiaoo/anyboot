import SwiftUI

/// 添加 / 编辑服务器（对齐 `ServersActivity.kt:990` EditJumpDialog）。
///
/// 设计稿是「底部上拉弹层 + 把手 + 全宽保存钮」；「删除」从列表卡片挪进这里
/// （卡片四钮里没有删除）。
struct JumpEditSheet: View {

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var jumps = JumpStore.shared

    @State private var name = ""
    @State private var isTelnet = false
    @State private var hostInput = ""
    @State private var portInput = ""
    @State private var username = ""
    @State private var authKey = false
    @State private var password = ""
    @State private var savePassword = true
    @State private var keyIndex = 0
    @State private var passphrase = ""
    @State private var note = ""
    @State private var jumpId = ""
    @State private var termPath = ""
    @State private var termShell = ""
    @State private var keyMenuOpen = false
    @State private var jumpMenuOpen = false
    @State private var loadedFor: String?

    private var editing: Bool { store.editTarget != nil }

    /// 候选排除自己（编辑时），不许自引
    private var jumpChoices: [JumpServer] {
        jumps.jumps.filter { store.editTarget == nil || $0.id != store.editTarget!.id }
    }

    var body: some View {
        ZStack(alignment: .bottom) {
            if store.editOpen {
                Color.black.opacity(0.42)
                    .ignoresSafeArea()
                    .onTapGesture { store.editOpen = false }

                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        Capsule()
                            .fill(ABColor.lineStrong)
                            .frame(width: ABMetric.px(40), height: ABMetric.px(4))
                            .frame(maxWidth: .infinity)
                            .padding(.top, ABMetric.px(12))
                            .padding(.bottom, ABMetric.px(4))

                        HStack {
                            Text(editing ? L("server.editAria") : L("server.add"))
                                .font(.system(size: 18, weight: .bold))
                                .foregroundColor(ABColor.fg)
                            Spacer(minLength: 0)
                            Button {
                                store.editOpen = false
                            } label: {
                                Text("✕")
                                    .font(.system(size: 15))
                                    .foregroundColor(ABColor.fgDim)
                                    .padding(ABMetric.px(8))
                            }
                            .buttonStyle(.plain)
                        }
                        .padding(.top, ABMetric.px(12))

                        Spacer().frame(height: ABMetric.px(12))

                        fieldLabel(L("keysWin.name"))
                        ABDialogInput(placeholder: L("and.namePh"), text: $name)

                        fieldLabel(L("srvEdit.proto"))
                        ABSegmented(options: ["SSH", L("and.telnetExperimental")], selected: isTelnet ? 1 : 0) {
                            isTelnet = $0 == 1
                        }

                        HStack(alignment: .top, spacing: ABMetric.px(12)) {
                            VStack(alignment: .leading, spacing: 0) {
                                fieldLabel(L("srvEdit.addr"))
                                ABDialogInput(placeholder: L("and.addrPh2"), text: $hostInput)
                            }
                            VStack(alignment: .leading, spacing: 0) {
                                fieldLabel(L("srvEdit.port"))
                                ABDialogInput(placeholder: isTelnet ? "23" : "22",
                                              text: Binding(
                                                get: { portInput },
                                                set: { portInput = $0.filter(\.isNumber) }),
                                              numeric: true)
                            }
                            .frame(width: ABMetric.px(88))
                        }

                        fieldLabel(L("server.kUser"))
                        ABDialogInput(placeholder: "root", text: $username)

                        fieldLabel(L("server.kAuth"))
                        ABSegmented(options: [L("srvEdit.authPassword"), L("srvEdit.authKey")], selected: authKey ? 1 : 0) {
                            authKey = $0 == 1
                        }

                        if !authKey {
                            ABDialogInput(placeholder: passwordPlaceholder,
                                          text: $password, secure: true)
                                .padding(.top, ABMetric.px(12))
                            // 「保存密码」卡片
                            HStack(alignment: .top, spacing: ABMetric.px(8)) {
                                Button {
                                    savePassword.toggle()
                                } label: {
                                    RoundedRectangle(cornerRadius: ABMetric.px(3), style: .continuous)
                                        .strokeBorder(ABColor.accent, lineWidth: 2)
                                        .background(
                                            RoundedRectangle(cornerRadius: ABMetric.px(3), style: .continuous)
                                                .fill(savePassword ? ABColor.accent : .clear))
                                        .frame(width: ABMetric.px(18), height: ABMetric.px(18))
                                        .padding(.top, ABMetric.px(2))
                                }
                                .buttonStyle(.plain)
                                VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                                    Text(L("and.saveCred"))
                                        .font(.system(size: 14))
                                        .foregroundColor(ABColor.fg)
                                    Text(L("and.saveCredHint"))
                                        .font(.system(size: 11))
                                        .foregroundColor(ABColor.fgFaint)
                                        .fixedSize(horizontal: false, vertical: true)
                                }
                                Spacer(minLength: 0)
                            }
                            .padding(.horizontal, ABMetric.px(12))
                            .padding(.vertical, ABMetric.px(10))
                            .background(
                                RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                                    .fill(ABColor.base))
                            .overlay(
                                RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                                    .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
                            .padding(.top, ABMetric.px(12))
                        } else {
                            fieldLabel(L("nav.keys"))
                            dropdown(text: keysLabel, open: $keyMenuOpen,
                                     enabled: !jumps.keys.isEmpty) {
                                ForEach(Array(jumps.keys.enumerated()), id: \.offset) { index, entry in
                                    menuRow(entry.name + "（" + entry.algorithm + "）") {
                                        keyIndex = index
                                        keyMenuOpen = false
                                    }
                                }
                            }
                            ABDialogInput(placeholder: L("and.keyPassPh"),
                                          text: $passphrase, secure: true)
                                .padding(.top, ABMetric.px(12))
                        }

                        fieldLabel(L("and.termPathOpt"))
                        ABDialogInput(placeholder: L("srvEdit.termPathPh"), text: $termPath)

                        fieldLabel(L("and.termShellOpt"))
                        ABDialogInput(placeholder: L("and.termShellPh2"), text: $termShell)
                        Text(L("and.termShellHint"))
                            .font(.system(size: 12))
                            .foregroundColor(ABColor.fgDim)
                            .padding(.top, ABMetric.px(4))
                            .fixedSize(horizontal: false, vertical: true)

                        fieldLabel(L("remote.notePh"))
                        ABDialogInput(placeholder: L("srvEdit.notesPh"), text: $note)

                        fieldLabel(L("hub.route.jump"))
                        dropdown(text: jumpLabel, open: $jumpMenuOpen, enabled: true) {
                            menuRow(L("srvEdit.jumpDirect")) {
                                jumpId = ""
                                jumpMenuOpen = false
                            }
                            ForEach(jumpChoices) { choice in
                                menuRow(choice.name + "（" + choice.addressLabel + "）") {
                                    jumpId = choice.id
                                    jumpMenuOpen = false
                                }
                            }
                        }

                        hintLine(L("and.jumpSelectHint"))
                        hintLine("SSH：支持隧道（外网连家里的电脑）+ 终端 + 文件；"
                                 + "Telnet：只有终端，明文传输，仅实验。")

                        Spacer().frame(height: ABMetric.px(16))

                        Button {
                            if let saved = persist() { store.editOpen = false }
                        } label: {
                            Text(L("common.save"))
                                .font(.system(size: 14, weight: .bold))
                                .foregroundColor(ABColor.onAccent)
                                .frame(maxWidth: .infinity)
                                .frame(height: ABMetric.px(48))
                                .background(
                                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                                        .fill(ABColor.accent))
                        }
                        .buttonStyle(.plain)

                        Spacer().frame(height: ABMetric.px(10))

                        if editing {
                            // 安卓「测试连接」= 先存再真发握手（ServersActivity:1294-1306）
                            Button {
                                if let saved = persist() {
                                    store.editOpen = false
                                    testConnection(saved)
                                }
                            } label: {
                                Text(L("bk.testConn"))
                                    .font(.system(size: 14))
                                    .foregroundColor(ABColor.fgDim)
                                    .frame(maxWidth: .infinity)
                                    .frame(height: ABMetric.px(48))
                                    .overlay(
                                        RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                                            .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
                            }
                            .buttonStyle(.plain)

                            Button {
                                store.editOpen = false
                                store.deleteTarget = store.editTarget
                            } label: {
                                Text(L("and.deleteThisServer"))
                                    .font(.system(size: 13))
                                    .foregroundColor(ABColor.bad)
                                    .padding(.top, ABMetric.px(6))
                                    .frame(maxWidth: .infinity)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, ABMetric.px(20))
                    .padding(.bottom, ABMetric.px(28))
                }
                .background(
                    UnevenRoundedRectangle(topLeadingRadius: ABMetric.px(20),
                                           topTrailingRadius: ABMetric.px(20),
                                           style: .continuous)
                        .fill(ABColor.card))
                // 安卓 ModalBottomSheet(skipPartiallyExpanded) ≈ 近全屏；之前写死 560 挤在半屏
                .frame(maxHeight: UIScreen.main.bounds.height * 0.9)
                .transition(.move(edge: .bottom))
            }
        }
        .onChange(of: store.editOpen) { _, open in
            if open { load() }
        }
    }

    // MARK: - 小件

    private func fieldLabel(_ text: String) -> some View {
        // Common.kt:272 FieldLabel：12sp、fgDim、上 10 / 下 2
        Text(text)
            .font(.system(size: 12))
            .foregroundColor(ABColor.fgDim)
            .padding(.top, ABMetric.px(10))
            .padding(.bottom, ABMetric.px(2))
    }

    private func hintLine(_ text: String) -> some View {
        // Android 这两条说明走 FieldLabel（ServersActivity:1267-1271）：12sp fgDim
        Text(text)
            .font(.system(size: 12))
            .foregroundColor(ABColor.fgDim)
            .padding(.top, ABMetric.px(10))
            .fixedSize(horizontal: false, vertical: true)
    }

    private func dropdown<Content: View>(text: String, open: Binding<Bool>, enabled: Bool,
                                         @ViewBuilder content: @escaping () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                guard enabled else { return }
                open.wrappedValue.toggle()
            } label: {
                HStack(spacing: ABMetric.px(8)) {
                    Text(text)
                        .font(.system(size: 13))
                        .foregroundColor(enabled ? ABColor.fg : ABColor.fgDim)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    Text(open.wrappedValue ? "▴" : "▾")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.fgDim)
                }
                .padding(.horizontal, ABMetric.px(12))
                .padding(.vertical, ABMetric.px(10))
                .background(
                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(ABColor.base))
                .overlay(
                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .strokeBorder(ABColor.lineStrong, lineWidth: ABMetric.hairline))
            }
            .buttonStyle(.plain)
            if open.wrappedValue {
                VStack(alignment: .leading, spacing: 0) { content() }
                    .padding(.vertical, ABMetric.px(2))
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .fill(ABColor.elevate))
                    .overlay(
                        RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
                    .padding(.top, ABMetric.px(4))
            }
        }
    }

    private func menuRow(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 13))
                .foregroundColor(ABColor.fg)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, ABMetric.px(12))
                .padding(.vertical, ABMetric.px(10))
        }
        .buttonStyle(.plain)
    }

    private var keysLabel: String {
        if jumps.keys.isEmpty { return L("and.noKeysHint") }
        let entry = jumps.keys[min(keyIndex, jumps.keys.count - 1)]
        return entry.name + "（" + entry.algorithm + "）"
    }

    private var jumpLabel: String {
        if jumpId.isEmpty { return L("srvEdit.jumpDirect") }
        if let choice = jumpChoices.first(where: { $0.id == jumpId }) { return choice.name }
        return L("and.jumpDeletedPd")
    }

    private var savedPassword: String {
        guard let target = store.editTarget, !target.passwordRef.isEmpty else { return "" }
        return KeyVault.load(account: target.passwordRef)
    }

    private var passwordPlaceholder: String {
        if !savedPassword.isEmpty { return L("and.pwdSavedEditable") }
        if let target = store.editTarget, !target.passwordRef.isEmpty {
            return L("and.pwdExpired")
        }
        return L("and.pwdKeepBlank")
    }

    // MARK: - 载入

    private func load() {
        let target = store.editTarget
        guard loadedFor != (target?.id ?? "new") else { return }
        loadedFor = target?.id ?? "new"
        name = target?.name ?? ""
        isTelnet = (target?.isSsh == false)
        hostInput = target?.hostname ?? ""
        portInput = target.map { "\($0.port)" } ?? ""
        username = target?.username ?? ""
        authKey = target?.authType == JumpServer.authKey
        password = ""
        savePassword = true
        passphrase = ""
        note = target?.note ?? ""
        jumpId = target?.jumpServerId ?? ""
        termPath = target?.termPath ?? ""
        termShell = target?.termShell ?? ""
        if let target, target.authType == JumpServer.authKey,
           let index = jumps.keys.firstIndex(where: { $0.id == target.keyId }) {
            keyIndex = index
        } else {
            keyIndex = 0
        }
        keyMenuOpen = false
        jumpMenuOpen = false
    }

    // MARK: - 保存（`:1362` persistJump）

    /// 校验 + 落库；返回保存后的档案（校验没过 = nil，弹层保持打开）
    private func persist() -> JumpServer? {
        // 跳板机只对 SSH 有意义：Telnet 选了跳板机直接拦下（表单提示里写明了）
        if isTelnet && !jumpId.isEmpty {
            store.toast(L("and.telnetNoJump"), long: true)
            return nil
        }
        var target = store.editTarget ?? JumpServer()
        target.name = name.trimmingCharacters(in: .whitespaces)
        target.protocolRaw = isTelnet ? JumpServer.telnet : JumpServer.ssh

        let port = portInput.trimmingCharacters(in: .whitespaces)
        var host = hostInput.trimmingCharacters(in: .whitespaces)
        target.username = username.trimmingCharacters(in: .whitespaces)
        if !port.isEmpty {
            host = host.split(separator: ":").first.map(String.init) ?? host
            host += ":" + port
        }
        target.applyHostInput(host)

        target.authType = authKey ? JumpServer.authKey : JumpServer.authPassword
        if !savePassword {
            // 不勾「保存密码」：清掉已存的（连接时将回退到密钥 / 失败提示）
            if !target.passwordRef.isEmpty { KeyVault.delete(account: target.passwordRef) }
            target.passwordRef = ""
        } else if !password.isEmpty {
            let ref = target.passwordRef.isEmpty ? "pwd-\(target.id)" : target.passwordRef
            // 写不进钥匙串就拒绝保存（对齐 Android：「密码加密保存失败…」且对话框不关），
            // 否则用户填了密码却等于白填，下次连接又弹应急框
            guard KeyVault.save(password, account: ref) else {
                store.toast(L("and.pwdSaveFail"), long: true)
                return nil
            }
            target.passwordRef = ref
        }

        if authKey && !jumps.keys.isEmpty {
            let key = jumps.keys[min(keyIndex, jumps.keys.count - 1)]
            target.keyId = key.id
            if !passphrase.isEmpty {
                let ref = target.keyPassphraseRef.isEmpty
                    ? "pass-\(target.id)" : target.keyPassphraseRef
                KeyVault.save(passphrase, account: ref)
                target.keyPassphraseRef = ref
            }
        } else if !authKey {
            target.keyId = ""
        }
        target.note = note.trimmingCharacters(in: .whitespaces)
        target.jumpServerId = isTelnet ? "" : jumpId.trimmingCharacters(in: .whitespaces)
        target.termPath = termPath.trimmingCharacters(in: .whitespaces)
        target.termShell = termShell.trimmingCharacters(in: .whitespaces)

        jumps.upsert(target)
        if store.editTarget != nil {
            // 改完（尤其是改了密码 / 用户名）立刻能重连，别被退避挡住
            SshSessionManager.shared.resetBackoff(target.id)
        }
        return target
    }

    // MARK: - 测试连接（ServersActivity testConnection:903）

    /// SSH 握手 / Telnet TCP 可达性，量往返；密码没存上先弹应急输入框
    private func testConnection(_ server: JumpServer) {
        if server.isSsh && SshSessionManager.shared.needsManualPassword(server) {
            store.manualPassword = .init(server: server, onReady: { testConnection(server) })
            return
        }
        store.toast(L("and.testingAddr", server.addressLabel))
        let started = Date()
        Task {
            do {
                if server.isSsh {
                    _ = try await SshSessionManager.shared.client(for: server)
                } else {
                    try await TelnetProbe.check(host: server.hostname, port: server.port)
                }
                let ms = Int(Date().timeIntervalSince(started) * 1000)
                var updated = server
                updated.lastRttMs = Double(ms)
                updated.lastSeen = Date()
                JumpStore.shared.upsert(updated)
                store.markSessionsChanged()
                store.toast(L("and.connOkMs", ms), long: true)
            } catch {
                store.toast(SshSessionManager.describe(error), long: true)
            }
        }
    }
}
