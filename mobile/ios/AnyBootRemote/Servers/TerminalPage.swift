import SwiftUI
import UIKit

/// 终端页（对齐 `TerminalActivity.kt`）：顶栏（‹ + 图标块 + 名称/状态 + 🌓 / Aa / ⌨）+
/// xterm 区 + 会话 pill 行 + 按键栏（8+4）+ Ctrl 字母行。
///
/// 会话由 `TerminalSessionsRegistry` 静态持有，本页销毁不关会话；
/// 从「文件」页跳进来的 `cdPath` 只在第一次绑定时打一次（别把用户敲的路径顶掉）。
struct TerminalPage: View {

    let jumpId: String
    var cdPath: String = ""

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var jumps = JumpStore.shared

    @State private var currentId = ""
    @State private var statusMessage: String? = "正在连接 …"
    @State private var ctrlArmed = false
    @State private var killTarget: TerminalSessionRef?
    @State private var manualPassword: ServerModuleStore.ManualPasswordRequest?
    @State private var fontMenuOpen = false
    @State private var pendingCd: String?
    @State private var pendingCdSent = false
    @State private var opened = false
    @State private var started = false

    private var session: TerminalSessionRef? { TerminalSessionsRegistry.get(currentId) }

    var body: some View {
        VStack(spacing: 0) {
            // ⚠️ 顶栏必须抬到终端区之上：Aa 的字号菜单是 .overlay 挂在按钮上的，
            // 而 VStack 按书写顺序绘制 —— 不设 zIndex 时后画的终端 ZStack（含
            // xterm WebView，背景不透明）会把菜单整块盖住，只剩顶栏下沿一条缝。
            topBar
                .zIndex(10)

            ZStack {
                (jumps.terminalDark ? ABColor.terminalDarkBg : ABColor.terminalLightBg)
                    .ignoresSafeArea(edges: .bottom)
                if let session {
                    TerminalView(session: session)
                        .id(session.server.id)
                }
                if let statusMessage {
                    Text(statusMessage)
                        .font(.system(size: 14))
                        .foregroundColor(ABColor.fgDim)
                        .multilineTextAlignment(.center)
                        .padding(ABMetric.px(16))
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            // 字号菜单开着时，点一下终端区就收起（对齐 Android Popup 的 outside tap）。
            // 顺带保证这一下不会漏给 WebView —— 否则会误敲字进终端。
            .overlay {
                if fontMenuOpen {
                    Color.clear
                        .contentShape(Rectangle())
                        .onTapGesture { fontMenuOpen = false }
                }
            }

            sessionTabs
            if jumps.terminalKeyBar { keyBar }
            if ctrlArmed { lettersRow }
        }
        .background((jumps.terminalDark ? ABColor.terminalDarkBg : ABColor.terminalLightBg)
            .ignoresSafeArea())
        .overlay { dialogs }
        .onAppear {
            currentId = jumpId
            pendingCd = cdPath.isEmpty ? nil : cdPath
            if !started {
                started = true
                Task { await ensureSession(jumpId: jumpId, alreadyOpen: false) }
            }
        }
    }

    // MARK: - 顶栏

    private var topBar: some View {
        HStack(spacing: 0) {
            ABBarIcon(text: "‹", description: "返回") { dismiss() }

            ZStack {
                RoundedRectangle(cornerRadius: ABMetric.px(10), style: .continuous)
                    .fill(ABColor.accent.opacity(0.15))
                DrawnIcon.term(ABColor.accent, size: 16)
            }
            .frame(width: ABMetric.px(32), height: ABMetric.px(32))

            Spacer().frame(width: ABMetric.px(10))

            VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                Text(title)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(ABColor.fg)
                    .lineLimit(1)
                HStack(spacing: ABMetric.px(5)) {
                    Circle()
                        .fill(connected ? ABColor.accent : ABColor.fgFaint)
                        .frame(width: ABMetric.px(6), height: ABMetric.px(6))
                    Text(statusText)
                        .font(.system(size: 10))
                        .foregroundColor(connected ? ABColor.accent : ABColor.fgDim)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            ABBarIcon(text: "🌓", description: "切换终端主题") {
                jumps.setTerminalDark(!jumps.terminalDark)
            }

            Button {
                fontMenuOpen.toggle()
            } label: {
                Text("Aa")
                    .font(.system(size: 15))
                    .foregroundColor(ABColor.fgDim)
                    .frame(width: 44, height: 44)   // 命中区 44×44（原先只有字形 + padding）
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("字号")
            .overlay(alignment: .topTrailing) {
                if fontMenuOpen {
                    VStack(alignment: .leading, spacing: 0) {
                        fontRow("缩小字号（当前 \(jumps.terminalFontSize)）") {
                            jumps.setTerminalFontSize(jumps.terminalFontSize - 1)
                        }
                        fontRow("放大字号（当前 \(jumps.terminalFontSize)）") {
                            jumps.setTerminalFontSize(jumps.terminalFontSize + 1)
                        }
                    }
                    .frame(width: ABMetric.px(180))
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .fill(ABColor.elevate))
                    .overlay(
                        RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
                    .shadow(color: .black.opacity(0.18), radius: 12, y: 6)
                    .offset(y: ABMetric.px(38))
                }
            }

            ABBarIcon(text: "⌨", description: "弹出/收起键盘") {
                NotificationCenter.default.post(name: .init("anyboot-term-focus"), object: nil)
            }
        }
        .padding(.leading, ABMetric.px(4))
        .padding(.trailing, ABMetric.px(8))
        .padding(.vertical, ABMetric.px(8))
        .background(ABColor.card)
    }

    private func fontRow(_ title: String, action: @escaping () -> Void) -> some View {
        Button {
            fontMenuOpen = false
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
        guard let server = session?.server else { return "终端" }
        return server.name.isEmpty ? server.addressLabel : server.name
    }

    private var connected: Bool { session != nil && statusMessage == nil }

    private var statusText: String {
        if let statusMessage { return statusMessage }
        guard let session else { return "未连接" }
        return "已连接 · " + ABFormat.duration(Int64(Date().timeIntervalSince(session.startedAt) * 1000))
    }

    // MARK: - 会话 pill 行

    private var sessionTabs: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: ABMetric.px(6)) {
                ForEach(TerminalSessionsRegistry.all) { item in
                    let active = item.server.id == currentId
                    HStack(spacing: 0) {
                        Text(item.server.name)
                            .font(.system(size: 11))
                            .foregroundColor(active ? ABColor.accent : ABColor.fgDim)
                            .lineLimit(1)
                            .padding(.trailing, ABMetric.px(4))
                            .onTapGesture { bind(item) }
                        Text("×")
                            .font(.system(size: 13))
                            .foregroundColor(active ? ABColor.accent : ABColor.fgFaint)
                            .padding(.horizontal, ABMetric.px(4))
                            .onTapGesture { killTarget = item }
                    }
                    .padding(.leading, ABMetric.px(10))
                    .padding(.trailing, ABMetric.px(6))
                    .padding(.vertical, ABMetric.px(4))
                    .background(
                        Capsule().fill(active ? ABColor.accent.opacity(0.15) : .clear))
                    .overlay(
                        Capsule().strokeBorder(active ? .clear : ABColor.line,
                                               lineWidth: ABMetric.hairline))
                }
            }
            .padding(.horizontal, ABMetric.px(12))
            .padding(.vertical, ABMetric.px(8))
        }
        .background(ABColor.card)
        .overlay(alignment: .top) {
            Rectangle().fill(ABColor.line).frame(height: ABMetric.hairline)
        }
    }

    // MARK: - 按键栏（8 + 4）

    private var keyBar: some View {
        VStack(spacing: ABMetric.px(4)) {
            HStack(spacing: ABMetric.px(4)) {
                keyCap("Esc") { send([0x1b]) }
                keyCap("Tab") { send([0x09]) }
                keyCap("Ctrl", armed: ctrlArmed) { ctrlArmed.toggle() }
                keyCap("/") { send(Array("/".utf8)) }
                keyCap("-") { send(Array("-".utf8)) }
                keyCap("↑") { send([0x1b, 0x5b, 0x41]) }
                keyCap("↓") { send([0x1b, 0x5b, 0x42]) }
                keyCap("|") { send(Array("|".utf8)) }
            }
            HStack(spacing: ABMetric.px(4)) {
                keyCap("←") { send([0x1b, 0x5b, 0x44]) }
                keyCap("→") { send([0x1b, 0x5b, 0x43]) }
                keyCap("Enter") { send([0x0d]) }
                keyCap("⌨") {
                    NotificationCenter.default.post(name: .init("anyboot-term-focus"), object: nil)
                }
            }
        }
        .padding(.horizontal, ABMetric.px(8))
        .padding(.vertical, ABMetric.px(8))
        .background(ABColor.card)
    }

    private func keyCap(_ label: String, armed: Bool = false,
                        onClick: @escaping () -> Void) -> some View {
        Button(action: onClick) {
            Text(label)
                .font(.system(size: 12, design: .monospaced))
                .foregroundColor(armed ? ABColor.onAccent : ABColor.fgDim)
                .frame(maxWidth: .infinity)
                .frame(height: ABMetric.px(36))
                .background(
                    RoundedRectangle(cornerRadius: ABMetric.px(10), style: .continuous)
                        .fill(armed ? ABColor.accent : ABColor.elevate))
        }
        .buttonStyle(.plain)
    }

    /// Ctrl 臂上的字母行（Ctrl 已武装才显示）
    private var lettersRow: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 0) {
                ForEach(Array("ABCDEFGHIJKLMNOPQRSTUVWXYZ".enumerated()), id: \.offset) { index, letter in
                    Button {
                        guard ctrlArmed else { return }
                        send([UInt8(index + 1)])
                    } label: {
                        Text(String(letter))
                            .font(.system(size: 13))
                            .foregroundColor(ABColor.fgDim)
                            .padding(.horizontal, ABMetric.px(12))
                            .padding(.vertical, ABMetric.px(6))
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, ABMetric.px(6))
            .padding(.vertical, ABMetric.px(4))
        }
        .background(ABColor.card)
    }

    /// 键帽写出：真网络 I/O，别在主线程做 —— 甩到后台串行队列，保按键顺序
    private func send(_ bytes: [UInt8]) {
        guard let session else { return }
        let data = Data(bytes)
        DispatchQueue.global(qos: .userInitiated).async {
            session.write(data)
        }
    }

    // MARK: - 会话建立 / 绑定

    private func ensureSession(jumpId: String, alreadyOpen: Bool) async {
        guard let server = jumps.jump(jumpId) else {
            statusMessage = "服务器不存在（可能已删除）"
            return
        }
        if let live = TerminalSessionsRegistry.get(jumpId) {
            bind(live)
            return
        }
        // 密码认证但本机没存上密码 → 先弹输入框（应急通道，本次连接有效）
        if server.isSsh && SshSessionManager.shared.needsManualPassword(server) {
            manualPassword = .init(server: server) {
                Task { await ensureSession(jumpId: jumpId, alreadyOpen: alreadyOpen) }
            }
            return
        }
        statusMessage = "正在连接 \(server.addressLabel) …"
        let fresh = TerminalSessionRef(server: server)
        do {
            if server.isSsh {
                try await fresh.startSsh()
            } else {
                try fresh.startTelnet()
            }
            TerminalSessionsRegistry.put(fresh)
            statusMessage = nil
            bind(fresh)
        } catch {
            let message = SshSessionManager.describe(error)
            statusMessage = message
            ServerModuleStore.shared.toast(message, long: true)
            if !alreadyOpen { dismiss() }
        }
    }

    /// 绑定一个会话到界面：重挂监听器、取走积攒回显、刷标签条
    private func bind(_ target: TerminalSessionRef) {
        let id = target.server.id
        NSLog("[ABTERM] bind jump=\(id) 已连接=\(target.isReady)")
        if currentId != id {
            TerminalSessionsRegistry.get(currentId)?.detach()
        }
        currentId = id
        // 下行（原生 → JS）：会话比页面活得久，每次都重挂，别喂进已经销毁的 WebView
        target.onOutput = { data in
            NotificationCenter.default.post(name: .init("anyboot-term-\(id)"), object: data)
        }
        target.onClosed = { reason in
            statusMessage = "会话已结束" + (reason.isEmpty ? "" : "：\(reason)")
            ServerModuleStore.shared.toast("会话结束" + (reason.isEmpty ? "" : "：\(reason)"),
                                           long: true)
        }
        statusMessage = nil
        // 从「文件」页跳来的话，在这儿把 cd 打进去（会话已连上、输入通道可用）
        if let dir = pendingCd, !pendingCdSent {
            pendingCdSent = true
            pendingCd = nil
            target.writeWhenReady(Data(("cd " + SftpOps.shellQuote(dir) + "\n").utf8))
        }
    }

    // MARK: - 弹层

    @ViewBuilder
    private var dialogs: some View {
        ABDialogHost(isPresented: killTarget != nil, onDismiss: { killTarget = nil }) {
            ABDialogCard("结束会话") {
                if let item = killTarget {
                    ABDialogNote("结束「\(item.server.displayName)」的终端会话吗？", color: ABColor.fg)
                    ABDialogButtons(confirmTitle: "结束",
                                    onConfirm: {
                                        killTarget = nil
                                        if item.server.id == currentId {
                                            item.detach()
                                        }
                                        TerminalSessionsRegistry.kill(item)
                                        if TerminalSessionsRegistry.all.isEmpty { dismiss() }
                                        else if let next = TerminalSessionsRegistry.all.first {
                                            bind(next)
                                        }
                                    },
                                    onDismiss: { killTarget = nil })
                }
            }
        }

        ABDialogHost(isPresented: manualPassword != nil, onDismiss: { manualPassword = nil }) {
            ABDialogCard("输入 SSH 密码") {
                if let request = manualPassword {
                    ManualPasswordBody(request: request, onDone: { manualPassword = nil })
                        .id(request.id)
                }
            }
        }
    }
}

/// 终端设置（对齐 `TerminalSettingsActivity`）：独立全屏页 —— base 底 + 顶部 card 色栏
/// （‹ + 「终端设置」），内容滚动铺满。改动即时落盘，没有「保存」按钮。
struct TerminalSettingsSheet: View {

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var jumps = JumpStore.shared

