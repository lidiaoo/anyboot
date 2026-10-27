import Foundation
import Combine
import Citadel
import NIOCore

/// 一条传输任务（可 Codable —— 直接就是 transfers.json 的元素）。
///
/// 字段与 Android `TransferManager.Task` 一一对应，除了 `sourceUri`：iOS 的
/// 文件选择器给的是安全作用域 URL，**跨进程重启就作废**，所以上传前先把源文件
/// 拷进 App 沙盒（`Documents/TransferSource/<taskId>/`），`localPath` 指向那份拷贝。
/// 代价是一次本地复制，换来的是「被杀之后还能续传」——比 Android 的 SAF 授权更可靠。
struct TransferTask: Identifiable, Codable, Hashable {

    enum Direction: Int, Codable {
        case upload = 1
        case download = 2
    }

    enum State: Int, Codable {
        case running = 0
        case done = 1
        case failed = 2
        /// 人主动取消：半截文件清掉
        case cancelled = 3
        /// 人主动暂停：半截文件留着，可「继续」
        case paused = 4
        /// 已被「继续」接替：只留个说法，不再有可点的动作
        case superseded = 5
    }

    var id: String = UUID().uuidString
    var direction: Direction
    var state: State = .running
    var startedAt: Date = Date()
    var finishedAt: Date?

    var serverId: String = ""
    var serverName: String = ""
    var name: String = ""
    var remoteDir: String = ""
    var remotePath: String = ""
    /// 下载 = 落盘路径；上传 = 沙盒里那份源文件拷贝的路径（续传要靠它）
    var localPath: String = ""
    /// 失败原因 / 补充说明
    var message: String = ""

    /// 总字节，-1 = 未知（进度条走不确定态，不假装知道）
    var total: Int64 = -1
    var transferred: Int64 = 0
    /// 本次启动时的起点（续传不为 0）—— 算速度要减掉，否则一续传就虚报
    var baseline: Int64 = 0

    /// 断点核对用：下载 = 起手时远端大小；上传 = 暂停时远端应有的大小
    var remoteSize: Int64 = -1
    /// 断点核对用（下载）：起手时远端文件的 mtime（秒）
    var remoteMTime: Int64 = -1

    /// 是被上次进程中断留下的（装回来的），UI 上要额外说明一句
    var interrupted = false
    /// 这次是接着上次的断点传的（不是重头来）
    var resumed = false

    var isRunning: Bool { state == .running }
    /// 还挂在清单上等着人操作（进行中 / 已暂停）—— 这两态都能点按钮
    var isPending: Bool { state == .running || state == .paused }

    /// 0…1；总量未知返回 0
    var fraction: Double {
        guard total > 0 else { return 0 }
        return min(1, max(0, Double(transferred) / Double(total)))
    }

    /// 平均速度（字节/秒）：只算**本次这一段**，续传带进来的那截不计
    var bytesPerSecond: Int64 {
        let end = finishedAt ?? Date()
        let span = end.timeIntervalSince(startedAt)
        guard span > 0, transferred > baseline else { return 0 }
        return Int64(Double(transferred - baseline) / span)
    }

    var directionLabel: String { direction == .upload ? "上传" : "下载" }
    /// 「已传 1.2 MB / 3.4 MB」
    var progressLabel: String {
        guard total > 0 else {
            return "已\(direction == .upload ? "传" : "下") " + TransferTask.humanBytes(transferred)
        }
        return "\(TransferTask.humanBytes(transferred)) / \(TransferTask.humanBytes(total))"
    }

    static func humanBytes(_ bytes: Int64) -> String {
        if bytes < 1024 { return "\(bytes) B" }
        let kb = Double(bytes) / 1024
        if kb < 1024 { return String(format: "%.1f KB", kb) }
        let mb = kb / 1024
        if mb < 1024 { return String(format: "%.1f MB", mb) }
        return String(format: "%.2f GB", mb / 1024)
    }
}

/// 停止意图：传输循环每块只看这一个字段（与 Android 的 `STOP_*` 同义）。
/// 用独立的锁盒子存，而不是回主线程问 —— 每块 30KB 就跨一次 actor 会把主线程压住。
private final class StopBox: @unchecked Sendable {

    enum Intent: Int {
        case none = 0
        /// 取消：半截文件清掉
        case cancel = 1
        /// 暂停：半截文件留着当续传起点
        case pause = 2
    }

