package com.anyboot.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
    }

    @Test
    fun fileNameComesFromUrlPath() {
        assertEquals(
            "AnyBoot_0.2.1_debug.apk",
            UpdateCore.fileNameOf("https://github.com/a/b/AnyBoot_0.2.1_debug.apk?download=1"),
        )
    }
}
