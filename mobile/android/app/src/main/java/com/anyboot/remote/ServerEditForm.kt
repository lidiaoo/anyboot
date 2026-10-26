package com.anyboot.remote

import android.content.Context
import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.util.Locale

/**
 * 电脑档案的编辑表单（**一套实现，两处宿主**）。
 *
 * 为什么要抽出来：手机上有两个入口都要改同一份 [ServerProfile] ——
 *   · [MainActivity] 的「服务端」入口（全屏 WebView 的抽屉里），外壳是 AlertDialog；
 *   · [ComputerListActivity] 的行菜单「编辑这台电脑」，外壳是底部 sheet。
 *
 * 以前只有前者，后者点「管理这台电脑」被甩进了 SSH 跳板机页。现在两处共用这套
 * [ServerEditFormBody]，字段与 PC 端 `ui/hub.html` 的 `rsEditOverlay` 一一对应：
 * 分组 / 名称 / 快速识别 / 地址+MAC 列表 / token / 跳板机 / 多网卡探测三选一 /
 * 局域网电源路由逐条（动作·模式·广播地址·端口·MAC·发包数·重试）。
 *
 * 表单只管「收集与校验」，写盘与副作用留给宿主（两边保存语义不同：
 * MainActivity 要合并同一分组、重连标签、探机器标识；电脑列表页只改这条档案）。
 */

/**
 * 编辑表单里的一个地址条目：地址 + 这块（这几块）网卡的 MAC。
 *
 * 用不可变数据类 + 替换元素，Compose 的 mutableStateList 才会重组成列表。
 */
internal data class AddrEntry(val address: String, val macs: List<String> = emptyList())

/**
 * 编辑中的一条配对（不可变；改行 = 整体替换元素）。
 *
 * 与 [RemotePair] 的差别只在「界面友好」：超时存秒文本（空 = 继承区块默认），
 * `lane` 不单独存（由地址形态推导），来源字段（mid / os / host / tunnelId）只读展示。
 */
internal data class PairEdit(
    val id: String,
    /** 区块 id：cloudflare / ts / tsnet */
    val kind: String,
    val addr: String = "",
    val token: String = "",
    val mid: String = "",
    val os: String = "",
    val host: String = "",
    val tunnelId: String = "",
    val note: String = "",
    /** 条目自己的探测超时（秒文本，空 = 继承区块 → 全局） */
    val timeoutText: String = "",
    val enabled: Boolean = true,
    val createdAt: Long = 0L,
) {
    /** 来源列：host · os；都空时退回 mid 短号 */
    val sourceLabel: String
        get() {
            val parts = ArrayList<String>()
            if (host.isNotBlank()) parts.add(host.trim())
            if (os.isNotBlank()) parts.add(os.trim())
            if (parts.isEmpty() && mid.isNotBlank()) {
                parts.add(mid.trim().let { if (it.length > 8) it.substring(0, 8) else it })
            }
            return parts.joinToString(" · ")
        }

    companion object {
        fun from(pair: RemotePair): PairEdit = PairEdit(
            id = pair.id.orEmpty(),
            kind = pair.kind.orEmpty(),
            addr = ServerProfile.normalizeRemoteOrigin(pair.addr),
            token = pair.token.orEmpty(),
            mid = pair.mid.orEmpty(),
            os = pair.os.orEmpty(),
            host = pair.host.orEmpty(),
            tunnelId = pair.tunnelId.orEmpty(),
            note = pair.note.orEmpty(),
            timeoutText = if (pair.probeTimeoutMs > 0) {
                val seconds = pair.probeTimeoutMs / 1000.0
                if (seconds == seconds.toLong().toDouble()) seconds.toLong().toString()
                else String.format(Locale.US, "%.1f", seconds)
            } else "",
            enabled = pair.enabled,
            createdAt = pair.createdAt,
        )
    }
}

/** 表单的全部可编辑状态；宿主保存时读这里的当前值 */
/** 五通道 id（连接优先级用；与 ServerProfile 的 ROUTE_* 常量一致） */
private val ROUTE_IDS_ALL = listOf("lan", "jump", "tunnel", "ts", "funnel", "tsnet")
private val ROUTE_LABELS_ZH = mapOf(
    "lan" to "直连",
    "jump" to "跳板机",
    "tunnel" to "Cloudflare 隧道",
    "ts" to "Tailscale 直连",
    "funnel" to "Tailscale Funnel",
    "tsnet" to "Tailscale Funnel（内建）",
)

