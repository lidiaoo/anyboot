# Tailscale / cloudflared 细则（系统版 hub_tailscale.rs + 内置 tsnet sidecar）

日常只记铁律，细则查当日日志（`2026-10-25.md` 最全）。

## 系统 Tailscale（`hub_tailscale.rs`）
- Linux 服务状态 = `systemctl is-active`、启停走 systemctl（polkit）。Funnel 报「serve config denied」是 **tailscaled 本地权限闸门**（root/operator）而非 tailnet 策略 → `sudo tailscale set --operator=<user>`。macsys **不认** `set --operator`，别照抄 Linux 兜底文案。
- **mac App 版 CLI 必须 `env("SHLVL","1")`**（`cli_command`）：CLI 拿 SHLVL 判断是否 shell 调用，缺了就误走 GUI 启动分支 → exit 0 + stdout 全是报错文本（GUI 起没起都一样）。CLI 还会挂死 → 子进程必须带超时（`run_cli_raw` 5s）。
- **mac 写操作必须提权**：App 版 up/down/set/funnel 只有 root 能做（status/ip 只读可普通用户）。mac 无 pkexec → 唯一通道 `osascript … with administrator privileges`（`elevated_retry` mac 分支）：命令写临时 .sh 避开引号地狱 + 脚本把 stdout/stderr/exit code 落文件 + 主进程边跑边 tail（登录链接要实时上屏）+ 脚本内补 `SHLVL`/`TAILSCALE_BE_CLI`（不补又误走 GUI 分支）。
- **「已安装」判据必须是 CLI 真能跑**（`cli_works` = version exit 0 且有输出），不能只看文件存在：pkg 在 `/usr/local/bin/tailscale` 留 68 字节 shim，删 App 后 shim 仍在、执行 exit 127 → 只看 `is_file()` 永久误判已安装、前端下载行永不出现。卸载同理删 shim（先校验体积+内容，别碰 brew 真二进制，commit b96fb07）。
- **`cli_works` 必须排除 GUI/CLI failed 报错文本**。
- **卸载必须显式摘扩展**：`rm -rf /Applications/Tailscale.app` 绕过 Finder 的「同时移除系统扩展」→ 记录成孤儿 → 重装必撞待重启卸载。顺序「先 `systemextensionsctl uninstall W5364U7YZB io.tailscale.ipn.macsys.network-extension` 再删 App」，失败静默但**卸载后复查**扩展状态并把残留写进返回文案；`gateway_tailscale_install` 装之前查一次，PendingUninstall 直接拦「先重启再装」。
- **装扩展 ≠ 必须重启**：首次批准（Touch ID/密码）后立刻可连，不用重启；只有 `terminated waiting to uninstall on reboot` 才必须重启（macOS 故意推迟到重启）。判据优先级 **已启用 > 待重启卸载 > 待用户允许**（`parse_extension_state`）；升级后「旧记录等重启卸载 + 新扩展已启用」网络其实正常，按待重启短路会让用户白重启 —— 能不能用交给 CLI，只在「有待卸载记录且无任何已启用记录」时才短路 `needs_reboot`。
- **卸装后没重启就重装 = 扩展卡死**：`systemextensionsctl list` 出现两条 tailscale（旧 `terminated waiting to uninstall on reboot` + 新 `activated waiting for user`）→ CLI 一律 `The Tailscale CLI failed to start … (CLIError error 1)` 且 **exit 0**，唯一解法重启 Mac（`systemextensionsctl reset` 要关 SIP）。
- **「一直正在检测」= 探测链没超时或前端静默抛错**：扩展未就绪时 CLI 先连 GUI 的 XPC 等十几秒才吐错（probe 一次候选跑 2 次 CLI × 5s），`systemextensionsctl` 裸 `output()` 无超时会僵死 → invoke 永不 resolve。已修：探测前先用一次 <100ms 的 `systemextensionsctl` 判定，`[activated waiting for user]` → `needs_approval`、`waiting to uninstall` → `needs_reboot`，两者**短路不跑 CLI**（0.28s→0.06s）；`systemextensionsctl` 走 `EXT_QUERY_TIMEOUT=3s`；前端补 `hub_report_error`（window.error + unhandledrejection → trace.log `[js]`，webview 没控制台只能靠它取证）+ `tsRender` 整体 try/catch（抛错就写屏）+ `[探测] 第 N 次 用时=Xms` 心跳日志。
- **日志落盘**：`ts_log` 除 400 行内存环外 append 到 `trace.log`（行首 `[tailscale]`）；只在**状态变化**时记 `[状态]`（UI 3 秒轮询，全记会刷屏）。

## cloudflared
- 孤儿清理/残留通道三函数按平台拆分：Windows=PowerShell/CIM，unix=`ps -axo pid=,lstart=,command=` 精确匹配 + kill -9；**新增平台分支两边都维护**。

## 内置 tsnet sidecar
- 五目标产物：`anyboot-tsnet.exe` + `anyboot-tsnet-{linux,darwin}-{amd64,arm64}`，一律跑 `scripts/build-sidecar.sh`（坑在脚本头：/tmp 是 10MB tmpfs 必须 TMPDIR 指大盘、goproxy.cn、CGO_ENABLED=0）。
- Go 用 `~/.workbuddy/binaries/go/go/bin/go`（1.27.3；`/usr/bin/go` 是 gccgo 编不了 tailscale）。缓存 `GOCACHE=/home/disk1/gocache GOPATH=/home/disk1/gopath`（会满盘，可整删重生成）。
- Rust 查找链：resource_dir/binaries → exe 目录 → app_data/tsnet；unix 文件名带 OS+arch 后缀，改名要同步 `sidecar_filename()` + `tauri.{macos,linux}.conf.json`。
- 孤儿 cloudflared / tsnet 由 `gateway/` 与 tsnet 状态目录管；杀主进程后子进程可能残留，替换二进制前逐一手工清。

## mac 上跨平台 check
- rustup 的 cargo 在 `~/.cargo/bin`（`/opt/local/bin` 是 MacPorts 的）；用 `--manifest-path`（cd 前缀会被环境剥掉）。Windows-msvc 目标被 ring 构建脚本阻塞（无 MSVC）→ Windows 分支只能在 Windows 机全量 check。
