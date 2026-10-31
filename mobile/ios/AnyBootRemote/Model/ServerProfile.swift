import Foundation

extension String {
    /// 去掉首尾空白（分组名 / 服务端名 / token / 机器标识输入统一走这里）
    var trimmedAnyBoot: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}

// MARK: - 电源路由

/// 一条局域网电源路由：把「动作 → 广播地址 → 端口 → 用哪些 MAC」绑起来。
///
/// 目标机上装的是 **sol**，它在局域网里按 UDP 端口区分动作（9 唤醒、别的端口
/// 重启 / 关机 / 睡眠）。字段与 PC 端 `hub.rs::PowerRoute`、Android `PowerRoute`
/// 同语义，改要三处一起改。
struct PowerRoute: Hashable, Codable {

    static let actionWake = "wake"
    static let actionReboot = "reboot"
    static let actionShutdown = "shutdown"
    static let actionSleep = "sleep"

    /// 四个电源动作（与 PC 端 hub.rs、hub.html、Android 同一套字符串）
    static let actions = [actionWake, actionSleep, actionReboot, actionShutdown]

    var action: String = PowerRoute.actionWake
    /// 广播地址（192.168.0.255 / 255.255.255.255）或单播 IP；空 = 自动
    var target: String = ""
    /// UDP 端口（0 = 9）
    var port: Int = 9
    /// 用哪些网卡的 MAC；空 = 这台电脑配过的全部
    var macs: [String] = []
    var enabled: Bool = true
    /// 这条路由每次发几个包（0 = 按动作默认：唤醒 10、其余 1）
    var burst: Int = 0
    /// 一轮没全发出去时重试几次（0 = 按动作默认：唤醒 10、其余 1）
    var retry: Int = 0
    /// sol 包没发出去时，改调这台电脑自带的 /api/power（所有动作默认勾上）
    var httpFallback: Bool = false
    /// 发送模式：auto / http_first / http_only / packet_only / hybrid；空 = 老数据按 httpFallback 推导
    var mode: String = ""

    init() {}

    init(action: String, target: String, port: Int) {
        self.action = ServerProfile.normalizePowerAction(action)
        self.target = target.trimmedAnyBoot
        self.port = port <= 0 ? 9 : port
    }

    /// 这条路由的发送模式（空 / 不认识按旧 httpFallback 推导，老档案平滑升级）
    var effectiveMode: String {
        let value = mode.trimmedAnyBoot
        if ["auto", "http_first", "http_only", "packet_only", "hybrid"].contains(value) {
            return value
        }
        return httpFallback ? "auto" : "packet_only"
    }

    var actionOrWake: String { ServerProfile.normalizePowerAction(action) }

    var effectivePort: Int { port <= 0 ? 9 : port }

    /// 每次发几个包（0 = 按动作默认，上限 50）
    var effectiveBurst: Int { burst <= 0 ? defaultBurst : min(50, burst) }

    /// 失败重试几次（0 = 按动作默认，上限 20）
    var effectiveRetry: Int { retry <= 0 ? defaultRetry : min(20, retry) }

    /// 没填发包数时的默认值：唤醒 10，其余 1
    var defaultBurst: Int { actionOrWake == Self.actionWake ? ServerProfile.defaultWakeBurst : ServerProfile.defaultPowerBurst }

    /// 没填重试次数时的默认值：唤醒 10，其余 1
    var defaultRetry: Int { actionOrWake == Self.actionWake ? ServerProfile.defaultWakeRetry : ServerProfile.defaultPowerRetry }

    func copy() -> PowerRoute { self }

    /// 换一个新字段组合（其余保持不变）
    func with(action newAction: String, target newTarget: String, port newPort: Int,
              macs newMacs: [String], enabled newEnabled: Bool) -> PowerRoute {
        var other = PowerRoute(action: newAction, target: newTarget, port: newPort)
        other.macs = newMacs
        other.enabled = newEnabled
        other.burst = burst
        other.retry = retry
        other.mode = mode
        other.httpFallback = httpFallback
        return other
    }

    /// 只改「发送模式」（其余字段照抄）；httpFallback 跟着派生，旧版兼容
    func withMode(_ value: String) -> PowerRoute {
        var other = copy()
        let cleaned = value.trimmedAnyBoot
        other.mode = ["auto", "http_first", "http_only", "packet_only"].contains(cleaned) ? cleaned : ""
        let effective = other.effectiveMode
        other.httpFallback = effective == "auto" || effective == "http_first"
        return other
    }

    /// 只改「发几个包 / 重试几次」（其余字段照抄）
    func withCounts(_ newBurst: Int, _ newRetry: Int) -> PowerRoute {
        var other = with(action: action, target: target, port: port, macs: macs, enabled: enabled)
        other.burst = newBurst <= 0 ? 0 : min(50, newBurst)
        other.retry = newRetry <= 0 ? 0 : min(20, newRetry)
        return other
    }

    private enum CodingKeys: String, CodingKey {
        case action, target, port, macs, enabled, burst, retry
        case httpFallback, httpFallbackSnake = "http_fallback"
        case mode
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        action = ServerProfile.normalizePowerAction((try? c.decodeIfPresent(String.self, forKey: .action)) ?? "")
        target = (try? c.decodeIfPresent(String.self, forKey: .target)) ?? ""
        port = (try? c.decodeIfPresent(Int.self, forKey: .port)) ?? 9
        macs = ServerProfile.cleanMacs((try? c.decodeIfPresent([String].self, forKey: .macs)) ?? [])
        enabled = (try? c.decodeIfPresent(Bool.self, forKey: .enabled)) ?? true
        burst = (try? c.decodeIfPresent(Int.self, forKey: .burst)) ?? 0
        retry = (try? c.decodeIfPresent(Int.self, forKey: .retry)) ?? 0
        httpFallback = (try? c.decodeIfPresent(Bool.self, forKey: .httpFallback)) ?? false
        if !httpFallback {
            httpFallback = (try? c.decodeIfPresent(Bool.self, forKey: .httpFallbackSnake)) ?? false
        }
        mode = ((try? c.decodeIfPresent(String.self, forKey: .mode)) ?? "").trimmedAnyBoot
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(actionOrWake, forKey: .action)
        try c.encode(target.trimmedAnyBoot, forKey: .target)
        try c.encode(effectivePort, forKey: .port)
        try c.encode(macs, forKey: .macs)
        try c.encode(enabled, forKey: .enabled)
        try c.encode(burst, forKey: .burst)
        try c.encode(retry, forKey: .retry)
        try c.encode(httpFallback, forKey: .httpFallback)
        try c.encode(mode, forKey: .mode)
    }
}

// MARK: - 解析中间结构

/// 地址输入框的解析结果：地址列表 + 每个地址的 MAC
struct ParsedAddressInput {
    var hosts: [String] = []
    var macs: [String: [String]] = [:]
    var isEmpty: Bool { hosts.isEmpty }
}

