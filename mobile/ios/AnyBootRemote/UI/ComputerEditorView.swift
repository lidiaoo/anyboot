import SwiftUI

/// 编辑器的「呈现令牌」。
///
/// 为什么必须有它：`ComputerEditorView` 的字段全是 `@State`，而 `@State` 的初始值
/// 只在**视图身份第一次建立**时生效（`init` 里写的 `State(initialValue:)` 之后就被忽略）。
/// 用 `.sheet(isPresented:)` 时身份是复用的 —— 第二次打开编辑器（尤其是上一次开的
/// 是「添加电脑」，那份 draft 全是空串）就会沿用上一次的状态，表现为**回显为空**，
/// 或者显示的是上一台电脑的内容。
///
/// 每次呈现发一个新 UUID 当身份（`.sheet(item:)` + `.id()`），`init` 的初始值才一定生效。
struct ComputerEditorTarget: Identifiable {
    let id = UUID()
    let profile: ServerProfile?
    /// 首启自动弹出（Android MainActivity「连接 AnyBoot 服务端」）：不可点外关闭、不显示取消
    var firstRun: Bool = false
}

/// 编辑表单里的一个地址条目：地址 + 这块（这几块）网卡的 MAC。
/// （对应 Android `ServerEditForm.AddrEntry`）
struct EditorAddrEntry: Equatable {
    var address: String
    var macs: [String]
}

/// 电脑编辑表单 —— **一比一复刻 Android `ServerEditForm` / `EditSheet`**。
///
/// 外壳（底部 sheet：把手 → 标题行+关闭圆钮 → 可滚表单（≤62% 屏高）→ 分隔线
/// → 底部动作栏「删除 | 全宽保存」）与 Android `ComputerListActivity.EditSheet` 同构；
/// 表单主体的分区、顺序、文案、配色与 Android `ServerEditFormBody` 逐项对应：
/// 电脑名称 / 快速识别 / 已添加的地址（结构化卡片）/ 手动添加 / token（已存回显打码）/
/// 远程配对（三区块常驻卡片 + 条目行）/ 连接策略（折叠，默认收起）/
/// 局域网电源路由（折叠，默认收起）。
struct ComputerEditorView: View {

    enum Result {
        case save(ServerProfile)
        case delete(String)
        case cancel
    }

    let profile: ServerProfile?
    /// 首启自动弹出（Android「连接 AnyBoot 服务端」）：标题三态、不显示取消
    var firstRun: Bool = false
    let onFinish: (Result) -> Void

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var jumps = JumpStore.shared

    // ---- 基本字段 ----
    @State private var draft: ServerProfile
    @State private var name: String
    @State private var token: String
    @State private var jumpServerId: String
    @State private var tokenShown = false

    // ---- 地址（结构化列表，Android AddrEntry 同构） ----
    @State private var addressEntries: [EditorAddrEntry]
    @State private var quickText = ""
    @State private var quickHint = ""
    @State private var newHost = ""
    @State private var newMacs = ""
    /// 粘进来的原文（token 只在 URL 里，归一化后就丢了，所以留一份原文）
    @State private var pastedRaw: String

    // ---- 远程配对 ----
    @State private var remoteText = ""
    @State private var remoteHint = ""

    // ---- 折叠区（默认收起 —— 配一次就不动） ----
    @State private var connFoldOpen = false
    @State private var powerFoldOpen = false

    // ---- 探测超时（界面秒文本；空 = 继承） ----
    @State private var probeTimeoutText: String
    @State private var routeTimeoutTexts: [String: String]
    @State private var blockTimeoutTexts: [String: String]

    // ---- 动作 ----
    @State private var showScanner = false
    @State private var showDelete = false
    @State private var emptyHintShown = false

    // ---- 表单内容高度（量出来后让 sheet 贴着内容包住，别在动作栏下面留空白） ----
    @State private var formContentHeight: CGFloat = 0

    private var editing: Bool { profile != nil }

    /// Android ROUTE_LABELS_ZH（编辑器里通道名的口径与列表页不同，以此为准）
    private static let routeNames: [String: String] = [
        ServerProfile.routeLan: "直连",
        ServerProfile.routeJump: "跳板机",
        ServerProfile.routeTunnel: "Cloudflare 隧道",
        ServerProfile.routeTS: "Tailscale 直连",
        ServerProfile.routeFunnel: "Tailscale Funnel",
        ServerProfile.routeTSNet: "Tailscale Funnel（内建）",
    ]

    init(profile: ServerProfile?, firstRun: Bool = false,
         onFinish: @escaping (Result) -> Void) {
        self.profile = profile
        self.firstRun = firstRun
        self.onFinish = onFinish
        let base = profile ?? ServerProfile()
        // 新建 = 预填四条默认电源策略（全关，用户 2026-10-31）；编辑 = 档案里已存的
        var seeded = base
        if profile == nil {
            seeded.powerRoutes = ServerProfile.defaultPowerRoutes()
        }
        _draft = State(initialValue: seeded)
        _name = State(initialValue: base.name)
        _token = State(initialValue: base.token)
        _jumpServerId = State(initialValue: base.jumpServerId)
        _addressEntries = State(initialValue:
            base.addresses.map { EditorAddrEntry(address: $0, macs: base.macsForHost($0)) })
        _pastedRaw = State(initialValue: base.addressText)
        _probeTimeoutText = State(initialValue:
            base.probeTimeoutMs > 0 ? Self.secondsText(base.probeTimeoutMs) : "")
        var routeTexts: [String: String] = [:]
        for (kind, ms) in base.routeTimeoutsMs where ms > 0 {
            routeTexts[kind] = Self.secondsText(ms)
        }
        _routeTimeoutTexts = State(initialValue: routeTexts)
        var blockTexts: [String: String] = [:]
        for (block, ms) in base.blockTimeoutsMs where ms > 0 {
            blockTexts[block] = Self.secondsText(ms)
        }
        _blockTimeoutTexts = State(initialValue: blockTexts)
    }

    /// 毫秒 → 界面秒文本（整秒不带小数点，省得「2.0」看着别扭）
    private static func secondsText(_ ms: Int) -> String {
        let seconds = Double(ms) / 1000.0
        if seconds == seconds.rounded() { return String(Int(seconds)) }
        return String(format: "%.1f", seconds)
    }

