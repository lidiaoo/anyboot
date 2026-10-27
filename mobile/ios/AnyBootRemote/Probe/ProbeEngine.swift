import Foundation

/// **按「连接策略」探测**：两层策略，与 PC 端 `probe_candidates_blocking`、Android
/// `ProbeEngine` 同一套语义。
///
/// - **外层** `ServerProfile.routeProbeMode` —— 连接优先级的探测方式，管**六个通道之间**
///   怎么试（并发首个 / 并发全部 / 串行），对每一个通道都生效；
/// - **内层** `ServerProfile.probeMode`（局域网 / 跳板机）或 `blockProbeMode`
///   （配对区块内）—— 管**同一个通道里的多个候选**之间怎么试。
///
/// 每个通道的探测超时各自取 `probeTimeoutFor` / `probeTimeoutForPair`（三层继承）。
///
/// 「没回来」的两种情形必须分开：**赢家已出、在途被掐 = 未测出**（不是不通）；
/// **等满时限 = 探测超时**。混成一个「不通」，用户会以为那条路真的坏了。
enum ProbeEngine {

    /// 一次探测的结果行（一个候选一条）
    struct Hit: Hashable {
        let kind: String
        let origin: String
        /// 这条结果属于哪条配对（局域网 / 跳板机为空）—— UI 按它回填「哪一行的状态」
        let pairId: String
        let ok: Bool
        let ms: Int
        let error: String
        let machineId: String
        let machineName: String
        let os: String

        static func failure(kind: String, origin: String, pairId: String, error: String) -> Hit {
            Hit(kind: kind, origin: origin, pairId: pairId, ok: false, ms: 0,
                error: error, machineId: "", machineName: "", os: "")
        }
    }

    /// 一台电脑的整体结论：赢家 + 逐行结果
    struct Result {
        var ok: Bool = false
        var kind: String = ""
        var origin: String = ""
        /// 赢家是哪条配对（局域网 / 跳板机为空）
        var pairId: String = ""
        var ms: Int = 0
        var machineId: String = ""
        var machineName: String = ""
        var os: String = ""
        var error: String = ""
        var hits: [Hit] = []

        /// 是否走了远程通道（隧道 / Tailscale）
        var remote: Bool { ProbeEngine.isRemote(kind) }
    }

    /// 跳板机端点解析：返回本机可访问的 host:port；解析不了抛错
    typealias JumpResolver = () async throws -> String

    /// 「本地」通道：局域网与跳板机。心跳先只探这一批 —— 本地通了就不用惊动公网
    static let localKinds = [ServerProfile.routeLan, ServerProfile.routeJump]

    // MARK: - 内部结构

    private struct Item {
        let origin: String
        let timeoutMs: Int
        let pairId: String
    }

    /// 一个通道的候选组。
    ///
    /// `innerMode` 是该组内部的探测策略：局域网 / 跳板机用档案的多网卡策略，
    /// 配对通道用**区块内**策略 —— 同一区块里的多条配对之间怎么试，
    /// 与「一块网卡多个 IP 之间怎么试」是两件事。
    private struct Group {
        let kind: String
        let innerMode: String
        var items: [Item]
    }

    private enum Outcome {
        case group(Int, [Hit])
        case deadline
    }

    // MARK: - 入口

    /// 按档案的连接策略探一台电脑（全部通道）
    static func probe(_ profile: ServerProfile?, jump: JumpResolver?) async -> Result {
        await probe(profile, jump: jump, onlyKinds: nil)
    }

    /// 只探指定通道（心跳分两阶段：先 `localKinds`，本地全挂再传 nil 探全部）
    static func probe(_ profile: ServerProfile?, jump: JumpResolver?,
                      onlyKinds: [String]?) async -> Result {
        var result = Result()
        guard let profile else {
            result.error = "没有档案"
            return result
        }
        let candidates = expand(profile, onlyKinds: onlyKinds)
        if candidates.isEmpty {
            // 全通道路径要报错给用户看；限定通道的阶段性探测「没有候选」是正常情况
            result.error = onlyKinds == nil ? "没有可用连接通道（地址为空，或通道都被停用）" : ""
            return result
        }
        let groups = group(profile, candidates)
        let routeMode = ServerProfile.normalizeProbeMode(profile.routeProbeMode)
        if routeMode == ServerProfile.probeSerial {
            return await probeChannelsSerial(groups, jump: jump)
        }
        return await probeChannelsParallel(groups, jump: jump,
                                           all: routeMode == ServerProfile.probeParallelAll)
    }

    /// `routeCandidates()` 里 lan 只占一条（请求路径用），探测要**每个局域网地址各一条**，
    /// 否则多网卡的机器永远只探第一块网卡。
    private static func expand(_ profile: ServerProfile,
                               onlyKinds: [String]?) -> [RouteCandidate] {
        var out: [RouteCandidate] = []
        for candidate in profile.routeCandidates() {
            let kind = candidate.kind
            if let onlyKinds, !onlyKinds.contains(kind) { continue }
            if kind == ServerProfile.routeLan {
                let timeout = profile.probeTimeoutFor(kind)
                for address in profile.addresses {
                    let trimmed = address.trimmedAnyBoot
                    if !trimmed.isEmpty {
                        out.append(RouteCandidate(kind: kind, origin: trimmed, token: "",
                                                  timeoutMs: timeout, pairId: ""))
                    }
                }
            } else {
                out.append(candidate)
            }
        }
        return out
    }

