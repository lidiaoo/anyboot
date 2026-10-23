package com.anyboot.remote

/**
 * 电源动作的提示小字 —— 电脑列表电源键与主界面悬浮球共用（对齐 PC 端 hub.html 的 pcPowerHint）。
 *
 * 小字以「传输方式」标识开头：HTTP / 魔法包 / HTTP+魔法包（混合）/ HTTP→魔法包（HTTP 优先）/
 * 魔法包→HTTP（auto）。`remoteActive` = 这台电脑当前正走远程隧道：隧道只转 HTTP，
 * 广播包腿发不出 —— 纯广播包的搭配退化成 HTTP，只剩广播包的标「发不出」。
 *
 * 配了「局域网电源路由」→ 标识后跟「→ 地址:端口 等 N 条」（取第一条，多条带计数）；
 * 没配路由 → 唤醒按 MAC/隧道三态给标识，其余动作用调用方给的兜底说明（如「挂起到内存」）。
 */
internal fun powerHint(
    profile: ServerProfile,
    action: String,
    fallback: String,
    remoteActive: Boolean = false,
): String {
    val routes = profile.routesFor(action)
    val badge = powerBadge(profile, action, routes, remoteActive)
    if (routes.isNotEmpty()) {
        val route = routes[0]
        val target = ServerProfile.text(route.target).ifEmpty { "自动广播" }
        val suffix = if (routes.size > 1) " 等 ${routes.size} 条" else ""
        return "$badge · → $target:${route.effectivePort()}$suffix"
    }
    // 唤醒走不了隧道（魔术包是局域网 UDP 广播，公网发不出去）：
    // 未配 MAC / 隧道使用中分别直说，别给会误导的提示
    if (action == ServerProfile.POWER_WAKE) {
        if (!profile.hasMacs()) return "未配 MAC"
        if (remoteActive) return "发不出 · 魔术包仅局域网有效"
        return "魔法包"
    }
    return if (fallback.isEmpty()) badge else "$badge · $fallback"
}

/** 传输方式标识（与 PC 端 pcPowerBadge 同一套）：routes = 该动作启用中的路由 */
internal fun powerBadge(
    profile: ServerProfile,
    action: String,
    routes: List<ServerProfile.PowerRoute>,
    remoteActive: Boolean,
): String {
    val packetOk = !remoteActive
    if (routes.isEmpty()) {
        if (action == ServerProfile.POWER_WAKE) {
            return when {
                !profile.hasMacs() -> "未配 MAC"
                packetOk -> "魔法包"
                else -> "发不出"
            }
        }
        return "HTTP"
    }
    return when (ServerProfile.routesMode(routes)) {
        "http_only" -> "HTTP"
        "http_first" -> if (packetOk) "HTTP→魔法包" else "HTTP"
        "hybrid" -> if (packetOk) "HTTP+魔法包" else "HTTP"
        "auto" -> if (packetOk) "魔法包→HTTP" else "HTTP"
        else -> if (packetOk) "魔法包" else "发不出"
    }
}
