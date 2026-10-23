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
import androidx.compose.foundation.layout.heightIn
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
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
    /** 左上角 ☰ 呼出的抽屉侧边栏（与电脑页/服务器页共用，见 Drawer.kt） */
    private var drawerOpen by mutableStateOf(false)
    private var query by mutableStateOf("")
    private var filter by mutableStateOf(FILTER_ALL)
    private var sheetRequest by mutableStateOf<SheetRequest?>(null)

    /**
     * 手机自己的 GRUB 列表格式（与 PC 端 ⋮ 菜单同一套五档）。
     * 空 = 跟随服务端 /api/grub 响应里的 listFormat（服务端没给就退 minimal）。
     * 用户在 GRUB 选择框里点过档位后就固定用手机自己的选择（存 anyboot_ui）。
     */
    private var grubFmt by mutableStateOf("")

    /** 正在编辑的电脑（非空 = 底部弹出编辑 sheet）；与 MainActivity 共用同一套表单 */
    private var editTarget by mutableStateOf<ServerProfile?>(null)

    /** 待确认删除的电脑（非空 = 弹确认框）；删除是不可逆的，必须二次确认 */
    private var deleteTarget by mutableStateOf<ServerProfile?>(null)

    // ---- ⋮ 溢出菜单（与电脑页 ⋮ 同一套功能，读写同一份偏好） ----
    private var moreServerListOpen by mutableStateOf(false)
    private var moreTransportOpen by mutableStateOf(false)
    private var moreFailReloadOpen by mutableStateOf(false)
    private var moreConnectAll by mutableStateOf(false)
    private var moreTransport by mutableStateOf("auto")
    private var moreFailReloadSec by mutableStateOf(5)

    /** 列表里选中的电脑（点卡片选中 = 设为「当前电脑」；「重载当前电脑」作用于它） */
    private var selectedId by mutableStateOf("")

    // ------------------------------------------------------------------
    // 生命周期
    // ------------------------------------------------------------------

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Ui.applySystemBars(this)
        grubFmt = getSharedPreferences(UI_PREFS, MODE_PRIVATE).getString("grubListFormat", "") ?: ""
        // ⋮ 菜单的三个全局开关初始值（与电脑页读写同一份偏好）
        moreConnectAll = ServerStore.connectAll(this)
        moreTransport = ServerStore.transport(this)
        moreFailReloadSec = getSharedPreferences(UI_PREFS, MODE_PRIVATE).getInt("failReloadSec", 5)
        // 默认选中电脑页的当前那台；之后点卡片即切换
        selectedId = ServerStore.activeId(this)
        reloadServers()
        setContent { Root() }
    }

    override fun onResume() {
        super.onResume()
        // 记「上次停留页」= 电脑列表：应用没退时再点桌面图标，singleTask 的 MainActivity
        // 会被 launcher intent 提到前台 —— 靠这个记录把用户送回列表（见 MainActivity.onNewIntent）
        getSharedPreferences(UI_PREFS, MODE_PRIVATE).edit().putString(PREF_ENTRY, "computers").apply()
        // 从「服务器」页回来可能增删过电脑 —— 重新读一遍再整体探活
        reloadServers()
        refreshAll()
        startAutoProbe()
    }

    override fun onPause() {
        super.onPause()
        // 页面不在前台就停表，回前台 onResume 会先补探一轮再排下去
        stopAutoProbe()
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
        // ⚠️ 取色必须在 AppTheme 里面：LocalPalette 的默认值是 paletteOf(Application())，
        // 那个 Application 没有 base context，一旦读到就是 ServerStore.prefs 上的 NPE
        // —— 少了这层包裹，进电脑列表页会直接闪退（与 TerminalSettingsActivity 同款约定）。
        AppTheme {
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
                    if (drawerOpen) AppDrawerOverlay(DRAWER_COMPUTER_LIST) { drawerOpen = false }
                }

                sheetRequest?.let { request -> PickSheet(request) }
                editTarget?.let { profile -> EditSheet(profile) }
                deleteTarget?.let { profile -> DeleteConfirm(profile) }
                if (moreServerListOpen) MoreServerListDialog()
                if (moreTransportOpen) MoreTransportDialog()
                if (moreFailReloadOpen) MoreFailReloadDialog()
                ConfirmDialog(bus)
            }
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
                TopBarIcon("侧边栏", { drawerOpen = true }) {
                    MenuIcon(tint = palette.fg, size = 20.dp)
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
                    Text(
                        "远程管理 - 电脑 · 点卡片即可发电源指令或切引导项",
                        fontSize = 11.sp,
                        color = palette.fgFaint,
                        maxLines = 1,
                    )
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
                OverflowMenu()
                Spacer(Modifier.width(2.dp))
            }

            Column(Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, bottom = 10.dp)) {
                SearchBox()
                // 统计筛选条：一个块同时承担 PC 端「三个大数字统计卡」和「四段筛选」两个职责。
                // 手机宽度放不下两套控件，合成一排是最省地方的做法。
                // ⚠️ 数字必须基于全量 rows —— 不能受搜索框和当前筛选影响，
                //    否则点进「离线」后「在线」会变成 0（PC 端 cqUpdateStats 也是这个语义）。
                Row(
                    horizontalArrangement = Arrangement.spacedBy(7.dp),
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 9.dp)
                        .horizontalScroll(rememberScrollState()),
                ) {
                    val online = rows.count { it.state == STATE_ONLINE }
                    val pending = rows.count { grubPending(it.grub) }
                    StatChip("全部", rows.size, filter == FILTER_ALL, false) { filter = FILTER_ALL }
                    StatChip("在线", online, filter == FILTER_ONLINE, false) { filter = FILTER_ONLINE }
                    StatChip("离线", rows.size - online, filter == FILTER_OFFLINE, false) {
                        filter = FILTER_OFFLINE
                    }
                    StatChip("待生效", pending, filter == FILTER_PENDING, true) {
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

    /** ⋮ 溢出菜单：与电脑页 ⋮（MainActivity）同一套功能，全局设置读写同一份偏好 */
    @Composable
    private fun OverflowMenu() {
        val palette = LocalPalette.current
        var open by remember { mutableStateOf(false) }
        Box {
            TopBarIcon("更多", { open = true }) {
                Text("⋮", fontSize = 19.sp, color = palette.fgDim)
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                DropdownMenuItem(text = { Text("添加服务端") }, onClick = {
                    open = false
                    editTarget = ServerProfile()
                })
                DropdownMenuItem(text = { Text("编辑当前服务端") }, onClick = {
                    open = false
                    val profile = ServerStore.find(servers.toList(), ServerStore.activeId(this@ComputerListActivity))
                    if (profile == null) toast("电脑页还没有当前电脑 —— 卡片右侧 ⋮ 也能直接编辑")
                    else editTarget = profile
                })
                DropdownMenuItem(text = { Text("服务端列表") }, onClick = {
                    open = false; moreServerListOpen = true
                })
                // 重载选中的那台（点卡片选中）；什么都没选就引导去点卡片
                DropdownMenuItem(text = { Text("重载当前电脑") }, onClick = {
                    open = false
                    val id = selectedId.ifEmpty { ServerStore.activeId(this@ComputerListActivity) }
                    val profile = ServerStore.find(servers.toList(), id)
                    if (profile == null) toast("先点一下卡片选中要重载的电脑")
                    else reloadBoot(profile)
                })
                DropdownMenuItem(text = { Text("全部重载") }, onClick = {
                    open = false; refreshAll()
                })
                DropdownMenuItem(
                    text = { Text(if (moreConnectAll) "✓ 启动时连接全部服务端" else "启动时连接全部服务端") },
                    onClick = {
                        open = false
                        moreConnectAll = !moreConnectAll
                        ServerStore.setConnectAll(this@ComputerListActivity, moreConnectAll)
                        toast(
                            if (moreConnectAll) "已开启：启动时把所有服务端一起连上"
                            else "已关闭：只连当前标签"
                        )
                    },
                )
                DropdownMenuItem(
                    text = {
                        Text(
                            if (moreFailReloadSec > 0) "连不上时自动重载（每 $moreFailReloadSec 秒）"
                            else "连不上时自动重载（已关闭）"
                        )
                    },
                    onClick = { open = false; moreFailReloadOpen = true },
                )
                DropdownMenuItem(text = { Text("传输方式（${transportLabel()}）") }, onClick = {
                    open = false; moreTransportOpen = true
                })
            }
        }
    }

    /** 菜单上那一条「传输方式（…）」给人看的说法（与 MainActivity 同一套值） */
    private fun transportLabel(): String = when (moreTransport) {
        "ws" -> "推送"
        "http" -> "轮询"
        else -> "自动"
    }

    // ---- ⋮ 菜单的三个弹窗（与 MainActivity 同一套设置，读写同一份偏好） ----

    /** 服务端列表：电脑页那个切换弹窗的列表页版 —— 这里点一台直接进编辑 */
    @Composable
    private fun MoreServerListDialog() {
        val palette = LocalPalette.current
        AlertDialog(
            onDismissRequest = { moreServerListOpen = false },
            containerColor = palette.card,
            title = { Text("服务端列表（点一台进行编辑）", fontSize = 15.sp, fontWeight = FontWeight.Bold) },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    if (servers.isEmpty()) {
                        Text("还没有电脑 —— 点「添加服务端」建一条。", fontSize = 13.sp, color = palette.fgDim)
                    }
                    servers.forEach { profile ->
                        // 用户要求：每个条目把所有配置好的 IP 都亮出来，不再用「另有 N 个地址」摘要
                        Column(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    moreServerListOpen = false
                                    editTarget = profile
                                }
                                .padding(vertical = 10.dp),
                        ) {
                            Text(
                                text = "· " + profile.label(),
                                fontSize = 14.sp,
                                color = palette.fg,
                            )
                            val count = profile.addressCount()
                            if (count == 0) {
                                Text(
                                    text = "　(未配地址)",
                                    fontSize = 11.sp,
                                    color = palette.fgDim,
                                )
                            } else {
                                for (i in 0 until count) {
                                    Text(
                                        text = "　" + profile.hostAt(i),
                                        fontSize = 11.sp,
                                        color = palette.fgDim,
                                        fontFamily = FontFamily.Monospace,
                                    )
                                }
                            }
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    moreServerListOpen = false
                    editTarget = ServerProfile()
                }) { Text("添加") }
            },
            dismissButton = {
                TextButton(onClick = { moreServerListOpen = false }) { Text("关闭") }
            },
        )
    }

    /** 连不上时自动重载的间隔设置（同一偏好：anyboot_ui/failReloadSec，电脑页同步生效） */
    @Composable
    private fun MoreFailReloadDialog() {
        val palette = LocalPalette.current
        var custom by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { moreFailReloadOpen = false },
            containerColor = palette.card,
            title = { Text("连不上时自动重载") },
            text = {
                Column {
                    Text(
                        "连接失败时每隔一段时间自动重载当前页面（重新探测全部地址）。",
                        fontSize = 13.sp, color = palette.fgDim,
                    )
                    Spacer(Modifier.height(10.dp))
                    listOf(5, 10, 30, 60, 0).forEach { sec ->
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            RadioButton(
                                selected = moreFailReloadSec == sec,
                                onClick = {
                                    moreFailReloadSec = sec
                                    saveMoreFailReloadSec()
                                },
                            )
                            Text(
                                if (sec == 0) "永不（关闭自动重载）" else "每 $sec 秒",
                                fontSize = 13.sp, color = palette.fg,
                            )
                        }
                    }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(
                            selected = moreFailReloadSec !in listOf(5, 10, 30, 60, 0),
                            onClick = { },
                        )
                        OutlinedTextField(
                            value = custom.ifEmpty {
                                if (moreFailReloadSec in listOf(5, 10, 30, 60, 0)) "" else moreFailReloadSec.toString()
                            },
                            onValueChange = { custom = it.filter(Char::isDigit) },
                            label = { Text("自定义秒数", fontSize = 11.sp) },
                            singleLine = true,
                            textStyle = TextStyle(fontSize = 13.sp, color = palette.fg),
                            modifier = Modifier.weight(1f),
                        )
                        Spacer(Modifier.width(8.dp))
                        TextButton(onClick = {
                            val parsed = custom.toIntOrNull()
                            if (parsed == null || parsed < 0) {
                                toast("间隔要是不小于 0 的整数秒")
                                return@TextButton
                            }
                            moreFailReloadSec = parsed
                            saveMoreFailReloadSec()
                            moreFailReloadOpen = false
                        }) { Text("应用") }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { moreFailReloadOpen = false }) { Text("完成") }
            },
        )
    }

    private fun saveMoreFailReloadSec() {
        getSharedPreferences(UI_PREFS, MODE_PRIVATE).edit().putInt("failReloadSec", moreFailReloadSec).apply()
        toast(
            if (moreFailReloadSec > 0) "连不上时每 $moreFailReloadSec 秒自动重载"
            else "已关闭自动重载"
        )
    }

    /** 传输方式（全局）：改完电脑页重载后生效 */
    @Composable
    private fun MoreTransportDialog() {
        val palette = LocalPalette.current
        val values = listOf("auto", "ws", "http")
        val labels = listOf(
            "自动（推送优先，连不上退回轮询）",
            "推送（WebSocket）",
            "轮询（HTTP 每 2s）",
        )
        AlertDialog(
            onDismissRequest = { moreTransportOpen = false },
            containerColor = palette.card,
            title = { Text("传输方式") },
            text = {
                Column {
                    values.forEachIndexed { index, value ->
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    moreTransport = value
                                    ServerStore.setTransport(this@ComputerListActivity, value)
                                    moreTransportOpen = false
                                    toast("已选择「${transportLabel()}」：电脑页重载后生效")
                                }
                                .padding(vertical = 6.dp),
                        ) {
                            RadioButton(selected = moreTransport == value, onClick = {
                                moreTransport = value
                                ServerStore.setTransport(this@ComputerListActivity, value)
                                moreTransportOpen = false
                                toast("已选择「${transportLabel()}」：电脑页重载后生效")
                            })
                            // 最长一条在 14sp 下已接近行宽，系统字体一放大会挤爆，提前上 weight
                            Text(labels[index], fontSize = 14.sp, color = palette.fg, modifier = Modifier.weight(1f, fill = false))
                        }
                    }
                }
            },
            confirmButton = {},
            dismissButton = {
                TextButton(onClick = { moreTransportOpen = false }) { Text("取消") }
            },
        )
    }

    @Composable
    private fun SearchBox() {        val palette = LocalPalette.current
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

    /**
     * 统计筛选块：数字在上、标签在下，块高 44dp。
     *
     * 为什么不用 PC 那种「三个大数字卡 + 四个筛选按钮」：手机宽度放不下两排，
     * 合成一排让每个块同时承担「看数」和「筛选」两个职责。
     * [tone] 为 true 的块（待生效）数字用琥珀色 —— 它代表「有待处理的改动」，
     * 混在一堆中性数字里会被看漏。
     */
    @Composable
    private fun StatChip(
        label: String,
        count: Int,
        selected: Boolean,
        tone: Boolean,
        onClick: () -> Unit,
    ) {
        val palette = LocalPalette.current
        val shape = RoundedCornerShape(12.dp)
        val accent = if (tone) palette.warn else palette.accent
        val background = if (selected) accent else palette.card
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.Start,
            modifier = Modifier
                .height(44.dp)
                .clip(shape)
                .background(background)
                .border(1.dp, if (selected) accent else palette.line, shape)
                .clickable(onClick = onClick)
                .padding(horizontal = 13.dp),
        ) {
            Column(horizontalAlignment = Alignment.Start) {
                Text(
                    count.toString(),
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                    fontFamily = FontFamily.Monospace,
                    color = when {
                        selected -> palette.onAccent
                        tone -> palette.warn
                        else -> palette.fg
                    },
                    maxLines = 1,
                )
                Spacer(Modifier.height(1.dp))
                Text(
                    label,
                    fontSize = 10.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = if (selected) palette.onAccent.copy(alpha = 0.78f) else palette.fgFaint,
                    maxLines = 1,
                )
            }
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
        // 选中 = 当前电脑：accent 底色 + accent 边框，比普通卡片明显一档
        val selected = row.profile.id == selectedId

        Column(
            modifier = Modifier
                .fillMaxWidth()
                .clip(shape)
                .background(if (selected) palette.accent.copy(alpha = 0.07f) else palette.card)
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
                .border(
                    if (selected) 1.5.dp else 1.dp,
                    if (selected) palette.accent else palette.line,
                    shape,
                )
                // 点卡片空白处 = 选中这台（卡片内的按钮各自消费点击，不会误触）
                .clickable { selectComputer(row.profile.id) }
                .padding(start = 13.dp, end = 13.dp, top = 12.dp, bottom = 13.dp),
        ) {
            // 名称 + 当前徽标 + 分组 chip + ⋮
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
                if (selected) {
                    Spacer(Modifier.width(7.dp))
                    Text(
                        "当前",
                        fontSize = 10.sp,
                        fontWeight = FontWeight.SemiBold,
                        color = palette.accent,
                        maxLines = 1,
                        modifier = Modifier
                            .clip(RoundedCornerShape(5.dp))
                            .background(palette.accent.copy(alpha = 0.16f))
                            .border(1.dp, palette.accent.copy(alpha = 0.3f), RoundedCornerShape(5.dp))
                            .padding(horizontal = 5.dp, vertical = 1.dp),
                    )
                }
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
                // 当前连接标注：直连 / 跳板机·xxx / 远程隧道（隧道 = 探测赢家确实是它）
                Spacer(Modifier.width(7.dp))
                val connColor = if (row.connRemote) palette.accent else palette.fgFaint
                Text(
                    row.conn,
                    fontSize = 10.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = connColor,
                    maxLines = 1,
                    modifier = Modifier
                        .clip(RoundedCornerShape(5.dp))
                        .background(connColor.copy(alpha = 0.12f))
                        .border(1.dp, connColor.copy(alpha = 0.28f), RoundedCornerShape(5.dp))
                        .padding(horizontal = 5.dp, vertical = 1.dp),
                )
                Spacer(Modifier.weight(1f))
                CardMenu(row)
            }

            // 状态 + 全部配置的地址：第一行跟着状态，其余地址（和各自的 MAC）逐行补在下面
            val addrCount = row.profile.addressCount()
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.padding(start = 4.dp, top = 5.dp),
            ) {
                StatusLine(row.state)
                if (addrCount > 0) {
                    val addr0 = displayAddrOf(row.profile.hostAt(0))
                    Spacer(Modifier.width(6.dp))
                    Text("·", fontSize = 11.sp, color = palette.fgFaint)
                    Spacer(Modifier.width(6.dp))
                    Text(
                        addr0,
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                        color = palette.fgFaint,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    row.profile.macsForHost(row.profile.hostAt(0)).forEach { mac ->
                        Spacer(Modifier.width(6.dp))
                        Text("·", fontSize = 11.sp, color = palette.fgFaint)
                        Spacer(Modifier.width(6.dp))
                        Text(
                            mac.uppercase(Locale.US),
                            fontSize = 11.sp,
                            fontFamily = FontFamily.Monospace,
                            color = palette.fgFaint,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                } else {
                    Spacer(Modifier.width(6.dp))
                    Text("·", fontSize = 11.sp, color = palette.fgFaint)
                    Spacer(Modifier.width(6.dp))
                    Text("(未配地址)", fontSize = 11.sp, color = palette.fgFaint)
                }
            }
            for (i in 1 until addrCount) {
                val addr = row.profile.hostAt(i)
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(start = 4.dp, top = 3.dp),
                ) {
                    Text(
                        displayAddrOf(addr),
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                        color = palette.fgFaint,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    row.profile.macsForHost(addr).forEach { mac ->
                        Spacer(Modifier.width(6.dp))
                        Text("·", fontSize = 11.sp, color = palette.fgFaint)
                        Spacer(Modifier.width(6.dp))
                        Text(
                            mac.uppercase(Locale.US),
                            fontSize = 11.sp,
                            fontFamily = FontFamily.Monospace,
                            color = palette.fgFaint,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
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
                // 提示小字与悬浮电源球同一套（PowerHint.kt）：传输标识（HTTP/魔法包）+ 路由摘要 / 未配 MAC；
                // 该行正走远程隧道时广播包腿发不出，标识会相应变成 HTTP 或「发不出」
                PowerButton(
                    row, POWER_WAKE, "唤醒",
                    hint = powerHint(row.profile, POWER_WAKE, if (row.profile.hasMacs()) "MAC 魔术包" else "未配 MAC", row.connRemote),
                    modifier = Modifier.weight(1f),
                )
                PowerButton(
                    row, POWER_SLEEP, "睡眠",
                    hint = powerHint(row.profile, POWER_SLEEP, "挂起到内存", row.connRemote),
                    modifier = Modifier.weight(1f),
                )
                PowerButton(
                    row, POWER_REBOOT, "重启",
                    hint = powerHint(row.profile, POWER_REBOOT, "", row.connRemote),
                    modifier = Modifier.weight(1f),
                )
                PowerButton(
                    row, POWER_SHUTDOWN, "关机",
                    hint = powerHint(row.profile, POWER_SHUTDOWN, "", row.connRemote),
                    modifier = Modifier.weight(1f),
                )
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

    /**
     * 行菜单：与 PC 端 `ui/hub.html` 的 `cqRowMenu` 一一对应。
     *
     * PC 那 7 项的「打开页面（内嵌查看）」和「在电脑页打开（选中标签）」在手机上是同一件事
     * —— 手机没有 iframe 容器，MainActivity 本身就是标签容器 —— 所以合并成一条「打开页面」，
     * 其余照搬：在浏览器打开 / 重新探测 / 编辑 / 复制地址 / 删除。
     *
     * ⚠️ 「编辑这台电脑」以前跳到 [ServersActivity]，那是 **SSH 跳板机管理**（终端 + SFTP），
     *    跟电脑档案（IP / MAC / 电源路由）完全无关 —— 点进去的是一脸懵的服务器列表。
     *    现在改成本页直接开编辑 sheet。
     */
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
                // —— 打开系列 ——
                MenuRow("打开页面", { ScreenIcon(tint = palette.fgFaint, size = 16.dp) }) {
                    open = false
                    openComputerPage(row.profile.id)
                }
                MenuRow("在浏览器打开", { GlobeIcon(tint = palette.fgFaint, size = 16.dp) }) {
                    open = false
                    openInBrowser(row.profile)
                }
                MenuDivider()
                // —— 维护系列 ——
                MenuRow("重新探测这台电脑", { RefreshIcon(tint = palette.fgFaint, size = 16.dp) }) {
                    open = false
                    reloadBoot(row.profile)
                }
                MenuRow("编辑这台电脑", { PencilIconSmall(tint = palette.fgFaint, size = 16.dp) }) {
                    open = false
                    editTarget = row.profile
                }
                MenuRow("复制地址", { CopyIcon(tint = palette.fgFaint, size = 16.dp) }) {
                    open = false
                    copyText(ServerProfile.normalizeHost(row.profile.host()))
                    toast("已复制 ${row.ip}")
                }
                MenuDivider()
                MenuRow(
                    "删除这台电脑",
                    { TrashIcon(tint = palette.bad, size = 16.dp) },
                    danger = true,
                ) {
                    open = false
                    deleteTarget = row.profile
                }
            }
        }
    }

    /** 菜单行：图标 + 文字，[danger] 的整行染红（删除项） */
    @Composable
    private fun MenuRow(
        label: String,
        icon: @Composable () -> Unit,
        danger: Boolean = false,
        onClick: () -> Unit,
    ) {
        val palette = LocalPalette.current
        DropdownMenuItem(
            text = {
                Text(
                    label,
                    fontSize = 13.sp,
                    color = if (danger) palette.bad else palette.fg,
                )
            },
            leadingIcon = icon,
            onClick = onClick,
        )
    }

    /** 菜单里的分隔线（PC 端 cqRowMenu 也有，用来分组） */
    @Composable
    private fun MenuDivider() {
        val palette = LocalPalette.current
        Box(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 3.dp)
                .height(1.dp)
                .background(palette.line)
        )
    }

    // ---- 编辑 / 删除 --------------------------------------------------

    /**
     * 编辑这台电脑：底部 sheet + 与 MainActivity 共用的表单体（[ServerEditFormBody]）。
     *
     * 这里不走 MainActivity 的 AlertDialog，因为它是另一棵树 —— 从本页跳过去还得回退，
     * 而且用户会把「编辑完回来列表没刷新」当成 bug。原位弹、原位存最省心。
     */
    @OptIn(ExperimentalMaterial3Api::class)
    @Composable
    private fun EditSheet(profile: ServerProfile) {
        val palette = LocalPalette.current
        val form = rememberServerEditFormState(profile)

        ModalBottomSheet(
            onDismissRequest = { editTarget = null },
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
                    .navigationBarsPadding(),
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(start = 18.dp, end = 8.dp, bottom = 10.dp),
                ) {
                    Text(
                        "编辑这台电脑",
                        fontSize = 15.sp,
                        fontWeight = FontWeight.Bold,
                        color = palette.fg,
                        maxLines = 1,
                    )
                    Spacer(Modifier.weight(1f))
                    Box(
                        Modifier
                            .size(30.dp)
                            .clip(CircleShape)
                            .background(palette.elevate)
                            .clickable { editTarget = null },
                        contentAlignment = Alignment.Center,
                    ) {
                        CloseIcon(tint = palette.fgDim, size = 14.dp)
                    }
                }
                // 表单自己带滚动：内容很长（电源路由可以无限加），高度交给它
                Box(Modifier.fillMaxWidth().heightIn(max = 520.dp).padding(horizontal = 18.dp)) {
                    ServerEditFormBody(form)
                }
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(start = 14.dp, end = 14.dp, top = 8.dp, bottom = 12.dp),
                ) {
                    // 删除放在左边、单独一格：它跟保存不是一类动作，混在右边容易被误点
                    Box(
                        Modifier
                            .height(40.dp)
                            .clip(RoundedCornerShape(11.dp))
                            .background(palette.bad.copy(alpha = 0.1f))
                            .border(1.dp, palette.bad.copy(alpha = 0.28f), RoundedCornerShape(11.dp))
                            .clickable {
                                val target = profile
                                editTarget = null
                                deleteTarget = target
                            }
                            .padding(horizontal = 14.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        TrashIcon(tint = palette.bad, size = 16.dp)
                    }
                    Spacer(Modifier.width(8.dp))
                    Box(
                        Modifier
                            .weight(1f)
                            .height(40.dp)
                            .clip(RoundedCornerShape(11.dp))
                            .background(palette.accent)
                            .clickable {
                                if (saveEdited(profile, form)) editTarget = null
                            },
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            "保存",
                            fontSize = 13.5.sp,
                            fontWeight = FontWeight.SemiBold,
                            color = palette.onAccent,
                        )
                    }
                }
            }
        }
    }

    /** 保存编辑结果：写盘 → 关 sheet → 立刻重探（地址/MAC 可能变了） */
    private fun saveEdited(profile: ServerProfile, form: ServerEditFormState): Boolean {
        if (form.toParsed().isEmpty()) {
            toast(SERVER_EDIT_EMPTY_ADDRESS_HINT)
            return false
        }
        if (!applyServerEdit(form, profile, replaceAddresses = true)) {
            toast(SERVER_EDIT_EMPTY_ADDRESS_HINT)
            return false
        }
        val list = servers.toMutableList()
        // ⋮「添加服务端」走的是空白新档案：不在列表里就补进去（编辑已有档案时本来就在）
        if (list.none { it.id == profile.id }) list.add(profile)
        ServerStore.sort(list)
        ServerStore.save(this, list)
        reloadServers()
        toast("已保存")
        reloadBoot(profile)
        return true
    }

    /** 删除确认：不可逆动作，二次确认（PC 端 cqDeleteRemote 也是 window.confirm） */
    @Composable
    private fun DeleteConfirm(profile: ServerProfile) {
        val palette = LocalPalette.current
        AlertDialog(
            onDismissRequest = { deleteTarget = null },
            containerColor = palette.card,
            title = { Text("删除这台电脑", fontSize = 15.sp, fontWeight = FontWeight.Bold) },
            text = {
                Text(
                    "确定删除「${profile.displayName().ifEmpty { profile.host() }}」？\n\n" +
                        "只删掉手机上的这条档案，不会动目标电脑上的 AnyBoot。",
                    fontSize = 13.sp,
                    color = palette.fgDim,
                    lineHeight = 20.sp,
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    deleteTarget = null
                    doDelete(profile)
                }) {
                    Text("删除", color = palette.bad)
                }
            },
            dismissButton = {
                TextButton(onClick = { deleteTarget = null }) { Text("取消") }
            },
        )
    }

    private fun doDelete(profile: ServerProfile) {
        val list = servers.filter { it.id != profile.id }
        ServerStore.save(this, list)
        boots.remove(profile.id)
        transients.remove(profile.id)
        busy.remove(profile.id)
        reloadServers()
        toast("已删除")
    }

    // ---- 打开页面 / 浏览器 ---------------------------------------------

    /** 打开这台电脑的 AnyBoot 页面（MainActivity 的标签容器） */
    private fun openComputerPage(id: String) {
        val intent = Intent(this, MainActivity::class.java)
            .putExtra(MainActivity.EXTRA_SERVER_ID, id)
            .addFlags(Intent.FLAG_ACTIVITY_REORDER_TO_FRONT)
        startActivity(intent)
    }

    /**
     * 用系统浏览器打开。
     *
     * 注意手机端是明文 HTTP 局域网地址，因此这里的 URL 直接用档案里的地址 + token，
     * 不做隧道处理 —— 走跳板机的电脑在浏览器里本来就连不上，明确提示比静默失败好。
     */
    private fun openInBrowser(profile: ServerProfile) {
        val host = ServerProfile.normalizeHost(profile.host())
        if (host.isEmpty()) {
            toast("这台电脑还没有可用地址")
            return
        }
        if (ServerProfile.text(profile.jumpServerId).isNotEmpty()) {
            toast("这台电脑走跳板机，系统浏览器连不上 —— 请用「打开页面」")
            return
        }
        val url = profile.urlFor(host, "")
        if (url.isEmpty()) {
            toast("地址还没就绪")
            return
        }
        try {
            startActivity(Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url)))
        } catch (error: Exception) {
            toast("没有能打开网页的应用")
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
            // rEFInd 直接用 label（走 GRUB 拼装会被折叠成「· N 个变体」）；
            // GRUB 的当前值取托盘同款拼装出来的 **GrubRow.label** —— 不能取
            // option.label：GRUB 的原料 label 恒为空串（展示文字在渲染期现算），
            // 直接取会显示空白（正是「GRUB 选中项空白」的根因）
            if (kind == KIND_REFIND) {
                val hit = group.options.firstOrNull { it.value == group.current }
                valueLabel = hit?.label ?: group.current.ifEmpty { "未设置" }
                dot = hit?.kind ?: "linux"
            } else {
                val hit = grubDisplayRows(group).firstOrNull { it.option?.value == group.current }
                valueLabel = hit?.label ?: group.current.ifEmpty { "未设置" }
                dot = hit?.option?.kind ?: "linux"
            }
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
    private fun PowerButton(
        row: CqRow,
        action: String,
        label: String,
        hint: String = "",
        modifier: Modifier = Modifier,
    ) {
        val palette = LocalPalette.current
        val tone = LocalTone.current
        val working = busy[row.profile.id] == "power:$action"
        // 用户定案：四个电源键**任何时候都能按**，不做变灰/禁用判定。
        // 离线卡片上「唤醒」给成实心强调色（只是高亮提醒，不是可用性区分）。
        val solid = row.state != STATE_ONLINE && action == POWER_WAKE

        val accent = when (action) {
            POWER_WAKE -> tone.online
            POWER_SLEEP -> palette.accent
            POWER_REBOOT -> Color(0xFF2563EB)
            else -> palette.bad
        }
        val shape = RoundedCornerShape(11.dp)
        val background = if (solid) tone.online else palette.card
        val contentColor = if (solid) Color.White else palette.fg
        // 提示小字：实心按钮上跟内容色半透明，普通卡片用次级灰（与悬浮球一致）
        val hintColor = if (solid) contentColor.copy(alpha = 0.75f) else palette.fgFaint

        Box(
            contentAlignment = Alignment.Center,
            modifier = modifier
                .height(54.dp)
                .clip(shape)
                .background(background)
                .then(
                    if (!solid) Modifier.border(1.dp, palette.line, shape) else Modifier
                )
                .clickable { performPower(row.profile, action) },
        ) {
            if (working) {
                CircularProgressIndicator(
                    modifier = Modifier.size(14.dp),
                    strokeWidth = 2.dp,
                    color = accent,
                )
            } else {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
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
                    if (hint.isNotEmpty()) {
                        Text(
                            hint,
                            fontSize = 9.sp,
                            color = hintColor,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.padding(top = 1.dp),
                        )
                    }
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

    /**
     * GRUB 条目文字的格式档位（与 PC 端 ⋮ 菜单同一套五档）。
     * 「跟随电脑」= 用 /api/grub 响应里的 listFormat（服务端没给就退极简）；
     * 点了具体档位就固定用手机自己的选择并记住（anyboot_ui/grubListFormat）。
     */
    @Composable
    private fun grubFormatChips(group: BootGroup) {
        val palette = LocalPalette.current
        val effective = grubEffectiveFormat(group)
        val choices = listOf(
            "" to "跟随电脑",
            "minimal" to "极简",
            "simple" to "简易",
            "medium" to "中等",
            "raw" to "原始",
            "detailed" to "复杂",
        )
        Column {
            Text(
                "条目文字格式（当前：${if (grubFmt.isEmpty()) "跟随电脑 · $effective" else effective}）",
                fontSize = 11.sp,
                color = palette.fgFaint,
            )
            Spacer(Modifier.height(6.dp))
            Row(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.horizontalScroll(rememberScrollState()),
            ) {
                choices.forEach { (value, name) ->
                    val selected = grubFmt == value
                    Text(
                        name,
                        fontSize = 11.5.sp,
                        fontWeight = if (selected) FontWeight.Bold else FontWeight.Normal,
                        color = if (selected) Color.White else palette.fgDim,
                        modifier = Modifier
                            .clip(RoundedCornerShape(8.dp))
                            .background(
                                if (selected) palette.accent else palette.elevate
                            )
                            .clickable {
                                grubFmt = value
                                getSharedPreferences(UI_PREFS, MODE_PRIVATE)
                                    .edit().putString("grubListFormat", value).apply()
                            }
                            .padding(horizontal = 9.dp, vertical = 5.dp),
                    )
                }
            }
            Spacer(Modifier.height(4.dp))
        }
    }

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
                        if (request.kind == KIND_GRUB) {
                            grubFormatChips(group)
                            Spacer(Modifier.height(6.dp))
                        }
                        // rEFInd 直接列条目（与 PC 端一致）——不能走 GRUB 拼装：rEFInd 只有
                        // label 没有 grubName/kernels，塞进去会被折叠成一行「· N 个变体」
                        val rows = if (request.kind == KIND_REFIND) {
                            group.options.map { GrubRow(it, it.label, false) }
                        } else {
                            grubDisplayRows(group)
                        }
                        rows.forEach { row ->
                            // 组头行（「▸ 组名（N 项…）」）不可选，渲染成小标题 —— 托盘同构
                            if (row.isHeader) {
                                Text(
                                    row.label,
                                    fontSize = 11.sp,
                                    color = palette.fgFaint,
                                    modifier = Modifier.padding(top = 7.dp, bottom = 2.dp, start = 2.dp),
                                )
                                return@forEach
                            }
                            val option = row.option ?: return@forEach
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
                                    row.label,
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
        // 搜索要能命中每一个配置好的地址和 MAC，不只是第一个
        val extras = StringBuilder()
        val n = row.profile.addressCount()
        for (i in 0 until n) {
            val addr = row.profile.hostAt(i)
            extras.append(' ').append(addr)
            row.profile.macsForHost(addr).forEach { extras.append(' ').append(it) }
        }
        val haystack = (row.name + " " + row.group + " " + row.ip + " " + row.mac + extras)
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
        // 当前连接：探测赢家确实是这台电脑的远程地址 → 按地址形态细分（Tailscale 直连 / 远程隧道）；
        // 否则配了跳板机 → 跳板机·名字；都没配 → 直连
        val remoteOrigin = if (profile.hasRemote()) {
            ServerProfile.normalizeRemoteOrigin(profile.remoteAddr)
        } else {
            ""
        }
        val remoteBare = remoteOrigin.removePrefix("https://").removePrefix("http://")
        val usingRemote = info != null && info.online &&
            remoteBare.isNotEmpty() && info.addr == remoteBare
        val conn = when {
            usingRemote && ServerProfile.isTailscaleAddr(remoteOrigin) -> "Tailscale 直连"
            usingRemote -> "远程隧道"
            ServerProfile.text(profile.jumpServerId).isNotEmpty() -> {
                val jump = JumpStore.findJump(JumpStore.loadJumps(this), profile.jumpServerId)
                "跳板机 · " + (if (ServerProfile.text(jump?.name).isNotEmpty()) {
                    jump!!.name
                } else {
                    jump?.host ?: "已删除"
                })
            }
            else -> "直连"
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
            conn = conn,
            connRemote = usingRemote,
        )
    }

    private fun displayAddress(profile: ServerProfile): String {
        val value = ServerProfile.normalizeHost(profile.host())
        if (value.isEmpty()) return "(未填地址)"
        val suffix = ":" + ServerProfile.DEFAULT_PORT
        return if (value.endsWith(suffix)) value.substring(0, value.length - suffix.length) else value
    }

    /** 单个地址的展示形式：归一化后去掉默认端口尾巴（和 displayAddress 同一套规则） */
    private fun displayAddrOf(raw: String): String {
        val value = ServerProfile.normalizeHost(raw)
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
                // 后台线程里任何未捕获异常都会直接崩进程（用户看到的就是「闪退」），
                // 所以每台电脑单独兜住：探挂了只标离线，不影响其它电脑和整页
                val info = try {
                    fetchBootInfo(profile)
                } catch (error: Throwable) {
                    offlineInfo("", error.message ?: "探测失败")
                }
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

    // ---- 定时自动探测（与电脑页心跳同一节奏：10 秒一轮） ------------------

    /** 页面在前台才跑；onPause 停表，回前台 onResume 先补探一轮再排下去 */
    private var autoProbeOn = false
    private val autoProbeTick = object : Runnable {
        override fun run() {
            if (!autoProbeOn) return
            refreshAllQuiet()
            handler.postDelayed(this, AUTO_PROBE_INTERVAL_MS)
        }
    }

    private fun startAutoProbe() {
        if (autoProbeOn) return
        autoProbeOn = true
        handler.postDelayed(autoProbeTick, AUTO_PROBE_INTERVAL_MS)
    }

    private fun stopAutoProbe() {
        autoProbeOn = false
        handler.removeCallbacks(autoProbeTick)
    }

    /** 定时自动探测的静默版：不弹 toast、不清动作过渡态；手动刷新在跑就跳过本轮 */
    private fun refreshAllQuiet() {
        if (refreshing || servers.isEmpty()) return
        val snapshot = servers.toList()
        refreshing = true
        Thread({
            for (profile in snapshot) {
                // 后台线程里任何未捕获异常都会直接崩进程 —— 每台电脑单独兜住
                val info = try {
                    fetchBootInfo(profile)
                } catch (error: Throwable) {
                    offlineInfo("", error.message ?: "探测失败")
                }
                runOnUiThread { boots[profile.id] = info }
            }
            runOnUiThread { refreshing = false }
        }, "anyboot-autoprobe").start()
    }

    /** 点卡片选中：同一时间只有一台「当前电脑」，与电脑页的 activeId 是同一份 */
    private fun selectComputer(id: String) {
        if (selectedId == id) return
        selectedId = id
        ServerStore.setActiveId(this, id)
    }

    private fun reloadBoot(profile: ServerProfile) {
        val id = profile.id
        if (busy[id] != null) return
        busy[id] = "boot:load"
        Thread({
            val info = try {
                fetchBootInfo(profile)
            } catch (error: Throwable) {
                offlineInfo("", error.message ?: "读取失败")
            }
            runOnUiThread {
                boots[id] = info
                busy.remove(id)
            }
        }, "anyboot-boot").start()
    }

    /** 后台线程：探活 + 读两套引导项（一边失败不影响另一边） */
    private fun fetchBootInfo(profile: ServerProfile): BootInfo {
        val choice = chooseEndpoint(profile) ?: return offlineInfo("", "这台电脑还没有可用地址")
        if (!choice.alive) {
            return offlineInfo(choice.displayAddr(), "")
        }
        val endpoint = choice.url
        val token = choice.token

        val errors = mutableListOf<String>()
        val refind = try {
            parseRefind(httpJson(endpoint, token, "/api/status"))
        } catch (error: Exception) {
            val message = error.message ?: "读取失败"
            errors.add(message)
            BootGroup(
                kind = KIND_REFIND, title = "rEFInd", available = false,
                current = "", next = "", saved = "", note = message, options = emptyList(),
            )
        }
        val grub = try {
            parseGrub(httpJson(endpoint, token, "/api/grub"))
        } catch (error: Exception) {
            val message = error.message ?: "读取失败"
            errors.add(message)
            BootGroup(
                kind = KIND_GRUB, title = "GRUB", available = false,
                current = "", next = "", saved = "", note = message, options = emptyList(),
            )
        }
        return BootInfo(
            ok = errors.size < 2,
            online = true,
            addr = choice.displayAddr(),
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
            refind = BootGroup(
                kind = KIND_REFIND, title = "rEFInd", available = false,
                current = "", next = "", saved = "", note = note, options = emptyList(),
            ),
            grub = BootGroup(
                kind = KIND_GRUB, title = "GRUB", available = false,
                current = "", next = "", saved = "", note = note, options = emptyList(),
            ),
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

    /** 选点结果：url 是完整 origin（http:// 或 https:// 开头），token 配套凭据 */
    private class EndpointChoice(val url: String, val token: String, val alive: Boolean) {
        /** 卡片/离线文案里显示用的地址（不带 scheme） */
        fun displayAddr(): String =
            url.removePrefix("https://").removePrefix("http://")
    }

    /**
     * 选点：局域网（或跳板机隧道）优先 —— 探通就用；没通且配了远程接入时探
     * 隧道兜底（出门场景）。全都没通时仍把局域网端点带回去（alive=false），
     * 让离线文案与地址显示维持原样。
     * 返回 null = 连一个能试的地址都没有。
     */
    private fun chooseEndpoint(profile: ServerProfile): EndpointChoice? {
        val remote = if (profile.hasRemote()) {
            ServerProfile.normalizeRemoteOrigin(profile.remoteAddr)
        } else {
            ""
        }
        // 远程优先（编辑里选的）：先探隧道，通了直接走；隧道挂了再退局域网链路。
        // 跳板机本身就是远程路径，不叠加隧道优先。
        if (profile.isRemoteFirst() && profile.jumpServerId.isEmpty() && remote.isNotEmpty()
            && HealthProbe.pingRemote(remote, REMOTE_PROBE_TIMEOUT_MS) > 0
        ) {
            return EndpointChoice(remote, ServerProfile.text(profile.remoteToken), alive = true)
        }
        val lan = try {
            endpointOf(profile)
        } catch (error: Exception) {
            ""
        }
        val lanToken = ServerProfile.text(profile.token)
        if (lan.isNotEmpty() && HealthProbe.ping(lan, PROBE_TIMEOUT_MS) > 0) {
            return EndpointChoice("http://" + lan, lanToken, alive = true)
        }
        if (remote.isNotEmpty() && HealthProbe.pingRemote(remote, REMOTE_PROBE_TIMEOUT_MS) > 0) {
            return EndpointChoice(remote, ServerProfile.text(profile.remoteToken), alive = true)
        }
        if (lan.isEmpty() && remote.isEmpty()) return null
        return EndpointChoice(if (lan.isNotEmpty()) "http://" + lan else remote, lanToken, alive = false)
    }

    private fun prompt(): SshSessionManager.Prompt = object : SshSessionManager.Prompt {
        override fun confirm(title: String, message: String): Boolean = bus.confirm(title, message)
    }

    /** 对电脑发一条 `GET {path}?token=…`，返回解析后的 JSON（语义对齐 PC 端 remote_http_json）。
     *  endpoint 接受裸 host:port（补 http://）或完整 origin（远程隧道是 https://…）。 */
    private fun httpJson(endpoint: String, token: String, path: String): JSONObject {
        val trimmed = ServerProfile.text(token)
        val query = if (trimmed.isEmpty()) "" else {
            (if (path.contains('?')) "&" else "?") + "token=" +
                URLEncoder.encode(trimmed, "UTF-8")
        }
        val base = if (endpoint.startsWith("http://") || endpoint.startsWith("https://")) {
            endpoint
        } else {
            "http://" + endpoint
        }
        val connection = URL(base + path + query)
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
                val system = optStr(item, "system").trim()
                if (system.isEmpty()) continue
                val label = optStr(item, "displayName").trim().ifEmpty { system }
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

    /** `/api/grub` 的 entries → GRUB 可选组的**原料**（拼装在 [grubDisplayRows]） */
    private fun parseGrub(json: JSONObject): BootGroup {
        // nextEntry/savedEntry 也可能是 JSON null（服务端老版本序列化习惯），
        // 裸 optString 会拿到字符串 "null" —— 统一走 optStr 兜底
        val next = optStr(json, "nextEntry").trim()
        val saved = optStr(json, "savedEntry").trim()
        val installed = json.optBoolean("installed", false)
        // 服务端自己的格式偏好（老版本没这个字段 → 空，前端退 minimal）
        val serverFormat = optStr(json, "listFormat")
        // submenu 的内部 id → 组名（老版本没有 groups，映射为空 → 组头退回内部 id）
        val groupNames = HashMap<String, String>()
        json.optJSONArray("groups")?.let { groups ->
            for (index in 0 until groups.length()) {
                val item = groups.optJSONObject(index) ?: continue
                val id = optStr(item, "id")
                val name = optStr(item, "name")
                if (id.isNotEmpty() && name.isNotEmpty()) groupNames[id] = name
            }
        }
        val options = mutableListOf<BootOption>()
        val entries = json.optJSONArray("entries")
        if (entries != null) {
            for (index in 0 until entries.length()) {
                val item = entries.optJSONObject(index) ?: continue
                val selector = optStr(item, "selector").trim()
                if (selector.isEmpty()) continue
                val name = optStr(item, "name").trim()
                val submenu = optStr(item, "submenu").trim()
                val klass = optStr(item, "class")
                val chainloader = item.optBoolean("chainloader", false)
                val kernels = mutableListOf<String>()
                item.optJSONArray("kernels")?.let { list ->
                    for (k in 0 until list.length()) {
                        val kernel = list.optString(k, "").trim()
                        if (kernel.isNotEmpty()) kernels.add(kernel)
                    }
                }
                options.add(
                    BootOption(
                        value = selector,
                        label = "",
                        kind = grubClass(name, klass, chainloader),
                        grubName = name,
                        grubKernels = kernels,
                        grubSubmenuId = submenu,
                        grubVariants = maxOf(1, item.optInt("variants", 1)),
                        grubChainloader = chainloader,
                        grubClassRaw = klass.trim(),
                    )
                )
            }
        }
        val note = if (options.isEmpty()) {
            optStr(json, "error").trim().ifEmpty { "这台电脑没部署 GRUB（或还没同步 grub.cfg）" }
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
            serverFormat = serverFormat,
            grubGroupNames = groupNames,
            grubEfiDir = optStr(json, "efiDir").trim(),
            grubPartuuid = optStr(json, "partuuid").trim(),
            options = options,
        )
    }

    /**
     * Android org.json 的经典坑：JSON 值是 null 时 `optString(key, "")` 返回的是
     * 字符串 "null" 而不是 fallback（NULL.toString() == "null"）。服务端把
     * `submenu` / `class` / `error` 序列化成 null，直接 optString 会在界面上拼出
     * 「null · Arch Linux」。这里统一兜住：NULL / 缺失都返回空串。
     */
    private fun optStr(obj: JSONObject, key: String): String {
        if (!obj.has(key) || obj.isNull(key)) return ""
        return obj.optString(key, "")
    }

    /** 这次渲染该用哪一档：手机自选优先，没选过就跟随服务端，再退 minimal */
    private fun grubEffectiveFormat(group: BootGroup): String =
        grubFmt.ifEmpty { group.serverFormat.ifEmpty { "minimal" } }

    // ---- GRUB 拼装：与 PC 托盘菜单（menu_model::grub_menu_nodes）同一条规则 ----

    /** 系统图标（与托盘 grub_os_icon 同一规则，不猜）：windows→🪟 mac→🍎 linux→🐧 */
    private fun grubOsIcon(text: String): String {
        val lowered = text.lowercase(Locale.US)
        return when {
            lowered.contains("windows") -> "🪟 "
            lowered.contains("mac") || lowered.contains("apple") -> "🍎 "
            lowered.contains("linux") -> "🐧 "
            else -> ""
        }
    }

    private fun grubStripKernel(name: String): String =
        name.removePrefix("vmlinuz-").removePrefix("linux-")

    /** 内核名：优先 linux 行解析出的文件名，空白按「没有」算，退回标题原文 */
    private fun grubKernelOf(option: BootOption): String =
        option.grubKernels.firstOrNull { it.isNotBlank() } ?: option.grubName

    /** 行文字随格式变（与托盘 grub_row_label 同一规则） */
    private fun grubRowLabel(format: String, option: BootOption): String = when (format) {
        "minimal", "simple" -> grubStripKernel(grubKernelOf(option))
        "medium" -> grubKernelOf(option)
        else -> option.grubName
    }

    /** 非原始格式按渲染结果数变体；原始格式不折叠（与托盘 grub_collapse 一致） */
    private fun grubCollapse(format: String, options: List<BootOption>): Map<String, Int> {
        if (format == "raw") return emptyMap()
        val counts = HashMap<String, Int>()
        options.forEach { counts.merge(grubRowLabel(format, it), 1, Int::plus) }
        return counts
    }

    /**
     * 拼出下拉里的展示行 —— 与 PC 托盘「GRUB 启动项」逐条对齐：
     * * 当前选中项置顶（组内子项带「组名 › 」前缀），原位照常保留；
     * * 每个 submenu 一行组头「▸ 图标 组名（N 项[ · ESP 位置]）」，位置按格式带不带；
     * * 子项「↳ 」缩进；顶层与各组各有一套折叠状态（非原始格式折叠同名变体）；
     * * 原始/复杂在行尾附内核名（或「转交引导」/class）。
     */
    private fun grubDisplayRows(group: BootGroup): List<GrubRow> {
        val options = group.options
        if (options.isEmpty()) return emptyList()
        val format = grubEffectiveFormat(group)
        val names = group.grubGroupNames
        val rows = mutableListOf<GrubRow>()

        fun push(option: BootOption, top: Boolean, prefix: String, counts: Map<String, Int>, emitted: MutableSet<String>) {
            val label = grubRowLabel(format, option)
            val parts = mutableListOf<String>()
            if (format == "raw") {
                // 原始格式不折叠；变体数用解析期并进的 grubVariants
                if (option.grubVariants > 1) parts.add("${option.grubVariants} 个变体")
            } else {
                if (!emitted.add(label)) return
                val variants = counts[label] ?: 1
                if (variants > 1) parts.add("$variants 个变体")
            }
            if (format == "raw" || format == "detailed") {
                val tag = when {
                    option.grubKernels.any { it.isNotBlank() } ->
                        option.grubKernels.first { it.isNotBlank() }
                    option.grubChainloader -> "转交引导"
                    else -> option.grubClassRaw
                }
                if (tag.isNotBlank()) parts.add(tag)
            }
            val suffix = if (parts.isEmpty()) "" else " · " + parts.joinToString(" · ")
            val icon = grubOsIcon(prefix + option.grubName)
            val arrow = if (top) "" else "↳ "
            rows.add(GrubRow(option, "$arrow$icon$prefix$label$suffix", false))
        }

        // 置顶当前选中项（组内子项补「组名 › 」前缀）
        options.firstOrNull { it.value == group.current }?.let { pinned ->
            val prefix = names[pinned.grubSubmenuId]?.takeIf { it.isNotBlank() }?.let { "$it › " } ?: ""
            push(pinned, top = true, prefix = prefix, counts = emptyMap(), emitted = mutableSetOf())
        }

        // 组头附加的 ESP 位置（简易/中等/复杂带，极简/原始不带）
        val location = if (format == "minimal" || format == "raw") {
            ""
        } else {
            val parts = mutableListOf<String>()
            if (group.grubEfiDir.isNotBlank()) parts.add("位于${group.grubEfiDir}")
            if (group.grubPartuuid.isNotBlank()) parts.add("UUID=${group.grubPartuuid}")
            if (parts.isEmpty()) "" else " · " + parts.joinToString(" ")
        }

        val children = options.filter { it.grubSubmenuId.isNotEmpty() }
        val topCounts = grubCollapse(format, options.filter { it.grubSubmenuId.isEmpty() })
        val topEmitted = mutableSetOf<String>()
        val emittedGroups = mutableSetOf<String>()
        options.forEach { option ->
            val submenuId = option.grubSubmenuId
            if (submenuId.isEmpty()) {
                push(option, top = true, prefix = "", counts = topCounts, emitted = topEmitted)
                return@forEach
            }
            if (!emittedGroups.add(submenuId)) return@forEach
            val title = names[submenuId]?.takeIf { it.isNotBlank() } ?: submenuId
            val count = children.count { it.grubSubmenuId == submenuId }
            rows.add(GrubRow(null, "▸ ${grubOsIcon(title)}$title（$count 项$location）", true))
            val groupChildren = children.filter { it.grubSubmenuId == submenuId }
            val counts = grubCollapse(format, groupChildren)
            val emitted = mutableSetOf<String>()
            groupChildren.forEach { push(it, top = false, prefix = "", counts, emitted) }
        }
        return rows
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
                val choice = chooseEndpoint(profile)
                    ?: throw Exception("这台电脑还没有可用地址")
                if (!choice.alive) throw Exception("电脑离线（局域网与远程都没通）")
                val path = if (kind == KIND_REFIND) {
                    "/api/switch?system=" + URLEncoder.encode(value, "UTF-8")
                } else {
                    "/api/grub-switch?entry=" + URLEncoder.encode(value, "UTF-8")
                }
                val json = httpJson(choice.url, choice.token, path)
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
        // 不做任何拦截：四个电源键任何时候都能按（用户定案），连按就是连发。
        busy[id] = "power:$action"
        Thread({
            val outcome = runCatching { powerMessage(profile, action) }
            runOnUiThread {
                // 只清自己这把钥匙：期间用户可能又点过别的操作（boot / 另一个电源动作）
                if (busy[id] == "power:$action") busy.remove(id)
                outcome.onSuccess { message ->
                    toast(message)
                    transients[id] = when (action) {
                        POWER_WAKE -> STATE_WAKING
                        POWER_SLEEP -> STATE_SLEEPING
                        POWER_SHUTDOWN -> STATE_SHUTTING
                        else -> STATE_REBOOTING
                    }
                    // 过渡态只在动作进行时存在：到点后必须让位给真实探测结果，
                    // 否则「唤醒」之后这张卡会永远停在「在线」，探测结果再也盖不住。
                    handler.postDelayed({
                        when (action) {
                            POWER_WAKE, POWER_REBOOT -> {
                                transients.remove(id)
                                reloadBoot(profile)
                            }
                            // 睡眠是持续状态（睡着的机器 ping 不通），留着它才能显示「已睡眠」
                            POWER_SLEEP -> transients[id] = STATE_SLEEPING
                            // 关机后确实离线：清掉过渡态，直接标离线
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
            val routed = WakeOnLan.sendRoutes(this@ComputerListActivity, profile, action, prompt)
            return when {
                routed != null -> routed
                ServerProfile.POWER_WAKE == action -> WakeOnLan.wake(this@ComputerListActivity, profile, prompt)
                // 广播包兜底的 HTTP：同样走选点（出门时广播包必然失败，HTTP 得能走隧道）
                else -> {
                    val (address, token) = powerTarget(profile)
                    PowerOps.send(address, token, action)
                }
            }
        }

        fun http(): String {
            val (address, token) = powerTarget(profile)
            return PowerOps.send(address, token, action)
        }

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

    /**
     * 电源 HTTP 的「地址 + 凭据」：局域网探通用局域网；没通且配了远程接入就探
     * 隧道兜底（出门时重启/关机/睡眠靠这条，唤醒的魔术包走不了隧道、照旧在
     * 广播包分支失败并给出原因）。全没通时退回局域网地址 —— 让失败文案与原
     * 行为一致（连接失败，而不是编造一条远程失败）。
     */
    private fun powerTarget(profile: ServerProfile): Pair<String, String> {
        val choice = chooseEndpoint(profile) ?: throw Exception("这台电脑还没有可用地址")
        return if (choice.alive) {
            Pair(choice.url, choice.token)
        } else {
            Pair("http://" + powerAddress(profile), ServerProfile.text(profile.token))
        }
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
        /** 当前连接标注：直连 / 跳板机·xxx / 远程隧道 / Tailscale 直连（远程只在探测赢家确实是它时） */
        val conn: String,
        /** 当前连接是否正在走远程隧道（chip 配色用） */
        val connRemote: Boolean,
    )

    /**
     * 一个可选的引导项。rEFInd 直接用 `label`；GRUB 只存原料（标题/内核/组 id/
     * 变体数），界面上的文字按托盘菜单同款规则在 [grubDisplayRows] 里现算，
     * 切格式档即时生效。
     */
    private data class BootOption(
        val value: String,
        val label: String,
        val kind: String,
        // —— GRUB 渲染原料（rEFInd 恒为空/默认）——
        val grubName: String = "",
        val grubKernels: List<String> = emptyList(),
        /** 父级 submenu 的内部 id（顶层条目为空） */
        val grubSubmenuId: String = "",
        /** 解析期折叠进来的同名变体数（老服务端不发，恒为 1） */
        val grubVariants: Int = 1,
        val grubChainloader: Boolean = false,
        /** `--class` 原文（原始/复杂档的行尾附注用） */
        val grubClassRaw: String = "",
    )

    private data class BootGroup(
        val kind: String,
        val title: String,
        val available: Boolean,
        val current: String,
        val next: String,
        val saved: String,
        val note: String,
        /** 服务端自己的格式偏好（/api/grub 的 listFormat，老版本为空） */
        val serverFormat: String = "",
        /** submenu 内部 id → 组名（老服务端没有 groups 时为空映射） */
        val grubGroupNames: Map<String, String> = emptyMap(),
        /** ESP 位置（组头附注用，简易/中等/复杂档带） */
        val grubEfiDir: String = "",
        val grubPartuuid: String = "",
        val options: List<BootOption>,
    )

    /** 一行展示项：[option] 为 null 时是组头（不可点），否则是可选项 + 拼好的文字 */
    private data class GrubRow(val option: BootOption?, val label: String, val isHeader: Boolean)

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
        const val UI_PREFS = "anyboot_ui"

        /** 上次停留页（entryPage）：与 MainActivity 同一份偏好 —— computers = 电脑列表 */
        const val PREF_ENTRY = "entryPage"

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
        /** 远程隧道探活：公网 + TLS 握手比局域网慢得多，给宽容些 */
        const val REMOTE_PROBE_TIMEOUT_MS = 8000
        const val CONNECT_TIMEOUT_MS = 4000
        const val READ_TIMEOUT_MS = 8000

        /** 定时自动探测间隔：与电脑页心跳同一节奏（10 秒一轮） */
        const val AUTO_PROBE_INTERVAL_MS = 10_000L

        val HELP_LINES = listOf(
            "唤醒：发标准魔术包（UDP 9）。睡眠 / 重启 / 关机按每台电脑「电源路由」里配的通道发送，" +
                "可以是局域网广播包，也可以走它自己的 AnyBoot 接口",
            "四个电源键任何时候都能按，不会因为在线/离线变灰；关机与重启立即执行，不会二次确认，请先保存工作",
            "引导项（rEFInd / GRUB）在下次重启时生效，可点「撤销」恢复成原来那项",
            "电脑离线时读不到引导项",
            // 补上 PC 端 cqHelp 的第 5 条：没有它，用户会把「未检测到」当成读取失败
            "显示「未检测到」= 这台电脑没部署对应的引导管理器（rEFInd / GRUB），不是读取失败",
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
