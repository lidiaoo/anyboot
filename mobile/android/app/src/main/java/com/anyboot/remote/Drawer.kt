package com.anyboot.remote

import android.app.Activity
import android.content.Context
import android.content.Intent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Info
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// ======================================================================
// 通用抽屉侧边栏：电脑页 / 电脑列表 / 服务器 三个入口的左上角 ☰ 共用。
// 原 MainActivity 私有实现上提：当前页高亮、主题循环通用化。
// ======================================================================

/** 抽屉导航的「当前页」标识（AppDrawerOverlay 的 current 参数） */
const val DRAWER_COMPUTERS = "computers" // 电脑页（MainActivity）
const val DRAWER_COMPUTER_LIST = "computer-list" // 电脑列表（ComputerListActivity）
const val DRAWER_SERVERS = "servers" // 服务器（ServersActivity）
const val DRAWER_BACKUP = "backup" // 备份与恢复（BackupActivity）

/** 应用版本号（抽屉头部「v*x* · remote」用） */
fun appVersionLabel(context: Context): String = try {
    context.packageManager.getPackageInfo(context.packageName, 0).versionName ?: "1.1"
} catch (ignored: Exception) {
    "1.1"
}

/**
 * 抽屉侧边栏（对齐设计稿 drawer 屏）：显示器图标块 + AnyBoot 版本行、
 * 导航（电脑 / 电脑列表 / 服务器 / 关于 / 主题）、底部心跳与 GitHub 说明。
 * 宽 290dp（窄屏最多 85%）；[current] 对应的导航行高亮；主题项是设计稿没有的
 * 既有功能，保留在导航末尾。调用方负责用状态控制显隐与 [onDismiss]。
 */
@Composable
fun AppDrawerOverlay(current: String, onDismiss: () -> Unit) {
    val palette = LocalPalette.current
    val context = LocalContext.current
    BoxWithConstraints(Modifier.fillMaxSize()) {
        // 抽屉宽度自适应：宽屏封顶 290dp（老观感不变），窄窗（分屏/折叠屏/小窗）最多占 85%
        val drawerWidth = minOf(290.dp, maxWidth * 0.85f)
        Box(Modifier.fillMaxSize()) {
            Box(
                Modifier
                    .fillMaxSize()
                    .background(Color(0x55000000))
                    .clickable { onDismiss() }
            )
            Column(
                Modifier
                    .fillMaxHeight()
                    .width(drawerWidth)
                    .background(palette.card)
                    .align(Alignment.CenterStart),
            ) {
                // 头部：图标块 + 名称/版本
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .drawBehind {
                            drawLine(palette.line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx())
                        }
                        .padding(horizontal = 20.dp, vertical = 28.dp)
                        .padding(top = 0.dp),
                ) {
                    Box(
                        Modifier
                            .size(44.dp)
                            .clip(RoundedCornerShape(16.dp))
                            .background(palette.accent.copy(alpha = 0.15f)),
                        contentAlignment = Alignment.Center,
                    ) {
                        DesktopIcon(palette.accent, 24.dp)
                    }
                    Spacer(Modifier.width(12.dp))
                    Column {
                        Text("AnyBoot", fontSize = 16.sp, fontWeight = FontWeight.Bold, color = palette.fg)
                        Text(
                            "v" + appVersionLabel(context) + " · remote",
                            fontSize = 10.sp,
                            color = palette.fgFaint,
                            modifier = Modifier.padding(top = 1.dp),
                        )
                    }
                }
                // 导航
                Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(12.dp)) {
                    DrawerNavRow("🖥", "电脑", selected = current == DRAWER_COMPUTERS,
                        iconSlot = { tint, size -> DesktopIcon(tint, size) }) {
                        onDismiss()
                        if (current != DRAWER_COMPUTERS) {
                            context.startActivity(Intent(context, MainActivity::class.java))
                        }
                    }
                    DrawerNavRow("🎛", "电脑列表", selected = current == DRAWER_COMPUTER_LIST,
                        iconSlot = { tint, size -> ServerStackIcon(tint, size) }) {
                        onDismiss()
                        if (current != DRAWER_COMPUTER_LIST) {
                            context.startActivity(Intent(context, ComputerListActivity::class.java))
                        }
                    }
                    DrawerNavRow("🖧", "服务器", selected = current == DRAWER_SERVERS,
                        iconSlot = { tint, size -> ServerStackIcon(tint, size) }) {
                        onDismiss()
                        if (current != DRAWER_SERVERS) {
                            context.startActivity(Intent(context, ServersActivity::class.java))
                        }
                    }
                    DrawerNavRow("⇅", "备份与恢复", selected = current == DRAWER_BACKUP,
                        iconSlot = { tint, size ->
                            SwapVerticalIcon(tint, size)
                        }) {
                        onDismiss()
                        context.startActivity(Intent(context, BackupActivity::class.java))
                    }
                    DrawerNavRow("ⓘ", "关于", iconSlot = { tint, size ->
                        Icon(Icons.Filled.Info, "关于", tint = tint,
                            modifier = Modifier.size(size))
                    }) {
                        onDismiss()
                        context.startActivity(Intent(context, AboutActivity::class.java))
                    }
                    DrawerNavRow("🌓", "主题：" + Ui.modeLabel(context)) {
                        Ui.cycle(context)
                        onDismiss()
                        (context as? Activity)?.recreate()
                    }
                }
                // 底部说明
                Column(
                    Modifier
                        .fillMaxWidth()
                        .drawBehind { drawLine(palette.line, Offset(0f, 0f), Offset(size.width, 0f), 1.dp.toPx()) }
                        .padding(16.dp),
                ) {
                    Text(
                        "心跳 10s · 并发探测\nGitHub · lidiaoo/anyboot",
                        fontSize = 10.sp,
                        lineHeight = 16.sp,
                        color = palette.fgFaint,
                    )
                    Text(
                        "外网连家里的电脑：先把家里能公网访问的机器加成 SSH 服务器，"
                                + "再在电脑的「经由跳板机」里选它。",
                        fontSize = 10.sp,
                        lineHeight = 15.sp,
                        color = palette.fgFaint,
                        modifier = Modifier.padding(top = 8.dp),
                    )
                }
            }
        }
    }
}

/**
 * 抽屉导航行：图标 + 文字（[selected] 高亮当前页）。
 * [iconSlot] 提供自绘线性图标；没提供就用 [icon] 文本（emoji 兜底）。
 */
@Composable
fun DrawerNavRow(
    icon: String,
    label: String,
    selected: Boolean = false,
    iconSlot: (@Composable (Color, Dp) -> Unit)? = null,
    onClick: () -> Unit,
) {
    val palette = LocalPalette.current
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(if (selected) palette.accent.copy(alpha = 0.12f) else Color.Transparent)
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 12.dp),
    ) {
        if (iconSlot != null) {
            iconSlot(if (selected) palette.accent else palette.fgDim, 20.dp)
        } else {
            Text(icon, fontSize = 16.sp)
        }
        Spacer(Modifier.width(12.dp))
        Text(
            label,
            fontSize = 14.sp,
            fontWeight = if (selected) FontWeight.SemiBold else null,
            color = if (selected) palette.accent else palette.fgDim,
            modifier = Modifier.weight(1f),
        )
    }
}
