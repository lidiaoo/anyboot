package com.anyboot.remote

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import java.util.Locale
import org.json.JSONObject

/**
 * 电脑列表（移动端）：design/mobile-computer-list.html 的 Compose 实现。
 *
 * 数据全部是真的 —— 每张卡片对应一条 [ServerProfile]（一台装了 AnyBoot 的电脑）：
 *   · 在线状态   → 打电脑端的 `GET /health`（[HealthProbe.ping]，免鉴权）
 *   · 引导项     → `GET /api/status`（rEFInd 的 EFI 变量）与 `GET /api/grub`（grub.cfg 条目）
 *   · 切换引导项 → `GET /api/switch?system=` / `GET /api/grub-switch?entry=`
 *   · 电源       → 与主界面同一套分发（[WakeOnLan.sendRoutes] / [WakeOnLan.wake] /
 *                  [PowerOps.send]，按每条路由的 auto / http_only / http_first / hybrid 模式派发）
 *
 * 走跳板机的电脑会先把内网地址经 SSH 转成本地 `127.0.0.1:port` 再发 HTTP
 * （[SshSessionManager.tunnelAddress]）。所有网络调用都在后台线程，结果回主线程写 Compose 状态。
 */
class ComputerListActivity : ComponentActivity() {

    // ------------------------------------------------------------------
    // 状态
    // ------------------------------------------------------------------

    /** 跳板机首次连接的 TOFU 主机指纹确认 */
    private val bus = ConfirmBus()
    private val handler = Handler(Looper.getMainLooper())

    private val servers = mutableStateListOf<ServerProfile>()

    /** id → 引导项读取结果（缺席 = 还没探过，卡片显示「检测中」） */
    private val boots = mutableStateMapOf<String, BootInfo>()

    /** id → 电源操作的过渡状态（waking / shutting / rebooting / sleeping） */
    private val transients = mutableStateMapOf<String, String>()

    /** id → 正在进行的操作键（"power:wake" / "boot:grub" / "boot:load"），按钮据此禁用并显示转圈 */
    private val busy = mutableStateMapOf<String, String>()

    private val toasts = mutableStateListOf<CqToast>()

    private var refreshing by mutableStateOf(false)
    private var query by mutableStateOf("")
    private var filter by mutableStateOf(FILTER_ALL)
    private var sheetRequest by mutableStateOf<SheetRequest?>(null)

