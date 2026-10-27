import Foundation
import Citadel
import NIOCore

/// SFTP 操作层（对齐 Android `SftpOps.java`）。
///
/// 每次操作单开一条 SFTP 通道（与终端 / 隧道互不影响），用完即关。
/// 递归复制用**两条**通道（一条只读、一条只写）—— Citadel 与 jsch 一样，
/// 同一条通道上同时挂着读流和写流会互相把请求队列堵死。
enum SftpOps {

    // MARK: - 数据结构

    /// 目录里的一项（只带列表要用的字段，够排序和展示）
    struct Entry: Identifiable, Hashable {
        var id: String { name }
        let name: String
        let dir: Bool
        /// 软链接单独一个图标 —— 免得看着像目录其实指向别处
        let link: Bool
        let size: Int64
        /// 修改时间，秒（SFTP 原始单位，展示时再 ×1000）
        let mtime: Int64
        /// 形如 `rwxr-xr-x`（不含最前面的类型字符）
        let permissions: String
    }

    /// 「文件详情」弹窗要的全部字段
    struct Detail {
        var path = ""
        var name = ""
        var linkTarget = ""
        var permissions = ""
        var typeLabel = ""
        var dir = false
        var link = false
        var size: Int64 = 0
        var mtime: Int64 = 0
        var atime: Int64 = 0
        var uid: Int64 = 0
        var gid: Int64 = 0
    }

    // MARK: - 通道

    private static func with<T>(_ jump: JumpServer,
                                _ body: (SFTPClient) async throws -> T) async throws -> T {
        let ssh = try await SshSessionManager.shared.client(for: jump)
        let sftp = try await ssh.openSFTP()
        defer { Task { try? await sftp.close() } }
        return try await body(sftp)
    }

    /// 开两条通道：`reader` 只读、`writer` 只写（复制 / 跨设备移动必须分开）
    private static func withTwo<T>(_ jump: JumpServer,
                                   _ body: (SFTPClient, SFTPClient) async throws -> T) async throws -> T {
        let ssh = try await SshSessionManager.shared.client(for: jump)
        let reader = try await ssh.openSFTP()
        defer { Task { try? await reader.close() } }
        let writer = try await ssh.openSFTP()
        defer { Task { try? await writer.close() } }
        return try await body(reader, writer)
    }

    // MARK: - 路径工具（和 TransferCenter.joinRemote 一个口径）

    static func join(_ dir: String, _ name: String) -> String {
        if name.isEmpty { return dir }
        if name.hasPrefix("/") { return name }
        if dir.isEmpty || dir == "/" { return "/" + name }
        return dir.hasSuffix("/") ? dir + name : dir + "/" + name
    }

    static func parent(_ path: String) -> String {
        if path.isEmpty || path == "/" { return "/" }
        let clean = path.hasSuffix("/") && path.count > 1 ? String(path.dropLast()) : path
        guard let index = clean.lastIndex(of: "/") else { return "/" }
        if index == clean.startIndex { return "/" }
        return String(clean[clean.startIndex..<index])
    }

    static func nameOf(_ path: String) -> String {
        let clean = path.hasSuffix("/") && path.count > 1 ? String(path.dropLast()) : path
        guard let index = clean.lastIndex(of: "/") else { return clean }
        return String(clean[clean.index(after: index)...])
    }

