import SwiftUI
import WebKit

/// xterm 终端视图：WKWebView 跑共享的 terminal/index.html（与 Android 同一份），
/// 字节流经 WKScriptMessageHandler 与 TerminalSessionRef 双向桥接。
///
/// 键盘输入不走 WebKit（第三方输入法在 xterm.js 的屏外 textarea 上事件序列对不上）：
/// 宿主容器里挂了一个原生 TerminalInputField（隐藏 UITextField），点终端区 / 按 ⌨
/// 都把焦点交给它，任何输入法提交的文本由它转成字节直接发会话。
struct TerminalView: UIViewRepresentable {

    let session: TerminalSessionRef

    func makeCoordinator() -> Coordinator {
        Coordinator(session)
    }

    func makeUIView(context: Context) -> TerminalHostView {
        let configuration = WKWebViewConfiguration()
        let preferences = WKPreferences()
        preferences.javaScriptEnabled = true
        configuration.preferences = preferences
        configuration.userContentController.add(context.coordinator, name: "iosTerminal")
        // JS 里的异常原本只能靠肉眼看空白屏 —— 收上来打日志（仅 DEBUG 生效）
        #if DEBUG
        configuration.userContentController.addUserScript(WKUserScript(
            source: """
            window.onerror = function (msg, url, line, col) {
              try {
                window.webkit.messageHandlers.iosTerminal.postMessage(
                  { jsError: String(msg) + ' @' + line + ':' + col });
              } catch (e) {}
              return false;
            };
            """,
            injectionTime: .atDocumentStart,
            forMainFrameOnly: true))
        #endif

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.isOpaque = false
        // 暗底与安卓同 token：TerminalDarkBg 0x04070F（之前 #1F1F1F 偏灰）
        webView.backgroundColor = JumpStore.shared.terminalDark
            ? UIColor(hex: 0x04070F)
            : .white
        webView.scrollView.isScrollEnabled = false
        webView.navigationDelegate = context.coordinator
        context.coordinator.webView = webView

        let host = TerminalHostView(webView: webView)
        context.coordinator.host = host
        host.inputField.onBytes = { [weak coordinator = context.coordinator] data in
            coordinator?.handleKeyboard(data)
        }

        webView.loadFileURL(Self.pageURL, allowingReadAccessTo: Self.pageDirectory)
        return host
    }

