import Foundation

// MARK: - 备份信封（docs/backup-design.md §3）

/// 备份分类 id（`includes` 数组与 `data` 键名、`sync.tombstones[].cat` 都用它）
enum BackupCategory: String, CaseIterable, Identifiable, Codable {
    case servers        // 电脑列表
    case jumps          // SSH 服务器
    case keys           // SSH 密钥
    case terminal       // 偏好设置（终端 5 项 + transport + connect_all + failReloadSec）
    case history        // 连接记录（会话历史 + TOFU 指纹）

    var id: String { rawValue }

    /// UI 文案（docs/backup-design.md §6 逐字）
    var label: String {
        switch self {
        case .servers: return "电脑列表"
        case .jumps: return "SSH 服务器"
        case .keys: return "SSH 密钥"
        case .terminal: return "偏好设置"
        case .history: return "连接记录"
        }
    }
}

/// 信封顶层。明文与加密同结构：加密时 `data == nil`、`payload != nil`。
struct BackupEnvelope: Codable {
    var format: String = "anyboot-backup"
    var version: Int = BackupConstants.version
    var app: String = ""
    var platform: String = "ios"
    /// ISO8601
    var exportedAt: String = ""
    var includes: [String] = []
    /// 仅云同步文件携带；手工导出恒 nil
    var sync: BackupSyncMeta?
    var encryption: BackupEncryption?
    var data: BackupData?
    var payload: String?

    var isEncrypted: Bool { payload != nil }
    /// 远端版本比本端新 → 友好拒绝（§3 version 兼容策略）
    var isFromNewerApp: Bool { version > BackupConstants.version }
}

enum BackupConstants {
    static let version = 1
    static let format = "anyboot-backup"
    static let maxFileBytes = 10 * 1024 * 1024   // 10 MB 防御（§7）
    static let pbkdf2Rounds = 310_000
}

/// 云同步元数据（§9.2）
struct BackupSyncMeta: Codable {
    var deviceId: String
    var rev: Int
    var tombstones: [BackupTombstone]
}

struct BackupTombstone: Codable, Hashable {
    var cat: String
    var id: String
}

/// 加密信封（§3.2）。AAD 覆盖除 payload 外的关键参数，防篡改 rounds/version。
struct BackupEncryption: Codable {
    var scheme: String = "PBKDF2-SHA256-310000 + AES-256-GCM"
    var salt: String      // base64, 16B
    var iv: String        // base64, 12B
    var rounds: Int = BackupConstants.pbkdf2Rounds

    /// AAD（三端逐字节一致的确定性串，见 BackupCrypto.aad）
    var aadSeed: String {
        "anyboot-backup|v1|\(scheme)|\(salt)|\(iv)|\(rounds)"
    }
}

// MARK: - data 各分类壳

struct BackupData: Codable {
    var servers: BackupServers?
    var jumps: BackupJumps?
    var keys: BackupKeys?
    var terminal: BackupPrefs?
    var history: BackupHistory?
}

struct BackupServers: Codable {
    var items: [CanonicalServer] = []
    var activeId: String = ""
}

struct BackupJumps: Codable {
    var items: [CanonicalJump] = []
}

struct BackupKeys: Codable {
    var items: [CanonicalKey] = []
    /// 稳定键明文表：`key:<id>:private` / `key:<id>:passphrase` / `jump:<id>:password` / `jump:<id>:keyPassphrase`
    /// 只出现在加密备份里（明文备份勾密钥由导入端强警示）。
    var secrets: [String: String] = [:]
}

struct BackupPrefs: Codable {
    var terminalDark: Bool?
    var terminalFontSize: Int?
    var terminalFontFamily: String?
    var terminalAutoKeyboard: Bool?
    var terminalKeyBar: Bool?
    var transport: String?
    var connectAll: Bool?
    var failReloadSec: Int?
}

struct BackupHistory: Codable {
    var sessions: [CanonicalSession] = []
    var trustedHosts: [CanonicalTrustedHost] = []
}

struct CanonicalSession: Codable {
    var serverId: String
    var serverName: String
    /// ISO8601
    var startedAt: String
    var endedAt: String
}

struct CanonicalTrustedHost: Codable, Hashable {
    var host: String
    var fingerprint: String
}

// MARK: - canonical 记录（字段键 = docs/backup-design.md §2.3 三端矩阵）

/// 电脑档案。全字段可选 → 低版本/缺字段备份照常解码（try-per-item 的地基）。
struct CanonicalServer: Codable {
    var id: String?
    var name: String?
    var group: String?
    var addresses: [String]?
    var token: String?
    var macs: [String: [String]]?
    var pairs: [CanonicalPair]?
    var powerRoutes: [CanonicalPowerRoute]?
    var machineId: String?
    var machineName: String?
    var machineOs: String?
    var probeMode: String?
    var routeProbeMode: String?
    var probeTimeoutMs: Int?
    var routeTimeoutsMs: [String: Int]?
    var blockProbeMode: [String: String]?
    var blockTimeoutsMs: [String: Int]?
    var routeOrder: [String]?
    var routeDisabled: [String]?
    var jumpServerId: String?
    /// 废弃字段，仅旧备份兼容（canonical 保留）
    var connMode: String?
    /// 同步 LWW 的依据（ISO8601）
    var updatedAt: String?
}

