import Foundation

/// 电源操作（唤醒 / 重启 / 关机 / 睡眠）—— **统一派发口**。
///
/// 与 Android `ComputerListActivity.powerMessage` 同一套四种模式：
///
/// - `http_only`：只调电脑端 `/api/power`；
/// - `http_first`：先 HTTP，通不了再发广播包；
/// - `hybrid`：两条通道同时发（覆盖面最大）；
/// - `auto`（默认）：优先广播包，失败才走 HTTP 兜底；
/// - `packet_only`：只发广播包，失败就失败。
///
/// 「发什么包」交给 `WakeOnLan`（电源路由 / 标准 WOL / 跳板机广播）。
@MainActor
enum PowerOps {

    /// 电源请求的超时：关机途中响应可能慢，给宽一点
    private static let timeout: TimeInterval = 15

    private static let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = timeout
        config.timeoutIntervalForResource = timeout + 5
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: config)
    }()

    enum PowerError: Error, LocalizedError {
        case message(String)

        var errorDescription: String? {
            switch self {
            case .message(let text): return text
            }
        }
    }

    /// 动作的中文名（提示文案用）
    static func label(_ action: String) -> String {
        ServerProfile.powerActionLabel(action)
    }

    // MARK: - 派发

    /// 发一个电源动作，返回给人看的一句话结果；失败抛错。调用方在 Task 里 await 即可。
    static func perform(_ profile: ServerProfile, action: String) async throws -> String {
        let wanted = ServerProfile.normalizePowerAction(action)
        let routes = profile.routesFor(wanted)
        let mode = ServerProfile.routesMode(routes)

        func http() async throws -> String {
            try await sendAll(profile: profile, action: wanted)
        }

        func packets() async throws -> String {
            if let routed = try await WakeOnLan.sendRoutes(profile: profile, action: wanted) {
                return routed
            }
            if wanted == PowerRoute.actionWake {
                return try await WakeOnLan.wake(profile: profile)
            }
            // 广播包兜底的 HTTP：同样走选点（出门时广播包必然失败，HTTP 得能走隧道）
            return try await http()
        }

        if routes.isEmpty { return try await packets() }

        switch mode {
        case "http_only":
            return L("and.httpOnlyMode") + (try await http())

        case "http_first":
            do {
                return L("and.httpFirstMode") + (try await http())
            } catch {
                do {
                    return L("and.httpFailedBroadcast") + error.localizedDescription
                        + (try await packets())
                } catch let packetError {
                    throw PowerError.message(L("and.httpAndPacketFail").replacingOccurrences(of: "{a}", with: error.localizedDescription).replacingOccurrences(of: "{b}", with: packetError.localizedDescription))
                }
            }

        case "hybrid":
            async let httpResult = capture { try await http() }
            async let packetResult = capture { try await packets() }
            let http = await httpResult
            let packets = await packetResult
            switch (http, packets) {
            case let (.success(httpMessage), .success(packetMessage)):
                return L("and.mixedBothSent", httpMessage, packetMessage)
            case let (.success(httpMessage), .failure(packetError)):
                return L("and.mixedHttpDonePacketFail").replacingOccurrences(of: "{h}", with: httpMessage).replacingOccurrences(of: "{m}", with: packetError.localizedDescription)
            case let (.failure(httpError), .success(packetMessage)):
                return L("and.mixedPacketSentHttpFail").replacingOccurrences(of: "{p}", with: packetMessage).replacingOccurrences(of: "{m}", with: httpError.localizedDescription)
            case let (.failure(httpError), .failure(packetError)):
                throw PowerError.message(L("and.mixedBothFail").replacingOccurrences(of: "{a}", with: packetError.localizedDescription).replacingOccurrences(of: "{b}", with: httpError.localizedDescription))
            }

        default:
            // auto = 优先广播包（失败走 HTTP 兜底）；packet_only = 失败就失败
            do {
                return try await packets()
            } catch let packetError {
                if mode != "auto" { throw packetError }
                do {
                    let outcome = try await http()
                    return L("and.broadcastFailUseHttp").replacingOccurrences(of: "{e}", with: packetError.localizedDescription).replacingOccurrences(of: "{m}", with: outcome)
                } catch let httpError {
                    throw PowerError.message(L("and.solAndHttpFail").replacingOccurrences(of: "{sol}", with: packetError.localizedDescription).replacingOccurrences(of: "{http}", with: httpError.localizedDescription))
                }
            }
        }
    }

    // MARK: - 选点

    /// 把「可能抛错的结果」收成一个值，供 `async let` 并发跑两条通道
    private static func capture(_ work: () async throws -> String) async -> Swift.Result<String, Error> {
        do {
            return .success(try await work())
        } catch {
            return .failure(error)
        }
    }

    /// 电源请求该打哪个地址：**优先用探测赢家**（出门时只可能是远程通道），
    /// 没探过就按优先级第一个候选；远程通道用那条配对自己的令牌。
    static func powerTarget(_ profile: ServerProfile) -> (address: String, token: String) {
        let state = Heartbeat.shared.state(profile.id)
        let candidates = profile.routeCandidates()
        if state.reach == .online, !state.kind.isEmpty,
           let candidate = candidates.first(where: { $0.kind == state.kind }) {
            if candidate.isRemote {
                return (candidate.origin, candidate.token)
            }
            if candidate.kind == ServerProfile.routeLan {
                return ("http://" + profile.host, profile.token)
            }
            if candidate.kind == ServerProfile.routeJump {
                return ("http://" + profile.host, profile.token)
            }
        }
        if let candidate = candidates.first {
            if candidate.isRemote { return (candidate.origin, candidate.token) }
        }
        return ("http://" + profile.host, profile.token)
    }

    // MARK: - HTTP 通道

    /// 多通道并发发送（与 PC / Android 同一口径）：
    /// 通道集合 = profile.powerHttpKinds；每条路由 httpLink 决定直连侧 / 跳板机侧是否参与；
    /// 所有选中通道同时发，任一成功即成功，逐条汇报。
    static func sendAll(profile: ServerProfile, action: String) async throws -> String {
        let wanted = ServerProfile.normalizePowerAction(action)
        let kinds = profile.powerHttpKinds.isEmpty
            ? ServerProfile.defaultPowerHttpKinds() : profile.powerHttpKinds
        let routes = profile.routesFor(wanted)
        var allowDirect = routes.isEmpty
        var allowJump = routes.isEmpty
        for route in routes {
            if route.httpLink != "jump" { allowDirect = true }
            if route.httpLink != "direct" { allowJump = true }
        }

        struct Job { let label: String; let address: String; let token: String; let failure: String }
        var jobs: [Job] = []
        if allowDirect {
            for candidate in profile.routeCandidates()
            where candidate.kind != ServerProfile.routeJump && kinds.contains(candidate.kind) {
                if candidate.kind == ServerProfile.routeLan {
                    jobs.append(Job(label: candidate.kind, address: "http://" + profile.host,
                                    token: profile.token, failure: ""))
                } else {
                    jobs.append(Job(label: candidate.kind, address: candidate.origin,
                                    token: candidate.token, failure: ""))
                }
            }
        }
        if allowJump && kinds.contains(ServerProfile.routeJump),
           !profile.primaryJumpServerId.isEmpty {
            do {
                let tunnel = try await SshSessionManager.shared.tunnelAddress(
                    for: profile, destination: profile.host, jumpId: nil)
                jobs.append(Job(label: "jump", address: "http://" + tunnel,
                                token: profile.token, failure: ""))
            } catch {
                jobs.append(Job(label: "jump", address: "", token: "",
                                failure: error.localizedDescription))
            }
        }
        if jobs.isEmpty {
            throw PowerError.message("没有可用的 HTTP 通道（多选里都未勾选 / 都被禁用 / 没配地址）")
        }

        let results = await withTaskGroup(of: (String, Result<String, Error>).self) { group -> [(String, Result<String, Error>)] in
            for job in jobs {
                group.addTask {
                    if !job.failure.isEmpty {
                        return (job.label, .failure(PowerError.message(job.failure)))
                    }
                    do {
                        let message = try await send(address: job.address, token: job.token, action: wanted)
                        return (job.label, .success(message))
                    } catch {
                        return (job.label, .failure(error))
                    }
                }
            }
            var collected: [(String, Result<String, Error>)] = []
            for await item in group { collected.append(item) }
            return collected
        }

        var ok: [String] = []
        var bad: [String] = []
        for (label, outcome) in results {
            switch outcome {
            case .success(let message): ok.append("\(label)：\(message)")
            case .failure(let error): bad.append("\(label)：\(error.localizedDescription)")
            }
        }
        if ok.isEmpty {
            throw PowerError.message("所有 HTTP 通道都失败：" + bad.joined(separator: "；"))
        }
        var text = "已同时调用 \(ok.count) 个通道：" + ok.joined(separator: "；")
        if !bad.isEmpty { text += "（其余失败：" + bad.joined(separator: "；") + "）" }
        return text
    }

    /// 发一次电源请求。`address` 接受裸 host:port（补 http://）或完整 origin。
    static func send(address: String, token: String, action: String) async throws -> String {
        let target = address.trimmedAnyBoot
        guard !target.isEmpty else {
            throw PowerError.message(L("error.noAddress"))
        }
        var raw = target.hasPrefix("http://") || target.hasPrefix("https://")
            ? target : "http://" + target
        raw += "/api/power?action=" + (action.addingPercentEncoding(
            withAllowedCharacters: .urlQueryAllowed) ?? action)
        let secret = token.trimmedAnyBoot
        if !secret.isEmpty {
            raw += "&token=" + (secret.addingPercentEncoding(
                withAllowedCharacters: .urlQueryAllowed) ?? secret)
        }
        guard let url = URL(string: raw) else {
            throw PowerError.message(L("and.powerAddrInvalid", raw))
        }
        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.timeoutInterval = timeout
        request.cachePolicy = .reloadIgnoringLocalCacheData
        let (data, response) = try await session.data(for: request)
        let code = (response as? HTTPURLResponse)?.statusCode ?? 0
        let body = String(data: data, encoding: .utf8) ?? ""
        if code == 401 {
            throw PowerError.message(L("and.computerTokenReject"))
        }
        if code >= 400 {
            throw PowerError.message(messageOf(body, fallback: L("and.computerCode", code)))
        }
        return messageOf(body, fallback: L("and.submittedAction", label(action)))
    }

    /// 从响应体里挑一句能看的话：优先 message，其次 error，都没有就用兜底文案
    private static func messageOf(_ body: String, fallback: String) -> String {
        if let data = body.data(using: .utf8),
           let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            let message = (object["message"] as? String ?? "").trimmedAnyBoot
            if !message.isEmpty { return message }
            let error = (object["error"] as? String ?? "").trimmedAnyBoot
            if !error.isEmpty { return error }
        }
        return fallback
    }
}
