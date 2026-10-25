# 远端管理（server-hub）：三端使用与平台差异

主窗口里的「远端管理」是一组独立窗口：**服务器档案**（地址 / 凭据 / 分组）、**终端**（PTY 跑 ssh）、
**文件浏览**（sftp）、**隧道**（本地转发 / SOCKS5）、**跳板机**（ProxyCommand）。

**入口统一有三处**，走到哪儿都能一步回到远程管理：

| 位置 | 说明 |
| --- | --- |
| 托盘菜单 →「远程管理…」 | `classic` 原生菜单 / `unified` 自绘菜单 / Linux dbusmenu 都有 |
| 主窗口标题栏 →「远程管理」 | 电源三键左边；网页远程控制页无此按钮（开的是电脑上的窗口） |
| 终端 / 文件 / 密钥窗顶栏 →「远程管理」 | 三个独立窗各自一份，已开时只做聚焦 |


它**不引 SSH 库**，直接复用系统自带的 `ssh` / `sftp` 命令行。所以「能不能用」取决于两件事：
系统有没有 OpenSSH 客户端，以及客户端的**能力差异**。后者才是三端表现不一样的原因。

---

## 一、前置条件

| 平台 | OpenSSH 客户端 | 说明 |
| --- | --- | --- |
| Linux | 自带 | 发行版一般预装 `openssh-client` |
| macOS | 自带 | 系统 `/usr/bin/ssh`（9.x） |
| Windows | 10（1809+）/ 11 自带 | `C:\Windows\System32\OpenSSH\ssh.exe`；缺失时「设置 ▸ 应用 ▸ 可选功能」装 *OpenSSH 客户端* |

在 Windows 上想确认应用实际会用到哪个 ssh：

```powershell
Get-Command ssh | Select-Object Source
# 正常应指向 C:\Windows\System32\OpenSSH\ssh.exe
```

> **别用 Git Bash 里的 `ssh` 判断应用行为**：那里解析到的是 `PortableGit/usr/bin/ssh.exe`（MSYS 版），
> 它支持连接复用，和系统自带的 Win32-OpenSSH **能力不同**。应用从开始菜单启动时走的是系统 PATH。

---

## 二、认证方式（三选一，编辑表单里分段）

| 方式 | 存的字段 | 怎么工作 |
| --- | --- | --- |
| 交互 | 都不填 | 密码 / 首次指纹在终端里手输，凭据不落盘。文件 / 隧道需要非交互认证，**这种方式用不了** |
| 密钥 | `key_id` | `ssh -i <私钥> -o IdentitiesOnly=yes`；带口令的私钥在终端里解锁 |
| 保存密码 | `password_enc` | AES-256-GCM 密文落盘；ssh 要密码时由 **本程序自身**充当 askpass 解密喂入 |

保存密码的实现要点（三端同一套）：

- `SSH_ASKPASS` 指向 **anyboot.exe 自己**，`SSH_ASKPASS_REQUIRE=force`（**环境变量**，不是 `-o` 选项），
  `ANYBOOT_ASKPASS_SERVER=<server_id>` 用环境变量带给子进程；
- ssh 调 askpass 时 `argv[1]` 是提示语（`user@host's password: `），程序解析出 host：
  与目标机对得上就用目标机的密码，与跳板机对得上就用跳板机的密码；
- 明文只在内存里活一瞬，写进 ssh 的 stdout 管道，不落盘、不进命令行。

以前是写 `#!/bin/sh` 脚本 + `chmod 700`，Windows 上跑不了（CreateProcess 不认 shebang），已废弃。

---

## 三、平台能力矩阵

| 能力 | Linux | macOS | Windows | 说明 |
| --- | --- | --- | --- | --- |
| 服务器档案 / 分组 / 加密存密码 | ✅ | ✅ | ✅ | 纯 Rust，无平台依赖 |
| 终端（PTY + ssh） | ✅ | ✅ | ✅ | Windows 走 ConPTY（10 1809+） |
| 保存密码免输 | ✅ | ✅ | ✅ | 三端都走「askpass = 自身 exe」 |
| 文件浏览 / 上传下载 / 断点续传 | ✅ | ✅ | ✅ | 见下「复用」差异 |
| 端口转发 / SOCKS5 | ✅ | ✅ | ✅ | |
| 跳板机（ProxyCommand） | ✅ | ✅ | ✅ | 密码由继承的 askpass 按 host 认领 |
| **连接复用（ControlMaster）** | ✅ | ✅ | ❌ | Win32-OpenSSH 不支持 |
| **系统 Tailscale 直连**（up/down + 服务开关 + Funnel） | ✅ | ✅ | ✅ | 见下「系统 Tailscale 接入」小节 |
| **内置 Tailscale 引擎（tsnet）** | ✅ | ✅ | ✅ | 免装客户端的 Funnel 公网通道，见下节 |

