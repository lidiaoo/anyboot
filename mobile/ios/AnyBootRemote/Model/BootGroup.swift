import Foundation

// MARK: - 引导管理数据层（对齐 ComputerListActivity.kt:2355-2821）
//
// AnyBoot 的核心功能：读电脑上的 rEFInd EFI 变量与 GRUB 条目，切换下次启动项。
// Android 用 ~1,300 行实现；iOS 此前**完全没有**，这里逐条移植。

enum BootKind {
    static let refind = "refind"
    static let grub = "grub"
}

/// 一个可选的启动项（rEFInd 的一条 EFI 变量，或 GRUB 的一个 menuentry）
struct BootOption: Hashable, Identifiable {

    var value: String            // rEFInd: system；GRUB: selector
    var label: String = ""       // rEFInd 的 displayName
    var kind: String = "linux"   // win / mac / linux / auto

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
    var grub: BootGroup
    /// 三引导器通道（sdboot / opencore / clover）；PC 版本太旧时为空数组
    var channels: [ChannelGroup] = []
    var machineOs: String = ""
    var hits: [ProbeEngine.Hit] = []
    var winnerKind: String = ""

    static func offline(_ addr: String, _ error: String) -> BootInfo {
        let note = "电脑离线，无法读取引导项"
        return BootInfo(
            ok: false, online: false, addr: addr, error: error,
            refind: BootGroup(kind: BootKind.refind, title: "rEFInd", available: false,
                              current: "", next: "", saved: "", note: note, options: []),
            grub: BootGroup(kind: BootKind.grub, title: "GRUB", available: false,
                            current: "", next: "", saved: "", note: note, options: []))
    }
}

/// 弹层里的一行（顶层项 / 组头 / 组内子项）
struct GrubRow: Identifiable {

    var option: BootOption?
    var text: String
    var isGroup: Bool

    var id: String {
        if let option { return (isGroup ? "t:" : "o:") + option.value }
        return "g:" + text
    }
}

// MARK: - 三引导器通道（systemd-boot / OpenCore / Clover，对齐后端 ChannelView）

/// 三引导器通道的一条可执行动作（后端 `ChannelEntry`，camelCase）
struct ChannelEntryView: Hashable, Identifiable {
    var id: String          // sdboot: 条目名；opencore/clover: 动作 id（本身就是切换值域）
    var title: String
    var detail: String
    var icon: String        // linux / windows / macos / menu
    var requiresElevation: Bool
}

/// 一条引导器通道（后端 `ChannelView`，camelCase 序列化，GET /api/boot-channels）
struct ChannelGroup: Hashable, Identifiable {
    var id: String          // sdboot / opencore / clover
    var label: String
    var detected: Bool
    var permission: String  // "file" = 免提权；"nvram" = 切换必经 NVRAM 写
    var chip: String        // ok / warn / off
    var detail: String
    var entries: [ChannelEntryView]
    var currentDefault: String
    var lastBoot: String
    var error: String
    var health: [String]
}

// MARK: - 解析

enum BootParser {

    /// `/api/status` 的 `variables` → rEFInd 可选组（对齐 PC 端 refind_from_status）
    static func parseRefind(_ json: [String: Any]) -> BootGroup {
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
            kind: BootKind.refind,
            title: "rEFInd",
            available: !options.isEmpty,
            current: current,
            next: current,
            saved: current,
            note: options.isEmpty ? "这台电脑没部署 rEFInd（或还没设置过下次启动项）" : "",
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
            note = reported.isEmpty ? "这台电脑没部署 GRUB（或还没同步 grub.cfg）" : reported
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

    /// `GET /api/boot-channels`（顶层就是 ChannelView 数组）→ 三引导器通道
    static func parseChannels(_ list: [[String: Any]]) -> [ChannelGroup] {
        list.map { item in
            let entries: [ChannelEntryView] = RemoteHttp.objects(item, "entries").map { entry in
                ChannelEntryView(
                    id: RemoteHttp.str(entry, "id").trimmingCharacters(in: .whitespaces),
                    title: RemoteHttp.str(entry, "title").trimmingCharacters(in: .whitespaces),
                    detail: RemoteHttp.str(entry, "detail").trimmingCharacters(in: .whitespaces),
                    icon: RemoteHttp.str(entry, "icon").trimmingCharacters(in: .whitespaces),
                    requiresElevation: RemoteHttp.bool(entry, "requiresElevation"))
            }
            let health = RemoteHttp.objects(item, "health").compactMap { issue -> String? in
                let text = RemoteHttp.str(issue, "text").trimmingCharacters(in: .whitespaces)
                return text.isEmpty ? nil : text
            }
            return ChannelGroup(
                id: RemoteHttp.str(item, "id").trimmingCharacters(in: .whitespaces),
                label: RemoteHttp.str(item, "label").trimmingCharacters(in: .whitespaces),
                detected: RemoteHttp.bool(item, "detected"),
                permission: RemoteHttp.str(item, "permission").trimmingCharacters(in: .whitespaces),
                chip: RemoteHttp.str(item, "chip").trimmingCharacters(in: .whitespaces),
                detail: RemoteHttp.str(item, "detail").trimmingCharacters(in: .whitespaces),
                entries: entries,
                currentDefault: RemoteHttp.str(item, "currentDefault").trimmingCharacters(in: .whitespaces),
                lastBoot: RemoteHttp.str(item, "lastBoot").trimmingCharacters(in: .whitespaces),
                error: RemoteHttp.str(item, "error").trimmingCharacters(in: .whitespaces),
                health: health)
        }
    }
}

// MARK: - GRUB 六档拼装（对齐 ComputerListActivity.kt:2690-2821）

enum GrubFormat {

