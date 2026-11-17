import Foundation

// MARK: - 引导管理数据层（对齐 ComputerListActivity.kt:2355-2821）
//
// AnyBoot 的核心功能：读电脑上的 rEFInd EFI 变量与 GRUB 条目，切换下次启动项。
// Android 用 ~1,300 行实现；iOS 此前**完全没有**，这里逐条移植。

enum BootKind {
    static let refind = "refind"
    static let refindplus = "refindplus"
    static let grub = "grub"
    static let sdboot = "sdboot"
    static let opencore = "opencore"
    static let clover = "clover"

    /// 后端 `channels[6]` 的固定顺序（refind,refindplus,grub,sdboot,opencore,clover）。
    /// 渲染一律按这个顺序遍历，保证 6 个引导器通道无差别对待。
    static let all = [refind, refindplus, grub, sdboot, opencore, clover]

    /// 通道默认标题（后端 title 为空时的兜底）
    static func defaultTitle(_ kind: String) -> String {
        switch kind {
        case refind: return "rEFInd"
        case refindplus: return "RefindPlus"
        case grub: return "GRUB"
        case sdboot: return "systemd-boot"
        case opencore: return "OpenCore"
        case clover: return "Clover"
        default: return kind
        }
    }
}

/// 一个可选的启动项（rEFInd 的一条 EFI 变量，或 GRUB 的一个 menuentry）
struct BootOption: Hashable, Identifiable {

    var value: String            // rEFInd: system；GRUB: selector
    var label: String = ""       // rEFInd 的 displayName
    var kind: String = "linux"   // win / mac / linux / auto
    /** 组头行（value 恒空、不可选）：rEFInd 同一分区的「父条目（N 个内核）」 */
    var header: Bool = false
    /// 行右侧小标签（rEFInd 子项来源：默认项/折叠内核/macOS 诊断…）
    var tag: String = ""
    // ---- 统一 6 引导器通道（后端 BootOption）字段 ----
    /// 固定项（后端 fixed=true）：通常不可改，仅展示
    var fixed: Bool = false
    /// 真实二级子项（后端 child）；缩进/竖线只看它
    var child: Bool = false
    /// 行副标题（后端 sub）
    var sub: String = ""
    /// 行备注（后端 note）
    var note: String = ""

    // ---- GRUB 专用 ----
    var grubName: String = ""
    var grubKernels: [String] = []
    var grubSubmenuId: String = ""
    var grubVariants: Int = 1
    var grubChainloader: Bool = false
    var grubClassRaw: String = ""

    var id: String { value }
}

/// 一套引导管理器（rEFInd 或 GRUB）的可选组
struct BootGroup {

    var kind: String
    var title: String
    var available: Bool
    var current: String
    var next: String
    var saved: String
    var note: String
    var options: [BootOption]
    /// 挂起的远程切换倒计时（对端 /api/status 的 switchPending）——pendingBar 显示「N 秒后执行」+ 撤销
    var pendingSeconds: Int = 0
    var pendingSummary: String = ""

    // ---- GRUB 专用 ----
    var serverFormat: String = ""
    var grubGroupNames: [String: String] = [:]
    var grubEfiDir: String = ""
    var grubPartuuid: String = ""

    var isGrub: Bool { kind == BootKind.grub }
}

/// 一台电脑的引导结论
struct BootInfo {

    var ok: Bool = false
    var online: Bool = false
    /// 卡片上显示的地址（不带 scheme）
    var addr: String = ""
    var error: String = ""
    var refind: BootGroup
    var refindplus: BootGroup
    var grub: BootGroup
    /// 统一 6 引导器通道（refind,refindplus,grub,sdboot,opencore,clover）。
    /// 后端已改契约：`channels` 是 6 个统一 BootGroup 的数组，以它为准渲染。
    var channels: [BootGroup] = []
    var machineOs: String = ""
    var hits: [ProbeEngine.Hit] = []
    var winnerKind: String = ""
    /// 赢家落在哪个地址（局域网 / 跳板机 = 那个直连地址，远程 = 通道地址）。
    /// 一址一跳板机：卡片按它回答「用的是这个地址的那台跳板机」
    var winnerAddr: String = ""

    static func offline(_ addr: String, _ error: String, hits: [ProbeEngine.Hit] = []) -> BootInfo {
        let note = L("and.offlineCannotReadBoot")
        return BootInfo(
            ok: false, online: false, addr: addr, error: error,
            refind: BootGroup(kind: BootKind.refind, title: "rEFInd", available: false,
                              current: "", next: "", saved: "", note: note, options: []),
            refindplus: BootGroup(kind: BootKind.refindplus, title: "RefindPlus", available: false,
                                  current: "", next: "", saved: "", note: note, options: []),
            grub: BootGroup(kind: BootKind.grub, title: "GRUB", available: false,
                            current: "", next: "", saved: "", note: note, options: []),
            hits: hits)
    }
}

