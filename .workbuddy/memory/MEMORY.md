# AnyBoot 项目长期备忘

索引 + 铁律。细则查当日日志 `2026-10-*.md`；桌面/打包/沙箱 → `MEMORY-detail-desktop.md`；托盘 → `MEMORY-detail-tray.md`；窗口 → `MEMORY-detail-windowing.md`；移动端 → `MEMORY-detail-mobile.md`。

## git
- **commit 我做、push 用户做**（本机无 GitHub 凭据）。只提交本轮相关文件；`src-tauri/src/hub*.rs`、`ui/hub.html`、`docs/client-ui/`、`debx/` 属 PC 端 server-hub 另一条线，别混进移动端提交。
- **`.workbuddy` 三块待遇不同**（用户定案）：`memory/`（MEMORY.md + 每日日志）和 `probes/`（量窗口坐标的小工具）**入库**；`artifacts/`（UI 验证截图）**已 gitignore**——一次性现场证据，二进制只增不减、事后对不上改动。要把证据跟改动绑一起就写进 commit message。
- **Rust 改动提交前必须至少 `cargo check` 一次**（E0425 教训：曾提交编译不过的 gpu_compat.rs）。**Rust 子模块不继承父模块作用域**——lib.rs（crate 根）的私有 const 在子模块里裸用名字必炸，`use crate::XXX;` 引入即可（crate 根私有 item 对后代模块可见，不必改 pub(crate)）。细则见 2026-10-18.md。

## 桌面端
- **Windows 提权不闪窗：`ShellExecuteExW` + `SEE_MASK_NO_CONSOLE`（`sol_panel.rs::run_elevated_hidden`）**。
  `Start-Process -Verb RunAs` 会为子进程开控制台 → 蓝窗口一闪；给子进程加 `-WindowStyle Hidden`
  **不够**（只是创建后隐藏，10ms `EnumWindows` 轮询照样抓得到）；外层 Start-Process 又不能加
  `-WindowStyle`（PowerShell 禁止它与 `-Verb` 同用）。正解：`fMask=SEE_MASK_NOCLOSEPROCESS|SEE_MASK_NO_CONSOLE`、
  `nShow=SW_HIDE`、句柄 `WaitForSingleObject` + `GetExitCodeProcess`。提权进程里再调 `schtasks` 也不闪。
  改完必须实测（`cargo test --lib elevated_hidden_no_console -- --ignored` + 窗口采样器），别只看代码。
