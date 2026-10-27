import Foundation

// MARK: - 备份核心（docs/backup-design.md §4/§5/§9.3）
//
// 纯函数核心：collect（导出组包）/ summarize（预览摘要）/ apply（导入落地 / 同步合并）。
// 不碰 UI；文件导入与 WebDAV 同步共用 apply。

enum BackupStrategy: String, Identifiable {
    case skipExisting   // 跳过本机已有
    case overwrite      // 备份覆盖本机

    var id: String { rawValue }

    var label: String {
        self == .skipExisting ? "跳过本机已有" : "备份覆盖本机"
    }
}

enum BackupError: LocalizedError {
    case tooLarge
    case badFormat          // 不是有效的 AnyBoot 备份文件
    case fromNewerVersion
    case decryptFailed      // 口令错误或文件已损坏
    case emptyPassword

    var errorDescription: String? {
        switch self {
        case .tooLarge: return "文件超过 10MB，已拒绝"
        case .badFormat: return "不是有效的 AnyBoot 备份文件"
        case .fromNewerVersion: return "备份来自更新版本，请先升级 App"
        case .decryptFailed: return "口令错误或文件已损坏"
        case .emptyPassword: return "请输入备份口令"
        }
    }
}

final class BackupStore {

    static let shared = BackupStore()
    private init() {}

    private let decoder = JSONDecoder()
    private let encoder = JSONEncoder()
    private var defaults: UserDefaults { .standard }

    // MARK: - 收集（导出组包）

    /// 组装明文 envelope。`categories` 为勾选的分类；`sync` 仅云同步携带。
    func collect(categories: Set<BackupCategory>, sync: BackupSyncMeta? = nil) throws -> BackupEnvelope {
        var data = BackupData()

        if categories.contains(.servers) {
            data.servers = BackupServers(
                items: ServerStore.shared.servers.map(canonical),
                activeId: ServerStore.shared.activeId)
        }
        if categories.contains(.jumps) {
            data.jumps = BackupJumps(items: JumpStore.shared.jumps.map(canonical))
        }
        if categories.contains(.keys) {
            var secrets: [String: String] = [:]
            var items: [CanonicalKey] = []
            for entry in JumpStore.shared.keys {
                items.append(canonicalKey(entry))
                // KeyVault 读失败 → 跳过该材料（不静默导空值；报告由 UI 层提示）
                let pem = KeyVault.load(account: entry.privateRef)
                if !pem.isEmpty { secrets[BackupSecretKey.keyPrivate(entry.id)] = pem }
            }
            data.keys = BackupKeys(items: items, secrets: secrets)
        }
        if categories.contains(.terminal) {
            let js = JumpStore.shared
            data.terminal = BackupPrefs(
                terminalDark: js.terminalDark,
                terminalFontSize: js.terminalFontSize,
                terminalFontFamily: js.terminalFontFamily,
                terminalAutoKeyboard: js.terminalAutoKeyboard,
                terminalKeyBar: js.terminalKeyBar,
                transport: ServerStore.shared.transport,
                connectAll: ServerStore.shared.connectAll,
                failReloadSec: defaults.integer(forKey: "failReloadSec"))
        }
        if categories.contains(.history) {
            data.history = collectHistory()
        }

        var envelope = BackupEnvelope()
        envelope.app = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? ""
        envelope.platform = "ios"
        envelope.exportedAt = BackupDate.string(Date())
        envelope.includes = categories.map(\.rawValue).sorted()
        envelope.sync = sync
        envelope.data = data
        return envelope
    }

    private func collectHistory() -> BackupHistory {
        var hosts: [CanonicalTrustedHost] = []
        for (key, value) in defaults.dictionaryRepresentation() {
            guard key.hasPrefix("tofu_"), let fp = value as? String else { continue }
            hosts.append(CanonicalTrustedHost(host: String(key.dropFirst(5)), fingerprint: fp))
        }
        let sessions = JumpStore.shared.history.map {
            CanonicalSession(serverId: $0.serverId, serverName: $0.serverName,
                             startedAt: BackupDate.string($0.startedAt),
                             endedAt: BackupDate.string($0.endedAt))
        }
        return BackupHistory(sessions: sessions, trustedHosts: hosts)
    }