    private let lock = NSLock()
    private var intent: Intent = .none

    var value: Intent {
        lock.lock()
        defer { lock.unlock() }
        return intent
    }

    func request(_ value: Intent) {
        lock.lock()
        intent = value
        lock.unlock()
    }
}

/// 传输中心：手机 ⇄ 服务器 的上传 / 下载任务注册表（单例持有，切页不丢）。
///
/// 与 Android `TransferManager` 同构：文件页只负责发起，进度、暂停、继续、取消、
/// 重试、结果都记在这里，传输页展示这份清单。
///
/// **为什么不用 Citadel 的 `readAll()` / 一次性写入**：那个是整份进内存，
/// 一个 2GB 的镜像直接 OOM。这里自己走 `SFTPFile.read(from:length:)` /
/// `write(_:at:)` 的定长分块循环，天然支持「从任意偏移接着传」，也顺带把
/// 暂停 / 取消做成块间检查一个标志位 —— 不用去断共享 SSH 会话。
@MainActor
final class TransferCenter: ObservableObject {

    static let shared = TransferCenter()

    @Published private(set) var tasks: [TransferTask] = [] {
        didSet { onChanged?() }
    }

    /// 任务有变化时的额外通知（服务器模块的传输状态条 / 传输页用它刷新 —— 对应 Android `TransferEvents`）
    var onChanged: (() -> Void)?

    private var stops: [String: StopBox] = [:]
    private var lastPersistAt = Date.distantPast
    private var restored = false

    private let storeURL: URL
    private let downloadsDir: URL
    private let sourceDir: URL

    private init() {
        let documents = FileManager.default
            .urls(for: .documentDirectory, in: .userDomainMask)[0]
        storeURL = documents.appendingPathComponent("transfers.json")
        downloadsDir = documents.appendingPathComponent("Downloads", isDirectory: true)
        sourceDir = documents.appendingPathComponent("TransferSource", isDirectory: true)
        try? FileManager.default.createDirectory(at: downloadsDir,
                                                 withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: sourceDir,
                                                 withIntermediateDirectories: true)
    }

    // MARK: - 落盘 / 装回

    /// 装回上次留下的任务清单 —— **App 启动后调一次**。
    ///
    /// 「上次还在传」的一律降成暂停：传输循环是进程内的东西，进程没了就没了，
    /// 那条任务只可能是停在半路。进度按磁盘上的半截文件长度纠正（比落盘那个数字准）。
    func restore() {
        guard !restored else { return }
        restored = true
        var loaded: [TransferTask] = []
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        let raw = (try? Data(contentsOf: storeURL)).flatMap {
            try? decoder.decode([TransferTask].self, from: $0)
        } ?? []
        for var task in raw {
            if task.state == .running {
                task.state = .paused
                task.transferred = max(task.transferred, Self.partialLength(task))
                task.baseline = task.transferred
                task.interrupted = true
                task.message = "上次没传完（App 关掉了），点「继续」接着传"
            }
            loaded.append(task)
        }
        // 已经在清单里的不覆盖（restore 之前可能已经手动发起过）
        let existing = Set(tasks.map(\.id))
        tasks.append(contentsOf: loaded.filter { !existing.contains($0.id) })
        tasks.sort { $0.startedAt < $1.startedAt }
    }

    /// 半截文件现在多长。只有下载能从磁盘问出来；上传的「进度」在远端，离线问不到
    private static func partialLength(_ task: TransferTask) -> Int64 {
        guard task.direction == .download, !task.localPath.isEmpty else { return 0 }
        let attrs = try? FileManager.default.attributesOfItem(atPath: task.localPath)
        return (attrs?[.size] as? NSNumber)?.int64Value ?? 0
    }

    private func persist() {
        // .atomic 本身就是「写临时文件再改名」：被杀在写一半不会留下坏 JSON
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .secondsSince1970
        guard let data = try? encoder.encode(tasks) else { return }
        do {
            try data.write(to: storeURL, options: .atomic)
            lastPersistAt = Date()
        } catch {
            // 存不上不影响这一轮传输，下次状态变化会再试
        }
    }

    private func persistProgress() {
        guard Date().timeIntervalSince(lastPersistAt) >= TransferLimits.persistInterval else { return }
        persist()
    }

