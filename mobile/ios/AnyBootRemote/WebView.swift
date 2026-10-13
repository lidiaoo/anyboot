import SwiftUI
import WebKit

/// 一个标签的在线状态（标签上的小圆点）
enum LoadStatus {
    case unknown
    case loading
    case ok
    case failed

    var color: Color {
        switch self {
        case .unknown: return Color(.systemGray)
        case .loading: return Color.orange
        case .ok: return Color.green
        case .failed: return Color.red
        }
    }
}

/// 一个最小可用的 WKWebView 外壳。
///
/// 只做三件事：加载 URL、把加载状态报给 SwiftUI、支持外部触发重载。
/// 页面本身（token、栏目、轮询）全部由服务端托管的前端负责，这里不重复实现。
///
/// 每台电脑各持有一个实例（多台同时在线），所以除了 URL 之外，
/// 还要用 `reloadToken` 区分「同一实例的重新加载」。
/// **一台电脑的多块网卡共用这一个实例** —— 换地址就是换 URL，不新建实例。
struct WebView: UIViewRepresentable {
    let url: URL
    /// 每次自增都会强制重新加载（SwiftUI 里拿来当「重试」的触发器）
    let reloadToken: Int
    var onStatus: (LoadStatus) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()   // token 存在 cookie / localStorage，必须用持久化存储
        configuration.allowsInlineMediaPlayback = true

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = true
        // 服务端是局域网明文 HTTP，WKWebView 允许（配合 Info.plist 的 ATS 例外）
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        context.coordinator.loadedToken = -1
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        context.coordinator.parent = self
        if context.coordinator.loadedToken != reloadToken {
            context.coordinator.loadedToken = reloadToken
            onStatus(.loading)
            webView.load(URLRequest(url: url))
        }
    }

    final class Coordinator: NSObject, WKNavigationDelegate {
        var parent: WebView
        var loadedToken = -1

        init(_ parent: WebView) {
            self.parent = parent
        }

        func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
            parent.onStatus(.loading)
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            parent.onStatus(.ok)
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            report(error)
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
                     withError error: Error) {
            report(error)
        }

        /// 主动取消（-999：例如加载中途又发起了新加载）**不算「连不上」**。
        /// 不然「加载时手动重载」会被当成地址不通，白跳过一个候选地址。
        private func report(_ error: Error) {
            let failure = error as NSError
            if failure.domain == NSURLErrorDomain && failure.code == NSURLErrorCancelled {
                return
            }
            parent.onStatus(.failed)
        }

        /// http(s) 之外（mailto: 等）交给系统
        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            guard let url = navigationAction.request.url else {
                decisionHandler(.allow)
                return
            }
            if url.scheme == "http" || url.scheme == "https" {
                decisionHandler(.allow)
                return
            }
            UIApplication.shared.open(url)
            decisionHandler(.cancel)
        }
    }
}