/// 配对串的解析结果（成功或失败都用它传，失败时 `error` 是给用户看的原因）
struct ParsedPair {
    var mid: String = ""
    var os: String = ""
    var host: String = ""
    /// 区块 id：cloudflare / ts / tsnet
    var kind: String = ""
    var tunnel: String = ""
    var token: String = ""
    var tunnelId: String = ""
    var error: String = ""

    var ok: Bool { error.isEmpty && !tunnel.isEmpty }
}

/// 一条配对展开出的连接候选项（令牌与超时在展开时就算好，消费方不必重复实现继承）
struct RouteCandidate: Hashable {
    let kind: String
    let origin: String
    let token: String
    let timeoutMs: Int
    let pairId: String

    var isRemote: Bool {
        kind != ServerProfile.routeLan && kind != ServerProfile.routeJump
    }
}

// MARK: - 电脑档案

/// 一条记录 = **一台电脑**（不是一个 IP）。
///
/// 与 Android `ServerProfile.java`（2062 行）1:1 同语义，是移动端的**唯一事实来源**：
///
/// - `addresses` 是有序候选地址列表（同一台电脑的多块网卡 / DHCP 换过 IP）；
/// - `pairs` 是远程接入配对（三区块多条目，不是四个单值槽）；
/// - `routeOrder` / `routeDisabled` 是连接策略（六通道优先级 + 停用）；
/// - 三层超时：`pairs[].probeTimeoutMs` → `blockTimeoutsMs[kind]` → `probeTimeoutMs`。
///
/// 「是不是同一台电脑」优先用电脑端 `/health` 给的 `machineId`（见 `sameMachine`）；
/// 拿不到标识时退回「分组名相同」这个用户可见的约定。
struct ServerProfile: Identifiable, Hashable, Codable {

    // MARK: 常量

    /// 电脑端「远程控制」的默认端口
    static let defaultPort = 12300

    /// 每块网卡默认发几个唤醒包 / 重试几次
    static let defaultWakeBurst = 10
    static let defaultWakeRetry = 10
    /// 重启 / 关机 / 睡眠默认只发 1 个包 —— 同一个包到两次，sol 就执行两次
    static let defaultPowerBurst = 1
    static let defaultPowerRetry = 1

    /// 多网卡探测：并发（首个）—— 每地址一个任务同探，任一响应立即胜出
    static let probeParallelFirst = "parallel_first"
    /// 多网卡探测：并发（全部）—— 等全部地址探完，逐行报成败与耗时，选最快的成功者
    static let probeParallelAll = "parallel_all"
    /// 多网卡探测：串行 —— 按地址顺序逐个探，第一个通的用之
    static let probeSerial = "serial"
    /// 旧值兼容：parallel = 并发（首个）
    static let probeParallel = ServerProfile.probeParallelFirst

    /// 探测超时默认值（毫秒）：局域网 RTT 通常 <10ms，2 秒足够；公网可在连接策略里调大
    static let defaultProbeTimeoutMs = 2000
    static let minProbeTimeoutMs = 200
    static let maxProbeTimeoutMs = 30000

    /// 每个档案的配对数软上限（与 PC 端 `MAX_PAIRS` 同一个值）
    static let maxPairs = 32

    /// 连接优先级六通道 id
    static let routeLan = "lan"
    static let routeJump = "jump"
    static let routeTunnel = "tunnel"
    static let routeTS = "ts"
    static let routeFunnel = "funnel"
    static let routeTSNet = "tsnet"

    /// 合法通道 id（读写档案时的白名单；加通道只改这一处 + `defaultRouteOrder`）
    static let routeKinds = [routeLan, routeJump, routeTunnel, routeTS, routeFunnel, routeTSNet]

    // MARK: 字段

    var id: String = UUID().uuidString
    /// 分组名（区分不同的电脑）；空串 = 未分组
    var group: String = ""
    /// 服务端名；空串 = 用地址兜底
    var name: String = ""
    /// 候选地址（有序，第 0 个是首选）：**唯一事实来源**
    var addresses: [String] = []
    /// 每个地址对应这块网卡的 MAC（唤醒用），key = 归一化后的 host:port
    var macs: [String: [String]] = [:]
    /// 局域网电源路由
    var powerRoutes: [PowerRoute] = []
    /// 访问令牌，可空
    var token: String = ""
    /// 电脑端 `/health` 给的机器标识（同一台电脑的多个 IP 靠它归并）；未知为空
    var machineId: String = ""
    /// 电脑端 `/health` 给的机器名；未知为空
    var machineName: String = ""
    /// 电脑端 `/health` 给的系统族：windows / macos / linux；未知为空
    var machineOs: String = ""
    /// **多网卡探测**（同一通道内多个地址之间怎么探）
    var probeMode: String = ServerProfile.probeParallel
    /// **连接优先级的探测方式**（六通道之间怎么探，默认并发全部）
    var routeProbeMode: String = ServerProfile.probeParallelAll
    /// **默认**探测超时（毫秒，默认 2000）；0 = 未设置
    var probeTimeoutMs: Int = ServerProfile.defaultProbeTimeoutMs
    /// **每个通道自己的探测超时**（通道 id → 毫秒）；缺 key = 用默认值
    var routeTimeoutsMs: [String: Int] = [:]
    /// 经由哪台跳板机（JumpServer.id）；空 = 直连
    var jumpServerId: String = ""
    /// 远程接入配对（多条目）：三个区块各自可以挂多条，全部保留、全部独立探测
    var pairs: [RemotePair] = []
    /// **区块内部**的探测策略（区块 id → parallel_first / parallel_all / serial）
    var blockProbeMode: [String: String] = [:]
    /// **区块默认**探测超时（区块 id → 毫秒）
    var blockTimeoutsMs: [String: Int] = [:]
    /// 已废弃（旧档案兼容用；读取时迁移到 routeOrder）
    var connMode: String = ""
    /// 连接优先级（六通道并列，有序）；空 = 老档案，按 connMode 迁移或默认序
    var routeOrder: [String] = []
    /// 禁用的通道 id（六个通道都能停；配对条目自身的启停走 `RemotePair.enabled`）
    var routeDisabled: [String] = []
    /// 最后改动时间（备份/云同步按条目 LWW 合并的依据；老档案缺省 nil 照常解码）
    var updatedAt: Date? = nil

    init() {}

    // MARK: 归一化

    /// 探测超时归一：0/负数 → 默认 2 秒；越界夹到 [200ms, 30s]
    static func clampProbeTimeoutMs(_ raw: Int) -> Int {
        if raw <= 0 { return defaultProbeTimeoutMs }
        return max(minProbeTimeoutMs, min(maxProbeTimeoutMs, raw))
    }

    /// 每通道超时表的清洗：非法通道 id 丢掉；0 视作「没单独设」直接删（回落默认）
    static func normalizeRouteTimeouts(_ raw: [String: Int]?) -> [String: Int] {
        var out: [String: Int] = [:]
        guard let raw else { return out }
        for (key, value) in raw {
            let id = key.trimmedAnyBoot.lowercased()
            if value <= 0 || !isRouteKind(id) { continue }
            out[id] = clampProbeTimeoutMs(value)
        }
        return out
    }