    /// 序列化 + 可选加密 → 可写文件/上传的最终字节
    func serialize(_ envelope: BackupEnvelope, password: String?) throws -> Data {
        if let password {
            let plain = try encoder.encode(envelope)
            let (payload, encryption) = try BackupCrypto.encrypt(plain: plain, password: password)
            var boxed = envelope
            boxed.data = nil
            boxed.encryption = encryption
            boxed.payload = payload
            return try encoder.encode(boxed)
        }
        return try encoder.encode(envelope)
    }

    // MARK: - 解析（导入/同步的拉取侧）

    /// 原始字节 → 明文 envelope。加密文件走 `decrypt(data:password:)`。
    func parse(raw: Data) throws -> BackupEnvelope {
        guard raw.count <= BackupConstants.maxFileBytes else { throw BackupError.tooLarge }
        let envelope = try decodeEnvelope(raw)
        if envelope.format != BackupConstants.format { throw BackupError.badFormat }
        if envelope.isFromNewerApp { throw BackupError.fromNewerVersion }
        return envelope
    }

    /// 加密文件：外层信封 + 口令 → 明文 envelope
    func decrypt(raw: Data, password: String) throws -> BackupEnvelope {
        guard raw.count <= BackupConstants.maxFileBytes else { throw BackupError.tooLarge }
        let envelope = try decodeEnvelope(raw)
        guard envelope.format == BackupConstants.format else { throw BackupError.badFormat }
        guard let encryption = envelope.encryption, let payload = envelope.payload else {
            // 未加密文件带口令调用：直接走明文路径
            if envelope.isFromNewerApp { throw BackupError.fromNewerVersion }
            return envelope
        }
        do {
            let plain = try BackupCrypto.decrypt(payloadB64: payload, encryption: encryption, password: password)
            var inner = try decodeEnvelope(plain)
            if inner.format != BackupConstants.format { throw BackupError.badFormat }
            if inner.isFromNewerApp { throw BackupError.fromNewerVersion }
            inner.sync = envelope.sync    // sync 元数据在外层
            return inner
        } catch let e as BackupCryptoError {
            throw e
        } catch let e as BackupError {
            throw e
        } catch {
            throw BackupError.decryptFailed
        }
    }

    /// 若文件是加密的，返回外层信封（供 UI 判断要不要口令页）；否则返回 nil
    func peekEncryption(raw: Data) -> BackupEncryption? {
        guard let envelope = try? decodeEnvelope(raw), envelope.format == BackupConstants.format else { return nil }
        return envelope.encryption
    }

    private func decodeEnvelope(_ raw: Data) throws -> BackupEnvelope {
        do { return try decoder.decode(BackupEnvelope.self, from: raw) }
        catch { throw BackupError.badFormat }
    }

    // MARK: - 摘要（预览页 S3）

    func summarize(_ envelope: BackupEnvelope) -> BackupSummary {
        var summary = BackupSummary()
        summary.platform = envelope.platform
        summary.exportedAt = envelope.exportedAt
        summary.app = envelope.app
        summary.isEncrypted = false   // 调到这一步时已解密

        let servers = envelope.data?.servers?.items ?? []
        let jumps = envelope.data?.jumps?.items ?? []
        let keys = envelope.data?.keys?.items ?? []
        let sessions = envelope.data?.history?.sessions ?? []
        let hosts = envelope.data?.history?.trustedHosts ?? []

        func row(_ cat: BackupCategory, backup: Int, local: Int, conflicts: [String]) {
            summary.rows[cat] = .init(backupCount: backup, localCount: local, conflicts: conflicts)
        }

        let localServerIds = Set(ServerStore.shared.servers.map(\.id))
        let serverConflicts = servers.filter { localServerIds.contains($0.id ?? "") }
            .compactMap { ServerStore.shared.profile($0.id ?? "")?.displayName }
        row(.servers, backup: servers.count, local: ServerStore.shared.servers.count, conflicts: serverConflicts)

        let localJumpIds = Set(JumpStore.shared.jumps.map(\.id))
        let jumpConflicts = jumps.filter { localJumpIds.contains($0.id ?? "") }
            .compactMap { JumpStore.shared.jump($0.id ?? "")?.displayName }
        row(.jumps, backup: jumps.count, local: JumpStore.shared.jumps.count, conflicts: jumpConflicts)

        let localKeyIds = Set(JumpStore.shared.keys.map(\.id))
        let keyConflicts = keys.filter { localKeyIds.contains($0.id ?? "") }
            .compactMap { JumpStore.shared.key($0.id ?? "")?.name }
        row(.keys, backup: keys.count, local: JumpStore.shared.keys.count, conflicts: keyConflicts)

        let hasPrefs = envelope.data?.terminal != nil
        row(.terminal, backup: hasPrefs ? 1 : 0, local: 1, conflicts: [])

        row(.history, backup: sessions.count + hosts.count, local: JumpStore.shared.history.count, conflicts: [])

        // 引用完整性：电脑关联的跳板机不在本备份
        let jumpIds = Set(jumps.compactMap(\.id))
        summary.missingJumpRefs = servers.filter {
            let ref = $0.jumpServerId ?? ""
            return !ref.isEmpty && !jumpIds.contains(ref)
        }.count
        return summary
    }