internal class ServerEditFormState(
    val profile: ServerProfile?,
    context: Context,
) {
    val editing: Boolean = profile != null

    var name by mutableStateOf(profile?.name ?: "")

    /** 地址：有序列表（第一个 = 连接用），由「快速识别」和「手动添加」往里面加 */
    val addressEntries = mutableStateListOf<AddrEntry>().apply {
        profile?.let { saved ->
            saved.addresses.forEach { address ->
                add(AddrEntry(address, saved.macsForHost(address)))
            }
        }
    }

    var quickText by mutableStateOf("")
    var quickHint by mutableStateOf("")
    var newHost by mutableStateOf("")
    var newMacs by mutableStateOf("")

    /** 粘进来的原文（token 只在 URL 里，归一化后就丢了，所以留一份原文） */
    var pastedRaw by mutableStateOf(profile?.addressText() ?: "")
    var token by mutableStateOf(profile?.token ?: "")
    // 已存令牌回显（打码 + 小眼睛）；token 输入框只承接「替换」的新值
    var tokenShown by mutableStateOf(false)
    val savedToken: String get() = profile?.token ?: ""

    // ---- 远程配对（三个区块，每个区块可挂多条；docs/remote-pairs.md §4.1） ----
    /**
     * 编辑中的配对行。用不可变 data class + 整体替换元素，Compose 才会重组。
     * 深拷贝自档案（取消编辑时原档案不受影响）。
     */
    val pairItems = mutableStateListOf<PairEdit>().apply {
        profile?.pairs?.forEach { add(PairEdit.from(it)) }
    }
    /** 粘贴 / 扫码的输入缓冲 */
    var remoteText by mutableStateOf("")
    var remoteHint by mutableStateOf("")
    /** 区块内部探测策略（区块 id → 模式）；缺 key = 并发首个 */
    val blockModes = mutableStateMapOf<String, String>().apply {
        profile?.blockProbeMode?.forEach { (block, mode) ->
            put(block, ServerProfile.normalizeProbeMode(mode))
        }
    }
    /** 区块默认探测超时（区块 id → 秒文本）；空 = 继承全局默认 */
    val blockTimeoutTexts = mutableStateMapOf<String, String>().apply {
        profile?.blockTimeoutsMs?.forEach { (block, ms) ->
            if (ms != null && ms > 0) put(block, secondsText(ms))
        }
    }
    /** 折叠区：连接策略 / 局域网电源路由（默认收起 —— 配一次就不动） */
    var connFoldOpen by mutableStateOf(false)
    var powerFoldOpen by mutableStateOf(false)

    /** 毫秒 → 界面秒文本（整秒不带小数点，省得「2.0」看着别扭） */
    private fun secondsText(ms: Int): String {
        val seconds = ms / 1000.0
        return if (seconds == seconds.toLong().toDouble()) seconds.toLong().toString()
        else String.format(Locale.US, "%.1f", seconds)
    }

    /** 有没有配对（保存时据此决定是否 clearRemote） */
    fun hasPairs(): Boolean = pairItems.any { it.addr.isNotBlank() }

    /** 某个区块里的配对（保持列表顺序） */
    fun pairsOf(block: String): List<PairEdit> = pairItems.filter { it.kind == block }

    fun indexOfPair(id: String): Int = pairItems.indexOfFirst { it.id == id }

    /** 改一行（地址 / 令牌 / 备注 / 超时 / 启用） */
    fun updatePair(id: String, transform: (PairEdit) -> PairEdit) {
        val index = indexOfPair(id)
        if (index >= 0) pairItems[index] = transform(pairItems[index])
    }

    /** 在区块末尾加一行空条目（地址留空 = 保存时丢弃） */
    fun addPair(kind: String) {
        if (pairItems.size >= ServerProfile.MAX_PAIRS) {
            remoteHint = "最多 ${ServerProfile.MAX_PAIRS} 条配对 —— 先删掉几条再加"
            return
        }
        pairItems.add(
            PairEdit(
                id = "new-" + System.nanoTime(),
                kind = kind,
                createdAt = System.currentTimeMillis(),
            ),
        )
    }

    fun removePair(id: String) {
        val index = indexOfPair(id)
        if (index >= 0) pairItems.removeAt(index)
    }

    /** 区块内上移 / 下移（跨区块不让动：区块顺序由「连接优先级」决定） */
    fun movePair(id: String, delta: Int) {
        val index = indexOfPair(id)
        if (index < 0) return
        val kind = pairItems[index].kind
        val sameBlock = pairItems.withIndex().filter { it.value.kind == kind }.map { it.index }
        val at = sameBlock.indexOf(index)
        val target = at + delta
        if (at < 0 || target < 0 || target >= sameBlock.size) return
        val moveTo = sameBlock[target]
        val item = pairItems.removeAt(index)
        pairItems.add(moveTo, item)
    }

    /** 清空所有配对（「解除配对」按钮）：保存即解绑 */
    fun clearPairs() {
        pairItems.clear()
        remoteText = ""
        remoteHint = "已清空 —— 点「保存」即解除配对（地址与配对令牌一起删掉）"
    }

    /** 通道 id（tunnel / ts / funnel / tsnet）：由区块 + lane/地址推导，与后端 route_kind 同语义 */
    fun remoteKindOf(edit: PairEdit): String {
        return when (edit.kind) {
            RemotePair.KIND_CLOUDFLARE -> ServerProfile.ROUTE_TUNNEL
            RemotePair.KIND_TSNET -> ServerProfile.ROUTE_TSNET
            RemotePair.KIND_TS ->
                if (ServerProfile.isFunnelOrigin(edit.addr)) ServerProfile.ROUTE_FUNNEL
                else ServerProfile.ROUTE_TS
            else -> ""
        }
    }

    /** 区块默认超时：界面秒 → 毫秒；空 / 非法 = 0（继承全局） */
    fun blockTimeoutMsFor(block: String): Int {
        val raw = blockTimeoutTexts[block]?.trim().orEmpty()
        if (raw.isEmpty()) return 0
        val value = raw.toDoubleOrNull() ?: return 0
        return ServerProfile.clampProbeTimeoutMs((value * 1000).toInt())
    }

    /** 保存用：编辑中的行 → RemotePair 列表（地址为空的行丢掉） */
    fun pairsForSave(): List<RemotePair> {
        val out = ArrayList<RemotePair>()
        for (edit in pairItems) {
            val addr = ServerProfile.normalizeRemoteOrigin(edit.addr)
            if (addr.isEmpty()) continue
            val pair = RemotePair()
            pair.id = if (edit.id.startsWith("new-")) RemotePair.newId() else edit.id
            pair.kind = edit.kind
            pair.addr = addr
            pair.lane = if (edit.kind == RemotePair.KIND_TS) {
                if (ServerProfile.isFunnelOrigin(addr)) RemotePair.LANE_FUNNEL else RemotePair.LANE_DIRECT
            } else ""
            pair.token = edit.token.trim()
            pair.probeTimeoutMs = if (edit.timeoutText.trim().isEmpty()) 0 else {
                ServerProfile.clampProbeTimeoutMs(
                    ((edit.timeoutText.trim().toDoubleOrNull() ?: 0.0) * 1000).toInt(),
                )
            }
            pair.enabled = edit.enabled
            pair.mid = edit.mid
            pair.os = edit.os
            pair.host = edit.host
            pair.tunnelId = edit.tunnelId
            pair.note = edit.note.trim()
            pair.createdAt = edit.createdAt
            out.add(pair)
        }
        return out
    }

    /** 任一配对已配 = 有远程接入 */
    fun hasRemoteSlots(): Boolean = hasPairs()

    /** 通道当前是否可用（与 PC rsRouteAvailable / 后端 routeCandidates 同语义） */
    fun routeAvailable(kind: String): Boolean = when (kind) {
        "lan" -> addressEntries.any { it.address.isNotBlank() }
        "jump" -> addressEntries.any { it.address.isNotBlank() } && jumpIndex > 0
        else -> pairItems.any { it.enabled && it.addr.isNotBlank() && remoteKindOf(it) == kind }
    }

    /** 通道副标：地址 / 没配置的原因 */
    fun routeSub(kind: String): String = when (kind) {
        "lan" -> addressEntries.firstOrNull { it.address.isNotBlank() }?.address ?: "未填局域网地址"
        "jump" -> if (jumpIndex > 0) "经 " + jumpLabel(jumps[jumpIndex - 1]) else "未选跳板机档案"
        "tunnel" -> firstAddrOf(ServerProfile.ROUTE_TUNNEL, "未配置隧道地址")
        "ts" -> firstAddrOf(ServerProfile.ROUTE_TS, "未配置 100.x 直连地址")
        "funnel" -> firstAddrOf(ServerProfile.ROUTE_FUNNEL, "未配置（系统 Tailscale 的 *.ts.net 地址）")
        "tsnet" -> firstAddrOf(ServerProfile.ROUTE_TSNET, "未配置（内置引擎的 *.ts.net 地址）")
        else -> ""
    }

    private fun firstAddrOf(kind: String, empty: String): String =
        pairItems.firstOrNull { it.addr.isNotBlank() && remoteKindOf(it) == kind }?.addr ?: empty

    /** 上移 / 下移（移动端不做拖动手势，按钮兜底） */
    fun routeMove(kind: String, delta: Int) {
        val from = routeOrder.indexOf(kind)
        val to = from + delta
        if (from < 0 || to < 0 || to >= routeOrder.size) return
        routeOrder.removeAt(from)
        routeOrder.add(to, kind)
    }

    /** 令牌打码：前 4 后 4 可见，其余 •；太短的整串打码 */
    fun maskToken(value: String): String {
        val text = value.trim()
        if (text.isEmpty()) return ""
        if (text.length <= 12) return "•".repeat(text.length.coerceAtLeast(6))
        return text.take(4) + "••••••••••" + text.takeLast(4)
    }

    /**
     * 粘贴框 / 扫码之后的分拣：解析 → 追加或更新一条配对。
     *
     * <p>归档规则（{@code docs/remote-pairs.md} §4.6）：同 {@code (mid, kind)} 更新那一条
     * （保留位置与原 id），否则追加到该区块末尾。
     *
     * <p>老配对码（无 {@code v=2}）会被拒收并给出明确提示 —— 老串没有 mid/type，
     * 硬解出来只会「配一个顶掉另一个」。
     */
    fun applyRemoteHint() {
        val raw = remoteText.trim()
        if (raw.isEmpty()) {
            remoteHint = ""
            return
        }
        val parsed = ServerProfile.parsePair(raw)
        if (!parsed.ok()) {
            remoteHint = parsed.error.ifEmpty { "没认出配对码" }
            return
        }
        val addr = ServerProfile.normalizeRemoteOrigin(parsed.tunnel)
        val mid = parsed.mid.trim()
        // 先按 (mid, kind) 找，没 mid 时按 (addr, kind) 找 —— 与 PC 端同一套幂等键
        var index = -1
        pairItems.forEachIndexed { i, item ->
            if (index >= 0 || item.kind != parsed.kind) return@forEachIndexed
            val sameMid = mid.isNotEmpty() && item.mid == mid
            val sameAddr = mid.isEmpty() &&
                ServerProfile.normalizeRemoteOrigin(item.addr) == addr
            if (sameMid || sameAddr) index = i
        }
        if (index >= 0) {
            val old = pairItems[index]
            pairItems[index] = old.copy(
                addr = addr,
                token = parsed.token.ifEmpty { old.token },
                mid = mid.ifEmpty { old.mid },
                os = parsed.os.ifEmpty { old.os },
                host = parsed.host.ifEmpty { old.host },
                tunnelId = parsed.tunnelId.ifEmpty { old.tunnelId },
                enabled = true,
            )
            remoteHint = "已更新「" + RemotePair.blockLabel(parsed.kind) + "」里的同一条配对：" + addr
        } else {
            if (pairItems.size >= ServerProfile.MAX_PAIRS) {
                remoteHint = "最多 ${ServerProfile.MAX_PAIRS} 条配对 —— 先删掉几条再加"
                return
            }
            pairItems.add(
                PairEdit(
                    id = "new-" + System.nanoTime(),
                    kind = parsed.kind,
                    addr = addr,
                    token = parsed.token,
                    mid = mid,
                    os = parsed.os,
                    host = parsed.host,
                    tunnelId = parsed.tunnelId,
                    createdAt = System.currentTimeMillis(),
                ),
            )
            remoteHint = "已加入「" + RemotePair.blockLabel(parsed.kind) + "」：" + addr
        }
        remoteText = ""
    }

    val jumps: List<JumpServer> = JumpStore.loadJumps(context)
    var jumpIndex by mutableStateOf(
        if (profile != null) {
            val found = jumps.indexOfFirst { it.id == profile.jumpServerId }
            if (found >= 0) found + 1 else 0
        } else 0
    )
    var jumpOpen by mutableStateOf(false)

    var probeMode by mutableStateOf(
        if (profile != null) ServerProfile.normalizeProbeMode(profile.probeMode ?: "")
        else ServerProfile.PROBE_PARALLEL_FIRST
    )

    /**
     * **连接优先级的探测方式**（六个通道之间怎么试，作用于所有通道元素）：与
     * [probeMode]（多网卡探测，只管同一通道里的多个地址）是两个不同粒度的独立开关。
     */
    var routeProbeMode by mutableStateOf(
        if (profile != null) ServerProfile.normalizeProbeMode(profile.routeProbeMode)
        else ServerProfile.PROBE_PARALLEL_FIRST
    )

    /**
     * 探测超时（界面单位是**秒**，落盘是毫秒）：对局域网地址与所有远程通通生效。
     *
     * <p>为什么要有：公网隧道（尤其 Funnel / tsnet 走中继时）握手一次可能要好几秒，
     * 2 秒的默认值会把「其实通、只是慢」的通道判死；反过来局域网地址又不该等这么久。
     * 留空 = 用默认 2 秒（与 PC 端 probe_timeout_ms 同一套边界：0.2 ~ 30 秒）。
     */
    var probeTimeoutText by mutableStateOf(
        if (profile != null && profile.probeTimeoutMs > 0) {
            val seconds = profile.probeTimeoutMs / 1000.0
            if (seconds == seconds.toLong().toDouble()) seconds.toLong().toString()
            else String.format(Locale.US, "%.1f", seconds)
        } else {
            ""
        }
    )

    /** 界面上的秒 → 落盘毫秒（空 / 非法 → 默认 2 秒，越界由 clamp 夹） */
    fun probeTimeoutMs(): Int {
        val value = probeTimeoutText.trim().toDoubleOrNull() ?: return ServerProfile.DEFAULT_PROBE_TIMEOUT_MS
        return ServerProfile.clampProbeTimeoutMs((value * 1000).toInt())
    }

    /**
     * **每个通道自己的探测超时**（通道 id → 秒文本）。空 = 用上面的默认值。
     * 为什么要分开：局域网 RTT <10ms，2 秒都算宽；Funnel / 内置 tsnet 走中继时
     * 首次 TLS 握手可能好几秒 —— 一个全局值必然一边偏松一边偏紧。
     */
    val routeTimeoutTexts = mutableStateMapOf<String, String>().apply {
        profile?.routeTimeoutsMs?.forEach { (kind, ms) ->
            if (ms != null && ms > 0) {
                val seconds = ms / 1000.0
                put(
                    kind,
                    if (seconds == seconds.toLong().toDouble()) seconds.toLong().toString()
                    else String.format(Locale.US, "%.1f", seconds),
                )
            }
        }
    }

    /** 某个通道该等多久（毫秒）：单独设过用它，否则回落默认值 */
    fun routeTimeoutMs(kind: String): Int {
        val seconds = routeTimeoutTexts[kind]?.trim()?.toDoubleOrNull() ?: return probeTimeoutMs()
        return ServerProfile.clampProbeTimeoutMs((seconds * 1000).toInt())
    }

    /**
     * 落盘用的整张表（只收**单独设过**的通道）。
     *
     * <p>配对通道（tunnel / ts / funnel / tsnet）不在这里设 —— 它们读区块默认值
     * （`block_timeouts_ms`）与条目自己的值，两套 key 空间不能混
     * （{@code docs/remote-pairs.md} §5.3）。
     */
    fun routeTimeoutsMs(): Map<String, Int> {
        val out = LinkedHashMap<String, Int>()
        for (kind in listOf(ServerProfile.ROUTE_LAN, ServerProfile.ROUTE_JUMP)) {
            if (!routeTimeoutTexts[kind].isNullOrBlank()) out[kind] = routeTimeoutMs(kind)
        }
        return out
    }

    /** 连接优先级（五通道并列，有序）：lan/jump/tunnel/ts/funnel；旧 connMode 已废弃只做迁移 */
    val routeOrder = mutableStateListOf<String>().apply {
        if (profile != null) {
            // 清洗非法 id / 去重 + 缺项按默认序补尾（与 ServerProfile.routeOrderSorted 同语义）
            val seen = LinkedHashSet<String>()
            profile.routeOrder.forEach { id ->
                if (id in ROUTE_IDS_ALL) seen.add(id)
            }
            ServerProfile.defaultRouteOrder().forEach { id -> if (id !in seen) seen.add(id) }
            addAll(seen)
        } else {
            addAll(ServerProfile.defaultRouteOrder())
        }
        // 老档案迁移：没存过 routeOrder 且 connMode=remote_first → 远程在前、LAN 殿后
        if (profile != null && profile.routeOrder.isEmpty() && profile.routePriorityRemoteFirst()) {
            clear()
            addAll(listOf("tunnel", "ts", "funnel", "jump", "lan"))
        }
    }
    val routeDisabled = mutableStateListOf<String>().apply {
        if (profile != null) {
            profile.routeDisabled.forEach { id -> if (id in ROUTE_IDS_ALL && id !in this) add(id) }
        }
    }

    /** 局域网电源路由：编辑中的副本（增删都改这份），点「保存」才写回 profile */
    val routeList = mutableStateListOf<ServerProfile.PowerRoute>().apply {
        profile?.powerRoutes?.forEach { add(it.copy()) }
    }
    var routeMenuIndex by mutableStateOf(-1)
    var macMenuIndex by mutableStateOf(-1)
    var modeMenuIndex by mutableStateOf(-1)

    /** MAC 池：以「已添加的地址」为准（地址 / 网卡变了，可选项跟着变） */
    val macPool: List<String>
        get() = addressEntries.flatMap { it.macs }.distinct()

    /** 往地址列表里加一个；已存在就只并 MAC。返回是否新加了一个地址。 */
    fun upsertAddress(address: String, macs: List<String>): Boolean {
        val index = addressEntries.indexOfFirst { it.address == address }
        if (index >= 0) {
            val merged = (addressEntries[index].macs + macs).distinct()
            addressEntries[index] = addressEntries[index].copy(macs = merged)
            return false
        }
        addressEntries.add(AddrEntry(address, macs.distinct()))
        return true
    }

    /** 快速识别框 → 地址列表（粘上来的整段文字里，备注自动忽略） */
    fun recognizeQuick() {
        if (quickText.isBlank()) {
            quickHint = "先粘点东西进来，比如 192.168.0.120:12300 AA:BB:CC:DD:EE:FF"
            return
        }
        val parsed = ServerProfile.parseAddressInput(quickText, true)
        if (parsed.hosts.isEmpty()) {
            quickHint = "没认出地址 —— 要有 IP 或主机名，比如 192.168.0.120"
            return
        }
        var added = 0
        parsed.hosts.forEach { host ->
            if (upsertAddress(host, parsed.macs[host] ?: emptyList())) added++
        }
        val macCount = parsed.macs.values.sumOf { it.size }
        quickHint = "认出 ${parsed.hosts.size} 个地址、$macCount 个 MAC" +
            if (added < parsed.hosts.size) "（其余本来就有，只并了 MAC）" else ""
        // 整条 URL 粘进来时顺手把 token 收了
        val found = ServerProfile.tokenFromUrl(quickText)
        if (found.isNotEmpty() && token.isBlank()) token = found
        pastedRaw = pastedRaw + "\n" + quickText
        quickText = ""
    }

    /** 手动添加：地址 + 可选的多个 MAC */
    fun addManualAddress() {
        val address = ServerProfile.normalizeHost(newHost)
        if (address.isEmpty()) {
            quickHint = "地址不对：填 IP 或主机名，端口可带可不带"
            return
        }
        val macs = newMacs.split(Regex("[\\s,;，；]+"))
            .map { ServerProfile.normalizeMac(it) }
            .filter { it.isNotEmpty() }
            .distinct()
        val added = upsertAddress(address, macs)
        quickHint = if (added) "已添加 $address" else "地址已存在，只追了 MAC：$address"
        newHost = ""
        newMacs = ""
    }

    /** 把表单里的地址列表折成 [ServerProfile.Parsed]（宿主保存时用） */
    fun toParsed(): ServerProfile.Parsed {
        val parsed = ServerProfile.Parsed()
        for (entry in addressEntries) {
            if (entry.address.isEmpty()) continue
            parsed.hosts.add(entry.address)
            if (entry.macs.isNotEmpty()) parsed.macs[entry.address] = entry.macs.toList()
        }
        return parsed
    }

    /** token：填了就用填的，没填从原文里再挖一次（整条 URL 粘进来的情况） */
    fun resolvedToken(): String {
        val value = token.trim()
        return value.ifEmpty { ServerProfile.tokenFromUrl(pastedRaw) }
    }

    /** 跳板机 id；空串 = 直连 */
    fun resolvedJumpId(): String =
        if (jumpIndex <= 0) "" else jumps[jumpIndex - 1].id

    fun jumpLabel(jump: JumpServer): String {
        val label = if (JumpStore.text(jump.name).isEmpty()) jump.host else jump.name
        return "经由：$label" + (if (jump.isSsh()) "" else "（仅 Telnet，不能做隧道）")
    }
}