    /// 系统族归一：只认三个徽标值，其余归空（不画徽标）
    static func normalizeMachineOs(_ raw: String) -> String {
        switch raw.trimmedAnyBoot.lowercased() {
        case "windows": return "windows"
        case "macos", "darwin", "osx": return "macos"
        case "linux": return "linux"
        default: return ""
        }
    }

    /// 探测模式归一：serial / parallel_all 之外（含旧 parallel、空值）一律并发（首个）
    static func normalizeProbeMode(_ raw: String) -> String {
        switch raw.trimmedAnyBoot {
        case probeSerial: return probeSerial
        case probeParallelAll: return probeParallelAll
        default: return probeParallelFirst
        }
    }

    /// 连接优先级探测方式归一：三种合法值原样保留，空值 / 拼错的归**默认并发全部**
    static func normalizeRouteProbeMode(_ raw: String) -> String {
        let value = raw.trimmedAnyBoot
        if value == probeSerial || value == probeParallelAll || value == probeParallelFirst {
            return value
        }
        return probeParallelAll
    }

    /// 默认优先级：直连 → 跳板 → 隧道 → Tailscale 直连 → Funnel → 内置 tsnet
    static func defaultRouteOrder() -> [String] {
        [routeLan, routeJump, routeTunnel, routeTS, routeFunnel, routeTSNet]
    }

    /// 旧名兼容（早期 iOS 侧叫 cleanProbeMode）
    static func cleanProbeMode(_ raw: String?) -> String {
        normalizeProbeMode(raw ?? "")
    }

    static func isRouteKind(_ id: String) -> Bool {
        routeKinds.contains(id)
    }

    /// Funnel 判型：https + 域名含 .ts.net（与 Tailscale 100.x 直连区分）
    static func isFunnelOrigin(_ origin: String) -> Bool {
        let lower = origin.trimmedAnyBoot.lowercased()
        return lower.hasPrefix("https://") && lower.dropFirst(8).contains(".ts.net")
    }

    /// 是否 Tailscale 地址：100.64.0.0/10 裸 IP（可带端口）或 *.ts.net 域名
    static func isTailscaleAddr(_ addr: String) -> Bool {
        var value = addr.trimmedAnyBoot
        if value.isEmpty { return false }
        let lower = value.lowercased()
        if lower.hasPrefix("https://") {
            value = String(value.dropFirst(8))
        } else if lower.hasPrefix("http://") {
            value = String(value.dropFirst(7))
        }
        if let slash = value.firstIndex(of: "/") { value = String(value[..<slash]) }
        value = value.trimmedAnyBoot
        if value.hasSuffix(".ts.net") { return true }
        if value.contains(":") {
            let parts = value.split(separator: ":")
            if parts.count != 2 { return false }   // 多个冒号是 IPv6 字面量
            value = String(parts[0])
        }
        let octets = value.split(separator: ".")
        guard octets.count >= 2, let a = Int(octets[0]), let b = Int(octets[1]) else { return false }
        return a == 100 && b >= 64 && b <= 127
    }

    /// 归一化远程 origin：补 scheme（Tailscale 裸地址补 http）、剥路径与 query、补默认端口
    static func normalizeRemoteOrigin(_ raw: String) -> String {
        var value = raw.trimmedAnyBoot
        if value.isEmpty { return "" }
        let lower = value.lowercased()
        var scheme: String
        if lower.hasPrefix("https://") {
            scheme = "https://"
            value = String(value.dropFirst(8))
        } else if lower.hasPrefix("http://") {
            scheme = "http://"
            value = String(value.dropFirst(7))
        } else if isTailscaleAddr(value) {
            // 无 scheme 的 Tailscale 裸地址：补 http://（本机服务是明文 HTTP）
            scheme = "http://"
        } else {
            scheme = "https://"
        }
        var end = value.count
        if let slash = value.firstIndex(of: "/") { end = min(end, value.distance(from: value.startIndex, to: slash)) }
        if let question = value.firstIndex(of: "?") { end = min(end, value.distance(from: value.startIndex, to: question)) }
        var authority = String(value.prefix(end)).trimmedAnyBoot
        if authority.isEmpty { return "" }
        if scheme == "http://", isTailscaleAddr(authority), !authority.contains(":") {
            authority += ":\(defaultPort)"
        }
        return scheme + authority
    }

    /// 通道 id → 中文文案（列表徽章 / 连接标注共用）
    static func routeLabel(_ kind: String) -> String {
        switch kind {
        case routeLan: return "直连"
        case routeJump: return "跳板机"
        case routeTunnel: return "隧道"
        case routeTS: return "TS 直连"
        case routeFunnel: return "TS Funnel"
        case routeTSNet: return "内置 Funnel"
        default: return ""
        }
    }

    /// 动作归一化：英文 / 中文 / 旧别名都认，不认识的一律当「唤醒」
    static func normalizePowerAction(_ raw: String) -> String {
        switch raw.trimmedAnyBoot.lowercased() {
        case "wake", "wol", "wakeup", "唤醒", "开机": return PowerRoute.actionWake
        case "reboot", "restart", "重启": return PowerRoute.actionReboot
        case "shutdown", "poweroff", "power-off", "关机": return PowerRoute.actionShutdown
        case "sleep", "suspend", "睡眠": return PowerRoute.actionSleep
        default: return PowerRoute.actionWake
        }
    }

    /// 动作的中文名（提示文案用）
    static func powerActionLabel(_ action: String) -> String {
        switch normalizePowerAction(action) {
        case PowerRoute.actionReboot: return "重启"
        case PowerRoute.actionShutdown: return "关机"
        case PowerRoute.actionSleep: return "睡眠"
        default: return "唤醒"
        }
    }

    /// 新增电脑时预填的「局域网电源策略」（用户 2026-10-31）：四个动作各一条，
    /// 端口按 sol 的分配 —— 唤醒 9 / 睡眠 10 / 关机 11 / 重启 12；**默认全关** ——
    /// 只替用户把骨架搭好，开哪条自己定；发包 / 重试 10、发送模式「优先广播包」。
    /// 停用的策略不参与发送（`routesFor` 过滤 enabled），新电脑电源行为与从前一致。
    /// 与电脑端 `default_power_routes()` / hub.html `defaultPowerRoutes()` 保持一致。
    static func defaultPowerRoutes() -> [PowerRoute] {
        let actions = [PowerRoute.actionWake, PowerRoute.actionSleep,
                       PowerRoute.actionShutdown, PowerRoute.actionReboot]
        let ports = [9, 10, 11, 12]
        var result: [PowerRoute] = []
        for (index, action) in actions.enumerated() {
            var route = PowerRoute(action: action, target: "", port: ports[index])
            route.enabled = false
            route.burst = 10
            route.retry = 10
            route.mode = "auto"
            route.httpFallback = true
            result.append(route)
        }
        return result
    }

