import Foundation
import Citadel
import Crypto
import NIOCore
import NIOSSH
import Network

/// SSH 会话栈（与 Android 端 SshSessionManager 同构）：
/// 一个跳板机一条 `SSHClient`，隧道（direct-tcpip）与终端（PTY）复用。
///
/// 依赖 Citadel（SwiftPM: https://github.com/orlandos-nl/Citadel ）。
/// API 按 2026-10 的 main 分支校对（connect / createDirectTCPIPChannel / openSFTP /
/// withPTY / SSHHostKeyValidator.custom）—— 若以后升级出现小幅漂移，先看这里。
final class SshSessionManager {

    static let shared = SshSessionManager()

    /// TOFU 弹窗：由 UI 层注入（主线程 Alert）。参数 (标题, 正文) → 是否信任。
    /// 同步等待用户点完才继续握手。
    var confirmHandler: ((String, String) async -> Bool)?

    private struct Entry {
        var client: SSHClient?
        var consecutiveFails = 0
        var nextAttemptAt: Date = .distantPast
    }

    private var entries: [String: Entry] = [:]
    /// jumpId|destHost:destPort → 本地监听端口
    private var tunnels: [String: Int] = [:]
    private var listeners: [String: NWListener] = [:]
    private let lock = NSLock()

    private func entry(_ id: String) -> Entry {
        lock.lock()
        defer { lock.unlock() }
        if let existing = entries[id] {
            return existing
        }
        let fresh = Entry()
        entries[id] = fresh
        return fresh
    }

    private func setEntry(_ id: String, _ value: Entry) {
        lock.lock()
        defer { lock.unlock() }
        entries[id] = value
    }

    // MARK: - 连接

    func client(for jump: JumpServer) async throws -> SSHClient {
        var state = entry(jump.id)
        if let client = state.client, client.isConnected {
            return client
        }
        if client(forId: jump.id) != nil, client(forId: jump.id)!.isConnected {
            return client(forId: jump.id)!
        }
        let now = Date()
        if now < state.nextAttemptAt {
            let wait = Int(state.nextAttemptAt.timeIntervalSince(now)) + 1
            throw SshError.backoff(wait)
        }
        do {
            let client = try await connect(jump)
            state.client = client
            state.consecutiveFails = 0
            state.nextAttemptAt = .distantPast
            setEntry(jump.id, state)
            return client
        } catch {
            state.consecutiveFails += 1
            let delay = min(60.0, pow(2.0, Double(min(state.consecutiveFails, 6))))
            state.nextAttemptAt = Date().addingTimeInterval(delay)
            setEntry(jump.id, state)
            throw error
        }
    }

    private func client(forId id: String) -> SSHClient? {
        lock.lock()
        defer { lock.unlock() }
        return entries[id]?.client
    }

    private func connect(_ jump: JumpServer) async throws -> SSHClient {
        let authenticationMethod: SSHAuthenticationMethod
        if jump.authType == JumpServer.authKey {
            guard let key = JumpStore.shared.key(jump.keyId) else {
                throw SshError.message("找不到选中的密钥，请到「密钥」页检查")
            }
            let pem = KeyVault.load(account: key.privateRef)
            guard !pem.isEmpty else {
                throw SshError.message("私钥读取失败（Keychain 不可用时需重新导入）")
            }
            let passphrase = KeyVault.load(account: jump.keyPassphraseRef)
            if pem.contains("BEGIN OPENSSH PRIVATE KEY"),
               pem.contains("ssh-ed25519") || key.algorithm == "ssh-ed25519" {
                let privateKey = try Curve25519.Signing.PrivateKey(sshEd25519: pem,
                                                                   decryptionKey: passphrase.isEmpty
                                                                   ? nil : Data(passphrase.utf8))
                authenticationMethod = .ed25519(username: jump.username.isEmpty
                        ? "root" : jump.username, privateKey: privateKey)
            } else {
                // OpenSSH 格式 RSA
                let privateKey = try Insecure.RSA.PrivateKey(sshRsa: pem)
                authenticationMethod = .rsa(username: jump.username.isEmpty
                        ? "root" : jump.username, privateKey: privateKey)
            }
        } else {
            let password = KeyVault.load(account: jump.passwordRef)
            authenticationMethod = .passwordBased(username: jump.username.isEmpty
                    ? "root" : jump.username, password: password)
        }

        let hostKey = jump.host
        let validator = makeTofuValidator(hostKey: hostKey)
        return try await SSHClient.connect(
            host: jump.hostname,
            port: jump.port,
            authenticationMethod: authenticationMethod,
            hostKeyValidator: validator,
            reconnect: .never
        )
    }