@Composable
internal fun rememberServerEditFormState(profile: ServerProfile?): ServerEditFormState {
    val context = androidx.compose.ui.platform.LocalContext.current
    // key 用 profile 的 id：同一台电脑重复打开不会残留上一次改动
    return remember(profile?.id, profile) { ServerEditFormState(profile, context) }
}

/**
 * 表单主体。宿主把自己选的外壳（AlertDialog / ModalBottomSheet）套在它外面。
 *
 * 内部自己带纵向滚动 —— 两个宿主的可显示高度差别很大，交给这里统一更省心。
 */
@Composable
internal fun ServerEditFormBody(state: ServerEditFormState) {
    val palette = LocalPalette.current
    val context = androidx.compose.ui.platform.LocalContext.current
    // 扫码读配对码：独立 Activity（CAMERA 运行时权限 + 相机生命周期都在那边），
    // 成功回传配对串 → 填进远程框并即时识别；取消/失败不动粘贴框
    val scanLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        val text = result.data?.getStringExtra(ScanPairActivity.EXTRA_TEXT).orEmpty().trim()
        if (text.isNotEmpty()) {
            state.remoteText = text
            state.applyRemoteHint()
        }
    }
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState())) {
        FieldLabel("电脑名称（同一台电脑的不同网卡/地址，用名字区分）")
        OutlinedTextField(
            value = state.name,
            onValueChange = { state.name = it },
            singleLine = true,
            placeholder = { Text("例如：有线网卡 / 5G WiFi（可留空，用地址兜底）", fontSize = 12.sp) },
            modifier = Modifier.fillMaxWidth(),
        )

        FieldLabel("快速识别（整段粘进来，自动拆出地址 / 端口 / MAC 加进下面）")
        OutlinedTextField(
            value = state.quickText,
            onValueChange = { state.quickText = it },
            placeholder = {
                Text(
                    "192.168.0.120:12300 AA:BB:CC:DD:EE:FF 台式机有线\n" +
                        "192.168.0.121 11-22-33-44-55-66（5G WiFi）",
                    fontSize = 12.sp,
                )
            },
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
            minLines = 2,
            maxLines = 4,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = { state.recognizeQuick() }) {
                Text("识别并加入", fontSize = 12.sp)
            }
            Text(
                state.quickHint.ifEmpty { "换行 / 空格 / 逗号都能分开；认不出的备注自动忽略" },
                fontSize = 11.sp, color = palette.fgFaint,
                modifier = Modifier.weight(1f),
            )
        }

        FieldLabel("已添加的地址（第一个用来连接，其余参与测试连接和唤醒）")
        if (state.addressEntries.isEmpty()) {
            Text(
                "还没加地址 —— 上面粘一段，或者下面手动填一个。",
                fontSize = 11.sp, color = palette.fgFaint,
            )
        }
        state.addressEntries.forEachIndexed { index, entry ->
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 6.dp)
                    .clip(RoundedCornerShape(10.dp))
                    .background(palette.card)
                    .padding(8.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        entry.address,
                        fontSize = 12.sp, color = palette.fg,
                        modifier = Modifier.weight(1f),
                    )
                    if (index == 0) {
                        Text("连接用", fontSize = 10.sp, color = palette.accent)
                    }
                    TextButton(onClick = { state.addressEntries.removeAt(index) }) {
                        Text("✕", fontSize = 12.sp, color = palette.bad)
                    }
                }
                if (entry.macs.isEmpty()) {
                    Text(
                        "没配 MAC —— 局域网电源发不出去",
                        fontSize = 10.sp, color = palette.fgFaint,
                    )
                } else {
                    entry.macs.forEach { mac ->
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                mac,
                                fontSize = 10.sp, color = palette.fgDim,
                                modifier = Modifier.weight(1f),
                            )
                            TextButton(onClick = {
                                state.addressEntries[index] =
                                    entry.copy(macs = entry.macs.filter { it != mac })
                            }) {
                                Text("✕", fontSize = 11.sp, color = palette.fgFaint)
                            }
                        }
                    }
                }
            }
        }

        FieldLabel("手动添加（地址 + 多个 MAC；地址已存在时只追加 MAC）")
        OutlinedTextField(
            value = state.newHost,
            onValueChange = { state.newHost = it },
            label = { Text("地址（192.168.0.120 或 192.168.0.120:12300）", fontSize = 10.sp) },
            singleLine = true,
            textStyle = MaterialTheme.typography.bodySmall,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
            modifier = Modifier.fillMaxWidth(),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(
                value = state.newMacs,
                onValueChange = { state.newMacs = it },
                label = { Text("MAC（可多个，空格隔开）", fontSize = 10.sp) },
                placeholder = { Text("AA:BB:CC:DD:EE:FF", fontSize = 12.sp) },
                singleLine = true,
                textStyle = MaterialTheme.typography.bodySmall,
                modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(8.dp))
            TextButton(onClick = { state.addManualAddress() }) {
                Text("添加", fontSize = 13.sp)
            }
        }

        FieldLabel("访问令牌 token（服务端设了才要填）")
        if (state.savedToken.isNotEmpty()) {
            // 已存令牌回显：默认打码，小眼睛切换明文（重开表单回到打码）
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(10.dp))
                    .background(palette.card)
                    .padding(horizontal = 8.dp, vertical = 4.dp),
            ) {
                Text(
                    if (state.tokenShown) state.savedToken else state.maskToken(state.savedToken),
                    fontSize = 12.sp,
                    fontFamily = FontFamily.Monospace,
                    color = if (state.tokenShown) palette.fg else palette.fgDim,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = { state.tokenShown = !state.tokenShown }) {
                    Text(if (state.tokenShown) "隐藏" else "显示", fontSize = 11.sp)
                }
            }
        }
        OutlinedTextField(
            value = state.token,
            onValueChange = { state.token = it },
            singleLine = true,
            placeholder = { Text("留空保持不变；要替换就填新令牌", fontSize = 12.sp) },
            modifier = Modifier.fillMaxWidth(),
        )

        FieldLabel("远程配对（三个区块，每个区块可以挂多条；同类型配一条不会顶掉另一条）")
        OutlinedTextField(
            value = state.remoteText,
            onValueChange = {
                state.remoteText = it
                state.applyRemoteHint()
            },
            placeholder = {
                Text("anyboot://pair?v=2&… （配对码）；也可直接粘 https://xxx.trycloudflare.com / http://100.x.y.z:12300", fontSize = 12.sp)
            },
            modifier = Modifier.fillMaxWidth(),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = {
                scanLauncher.launch(Intent(context, ScanPairActivity::class.java))
            }) {
                Text("📷 扫码读码", fontSize = 12.sp)
            }
            TextButton(onClick = { state.clearPairs() }) {
                Text(
                    if (state.hasPairs()) "全部清空" else "清空",
                    fontSize = 12.sp, color = palette.fgDim,
                )
            }
            Text(
                if (state.remoteHint.isNotEmpty()) state.remoteHint
                else "扫码或粘贴后自动加入对应区块；同一条会更新而不是重复",
                fontSize = 11.sp, color = palette.fgFaint,
                modifier = Modifier.weight(1f),
            )
        }
        // 三个区块逐块列出：区块标题 + 条数 + 「添加」，行里是地址 / lane / 来源 / 令牌 / 超时
        for (block in RemotePair.BLOCK_IDS) {
            val items = state.pairsOf(block)
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 8.dp)
                    .clip(RoundedCornerShape(11.dp))
                    .background(palette.card)
                    .padding(horizontal = 9.dp, vertical = 7.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        RemotePair.blockLabel(block),
                        fontSize = 12.sp,
                        fontWeight = FontWeight.SemiBold,
                        color = palette.fg,
                        modifier = Modifier.weight(1f),
                    )
                    Text(
                        if (items.isEmpty()) "未配" else items.size.toString() + " 条",
                        fontSize = 10.sp, color = palette.fgFaint,
                    )
                    TextButton(onClick = { state.addPair(block) }) {
                        Text("＋ 添加", fontSize = 11.sp)
                    }
                }
                if (items.isEmpty()) {
                    Text(
                        "这个区块还没有配对 —— 上面粘一条配对码，或点「＋ 添加」手填",
                        fontSize = 10.sp, color = palette.fgFaint,
                    )
                }
                items.forEach { item ->
                    PairEditRow(
                        state = state,
                        item = item,
                        palette = palette,
                        onCopy = { value -> context.copyRemoteSlot("配对地址", value) },
                    )
                }
            }
        }
        Text(
            "保存后每个区块里的配对各自独立探测、各自计时；清空全部保存 = 解除配对。" +
                "配对码等同密码，别转发给别人。",
            fontSize = 11.sp, color = palette.fgFaint,
            modifier = Modifier.padding(top = 6.dp),
        )

        FoldHeader(
            title = "连接策略",
            summary = "优先级 " + state.routeOrder.count { state.routeAvailable(it) && it !in state.routeDisabled } + "/" + ROUTE_IDS_ALL.size +
                " · 通道间 " + probeLabel(state.routeProbeMode) +
                " · 多网卡 " + probeLabel(state.probeMode) +
                " · 默认 " + String.format(Locale.US, "%.1f", state.probeTimeoutMs() / 1000.0) + "s" +
                (state.routeTimeoutsMs().size.let { if (it > 0) "（$it 个通道单独设）" else "" }) +
                (if (state.jumpIndex > 0) " · 跳板机" else ""),
            open = state.connFoldOpen,
            onToggle = { state.connFoldOpen = !state.connFoldOpen },
        )
        if (state.connFoldOpen) {
        FieldLabel("连接优先级（↑↓ 调顺序；行尾开关启用 / 停用；名称下的小框 = 这个通道自己的探测超时/秒，留空用下面的默认值）")
        state.routeOrder.forEachIndexed { index, kind ->
            val available = state.routeAvailable(kind)
            val enabled = available && kind !in state.routeDisabled
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(vertical = 2.dp),
            ) {
                Text(
                    "☰",
                    fontSize = 13.sp,
                    color = palette.fgFaint,
                    modifier = Modifier.padding(end = 8.dp),
                )
                Column(Modifier.weight(1f)) {
                    Text(
                        ROUTE_LABELS_ZH[kind] ?: kind,
                        fontSize = 13.sp,
                        fontWeight = FontWeight.Medium,
                        color = if (available) palette.fg else palette.fgFaint,
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            state.routeSub(kind) + if (available) "" else "（不可用）",
                            fontSize = 10.sp,
                            color = palette.fgFaint,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        Spacer(Modifier.width(6.dp))
                        if (kind == ServerProfile.ROUTE_LAN || kind == ServerProfile.ROUTE_JUMP) {
                            InlineTimeoutField(
                                label = "超时",
                                value = state.routeTimeoutTexts[kind] ?: "",
                                enabled = available,
                                onValueChange = { raw ->
                                    val cleaned = raw.filter { it.isDigit() || it == '.' }.take(5)
                                    if (cleaned.isBlank()) state.routeTimeoutTexts.remove(kind)
                                    else state.routeTimeoutTexts[kind] = cleaned
                                },
                            )
                        } else {
                            // 配对通道的超时在下面「区块默认超时」与每条配对上设
                            Text(
                                "超时见区块设置",
                                fontSize = 10.sp,
                                color = palette.fgFaint,
                                modifier = Modifier.padding(start = 2.dp),
                            )
                        }
                    }
                }
                TinyTextButton(
                    text = "↑",
                    tint = palette.fgDim,
                    enabled = index > 0,
                    onClick = { state.routeMove(kind, -1) },
                )
                TinyTextButton(
                    text = "↓",
                    tint = palette.fgDim,
                    enabled = index < state.routeOrder.size - 1,
                    onClick = { state.routeMove(kind, 1) },
                )
                Switch(
                    checked = enabled,
                    onCheckedChange = { on ->
                        if (on) state.routeDisabled.remove(kind) else if (kind !in state.routeDisabled) {
                            state.routeDisabled.add(kind)
                        }
                    },
                    enabled = available,
                )
            }
        }
        Text(
            "探活与接口请求按此顺序逐通道试，第一个通的就走谁；每个通道可以单独设探测超时" +
                "（局域网 1~2 秒够，走公网中继的 Funnel / 内置 Funnel 首次握手慢，可以单独给 5~8 秒）。" +
                "跳板机需先在下方选好档案；电源唤醒不走此排序 —— 魔术包只能在局域网内广播。",
            fontSize = 10.sp, color = palette.fgFaint,
            modifier = Modifier.padding(top = 4.dp, bottom = 2.dp),
        )

        // ---- 两个粒度的探测方式：先通道之间，再同一通道内的多个地址 ----
        FieldLabel("连接优先级的探测方式（六个通道之间怎么试，对上面每一个通道都生效）")
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
            ProbeModeChoice(
                "并发·首个", state.routeProbeMode == ServerProfile.PROBE_PARALLEL_FIRST,
                Modifier.weight(1f),
            ) { state.routeProbeMode = ServerProfile.PROBE_PARALLEL_FIRST }
            ProbeModeChoice(
                "并发·全部", state.routeProbeMode == ServerProfile.PROBE_PARALLEL_ALL,
                Modifier.weight(1f),
            ) { state.routeProbeMode = ServerProfile.PROBE_PARALLEL_ALL }
            ProbeModeChoice(
                "串行", state.routeProbeMode == ServerProfile.PROBE_SERIAL,
                Modifier.weight(1f),
            ) { state.routeProbeMode = ServerProfile.PROBE_SERIAL }
        }
        Text(
            "并发·首个：全部通道同时发出，谁先成功用谁；并发·全部：等全部返回，逐行报每个通道的成败与耗时；" +
                "串行：按上面的优先级逐个通道试，第一个通就停。",
            fontSize = 10.sp, color = palette.fgFaint,
            modifier = Modifier.padding(top = 4.dp, bottom = 8.dp),
        )

        FieldLabel("多网卡探测（同一通道里的多个地址之间怎么探）")
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
            ProbeModeChoice(
                "并发·首个", state.probeMode == ServerProfile.PROBE_PARALLEL_FIRST,
                Modifier.weight(1f),
            ) { state.probeMode = ServerProfile.PROBE_PARALLEL_FIRST }
            ProbeModeChoice(
                "并发·全部", state.probeMode == ServerProfile.PROBE_PARALLEL_ALL,
                Modifier.weight(1f),
            ) { state.probeMode = ServerProfile.PROBE_PARALLEL_ALL }
            ProbeModeChoice(
                "串行", state.probeMode == ServerProfile.PROBE_SERIAL,
                Modifier.weight(1f),
            ) { state.probeMode = ServerProfile.PROBE_SERIAL }
        }
        Text(
            "一台电脑配了多个局域网地址（多块网卡 / DHCP 换过 IP）时生效：并发谁先通用谁，串行按地址顺序试。",
            fontSize = 10.sp, color = palette.fgFaint,
            modifier = Modifier.padding(top = 4.dp, bottom = 8.dp),
        )

        // ---- 区块内部策略 + 区块默认超时（配对专用，与上面两个粒度并列的第三层） ----
        // 一行塞不下六个元素（354dp 视口下超时框被挤得只剩壳）：拆两行，
        // 第一行「区块名 …… 默认超时[__]秒」，第二行三个策略按钮均分占满 —— 与 PC 端同构。
        FieldLabel("区块内部策略（同一个区块里的多条配对之间怎么探）与区块默认超时")
        for (block in RemotePair.BLOCK_IDS) {
            val count = state.pairsOf(block).count { it.addr.isNotBlank() }
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(vertical = 2.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        RemotePair.blockLabel(block),
                        fontSize = 12.sp,
                        color = if (count > 0) palette.fg else palette.fgFaint,
                        modifier = Modifier.weight(1f),
                    )
                    InlineTimeoutField(
                        label = "默认超时",
                        value = state.blockTimeoutTexts[block] ?: "",
                        enabled = count > 0,
                        onValueChange = { raw ->
                            val cleaned = raw.filter { it.isDigit() || it == '.' }.take(5)
                            if (cleaned.isBlank()) state.blockTimeoutTexts.remove(block)
                            else state.blockTimeoutTexts[block] = cleaned
                        },
                    )
                }
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 2.dp),
                ) {
                    val mode = state.blockModes[block] ?: ServerProfile.PROBE_PARALLEL_FIRST
                    ProbeModeChoice("并发·首个", mode == ServerProfile.PROBE_PARALLEL_FIRST, Modifier.weight(1f)) {
                        state.blockModes[block] = ServerProfile.PROBE_PARALLEL_FIRST
                    }
                    ProbeModeChoice("全部", mode == ServerProfile.PROBE_PARALLEL_ALL, Modifier.weight(1f)) {
                        state.blockModes[block] = ServerProfile.PROBE_PARALLEL_ALL
                    }
                    ProbeModeChoice("串行", mode == ServerProfile.PROBE_SERIAL, Modifier.weight(1f)) {
                        state.blockModes[block] = ServerProfile.PROBE_SERIAL
                    }
                }
            }
        }
        Text(
            "并发·首个：同区块全部同时发出，先成功的不一定是最前面那条；串行：严格按区块内的行序试。" +
                "每条配对还能在它自己那行再设一个超时（条目 → 区块 → 全局默认）。",
            fontSize = 10.sp, color = palette.fgFaint,
            modifier = Modifier.padding(top = 4.dp, bottom = 8.dp),
        )

        FieldLabel(
            "经由跳板机（外网连家里：先到「☰ → 服务器」添加 SSH 跳板机；"
                + "填的地址就变成「跳板机视角的内网地址」）"
        )
        Box {
            OutlinedTextField(
                value = if (state.jumpIndex <= 0) "直连（默认）"
                else state.jumpLabel(state.jumps[state.jumpIndex - 1]),
                onValueChange = {},
                readOnly = true,
                singleLine = true,
                // ⚠️ 必须 disabled：readOnly 的 TextField 会把点击吃成「聚焦」（边框绿一下），
                // 挂在它身上的 clickable 一次都不触发 —— 下拉菜单永远打不开。
                // disabled 的 TextField 不处理指针，clickable 才收得到；配色见 disabledFieldColors。
                enabled = false,
                colors = disabledFieldColors(palette.fg, palette.lineStrong, palette.fgDim),
                trailingIcon = { Text(if (state.jumpOpen) "▴" else "▾") },
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { state.jumpOpen = true },
            )
            DropdownMenu(expanded = state.jumpOpen, onDismissRequest = { state.jumpOpen = false }) {
                DropdownMenuItem(text = { Text("直连（默认）") }, onClick = {
                    state.jumpIndex = 0; state.jumpOpen = false
                })
                state.jumps.forEachIndexed { index, jump ->
                    DropdownMenuItem(
                        text = { Text(state.jumpLabel(jump)) },
                        onClick = { state.jumpIndex = index + 1; state.jumpOpen = false },
                    )
                }
            }
        }

        FieldLabel(
            "探测方式（连接优先级整体的策略，对上面每一个通道都生效）"
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            RadioButton(
                selected = state.probeMode == ServerProfile.PROBE_PARALLEL_FIRST,
                onClick = { state.probeMode = ServerProfile.PROBE_PARALLEL_FIRST }
            )
            Text(
                "并发·首个",
                fontSize = 13.sp, color = palette.fg,
                modifier = Modifier.weight(1f, fill = false),
            )
            Spacer(Modifier.width(10.dp))
            RadioButton(
                selected = state.probeMode == ServerProfile.PROBE_PARALLEL_ALL,
                onClick = { state.probeMode = ServerProfile.PROBE_PARALLEL_ALL }
            )
            Text(
                "并发·全部",
                fontSize = 13.sp, color = palette.fg,
                modifier = Modifier.weight(1f, fill = false),
            )
            Spacer(Modifier.width(10.dp))
            RadioButton(
                selected = state.probeMode == ServerProfile.PROBE_SERIAL,
                onClick = { state.probeMode = ServerProfile.PROBE_SERIAL }
            )
            Text("串行", fontSize = 13.sp, color = palette.fg)
        }
        Text(
            "并发·首个：每地址一个线程，任一响应立即用它；并发·全部：等全部探完，逐行报成败；串行：按顺序试，第一个通就停",
            fontSize = 10.sp, color = palette.fgFaint,
            modifier = Modifier.padding(top = 4.dp, bottom = 8.dp),
        )

        FieldLabel("默认探测超时（秒）—— 通道那格留空时用它")
        Row(verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(
                value = state.probeTimeoutText,
                onValueChange = { value ->
                    // 只收数字与一个小数点（0.2 ~ 30 秒；留空 = 默认 2 秒）
                    var dot = false
                    val filtered = buildString {
                        for (ch in value) {
                            if (ch.isDigit()) append(ch)
                            else if ((ch == '.' || ch == '。') && !dot) {
                                append('.')
                                dot = true
                            }
                        }
                    }
                    state.probeTimeoutText = filtered.take(5)
                },
                label = { Text("秒", fontSize = 10.sp) },
                placeholder = { Text("2", fontSize = 12.sp) },
                singleLine = true,
                textStyle = MaterialTheme.typography.bodySmall,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
                modifier = Modifier.width(110.dp),
            )
            Spacer(Modifier.width(10.dp))
            Text(
                "通道那格填了就以它自己为准；没填的通道才用这个默认值（上限 30 秒）",
                fontSize = 10.sp, color = palette.fgFaint,
                modifier = Modifier.weight(1f),
            )
        }
        } // end if (state.connFoldOpen)

        FoldHeader(
            title = "局域网电源路由",
            summary = if (state.routeList.isEmpty()) "未配置" else "${state.routeList.size} 条",
            open = state.powerFoldOpen,
            onToggle = { state.powerFoldOpen = !state.powerFoldOpen },
        )
        if (state.powerFoldOpen) {
        FieldLabel(
            "局域网电源路由（可选：每个动作发到哪个地址、哪个端口。" +
                "目标机装 sol 后按端口执行 —— 不装 sol 就只有唤醒能靠 WOL 生效）"
        )
        if (state.routeList.isEmpty()) {
            Text(
                "还没配：电源按钮仍走老路 —— 唤醒发标准 WOL，重启/关机/睡眠调电脑端接口。",
                fontSize = 11.sp, color = palette.fgFaint,
            )
        }
        state.routeList.forEachIndexed { index, route ->
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 6.dp)
                    .clip(RoundedCornerShape(10.dp))
                    .background(palette.card)
                    .padding(8.dp),
            ) {
                Text(
                    "路由 ${index + 1} / ${state.routeList.size}",
                    fontSize = 10.sp, color = palette.fgFaint,
                    modifier = Modifier.padding(bottom = 2.dp),
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box {
                        OutlinedButton(
                            onClick = {
                                state.routeMenuIndex =
                                    if (state.routeMenuIndex == index) -1 else index
                            },
                            contentPadding = PaddingValues(horizontal = 10.dp, vertical = 2.dp),
                        ) {
                            Text(ServerProfile.powerActionLabel(route.action), fontSize = 12.sp)
                        }
                        DropdownMenu(
                            expanded = state.routeMenuIndex == index,
                            onDismissRequest = { state.routeMenuIndex = -1 },
                        ) {
                            listOf(
                                ServerProfile.POWER_WAKE,
                                ServerProfile.POWER_REBOOT,
                                ServerProfile.POWER_SHUTDOWN,
                                ServerProfile.POWER_SLEEP,
                            ).forEach { action ->
                                DropdownMenuItem(
                                    text = { Text(ServerProfile.powerActionLabel(action)) },
                                    onClick = {
                                        val before = route.actionOrWake()
                                        val oldBurst = if (before == ServerProfile.POWER_WAKE) {
                                            ServerProfile.DEFAULT_WAKE_BURST
                                        } else {
                                            ServerProfile.DEFAULT_POWER_BURST
                                        }
                                        val oldRetry = if (before == ServerProfile.POWER_WAKE) {
                                            ServerProfile.DEFAULT_WAKE_RETRY
                                        } else {
                                            ServerProfile.DEFAULT_POWER_RETRY
                                        }
                                        // 没手动改过（还是旧动作的默认值）就跟着换成新动作的默认值；
                                        // 0 = 「按动作默认」，交给 ServerProfile 算
                                        val burst =
                                            if (route.burst == 0 || route.burst == oldBurst) 0 else route.burst
                                        val retry =
                                            if (route.retry == 0 || route.retry == oldRetry) 0 else route.retry
                                        state.routeList[index] = route
                                            .with(action, route.target, route.port, route.macs, route.enabled)
                                            .withCounts(burst, retry)
                                        state.routeMenuIndex = -1
                                    },
                                )
                            }
                        }
                    }
                    Spacer(Modifier.weight(1f))
                    TextButton(onClick = {
                        state.routeList.removeAt(index)
                        state.routeMenuIndex = -1
                        state.modeMenuIndex = -1
                        state.macMenuIndex = -1
                    }) {
                        Text("删除", fontSize = 12.sp, color = palette.bad)
                    }
                }

                // 发送模式（共同设置区：动作后面、端口前面）。广播包 = 局域网
                // sol / WOL 魔术包；HTTP = 这台电脑自带的 AnyBoot 接口；混合 = 两者同时发。
                Box {
                    OutlinedButton(
                        onClick = {
                            state.modeMenuIndex = if (state.modeMenuIndex == index) -1 else index
                        },
                        contentPadding = PaddingValues(horizontal = 10.dp, vertical = 2.dp),
                    ) {
                        Text(
                            "模式：" + ServerProfile.modeLabel(route.effectiveMode()),
                            fontSize = 12.sp,
                        )
                    }
                    DropdownMenu(
                        expanded = state.modeMenuIndex == index,
                        onDismissRequest = { state.modeMenuIndex = -1 },
                    ) {
                        listOf("auto", "http_first", "http_only", "packet_only", "hybrid").forEach { value ->
                            DropdownMenuItem(
                                text = { Text(ServerProfile.modeLabel(value)) },
                                onClick = {
                                    state.routeList[index] = route.withMode(value)
                                    state.modeMenuIndex = -1
                                },
                            )
                        }
                    }
                }

                // —— 广播包设置区：端口 / 地址 / MAC / 发包数 / 重试 ——
                Text(
                    "广播包",
                    fontSize = 10.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.fgDim,
                    modifier = Modifier.padding(top = 8.dp),
                )
                OutlinedTextField(
                    value = if (route.port <= 0) "" else route.port.toString(),
                    onValueChange = { value ->
                        val digits = value.filter { it.isDigit() }.take(5)
                        state.routeList[index] = route.with(
                            route.action, route.target, digits.toIntOrNull() ?: 0,
                            route.macs, route.enabled,
                        )
                    },
                    label = { Text("端口", fontSize = 10.sp) },
                    singleLine = true,
                    textStyle = MaterialTheme.typography.bodySmall,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                    modifier = Modifier.width(92.dp),
                )
                OutlinedTextField(
                    value = route.target,
                    onValueChange = { value ->
                        state.routeList[index] = route.with(
                            route.action, value, route.port, route.macs, route.enabled,
                        )
                    },
                    label = { Text("广播地址（留空 = 自动：全局 + 各地址的 /24）", fontSize = 10.sp) },
                    placeholder = { Text("192.168.0.255", fontSize = 12.sp) },
                    singleLine = true,
                    textStyle = MaterialTheme.typography.bodySmall,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 6.dp),
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        "MAC：" + if (route.macs.isEmpty()) {
                            "全部（${state.macPool.size} 块）"
                        } else {
                            "${route.macs.size} / ${state.macPool.size} 块"
                        },
                        fontSize = 11.sp, color = palette.fgFaint,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(
                        onClick = {
                            state.macMenuIndex = if (state.macMenuIndex == index) -1 else index
                        },
                        enabled = state.macPool.isNotEmpty(),
                    ) {
                        Text(if (state.macPool.isEmpty()) "没配 MAC" else "选择", fontSize = 12.sp)
                    }
                }
                if (state.macMenuIndex == index && state.macPool.isNotEmpty()) {
                    // 空列表 = 全部网卡（以后新加的 MAC 也自动包含）
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier
                            .fillMaxWidth()
                            .clickable {
                                state.routeList[index] = route.with(
                                    route.action, route.target, route.port, emptyList(), route.enabled,
                                )
                            },
                    ) {
                        Checkbox(checked = route.macs.isEmpty(), onCheckedChange = null)
                        Text("全部网卡", fontSize = 11.sp, color = palette.fgDim)
                    }
                    if (route.macs.isNotEmpty()) {
                        state.macPool.forEach { mac ->
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .clickable {
                                        val next = if (route.macs.contains(mac)) {
                                            route.macs - mac
                                        } else {
                                            route.macs + mac
                                        }
                                        state.routeList[index] = route.with(
                                            route.action, route.target, route.port, next, route.enabled,
                                        )
                                    },
                            ) {
                                Checkbox(checked = route.macs.contains(mac), onCheckedChange = null)
                                Text(mac, fontSize = 11.sp, color = palette.fgDim)
                            }
                        }
                    }
                }

                // 发包数 / 重试：每个动作都能设（默认唤醒 10 / 其余 1）。
                // 注意 value 只认「用户自己填的数」，空着就用占位符显示默认值 ——
                // 千万不能把 effective 值当 value 回填，否则删一个字符就被弹回来，
                // 光标一直跳（看着就是输入框在闪）。
                Row(verticalAlignment = Alignment.CenterVertically) {
                    val waking = route.actionOrWake() == ServerProfile.POWER_WAKE
                    OutlinedTextField(
                        value = if (route.burst > 0) route.burst.toString() else "",
                        onValueChange = { value ->
                            val number = value.filter { it.isDigit() }.take(2).toIntOrNull() ?: 0
                            state.routeList[index] = route.withCounts(number, route.retry)
                        },
                        label = { Text(if (waking) "每块发几个包" else "发几个包", fontSize = 10.sp) },
                        placeholder = { Text(route.defaultBurst().toString(), fontSize = 11.sp) },
                        singleLine = true,
                        textStyle = MaterialTheme.typography.bodySmall,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        // 用 weight 按老比例分宽度（120:104）：窄窗（分屏/小窗）自动收缩，不横向溢出
                        modifier = Modifier.weight(120f),
                    )
                    Spacer(Modifier.width(8.dp))
                    OutlinedTextField(
                        value = if (route.retry > 0) route.retry.toString() else "",
                        onValueChange = { value ->
                            val number = value.filter { it.isDigit() }.take(2).toIntOrNull() ?: 0
                            state.routeList[index] = route.withCounts(route.burst, number)
                        },
                        label = { Text("重试次数", fontSize = 10.sp) },
                        placeholder = { Text(route.defaultRetry().toString(), fontSize = 11.sp) },
                        singleLine = true,
                        textStyle = MaterialTheme.typography.bodySmall,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        modifier = Modifier.weight(104f),
                    )
                }
                Text(
                    "重试 = 没发出去就隔 0.2 秒再打一轮；走跳板机时每轮重试都会重新连接跳板机，直到次数用完。",
                    fontSize = 9.5.sp,
                    color = palette.fgFaint,
                    modifier = Modifier.padding(top = 3.dp),
                )

                // —— HTTP 设置区：无可配置项，仅说明（HTTP 走这台电脑
                // 自带的接口，token 与连接共用）；混合模式两条通道同时发。
                Text(
                    "HTTP",
                    fontSize = 10.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.fgDim,
                    modifier = Modifier.padding(top = 8.dp),
                )
                Text(
                    "调这台电脑自带的 /api/power 接口（token 与连接共用，无需单独配置）。" +
                        "混合模式 = HTTP 与广播包同时发。",
                    fontSize = 10.sp, color = palette.fgFaint,
                )
            }
        }
        TextButton(onClick = {
            // 发送模式默认「优先广播包」（= 旧的 HTTP 兜底勾选）
            state.routeList.add(
                ServerProfile.PowerRoute(ServerProfile.POWER_WAKE, "", 9)
                    .withMode("auto")
            )
        }) { Text("+ 添加一条路由", fontSize = 13.sp) }
        } // end if (state.powerFoldOpen)

        Spacer(Modifier.height(4.dp))
    }
}

