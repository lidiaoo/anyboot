package com.anyboot.remote

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.FileProvider
import okhttp3.OkHttpClient
import okhttp3.Request
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.security.MessageDigest
import java.util.concurrent.TimeUnit

/** 一次检查的结果（已经选好本平台安装包） */
data class AppRelease(
    val tag: String,
    val version: String,
    val notes: String,
    val publishedAt: String,
    val apkName: String,
    val apkUrl: String,
    val apkSize: Long,
)

/** 下载 + 校验成功后的产物 */
data class DownloadOutcome(val file: File, val source: String)

/**
 * Android 端的检查更新 / 下载 / 校验 / 安装（口径与桌面端 update.rs 一致）。
 *
 * 所有方法都是**阻塞**的，调用方必须放在后台线程（UI 层统一用 Thread + runOnUiThread）。
 * 客户端不做任何覆盖逻辑：下载好校验通过后交给系统安装器，同包名覆盖升级由系统处理，
 * 应用数据保留。
 */
object AppUpdate {

    private const val PREFS = "anyboot_update"
    private const val KEY_SKIP = "skip_version"
    private const val KEY_CHECKED_AT = "checked_at"
    private const val KEY_LATEST = "latest_version"

    private val plain: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .connectTimeout(10, TimeUnit.SECONDS)
            .readTimeout(20, TimeUnit.SECONDS)
            .build()
    }

    /** 探测专用：**不跟随重定向**，才能从 `Location` 里读出真实 tag */
    private val probe: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .connectTimeout(10, TimeUnit.SECONDS)
            .readTimeout(20, TimeUnit.SECONDS)
            .followRedirects(false)
            .followSslRedirects(false)
            .build()
    }

    // ── 检查 ────────────────────────────────────────────────────────────────

    /**
     * 检查最新版本。失败抛 [IOException]（文案已带下一步），成功返回带安装包地址的结果。
     * 调用方自己决定「有新版本」还是「已最新」——这里不做判断，避免口径分散。
     */
    fun check(): AppRelease {
        val tag = probeTag()
        val version = UpdateCore.normalize(tag)
        val api = fetchApi()
        val notes = api?.optString("body").orEmpty()
        val publishedAt = api?.optString("published_at").orEmpty()
        val chosen = pickFromApi(api, version) ?: probeApk(tag, version)
        return AppRelease(
            tag = tag,
            version = version,
            notes = notes,
            publishedAt = publishedAt,
            apkName = chosen.first,
            apkUrl = chosen.second,
            apkSize = chosen.third,
        )
    }

    /** 主通道：`HEAD /releases/latest` → 302 `Location` → tag（零速率限制） */
    private fun probeTag(): String {
        val request = Request.Builder()
            .url(UpdateCore.LATEST_PAGE)
            .head()
            .header("User-Agent", "AnyBootRemote")
            .build()
        val location = try {
            probe.newCall(request).execute().use { response ->
                // 不跟随重定向时 code 是 301/302，Location 在响应头里
                response.header("Location") ?: response.header("location")
            }
        } catch (error: Exception) {
            throw IOException("探测最新版本失败：${error.message ?: "网络不可用"}")
        }
        val tag = UpdateCore.tagFromLocation(location)
            ?: throw IOException("探测最新版本失败：响应里没有可用的版本号")
        return tag
    }

    /** 补充通道：匿名 API（只用来取更新日志与 assets，403 时静默放弃） */
    private fun fetchApi(): JSONObject? {
        val request = Request.Builder()
            .url(UpdateCore.API_LATEST)
            .header("User-Agent", "AnyBootRemote")
            .header("Accept", "application/vnd.github+json")
            .build()
        return try {
            plain.newCall(request).execute().use { response ->
                if (!response.isSuccessful) return null
                val body = response.body?.string().orEmpty()
                if (body.isEmpty()) null else JSONObject(body)
            }
        } catch (ignored: Exception) {
            null
        }
    }

    /** API 的 assets 里按候选名挑（地址必须可信）；挑不到返回 null，交给命名规则兜底 */
    private fun pickFromApi(api: JSONObject?, version: String): Triple<String, String, Long>? {
        if (api == null) return null
        val assets = api.optJSONArray("assets") ?: return null
        for (wanted in UpdateCore.apkCandidates(version)) {
            for (index in 0 until assets.length()) {
                val item = assets.optJSONObject(index) ?: continue
                if (item.optString("name") != wanted) continue
                val url = item.optString("browser_download_url")
                if (!UpdateCore.isTrustedUrl(url)) continue
                return Triple(wanted, url, item.optLong("size", 0L))
            }
        }
        return null
    }

    /** API 不可用时的兜底：按固定资产名拼地址并 HEAD 探测存在性 */
    private fun probeApk(tag: String, version: String): Triple<String, String, Long> {
        val candidates = UpdateCore.apkCandidates(version)
        if (candidates.isEmpty()) throw IOException("版本号解析失败（$tag），无法确定安装包名")
        for (name in candidates) {
            val url = UpdateCore.assetUrl(tag, name)
            val request = Request.Builder()
                .url(url)
                .head()
                .header("User-Agent", "AnyBootRemote")
                .build()
            try {
                plain.newCall(request).execute().use { response ->
                    if (response.isSuccessful) {
                        return Triple(name, url, response.body?.contentLength() ?: 0L)
                    }
                }
            } catch (ignored: Exception) {
                // 这一个候选不通就试下一个
            }
        }
        throw IOException("这个版本还没有本机可用的安装包（$tag），可到 Releases 页手动下载")
    }

    // ── 下载 + 校验 ─────────────────────────────────────────────────────────

    /**
     * 下载 APK 并做 SHA-256 校验，逐源回退（直连 → 镜像）。
     * [onProgress] 在后台线程回调：(已下载字节, 总字节, 源名)。
     */
    fun download(
        context: Context,
        release: AppRelease,
        onProgress: (Long, Long, String) -> Unit = { _, _, _ -> },
    ): DownloadOutcome {
        if (!UpdateCore.isTrustedUrl(release.apkUrl)) {
            throw IOException("安装包地址不在可信域名内（只接受 github.com）")
        }
        val dir = File(context.filesDir, "update")
        dir.mkdirs()
        // 清掉上次的残留（旧版本包 / 半截临时文件），避免越攒越多
        dir.listFiles()?.forEach { it.deleteRecursively() }

        val failures = mutableListOf<String>()
        for ((label, prefix) in UpdateCore.downloadSources()) {
            // 同一个源试两次：国内链路上偶发的半截包 / 代理回网页，第二次常常就正常了。
            // 只重试一次是刻意的——真被劫持时重试也没用，别让用户干等。
            var lastError = ""
            for (attempt in 1..2) {
                val temp = File(dir, "${release.apkName}.downloading")
                val finalFile = File(dir, release.apkName)
                try {
                    val url = UpdateCore.applyPrefix(prefix, release.apkUrl)
                    val fetched = downloadTo(url, temp, release.apkSize, label, onProgress)
                    onProgress(fetched.bytes, fetched.bytes, label)

                    // 先把「下到的到底是不是这个包」查清楚，再花时间算 SHA-256：
                    // 代理塞回来的拦截页 / 半截包在这里就该被拦下，而不是报一句笼统的「校验失败」。
                    if (fetched.contentType.contains("text/html", ignoreCase = true)) {
                        throw IOException("这个源返回的是网页而不是安装包（网络被劫持或代理拦截）")
                    }
                    if (fetched.declared > 0 && fetched.bytes != fetched.declared) {
                        throw IOException(
                            "传输中断：只收到 ${humanSize(fetched.bytes)}，应为 ${humanSize(fetched.declared)}"
                        )
                    }
                    if (release.apkSize > 0 && fetched.bytes != release.apkSize) {
                        throw IOException(
                            "包大小不符：收到 ${humanSize(fetched.bytes)}，" +
                                "官方是 ${humanSize(release.apkSize)}（传输被中间环节改写）"
                        )
                    }

                    // 取回来的是整份 SHA256SUMS.txt 的**文本**（当前 735 字节 / 8 行），
                    // 必须按资产名解析出那一行的 64 位十六进制再比。
                    // v0.2.1 / v0.2.2 就是漏了这一步：把整份文本当 expected 去跟 64 位实测值比，
                    // 结果每个源都判「不匹配」——包本身是好的，是比对写错了。
                    val sumsText = fetchSumsText(release.tag, prefix)
                        ?: throw IOException("取不到 SHA256SUMS.txt")
                    val expected = UpdateCore.parseSums(sumsText, release.apkName)
                        ?: throw IOException(
                            "SHA256SUMS.txt 里没有 ${release.apkName}（文件可能被代理换成了网页）"
                        )
                    val actual = sha256(temp)
                    if (!actual.equals(expected, ignoreCase = true)) {
                        throw IOException(
                            "SHA-256 不匹配：已收到完整 ${humanSize(fetched.bytes)}，" +
                                "实测 ${actual.take(12)}… / 官方 ${expected.take(12)}…" +
                                "（文件在传输中被改写）"
                        )
                    }
                    if (!temp.renameTo(finalFile)) throw IOException("落盘失败（重命名临时文件）")
                    return DownloadOutcome(finalFile, label)
                } catch (error: Exception) {
                    temp.delete()
                    lastError = error.message ?: "下载失败"
                }
            }
            failures += "$label：$lastError"
        }
        throw IOException(
            "下载失败（已尝试 ${failures.size} 个源）：${failures.joinToString("；")}。" +
                "下一步：点「重试」重新下载；反复失败请换个网络或稍后再试。"
        )
    }

    /**
     * 取 `SHA256SUMS.txt` 的**原文**（不解析）。
     * 名字里带 Text 是刻意的：这里返回的不是哈希，拿它直接跟 SHA-256 比就是错的。
     */
    private fun fetchSumsText(tag: String, prefix: String): String? {
        val url = UpdateCore.applyPrefix(prefix, UpdateCore.sumsUrl(tag))
        val request = Request.Builder().url(url).header("User-Agent", "AnyBootRemote").build()
        return try {
            plain.newCall(request).execute().use { response ->
                if (!response.isSuccessful) return null
                val body = response.body?.string() ?: return null
                // 有镜像会把 404 渲染成网页返回 200，正文里全是 HTML
                if (body.contains("<html", ignoreCase = true)) return null
                body
            }
        } catch (ignored: Exception) {
            null
        }
    }

    /** 一次下载拿到的东西：实际字节数、服务端声明的长度、Content-Type（用来识别拦截页） */
    private data class Fetched(val bytes: Long, val declared: Long, val contentType: String)

    private fun downloadTo(
        url: String,
        target: File,
        knownSize: Long,
        label: String,
        onProgress: (Long, Long, String) -> Unit,
    ): Fetched {
        val request = Request.Builder().url(url).header("User-Agent", "AnyBootRemote").build()
        val response = plain.newCall(request).execute()
        response.use {
            if (!response.isSuccessful) {
                throw IOException("HTTP ${response.code}")
            }
            val body = response.body ?: throw IOException("响应为空")
            val declared = body.contentLength()
            val total = if (declared > 0) declared else knownSize
            val contentType = response.header("Content-Type").orEmpty()
            var written = 0L
            var lastTick = 0L
            var lastBytes = 0L
            body.byteStream().use { input ->
                target.outputStream().use { output ->
                    val buffer = ByteArray(256 * 1024)
                    while (true) {
                        val count = input.read(buffer)
                        if (count <= 0) break
                        output.write(buffer, 0, count)
                        written += count
                        val now = System.currentTimeMillis()
                        // 节流：120ms 或累计 512KB 才回调一次，别把 UI 线程刷爆
                        if (now - lastTick >= 120 || written - lastBytes >= 512 * 1024) {
                            lastTick = now
                            lastBytes = written
                            onProgress(written, total, label)
                        }
                    }
                    output.flush()
                }
            }
            onProgress(written, if (total > 0) total else written, label)
            if (written == 0L) throw IOException("下载到 0 字节")
            return Fetched(written, declared, contentType)
        }
    }

    /** 12.3 MB / 845 KB 这种给人看的说法（报错里带尺寸，才判断得出是半截还是被改写） */
    private fun humanSize(bytes: Long): String {
        if (bytes < 1024) return "$bytes B"
        val units = listOf("KB", "MB", "GB")
        var value = bytes.toDouble() / 1024
        var unit = 0
        while (value >= 1024 && unit < units.size - 1) {
            value /= 1024
            unit++
        }
        return String.format("%.1f %s", value, units[unit])
    }

    private fun sha256(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(256 * 1024)
            while (true) {
                val count = input.read(buffer)
                if (count <= 0) break
                digest.update(buffer, 0, count)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    // ── 安装 ────────────────────────────────────────────────────────────────

    /** Android 8+ 需要用户单独授予「安装未知应用」；24~25 默认允许 */
    fun canInstallPackages(context: Context): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.packageManager.canRequestPackageInstalls()
        } else {
            true
        }

    /** 跳到系统的「安装未知应用」授权页（返回后由调用方重新检查） */
    fun openInstallPermission(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val intent = Intent(
            Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
            Uri.parse("package:${context.packageName}"),
        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(intent)
        } catch (error: Exception) {
            context.toast("打不开授权页面：${error.message ?: "系统限制"}")
        }
    }

    /**
     * 交给系统安装器（ACTION_VIEW + application/vnd.android.package-archive）。
     * 必须走 FileProvider：Android 7+ 的 StrictMode 禁止把 `file://` 暴露给别的应用。
     */
    fun install(context: Context, file: File) {
        if (!file.isFile || file.length() <= 0) {
            throw IOException("安装包不存在或为空，请重新下载")
        }
        val uri = FileProvider.getUriForFile(
            context,
            "${context.packageName}.fileprovider",
            file,
        )
        val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, "application/vnd.android.package-archive")
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        context.startActivity(intent)
    }

    // ── 本地状态（跳过版本 / 上次检查时间） ────────────────────────────────

    fun currentVersion(context: Context): String = try {
        context.packageManager.getPackageInfo(context.packageName, 0).versionName ?: "?"
    } catch (ignored: Exception) {
        "?"
    }

    fun skippedVersion(context: Context): String =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY_SKIP, "").orEmpty()

    fun skipVersion(context: Context, version: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString(KEY_SKIP, version.trim())
            .apply()
    }

    fun rememberCheck(context: Context, version: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putLong(KEY_CHECKED_AT, System.currentTimeMillis() / 1000)
            .putString(KEY_LATEST, version)
            .apply()
    }

    /** 上次检查的 Unix 秒（0 = 没检查过） */
    fun lastCheckedAt(context: Context): Long =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getLong(KEY_CHECKED_AT, 0L)
}
