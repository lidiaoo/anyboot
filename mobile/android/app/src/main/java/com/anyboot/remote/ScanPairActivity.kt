package com.anyboot.remote

import android.content.Intent
import android.os.Bundle
import android.util.Size
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.LuminanceSource
import com.google.zxing.MultiFormatReader
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import java.util.concurrent.Executors

/**
 * 扫码读「远程接入」配对码（anyboot://pair?… 或 https://xxx.trycloudflare.com）。
 *
 * 为什么独立 Activity：CAMERA 是运行时权限，且相机生命周期要挂在
 * LifecycleOwner 上 —— 宿主是两个编辑弹层（AlertDialog / 底部 sheet），
 * 塞在里面权限请求与相机绑定都很别扭；StartActivityForResult 回传一行文本最干净。
 *
 * 依赖组合：CameraX 取景（camera-view 的 PreviewView）+ zxing core 解码
 * （纯解码库，YUV 平面直接喂 PlanarYUVLuminanceSource）。锁竖屏
 * （Manifest），但传感器方向仍可能是横向帧，解码循环里把 luminance
 * source 旋转四向各试一遍。
 *
 * 结果：识别成功 RESULT_OK + EXTRA_TEXT；用户放弃 / 没权限 / 相机起不来
 * RESULT_CANCELED（宿主粘贴框保持原样，等价于取消）。
 */
class ScanPairActivity : ComponentActivity() {

    companion object {
        const val EXTRA_TEXT = "text"
    }

    /** 识别成功后置位：分析线程还会继续吐帧，别让结果被二次回传 */
    private val delivered = java.util.concurrent.atomic.AtomicBoolean(false)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Ui.applySystemBars(this)
        setContent { Root() }
    }

    /** 统一回传出口：text 空 = 放弃 */
    private fun finishWith(text: String) {
        val intent = Intent().putExtra(EXTRA_TEXT, text)
        if (text.isEmpty()) {
            setResult(RESULT_CANCELED, intent)
        } else {
            setResult(RESULT_OK, intent)
        }
        finish()
    }

    @Composable
    private fun Root() {
        // ⚠️ 取色必须在 AppTheme 里面：LocalPalette 的默认值是
        // paletteOf(Application())，没有 base context，直接读会 NPE（项目铁律）
        AppTheme {
            val palette = LocalPalette.current
            val context = LocalContext.current
            var granted by remember {
                mutableStateOf(
                    checkSelfPermission(android.Manifest.permission.CAMERA)
                        == android.content.pm.PackageManager.PERMISSION_GRANTED
                )
            }
            val permissionLauncher = rememberLauncherForActivityResult(
                ActivityResultContracts.RequestPermission()
            ) { granted = it }
            LaunchedEffect(Unit) {
                if (!granted) permissionLauncher.launch(android.Manifest.permission.CAMERA)
            }

            Column(Modifier.fillMaxSize().background(palette.base)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(palette.base)
                        .padding(start = 4.dp, top = 10.dp, bottom = 8.dp),
                ) {
                    BarIcon("‹", "返回") { finishWith("") }
                    Text("扫码读配对码", fontSize = 20.sp, fontWeight = FontWeight.Bold, color = palette.fg)
                }

                Text(
                    "对准电脑端「远程接入」页生成的二维码，认出来自动填回粘贴框。" +
                        "没有摄像头权限也可以手动复制粘贴配对串。",
                    fontSize = 12.sp, color = palette.fgDim,
                    modifier = Modifier.padding(horizontal = 16.dp),
                )

                if (granted) {
                    Box(
                        modifier = Modifier
                            .fillMaxWidth()
                            .weight(1f)
                            .padding(16.dp)
                            .clip(RoundedCornerShape(16.dp))
                            .background(Color.Black),
                    ) {
                        CameraView(
                            modifier = Modifier.fillMaxSize(),
                            onText = { text ->
                                // 分析线程回调：只收首次有效结果，回主线程收工
                                if (text.isNotEmpty() && delivered.compareAndSet(false, true)) {
                                    runOnUiThread { finishWith(text) }
                                }
                            },
                        )
                    }
                    Text(
                        "把二维码放进框内即可，稍等一两秒",
                        fontSize = 11.sp, color = palette.fgFaint,
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(bottom = 16.dp),
                        textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    )
                } else {
                    // 没权限：给个说法 + 出口，不黑屏干等
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        modifier = Modifier
                            .fillMaxWidth()
                            .weight(1f)
                            .padding(24.dp),
                        verticalArrangement = androidx.compose.foundation.layout.Arrangement.Center,
                    ) {
                        Text("相机权限没给", fontSize = 16.sp, fontWeight = FontWeight.Bold, color = palette.fg)
                        Spacer(Modifier.height(8.dp))
                        Text(
                            "扫码要用摄像头。不给也行 —— 回到上一页，把电脑端显示的" +
                                "配对串复制粘贴进「远程接入」框，效果一样。",
                            fontSize = 12.sp, color = palette.fgDim,
                            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                        )
                        Spacer(Modifier.height(16.dp))
                        TextButton(onClick = { finishWith("") }) { Text("返回") }
                        TextButton(onClick = {
                            // 系统不给「永久拒绝」的再次拉起，用户只能去设置里开
                            try {
                                startActivity(
                                    Intent(
                                        android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                                        android.net.Uri.fromParts("package", packageName, null),
                                    )
                                )
                            } catch (ignored: Exception) {
                                Toast.makeText(context, "打不开设置，请手动到系统设置里授权", Toast.LENGTH_SHORT).show()
                            }
                        }) { Text("去设置里授权") }
                    }
                }
            }
        }
    }

    /** CameraX 取景 + zxing 解码；识别到文本回传一次 */
    @Composable
    private fun CameraView(modifier: Modifier, onText: (String) -> Unit) {
        val lifecycleOwner = LocalLifecycleOwner.current
        // 单线程分析器池：解码是 CPU 活，串行即可；KEEP_ONLY_LATEST 保证不积压
        val analyzerExecutor = remember {
            Executors.newSingleThreadExecutor { r ->
                Thread(r, "anyboot-qr").apply { isDaemon = true }
            }
        }

        AndroidView(
            factory = { ctx ->
                val previewView = PreviewView(ctx)
                previewView.scaleType = PreviewView.ScaleType.FILL_CENTER
                val providerFuture = ProcessCameraProvider.getInstance(ctx)
                providerFuture.addListener({
                    try {
                        val provider = providerFuture.get()
                        val preview = Preview.Builder().build().also {
                            it.setSurfaceProvider(previewView.surfaceProvider)
                        }
                        val analysis = ImageAnalysis.Builder()
                            .setResolutionSelector(
                                ResolutionSelector.Builder()
                                    .setResolutionStrategy(
                                        ResolutionStrategy(
                                            Size(960, 960),
                                            ResolutionStrategy.FALLBACK_RULE_CLOSEST_LOWER_THEN_HIGHER,
                                        )
                                    )
                                    .build()
                            )
                            .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                            .build()
                            .also {
                                it.setAnalyzer(analyzerExecutor, QrAnalyzer(onText))
                            }
                        provider.unbindAll()
                        provider.bindToLifecycle(
                            lifecycleOwner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis,
                        )
                    } catch (ignored: Exception) {
                        // 相机被占用 / 没有摄像头等：不闪退，提示后退出
                        runOnUiThread {
                            Toast.makeText(ctx, "相机启动失败，可以手动粘贴配对串", Toast.LENGTH_LONG).show()
                        }
                    }
                }, ContextCompat.getMainExecutor(ctx))
                previewView
            },
            modifier = modifier,
        )
    }
}

