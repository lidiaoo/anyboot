package com.anyboot.remote

/**
 * 检查更新的纯逻辑（**不 import 任何 android.***，可跑 JVM 单测：`./gradlew :app:testDebugUnitTest`）。
 *
 * 口径与桌面端 `src-tauri/src/update.rs` 严格一致，改任何一边必须同步另一边：
 *  - 版本**严格只升不降**：解析不出、比较不成立都按「没有更新」处理，绝不拿另一份数据垫底；
 *  - 下载地址只信 github.com 自家域名；
 *  - 发布产物的**固定资产名**是硬契约（见 docs/检查更新-设计.md §6 与 update.rs 的 asset_candidates）；
 *  - SHA-256 对不上就不装。
 */
object UpdateCore {

    const val REPO = "lidiaoo/anyboot"

    const val LATEST_PAGE = "https://github.com/$REPO/releases/latest"
    const val API_LATEST = "https://api.github.com/repos/$REPO/releases/latest"
    const val DOWNLOAD_BASE = "https://github.com/$REPO/releases/download"

    /** 与桌面端 MIRROR_PREFIXES 同一批镜像（国内直连 GitHub 常常几 KB/s 甚至断流） */
    val MIRRORS: List<Pair<String, String>> = listOf(
        "ghfast" to "https://ghfast.top/",
        "ghproxy" to "https://gh-proxy.com/",
    )

    /** `v0.2.1` / ` 0.2.1 ` → `0.2.1` */
    fun normalize(raw: String): String = raw.trim().trimStart('v', 'V').trim()

    /**
     * 从 `Location: https://github.com/o/r/releases/tag/v0.2.1` 抠 tag。
     * 认不出就返回 null —— 不猜、不回落（桌面端 `tag_from_location` 同一实现）。
     */
    fun tagFromLocation(location: String?): String? {
        val text = location?.trim().orEmpty()
        if (text.isEmpty()) return null
        val marker = "/releases/tag/"
        val index = text.indexOf(marker)
        if (index < 0) return null
        val rest = text.substring(index + marker.length)
        val tag = rest.substringBefore('?').substringBefore('#').substringBefore('/').trim()
        return tag.ifEmpty { null }
    }

    /**
     * 解析 `major.minor[.patch]` → 数字数组。解析不出（空、非数字）返回 null。
     * 预发布后缀（`-beta.1`）在这里**不参与**比较：正式版优先，后缀只影响展示。
     */
    fun parseVersion(raw: String): List<Int>? {
        val text = normalize(raw)
        if (text.isEmpty()) return null
        val match = Regex("^(\\d+)(?:\\.(\\d+))?(?:\\.(\\d+))?").find(text) ?: return null
        val parts = match.groupValues.drop(1).map { group ->
            if (group.isEmpty()) 0 else group.toIntOrNull() ?: return null
        }
        return if (parts.isEmpty()) null else parts
    }

    /**
     * latest 是否比 current 新。任一侧解析失败 → false（按「没有更新」，不打扰用户）。
     * 逐段比较、缺段补 0：`0.2` == `0.2.0`，`0.2.1` > `0.2.0`。
     */
    fun isNewer(latest: String, current: String): Boolean {
        val a = parseVersion(latest) ?: return false
        val b = parseVersion(current) ?: return false
        val size = maxOf(a.size, b.size)
        for (i in 0 until size) {
            val left = a.getOrElse(i) { 0 }
            val right = b.getOrElse(i) { 0 }
            if (left != right) return left > right
        }
        return false
    }

    /**
     * 按发布的**固定资产名**给出候选 APK 名（有序）。
     * 发布 job（.github/workflows/build-all.yml 的 release）产出的就是第一个。
     */
    fun apkCandidates(version: String): List<String> {
        val v = normalize(version)
        if (v.isEmpty()) return emptyList()
        return listOf("AnyBoot_${v}_debug.apk")
    }

    /** 静态资源地址：`https://github.com/lidiaoo/anyboot/releases/download/v0.2.1/AnyBoot_0.2.1_debug.apk` */
    fun assetUrl(tag: String, name: String): String = "$DOWNLOAD_BASE/$tag/$name"

    /** 校验文件地址（发布 job 生成，与 APK 同目录） */
    fun sumsUrl(tag: String): String = "$DOWNLOAD_BASE/$tag/SHA256SUMS.txt"

    /**
     * 从 SHA256SUMS.txt 里取指定文件的哈希。
     * 只认 64 位十六进制——防止把说明行/短哈希误当校验值（桌面端 parse_sums 同规则）。
     */
    fun parseSums(text: String, wanted: String): String? {
        if (wanted.isEmpty()) return null
        for (line in text.lineSequence()) {
            val trimmed = line.trim()
            if (trimmed.isEmpty() || trimmed.startsWith("#")) continue
            // 兼容 `hash  name` / `hash *name` 两种格式
            val parts = trimmed.split(Regex("\\s+"), limit = 2)
            if (parts.size < 2) continue
            val hash = parts[0].lowercase()
            val name = parts[1].trim().removePrefix("*").trim()
            if (name != wanted) continue
            if (hash.length != 64 || hash.any { it !in "0123456789abcdef" }) return null
            return hash
        }
        return null
    }

    /** 只信 GitHub 自家域名：API 的下载地址 / 页面 / 对象存储 */
    fun isTrustedUrl(url: String): Boolean {
        if (!url.startsWith("https://")) return false
        val host = url.removePrefix("https://").substringBefore('/').substringBefore(':')
        return host == "github.com" ||
            host == "api.github.com" ||
            host.endsWith(".githubusercontent.com") ||
            host.endsWith(".github.com")
    }

    /** 镜像只改 host、不改路径（与桌面端 apply_prefix 同行为） */
    fun applyPrefix(prefix: String, url: String): String {
        if (prefix.isEmpty()) return url
        return if (prefix.endsWith("/")) prefix + url else "$prefix/$url"
    }

    /** 下载源顺序：直连优先，再逐个镜像回退 */
    fun downloadSources(): List<Pair<String, String>> =
        listOf("直连" to "") + MIRRORS

    /** 从下载地址取文件名（去掉查询串与百分号编码尾巴） */
    fun fileNameOf(url: String): String =
        url.substringBefore('?').substringBefore('#').substringAfterLast('/')
}
