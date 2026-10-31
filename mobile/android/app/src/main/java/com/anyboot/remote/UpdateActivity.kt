package com.anyboot.remote

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.io.File

/**
 * 检查更新页（对齐设计稿 docs/update-ui-mockup.html D 区「移动端简版」）。
 *
 * 状态机与桌面端一致：idle → checking → up_to_date / available → downloading →
 * verifying → ready → launching → error。**客户端不做覆盖逻辑**：校验通过后把 APK
 * 交给系统安装器，同包名覆盖升级由系统处理，应用数据保留。
 *
 * 口径：版本严格只升不降（UpdateCore.isNewer 解析失败 → 无更新）；校验不过不装。
 */
class UpdateActivity : ComponentActivity() {

    private var phase by mutableStateOf("idle")
    private var release by mutableStateOf<AppRelease?>(null)
    private var error by mutableStateOf("")
    private var downloaded by mutableStateOf(0L)
    private var total by mutableStateOf(0L)
    private var source by mutableStateOf("")
    private var installedFile by mutableStateOf<File?>(null)
    private var currentVersion by mutableStateOf("")
    private var checkedAt by mutableStateOf(0L)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Ui.applySystemBars(this)
        currentVersion = AppUpdate.currentVersion(this)
        checkedAt = AppUpdate.lastCheckedAt(this)
        setContent { Root() }
        // 进页面就查一次：用户点进来就是想知道有没有新版
        check(manual = false)
    }

    // ── 业务动作（全部丢后台线程，结果回到主线程改状态） ─────────────────────

    private fun check(manual: Boolean) {
        if (phase == "checking" || phase == "downloading") return
        phase = "checking"
        error = ""
        Thread({
            try {
                val found = AppUpdate.check()
                AppUpdate.rememberCheck(this, found.version)
                runOnUiThread {
                    release = found
                    checkedAt = AppUpdate.lastCheckedAt(this)
                    phase = if (UpdateCore.isNewer(found.version, currentVersion)) "available" else "up_to_date"
                    if (!manual && phase == "up_to_date") {
                        // 自动检查点到即止：已最新就不多话
                    }
                }
            } catch (failure: Exception) {
                runOnUiThread {
                    phase = "error"
                    error = failure.message ?: "检查失败"
                }
            }
        }, "anyboot-update-check").start()
    }

    private fun download() {
        val target = release ?: return
        if (phase == "downloading") return
        phase = "downloading"
        downloaded = 0
        total = target.apkSize
        source = ""
        error = ""
        Thread({
            try {
                val outcome = AppUpdate.download(this, target) { done, size, label ->
                    runOnUiThread {
                        downloaded = done
                        if (size > 0) total = size
                        source = label
                        if (phase == "downloading" && done < size) phase = "downloading"
                    }
                }
                runOnUiThread {
                    installedFile = outcome.file
                    source = outcome.source
                    downloaded = total
                    phase = "ready"
                }
            } catch (failure: Exception) {
                runOnUiThread {
                    phase = "error"
                    error = failure.message ?: "下载失败"
                }
            }
        }, "anyboot-update-download").start()
    }

    private fun install() {
        val file = installedFile ?: return
        if (!AppUpdate.canInstallPackages(this)) {
            // 第一次装会遇到这道门：把「为什么」和「去哪儿开」说清楚再跳系统授权页
            error = "Android 需要你允许「安装未知应用」才能装这个 APK。" +
                "下一步：在接下来的系统页面里打开「允许来自此来源」，然后回来再点安装。"
            phase = "need_permission"
            AppUpdate.openInstallPermission(this)
            return
        }
        try {
            AppUpdate.install(this, file)
            phase = "launching"
            error = ""
        } catch (failure: Exception) {
            phase = "error"
            error = failure.message ?: "无法启动系统安装器"
        }
    }

    // ── UI ─────────────────────────────────────────────────────────────────

    @Composable
    private fun Root() {
        AppTheme {
            val palette = LocalPalette.current
            Column(Modifier.fillMaxSize().background(palette.base)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(palette.base)
                        .padding(start = 4.dp, top = 10.dp, bottom = 8.dp),
                ) {
                    BarIcon("‹", "返回") { finish() }
                    Text("检查更新", fontSize = 20.sp, fontWeight = FontWeight.Bold, color = palette.fg)
                }

                Column(
                    Modifier
                        .fillMaxWidth()
                        .verticalScroll(rememberScrollState())
                        .padding(start = 20.dp, top = 8.dp, end = 20.dp, bottom = 28.dp),
                ) {
                    CurrentVersionCard()
                    Spacer(Modifier.height(14.dp))
                    PhaseBlock()
                    val info = release
                    if (info != null && showReleaseCard()) {
                        Spacer(Modifier.height(14.dp))
                        ReleaseCard(info)
                    }
                    if (showProgress()) {
                        Spacer(Modifier.height(14.dp))
                        ProgressBlock()
                    }
                    if (error.isNotEmpty()) {
                        Spacer(Modifier.height(14.dp))
                        ErrorBlock()
                    }
                    Spacer(Modifier.height(18.dp))
                    Actions()
                    Spacer(Modifier.height(18.dp))
                    Text(
                        "安装由系统安装器完成：同包名会覆盖升级，应用数据（服务器、密钥、设置）保留。" +
                            "首次安装需要允许「安装未知应用」。",
                        fontSize = 11.sp,
                        lineHeight = 17.sp,
                        color = palette.fgFaint,
                    )
                }
            }
        }
    }

    @Composable
    private fun CurrentVersionCard() {
        val palette = LocalPalette.current
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(16.dp))
                .background(palette.card)
                .border(1.dp, palette.line, RoundedCornerShape(16.dp))
                .padding(horizontal = 16.dp, vertical = 14.dp),
        ) {
            Column(Modifier.weight(1f)) {
                Text("AnyBoot Remote", fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = palette.fg)
                Text(
                    "当前版本 v$currentVersion",
                    fontSize = 12.sp,
                    fontFamily = FontFamily.Monospace,
                    color = palette.fgFaint,
                    modifier = Modifier.padding(top = 2.dp),
                )
            }
            val whenText = humanAgo(checkedAt)
            if (whenText.isNotEmpty()) {
                Text(whenText, fontSize = 11.sp, color = palette.fgFaint)
            }
        }
    }

    @Composable
    private fun PhaseBlock() {
        val palette = LocalPalette.current
        val text = when (phase) {
            "idle" -> ""
            "checking" -> "正在检查新版本…"
            "up_to_date" -> "已是最新版本"
            "available" -> "发现新版本 v${release?.version ?: ""}"
            "downloading" -> "正在下载 v${release?.version ?: ""}…"
            "verifying" -> "正在校验 SHA-256…"
            "ready" -> "下载完成，SHA-256 校验通过"
            "need_permission" -> "还需要一次系统授权"
            "launching" -> "已交给系统安装器"
            "error" -> "更新没有完成"
            else -> ""
        }
        if (text.isEmpty()) return
        val tint = when (phase) {
            "up_to_date", "ready" -> palette.ok
            "error" -> palette.bad
            "available", "downloading", "verifying", "need_permission", "launching" -> palette.accent
            else -> palette.fgDim
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (phase == "checking" || phase == "downloading" || phase == "verifying") {
                CircularProgressIndicator(
                    color = tint,
                    strokeWidth = 2.dp,
                    modifier = Modifier.size(14.dp),
                )
                Spacer(Modifier.width(8.dp))
            }
            Text(text, fontSize = 13.sp, fontWeight = FontWeight.Medium, color = tint)
            if (phase == "up_to_date") {
                Spacer(Modifier.width(10.dp))
                Text(
                    "再检查",
                    fontSize = 12.sp,
                    color = palette.accent,
                    modifier = Modifier.clickable { check(manual = true) },
                )
            }
        }
    }

    @Composable
    private fun ReleaseCard(info: AppRelease) {
        val palette = LocalPalette.current
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(16.dp))
                .background(palette.card)
                .border(1.dp, palette.accent.copy(alpha = 0.35f), RoundedCornerShape(16.dp))
                .padding(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    "v${info.version}",
                    fontSize = 15.sp,
                    fontWeight = FontWeight.Bold,
                    color = palette.fg,
                )
                Spacer(Modifier.width(8.dp))
                if (info.publishedAt.isNotEmpty()) {
                    Text(
                        info.publishedAt.take(10),
                        fontSize = 11.sp,
                        fontFamily = FontFamily.Monospace,
                        color = palette.fgFaint,
                    )
                }
                Spacer(Modifier.weight(1f))
                if (info.apkSize > 0) {
                    Text(humanSize(info.apkSize), fontSize = 11.sp, color = palette.fgFaint)
                }
            }
            val notes = info.notes.trim()
            if (notes.isNotEmpty()) {
                Spacer(Modifier.height(10.dp))
                Text(
                    notes,
                    fontSize = 12.sp,
                    lineHeight = 19.sp,
                    color = palette.fgDim,
                    maxLines = 14,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }

    @Composable
    private fun ProgressBlock() {
        val palette = LocalPalette.current
        val fraction = if (total > 0) (downloaded.toFloat() / total).coerceIn(0f, 1f) else 0f
        val percent = (fraction * 100).toInt()
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(16.dp))
                .background(palette.card)
                .border(1.dp, palette.line, RoundedCornerShape(16.dp))
                .padding(16.dp),
        ) {
            LinearProgressIndicator(
                progress = { if (phase == "downloading") fraction else 1f },
                modifier = Modifier
                    .fillMaxWidth()
                    .height(8.dp)
                    .clip(RoundedCornerShape(999.dp)),
                color = palette.accent,
                trackColor = palette.elevate,
            )
            Spacer(Modifier.height(8.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    if (phase == "downloading") "$percent%" else "校验完成",
                    fontSize = 12.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = palette.fg,
                )
                Spacer(Modifier.width(10.dp))
                Text(
                    if (total > 0) "${humanSize(downloaded)} / ${humanSize(total)}" else humanSize(downloaded),
                    fontSize = 11.sp,
                    color = palette.fgFaint,
                )
                Spacer(Modifier.weight(1f))
                if (source.isNotEmpty()) {
                    Text("来源：$source", fontSize = 11.sp, color = palette.fgFaint)
                }
            }
        }
    }

    @Composable
    private fun ErrorBlock() {
        val palette = LocalPalette.current
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(palette.bad.copy(alpha = 0.12f))
                .border(1.dp, palette.bad.copy(alpha = 0.35f), RoundedCornerShape(12.dp))
                .padding(12.dp),
        ) {
            Text(error, fontSize = 12.sp, lineHeight = 19.sp, color = palette.bad)
        }
    }

    @Composable
    private fun Actions() {
        val palette = LocalPalette.current
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            when (phase) {
                "available" -> PrimaryAction("下载并安装", enabled = true) { download() }
                "downloading" -> PrimaryAction("下载中…", enabled = false) {}
                "verifying" -> PrimaryAction("校验中…", enabled = false) {}
                "ready" -> PrimaryAction("安装（交给系统安装器）", enabled = true) { install() }
                "need_permission" -> PrimaryAction("我已授权，继续安装", enabled = true) { install() }
                "launching" -> {
                    Text(
                        "系统安装器已打开，按提示完成后回到这里 —— 重新打开 AnyBoot Remote 即为新版本。",
                        fontSize = 12.sp,
                        lineHeight = 19.sp,
                        color = palette.fgDim,
                    )
                }
                "error" -> PrimaryAction("重试", enabled = true) {
                    if (installedFile != null && release != null) download() else check(manual = true)
                }
                "up_to_date" -> SecondaryAction("再检查一次") { check(manual = true) }
                "checking" -> PrimaryAction("检查中…", enabled = false) {}
                else -> PrimaryAction("检查更新", enabled = true) { check(manual = true) }
            }
            if (phase == "available" && release != null) {
                Text(
                    "会先下载到应用私有目录并校验 SHA-256，通过后才交给系统安装器。",
                    fontSize = 11.sp,
                    lineHeight = 17.sp,
                    color = palette.fgFaint,
                )
            }
        }
    }

    @Composable
    private fun PrimaryAction(text: String, enabled: Boolean, onClick: () -> Unit) {
        val palette = LocalPalette.current
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.Center,
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(if (enabled) palette.accent else palette.elevate)
                .clickable(enabled = enabled, onClick = onClick)
                .padding(vertical = 14.dp),
        ) {
            Text(
                text,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                color = if (enabled) palette.onAccent else palette.fgFaint,
            )
        }
    }

    @Composable
    private fun SecondaryAction(text: String, onClick: () -> Unit) {
        val palette = LocalPalette.current
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.Center,
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(Color.Transparent)
                .clickable(onClick = onClick)
                .padding(vertical = 12.dp),
        ) {
            Text(text, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = palette.accent)
        }
    }

    // ── 展示辅助 ───────────────────────────────────────────────────────────

    private fun showReleaseCard(): Boolean =
        phase in listOf("available", "downloading", "verifying", "ready", "need_permission", "launching")

    private fun showProgress(): Boolean =
        phase in listOf("downloading", "verifying", "ready", "need_permission", "launching")

    private fun humanAgo(seconds: Long): String {
        if (seconds <= 0L) return ""
        val now = System.currentTimeMillis() / 1000
        val diff = (now - seconds).coerceAtLeast(0)
        return when {
            diff < 60 -> "刚刚检查"
            diff < 3600 -> "${diff / 60} 分钟前检查"
            diff < 86400 -> "${diff / 3600} 小时前检查"
            else -> "${diff / 86400} 天前检查"
        }
    }
}
