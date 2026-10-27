import SwiftUI

/// 展示层派生属性（模型层保持纯 Foundation，颜色/徽标跟 UI 走）。
extension ServerProfile {

    /// 分组配色：同一个分组恒定同一个颜色，扫一眼标签栏就知道哪几个是同一台电脑
    var groupColor: Color {
        let groupName = group.trimmedAnyBoot
        guard !groupName.isEmpty else { return Color(.systemGray3) }
        var hash: UInt64 = 5381
        for byte in groupName.utf8 {
            hash = hash &* 33 &+ UInt64(byte)
        }
        let hue = Double(hash % 360) / 360.0
        return Color(hue: hue, saturation: 0.45, brightness: 0.78)
    }

    /// 系统族徽标（SF Symbol 名）：列表卡片左上角那个小图标
    var osSymbol: String {
        switch ServerProfile.normalizeMachineOs(machineOs) {
        case "windows": return "pc"
        case "macos": return "laptopcomputer"
        case "linux": return "terminal"
        default: return "desktopcomputer"
        }
    }

    /// 系统族的短文案（没有标识时给空串，UI 不画）
    var osLabel: String {
        switch ServerProfile.normalizeMachineOs(machineOs) {
        case "windows": return "Windows"
        case "macos": return "macOS"
        case "linux": return "Linux"
        default: return ""
        }
    }
}

extension RemotePair {
    /// 通道配色的语义名（UI 里再映射成实际 Color）
    var channelTone: String {
        switch routeKind {
        case ServerProfile.routeTunnel: return "cloudflare"
        case ServerProfile.routeTS: return "ts"
        case ServerProfile.routeFunnel: return "funnel"
        case ServerProfile.routeTSNet: return "tsnet"
        default: return "other"
        }
    }
}