    // MARK: - 查询

    /// 还在清单上的（进行中 + 已暂停；新的在前）
    var active: [TransferTask] {
        Array(tasks.filter(\.isPending).reversed())
    }

    /// 已收尾的（新的在前）
    var finished: [TransferTask] {
        Array(tasks.filter { !$0.isPending }.reversed())
    }

    var runningCount: Int {
        tasks.filter(\.isRunning).count
    }

    /// 同一方向、同一个远端路径上还挂着的任务 —— 用它挡住重复发起。
    /// 两条任务同时写同一个文件会互相踩（下载各自覆盖、上传各自追加），出来的文件是坏的。
    func pendingTask(direction: TransferTask.Direction, remotePath: String) -> TransferTask? {
        tasks.first { $0.isPending && $0.direction == direction && $0.remotePath == remotePath }
    }

    func task(_ id: String) -> TransferTask? {
        tasks.first { $0.id == id }
    }

    // MARK: - 发起

    /// 下载：把服务器上的文件拉到 App 的 Downloads 目录
    @discardableResult
    func startDownload(jump: JumpServer, remoteDir: String, name: String) -> TransferTask {
        let remotePath = Self.joinRemote(remoteDir, name)
        if let clashing = pendingTask(direction: .download, remotePath: remotePath) {
            return clashing
        }
        var task = TransferTask(direction: .download)
        task.serverId = jump.id
        task.serverName = jump.displayName
        task.name = name
        task.remoteDir = remoteDir
        task.remotePath = remotePath
        // 落盘位置提前算出来：进程要是被杀在「还在传」的状态上，
        // 装回时要靠 localPath 找到半截文件、把进度纠正过来
        task.localPath = downloadsDir.appendingPathComponent(name).path
        register(task)
        launch(task, jump: jump, resume: false)
        return task
    }

    /// 上传：把本地文件推到服务器目录。
    ///
    /// 先把源文件拷进沙盒（安全作用域 URL 重启后作废，拷贝才谈得上续传），
    /// 拷贝本身在传输循环里做，进度从 0 起、总长按拷贝后的文件算。
    @discardableResult
    func startUpload(jump: JumpServer, remoteDir: String, source: URL) -> TransferTask {
        let name = source.lastPathComponent
        let remotePath = Self.joinRemote(remoteDir, name)
        if let clashing = pendingTask(direction: .upload, remotePath: remotePath) {
            return clashing
        }
        var task = TransferTask(direction: .upload)
        task.serverId = jump.id
        task.serverName = jump.displayName
        task.name = name
        task.remoteDir = remoteDir
        task.remotePath = remotePath
        task.localPath = Self.sourcePath(taskId: task.id, name: name)
        register(task)
        launch(task, jump: jump, resume: false, source: source)
        return task
    }

    private static func sourcePath(taskId: String, name: String) -> String {
        let documents = FileManager.default
            .urls(for: .documentDirectory, in: .userDomainMask)[0]
        return documents
            .appendingPathComponent("TransferSource", isDirectory: true)
            .appendingPathComponent(taskId, isDirectory: true)
            .appendingPathComponent(name).path
    }

    private func register(_ task: TransferTask) {
        tasks.append(task)
        // 收尾的超过上限就把最老的丢掉
        let finishedIds = tasks.filter { !$0.isPending }.map(\.id)
        let overflow = finishedIds.count - TransferLimits.finishedLimit
        if overflow > 0 {
            let drop = Set(finishedIds.prefix(overflow))
            tasks.removeAll { drop.contains($0.id) }
        }
        persist()
    }

    // MARK: - 暂停 / 继续 / 取消 / 重试

    func pause(_ id: String) {
        guard let task = task(id), task.isRunning else { return }
        stops[id]?.request(.pause)
        // 循环下一块就会看到；这里先不回状态，等它自己收尾（才知道传到了哪里）
    }

