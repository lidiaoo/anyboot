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
        case .dark, .nord, .dracula: return true
        case .light, .solarized: return false
        case .auto:
            return UITraitCollection.current.userInterfaceStyle == .dark
        }
    }
}

// MARK: - 配色板（与 PC 端 5 套一致：light / dark / nord / dracula / solarized）

struct ABPalette {
    let base: Color, card: Color, elevate: Color
    let line: Color, lineStrong: Color
    let fg: Color, fgDim: Color, fgFaint: Color
    let ok: Color, accent: Color, warn: Color, bad: Color, onAccent: Color
    init(_ b: UInt32,_ c: UInt32,_ e: UInt32,_ l: UInt32,_ ls: UInt32,_ fg: UInt32,_ fd: UInt32,_ ff: UInt32,_ ok: UInt32,_ ac: UInt32,_ wa: UInt32,_ ba: UInt32) {
        base=Color(hex:b); card=Color(hex:c); elevate=Color(hex:e)
        line=Color(hex:l); lineStrong=Color(hex:ls)
        self.fg=Color(hex:fg); fgDim=Color(hex:fd); fgFaint=Color(hex:ff)
        self.ok=Color(hex:ok); accent=Color(hex:ac); warn=Color(hex:wa); bad=Color(hex:ba)
        onAccent = Color(hex: 0xFFFFFF)
    }
}

extension ABThemeStore {
    nonisolated static let palettes: [String: ABPalette] = [
        "dark":      ABPalette(0x020617,0x0E1223,0x141A2E,0x242B41,0x334155,0xF8FAFC,0x94A3B8,0x64748B,0x22C55E,0x6366F1,0xF59E0B,0xEF4444),
        "light":     ABPalette(0xF4F6FA,0xFFFFFF,0xF1F4F9,0xE2E8F0,0xCBD5E1,0x0F172A,0x5A6476,0x8A94A6,0x16A34A,0x4F46E5,0xB45309,0xDC2626),
        "nord":      ABPalette(0x2E3440,0x3B4252,0x434C5E,0x4C566A,0x5E6980,0xECEFF4,0xD8DEE9,0x8F9CB3,0xA3BE8C,0x88C0D0,0xEBCB8B,0xBF616A),
        "dracula":   ABPalette(0x282A36,0x323444,0x44475A,0x44475A,0x626680,0xF8F8F2,0xC8CAD6,0x82859C,0x50FA7B,0xBD93F9,0xF1FA8C,0xFF5555),
        "solarized": ABPalette(0xFDF6E3,0xFFFFFF,0xEEE8D5,0xDED8C5,0xC8C2AF,0x073642,0x586E75,0x93A1A1,0x859900,0x268BD2,0xB58900,0xDC322F),
    ]
    /// 当前生效配色 id：auto → 按系统明暗
    var paletteId: String {
        switch mode {
        case .auto:
            return UITraitCollection.current.userInterfaceStyle == .dark ? "dark" : "light"
        default:
            return mode.rawValue
        }
    }
    var palette: ABPalette { Self.palettes[paletteId] ?? Self.palettes["dark"]! }
}

extension ABThemeStore {
    /// nonisolated：供 ABColor 静态色在任意上下文取当前配色（读 UserDefaults，不碰 @MainActor 状态）
    nonisolated static var currentPalette: ABPalette {
        let raw = UserDefaults.standard.string(forKey: "ui_theme") ?? "auto"
        let id: String
        if raw == "auto" || raw.isEmpty {
            id = UITraitCollection.current.userInterfaceStyle == .dark ? "dark" : "light"
        } else { id = raw }
        return palettes[id] ?? palettes["dark"]!
    }
}
