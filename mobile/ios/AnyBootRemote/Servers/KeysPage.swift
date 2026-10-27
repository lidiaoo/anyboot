import SwiftUI
import Citadel
import Crypto

/// 密钥页（对齐 `ServersActivity.kt:1431` KeysPage）：密钥卡 + 底部虚线提示框；
/// 生成 / 导入从顶栏 FAB 菜单进。
struct KeysPage: View {

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var jumps = JumpStore.shared

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if jumps.keys.isEmpty {
                    ABEmptyHint("还没有密钥。导入已有的私钥，或直接在本机生成一对 —— "
                                + "把公钥放到服务器上（authorized_keys）就能免密登录。")
                }
                ForEach(jumps.keys) { entry in
                    KeyCardRow(entry: entry)
                    Spacer().frame(height: ABMetric.px(10))
                }
                ABDashedHint("公钥需追加到服务器 ~/.ssh/authorized_keys\n私钥加密存本机，永不上传")
                Spacer().frame(height: ABMetric.px(12))
            }
            .padding(.horizontal, ABMetric.px(20))
            .padding(.bottom, ABMetric.px(20))
        }
        .background(ABColor.base)
        .fileImporter(isPresented: $store.keyImportOpen,
                      allowedContentTypes: [.data], allowsMultipleSelection: false) { result in
            if case .success(let urls) = result, let url = urls.first { store.importKey(url) }
        }
        .overlay { deleteDialog }
    }

    @ViewBuilder
    private var deleteDialog: some View {
        ABDialogHost(isPresented: store.deleteKeyTarget != nil,
                     onDismiss: { store.deleteKeyTarget = nil }) {
            ABDialogCard("删除密钥") {
                if let entry = store.deleteKeyTarget {
                    ABDialogNote("确定删除「\(entry.name)」吗？私钥不可恢复。", color: ABColor.fg)
                    ABDialogButtons(confirmTitle: "删除", confirmColor: ABColor.bad,
                                    onConfirm: {
                                        store.deleteKeyTarget = nil
                                        JumpStore.shared.removeKey(entry.id)
                                    },
                                    onDismiss: { store.deleteKeyTarget = nil })
                }
            }
        }
    }

    /// 生成 / 导入的入口给顶栏 FAB 用（导入走 store.keyImportOpen）
    func startImport() { store.keyImportOpen = true }
}

// MARK: - 一张密钥卡（`:1473`）

struct KeyCardRow: View {

    let entry: SshKeyEntry

    @ObservedObject private var store = ServerModuleStore.shared

