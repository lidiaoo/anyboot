import Foundation
import Combine

/// 一个终端会话：SSH PTY 或 Telnet socket 的统一外壳 + 注册表
///（与 Android 端 TerminalSession / TerminalRegistry 同构）。
///
/// 下行先攒 backlog（封顶 256KB），终端页挂上后一次喂给 xterm.js；
/// 会话由静态注册表持有，页面销毁不关会话。
final class TerminalSessionRef: ObservableObject, Identifiable {

    let server: JumpServer
    let startedAt = Date()

    init(server: JumpServer) {
        self.server = server
    }

    private var shell: ShellHandle?
    private var telnet: TelnetSession?
    private var backlog = Data()
    private static let backlogCap = 256 * 1024
    private var attached = false
    private(set) var closed = false
    private(set) var closeReason = ""
    private let lock = NSLock()

    /// 实时下行（终端页订阅）
    var onOutput: ((Data) -> Void)?
    var onClosed: ((String) -> Void)?

    // MARK: - 建立

    func startSsh() async throws {
        let manager = SshSessionManager.shared
        let handle = try await manager.openShell(
            jump: server,
            columns: 80, rows: 24,
            onOutput: { [weak self] data in self?.deliver(data) },
            onClosed: { [weak self] reason in self?.close(reason) })
        shell = handle
    }

    func startTelnet() throws {
        let session = TelnetSession()
        session.onOutput = { [weak self] data in self?.deliver(data) }
        session.onClosed = { [weak self] reason in self?.close(reason) }
        try session.start(jump: server)
        telnet = session
    }

    // MARK: - 数据

    /// 键盘 → 远端
    func write(_ data: Data) {
        lock.lock()
        let shell = self.shell
        let telnet = self.telnet
        lock.unlock()
        if let shell {
            shell.write(data)
        } else if let telnet {
            telnet.write(data)
        }
    }

    /// 挂上界面并取走积攒的回显
    func attachAndDrain() -> Data {
        lock.lock()
        defer { lock.unlock() }
        attached = true
        let data = backlog
        backlog = Data()
        return data
    }

    func detach() {
        lock.lock()
        defer { lock.unlock() }
        attached = false
    }

    /// 终端窗口变化（SSH 侧 CITADEL 暂不透传 resize，见 ShellHandle.resize）
    func resize(columns: Int, rows: Int) {
        lock.lock()
        let shell = self.shell
        lock.unlock()
        shell?.resize(columns: columns, rows: rows)
    }

    func close(_ reason: String) {
        lock.lock()
        guard !closed else {
            lock.unlock()
            return
        }
        closed = true
        closeReason = reason
        attached = false
        let shell = self.shell
        let telnet = self.telnet
        lock.unlock()
        telnet?.close(reason)
        // 注意：不关 SSHClient —— 隧道（WebView/心跳）与终端共用一条会话；
        // PTY 通道随 withPTY 闭包返回自然结束。
        let callback = onClosed
        let serverId = server.id
        DispatchQueue.main.async {
            TerminalSessionsRegistry.remove(serverId)
            // 历史落一条
            JumpStore.shared.appendHistory(.init(serverId: serverId,
                                                 serverName: self.server.name,
                                                 startedAt: self.startedAt,
                                                 endedAt: Date()))
            callback?(reason)
        }
    }

    private func deliver(_ data: Data) {
        lock.lock()
        if backlog.count < Self.backlogCap {
            backlog.append(data)
        }
        let attached = self.attached
        lock.unlock()
        if attached {
            onOutput?(data)
        }
    }
}

/// 活着的多会话注册表（静态，页面重建不丢）
enum TerminalSessionsRegistry {

    static func setObserver(_ callback: (() -> Void)?) {
        observer = callback
    }

    static func get(_ jumpId: String) -> TerminalSessionRef? {
        sessions.first { $0.server.id == jumpId && !$0.closed }
    }

    static func put(_ session: TerminalSessionRef) {
        sessions.append(session)
        observer?()
    }

    static func remove(_ jumpId: String) {
        sessions.removeAll { $0.server.id == jumpId }
        observer?()
    }

    static var all: [TerminalSessionRef] {
        sessions.filter { !$0.closed }
    }

    static func kill(_ session: TerminalSessionRef) {
        session.close("用户结束")
        remove(session.server.id)
    }

    static private(set) var sessions: [TerminalSessionRef] = []
    static var observer: (() -> Void)?
}
