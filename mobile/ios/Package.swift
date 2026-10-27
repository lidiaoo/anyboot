// swift-tools-version:5.9
import PackageDescription

// 契约测试包（**不参与 App 构建**，只是为了能一条命令跑回归用例）。
//
// 为什么单独开包、而不是在 AnyBootRemote.xcodeproj 里加 unit-test target：
// 本机 Xcode 26.6 上，只要给工程加一个依赖 App 的 test target，xcodebuild 就会在
// 「预构建 SPM 包」阶段失灵，应用 target 直接报 `unable to resolve module dependency:
// 'Flow' / 'Crypto' / 'Citadel'…`（build 与 test 都挂）。模型层是纯 Foundation，
// 拉出来单独编译最稳，也不用起模拟器。
//
// 用法：cd mobile/ios && swift test
//
// 覆盖：androidTest/.../PairUpsertTest.java（5 例）
//      androidTest/.../ApplyRemoteHintTest.kt（2 例 + 1 条负向）
let package = Package(
    name: "AnyBootContracts",
    platforms: [.macOS(.v13)],
    targets: [
        // 直接编译 App 里的两个模型文件（不复制，避免口径漂移）
        .target(
            name: "AnyBootModel",
            path: "AnyBootRemote/Model",
            sources: [
                "RemotePair.swift",
                "ServerProfile.swift",
            ]
        ),
        .testTarget(
            name: "AnyBootModelTests",
            dependencies: ["AnyBootModel"],
            path: "Contracts/Tests/AnyBootModelTests"
        ),
    ]
)