    /// 超时输入统一清洗：只收数字与一个小数点，最多 5 位
    private static func cleanedTimeout(_ raw: String) -> String {
        var dot = false
        var out = ""
        for ch in raw {
            if ch.isNumber {
                out.append(ch)
            } else if (ch == "." || ch == "。") && !dot {
                out.append(".")
                dot = true
            }
        }
        return String(out.prefix(5))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // 把手（设计稿 36×4 圆头）
            HStack {
                Spacer()
                Capsule()
                    .fill(ABColor.lineStrong)
                    .frame(width: 36, height: 4)
                    .padding(.vertical, 8)
                Spacer()
            }
            // 标题行：标题 + 关闭圆钮（首启标题「连接 AnyBoot 服务端」且无关闭钮 ——
            // 对齐 Android EditDialog：onDismissRequest if (!request.firstRun)）
            HStack(spacing: 8) {
                Text(editing ? "编辑这台电脑"
                     : (firstRun ? "连接 AnyBoot 服务端" : "添加电脑"))
                    .font(.system(size: 15, weight: .bold))
                    .foregroundColor(ABColor.fg)
                    .lineLimit(1)
                Spacer()
                if !firstRun {
                    Button {
                        onFinish(.cancel)
                        dismiss()
                    } label: {
                        ZStack {
                            Circle().fill(ABColor.elevate)
                            Image(systemName: "xmark")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundColor(ABColor.fgDim)
                        }
                        .frame(width: 30, height: 30)
                        .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("关闭")
                }
            }
            .padding(.leading, 18)
            .padding(.trailing, 8)
            .padding(.bottom, 10)

            // 表单自己带滚动：高度上限 62% 屏高（与 Android EditSheet 同算法）；
            // 内容量高用：sheet 的自定义 detent 需要知道表单实际有多高
            ScrollView {
                formBody
                    .padding(.horizontal, 18)
                    .background(GeometryReader { geo in
                        Color.clear.preference(
                            key: EditorFormHeightKey.self, value: geo.size.height)
                    })
            }
            .frame(maxHeight: UIScreen.main.bounds.height * 0.62)
            .onPreferenceChange(EditorFormHeightKey.self) { height in
                formContentHeight = height
                guard height > 0 else { return }
                let cap = UIScreen.main.bounds.height * 0.62
                EditorSheetDetent.heightOverride =
                    EditorSheetDetent.chromeHeight + min(height, cap)
            }

            // 底部动作栏与滚动内容之间画一条线：不然内容滚到一半被裁，
            // 看着像被按钮压住了
            Rectangle()
                .fill(ABColor.line)
                .frame(height: 1)

            // 底部动作栏：删除放左边单独一格（跟保存不是一类动作，混在右边易误点）；
            // 添加模式没有可删的对象，整格不出现。
            HStack(spacing: 8) {
                if editing {
                    Button {
                        showDelete = true
                    } label: {
                        ZStack {
                            RoundedRectangle(cornerRadius: 11, style: .continuous)
                                .fill(ABColor.bad.opacity(0.1))
                            RoundedRectangle(cornerRadius: 11, style: .continuous)
                                .strokeBorder(ABColor.bad.opacity(0.28))
                            Image(systemName: "trash")
                                .font(.system(size: 14))
                                .foregroundColor(ABColor.bad)
                        }
                        .frame(width: 52, height: 40)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("删除这台电脑")
                }
                Button {
                    save()
                } label: {
                    ZStack {
                        RoundedRectangle(cornerRadius: 11, style: .continuous)
                            .fill(ABColor.accent)
                        Text("保存")
                            .font(.system(size: 13.5, weight: .semibold))
                            .foregroundColor(ABColor.onAccent)
                    }
                    .frame(maxWidth: .infinity)
                    .frame(height: 40)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("editor.save")
            }
            .padding(.horizontal, 14)
            .padding(.top, 8)
            .padding(.bottom, 12)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(ABColor.card.ignoresSafeArea())
        .presentationDetents([.custom(EditorSheetDetent.self)])
        .presentationDragIndicator(.hidden)
        .interactiveDismissDisabled(firstRun)
        .confirmationDialog("确定删除这台电脑吗？", isPresented: $showDelete, titleVisibility: .visible) {
            Button("删除", role: .destructive) {
                if let id = profile?.id { onFinish(.delete(id)) }
                dismiss()
            }
            Button("取消", role: .cancel) {}
        }
        .sheet(isPresented: $showScanner) {
            QRScannerView { code in
                remoteText = code
                applyRemoteHint()
            }
        }
        .abToast($emptyHintShown, title: "请填电脑地址", kind: .failure)
    }

    // MARK: - 表单主体（Android ServerEditFormBody 逐段对应）

    private var formBody: some View {
        VStack(alignment: .leading, spacing: 0) {
            // ---- 电脑名称 ----
            fieldLabel("电脑名称（同一台电脑的不同网卡/地址，用名字区分）")
            TextField("例如：有线网卡 / 5G WiFi（可留空，用地址兜底）", text: $name)
                .autocorrectionDisabled()
                .abField()
                .accessibilityIdentifier("editor.name")

            // ---- 快速识别 ----
            fieldLabel("快速识别（整段粘进来，自动拆出地址 / 端口 / MAC 加进下面）")
            TextField("192.168.0.120:12300 AA:BB:CC:DD:EE:FF 台式机有线\n192.168.0.121 11-22-33-44-55-66（5G WiFi）",
                      text: $quickText, axis: .vertical)
                .lineLimit(2...4)
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .abField()
                .accessibilityIdentifier("editor.address")
            HStack(spacing: 8) {
                Button {
                    recognizeQuick()
                } label: {
                    Text("识别并加入")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.accent)
                }
                .buttonStyle(.plain)
                Text(quickHint.isEmpty ? "换行 / 空格 / 逗号都能分开；认不出的备注自动忽略" : quickHint)
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.top, 2)

            // ---- 已添加的地址（结构化卡片） ----
            fieldLabel("已添加的地址（第一个用来连接，其余参与测试连接和唤醒）")
            if addressEntries.isEmpty {
                Text("还没加地址 —— 上面粘一段，或者下面手动填一个。")
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
            }
            ForEach(Array(addressEntries.enumerated()), id: \.offset) { index, entry in
                addressCard(index: index, entry: entry)
            }

            // ---- 手动添加 ----
            fieldLabel("手动添加（地址 + 多个 MAC；地址已存在时只追加 MAC）")
            TextField("地址（192.168.0.120 或 192.168.0.120:12300）", text: $newHost)
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .font(.system(size: 13))
                .abField()
            HStack(spacing: 8) {
                TextField("MAC（可多个，空格隔开）", text: $newMacs, prompt:
                    Text("AA:BB:CC:DD:EE:FF").font(.system(size: 12)))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .font(.system(size: 13))
                    .abField()
                Button {
                    addManualAddress()
                } label: {
                    Text("添加")
                        .font(.system(size: 13))
                        .foregroundColor(ABColor.accent)
                }
                .buttonStyle(.plain)
            }
            .padding(.top, 6)

            // ---- 访问令牌 ----
            fieldLabel("访问令牌 token（服务端设了才要填）")
            if !savedToken.isEmpty {
                // 已存令牌回显：默认打码，小眼睛切换明文（重开表单回到打码）
                HStack(spacing: 4) {
                    Text(tokenShown ? savedToken : maskToken(savedToken))
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundColor(tokenShown ? ABColor.fg : ABColor.fgDim)
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Button {
                        tokenShown.toggle()
                    } label: {
                        Text(tokenShown ? "隐藏" : "显示")
                            .font(.system(size: 11))
                            .foregroundColor(ABColor.accent)
                    }
                    .buttonStyle(.plain)
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(ABColor.card))
                .padding(.bottom, 6)
            }
            TextField("留空保持不变；要替换就填新令牌", text: $token)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .abField()
                .accessibilityIdentifier("editor.token")

            // ---- 远程配对 ----
            fieldLabel("远程配对（三个区块，每个区块可以挂多条；同类型配一条不会顶掉另一条）")
            TextField("anyboot://pair?v=2&… （配对码）；也可直接粘 https://xxx.trycloudflare.com / http://100.x.y.z:12300",
                      text: Binding(get: { remoteText }, set: { remoteText = $0; applyRemoteHint() }),
                      axis: .vertical)
                .lineLimit(1...3)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .abField()
            HStack(spacing: 4) {
                Button {
                    showScanner = true
                } label: {
                    Text("📷 扫码读码")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.accent)
                }
                .buttonStyle(.plain)
                Button {
                    clearPairs()
                } label: {
                    Text(hasPairs ? "全部清空" : "清空")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.fgDim)
                }
                .buttonStyle(.plain)
                Text(remoteHint.isEmpty ? "扫码或粘贴后自动加入对应区块；同一条会更新而不是重复" : remoteHint)
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .lineLimit(2)
            }
            .padding(.top, 2)

            // 三个区块逐块列出：区块标题 + 条数 + 「＋ 添加」，行里是地址 / lane / 来源 / 令牌 / 超时
            ForEach(RemotePair.blockIds, id: \.self) { block in
                blockCard(block)
            }
            Text("保存后每个区块里的配对各自独立探测、各自计时；清空全部保存 = 解除配对。"
                + "配对码等同密码，别转发给别人。")
                .font(.system(size: 11))
                .foregroundColor(ABColor.fgFaint)
                .padding(.top, 6)

            // ---- 连接策略（折叠，默认收起） ----
            foldHeader(title: "连接策略",
                       summary: connSummary,
                       open: connFoldOpen) { connFoldOpen.toggle() }
            if connFoldOpen {
                connFoldContent
            }

            // ---- 局域网电源路由（折叠，默认收起） ----
            foldHeader(title: "局域网电源路由",
                       summary: draft.powerRoutes.isEmpty ? "未配置" : "\(draft.powerRoutes.count) 条",
                       open: powerFoldOpen) { powerFoldOpen.toggle() }
            if powerFoldOpen {
                powerFoldContent
            }

            Spacer(minLength: 4)
        }
        .padding(.top, 2)
    }

