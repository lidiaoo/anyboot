import SwiftUI

/// 应急手输 SSH 密码 —— 终端页与服务器页**共用这一份**（对齐 Android 同名的两处实现，
/// 文案 / 默认值 / 落盘行为完全一致）。
///
/// 密码保存策略（与 Android 6786caf 之后的现状对齐）：
/// 1. 编辑服务器里勾「保存密码 / 凭据」（默认勾选）→ 加密存本机，连接免输；
/// 2. 只有「密码认证 && 本机拿不到可用的密码」才弹这个框；
/// 3. 框里「保存密码」默认勾选 → 输一次即落 Keychain，下次不再弹；
/// 4. 落盘失败（钥匙串异常）不静默：明确提示，且绝不写明文。
struct ManualPasswordBody: View {

    let request: ServerModuleStore.ManualPasswordRequest
    let onDone: () -> Void

    @State private var password = ""
    @State private var remember = true

    private var userLabel: String {
        let value = request.server.username.trimmingCharacters(in: .whitespaces)
        return value.isEmpty ? "root" : value
    }

    var body: some View {
        ABDialogNote(request.server.addressLabel + " · 用户 " + userLabel)
        ABDialogInput(placeholder: remember ? "输一次就存下来（下次免输）" : "仅本次连接有效，不保存",
                      text: $password, secure: true)
        Button {
            remember.toggle()
        } label: {
            HStack(spacing: ABMetric.px(8)) {
                RoundedRectangle(cornerRadius: ABMetric.px(3), style: .continuous)
                    .strokeBorder(ABColor.accent, lineWidth: 2)
                    .background(
                        RoundedRectangle(cornerRadius: ABMetric.px(3), style: .continuous)
                            .fill(remember ? ABColor.accent : .clear))
                    .frame(width: ABMetric.px(16), height: ABMetric.px(16))
                Text("保存密码（加密存本机 · 下次免输）")
                    .font(.system(size: 13))
                    .foregroundColor(ABColor.fg)
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("保存密码")
        .accessibilityValue(remember ? "已勾选" : "未勾选")
        ABDialogButtons(confirmTitle: "连接",
                        onConfirm: {
                            SshSessionManager.shared.setManualPassword(request.server.id, password)
                            if remember && !password.isEmpty
                                && !JumpStore.shared.rememberPassword(request.server.id, password) {
                                // 与 Android 一致：写盘失败要说出来，但这次连接照常继续
                                ServerModuleStore.shared.toast("密码加密保存失败（钥匙串异常），"
                                                               + "本次仍可连接，下次需重输", long: true)
                            }
                            onDone()
                            request.onReady()
                        },
                        onDismiss: onDone)
    }
}
