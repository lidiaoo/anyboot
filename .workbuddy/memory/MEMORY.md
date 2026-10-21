# AnyBoot 项目长期备忘

索引 + 铁律。细则查当日日志 `2026-10-*.md`；桌面/打包/沙箱 → `MEMORY-detail-desktop.md`；托盘 → `MEMORY-detail-tray.md`；窗口 → `MEMORY-detail-windowing.md`；移动端 → `MEMORY-detail-mobile.md`。

## git
- **commit 我做、push 用户做**（本机无 GitHub 凭据）。只提交本轮相关文件；`src-tauri/src/hub*.rs`、`ui/hub.html`、`docs/client-ui/`、`debx/` 属 PC 端 server-hub 另一条线，别混进移动端提交。
- **Rust 改动提交前必须至少 `cargo check` 一次**（E0425 教训：曾提交编译不过的 gpu_compat.rs）。**Rust 子模块不继承父模块作用域**——lib.rs（crate 根）的私有 const 在子模块里裸用名字必炸，`use crate::XXX;` 引入即可（crate 根私有 item 对后代模块可见，不必改 pub(crate)）。细则见 2026-10-18.md。

## 桌面端
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
- **`ServersActivity` 是 `exported=false`** → `monkey … LAUNCHER 1` 再走抽屉（☰ → 🖧 服务器）；**顶栏图标点了没反应先怀疑弹着模态对话框**；**别对该应用用 `adb shell pm clear`**（清掉用户配好的跳板机，密码没别处备份）。
- **`Task` 加字段必须同步改 `resume()` 的逐字段拷贝**，漏一个整道续传校验静默变死代码。
- **别拿 APK 体积当变更判据** → 解 dex 搜符号。待办：iOS 同套 token 重构；PC server-hub 独立窗口。

## 远端管理（server-hub）：认证层铁律
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
