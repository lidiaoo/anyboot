package com.anyboot.remote

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * 设计稿（docs/client-ui/mobile-redesign-all.html）里的线性小图标。
 *
 * material-icons-core 的图标集太窄（没有显示器 / 服务器 / 终端 / 钥匙 / 文件夹 / 文件），
 * 这里按 24dp 网格用 Canvas 手绘：线宽 1.8、圆头圆角，观感对齐设计稿的 stroke 图标。
 * 坐标全部乘以 [DrawScope.scale]（实际渲染尺寸 / 24），换尺寸不变形。
 */

/** 24dp 网格 → 实际尺寸的缩放系数与描边 */
private fun DrawScope.grid(): Float = this.size.minDimension / 24f

private fun DrawScope.pen(grid: Float): Stroke =
    Stroke(width = 1.8f * grid, cap = StrokeCap.Round)

@Composable
private fun DrawnIcon(
    size: Dp,
    tint: Color,
    modifier: Modifier = Modifier,
    draw: DrawScope.(Float) -> Unit,
) {
    Canvas(modifier.size(size)) {
        val grid = this.grid()
        draw(grid)
    }
}

/** 显示器（抽屉头部 / 终端按钮 / 关于页主图） */
@Composable
fun DesktopIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawRoundRect(
        tint, topLeft = Offset(3f * s, 4f * s), size = Size(18f * s, 14f * s),
        cornerRadius = CornerRadius(2f * s), style = pen(s),
    )
    drawLine(tint, Offset(8f * s, 21f * s), Offset(16f * s, 21f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(12f * s, 18f * s), Offset(12f * s, 21f * s), 1.8f * s, StrokeCap.Round)
}

/** 服务器机架（两层 + 指示点）：底部导航「服务器」/ 服务器卡片 */
@Composable
fun ServerStackIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawRoundRect(
        tint, topLeft = Offset(2f * s, 3f * s), size = Size(20f * s, 7f * s),
        cornerRadius = CornerRadius(2f * s), style = pen(s),
    )
    drawRoundRect(
        tint, topLeft = Offset(2f * s, 14f * s), size = Size(20f * s, 7f * s),
        cornerRadius = CornerRadius(2f * s), style = pen(s),
    )
    drawCircle(tint, radius = 0.9f * s, center = Offset(6f * s, 6.5f * s))
    drawCircle(tint, radius = 0.9f * s, center = Offset(6f * s, 17.5f * s))
}

/** 终端提示符（方框 + ›）：底部导航「终端」/ 卡片终端按钮 / 终端页顶栏 */
@Composable
fun TermIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawRoundRect(
        tint, topLeft = Offset(2f * s, 4f * s), size = Size(20f * s, 14f * s),
        cornerRadius = CornerRadius(2f * s), style = pen(s),
    )
    drawLine(tint, Offset(6f * s, 9f * s), Offset(9f * s, 12f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(9f * s, 12f * s), Offset(6f * s, 15f * s), 1.8f * s, StrokeCap.Round)
}

/** 钥匙：底部导航「密钥」/ 密钥卡 */
@Composable
fun KeyIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawCircle(tint, radius = 4f * s, center = Offset(8f * s, 15f * s), style = pen(s))
    drawLine(tint, Offset(10.8f * s, 12.2f * s), Offset(20f * s, 3f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(17f * s, 6f * s), Offset(19.5f * s, 8.5f * s), 1.8f * s, StrokeCap.Round)
}

/** 文件夹：底部导航「文件」/ 文件列表目录行 */
@Composable
fun FolderIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    val path = Path().apply {
        moveTo(2f * s, 18.5f * s)
        lineTo(2f * s, 5.5f * s)
        lineTo(7.5f * s, 5.5f * s)
        lineTo(9.5f * s, 8f * s)
        lineTo(20f * s, 8f * s)
        lineTo(22f * s, 10f * s)
        lineTo(22f * s, 18.5f * s)
        close()
    }
    drawPath(path, tint, style = Stroke(1.8f * s, cap = StrokeCap.Round, pathEffect = null))
}

/** 文档（折角）：文件列表文件行 */
@Composable
fun DocFileIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    val path = Path().apply {
        moveTo(6f * s, 2.5f * s)
        lineTo(13.5f * s, 2.5f * s)
        lineTo(20f * s, 9f * s)
        lineTo(20f * s, 20f * s)
        lineTo(6f * s, 20f * s)
        close()
    }
    drawPath(path, tint, style = Stroke(1.8f * s, cap = StrokeCap.Round))
    drawLine(tint, Offset(13.5f * s, 2.5f * s), Offset(13.5f * s, 9f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(13.5f * s, 9f * s), Offset(20f * s, 9f * s), 1.8f * s, StrokeCap.Round)
}

/** 闪电（脚本 / 可执行文件行） */
@Composable
fun BoltIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    val path = Path().apply {
        moveTo(13f * s, 2f * s)
        lineTo(3f * s, 14f * s)
        lineTo(10f * s, 14f * s)
        lineTo(9f * s, 22f * s)
        lineTo(19f * s, 10f * s)
        lineTo(12f * s, 10f * s)
        close()
    }
    drawPath(path, tint, style = Stroke(1.8f * s, cap = StrokeCap.Round))
}

/** 时钟（关于页「版本更新」行） */
@Composable
fun ClockIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawCircle(tint, radius = 9f * s, center = Offset(12f * s, 12f * s), style = pen(s))
    drawLine(tint, Offset(12f * s, 7.5f * s), Offset(12f * s, 12f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(12f * s, 12f * s), Offset(14.5f * s, 14.5f * s), 1.8f * s, StrokeCap.Round)
}

/** 代码尖括号（GitHub 仓库卡 / 开源许可行） */
@Composable
fun CodeIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawLine(tint, Offset(9f * s, 18f * s), Offset(3.5f * s, 12f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(3.5f * s, 12f * s), Offset(9f * s, 6f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(15f * s, 6f * s), Offset(20.5f * s, 12f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(20.5f * s, 12f * s), Offset(15f * s, 18f * s), 1.8f * s, StrokeCap.Round)
}