    /// 按 kind 分组，**保持候选出现顺序**（组顺序 = 用户在优先级表里排的顺序）
    private static func group(_ profile: ServerProfile,
                              _ candidates: [RouteCandidate]) -> [Group] {
        var groups: [Group] = []
        for candidate in candidates {
            var index = groups.firstIndex { $0.kind == candidate.kind }
            if index == nil {
                let block = RemotePair.blockOfRoute(candidate.kind)
                let inner = block.isEmpty
                    ? profile.probeMode
                    : profile.blockProbeModeFor(block)
                groups.append(Group(kind: candidate.kind,
                                    innerMode: ServerProfile.normalizeProbeMode(inner),
                                    items: []))
                index = groups.count - 1
            }
            guard let slot = index else { continue }
            // 同一个 origin 只探一次；不同配对即使地址相同也各占一行（令牌不同 = 另一条路）
            let seen = groups[slot].items.contains {
                $0.origin == candidate.origin && $0.pairId == candidate.pairId
            }
            if !seen {
                groups[slot].items.append(Item(origin: candidate.origin,
                                               timeoutMs: candidate.timeoutMs,
                                               pairId: candidate.pairId))
            }
        }
        return groups
    }

    /// 这组最坏等多久（内层串行时＝逐个累加，否则取最长的一条）
    private static func worstOf(_ group: Group) -> Int {
        if group.innerMode == ServerProfile.probeSerial {
            return group.items.reduce(0) { $0 + $1.timeoutMs }
        }
        return group.items.map(\.timeoutMs).max() ?? 0
    }

    // MARK: - 外层策略

    /// 外层串行：一个通道一个通道地试，某个通道命中就不再往下走
    private static func probeChannelsSerial(_ groups: [Group],
                                            jump: JumpResolver?) async -> Result {
        var hits: [Hit] = []
        for group in groups {
            let groupHits = await probeGroup(group, jump: jump)
            let ok = groupHits.contains { $0.ok }
            hits.append(contentsOf: groupHits)
            if ok { break }
        }
        // 没轮到的通道如实标「没探」，逐行结果才能和优先级表对得上
        fillMissing(&hits, groups, reason: "前面已有通道命中，按串行策略未探测")
        return finish(hits)
    }

    /// 外层并发：一个通道一个任务，组内再按内层策略处理它的多个候选。
    /// `all = true` 等全部通道返回取最快成功者；`all = false` 第一个成功的通道立刻收工。
    private static func probeChannelsParallel(_ groups: [Group],
                                              jump: JumpResolver?,
                                              all: Bool) async -> Result {
        let slowest = groups.map(worstOf).max() ?? 0
        // 硬顶 30 秒：超时下放到条目后最坏耗时是乘出来的，不封顶「探测」按钮能转一分钟
        let budgetMs = min(slowest + 500, ServerProfile.maxProbeTimeoutMs)
        var ordered = [[Hit]?](repeating: nil, count: groups.count)
        var wonEarly = false
        var timedOut = false

        await withTaskGroup(of: Outcome.self) { taskGroup in
            for (index, group) in groups.enumerated() {
                taskGroup.addTask {
                    .group(index, await probeGroup(group, jump: jump))
                }
            }
            taskGroup.addTask {
                try? await Task.sleep(nanoseconds: UInt64(budgetMs) * 1_000_000)
                return .deadline
            }
            for await outcome in taskGroup {
                switch outcome {
                case .deadline:
                    timedOut = true
                    taskGroup.cancelAll()
                    return
                case .group(let index, let groupHits):
                    ordered[index] = groupHits
                    // 并发首个：第一个成功的通道立刻收工，其余在途结果不再等
                    if !all && groupHits.contains(where: { $0.ok }) {
                        wonEarly = true
                        taskGroup.cancelAll()
                        return
                    }
                }
            }
        }

        var hits: [Hit] = []
        for groupHits in ordered {
            if let groupHits { hits.append(contentsOf: groupHits) }
        }
        // 没回来的通道 / 候选如实标出来（带上它自己的超时值）
        for group in groups {
            for item in group.items {
                if contains(hits, kind: group.kind, origin: item.origin, pairId: item.pairId) {
                    continue
                }
                let reason: String
                if wonEarly {
                    reason = "已有通道命中，未测出"
                } else if timedOut {
                    reason = "探测超时（>\(item.timeoutMs)ms）"
                } else {
                    reason = "没有结果（>\(item.timeoutMs)ms）"
                }
                hits.append(.failure(kind: group.kind, origin: item.origin,
                                     pairId: item.pairId, error: reason))
            }
        }
        return finish(hits)
    }

    // MARK: - 内层策略