/// 弹层里的一行（顶层项 / 组头 / 组内子项）
struct GrubRow: Identifiable {

    var option: BootOption?
    var text: String
    var isGroup: Bool
    var pinned: Bool = false
    var child: Bool = false
    /// 固定项（后端 fixed）：沿用统一字段，渲染时不再按通道特判
    var fixed: Bool = false
    /// 行右侧灰字标签（组头=「N 项」；子项=来源 tag）
    var trailing: String = ""

    var id: String {
        // 组内子项可能共用同一 value（rEFInd 折叠内核全部 value=父条目 title），
        // id 必须带上 label 区分，否则 ForEach 重复 id 崩溃
        if let option { return (isGroup ? "t:" : "o:") + option.value + ":" + text }
        return "g:" + text
    }
}

// MARK: - 解析

enum BootParser {

    /// `/api/status` 的 `variables` → rEFInd 可选组（对齐 PC 端 refind_from_status）
    /// `/api/ui-state` 的 `refindSim` → rEFInd 可选组（**自动模式**的数据源）。
    ///
    /// 与手动模式（vars 快照）的区别：自动模式是按 rEFInd 自己的扫描规则
    /// **重算**出来的菜单，含二级子屏；「当前项」看 `entries[].lastBoot`
    /// 命中的那条（与 PC 端 effectiveGrubForNextBoot / 托盘同一口径），
    /// 没有就退回 `previousBoot`。
    static func parseRefindSim(_ json: [String: Any], kind: String = BootKind.refind, title groupTitle: String = "rEFInd") -> BootGroup {
        var options: [BootOption] = []
        var current = ""
        let entries = json["entries"] as? [[String: Any]] ?? []
        for item in entries {
            let title = (item["title"] as? String ?? "").trimmingCharacters(in: .whitespaces)
            if title.isEmpty { continue }
            let kindRaw = (item["kind"] as? String ?? "").trimmingCharacters(in: .whitespaces)
            let kind = kindRaw.isEmpty ? "linux" : kindRaw
            // 折叠子屏（rEFInd fold_linux_kernels）：同分区多个内核在 children 里。
            // 展开成平铺 option —— value 用**父条目 title 原文**（切到
            // /api/switch?system=<父title> → 后端 resolve_sim_title 命中顶层条目
            // → refind_sim_switch），label 显示「父条目 › 内核名」。
            // 只展开「折叠的内核」子项（source=kernel）；macOS 诊断项等不展开。
            let children = item["children"] as? [[String: Any]] ?? []
            // 与 PC simRows 一致：展开**全部** children（含 默认项/refind_linux.conf/macOS 诊断/折叠内核），带右侧来源标签。
            let childRows: [(String, String)] = children.compactMap { child in
                let src = (child["source"] as? String ?? "").trimmingCharacters(in: .whitespaces)
                let ctitle = (child["title"] as? String ?? "").trimmingCharacters(in: .whitespaces)
                guard !ctitle.isEmpty else { return nil }
                let cname0 = ctitle.components(separatedBy: ":").first?
                    .trimmingCharacters(in: .whitespaces) ?? ctitle
                let cname = GrubFormat.stripKernel(cname0.isEmpty ? ctitle : cname0)
                let tag = GrubFormat.childSourceLabel(src)
                return (cname, tag)
            }
            if !childRows.isEmpty {
                // 父条目作组头（不可选）+ 组内子项（label 仅内核名，来源标签走 tag 字段，渲染时右对齐）
                options.append(BootOption(value: "", label: title, kind: kind, header: true))  // 统一 GRUB 口径：纯文本，不拼 ▾/emoji
                for (cname, tag) in childRows {
                    options.append(BootOption(value: title, label: cname, kind: kind, tag: tag, child: true))
                }
            } else {
                options.append(BootOption(value: title, label: title, kind: kind))
            }
            if (item["lastBoot"] as? Bool) == true && current.isEmpty { current = title }
        }
        if current.isEmpty {
            // 回显：优先 previousBoot。安卓同口径「上次启动/当前默认项」。即便没能精确
            // 命中某个 option（对端标题末尾可能带空格），也直接回显它，避免通道按钮空白。
            let prev = (json["previousBoot"] as? String ?? "").trimmingCharacters(in: .whitespaces)
            if !prev.isEmpty { current = prev }
        }
        return BootGroup(
            kind: kind, title: groupTitle,
            available: !options.isEmpty,
            current: current, next: "", saved: "",
            note: (json["summary"] as? String ?? "").trimmingCharacters(in: .whitespaces),
            options: options)
    }

