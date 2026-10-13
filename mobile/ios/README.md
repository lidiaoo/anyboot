# AnyBoot 远程 · iOS 客户端

和 Android 侧完全同构：**一批 WKWebView 外壳**，每台电脑一个，加载电脑端 AnyBoot 内置
HTTP 服务（默认 `12300`）托管的同一份前端页面。业务逻辑一行都不在这里——它只负责记住电脑
列表、记住 token、连不上时给个重试面板。

所以 iOS 端不需要、也不应该去编译 Rust 核心；服务端永远是 Windows / Linux / macOS 上的 AnyBoot。

## 多电脑 / 多网卡

**一条记录 = 一台电脑**；一条记录内部可以放多个候选地址（多块网卡 / 换过 DHCP）。

| 能力 | 说明 |
| --- | --- |
| 同时连多台电脑 | 每台电脑一个 `WKWebView`（用 `opacity` + `allowsHitTesting` 切换显隐，实例一直活着）；菜单里「启动时连接全部服务端」可让它们一起在线 |
| 顶部标签快速切换 | 标签栏横向滚动，点一下即切，已加载过的标签不重新加载 |
| 一台电脑一个标签 | 标签数 = 电脑数（不是 IP 数）。多块网卡是**同一条记录里的多个候选地址** |
| 多地址自动切换 | 地址框支持**多行**（一行一个）。首选地址不通就自动换下一个 —— 还是同一个标签、同一个 `WKWebView`（换的是 URL，不是实例） |
| 记住可用地址 | 哪个地址连通就把它提到最前，下次直连它（DHCP 换过 IP 也能自己找回来） |
| 全试完才报错 | 候选地址全试过才显示重试面板，面板里把试过的地址列出来 |
| 自动归并 | 保存地址后探一次 `GET /health`（免 token）拿 `machineId`：和已有记录相同就**并进那条记录**；拿不到标识时退回「同分组名 = 同一台电脑」 |
| 一键合并老配置 | 服务端列表页左上角**「合并同机地址」** |
| 标签名 = 组名 ＋ 服务端名 | 未分组时只显示服务端名；名字留空则用地址兜底 |
| 分组配色 | 同一分组标签描边同色（组名哈希定色相，恒定不变） |
| 在线状态点 | 标签左侧小圆点：灰 / 橙（加载中）/ 绿 / 红 |
| 增删改 | `＋` 添加、**长按标签**编辑、`⋯` 菜单或列表页切换删除；列表页支持左滑编辑/删除 |
| 1.0 / 1.1 升级 | 旧版单服务端 `host`/`token` 自动迁移成列表第一条；旧版只有单个 `host` 的条目会升级成「只有一个候选地址」的记录 |

## 文件

