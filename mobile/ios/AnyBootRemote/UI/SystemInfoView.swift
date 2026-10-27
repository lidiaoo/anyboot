import SwiftUI

// MARK: - 居中弹层容器（对齐 AlertDialog 观感，替掉系统的 .alert）
//
// 用自绘 overlay 而不是 `.sheet`：Android 的 SystemInfoDialog 是**居中对话框**，
// 从底部升起的 sheet 观感对不上。宽度给上限、内容自适应，iPad 上不会拉成一整条。

struct ABDialogCard<Content: View>: View {

    let title: String
    /// iPad / 横屏上限宽，手机上自然撑满可用宽度
    var maxWidth: CGFloat = 360
    private let content: () -> Content

    init(_ title: String, maxWidth: CGFloat = 360, @ViewBuilder content: @escaping () -> Content) {
        self.title = title
        self.maxWidth = maxWidth
        self.content = content
    }

    var body: some View {
        VStack(alignment: .leading, spacing: ABMetric.px(12)) {
            Text(title)
                .font(.system(size: 16, weight: .bold))
                .foregroundColor(ABColor.fg)
            content()
        }
        .padding(ABMetric.px(18))
        .frame(maxWidth: maxWidth)
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(18), style: .continuous)
                .fill(ABColor.card))
        .overlay(
            RoundedRectangle(cornerRadius: ABMetric.px(18), style: .continuous)
                .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
        .shadow(color: .black.opacity(0.22), radius: 24, y: 10)
        .padding(.horizontal, ABMetric.px(24))
    }
}

/// 弹层底座：暗底 + 居中卡片 + 点外部关闭
struct ABDialogHost<Card: View>: View {

    let isPresented: Bool
    let onDismiss: () -> Void
    private let card: () -> Card

    init(isPresented: Bool, onDismiss: @escaping () -> Void,
         @ViewBuilder card: @escaping () -> Card) {
        self.isPresented = isPresented
        self.onDismiss = onDismiss
        self.card = card
    }

    var body: some View {
        if isPresented {
            ZStack {
                Color.black.opacity(0.42)
                    .ignoresSafeArea()
                    .contentShape(Rectangle())
                    .onTapGesture { onDismiss() }
                card()
                    .transition(.scale(scale: 0.96).combined(with: .opacity))
            }
            .transition(.opacity)
        }
    }
}

// MARK: - 系统信息弹层（Common.kt:496-563）

struct SystemInfoOverlay: View {

    @ObservedObject private var center = SystemInfoCenter.shared

    var body: some View {
        ABDialogHost(isPresented: center.isOpen, onDismiss: { center.dismiss() }) {
            ABDialogCard("系统信息") {
                Text(center.title)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(ABColor.fg)
                    .padding(.bottom, ABMetric.px(8))

                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        if center.loading {
                            HStack {
                                Spacer()
                                ProgressView().scaleEffect(0.8)
                                Spacer()
                            }
                            .padding(.vertical, ABMetric.px(12))
                        } else if !center.error.isEmpty {
                            Text(center.error)
                                .font(.system(size: 12))
                                .foregroundColor(ABColor.bad)
                                .fixedSize(horizontal: false, vertical: true)
                        } else {
                            ForEach(center.rows) { row in
                                VStack(alignment: .leading, spacing: ABMetric.px(1)) {
                                    Text(row.label)
                                        .font(.system(size: 10))
                                        .foregroundColor(ABColor.fgFaint)
                                        .lineLimit(1)
                                    Text(row.value)
                                        .font(.system(size: 12.5, design: .monospaced))
                                        .foregroundColor(ABColor.fg)
                                        .fixedSize(horizontal: false, vertical: true)
                                        .textSelection(.enabled)
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.vertical, ABMetric.px(4))
                            }
                            if center.rows.isEmpty {
                                Text("对方没有返回任何系统信息")
                                    .font(.system(size: 12))
                                    .foregroundColor(ABColor.fgFaint)
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: ABMetric.px(420))

                HStack(spacing: ABMetric.px(8)) {
                    Spacer(minLength: 0)
                    if !center.loading && center.error.isEmpty && !center.rows.isEmpty {
                        ABButton(title: "复制", kind: .ghost, compact: true) {
                            center.copyToPasteboard()
                        }
                    }
                    ABButton(title: "关闭", kind: .ghost, compact: true) {
                        center.dismiss()
                    }
                }
            }
        }
    }
}
