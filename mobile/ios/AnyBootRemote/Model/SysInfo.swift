import SwiftUI

// MARK: - 系统信息（一比一移植 Common.kt:376-563）

/// 系统详情的一行；数组顺序即展示顺序
struct SysInfoRow: Identifiable, Hashable {
    let label: String
    let value: String
    var id: String { label }
}

enum SysInfo {

    /// `GET /api/system`（鉴权，camelCase 响应）
    static func fetchRows(endpoint: String, token: String, timeoutMs: Int = 5000) async throws -> [SysInfoRow] {
        let json = try await RemoteHttp.json(endpoint: endpoint, token: token,
                                             path: "/api/system",
                                             timeoutMs: timeoutMs)
        return parse(json)
    }

    /// `/api/system` 的 JSON → 有序展示行（纯逻辑，缺字段就跳过）
    static func parse(_ json: [String: Any]) -> [SysInfoRow] {
        var rows: [SysInfoRow] = []

        func add(_ label: String, _ value: String) {
            let text = value.trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty { rows.append(SysInfoRow(label: label, value: text)) }
        }

        let osName = RemoteHttp.str(json, "osName")
        let osVersion = RemoteHttp.str(json, "osVersion")
        add("系统", [osName, osVersion]
            .filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
            .joined(separator: " "))
        add("内核", RemoteHttp.str(json, "kernel"))
        add("架构", RemoteHttp.str(json, "arch"))
        add("主机名", RemoteHttp.str(json, "hostname"))

        let cores = RemoteHttp.int(json, "cpuCores")
        let cpu = RemoteHttp.str(json, "cpuModel")
        add("处理器", cores > 0 ? "\(cpu)（\(cores) 核）" : cpu)

        let total = RemoteHttp.int(json, "memTotalMb")
        let free = RemoteHttp.int(json, "memFreeMb")
        if total > 0 {
            let text = free > 0
                ? String(format: "%.1f GB（可用 %.1f GB）", Double(total) / 1024.0, Double(free) / 1024.0)
                : String(format: "%.1f GB", Double(total) / 1024.0)
            add("内存", text)
        }

        let uptime = RemoteHttp.int(json, "uptimeSecs")
        if uptime > 0 { add("运行时长", ABFormat.duration(Int64(uptime) * 1000)) }

        let lanIps = RemoteHttp.strings(json, "lanIps")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        if !lanIps.isEmpty { add("局域网地址", lanIps.joined(separator: "  ")) }

        let port = RemoteHttp.int(json, "httpPort")
        if port > 0 { add("服务端口", "\(port)") }

        add("AnyBoot", RemoteHttp.str(json, "appVersion"))
        return rows
    }

    /// 找一个能读 `/api/system` 的端点：先本地（走跳板机则是它解析出的隧道端口），
    /// 不通再按「连接策略」的通道优先级逐个试远程槽。
    static func loadRows(_ profile: ServerProfile) async throws -> [SysInfoRow] {
        var lastError = ""

        let local = (try? await BootService.localEndpoint(profile)) ?? ""
        if !local.isEmpty {
            do {
                return try await fetchRows(endpoint: local, token: profile.token)
            } catch {
                lastError = (error as? RemoteHttpError)?.message ?? error.localizedDescription
            }
        }

        // 远程兜底：逐条配对试，各用各的令牌
        for candidate in profile.routeCandidates() where candidate.isRemote {
            if candidate.origin.isEmpty { continue }
            do {
                return try await fetchRows(endpoint: candidate.origin, token: candidate.token)
            } catch {
                lastError = (error as? RemoteHttpError)?.message ?? error.localizedDescription
            }
        }

        throw RemoteHttpError(message: lastError.isEmpty ? "连不上这台电脑，读不到系统信息" : lastError)
    }

    /// 系统信息弹层要用的纯文本（「复制」按钮与分享用）
    static func text(title: String, rows: [SysInfoRow]) -> String {
        var out = title
        for row in rows {
            out += "\n\(row.label)：\(row.value)"
        }
        return out
    }
}

// MARK: - 系统信息弹层（多入口共用一个）

@MainActor
final class SystemInfoCenter: ObservableObject {

    static let shared = SystemInfoCenter()

    @Published var isOpen = false
    @Published private(set) var title = ""
    @Published private(set) var rows: [SysInfoRow] = []
    @Published private(set) var loading = false
    @Published private(set) var error = ""

    private var task: Task<Void, Never>?

    /// 入口：卡片 OS 徽标 / ⋮ 菜单 / 页签长按
    func show(_ profile: ServerProfile) {
        title = profile.displayName
        rows = []
        error = ""
        loading = true
        isOpen = true
        task?.cancel()
        task = Task { [weak self] in
            do {
                let loaded = try await SysInfo.loadRows(profile)
                guard !Task.isCancelled else { return }
                await MainActor.run {
                    guard let self else { return }
                    self.rows = loaded
                    self.loading = false
                }
            } catch {
                guard !Task.isCancelled else { return }
                await MainActor.run {
                    guard let self else { return }
                    let message = (error as? RemoteHttpError)?.message ?? error.localizedDescription
                    self.error = message.isEmpty ? "读取系统信息失败" : message
                    self.loading = false
                }
            }
        }
    }

    func dismiss() {
        task?.cancel()
        isOpen = false
    }

    func copyToPasteboard() {
        let body = SysInfo.text(title: title, rows: rows)
        UIPasteboard.general.string = body
        ABToastBus.shared.show("已复制系统信息")
    }
}
