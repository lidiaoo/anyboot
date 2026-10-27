import SwiftUI

// MARK: - 文件 / 目录动作面板（`ServersActivity.kt:2077` file-menu 屏一比一）
//
// 预览卡（图标 / 名称 / 大小·时间）→ 变体动作（文件=下载到本机/重命名/复制路径/删除；
// 目录=重命名/复制路径/删除含全部内容）→ 分隔线后折叠「更多操作」收纳原有全套能力。

struct FileActionSheet: View {

    @ObservedObject private var store = FilesStore.shared
    @ObservedObject private var module = ServerModuleStore.shared

    var body: some View {
        if let entry = store.sheetEntry {
            let path = SftpOps.join(store.path, entry.name)
            let script = entry.name.hasSuffix(".sh") && !entry.dir
            VStack(spacing: 0) {
                Spacer(minLength: 0)
                VStack(alignment: .leading, spacing: 0) {
                    // 对象预览卡
                    HStack(spacing: ABMetric.px(10)) {
                        ZStack {
                            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                                .fill(ABColor.accent.opacity(0.12))
                            if entry.link {
                                Text("🔗").font(.system(size: 18))
                            } else if entry.dir {
                                DrawnIcon.folder(ABColor.accent, size: 20)
                            } else if script {
                                DrawnIcon.bolt(ABColor.warn, size: 20)
                            } else {
                                DrawnIcon.docFile(ABColor.accent, size: 20)
                            }
                        }
                        .frame(width: ABMetric.px(40), height: ABMetric.px(40))
                        VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                            Text(entry.name)
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundColor(script ? ABColor.warn : ABColor.fg)
                                .lineLimit(1)
                            Text((entry.dir ? "目录" : ABFormat.size(entry.size))
                                 + " · " + FilesStore.timeText(entry.mtime))
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundColor(ABColor.fgDim)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        Button {
                            store.sheetEntry = nil
                        } label: {
                            Text("✕")
                                .font(.system(size: 14))
                                .foregroundColor(ABColor.fgDim)
                                .padding(ABMetric.px(8))
                        }
                        .buttonStyle(.plain)
                    }
                    .padding(ABMetric.px(12))
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(16), style: .continuous)
                            .fill(ABColor.base))
                    .overlay(
                        RoundedRectangle(cornerRadius: ABMetric.px(16), style: .continuous)
                            .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))

                    Spacer().frame(height: ABMetric.px(12))

                    if entry.dir {
                        action("重命名") { store.askRename = entry }
                        action("复制路径") { copyPath(path) }
                        action("删除（含目录内全部内容）", color: ABColor.bad) { store.askDelete = entry }
                    } else {
                        action("下载到本机", color: ABColor.accent) { store.startDownload(entry) }
                        action("重命名") { store.askRename = entry }
                        action("复制路径") { copyPath(path) }
                        action("删除", color: ABColor.bad) { store.askDelete = entry }
                    }

                    // 分隔线 + 折叠「更多操作」
                    Rectangle()
                        .fill(ABColor.line)
                        .frame(height: ABMetric.hairline)
                        .padding(.vertical, ABMetric.px(10))
                    Text("更多操作")
                        .font(.system(size: 11))
                        .foregroundColor(ABColor.fgFaint)
                        .padding(.bottom, ABMetric.px(4))
                    action("详情", small: true) { store.askDetail = entry }
                    action("复制", small: true) { store.setClip(.copy, entry) }
                    action("剪切", small: true) { store.setClip(.cut, entry) }
                    if entry.dir && store.clip != .none {
                        action("粘贴到此处", small: true) { store.paste(destDir: store.path) }
                    }
                    if entry.dir {
                        action("在终端打开", small: true) {
                            if let jump = store.server {
                                module.openTerminal(jumpId: jump.id, cdPath: path)
                            }
                        }
                    }
                    action("复制到…", small: true) {
                        store.pick = FilesStore.PickRequest(cut: false, paths: [path],
                                                           label: entry.name, startDir: store.path)
                    }
                    action("移动到…", small: true) {
                        store.pick = FilesStore.PickRequest(cut: true, paths: [path],
                                                           label: entry.name, startDir: store.path)
                    }
                }
                .padding(.horizontal, ABMetric.px(20))
                .padding(.top, ABMetric.px(12))
                .padding(.bottom, ABMetric.px(24))
                .background(
                    UnevenRoundedRectangle(topLeadingRadius: ABMetric.px(20),
                                           topTrailingRadius: ABMetric.px(20),
                                           style: .continuous)
                        .fill(ABColor.card))
            }
            .background(Color.black.opacity(0.42).ignoresSafeArea()
                .onTapGesture { store.sheetEntry = nil })
            .transition(.opacity)
        }
    }

    /// 一行动作（图标 + 文案）；`small` = 「更多操作」组的小字行
    private func action(_ text: String, color: Color? = nil, small: Bool = false,
                        onClick: @escaping () -> Void) -> some View {
        Button {
            store.sheetEntry = nil
            onClick()
        } label: {
            HStack(spacing: ABMetric.px(12)) {
                icon(text, color: color, small: small)
                Text(text)
                    .font(.system(size: small ? 13 : 14,
                                  weight: (!small && color != nil) ? .medium : .regular))
                    .foregroundColor(color ?? ABColor.fg)
                Spacer(minLength: 0)
            }
            .frame(minHeight: ABMetric.px(small ? 38 : 44))
            .padding(.horizontal, ABMetric.px(12))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private func icon(_ text: String, color: Color?, small: Bool) -> some View {
        let size = ABMetric.px(small ? 15 : 18)
        let tint = color ?? ABColor.fgDim
        if text.hasPrefix("下载") {
            DrawnIcon.download(tint, size: size)
        } else if text.hasPrefix("重命名") {
            DrawnIcon.pencil(tint, size: size)
        } else if text.hasPrefix("复制路径") {
            DrawnIcon.copy(tint, size: size)
        } else if text.hasPrefix("删除") {
            DrawnIcon.trash(color ?? ABColor.bad, size: size)
        } else {
            DrawnIcon.docFile(tint, size: size)
        }
    }

    private func copyPath(_ path: String) {
        UIPasteboard.general.string = path
        store.toast("已复制")
    }
}

// MARK: - 文件详情（字段现取，软链接会连指向一起给）—— `:2620`

struct FileDetailDialog: View {

    @ObservedObject private var store = FilesStore.shared

    @State private var rows: [(String, String)]?
    @State private var error = ""
    @State private var loading = false

    var body: some View {
        ABDialogHost(isPresented: store.askDetail != nil,
                     onDismiss: { store.askDetail = nil }) {
            ABDialogCard("文件详情") {
                VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                    if !error.isEmpty {
                        ABDialogNote(error, color: ABColor.bad)
                    } else if let rows {
                        ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                            ABDialogFieldRow(label: row.0, value: row.1)
                        }
                    } else {
                        HStack(spacing: ABMetric.px(8)) {
                            ProgressView().scaleEffect(0.7)
                                .frame(width: ABMetric.px(16), height: ABMetric.px(16))
                            ABSecondaryText("读取中 …")
                        }
                    }
                }
                ABDialogButtons(confirmTitle: "复制路径", dismissTitle: "关闭",
                                onConfirm: {
                                    if let entry = store.askDetail {
                                        UIPasteboard.general.string = SftpOps.join(store.path, entry.name)
                                        store.toast("已复制")
                                    }
                                    store.askDetail = nil
                                },
                                onDismiss: { store.askDetail = nil })
            }
        }
        .onChange(of: store.askDetail?.name) { _, _ in load() }
        .onAppear { load() }
    }

    private func load() {
        guard let entry = store.askDetail, let jump = store.server else {
            rows = nil
            error = ""
            return
        }
        let path = SftpOps.join(store.path, entry.name)
        rows = nil
        error = ""
        Task {
            do {
                let detail = try await SftpOps.detail(jump: jump, path: path)
                var out: [(String, String)] = []
                out.append(("名称", detail.name))
                out.append(("类型", detail.typeLabel))
                out.append(("完整路径", detail.path))
                if !detail.dir {
                    out.append(("大小", ABFormat.size(detail.size)
                                + "（\(detail.size) 字节）"))
                }
                out.append(("修改时间", FilesStore.timeText(detail.mtime)))
                out.append(("访问时间", FilesStore.timeText(detail.atime)))
                out.append(("权限", detail.permissions.isEmpty ? "-" : detail.permissions))
                out.append(("属主", "\(detail.uid) : \(detail.gid)"))
                if !detail.linkTarget.isEmpty {
                    out.append(("指向", detail.linkTarget))
                }
                rows = out
            } catch {
                self.error = SftpOps.reason(error, action: "读取详情", path: entry.name)
            }
        }
    }
}