- **验证 PC 端 UI 的四条铁律**（见 `2026-10-18.md`）：① 必须用**计划任务跑 .bat** 启动（`set` 环境变量 + `start ""`），工具沙箱会杀 WebView2 浏览器进程 → 全窗口黑屏/卡死假象；② `PrintWindow` 抓 WebView2 恒黑，用 `GetDC(NULL)+BitBlt` 抓屏，读坐标要在 1:1 裁剪图上量；③ webview 不吃注入点击（hover 有效、点击无效），验证走 `ANYBOOT_TEST_OPEN_HUB=1` / 再 `ANYBOOT_TEST_OPEN_TERM=1` 钩子；④ 本机**没有 `strings` 命令**，静默返回 0 会骗人，用 Python 提串。
- **WebView2 CDP 调试端口**：`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 在运行时 155 已失效（wry API 参数优先）；用 `ANYBOOT_CDP_PORT=9223` + 所有 `WebviewWindowBuilder` 统一过 `lib.rs::apply_browser_args`。铁律（tauri#11144）：同一 data folder 的所有 webview 环境参数必须一致，不一致 = 0x8007139F 开不出窗；杀旧实例后等 5s 再启动。
- 前端改动**必须强制 codegen**（删 `out` + `touch build.rs` 重编）；核对解 `out/tauri-codegen-assets/*`；**二进制里搜中文按 UTF-8 字节比**（HTML 资产是 brotli 压缩嵌入，裸搜原串必为 0，要拿 codegen 资产字节做精确匹配）。`data-page-node-id` 原样保留；文档路径写 `~`。
- **验证闭环全流程已沉淀技能 `~/.workbuddy/skills/anyboot-pc-ui-verify/SKILL.md`**（jsdom 姿势 / 资产哈希名与 zig 重链后新旧共存坑 / jammy 打包 env / 无头冒烟不可行）。要点：资产名是内容哈希，嵌入核验先在目标二进制里定位资产再验内容；curl 探活加 `--noproxy '*'`。
- **ui/*.html 里 id 严禁与 `window.xxx` 状态变量同名**：带 id 的元素会被 named access 挂成 window 同名属性（真值），`if (window.term)` 之类判空全失效——hub-term 黑屏教训（`id="term"`→`termHost`，已修）。
- **往主窗 hub.html 移植独立窗页面：id/class 全量前缀**（fb- 手法，见 2026-10-17 日志）；主窗「文件」页已是内嵌浏览器（双入口并存，独立窗靠 `fbOpenWin`/`hub_open_files_window` 唤起）。打包前 kill 陈旧打包进程（旧包用旧二进制）。
- 交付 = 测试 + release 构建 + 实机验证；构建前退出运行中的 anyboot；**md5 必核**。

## 托盘
- **菜单有两套实现**：`grub_menu_nodes`（主窗/自绘窗/Linux dbusmenu）与 `build_tray_menu`（Win/mac classic 手写原生版）。**新增菜单项必须两边都加**——hub「远程管理…」当初只加进前者，classic 用户托盘上根本没有入口（2cd1168 修）。
- `trayMenuStyle` 启动定接线、切换需重启：classic = Win/mac muda 原生菜单 / **Linux 自研 `linux_sni` 全量 dbusmenu**；unified = Win/mac 自绘窗 / Linux SNI 导出（GNOME 壳菜单只两条，opened → 弹自绘窗）。
- 自绘窗落位：**unified 贴图标正下方居中、放不下翻上、绝不盖图标**；classic 上方 + 左对齐；`TrayCenter` 弃用。`grub_menu_nodes` **一份函数喂四处**（主窗/自绘窗/Linux dbusmenu/原生菜单）。其余（失焦看门狗、dbusmenu 信号与 `toggle-*` 定则、macOS 对话框走 `osascript`）见 detail-tray。

## 产品定则
- 默认值常量钉着（改值同步单测）：亮色/90%/`remoteAllowGuiAuth=true`/默认高度=整屏 2/3；`window_height==0`=没记过。落位 center=禁拖。
- 下次启动大字=级联全链；Rust 文案必须与前端 `nextBootText`/`rowLabel` 一字不差。
- 忙闸必须有放闸路径（判据不能是「命令返回成功」）；`POWER_RELEASE_MS=15s` 放闸；远程动作=本机点头，灰掉不藏。
- 顶部标题栏 = 电源三键 + 重启/退出；`restart_app` 顺序 **stop_http_server → release_instance_lock → app.restart()**。单实例 `anyboot.lock`（守卫放 `setup` 最前）。

## 移动端（当前主战场）
**细则全部在 `MEMORY-detail-mobile.md`**（构建 env / drv.py 坑 / TransferManager 暂停续传与持久化 / 终端页 / KeyVault / Compose 坑 / 产物核验）。只记三条最容易忘的：
- **输入法相关**：`hub.html` 里所有全局 `keydown` 快捷键（Enter 保存 / Esc 关弹窗）必须先判
  `isImeKey(e)`（`e.isComposing || e.keyCode === 229`）—— 否则中文输入法回车选词会触发保存，
  弹窗反复重建，看着就是「输入框一直闪」。移动端 Compose 的数字输入框**不能**把 `effective*()`
  算出来的默认值当 `value` 回填（删一个字符就被弹回来，光标乱跳），要用原始字段 + `placeholder` 显示默认值。
- **`ServersActivity` 是 `exported=false`** → `monkey … LAUNCHER 1` 再走抽屉（☰ → 🖧 服务器）；**顶栏图标点了没反应先怀疑弹着模态对话框**；**别对该应用用 `adb shell pm clear`**（清掉用户配好的跳板机，密码没别处备份）。
- **`Task` 加字段必须同步改 `resume()` 的逐字段拷贝**，漏一个整道续传校验静默变死代码。
- **别拿 APK 体积当变更判据** → 解 dex 搜符号。待办：iOS 同套 token 重构；PC server-hub 独立窗口。
- **地址 = 一行 IP + 可选多个 MAC**（`ServerProfile.parseAddressInput`）：`192.168.0.120:12300 AA:BB:CC:DD:EE:FF 11:22:...`。
  解析规则三处必须一致（移动端 Java / `hub.html` JS / Rust `hub::is_mac`）：**先认 IPv4 再认 MAC**
  （`192.168.100.100` 去掉点正好 12 位十六进制，不先排除会被当 MAC）。MAC 只用于唤醒，不参与连接。
  电源操作：移动端 `PowerOps`（直连/隧道调 `/api/power`）+ `WakeOnLan`（跳板机 SSH 或手机 UDP）。

## 远端管理（server-hub）：认证层铁律
- **服务器多地址模型**：`HubServer.addresses`（有序）+ `macs`（地址→MAC），`normalize()` 末尾把
  `addresses[0]` 回写 `host`/`port` —— **连接永远只用 host/port**，所以 ssh/sftp/隧道零改动；
  探活（`test_direct_tcp`）按顺序试全部地址。电源在 `hub_power.rs`（`hub_server_power` / `hub_wake_server`），
  前端在服务器详情「电源」区。重启/关机/睡眠依赖目标机 `sudo -n`；唤醒：有跳板机就登跳板机发 WOL，否则本机 UDP 广播。
- **局域网电源路由（sol）**：`HubServer.power_routes: Vec<PowerRoute>`（动作/广播地址/端口/MAC 子集/启用），
  空 = 没配。`hub_power::dispatch()` 先查该动作有没有路由：有 → 发 UDP（魔术包 + `ANYBOOT-POWER:<动作>` 尾标，
  可经跳板机 python3），没有才回落 SSH / 标准 WOL。**sol 按端口区分动作**，所以端口是用户配的，
  不同动作可以各自绑端口；不装 sol 只有唤醒（WOL）能生效。非唤醒动作只发一个目标、一个包
  （重复到达 = 重复执行）。移动端同构：`ServerProfile.powerRoutes` + `WakeOnLan.sendRoutes`。
  改路由语义要同步 `hub.html`、`ServerProfile.java`、`MainActivity.kt` 三处。
  路由还有 `burst` / `retry`（间隔 200ms），**每个动作都能单独设**：0 = 按动作默认（唤醒 10/10、
  重启关机睡眠 1/1，因为发几个包 sol 就执行几次）。归一化时 0 要原样保留（别烤成固定数字）。
  **端口只在配了路由时生效**；没配路由的老路固定 9 + 7。
  路由还有 `http_fallback`（**默认 true**，serde default_true / 手机 optBoolean true / 前端 `!== false`）：
  sol 包没发出去就改调这台电脑自己的 /api/power。**唤醒也能兜** —— 服务端把 `action=wake` 解析成
  `RemoteCommand::WakeAck`（HTTP 能送达 = 机器本来就醒着，直接回执不做动作），别再按 400 处理。
  **电脑档案（remote_power）没配唤醒路由时走默认 WOL 广播**（10 包/重试 10，plan_routes+send_plan），
  不是调 HTTP —— 机器关着接口是死的。
  地址编辑是「快速识别框 + 已添加列表 + 手动添加」三段式（两端同构，状态是 `[{address, macs[]}]`），
  保存时才展开成 `addresses` + `macs` 映射。`normalizeAddress(raw, port, strict)`：strict 下无点无端口的
  单词不当地址（整段粘贴里的备注）。改地址解析规则要同步移动端与 `hub.html::parseAddressInput`。
- **唯一入口 `hub_tunnel::apply_auth`**：凭据（密钥 `-i` / 存档密码 askpass）→ 跳板 ProxyCommand → 复用策略。
  新加任何要跑 ssh/sftp 的地方都挂它，别再手写 `BatchMode/ControlPath`。
- **askpass = 本程序自身**：`SSH_ASKPASS=<exe>`、`SSH_ASKPASS_REQUIRE=force`、`ANYBOOT_ASKPASS_SERVER=<id>`，
  **全是环境变量**（`SSH_ASKPASS_REQUIRE` 写成 `-o` 会被 Win32-OpenSSH 判非法）。ssh 传回提示语，
  按提示语里的 `user@host` 区分目标机/跳板机 → 所以 ProxyCommand **不要**再加 `env VAR=…` 前缀。
- **`main.rs` 的 askpass 分支必须排在 `cli_requested` 之前**：argv 是提示语，不像命令，否则会被当 GUI 启动。
- **Windows 没有 ControlMaster**（Win32-OpenSSH 实测 `getsockname failed: Not a socket`，不落 socket 文件）：
  Unix 复用 master、`BatchMode=yes`；Windows 每次自己认证，且**只有密钥能开 BatchMode**（它会连 askpass 一起禁）。
- **`sftp -b` 会自己把 BatchMode 打开** → 密码认证必死（`No more authentication methods to try`，文件列表恒空）。
  `apply_auth` 在「有密码没密钥」时显式补 `-o BatchMode=no`；新加 sftp 调用别再自己拼参数。
- **在 IPC 回调里建 webview 必须走 async 命令**：`#[tauri::command] fn` 是同步的，跑在 WebView2 的
  `WebMessageReceived`（UI 线程）上，在同一线程重入建新 webview → 窗口建得出来但页面**永远停在白底首帧**
  （不导航、不执行 JS），用户看到「空白窗口卡死」。改成 `async fn` 即在 tokio 线程创建，正常加载。
  本轮 `hub_open_term_window` / `hub_open_files_window` / `hub_open_keys_window` 三个都踩了这个坑。
- 平台能力/排错/验收边界都在 `docs/server-hub.md`；改这条线先跑 Windows 目标 `cargo check`（Linux 通过不代表 Win 通过）。
- **子进程一律走 `crate::command(program)`**（`lib.rs`，带 `CREATE_NO_WINDOW`）。hub 系列曾直接
  `std::process::Command::new` → 文件列表/上传下载/远端复制每次都闪黑框。终端是 ConPTY，无需处理。
- **下载目录空值 = 系统下载目录**（`hub_download_dir_default`：Windows 已知文件夹 / mac `~/Downloads` /
  Linux XDG）。旧版本的 `~` 在前端也当空（`normalizeDownDir`）；`expand_home` 要读 `HOME` **和**
  `USERPROFILE` —— Windows 没有 `HOME`，只读它会让 `~` 落到进程工作目录。
- **远程管理入口现在四处**：托盘、「远程管理」主窗标题栏、终端/文件/密钥三窗顶栏（都调 `hub_open_window`，
  必须 async）。
- **同步 `#[tauri::command]` = 跑在 UI 线程**：命令体里出现 openpty / 起 ssh·sftp 子进程 / TCP 连接 /
  `thread::sleep`，用户看到的就是「点一下终端整窗卡死」。定则：写成 `pub async fn` 包装 +
  真正实现在 `foo_blocking` 里走 `spawn_blocking`。守卫单测 `src/blocking_guard.rs` 扫 `src/*.rs`
  同步命令体（含间接调用），违规即 `cargo test` 红；转义词只有一个：`// blocking-ok: 理由`。
  终端上行故意保持同步 —— 它只往有界队列 `try_send`，`write_all` 在专属写线程上。
- **传输状态只有一个真相源**：`XferTask.state` 五态（running/paused/done/failed/abandoned），
  变更一律 `app.emit` **广播**（别用 `emit_to(窗口)`：主窗没开时直接丢事件），
  快照 `hub_sftp_transfer_list` = running + 断点清单 + `recent`（最近 30 条终态）。
  进程退出必须 `detach_running`（按 `Arc::ptr_eq` 只摘本进程）—— 漏了就是清单永远报
  `running` 的僵尸卡，这是实测复现的坑。中断报 `paused` 不报 `failed`（要能续传）。
  批量 `key=null`：pause 全部 running / resume 逐条全部 / abandon 清 running+paused 全部。
- **验证脚本别用带中文注释的 bat**：UTF-8 中文 + LF 行尾会让后面的 `set` 整行失效，hook 静默不跑，
  看起来像功能坏了（白查半天）。启动一律用纯 ASCII 启动器（Python + DETACHED_PROCESS），
  且 `[hub-term]` 会把 ssh 报错原文抄进 stderr，连不上时先看它。

## SOL 面板
- **`~/sol` 是上游压缩包解压出来的，禁止改里面的文件**（`install.ps1` / `install.cmd` / `install.sh` /
  `sol.exe` / `README*` / `sol.schema.json` / `sol-example.yaml` / `install-windows.yaml`）。
  那是上游产物，本项目只做**调用方**：改脚本 = 下次升级覆盖 + 排查时分不清是谁的问题。
  要改行为只能改 `src-tauri/src/sol_panel.rs` 的调用方式（环境变量、stdin 喂菜单、参数）。
- 区分「我改的」和「脚本自己写的」：运行 `/sol` 安装脚本会重写 `install.json`（台账）、`install.yaml`、
  `sol.yaml`、`install.log`、`elevated-install.progress`、`skills/` —— 这些是运行产物，正常。
  核验有没有手改过源文件：`ls -la --time-style=+%m-%d\ %H:%M:%S ~/sol`，源文件时间戳应全部等于
  解压时间（本机 = `10-21 09:03:30`）。

## 构建（Windows 本机）
- 项目内 `src-tauri/target` 整棵树被标了 Low 完整性 + `Everyone:(DENY)(S,DC)`，导致任何
  build script 写 `OUT_DIR` 都 panic `Os { code: 5, PermissionDenied }`（serde / serde_core 全军覆没）。
  **别在原地修**，直接换 target 目录。
- **C 盘空间极小（<3G）**：别用 %TEMP% 当 CARGO_TARGET_DIR（debug 编译直接 ENOSPC，连 shell 都起不来）。
  一律 `CARGO_TARGET_DIR=J:/tmp/anyboot-target cargo build/test`（J 盘 90G 空闲，本地 NTFS）。
  **别用 E 盘（已满）和 F 盘（是 NAS 网络共享，build script 写不进去）**。产物 `anyboot.exe` + `anyboot_lib.dll` 要**一起**复制
  到 `C://Users//jianghh//AppData//Local//AnyBoot`（先备份 `*.bak-日期`）。
- 用户跑的是安装版 `C:/Users/jianghh/AppData/Local/AnyBoot/anyboot.exe`，前端的 `ui/*.html`
  是编译期嵌进 exe 并**压缩**的：改完必须重编 + 覆盖 exe 才生效，`grep` 二进制里搜不到标记字符串，
  别拿 grep 结果判断「有没有打进去」。

## 2026-10-22 端口坑补充
- 12300 落在 Windows 保留段（hns/WSL 开机占用）时 bind 报 WSAEACCES(10013)，服务静默起不来，本机 iframe 白屏。排查：netsh int ipv4 show excludedportrange protocol=tcp；恢复：netsh int ipv4 set dynamicport tcp start=49152 num=16384 后重启 winnat（管理员）。
- hub.html 的「本机」chip 是静态 HTML，不在 renderPcChips 循环里，必须单独绑 click（这次「本机点不动」的根因）。

