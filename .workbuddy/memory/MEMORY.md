# AnyBoot 项目长期备忘

索引 + 铁律。细则查当日日志 `2026-10-*.md`；桌面/打包/沙箱 → `MEMORY-detail-desktop.md`；托盘 → `MEMORY-detail-tray.md`；窗口 → `MEMORY-detail-windowing.md`；移动端 → `MEMORY-detail-mobile.md`。

## git
- **commit 我做、push 用户做**（本机无 GitHub 凭据）。只提交本轮相关文件；`src-tauri/src/hub*.rs`、`ui/hub.html`、`docs/client-ui/`、`debx/` 属 PC 端 server-hub 另一条线，别混进移动端提交。

## 桌面端
- 前端改动**必须强制 codegen**（删 `out` + `touch build.rs` 重编）；核对解 `out/tauri-codegen-assets/*`；**二进制里搜中文按 UTF-8 字节比**（HTML 资产是 brotli 压缩嵌入，裸搜原串必为 0，要拿 codegen 资产字节做精确匹配）。`data-page-node-id` 原样保留；文档路径写 `~`。
- **验证闭环全流程已沉淀技能 `~/.workbuddy/skills/anyboot-pc-ui-verify/SKILL.md`**（jsdom 姿势 / 资产哈希名与 zig 重链后新旧共存坑 / jammy 打包 env / 无头冒烟不可行）。要点：资产名是内容哈希，嵌入核验先在目标二进制里定位资产再验内容；curl 探活加 `--noproxy '*'`。
- **ui/*.html 里 id 严禁与 `window.xxx` 状态变量同名**：带 id 的元素会被 named access 挂成 window 同名属性（真值），`if (window.term)` 之类判空全失效——hub-term 黑屏教训（`id="term"`→`termHost`，已修）。
- **往主窗 hub.html 移植独立窗页面：id/class 全量前缀**（fb- 手法，见 2026-10-17 日志）；主窗「文件」页已是内嵌浏览器（双入口并存，独立窗靠 `fbOpenWin`/`hub_open_files_window` 唤起）。打包前 kill 陈旧打包进程（旧包用旧二进制）。
- 交付 = 测试 + release 构建 + 实机验证；构建前退出运行中的 anyboot；**md5 必核**。

## 托盘
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
- 平台能力/排错/验收边界都在 `docs/server-hub.md`；改这条线先跑 Windows 目标 `cargo check`（Linux 通过不代表 Win 通过）。