    /// TOFU（首连弹窗确认、存指纹、对不上拒连）。
    /// Citadel 的自定义校验 = `SSHHostKeyValidator.custom(NIOSSHClientServerAuthenticationDelegate)`。
    private func makeTofuValidator(hostKey: String) -> SSHHostKeyValidator {
        .custom(TofuDelegate(hostPort: hostKey, manager: self))
    }

    private final class TofuDelegate: NIOSSHClientServerAuthenticationDelegate, @unchecked Sendable {
        let hostPort: String
        weak var manager: SshSessionManager?

        init(hostPort: String, manager: SshSessionManager) {
            self.hostPort = hostPort
            self.manager = manager
        }

        func validateHostKey(hostKey: NIOSSHPublicKey,
                             validationCompletePromise: EventLoopPromise<Void>) {
            guard let manager else {
                validationCompletePromise.fail(SshError.message("会话管理器已释放"))
                return
            }
            // 指纹：OpenSSH 口径 = SHA256(wire-format key blob，带算法名头)。
            // 当前 NIOSSH API：`write(to:)` 把带头的公钥 blob 写进 ByteBuffer。
            var keyBuffer = ByteBufferAllocator().buffer(capacity: 1024)
            hostKey.write(to: &keyBuffer)
            let keyBytes = keyBuffer.readBytes(length: keyBuffer.readableBytes) ?? []
            let fingerprint = KeyVault.sha256Fingerprint(Data(keyBytes))
            let stored = UserDefaults.standard.string(forKey: "tofu_\(hostPort)") ?? ""
            let task = Task {
                if stored.isEmpty {
                    let message = "首次连接 \(hostPort)\n\n主机指纹（SHA256）：\(fingerprint)"
                            + "\n\n确认是你要连的服务器吗？（确认后记住，不再询问）"
                    guard await manager.confirmHandler?("确认主机指纹", message) == true else {
                        validationCompletePromise.fail(SshError.message("用户取消了主机指纹确认"))
                        return
                    }
                    UserDefaults.standard.set(fingerprint, forKey: "tofu_\(hostPort)")
                } else if stored != fingerprint {
                    validationCompletePromise.fail(SshError.message(
                            "主机指纹变了！可能服务器重装了系统，也可能有人在中间。—— 到服务器详情里「重置信任」后可重试"))
                    return
                }
                validationCompletePromise.succeed(())
            }
            _ = task   // promise 自己保证生命周期
        }
    }

    // MARK: - 隧道（本地端口转发）

