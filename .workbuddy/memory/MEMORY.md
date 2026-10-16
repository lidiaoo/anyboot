# AnyBoot 项目长期备忘

**细则：** 当日日志 `2026-10-*.md`；桌面/打包/沙箱 → `MEMORY-detail-desktop.md`；托盘/dbusmenu → `MEMORY-detail-tray.md`；窗口/贴边 → `MEMORY-detail-windowing.md`。

## git
- **commit 由我做，push 由用户在 IDE 做**（本机无 GitHub 凭据）。只提交本轮相关文件 —— `src-tauri/src/hub*.rs`、`ui/hub.html`、`docs/client-ui/`、`debx/` 属 PC 端 server-hub 另一条线，别混进移动端提交。

## 桌面端（细则见 detail-desktop）
- 前端改动**必须强制 codegen**（删 `out` + `touch build.rs` 重编）；核对解 `out/tauri-codegen-assets/*`；**二进制里搜中文用 `buf.includes(串,"utf8")`**。
- 交付 = 测试 + release 构建 + 实机验证；构建前退出运行中的 anyboot；**产物 md5 必核**。
- `ui/index.html` 的 `data-page-node-id` 原样保留；标题字号别挂在会搬家的容器下；文档路径写 `~`。

## 托盘（细则见 detail-tray）
- `trayMenuStyle` 启动时定接线、切换需重启：classic = Win/mac muda 原生菜单 / **Linux 自研 `linux_sni` 全量 dbusmenu**；unified = Win/mac 自绘窗 / Linux SNI 导出 dbusmenu（GNOME 壳菜单只两条，opened → 弹自绘窗）。
- 自绘窗落位：**unified 贴图标正下方居中、放不下翻正上方、绝不盖图标**；classic 上方 + 左对齐。`TrayCenter` 弃用；失焦看门狗 250ms；落位全手算。
- `grub_menu_nodes` **一份函数喂四处**（主窗 / unified 自绘窗 / Linux dbusmenu / Win·mac 原生菜单），改它三头都变；行文字**箭头在前、图标在后**。
- dbusmenu 死线：信号从 `MENU_PATH` 发、回显走 `ItemsPropertiesUpdated` 差分、签名 `a(ia{sv})a(ias)`、**禁带 `toggle-*`**（勾选画进 label `◉`/`○`）。
- macOS 原生对话框不可用（rfd 无 parent 阻塞 ~30s）→ 走 `osascript`。

## 产品定则
- 默认值常量钉着（改值同步单测）：亮色 / 90% / `remoteAllowGuiAuth=true` / 默认高度=整屏 2/3；`window_height==0` = 「没记过尺寸」。落位 center=禁拖；锁定大小只禁手拖缩放。
- 下次启动大字=级联全链；Rust 侧文案必须与前端 `nextBootText`/`rowLabel` 一字不差。
- 忙闸必须有放闸路径（判据不能是「命令返回成功」）；`POWER_RELEASE_MS=15s` 世代号放闸；被闸点击必须 record_action。远程动作=本机点头（pending+授权窗），灰掉而不藏。
- 顶部标题栏 = 电源三键 + 重启/退出程序；`restart_app` 顺序固定 **stop_http_server → release_instance_lock → app.restart()**。
- 单实例 `anyboot.lock`（`File::try_lock`，守卫放 `setup` 最前）；tiny_http Windows `stop()` 必须自连接轮询。

