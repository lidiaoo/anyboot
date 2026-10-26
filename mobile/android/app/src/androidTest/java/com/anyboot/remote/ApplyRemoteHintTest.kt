package com.anyboot.remote

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

/**
 * 编辑表单「粘贴 / 扫码 → applyRemoteHint 分拣」的真机验证 —— 这是扫码实际走的落库路径
 * （ScanPairActivity 只把串填回粘贴框，onValueChange 触发 applyRemoteHint）。
 *
 * <p>回归背景：这里的匹配一度按 (mid, kind) 找，漏了 lane —— 扫直连码再扫 Funnel 码
 * 会把直连整行顶掉（用户实测复现的 bug）。
 */
@RunWith(AndroidJUnit4::class)
class ApplyRemoteHintTest {

    private val direct =
        "anyboot://pair?v=2&mid=test-machine-01&os=macos&host=TestMac&type=ts" +
            "&tunnel=http://100.64.0.7:12300&token=direct-token-123"

    private val funnel =
        "anyboot://pair?v=2&mid=test-machine-01&os=macos&host=TestMac&type=ts" +
            "&tunnel=https://test-mac.tailc5a8efts.ts.net:12300&token=funnel-token-456"

    @Test
    fun scanDirectThenFunnelAddsTwoRows() {
        val state = ServerEditFormState(null, ApplicationProvider.getApplicationContext())

        state.remoteText = direct
        state.applyRemoteHint()
        assertEquals("直连串加入 1 条", 1, state.pairItems.size)
        assertEquals("已加入", state.remoteHint.take(3))

        state.remoteText = funnel
        state.applyRemoteHint()
        assertEquals("同 mid 的 Funnel 码必须新增一条而不是顶掉直连", 2, state.pairItems.size)
        assertEquals(2, state.pairsOf(RemotePair.KIND_TS).size)
        val tsLanes = state.pairsOf(RemotePair.KIND_TS)
            .map { RemotePair.laneFor(it.kind, it.addr) }.sorted()
        assertEquals(listOf(RemotePair.LANE_DIRECT, RemotePair.LANE_FUNNEL), tsLanes)
    }

    @Test
    fun rescanSameLaneUpdatesNotDuplicates() {
        val state = ServerEditFormState(null, ApplicationProvider.getApplicationContext())
        state.remoteText = direct
        state.applyRemoteHint()
        state.remoteText = funnel
        state.applyRemoteHint()

        state.remoteText = direct.replace("direct-token-123", "direct-token-new")
        state.applyRemoteHint()
        assertEquals("重扫同 lane 更新原行", 2, state.pairItems.size)
        assertEquals("direct-token-new", state.pairItems[0].token)
    }
}
