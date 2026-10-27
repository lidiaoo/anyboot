import SwiftUI

// MARK: - 弹层与表单小件（对齐 Android Material3 AlertDialog / OutlinedTextField / Segmented 的观感）

/// 弹层里的说明文字
struct ABDialogNote: View {
    let text: String
    var color: Color = ABColor.fgDim

    init(_ text: String, color: Color = ABColor.fgDim) {
        self.text = text
        self.color = color
    }

    var body: some View {
        Text(text)
            .font(.system(size: 13))
            .foregroundColor(color)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// 弹层里的单行输入框（描边 + 圆角，与 OutlinedTextField 同观感）
struct ABDialogInput: View {

    let placeholder: String
    @Binding var text: String
    var secure: Bool = false
    var numeric: Bool = false

    var body: some View {
        Group {
            if secure {
                SecureField(placeholder, text: $text)
            } else {
                TextField(placeholder, text: $text)
            }
        }
        .font(.system(size: 13))
        .foregroundColor(ABColor.fg)
        .keyboardType(numeric ? .numberPad : .default)
        .autocorrectionDisabled()
        .textInputAutocapitalization(.never)
        .padding(.horizontal, ABMetric.px(12))
        .padding(.vertical, ABMetric.px(10))
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.base))
        .overlay(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .strokeBorder(ABColor.lineStrong, lineWidth: ABMetric.hairline))
    }
}

/// 弹层底部按钮行（右对齐；confirm 用强调色）
struct ABDialogButtons: View {

    var confirmTitle: String
    var confirmColor: Color = ABColor.accent
    var dismissTitle: String = "取消"
    var onConfirm: () -> Void
    var onDismiss: () -> Void

    var body: some View {
        HStack(spacing: ABMetric.px(6)) {
            Spacer(minLength: 0)
            Button(action: onDismiss) {
                Text(dismissTitle)
                    .font(.system(size: 14))
                    .foregroundColor(ABColor.fgDim)
                    .padding(.horizontal, ABMetric.px(12))
                    .padding(.vertical, ABMetric.px(8))
            }
            .buttonStyle(.plain)
            Button(action: onConfirm) {
                Text(confirmTitle)
                    .font(.system(size: 14))
                    .foregroundColor(confirmColor)
                    .padding(.horizontal, ABMetric.px(12))
                    .padding(.vertical, ABMetric.px(8))
            }
            .buttonStyle(.plain)
        }
    }
}

/// 分段控件（容器描边 + 选中格强调色浅底）—— `ServersActivity.kt:1332`
struct ABSegmented: View {

    let options: [String]
    let selected: Int
    let onSelect: (Int) -> Void

    var body: some View {
        HStack(spacing: ABMetric.px(4)) {
            ForEach(Array(options.enumerated()), id: \.offset) { index, label in
                Button {
                    onSelect(index)
                } label: {
                    Text(label)
                        .font(.system(size: 12, weight: selected == index ? .semibold : .regular))
                        .foregroundColor(selected == index ? ABColor.accent : ABColor.fgDim)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, ABMetric.px(9))
                        .background(
                            RoundedRectangle(cornerRadius: ABMetric.px(8), style: .continuous)
                                .fill(selected == index ? ABColor.accent.opacity(0.15) : .clear))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(ABMetric.px(4))
        .background(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .fill(ABColor.base))
        .overlay(
            RoundedRectangle(cornerRadius: ABMetric.px(12), style: .continuous)
                .strokeBorder(ABColor.line, lineWidth: ABMetric.hairline))
    }
}

/// 详情弹层的「标签 + 值」两列行
struct ABDialogFieldRow: View {

    let label: String
    let value: String

    var body: some View {
        HStack(alignment: .top, spacing: ABMetric.px(6)) {
            Text(label)
                .font(.system(size: 12))
                .foregroundColor(ABColor.fgDim)
                .frame(width: ABMetric.px(68), alignment: .leading)
            Text(value)
                .font(.system(size: 13))
                .foregroundColor(ABColor.fg)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, ABMetric.px(3))
    }
}

/// 虚线提示框（keys 屏底部说明）—— `ServersActivity.kt:1578`
struct ABDashedHint: View {

    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundColor(ABColor.fgFaint)
            .multilineTextAlignment(.center)
            .lineSpacing(ABMetric.px(4))
            .frame(maxWidth: .infinity)
            .padding(.horizontal, ABMetric.px(16))
            .padding(.vertical, ABMetric.px(14))
            .background(
                RoundedRectangle(cornerRadius: ABMetric.px(16), style: .continuous)
                    .strokeBorder(ABColor.lineStrong,
                                  style: StrokeStyle(lineWidth: ABMetric.hairline,
                                                     dash: [ABMetric.px(6), ABMetric.px(5)])))
    }
}
