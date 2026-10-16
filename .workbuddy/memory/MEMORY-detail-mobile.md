# 移动端细则（Android / iOS）

入口在 `MEMORY.md`。构建 skill：`android-gradle-compose-build` / `android-apk-lite-build` / `android-compose-ui-debug` / `android-sftp-fixture`。

## 构建
- Android 已迁 **Jetpack Compose**（业务 Java 类保留）。env：`GRADLE_USER_HOME=~/.cache/ab-gradle`、`JAVA_HOME=~/.sdkm/links/java`、`ANDROID_HOME=~/Android/Sdk`、`ANDROID_USER_HOME=~/.android`、`HOME=/home/jianghh`；AGP 8.7.3 / Gradle 8.11.1 / android-34。
- **`ulimit -v` 4G 硬限** → `_JAVA_OPTIONS="-Xmx768m -Xms64m -XX:MaxMetaspaceSize=512m"`（只放大 metaspace，heap 仍压着）。
- 交付包 `mobile/AnyBoot-remote-v1.2-compose-debug.apk`。

## 驱动与坑（`~/.cache/ab-droid/drv.py`）
- `dump` / `rows` / `find` / `tap` / `tapxy`。dump 失败会留旧 `/sdcard/ui.xml` → 先删远端文件，判据看输出里有没有 `dumped to`。
- `tap` 是子串匹配，会命中「引用该符号」的正文 → 底部导航一律 `tapxy`（1080 宽五格中心 x=100/320/540/760/980, y=2243）。
- **单次 `find`/`tap` 要拉一份 dump，约 2~3 秒**；对几秒就传完的传输来不及按「暂停」→ 先量一次坐标，之后直接 `input tap x y`。
- dump 看不到图标（`content-desc`）→ 用截图核。模拟器 `emulator-5554` 1080x2400 @420。
- **`ServersActivity` 是 `exported=false`**，`am start -n` 必抛 SecurityException → `monkey -p com.anyboot.remote -c android.intent.category.LAUNCHER 1` 再走抽屉（☰ → 🖧 服务器）。
- **顶栏图标点了没反应，先怀疑弹着模态对话框**：主界面在遮罩下，dumpsys/dump 里看不到 TopBar 节点，应用窗口 bounds 明显小于屏幕（如 `[120,63][960,2337]`），灰色那圈是被压暗的主界面而非桌面。`pm clear` 后 MainActivity 会自动弹「连接 AnyBoot 服务端」添加框，正好踩这个坑。
- ⚠️ **别对应用用 `adb shell pm clear`**：会清掉用户配好的跳板机（密码只以密文存在应用里，没别处备份）。
- **删掉最后一台服务端 → App 被锁在「连接 AnyBoot 服务端」**：MainActivity 的 `firstRun` 分支**不给「取消」按钮、`onDismissRequest` 也被 `if (!request.firstRun)` 拦住**，退无可退，只能保存一台才出得来。想绕过它去别的页（如传输页）也不行 —— `ServersActivity` 不导出（见上）。此时要清数据只能 `adb shell run-as com.anyboot.remote rm -f files/transfers.json`（debug 包 run-as 可用，比 `pm clear` 精确，不碰跳板机配置）。
- **清测试残留**：`run-as` 删 `files/transfers.json`（任务记录）+ `rm -f /storage/emulated/0/Android/data/com.anyboot.remote/files/Download/*`（下载产物）。靶机侧 `~/.cache/ab-droid/sftp-root/` 那几个 8/16/18 MiB 的测试文件加起来 45 MB，记得也删（靶机下次起会自己重铺）。

## 结构
- 服务器模块：标题栏（齿轮=终端设置 / ＋=添加）+ 五页底导 **服务器 / 会话 / 密钥 / 传输 / 文件**（0 起下标，默认页 0）。

