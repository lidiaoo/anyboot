package com.anyboot.remote

import android.annotation.SuppressLint
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.os.Bundle
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Info
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

/**
 * AnyBoot 远程客户端外壳（多服务端版，Jetpack Compose / Material 3 重写）。
 *
 * 它本身不实现业务：每台电脑一个标签，每个标签就是一个 WebView，加载那台电脑上
 * AnyBoot 内置 HTTP 服务托管的前端页面。业务逻辑（多地址自愈、跳板机隧道、
 * 心跳、同一台电脑归并）与旧 View 实现逐段平移，界面层换成 Compose。
 */
class MainActivity : ComponentActivity() {

    companion object {
        private const val STATUS_UNKNOWN = 0
        private const val STATUS_LOADING = 1
        private const val STATUS_OK = 2
        private const val STATUS_FAIL = 3
        /** 界面偏好（自动重载间隔这类小开关）的 SharedPreferences 名 */
        private const val UI_PREFS = "anyboot_ui"

        /** 上次停留页（entryPage）：computers = 电脑列表，home = 电脑页 —— launcher 重新进入按它路由 */
        private const val PREF_ENTRY = "entryPage"

        /**
         * 启动时直接打开哪台电脑的页面（[ServerProfile.id]）。
         *
         * 电脑列表页的「打开页面」用它把用户送到对应标签；不传就沿用上次的 activeId。
         */
        const val EXTRA_SERVER_ID = "anyboot.server.id"
    }

    /** 一台电脑的运行时状态（WebView 懒创建：没点过的标签不占内存） */
    private class Tab(var profile: ServerProfile) {
        var view: WebView? = null
        var status = STATUS_UNKNOWN
        var mainFailed = false
        var started = false
        var addressIndex = 0
        val tried = ArrayList<String>()
        var effectiveAddress = ""
        /** 当前加载的是远程接入地址（局域网全挂后的兜底；本轮内失败不再重试） */
        var usingRemote = false
        /** 本轮连接尝试里远程通道已经试过一次（防远程优先模式 LAN↔隧道乒乓） */
        var triedRemote = false
    }

    data class ErrorInfo(val label: String, val addressSummary: String, val detail: String)
    private data class EditRequest(val profile: ServerProfile?, val firstRun: Boolean)

    private val bus = ConfirmBus()

    // ---- 可观察 UI 状态 ----
    private val servers = mutableStateListOf<ServerProfile>()
    private var chipTick by mutableIntStateOf(0)   // tabs 状态有变时自增，驱动标签条重绘
    private var activeId by mutableStateOf("")
    private var connectAll by mutableStateOf(false)
    private var transport by mutableStateOf("auto")
    private var loading by mutableStateOf(false)
    private var errorInfo by mutableStateOf<ErrorInfo?>(null)
    private var drawerOpen by mutableStateOf(false)
    private var editRequest by mutableStateOf<EditRequest?>(null)
    private var deleteServerTarget by mutableStateOf<ServerProfile?>(null)
    private var serverListOpen by mutableStateOf(false)
    private var transportOpen by mutableStateOf(false)

    // ---- 电源悬浮球 ----
    /** 有电源请求在飞（球上转圈，防止连点） */
    private var powerBusy by mutableStateOf(false)
    /** 睡眠旁的 ? 说明 */
    private var sleepHelpOpen by mutableStateOf(false)

    // ---- 连不上时定时自动重载 ----
    /** 间隔秒数；0 = 永不。默认 5 秒，设置入口在 ⋮ 溢出菜单 */
    private var failReloadSec by mutableStateOf(5)
    private var failReloadOpen by mutableStateOf(false)

    // ---- 非 UI 状态（WebView 指令式管理，同旧实现） ----
    private val tabs = LinkedHashMap<String, Tab>()
    private var container: FrameLayout? = null
    private var heartbeat: HeartbeatManager? = null
    private var firstRunPending = false

    private val tunnelResolver = HeartbeatManager.TunnelResolver { profile ->
        val jump = JumpStore.findJump(JumpStore.loadJumps(this), profile.jumpServerId)
            ?: throw Exception("跳板机不存在（可能已删除）")
        SshSessionManager.shared().tunnelAddress(this, jump, profile.host()) { title, message ->
            bus.confirm(title, message)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Ui.applySystemBars(this)

        val loaded = ServerStore.load(this)
        servers.clear(); servers.addAll(loaded)
        connectAll = ServerStore.connectAll(this)
        transport = ServerStore.transport(this)
        activeId = ServerStore.activeId(this)
        // 「电脑列表 → 打开页面」会带上这台电脑的 id：命中就切过去。
        // 不带这个，点了只会打开上次那台（activeId 是上次的），看着像没生效。
        val wanted = intent?.getStringExtra(EXTRA_SERVER_ID) ?: ""
        if (wanted.isNotEmpty() && servers.any { it.id == wanted }) {
            activeId = wanted
            ServerStore.setActiveId(this, wanted)
        }
        // 同一个 Intent 只消费一次：否则从电脑列表回来点第二台还是被它拽回去
        intent?.removeExtra(EXTRA_SERVER_ID)
        failReloadSec = getSharedPreferences(UI_PREFS, MODE_PRIVATE).getInt("failReloadSec", 5)
        firstRunPending = servers.isEmpty()

        // 点桌面图标（冷启动）默认先进「电脑列表」，与 PC 端远程管理窗口默认开电脑列表一致。
        // 不 finish：电脑页留在返回栈底，从电脑列表按返回仍能回到这里（老路径不变）。
        // 只认 launcher 冷启动 —— 「电脑列表 → 打开页面」带 EXTRA_SERVER_ID 进来的不拦。
        // 上次明确停在电脑页（entryPage=home）的除外：冷启动也还原到电脑页。
        if (intent?.action == Intent.ACTION_MAIN && savedInstanceState == null
            && getSharedPreferences(UI_PREFS, MODE_PRIVATE).getString(PREF_ENTRY, "") != "home"
        ) {
            startActivity(Intent(this, ComputerListActivity::class.java))
        }

        heartbeat = HeartbeatManager({ ArrayList(servers) }, object : HeartbeatManager.Listener {
            override fun onHeartbeatChanged() {
                chipTick++
            }

            override fun onWentOnline(id: String) {
                // 心跳上线自愈：还停在错误面板上的那台，一探通就自动重连
                val profile = ServerStore.find(servers, id) ?: return
                val tab = tabs[id] ?: return
                if (tab.status == STATUS_FAIL) {
                    runOnUiThread { startAttempt(ensureTab(profile)) }
                }
            }

            override fun onLanRecovered(id: String) {
                // 局域网恢复：还挂在远程隧道上的标签自动切回直连 ——
                // 重开一轮完整探测（赢家说了算），不会在局域网抖动时乒乓
                // （探测有赢家才加载；失败路径有 triedRemote 闸）。
                val tab = tabs[id] ?: return
                if (!tab.usingRemote) return
                val profile = ServerStore.find(servers, id) ?: return
                if (profile.isRemoteFirst()) return // 远程优先是用户选的，不抢
                runOnUiThread { startAttempt(ensureTab(profile)) }
            }
        })
        heartbeat?.tunnelResolver = tunnelResolver

        setContent { Root() }
    }

    /**
     * `launchMode="singleTask"` → 从电脑列表页再点「打开页面」不会走 onCreate，
     * 只把已有实例提到前台并回调这里。不处理的话第二次以后永远停在旧标签。
     */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        // 应用没退时再点桌面图标：launcher intent 会路由到 singleTask 的这里，把电脑页
        // 提到前台、电脑列表被压掉 —— 看着就是「回到电脑页」。按「上次停留页」送回去。
        if (intent.action == Intent.ACTION_MAIN
            && intent.hasCategory(Intent.CATEGORY_LAUNCHER)
            && getSharedPreferences(UI_PREFS, MODE_PRIVATE).getString(PREF_ENTRY, "") != "home"
        ) {
            startActivity(Intent(this, ComputerListActivity::class.java))
            return
        }
        val wanted = intent.getStringExtra(EXTRA_SERVER_ID) ?: return
        intent.removeExtra(EXTRA_SERVER_ID)
        if (wanted.isEmpty()) return
        if (servers.none { it.id == wanted }) return
        runOnUiThread { activate(wanted) }
    }

    // ------------------------------------------------------------------
    // 界面
    // ------------------------------------------------------------------