    // MARK: - 落地（导入 apply / 同步合并共用）

    /// 用户导入：勾选分类 + 合并策略 → 报告
    @discardableResult
    func apply(_ envelope: BackupEnvelope,
               categories: Set<BackupCategory>,
               strategy: BackupStrategy) -> ImportReport {
        var report = ImportReport()
        // secrets 明文表装填（restoreJumpSecrets / applyKeys 消费）
        currentSecrets = envelope.data?.keys?.secrets ?? [:]
        defer { currentSecrets = [:] }   // 用完即清，明文不常驻
        let tombstones = Set((envelope.sync?.tombstones ?? []).map { "\($0.cat)|\($0.id)" })

        if categories.contains(.servers), let payload = envelope.data?.servers {
            report.categories.append(applyServers(payload, strategy: strategy, tombstones: tombstones))
        }
        if categories.contains(.jumps), let payload = envelope.data?.jumps {
            report.categories.append(applyJumps(payload, strategy: strategy, tombstones: tombstones))
        }
        if categories.contains(.keys), let payload = envelope.data?.keys {
            report.categories.append(applyKeys(payload, strategy: strategy, tombstones: tombstones))
        }
        if categories.contains(.terminal), let prefs = envelope.data?.terminal {
            report.categories.append(applyPrefs(prefs))
        }
        if categories.contains(.history), let payload = envelope.data?.history {
            report.categories.append(applyHistory(payload, strategy: strategy))
        }
        return report
    }

    // ---- 电脑列表 ----

    private func applyServers(_ payload: BackupServers, strategy: BackupStrategy,
                              tombstones: Set<String>) -> CategoryReport {
        var report = CategoryReport(category: .servers)
        let store = ServerStore.shared

        if strategy == .overwrite {
            // 整表替换：先清被替换档案的引用材料（无密钥材料），再落表
            for old in store.servers where tombstones.contains("servers|\(old.id)") {
                _ = old   // 电脑档案无安全存储材料；墓碑命中即随整表替换消失
            }
            var list: [ServerProfile] = []
            for (index, item) in payload.items.enumerated() {
                guard var profile = server(from: item) else {
                    report.failed.append("第 \(index + 1) 台电脑：档案字段不合法")
                    continue
                }
                touch(&profile)
                list.append(profile)
            }
            store.replaceAll(list, activeId: payload.activeId)
            report.added = list.count
            return report
        }

        for (index, item) in payload.items.enumerated() {
            guard var profile = server(from: item) else {
                report.failed.append("第 \(index + 1) 台电脑：档案字段不合法")
                continue
            }
            let id = profile.id
            if tombstones.contains("servers|\(id)") {
                if store.profile(id) != nil { store.remove(id) }
                report.skipped += 1
                continue
            }
            if let existing = store.profile(id) {
                if strategy == .skipExisting {
                    report.skipped += 1
                } else {
                    touch(&profile)
                    store.upsert(profile)
                    report.updated += 1
                    _ = existing
                }
            } else {
                touch(&profile)
                store.upsert(profile)
                report.added += 1
            }
        }
        // 引用完整性：不静默修复
        let jumpIds = Set(JumpStore.shared.jumps.map(\.id))
        for profile in store.servers {
            let ref = profile.jumpServerId
            if !ref.isEmpty && !jumpIds.contains(ref) {
                report.missingRefs.append("「\(profile.displayName)」的跳板机待配置")
            }
        }
        return report
    }

    // ---- SSH 服务器 ----

