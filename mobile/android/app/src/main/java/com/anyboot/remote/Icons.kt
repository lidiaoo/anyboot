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

/** 上下双箭头（备份与恢复：导出↑ / 导入↓） */
@Composable
fun SwapVerticalIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    // ↑（导出）
    drawLine(tint, Offset(9f * s, 15f * s), Offset(9f * s, 4f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(5.5f * s, 7.5f * s), Offset(9f * s, 4f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(12.5f * s, 7.5f * s), Offset(9f * s, 4f * s), 1.8f * s, StrokeCap.Round)
    // ↓（导入）
    drawLine(tint, Offset(15f * s, 9f * s), Offset(15f * s, 20f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(11.5f * s, 16.5f * s), Offset(15f * s, 20f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(18.5f * s, 16.5f * s), Offset(15f * s, 20f * s), 1.8f * s, StrokeCap.Round)
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

/** 下载（↓ + 底部托盘）：文件动作菜单「下载到本机」 */
@Composable
fun DownloadIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawLine(tint, Offset(12f * s, 3f * s), Offset(12f * s, 15f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(7f * s, 10f * s), Offset(12f * s, 15f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(17f * s, 10f * s), Offset(12f * s, 15f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(4f * s, 21f * s), Offset(20f * s, 21f * s), 1.8f * s, StrokeCap.Round)
}

/** 铅笔：文件动作菜单「重命名」/ 更多操作 */
@Composable
fun PencilIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawLine(tint, Offset(12f * s, 20f * s), Offset(21f * s, 20f * s), 1.8f * s, StrokeCap.Round)
    val path = Path().apply {
        moveTo(16.5f * s, 3.5f * s)
        lineTo(19.5f * s, 6.5f * s)
        lineTo(7f * s, 19f * s)
        lineTo(3f * s, 20f * s)
        lineTo(4f * s, 16f * s)
        close()
    }
    drawPath(path, tint, style = Stroke(1.8f * s, cap = StrokeCap.Round))
}

/** 复制（双层方块）：文件动作菜单「复制路径」/ 更多操作 */
@Composable
fun CopyIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawRoundRect(
        tint, topLeft = Offset(9f * s, 9f * s), size = Size(12f * s, 12f * s),
        cornerRadius = CornerRadius(2f * s), style = pen(s),
    )
    val path = Path().apply {
        moveTo(5f * s, 15f * s)
        lineTo(4f * s, 15f * s)
        lineTo(4f * s, 4f * s)
        lineTo(15f * s, 4f * s)
        lineTo(15f * s, 5f * s)
    }
    drawPath(path, tint, style = Stroke(1.8f * s, cap = StrokeCap.Round))
}

/** 垃圾桶：文件动作菜单「删除」/ 密钥/服务器删除入口 */
@Composable
fun TrashIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawLine(tint, Offset(3f * s, 6f * s), Offset(21f * s, 6f * s), 1.8f * s, StrokeCap.Round)
    val lid = Path().apply {
        moveTo(8f * s, 6f * s)
        lineTo(8f * s, 4f * s)
        lineTo(16f * s, 4f * s)
        lineTo(16f * s, 6f * s)
    }
    drawPath(lid, tint, style = Stroke(1.8f * s, cap = StrokeCap.Round))
    val body = Path().apply {
        moveTo(19f * s, 6f * s)
        lineTo(18f * s, 20f * s)
        lineTo(6f * s, 20f * s)
        lineTo(5f * s, 6f * s)
    }
    drawPath(body, tint, style = Stroke(1.8f * s, cap = StrokeCap.Round))
}

// ----------------------------------------------------------------------
// 电脑列表页（design/mobile-computer-list.html）用到的图标
// ----------------------------------------------------------------------

/** ‹ 返回：各次级页面顶栏 */
@Composable
fun ChevronLeftIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawLine(tint, Offset(14.5f * s, 5f * s), Offset(8f * s, 12f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(8f * s, 12f * s), Offset(14.5f * s, 19f * s), 1.8f * s, StrokeCap.Round)
}

/** ☰ 菜单：各主页面左上角呼出抽屉侧边栏（Drawer.kt） */
@Composable
fun MenuIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawLine(tint, Offset(4f * s, 7f * s), Offset(20f * s, 7f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(4f * s, 12f * s), Offset(20f * s, 12f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(4f * s, 17f * s), Offset(20f * s, 17f * s), 1.8f * s, StrokeCap.Round)
}

/** ∨ 展开：引导项按钮右侧的 chevron */
@Composable
fun ChevronDownIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawLine(tint, Offset(6f * s, 9.5f * s), Offset(12f * s, 15.5f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(12f * s, 15.5f * s), Offset(18f * s, 9.5f * s), 1.8f * s, StrokeCap.Round)
}

/** 放大镜：搜索框前置图标 */
@Composable
fun SearchIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawCircle(tint, radius = 6f * s, center = Offset(10.5f * s, 10.5f * s), style = pen(s))
    drawLine(tint, Offset(15f * s, 15f * s), Offset(20f * s, 20f * s), 1.8f * s, StrokeCap.Round)
}

/** ⟳ 刷新（弧 + 箭头）：顶栏刷新键，转起来表示正在探测 */
@Composable
fun RefreshIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawArc(
        tint, startAngle = -55f, sweepAngle = 305f, useCenter = false,
        topLeft = Offset(3.5f * s, 3.5f * s), size = Size(17f * s, 17f * s), style = pen(s),
    )
    drawLine(tint, Offset(20.5f * s, 3.5f * s), Offset(20.5f * s, 8.6f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(20.5f * s, 8.6f * s), Offset(15.4f * s, 8.6f * s), 1.8f * s, StrokeCap.Round)
}

/** ? 帮助：顶栏帮助键（打开操作说明 sheet） */
@Composable
fun HelpIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawCircle(tint, radius = 9f * s, center = Offset(12f * s, 12f * s), style = pen(s))
    drawArc(
        tint, startAngle = 165f, sweepAngle = 215f, useCenter = false,
        topLeft = Offset(8.6f * s, 6.6f * s), size = Size(6.8f * s, 6.6f * s), style = pen(s),
    )
    drawLine(tint, Offset(12f * s, 13.4f * s), Offset(12f * s, 15.2f * s), 1.8f * s, StrokeCap.Round)
    drawCircle(tint, radius = 0.95f * s, center = Offset(12f * s, 18f * s))
}

/** ⋮ 更多：卡片右上角的操作菜单入口 */
@Composable
fun MoreIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawCircle(tint, radius = 1.15f * s, center = Offset(12f * s, 5.5f * s))
    drawCircle(tint, radius = 1.15f * s, center = Offset(12f * s, 12f * s))
    drawCircle(tint, radius = 1.15f * s, center = Offset(12f * s, 18.5f * s))
}

/** ⊕ 地球：行菜单「在浏览器打开」 */
@Composable
fun GlobeIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawCircle(tint, radius = 9f * s, center = Offset(12f * s, 12f * s), style = pen(s))
    // 赤道
    drawLine(tint, Offset(3f * s, 12f * s), Offset(21f * s, 12f * s), 1.8f * s, StrokeCap.Round)
    // 经线（一个椭圆，两段弧拼）
    drawArc(
        tint, startAngle = 90f, sweepAngle = 180f, useCenter = false,
        topLeft = Offset(7.4f * s, 3f * s), size = Size(9.2f * s, 18f * s), style = pen(s),
    )
    drawArc(
        tint, startAngle = 270f, sweepAngle = 180f, useCenter = false,
        topLeft = Offset(7.4f * s, 3f * s), size = Size(9.2f * s, 18f * s), style = pen(s),
    )
}

