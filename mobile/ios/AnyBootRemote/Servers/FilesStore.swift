import SwiftUI
import UniformTypeIdentifiers

/// 文件页的状态与全部目录操作（对齐 `ServersActivity.kt` 的 sftp* 字段与 sftpRun / transferInto 一族）。
///
/// UI 只管画；切目录、拉列表、复制粘贴、目标目录选择器、重名策略都在这里，
/// 这样「复制到…」「粘贴」两条路径共用同一套判断，不会各写一份。
@MainActor
final class FilesStore: ObservableObject {

    static let shared = FilesStore()

    enum SortKey: String, CaseIterable {
        case name, size, time

        var label: String {
            switch self {
            case .name: return "名称"
            case .size: return "大小"
            case .time: return "时间"
            }
        }
    }

    /// 应用内剪贴板的两种模式（不碰系统剪贴板）
    enum Clip { case none, copy, cut }

    /// 「复制到… / 移动到…」的目标目录选择器请求
    struct PickRequest: Identifiable {
        let id = UUID()
        let cut: Bool
        let paths: [String]
        let label: String
        let startDir: String
    }

    // ---- 状态 ----
    @Published var serverId = ""
    @Published var path = "/"
    @Published var entries: [SftpOps.Entry] = []
    @Published var busy = false
    @Published var error = ""
    /// 已经拉取过的「服务器|路径」标记 —— 空目录也不能反复重拉（否则死循环）
    private var loadedKey = ""

    @Published var sort: SortKey = .name
    @Published var sortDesc = false
    @Published var showHidden = false
    @Published var filterOpen = false
    @Published var filter = ""
    /// 非空 = 正有一个目录操作在跑，期间不再接第二个
    @Published var opLabel = ""

    @Published var clip: Clip = .none
    @Published var clipServer = ""
    @Published var clipPaths: [String] = []
    @Published var clipNames = ""

    // ---- 弹层 ----
    @Published var sheetEntry: SftpOps.Entry?
    @Published var askDelete: SftpOps.Entry?
    @Published var askRename: SftpOps.Entry?
    @Published var askDetail: SftpOps.Entry?
    @Published var mkdirOpen = false
    @Published var locateOpen = false
    @Published var pick: PickRequest?

    private init() {}

    // MARK: - 只读派生

    /// SSH 服务器（文件页只对 SSH 有意义）
    var sshServers: [JumpServer] { JumpStore.shared.jumps.filter(\.isSsh) }

    var server: JumpServer? {
        let list = sshServers
        if list.contains(where: { $0.id == serverId }) { return list.first { $0.id == serverId } }
        return list.first
    }

    /// 选中的服务器没了就自动落到第一台（写一次，不自激）
    func ensureServer() {
        let list = sshServers
        if !list.contains(where: { $0.id == serverId }) {
            serverId = list.first?.id ?? ""
        }
    }

    var isUp: Bool { !serverId.isEmpty && SshSessionManager.shared.isUp(serverId) }

    /// 过滤 + 排序后的可见项
    var shown: [SftpOps.Entry] {
        order(entries.filter { entry in
            (showHidden || !entry.name.hasPrefix("."))
                && (filter.isEmpty || entry.name.localizedCaseInsensitiveContains(filter))
        })
    }

    /// 排序：目录恒定排在最前，组内再按用户选的规则，最后拿名字兜底
    /// —— 大小 / 时间相同的项顺序也稳定，不会每刷一次就跳动。
    private func order(_ raw: [SftpOps.Entry]) -> [SftpOps.Entry] {
        let base: (SftpOps.Entry, SftpOps.Entry) -> Bool
        switch sort {
        case .name:
            base = { $0.name.lowercased() < $1.name.lowercased() }
        case .size:
            base = { $0.size < $1.size }
        case .time:
            base = { $0.mtime < $1.mtime }
        }
        let ordered = raw.sorted { a, b in
            if a.dir != b.dir { return a.dir && !b.dir }
            if base(a, b) != base(b, a) { return sortDesc ? base(b, a) : base(a, b) }
            return a.name.lowercased() < b.name.lowercased()
        }
        return ordered
    }

    // MARK: - 目录导航

    /// 切目录。`force = true` 用于「就地重拉」（路径没变也要重新拉一遍）
    func gotoDir(_ target: String, force: Bool = false) {
        if target == path && !force { return }
        path = target
        entries = []
        error = ""
        busy = true          // 先亮「加载中」，免得中间闪一下「（空目录）」
        loadedKey = ""
        Task { await load() }
    }

    func reloadDir() { gotoDir(path, force: true) }

    /// 自动加载：这个「服务器|路径」还没拉过才拉
    func loadIfNeeded() {
        guard !serverId.isEmpty else { return }
        if loadedKey == "\(serverId)|\(path)" { return }
        Task { await load() }
    }

