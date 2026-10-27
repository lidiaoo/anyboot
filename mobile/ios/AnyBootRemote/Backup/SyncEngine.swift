import Foundation

// MARK: - 同步引擎（docs/backup-design.md §9.3/§9.4）
//
// 流程：pull → merge(LWW 按条目) → push。
// 互斥：同一时刻只允许一次 syncNow（手动与自动共用一把锁）；
// 失败退避：由周期定时器天然兜底，不在引擎里做指数退避。

@MainActor
final class SyncEngine {

    static let shared = SyncEngine()
    private init() {}

    private let store = SyncStore.shared
    private let backup = BackupStore.shared

    /// 互斥标记（@MainActor 串行 + 显式判断，双保险）
    private var running = false
    /// 去抖任务（记录变化 → 30s 后同步）
    private var debounceTask: Task<Void, Never>?
    /// 周期任务
    private var periodTask: Task<Void, Never>?
    private let debounceSeconds: UInt64 = 30

    /// 本设备 id（合并方用于日志/展示；rev 冲突仲裁不依赖它）
    private var deviceId: String {
        if let id = UserDefaults.standard.string(forKey: "backup_device_id") { return id }
        let id = UUID().uuidString
        UserDefaults.standard.set(id, forKey: "backup_device_id")
        return id
    }

    // MARK: - 对外

    /// 立即同步（S1 L6）。返回 nil = 成功；非 nil = 失败原因。
    @discardableResult
    func syncNow() async -> String? {
        guard !running else { return "正在同步，请稍后" }
        guard store.config.isConfigured else { return "请先配置 WebDAV 服务器和同步内容" }
        running = true
        store.setStatus(.running)
        defer {
            running = false
            if case .running = store.status { store.setStatus(.idle) }
        }

        let result = await runOnce()
        switch result {
        case .ok:
            store.setStatus(.success(Date()))
            store.pruneTombstones()
            return nil
        case .fail(let reason):
            store.setStatus(.failed(reason))
            return reason
        }
    }

    /// 测试连接（S2 L9）。返回 nil = 成功；`notFound` 单独返回供「自动创建」弹窗。
    enum TestResult: Equatable {
        case ok
        case directoryMissing
        case failed(String)
    }
    func testConnection() async -> TestResult {
        let client = WebDavClient(config: store.config.webDav)
        switch await client.testConnection() {
        case .success: return .ok
        case .failure(.notFound): return .directoryMissing
        case .failure(let e): return .failed(e.errorDescription ?? "连接失败")
        }
    }

    /// 404 后创建远端目录再复测
    func createRemoteDirectory() async -> String? {
        let client = WebDavClient(config: store.config.webDav)
        switch await client.createDirectory() {
        case .success: return nil
        case .failure(let e): return e.errorDescription
        }
    }

