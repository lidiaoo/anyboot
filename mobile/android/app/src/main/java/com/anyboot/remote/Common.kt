package com.anyboot.remote

import android.content.Context
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.util.Locale
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.runBlocking

/**
 * 应用主题壳：语义色板（LocalPalette）+ Material3 colorScheme 一起配好，
 * 所有 M3 控件（菜单/对话框/输入框）自动跟上我们的深浅两套 token。
 */
@Composable
fun AppTheme(content: @Composable () -> Unit) {
    val context = LocalContext.current
    val palette = remember(context) { paletteOf(context) }
    val dark = Ui.dark(context)
    MaterialTheme(
        colorScheme = if (dark) darkColorScheme(
            primary = palette.accent, onPrimary = palette.onAccent,
            background = palette.base, surface = palette.card, onSurface = palette.fg,
            surfaceVariant = palette.elevate, onSurfaceVariant = palette.fgDim,
            outline = palette.lineStrong, error = palette.bad,
        ) else lightColorScheme(
            primary = palette.accent, onPrimary = palette.onAccent,
            background = palette.base, surface = palette.card, onSurface = palette.fg,
            surfaceVariant = palette.elevate, onSurfaceVariant = palette.fgDim,
            outline = palette.lineStrong, error = palette.bad,
        )
    ) {
        CompositionLocalProvider(LocalPalette provides palette, content = content)
    }
}

/** TerminalRegistry 的多播：静态观察者只有一个坑位，这里转发给所有登记的监听者 */
object SessionEvents {
    private val listeners = mutableListOf<() -> Unit>()

    init {
        TerminalRegistry.setObserver { listeners.toList().forEach { it() } }
    }

    fun register(listener: () -> Unit) {
        listeners.add(listener)
    }

    fun unregister(listener: () -> Unit) {
        listeners.remove(listener)
    }
}

fun Context.toast(message: String, long: Boolean = false) {
    Toast.makeText(this, message, if (long) Toast.LENGTH_LONG else Toast.LENGTH_SHORT).show()
}

/**
 * 同步等用户点按钮的确认框（TOFU 主机指纹）。SSH 连接线程里调 [confirm] 阻塞等待，
 * Compose 侧挂 [ConfirmDialog] 展示弹窗 —— 谁点了谁放行。
 *
 * 注意：[confirm] 必须在后台线程调用（runBlocking 等 UI），和旧实现语义一致。
 */
class ConfirmBus {

    class Request(val title: String, val message: String, val deferred: CompletableDeferred<Boolean>)

    var request by mutableStateOf<Request?>(null)
        private set

    fun confirm(title: String, message: String): Boolean {
        val deferred = CompletableDeferred<Boolean>()
        request = Request(title, message, deferred)
        return runBlocking { deferred.await() }
    }

    fun complete(accepted: Boolean) {
        val current = request ?: return
        request = null
        current.deferred.complete(accepted)
    }
}

/** Compose 侧的指纹确认弹窗（挂在每棵树根部） */
@Composable
fun ConfirmDialog(bus: ConfirmBus) {
    val request = bus.request ?: return
    AlertDialog(
        onDismissRequest = { /* 不可点外面关 —— 必须明确选信任/取消 */ },
        title = { Text(request.title) },
        text = { Text(request.message) },
        confirmButton = {
            TextButton(onClick = { bus.complete(true) }) { Text("信任并连接") }
        },
        dismissButton = {
            TextButton(onClick = { bus.complete(false) }) { Text("取消") }
        },
    )
}

// ----------------------------------------------------------------------
// 共用小件（对齐旧实现的观感）
// ----------------------------------------------------------------------

/** 分组标题（进行中 / 历史 / 密钥…） */
@Composable
fun SectionTitle(text: String) {
    val palette = LocalPalette.current
    Text(
        text = text,
        fontSize = 14.sp,
        fontWeight = FontWeight.Bold,
        color = palette.fgDim,
        modifier = Modifier.padding(start = 12.dp, top = 14.dp, end = 12.dp, bottom = 4.dp),
    )
}

/** 空状态提示 */
@Composable
fun EmptyHint(text: String) {
    val palette = LocalPalette.current
    Text(
        text = text,
        fontSize = 13.sp,
        color = palette.fgFaint,
        modifier = Modifier.padding(horizontal = 20.dp, vertical = 16.dp),
    )
}

/** 次要文字（副行 / 说明） */
@Composable
fun SecondaryText(text: String, modifier: Modifier = Modifier) {
    val palette = LocalPalette.current
    Text(
        text = text,
        fontSize = 12.sp,
        color = palette.fgDim,
        modifier = modifier,
    )
}

/** 小按钮 chip（强调色文字 + 圆角抬升底），旧 smallButton 的对应物 */
@Composable
fun ChipButton(text: String, onClick: () -> Unit) {
    val palette = LocalPalette.current
    Text(
        text = text,
        fontSize = 12.sp,
        color = palette.accent,
        modifier = Modifier
            .clip(RoundedCornerShape(12.dp))
            .background(palette.elevate)
            .padding(horizontal = 10.dp, vertical = 6.dp)
            .clickable(onClick = onClick),
    )
}

/** 顶栏 / 按键栏的纯文字图标按钮（☰ ＋ ▹ 之类） */
@Composable
fun BarIcon(text: String, description: String, tint: Color? = null, onClick: () -> Unit) {
    val palette = LocalPalette.current
    Text(
        text = text,
        fontSize = 17.sp,
        color = tint ?: palette.fgDim,
        modifier = Modifier
            .padding(horizontal = 12.dp, vertical = 8.dp)
            .clickable(onClick = onClick),
    )
}

/** 表单字段上方的小标签 */
@Composable
fun FieldLabel(text: String) {
    val palette = LocalPalette.current
    Text(
        text,
        fontSize = 12.sp,
        color = palette.fgDim,
        modifier = Modifier.padding(top = 10.dp, bottom = 2.dp),
    )
}

/** 列表行之间的细分隔线（缩进对齐文字列，避开徽标） */
@Composable
fun ListDivider() {
    val palette = LocalPalette.current
    Box(
        Modifier
            .padding(start = 58.dp, end = 12.dp)
            .height(1.dp)
            .fillMaxWidth()
            .background(palette.line)
    )
}

fun humanDuration(ms: Long): String {
    val seconds = ms / 1000
    if (seconds < 60) return "$seconds 秒"
    val minutes = seconds / 60
    if (minutes < 60) return "$minutes 分钟"
    return "${minutes / 60} 小时 ${minutes % 60} 分"
}

fun humanSize(bytes: Long): String {
    if (bytes < 1024) return "$bytes B"
    if (bytes < 1024 * 1024) return String.format(Locale.US, "%.1f KB", bytes / 1024.0)
    return String.format(Locale.US, "%.1f MB", bytes / 1024.0 / 1024.0)
}
