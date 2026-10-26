package com.anyboot.remote;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/**
 * 配对幂等键 (mid, kind, lane) 的真机验证（instrumented test，跑在实机上）。
 *
 * <p>回归背景：幂等键一度漏了 lane —— ts 直连（100.x）与系统 Funnel（*.ts.net）同属
 * ts 区块，后扫的码会把先扫的整行顶掉。三处消费（upsertPair / RemotePair.key /
 * 编辑表单 applyRemoteHint）必须同一口径，lane 统一由 {@link RemotePair#laneFor} 算。
 */
public class PairUpsertTest {

    private static final String MID = "test-machine-01";

    private static final String DIRECT =
            "anyboot://pair?v=2&mid=" + MID + "&os=macos&host=TestMac&type=ts"
                    + "&tunnel=http://100.64.0.7:12300&token=direct-token-123";

    private static final String FUNNEL =
            "anyboot://pair?v=2&mid=" + MID + "&os=macos&host=TestMac&type=ts"
                    + "&tunnel=https://test-mac.tailc5a8efts.ts.net:12300&token=funnel-token-456";

    @Test
    public void sameMidTsDirectAndFunnelCoexist() {
        ServerProfile profile = new ServerProfile();
        RemotePair first = profile.addPairFromPaste(DIRECT);
        assertNotNull("直连串应解析成功", first);
        RemotePair second = profile.addPairFromPaste(FUNNEL);
        assertNotNull("Funnel 串应解析成功", second);

        assertEquals("同 mid 的直连与 Funnel 必须并存（2 条）", 2, profile.pairs.size());
        assertEquals(RemotePair.LANE_DIRECT, profile.pairs.get(0).effectiveLane());
        assertEquals(RemotePair.LANE_FUNNEL, profile.pairs.get(1).effectiveLane());
        assertEquals("direct-token-123", profile.pairs.get(0).token);
        assertEquals("funnel-token-456", profile.pairs.get(1).token);
    }

    @Test
    public void rescanSameLaneUpdatesInPlace() {
        ServerProfile profile = new ServerProfile();
        profile.addPairFromPaste(DIRECT);
        profile.addPairFromPaste(FUNNEL);

        // 换地址换令牌重扫直连：应更新第一条，不新增、不打乱顺序
        String rescanned = DIRECT.replace("100.64.0.7", "100.64.0.9")
                .replace("direct-token-123", "direct-token-xyz");
        RemotePair hit = profile.addPairFromPaste(rescanned);
        assertNotNull(hit);
        assertEquals(2, profile.pairs.size());
        assertEquals("http://100.64.0.9:12300", profile.pairs.get(0).addr);
        assertEquals("direct-token-xyz", profile.pairs.get(0).token);
        assertEquals("首配时间位置不变（更新而非追加）", firstIdOf(profile), hit.id);
    }

    @Test
    public void differentMidNeverMerges() {
        ServerProfile profile = new ServerProfile();
        profile.addPairFromPaste(DIRECT);
        String other = DIRECT.replace(MID, "another-machine-99");
        profile.addPairFromPaste(other);
        assertEquals("不同 mid 的同 lane 配对各自成条", 2, profile.pairs.size());
    }

    @Test
    public void legacyLaneEmptyDataStillMatches() {
        // 老数据 lane 写空（升级前落库的）：按地址形态现算 lane，重扫同 lane 才命中
        ServerProfile profile = new ServerProfile();
        RemotePair direct = profile.addPairFromPaste(DIRECT);
        direct.lane = "";
        RemotePair hit = profile.addPairFromPaste(
                DIRECT.replace("direct-token-123", "direct-token-new"));
        assertNotNull(hit);
        assertEquals("老 lane 空数据重扫同 lane = 更新那一条", 1, profile.pairs.size());
        assertEquals("direct-token-new", profile.pairs.get(0).token);
        assertEquals(RemotePair.LANE_DIRECT, profile.pairs.get(0).lane);
    }

    @Test
    public void keyIncludesLane() {
        ServerProfile profile = new ServerProfile();
        RemotePair direct = profile.addPairFromPaste(DIRECT);
        RemotePair funnel = profile.addPairFromPaste(FUNNEL);
        assertTrue("两条配对的幂等键必须不同", !direct.key().equals(funnel.key()));
    }

    private static String firstIdOf(ServerProfile profile) {
        return profile.pairs.get(0).id;
    }
}