    /// 记录变化触发的去抖同步（Store 落库点调用）
    func notifyChanged() {
        guard store.enabled, store.config.isConfigured, store.config.syncOnChange else { return }
        guard !running else { return }
        debounceTask?.cancel()
        debounceTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: self?.debounceSeconds ?? 30 * 1_000_000_000)
            guard !Task.isCancelled else { return }
            _ = await self?.syncNow()
        }
    }

    /// 启动周期同步（App 启动 / 配置保存后调用）
    func restartPeriodic() {
        periodTask?.cancel()
        guard store.enabled, store.config.isConfigured, store.config.periodMinutes > 0 else { return }
        let minutes = store.config.periodMinutes
        periodTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64(minutes) * 60 * 1_000_000_000)
                guard !Task.isCancelled else { return }
                _ = await self?.syncNow()
            }
        }
    }

    func stopAll() {
        debounceTask?.cancel()
        periodTask?.cancel()
    }

    // MARK: - 单轮拉推

    enum Outcome {
        case ok
        case fail(String)
    }

    private func runOnce() async -> Outcome {
        let cats = store.config.categories
        let client = WebDavClient(config: store.config.webDav)

        // ---- pull ----
        let remoteRaw: Data?
        switch await client.download() {
        case .success(let data):
            remoteRaw = data
        case .failure(let e):
            return .fail(e.errorDescription ?? "拉取失败")
        }

        var remoteRev = store.rev

        if let raw = remoteRaw {
            // 解密 / 解析
            let envelope: BackupEnvelope
            do {
                if backup.peekEncryption(raw: raw) != nil {
                    let password = KeyVault.load(account: store.config.encryptPasswordRef)
                    if password.isEmpty { return .fail("云端备份已加密，请先在本机配置同步口令") }
                    envelope = try backup.decrypt(raw: raw, password: password)
                } else {
                    envelope = try backup.parse(raw: raw)
                }
            } catch {
                return .fail((error as? BackupError)?.errorDescription ?? "云端文件无法解析")
            }
            // merge（只合配置里的分类）
            let merged = merge(from: envelope, categories: cats)
            remoteRev = envelope.sync?.rev ?? store.rev
            if !merged.localChanged { return .ok }   // 双端一致，不必推
        }

        // ---- push ----
        let password = store.config.encrypt ? KeyVault.load(account: store.config.encryptPasswordRef) : nil
        if store.config.encrypt && (password ?? "").isEmpty {
            return .fail("已开启同步加密但未设置口令")
        }
        let payload: Data
        do {
            payload = try backup.serialize(try collectedEnvelope(remoteRev + 1), password: password)
        } catch {
            return .fail("组包失败：\((error as NSError).localizedDescription)")
        }
        switch await client.upload(payload, ifMatchETag: store.etag.isEmpty ? nil : store.etag) {
        case .success(let newEtag):
            store.noteRevAndEtag(rev: remoteRev + 1, etag: newEtag)
            return .ok
        case .failure(.conflict):
            // 云端被其他设备更新过：再拉一轮合并后重推一次（一轮即可，防活锁）
            return await resolveConflictAndPush(client: client, categories: cats)
        case .failure(let e):
            return .fail(e.errorDescription ?? "上传失败")
        }
    }

    /// collect 的包装（组 sync 元数据）——抽出便于 conflict 路径复用
    private func collectedEnvelope(_ newRev: Int) throws -> BackupEnvelope {
        try backup.collect(categories: store.config.categories, sync: BackupSyncMeta(
            deviceId: deviceId, rev: newRev, tombstones: store.tombstones))
    }

    private func resolveConflictAndPush(client: WebDavClient,
                                        categories: Set<BackupCategory>) async -> Outcome {
        // 重新拉
        let raw: Data?
        switch await client.download() {
        case .success(let d): raw = d
        case .failure(let e): return .fail(e.errorDescription ?? "拉取失败")
        }
        var remoteRev = store.rev
        if let raw {
            do {
                let envelope = try decryptIfNeeded(raw)
                remoteRev = envelope.sync?.rev ?? store.rev
                _ = merge(from: envelope, categories: categories)
            } catch {
                return .fail((error as? BackupError)?.errorDescription ?? "云端文件无法解析")
            }
        }
        let payload: Data
        do {
            let password = store.config.encrypt ? KeyVault.load(account: store.config.encryptPasswordRef) : nil
            payload = try backup.serialize(try collectedEnvelope(remoteRev + 1), password: password)
        } catch {
            return .fail("组包失败：\((error as NSError).localizedDescription)")
        }
        switch await client.upload(payload, ifMatchETag: nil) {
        case .success(let newEtag):
            store.noteRevAndEtag(rev: remoteRev + 1, etag: newEtag)
            return .ok
        case .failure(let e):
            return .fail(e.errorDescription ?? "上传失败")
        }
    }

    private func decryptIfNeeded(_ raw: Data) throws -> BackupEnvelope {
        if backup.peekEncryption(raw: raw) != nil {
            return try backup.decrypt(raw: raw, password: KeyVault.load(account: store.config.encryptPasswordRef))
        }
        return try backup.parse(raw: raw)
    }

    // MARK: - LWW 合并（§9.3 语义表）

    struct MergeOutcome {
        var localChanged = false
        var added = 0
        var updated = 0
        var skipped = 0
    }

    private func merge(from envelope: BackupEnvelope, categories: Set<BackupCategory>) -> MergeOutcome {
        var outcome = MergeOutcome()
        let remoteTombstones = Set((envelope.sync?.tombstones ?? []).map { "\($0.cat)|\($0.id)" })

        if categories.contains(.servers), let payload = envelope.data?.servers {
            let r = mergeServers(payload, tombstones: remoteTombstones)
            absorb(&outcome, r)
        }
        if categories.contains(.jumps), let payload = envelope.data?.jumps {
            let r = mergeJumps(payload, tombstones: remoteTombstones,
                               secrets: envelope.data?.keys?.secrets ?? [:])
            absorb(&outcome, r)
        }
        if categories.contains(.keys), let payload = envelope.data?.keys {
            let r = mergeKeys(payload, tombstones: remoteTombstones, secrets: payload.secrets)
            absorb(&outcome, r)
        }
        if categories.contains(.terminal), let prefs = envelope.data?.terminal {
            // 偏好：远端整体胜（无 updatedAt，收敛到「最后同步者」口径）
            let r = backup.applyPrefsForSync(prefs)
            outcome.localChanged = true
            outcome.updated += 1
            _ = r
        }
        if categories.contains(.history), let payload = envelope.data?.history {
            let r = backup.applyHistoryForSync(payload, strategy: .skipExisting)
            if r.added > 0 { outcome.localChanged = true; outcome.added += r.added }
        }
        return outcome
    }

    private func absorb(_ outcome: inout MergeOutcome, _ r: MergeOutcome) {
        if r.localChanged { outcome.localChanged = true }
        outcome.added += r.added
        outcome.updated += r.updated
        outcome.skipped += r.skipped
    }

    private func mergeServers(_ payload: BackupServers, tombstones: Set<String>) -> MergeOutcome {
        var outcome = MergeOutcome()
        let storeS = ServerStore.shared
        for item in payload.items {
            guard let id = item.id, !id.isEmpty else { continue }
            if tombstones.contains("servers|\(id)") {
                if storeS.profile(id) != nil { storeS.remove(id); outcome.localChanged = true }
                outcome.skipped += 1
                continue
            }
            guard let remote = backup.serverFrom(item) else { continue }
            if let local = storeS.profile(id) {
                if isNewer(remote.updatedAt, than: local.updatedAt) {
                    var incoming = remote
                    incoming.updatedAt = incoming.updatedAt ?? Date()   // 远端无时间戳：视为更新
                    storeS.upsert(incoming)
                    outcome.localChanged = true
                    outcome.updated += 1
                }
            } else {
                var incoming = remote
                incoming.updatedAt = incoming.updatedAt ?? Date()
                storeS.upsert(incoming)
                outcome.localChanged = true
                outcome.added += 1
            }
        }
        // 本机被删但远端还活着 → 记墓碑（下次推给其他设备）
        let remoteIds = Set((payload.items).compactMap(\.id))
        for local in storeS.servers where !remoteIds.contains(local.id) {
            if !tombstones.contains("servers|\(local.id)") {
                store.recordDeletion(cat: "servers", id: local.id)
            }
        }
        return outcome
    }

    private func mergeJumps(_ payload: BackupJumps, tombstones: Set<String>,
                            secrets: [String: String]) -> MergeOutcome {
        var outcome = MergeOutcome()
        let js = JumpStore.shared
        for item in payload.items {
            guard let id = item.id, !id.isEmpty else { continue }
            if tombstones.contains("jumps|\(id)") {
                if let local = js.jump(id) { js.remove(local.id); outcome.localChanged = true }
                outcome.skipped += 1
                continue
            }
            guard var incoming = backup.jumpFrom(item) else { continue }
            if let local = js.jump(id) {
                if isNewer(incoming.updatedAt, than: local.updatedAt) {
                    backup.restoreJumpSecretsForSync(&incoming, secrets: secrets)
                    js.upsert(incoming)
                    outcome.localChanged = true
                    outcome.updated += 1
                }
            } else {
                backup.restoreJumpSecretsForSync(&incoming, secrets: secrets)
                incoming.updatedAt = incoming.updatedAt ?? Date()
                js.upsert(incoming)
                outcome.localChanged = true
                outcome.added += 1
            }
        }
        let remoteIds = Set(payload.items.compactMap(\.id))
        for local in js.jumps where !remoteIds.contains(local.id) {
            if !tombstones.contains("jumps|\(local.id)") {
                store.recordDeletion(cat: "jumps", id: local.id)
            }
        }
        return outcome
    }

    private func mergeKeys(_ payload: BackupKeys, tombstones: Set<String>,
                           secrets: [String: String]) -> MergeOutcome {
        var outcome = MergeOutcome()
        let js = JumpStore.shared
        for item in payload.items {
            guard let id = item.id, !id.isEmpty else { continue }
            if tombstones.contains("keys|\(id)") {
                if let local = js.key(id) {
                    if !local.privateRef.isEmpty { KeyVault.delete(account: local.privateRef) }
                    js.removeKey(id)
                    outcome.localChanged = true
                }
                outcome.skipped += 1
                continue
            }
            guard var incoming = backup.keyEntryFrom(item) else { continue }
            if let local = js.key(id) {
                if isNewer(incoming.updatedAt, than: local.updatedAt) {
                    incoming.privateRef = local.privateRef
                    // 远端带来私钥材料且本机没有 → 写入钥匙串
                    if (local.privateRef.isEmpty), let pem = secrets[BackupSecretKey.keyPrivate(id)], !pem.isEmpty {
                        let ref = "priv-\(id)"
                        if KeyVault.save(pem, account: ref) { incoming.privateRef = ref }
                    }
                    js.upsert(incoming)
                    outcome.localChanged = true
                    outcome.updated += 1
                }
            } else {
                incoming.privateRef = ""
                if let pem = secrets[BackupSecretKey.keyPrivate(id)], !pem.isEmpty {
                    let ref = "priv-\(id)"
                    if KeyVault.save(pem, account: ref) { incoming.privateRef = ref }
                }
                incoming.updatedAt = incoming.updatedAt ?? Date()
                js.upsert(incoming)
                outcome.localChanged = true
                outcome.added += 1
            }
        }
        let remoteIds = Set(payload.items.compactMap(\.id))
        for local in js.keys where !remoteIds.contains(local.id) {
            if !tombstones.contains("keys|\(local.id)") {
                store.recordDeletion(cat: "keys", id: local.id)
            }
        }
        return outcome
    }

    /// LWW：nil 时间视为「极旧」；相等视为不更新
    private func isNewer(_ a: Date?, than b: Date?) -> Bool {
        switch (a, b) {
        case let (lhs?, rhs?): return lhs > rhs
        case (.some, .none): return true
        default: return false
        }
    }
}