    /// 继续：接着上次的断点传（**新建一条任务**，暂停那条留作历史）。返回 nil = 已发起
    func resume(_ id: String) -> String? {
        guard let previous = task(id) else { return "这条任务不在了" }
        guard previous.state == .paused else { return "这条任务不是暂停状态" }
        guard let jump = JumpStore.shared.jump(previous.serverId) else { return "服务器已删除，接不上了" }
        if previous.direction == .upload,
           !FileManager.default.fileExists(atPath: previous.localPath) {
            return "本地文件拷贝已被清理，只能重新上传"
        }
        if let clashing = pendingTask(direction: previous.direction,
                                      remotePath: previous.remotePath),
           clashing.id != previous.id {
            return "这条已经在传了"
        }

        var task = TransferTask(direction: previous.direction)
        task.serverId = previous.serverId
        task.serverName = previous.serverName
        task.name = previous.name
        task.remoteDir = previous.remoteDir
        task.remotePath = previous.remotePath
        task.localPath = previous.localPath
        task.total = previous.total
        task.transferred = previous.transferred
        task.baseline = previous.transferred
        // ⚠️ 这两个必须一起带走：断点核对拿它们当基线，落成 -1 的话
        // 「远端被换过」的条件永远不成立 —— 校验会变成一段死代码
        task.remoteSize = previous.remoteSize
        task.remoteMTime = previous.remoteMTime
        // 暂停那条就此收尾（只是留个说法），否则会挂成一行没有意义的「已暂停」
        finish(previous.id, state: .superseded,
               message: "已接续（本次从 \(TransferTask.humanBytes(previous.transferred)) 接着传）")
        register(task)
        launch(task, jump: jump, resume: true)
        return nil
    }

    /// 取消：进行中的让循环停手（半截文件清掉）；已暂停的当场清
    func cancel(_ id: String) {
        guard let task = task(id), task.isPending else { return }
        if task.state == .paused {
            // 已经停下来了，没有循环会来收尾 —— 这里自己处理半截文件
            Task {
                let cleaned = await discardPartial(task)
                let note: String
                if task.direction == .download {
                    note = cleaned ? "半截文件已删除" : "半截文件没能删掉"
                } else {
                    note = cleaned ? "半截文件已清理" : "半截文件可能还在服务器上"
                }
                let verb = task.direction == .upload ? "已传" : "已下"
                finish(id, state: .cancelled,
                       message: task.transferred > 0
                        ? "已取消（\(verb) \(TransferTask.humanBytes(task.transferred))，\(note)）"
                        : "已取消")
            }
            return
        }
        stops[id]?.request(.cancel)
    }

    /// 重试：按原样再发一次（**新建一条任务**）。重试是**从头来**；接着断点走要用 `resume`
    func retry(_ id: String) -> String? {
        guard let previous = task(id), !previous.isPending else { return "这条任务还挂着" }
        guard let jump = JumpStore.shared.jump(previous.serverId) else { return "服务器已删除，重试不了" }
        if pendingTask(direction: previous.direction, remotePath: previous.remotePath) != nil {
            return "这条已经在传了"
        }
        if previous.direction == .upload {
            guard FileManager.default.fileExists(atPath: previous.localPath) else {
                return "找不到原来的本地文件拷贝，只能重新上传"
            }
            var task = TransferTask(direction: .upload)
            task.serverId = previous.serverId
            task.serverName = previous.serverName
            task.name = previous.name
            task.remoteDir = previous.remoteDir
            task.remotePath = previous.remotePath
            task.localPath = previous.localPath
            register(task)
            launch(task, jump: jump, resume: false)
            return nil
        }
        startDownload(jump: jump, remoteDir: previous.remoteDir, name: previous.name)
        return nil
    }

    /// 清掉所有已收尾的记录（进行中 / 暂停的都不动）
    func clearFinished() {
        tasks.removeAll { !$0.isPending }
        persist()
    }