    private func applyJumps(_ payload: BackupJumps, strategy: BackupStrategy,
                            tombstones: Set<String>) -> CategoryReport {
        var report = CategoryReport(category: .jumps)
        let store = JumpStore.shared

        if strategy == .overwrite {
            for old in store.jumps {
                if !old.passwordRef.isEmpty { KeyVault.delete(account: old.passwordRef) }
                if !old.keyPassphraseRef.isEmpty { KeyVault.delete(account: old.keyPassphraseRef) }
            }
            var list: [JumpServer] = []
            for (index, item) in payload.items.enumerated() {
                guard var jump = jump(from: item) else {
                    report.failed.append("第 \(index + 1) 台服务器：档案字段不合法")
                    continue
                }
                restoreJumpSecrets(&jump, secrets: currentSecrets)
                touch(&jump)
                list.append(jump)
            }
            store.replaceAll(list)
            report.added = list.count
            return report
        }

        for (index, item) in payload.items.enumerated() {
            guard var jump = jump(from: item) else {
                report.failed.append("第 \(index + 1) 台服务器：档案字段不合法")
                continue
            }
            let id = jump.id
            if tombstones.contains("jumps|\(id)") {
                if let old = store.jump(id) { store.remove(old.id) }
                report.skipped += 1
                continue
            }
            if store.jump(id) != nil {
                if strategy == .skipExisting {
                    report.skipped += 1
                } else {
                    restoreJumpSecrets(&jump, secrets: currentSecrets)
                    touch(&jump)
                    store.upsert(jump)
                    report.updated += 1
                }
            } else {
                restoreJumpSecrets(&jump, secrets: currentSecrets)
                touch(&jump)
                store.upsert(jump)
                report.added += 1
            }
        }
        return report
    }

    /// applyJumps 执行期间的 secrets 上下文（apply 入口装填）
    private var currentSecrets: [String: String] = [:]

    // MARK: - SyncEngine 内部入口（本文件私有步骤对同步引擎开放）

    func serverFrom(_ item: CanonicalServer) -> ServerProfile? { server(from: item) }
    func jumpFrom(_ item: CanonicalJump) -> JumpServer? { jump(from: item) }
    func keyEntryFrom(_ item: CanonicalKey) -> SshKeyEntry? { keyEntry(from: item) }

    /// 同步合并用：装填本次同步信封的 secrets 后恢复 SSH 服务器材料
    func restoreJumpSecretsForSync(_ jump: inout JumpServer, secrets: [String: String]) {
        restoreJumpSecrets(&jump, secrets: secrets)
    }

    /// 同步合并用：仅应用偏好（偏好无 LWW 时间戳，远端整体胜）
    @discardableResult
    func applyPrefsForSync(_ prefs: BackupPrefs) -> CategoryReport { applyPrefs(prefs) }

    /// 同步合并用：历史去重追加
    @discardableResult
    func applyHistoryForSync(_ payload: BackupHistory, strategy: BackupStrategy) -> CategoryReport {
        applyHistory(payload, strategy: strategy)
    }

    /// 把稳定键明文写回 Keychain 并在档案上生成本端 ref（ref 是本端新造的，备份里的不入库）
    private func restoreJumpSecrets(_ jump: inout JumpServer, secrets: [String: String]) {
        jump.passwordRef = ""
        jump.keyPassphraseRef = ""
        if let password = secrets[BackupSecretKey.jumpPassword(jump.id)], !password.isEmpty {
            let ref = "pwd-\(jump.id)"
            if KeyVault.save(password, account: ref) { jump.passwordRef = ref }
        }
        if let passphrase = secrets[BackupSecretKey.jumpKeyPassphrase(jump.id)], !passphrase.isEmpty {
            let ref = "kpp-\(jump.id)"
            if KeyVault.save(passphrase, account: ref) { jump.keyPassphraseRef = ref }
        }
    }

    // ---- SSH 密钥 ----

