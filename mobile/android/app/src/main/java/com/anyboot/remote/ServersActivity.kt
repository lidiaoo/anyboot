package com.anyboot.remote

import android.annotation.SuppressLint
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.text.format.Formatter
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.jcraft.jsch.ChannelSftp
import com.jcraft.jsch.JSch
import com.jcraft.jsch.KeyPair
import com.jcraft.jsch.Session
import com.jcraft.jsch.SftpATTRS
import java.io.OutputStream
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * 服务器模块（跳板机管理）：FinalShell 风格的服务器列表 + 模块内底部导航五页
 * （服务器 / 会话 / 密钥 / 传输 / 文件）。标题栏右侧的齿轮进「终端设置」。
 * Jetpack Compose / Material 3 重写，业务逻辑平移。
 */
class ServersActivity : ComponentActivity() {

    private val bus = ConfirmBus()

    // ---- 可观察状态 ----
    private val jumps = mutableStateListOf<JumpServer>()
    private val keys = mutableStateListOf<SshKeyEntry>()
    private var currentPage by mutableIntStateOf(0)
    /** 左上角 ☰ 呼出的抽屉侧边栏（与电脑页/电脑列表共用，见 Drawer.kt） */
    private var drawerOpen by mutableStateOf(false)
    private val expanded = mutableStateListOf<Boolean>()
    private var search by mutableStateOf("")
    private var tick by mutableIntStateOf(0)   // 会话注册表变化时自增
    private var transferTick by mutableIntStateOf(0)   // 传输任务有变时自增

    // 编辑/删除/生成弹窗
    private var editTarget by mutableStateOf<JumpServer?>(null)
    private var editOpen by mutableStateOf(false)
    private var deleteTarget by mutableStateOf<JumpServer?>(null)
    private var killTarget by mutableStateOf<TerminalSession?>(null)
    private var generateKeyOpen by mutableStateOf(false)
    private var manualPassword by mutableStateOf<ManualPwdRequest?>(null)

    private class ManualPwdRequest(val server: JumpServer, val onReady: Runnable)

    // 文件页状态
    private var sftpServerId by mutableStateOf("")
    private var sftpPath by mutableStateOf("/")
    private var sftpEntries by mutableStateOf<List<SftpOps.Entry>>(emptyList())
    private var sftpBusy by mutableStateOf(false)
    private var sftpError by mutableStateOf("")
    /** 已经拉取过的「服务器|路径」标记 —— 空目录也不能反复重拉（否则死循环） */
    private var sftpLoadedKey by mutableStateOf("")

    // 文件页：工具行（排序 / 显示隐藏 / 过滤 / 操作进行中）
    private var sftpSort by mutableStateOf(SftpSort.NAME)
    private var sftpSortDesc by mutableStateOf(false)
    private var sftpShowHidden by mutableStateOf(false)
    private var sftpFilterOpen by mutableStateOf(false)
    private var sftpFilter by mutableStateOf("")
    /** 非空 = 正有一个目录操作在跑，期间不再接第二个 */
    private var sftpOpLabel by mutableStateOf("")

    // 文件页：应用内剪贴板（不碰手机的系统剪贴板；「复制路径」才走系统剪贴板）
    private var sftpClip by mutableStateOf(SftpClip.NONE)
    private var sftpClipServer by mutableStateOf("")
    private var sftpClipPaths by mutableStateOf<List<String>>(emptyList())
    private var sftpClipNames by mutableStateOf("")

    // 文件页：弹窗
    private var sftpSheetEntry by mutableStateOf<SftpOps.Entry?>(null)
    private var sftpAskDelete by mutableStateOf<SftpOps.Entry?>(null)
    private var sftpAskRename by mutableStateOf<SftpOps.Entry?>(null)
    private var sftpAskDetail by mutableStateOf<SftpOps.Entry?>(null)
    private var sftpMkdirOpen by mutableStateOf(false)
    private var sftpLocateOpen by mutableStateOf(false)
    private var sftpPick by mutableStateOf<SftpPick?>(null)

    /** 排序键（目录恒定排在最前，见 [orderEntries]） */
    private enum class SftpSort { NAME, SIZE, TIME }

    /** 应用内剪贴板的两种模式 */
    private enum class SftpClip { NONE, COPY, CUT }