### Windows 没有 ControlMaster，意味着什么

Unix 上终端先建立一条 ControlMaster，sftp / 隧道 / 远端命令都靠 `ControlPath` 复用它 ——
同一台服务器只认证一次。**Win32-OpenSSH 不支持多路复用**（实测：带 `ControlMaster=yes` +
`ControlPath=…` 连接会直接 `getsockname failed: Not a socket`，控制套接字文件根本不落地）。

所以 Windows 上改成**每次操作自己认证**：

- 密钥认证：`-i` + `BatchMode=yes`，快；
- 保存密码：`SSH_ASKPASS_REQUIRE=force` 走 askpass，每次多一次进程启动 + 一次握手
  （本地实测百毫秒级，文件浏览这种批操作一个批次只认证一次）。

> **`sftp -b` 必须显式 `-o BatchMode=no`**：`sftp` 拿到 `-b` 会在内部把 BatchMode 打开，
> 而 BatchMode 会连 askpass 一起禁掉 —— 密码认证的服务器于是只报
> `No more authentication methods to try` / `Permission denied (password)`，
> 文件列表永远空。`apply_auth` 在「只有密码、没有密钥」时会补上这个开关。

副作用：Windows 上每次操作都会在服务器 `auth.log` 里留一条认证记录；Unix 上靠复用则会少很多。

### 系统 Tailscale 接入（`hub_tailscale.rs`，远程接入页「Tailscale 直连」卡）

统一走本机 tailscale CLI：状态 `status --json`（退 `ip -4` 兜底）、主开关 `up/down`、
Funnel `funnel --bg <http_port>` / `funnel reset`（配置在 tailscaled 服务内，无子进程残留）。
CLI 探测链见模块头注释（三端各自的候选路径 + PATH 兜底）。

- **主开关（up/down）超时与既有偏好（2026-10-25）**：`up` 在未登录时会阻塞等浏览器授权，
  用 120s 长超时（通用 5s 档会误杀并误报「failed to spawn」）；超时把 CLI 已打出的部分
  输出（含登录链接）带回展示。机器已有非默认偏好时 `up` 报「requires mentioning all」——
  解析官方报错里 4 空格缩进的旗标清单自动补齐重试（保留既有设置，不用 --reset）。
  执行器区分「超时」与「spawn 失败」（`CliOutcome`），提权重试沿用调用方超时档。

- **与内置引擎同款的登录体验（2026-10-25）**：CLI 输出按行流式回传，等授权期间不再静默；
  登录相关行（`To authenticate` 与链接本体）**不进日志**，链接改由登录卡片独占展示
  （`tailscale-login-url` 事件 → 打开登录页 / 复制 / 重新生成三按钮 + 10 分钟有效期提示），
  卡片文案与内置引擎保持一字不差。直连卡的行序（未安装 → AuthKey → 登录 → 地址 →
  清除凭据 → 自启）也按内置卡对齐。
- **AuthKey（直连）**：保存到设置 `tailscaleAuthkey`，连接时由后端写成 0600 密钥文件、
  以 `--auth-key=file:<路径>` 传给 CLI（密钥不进 argv，ps 看不到）；空串 = 清除（同时删文件）。
  前端只拿 `authkey_set` 布尔，不回传密钥。
- **清除登录凭据**：`tailscale logout`（走提权入口，30s 档），断开并清本机凭据；管理台设备
  记录需用户自行移除（文案已说明）。

- **服务级副开关**：Windows=`sc query/start/stop Tailscale`（需管理员，CREATE_NO_WINDOW）；
  Linux=`systemctl is-active`（只读查询免 root）+ `systemctl start/stop/restart`（桌面会话弹
  polkit 授权框；无桌面会报「Interactive authentication」的人话翻译）；macOS 无此开关
  （Tailscale.app 常驻代管）。前端按 `service_running >= 0` 自动点亮，无需平台判断。
