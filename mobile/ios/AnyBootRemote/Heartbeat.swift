import Foundation
import SwiftUI
import UIKit

/// 10 秒心跳：轮询全部服务端条目的 `/health`，维护「在线 / 离线」状态，
/// 给顶部标签的状态点用（在线=绿、离线=灰；加载中的黄点仍由 WebView 报）。
///
/// 规则（与 docs/client-todo.md F1 一致）：
/// - 一台电脑有多个候选地址时按 `probeMode` 探：
///   **并发（默认）**同探所有地址取延迟最低；**串行**按顺序探，第一个通的即止；
/// - 上线**立即亮**；连续 `offlineAfterFails` 轮全失败才判**离线**（防 WiFi 抖动误报）；
/// - 只在 App 前台跑：回前台立即先探一轮，退后台停表；
/// - 一轮没跑完不叠加下一轮（`running` 闸）。
final class Heartbeat: ObservableObject {

    enum Reach {
        case unknown
        case online
        case offline

        var color: Color {
            switch self {
            case .online: return Color.green
            case .offline: return Color(.systemGray)
            case .unknown: return Color(.systemGray)
            }
        }
    }

    struct State {
        var reach: Reach = .unknown
        /// 最近一次探通的往返毫秒（并发模式取所有地址里最快的）
        var rttMs: Double = 0
        /// 最近一次探通的时间
        var lastSeen: Date?
        /// 连续整轮全失败的次数（判离线的计数器）
        var failures = 0
    }

    static let shared = Heartbeat()

    static let interval: TimeInterval = 10
    /// 单地址探测超时：比保存时的一次性探测宽容，弱网别误报离线
    static let timeout: TimeInterval = 3
    /// 连续这么多轮全失败才判离线
    static let offlineAfterFails = 2

    @Published private(set) var states: [String: State] = [:]

    /// 某台电脑从「非在线」变成「在线」时回调（主线程）——
    /// ContentView 拿它给还停在错误面板上的那台自愈重连。
    var onWentOnline: ((String) -> Void)?

    /// 「经由跳板机」的电脑：把内网地址解析成本地隧道地址
    /// （127.0.0.1:端口）。心跳在 Task 里调用；由 ContentView 注入。
    /// nil / 抛错 = 判这一轮没探到。
    var tunnelResolver: ((ServerProfile) async throws -> String)?

    private var timer: Timer?
    private var running = false

    private init() {
        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(appDidBecomeActive),
                           name: UIApplication.didBecomeActiveNotification, object: nil)
        center.addObserver(self, selector: #selector(appWillResignActive),
                           name: UIApplication.willResignActiveNotification, object: nil)
    }

    // MARK: - 前台 / 后台

    @objc private func appDidBecomeActive() {
        refreshNow()
    }

    @objc private func appWillResignActive() {
        timer?.invalidate()
        timer = nil
    }

    /// 立即先探一轮，并重排 10 秒定时器（回前台 / 手动触发都走这里）
    func refreshNow() {
        probeAll()
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.timer?.invalidate()
            self.timer = Timer.scheduledTimer(withTimeInterval: Self.interval, repeats: true) { [weak self] _ in
                self?.probeAll()
            }
        }
    }

    // MARK: - 读取

    func state(_ id: String) -> State {
        states[id] ?? State()
    }

    /// 状态点颜色：加载中（WebView 正在拉页面）黄点优先，其余听心跳的。
    func dotColor(_ id: String, loading: Bool) -> Color {
        if loading { return Color.orange }
        return state(id).reach.color
    }

    // MARK: - 探测

    private func probeAll() {
        guard !running else { return }   // 上一轮（串行多地址时可能超 10 秒）没跑完就不叠加
        running = true
        let group = DispatchGroup()
        for profile in ServerStore.shared.servers where !profile.addresses.isEmpty {
            group.enter()
            probeProfile(profile) { group.leave() }
        }
        group.notify(queue: .main) { [weak self] in
            self?.running = false
        }
    }

    /// 探一台电脑（它的全部候选地址按模式走），完成后回调 `done`（主线程）。
    private func probeProfile(_ profile: ServerProfile, done: @escaping () -> Void) {
        // 经由跳板机的电脑：全部探测走 SSH 隧道（解析出的 127.0.0.1:端口）
        if !profile.jumpServerId.trimmedAnyBoot.isEmpty {
            let resolver = tunnelResolver
            Task {
                var rtt: Double? = nil
                if let resolver {
                    do {
                        let local = try await resolver(profile)
                        if !local.isEmpty {
                            rtt = await withCheckedContinuation { continuation in
                                HealthProbe.ping(host: local, timeout: Self.timeout) { value in
                                    continuation.resume(returning: value)
                                }
                            }
                        }
                    } catch {
                        rtt = nil
                    }
                }
                await MainActor.run {
                    self.apply(profile.id, online: rtt != nil, rttMs: rtt ?? 0)
                    done()
                }
            }
            return
        }
        let addresses = profile.addresses
        if ServerProfile.cleanProbeMode(profile.probeMode) == ServerProfile.probeSerial {
            // 串行：按顺序逐个探，第一个通的即止（保留用户排的地址优先级）
            var index = 0
            func next() {
                guard index < addresses.count else {
                    apply(profile.id, online: false, rttMs: 0)
                    done()
                    return
                }
                let address = addresses[index]
                index += 1
                HealthProbe.ping(host: address, timeout: Self.timeout) { rtt in
                    if let rtt {
                        self.apply(profile.id, online: true, rttMs: rtt)
                        done()
                    } else {
                        next()
                    }
                }
            }
            next()
        } else {
            // 并发：所有地址同时探，取 RTT 最低的当「这台电脑现在的延迟」
            let inner = DispatchGroup()
            let lock = NSLock()
            var bestRtt = Double.greatestFiniteMagnitude
            var found = false
            for address in addresses {
                inner.enter()
                HealthProbe.ping(host: address, timeout: Self.timeout) { rtt in
                    if let rtt {
                        lock.lock()
                        found = true
                        bestRtt = min(bestRtt, rtt)
                        lock.unlock()
                    }
                    inner.leave()
                }
            }
            inner.notify(queue: .main) { [weak self] in
                self?.apply(profile.id, online: found,
                            rttMs: found ? bestRtt : 0)
                done()
            }
        }
    }

    /// 落一笔探测结果：上线立即亮，连续全失败才判离线
    private func apply(_ id: String, online: Bool, rttMs: Double) {
        var updated = states[id] ?? State()
        if online {
            updated.failures = 0
            updated.rttMs = rttMs
            updated.lastSeen = Date()
            let wentOnline = updated.reach != .online
            updated.reach = .online
            states[id] = updated
            if wentOnline {
                onWentOnline?(id)
            }
        } else {
            updated.failures += 1
            if updated.failures >= Self.offlineAfterFails && updated.reach != .offline {
                updated.reach = .offline
            }
            states[id] = updated
        }
    }
}