    private func applyKeys(_ payload: BackupKeys, strategy: BackupStrategy,
                           tombstones: Set<String>) -> CategoryReport {
        var report = CategoryReport(category: .keys)
        let store = JumpStore.shared
        let secrets = currentSecrets

        if strategy == .overwrite {
            // 清掉本机不在备份内的密钥材料（§5.1 覆盖语义）
            let keepIds = Set(payload.items.compactMap(\.id))
            for old in store.keys where !keepIds.contains(old.id) {
                if !old.privateRef.isEmpty { KeyVault.delete(account: old.privateRef) }
            }
        }

        for (index, item) in payload.items.enumerated() {
            guard var entry = keyEntry(from: item) else {
                report.failed.append("第 \(index + 1) 把密钥：元数据字段不合法")
                continue
            }
            let id = entry.id
            if tombstones.contains("keys|\(id)") {
                if let old = store.key(id), !old.privateRef.isEmpty {
                    KeyVault.delete(account: old.privateRef)
                }
                store.removeKey(id)
                report.skipped += 1
                continue
            }
            if store.key(id) != nil && strategy == .skipExisting {
                report.skipped += 1
                continue
            }
            // 私钥 PEM：没有材料 = 条目照常导入但标注失败原因（SSH 时才会发现）
            entry.privateRef = ""
            if let pem = secrets[BackupSecretKey.keyPrivate(id)], !pem.isEmpty {
                let ref = "priv-\(id)"
                if KeyVault.save(pem, account: ref) {
                    entry.privateRef = ref
                } else {
                    report.failed.append("密钥「\(entry.name)」：私钥写入钥匙串失败，已跳过私钥")
                }
            } else if store.key(id) == nil {
                report.failed.append("密钥「\(entry.name)」：备份中无私钥材料（明文备份？）")
            }
            touch(&entry)
            store.upsert(entry)
            if store.key(id) != nil && strategy == .overwrite { report.updated += 1 } else { report.added += 1 }
        }
        return report
    }

    private func keyEntry(from item: CanonicalKey) -> SshKeyEntry? {
        guard let id = item.id?.trimmed, !id.isEmpty else { return nil }
        var entry = SshKeyEntry()
        entry.id = id
        entry.name = item.name ?? ""
        entry.algorithm = item.algorithm ?? ""
        entry.fingerprint = item.fingerprint ?? ""
        entry.publicKey = item.publicKey ?? ""
        entry.createdAt = BackupDate.date(item.createdAt) ?? Date()
        return entry
    }

    // ---- 偏好设置 ----

    private func applyPrefs(_ prefs: BackupPrefs) -> CategoryReport {
        var report = CategoryReport(category: .terminal)
        let js = JumpStore.shared
        if let dark = prefs.terminalDark { js.setTerminalDark(dark) }
        if let size = prefs.terminalFontSize { js.setTerminalFontSize(size) }
        if let family = prefs.terminalFontFamily { js.setTerminalFontFamily(family) }
        if let auto = prefs.terminalAutoKeyboard { js.setTerminalAutoKeyboard(auto) }
        if let bar = prefs.terminalKeyBar { js.setTerminalKeyBar(bar) }
        if let transport = prefs.transport, transport == ServerStore.transportAuto
            || transport == ServerStore.transportPush || transport == ServerStore.transportPoll {
            ServerStore.shared.setTransport(transport)
        } else if let transport = prefs.transport, !transport.isEmpty {
            // 桌面 ws/http 等跨端值：回落 auto（§2.3 值域过滤）
            ServerStore.shared.setTransport(ServerStore.transportAuto)
        }
        if let connectAll = prefs.connectAll { ServerStore.shared.setConnectAll(connectAll) }
        if let failReload = prefs.failReloadSec { defaults.set(failReload, forKey: "failReloadSec") }
        js.broadcastTerminalSettings()
        report.added = 1
        return report
    }

    // ---- 连接记录 ----

    private func applyHistory(_ payload: BackupHistory, strategy: BackupStrategy) -> CategoryReport {
        var report = CategoryReport(category: .history)
        let js = JumpStore.shared

        // 历史：按 (serverId, startedAt) 去重追加
        var existing = Set(js.history.map { "\($0.serverId)|\($0.startedAt.timeIntervalSince1970)" })
        var merged = js.history
        for session in payload.sessions {
            guard let start = BackupDate.date(session.startedAt),
                  let end = BackupDate.date(session.endedAt) else {
                report.failed.append("会话「\(session.serverName)」：时间字段不合法")
                continue
            }
            let dedupeKey = "\(session.serverId)|\(start.timeIntervalSince1970)"
            if existing.contains(dedupeKey) { report.skipped += 1; continue }
            existing.insert(dedupeKey)
            merged.append(JumpStore.HistoryEntry(id: UUID(), serverId: session.serverId,
                                                 serverName: session.serverName,
                                                 startedAt: start, endedAt: end))
            report.added += 1
        }
        js.setHistory(merged)

        // TOFU 指纹：按 host 覆盖写（iOS 本端动态键 tofu_<host>）
        for host in payload.trustedHosts {
            guard !host.host.isEmpty else { continue }
            defaults.set(host.fingerprint, forKey: "tofu_\(host.host)")
            report.added += 1
        }
        return report
    }

