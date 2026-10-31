package com.anyboot.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 检查更新纯逻辑的 JVM 单测（`./gradlew :app:testDebugUnitTest`）。
 *
 * 桌面端 `src-tauri/src/update.rs` 有同一批断言的 Rust 版本，两边口径必须一致：
 * 关键用例（解析失败 → 无更新、短哈希不算校验值、跨平台不拿别人的包）改一边要同步另一边。
 */
class UpdateCoreTest {

    @Test
    fun tagIsReadFromRedirectLocation() {
        assertEquals(
            "v0.2.1",
            UpdateCore.tagFromLocation(
                "https://github.com/lidiaoo/anyboot/releases/tag/v0.2.1"
            ),
        )
        // 带查询串 / 尾斜杠也要认
        assertEquals(
            "v0.2.1",
            UpdateCore.tagFromLocation("https://github.com/lidiaoo/anyboot/releases/tag/v0.2.1?x=1"),
        )
        // 认不出就 null —— 不猜、不回落
        assertNull(UpdateCore.tagFromLocation(null))
        assertNull(UpdateCore.tagFromLocation(""))
        assertNull(UpdateCore.tagFromLocation("https://github.com/lidiaoo/anyboot/releases"))
    }

    @Test
    fun versionParsingIsStrict() {
        assertEquals(listOf(0, 2, 1), UpdateCore.parseVersion("v0.2.1"))
        assertEquals(listOf(0, 2, 0), UpdateCore.parseVersion(" 0.2.0 "))
        assertEquals(listOf(1, 2, 0), UpdateCore.parseVersion("1.2"))
        assertNull(UpdateCore.parseVersion(""))
        assertNull(UpdateCore.parseVersion("latest"))
    }

    @Test
    fun newerComparisonOnlyGoesUp() {
        assertTrue(UpdateCore.isNewer("0.2.1", "0.2.0"))
        assertTrue(UpdateCore.isNewer("0.3.0", "0.2.9"))
        assertTrue(UpdateCore.isNewer("1.0.0", "0.9.9"))
        assertFalse(UpdateCore.isNewer("0.2.0", "0.2.0"))
        // 只升不降：服务端版本更低（回滚 / 手滑）绝不提示"更新"
        assertFalse(UpdateCore.isNewer("0.1.9", "0.2.0"))
        // 缺段补 0
        assertFalse(UpdateCore.isNewer("0.2", "0.2.0"))
        assertTrue(UpdateCore.isNewer("0.2.1", "0.2"))
        // 解析失败一律按无更新（不打扰用户，也不是回落）
        assertFalse(UpdateCore.isNewer("latest", "0.2.0"))
        assertFalse(UpdateCore.isNewer("0.2.1", "?"))
    }

    @Test
    fun apkNameMatchesReleaseContract() {
        assertEquals(listOf("AnyBoot_0.2.1_debug.apk"), UpdateCore.apkCandidates("v0.2.1"))
        assertTrue(UpdateCore.apkCandidates("").isEmpty())
        assertEquals(
            "https://github.com/lidiaoo/anyboot/releases/download/v0.2.1/AnyBoot_0.2.1_debug.apk",
            UpdateCore.assetUrl("v0.2.1", "AnyBoot_0.2.1_debug.apk"),
        )
        assertEquals(
            "https://github.com/lidiaoo/anyboot/releases/download/v0.2.1/SHA256SUMS.txt",
            UpdateCore.sumsUrl("v0.2.1"),
        )
    }

    @Test
    fun sumsOnlyAcceptRealHashes() {
        val hash = "a".repeat(64)
        val text = """
            $hash  AnyBoot_0.2.1_debug.apk
            ${"b".repeat(64)}  something-else.zip
        """.trimIndent()
        assertEquals(hash, UpdateCore.parseSums(text, "AnyBoot_0.2.1_debug.apk"))
        assertNull(UpdateCore.parseSums(text, "missing.apk"))
        // 短哈希不算数（防止把说明行误当校验值）
        assertNull(UpdateCore.parseSums("deadbeef  a.apk", "a.apk"))
        // 兼容 sha256sum 的二进制模式星号
        assertEquals(hash, UpdateCore.parseSums("$hash *a.apk", "a.apk"))
        // 非十六进制字符不算数
        assertNull(UpdateCore.parseSums("z".repeat(64) + "  a.apk", "a.apk"))
    }

