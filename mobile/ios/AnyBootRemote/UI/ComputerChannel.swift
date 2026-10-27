import Foundation

/// 电脑卡片里的「一行通道」—— 直连地址 / 跳板机 / 每条远程配对。
///
/// 这是**列表机制的唯一展示口径**：优先级（顺序）、停用（置灰）、探测状态（耗时 / 未测出）、
/// 使用中（赢家）全部由模型 + 心跳算好，UI 只画。电脑页不许另写一套连接标注。
struct ComputerChannel: Identifiable {

    /// 行 id：`kind + pairId + origin`（同一通道里的多条配对、多个地址各占一行）
    let id: String
    let kind: String
    /// 徽章文案（直连 / 跳板机 / Cloudflare / TS 直连 / TS Funnel / 内置 Funnel）
    let title: String
    /// 第二列：地址或跳板机名
    let detail: String
    let pairId: String
    /// 探测匹配用的 origin（局域网行 = host:port；跳板机行为空；远程行 = 归一 origin）
    let origin: String
    /// 停用（通道停用或配对条目停用）—— 照常亮出但置灰
    let disabled: Bool
    /// 复制 / 浏览器打开用的地址（跳板机行没有）
    let copyText: String
}

extension ServerProfile {

    /// 按优先级顺序展开全部通道行（含停用的 —— 停用也要看得见，否则用户以为配置丢了）
    func channels(jumpName: (String) -> String?, transport: String = "") -> [ComputerChannel] {
        var rows: [ComputerChannel] = []
        var doneBlocks: [String] = []
        for kind in routeOrderSorted() {
            let channelOff = routeDisabled.contains(kind)
            if kind == ServerProfile.routeLan {
                for (index, address) in addresses.enumerated() {
                    let url = url(at: index, transport: transport)?.absoluteString ?? address
                    rows.append(ComputerChannel(
                        id: "lan|\(address)", kind: kind,
                        title: ServerProfile.routeLabel(kind), detail: address,
                        pairId: "", origin: address, disabled: channelOff, copyText: url))
                }
                continue
            }
            if kind == ServerProfile.routeJump {
                let jumpId = jumpServerId.trimmedAnyBoot
                guard !jumpId.isEmpty else { continue }
                let name = jumpName(jumpId) ?? "（跳板机已删除）"
                rows.append(ComputerChannel(
                    id: "jump|\(jumpId)", kind: kind,
                    title: ServerProfile.routeLabel(kind), detail: name,
                    pairId: "", origin: "", disabled: channelOff,
                    copyText: ""))
                continue
            }
            let block = RemotePair.blockOfRoute(kind)
            if block.isEmpty || doneBlocks.contains(block) { continue }
            doneBlocks.append(block)
            for pair in pairsOfBlock(block, enabledOnly: false) {
                let pairKind = pair.routeKind
                guard !pairKind.isEmpty else { continue }
                let origin = ServerProfile.normalizeRemoteOrigin(pair.addr)
                var copy = origin
                let secret = pair.token.trimmedAnyBoot
                if !secret.isEmpty {
                    copy += "/?ab_token=" + (secret.addingPercentEncoding(
                        withAllowedCharacters: .urlQueryAllowed) ?? secret)
                }
                let host = pair.host.trimmedAnyBoot
                let detail = host.isEmpty ? origin : "\(host) · \(origin)"
                rows.append(ComputerChannel(
                    id: "\(pairKind)|\(pair.id)|\(origin)", kind: pairKind,
                    title: ServerProfile.routeLabel(pairKind), detail: detail,
                    pairId: pair.id, origin: origin,
                    disabled: channelOff || !pair.enabled, copyText: copy))
            }
        }
        return rows
    }
}
