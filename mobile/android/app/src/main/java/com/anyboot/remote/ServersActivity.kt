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
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
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
    private var sftpEntries by mutableStateOf<List<List<String>>>(emptyList()) // [name, kind, size]
    private var sftpBusy by mutableStateOf(false)
    private var sftpError by mutableStateOf("")
    /** 已经拉取过的「服务器|路径」标记 —— 空目录也不能反复重拉（否则死循环） */
    private var sftpLoadedKey by mutableStateOf("")

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Ui.applySystemBars(this)
        jumps.addAll(JumpStore.loadJumps(this))
        keys.addAll(JumpStore.loadKeys(this))
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
            Column(Modifier.fillMaxSize().background(palette.base)) {
                // 标题栏（齿轮 = 终端设置，就摆在 ＋ 左边；图标无文字）
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(palette.card)
                        .padding(8.dp),
                ) {
                    BarIcon("‹", "返回") { finish() }
                    Text(
                        "服务器（SSH / Telnet 跳板机）",
                        fontSize = 16.sp,
                        color = palette.fg,
                        modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
                    )
                    Icon(
                        Icons.Filled.Settings,
                        contentDescription = "终端设置",
                        tint = palette.fgDim,
                        modifier = Modifier
                            .size(20.dp)
                            .clickable {
                                startActivity(
                                    Intent(this@ServersActivity, TerminalSettingsActivity::class.java)
                                )
                            },
                    )
                    BarIcon("＋", "添加服务器") {
                        editTarget = null
                        editOpen = true
                    }
                }

                // 五页：服务器 / 会话 / 密钥 / 传输 / 文件（切页重组，无需叠放显隐）
                Box(Modifier.fillMaxWidth().weight(1f)) {
                    when (currentPage) {
                        0 -> ServersPage()
                        1 -> SessionsPage()
                        2 -> KeysPage()
                        3 -> TransfersPage()
                        else -> FilesPage()
                    }
                }

                // 底部导航：顺序与上面的下标一一对应
                NavigationBar(containerColor = palette.card, tonalElevation = 0.dp) {
                    listOf("服务器", "会话", "密钥", "传输", "文件").forEachIndexed { index, label ->
                        NavigationBarItem(
                            selected = currentPage == index,
                            onClick = { currentPage = index },
                            icon = {},
                            label = {
                                Text(
                                    label,
                                    fontSize = 14.sp,
                                    fontWeight = if (currentPage == index) FontWeight.Bold else FontWeight.Normal,
                                )
                            },
                            colors = androidx.compose.material3.NavigationBarItemDefaults.colors(
                                selectedTextColor = palette.accent,
                                unselectedTextColor = palette.fgDim,
                                indicatorColor = palette.elevate,
                            ),
                        )
                    }
                }
            }

            if (editOpen) EditJumpDialog(editTarget)
            deleteTarget?.let { DeleteJumpDialog(it) }
            killTarget?.let { KillDialog(it) }
            if (generateKeyOpen) GenerateKeyDialog()
            manualPassword?.let { ManualPasswordDialog(it) }
            ConfirmDialog(bus)
        }

        // 会话注册表变化 → 会话页刷新；传输任务变化 → 传输页/文件页刷新
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
                placeholder = { Text("🔍  搜索名称 / 地址", fontSize = 14.sp) },
                shape = RoundedCornerShape(12.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(start = 12.dp, top = 8.dp, end = 12.dp, bottom = 4.dp),
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
            jumps.forEachIndexed { index, server ->
                val name = JumpStore.text(server.name)
                if (query.isNotEmpty()
                    && !name.lowercase(Locale.ROOT).contains(query)
                    && !server.host.lowercase(Locale.ROOT).contains(query)
                ) {
                    return@forEachIndexed
                }
                if (index > 0) ListDivider()
                ServerRow(server, index)
                if (index < expanded.size && expanded[index]) {
                    DetailPanel(server, index)
                }
            }
            Spacer(Modifier.height(12.dp))
        }
    }

    @Composable
    private fun ServerRow(server: JumpServer, index: Int) {
        val palette = LocalPalette.current
        val expand = index < expanded.size && expanded[index]
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .clickable {
                    while (expanded.size < jumps.size) expanded.add(false)
                    expanded[index] = !expanded[index]
                }
                .padding(start = 12.dp, top = 12.dp, end = 8.dp, bottom = 12.dp),
        ) {
            // 字母徽标（FinalShell 的发行版图标用首字母兜底）
            val name = JumpStore.text(server.name)
            Box(
                Modifier
                    .size(36.dp)
                    .clip(RoundedCornerShape(20.dp))
                    .background(palette.accent),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    if (name.isEmpty()) "S" else name.substring(0, 1).uppercase(Locale.ROOT),
                    color = Color.White,
                    fontSize = 15.sp,
                )
            }
            Column(Modifier.weight(1f).padding(horizontal = 10.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        if (name.isEmpty()) server.addressLabel() else name,
                        fontSize = 15.sp,
                        color = palette.fg,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Spacer(Modifier.width(4.dp))
                    Text(
                        " ${server.protocolLabel()} ",
                        fontSize = 10.sp,
                        color = if (server.isSsh()) palette.accent else palette.warn,
                        modifier = Modifier
                            .clip(RoundedCornerShape(4.dp))
                            .background(palette.elevate),
                    )
                    if (server.lastRttMs >= 0) {
                        Spacer(Modifier.width(6.dp))
                        Text("  ${server.lastRttMs}ms", fontSize = 11.sp, color = palette.ok)
                    }
                }
                // 名称没填时主标题已经是地址了，副行别再重复一遍 —— 换成用户/端口摘要
                var secondary = if (name.isEmpty()) {
                    "用户 " + (if (JumpStore.text(server.username).isEmpty()) "root" else server.username) + " · 端口 " + server.port()
                } else {
                    server.addressLabel()
                }
                if (!server.isSsh()) secondary += "（实验性终端）"
                SecondaryText(secondary, Modifier.padding(top = 2.dp))
            }

            if (server.isSsh()) {
                ChipButton("▶ 终端") { openTerminal(server) }
            }
            Text(
                if (expand) "▾" else "▸",
                fontSize = 16.sp,
                color = palette.fgDim,
                modifier = Modifier.padding(start = 10.dp, end = 4.dp),
            )
        }
    }

    /** 展开的详情面板：信息 + 测试连接 / 打开终端 / 编辑 / 删除 */
    @OptIn(ExperimentalLayoutApi::class)
    @Composable
    private fun DetailPanel(server: JumpServer, position: Int) {
        Column(Modifier.padding(start = 48.dp, top = 4.dp, end = 12.dp, bottom = 10.dp)) {
            SecondaryText(
                "端口 " + server.port()
                        + " · 用户 " + (if (JumpStore.text(server.username).isEmpty()) "（未填，连接时用 root）" else server.username)
                        + " · 认证 " + server.authLabel()
            )
            if (JumpStore.text(server.note).isNotEmpty()) {
                SecondaryText("备注：" + server.note)
            }
            if (server.isSsh()) {
                SecondaryText("经由此服务器的电脑：" + JumpStore.computersVia(this@ServersActivity, server.id) + " 台")
            }
            Spacer(Modifier.height(6.dp))
            FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                if (server.isSsh()) {
                    ChipButton("▶ 打开终端") { openTerminal(server) }
                    ChipButton("浏览文件") {
                        currentPage = 4
                        sftpServerId = server.id
                        sftpPath = "/"
                        sftpEntries = emptyList()
                        sftpError = ""
                        sftpLoadedKey = ""
                    }
                }
                ChipButton("测试连接") { testConnection(server) }
                ChipButton("编辑") { editTarget = server; editOpen = true }
                ChipButton("删除") { deleteTarget = server }
            }
            if (server.isSsh() && SshSessionManager.shared().isUp(server.id)) {
                SecondaryText("SSH 会话在线（隧道与终端共享）", Modifier.padding(top = 4.dp))
            }
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
        AlertDialog(
            onDismissRequest = { deleteTarget = null },
            title = { Text("删除服务器") },
            text = {
                Text(
                    "确定删除「" + JumpStore.text(server.name) + "」吗？"
                            + (if (via > 0) "\n\n注意：有 $via 台电脑配置了经由它连接，删除后这些电脑会退回直连。" else "")
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

    @Composable
    private fun EditJumpDialog(existing: JumpServer?) {
        val palette = LocalPalette.current
        val editing = existing != null

        var name by remember(existing) { mutableStateOf(existing?.name ?: "") }
        var isTelnet by remember(existing) { mutableStateOf(editing && existing?.isSsh() == false) }
        var host by remember(existing) { mutableStateOf(existing?.host ?: "") }
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
        // 已存密码要回显 —— 否则用户没法确认到底存没存（解不开等于没存，一并暴露）
        val savedPassword = remember(existing) {
            if (editing) KeyVault.decrypt(existing?.passwordEnc ?: "") else ""
        }

        AlertDialog(
            onDismissRequest = { editOpen = false },
            // 键盘弹出时不能把底部的「保存 / 取消」盖住（对话框已经顶到屏幕上沿、无处可让），
            // 让内容区把 IME 的高度让出来——它本来就是 verticalScroll，能自己压缩。
            modifier = Modifier.imePadding(),
            properties = DialogProperties(decorFitsSystemWindows = false),
            title = { Text(if (editing) "编辑服务器" else "添加服务器") },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    FieldLabel("名称")
                    OutlinedTextField(
                        value = name, onValueChange = { name = it }, singleLine = true,
                        placeholder = { Text("例如：pve / 家里路由器", fontSize = 12.sp) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    // 两个选项各占一行：窄屏下一行塞不下两段中文说明，Row 溢出会把末尾 Text
                    // 压成 0 宽逐字换行、整行撑到 ~300dp（并把它后面的「保存密码」顶出视口）。
                    FieldLabel("协议")
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = !isTelnet, onClick = { isTelnet = false })
                        Text(
                            "SSH（推荐：隧道+终端+文件）",
                            fontSize = 13.sp,
                            modifier = Modifier.weight(1f),
                        )
                    }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = isTelnet, onClick = { isTelnet = true })
                        Text(
                            "Telnet（实验性，仅终端）",
                            fontSize = 13.sp,
                            modifier = Modifier.weight(1f),
                        )
                    }
                    FieldLabel("地址（host:port）")
                    OutlinedTextField(
                        value = host, onValueChange = { host = it }, singleLine = true,
                        placeholder = { Text("host:port 或 user@host:port（如 1.2.3.4:22）", fontSize = 12.sp) },
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    FieldLabel("用户名")
                    OutlinedTextField(
                        value = username, onValueChange = { username = it }, singleLine = true,
                        placeholder = { Text("root", fontSize = 12.sp) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    FieldLabel("认证方式")
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = !authKey, onClick = { authKey = false })
                        Text("密码", fontSize = 13.sp)
                        Spacer(Modifier.width(8.dp))
                        RadioButton(selected = authKey, onClick = { authKey = true })
                        Text("密钥", fontSize = 13.sp)
                    }
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
                        modifier = Modifier.fillMaxWidth(),
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(checked = savePassword, onCheckedChange = { savePassword = it })
                        Text("保存密码（加密存本机 · 连接免输）", fontSize = 13.sp)
                    }
                    FieldLabel("密钥")
                    Box {
                        OutlinedTextField(
                            value = if (keys.isEmpty()) "（还没有密钥，先到「密钥」页导入/生成）"
                            else keys[keyIndex.coerceIn(0, keys.size - 1)].let {
                                JumpStore.text(it.name) + "（" + it.algorithm + "）"
                            },
                            onValueChange = {}, readOnly = true, singleLine = true,
                            enabled = keys.isNotEmpty(),
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
                        modifier = Modifier.fillMaxWidth(),
                    )
                    FieldLabel("备注")
                    OutlinedTextField(
                        value = note, onValueChange = { note = it }, singleLine = true,
                        placeholder = { Text("可选", fontSize = 12.sp) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    FieldLabel(
                        "SSH：支持隧道（外网连家里的电脑）+ 终端 + 文件；"
                                + "Telnet：只有终端，明文传输，仅实验。"
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    if (persistJump(existing, name, isTelnet, host, username, authKey,
                            password, savePassword, keyIndex, passphrase, note)) {
                        editOpen = false
                    }
                }) { Text("保存") }
            },
            dismissButton = {
                Row {
                    if (editing) {
                        TextButton(onClick = {
                            // 先把表单落下来再测（改了地址没保存就测会测到旧的）
                            if (persistJump(existing, name, isTelnet, host, username, authKey,
                                    password, savePassword, keyIndex, passphrase, note)) {
                                editOpen = false
                                testConnection(existing)
                            }
                        }) { Text("测试连接") }
                        TextButton(onClick = { editOpen = false }) { Text("取消") }
                    } else {
                        TextButton(onClick = { editOpen = false }) { Text("取消") }
                    }
                }
            },
        )
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
    ): Boolean {
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

    @Composable
    private fun KeysPage() {
        val importLauncher = rememberLauncherForActivityResult(
            ActivityResultContracts.OpenDocument()
        ) { uri -> if (uri != null) importKey(uri) }

        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            SectionTitle("密钥（私钥只存本机，Keystore 加密落盘）")
            if (keys.isEmpty()) {
                EmptyHint(
                    "还没有密钥。导入已有的私钥，或直接在本机生成一对 —— "
                            + "把公钥放到服务器上（authorized_keys）就能免密登录。"
                )
            }
            for (entry in keys) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(start = 12.dp, top = 12.dp, end = 8.dp, bottom = 12.dp),
                ) {
                    Column(Modifier.weight(1f)) {
                        PrimaryText(if (JumpStore.text(entry.name).isEmpty()) "未命名密钥" else entry.name)
                        SecondaryText(
                            entry.algorithm
                                    + (if (JumpStore.text(entry.fingerprint).isEmpty()) "" else " · " + entry.fingerprint)
                                    + " · 关联 " + JumpStore.keysInUse(jumps, entry.id) + " 台服务器"
                        )
                    }
                    ChipButton("复制公钥") {
                        if (JumpStore.text(entry.publicKey).isEmpty()) {
                            toast("这把密钥没有记录公钥")
                        } else {
                            copyClipboard(JumpStore.text(entry.name) + " 公钥", entry.publicKey)
                        }
                    }
                    ChipButton("删除") {
                        val inUse = JumpStore.keysInUse(jumps, entry.id)
                        if (inUse > 0) {
                            toast("有 $inUse 台服务器在用这把密钥，先改它们的认证方式", long = true)
                        } else {
                            deleteTargetKey = entry
                        }
                    }
                }
            }
            TextButton(
                onClick = {
                    importLauncher.launch(arrayOf("*/*"))
                },
                modifier = Modifier.align(Alignment.CenterHorizontally),
            ) { Text("导入私钥文件") }
            TextButton(
                onClick = { generateKeyOpen = true },
                modifier = Modifier.align(Alignment.CenterHorizontally),
            ) { Text("在本机生成密钥对") }
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

    @Composable
    private fun FilesPage() {
        val uploadLauncher = rememberLauncherForActivityResult(
            ActivityResultContracts.OpenDocument()
        ) { uri -> if (uri != null) uploadPicked(uri) }

        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            SectionTitle("文件（SFTP，仅 SSH 服务器）")

            val sshServers = jumps.filter { it.isSsh() }
            if (sshServers.isEmpty()) {
                EmptyHint("没有 SSH 服务器 —— 先到「服务器」页添加。Telnet 不支持文件传输。")
                return@Column
            }
            var serverOpen by remember { mutableStateOf(false) }
            val selectedId = if (sshServers.any { it.id == sftpServerId }) sftpServerId
            else sshServers[0].id.also { sftpServerId = it }
            val selected = sshServers.first { it.id == selectedId }

            Box {
                OutlinedTextField(
                    value = if (JumpStore.text(selected.name).isEmpty()) selected.addressLabel()
                    else JumpStore.text(selected.name),
                    onValueChange = {}, readOnly = true, singleLine = true,
                    label = { Text("服务器") },
                    trailingIcon = { Text(if (serverOpen) "▴" else "▾") },
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 12.dp, vertical = 4.dp)
                        .clickable { serverOpen = true },
                )
                DropdownMenu(expanded = serverOpen, onDismissRequest = { serverOpen = false }) {
                    sshServers.forEach { server ->
                        DropdownMenuItem(
                            text = {
                                Text(
                                    if (JumpStore.text(server.name).isEmpty()) server.addressLabel()
                                    else JumpStore.text(server.name)
                                )
                            },
                            onClick = {
                                serverOpen = false
                                if (server.id != sftpServerId) {
                                    sftpServerId = server.id
                                    sftpPath = "/"
                                    sftpEntries = emptyList()
                                    sftpError = ""
                                    sftpLoadedKey = ""
                                }
                            },
                        )
                    }
                }
            }

            // 面包屑 + 上传
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 4.dp),
            ) {
                Text(sftpPath, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f))
                ChipButton("上级") {
                    if (sftpPath != "/") {
                        val cut = sftpPath.lastIndexOf('/')
                        sftpPath = if (cut <= 0) "/" else sftpPath.substring(0, cut)
                        sftpEntries = emptyList()
                        sftpError = ""
                        sftpLoadedKey = ""
                    }
                }
                Spacer(Modifier.width(4.dp))
                ChipButton("上传") { uploadLauncher.launch(arrayOf("*/*")) }
            }

            if (!SshSessionManager.shared().isUp(sftpServerId)) {
                SecondaryText("　（先在「服务器」页连上这台服务器，或点行内「▶ 终端」——"
                        + " SFTP 复用同一条 SSH 会话）", Modifier.padding(horizontal = 12.dp))
            }

            when {
                sftpError.isNotEmpty() -> SecondaryText("　读取失败：$sftpError",
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
                else -> {
                    sftpEntries.forEach { entry ->
                        val (entryName, kind, sizeText) = entry
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable {
                                    if (kind == "dir") {
                                        sftpPath = if (sftpPath == "/") "/$entryName"
                                        else "$sftpPath/$entryName"
                                        sftpEntries = emptyList()
                                        sftpError = ""
                                        sftpLoadedKey = ""
                                    } else {
                                        startDownload(entryName)
                                    }
                                }
                                .padding(start = 12.dp, top = 12.dp, end = 8.dp, bottom = 12.dp),
                        ) {
                            Text(if (kind == "dir") "📁" else "📄")
                            Column(Modifier.weight(1f).padding(horizontal = 8.dp)) {
                                PrimaryText(entryName)
                                if (kind == "file") {
                                    SecondaryText(humanSize(sizeText.toLong()))
                                }
                            }
                        }
                    }
                }
            }

            // 目录还没拉取过时自动加载一次
            LaunchedEffect(sftpServerId, sftpPath, sftpLoadedKey) {
                if (sftpLoadedKey != "$sftpServerId|$sftpPath" && !sftpBusy
                    && sftpServerId.isNotEmpty() && sshServers.isNotEmpty()) {
                    loadSftpListing()
                }
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
                    sftpEntries = emptyList()
                    sftpError = ""
                    sftpLoadedKey = ""
                }
            }
        }
    }

    /** 后台拉目录（结果落 sftpEntries / sftpError 状态） */
    private fun loadSftpListing() {
        val serverId = sftpServerId
        val path = sftpPath
        sftpBusy = true
        val appContext = applicationContext
        Thread({
            val entries = ArrayList<List<String>>()
            var error: String? = null
            try {
                val server = JumpStore.findJump(JumpStore.loadJumps(appContext), serverId)
                    ?: throw Exception("服务器不存在")
                val session: Session = SshSessionManager.shared().ensureSession(
                    appContext, server
                ) { title, message -> bus.confirm(title, message) }
                @Suppress("UNCHECKED_CAST")
                val sftp = session.openChannel("sftp") as ChannelSftp
                sftp.connect(15000)
                try {
                    val raw = sftp.ls(path)
                    for (item in raw) {
                        val entryName = item.filename
                        if ("." == entryName || ".." == entryName) continue
                        val attrs: SftpATTRS = item.attrs
                        entries.add(listOf(
                            entryName,
                            if (attrs.isDir) "dir" else "file",
                            attrs.size.toString(),
                        ))
                    }
                } finally {
                    sftp.disconnect()
                }
            } catch (failure: Exception) {
                error = failure.message ?: failure.javaClass.simpleName
            }
            val result = entries
            val failureMessage = error
            runOnUiThread {
                sftpBusy = false
                if (serverId != sftpServerId || path != sftpPath) {
                    return@runOnUiThread   // 用户已经切走了
                }
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
        TransferManager.startUpload(applicationContext, server, sftpPath, uri, name) { title, message ->
            bus.confirm(title, message)
        }
        toast("已加入传输：$name（进度见「传输」页）")
    }

    // ------------------------------------------------------------------
    // 传输页（上传 / 下载任务；发起在「文件」页）
    // ------------------------------------------------------------------

    @Composable
    private fun TransfersPage() {
        transferTick // 订阅任务变化
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            val running = TransferManager.active()
            SectionTitle(if (running.isEmpty()) "进行中" else "进行中（${running.size}）")
            if (running.isEmpty()) {
                EmptyHint(
                    "没有正在传输的文件。到「文件」页点「上传」，或点一个远端文件下载 —— "
                            + "任务都会出现在这里，可以看进度、取消。"
                )
            }
            for (task in running) {
                TransferRow(task)
            }

            SectionTitle("已完成 / 失败")
            val finished = TransferManager.finished()
            if (finished.isEmpty()) {
                EmptyHint("还没有传输记录。")
            }
            for (task in finished) {
                TransferRow(task)
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

    @Composable
    private fun TransferRow(task: TransferManager.Task) {
        val palette = LocalPalette.current
        val uploading = task.direction == TransferManager.DIR_UPLOAD
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
                        (if (uploading) "上传到 " else "下载自 ")
                                + task.serverName + " · " + task.remoteDir
                    )
                }
                if (task.isRunning()) {
                    ChipButton("取消") { TransferManager.cancel(task.id) }
                } else {
                    ChipButton("重试") { retryTransfer(task) }
                }
            }
            if (task.isRunning()) {
                Spacer(Modifier.height(8.dp))
                TransferProgressBar(task.fraction(), task.total <= 0)
                Spacer(Modifier.height(4.dp))
                SecondaryText(progressText(task))
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