- **应用内一键安装（三端）**：Windows=setup exe `/quiet`（UAC）；macOS=.pkg `open`；
  Linux=官方静态 tarball（manifest `Tarballs` 字段；映射 amd64/arm64/386/arm/riscv64，
  mips 系不认）→ pkexec 提权解压双二进制到
  `/usr/local/bin` → 注册 systemd unit（**仅当系统无 tailscaled unit**，不覆盖发行版包配置）
  → `enable --now` → 轮询 is-active 确认。解压暂存放 tarball 同目录（app_data），不落 /tmp。
  **Linux 下载前前置检测（fail-fast，2026-10-25）**：systemd 在跑（`/run/systemd/system`）、
  pkexec 可用、未装发行版包（`/usr/lib` 有 tailscaled unit 或 `/usr/bin`/`/usr/sbin` 有二进制
  即拒绝——混装会导致 CLI/daemon 版本漂移且卸载边界分不清）。不通过直接报人话指引，
  不白下 30MB。安装后服务未运行且缺 `/dev/net/tun` 时，报错附容器场景提示
  （tun 模块 / `TS_USERSPACE=1`）。**安装/卸载脚本连带清理 unit 的 `.d` drop-in 目录**
  （2026-10-25 本机踩坑：旧 override 残留在重装后盖掉新 ExecStart → 203/EXEC；
  发行版 unit 的 drop-in 一律不碰）。
- **macOS App 版 CLI 两个专属坑**（都在 `cli_command`/`probe` 处理）：`SHLVL=1` 强制 CLI
  分支（否则误走 GUI 启动分支，exit 0 + stdout 全是报错文本）；IPv4 严格形状校验防报错
  文本被当 IP 拼进直连地址。
- **Linux 两个高频故障的人话翻译**（2026-10-25 补）：
  - `failed to connect to local tailscaled` → tailscaled 服务没跑，提示副开关或
    `sudo systemctl enable --now tailscaled`；
  - Funnel 报 `Access denied: serve config denied` → **不是 tailnet 策略**，是 tailscaled
    本地权限闸门：prefs/serve 变更只允许 root/operator。变更类调用（up/down/funnel）走
    `run_cli_privileged`：被拒时 Linux 自动 pkexec 提权（polkit 弹桌面授权框），首选
    一次性设 operator（授权一次终身免提权）；pkexec 不可用/用户取消时回退提示手动执行
    `sudo tailscale set --operator=<用户名>`。tailnet 策略的 `nodeAttrs` funnel
    授权是另一道独立的门（较新 tailnet 默认已放行），两道都过 Funnel 才通。
    兜底细则（2026-10-25 补强）：授权框被取消时 pkexec 报 `Not authorized`（不含
    `access denied`），elevated_retry 打「提权未完成」标记让调用方追加手动命令；
    `up` 输出含登录链接时失败信息整段保留（不再只取第一行把 URL 掐掉），并提示
    「浏览器完成授权后再点一次开关」；operator 设好后重试期间有日志，不再像卡死。

### 内置 Tailscale 引擎（tsnet sidecar）—— 三端布局与排错（2026-10-25 起覆盖 mac/linux）

Go sidecar 把本机 HTTP 服务经 tsnet 的 Funnel 发布成 `https://<节点>.<tailnet>.ts.net`。
源码 `src-tauri/sidecar-tsnet/`，构建 `scripts/build-sidecar.sh`（全 5 个目标；坑都写在脚本头注释）。

**产物文件名**（`hub_tsnet.rs::sidecar_filename` 按平台+架构选）：

| 平台 | 文件 | 打包配置 |
| --- | --- | --- |
| Windows | `anyboot-tsnet.exe` | `tauri.windows.conf.json` |
| Linux | `anyboot-tsnet-linux-{amd64,arm64}` | `tauri.linux.conf.json` |
| macOS | `anyboot-tsnet-darwin-{amd64,arm64}` | `tauri.macos.conf.json` |

**查找链**（第一个命中即用）：`resource_dir()/binaries/`（Windows=exe 目录、mac=`.app/Contents/Resources`、
Linux=`/usr/lib/<identifier>`）→ exe 目录 `binaries/` → exe 同目录 → 数据目录 `tsnet/`（手动放置位）。
spawn 前 unix 会兜底补 `0o755`（打包链路丢执行位时救场）。

**排错**：
- 「内置 Tailscale 引擎缺失（<路径>）」：括号里就是实际找的第一个候选路径，对照上表看包里有没有带对应文件；
  手动救急 = 把产物拷到数据目录 `tsnet/` 下并按当前平台命名。