## 传输中心 `TransferManager.java`
- 静态任务表 + 单槽 Observer（`Common.kt` 的 `TransferEvents` 转主线程），跑在共享会话的独立 sftp 通道上。
- **取消/暂停依据：jsch `SftpProgressMonitor.count(long)` 收到的是本块增量（不是累计），返回 false 就地把循环 break，不抛异常、不用断共享会话**（`javap -c` 核过）；代价是留半截文件。
- Java 侧不能调 Kotlin 顶层函数（`humanSize`）→ 自备 `humanBytes`。
- **暂停 / 继续 = 同一机制 + 留不留半截文件**：`stop`（NONE/CANCEL/PAUSE）取代 `cancelRequested`；`isPending()`（进行中/已暂停）与「已收尾」分开；`resume()` **新建一条任务**并把原暂停记录标 `STATE_SUPERSEDED`（否则每点一次「继续」留一行僵尸「已暂停」）；`pendingFor(direction, remotePath)` 挡重复发起（两条任务写同一文件会互相踩坏）。
- **续传直接用 jsch `ChannelSftp.RESUME`**（`get(src,dst,monitor,RESUME)` 路径变体自己比对本地长度；`put(input,dst,monitor,RESUME)` 自己 `skip` 源流 + APPEND），**不用升 jsch**。
- **两个方向的断点上报方式不同**：下载 jsch 在 init 后补一次 `count(已下长度)`（当基线吃掉，否则速度虚报）、上传不报（自己 `lstat` 远端喂 `initialTransferred`）。传完要 `settleAtTotal()`（本地已跟远端一样大时 jsch 一个 count 都不报，进度条会停中间）。
- **任务落盘 `<filesDir>/transfers.json`**（`org.json`，不加依赖）：`ServersActivity.onCreate` 里 `restore(this)`。RUNNING 的记录装回后一律降级 `PAUSED + interrupted=true`，文案「上次没传完（App 关掉了），点「继续」接着传」；`transferred` 用**本地半截文件的真实长度**纠正（进度落盘有 3 秒节流，会偏小）。写入走 `writeText`：tmp → `flush()` → `getFD().sync()` → `renameTo`。`persist()` 挂 `register/finish/clearFinished`，`persistProgress()` 挂 `Progress.count()`（3 秒节流）。
- **`startDownload` 必须提前算 `localPath`**（`defaultTarget`）：原先只在 `runDownload` 里算，于是被杀在 RUNNING 状态的记录 `localPath` 为空 → 装回时找不到半截文件，只能显示「已传 0 B」、`total` 也是 -1。
- 上传源跨进程要能读：`OpenDocument` 结果调 `takePersistableUriPermission(uri, FLAG_GRANT_READ_URI_PERMISSION)`（拿不到只记日志，不拦本次上传）。
- **续传前核对远端**：下载起手记 `remoteSize + remoteMTime`，续传前 `lstat` 重取，任一不同就删半截文件整份重下（文案「（服务器上的文件变过，已改为重新下载）」）；上传暂停时记 `remoteSize = transferred`，续传前远端更大就整份重传（文案「（服务器上的文件比预期大，已改为重新上传）」）。没有这道闸，追加只会拼出一个**看着完整、内容错位**的文件（长度对、md5 全错），比报错难查得多。命中后要把基线刷新成新文件，否则再暂停一次会重复整份重传。
- **⚠️ `Task` 新增字段时，必须同步改 `resume()` 里的逐字段拷贝。** 本轮就漏了 `remoteSize`/`remoteMTime` → 新任务是 -1 → 校验条件恒假 → **整道校验静默变死代码**（实机证据：续传记录 `"remoteSize":-1`，且在已变过的远端文件上接着追加到「17.0 MB / 17.0 MB」）。编译器不会提醒这种漏字段。

## 终端页
- WebView + xterm.js，**尺寸绝不能依赖百分比高度**（解析成 0 → 兜底 1 行 → 回车无输出）；唯一来源是 JS `syncSize()`，会话绑定与 `onResume` 必须 `termSync()` 重报 PTY。诊断 tag **ABTERM**。
- 偏好键 `terminal_font_size`(14, 8~28) / `terminal_auto_keyboard`(false) / `terminal_key_bar`(true)，改完 `onResume` 走 `applyStoredSettings()` 热套用。
- **iOS PTY 尺寸是坏的**（`ShellHandle.resize()` 空实现 → 恒 80x24）；**本机无 swift 工具链，iOS 原生改动一律无法编译验证**。共享页 `terminal/index.html` 两端逐字一致。

## 密钥
- **KeyVault**：`generateKey()` 对已存在 alias 是**覆盖**不是复用 → 必须先 `getEntry(alias,null)` 查；否则每调一次 encrypt/decrypt 就换钥匙、密文当场作废（密码 / 私钥 PEM / 口令共用一把）。历史密文救不回。
- **Ed25519 本机生成不了、当前不能修**：jsch 0.2.20 无 EdDSA 落盘 API，缺 BC 类加载即炸；但加 BC 或升 jsch ≥0.2.27 **会改变主机密钥协商**，而 TOFU 是**按服务器记一条指纹** → 老装机全部 `reject HostKey`。故 `generateKey` 前置拦截给可读提示；要真支持须先改 TOFU。

