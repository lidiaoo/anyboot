import Foundation
import Combine

/// 一台「服务器」= SSH / Telnet 跳板机（与 Android 端 JumpServer 同构，
/// 字段名与 JSON 键完全一致，互导配置不失真）。
struct JumpServer: Identifiable, Hashable, Codable {

    var id: String = UUID().uuidString
    var name: String = ""
    /// `ssh` / `telnet`
    var protocolRaw: String = JumpServer.ssh
    /// host 或 host:port
    var host: String = ""
    var username: String = ""
    /// `password` / `key`
    var authType: String = JumpServer.authPassword
    /// Keychain 里密码的 account 键（空 = 没存密码）
    var passwordRef: String = ""
    /// 引用 SshKeyEntry.id
    var keyId: String = ""
    /// Keychain 里密钥口令的 account 键
    var keyPassphraseRef: String = ""
    var note: String = ""
    /// 经由哪台服务器中转（空 = 直连）—— 与 Android `JumpServer.jumpServerId` 同义同键
    var jumpServerId: String = ""
    /// 终端启动路径（可选，每台服务器各自配）：非空时登录后先 `cd` 过去；目录打不通
    /// 时终端里报错、照常落家目录（与 PC/Android 同语义）。可选让老档案照常解码。
    var termPath: String? = nil
    /// 终端 shell（可选）：/bin/bash /bin/sh /bin/zsh /bin/fish 等预设，或自定义命令
    /// 原样 exec。空/nil = 远端账号的登录 shell，零干预。
    var termShell: String? = nil
    /// 最近一次连通的往返毫秒；-1 = 未知
    var lastRttMs: Double = -1
    var lastSeen: Date?
    /// 最后改动时间（备份/云同步按条目 LWW 合并的依据；老档案缺省 nil 照常解码）
    var updatedAt: Date?

    static let ssh = "ssh"
    static let telnet = "telnet"
    static let authPassword = "password"
    static let authKey = "key"

    var isSsh: Bool { protocolRaw != Self.telnet }

    var hostname: String {
        guard let index = host.lastIndex(of: ":"), index < host.index(before: host.endIndex)
        else { return host.trimmedAnyBoot }
        return String(host[..<index]).trimmedAnyBoot
    }

    var port: Int {
        guard let index = host.lastIndex(of: ":") else { return defaultPort }
        let value = String(host[host.index(after: index)...]).trimmedAnyBoot
        return Int(value) ?? defaultPort
    }

    var defaultPort: Int {
        protocolRaw == Self.telnet ? 23 : 22
    }

    /// 列表行第二行：user@host:port
    var addressLabel: String {
        let user = username.trimmedAnyBoot
        return (user.isEmpty ? "" : "\(user)@") + host
    }

    var protocolLabel: String { protocolRaw == Self.telnet ? "Telnet" : "SSH" }

    /// 认证方式的中文短标签（卡片三格信息里的「认证」）
    var authLabel: String { authType == Self.authKey ? "密钥" : "密码" }

    var displayName: String {
        let value = name.trimmedAnyBoot
        return value.isEmpty ? addressLabel : value
    }

    /// 解析 host 输入：容忍 user@host:port / 整条 URL，user 摘进 username
    mutating func applyHostInput(_ raw: String) {
        var value = raw.trimmedAnyBoot
        guard !value.isEmpty else { return }
        if let range = value.range(of: "://") {
            value = String(value[range.upperBound...])
        }
        if let range = value.range(of: "/") {
            value = String(value[..<range.lowerBound])
        }
        if let index = value.lastIndex(of: "@"), index != value.startIndex {
            let user = String(value[..<index]).trimmedAnyBoot
            if username.trimmedAnyBoot.isEmpty { username = user }
            value = String(value[value.index(after: index)...])
        }
        if !value.contains(":") {
            value += ":\(defaultPort)"
        }
        host = value
    }
}

/// 一把 SSH 密钥（密钥页的密钥卡）。私钥 PEM 存 Keychain（`privateRef`）。
struct SshKeyEntry: Identifiable, Hashable, Codable {
    var id: String = UUID().uuidString
    var name: String = ""
    /// 算法名（ssh-ed25519 / ssh-rsa …）
    var algorithm: String = ""
    /// SHA256:base64 公钥指纹
    var fingerprint: String = ""
    /// Keychain 里私钥 PEM 的 account 键
    var privateRef: String = ""
    /// 公钥（OpenSSH 一行格式，可空）
    var publicKey: String = ""
    var createdAt: Date = Date()
    /// 最后改动时间（备份/云同步按条目 LWW 合并的依据；老档案缺省 nil 照常解码）
    var updatedAt: Date? = nil
}

/// 服务器（跳板机）与密钥的持久化 + 终端偏好 + 会话历史。
final class JumpStore: ObservableObject {

