import Foundation

/// 探测电脑端的 `GET /health`（免鉴权探活接口）。
///
/// 与 Android `HealthProbe.java` 同语义：**一个候选地址一次请求**，
/// 通 → 带回 `machineId` / `machineName` / `os` / 往返毫秒；不通 → nil。
///
/// - 局域网 / 跳板机隧道：`http://<host:port>/health`
/// - 远程通道（Cloudflare / TS / Funnel / 内建 tsnet）：`<origin>/health`（origin 自带 scheme）
enum HealthProbe {

    /// `/health` 里认出「这是哪台电脑」的字段（os 是新加的，老服务端没有 → 空串）
    struct Result {
        let machineId: String
        let machineName: String
        let os: String
        let ms: Int
    }

    /// 探测用的会话：不复用 cookie / 缓存，超时自己控（每次探测单独设 timeoutInterval）
    private static let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 30
        config.timeoutIntervalForResource = 35
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        config.httpShouldSetCookies = false
        return URLSession(configuration: config)
    }()

    /// 解析响应体（纯逻辑，便于单测）；不是 JSON 时给 nil
    static func parse(_ body: String) -> Result? {
        guard let data = body.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }
        return Result(machineId: (object["machineId"] as? String ?? "").trimmedAnyBoot,
                      machineName: (object["machineName"] as? String ?? "").trimmedAnyBoot,
                      os: ServerProfile.normalizeMachineOs(object["os"] as? String ?? ""),
                      ms: 0)
    }

    /// 拼探测地址：远程 origin 自带 scheme，局域网/隧道要补 http://
    static func url(target: String, remote: Bool) -> URL? {
        let value = target.trimmedAnyBoot
        guard !value.isEmpty else { return nil }
        let raw = remote ? "\(value)/health" : "http://\(value)/health"
        return URL(string: raw)
    }

    /// 探一个候选（async）。不通 / 非 200 / 不是 AnyBoot → nil。
    static func probe(_ target: String, remote: Bool, timeoutMs: Int) async -> Result? {
        guard let url = url(target: target, remote: remote) else { return nil }
        var request = URLRequest(url: url)
        request.timeoutInterval = TimeInterval(max(50, timeoutMs)) / 1000.0
        request.cachePolicy = .reloadIgnoringLocalCacheData
        let started = Date()
        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                NSLog("[ABPROBE] \(url.absoluteString) → 非200")
                return nil
            }
            let elapsed = Int((Date().timeIntervalSince(started) * 1000).rounded())
            guard let body = String(data: data, encoding: .utf8) else { return nil }
            guard let parsed = parse(body) else {
                // 200 但不是 AnyBoot 的 JSON：算通（页面上有 HTTP 服务），但没有机器标识
                return Result(machineId: "", machineName: "", os: "", ms: elapsed)
            }
            return Result(machineId: parsed.machineId, machineName: parsed.machineName,
                          os: parsed.os, ms: elapsed)
        } catch {
            NSLog("[ABPROBE] \(url.absoluteString) → 失败 \(error.localizedDescription)")
            return nil
        }
    }

    /// 「保存地址后顺手认一次机器」用的一次性探测（超时短，别拖住用户）
    static func fetch(target: String, remote: Bool = false, completion: @escaping (Result?) -> Void) {
        Task {
            let result = await probe(target, remote: remote, timeoutMs: 1500)
            await MainActor.run { completion(result) }
        }
    }
}

// MARK: - 旧入口兼容（ContentView 重写后删除）

extension HealthProbe {
    struct Machine {
        let id: String
        let name: String
    }

    static func fetch(host: String, completion: @escaping (Machine?) -> Void) {
        fetch(target: host, remote: false) { result in
            guard let result else {
                completion(nil)
                return
            }
            completion(Machine(id: result.machineId, name: result.machineName))
        }
    }
}