    private var inUse: Int { JumpStore.shared.keysInUse(entry.id) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: ABMetric.px(12)) {
                ZStack {
                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(inUse > 0 ? ABColor.accent.opacity(0.1) : ABColor.elevate)
                    DrawnIcon.key(inUse > 0 ? ABColor.accent : ABColor.fgDim, size: 20)
                }
                .frame(width: ABMetric.px(40), height: ABMetric.px(40))

                VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                    HStack(spacing: ABMetric.px(6)) {
                        Text(entry.name.isEmpty ? "未命名密钥" : entry.name)
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(ABColor.fg)
                            .lineLimit(1)
                        if inUse > 0 {
                            Text("\(inUse) 台使用")
                                .font(.system(size: 9, design: .monospaced))
                                .foregroundColor(ABColor.accent)
                                .padding(.horizontal, ABMetric.px(5))
                                .padding(.vertical, ABMetric.px(1))
                                .background(
                                    RoundedRectangle(cornerRadius: ABMetric.px(4), style: .continuous)
                                        .fill(ABColor.accent.opacity(0.1)))
                        }
                    }
                    Text(entry.algorithm
                         + (entry.fingerprint.isEmpty ? "" : " · " + Self.shorten(entry.fingerprint)))
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundColor(ABColor.fgDim)
                        .lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            Spacer().frame(height: ABMetric.px(12))
            HStack(spacing: ABMetric.px(8)) {
                plainButton("复制公钥") {
                    if entry.publicKey.isEmpty {
                        store.toast("这把密钥没有记录公钥")
                    } else {
                        UIPasteboard.general.string = entry.publicKey
                        store.toast("已复制")
                    }
                }
                plainButton("删除") {
                    if inUse > 0 {
                        store.toast("有 \(inUse) 台服务器在用这把密钥，先改它们的认证方式", long: true)
                    } else {
                        store.deleteKeyTarget = entry
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
                .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
    }

    private func plainButton(_ title: String, onClick: @escaping () -> Void) -> some View {
        Button(action: onClick) {
            Text(title)
                .font(.system(size: 12))
                .foregroundColor(title == "删除" ? ABColor.fgDim : ABColor.fg)
                .frame(maxWidth: .infinity)
                .frame(height: ABMetric.px(36))
                .background(
                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(ABColor.elevate))
                .overlay(
                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
        }
        .buttonStyle(.plain)
    }

    /// `SHA256:abcd…wxyz` 形式的指纹缩写 —— `:1571`
    static func shorten(_ fingerprint: String) -> String {
        let clean = fingerprint.hasPrefix("SHA256:")
            ? String(fingerprint.dropFirst("SHA256:".count)) : fingerprint
        if clean.count <= 12 { return fingerprint }
        return "SHA256:" + clean.prefix(4) + "…" + clean.suffix(4)
    }
}

// MARK: - 生成密钥（`:1614` 弹窗 + 落地）

struct GenerateKeyDialog: View {

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var jumps = JumpStore.shared

    @State private var name = ""
    @State private var ed25519 = true
    @State private var busy = false

    var body: some View {
        ABDialogHost(isPresented: store.generateKeyOpen,
                     onDismiss: { store.generateKeyOpen = false }) {
            ABDialogCard("生成密钥对") {
                ABDialogInput(placeholder: "名称，如 home-ed25519", text: $name)
                HStack(spacing: ABMetric.px(6)) {
                    radio("Ed25519（推荐）", on: ed25519) { ed25519 = true }
                    Spacer(minLength: ABMetric.px(8))
                    radio("RSA 4096", on: !ed25519) { ed25519 = false }
                }
                if busy {
                    HStack(spacing: ABMetric.px(8)) {
                        ProgressView().scaleEffect(0.7)
                            .frame(width: ABMetric.px(16), height: ABMetric.px(16))
                        ABSecondaryText("正在生成 …")
                    }
                }
                ABDialogButtons(confirmTitle: "生成",
                                onConfirm: {
                                    let title = name.trimmingCharacters(in: .whitespaces)
                                    store.generateKeyOpen = false
                                    generate(title, ed25519: ed25519)
                                },
                                onDismiss: { store.generateKeyOpen = false })
            }
        }
    }

    private func radio(_ label: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: ABMetric.px(6)) {
                Circle()
                    .strokeBorder(ABColor.accent, lineWidth: 2)
                    .background(Circle().fill(on ? ABColor.accent : .clear))
                    .frame(width: ABMetric.px(18), height: ABMetric.px(18))
                Text(label)
                    .font(.system(size: 13))
                    .foregroundColor(ABColor.fg)
            }
        }
        .buttonStyle(.plain)
    }

    /// Ed25519 在 iOS 上可用（Crypto 内置 Curve25519），RSA 也能生成并导出 OpenSSH 私钥。
    private func generate(_ title: String, ed25519: Bool) {
        busy = true
        Task {
            do {
                let entry = try KeyFactory.generate(name: title.isEmpty ? "key-\(jumps.keys.count)" : title,
                                                   ed25519: ed25519)
                JumpStore.shared.upsert(entry)
                busy = false
                store.toast("已生成 \(entry.algorithm) 密钥「\(entry.name)」—— 复制公钥放到服务器的"
                            + " authorized_keys", long: true)
            } catch {
                busy = false
                store.toast("生成失败：" + error.localizedDescription, long: true)
            }
        }
    }
}
