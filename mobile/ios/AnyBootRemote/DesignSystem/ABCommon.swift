import SwiftUI
import UIKit
import AlertToast

// MARK: - 通用小件（一比一移植 Common.kt:193-374）
//
// 字号 / 色值 / 内边距全部照抄 Android，一个数值都不改。

/// 分组标题（进行中 / 历史 / 密钥…）—— `Common.kt:199`
struct ABSectionTitle: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text)
            .font(.system(size: 14, weight: .bold))
            .foregroundColor(ABColor.fgDim)
            .padding(.leading, ABMetric.px(12))
            .padding(.top, ABMetric.px(14))
            .padding(.trailing, ABMetric.px(12))
            .padding(.bottom, ABMetric.px(4))
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// 空状态提示 —— `Common.kt:212`
struct ABEmptyHint: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text)
            .font(.system(size: 13))
            .foregroundColor(ABColor.fgFaint)
            .padding(.horizontal, ABMetric.px(20))
            .padding(.vertical, ABMetric.px(16))
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// 次要文字（副行 / 说明）—— `Common.kt:224`
struct ABSecondaryText: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundColor(ABColor.fgDim)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// 表单字段上方的小标签 —— `Common.kt:272`
struct ABFieldLabel: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundColor(ABColor.fgDim)
            .padding(.top, ABMetric.px(10))
            .padding(.bottom, ABMetric.px(2))
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// 列表行之间的细分隔线（缩进对齐文字列，避开徽标）—— `Common.kt:308`
struct ABListDivider: View {
    var body: some View {
        Rectangle()
            .fill(ABColor.line)
            .frame(height: 1)
            .padding(.leading, ABMetric.px(58))
            .padding(.trailing, ABMetric.px(12))
    }
}

/// 小按钮 chip（强调色文字 + 圆角抬升底）—— `Common.kt:242`
struct ABChipButton: View {
    let text: String
    var selected: Bool = false
    let onClick: () -> Void

    var body: some View {
        Button(action: onClick) {
            Text(text)
                .font(.system(size: 12))
                .foregroundColor(selected ? ABColor.onAccent : ABColor.accent)
                .padding(.horizontal, ABMetric.px(10))
                .padding(.vertical, ABMetric.px(6))
                .background(
                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(selected ? ABColor.accent : ABColor.elevate))
        }
        .buttonStyle(.plain)
    }
}

/// 顶栏 / 按键栏的纯文字图标按钮（☰ ＋ ▹ 之类）—— `Common.kt:258`
struct ABBarIcon: View {
    let text: String
    var description: String = ""
    var tint: Color? = nil
    let onClick: () -> Void

    var body: some View {
        Button(action: onClick) {
            Text(text)
                .font(.system(size: 17))
                .foregroundColor(tint ?? ABColor.fgDim)
                // 命中区固定 44×44（对齐 Android `BarIcon` 的 44dp 图标块）。
                // 原来只靠 padding 撑开：Text 的透明内边距不进命中区，实测 ☰
                // 的可点范围只有 15×20pt，手指稍偏一点就落空 —— 表现就是「点了没反应」。
                .frame(width: ABMetric.px(44), height: ABMetric.px(44))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(description)
    }
}

// MARK: - 格式化（口径逐字照抄 Common.kt:319-336）

enum ABFormat {

    /// `humanDuration(ms)`：秒 → 分钟 → 小时+分
    static func duration(_ ms: Int64) -> String {
        let seconds = ms / 1000
        if seconds < 60 { return "\(seconds) 秒" }
        let minutes = seconds / 60
        if minutes < 60 { return "\(minutes) 分钟" }
        return "\(minutes / 60) 小时 \(minutes % 60) 分"
    }

    /// `humanSize(bytes)`：B → KB → MB（注意 Android 只到 MB，不升 GB）
    static func size(_ bytes: Int64) -> String {
        if bytes < 1024 { return "\(bytes) B" }
        if bytes < 1024 * 1024 {
            return String(format: "%.1f KB", Double(bytes) / 1024.0)
        }
        return String(format: "%.1f MB", Double(bytes) / 1024.0 / 1024.0)
    }

    /// 系统族展示名；认不出返回空串（= 不画徽标）
    static func osShowName(_ os: String) -> String {
        switch ServerProfile.normalizeMachineOs(os) {
        case "windows": return "Windows"
        case "macos": return "macOS"
        case "linux": return "Linux"
        default: return ""
        }
    }
}

// MARK: - 系统族徽标（Common.kt:357）

struct ABOsBadge: View {

    let os: String
    /// 放在强调色（深底）上时用浅色文字
    var onDark: Bool = false
    var onClick: (() -> Void)? = nil

    var body: some View {
        let name = ABFormat.osShowName(os)
        if name.isEmpty {
            EmptyView()
        } else {
            let tint = onDark ? Color.white.opacity(0.9) : ABColor.fgDim
            Text(name)
                .font(.system(size: 9, weight: .semibold))
                .foregroundColor(tint)
                .lineLimit(1)
                .padding(.horizontal, ABMetric.px(4))
                .padding(.vertical, ABMetric.px(1))
                .background(
                    RoundedRectangle(cornerRadius: ABMetric.px(4), style: .continuous)
                        .fill(tint.opacity(0.15)))
                .contentShape(Rectangle())
                .onTapGesture { onClick?() }
        }
    }
}

// MARK: - 剪贴板（Common.kt:132）

enum ABClipboard {

