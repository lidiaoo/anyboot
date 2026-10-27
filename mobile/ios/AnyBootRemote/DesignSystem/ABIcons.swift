import SwiftUI

// MARK: - 手绘线性图标（一比一移植 Icons.kt 426 行）
//
// Android **不是**用系统图标：`Icons.kt` 在 24×24 网格上用 Canvas 手绘，
// 线宽 1.8、圆头圆角。这里逐条照抄路径坐标 —— 用 SF Symbols 顶替形状就对不上，
// 所以一个都不许换。
//
// 坐标统一乘以 `s = 实际尺寸 / 24`，换尺寸不变形。

struct DrawnIcon: View {

    let tint: Color
    var size: CGFloat = 20
    /// 在 24×24 网格里作画，`s` 是单格长度
    let draw: (inout GraphicsContext, CGFloat) -> Void

    init(_ tint: Color, size: CGFloat = 20,
         draw: @escaping (inout GraphicsContext, CGFloat) -> Void) {
        self.tint = tint
        self.size = size
        self.draw = draw
    }

    var body: some View {
        Canvas { context, canvasSize in
            let s = min(canvasSize.width, canvasSize.height) / 24
            draw(&context, s)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

// MARK: - 绘制工具

/// 描边样式：线宽 1.8 格、圆头（对齐 `pen(grid)`）
private func pen(_ s: CGFloat) -> StrokeStyle {
    StrokeStyle(lineWidth: 1.8 * s, lineCap: .round, lineJoin: .round)
}

/// 实心
private func fillStyle() -> StrokeStyle {
    StrokeStyle(lineWidth: 1)
}

/// 用折线采样画弧 —— 不用 addArc，避免 clockwise 语义在翻转坐标系里搞反。
/// `start` / `sweep` 单位是度，0 = 3 点钟方向，正方向顺时针（与 Compose drawArc 一致）。
private func arcPath(center: CGPoint, radius: CGFloat,
                     start: CGFloat, sweep: CGFloat,
                     scale s: CGFloat? = nil,
                     radiusY: CGFloat? = nil) -> Path {
    var path = Path()
    let steps = max(8, Int(abs(sweep) / 4))
    let ry = radiusY ?? radius
    for i in 0...steps {
        let angle = (start + sweep * CGFloat(i) / CGFloat(steps)) * .pi / 180
        let point = CGPoint(x: center.x + cos(angle) * radius,
                            y: center.y + sin(angle) * radius)
        _ = ry
        if i == 0 {
            path.move(to: point)
        } else {
            path.addLine(to: point)
        }
    }
    return path
}

/// 椭圆弧（drawArc 用 topLeft/size 描述的是一个椭圆外框）
private func ellipseArcPath(rect: CGRect, start: CGFloat, sweep: CGFloat) -> Path {
    var path = Path()
    let rx = rect.width / 2
    let ry = rect.height / 2
    let cx = rect.midX
    let cy = rect.midY
    let steps = max(10, Int(abs(sweep) / 4))
    for i in 0...steps {
        let angle = (start + sweep * CGFloat(i) / CGFloat(steps)) * .pi / 180
        let point = CGPoint(x: cx + cos(angle) * rx, y: cy + sin(angle) * ry)
        if i == 0 { path.move(to: point) } else { path.addLine(to: point) }
    }
    return path
}

private func line(_ a: CGPoint, _ b: CGPoint) -> Path {
    var path = Path()
    path.move(to: a)
    path.addLine(to: b)
    return path
}

private func roundedBox(_ rect: CGRect, _ radius: CGFloat) -> Path {
    Path(roundedRect: rect, cornerRadius: radius)
}

extension GraphicsContext {
    func strokeLine(_ path: Path, _ color: Color, _ s: CGFloat) {
        stroke(path, with: .color(color), style: pen(s))
    }
    func fillPath(_ path: Path, _ color: Color) {
        fill(path, with: .color(color))
    }
    func dot(_ center: CGPoint, _ radius: CGFloat, _ color: Color) {
        let rect = CGRect(x: center.x - radius, y: center.y - radius,
                          width: radius * 2, height: radius * 2)
        fill(Path(ellipseIn: rect), with: .color(color))
    }
}

// MARK: - 图标

extension DrawnIcon {

    /// 显示器（抽屉头部 / 终端按钮 / 关于页主图）
    static func desktop(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(roundedBox(CGRect(x: 3 * s, y: 4 * s, width: 18 * s, height: 14 * s), 2 * s), tint, s)
            ctx.strokeLine(line(CGPoint(x: 8 * s, y: 21 * s), CGPoint(x: 16 * s, y: 21 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 18 * s), CGPoint(x: 12 * s, y: 21 * s)), tint, s)
        }
    }

    /// 服务器机架（两层 + 指示点）：底部导航「服务器」/ 服务器卡片
    static func serverStack(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(roundedBox(CGRect(x: 2 * s, y: 3 * s, width: 20 * s, height: 7 * s), 2 * s), tint, s)
            ctx.strokeLine(roundedBox(CGRect(x: 2 * s, y: 14 * s, width: 20 * s, height: 7 * s), 2 * s), tint, s)
            ctx.dot(CGPoint(x: 6 * s, y: 6.5 * s), 0.9 * s, tint)
            ctx.dot(CGPoint(x: 6 * s, y: 17.5 * s), 0.9 * s, tint)
        }
    }

    /// 终端提示符（方框 + ›）：底部导航「终端」/ 卡片终端按钮 / 终端页顶栏
    static func term(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(roundedBox(CGRect(x: 2 * s, y: 4 * s, width: 20 * s, height: 14 * s), 2 * s), tint, s)
            ctx.strokeLine(line(CGPoint(x: 6 * s, y: 9 * s), CGPoint(x: 9 * s, y: 12 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 9 * s, y: 12 * s), CGPoint(x: 6 * s, y: 15 * s)), tint, s)
        }
    }

