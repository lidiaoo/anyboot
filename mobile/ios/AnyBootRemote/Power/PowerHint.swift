import Foundation

// MARK: - 电源动作提示小字（一比一移植 PowerHint.kt 62 行）
//
// 电脑列表的电源键与电脑页悬浮球共用一套文案。小字以「传输方式」标识开头：
// HTTP / 魔法包 / HTTP+魔法包（混合）/ HTTP→魔法包（HTTP 优先）/ 魔法包→HTTP（auto）。
// `remoteActive` = 这台电脑当前正走远程隧道：隧道只转 HTTP，广播包发不出去 ——
// 纯广播包的搭配退化成 HTTP，只剩广播包的标「发不出」。

enum PowerHint {

    static func text(_ profile: ServerProfile,
                     action: String,
                     fallback: String = "",
                     remoteActive: Bool = false) -> String {
        let routes = profile.routesFor(action)
        let badge = self.badge(profile, action: action, routes: routes, remoteActive: remoteActive)

        if !routes.isEmpty {
            let route = routes[0]
            let rawTarget = route.target.trimmingCharacters(in: .whitespaces)
            let target = rawTarget.isEmpty ? "自动广播" : rawTarget
            let suffix = routes.count > 1 ? " 等 \(routes.count) 条" : ""
            return "\(badge) · → \(target):\(route.effectivePort)\(suffix)"
        }

        // 唤醒走不了隧道（魔术包是局域网 UDP 广播，公网发不出去）：
        // 未配 MAC / 隧道使用中分别直说，别给会误导的提示
        if action == PowerRoute.actionWake {
            if !profile.hasMacs { return "未配 MAC" }
            if remoteActive { return "发不出 · 魔术包仅局域网有效" }
            return "魔法包"
        }
        return fallback.isEmpty ? badge : "\(badge) · \(fallback)"
    }

    /// 传输方式标识（与 PC 端 pcPowerBadge 同一套）：routes = 该动作启用中的路由
    static func badge(_ profile: ServerProfile,
                      action: String,
                      routes: [PowerRoute],
                      remoteActive: Bool) -> String {
        let packetOk = !remoteActive
        if routes.isEmpty {
            if action == PowerRoute.actionWake {
                if !profile.hasMacs { return "未配 MAC" }
                return packetOk ? "魔法包" : "发不出"
            }
            return "HTTP"
        }
        switch ServerProfile.routesMode(routes) {
        case "http_only":
            return "HTTP"
        case "http_first":
            return packetOk ? "HTTP→魔法包" : "HTTP"
        case "hybrid":
            return packetOk ? "HTTP+魔法包" : "HTTP"
        case "auto":
            return packetOk ? "魔法包→HTTP" : "HTTP"
        default:
            return packetOk ? "魔法包" : "发不出"
        }
    }
}
