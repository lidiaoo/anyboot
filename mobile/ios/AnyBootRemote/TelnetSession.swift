import Foundation
import Network

/// Telnet 会话（实验性）：NWConnection 直连 + IAC 控制字节过滤（与 Android 端同构）。
final class TelnetSession {

    static let iac: UInt8 = 255
    static let dont: UInt8 = 254
    static let doCmd: UInt8 = 253
    static let wont: UInt8 = 252
    static let will: UInt8 = 251
    static let se: UInt8 = 240

    private var connection: NWConnection?
    private var closed = false
    private let lock = NSLock()

    var onOutput: ((Data) -> Void)?
    var onClosed: ((String) -> Void)?

    func start(jump: JumpServer) throws {
        guard let endpoint = NWEndpoint.Host(jump.hostname) else {
            throw NSError(domain: "TelnetSession", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "地址不合法"])
        }
        let connection = NWConnection(
            host: endpoint,
            port: NWEndpoint.Port(rawValue: UInt16(jump.port)) ?? 23,
            using: .tcp)
        self.connection = connection
        connection.stateUpdateHandler = { [weak self] state in
            switch state {
            case .failed(let error):
                self?.finish("连接失败：\(error)")
            case .cancelled:
                self?.finish("连接已关闭")
            default:
                break
            }
        }
        connection.start(queue: DispatchQueue(label: "anyboot-telnet"))
        receiveLoop(connection)
    }

    private func receiveLoop(_ connection: NWConnection) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 16 * 1024) { [weak self] data, _, isComplete, error in
            guard let self, !self.isClosed else { return }
            if let data, !data.isEmpty {
                self.filterIac(data)
            }
            if isComplete || error != nil {
                self.finish("连接已关闭")
                return
            }
            self.receiveLoop(connection)
        }
    }

    /// 发送键盘字节
    func write(_ data: Data) {
        connection?.send(content: data, completion: .contentProcessed { _ in })
    }

    func close(_ reason: String) {
        finish(reason)
    }

    var isAlive: Bool {
        lock.lock()
        defer { lock.unlock() }
        return !closed && connection != nil
    }

    // MARK: - IAC 状态机

    /// 半截命令跨包的残留
    private var pending: [UInt8] = []

    private func filterIac(_ raw: Data) {
        var bytes = pending
        bytes.append(contentsOf: [UInt8](raw))
        pending = []
        var output = Data()
        var i = 0
        while i < bytes.count {
            let b = bytes[i]
            if b == TelnetSession.iac {
                if i + 1 >= bytes.count {
                    pending = Array(bytes[i...])   // 等下一包
                    break
                }
                let cmd = bytes[i + 1]
                if cmd == TelnetSession.iac {
                    output.append(255)
                    i += 2
                    continue
                }
                if cmd == TelnetSession.will || cmd == TelnetSession.doCmd
                    || cmd == TelnetSession.wont || cmd == TelnetSession.dont {
                    if i + 2 >= bytes.count {
                        pending = Array(bytes[i...])
                        break
                    }
                    let option = bytes[i + 2]
                    // 只接受 WILL ECHO(1) / WILL SGA(3)，其余拒绝；对端要我们 DO 什么，一律 WONT
                    if cmd == TelnetSession.will && (option == 1 || option == 3) {
                        connection?.send(content: Data([TelnetSession.iac, TelnetSession.doCmd, option]),
                                         completion: .contentProcessed { _ in })
                    } else if cmd == TelnetSession.will {
                        connection?.send(content: Data([TelnetSession.iac, TelnetSession.dont, option]),
                                         completion: .contentProcessed { _ in })
                    } else if cmd == TelnetSession.doCmd {
                        connection?.send(content: Data([TelnetSession.iac, TelnetSession.wont, option]),
                                         completion: .contentProcessed { _ in })
                    }
                    i += 3
                    continue
                }
                // SB(250)…SE(240) 子协商整段丢弃；其它命令 2 字节
                if cmd == 250 {
                    i += 2
                    while i < bytes.count && bytes[i] != TelnetSession.se {
                        i += 1
                    }
                    i += 1
                } else {
                    i += 2
                }
                continue
            }
            output.append(b)
            i += 1
        }
        if !output.isEmpty {
            onOutput?(output)
        }
    }

    private func finish(_ reason: String) {
        lock.lock()
        let already = closed
        closed = true
        lock.unlock()
        connection?.cancel()
        connection = nil
        if !already {
            onClosed?(reason)
        }
    }
}