/**
 * 把表单状态写回一条 [ServerProfile]（不含落盘，落盘交给宿主）。
 *
 * [replaceAddresses] 决定地址怎么写：
 *  · true  = 以表单为准整体替换（编辑已存在的档案，方便删掉过期地址、调整顺序）；
 *  · false = 并入（新增时按分组落到同一台电脑，多块网卡合并成一条）。
 *
 * 返回 false = 校验没过（调用方保持表单打开）。
 */
internal fun applyServerEdit(
    state: ServerEditFormState,
    target: ServerProfile,
    replaceAddresses: Boolean,
): Boolean {
    val parsed = state.toParsed()
    if (parsed.isEmpty()) return false
    if (replaceAddresses) {
        target.replaceParsed(parsed)
    } else {
        target.mergeParsed(parsed)
    }
    target.name = state.name.trim().ifEmpty { target.host() }
    target.token = state.resolvedToken()
    target.probeMode = ServerProfile.normalizeProbeMode(state.probeMode)
    // 连接优先级的探测方式（六个通道之间）—— 与上面的多网卡探测是两个粒度
    target.routeProbeMode = ServerProfile.normalizeProbeMode(state.routeProbeMode)
    // 默认探测超时（界面秒 → 落盘毫秒，空/非法回落默认 2 秒）
    target.probeTimeoutMs = state.probeTimeoutMs()
    // 每通道自己的超时（只写单独设过的；空表 = 全部继承默认值）
    target.routeTimeoutsMs = LinkedHashMap(state.routeTimeoutsMs())
    // 五通道优先级；旧 connMode 已废弃恒空（后端/两端按 routeOrder 消费）
    target.connMode = ""
    target.routeOrder.clear()
    target.routeOrder.addAll(state.routeOrder)
    target.routeDisabled.clear()
    target.routeDisabled.addAll(state.routeDisabled)
    target.jumpServerId = state.resolvedJumpId()
    // 远程配对：以表单里的这份为准（空 = 解除配对，地址与令牌一起清）。
    // 令牌保活在这里不需要单独做 —— 表单回填时就是原令牌，用户没改就原样写回去。
    val pairs = state.pairsForSave()
    target.pairs.clear()
    target.pairs.addAll(pairs)
    target.blockProbeMode.clear()
    for (block in RemotePair.BLOCK_IDS) {
        val mode = ServerProfile.normalizeProbeMode(
            state.blockModes[block] ?: ServerProfile.PROBE_PARALLEL_FIRST,
        )
        // 默认值不落盘（等号默认写出去只会让配置文件越读越乱）
        if (mode != ServerProfile.PROBE_PARALLEL_FIRST) {
            target.blockProbeMode[block] = mode
        }
    }
    target.blockTimeoutsMs.clear()
    for (block in RemotePair.BLOCK_IDS) {
        val ms = state.blockTimeoutMsFor(block)
        if (ms > 0) {
            target.blockTimeoutsMs[block] = ms
        }
    }
    // 局域网电源路由：以表单里的这份为准（空 = 没配，走老路）
    target.powerRoutes.clear()
    for (route in state.routeList) {
        target.powerRoutes.add(route.copy())
    }
    return true
}

