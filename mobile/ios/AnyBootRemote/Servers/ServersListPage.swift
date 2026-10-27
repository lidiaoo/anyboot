import SwiftUI
import Network

/// 服务器页（对齐 `ServersActivity.kt` 的 ServersPage / ServerCard）：
/// 搜索框 + 服务器卡片（44dp 图标块 + 名称/协议徽标 + mono 地址 + 右侧 ms 与状态点），
/// 点开三格信息与终端 / 文件 / 测试 / 编辑四钮。
struct ServersListPage: View {

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var jumps = JumpStore.shared

    @State private var confirmingDelete: JumpServer?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                searchField

                if jumps.jumps.isEmpty {
                    ABEmptyHint("还没有服务器。\n\nSSH 跳板机：外网连家里的必经之路 —— 手机先 SSH 登录它，"
                                + "再由它访问局域网内的 anyboot 电脑。\nTelnet：只有实验性终端。")
                    Button {
                        store.editTarget = nil
                        store.editOpen = true
                    } label: {
                        Text("＋ 添加服务器")
                            .font(.system(size: 14))
                            .foregroundColor(ABColor.accent)
                            .padding(.vertical, ABMetric.px(10))
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.plain)
                } else {
                    ForEach(filtered) { server in
                        ServerCardRow(server: server)
                        Spacer().frame(height: ABMetric.px(10))
                    }
                    Spacer().frame(height: ABMetric.px(4))
                }
            }
            .padding(.horizontal, ABMetric.px(20))
            .padding(.bottom, ABMetric.px(20))
        }
        .background(ABColor.base)
    }

    private var filtered: [JumpServer] {
        let query = store.search.lowercased()
        guard !query.isEmpty else { return jumps.jumps }
        return jumps.jumps.filter {
            $0.name.lowercased().contains(query) || $0.host.lowercased().contains(query)
        }
    }

    private var searchField: some View {
        HStack(spacing: ABMetric.px(8)) {
            DrawnIcon.search(ABColor.fgFaint, size: 18)
            TextField("搜索名称 / 地址", text: $store.search)
                .font(.system(size: 14))
                .foregroundColor(ABColor.fg)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
        }
        .padding(.horizontal, ABMetric.px(12))
        .padding(.vertical, ABMetric.px(10))
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.elevate))
        .overlay(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
        .padding(.top, ABMetric.px(4))
        .padding(.bottom, ABMetric.px(8))
    }
}

// MARK: - 一张服务器卡片（`ServersActivity.kt:677`）

struct ServerCardRow: View {

    let server: JumpServer

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var jumps = JumpStore.shared

    private var up: Bool { server.isSsh && SshSessionManager.shared.isUp(server.id) }
    private var expand: Bool { store.expanded.contains(server.id) }
    private var name: String { server.name.trimmingCharacters(in: .whitespaces) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: ABMetric.px(12)) {
                // 图标块：SSH 在线 = 强调色底 / 其余抬升底
                ZStack {
                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(up ? ABColor.accent.opacity(0.1) : ABColor.elevate)
                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .strokeBorder(up ? ABColor.accent.opacity(0.3) : ABColor.line,
                                      lineWidth: ABMetric.hairline)
                    DrawnIcon.serverStack(up ? ABColor.accent : ABColor.fgDim, size: 22)
                }
                .frame(width: ABMetric.px(44), height: ABMetric.px(44))

                VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                    HStack(spacing: ABMetric.px(6)) {
                        Text(name.isEmpty ? server.addressLabel : name)
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(ABColor.fg)
                            .lineLimit(1)
                        Text(server.protocolLabel)
                            .font(.system(size: 9, design: .monospaced))
                            .foregroundColor(server.isSsh ? ABColor.fgDim : ABColor.warn)
                            .padding(.horizontal, ABMetric.px(4))
                            .padding(.vertical, ABMetric.px(1))
                            .background(
                                RoundedRectangle(cornerRadius: ABMetric.px(4), style: .continuous)
                                    .fill(ABColor.elevate))
                    }
                    Text(subtitle)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundColor(ABColor.fgDim)
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                VStack(alignment: .trailing, spacing: ABMetric.px(2)) {
                    if server.lastRttMs >= 0 {
                        Text("\(Int(server.lastRttMs))ms")
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundColor(ABColor.accent)
                    }
                    HStack(spacing: ABMetric.px(4)) {
                        Circle()
                            .fill(statusColor)
                            .frame(width: ABMetric.px(6), height: ABMetric.px(6))
                        Text(statusLabel)
                            .font(.system(size: 10))
                            .foregroundColor(statusColor)
                    }
                }
            }

