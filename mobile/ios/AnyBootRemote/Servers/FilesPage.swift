import SwiftUI
import UniformTypeIdentifiers

/// 文件页（对齐 `ServersActivity.kt` 的 FilesPage）：服务器下拉 + 路径面包屑 + 工具行 +
/// 三列表格 + 动作面板/详情/重命名/删除/新建/定位/目标目录选择器。
///
/// 传输不在这页自己搬字节 —— 交给 `TransferCenter`（流式分块 + 断点续传 + 落盘恢复），
/// 这页只负责发起，进度去「传输」页看。
struct FilesPage: View {

    @ObservedObject private var store = FilesStore.shared
    @ObservedObject private var module = ServerModuleStore.shared

    @State private var showImporter = false
    @State private var serverMenuOpen = false
    @State private var sortMenuOpen = false
    /// 刚自动刷新过的那次上传（防重复拉取 —— Android lastSeenUpload 同款）
    @State private var lastSeenUploadId = ""

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if store.server == nil {
                    ABEmptyHint("没有 SSH 服务器 —— 先到「服务器」页添加。Telnet 不支持文件传输。")
                        .padding(.top, ABMetric.px(12))
                } else {
                    serverPicker
                    pathBar
                    toolRow
                    if store.filterOpen { filterField }
                    if !store.opLabel.isEmpty { opRow }
                    if store.clip != .none { clipRow }
                    if !store.isUp {
                        ABSecondaryText("　（先在「服务器」页连上这台服务器，或点行内「终端」—— SFTP 复用同一条 SSH 会话）")
                    }
                    if store.error.isEmpty && !store.busy && !store.entries.isEmpty { tableHeader }
                    listing
                }
            }
            .padding(.horizontal, ABMetric.px(20))
            .padding(.bottom, ABMetric.px(24))
        }
        .background(ABColor.base)
        .onAppear {
            store.ensureServer()
            store.loadIfNeeded()
        }
        // 刚完成的上传落在当前目录 → 自动刷新列表（ServersActivity:2006-2018）
        .onChange(of: module.transferTick) { _, _ in
            guard let serverId = store.server?.id else { return }
            let fresh = TransferCenter.shared.tasks.first { task in
                task.direction == .upload
                    && task.state == .done
                    && task.serverId == serverId
                    && task.remoteDir == store.path
                    && task.id != lastSeenUploadId
            }
            guard let upload = fresh else { return }
            lastSeenUploadId = upload.id
            store.reloadDir()
        }
        .fileImporter(isPresented: $showImporter,
                      allowedContentTypes: [.data, .archive, .image],
                      allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { store.upload(urls) }
        }
        .overlay { dialogs }
    }

    // MARK: - 服务器下拉

    private var serverPicker: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                serverMenuOpen.toggle()
            } label: {
                HStack(spacing: ABMetric.px(8)) {
                    Text(store.server.map { $0.name.isEmpty ? $0.addressLabel : $0.name } ?? "")
                        .font(.system(size: 13))
                        .foregroundColor(ABColor.fg)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    Text(serverMenuOpen ? "▴" : "▾")
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.fgDim)
                }
                .padding(.horizontal, ABMetric.px(12))
                .padding(.vertical, ABMetric.px(10))
                .background(
                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .fill(ABColor.base))
                .overlay(
                    RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                        .strokeBorder(ABColor.lineStrong, lineWidth: ABMetric.hairline))
            }
            .buttonStyle(.plain)
            .overlay(alignment: .topLeading) {
                Text("服务器")
                    .font(.system(size: 9))
                    .foregroundColor(ABColor.fgFaint)
                    .padding(.horizontal, ABMetric.px(4))
                    .background(ABColor.base)
                    .offset(x: ABMetric.px(10), y: -ABMetric.px(5))
            }
            .overlay(alignment: .topTrailing) {
                if serverMenuOpen {
                    menu
                        .offset(y: ABMetric.px(44))
                }
            }
        }
        .padding(.vertical, ABMetric.px(4))
        .zIndex(2)
    }

    private var menu: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(store.sshServers) { item in
                Button {
                    serverMenuOpen = false
                    if item.id != store.serverId {
                        store.serverId = item.id
                        store.clearClip()
                        store.gotoDir("/", force: true)
                    }
                } label: {
                    Text(item.name.isEmpty ? item.addressLabel : item.name)
                        .font(.system(size: 13))
                        .foregroundColor(ABColor.fg)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, ABMetric.px(14))
                        .padding(.vertical, ABMetric.px(10))
                }
                .buttonStyle(.plain)
            }
        }
        .frame(width: ABMetric.px(230))
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.elevate))
        .overlay(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
        .shadow(color: .black.opacity(0.18), radius: 12, y: 6)
    }

    // MARK: - 路径面包屑

    private var pathBar: some View {
        // 长路径时当前目录段保持可见（ServersActivity:2041-2043 scrollToItem 注释同款）
        ScrollViewReader { proxy in
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 0) {
                    let segments = FilesStore.pathSegments(store.path)
                    ForEach(Array(segments.enumerated()), id: \.offset) { index, segment in
                        if index > 0 {
                            Text("/")
                                .font(.system(size: 13))
                                .foregroundColor(ABColor.fgFaint)
                        }
                        Button {
                            store.gotoDir(FilesStore.pathUpTo(segments, index))
                        } label: {
                            Text(segment)
                                .font(.system(size: 13))
                                .foregroundColor(index == segments.count - 1 ? ABColor.fg : ABColor.accent)
                                .lineLimit(1)
                                .padding(.horizontal, ABMetric.px(6))
                                .padding(.vertical, ABMetric.px(4))
                        }
                        .buttonStyle(.plain)
                        .id(index)
                    }
                }
            }
            .onChange(of: store.path) { _, newValue in
                let count = FilesStore.pathSegments(newValue).count
                guard count > 0 else { return }
                withAnimation(.easeInOut(duration: 0.15)) {
                    proxy.scrollTo(count - 1, anchor: .trailing)
                }
            }
        }
        .padding(.horizontal, ABMetric.px(10))
        .padding(.vertical, ABMetric.px(4))
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.card))
        .overlay(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
        .padding(.vertical, ABMetric.px(6))
    }

    // MARK: - 工具行

    private var toolRow: some View {
        ABFlow(itemSpacing: ABMetric.px(6), rowSpacing: ABMetric.px(6)) {
            ABChipButton(text: "上级") { store.gotoDir(SftpOps.parent(store.path)) }
            ABChipButton(text: "上传") { showImporter = true }
            ABChipButton(text: "＋ 新建目录") { store.mkdirOpen = true }
            ABChipButton(text: "刷新") { store.reloadDir() }
            ABChipButton(text: "路径") { store.locateOpen = true }
            ABChipButton(text: "终端") { openTerminal(at: store.path) }

            Button {
                sortMenuOpen.toggle()
            } label: {
                Text("排序 \(store.sort.label)" + (store.sortDesc ? " ↓" : " ↑"))
                    .font(.system(size: 12))
                    .foregroundColor(ABColor.accent)
                    .padding(.horizontal, ABMetric.px(10))
                    .padding(.vertical, ABMetric.px(6))
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .fill(ABColor.elevate))
            }
            .buttonStyle(.plain)
            .overlay(alignment: .topLeading) {
                if sortMenuOpen {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(FilesStore.SortKey.allCases, id: \.self) { key in
                            Button {
                                store.sort = key
                                sortMenuOpen = false
                            } label: {
                                Text(key.label + (store.sort == key ? "　✓" : ""))
                                    .font(.system(size: 13))
                                    .foregroundColor(ABColor.fg)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.horizontal, ABMetric.px(14))
                                    .padding(.vertical, ABMetric.px(10))
                            }
                            .buttonStyle(.plain)
                        }
                        Button {
                            store.sortDesc.toggle()
                            sortMenuOpen = false
                        } label: {
                            Text(store.sortDesc ? "改为升序 ↑" : "改为降序 ↓")
                                .font(.system(size: 13))
                                .foregroundColor(ABColor.fg)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, ABMetric.px(14))
                                .padding(.vertical, ABMetric.px(10))
                        }
                        .buttonStyle(.plain)
                    }
                    .frame(width: ABMetric.px(150))
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .fill(ABColor.elevate))
                    .overlay(
                        RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                            .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
                    .shadow(color: .black.opacity(0.18), radius: 12, y: 6)
                    .offset(y: ABMetric.px(34))
                }
            }
            .zIndex(1)

            ABChipButton(text: "显示隐藏", selected: store.showHidden) {
                store.showHidden.toggle()
            }
            ABChipButton(text: "过滤", selected: store.filterOpen) {
                store.filterOpen.toggle()
                if !store.filterOpen { store.filter = "" }
            }
        }
        .padding(.vertical, ABMetric.px(4))
    }

    private var filterField: some View {
        ABDialogInput(placeholder: "按名称过滤（不区分大小写），清空就全显示", text: $store.filter)
            .padding(.vertical, ABMetric.px(2))
    }

    private var opRow: some View {
        HStack(spacing: ABMetric.px(8)) {
            ProgressView().scaleEffect(0.6).frame(width: ABMetric.px(14), height: ABMetric.px(14))
            ABSecondaryText("正在\(store.opLabel) …")
        }
        .padding(.top, ABMetric.px(6))
    }

    private var clipRow: some View {
        HStack(spacing: ABMetric.px(6)) {
            Text((store.clip == .copy ? "📋 已复制 " : "✂️ 已剪切 ") + store.clipNames)
                .font(.system(size: 12))
                .foregroundColor(ABColor.fgDim)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            ABChipButton(text: "粘贴到当前目录") { store.paste(destDir: store.path) }
            ABChipButton(text: "取消") { store.clearClip() }
        }
        .padding(.vertical, ABMetric.px(6))
    }

    private var tableHeader: some View {
        HStack(spacing: 0) {
            Text("名称")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text("大小")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
                .frame(width: ABMetric.px(64), alignment: .leading)
            Text("修改时间")
                .font(.system(size: 10))
                .foregroundColor(ABColor.fgFaint)
                .frame(width: ABMetric.px(72), alignment: .trailing)
            Spacer().frame(width: ABMetric.px(30))
        }
        .padding(.horizontal, ABMetric.px(8))
        .padding(.vertical, ABMetric.px(4))
    }

    // MARK: - 列表

    @ViewBuilder
    private var listing: some View {
        let shown = store.shown
        if !store.error.isEmpty {
            ABSecondaryText("　" + store.error).padding(.horizontal, ABMetric.px(12))
        } else if store.busy {
            HStack(spacing: ABMetric.px(8)) {
                ProgressView().scaleEffect(0.7).frame(width: ABMetric.px(16), height: ABMetric.px(16))
                ABSecondaryText("加载中 …")
            }
            .padding(ABMetric.px(16))
        } else if store.entries.isEmpty {
            ABSecondaryText("　（空目录）").padding(ABMetric.px(16))
        } else if shown.isEmpty {
            ABSecondaryText("　（没有匹配的项）").padding(ABMetric.px(16))
        } else {
            VStack(spacing: 0) {
                ForEach(shown) { entry in
                    row(entry)
                }
            }
        }
    }

    private func row(_ entry: SftpOps.Entry) -> some View {
        let script = entry.name.hasSuffix(".sh") && !entry.dir
        return HStack(spacing: 0) {
            if entry.link {
                Text("🔗").font(.system(size: 15))
            } else if entry.dir {
                DrawnIcon.folder(ABColor.accent, size: 18)
            } else if script {
                DrawnIcon.bolt(ABColor.warn, size: 18)
            } else {
                DrawnIcon.docFile(ABColor.fgDim, size: 18)
            }
            Text(entry.name)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(script ? ABColor.warn : ABColor.fg)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, ABMetric.px(10))
            Text(entry.dir ? "—" : ABFormat.size(entry.size))
                .font(.system(size: 11, design: .monospaced))
                .foregroundColor(entry.dir ? ABColor.fgFaint : ABColor.fgDim)
                .frame(width: ABMetric.px(64), alignment: .leading)
            Text(FilesStore.timeText(entry.mtime))
                .font(.system(size: 11, design: .monospaced))
                .foregroundColor(ABColor.fgDim)
                .lineLimit(1)
                .frame(width: ABMetric.px(72), alignment: .trailing)
            Button {
                store.sheetEntry = entry
            } label: {
                Text("⋮")
                    .font(.system(size: 17))
                    .foregroundColor(ABColor.fgDim)
                    .padding(.horizontal, ABMetric.px(12))
                    .padding(.vertical, ABMetric.px(6))
            }
            .buttonStyle(.plain)
        }
        .padding(.leading, ABMetric.px(14))
        .padding(.trailing, ABMetric.px(2))
        .padding(.vertical, ABMetric.px(10))
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.card))
        .overlay(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
        .padding(.vertical, ABMetric.px(3))
        .contentShape(Rectangle())
        .onTapGesture {
            if entry.dir {
                store.gotoDir(SftpOps.join(store.path, entry.name))
            } else {
                store.startDownload(entry)
            }
        }
        .onLongPressGesture { store.sheetEntry = entry }
    }

    // MARK: - 弹层

    @ViewBuilder
    private var dialogs: some View {
        FileActionSheet()
        FileDetailDialog()
        FileRenameDialog()
        FileDeleteDialog()
        FileMkdirDialog()
        FileLocateDialog()
        DestPickerDialog()
    }

    // MARK: - 打开终端

    private func openTerminal(at dir: String) {
        guard let jump = store.server else { return }
        guard jump.isSsh else {
            store.toast("Telnet 只有终端，没有文件功能")
            return
        }
        module.openTerminal(jumpId: jump.id, cdPath: dir)
    }
}