    #if DEBUG
    /// 调试种子：`-ABSeedTransfers YES` 启动参数造几条示例任务，只为看 UI（与 `DemoSeed` 同一套做法）。
    /// 注意：`simctl launch` 传的 `-Key Value` 会被 NSUserDefaults 的 argument 域吃掉，
    /// 所以这里读 UserDefaults 而不是 ProcessInfo（那边裸 flag 不一定留得下）。
    /// 不落盘 —— 下次正常启动就没了。
    func seedDemoIfRequested() {
        let enabled = UserDefaults.standard.bool(forKey: "ABSeedTransfers")
            || ProcessInfo.processInfo.arguments.contains("-ABSeedTransfers")
        guard enabled, tasks.isEmpty else { return }
        var running = TransferTask(direction: .download)
        running.name = "ubuntu-24.04-desktop-amd64.iso"
        running.serverName = "黑苹果"
        running.remotePath = "/Users/lidiaoo/Downloads/ubuntu-24.04-desktop-amd64.iso"
        running.total = 5_368_709_120
        running.transferred = 2_254_857_830
        running.state = .running
        running.startedAt = Date().addingTimeInterval(-42)

        var paused = TransferTask(direction: .upload)
        paused.name = "boot-backup-2026-10.tar.zst"
        paused.serverName = "书房工作站"
        paused.remotePath = "/backup/boot-backup-2026-10.tar.zst"
        paused.total = 1_073_741_824
        paused.transferred = 196_293_222
        paused.state = .paused
        paused.resumed = true
        paused.message = "已暂停（已传 187.2 MB），点「继续」接着传"

        var failed = TransferTask(direction: .download)
        failed.name = "grub.cfg"
        failed.serverName = "黑苹果"
        failed.remotePath = "/boot/grub/grub.cfg"
        failed.total = 8_192
        failed.transferred = 0
        failed.state = .failed
        failed.message = "下载失败：SFTPError.timeout"
        failed.startedAt = Date().addingTimeInterval(-300)

        var done = TransferTask(direction: .download)
        done.name = "anyboot.log"
        done.serverName = "黑苹果"
        done.remotePath = "/var/log/anyboot.log"
        done.total = 524_288
        done.transferred = 524_288
        done.state = .done
        done.message = "已下载到 /Documents/Downloads/anyboot.log"
        done.startedAt = Date().addingTimeInterval(-600)
        done.finishedAt = Date().addingTimeInterval(-590)

        tasks = [running, paused, failed, done]
    }
    #endif

    // MARK: - 状态更新（只从主线程调）

    private func applyProgress(id: String, transferred: Int64, total: Int64) {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { return }
        tasks[index].transferred = transferred
        if total > 0 { tasks[index].total = total }
        persistProgress()
        objectWillChange.send()
    }

    private func finish(_ id: String, state: TransferTask.State, message: String) {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { return }
        tasks[index].state = state
        tasks[index].message = message
        tasks[index].finishedAt = Date()
        stops[id] = nil
        persist()
        objectWillChange.send()
    }

    // MARK: - 干活

    private func launch(_ task: TransferTask, jump: JumpServer,
                        resume: Bool, source: URL? = nil) {
        let box = StopBox()
        stops[task.id] = box
        let id = task.id
        let direction = task.direction
        Task.detached(priority: .utility) {
            if direction == .upload {
                await Self.runUpload(taskId: id, jump: jump, box: box,
                                     resume: resume, source: source)
            } else {
                await Self.runDownload(taskId: id, jump: jump, box: box, resume: resume)
            }
        }
    }

    // MARK: 下载