    /// 打开（或复用）一条到 destHost:destPort 的本地转发，返回本地端口。
    ///
    /// 实现：NWListener 监听 127.0.0.1:0，每个入站连接开一条
    /// `createDirectTCPIPChannel`，字节在 NWConnection 与 SSH 通道间泵。
    func openTunnel(jump: JumpServer, destHost: String, destPort: Int) async throws -> Int {
        _ = try await client(for: jump)   // 先确保会话
        let key = "\(jump.id)|\(destHost):\(destPort)"
        if let existing = tunnels[key], let listener = listeners[key], listener.state == .ready {
            return existing
        }
        let client = try await client(for: jump)

        // 起本地监听（127.0.0.1:0 = 系统挑空闲端口，不暴露）
        let params = NWParameters.tcp
        params.allowLocalEndpointReuse = true
        guard let address = try? SocketAddress(ipAddress: "127.0.0.1", port: 0),
              let listener = try? NWListener(using: params, on: NWEndpoint.Port(rawValue: 0)!) else {
            throw SshError.message("本地端口监听失败")
        }
        let portBox = PortBox()
        listener.stateUpdateHandler = { state in
            if case .ready = state, let port = listener.port?.rawValue {
                portBox.set(Int(port))
            }
        }
        listener.newConnectionHandler = { [weak self] inbound in
            guard let self else { return }
            Task {
                do {
                    try await self.pump(inbound: inbound, client: client,
                                        destHost: destHost, destPort: destPort)
                } catch {
                    inbound.cancel()
                }
            }
        }
        listener.start(queue: DispatchQueue(label: "anyboot-tunnel-\(jump.id)"))
        // 等端口就绪（一般毫秒级）
        for _ in 0..<50 where !portBox.isSet {
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        guard let port = portBox.get() else {
            throw SshError.message("本地端口未能就绪")
        }
        tunnels[key] = port
        listeners[key] = listener
        return port
    }

    /// 「经由跳板机」的电脑：把内网地址解析成本地隧道地址（127.0.0.1:port）
    func tunnelAddress(for profile: ServerProfile) async throws -> String {
        guard let jump = JumpStore.shared.jump(profile.jumpServerId) else {
            throw SshError.message("跳板机不存在（可能已删除）")
        }
        let target = profile.host
        let destHost: String
        var destPort = ServerProfile.defaultPort
        if let index = target.lastIndex(of: ":") {
            destHost = String(target[..<index])
            destPort = Int(target[target.index(after: index)...]) ?? ServerProfile.defaultPort
        } else {
            destHost = target
        }
        let port = try await openTunnel(jump: jump, destHost: destHost, destPort: destPort)
        return "127.0.0.1:\(port)"
    }

    /// 一个入站连接 ↔ 一条 direct-tcpip 通道
    private func pump(inbound: NWConnection, client: SSHClient,
                      destHost: String, destPort: Int) async throws {
        let originator = try SocketAddress(ipAddress: "127.0.0.1", port: 22)
        let bridge = TunnelBridge(connection: inbound)
        let channel = try await client.createDirectTCPIPChannel(
            using: SSHChannelType.DirectTCPIP(
                targetHost: destHost,
                targetPort: destPort,
                originatorAddress: originator
            )
        ) { channel in
            channel.pipeline.addHandler(bridge)
        }
        inbound.stateUpdateHandler = { [weak bridge] state in
            if case .failed = state, let bridge {
                bridge.closeRemote()
            }
            if case .cancelled = state, let bridge {
                bridge.closeRemote()
            }
        }
        inbound.start(queue: DispatchQueue(label: "anyboot-tunnel-conn"))
        _ = channel   // 通道随 bridge 的 closeFuture 一起关
    }

    // MARK: - 关闭

    func shutdown(jumpId: String) {
        lock.lock()
        let state = entries.removeValue(forKey: jumpId)
        let client = state?.client
        tunnels = tunnels.filter { !$0.key.hasPrefix("\(jumpId)|") }
        let dead = listeners.filter { $0.key.hasPrefix("\(jumpId)|") }
        listeners = listeners.filter { !$0.key.hasPrefix("\(jumpId)|") }
        lock.unlock()
        dead.values.forEach { $0.cancel() }
        Task { try? await client?.close() }
    }

    func isUp(_ jumpId: String) -> Bool {
        client(forId: jumpId)?.isConnected ?? false
    }

    // MARK: - 终端

    /// 打开 PTY 会话。回调块里持有 ttyStdinWriter / 输出流；
    /// 返回的句柄用来写键盘字节、等待关闭。
    ///
    /// Citadel 的 withPTY 闭包签名：`(ttyOutput: AsyncSequence<ByteBuffer>, ttyStdinWriter)`。
    /// 不同版本 ttyStdinWriter 的写入形态略有差异 —— 这里的 `bind` 只收
    /// 「写一段键盘字节」的闭包，把形态差异圈在这一处。
    func openShell(jump: JumpServer, columns: Int, rows: Int,
                   onOutput: @escaping (Data) -> Void,
                   onClosed: @escaping (String) -> Void) async throws -> ShellHandle {
        let client = try await client(for: jump)
        let stdinBox = StdinBox()
        let done = ClosedBox()
        let request = SSHChannelRequestEvent.PseudoTerminalRequest(
            wantReply: true,
            term: "xterm-256color",
            terminalCharacterWidth: columns,
            terminalRowHeight: rows,
            terminalPixelWidth: 0,
            terminalPixelHeight: 0,
            terminalModes: .init([:])
        )
        // 当前 Citadel main：withPTY 返回 Void；输出序列元素是 ExecCommandOutput
        //（.stdout / .stderr 各带 ByteBuffer），stdin 是 TTYStdinWriter.write(ByteBuffer)。
        try await client.withPTY(request) { ttyOutput, ttyStdinWriter in
            // 启动路径 / 默认 shell（与 PC 端 hub_term.rs 同一套语义）：往登录
            // shell 的 stdin 写一行命令，像手敲一样执行。目录打不通时把报错
            // 留在终端里，`;` 继续往下走，绝不因填错路径断会话。
            let startup = Self.terminalStartupLine(for: jump)
            if !startup.isEmpty {
                var line = ByteBufferAllocator().buffer(capacity: startup.utf8.count + 1)
                line.writeString(startup + "\n")
                try? await ttyStdinWriter.write(line)
            }
            stdinBox.bind { data in
                var buffer = ByteBufferAllocator().buffer(capacity: data.count)
                buffer.writeBytes(data)
                try await ttyStdinWriter.write(buffer)
            }
            // 读远端输出直到通道关闭（stdout / stderr 都喂给终端）
            do {
                for try await output in ttyOutput {
                    switch output {
                    case .stdout(let buffer), .stderr(let buffer):
                        var copy = buffer
                        if let bytes = copy.readBytes(length: copy.readableBytes) {
                            onOutput(Data(bytes))
                        }
                    }
                }
            } catch {
                // 通道断开：正常结束
            }
            done.close()
            onClosed("连接已关闭")
        }
        return ShellHandle(stdinBox: stdinBox, channel: nil, done: done)
    }

    /// 组终端启动行（PC 端 term_remote_command 的 nio-ssh 版）：
    /// - 路径与 shell 都空 → 空串（什么都不写，纯登录）；
    /// - 只配路径 → `cd '<path>'`（PTY 通道本来就是交互登录 shell，不用 exec）；
    /// - 配了 shell → `exec <shell>`。预设（bash/sh/zsh/fish）带 `-l` 对齐
    ///   login-shell 语义并单引号包住；自定义内容原样 exec（tmux 之类自己写完整命令）。
    static func terminalStartupLine(for jump: JumpServer) -> String {
        let path = (jump.termPath ?? "").trimmingCharacters(in: .whitespaces)
        let shell = (jump.termShell ?? "").trimmingCharacters(in: .whitespaces)
        var parts: [String] = []
        if !path.isEmpty {
            parts.append("cd " + shQuote(path))
        }
        if !shell.isEmpty {
            let exec: String
            switch shell {
            case "/bin/bash", "/bin/sh", "/bin/zsh", "/bin/fish":
                exec = "exec " + shQuote(shell) + " -l"
            default:
                exec = "exec " + shell
            }
            parts.append(exec)
        }
        return parts.joined(separator: "; ")
    }

    /// 远程命令里的安全引号：单引号包住，内嵌 `'` 按 `'\''` 断开再接。
    private static func shQuote(_ value: String) -> String {
        "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    enum SshError: Error, LocalizedError {
        case backoff(Int)
        case message(String)

        var errorDescription: String? {
            switch self {
            case .backoff(let seconds):
                return "连接失败过，\(seconds) 秒后再试（自动退避）"
            case .message(let text):
                return text
            }
        }
    }
}

/// 键盘写字节的小盒子：bind 的是「写一段键盘字节」的 async 闭包，
/// 具体的 Citadel 写入器形态差异圈在 bind 的适配层。
final class StdinBox: @unchecked Sendable {
    private var writer: ((Data) async throws -> Void)?
    private let lock = NSLock()

