# AnyBoot 项目长期备忘

**细则查：** 当日日志 `.workbuddy/memory/2026-10-*.md`；桌面/打包/沙箱 → `MEMORY-detail-desktop.md`；
托盘与 dbusmenu → `MEMORY-detail-tray.md`；窗口/贴边/多窗 → `MEMORY-detail-windowing.md`。

## git
- **commit 由我做，push 由用户在 IDE 做**（本机无 GitHub 凭据，push 必失败）。远端 https://github.com/lidiaoo/anyboot.git
- 只提交本轮相关文件；`src-tauri/src/hub*.rs`、`ui/hub.html`、`docs/client-ui/pc-entry-map.html`、`debx/` 属 PC 端 server-hub 另一条线，别混进移动端提交。

## 桌面端（最常踩的三条）
- 前端改动必须强制 codegen：`rm -rf src-tauri/target/release/build/anyboot-*/out && touch src-tauri/build.rs && cargo build --release`；核对解 `out/tauri-codegen-assets/*`（node brotli），**二进制里搜中文用 `buf.includes(串,"utf8")`**（latin1 匹配不上）。
- 交付 = 测试 + release 构建 + 实机验证；构建前先退出运行中的 anyboot；**产物 md5 必核**（Text file busy 会静默失败）。
- `ui/index.html` 的 `data-page-node-id` 原样保留；标题字号别挂在会搬家的容器下；文档路径写 `~`，不写死 `/home/jianghh`。

## 托盘（要点，细节见 MEMORY-detail-tray.md）
- `trayMenuStyle` 启动时定接线、切换需重启。classic = Win/mac muda 原生菜单，**Linux 走自研 `linux_sni` 全量 dbusmenu**；unified = Win/mac 自绘窗，Linux SNI 导出 dbusmenu（GNOME 壳菜单只有 [显示主窗口, 退出程序]，opened → 弹自绘窗 + 150ms 合成 Esc）。
- 自绘窗落位：unified 贴图标**正下方**居中、放不下翻正上方、绝不盖图标；classic 上方 + 左对齐。`TrayCenter` 已弃用；失焦看门狗 250ms。**落位全手算，positioner 只维护插件状态**。
- `grub_menu_nodes` **一份函数喂四处**（主窗 / unified 自绘窗 / Linux dbusmenu / Win·mac 原生菜单）——改它三头都变；行文字**箭头在前、图标在后**。
- dbusmenu 四条死线：信号从 `MENU_PATH` 发；回显走 `ItemsPropertiesUpdated` 差分（都没变则不发）；信号体签名 **`a(ia{sv})a(ias)`**；**禁带 `toggle-type`/`toggle-state`**（勾选画进 label `◉`/`○`）。`ItemIsMenu=false` + `Menu=object path`；`AboutToShow` = 真脏标记（GetLayout 应答清零）。
- macOS 原生对话框不可用（rfd 无 parent 时阻塞 ~30s）→ 走 `osascript -e 'display dialog …'`。
- 待办：本机无 win/mac target，**上 Windows 先 build 再实机点托盘复验两形态**。

## 产品定则
- 默认值常量钉着（改值同步单测）：亮色 / 90% / `remoteAllowGuiAuth=true` / 默认高度=整屏 2/3（基准 `monitor.size()`）；`window_height==0` 是「没记过尺寸」哨兵。落位 free/center/last：center=禁拖；锁定窗口大小只禁手拖缩放、不拦落宽。
- 下次启动大字=级联全链；Rust 侧级联文案必须与前端 `nextBootText`/`rowLabel` 一字不差；托盘指纹含所有可见字段。
- 忙闸必须有放闸路径（判据不能是「命令返回成功」——`systemctl suspend` 排队即返回 0）；`POWER_RELEASE_MS=15s` 世代号放闸；被闸点击必须 record_action 留痕。
- 远程动作 = 本机点头（pending + 授权窗）；灰掉而不藏按钮。
- 顶部标题栏 = 电源三键 + 重启程序 / 退出程序。`restart_app` 顺序固定：**stop_http_server → release_instance_lock → app.restart()**。
- 单实例 `anyboot.lock`（`File::try_lock`；守卫放 `setup` 最前，拿不到锁就只提示、后面一行都不做）。tiny_http Windows `stop()` 必须 127.0.0.1 自连接轮询；Linux 写 ESP 探测失败走 pkexec。