    private nonisolated static func runDownload(taskId: String, jump: JumpServer,
                                    box: StopBox, resume: Bool) async {
        let center = await TransferCenter.shared
        guard let snapshot = await center.task(taskId) else { return }
        do {
            let ssh = try await SshSessionManager.shared.client(for: jump)
            let sftp = try await ssh.openSFTP()
            defer { Task { try? await sftp.close() } }

            let attrs = try await sftp.getAttributes(at: snapshot.remotePath)
            let remoteTotal = Int64(attrs.size ?? 0)
            let remoteMTime = attrs.accessModificationTime
                .map { Int64($0.modificationTime.timeIntervalSince1970) } ?? -1

            if !resume {
                // 起手把远端「大小 + mtime」记下来，之后续传要拿它核对文件有没有被换过
                await center.applyBaseline(id: taskId, total: remoteTotal,
                                           remoteSize: remoteTotal, remoteMTime: remoteMTime)
            } else {
                await center.applyBaseline(id: taskId, total: remoteTotal,
                                           remoteSize: nil, remoteMTime: nil)
            }

            // 续传认原来那个半截文件；否则（首传 / 半截文件没了）新开一个
            let targetPath = snapshot.localPath
            var already = resume ? partialSize(path: targetPath) : 0
            var note = ""
            let recordedSize = snapshot.remoteSize
            let recordedMTime = snapshot.remoteMTime
            if already > 0 && recordedSize > 0 && recordedMTime > 0
                && (remoteTotal != recordedSize || remoteMTime != recordedMTime) {
                // 远端变了：断点跟它已经对不上，追加只会拼出「看着完整、内容错位」的文件
                try? FileManager.default.removeItem(atPath: targetPath)
                already = 0
                note = "（服务器上的文件变过，已改为重新下载）"
                // 基线跟着换成新文件，否则这次重下再被暂停、再继续时还会拿旧值比一次
                await center.applyBaseline(id: taskId, total: remoteTotal,
                                           remoteSize: remoteTotal, remoteMTime: remoteMTime)
            }
            if already > 0 && remoteTotal > 0 && already > remoteTotal {
                // 兜底（远端大小/mtime 没拿到时走这条）
                try? FileManager.default.removeItem(atPath: targetPath)
                already = 0
                note = "（断点比远端文件还长，已改为重新下载）"
            }

            let handle = try openLocalForWrite(atPath: targetPath, offset: already)
            let file = try await sftp.openFile(filePath: snapshot.remotePath, flags: .read)

            var offset = already
            var lastReport = Date.distantPast
            var stopped = StopBox.Intent.none
            while true {
                stopped = box.value
                if stopped != .none { break }
                let buffer = try await file.read(from: UInt64(offset),
                                                 length: UInt32(TransferLimits.chunk))
                let count = buffer.readableBytes
                if count == 0 { break }
                let bytes = buffer.getBytes(at: buffer.readerIndex, length: count) ?? []
                try handle.write(contentsOf: Data(bytes))
                offset += Int64(count)
                let now = Date()
                let reachedEnd = remoteTotal > 0 && offset >= remoteTotal
                if now.timeIntervalSince(lastReport) >= TransferLimits.reportInterval || reachedEnd {
                    lastReport = now
                    await center.applyProgress(id: taskId, transferred: offset, total: remoteTotal)
                }
            }
            try? await file.close()
            try? handle.close()

            if stopped != .none {
                if stopped == .pause {
                    // 本地那半截留着 —— 它就是「继续」的起点
                    await center.finish(taskId, state: .paused,
                                        message: pausedMessage(transferred: offset, verb: "已下"))
                } else {
                    try? FileManager.default.removeItem(atPath: targetPath)
                    await center.finish(taskId, state: .cancelled,
                                        message: cancelledMessage(transferred: offset, verb: "已下",
                                                                  note: "半截文件已删除"))
                }
                return
            }
            await center.finish(taskId, state: .done,
                                message: "已下载到 \(targetPath)\(note)")
        } catch {
            let stopped = box.value
            if stopped != .none {
                if stopped == .pause {
                    let done = await center.transferredOf(taskId)
                    await center.finish(taskId, state: .paused,
                                        message: pausedMessage(transferred: done, verb: "已下"))
                } else {
                    try? FileManager.default.removeItem(atPath: snapshot.localPath)
                    await center.finish(taskId, state: .cancelled, message: "已取消")
                }
                return
            }
            await center.finish(taskId, state: .failed, message: reason(error, "下载失败"))
        }
    }

    // MARK: 上传

