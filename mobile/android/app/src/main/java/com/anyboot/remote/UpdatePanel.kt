package com.anyboot.remote

import androidx.activity.ComponentActivity
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.io.File

/**
 * 「关于」页里的检查更新区块（**没有独立页**，与 PC 的 ui/hub.html #page-about 同一套语义：
 * 版本卡上的胶囊点开 → 就地展开；收起后胶囊上仍能看到最后的状态）。
 *
 * 状态机与桌面端一致：idle → checking → up_to_date / available → downloading →
 * verifying → ready → launching → error。**客户端不做覆盖逻辑**：校验通过后把 APK
 * 交给系统安装器，同包名覆盖升级由系统处理，应用数据保留。
 *
 * 口径：版本严格只升不降（UpdateCore.isNewer 解析失败 → 无更新）；校验不过不装。
 */
class UpdateState(
    private val activity: ComponentActivity,
    private val currentVersion: String,
) {

    var phase by mutableStateOf("idle")
        private set
    var release by mutableStateOf<AppRelease?>(null)
        private set
    var error by mutableStateOf("")
        private set
    var downloaded by mutableStateOf(0L)
        private set
    var total by mutableStateOf(0L)
        private set
    var source by mutableStateOf("")
        private set
    var checkedAt by mutableStateOf(0L)
        private set

    private var installedFile by mutableStateOf<File?>(null)

    /** 版本卡上那枚胶囊的字：收起时也要能看出当前状态，别让用户为了看结果再点开一次 */
    fun capsuleLabel(): String = when (phase) {
        "checking" -> "检查中…"
        "up_to_date" -> "已是最新 v$currentVersion"
        "available" -> "发现新版本 v${release?.version ?: ""}"
        "downloading", "verifying" -> "正在下载 v${release?.version ?: ""}…"
        "ready", "need_permission", "launching" -> "v${release?.version ?: ""} 待安装"
        "error" -> "更新未完成"
        else -> "检查更新"
    }

    /** 展开时调一次：只有第一次展开才真的查，之后展开只是回看结果 */
    fun checkOnFirstOpen() {
        if (phase == "idle") check(manual = true)
    }

    fun check(manual: Boolean) {
        if (phase == "checking" || phase == "downloading") return
        phase = "checking"
        error = ""
        Thread({
            try {
                val found = AppUpdate.check()
                AppUpdate.rememberCheck(activity, found.version)
                activity.runOnUiThread {
                    release = found
                    checkedAt = AppUpdate.lastCheckedAt(activity)
                    phase = if (UpdateCore.isNewer(found.version, currentVersion)) "available" else "up_to_date"
                }
            } catch (failure: Exception) {
                activity.runOnUiThread {
                    phase = "error"
                    error = failure.message ?: "检查失败"
                }
            }
        }, "anyboot-update-check").start()
    }

    fun download() {
        val target = release ?: return
        if (phase == "downloading") return
        phase = "downloading"
        downloaded = 0
        total = target.apkSize
        source = ""
        error = ""
        Thread({
            try {
                val outcome = AppUpdate.download(activity, target) { done, size, label ->
                    activity.runOnUiThread {
                        downloaded = done
                        if (size > 0) total = size
                        source = label
                    }
                }
                activity.runOnUiThread {
                    installedFile = outcome.file
                    source = outcome.source
                    downloaded = total
                    phase = "ready"
                }
            } catch (failure: Exception) {
                activity.runOnUiThread {
                    phase = "error"
                    error = failure.message ?: "下载失败"
                }
            }
        }, "anyboot-update-download").start()
    }

    fun install() {
        val file = installedFile ?: return
        if (!AppUpdate.canInstallPackages(activity)) {
            // 第一次装会遇到这道门：把「为什么」和「去哪儿开」说清楚再跳系统授权页
            error = "Android 需要你允许「安装未知应用」才能装这个 APK。" +
                "下一步：在接下来的系统页面里打开「允许来自此来源」，然后回来再点安装。"
            phase = "need_permission"
            AppUpdate.openInstallPermission(activity)
            return
        }
        try {
            AppUpdate.install(activity, file)
            phase = "launching"
            error = ""
        } catch (failure: Exception) {
            phase = "error"
            error = failure.message ?: "无法启动系统安装器"
        }
    }

    /** 重试：已经下过包就重下，否则重新检查 */
    fun retry() {
        if (installedFile != null) download() else check(manual = true)
    }

    fun openDownloadPage() {
        val url = release?.let { "https://github.com/${UpdateCore.REPO}/releases/tag/${it.tag}" }
            ?: UpdateCore.LATEST_PAGE
        try {
            activity.startActivity(
                android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(url))
                    .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        } catch (failure: Exception) {
            error = "这台设备没有可打开网页的应用。地址：$url"
        }
    }

    fun checkedAgoText(): String {
        if (checkedAt <= 0L) return ""
        val diff = (System.currentTimeMillis() / 1000 - checkedAt).coerceAtLeast(0)
        return when {
            diff < 60 -> "刚刚检查"
            diff < 3600 -> "${diff / 60} 分钟前检查"
            diff < 86400 -> "${diff / 3600} 小时前检查"
            else -> "${diff / 86400} 天前检查"
        }
    }
}

