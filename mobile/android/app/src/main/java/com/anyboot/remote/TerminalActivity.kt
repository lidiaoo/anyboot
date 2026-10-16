package com.anyboot.remote

import android.annotation.SuppressLint
import android.content.Context
import android.os.Bundle
import android.util.Base64
import android.util.Log
import android.view.ViewGroup
import android.view.inputmethod.InputMethodManager
import android.webkit.JavascriptInterface
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView

/**
 * xterm 终端会话页：WebView 里跑 xterm.js，字节流和 [TerminalSession] 双向桥接。
 * Jetpack Compose / Material 3 重写；会话生命周期照旧由 TerminalRegistry 静态持有。
 */
class TerminalActivity : ComponentActivity() {

    private val bus = ConfirmBus()

    /**
     * 诊断通道：终端链路每一环都留痕（`adb logcat -s ABTERM`）。
     * tlog = 关键状态与异常（常开）；tlogV = 高频事件（每帧数据 / 每次按键），默认关。
     */
    companion object {
        private const val VERBOSE = false

        @JvmStatic
        fun tlog(message: String) {
            Log.i("ABTERM", message)
        }

        @JvmStatic
        fun tlogV(message: String) {
            if (VERBOSE) {
                Log.i("ABTERM", message)
            }
        }
    }

    private var currentId by mutableStateOf("")
    private var tick by mutableIntStateOf(0)   // 标签集合变化时自增
    private var statusMessage by mutableStateOf<String?>("正在连接 …")
    private var ctrlArmed by mutableStateOf(false)
    private var fontSize by mutableIntStateOf(14)
    private var dark by mutableStateOf(true)
    private var killTarget by mutableStateOf<TerminalSession?>(null)
    private var manualPassword by mutableStateOf<ManualPwdRequest?>(null)

    private class ManualPwdRequest(val server: JumpServer, val onReady: Runnable)

    private var terminalView: WebView? = null
    private var current: TerminalSession? = null

    /** 宿主页 xterm 是否就绪（onPageFinished 后为 true） */
    private var pageReady = false