    @Composable
    private fun Root() {
        AppTheme {
            Box(Modifier.fillMaxSize().background(LocalPalette.current.base)) {
                    Column(Modifier.fillMaxSize()) {
                        TopBar()
                        Box(Modifier.fillMaxWidth().weight(1f)) {
                            // WebView 容器（指令式管理）
                            AndroidView(
                                modifier = Modifier.fillMaxSize(),
                                factory = { ctx -> FrameLayout(ctx).also { container = it } },
                            )
                            if (loading) {
                                LinearProgressIndicator(
                                    Modifier.fillMaxWidth().height(3.dp).align(Alignment.TopCenter)
                                )
                            }
                            errorInfo?.let { info -> ErrorPanel(info) }
                            // 悬浮刷新：半透明常驻（50%），点一下 = 重载当前页面
                            //（多地址先按探测模式比一遍，赢家直接上 —— 换 IP 也能跟上）
                            RefreshFab()
                            PowerFab()
                        }
                    }
                    if (drawerOpen) AppDrawerOverlay(DRAWER_COMPUTERS) { drawerOpen = false }
                }
                editRequest?.let { EditDialog(it) }
                deleteServerTarget?.let { DeleteServerDialog(it) }
                if (serverListOpen) ServerListDialog()
                if (transportOpen) TransportDialog()
                if (failReloadOpen) FailReloadDialog()
                if (sleepHelpOpen) SleepHelpDialog()
                ConfirmDialog(bus)
        }

        // 首次组合后：首启弹添加对话框 / 激活上次的服务端 / 连接全部
        LaunchedEffect(Unit) {
            if (firstRunPending) {
                firstRunPending = false
                editRequest = EditRequest(null, true)
                return@LaunchedEffect
            }
            if (ServerStore.find(servers, activeId) == null && servers.isNotEmpty()) {
                activeId = servers[0].id
                ServerStore.setActiveId(this@MainActivity, activeId)
            }
            activate(activeId)
            if (connectAll) connectAllServers()
        }

        // 连不上时定时自动重载：失败面板挂着就按间隔重试（0 = 永不）。
        // reloadActive 会清掉 errorInfo，本 effect 随之重启 —— 失败重现就再计时。
        LaunchedEffect(errorInfo, failReloadSec) {
            if (errorInfo == null || failReloadSec <= 0) return@LaunchedEffect
            delay(failReloadSec * 1000L)
            if (errorInfo != null) {
                toast("连接失败，自动重载中…")
                reloadActive()
            }
        }

        // WebView 自己能退就让它退（弹窗、页内路由），退不动再交给系统
        BackHandler(enabled = tabs[activeId]?.view != null) {
            val view = tabs[activeId]?.view
            if (view != null && view.canGoBack()) {
                view.goBack()
            } else {
                finish()
            }
        }
    }