    static func parseRefind(_ json: [String: Any], kind: String = BootKind.refind, title groupTitle: String = "rEFInd") -> BootGroup {
        var options: [BootOption] = []
        var current = ""
        for item in RemoteHttp.objects(json, "variables") {
            guard RemoteHttp.bool(item, "exists") else { continue }
            let system = RemoteHttp.str(item, "system").trimmingCharacters(in: .whitespaces)
            if system.isEmpty { continue }
            let rawLabel = RemoteHttp.str(item, "displayName").trimmingCharacters(in: .whitespaces)
            let label = rawLabel.isEmpty ? system : rawLabel
            if RemoteHttp.bool(item, "active") { current = system }
            options.append(BootOption(value: system, label: label, kind: refindClass(system)))
        }
        return BootGroup(
            kind: kind,
            title: groupTitle,
            available: !options.isEmpty,
            current: current,
            next: current,
            saved: current,
            note: options.isEmpty ? L("and.notDeployedRefind") : "",
            options: options)
    }

    /// `/api/grub` 的 `entries` → GRUB 可选组的**原料**（真正的拼装在 `grubDisplayRows`）
    static func parseGrub(_ json: [String: Any]) -> BootGroup {
        let next = RemoteHttp.str(json, "nextEntry").trimmingCharacters(in: .whitespaces)
        let saved = RemoteHttp.str(json, "savedEntry").trimmingCharacters(in: .whitespaces)
        let installed = RemoteHttp.bool(json, "installed")
        let serverFormat = RemoteHttp.str(json, "listFormat")

        var groupNames: [String: String] = [:]
        for item in RemoteHttp.objects(json, "groups") {
            let id = RemoteHttp.str(item, "id")
            let name = RemoteHttp.str(item, "name")
            if !id.isEmpty && !name.isEmpty { groupNames[id] = name }
        }

        var options: [BootOption] = []
        for item in RemoteHttp.objects(json, "entries") {
            let selector = RemoteHttp.str(item, "selector").trimmingCharacters(in: .whitespaces)
            if selector.isEmpty { continue }
            let name = RemoteHttp.str(item, "name").trimmingCharacters(in: .whitespaces)
            let submenu = RemoteHttp.str(item, "submenu").trimmingCharacters(in: .whitespaces)
            let klass = RemoteHttp.str(item, "class")
            let chainloader = RemoteHttp.bool(item, "chainloader")
            let kernels = RemoteHttp.strings(item, "kernels")
                .map { $0.trimmingCharacters(in: .whitespaces) }
                .filter { !$0.isEmpty }
            options.append(BootOption(
                value: selector,
                label: "",
                kind: grubClass(name: name, klass: klass, chainloader: chainloader),
                grubName: name,
                grubKernels: kernels,
                grubSubmenuId: submenu,
                grubVariants: max(1, RemoteHttp.int(item, "variants") == 0 ? 1 : RemoteHttp.int(item, "variants")),
                grubChainloader: chainloader,
                grubClassRaw: klass.trimmingCharacters(in: .whitespaces)))
        }

        let note: String
        if options.isEmpty {
            let reported = RemoteHttp.str(json, "error").trimmingCharacters(in: .whitespaces)
            note = reported.isEmpty ? L("and.notDeployedGrub") : reported
        } else {
            note = ""
        }

        return BootGroup(
            kind: BootKind.grub,
            title: "GRUB",
            available: installed || !options.isEmpty,
            current: next.isEmpty ? saved : next,
            next: next,
            saved: saved,
            note: note,
            options: options,
            serverFormat: serverFormat,
            grubGroupNames: groupNames,
            grubEfiDir: RemoteHttp.str(json, "efiDir").trimmingCharacters(in: .whitespaces),
            grubPartuuid: RemoteHttp.str(json, "partuuid").trimmingCharacters(in: .whitespaces))
    }

    static func refindClass(_ system: String) -> String {
        switch system {
        case "windows": return "win"
        case "macos": return "mac"
        default: return "linux"
        }
    }

    static func grubClass(name: String, klass: String, chainloader: Bool) -> String {
        let haystack = "\(name) \(klass)".lowercased()
        if haystack.contains("windows") || haystack.contains("win") { return "win" }
        if haystack.contains("osx") || haystack.contains("mac") || haystack.contains("apple") {
            return "mac"
        }
        if chainloader && haystack.contains("efi") { return "auto" }
        return "linux"
    }

