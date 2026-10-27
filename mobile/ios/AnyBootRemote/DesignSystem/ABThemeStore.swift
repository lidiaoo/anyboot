import SwiftUI

// MARK: - 主题模式落盘（对齐 Ui.java:27-74，键名 ui_theme）

@MainActor
final class ABThemeStore: ObservableObject {

    static let shared = ABThemeStore()

    private let key = "ui_theme"

    @Published private(set) var mode: ABThemeMode

    private init() {
        let raw = UserDefaults.standard.string(forKey: key) ?? ABThemeMode.auto.rawValue
        mode = ABThemeMode(rawValue: raw) ?? .auto
    }

    /// 「暗色 / 亮色 / 跟随系统」
    var label: String { mode.label }

    /// 三挡循环：auto → dark → light → auto
    func cycle() {
        set(mode.next)
    }

    func set(_ next: ABThemeMode) {
        mode = next
        UserDefaults.standard.set(next.rawValue, forKey: key)
    }

    /// 当前是否深色（给需要在 Swift 侧判深浅的地方用，例如终端初始配色）
    var isDark: Bool {
        switch mode {
        case .dark: return true
        case .light: return false
        case .auto:
            return UITraitCollection.current.userInterfaceStyle == .dark
        }
    }
}