    /// 探一个通道：组内多个候选按**该组的内部策略**处理，返回按 items 顺序排列的命中。
    /// 未探测的候选也补一行，这样逐行结果永远和优先级表对得上。
    private static func probeGroup(_ group: Group, jump: JumpResolver?) async -> [Hit] {
        if group.items.count == 1, let item = group.items.first {
            return [await probeOne(kind: group.kind, item: item, jump: jump)]
        }
        if group.innerMode == ServerProfile.probeSerial {
            // 内层串行：按用户排的顺序逐个试，第一个通就停
            var hits: [Hit] = []
            for item in group.items {
                let hit = await probeOne(kind: group.kind, item: item, jump: jump)
                hits.append(hit)
                if hit.ok { break }
            }
            for item in group.items where !contains(hits, kind: group.kind,
                                                    origin: item.origin, pairId: item.pairId) {
                hits.append(.failure(kind: group.kind, origin: item.origin,
                                     pairId: item.pairId, error: "同通道前面已命中，未探测"))
            }
            return hits
        }

        // 内层并发：全部同时发出，按完成先后取（首个成功即收工 / 等全部）
        let all = group.innerMode == ServerProfile.probeParallelAll
        var ordered = [Hit?](repeating: nil, count: group.items.count)
        var wonEarly = false
        await withTaskGroup(of: (Int, Hit).self) { taskGroup in
            for (index, item) in group.items.enumerated() {
                taskGroup.addTask {
                    (index, await probeOne(kind: group.kind, item: item, jump: jump))
                }
            }
            for await (index, hit) in taskGroup {
                ordered[index] = hit
                if hit.ok && !all {
                    wonEarly = true
                    taskGroup.cancelAll()
                    return
                }
            }
        }
        var hits: [Hit] = []
        for (index, item) in group.items.enumerated() {
            if let hit = ordered[index] {
                hits.append(hit)
            } else {
                hits.append(.failure(kind: group.kind, origin: item.origin, pairId: item.pairId,
                                     error: wonEarly ? "同通道已命中，未测出"
                                                     : "探测超时（>\(item.timeoutMs)ms）"))
            }
        }
        return hits
    }

    /// 探一个候选：局域网 → `http://host:port/health`；远程 → `<origin>/health`
    private static func probeOne(kind: String, item: Item, jump: JumpResolver?) async -> Hit {
        var remote = isRemote(kind)
        var target = item.origin
        if kind == ServerProfile.routeJump {
            guard let jump else {
                return .failure(kind: kind, origin: "", pairId: item.pairId, error: "没配跳板机解析器")
            }
            do {
                target = try await jump()
            } catch {
                return .failure(kind: kind, origin: "", pairId: item.pairId,
                                error: "跳板机不可用：\(error.localizedDescription)")
            }
            if target.trimmedAnyBoot.isEmpty {
                return .failure(kind: kind, origin: "", pairId: item.pairId,
                                error: "跳板机隧道没建起来")
            }
            remote = false
        }
        guard let probe = await HealthProbe.probe(target, remote: remote,
                                                  timeoutMs: item.timeoutMs) else {
            return .failure(kind: kind, origin: target, pairId: item.pairId,
                            error: "连不上（超时或对方没起 HTTP 服务）")
        }
        return Hit(kind: kind, origin: target, pairId: item.pairId, ok: true, ms: probe.ms,
                   error: "", machineId: probe.machineId, machineName: probe.machineName,
                   os: probe.os)
    }

    // MARK: - 收束

    static func isRemote(_ kind: String) -> Bool {
        kind == ServerProfile.routeTunnel || kind == ServerProfile.routeTS
            || kind == ServerProfile.routeFunnel || kind == ServerProfile.routeTSNet
    }

    private static func contains(_ hits: [Hit], kind: String, origin: String,
                                 pairId: String) -> Bool {
        hits.contains { $0.kind == kind && $0.origin == origin && $0.pairId == pairId }
    }

    /// 把没探到的候选补成「跳过」行，逐行结果才和优先级表一致
    private static func fillMissing(_ hits: inout [Hit], _ groups: [Group], reason: String) {
        for group in groups {
            for item in group.items
            where !contains(hits, kind: group.kind, origin: item.origin, pairId: item.pairId) {
                hits.append(.failure(kind: group.kind, origin: item.origin,
                                     pairId: item.pairId, error: reason))
            }
        }
    }

    /// 收束：赢家 = 延迟最低的成功者；全挂时带上第一条失败原因
    private static func finish(_ hits: [Hit]) -> Result {
        var result = Result()
        result.hits = hits
        let winner = hits.filter { $0.ok }.min { $0.ms < $1.ms }
        guard let winner else {
            if let firstError = hits.first(where: { !$0.error.isEmpty })?.error {
                result.error = "所有通道都不通：\(firstError)"
            } else {
                result.error = "所有通道都不通"
            }
            return result
        }
        result.ok = true
        result.kind = winner.kind
        result.origin = winner.origin
        result.pairId = winner.pairId
        result.ms = winner.ms
        result.machineId = winner.machineId
        result.machineName = winner.machineName
        result.os = winner.os
        return result
    }
}