    /** 顶栏：☰ + 标签条 + ＋ + ⋮ */
    @Composable
    private fun TopBar() {
        val palette = LocalPalette.current
        val listState = rememberLazyListState()
        val scope = rememberCoroutineScope()

        // 切标签时把激活的 chip 滚进可视区
        LaunchedEffect(activeId, chipTick) {
            val index = servers.indexOfFirst { it.id == activeId }
            if (index >= 0) runCatching { listState.animateScrollToItem(index) }
        }

        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .background(palette.card)
                .padding(start = 4.dp, top = 6.dp, end = 4.dp, bottom = 6.dp),
        ) {
            BarIcon("☰", "侧边栏") { drawerOpen = true }
            LazyRow(
                state = listState,
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.weight(1f),
            ) {
                items(servers) { profile -> ServerChip(profile) }
            }
            BarIcon("＋", "添加服务端") { editRequest = EditRequest(null, false) }
            OverflowMenu()
        }
    }

    /** 标签 chip：状态点 + 名称（点击切换，长按编辑） */
    @OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
    @Composable
    private fun ServerChip(profile: ServerProfile) {
        val palette = LocalPalette.current
        chipTick // 读取以订阅状态变化
        val tab = tabs[profile.id]
        val active = profile.id == activeId
        val statusColor = when {
            tab != null && tab.status == STATUS_LOADING -> palette.warn
            heartbeat?.stateOf(profile.id)?.isOnline == true -> palette.ok
            heartbeat?.stateOf(profile.id)?.isOffline == true -> palette.fgFaint
            tab != null && tab.status == STATUS_OK -> palette.ok
            tab != null && tab.status == STATUS_FAIL -> palette.bad
            else -> palette.fgFaint
        }
        val border =
            if (ServerProfile.text(profile.group).isNotEmpty()) profile.groupColor() else palette.line
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .clip(RoundedCornerShape(16.dp))
                .background(if (active) palette.accent else palette.elevate)
                .combinedClickable(
                    onClick = { activate(profile.id) },
                    onLongClick = { editRequest = EditRequest(profile, false) },
                )
                .padding(start = 10.dp, top = 5.dp, end = 12.dp, bottom = 5.dp)
                .height(22.dp),
        ) {
            Box(
                Modifier
                    .size(8.dp)
                    .clip(CircleShape)
                    .background(statusColor)
            )
            Spacer(Modifier.width(6.dp))
            Text(
                text = profile.label(),
                fontSize = 13.sp,
                color = if (active) Color.White else palette.fg,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            // 当前连接后缀：直连 / 跳板机 / 隧道（远程赢家实时变化，chipTick 驱动重绘）
            Text(
                text = when {
                    tab?.usingRemote == true -> "隧道"
                    ServerProfile.text(profile.jumpServerId).isNotEmpty() -> "跳板机"
                    else -> "直连"
                },
                fontSize = 9.sp,
                color = if (active) Color.White.copy(alpha = 0.75f) else palette.fgFaint,
                maxLines = 1,
            )
        }
    }

    /** ⋮ 溢出菜单（旧 options menu 的对应物） */
    @Composable
    private fun OverflowMenu() {
        val palette = LocalPalette.current
        var open by remember { mutableStateOf(false) }
        Box {
            BarIcon("⋮", "更多") { open = true }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                DropdownMenuItem(text = { Text("添加服务端") }, onClick = {
                    open = false; editRequest = EditRequest(null, false)
                })
                DropdownMenuItem(text = { Text("编辑当前服务端") }, onClick = {
                    open = false
                    editRequest = EditRequest(ServerStore.find(servers, activeId), false)
                })
                DropdownMenuItem(text = { Text("服务端列表") }, onClick = {
                    open = false; serverListOpen = true
                })
                DropdownMenuItem(text = { Text("重载当前页面") }, onClick = {
                    open = false; reloadActive()
                })
                DropdownMenuItem(text = { Text("全部重载") }, onClick = {
                    open = false; reloadAll()
                })
                DropdownMenuItem(
                    text = { Text(if (connectAll) "✓ 启动时连接全部服务端" else "启动时连接全部服务端") },
                    onClick = {
                        open = false
                        connectAll = !connectAll
                        ServerStore.setConnectAll(this@MainActivity, connectAll)
                        if (connectAll) connectAllServers()
                        toast(if (connectAll) "已开启：启动时把所有服务端一起连上"
                        else "已关闭：只连当前标签")
                    },
                )
                DropdownMenuItem(
                    text = {
                        Text(
                            if (failReloadSec > 0) "连不上时自动重载（每 $failReloadSec 秒）"
                            else "连不上时自动重载（已关闭）"
                        )
                    },
                    onClick = {
                        open = false; failReloadOpen = true
                    },
                )
                DropdownMenuItem(text = { Text("传输方式（${transportLabel()}）") }, onClick = {
                    open = false; transportOpen = true
                })
            }
        }
    }

    /** 连接失败面板 */
    @Composable
    private fun ErrorPanel(info: ErrorInfo) {
        val palette = LocalPalette.current
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier
                .fillMaxSize()
                .background(palette.card)
                .verticalScroll(rememberScrollState())
                .padding(24.dp),
        ) {
            Spacer(Modifier.weight(1f))
            Text("连不上服务端", fontSize = 20.sp, color = palette.fg, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(12.dp))
            Text(
                text = info.detail,
                fontSize = 14.sp,
                color = palette.fgDim,
                modifier = Modifier.padding(vertical = 12.dp),
            )
            // 连不上时最常见的三种原因都写出来：服务没跑 / 端口不一致 / 端口被占用
            Text(
                text = "顺手检查这几点：\n" +
                    "· 目标电脑的 AnyBoot 是否在跑，设置里「HTTP 服务」是否显示正在监听；\n" +
                    "· 这里填的地址端口，和目标机设置里的端口是否一致；\n" +
                    "· 那个端口是否被别的程序占用 —— 被占用时目标机的服务根本起不来；\n" +
                    "· 目标机防火墙是否放行了这个端口。",
                fontSize = 12.sp,
                color = palette.fgFaint,
                lineHeight = 17.sp,
                modifier = Modifier.padding(bottom = 12.dp),
            )
            Text(
                text = if (failReloadSec > 0) "每 $failReloadSec 秒自动重试（⋮ 里可改）"
                else "自动重试已关闭（⋮ 里可改）",
                fontSize = 11.sp,
                color = palette.fgFaint,
            )
            Spacer(Modifier.height(10.dp))
            Button(onClick = { reloadActive() }) { Text("重试") }
            Spacer(Modifier.height(8.dp))
            OutlinedButton(onClick = {
                editRequest = EditRequest(ServerStore.find(servers, activeId), false)
            }) { Text("修改这个服务端") }
            Spacer(Modifier.weight(1f))
        }
    }

    // ------------------------------------------------------------------
    // 电源悬浮球（重启 / 关机 / 睡眠 / 唤醒）
    // ------------------------------------------------------------------

    /**
     * 右下角悬浮球：点一下向上展开四个电源操作。
     *
     * <p>每个动作先看「局域网电源路由」：配了就发 UDP 包给目标机上的 sol（按端口执行），
     * 没配才回落 —— 唤醒走标准 WOL，重启/关机/睡眠走电脑端 {@code /api/power}。
     */
    @Composable
    private fun PowerFab() {
        val palette = LocalPalette.current
        val profile = ServerStore.find(servers, activeId) ?: return
        var expanded by remember { mutableStateOf(false) }
        chipTick // 读取以订阅状态变化（远程赢家变化时提示小字的传输标识跟着重算）
        val tab = tabs[profile.id]

        Box(
            // 底部抬高 34dp：别盖住 WebView 页面的状态条/通知（用户定案 2026-10-23）
            modifier = Modifier.fillMaxSize().padding(start = 14.dp, end = 14.dp, bottom = 48.dp),
            contentAlignment = Alignment.BottomEnd,
        ) {
            Column(
                horizontalAlignment = Alignment.End,
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                if (expanded) {
                    // 说明条：这一列按钮走的是局域网，能执行全靠目标机上的 sol
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier
                            .clip(RoundedCornerShape(10.dp))
                            .background(palette.card)
                            .clickable { sleepHelpOpen = true }
                            .padding(start = 10.dp, top = 5.dp, end = 5.dp, bottom = 5.dp),
                    ) {
                        Text("电源走局域网 · 需 sol", fontSize = 10.sp, color = palette.fgFaint)
                        Spacer(Modifier.width(4.dp))
                        Box(
                            contentAlignment = Alignment.Center,
                            modifier = Modifier
                                .size(18.dp)
                                .clip(CircleShape)
                                .background(palette.elevate),
                        ) {
                            Text("?", fontSize = 10.sp, color = palette.fgDim)
                        }
                    }
                    PowerChip(
                        "唤醒",
                        powerHint(
                            profile, ServerProfile.POWER_WAKE,
                            if (profile.hasMacs()) "MAC 魔术包" else "未配 MAC",
                            tab?.usingRemote == true,
                        ),
                    ) {
                        expanded = false
                        runPowerAction(profile, ServerProfile.POWER_WAKE)
                    }
                    PowerChip(
                        "睡眠",
                        powerHint(profile, ServerProfile.POWER_SLEEP, "挂起到内存", tab?.usingRemote == true),
                    ) {
                        expanded = false
                        runPowerAction(profile, ServerProfile.POWER_SLEEP)
                    }
                    PowerChip("关机", powerHint(profile, ServerProfile.POWER_SHUTDOWN, "", tab?.usingRemote == true)) {
                        expanded = false
                        runPowerAction(profile, ServerProfile.POWER_SHUTDOWN)
                    }
                    PowerChip("重启", powerHint(profile, ServerProfile.POWER_REBOOT, "", tab?.usingRemote == true)) {
                        expanded = false
                        runPowerAction(profile, ServerProfile.POWER_REBOOT)
                    }
                }
                Box(
                    contentAlignment = Alignment.Center,
                    modifier = Modifier
                        .size(52.dp)
                        .clip(CircleShape)
                        .background(if (powerBusy) palette.fgFaint else palette.accent)
                        .clickable { if (!powerBusy) expanded = !expanded },
                ) {
                    if (powerBusy) {
                        CircularProgressIndicator(
                            modifier = Modifier.size(22.dp),
                            color = Color.White,
                            strokeWidth = 2.dp,
                        )
                    } else {
                        Text(
                            text = if (expanded) "×" else "⏻",
                            fontSize = 20.sp,
                            color = Color.White,
                        )
                    }
                }
            }
        }
    }

    // powerHint 已抽到 PowerHint.kt（电脑列表电源键共用同一套提示文案）


    /** 悬浮球展开后的一个操作条；`help` 非空时右侧多一个 ? 圆点 */
    @Composable
    private fun PowerChip(
        label: String,
        hint: String = "",
        help: (() -> Unit)? = null,
        onClick: () -> Unit,
    ) {
        val palette = LocalPalette.current
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .clip(RoundedCornerShape(12.dp))
                .background(palette.card)
                .clickable(onClick = onClick)
                .padding(start = 14.dp, top = 7.dp, end = if (help == null) 14.dp else 6.dp, bottom = 7.dp),
        ) {
            Column {
                Text(label, fontSize = 14.sp, color = palette.fg)
                if (hint.isNotEmpty()) {
                    Text(hint, fontSize = 10.sp, color = palette.fgFaint)
                }
            }
            if (help != null) {
                Spacer(Modifier.width(8.dp))
                Box(
                    contentAlignment = Alignment.Center,
                    modifier = Modifier
                        .size(24.dp)
                        .clip(CircleShape)
                        .background(palette.elevate)
                        .clickable(onClick = help),
                ) {
                    Text("?", fontSize = 12.sp, color = palette.fgDim)
                }
            }
        }
    }

    /** 当前这台电脑「能访问到的地址」：直连就是候选地址，走跳板机就先解析成隧道端口 */
    private fun powerAddress(profile: ServerProfile): String {
        val tab = tabs[profile.id]
        val index = tab?.addressIndex ?: 0
        val address = profile.hostAt(index)
        if (address.isEmpty()) {
            throw Exception("这台电脑还没有可用地址")
        }
        val tunnel = tab?.effectiveAddress ?: ""
        if (ServerProfile.text(profile.jumpServerId).isEmpty()) {
            return if (tunnel.isNotEmpty()) tunnel else address
        }
        val jump = JumpStore.findJump(JumpStore.loadJumps(this), profile.jumpServerId)
            ?: throw Exception("跳板机不存在，请到「服务器」页检查")
        return SshSessionManager.shared().tunnelAddress(this, jump, address) { title, message ->
            bus.confirm(title, message)
        }
    }

    /**
     * 发一个电源动作。后台线程；结果用 toast 回。
     *
     * <p>优先级：这台电脑给该动作配了「局域网电源路由」→ 按路由发 UDP（目标机 sol
     * 按端口执行）；没配才回落 —— 唤醒走标准 WOL，重启/关机/睡眠走电脑端 /api/power。
     * 走路由时**不解析隧道地址**：机器可能已经关机，SSH 根本连不上。
     */
    private fun runPowerAction(profile: ServerProfile, action: String) {
        if (powerBusy) return
        powerBusy = true
        // 埋点：电源动作必须可追溯（logcat 过滤 ABPOWER）—— 记入口与结果原文
        android.util.Log.i(
            "ABPOWER",
            "runPowerAction 入口：电脑「${profile.name}」(${profile.id}) 动作 $action " +
                "模式 ${ServerProfile.routesMode(profile.routesFor(action))} 路由 ${profile.routesFor(action).size} 条",
        )
        Thread({
            try {
                val prompt = object : SshSessionManager.Prompt {
                    override fun confirm(title: String, message: String): Boolean =
                        bus.confirm(title, message)
                }
                val message = run {
                    // 聚合发送模式（与 PC 端同一套）：没配路由 = 老路，按 auto 处理
                    val routes = profile.routesFor(action)
                    val mode = ServerProfile.routesMode(routes)
                    fun packets(): String {
                        val routed = WakeOnLan.sendRoutes(this, profile, action, prompt)
                        return when {
                            routed != null -> routed
                            ServerProfile.POWER_WAKE == action -> WakeOnLan.wake(this, profile, prompt)
                            else -> PowerOps.send(powerAddress(profile), profile.token, action)
                        }
                    }
                    fun http(): String = PowerOps.send(powerAddress(profile), profile.token, action)
                    when {
                        // 没配路由：老路 —— 唤醒广播 / 其余直接调接口（都在 packets() 里）
                        routes.isEmpty() -> packets()
                        // 仅 HTTP：一个广播包都不发
                        mode == "http_only" -> "（仅 HTTP 模式）" + http()
                        // 优先 HTTP：接口通了就完事；没通退回广播包，两边错误都要让人看到
                        mode == "http_first" -> try {
                            "（优先 HTTP 模式）" + http()
                        } catch (httpError: Exception) {
                            try {
                                "HTTP 没通（${httpError.message ?: "未知错误"}），已改发广播包：${packets()}"
                            } catch (packetError: Exception) {
                                throw Exception(
                                    "HTTP 没通（${httpError.message ?: "未知错误"}）；" +
                                        "广播包也没发出去：${packetError.message ?: "未知错误"}"
                                )
                            }
                        }
                        // 混合：HTTP 与广播包同时发 —— HTTP 起独立线程，广播包走当前
                        // 线程，互不等待；最后把两边结果合并汇报（哪边成了哪边败都说明白）
                        mode == "hybrid" -> {
                            val httpOutcome = java.util.concurrent.atomic.AtomicReference<Any>(null)
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
                            if (mode != "auto") {
                                throw packetError
                            }
                            val http = try {
                                http()
                            } catch (httpError: Exception) {
                                throw Exception(
                                    "${packetError.message ?: "sol 包没发出去"}；" +
                                        "HTTP 兜底也失败：${httpError.message ?: "未知错误"}"
                                )
                            }
                            "sol 包没发出去（${packetError.message ?: "未知错误"}），已改走它自己的 AnyBoot 接口：$http"
                        }
                    }
                }
                runOnUiThread {
                    powerBusy = false
                    android.util.Log.i("ABPOWER", "runPowerAction 完成：$message")
                    toast(message)
                }
            } catch (error: Exception) {
                android.util.Log.w(
                    "ABPOWER",
                    "runPowerAction 失败：${ServerProfile.powerActionLabel(action)} ${error.message ?: "未知错误"}",
                )
                runOnUiThread {
                    powerBusy = false
                    toast("${ServerProfile.powerActionLabel(action)}没发出去：${error.message ?: "未知错误"}")
                }
            }
        }, "anyboot-power").start()
    }

    /** 悬浮刷新按钮：50% 透明度常驻左下角，点一下 = 重载当前页面（先探测全部地址） */
    @Composable
    private fun BoxScope.RefreshFab() {
        val palette = LocalPalette.current
        Box(
            contentAlignment = Alignment.Center,
            modifier = Modifier
                .align(Alignment.BottomStart)
                .padding(start = 16.dp, bottom = 48.dp)
                .size(44.dp)
                .alpha(0.5f)
                .clip(CircleShape)
                .background(palette.elevate)
                .border(1.dp, palette.line, CircleShape)
                .clickable { reloadActive() },
        ) {
            Text("⟳", fontSize = 20.sp, color = palette.fg)
        }
    }

    /** 连不上时自动重载的间隔设置（⋮ 溢出菜单里打开） */
    @Composable
    private fun FailReloadDialog() {
        val palette = LocalPalette.current
        var custom by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { failReloadOpen = false },
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
                                selected = failReloadSec == sec,
                                onClick = {
                                    failReloadSec = sec
                                    saveFailReloadSec()
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
                            selected = failReloadSec !in listOf(5, 10, 30, 60, 0),
                            onClick = { },
                        )
                        OutlinedTextField(
                            value = custom.ifEmpty {
                                if (failReloadSec in listOf(5, 10, 30, 60, 0)) "" else failReloadSec.toString()
                            },
                            onValueChange = { custom = it.filter(Char::isDigit) },
                            label = { Text("自定义秒数", fontSize = 11.sp) },
                            singleLine = true,
                            textStyle = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.weight(1f),
                        )
                        Spacer(Modifier.width(8.dp))
                        TextButton(onClick = {
                            val parsed = custom.toIntOrNull()
                            if (parsed == null || parsed < 0) {
                                toast("间隔要是不小于 0 的整数秒")
                                return@TextButton
                            }
                            failReloadSec = parsed
                            saveFailReloadSec()
                            failReloadOpen = false
                        }) { Text("应用") }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { failReloadOpen = false }) { Text("完成") }
            },
        )
    }

    private fun saveFailReloadSec() {
        getSharedPreferences(UI_PREFS, MODE_PRIVATE).edit().putInt("failReloadSec", failReloadSec).apply()
        toast(
            if (failReloadSec > 0) "连不上时每 $failReloadSec 秒自动重载"
            else "已关闭自动重载"
        )
    }

    /** 睡眠旁的 ?：讲清楚「睡眠之后怎么叫醒」 */
    @Composable
    private fun SleepHelpDialog() {
        val palette = LocalPalette.current
        AlertDialog(
            onDismissRequest = { sleepHelpOpen = false },
            title = { Text("电源 · 睡眠与唤醒") },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    Text(
                        "电源按钮点了**立即执行**，不再弹确认框 —— 各动作的后果都在这里：",
                        fontSize = 13.sp, color = palette.fg,
                    )
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "· 睡眠 = 挂起到内存：几乎立刻能醒，但网络服务全停 —— 这个标签会显示离线、页面打不开，这是预期的。叫醒请点「唤醒」（需配好 MAC 并开启网络唤醒）；\n" +
                            "· 关机：电脑立刻关机，网页服务随之中断 —— 之后只能用「唤醒」开机（需配好 MAC），或到机器旁按电源键；\n" +
                            "· 重启：电脑立刻重启，网页会话断开 —— 重启完成后心跳会自动恢复在线；\n" +
                            "· 唤醒：发魔术包让电脑开机，需配好 MAC 并在主板开启网络唤醒。",
                        fontSize = 13.sp, color = palette.fgDim,
                    )
                    Spacer(Modifier.height(10.dp))
                    Text("这些按钮怎么执行（局域网 / sol）", fontSize = 13.sp, color = palette.fg)
                    Text(
                        "给这台电脑配了「局域网电源路由」的动作，会往路由里的**广播地址:端口**发一个 UDP 包，" +
                            "由目标机上装着的 **sol** 按端口执行 —— 这样机器睡着、网页服务没起来也能操作。" +
                            "没装 sol，包发得出去但没人执行。\n" +
                            "没配路由的动作走老路：唤醒发标准 WOL（魔术包），重启/关机/睡眠调电脑端的网页接口。",
                        fontSize = 13.sp, color = palette.fgDim,
                    )
                    Spacer(Modifier.height(10.dp))
                    Text("唤醒包从哪发：", fontSize = 13.sp, color = palette.fg)
                    Text(
                        "· 配了「唤醒」路由 → 按路由里的地址和端口发（MAC 可只挑几块网卡）；\n" +
                            "· 配了 SSH 跳板机 → 由跳板机在内网广播（在外网也能用）；\n" +
                            "· 都没有 → 手机自己往局域网广播（手机必须和电脑同一网段）；\n" +
                            "· 只有「远程接入」可用（出门经公网隧道） → 唤醒发不出去：" +
                            "魔术包是局域网 UDP 广播，走不了隧道。重启/关机/睡眠调电脑网页接口，隧道照常可用。",
                        fontSize = 13.sp, color = palette.fgDim,
                    )
                    Spacer(Modifier.height(10.dp))
                    Text("点了没反应，按顺序检查：", fontSize = 13.sp, color = palette.fg)
                    Text(
                        "① 主板 BIOS 里开启了 Wake on LAN / 网络唤醒（只影响唤醒）；\n" +
                            "② 用的是**有线网卡**的 MAC —— 绝大多数无线网卡不支持关机唤醒；\n" +
                            "③ MAC 填在「编辑服务端」里对应地址那一行（如 192.168.0.120:12300 AA:BB:CC:DD:EE:FF）；\n" +
                            "④ 路由里的广播地址和端口对不对（要和 sol 侧监听的一致）；\n" +
                            "⑤ 主机没被拔电源、插排没断电（关机后网卡要靠待机供电收包）。",
                        fontSize = 13.sp, color = palette.fgDim,
                    )
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "Windows 还要在「设备管理器 → 网卡 → 电源管理」勾上「允许此设备唤醒计算机」，" + "并关掉「快速启动」（快速启动是休眠，会跳过 WOL）。",
                        fontSize = 13.sp, color = palette.fgDim,
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = { sleepHelpOpen = false }) { Text("知道了") }
            },
        )
    }

    // ------------------------------------------------------------------
    // 弹窗
    // ------------------------------------------------------------------

    @Composable
    private fun EditDialog(request: EditRequest) {
        val palette = LocalPalette.current
        // 表单体与「电脑列表 → 编辑这台电脑」共用同一份实现（ServerEditForm.kt），
        // 这里只负责 AlertDialog 这层壳
        val form = rememberServerEditFormState(request.profile)

        AlertDialog(
            onDismissRequest = { if (!request.firstRun) editRequest = null },
            // 键盘弹出时不让底部的按钮被盖住（内容区自己让出 IME 的高度）
            modifier = Modifier.imePadding(),
            properties = DialogProperties(decorFitsSystemWindows = false),
            title = {
                Text(
                    when {
                        form.editing -> "编辑服务端"
                        request.firstRun -> "连接 AnyBoot 服务端"
                        else -> "添加服务端"
                    }
                )
            },
            text = { ServerEditFormBody(form) },
            confirmButton = {
                TextButton(onClick = {
                    if (saveServer(request, form)) {
                        editRequest = null
                    }
                }) { Text("保存") }
            },
            dismissButton = {
                Row {
                    if (form.editing) {
                        TextButton(onClick = { request.profile?.let { confirmDelete(it) } }) {
                            Text("删除", color = palette.bad)
                        }
                    }
                    if (!request.firstRun) {
                        TextButton(onClick = { editRequest = null }) { Text("取消") }
                    }
                }
            },
        )
    }

    /**
     * 保存表单；返回 false = 校验没过（对话框保持打开）。
     *
     * 地址的写入语义分两种，别混：**编辑时以输入为准**（replace，方便删掉过期地址），
     * **新增时按分组并入同一台电脑**（merge，同一台机的多块网卡合成一条）。
     */
    private fun saveServer(request: EditRequest, form: ServerEditFormState): Boolean {
        val editing = request.profile
        val parsed = form.toParsed()
        if (parsed.isEmpty()) {
            toast(SERVER_EDIT_EMPTY_ADDRESS_HINT)
            return false
        }
        val previousAddresses = editing?.let { ArrayList(it.addresses) } ?: ArrayList()
        val previousToken = editing?.let { ServerProfile.text(it.token) } ?: ""

        val target: ServerProfile
        var notice = ""
        if (editing != null) {
            target = editing
        } else {
            // 新增：分组名相同的记录就是同一台电脑，并进去，不新建标签
            val merged = findByGroup(form.group.trim())
            target = if (merged != null) {
                notice = "已并入同一台电脑：" + merged.label()
                merged
            } else {
                val fresh = ServerProfile()
                servers.add(fresh)
                fresh
            }
        }
        if (!applyServerEdit(form, target, replaceAddresses = editing != null)) {
            toast(SERVER_EDIT_EMPTY_ADDRESS_HINT)
            return false
        }
        ServerStore.sort(servers)
        ServerStore.save(this, servers)
        editRequest = null
        if (notice.isNotEmpty()) toast(notice)
        val changed = previousAddresses != target.addresses
                || previousToken != ServerProfile.text(target.token)
        connect(target, changed)
        activate(target.id)
        // 顺手探一次 /health：知道「这台电脑」的机器标识后，才认得出它
        // 是不是已经有另一块网卡的记录 —— 有就自动并成一条
        probeMachine(target)
        return true
    }

    @Composable
    private fun ServerListDialog() {
        val palette = LocalPalette.current
        AlertDialog(
            onDismissRequest = { serverListOpen = false },
            title = { Text("切换服务端（长按标签也能编辑）") },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    if (servers.isEmpty()) {
                        EmptyHint("还没有服务端。")
                    }
                    servers.forEach { profile ->
                        Text(
                            text = (if (profile.id == activeId) "● " else "　") + profile.label()
                                    + "　" + profile.addressSummary(),
                            fontSize = 14.sp,
                            color = palette.fg,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    serverListOpen = false
                                    activate(profile.id)
                                }
                                .padding(vertical = 12.dp),
                        )
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    serverListOpen = false
                    editRequest = EditRequest(null, false)
                }) { Text("添加") }
            },
            dismissButton = {
                TextButton(onClick = {
                    serverListOpen = false
                    val removed = mergeSameMachine()
                    toast(
                        if (removed > 0) "已合并 $removed 条重复记录 —— 同一台电脑只留一个标签"
                        else "没有需要合并的：同一台电脑要么已经只有一条，要么分组名不同",
                        long = true,
                    )
                }) { Text("合并同一台电脑的地址") }
            },
        )
    }

    @Composable
    private fun TransportDialog() {
        val values = listOf("auto", "ws", "http")
        val labels = listOf(
            "自动（推送优先，连不上退回轮询）",
            "推送（WebSocket）",
            "轮询（HTTP 每 2s）",
        )
        AlertDialog(
            onDismissRequest = { transportOpen = false },
            title = { Text("传输方式") },
            text = {
                Column {
                    values.forEachIndexed { index, value ->
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    transport = value
                                    ServerStore.setTransport(this@MainActivity, value)
                                    transportOpen = false
                                    toast("已选择「${transportLabel()}」：重载页面后生效（页面上也能即切）")
                                }
                                .padding(vertical = 6.dp),
                        ) {
                            RadioButton(selected = transport == value, onClick = {
                                transport = value
                                ServerStore.setTransport(this@MainActivity, value)
                                transportOpen = false
                                toast("已选择「${transportLabel()}」：重载页面后生效（页面上也能即切）")
                            })
                            // 最长一条「自动（推送优先，连不上退回轮询）」在 14sp 下已接近本行可用宽度，
                            // 系统字体一放大就会复现「末尾 Text 被压成 0 宽逐字换行」那种崩坏，提前上 weight
                            Text(labels[index], fontSize = 14.sp, modifier = Modifier.weight(1f, fill = false))
                        }
                    }
                }
            },
            confirmButton = {},
            dismissButton = {
                TextButton(onClick = { transportOpen = false }) { Text("取消") }
            },
        )
    }

    /** 菜单上那一条「传输方式（…）」给人看的说法 */
    private fun transportLabel(): String = when (transport) {
        "ws" -> "推送"
        "http" -> "轮询"
        else -> "自动"
    }

    private fun confirmDelete(profile: ServerProfile) {
        deleteServerTarget = profile
    }

    @Composable
    private fun DeleteServerDialog(profile: ServerProfile) {
        AlertDialog(
            onDismissRequest = { editRequest = null },
            title = { Text("删除服务端") },
            text = { Text("确定删除「${profile.label()}」吗？只会删掉手机上的这条记录。") },
            confirmButton = {
                TextButton(onClick = {
                    editRequest = null
                    deleteServerTarget = null
                    servers.remove(profile)
                    dropTab(profile)
                    ServerStore.save(this, servers)
                    if (servers.isEmpty()) {
                        activeId = ""
                        ServerStore.setActiveId(this, "")
                        editRequest = EditRequest(null, true)
                        return@TextButton
                    }
                    if (profile.id == activeId) {
                        activeId = servers[0].id
                    }
                    activate(activeId)
                }) { Text("删除", color = LocalPalette.current.bad) }
            },
            dismissButton = {
                TextButton(onClick = { deleteServerTarget = null }) { Text("取消") }
            },
        )
    }

    // ------------------------------------------------------------------
    // 连接 / 切换（与旧实现逐段平移）
    // ------------------------------------------------------------------

    private fun ensureTab(profile: ServerProfile): Tab {
        var tab = tabs[profile.id]
        if (tab == null) {
            tab = Tab(profile)
            tabs[profile.id] = tab
        }
        tab.profile = profile
        if (tab.view == null) {
            val view = createWebView(profile)
            view.visibility = ViewGroup.GONE
            tab.view = view
            container?.addView(view, 0, FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        }
        return tab
    }

    /** 保证这台电脑的 WebView 存在并加载（多台电脑可以同时在线） */
    private fun connect(profile: ServerProfile, forceReload: Boolean) {
        val tab = ensureTab(profile)
        if (!forceReload && tab.started) {
            // 已经连过就保留后台页面，切回来直接显示，不重新拉
            return
        }
        tab.addressIndex = 0      // 重新连：从首选地址开始
        tab.tried.clear()
        loadCurrentAddress(tab)
    }

    /** 用「当前地址下标」加载这条记录 —— 首次连接和换地址都走这里 */
    private fun loadCurrentAddress(tab: Tab) {
        val profile = tab.profile
        tab.usingRemote = false // LAN 路径：清掉远程标记
        val address = profile.hostAt(tab.addressIndex)
        if (address.isEmpty()) return
        tab.tried.remove(address)     // 正在试的这个不算「试过了」
        tab.mainFailed = false
        tab.started = true
        tab.status = STATUS_LOADING
        chipTick++
        if (profile.id == activeId) {
            errorInfo = null
            loading = true
        }
        // 经由跳板机：先把「内网地址」解析成本地隧道端口（后台），再从 localhost 加载
        if (ServerProfile.text(profile.jumpServerId).isNotEmpty()) {
            connectThroughTunnel(tab, address)
            return
        }
        tab.effectiveAddress = ""
        // 传输方式从 URL 带进网页 —— 用 localStorage 会随地址（来源）变化而丢
        tab.view?.loadUrl(profile.urlAt(tab.addressIndex, transport))
    }

    /**
     * 走跳板机连接：开（或复用）SSH 本地转发，然后 WebView 照常加载
     * {@code 127.0.0.1:端口} —— 页面、/api、心跳全部自动经隧道出去。
     */
    private fun connectThroughTunnel(tab: Tab, destAddress: String) {
        Thread({
            val localAddress: String
            try {
                val jump = JumpStore.findJump(JumpStore.loadJumps(this), tab.profile.jumpServerId)
                    ?: throw Exception("跳板机不存在，请到「服务器」页检查")
                localAddress = SshSessionManager.shared().tunnelAddress(
                    this, jump, destAddress
                ) { title, message -> bus.confirm(title, message) }
            } catch (error: Exception) {
                val message = error.message ?: "隧道建立失败"
                runOnUiThread {
                    // 还有备用地址（跳板机视角）就继续换；全试完才算失败
                    if (!advanceAddress(tab)) {
                        tab.mainFailed = true
                        tab.status = STATUS_FAIL
                        chipTick++
                        if (tab.profile.id == activeId) {
                            showErrorFor(tab.profile, "经由跳板机连接失败：$message")
                        }
                    }
                }
                return@Thread
            }
            runOnUiThread {
                val fresh = tabs[tab.profile.id] ?: return@runOnUiThread
                fresh.effectiveAddress = localAddress
                fresh.view?.loadUrl(tab.profile.urlFor(localAddress, transport))
            }
        }, "anyboot-tunnel").start()
    }

    /**
     * 换到下一个还没试过的地址（同一台电脑的其它网卡）。
     * 换地址不换页面：还是同一个标签、同一个 WebView。
     *
     * @return 还有地址可试（已经发起加载）返回 true；全试完了返回 false
     */
    private fun advanceAddress(tab: Tab): Boolean {
        // 远程通道这轮已经试过且也挂了：收工，等下次手动 / 自动重载再起新一轮
        if (tab.usingRemote) {
            if (!tab.profile.isRemoteFirst()) {
                return false
            }
            // 远程优先但隧道挂了：退回局域网链路继续试（triedRemote 闸防再来一轮远程）
            tab.usingRemote = false
            tab.tried.clear()
        }
        val profile = tab.profile
        tab.tried.add(profile.hostAt(tab.addressIndex))
        for (i in 0 until profile.addressCount()) {
            val address = profile.hostAt(i)
            if (tab.tried.contains(address)) continue
            tab.addressIndex = i
            tab.status = STATUS_LOADING
            chipTick++
            if (profile.id == activeId) {
                loading = true
                toast("这个地址不通，换一个地址重试：$address")
            }
            loadCurrentAddress(tab)
            return true
        }
        // 局域网候选全试完：配了远程接入就兜底走公网隧道
        return connectRemote(tab)
    }

    /**
     * 局域网候选全挂 → 远程接入兜底：加载 {@code https://origin/?ab_token=…}。
     * 没配远程返回 false（调用方走失败页）；本轮内远程失败也不再重试
     * （advanceAddress 的 usingRemote 闸）。
     */
    private fun connectRemote(tab: Tab): Boolean {
        val profile = tab.profile
        // triedRemote 闸：本轮（含远程优先退回局域网后再失败的情况）远程只试一次
        if (tab.triedRemote || !profile.hasRemote()) {
            return false
        }
        tab.triedRemote = true
        tab.usingRemote = true
        tab.tried.clear()
        tab.mainFailed = false
        tab.started = true
        tab.status = STATUS_LOADING
        chipTick++
        if (profile.id == activeId) {
            errorInfo = null
            loading = true
            toast("局域网不通，改走远程通道…")
        }
        tab.effectiveAddress = ""
        tab.view?.loadUrl(profile.remoteUrl(transport))
        return true
    }

    /**
     * 连上了：把当前地址提到最前（下次优先用它）。
     * 这就是「多块网卡 / DHCP 换 IP」能自愈的原因。
     */
    private fun rememberWorkingAddress(tab: Tab) {
        if (!tab.profile.promoteAddress(tab.addressIndex)) return
        tab.addressIndex = 0
        ServerStore.save(this, servers)
    }

    private fun activate(id: String) {
        val profile = ServerStore.find(servers, id) ?: return
        activeId = id
        ServerStore.setActiveId(this, id)
        // 先确保这个服务端的 WebView 在（可能是第一次点它），再统一按 activeId 定显隐
        connect(profile, false)
        applyVisibility()
        chipTick++
        val tab = tabs[id]
        if (tab != null && tab.status == STATUS_FAIL) {
            showErrorFor(profile, "上一次加载失败")
        } else {
            errorInfo = null
            loading = tab != null && tab.status == STATUS_LOADING
            applyVisibility()
        }
    }

    /** 「启动时连接全部」：把每个服务端都拉起来，标签上能同时看到它们的状态 */
    private fun connectAllServers() {
        for (profile in servers) {
            connect(profile, false)
        }
        applyVisibility()
        val active = tabs[activeId]
        if (active != null && active.status != STATUS_FAIL) {
            errorInfo = null
        }
    }

    private fun applyVisibility() {
        for (tab in tabs.values) {
            tab.view?.visibility = if (tab.profile.id == activeId) ViewGroup.VISIBLE else ViewGroup.GONE
        }
    }

    private fun reloadActive() {
        val profile = ServerStore.find(servers, activeId) ?: return
        startAttempt(ensureTab(profile))
    }

    /** 单个地址的探测结果 */
    private class ProbeOutcome(val address: String, val ok: Boolean, val ms: Long)

    /** 探测一轮的结果：逐地址结果表 + 选中的赢家（连接用它） */
    private class ProbeResult(val outcomes: List<ProbeOutcome>, val winnerAddress: String?)

    /**
     * 按档案的探测模式探所有候选地址（后台线程调用）。三个模式：
     * - **并发（首个）**：每地址一个线程，任一响应立即胜出（done 旗标），其余作废；
     * - **并发（全部）**：等全部线程结束，赢家 = 最快的成功者，逐行结果全带回去；
     * - **串行**：按顺序第一个通就停。
     * 单地址 / 跳板机不探：交给 WebView 加载链路自己兜。
     */
    private fun probeAddresses(profile: ServerProfile): ProbeResult {
        val addresses = profile.addresses
        if (addresses.size < 2) {
            return ProbeResult(emptyList(), null)
        }
        val mode = ServerProfile.normalizeProbeMode(profile.probeMode)
        val timeoutMs = 3000
        val outcomes = arrayOfNulls<ProbeOutcome>(addresses.size)
        if (mode == ServerProfile.PROBE_SERIAL) {
            // 串行：按顺序逐个探，第一个通的即止（保留用户排的地址优先级）
            var winner: String? = null
            for (i in addresses.indices) {
                val rtt = HealthProbe.ping(addresses[i], timeoutMs)
                outcomes[i] = ProbeOutcome(addresses[i], rtt >= 0, if (rtt >= 0) rtt else 0)
                if (rtt >= 0) {
                    winner = addresses[i]
                    break
                }
            }
            return ProbeResult(outcomes.filterNotNull(), winner)
        }
        val firstOnly = mode == ServerProfile.PROBE_PARALLEL_FIRST
        val done = AtomicBoolean(false)
        val winnerAddress = AtomicReference<String?>(null)
        val winnerMs = AtomicLong(0)
        val latch = CountDownLatch(addresses.size)
        for (i in addresses.indices) {
            val address = addresses[i]
            Thread({
                try {
                    if (firstOnly && done.get()) {
                        return@Thread // 已有赢家：不再白探（在途的等它自己超时收尾）
                    }
                    val rtt = HealthProbe.ping(address, timeoutMs)
                    if (rtt >= 0 && (!firstOnly || done.compareAndSet(false, true))) {
                        if (firstOnly) {
                            winnerAddress.set(address)
                            winnerMs.set(rtt)
                        } else {
                            // 并发（全部）：谁快谁当赢家
                            synchronized(winnerAddress) {
                                val current = winnerAddress.get()
                                if (current == null || rtt < winnerMs.get()) {
                                    winnerAddress.set(address)
                                    winnerMs.set(rtt)
                                }
                            }
                        }
                    }
                    outcomes[i] = ProbeOutcome(address, rtt >= 0, if (rtt >= 0) rtt else 0)
                } finally {
                    latch.countDown()
                }
            }, "anyboot-probe").start()
        }
        latch.await(10, TimeUnit.SECONDS)
        return ProbeResult(outcomes.filterNotNull(), winnerAddress.get())
    }

    /** 重新开始一轮连接：多地址先按探测模式比一遍（赢家直接上），单地址 / 跳板机走老链路 */
    private fun startAttempt(tab: Tab) {
        tab.tried.clear()
        tab.usingRemote = false
        tab.triedRemote = false
        errorInfo = null
        val profile = tab.profile
        // 远程优先（编辑里选的）：直接走隧道，局域网只做兜底（隧道挂了 advanceAddress 会退回来）
        if (profile.isRemoteFirst() && profile.hasRemote()) {
            connectRemote(tab)
            return
        }
        if (ServerProfile.text(profile.jumpServerId).isNotEmpty() || profile.addresses.size < 2) {
            loadCurrentAddress(tab)
            return
        }
        tab.status = STATUS_LOADING
        chipTick++
        if (profile.id == activeId) {
            loading = true
        }
        Thread({
            val result = probeAddresses(profile)
            runOnUiThread {
                val fresh = tabs[profile.id] ?: return@runOnUiThread
                if (fresh.profile.id != profile.id) return@runOnUiThread // 标签已被编辑/删除
                applyProbeResult(fresh, result)
            }
        }, "anyboot-probe").start()
    }

    /** 探测完成：提示结果行，赢家提到候选列表最前并直接加载；全挂就给失败页 */
    private fun applyProbeResult(tab: Tab, result: ProbeResult) {
        val profile = tab.profile
        val mode = ServerProfile.normalizeProbeMode(profile.probeMode)
        val winner = result.winnerAddress
        if (winner == null) {
            // 局域网候选全挂：配了远程接入就兜底走公网隧道
            if (connectRemote(tab)) return
            tab.mainFailed = true
            tab.status = STATUS_FAIL
            chipTick++
            if (profile.id == activeId) {
                showErrorFor(profile, "全部地址都连不上")
            }
            return
        }
        if (profile.id == activeId) {
            if (mode == ServerProfile.PROBE_PARALLEL_ALL) {
                toast(result.outcomes.joinToString("\n") { o ->
                    o.address + if (o.ok) " 连接成功 ${o.ms} ms" else " 连接失败"
                })
            } else {
                val ms = result.outcomes.firstOrNull { it.address == winner }?.ms ?: 0L
                toast("ip $winner 连接成功 $ms ms")
            }
        }
        val index = profile.addresses.indexOf(winner)
        if (index > 0) {
            // 赢家提到最前并落盘：下次连接 / 心跳都优先用它（DHCP 换 IP 自愈同款）
            profile.promoteAddress(index)
            ServerStore.save(this, servers)
        }
        tab.addressIndex = profile.addresses.indexOf(winner).coerceAtLeast(0)
        loadCurrentAddress(tab)
    }

    /** 全部重载 = 把没连上的也连上，并把在线的一起刷新（一律从首选地址重新试） */
    private fun reloadAll() {
        for (profile in servers) {
            val tab = ensureTab(profile)
            tab.addressIndex = 0
            tab.tried.clear()
            loadCurrentAddress(tab)
        }
        activate(activeId)
    }

    private fun dropTab(profile: ServerProfile) {
        val tab = tabs.remove(profile.id)
        if (tab?.view != null) {
            container?.removeView(tab.view)
            tab.view?.destroy()
            tab.view = null
        }
    }

    private fun showErrorFor(profile: ServerProfile, description: String) {
        loading = false
        for (tab in tabs.values) {
            tab.view?.visibility = ViewGroup.GONE
        }
        val count = profile.addressCount()
        val detail = buildString {
            append("服务端：").append(profile.label()).append('\n')
            append("地址：").append(profile.addressSummary()).append("\n\n")
            append(description)
            if (count > 1) {
                append("\n\n这台电脑的 ").append(count).append(" 个地址都试过了：\n  ")
                append(profile.addresses.joinToString("\n  "))
            }
            append("\n\n请确认：\n")
            append("· 电脑上 AnyBoot 的「远程控制」已打开，手机和电脑在同一局域网\n")
            append("· 地址填的是电脑上的 IP 和端口（例如 192.168.0.120:12300）")
        }
        errorInfo = ErrorInfo(profile.label(), profile.addressSummary(), detail)
    }

    // ------------------------------------------------------------------
    // 「同一台电脑」的识别与合并
    // ------------------------------------------------------------------

    /** 找找有没有这个分组的记录（分组 = 一台电脑） */
    private fun findByGroup(group: String): ServerProfile? {
        val target = ServerProfile.text(group)
        if (target.isEmpty()) return null
        for (profile in servers) {
            if (target.equals(ServerProfile.text(profile.group), ignoreCase = true)) {
                return profile
            }
        }
        return null
    }

    /** 后台探一次电脑端的 `/health`，拿到机器标识（多块网卡自动归并的关键） */
    private fun probeMachine(profile: ServerProfile) {
        val host = profile.host()
        if (host.isEmpty()) return
        Thread({
            val result = HealthProbe.fetch(host) ?: return@Thread
            runOnUiThread { applyMachineIdentity(profile, result) }
        }, "anyboot-health-probe").start()
    }

    /** 探到了机器身份：记下来；如果这台电脑已经有别的记录，就把两条并成一条 */
    private fun applyMachineIdentity(profile: ServerProfile, result: HealthProbe.Result) {
        if (!servers.contains(profile)) return
        if (result.machineId.isNotEmpty()) profile.machineId = result.machineId
        if (result.machineName.isNotEmpty()) profile.machineName = result.machineName
        var sibling: ServerProfile? = null
        for (other in servers) {
            if (profile.sameMachine(other)) {
                sibling = other
                break
            }
        }
        sibling ?: run {
            ServerStore.save(this, servers)   // 只把机器标识记下来
            return
        }
        // 同一台电脑的两条记录 → 并成一条。正在看的那条留下，免得页面跳走。
        val keep: ServerProfile
        val drop: ServerProfile
        if (profile.id == activeId) {
            keep = profile; drop = sibling
        } else if (sibling.id == activeId) {
            keep = sibling; drop = profile
        } else {
            keep = sibling   // 都不在用：留早先那条，并掉刚探到的
            drop = profile
        }
        mergeInto(keep, drop)
        dropTab(drop)
        servers.remove(drop)
        ServerStore.sort(servers)
        ServerStore.save(this, servers)
        if (drop.id == activeId) {
            activeId = keep.id
            ServerStore.setActiveId(this, keep.id)
        }
        activate(activeId)
        toast(
            "已把同一台电脑的地址合并到一个标签：" + keep.label()
                    + (if (result.machineName.isEmpty()) "" else "（${result.machineName}）"),
            long = true,
        )
    }

    /** 把 drop 的地址并进 keep（去重保序），并补上只有 drop 才知道的信息 */
    private fun mergeInto(keep: ServerProfile, drop: ServerProfile) {
        keep.mergeAddresses(drop)
        if (ServerProfile.text(keep.machineId).isEmpty()) keep.machineId = drop.machineId
        if (ServerProfile.text(keep.machineName).isEmpty()) keep.machineName = drop.machineName
        if (ServerProfile.text(keep.group).isEmpty()) keep.group = drop.group
        if (ServerProfile.text(keep.name).isEmpty()) keep.name = drop.name
        if (ServerProfile.text(keep.token).isEmpty()) keep.token = drop.token
        if (ServerProfile.text(keep.jumpServerId).isEmpty()) keep.jumpServerId = drop.jumpServerId
        // 远程接入也是同一台电脑的属性：留下来的没配而被并的配了，就整体搬过来
        //（否则编辑后保存 → probeMachine 触发合并 → 配好的远程信息跟着 drop 一起消失）
        if (!keep.hasRemote() && drop.hasRemote()) {
            keep.remoteAddr = drop.remoteAddr
            keep.remoteTunnelAddr = drop.remoteTunnelAddr
            keep.remoteTsAddr = drop.remoteTsAddr
            keep.remoteToken = drop.remoteToken
            keep.connMode = drop.connMode // 连接优先跟着远程接入一起走
        }
    }

    /** 把「其实是同一台电脑」的多条记录并成一条 —— 给老配置用 */
    private fun mergeSameMachine(): Int {
        var removed = 0
        var i = 0
        while (i < servers.size) {
            var keep = servers[i]
            for (j in servers.size - 1 downTo i + 1) {
                var drop = servers[j]
                if (!keep.sameMachine(drop)) continue
                if (drop.id == activeId) {
                    // 正在用的那条留下来（页面不跳走）：和 keep 换个位置
                    servers[i] = drop
                    servers[j] = keep
                    val swap = keep
                    keep = drop
                    drop = swap
                }
                mergeInto(keep, drop)
                dropTab(drop)
                servers.removeAt(j)
                removed++
            }
            i++
        }
        if (removed > 0) {
            ServerStore.sort(servers)
            ServerStore.save(this, servers)
            if (ServerStore.find(servers, activeId) == null) {
                activeId = if (servers.isEmpty()) "" else servers[0].id
                ServerStore.setActiveId(this, activeId)
            }
            activate(activeId)
        }
        return removed
    }

    // ------------------------------------------------------------------
    // WebView
    // ------------------------------------------------------------------

    @SuppressLint("SetJavaScriptEnabled")
    private fun createWebView(profile: ServerProfile): WebView {
        val view = WebView(this)
        val settings = view.settings
        settings.javaScriptEnabled = true
        // 前端把 token 存在 cookie、栏目选择存在 localStorage，都要 DOM storage
        settings.domStorageEnabled = true
        settings.databaseEnabled = true
        settings.useWideViewPort = true
        settings.loadWithOverviewMode = true
        settings.setSupportZoom(false)
        settings.builtInZoomControls = false
        // 服务端是局域网明文 HTTP，这里必须放行（否则 WebView 会当成不安全混合内容拦掉）
        settings.mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
        settings.cacheMode = WebSettings.LOAD_DEFAULT
        settings.allowFileAccess = false
        settings.allowContentAccess = false
        // 让服务端日志/前端能认出这是 App 客户端而不是普通浏览器
        settings.userAgentString = settings.userAgentString + " AnyBootRemote/1.2"

        CookieManager.getInstance().setAcceptCookie(true)

        view.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(target: WebView, request: WebResourceRequest): Boolean {
                val uri = request.url
                val scheme = uri?.scheme
                if ("anyboot".equals(scheme, true) && uri.host == "reload") {
                    // 页面下拉刷新在 App 里 = 原生整页重载（先探测全部候选地址，换 IP 也跟得上）
                    reloadActive()
                    return true
                }
                if ("http".equals(scheme, true) || "https".equals(scheme, true)) {
                    // 站内跳转（含 SPA 路由）留在 WebView 里
                    return false
                }
                // http(s) 之外（mailto:、tel: 等）交给系统
                try {
                    startActivity(Intent(Intent.ACTION_VIEW, uri))
                } catch (ignored: Exception) {
                    // 没有对应应用就静默忽略，不能因为外链让客户端崩掉
                }
                return true
            }

            override fun onPageStarted(target: WebView, url: String, favicon: Bitmap?) {
                val tab = tabs[profile.id] ?: return
                if (!isCurrentAddress(tab, url)) {
                    // 换地址之后旧地址的回调，丢掉 —— 否则会把状态改回去
                    return
                }
                tab.mainFailed = false
                tab.status = STATUS_LOADING
                chipTick++
                if (profile.id == activeId) {
                    errorInfo = null
                    loading = true
                }
            }

            override fun onPageFinished(target: WebView, url: String) {
                val tab = tabs[profile.id] ?: return
                if (!isCurrentAddress(tab, url)) return
                if (tab.status != STATUS_FAIL) {
                    tab.status = STATUS_OK
                    chipTick++
                    // 连上了：把这个地址记成首选，下次开机直连它（DHCP 换过 IP 也能自愈）
                    rememberWorkingAddress(tab)
                }
                if (profile.id == activeId) loading = false
            }

            override fun onReceivedError(target: WebView, request: WebResourceRequest, error: WebResourceError?) {
                // 只有主文档失败才算「连不上」；子资源（图标、字体）失败不该盖住页面
                if (!request.isForMainFrame) return
                val tab = tabs[profile.id] ?: return
                if (!isCurrentAddress(tab, request.url?.toString())) return
                tab.mainFailed = true
                // 一台电脑常有多个地址（多块网卡）：这个不通就先换下一个，
                // 别急着把「连不上」摔到用户脸上
                if (advanceAddress(tab)) return
                tab.status = STATUS_FAIL
                chipTick++
                if (profile.id == activeId) {
                    showErrorFor(tab.profile, error?.description?.toString() ?: "")
                }
            }
        }
        return view
    }

    /**
     * 这个回调说的是「当前正在用的那个地址」吗？
     * 换地址之后，旧地址的回调还会陆续回来，不认一下就换错了。
     */
    private fun isCurrentAddress(tab: Tab, url: String?): Boolean {
        if (url == null) return false
        // 远程兜底：加载的是 https 隧道 origin，按前缀比对
        if (tab.usingRemote) {
            val origin = tab.profile.remoteOrigin()
            return origin.isNotEmpty() && url.startsWith(origin)
        }
        // 走跳板机：加载的是隧道地址（127.0.0.1:端口），直连才用候选地址
        val address = if (tab.effectiveAddress.isNotEmpty()) tab.effectiveAddress
        else tab.profile.hostAt(tab.addressIndex)
        if (address.isEmpty()) return false
        val prefix = "http://$address"
        // 地址后面必须紧跟边界：否则 192.168.1.5 会把 192.168.1.50 的页面认成自己
        return url.length > prefix.length
                && url.startsWith(prefix)
                && (url[prefix.length] == '/' || url[prefix.length] == ':')
    }

    // ------------------------------------------------------------------
    // 生命周期
    // ------------------------------------------------------------------

    override fun onResume() {
        super.onResume()
        // 记「上次停留页」= 电脑页。冷启动时这里先记 home，紧接着电脑列表的 onResume
        // 会记回 computers —— 以最终停留页为准；从电脑列表按返回回到这里时只有这条生效。
        getSharedPreferences(UI_PREFS, MODE_PRIVATE).edit().putString(PREF_ENTRY, "home").apply()
        for (tab in tabs.values) {
            tab.view?.onResume()
        }
        // 回前台：心跳立即先探一轮（状态点总是新鲜的），再按 10 秒排下去
        heartbeat?.start()
    }

    override fun onPause() {
        // 后台标签也一起暂停 JS/定时器，回前台再恢复
        for (tab in tabs.values) {
            tab.view?.onPause()
        }
        // 心跳停表省电；回前台 onResume 会先补探一轮
        heartbeat?.stop()
        super.onPause()
    }
}