/**
 * 电脑档案保存的**唯一实现**（MainActivity / ComputerListActivity 两个入口共用：
 * 一个弹窗、一份逻辑、一个存储 [ServerStore]）。
 *
 * 地址写入语义：编辑 = 以表单为准整体替换；新增 = 按分组并入同一台电脑
 * （同一台机多块网卡合成一条，不新建标签）。落盘前统一排序。
 *
 * 返回 null = 校验没过（地址为空），调用方保持弹窗打开；
 * 否则带 [mergedExisting]（新增时并进了已有档案，宿主可提示「已并入」）。
 * 保存后的页面动作（重连 / 重探 / 刷新列表）由各宿主自己做 —— 那是页面状态，不属于存储。
 */
internal fun saveServerProfile(
    context: Context,
    servers: MutableList<ServerProfile>,
    editing: ServerProfile?,
    form: ServerEditFormState,
): ServerSaveResult? {
    val parsed = form.toParsed()
    if (parsed.isEmpty()) return null
    var mergedExisting = false
    val target: ServerProfile
    if (editing != null) {
        // 编辑：按 **id** 从当前共享列表里取对象，别直接用传进来的引用。
        // 列表被重新载入过（另一个页面保存 / 删除后 reload）时，传进来的可能是
        // 已经不在列表里的旧对象 —— 改它等于改了个孤儿，落盘时被无视，用户看到
        // 的就是「保存了但没生效」。
        target = servers.firstOrNull { it.id == editing.id } ?: editing
    } else {
        // 新增：没有分组了，所以**不再自动并入**已有档案 —— 用户想合并就在「地址」里
        // 把同一台机器的多个地址都填上；跨档案合并交给「是不是同一台电脑」的机器标识。
        val fresh = ServerProfile()
        servers.add(fresh)
        target = fresh
    }
    if (!applyServerEdit(form, target, replaceAddresses = editing != null)) return null
    ServerRepository.persist(context, servers)
    return ServerSaveResult(target, mergedExisting)
}