    private nonisolated static func runUpload(taskId: String, jump: JumpServer, box: StopBox,
                                  resume: Bool, source: URL?) async {
        let center = await TransferCenter.shared
        guard let snapshot = await center.task(taskId) else { return }
        do {
            // ① 源文件进沙盒（只有首传需要；续传时那份拷贝还在）
            let localPath = snapshot.localPath
            if !resume {
                try FileManager.default.createDirectory(
                    atPath: (localPath as NSString).deletingLastPathComponent,
                    withIntermediateDirectories: true)
                try? FileManager.default.removeItem(atPath: localPath)
                guard let source else {
                    throw TransferError.message("找不到本地文件")
                }
                try copyIntoContainer(from: source, to: localPath)
            }
            let localTotal = partialSize(path: localPath)
            guard localTotal > 0 else {
                throw TransferError.message("本地文件读不到（可能已被删除）")
            }
            await center.applyBaseline(id: taskId, total: localTotal,
                                       remoteSize: nil, remoteMTime: nil)

            let ssh = try await SshSessionManager.shared.client(for: jump)
            let sftp = try await ssh.openSFTP()
            defer { Task { try? await sftp.close() } }

            // 远端已经有多少 = 本地要跳过多少
            var startAt = resume ? (try? await remoteSize(sftp, snapshot.remotePath)) ?? 0 : 0
            var note = ""
            let recorded = snapshot.remoteSize
            if resume && recorded >= 0 && startAt > recorded {
                // 远端比我们记录的多 —— 有别人往里写过，接着追加会拼出一个内容错位的文件
                startAt = 0
                note = "（服务器上的文件比预期大，已改为重新上传）"
            }
            if startAt > localTotal { startAt = 0 }

            let flags: SFTPOpenFileFlags = startAt > 0
                ? [.write, .create]
                : [.write, .create, .truncate]
            let file = try await sftp.openFile(filePath: snapshot.remotePath, flags: flags)
            let handle = try openLocalForRead(atPath: localPath, offset: startAt)

            var offset = startAt
            var lastReport = Date.distantPast
            var stopped = StopBox.Intent.none
            while true {
                stopped = box.value
                if stopped != .none { break }
                guard let chunk = try handle.read(upToCount: TransferLimits.chunk), !chunk.isEmpty else {
                    break
                }
                try await file.write(ByteBuffer(bytes: chunk), at: UInt64(offset))
                offset += Int64(chunk.count)
                let now = Date()
                if now.timeIntervalSince(lastReport) >= TransferLimits.reportInterval {
                    lastReport = now
                    await center.applyProgress(id: taskId, transferred: offset, total: localTotal)
                }
            }
            try? await file.close()
            try? handle.close()

            if stopped != .none {
                if stopped == .pause {
                    // 远端那半截留着 —— 顺手记下「此刻远端应该有多大」，续传前拿它核对
                    await center.recordRemoteSize(id: taskId, size: offset)
                    await center.finish(taskId, state: .paused,
                                        message: pausedMessage(transferred: offset, verb: "已传"))
                } else {
                    // 取消：尽力清掉远端那半截（清不掉也只是留个残文件，不影响结论）
                    try? await sftp.remove(at: snapshot.remotePath)
                    await center.finish(taskId, state: .cancelled,
                                        message: cancelledMessage(transferred: offset, verb: "已传",
                                                                  note: "半截文件已清理"))
                }
                return
            }
            await center.applyProgress(id: taskId, transferred: localTotal, total: localTotal)
            await center.finish(taskId, state: .done,
                                message: "已上传到 \(snapshot.remotePath)\(note)")
        } catch {
            let stopped = box.value
            if stopped != .none {
                let done = await center.transferredOf(taskId)
                if stopped == .pause {
                    await center.recordRemoteSize(id: taskId, size: done)
                    await center.finish(taskId, state: .paused,
                                        message: pausedMessage(transferred: done, verb: "已传"))
                } else {
                    await center.finish(taskId, state: .cancelled, message: "已取消")
                }
                return
            }
            await center.finish(taskId, state: .failed, message: reason(error, "上传失败"))
        }
    }

    /// 安全作用域 URL → 沙盒拷贝。大文件走 256KB 缓冲，别整份进内存
    private nonisolated static func copyIntoContainer(from source: URL, to path: String) throws {
        let secured = source.startAccessingSecurityScopedResource()
        defer { if secured { source.stopAccessingSecurityScopedResource() } }
        guard let input = InputStream(url: source) else {
            throw TransferError.message("打不开本地文件")
        }
        input.open()
        defer { input.close() }
        FileManager.default.createFile(atPath: path, contents: nil)
        let output = try FileHandle(forWritingTo: URL(fileURLWithPath: path))
        defer { try? output.close() }
        var buffer = [UInt8](repeating: 0, count: 262_144)
        while input.hasBytesAvailable {
            let read = input.read(&buffer, maxLength: buffer.count)
            if read <= 0 { break }
            try output.write(contentsOf: Data(buffer[0..<read]))
        }
    }

    // MARK: - 半截文件

    /// 取消一条**已经暂停**的任务时清掉半截文件。返回是否确认清掉了
    private func discardPartial(_ task: TransferTask) async -> Bool {
        if task.direction == .download {
            guard !task.localPath.isEmpty else { return true }
            guard FileManager.default.fileExists(atPath: task.localPath) else { return true }
            try? FileManager.default.removeItem(atPath: task.localPath)
            return !FileManager.default.fileExists(atPath: task.localPath)
        }
        guard let jump = JumpStore.shared.jump(task.serverId) else { return false }
        do {
            let ssh = try await SshSessionManager.shared.client(for: jump)
            let sftp = try await ssh.openSFTP()
            defer { Task { try? await sftp.close() } }
            try await sftp.remove(at: task.remotePath)
            return true
        } catch {
            return false
        }
    }