    // MARK: - 地址卡片 / 手动添加 / 快速识别

    private func addressCard(index: Int, entry: EditorAddrEntry) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                Text(entry.address)
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.fg)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if index == 0 {
                    Text("连接用")
                        .font(.system(size: 10))
                        .foregroundColor(ABColor.accent)
                }
                Button {
                    addressEntries.remove(at: index)
                } label: {
                    Text("✕")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.bad)
                }
                .buttonStyle(.plain)
            }
            if entry.macs.isEmpty {
                Text("没配 MAC —— 局域网电源发不出去")
                    .font(.system(size: 10))
                    .foregroundColor(ABColor.fgFaint)
            } else {
                ForEach(entry.macs, id: \.self) { mac in
                    HStack(spacing: 4) {
                        Text(mac)
                            .font(.system(size: 10))
                            .foregroundColor(ABColor.fgDim)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Button {
                            addressEntries[index].macs.removeAll { $0 == mac }
                        } label: {
                            Text("✕")
                                .font(.system(size: 11))
                                .foregroundColor(ABColor.fgFaint)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .padding(8)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(ABColor.card))
        .padding(.top, 6)
    }

    /// 快速识别框 → 地址列表（粘上来的整段文字里，备注自动忽略）
    private func recognizeQuick() {
        let raw = quickText.trimmingCharacters(in: .whitespacesAndNewlines)
        if raw.isEmpty {
            quickHint = "先粘点东西进来，比如 192.168.0.120:12300 AA:BB:CC:DD:EE:FF"
            return
        }
        let parsed = ServerProfile.parseAddressInput(raw, strict: true)
        if parsed.hosts.isEmpty {
            quickHint = "没认出地址 —— 要有 IP 或主机名，比如 192.168.0.120"
            return
        }
        var added = 0
        for host in parsed.hosts {
            if upsertAddress(host, macs: parsed.macs[host] ?? []) { added += 1 }
        }
        let macCount = parsed.macs.values.reduce(0) { $0 + $1.count }
        quickHint = "认出 \(parsed.hosts.count) 个地址、\(macCount) 个 MAC"
            + (added < parsed.hosts.count ? "（其余本来就有，只并了 MAC）" : "")
        // 整条 URL 粘进来时顺手把 token 收了
        let found = ServerProfile.token(fromURL: raw)
        if !found.isEmpty && token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            token = found
        }
        pastedRaw += "\n" + raw
        quickText = ""
    }

    /// 往地址列表里加一个；已存在就只并 MAC。返回是否新加了一个地址。
    @discardableResult
    private func upsertAddress(_ address: String, macs: [String]) -> Bool {
        if let index = addressEntries.firstIndex(where: { $0.address == address }) {
            let merged = (addressEntries[index].macs + macs).removingDuplicates()
            addressEntries[index].macs = merged
            return false
        }
        addressEntries.append(EditorAddrEntry(address: address, macs: macs.removingDuplicates()))
        return true
    }

    /// 手动添加：地址 + 可选的多个 MAC
    private func addManualAddress() {
        let address = newHost.trimmingCharacters(in: .whitespacesAndNewlines)
        if address.isEmpty {
            quickHint = "地址不对：填 IP 或主机名，端口可带可不带"
            return
        }
        let macs = newMacs
            .split(whereSeparator: { " \t,;，；\n\r".contains($0) })
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        let added = upsertAddress(address, macs: macs)
        quickHint = added ? "已添加 \(address)" : "地址已存在，只追了 MAC：\(address)"
        newHost = ""
        newMacs = ""
    }

    // MARK: - 远程配对

    private var savedToken: String { profile?.token ?? "" }

    private var hasPairs: Bool {
        draft.pairs.contains { !$0.addr.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    }

    /// 令牌打码：前 4 后 4 可见，其余 •；太短的整串打码
    private func maskToken(_ value: String) -> String {
        let text = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty { return "" }
        if text.count <= 12 { return String(repeating: "•", count: max(6, text.count)) }
        return String(text.prefix(4)) + "••••••••••" + String(text.suffix(4))
    }

    /// 粘贴 / 扫码之后的分拣：解析 → 追加或更新一条配对（逐字即时，无独立「添加」钮）
    private func applyRemoteHint() {
        let raw = remoteText.trimmingCharacters(in: .whitespacesAndNewlines)
        if raw.isEmpty {
            remoteHint = ""
            return
        }
        let parsed = ServerProfile.parsePair(raw)
        guard parsed.ok else {
            remoteHint = parsed.error.isEmpty ? "没认出配对码" : parsed.error
            return
        }
        let existed = draft.pairs.contains { $0.key == RemotePair(kind: parsed.kind, addr: parsed.tunnel).key }
        if draft.upsertPair(parsed) == nil {
            remoteHint = "最多 \(ServerProfile.maxPairs) 条配对 —— 先删掉几条再加"
            return
        }
        let label = RemotePair.blockLabel(parsed.kind)
        remoteHint = (existed ? "已更新「\(label)」里的同一条配对：" : "已加入「\(label)」：") + parsed.tunnel
        remoteText = ""
    }

    /// 清空所有配对（「全部清空」按钮）：保存即解绑
    private func clearPairs() {
        draft.pairs.removeAll()
        remoteText = ""
        remoteHint = "已清空 —— 点「保存」即解除配对（地址与配对令牌一起删掉）"
    }

    /// 一个区块卡片：常驻（空的时候也在，显示「未配」+ 空态文案）
    private func blockCard(_ block: String) -> some View {
        let items = draft.pairsOfBlock(block, enabledOnly: false)
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 4) {
                Text(RemotePair.blockLabel(block))
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(ABColor.fg)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(items.isEmpty ? "未配" : "\(items.count) 条")
                    .font(.system(size: 10))
                    .foregroundColor(ABColor.fgFaint)
                Button {
                    addPair(block)
                } label: {
                    Text("＋ 添加")
                        .font(.system(size: 11))
                        .foregroundColor(ABColor.accent)
                }
                .buttonStyle(.plain)
            }
            if items.isEmpty {
                Text("这个区块还没有配对 —— 上面粘一条配对码，或点「＋ 添加」手填")
                    .font(.system(size: 10))
                    .foregroundColor(ABColor.fgFaint)
            }
            ForEach(items) { pair in
                PairEditRowView(
                    pair: pair,
                    addr: pairTextBinding(pair, \.addr),
                    token: pairTextBinding(pair, \.token),
                    timeoutText: pairTimeoutBinding(pair),
                    enabled: pairBoolBinding(pair),
                    onMove: { movePair(pair.id, by: $0) },
                    onRemove: { draft.pairs.removeAll { $0.id == pair.id } }
                )
            }
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 7)
        .background(RoundedRectangle(cornerRadius: 11, style: .continuous).fill(ABColor.card))
        .padding(.top, 8)
    }

    /// 在区块末尾加一行空条目（地址留空 = 保存时丢弃）
    private func addPair(_ kind: String) {
        if draft.pairs.count >= ServerProfile.maxPairs {
            remoteHint = "最多 \(ServerProfile.maxPairs) 条配对 —— 先删掉几条再加"
            return
        }
        var pair = RemotePair(kind: kind, addr: "")
        pair.id = "new-" + UUID().uuidString
        draft.pairs.append(pair)
    }

    /// 区块内上移 / 下移（跨区块不让动：区块顺序由「连接优先级」决定）
    private func movePair(_ id: String, by delta: Int) {
        guard let index = draft.pairs.firstIndex(where: { $0.id == id }) else { return }
        let kind = draft.pairs[index].kind
        let sameBlock = draft.pairs.enumerated().filter { $0.element.kind == kind }.map(\.offset)
        guard let at = sameBlock.firstIndex(of: index) else { return }
        let target = at + delta
        guard sameBlock.indices.contains(target) else { return }
        let item = draft.pairs.remove(at: index)
        draft.pairs.insert(item, at: sameBlock[target])
    }

    private func updatePair(_ id: String, change: (inout RemotePair) -> Void) {
        guard let index = draft.pairs.firstIndex(where: { $0.id == id }) else { return }
        var pair = draft.pairs[index]
        change(&pair)
        draft.pairs[index] = pair
    }

    private func pairTextBinding(_ pair: RemotePair,
                                 _ keyPath: WritableKeyPath<RemotePair, String>) -> Binding<String> {
        Binding(
            get: { draft.pairs.first { $0.id == pair.id }?[keyPath: keyPath] ?? "" },
            set: { newValue in
                updatePair(pair.id) { item in
                    item[keyPath: keyPath] = newValue
                }
            })
    }

    private func pairBoolBinding(_ pair: RemotePair) -> Binding<Bool> {
        Binding(
            get: { draft.pairs.first { $0.id == pair.id }?.enabled ?? pair.enabled },
            set: { newValue in
                updatePair(pair.id) { item in
                    item.enabled = newValue
                }
            })
    }

    /// 条目超时（秒文本；空 = 继承区块默认）
    private func pairTimeoutBinding(_ pair: RemotePair) -> Binding<String> {
        Binding(
            get: {
                guard let current = draft.pairs.first(where: { $0.id == pair.id }) else { return "" }
                return current.probeTimeoutMs > 0 ? Self.secondsText(current.probeTimeoutMs) : ""
            },
            set: {
                let cleaned = Self.cleanedTimeout($0)
                let ms = Double(cleaned).map { ServerProfile.clampProbeTimeoutMs(Int($0 * 1000)) } ?? 0
                updatePair(pair.id) { $0.probeTimeoutMs = ms }
            })
    }

    // MARK: - 连接策略（折叠区）

    private var connSummary: String {
        let availableEnabled = draft.routeOrderSorted().filter {
            routeAvailable($0) && !draft.routeDisabled.contains($0)
        }.count
        var text = "优先级 \(availableEnabled)/\(Self.routeNames.count)"
            + " · 通道间 \(probeShort(draft.routeProbeMode))"
            + " · 多网卡 \(probeShort(draft.probeMode))"
            + " · 默认 \(String(format: "%.1f", Double(probeTimeoutMsValue) / 1000.0))s"
        let separatelySet = [ServerProfile.routeLan, ServerProfile.routeJump].filter {
            !(routeTimeoutTexts[$0] ?? "").trimmingCharacters(in: .whitespaces).isEmpty
        }.count
        if separatelySet > 0 { text += "（\(separatelySet) 个通道单独设）" }
        if !jumpServerId.trimmedAnyBoot.isEmpty { text += " · 跳板机" }
        return text
    }

    private var probeTimeoutMsValue: Int {
        let raw = probeTimeoutText.trimmingCharacters(in: .whitespaces)
        guard let value = Double(raw), value > 0 else { return ServerProfile.defaultProbeTimeoutMs }
        return ServerProfile.clampProbeTimeoutMs(Int(value * 1000))
    }

    /// 探测模式的短标签（折叠头摘要用）
    private func probeShort(_ mode: String) -> String {
        switch mode {
        case ServerProfile.probeParallelAll: return "并发全部"
        case ServerProfile.probeSerial: return "串行"
        default: return "并发首个"
        }
    }

    private var connFoldContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            fieldLabel("连接优先级（↑↓ 调顺序；行尾开关启用 / 停用；名称下的小框 = 这个通道自己的探测超时/秒，留空用下面的默认值）")
            let order = draft.routeOrderSorted()
            ForEach(Array(order.enumerated()), id: \.element) { index, kind in
                routeRow(kind: kind, index: index, total: order.count)
            }
            Text("探活与接口请求按此顺序逐通道试，第一个通的就走谁；每个通道可以单独设探测超时"
                + "（局域网 1~2 秒够，走公网中继的 Funnel / 内置 Funnel 首次握手慢，可以单独给 5~8 秒）。"
                + "跳板机需先在下方选好档案；电源唤醒不走此排序 —— 魔术包只能在局域网内广播。")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
                .padding(.top, 4)

            fieldLabel("连接优先级的探测方式（六个通道之间怎么试，对上面每一个通道都生效）")
            HStack(spacing: 0) {
                probeChoice("并发·首个", draft.routeProbeMode == ServerProfile.probeParallelFirst) {
                    draft.routeProbeMode = ServerProfile.probeParallelFirst
                }
                probeChoice("并发·全部", draft.routeProbeMode == ServerProfile.probeParallelAll) {
                    draft.routeProbeMode = ServerProfile.probeParallelAll
                }
                probeChoice("串行", draft.routeProbeMode == ServerProfile.probeSerial) {
                    draft.routeProbeMode = ServerProfile.probeSerial
                }
            }
            Text("并发·首个：全部通道同时发出，谁先成功用谁；并发·全部：等全部返回，逐行报每个通道的成败与耗时；"
                + "串行：按上面的优先级逐个通道试，第一个通就停。")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
                .padding(.top, 4)

            fieldLabel("多网卡探测（同一通道里的多个地址之间怎么探）")
            HStack(spacing: 0) {
                probeChoice("并发·首个", draft.probeMode == ServerProfile.probeParallelFirst) {
                    draft.probeMode = ServerProfile.probeParallelFirst
                }
                probeChoice("并发·全部", draft.probeMode == ServerProfile.probeParallelAll) {
                    draft.probeMode = ServerProfile.probeParallelAll
                }
                probeChoice("串行", draft.probeMode == ServerProfile.probeSerial) {
                    draft.probeMode = ServerProfile.probeSerial
                }
            }
            Text("一台电脑配了多个局域网地址（多块网卡 / DHCP 换过 IP）时生效：并发谁先通用谁，串行按地址顺序试。")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
                .padding(.top, 4)

            // 区块内部策略 + 区块默认超时（配对专用）：一行「区块名 …… 默认超时[__]秒」，
            // 第二行三个策略按钮均分占满 —— 与 PC 端同构
            fieldLabel("区块内部策略（同一个区块里的多条配对之间怎么探）与区块默认超时")
            ForEach(RemotePair.blockIds, id: \.self) { block in
                blockPolicyRow(block)
            }
            Text("并发·首个：同区块全部同时发出，先成功的不一定是最前面那条；串行：严格按区块内的行序试。"
                + "每条配对还能在它自己那行再设一个超时（条目 → 区块 → 全局默认）。")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
                .padding(.top, 4)

            fieldLabel("经由跳板机（外网连家里：先到「☰ → 服务器」添加 SSH 跳板机；"
                + "填的地址就变成「跳板机视角的内网地址」）")
            jumpPicker

            fieldLabel("探测方式（连接优先级整体的策略，对上面每一个通道都生效）")
            HStack(spacing: 10) {
                probeChoice("并发·首个", draft.probeMode == ServerProfile.probeParallelFirst) {
                    draft.probeMode = ServerProfile.probeParallelFirst
                }
                probeChoice("并发·全部", draft.probeMode == ServerProfile.probeParallelAll) {
                    draft.probeMode = ServerProfile.probeParallelAll
                }
                probeChoice("串行", draft.probeMode == ServerProfile.probeSerial) {
                    draft.probeMode = ServerProfile.probeSerial
                }
            }
            Text("并发·首个：每地址一个线程，任一响应立即用它；并发·全部：等全部探完，逐行报成败；串行：按顺序试，第一个通就停")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
                .padding(.top, 4)

            fieldLabel("默认探测超时（秒）—— 通道那格留空时用它")
            HStack(spacing: 10) {
                timeoutTextField("秒",
                                 text: Binding(
                                    get: { probeTimeoutText },
                                    set: { probeTimeoutText = Self.cleanedTimeout($0) }),
                                 placeholder: "2")
                    .frame(width: 110)
                Text("通道那格填了就以它自己为准；没填的通道才用这个默认值（上限 30 秒）")
                    .font(.system(size: 10))
                    .foregroundColor(ABColor.fgFaint)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(.top, 2)
    }

    /// 连接优先级的一行：☰ + 名称/副标+超时 + ↑ ↓ + 开关
    private func routeRow(kind: String, index: Int, total: Int) -> some View {
        let available = routeAvailable(kind)
        return HStack(spacing: 8) {
            Text("☰")
                .font(.system(size: 13))
                .foregroundColor(ABColor.fgFaint)
            VStack(alignment: .leading, spacing: 1) {
                Text(Self.routeNames[kind] ?? kind)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(available ? ABColor.fg : ABColor.fgFaint)
                HStack(spacing: 6) {
                    Text(routeSub(kind) + (available ? "" : "（不可用）"))
                        .font(.system(size: 10))
                        .foregroundColor(ABColor.fgFaint)
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if kind == ServerProfile.routeLan || kind == ServerProfile.routeJump {
                        inlineTimeoutField(
                            label: "超时",
                            text: routeTimeoutBinding(kind),
                            enabled: available)
                    } else {
                        Text("超时见区块设置")
                            .font(.system(size: 10))
                            .foregroundColor(ABColor.fgFaint)
                    }
                }
            }
            tinyTextButton("↑", tint: ABColor.fgDim, enabled: index > 0) {
                moveRoute(kind: kind, by: -1)
            }
            tinyTextButton("↓", tint: ABColor.fgDim, enabled: index < total - 1) {
                moveRoute(kind: kind, by: 1)
            }
            Toggle("", isOn: Binding(
                // 开关只听 routeDisabled：没配配对的通道也要能停用（用户 2026-10-28：
                // 「不可用」只是暂没配地址，不允许关掉反而让停用开关点不动）
                get: { !draft.routeDisabled.contains(kind) },
                set: { on in
                    if on {
                        draft.routeDisabled.removeAll { $0 == kind }
                    } else if !draft.routeDisabled.contains(kind) {
                        draft.routeDisabled.append(kind)
                    }
                }))
                .labelsHidden()
                .scaleEffect(0.7, anchor: .trailing)
                .frame(width: 36)
        }
        .padding(.vertical, 2)
    }

    /// 经由跳板机：只读触发框 + 下拉（安卓用 disabled TextField + DropdownMenu，语义相同）
    private var jumpPicker: some View {
        Menu {
            Button("直连（默认）") { jumpServerId = "" }
            ForEach(jumps.jumps) { jump in
                Button(jumpLabel(jump)) { jumpServerId = jump.id }
            }
        } label: {
            HStack(spacing: 6) {
                Text(currentJumpLabel)
                    .font(.system(size: 13))
                    .foregroundColor(ABColor.fg)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text("▾")
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.fgDim)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 9)
            .background(RoundedRectangle(cornerRadius: ABMetric.radiusControl, style: .continuous)
                .fill(ABColor.surfaceRaised))
            .overlay(RoundedRectangle(cornerRadius: ABMetric.radiusControl, style: .continuous)
                .strokeBorder(ABColor.lineStrong))
        }
    }

    private var currentJumpLabel: String {
        guard !jumpServerId.trimmedAnyBoot.isEmpty,
              let jump = jumps.jumps.first(where: { $0.id == jumpServerId }) else {
            return "直连（默认）"
        }
        return jumpLabel(jump)
    }

    private func jumpLabel(_ jump: JumpServer) -> String {
        let label = jump.displayName.isEmpty ? jump.host : jump.displayName
        return "经由：\(label)" + (jump.isSsh ? "" : "（仅 Telnet，不能做隧道）")
    }

    private func routeTimeoutBinding(_ kind: String) -> Binding<String> {
        Binding(
            get: { routeTimeoutTexts[kind] ?? "" },
            set: {
                let cleaned = Self.cleanedTimeout($0)
                if cleaned.isEmpty { routeTimeoutTexts[kind] = nil } else { routeTimeoutTexts[kind] = cleaned }
            })
    }

    private func blockTimeoutBinding(_ block: String) -> Binding<String> {
        Binding(
            get: { blockTimeoutTexts[block] ?? "" },
            set: {
                let cleaned = Self.cleanedTimeout($0)
                if cleaned.isEmpty { blockTimeoutTexts[block] = nil } else { blockTimeoutTexts[block] = cleaned }
            })
    }

    private func blockModeBinding(_ block: String) -> Binding<String> {
        Binding(
            get: { ServerProfile.normalizeProbeMode(draft.blockProbeMode[block] ?? ServerProfile.probeParallelFirst) },
            set: { draft.blockProbeMode[block] = ServerProfile.normalizeProbeMode($0) })
    }

    private func blockPolicyRow(_ block: String) -> some View {
        let count = draft.pairs.filter {
            $0.kind == block && !$0.addr.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }.count
        let mode = blockModeBinding(block)
        return VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                Text(RemotePair.blockLabel(block))
                    .font(.system(size: 12))
                    .foregroundColor(count > 0 ? ABColor.fg : ABColor.fgFaint)
                    .frame(maxWidth: .infinity, alignment: .leading)
                inlineTimeoutField(label: "默认超时",
                                   text: blockTimeoutBinding(block),
                                   enabled: count > 0)
            }
            HStack(spacing: 0) {
                probeChoice("并发·首个", mode.wrappedValue == ServerProfile.probeParallelFirst) {
                    mode.wrappedValue = ServerProfile.probeParallelFirst
                }
                probeChoice("全部", mode.wrappedValue == ServerProfile.probeParallelAll) {
                    mode.wrappedValue = ServerProfile.probeParallelAll
                }
                probeChoice("串行", mode.wrappedValue == ServerProfile.probeSerial) {
                    mode.wrappedValue = ServerProfile.probeSerial
                }
            }
        }
        .padding(.vertical, 2)
    }

    /// 通道当前是否可用（与 PC rsRouteAvailable / 后端 routeCandidates 同语义）
    private func routeAvailable(_ kind: String) -> Bool {
        switch kind {
        case ServerProfile.routeLan:
            return addressEntries.contains { !$0.address.trimmedAnyBoot.isEmpty }
        case ServerProfile.routeJump:
            return routeAvailable(ServerProfile.routeLan) && !jumpServerId.trimmedAnyBoot.isEmpty
        default:
            return draft.pairs.contains {
                $0.enabled && !$0.addr.trimmedAnyBoot.isEmpty && $0.routeKind == kind
            }
        }
    }

    /// 通道副标：地址 / 没配置的原因
    private func routeSub(_ kind: String) -> String {
        switch kind {
        case ServerProfile.routeLan:
            return addressEntries.first { !$0.address.trimmedAnyBoot.isEmpty }?.address ?? "未填局域网地址"
        case ServerProfile.routeJump:
            if !jumpServerId.trimmedAnyBoot.isEmpty,
               let jump = jumps.jumps.first(where: { $0.id == jumpServerId }) {
                return "经 " + jumpLabel(jump)
            }
            return "未选跳板机档案"
        default:
            let empty: String
            switch kind {
            case ServerProfile.routeTunnel: empty = "未配置隧道地址"
            case ServerProfile.routeTS: empty = "未配置 100.x 直连地址"
            case ServerProfile.routeFunnel: empty = "未配置（系统 Tailscale 的 *.ts.net 地址）"
            case ServerProfile.routeTSNet: empty = "未配置（内置引擎的 *.ts.net 地址）"
            default: empty = ""
            }
            return draft.pairs.first {
                !$0.addr.trimmedAnyBoot.isEmpty && $0.routeKind == kind
            }?.addr ?? empty
        }
    }

    private func moveRoute(kind: String, by delta: Int) {
        var order = draft.routeOrderSorted()
        guard let from = order.firstIndex(of: kind) else { return }
        let to = from + delta
        guard order.indices.contains(to) else { return }
        order.swapAt(from, to)
        draft.routeOrder = order
    }

    // MARK: - 局域网电源路由（折叠区）

    private var powerFoldContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            fieldLabel("局域网电源路由（可选：每个动作发到哪个地址、哪个端口。"
                + "目标机装 sol 后按端口执行 —— 不装 sol 就只有唤醒能靠 WOL 生效）")
            if draft.powerRoutes.isEmpty {
                Text("还没配：电源按钮仍走老路 —— 唤醒发标准 WOL，重启/关机/睡眠调电脑端接口。")
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
            }
            ForEach(Array(draft.powerRoutes.enumerated()), id: \.offset) { index, _ in
                PowerRouteCardView(
                    route: powerRouteBinding(index),
                    macPool: macPool,
                    index: index,
                    total: draft.powerRoutes.count,
                    onRemove: {
                        draft.powerRoutes.remove(at: index)
                    })
                    .padding(.top, 6)
            }
            Button {
                // 发送模式默认「优先广播包」（= 旧的 HTTP 兜底勾选）
                draft.powerRoutes.append(PowerRoute(action: PowerRoute.actionWake, target: "", port: 9))
            } label: {
                Text("+ 添加一条路由")
                    .font(.system(size: 13))
                    .foregroundColor(ABColor.accent)
            }
            .buttonStyle(.plain)
            .padding(.top, 6)
        }
        .padding(.top, 2)
    }

    /// MAC 池：以「已添加的地址」为准（地址 / 网卡变了，可选项跟着变）
    private var macPool: [String] {
        addressEntries.flatMap { $0.macs }.removingDuplicates()
    }

    private func powerRouteBinding(_ index: Int) -> Binding<PowerRoute> {
        Binding(
            get: { index < draft.powerRoutes.count ? draft.powerRoutes[index] : PowerRoute() },
            set: {
                guard index < draft.powerRoutes.count else { return }
                draft.powerRoutes[index] = $0
            })
    }

    // MARK: - 保存

    private func save() {
        // 兜底：快速识别框里粘了但没点「识别并加入」—— 保存时顺手认一次
        if !addressEntries.contains(where: { !$0.address.trimmedAnyBoot.isEmpty }),
           !quickText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            recognizeQuick()
        }
        var result = draft
        result.addresses = []
        result.macs = [:]
        var parsed = ParsedAddressInput()
        for entry in addressEntries where !entry.address.trimmedAnyBoot.isEmpty {
            parsed.hosts.append(entry.address)
            if !entry.macs.isEmpty { parsed.macs[entry.address] = entry.macs }
        }
        result.replaceParsed(parsed)
        let trimmedName = name.trimmedAnyBoot
        result.name = trimmedName.isEmpty ? (result.host.isEmpty ? "" : result.host) : trimmedName
        let secret = token.trimmedAnyBoot
        result.token = secret.isEmpty ? ServerProfile.token(fromURL: pastedRaw) : secret
        result.jumpServerId = jumpServerId
        result.probeMode = ServerProfile.normalizeProbeMode(result.probeMode)
        result.routeProbeMode = ServerProfile.normalizeRouteProbeMode(result.routeProbeMode)
        result.probeTimeoutMs = probeTimeoutMsValue
        // 每通道自己的超时：只收单独设过的 lan / jump（与 Android routeTimeoutsMs() 同口径）
        var timeouts: [String: Int] = [:]
        for kind in [ServerProfile.routeLan, ServerProfile.routeJump] {
            let raw = (routeTimeoutTexts[kind] ?? "").trimmingCharacters(in: .whitespaces)
            guard !raw.isEmpty, let value = Double(raw) else { continue }
            timeouts[kind] = ServerProfile.clampProbeTimeoutMs(Int(value * 1000))
        }
        result.routeTimeoutsMs = timeouts
        // 区块默认超时（秒文本 → 毫秒；空 = 继承全局）
        var blockTimeouts: [String: Int] = [:]
        for block in RemotePair.blockIds {
            let raw = (blockTimeoutTexts[block] ?? "").trimmingCharacters(in: .whitespaces)
            guard !raw.isEmpty, let value = Double(raw) else { continue }
            let ms = ServerProfile.clampProbeTimeoutMs(Int(value * 1000))
            if ms > 0 { blockTimeouts[block] = ms }
        }
        result.blockTimeoutsMs = blockTimeouts
        // 区块探测策略：默认值（并发·首个）不落盘
        result.blockProbeMode = result.blockProbeMode.filter {
            ServerProfile.normalizeProbeMode($0.value) != ServerProfile.probeParallelFirst
        }
        // 配对：地址为空的行（点了「＋ 添加」没填的）丢弃
        result.pairs = result.pairs.filter { !$0.addr.trimmedAnyBoot.isEmpty }
        result.routeOrder = result.routeOrderSorted()
        result.pruneMacs()
        guard !result.addresses.isEmpty else {
            emptyHintShown = true
            return
        }
        onFinish(.save(result))
        dismiss()
    }
}