    /// 复制远程接入的某一项，**一定给出结果提示**（Android 那边吃过「静默成功」的亏）
    @MainActor
    static func copyRemoteSlot(_ label: String, _ value: String) {
        let text = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty {
            ABToastBus.shared.show("没有可复制的\(label)")
            return
        }
        UIPasteboard.general.string = text
        ABToastBus.shared.show("已复制\(label)")
    }
}

// MARK: - Toast 总线（替代 Android 的 Toast.makeText）

/// 全局轻提示：任何地方 `ABToastBus.shared.show("…")`，根视图统一弹。
@MainActor
final class ABToastBus: ObservableObject {

    static let shared = ABToastBus()

    @Published var message: String?
    @Published private(set) var token = UUID()

    private var clearTask: Task<Void, Never>?

    func show(_ text: String, seconds: Double = 2.0) {
        message = text
        token = UUID()
        clearTask?.cancel()
        clearTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            guard !Task.isCancelled else { return }
            await MainActor.run { self?.message = nil }
        }
    }
}

extension View {
    /// 挂在根视图上：显示 `ABToastBus` 的提示
    func abToastBus() -> some View {
        modifier(ABToastBusModifier())
    }
}

private struct ABToastBusModifier: ViewModifier {

    @ObservedObject private var bus = ABToastBus.shared
    @State private var presenting = false

    func body(content: Content) -> some View {
        content
            .onChange(of: bus.token) { _, _ in
                guard bus.message != nil else { return }
                presenting = true
            }
            .toast(isPresenting: $presenting) {
                AlertToast(type: .regular, title: bus.message)
            }
    }
}

// MARK: - 确认框总线（Common.kt:156-191 的 ConfirmBus）

/// SSH 线程里要「等用户点按钮」才能继续（TOFU 主机指纹）。
/// Android 用 `runBlocking` 阻塞等 Compose 弹窗；Swift 这边并发模型不同，
/// 改用 continuation：`await ABConfirmBus.shared.confirm(...)` 挂起，用户点完 resume。
///
/// 弹出用 UIKit 的 `UIAlertController`，**不是** SwiftUI 的 `.alert`：
/// 终端页是 `fullScreenCover`，挂在根视图上的 `.alert` 在盖子之上弹不出来
/// （UIKit 只会打 already presenting 的警告），弹不出来就没人 resume，连接会卡在握手。
/// 挂在「当前最上层 view controller」上则没有这个问题，覆盖全屏盖 / sheet 都行。
@MainActor
final class ABConfirmBus {

    static let shared = ABConfirmBus()

    /// 同一标题+内容的并发请求共享一次弹窗（典型：心跳/预览/终端同时连同一台跳板机，
    /// TOFU 各自走到确认这一步 —— 内容完全一样，答一次全体生效）。
    private var waiters: [String: [CheckedContinuation<Bool, Never>]] = [:]
    /// key → 展示用的（标题，确认按钮文案）
    private var displays: [String: (String, String)] = [:]
    /// 不同内容的请求排队串行弹（UIAlertController 同时只能 present 一个）
    private var queue: [String] = []
    private var presenting = false

    func confirm(title: String,
                 message: String,
                 confirmTitle: String = "信任并连接") async -> Bool {
        let key = "\(title)|\(confirmTitle)|\(message)"
        if waiters[key] != nil {
            // 已有同样的确认在等/在弹：挂上去共享答案，不再重复弹
            return await withCheckedContinuation { waiters[key]?.append($0) }
        }
        waiters[key] = []
        displays[key] = (title, confirmTitle)
        queue.append(key)
        return await withCheckedContinuation { (cont: CheckedContinuation<Bool, Never>) in
            waiters[key]?.append(cont)
            pump()
        }
    }

    /// 依次弹队首的确认框；答案分发给共享该问题的所有等待者
    private func pump() {
        guard !presenting, let key = queue.first else { return }
        guard let host = ABConfirmBus.topViewController() else {
            // 没有窗口 = 界面还没起来，宁可报错也不要空等
            NSLog("[ABCONFIRM] 没有可用的窗口，确认框弹不出来")
            settle(key, false)
            return
        }
        presenting = true
        let parts = key.split(separator: "|", maxSplits: 2, omittingEmptySubsequences: false)
        let alert = UIAlertController(title: String(parts.first ?? ""),
                                      message: parts.count > 2 ? String(parts[2]) : "",
                                      preferredStyle: .alert)
        // 不跟随 App 的三挡主题会打架（暗色 App + 亮色系统 → 白弹窗）
        switch ABThemeStore.shared.mode {
        case .auto: alert.overrideUserInterfaceStyle = .unspecified
        case .dark: alert.overrideUserInterfaceStyle = .dark
        case .light: alert.overrideUserInterfaceStyle = .light
        }
        // 不可点外面关 —— 必须明确选信任 / 取消（对齐 Android onDismissRequest = {}）
        alert.addAction(UIAlertAction(title: displays[key]?.1 ?? "信任并连接",
                                      style: .default) { [weak self] _ in
            self?.settle(key, true)
        })
        alert.addAction(UIAlertAction(title: "取消", style: .cancel) { [weak self] _ in
            self?.settle(key, false)
        })
        host.present(alert, animated: true)
    }

    /// 用一个答案结清某条确认的所有等待者，然后弹下一条
    private func settle(_ key: String, _ accepted: Bool) {
        presenting = false
        queue.removeAll { $0 == key }
        displays.removeValue(forKey: key)
        let group = waiters.removeValue(forKey: key) ?? []
        for cont in group { cont.resume(returning: accepted) }
        pump()
    }

    /// 当前最上层、能 present 的 view controller（全屏盖 / sheet 之上也能弹）
    private static func topViewController() -> UIViewController? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let scene = scenes.first { $0.activationState == .foregroundActive } ?? scenes.first
        guard let window = scene?.windows.first(where: { $0.isKeyWindow }) ?? scene?.windows.first,
              var top = window.rootViewController else { return nil }
        while let presented = top.presentedViewController { top = presented }
        return top
    }
}
