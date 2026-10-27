import SwiftUI
import AlertToast

// MARK: - 原子组件
//
// 每个组件都只用相对约束（maxWidth / minHeight / padding），没有一处写死宽度，
// 所以换机型、横竖屏、iPad 分栏都不会错位。

/// 胶囊标签：通道名 / 状态 / 计数
struct ABBadge: View {

    let text: String
    var tint: Color = ABColor.textDim
    var icon: String? = nil
    /// 实心（强调用）还是淡淡的一层底（默认）
    var filled: Bool = false

    var body: some View {
        HStack(spacing: 3) {
            if let icon {
                Image(systemName: icon).font(.system(size: 9, weight: .bold))
            }
            Text(text)
                .font(ABFont.badge)
                .lineLimit(1)
        }
        .foregroundColor(filled ? .white : tint)
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .background(
            Capsule(style: .continuous)
                .fill(filled ? tint : tint.opacity(0.14)))
        .overlay(
            Capsule(style: .continuous)
                .strokeBorder(filled ? .clear : tint.opacity(0.22), lineWidth: 0.7))
    }
}

/// 状态小圆点（在线绿 / 离线红 / 未测出灰），带一圈柔光
struct ABDot: View {

    var tint: Color
    var size: CGFloat = 8
    var glowing: Bool = true

    var body: some View {
        Circle()
            .fill(tint)
            .frame(width: size, height: size)
            .overlay(
                Circle()
                    .stroke(tint.opacity(glowing ? 0.28 : 0), lineWidth: 3)
                    .scaleEffect(glowing ? 1.8 : 1))
            .frame(width: size, height: size)
    }
}

/// 工具条 chip（可选中状态），配合 `ABFlow` 自动换行
struct ABChip: View {

    let title: String
    var icon: String? = nil
    var selected: Bool = false
    var tint: Color = ABColor.accent
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                if let icon {
                    Image(systemName: icon).font(.system(size: 11, weight: .semibold))
                }
                Text(title).font(.system(size: 12, weight: .medium))
            }
            .foregroundColor(selected ? .white : ABColor.text)
            .padding(.horizontal, 11)
            .padding(.vertical, 7)
            .background(
                Capsule(style: .continuous)
                    .fill(selected ? tint : ABColor.surfaceRaised))
            .overlay(
                Capsule(style: .continuous)
                    .strokeBorder(selected ? .clear : ABColor.stroke, lineWidth: ABMetric.hairline))
        }
        .buttonStyle(.plain)
    }
}

/// 按钮：主 / 次 / 危险 / 幽灵。高度走 minHeight，宽度自适应内容或撑满由调用方 `.frame(maxWidth:)` 决定
struct ABButton: View {

    enum Kind { case primary, secondary, destructive, ghost }

    let title: String
    var icon: String? = nil
    var kind: Kind = .secondary
    var compact: Bool = false
    var enabled: Bool = true
    let action: () -> Void

    var body: some View {
        Button {
            action()
        } label: {
            HStack(spacing: 5) {
                if let icon {
                    Image(systemName: icon).font(.system(size: compact ? 11 : 13, weight: .semibold))
                }
                Text(title).font(.system(size: compact ? 12 : 14, weight: .semibold))
                    .lineLimit(1)
            }
            .foregroundColor(foreground)
            .padding(.horizontal, compact ? 10 : 14)
            .padding(.vertical, compact ? 6 : 9)
            .frame(minHeight: compact ? 30 : 38)
            .background(
                RoundedRectangle(cornerRadius: ABMetric.radiusControl, style: .continuous)
                    .fill(background))
            .overlay(
                RoundedRectangle(cornerRadius: ABMetric.radiusControl, style: .continuous)
                    .strokeBorder(border, lineWidth: ABMetric.hairline))
        }
        .buttonStyle(.plain)
        .opacity(enabled ? 1 : 0.45)
        .disabled(!enabled)
    }

    private var foreground: Color {
        switch kind {
        case .primary: return .white
        case .destructive: return ABColor.bad
        case .secondary: return ABColor.text
        case .ghost: return ABColor.textDim
        }
    }

    private var background: Color {
        switch kind {
        case .primary: return ABColor.accent
        case .destructive: return ABColor.bad.opacity(0.12)
        case .secondary: return ABColor.surfaceRaised
        case .ghost: return .clear
        }
    }

    private var border: Color {
        switch kind {
        case .primary, .ghost: return .clear
        case .destructive: return ABColor.bad.opacity(0.25)
        case .secondary: return ABColor.stroke
        }
    }
}

/// 图标按钮（工具栏 / 卡片操作行）
struct ABIconButton: View {

    let icon: String
    var tint: Color = ABColor.text
    var size: CGFloat = 34
    var busy: Bool = false
    var enabled: Bool = true
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: busy ? "hourglass" : icon)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(tint)
                .frame(width: size, height: size)
                .background(
                    RoundedRectangle(cornerRadius: ABMetric.radiusControl, style: .continuous)
                        .fill(ABColor.surfaceRaised))
                .overlay(
                    RoundedRectangle(cornerRadius: ABMetric.radiusControl, style: .continuous)
                        .strokeBorder(ABColor.stroke, lineWidth: ABMetric.hairline))
        }
        .buttonStyle(.plain)
        .opacity(enabled ? 1 : 0.4)
        .disabled(!enabled)
    }
}

/// 进度条：确定态（0…1）与不确定态（总量未知，来回扫）
struct ABProgressBar: View {