## Compose 坑
- 窄屏行内长文本 Row 溢出时末尾 Text 压成 0 宽逐字换行（行高 48dp → 300dp+）→ **只给较长的那条**加 `Modifier.weight(1f, fill = false)`。
- 含输入框的 AlertDialog 必须 `imePadding()` + `DialogProperties(decorFitsSystemWindows=false)`（dump 不含 IME，坐标须截图交叉验证）。
- **新 Activity 取色必须包在 `AppTheme{}` 里**：`LocalPalette` 是 `staticCompositionLocalOf`，默认值 `paletteOf(Application())` 没有 base context，在外面读 `LocalPalette.current` 直接 NPE。
- **独立 composable 吃普通可变字段（非 State）会被跳过重画**：判据是「表头变了而行没变」，必须再喂一个会变的 tick 参数；行**内联**在页面函数里就不受影响。
- Kotlin `return` 的续行若以 `+` 开头会被当一元 `unaryPlus` → 编译失败，用字符串模板。

## 产物核验
- **别拿 APK 体积当「有没有变更」的判据**：本轮加约 320 行（持久化 + 校验）后，clean 包体积与上一轮的暂停/续传包**一模一样**（都是 9,361,632），只有 md5 不同；增量构建当时还产出过 9,685,741 的一份。逐条目比对过：解压后 154 个文件、内容 md5 全同、压缩后总量也全同，差的 324 KB 全在打包层（zip 条目顺序 / 对齐 / 签名块）。
- 核变更一律解 dex 搜符号；ASCII 串 `strings` 能捞，**中文串 `strings` 认不出**，要在字节里按 UTF-8 找。

## 文件页 / SftpOps
- **jsch 同一条 sftp 通道绝不能既 `get()` 又 `put()`**：会死锁——只写出一个数据包（2 MiB 源只落 32,755 B）后 `read()` 永不返回。修法是开两条通道（`SftpOps.withTwo()`：reader 只读 / writer 只写）；实测两通道直连对流 5.3s，比「先落本地临时文件再上传」(10.4s) 快一倍还不占磁盘。对照探针在 `~/.cache/ab-droid/SftpCopyProbe.java`（A/B/C/D 四组，可重跑复现）。
- 手机 WebView 排查网页弹窗/状态：`uiautomator dump` 看不到 WebView 内容，走 CDP——`adb forward tcp:9222 localabstract:webview_devtools_remote_<pid>` + `~/.cache/ab-droid/cdp.py eval "<js>"` 直读页面变量与 computed style。websocket-client 必须 `suppress_origin=True`，否则 403。

## 七屏重排（2026-10-17，commit 92ade76）
- 底部导航设计稿四项（服务器/终端/密钥/文件）；**会话/传输页整体保留，入口在顶栏 ⋮**；「终端」导航项=跳最近活会话。
- **键帽写 socket 必须离网线程**：`sendRaw` 曾在主线程调 `TerminalSession.write`（真网络 I/O）→ NetworkOnMainThreadException，会话关成「发送失败」。xterm.js 键盘输入走 WebView 桥线程所以一直没暴露；键帽一点就炸。修法：`keyWriter` 单线程执行器（daemon）保序写出。新加的任何往 session 写字节的 UI 入口都得走它。
- 编辑弹窗已改 ModalBottomSheet（skipPartiallyExpanded+自绘把手+imePadding）；地址/端口两格输入、保存拼回 host:port，**存储仍是 host:port 合一**（persistJump 没动）。
- 设计稿的「经由跳板机」下拉/三格「跳板机」没做——JumpServer 无 via 字段，不做假数据。
- **Compose 坑**：拆出去的子 Composable 里 `Modifier.weight()` Unresolved（RowScope 扩展）→ 加 `modifier` 参数在调用点传；`ManagedActivityResultLauncher` 的结果泛型要写 `Uri?`；ModalBottomSheet 要 `@OptIn(ExperimentalMaterial3Api::class)`；material icons 不能全限定 `Icons.Filled.Info`（Icons.Filled 是对象扩展），必须 import。
- 设计稿图标 material-icons-core 没有（显示器/机架/终端/钥匙/文件夹/文档/闪电/时钟/代码括号）→ `Icons.kt` Canvas 手绘（24dp 网格×grid() 缩放，线宽 1.8 圆头）。
- 本沙箱无 /dev/kvm（kvm 模块在但 mknod 被拒），模拟器死掉后起不回来；软模拟（-no-accel）API 37 卡死不出日志。AI 侧实机验证依赖用户环境已有模拟器。
