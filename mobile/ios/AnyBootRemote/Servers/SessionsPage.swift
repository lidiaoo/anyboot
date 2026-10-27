import SwiftUI

/// 会话页（对齐 `ServersActivity.kt:540` SessionsPage）：进行中 + 历史 + 清空历史。
struct SessionsPage: View {

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var jumps = JumpStore.shared

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                ABSectionTitle("进行中")
                let live = TerminalSessionsRegistry.all
                if live.isEmpty {
                    ABEmptyHint("没有进行中的会话。到「服务器」页点「▶ 打开终端」开始。")
                }
                ForEach(live) { session in
                    HStack(spacing: 0) {
                        Button {
                            store.openTerminal(jumpId: session.server.id)
                        } label: {
                            HStack(spacing: ABMetric.px(8)) {
                                Text("●")
                                    .font(.system(size: 13))
                                    .foregroundColor(ABColor.ok)
                                VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                                    Text(session.server.name.isEmpty
                                         ? session.server.addressLabel : session.server.name)
                                        .font(.system(size: 15))
                                        .foregroundColor(ABColor.fg)
                                        .lineLimit(1)
                                    ABSecondaryText("已连接 "
                                        + ABFormat.duration(Int64(Date().timeIntervalSince(session.startedAt) * 1000)))
                                }
                                Spacer(minLength: 0)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        ABChipButton(text: "结束") { store.killTarget = session }
                    }
                    .padding(.leading, ABMetric.px(12))
                    .padding(.trailing, ABMetric.px(8))
                    .padding(.vertical, ABMetric.px(12))
                }

                ABSectionTitle("历史")
                let history = jumps.history.filter { $0.endedAt > $0.startedAt }
                if jumps.history.isEmpty {
                    ABEmptyHint("还没有会话历史。")
                }
                ForEach(history) { entry in
                    VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                        Text(entry.serverName.isEmpty ? "服务器" : entry.serverName)
                            .font(.system(size: 15))
                            .foregroundColor(ABColor.fg)
                            .lineLimit(1)
                        ABSecondaryText(Self.stamp.string(from: entry.startedAt)
                            + " · 时长 "
                            + ABFormat.duration(Int64(entry.endedAt.timeIntervalSince(entry.startedAt) * 1000)))
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.leading, ABMetric.px(12))
                    .padding(.trailing, ABMetric.px(8))
                    .padding(.vertical, ABMetric.px(12))
                }
                if !jumps.history.isEmpty {
                    Button {
                        jumps.clearHistory()
                    } label: {
                        Text("清空历史")
                            .font(.system(size: 14))
                            .foregroundColor(ABColor.accent)
                            .padding(.vertical, ABMetric.px(10))
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.plain)
                }
                Spacer().frame(height: ABMetric.px(12))
            }
            // 安卓会话页根布局没有页面级水平边距（行自带 leading 12）
        }
        .background(ABColor.base)
        .overlay { killDialog }
    }

    private static let stamp: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter
    }()

    /// 「结束会话」确认弹窗 —— `:605`
    @ViewBuilder
    private var killDialog: some View {
        ABDialogHost(isPresented: store.killTarget != nil,
                     onDismiss: { store.killTarget = nil }) {
            ABDialogCard("结束会话") {
                if let session = store.killTarget {
                    ABDialogNote("结束「\(session.server.displayName)」的终端会话吗？", color: ABColor.fg)
                    ABDialogButtons(confirmTitle: "结束",
                                    onConfirm: {
                                        store.killTarget = nil
                                        TerminalSessionsRegistry.kill(session)
                                        store.markSessionsChanged()
                                    },
                                    onDismiss: { store.killTarget = nil })
                }
            }
        }
    }
}