    /// `channels`（6 个统一 BootGroup 的 JSON 数组）→ [BootGroup]。
    /// 每个元素形如 {kind,title,available,current,next,saved,note,options[]}。
    static func parseChannels(_ list: [[String: Any]]) -> [BootGroup] {
        list.map { item in
            BootGroup(
                kind: RemoteHttp.str(item, "kind").trimmingCharacters(in: .whitespaces),
                title: RemoteHttp.str(item, "title").trimmingCharacters(in: .whitespaces),
                available: RemoteHttp.bool(item, "available"),
                current: RemoteHttp.str(item, "current").trimmingCharacters(in: .whitespaces),
                next: RemoteHttp.str(item, "next").trimmingCharacters(in: .whitespaces),
                saved: RemoteHttp.str(item, "saved").trimmingCharacters(in: .whitespaces),
                note: RemoteHttp.str(item, "note").trimmingCharacters(in: .whitespaces),
                options: RemoteHttp.objects(item, "options").map(parseBootOption))
        }
    }

    /// 单条统一 BootOption：{value,label,class,header,fixed,tag,sub,note}
    static func parseBootOption(_ item: [String: Any]) -> BootOption {
        let cls = RemoteHttp.str(item, "class").trimmingCharacters(in: .whitespaces)
        return BootOption(
            value: RemoteHttp.str(item, "value").trimmingCharacters(in: .whitespaces),
            label: RemoteHttp.str(item, "label").trimmingCharacters(in: .whitespaces),
            kind: cls.isEmpty ? "linux" : cls,
            header: RemoteHttp.bool(item, "header"),
            tag: RemoteHttp.str(item, "tag").trimmingCharacters(in: .whitespaces),
            fixed: RemoteHttp.bool(item, "fixed"),
            child: RemoteHttp.bool(item, "child"),
            sub: RemoteHttp.str(item, "sub").trimmingCharacters(in: .whitespaces),
            note: RemoteHttp.str(item, "note").trimmingCharacters(in: .whitespaces))
    }

    /// 老对端没有统一 `channels[6]` 时的回退：用 refind/refindplus/grub 拼出 6 个，
    /// 缺的 sdboot/opencore/clover 补成不可用的空组。GRUB 旧结构（label 空）
    /// 顺手补全成统一 option，保证通用渲染下有可见文案。
    static func fallbackChannels(refind: BootGroup,
                                 refindplus: BootGroup,
                                 grub: BootGroup) -> [BootGroup] {
        var out: [BootGroup] = [refind, refindplus]
        var g = grub
        g.options = grub.options.map { o in
            var n = o
            if n.label.isEmpty { n.label = GrubFormat.kernelOf(o) }
            if n.label.isEmpty { n.label = o.value }
            return n
        }
        out.append(g)
        for kind in [BootKind.sdboot, BootKind.opencore, BootKind.clover] {
            out.append(BootGroup(kind: kind, title: BootKind.defaultTitle(kind),
                                 available: false, current: "", next: "", saved: "",
                                 note: "", options: []))
        }
        return out
    }
}

// MARK: - GRUB 六档拼装（对齐 ComputerListActivity.kt:2690-2821）

enum GrubFormat {

    /// 六个 chip：跟随电脑（""）+ 极简 / 简易 / 中等 / 原始 / 复杂
    static let all = ["", "minimal", "simple", "medium", "raw", "detailed"]

    static func osIcon(_ text: String) -> String {
        let lowered = text.lowercased()
        if lowered.contains("windows") { return "🪟 " }
        if lowered.contains("mac") || lowered.contains("apple") { return "🍎 " }
        if lowered.contains("linux") { return "🐧 " }
        return ""
    }

    static func childSourceLabel(_ source: String) -> String {
        switch source {
        case "default": return L("hub.child.default")
        case "options": return "refind_linux.conf"
        case "kernel": return L("hub.child.kernel")
        case "macos": return L("hub.child.macosDiag")
        case "stanza": return "submenuentry"
        default: return ""
        }
    }
    static func stripKernel(_ name: String) -> String {
        var value = name.trimmingCharacters(in: .whitespaces)
        if value.hasPrefix("vmlinuz-") { value.removeFirst("vmlinuz-".count) }
        if value.hasPrefix("linux-") { value.removeFirst("linux-".count) }
        return value
    }

    /// 内核名：优先 linux 行解析出的文件名，空白按「没有」算，退回标题原文
    static func kernelOf(_ option: BootOption) -> String {
        option.grubKernels.first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
            ?? option.grubName
    }

}