// MARK: - 表单小件（Android 同名件的 SwiftUI 版）

/// 字段标签：12pt fgDim，上 10 下 2（Android FieldLabel 同规格）
private func fieldLabel(_ text: String) -> some View {
    Text(text)
        .font(.system(size: 12))
        .foregroundColor(ABColor.fgDim)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, 10)
        .padding(.bottom, 2)
}

/// 折叠区头：标题 + 右侧状态摘要，点击展开/收起（高级项默认收起）
private func foldHeader(title: String, summary: String, open: Bool,
                        onToggle: @escaping () -> Void) -> some View {
    Button(action: onToggle) {
        HStack(spacing: 6) {
            Text(open ? "▾" : "▸")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
            Text(title)
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(ABColor.fgDim)
            Spacer()
            Text(summary)
                .font(.system(size: 10.5))
                .foregroundColor(ABColor.fgFaint)
                .lineLimit(1)
        }
        .padding(.vertical, 6)
        .padding(.horizontal, 2)
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .padding(.top, 8)
}

/// 紧凑的小图标按钮（30pt 方框、纯符号、无底色；Android TinyTextButton）
private func tinyTextButton(_ text: String, tint: Color, enabled: Bool = true,
                            action: @escaping () -> Void) -> some View {
    Button(action: action) {
        Text(text)
            .font(.system(size: 14))
            .foregroundColor(enabled ? tint : tint.opacity(0.35))
            .frame(width: 30, height: 30)
            .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .disabled(!enabled)
}

/// 单选的「探测方式」小件（Android ProbeModeChoice：radio + 标签，三等分一宽）
private func probeChoice(_ label: String, _ selected: Bool,
                         action: @escaping () -> Void) -> some View {
    Button(action: action) {
        HStack(spacing: 2) {
            ABRadioMark(selected: selected)
            Text(label)
                .font(.system(size: 11.5))
                .foregroundColor(ABColor.fg)
                .lineLimit(1)
                .minimumScaleFactor(0.7)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
}

private struct ABRadioMark: View {
    let selected: Bool
    var body: some View {
        ZStack {
            Circle()
                .strokeBorder(selected ? ABColor.accent : ABColor.fgFaint, lineWidth: 1.6)
                .frame(width: 17, height: 17)
            if selected {
                Circle()
                    .fill(ABColor.accent)
                    .frame(width: 8.5, height: 8.5)
            }
        }
    }
}

/// 「超时 [__] 秒」控件组（Android InlineTimeoutField + SmallNumberField）：
/// 26pt 高、等宽居中的小数字框，三处复用（通道行 / 区块默认 / 每条配对）
private func inlineTimeoutField(label: String, text: Binding<String>, enabled: Bool) -> some View {
    HStack(spacing: 4) {
        Text(label)
            .font(.system(size: 10))
            .foregroundColor(ABColor.fgFaint)
        ZStack {
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .fill(ABColor.card)
            RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(enabled ? ABColor.line : ABColor.line.opacity(0.5))
            if text.wrappedValue.isEmpty {
                Text("默认")
                    .font(.system(size: 9))
                    .foregroundColor(ABColor.fgFaint)
            }
            TextField("", text: text)
                .font(.system(size: 11, design: .monospaced))
                .foregroundColor(enabled ? ABColor.fg : ABColor.fgFaint)
                .multilineTextAlignment(.center)
                .keyboardType(.numbersAndPunctuation)
                .disabled(!enabled)
                .padding(.horizontal, 4)
        }
        .frame(width: 56, height: 26)
        Text("秒")
            .font(.system(size: 10))
            .foregroundColor(ABColor.fgFaint)
    }
}

/// 大一号的超时输入框（默认探测超时用，Android 是 110dp 宽 OutlinedTextField）
private func timeoutTextField(_ unit: String, text: Binding<String>, placeholder: String) -> some View {
    TextField(placeholder, text: text, prompt: Text(placeholder).font(.system(size: 12)))
        .font(.system(size: 13))
        .multilineTextAlignment(.leading)
        .keyboardType(.numbersAndPunctuation)
        .abField()
        .overlay(alignment: .trailing) {
            Text(unit)
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
                .padding(.trailing, 8)
                .allowsHitTesting(false)
        }
}

// MARK: - 配对行（Android PairEditRow）

/// 一行配对：启用开关 · 地址（可编辑）· ↑↓ ✕ · lane · 来源 · 条目超时 · 令牌（打码/可改）· 复制地址
private struct PairEditRowView: View {

    let pair: RemotePair
    @Binding var addr: String
    @Binding var token: String
    @Binding var timeoutText: String
    @Binding var enabled: Bool
    let onMove: (Int) -> Void
    let onRemove: () -> Void

    @State private var tokenShown = false
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            // 行 1：开关 + 地址 + ↑ ↓ ✕
            HStack(spacing: 2) {
                Toggle("", isOn: $enabled)
                    .labelsHidden()
                    .scaleEffect(0.7, anchor: .leading)
                    .frame(width: 34)
                TextField("地址（https://… 或 100.x:12300）", text: $addr)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .font(.system(size: 13))
                    .abField()
                tinyTextButton("↑", tint: ABColor.fgDim) { onMove(-1) }
                tinyTextButton("↓", tint: ABColor.fgDim) { onMove(1) }
                tinyTextButton("✕", tint: ABColor.bad) { onRemove() }
            }
            // 行 2：lane 徽标 + 来源 + 条目超时
            HStack(spacing: 6) {
                if !lane.isEmpty {
                    Text(lane)
                        .font(.system(size: 10))
                        .foregroundColor(lane == "Funnel" ? ABColor.ok : ABColor.accent)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .overlay(Capsule().strokeBorder(
                            (lane == "Funnel" ? ABColor.ok : ABColor.accent).opacity(0.5)))
                }
                Text(source)
                    .font(.system(size: 10))
                    .foregroundColor(ABColor.fgFaint)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                inlineTimeoutField(label: "超时", text: $timeoutText, enabled: enabled)
            }
            // 行 3：令牌（打码显示，点「改/看」变可编辑）
            HStack(spacing: 6) {
                Text("令牌")
                    .font(.system(size: 10))
                    .foregroundColor(ABColor.fgFaint)
                if tokenShown {
                    TextField("", text: $token)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.system(size: 13))
                        .abField()
                } else {
                    Text(token.isEmpty ? "（未配）" : masked)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundColor(ABColor.fgDim)
                        .lineLimit(expanded ? 3 : 1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                        .onTapGesture { expanded.toggle() }
                }
                Button {
                    tokenShown.toggle()
                } label: {
                    Text(tokenShown ? "打码" : "改/看")
                        .font(.system(size: 11))
                        .foregroundColor(ABColor.accent)
                }
                .buttonStyle(.plain)
                Button {
                    UIPasteboard.general.string = pair.addr
                } label: {
                    Text("复制地址")
                        .font(.system(size: 11))
                        .foregroundColor(ABColor.fgDim)
                }
                .buttonStyle(.plain)
            }
            if expanded && !pair.token.isEmpty && !tokenShown {
                Text(pair.token)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(ABColor.fg)
                    .lineLimit(3)
                    .padding(.top, 2)
            }
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 5)
        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(ABColor.elevate))
        .padding(.top, 6)
    }

    /// lane：只对 ts 区块有区分度（Funnel / 直连），其余区块不显示
    private var lane: String {
        guard pair.kind == RemotePair.kindTS else { return "" }
        return ServerProfile.isFunnelOrigin(pair.addr) ? "Funnel" : "直连"
    }

    /// 来源列：host · os；都空时退回 mid 短号；都没有 = 「手填」
    private var source: String {
        let label = pair.sourceLabel
        return label.isEmpty ? "手填" : label
    }

    private var masked: String {
        let text = pair.token.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty { return "" }
        if text.count <= 12 { return String(repeating: "•", count: max(6, text.count)) }
        return String(text.prefix(4)) + "••••••••••" + String(text.suffix(4))
    }
}

// MARK: - 电源路由卡（Android 电源路由卡片段）

/// 一条电源路由：动作菜单 + 删除 / 模式菜单 / 广播包（端口·地址·MAC·包数·重试）/ HTTP 说明
private struct PowerRouteCardView: View {

    @Binding var route: PowerRoute
    let macPool: [String]
    let index: Int
    let total: Int
    let onRemove: () -> Void

    @State private var macsOpen = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("路由 \(index + 1) / \(total)")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)

            // 动作 + 删除（行尾加「启用」开关，用户 2026-10-31，与通道/配对开关同款：
            // 默认开；关掉 = 这条策略不参与发送，策略留着不删，整卡半透明）
            HStack {
                Menu {
                    // 与 Android 同序：唤醒 / 重启 / 关机 / 睡眠
                    ForEach([PowerRoute.actionWake, PowerRoute.actionReboot,
                             PowerRoute.actionShutdown, PowerRoute.actionSleep], id: \.self) { action in
                        Button(ServerProfile.powerActionLabel(action)) {
                            route = routeWithAction(action)
                        }
                    }
                } label: {
                    outlinedButtonLabel(ServerProfile.powerActionLabel(route.actionOrWake))
                }
                Spacer()
                VStack(spacing: 0) {
                    Toggle("", isOn: $route.enabled)
                        .labelsHidden()
                        .scaleEffect(0.7, anchor: .trailing)
                        .frame(width: 36)
                    Text("启用")
                        .font(.system(size: 9))
                        .foregroundColor(ABColor.fgFaint)
                }
                Button(action: onRemove) {
                    Text("删除")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.bad)
                }
                .buttonStyle(.plain)
            }

            // 发送模式（共同设置区）
            Menu {
                ForEach(["auto", "http_first", "http_only", "packet_only", "hybrid"], id: \.self) { value in
                    Button(ServerProfile.modeLabel(value)) {
                        route = route.withMode(value)
                    }
                }
            } label: {
                outlinedButtonLabel("模式：" + ServerProfile.modeLabel(route.effectiveMode))
            }

            // —— 广播包设置区：端口 / 地址 / MAC / 发包数 / 重试 ——
            Text("广播包")
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(ABColor.fgDim)
                .padding(.top, 2)
            TextField("", value: Binding(
                get: { route.port <= 0 ? nil : route.port },
                set: { route.port = $0 ?? 0 }), format: .number, prompt:
                Text("端口").font(.system(size: 10)))
                .keyboardType(.numberPad)
                .font(.system(size: 13))
                .abField()
                .frame(width: 92)
            TextField("广播地址（留空 = 自动：全局 + 各地址的 /24）", text: $route.target, prompt:
                Text("广播地址（留空 = 自动：全局 + 各地址的 /24）").font(.system(size: 10)))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .font(.system(size: 13))
                .abField()
                .padding(.top, 2)

            // MAC 选择
            HStack {
                Text(macSummary)
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgFaint)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Button {
                    macsOpen.toggle()
                } label: {
                    Text(macPool.isEmpty ? "没配 MAC" : "选择")
                        .font(.system(size: 12))
                        .foregroundColor(macPool.isEmpty ? ABColor.fgFaint : ABColor.accent)
                }
                .buttonStyle(.plain)
                .disabled(macPool.isEmpty)
            }
            if macsOpen && !macPool.isEmpty {
                // 空列表 = 全部网卡（以后新加的 MAC 也自动包含）
                macRow("全部网卡", checked: route.macs.isEmpty) {
                    route.macs = []
                }
                if !route.macs.isEmpty {
                    ForEach(macPool, id: \.self) { mac in
                        macRow(mac, checked: route.macs.contains(mac)) {
                            if let at = route.macs.firstIndex(of: mac) {
                                route.macs.remove(at: at)
                            } else {
                                route.macs.append(mac)
                            }
                        }
                    }
                }
            }

            // 发包数 / 重试：value 只认「用户自己填的数」，空着用占位符显示默认值
            HStack(spacing: 8) {
                countsField(label: route.actionOrWake == PowerRoute.actionWake ? "每块发几个包" : "发几个包",
                            value: Binding(
                                get: { route.burst > 0 ? route.burst : nil },
                                set: { route.burst = min(50, $0 ?? 0) }),
                            placeholder: "\(route.defaultBurst)")
                    .frame(maxWidth: .infinity)
                countsField(label: "重试次数",
                            value: Binding(
                                get: { route.retry > 0 ? route.retry : nil },
                                set: { route.retry = min(20, $0 ?? 0) }),
                            placeholder: "\(route.defaultRetry)")
                    .frame(maxWidth: .infinity)
            }
            Text("重试 = 没发出去就隔 0.2 秒再打一轮；走跳板机时每轮重试都会重新连接跳板机，直到次数用完。")
                .font(.system(size: 9.5))
                .foregroundColor(ABColor.fgFaint)

            // —— HTTP 设置区：无可配置项，仅说明 ——
            Text("HTTP")
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(ABColor.fgDim)
                .padding(.top, 2)
            Text("调这台电脑自带的 /api/power 接口（token 与连接共用，无需单独配置）。"
                + "混合模式 = HTTP 与广播包同时发。")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
        }
        .padding(8)
        // 停用的策略整卡半透明（策略还在，只是不参与发送），与 PC / Android 同语义
        .opacity(route.enabled ? 1 : 0.55)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(ABColor.card))
    }

    /// 换动作：没手动改过包数/重试（还是旧动作默认值）就跟着换成新动作默认值（0 = 按动作默认）
    private func routeWithAction(_ newAction: String) -> PowerRoute {
        let before = route.actionOrWake
        let oldBurst = before == PowerRoute.actionWake
            ? ServerProfile.defaultWakeBurst : ServerProfile.defaultPowerBurst
        let oldRetry = before == PowerRoute.actionWake
            ? ServerProfile.defaultWakeRetry : ServerProfile.defaultPowerRetry
        let burst = (route.burst == 0 || route.burst == oldBurst) ? 0 : route.burst
        let retry = (route.retry == 0 || route.retry == oldRetry) ? 0 : route.retry
        return route.with(action: newAction, target: route.target, port: route.port,
                          macs: route.macs, enabled: route.enabled)
            .withCounts(burst, retry)
    }

    private var macSummary: String {
        if route.macs.isEmpty {
            return "MAC：全部（\(macPool.count) 块）"
        }
        return "MAC：\(route.macs.count) / \(macPool.count) 块"
    }

    private func macRow(_ label: String, checked: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                ZStack {
                    RoundedRectangle(cornerRadius: 4)
                        .strokeBorder(checked ? ABColor.accent : ABColor.fgFaint, lineWidth: 1.4)
                        .frame(width: 16, height: 16)
                        .background(checked ? ABColor.accent.opacity(0.12) : .clear)
                    if checked {
                        Image(systemName: "checkmark")
                            .font(.system(size: 9, weight: .bold))
                            .foregroundColor(ABColor.accent)
                    }
                }
                Text(label)
                    .font(.system(size: 11))
                    .foregroundColor(ABColor.fgDim)
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.vertical, 2)
    }

    /// 安卓 OutlinedButton 观感的菜单触发钮
    private func outlinedButtonLabel(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundColor(ABColor.fg)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(RoundedRectangle(cornerRadius: 9, style: .continuous)
                .strokeBorder(ABColor.lineStrong))
    }

    private func countsField(label: String,
                             value: Binding<Int?>,
                             placeholder: String) -> some View {
        TextField("", value: value, format: .number, prompt:
            Text(placeholder).font(.system(size: 11)))
            .keyboardType(.numberPad)
            .font(.system(size: 13))
            .abField()
            .overlay(alignment: .leading) {
                Text(label)
                    .font(.system(size: 10))
                    .foregroundColor(ABColor.fgFaint)
                    .padding(.leading, 10)
                    .allowsHitTesting(false)
                    .opacity(value.wrappedValue == nil ? 1 : 0)
            }
    }
}