## 移动端（mobile/）——当前主战场
- Android 已迁 **Jetpack Compose**（业务 Java 类保留）。构建 env：`GRADLE_USER_HOME=~/.cache/ab-gradle`、`JAVA_HOME=~/.sdkm/links/java`、`ANDROID_HOME=~/Android/Sdk`、`ANDROID_USER_HOME=~/.android`、`HOME=/home/jianghh`；AGP 8.7.3 / Gradle 8.11.1 / android-34。**`ulimit -v` 4G 硬限** → 加 `_JAVA_OPTIONS="-Xmx768m -Xms64m -XX:MaxMetaspaceSize=512m"`。skill：`android-gradle-compose-build` / `android-apk-lite-build` / `android-compose-ui-debug`。
- 驱动脚本 `~/.cache/ab-droid/drv.py`（`dump`/`rows`/`find`/`tap`/`tapxy`）。坑：dump 失败会留旧 `/sdcard/ui.xml`（先 rm，判据看 `dumped to`）；`tap` 子串会命中引用该符号的正文（底部导航一律 `tapxy`，1080 宽五格中心 x=100/320/540/760/980, y=2243）；dump 连续调会饿死 UI（进度数字看着不动）；dump 看不到 `content-desc`（图标）→ 截图核。模拟器 `emulator-5554` 1080x2400 @420；界面归属以 `dumpsys window|grep mCurrentFocus` 为准（screencap 滞后）。
- **`ServersActivity` 是 `exported=false`，`am start -n` 必抛 SecurityException** → `monkey -p com.anyboot.remote -c android.intent.category.LAUNCHER 1` 再走抽屉（☰ → 🖧 服务器）。
- 服务器模块结构：标题栏（齿轮=终端设置 / ＋=添加）+ 五页底部导航 **服务器 / 会话 / 密钥 / 传输 / 文件**（0 起下标，默认页 0）。
- **传输中心 `TransferManager.java`**（静态任务表 + 单槽 Observer，`Common.kt` 的 `TransferEvents` 转主线程）：跑在共享会话的独立 sftp 通道上。**取消依据：jsch 0.2.20 `SftpProgressMonitor.count(long)` 收到的是本块增量（不是累计），返回 false 就地把循环 break，不抛异常、不用断会话**（`javap -c` 核过）——代价是留半截文件，尽力清理。Java 侧不能调 Kotlin 顶层函数（`humanSize`）→ 自备 `humanBytes`。
- **传输暂停 / 继续 = 同一机制 + 留不留半截文件**：`stop`（NONE/CANCEL/PAUSE）取代 `cancelRequested`；`isPending()`（进行中/已暂停）与「已收尾」分开；`resume()` 新建任务并把原暂停记录标 `STATE_SUPERSEDED`（否则留僵尸行）。**续传直接用 jsch 的 `ChannelSftp.RESUME`**（`get(src,dst,monitor,RESUME)` 路径变体自己比对本地长度；`put(input,dst,monitor,RESUME)` 自己 `skip` 源流 + APPEND），不用升 jsch。**两种方向的断点上报方式不同**：下载 jsch 会在 init 后补一次 `count(已下长度)`（当基线吃掉，否则速度虚报）、上传不报（自己 `lstat` 远端喂 `initialTransferred`）。`pendingFor()` 挡重复发起（两任务写同一文件会踩出坏文件）。任务表是进程内静态的，**杀进程后暂停记录会丢**（跨进程续传要先做持久化）。
- 终端页（WebView + xterm.js）**尺寸绝不能依赖百分比高度**（解析成 0 → 兜底 1 行 → 回车无输出）；唯一来源 JS `syncSize()`，会话绑定与 onResume 必须 `termSync()` 重报 PTY。诊断 tag **ABTERM**（onCreate 一行打全 dark/fontSize/keyBar/autoKeyboard，核对设置是否吃进去最快）。终端偏好键：`terminal_font_size`(14,8~28) / `terminal_auto_keyboard`(false) / `terminal_key_bar`(true)，改完 `onResume` 走 `applyStoredSettings()` 热套用。
- **iOS PTY 尺寸是坏的**（`ShellHandle.resize()` 空实现 → 恒 80x24）；**本机无 swift 工具链，改 iOS 原生一律无法编译验证**。共享页 `terminal/index.html` 两端逐字一致。
- **KeyVault**：`generateKey()` 对已存在 alias 是**覆盖**不是复用 → 必须 `getEntry(alias,null)` 先查；否则每调一次 encrypt/decrypt 就换钥匙、密文当场作废（密码 / 私钥 PEM / 口令共用一把）。历史密文救不回，用户需重输一次。
- **Ed25519 本机生成不了、当前不能修**：jsch 0.2.20 无 EdDSA 落盘 API，缺 BC 类加载即炸；但加 BC 或升 jsch ≥0.2.27 **会改变主机密钥协商**，而 TOFU 是**按服务器记一条指纹** → 老装机全部 `reject HostKey`。故 `generateKey` 前置拦截给可读提示；要真支持须先改 TOFU。
- **Compose**：窄屏行内长文本 Row 溢出时末尾 Text 压成 0 宽逐字换行（行高 48dp → 300dp+）→ **只给较长的那条**加 `Modifier.weight(1f, fill = false)`，放不下就拆行。**含输入框的 AlertDialog 必须 `imePadding()` + `DialogProperties(decorFitsSystemWindows=false)`**（dump 不含 IME，坐标须截图交叉验证）。**新 Activity 取色必须包在 `AppTheme{}` 里** —— `LocalPalette` 是 `staticCompositionLocalOf`，默认值 `paletteOf(Application())` 没有 base context，在外面读 `LocalPalette.current` 直接 NPE。**独立 composable 吃普通可变字段（非 State）会被跳过重画** —— 判据是「表头变了而行没变」，必须再喂一个会变的 tick 参数；行**内联**在页面函数里就不受影响。Kotlin `return` 的续行若以 `+` 开头会被当一元 `unaryPlus` → 编译失败，用字符串模板。
- **产物核验**：**别拿 APK 体积当「有没有变更」的判据** —— 本轮 +250 行只让 `classes3.dex` 变大 9KB，整包字节数一模一样（打包对齐把差值吃掉了），md5 才变了。核变更一律解 dex 搜符号。
- 主题：Compose `Theme.kt`/`Common.kt`；PC 设计稿 `docs/client-ui/*.html`。待办：iOS 同套 token 重构；PC server-hub 独立窗口。