    static let shared = JumpStore()

    @Published private(set) var jumps: [JumpServer] = []
    @Published private(set) var keys: [SshKeyEntry] = []
    /// 终端主题（深色默认）
    @Published private(set) var terminalDark: Bool = true
    /// 终端字体族（xterm 的 CSS font-family）：空 = 资产页内置 Menlo/等宽栈
    @Published private(set) var terminalFontFamily: String = ""
    /// 终端字号（8~28，默认 14；与 Android `terminal_font_size` 同键同值域）
    @Published private(set) var terminalFontSize: Int = JumpStore.defaultFontSize
    /// 进终端自动弹软键盘（默认关；与 Android `terminal_auto_keyboard` 同键）
    @Published private(set) var terminalAutoKeyboard: Bool = false
    /// 底部按键栏（Esc/Tab/Ctrl/方向/Enter，默认开；与 Android `terminal_key_bar` 同键）
    @Published private(set) var terminalKeyBar: Bool = true

    static let defaultFontSize = 14
    static let fontSizeRange = 8...28
    /// 会话历史（最近 50 条）
    @Published private(set) var history: [HistoryEntry] = []

    struct HistoryEntry: Identifiable, Codable {
        var id = UUID()
        var serverId: String
        var serverName: String
        var startedAt: Date
        var endedAt: Date
    }

    private let keyJumps = "jump_servers"
    private let keyKeys = "ssh_keys"
    private let keyTerminalDark = "terminal_dark"
    private let keyTerminalFontFamily = "terminal_font_family"
    private let keyTerminalFontSize = "terminal_font_size"
    private let keyTerminalAutoKeyboard = "terminal_auto_keyboard"
    private let keyTerminalKeyBar = "terminal_key_bar"
    private let keyHistory = "session_history"
    private let defaults = UserDefaults.standard

    private init() {
        if let data = defaults.data(forKey: keyJumps),
           let list = try? JSONDecoder().decode([JumpServer].self, from: data) {
            jumps = list
        }
        if let data = defaults.data(forKey: keyKeys),
           let list = try? JSONDecoder().decode([SshKeyEntry].self, from: data) {
            keys = list
        }
        terminalDark = defaults.object(forKey: keyTerminalDark) as? Bool ?? true
        terminalFontFamily = (defaults.string(forKey: keyTerminalFontFamily) ?? "").trimmingCharacters(in: .whitespaces)
        // object(forKey:) 判存在：缺键时 integer(forKey:) 给 0，会被夹到下限 8 而不是默认 14
        let storedSize = defaults.object(forKey: keyTerminalFontSize) as? Int
        terminalFontSize = Self.clampFontSize(storedSize ?? Self.defaultFontSize)
        terminalAutoKeyboard = defaults.object(forKey: keyTerminalAutoKeyboard) as? Bool ?? false
        terminalKeyBar = defaults.object(forKey: keyTerminalKeyBar) as? Bool ?? true
        if let data = defaults.data(forKey: keyHistory),
           let list = try? JSONDecoder().decode([HistoryEntry].self, from: data) {
            history = list
        }
    }

    // MARK: - 跳板机

    func jump(_ id: String) -> JumpServer? {
        jumps.first { $0.id == id }
    }

    func upsert(_ server: JumpServer) {
        var server = server
        server.updatedAt = Date()
        if let index = jumps.firstIndex(where: { $0.id == server.id }) {
            jumps[index] = server
        } else {
            jumps.append(server)
        }
        persistJumps()
        syncTouch()
    }

    func remove(_ id: String) {
        if let server = jump(id), !server.passwordRef.isEmpty {
            KeyVault.delete(account: server.passwordRef)
        }
        if jump(id) != nil {
            Task { @MainActor in SyncStore.shared.recordDeletion(cat: "jumps", id: id) }
        }
        jumps.removeAll { $0.id == id }
        persistJumps()
        syncTouch()
    }

    /// 应急密码框勾了「保存密码」→ 加密落 Keychain 并把引用写回档案。
    /// 对齐 Android `JumpStore.rememberPassword`：空密码 / 档案不存在 / 写盘失败都返回 false，
    /// 由调用方决定怎么提示（绝不落明文）。
    @discardableResult
    func rememberPassword(_ id: String, _ password: String) -> Bool {
        let value = password.trimmingCharacters(in: .whitespaces)
        guard !value.isEmpty else { return false }
        guard var target = jump(id) else { return false }
        let ref = target.passwordRef.isEmpty ? "pwd-\(target.id)" : target.passwordRef
        // 钥匙串异常就宁可这次不落盘，也别把明文写进 UserDefaults
        guard KeyVault.save(value, account: ref) else { return false }
        target.passwordRef = ref
        upsert(target)
        return true
    }