// MARK: - 端点选择 + 读取 / 切换

/// 选点结果：url 是完整 origin（http:// 或 https:// 开头），token 配套凭据
struct BootEndpoint {

    var url: String
    var token: String
    var alive: Bool
    /// 探测赢家顺手带回来的系统族
    var os: String = ""
    var hits: [ProbeEngine.Hit] = []
    var kind: String = ""
    /// 赢家落在哪个候选上：局域网 / 跳板机 = 那个直连地址，远程 = 通道地址。
    /// 一址一跳板机：卡片按它回答「用的是这个地址的那台跳板机」（url 对跳板机赢家是本机端口）
    var winAddr: String = ""

    /// 卡片 / 离线文案里显示用的地址（不带 scheme）
    var displayAddr: String {
        var value = url
        if value.hasPrefix("https://") { value.removeFirst("https://".count) }
        else if value.hasPrefix("http://") { value.removeFirst("http://".count) }
        return value
    }
}

enum BootService {

    /// 这台电脑在某个地址上的 HTTP 端点（走跳板机就先拉**那个地址自己的**隧道）。
    /// 一址一跳板机：默认取首选地址，也可以指定某个直连地址。
    static func localEndpoint(_ profile: ServerProfile,
                             address: String? = nil) async throws -> String {
        let raw = address ?? profile.host
        guard let host = ServerProfile.normalizeHost(raw), !host.isEmpty else {
            throw RemoteHttpError(message: L("error.noAddress"))
        }
        let jumpId = profile.jumpServerId(for: host)
        if jumpId.isEmpty { return host }
        return try await SshSessionManager.shared.tunnelAddress(for: profile, destination: host, jumpId: jumpId)
    }

    /// 选点：**完全按「连接策略」探**（ProbeEngine），全都不通时仍带局域网端点回来（alive=false）
    static func chooseEndpoint(_ profile: ServerProfile) async -> BootEndpoint? {
        let lanToken = profile.token
        // 一址一跳板机：解析器按「要探的那个地址」查它自己那台跳板机，各建各的隧道
        let resolver: ProbeEngine.JumpResolver? = profile.jumpBindings.isEmpty
            ? nil
            : { target in try await localEndpoint(profile, address: target) }

        let result = await ProbeEngine.probe(profile, jump: resolver)
        if result.ok && !result.origin.isEmpty {
            let isLocal = result.kind == ServerProfile.routeLan || result.kind == ServerProfile.routeJump
            if isLocal {
                return BootEndpoint(url: "http://" + result.origin, token: lanToken, alive: true,
                                    os: result.os, hits: result.hits, kind: result.kind,
                                    winAddr: result.winOrigin)
            }
            // 令牌取**赢家那条配对**自己的：同机 cloudflare 与 tsnet 是两个令牌
            return BootEndpoint(url: result.origin, token: tokenForPair(profile, result.pairId),
                                alive: true, os: result.os, hits: result.hits, kind: result.kind,
                                winAddr: result.winOrigin)
        }

        // 全都不通：带一个「看起来该连」的地址回去，卡片照样显示它的 IP
        let lan = (try? await localEndpoint(profile)) ?? ""
        if !lan.isEmpty {
            return BootEndpoint(url: "http://" + lan, token: lanToken, alive: false,
                                hits: result.hits, kind: result.kind)
        }
        if let primary = profile.routeCandidates().first(where: { $0.isRemote }),
           !primary.origin.isEmpty {
            return BootEndpoint(url: primary.origin, token: primary.token, alive: false,
                                hits: result.hits, kind: result.kind)
        }
        return nil
    }

    /// 按配对 id 取令牌；找不到就退回生效配对那条
    static func tokenForPair(_ profile: ServerProfile, _ pairId: String) -> String {
        let id = pairId.trimmingCharacters(in: .whitespaces)
        if !id.isEmpty {
            for pair in profile.pairs where pair.id == id { return pair.token }
        }
        return profile.effectiveRemoteToken
    }

    /// 探活 + 读两套引导项（一边失败不影响另一边）
    /// 每台电脑上次成功的端点（profileId → BootEndpoint）。
    ///
    /// 2026-11-02 用户定案：「我并不想数据刷新会调用重新探测地址 那样地址会频繁换
    /// 只有离线的时候用重新探测地址接管才稳妥」。
    /// 快路径直接复用这个端点；失败才走完整探测（离线接管）。
    private static var lastGoodEndpoints: [String: BootEndpoint] = [:]
    private static let lastGoodLock = NSLock()

