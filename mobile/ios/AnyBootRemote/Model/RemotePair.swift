import Foundation

/// 一条远程配对 = 「某套 AnyBoot 配置的某一种接入方式」。
///
/// 与 Android `RemotePair.java`、PC 端 `RemotePair` **逐字同语义**
/// （`docs/remote-pairs.md` §4.1）：
///
/// - **`kind` 是区块 id**（cloudflare / ts / tsnet），不是通道 id；
///   通道 id（tunnel / ts / funnel / tsnet）由 `routeKind` 算出来 ——
///   ts 区块内部再按 `lane` 分成「100.x 直连」与 `*.ts.net` Funnel 两条通道。
/// - 幂等键是 `(mid, kind, lane)`，**不是** `(mid, kind)`：同一台电脑的 ts 直连与
///   系统 Funnel 是并存的两条配对，少比 lane 会让扫 Funnel 码顶掉直连码。
struct RemotePair: Identifiable, Hashable, Codable {

    static let kindCloudflare = "cloudflare"
    static let kindTS = "ts"
    static let kindTSNet = "tsnet"

    /// 三个区块 id（存储层；顺序即默认区块序）
    static let blockIds = [kindCloudflare, kindTS, kindTSNet]

    static let laneDirect = "direct"
    static let laneFunnel = "funnel"

    var id: String = UUID().uuidString
    /// 区块 id：cloudflare / ts / tsnet
    var kind: String = ""
    /// 只有 ts 区块有意义：direct / funnel（写空则按地址形态推导）
    var lane: String = ""
    /// 归一后的 origin（`https://x.trycloudflare.com` / `http://100.64.0.7:12300`）
    var addr: String = ""
    /// 这条自己的配对令牌（签发端按客户端发的，与档案的局域网 token 是两套）
    var token: String = ""
    /// 0 = 继承区块默认 → 全局默认
    var probeTimeoutMs: Int = 0
    /// 停用只是不探测，条目本身留着
    var enabled: Bool = true
    /// 对方系统自报的 machine_id —— 幂等键的一半（另一半是 kind + lane）
    var mid: String = ""
    /// windows / macos / linux（列表徽标 + 来源显示）
    var os: String = ""
    /// 对方主机名（纯显示）
    var host: String = ""
    /// named tunnel 的 id（识别同一隧道被多套系统共用）
    var tunnelId: String = ""
    var note: String = ""
    var createdAt: Int64 = 0

    init() {}

    init(kind: String, addr: String) {
        self.kind = kind.trimmedAnyBoot
        self.addr = addr.trimmedAnyBoot
    }

    // MARK: - 区块 / 通道

    /// 幂等键的 lane 槽（唯一计算口径）：ts 区块按地址形态分 direct / funnel，其余区块恒空。
    ///
    /// `upsertPair` 的命中比较、编辑表单的分拣、保存回写三处都用它 —— lane 是地址的
    /// 派生值，不单独存储判断，避免口径漂移（漂移一次就是一次「互顶」）。
    static func laneFor(kind: String, addr: String) -> String {
        guard kind.trimmedAnyBoot.lowercased() == kindTS else { return "" }
        return ServerProfile.isFunnelOrigin(addr) ? laneFunnel : laneDirect
    }

    /// 区块 id 是否合法
    static func isBlockId(_ raw: String) -> Bool {
        blockIds.contains(raw.trimmedAnyBoot.lowercased())
    }

    /// 通道 id → 区块 id（lan / jump 不属于任何区块，返回空串）
    static func blockOfRoute(_ routeKind: String) -> String {
        switch routeKind.trimmedAnyBoot {
        case ServerProfile.routeTunnel: return kindCloudflare
        case ServerProfile.routeTS, ServerProfile.routeFunnel: return kindTS
        case ServerProfile.routeTSNet: return kindTSNet
        default: return ""
        }
    }

    static func blockLabel(_ block: String) -> String {
        switch block.trimmedAnyBoot {
        case kindCloudflare: return "Cloudflare"
        case kindTS: return "Tailscale"
        case kindTSNet: return "Tailscale（内建）"
        default: return ""
        }
    }

    /// 上报/展示用的通道 id（`routeOrder` / `ProbeOutcome.kind` 那一套）。
    /// 空 = 这条配对不可用（kind 非法）。
    var routeKind: String {
        switch kind.trimmedAnyBoot.lowercased() {
        case Self.kindCloudflare: return ServerProfile.routeTunnel
        case Self.kindTSNet: return ServerProfile.routeTSNet
        case Self.kindTS:
            return effectiveLane == Self.laneFunnel
                ? ServerProfile.routeFunnel : ServerProfile.routeTS
        default: return ""
        }
    }

    /// 这条配对的 lane（写空了的按地址形态补：100.64/10 → direct，`*.ts.net` → funnel）
    var effectiveLane: String {
        let value = lane.trimmedAnyBoot.lowercased()
        if value == Self.laneDirect { return Self.laneDirect }
        if value == Self.laneFunnel { return Self.laneFunnel }
        return ServerProfile.isFunnelOrigin(addr) ? Self.laneFunnel : Self.laneDirect
    }