    /// MAC 形态（aa:bb:cc:dd:ee:ff / aa-bb-... / aabb.ccdd.eeff）
    private static let macPattern = try! NSRegularExpression(
        pattern: "(?:[0-9a-fA-F]{2}[:-]){5}[0-9a-fA-F]{2}|[0-9a-fA-F]{4}\\.[0-9a-fA-F]{4}\\.[0-9a-fA-F]{4}")

    private static let ipv4Pattern = try! NSRegularExpression(pattern: "^(\\d{1,3}\\.){3}\\d{1,3}$")

    private static func matches(_ regex: NSRegularExpression, _ value: String) -> Bool {
        regex.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)) != nil
    }

    /// 归一化成小写冒号分隔的 MAC；不是 MAC 返回空串
    static func normalizeMac(_ raw: String) -> String {
        let value = raw.trimmedAnyBoot
        if value.isEmpty || matches(ipv4Pattern, value) { return "" }
        var source = value
        if let hit = macPattern.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)),
           let range = Range(hit.range, in: value) {
            source = String(value[range])
        }
        let hex = source.replacingOccurrences(of: ":", with: "")
            .replacingOccurrences(of: "-", with: "")
            .replacingOccurrences(of: ".", with: "")
        guard hex.count == 12, hex.allSatisfy({ $0.isHexDigit }) else { return "" }
        var out: [String] = []
        var index = hex.startIndex
        while index < hex.endIndex {
            let next = hex.index(index, offsetBy: 2)
            out.append(hex[index..<next].lowercased())
            index = next
        }
        return out.joined(separator: ":")
    }

    /// MAC 列表归一化 + 去重（保持顺序）
    static func cleanMacs(_ values: [String]?) -> [String] {
        var result: [String] = []
        guard let values else { return result }
        for value in values {
            let mac = normalizeMac(value)
            if !mac.isEmpty && !result.contains(mac) { result.append(mac) }
        }
        return result
    }

    /// 是不是一个 MAC 地址（IPv4 先排除：192.168.100.100 去掉分隔符也是 12 位十六进制）
    static func isMac(_ raw: String) -> Bool {
        let value = raw.trimmedAnyBoot
        if value.isEmpty { return false }
        if matches(ipv4Pattern, value) { return false }
        return normalizeMac(value).count == 17
    }

    /// 用户可能粘贴整条 URL，也可能只写 IP，统一归一成 host:port
    static func normalizeHost(_ raw: String) -> String? {
        var value = raw.trimmedAnyBoot
        guard !value.isEmpty else { return nil }
        if let range = value.range(of: "://") {
            value = String(value[range.upperBound...])
        }
        for separator in ["/", "?"] {
            if let range = value.range(of: separator) {
                value = String(value[..<range.lowerBound])
            }
        }
        value = value.trimmedAnyBoot
        guard !value.isEmpty else { return nil }
        if !value.contains(":") {
            value += ":\(defaultPort)"
        }
        return value
    }

    /// 归一化 + 去掉空项 + 去重（保持顺序）
    static func clean(_ raw: [String]) -> [String] {
        var result: [String] = []
        for item in raw {
            guard let value = normalizeHost(item) else { continue }
            if !result.contains(value) { result.append(value) }
        }
        return result
    }

    /// 解析地址输入框（一行一个地址，可跟多个 MAC；也兼容逗号/分号/空格分隔）
    static func parseAddressInput(_ raw: String, strict: Bool = false) -> ParsedAddressInput {
        var parsed = ParsedAddressInput()
        var lastHost = ""
        for line in raw.components(separatedBy: .newlines) {
            var currentHost = ""
            let separators = CharacterSet(charactersIn: " \t,;\u{FF0C}\u{FF1B}")
            for piece in line.components(separatedBy: separators) {
                let token = piece.trimmedAnyBoot
                if token.isEmpty { continue }
                if isMac(token) {
                    let mac = normalizeMac(token)
                    let owner = currentHost.isEmpty ? lastHost : currentHost
                    if mac.isEmpty || owner.isEmpty { continue }
                    var list = parsed.macs[owner] ?? []
                    if !list.contains(mac) { list.append(mac) }
                    parsed.macs[owner] = list
                    continue
                }
                guard let host = normalizeHost(token) else { continue }
                if strict && !token.contains(":") && !host.contains(".")
                    && !host.lowercased().hasPrefix("localhost") {
                    continue
                }
                if !parsed.hosts.contains(host) { parsed.hosts.append(host) }
                currentHost = host
                lastHost = host
            }
        }
        return parsed
    }

    /// 只要地址列表的便捷入口（编辑框的保存按钮判空用）
    static func parseAddresses(_ raw: String) -> [String] {
        parseAddressInput(raw).hosts
    }

    /// 从粘贴的 URL 里顺手把 token 挖出来
    static func token(fromURL raw: String) -> String {
        guard raw.contains("token=") else { return "" }
        if let components = URLComponents(string: raw.trimmedAnyBoot),
           let value = components.queryItems?.first(where: { $0.name == "token" })?.value {
            return value.trimmedAnyBoot
        }
        guard let range = raw.range(of: "token=") else { return "" }
        let tail = String(raw[range.upperBound...])
        return tail.split(separator: "&").first.map { String($0).trimmedAnyBoot } ?? ""
    }

    // MARK: 配对解析

    /// 区块 id 归一（老写法只在这条路上（手填地址）容忍，v2 配对串里非法值一律拒收）
    static func blockFromLegacyType(_ raw: String) -> String {
        switch raw.trimmedAnyBoot.lowercased() {
        case "cloudflare", "cf", "tunnel": return RemotePair.kindCloudflare
        case "ts", "tailscale", "funnel": return RemotePair.kindTS
        case "tsnet", "builtin", "builtin_ts": return RemotePair.kindTSNet
        default: return ""
        }
    }

    /// 没有显式类型时的兜底判型：`*.ts.net` → ts（Funnel lane）、100.x → ts（直连）、其余 → cloudflare。
    /// **不猜 tsnet** —— 内建 tsnet 与系统 Funnel 的域名同形，猜错就会把两条路混成一条。
    static func defaultBlockFor(_ origin: String) -> String {
        let value = normalizeRemoteOrigin(origin)
        if isFunnelOrigin(value) || isTailscaleAddr(value) {
            return RemotePair.kindTS
        }
        return RemotePair.kindCloudflare
    }

    /// 解析粘贴的配对内容：`anyboot://pair?v=2&…` 或直接一条带 ab_token 的 URL
    ///
    /// **配对串只认 `v=2`**：老串没有 mid / type，硬解出来的条目既无法去重也判不出区块，
    /// 结果是「配一个顶掉另一个」，所以老串一律拒收。
    static func parsePair(_ raw: String) -> ParsedPair {
        let value = raw.trimmedAnyBoot
        var out = ParsedPair()
        if value.isEmpty {
            out.error = "没认出配对码 —— 要 anyboot://pair?v=2&… 串，或 https://… / http://100.x.y.z:12300 地址"
            return out
        }
        if value.lowercased().hasPrefix("anyboot://pair?") {
            guard let components = URLComponents(string: value) else {
                out.error = "配对码不是合法 URL"
                return out
            }
            let items = components.queryItems ?? []
            func item(_ name: String) -> String {
                items.first(where: { $0.name == name })?.value?.trimmedAnyBoot ?? ""
            }
            let version = item("v")
            if version.isEmpty {
                out.error = "这是旧版配对码（缺 v=2），请在电脑上重新生成一个"
                return out
            }
            if version != "2" {
                out.error = "配对码版本不支持（v=\(version)），请在电脑上重新生成一个"
                return out
            }
            let type = item("type").lowercased()
            guard RemotePair.isBlockId(type) else {
                out.error = "配对码里的 type 非法（\(type.isEmpty ? "缺失" : type)），要 cloudflare / ts / tsnet"
                return out
            }
            out.kind = type
            out.tunnel = normalizeRemoteOrigin(item("tunnel"))
            out.mid = item("mid")
            out.os = normalizeMachineOs(item("os"))
            let host = item("host")
            out.host = host.isEmpty ? item("gateway") : host
            out.token = item("token")
            if out.token.isEmpty { out.token = item("ab_token") }
            out.tunnelId = item("tunnel_id")
            if out.tunnel.isEmpty {
                out.error = "配对码里没有可用地址（tunnel 为空）—— 让对方先开启通道再重新出码"
            }
            return out
        }
        // 直接粘 URL / 裸 host：挖 ab_token / token 参数，类型按地址形态判
        let origin = normalizeRemoteOrigin(value)
        if origin.isEmpty {
            out.error = "没认出地址"
            return out
        }
        out.tunnel = origin
        if let components = URLComponents(string: value) {
            let items = components.queryItems ?? []
            let candidate = items.first(where: { $0.name == "ab_token" })?.value?.trimmedAnyBoot ?? ""
            let fallback = items.first(where: { $0.name == "token" })?.value?.trimmedAnyBoot ?? ""
            out.token = candidate.isEmpty ? fallback : candidate
            out.kind = blockFromLegacyType(items.first(where: { $0.name == "type" })?.value ?? "")
        }
        if out.kind.isEmpty { out.kind = defaultBlockFor(origin) }
        return out
    }

    // MARK: 地址

    /// 首选地址（没有地址时是空串）
    var host: String { addresses.first ?? "" }

    /// 第 index 个候选地址（越界给空串，调用方不用自己判边界）
    func hostAt(_ index: Int) -> String {
        addresses.indices.contains(index) ? addresses[index] : ""
    }

    func host(at index: Int) -> String { hostAt(index) }

    var addressCount: Int { addresses.count }

    func hasAddress(_ value: String) -> Bool {
        guard let normalized = Self.normalizeHost(value) else { return false }
        return addresses.contains(normalized)
    }

    /// 追加一个地址（归一化 + 去重）。返回是否真的加进去了。
    @discardableResult
    mutating func addAddress(_ raw: String) -> Bool {
        guard let value = Self.normalizeHost(raw), !addresses.contains(value) else { return false }
        addresses.append(value)
        return true
    }

    /// 用输入替换地址列表（编辑入口：以输入的顺序为准）
    mutating func replaceAddresses(_ raw: [String]) {
        addresses = Self.clean(raw)
        pruneMacs()
    }

    /// 用解析结果整体替换（地址 + MAC）
    mutating func replaceParsed(_ parsed: ParsedAddressInput) {
        addresses = parsed.hosts
        macs = [:]
        for (host, list) in parsed.macs where addresses.contains(host) {
            let cleaned = Self.cleanMacs(list)
            if !cleaned.isEmpty { macs[host] = cleaned }
        }
        pruneMacs()
    }

    /// 并入解析结果（地址 + MAC 都并，已有的顺序不变）
    mutating func mergeParsed(_ parsed: ParsedAddressInput) {
        for host in parsed.hosts { addAddress(host) }
        for (host, list) in parsed.macs {
            guard addresses.contains(host) else { continue }
            var mine = macs[host] ?? []
            for mac in Self.cleanMacs(list) where !mine.contains(mac) {
                mine.append(mac)
            }
            macs[host] = mine
        }
    }

    /// 把这个地址提到最前（下次优先用它）。本来就是首选时返回 false。
    @discardableResult
    mutating func promoteAddress(_ index: Int) -> Bool {
        guard index > 0, addresses.indices.contains(index) else { return false }
        addresses.insert(addresses.remove(at: index), at: 0)
        return true
    }

    /// 并入另一条记录的地址（去重，本条已有的顺序不变），返回新增个数
    @discardableResult
    mutating func mergeAddresses(from other: ServerProfile) -> Int {
        var added = 0
        for address in other.addresses {
            if addAddress(address) { added += 1 }
            let theirs = other.macsForHost(address)
            if !theirs.isEmpty {
                var mine = macs[address] ?? []
                for mac in theirs where !mine.contains(mac) { mine.append(mac) }
                macs[address] = mine
            }
        }
        return added
    }

    /// 编辑框内容：一行一个地址；该地址配了 MAC 就跟在同一行后面（可多个）
    var addressText: String {
        addresses.map { address in
            ([address] + macsForHost(address)).joined(separator: " ")
        }.joined(separator: "\n")
    }

    /// 列表 / 错误面板里显示的地址摘要：首选地址（还有备用时补一句）
    var addressSummary: String {
        let spare = addresses.count - 1
        return spare > 0 ? "\(host)（另有 \(spare) 个地址）" : host
    }

    // MARK: MAC / 电源

    /// 该地址配的 MAC（没有返回空数组，调用方不用判空）
    func macsForHost(_ value: String) -> [String] {
        guard let normalized = Self.normalizeHost(value) else { return [] }
        return macs[normalized] ?? []
    }

    func macsAt(_ index: Int) -> [String] { macsForHost(hostAt(index)) }

    /// 这台电脑配过的所有 MAC（去重、保持地址顺序）
    func allMacs() -> [String] {
        var result: [String] = []
        for address in addresses {
            for mac in macsForHost(address) where !result.contains(mac) {
                result.append(mac)
            }
        }
        // 防御：macs 里可能残留已不在 addresses 中的键（手工改过 JSON 才会出现）
        for (_, values) in macs {
            for mac in values where !result.contains(mac) { result.append(mac) }
        }
        return result
    }

    var hasMacs: Bool { !allMacs().isEmpty }

    /// 某个动作配了哪些（启用中的）局域网路由
    func routesFor(_ action: String) -> [PowerRoute] {
        let wanted = Self.normalizePowerAction(action)
        return powerRoutes.filter { $0.enabled && $0.actionOrWake == wanted }
    }

    /// 一个动作下所有路由**聚合**后的发送模式：
    /// 任何 hybrid → hybrid；任何 http_first → http_first；全部 http_only → http_only；
    /// 否则有 auto → auto；否则 packet_only
    static func routesMode(_ routes: [PowerRoute]) -> String {
        if routes.isEmpty { return "auto" }
        var anyHybrid = false
        var anyHttpFirst = false
        var allHttpOnly = true
        var anyAuto = false
        for route in routes {
            switch route.effectiveMode {
            case "hybrid": anyHybrid = true
            case "http_first": anyHttpFirst = true
            case "auto":
                anyAuto = true
                allHttpOnly = false
            case "http_only": break
            default: allHttpOnly = false
            }
        }
        if anyHybrid { return "hybrid" }
        if anyHttpFirst { return "http_first" }
        if allHttpOnly { return "http_only" }
        return anyAuto ? "auto" : "packet_only"
    }

    /// 发送模式的展示文案（与 PC 端一致）
    static func modeLabel(_ mode: String) -> String {
        switch mode {
        case "http_first": return "优先 HTTP"
        case "http_only": return "仅 HTTP"
        case "packet_only": return "仅广播包"
        case "hybrid": return "混合（同时发）"
        default: return "优先广播包"
        }
    }

    /// 这条路由实际要发的 MAC：路由里选了就用选的，没选 = 这台电脑配过的全部
    func routeMacs(_ route: PowerRoute?) -> [String] {
        guard let route, !route.macs.isEmpty else { return allMacs() }
        return route.macs
    }

    /// 删掉不在地址列表里的 MAC（地址被用户删了，MAC 也留着没意义）
    mutating func pruneMacs() {
        macs = macs.filter { addresses.contains($0.key) }
    }

    // MARK: 配对（三区块 / 六通道）

    /// 生效配对：区块顺序 + 区块内顺序，第一条启用的；没有返回 nil
    var effectivePair: RemotePair? { flatPairs.first }

    /// 生效地址（不再是存储字段）
    var effectiveRemoteAddr: String {
        guard let pair = effectivePair else { return "" }
        return Self.normalizeRemoteOrigin(pair.addr)
    }

    /// 生效令牌（= 赢家那条自己的令牌）
    var effectiveRemoteToken: String { effectivePair?.token ?? "" }

    /// 有条目被配的远程接入（任何一条启用的配对都算）
    var hasRemote: Bool { !flatPairs.isEmpty }

    /// 清掉远程接入（所有配对）
    mutating func clearRemote() { pairs.removeAll() }

    /// 区块顺序：从 `routeOrderSorted()` 里的配对通道推导（去重保序），缺的按默认序补尾
    func pairBlockOrder() -> [String] {
        var seen: [String] = []
        for kind in routeOrderSorted() {
            let block = RemotePair.blockOfRoute(kind)
            if !block.isEmpty && !seen.contains(block) { seen.append(block) }
        }
        for block in RemotePair.blockIds where !seen.contains(block) { seen.append(block) }
        return seen
    }

    /// 某个区块里的配对（按数组顺序；`enabledOnly` = 跳过停用的）
    func pairsOfBlock(_ block: String, enabledOnly: Bool) -> [RemotePair] {
        let target = block.trimmedAnyBoot.lowercased()
        return pairs.filter { pair in
            guard pair.kind.trimmedAnyBoot.lowercased() == target else { return false }
            if enabledOnly && !pair.enabled { return false }
            return !pair.addr.trimmedAnyBoot.isEmpty
        }
    }

    /// 按区块顺序展开所有可用配对
    var flatPairs: [RemotePair] {
        pairBlockOrder().flatMap { pairsOfBlock($0, enabledOnly: true) }
    }

    /// 有没有**启用中**的远程候选：连接优先级里排上、且没被 routeDisabled 停用
    var hasEnabledRemote: Bool {
        routeCandidates().contains { $0.isRemote }
    }

    /// 这条配对的探测超时：条目自己的 → 区块默认 → 全局默认（三层继承）
    func probeTimeoutForPair(_ pair: RemotePair?) -> Int {
        if let pair, pair.probeTimeoutMs > 0 {
            return Self.clampProbeTimeoutMs(pair.probeTimeoutMs)
        }
        let block = pair?.kind.trimmedAnyBoot.lowercased() ?? ""
        if let blockTimeout = blockTimeoutsMs[block], blockTimeout > 0 {
            return Self.clampProbeTimeoutMs(blockTimeout)
        }
        return defaultProbeTimeoutMsValue
    }

    /// 区块内部的探测策略（缺 key = 并发首个）
    func blockProbeModeFor(_ block: String) -> String {
        let value = blockProbeMode[block.trimmedAnyBoot.lowercased()]?.trimmedAnyBoot ?? ""
        if value == Self.probeSerial || value == Self.probeParallelAll { return value }
        return Self.probeParallelFirst
    }

    /// 默认探测超时（毫秒，通道没单独设时用它）
    var defaultProbeTimeoutMsValue: Int { Self.clampProbeTimeoutMs(probeTimeoutMs) }

    /// 某个通道自己的探测超时：单独设过就用它，否则回落默认值
    func probeTimeoutFor(_ kind: String) -> Int {
        if let own = routeTimeoutsMs[kind.trimmedAnyBoot.lowercased()], own > 0 {
            return Self.clampProbeTimeoutMs(own)
        }
        return defaultProbeTimeoutMsValue
    }

    /// 有序六通道（自愈）：清洗非法 id / 去重，缺的按默认序补尾；全空按 connMode 迁移
    func routeOrderSorted() -> [String] {
        var seen: [String] = []
        for raw in routeOrder {
            let id = raw.trimmedAnyBoot
            if Self.isRouteKind(id) && !seen.contains(id) { seen.append(id) }
        }
        if seen.isEmpty && connMode.trimmedAnyBoot == "remote_first" {
            // 迁移：远程通道在前，跳板/直连殿后
            return [Self.routeTunnel, Self.routeTS, Self.routeFunnel,
                    Self.routeTSNet, Self.routeJump, Self.routeLan]
        }
        for id in Self.defaultRouteOrder() where !seen.contains(id) { seen.append(id) }
        return seen
    }

    /// 按优先级产出启用且可用的连接候选。
    ///
    /// lan / jump 的 origin 留空（消费方用自己的局域网端点与探测）；
    /// jump 需先配好跳板机档案。配对区块**逐条展开**：同一区块里三条配对就是三个候选。
    func routeCandidates() -> [RouteCandidate] {
        var out: [RouteCandidate] = []
        let hasLanAddress = addresses.contains { !$0.trimmedAnyBoot.isEmpty }
        var doneBlocks: [String] = []
        for kind in routeOrderSorted() {
            if routeDisabled.contains(kind) { continue }
            if kind == Self.routeLan {
                if hasLanAddress {
                    out.append(RouteCandidate(kind: kind, origin: "", token: "",
                                              timeoutMs: probeTimeoutFor(kind), pairId: ""))
                }
                continue
            }
            if kind == Self.routeJump {
                if hasLanAddress && !jumpServerId.trimmedAnyBoot.isEmpty {
                    out.append(RouteCandidate(kind: kind, origin: "", token: "",
                                              timeoutMs: probeTimeoutFor(kind), pairId: ""))
                }
                continue
            }
            let block = RemotePair.blockOfRoute(kind)
            if block.isEmpty || doneBlocks.contains(block) {
                // ts 与 funnel 同属 ts 区块：整块只展开一次（否则同一批配对报两遍）
                continue
            }
            doneBlocks.append(block)
            for pair in pairsOfBlock(block, enabledOnly: true) {
                // 通道停用按 kind 粒度：ts 区块里「直连」与「Funnel」是两行独立通道
                let pairKind = pair.routeKind
                if !pairKind.isEmpty && routeDisabled.contains(pairKind) { continue }
                out.append(RouteCandidate(kind: pairKind,
                                          origin: Self.normalizeRemoteOrigin(pair.addr),
                                          token: pair.token,
                                          timeoutMs: probeTimeoutForPair(pair),
                                          pairId: pair.id))
            }
        }
        return out
    }

    /// 优先级排序下「远程通道在前」：第一个启用的候选就是远程通道
    var routePriorityRemoteFirst: Bool {
        guard let first = routeCandidates().first else { return false }
        return first.isRemote
    }

    /// 连接状态是否等价（`ServerProfile.java:1446`）
    ///
    /// 只比「连得上连不上」相关的东西：地址池、令牌、通道优先级与停用、三层超时、
    /// 以及每条配对的 `(kind, addr, token)`。名字 / 排序位 / 电源配置变了不算 ——
    /// 心跳靠它决定**要不要通知 UI 重画**，每 10 秒全量重组会把列表刷成幻灯片。
    static func sameConnState(_ a: ServerProfile?, _ b: ServerProfile?) -> Bool {
        guard let a, let b else { return a == nil && b == nil }
        if a.id == b.id && a == b { return true }
        if a.addresses != b.addresses { return false }
        if a.token.trimmedAnyBoot != b.token.trimmedAnyBoot { return false }
        if a.routeOrderSorted() != b.routeOrderSorted() { return false }
        if Set(a.routeDisabled) != Set(b.routeDisabled) { return false }
        if a.probeTimeoutMs != b.probeTimeoutMs { return false }
        if a.blockProbeMode != b.blockProbeMode { return false }
        if a.blockTimeoutsMs != b.blockTimeoutsMs { return false }
        let mine = a.flatPairs
        let theirs = b.flatPairs
        if mine.count != theirs.count { return false }
        for i in 0..<mine.count {
            let x = mine[i]
            let y = theirs[i]
            if x.kind != y.kind || x.addr != y.addr || x.token != y.token { return false }
        }
        return true
    }

    // MARK: 配对写入

    /// 用粘贴内容加入一条配对（整条 `anyboot://pair?v=2&…` 或纯 URL 都行）
    ///
    /// 返回落库的配对；解析失败返回 nil 且不改动现有配对。
    @discardableResult
    mutating func addPairFromPaste(_ raw: String) -> RemotePair? {
        let parsed = Self.parsePair(raw)
        guard parsed.ok else { return nil }
        return upsertPair(parsed)
    }

    /// 把解析结果写进 `pairs`：先按 `(mid, kind, lane)` 找，没有就按 `(addr, kind)`，
    /// 都没有则追加。更新时**保留原 id 与首次配对时间**。
    @discardableResult
    mutating func upsertPair(_ parsed: ParsedPair) -> RemotePair? {
        guard parsed.ok else { return nil }
        let addr = Self.normalizeRemoteOrigin(parsed.tunnel)
        if addr.isEmpty { return nil }
        let kind = parsed.kind.isEmpty ? Self.defaultBlockFor(addr) : parsed.kind
        let mid = parsed.mid.trimmedAnyBoot
        let incomingLane = RemotePair.laneFor(kind: kind, addr: addr)
        var hitIndex: Int?
        for (index, pair) in pairs.enumerated() {
            if pair.kind.trimmedAnyBoot.lowercased() != kind { continue }
            if !mid.isEmpty {
                let pairLane = kind == RemotePair.kindTS ? pair.effectiveLane : ""
                if mid == pair.mid.trimmedAnyBoot && pairLane == incomingLane {
                    hitIndex = index
                    break
                }
            } else if addr == Self.normalizeRemoteOrigin(pair.addr) {
                hitIndex = index
                break
            }
        }
        if hitIndex == nil {
            if pairs.count >= Self.maxPairs { return nil }
            var fresh = RemotePair(kind: kind, addr: addr)
            fresh.id = UUID().uuidString
            fresh.createdAt = Int64(Date().timeIntervalSince1970 * 1000)
            pairs.append(fresh)
            hitIndex = pairs.count - 1
        }
        guard let index = hitIndex else { return nil }
        var hit = pairs[index]
        hit.kind = kind
        hit.addr = addr
        if !parsed.token.trimmedAnyBoot.isEmpty { hit.token = parsed.token.trimmedAnyBoot }
        if !mid.isEmpty { hit.mid = mid }
        if !parsed.os.trimmedAnyBoot.isEmpty { hit.os = Self.normalizeMachineOs(parsed.os) }
        if !parsed.host.trimmedAnyBoot.isEmpty { hit.host = parsed.host.trimmedAnyBoot }
        if !parsed.tunnelId.trimmedAnyBoot.isEmpty { hit.tunnelId = parsed.tunnelId.trimmedAnyBoot }
        hit.lane = RemotePair.laneFor(kind: kind, addr: addr)
        hit.enabled = true
        pairs[index] = hit
        return hit
    }

    // MARK: 访问地址

    /// 带 token 的访问地址（首选地址）
    var url: URL? { url(at: 0) }

    func url(at index: Int) -> URL? { url(at: index, transport: "") }

    /// 第 index 个候选地址的访问地址，并附带传输方式偏好（空 / `auto` 时不加）
    ///
    /// 为什么这个参数要由 App 从 URL 上带过去：网页端的 localStorage 按**来源站点**存，
    /// 一台电脑有多块网卡时会换地址（换地址 = 换来源），偏好就跟着丢了。
    func url(at index: Int, transport: String) -> URL? {
        url(for: hostAt(index), transport: transport)
    }

    /// 用给定的 host:port 造访问地址（token / transport 照常带上）
    func url(for hostPort: String, transport: String) -> URL? {
        let value = hostPort.trimmedAnyBoot
        guard !value.isEmpty else { return nil }
        var components = URLComponents()
        components.scheme = "http"
        let parts = value.split(separator: ":")
        components.host = URLComponents(string: "http://\(value)")?.host ?? parts.first.map(String.init)
        if parts.count > 1, let port = Int(parts[parts.count - 1]) {
            components.port = port
        }
        var items: [URLQueryItem] = []
        let secret = token.trimmedAnyBoot
        if !secret.isEmpty { items.append(URLQueryItem(name: "token", value: secret)) }
        let mode = transport.trimmedAnyBoot
        if !mode.isEmpty && mode != "auto" {
            items.append(URLQueryItem(name: "transport", value: mode))
        }
        if !items.isEmpty { components.queryItems = items }
        return components.url
    }

    // MARK: 展示

    /// 标签上显示的服务端名
    var displayName: String {
        let value = name.trimmedAnyBoot
        return value.isEmpty ? host : value
    }

    /// 标签文本：组名 + 服务端名（未分组时只显示服务端名）
    var label: String {
        let groupName = group.trimmedAnyBoot
        return groupName.isEmpty ? displayName : "\(groupName) · \(displayName)"
    }

    /// 排序键：先按分组（未分组排最后），再按显示名
    var sortKey: String {
        let groupName = group.trimmedAnyBoot
        let prefix = groupName.isEmpty ? "\u{FFFF}" : groupName
        return "\(prefix)\u{0}\(displayName)\u{0}\(host)"
    }

    /// 是不是同一台电脑（可以合并成一条记录）
    func sameMachine(_ other: ServerProfile) -> Bool {
        guard other.id != id else { return false }
        let mine = machineId.trimmedAnyBoot
        let theirs = other.machineId.trimmedAnyBoot
        if !mine.isEmpty && !theirs.isEmpty {
            return mine.caseInsensitiveCompare(theirs) == .orderedSame
        }
        let myGroup = group.trimmedAnyBoot
        return !myGroup.isEmpty
            && myGroup.caseInsensitiveCompare(other.group.trimmedAnyBoot) == .orderedSame
    }

    // MARK: - Codable（老数据兼容：只有单个 host 的 1.1 档案也要能读进来）

    private enum CodingKeys: String, CodingKey {
        case id, group, name, addresses, host, token
        case machineId, machineName, machineOs
        case probeMode, routeProbeMode, probeTimeoutMs
        case routeTimeoutsMs, routeTimeoutsMsSnake = "route_timeouts_ms"
        case jumpServerId, connMode, routeOrder, routeDisabled
        case pairs, blockProbeMode, blockTimeoutsMs, macs, powerRoutes
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = (try? c.decodeIfPresent(String.self, forKey: .id)) ?? UUID().uuidString
        group = (try? c.decodeIfPresent(String.self, forKey: .group)) ?? ""
        name = (try? c.decodeIfPresent(String.self, forKey: .name)) ?? ""
        token = (try? c.decodeIfPresent(String.self, forKey: .token)) ?? ""
        machineId = (try? c.decodeIfPresent(String.self, forKey: .machineId)) ?? ""
        machineName = (try? c.decodeIfPresent(String.self, forKey: .machineName)) ?? ""
        machineOs = Self.normalizeMachineOs((try? c.decodeIfPresent(String.self, forKey: .machineOs)) ?? "")
        probeMode = Self.normalizeProbeMode((try? c.decodeIfPresent(String.self, forKey: .probeMode)) ?? "")
        routeProbeMode = Self.normalizeRouteProbeMode(
            (try? c.decodeIfPresent(String.self, forKey: .routeProbeMode)) ?? "")
        probeTimeoutMs = Self.clampProbeTimeoutMs(
            (try? c.decodeIfPresent(Int.self, forKey: .probeTimeoutMs)) ?? Self.defaultProbeTimeoutMs)
        routeTimeoutsMs = Self.normalizeRouteTimeouts(
            (try? c.decodeIfPresent([String: Int].self, forKey: .routeTimeoutsMs)) ?? nil)
        jumpServerId = (try? c.decodeIfPresent(String.self, forKey: .jumpServerId)) ?? ""
        connMode = (try? c.decodeIfPresent(String.self, forKey: .connMode)) ?? ""
        routeOrder = Self.cleanRouteOrder((try? c.decodeIfPresent([String].self, forKey: .routeOrder)) ?? [])
        routeDisabled = Self.cleanRouteOrder((try? c.decodeIfPresent([String].self, forKey: .routeDisabled)) ?? [])
        pairs = (try? c.decodeIfPresent([RemotePair].self, forKey: .pairs)) ?? []
        blockProbeMode = (try? c.decodeIfPresent([String: String].self, forKey: .blockProbeMode)) ?? [:]
        blockTimeoutsMs = (try? c.decodeIfPresent([String: Int].self, forKey: .blockTimeoutsMs)) ?? [:]
        macs = (try? c.decodeIfPresent([String: [String]].self, forKey: .macs)) ?? [:]
        powerRoutes = (try? c.decodeIfPresent([PowerRoute].self, forKey: .powerRoutes)) ?? []

        // 地址：老数据只有单个 host，读的时候当成唯一的候选地址
        let list = (try? c.decodeIfPresent([String].self, forKey: .addresses)) ?? []
        let legacy = (try? c.decodeIfPresent(String.self, forKey: .host)) ?? ""
        addresses = Self.clean(legacy.isEmpty ? list : list + [legacy])

        // MAC 表清洗（键归一、值去重、键必须在地址里）
        var cleanedMacs: [String: [String]] = [:]
        for (host, list) in macs {
            guard let normalized = Self.normalizeHost(host) else { continue }
            let values = Self.cleanMacs(list)
            if !values.isEmpty { cleanedMacs[normalized] = values }
        }
        macs = cleanedMacs
        pruneMacs()
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(group, forKey: .group)
        try c.encode(name, forKey: .name)
        try c.encode(addresses, forKey: .addresses)
        // host = 首选地址：1.1 及更早的客户端读的是这个键，继续写出去
        try c.encode(host, forKey: .host)
        try c.encode(token, forKey: .token)
        try c.encode(machineId, forKey: .machineId)
        try c.encode(machineName, forKey: .machineName)
        try c.encode(machineOs, forKey: .machineOs)
        try c.encode(probeMode, forKey: .probeMode)
        try c.encode(routeProbeMode, forKey: .routeProbeMode)
        try c.encode(probeTimeoutMs, forKey: .probeTimeoutMs)
        try c.encode(routeTimeoutsMs, forKey: .routeTimeoutsMs)
        try c.encode(jumpServerId, forKey: .jumpServerId)
        try c.encode(connMode, forKey: .connMode)
        try c.encode(routeOrder, forKey: .routeOrder)
        try c.encode(routeDisabled, forKey: .routeDisabled)
        try c.encode(pairs, forKey: .pairs)
        try c.encode(blockProbeMode, forKey: .blockProbeMode)
        try c.encode(blockTimeoutsMs, forKey: .blockTimeoutsMs)
        try c.encode(macs, forKey: .macs)
        try c.encode(powerRoutes, forKey: .powerRoutes)
    }

    /// 通道 id 清洗（去空、去非法、去重、保序）
    static func cleanRouteOrder(_ raw: [String]) -> [String] {
        var out: [String] = []
        for item in raw {
            let id = item.trimmedAnyBoot
            if isRouteKind(id) && !out.contains(id) { out.append(id) }
        }
        return out
    }
}