    private static func cachedEndpoint(_ id: String) -> BootEndpoint? {
        lastGoodLock.lock(); defer { lastGoodLock.unlock() }
        return lastGoodEndpoints[id]
    }

    private static func setCachedEndpoint(_ id: String, _ endpoint: BootEndpoint?) {
        lastGoodLock.lock(); defer { lastGoodLock.unlock() }
        if let endpoint { lastGoodEndpoints[id] = endpoint } else { lastGoodEndpoints.removeValue(forKey: id) }
    }

    static func fetchBootInfo(_ profile: ServerProfile) async -> BootInfo {
        // ① 快路径：用上次成功的端点直接拉 /api/ui-state（一次请求 = 验证 + 取数）
        // 用户在设置里选了「每次重新探测地址」时跳过复用
        let reprobeAlways = UserDefaults.standard.bool(forKey: "reprobeAlways")
        if !reprobeAlways, let cached = cachedEndpoint(profile.id), cached.alive {
            if let state = try? await RemoteHttp.json(endpoint: cached.url, token: cached.token,
                                                      path: "/api/ui-state") {
                return await buildInfo(fromUiState: state, choice: cached)
            }
            setCachedEndpoint(profile.id, nil)   // 直连失败 → 清缓存
        }

        // ② 慢路径：完整探测重新选路（离线接管）
        guard let choice = await chooseEndpoint(profile) else {
            return .offline("", L("error.noAddress"))
        }
        guard choice.alive else {
            setCachedEndpoint(profile.id, nil)
            return .offline(choice.displayAddr, "", hits: choice.hits)
        }
        setCachedEndpoint(profile.id, choice)

        var errors: [String] = []

        // 老服务端没有该接口时自动回落到三个老接口
        if let state = try? await RemoteHttp.json(endpoint: choice.url, token: choice.token,
                                                  path: "/api/ui-state") {
            return await buildInfo(fromUiState: state, choice: choice)
        }

        var pendingSeconds = 0
        var pendingSummary = ""
        let refindGroup: BootGroup
        do {
            let json = try await RemoteHttp.json(endpoint: choice.url, token: choice.token,
                                                 path: "/api/status")
            // 挂起的远程切换倒计时（对端 /api/status 的 switchPending 字段）：
            // pendingBar 显示「N 秒后执行」，撤销按钮走 /api/pending-cancel 立即作废
            let sp = json["switchPending"] as? [String: Any]
            if let flag = sp?["pending"] as? Bool, flag {
                pendingSeconds = sp?["secondsLeft"] as? Int ?? 0
                pendingSummary = sp?["summary"] as? String ?? ""
            }
            refindGroup = BootParser.parseRefind(json)
        } catch {
            let message = (error as? RemoteHttpError)?.message ?? error.localizedDescription
            errors.append(message)
            refindGroup = BootGroup(kind: BootKind.refind, title: "rEFInd", available: false,
                                    current: "", next: "", saved: "", note: message, options: [])
        }

        // RefindPlus 通道（与 rEFInd 并列）；老对端没有 → 占位不可用（对齐 Android 老路径）
        var refindplusGroup: BootGroup
        do {
            let json = try await RemoteHttp.json(endpoint: choice.url, token: choice.token,
                                                 path: "/api/status")
            refindplusGroup = BootParser.parseRefind(json, kind: BootKind.refindplus, title: "RefindPlus")
        } catch {
            refindplusGroup = BootGroup(kind: BootKind.refindplus, title: "RefindPlus", available: false,
                                        current: "", next: "", saved: "", note: "", options: [])
        }

        var grubGroup: BootGroup
        do {
            let json = try await RemoteHttp.json(endpoint: choice.url, token: choice.token,
                                                 path: "/api/grub")
            grubGroup = BootParser.parseGrub(json)
        } catch {
            let message = (error as? RemoteHttpError)?.message ?? error.localizedDescription
            errors.append(message)
            grubGroup = BootGroup(kind: BootKind.grub, title: "GRUB", available: false,
                                  current: "", next: "", saved: "", note: message, options: [])
        }
        // 挂起的远程切换倒计时挂到 GRUB 组（pendingBar 消费）
        grubGroup.pendingSeconds = pendingSeconds
        grubGroup.pendingSummary = pendingSummary

        // 统一 6 引导器通道：失败不算致命；老对端没有 kind 字段时用三大引导器回退拼 6 个
        let channelGroups: [BootGroup]
        do {
            let list = try await RemoteHttp.array(endpoint: choice.url, token: choice.token,
                                                  path: "/api/boot-channels")
            if list.contains(where: { !(($0["kind"] as? String) ?? "").trimmingCharacters(in: .whitespaces).isEmpty }) {
                channelGroups = BootParser.parseChannels(list)
            } else {
                channelGroups = BootParser.fallbackChannels(refind: refindGroup,
                                                            refindplus: refindplusGroup, grub: grubGroup)
            }
        } catch {
            channelGroups = BootParser.fallbackChannels(refind: refindGroup,
                                                        refindplus: refindplusGroup, grub: grubGroup)
        }

        return BootInfo(
            ok: errors.count < 3,
            online: true,
            addr: choice.displayAddr,
            error: errors.joined(separator: "；"),
            refind: refindGroup,
            refindplus: refindplusGroup,
            grub: grubGroup,
            channels: channelGroups,
            machineOs: choice.os,
            hits: choice.hits,
            winnerKind: choice.kind,
            winnerAddr: choice.winAddr)
    }

