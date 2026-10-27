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
            let target = powerTarget(profile)
            return try await send(address: target.address, token: target.token, action: wanted)
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
            return "（仅 HTTP 模式）" + (try await http())

        case "http_first":
            do {
                return "（优先 HTTP 模式）" + (try await http())
            } catch {
                do {
                    return "HTTP 没通（\(error.localizedDescription)），已改发广播包："
                        + (try await packets())
                } catch let packetError {
                    throw PowerError.message("HTTP 没通（\(error.localizedDescription)）；"
                        + "广播包也没发出去：\(packetError.localizedDescription)")
                }
            }

        case "hybrid":
            async let httpResult = capture { try await http() }
            async let packetResult = capture { try await packets() }
            let http = await httpResult
            let packets = await packetResult
            switch (http, packets) {
            case let (.success(httpMessage), .success(packetMessage)):
                return "（混合模式）HTTP 与广播包已同时发送 —— HTTP：\(httpMessage)；广播包：\(packetMessage)"
            case let (.success(httpMessage), .failure(packetError)):
                return "（混合模式）HTTP 接口已执行：\(httpMessage)；"
                    + "广播包没发出去（\(packetError.localizedDescription)）"
            case let (.failure(httpError), .success(packetMessage)):
                return "（混合模式）广播包已发出：\(packetMessage)；"
                    + "HTTP 接口失败：\(httpError.localizedDescription)"
            case let (.failure(httpError), .failure(packetError)):
                throw PowerError.message("（混合模式）两边都失败 —— 广播包："
                    + "\(packetError.localizedDescription)；HTTP：\(httpError.localizedDescription)")
            }

        default:
            // auto = 优先广播包（失败走 HTTP 兜底）；packet_only = 失败就失败
            do {
                return try await packets()
            } catch let packetError {
                if mode != "auto" { throw packetError }
                do {
                    let outcome = try await http()
                    return "广播包没发出去（\(packetError.localizedDescription)），已改走 HTTP：\(outcome)"
                } catch let httpError {
                    throw PowerError.message("\(packetError.localizedDescription)；"
                        + "HTTP 兜底也失败：\(httpError.localizedDescription)")
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

    /// 发一次电源请求。`address` 接受裸 host:port（补 http://）或完整 origin。
    static func send(address: String, token: String, action: String) async throws -> String {
        let target = address.trimmedAnyBoot
        guard !target.isEmpty else {
            throw PowerError.message("这台电脑还没有可用地址")
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
            throw PowerError.message("电源地址不合法：\(raw)")
        }
        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.timeoutInterval = timeout
        request.cachePolicy = .reloadIgnoringLocalCacheData
        let (data, response) = try await session.data(for: request)
        let code = (response as? HTTPURLResponse)?.statusCode ?? 0
        let body = String(data: data, encoding: .utf8) ?? ""
        if code == 401 {
            throw PowerError.message("电脑端不认这个 token（401）—— 到「编辑电脑」核对访问令牌")
        }
        if code >= 400 {
            throw PowerError.message(messageOf(body, fallback: "电脑端返回 \(code)"))
        }
        return messageOf(body, fallback: "已提交\(label(action))")
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
