import SwiftUI

/// TOFU 主机指纹确认（SSH 首连）：continuation 挂在 State 里等按钮回填。
///
/// 为什么不用 `alert` 的 async 版本：确认框要在**任意**调用点弹（隧道、终端、跳板机测试），
/// 而弹窗宿主只可能有一个 —— 把 continuation 挂到 @State 里，谁需要谁等。
struct TofuPrompt: Identifiable {
    let id = UUID()
    let title: String
    let message: String
    let continuation: CheckedContinuation<Bool, Never>

    func resume(_ accepted: Bool) {
        continuation.resume(returning: accepted)
    }
}
