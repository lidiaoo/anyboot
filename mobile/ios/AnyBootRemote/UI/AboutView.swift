import SwiftUI

// MARK: - 关于页（一比一移植 AboutActivity.kt 354 行）
//
// 三块：版本卡（64pt 应用图标 + 名称 + `v* · 多系统启动管理器` + 一句简介）
//      → GitHub 仓库卡（实时 ★/⑂/Issues + Star 引导）
//      → 链接卡（更新日志 / 问题反馈 / 开源许可 MIT）+ mono 署名。
//
// 与 Android 的差异只有一处、且是被平台逼的：Star 数没有 Material 的
// Filled.Star 可画，用等价的 SF Symbol 实心星（Android 这里本身也是
// 系统 Material 矢量图标，不是 Icons.kt 手绘图标）。

struct AboutView: View {

    @ObservedObject private var nav = ABNavigator.shared

    private static let repoURL = "https://github.com/lidiaoo/anyboot"
    private static let repoAPI = "https://api.github.com/repos/lidiaoo/anyboot"

    /// GitHub API 原始 JSON（缓存键 github_cache，与 JumpStore.githubCache 同名）
    @State private var stats = ""

    var body: some View {
        VStack(spacing: 0) {
            titleBar

            ScrollView {
                VStack(spacing: 0) {
                    versionCard
                    Spacer().frame(height: ABMetric.px(16))
                    repoCard
                    Spacer().frame(height: ABMetric.px(16))
                    linkCard
                    footer
                }
                .padding(.leading, ABMetric.px(20))
                .padding(.top, ABMetric.px(8))
                .padding(.trailing, ABMetric.px(20))
                .padding(.bottom, ABMetric.px(24))
            }
        }
        .background(ABColor.base.ignoresSafeArea())
        .onAppear(perform: loadRepoStats)
    }

    // MARK: - 标题栏（:75-84）