/** ▭ 显示器：行菜单「打开页面」 */
@Composable
fun ScreenIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    val box = Path().apply {
        moveTo(3f * s, 4.5f * s)
        lineTo(21f * s, 4.5f * s)
        lineTo(21f * s, 16.5f * s)
        lineTo(3f * s, 16.5f * s)
        close()
    }
    drawPath(box, tint, style = pen(s))
    drawLine(tint, Offset(8.5f * s, 20.5f * s), Offset(15.5f * s, 20.5f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(12f * s, 16.5f * s), Offset(12f * s, 20.5f * s), 1.8f * s, StrokeCap.Round)
}

/** ✎ 铅笔：行菜单「编辑这台电脑」 */
@Composable
fun PencilIconSmall(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    val body = Path().apply {
        moveTo(4f * s, 20f * s)
        lineTo(8f * s, 20f * s)
        lineTo(19.5f * s, 8.5f * s)
        lineTo(15.5f * s, 4.5f * s)
        lineTo(4f * s, 16f * s)
        close()
    }
    drawPath(body, tint, style = pen(s))
    drawLine(tint, Offset(13.5f * s, 6.5f * s), Offset(17.5f * s, 10.5f * s), 1.8f * s, StrokeCap.Round)
}

/** ⚡ 开机（唤醒）：电源四键之一 */
@Composable
fun PowerIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    val path = Path().apply {
        moveTo(13f * s, 2f * s)
        lineTo(4.5f * s, 13.5f * s)
        lineTo(11f * s, 13.5f * s)
        lineTo(10f * s, 22f * s)
        lineTo(19.5f * s, 10f * s)
        lineTo(13f * s, 10f * s)
        close()
    }
    drawPath(path, tint, style = Stroke(1.8f * s, cap = StrokeCap.Round))
}