extension Array where Element == String {
    /// 去重且保持顺序（合并 MAC 池用）
    func removingDuplicates() -> [String] {
        var seen = Set<String>()
        return filter { seen.insert($0).inserted }
    }
}

// MARK: - sheet 高度贴合内容（安卓 BottomSheet 是 wrap-content）

/// 之前固定 `.large`：整屏高的 sheet 里表单只占上半截，保存按钮下面剩一大段空白。
/// 改成自定义 detent：高度 = 外壳（把手/标题/分隔线/动作栏）+ 表单内容高度（封顶 62% 屏高），
/// sheet 就像安卓一样贴着表单包住。表单量高之前先给近似值，量到后更新 —— iOS 16.4+
/// 的 sheet 会在视图更新时重新询问 detent 高度，折叠区展开/收起也会跟着长高。
private struct EditorSheetDetent: CustomPresentationDetent {
    /// 外壳固定高度：把手 20 + 标题行 40 + 分隔线 1 + 动作栏 60（全是固定 frame，可精确算出）
    static let chromeHeight: CGFloat = 121
    /// 表单量高之后的精确值；nil = 还没量到，先给个近似值避免首帧闪一下
    static var heightOverride: CGFloat?

    static func height(in context: Context) -> CGFloat? {
        if let heightOverride { return heightOverride }
        return min(chromeHeight + context.maxDetentValue * 0.62, context.maxDetentValue)
    }
}

/// 表单内容的高度（GeometryReader 背景量出来的，滚动裁切不影响）
private struct EditorFormHeightKey: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = max(value, nextValue())
    }
}
