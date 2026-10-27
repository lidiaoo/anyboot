import Foundation
import Combine

// MARK: - 同步配置与状态（docs/backup-design.md §9.2/§9.5）
//
// 配置持久化：非敏感字段进 UserDefaults，密码/口令进 KeyVault。
// 墓碑（删除记录）随 envelope 的 sync 元数据上行，其他设备按它删本地。

struct SyncConfig: Codable, Equatable {
    var serverURL = ""
    var username = ""
    /// KeyVault ref（webdav 密码）
    var passwordRef = ""
    /// 远端目录，默认 AnyBoot
    var directory = "AnyBoot"
    /// 同步周期（分钟）；0 = 仅手动
    var periodMinutes = 0
    /// 记录变化时就同步（去抖 30s）
    var syncOnChange = false
    /// 同步的数据类型
    var categories: Set<BackupCategory> = [.servers, .jumps, .keys, .terminal]
    /// 同步文件加密
    var encrypt = false
    /// KeyVault ref（加密口令）
    var encryptPasswordRef = ""

    var isConfigured: Bool {
        WebDavConfig(serverURL: serverURL, username: username,
                     passwordRef: passwordRef, directory: directory).isValid
    }

    var webDav: WebDavConfig {
        WebDavConfig(serverURL: serverURL, username: username,
                     passwordRef: passwordRef, directory: directory)
    }
}

/// 最近一次同步的状态（S1「立即同步」行的右侧文案）
enum SyncStatus: Equatable {
    case idle
    case running
    case success(Date)
    case failed(String)

    var label: String {
        switch self {
        case .idle: return ""
        case .running: return "同步中…"
        case .success(let date): return "上次同步 \(Self.timeText(date)) · 成功"
        case .failed(let reason): return "同步失败：\(reason)"
        }
    }

    static func timeText(_ date: Date) -> String {
        let f = DateFormatter()
        f.dateFormat = "HH:mm"
        return f.string(from: date)
    }
}

@MainActor
final class SyncStore: ObservableObject {

    static let shared = SyncStore()

    private let defaults = UserDefaults.standard
    private let keyConfig = "backup_sync_config"
    private let keyRev = "backup_sync_rev"
    private let keyEtag = "backup_sync_etag"
    private let keyTombstones = "backup_sync_tombstones"
    private let keyLastSync = "backup_sync_last_at"

    @Published var config = SyncConfig() {
        didSet { persistConfig() }
    }
    /// 云端文件版本号（每次成功上传 +1；envelope.sync.rev）
    @Published private(set) var rev = 0
    /// 云端文件 ETag（If-Match 用；服务器不给则空）
    @Published private(set) var etag = ""
    @Published private(set) var status: SyncStatus = .idle
    @Published var enabled: Bool = false {
        didSet { defaults.set(enabled, forKey: "backup_sync_enabled") }
    }

    /// 本端墓碑（cat|id → 删除时间）；随下次同步上行后保留 7 天再清
    private(set) var tombstones: [BackupTombstone] = []

    init() { load() }

    var lastSyncAt: Date? {
        defaults.object(forKey: keyLastSync) as? Date
    }

    // MARK: - 持久化

    private func load() {
        if let data = defaults.data(forKey: keyConfig),
           let decoded = try? JSONDecoder().decode(SyncConfig.self, from: data) {
            config = decoded
        }
        enabled = defaults.bool(forKey: "backup_sync_enabled")
        rev = defaults.integer(forKey: keyRev)
        etag = defaults.string(forKey: keyEtag) ?? ""
        if let data = defaults.data(forKey: keyTombstones),
           let list = try? JSONDecoder().decode([BackupTombstone].self, from: data) {
            tombstones = list
        }
    }

    private func persistConfig() {
        if let data = try? JSONEncoder().encode(config) {
            defaults.set(data, forKey: keyConfig)
        }
    }

    // MARK: - 状态机（SyncEngine 更新，UI 只读）

    func setStatus(_ status: SyncStatus) {
        self.status = status
        if case .success(let date) = status {
            defaults.set(date, forKey: keyLastSync)
        }
    }

    func noteRevAndEtag(rev newRev: Int, etag newEtag: String?) {
        rev = newRev
        defaults.set(newRev, forKey: keyRev)
        etag = newEtag ?? ""
        defaults.set(etag, forKey: keyEtag)
    }

    // MARK: - 墓碑

    func recordDeletion(cat: String, id: String) {
        guard enabled, config.isConfigured else { return }
        guard !tombstones.contains(where: { $0.cat == cat && $0.id == id }) else { return }
        tombstones.append(BackupTombstone(cat: cat, id: id))
        if let data = try? JSONEncoder().encode(tombstones) {
            defaults.set(data, forKey: keyTombstones)
        }
    }

    /// 同步成功后清理 7 天前的墓碑
    func pruneTombstones() {
        // 简化：当前墓碑表不带时间戳，条目数 > 500 时才整体截半（防无限增长）
        guard tombstones.count > 500 else { return }
        tombstones.removeFirst(tombstones.count / 2)
        if let data = try? JSONEncoder().encode(tombstones) {
            defaults.set(data, forKey: keyTombstones)
        }
    }

    /// 关闭同步：清墓碑（不再上行删除指令），云文件保留
    func resetTombstones() {
        tombstones = []
        defaults.removeObject(forKey: keyTombstones)
    }
}