    /// 给 shell 用的单引号转义（终端里 cd 过去时用）
    static func shellQuote(_ path: String) -> String {
        "'" + path.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    // MARK: - 读

    /// 列目录（不含 `.` 和 `..`）
    static func list(jump: JumpServer, path: String) async throws -> [Entry] {
        try await with(jump) { sftp in
            var result: [Entry] = []
            let raw = try await sftp.listDirectory(atPath: path)
            for message in raw {
                for item in message.components {
                    var name = item.filename
                    guard name != ".", name != ".." else { continue }
                    // 个别服务器回目录名带尾斜杠，会把排序和路径拼接一起带错
                    while name.count > 1 && name.hasSuffix("/") { name.removeLast() }
                    let attrs = item.attributes
                    let dir = attrs.permissions.map { $0 & 0o40000 != 0 } ?? false
                    let link = attrs.permissions.map { $0 & 0o120000 == 0o120000 } ?? false
                    result.append(Entry(
                        name: name,
                        dir: dir && !link,
                        link: link,
                        size: Int64(attrs.size ?? 0),
                        mtime: Int64(attrs.accessModificationTime?.modificationTime.timeIntervalSince1970 ?? 0),
                        permissions: permissionText(attrs.permissions ?? 0)))
                }
            }
            return result
        }
    }

    /// 取一项的详细信息（软链接会连指向目标一起给）
    static func detail(jump: JumpServer, path: String) async throws -> Detail {
        try await with(jump) { sftp in
            var detail = Detail()
            detail.path = path
            detail.name = nameOf(path)
            // readdir 的属性是 lstat 口径（stat 会跟随软链接），所以软链接判定要先看父目录
            let parentPath = parent(path)
            let listing = try await sftp.listDirectory(atPath: parentPath)
            var found: SFTPFileAttributes?
            var longname = ""
            for message in listing {
                for item in message.components where item.filename == detail.name {
                    found = item.attributes
                    longname = item.longname
                }
            }
            let attrs: SFTPFileAttributes
            if let found {
                attrs = found
            } else {
                attrs = try await sftp.getAttributes(at: path)
            }
            fill(&detail, attrs)
            if detail.link {
                // longname 形如 `lrwxrwxrwx 1 u g 4 Jan 1 00:00 name -> target`
                if let range = longname.range(of: " -> ") {
                    detail.linkTarget = String(longname[range.upperBound...])
                }
            }
            return detail
        }
    }

    private static func fill(_ detail: inout Detail, _ attrs: SFTPFileAttributes) {
        detail.size = Int64(attrs.size ?? 0)
        detail.mtime = Int64(attrs.accessModificationTime?.modificationTime.timeIntervalSince1970 ?? 0)
        detail.atime = Int64(attrs.accessModificationTime?.accessTime.timeIntervalSince1970 ?? 0)
        detail.uid = Int64(attrs.uidgid?.userId ?? 0)
        detail.gid = Int64(attrs.uidgid?.groupId ?? 0)
        detail.permissions = permissionText(attrs.permissions ?? 0)
        let bits = attrs.permissions ?? 0
        detail.dir = bits & 0o40000 != 0
        detail.link = bits & 0o120000 == 0o120000
        if detail.link {
            detail.typeLabel = "软链接"
        } else if detail.dir {
            detail.typeLabel = "目录"
        } else {
            detail.typeLabel = "文件"
        }
    }

    /// 权限位 → `rwxr-xr-x`
    static func permissionText(_ bits: UInt32) -> String {
        guard bits != 0 else { return "" }
        let chars: [Character] = ["r", "w", "x"]
        var out = ""
        for shift in stride(from: 6, through: 0, by: -3) {
            let part = (bits >> UInt32(shift)) & 0o7
            for (index, char) in chars.enumerated() {
                out.append(part & (1 << UInt32(2 - index)) != 0 ? char : "-")
            }
        }
        return out
    }

    /// 把路径解析成服务器上的规范绝对路径；不存在 / 没权限会抛异常（正好当「路径合法吗」的校验）
    static func resolve(jump: JumpServer, path: String) async throws -> String {
        try await with(jump) { sftp in
            try await sftp.getRealPath(atPath: path)
        }
    }

    /// 登录用户的家目录
    static func home(jump: JumpServer) async throws -> String {
        try await with(jump) { sftp in
            try await sftp.getRealPath(atPath: ".")
        }
    }

    // MARK: - 写

    static func mkdir(jump: JumpServer, path: String) async throws {
        try await with(jump) { sftp in
            try await sftp.createDirectory(atPath: path)
        }
    }

    /// 改名 / 移动（同一分区下改名即可，跨设备由调用方走 `move`）
    static func rename(jump: JumpServer, from: String, to: String) async throws {
        try await with(jump) { sftp in
            try await sftp.rename(at: from, to: to)
        }
    }

    /// 删除：目录递归删干净（SFTP 没有 rm -r，只能自己走）
    static func delete(jump: JumpServer, path: String) async throws {
        try await with(jump) { sftp in
            try await removeTree(sftp, path)
        }
    }

    /// 复制（目录递归；目标已存在同名项时由调用方先改名，这里不做覆盖判断）
    static func copy(jump: JumpServer, from: String, to: String) async throws {
        try await withTwo(jump) { reader, writer in
            try await copyTree(reader, writer, from, to)
        }
    }

    /// 移动：先试 rename（同一分区瞬时完成），失败再退化成「复制 + 删源」
    static func move(jump: JumpServer, from: String, to: String) async throws {
        try await withTwo(jump) { reader, writer in
            do {
                try await reader.rename(at: from, to: to)
                return
            } catch {
                // 跨分区时 rename 会失败，这是唯一能走的办法
            }
            try await copyTree(reader, writer, from, to)
            try await removeTree(reader, from)
        }
    }

    /// 目标存在才返回 true（用来决定要不要加「(2)」后缀）
    static func exists(jump: JumpServer, path: String) async throws -> Bool {
        try await with(jump) { sftp in
            do {
                _ = try await sftp.getAttributes(at: path)
                return true
            } catch {
                return false
            }
        }
    }

    // MARK: - 递归

    /// 这一项自己是不是软链接（readdir 的属性是 lstat 口径，stat 会跟随）
    private static func isLink(_ sftp: SFTPClient, _ path: String) async -> Bool {
        let parentPath = parent(path)
        let name = nameOf(path)
        guard let listing = try? await sftp.listDirectory(atPath: parentPath) else { return false }
        for item in listing.flatMap(\.components) where item.filename == name {
            if let perms = item.attributes.permissions { return perms & 0o120000 == 0o120000 }
        }
        return false
    }

    private static func removeTree(_ sftp: SFTPClient, _ path: String) async throws {
        // 软链接当文件删 —— 删链接本身，别把指向的目标也拆了
        if await isLink(sftp, path) {
            try await sftp.remove(at: path)
            return
        }
        let listing: [SFTPMessage.Name]
        do {
            listing = try await sftp.listDirectory(atPath: path)
        } catch {
            // 不是目录（或没权限列）→ 按文件删
            try await sftp.remove(at: path)
            return
        }
        for item in listing.flatMap(\.components) where item.filename != "." && item.filename != ".." {
            try await removeTree(sftp, join(path, item.filename))
        }
        try await sftp.rmdir(at: path)
    }

    /// 递归复制。读走 `reader`、写走 `writer`，**两条通道必须是不同的对象**。
    private static func copyTree(_ reader: SFTPClient, _ writer: SFTPClient,
                                 _ from: String, _ to: String) async throws {
        // 软链接按文件复制（读穿链接拿内容），别跟着递归进目标目录
        let linkOrFile = await isLink(reader, from)
        if !linkOrFile, let listing = try? await reader.listDirectory(atPath: from) {
            let nested = listing.flatMap(\.components)
            let isSelfDir = listing.flatMap(\.components).first { $0.filename == "." }
                .flatMap { $0.attributes.permissions } ?? 0
            if !nested.isEmpty || isSelfDir & 0o40000 != 0 {
                try? await writer.createDirectory(atPath: to)   // 已存在就当目录可用，继续往里拷
                for item in nested where item.filename != "." && item.filename != ".." {
                    try await copyTree(reader, writer, join(from, item.filename), join(to, item.filename))
                }
                return
            }
        }
        let source = try await reader.openFile(filePath: from, flags: .read)
        defer { Task { try? await source.close() } }
        let target = try await writer.openFile(filePath: to, flags: [.write, .create, .truncate])
        defer { Task { try? await target.close() } }
        var offset: UInt64 = 0
        while true {
            let chunk = try await source.read(from: offset, length: 64 * 1024)
            let count = chunk.readableBytes
            if count <= 0 { break }
            try await target.write(chunk, at: offset)
            offset += UInt64(count)
        }
    }

    // MARK: - 出错信息

    /// `"\(error)"` 会走反射，把自定义错误打成 `message("…")` —— 用户看到的就是这行
    /// 括号噪音。有 `errorDescription` 就用它（我们自己的中文错误都实现在那），
    /// 没有才退回系统描述（Citadel / POSIX 的英文原话，下面靠关键词翻译）。
    private static func text(_ error: Error) -> String {
        if let described = (error as? LocalizedError)?.errorDescription, !described.isEmpty {
            return described
        }
        return error.localizedDescription
    }

    /// 把底层英文失败原因换成能看懂的中文（对齐 `SftpOps.reason` 的措辞）
    static func reason(_ error: Error, action: String, path: String) -> String {
        let raw = text(error)
        let lower = raw.lowercased()
        if lower.contains("no such file") || lower.contains("not found") {
            return action + "失败：服务器上找不到「" + path + "」"
        }
        if lower.contains("permission denied") {
            return action + "失败：权限不够（" + path + "）"
        }
        if lower.contains("operation not supported") || lower.contains("unsupported") {
            return action + "失败：这台服务器不支持这个操作"
        }
        if lower.contains("connection") && (lower.contains("lost") || lower.contains("closed") || lower.contains("reset")) {
            return action + "失败：连接断了，重进这一页会自动重连"
        }
        if lower.contains("failure") {
            return action + "失败：服务器拒绝了（" + path + "）"
        }
        return action + "失败：" + (raw.isEmpty ? "未知错误" : raw)
    }

    /// 我们自己抛的（已经是给用户看的中文）原样透出，免得「删除失败：删除失败：…」套娃
    static func describe(_ error: Error, action: String, label: String) -> String {
        let raw = text(error)
        if raw.contains("失败") || raw.contains("不支持") || raw.contains("不能") {
            return raw
        }
        return reason(error, action: action, path: label)
    }
}