    // MARK: - canonical → 本端模型（含容错）

    private func server(from item: CanonicalServer) -> ServerProfile? {
        guard let id = item.id?.trimmed, !id.isEmpty else { return nil }
        var p = ServerProfile()
        p.id = id
        p.name = item.name ?? ""
        p.group = item.group ?? ""
        p.addresses = (item.addresses ?? []).map(\.trimmed).filter { !$0.isEmpty }
        guard !p.addresses.isEmpty else { return nil }        // 「地址字段不合法」的判定点
        p.token = item.token ?? ""
        p.macs = item.macs ?? [:]
        p.pairs = (item.pairs ?? []).compactMap { pair(from: $0) }
        p.powerRoutes = (item.powerRoutes ?? []).compactMap { powerRoute(from: $0) }
        p.machineId = item.machineId ?? ""
        p.machineName = item.machineName ?? ""
        p.machineOs = ServerProfile.normalizeMachineOs(item.machineOs ?? "")
        p.probeMode = item.probeMode ?? ServerProfile.probeParallel
        p.routeProbeMode = item.routeProbeMode ?? ServerProfile.probeParallelAll
        p.probeTimeoutMs = item.probeTimeoutMs ?? ServerProfile.defaultProbeTimeoutMs
        p.routeTimeoutsMs = ServerProfile.normalizeRouteTimeouts(item.routeTimeoutsMs)
        p.blockProbeMode = item.blockProbeMode ?? [:]
        p.blockTimeoutsMs = item.blockTimeoutsMs ?? [:]
        p.routeOrder = (item.routeOrder ?? []).filter { ServerProfile.isRouteKind($0) }
        p.routeDisabled = (item.routeDisabled ?? []).filter { ServerProfile.isRouteKind($0) }
        p.jumpServerId = item.jumpServerId ?? ""
        p.connMode = item.connMode ?? ""
        p.updatedAt = BackupDate.date(item.updatedAt)
        return p
    }

    private func pair(from item: CanonicalPair) -> RemotePair? {
        guard let id = item.id?.trimmed, !id.isEmpty,
              let kind = item.kind?.trimmed, RemotePair.isBlockId(kind) else { return nil }
        var pair = RemotePair(kind: kind, addr: item.addr ?? "")
        pair.id = id
        pair.lane = item.lane ?? RemotePair.laneFor(kind: kind, addr: pair.addr)
        pair.token = item.token ?? ""
        pair.probeTimeoutMs = item.probeTimeoutMs ?? 0
        pair.enabled = item.enabled ?? true
        pair.mid = item.mid ?? ""
        pair.os = item.os ?? ""
        pair.host = item.host ?? ""
        pair.tunnelId = item.tunnelId ?? ""
        pair.note = item.note ?? ""
        pair.createdAt = item.createdAt ?? Int64(Date().timeIntervalSince1970 * 1000)
        return pair
    }

    private func powerRoute(from item: CanonicalPowerRoute) -> PowerRoute? {
        guard let action = item.action?.trimmed, !action.isEmpty else { return nil }
        var route = PowerRoute()
        route.action = ServerProfile.normalizePowerAction(action)
        route.target = item.target ?? ""
        route.port = item.port ?? 9
        route.macs = item.macs ?? []
        route.enabled = item.enabled ?? true
        route.burst = item.burst ?? 0
        route.retry = item.retry ?? 0
        route.mode = item.mode ?? ""
        route.httpFallback = item.httpFallback ?? false
        return route
    }