- 运行日志框（环形缓冲 200 条）第一行会打 sidecar 实际路径与状态目录；登录链接以
  `login.tailscale.com` 域白名单校验后才发给前端打开浏览器。
- 引擎状态目录在数据目录 `tsnet/state/`（登录凭据持久化处）；「清除登录凭据」= 停进程 + 删该目录。
- mac/linux 端 2026-10-25 才有产物，端到端（真实 tailnet 登录 → Funnel ready）尚未在真机验过。

### 三条日志线（2026-10-25 起统一形态）

远程接入页三条通道（Cloudflare 通道 / Tailscale 直连 / 内置引擎）的日志框形态对齐：
卡片头部「日志」按钮 → 弹窗（2s 轮询、错误行标红、复制全部/清空/关闭），内置引擎保留
内嵌小框实时跟看 + 「日志」按钮开放大版。

- **存储**：三条线都是**内存环形缓冲**（通道 500 条 / 直连 400 条 / 内置引擎 200 条），
  重启即清，不落文件。
- **直连线记录范围**：下载（清单 → 候选直链逐个结果）、安装（pkexec 脚本输出逐行、
  服务确认）、开关 / Funnel 成败、pkexec 提权每一步——弹窗可回看全程，不用复现抓 toast。
- **「清空」语义三处一致**：清后端日志环（`gateway_tunnel_logs_clear` /
  `gateway_tailscale_logs_clear` / `tsnet_logs_clear`）+ 清屏。只清屏幕会被下一轮
  轮询用旧日志恢复（内置引擎清空显示曾踩过），所以必须动源头。
- **内置引擎噪音过滤（2026-10-25）**：`trying bootstrapDNS(...)`（logtail 上报端点
  不可达时用 DERP IP 引导解析的重试日志，每条内容都不同、国内网络常刷屏）在
  sidecar 的 `logf`/`userLogf` 源头丢弃，Rust 侧 log 事件再兜一道；引擎上线成功与
  拿到登录链接时会打「初始化环境资源需几分钟、就绪后打印公网地址」的预期管理提示。

### 远程接入页自动刷新（2026-10-25）

页头右上角一个三合一控件（`#gwAuto`）：**默认 3 秒**、点按钮暂停/继续、下拉选
1/2/3/5/10/30/60 秒、也可直接在框里敲 1–3600 之间的任意秒数（回车或失焦生效，
越界夹紧、非法值还原）。间隔与暂停态存 `localStorage: ab-hub-gwauto`。

- **一次刷新 = `gwRefresh()`**（通道配置+快照、配对客户端、残留通道、Tailscale 直连、
  内置引擎），所以四个分页共用这一个节奏。
- **只在看的见时候刷**：离开这一页或窗口隐藏时不计刷新（回到页面立刻补一次）；
  上一轮没回来就跳过本轮，后端慢时不堆请求。
- **暂停要真停**：内置引擎那条独立的 5s 轮询也要跟着停（`gwAutoIsPaused()` 闸门），
  否则暂停了卡片还在自己跳，看着像没生效。
- **不打断输入**：周期刷新重跑 `applyConfig()` 时，正在聚焦的命名隧道地址框不覆盖。

### 卸载与重新检测（2026-10-25）

三张卡头部布局统一：开关组 + 「卸载」+「日志」+「重新检测」（内置引擎的按钮与
Tailscale 直连同位同款；其内嵌日志框已移除，入口只剩「日志」弹窗）。

- **卸载边界 = 「是通过 AnyBoot 装的吗」**：
  - cloudflared：二进制恒在 AnyBoot 缓存位（下载 / 手动路径都复制进来，单一事实
    来源）→ 缓存位有货即可卸，只删缓存副本、不碰手动路径的原始文件；卸载前先停通道。
  - Tailscale 直连：安装流程跑完写数据目录标记 `tailscale.anyboot-install`；有标记
    才允许卸载——Windows=注册表 UninstallString 解 GUID 后 `msiexec /x /quiet`
    （UAC），macOS=退出 App 后 osascript 提权删 `/Applications/Tailscale.app`，
    Linux=pkexec 停服务 + 删 `/usr/local/bin` 二进制 + 只删 AnyBoot 写的 unit
    （指纹 = ExecStart 指向 /usr/local/bin 且系统无发行版 unit）。无标记 → 拒绝并
    按平台给手动卸载指引。**Linux 无标记时用安装指纹兜底（2026-10-25）**：marker
    功能晚于早期安装，`/usr/local/bin` 双二进制 + ExecStart 指向它的自家 unit +
    无 `/usr/lib` 发行版 unit 三者同时成立 = 确凿 AnyBoot 装的 → 补写标记后照常
    卸载；不匹配才拒绝。Win/mac 无等价指纹，维持一票否决。
  - 内置引擎：引擎本体随 AnyBoot 安装包自带（恒属「AnyBoot 装的」）→ 卸载 = 停引擎
    + 删状态目录（登录凭据）+ 清数据目录里手动放置的 sidecar 副本；重开开关即恢复。

