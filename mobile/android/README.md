# AnyBoot 远程 · Android 客户端

**它不是 AnyBoot 的安卓移植版**，而是一个 WebView 外壳：加载电脑端 AnyBoot 内置 HTTP 服务
（默认 `12300`）托管的**同一份前端页面**，页面里的 `/api/ui-state`、`/api/ui-command`
都是同源请求。所以：

- 不需要把 Rust 核心交叉编译到 `aarch64-linux-android`，也不需要 NDK；
  服务端永远是 Windows / Linux / macOS 上的那个 AnyBoot。
- 电脑端前端一改，手机端刷新就是新的，不存在两套 UI 走样。
- 手机端只做浏览器不顺手的事：**多台电脑同时在线、一台电脑一个标签、多网卡地址自动切换、记住 token、断线重试面板**。

## 多电脑 / 多网卡

**一条记录 = 一台电脑**；一条记录内部可以放多个候选地址（多块网卡 / 换过 DHCP）。

| 能力 | 说明 |
| --- | --- |
| 同时连多台电脑 | 每台电脑一个标签、一个 `WebView`，互不影响；菜单里「启动时连接全部服务端」可让它们一起在线 |
| 顶部标签快速切换 | 标签栏横向滚动，点一下就切；已加载过的标签**不重新加载**，秒切 |
| 一台电脑一个标签 | 标签数 = 电脑数（不是 IP 数）。多块网卡是**同一条记录里的多个候选地址** |
| 多地址自动切换 | 地址框支持**多行**（一行一个）。首选地址不通就自动换下一个 —— 还是同一个标签、同一个 `WebView`，会提示「这个地址不通，换一个地址重试：…」 |
| 记住可用地址 | 哪个地址连通就把它提到最前，下次直连它（DHCP 换过 IP 也能自己找回来） |
| 全试完才报错 | 候选地址全试过才显示重试面板，面板里把试过的地址全列出来 |
| 自动归并 | 保存地址后探一次 `GET /health`（免 token）拿 `machineId`：和已有记录相同就**并进那条记录**，不新建标签；拿不到标识时退回「同分组名 = 同一台电脑」 |
| 一键合并老配置 | 以前「每个 IP 一条记录」的老配置：`☰` 列表里点**「合并同一台电脑的地址」** |
| 标签名 = 组名 ＋ 服务端名 | 未分组时只显示服务端名；名字留空则用地址兜底 |
| 分组配色 | 同一个分组的标签描边同色（颜色由组名哈希决定，恒定不变） |
| 在线状态点 | 标签左侧小圆点，**由 10 秒心跳驱动**：绿=在线、灰=离线/未探到、橙=仅加载中 |
| 10 秒心跳 | 前台每 10 秒探一遍所有服务端 `/health`（`HeartbeatManager.java`）：上线立即变绿，**连续 2 轮全失败**才判离线；多网卡探测**并发（默认，同探所有地址用最快的）/ 串行（按顺序）**在编辑服务端里选；停在错误面板上的那台一探通就自动重连；退后台停表省电；**经由跳板机的电脑，探测也从 SSH 隧道里走** |

## 服务器模块（跳板机）与外网连接 —— F2–F7

侧边栏（左上 ☰）：**电脑 / 服务器 / 关于**。

