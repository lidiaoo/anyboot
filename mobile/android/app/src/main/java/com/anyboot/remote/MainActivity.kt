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
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.launch

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
        firstRunPending = servers.isEmpty()

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
        })
        heartbeat?.tunnelResolver = tunnelResolver

        setContent { Root() }
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
                        }
                    }
                    DrawerOverlay()
                }
                editRequest?.let { EditDialog(it) }
                deleteServerTarget?.let { DeleteServerDialog(it) }
                if (serverListOpen) ServerListDialog()
                if (transportOpen) TransportDialog()
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
            Button(onClick = { reloadActive() }) { Text("重试") }
            Spacer(Modifier.height(8.dp))
            OutlinedButton(onClick = {
                editRequest = EditRequest(ServerStore.find(servers, activeId), false)
            }) { Text("修改这个服务端") }
            Spacer(Modifier.weight(1f))
        }
    }

    /** 抽屉侧边栏：AB 标识 + 电脑 / 服务器 / 关于 / 主题 */
    @Composable
    private fun DrawerOverlay() {
        val palette = LocalPalette.current
        val context = LocalContext.current
        if (!drawerOpen) return
        Box(Modifier.fillMaxSize()) {
            Box(
                Modifier
                    .fillMaxSize()
                    .background(Color(0x55000000))
                    .clickable { drawerOpen = false }
            )
            Column(
                Modifier
                    .fillMaxHeight()
                    .width(264.dp)
                    .background(palette.card)
                    .align(Alignment.CenterStart),
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(palette.accent)
                        .padding(horizontal = 16.dp, vertical = 20.dp),
                ) {
                    Box(
                        Modifier
                            .size(40.dp)
                            .clip(CircleShape)
                            .background(Color(0x33FFFFFF)),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text("AB", color = Color.White, fontSize = 20.sp, fontWeight = FontWeight.Bold)
                    }
                    Spacer(Modifier.width(12.dp))
                    Text("AnyBoot", color = Color.White, fontSize = 17.sp)
                }
                DrawerItem("🖥  电脑", "回到电脑列表") { drawerOpen = false }
                DrawerItem("🖧  服务器", "SSH / Telnet 跳板机管理") {
                    drawerOpen = false
                    context.startActivity(Intent(context, ServersActivity::class.java))
                }
                DrawerItem("ⓘ  关于", "项目信息与 Star") {
                    drawerOpen = false
                    context.startActivity(Intent(context, AboutActivity::class.java))
                }
                DrawerItem(
                    "🌓  主题：" + Ui.modeLabel(this@MainActivity),
                    "点按在 跟随系统 / 暗色 / 亮色 之间切换",
                ) {
                    Ui.cycle(this@MainActivity)
                    drawerOpen = false
                    (context as? MainActivity)?.recreate()
                }
                Spacer(Modifier.weight(1f))
                Text(
                    text = "外网连家里的电脑：先把家里能公网访问的机器加成 SSH 服务器，"
                            + "再在电脑的「经由跳板机」里选它。",
                    fontSize = 12.sp,
                    color = palette.fgFaint,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 16.dp),
                )
            }
        }
    }

    @Composable
    private fun DrawerItem(label: String, description: String, onClick: () -> Unit) {
        val palette = LocalPalette.current
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .clickable(onClick = onClick)
                .padding(start = 20.dp, top = 14.dp, end = 12.dp, bottom = 14.dp),
        ) {
            Text(label, fontSize = 15.sp, color = palette.fg, modifier = Modifier.weight(1f))
            Text("›", fontSize = 16.sp, color = palette.fgFaint)
        }
    }

    // ------------------------------------------------------------------
    // 弹窗
    // ------------------------------------------------------------------

    @Composable
    private fun EditDialog(request: EditRequest) {
        val palette = LocalPalette.current
        val editing = request.profile != null
        val profile = request.profile

        var group by remember(request) { mutableStateOf(profile?.group ?: "") }
        var name by remember(request) { mutableStateOf(profile?.name ?: "") }
        var hosts by remember(request) { mutableStateOf(profile?.addressText() ?: "") }
        var token by remember(request) { mutableStateOf(profile?.token ?: "") }
        val jumps = remember(request) { JumpStore.loadJumps(this) }
        var jumpIndex by remember(request) {
            mutableStateOf(
                if (editing) {
                    val found = jumps.indexOfFirst { it.id == profile?.jumpServerId }
                    if (found >= 0) found + 1 else 0
                } else 0
            )
        }
        var jumpOpen by remember { mutableStateOf(false) }
        var serialProbe by remember(request) {
            mutableStateOf(editing && ServerProfile.PROBE_SERIAL == profile?.probeMode)
        }

        AlertDialog(
            onDismissRequest = { if (!request.firstRun) editRequest = null },
            // 键盘弹出时不让底部的按钮被盖住（内容区自己让出 IME 的高度）
            modifier = Modifier.imePadding(),
            properties = DialogProperties(decorFitsSystemWindows = false),
            title = {
                Text(
                    when {
                        editing -> "编辑服务端"
                        request.firstRun -> "连接 AnyBoot 服务端"
                        else -> "添加服务端"
                    }
                )
            },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    FieldLabel("分组（用来区分不同的电脑）")
                    OutlinedTextField(
                        value = group,
                        onValueChange = { group = it },
                        singleLine = true,
                        placeholder = { Text("例如：书房台式机（同一台电脑的多个 IP 填同一个分组）", fontSize = 12.sp) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    FieldLabel("服务端名（同一台电脑的不同网卡/地址，用名字区分）")
                    OutlinedTextField(
                        value = name,
                        onValueChange = { name = it },
                        singleLine = true,
                        placeholder = { Text("例如：有线网卡 / 5G WiFi（可留空，用地址兜底）", fontSize = 12.sp) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    FieldLabel("服务端地址（电脑端「远程控制」里显示的地址）")
                    OutlinedTextField(
                        value = hosts,
                        onValueChange = { hosts = it },
                        placeholder = { Text("192.168.0.120:12300 或整条带 token 的地址", fontSize = 12.sp) },
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                        minLines = 1,
                        maxLines = 4,
                        modifier = Modifier.fillMaxWidth(),
                    )
                    FieldLabel("同一台电脑有多块网卡就填多行，每行一个 —— 只占一个标签，哪个通就用哪个")
                    FieldLabel("访问令牌 token")
                    OutlinedTextField(
                        value = token,
                        onValueChange = { token = it },
                        singleLine = true,
                        placeholder = { Text("可留空，页面上也能填", fontSize = 12.sp) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    FieldLabel("经由跳板机（外网连家里：先到「☰ → 服务器」添加 SSH 跳板机；"
                            + "填的地址就变成「跳板机视角的内网地址」）")
                    Box {
                        OutlinedTextField(
                            value = if (jumpIndex <= 0) "直连（默认）"
                            else jumpLabel(jumps[jumpIndex - 1]),
                            onValueChange = {},
                            readOnly = true,
                            singleLine = true,
                            trailingIcon = { Text(if (jumpOpen) "▴" else "▾") },
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable { jumpOpen = true },
                        )
                        DropdownMenu(expanded = jumpOpen, onDismissRequest = { jumpOpen = false }) {
                            DropdownMenuItem(text = { Text("直连（默认）") }, onClick = {
                                jumpIndex = 0; jumpOpen = false
                            })
                            jumps.forEachIndexed { index, jump ->
                                DropdownMenuItem(
                                    text = { Text(jumpLabel(jump)) },
                                    onClick = { jumpIndex = index + 1; jumpOpen = false },
                                )
                            }
                        }
                    }
                    FieldLabel("多网卡探测（有多个地址时怎么探）")
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = !serialProbe, onClick = { serialProbe = false })
                        // 只给较长的那条加 weight，且 fill = false：文字按自身宽度排版，
                        // 放不下时最多被压到分配宽度内换行，绝不会把整行撑爆。
                        // 两条都加 weight 会平分宽度，反而把长的那条挤成两行。
                        Text(
                            "并发探测（推荐）",
                            fontSize = 13.sp, color = palette.fg,
                            modifier = Modifier.weight(1f, fill = false),
                        )
                        Spacer(Modifier.width(12.dp))
                        RadioButton(selected = serialProbe, onClick = { serialProbe = true })
                        Text("串行探测", fontSize = 13.sp, color = palette.fg)
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    if (saveServer(request, group, name, hosts, token, serialProbe, jumpIndex, jumps)) {
                        editRequest = null
                    }
                }) { Text("保存") }
            },
            dismissButton = {
                Row {
                    if (editing) {
                        TextButton(onClick = { profile?.let { confirmDelete(it) } }) {
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

    private fun jumpLabel(jump: JumpServer): String {
        val label = if (JumpStore.text(jump.name).isEmpty()) jump.host else jump.name
        return "经由：$label" + (if (jump.isSsh()) "" else "（仅 Telnet，不能做隧道）")
    }

    /** 保存表单；返回 false = 校验没过（对话框保持打开） */
    private fun saveServer(
        request: EditRequest,
        group: String,
        name: String,
        hosts: String,
        token: String,
        serialProbe: Boolean,
        jumpIndex: Int,
        jumps: List<JumpServer>,
    ): Boolean {
        val editing = request.profile
        val parsed = ServerProfile.parseAddresses(hosts)
        if (parsed.isEmpty()) {
            toast("请填服务端地址")
            return false
        }
        var tokenValue = token.trim()
        if (tokenValue.isEmpty()) {
            // 整条 URL 粘进来的情况
            tokenValue = ServerProfile.tokenFromUrl(hosts)
        }
        val previousAddresses = editing?.let { ArrayList(it.addresses) } ?: ArrayList()
        val previousToken = editing?.let { ServerProfile.text(it.token) } ?: ""

        val target: ServerProfile
        var notice = ""
        if (editing != null) {
            target = editing
            // 编辑 = 以输入为准（顺序也照用户写的来），方便删掉过期的地址
            target.replaceAddresses(parsed)
        } else {
            // 新增：分组名相同的记录就是同一台电脑，并进去，不新建标签
            val merged = findByGroup(group.trim())
            target = if (merged != null) {
                notice = "已并入同一台电脑：" + merged.label()
                merged
            } else {
                val fresh = ServerProfile()
                servers.add(fresh)
                fresh
            }
            for (address in parsed) {
                target.addAddress(address)
            }
        }
        target.group = group.trim()
        target.name = name.trim().ifEmpty { target.host() }
        target.token = tokenValue
        target.probeMode =
            if (serialProbe) ServerProfile.PROBE_SERIAL else ServerProfile.PROBE_PARALLEL
        target.jumpServerId = if (jumpIndex <= 0) "" else jumps[jumpIndex - 1].id
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
                            Text(labels[index], fontSize = 14.sp)
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
        return false
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

    /** 重新开始一轮地址尝试：清掉「已试过」名单，从当前地址再来（还能继续往后换） */
    private fun startAttempt(tab: Tab) {
        tab.tried.clear()
        errorInfo = null
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