    /// 后台拉目录（结果落 entries / error 状态）
    func load() async {
        let targetServer = serverId
        let targetPath = path
        guard let jump = server, !targetServer.isEmpty else { return }
        busy = true
        do {
            let list = try await SftpOps.list(jump: jump, path: targetPath)
            // 已经被后来的请求接管了：busy 由那个请求的收尾负责清，这里别抢
            guard targetServer == serverId, targetPath == path else { return }
            busy = false
            loadedKey = "\(targetServer)|\(targetPath)"   // 成功失败都算「拉过了」
            entries = list
        } catch {
            guard targetServer == serverId, targetPath == path else { return }
            busy = false
            loadedKey = "\(targetServer)|\(targetPath)"
            self.error = SftpOps.describe(error, action: "读取", label: targetPath)
        }
    }

    /// `/a/b/c` → `["/", "a", "b", "c"]`（根目录就是 `["/"]`）
    static func pathSegments(_ path: String) -> [String] {
        let clean = path.trimmingCharacters(in: .whitespaces)
        if clean.isEmpty || clean == "/" { return ["/"] }
        return ["/"] + clean.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            .split(separator: "/").map(String.init)
    }

    /// 面包屑第 index 段对应的完整路径（index=0 是根）
    static func pathUpTo(_ segments: [String], _ index: Int) -> String {
        if index <= 0 { return "/" }
        return "/" + segments[1...min(index, segments.count - 1)].joined(separator: "/")
    }

    static func timeText(_ seconds: Int64) -> String {
        if seconds <= 0 { return "-" }
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        formatter.locale = Locale(identifier: "en_US_POSIX")
        return formatter.string(from: Date(timeIntervalSince1970: TimeInterval(seconds)))
    }

    // MARK: - 传输

    func startDownload(_ entry: SftpOps.Entry) {
        guard server != nil else { toast("服务器不存在（可能已删除）"); return }
        let remote = SftpOps.join(path, entry.name)
        // 同一条文件已经在传了就别再起一个：两个任务写同一个本地文件会互相踩
        if TransferCenter.shared.active.contains(where: {
            $0.direction == .download && $0.remotePath == remote
        }) {
            toast("这条已经在传了，进度见「传输」页")
            return
        }
        _ = TransferCenter.shared.startDownload(jump: server!, remoteDir: path, name: entry.name)
        toast("已加入传输：\(entry.name)（进度见「传输」页）")
    }

    func upload(_ urls: [URL]) {
        guard let jump = server else { toast("先选择服务器"); return }
        var added = 0
        for url in urls {
            let name = url.lastPathComponent.isEmpty ? "upload-\(Int(Date().timeIntervalSince1970))"
                                                     : url.lastPathComponent
            let remote = SftpOps.join(path, name)
            if TransferCenter.shared.active.contains(where: {
                $0.direction == .upload && $0.remotePath == remote
            }) {
                toast("这条已经在传了，进度见「传输」页")
                continue
            }
            _ = TransferCenter.shared.startUpload(jump: jump, remoteDir: path, source: url)
            added += 1
        }
        if added > 0 {
            toast(added == 1 ? "已加入传输：\(urls[0].lastPathComponent)" : "已加入 \(added) 个传输任务")
        }
    }

    // MARK: - 目录操作

    /// 后台跑一次目录操作。期间 `opLabel` 非空（工具行亮「正在…」、再点也只提示不并发）。
    /// 完成后成功 → 重拉当前目录；失败 → 长提示弹出来。
    private func run(_ label: String, work: @escaping () async throws -> Void) {
        if !opLabel.isEmpty {
            toast("正在\(opLabel)，稍等一下")
            return
        }
        opLabel = label
        Task {
            do {
                try await work()
                opLabel = ""
                reloadDir()
            } catch {
                opLabel = ""
                toast(Self.message(of: error), long: true)
            }
        }
    }

    /// 我们自己抛的（已经是中文）原样透出，免得「删除失败：删除失败：权限不够」套娃
    private static func message(of error: Error) -> String {
        // `"\(error)"` 走反射会把自定义错误打成 `message("…")`，必须先取 errorDescription
        let text = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        if text.contains("失败") || text.contains("不支持") || text.contains("不能") || text.contains("已经") {
            return text
        }
        return text.isEmpty ? "操作失败" : text
    }

    func mkdir(_ name: String) {
        guard let jump = server else { return }
        let dir = path
        let target = SftpOps.join(dir, name)
        run("新建目录") {
            do {
                try await SftpOps.mkdir(jump: jump, path: target)
            } catch {
                throw FilesOpError(SftpOps.reason(error, action: "新建目录", path: name))
            }
        }
    }