| 能力 | 说明 |
| --- | --- |
| 服务器管理 | 独立 `ServersActivity`（FinalShell 风格：徽标 + 名称 + 协议徽标 + 延迟 + 地址 + 行展开详情：端口/认证/备注/经由电脑数 + 测试连接/编辑/删除/▶ 终端/浏览文件） |
| SSH 隧道（外网连家里） | 添加电脑表单里选「经由跳板机」→ 地址语义变为「跳板机视角的内网地址」；WebView / 心跳 / 全部请求都从 `127.0.0.1:随机端口` 走 SSH `direct-tcpip` 本地转发（`SshSessionManager.setPortForwardingL`） |
| 终端 | `TerminalActivity`：xterm.js（assets/terminal/）+ 底部按键栏（Esc/Tab/Ctrl/方向键，Ctrl 臂上字母行）+ 深/亮主题 + 字号/键盘 + 多会话标签（Activity 销毁会话不断，`TerminalRegistry` 静态持有） |
| 密钥 | 密钥页：导入私钥（文件选择器）/ 本机生成（Ed25519 / RSA4096，JSch KeyPair）；私钥**只进 Android Keystore 加密落盘**（`KeyVault` AES-GCM）；SSH 首连 **TOFU 指纹弹窗确认**并存住，对不上（中间人）拒连 |
| 文件 | 文件页（`ChannelSftp`）：浏览/上级/上传（文件选择器）/下载到 App 私有 Downloads；Telnet 服务器该页提示不支持 |
| 会话 | 会话页：进行中（状态点 + 时长）+ 历史（最近 50 条，`JumpStore`） |
| 关于页 | `AboutActivity`：GitHub 仓库卡片（★/⑂/Issues 实时拉 API，失败用缓存）+ **★ Star** 按钮（打开仓库网页，App 不代点） |
| 断线重连 | SSH 会话断线指数退避重连（1s→2s→…封顶 60s）；`ServerAliveInterval` 15s NAT 保活；回前台心跳先补探一轮 |
| Telnet | 实验性：socket 直连 + IAC 过滤（`TelnetSession.java`），只有终端，无隧道无文件 |

**依赖**：`com.github.mwiede:jsch:0.2.20`（app/build.gradle.kts）。mwiede 分叉支持
rsa-sha2-*，老版 0.1.55 连现代 OpenSSH 会被拒。API 与老 JSch 有差异处
（HostKeyRepository 是 interface、KeyPair.getPrivateKey 包私有走 writePrivateKey 导出）
已在 `SshSessionManager` / `ServersActivity` 注释里标明。
| 增删改 | 标签栏 `＋` 添加；**长按标签**直接编辑；`☰` 或菜单「服务端列表」切换/删除；编辑框里也有「删除」 |
| 1.0 / 1.1 升级 | 旧版单服务端配置（`host`/`token`）自动迁移成列表第一条；旧版本每条只有一个 `host` 的记录会升级成「只有一个候选地址」的记录，用户无感 |

## 目录

```
mobile/android/
├── settings.gradle.kts / build.gradle.kts / gradle.properties
├── local.properties                         # sdk.dir，已被 .gitignore 忽略
└── app/
    ├── build.gradle.kts                     # compileSdk 34 / minSdk 24 / AGP 8.7.3
    └── src/main/
        ├── AndroidManifest.xml              # INTERNET + usesCleartextTraffic
        ├── java/com/anyboot/remote/
        │   ├── MainActivity.java            # 标签栏 + 每台电脑一个 WebView + 多地址切换 + 增删改对话框 + 侧边栏 + 跳板机隧道接线
        │   ├── ServerProfile.java           # 一条记录（= 一台电脑）：地址列表/分组/名称/token + 地址归一化 + jumpServerId
        │   ├── ServerStore.java             # SharedPreferences(JSON) 持久化 + 旧版迁移
        │   ├── HealthProbe.java             # 探 /health 拿 machineId（自动归并同一台电脑）+ 心跳 ping
        │   ├── HeartbeatManager.java        # 10 秒心跳（在线状态轮询，多网卡并发/串行探测，隧道解析器注入）
        │   ├── JumpServer.java              # 一台跳板机（SSH/Telnet）：协议/认证/密钥引用
        │   ├── JumpStore.java               # 跳板机 + 密钥持久化、会话历史、终端主题、TOFU 指纹、GitHub 缓存
        │   ├── SshKeyEntry.java / KeyVault.java  # 密钥模型 + Keystore AES-GCM 加密落盘
        │   ├── SshSessionManager.java       # SSH 会话栈：连接复用/退避、TOFU、隧道（setPortForwardingL）、shell 通道
        │   ├── TelnetSession.java           # Telnet 直连 + IAC 过滤（实验性）
        │   ├── TerminalSession.java / TerminalRegistry.java  # 终端会话统一外壳（backlog/resize）+ 静态注册表
        │   ├── TerminalActivity.java        # xterm.js 终端页（按键栏/主题/字号/键盘/多会话标签）
        │   ├── ServersActivity.java         # 服务器模块四页（会话/终端/密钥/文件 SFTP）+ 增删改表单
        │   └── AboutActivity.java           # 关于页（GitHub 卡片 + Star）
        ├── assets/terminal/                 # xterm.js 5.3.0 + fit 插件 + 宿主页（iOS 共用同一份）
        └── res/                             # 图标 + app_name
```

