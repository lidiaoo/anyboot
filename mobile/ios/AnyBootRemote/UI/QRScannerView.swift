import SwiftUI
import AVFoundation

// MARK: - 扫码读配对码（一比一移植 ScanPairActivity.kt 297 行）
//
// Android 那边是独立 Activity（CAMERA 是运行时权限，相机生命周期要挂 Lifecycle），
// iOS 没有这层约束，所以是同一个 sheet 里的一整页，但**布局与文案逐字对齐**：
//
//   顶栏（‹ + 扫码读配对码 20Bold）
//   → 说明文字 12sp / fgDim
//   → 有权：黑底圆角 16 取景框（外边距 16）+ 底部「把二维码放进框内即可，稍等一两秒」
//     没权：「相机权限没给」+ 说明 + 返回 / 去设置里授权
//
// 只负责**取到那段文本**：解析与落库仍走 ServerProfile.parsePair / upsertPair
// （唯一一条解析路径，两处不能各写一套）。

struct QRScannerView: View {

    /// 扫到内容就回调（只回调一次）
    let onCode: (String) -> Void

    @Environment(\.dismiss) private var dismiss

    @State private var granted = AVCaptureDevice.authorizationStatus(for: .video) == .authorized
    /// 识别成功后分析线程还会继续吐帧，别让结果被二次回传
    @State private var delivered = false

    var body: some View {
        VStack(spacing: 0) {
            titleBar
            hintText

            if granted {
                cameraArea
            } else {
                deniedArea
            }
        }
        .background(ABColor.base.ignoresSafeArea())
        .onAppear(perform: requestPermissionIfNeeded)
    }

    // MARK: - 标题栏（:118-128）

    private var titleBar: some View {
        HStack(spacing: 0) {
            ABBarIcon(text: "‹", description: "返回") { dismiss() }
            Text("扫码读配对码")
                .font(.system(size: 20, weight: .bold))
                .foregroundColor(ABColor.fg)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity)
        .padding(.leading, ABMetric.px(4))
        .padding(.top, ABMetric.px(10))
        .padding(.bottom, ABMetric.px(8))
        .background(ABColor.base)
    }

    // MARK: - 说明（:130-135）

    private var hintText: some View {
        Text("对准电脑端「远程接入」页生成的二维码，认出来自动填回粘贴框。"
             + "没有摄像头权限也可以手动复制粘贴配对串。")
            .font(.system(size: 12))
            .foregroundColor(ABColor.fgDim)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, ABMetric.px(16))
    }

    // MARK: - 取景（:137-163）

    private var cameraArea: some View {
        VStack(spacing: 0) {
            QRCameraPreview(onCode: handle, onFailure: {
                // 相机被占用 / 没有摄像头等：不闪退，提示后留在页面上（Android 同）
                ABToastBus.shared.show("相机启动失败，可以手动粘贴配对串", seconds: 3.5)
            })
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .clipShape(RoundedRectangle(cornerRadius: ABMetric.px(16), style: .continuous))
            .padding(ABMetric.px(16))

            Text("把二维码放进框内即可，稍等一两秒")
                .font(.system(size: 11))
                .foregroundColor(ABColor.fgFaint)
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)
                .padding(.bottom, ABMetric.px(16))
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - 没权限：给个说法 + 出口，不黑屏干等（:164-197）

    private var deniedArea: some View {
        VStack(spacing: 0) {
            Text("相机权限没给")
                .font(.system(size: 16, weight: .bold))
                .foregroundColor(ABColor.fg)

            Spacer().frame(height: ABMetric.px(8))

            Text("扫码要用摄像头。不给也行 —— 回到上一页，把电脑端显示的"
                 + "配对串复制粘贴进「远程接入」框，效果一样。")
                .font(.system(size: 12))
                .foregroundColor(ABColor.fgDim)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)

            Spacer().frame(height: ABMetric.px(16))

            textButton("返回") { dismiss() }
            // 系统不给「永久拒绝」的再次拉起，用户只能去设置里开
            textButton("去设置里授权", action: openSystemSettings)
        }
        .padding(ABMetric.px(24))
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// Material TextButton 口径：纯文字、强调色、无描边
    private func textButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(ABColor.accent)
                .padding(.horizontal, ABMetric.px(12))
                .padding(.vertical, ABMetric.px(8))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - 行为

    private func requestPermissionIfNeeded() {
        guard !granted else { return }
        AVCaptureDevice.requestAccess(for: .video) { ok in
            DispatchQueue.main.async { granted = ok }
        }
    }

    private func handle(_ text: String) {
        guard !text.isEmpty, !delivered else { return }
        delivered = true
        onCode(text)
        dismiss()
    }

    private func openSystemSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url) { ok in
            if !ok { ABToastBus.shared.show("打不开设置，请手动到系统设置里授权") }
        }
    }
}

// MARK: - 取景 + 解码

/// AVCaptureVideoPreviewLayer 直接做 backing layer，尺寸跟着布局走
private final class QRCameraPreviewView: UIView {
    override static var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
    var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
}

private struct QRCameraPreview: UIViewControllerRepresentable {

    let onCode: (String) -> Void
    let onFailure: () -> Void

    func makeUIViewController(context: Context) -> Controller {
        let controller = Controller()
        controller.onCode = onCode
        controller.onFailure = onFailure
        return controller
    }

    func updateUIViewController(_ controller: Controller, context: Context) {}

    final class Controller: UIViewController, AVCaptureMetadataOutputObjectsDelegate {

        var onCode: ((String) -> Void)?
        var onFailure: (() -> Void)?

        private let session = AVCaptureSession()
        private var handled = false

        override func loadView() {
            let view = QRCameraPreviewView()
            view.backgroundColor = .black
            view.previewLayer.session = session
            // Android: PreviewView.ScaleType.FILL_CENTER
            view.previewLayer.videoGravity = .resizeAspectFill
            self.view = view
        }

        override func viewDidLoad() {
            super.viewDidLoad()
            configure()
        }

        private func configure() {
            guard let device = AVCaptureDevice.default(.builtInWideAngleCamera,
                                                       for: .video, position: .back)
                    ?? AVCaptureDevice.default(for: .video),
                  let input = try? AVCaptureDeviceInput(device: device),
                  session.canAddInput(input) else {
                onFailure?()
                return
            }
            session.beginConfiguration()
            session.addInput(input)

            let output = AVCaptureMetadataOutput()
            guard session.canAddOutput(output) else {
                session.commitConfiguration()
                onFailure?()
                return
            }
            session.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            // Android 只解 QR_CODE（zxing POSSIBLE_FORMATS）
            if output.availableMetadataObjectTypes.contains(.qr) {
                output.metadataObjectTypes = [.qr]
            }
            // 960×960 那档分辨率在 iOS 由 AVCaptureSession 自选，不加 preset 更高清即可
            session.commitConfiguration()

            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                self?.session.startRunning()
            }
        }

        override func viewWillDisappear(_ animated: Bool) {
            super.viewWillDisappear(animated)
            guard session.isRunning else { return }
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                self?.session.stopRunning()
            }
        }

        func metadataOutput(_ output: AVCaptureMetadataOutput,
                            didOutput metadataObjects: [AVMetadataObject],
                            from connection: AVCaptureConnection) {
            guard !handled,
                  let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
                  let value = object.stringValue, !value.isEmpty else { return }
            handled = true
            onCode?(value)
        }
    }
}