    var body: some View {
        if store.terminalSettingsOpen {
            VStack(spacing: 0) {
                // 顶部色栏（TerminalSettingsActivity:64-85）
                HStack(spacing: 0) {
                    ABBarIcon(text: "‹", description: "返回") {
                        store.terminalSettingsOpen = false
                    }
                    Text("终端设置")
                        .font(.system(size: 16))
                        .foregroundColor(ABColor.fg)
                    Spacer(minLength: 0)
                }
                .padding(ABMetric.px(8))
                .background(ABColor.card)

                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        settingsBody
                    }
                    .padding(.bottom, ABMetric.px(24))
                }
            }
            .background(ABColor.base.ignoresSafeArea())
            .transition(.move(edge: .trailing).combined(with: .opacity))
        }
    }

    private var settingsBody: some View {
        VStack(alignment: .leading, spacing: 0) {
            ABSectionTitle("终端配色")
            radio("暗色（OLED 上省电，长时间看更舒服）", on: jumps.terminalDark) {
                jumps.setTerminalDark(true)
            }
            radio("亮色", on: !jumps.terminalDark) {
                jumps.setTerminalDark(false)
            }

            ABSectionTitle("终端字号")
            HStack(spacing: 0) {
                ABChipButton(text: "−") {
                    jumps.setTerminalFontSize(jumps.terminalFontSize - 1)
                }
                Text("\(jumps.terminalFontSize)")
                    .font(.system(size: 15, weight: .bold))
                    .foregroundColor(ABColor.fg)
                    .padding(.horizontal, ABMetric.px(16))
                ABChipButton(text: "＋") {
                    jumps.setTerminalFontSize(jumps.terminalFontSize + 1)
                }
                Spacer().frame(width: ABMetric.px(10))
                ABSecondaryText("范围 \(JumpStore.fontSizeRange.lowerBound)"
                                + " - \(JumpStore.fontSizeRange.upperBound)")
            }
            .padding(.horizontal, ABMetric.px(12))
            .padding(.vertical, ABMetric.px(6))

            // 按真实配色 + 真实字号给一行样例
            Text("root@anyboot:~# uname -a")
                .font(.system(size: CGFloat(jumps.terminalFontSize), design: .monospaced))
                .foregroundColor(jumps.terminalDark
                                 ? Color(hex: 0xD8E0F0) : Color(hex: 0x1A1A1A))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, ABMetric.px(12))
                .padding(.vertical, ABMetric.px(10))
                .background(
                    RoundedRectangle(cornerRadius: ABMetric.px(8), style: .continuous)
                        .fill(jumps.terminalDark
                              ? ABColor.terminalDarkBg : ABColor.terminalLightBg))
                .padding(.horizontal, ABMetric.px(12))
                .padding(.vertical, ABMetric.px(2))

            ABSectionTitle("终端字体族")
            VStack(alignment: .leading, spacing: ABMetric.px(4)) {
                ABDialogInput(placeholder: "留空 = 内置等宽",
                              text: Binding(
                                get: { jumps.terminalFontFamily },
                                set: { jumps.setTerminalFontFamily($0) }))
                Text("填 CSS font-family，如 JetBrains Mono / Menlo；缺失自动回落等宽。返回终端页即生效。")
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, ABMetric.px(12))

            ABSectionTitle("终端行为")
            check("进入终端自动弹出键盘",
                  hint: "默认关：先看到输出，要打字时点一下终端区或 ⌨",
                  on: jumps.terminalAutoKeyboard) {
                jumps.setTerminalAutoKeyboard($0)
            }
            check("显示快捷键栏",
                  hint: "终端底部的 Esc / Tab / Ctrl / 方向键 / Enter 那一条",
                  on: jumps.terminalKeyBar) {
                jumps.setTerminalKeyBar($0)
            }

            Button {
                jumps.setTerminalDark(true)
                jumps.setTerminalFontSize(JumpStore.defaultFontSize)
                jumps.setTerminalFontFamily("")
                jumps.setTerminalAutoKeyboard(false)
                jumps.setTerminalKeyBar(true)
                store.toast("已恢复默认：暗色 · 字号 \(JumpStore.defaultFontSize) · 快捷键栏开")
            } label: {
                Text("恢复默认设置")
                    .font(.system(size: 14))
                    .foregroundColor(ABColor.accent)
                    .padding(.vertical, ABMetric.px(10))
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.plain)

            ABSecondaryText("改动即时生效：已经打开的终端页返回时会自动套用，也可以直接用终端页顶栏的 "
                            + "A- / A+ 临时调字号（同样会存下来）。")
                .padding(.horizontal, ABMetric.px(16))
                .padding(.vertical, ABMetric.px(8))
        }
    }

    /// M3 RadioButton：未选中环 = fgDim（之前恒 accent）；行 padding h8/v2（ThemeRow 223-234）
    private func radio(_ label: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: ABMetric.px(6)) {
                ZStack {
                    Circle()
                        .strokeBorder(on ? ABColor.accent : ABColor.fgDim, lineWidth: 2)
                        .frame(width: ABMetric.px(18), height: ABMetric.px(18))
                    if on {
                        Circle().fill(ABColor.accent)
                            .frame(width: ABMetric.px(9), height: ABMetric.px(9))
                    }
                }
                Text(label)
                    .font(.system(size: 14))
                    .foregroundColor(ABColor.fg)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, ABMetric.px(8))
            .padding(.vertical, ABMetric.px(2))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    /// M3 Checkbox：未选中描边 = fgDim；行 padding h8/v2（CheckRow 237-251）
    private func check(_ title: String, hint: String, on: Bool,
                       onToggle: @escaping (Bool) -> Void) -> some View {
        Button {
            onToggle(!on)
        } label: {
            HStack(alignment: .top, spacing: ABMetric.px(8)) {
                RoundedRectangle(cornerRadius: ABMetric.px(2), style: .continuous)
                    .strokeBorder(on ? ABColor.accent : ABColor.fgDim, lineWidth: 2)
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(2), style: .continuous)
                            .fill(on ? ABColor.accent : .clear))
                    .frame(width: ABMetric.px(18), height: ABMetric.px(18))
                    .padding(.top, ABMetric.px(2))
                VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                    Text(title)
                        .font(.system(size: 14))
                        .foregroundColor(ABColor.fg)
                    Text(hint)
                        .font(.system(size: 13))
                        .foregroundColor(ABColor.fgFaint)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, ABMetric.px(8))
            .padding(.vertical, ABMetric.px(2))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
