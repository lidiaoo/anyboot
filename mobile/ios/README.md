# AnyBoot 远程 · iOS 客户端

和 Android 侧完全同构：**一批 WKWebView 外壳**，每个服务端标签一个，加载电脑端 AnyBoot 内置
HTTP 服务（默认 `12300`）托管的同一份前端页面。业务逻辑一行都不在这里——它只负责记住服务端
列表、记住 token、连不上时给个重试面板。

所以 iOS 端不需要、也不应该去编译 Rust 核心；服务端永远是 Windows / Linux / macOS 上的 AnyBoot。

## 多服务端 / 分组

| 能力 | 说明 |
| --- | --- |
| 同时连多个服务端 | 每个标签一个独立 `WKWebView`（用 `opacity` + `allowsHitTesting` 切换显隐，实例一直活着）；菜单里「启动时连接全部服务端」可让它们一起在线 |
| 顶部标签快速切换 | 标签栏横向滚动，点一下即切，已加载过的标签不重新加载 |
| 标签名 = 组名 ＋ 服务端名 | 未分组时只显示服务端名；名字留空则用地址兜底 |
| 分组 | 用来**区分不同的电脑**：一台电脑多块网卡 / 多次 DHCP 拿到不同 IP 时，这几条地址填同一个分组 |
| 分组配色 | 同一分组标签描边同色（组名哈希定色相，恒定不变） |
| 在线状态点 | 标签左侧小圆点：灰 / 橙（加载中）/ 绿 / 红 |
| 增删改 | `＋` 添加、**长按标签**编辑、`⋯` 菜单或列表页切换删除；列表页支持左滑编辑/删除 |
| 1.0 升级 | 旧版单服务端 `host`/`token` 自动迁移成列表第一条 |

## 文件

| 文件 | 作用 |
| --- | --- |
| `AnyBootRemote/AnyBootRemoteApp.swift` | `@main` 入口 |
| `AnyBootRemote/ContentView.swift` | 标签栏、多 WebView 容器、错误面板、增删改表单 |
| `AnyBootRemote/ServerProfile.swift` | 一条服务端记录 + 标签文本/排序/**分组配色** + 地址归一化 |
| `AnyBootRemote/ServerStore.swift` | `UserDefaults`(JSON) 持久化 + 旧版迁移 |
| `AnyBootRemote/WebView.swift` | `WKWebView` 的 `UIViewRepresentable` 封装 + 加载状态回调 |
| `Info.plist` | ATS 明文放行 + 本地网络用途说明（**不能少**） |

## 怎么建工程（Windows 上做不了，需要在 Mac 上操作）

1. Xcode → **File ▸ New ▸ Project ▸ iOS ▸ App**；
   Product Name 填 `AnyBootRemote`，Interface 选 **SwiftUI**，Language 选 **Swift**。
2. 删除模板生成的 `ContentView.swift` 和 `XXXApp.swift`，把本目录 `AnyBootRemote/` 下的
   **5 个** `.swift` 拖进工程（勾选 *Copy items if needed*、目标 target 打勾）。
3. 选中 target ▸ **Info**，加上这两项（内容照 `../ios/Info.plist`）：
   - `App Transport Security Settings` → `Allow Arbitrary Loads` = YES
   - `Privacy - Local Network Usage Description` = 上面那段说明文字
4. 真机运行：target ▸ Signing & Capabilities ▸ 勾 *Automatically manage signing*，
   Team 选自己的 Apple ID（免费账号即可，App 7 天要重签一次）。
   模拟器直接 ⌘R 也能跑，模拟器走宿主网络，连局域网 IP 没问题。

> 想用 XcodeGen 就不用手点：把 `AnyBootRemote/` 五个 swift + `Info.plist` 交给它，
> `INFOPLIST_FILE` 指向 `Info.plist` 即可。

## 用法

1. 电脑上打开 AnyBoot，「基本设置 ▸ 远程控制」**打开**并使端口为 `12300`（或你改过的端口）。
2. 确认手机与电脑在同一局域网（手机连的路由器 / 热点要和电脑一致）。
3. 首次启动 App 会弹出表单：
   - **分组**：例如「书房台式机」；同一台电脑的多个 IP 填同一个分组。
   - **服务端名**：例如「有线网卡 / 5G WiFi」（可留空，用地址兜底）。
   - **服务端地址**：形如 `192.168.0.120:12300`；也可以把整条带 token 的 URL 粘进来，自动拆出 token。
   - **访问令牌 token**：可留空——留空时页面自己会弹输入框要 token。
4. 点标签栏 `＋` 继续添加第二台电脑；长按标签改这一条。
5. 左上「重载」刷新当前页；右上 `⋯` 里有「添加 / 编辑 / 列表 / 全部重载 / 启动时连接全部服务端」。

## 注意

- 本机（Windows）**无法构建** iOS：没有 Xcode。这里交付的是可直接拖进工程的源码。
- 服务端是明文 HTTP，仅限可信局域网使用；不要把 `12300` 直接暴露到公网。
- 页面里的 `?token=` 会在验证通过后写进 cookie，所以地址栏里带的 token 不会长期暴露在 URL 上。