// MARK: - 重命名 —— `:2694`

struct FileRenameDialog: View {

    @ObservedObject private var store = FilesStore.shared
    @State private var name = ""

    var body: some View {
        ABDialogHost(isPresented: store.askRename != nil,
                     onDismiss: { store.askRename = nil }) {
            ABDialogCard("重命名") {
                if let entry = store.askRename {
                    ABSecondaryText(entry.name)
                    ABDialogInput(placeholder: "新名称", text: $name)
                    ABDialogButtons(confirmTitle: "重命名",
                                    onConfirm: {
                                        let target = name.trimmingCharacters(in: .whitespaces)
                                        if target.isEmpty { store.toast("名称不能为空"); return }
                                        if target.contains("/") { store.toast("名称里不能带 /"); return }
                                        store.askRename = nil
                                        if target != entry.name { store.rename(entry, to: target) }
                                    },
                                    onDismiss: { store.askRename = nil })
                }
            }
        }
        .onChange(of: store.askRename?.name) { _, value in
            name = value ?? ""
        }
    }
}

// MARK: - 删除 —— `:2735`

struct FileDeleteDialog: View {

    @ObservedObject private var store = FilesStore.shared

    var body: some View {
        ABDialogHost(isPresented: store.askDelete != nil,
                     onDismiss: { store.askDelete = nil }) {
            ABDialogCard("删除") {
                if let entry = store.askDelete {
                    Text(entry.dir
                         ? "删除目录「\(entry.name)」？\n里面的东西会一起删掉，删了就找不回来。"
                         : "删除文件「\(entry.name)」？删了就找不回来。")
                        .font(.system(size: 14))
                        .foregroundColor(ABColor.fg)
                        .fixedSize(horizontal: false, vertical: true)
                    ABDialogButtons(confirmTitle: "删除", confirmColor: ABColor.bad,
                                    onConfirm: {
                                        store.askDelete = nil
                                        store.delete(entry)
                                    },
                                    onDismiss: { store.askDelete = nil })
                }
            }
        }
    }
}

