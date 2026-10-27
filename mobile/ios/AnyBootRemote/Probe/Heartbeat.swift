import SwiftUI
import UIKit

/// 10 秒心跳 + 按连接策略探测的**唯一入口**。
///
/// 与 Android `HeartbeatManager.java` 逐条同构，职责划分：
/// 本类只管**什么时候探**，怎么探全交给 `ProbeEngine`
/// （六通道候选 / 两层策略 / 三层超时 / 赢家语义）。结果按电脑 id 落账，UI 只读 `states`。
///
/// 规则（顺序即执行顺序）：
/// 1. **空档案跳过**：没地址、也没有启用的远程接入 → 这轮不探；
/// 2. **就地优先**：通道优先级里第一个启用的是本地通道时，先只探局域网 / 跳板机，
///    通了就到此为止（不惊动公网）；本地全挂且配了远程才走第 3 步；
/// 3. **远程降频 60 秒**：公网探测最多每分钟一次 —— 10 秒一轮 × 六通道会把隧道打成筛子；
/// 4. 上线**立即亮**；连续 `offlineAfterFails` 轮全失败才判**离线**（防 WiFi 抖动误报）；
/// 5. **局域网恢复沿**：上轮局域网不通、这轮通了 → 通知宿主（远程兜着时可切回直连）；
/// 6. **只在真变化时发布**：`sameConnState` 式比对，避免每 10 秒全量重组列表；
/// 7. 一轮没跑完不叠加下一轮（`running` 闸）；只在 App 前台跑，回前台立即补探一轮。
@MainActor
final class Heartbeat: ObservableObject {

    enum Reach: Equatable {
        case unknown
        case online
        case offline

        /// 状态点颜色（`MainActivity.kt:443-449` ServerChip 口径）：
        /// 在线 `ok` / 离线 `fgFaint` / 未测出 `fgFaint`（同档，不另立灰阶）
        var color: Color {
            switch self {
            case .online: return ABColor.ok
            case .offline, .unknown: return ABColor.fgFaint
            }
        }
    }

    struct State: Equatable {
        var reach: Reach = .unknown
        /// 最近一次探通的往返毫秒（赢家那条的延迟）
        var rttMs: Int = 0
        /// 赢家通道 id（`lan` / `jump` / `tunnel` / `ts` / `funnel` / `tsnet`）
        var kind: String = ""
        /// 逐行结果（每个候选一条，顺序 = 优先级顺序）
        var hits: [ProbeEngine.Hit] = []
        /// 最近一次探测的错误摘要（全挂时给列表用）
        var error: String = ""
        /// 最近一次探通的时刻
        var lastSeen: Date?
        /// 连续整轮全失败的次数（判离线的计数器）
        var failures: Int = 0
        /// 本机是否正在测（手动「测试连接」会置它）
        var probing: Bool = false
        /// 局域网路径这轮通不通（`nil` = 还没测过）—— 恢复沿用
        var lanOk: Bool?

        /// 有效在线：离线要等连续失败凑满才翻 false
        var isOnline: Bool { reach == .online }
        /// 连续全失败凑满 `offlineAfterFails` 轮才算离线
        var isOffline: Bool { reach == .offline }
    }

    static let shared = Heartbeat()

    static let interval: TimeInterval = 10
    /// 连续这么多轮全失败才判离线
    static let offlineAfterFails = 2
    /// 远程兜底探测的降频间隔：局域网全挂时才探远程，最多每分钟打一次公网
    static let remoteFallbackInterval: TimeInterval = 60

    @Published private(set) var states: [String: State] = [:]

    /// 某台电脑从「非在线」变成「在线」时回调（主线程）——
    /// 电脑页拿它给还停在错误面板上的那台自愈重连。
    var onWentOnline: ((String) -> Void)?
    /// 局域网路径恢复可达（上轮不通、这轮通了；主线程）—— 仍挂在远程隧道上的标签可切回直连
    var onLanRecovered: ((String) -> Void)?
    /// 状态集合有真变化（主线程）—— 不想用 `@Published` 的宿主可以挂这里
    var onChanged: (() -> Void)?