internal class ServerSaveResult(val target: ServerProfile, val mergedExisting: Boolean)

/** 表单底部提示用的一行说明（两个宿主共用同一句） */

/** 探测模式的短标签（折叠头摘要用） */
private fun probeLabel(mode: String): String = when (mode) {
    ServerProfile.PROBE_PARALLEL_ALL -> "并发全部"
    ServerProfile.PROBE_SERIAL -> "串行"
    else -> "并发首个"
}

/** 折叠区头：标题 + 右侧状态摘要，点击展开/收起（高级项默认收起） */
@Composable
private fun FoldHeader(title: String, summary: String, open: Boolean, onToggle: () -> Unit) {
    val palette = LocalPalette.current
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 8.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(onClick = onToggle)
            .padding(vertical = 6.dp, horizontal = 2.dp),
    ) {
        Text(if (open) "▾" else "▸", fontSize = 10.sp, color = palette.fgFaint)
        Text(
            title,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            color = palette.fgDim,
            modifier = Modifier.padding(start = 6.dp),
        )
        Spacer(Modifier.weight(1f))
        Text(summary, fontSize = 10.5.sp, color = palette.fgFaint)
    }
}

/**
 * 一行配对：启用开关 · 地址（可编辑，mono）· lane · 来源 · 条目超时 · 令牌 · ↑↓ ✕。
 *
 * 和 PC 端 `rsPairRow` 同一套信息：地址、lane 徽标、来源（host · os）、条目自己的超时。
 * 令牌打码显示，点「显示」看明文（配对码等同密码）。
 */
