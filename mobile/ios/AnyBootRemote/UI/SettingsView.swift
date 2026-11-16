import SwiftUI

/// 设置（侧边栏「设置」进入）：**语言** + **主题** + **终端设置** 三区块。
/// 原服务器模块标题栏齿轮/⋮菜单的「终端设置」并入这里，齿轮与菜单项已删除。
struct SettingsView: View {

    @ObservedObject private var nav = ABNavigator.shared
    @ObservedObject private var theme = ABThemeStore.shared
    @ObservedObject private var langStore = ABLanguageStore.shared

    private static let langNames: [(String, String)] = [
        ("auto", "跟随系统 / Auto"), ("zh-CN", "简体中文"), ("zh-TW", "繁體中文"),
        ("en-US", "English"), ("ja-JP", "日本語"), ("ko-KR", "한국어"),
        ("de-DE", "Deutsch"), ("ru-RU", "Русский"), ("pt-BR", "Português (Brasil)"), ("es-ES", "Español"),
    ]

    var body: some View {
        VStack(spacing: 0) {
            titleBar
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    languageSection
                    themeSection
                    // 终端设置区块（复用 TerminalSettingsSheet 的 settingsBody，整段并入）
                    TerminalSettingsSheet().settingsBody
                }
                .padding(.leading, ABMetric.px(20))
                .padding(.top, ABMetric.px(8))
                .padding(.trailing, ABMetric.px(20))
                .padding(.bottom, ABMetric.px(24))
            }
        }
        .background(ABColor.base.ignoresSafeArea())
    }

    private var titleBar: some View {
        HStack(spacing: 0) {
            ABBarIcon(text: "‹", description: L("and.back")) { nav.back() }
            Text(L("and.settings"))
                .font(.system(size: 20, weight: .bold))
                .foregroundColor(ABColor.fg)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity)
        .padding(.leading, ABMetric.px(4))
        .padding(.top, ABMetric.px(10))
        .padding(.bottom, ABMetric.px(8))
        .background(ABColor.base)
    }

    // MARK: - 语言

    private var languageSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            ABSectionTitle(L("and.language"))
            ForEach(Self.langNames, id: \.0) { tag, name in
                let selected = langStore.current == tag
                Button { langStore.set(tag) } label: {
                    HStack {
                        Text(name).font(.system(size: 14))
                            .foregroundColor(selected ? ABColor.accent : ABColor.fg)
                        Spacer()
                        if selected { Text("✓").foregroundColor(ABColor.accent) }
                    }
                    .padding(.horizontal, ABMetric.px(12))
                    .padding(.vertical, ABMetric.px(9))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }

    // MARK: - 主题

    private var themeSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            ABSectionTitle(L("and.themeMode"))
            themeRadio(.auto, L("and.themeAuto"))
            themeRadio(.light, L("and.themeLight"))
            themeRadio(.dark, L("and.themeDark"))
            themeRadio(.nord, L("and.themeNord"))
            themeRadio(.dracula, L("and.themeDracula"))
            themeRadio(.solarized, L("and.themeSolarized"))
        }
    }

    private func themeRadio(_ mode: ABThemeMode, _ label: String) -> some View {
        Button { theme.set(mode) } label: {
            HStack(spacing: ABMetric.px(6)) {
                ZStack {
                    Circle()
                        .strokeBorder(theme.mode == mode ? ABColor.accent : ABColor.fgDim, lineWidth: 2)
                        .frame(width: ABMetric.px(18), height: ABMetric.px(18))
                    if theme.mode == mode {
                        Circle().fill(ABColor.accent).frame(width: ABMetric.px(9), height: ABMetric.px(9))
                    }
                }
                Text(label).font(.system(size: 14)).foregroundColor(ABColor.fg)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, ABMetric.px(12))
            .padding(.vertical, ABMetric.px(6))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