    /// 「经由跳板机」的电脑：把内网地址解析成本地隧道地址（`127.0.0.1:端口`）。
    /// nil / 抛错 = 这一轮判没探到。由 App 注入（探测层不碰 SSH 栈）。
    var tunnelResolver: ((ServerProfile) async throws -> String)?

    /// 探到的机器标识回写（`machineId` / `machineName` / `os`）——
    /// **只在指纹变了时**回调（别每 10 秒写一次盘），由 App 存回档案
    var onMachineInfo: ((_ profileId: String, _ machineId: String,
                         _ machineName: String, _ os: String) -> Void)?

    private var timer: Timer?
    private var running = false
    /// 远程兜底探测的节流账本（profile.id → 上次探远程的时刻）
    private var remoteProbes: [String: Date] = [:]
    /// 上轮上报过的机器指纹（profile.id → "机器标识\0机器名\0系统族"）：变了才回调
    private var machineFlags: [String: String] = [:]

    private init() {
        let center = NotificationCenter.default
        center.addObserver(forName: UIApplication.didBecomeActiveNotification,
                           object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.refreshNow() }
        }
        center.addObserver(forName: UIApplication.willResignActiveNotification,
                           object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.timer?.invalidate()
                self?.timer = nil
            }
        }
    }

    // MARK: - 读取

    func state(_ id: String) -> State { states[id] ?? State() }

    /// 状态点颜色：加载中（WebView 正在拉页面）黄点优先，其余听心跳的
    func dotColor(_ id: String, loading: Bool) -> Color {
        if loading { return ABColor.warn }
        return state(id).reach.color
    }

    /// 某个候选的逐行结果（列表按 `kind + pairId (+ origin)` 回填那一行）
    func hit(for id: String, kind: String, pairId: String, origin: String = "") -> ProbeEngine.Hit? {
        let hits = state(id).hits.filter { $0.kind == kind && $0.pairId == pairId }
        if !origin.isEmpty {
            return hits.first { $0.origin == origin } ?? nil
        }
        return hits.first
    }

    /// 赢家是不是这一行（「使用中」标记）
    func isWinner(_ id: String, kind: String, pairId: String) -> Bool {
        let value = state(id)
        guard value.reach == .online, value.kind == kind else { return false }
        return value.kind == kind && (value.kind == ServerProfile.routeLan
            || value.kind == ServerProfile.routeJump || isWinnerPair(id, pairId: pairId))
    }

    private func isWinnerPair(_ id: String, pairId: String) -> Bool {
        guard !pairId.isEmpty else { return true }
        return states[id]?.hits.first { $0.ok && $0.kind == states[id]?.kind }?.pairId == pairId
    }

    // MARK: - 调度

    /// 立即先探一轮，并重排定时器（回前台 / 手动触发都走这里）
    func refreshNow() {
        probeAll()
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: Self.interval, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.probeAll() }
        }
    }

    /// 手动探一台（返回详细结果给调用方展示），同时落账。
    ///
    /// **手动 = 全通道**（不像 10 秒心跳那样「本地通了就收工」）——
    /// 用户点「测一下」就是想看每一条通道各是什么状态。
    @discardableResult
    func probeNow(_ profile: ServerProfile) async -> ProbeEngine.Result {
        var state = states[profile.id] ?? State()
        state.probing = true
        states[profile.id] = state
        let result = await ProbeEngine.probe(profile, jump: resolver(for: profile), onlyKinds: nil)
        apply(profile, result: result)
        return result
    }

    private func resolver(for profile: ServerProfile) -> ProbeEngine.JumpResolver? {
        guard !profile.jumpServerId.trimmedAnyBoot.isEmpty, let tunnel = tunnelResolver else { return nil }
        return { try await tunnel(profile) }
    }

    private func probeAll() {
        guard !running else { return }   // 上一轮没跑完就不叠加
        running = true
        let profiles = ServerStore.shared.servers
        Task { @MainActor in
            await withTaskGroup(of: Void.self) { group in
                for profile in profiles {
                    // 空档案跳过：没配局域网地址也可能配了远程接入（出门场景）
                    guard !profile.addresses.isEmpty || profile.hasEnabledRemote else { continue }
                    group.addTask { @MainActor in
                        let result = await self.run(profile)
                        self.apply(profile, result: result)
                    }
                }
            }
            self.running = false
        }
    }

    /// 两阶段探测 + 远程降频。
    ///
    /// 就地优先时先只探本地，本地全挂才看公网，且 `remoteFallbackInterval` 内只打一次
    /// —— 与 Android `probeProfile()` 完全一致（含「没到点就按本地不通记」的旧行为）。
    private func run(_ profile: ServerProfile) async -> ProbeEngine.Result {
        let jump = resolver(for: profile)
        if !profile.routePriorityRemoteFirst {
            let local = await ProbeEngine.probe(profile, jump: jump,
                                                onlyKinds: ProbeEngine.localKinds)
            if local.ok || !profile.hasEnabledRemote {
                return local
            }
            if !remoteDue(profile.id) {
                return local
            }
        }
        return await ProbeEngine.probe(profile, jump: jump, onlyKinds: nil)
    }

    /// 远程兜底探测的节流闸：到点了就记账并返回 true
    private func remoteDue(_ id: String) -> Bool {
        let now = Date()
        if let last = remoteProbes[id], now.timeIntervalSince(last) < Self.remoteFallbackInterval {
            return false
        }
        remoteProbes[id] = now
        return true
    }

    // MARK: - 落账

    /// 落一笔探测结果。
    ///
    /// 只在**真变化**时写 `states`：`rttMs` / `kind` / 命中集合 / 错误摘要任何一项变了才发布，
    /// 否则每 10 秒一次全量 `objectWillChange` 会把列表刷成幻灯片
    /// （Android 是「主线程统一 `onHeartbeatChanged()` + 各处自己比 `sameConnState`」，这里合并成一处）。
    private func apply(_ profile: ServerProfile, result: ProbeEngine.Result) {
        var state = states[profile.id] ?? State()
        let before = state

        // 局域网恢复沿：与总在线无关 —— 远程兜着也算「局域网不通」
        let lanOkNow = Self.lanRtt(result) >= 0
        let lanOkBefore = state.lanOk ?? false
        state.lanOk = lanOkNow

        state.probing = false
        state.hits = result.hits
        state.error = result.ok ? "" : result.error

        if result.ok {
            state.failures = 0
            state.rttMs = result.ms
            state.kind = result.kind
            state.lastSeen = Date()
            state.reach = .online
        } else {
            state.failures += 1
            if state.failures >= Self.offlineAfterFails && state.reach != .offline {
                state.reach = .offline
                state.kind = ""
                state.rttMs = 0
            } else if state.reach == .unknown {
                // 首轮就没通：维持 unknown（列表画灰点「未测出」），别直接判离线
                state.reach = .unknown
            }
        }

        let wentOnline = before.reach != .online && state.reach == .online
        let changed = before != state
        if changed { states[profile.id] = state }

        // 机器指纹变了才回写（别每 10 秒写一次盘）
        if result.ok, !result.machineId.isEmpty || !result.machineName.isEmpty || !result.os.isEmpty {
            let fingerprint = result.machineId + "\u{0}" + result.machineName + "\u{0}" + result.os
            if machineFlags[profile.id] != fingerprint {
                machineFlags[profile.id] = fingerprint
                onMachineInfo?(profile.id, result.machineId, result.machineName, result.os)
            }
        }

        if changed { onChanged?() }
        if wentOnline { onWentOnline?(profile.id) }
        if lanOkNow && !lanOkBefore { onLanRecovered?(profile.id) }
    }

    /// 局域网路径的最好成绩（没探 / 全挂 → -1）
    private static func lanRtt(_ result: ProbeEngine.Result) -> Int {
        var best = -1
        for hit in result.hits where hit.ok && hit.kind == ServerProfile.routeLan {
            if best < 0 || hit.ms < best { best = hit.ms }
        }
        return best
    }
}
