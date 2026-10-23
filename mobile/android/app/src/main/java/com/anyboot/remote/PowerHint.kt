package com.anyboot.remote

/**
 * 电源动作的悬浮提示小字 —— 电脑列表电源键与主界面悬浮球共用（对齐 PC 端 hub.html 的 pcPowerHint）。
 *
 * 配了「局域网电源路由」→ 显示「→ 地址:端口 等 N 条」（取第一条，多条带计数）；
 * 没配路由 → 返回调用方给的兜底说明（唤醒 = MAC 魔术包 / 未配 MAC，睡眠 = 挂起到内存，重启/关机 = 空）。
 */
internal fun powerHint(profile: ServerProfile, action: String, fallback: String): String {
    val routes = profile.routesFor(action)
    if (routes.isNotEmpty()) {
        val route = routes[0]
        val target = ServerProfile.text(route.target).ifEmpty { "自动广播" }
        val suffix = if (routes.size > 1) " 等 ${routes.size} 条" else ""
        return "→ $target:${route.effectivePort()}$suffix"
    }
    // 唤醒走不了隧道（魔术包是局域网 UDP 广播，公网发不出去）：配了远程接入时
    // 别再给「MAC 魔术包」这种会误导的提示，按三态说清楚
    if (action == ServerProfile.POWER_WAKE && profile.hasRemote()) {
        return if (profile.hasMacs()) {
            "MAC 魔术包（仅局域网有效；远程时发不出）"
        } else {
            "未配 MAC —— 远程时无法唤醒，需在局域网内"
        }
    }
    return fallback
}