    func rename(_ entry: SftpOps.Entry, to newName: String) {
        guard let jump = server else { return }
        let from = SftpOps.join(path, entry.name)
        let to = SftpOps.join(path, newName)
        run("重命名") {
            do {
                try await SftpOps.rename(jump: jump, from: from, to: to)
            } catch {
                throw FilesOpError(SftpOps.reason(error, action: "重命名", path: entry.name))
            }
        }
    }

    func delete(_ entry: SftpOps.Entry) {
        guard let jump = server else { return }
        let target = SftpOps.join(path, entry.name)
        run("删除") {
            do {
                try await SftpOps.delete(jump: jump, path: target)
            } catch {
                throw FilesOpError(SftpOps.reason(error, action: "删除", path: entry.name))
            }
        }
    }

    /// 定位路径：支持绝对路径、`~/` 家目录、相对当前目录；交给服务器 realpath 校验一遍再跳
    func locate(_ input: String) {
        guard let jump = server else { return }
        let raw = input.trimmingCharacters(in: .whitespaces)
        if raw.isEmpty { return }
        if !opLabel.isEmpty {
            toast("正在\(opLabel)，稍等一下")
            return
        }
        let current = path
        opLabel = "定位路径"
        Task {
            do {
                var value = raw
                if raw == "~" || raw.hasPrefix("~/") {
                    let home = try await SftpOps.home(jump: jump)
                    value = home + raw.dropFirst()
                } else if !raw.hasPrefix("/") {
                    value = SftpOps.join(current, raw)
                }
                let resolved = try await SftpOps.resolve(jump: jump, path: value)
                opLabel = ""
                gotoDir(resolved)
            } catch {
                opLabel = ""
                toast(SftpOps.reason(error, action: "定位", path: raw), long: true)
            }
        }
    }

    // MARK: - 剪贴板

    func setClip(_ mode: Clip, _ entry: SftpOps.Entry) {
        clip = mode
        clipServer = serverId
        clipPaths = [SftpOps.join(path, entry.name)]
        clipNames = entry.name
        toast((mode == .cut ? "已剪切：" : "已复制：") + entry.name)
    }

    func clearClip() {
        clip = .none
        clipServer = ""
        clipPaths = []
        clipNames = ""
    }

    /// 粘贴到指定目录
    func paste(destDir: String) {
        guard clip != .none, let jump = server else { return }
        if clipServer != jump.id {
            toast("剪贴板里的东西在另一台服务器上，只能在同一台里粘贴")
            return
        }
        transferInto(PickRequest(cut: clip == .cut, paths: clipPaths,
                                 label: clipNames, startDir: destDir), destDir: destDir)
    }

    /// 复制 / 移动到目标目录。
    ///
    /// 目标已有同名项时：复制自动改名成「名字 (2)」，剪切直接报错 ——
    /// 剪切是「移动」，静默改个名会让用户以为东西没了，不如让他自己决定。
    func transferInto(_ request: PickRequest, destDir: String) {
        guard let jump = server else { return }
        run(request.cut ? "移动" : "复制") {
            for source in request.paths {
                // 剪到自己待的目录：没意义，跳过
                if request.cut && SftpOps.parent(source) == destDir { continue }
                let name = SftpOps.nameOf(source)
                var target = SftpOps.join(destDir, name)
                do {
                    if try await SftpOps.exists(jump: jump, path: target) {
                        if request.cut { throw FilesOpError("目标目录里已经有「\(name)」了") }
                        target = try await self.uniqueTarget(jump, destDir, name)
                    }
                    if request.cut {
                        try await SftpOps.move(jump: jump, from: source, to: target)
                    } else {
                        try await SftpOps.copy(jump: jump, from: source, to: target)
                    }
                } catch {
                    throw FilesOpError(SftpOps.reason(error,
                        action: request.cut ? "移动" : "复制", path: name))
                }
            }
            if request.cut {
                await MainActor.run { self.clearClip() }
            }
        }
    }

    /// 复制时避免覆盖：`foo.txt` → `foo (2).txt`
    private func uniqueTarget(_ jump: JumpServer, _ dir: String, _ name: String) async throws -> String {
        var stem = name
        var ext = ""
        if !name.hasPrefix("."), let index = name.lastIndex(of: "."), index != name.startIndex {
            stem = String(name[..<index])
            ext = String(name[index...])
        }
        for index in 2...99 {
            let candidate = SftpOps.join(dir, "\(stem) (\(index))\(ext)")
            if (try? await SftpOps.exists(jump: jump, path: candidate)) == false { return candidate }
        }
        throw FilesOpError("同名副本太多了（已经到 99 个）")
    }

    // MARK: - 小件

    func toast(_ text: String, long: Bool = false) {
        ABToastBus.shared.show(text, seconds: long ? 3.4 : 2.0)
    }
}

/// 业务侧自己抛的中文错误（原样给用户看）
struct FilesOpError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}
