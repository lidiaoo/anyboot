import SwiftUI

@main
struct AnyBootRemoteApp: App {

    @ObservedObject private var theme = ABThemeStore.shared

    init() {
        #if DEBUG
        // 调试种子：必须在 ServerStore 首次读取之前写入（见 DemoSeed 注释）
        DemoSeed.applyIfRequested()
        #endif
        // 跳板机通道的心跳解析器：探测层不碰 SSH 栈，由 App 在这里接上。
        // ⚠️ 此前从未注入 → resolver 恒为 nil → 走跳板的电脑心跳探测永远静默失败
        //（probeOne 报「没配跳板机解析器」），而预览/加载走 BootService.localEndpoint
        // 不经它 —— 表现成「通信通、网络探测不通」。
        Heartbeat.shared.tunnelResolver = { profile in
            try await SshSessionManager.shared.tunnelAddress(for: profile)
        }
        // 装回上次的传输清单：进程被杀后暂停中的任务要还在
        Task { @MainActor in TransferCenter.shared.restore() }
        // 周期云同步的定时器（SyncEngine 内部自判开关与配置）
        Task { @MainActor in SyncEngine.shared.restartPeriodic() }
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                // 主题三挡（ui_theme）：跟随系统 / 暗色 / 亮色
                .preferredColorScheme(theme.mode.colorScheme)
                // 全局轻提示（替代 Android 的 Toast.makeText）
                .abToastBus()
                // 主机指纹确认（TOFU）由 ABConfirmBus 自己用 UIAlertController 弹，
                // 不再挂在根视图上 —— 终端页是全屏盖，根上的 .alert 盖不住也弹不出
        }
    }
}

/// 根容器：抽屉 + 当前页
struct RootView: View {

    @StateObject private var nav = ABNavigator.shared

    var body: some View {
        ZStack {
            ABColor.base.ignoresSafeArea()

            Group {
                switch nav.page {
                case .computer:
                    ComputerListView()
                case .computers:
                    ComputerTabsView()
                case .servers:
                    ServerModuleView()
                case .about:
                    AboutView()
                case .backup:
                    BackupPage()
                }
            }
            // 全局控件色 = 靛蓝 accent：所有没显式染色的 ProgressView 等
            // 走 token 色而不是系统蓝（对齐 Android 的 M3 primary = accent）
            .tint(ABColor.accent)

            // 系统信息弹层（多入口共用，挂在根上）
            SystemInfoOverlay()

            // 抽屉
            ABDrawerOverlay()
        }
    }
}

// MARK: - 全局导航（抽屉里的五项）

enum ABPage: String {
    // ⚠️ 这两个 case 名跟直观读法相反（历史包袱，别按字面理解）：
    //   .computers = 电脑页（多标签 WebView，Android 的 MainActivity）
    //   .computer  = 电脑列表（卡片列表，Android 的 ComputerListActivity，默认进入页）
    case computers    // 电脑页（多标签）
    case computer     // 电脑列表
    case servers      // 服务器模块
    case about        // 关于
    case backup       // 备份与恢复
}

@MainActor
final class ABNavigator: ObservableObject {

    static let shared = ABNavigator()

    /// 启动落「电脑列表」—— 对齐 Android：launcher 冷启动 / 点桌面图标一律先送
    /// 电脑列表页（MainActivity onCreate/onNewIntent 里 startActivity(ComputerListActivity)）
    @Published var page: ABPage = .computer
    @Published var drawerOpen = false

    /// 电脑页要切到哪台（从列表点卡片进来时用）
    @Published var activeProfileId: String = ""

    /// 上一个页（关于页的 ‹ 要回得去，对齐 AboutActivity 的 finish()）
    private var previous: ABPage = .computer

    private init() {}

    func go(_ target: ABPage) {
        if target != page { previous = page }
        page = target
        drawerOpen = false
    }

    /// 返回上一页（只有关于页用到：Android 那边是 finish() 回宿主 Activity）
    func back() {
        page = previous
        drawerOpen = false
    }

    func openDrawer() { drawerOpen = true }
    func closeDrawer() { drawerOpen = false }
}