            if expand {
                Spacer().frame(height: ABMetric.px(14))
                Rectangle()
                    .fill(ABColor.line)
                    .frame(height: ABMetric.hairline)
                Spacer().frame(height: ABMetric.px(12))

                HStack(spacing: ABMetric.px(8)) {
                    infoCell("用户", name.isEmpty ? "root" : (server.username.isEmpty ? "root" : server.username))
                    infoCell("认证", server.authLabel)
                    infoCell("端口", "\(server.port)")
                }
                if !server.note.trimmingCharacters(in: .whitespaces).isEmpty {
                    ABSecondaryText("备注：" + server.note).padding(.top, ABMetric.px(8))
                }
                if server.isSsh {
                    ABSecondaryText("经由此服务器的电脑：\(store.computersVia(server.id)) 台")
                        .padding(.top, ABMetric.px(2))
                }
                if server.isSsh && up {
                    ABSecondaryText("SSH 会话在线（隧道与终端共享）")
                        .padding(.top, ABMetric.px(2))
                }
                Spacer().frame(height: ABMetric.px(12))

                HStack(spacing: ABMetric.px(8)) {
                    if server.isSsh {
                        cardAction("终端", filled: true) {
                            store.openTerminal(jumpId: server.id)
                        }
                        cardAction("文件", filled: false) {
                            store.page = .files
                            FilesStackBridge.open(server: server)
                        }
                    }
                    cardAction("测试", filled: false) { test() }
                    cardAction("编辑", filled: false) {
                        store.editTarget = server
                        store.editOpen = true
                    }
                }
            }
        }
        .padding(ABMetric.px(16))
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(16), style: .continuous)
                .fill(ABColor.card))
        .overlay(
            RoundedRectangle(cornerRadius: ABMetric.px(16), style: .continuous)
                .strokeBorder(expand ? ABColor.accent.opacity(0.4) : ABColor.line,
                              lineWidth: ABMetric.hairline))
        .contentShape(Rectangle())
        .onTapGesture { store.toggleExpanded(server.id) }
    }

    private var subtitle: String {
        var text = ""
        if name.isEmpty {
            text += "用户 " + (server.username.isEmpty ? "root" : server.username)
            text += " · 端口 \(server.port)"
        } else {
            text += server.addressLabel
        }
        if !server.isSsh { text += "（实验性终端）" }
        return text
    }

    private var statusColor: Color {
        if server.isSsh && up { return ABColor.accent }
        if !server.isSsh { return ABColor.warn }
        return ABColor.fgFaint
    }

    private var statusLabel: String {
        if server.isSsh && up { return "在线" }
        if !server.isSsh { return "实验性" }
        return "未连接"
    }

    private func infoCell(_ label: String, _ value: String) -> some View {
        VStack(spacing: ABMetric.px(1)) {
            Text(value)
                .font(.system(size: 12))
                .foregroundColor(ABColor.fg)
                .lineLimit(1)
                .padding(.horizontal, ABMetric.px(6))
            Text(label)
                .font(.system(size: 9))
                .foregroundColor(ABColor.fgFaint)
        }
        .padding(.vertical, ABMetric.px(8))
        .frame(maxWidth: .infinity)
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(8), style: .continuous)
                .fill(ABColor.elevate.opacity(0.6)))
    }

    private func cardAction(_ label: String, filled: Bool,
                            onClick: @escaping () -> Void) -> some View {
        Button(action: onClick) {
            VStack(spacing: ABMetric.px(2)) {
                actionIcon(label, filled: filled)
                Text(label)
                    .font(.system(size: 9, weight: filled ? .semibold : .regular))
                    .foregroundColor(filled ? ABColor.onAccent
                                     : (label == "编辑" ? ABColor.fgDim : ABColor.fg))
            }
            .frame(maxWidth: .infinity)
            .frame(height: ABMetric.px(40))
            .background(
                RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                    .fill(filled ? ABColor.accent : ABColor.elevate))
            .overlay(
                RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                    .strokeBorder(filled ? .clear : ABColor.line, lineWidth: ABMetric.hairline))
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private func actionIcon(_ label: String, filled: Bool) -> some View {
        switch label {
        case "终端": DrawnIcon.term(filled ? ABColor.onAccent : ABColor.fg, size: 15)
        case "文件": DrawnIcon.folder(ABColor.fg, size: 15)
        case "测试": DrawnIcon.refresh(ABColor.fg, size: 15)
        default: DrawnIcon.pencil(ABColor.fgDim, size: 15)
        }
    }

    /// 测试连接：SSH 握手 / Telnet TCP，量往返 —— `ServersActivity.kt:903`
    private func test() {
        // 密码认证但本机没存上密码 → 先弹输入框（应急通道，本次连接有效）
        if server.isSsh && SshSessionManager.shared.needsManualPassword(server) {
            store.manualPassword = .init(server: server, onReady: { test() })
            return
        }
        store.toast("正在测试 \(server.addressLabel) …")
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
                store.toast("连接成功（\(ms)ms）", long: true)
            } catch {
                store.toast(SshSessionManager.describe(error), long: true)
            }
        }
    }
}

/// 文件页要跳的服务器：卡片上点「文件」时把选中的服务器塞进去
@MainActor
enum FilesStackBridge {
    static func open(server: JumpServer) {
        let store = FilesStore.shared
        if store.serverId != server.id {
            store.serverId = server.id
            store.clearClip()
            store.gotoDir("/", force: true)
        } else {
            store.reloadDir()
        }
    }
}

/// Telnet 只有 TCP 可达性可测
enum TelnetProbe {
    static func check(host: String, port: Int) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            var done = false
            let finish: (Error?) -> Void = { error in
                guard !done else { return }
                done = true
                if let error { continuation.resume(throwing: error) } else { continuation.resume() }
            }
            let connection = NWConnection(host: NWEndpoint.Host(host),
                                          port: NWEndpoint.Port(rawValue: UInt16(port)) ?? 23,
                                          using: .tcp)
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    connection.cancel()
                    finish(nil)
                case .failed(let error):
                    connection.cancel()
                    finish(error)
                case .cancelled:
                    finish(NSError(domain: "telnet", code: -1))
                default:
                    break
                }
            }
            connection.start(queue: .global())
            DispatchQueue.global().asyncAfter(deadline: .now() + 8) {
                connection.cancel()
                finish(NSError(domain: "telnet", code: -2,
                               userInfo: [NSLocalizedDescriptionKey: "连接超时"]))
            }
        }
    }
}
