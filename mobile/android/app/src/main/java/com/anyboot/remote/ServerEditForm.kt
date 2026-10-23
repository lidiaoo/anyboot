package com.anyboot.remote

import android.content.Context
import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
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
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

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

/** 表单的全部可编辑状态；宿主保存时读这里的当前值 */
internal class ServerEditFormState(
    val profile: ServerProfile?,
    context: Context,
) {
    val editing: Boolean = profile != null

    var group by mutableStateOf(profile?.group ?: "")
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

    /**
     * 远程接入：粘贴框内容（配对串 / 隧道 URL）。编辑态不回显令牌 ——
     * 有已存地址时只显示地址本身，重粘配对串才会更新令牌。
     */
    var remoteText by mutableStateOf(
        if (profile?.hasRemote() == true) ServerProfile.normalizeRemoteOrigin(profile.remoteAddr) else ""
    )
    var remoteHint by mutableStateOf("")

    /** 粘贴框内容变化后的即时识别提示 */
    fun applyRemoteHint() {
        val raw = remoteText.trim()
        if (raw.isEmpty()) {
            remoteHint = ""
            return
        }
        remoteHint = if (ServerProfile.parsePair(raw) != null) {
            "已识别：${ServerProfile.normalizeRemoteOrigin(if (raw.startsWith("anyboot://", ignoreCase = true)) ServerProfile.parsePair(raw)!![0] else raw)}"
        } else {
            "没认出配对码 —— 要 anyboot://pair?… 串或 https://… 隧道地址"
        }
    }

    /**
     * 保存用的远程解析结果：[origin, token] 或 null（框为空 = 取消远程接入）。
     * token 挖不出来时给空串 —— 宿主侧「留空保持已存令牌」。
     */
    fun resolvedRemote(): Array<String>? {
        val raw = remoteText.trim()
        if (raw.isEmpty()) {
            return null
        }
        val pair = ServerProfile.parsePair(raw) ?: return null
        return arrayOf(pair[0], pair[1])
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

    /** 连接优先："" = 自动（局域网优先）；"remote_first" = 远程优先（配了远程才生效） */
    var connMode by mutableStateOf(if (profile?.isRemoteFirst() == true) "remote_first" else "")

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
        FieldLabel("分组（用来区分不同的电脑）")
        OutlinedTextField(
            value = state.group,
            onValueChange = { state.group = it },
            singleLine = true,
            placeholder = { Text("例如：书房台式机（同一台电脑的多个 IP 填同一个分组）", fontSize = 12.sp) },
            modifier = Modifier.fillMaxWidth(),
        )
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

        FieldLabel("访问令牌 token")
        OutlinedTextField(
            value = state.token,
            onValueChange = { state.token = it },
            singleLine = true,
            placeholder = { Text("可留空，页面上也能填", fontSize = 12.sp) },
            modifier = Modifier.fillMaxWidth(),
        )

        FieldLabel("远程接入（出门时经公网隧道访问这台电脑；粘贴它生成的配对码）")
        OutlinedTextField(
            value = state.remoteText,
            onValueChange = {
                state.remoteText = it
                state.applyRemoteHint()
            },
            placeholder = {
                Text("anyboot://pair?… 或 https://xxx.trycloudflare.com", fontSize = 12.sp)
            },
            modifier = Modifier.fillMaxWidth(),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = {
                scanLauncher.launch(Intent(context, ScanPairActivity::class.java))
            }) {
                Text("📷 扫码读码", fontSize = 12.sp)
            }
            Text(
                if (state.remoteHint.isNotEmpty()) state.remoteHint
                else "扫电脑端弹出的配对二维码，或直接粘贴配对串",
                fontSize = 11.sp, color = palette.fgFaint,
                modifier = Modifier.weight(1f),
            )
        }
        Text(
            "局域网地址优先使用，全连不上才走远程；清空后保存 = 取消远程接入。" +
                "配对码等同密码，别转发给别人。",
            fontSize = 11.sp, color = palette.fgFaint,
        )

        FieldLabel("连接优先（配了远程接入时怎么选路）")
        Row(verticalAlignment = Alignment.CenterVertically) {
            RadioButton(
                selected = state.connMode != "remote_first",
                onClick = { state.connMode = "" }
            )
            Text(
                "自动（局域网优先）",
                fontSize = 13.sp, color = palette.fg,
                modifier = Modifier.weight(1f, fill = false),
            )
            Spacer(Modifier.width(10.dp))
            RadioButton(
                selected = state.connMode == "remote_first",
                onClick = { state.connMode = "remote_first" }
            )
            Text("远程优先", fontSize = 13.sp, color = palette.fg)
        }
        Text(
            "远程优先：隧道通了就直接走隧道，局域网只做兜底 —— 适合在家验证隧道、" +
                "或局域网不可信的场景；没配远程接入时此项无效",
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

        FieldLabel("多网卡探测（有多个地址时怎么探）")
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
    target.group = state.group.trim()
    target.name = state.name.trim().ifEmpty { target.host() }
    target.token = state.resolvedToken()
    target.probeMode = ServerProfile.normalizeProbeMode(state.probeMode)
    target.connMode = if (state.connMode == "remote_first") "remote_first" else ""
    target.jumpServerId = state.resolvedJumpId()
    // 远程接入：框为空 = 取消（清地址与令牌）；认出来 = 写 origin；
    // 令牌挖不到时保持已存的（编辑态不回显令牌，用户重粘配对串才会更新）
    val remote = state.resolvedRemote()
    if (remote == null) {
        target.clearRemote()
    } else {
        target.remoteAddr = remote[0]
        if (remote[1].isNotEmpty()) {
            target.remoteToken = remote[1]
        }
    }
    // 局域网电源路由：以表单里的这份为准（空 = 没配，走老路）
    target.powerRoutes.clear()
    for (route in state.routeList) {
        target.powerRoutes.add(route.copy())
    }
    return true
}

/** 表单底部提示用的一行说明（两个宿主共用同一句） */
internal const val SERVER_EDIT_EMPTY_ADDRESS_HINT = "请填电脑地址"