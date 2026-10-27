import UIKit

/// 终端原生输入中继（第三方输入法根治）。
///
/// 背景：终端是 WKWebView 跑 xterm.js，xterm 靠一个屏外隐藏 textarea「偷」键盘事件。
/// 系统键盘老老实实走 WebKit 的 keydown/composition 事件链，能正常打字；
/// 第三方输入法（搜狗/百度/Gboard）依赖真实文本框的几何信息与 marked text 流程，
/// 在屏外小框里事件序列对不上 —— 键盘弹出但按键没反应、候选词丢失。
///
/// 修法（Termius 等专业终端同款）：键盘一律喂给这个原生 UITextField。
/// 任何键盘（含第三方）都走 iOS 原生 UITextInput 协议，由这里把提交的文本
/// 转成字节流直接发给会话；键盘事件不再经过 WebKit。
/// 字段本身永远为空、完全透明（alpha 0.01，1×1pt），只是键盘的「宿主」。
///
/// 注意：拼音组词期间 UIKit 走 setMarkedText（候选条由此读取组词内容），
/// 这里不拦；只在「提交」的三个入口转发 —— insertText / replace / deleteBackward。
final class TerminalInputField: UITextField {

    /// 键盘提交 → 字节流（回车映射为 CR，其余原样 UTF-8）
    var onBytes: ((Data) -> Void)?

    /// 保序：键盘字节必须按敲击顺序写进会话，回显才不会乱
    private let wire = DispatchQueue(label: "anyboot.term.input")

    override init(frame: CGRect) {
        super.init(frame: frame)
        delegate = self
        backgroundColor = .clear
        textColor = .clear
        tintColor = .clear
        font = .systemFont(ofSize: 1)
        alpha = 0.01
        // 终端要原始字符：关掉一切自作聪明的替换
        autocorrectionType = .no
        autocapitalizationType = .none
        spellCheckingType = .no
        smartQuotesType = .no
        smartDashesType = .no
        smartInsertDeleteType = .no
        enablesReturnKeyAutomatically = false
        keyboardType = .default        // 保留 default，才能切到中文等 IME
        keyboardAppearance = .dark
        clearsOnBeginEditing = false
        clearButtonMode = .never
        isAccessibilityElement = true
        accessibilityLabel = "终端输入"
    }

    required init?(coder: NSCoder) {
        fatalError("storyboard 不用这个类")
    }

    // MARK: - 输入转发

    /// 系统键盘 / 第三方输入法提交文本的常规入口
    override func insertText(_ text: String) {
        NSLog("[ABIN] insertText len=\(text.count) '\(text)'")
        forward(text)
    }

    /// 部分输入法不走 insertText，直接按范围替换（提交组词结果的另一条路）
    override func replace(_ range: UITextRange, withText text: String) {
        NSLog("[ABIN] replace len=\(text.count) '\(text)'")
        forward(text)
    }

    /// 组词过程（拼音字母上屏前）：只观察，不转发 —— 终端是字节流，组词内容
    /// 得等用户选完候选提交后再发，否则远端会收到半截拼音
    override func setMarkedText(_ markedText: String?, selectedRange: NSRange) {
        NSLog("[ABIN] setMarkedText '\(markedText ?? "nil")'")
        super.setMarkedText(markedText, selectedRange: selectedRange)
    }

    /// 退格：终端语义是 DEL（0x7f）。但组词进行中例外 —— 此时退格是在改拼音，
    /// 交给 super 删组词里的字母，绝不能把 DEL 发给远端。
    override func deleteBackward() {
        if markedTextRange != nil {
            super.deleteBackward()
            return
        }
        NSLog("[ABIN] deleteBackward → DEL")
        forward("\u{7f}")
    }

    /// 永远「有字」：空字段也允许连续退格（别让系统拦下来不调 deleteBackward）
    override var hasText: Bool { true }

    /// 有输入法绕开 insertText/replace 直接改 text —— 观察用（不改行为）
    override var text: String? {
        get { super.text }
        set {
            if let newValue, !newValue.isEmpty {
                NSLog("[ABIN] text 被直接设置 '\(newValue)'")
            }
            super.text = newValue
        }
    }

    /// 键盘逐键输入的主通路（iOS 17+ 实测：软键盘/硬件键的文本插入走这里，
    /// 不经过可重写的 insertText）。原样转发 replacement，字段保持为空。
    /// 返回 false 会让字段内部状态与输入法失同步，所以放行 + 异步清空。
    func textField(_ textField: UITextField,
                   shouldChangeCharactersIn range: NSRange,
                   replacementString string: String) -> Bool {
        NSLog("[ABIN] shouldChange '\(string)' range=\(range)")
        if !string.isEmpty {
            forward(string)
        }
        // 组词期间绝不清空（会打断候选词）；提交完/普通输入后把内部积的文本擦掉
        DispatchQueue.main.async { [weak self] in
            guard let self, self.markedTextRange == nil,
                  let current = self.text, !current.isEmpty else { return }
            self.text = ""
        }
        return true
    }

    /// 不调 super：字段永远保持为空，只负责收键盘
    private func forward(_ text: String) {
        let payload: Data
        if text == "\n" || text == "\r" {
            payload = Data([0x0d])
        } else {
            payload = Data(text.utf8)
        }
        #if DEBUG
        debugDump(payload)
        #endif
        wire.async { [onBytes] in onBytes?(payload) }
    }

    /// 仅 DEBUG：把经手的所有字节追加到宿主机文件，供 UI 探针断言「键盘 → 字节」逐键通路
    /// （模拟器 App 直接写宿主机文件系统，真机上路径不可写时静默跳过）。
    private func debugDump(_ payload: Data) {
        let text = String(decoding: payload, as: UTF8.self)
            .replacingOccurrences(of: "\r", with: "\\r")
            .replacingOccurrences(of: "\n", with: "\\n")
        let line = "\(Int(Date().timeIntervalSince1970)) \(text)\n"
        let path = "/tmp/abterm_input.log"
        if let fh = FileHandle(forWritingAtPath: path) {
            defer { try? fh.close() }
            fh.seekToEndOfFile()
            if let d = line.data(using: .utf8) { fh.write(d) }
        } else if let d = line.data(using: .utf8) {
            try? d.write(to: URL(fileURLWithPath: path))
        }
    }
}

extension TerminalInputField: UITextFieldDelegate {

    /// 软键盘 return 键走 delegate 而不是 insertText —— 直发 CR，且别让默认行为收键盘。
    /// 组词没提交时按 return：iOS 默认把组词缓冲整个丢掉（探针实测只发出 CR），
    /// 这里先把组词里的原文补发出去再发 CR，对齐「return = 提交原始字母」的 IME 直觉
    /// （想要中文就用候选词/空格提交，那走 insertText/replace，本来就会转发）。
    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        if let range = markedTextRange, let raw = text(in: range), !raw.isEmpty {
            forward(raw)
            unmarkText()
        }
        forward("\r")
        return false
    }
}