    /// 钥匙：底部导航「密钥」/ 密钥卡
    static func key(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(Path(ellipseIn: CGRect(x: 4 * s, y: 11 * s, width: 8 * s, height: 8 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 10.8 * s, y: 12.2 * s), CGPoint(x: 20 * s, y: 3 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 17 * s, y: 6 * s), CGPoint(x: 19.5 * s, y: 8.5 * s)), tint, s)
        }
    }

    /// 文件夹：底部导航「文件」/ 文件列表目录行
    static func folder(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            var path = Path()
            path.move(to: CGPoint(x: 2 * s, y: 18.5 * s))
            path.addLine(to: CGPoint(x: 2 * s, y: 5.5 * s))
            path.addLine(to: CGPoint(x: 7.5 * s, y: 5.5 * s))
            path.addLine(to: CGPoint(x: 9.5 * s, y: 8 * s))
            path.addLine(to: CGPoint(x: 20 * s, y: 8 * s))
            path.addLine(to: CGPoint(x: 22 * s, y: 10 * s))
            path.addLine(to: CGPoint(x: 22 * s, y: 18.5 * s))
            path.closeSubpath()
            ctx.strokeLine(path, tint, s)
        }
    }

    /// 文档（折角）：文件列表文件行
    static func docFile(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            var path = Path()
            path.move(to: CGPoint(x: 6 * s, y: 2.5 * s))
            path.addLine(to: CGPoint(x: 13.5 * s, y: 2.5 * s))
            path.addLine(to: CGPoint(x: 20 * s, y: 9 * s))
            path.addLine(to: CGPoint(x: 20 * s, y: 20 * s))
            path.addLine(to: CGPoint(x: 6 * s, y: 20 * s))
            path.closeSubpath()
            ctx.strokeLine(path, tint, s)
            ctx.strokeLine(line(CGPoint(x: 13.5 * s, y: 2.5 * s), CGPoint(x: 13.5 * s, y: 9 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 13.5 * s, y: 9 * s), CGPoint(x: 20 * s, y: 9 * s)), tint, s)
        }
    }

    /// 闪电（脚本 / 可执行文件行）
    static func bolt(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            var path = Path()
            path.move(to: CGPoint(x: 13 * s, y: 2 * s))
            path.addLine(to: CGPoint(x: 3 * s, y: 14 * s))
            path.addLine(to: CGPoint(x: 10 * s, y: 14 * s))
            path.addLine(to: CGPoint(x: 9 * s, y: 22 * s))
            path.addLine(to: CGPoint(x: 19 * s, y: 10 * s))
            path.addLine(to: CGPoint(x: 12 * s, y: 10 * s))
            path.closeSubpath()
            ctx.strokeLine(path, tint, s)
        }
    }

    /// 时钟（关于页「版本更新」行）
    static func clock(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(Path(ellipseIn: CGRect(x: 3 * s, y: 3 * s, width: 18 * s, height: 18 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 7.5 * s), CGPoint(x: 12 * s, y: 12 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 12 * s), CGPoint(x: 14.5 * s, y: 14.5 * s)), tint, s)
        }
    }

    /// 代码尖括号（GitHub 仓库卡 / 开源许可行）
    static func code(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 9 * s, y: 18 * s), CGPoint(x: 3.5 * s, y: 12 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 3.5 * s, y: 12 * s), CGPoint(x: 9 * s, y: 6 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 15 * s, y: 6 * s), CGPoint(x: 20.5 * s, y: 12 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 20.5 * s, y: 12 * s), CGPoint(x: 15 * s, y: 18 * s)), tint, s)
        }
    }

    /// 下载（↓ + 底部托盘）：文件动作菜单「下载到本机」
    static func download(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 3 * s), CGPoint(x: 12 * s, y: 15 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 7 * s, y: 10 * s), CGPoint(x: 12 * s, y: 15 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 17 * s, y: 10 * s), CGPoint(x: 12 * s, y: 15 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 4 * s, y: 21 * s), CGPoint(x: 20 * s, y: 21 * s)), tint, s)
        }
    }

    /// 上载（↑ + 底部托盘）：文件动作菜单「上传到此」（DownloadIcon 的镜像）
    static func upload(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 15 * s), CGPoint(x: 12 * s, y: 3 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 7 * s, y: 8 * s), CGPoint(x: 12 * s, y: 3 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 17 * s, y: 8 * s), CGPoint(x: 12 * s, y: 3 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 4 * s, y: 21 * s), CGPoint(x: 20 * s, y: 21 * s)), tint, s)
        }
    }

    /// 铅笔：文件动作菜单「重命名」/ 更多操作
    static func pencil(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 20 * s), CGPoint(x: 21 * s, y: 20 * s)), tint, s)
            var path = Path()
            path.move(to: CGPoint(x: 16.5 * s, y: 3.5 * s))
            path.addLine(to: CGPoint(x: 19.5 * s, y: 6.5 * s))
            path.addLine(to: CGPoint(x: 7 * s, y: 19 * s))
            path.addLine(to: CGPoint(x: 3 * s, y: 20 * s))
            path.addLine(to: CGPoint(x: 4 * s, y: 16 * s))
            path.closeSubpath()
            ctx.strokeLine(path, tint, s)
        }
    }

    /// 复制（双层方块）：文件动作菜单「复制路径」/ 更多操作
    static func copy(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(roundedBox(CGRect(x: 9 * s, y: 9 * s, width: 12 * s, height: 12 * s), 2 * s), tint, s)
            var path = Path()
            path.move(to: CGPoint(x: 5 * s, y: 15 * s))
            path.addLine(to: CGPoint(x: 4 * s, y: 15 * s))
            path.addLine(to: CGPoint(x: 4 * s, y: 4 * s))
            path.addLine(to: CGPoint(x: 15 * s, y: 4 * s))
            path.addLine(to: CGPoint(x: 15 * s, y: 5 * s))
            ctx.strokeLine(path, tint, s)
        }
    }

    /// 垃圾桶：文件动作菜单「删除」/ 密钥/服务器删除入口
    static func trash(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 3 * s, y: 6 * s), CGPoint(x: 21 * s, y: 6 * s)), tint, s)
            var lid = Path()
            lid.move(to: CGPoint(x: 8 * s, y: 6 * s))
            lid.addLine(to: CGPoint(x: 8 * s, y: 4 * s))
            lid.addLine(to: CGPoint(x: 16 * s, y: 4 * s))
            lid.addLine(to: CGPoint(x: 16 * s, y: 6 * s))
            ctx.strokeLine(lid, tint, s)
            var body = Path()
            body.move(to: CGPoint(x: 19 * s, y: 6 * s))
            body.addLine(to: CGPoint(x: 18 * s, y: 20 * s))
            body.addLine(to: CGPoint(x: 6 * s, y: 20 * s))
            body.addLine(to: CGPoint(x: 5 * s, y: 6 * s))
            ctx.strokeLine(body, tint, s)
        }
    }

    /// ‹ 返回：各次级页面顶栏
    static func chevronLeft(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 14.5 * s, y: 5 * s), CGPoint(x: 8 * s, y: 12 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 8 * s, y: 12 * s), CGPoint(x: 14.5 * s, y: 19 * s)), tint, s)
        }
    }

    /// › 前进（chevron 右）
    static func chevronRight(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 9.5 * s, y: 5 * s), CGPoint(x: 16 * s, y: 12 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 16 * s, y: 12 * s), CGPoint(x: 9.5 * s, y: 19 * s)), tint, s)
        }
    }

    /// ☰ 菜单：各主页面左上角呼出抽屉侧边栏
    static func menu(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 4 * s, y: 7 * s), CGPoint(x: 20 * s, y: 7 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 4 * s, y: 12 * s), CGPoint(x: 20 * s, y: 12 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 4 * s, y: 17 * s), CGPoint(x: 20 * s, y: 17 * s)), tint, s)
        }
    }

    /// ∨ 展开：引导项按钮右侧的 chevron
    static func chevronDown(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 6 * s, y: 9.5 * s), CGPoint(x: 12 * s, y: 15.5 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 15.5 * s), CGPoint(x: 18 * s, y: 9.5 * s)), tint, s)
        }
    }

    /// 放大镜：搜索框前置图标
    static func search(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(Path(ellipseIn: CGRect(x: 4.5 * s, y: 4.5 * s, width: 12 * s, height: 12 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 15 * s, y: 15 * s), CGPoint(x: 20 * s, y: 20 * s)), tint, s)
        }
    }

    /// ⟳ 刷新（弧 + 箭头）：顶栏刷新键
    static func refresh(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            let rect = CGRect(x: 3.5 * s, y: 3.5 * s, width: 17 * s, height: 17 * s)
            ctx.strokeLine(ellipseArcPath(rect: rect, start: -55, sweep: 305), tint, s)
            ctx.strokeLine(line(CGPoint(x: 20.5 * s, y: 3.5 * s), CGPoint(x: 20.5 * s, y: 8.6 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 20.5 * s, y: 8.6 * s), CGPoint(x: 15.4 * s, y: 8.6 * s)), tint, s)
        }
    }

    /// ? 帮助：顶栏帮助键（打开操作说明 sheet）
    static func help(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(Path(ellipseIn: CGRect(x: 3 * s, y: 3 * s, width: 18 * s, height: 18 * s)), tint, s)
            let rect = CGRect(x: 8.6 * s, y: 6.6 * s, width: 6.8 * s, height: 6.6 * s)
            ctx.strokeLine(ellipseArcPath(rect: rect, start: 165, sweep: 215), tint, s)
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 13.4 * s), CGPoint(x: 12 * s, y: 15.2 * s)), tint, s)
            ctx.dot(CGPoint(x: 12 * s, y: 18 * s), 0.95 * s, tint)
        }
    }

    /// ⋮ 更多：卡片右上角的操作菜单入口
    static func more(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.dot(CGPoint(x: 12 * s, y: 5.5 * s), 1.15 * s, tint)
            ctx.dot(CGPoint(x: 12 * s, y: 12 * s), 1.15 * s, tint)
            ctx.dot(CGPoint(x: 12 * s, y: 18.5 * s), 1.15 * s, tint)
        }
    }

    /// 地球：行菜单「在浏览器打开」
    static func globe(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(Path(ellipseIn: CGRect(x: 3 * s, y: 3 * s, width: 18 * s, height: 18 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 3 * s, y: 12 * s), CGPoint(x: 21 * s, y: 12 * s)), tint, s)
            ctx.strokeLine(ellipseArcPath(rect: CGRect(x: 7.4 * s, y: 3 * s, width: 9.2 * s, height: 18 * s),
                                          start: 90, sweep: 180), tint, s)
            ctx.strokeLine(ellipseArcPath(rect: CGRect(x: 7.4 * s, y: 3 * s, width: 9.2 * s, height: 18 * s),
                                          start: 270, sweep: 180), tint, s)
        }
    }

    /// 显示器（方框版）：行菜单「打开页面」
    static func screen(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            var box = Path()
            box.move(to: CGPoint(x: 3 * s, y: 4.5 * s))
            box.addLine(to: CGPoint(x: 21 * s, y: 4.5 * s))
            box.addLine(to: CGPoint(x: 21 * s, y: 16.5 * s))
            box.addLine(to: CGPoint(x: 3 * s, y: 16.5 * s))
            box.closeSubpath()
            ctx.strokeLine(box, tint, s)
            ctx.strokeLine(line(CGPoint(x: 8.5 * s, y: 20.5 * s), CGPoint(x: 15.5 * s, y: 20.5 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 16.5 * s), CGPoint(x: 12 * s, y: 20.5 * s)), tint, s)
        }
    }

    /// ✎ 铅笔（细长版）：行菜单「编辑这台电脑」
    static func pencilSmall(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            var body = Path()
            body.move(to: CGPoint(x: 4 * s, y: 20 * s))
            body.addLine(to: CGPoint(x: 8 * s, y: 20 * s))
            body.addLine(to: CGPoint(x: 19.5 * s, y: 8.5 * s))
            body.addLine(to: CGPoint(x: 15.5 * s, y: 4.5 * s))
            body.addLine(to: CGPoint(x: 4 * s, y: 16 * s))
            body.closeSubpath()
            ctx.strokeLine(body, tint, s)
            ctx.strokeLine(line(CGPoint(x: 13.5 * s, y: 6.5 * s), CGPoint(x: 17.5 * s, y: 10.5 * s)), tint, s)
        }
    }

    /// ⚡ 开机（唤醒）：电源四键之一
    static func power(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            var path = Path()
            path.move(to: CGPoint(x: 13 * s, y: 2 * s))
            path.addLine(to: CGPoint(x: 4.5 * s, y: 13.5 * s))
            path.addLine(to: CGPoint(x: 11 * s, y: 13.5 * s))
            path.addLine(to: CGPoint(x: 10 * s, y: 22 * s))
            path.addLine(to: CGPoint(x: 19.5 * s, y: 10 * s))
            path.addLine(to: CGPoint(x: 13 * s, y: 10 * s))
            path.closeSubpath()
            ctx.strokeLine(path, tint, s)
        }
    }

    /// 月亮：睡眠
    static func moon(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            var path = Path()
            path.move(to: CGPoint(x: 12.5 * s, y: 3 * s))
            path.addCurve(to: CGPoint(x: 5.2 * s, y: 15.2 * s),
                          control1: CGPoint(x: 6.2 * s, y: 3.6 * s),
                          control2: CGPoint(x: 2.6 * s, y: 9.6 * s))
            path.addCurve(to: CGPoint(x: 18.4 * s, y: 17.4 * s),
                          control1: CGPoint(x: 7.6 * s, y: 20.4 * s),
                          control2: CGPoint(x: 14.4 * s, y: 21.4 * s))
            path.addCurve(to: CGPoint(x: 12.5 * s, y: 3 * s),
                          control1: CGPoint(x: 13.4 * s, y: 17.6 * s),
                          control2: CGPoint(x: 8.6 * s, y: 12.2 * s))
            path.closeSubpath()
            ctx.strokeLine(path, tint, s)
        }
    }

    /// ↻ 重启：弧 + 箭头
    static func restart(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            let rect = CGRect(x: 3.5 * s, y: 3.5 * s, width: 17 * s, height: 17 * s)
            ctx.strokeLine(ellipseArcPath(rect: rect, start: -20, sweep: 300), tint, s)
            ctx.strokeLine(line(CGPoint(x: 20.5 * s, y: 2.6 * s), CGPoint(x: 20.5 * s, y: 8.2 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 20.5 * s, y: 8.2 * s), CGPoint(x: 15.4 * s, y: 8.2 * s)), tint, s)
        }
    }

    /// ⏻ 关机：电源符号
    static func powerOff(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 3 * s), CGPoint(x: 12 * s, y: 12.2 * s)), tint, s)
            let rect = CGRect(x: 3.5 * s, y: 4.2 * s, width: 17 * s, height: 17 * s)
            ctx.strokeLine(ellipseArcPath(rect: rect, start: -60, sweep: 300), tint, s)
        }
    }

    /// ✓ 勾：底部选择器里的「当前选中」标记
    static func check(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 4.5 * s, y: 12.6 * s), CGPoint(x: 9.6 * s, y: 17.6 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 9.6 * s, y: 17.6 * s), CGPoint(x: 19.5 * s, y: 6.2 * s)), tint, s)
        }
    }

    /// ✕ 关闭：sheet 右上角
    static func close(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 6 * s, y: 6 * s), CGPoint(x: 18 * s, y: 18 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 18 * s, y: 6 * s), CGPoint(x: 6 * s, y: 18 * s)), tint, s)
        }
    }

    /// ⓘ 提示：待生效提示条前缀
    static func info(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(Path(ellipseIn: CGRect(x: 3 * s, y: 3 * s, width: 18 * s, height: 18 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 11 * s), CGPoint(x: 12 * s, y: 16.6 * s)), tint, s)
            ctx.dot(CGPoint(x: 12 * s, y: 7.8 * s), 0.95 * s, tint)
        }
    }

    /// ＋ 加号：添加电脑 / 添加配对
    static func plus(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 12 * s, y: 5 * s), CGPoint(x: 12 * s, y: 19 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 5 * s, y: 12 * s), CGPoint(x: 19 * s, y: 12 * s)), tint, s)
        }
    }

    /// 齿轮：服务器页顶栏「终端设置」
    static func gear(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(Path(ellipseIn: CGRect(x: 8.4 * s, y: 8.4 * s, width: 7.2 * s, height: 7.2 * s)), tint, s)
            ctx.strokeLine(Path(ellipseIn: CGRect(x: 3.4 * s, y: 3.4 * s, width: 17.2 * s, height: 17.2 * s)), tint, s)
            for index in 0..<8 {
                let angle = CGFloat(index) * .pi / 4
                let inner = CGPoint(x: 12 * s + cos(angle) * 8.6 * s,
                                    y: 12 * s + sin(angle) * 8.6 * s)
                let outer = CGPoint(x: 12 * s + cos(angle) * 11 * s,
                                    y: 12 * s + sin(angle) * 11 * s)
                ctx.strokeLine(line(inner, outer), tint, s)
            }
        }
    }

    /// 相机扫码：配对粘贴框右侧
    static func qrCode(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(roundedBox(CGRect(x: 3 * s, y: 3 * s, width: 6 * s, height: 6 * s), 1 * s), tint, s)
            ctx.strokeLine(roundedBox(CGRect(x: 15 * s, y: 3 * s, width: 6 * s, height: 6 * s), 1 * s), tint, s)
            ctx.strokeLine(roundedBox(CGRect(x: 3 * s, y: 15 * s, width: 6 * s, height: 6 * s), 1 * s), tint, s)
            ctx.strokeLine(line(CGPoint(x: 15 * s, y: 15 * s), CGPoint(x: 21 * s, y: 15 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 18 * s, y: 15 * s), CGPoint(x: 18 * s, y: 21 * s)), tint, s)
        }
    }

    /// 文件夹打开（「在终端打开」/「定位」）
    static func folderOpen(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            var path = Path()
            path.move(to: CGPoint(x: 2 * s, y: 18.5 * s))
            path.addLine(to: CGPoint(x: 2 * s, y: 5.5 * s))
            path.addLine(to: CGPoint(x: 7.5 * s, y: 5.5 * s))
            path.addLine(to: CGPoint(x: 9.5 * s, y: 8 * s))
            path.addLine(to: CGPoint(x: 20 * s, y: 8 * s))
            ctx.strokeLine(path, tint, s)
            var inner = Path()
            inner.move(to: CGPoint(x: 2 * s, y: 18.5 * s))
            inner.addLine(to: CGPoint(x: 5 * s, y: 11 * s))
            inner.addLine(to: CGPoint(x: 22 * s, y: 11 * s))
            inner.addLine(to: CGPoint(x: 19 * s, y: 18.5 * s))
            inner.closeSubpath()
            ctx.strokeLine(inner, tint, s)
        }
    }

    /// 双向箭头：传输页方向标识
    static func swapVertical(_ tint: Color, size: CGFloat = 20) -> DrawnIcon {
        DrawnIcon(tint, size: size) { ctx, s in
            ctx.strokeLine(line(CGPoint(x: 8 * s, y: 4 * s), CGPoint(x: 8 * s, y: 20 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 4.5 * s, y: 7.5 * s), CGPoint(x: 8 * s, y: 4 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 11.5 * s, y: 7.5 * s), CGPoint(x: 8 * s, y: 4 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 16 * s, y: 20 * s), CGPoint(x: 16 * s, y: 4 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 12.5 * s, y: 16.5 * s), CGPoint(x: 16 * s, y: 20 * s)), tint, s)
            ctx.strokeLine(line(CGPoint(x: 19.5 * s, y: 16.5 * s), CGPoint(x: 16 * s, y: 20 * s)), tint, s)
        }
    }
}

// 允许直接写 `DrawnIcon.serverStack(...)`：上面扩展里的 static 方法已满足
