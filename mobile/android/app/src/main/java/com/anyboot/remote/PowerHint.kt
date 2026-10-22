package com.anyboot.remote

/**
 * 电源动作的悬浮提示小字 —— 电脑列表电源键与主界面悬浮球共用（对齐 PC 端 hub.html 的 pcPowerHint）。
 *
 * 配了「局域网电源路由」→ 显示「→ 地址:端口 等 N 条」（取第一条，多条带计数）；
 * 没配路由 → 返回调用方给的兜底说明（唤醒 = MAC 魔术包 / 未配 MAC，睡眠 = 挂起到内存，重启/关机 = 空）。
 */
internal fun powerHint(profile: ServerProfile, action: String, fallback: String): String {
    val routes = profile.routesFor(action)
    if (routes.isEmpty()) {
        return fallback
    }
    val route = routes[0]
    val target = ServerProfile.text(route.target).ifEmpty { "自动广播" }
    val suffix = if (routes.size > 1) " 等 ${routes.size} 条" else ""
    return "→ $target:${route.effectivePort()}$suffix"
}