    private func jump(from item: CanonicalJump) -> JumpServer? {
        guard let id = item.id?.trimmed, !id.isEmpty else { return nil }
        var j = JumpServer()
        j.id = id
        j.name = item.name ?? ""
        j.protocolRaw = (item.protocol ?? JumpServer.ssh) == JumpServer.telnet ? JumpServer.telnet : JumpServer.ssh
        var host = (item.host ?? "").trimmed
        if host.isEmpty, let first = (item.addresses ?? []).first(where: { !$0.trimmed.isEmpty }) {
            host = first.trimmed
        }
        guard !host.isEmpty else { return nil }
        j.host = host
        j.username = item.username ?? ""
        j.authType = (item.authType ?? JumpServer.authPassword) == JumpServer.authKey
            ? JumpServer.authKey : JumpServer.authPassword
        j.keyId = item.keyId ?? ""
        j.jumpServerId = item.jumpServerId ?? ""
        j.note = item.note ?? ""
        let termPath = item.termPath ?? ""
        j.termPath = termPath.isEmpty ? nil : termPath
        let termShell = item.termShell ?? ""
        j.termShell = termShell.isEmpty ? nil : termShell
        j.lastRttMs = item.lastRttMs ?? -1
        j.lastSeen = BackupDate.date(item.lastSeen)
        j.updatedAt = BackupDate.date(item.updatedAt)
        return j
    }

    // MARK: - 本端模型 → canonical（导出）

    func canonical(_ p: ServerProfile) -> CanonicalServer {
        CanonicalServer(
            id: p.id, name: p.name, group: p.group, addresses: p.addresses, token: p.token,
            macs: p.macs, pairs: p.pairs.map(canonical), powerRoutes: p.powerRoutes.map(canonical),
            machineId: p.machineId, machineName: p.machineName, machineOs: p.machineOs,
            probeMode: p.probeMode, routeProbeMode: p.routeProbeMode, probeTimeoutMs: p.probeTimeoutMs,
            routeTimeoutsMs: p.routeTimeoutsMs, blockProbeMode: p.blockProbeMode,
            blockTimeoutsMs: p.blockTimeoutsMs, routeOrder: p.routeOrder, routeDisabled: p.routeDisabled,
            jumpServerId: p.jumpServerId, connMode: p.connMode,
            updatedAt: p.updatedAt.map(BackupDate.string))
    }

    private func canonical(_ pair: RemotePair) -> CanonicalPair {
        CanonicalPair(id: pair.id, kind: pair.kind, lane: pair.lane, addr: pair.addr, token: pair.token,
                      probeTimeoutMs: pair.probeTimeoutMs, enabled: pair.enabled, mid: pair.mid,
                      os: pair.os, host: pair.host, tunnelId: pair.tunnelId, note: pair.note,
                      createdAt: pair.createdAt, updatedAt: nil)
    }

    private func canonical(_ route: PowerRoute) -> CanonicalPowerRoute {
        CanonicalPowerRoute(action: route.action, target: route.target, port: route.port,
                            macs: route.macs, enabled: route.enabled, burst: route.burst,
                            retry: route.retry, mode: route.mode, httpFallback: route.httpFallback,
                            updatedAt: nil)
    }

    func canonical(_ j: JumpServer) -> CanonicalJump {
        CanonicalJump(id: j.id, name: j.name, protocol: j.protocolRaw, host: j.host,
                      addresses: nil, username: j.username, authType: j.authType, keyId: j.keyId,
                      jumpServerId: j.jumpServerId, note: j.note,
                      termPath: j.termPath, termShell: j.termShell,
                      lastRttMs: j.lastRttMs,
                      lastSeen: j.lastSeen.map(BackupDate.string),
                      updatedAt: j.updatedAt.map(BackupDate.string))
    }

    func canonicalKey(_ entry: SshKeyEntry) -> CanonicalKey {
        CanonicalKey(id: entry.id, name: entry.name, algorithm: entry.algorithm,
                     fingerprint: entry.fingerprint, publicKey: entry.publicKey,
                     createdAt: BackupDate.string(entry.createdAt),
                     updatedAt: entry.updatedAt.map(BackupDate.string))
    }

    // MARK: - updatedAt touch（LWW 地基）

    private func touch(_ p: inout ServerProfile) { p.updatedAt = p.updatedAt ?? Date(); p.updatedAt = Date() }
    private func touch(_ j: inout JumpServer) { j.updatedAt = Date() }
    private func touch(_ k: inout SshKeyEntry) { k.updatedAt = Date() }
}

// 批量落库入口（replaceAll / setHistory）定义在各自 Store 文件内 —— 同文件才能访问 private 存储属性。

// MARK: - 小工具

private extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}
