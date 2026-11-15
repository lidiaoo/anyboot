import SwiftUI

/// 会话页（对齐 `ServersActivity.kt:540` SessionsPage）：进行中 + 历史 + 清空历史。
struct SessionsPage: View {

    @ObservedObject private var store = ServerModuleStore.shared
    @ObservedObject private var jumps = JumpStore.shared

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                ABSectionTitle(L("and.ongoing"))
                let live = TerminalSessionsRegistry.all
                if live.isEmpty {
                    ABEmptyHint(L("and.noActiveSessionHint"))
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
                                    ABSecondaryText(L("and.connectedShort")
                                        + ABFormat.duration(Int64(Date().timeIntervalSince(session.startedAt) * 1000)))
                                }
                                Spacer(minLength: 0)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        ABChipButton(text: L("and.end")) { store.killTarget = session }
                    }
                    .padding(.leading, ABMetric.px(12))
                    .padding(.trailing, ABMetric.px(8))
                    .padding(.vertical, ABMetric.px(12))
                }

                ABSectionTitle(L("and.history"))
                let history = jumps.history.filter { $0.endedAt > $0.startedAt }
                if jumps.history.isEmpty {
                    ABEmptyHint(L("and.noHistory"))
                }
                ForEach(history) { entry in
                    VStack(alignment: .leading, spacing: ABMetric.px(2)) {
                        Text(entry.serverName.isEmpty ? L("nav.servers") : entry.serverName)
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
                        Text(L("and.clearHistory"))
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
            ABDialogCard(L("and.endSessionTitle")) {
                if let session = store.killTarget {
                    ABDialogNote(L("and.endSessionConfirm", session.server.displayName), color: ABColor.fg)
                    ABDialogButtons(confirmTitle: L("and.end"),
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
