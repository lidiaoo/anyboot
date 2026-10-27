import Foundation

// MARK: - WebDAV 客户端（docs/backup-design.md §9.1）
//
// 只用三个动词：PROPFIND（测活/查文件）/ GET（拉）/ PUT（推，If-Match 防覆写）。
// 认证走 HTTP Basic；不解析完整 XML，只认 207/404 状态码。

enum WebDavError: LocalizedError, Equatable {
    case badURL
    case unauthorized          // 401 用户名或密码错误
    case notFound              // 404 远端目录不存在
    case conflict              // 412 云端已被其他设备更新
    case serverError(Int)      // 5xx/其他
    case quota                 // 507 空间不足
    case network(String)       // 连不上 / 超时

    var errorDescription: String? {
        switch self {
        case .badURL: return "服务器地址不合法"
        case .unauthorized: return "401 用户名或密码错误"
        case .notFound: return "404 远端目录不存在"
        case .conflict: return "云端已被其他设备更新，请重试"
        case .serverError(let code): return "服务器无响应（\(code)），请稍后重试"
        case .quota: return "WebDAV 空间不足"
        case .network(let reason): return "连接失败：\(reason)"
        }
    }
}

struct WebDavConfig {
    /// 完整服务器根地址，如 `https://dav.jianguoyun.com/dav/`
    var serverURL: String
    var username: String
    /// KeyVault ref；明文只在请求瞬间解出
    var passwordRef: String
    /// 远端同步目录，如 `AnyBoot`（相对 serverURL；允许 `/` 开头，归一后拼接）
    var directory: String

    /// 归一化目录：去首尾斜杠、折叠连续斜杠；根目录 → 空串
    var cleanDirectory: String {
        directory.split(separator: "/").map(String.init).joined(separator: "/")
    }

    /// 文件资源 URL：serverURL + directory + anyboot-sync.json
    var fileURL: URL? {
        guard var base = URL(string: serverURL.trimmed), base.scheme != nil else { return nil }
        let dir = cleanDirectory
        if !dir.isEmpty {
            for part in dir.split(separator: "/") {
                base.appendPathComponent(String(part))
            }
        }
        base.appendPathComponent("anyboot-sync.json")
        return base
    }

    /// 目录集合 URL（父链）
    var directoryURL: URL? {
        fileURL?.deletingLastPathComponent()
    }

    var isValid: Bool {
        guard let url = URL(string: serverURL.trimmed), url.scheme == "http" || url.scheme == "https",
              let host = url.host, !host.isEmpty else { return false }
        return true
    }
}

private extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}

final class WebDavClient {

    private let config: WebDavConfig
    private let session: URLSession

    init(config: WebDavConfig) {
        self.config = config
        let s = URLSessionConfiguration.ephemeral
        s.timeoutIntervalForRequest = 20
        s.timeoutIntervalForResource = 60
        self.session = URLSession(configuration: s)
    }

    // MARK: - 三个动词

    /// PROPFIND Depth:0 测活。404 = 服务器通、目录不存在（调用方决定要不要建）。
    func testConnection() async -> Result<Void, WebDavError> {
        guard let dirURL = config.directoryURL else { return .failure(.badURL) }
        do {
            var req = authorizedRequest(url: dirURL, method: "PROPFIND")
            req.setValue("0", forHTTPHeaderField: "Depth")
            let (_, response) = try await session.data(for: req)
            return Self.finishPropfind(response)
        } catch {
            return .failure(.network(WebDavClient.shortReason(error)))
        }
    }