    /// 从 `/api/ui-state` 的完整快照组装 BootInfo（与电脑页同一份数据源）。
    ///
    /// 快照结构：`{ status: {...}, grub: {...}, channels: [...] }` —— 各段与老接口
    /// 的返回**同构**，所以直接复用现成的解析器。
    private static func buildInfo(fromUiState state: [String: Any],
                                  choice: BootEndpoint) async -> BootInfo {
        var errors: [String] = []

        // 2026-11-02 用户指出：「电脑列表页…没有关注到 refind(自动/手动)
        // grub(自动/手动) 的逻辑 根据用户选的自动手动才能展示相应的引导数据」。
        //
        // 快照里两套数据并存：
        //   rEFInd  自动 → refindSim（按扫描规则重算的菜单，含子屏）
        //           手动 → status（vars 快照）
        //   GRUB    自动 → linuxGrub（Linux 分区自己的 grub.cfg）
        //           手动 → grub（ESP 上的 grub.cfg）
        let settings = state["settings"] as? [String: Any] ?? [:]
        let refindManual = (settings["refindMode"] as? String) == "manual"
        let grubManual = (settings["grubMode"] as? String) == "manual"

        let refindGroup: BootGroup
        if refindManual, let status = state["status"] as? [String: Any] {
            refindGroup = BootParser.parseRefind(status)
        } else if !refindManual, let sim = state["refindSim"] as? [String: Any] {
            refindGroup = BootParser.parseRefindSim(sim)
        } else if let status = state["status"] as? [String: Any] {
            refindGroup = BootParser.parseRefind(status)
        } else {
            errors.append(L("and.snapshotNoStatus"))
            refindGroup = BootGroup(kind: BootKind.refind, title: "rEFInd", available: false,
                                    current: "", next: "", saved: "",
                                    note: L("and.snapshotNoStatus"), options: [])
        }

        let refindplusGroup: BootGroup
        let refplusManual = (settings["refindplusMode"] as? String) == "manual"
        if refplusManual, let rp = state["refindplus"] as? [String: Any] {
            refindplusGroup = BootParser.parseRefind(rp, kind: BootKind.refindplus, title: "RefindPlus")
        } else if let sim = state["refindplusSim"] as? [String: Any] {
            refindplusGroup = BootParser.parseRefindSim(sim, kind: BootKind.refindplus, title: "RefindPlus")
        } else if let rp = state["refindplus"] as? [String: Any] {
            refindplusGroup = BootParser.parseRefind(rp, kind: BootKind.refindplus, title: "RefindPlus")
        } else {
            refindplusGroup = BootGroup(kind: BootKind.refindplus, title: "RefindPlus", available: false,
                                        current: "", next: "", saved: "", note: "", options: [])
        }

        let grubGroup: BootGroup
        // 自动模式用 linuxGrub；两者 entries/groups/nextEntry/savedEntry 结构
        // 完全相同，解析器可直接复用。
        let grubSource: [String: Any]? = grubManual
            ? (state["grub"] as? [String: Any])
            : ((state["linuxGrub"] as? [String: Any]) ?? (state["grub"] as? [String: Any]))
        if let grub = grubSource {
            grubGroup = BootParser.parseGrub(grub)
        } else {
            errors.append(L("and.snapshotNoGrub"))
            grubGroup = BootGroup(kind: BootKind.grub, title: "GRUB", available: false,
                                  current: "", next: "", saved: "",
                                  note: L("and.snapshotNoGrub"), options: [])
        }

        // 6 引导器通道：优先拉 /api/boot-channels（新契约，含格式化好的 label 与 opencore options），
        // 失败/老对端才回退到 ui-state 的旧通道或三大引导器拼 6 段。
        let channelGroups: [BootGroup]
        do {
            let list = try await RemoteHttp.array(endpoint: choice.url, token: choice.token,
                                                  path: "/api/boot-channels")
            if list.contains(where: { !(($0["kind"] as? String) ?? "").trimmingCharacters(in: .whitespaces).isEmpty }) {
                channelGroups = BootParser.parseChannels(list)
            } else {
                channelGroups = BootParser.fallbackChannels(refind: refindGroup,
                                                            refindplus: refindplusGroup, grub: grubGroup)
            }
        } catch {
            channelGroups = BootParser.fallbackChannels(refind: refindGroup,
                                                        refindplus: refindplusGroup, grub: grubGroup)
        }

        return BootInfo(
            ok: errors.isEmpty,
            online: true,
            addr: choice.displayAddr,
            error: errors.joined(separator: "；"),
            refind: refindGroup,
            refindplus: refindplusGroup,
            grub: grubGroup,
            channels: channelGroups,
            machineOs: choice.os,
            hits: choice.hits,
            winnerKind: choice.kind,
            winnerAddr: choice.winAddr)
    }