| 文件 | 作用 |
| --- | --- |
| `AnyBootRemote/AnyBootRemoteApp.swift` | `@main` 入口 |
| `AnyBootRemote/ContentView.swift` | 标签栏、多 WebView 容器、多地址切换、错误面板、增删改表单 |
| `AnyBootRemote/ServerProfile.swift` | 一条记录（= 一台电脑）：地址列表 + 标签文本/排序/**分组配色** + 地址归一化 + 同机判定 |
| `AnyBootRemote/ServerStore.swift` | `UserDefaults`(JSON) 持久化 + 旧版迁移 + 一键合并同机记录 + 传输方式偏好 |
| `AnyBootRemote/WebView.swift` | `WKWebView` 的 `UIViewRepresentable` 封装 + 加载状态回调 |
| `AnyBootRemote/HealthProbe.swift` | 探 `/health` 拿 `machineId`（自动归并同一台电脑） |
| `Info.plist` | ATS 明文放行 + 本地网络用途说明（**不能少**） |

## 怎么建工程（Windows 上做不了，需要在 Mac 上操作）

1. Xcode → **File ▸ New ▸ Project ▸ iOS ▸ App**；
   Product Name 填 `AnyBootRemote`，Interface 选 **SwiftUI**，Language 选 **Swift**。
2. 删除模板生成的 `ContentView.swift` 和 `XXXApp.swift`，把本目录 `AnyBootRemote/` 下的
   **6 个** `.swift` 拖进工程（勾选 *Copy items if needed*、目标 target 打勾）。
3. 选中 target ▸ **Info**，加上这两项（内容照 `../ios/Info.plist`）：
   - `App Transport Security Settings` → `Allow Arbitrary Loads` = YES
   - `Privacy - Local Network Usage Description` = 上面那段说明文字
4. 真机运行：target ▸ Signing & Capabilities ▸ 勾 *Automatically manage signing*，
   Team 选自己的 Apple ID（免费账号即可，App 7 天要重签一次）。
   模拟器直接 ⌘R 也能跑，模拟器走宿主网络，连局域网 IP 没问题。

> 想用 XcodeGen 就不用手点：把 `AnyBootRemote/` 六个 swift + `Info.plist` 交给它，
> `INFOPLIST_FILE` 指向 `Info.plist` 即可。

## 用法

1. 电脑上打开 AnyBoot，「基本设置 ▸ 远程控制」**打开**并使端口为 `12300`（或你改过的端口）。
2. 确认手机与电脑在同一局域网（手机连的路由器 / 热点要和电脑一致）。
3. 首次启动 App 会弹出表单：
   - **分组**：例如「书房台式机」；同一台电脑的多个 IP 填同一个分组（会自动并成一个标签）。
   - **服务端名**：例如「有线网卡 / 5G WiFi」（可留空，用地址兜底）。
   - **服务端地址**：形如 `192.168.0.120:12300`；**这台电脑有多个网卡就一行一个全填上**；
     也可以把整条带 token 的 URL 粘进来，自动拆出 token。
   - **访问令牌 token**：可留空——留空时页面自己会弹输入框要 token。
4. 点标签栏 `＋` 继续添加第二台电脑；想给**同一台**电脑补地址，分组填同一个名字即可。
   长按标签改这一条；列表页左上角「合并同机地址」能把老配置里重复的记录收成一条。
5. 左上「重载」刷新当前页；右上 `⋯` 里有「添加 / 编辑 / 列表 / 全部重载 / 启动时连接全部服务端 /
   **传输方式**」。

### 传输方式与刷新

页面默认走**推送**（`/api/ws`，WebSocket）。`⋯` 菜单里的 **「传输方式（自动 / 推送 / 轮询）」**
是个子菜单，三项前面带勾的就是当前档位：

- **自动**（默认）：先试推送，连不上落到 2 秒轮询，后台继续试着接回推送；
- **推送**：只用 WebSocket，断了提示「推送通道已断开，正在重连…」并退避重连（不偷偷改回轮询）；
- **轮询**：只用 HTTP。

选的值存在 `UserDefaults`（`ServerStore.setTransport`），加载页面时由 `url(at:transport:)`
拼成 `?transport=` 交给网页决定 —— 因为网页的 `localStorage` 按**来源站点**（IP）存，
换网卡就换地址、偏好会丢。选完**不立刻重载**（菜单标题即时反映选择），下次加载生效；
页面里也能在横幅的下拉里即切即用（同一个偏好，网页端会记住）。

**刷新**：页面上**下拉**即刷新（「下拉刷新 → 松开刷新 → 正在刷新…」），横幅上还有
**「刷新」** 按钮。两者都走 `/api/ui-state`，与当前通道无关。左上「重载」是**重新加载整个页面**，
和「刷新」不是一回事。

## 注意

- 本机（Windows / Linux）**无法构建** iOS：没有 Xcode。这里交付的是可直接拖进工程的源码。
- 地址切换的边界：`?token=` 是**按来源站点**存 cookie 的，所以换地址（换了 IP）后页面会重新
  读一次 URL 里的 token。**把 token 填进 App 里**最省事 —— 否则换地址后可能需要再输一次。
- 服务端是明文 HTTP，仅限可信局域网使用；不要把 `12300` 直接暴露到公网。
- 页面里的 `?token=` 会在验证通过后写进 cookie，所以地址栏里带的 token 不会长期暴露在 URL 上。
- **推送与下拉刷新都不用 iOS 侧写代码**：`new WebSocket(...)` 与触摸手势都在网页端，
  WKWebView 原样支持。iOS 侧只做两件事：把「传输方式」偏好拼进 URL（`?transport=`）、
  在 `didFailProvisionalNavigation` 里**忽略 `NSURLErrorCancelled`（-999）**——
  自取消不算「地址不通」，否则换地址时会被误判成失败、白跳过候选地址。