### 卸载前的凭据处置（2026-10-25）

老版本卸载只删程序，认证信息留在原地：cloudflared 的 Tunnel Token、Tailscale 的
AuthKey 都还在，下次重装会**继承一份用户以为已经没了的登录态**。现在统一成
「默认连凭据一起清，想留必须手动勾」：

- **三张卡共用一个卸载弹窗**（`#gwUniOverlay`），里面一枚「保留本机数据
  （登录凭据 / 配置 / 状态目录）」勾选框，**默认不勾** = 清理。后端命令因此都多了一个
  `keepData: bool` 参数（缺省 false，老前端不传也走清理）。
- 各通道清什么：
  - cloudflared：删二进制 + 清 `gateway_named_token` / `gateway_named_url`、模式回落
    `quick`、关自启。勾保留则只删二进制。
  - Tailscale 直连：**卸载前**先 `tailscale logout`（凭据在 tailscaled 状态目录里，
    卸载后就再没有 CLI 可用了），卸载成功后清 `tailscale_authkey`（含 0600 密钥文件）
    与自启开关。logout 失败只记日志、不阻断卸载（best effort）。
  - 内置引擎：停引擎 + 删 `tsnet/state`（节点凭据）+ 清 `tsnet_authkey` 与自启；
    勾保留则只停引擎、清引擎副本，状态目录留着（重开即 ready）。
- **「清除登录凭据」按钮**（Cloudflare 卡新增，`gateway_clear_credentials`）：停通道 +
  抹 Token/对外地址，不动程序本体 —— 与 Tailscale/内置引擎的同类按钮同语义。
- **Tailscale 的 logout 顺带清 AuthKey**：只注销不删密钥的话，下次连接拿密钥文件
  一跑就自动重登，用户会以为「清了但没用」。
- 顺手补的**真进度条**：`download_to_tmp_observed(url, tmp, on_progress)` 逐块回调
  （~120ms 节流、末块必发），cloudflared 与 Tailscale 安装包两条下载都接上；
  此前 cloudflared 只在换源时发一次、Tailscale 只在完成时发一次 100%，进度条形同摆设。
  Tailscale 未安装态的下载区也改成与 cloudflared 二进制行同一套布局
  （状态文字 + 右对齐按钮 + 紧跟其下的进度条）。

### 子进程不能弹控制台窗口

GUI 程序（`windows_subsystem = "windows"`）起 `ssh` / `sftp` / `ssh-keygen` 时，Windows 会为子进程
新建一个控制台窗口 —— 用户看到的就是「每次操作都闪一下黑框」。所有子进程统一走
`lib.rs` 的 `command()`（带 `CREATE_NO_WINDOW`）；**hub 系列曾经绕过它直接
`std::process::Command::new`**，所以文件列表 / 上传下载 / 远端复制每操作一次闪一次。
新增任何子进程调用都必须用 `crate::command(...)`，不要直接 `Command::new`。

终端窗是例外：portable-pty 走 ConPTY，它的 conhost 命令行带 `--headless`，本来就不显示窗口。

### 会阻塞的命令必须是 `async`（否则整窗无响应）

同步的 `#[tauri::command]` 跑在 **WebView2 的 IPC 回调线程，也就是 UI 线程**上。
在这些命令里做任何会等的事 —— 建 PTY、起 `ssh`/`sftp` 子进程、TCP 连接、`thread::sleep` ——
都不只是「慢一点」：UI 线程被占住时窗口不重绘、不响应消息，用户看到的就是**点一下终端，
窗口卡死/白屏**。

定则：