/**
 * zxing 解码器：CameraX 的 YUV_420_888 第一平面（luma）直接够用，
 * 竖屏锁了但传感器帧可能横向 —— luminance source 旋转四向各试一遍。
 */
private class QrAnalyzer(private val onText: (String) -> Unit) : ImageAnalysis.Analyzer {

    private val reader = MultiFormatReader().apply {
        setHints(mapOf(DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE)))
    }

    override fun analyze(image: ImageProxy) {
        try {
            val plane = image.planes[0]
            val data = ByteArray(plane.buffer.remaining()).also { plane.buffer.get(it) }
            var source: LuminanceSource = PlanarYUVLuminanceSource(
                data, image.width, image.height, 0, 0, image.width, image.height, false,
            )
            // 四个朝向各试一遍（0/90/180/270）：旋转开销在 960px 下可接受。
            // 注意解码会抛 NotFoundException/ChecksumException/FormatException，
            // 都收进 ReaderException（只捕 NotFound 会半途打断旋转循环）
            repeat(4) {
                try {
                    val text = reader.decodeWithState(BinaryBitmap(HybridBinarizer(source))).text
                    onText(text)
                    return
                } catch (ignored: com.google.zxing.ReaderException) {
                    reader.reset()
                    source = source.rotateCounterClockwise()
                }
            }
        } catch (ignored: Exception) {
            // 坏帧 / 半帧：丢掉，下一帧再来（子线程裸奔会崩进程，项目铁律）
        } finally {
            reader.reset()
            image.close()
        }
    }
}
