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

    /// TOFU 弹窗：参数 (标题, 正文) → 是否信任，挂起等用户点完才继续握手。
    ///
    /// ⚠️ 这里**必须有默认实现**。旧版把注入留给「UI 层谁用谁设」，结果谁都没设，
    /// 首连时 `confirmHandler?()` 对 nil 直接返回 nil → 一律按「用户取消」处理，
    /// 症状就是「一上来就提示用户取消了主机确认」，弹窗根本没出来。
    var confirmHandler: ((String, String) async -> Bool)?

    private init() {
        confirmHandler = { title, message in
            await ABConfirmBus.shared.confirm(title: title, message: message,
                                             confirmTitle: "信任并连接")
        }
    }

    private struct Entry {
        var client: SSHClient?
        var consecutiveFails = 0
        var nextAttemptAt: Date = .distantPast
    }

    private var entries: [String: Entry] = [:]
    /// jumpId|destHost:destPort → 一条本地转发（端口 + 监听 + 绑定的 SSH 会话）
    private struct Tunnel {
        let port: Int
        let listener: NWListener
        let clientID: ObjectIdentifier
    }
    private var tunnels: [String: Tunnel] = [:]
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
            NSLog("[ABSSH] 会话建立 jump=\(jump.id) client=\(ObjectIdentifier(client).hashValue)")
            state.client = client
            state.consecutiveFails = 0
            state.nextAttemptAt = .distantPast
            setEntry(jump.id, state)
            // 会话换了：旧隧道全都绑在死连接上，立刻作废（下次 openTunnel 会重建）
            invalidateTunnels(jumpId: jump.id)
            return client
        } catch {
            // 主机指纹没确认 / 变了：这是用户选择，不是网络故障 —— 别退避，
            // 否则点「取消」之后 2 秒内再点连接会先撞「X 秒后再试」（Android 亦如此）
            if let ssh = error as? SshError, case .hostKey = ssh { throw error }
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

    // MARK: - 应急密码 / 退避重置（对应 Android `needsManualPassword` / `setManualPassword` / `resetBackoff`）

    /// 手动输入的密码（**只在本次进程有效**，不落 Keychain）—— 跳板机 id → 密码
    private var manualPasswords: [String: String] = [:]

    /// 密码认证但本机没存上密码：连接前要先弹输入框
    func needsManualPassword(_ jump: JumpServer) -> Bool {
        guard jump.isSsh, jump.authType == JumpServer.authPassword else { return false }
        lock.lock()
        let manual = manualPasswords[jump.id] ?? ""
        lock.unlock()
        if !manual.isEmpty { return false }
        return KeyVault.load(account: jump.passwordRef).isEmpty
    }

    func setManualPassword(_ jumpId: String, _ password: String) {
        lock.lock()
        manualPasswords[jumpId] = password
        lock.unlock()
    }

    /// 改完档案（尤其是改了密码 / 用户名）立刻能重连，别被退避挡住
    func resetBackoff(_ jumpId: String) {
        var state = entry(jumpId)
        state.consecutiveFails = 0
        state.nextAttemptAt = .distantPast
        setEntry(jumpId, state)
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
            lock.lock()
            let manual = manualPasswords[jump.id] ?? ""
            lock.unlock()
            let password = manual.isEmpty ? KeyVault.load(account: jump.passwordRef) : manual
            authenticationMethod = .passwordBased(username: jump.username.isEmpty
                    ? "root" : jump.username, password: password)
        }

        let validator = makeTofuValidator(jump: jump)
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
    private func makeTofuValidator(jump: JumpServer) -> SSHHostKeyValidator {
        // 指纹按 `host`（host[:port] 原样）记，跟 Android `JumpStore.trustedHostKey(server.host)` 同键；
        // 弹窗正文用 `user@host`（Android `server.addressLabel()`），别把两个混用。
        .custom(TofuDelegate(storeKey: jump.host, label: jump.addressLabel, manager: self))
    }

    private final class TofuDelegate: NIOSSHClientServerAuthenticationDelegate, @unchecked Sendable {
        let storeKey: String
        let label: String
        weak var manager: SshSessionManager?

        init(storeKey: String, label: String, manager: SshSessionManager) {
            self.storeKey = storeKey
            self.label = label
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
            let storeKey = self.storeKey
            let stored = UserDefaults.standard.string(forKey: "tofu_\(storeKey)") ?? ""
            let task = Task {
                if stored.isEmpty {
                    guard let confirm = manager.confirmHandler else {
                        // 没有确认框就没法问 —— 明说原因，别再谎报「用户取消」
                        NSLog("[ABSSH] confirmHandler 未就绪，主机指纹确认失败 host=\(storeKey)")
                        validationCompletePromise.fail(SshError.hostKey(
                                "确认框没弹出来（界面未就绪），请重试"))
                        return
                    }
                    let message = "首次连接 \(label)\n\n主机指纹（SHA256）：\(fingerprint)"
                            + "\n\n确认是你要连的服务器吗？（确认后记住，不再询问）"
                    guard await confirm("确认主机指纹", message) else {
                        NSLog("[ABSSH] 用户拒绝主机指纹 host=\(storeKey)")
                        validationCompletePromise.fail(SshError.hostKey("已取消：主机指纹没有确认"))
                        return
                    }
                    UserDefaults.standard.set(fingerprint, forKey: "tofu_\(storeKey)")
                } else if stored != fingerprint {
                    NSLog("[ABSSH] 主机指纹不匹配 host=\(storeKey) 存=\(stored) 收=\(fingerprint)")
                    validationCompletePromise.fail(SshError.hostKey(
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
    ///
    /// ⚠️ 缓存**必须绑定会话身份**：`client(for:)` 在连接断开后会换新 SSHClient，
    /// 若只看 `listener.state == .ready` 就复用，旧隧道会把所有转发泵进已死的旧连接
    /// —— 症状是「终端能连，走跳板的探测/通信全静默失败，服务器根本收不到请求」
    /// （iOS 进后台、切网、服务器重启都会触发换会话）。
    func openTunnel(jump: JumpServer, destHost: String, destPort: Int) async throws -> Int {
        let client = try await client(for: jump)   // 现在活着的会话
        let clientID = ObjectIdentifier(client)
        NSLog("[ABSSH] openTunnel \(destHost):\(destPort) client=\(clientID.hashValue) connected=\(client.isConnected)")
        let key = "\(jump.id)|\(destHost):\(destPort)"
        lock.lock()
        let cached = tunnels[key]
        lock.unlock()
        if let cached, cached.clientID == clientID, cached.listener.state == .ready {
            return cached.port
        }
        // 旧隧道绑的是别的（多半已死）会话：拆掉重建，绝不让它继续吃请求
        if let cached {
            NSLog("[ABSSH] 隧道会话已更换，重建 \(key)")
            cached.listener.cancel()
            lock.lock()
            tunnels[key] = nil
            lock.unlock()
        }

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
                    NSLog("[ABSSH] 转发 \(destHost):\(destPort) 失败：\(error)")
                    inbound.cancel()
                    // 泵失败 = 这条隧道绑的会话多半已经死了：作废缓存，
                    // 下一次探测会重建（否则旧监听会一直吃请求、永远失败）
                    self.invalidateTunnelIfBound(key: key, clientID: ObjectIdentifier(client))
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
        lock.lock()
        tunnels[key] = Tunnel(port: port, listener: listener, clientID: clientID)
        lock.unlock()

        // 自检一次转发：服务器禁端口转发（AllowTcpForwarding no）或目标不可达时，
        // OpenSSH 在通道开启阶段就直接拒绝 —— 在这里报出真实原因，
        // 别让用户的 HTTP 请求静默超时还不知道是谁的锅。
        if ProcessInfo.processInfo.environment["AB_NO_VERIFY"] != "1" {
            do {
                try await verifyForwarding(client: client, destHost: destHost, destPort: destPort)
            } catch {
                // 自检不过的隧道不能留：拆掉，下次探测原样重建并报出真实原因
                lock.lock()
                tunnels[key] = nil
                lock.unlock()
                listener.cancel()
                throw error
            }
        }
        return port
    }

    /// 换了新 SSH 会话后，该跳板机名下的旧隧道全部作废（绑定关系已失效）
    private func invalidateTunnels(jumpId: String) {
        lock.lock()
        let stale = tunnels.filter { $0.key.hasPrefix("\(jumpId)|") }
        tunnels = tunnels.filter { !$0.key.hasPrefix("\(jumpId)|") }
        lock.unlock()
        guard !stale.isEmpty else { return }
        NSLog("[ABSSH] 会话重建，作废 \(stale.count) 条旧隧道 jump=\(jumpId)")
        stale.values.forEach { $0.listener.cancel() }
    }

    /// 仅当缓存里的隧道还绑着指定会话时才作废（泵失败时的定向清理，别误伤重建后的新隧道）
    private func invalidateTunnelIfBound(key: String, clientID: ObjectIdentifier) {
        lock.lock()
        let cached = tunnels[key]
        let matches = cached?.clientID == clientID
        if matches { tunnels[key] = nil }
        lock.unlock()
        if let cached, matches {
            NSLog("[ABSSH] 转发失败，作废隧道 \(key)")
            cached.listener.cancel()
        }
    }

    /// 向跳板机申请一条到目标的 direct-tcpip 通道并立刻关闭，验证「转发可用」。
    /// 失败 = 服务器拒绝转发或连不上目标，把原因翻译成人话抛出去。
    private func verifyForwarding(client: SSHClient, destHost: String, destPort: Int) async throws {
        let originator = try SocketAddress(ipAddress: "127.0.0.1", port: 22)
        NSLog("[ABSSH] verify 开始 \(destHost):\(destPort) client=\(ObjectIdentifier(client).hashValue) connected=\(client.isConnected)")
        do {
            let channel = try await Self.withTimeout(seconds: 10) {
                try await client.createDirectTCPIPChannel(
                    using: SSHChannelType.DirectTCPIP(
                        targetHost: destHost,
                        targetPort: destPort,
                        originatorAddress: originator
                    )
                ) { channel in
                    // 只验「通道开得开」，不泵数据
                    channel.eventLoop.makeSucceededVoidFuture()
                }
            }
            NSLog("[ABSSH] verify 通道已开，关闭之 client=\(ObjectIdentifier(client).hashValue) connected=\(client.isConnected)")
            // close 偶发不返回时别拖着整条链路 —— 3 秒拿不到结果就放弃（目标侧连接随后由服务器收尾）
            try? await Self.withTimeout(seconds: 3) { try await channel.close() }
            NSLog("[ABSSH] verify 成功 client=\(ObjectIdentifier(client).hashValue) connected=\(client.isConnected)")
        } catch {
            NSLog("[ABSSH] verify 失败 \(destHost):\(destPort)：\(error)")
            throw SshError.message(
                "跳板机无法转发到 \(destHost):\(destPort)（\(Self.describeForwardError(error))）")
        }
    }

    private static func describeForwardError(_ error: Error) -> String {
        // 服务器拒绝开 direct-tcpip 通道：禁转发或目标连不上，OpenSSH 不细分，照实说
        if let clientError = error as? SSHClientError,
           case .channelCreationFailed = clientError {
            return "服务器拒绝了转发请求：可能禁用了端口转发（sshd 的 AllowTcpForwarding），也可能连不上目标"
        }
        let text = String(describing: error)
        let lowered = text.lowercased()
        if lowered.contains("administratively") || lowered.contains("prohibited")
            || lowered.contains("permission") {
            return "服务器禁用了端口转发，请检查 sshd 的 AllowTcpForwarding 设置"
        }
        if lowered.contains("refused") {
            return "目标端口没有服务在听，或地址配错"
        }
        if lowered.contains("timeout") || lowered.contains("timed out")
            || text.contains("超时") || text.contains("无响应") {
            return "目标无响应，可能地址不通或被防火墙拦截"
        }
        return text.isEmpty ? "未知错误" : text
    }

    /// 给慢操作限时：超时抛 SshError（输的一方自然收尾，不强杀）
    private static func withTimeout<T>(seconds: TimeInterval,
                                       _ body: @escaping @Sendable () async throws -> T) async throws -> T {
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { try await body() }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                throw SshError.message("\(Int(seconds)) 秒无响应（timeout）")
            }
            guard let first = try await group.next() else {
                throw SshError.message("操作未返回（timeout）")
            }
            group.cancelAll()
            return first
        }
    }

    /// 「经由跳板机」的电脑：把内网地址解析成本地隧道地址（127.0.0.1:port）
    func tunnelAddress(for profile: ServerProfile) async throws -> String {
        try await tunnelAddress(for: profile, destination: profile.host)
    }

    /// 同一个跳板机、指定目标地址（多块网卡时换地址重连用）
    func tunnelAddress(for profile: ServerProfile, destination: String) async throws -> String {
        guard let jump = JumpStore.shared.jump(profile.jumpServerId) else {
            throw SshError.message("跳板机不存在（可能已删除）")
        }
        let target = destination.isEmpty ? profile.host : destination
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
        NSLog("[ABSSH] pump 入站 client=\(ObjectIdentifier(client).hashValue) connected=\(client.isConnected)")
        let originator = try SocketAddress(ipAddress: "127.0.0.1", port: 22)
        let bridge = TunnelBridge(connection: inbound)
        // ⚠️ 必须先 start 再开 SSH 通道：桥的 receive 在 handlerAdded 里就发出了，
        // 而 Network.framework 要求连接 start 之后 receive 才有效——
        // 顺序反了会让 receive 报错、输入被静默关闭，TCP→SSH 方向一个字节都过不去。
        inbound.stateUpdateHandler = { [weak bridge] state in
            if case .failed = state, let bridge {
                bridge.closeRemote()
            }
            if case .cancelled = state, let bridge {
                bridge.closeRemote()
            }
        }
        inbound.start(queue: DispatchQueue(label: "anyboot-tunnel-conn"))
        let channel = try await client.createDirectTCPIPChannel(
            using: SSHChannelType.DirectTCPIP(
                targetHost: destHost,
                targetPort: destPort,
                originatorAddress: originator
            )
        ) { channel in
            NSLog("[ABSSH] pump 通道初始化")
            return channel.pipeline.addHandler(bridge)
        }
        NSLog("[ABSSH] pump 通道已开")
        _ = channel   // 通道随 bridge 的 closeFuture 一起关
    }

    // MARK: - 关闭

    func shutdown(jumpId: String) {
        lock.lock()
        let state = entries.removeValue(forKey: jumpId)
        let client = state?.client
        let dead = tunnels.filter { $0.key.hasPrefix("\(jumpId)|") }
        tunnels = tunnels.filter { !$0.key.hasPrefix("\(jumpId)|") }
        lock.unlock()
        dead.values.forEach { $0.listener.cancel() }
        Task { try? await client?.close() }
    }

    func isUp(_ jumpId: String) -> Bool {
        client(forId: jumpId)?.isConnected ?? false
    }

    // MARK: - 远程命令（exec 通道）

    /// 在跳板机上跑一条 exec 命令并收集 stdout（stderr 合流）。
    ///
    /// 用途：局域网电源包（魔术包）由跳板机在内网广播 —— 手机在外网也能唤醒家里。
    /// 20 秒兜底：远端命令卡住时不能让 UI 一直转。
    func runCommand(jump: JumpServer, command: String, timeout: TimeInterval = 20) async throws -> String {
        let client = try await client(for: jump)
        return try await withThrowingTaskGroup(of: String.self) { group in
            group.addTask {
                let buffer = try await client.executeCommand(command)
                var copy = buffer
                guard let bytes = copy.readBytes(length: copy.readableBytes) else { return "" }
                return String(bytes: bytes, encoding: .utf8) ?? ""
            }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(timeout * 1_000_000_000))
                throw SshError.message("跳板机命令超时（\(Int(timeout))s）")
            }
            guard let first = try await group.next() else {
                throw SshError.message("跳板机命令没有返回")
            }
            group.cancelAll()
            return first
        }
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
        let resizeBox = ResizeBox()
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
        //
        // ⚠️ `withPTY` 会一直 `await` 它的 perform 闭包，闭包里是「读到通道关闭」的循环，
        // 也就是说它**只有会话结束时才返回**。早先在这儿直接 `try await`，
        // 调用方 `TerminalPage.ensureSession` 就一直卡在 `startSsh()` 上：
        // 界面永远停在「正在连接 …」（PTY 其实早就开好了、提示符也吐出来了，
        // 只是没人去 bind 界面）。必须甩到后台任务，用闸门在通道建好那一刻放行。
        let ready = OneShotGate()
        let lifetime = Task {
            do {
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
                    // 终端尺寸：Citadel main 的 TTYStdinWriter 有 changeSize（内部发
                    // SSHChannelRequestEvent.WindowChangeRequest），PTY 不再恒 80x24
                    resizeBox.bind { columns, rows in
                        try await ttyStdinWriter.changeSize(cols: columns, rows: rows,
                                                           pixelWidth: 0, pixelHeight: 0)
                    }
                    // 通道确实建起来了（PTY 已被服务器接受）→ 放行上层，界面可以挂了
                    NSLog("[ABSSH] PTY 已建立 host=\(jump.host) \(columns)x\(rows)")
                    ready.succeed()
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
                // 闭包正常返回 = 会话结束；若闸门还没开，说明通道一开就关了
                ready.fail(SshError.message("终端通道刚建立就关闭了（服务器可能不允许分配 PTY）"))
            } catch {
                ready.fail(error)
                // 闸门已放行过（会话中途出错）→ 走「会话结束」这条路，别让界面卡死
                if ready.didOpen {
                    done.close()
                    onClosed(SshSessionManager.describe(error))
                }
            }
        }
        do {
            try await ready.wait(seconds: 15)
        } catch {
            lifetime.cancel()
            throw error
        }
        return ShellHandle(stdinBox: stdinBox, resizeBox: resizeBox, channel: nil,
                           done: done, lifetime: lifetime)
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
        /// 主机指纹这一环没走通（没确认 / 指纹变了 / 确认框起不来）——
        /// 单独一档：文案直接给用户看，且**不计入连接退避**
        case hostKey(String)
        case message(String)

        var errorDescription: String? {
            switch self {
            case .backoff(let seconds):
                return "连接失败过，\(seconds) 秒后再试（自动退避）"
            case .hostKey(let text):
                return text
            case .message(let text):
                return text
            }
        }
    }

    /// 能按类型判的直接判（Citadel 的枚举比字符串匹配可靠）
    private static func typedDescription(_ error: Error) -> String? {
        guard let clientError = error as? SSHClientError else { return nil }
        switch clientError {
        case .allAuthenticationOptionsFailed:
            return "认证失败：用户名或密码不正确（服务器拒绝了登录）"
        case .unsupportedPasswordAuthentication:
            return "这台服务器不接受密码登录（改用密钥认证）"
        case .unsupportedPrivateKeyAuthentication:
            return "这台服务器不接受密钥登录（改用密码认证）"
        case .unsupportedHostBasedAuthentication:
            return "这台服务器只允许 host-based 认证，App 不支持"
        case .channelCreationFailed:
            return "连接失败：服务器拒绝建立通道"
        default:
            return nil
        }
    }

    /// 把底层英文异常翻成人话（对齐 Android：`Auth fail` → 「认证失败：用户名或密码不正确」）。
    ///
    /// Citadel / NIO 的报错长这样：
    /// `The operation couldn't be completed. (Citadel.ClientHandshakeHandler…Disconnected error 1.)`
    /// 直接抛给用户等于没说。原始信息打到控制台留档，界面只给可行动的结论。
    static func describe(_ error: Error) -> String {
        if let ssh = error as? SshError {
            return ssh.errorDescription ?? "连接失败"
        }
        if let typed = typedDescription(error) {
            NSLog("[ABSSH] 原始错误: \(error)")
            return typed
        }
        let raw = "\(error)"
        let lower = raw.lowercased()
        NSLog("[ABSSH] 原始错误: \(raw)")
        if lower.contains("allauthenticationoptionsfailed")
            || lower.contains("authentication failed")
            || lower.contains("authenticationfailed") {
            return "认证失败：用户名或密码不正确（服务器拒绝了登录）"
        }
        if lower.contains("permission denied") || lower.contains("publickey") {
            return "认证失败：密钥被服务器拒绝（检查公钥是否已加到服务器）"
        }
        if lower.contains("connection refused") || lower.contains("econnrefused") {
            return "连接被拒绝：目标端口没在监听（SSH 服务没开？）"
        }
        if lower.contains("timed out") || lower.contains("timeout") || lower.contains("etimedout") {
            return "连接超时：地址或端口不通（检查网络 / 防火墙）"
        }
        if lower.contains("no route to host") || lower.contains("unreachable")
            || lower.contains("ehostunreach") || lower.contains("enetunreach") {
            return "连不上：网络不可达（检查地址与所在网段）"
        }
        if lower.contains("unknown host") || lower.contains("could not resolve")
            || lower.contains("nodename nor servname") || lower.contains("dnserror") {
            return "找不到主机：域名解析失败"
        }
        if lower.contains("disconnected") || lower.contains("connection closed")
            || lower.contains("channelclosed") {
            return "连不上：TCP 连接没建立起来（网络不通 / 端口没开 / 地址写错）"
        }
        return "连接失败：无法建立到服务器的 SSH 连接（网络不通 / 端口没开 / 地址写错）"
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

/// 终端尺寸变更的盒子：把 `changeSize` 的 writer 与调用方隔开
/// （Citadel 的 TTYStdinWriter 只能在 withPTY 闭包里拿到）
final class ResizeBox: @unchecked Sendable {
    private var writer: ((Int, Int) async throws -> Void)?
    private let lock = NSLock()
    private var lastColumns = 0
    private var lastRows = 0

    func bind(_ raw: @escaping (Int, Int) async throws -> Void) {
        lock.lock()
        writer = raw
        lock.unlock()
    }

    /// 同尺寸重报会被 OpenSSH 自己比对掉，这里再省一次网络往返
    func resize(columns: Int, rows: Int) {
        guard columns > 0, rows > 0 else { return }
        lock.lock()
        let raw = writer
        let same = columns == lastColumns && rows == lastRows
        lastColumns = columns
        lastRows = rows
        lock.unlock()
        guard let raw, !same else { return }
        Task { try? await raw(columns, rows) }
    }
}

/// 终端会话句柄
final class ShellHandle {
    let stdinBox: StdinBox
    let resizeBox: ResizeBox
    let channel: Channel?
    let done: ClosedBox
    /// PTY 后台任务的句柄（withPTY 只有会话结束才返回，必须持有它才能取消）
    private let lifetime: Task<Void, Never>?

    init(stdinBox: StdinBox, resizeBox: ResizeBox, channel: Channel?, done: ClosedBox,
         lifetime: Task<Void, Never>? = nil) {
        self.stdinBox = stdinBox
        self.resizeBox = resizeBox
        self.channel = channel
        self.done = done
        self.lifetime = lifetime
    }

    func write(_ data: Data) {
        stdinBox.write(data)
    }

    /// 终端窗口变了（xterm.js fit 之后）→ 发 WindowChangeRequest，PTY 跟着变
    /// （此前是空实现，vim/htop 在 iOS 上永远以为自己是 80x24）
    func resize(columns: Int, rows: Int) {
        resizeBox.resize(columns: columns, rows: rows)
    }

    /// 结束 PTY 后台任务（连接失败/页面退出时别留悬挂的读循环）
    func cancel() {
        lifetime?.cancel()
    }
}

/// 一次性闸门：后台任务「准备好了」才放行等待方，带超时。
/// 用来把 `withPTY` 那种「一直挂到会话结束」的调用拆成「先返回句柄、后跑循环」。
final class OneShotGate: @unchecked Sendable {

    private let lock = NSLock()
    private var result: Result<Void, Error>?
    private var continuation: CheckedContinuation<Void, Error>?
    private var opened = false

    /// 闸门是否已经以「成功」放行过（用于区分「连不上」和「连上后断了」）
    var didOpen: Bool {
        lock.lock()
        defer { lock.unlock() }
        return opened
    }

    func succeed() {
        resolve(.success(()))
    }

    func fail(_ error: Error) {
        resolve(.failure(error))
    }

    private func resolve(_ value: Result<Void, Error>) {
        lock.lock()
        if result != nil {
            lock.unlock()
            return
        }
        result = value
        if case .success = value { opened = true }
        let waiting = continuation
        continuation = nil
        lock.unlock()
        waiting?.resume(with: value)
    }

    /// 等放行；超时按失败处理（并唤醒等待方，不留悬挂 Task）
    func wait(seconds: Double) async throws {
        let timeout = Task {
            do {
                try await Task.sleep(nanoseconds: UInt64(max(0.1, seconds) * 1_000_000_000))
            } catch {
                return   // 被取消 = 正常放行了
            }
            self.fail(SshSessionManager.SshError.message(
                    "打开终端超时：服务器没回应 PTY 请求（可能禁用了 shell）"))
        }
        defer { timeout.cancel() }
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            lock.lock()
            if let done = result {
                lock.unlock()
                cont.resume(with: done)
                return
            }
            continuation = cont
            lock.unlock()
        }
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
                // ⚠️ 必须 writeAndFlush（出站 → head 的 NIOSSH → CHANNEL_DATA）。
                // 之前误用 fireChannelRead（入站方向）：数据直接从 pipeline 尾部漏掉，
                // 症状是「通道开得开、服务器也连上目标，但一个字节都过不去」——
                // HTTP 请求静默挂死，表现成跳板机「没转发」。
                context.writeAndFlush(self.wrapOutboundOut(buffer)).whenFailure { err in
                    NSLog("[ABSSH] 转发写入失败：\(err)")
                }
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