- 只要命令里有上面任意一种调用，一律写成 `pub async fn`，把真正干活的同步函数放进
  `tauri::async_runtime::spawn_blocking(...)`（命名习惯：`foo` 是 async 包装，`foo_blocking`
  才是原实现，函数上写明「只允许在阻塞池里调用」）。
- 纯内存操作（查表、解 base64、往有界通道 `try_send`）可以保持同步 —— 比如 `hub_term_write`
  故意同步：它只入队，真正会阻塞的 `write_all` 在专属写线程上（`INPUT_QUEUE` 满了就明确报错）。
- **守卫单测**：`src-tauri/src/blocking_guard.rs` 会扫描 `src/*.rs`，同步命令体里出现
  `Command::new` / `openpty` / `spawn_command` / `TcpStream::connect` / `thread::sleep` /
  `.wait()` / `.output()` / `dialog()` 就测试失败（含「调了同文件里另一个同步函数」的间接命中）。
  绕不过去时在函数体里写 `// blocking-ok: <理由>`（守卫只认这个词），别删测试。

前端也要给可见进展：终端页 `openSession` 立刻显示「正在连接…」、按 server.id 防重入、
`OPEN_TIMEOUT_MS`（10s）超时后报错而不是静默黑屏。

### 传输状态同步（文件窗 ↔ 传输页）

传输状态只有一个真相源：后端每条任务带 `state`，五态之一
`running / paused / done / failed / abandoned`，任何变更都**广播**
（`app.emit`，不是 `emit_to`）一条 `hub-sftp-progress`。三个订阅方都吃同一份：
文件窗进度条、主窗内嵌文件浏览的进度条、传输页列表。

几条踩过的坑，改这块之前先读：

- **不要用 `emit_to(窗口)`**：传输页开在主窗里，主窗没打开时 emit_to 直接失败
  （返回值过去被丢弃），用户再打开传输页就什么都看不到。广播 + 快照兜底才对。
- **快照必须带终态**：`hub_sftp_transfer_list` 返回 running + 断点清单 + `recent`
  （最近 30 条 done/failed/abandoned）。只给前两类的话，「刚才下载完的」在传输页里
  根本不存在、「刚才放弃的」卡片永远消不掉 —— 事件丢了没有第二条路可走。
- **进程退出就要从 running 表摘掉自己**（`detach_running`，按 `Arc::ptr_eq` 只摘本进程）。
  暂停/中断分支过去漏了这一步，于是清单一直把死进程报成 `running`，前端卡片永远停在
  「下载中」。这条是实测复现出来的，不是推测。
- **中断（网络断/连接被重置）报 `paused` 而不是 `failed`**：它确实还能续传，而
  `failed` 会让前端删卡、清单里却还留着记录 —— 两边对不上账就是「状态不同步」。
- **批量操作的 `key=null` 语义**：`pause` 作用于所有 running、`resume` 逐条续传
  **全部**断点、`abandon` 清掉「所有 running + 全部断点」。`resume` 以前走
  `pick_paused(None)` 只取最近一条，于是「全部继续」看着成功、实际只续了一个。
- **开跑即广播 `running`**：下载过去没有初始事件，列表要等第一次字节变化才出现卡片，
  小文件甚至连一条都没有。现在统一放在 `spawn_sftp_transfer` 入表之后发。
- **续传沿用原窗口**：`XferTask.window` 记任务来源窗口，续传时按它发事件、关窗时按它
  收进程（旧任务是主窗内嵌浏览发起的，不能硬编码成文件窗）。

### 下载目录

设置页「下载路径」留空 = 系统下载目录，由后端 `hub_download_dir_default` 给
（Windows 已知文件夹 / macOS `~/Downloads` / Linux 的 XDG 下载目录），取不到退回主目录；
右侧「选择…」调 `hub_pick_directory` 开系统目录选择框（资源管理器 / Finder / 文件管理器）。

前端把空值和旧版本的 `~` 都当「未设置」（`normalizeDownDir`）。`~` 展开由后端
`expand_home` 负责，读 `HOME` **和** `USERPROFILE` —— Windows 没有 `HOME`，只读它会让
`~` 展开失败、文件落到进程工作目录。

---

## 四、排错