## 构建

**两台开发机都能出 APK**，产物一样（都是 debug 签名包，可直接 `adb install`）。

### Linux（本仓库所在的开发机）

工具链是 2026-10-13 就地装好的，位置固定：

| 组件 | 路径 |
| --- | --- |
| Android SDK | `~/android-sdk`（cmdline-tools 11076708 + platform-tools + `platforms;android-34` + `build-tools;34.0.0`，许可已接受） |
| Gradle 8.11.1 | `~/.cache/ab-android/gradle-8.11.1`（解包即用） |
| JDK 21 | `/home/disk2/software/jdk/jdk-21` |

一键脚本：`~/.cache/ab-android/build.sh`（等价于下面这段手敲的命令）：

```bash
export JAVA_HOME=/home/disk2/software/jdk/jdk-21
export ANDROID_HOME=$HOME/android-sdk
export ANDROID_USER_HOME=$HOME/.android                        # 坑 1
export GRADLE_USER_HOME=$HOME/.cache/ab-android/gradle-home     # 坑 2
cd mobile/android
env -u http_proxy -u https_proxy -u HTTP_PROXY -u ALL_PROXY \
  $HOME/.cache/ab-android/gradle-8.11.1/bin/gradle assembleDebug --no-daemon --console=plain
```

四个必踩的坑（都在这台机器上踩过）：

1. **Java 在 Linux 上从 passwd 取 `user.home`，不看 `$HOME`** —— 本机 passwd 里是 `/root`，
   于是 sdkmanager / Gradle 都往 `/root` 写 → 权限失败。现象会误导人：
   sdkmanager 报的是 `Failed to download any source lists!`，看着像网络问题，其实是写不进去。
   所以 `ANDROID_USER_HOME` / `GRADLE_USER_HOME` 必须显式指向可写目录。
2. **`~/.gradle/init.gradle` 与工程的 `FAIL_ON_PROJECT_REPOS` 冲突** —— 那份是用户自己的国内镜像
   配置，它会往 `allprojects` 里注入 maven 仓库，而本工程声明了「不许在工程之外加仓库」→ 直接构建失败。
   **别去删用户的文件**，换一个隔离的 `GRADLE_USER_HOME` 就绕开了。
3. **会话里有 `http_proxy` / `https_proxy`**（端口每次会话不同），Gradle 拉依赖会走它。
   用 `env -u ...` 摘掉再用；`curl` 同理（本项目自测一律 `--noproxy '*'`）。
4. **`export A=x PATH="$A/bin:$PATH"` 这种写法同一句里展开**：shell 先展开 `$A`（此时还是空的），
   结果是 `PATH="/bin:$PATH"` → 报 `java: not found`。A、PATH 分两句写。

另：**不要拿 JetBrains 自带的 jbr 当 `JAVA_HOME`** —— 那个是 JDK 25，Gradle 8.11.1 起不来。
17～23 都行，本机用的是 21。