    /// 切换引导项；返回给用户看的 message
    /** 撤销挂起的切换倒计时：GET /api/pending-cancel（对端立即作废 pending） */
    static func cancelPending(_ profile: ServerProfile) async throws -> String {
        guard let choice = await chooseEndpoint(profile) else {
            throw RemoteHttpError(message: L("error.noAddress"))
        }
        guard choice.alive else {
            throw RemoteHttpError(message: L("error.hostOffline"))
        }
        let json = try await RemoteHttp.json(endpoint: choice.url, token: choice.token,
                                             path: "/api/pending-cancel")
        let message = RemoteHttp.str(json, "message").trimmingCharacters(in: .whitespaces)
        return message.isEmpty ? L("and.undoPendingDone") : message
    }

    static func switchBoot(_ profile: ServerProfile, kind: String, value: String) async throws -> String {
        let target = value.trimmingCharacters(in: .whitespaces)
        if target.isEmpty { throw RemoteHttpError(message: L("and.noSwitchTargetBoot")) }
        guard let choice = await chooseEndpoint(profile) else {
            throw RemoteHttpError(message: L("error.noAddress"))
        }
        guard choice.alive else {
            throw RemoteHttpError(message: L("error.hostOffline"))
        }
        let encoded = target.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? target
        let path: String
        if kind == BootKind.refind {
            path = "/api/switch?system=\(encoded)"
        } else if kind == BootKind.refindplus {
            path = "/api/switch?system=\(encoded)&channel=refindplus"
        } else if kind == BootKind.grub {
            path = "/api/grub-switch?entry=\(encoded)"
        } else {
            // sdboot / opencore / clover：value 是完整 spec
            // （opencore-default:<卷>\u{1}<路径> / sdboot:<id> / clover-oneshot …），
            // 在第一个冒号处拆成 channel+target → 通道切换端点；绝不能落到 grub-switch
            // （否则「当前 grub.cfg 里没有这一项」，OpenCore 选中被当 GRUB 的根因）。
            guard let sep = target.firstIndex(of: ":") else {
                throw RemoteHttpError(message: L("and.cannotSetDefault"))
            }
            let chName = String(target[target.startIndex..<sep])
            let chTarget = String(target[target.index(after: sep)...])
            let encCh = chName.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? chName
            var p2 = "/api/boot-channel-switch?channel=\(encCh)"
            if !chTarget.isEmpty {
                let encT = chTarget.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? chTarget
                p2 += "&target=\(encT)"
            }
            path = p2
        }
        let json = try await RemoteHttp.json(endpoint: choice.url, token: choice.token, path: path)
        let message = RemoteHttp.str(json, "message").trimmingCharacters(in: .whitespaces)
        if !message.isEmpty { return message }
        if RemoteHttp.bool(json, "ok") || !json.keys.contains("ok") {
            return L("and.nextBootSet")
        }
        let failure = RemoteHttp.str(json, "error").trimmingCharacters(in: .whitespaces)
        return failure.isEmpty ? L("and.requestRejected") : failure
    }

    /// GRUB 条目文字格式偏好：落盘 key = `grubListFormat`，命名空间 `anyboot_ui`
    /// （不是 profile 字段 —— 与 Android 同 key，一台手机一个偏好）
    static let formatKey = "grubListFormat"

    static var selectedFormat: String {
        get { UserDefaults.standard.string(forKey: formatKey) ?? "" }
        set { UserDefaults.standard.set(newValue, forKey: formatKey) }
    }
}
