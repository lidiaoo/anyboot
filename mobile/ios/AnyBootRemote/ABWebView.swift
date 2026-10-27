import SwiftUI
import WebKit

// MARK: - WebView 壳层（对齐 MainActivity.kt:1938-2060）
//
// 审计发现旧实现有三个真 bug，这里全部修掉：
// 1. **UA 没加 `AnyBootRemote/1.2`** —— 前端 `ui/app.js` 靠这个正则判断是不是 App 壳，
//    认不出就会把下拉刷新当成浏览器行为，原生整页重载永远不触发；
// 2. **没拦 `anyboot://reload`** —— 旧代码把非 http(s) 全丢给 `UIApplication.open`，
//    下拉刷新点了等于没反应；
// 3. **缺 isCurrentAddress 守卫** —— 换地址后旧地址的迟到回调会把状态改回去。

/// 一个标签的加载状态
enum ABLoadStatus {
    case unknown, loading, ok, failed
}

final class ABWebViewBox: NSObject {

    let webView: WKWebView

    /// 当前这次加载的目标 URL（用来识别「旧地址的迟到回调」）
    var currentURL: String = ""
    /// 回调过来时问一下：这个 URL 还是当前在试的地址吗？
    var isCurrent: ((String) -> Bool)?
    var onStart: (() -> Void)?
    var onFinish: (() -> Void)?
    var onFail: ((String) -> Void)?
    var onReloadCommand: (() -> Void)?

    override init() {
        let configuration = WKWebViewConfiguration()
        // token 存在 cookie / localStorage，必须用持久化存储
        configuration.websiteDataStore = .default()
        configuration.allowsInlineMediaPlayback = true
        // 局域网明文 HTTP 也要能过（Android 是 MIXED_CONTENT_ALWAYS_ALLOW）
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        // ① UA 标记：追加在系统 UA 后面，前端 / 服务端日志能认出这是 App 客户端。
        //    用这个官方入口，不要去改 navigator.userAgent（那样会和系统 UA 打架）
        configuration.applicationNameForUserAgent = "AnyBootRemote/1.2"

        webView = WKWebView(frame: .zero, configuration: configuration)

        super.init()

        webView.navigationDelegate = self
        webView.allowsBackForwardNavigationGestures = true
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        webView.isOpaque = false
        webView.backgroundColor = .clear
    }

    func load(_ url: URL) {
        currentURL = url.absoluteString
        webView.load(URLRequest(url: url))
    }

    func reload() {
        guard !currentURL.isEmpty else { return }
        webView.load(URLRequest(url: URL(string: currentURL)!))
    }
}

extension ABWebViewBox: WKNavigationDelegate {

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = navigationAction.request.url else {
            decisionHandler(.allow)
            return
        }
        // ② 下拉刷新 = 原生整页重载（先探全部候选再重载，换 IP 也跟得上）
        if url.scheme?.lowercased() == "anyboot" && url.host == "reload" {
            onReloadCommand?()
            decisionHandler(.cancel)
            return
        }
        if url.scheme == "http" || url.scheme == "https" {
            // 站内跳转（含 SPA 路由）留在 WebView
            decisionHandler(.allow)
            return
        }
        // http(s) 之外（mailto:、tel: 等）交给系统，失败静默忽略 —— 不能因为外链崩掉
        UIApplication.shared.open(url, options: [:], completionHandler: nil)
        decisionHandler(.cancel)
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        guard isCurrent?(webView.url?.absoluteString ?? "") ?? true else { return }
        onStart?()
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        // ③ 换地址之后旧地址的回调。丢掉，否则会把状态改回去
        guard isCurrent?(webView.url?.absoluteString ?? "") ?? true else { return }
        onFinish?()
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        report(error)
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!,
                 withError error: Error) {
        report(error)
    }

    private func report(_ error: Error) {
        let failure = error as NSError
        // 主动取消（-999：加载中途又发起了新加载）**不算「连不上」**
        if failure.domain == NSURLErrorDomain && failure.code == NSURLErrorCancelled { return }
        guard isCurrent?(webView.url?.absoluteString ?? "") ?? true else { return }
        onFail?(failure.localizedDescription)
    }
}

/// 把已有的 WKWebView 实例塞进 SwiftUI（实例由 Tab 持有，所以切走再切回不重载）
struct ABWebViewHost: UIViewRepresentable {

    let view: UIView

    func makeUIView(context: Context) -> UIView {
        view.backgroundColor = .clear
        return view
    }

    func updateUIView(_ uiView: UIView, context: Context) {}
}