    /// 六个 chip：跟随电脑（""）+ 极简 / 简易 / 中等 / 原始 / 复杂
    static let all = ["", "minimal", "simple", "medium", "raw", "detailed"]

    static func label(_ format: String) -> String {
        switch format {
        case "minimal": return "极简"
        case "simple": return "简易"
        case "medium": return "中等"
        case "raw": return "原始"
        case "detailed": return "复杂"
        default: return "跟随电脑"
        }
    }

    /// 这次渲染该用哪一档：手机自选优先，没选过就跟随服务端，再退 minimal
    static func effective(selected: String, group: BootGroup) -> String {
        if !selected.isEmpty { return selected }
        return group.serverFormat.isEmpty ? "minimal" : group.serverFormat
    }

    /// 系统图标（与托盘 grub_os_icon 同一规则，不猜）
    static func osIcon(_ text: String) -> String {
        let lowered = text.lowercased()
        if lowered.contains("windows") { return "🪟 " }
        if lowered.contains("mac") || lowered.contains("apple") { return "🍎 " }
        if lowered.contains("linux") { return "🐧 " }
        return ""
    }

    static func stripKernel(_ name: String) -> String {
        var value = name
        if value.hasPrefix("vmlinuz-") { value.removeFirst("vmlinuz-".count) }
        else if value.hasPrefix("linux-") { value.removeFirst("linux-".count) }
        return value
    }

    /// 内核名：优先 linux 行解析出的文件名，空白按「没有」算，退回标题原文
    static func kernelOf(_ option: BootOption) -> String {
        option.grubKernels.first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
            ?? option.grubName
    }

    static func rowLabel(_ format: String, _ option: BootOption) -> String {
        switch format {
        case "minimal", "simple": return stripKernel(kernelOf(option))
        case "medium": return kernelOf(option)
        default: return option.grubName
        }
    }

    /// 非原始格式按渲染结果数变体；原始格式不折叠
    static func collapse(_ format: String, _ options: [BootOption]) -> [String: Int] {
        if format == "raw" { return [:] }
        var counts: [String: Int] = [:]
        for option in options {
            counts[rowLabel(format, option), default: 0] += 1
        }
        return counts
    }