| 现象 | 原因 / 处理 |
| --- | --- |
| `Permission denied (publickey,password,…)` | 密码错 / 公钥没加到服务器 `~/.ssh/authorized_keys`；也可能是用了未解锁的带口令私钥 |
| 提示「这台服务器还没有保存凭据…」 | 认证方式选了「交互」——文件 / 隧道需要非交互认证，改成密钥或保存密码 |
| 提示「无法启动 ssh」 | PATH 里没有 ssh：Windows 装「OpenSSH 客户端」；Linux 装 `openssh-client` |
| 跳板机报「没有保存凭据」 | 跳板机自己也必须是密钥或保存密码，不能只在终端里手输过一次 |
| Windows 上文件浏览很慢 | 没有连接复用，每次重新认证；属预期 |
| `Connection closed by <host> port <port>`，`-v` 里是 `banner line 0: Not allowed at this time` | **服务端主动拒绝**，握手第一行就没给 banner。常见于 fail2ban / CrowdSec / 云厂商的爆破防护把本机 IP 临时拉黑（反复试错密码就会触发）。与客户端无关，换网络/等封禁过期/改用密钥 |

---

## 五、验证到什么程度

- Windows：`cargo test` 166 项全过；askpass 端到端（真实二进制 + 真实加密配置）验证了
  「目标机密码 / 跳板机密码 / 回落 / CLI 不被劫持」四条；真实 `ssh` / `sftp` 进程确认会消费这套
  环境变量、参数顺序正确（以错误密码得到 `Permission denied` 收尾，不是挂死）。
- Windows 实机（2026-10-18，真实服务器 81.68.251.231，保存密码）：终端开窗即连上（MOTD + 提示符
  + 延迟上报）、文件页能列 `/root`（36 项，权限/大小/时间正确）。
- Windows 实机（2026-10-18 二轮）：四个窗口顶栏入口齐全（主窗 / 远程管理 / 终端 / 文件 / 密钥）；
  设置页「下载路径」显示系统下载目录占位符 +「选择…」按钮；重启 + 打开文件窗期间
  **没有任何带窗口的 conhost**（只有 ConPTY 的 `--headless` 一个），确认黑框消失。
- 同轮修掉的两个 Windows 专属缺陷：
  ① hub 里点「打开终端 / 浏览文件 / 密钥」开出来的子窗**全白**——原因是这些命令是**同步命令**，
     跑在 WebView2 的 IPC 回调（UI 线程）里，在同一线程重入 WebView2 建新 webview，窗口建得出来
     但页面永不导航。改成 `async fn`（在 tokio 工作线程创建）即恢复正常。
  ② 文件列表恒空——见上「`sftp -b` 必须显式 `BatchMode=no`」。
- **未验证**：macOS / Linux 实机（mac 上 hub 开窗同样是 async 路径，但没跑过）。
- Windows 实机（2026-10-18 三轮，症状「点开终端卡死」根治）：
  - 病根同上一轮 ①，但位置在**命令体**而不是建窗：`hub_term_open`（openpty + 起 ssh）、
    `hub_sftp_home/list/mkdir/remove/rename/copy/readlink`（起 sftp/ssh 子进程）、`hub_test_server`
    （TCP 连接）、`hub_key_generate`、`hub_key_public_key`、`hub_keys_list`、`hub_tunnel_start/stop`、
    `remote_test` / `remote_delete` / `remote_test_via`、grub 的 `detect_esp` / `sync_grub` /
    `install_grub_script` / `uninstall_grub_script` 全是同步命令。
  - 全部改为 `async fn` + `spawn_blocking`；终端上行从「命令里直接 `write_all`」改成有界队列
    + 专属写线程（满了返回明确错误，不静默排队）。
  - 量测方式：对 anyboot 每个顶层窗口 20ms 一次 `WM_NULL` + `SendMessageTimeout`（UI 线程被占就
    必然出现尖峰或 timeout）。覆盖「启动 → 开远程管理窗 → 开终端窗 → 建 ssh 会话」全程
    45s / 3837 样本：**最大往返 23ms，>100ms 尖峰 0 次，timeout 0 次**。
  - 另修一条验证链路的坑：启动脚本用 bat 时，**UTF-8 中文注释 + LF 行尾会让后面的 `set` 不生效**
    （hook 静默不跑，看起来像「功能坏了」）。改为纯 ASCII 启动器（Python），hook 才有输出。
  - `[hub-term]` 现在会把 ssh 的报错原文抄进 stderr（会话非 0 退出时打印最近 4KB），
    连不上不再只有界面里那个死标签可看。