/** 月亮：睡眠 */
@Composable
fun MoonIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    val path = Path().apply {
        moveTo(12.5f * s, 3f * s)
        // 大圆弧：从 (12.5,3) 绕到 (12.6,21)，右侧被矢量切掉的那块
        cubicTo(6.2f * s, 3.6f * s, 2.6f * s, 9.6f * s, 5.2f * s, 15.2f * s)
        cubicTo(7.6f * s, 20.4f * s, 14.4f * s, 21.4f * s, 18.4f * s, 17.4f * s)
        cubicTo(13.4f * s, 17.6f * s, 8.6f * s, 12.2f * s, 12.5f * s, 3f * s)
        close()
    }
    drawPath(path, tint, style = Stroke(1.8f * s, cap = StrokeCap.Round))
}

/** ↻ 重启：弧 + 箭头（与刷新同形，语义不同，独立函数便于以后区分） */
@Composable
fun RestartIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawArc(
        tint, startAngle = -20f, sweepAngle = 300f, useCenter = false,
        topLeft = Offset(3.5f * s, 3.5f * s), size = Size(17f * s, 17f * s), style = pen(s),
    )
    drawLine(tint, Offset(20.5f * s, 2.6f * s), Offset(20.5f * s, 8.2f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(20.5f * s, 8.2f * s), Offset(15.4f * s, 8.2f * s), 1.8f * s, StrokeCap.Round)
}

/** ⏻ 关机：电源符号 */
@Composable
fun PowerOffIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawLine(tint, Offset(12f * s, 3f * s), Offset(12f * s, 12.2f * s), 1.8f * s, StrokeCap.Round)
    drawArc(
        tint, startAngle = -60f, sweepAngle = 300f, useCenter = false,
        topLeft = Offset(3.5f * s, 4.2f * s), size = Size(17f * s, 17f * s), style = pen(s),
    )
}

/** ✓ 勾：底部选择器里的「当前选中」标记 / toast 前缀 */
@Composable
fun CheckIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawLine(tint, Offset(4.5f * s, 12.6f * s), Offset(9.6f * s, 17.6f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(9.6f * s, 17.6f * s), Offset(19.5f * s, 6.2f * s), 1.8f * s, StrokeCap.Round)
}

/** ✕ 关闭：sheet 右上角 */
@Composable
fun CloseIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawLine(tint, Offset(6f * s, 6f * s), Offset(18f * s, 18f * s), 1.8f * s, StrokeCap.Round)
    drawLine(tint, Offset(18f * s, 6f * s), Offset(6f * s, 18f * s), 1.8f * s, StrokeCap.Round)
}

/** ⓘ 提示：待生效提示条前缀（描边版，比 material 的实心圆更贴设计稿） */
@Composable
fun InfoIcon(tint: Color, size: Dp = 20.dp, modifier: Modifier = Modifier) = DrawnIcon(size, tint, modifier) {
    val s = it
    drawCircle(tint, radius = 9f * s, center = Offset(12f * s, 12f * s), style = pen(s))
    drawLine(tint, Offset(12f * s, 11f * s), Offset(12f * s, 16.6f * s), 1.8f * s, StrokeCap.Round)
    drawCircle(tint, radius = 0.95f * s, center = Offset(12f * s, 7.8f * s))
}
