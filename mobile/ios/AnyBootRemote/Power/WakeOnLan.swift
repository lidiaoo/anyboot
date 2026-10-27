import Foundation

/// 网络唤醒（Wake-on-LAN）+ 局域网电源路由（sol）。
///
/// 与 Android `WakeOnLan.java` 同语义，两条通道按有没有配跳板机自动选：
///
/// - **配了 SSH 跳板机**：登录跳板机，由它在内网广播 —— 手机在外网也能唤醒家里；
///   优先 `python3`（现拼魔术包）；跳板机没 python3 直接给可读报错（重试没意义）。
/// - **没配跳板机**：手机自己往局域网 UDP 广播，只有手机和这台电脑同网段时有效。
///
/// 包体永远是**标准 102 字节魔术包**（6×0xFF + MAC×16），一个自定义字节都不带 ——
/// **端口就是指令**：目标机上的 sol 按端口执行对应动作（9 唤醒、别的端口重启/关机/睡眠）。
enum WakeOnLan {

    /// 魔术包端口：9 是标准，7 是老实现（有些网卡只认 7）
    static let legacyPorts = [9, 7]

    /// 重试之间的间隔：WOL 没有回执，只能隔一会儿再打一轮
    static let retryIntervalMs = 200

    enum PowerError: Error, LocalizedError {
        case message(String)

        var errorDescription: String? {
            switch self {
            case .message(let text): return text
            }
        }
    }

    /// 一个待发的 UDP 包（times = 发几个，retry = 没全发出去再试几次）
    struct Delivery {
        let target: String
        let port: UInt16
        let packet: Data
        let times: Int
        let retry: Int

        init(target: String, port: UInt16, packet: Data, times: Int, retry: Int) {
            self.target = target
            self.port = port
            self.packet = packet
            self.times = max(1, times)
            self.retry = max(0, retry)
        }
    }

    // MARK: - 包体

    /// 标准魔术包：6 个 0xFF + 目标 MAC 重复 16 次（共 102 字节）
    static func magicPacket(_ mac: String) throws -> Data {
        let hex = ServerProfile.normalizeMac(mac).replacingOccurrences(of: ":", with: "")
        guard hex.count == 12 else {
            throw PowerError.message("MAC 格式不对：\(mac)")
        }
        var packet = Data(repeating: 0xFF, count: 6)
        var bytes: [UInt8] = []
        var index = hex.startIndex
        while index < hex.endIndex {
            let next = hex.index(index, offsetBy: 2)
            bytes.append(UInt8(hex[index..<next], radix: 16) ?? 0)
            index = next
        }
        for _ in 0..<16 { packet.append(contentsOf: bytes) }
        return packet
    }

    /// 包体转十六进制（走跳板机时 JSON 里只能放文本）
    static func hex(_ data: Data) -> String {
        data.map { String(format: "%02x", $0) }.joined()
    }

    /// 广播目标：**先目标地址所在 /24 的子网广播，再全局广播**。
    ///
    /// 顺序有意义：非唤醒动作只发第一个目标（同一动作发两遍会被 sol 执行两遍），
    /// 而子网广播比 255.255.255.255 更容易真的送到（很多路由器不转发全局广播）。
    static func broadcastTargets(_ address: String) -> [String] {
        var host = address.trimmedAnyBoot
        if let colon = host.lastIndex(of: ":") { host = String(host[..<colon]) }
        var targets: [String] = []
        let octets = host.split(separator: ".")
        if octets.count == 4, octets.allSatisfy({ Int($0) != nil }) {
            targets.append(octets.prefix(3).joined(separator: ".") + ".255")
        }
        targets.append("255.255.255.255")
        return targets
    }

    // MARK: - 局域网电源路由（一个动作可能有多条路由）