    /** 页面就绪前的下行缓冲（b64），就绪后按序冲给 xterm —— 防积攒回显在 WebView 建好前丢失 */
    private val pendingFeeds = ArrayDeque<String>()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Ui.applySystemBars(this)
        dark = JumpStore.terminalDark(this)
        val jumpId = intent.getStringExtra("jumpId")
        if (jumpId.isNullOrEmpty()) {
            finish()
            return
        }
        currentId = jumpId
        tlog("onCreate jumpId=$jumpId")
        setContent { Root() }
        ensureSession(jumpId, false)
    }

    // ------------------------------------------------------------------
    // 会话获取 / 建立
    // ------------------------------------------------------------------

    /**
     * 找到目标会话；没有就在后台开一条。alreadyOpen=true 表示是「切标签」进来的，
     * 建立失败不用退页面（用户还能切回别的）。
     */
    private fun ensureSession(jumpId: String, alreadyOpen: Boolean) {
        val server = JumpStore.findJump(JumpStore.loadJumps(this), jumpId)
        if (server == null) {
            toast("服务器不存在（可能已删除）")
            finish()
            return
        }
        val session = TerminalRegistry.get(jumpId)
        if (session != null) {
            bind(session)
            return
        }
        // 密码认证但本机没存上密码 → 先弹输入框（应急通道，本次连接有效）
        if (server.isSsh() && SshSessionManager.shared().needsManualPassword(server)) {
            manualPassword = ManualPwdRequest(server) { ensureSession(jumpId, alreadyOpen) }
            return
        }
        statusMessage = "正在连接 ${server.addressLabel()} …"
        val appContext = applicationContext
        val listener = listenerFor(server.id)
        Thread({
            try {
                val fresh = TerminalSession(server, listener)
                if (server.isSsh()) {
                    fresh.startSsh(appContext) { title, message -> bus.confirm(title, message) }
                } else {
                    fresh.startTelnet()
                }
                TerminalRegistry.put(fresh)
                tlog("会话建立成功 id=${server.id} closed=${fresh.isClosed()}")
                runOnUiThread {
                    statusMessage = null
                    val live = TerminalRegistry.get(jumpId) ?: return@runOnUiThread
                    bind(live)
                }
            } catch (error: Exception) {
                val message = error.message ?: "连接失败"
                tlog("会话建立失败: ${error.javaClass.simpleName}: $message")
                runOnUiThread {
                    statusMessage = "连接失败：$message"
                    toast("连接失败：$message", long = true)
                    if (!alreadyOpen) finish()
                }
            }
        }, "anyboot-term-open").start()
    }

    /** 每条会话一个监听器（捕住 serverId，别用 currentId —— 切标签后会张冠李戴） */
    private fun listenerFor(serverId: String): TerminalSession.Listener {
        return object : TerminalSession.Listener {
            override fun onData(bytes: ByteArray) {
                tlogV("down ${bytes.size}B (server=$serverId)")
                val b64 = Base64.encodeToString(bytes, Base64.NO_WRAP)
                runOnUiThread { feed(b64) }
            }

            override fun onClosed(reason: String?) {
                tlog("onClosed server=$serverId reason=$reason")
                runOnUiThread {
                    TerminalRegistry.remove(serverId)
                    // 落一条完成的会话历史（时长 = 结束 - 开始）
                    val history = JumpStore.HistoryEntry()
                    history.serverId = serverId
                    history.startedAt = System.currentTimeMillis()
                    history.endedAt = System.currentTimeMillis()
                    history.serverName = sessionName(serverId)
                    JumpStore.appendHistory(this@TerminalActivity, history)
                    tick++
                    if (serverId == currentId) {
                        statusMessage = "会话已结束" + (if (reason.isNullOrEmpty()) "" else "：$reason")
                        toast("会话结束" + (if (reason.isNullOrEmpty()) "" else "：$reason"), long = true)
                    }
                }
            }
        }
    }

    /** 会话刚被关掉，注册表里拿不到 server 了 —— 名字从服务器配置里补 */
    private fun sessionName(serverId: String): String {
        val server = JumpStore.findJump(JumpStore.loadJumps(this), serverId)
        return server?.let { JumpStore.text(it.name) } ?: ""
    }

    /** JS → 原生上行（桥接方法跑在 WebView 的桥线程，不在主线程） */
    private inner class Bridge {
        @JavascriptInterface
        fun log(text: String) {
            tlog("JS: $text")
        }

        @JavascriptInterface
        fun onData(b64: String) {
            val session = current
            if (session == null || b64.isEmpty()) {
                tlog("JS->up DROP (session=${session != null}, len=${b64.length})")
                return
            }
            try {
                val bytes = Base64.decode(b64, Base64.NO_WRAP)
                tlogV("JS->up ${bytes.size}B")
                session.write(bytes)
            } catch (e: Exception) {
                tlog("JS->up 解码失败: ${e.message}")
            }
        }

        @JavascriptInterface
        fun onResize(columns: String, rows: String) {
            val session = current
            if (session == null) {
                tlog("JS->resize DROP cols=$columns rows=$rows")
                return
            }
            tlogV("JS->resize cols=$columns rows=$rows")
            try {
                session.resize(columns.toInt(), rows.toInt())
            } catch (ignored: Exception) {
                // resize 失败不影响
            }
        }
    }

    /** 绑定一个会话到界面：重挂监听器、切积攒回显、刷标签条 */
    private fun bind(session: TerminalSession) {
        tlog("bind id=${session.server.id} pageReady=$pageReady current=${current?.server?.id}")
        current?.let { if (it !== session) it.detach() }
        current = session
        currentId = session.server.id
        // 会话比 Activity 活得久：监听器可能还挂在已销毁的旧实例上（回显喂进死 WebView → 输入字消失），重挂到本 Activity
        session.setListener(listenerFor(session.server.id))
        val backlog = session.attachAndDrain()
        tlog("bind backlog=${backlog.size}B")
        if (pageReady) {
            // 单 WebView 多会话共用一个 xterm：切会话先清屏，再喂目标会话的积攒回显
            eval("if (term) term.reset()")
            // 会话刚绑上：把 xterm 的真实行列数重报给 PTY。
            // 新会话的 PTY 尺寸是 openShell 里定死的 80x24，与 xterm 对不上会导致换行错位、
            // 回车后输出被滚掉。会话建立前的 resize 上报（current==null）会被丢掉，这里补一次。
            eval("if (window.termSync) termSync()")
        }
        if (backlog.isNotEmpty()) {
            feed(Base64.encodeToString(backlog, Base64.NO_WRAP))
        }
        statusMessage = null
        tick++
    }

    // ------------------------------------------------------------------
    // 界面
    // ------------------------------------------------------------------

    @SuppressLint("SetJavaScriptEnabled")
    @Composable
    private fun Root() {
        AppTheme {
            val palette = LocalPalette.current
            val shellColor = if (dark) TerminalDarkBg else TerminalLightBg
            BackHandler(enabled = true) { finish() }

            Column(Modifier.fillMaxSize().background(shellColor)) {
                // 顶栏
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(palette.card)
                        .padding(start = 4.dp, top = 6.dp, end = 4.dp, bottom = 6.dp),
                ) {
                    BarIcon("‹", "返回") { finish() }
                    val session = TerminalRegistry.get(currentId)
                    Text(
                        session?.server?.name ?: "终端",
                        fontSize = 16.sp,
                        color = palette.fg,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
                    )
                    BarIcon("🌓", "切换终端主题") {
                        dark = !dark
                        JumpStore.setTerminalDark(this@TerminalActivity, dark)
                        eval("termSetTheme('" + (if (dark) "dark" else "light") + "')")
                    }
                    BarIcon("A-", "缩小字号") {
                        fontSize = (fontSize - 1).coerceAtLeast(8)
                        eval("termSetFont($fontSize)")
                    }
                    BarIcon("A+", "放大字号") {
                        fontSize = (fontSize + 1).coerceAtMost(28)
                        eval("termSetFont($fontSize)")
                    }
                    BarIcon("⌨", "弹出/收起键盘") { toggleKeyboard() }
                }

                // 多会话标签条
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(36.dp)
                        .background(palette.elevate)
                        .horizontalScroll(rememberScrollState())
                        .padding(horizontal = 8.dp, vertical = 4.dp),
                ) {
                    tick // 订阅会话注册表变化
                    for (session in TerminalRegistry.all()) {
                        val active = session.server.id == currentId
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier
                                .clip(RoundedCornerShape(14.dp))
                                .background(if (active) palette.accent else palette.card)
                                .padding(horizontal = 4.dp)
                                .height(24.dp),
                        ) {
                            Text(
                                session.server.name,
                                fontSize = 13.sp,
                                color = if (active) Color.White else palette.fg,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier
                                    .clickable {
                                        val target = TerminalRegistry.get(session.server.id)
                                        if (target != null && target !== current) bind(target)
                                    }
                                    .padding(horizontal = 8.dp, vertical = 4.dp),
                            )
                            Text(
                                "×",
                                fontSize = 15.sp,
                                color = if (active) Color(0xCCFFFFFF) else palette.fgFaint,
                                modifier = Modifier
                                    .clickable { killTarget = session }
                                    .padding(horizontal = 6.dp, vertical = 2.dp),
                            )
                        }
                        Spacer(Modifier.width(6.dp))
                    }
                }

                // 终端区 + 连接状态覆盖
                Box(
                    Modifier
                        .fillMaxWidth()
                        .weight(1f)
                        .background(shellColor),
                ) {
                    AndroidView(
                        modifier = Modifier.fillMaxSize(),
                        factory = { ctx ->
                            WebView(ctx).also { view ->
                                terminalView = view
                                view.settings.javaScriptEnabled = true
                                view.addJavascriptInterface(Bridge(), "AndroidTerminal")
                                view.setBackgroundColor(
                                    if (dark) 0xFF04070F.toInt() else 0xFFFFFFFF.toInt()
                                )
                                view.webViewClient = object : WebViewClient() {
                                    override fun onPageFinished(target: WebView, url: String) {
                                        tlog("onPageFinished url=$url pageReady=$pageReady")
                                        val theme = if (dark) "dark" else "light"
                                        view.evaluateJavascript(
                                            "(function(){try{termInit('$theme', $fontSize);return 'ok:" +
                                                    "term='+(term?'yes':'no')}catch(e){return 'ERR:'+e.message}})()"
                                        ) { result -> tlog("termInit -> $result") }
                                        pageReady = true
                                        // 冲就绪前积压的下行（含 bind 时取走的积攒回显）
                                        tlog("flush pending=${pendingFeeds.size}")
                                        while (pendingFeeds.isNotEmpty()) {
                                            eval("termFeed('" + pendingFeeds.removeFirst() + "')")
                                        }
                                        // 几何追踪：容器尺寸是否最终长起来（1 行高问题的取证；VERBOSE 打开时才有）
                                        if (VERBOSE) {
                                            val handler = android.os.Handler(mainLooper)
                                            listOf(300L, 1000L, 3000L).forEach { delay ->
                                                handler.postDelayed({
                                                    view.evaluateJavascript(
                                                        "typeof termGeom==='function'?termGeom():'no'"
                                                    ) { r -> tlog("几何(+${delay}ms) -> $r") }
                                                }, delay)
                                            }
                                        }
                                    }

                                    override fun onRenderProcessGone(
                                        view: WebView,
                                        detail: android.webkit.RenderProcessGoneDetail
                                    ): Boolean {
                                        // 渲染进程崩了（否则表现为无声黑屏+输入全吞，查不到方向）：
                                        // 明示用户重进，别让系统顺手把整个应用杀掉
                                        tlog("onRenderProcessGone didCrash=${detail.didCrash()} pageReady=$pageReady")
                                        pageReady = false
                                        pendingFeeds.clear()
                                        runOnUiThread {
                                            statusMessage = "终端渲染进程崩溃，请退出本页重进"
                                        }
                                        return true
                                    }

                                    override fun onReceivedError(
                                        view: WebView,
                                        request: android.webkit.WebResourceRequest,
                                        error: android.webkit.WebResourceError
                                    ) {
                                        tlog("onReceivedError ${request.url} ${error.errorCode} ${error.description}")
                                    }
                                }
                                tlog("WebView 创建，开始加载 index.html")
                                view.loadUrl("file:///android_asset/terminal/index.html")
                            }
                        },
                        update = { view ->
                            view.setBackgroundColor(
                                if (dark) 0xFF04070F.toInt() else 0xFFFFFFFF.toInt()
                            )
                        },
                    )
                    statusMessage?.let { message ->
                        Text(
                            message,
                            fontSize = 14.sp,
                            color = palette.fgDim,
                            modifier = Modifier.align(Alignment.Center).padding(16.dp),
                        )
                    }
                }

                // 按键栏 + Ctrl 字母行
                KeyBar()
                if (ctrlArmed) LettersRow()
            }

            killTarget?.let { session -> KillDialog(session) }
            manualPassword?.let { request -> ManualPasswordDialog(request) }
            ConfirmDialog(bus)
        }

        DisposableEffect(Unit) {
            val listener = { tick += 1 }
            SessionEvents.register(listener)
            onDispose { SessionEvents.unregister(listener) }
        }
    }

    private fun toggleKeyboard() {
        val manager = getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager
        manager.toggleSoftInput(InputMethodManager.SHOW_FORCED, 0)
    }

    /** 按键栏：Esc / Tab / Ctrl / 方向键 / Enter / 键盘 */
    @Composable
    private fun KeyBar() {
        val palette = LocalPalette.current
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .background(palette.card)
                .horizontalScroll(rememberScrollState())
                .padding(horizontal = 6.dp, vertical = 4.dp),
        ) {
            TerminalKey("Esc") { sendRaw(byteArrayOf(0x1b)) }
            TerminalKey("Tab") { sendRaw(byteArrayOf(0x09)) }
            Text(
                "Ctrl",
                fontSize = 13.sp,
                color = if (ctrlArmed) palette.onAccent else palette.fgDim,
                modifier = Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (ctrlArmed) palette.accent else Color.Transparent)
                    .clickable { ctrlArmed = !ctrlArmed }
                    .padding(horizontal = 14.dp, vertical = 6.dp),
            )
            TerminalKey("↑") { sendRaw(byteArrayOf(0x1b, '['.code.toByte(), 'A'.code.toByte())) }
            TerminalKey("↓") { sendRaw(byteArrayOf(0x1b, '['.code.toByte(), 'B'.code.toByte())) }
            TerminalKey("←") { sendRaw(byteArrayOf(0x1b, '['.code.toByte(), 'D'.code.toByte())) }
            TerminalKey("→") { sendRaw(byteArrayOf(0x1b, '['.code.toByte(), 'C'.code.toByte())) }
            TerminalKey("Enter") { sendRaw(byteArrayOf(0x0d)) }
            TerminalKey("⌨") { toggleKeyboard() }
        }
    }

    /** Ctrl 臂上的字母行（Ctrl 已武装才显示） */
    @Composable
    private fun LettersRow() {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .background(LocalPalette.current.card)
                .horizontalScroll(rememberScrollState())
                .padding(horizontal = 6.dp, vertical = 4.dp),
        ) {
            ('A'..'Z').forEach { letter ->
                val lower = letter.lowercaseChar()
                Text(
                    letter.toString(),
                    fontSize = 13.sp,
                    color = LocalPalette.current.fgDim,
                    modifier = Modifier
                        .clickable(enabled = ctrlArmed) {
                            sendRaw(byteArrayOf((lower - 'a' + 1).toByte()))
                        }
                        .padding(horizontal = 12.dp, vertical = 6.dp),
                )
            }
        }
    }

    @Composable
    private fun TerminalKey(label: String, onClick: () -> Unit) {
        Text(
            label,
            fontSize = 13.sp,
            color = LocalPalette.current.fgDim,
            modifier = Modifier
                .padding(horizontal = 3.dp)
                .clickable(onClick = onClick)
                .padding(horizontal = 11.dp, vertical = 6.dp),
        )
    }

    @Composable
    private fun KillDialog(session: TerminalSession) {
        AlertDialog(
            onDismissRequest = { killTarget = null },
            title = { Text("结束会话") },
            text = { Text("结束「${session.server.name}」的终端会话吗？") },
            confirmButton = {
                TextButton(onClick = {
                    killTarget = null
                    if (session === current) {
                        current?.detach()
                        current = null
                    }
                    session.close("用户结束")
                    TerminalRegistry.remove(session.server.id)
                    tick++
                    if (TerminalRegistry.all().isEmpty()) finish()
                }) { Text("结束") }
            },
            dismissButton = {
                TextButton(onClick = { killTarget = null }) { Text("取消") }
            },
        )
    }

    /** 应急：手输 SSH 密码（只放内存，不保存），输完继续原流程 */
    @Composable
    private fun ManualPasswordDialog(request: ManualPwdRequest) {
        var password by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { manualPassword = null },
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
                        placeholder = { Text("仅本次连接有效，不保存", fontSize = 12.sp) },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                            keyboardType = KeyboardType.Password
                        ),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    manualPassword = null
                    SshSessionManager.shared().setManualPassword(request.server.id, password)
                    request.onReady.run()
                }) { Text("连接") }
            },
            dismissButton = {
                TextButton(onClick = { manualPassword = null }) { Text("取消") }
            },
        )
    }

    // ------------------------------------------------------------------
    // 数据
    // ------------------------------------------------------------------

    private fun sendRaw(bytes: ByteArray) {
        tlogV("KeyBar->write ${bytes.size}B  current=${current?.server?.id}")
        current?.write(bytes)
    }

    private fun feed(b64: String) {
        if (!pageReady) {
            // WebView 还没建好/页面没加载完：先攒着，onPageFinished 里按序冲给 xterm
            pendingFeeds.addLast(b64)
            tlogV("feed 缓冲 (pageReady=false) 队列=${pendingFeeds.size} len=${b64.length}")
            while (pendingFeeds.size > 128) {
                pendingFeeds.removeFirst()   // 上限 ~512KB，保内存
            }
            return
        }
        tlogV("feed->termFeed len=${b64.length}")
        eval("termFeed('$b64')")
    }

    private fun eval(script: String) {
        terminalView?.evaluateJavascript(script, null)
    }

    // ------------------------------------------------------------------
    // 生命周期
    // ------------------------------------------------------------------

    override fun onResume() {
        super.onResume()
        terminalView?.onResume()
        val session = TerminalRegistry.get(currentId)
        tlog("onResume currentId=$currentId registry=${TerminalRegistry.all().size} session=${session != null} current=${current != null}")
        when {
            session == null -> Unit
            session !== current -> bind(session)
            else -> {
                // 监听器一律先重挂到本 Activity：返回栈里可能压着更早的终端页实例，
                // 会话的监听器还指着已销毁的那个（回显喂进死 WebView → 输入被吞）
                session.setListener(listenerFor(session.server.id))
                // 同一会话从后台回来：onPause 时 detach 了，把后台期间积攒的回显补喂上
                val backlog = session.attachAndDrain()
                tlog("onResume 同会话 reattach backlog=${backlog.size}B")
                if (backlog.isNotEmpty()) {
                    feed(Base64.encodeToString(backlog, Base64.NO_WRAP))
                }
                // 从后台回来：软键盘/系统栏可能改过可用高度，重算尺寸并同步给 PTY
                eval("if (window.termSync) termSync()")
            }
        }
    }

    override fun onPause() {
        tlog("onPause")
        terminalView?.onPause()
        current?.detach()
        super.onPause()
    }

    override fun onDestroy() {
        tlog("onDestroy")
        // 会话不关（注册表持有），只把 WebView 拆了
        terminalView?.destroy()
        terminalView = null
        super.onDestroy()
    }
}