// MARK: - 新建目录 —— `:2764`

struct FileMkdirDialog: View {

    @ObservedObject private var store = FilesStore.shared
    @State private var name = ""

    var body: some View {
        ABDialogHost(isPresented: store.mkdirOpen, onDismiss: { store.mkdirOpen = false }) {
            ABDialogCard("新建目录") {
                ABSecondaryText("位置：\(store.path)")
                ABDialogInput(placeholder: "目录名", text: $name)
                ABDialogButtons(confirmTitle: "新建",
                                onConfirm: {
                                    let target = name.trimmingCharacters(in: .whitespaces)
                                    if target.isEmpty { store.toast("目录名不能为空"); return }
                                    if target.contains("/") { store.toast("目录名里不能带 /"); return }
                                    store.mkdirOpen = false
                                    name = ""
                                    store.mkdir(target)
                                },
                                onDismiss: { store.mkdirOpen = false })
            }
        }
    }
}

// MARK: - 定位路径 —— `:2804`

struct FileLocateDialog: View {

    @ObservedObject private var store = FilesStore.shared
    @State private var path = "/"

    var body: some View {
        ABDialogHost(isPresented: store.locateOpen, onDismiss: { store.locateOpen = false }) {
            ABDialogCard("定位路径") {
                ABDialogInput(placeholder: "/home/pi 或 ~/logs", text: $path)
                ABSecondaryText("绝对路径、~ 开头（家目录）、或相对当前目录都行")
                ABDialogButtons(confirmTitle: "前往",
                                onConfirm: {
                                    store.locateOpen = false
                                    store.locate(path)
                                },
                                onDismiss: { store.locateOpen = false })
            }
        }
        .onAppear { path = store.path }
    }
}