    // ------------------------------------------------------------------
    // 生命周期
    // ------------------------------------------------------------------

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Ui.applySystemBars(this)
        reloadServers()
        setContent { Root() }
    }

    override fun onResume() {
        super.onResume()
        // 从「服务器」页回来可能增删过电脑 —— 重新读一遍再整体探活
        reloadServers()
        refreshAll()
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        super.onDestroy()
    }

    private fun reloadServers() {
        val list = ServerStore.load(this)
        ServerStore.sort(list)
        servers.clear()
        servers.addAll(list)
    }

    // ------------------------------------------------------------------
    // 界面
    // ------------------------------------------------------------------

    @Composable
    private fun Root() {
        val palette = LocalPalette.current
        val tone = toneOf()
        val rows = rowsOf()

        CompositionLocalProvider(LocalTone provides tone) {
            Box(Modifier.fillMaxSize().background(palette.base)) {
                Column(Modifier.fillMaxSize()) {
                    TopBar(rows)
                    Box(Modifier.fillMaxWidth().weight(1f)) {
                        val visible = rows.filter { matches(it) }
                        if (visible.isEmpty()) {
                            EmptyState(rows.isEmpty())
                        } else {
                            HostList(visible)
                        }
                    }
                }
                ToastStack(Modifier.align(Alignment.BottomCenter))
            }

            sheetRequest?.let { request -> PickSheet(request) }
            ConfirmDialog(bus)
        }
    }

    // ---- 顶部栏 ------------------------------------------------------

    @Composable
    private fun TopBar(rows: List<CqRow>) {
        val palette = LocalPalette.current
        Column(Modifier.fillMaxWidth().background(palette.card)) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .height(54.dp)
                    .padding(start = 6.dp, end = 8.dp),
            ) {
                TopBarIcon("返回", { finish() }) {
                    ChevronLeftIcon(tint = palette.fg, size = 20.dp)
                }
                Spacer(Modifier.width(6.dp))
                Column(Modifier.weight(1f)) {
                    Text(
                        "电脑列表",
                        fontSize = 16.5.sp,
                        fontWeight = FontWeight.Bold,
                        color = palette.fg,
                        maxLines = 1,
                    )
                    Text("远程管理 - 电脑", fontSize = 11.sp, color = palette.fgFaint, maxLines = 1)
                }
                if (refreshing) {
                    Box(Modifier.size(38.dp), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(
                            modifier = Modifier.size(16.dp),
                            strokeWidth = 2.dp,
                            color = palette.accent,
                        )
                    }
                } else {
                    TopBarIcon("刷新", { refreshAll() }) {
                        RefreshIcon(tint = palette.fgDim, size = 19.dp)
                    }
                }
                TopBarIcon("操作说明", { sheetRequest = SheetRequest("", KIND_HELP) }) {
                    HelpIcon(tint = palette.fgDim, size = 19.dp)
                }
                Spacer(Modifier.width(2.dp))
            }

            Column(Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, bottom = 10.dp)) {
                SearchBox()
                Row(
                    horizontalArrangement = Arrangement.spacedBy(7.dp),
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 10.dp)
                        .horizontalScroll(rememberScrollState()),
                ) {
                    val online = rows.count { it.state == STATE_ONLINE }
                    val pending = rows.count { grubPending(it.grub) }
                    FilterChip("全部", rows.size, filter == FILTER_ALL) { filter = FILTER_ALL }
                    FilterChip("在线", online, filter == FILTER_ONLINE) { filter = FILTER_ONLINE }
                    FilterChip("离线", rows.size - online, filter == FILTER_OFFLINE) {
                        filter = FILTER_OFFLINE
                    }
                    FilterChip("待生效", pending, filter == FILTER_PENDING) {
                        filter = FILTER_PENDING
                    }
                }
            }
            Box(Modifier.fillMaxWidth().height(1.dp).background(palette.line))
        }
    }

    /** 设计稿的 38×38 方角按钮：透明底、按下显浅灰，图标居中 */
    @Composable
    private fun TopBarIcon(
        description: String,
        onClick: () -> Unit,
        content: @Composable () -> Unit,
    ) {
        Box(
            Modifier
                .size(38.dp)
                .clip(RoundedCornerShape(10.dp))
                .clickable(onClickLabel = description, onClick = onClick),
            contentAlignment = Alignment.Center,
        ) {
            content()
        }
    }

    @Composable
    private fun SearchBox() {
        val palette = LocalPalette.current
        BasicTextField(
            value = query,
            onValueChange = { query = it },
            singleLine = true,
            textStyle = TextStyle(fontSize = 13.sp, color = palette.fg),
            cursorBrush = SolidColor(palette.accent),
            modifier = Modifier.fillMaxWidth(),
            decorationBox = { inner ->
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(38.dp)
                        .clip(RoundedCornerShape(11.dp))
                        .background(palette.elevate)
                        .border(1.dp, palette.line, RoundedCornerShape(11.dp))
                        .padding(start = 11.dp, end = 8.dp),
                ) {
                    SearchIcon(tint = palette.fgFaint, size = 15.dp)
                    Spacer(Modifier.width(8.dp))
                    Box(Modifier.weight(1f)) {
                        if (query.isEmpty()) {
                            Text(
                                "搜索电脑名称、IP 或 MAC",
                                fontSize = 13.sp,
                                color = palette.fgFaint,
                                maxLines = 1,
                            )
                        }
                        inner()
                    }
                    if (query.isNotEmpty()) {
                        Box(
                            Modifier
                                .size(22.dp)
                                .clip(CircleShape)
                                .clickable { query = "" },
                            contentAlignment = Alignment.Center,
                        ) {
                            CloseIcon(tint = palette.fgFaint, size = 13.dp)
                        }
                    }
                }
            },
        )
    }

    @Composable
    private fun FilterChip(label: String, count: Int, selected: Boolean, onClick: () -> Unit) {
        val palette = LocalPalette.current
        val shape = RoundedCornerShape(50)
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .height(30.dp)
                .clip(shape)
                .background(if (selected) palette.accent else palette.card)
                .border(1.dp, if (selected) palette.accent else palette.line, shape)
                .clickable(onClick = onClick)
                .padding(horizontal = 12.dp),
        ) {
            Text(
                label,
                fontSize = 12.5.sp,
                fontWeight = FontWeight.SemiBold,
                color = if (selected) palette.onAccent else palette.fgDim,
                maxLines = 1,
            )
            Spacer(Modifier.width(6.dp))
            Text(
                count.toString(),
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
                color = if (selected) palette.onAccent.copy(alpha = 0.82f) else palette.fgFaint,
            )
        }
    }

    // ---- 列表 -------------------------------------------------------

    @Composable
    private fun HostList(rows: List<CqRow>) {
        val palette = LocalPalette.current
        LazyColumn(
            verticalArrangement = Arrangement.spacedBy(10.dp),
            contentPadding = PaddingValues(start = 12.dp, end = 12.dp, top = 12.dp, bottom = 28.dp),
            modifier = Modifier.fillMaxSize(),
        ) {
            items(rows, key = { it.profile.id }) { row -> HostCard(row) }
            item {
                Text(
                    "电源指令通过魔术包发送，无需目标机在线\n引导项变更在下次重启时生效",
                    fontSize = 11.sp,
                    color = palette.fgFaint,
                    textAlign = TextAlign.Center,
                    lineHeight = 18.sp,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(start = 16.dp, end = 16.dp, top = 4.dp),
                )
            }
        }
    }

    @Composable
    private fun EmptyState(noServers: Boolean) {
        val palette = LocalPalette.current
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
            modifier = Modifier.fillMaxSize().padding(horizontal = 32.dp),
        ) {
            DesktopIcon(tint = palette.fgFaint.copy(alpha = 0.5f), size = 26.dp)
            Spacer(Modifier.height(8.dp))
            Text(
                if (noServers) {
                    "还没有添加任何电脑\n到「服务器」里添加一台，这里就会出现它的电源与引导项"
                } else {
                    "没有符合条件的电脑"
                },
                fontSize = 13.sp,
                color = palette.fgDim,
                textAlign = TextAlign.Center,
                lineHeight = 20.sp,
            )
        }
    }

    @Composable
    private fun HostCard(row: CqRow) {
        val palette = LocalPalette.current
        val tone = LocalTone.current
        val shape = RoundedCornerShape(15.dp)
        val stripe = stateColor(row.state)

        Column(
            modifier = Modifier
                .fillMaxWidth()
                .clip(shape)
                .background(palette.card)
                .drawBehind {
                    val inset = 14.dp.toPx()
                    val bottom = size.height - inset
                    if (bottom > inset) {
                        drawRoundRect(
                            color = stripe,
                            topLeft = Offset(0f, inset),
                            size = Size(3.dp.toPx(), bottom - inset),
                            cornerRadius = CornerRadius(0f, 3.dp.toPx()),
                        )
                    }
                }
                .border(1.dp, palette.line, shape)
                .padding(start = 13.dp, end = 13.dp, top = 12.dp, bottom = 13.dp),
        ) {
            // 名称 + 分组 chip + ⋮
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.padding(start = 4.dp),
            ) {
                Text(
                    row.name,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.fg,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                if (row.group.isNotEmpty()) {
                    Spacer(Modifier.width(7.dp))
                    val groupTint = Color(row.profile.groupColor())
                    Text(
                        row.group,
                        fontSize = 10.sp,
                        fontWeight = FontWeight.SemiBold,
                        color = groupTint,
                        maxLines = 1,
                        modifier = Modifier
                            .clip(RoundedCornerShape(5.dp))
                            .background(groupTint.copy(alpha = 0.16f))
                            .border(1.dp, groupTint.copy(alpha = 0.3f), RoundedCornerShape(5.dp))
                            .padding(horizontal = 5.dp, vertical = 1.dp),
                    )
                }
                Spacer(Modifier.weight(1f))
                CardMenu(row)
            }

            // 状态 · IP · MAC
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.padding(start = 4.dp, top = 5.dp),
            ) {
                StatusLine(row.state)
                Spacer(Modifier.width(6.dp))
                Text("·", fontSize = 11.sp, color = palette.fgFaint)
                Spacer(Modifier.width(6.dp))
                Text(
                    row.ip,
                    fontSize = 11.sp,
                    fontFamily = FontFamily.Monospace,
                    color = palette.fgFaint,
                    maxLines = 1,
                )
                if (row.mac.isNotEmpty()) {
                    Spacer(Modifier.width(6.dp))
                    Text("·", fontSize = 11.sp, color = palette.fgFaint)
                    Spacer(Modifier.width(6.dp))
                    Text(
                        row.mac,
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                        color = palette.fgFaint,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }

            // 两个引导项
            Column(
                verticalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.padding(top = 11.dp),
            ) {
                BootButton(row, KIND_REFIND, row.refind)
                BootButton(row, KIND_GRUB, row.grub)
            }

            // 待生效提示条
            if (grubPending(row.grub)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 8.dp)
                        .clip(RoundedCornerShape(9.dp))
                        .background(tone.pendBg)
                        .border(1.dp, tone.pendBorder, RoundedCornerShape(9.dp))
                        .padding(horizontal = 10.dp, vertical = 7.dp),
                ) {
                    InfoIcon(tint = tone.pendText, size = 12.dp)
                    Spacer(Modifier.width(6.dp))
                    Text("引导项已修改，重启后生效", fontSize = 11.sp, color = tone.pendText)
                    Spacer(Modifier.weight(1f))
                    Text(
                        "撤销",
                        fontSize = 11.sp,
                        fontWeight = FontWeight.Bold,
                        color = palette.accent,
                        modifier = Modifier
                            .clickable { switchBoot(row.profile, KIND_GRUB, row.grub.saved, "撤销") }
                            .padding(horizontal = 2.dp, vertical = 2.dp),
                    )
                }
            }

            // 电源四键（上方的虚线分隔）
            Spacer(
                Modifier
                    .fillMaxWidth()
                    .padding(top = 11.dp)
                    .height(1.dp)
                    .drawBehind {
                        drawLine(
                            color = palette.line,
                            start = Offset(0f, 0f),
                            end = Offset(size.width, 0f),
                            strokeWidth = size.height,
                            pathEffect = PathEffect.dashPathEffect(
                                floatArrayOf(4.dp.toPx(), 3.dp.toPx()),
                            ),
                        )
                    },
            )
            Row(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.fillMaxWidth().padding(top = 11.dp),
            ) {
                PowerButton(row, POWER_WAKE, "开机", Modifier.weight(1f))
                PowerButton(row, POWER_SLEEP, "睡眠", Modifier.weight(1f))
                PowerButton(row, POWER_REBOOT, "重启", Modifier.weight(1f))
                PowerButton(row, POWER_SHUTDOWN, "关机", Modifier.weight(1f))
            }
        }
    }

    @Composable
    private fun StatusLine(state: String) {
        val palette = LocalPalette.current
        val tone = LocalTone.current
        val color: Color
        val label: String
        when (state) {
            STATE_ONLINE -> { color = tone.online; label = "在线" }
            STATE_OFFLINE -> { color = tone.offlineText; label = "离线" }
            STATE_WAKING -> { color = tone.busy; label = "唤醒中" }
            STATE_SHUTTING -> { color = tone.busy; label = "关机中" }
            STATE_REBOOTING -> { color = tone.busy; label = "重启中" }
            STATE_SLEEPING -> { color = tone.sleep; label = "已睡眠" }
            else -> { color = palette.fgFaint; label = "检测中…" }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(5.dp).clip(CircleShape).background(color))
            Spacer(Modifier.width(4.dp))
            Text(label, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, color = color, maxLines = 1)
        }
    }

    @Composable
    private fun CardMenu(row: CqRow) {
        val palette = LocalPalette.current
        var open by remember { mutableStateOf(false) }
        Box {
            Box(
                Modifier
                    .size(30.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .clickable { open = true },
                contentAlignment = Alignment.Center,
            ) {
                MoreIcon(tint = palette.fgFaint, size = 17.dp)
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                DropdownMenuItem(
                    text = { Text("重新读取引导项", fontSize = 13.sp) },
                    onClick = { open = false; reloadBoot(row.profile) },
                )
                DropdownMenuItem(
                    text = { Text("复制地址", fontSize = 13.sp) },
                    onClick = {
                        open = false
                        copyText(ServerProfile.normalizeHost(row.profile.host()))
                        toast("已复制 ${row.ip}")
                    },
                )
                DropdownMenuItem(
                    text = { Text("管理这台电脑", fontSize = 13.sp) },
                    onClick = {
                        open = false
                        startActivity(
                            Intent(this@ComputerListActivity, ServersActivity::class.java)
                        )
                    },
                )
            }
        }
    }

    // ---- 引导项按钮 --------------------------------------------------

    @Composable
    private fun BootButton(row: CqRow, kind: String, group: BootGroup) {
        val palette = LocalPalette.current
        val tone = LocalTone.current
        val title = if (kind == KIND_REFIND) "rEFInd" else "GRUB"
        val work = busy[row.profile.id]
        val working = work != null && work.startsWith("boot:")
        val pending = kind == KIND_GRUB && grubPending(group)
        val usable = group.available && row.state == STATE_ONLINE && work == null

        val valueLabel: String
        val dot: String
        if (!group.available) {
            valueLabel = when {
                row.state == STATE_UNKNOWN -> "读取中…"
                row.state == STATE_OFFLINE -> "离线不可改"
                else -> "未检测到"
            }
            dot = ""
        } else {
            val hit = group.options.firstOrNull { it.value == group.current }
            valueLabel = hit?.label ?: group.current.ifEmpty { "未设置" }
            dot = hit?.kind ?: "linux"
        }

        val shape = RoundedCornerShape(11.dp)
        val background = when {
            !usable -> tone.softDisabled
            pending -> tone.pendBg
            else -> tone.soft
        }
        val borderColor = when {
            !usable -> tone.dashed
            pending -> tone.pendBorder
            else -> palette.line
        }

        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .height(42.dp)
                .clip(shape)
                .background(background)
                .then(
                    if (usable) Modifier.border(1.dp, borderColor, shape)
                    else Modifier.dashedBorder(borderColor, 11.dp, 1.dp)
                )
                .clickable(enabled = usable) {
                    when {
                        group.options.isEmpty() -> toast("该设备没有可选的 $title 启动项")
                        row.state != STATE_ONLINE -> toast("设备不在线，无法修改引导项")
                        else -> sheetRequest = SheetRequest(row.profile.id, kind)
                    }
                }
                .padding(start = 11.dp, end = 10.dp),
        ) {
            Text(
                title,
                fontSize = 10.5.sp,
                fontWeight = FontWeight.SemiBold,
                fontFamily = FontFamily.Monospace,
                color = if (usable) palette.fgDim else palette.fgFaint.copy(alpha = 0.75f),
                maxLines = 1,
                modifier = Modifier.width(46.dp),
            )
            if (working) {
                CircularProgressIndicator(
                    modifier = Modifier.size(13.dp),
                    strokeWidth = 2.dp,
                    color = palette.accent,
                )
                Spacer(Modifier.width(7.dp))
            } else if (dot.isNotEmpty()) {
                BootDot(dot)
                Spacer(Modifier.width(7.dp))
            }
            Text(
                valueLabel,
                fontSize = 12.5.sp,
                fontWeight = if (usable) FontWeight.Medium else FontWeight.Normal,
                color = if (usable) palette.fg else palette.fgFaint.copy(alpha = 0.85f),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            if (pending) {
                Spacer(Modifier.width(6.dp))
                Text(
                    "待重启",
                    fontSize = 9.5.sp,
                    fontWeight = FontWeight.Bold,
                    color = tone.pendText,
                    maxLines = 1,
                    modifier = Modifier
                        .clip(RoundedCornerShape(4.dp))
                        .background(tone.pendBorder.copy(alpha = 0.32f))
                        .padding(horizontal = 5.dp, vertical = 1.dp),
                )
            }
            if (usable) {
                Spacer(Modifier.width(7.dp))
                ChevronDownIcon(tint = palette.fgFaint, size = 14.dp)
            }
        }
    }

    /** 启动项前面的分类小圆点：win 蓝 / mac 灰 / linux 橙 / auto 空心环 */
    @Composable
    private fun BootDot(kind: String) {
        val color = when (kind) {
            "win" -> Color(0xFF2563EB)
            "mac" -> Color(0xFF64748B)
            "linux" -> Color(0xFFD97706)
            "auto" -> Color(0xFF6366F1)
            else -> Color(0xFF94A3B8)
        }
        if (kind == "auto") {
            Box(Modifier.size(7.dp).clip(CircleShape).border(1.6.dp, color, CircleShape))
        } else {
            Box(Modifier.size(7.dp).clip(CircleShape).background(color))
        }
    }

    // ---- 电源按钮 ----------------------------------------------------

    @Composable
    private fun PowerButton(row: CqRow, action: String, label: String, modifier: Modifier) {
        val palette = LocalPalette.current
        val tone = LocalTone.current
        val working = busy[row.profile.id] == "power:$action"
        val anyBusy = busy[row.profile.id] != null
        val enabled = !anyBusy && powerEnabled(row.state, action)
        // 离线卡片上「开机」是唯一可用项 —— 给成实心强调色，一眼能看到
        val solid = enabled && row.state != STATE_ONLINE && action == POWER_WAKE

        val accent = when (action) {
            POWER_WAKE -> tone.online
            POWER_SLEEP -> palette.accent
            POWER_REBOOT -> Color(0xFF2563EB)
            else -> palette.bad
        }
        val shape = RoundedCornerShape(11.dp)
        val background = when {
            solid -> tone.online
            enabled -> palette.card
            else -> tone.softDisabled
        }
        val contentColor = when {
            solid -> Color.White
            enabled -> palette.fg
            else -> palette.fgFaint.copy(alpha = 0.42f)
        }

        Box(
            contentAlignment = Alignment.Center,
            modifier = modifier
                .height(44.dp)
                .clip(shape)
                .background(background)
                .then(
                    if (enabled && !solid) Modifier.border(1.dp, palette.line, shape) else Modifier
                )
                .clickable(enabled = enabled) { performPower(row.profile, action) },
        ) {
            if (working) {
                CircularProgressIndicator(
                    modifier = Modifier.size(14.dp),
                    strokeWidth = 2.dp,
                    color = accent,
                )
            } else {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    when (action) {
                        POWER_WAKE -> PowerIcon(tint = contentColor, size = 15.dp)
                        POWER_SLEEP -> MoonIcon(tint = contentColor, size = 15.dp)
                        POWER_REBOOT -> RestartIcon(tint = contentColor, size = 15.dp)
                        else -> PowerOffIcon(tint = contentColor, size = 15.dp)
                    }
                    Spacer(Modifier.width(5.dp))
                    Text(
                        label,
                        fontSize = 11.sp,
                        fontWeight = FontWeight.SemiBold,
                        color = contentColor,
                        maxLines = 1,
                    )
                }
            }
        }
    }

    // ---- toast -------------------------------------------------------

    @Composable
    private fun ToastStack(modifier: Modifier = Modifier) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(6.dp),
            modifier = modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 24.dp),
        ) {
            toasts.forEach { item ->
                AnimatedVisibility(visible = item.shown, enter = fadeIn(), exit = fadeOut()) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier
                            .clip(RoundedCornerShape(11.dp))
                            .background(Color(0xFF1E1B4B))
                            .padding(horizontal = 14.dp, vertical = 9.dp),
                    ) {
                        CheckIcon(tint = Color(0xFF6EE7B7), size = 14.dp)
                        Spacer(Modifier.width(7.dp))
                        Text(item.text, fontSize = 12.sp, color = Color.White, maxLines = 3)
                    }
                }
            }
        }
    }

    // ---- 底部选择器 --------------------------------------------------

    @OptIn(ExperimentalMaterial3Api::class)
    @Composable
    private fun PickSheet(request: SheetRequest) {
        val palette = LocalPalette.current
        val help = request.kind == KIND_HELP
        val profile = servers.firstOrNull { it.id == request.rid }
        if (!help && profile == null) return
        val info = profile?.let { boots[it.id] }
        val group = when {
            help -> null
            request.kind == KIND_REFIND -> info?.refind
            else -> info?.grub
        }

        ModalBottomSheet(
            onDismissRequest = { sheetRequest = null },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = palette.card,
            dragHandle = {
                Box(
                    Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 8.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Box(
                        Modifier
                            .width(36.dp)
                            .height(4.dp)
                            .clip(RoundedCornerShape(2.dp))
                            .background(palette.lineStrong),
                    )
                }
            },
        ) {
            Column(
                Modifier
                    .fillMaxWidth()
                    .navigationBarsPadding()
                    .padding(bottom = 12.dp),
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(start = 18.dp, end = 8.dp, bottom = 10.dp),
                ) {
                    Text(
                        if (help) "操作说明" else if (request.kind == KIND_REFIND) "rEFInd 启动项" else "GRUB 启动项",
                        fontSize = 14.5.sp,
                        fontWeight = FontWeight.Bold,
                        color = palette.fg,
                        maxLines = 1,
                    )
                    Spacer(Modifier.weight(1f))
                    if (!help && profile != null) {
                        Text(
                            profile.displayName(),
                            fontSize = 11.sp,
                            color = palette.fgFaint,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f, fill = false),
                        )
                        Spacer(Modifier.width(8.dp))
                    }
                    Box(
                        Modifier
                            .size(30.dp)
                            .clip(CircleShape)
                            .background(palette.elevate)
                            .clickable { sheetRequest = null },
                        contentAlignment = Alignment.Center,
                    ) {
                        CloseIcon(tint = palette.fgDim, size = 14.dp)
                    }
                }

                Column(
                    Modifier
                        .fillMaxWidth()
                        .verticalScroll(rememberScrollState())
                        .padding(horizontal = 18.dp),
                ) {
                    if (help) {
                        HELP_LINES.forEach { line ->
                            Row(Modifier.padding(vertical = 4.dp)) {
                                Text("·", fontSize = 12.5.sp, color = palette.fgDim)
                                Spacer(Modifier.width(6.dp))
                                Text(line, fontSize = 12.5.sp, color = palette.fgDim, lineHeight = 22.sp)
                            }
                        }
                        Spacer(Modifier.height(8.dp))
                        Text("电源操作不会弹出确认框", fontSize = 11.sp, color = palette.fgFaint)
                    } else if (group != null && group.options.isNotEmpty() && profile != null) {
                        val rid = profile
                        group.options.forEach { option ->
                            val selected = option.value == group.current
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(bottom = 4.dp)
                                    .clip(RoundedCornerShape(11.dp))
                                    .background(
                                        if (selected) palette.accent.copy(alpha = 0.14f)
                                        else Color.Transparent
                                    )
                                    .clickable {
                                        sheetRequest = null
                                        switchBoot(rid, request.kind, option.value, "切换")
                                    }
                                    .padding(horizontal = 11.dp, vertical = 12.dp),
                            ) {
                                BootDot(option.kind)
                                Spacer(Modifier.width(8.dp))
                                Text(
                                    option.label,
                                    fontSize = 14.sp,
                                    color = if (selected) palette.accent else palette.fg,
                                    maxLines = 2,
                                    overflow = TextOverflow.Ellipsis,
                                    modifier = Modifier.weight(1f),
                                )
                                if (selected) {
                                    Spacer(Modifier.width(8.dp))
                                    CheckIcon(tint = palette.accent, size = 16.dp)
                                }
                            }
                        }
                        Spacer(Modifier.height(8.dp))
                        Text("选择后将在下次重启时生效", fontSize = 11.sp, color = palette.fgFaint)
                    } else {
                        Text(
                            group?.note?.ifEmpty { "没有可选的启动项" } ?: "没有可选的启动项",
                            fontSize = 12.5.sp,
                            color = palette.fgDim,
                            lineHeight = 22.sp,
                        )
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // 过滤
    // ------------------------------------------------------------------

    private fun matches(row: CqRow): Boolean {
        val byState = when (filter) {
            FILTER_ONLINE -> row.state == STATE_ONLINE
            FILTER_OFFLINE -> row.state != STATE_ONLINE
            FILTER_PENDING -> grubPending(row.grub)
            else -> true
        }
        if (!byState) return false
        val needle = query.trim().lowercase(Locale.US)
        if (needle.isEmpty()) return true
        val haystack = (row.name + " " + row.group + " " + row.ip + " " + row.mac)
            .lowercase(Locale.US)
        return haystack.contains(needle)
    }

    // ------------------------------------------------------------------
    // 行模型
    // ------------------------------------------------------------------

    private fun rowsOf(): List<CqRow> = servers.map { profile ->
        val info = boots[profile.id]
        val state = transients[profile.id] ?: when {
            info == null -> STATE_UNKNOWN
            !info.online -> STATE_OFFLINE
            else -> STATE_ONLINE
        }
        CqRow(
            profile = profile,
            name = profile.displayName().ifEmpty { "(未命名)" },
            group = ServerProfile.text(profile.group),
            ip = displayAddress(profile),
            mac = profile.allMacs().firstOrNull()?.uppercase(Locale.US) ?: "",
            refind = info?.refind ?: emptyGroup(KIND_REFIND),
            grub = info?.grub ?: emptyGroup(KIND_GRUB),
            state = state,
        )
    }

    private fun displayAddress(profile: ServerProfile): String {
        val value = ServerProfile.normalizeHost(profile.host())
        if (value.isEmpty()) return "(未填地址)"
        val suffix = ":" + ServerProfile.DEFAULT_PORT
        return if (value.endsWith(suffix)) value.substring(0, value.length - suffix.length) else value
    }

    private fun emptyGroup(kind: String) = BootGroup(
        kind = kind,
        title = if (kind == KIND_REFIND) "rEFInd" else "GRUB",
        available = false,
        current = "",
        next = "",
        saved = "",
        note = "",
        options = emptyList(),
    )

    /** 「待生效」的唯一判据：GRUB 的 nextEntry 与 savedEntry 都有值且不同（rEFInd 没有这两个值） */
    private fun grubPending(group: BootGroup): Boolean =
        group.available && group.next.isNotEmpty() && group.saved.isNotEmpty() && group.next != group.saved

    /**
     * 电源键可用性：与设计稿一致 —— 过渡态（唤醒中/关机中/重启中）全禁；
     * 在线时「开机」无意义（已开着）；离线/已睡眠时只有「开机」可用。
     */
    private fun powerEnabled(state: String, action: String): Boolean = when (state) {
        STATE_ONLINE -> action != POWER_WAKE
        STATE_OFFLINE, STATE_SLEEPING -> action == POWER_WAKE
        else -> false
    }

    // ------------------------------------------------------------------
    // 网络：探测 + 读引导项
    // ------------------------------------------------------------------

    private fun refreshAll() {
        if (refreshing) return
        val snapshot = servers.toList()
        if (snapshot.isEmpty()) return
        refreshing = true
        toast("正在探测全部电脑…")
        Thread({
            for (profile in snapshot) {
                val info = fetchBootInfo(profile)
                runOnUiThread { boots[profile.id] = info }
            }
            runOnUiThread {
                // 主动刷新 = 要真实状态，之前动作留下的过渡态一律作废
                transients.clear()
                refreshing = false
                toast("探测完成 · 已重载")
            }
        }, "anyboot-refresh").start()
    }

    private fun reloadBoot(profile: ServerProfile) {
        val id = profile.id
        if (busy[id] != null) return
        busy[id] = "boot:load"
        Thread({
            val info = fetchBootInfo(profile)
            runOnUiThread {
                boots[id] = info
                busy.remove(id)
            }
        }, "anyboot-boot").start()
    }

    /** 后台线程：探活 + 读两套引导项（一边失败不影响另一边） */
    private fun fetchBootInfo(profile: ServerProfile): BootInfo {
        val endpoint = try {
            endpointOf(profile)
        } catch (error: Exception) {
            return offlineInfo("", error.message ?: "地址不可用")
        }
        if (endpoint.isEmpty()) return offlineInfo("", "这台电脑还没有可用地址")
        if (HealthProbe.ping(endpoint, PROBE_TIMEOUT_MS) <= 0) return offlineInfo(endpoint, "")

        val errors = mutableListOf<String>()
        val refind = try {
            parseRefind(httpJson(endpoint, profile.token, "/api/status"))
        } catch (error: Exception) {
            val message = error.message ?: "读取失败"
            errors.add(message)
            BootGroup(KIND_REFIND, "rEFInd", false, "", "", "", message, emptyList())
        }
        val grub = try {
            parseGrub(httpJson(endpoint, profile.token, "/api/grub"))
        } catch (error: Exception) {
            val message = error.message ?: "读取失败"
            errors.add(message)
            BootGroup(KIND_GRUB, "GRUB", false, "", "", "", message, emptyList())
        }
        return BootInfo(
            ok = errors.size < 2,
            online = true,
            addr = endpoint,
            error = errors.joinToString("；"),
            refind = refind,
            grub = grub,
        )
    }

    private fun offlineInfo(endpoint: String, error: String): BootInfo {
        val note = "电脑离线，无法读取引导项"
        return BootInfo(
            ok = false,
            online = false,
            addr = endpoint,
            error = error,
            refind = BootGroup(KIND_REFIND, "rEFInd", false, "", "", "", note, emptyList()),
            grub = BootGroup(KIND_GRUB, "GRUB", false, "", "", "", note, emptyList()),
        )
    }

    /** 这台电脑的 HTTP 端点：直连取首个地址；走跳板机先拉一条本地隧道 */
    private fun endpointOf(profile: ServerProfile): String {
        val host = ServerProfile.normalizeHost(profile.host())
        if (host.isEmpty()) throw Exception("这台电脑还没有可用地址")
        if (ServerProfile.text(profile.jumpServerId).isEmpty()) return host
        val jump = JumpStore.findJump(JumpStore.loadJumps(this), profile.jumpServerId)
            ?: throw Exception("跳板机不存在（可能已删除）")
        return SshSessionManager.shared().tunnelAddress(this, jump, host, prompt())
    }

    private fun prompt(): SshSessionManager.Prompt = object : SshSessionManager.Prompt {
        override fun confirm(title: String, message: String): Boolean = bus.confirm(title, message)
    }

    /** 对电脑发一条 `GET {path}?token=…`，返回解析后的 JSON（语义对齐 PC 端 remote_http_json） */
    private fun httpJson(endpoint: String, token: String, path: String): JSONObject {
        val trimmed = ServerProfile.text(token)
        val query = if (trimmed.isEmpty()) "" else {
            (if (path.contains('?')) "&" else "?") + "token=" +
                URLEncoder.encode(trimmed, "UTF-8")
        }
        val connection = URL("http://" + endpoint + path + query)
            .openConnection() as HttpURLConnection
        connection.requestMethod = "GET"
        connection.connectTimeout = CONNECT_TIMEOUT_MS
        connection.readTimeout = READ_TIMEOUT_MS
        connection.instanceFollowRedirects = false
        try {
            val status = connection.responseCode
            if (status == 401) {
                throw Exception("这台电脑不认这个 token（401）—— 到「服务器」里核对访问令牌")
            }
            val stream = if (status >= 400) connection.errorStream else connection.inputStream
            val body = stream?.use { it.readBytes().toString(Charsets.UTF_8) } ?: ""
            val json = try {
                JSONObject(body.trim())
            } catch (error: Exception) {
                throw Exception("这台电脑返回的不是合法 JSON（$status）：${error.message ?: ""}")
            }
            if (status >= 400) {
                throw Exception(json.optString("message", "").ifEmpty { "请求被拒绝" })
            }
            return json
        } finally {
            connection.disconnect()
        }
    }

    /** `/api/status` 的 variables → rEFInd 可选组（对齐 PC 端 refind_from_status） */
    private fun parseRefind(json: JSONObject): BootGroup {
        val options = mutableListOf<BootOption>()
        var current = ""
        val variables = json.optJSONArray("variables")
        if (variables != null) {
            for (index in 0 until variables.length()) {
                val item = variables.optJSONObject(index) ?: continue
                if (!item.optBoolean("exists", false)) continue
                val system = item.optString("system", "").trim()
                if (system.isEmpty()) continue
                val label = item.optString("displayName", "").trim().ifEmpty { system }
                if (item.optBoolean("active", false)) current = system
                options.add(BootOption(system, label, refindClass(system)))
            }
        }
        return BootGroup(
            kind = KIND_REFIND,
            title = "rEFInd",
            available = options.isNotEmpty(),
            current = current,
            next = current,
            saved = current,
            note = if (options.isEmpty()) "这台电脑没部署 rEFInd（或还没设置过下次启动项）" else "",
            options = options,
        )
    }

    /** `/api/grub` 的 entries → GRUB 可选组（对齐 PC 端 grub_from_json） */
    private fun parseGrub(json: JSONObject): BootGroup {
        val next = json.optString("nextEntry", "").trim()
        val saved = json.optString("savedEntry", "").trim()
        val installed = json.optBoolean("installed", false)
        val options = mutableListOf<BootOption>()
        val entries = json.optJSONArray("entries")
        if (entries != null) {
            for (index in 0 until entries.length()) {
                val item = entries.optJSONObject(index) ?: continue
                val selector = item.optString("selector", "").trim()
                if (selector.isEmpty()) continue
                val name = item.optString("name", "").trim()
                val submenu = item.optString("submenu", "").trim()
                val klass = item.optString("class", "")
                val chainloader = item.optBoolean("chainloader", false)
                val label = if (submenu.isEmpty() || name.startsWith(submenu)) {
                    name
                } else {
                    "$submenu · $name"
                }
                options.add(
                    BootOption(selector, label.ifEmpty { selector }, grubClass(name, klass, chainloader))
                )
            }
        }
        val note = if (options.isEmpty()) {
            json.optString("error", "").trim().ifEmpty { "这台电脑没部署 GRUB（或还没同步 grub.cfg）" }
        } else {
            ""
        }
        return BootGroup(
            kind = KIND_GRUB,
            title = "GRUB",
            available = installed || options.isNotEmpty(),
            current = next.ifEmpty { saved },
            next = next,
            saved = saved,
            note = note,
            options = options,
        )
    }

    private fun refindClass(system: String): String = when (system) {
        "windows" -> "win"
        "macos" -> "mac"
        else -> "linux"
    }

    private fun grubClass(name: String, klass: String, chainloader: Boolean): String {
        val haystack = "$name $klass".lowercase(Locale.US)
        if (haystack.contains("windows") || haystack.contains("win")) return "win"
        if (haystack.contains("osx") || haystack.contains("mac") || haystack.contains("apple")) {
            return "mac"
        }
        if (chainloader && haystack.contains("efi")) return "auto"
        return "linux"
    }

    // ------------------------------------------------------------------
    // 动作：切换引导项 / 电源
    // ------------------------------------------------------------------

    private fun switchBoot(profile: ServerProfile, kind: String, value: String, verb: String) {
        val id = profile.id
        if (busy[id] != null) return
        if (value.isEmpty()) {
            toast("没有可切换的目标启动项")
            return
        }
        busy[id] = "boot:$kind"
        Thread({
            val outcome = runCatching {
                val endpoint = endpointOf(profile)
                val path = if (kind == KIND_REFIND) {
                    "/api/switch?system=" + URLEncoder.encode(value, "UTF-8")
                } else {
                    "/api/grub-switch?entry=" + URLEncoder.encode(value, "UTF-8")
                }
                val json = httpJson(endpoint, profile.token, path)
                json.optString("message", "").ifEmpty {
                    if (json.optBoolean("ok", true)) "已设置下次启动项"
                    else json.optString("error", "请求被拒绝")
                }
            }
            runOnUiThread {
                busy.remove(id)
                outcome.onSuccess { message ->
                    toast(message)
                    reloadBoot(profile)
                }.onFailure { error ->
                    toast("$verb 失败：${error.message ?: "未知错误"}")
                }
            }
        }, "anyboot-boot-switch").start()
    }

    private fun performPower(profile: ServerProfile, action: String) {
        val id = profile.id
        if (busy[id] != null) return
        busy[id] = "power:$action"
        Thread({
            val outcome = runCatching { powerMessage(profile, action) }
            runOnUiThread {
                busy.remove(id)
                outcome.onSuccess { message ->
                    toast(message)
                    transients[id] = when (action) {
                        POWER_WAKE -> STATE_WAKING
                        POWER_SLEEP -> STATE_SLEEPING
                        POWER_SHUTDOWN -> STATE_SHUTTING
                        else -> STATE_REBOOTING
                    }
                    // 过渡态只在动作进行时存在：到点后必须让位给真实探测结果，
                    // 否则「开机」之后这张卡会永远停在「在线」，探测结果再也盖不住。
                    handler.postDelayed({
                        when (action) {
                            POWER_WAKE, POWER_REBOOT -> {
                                transients.remove(id)
                                reloadBoot(profile)
                            }
                            // 睡眠是持续状态（睡着的机器 ping 不通），留着它才能显示「已睡眠」
                            POWER_SLEEP -> transients[id] = STATE_SLEEPING
                            // 关机后确实离线：清掉过渡态，直接标离线，按钮回到「只有开机可用」
                            else -> {
                                transients.remove(id)
                                boots[id] = offlineInfo(boots[id]?.addr ?: "", "")
                            }
                        }
                    }, if (action == POWER_WAKE) 1700L else 1400L)
                }.onFailure { error ->
                    toast(
                        "${ServerProfile.powerActionLabel(action)}没发出去：" +
                            (error.message ?: "未知错误")
                    )
                }
            }
        }, "anyboot-power").start()
    }

    /**
     * 与主界面完全一致的电源分发（四条路由模式）。
     * 返回给人看的一句话结果；失败抛异常。**必须在后台线程调用。**
     */
    private fun powerMessage(profile: ServerProfile, action: String): String {
        val routes = profile.routesFor(action)
        val mode = ServerProfile.routesMode(routes)
        val prompt = prompt()

        fun packets(): String {
            val routed = WakeOnLan.sendRoutes(this, profile, action, prompt)
            return when {
                routed != null -> routed
                ServerProfile.POWER_WAKE == action -> WakeOnLan.wake(this, profile, prompt)
                else -> PowerOps.send(powerAddress(profile), profile.token, action)
            }
        }

        fun http(): String = PowerOps.send(powerAddress(profile), profile.token, action)

        if (routes.isEmpty()) return packets()
        return when (mode) {
            "http_only" -> "（仅 HTTP 模式）" + http()

            "http_first" -> try {
                "（优先 HTTP 模式）" + http()
            } catch (httpError: Exception) {
                try {
                    "HTTP 没通（${httpError.message ?: "未知错误"}），已改发广播包：" + packets()
                } catch (packetError: Exception) {
                    throw Exception(
                        "HTTP 没通（${httpError.message ?: "未知错误"}）；" +
                            "广播包也没发出去：${packetError.message ?: "未知错误"}"
                    )
                }
            }

            "hybrid" -> {
                val httpOutcome = java.util.concurrent.atomic.AtomicReference<Any?>(null)
                val httpThread = Thread {
                    httpOutcome.set(
                        try {
                            http()
                        } catch (error: Exception) {
                            error
                        }
                    )
                }.apply { start() }
                var packetMessage: String? = null
                var packetError: Exception? = null
                try {
                    packetMessage = packets()
                } catch (error: Exception) {
                    packetError = error
                }
                httpThread.join()
                when (val httpValue = httpOutcome.get()) {
                    is Exception -> if (packetMessage != null) {
                        "（混合模式）广播包已发出：$packetMessage；" +
                            "HTTP 接口失败：${httpValue.message ?: "未知错误"}"
                    } else {
                        throw Exception(
                            "（混合模式）两边都失败 —— 广播包：" +
                                "${packetError?.message ?: "未知错误"}；" +
                                "HTTP：${httpValue.message ?: "未知错误"}"
                        )
                    }

                    else -> if (packetMessage != null) {
                        "（混合模式）HTTP 与广播包已同时发送 —— HTTP：$httpValue；广播包：$packetMessage"
                    } else {
                        "（混合模式）HTTP 接口已执行：$httpValue；" +
                            "广播包没发出去（${packetError?.message ?: "未知错误"}）"
                    }
                }
            }

            // auto = 优先广播包（失败走 HTTP 兜底）；packet_only = 失败就失败
            else -> try {
                packets()
            } catch (packetError: Exception) {
                if (mode != "auto") throw packetError
                val outcome = try {
                    http()
                } catch (httpError: Exception) {
                    throw Exception(
                        "${packetError.message ?: "广播包没发出去"}；" +
                            "HTTP 兜底也失败：${httpError.message ?: "未知错误"}"
                    )
                }
                "广播包没发出去（${packetError.message ?: "未知错误"}），" +
                    "已改走它自己的 AnyBoot 接口：$outcome"
            }
        }
    }

    private fun powerAddress(profile: ServerProfile): String {
        val host = profile.host()
        if (host.isEmpty()) throw Exception("这台电脑还没有可用地址")
        if (ServerProfile.text(profile.jumpServerId).isEmpty()) return host
        val jump = JumpStore.findJump(JumpStore.loadJumps(this), profile.jumpServerId)
            ?: throw Exception("跳板机不存在（可能已删除）")
        return SshSessionManager.shared().tunnelAddress(this, jump, host, prompt())
    }

    // ------------------------------------------------------------------
    // 小工具
    // ------------------------------------------------------------------

    @Composable
    private fun toneOf(): CqTone {
        val dark = remember { Ui.dark(this) }
        return remember(dark) { if (dark) CqTone.dark() else CqTone.light() }
    }

    @Composable
    private fun stateColor(state: String): Color {
        val tone = LocalTone.current
        return when (state) {
            STATE_ONLINE -> tone.online
            STATE_OFFLINE -> tone.offline
            STATE_SLEEPING -> tone.sleep
            STATE_UNKNOWN -> LocalPalette.current.lineStrong
            else -> tone.busy
        }
    }

    private fun toast(text: String) {
        val item = CqToast(text)
        toasts.add(item)
        handler.postDelayed({
            item.shown = false
            handler.postDelayed({ toasts.remove(item) }, 240)
        }, 2400)
    }

    private fun copyText(value: String) {
        val manager = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        manager.setPrimaryClip(ClipData.newPlainText("anyboot", value))
    }

    // ------------------------------------------------------------------
    // 数据模型
    // ------------------------------------------------------------------

    private data class CqRow(
        val profile: ServerProfile,
        val name: String,
        val group: String,
        val ip: String,
        val mac: String,
        val refind: BootGroup,
        val grub: BootGroup,
        val state: String,
    )

    private data class BootOption(val value: String, val label: String, val kind: String)

    private data class BootGroup(
        val kind: String,
        val title: String,
        val available: Boolean,
        val current: String,
        val next: String,
        val saved: String,
        val note: String,
        val options: List<BootOption>,
    )

    private data class BootInfo(
        val ok: Boolean,
        val online: Boolean,
        val addr: String,
        val error: String,
        val refind: BootGroup,
        val grub: BootGroup,
    )

    private data class SheetRequest(val rid: String, val kind: String)

    private class CqToast(val text: String) {
        var shown by mutableStateOf(true)
    }

    private companion object {
        const val FILTER_ALL = "all"
        const val FILTER_ONLINE = "online"
        const val FILTER_OFFLINE = "offline"
        const val FILTER_PENDING = "pending"

        const val STATE_ONLINE = "online"
        const val STATE_OFFLINE = "offline"
        const val STATE_WAKING = "waking"
        const val STATE_SHUTTING = "shutting"
        const val STATE_REBOOTING = "rebooting"
        const val STATE_SLEEPING = "sleeping"
        const val STATE_UNKNOWN = "unknown"

        const val KIND_REFIND = "refind"
        const val KIND_GRUB = "grub"
        const val KIND_HELP = "help"

        const val POWER_WAKE = "wake"
        const val POWER_SLEEP = "sleep"
        const val POWER_REBOOT = "reboot"
        const val POWER_SHUTDOWN = "shutdown"

        const val PROBE_TIMEOUT_MS = 3000
        const val CONNECT_TIMEOUT_MS = 4000
        const val READ_TIMEOUT_MS = 8000

        val HELP_LINES = listOf(
            "开机：发标准魔术包（UDP 9）。睡眠 / 重启 / 关机按每台电脑「电源路由」里配的通道发送，" +
                "可以是局域网广播包，也可以走它自己的 AnyBoot 接口",
            "关机与重启立即执行，不会二次确认，请先保存工作",
            "引导项（rEFInd / GRUB）在下次重启时生效，可点「撤销」恢复成原来那项",
            "电脑离线时读不到引导项，只有「开机」可用",
        )
    }
}

/** 电脑列表页专用的一层语义色（设计稿的 #059669 / #C7CEDD / #D97706 / #2563EB 在暗色下要提亮） */
private data class CqTone(
    val online: Color,
    val offline: Color,
    val offlineText: Color,
    val busy: Color,
    val sleep: Color,
    val soft: Color,
    val softDisabled: Color,
    val dashed: Color,
    val pendBg: Color,
    val pendBorder: Color,
    val pendText: Color,
) {
    companion object {
        fun light() = CqTone(
            online = Color(0xFF059669),
            offline = Color(0xFFC7CEDD),
            offlineText = Color(0xFF7B8497),
            busy = Color(0xFFD97706),
            sleep = Color(0xFF2563EB),
            soft = Color(0xFFFAFBFF),
            softDisabled = Color(0xFFF7F8FC),
            dashed = Color(0xFFDDE1EE),
            pendBg = Color(0xFFFFFCF2),
            pendBorder = Color(0xFFF0C979),
            pendText = Color(0xFFB45309),
        )

        fun dark() = CqTone(
            online = Color(0xFF22C55E),
            offline = Color(0xFF3F4A63),
            offlineText = Color(0xFF94A3B8),
            busy = Color(0xFFF59E0B),
            sleep = Color(0xFF60A5FA),
            soft = Color(0xFF111A2E),
            softDisabled = Color(0xFF0B1120),
            dashed = Color(0xFF2A3550),
            pendBg = Color(0xFF2A2210),
            pendBorder = Color(0xFF5C4718),
            pendText = Color(0xFFFBBF24),
        )
    }
}

private val LocalTone = staticCompositionLocalOf { CqTone.light() }

/** 虚线边框（`Modifier.border` 只会画实线，这里按圆角矩形自己描一条虚线） */
private fun Modifier.dashedBorder(color: Color, radius: Dp, width: Dp): Modifier = drawBehind {
    drawRoundRect(
        color = color,
        cornerRadius = CornerRadius(radius.toPx(), radius.toPx()),
        style = Stroke(
            width = width.toPx(),
            pathEffect = PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 3.dp.toPx())),
        ),
    )
}