    /// 有几台电脑经由这台跳板机
    func computersVia(_ id: String) -> Int {
        ServerStore.shared.servers.filter { $0.jumpServerId == id }.count
    }

    private func persistJumps() {
        if let data = try? JSONEncoder().encode(jumps) {
            defaults.set(data, forKey: keyJumps)
        }
    }

    /// 同步钩子：跳板机档案变化 → 去抖同步（SyncEngine 内部自判开关）
    private func syncTouch() {
        Task { @MainActor in SyncEngine.shared.notifyChanged() }
    }

    // MARK: - 备份/同步批量落库（BackupStore 专用）

    /// 整表替换（覆盖导入）；调用方已清理旧 Keychain 材料
    func replaceAll(_ list: [JumpServer]) {
        jumps = list
        persistJumps()
    }

    /// 历史整表写回（导入合并后落库；仍保持 50 条上限、最新在前）
    func setHistory(_ list: [HistoryEntry]) {
        var list = list.sorted { $0.startedAt > $1.startedAt }
        while list.count > 50 { list.removeLast() }
        history = list
        if let data = try? JSONEncoder().encode(history) {
            defaults.set(data, forKey: keyHistory)
        }
    }

    // MARK: - 密钥

    func key(_ id: String) -> SshKeyEntry? {
        keys.first { $0.id == id }
    }

    func upsert(_ entry: SshKeyEntry) {
        var entry = entry
        entry.updatedAt = Date()
        if let index = keys.firstIndex(where: { $0.id == entry.id }) {
            keys[index] = entry
        } else {
            keys.append(entry)
        }
        persistKeys()
        syncTouch()
    }

    func removeKey(_ id: String) {
        if key(id) != nil {
            Task { @MainActor in SyncStore.shared.recordDeletion(cat: "keys", id: id) }
        }
        keys.removeAll { $0.id == id }
        persistKeys()
        syncTouch()
    }

    /// 有几台服务器在用这把密钥
    func keysInUse(_ keyId: String) -> Int {
        jumps.filter { $0.keyId == keyId }.count
    }

    private func persistKeys() {
        if let data = try? JSONEncoder().encode(keys) {
            defaults.set(data, forKey: keyKeys)
        }
    }

    // MARK: - 偏好 / 历史

    /// 终端配色：落盘后立即广播（顶栏 🌓 热切换 —— 对齐 Android eval("termSetTheme(...)"))
    func setTerminalDark(_ dark: Bool) {
        terminalDark = dark
        defaults.set(dark, forKey: keyTerminalDark)
        NotificationCenter.default.post(name: .init("anyboot-term-theme"),
                                        object: dark ? "dark" : "light")
    }

    /// 终端字体族：字体缺失由 WebView 自动回落等宽，不做存在性检查
    func setTerminalFontFamily(_ family: String) {
        let value = family.trimmingCharacters(in: .whitespaces)
        terminalFontFamily = value
        defaults.set(value, forKey: keyTerminalFontFamily)
    }

    /// 终端字号：夹到 8~28；落盘后广播给已开的终端页热套用
    func setTerminalFontSize(_ size: Int) {
        let value = Self.clampFontSize(size)
        terminalFontSize = value
        defaults.set(value, forKey: keyTerminalFontSize)
        NotificationCenter.default.post(name: .init("anyboot-term-size"), object: value)
    }

    func setTerminalAutoKeyboard(_ on: Bool) {
        terminalAutoKeyboard = on
        defaults.set(on, forKey: keyTerminalAutoKeyboard)
    }

    func setTerminalKeyBar(_ on: Bool) {
        terminalKeyBar = on
        defaults.set(on, forKey: keyTerminalKeyBar)
    }

    static func clampFontSize(_ size: Int) -> Int {
        min(max(size, fontSizeRange.lowerBound), fontSizeRange.upperBound)
    }

    /// 三项设置的整体热套用（改完偏好后调一次；终端页各自监听对应通知）
    func broadcastTerminalSettings() {
        NotificationCenter.default.post(name: .init("anyboot-term-size"), object: terminalFontSize)
        NotificationCenter.default.post(name: .init("anyboot-term-font"), object: terminalFontFamily)
        NotificationCenter.default.post(name: .init("anyboot-term-theme"),
                                        object: terminalDark ? "dark" : "light")
    }

    func appendHistory(_ entry: HistoryEntry) {
        history.insert(entry, at: 0)
        while history.count > 50 {
            history.removeLast()
        }
        if let data = try? JSONEncoder().encode(history) {
            defaults.set(data, forKey: keyHistory)
        }
    }

    func clearHistory() {
        history = []
        defaults.removeObject(forKey: keyHistory)
    }
}
