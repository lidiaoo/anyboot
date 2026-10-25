package com.anyboot.remote

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextFieldDefaults
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import java.util.Locale
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.runBlocking
import org.json.JSONObject

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

/**
 * TransferManager 的多播：传输进度是从传输线程回调上来的，这里统一跳到主线程再通知，
 * 免得 Compose 状态在后台线程被写。
 */
object TransferEvents {
    private val listeners = mutableListOf<() -> Unit>()
    private val main = android.os.Handler(android.os.Looper.getMainLooper())

    init {
        TransferManager.setObserver {
            main.post { listeners.toList().forEach { it() } }
        }
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
 * 复制远程接入的某一项（地址 / 配对令牌），并且**一定给出结果提示**。
 *
 * 为什么不再让调用方自己写剪贴板：之前几处复制是「静默成功」，用户按完按钮屏幕上
 * 什么都没变，压根不知道有没有复制上（PC 端远程接入页也是同一个毛病）。这里统一
 * 收口：成功说「已复制<标签>」，失败说失败原因，空值直接告诉用户没东西可复制。
 *
 * @param label 提示里用的中文名（如「Cloudflare 隧道地址」）
 * @param value 要写入剪贴板的文本；会自动 trim，空白视为空
 */
fun Context.copyRemoteSlot(label: String, value: String) {
    val text = value.trim()
    if (text.isEmpty()) {
        toast("没有可复制的$label", long = true)
        return
    }
    try {
        val manager = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        // Android 13+ 系统自带「已复制」浮层，但各家 ROM 行为不一致（有的静默），
        // 我们照旧吐自己的 toast —— 有总比没有好，用户要的是确定感。
        manager.setPrimaryClip(ClipData.newPlainText("anyboot-$label", text))
        toast("已复制$label")
    } catch (e: Exception) {
        // 剪贴板在某些定制 ROM / 无窗口场景会抛异常，别让它把页面带崩
        toast("复制$label 失败：${e.message ?: e.javaClass.simpleName}", long = true)
    }
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

/**
 * 小按钮 chip（强调色文字 + 圆角抬升底），旧 smallButton 的对应物。
 *
 * [selected] 是「开着」的状态（如「显示隐藏」这类开关）—— 反色显示。
 * 注意它排在 [onClick] 前面：排后面的话 `ChipButton("上级") { … }` 这种尾随 lambda
 * 会去匹配 [selected]，直接编译不过。
 */
@Composable
fun ChipButton(text: String, selected: Boolean = false, onClick: () -> Unit) {
    val palette = LocalPalette.current
    Text(
        text = text,
        fontSize = 12.sp,
        color = if (selected) palette.onAccent else palette.accent,
        modifier = Modifier
            .clip(RoundedCornerShape(12.dp))
            .background(if (selected) palette.accent else palette.elevate)
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

/**
 * 「只读下拉触发框」的配色：把**禁用态**配成跟正常态一模一样。
 *
 * 为什么要吃这个亏：下拉触发框必须写成 `enabled = false`。
 * readOnly 的 TextField 自己会把点击吃成「聚焦」（边框绿一下），挂在它身上的
 * `Modifier.clickable` 一次都收不到事件 —— 下拉菜单于是永远打不开。
 * 只有 disabled 的 TextField 不处理指针，我们的 clickable 才会生效；代价是文字/边框
 * 默认变灰，所以在这里把四个禁用色配回来。
 *
 * 也试过「盖一层透明 Box 接点击」：菜单是能开，但那层会把输入框整个挡在无障碍树外
 * （`isVisibleToUser=false`），uiautomator 和 TalkBack 都读不到这个字段的文本。
 */
@Composable
fun disabledFieldColors(
    textColor: Color,
    borderColor: Color,
    labelColor: Color,
) = OutlinedTextFieldDefaults.colors(
    disabledTextColor = textColor,
    disabledBorderColor = borderColor,
    disabledLabelColor = labelColor,
    disabledTrailingIconColor = labelColor,
)

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

// ----------------------------------------------------------------------
// 系统族徽标 + 系统详情
// ----------------------------------------------------------------------

/** 系统族的展示名（Windows / macOS / Linux）；认不出返回空串（= 不画徽标） */
fun osShowName(os: String): String = when (ServerProfile.normalizeMachineOs(os)) {
    "windows" -> "Windows"
    "macos" -> "macOS"
    "linux" -> "Linux"
    else -> ""
}

/**
 * 系统族徽标：标签 chip / 电脑卡片上那一小撮「Windows · macOS · Linux」。
 *
 * <p>为什么值得占这点地方：同一个局域网或 tailnet 里往往 windows、mac、NAS(linux)
 * 混着放，光看名字分不清哪台是哪台；徽标让人扫一眼就定位到要操作的那台。
 * 系统族来自对方 `/health` 的 `os` 字段，探活时顺手记进档案，老服务端没有就留空
 * （这时什么都不画，不占位）。
 *
 * @param onDark 放在强调色（深底）上时用浅色文字
 * @param onClick 非空时徽标可点（用于打开系统详情）
 */
@Composable
fun OsBadge(os: String, onDark: Boolean = false, onClick: (() -> Unit)? = null) {
    val name = osShowName(os)
    if (name.isEmpty()) return
    val palette = LocalPalette.current
    val tint = if (onDark) Color.White.copy(alpha = 0.9f) else palette.fgDim
    Text(
        text = name,
        fontSize = 9.sp,
        fontWeight = FontWeight.SemiBold,
        color = tint,
        maxLines = 1,
        modifier = Modifier
            .clip(RoundedCornerShape(4.dp))
            .background(tint.copy(alpha = 0.15f))
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(horizontal = 4.dp, vertical = 1.dp),
    )
}

/** 系统详情的一行；顺序即展示顺序 */
class SysInfoRow(val label: String, val value: String)

/**
 * 读对方电脑的系统详情：`GET /api/system`（鉴权，camelCase 响应）。
 *
 * <p>**阻塞调用，必须在后台线程跑**（结果回主线程再显示）。字段缺失就跳过那一行
 * —— 老版本服务端没有 `/api/system`，会走异常分支，调用方显示错误原因。
 */
fun fetchSysInfoRows(endpoint: String, token: String, timeoutMs: Int = 5000): List<SysInfoRow> {
    val base = endpoint.trim()
    if (base.isEmpty()) throw Exception("还没有可用地址")
    val secret = token.trim()
    val url = (if (base.startsWith("http://") || base.startsWith("https://")) base else "http://$base") +
        "/api/system" + if (secret.isEmpty()) "" else "?token=" + URLEncoder.encode(secret, "UTF-8")
    val connection = URL(url).openConnection() as HttpURLConnection
    connection.requestMethod = "GET"
    connection.connectTimeout = timeoutMs
    connection.readTimeout = timeoutMs
    connection.instanceFollowRedirects = false
    try {
        val status = connection.responseCode
        if (status == 401) throw Exception("这台电脑不认这个令牌（401）—— 核对访问令牌")
        val stream = if (status >= 400) connection.errorStream else connection.inputStream
        val body = stream?.use { it.readBytes().toString(Charsets.UTF_8) } ?: ""
        if (status >= 400) throw Exception("读取系统信息失败（HTTP $status）")
        return parseSysInfo(JSONObject(body.trim()))
    } catch (error: Exception) {
        throw if (error.message.isNullOrEmpty()) Exception("读取系统信息失败") else error
    } finally {
        connection.disconnect()
    }
}

/** `/api/system` 的 JSON → 有序展示行（纯逻辑，缺字段就跳过） */
fun parseSysInfo(json: JSONObject): List<SysInfoRow> {
    val rows = mutableListOf<SysInfoRow>()
    fun add(label: String, value: String) {
        val text = value.trim()
        if (text.isNotEmpty()) rows.add(SysInfoRow(label, text))
    }
    val osName = json.optString("osName", "")
    val osVersion = json.optString("osVersion", "")
    add("系统", listOf(osName, osVersion).filter { it.isNotBlank() }.joinToString(" "))
    add("内核", json.optString("kernel", ""))
    add("架构", json.optString("arch", ""))
    add("主机名", json.optString("hostname", ""))
    val cores = json.optInt("cpuCores", 0)
    val cpu = json.optString("cpuModel", "")
    add("处理器", if (cores > 0) "$cpu（$cores 核）" else cpu)
    val total = json.optLong("memTotalMb", 0L)
    val free = json.optLong("memFreeMb", 0L)
    if (total > 0) {
        val text = if (free > 0) {
            String.format(Locale.US, "%.1f GB（可用 %.1f GB）", total / 1024.0, free / 1024.0)
        } else {
            String.format(Locale.US, "%.1f GB", total / 1024.0)
        }
        add("内存", text)
    }
    val uptime = json.optLong("uptimeSecs", 0L)
    if (uptime > 0) add("运行时长", humanDuration(uptime * 1000))
    val lanIps = json.optJSONArray("lanIps")
    if (lanIps != null && lanIps.length() > 0) {
        val values = mutableListOf<String>()
        for (index in 0 until lanIps.length()) {
            val value = lanIps.optString(index, "").trim()
            if (value.isNotEmpty()) values.add(value)
        }
        add("局域网地址", values.joinToString("  "))
    }
    val port = json.optInt("httpPort", 0)
    if (port > 0) add("服务端口", port.toString())
    add("AnyBoot", json.optString("appVersion", ""))
    return rows
}

/**
 * 找一个能读 `/api/system` 的端点：先本地（局域网地址；走跳板机则是它解析出的
 * 隧道端口），不通再按「连接策略」的通道优先级逐个试远程槽。
 *
 * <p>**阻塞调用，必须在后台线程跑。** [lanEndpoint] 里可以做建隧道这类慢动作。
 */
internal fun loadSysInfoRows(profile: ServerProfile, lanEndpoint: () -> String): List<SysInfoRow> {
    var lastError = ""
    val local = try {
        lanEndpoint()
    } catch (error: Exception) {
        lastError = error.message ?: ""
        ""
    }
    if (local.isNotEmpty()) {
        try {
            return fetchSysInfoRows(local, profile.token)
        } catch (error: Exception) {
            lastError = error.message ?: ""
        }
    }
    // 远程兜底：逐条配对试，各用各的令牌（一台机器上 cloudflare 与 tsnet 是两个令牌）
    for (candidate in profile.routeCandidates()) {
        if (!candidate.isRemote) continue
        val origin = candidate.origin
        if (origin.isEmpty()) continue
        try {
            return fetchSysInfoRows(origin, candidate.token)
        } catch (error: Exception) {
            lastError = error.message ?: ""
        }
    }
    throw Exception(lastError.ifEmpty { "连不上这台电脑，读不到系统信息" })
}

/** 系统信息弹层要用的纯文本（「复制」按钮与分享用） */
fun sysInfoText(title: String, rows: List<SysInfoRow>): String = buildString {
    append(title)
    for (row in rows) {
        append('\n').append(row.label).append("：").append(row.value)
    }
}

/**
 * 系统详情弹层：标题 + 逐行「标签 → 值」+ 复制 / 关闭。
 *
 * <p>[loading] / [error] 用来表达「正在读 / 读失败」—— 别让用户对着空框发呆，
 * 这也是 PC 端系统信息弹层的同一套观感。
 */
@Composable
fun SystemInfoDialog(
    title: String,
    rows: List<SysInfoRow>,
    loading: Boolean,
    error: String,
    onDismiss: () -> Unit,
    onCopy: () -> Unit,
) {
    val palette = LocalPalette.current
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("系统信息") },
        text = {
            Column(
                Modifier
                    .fillMaxWidth()
                    .heightIn(max = 420.dp)
                    .verticalScroll(rememberScrollState()),
            ) {
                Text(
                    title,
                    fontSize = 13.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.fg,
                    modifier = Modifier.padding(bottom = 8.dp),
                )
                if (loading) {
                    Box(Modifier.fillMaxWidth().padding(vertical = 12.dp), Alignment.Center) {
                        CircularProgressIndicator(modifier = Modifier.size(20.dp), strokeWidth = 2.dp)
                    }
                } else if (error.isNotEmpty()) {
                    Text(error, fontSize = 12.sp, color = palette.bad)
                } else {
                    for (row in rows) {
                        Column(Modifier.padding(vertical = 4.dp)) {
                            Text(row.label, fontSize = 10.sp, color = palette.fgFaint, maxLines = 1)
                            Spacer(Modifier.height(1.dp))
                            Text(
                                row.value,
                                fontSize = 12.5.sp,
                                fontFamily = FontFamily.Monospace,
                                color = palette.fg,
                            )
                        }
                    }
                    if (rows.isEmpty()) {
                        Text("对方没有返回任何系统信息", fontSize = 12.sp, color = palette.fgFaint)
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("关闭") }
        },
        dismissButton = {
            if (!loading && error.isEmpty() && rows.isNotEmpty()) {
                TextButton(onClick = onCopy) { Text("复制") }
            }
        },
    )
}