    func bind(_ raw: @escaping (Data) async throws -> Void) {
        lock.lock()
        writer = raw
        lock.unlock()
    }

    func write(_ data: Data) {
        lock.lock()
        let raw = writer
        lock.unlock()
        guard let raw else { return }
        Task { try? await raw(data) }
    }
}

/// 终端会话句柄
final class ShellHandle {
    let stdinBox: StdinBox
    let channel: Channel?
    let done: ClosedBox

    init(stdinBox: StdinBox, channel: Channel?, done: ClosedBox) {
        self.stdinBox = stdinBox
        self.channel = channel
        self.done = done
    }

    func write(_ data: Data) {
        stdinBox.write(data)
    }

    /// 终端窗口变了（xterm.js fit 之后）
    func resize(columns: Int, rows: Int) {
        // NIOSSH 的窗口调整随数据侧报文发；Citadel 的 withPTY 不直接暴露 resize，
        // 先保持初始尺寸 —— vim/htop 在 80x24 起步后自适应用户拖动即可。
        // TODO(iOS)：升级到暴露 channelWindowUpdate 的版本后接上。
    }
}

final class ClosedBox: @unchecked Sendable {
    private var closed = false

    func close() {
        closed = true
    }

    var isClosed: Bool { closed }
}

/// 隧道端口的中转盒（回调线程 → async）
final class PortBox: @unchecked Sendable {
    private var value: Int?
    private let lock = NSLock()

