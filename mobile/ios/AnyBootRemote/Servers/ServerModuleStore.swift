import SwiftUI

/// 服务器模块（对齐 Android `ServersActivity`）的全部可变状态。
///
/// Android 是一个 Activity 里的一堆 `mutableStateOf`；iOS 收进一个 `ObservableObject`，
/// 页面按它切，弹层由根视图统一挂 —— 行为与字段一一对应，别在页面里另起同名状态。
@MainActor
final class ServerModuleStore: ObservableObject {

    static let shared = ServerModuleStore()

    /// 五个页面（底部导航只有四项；会话 / 传输在顶部 ⋮ 里）
    enum Page: Int, CaseIterable {
        case servers = 0
        case keys = 1
        case files = 2
        case sessions = 3
        case transfers = 4
    }

    // ---- 导航 ----
    @Published var page: Page = .servers
    // 抽屉态在 ABNavigator（RootView 统一挂一份），这里不重复持有一份，
    // 否则两套 drawerOpen 会打架（曾是「点 ☰ 没反应」的原因）
    /// 搜索（服务器页）
    @Published var search = ""
    /// 卡片展开态（按 jump.id 记，顺序变了也不串）
    @Published var expanded: Set<String> = []
    /// 会话注册表 / 传输任务变化的自增计数（订阅刷新用）
    @Published var tick = 0
    @Published var transferTick = 0

    // ---- 弹层 ----
    @Published var editOpen = false
    @Published var editTarget: JumpServer?
    @Published var deleteTarget: JumpServer?
    @Published var killTarget: TerminalSessionRef?
    @Published var generateKeyOpen = false
    @Published var deleteKeyTarget: SshKeyEntry?
    /// 导入私钥文件的选择器
    @Published var keyImportOpen = false
    @Published var manualPassword: ManualPasswordRequest?

    /// 终端页：非空 = 正打开这台服务器的终端（`cdPath` 非空时连上先 cd 过去）
    @Published var terminalJumpId: String?
    @Published var terminalCdPath = ""
    /// 终端设置（顶栏齿轮 / ⋮ 菜单都进这里）
    @Published var terminalSettingsOpen = false

    /// 应急手输密码请求（对齐 `ManualPwdRequest`）
    struct ManualPasswordRequest: Identifiable {
        let id = UUID()
        let server: JumpServer
        let onReady: () -> Void
    }

    private init() {}

    // MARK: - 导航

    func open(_ page: Page) { self.page = page }

    func toggleExpanded(_ id: String) {
        guard tick >= 0 else { return }
        if expanded.contains(id) {
            expanded.remove(id)
        } else {
            expanded.insert(id)
        }
    }

    /// 「终端」导航项：跳最近一个活会话；没有就说明去哪开 —— `ServersActivity.kt:457`
    func openLatestSession() -> String? {
        let live = TerminalSessionsRegistry.all
        guard let latest = live.last else {
            toast("没有进行中的会话 —— 到「服务器」页点「终端」开始", long: true)
            return nil
        }
        return latest.server.id
    }

    /// 打开某台服务器的终端页（可带启动目录）
    func openTerminal(jumpId: String, cdPath: String = "") {
        terminalCdPath = cdPath
        terminalJumpId = jumpId
    }

    func closeTerminal() {
        terminalJumpId = nil
        terminalCdPath = ""
    }

    // MARK: - 小件

    func toast(_ text: String, long: Bool = false) {
        ABToastBus.shared.show(text, seconds: long ? 3.4 : 2.0)
    }

    /// 会话注册表变了：刷在线数 / 会话页
    func markSessionsChanged() { tick &+= 1 }
    func markTransfersChanged() { transferTick &+= 1 }

    /// 导入私钥文件（`:1702`）
    func importKey(_ url: URL) {
        do {
            let entry = try KeyFactory.importPem(at: url, fallbackName: "imported-\(JumpStore.shared.keys.count)")
            JumpStore.shared.upsert(entry)
            toast("已导入 \(entry.algorithm) 密钥")
        } catch {
            toast("导入失败：" + (error.localizedDescription), long: true)
        }
    }

    /// 「经由此服务器的电脑」台数
    func computersVia(_ id: String) -> Int { JumpStore.shared.computersVia(id) }
}