    /// MKCOL 逐级创建目录（坚果云等不支持深路径一次创建）
    func createDirectory() async -> Result<Void, WebDavError> {
        guard let base = URL(string: config.serverURL.trimmed), base.scheme != nil else {
            return .failure(.badURL)
        }
        var current = base
        for part in config.cleanDirectory.split(separator: "/") {
            current.appendPathComponent(String(part))
            do {
                var req = authorizedRequest(url: current, method: "MKCOL")
                req.setValue("*", forHTTPHeaderField: "Content-Length")
                let (_, response) = try await session.data(for: req)
                let code = (response as? HTTPURLResponse)?.statusCode ?? 0
                // 405 = 已存在，算成功
                guard (200..<300).contains(code) || code == 405 else {
                    return .failure(code == 401 ? .unauthorized : .serverError(code))
                }
            } catch {
                return .failure(.network(WebDavClient.shortReason(error)))
            }
        }
        return .success(())
    }

    /// 拉取远端同步文件。404 → nil（首次同步，无远端）。
    func download() async -> Result<Data?, WebDavError> {
        guard let url = config.fileURL else { return .failure(.badURL) }
        do {
            let (data, response) = try await session.data(for: authorizedRequest(url: url, method: "GET"))
            guard let http = response as? HTTPURLResponse else { return .failure(.serverError(-1)) }
            switch http.statusCode {
            case 200: return .success(data)
            case 404: return .success(nil)
            case 401: return .failure(.unauthorized)
            default: return .failure(.serverError(http.statusCode))
            }
        } catch {
            return .failure(.network(WebDavClient.shortReason(error)))
        }
    }

    /// 上传同步文件。`ifMatchETag` 非空时携带 If-Match（§9.4 乐观锁）；
    /// 返回新 ETag（服务器可能不给，给 nil 就靠 envelope 内 rev 兜底）。
    func upload(_ data: Data, ifMatchETag: String?) async -> Result<String?, WebDavError> {
        guard let url = config.fileURL else { return .failure(.badURL) }
        do {
            var req = authorizedRequest(url: url, method: "PUT")
            if let ifMatchETag { req.setValue("\"\(ifMatchETag)\"", forHTTPHeaderField: "If-Match") }
            req.httpBody = data
            let (_, response) = try await session.data(for: req)
            guard let http = response as? HTTPURLResponse else { return .failure(.serverError(-1)) }
            switch http.statusCode {
            case 200, 201, 204:
                return .success(http.value(forHTTPHeaderField: "ETag")
                    .map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "\"")) })
            case 401: return .failure(.unauthorized)
            case 412: return .failure(.conflict)
            case 507: return .failure(.quota)
            default: return .failure(.serverError(http.statusCode))
            }
        } catch {
            return .failure(.network(WebDavClient.shortReason(error)))
        }
    }

    // MARK: - 内部

    private func authorizedRequest(url: URL, method: String) -> URLRequest {
        var req = URLRequest(url: url)
        req.httpMethod = method
        if !config.username.isEmpty || !password.isEmpty {
            let raw = "\(config.username):\(password)"
            req.setValue("Basic \(Data(raw.utf8).base64EncodedString())", forHTTPHeaderField: "Authorization")
        }
        return req
    }

    /// 密码明文（KeyVault 读不到 = 空串，由 401 路径兜底提示）
    private var password: String { KeyVault.load(account: config.passwordRef) }

    /// 把 NSError 的.localizedDescription 压成短因（去掉「The request failed:」前缀噪音）
    static func shortReason(_ error: Error) -> String {
        let text = (error as NSError).localizedDescription
        return text.split(separator: ":").last.map { $0.trimmingCharacters(in: .whitespaces) } ?? text
    }
}

// MARK: - PROPFIND 状态码归一

private extension WebDavClient {
    static func finishPropfind(_ response: URLResponse) -> Result<Void, WebDavError> {
        guard let http = response as? HTTPURLResponse else { return .failure(.serverError(-1)) }
        switch http.statusCode {
        case 207, 200, 201, 204: return .success(())
        case 401: return .failure(.unauthorized)
        case 404: return .failure(.notFound)
        case 507: return .failure(.quota)
        default: return .failure(.serverError(http.statusCode))
        }
    }
}