`gradlew` 用不了：`gradle/wrapper/` 里只有 `gradle-wrapper.properties`、没有 `gradle-wrapper.jar`
（仓库里不放二进制包）。要用 wrapper 就先 `gradle wrapper --gradle-version 8.11.1` 生成一次。

### Windows（Git Bash）

```bash
export JAVA_HOME=/e/Android/jdk17/jdk-17.0.20.1+1
export GRADLE_USER_HOME='E:\Android\gradle-home'    # 缓存全在 E 盘，不占 C 盘
cd mobile/android
/e/Android/gradle-8.11.1/bin/gradle assembleDebug --no-daemon --console=plain
```

产物：`mobile/android/app/build/outputs/apk/debug/app-debug.apk`

首次构建会从 Google Maven 拉 AGP 及依赖（几百 MB）到 `GRADLE_USER_HOME`。
用 Android Studio 打开 `mobile/android/` 也可以，AGP/Gradle 版本已对齐。

### 只有 JDK、没装 SDK 的机器上怎么先做类型检查

临时在别的机器（或 CI）上想拦住语法/类型错误、又不想装整套 SDK 时，JDK + 一份 Android 框架
stub 就能做**完整类型检查**（不打包）：

```bash
# 一次性：拉 Android 14 的框架类（Robolectric 的 android-all，含 android.R）
curl --noproxy '*' -sSL -o android-all.jar \
  https://repo1.maven.org/maven2/org/robolectric/android-all/14-robolectric-10818077/android-all-14-robolectric-10818077.jar

JBR=/path/to/jdk17+/bin/javac        # 17 及以上
mkdir -p out
"$JBR" -encoding UTF-8 --release 17 -classpath android-all.jar -d out \
  app/src/main/java/com/anyboot/remote/*.java
```

> 本仓库所在的 Linux 开发机**已经装了真 SDK，直接用上面的 `build.sh` 出 APK 就行**，
> 这段只是给「手边没有 SDK」的机器留的后路 —— 它只提前拦编译错误，不产出包。

## 安装与使用

```bash
adb install -r mobile/android/app/build/outputs/apk/debug/app-debug.apk
```

1. 电脑端 AnyBoot：**基本设置 ▸ 远程控制** 打开，端口 `12300`。
2. 手机与电脑在同一局域网（同一路由器 / 同一热点）。
3. 打开 App → 首次会要求填一台电脑：分组、服务端名（可留空）、地址
   （`192.168.0.120:12300`，也可以整条带 token 的地址粘进来，token 会自动拆出来）。
   **这台电脑有多个网卡就一行一个地址全填上**，客户端会用能通的那个。
4. 想连第二台电脑：点标签栏的 `＋`；想给**同一台**电脑补地址，
   **分组填同一个名字**即可 —— 会自动并进那个标签，不会多出一个。
5. 长按标签改这一条；`☰` 看列表并切换（里面还有「合并同一台电脑的地址」）；
   菜单里可「重载当前页面 / 全部重载 / 启动时连接全部服务端 / **传输方式**」。
6. 返回键先退当前页面的站内导航，退不动才退出 App。

### 传输方式与刷新

页面默认走**推送**（`/api/ws`，WebSocket），菜单里的 **「传输方式（自动 / 推送 / 轮询）」** 可切换：

- **自动**（默认）：先试推送，连不上落到 2 秒轮询，后台继续试着接回推送；
- **推送**：只用 WebSocket，断了就退避重连（不偷偷改回轮询）；
- **轮询**：只用 HTTP。

选的值存在 `SharedPreferences`（`ServerStore.setTransport`），加载页面时由
`ServerProfile.urlAt(index, transport)` 追加成 `?transport=` 交给网页决定 —— 之所以不只在网页端存，
是因为网页的 `localStorage` 按**来源站点**（IP）存，换网卡就换地址、偏好会丢。
选完**不立刻重载**（免得打断正在看的东西），下次加载生效；页面上也能在横幅下拉里即切。