    /// 拼出选择面板里的展示行 —— 与 PC 托盘「GRUB 启动项」逐条对齐
    static func displayRows(_ group: BootGroup, selectedFormat: String) -> [GrubRow] {
        let options = group.options
        if options.isEmpty { return [] }
        let format = effective(selected: selectedFormat, group: group)
        let names = group.grubGroupNames
        var rows: [GrubRow] = []

        func push(_ option: BootOption, top: Bool, prefix: String,
                  counts: [String: Int], emitted: inout Set<String>) {
            let label = rowLabel(format, option)
            var parts: [String] = []
            if format == "raw" {
                // 原始格式不折叠；变体数用解析期并进的 grubVariants
                if option.grubVariants > 1 { parts.append("\(option.grubVariants) 个变体") }
            } else {
                if emitted.contains(label) { return }
                emitted.insert(label)
                let variants = counts[label] ?? 1
                if variants > 1 { parts.append("\(variants) 个变体") }
            }
            if format == "raw" || format == "detailed" {
                let tag: String
                if option.grubKernels.contains(where: { !$0.trimmingCharacters(in: .whitespaces).isEmpty }) {
                    tag = option.grubKernels.first { !$0.trimmingCharacters(in: .whitespaces).isEmpty } ?? ""
                } else if option.grubChainloader {
                    tag = "转交引导"
                } else {
                    tag = option.grubClassRaw
                }
                if !tag.trimmingCharacters(in: .whitespaces).isEmpty { parts.append(tag) }
            }
            let suffix = parts.isEmpty ? "" : " · " + parts.joined(separator: " · ")
            let icon = osIcon(prefix + option.grubName)
            let arrow = top ? "" : "↳ "
            rows.append(GrubRow(option: option, text: "\(arrow)\(icon)\(prefix)\(label)\(suffix)",
                                isGroup: false))
        }

        // 置顶当前选中项（组内子项补「组名 › 」前缀）
        if let pinned = options.first(where: { $0.value == group.current }) {
            var prefix = ""
            if let name = names[pinned.grubSubmenuId],
               !name.trimmingCharacters(in: .whitespaces).isEmpty {
                prefix = "\(name) › "
            }
            var emitted = Set<String>()
            push(pinned, top: true, prefix: prefix, counts: [:], emitted: &emitted)
        }

        // 组头附加的 ESP 位置（简易/中等/复杂带，极简/原始不带）
        var location = ""
        if format != "minimal" && format != "raw" {
            var parts: [String] = []
            if !group.grubEfiDir.isEmpty { parts.append("位于\(group.grubEfiDir)") }
            if !group.grubPartuuid.isEmpty { parts.append("UUID=\(group.grubPartuuid)") }
            if !parts.isEmpty { location = " · " + parts.joined(separator: " ") }
        }

        let children = options.filter { !$0.grubSubmenuId.isEmpty }
        let topCounts = collapse(format, options.filter { $0.grubSubmenuId.isEmpty })
        var topEmitted = Set<String>()
        var emittedGroups = Set<String>()

        for option in options {
            let submenuId = option.grubSubmenuId
            if submenuId.isEmpty {
                push(option, top: true, prefix: "", counts: topCounts, emitted: &topEmitted)
                continue
            }
            if emittedGroups.contains(submenuId) { continue }
            emittedGroups.insert(submenuId)
            let rawTitle = names[submenuId] ?? ""
            let title = rawTitle.trimmingCharacters(in: .whitespaces).isEmpty ? submenuId : rawTitle
            let groupChildren = children.filter { $0.grubSubmenuId == submenuId }
            rows.append(GrubRow(option: nil,
                                text: "▸ \(osIcon(title))\(title)（\(groupChildren.count) 项\(location)）",
                                isGroup: true))
            let counts = collapse(format, groupChildren)
            var emitted = Set<String>()
            for child in groupChildren {
                push(child, top: false, prefix: "", counts: counts, emitted: &emitted)
            }
        }
        return rows
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

    /// 卡片 / 离线文案里显示用的地址（不带 scheme）
    var displayAddr: String {
        var value = url
        if value.hasPrefix("https://") { value.removeFirst("https://".count) }
        else if value.hasPrefix("http://") { value.removeFirst("http://".count) }
        return value
    }
}

enum BootService {

    /// 这台电脑的 HTTP 端点（走跳板机先拉一条本地隧道）
    static func localEndpoint(_ profile: ServerProfile) async throws -> String {
        guard let host = ServerProfile.normalizeHost(profile.host), !host.isEmpty else {
            throw RemoteHttpError(message: "这台电脑还没有可用地址")
        }
        if profile.jumpServerId.trimmingCharacters(in: .whitespaces).isEmpty { return host }
        return try await SshSessionManager.shared.tunnelAddress(for: profile)
    }

