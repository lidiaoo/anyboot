# AnyBoot 项目长期备忘

## 构建与验证（照抄这串，别漏）
- `export HOME=/home/jianghh CARGO_HOME=/home/jianghh/.cargo`（本机 `$HOME=/root` 属主 nobody → cargo 读 `~/.rustfmt.toml` Permission denied）。
- **cargo 必须非沙箱跑**（沙箱里 tauri-codegen 读 `target/release/build/anyboot-*/out/` 报 os error 5）。
- `cargo test --release` **不产出可交付 exe**；交付 = 测试 **+ `cargo build --release`**，构建前退出运行中的 anyboot。
- 只改前端时增量编译**不重跑打包**，前端不进 exe。强制：`rm -rf target/release/build/anyboot-*/out && touch build.rs && cargo build --release`（`rm` 被 safe-delete 拦时用 `/usr/bin/rm`）。核对：`out/tauri-codegen-assets/*.css|js` 是 brotli，node `zlib.brotliDecompressSync` 解开 grep。前端语法：`node --check ui/<每个>.js`。
- 冒烟脚本在 `/home/jianghh/.cache/ab-test/`（run.sh 贴边 / mode-smoke.sh 四模式 / size-smoke.sh 窗口尺寸 / notify-smoke.sh 弹窗）。宿主 12300 被占时给 settings 预置 `httpPort: 12400`。

## 结构要点
- 后端 `src-tauri/src/lib.rs`（单文件很大）+ `http.rs`（tiny_http 0.12）；前端 `ui/`：`index/app.js/styles.css` 主窗、`notify.*` 弹窗、`grub-list-view.js` 树形渲染（主窗/GRUB 窗共用）。
- 多窗口同源：`publish_ui` emit `ui-state-changed` 广播；新窗口先 `get_ui_state` 再监听同事件。新窗口 `.visible(false)` 建好摆好再 show。
- `UiState` 是唯一事实来源；明文 token 不进快照。子进程必须走 `fn command(program)`（Windows CREATE_NO_WINDOW）。

## tiny_http 的坑（Windows 致命）
- `Server::drop` 用 `connect(listening_addr)` 唤醒 accept 线程；绑 `0.0.0.0` 时 Windows 上必失败 → 端口永不释放。`RunningServer::stop()` 必须自己补 127.0.0.1 自连接并轮询到端口真能 bind，契约「返回时端口已放开」。

## Linux 写 ESP / 同步 grub.cfg 权限模型
- 读 `/boot/grub/grub.cfg`（mkconfig 固定 0600 root）与写 `/boot/efi`（vfat 无 uid=）只有 root 能做。修法：`copy_into_esp`/`write_esp_bytes` 先 `access(2)`，写不动就 `pkexec install -p -m 0644`（`install -p` + `same_size_and_time` 防重复弹框）。远程请求用 `RemoteNoGuiAuth::enter` 圈住整次执行（不能弹框）。

## 窗口/WebView 定则（细节见 MEMORY-detail-windowing.md）
- 判位**只用客户区坐标**（`main_client_position()`/`inner_position()`）；`Moved` 载荷与 `outer_position()` 是窗框坐标，Linux 去装饰后恒偏一个标题栏 → 拿来比 `dock_pos` 必错。
- 贴边三道闸：`geometry_busy()`（发几何请求**之前**立）+ `main_moved_recently()`（300ms 静默期）+ 回声哨兵（持久，±8px）。dock 与 undock 两侧都要挡。
- 可见窗口上「去装饰+改几何」会被重父化吃掉 → **hide() → 改几何 → show()**，monitor 在 hide 前算。
- **GTK 上 set_position 语义随装饰状态变**：去装饰=客户区坐标（dock 路径准）；带装饰=**窗框**原点，客户区=请求+标题栏（真机 Cinnamon 2x=64px）。凡「带装饰落位」要么实况自校准（undock 重锚牛顿步：偏差反扣 offset、收敛写回跨轮复用），要么显式扣标题栏。Xvfb 无主题标题栏 0px → 复现台永远测不出这条。
- tao 未映射窗口 `outer_size()` 返回 0x0（inner 可读）。
- **隐藏窗口的 WebView 视口在 Linux/GTK 上是 0x0**（Windows/macOS 有真实分配）：0 宽视口排版把文字折成竖条 → 前端量「内容高度」严重虚高（notify 弹窗曾恒顶满上限 460，底部空白一大块）。修法：`.panel` 钉 `min-width` = 窗口宽度常量（两处必须一致）+ 监听 `resize` 在 show 后重测自愈。凡是「隐藏窗口里量 DOM 排版」都要过这条。
- 判「乱闪乱跳」只认振荡判据：状态 <600ms 内回到两拍前的值（A→B→A）。

