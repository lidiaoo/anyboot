import XCTest
@testable import AnyBootModel

/// 编辑表单「粘贴 / 扫码 → 落库」的契约测试 —— 移植 Android
/// `androidTest/java/com/anyboot/remote/ApplyRemoteHintTest.kt`。
///
/// 口径对应关系：
/// - Android `ServerEditFormState.applyRemoteHint()`（onValueChange 里分拣）
/// - iOS `ComputerEditorView.addPaste()`：`ServerProfile.parsePair` + `draft.upsertPair`
///
/// 扫码页（`ScanPairActivity` / `QRScannerView`）两端都只把串填回粘贴框，
/// 真正的落库走这条路径，所以这里测的就是「扫码之后会发生什么」。
///
/// 回归背景：这里的匹配一度按 (mid, kind) 找，漏了 lane —— 扫直连码再扫 Funnel 码
/// 会把直连整行顶掉（用户实测复现的 bug）。
final class ApplyRemoteHintTests: XCTestCase {

    private let direct = "anyboot://pair?v=2&mid=test-machine-01&os=macos&host=TestMac&type=ts"
        + "&tunnel=http://100.64.0.7:12300&token=direct-token-123"

    private let funnel = "anyboot://pair?v=2&mid=test-machine-01&os=macos&host=TestMac&type=ts"
        + "&tunnel=https://test-mac.tailc5a8efts.ts.net:12300&token=funnel-token-456"

    /// 与 `addPaste()` 同一条路径：解析成功才写库，失败不动现有配对
    @discardableResult
    private func paste(_ raw: String, into profile: inout ServerProfile) -> RemotePair? {
        let parsed = ServerProfile.parsePair(raw)
        guard parsed.ok else { return nil }
        return profile.upsertPair(parsed)
    }

    /// scanDirectThenFunnelAddsTwoRows
    func testScanDirectThenFunnelAddsTwoRows() {
        var profile = ServerProfile()

        XCTAssertNotNil(paste(direct, into: &profile), "直连串加入 1 条")
        XCTAssertEqual(profile.pairs.count, 1)

        XCTAssertNotNil(paste(funnel, into: &profile),
                        "同 mid 的 Funnel 码必须新增一条而不是顶掉直连")
        XCTAssertEqual(profile.pairs.count, 2)

        let ts = profile.pairs.filter { $0.kind == RemotePair.kindTS }
        XCTAssertEqual(ts.count, 2)
        let lanes = ts.map { RemotePair.laneFor(kind: $0.kind, addr: $0.addr) }.sorted()
        XCTAssertEqual(lanes, [RemotePair.laneDirect, RemotePair.laneFunnel])
    }

    /// rescanSameLaneUpdatesNotDuplicates
    func testRescanSameLaneUpdatesNotDuplicates() {
        var profile = ServerProfile()
        paste(direct, into: &profile)
        paste(funnel, into: &profile)

        paste(direct.replacingOccurrences(of: "direct-token-123", with: "direct-token-new"),
              into: &profile)
        XCTAssertEqual(profile.pairs.count, 2, "重扫同 lane 更新原行")
        XCTAssertEqual(profile.pairs[0].token, "direct-token-new")
    }
}