    /// 选点：**完全按「连接策略」探**（ProbeEngine），全都不通时仍带局域网端点回来（alive=false）
    static func chooseEndpoint(_ profile: ServerProfile) async -> BootEndpoint? {
        let lanToken = profile.token
        let resolver: ProbeEngine.JumpResolver? = profile.jumpServerId
            .trimmingCharacters(in: .whitespaces).isEmpty
            ? nil
            : { try await localEndpoint(profile) }

        let result = await ProbeEngine.probe(profile, jump: resolver)
        if result.ok && !result.origin.isEmpty {
            let isLocal = result.kind == ServerProfile.routeLan || result.kind == ServerProfile.routeJump
            if isLocal {
                return BootEndpoint(url: "http://" + result.origin, token: lanToken, alive: true,
                                    os: result.os, hits: result.hits, kind: result.kind)
            }
            // 令牌取**赢家那条配对**自己的：同机 cloudflare 与 tsnet 是两个令牌
            return BootEndpoint(url: result.origin, token: tokenForPair(profile, result.pairId),
                                alive: true, os: result.os, hits: result.hits, kind: result.kind)
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
    static func fetchBootInfo(_ profile: ServerProfile) async -> BootInfo {
        guard let choice = await chooseEndpoint(profile) else {
            return .offline("", "这台电脑还没有可用地址")
        }
        guard choice.alive else {
            return .offline(choice.displayAddr, "")
        }

        var errors: [String] = []

        let refindGroup: BootGroup
        do {
            let json = try await RemoteHttp.json(endpoint: choice.url, token: choice.token,
                                                 path: "/api/status")
            refindGroup = BootParser.parseRefind(json)
        } catch {
            let message = (error as? RemoteHttpError)?.message ?? error.localizedDescription
            errors.append(message)
            refindGroup = BootGroup(kind: BootKind.refind, title: "rEFInd", available: false,
                                    current: "", next: "", saved: "", note: message, options: [])
        }

        let grubGroup: BootGroup
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

        // 三引导器通道：失败不算致命（PC 版本旧没有这个接口，通道静默为空）
        let channelGroups: [ChannelGroup]
        do {
            let list = try await RemoteHttp.array(endpoint: choice.url, token: choice.token,
                                                  path: "/api/boot-channels")
            channelGroups = BootParser.parseChannels(list)
        } catch {
            channelGroups = []
        }

        return BootInfo(
            ok: errors.count < 3,
            online: true,
            addr: choice.displayAddr,
            error: errors.joined(separator: "；"),
            refind: refindGroup,
            grub: grubGroup,
            channels: channelGroups,
            machineOs: choice.os,
            hits: choice.hits,
            winnerKind: choice.kind)
    }

    /// 切换引导项；返回给用户看的 message
    static func switchBoot(_ profile: ServerProfile, kind: String, value: String) async throws -> String {
        let target = value.trimmingCharacters(in: .whitespaces)
        if target.isEmpty { throw RemoteHttpError(message: "没有可切换的目标启动项") }
        guard let choice = await chooseEndpoint(profile) else {
            throw RemoteHttpError(message: "这台电脑还没有可用地址")
        }
        guard choice.alive else {
            throw RemoteHttpError(message: "电脑离线（局域网与远程都没通）")
        }
        let encoded = target.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? target
        let path = kind == BootKind.refind
            ? "/api/switch?system=\(encoded)"
            : "/api/grub-switch?entry=\(encoded)"
        let json = try await RemoteHttp.json(endpoint: choice.url, token: choice.token, path: path)
        let message = RemoteHttp.str(json, "message").trimmingCharacters(in: .whitespaces)
        if !message.isEmpty { return message }
        if RemoteHttp.bool(json, "ok") || !json.keys.contains("ok") {
            return "已设置下次启动项"
        }
        let failure = RemoteHttp.str(json, "error").trimmingCharacters(in: .whitespaces)
        return failure.isEmpty ? "请求被拒绝" : failure
    }

    /// 三引导器通道切换：`GET /api/boot-channel-switch?channel=&target=`。
    /// spec 组装对齐 PC 网页端 `bootChannelSpec`（ui/boot-list-view.js:531）：
    /// sdboot 条目 → channel=sdboot、target=条目 id（entry.id 就是 loader/entries 的名字）；
    /// opencore / clover 的条目 id 本身就是通道值域（`opencore`、`opencore-register`、
    /// `clover-oneshot`），target 省略。
    static func switchChannel(_ profile: ServerProfile,
                              group: ChannelGroup,
                              entry: ChannelEntryView) async throws -> String {
        let channelName: String
        let target: String
        if group.id == "sdboot" {
            channelName = "sdboot"
            target = entry.id.trimmingCharacters(in: .whitespaces)
            if target.isEmpty { throw RemoteHttpError(message: "没有可切换的目标条目") }
        } else {
            channelName = entry.id.trimmingCharacters(in: .whitespaces)
            target = ""
            if channelName.isEmpty { throw RemoteHttpError(message: "没有可执行的通道动作") }
        }
        guard let choice = await chooseEndpoint(profile) else {
            throw RemoteHttpError(message: "这台电脑还没有可用地址")
        }
        guard choice.alive else {
            throw RemoteHttpError(message: "电脑离线（局域网与远程都没通）")
        }
        func encode(_ value: String) -> String {
            value.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? value
        }
        var path = "/api/boot-channel-switch?channel=\(encode(channelName))"
        if !target.isEmpty { path += "&target=\(encode(target))" }
        let json = try await RemoteHttp.json(endpoint: choice.url, token: choice.token, path: path)
        let message = RemoteHttp.str(json, "message").trimmingCharacters(in: .whitespaces)
        if !message.isEmpty { return message }
        if RemoteHttp.bool(json, "ok") || !json.keys.contains("ok") {
            return "已设置下次启动项"
        }
        let failure = RemoteHttp.str(json, "error").trimmingCharacters(in: .whitespaces)
        return failure.isEmpty ? "请求被拒绝" : failure
    }

    /// GRUB 条目文字格式偏好：落盘 key = `grubListFormat`，命名空间 `anyboot_ui`
    /// （不是 profile 字段 —— 与 Android 同 key，一台手机一个偏好）
    static let formatKey = "grubListFormat"

    static var selectedFormat: String {
        get { UserDefaults.standard.string(forKey: formatKey) ?? "" }
        set { UserDefaults.standard.set(newValue, forKey: formatKey) }
    }
}
