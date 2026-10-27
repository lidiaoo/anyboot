import Foundation

// MARK: - 对电脑发 HTTP（对齐 ComputerListActivity.kt:2538-2576 / Common.kt:385-408）

/// 统一个错误载体：`message` 直接就是给用户看的中文
struct RemoteHttpError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

enum RemoteHttp {

    /// 对齐 Android 的 CONNECT_TIMEOUT_MS / READ_TIMEOUT_MS
    static let connectTimeoutMs = 4000
    static let readTimeoutMs = 8000

    /// `GET {endpoint}{path}?token=…`，返回解析后的 JSON 对象。
    /// - endpoint 接受裸 `host:port`（补 http://）或完整 origin（远程隧道是 https://…）
    static func json(endpoint: String,
                     token: String,
                     path: String,
                     timeoutMs: Int? = nil) async throws -> [String: Any] {
        let base = endpoint.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !base.isEmpty else { throw RemoteHttpError(message: "还没有可用地址") }

        let secret = token.trimmingCharacters(in: .whitespacesAndNewlines)
        let origin = (base.hasPrefix("http://") || base.hasPrefix("https://")) ? base : "http://" + base
        var full = origin + path
        if !secret.isEmpty {
            let joined = path.contains("?") ? "&" : "?"
            full += joined + "token=" + (secret.addingPercentEncoding(
                withAllowedCharacters: .alphanumerics) ?? secret)
        }
        guard let url = URL(string: full) else {
            throw RemoteHttpError(message: "地址不合法：\(full)")
        }

        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.timeoutInterval = Double(timeoutMs ?? readTimeoutMs) / 1000.0
        // Android 明确 instanceFollowRedirects = false：远程隧道重定向了说明地址不对
        request.cachePolicy = .reloadIgnoringLocalCacheData

        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = Double(timeoutMs ?? connectTimeoutMs) / 1000.0
        config.timeoutIntervalForResource = Double(timeoutMs ?? readTimeoutMs) / 1000.0
        config.httpShouldSetCookies = false
        let session = URLSession(configuration: config)
        defer { session.finishTasksAndInvalidate() }

        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            throw RemoteHttpError(message: friendly(error))
        }

        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if status == 401 {
            throw RemoteHttpError(message: "这台电脑不认这个 token（401）—— 到「服务器」里核对访问令牌")
        }

        let body = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        guard let parsed = try? JSONSerialization.jsonObject(with: Data(body.utf8)),
              let json = parsed as? [String: Any] else {
            throw RemoteHttpError(message: "这台电脑返回的不是合法 JSON（\(status)）：\(body.prefix(80))")
        }
        if status >= 400 {
            let message = (json["message"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            throw RemoteHttpError(message: message.isEmpty ? "请求被拒绝" : message)
        }
        return json
    }

    /// 把 URLSession 的错误翻译成 Android 那几句中文
    static func friendly(_ error: Error) -> String {
        let ns = error as NSError
        switch ns.code {
        case NSURLErrorTimedOut:
            return "连接超时"
        case NSURLErrorCannotConnectToHost, NSURLErrorCannotFindHost:
            return "连不上这台电脑"
        case NSURLErrorNotConnectedToInternet:
            return "手机没有网络"
        case NSURLErrorSecureConnectionFailed, NSURLErrorServerCertificateUntrusted:
            return "证书校验失败"
        default:
            let text = ns.localizedDescription.trimmingCharacters(in: .whitespacesAndNewlines)
            return text.isEmpty ? "请求失败" : text
        }
    }

    /// 从 JSON 里取字符串，**JSON null 当空串**（Android `optStr` 那个坑：
    /// `optString` 遇到 null 会返回字符串 "null"，Swift 这里天然正确，但保持同名语义）
    static func str(_ json: [String: Any], _ key: String) -> String {
        guard let value = json[key], !(value is NSNull) else { return "" }
        if let text = value as? String { return text }
        if let number = value as? NSNumber { return number.stringValue }
        return ""
    }

    static func bool(_ json: [String: Any], _ key: String) -> Bool {
        guard let value = json[key], !(value is NSNull) else { return false }
        if let flag = value as? Bool { return flag }
        if let number = value as? NSNumber { return number.boolValue }
        if let text = value as? String { return text == "true" || text == "1" }
        return false
    }

    static func int(_ json: [String: Any], _ key: String) -> Int {
        guard let value = json[key], !(value is NSNull) else { return 0 }
        if let number = value as? NSNumber { return number.intValue }
        if let text = value as? String { return Int(text) ?? 0 }
        return 0
    }

    static func objects(_ json: [String: Any], _ key: String) -> [[String: Any]] {
        guard let list = json[key] as? [Any] else { return [] }
        return list.compactMap { $0 as? [String: Any] }
    }

    static func strings(_ json: [String: Any], _ key: String) -> [String] {
        guard let list = json[key] as? [Any] else { return [] }
        return list.compactMap { item in
            if item is NSNull { return nil }
            if let text = item as? String { return text }
            if let number = item as? NSNumber { return number.stringValue }
            return nil
        }
    }
}
