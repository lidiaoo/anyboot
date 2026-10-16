package com.anyboot.remote

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Star
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/**
 * 关于页（对齐设计稿 about 屏）：版本卡 + GitHub 仓库卡（Star 数实时拉 API，拉不到用缓存）
 * + 链接卡（更新日志 / 问题反馈 / 开源许可）+ mono 署名。
 * Star 引导照旧：点卡片打开仓库网页 —— App 不代点，GitHub 的规则。
 */
class AboutActivity : ComponentActivity() {

    companion object {
        private const val REPO_URL = "https://github.com/lidiaoo/anyboot"
        private const val REPO_API = "https://api.github.com/repos/lidiaoo/anyboot"
    }

    private var stats by mutableStateOf("")

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Ui.applySystemBars(this)
        val cached = JumpStore.githubCache(this)
        if (cached.isNotEmpty()) stats = cached
        setContent { Root() }
        loadRepoStats()
    }

    @Composable
    private fun Root() {
        AppTheme {
            val palette = LocalPalette.current
            Column(Modifier.fillMaxSize().background(palette.base)) {
                // 标题（设计稿：大标题直接坐在页面底色上）
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(palette.base)
                        .padding(start = 4.dp, top = 10.dp, bottom = 8.dp),
                ) {
                    BarIcon("‹", "返回") { finish() }
                    Text("关于", fontSize = 20.sp, fontWeight = FontWeight.Bold, color = palette.fg)
                }

                Column(
                    Modifier
                        .fillMaxWidth()
                        .verticalScroll(rememberScrollState())
                        .padding(start = 20.dp, top = 8.dp, end = 20.dp, bottom = 24.dp),
                ) {
                    // ---- 版本卡 ----
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        modifier = Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(16.dp))
                            .background(palette.card)
                            .border(1.dp, palette.line, RoundedCornerShape(16.dp))
                            .padding(vertical = 24.dp, horizontal = 16.dp),
                    ) {
                        Box(
                            Modifier
                                .size(64.dp)
                                .clip(RoundedCornerShape(24.dp))
                                .background(palette.accent.copy(alpha = 0.15f)),
                            contentAlignment = Alignment.Center,
                        ) { DesktopIcon(palette.accent, 36.dp) }
                        Text(
                            "AnyBoot Remote",
                            fontSize = 18.sp,
                            fontWeight = FontWeight.Bold,
                            color = palette.fg,
                            modifier = Modifier.padding(top = 12.dp),
                        )
                        Text(
                            "v" + versionName() + " · 多系统启动管理器",
                            fontSize = 12.sp,
                            fontFamily = FontFamily.Monospace,
                            color = palette.fgFaint,
                            modifier = Modifier.padding(top = 2.dp),
                        )
                        Text(
                            "多系统一键切换 · GRUB 管理 · 远程电源控制 · SSH 跳板机隧道",
                            fontSize = 12.sp,
                            color = palette.fgDim,
                            textAlign = TextAlign.Center,
                            lineHeight = 19.sp,
                            modifier = Modifier.padding(top = 12.dp),
                        )
                    }
                    Spacer(Modifier.height(16.dp))

                    // ---- GitHub 仓库卡 ----
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(16.dp))
                            .background(palette.card)
                            .border(1.dp, palette.line, RoundedCornerShape(16.dp))
                            .clickable { open(REPO_URL) }
                            .padding(16.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            CodeIcon(palette.fg, 22.dp)
                            Spacer(Modifier.width(12.dp))
                            Column(Modifier.weight(1f)) {
                                Text(
                                    "GitHub · lidiaoo/anyboot",
                                    fontSize = 14.sp,
                                    fontWeight = FontWeight.SemiBold,
                                    color = palette.fg,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                                Text(
                                    "开源项目主页",
                                    fontSize = 11.sp,
                                    color = palette.fgDim,
                                    modifier = Modifier.padding(top = 1.dp),
                                )
                            }
                            // Star 徽标：实时拉到的 star 数（拉不到显示「Star」）
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                modifier = Modifier
                                    .clip(RoundedCornerShape(999.dp))
                                    .background(palette.warn.copy(alpha = 0.1f))
                                    .padding(horizontal = 10.dp, vertical = 4.dp),
                            ) {
                                Icon(
                                    Icons.Filled.Star,
                                    contentDescription = "Star",
                                    tint = palette.warn,
                                    modifier = Modifier.size(12.dp),
                                )
                                Text(
                                    " " + statsLabel("stargazers_count", ""),
                                    fontSize = 11.sp,
                                    fontWeight = FontWeight.SemiBold,
                                    color = palette.warn,
                                )
                            }
                            Text(" ›", fontSize = 14.sp, color = palette.fgFaint)
                        }
                        // Star / Fork / Issues 三格统计（真实 API 数据）
                        Spacer(Modifier.height(12.dp))
                        Row(Modifier.fillMaxWidth()) {
                            StatText(statsLabel("stargazers_count", "Star"), Modifier.weight(1f))
                            StatText(statsLabel("forks_count", "Fork"), Modifier.weight(1f))
                            StatText(statsLabel("open_issues_count", "Issues"), Modifier.weight(1f))
                        }
                        Text(
                            "点 Star 会打开 GitHub 网页，请在网页上点一下 —— 这是给开源作者最好的鼓励。",
                            fontSize = 11.sp,
                            color = palette.fgFaint,
                            lineHeight = 16.sp,
                            modifier = Modifier.padding(top = 10.dp),
                        )
                    }
                    Spacer(Modifier.height(16.dp))

                    // ---- 链接卡：三条，行间细分隔线 ----
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(16.dp))
                            .background(palette.card)
                            .border(1.dp, palette.line, RoundedCornerShape(16.dp)),
                    ) {
                        LinkRow("更新日志", REPO_URL + "/releases")
                        LinkDivider()
                        LinkRow("问题反馈（Issues）", REPO_URL + "/issues")
                        LinkDivider()
                        LinkRow("开源许可", REPO_URL + "/blob/main/LICENSE", suffix = "MIT")
                    }

                    Text(
                        "Made with AnyBoot · 2026",
                        fontSize = 10.sp,
                        fontFamily = FontFamily.Monospace,
                        color = palette.fgFaint,
                        textAlign = TextAlign.Center,
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 20.dp),
                    )
                }
            }
        }
    }

    @Composable
    private fun LinkDivider() {
        val palette = LocalPalette.current
        Box(
            Modifier
                .fillMaxWidth()
                .height(1.dp)
                .background(palette.line)
        )
    }

    @Composable
    private fun StatText(label: String, modifier: Modifier = Modifier) {
        Text(label, fontSize = 12.sp, color = LocalPalette.current.fgDim, modifier = modifier)
    }

    private fun statsLabel(key: String, suffix: String): String {
        return try {
            val value = JSONObject(stats).optInt(key, 0).toString()
            if (suffix.isEmpty()) value else "$value $suffix"
        } catch (ignored: Exception) {
            if (suffix.isEmpty()) "…" else "… $suffix"
        }
    }

    private fun versionName(): String {
        return try {
            packageManager.getPackageInfo(packageName, 0).versionName ?: "?"
        } catch (ignored: Exception) {
            "?"
        }
    }

    /** 链接行：图标 + 文字 +（可选 mono 后缀）+ ›（设计稿链接卡样式） */
    @Composable
    private fun LinkRow(label: String, url: String, suffix: String = "") {
        val palette = LocalPalette.current
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .clickable { open(url) }
                .padding(horizontal = 16.dp, vertical = 14.dp),
        ) {
            when {
                label.startsWith("更新日志") -> ClockIcon(palette.fgDim, 18.dp)
                label.startsWith("问题反馈") -> CodeIcon(palette.fgDim, 18.dp)
                else -> DocIconForLicense(palette.fgDim)
            }
            Spacer(Modifier.width(12.dp))
            Text(label, fontSize = 14.sp, color = palette.fg, modifier = Modifier.weight(1f))
            if (suffix.isNotEmpty()) {
                Text(
                    suffix,
                    fontSize = 11.sp,
                    fontFamily = FontFamily.Monospace,
                    color = palette.fgFaint,
                )
            }
            Text(" ›", fontSize = 14.sp, color = palette.fgFaint)
        }
    }

    @Composable
    private fun DocIconForLicense(tint: Color) {
        // 开源许可行：文档图标（设计稿用 doc 线性图标）
        CanvasDoc(tint)
    }

    @Composable
    private fun CanvasDoc(tint: Color) {
        androidx.compose.foundation.Canvas(Modifier.size(18.dp)) {
            val s = this.size.minDimension / 24f
            val path = androidx.compose.ui.graphics.Path().apply {
                moveTo(6f * s, 2.5f * s); lineTo(13.5f * s, 2.5f * s)
                lineTo(20f * s, 9f * s); lineTo(20f * s, 20f * s)
                lineTo(6f * s, 20f * s); close()
            }
            drawPath(path, tint, style = androidx.compose.ui.graphics.drawscope.Stroke(
                1.8f * s, cap = androidx.compose.ui.graphics.StrokeCap.Round))
            drawLine(tint, Offset(13.5f * s, 2.5f * s), Offset(13.5f * s, 9f * s), 1.8f * s)
            drawLine(tint, Offset(13.5f * s, 9f * s), Offset(20f * s, 9f * s), 1.8f * s)
            drawLine(tint, Offset(9f * s, 14f * s), Offset(15f * s, 14f * s), 1.8f * s)
        }
    }

    /** ★/⑂/Issues 实时拉 GitHub API；拉不到用上次缓存 */
    private fun loadRepoStats() {
        Thread({
            try {
                val connection = java.net.URL(REPO_API).openConnection()
                        as java.net.HttpURLConnection
                connection.setRequestProperty("Accept", "application/vnd.github+json")
                connection.connectTimeout = 5000
                connection.readTimeout = 5000
                if (connection.responseCode == 200) {
                    connection.inputStream.use { input ->
                        val buffer = java.io.ByteArrayOutputStream()
                        val chunk = ByteArray(4096)
                        var count: Int
                        while (input.read(chunk).also { count = it } > 0) {
                            buffer.write(chunk, 0, count)
                        }
                        val json = buffer.toString("UTF-8")
                        JumpStore.setGithubCache(this, json)
                        runOnUiThread { stats = json }
                    }
                }
            } catch (ignored: Exception) {
                // 拉不到就维持缓存值 / 占位符
            }
        }, "anyboot-github").start()
    }

    private fun open(url: String) {
        try {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
        } catch (ignored: Exception) {
            toast("没有可用的浏览器")
        }
    }
}