    private var titleBar: some View {
        HStack(spacing: 0) {
            ABBarIcon(text: "‹", description: "返回") { nav.back() }
            Text("关于")
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

    // MARK: - 版本卡（:92-131）

    private var versionCard: some View {
        VStack(spacing: 0) {
            ZStack {
                RoundedRectangle(cornerRadius: ABMetric.px(24), style: .continuous)
                    .fill(ABColor.accent.opacity(0.15))
                    .frame(width: ABMetric.px(64), height: ABMetric.px(64))
                DrawnIcon.desktop(ABColor.accent, size: ABMetric.px(36))
            }

            Text("AnyBoot Remote")
                .font(.system(size: 18, weight: .bold))
                .foregroundColor(ABColor.fg)
                .padding(.top, ABMetric.px(12))

            Text("v" + Self.versionName + " · 多系统启动管理器")
                .font(ABFont.mono(12))
                .foregroundColor(ABColor.fgFaint)
                .padding(.top, ABMetric.px(2))

            Text("多系统一键切换 · GRUB 管理 · 远程电源控制 · SSH 跳板机隧道")
                .font(.system(size: 12))
                .foregroundColor(ABColor.fgDim)
                .multilineTextAlignment(.center)
                .lineSpacing(ABMetric.px(19) - 12)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, ABMetric.px(12))
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, ABMetric.px(24))
        .padding(.horizontal, ABMetric.px(16))
        .background(abCardBackground(corner: 16))
    }

    // MARK: - GitHub 仓库卡（:134-200）

    private var repoCard: some View {
        VStack(spacing: 0) {
            Button {
                open(Self.repoURL)
            } label: {
                HStack(spacing: ABMetric.px(12)) {
                    DrawnIcon.code(ABColor.fg, size: ABMetric.px(22))

                    VStack(alignment: .leading, spacing: 0) {
                        Text("GitHub · lidiaoo/anyboot")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(ABColor.fg)
                            .lineLimit(1)
                            .truncationMode(.tail)
                        Text("开源项目主页")
                            .font(.system(size: 11))
                            .foregroundColor(ABColor.fgDim)
                            .padding(.top, ABMetric.px(1))
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)

                    // Star 徽标：实时拉到就用真实数，拉不到显示「…」+ Star
                    HStack(spacing: 0) {
                        Image(systemName: "star.fill")
                            .font(.system(size: ABMetric.px(12)))
                            .foregroundColor(ABColor.warn)
                        Text(" " + statsLabel("stargazers_count", ""))
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundColor(ABColor.warn)
                    }
                    .padding(.horizontal, ABMetric.px(10))
                    .padding(.vertical, ABMetric.px(4))
                    .background(Capsule().fill(ABColor.warn.opacity(0.1)))

                    Text(" ›")
                        .font(.system(size: 14))
                        .foregroundColor(ABColor.fgFaint)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            // Star / Fork / Issues 三格（真实 API 数据）
            Spacer().frame(height: ABMetric.px(12))
            HStack(spacing: 0) {
                statText(statsLabel("stargazers_count", "Star"))
                statText(statsLabel("forks_count", "Fork"))
                statText(statsLabel("open_issues_count", "Issues"))
            }
            .frame(maxWidth: .infinity)

            Text("点 Star 会打开 GitHub 网页，请在网页上点一下 —— 这是给开源作者最好的鼓励。")
                .font(.system(size: 11))
                .foregroundColor(ABColor.fgFaint)
                .lineSpacing(ABMetric.px(16) - 11)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, ABMetric.px(10))
        }
        .padding(ABMetric.px(16))
        .background(abCardBackground(corner: 16))
    }

    private func statText(_ label: String) -> some View {
        Text(label)
            .font(.system(size: 12))
            .foregroundColor(ABColor.fgDim)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: - 链接卡（:203-216）

    private var linkCard: some View {
        VStack(spacing: 0) {
            linkRow("更新日志", url: Self.repoURL + "/releases") {
                DrawnIcon.clock(ABColor.fgDim, size: ABMetric.px(18))
            }
            linkDivider
            linkRow("问题反馈（Issues）", url: Self.repoURL + "/issues") {
                DrawnIcon.code(ABColor.fgDim, size: ABMetric.px(18))
            }
            linkDivider
            linkRow("开源许可", url: Self.repoURL + "/blob/main/LICENSE", suffix: "MIT") {
                DrawnIcon.docFile(ABColor.fgDim, size: ABMetric.px(18))
            }
        }
        .background(abCardBackground(corner: 16))
    }

    private var linkDivider: some View {
        Rectangle()
            .fill(ABColor.line)
            .frame(height: 1)
    }

    /// 链接行：图标 + 文字 +（可选 mono 后缀）+ ›（设计稿链接卡样式）
    private func linkRow<Icon: View>(_ label: String, url: String, suffix: String = "",
                                     @ViewBuilder icon: () -> Icon) -> some View {
        Button {
            open(url)
        } label: {
            HStack(spacing: 0) {
                icon()
                Spacer().frame(width: ABMetric.px(12))
                Text(label)
                    .font(.system(size: 14))
                    .foregroundColor(ABColor.fg)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if !suffix.isEmpty {
                    Text(suffix)
                        .font(ABFont.mono(11))
                        .foregroundColor(ABColor.fgFaint)
                }
                Text(" ›")
                    .font(.system(size: 14))
                    .foregroundColor(ABColor.fgFaint)
            }
            .padding(.horizontal, ABMetric.px(16))
            .padding(.vertical, ABMetric.px(14))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - 署名（:218-227）

    private var footer: some View {
        Text("Made with AnyBoot · 2026")
            .font(ABFont.mono(10))
            .foregroundColor(ABColor.fgFaint)
            .multilineTextAlignment(.center)
            .frame(maxWidth: .infinity)
            .padding(.top, ABMetric.px(20))
    }

    // MARK: - 卡片底（card + 1pt line，圆角 16）

    private func abCardBackground(corner: CGFloat) -> some View {
        RoundedRectangle(cornerRadius: ABMetric.px(corner), style: .continuous)
            .fill(ABColor.card)
            .overlay(
                RoundedRectangle(cornerRadius: ABMetric.px(corner), style: .continuous)
                    .strokeBorder(ABColor.line, lineWidth: 1))
    }

    // MARK: - 数据

    /// `statsLabel`（:249-256）：解析不到就是「…」
    private func statsLabel(_ key: String, _ suffix: String) -> String {
        guard let data = stats.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return suffix.isEmpty ? "…" : "… \(suffix)" }
        let value = "\(object[key] as? Int ?? 0)"
        return suffix.isEmpty ? value : "\(value) \(suffix)"
    }

    private static var versionName: String {
        (Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String) ?? "?"
    }

    /// 实时拉 GitHub API；拉不到维持缓存值 / 占位符（:320-345）
    private func loadRepoStats() {
        // 先上缓存，网络回包再覆盖
        if let cached = UserDefaults.standard.string(forKey: "github_cache"), !cached.isEmpty {
            stats = cached
        }
        Task {
            guard let url = URL(string: Self.repoAPI) else { return }
            var request = URLRequest(url: url)
            request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
            request.timeoutInterval = 5
            guard let (data, response) = try? await URLSession.shared.data(for: request),
                  (response as? HTTPURLResponse)?.statusCode == 200,
                  let text = String(data: data, encoding: .utf8) else { return }
            UserDefaults.standard.set(text, forKey: "github_cache")
            await MainActor.run { stats = text }
        }
    }

    private func open(_ url: String) {
        guard let url = URL(string: url) else { return }
        UIApplication.shared.open(url) { ok in
            if !ok { ABToastBus.shared.show("没有可用的浏览器") }
        }
    }
}
