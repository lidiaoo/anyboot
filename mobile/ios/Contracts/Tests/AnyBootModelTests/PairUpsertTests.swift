import XCTest
@testable import AnyBootModel

/// 配对幂等键 `(mid, kind, lane)` 的契约测试 —— 一比一移植 Android
/// `androidTest/java/com/anyboot/remote/PairUpsertTest.java`（5 个用例，逐条对应）。
///
/// 回归背景（两端同一个 bug）：幂等键一度漏了 lane —— ts 直连（100.x）与系统 Funnel
/// （`*.ts.net`）同属 ts 区块，后扫的码会把先扫的整行顶掉。三处消费
/// （`upsertPair` / `RemotePair.key` / 编辑表单的粘贴近路径）必须同一口径，
/// lane 统一由 `RemotePair.laneFor(kind:addr:)` 算。
final class PairUpsertTests: XCTestCase {

    private let mid = "test-machine-01"

    private var direct: String {
        "anyboot://pair?v=2&mid=\(mid)&os=macos&host=TestMac&type=ts"
            + "&tunnel=http://100.64.0.7:12300&token=direct-token-123"
    }

    private var funnel: String {
        "anyboot://pair?v=2&mid=\(mid)&os=macos&host=TestMac&type=ts"
            + "&tunnel=https://test-mac.tailc5a8efts.ts.net:12300&token=funnel-token-456"
    }

    /// sameMidTsDirectAndFunnelCoexist
    func testSameMidTsDirectAndFunnelCoexist() {
        var profile = ServerProfile()

        let first = profile.addPairFromPaste(direct)
        XCTAssertNotNil(first, "直连串应解析成功")
        let second = profile.addPairFromPaste(funnel)
        XCTAssertNotNil(second, "Funnel 串应解析成功")

        XCTAssertEqual(profile.pairs.count, 2, "同 mid 的直连与 Funnel 必须并存（2 条）")
        XCTAssertEqual(profile.pairs[0].effectiveLane, RemotePair.laneDirect)
        XCTAssertEqual(profile.pairs[1].effectiveLane, RemotePair.laneFunnel)
        XCTAssertEqual(profile.pairs[0].token, "direct-token-123")
        XCTAssertEqual(profile.pairs[1].token, "funnel-token-456")
    }

    /// rescanSameLaneUpdatesInPlace
    func testRescanSameLaneUpdatesInPlace() {
        var profile = ServerProfile()
        profile.addPairFromPaste(direct)
        profile.addPairFromPaste(funnel)

        // 换地址换令牌重扫直连：应更新第一条，不新增、不打乱顺序
        let rescanned = direct
            .replacingOccurrences(of: "100.64.0.7", with: "100.64.0.9")
            .replacingOccurrences(of: "direct-token-123", with: "direct-token-xyz")
        let hit = profile.addPairFromPaste(rescanned)
        XCTAssertNotNil(hit)
        XCTAssertEqual(profile.pairs.count, 2)
        XCTAssertEqual(profile.pairs[0].addr, "http://100.64.0.9:12300")
        XCTAssertEqual(profile.pairs[0].token, "direct-token-xyz")
        XCTAssertEqual(profile.pairs[0].id, hit?.id, "首配时间位置不变（更新而非追加）")
    }

    /// differentMidNeverMerges
    func testDifferentMidNeverMerges() {
        var profile = ServerProfile()
        profile.addPairFromPaste(direct)
        profile.addPairFromPaste(direct.replacingOccurrences(of: mid, with: "another-machine-99"))
        XCTAssertEqual(profile.pairs.count, 2, "不同 mid 的同 lane 配对各自成条")
    }

    /// legacyLaneEmptyDataStillMatches
    func testLegacyLaneEmptyDataStillMatches() {
        // 老数据 lane 写空（升级前落库的）：按地址形态现算 lane，重扫同 lane 才命中
        var profile = ServerProfile()
        profile.addPairFromPaste(direct)
        profile.pairs[0].lane = ""

        let hit = profile.addPairFromPaste(
            direct.replacingOccurrences(of: "direct-token-123", with: "direct-token-new"))
        XCTAssertNotNil(hit)
        XCTAssertEqual(profile.pairs.count, 1, "老 lane 空数据重扫同 lane = 更新那一条")
        XCTAssertEqual(profile.pairs[0].token, "direct-token-new")
        XCTAssertEqual(profile.pairs[0].lane, RemotePair.laneDirect)
    }

    /// keyIncludesLane
    func testKeyIncludesLane() {
        var profile = ServerProfile()
        let first = profile.addPairFromPaste(direct)
        let second = profile.addPairFromPaste(funnel)
        XCTAssertNotNil(first)
        XCTAssertNotNil(second)
        XCTAssertNotEqual(first?.key, second?.key, "两条配对的幂等键必须不同")
    }
}
