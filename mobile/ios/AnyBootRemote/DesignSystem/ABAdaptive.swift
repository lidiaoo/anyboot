import SwiftUI
import Flow

// MARK: - 自适应布局原语
//
// 三条原则：
// 1. **不写死宽高** —— 一律 maxWidth: .infinity / minWidth / aspectRatio 这类相对约束；
// 2. **窄了就折行** —— 用 `ABFlow`（HFlow 真 flex-wrap），而不是把字压成 0 宽；
// 3. **容器变宽就多列** —— 用 `ABGrid`（LazyVGrid .adaptive），手机上 1 列、iPad 上自动 2~3 列。

/// flex 换行容器：内容排不下就自动折到下一行，每行高度自适应。
struct ABFlow<Content: View>: View {

    var alignment: VerticalAlignment
    var itemSpacing: CGFloat?
    var rowSpacing: CGFloat?
    var justified: Bool
    var distributeEvenly: Bool
    private let content: () -> Content

    init(alignment: VerticalAlignment = .center,
         itemSpacing: CGFloat? = ABMetric.s2,
         rowSpacing: CGFloat? = ABMetric.s2,
         justified: Bool = false,
         distributeEvenly: Bool = false,
         @ViewBuilder content: @escaping () -> Content) {
        self.alignment = alignment
        self.itemSpacing = itemSpacing
        self.rowSpacing = rowSpacing
        self.justified = justified
        self.distributeEvenly = distributeEvenly
        self.content = content
    }

    var body: some View {
        HFlow(alignment: alignment,
              itemSpacing: itemSpacing,
              rowSpacing: rowSpacing,
              justified: justified,
              distributeItemsEvenly: distributeEvenly,
              content: content)
    }
}

/// 响应式网格：给定最小列宽，列数随容器宽度自动决定（手机 1 列 / iPad 2~3 列）
struct ABGrid<Data: RandomAccessCollection, Content: View>: View
where Data.Element: Identifiable {

    let data: Data
    var minItemWidth: CGFloat
    var spacing: CGFloat
    private let content: (Data.Element) -> Content

    init(_ data: Data,
         minItemWidth: CGFloat = 320,
         spacing: CGFloat = ABMetric.gap,
         @ViewBuilder content: @escaping (Data.Element) -> Content) {
        self.data = data
        self.minItemWidth = minItemWidth
        self.spacing = spacing
        self.content = content
    }

    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: minItemWidth),
                                     spacing: spacing,
                                     alignment: .top)],
                  alignment: .leading,
                  spacing: spacing) {
            ForEach(Array(data)) { element in
                content(element)
            }
        }
    }
}

/// 表单行：宽度够就「标签 … 控件」一行，不够就自动上下堆（ViewThatFits 择一）
struct ABFormRow<Label: View, Control: View>: View {

    var alignment: VerticalAlignment
    private let label: () -> Label
    private let control: () -> Control

    init(alignment: VerticalAlignment = .firstTextBaseline,
         @ViewBuilder label: @escaping () -> Label,
         @ViewBuilder control: @escaping () -> Control) {
        self.alignment = alignment
        self.label = label
        self.control = control
    }

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: alignment, spacing: ABMetric.s3) {
                label().layoutPriority(1)
                Spacer(minLength: ABMetric.s2)
                control().fixedSize(horizontal: false, vertical: true)
            }
            VStack(alignment: .leading, spacing: ABMetric.s1) {
                label()
                control()
            }
        }
    }
}

/// 自适应按钮组：宽了横排，窄了折行（用 flex 布局，不是 ScrollView）
struct ABButtonRow<Content: View>: View {

    private let content: () -> Content

    init(@ViewBuilder content: @escaping () -> Content) {
        self.content = content
    }

    var body: some View {
        ABFlow(itemSpacing: ABMetric.s2, rowSpacing: ABMetric.s2) {
            content()
        }
    }
}

// MARK: - 页面骨架

extension View {
    /// 统一页面底色（含导航栏背后的延伸），替代系统默认灰
    func abPage() -> some View {
        self
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .background(ABColor.canvas.ignoresSafeArea())
    }

    /// 卡片容器
    func abCard(padding: CGFloat = ABMetric.cardPadding,
                raised: Bool = false,
                corner: CGFloat = ABMetric.radiusCard) -> some View {
        self
            .padding(padding)
            .background(
                RoundedRectangle(cornerRadius: corner, style: .continuous)
                    .fill(raised ? ABColor.surfaceRaised : ABColor.surface))
            .overlay(
                RoundedRectangle(cornerRadius: corner, style: .continuous)
                    .strokeBorder(ABColor.stroke, lineWidth: ABMetric.hairline))
    }
}

/// 一条水平细线（比 Divider 更可控）
struct ABHairline: View {
    var inset: CGFloat = 0
    var body: some View {
        Rectangle()
            .fill(ABColor.stroke)
            .frame(height: ABMetric.hairline)
            .padding(.leading, inset)
    }
}