    // MARK: - 主线程辅助（供后台循环回调）

    /// 一次性把总量 / 断点核对基线写进去；传 nil 表示这一项不动
    private func applyBaseline(id: String, total: Int64,
                               remoteSize: Int64?, remoteMTime: Int64?) {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { return }
        if total > 0 { tasks[index].total = total }
        if let remoteSize { tasks[index].remoteSize = remoteSize }
        if let remoteMTime { tasks[index].remoteMTime = remoteMTime }
        objectWillChange.send()
    }

    private func recordRemoteSize(id: String, size: Int64) {
        guard let index = tasks.firstIndex(where: { $0.id == id }) else { return }
        tasks[index].remoteSize = size
    }

    private func transferredOf(_ id: String) -> Int64 {
        task(id)?.transferred ?? 0
    }

    // MARK: - 小件

    private nonisolated static func joinRemote(_ dir: String, _ name: String) -> String {
        if dir.isEmpty || dir == "/" { return "/" + name }
        return dir.hasSuffix("/") ? dir + name : dir + "/" + name
    }

    private nonisolated static func partialSize(path: String) -> Int64 {
        let attrs = try? FileManager.default.attributesOfItem(atPath: path)
        return (attrs?[.size] as? NSNumber)?.int64Value ?? 0
    }

    private nonisolated static func remoteSize(_ sftp: SFTPClient, _ path: String) async throws -> Int64 {
        let attrs = try await sftp.getAttributes(at: path)
        return Int64(attrs.size ?? 0)
    }

    /// 下载落盘：已有半截就从断点续写，没有就新建
    private nonisolated static func openLocalForWrite(atPath path: String, offset: Int64) throws -> FileHandle {
        if offset == 0 {
            FileManager.default.createFile(atPath: path, contents: nil)
            let handle = try FileHandle(forWritingTo: URL(fileURLWithPath: path))
            try handle.truncate(atOffset: 0)
            return handle
        }
        let handle = try FileHandle(forWritingTo: URL(fileURLWithPath: path))
        try handle.seek(toOffset: UInt64(offset))
        return handle
    }

    /// 上传读源：从断点开始读
    private nonisolated static func openLocalForRead(atPath path: String, offset: Int64) throws -> FileHandle {
        let handle = try FileHandle(forReadingFrom: URL(fileURLWithPath: path))
        if offset > 0 { try handle.seek(toOffset: UInt64(offset)) }
        return handle
    }

    private nonisolated static func pausedMessage(transferred: Int64, verb: String) -> String {
        let size = transferred > 0
            ? "（\(verb) \(TransferTask.humanBytes(transferred))）" : ""
        return "已暂停\(size)，点「继续」接着传"
    }

    private nonisolated static func cancelledMessage(transferred: Int64, verb: String, note: String) -> String {
        transferred > 0
            ? "已取消（\(verb) \(TransferTask.humanBytes(transferred))，\(note)）"
            : "已取消"
    }

    private nonisolated static func reason(_ error: Error, _ fallback: String) -> String {
        if let error = error as? TransferError, case let .message(text) = error { return text }
        // 有 errorDescription 就用它：`"\(error)"` 会把自定义错误打成 `message("…")`
        let text = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        return text.isEmpty ? fallback : "\(fallback)：\(text)"
    }
}

enum TransferError: Error {
    case message(String)
}

/// 全局常量（放在类型外：`@MainActor` 类型里的 static 属性在 nonisolated 上下文读不到）
private enum TransferLimits {
    /// 分块大小：SFTP 的 write 内部按 32000 再切，这里对齐，省一次拆包
    static let chunk = 32000
    /// 进度刷新的最小间隔（秒）：SFTP 回调很密，太频会把主线程压住
    static let reportInterval: TimeInterval = 0.25
    /// 进度落盘的节流间隔（秒）：让「传到一半被杀」那条记录不至于是全 0
    static let persistInterval: TimeInterval = 3
    /// 已完成任务最多留这么多条
    static let finishedLimit = 30
}