    /// 按这台电脑配的「局域网电源路由」发一个动作。
    ///
    /// 返回给用户看的提示；**这台电脑没给该动作配路由时返回 nil** ——
    /// 调用方回落到老路（唤醒走标准 WOL，其余走电脑端 `/api/power`）。
    static func sendRoutes(profile: ServerProfile, action: String) async throws -> String? {
        let wanted = ServerProfile.normalizePowerAction(action)
        let routes = profile.routesFor(wanted)
        if routes.isEmpty { return nil }

        let waking = wanted == PowerRoute.actionWake
        var deliveries: [Delivery] = []
        var described: [String] = []
        var problems: [String] = []

        for route in routes {
            let macs = profile.routeMacs(route)
            if macs.isEmpty {
                problems.append("这条路由没配 MAC，标准魔术包发不出去")
                continue
            }
            let targets = route.target.trimmedAnyBoot.isEmpty
                ? broadcastTargets(profile.host)
                : [route.target.trimmedAnyBoot]
            let port = UInt16(clamping: route.effectivePort)
            if waking {
                // 唤醒：每块网卡各发一份（不知道现在插的是哪根网线），每份 burst 个 + 重试 retry 次
                for mac in macs {
                    let packet = try magicPacket(mac)
                    for target in targets {
                        deliveries.append(Delivery(target: target, port: port, packet: packet,
                                                   times: route.effectiveBurst,
                                                   retry: route.effectiveRetry))
                    }
                }
            } else {
                // 其余动作：只发一个目标，但发包数 / 重试次数照样按路由走
                let packet = try magicPacket(macs[0])
                if let target = targets.first {
                    deliveries.append(Delivery(target: target, port: port, packet: packet,
                                               times: route.effectiveBurst,
                                               retry: route.effectiveRetry))
                }
            }
            let summary = targets.joined(separator: " / ")
            let counts = waking
                ? "（\(macs.count) 个 MAC × \(route.effectiveBurst) 包 / 重试 \(route.effectiveRetry) 次）"
                : "（\(route.effectiveBurst) 包 / 重试 \(route.effectiveRetry) 次）"
            described.append("\(ServerProfile.powerActionLabel(wanted)) → \(summary):\(route.effectivePort)\(counts)")
        }

        if deliveries.isEmpty {
            throw PowerError.message("没有可发的电源包："
                + (problems.isEmpty ? "这些路由都被关掉了" : problems.joined(separator: "；")))
        }

        let jumpId = profile.jumpServerId.trimmedAnyBoot
        var whereFrom = "本机"
        var sent = 0
        if jumpId.isEmpty {
            sent = try sendLocal(deliveries)
        } else {
            guard let jump = JumpStore.shared.jump(jumpId) else {
                throw PowerError.message("跳板机不存在（可能已删除），请到「服务器」页检查")
            }
            guard jump.isSsh else {
                throw PowerError.message("跳板机是 Telnet，发不了局域网电源包 —— 换一台 SSH 服务器")
            }
            let maxRetry = deliveries.map(\.retry).max() ?? 0
            var done = false
            var lastError: Error?
            for attempt in 0...maxRetry {
                do {
                    sent = try await sendViaJump(jump, deliveries)
                    done = true
                    break
                } catch {
                    let message = error.localizedDescription
                    if message.contains("python3") { throw error }   // 没装 python3 不会自愈
                    lastError = error
                    if attempt < maxRetry {
                        try? await Task.sleep(nanoseconds: UInt64(retryIntervalMs) * 1_000_000)
                    }
                }
            }
            if !done {
                throw lastError ?? PowerError.message("跳板机没能发出电源包")
            }
            let label = jump.name.trimmedAnyBoot.isEmpty ? jump.host : jump.name
            whereFrom = "跳板机「\(label)」"
        }

        if sent == 0 {
            throw PowerError.message(whereFrom + "一个包都没发出去：可能在隔离网络、防火墙拦了 UDP 广播，"
                + "或者广播地址填错了（手机要能访问到那个网段）")
        }

        var message = "已从\(whereFrom)按路由发送\(ServerProfile.powerActionLabel(wanted))："
            + described.joined(separator: " · ")
        if !waking {
            message += "。这个动作靠目标机上的 sol 监听该端口执行 —— 没装 sol 不会有反应"
        }
        if !problems.isEmpty {
            message += "（跳过 \(problems.count) 条：\(problems.joined(separator: "；"))）"
        }
        return message
    }

    // MARK: - 标准唤醒（没配电源路由时的老路）

    /// 发唤醒包。配了跳板机就走跳板机广播，否则本机广播。
    static func wake(profile: ServerProfile) async throws -> String {
        let macs = profile.allMacs()
        guard !macs.isEmpty else {
            throw PowerError.message("这台电脑还没配 MAC —— 编辑里在地址后面跟上网卡 MAC"
                + "（如 192.168.0.120:12300 AA:BB:CC:DD:EE:FF）")
        }
        let jumpId = profile.jumpServerId.trimmedAnyBoot
        if !jumpId.isEmpty {
            return try await wakeViaJump(profile: profile, macs: macs, jumpId: jumpId)
        }
        return try wakeLocal(profile: profile, macs: macs)
    }

