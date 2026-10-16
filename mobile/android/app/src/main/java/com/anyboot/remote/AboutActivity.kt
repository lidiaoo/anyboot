package com.anyboot.remote

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/**
 * 关于页：App 信息 + GitHub 仓库卡片（★/⑂/Issues 实时拉 API，拉不到用缓存）
 * + Star 引导（打开仓库网页 —— App 不代点，GitHub 的规则）。
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
                // 标题栏
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier
                        .fillMaxWidth()
                        .background(palette.card)
                        .padding(8.dp),
                ) {
                    BarIcon("‹", "返回") { finish() }
                    Text(
                        "关于 AnyBoot",
                        fontSize = 16.sp,
                        color = palette.fg,
                        modifier = Modifier.weight(1f).padding(start = 8.dp),
                    )
                }

                Column(
                    Modifier
                        .fillMaxWidth()
                        .verticalScroll(rememberScrollState())
                        .padding(start = 20.dp, top = 24.dp, end = 20.dp, bottom = 16.dp),
                ) {
                    Text(
                        "AnyBoot 远程客户端",
                        fontSize = 22.sp,
                        fontWeight = FontWeight.Bold,
                        color = palette.fg,
                    )
                    Text(
                        text = "版本 " + versionName(),
                        fontSize = 13.sp,
                        color = palette.fgDim,
                        modifier = Modifier.padding(top = 4.dp, bottom = 20.dp),
                    )

                    // GitHub 仓库卡片
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(16.dp))
                            .background(palette.card)
                            .padding(16.dp),
                    ) {
                        Text("lidiaoo/anyboot", fontSize = 16.sp, fontWeight = FontWeight.Bold,
                            color = palette.accent)
                        Text(
                            "多系统 chroot 管理与 GRUB 引导修复工具 —— "
                                    + "一键进入各系统 / 修复引导，内置远程控制。",
                            fontSize = 13.sp,
                            color = palette.fgDim,
                            modifier = Modifier.padding(top = 6.dp, bottom = 10.dp),
                        )
                        Row {
                            StatText(statsLabel("stargazers_count", "Star"), Modifier.weight(1f))
                            StatText(statsLabel("forks_count", "Fork"), Modifier.weight(1f))
                            StatText(statsLabel("open_issues_count", "Issues"), Modifier.weight(1f))
                        }
                        Text(
                            "★ Star on GitHub",
                            color = Color.White,
                            fontSize = 15.sp,
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(top = 10.dp)
                                .clip(RoundedCornerShape(12.dp))
                                .background(Color(0xFF24292F))
                                .clickable { open(REPO_URL) }
                                .padding(vertical = 12.dp),
                        )
                        Text(
                            "点 Star 会打开 GitHub 网页，请在网页上点一下 —— 这是给开源作者最好的鼓励。",
                            fontSize = 12.sp,
                            color = palette.fgFaint,
                            modifier = Modifier.padding(top = 8.dp),
                        )
                    }

                    LinkRow("更新日志", REPO_URL + "/releases")
                    LinkRow("问题反馈（Issues）", REPO_URL + "/issues")
                    LinkRow("开源许可", REPO_URL + "/blob/main/LICENSE")
                }
            }
        }
    }

    @Composable
    private fun StatText(label: String, modifier: Modifier = Modifier) {
        Text(label, fontSize = 12.sp, color = LocalPalette.current.fgDim, modifier = modifier)
    }

    private fun statsLabel(key: String, suffix: String): String {
        return try {
            JSONObject(stats).optInt(key, 0).toString() + " " + suffix
        } catch (ignored: Exception) {
            "… $suffix"
        }
    }

    private fun versionName(): String {
        return try {
            packageManager.getPackageInfo(packageName, 0).versionName ?: "?"
        } catch (ignored: Exception) {
            "?"
        }
    }

    @Composable
    private fun LinkRow(label: String, url: String) {
        val palette = LocalPalette.current
        Text(
            "$label  ›",
            fontSize = 15.sp,
            color = palette.fg,
            modifier = Modifier
                .fillMaxWidth()
                .clickable { open(url) }
                .padding(horizontal = 4.dp, vertical = 14.dp),
        )
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