    /// 幂等键：有 mid 用 `mid|kind|lane|mid`，没有 mid 用 `addr|kind|addr`。
    var key: String {
        let machine = mid.trimmedAnyBoot
        if !machine.isEmpty {
            let laneSlot = kind.trimmedAnyBoot.lowercased() == Self.kindTS ? effectiveLane : ""
            return "mid|\(kind.trimmedAnyBoot)|\(laneSlot)|\(machine)"
        }
        return "addr|\(kind.trimmedAnyBoot)|\(addr.trimmedAnyBoot)"
    }

    /// 拷贝（编辑弹窗里改副本，取消时原档案不受影响）
    func copy() -> RemotePair { self }

    /// 行标题：`<host> · <kind>`；host 空就只写区块名
    var title: String {
        let machine = host.trimmedAnyBoot
        let label = RemotePair.blockLabel(kind)
        if machine.isEmpty { return label.isEmpty ? kind : label }
        return "\(machine) · \(label.isEmpty ? kind : label)"
    }

    /// 来源列：`host · os`；都空时退回 mid 短号（再空就空串）
    var sourceLabel: String {
        var parts: [String] = []
        let machine = host.trimmedAnyBoot
        let system = ServerProfile.normalizeMachineOs(os)
        if !machine.isEmpty { parts.append(machine) }
        if !system.isEmpty { parts.append(system) }
        if parts.isEmpty {
            let value = mid.trimmedAnyBoot
            return value.count > 8 ? String(value.prefix(8)) : value
        }
        return parts.joined(separator: " · ")
    }

    // MARK: - 序列化

    /// 同时接受 camelCase 与 Android/PC 的 snake_case 写法（将来导入别端的 settings 用得上）
    private enum CodingKeys: String, CodingKey {
        case id, kind, lane, addr, token, enabled, mid, os, host, note
        case probeTimeoutMs, probeTimeoutMsSnake = "probe_timeout_ms"
        case tunnelId, tunnelIdSnake = "tunnel_id"
        case createdAt, createdAtSnake = "created_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = (try? c.decodeIfPresent(String.self, forKey: .id)) ?? UUID().uuidString
        kind = ((try? c.decodeIfPresent(String.self, forKey: .kind)) ?? "").trimmedAnyBoot.lowercased()
        lane = ((try? c.decodeIfPresent(String.self, forKey: .lane)) ?? "").trimmedAnyBoot.lowercased()
        addr = ServerProfile.normalizeRemoteOrigin((try? c.decodeIfPresent(String.self, forKey: .addr)) ?? "")
        token = ((try? c.decodeIfPresent(String.self, forKey: .token)) ?? "").trimmedAnyBoot
        enabled = (try? c.decodeIfPresent(Bool.self, forKey: .enabled)) ?? true
        mid = ((try? c.decodeIfPresent(String.self, forKey: .mid)) ?? "").trimmedAnyBoot
        os = ServerProfile.normalizeMachineOs((try? c.decodeIfPresent(String.self, forKey: .os)) ?? "")
        host = ((try? c.decodeIfPresent(String.self, forKey: .host)) ?? "").trimmedAnyBoot
        note = ((try? c.decodeIfPresent(String.self, forKey: .note)) ?? "").trimmedAnyBoot
        tunnelId = (try? c.decodeIfPresent(String.self, forKey: .tunnelId)) ?? ""
        if tunnelId.isEmpty {
            tunnelId = (try? c.decodeIfPresent(String.self, forKey: .tunnelIdSnake)) ?? ""
        }
        probeTimeoutMs = (try? c.decodeIfPresent(Int.self, forKey: .probeTimeoutMs)) ?? 0
        if probeTimeoutMs == 0 {
            probeTimeoutMs = (try? c.decodeIfPresent(Int.self, forKey: .probeTimeoutMsSnake)) ?? 0
        }
        createdAt = (try? c.decodeIfPresent(Int64.self, forKey: .createdAt)) ?? 0
        if createdAt == 0 {
            createdAt = (try? c.decodeIfPresent(Int64.self, forKey: .createdAtSnake)) ?? 0
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(kind, forKey: .kind)
        try c.encode(lane, forKey: .lane)
        try c.encode(ServerProfile.normalizeRemoteOrigin(addr), forKey: .addr)
        try c.encode(token, forKey: .token)
        try c.encode(probeTimeoutMs > 0 ? probeTimeoutMs : 0, forKey: .probeTimeoutMs)
        try c.encode(enabled, forKey: .enabled)
        try c.encode(mid, forKey: .mid)
        try c.encode(ServerProfile.normalizeMachineOs(os), forKey: .os)
        try c.encode(host, forKey: .host)
        try c.encode(tunnelId, forKey: .tunnelId)
        try c.encode(note, forKey: .note)
        try c.encode(createdAt, forKey: .createdAt)
    }
}
