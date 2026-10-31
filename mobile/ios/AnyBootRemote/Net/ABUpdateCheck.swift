import Foundation

// MARK: - 检查更新（iOS：只探测 + 引导，不做应用内自装）
//
// 为什么 iOS 不一样：未上架 App Store / TestFlight 之前，`AnyBoot-<ver>_unsigned.ipa`
// 没法自装（要用户自己重签名再走 Xcode / 第三方工具），所以客户端只负责
//   1) 探测最新 tag（`HEAD releases/latest` 读 302 Location —— 零速率限制，
//      与桌面端 update.rs、Android 的 UpdateCore 同一通道）；
//   2) **严格只升不降**地比版本（解析失败按「没有更新」，不猜、不回落）；
//   3) 有新版就把 Releases 页和直链交给 UI，由用户自己去下载。
//
// 口径与 `src-tauri/src/update.rs` / Android `UpdateCore.kt` 保持一致：
// 改任何一边（400/500/`v` 前缀/哈希规则）必须同步其余两处。

enum ABUpdateCheck {

    static let repo = "lidiaoo/anyboot"
    static let latestPage = "https://github.com/\(repo)/releases/latest"
    static let releasesPage = "https://github.com/\(repo)/releases"

    struct Outcome {
        let tag: String        // v0.2.1
        let version: String    // 0.2.1
        let isNewer: Bool
        let notes: String
        let ipaURL: String
    }

    enum Failure: LocalizedError {
        case network(String)
        case noTag

        var errorDescription: String? {
            switch self {
            case .network(let text):
                return "检查更新失败：\(text)。下一步：检查网络后重试，或直接打开 Releases 页。"
            case .noTag:
                return "检查更新失败：响应里没有可用的版本号。下一步：稍后重试，或直接打开 Releases 页。"
            }
        }
    }

    /// 检查是否有更新。[current] 传当前 `CFBundleShortVersionString`。
    static func check(current: String) async throws -> Outcome {
        let tag = try await probeTag()
        let version = normalize(tag)
        var notes = ""
        var ipaURL = "https://github.com/\(repo)/releases/download/\(tag)/AnyBoot_\(version)_unsigned.ipa"

        if let api = await fetchAPI() {
            notes = (api["body"] as? String) ?? ""
            let wanted = "AnyBoot_\(version)_unsigned.ipa"
            if let assets = api["assets"] as? [[String: Any]] {
                for item in assets where (item["name"] as? String) == wanted {
                    if let url = item["browser_download_url"] as? String, isTrusted(url) {
                        ipaURL = url
                    }
                }
            }
        }
        return Outcome(
            tag: tag,
            version: version,
            isNewer: isNewer(version, current),
            notes: notes.trimmingCharacters(in: .whitespacesAndNewlines),
            ipaURL: ipaURL
        )
    }

    // MARK: - 通道

    /// 不跟随重定向的会话：3xx 的 `Location` 才是真实 tag
    private final class NoRedirect: NSObject, URLSessionTaskDelegate {
        func urlSession(_ session: URLSession, task: URLSessionTask,
                        willPerformHTTPRedirection response: HTTPURLResponse,
                        newRequest request: URLRequest,
                        completionHandler: @escaping (URLRequest?) -> Void) {
            completionHandler(nil)
        }
    }

    private static func probeTag() async throws -> String {
        guard let url = URL(string: latestPage) else { throw Failure.noTag }
        var request = URLRequest(url: url)
        request.httpMethod = "HEAD"
        request.timeoutInterval = 10
        request.setValue("AnyBootRemote", forHTTPHeaderField: "User-Agent")

        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 10
        let session = URLSession(configuration: configuration, delegate: NoRedirect(), delegateQueue: nil)
        defer { session.finishTasksAndInvalidate() }

        let (_, response): (Data, URLResponse)
        do {
            (_, response) = try await session.data(for: request)
        } catch {
            throw Failure.network(error.localizedDescription)
        }
        let location = (response as? HTTPURLResponse)?.value(forHTTPHeaderField: "Location")
        guard let location, let tag = tagFromLocation(location) else { throw Failure.noTag }
        return tag
    }

    /// 补充通道：匿名 API 只用来取更新日志与 assets（403 就静默放弃）
    private static func fetchAPI() async -> [String: Any]? {
        guard let url = URL(string: "https://api.github.com/repos/\(repo)/releases/latest") else { return nil }
        var request = URLRequest(url: url)
        request.timeoutInterval = 12
        request.setValue("AnyBootRemote", forHTTPHeaderField: "User-Agent")
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        guard let (data, response) = try? await URLSession.shared.data(for: request),
              (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    // MARK: - 纯逻辑（与桌面/Android 同规则）

    /// `v0.2.1` / ` 0.2.1 ` → `0.2.1`
    static func normalize(_ raw: String) -> String {
        var text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        while text.hasPrefix("v") || text.hasPrefix("V") { text.removeFirst() }
        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// 从 `Location: https://github.com/o/r/releases/tag/v0.2.1` 抠 tag；认不出返回 nil
    static func tagFromLocation(_ location: String?) -> String? {
        guard let text = location?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty,
              let range = text.range(of: "/releases/tag/") else { return nil }
        var tail = String(text[range.upperBound...])
        for separator in ["?", "#", "/"] {
            if let cut = tail.firstIndex(of: Character(separator)) {
                tail = String(tail[..<cut])
            }
        }
        let tag = tail.trimmingCharacters(in: .whitespacesAndNewlines)
        return tag.isEmpty ? nil : tag
    }

    /// `major.minor[.patch]` → 数字数组；解析不出返回 nil
    static func parseVersion(_ raw: String) -> [Int]? {
        let text = normalize(raw)
        guard !text.isEmpty else { return nil }
        let pieces = text.split(separator: ".", omittingEmptySubsequences: false)
        guard !pieces.isEmpty else { return nil }
        var parts: [Int] = []
        for (index, piece) in pieces.enumerated() {
            if index > 2 { break }
            let digits = piece.prefix { $0.isNumber }
            guard !digits.isEmpty, let value = Int(digits) else { return nil }
            parts.append(value)
        }
        return parts.isEmpty ? nil : parts
    }

    /// 严格只升不降：任一侧解析失败 → false（按「没有更新」处理）
    static func isNewer(_ latest: String, _ current: String) -> Bool {
        guard let a = parseVersion(latest), let b = parseVersion(current) else { return false }
        let count = max(a.count, b.count)
        for index in 0..<count {
            let left = index < a.count ? a[index] : 0
            let right = index < b.count ? b[index] : 0
            if left != right { return left > right }
        }
        return false
    }

    /// 只信 GitHub 自家域名（API 的下载地址可能是 objects.githubusercontent.com）
    static func isTrusted(_ url: String) -> Bool {
        guard url.hasPrefix("https://") else { return false }
        let rest = url.dropFirst("https://".count)
        let host = rest.split(separator: "/").first.map(String.init) ?? ""
        let name = host.split(separator: ":").first.map(String.init) ?? host
        return name == "github.com"
            || name == "api.github.com"
            || name.hasSuffix(".githubusercontent.com")
            || name.hasSuffix(".github.com")
    }
}
