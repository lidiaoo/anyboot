import SwiftUI

// MARK: - 设计令牌（一比一照抄 Android）
//
// 唯一来源：android/.../Ui.java:81-141（色板）、:151-162（自适应因子）、
//          Theme.kt:49-50（终端底色）、ComputerListActivity.kt:3244-3270（CqTone 两套）、
//          :1713-1717（启动项分类点）。
//
// ⚠️ 这里**不允许自行调色或改数值**。iOS 之前那套（accent #2563EB、六通道专属色）
// 是自己编的，已全部作废：Android 根本没有「每个通道一种颜色」，
// 通道徽章用 fgDim，赢家标注用 accent。

enum ABColor {

    // ---- Ui.base / card / elevate：三层底 ----
    static let base   = Color(light: 0xF4F6FA, dark: 0x020617)  // Ui.base
    static let card   = Color(light: 0xFFFFFF, dark: 0x0E1223)  // Ui.card
    static let elevate = Color(light: 0xF1F4F9, dark: 0x141A2E) // Ui.elevate

    // ---- 线 ----
    static let line       = Color(light: 0xE2E8F0, dark: 0x242B41) // Ui.line
    static let lineStrong = Color(light: 0xCBD5E1, dark: 0x334155) // Ui.lineStrong

    // ---- 文字三级 ----
    static let fg      = Color(light: 0x0F172A, dark: 0xF8FAFC) // Ui.fg
    static let fgDim   = Color(light: 0x5A6476, dark: 0x94A3B8) // Ui.fgDim
    static let fgFaint = Color(light: 0x8A94A6, dark: 0x64748B) // Ui.fgFaint

    // ---- 语义 ----
    static let ok       = Color(light: 0x16A34A, dark: 0x22C55E) // Ui.ok
    static let accent   = Color(light: 0x4F46E5, dark: 0x6366F1) // Ui.accent
    static let warn     = Color(light: 0xB45309, dark: 0xF59E0B) // Ui.warn
    static let bad      = Color(light: 0xDC2626, dark: 0xEF4444) // Ui.bad
    static let onAccent = Color(light: 0xFFFFFF, dark: 0xFFFFFF) // Ui.onAccent

    /// 未测出 / 中性：Android 用 fgFaint，不另立色
    static let neutral = fgFaint

    // ---- CqTone（ComputerListActivity.kt:3244-3270）----
    /// 在线点 / 唤醒键实心底
    static let toneOnline     = Color(light: 0x059669, dark: 0x22C55E)
    /// 离线点
    static let toneOffline    = Color(light: 0xC7CEDD, dark: 0x3F4A63)
    /// 离线文字
    static let toneOfflineText = Color(light: 0x7B8497, dark: 0x94A3B8)
    /// 忙碌 / 读取中
    static let toneBusy       = Color(light: 0xD97706, dark: 0xF59E0B)
    /// 睡眠
    static let toneSleep      = Color(light: 0x2563EB, dark: 0x60A5FA)
    /// 启动项软底
    static let toneSoft         = Color(light: 0xFAFBFF, dark: 0x111A2E)
    /// 启动项不可用底
    static let toneSoftDisabled = Color(light: 0xF7F8FC, dark: 0x0B1120)
    /// 虚线边框
    static let toneDashed       = Color(light: 0xDDE1EE, dark: 0x2A3550)
    /// 待生效条底 / 边 / 字
    static let tonePendBg     = Color(light: 0xFFFCF2, dark: 0x2A2210)
    static let tonePendBorder = Color(light: 0xF0C979, dark: 0x5C4718)
    static let tonePendText   = Color(light: 0xB45309, dark: 0xFBBF24)

    // ---- 启动项分类点（:1713-1717）----
    static let bootWin   = Color(light: 0x2563EB, dark: 0x2563EB)
    static let bootMac   = Color(light: 0x64748B, dark: 0x64748B)
    static let bootLinux = Color(light: 0xD97706, dark: 0xD97706)
    static let bootAuto  = Color(light: 0x6366F1, dark: 0x6366F1)
    static let bootOther = Color(light: 0x94A3B8, dark: 0x94A3B8)

    /// 电源「重启」键色（:1746，不随主题翻面）
    static let powerReboot = Color(light: 0x2563EB, dark: 0x2563EB)

    // ---- 终端区底色（Theme.kt:49-50，恒深不翻面）----
    static let terminalDarkBg  = Color(light: 0x04070F, dark: 0x04070F)
    static let terminalLightBg = Color(light: 0xFFFFFF, dark: 0xFFFFFF)

    /// 启动项分类 → 点颜色（对齐 BootDot）
    static func bootDot(_ kind: String) -> Color {
        switch kind {
        case "win": return bootWin
        case "mac": return bootMac
        case "linux": return bootLinux
        case "auto": return bootAuto
        default: return bootOther
        }
    }

    /// 通道徽章文字色：Android 用 fgDim，**没有**每通道专属色
    static func lane(_ kind: String) -> Color { fgDim }

    // ---- Android 旧名兼容（历史调用点用这些名字，值已统一到上面）----
    static var canvas: Color { base }
    static var surface: Color { card }
    static var surfaceRaised: Color { elevate }
    static var stroke: Color { line }
    static var strokeStrong: Color { lineStrong }
    static var text: Color { fg }
    static var textDim: Color { fgDim }
    static var textFaint: Color { fgFaint }
    static var good: Color { ok }
}

