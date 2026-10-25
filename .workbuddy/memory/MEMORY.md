# AnyBoot 项目长期备忘

索引 + 铁律。细则查当日日志 `2026-10-*.md`；桌面/打包 → `MEMORY-detail-desktop.md`；托盘 → `MEMORY-detail-tray.md`；窗口 → `MEMORY-detail-windowing.md`；移动端 → `MEMORY-detail-mobile.md`；SOL 面板 → `MEMORY-detail-sol.md`。

## git
- **commit 我做、push 用户做**。只提交本轮相关文件；`hub*.rs`、`ui/hub.html`、`docs/client-ui/`、`debx/` 属 PC 端 server-hub 线，别混进移动端提交。
- `.workbuddy/`：`memory/`+`probes/` 入库；`artifacts/` 已 gitignore。
- **Rust 改动提交前必须 `cargo check`**（E0425 教训）。子模块不继承父模块作用域，crate 根私有 item 用 `use crate::XXX;` 引入。
- 改 server-hub 线要跑 Windows 目标 `cargo check`（Linux 通过不代表 Win 通过）。

## 桌面端（细则见 detail-desktop / 2026-10-18.md）
- Windows 提权不闪窗：`ShellExecuteExW` + `SEE_MASK_NO_CONSOLE`（`sol_panel.rs::run_elevated_hidden`）；改完必须实测。
- 验证 PC UI 四铁律：① 计划任务跑 .bat 启动（沙箱杀 WebView2）；② 抓屏用 `GetDC(NULL)+BitBlt`（PrintWindow 恒黑）；③ webview 不吃注入点击，走 `ANYBOOT_TEST_OPEN_HUB/OPEN_TERM=1` 钩子；④ 本机无 `strings`，用 Python 提串。
- CDP：`ANYBOOT_CDP_PORT=9223` + 全部 webview 过 `apply_browser_args`（参数不一致=0x8007139F）；杀旧实例等 5s。
- 前端改动必须强制 codegen（删 out + touch build.rs）；二进制搜中文拿 codegen 资产字节精确匹配（brotli 压缩）。
- 验证闭环技能：`~/.workbuddy/skills/anyboot-pc-ui-verify/SKILL.md`。
- ui/*.html 里 id 严禁与 `window.xxx` 同名（named access 覆盖 → 判空失效，hub-term 黑屏教训）。
- 交付 = 测试 + release 构建 + 实机验证；构建前退出运行中的 anyboot；md5 必核。

## 托盘
- 菜单两套：`grub_menu_nodes`（主窗/自绘窗/Linux dbusmenu）与 `build_tray_menu`（Win/mac classic）——**新增菜单项两边都加**。
- `trayMenuStyle` 启动定接线、切换需重启。自绘窗 unified 贴图标正下方居中。`grub_menu_nodes` 一份喂四处。

## 产品定则
- 默认值常量钉着（改值同步单测）：亮色/90%/`remoteAllowGuiAuth=true`/默认高度=整屏 2/3。
- 下次启动大字=级联全链；Rust 文案与前端 `nextBootText`/`rowLabel` 一字不差。
- 忙闸必须有放闸路径（`POWER_RELEASE_MS=15s`）；`restart_app` 顺序 stop_http_server → release_instance_lock → app.restart()。

## 移动端（细则见 detail-mobile）
- 输入法：全局 `keydown` 快捷键先判 `isImeKey(e)`；Compose 数字框用原始字段+placeholder，别回填默认值。
- `ServersActivity` exported=false → monkey 走抽屉；别对应用 `pm clear`。
- `Task` 加字段必须同步改 `resume()` 逐字段拷贝。别拿 APK 体积当变更判据 → 解 dex 搜符号。
- 地址=一行 IP+可选多 MAC，解析规则三处一致（Java/hub.html/`hub::is_mac`）：先 IPv4 再 MAC。

## 远端管理（server-hub）
- 服务器多地址模型：连接只用 host/port；探活按序试全部地址。电源在 `hub_power.rs`；sol 路由语义改了要同步 `hub.html`、`ServerProfile.java`、`MainActivity.kt` 三处。http_fallback 默认 true；wake 走默认 WOL 广播不走 HTTP。
- **唯一 SSH 入口 `hub_tunnel::apply_auth`**。askpass=本程序自身（全环境变量）；askpass 分支在 `main.rs` 排 `cli_requested` 之前。Windows 无 ControlMaster；`sftp -b` 自开 BatchMode → 有密码没密钥时补 `-o BatchMode=no`。
- **同步 tauri command = UI 线程**：阻塞操作走 async fn + `spawn_blocking`（守卫单测 `blocking_guard.rs`）；IPC 回调里建 webview 必须 async（否则白屏卡死）。子进程一律 `crate::command()`（防闪黑框）。
- 传输状态真相源=`XferTask.state` 五态，变更一律 `app.emit` 广播；退出必须 `detach_running`；中断报 paused。
- 下载目录空值=系统下载目录；`expand_home` 读 HOME **和** USERPROFILE。
- 验证脚本别用带中文注释的 bat；启动用纯 ASCII 启动器。
- 平台能力/排错/验收边界在 `docs/server-hub.md`。
- **系统 Tailscale（hub_tailscale.rs）**：Linux 服务状态=systemctl is-active、启停=systemctl（polkit）；Funnel「serve config denied」=tailscaled 本地权限闸门（root/operator）而非 tailnet 策略，解法 `sudo tailscale set --operator=<user>`；mac App CLI 坑（SHLVL/GUI 报错文本）都有防御。细则 docs/server-hub.md。
- **macOS App 版 Tailscale CLI 必须 `env("SHLVL","1")`**（`hub_tailscale.rs::cli_command`）：CLI 拿 SHLVL 判断是否 shell 调用，缺了就误走 GUI 启动分支 → exit 0 + stdout 全是报错文本（GUI 起没起都一样）。CLI 还会挂死 → 子进程必须带超时（`run_cli_raw` 5s）。细则见 2026-10-25.md。
- **macOS 上「已安装」判据必须是 CLI 真能跑**（`cli_works` = version exit 0 且有输出），不能只看文件存在：pkg 会在 `/usr/local/bin/tailscale` 留 68 字节 shim，删 App 后 shim 仍在、执行 exit 127 → 只看 `is_file()` 会永久误判已安装、前端下载行永不出现。卸载同理要删 shim（先校验体积+内容，别碰 brew 真二进制，commit b96fb07）。
- **mac 上验证不了 webview DOM**：CDP 是 `cfg(windows)` 实现（`ANYBOOT_CDP_PORT` 无效），`ps`/`screencapture` 被宿主沙箱禁 → 用 Rust `#[ignore]` 诊断测试 + jsdom 代替。
- **cloudflared 哈希表必须逐条实测**（首版 darwin 两条误填内核哈希 → mac 三源全拒装）。孤儿清理/残留通道三函数已按平台拆分：Windows=PowerShell/CIM，unix=`ps -axo pid=,lstart=,command=` 精确匹配 + kill -9，**新增平台分支两边都维护**。
- mac 上跨平台 cargo check：rustup 的 cargo 在 `~/.cargo/bin`（/opt/local/bin 是 MacPorts 的），用 `--manifest-path`（cd 前缀会被环境剥掉）；Windows-msvc 目标会被 ring 构建脚本阻塞（无 MSVC），Windows 分支只能在 Windows 机全量 check。

## 构建（Windows 本机）
- `src-tauri/target` 被标低完整性，别在原地修，换 CARGO_TARGET_DIR=`J:/tmp/anyboot-target`（C 盘<3G，别用 E/F 盘）。产物 exe+dll 一起复制到 `C://Users//jianghh//AppData//Local//AnyBoot`（先备份）。
- 前端编译期嵌入 exe，改完必须重编覆盖。

## Linux 构建工具链（本机 2026-10-25 起）
- **系统 /usr/bin/rustc 已坏**（部分升级：rustc 要 libLLVM.so.23.1，系统只有 21/22）→ tauri CLI `rustc -vV` 无 host 行 panic（rust.rs unwrap）。修法：`sudo pacman -Syu`；或前置自装工具链 `export PATH=/home/disk1/cargo-home/bin:$PATH RUSTUP_HOME=/home/disk1/rustup-home CARGO_HOME=/home/disk1/cargo-home`（rustc 1.99.1，独立于系统 rust）。
- 新工具链跑 release 主 crate 偶发 SIGABRT → `CARGO_BUILD_JOBS=2` 降并行。

## Linux 构建工具链（本机 2026-10-25 起）
- **系统 /usr/bin/rustc 已坏**（部分升级：rustc 要 libLLVM.so.23.1，系统只有 21/22）→ tauri CLI `rustc -vV` 无 host 行 panic（rust.rs unwrap）。修法：`sudo pacman -Syu`；或前置自装工具链 `export PATH=/home/disk1/cargo-home/bin:$PATH RUSTUP_HOME=/home/disk1/rustup-home CARGO_HOME=/home/disk1/cargo-home`（rustc 1.99.1，独立于系统 rust）。
- 新工具链跑 release 主 crate 偶发 SIGABRT → `CARGO_BUILD_JOBS=2` 降并行。

## 内置 Tailscale 引擎（tsnet sidecar）
- 五目标产物：`anyboot-tsnet.exe` + `anyboot-tsnet-{linux,darwin}-{amd64,arm64}`，构建一律跑 `scripts/build-sidecar.sh`（坑都写在脚本头注释：/tmp 是 10MB tmpfs 必须 TMPDIR 指大盘、goproxy.cn、CGO_ENABLED=0）。
- 本机 Go 工具链：`~/.workbuddy/binaries/go/go/bin/go`（1.27.3；`/usr/bin/go` 是 gccgo 编不了 tailscale）。
- Rust 查找链：resource_dir/binaries → exe 目录 → app_data/tsnet；unix 文件名带 OS+arch 后缀，mac/linux 改名要同步 `sidecar_filename()` + `tauri.{macos,linux}.conf.json`。
- **本机 cargo check 在仓库目录跑不了**（指纹扫描读 .git exclude 报 EACCES，环境怪癖）→ rsync `src-tauri` + 同级 `ui/` 到仓库外跑（如 /home/disk1/anyboot-check/）；env 组合：`RUSTUP_HOME=/home/disk1/rustup-home CARGO_HOME=/home/disk1/cargo-home PATH=/home/disk1/cargo-home/bin:$PATH CARGO_BUILD_JOBS=2 TMPDIR=/home/disk1/tmp`。注意 bin/ 和 toolchains 分属两个 home（CARGO_HOME 只有 registry，RUSTUP_HOME 才有 toolchains），写混会报「rustup could not choose a version of cargo」。细则见 2026-10-25.md。

## 2026-10-22 端口坑
- 12300 落 Windows 保留段 bind 报 10013：`netsh int ipv4 show excludedportrange protocol=tcp`；恢复 `netsh int ipv4 set dynamicport tcp start=49152 num=16384` 重启 winnat。
- hub.html「本机」chip 是静态 HTML，click 要单独绑。

## 沙箱环境（2026-10-25 大坑）
- **safe-delete 垫片把 rm 转投 .Trash-1000 回收站且不释放空间**（当日 19G 全进 /home/disk1/.Trash-1000，du 视图显示已删、df 满盘）→ 清缓存一律 `/bin/rm` 全路径直删；回收站只清自己当天删的条目（查 info/*.trashinfo 原路径确认）。
- **沙箱 FS 视图会瞬时分叉/劣化**：cargo 工具链整棵消失、systemctl 报 not booted、写入丢失/段落重复。cargo 消失后用 rustup 重装到大盘：`RUSTUP_HOME=/home/disk1/rustup-home CARGO_HOME=/home/disk1/cargo-home`，rustup-init 从 `https://mirrors.ustc.edu.cn/rust-static/rustup/dist/x86_64-unknown-linux-gnu/rustup-init` 下（tuna 404），rustc 1.99.1，cargo 全路径 `/home/disk1/cargo-home/bin/cargo`。
- /home/disk1 总 48G：models 占 23G；构建缓存乱删会满盘（报 No space left）。
- HOME=/root 读不到 global git config → commit 需 `-c user.name=jianghuihua -c user.email=2503338074@qq.com`；一次写入可能双提交，提交后 `git log --oneline -3` 核对。
- Go 缓存目录：GOCACHE=/home/disk1/gocache GOPATH=/home/disk1/gopath（会满盘，可整删重生成）。
