import SwiftUI
import Citadel
import NIOCore

/// 文件页：SFTP 浏览 / 上传下载（仅 SSH；Telnet 服务器进这页提示不支持）。
/// 与 Android 端文件页同构：选服务器 → 面包屑路径 → 目录列表 → 传输。
struct FilesPage: View {

    @ObservedObject private var jumps = JumpStore.shared

    @State private var serverId = ""
    @State private var path = "/"
    @State private var entries: [Entry] = []
    @State private var loading = false
    @State private var message: String?
    @State private var busy = false
    @State private var showImporter = false

    struct Entry: Identifiable {
        var id: String { name }
        let name: String
        let isDir: Bool
        let size: Int
    }

    var body: some View {
        VStack(spacing: 0) {
            Picker("服务器", selection: $serverId) {
                if serverId.isEmpty {
                    Text("（选择 SSH 服务器）").tag("")
                }
                ForEach(jumps.jumps.filter { $0.isSsh }) { jump in
                    Text(jump.displayName).tag(jump.id)
                }
            }
            .pickerStyle(.menu)
            .padding(.horizontal, 12)

            HStack {
                Text(path).font(.caption).lineLimit(1)
                Spacer()
                Button("上级") {
                    if path != "/" {
                        path = (path as NSString).deletingLastPathComponent
                        if path.isEmpty { path = "/" }
                        load()
                    }
                }
                .font(.footnote)
                Button("上传") { showImporter = true }
                    .font(.footnote)
            }
            .padding(.horizontal, 12)

            if loading {
                ProgressView().padding(20)
            }
            List {
                if !loading && entries.isEmpty && !serverId.isEmpty {
                    Text("（空目录）").font(.footnote).foregroundStyle(.secondary)
                }
                ForEach(entries) { entry in
                    HStack(spacing: 10) {
                        Image(systemName: entry.isDir ? "folder.fill" : "doc")
                            .foregroundColor(entry.isDir ? .accentColor : .secondary)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(entry.name)
                            if !entry.isDir {
                                Text(ByteCountFormatter.string(fromByteCount: Int64(entry.size),
                                                               countStyle: .file))
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                    .contentShape(Rectangle())
                    .onTapGesture {
                        if entry.isDir {
                            path = path == "/" ? "/" + entry.name : path + "/" + entry.name
                            load()
                        } else {
                            download(entry)
                        }
                    }
                }
            }
            if busy {
                Text("传输中 …").font(.footnote).foregroundStyle(.secondary)
            }
        }
        .alert("传输", isPresented: Binding(
            get: { message != nil },
            set: { if !$0 { message = nil } })) {
            Button("好") { message = nil }
        } message: {
            Text(message ?? "")
        }
        .fileImporter(isPresented: $showImporter, allowedContentTypes: [.data]) { result in
            if case .success(let url) = result {
                upload(from: url)
            }
        }
        .onAppear {
            if serverId.isEmpty {
                serverId = jumps.jumps.first { $0.isSsh }?.id ?? ""
            }
            if !serverId.isEmpty && entries.isEmpty && !loading {
                load()
            }
        }
        .overlay {
            if jumps.jumps.filter({ $0.isSsh }).isEmpty {
                ContentUnavailableView("没有 SSH 服务器",
                                       systemImage: "server.rack",
                                       description: Text("先到「终端」页添加。Telnet 不支持文件传输。"))
            }
        }
        .onChange(of: serverId) { _ in
            path = "/"
            load()
        }
    }

    private func load() {
        guard let jump = jumps.jump(serverId), jump.isSsh else { return }
        loading = true
        let target = path
        let client = SshSessionManager.shared
        Task {
            do {
                let ssh = try await client.client(for: jump)
                let sftp = try await ssh.openSFTP()
                defer { Task { try? await sftp.close() } }
                var list: [Entry] = []
                let raw = try await sftp.listDirectory(atPath: target)
                for nameMessage in raw {
                    for item in nameMessage.components {
                        let name = item.filename
                        guard name != ".", name != ".." else { continue }
                        let attrs = item.attributes
                        // 当前 Citadel 的 SFTPFileAttributes 没有 isDirectory，
                        // 目录判定走 permissions 的 S_IFDIR 位（0o40000）
                        let isDir = attrs.permissions.map { $0 & 0o40000 != 0 } ?? false
                        list.append(Entry(name: name, isDir: isDir,
                                          size: Int(attrs.size ?? 0)))
                    }
                }
                list.sort { ($0.isDir == $1.isDir ? $0.name < $1.name : $0.isDir) }
                await MainActor.run {
                    if target == path {
                        entries = list
                        loading = false
                    }
                }
            } catch {
                await MainActor.run {
                    loading = false
                    message = "读取失败：\(error.localizedDescription)"
                }
            }
        }
    }

    private func download(_ entry: Entry) {
        guard let jump = jumps.jump(serverId) else { return }
        busy = true
        let remote = path == "/" ? "/" + entry.name : path + "/" + entry.name
        Task {
            do {
                let ssh = try await SshSessionManager.shared.client(for: jump)
                let sftp = try await ssh.openSFTP()
                defer { Task { try? await sftp.close() } }
                let data = try await sftp.withFile(filePath: remote, flags: .read) { file in
                    try await file.readAll()
                }
                var bytes = Data()
                bytes = data.getData(at: 0, length: data.readableBytes) ?? Data()
                let url = FileManager.default
                    .urls(for: .documentDirectory, in: .userDomainMask)[0]
                    .appendingPathComponent(entry.name)
                try bytes.write(to: url)
                await MainActor.run {
                    busy = false
                    message = "已下载到 App 文件目录：\(url.lastPathComponent)"
                }
            } catch {
                await MainActor.run {
                    busy = false
                    message = "下载失败：\(error.localizedDescription)"
                }
            }
        }
    }

    /// 上传（fileImporter 的回调接这里；一期入口放工具条）
    func upload(from url: URL) {
        guard let jump = jumps.jump(serverId) else { return }
        busy = true
        let secured = url.startAccessingSecurityScopedResource()
        Task {
            defer {
                if secured { url.stopAccessingSecurityScopedResource() }
            }
            do {
                let data = try Data(contentsOf: url)
                let ssh = try await SshSessionManager.shared.client(for: jump)
                let sftp = try await ssh.openSFTP()
                defer { Task { try? await sftp.close() } }
                let remote = (path == "/" ? "" : path) + "/" + url.lastPathComponent
                try await sftp.withFile(filePath: remote,
                                        flags: [.write, .create, .truncate]) { file in
                    var buffer = ByteBufferAllocator().buffer(capacity: data.count)
                    buffer.writeBytes(data)
                    try await file.write(buffer)
                }
                await MainActor.run {
                    busy = false
                    message = "上传完成：\(url.lastPathComponent)"
                    load()
                }
            } catch {
                await MainActor.run {
                    busy = false
                    message = "上传失败：\(error.localizedDescription)"
                }
            }
        }
    }
}

// MARK: - 关于页（GitHub 仓库卡片 + Star 引导）

struct AboutView: View {

    @Environment(\.dismiss) private var dismiss

    private static let repoURL = "https://github.com/lidiaoo/anyboot"
    private static let apiURL = "https://api.github.com/repos/lidiaoo/anyboot"

    @State private var stars = "…"
    @State private var forks = "…"
    @State private var issues = "…"

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("AnyBoot 远程客户端")
                            .font(.title2.bold())
                        Text("版本 \(Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.0")")
                            .font(.footnote).foregroundStyle(.secondary)
                    }

                    VStack(alignment: .leading, spacing: 10) {
                        Text("lidiaoo/anyboot")
                            .font(.headline)
                            .foregroundColor(.accentColor)
                        Text("多系统 chroot 管理与 GRUB 引导修复工具 —— 一键进入各系统 / 修复引导，"
                             + "内置远程控制。")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                        HStack(spacing: 16) {
                            Label("\(stars) Star", systemImage: "star")
                            Label("\(forks) Fork", systemImage: "arrow.triangle.branch")
                            Label("\(issues) Issues", systemImage: "exclamationmark.circle")
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)

                        Button {
                            open(Self.repoURL)
                        } label: {
                            Label("★ Star on GitHub", systemImage: "star.fill")
                                .font(.subheadline.weight(.semibold))
                                .frame(maxWidth: .infinity)
                                .padding(.vertical, 10)
                                .background(Color.black)
                                .foregroundColor(.white)
                                .cornerRadius(12)
                        }
                        Text("点 Star 会打开 GitHub 网页，请在网页上点一下 —— "
                             + "这是给开源作者最好的鼓励。")
                            .font(.caption2).foregroundStyle(.secondary)
                    }
                    .padding(16)
                    .background(RoundedRectangle(cornerRadius: 16)
                        .fill(Color(.secondarySystemBackground)))

                    Button("更新日志  ›") { open(Self.repoURL + "/releases") }
                        .foregroundColor(.primary)
                    Button("问题反馈（Issues）  ›") { open(Self.repoURL + "/issues") }
                        .foregroundColor(.primary)
                    Button("开源许可  ›") { open(Self.repoURL + "/blob/main/LICENSE") }
                        .foregroundColor(.primary)
                }
                .padding(20)
            }
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("完成") { dismiss() }
                }
                ToolbarItem(placement: .principal) {
                    Text("关于 AnyBoot").font(.headline)
                }
            }
            .onAppear { loadStats() }
        }
    }

    /// ★/⑂/Issues 实时拉 GitHub API；拉不到用上次缓存
    private func loadStats() {
        if let cached = UserDefaults.standard.string(forKey: "github_cache"),
           let data = cached.data(using: .utf8) {
            apply(data)
        }
        Task {
            guard let url = URL(string: Self.apiURL) else { return }
            var request = URLRequest(url: url)
            request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
            request.timeoutInterval = 6
            guard let (data, response) = try? await URLSession.shared.data(for: request),
                  (response as? HTTPURLResponse)?.statusCode == 200 else { return }
            UserDefaults.standard.set(String(data: data, encoding: .utf8) ?? "",
                                      forKey: "github_cache")
            await MainActor.run { apply(data) }
        }
    }

    private func apply(_ data: Data) {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return }
        stars = "\(object["stargazers_count"] as? Int ?? 0)"
        forks = "\(object["forks_count"] as? Int ?? 0)"
        issues = "\(object["open_issues_count"] as? Int ?? 0)"
    }

    private func open(_ url: String) {
        guard let url = URL(string: url) else { return }
        UIApplication.shared.open(url)
    }
}