/// 配对条目：幂等键 (mid, kind, lane) 三端一致（§2.3）。桌面多 note/created_at，移动端忽略。
struct CanonicalPair: Codable {
    var id: String?
    var kind: String?
    var lane: String?
    var addr: String?
    var token: String?
    var probeTimeoutMs: Int?
    var enabled: Bool?
    var mid: String?
    var os: String?
    var host: String?
    var tunnelId: String?
    var note: String?
    var createdAt: Int64?
    var updatedAt: String?
}

struct CanonicalPowerRoute: Codable {
    var action: String?
    var target: String?
    var port: Int?
    var macs: [String]?
    var enabled: Bool?
    var burst: Int?
    var retry: Int?
    var mode: String?
    var httpFallback: Bool?
    var updatedAt: String?
}

/// SSH 服务器（canonical：`protocol` 不是 Swift 关键字冲突，编码用 `protocol` 键）
struct CanonicalJump: Codable {
    var id: String?
    var name: String?
    /// ssh / telnet —— canonical 键名 `protocol`（iOS 本端叫 protocolRaw，适配）
    var `protocol`: String?
    var host: String?
    /// 桌面原生结构化地址；移动端导出拆自 host，导入时非空则回填
    var addresses: [String]?
    var username: String?
    var authType: String?
    var keyId: String?
    var jumpServerId: String?
    var note: String?
    var termPath: String?
    var termShell: String?
    var lastRttMs: Double?
    /// ISO8601（Android 毫秒 → 导入端归一）
    var lastSeen: String?
    var updatedAt: String?

    enum CodingKeys: String, CodingKey {
        case id, name, host, addresses, username
        case `protocol` = "protocol"
        case authType, keyId, jumpServerId, note, termPath, termShell
        case lastRttMs, lastSeen, updatedAt
    }
}

/// SSH 密钥元数据（私钥/口令本体走 `BackupKeys.secrets`，不在这里）
struct CanonicalKey: Codable {
    var id: String?
    var name: String?
    var algorithm: String?
    var fingerprint: String?
    var publicKey: String?
    /// ISO8601
    var createdAt: String?
    var updatedAt: String?
}

// MARK: - secrets 稳定键（§3.1）

enum BackupSecretKey {
    static func keyPrivate(_ keyId: String) -> String { "key:\(keyId):private" }
    static func keyPassphrase(_ keyId: String) -> String { "key:\(keyId):passphrase" }
    static func jumpPassword(_ jumpId: String) -> String { "jump:\(jumpId):password" }
    static func jumpKeyPassphrase(_ jumpId: String) -> String { "jump:\(jumpId):keyPassphrase" }
}

// MARK: - 日期工具（ISO8601 统一口径）

enum BackupDate {
    private static let formatter: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()

    static func string(_ date: Date) -> String {
        formatter.string(from: date)
    }

    static func date(_ string: String?) -> Date? {
        guard let string, !string.isEmpty else { return nil }
        return formatter.date(from: string)
    }
}

// MARK: - 导入报告

/// 逐分类的执行结果（结果报告页与同步报告共用）
struct CategoryReport {
    var category: BackupCategory
    var added = 0
    var updated = 0
    var skipped = 0
    var failed: [String] = []      // 逐条原因，如「第 3 台电脑：地址字段不合法」
    /// 缺失引用提示（电脑→跳板机 / 服务器→密钥）
    var missingRefs: [String] = []
}

struct ImportReport {
    var categories: [CategoryReport] = []
    var totalAdded: Int { categories.reduce(0) { $0 + $1.added } }
    var totalUpdated: Int { categories.reduce(0) { $0 + $1.updated } }
    var totalSkipped: Int { categories.reduce(0) { $0 + $1.skipped } }
    var totalFailed: Int { categories.reduce(0) { $0 + $1.failed.count } }
    var isEmpty: Bool { categories.isEmpty }
}

/// 导入前摘要（预览页 S3 的数据源）
struct BackupSummary {
    var platform: String = ""
    var exportedAt: String = ""
    var app: String = ""
    var isEncrypted = false
    /// 每分类：（备份条数，本机条数，id 冲突明细）
    var rows: [BackupCategory: CategorySummary] = [:]
    /// 备份里电脑关联的跳板机 id 不在备份 jumps 里的数量
    var missingJumpRefs = 0

    struct CategorySummary {
        var backupCount = 0
        var localCount = 0
        /// id 冲突对象（覆盖/跳过的对象名），预览明细展开用
        var conflicts: [String] = []
        var available: Bool { true }
    }
}