    var isSet: Bool {
        lock.lock()
        defer { lock.unlock() }
        return value != nil
    }

    func set(_ port: Int) {
        lock.lock()
        value = port
        lock.unlock()
    }

    func get() -> Int? {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

/// NWConnection ↔ SSH direct-tcpip 通道的双向桥（ChannelHandler）
final class TunnelBridge: ChannelDuplexHandler, @unchecked Sendable {
    typealias InboundIn = ByteBuffer
    typealias InboundOut = ByteBuffer   // 不写这个协议默认 Never，fireChannelRead 会报型不匹配
    typealias OutboundIn = ByteBuffer
    typealias OutboundOut = ByteBuffer

    private let connection: NWConnection
    private let queue = DispatchQueue(label: "anyboot-bridge")
    private weak var context: ChannelHandlerContext?
    private var remoteClosed = false

    init(connection: NWConnection) {
        self.connection = connection
    }

    func handlerAdded(context: ChannelHandlerContext) {
        self.context = context
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, _, error in
            self?.onReceive(data: data, error: error)
        }
    }

    private func onReceive(data: Data?, error: NWError?) {
        guard !remoteClosed else { return }
        if let data, !data.isEmpty {
            context?.eventLoop.execute { [weak self] in
                guard let self, let context = self.context else { return }
                var buffer = context.channel.allocator.buffer(capacity: data.count)
                buffer.writeBytes(data)
                context.fireChannelRead(wrapInboundOut(buffer))
            }
            connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, _, error in
                self?.onReceive(data: data, error: error)
            }
        }
        if error != nil {
            closeRemote()
            context?.eventLoop.execute { [weak self] in
                self?.context?.close(mode: .input, promise: nil)
            }
        }
    }

    func closeRemote() {
        remoteClosed = true
    }

    func channelRead(context: ChannelHandlerContext, data: NIOAny) {
        let buffer = unwrapInboundIn(data)
        var copy = buffer
        if let bytes = copy.readBytes(length: copy.readableBytes) {
            connection.send(content: Data(bytes), completion: .contentProcessed { _ in })
        }
    }

    func write(context: ChannelHandlerContext, data: NIOAny, promise: EventLoopPromise<Void>?) {
        let buffer = unwrapOutboundIn(data)
        var copy = buffer
        if let bytes = copy.readBytes(length: copy.readableBytes) {
            connection.send(content: Data(bytes), completion: .contentProcessed { _ in
                promise?.succeed(())
            })
        } else {
            promise?.succeed(())
        }
    }

    func handlerRemoved(context: ChannelHandlerContext) {
        connection.cancel()
    }
}