// MARK: - 尺寸

enum ABMetric {

    /// 宽度自适应因子：Ui.java:151-155
    /// 360pt 基准 = 1.0；最低 0.85（小屏不挤爆）、最高 1.25（平板留白同步放大）
    static var scale: CGFloat {
        let width = UIScreen.main.bounds.width
        return min(1.25, max(0.85, width / 360))
    }

    /// 所有尺寸的统一出口（对齐 Ui.dp：先乘因子再落像素）
    static func px(_ value: CGFloat) -> CGFloat { (value * scale).rounded() }

    // 4 的倍数阶梯（缩放后）
    static var s1: CGFloat { px(4) }
    static var s2: CGFloat { px(8) }
    static var s3: CGFloat { px(12) }
    static var s4: CGFloat { px(16) }
    static var s5: CGFloat { px(24) }
    static var s6: CGFloat { px(32) }

    /// 列表卡片之间
    static var gap: CGFloat { px(12) }
    /// 页面外边距
    static var pagePadding: CGFloat { px(16) }
    /// 卡片内边距（HostCard：start13/end13/top12/bottom13，取 13 为基准）
    static var cardPadding: CGFloat { px(13) }
    static var cardPaddingTop: CGFloat { px(12) }

    // 圆角（照抄具体值，不取整到设计系统阶梯）
    static var radiusCard: CGFloat { px(15) }     // 电脑卡片
    static var radiusBoot: CGFloat { px(11) }     // 启动项 / 电源键
    static var radiusChip: CGFloat { px(5) }      // 徽章 / 当前 chip
    static var radiusChipSmall: CGFloat { px(4) } // OS 徽标 / 待重启 chip
    static var radiusKey: CGFloat { px(10) }      // 键帽
    /// 通用控件圆角（输入框 / 小节卡）
    static var radiusControl: CGFloat { px(10) }
    static var radiusIcon: CGFloat { px(16) }     // 抽屉图标块
    static var radiusBar: CGFloat { CGFloat(999) } // 胶囊，不缩放

    static var hairline: CGFloat { 1 }
    /// 可点区域下限（系统无障碍要求，不缩放）
    static let minTap: CGFloat = 44

    // 组件固定高（缩放后）
    static var bootRowHeight: CGFloat { px(42) }
    static var powerKeyHeight: CGFloat { px(54) }
    static var keyCapHeight: CGFloat { px(36) }
    static var statusStripeWidth: CGFloat { px(3) }
}

// MARK: - 字体（字号照抄 Android 的 sp 值）

enum ABFont {
    static let pageTitle = Font.system(size: 20, weight: .bold)   // 扫码页等标题
    static let barTitle  = Font.system(size: 16, weight: .regular) // 顶栏标题
    static let cardName  = Font.system(size: 15, weight: .semibold) // 电脑名
    static let value     = Font.system(size: 12.5, weight: .medium) // 启动项当前值
    static let body      = Font.system(size: 14, weight: .regular)
    static let caption   = Font.system(size: 12, weight: .regular)
    static let micro     = Font.system(size: 11, weight: .regular)  // 地址 / MAC
    static let state     = Font.system(size: 10.5, weight: .regular) // 状态线
    static let pair      = Font.system(size: 10, weight: .semibold) // 配对状态 / 通道徽章
    static let tiny      = Font.system(size: 10, weight: .regular)
    static let powerLabel = Font.system(size: 11, weight: .semibold)
    static let powerHint = Font.system(size: 8.5, weight: .regular)

    static func mono(_ size: CGFloat = 11, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }
    static func number(_ size: CGFloat = 15, _ weight: Font.Weight = .semibold) -> Font {
        .system(size: size, weight: weight, design: .rounded).monospacedDigit()
    }

    // 历史调用点兼容（值已统一到上面）
    static var title: Font { pageTitle }
    static var cardTitle: Font { cardName }
    static var badge: Font { .system(size: 11, weight: .semibold) }
}

// MARK: - 主题模式（Ui.java:27-74：auto / dark / light，键名 ui_theme）

enum ABThemeMode: String, CaseIterable {
    case auto, dark, light

    var label: String {
        switch self {
        case .auto: return "跟随系统"
        case .dark: return "暗色"
        case .light: return "亮色"
        }
    }

    var colorScheme: ColorScheme? {
        switch self {
        case .auto: return nil
        case .dark: return .dark
        case .light: return .light
        }
    }

    /// 循环 auto → dark → light → auto
    var next: ABThemeMode {
        switch self {
        case .auto: return .dark
        case .dark: return .light
        case .light: return .auto
        }
    }
}

// MARK: - Color 便利

extension Color {
    /// 明暗两套十六进制，跟随系统外观自动切
    init(light: UInt32, dark: UInt32) {
        self.init(uiColor: UIColor { traits in
            UIColor(hex: traits.userInterfaceStyle == .dark ? dark : light)
        })
    }

    /// 固定色（不随主题翻面）
    init(hex: UInt32) {
        self.init(uiColor: UIColor(hex: hex))
    }
}

extension UIColor {
    convenience init(hex: UInt32) {
        self.init(red: CGFloat((hex >> 16) & 0xFF) / 255,
                  green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255,
                  alpha: 1)
    }
}