/** 关于页里就地展开的更新区块（视觉同 PC 的更新卡：左对齐、细边框、无二级标题） */
@Composable
fun UpdatePanel(state: UpdateState, modifier: Modifier = Modifier) {
    val palette = LocalPalette.current
    Column(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(palette.card)
            .border(1.dp, palette.line, RoundedCornerShape(16.dp))
            .padding(16.dp),
    ) {
        StatusRow(state)
        val info = state.release
        if (info != null && state.phase in RELEASE_PHASES) {
            Spacer(Modifier.height(12.dp))
            ReleaseBlock(info)
        }
        if (state.phase in PROGRESS_PHASES) {
            Spacer(Modifier.height(12.dp))
            ProgressBlock(state)
        }
        if (state.error.isNotEmpty()) {
            Spacer(Modifier.height(12.dp))
            Text(
                state.error,
                fontSize = 12.sp,
                lineHeight = 19.sp,
                color = palette.bad,
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(10.dp))
                    .background(palette.bad.copy(alpha = 0.12f))
                    .padding(10.dp),
            )
        }
        Spacer(Modifier.height(14.dp))
        val actions = state.phase
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (actions == "available" || actions == "error") {
                PanelButton(
                    text = if (actions == "available") "下载并安装" else "重试",
                    primary = true,
                ) { if (actions == "available") state.download() else state.retry() }
            }
            if (actions == "ready" || actions == "need_permission") {
                PanelButton(text = "安装", primary = true) { state.install() }
            }
            if (actions == "up_to_date") {
                PanelButton(text = "再检查一次", primary = false) { state.check(manual = true) }
            }
            if (actions == "error") {
                Spacer(Modifier.width(10.dp))
                PanelButton(text = "浏览器下载", primary = false) { state.openDownloadPage() }
            }
        }
        Spacer(Modifier.height(10.dp))
        Text(
            "安装由系统安装器完成：同包名会覆盖升级，服务器 / 密钥 / 设置都保留。" +
                "首次安装需要允许「安装未知应用」。",
            fontSize = 11.sp,
            lineHeight = 17.sp,
            color = palette.fgFaint,
        )
    }
}

private val RELEASE_PHASES = listOf(
    "available", "downloading", "verifying", "ready", "need_permission", "launching",
)

private val PROGRESS_PHASES = listOf(
    "downloading", "verifying", "ready", "need_permission", "launching",
)

@Composable
private fun StatusRow(state: UpdateState) {
    val palette = LocalPalette.current
    val text = when (state.phase) {
        "checking" -> "正在检查新版本…"
        "up_to_date" -> "已是最新版本"
        "available" -> "发现新版本 v${state.release?.version ?: ""}"
        "downloading" -> "正在下载 v${state.release?.version ?: ""}…"
        "verifying" -> "正在校验 SHA-256…"
        "ready" -> "下载完成，SHA-256 校验通过"
        "need_permission" -> "还需要一次系统授权"
        "launching" -> "已交给系统安装器"
        "error" -> "更新没有完成"
        else -> "还没检查过"
    }
    val tint = when (state.phase) {
        "up_to_date", "ready" -> palette.ok
        "error" -> palette.bad
        "available", "downloading", "verifying", "need_permission", "launching" -> palette.accent
        else -> palette.fgDim
    }
    Column {
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (state.phase in listOf("checking", "downloading", "verifying")) {
                CircularProgressIndicator(
                    color = tint,
                    strokeWidth = 2.dp,
                    modifier = Modifier.size(13.dp),
                )
                Spacer(Modifier.width(8.dp))
            }
            Text(text, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, color = tint)
        }
        val whenText = state.checkedAgoText()
        if (whenText.isNotEmpty()) {
            Text(
                whenText,
                fontSize = 11.sp,
                color = palette.fgFaint,
                modifier = Modifier.padding(top = 4.dp),
            )
        }
    }
}

@Composable
private fun ReleaseBlock(info: AppRelease) {
    val palette = LocalPalette.current
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(palette.elevate.copy(alpha = 0.55f))
            .padding(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                "v${info.version}",
                fontSize = 14.sp,
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
            Text(
                notes,
                fontSize = 12.sp,
                lineHeight = 18.sp,
                color = palette.fgDim,
                maxLines = 6,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(top = 8.dp),
            )
        }
    }
}

@Composable
private fun ProgressBlock(state: UpdateState) {
    val palette = LocalPalette.current
    val fraction = if (state.total > 0) {
        (state.downloaded.toFloat() / state.total).coerceIn(0f, 1f)
    } else 0f
    Column(Modifier.fillMaxWidth()) {
        LinearProgressIndicator(
            progress = { if (state.phase == "downloading") fraction else 1f },
            modifier = Modifier
                .fillMaxWidth()
                .height(6.dp)
                .clip(RoundedCornerShape(999.dp)),
            color = palette.accent,
            trackColor = palette.elevate,
        )
        Spacer(Modifier.height(6.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                if (state.phase == "downloading") "${(fraction * 100).toInt()}%" else "校验完成",
                fontSize = 11.sp,
                fontWeight = FontWeight.SemiBold,
                color = palette.fg,
            )
            Spacer(Modifier.width(8.dp))
            Text(
                if (state.total > 0) {
                    "${humanSize(state.downloaded)} / ${humanSize(state.total)}"
                } else {
                    humanSize(state.downloaded)
                },
                fontSize = 11.sp,
                color = palette.fgFaint,
            )
            Spacer(Modifier.weight(1f))
            if (state.source.isNotEmpty()) {
                Text("来源：${state.source}", fontSize = 11.sp, color = palette.fgFaint)
            }
        }
    }
}

@Composable
private fun PanelButton(text: String, primary: Boolean, onClick: () -> Unit) {
    val palette = LocalPalette.current
    Box(
        Modifier
            .clip(RoundedCornerShape(10.dp))
            .background(if (primary) palette.accent else palette.elevate)
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 9.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            fontSize = 13.sp,
            fontWeight = FontWeight.SemiBold,
            color = if (primary) palette.onAccent else palette.fg,
        )
    }
}
