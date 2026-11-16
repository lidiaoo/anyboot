import Foundation
import Combine

/// 应用内语言（I0 基础设施）。
///
/// 与 PC / Android 共用同一套 key（dotted，如 `hub.list.title`）与 9 种语言：
/// zh-CN / zh-TW / en-US / ja-JP / ko-KR / de-DE / ru-RU / pt-BR / es-ES。
/// 资源是原生 `Resources/<lproj>/Localizable.strings`（不用 JS 桥）。
/// 偏好存 UserDefaults("anyboot.lang")：空 / "auto" = 跟随系统。
enum AppLanguage {
    static let key = "anyboot.lang"
    static let supported = ["auto", "zh-CN", "zh-TW", "en-US", "ja-JP", "ko-KR", "de-DE", "ru-RU", "pt-BR", "es-ES"]

    static var current: String {
        get { UserDefaults.standard.string(forKey: key) ?? "auto" }
        set { UserDefaults.standard.set(newValue, forKey: key) }
    }

    /// 解析成具体语言标签（auto 跟随系统）
    static var resolved: String {
        let p = current
        if p == "auto" || p.isEmpty { return Locale.preferredLanguages.first ?? "zh-CN" }
        return p
    }

    /// 映射到 `.lproj` 名
    static var lproj: String {
        let tag = resolved
        if tag.hasPrefix("zh") { return (tag.contains("TW") || tag.contains("Hant")) ? "zh-Hant" : "zh-Hans" }
        if tag.hasPrefix("pt") { return "pt-BR" }
        return tag.split(separator: "-").first.map(String.init) ?? tag
    }

    private static var bundle: Bundle {
        if let path = Bundle.main.path(forResource: lproj, ofType: "lproj"),
           let b = Bundle(path: path) {
            return b
        }
        return Bundle.main
    }

    static func string(_ key: String, _ args: [CVarArg]) -> String {
        var fmt = bundle.localizedString(forKey: key, value: nil, table: nil)
        // 资源用 {name}/{n}/{x} 等命名占位；按参数顺序逐个替换任意 {…}
        for a in args {
            guard let r = fmt.range(of: "\\{[^}]*\\}", options: .regularExpression) else { break }
            fmt.replaceSubrange(r, with: String(describing: a))
        }
        return fmt
    }
}

/// 语言切换触发器：切换时 `objectWillChange` → 根视图 `.id` 变 → 整树重建，立即以新语言重绘（无需重启 App）。
final class ABLanguageStore: ObservableObject {
    static let shared = ABLanguageStore()
    @Published var current: String = AppLanguage.current
    func set(_ tag: String) { AppLanguage.current = tag; current = tag }
}

/// 本地化取词：`L("hub.list.title")` 或 `L("hub.power.wake", name)`。
func L(_ key: String, _ args: CVarArg...) -> String {
    AppLanguage.string(key, args)
}