## 移动端（Android / iOS）= WebView 客户端，**不是**原生移植
- 手机端与 web 端同构：只加载电脑端内置 HTTP 服务（默认绑 `0.0.0.0:12300`）托管的那份 `ui/` 前端，
  `/api/*` 全是同源请求。**不要**为安卓/iOS 交叉编译 Rust 核心（不需要 NDK；
  `cargo check --target aarch64-linux-android` 那条路是错的，`gen/android` 已删）。
  服务端 bind `0.0.0.0`，`is_loopback` 只用于禁「LAN 冒充 CLI 渠道」，不拦手机。
- `mobile/android/`：Java + 系统 WebView，**不引 AndroidX/Kotlin**（依赖仅 AGP 8.7.3）。
  Gradle 8.11.1 + compileSdk/targetSdk 34 + minSdk 24。
  `export JAVA_HOME=/e/Android/jdk17/jdk-17.0.20.1+1 GRADLE_USER_HOME='E:\Android\gradle-home'`；
  `cd mobile/android && /e/Android/gradle-8.11.1/bin/gradle assembleDebug --no-daemon`
  → `app/build/outputs/apk/debug/app-debug.apk`。Gradle 解包在 `E:\Android\gradle-8.11.1`。
- Android 三必备：`usesCleartextTraffic=true`、`setDomStorageEnabled(true)`（token 走 cookie）、
  `onReceivedError` 只认 `isForMainFrame`。
- `mobile/ios/`：SwiftUI + WKWebView 源码 + `Info.plist`（ATS 放行 + `NSLocalNetworkUsageDescription`）。
- **Windows 原生 python 不认 MSYS 路径**：`/e/Android/x` → `<当前盘>:\e\Android\x`（写错树），必须传 `E:/Android/x`。

## Windows 宿主链接器（踩过两次）
- 宿主 msvc 目标必须有 VC 工具链；PATH 里 `/usr/bin`（Git 自带 coreutils `link`）早于 MSVC 时会遮蔽它，
  报 `link: extra operand ... Try 'link --help'`。自检：`where link.exe` + `vswhere -latest -products '*' -property installationPath`。
- 本机 VS 生成工具装在 **`D:\VS`**（VCTools + Windows SDK 10.0.26100.0，C 盘紧张刻意装 D 盘），
  rustc 走 vswhere 自动发现，不用改 PATH。

## 前端栏目 = 地址栏路由
- 栏目写进 hash：`#/boot`（系统引导）、`#/settings`（基本设置），`history.replaceState` 同步、`hashchange` 反同步。
- `initialTab()`：hash > （非浏览器才读 localStorage）> `boot`。**网页端/手机 WebView 默认「系统引导」**，
  只有桌面主窗口记忆上次栏目 —— 改这里别把网页端又拖回「一刷新就进基本设置」。
- 本机自测 HTTP：Bash 的 curl 带 `http_proxy=127.0.0.1:3564`，必须 `--noproxy '*'` 才连得到 12300。
- **启动期别偷偷点页签**：浏览器模式的 `listen` shim 对 `focus-remote` 原写成「等价于切到设置栏目」，
  而 shim 在**注册时**就执行 → 每次开网页都被顶到基本设置。改栏目/首屏怪象先查启动期有没有代点按钮的代码。