@Composable
private fun PairEditRow(
    state: ServerEditFormState,
    item: PairEdit,
    palette: Palette,
    onCopy: (String) -> Unit,
) {
    var addrText by remember(item.id, item.addr) { mutableStateOf(item.addr) }
    var tokenText by remember(item.id, item.token) { mutableStateOf(item.token) }
    var tokenShown by remember(item.id) { mutableStateOf(false) }
    var expanded by remember(item.id) { mutableStateOf(false) }
    val lane = if (item.kind == RemotePair.KIND_TS) {
        if (ServerProfile.isFunnelOrigin(item.addr)) "Funnel" else "直连"
    } else {
        ""
    }
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 6.dp)
            .clip(RoundedCornerShape(9.dp))
            .background(palette.elevate)
            .padding(horizontal = 7.dp, vertical = 5.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(
                checked = item.enabled,
                onCheckedChange = { on -> state.updatePair(item.id) { it.copy(enabled = on) } },
                modifier = Modifier.scale(0.7f),
            )
            OutlinedTextField(
                value = addrText,
                onValueChange = {
                    addrText = it
                    state.updatePair(item.id) { pair -> pair.copy(addr = it) }
                },
                singleLine = true,
                textStyle = MaterialTheme.typography.bodySmall,
                placeholder = { Text("地址（https://… 或 100.x:12300）", fontSize = 11.sp) },
                modifier = Modifier.weight(1f),
            )
            TinyTextButton(
                text = "↑",
                tint = palette.fgDim,
                onClick = { state.movePair(item.id, -1) },
            )
            TinyTextButton(
                text = "↓",
                tint = palette.fgDim,
                onClick = { state.movePair(item.id, 1) },
            )
            TinyTextButton(
                text = "✕",
                tint = palette.bad,
                onClick = { state.removePair(item.id) },
            )
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (lane.isNotEmpty()) {
                Text(
                    lane,
                    fontSize = 10.sp,
                    color = if (lane == "Funnel") palette.ok else palette.accent,
                    modifier = Modifier
                        .border(
                            1.dp,
                            (if (lane == "Funnel") palette.ok else palette.accent).copy(alpha = 0.5f),
                            RoundedCornerShape(999.dp),
                        )
                        .padding(horizontal = 5.dp, vertical = 1.dp),
                )
                Spacer(Modifier.width(6.dp))
            }
            Text(
                item.sourceLabel.ifEmpty { "手填" },
                fontSize = 10.sp, color = palette.fgFaint,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            InlineTimeoutField(
                label = "超时",
                value = item.timeoutText,
                enabled = item.enabled,
                onValueChange = { raw ->
                    val cleaned = raw.filter { it.isDigit() || it == '.' }.take(5)
                    state.updatePair(item.id) { pair -> pair.copy(timeoutText = cleaned) }
                },
            )
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("令牌", fontSize = 10.sp, color = palette.fgFaint)
            Spacer(Modifier.width(6.dp))
            if (tokenShown) {
                OutlinedTextField(
                    value = tokenText,
                    onValueChange = {
                        tokenText = it
                        state.updatePair(item.id) { pair -> pair.copy(token = it) }
                    },
                    singleLine = true,
                    textStyle = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.weight(1f),
                )
            } else {
                Text(
                    if (item.token.isEmpty()) "（未配）" else state.maskToken(item.token),
                    fontSize = 11.sp,
                    fontFamily = FontFamily.Monospace,
                    color = palette.fgDim,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier
                        .weight(1f)
                        .clickable { expanded = !expanded },
                )
            }
            TextButton(onClick = { tokenShown = !tokenShown }) {
                Text(if (tokenShown) "打码" else "改/看", fontSize = 11.sp)
            }
            TextButton(onClick = { onCopy(item.addr) }) {
                Text("复制地址", fontSize = 11.sp, color = palette.fgDim)
            }
        }
        if (expanded && item.token.isNotEmpty() && !tokenShown) {
            Text(
                item.token,
                fontSize = 11.sp,
                fontFamily = FontFamily.Monospace,
                color = palette.fg,
                modifier = Modifier.padding(top = 2.dp),
            )
        }
    }
}

