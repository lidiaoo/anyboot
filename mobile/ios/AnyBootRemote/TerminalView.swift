import SwiftUI
import WebKit

/// xterm 终端视图：WKWebView 跑共享的 terminal/index.html（与 Android 同一份），
/// 字节流经 WKScriptMessageHandler 与 TerminalSessionRef 双向桥接。
struct TerminalView: UIViewRepresentable {

    let session: TerminalSessionRef

    func makeCoordinator() -> Coordinator {
        Coordinator(session)
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        let preferences = WKPreferences()
        preferences.javaScriptEnabled = true
        configuration.preferences = preferences
        configuration.userContentController.add(context.coordinator, name: "iosTerminal")

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.isOpaque = false
        webView.backgroundColor = JumpStore.shared.terminalDark
            ? UIColor(red: 0.12, green: 0.12, blue: 0.12, alpha: 1)
            : .white
        webView.scrollView.isScrollEnabled = false
        webView.navigationDelegate = context.coordinator
        context.coordinator.webView = webView
        webView.loadFileURL(Self.pageURL, allowingReadAccessTo: Self.pageDirectory)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        context.coordinator.session = session
        context.coordinator.ensureAttached()
    }

    static var pageDirectory: URL {
        Bundle.main.resourceURL?.appendingPathComponent("terminal")
            ?? Bundle.main.bundleURL.appendingPathComponent("terminal")
    }

    static var pageURL: URL {
        pageDirectory.appendingPathComponent("index.html")
    }

    final class Coordinator: NSObject, WKScriptMessageHandler, WKNavigationDelegate {

        var session: TerminalSessionRef
        weak var webView: WKWebView?
        private var pageReady = false
        private var pending: [String] = []
        private var observer: NSObjectProtocol?

        init(_ session: TerminalSessionRef) {
            self.session = session
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            let theme = JumpStore.shared.terminalDark ? "dark" : "light"
            webView.evaluateJavaScript("termInit('\(theme)', 14)", completionHandler: nil)
            pageReady = true
            for chunk in pending {
                feed(chunk)
            }
            pending.removeAll()
            ensureAttached()
            // 挂上原生层的下行监听（同一会话可能被多个视图挂过，先清旧的）
            if let observer {
                NotificationCenter.default.removeObserver(observer)
            }
            observer = NotificationCenter.default.addObserver(
                forName: .init("anyboot-term-\(session.server.id)"),
                object: nil, queue: .main) { [weak self] note in
                guard let data = note.object as? Data else { return }
                self?.feedNative(data)
            }
        }

        /// 挂会话：把积攒的回显冲进去
        func ensureAttached() {
            let data = session.attachAndDrain()
            if !data.isEmpty {
                feedNative(data)
            }
        }

        // 下行（原生 → JS）：base64 后塞给 termFeed
        private func feedNative(_ data: Data) {
            feed(data.base64EncodedString())
        }

        private func feed(_ b64: String) {
            guard pageReady, let webView else {
                pending.append(b64)
                return
            }
            webView.evaluateJavaScript("termFeed('\(b64)')", completionHandler: nil)
        }

        // 上行（JS → 原生）：键盘字节 + 窗口尺寸
        func userContentController(_ userContentController: WKUserContentController,
                                   didReceive message: WKScriptMessage) {
            guard let body = message.body as? [String: String] else { return }
            if let b64 = body["data"], let bytes = Data(base64Encoded: b64) {
                session.write(bytes)
            }
            if let cols = body["cols"], let rows = body["rows"],
               let columns = Int(cols), let rowCount = Int(rows) {
                session.resize(columns: columns, rows: rowCount)
            }
        }

        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            decisionHandler(navigationAction.request.url?.isFileURL == true ? .allow : .cancel)
        }
    }
}