    /**
     * 「复制到… / 移动到…」的目标目录选择器请求。刻意不复用剪贴板 ——
     * 用户点「复制到…」时不该把手上那份剪贴板内容悄悄换掉。
     */
    private class SftpPick(
        val cut: Boolean,
        val paths: List<String>,
        val label: String,
        val startDir: String,
    )

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Ui.applySystemBars(this)
        jumps.addAll(JumpStore.loadJumps(this))
        keys.addAll(JumpStore.loadKeys(this))
        // 装回上次进程留下的传输任务（暂停中的那条还在，半截文件也认领得回来）
        TransferManager.restore(this)
        setContent { Root() }
    }

    override fun onResume() {
        super.onResume()
        jumps.clear(); jumps.addAll(JumpStore.loadJumps(this))
        keys.clear(); keys.addAll(JumpStore.loadKeys(this))
        tick++
    }

    @Composable
    private fun Root() {
        AppTheme {
            val palette = LocalPalette.current
            val importLauncher = rememberLauncherForActivityResult(
                ActivityResultContracts.OpenDocument()
            ) { uri -> if (uri != null) importKey(uri) }

            Box(Modifier.fillMaxSize()) {
            Column(Modifier.fillMaxSize().background(palette.base)) {
                // 顶部栏（设计稿：标题 + 计数/状态 pill + ⋮ + FAB；齿轮 = 终端设置）
                TopBar(importLauncher)

                // 五个页面：设计稿底部导航四项（服务器/密钥/文件）+ ⋮ 里的会话/传输。
                // 「终端」导航项直接跳最近会话的终端页，不是本 Activity 里的一页。
                Box(Modifier.fillMaxWidth().weight(1f)) {
                    when (currentPage) {
                        0 -> ServersPage()
                        1 -> KeysPage(importLauncher)
                        2 -> FilesPage()
                        3 -> SessionsPage()
                        else -> TransfersPage()
                    }
                }

                // 文件页底部的传输状态条（设计稿 files 屏：spin + 进度 + 百分比）
                if (currentPage == 2) TransferStatusBar()

                BottomNav()
            }

            if (drawerOpen) AppDrawerOverlay(DRAWER_SERVERS) { drawerOpen = false }
            }

            if (editOpen) EditJumpDialog(editTarget)
            deleteTarget?.let { DeleteJumpDialog(it) }
            killTarget?.let { KillDialog(it) }
            if (generateKeyOpen) GenerateKeyDialog()
            manualPassword?.let { ManualPasswordDialog(it) }
            ConfirmDialog(bus)
        }

        // 会话注册表变化 → 会话页/顶栏刷新；传输任务变化 → 传输条/传输页/文件页刷新
        DisposableEffect(Unit) {
            val listener = { tick += 1 }
            SessionEvents.register(listener)
            val transferListener = { transferTick += 1 }
            TransferEvents.register(transferListener)
            onDispose {
                SessionEvents.unregister(listener)
                TransferEvents.unregister(transferListener)
            }
        }
    }

    // ------------------------------------------------------------------
    // 顶部栏 / 底部导航（设计稿公共骨架）
    // ------------------------------------------------------------------

    /** 顶部栏：‹ + 标题 + 状态 pill + 齿轮(终端设置，仅服务器页) + ⋮ + FAB */
    @Composable
    private fun TopBar(
        importLauncher: androidx.activity.compose.ManagedActivityResultLauncher<Array<String>, Uri?>,
    ) {
        val palette = LocalPalette.current
        tick // 订阅会话注册表变化：在线数 / SFTP 状态 pill 要跟着刷
        var moreOpen by remember { mutableStateOf(false) }
        var fabMenuOpen by remember { mutableStateOf(false) }

        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .background(palette.base)
                .padding(start = 4.dp, top = 10.dp, end = 20.dp, bottom = 8.dp),
        ) {
            BarIcon("☰", "侧边栏") { drawerOpen = true }
            Text(
                when (currentPage) {
                    1 -> "密钥"; 2 -> "文件"; 3 -> "会话"; 4 -> "传输"; else -> "服务器"
                },
                fontSize = 20.sp,
                fontWeight = FontWeight.Bold,
                color = palette.fg,
            )
            Spacer(Modifier.width(10.dp))
            PagePill()
            Spacer(Modifier.weight(1f))
            if (currentPage == 0) {
                Icon(
                    Icons.Filled.Settings,
                    contentDescription = "终端设置",
                    tint = palette.fgDim,
                    modifier = Modifier
                        .size(36.dp)
                        .clickable {
                            startActivity(
                                Intent(this@ServersActivity, TerminalSettingsActivity::class.java)
                            )
                        }
                        .padding(8.dp),
                )
            }
            Box {
                BarIcon("⋮", "更多") { moreOpen = true }
                DropdownMenu(expanded = moreOpen, onDismissRequest = { moreOpen = false }) {
                    DropdownMenuItem(text = { Text("会话（进行中 / 历史）") }, onClick = {
                        moreOpen = false; currentPage = 3
                    })
                    DropdownMenuItem(text = { Text("传输管理") }, onClick = {
                        moreOpen = false; currentPage = 4
                    })
                    DropdownMenuItem(text = { Text("终端设置") }, onClick = {
                        moreOpen = false
                        startActivity(Intent(this@ServersActivity, TerminalSettingsActivity::class.java))
                    })
                }
            }
            if (currentPage == 0 || currentPage == 1) {
                // FAB：服务器页 = 添加服务器；密钥页 = 生成 / 导入菜单（设计稿为「生成密钥」）
                Box {
                    Box(
                        Modifier
                            .size(36.dp)
                            .clip(CircleShape)
                            .background(palette.accent)
                            .clickable {
                                if (currentPage == 0) {
                                    editTarget = null
                                    editOpen = true
                                } else {
                                    fabMenuOpen = true
                                }
                            },
                        contentAlignment = Alignment.Center,
                    ) { Icon(Icons.Filled.Add, "添加", tint = palette.onAccent, modifier = Modifier.size(20.dp)) }
                    DropdownMenu(expanded = fabMenuOpen, onDismissRequest = { fabMenuOpen = false }) {
                        DropdownMenuItem(text = { Text("生成密钥对") }, onClick = {
                            fabMenuOpen = false; generateKeyOpen = true
                        })
                        DropdownMenuItem(text = { Text("导入私钥文件") }, onClick = {
                            fabMenuOpen = false
                            importLauncher.launch(arrayOf("*/*"))
                        })
                    }
                }
            }
        }
    }

    /** 标题旁的状态 pill：服务器页=在线数；密钥页=把数；文件页=SFTP 状态（都是真实数据） */
    @Composable
    private fun PagePill() {
        val palette = LocalPalette.current
        when (currentPage) {
            0 -> {
                tick
                val online = jumps.count { it.isSsh() && SshSessionManager.shared().isUp(it.id) }
                PillText("$online 在线")
            }
            1 -> PillText("${keys.size} 把")
            2 -> {
                val up = sftpServerId.isNotEmpty() && SshSessionManager.shared().isUp(sftpServerId)
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .clip(RoundedCornerShape(999.dp))
                        .background(if (up) palette.accent.copy(alpha = 0.1f) else palette.elevate)
                        .padding(horizontal = 8.dp, vertical = 3.dp),
                ) {
                    Box(
                        Modifier
                            .size(6.dp)
                            .clip(CircleShape)
                            .background(if (up) palette.accent else palette.fgFaint)
                    )
                    Spacer(Modifier.width(5.dp))
                    Text(
                        if (up) "SFTP 就绪" else "未连接",
                        fontSize = 10.sp,
                        color = if (up) palette.accent else palette.fgDim,
                    )
                }
            }
        }
    }

    @Composable
    private fun PillText(text: String) {
        val palette = LocalPalette.current
        Text(
            text,
            fontSize = 10.sp,
            fontFamily = FontFamily.Monospace,
            color = palette.fgDim,
            modifier = Modifier
                .clip(RoundedCornerShape(999.dp))
                .background(palette.elevate)
                .border(1.dp, palette.line, RoundedCornerShape(999.dp))
                .padding(horizontal = 8.dp, vertical = 3.dp),
        )
    }

    /** 底部导航（设计稿四项：服务器 / 终端 / 密钥 / 文件；终端跳最近会话） */
    @Composable
    private fun BottomNav() {
        val palette = LocalPalette.current
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .drawBehind { drawLine(palette.line, Offset.Zero, Offset(size.width, 0f), 1.dp.toPx()) }
                .background(palette.card)
                .navigationBarsPadding()
                .padding(top = 8.dp, bottom = 6.dp),
        ) {
            NavItem("服务器", active = currentPage == 0, onClick = { currentPage = 0 },
                modifier = Modifier.weight(1f)) { tint, size ->
                ServerStackIcon(tint, size)
            }
            NavItem("终端", active = false, onClick = { openLatestSession() },
                modifier = Modifier.weight(1f)) { tint, size ->
                TermIcon(tint, size)
            }
            NavItem("密钥", active = currentPage == 1, onClick = { currentPage = 1 },
                modifier = Modifier.weight(1f)) { tint, size ->
                KeyIcon(tint, size)
            }
            NavItem("文件", active = currentPage == 2, onClick = { currentPage = 2 },
                modifier = Modifier.weight(1f)) { tint, size ->
                FolderIcon(tint, size)
            }
        }
    }

    @Composable
    private fun NavItem(
        label: String,
        active: Boolean,
        onClick: () -> Unit,
        modifier: Modifier = Modifier,
        icon: @Composable (Color, androidx.compose.ui.unit.Dp) -> Unit,
    ) {
        val palette = LocalPalette.current
        val tint = if (active) palette.accent else palette.fgFaint
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = modifier
                .clickable(onClick = onClick)
                .padding(vertical = 2.dp),
        ) {
            icon(tint, 20.dp)
            Text(
                label,
                fontSize = 10.sp,
                fontWeight = if (active) FontWeight.Medium else FontWeight.Normal,
                color = tint,
                modifier = Modifier.padding(top = 3.dp),
            )
        }
    }

    /** 「终端」导航项：跳最近一个活会话的终端页；没有就说明去哪开 */
    private fun openLatestSession() {
        val live = TerminalRegistry.all()
        if (live.isEmpty()) {
            toast("没有进行中的会话 —— 到「服务器」页点「终端」开始", long = true)
            return
        }
        val intent = Intent(this, TerminalActivity::class.java)
        intent.putExtra("jumpId", live.last().server.id)
        startActivity(intent)
    }

    /** 文件页底部的传输状态条：有活动任务才出现，点一下进「传输」页看全部 */
    @Composable
    private fun TransferStatusBar() {
        transferTick
        val active = TransferManager.active()
        if (active.isEmpty()) return
        val task = active.first()
        val palette = LocalPalette.current
        val fraction = task.fraction()
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .drawBehind { drawLine(palette.line, Offset.Zero, Offset(size.width, 0f), 1.dp.toPx()) }
                .background(palette.card)
                .clickable { currentPage = 4 }
                .padding(horizontal = 20.dp, vertical = 10.dp),
        ) {
            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = palette.accent)
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    task.name,
                    fontSize = 11.sp,
                    color = palette.fg,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Spacer(Modifier.height(4.dp))
                Box(
                    Modifier
                        .fillMaxWidth()
                        .height(6.dp)
                        .clip(RoundedCornerShape(3.dp))
                        .background(palette.elevate)
                ) {
                    if (task.state == TransferManager.STATE_PAUSED) {
                        Box(
                            Modifier
                                .fillMaxWidth(fraction.coerceIn(0.01f, 1f))
                                .fillMaxHeight()
                                .background(palette.warn)
                        )
                    } else {
                        Box(
                            Modifier
                                .fillMaxWidth(fraction.coerceIn(0.02f, 1f))
                                .fillMaxHeight()
                                .background(palette.accent)
                        )
                    }
                }
            }
            Spacer(Modifier.width(10.dp))
            Text(
                when {
                    task.state == TransferManager.STATE_PAUSED -> "暂停"
                    task.total <= 0 -> "…"
                    else -> "${(fraction * 100).toInt()}%"
                },
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
                color = if (task.state == TransferManager.STATE_PAUSED) palette.warn else palette.accent,
            )
        }
    }

    // ------------------------------------------------------------------
    // 会话页
    // ------------------------------------------------------------------

    @Composable
    private fun SessionsPage() {
        tick // 订阅会话注册表变化
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            SectionTitle("进行中")
            val live = TerminalRegistry.all()
            if (live.isEmpty()) {
                EmptyHint("没有进行中的会话。到「服务器」页点「▶ 打开终端」开始。")
            }
            for (session in live) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable {
                            val intent = Intent(this@ServersActivity, TerminalActivity::class.java)
                            intent.putExtra("jumpId", session.server.id)
                            startActivity(intent)
                        }
                        .padding(start = 12.dp, top = 12.dp, end = 8.dp, bottom = 12.dp),
                ) {
                    Text("●", color = LocalPalette.current.ok)
                    Column(Modifier.weight(1f).padding(horizontal = 8.dp)) {
                        PrimaryText(session.server.name)
                        SecondaryText("已连接 " + humanDuration(System.currentTimeMillis() - session.startedAt))
                    }
                    ChipButton("结束") { killTarget = session }
                }
            }

            SectionTitle("历史")
            val history = JumpStore.loadHistory(this@ServersActivity)
            if (history.isEmpty()) {
                EmptyHint("还没有会话历史。")
            }
            val format = SimpleDateFormat("MM-dd HH:mm", Locale.getDefault())
            for (entry in history) {
                if (entry.endedAt <= 0) continue
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(start = 12.dp, top = 12.dp, end = 8.dp, bottom = 12.dp),
                ) {
                    Column(Modifier.weight(1f)) {
                        PrimaryText(if (JumpStore.text(entry.serverName).isEmpty()) "服务器" else entry.serverName)
                        val duration = Math.max(0, entry.endedAt - Math.max(entry.startedAt, 0))
                        SecondaryText(format.format(Date(entry.startedAt)) + " · 时长 " + humanDuration(duration))
                    }
                }
            }
            if (history.isNotEmpty()) {
                TextButton(
                    onClick = {
                        ServerStore.prefs(this@ServersActivity).edit()
                            .putString("session_history", "").apply()
                        tick++
                    },
                    modifier = Modifier.align(Alignment.CenterHorizontally),
                ) { Text("清空历史") }
            }
        }
    }

    /** 「结束会话」确认弹窗（状态驱动，别在点击回调里直接组 UI） */
    @Composable
    private fun KillDialog(session: TerminalSession) {
        AlertDialog(
            onDismissRequest = { killTarget = null },
            title = { Text("结束会话") },
            text = { Text("结束「${session.server.name}」的终端会话吗？") },
            confirmButton = {
                TextButton(onClick = {
                    killTarget = null
                    session.close("用户结束")
                    TerminalRegistry.remove(session.server.id)
                    tick++
                }) { Text("结束") }
            },
            dismissButton = { TextButton(onClick = { killTarget = null }) { Text("取消") } },
        )
    }

    // ------------------------------------------------------------------
    // 终端页（服务器列表）
    // ------------------------------------------------------------------

    @Composable
    private fun ServersPage() {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            OutlinedTextField(
                value = search,
                onValueChange = { search = it.trim() },
                singleLine = true,
                placeholder = { Text("搜索名称 / 地址", fontSize = 14.sp) },
                leadingIcon = { Icon(Icons.Filled.Search, "搜索", tint = LocalPalette.current.fgFaint) },
                shape = RoundedCornerShape(12.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(start = 20.dp, top = 4.dp, end = 20.dp, bottom = 8.dp),
            )

            if (jumps.isEmpty()) {
                EmptyHint(
                    "还没有服务器。\n\nSSH 跳板机：外网连家里的必经之路 —— 手机先 SSH 登录它，"
                            + "再由它访问局域网内的 anyboot 电脑。\nTelnet：只有实验性终端。"
                )
                TextButton(
                    onClick = { editTarget = null; editOpen = true },
                    modifier = Modifier.align(Alignment.CenterHorizontally),
                ) { Text("＋ 添加服务器") }
                return@Column
            }
            while (expanded.size < jumps.size) expanded.add(false)

            val query = search.lowercase(Locale.ROOT)
            var last = true
            jumps.forEachIndexed { index, server ->
                val name = JumpStore.text(server.name)
                if (query.isNotEmpty()
                    && !name.lowercase(Locale.ROOT).contains(query)
                    && !server.host.lowercase(Locale.ROOT).contains(query)
                ) {
                    return@forEachIndexed
                }
                last = false
                ServerCard(server, index)
                Spacer(Modifier.height(10.dp))
            }
            if (!last) Spacer(Modifier.height(4.dp))
        }
    }

    /**
     * 服务器卡片（设计稿 servers 屏）：44dp 图标块 + 名称/协议徽标 + mono 地址 +
     * 右侧 ms 与状态点；点击展开三格信息与终端/文件/测试/编辑四钮。
     */
    @Composable
    private fun ServerCard(server: JumpServer, index: Int) {
        val palette = LocalPalette.current
        val expand = index < expanded.size && expanded[index]
        val name = JumpStore.text(server.name)
        val up = server.isSsh() && SshSessionManager.shared().isUp(server.id)
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(16.dp))
                .background(palette.card)
                .border(
                    1.dp,
                    if (expand) palette.accent.copy(alpha = 0.4f) else palette.line,
                    RoundedCornerShape(16.dp),
                )
                .clickable {
                    while (expanded.size < jumps.size) expanded.add(false)
                    expanded[index] = !expanded[index]
                }
                .padding(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                // 图标块：SSH 在线 = 绿底图标；其余抬升底
                Box(
                    Modifier
                        .size(44.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(
                            if (up) palette.accent.copy(alpha = 0.1f) else palette.elevate
                        )
                        .border(
                            1.dp,
                            if (up) palette.accent.copy(alpha = 0.3f) else palette.line,
                            RoundedCornerShape(12.dp),
                        ),
                    contentAlignment = Alignment.Center,
                ) {
                    ServerStackIcon(if (up) palette.accent else palette.fgDim, 22.dp)
                }
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            if (name.isEmpty()) server.addressLabel() else name,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.SemiBold,
                            color = palette.fg,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Spacer(Modifier.width(6.dp))
                        Text(
                            server.protocolLabel(),
                            fontSize = 9.sp,
                            fontFamily = FontFamily.Monospace,
                            color = if (server.isSsh()) palette.fgDim else palette.warn,
                            modifier = Modifier
                                .clip(RoundedCornerShape(4.dp))
                                .background(palette.elevate)
                                .padding(horizontal = 4.dp, vertical = 1.dp),
                        )
                    }
                    // 名称没填时主标题已经是地址了，副行别再重复一遍 —— 换成用户/端口摘要
                    val subtitle = buildString {
                        if (name.isEmpty()) {
                            append("用户 ")
                            append(if (JumpStore.text(server.username).isEmpty()) "root" else server.username)
                            append(" · 端口 ")
                            append(server.port())
                        } else {
                            append(server.addressLabel())
                        }
                        if (!server.isSsh()) append("（实验性终端）")
                    }
                    Text(
                        subtitle,
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                        color = palette.fgDim,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(top = 2.dp),
                    )
                }
                Column(horizontalAlignment = Alignment.End) {
                    if (server.lastRttMs >= 0) {
                        Text(
                            "${server.lastRttMs}ms",
                            fontSize = 12.sp,
                            fontFamily = FontFamily.Monospace,
                            color = palette.accent,
                        )
                    }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        val (dotColor, label) = when {
                            server.isSsh() && up -> palette.accent to "在线"
                            !server.isSsh() -> palette.warn to "实验性"
                            else -> palette.fgFaint to "未连接"
                        }
                        Box(
                            Modifier
                                .size(6.dp)
                                .clip(CircleShape)
                                .background(dotColor)
                        )
                        Spacer(Modifier.width(4.dp))
                        Text(label, fontSize = 10.sp, color = dotColor)
                    }
                }
            }

            if (expand) {
                Spacer(Modifier.height(14.dp))
                // 详情展开：分隔线 + 三格信息 + 四格按钮
                Box(
                    Modifier
                        .fillMaxWidth()
                        .height(1.dp)
                        .background(palette.line)
                )
                Spacer(Modifier.height(12.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    InfoCell("用户", if (JumpStore.text(server.username).isEmpty()) "root" else server.username,
                        Modifier.weight(1f))
                    InfoCell("认证", server.authLabel(), Modifier.weight(1f))
                    InfoCell("端口", server.port().toString(), Modifier.weight(1f))
                }
                if (JumpStore.text(server.note).isNotEmpty()) {
                    SecondaryText("备注：" + server.note, Modifier.padding(top = 8.dp))
                }
                if (server.isSsh()) {
                    SecondaryText(
                        "经由此服务器的电脑：" + JumpStore.computersVia(this@ServersActivity, server.id) + " 台",
                        Modifier.padding(top = 2.dp),
                    )
                }
                if (server.isSsh() && up) {
                    SecondaryText("SSH 会话在线（隧道与终端共享）", Modifier.padding(top = 2.dp))
                }
                Spacer(Modifier.height(12.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (server.isSsh()) {
                        CardAction("终端", filled = true, Modifier.weight(1f)) { openTerminal(server) }
                        CardAction("文件", filled = false, Modifier.weight(1f)) {
                            currentPage = 2
                            sftpServerId = server.id
                            // 走统一的切目录入口：它会顺手把「加载中」点亮，免得闪一下「（空目录）」
                            gotoDir("/", true)
                        }
                    }
                    CardAction("测试", filled = false, Modifier.weight(1f)) { testConnection(server) }
                    CardAction("编辑", filled = false, Modifier.weight(1f)) { editTarget = server; editOpen = true }
                }
            }
        }
    }

    /** 展开区的信息格（用户 / 认证 / 端口） */
    @Composable
    private fun InfoCell(label: String, value: String, modifier: Modifier = Modifier) {
        val palette = LocalPalette.current
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = modifier
                .clip(RoundedCornerShape(8.dp))
                .background(palette.elevate.copy(alpha = 0.6f))
                .padding(vertical = 8.dp),
        ) {
            Text(
                value,
                fontSize = 12.sp,
                color = palette.fg,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = 6.dp),
            )
            Text(label, fontSize = 9.sp, color = palette.fgFaint, modifier = Modifier.padding(top = 1.dp))
        }
    }

    /** 展开区的操作钮（终端为实底强调色，其余描边抬升底） */
    @Composable
    private fun CardAction(label: String, filled: Boolean, modifier: Modifier = Modifier, onClick: () -> Unit) {
        val palette = LocalPalette.current
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
            modifier = modifier
                .height(40.dp)
                .clip(RoundedCornerShape(12.dp))
                .background(if (filled) palette.accent else palette.elevate)
                .border(
                    1.dp,
                    if (filled) Color.Transparent else palette.line,
                    RoundedCornerShape(12.dp),
                )
                .clickable(onClick = onClick),
        ) {
            when (label) {
                "终端" -> TermIcon(palette.onAccent, 15.dp)
                "文件" -> FolderIcon(palette.fg, 15.dp)
                "测试" -> Icon(
                    Icons.Filled.Refresh, label, tint = palette.fg,
                    modifier = Modifier.size(15.dp),
                )
                "编辑" -> Icon(
                    Icons.Filled.Edit, label, tint = palette.fgDim,
                    modifier = Modifier.size(15.dp),
                )
            }
            Text(
                label,
                fontSize = 9.sp,
                fontWeight = if (filled) FontWeight.SemiBold else FontWeight.Normal,
                color = if (filled) palette.onAccent else if (label == "编辑") palette.fgDim else palette.fg,
            )
        }
    }

    private fun openTerminal(server: JumpServer) {
        val intent = Intent(this, TerminalActivity::class.java)
        intent.putExtra("jumpId", server.id)
        startActivity(intent)
    }

    /** 测试连接：SSH 握手 / Telnet TCP，量往返 */
    private fun testConnection(server: JumpServer) {
        // 密码认证但本机没存上密码 → 先弹输入框（应急通道，本次连接有效）
        if (server.isSsh() && SshSessionManager.shared().needsManualPassword(server)) {
            manualPassword = ManualPwdRequest(server) { testConnection(server) }
            return
        }
        toast("正在测试 ${server.addressLabel()} …")
        val appContext = applicationContext
        Thread({
            var result: String
            val started = System.currentTimeMillis()
            try {
                if (server.isSsh()) {
                    SshSessionManager.shared().ensureSession(
                        appContext, server
                    ) { title, message -> bus.confirm(title, message) }
                } else {
                    java.net.Socket().use { socket ->
                        socket.connect(
                            java.net.InetSocketAddress(server.hostname(), server.port()), 8000
                        )
                    }
                }
                result = "连接成功（${System.currentTimeMillis() - started}ms）"
                server.lastRttMs = System.currentTimeMillis() - started
                server.lastSeen = System.currentTimeMillis()
                JumpStore.saveJumps(this, jumps)
            } catch (error: Exception) {
                result = "连接失败：" + (error.message ?: error.javaClass.simpleName)
            }
            val message = result
            runOnUiThread {
                tick++
                toast(message, long = true)
            }
        }, "anyboot-test-conn").start()
    }

    @Composable
    private fun DeleteJumpDialog(server: JumpServer) {
        val via = JumpStore.computersVia(this, server.id)
        val viaServers = jumps.count { it.id != server.id && it.jumpServerId == server.id }
        AlertDialog(
            onDismissRequest = { deleteTarget = null },
            title = { Text("删除服务器") },
            text = {
                Text(
                    "确定删除「" + JumpStore.text(server.name) + "」吗？"
                            + (if (via > 0) "\n\n注意：有 $via 台电脑配置了经由它连接，删除后这些电脑会退回直连。" else "")
                            + (if (viaServers > 0) "\n\n另有 $viaServers 台服务器经由它中转，删除后退回直连。" else "")
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    deleteTarget = null
                    SshSessionManager.shared().shutdown(server.id)
                    val session = TerminalRegistry.get(server.id)
                    if (session != null) {
                        session.close("服务器已删除")
                        TerminalRegistry.remove(server.id)
                    }
                    jumps.remove(server)
                    // 经由它中转的其他服务器退回直连（与电脑经由跳板机删除时同口径）
                    for (other in jumps) {
                        if (server.id == other.jumpServerId) {
                            other.jumpServerId = ""
                        }
                    }
                    JumpStore.saveJumps(this, jumps)
                }) { Text("删除", color = LocalPalette.current.bad) }
            },
            dismissButton = {
                TextButton(onClick = { deleteTarget = null }) { Text("取消") }
            },
        )
    }

    // ------------------------------------------------------------------
    // 添加 / 编辑服务器
    // ------------------------------------------------------------------

    /**
     * 添加 / 编辑服务器（设计稿 edit 屏：底部上拉弹层 + 把手 + 全宽保存钮）。
     * 原AlertDialog 里的全部字段平移过来；「删除」从列表卡片挪进这里（设计稿卡片四钮里没有删除）。
     */
    @OptIn(ExperimentalMaterial3Api::class)
    @Composable
    private fun EditJumpDialog(existing: JumpServer?) {
        val palette = LocalPalette.current
        val editing = existing != null

        var name by remember(existing) { mutableStateOf(existing?.name ?: "") }
        var isTelnet by remember(existing) { mutableStateOf(editing && existing?.isSsh() == false) }
        // 设计稿把地址 / 端口拆成两格；存储仍是 host:port 合一，保存时拼回去
        var hostInput by remember(existing) { mutableStateOf(existing?.hostname() ?: "") }
        var portInput by remember(existing) {
            mutableStateOf(if (editing) existing?.port()?.toString() ?: "" else "")
        }
        var username by remember(existing) { mutableStateOf(existing?.username ?: "") }
        var authKey by remember(existing) {
            mutableStateOf(editing && JumpServer.AUTH_KEY == existing?.authType)
        }
        var password by remember(existing) { mutableStateOf("") }
        // 默认就存：以前这里是「编辑一台没存过密码的服务器 → 不勾」，
        // 而那个复选框又被上面协议行的空白顶出了视口，
        // 用户填了密码却在保存时被丢成空串，连接时只好再输一次「临时密码」。
        var savePassword by remember(existing) { mutableStateOf(true) }
        var keyIndex by remember(existing) {
            mutableStateOf(
                if (editing && JumpServer.AUTH_KEY == existing?.authType) {
                    val found = keys.indexOfFirst { it.id == existing.keyId }
                    if (found >= 0) found else 0
                } else 0
            )
        }
        var keyOpen by remember { mutableStateOf(false) }
        var passphrase by remember(existing) { mutableStateOf("") }
        var note by remember(existing) { mutableStateOf(existing?.note ?: "") }
        // 经由跳板机：存的是 JumpServer.id（空 = 直连）；候选排除自己（编辑时），不许自引
        var jumpId by remember(existing) { mutableStateOf(existing?.jumpServerId ?: "") }
        var jumpOpen by remember { mutableStateOf(false) }
        // 终端启动路径 / 默认 shell（每台服务器各自配，留空 = 远端家目录 + 登录 shell）
        var termPath by remember(existing) { mutableStateOf(existing?.termPath ?: "") }
        var termShell by remember(existing) { mutableStateOf(existing?.termShell ?: "") }
        val jumpChoices = jumps.filter { existing == null || it.id != existing.id }
        // 已存密码要回显 —— 否则用户没法确认到底存没存（解不开等于没存，一并暴露）
        val savedPassword = remember(existing) {
            if (editing) KeyVault.decrypt(existing?.passwordEnc ?: "") else ""
        }

        /** 地址框 + 端口框 → 存储用的 host(:port)；端口留空交给 applyHostInput 补协议默认值 */
        fun combinedHost(): String {
            val port = portInput.trim()
            return if (port.isNotEmpty()) hostInput.trim() + ":" + port else hostInput.trim()
        }

        ModalBottomSheet(
            onDismissRequest = { editOpen = false },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = palette.card,
            dragHandle = {
                // 设计稿的把手：40×4 圆头
                Box(
                    Modifier
                        .padding(top = 12.dp, bottom = 4.dp)
                        .width(40.dp)
                        .height(4.dp)
                        .clip(RoundedCornerShape(2.dp))
                        .background(palette.lineStrong)
                )
            },
        ) {
            Column(
                Modifier
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .imePadding()
                    .padding(horizontal = 20.dp)
                    .padding(bottom = 28.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        if (editing) "编辑服务器" else "添加服务器",
                        fontSize = 18.sp,
                        fontWeight = FontWeight.Bold,
                        color = palette.fg,
                        modifier = Modifier.weight(1f),
                    )
                    Icon(
                        Icons.Filled.Close,
                        contentDescription = "关闭",
                        tint = palette.fgDim,
                        modifier = Modifier
                            .size(32.dp)
                            .clickable { editOpen = false }
                            .padding(8.dp),
                    )
                }
                Spacer(Modifier.height(12.dp))

                FieldLabel("名称")
                OutlinedTextField(
                    value = name, onValueChange = { name = it }, singleLine = true,
                    placeholder = { Text("例如：pve / 家里路由器", fontSize = 12.sp) },
                    shape = RoundedCornerShape(12.dp),
                    modifier = Modifier.fillMaxWidth(),
                )
                FieldLabel("协议")
                Segmented(listOf("SSH", "Telnet（实验性）"), if (isTelnet) 1 else 0) {
                    isTelnet = it == 1
                }
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Column(Modifier.weight(1f)) {
                        FieldLabel("地址")
                        OutlinedTextField(
                            value = hostInput, onValueChange = { hostInput = it }, singleLine = true,
                            placeholder = { Text("host 或 user@host", fontSize = 12.sp) },
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                            shape = RoundedCornerShape(12.dp),
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                    Column(Modifier.width(88.dp)) {
                        FieldLabel("端口")
                        OutlinedTextField(
                            value = portInput, onValueChange = { portInput = it.filter { c -> c.isDigit() } },
                            singleLine = true,
                            placeholder = { Text(if (isTelnet) "23" else "22", fontSize = 12.sp) },
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                            shape = RoundedCornerShape(12.dp),
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }
                FieldLabel("用户名")
                OutlinedTextField(
                    value = username, onValueChange = { username = it }, singleLine = true,
                    placeholder = { Text("root", fontSize = 12.sp) },
                    shape = RoundedCornerShape(12.dp),
                    modifier = Modifier.fillMaxWidth(),
                )
                FieldLabel("认证方式")
                Segmented(listOf("密码", "密钥"), if (authKey) 1 else 0) {
                    authKey = it == 1
                }
                if (!authKey) {
                    OutlinedTextField(
                        value = password,
                        onValueChange = { password = it },
                        singleLine = true,
                        placeholder = {
                            Text(
                                when {
                                    savedPassword.isNotEmpty() -> "已保存密码（可改）"
                                    // 有密文却解不开：密钥库那把钥匙换过（老版本每次调用都覆盖），
                                    // 留空保存只会把这份坏密文继续留着，必须提示重输
                                    JumpStore.text(existing?.passwordEnc).isNotEmpty() ->
                                        "已存密码已失效，请重新输入一次"
                                    else -> "留空保持原密码不变"
                                },
                                fontSize = 12.sp,
                            )
                        },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        shape = RoundedCornerShape(12.dp),
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 12.dp),
                    )
                    // 「保存密码」卡片（设计稿样式）
                    Row(
                        verticalAlignment = Alignment.Top,
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 12.dp)
                            .clip(RoundedCornerShape(12.dp))
                            .background(palette.base)
                            .border(1.dp, palette.line, RoundedCornerShape(12.dp))
                            .padding(horizontal = 12.dp, vertical = 10.dp),
                    ) {
                        Checkbox(checked = savePassword, onCheckedChange = { savePassword = it })
                        Column(Modifier.padding(top = 2.dp)) {
                            Text("保存密码 / 凭据", fontSize = 14.sp, color = palette.fg)
                            Text(
                                "加密存本机（Keystore），连接免输；不勾则清除已存凭据",
                                fontSize = 11.sp,
                                color = palette.fgFaint,
                                modifier = Modifier.padding(top = 2.dp),
                            )
                        }
                    }
                } else {
                    FieldLabel("密钥")
                    Box {
                        OutlinedTextField(
                            value = if (keys.isEmpty()) "（还没有密钥，先到「密钥」页导入/生成）"
                            else keys[keyIndex.coerceIn(0, keys.size - 1)].let {
                                JumpStore.text(it.name) + "（" + it.algorithm + "）"
                            },
                            onValueChange = {}, readOnly = true, singleLine = true,
                            // 必须 disabled：readOnly 的 TextField 会吃掉点击，clickable 收不到（详见 disabledFieldColors）
                            enabled = false,
                            shape = RoundedCornerShape(12.dp),
                            colors = disabledFieldColors(palette.fg, palette.lineStrong, palette.fgDim),
                            trailingIcon = { Text(if (keyOpen) "▴" else "▾") },
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable(enabled = keys.isNotEmpty()) { keyOpen = true },
                        )
                        DropdownMenu(expanded = keyOpen, onDismissRequest = { keyOpen = false }) {
                            keys.forEachIndexed { index, entry ->
                                DropdownMenuItem(
                                    text = { Text(JumpStore.text(entry.name) + "（" + entry.algorithm + "）") },
                                    onClick = { keyIndex = index; keyOpen = false },
                                )
                            }
                        }
                    }
                    OutlinedTextField(
                        value = passphrase,
                        onValueChange = { passphrase = it },
                        singleLine = true,
                        placeholder = { Text("密钥口令（可空）", fontSize = 12.sp) },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        shape = RoundedCornerShape(12.dp),
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 12.dp),
                    )
                }
                // 终端启动路径 / shell（可选）：目录打不通时终端里报错、照常落家目录
                FieldLabel("终端启动路径（可选）")
                OutlinedTextField(
                    value = termPath, onValueChange = { termPath = it }, singleLine = true,
                    placeholder = { Text("如 /data/logs，留空 = 远端家目录", fontSize = 12.sp) },
                    shape = RoundedCornerShape(12.dp),
                    modifier = Modifier.fillMaxWidth(),
                )
                FieldLabel("终端 shell（可选）")
                OutlinedTextField(
                    value = termShell, onValueChange = { termShell = it }, singleLine = true,
                    placeholder = { Text("/bin/bash /bin/zsh 等，留空 = 登录 shell", fontSize = 12.sp) },
                    supportingText = { Text("预设 shell 自动带 -l；也可填自定义命令原样执行（如 tmux attach）", fontSize = 11.sp) },
                    shape = RoundedCornerShape(12.dp),
                    modifier = Modifier.fillMaxWidth(),
                )
                FieldLabel("备注")
                OutlinedTextField(
                    value = note, onValueChange = { note = it }, singleLine = true,
                    placeholder = { Text("可选", fontSize = 12.sp) },
                    shape = RoundedCornerShape(12.dp),
                    modifier = Modifier.fillMaxWidth(),
                )
                // 经由跳板机（设计稿 server-edit 屏：直连（默认）/ 其他 SSH 服务器）
                FieldLabel("经由跳板机")
                Box {
                    OutlinedTextField(
                        value = if (jumpId.isEmpty()) "直连（默认）"
                        else jumpChoices.firstOrNull { it.id == jumpId }?.let { JumpStore.text(it.name) }
                            ?: "（已删除，保存后回到直连）",
                        onValueChange = {}, readOnly = true, singleLine = true,
                        enabled = false,
                        shape = RoundedCornerShape(12.dp),
                        colors = disabledFieldColors(palette.fg, palette.lineStrong, palette.fgDim),
                        trailingIcon = { Text(if (jumpOpen) "▴" else "▾") },
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable { jumpOpen = true },
                    )
                    DropdownMenu(expanded = jumpOpen, onDismissRequest = { jumpOpen = false }) {
                        DropdownMenuItem(
                            text = { Text("直连（默认）") },
                            onClick = { jumpId = ""; jumpOpen = false },
                        )
                        jumpChoices.forEach { choice ->
                            DropdownMenuItem(
                                text = { Text(JumpStore.text(choice.name) + "（" + choice.addressLabel() + "）") },
                                onClick = { jumpId = choice.id; jumpOpen = false },
                            )
                        }
                    }
                }
                FieldLabel("选中后先登录跳板机，再由它转发到这台服务器（外网连内网机器）；Telnet 不支持")
                FieldLabel(
                    "SSH：支持隧道（外网连家里的电脑）+ 终端 + 文件；"
                            + "Telnet：只有终端，明文传输，仅实验。"
                )
                Spacer(Modifier.height(16.dp))

                // 保存（设计稿：全宽强调色大按钮）
                Box(
                    Modifier
                        .fillMaxWidth()
                        .height(48.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(palette.accent)
                        .clickable {
                            if (persistJump(existing, name, isTelnet, combinedHost(), username, authKey,
                                    password, savePassword, keyIndex, passphrase, note, jumpId, termPath, termShell)) {
                                editOpen = false
                            }
                        },
                    contentAlignment = Alignment.Center,
                ) {
                    Text("保存", fontSize = 14.sp, fontWeight = FontWeight.Bold, color = palette.onAccent)
                }
                Spacer(Modifier.height(10.dp))
                if (editing) {
                    // 测试连接（先落表单再测，改了地址没保存就测会测到旧的）
                    Box(
                        Modifier
                            .fillMaxWidth()
                            .height(48.dp)
                            .clip(RoundedCornerShape(12.dp))
                            .border(1.dp, palette.line, RoundedCornerShape(12.dp))
                            .clickable {
                                if (persistJump(existing, name, isTelnet, combinedHost(), username, authKey,
                                        password, savePassword, keyIndex, passphrase, note, jumpId, termPath, termShell)) {
                                    editOpen = false
                                    testConnection(existing)
                                }
                            },
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            "测试连接",
                            fontSize = 14.sp,
                            color = palette.fgDim,
                        )
                    }
                    // 删除入口（设计稿卡片四钮里没有删除；收进编辑弹层，删除确认弹窗照旧）
                    TextButton(
                        onClick = {
                            editOpen = false
                            deleteTarget = existing
                        },
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 6.dp),
                    ) { Text("删除此服务器", color = palette.bad, fontSize = 13.sp) }
                }
            }
        }
    }

    /** 设计稿的分段控件：容器描边 + 选中格强调色浅底 */
    @Composable
    private fun Segmented(options: List<String>, selected: Int, onSelect: (Int) -> Unit) {
        val palette = LocalPalette.current
        Row(
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(palette.base)
                .border(1.dp, palette.line, RoundedCornerShape(12.dp))
                .padding(4.dp),
        ) {
            options.forEachIndexed { index, label ->
                Text(
                    label,
                    fontSize = 12.sp,
                    fontWeight = if (selected == index) FontWeight.SemiBold else FontWeight.Normal,
                    color = if (selected == index) palette.accent else palette.fgDim,
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    modifier = Modifier
                        .weight(1f)
                        .clip(RoundedCornerShape(8.dp))
                        .background(if (selected == index) palette.accent.copy(alpha = 0.15f) else Color.Transparent)
                        .clickable { onSelect(index) }
                        .padding(vertical = 9.dp),
                )
            }
        }
    }

    /** 保存表单；返回 false = 校验没过（比如密码加密失败），对话框保持打开 */
    private fun persistJump(
        existing: JumpServer?,
        name: String,
        isTelnet: Boolean,
        host: String,
        username: String,
        authKey: Boolean,
        password: String,
        savePassword: Boolean,
        keyIndex: Int,
        passphrase: String,
        note: String,
        jumpId: String,
        termPath: String,
        termShell: String,
    ): Boolean {
        // 跳板机只对 SSH 有意义：Telnet 选了跳板机直接拦下（表单提示里写明了）
        if (isTelnet && jumpId.isNotEmpty()) {
            toast("Telnet 服务器不支持经由跳板机连接，请改回直连", long = true)
            return false
        }
        val target = existing ?: JumpServer()
        target.name = name.trim()
        target.protocol = if (isTelnet) JumpServer.PROTO_TELNET else JumpServer.PROTO_SSH
        target.applyHostInput(host)
        target.username = username.trim()
        target.authType = if (authKey) JumpServer.AUTH_KEY else JumpServer.AUTH_PASSWORD
        if (!savePassword) {
            // 不勾「保存密码」：清掉已存的（连接时将回退到密钥 / 失败提示）
            target.passwordEnc = ""
        } else if (password.isNotEmpty()) {
            val encrypted = KeyVault.encrypt(password)
            if (encrypted.isEmpty()) {
                // 设备密钥库异常：宁可拒绝保存也别静默吞掉密码
                toast("密码加密保存失败（设备密钥库异常），请重试一次", long = true)
                return false
            }
            target.passwordEnc = encrypted
        }
        if (authKey && keys.isNotEmpty()) {
            val key = keys[keyIndex.coerceIn(0, keys.size - 1)]
            target.keyId = key.id
            if (passphrase.isNotEmpty()) {
                target.keyPassphraseEnc = KeyVault.encrypt(passphrase)
            }
        } else if (!authKey) {
            target.keyId = ""
        }
        target.note = note.trim()
        target.jumpServerId = if (isTelnet) "" else jumpId.trim()
        target.termPath = termPath.trim()
        target.termShell = termShell.trim()
        if (existing == null) {
            jumps.add(target)
        }
        JumpStore.saveJumps(this, jumps)
        if (existing != null) {
            // 改完（尤其是改了密码/用户名）立刻能重连，别被退避挡住
            SshSessionManager.shared().resetBackoff(target.id)
        }
        return true
    }

    // ------------------------------------------------------------------
    // 密钥页
    // ------------------------------------------------------------------

    /** 密钥页（设计稿 keys 屏）：密钥卡 + 底部虚线提示框；生成/导入从 FAB 菜单进 */
    @Composable
    private fun KeysPage(
        importLauncher: androidx.activity.compose.ManagedActivityResultLauncher<Array<String>, Uri?>,
    ) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp)) {
            if (keys.isEmpty()) {
                EmptyHint(
                    "还没有密钥。导入已有的私钥，或直接在本机生成一对 —— "
                            + "把公钥放到服务器上（authorized_keys）就能免密登录。"
                )
            }
            for (entry in keys) {
                KeyCard(entry)
                Spacer(Modifier.height(10.dp))
            }
            // 设计稿的虚线提示框
            DashedHint(
                "公钥需追加到服务器 ~/.ssh/authorized_keys\n私钥加密存本机，永不上传"
            )
            Spacer(Modifier.height(12.dp))
        }

        deleteTargetKey?.let { entry ->
            AlertDialog(
                onDismissRequest = { deleteTargetKey = null },
                title = { Text("删除密钥") },
                text = { Text("确定删除「" + JumpStore.text(entry.name) + "」吗？私钥不可恢复。") },
                confirmButton = {
                    TextButton(onClick = {
                        deleteTargetKey = null
                        keys.remove(entry)
                        JumpStore.saveKeys(this@ServersActivity, keys)
                    }) { Text("删除", color = LocalPalette.current.bad) }
                },
                dismissButton = {
                    TextButton(onClick = { deleteTargetKey = null }) { Text("取消") }
                },
            )
        }
    }

    /** 一张密钥卡：图标块 + 名称/使用徽标 + 指纹行 + 复制公钥 / 删除 */
    @Composable
    private fun KeyCard(entry: SshKeyEntry) {
        val palette = LocalPalette.current
        val inUse = JumpStore.keysInUse(jumps, entry.id)
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(16.dp))
                .background(palette.card)
                .border(1.dp, palette.line, RoundedCornerShape(16.dp))
                .padding(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier
                        .size(40.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(if (inUse > 0) palette.accent.copy(alpha = 0.1f) else palette.elevate),
                    contentAlignment = Alignment.Center,
                ) {
                    KeyIcon(if (inUse > 0) palette.accent else palette.fgDim, 20.dp)
                }
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            if (JumpStore.text(entry.name).isEmpty()) "未命名密钥" else entry.name,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.SemiBold,
                            color = palette.fg,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        if (inUse > 0) {
                            Spacer(Modifier.width(6.dp))
                            Text(
                                "N 台使用".replace("N", inUse.toString()),
                                fontSize = 9.sp,
                                fontFamily = FontFamily.Monospace,
                                color = palette.accent,
                                modifier = Modifier
                                    .clip(RoundedCornerShape(4.dp))
                                    .background(palette.accent.copy(alpha = 0.1f))
                                    .padding(horizontal = 5.dp, vertical = 1.dp),
                            )
                        }
                    }
                    Text(
                        entry.algorithm
                                + (if (JumpStore.text(entry.fingerprint).isEmpty()) ""
                                   else " · " + shortenFingerprint(entry.fingerprint)),
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                        color = palette.fgDim,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(top = 2.dp),
                    )
                }
            }
            Spacer(Modifier.height(12.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Box(
                    Modifier
                        .weight(1f)
                        .height(36.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(palette.elevate)
                        .border(1.dp, palette.line, RoundedCornerShape(12.dp))
                        .clickable {
                            if (JumpStore.text(entry.publicKey).isEmpty()) {
                                toast("这把密钥没有记录公钥")
                            } else {
                                copyClipboard(JumpStore.text(entry.name) + " 公钥", entry.publicKey)
                            }
                        },
                    contentAlignment = Alignment.Center,
                ) { Text("复制公钥", fontSize = 12.sp, color = palette.fg) }
                Box(
                    Modifier
                        .weight(1f)
                        .height(36.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(palette.elevate)
                        .border(1.dp, palette.line, RoundedCornerShape(12.dp))
                        .clickable {
                            if (inUse > 0) {
                                toast("有 $inUse 台服务器在用这把密钥，先改它们的认证方式", long = true)
                            } else {
                                deleteTargetKey = entry
                            }
                        },
                    contentAlignment = Alignment.Center,
                ) { Text("删除", fontSize = 12.sp, color = palette.fgDim) }
            }
        }
    }

    /** SHA256:abcd…wxyz 形式的指纹缩写（设计稿样式） */
    private fun shortenFingerprint(fingerprint: String): String {
        val clean = fingerprint.removePrefix("SHA256:")
        if (clean.length <= 12) return fingerprint
        return "SHA256:" + clean.substring(0, 4) + "…" + clean.substring(clean.length - 4)
    }

    /** 设计稿的虚线提示框（keys 屏底部说明） */
    @Composable
    private fun DashedHint(text: String) {
        val palette = LocalPalette.current
        Box(
            Modifier
                .fillMaxWidth()
                .drawBehind {
                    val stroke = androidx.compose.ui.graphics.drawscope.Stroke(
                        width = 1.dp.toPx(),
                        pathEffect = androidx.compose.ui.graphics.PathEffect
                            .dashPathEffect(floatArrayOf(6.dp.toPx(), 5.dp.toPx())),
                    )
                    drawRoundRect(
                        color = palette.lineStrong,
                        topLeft = Offset.Zero,
                        size = size,
                        cornerRadius = androidx.compose.ui.geometry.CornerRadius(16.dp.toPx()),
                        style = stroke,
                    )
                }
                .padding(horizontal = 16.dp, vertical = 14.dp),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text,
                fontSize = 12.sp,
                lineHeight = 19.sp,
                color = palette.fgFaint,
                textAlign = androidx.compose.ui.text.style.TextAlign.Center,
            )
        }
    }

    private var deleteTargetKey by mutableStateOf<SshKeyEntry?>(null)

    @Composable
    private fun GenerateKeyDialog() {
        var name by remember { mutableStateOf("") }
        var ed25519 by remember { mutableStateOf(true) }
        AlertDialog(
            onDismissRequest = { generateKeyOpen = false },
            modifier = Modifier.imePadding(),
            properties = DialogProperties(decorFitsSystemWindows = false),
            title = { Text("生成密钥对") },
            text = {
                Column {
                    OutlinedTextField(
                        value = name, onValueChange = { name = it }, singleLine = true,
                        placeholder = { Text("名称，如 home-ed25519", fontSize = 12.sp) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = ed25519, onClick = { ed25519 = true })
                        Text(
                            "Ed25519（推荐）",
                            fontSize = 13.sp,
                            modifier = Modifier.weight(1f, fill = false),
                        )
                        Spacer(Modifier.width(8.dp))
                        RadioButton(selected = !ed25519, onClick = { ed25519 = false })
                        Text("RSA 4096", fontSize = 13.sp)
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    generateKeyOpen = false
                    generateKey(name.trim(), ed25519)
                }) { Text("生成") }
            },
            dismissButton = {
                TextButton(onClick = { generateKeyOpen = false }) { Text("取消") }
            },
        )
    }

    private fun generateKey(name: String, ed: Boolean) {
        Thread({
            try {
                // ssh-ed25519 在本机生成不出来，两头都堵死，所以先拦下并说清楚：
                //  · 没 Bouncy Castle 时 jsch 的 EdDSA 实现类加载不到 → NoClassDefFoundError；
                //  · 补上 Bouncy Castle 则会改变主机密钥协商，让老装机已存的 TOFU 指纹对不上、
                //    连接被硬拒（见 build.gradle.kts 的长注释），代价比收益大，暂不能加。
                // 于是干脆别让它跑到一半才炸 —— 老版本是在导出私钥那步抛
                // UnsupportedOperationException（message 为 null，界面只显示「未知错误」）。
                if (ed) {
                    throw Exception("当前版本还不能在本机生成 Ed25519 私钥。"
                            + "请改选 RSA 4096，或用 ssh-keygen 生成后走「导入私钥文件」。")
                }
                val jsch = JSch()
                val pair = if (ed) KeyPair.genKeyPair(jsch, KeyPair.ED25519)
                else KeyPair.genKeyPair(jsch, KeyPair.RSA, 4096)
                val entry = SshKeyEntry()
                entry.name = if (name.isEmpty()) "key-" + keys.size else name
                entry.algorithm = pair.keyTypeString
                val publicBlob = pair.publicKeyBlob
                entry.fingerprint = publicBlob?.let { KeyVault.sha256Fingerprint(it) } ?: ""
                // getPrivateKey 是包私有 —— 导出走 writePrivateKey(OutputStream)
                val pemOut = java.io.ByteArrayOutputStream()
                pair.writePrivateKey(pemOut)
                entry.privateEnc = KeyVault.encrypt(pemOut.toString("UTF-8"))
                entry.publicKey = publicBlob?.let {
                    pair.keyTypeString + " " + android.util.Base64.encodeToString(
                        it, android.util.Base64.NO_WRAP
                    )
                } ?: ""
                pair.dispose()
                runOnUiThread {
                    keys.add(entry)
                    JumpStore.saveKeys(this, keys)
                    toast(
                        "已生成 " + entry.algorithm + " 密钥「" + entry.name + "」—— 复制公钥放到服务器的"
                                + " authorized_keys", long = true
                    )
                }
            } catch (error: Exception) {
                // 以前只 toast 不落日志 —— 这类失败在 logcat 里完全看不见（message 为 null 时 toast 只显示「未知错误」）。
                TerminalActivity.tlog("生成密钥失败: " + android.util.Log.getStackTraceString(error))
                val message = error.message ?: "未知错误"
                runOnUiThread { toast("生成失败：$message", long = true) }
            }
        }, "anyboot-keygen").start()
    }

    private fun importKey(uri: Uri) {
        try {
            contentResolver.openInputStream(uri)?.use { stream ->
                val buffer = java.io.ByteArrayOutputStream()
                val chunk = ByteArray(8192)
                var count: Int
                while (stream.read(chunk).also { count = it } > 0) {
                    buffer.write(chunk, 0, count)
                    if (buffer.size() > 256 * 1024) throw Exception("文件太大，不像私钥")
                }
                val pem = buffer.toString("UTF-8").trim()
                if (!pem.startsWith("-----BEGIN")) {
                    throw Exception("不像 PEM 格式的私钥（应以 -----BEGIN 开头）")
                }
                Thread({
                    try {
                        val pair = KeyPair.load(JSch(), pem.toByteArray(Charsets.UTF_8), null)
                        val entry = SshKeyEntry()
                        val fileName = queryFileName(uri)
                        entry.name = fileName ?: ("imported-" + keys.size)
                        entry.algorithm = pair.keyTypeString
                        val publicBlob = pair.publicKeyBlob
                        entry.fingerprint = publicBlob?.let { KeyVault.sha256Fingerprint(it) } ?: ""
                        entry.privateEnc = KeyVault.encrypt(pem)
                        entry.publicKey = publicBlob?.let {
                            pair.keyTypeString + " " + android.util.Base64.encodeToString(
                                it, android.util.Base64.NO_WRAP
                            )
                        } ?: ""
                        pair.dispose()
                        runOnUiThread {
                            keys.add(entry)
                            JumpStore.saveKeys(this, keys)
                            toast("已导入 " + entry.algorithm + " 密钥")
                        }
                    } catch (error: Exception) {
                        val message = error.message ?: "未知错误"
                        runOnUiThread { toast("导入失败：$message", long = true) }
                    }
                }, "anyboot-key-import").start()
            }
        } catch (error: Exception) {
            toast("读取失败：" + error.message, long = true)
        }
    }

    private fun queryFileName(uri: Uri): String? {
        try {
            contentResolver.query(uri, null, null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    if (index >= 0) return cursor.getString(index)
                }
            }
        } catch (ignored: Exception) {
            // 拿不到文件名就用默认名
        }
        return null
    }

    private fun copyClipboard(label: String, text: String) {
        val manager = getSystemService(CLIPBOARD_SERVICE) as android.content.ClipboardManager
        manager.setPrimaryClip(android.content.ClipData.newPlainText(label, text))
        toast("已复制")
    }

    // ------------------------------------------------------------------
    // 文件页（SFTP，仅 SSH）
    // ------------------------------------------------------------------

    @OptIn(ExperimentalLayoutApi::class)
    @Composable
    private fun FilesPage() {
        val palette = LocalPalette.current
        val uploadLauncher = rememberLauncherForActivityResult(
            ActivityResultContracts.OpenDocument()
        ) { uri ->
            if (uri != null) {
                // OpenDocument 的授权默认只活到进程结束 —— 申请持久化，上传暂停后重进 App 才接得上
                try {
                    contentResolver.takePersistableUriPermission(
                        uri, Intent.FLAG_GRANT_READ_URI_PERMISSION
                    )
                } catch (e: Exception) {
                    // 个别 provider 不支持持久化授权：不影响本次上传，只是跨进程续不了
                    TerminalActivity.tlog("上传：拿不到持久化授权（${e.message}）")
                }
                uploadPicked(uri)
            }
        }

        val sshServers = jumps.filter { it.isSsh() }
        // 选中的服务器在 Column 之前算好：下面那层弹窗也要用它。写 sftpServerId 只会在
        // 「当前选中的服务器没了」时发生一次，之后条件成立、不再写，不会自激。
        val selectedId = if (sshServers.any { it.id == sftpServerId }) sftpServerId
        else sshServers.firstOrNull()?.id?.also { sftpServerId = it } ?: ""
        val selected = sshServers.firstOrNull { it.id == selectedId }

        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp)) {
            val server = selected
            if (server == null) {
                EmptyHint("没有 SSH 服务器 —— 先到「服务器」页添加。Telnet 不支持文件传输。")
                return@Column
            }

            var serverOpen by remember { mutableStateOf(false) }
            Box {
                OutlinedTextField(
                    value = if (JumpStore.text(server.name).isEmpty()) server.addressLabel()
                    else JumpStore.text(server.name),
                    onValueChange = {}, readOnly = true, singleLine = true,
                    label = { Text("服务器") },
                    // 必须 disabled：readOnly 的 TextField 会吃掉点击，clickable 收不到（详见 disabledFieldColors）
                    enabled = false,
                    colors = disabledFieldColors(
                        LocalPalette.current.fg,
                        LocalPalette.current.lineStrong,
                        LocalPalette.current.fgDim,
                    ),
                    trailingIcon = { Text(if (serverOpen) "▴" else "▾") },
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 4.dp)
                        .clickable { serverOpen = true },
                )
                DropdownMenu(expanded = serverOpen, onDismissRequest = { serverOpen = false }) {
                    sshServers.forEach { item ->
                        DropdownMenuItem(
                            text = {
                                Text(
                                    if (JumpStore.text(item.name).isEmpty()) item.addressLabel()
                                    else JumpStore.text(item.name)
                                )
                            },
                            onClick = {
                                serverOpen = false
                                if (item.id != sftpServerId) {
                                    sftpServerId = item.id
                                    clearClip()
                                    gotoDir("/", true)
                                }
                            },
                        )
                    }
                }
            }

            // 路径面包屑（设计稿 files 屏：rounded-xl 卡片容器内的 mono 路径段），
            // 每一段都能点着往回跳，长路径横向滚动、不挤爆整行
            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(vertical = 6.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .background(palette.card)
                    .border(1.dp, palette.line, RoundedCornerShape(12.dp))
                    .padding(horizontal = 10.dp, vertical = 4.dp),
            ) {
                PathBar()
            }

            // 工具行（FlowRow 会自动换行，窄屏上也不会把某一条压成 0 宽）
            FlowRow(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(vertical = 4.dp),
            ) {
                ChipButton("上级") { gotoDir(SftpOps.parent(sftpPath)) }
                ChipButton("上传") { uploadLauncher.launch(arrayOf("*/*")) }
                ChipButton("＋ 新建目录") { sftpMkdirOpen = true }
                ChipButton("刷新") { reloadDir() }
                ChipButton("路径") { sftpLocateOpen = true }
                ChipButton("终端") { openTerminalAt(server, sftpPath) }

                var sortOpen by remember { mutableStateOf(false) }
                Box {
                    ChipButton("排序 " + sortName(sftpSort) + (if (sftpSortDesc) " ↓" else " ↑")) {
                        sortOpen = true
                    }
                    DropdownMenu(expanded = sortOpen, onDismissRequest = { sortOpen = false }) {
                        SftpSort.entries.forEach { key ->
                            DropdownMenuItem(
                                text = { Text(sortName(key) + if (sftpSort == key) "　✓" else "") },
                                onClick = { sftpSort = key; sortOpen = false },
                            )
                        }
                        DropdownMenuItem(
                            text = { Text(if (sftpSortDesc) "改为升序 ↑" else "改为降序 ↓") },
                            onClick = { sftpSortDesc = !sftpSortDesc; sortOpen = false },
                        )
                    }
                }

                ChipButton("显示隐藏", selected = sftpShowHidden) {
                    sftpShowHidden = !sftpShowHidden
                }
                ChipButton("过滤", selected = sftpFilterOpen) {
                    sftpFilterOpen = !sftpFilterOpen
                    if (!sftpFilterOpen) sftpFilter = ""
                }
            }

            if (sftpFilterOpen) {
                OutlinedTextField(
                    value = sftpFilter,
                    onValueChange = { sftpFilter = it },
                    singleLine = true,
                    placeholder = {
                        Text("按名称过滤（不区分大小写），清空就全显示", fontSize = 12.sp)
                    },
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 2.dp),
                )
            }

            if (sftpOpLabel.isNotEmpty()) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(top = 6.dp),
                ) {
                    CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.width(8.dp))
                    SecondaryText("正在$sftpOpLabel …")
                }
            }

            if (sftpClip != SftpClip.NONE) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(vertical = 6.dp),
                ) {
                    Text(
                        (if (sftpClip == SftpClip.COPY) "📋 已复制 " else "✂️ 已剪切 ") + sftpClipNames,
                        fontSize = 12.sp,
                        color = LocalPalette.current.fgDim,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    ChipButton("粘贴到当前目录") { pasteClipboard(server, sftpPath) }
                    Spacer(Modifier.width(6.dp))
                    ChipButton("取消") { clearClip() }
                }
            }

            if (!SshSessionManager.shared().isUp(sftpServerId)) {
                SecondaryText("　（先在「服务器」页连上这台服务器，或点行内「终端」——"
                        + " SFTP 复用同一条 SSH 会话）")
            }

            // 设计稿的三列表头：名称 / 大小 / 修改时间
            if (sftpError.isEmpty() && !sftpBusy && sftpEntries.isNotEmpty()) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 8.dp, vertical = 4.dp),
                ) {
                    Text("名称", fontSize = 10.sp, color = palette.fgFaint, modifier = Modifier.weight(1f))
                    Text("大小", fontSize = 10.sp, color = palette.fgFaint, modifier = Modifier.width(64.dp))
                    Text("修改时间", fontSize = 10.sp, color = palette.fgFaint, modifier = Modifier.width(72.dp), textAlign = androidx.compose.ui.text.style.TextAlign.End)
                    Spacer(Modifier.width(30.dp)) // 行末 ⋮ 的占位
                }
            }

            val shown = orderEntries(
                sftpEntries.filter {
                    (sftpShowHidden || !it.name.startsWith("."))
                            && (sftpFilter.isEmpty() || it.name.contains(sftpFilter, true))
                }
            )

            when {
                // sftpError 已经带着「读取失败：…」前缀（sftpReason 生成），这里别再加一层，
                // 不然就是「读取失败：读取失败：…」套娃
                sftpError.isNotEmpty() -> SecondaryText("　$sftpError",
                    Modifier.padding(horizontal = 12.dp))
                sftpBusy -> Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(16.dp),
                ) {
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.width(8.dp))
                    SecondaryText("加载中 …")
                }
                sftpEntries.isEmpty() -> SecondaryText("　（空目录）", Modifier.padding(16.dp))
                shown.isEmpty() -> SecondaryText("　（没有匹配的项）", Modifier.padding(16.dp))
                else -> shown.forEach { entry -> FileRow(entry) }
            }

            // 目录还没按「服务器|路径」拉取过时自动加载一次。
            // 判据不再看 sftpBusy —— 快速连点两个目录时，前一个请求还在跑会把新目录的加载
            // 整个挡掉，然后谁都拉不动（LaunchedEffect 的 key 已经变过、不会再触发）。
            LaunchedEffect(sftpServerId, sftpPath, sftpLoadedKey) {
                if (sftpLoadedKey == "$sftpServerId|$sftpPath") return@LaunchedEffect
                if (sftpServerId.isEmpty()) return@LaunchedEffect
                loadSftpListing()
            }

            // 上传完成 → 当前目录内容变了，重拉一次。
            // 只在「刚收尾的那条上传」正好落在当前目录时才清 key，否则会来回空拉。
            var lastSeenUpload by remember { mutableStateOf("") }
            LaunchedEffect(transferTick) {
                val done = TransferManager.newestFinishedUpload() ?: return@LaunchedEffect
                if (done.id == lastSeenUpload) return@LaunchedEffect
                lastSeenUpload = done.id
                if (done.state == TransferManager.STATE_DONE
                    && done.serverId == sftpServerId && done.remoteDir == sftpPath
                ) {
                    reloadDir()
                }
            }
        }

        // 弹窗都挂在 Column 外面（Dialog 是独立窗口，放这儿不占布局，也不会被滚动带走）
        val dialogServer = selected
        if (dialogServer != null) {
            sftpAskDetail?.let { FileDetailDialog(it, dialogServer) }
            sftpAskRename?.let { RenameFileDialog(it, dialogServer) }
            sftpAskDelete?.let { DeleteFileDialog(it, dialogServer) }
            sftpSheetEntry?.let { FileActionSheet(it, dialogServer) }
            if (sftpMkdirOpen) MkdirDialog(dialogServer)
            if (sftpLocateOpen) LocateDialog(dialogServer)
            sftpPick?.let { DestPickerDialog(it, dialogServer) }
        }
    }

    /** 路径面包屑：/ a / b / c，每一段点一下跳过去 */
    @Composable
    private fun PathBar() {
        val palette = LocalPalette.current
        val segments = remember(sftpPath) { pathSegments(sftpPath) }
        val state = rememberLazyListState()
        // 当前目录优先可见（路径很长时，尾巴比开头有用）
        LaunchedEffect(segments) {
            if (segments.isNotEmpty()) state.scrollToItem(segments.size - 1)
        }
        LazyRow(
            state = state,
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp),
        ) {
            itemsIndexed(segments) { index, segment ->
                if (index > 0) {
                    Text("/", fontSize = 13.sp, color = palette.fgFaint)
                }
                Text(
                    segment,
                    fontSize = 13.sp,
                    color = if (index == segments.size - 1) palette.fg else palette.accent,
                    maxLines = 1,
                    modifier = Modifier
                        .clip(RoundedCornerShape(6.dp))
                        .clickable { gotoDir(pathUpTo(segments, index)) }
                        .padding(horizontal = 6.dp, vertical = 4.dp),
                )
            }
        }
    }

    /**
     * 文件 / 目录「⋯」底部动作面板（设计稿 file-menu 屏一比一）：
     * 预览卡（图标/名称/大小·时间）→ 变体动作（文件=下载到本机/重命名/复制路径/删除；
     * 目录=重命名/复制路径/删除含全部内容）→ 分隔线后折叠「更多操作」收纳原有全套能力。
     * 每个动作先收面板再干活；删除等确认弹窗沿用原有 sftpAsk* 通道。
     */
    @OptIn(ExperimentalMaterial3Api::class)
    @Composable
    private fun FileActionSheet(entry: SftpOps.Entry, server: JumpServer) {
        val palette = LocalPalette.current
        val path = SftpOps.join(sftpPath, entry.name)
        val close = { sftpSheetEntry = null }

        ModalBottomSheet(
            onDismissRequest = { sftpSheetEntry = null },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = palette.card,
        ) {
            Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp)) {
                // 对象预览卡
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(16.dp))
                        .background(palette.base)
                        .border(1.dp, palette.line, RoundedCornerShape(16.dp))
                        .padding(12.dp),
                ) {
                    Box(
                        Modifier
                            .size(40.dp)
                            .clip(RoundedCornerShape(12.dp))
                            .background(palette.accent.copy(alpha = 0.12f)),
                        contentAlignment = Alignment.Center,
                    ) {
                        when {
                            entry.dir -> FolderIcon(palette.accent, 20.dp)
                            entry.name.endsWith(".sh") -> BoltIcon(palette.warn, 20.dp)
                            else -> DocFileIcon(palette.accent, 20.dp)
                        }
                    }
                    Column(Modifier.weight(1f).padding(horizontal = 10.dp)) {
                        Text(
                            entry.name,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.SemiBold,
                            color = if (entry.name.endsWith(".sh") && !entry.dir) palette.warn else palette.fg,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            (if (entry.dir) "目录" else humanSize(entry.size)) + " · " + timeText(entry.mtime),
                            fontSize = 11.sp,
                            fontFamily = FontFamily.Monospace,
                            color = palette.fgDim,
                        )
                    }
                    Text(
                        "✕",
                        fontSize = 14.sp,
                        color = palette.fgDim,
                        modifier = Modifier
                            .clip(RoundedCornerShape(999.dp))
                            .clickable { close() }
                            .padding(8.dp),
                    )
                }

                Spacer(Modifier.height(12.dp))

                // 变体动作（设计稿主区）
                if (entry.dir) {
                    SheetAction("重命名") { close(); sftpAskRename = entry }
                    SheetAction("复制路径") { close(); copyClipboard("路径", path) }
                    SheetAction("删除（含目录内全部内容）", palette.bad) { close(); sftpAskDelete = entry }
                } else {
                    SheetAction("下载到本机", palette.accent) { close(); startDownload(entry.name) }
                    SheetAction("重命名") { close(); sftpAskRename = entry }
                    SheetAction("复制路径") { close(); copyClipboard("路径", path) }
                    SheetAction("删除", palette.bad) { close(); sftpAskDelete = entry }
                }

                // 分隔线 + 折叠「更多操作」：原有全套能力原样保留
                Box(
                    Modifier
                        .fillMaxWidth()
                        .padding(vertical = 10.dp)
                        .height(1.dp)
                        .background(palette.line),
                )
                Text("更多操作", fontSize = 11.sp, color = palette.fgFaint, modifier = Modifier.padding(bottom = 4.dp))
                SheetAction("详情", small = true) { close(); sftpAskDetail = entry }
                SheetAction("复制", small = true) { close(); setClip(SftpClip.COPY, entry) }
                SheetAction("剪切", small = true) { close(); setClip(SftpClip.CUT, entry) }
                if (entry.dir && sftpClip != SftpClip.NONE) {
                    SheetAction("粘贴到此处", small = true) { close(); pasteClipboard(server, sftpPath) }
                }
                if (entry.dir) {
                    SheetAction("在终端打开", small = true) { close(); openTerminalAt(server, path) }
                }
                SheetAction("复制到…", small = true) {
                    close(); sftpPick = SftpPick(false, listOf(path), entry.name, sftpPath)
                }
                SheetAction("移动到…", small = true) {
                    close(); sftpPick = SftpPick(true, listOf(path), entry.name, sftpPath)
                }
            }
        }
    }

    /** 动作面板里的一行（设计稿 h-11 行：图标 + 文案；small=「更多操作」组的小字行） */
    @Composable
    private fun SheetAction(text: String, color: Color? = null, small: Boolean = false, onClick: () -> Unit) {
        val palette = LocalPalette.current
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = if (small) 38.dp else 44.dp)
                .clip(RoundedCornerShape(10.dp))
                .clickable { onClick() }
                .padding(horizontal = 12.dp),
        ) {
            when {
                text.startsWith("下载") -> DownloadIcon(color ?: palette.fgDim, if (small) 15.dp else 18.dp)
                text.startsWith("重命名") -> PencilIcon(color ?: palette.fgDim, if (small) 15.dp else 18.dp)
                text.startsWith("复制路径") -> CopyIcon(color ?: palette.fgDim, if (small) 15.dp else 18.dp)
                text.startsWith("删除") -> TrashIcon(color ?: palette.bad, if (small) 15.dp else 18.dp)
            }
            Text(
                text,
                fontSize = if (small) 13.sp else 14.sp,
                fontWeight = if (!small && color != null) FontWeight.Medium else FontWeight.Normal,
                color = color ?: palette.fg,
                modifier = Modifier.padding(start = 12.dp),
            )
        }
    }

    /** 文件 / 目录一行（设计稿三列行卡）：点一下进目录或下载；长按整行、或点行末 ⋮ 都出操作菜单 */
    @OptIn(ExperimentalFoundationApi::class)
    @Composable
    private fun FileRow(entry: SftpOps.Entry) {
        val palette = LocalPalette.current
        val path = SftpOps.join(sftpPath, entry.name)
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .padding(vertical = 3.dp)
                .clip(RoundedCornerShape(12.dp))
                .background(palette.card)
                .border(1.dp, palette.line, RoundedCornerShape(12.dp))
                .combinedClickable(
                    onClick = {
                        if (entry.dir) {
                            gotoDir(path)
                        } else {
                            startDownload(entry.name)
                        }
                    },
                    onLongClick = { sftpSheetEntry = entry },
                )
                .padding(start = 14.dp, end = 2.dp, top = 10.dp, bottom = 10.dp),
        ) {
            // 图标：目录=文件夹（强调色）/ 脚本=闪电（警示色）/ 其他=文档
            when {
                entry.dir -> FolderIcon(palette.accent, 18.dp)
                entry.name.endsWith(".sh") -> BoltIcon(palette.warn, 18.dp)
                else -> DocFileIcon(palette.fgDim, 18.dp)
            }
            Text(
                entry.name,
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
                color = if (entry.name.endsWith(".sh") && !entry.dir) palette.warn else palette.fg,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier
                    .weight(1f)
                    .padding(horizontal = 10.dp),
            )
            Text(
                if (entry.dir) "—" else humanSize(entry.size),
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
                color = if (entry.dir) palette.fgFaint else palette.fgDim,
                modifier = Modifier.width(64.dp),
            )
            Text(
                timeText(entry.mtime),
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
                color = palette.fgDim,
                maxLines = 1,
                textAlign = androidx.compose.ui.text.style.TextAlign.End,
                modifier = Modifier.width(72.dp),
            )
            // 行末 ⋮：底部动作面板（设计稿 file-menu 屏一比一：预览卡 + 文件/目录变体动作 + 折叠更多）
            Text(
                "⋮",
                fontSize = 17.sp,
                color = palette.fgDim,
                modifier = Modifier
                    .clickable { sftpSheetEntry = entry }
                    .padding(horizontal = 12.dp, vertical = 6.dp),
            )
        }
    }

    /** 后台拉目录（结果落 sftpEntries / sftpError 状态） */
    private fun loadSftpListing() {
        val serverId = sftpServerId
        val path = sftpPath
        sftpBusy = true
        val appContext = applicationContext
        Thread({
            var entries: List<SftpOps.Entry> = emptyList()
            var error: String? = null
            try {
                val server = JumpStore.findJump(JumpStore.loadJumps(appContext), serverId)
                    ?: throw Exception("服务器不存在（可能已删除）")
                entries = SftpOps.list(appContext, server, path) { title, message ->
                    bus.confirm(title, message)
                }
            } catch (failure: Exception) {
                error = sftpReason("读取", path, failure)
            }
            val result = entries
            val failureMessage = error
            runOnUiThread {
                // 已经被后来的请求接管了：busy 由那个请求的收尾负责清，这里别抢
                if (serverId != sftpServerId || path != sftpPath) {
                    return@runOnUiThread
                }
                sftpBusy = false
                sftpLoadedKey = "$serverId|$path"   // 成功失败都算「拉过了」，空目录别再循环拉
                if (failureMessage != null) {
                    sftpError = failureMessage
                    return@runOnUiThread
                }
                sftpEntries = result
            }
        }, "anyboot-sftp-ls").start()
    }

    /**
     * 下载：交给传输中心（进度、取消、结果都在「传输」页；这里只负责发起）。
     * 传完落盘的位置由 TransferManager 决定（App 的下载目录），完成提示里给绝对路径。
     */
    private fun startDownload(fileName: String) {
        val server = JumpStore.findJump(JumpStore.loadJumps(this), sftpServerId)
        if (server == null) {
            toast("服务器不存在（可能已删除）")
            return
        }
        // 同一条文件已经在传了就别再起一个：两个任务写同一个本地文件会互相踩
        if (TransferManager.pendingFor(TransferManager.DIR_DOWNLOAD, remotePathOf(fileName)) != null) {
            toast("这条已经在传了，进度见「传输」页")
            return
        }
        TransferManager.startDownload(applicationContext, server, sftpPath, fileName) { title, message ->
            bus.confirm(title, message)
        }
        toast("已加入传输：$fileName（进度见「传输」页）")
    }

    /** 上传：同上，交给传输中心 */
    private fun uploadPicked(uri: Uri) {
        val server = JumpStore.findJump(JumpStore.loadJumps(this), sftpServerId)
        if (server == null) {
            toast("先选择服务器")
            return
        }
        var name = queryFileName(uri)
        if (name.isNullOrEmpty()) name = "upload-" + System.currentTimeMillis()
        if (TransferManager.pendingFor(TransferManager.DIR_UPLOAD, remotePathOf(name)) != null) {
            toast("这条已经在传了，进度见「传输」页")
            return
        }
        TransferManager.startUpload(applicationContext, server, sftpPath, uri, name) { title, message ->
            bus.confirm(title, message)
        }
        toast("已加入传输：$name（进度见「传输」页）")
    }

    /** 与 TransferManager.joinRemote 同一口径（重复发起检查要用完整远端路径比对） */
    private fun remotePathOf(name: String): String =
        if (sftpPath.isEmpty() || sftpPath == "/") "/$name"
        else if (sftpPath.endsWith("/")) sftpPath + name else "$sftpPath/$name"

    // ------------------------------------------------------------------
    // 文件页：目录导航与排序
    // ------------------------------------------------------------------

    /** 切目录。force=true 用于「就地重拉」（路径没变也要重新拉一遍） */
    private fun gotoDir(path: String, force: Boolean = false) {
        if (path == sftpPath && !force) return
        sftpPath = path
        sftpEntries = emptyList()
        sftpError = ""
        sftpBusy = true     // 先亮「加载中」，免得中间闪一下「（空目录）」
        sftpLoadedKey = ""
    }

    /** 刷新当前目录 */
    private fun reloadDir() = gotoDir(sftpPath, true)

    /** /a/b/c → ["/", "a", "b", "c"]（根目录就是 ["/"]） */
    private fun pathSegments(path: String): List<String> {
        val clean = path.trim().ifEmpty { "/" }
        if (clean == "/") return listOf("/")
        return listOf("/") + clean.trim('/').split('/').filter { it.isNotEmpty() }
    }

    /** 面包屑第 index 段对应的完整路径（index=0 是根） */
    private fun pathUpTo(segments: List<String>, index: Int): String {
        if (index <= 0) return "/"
        return "/" + segments.subList(1, minOf(index + 1, segments.size)).joinToString("/")
    }

    /**
     * 排序：目录恒定排在最前（跟手机上的文件管理器一个习惯），组内再按用户选的规则，
     * 最后拿名字兜底 —— 大小 / 时间相同的项顺序也稳定，不会每刷一次就跳动。
     */
    private fun orderEntries(raw: List<SftpOps.Entry>): List<SftpOps.Entry> {
        var key: Comparator<SftpOps.Entry> = when (sftpSort) {
            SftpSort.NAME -> compareBy<SftpOps.Entry> { it.name.lowercase(Locale.ROOT) }
            SftpSort.SIZE -> compareBy<SftpOps.Entry> { it.size }
            SftpSort.TIME -> compareBy<SftpOps.Entry> { it.mtime }
        }
        if (sftpSortDesc) key = key.reversed()
        return raw.sortedWith(
            compareBy<SftpOps.Entry> { if (it.dir) 0 else 1 }
                .then(key)
                .thenBy { it.name.lowercase(Locale.ROOT) }
        )
    }

    private fun sortName(sort: SftpSort): String = when (sort) {
        SftpSort.NAME -> "名称"
        SftpSort.SIZE -> "大小"
        SftpSort.TIME -> "时间"
    }

    private fun timeText(seconds: Long): String {
        if (seconds <= 0) return "-"
        return SimpleDateFormat("yyyy-MM-dd HH:mm:ss", Locale.US).format(Date(seconds * 1000L))
    }

    // ------------------------------------------------------------------
    // 文件页：目录操作（都在后台线程跑，主线程只负责发指令和刷状态）
    // ------------------------------------------------------------------

    private fun sftpPrompt(): SshSessionManager.Prompt =
        SshSessionManager.Prompt { title, message -> bus.confirm(title, message) }

    /**
     * 后台跑一次目录操作。期间 [sftpOpLabel] 非空（工具行亮「正在…」、再点也只提示不并发）。
     * 完成后：成功 → [after]（默认重拉当前目录）；失败 → 长提示弹出来。
     * [work] 里直接抛异常就行，message 会原样给用户看。
     *
     * ⚠️ [after] 必须排在 [work] 前面：尾随 lambda 只会绑到最后一个函数参数上，
     * 换言之 `sftpRun("删除") { … }` 里的 lambda 永远进 [work]。反过来写会直接编译不过。
     */
    private fun sftpRun(label: String, after: (() -> Unit)? = null, work: () -> Unit) {
        if (sftpOpLabel.isNotEmpty()) {
            toast("正在$sftpOpLabel，稍等一下")
            return
        }
        sftpOpLabel = label
        Thread({
            var failure: String? = null
            try {
                work()
            } catch (error: Exception) {
                failure = error.message ?: error.javaClass.simpleName
            }
            val message = failure
            runOnUiThread {
                sftpOpLabel = ""
                if (message != null) {
                    toast(message, long = true)
                } else if (after != null) {
                    after()
                } else {
                    reloadDir()
                }
            }
        }, "anyboot-sftp-op").start()
    }

    /**
     * jsch 的英文报错换成中文；我们自己抛的（已经是给用户看的中文）原样透出，
     * 免得出现「删除失败：删除失败：权限不够」这种套娃。
     */
    private fun sftpReason(action: String, label: String, error: Exception): String {
        val fromJsch = error is com.jcraft.jsch.SftpException
                || error is com.jcraft.jsch.JSchException
        return if (fromJsch) SftpOps.reason(error, action, label)
        else (error.message ?: "$action 失败")
    }

    private fun mkdirHere(server: JumpServer, name: String) {
        val target = SftpOps.join(sftpPath, name)
        sftpRun("新建目录") {
            try {
                SftpOps.mkdir(applicationContext, server, target, sftpPrompt())
            } catch (error: Exception) {
                throw Exception(sftpReason("新建目录", name, error))
            }
        }
    }

    private fun renameEntry(server: JumpServer, entry: SftpOps.Entry, newName: String) {
        val from = SftpOps.join(sftpPath, entry.name)
        val to = SftpOps.join(sftpPath, newName)
        sftpRun("重命名") {
            try {
                SftpOps.rename(applicationContext, server, from, to, sftpPrompt())
            } catch (error: Exception) {
                throw Exception(sftpReason("重命名", entry.name, error))
            }
        }
    }

    private fun deleteEntry(server: JumpServer, entry: SftpOps.Entry) {
        val path = SftpOps.join(sftpPath, entry.name)
        sftpRun("删除") {
            try {
                SftpOps.delete(applicationContext, server, path, sftpPrompt())
            } catch (error: Exception) {
                throw Exception(sftpReason("删除", entry.name, error))
            }
        }
    }

    /** 定位路径：支持绝对路径、~/ 家目录、相对当前目录；交给服务器 realpath 校验一遍再跳 */
    private fun locate(server: JumpServer, input: String) {
        val raw = input.trim()
        if (raw.isEmpty()) return
        var resolved: String? = null
        sftpRun("定位路径", work = {
            try {
                var value = raw
                if (raw == "~" || raw.startsWith("~/")) {
                    value = SftpOps.home(applicationContext, server, sftpPrompt()) + raw.substring(1)
                } else if (!raw.startsWith("/")) {
                    value = SftpOps.join(sftpPath, raw)
                }
                resolved = SftpOps.resolve(applicationContext, server, value, sftpPrompt())
            } catch (error: Exception) {
                throw Exception(sftpReason("定位", raw, error))
            }
        }, after = { resolved?.let { gotoDir(it) } })
    }

    /** 复制 / 剪切进剪贴板（应用内，不碰手机的系统剪贴板） */
    private fun setClip(mode: SftpClip, entry: SftpOps.Entry) {
        sftpClip = mode
        sftpClipServer = sftpServerId
        sftpClipPaths = listOf(SftpOps.join(sftpPath, entry.name))
        sftpClipNames = entry.name
        toast((if (mode == SftpClip.CUT) "已剪切：" else "已复制：") + entry.name)
    }

    private fun clearClip() {
        sftpClip = SftpClip.NONE
        sftpClipServer = ""
        sftpClipPaths = emptyList()
        sftpClipNames = ""
    }

    /** 粘贴到指定目录 */
    private fun pasteClipboard(server: JumpServer, destDir: String) {
        if (sftpClip == SftpClip.NONE) return
        if (sftpClipServer != server.id) {
            toast("剪贴板里的东西在另一台服务器上，只能在同一台里粘贴")
            return
        }
        val request = SftpPick(sftpClip == SftpClip.CUT, sftpClipPaths, sftpClipNames, destDir)
        transferInto(server, request, destDir)
    }

    /**
     * 复制 / 移动到目标目录。
     * 目标已有同名项时：复制会自动改名成「名字 (2)」，剪切直接报错 ——
     * 剪切是「移动」，静默改个名会让用户以为东西没了，不如让他自己决定。
     */
    private fun transferInto(server: JumpServer, request: SftpPick, destDir: String) {
        sftpRun(if (request.cut) "移动" else "复制") {
            for (source in request.paths) {
                if (request.cut && SftpOps.parent(source) == destDir) continue   // 剪到自己待的目录：没意义，跳过
                val name = SftpOps.nameOf(source)
                var target = SftpOps.join(destDir, name)
                try {
                    if (SftpOps.exists(applicationContext, server, target, sftpPrompt())) {
                        if (request.cut) {
                            throw Exception("目标目录里已经有「$name」了")
                        }
                        target = uniqueTarget(server, destDir, name)
                    }
                    if (request.cut) {
                        SftpOps.move(applicationContext, server, source, target, sftpPrompt())
                    } else {
                        SftpOps.copy(applicationContext, server, source, target, sftpPrompt())
                    }
                } catch (error: Exception) {
                    throw Exception(sftpReason(if (request.cut) "移动" else "复制", name, error))
                }
            }
            if (request.cut) {
                // 剪切成功 → 剪贴板空了（Compose 状态只能在主线程写）
                runOnUiThread { clearClip() }
            }
        }
    }

    /** 复制时避免覆盖：foo.txt → foo (2).txt */
    private fun uniqueTarget(server: JumpServer, dir: String, name: String): String {
        val dot = name.lastIndexOf('.')
        val stem = if (dot > 0) name.substring(0, dot) else name
        val ext = if (dot > 0) name.substring(dot) else ""
        for (index in 2..99) {
            val candidate = SftpOps.join(dir, "$stem ($index)$ext")
            if (!SftpOps.exists(applicationContext, server, candidate, sftpPrompt())) {
                return candidate
            }
        }
        throw Exception("同名副本太多了（已经到 99 个）")
    }

    /** 打开这台服务器的终端，并让它 cd 到指定目录（终端页读 cdPath，会话一连上就打进去） */
    private fun openTerminalAt(server: JumpServer, dir: String) {
        if (!server.isSsh()) {
            toast("Telnet 只有终端，没有文件功能")
            return
        }
        val intent = Intent(this, TerminalActivity::class.java)
        intent.putExtra("jumpId", server.id)
        intent.putExtra("cdPath", dir)
        startActivity(intent)
    }

    // ------------------------------------------------------------------
    // 文件页：弹窗
    // ------------------------------------------------------------------

    /** 文件详情（字段现取，软链接会连指向一起给） */
    @Composable
    private fun FileDetailDialog(entry: SftpOps.Entry, server: JumpServer) {
        val palette = LocalPalette.current
        val path = SftpOps.join(sftpPath, entry.name)
        var fields by remember(entry) { mutableStateOf<List<Pair<String, String>>?>(null) }
        var error by remember(entry) { mutableStateOf("") }
        LaunchedEffect(entry, sftpPath) {
            Thread({
                var failure: String? = null
                val rows = ArrayList<Pair<String, String>>()
                try {
                    val detail = SftpOps.detail(applicationContext, server, path) { title, message ->
                        bus.confirm(title, message)
                    }
                    rows.add("名称" to detail.name)
                    rows.add("类型" to detail.typeLabel)
                    rows.add("完整路径" to detail.path)
                    if (!detail.dir) {
                        rows.add("大小" to (humanSize(detail.size) + "（" + detail.size + " 字节）"))
                    }
                    rows.add("修改时间" to timeText(detail.mtime))
                    rows.add("访问时间" to timeText(detail.atime))
                    rows.add("权限" to detail.permissions.ifEmpty { "-" })
                    rows.add("属主" to (detail.uid.toString() + " : " + detail.gid))
                    if (detail.linkTarget.isNotEmpty()) {
                        rows.add("指向" to detail.linkTarget)
                    }
                } catch (e: Exception) {
                    failure = sftpReason("读取详情", entry.name, e)
                }
                val done = rows
                val message = failure
                runOnUiThread {
                    if (message != null) error = message else fields = done
                }
            }, "anyboot-sftp-detail").start()
        }
        AlertDialog(
            onDismissRequest = { sftpAskDetail = null },
            title = { Text("文件详情") },
            text = {
                Column {
                    val errorText = error
                    val list = fields
                    when {
                        errorText.isNotEmpty() -> Text(errorText, fontSize = 13.sp, color = palette.bad)
                        list == null -> Row(verticalAlignment = Alignment.CenterVertically) {
                            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                            Spacer(Modifier.width(8.dp))
                            SecondaryText("读取中 …")
                        }
                        else -> list.forEach { row ->
                            Row(Modifier.padding(vertical = 3.dp)) {
                                Text(row.first, fontSize = 12.sp, color = palette.fgDim,
                                    modifier = Modifier.width(68.dp))
                                Text(row.second, fontSize = 13.sp, color = palette.fg,
                                    modifier = Modifier.weight(1f))
                            }
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    sftpAskDetail = null
                    copyClipboard("路径", path)
                }) { Text("复制路径") }
            },
            dismissButton = {
                TextButton(onClick = { sftpAskDetail = null }) { Text("关闭") }
            },
        )
    }

    @Composable
    private fun RenameFileDialog(entry: SftpOps.Entry, server: JumpServer) {
        var name by remember(entry) { mutableStateOf(entry.name) }
        AlertDialog(
            onDismissRequest = { sftpAskRename = null },
            modifier = Modifier.imePadding(),
            properties = DialogProperties(decorFitsSystemWindows = false),
            title = { Text("重命名") },
            text = {
                Column {
                    SecondaryText(entry.name)
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(
                        value = name,
                        onValueChange = { name = it },
                        singleLine = true,
                        placeholder = { Text("新名称", fontSize = 12.sp) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    val target = name.trim()
                    when {
                        target.isEmpty() -> toast("名称不能为空")
                        target.contains('/') -> toast("名称里不能带 /")
                        target == entry.name -> sftpAskRename = null
                        else -> {
                            sftpAskRename = null
                            renameEntry(server, entry, target)
                        }
                    }
                }) { Text("重命名") }
            },
            dismissButton = {
                TextButton(onClick = { sftpAskRename = null }) { Text("取消") }
            },
        )
    }

    @Composable
    private fun DeleteFileDialog(entry: SftpOps.Entry, server: JumpServer) {
        val palette = LocalPalette.current
        AlertDialog(
            onDismissRequest = { sftpAskDelete = null },
            title = { Text("删除") },
            text = {
                Text(
                    if (entry.dir) {
                        "删除目录「" + entry.name + "」？\n里面的东西会一起删掉，删了就找不回来。"
                    } else {
                        "删除文件「" + entry.name + "」？删了就找不回来。"
                    },
                    fontSize = 14.sp,
                    color = palette.fg,
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    sftpAskDelete = null
                    deleteEntry(server, entry)
                }) { Text("删除", color = palette.bad) }
            },
            dismissButton = {
                TextButton(onClick = { sftpAskDelete = null }) { Text("取消") }
            },
        )
    }

    @Composable
    private fun MkdirDialog(server: JumpServer) {
        var name by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { sftpMkdirOpen = false },
            modifier = Modifier.imePadding(),
            properties = DialogProperties(decorFitsSystemWindows = false),
            title = { Text("新建目录") },
            text = {
                Column {
                    SecondaryText("位置：" + sftpPath)
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(
                        value = name,
                        onValueChange = { name = it },
                        singleLine = true,
                        placeholder = { Text("目录名", fontSize = 12.sp) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    val target = name.trim()
                    when {
                        target.isEmpty() -> toast("目录名不能为空")
                        target.contains('/') -> toast("目录名里不能带 /")
                        else -> {
                            sftpMkdirOpen = false
                            mkdirHere(server, target)
                        }
                    }
                }) { Text("新建") }
            },
            dismissButton = {
                TextButton(onClick = { sftpMkdirOpen = false }) { Text("取消") }
            },
        )
    }

    @Composable
    private fun LocateDialog(server: JumpServer) {
        var path by remember { mutableStateOf(sftpPath) }
        AlertDialog(
            onDismissRequest = { sftpLocateOpen = false },
            modifier = Modifier.imePadding(),
            properties = DialogProperties(decorFitsSystemWindows = false),
            title = { Text("定位路径") },
            text = {
                Column {
                    OutlinedTextField(
                        value = path,
                        onValueChange = { path = it },
                        singleLine = true,
                        placeholder = { Text("/home/pi 或 ~/logs", fontSize = 12.sp) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(6.dp))
                    SecondaryText("绝对路径、~ 开头（家目录）、或相对当前目录都行")
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    sftpLocateOpen = false
                    locate(server, path)
                }) { Text("前往") }
            },
            dismissButton = {
                TextButton(onClick = { sftpLocateOpen = false }) { Text("取消") }
            },
        )
    }

    /** 「复制到… / 移动到…」的目标目录选择器：只列目录，能往下钻，点「选这里」才动手 */
    @Composable
    private fun DestPickerDialog(request: SftpPick, server: JumpServer) {
        val palette = LocalPalette.current
        val appContext = applicationContext
        var path by remember { mutableStateOf(request.startDir) }
        var dirs by remember { mutableStateOf<List<SftpOps.Entry>>(emptyList()) }
        var busy by remember { mutableStateOf(true) }
        var error by remember { mutableStateOf("") }

        LaunchedEffect(path) {
            val target = path
            busy = true
            error = ""
            dirs = emptyList()
            Thread({
                var failure: String? = null
                var list: List<SftpOps.Entry> = emptyList()
                try {
                    list = SftpOps.list(appContext, server, target) { title, message ->
                        bus.confirm(title, message)
                    }.filter { it.dir && !it.link }
                        .sortedBy { it.name.lowercase(Locale.ROOT) }
                } catch (e: Exception) {
                    failure = sftpReason("打开", target, e)
                }
                val done = list
                val message = failure
                runOnUiThread {
                    if (path != target) return@runOnUiThread   // 已经钻到别的目录去了
                    busy = false
                    if (message != null) error = message else dirs = done
                }
            }, "anyboot-sftp-dest").start()
        }

        AlertDialog(
            onDismissRequest = { sftpPick = null },
            title = { Text(if (request.cut) "移动到…" else "复制到…") },
            text = {
                Column {
                    SecondaryText(request.label)
                    Spacer(Modifier.height(8.dp))
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            path,
                            fontSize = 13.sp,
                            color = palette.fg,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        Spacer(Modifier.width(6.dp))
                        ChipButton("上级") { path = SftpOps.parent(path) }
                    }
                    when {
                        busy -> Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.padding(vertical = 10.dp),
                        ) {
                            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                            Spacer(Modifier.width(8.dp))
                            SecondaryText("读取中 …")
                        }
                        error.isNotEmpty() -> SecondaryText(error,
                            Modifier.padding(vertical = 8.dp))
                        else -> Column(
                            Modifier
                                .heightIn(max = 280.dp)
                                .verticalScroll(rememberScrollState())
                        ) {
                            if (dirs.isEmpty()) SecondaryText("（没有子目录）")
                            dirs.forEach { dir ->
                                Row(
                                    verticalAlignment = Alignment.CenterVertically,
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .clickable { path = SftpOps.join(path, dir.name) }
                                        .padding(vertical = 10.dp),
                                ) {
                                    Text("📁")
                                    Text(
                                        dir.name,
                                        fontSize = 14.sp,
                                        color = palette.fg,
                                        maxLines = 1,
                                        overflow = TextOverflow.Ellipsis,
                                        modifier = Modifier.padding(start = 8.dp),
                                    )
                                }
                            }
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    val dest = path
                    sftpPick = null
                    transferInto(server, request, dest)
                }) { Text("选这里") }
            },
            dismissButton = {
                TextButton(onClick = { sftpPick = null }) { Text("取消") }
            },
        )
    }

    // ------------------------------------------------------------------
    // 传输页（上传 / 下载任务；发起在「文件」页）
    // ------------------------------------------------------------------

    @Composable
    private fun TransfersPage() {
        transferTick // 订阅任务变化
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            val pending = TransferManager.active()
            val paused = pending.count { it.state == TransferManager.STATE_PAUSED }
            SectionTitle(
                when {
                    pending.isEmpty() -> "进行中"
                    paused == 0 -> "进行中（${pending.size}）"
                    else -> "进行中（${pending.size - paused}）· 已暂停（$paused）"
                }
            )
            if (pending.isEmpty()) {
                EmptyHint(
                    "没有正在传输的文件。到「文件」页点「上传」，或点一个远端文件下载 —— "
                            + "任务都会出现在这里，可以看进度、暂停 / 继续、取消。"
                )
            }
            for (task in pending) {
                TransferRow(task, transferTick)
            }

            SectionTitle("已完成 / 失败")
            val finished = TransferManager.finished()
            if (finished.isEmpty()) {
                EmptyHint("还没有传输记录。")
            }
            for (task in finished) {
                TransferRow(task, transferTick)
            }
            if (finished.isNotEmpty()) {
                TextButton(
                    onClick = { TransferManager.clearFinished() },
                    modifier = Modifier.align(Alignment.CenterHorizontally),
                ) { Text("清空记录") }
            }
            Spacer(Modifier.height(12.dp))
        }
    }

    /**
     * 一条任务。
     *
     * <p>⚠️ `tick` 不是多余的：`Task` 里全是普通字段（volatile long / int），Compose 看不见它们的变化；
     * 而这个函数是**独立的 composable**，参数又是同一个 `Task` 实例 —— 少了这个每变一次就不同的
     * `Int` 参数，整行会被 Compose 判定「入参没变」直接跳过重画，于是进度、按钮全停在最后一帧
     * （表现是「表头已经变了、行里还是旧状态」）。加它纯粹是为了把本行拉起来重读。
     */
    @Composable
    private fun TransferRow(task: TransferManager.Task, tick: Int) {
        tick // 订阅任务变化（见上面的说明，别删）
        val palette = LocalPalette.current
        val uploading = task.direction == TransferManager.DIR_UPLOAD
        val paused = task.state == TransferManager.STATE_PAUSED
        // 续传的标一下，一眼看得出这条是从断点接着传的（不是重头来）
        val resumedTag = if (task.resumed) " · 续传" else ""
        Column(
            Modifier
                .fillMaxWidth()
                .padding(start = 12.dp, top = 10.dp, end = 8.dp, bottom = 10.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    if (uploading) "↑" else "↓",
                    fontSize = 16.sp,
                    color = if (task.isRunning()) palette.accent else palette.fgDim,
                )
                Column(Modifier.weight(1f).padding(horizontal = 8.dp)) {
                    PrimaryText(task.name)
                    SecondaryText(
                        "${if (uploading) "上传到 " else "下载自 "}${task.serverName} · ${task.remoteDir}$resumedTag"
                    )
                }
                when (task.state) {
                    TransferManager.STATE_RUNNING -> {
                        ChipButton("暂停") { TransferManager.pause(task.id) }
                        Spacer(Modifier.width(6.dp))
                        ChipButton("取消") { TransferManager.cancel(this@ServersActivity, task.id) }
                    }
                    TransferManager.STATE_PAUSED -> {
                        ChipButton("继续") { resumeTransfer(task) }
                        Spacer(Modifier.width(6.dp))
                        ChipButton("取消") { TransferManager.cancel(this@ServersActivity, task.id) }
                    }
                    // 已被新任务接替：没有可点的动作，只留一行说明
                    TransferManager.STATE_SUPERSEDED -> Unit
                    else -> ChipButton("重试") { retryTransfer(task) }
                }
            }
            if (task.isPending()) {
                Spacer(Modifier.height(8.dp))
                TransferProgressBar(task.fraction(), task.total <= 0 && !paused)
                Spacer(Modifier.height(4.dp))
                SecondaryText(if (paused) pausedText(task) else progressText(task))
                // 被上次进程中断留下的，额外说明一句（普通暂停不用，行里已经写了「已暂停」）
                if (paused && task.interrupted) {
                    Spacer(Modifier.height(4.dp))
                    Text(
                        task.message,
                        fontSize = 12.sp,
                        color = palette.fgDim,
                        modifier = Modifier.padding(start = 24.dp),
                    )
                }
            } else {
                Spacer(Modifier.height(4.dp))
                Text(
                    task.message,
                    fontSize = 12.sp,
                    color = when (task.state) {
                        TransferManager.STATE_DONE -> palette.ok
                        TransferManager.STATE_FAILED -> palette.bad
                        else -> palette.fgDim
                    },
                    modifier = Modifier.padding(start = 24.dp),
                )
            }
        }
    }

    private fun resumeTransfer(task: TransferManager.Task) {
        val reason = TransferManager.resume(this, task) { title, message ->
            bus.confirm(title, message)
        }
        if (reason == null) {
            toast("接着传：${task.name}（进度见上面「进行中」）")
        } else {
            toast(reason, long = true)
        }
    }

    private fun retryTransfer(task: TransferManager.Task) {
        val reason = TransferManager.retry(this, task) { title, message ->
            bus.confirm(title, message)
        }
        if (reason == null) {
            toast("已重新加入传输：${task.name}（进度见上面「进行中」）")
        } else {
            toast(reason, long = true)
        }
    }

    private fun progressText(task: TransferManager.Task): String {
        if (task.total <= 0) {
            // 总量未知（有的 provider 不给长度）：只报已传多少 + 速度，不假装有百分比
            return "已传 ${TransferManager.humanBytes(task.transferred)} · ${speedText(task)}"
        }
        val done = TransferManager.humanBytes(task.transferred)
        val total = TransferManager.humanBytes(task.total)
        return "$done / $total · ${speedText(task)}"
    }

    /** 暂停行不报速度：那是「停下来那一刻的均值」，续传过、或重启后装回来的都没有意义 */
    private fun pausedText(task: TransferManager.Task): String {
        val size = if (task.total <= 0) {
            "已传 ${TransferManager.humanBytes(task.transferred)}"
        } else {
            "${TransferManager.humanBytes(task.transferred)} / ${TransferManager.humanBytes(task.total)}"
        }
        return "已暂停 · $size"
    }

    private fun speedText(task: TransferManager.Task): String {
        val speed = task.bytesPerSecond()
        if (speed <= 0) {
            return "计算中 …"
        }
        return TransferManager.humanBytes(speed) + "/s"
    }

    /** 细进度条：手绘一条，省得跟 Material3 进度条的 API 形状较劲 */
    @Composable
    private fun TransferProgressBar(fraction: Float, indeterminate: Boolean) {
        val palette = LocalPalette.current
        Box(
            Modifier
                .fillMaxWidth()
                .padding(start = 24.dp)
                .height(4.dp)
                .clip(RoundedCornerShape(2.dp))
                .background(palette.elevate),
        ) {
            if (indeterminate) {
                Box(
                    Modifier
                        .fillMaxWidth()
                        .fillMaxHeight()
                        .background(palette.accent.copy(alpha = 0.35f))
                )
            } else {
                Box(
                    Modifier
                        .fillMaxWidth(fraction.coerceIn(0.01f, 1f))
                        .fillMaxHeight()
                        .background(palette.accent)
                )
            }
        }
    }

    // ------------------------------------------------------------------
    // 小件
    // ------------------------------------------------------------------

    @Composable
    private fun PrimaryText(text: String) {
        Text(
            text,
            fontSize = 15.sp,
            color = LocalPalette.current.fg,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }

    /** 应急：手输 SSH 密码。默认勾「保存密码」——勾了就加密落盘，下次连接免输 */
    @Composable
    private fun ManualPasswordDialog(request: ManualPwdRequest) {
        var password by remember { mutableStateOf("") }
        var remember by remember { mutableStateOf(true) }
        AlertDialog(
            onDismissRequest = { manualPassword = null },
            // 同上：输完密码要能直接点到「连接」，不被键盘挡住
            modifier = Modifier.imePadding(),
            properties = DialogProperties(decorFitsSystemWindows = false),
            title = { Text("输入 SSH 密码") },
            text = {
                Column {
                    Text(
                        request.server.addressLabel() + " · 用户 "
                                + (if (JumpStore.text(request.server.username).isEmpty()) "root"
                        else request.server.username),
                        fontSize = 14.sp,
                        color = LocalPalette.current.fgDim,
                    )
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(
                        value = password,
                        onValueChange = { password = it },
                        singleLine = true,
                        placeholder = {
                            Text(
                                if (remember) "输一次就存下来（下次免输）" else "仅本次连接有效，不保存",
                                fontSize = 12.sp,
                            )
                        },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(checked = remember, onCheckedChange = { remember = it })
                        Text(
                            "保存密码（加密存本机 · 下次免输）",
                            fontSize = 13.sp,
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    manualPassword = null
                    SshSessionManager.shared().setManualPassword(request.server.id, password)
                    if (remember) {
                        JumpStore.rememberPassword(this@ServersActivity, request.server.id, password)
                    }
                    request.onReady.run()
                }) { Text("连接") }
            },
            dismissButton = {
                TextButton(onClick = { manualPassword = null }) { Text("取消") }
            },
        )
    }
}
