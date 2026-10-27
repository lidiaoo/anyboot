import SwiftUI

// MARK: - 抽屉侧边栏（一比一移植 Drawer.kt 213 行）
//
// 三个入口（电脑页 / 电脑列表 / 服务器）共用左上角 ☰。当前页高亮、主题循环。

struct ABDrawerOverlay: View {

    @ObservedObject private var nav = ABNavigator.shared
    @ObservedObject private var theme = ABThemeStore.shared

    var body: some View {
        GeometryReader { geo in
            // 宽度自适应：宽屏封顶 290，窄窗（分屏/小窗）最多占 85%
            let width = min(ABMetric.px(290), geo.size.width * 0.85)

            ZStack(alignment: .leading) {
                if nav.drawerOpen {
                    Color.black.opacity(0.33)   // #55000000
                        .ignoresSafeArea()
                        .contentShape(Rectangle())
                        .onTapGesture { nav.closeDrawer() }
                        .transition(.opacity)

                    drawer(width: width)
                        .frame(width: width)
                        .frame(maxHeight: .infinity)
                        .background(ABColor.card)
                        .transition(.move(edge: .leading))
                }
            }
            .animation(.spring(response: 0.32, dampingFraction: 0.86), value: nav.drawerOpen)
            .gesture(
                DragGesture(minimumDistance: 24)
                    .onEnded { value in
                        // 左滑关闭；从边缘右滑打开
                        if nav.drawerOpen && value.translation.width < -40 {
                            nav.closeDrawer()
                        } else if !nav.drawerOpen && value.startLocation.x < 24
                                    && value.translation.width > 40 {
                            nav.openDrawer()
                        }
                    }
            )
        }
    }

    private func drawer(width: CGFloat) -> some View {
        VStack(spacing: 0) {
            header
            ABHairline()

            ScrollView {
                VStack(spacing: 0) {
                    // 对齐 Drawer.kt:116-129：
                    //   「电脑」→ MainActivity（多标签电脑页 / WebView）
                    //   「电脑列表」→ ComputerListActivity（卡片列表）
                    // 注意 ABPage 的 case 名是历史包袱，跟直观读法相反：
                    //   .computers = 电脑页（多标签） / .computer = 电脑列表
                    navRow(.computers, "电脑") { tint, size in
                        DrawnIcon.desktop(tint, size: size)
                    }
                    navRow(.computer, "电脑列表") { tint, size in
                        DrawnIcon.serverStack(tint, size: size)
                    }
                    navRow(.servers, "服务器") { tint, size in
                        DrawnIcon.serverStack(tint, size: size)
                    }
                    navRow(.backup, "备份与恢复") { tint, size in
                        DrawnIcon.swapVertical(tint, size: size)
                    }
                    navRow(.about, "关于") { tint, size in
                        DrawnIcon.info(tint, size: size)
                    }
                    // 主题项是设计稿没有的既有功能，保留在导航末尾
                    Button {
                        theme.cycle()
                        nav.closeDrawer()
                    } label: {
                        rowBody(icon: { tint, size in
                            DrawnIcon.moon(tint, size: size)
                        }, label: "主题：\(theme.label)", selected: false)
                    }
                    .buttonStyle(.plain)
                }
                .padding(ABMetric.px(12))
            }

            ABHairline()
            footer
        }
        .frame(maxHeight: .infinity, alignment: .top)
    }

    private var header: some View {
        HStack(spacing: ABMetric.px(12)) {
            ZStack {
                RoundedRectangle(cornerRadius: ABMetric.px(16), style: .continuous)
                    .fill(ABColor.accent.opacity(0.15))
                    .frame(width: ABMetric.px(44), height: ABMetric.px(44))
                DrawnIcon.desktop(ABColor.accent, size: ABMetric.px(24))
            }
            VStack(alignment: .leading, spacing: 0) {
                Text("AnyBoot")
                    .font(.system(size: 16, weight: .bold))
                    .foregroundColor(ABColor.fg)
                Text("v\(Self.appVersion) · remote")
                    .font(.system(size: 10))
                    .foregroundColor(ABColor.fgFaint)
                    .padding(.top, ABMetric.px(1))
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, ABMetric.px(20))
        .padding(.vertical, ABMetric.px(28))
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("心跳 10s · 并发探测\nGitHub · lidiaoo/anyboot")
                .font(.system(size: 10))
                .lineSpacing(ABMetric.px(16) - 12)
                .foregroundColor(ABColor.fgFaint)
                .fixedSize(horizontal: false, vertical: true)
            Text("外网连家里的电脑：先把家里能公网访问的机器加成 SSH 服务器，"
                 + "再在电脑的「经由跳板机」里选它。")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, ABMetric.px(8))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(ABMetric.px(16))
    }

    // MARK: - 导航行

    private func navRow(_ page: ABPage, _ label: String,
                        @ViewBuilder icon: @escaping (Color, CGFloat) -> some View) -> some View {
        Button {
            nav.go(page)
        } label: {
            rowBody(icon: icon, label: label, selected: nav.page == page)
        }
        .buttonStyle(.plain)
    }

    private func rowBody<Icon: View>(icon: (Color, CGFloat) -> Icon,
                                     label: String,
                                     selected: Bool) -> some View {
        HStack(spacing: ABMetric.px(12)) {
            icon(selected ? ABColor.accent : ABColor.fgDim, ABMetric.px(20))
            Text(label)
                .font(.system(size: 14, weight: selected ? .semibold : .regular))
                .foregroundColor(selected ? ABColor.accent : ABColor.fgDim)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, ABMetric.px(14))
        .padding(.vertical, ABMetric.px(12))
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(selected ? ABColor.accent.opacity(0.12) : .clear))
        .contentShape(Rectangle())
    }

    /// 版本号（抽屉头部「v* · remote」）
    static var appVersion: String {
        (Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String) ?? "1.2"
    }
}