- Windows 实机（2026-10-18 四轮，症状「下载进度/暂停/继续/放弃没有同步到传输列表」）：
  用 WebView2 调试端口在真实页面 JS 上下文里驱动，服务器当前被服务端 fail2ban 拒绝，正好走**失败/中断**分支
  —— 状态机的每条边都能验：
  - `hub_sftp_download` 立即返回（7ms，不阻塞 UI）；4s 后 `transfer_list` 只有一条
    `state=paused`（修复前是 `running` + `paused` 两条，running 就是僵尸进程）。
  - 传输页 DOM：卡片从「↓ 已暂停 / 0% · 断点」出现，点「全部放弃」后消失回空态。
  - 两张暂停卡 → 「全部放弃」一次清掉，两张同时消失（`abandon` 返回 2）。
  - `resume(key=null)` 在空清单上返回可读错误而不是崩溃。
  - **未验证**：真实下载完成（`done`）路径与「全部继续」真的把多条续起来 ——
    需要一台能连上的服务器，当前被服务端拒绝。


### 白框（窗口在、页面死了）—— 心跳体检（2026-10-18 五轮）

**症状**：重启电脑后点「远程管理 → 终端 / 文件」出白框、无响应，点多少次都一样；
只重启 anyboot 软件没用。

**成因**：复用窗口的分支只看 `get_webview_window(label)` 有没有返回。窗口对象在 ≠
页面还活着 —— WebView2 渲染进程崩掉 / 首帧没加载出来时，窗口壳还在注册表里，
内容是纯白。之后每次点击都只是 `show` 那个白壳、往死页面 emit（错误被 `let _ =`
吞掉），永远不会重建。只有重启系统把进程清掉才恢复。

**修复**：新增 `src-tauri/src/hub_health.rs`（心跳体检层）：

- 四个 hub 页面（hub / term / files / keys）每 3s 发一次 `hub_page_ping{ready}`。
- 所有复用分支先过 `reuse_or_clear`：心跳新鲜 → 复用；**窗口在但页面已死
  （曾 ready 后停跳 >15s，或从未 ready 且超首帧宽限 25s）→ 销毁重建**，绝不再
  show 白框。判据是纯函数 `beat_alive`，5 个单测钉死。
- 建窗后挂一次性首帧看门狗（12s 未 ready → 记日志 + 隐藏，下次点击走重建）。
- 诊断无条件写 `startup-debug.log`（`[hub] server-term: 窗口在但页面已死
  （ready=… pings=… 建窗后=… 最后心跳=… 前）→ 销毁重建`）。

**实机验证**（CDP 驱动，2026-10-18）：P1 term 页可执行 JS；P2 取到服务器 id；
P3 正常点击走复用（无销毁重建日志）；P4 用 JS 死循环冻结页面 → 22s 后再点 →
日志出现「窗口在但页面已死 → 销毁重建」，新页面 `readyState=complete`。

**注意**：往主窗或新窗加页面时，必须在页面里带同款心跳脚本（`</body>` 前的
`hub_page_ping` invoke + setInterval 3000），否则 25s 宽限期后该窗会被判死、
点一次重建一次。

### CDP 调试端口：环境变量路线已死，用 `ANYBOOT_CDP_PORT`（2026-10-18）

WebView2 运行时 155.0.4283.60 实测**不再应用** `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`
环境变量（wry 经 API 传的 additional_browser_args 优先生效，变量进了进程但进不了
浏览器命令行，端口永远不监听）。现改为应用内显式传参：

- 启动前设 `ANYBOOT_CDP_PORT=9223`，主窗（第一个 webview，决定浏览器进程参数）
  以 `additional_browser_args` 带上 `--remote-debugging-port`；不设则与原 config
  建窗完全一致，生产无感知。
- **铁律**（tauri#11144）：同一 user data folder 下的所有 webview 的环境参数必须
  完全一致，否则 webview 创建直接报 0x8007139F（ERROR_INVALID_STATE）。所以全部
  8 个 `WebviewWindowBuilder` 调用点统一过 `lib.rs::apply_browser_args` —— 新加窗
  必须同样包裹，别手写 `additional_browser_args`。
- 主窗因此从 tauri.conf.json 的 `windows` 挪到 setup 里用 builder 建（参数逐项
  与原 config 一致）。
- 连 CDP 必须连 `127.0.0.1`（localhost 解析 ::1 会 ConnectionRefused）；重启测试
  前杀掉旧实例后等几秒再启动，撞上参数不一致的残留浏览器进程同样 0x8007139F。