/**
 * 单选的「探测方式」小件（连接优先级 / 多网卡各一组，所以抽出来）。
 * 每个选项占三等分之一宽，窄屏靠 Ellipsis 兜底，不会把文字压成 0 宽。
 */
@Composable
private fun ProbeModeChoice(
    label: String,
    selected: Boolean,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = modifier.clickable(onClick = onClick),
    ) {
        RadioButton(selected = selected, onClick = onClick)
        Text(
            label,
            fontSize = 11.5.sp,
            color = LocalPalette.current.fg,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

internal const val SERVER_EDIT_EMPTY_ADDRESS_HINT = "请填电脑地址"

/**
 * 紧凑的小图标按钮（30dp 方框、纯符号、无底色）。
 *
 * <p>为什么不用 `TextButton`：M3 的 TextButton 有 58dp 的最小宽度，一行里放三个
 * （配对行的 ↑ ↓ ✕）就吃掉 174dp —— 354dp 宽的手机上地址输入框只剩 78dp，
 * 实测只能显示「https」四个字母，地址根本没法看也没法改。这里自绘 30dp，
 * 三个才 90dp，地址框就能拿回 84dp。顺带干掉了按压态的紫色胶囊残留。
 */
@Composable
private fun TinyTextButton(
    text: String,
    tint: Color,
    enabled: Boolean = true,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Box(
        modifier = modifier
            .size(30.dp)
            .clip(RoundedCornerShape(8.dp))
            .clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            fontSize = 14.sp,
            color = if (enabled) tint else tint.copy(alpha = 0.35f),
        )
    }
}

/**
 * 「超时 [__] 秒」控件组：裸数字框 + 占位「默认」时用户看不出它是什么（与 PC 端同病同修），
 * 三处复用：通道优先级行 / 区块默认超时 / 每条配对。label 由调用处给（「超时」/「默认超时」）。
 */
@Composable
private fun InlineTimeoutField(
    label: String,
    value: String,
    enabled: Boolean,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val palette = LocalPalette.current
    Row(verticalAlignment = Alignment.CenterVertically, modifier = modifier) {
        Text(label, fontSize = 10.sp, color = palette.fgFaint)
        Spacer(Modifier.width(4.dp))
        SmallNumberField(
            value = value,
            enabled = enabled,
            placeholder = "默认",
            onValueChange = onValueChange,
            modifier = Modifier.width(56.dp),
        )
        Spacer(Modifier.width(2.dp))
        Text("秒", fontSize = 10.sp, color = palette.fgFaint)
    }
}
/**
 * 紧凑的小数字输入框（26dp 高、等宽居中）：探测超时用。
 *
 * <p>为什么不用 OutlinedTextField：M3 那个最小高度 56dp，六个通道就是 300+ dp，
 * 把整个连接策略折叠区撑爆；这里只要能输入几个数字，自绘更合适。
 */
@Composable
private fun SmallNumberField(
    value: String,
    enabled: Boolean,
    placeholder: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val palette = LocalPalette.current
    Box(
        modifier = modifier
            .height(26.dp)
            .clip(RoundedCornerShape(7.dp))
            .background(palette.card)
            .border(1.dp, if (enabled) palette.line else palette.line.copy(alpha = 0.5f), RoundedCornerShape(7.dp))
            .padding(horizontal = 4.dp),
        contentAlignment = Alignment.Center,
    ) {
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            enabled = enabled,
            singleLine = true,
            textStyle = TextStyle(
                fontSize = 11.sp,
                color = if (enabled) palette.fg else palette.fgFaint,
                textAlign = TextAlign.Center,
                fontFamily = FontFamily.Monospace,
            ),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
            decorationBox = { inner ->
                if (value.isEmpty()) {
                    Text(placeholder, fontSize = 9.sp, color = palette.fgFaint, maxLines = 1)
                }
                inner()
            },
        )
    }
}