    var value: Double
    var tint: Color = ABColor.accent
    var height: CGFloat = 6
    /// 总量未知：不假装知道百分比
    var indeterminate: Bool = false

    @State private var sweep: CGFloat = -0.35

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule(style: .continuous)
                    .fill(tint.opacity(0.15))
                if indeterminate {
                    Capsule(style: .continuous)
                        .fill(tint)
                        .frame(width: max(24, geo.size.width * 0.3))
                        .offset(x: sweep * geo.size.width)
                } else {
                    Capsule(style: .continuous)
                        .fill(tint)
                        .frame(width: max(0, min(1, value)) * geo.size.width)
                        .animation(.easeOut(duration: 0.25), value: value)
                }
            }
        }
        .frame(height: height)
        .onAppear {
            guard indeterminate else { return }
            withAnimation(.linear(duration: 1.15).repeatForever(autoreverses: false)) {
                sweep = 1.05
            }
        }
    }
}

/// 区块标题（带右下角操作）
struct ABSectionHeader<Trailing: View>: View {

    let title: String
    var subtitle: String? = nil
    private let trailing: () -> Trailing

    init(_ title: String,
         subtitle: String? = nil,
         @ViewBuilder trailing: @escaping () -> Trailing) {
        self.title = title
        self.subtitle = subtitle
        self.trailing = trailing
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: ABMetric.s2) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(ABColor.textDim)
                if let subtitle {
                    Text(subtitle).font(ABFont.micro).foregroundColor(ABColor.textFaint)
                }
            }
            Spacer(minLength: 0)
            trailing()
        }
    }
}

extension ABSectionHeader where Trailing == EmptyView {
    init(_ title: String, subtitle: String? = nil) {
        self.init(title, subtitle: subtitle) { EmptyView() }
    }
}

/// 空状态
struct ABEmptyState: View {

    let icon: String
    let title: String
    var message: String? = nil
    var actionTitle: String? = nil
    var action: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: ABMetric.s3) {
            Image(systemName: icon)
                .font(.system(size: 30, weight: .light))
                .foregroundColor(ABColor.textFaint)
            VStack(spacing: ABMetric.s1) {
                Text(title).font(ABFont.cardTitle).foregroundColor(ABColor.text)
                if let message {
                    Text(message)
                        .font(ABFont.caption)
                        .foregroundColor(ABColor.textFaint)
                        .multilineTextAlignment(.center)
                }
            }
            if let actionTitle, let action {
                ABButton(title: actionTitle, kind: .secondary, action: action)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, ABMetric.s6)
        .padding(.horizontal, ABMetric.s4)
    }
}

/// 一个数值 + 一句说明（耗时 / 速度 / 大小）
struct ABStat: View {

    let value: String
    let label: String
    var tint: Color = ABColor.text

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(value).font(ABFont.number(14)).foregroundColor(tint)
            Text(label).font(ABFont.micro).foregroundColor(ABColor.textFaint)
        }
    }
}

/// 详情行：左标题 + 右值（宽度不够自动上下堆）
struct ABKeyValue: View {

    let key: String
    let value: String
    var mono: Bool = false

    var body: some View {
        ABFormRow {
            Text(key).font(ABFont.caption).foregroundColor(ABColor.textFaint)
        } control: {
            Text(value)
                .font(mono ? ABFont.mono(12) : ABFont.body)
                .foregroundColor(ABColor.text)
                .multilineTextAlignment(.trailing)
                .textSelection(.enabled)
        }
    }
}

/// 表单分区卡：标题 + 一张卡片装若干行 + 可选脚注。
/// 用它替掉 `Section`，表单就从系统 Form 外观变成卡片式。
struct ABSectionCard<Content: View>: View {

    let title: String
    var note: String? = nil
    private let content: () -> Content

    init(_ title: String, note: String? = nil, @ViewBuilder content: @escaping () -> Content) {
        self.title = title
        self.note = note
        self.content = content
    }

    var body: some View {
        VStack(alignment: .leading, spacing: ABMetric.s2) {
            ABSectionHeader(title)
            VStack(alignment: .leading, spacing: ABMetric.s3) {
                content()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .abCard()
            if let note, !note.isEmpty {
                Text(note)
                    .font(ABFont.micro)
                    .foregroundColor(ABColor.textFaint)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 2)
            }
        }
    }
}

extension View {
    /// 输入框外观（替代 Form 自带的行内输入样式）
    func abField() -> some View {
        self
            .font(ABFont.body)
            .padding(.horizontal, 10)
            .padding(.vertical, 9)
            .background(
                RoundedRectangle(cornerRadius: ABMetric.radiusControl, style: .continuous)
                    .fill(ABColor.surfaceRaised))
            .overlay(
                RoundedRectangle(cornerRadius: ABMetric.radiusControl, style: .continuous)
                    .strokeBorder(ABColor.stroke, lineWidth: ABMetric.hairline))
    }
}

// MARK: - Toast

enum ABToastKind {
    case info, success, failure, loading

    var type: AlertToast.AlertType {
        switch self {
        case .info: return .regular
        case .success: return .complete(ABColor.good)
        case .failure: return .error(ABColor.bad)
        case .loading: return .loading
        }
    }
}

extension View {
    /// 统一的轻提示（替代系统 alert 做「操作成功/失败」这类反馈）
    func abToast(_ presenting: Binding<Bool>,
                 title: String,
                 subtitle: String? = nil,
                 kind: ABToastKind = .info) -> some View {
        self.toast(isPresenting: presenting) {
            AlertToast(type: kind.type,
                       title: title,
                       subTitle: subtitle)
        }
    }
}