    private static func wakeLocal(profile: ServerProfile, macs: [String]) throws -> String {
        let targets = broadcastTargets(profile.host)
        var sent = 0
        for mac in macs {
            let packet = try magicPacket(mac)
            for target in targets {
                // 9 端口按默认数量多发几个（丢一两个也能醒）；7 端口给老网卡，发一个兜底
                for port in legacyPorts {
                    let times = port == 9 ? ServerProfile.defaultWakeBurst : 1
                    for _ in 0..<times where sendUDP(target: target, port: UInt16(port), packet: packet) {
                        sent += 1
                    }
                }
            }
        }
        if sent == 0 {
            throw PowerError.message("局域网广播失败：手机和这台电脑不在同一网段时用不了，请配跳板机")
        }
        return "已向局域网广播唤醒包（\(macs.count) 个 MAC），等 10~30 秒看它能不能连上"
    }

    private static func wakeViaJump(profile: ServerProfile, macs: [String],
                                    jumpId: String) async throws -> String {
        guard let jump = JumpStore.shared.jump(jumpId) else {
            throw PowerError.message("跳板机不存在（可能已删除），请到「服务器」页检查")
        }
        guard jump.isSsh else {
            throw PowerError.message("跳板机是 Telnet，发不了唤醒命令 —— 换一台 SSH 服务器")
        }
        let targets = broadcastTargets(profile.host)
        let broadcast = targets.count > 1 ? targets[1] : "255.255.255.255"
        let output = try await SshSessionManager.shared.runCommand(
            jump: jump, command: wolCommand(macs: macs, broadcast: broadcast))
        if output.contains("ANYBOOT-WOL-NOTOOL") {
            throw PowerError.message("跳板机上没有 python3 / wakeonlan / etherwake —— "
                + "装一个（apt install wakeonlan）或换台跳板机")
        }
        guard output.contains("ANYBOOT-WOL-OK") else {
            let detail = truncate(output, 160)
            throw PowerError.message("跳板机没能发出唤醒包" + (detail.isEmpty ? "" : "：\(detail)"))
        }
        let label = jump.name.trimmedAnyBoot.isEmpty ? jump.host : jump.name
        return "已通过跳板机「\(label)」广播唤醒包（\(macs.count) 个 MAC，广播地址 \(broadcast)），"
            + "等 10~30 秒看它能不能连上"
    }

    // MARK: - 发送实现

    /// 本机发：每条按 times 发，没全发出去再试，最多 retry 次
    private static func sendLocal(_ deliveries: [Delivery]) throws -> Int {
        var sent = 0
        for delivery in deliveries {
            for attempt in 0...delivery.retry {
                var round = 0
                for _ in 0..<delivery.times
                where sendUDP(target: delivery.target, port: delivery.port, packet: delivery.packet) {
                    round += 1
                }
                sent += round
                if round == delivery.times || attempt == delivery.retry { break }
                Thread.sleep(forTimeInterval: Double(retryIntervalMs) / 1000)
            }
        }
        return sent
    }

    /// 一个 UDP 包（BSD socket + SO_BROADCAST）。失败返回 false，调用方继续发别的。
    static func sendUDP(target: String, port: UInt16, packet: Data) -> Bool {
        var address = sockaddr_in()
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = port.bigEndian
        guard inet_pton(AF_INET, target, &address.sin_addr) == 1 else { return false }

        let fd = socket(AF_INET, SOCK_DGRAM, 0)
        guard fd >= 0 else { return false }
        defer { close(fd) }

        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_BROADCAST, &on, socklen_t(MemoryLayout<Int32>.size))

