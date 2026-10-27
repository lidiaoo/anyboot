import SwiftUI

/// 传输页（对齐 `ServersActivity.kt:2949` TransfersPage）：进行中 + 已完成/失败。
/// 发起在「文件」页，这里只管看进度、暂停 / 继续 / 取消 / 重试。
struct TransfersPage: View {

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var center = TransferCenter.shared

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                let pending = center.active
                let paused = pending.filter { $0.state == .paused }.count
                let heading: String = {
                    if pending.isEmpty { return "进行中" }
                    if paused == 0 { return "进行中（\(pending.count)）" }
                    return "进行中（\(pending.count - paused)）· 已暂停（\(paused)）"
                }()
                ABSectionTitle(heading)
                if pending.isEmpty {
                    ABEmptyHint("没有正在传输的文件。到「文件」页点「上传」，或点一个远端文件下载 —— "
                                + "任务都会出现在这里，可以看进度、暂停 / 继续、取消。")
                }
                ForEach(pending) { task in
                    TransferRow(task: task)
                }

                ABSectionTitle("已完成 / 失败")
                let finished = center.finished
                if finished.isEmpty {
                    ABEmptyHint("还没有传输记录。")
                }
                ForEach(finished) { task in
                    TransferRow(task: task)
                }
                if !finished.isEmpty {
                    Button {
                        center.clearFinished()
                    } label: {
                        Text("清空记录")
                            .font(.system(size: 14))
                            .foregroundColor(ABColor.accent)
                            .padding(.vertical, ABMetric.px(10))
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.plain)
                }
                Spacer().frame(height: ABMetric.px(12))
            }
            // 安卓传输页根布局没有页面级水平边距（行/标题自带 leading 12）
        }
        .background(ABColor.base)
    }
}

/// 一条任务 —— `:2998`
struct TransferRow: View {

    let task: TransferTask

    @ObservedObject private var store = ServerModuleStore.shared

    private var uploading: Bool { task.direction == .upload }
    private var paused: Bool { task.state == .paused }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 0) {
                Text(uploading ? "↑" : "↓")
                    .font(.system(size: 16))
                    .foregroundColor(task.isRunning ? ABColor.accent : ABColor.fgDim)
                VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                    Text(task.name)
                        .font(.system(size: 15))
                        .foregroundColor(ABColor.fg)
                        .lineLimit(1)
                    ABSecondaryText((uploading ? "上传到 " : "下载自 ")
                                    + task.serverName + " · " + task.remoteDir
                                    + (task.resumed ? " · 续传" : ""))
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, ABMetric.px(8))

                switch task.state {
                case .running:
                    ABChipButton(text: "暂停") { TransferCenter.shared.pause(task.id) }
                    Spacer().frame(width: ABMetric.px(6))
                    ABChipButton(text: "取消") { TransferCenter.shared.cancel(task.id) }
                case .paused:
                    ABChipButton(text: "继续") { resume() }
                    Spacer().frame(width: ABMetric.px(6))
                    ABChipButton(text: "取消") { TransferCenter.shared.cancel(task.id) }
                case .superseded:
                    EmptyView()
                default:
                    ABChipButton(text: "重试") { retry() }
                }
            }
            if task.isPending {
                Spacer().frame(height: ABMetric.px(8))
                progressBar
                Spacer().frame(height: ABMetric.px(4))
                ABSecondaryText(paused ? pausedText : progressText)
                if paused && task.interrupted {
                    Spacer().frame(height: ABMetric.px(4))
                    Text(task.message)
                        .font(.system(size: 12))
                        .foregroundColor(ABColor.fgDim)
                        .padding(.leading, ABMetric.px(24))
                }
            } else {
                Spacer().frame(height: ABMetric.px(4))
                Text(task.message)
                    .font(.system(size: 12))
                    .foregroundColor(stateColor)
                    .padding(.leading, ABMetric.px(24))
            }
        }
        .padding(.leading, ABMetric.px(12))
        .padding(.trailing, ABMetric.px(8))
        .padding(.vertical, ABMetric.px(10))
    }

    private var stateColor: Color {
        switch task.state {
        case .done: return ABColor.ok
        case .failed: return ABColor.bad
        default: return ABColor.fgDim
        }
    }

    /// 细进度条（总量未知走不确定态）
    private var progressBar: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: ABMetric.px(2), style: .continuous)
                    .fill(ABColor.elevate)
                if task.total <= 0 && !paused {
                    RoundedRectangle(cornerRadius: ABMetric.px(2), style: .continuous)
                        .fill(ABColor.accent.opacity(0.35))
                } else {
                    RoundedRectangle(cornerRadius: ABMetric.px(2), style: .continuous)
                        .fill(paused ? ABColor.warn : ABColor.accent)
                        .frame(width: geo.size.width * min(1, max(0.01, task.fraction)))
                }
            }
        }
        .frame(height: ABMetric.px(4))
        .padding(.leading, ABMetric.px(24))
    }

    private var progressText: String {
        if task.total <= 0 {
            return "已传 \(TransferTask.humanBytes(task.transferred)) · \(speedText)"
        }
        return "\(TransferTask.humanBytes(task.transferred)) / \(TransferTask.humanBytes(task.total))"
            + " · \(speedText)"
    }

    /// 暂停行不报速度：那是「停下来那一刻的均值」，续传过、或重启后装回来的都没有意义
    private var pausedText: String {
        let size = task.total <= 0
            ? "已传 \(TransferTask.humanBytes(task.transferred))"
            : "\(TransferTask.humanBytes(task.transferred)) / \(TransferTask.humanBytes(task.total))"
        return "已暂停 · \(size)"
    }

    private var speedText: String {
        let speed = task.bytesPerSecond
        if speed <= 0 { return "计算中 …" }
        return TransferTask.humanBytes(speed) + "/s"
    }

    private func resume() {
        if let reason = TransferCenter.shared.resume(task.id) {
            store.toast(reason, long: true)
        } else {
            store.toast("接着传：\(task.name)（进度见上面「进行中」）")
        }
    }

    private func retry() {
        if let reason = TransferCenter.shared.retry(task.id) {
            store.toast(reason, long: true)
        } else {
            store.toast("已重新加入传输：\(task.name)（进度见上面「进行中」）")
        }
    }
}