- **Windows 打开 URL 用 `ShellExecuteW`，别用 `explorer <url>`**：explorer 会把带 query 的地址当路径解析；
  再配 `.status()` 同步等，弹错误框时进程永不退出 → 命令挂死（表现＝按钮点了没反应）。
  `ShellExecuteW` 返回值 **≤32 是错误码**，>32 才算成功。依赖：windows-sys 0.59（UI_Shell + WindowsAndMessaging + Foundation）。
- **GUI 弹窗/进程启动类行为在 CLI 会话里测不出来**：shell 里 explorer / Start-Process / 非沙箱 ShellExecuteW
  都"成功"返回，但桌面无新窗口、本地探针服务器收不到请求 → 必须让用户实机点一次确认，别拿 CLI 结果当结论。
- **别让后台重试碰用户正在输入的控件**：网页端 2s 轮询 401 → `requestRemoteToken()` 会把
  `#token` 输入框 `value` 抹空 + 抢焦点（表现为「token 打一半就没了／像页面被刷新」）。
  定则：① 模态框只在「确实新打开」时写 value/focus，已开着只更新文案；② 轮询/重试路径开头
  `if (!remoteToken) return;`，没 token 不产生 401 洪流；③ 用户点过「暂不连接」要记住，别自动弹回。
  另：给 token 这类输入框一律关 `autocapitalize/autocorrect/spellcheck`（手机软键盘会悄悄改坏）。

## 移动端（`mobile/`）：WebView 客户端，服务端一行不改
- 结构：Android = Java + 系统 WebView（依赖只有 AGP，无 AndroidX/Kotlin）；iOS = SwiftUI + WKWebView。
  两端同构，加载电脑端托管的同一份 `ui/`，`/api/ui-state`、`/api/ui-command` 都是同源请求。
- **多服务端 = 每个标签一个独立 WebView**（Android 懒创建 + `Tab.started` 判重；iOS 常驻 + opacity 切换），
  才谈得上「同时在线、切换不重载」。标签名 = `组名 · 服务端名`，分组 = 一台电脑（多网卡/多 IP 归一组），
  分组配色取组名哈希定色相，标签圆点 = 该服务端加载状态。
- Android 构建（本机已验证）：`JAVA_HOME=/e/Android/jdk17/jdk-17.0.20.1+1`、
  `GRADLE_USER_HOME=E:\Android\gradle-home`，跑 `/e/Android/gradle-8.11.1/bin/gradle assembleDebug`，
  产物 `mobile/android/app/build/outputs/apk/debug/app-debug.apk`（debug 已签名，可直接 adb install）。
  核对入包：`unzip -o app-debug.apk "classes*.dex"` 后查 `classes2.dex`（代码不在 `classes.dex`，那里只有 1.3KB 存根）。
- 定则：`activate()` 必须**先 ensureTab/connect 再 applyVisibility()**；判「是否已加载」用自建 `started` 标记，
  不要用 `WebView.getUrl()`（刚 loadUrl 时可能还是 null → 重复加载）。
- **文档落点（改客户端行为时同步改这三份，别漏）**：`ui/README.md` = Web 端使用说明（打开方式/token/栏目 hash/`gui_only` 边界）；
  `mobile/README.md` + `mobile/android|ios/README.md` = 移动端安装与用法；根 `README.md` 的「客户端（Web 端 / 移动端）」小节是入口。
- **提交约定**：构建产物不入库（`.gitignore` 已挡 `mobile/android/**/{build,.gradle,local.properties}`、`*.apk/*.aab/*.keystore`、
  iOS `DerivedData/xcuserdata`）。交付 APK 时在 README 里给 gradle 命令现打，不要为了图省事把 APK 提交进去。