        let result = withUnsafePointer(to: &address) { pointer -> Int in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
                packet.withUnsafeBytes { raw -> Int in
                    guard let base = raw.baseAddress else { return -1 }
                    return sendto(fd, base, packet.count, 0, socketAddress,
                                  socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
        }
        return result == packet.count
    }

    /// 经跳板机发：登录跳板机跑 python，按计划表把包发到内网
    private static func sendViaJump(_ jump: JumpServer, _ deliveries: [Delivery]) async throws -> Int {
        var plan: [[String: Any]] = []
        for delivery in deliveries {
            plan.append([
                "targets": [delivery.target],
                "port": Int(delivery.port),
                "packet": hex(delivery.packet),
                "times": delivery.times,
                "retry": delivery.retry,
            ])
        }
        let planJSON = (try? JSONSerialization.data(withJSONObject: plan)) ?? Data()
        let encodedPlan = planJSON.base64EncodedString()
        let encodedScript = Data(pyPower.utf8).base64EncodedString()
        let command = "if command -v python3 >/dev/null 2>&1; then "
            + "python3 -c \"import base64,sys;exec(base64.b64decode('" + encodedScript
            + "').decode())\" " + encodedPlan
            + "; else echo ANYBOOT-POWER-NOTOOL; fi"

        let output = try await SshSessionManager.shared.runCommand(jump: jump, command: command)
        if output.contains("ANYBOOT-POWER-NOTOOL") {
            throw PowerError.message("跳板机上没有 python3 —— 装一个（apt install python3）或换台跳板机")
        }
        guard let range = output.range(of: "ANYBOOT-POWER-OK") else {
            let detail = truncate(output, 160)
            throw PowerError.message("跳板机没能发出电源包" + (detail.isEmpty ? "" : "：\(detail)"))
        }
        let rest = String(output[range.upperBound...]).trimmedAnyBoot
        return Int(rest.split(separator: " ").first.map(String.init) ?? "") ?? 1
    }

    // MARK: - 跳板机侧脚本

    /// 跳板机侧的 python：按计划表逐条发（不依赖装了 wakeonlan / etherwake）
    private static let pyPower = """
    import base64,json,socket,sys,time
    plan=json.loads(base64.b64decode(sys.argv[1]).decode('utf-8'))
    s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET,socket.SO_BROADCAST,1)
    sent=0
    for item in plan:
        packet=bytes.fromhex(item['packet'])
        times=max(1,int(item.get('times',1)))
        retry=int(item.get('retry',0))
        for target in item['targets']:
            for attempt in range(retry+1):
                ok=0
                for _ in range(times):
                    try:
                        s.sendto(packet,(target,int(item['port'])))
                        ok+=1
                    except Exception:
                        pass
                sent+=ok
                if ok==times:
                    break
                time.sleep(0.2)
    print('ANYBOOT-POWER-OK %d' % sent)
    s.close()
    """

    /// 跳板机侧的唤醒脚本（自己拼包，不依赖装了什么 WOL 工具）
    private static let pyWake = """
    import socket,sys
    targets=[sys.argv[1],'255.255.255.255']
    macs=sys.argv[2:]
    s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET,socket.SO_BROADCAST,1)
    for m in macs:
        h=m.replace(':','').replace('-','')
        p=b'\\xff'*6+bytes.fromhex(h)*16
        for t in targets:
            for port in (9,7):
                try: s.sendto(p,(t,port))
                except Exception: pass
    s.close()
    """

    /// 跳板机上跑的 shell 片段：python3 走 base64 传脚本（引号/反斜杠/换行不会被打散）
    private static func wolCommand(macs: [String], broadcast: String) -> String {
        let encoded = Data(pyWake.utf8).base64EncodedString()
        let joined = macs.joined(separator: " ")
        let python = "python3 -c \"import base64;exec(base64.b64decode('" + encoded
            + "').decode('utf-8'))\" " + broadcast + " " + joined
        let fallback = "for m in " + joined + "; do wakeonlan \"$m\"; done"
        let etherwake = "for m in " + joined + "; do etherwake \"$m\"; done"
        return "if command -v python3 >/dev/null 2>&1; then " + python
            + " && echo ANYBOOT-WOL-OK; "
            + "elif command -v wakeonlan >/dev/null 2>&1; then " + fallback
            + " && echo ANYBOOT-WOL-OK; "
            + "elif command -v etherwake >/dev/null 2>&1; then " + etherwake
            + " && echo ANYBOOT-WOL-OK; "
            + "else echo ANYBOOT-WOL-NOTOOL; fi"
    }

    static func truncate(_ value: String, _ limit: Int) -> String {
        let text = value.trimmedAnyBoot.replacingOccurrences(of: "\n", with: " ")
        return text.count <= limit ? text : String(text.prefix(limit)) + "…"
    }
}