## 移动端（mobile/）——当前主战场
- Android 已迁 **Jetpack Compose**（Kotlin；业务 Java 类保留）。构建 env：`GRADLE_USER_HOME=~/.cache/ab-gradle`、`JAVA_HOME=~/.sdkm/links/java`(21.0.11)、`ANDROID_HOME=~/Android/Sdk`、`ANDROID_USER_HOME=~/.android`、`HOME=/home/jianghh`；AGP 8.7.3 / Gradle 8.11.1（`~/.gradle/wrapper/dists/gradle-8.11.1-bin/*/gradle-8.11.1/bin/gradle`）、SDK android-34。**本机 shell `ulimit -v` 4G 硬限** → daemon jvmargs 压小 + `_JAVA_OPTIONS="-Xmx768m -Xms64m"`，否则 metaspace 映射崩（hs_err）。skill：`android-gradle-compose-build` / `android-apk-lite-build`。
- 驱动脚本 `~/.cache/ab-droid/drv.py`（自带 HOME/ANDROID_USER_HOME）：`dump` / `rows`（按 y 打印 text 节点，看行高与留白）/ `find <子串>` / `tap <子串>`。模拟器 `emulator-5554`，1080x2400 @420（1dp=2.625px）。
- 终端页（WebView + xterm.js）**尺寸绝不能依赖 html/body 百分比高度**——本机 WebView 解析成 0，FitAddon 兜底 1 行，表现为「回车无输出 / 输入被吞」。唯一尺寸来源是 JS `syncSize()`；会话绑定与 onResume 时必须调 `termSync()` 把真实行列重报给 PTY。诊断 tag **ABTERM**（tlog 常开 / tlogV 关）。
- **iOS 侧 PTY 尺寸是坏的**：`ShellHandle.resize()` 空实现（Citadel `withPTY` 不暴露 resize）→ PTY 恒为 `openShell(columns:80, rows:24)`，且 `ensureAttached()` 不调 `termSync`。修需 NIOSSH `WindowChangeRequest` + `channel.triggerUserOutboundEvent` 并补 pendingSize。**本机无 swift 工具链，改 iOS 原生一律无法编译验证**。两端共享页 `terminal/index.html` 保持逐字一致（除 Android 独有诊断日志）。iOS 密码走钥匙串、协议用 Picker → 无需改。
- **KeyVault（Android Keystore）**：`generateKey()` 对已存在的 alias 是**覆盖**不是复用 → 必须 `getEntry(alias,null)` 先查、只在缺失时生成；否则每调一次 encrypt/decrypt 就换一把钥匙、密文当场作废（密码 / 私钥 PEM / 密钥口令**共用同一把**；连「打开编辑框回显」都会调 decrypt 而废掉别的条目）。异常必须留痕。历史遗留密文救不回，用户需重输一次。
- **Compose 窄屏行内长文本会撑爆行高**：Row 溢出时末尾 Text 被压成 0 宽逐字换行，行高 48dp → 300dp+（现象=巨大留白 + 文案连语义节点都消失）→ **只给较长的那条**加 `Modifier.weight(1f, fill = false)`（同行多个 weight 会平分宽度反而挤坏；`fill=true` 强制占满），放不下就拆行。**含输入框的 AlertDialog 必须 `imePadding()` + `DialogProperties(decorFitsSystemWindows=false)`**，否则键盘盖住底部按钮且点不到（`uiautomator dump` 不含 IME 窗口，坐标必须截图交叉验证）。
- 主题：Compose 版 `Theme.kt`/`Common.kt`（三挡 auto/dark/light，终端区恒深 OLED）；PC 设计稿 `docs/client-ui/*.html` 双主题（ab-theme + localStorage）。
- 待办：iOS 同套 token 重构；PC 端 server-hub 独立窗口（设计已定稿）。