    /**
     * 回归：v0.2.1 / v0.2.2 的安卓校验把**整份 SHA256SUMS.txt** 当成 expected 哈希，
     * 去跟 64 位实测值比 —— 比较永远为 false，于是直连 / ghproxy / ghfast 每个源都报
     * 「SHA-256 不匹配」。包本身是好的（官方发布与各下载源实测哈希完全一致），是比对写错了。
     * 这条测试盯住「取到的是文本，不是哈希」这个前提。
     */
    @Test
    fun sumsTextMustBeParsedNotComparedWhole() {
        val apkHash = "6bb2847625530f51b419a4622aac91ba97b8e3c755cb67ccf5a76744be2355ac"
        // 取 v0.2.2 真实 SHA256SUMS.txt 的前三行（目标行不在首行，正是踩坑场景）
        val fullText = """
            44395c81b8048ceb8f6e231a516f484e4a1876c1af2b65a8d9e7a6c19741fd18  AnyBoot_0.2.2_amd64.AppImage
            2cad215abbf75e3119164032353e9e613569ba4eb1bd2eba1016099c8b0d51ca  AnyBoot_0.2.2_amd64.deb
            $apkHash  AnyBoot_0.2.2_debug.apk
        """.trimIndent()

        assertEquals(apkHash, UpdateCore.parseSums(fullText, "AnyBoot_0.2.2_debug.apk"))
        // 全文 735 字节，单个哈希 64 字符：拿全文去比，永远不相等
        assertNotEquals(fullText, UpdateCore.parseSums(fullText, "AnyBoot_0.2.2_debug.apk"))
        assertNotEquals(apkHash, fullText.trim())
        // 未解析时「全文里包含该哈希」也不代表能用来比较
        assertTrue(fullText.contains(apkHash))
        assertNotEquals(apkHash, fullText)
    }

    @Test
    fun onlyGithubHostsAreTrusted() {
        assertTrue(UpdateCore.isTrustedUrl("https://github.com/lidiaoo/anyboot/releases"))
        assertTrue(UpdateCore.isTrustedUrl("https://objects.githubusercontent.com/x"))
        assertTrue(UpdateCore.isTrustedUrl("https://api.github.com/repos/x"))
        assertFalse(UpdateCore.isTrustedUrl("http://github.com/x"))
        assertFalse(UpdateCore.isTrustedUrl("https://github.com.evil.example/x"))
        assertFalse(UpdateCore.isTrustedUrl("https://evil.example/github.com/x"))
        assertFalse(UpdateCore.isTrustedUrl(""))
    }

    @Test
    fun mirrorOnlyRewritesHost() {
        val url = "https://github.com/lidiaoo/anyboot/releases/download/v0.2.1/a.apk"
        assertEquals(url, UpdateCore.applyPrefix("", url))
        assertEquals("https://ghfast.top/$url", UpdateCore.applyPrefix("https://ghfast.top/", url))
        // 直连排第一，其余是镜像回退
        assertEquals("直连", UpdateCore.downloadSources().first().first)
        assertEquals(3, UpdateCore.downloadSources().size)
        // gh-proxy 排第一顺位镜像：2026-11-01 实测它是唯一能返回「字节与官方一致」的源，
        // ghfast 当天 TLS 直接断开，降到末位当长尾（顺序反了会让每次更新都白等一次超时）
        assertEquals("ghproxy", UpdateCore.downloadSources()[1].first)
        assertEquals("ghfast", UpdateCore.downloadSources()[2].first)
    }

    @Test
    fun fileNameComesFromUrlPath() {
        assertEquals(
            "AnyBoot_0.2.1_debug.apk",
            UpdateCore.fileNameOf("https://github.com/a/b/AnyBoot_0.2.1_debug.apk?download=1"),
        )
    }
}