**刷新**：页面上**下拉**即刷新（「下拉刷新 → 松开刷新 → 正在刷新…」），横幅上也有
**「刷新」** 按钮立刻抓一份快照。这两个走的是 `/api/ui-state`，与当前通道无关 ——
推送丢了一拍或电脑上刚改完，拉一下立刻对齐。菜单里的「重载当前页面」是**重新加载整个页面**，
和「刷新」不是一回事。

## 实现要点（踩过的坑）

- **明文 HTTP 必须显式放行**：`android:usesCleartextTraffic="true"`，
  否则 Android 9+ 直接掐断到 `http://192.168.x.x:12300` 的连接；
  WebView 侧还要 `setMixedContentMode(MIXED_CONTENT_ALWAYS_ALLOW)`。
- **必须开 DOM storage**：前端的 token 存在 cookie、栏目选择存在 localStorage，
  不开 `setDomStorageEnabled(true)` 会表现为「token 永远存不住、每次都要重输」。
- **只有主文档失败才算「连不上」**：`onReceivedError` 里判 `request.isForMainFrame()`，
  否则一个 favicon 404 就会把整个页面替换成错误面板。
- **换地址必须先认「这个回调说的是哪个地址」**：换了地址之后，旧地址的
  `onPageStarted / onReceivedError / onPageFinished` 还会陆续回来，
  所以三者都先过 `isCurrentAddress(tab, url)`（比对**当前**地址前缀，且后面必须紧跟
  `/` 或 `:` —— 否则 `192.168.1.5` 会把 `192.168.1.50` 的页面认成自己）。
- **失败只在「本轮没试过的地址」里往下走**：`Tab.tried` 记着试过哪些，
  全试完才显示重试面板；`onPageFinished` 里还加了 `status != STATUS_FAIL` 的闸门，
  防止错误页反过来把状态刷成「已加载」。
- **`promoteAddress` 之后下标要归零**：成功的地址会被提到 `addresses[0]`，
  这时必须把 `Tab.addressIndex` 也设回 0，否则下一次 `urlAt(index)` 会指到别的地址。
- **`/health` 探测必须在子线程**：Android 主线程禁网络（`NetworkOnMainThreadException`），
  所以 `HealthProbe.fetch` 跑在 `anyboot-health-probe` 线程里，回主线程再改 UI。
- **WebView 懒创建**：只有切到过的标签（或开了「启动时连接全部」）才建 `WebView`，
  十条电脑配置不会在启动时白占十份内存。
- **切标签不重载**：已加载过的页面保留在后台（切回来直接显示），
  只有编辑过地址/token 或手动重载才重新拉页面。
- **`configChanges` 里带 `orientation|screenSize`**：转屏不重建 Activity，
  否则十个标签会一起重新加载。
- 不引 AndroidX / Kotlin：全部用系统 `WebView`、`AlertDialog`，依赖只有 AGP 本身，
  构建体积和失败面都小。
- **传输方式要由 App 从 URL 带过去**：`urlAt(index, transport)` 追加 `?transport=`。
  网页端的 `localStorage` 是按来源站点存的，换网卡会换地址（= 换来源），只存网页端的话
  偏好会跟着丢。空值和 `auto` 都不加参数（默认就是自动）。
- **WebSocket 不用 App 操心**：页面里的 `new WebSocket(...)` 由 WebView 自己发，
  握手用 `?token=`（前端 `connectPush` 拼上去）或 cookie。所以除了「把偏好传进 URL」，
  客户端没有任何额外工作 —— 推送、退避重连、退回轮询全在网页端。
- **下拉刷新也在页面里**：`ui/app.js` 的 `installPullToRefresh()` 用非 passive 的
  `touchmove` + `preventDefault` 接管手势（不拦的话 WebView 会当成整页回弹刷新）。
  客户端什么都不用做，但要保证 `setDomStorageEnabled(true)` 之类的 WebView 能力都开着。