// MARK: - 「复制到… / 移动到…」目标目录选择器 —— `:2838`
//
// 只列目录，能往下钻，点「选这里」才动手。

struct DestPickerDialog: View {

    @ObservedObject private var store = FilesStore.shared

    @State private var path = "/"
    @State private var dirs: [SftpOps.Entry] = []
    @State private var busy = true
    @State private var error = ""

    var body: some View {
        ABDialogHost(isPresented: store.pick != nil, onDismiss: { store.pick = nil }) {
            ABDialogCard(store.pick?.cut == true ? "移动到…" : "复制到…") {
                ABSecondaryText(store.pick?.label ?? "")
                HStack(spacing: ABMetric.px(6)) {
                    Text(path)
                        .font(.system(size: 13))
                        .foregroundColor(ABColor.fg)
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    ABChipButton(text: "上级") { path = SftpOps.parent(path) }
                }
                Group {
                    if busy {
                        HStack(spacing: ABMetric.px(8)) {
                            ProgressView().scaleEffect(0.7)
                                .frame(width: ABMetric.px(16), height: ABMetric.px(16))
                            ABSecondaryText("读取中 …")
                        }
                        .padding(.vertical, ABMetric.px(10))
                    } else if !error.isEmpty {
                        ABSecondaryText(error).padding(.vertical, ABMetric.px(8))
                    } else {
                        ScrollView {
                            VStack(alignment: .leading, spacing: 0) {
                                if dirs.isEmpty { ABSecondaryText("（没有子目录）") }
                                ForEach(dirs) { dir in
                                    Button {
                                        path = SftpOps.join(path, dir.name)
                                    } label: {
                                        HStack(spacing: ABMetric.px(8)) {
                                            Text("📁")
                                            Text(dir.name)
                                                .font(.system(size: 14))
                                                .foregroundColor(ABColor.fg)
                                                .lineLimit(1)
                                            Spacer(minLength: 0)
                                        }
                                        .padding(.vertical, ABMetric.px(10))
                                        .contentShape(Rectangle())
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                        }
                        .frame(maxHeight: ABMetric.px(280))
                    }
                }
                ABDialogButtons(confirmTitle: "选这里",
                                onConfirm: {
                                    guard let request = store.pick else { return }
                                    let dest = path
                                    store.pick = nil
                                    store.transferInto(request, destDir: dest)
                                },
                                onDismiss: { store.pick = nil })
            }
        }
        .onChange(of: store.pick?.id) { _, _ in
            path = store.pick?.startDir ?? store.path
            load()
        }
    }

    private func load() {
        guard let jump = store.server else { return }
        let target = path
        busy = true
        error = ""
        dirs = []
        Task {
            do {
                let all = try await SftpOps.list(jump: jump, path: target)
                guard path == target else { return }   // 已经钻到别的目录去了
                busy = false
                dirs = all.filter { $0.dir && !$0.link }
                    .sorted { $0.name.lowercased() < $1.name.lowercased() }
            } catch {
                guard path == target else { return }
                busy = false
                self.error = SftpOps.reason(error, action: "打开", path: target)
            }
        }
    }
}