    func updateUIView(_ host: TerminalHostView, context: Context) {
        context.coordinator.host = host
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
        weak var host: TerminalHostView?
        private var pageReady = false
        private var pending: [String] = []
        private var observers: [NSObjectProtocol] = []

        init(_ session: TerminalSessionRef) {
            self.session = session
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            let store = JumpStore.shared
            let theme = store.terminalDark ? "dark" : "light"
            webView.evaluateJavaScript(
                "termInit('\(theme)', \(store.terminalFontSize), '\(Self.jsEscape(store.terminalFontFamily))')",
                completionHandler: nil)
            pageReady = true
            #if DEBUG
            // 诊断用：xterm 到底建起来没有（空白天花板排查）
            webView.evaluateJavaScript(
                "JSON.stringify({term: typeof term, feed: typeof window.termFeed, "
                + "init: typeof window.termInit, ready: !!window.__ready})"
            ) { result, error in
                NSLog("[ABTERM] JS 状态=\(String(describing: result)) err=\(String(describing: error))")
            }
            #endif
            for chunk in pending {
                feed(chunk)
            }
            pending.removeAll()
            // 挂上原生层的下行监听（同一会话可能被多个视图挂过，先清旧的）
            for observer in observers {
                NotificationCenter.default.removeObserver(observer)
            }
            observers.removeAll()
            observers.append(NotificationCenter.default.addObserver(
                forName: .init("anyboot-term-\(session.server.id)"),
                object: nil, queue: .main) { [weak self] note in
                guard let data = note.object as? Data else { return }
                self?.feedNative(data)
            })
            // 先挂监听再取积攒回显：反过来的话，两步之间的输出既没进 backlog 也没人接
            ensureAttached()
            // 终端设置热更：字体族改动即时套到本页（终端 sheet 的菜单里改）
            observers.append(NotificationCenter.default.addObserver(
                forName: .init("anyboot-term-font"), object: nil, queue: .main) { [weak self] note in
                guard let family = note.object as? String else { return }
                self?.webView?.evaluateJavaScript(
                    "termSetFontFamily('\(Self.jsEscape(family))')", completionHandler: nil)
            })
            // 字号热更（termSetFont 内部会重排 fit，行列数变了会自己上报 PTY）
            observers.append(NotificationCenter.default.addObserver(
                forName: .init("anyboot-term-size"), object: nil, queue: .main) { [weak self] note in
                guard let size = note.object as? Int else { return }
                self?.webView?.evaluateJavaScript(
                    "termSetFont(\(size))", completionHandler: nil)
            })
            // 主题热更：xterm 配色 + WebView 底色一起换（键盘外观跟着终端主题走）
            observers.append(NotificationCenter.default.addObserver(
                forName: .init("anyboot-term-theme"), object: nil, queue: .main) { [weak self] note in
                guard let name = note.object as? String else { return }
                self?.webView?.evaluateJavaScript(
                    "termSetTheme('\(Self.jsEscape(name))')", completionHandler: nil)
                self?.webView?.backgroundColor = name == "light"
                    ? .white
                    : UIColor(hex: 0x04070F)
                self?.host?.inputField.keyboardAppearance = name == "light" ? .light : .dark
            })
            // Ctrl 武装位（原生按键栏 ↔ JS）
            observers.append(NotificationCenter.default.addObserver(
                forName: .init("anyboot-term-ctrl-arm"), object: nil, queue: .main) { [weak self] note in
                let on = (note.object as? Bool) ?? false
                self?.webView?.evaluateJavaScript(
                    "termArmCtrl(\(on ? "true" : "false"))", completionHandler: nil)
            })
            // 手动请求聚焦（⌨ 按钮：弹出/收起键盘）—— 焦点交给原生输入中继
            observers.append(NotificationCenter.default.addObserver(
                forName: .init("anyboot-term-focus"), object: nil, queue: .main) { [weak self] _ in
                self?.host?.toggleKeyboard()
            })
            // 自动弹键盘：设置里开启时，页面就绪后把焦点交给原生输入中继。
            // 仅在“刚点过东西”的手势窗口内有效，失败不报错（用户点一下终端照样能打字）。
            if JumpStore.shared.terminalAutoKeyboard {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) { [weak self] in
                    self?.host?.showKeyboard()
                }
            }
            #if DEBUG
            // 探针钩子：-ABTermType=<命令> —— 会话就绪后模拟一次 IME 提交（走
            // insertText 同一入口），用于端到端验收「中继 → 字节流 → 远端 shell」。
            if let cmd = ProcessInfo.processInfo.arguments
                .first(where: { $0.hasPrefix("-ABTermType=") })?
                .dropFirst("-ABTermType=".count), !cmd.isEmpty {
                DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak self] in
                    self?.host?.inputField.insertText(cmd + "\n")
                }
            }
            #endif
        }

        /// 原生输入中继 → 会话：键盘字节不再经过 WebKit。
        /// 字节顺序由输入域的串行队列保证；`TerminalSessionRef.write` 自带锁，哪里调都行。
        func handleKeyboard(_ data: Data) {
            NSLog("[ABTERM] handleKeyboard \(data.count) 字节 → 会话 \(session.server.id)")
            session.write(data)
        }

        /// JS 单引号字符串里的转义：反斜杠与单引号（字体名是用户输入，不能裸拼）
        static func jsEscape(_ value: String) -> String {
            value.replacingOccurrences(of: "\\", with: "\\\\")
                .replacingOccurrences(of: "'", with: "\\'")
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
            // JS 异常（窗口 onerror 投上来的）——只打日志，不影响终端数据流
            if let err = body["jsError"] {
                NSLog("[ABTERM] JS 异常: \(err)")
            }
            // 按键栏 Ctrl 被消费后回报：原生高亮跟着灭
            if let ctrl = body["ctrl"] {
                NotificationCenter.default.post(
                    name: .init("anyboot-term-ctrl-state"), object: ctrl == "1")
            }
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

/// 终端宿主容器：WKWebView（显示）+ TerminalInputField（键盘宿主）。
/// 所有焦点入口都改道原生输入中继，键盘字节不再经过 WebKit。
final class TerminalHostView: UIView {

    let webView: WKWebView
    let inputField = TerminalInputField(frame: CGRect(x: 0, y: 0, width: 1, height: 1))

    init(webView: WKWebView) {
        self.webView = webView
        super.init(frame: .zero)
        addSubview(webView)
        addSubview(inputField)
        webView.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            webView.topAnchor.constraint(equalTo: topAnchor),
            webView.bottomAnchor.constraint(equalTo: bottomAnchor),
            webView.leadingAnchor.constraint(equalTo: leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
        // 点终端区 → 焦点交给原生输入中继。cancelsTouchesInView：别让 WebKit 顺带
        // 聚焦 xterm 自己的隐藏 textarea，否则键盘事件又绕回对第三方输入法不友好的旧链路。
        // 长按不受影响，仍留给 WebKit（选中文本 / 粘贴菜单）。
        // 挂在宿主而不是 webView 上：手势识别器沿视图树向上参与分派，挂在祖先上
        // 一样能收到；同时允许与 WebKit 内部识别器并行，避免被挤掉（探针实测挂在
        // webView 上时合成点击从未触发过本识别器）。
        let tap = UITapGestureRecognizer(target: self, action: #selector(showKeyboard))
        tap.cancelsTouchesInView = true
        tap.delegate = self
        addGestureRecognizer(tap)
    }

    required init?(coder: NSCoder) {
        fatalError("storyboard 不用这个类")
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        // 页面销毁时把键盘收掉，别让它挂在已消失的会话上
        if window == nil && inputField.isFirstResponder {
            inputField.resignFirstResponder()
        }
    }

    /// 点终端区 / 「自动弹键盘」：只弹不收
    @objc func showKeyboard() {
        inputField.becomeFirstResponder()
    }

    /// ⌨ 按钮：弹出/收起（按钮文案本来就是「弹出/收起键盘」，此前只弹不收）
    func toggleKeyboard() {
        if inputField.isFirstResponder {
            inputField.resignFirstResponder()
        } else {
            inputField.becomeFirstResponder()
        }
    }
}

extension TerminalHostView: UIGestureRecognizerDelegate {

    /// 与 WebKit 内部识别器并行识别：不抢内容手势，只确保自己的 tap 一定触发
    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                           shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
        true
    }
}
