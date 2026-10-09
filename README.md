# rEFInd Switcher

一个基于 Tauri 2 的跨平台托盘工具：改写 rEFInd 的 `EFI/refind/vars/PreviousBoot`，决定**下次**启动进哪个系统；并且内置一个 HTTP 服务，让你**远程**（webhook / curl / 面板 / ssh）就能切系统、重启、关机、睡眠。

Windows / macOS / Linux 三端同一份代码，行为对齐。

---

## 我想做什么 → 从哪开始 → 得到什么

| 我想做什么 | 从哪开始 | 得到什么 |
| --- | --- | --- |
| 在这台机器上点一下切换下次启动的系统 | 托盘图标右键 → 直接点 `Linux` / `Windows` / `macOS` | 托盘里的「下次启动」立刻变成所选系统（下一次开机生效） |
| 在另一台电脑/手机上远程切系统、重启、关机、睡眠 | [远程控制](#远程控制http-服务) → 拿 token → 按 [端点表](#端点表每条都给了可复制的-curl) 发一条请求 | 这台机器按请求动作；默认先弹窗 5 秒（可在弹窗或主窗口取消）再执行 |
| 只想 ssh 过去一条命令搞定 | [ssh 两种用法](#ssh-远程调用两种用法) | `ssh user@host 'refind-switcher --switch windows --action reboot'` 一条就够 |
| 用现成的面板 / 手机 App（UpSnap、PowerControl、Postman…） | [常见客户端与面板](#常见客户端与面板) | 把 URL + Header 填进客户端即可，不用写代码 |
| 让这台机器开机就常驻托盘、随时可被远程调用 | [应用设置](#应用设置) | 开机启动 + 启动时隐藏主界面 + HTTP 服务默认开启 |
| 先确认“远程到底会执行什么命令”再让它真执行 | [调试环境变量](#调试环境变量)（`REFIND_SWITCHER_POWER_DRY_RUN=1`） | 只回显将要执行的命令，不真的重启/关机/睡眠 |
| 我自己改代码 / 打安装包 | [开发运行](#开发运行) 与 [构建产物](#构建产物) | `npm run tauri dev`；三端各自的安装包 |

目录

- [功能一览](#功能一览)
- [远程控制（HTTP 服务）](#远程控制http-服务)
  - [默认行为与安全默认值](#默认行为与安全默认值)
  - [token 去哪看、怎么换](#token-去哪看怎么换)
  - [三端各自的防火墙处理](#三端各自的防火墙处理)
  - [端点表（每条都给了可复制的 curl）](#端点表每条都给了可复制的-curl)
  - [电源动作：重启 / 关机 / 睡眠](#电源动作重启--关机--睡眠)
  - [通知弹窗与延迟执行（可配 0 秒立即执行）](#通知弹窗与延迟执行可配-0-秒立即执行)
  - [常见客户端与面板](#常见客户端与面板)
  - [给每台机器起个名字（多机管理）](#给每台机器起个名字多机管理)
  - [ssh 远程调用两种用法](#ssh-远程调用两种用法)
  - [远程需要“本机授权”时怎么办](#远程需要本机授权时怎么办)
- [应用设置](#应用设置)
- [环境要求](#环境要求)
- [开发运行](#开发运行)
- [构建产物](#构建产物)
- [调试环境变量](#调试环境变量)
- [权限说明](#权限说明)
- [常见问题](#常见问题)
- [注意](#注意)

---

## 功能一览

- 托盘菜单直接切换下次启动 Linux / Windows / macOS（读 `PreviousBoot-linux|windows|mac` 模板，写 `PreviousBoot`）。
- 主窗口不占任务栏 / Dock，只在托盘常驻；托盘与主窗口**双向联动**（同一份状态快照，一边改另一边立刻跟着变）。
- **电源动作**：重启、关机、睡眠，主窗口三个按钮与托盘「电源操作」子菜单都能用（见 [电源动作](#电源动作重启--关机--睡眠)）。
- **内置 HTTP 服务**：默认开启、默认 `0.0.0.0:12300`、必须带 token；webhook / curl / 面板 / 手机 App 都能远程切系统与电源动作。
- **带参数的 CLI**：与 GUI 同一个可执行文件，无参数开界面，带参数走命令行（ssh 一条命令切换）。
- **远程请求通知弹窗 + 延迟执行**：远程请求先弹置顶小窗倒计时（默认 5 秒），期间可在弹窗或主窗口点「取消」；两条渠道（HTTP / SSH-CLI）各自的开关与延迟都能单独配。
- 开机启动开关（macOS 走系统登录项，Windows 写注册表 Run，Linux 写 `~/.config/autostart`）。
- Linux 能用 `lsblk` 找未挂载的 ESP/FAT 分区并临时只读挂载，写入前自动换读写、写完恢复只读。
- 写入用同目录临时文件 + 原子替换 + SHA-256 校验。

---

## 远程控制（HTTP 服务）

### 默认行为与安全默认值

装好第一次启动，`settings.json` 里就已经是这样的默认值（不用你去翻文档找开关）：

| 设置项（`settings.json`） | 默认值 | 含义 |
| --- | --- | --- |
| `httpEnabled` | `true` | HTTP 服务默认**开**启 |
| `httpBind` | `"0.0.0.0"` | 监听所有网卡（局域网可直连；只想本机/隧道用就改成 `127.0.0.1`） |
| `httpPort` | `12300` | 端口 |
| `httpToken` | 首次运行自动生成 32 位十六进制 | **必填**；为空则服务拒绝启动（不允许无鉴权监听） |
| `notifyEnabled` | `true` | 远程请求通知总开关 |
| `channels.http.notify` | `true` | webhook / curl / 面板 这条渠道弹不弹窗 |
| `channels.http.delaySeconds` | `5` | 这条渠道延迟几秒执行（`0` = 立即执行） |
| `channels.cli.notify` | `true` | ssh / CLI 这条渠道弹不弹窗 |
| `channels.cli.delaySeconds` | `5` | 这条渠道延迟几秒执行 |
| `remoteAllowGuiAuth` | `false` | 远程/CLI 请求**不会**弹本机授权框（见 [远程需要“本机授权”时怎么办](#远程需要本机授权时怎么办)） |

必须知道的三件事：

1. **是明文 HTTP，不是 HTTPS**。token 会以明文经过网络，请只在可信局域网或 SSH 隧道里用。
2. **token 必填**。服务启动的前提是 token 非空；日志里只记 token 前 8 位指纹，完整值不会写进日志，也不会出现在托盘菜单和通知弹窗里（防截图）。
3. **默认监听 `0.0.0.0`**。不希望局域网能访问就把监听地址改成 `127.0.0.1`，然后走 SSH 隧道（见 [ssh 两种用法](#ssh-远程调用两种用法)）。

### token 去哪看、怎么换

三个入口，改哪个都一样（同一份 `settings.json`）：

- **主窗口 → 远程控制**：有「显示 / 复制 / 重新生成」三个按钮，还有监听地址/端口输入框和「复制命令」（直接给你含 token 的 curl 与 ssh 示例）。
- **托盘右键 → 远程控制**：HTTP 服务开关、远程请求通知开关、远程执行延迟预设（立即 / 3 / 5 / 10 / 30 秒），以及「远程控制设置…」（打开主窗口并滚到该区）。
- **直接改文件**：`settings.json` 里的 `httpToken`。改完在界面上再动一下任意远程设置，或在主窗口开关一次服务即可生效。

配置目录（放 `settings.json`，日志 `trace.log` 也在同一目录）：

| 平台 | 路径 |
| --- | --- |
| Windows | `%APPDATA%\com.example.refind-switcher\`（即 `C:\Users\<你>\AppData\Roaming\com.example.refind-switcher\`） |
| macOS | `~/Library/Application Support/com.example.refind-switcher/` |
| Linux | `~/.config/com.example.refind-switcher/` |

忘了 token：主窗口「显示」按钮，或直接读文件。

```bash
# Linux / macOS
python3 -c "import json,os;print(json.load(open(os.path.expanduser('~/.config/com.example.refind-switcher/settings.json')))['httpToken'])"
```

```powershell
# Windows PowerShell
(Get-Content "$env:APPDATA\com.example.refind-switcher\settings.json" | ConvertFrom-Json).httpToken
```

换 token 后旧 token **立刻失效**（服务会用新 token 重建）。

### 三端各自的防火墙处理

应用能监听，但系统防火墙可能挡着外部访问。

```powershell
# Windows（管理员 PowerShell）：放行 12300/TCP
New-NetFirewallRule -DisplayName "rEFInd Switcher HTTP" -Direction Inbound -Protocol TCP -LocalPort 12300 -Action Allow
# 撤销
Remove-NetFirewallRule -DisplayName "rEFInd Switcher HTTP"
```

```bash
# macOS：第一次被外部访问时会弹「是否允许传入连接」，选“允许”
# 也可手动看：系统设置 → 网络 → 防火墙 → 选项 → 允许传入连接
/usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate   # 看防火墙状态
```

```bash
# Linux（ufw 系）
sudo ufw allow 12300/tcp && sudo ufw status

# Linux（firewalld 系）
sudo firewall-cmd --add-port=12300/tcp --permanent && sudo firewall-cmd --reload

# 看谁在监听（三端各自的办法）
ss -ltnp | grep 12300                 # Linux
lsof -nP -iTCP:12300 -sTCP:LISTEN     # macOS
netstat -ano | findstr 12300          # Windows
```

### 端点表（每条都给了可复制的 curl）

除 `/health` 外**所有端点都要 token**。token 三种放法任选：请求头 `X-Auth-Token: <token>`（推荐）、query `?token=<token>`、表单/JSON body 里的 `token` 字段。

```bash
# 先把这两个变量设好，后面直接复制
TOKEN='把主窗口里复制的 token 贴这里'
HOST='192.168.1.50'      # 目标机的局域网 IP；本机测试用 127.0.0.1
```

| 端点 | 干什么 | 会改状态吗 |
| --- | --- | --- |
| `GET /health` | 服务活着吗（免 token） | 不会 |
| `GET /api/status` | 变量目录、三个系统文件是否存在、下次启动是哪个 | 不会 |
| `GET /api/requests` | 最近 20 条远程请求（时间/来源/渠道/动作/结果） | 不会 |
| `POST /api/switch?system=linux` | 切换下次启动为 linux / windows / macos | 会 |
| `POST /api/power?action=reboot` | 重启 / 关机 / 睡眠（`action` 缺省 = `reboot`） | 会 |
| `POST /api/switch-and-power?system=windows&action=reboot` | 先切下次启动，成功后立刻执行电源动作 | 会 |

**① 服务活着吗（不需要 token）**

```bash
curl -s http://$HOST:12300/health
```

预期回答（HTTP 200）：

```json
{"ok":true,"service":"refind-switcher"}
```

**② 看当前状态**

```bash
curl -s -H "X-Auth-Token: $TOKEN" http://$HOST:12300/api/status
```

怎么读：`message` 是给人看的一句话（例如 `下次启动：Windows`）；`variables` 是三个模板文件的存在与选中情况。

```json
{
  "ok": true,
  "varsDir": "C:\\EFI\\refind\\vars",
  "message": "下次启动：Windows",
  "variables": [
    {"system":"linux","displayName":"Linux","exists":true,"active":false},
    {"system":"windows","displayName":"Windows","exists":true,"active":true},
    {"system":"macos","displayName":"macOS","exists":true,"active":false}
  ]
}
```

**③ 切换下次启动（不重启）**

```bash
curl -s -X POST -H "X-Auth-Token: $TOKEN" "http://$HOST:12300/api/switch?system=linux"
```

预期回答（HTTP 200，`pending:false` 表示已执行）：

```json
{"ok":true,"pending":false,"channel":"http","message":"下次启动：Linux"}
```

**④ 电源动作（默认重启）**

```bash
curl -s -X POST -H "X-Auth-Token: $TOKEN" "http://$HOST:12300/api/power"                  # 重启（缺省）
curl -s -X POST -H "X-Auth-Token: $TOKEN" "http://$HOST:12300/api/power?action=shutdown"   # 关机
curl -s -X POST -H "X-Auth-Token: $TOKEN" "http://$HOST:12300/api/power?action=sleep"      # 睡眠
```

预期回答：`{"ok":true,"pending":false,"channel":"http","message":"重启系统（已提交）"}`（关机/睡眠分别是「关机（已提交）」「睡眠（已提交）」）。

**⑤ 一条命令搞定“切到 Windows 并重启”（最常用的自动化姿势）**

```bash
curl -s -X POST -H "X-Auth-Token: $TOKEN" \
  "http://$HOST:12300/api/switch-and-power?system=windows&action=reboot"
```

预期回答：`{"ok":true,"pending":false,"channel":"http","message":"下次启动：Windows；重启系统（已提交）"}`

**⑥ 看最近请求（排查“我发的请求到底被执行了吗”）**

```bash
curl -s -H "X-Auth-Token: $TOKEN" http://$HOST:12300/api/requests
```

`status` 字段含义：`pending` 已收到、等本机确认（倒计时中）/ `executed` 已执行 / `cancelled` 本机取消了 / `failed` 执行失败 / `rejected` 被拒（token 错、参数错、或倒计时期间又来一条）。

**预期回答怎么读（通用）**

- `ok:true` 且 `pending:false` → 动作已经真的做了。
- `ok:true` 且 `pending:true` → 进了倒计时：`executeInSeconds` 是还剩几秒，`requestId` 可与 `/api/requests` 对上；这期间**再来一条会得到 HTTP 409**（避免连着切两次）。
- `401` → token 不对或没带，响应体 `{"ok":false,"error":"invalid_token","message":"token 缺失或无效"}`（故意不区分“没带”和“不对”）。
- `400` → 参数写错（`unknown_system` / `unknown_action`）；`404` → 路径写错；`405` → 用了 PUT/DELETE 之类；`413` → body 太大；`409` → 已有待执行请求。

### 电源动作：重启 / 关机 / 睡眠

主窗口三个按钮、托盘「电源操作」子菜单、HTTP `/api/power`、CLI `--action` 走的是**同一套命令表**：

| 平台 | 重启 | 关机 | 睡眠 |
| --- | --- | --- | --- |
| Windows | `shutdown /r /t 0` | `shutdown /s /t 0` | `rundll32.exe powrprof.dll,SetSuspendState 0,1,0` |
| Linux | `systemctl reboot`（失败退 `loginctl reboot` → `pkexec systemctl reboot`） | `systemctl poweroff`（同样三级退让） | `systemctl suspend`（退 `loginctl suspend` → `pkexec`） |
| macOS | `osascript` 管理员授权执行 `/sbin/shutdown -r now` | 同上（`-h now`） | 先试 `System Events` 睡眠 → 失败 `sudo -n pmset sleepnow` → 再失败走管理员授权 |

两条安全约定（一直有效）：

- **只有「命令不存在」才换下一条候选**。命令能执行但失败（没权限、用户取消了授权框）就立刻停手并把原因显示出来——不会“取消授权之后换种方式照样关机”。
- **提权弹窗可能停留很久**，动作都在后台线程执行；重复点击会被忽略（按钮和托盘项同时置灰）。

Windows 睡眠的实话实说：`SetSuspendState 0,1,0` 在「休眠已启用」的机器上会变成**休眠**而不是睡眠。应用会在动手前做一次**只读**预检（注册表 `HKLM\SYSTEM\CurrentControlSet\Control\Power\HibernateEnabled`，读不到就跑 `powercfg /a`），把结论写进回执与日志（例如「本次会进入睡眠（S3）」或「休眠已启用，本次会变休眠」）。想自己确认：

```powershell
# Windows：只读查询，不改任何东西
reg query "HKLM\SYSTEM\CurrentControlSet\Control\Power" /v HibernateEnabled
powercfg /a
```

```bash
# Linux：只读查询
cat /sys/power/state                                   # 看到 freeze mem disk 就是支持睡眠/休眠
systemctl status sleep.target --no-pager | head -3     # 有 systemd 时
```

```bash
# macOS：只读查询
pmset -g | grep -i hibernatemode
```

### 通知弹窗与延迟执行（可配 0 秒立即执行）

默认：**任何**远程请求（HTTP 或 ssh）都先弹一个置顶小窗，倒计时 5 秒后自动执行。

- 弹窗有「立即执行」和「取消」；**关掉窗口就等于取消**（误关不会把机器关掉）。
- 主窗口「远程控制」区有「待执行请求」卡片（含倒计时、来源、渠道），也能在这里取消/立即执行。
- 想不打扰：把该渠道 `delaySeconds` 设成 `0`（立即执行，不再弹倒计时窗，只弹一条 3 秒自动消失的结果提示），或把 `notify` 关掉（不弹窗但延迟照旧，取消入口只剩主窗口）。
- **两条渠道分开配**：`channels.http`（webhook / curl / 面板）与 `channels.cli`（ssh / CLI 转发）。典型用法：让 ssh 立即执行（`delaySeconds:0`），让对外的 webhook 留 5 秒倒计时给自己留后路。
- 本机自己点按钮/托盘项**不会**弹窗、也不进待执行队列。

### 常见客户端与面板

任何能发 HTTP 请求、能自定义 Header 的东西都能用：

| 客户端 / 面板 | 怎么用 |
| --- | --- |
| **curl / 脚本 / CI** | 就是上面端点表那些命令；token 放 `X-Auth-Token` 头 |
| **Postman / ApiPost / Insomnia** | 新建 `POST` → URL `http://<IP>:12300/api/power?action=reboot` → Headers 加 `X-Auth-Token: <token>` → Send；看到 `"ok":true` 就是成功 |
| **UpSnap**（自托管网络唤醒面板） | 设备里把自定义 HTTP 动作指向 `/api/switch-and-power?system=windows&action=reboot`，Header 带 `X-Auth-Token`（token 也可写进 query：`?...&token=<token>`） |
| **PowerControl / 手机 Wake-on-LAN 类 App** | 在「唤醒/自定义 HTTP」里填 URL + Header；只支持“发一个 GET/POST”的，用 `?token=` 放 query 最省事 |
| **Home Assistant / Node-RED / n8n** | `rest_command` / HTTP Request 节点，POST 到 `/api/power?action=sleep`，Header 带 token |
| **路由器 / 群晖的计划任务** | 自带 curl 的，`curl` 一条即可 |

小技巧：面板只方便填一个 URL 时，把 token 放 query：`http://<IP>:12300/api/power?action=reboot&token=<token>`。能用 Header 就优先 Header（URL 容易被日志记下来）。

### 给每台机器起个名字（多机管理）

一台机器管多台时，建议按「设备名 + 动作」组织，别让一堆 IP 散在脚本里：

| 设备名 | 干什么 | 调用 |
| --- | --- | --- |
| `nas` | 切到 Linux 并重启 | `curl -s -X POST "http://nas:12300/api/switch-and-power?system=linux&action=reboot" -H "X-Auth-Token: $NAS_TOKEN"` |
| `nas` | 只关机 | `curl -s -X POST "http://nas:12300/api/power?action=shutdown" -H "X-Auth-Token: $NAS_TOKEN"` |
| `nas` | 睡眠 | `curl -s -X POST "http://nas:12300/api/power?action=sleep" -H "X-Auth-Token: $NAS_TOKEN"` |
| `desk` | 切到 Windows 并重启 | `curl -s -X POST "http://desk:12300/api/switch-and-power?system=windows&action=reboot" -H "X-Auth-Token: $DESK_TOKEN"` |
| `desk` | 看状态 | `curl -s -H "X-Auth-Token: $DESK_TOKEN" "http://desk:12300/api/status"` |

一台设备的所有动作（状态 / 切换 / 重启 / 关机 / 睡眠）放一组，token 变量名用 `设备名_TOKEN`（Windows 上写 `$env:NAS_TOKEN`）。给每台机器在路由器里配静态 IP 或 DHCP 保留，名字才稳。

### ssh 远程调用两种用法

**用法一：ssh 直连（推荐，不额外暴露端口）**

同一个可执行文件带参数就是 CLI；应用正在运行时它会自动把命令转发给 GUI 执行（于是与 webhook 走同一套通知/延迟，渠道算 `cli`）；应用没在运行时它自己执行（无界面，相当于立即执行）。

```bash
# Linux / macOS 客户端
ssh user@192.168.1.50 'refind-switcher --status'
ssh user@192.168.1.50 'refind-switcher --switch windows'                    # 只切换
ssh user@192.168.1.50 'refind-switcher --switch windows --action reboot'    # 切换并重启
ssh user@192.168.1.50 'refind-switcher --action sleep'                      # 只睡眠
ssh user@192.168.1.50 'refind-switcher --status --json'                     # 脚本友好的 JSON
```

```powershell
# Windows 客户端（win10/11 自带 OpenSSH 客户端）
ssh user@192.168.1.50 "refind-switcher.exe --status"
ssh user@192.168.1.50 "refind-switcher.exe --switch windows --action reboot"
ssh user@192.168.1.50 "refind-switcher.exe --action sleep"
```

CLI 参数：

| 参数 | 说明 |
| --- | --- |
| `--status` | 变量目录、三个模板是否存在、下次启动是哪个 |
| `--switch <linux\|windows\|macos>` | 切换下次启动（单独用不重启） |
| `--action <reboot\|shutdown\|sleep>` | 电源动作；**缺省是 `reboot`**；别名 `restart` / `poweroff` / `halt` / `suspend` 也认 |
| `--switch X --action Y` | 先切换，成功后立刻执行 Y |
| `--json` | 输出与 HTTP 响应同结构的 JSON（脚本友好） |
| `--port <n>` | 临时指定端口（调试/多实例） |
| `--help` / `--version` | 用法 / 版本 |

退出码：`0` 成功 / `1` 动作失败 / `2` 参数错误 / `3` 需要本机授权 / `4` 变量目录未就绪。

```bash
if ssh user@nas 'refind-switcher --switch linux --action reboot'; then
  echo "已提交"
else
  echo "失败，退出码 $?"
fi
```

**用法二：SSH 隧道（不想暴露端口，又想用 curl/面板时）**

```bash
# 把本机 12300 隧道到目标机 12300（目标机 httpBind 可以就是 127.0.0.1）
ssh -N -L 12300:127.0.0.1:12300 user@192.168.1.50

# 另一个终端：直接对本机 12300 发请求，等同于在目标机上本机调用
curl -s -H "X-Auth-Token: $TOKEN" http://127.0.0.1:12300/api/status
curl -s -X POST -H "X-Auth-Token: $TOKEN" "http://127.0.0.1:12300/api/power?action=reboot"
```

隧道这一路请求来自目标机的 loopback，所以客户端带 `channel=cli` 会被算作 `cli` 渠道；从局域网直连时伪造 `channel=cli` 无效（一律按 `http` 渠道）。

### 远程需要“本机授权”时怎么办

默认 `remoteAllowGuiAuth = false`：远程/CLI 请求**不会**在目标机弹授权框，遇到必须授权的操作会直接失败并说明原因（HTTP 里是 `ok:false` + `message`，CLI 退出码 `3`）。这样远程请求不会“挂在那里等有人去点确认”。

想让远程一次成功，就在**目标机**上提前把权限配好：

```bash
# Linux：免密 sudo（把 <user> 换成实际用户；只放开这三个命令，别写 ALL）
#   sudo visudo -f /etc/sudoers.d/refind-switcher
<user> ALL=(ALL) NOPASSWD: /usr/bin/systemctl reboot, /usr/bin/systemctl poweroff, /usr/bin/systemctl suspend

# Linux：顺手把 ESP 预挂载好（fstab），应用就不需要临时提权挂载
#   /etc/fstab 里加一行（UUID 用 blkid 查）
UUID=1234-ABCD  /boot/efi  vfat  defaults,nofail,umask=0022  0  2
sudo mount -a
```

```bash
# macOS：免密 sudo（睡眠/重启会用到）
#   sudo visudo
<user> ALL=(ALL) NOPASSWD: /sbin/shutdown, /usr/bin/pmset
```

```powershell
# Windows：让 ESP 一直挂着（管理员 PowerShell / diskpart，一次就够）
diskpart
  select disk 0
  select partition 1
  assign letter=S
```

如果你确实想让远程请求在目标机弹授权框（有人在场），把 `remoteAllowGuiAuth` 打开即可——但那意味着必须有人在机器前点“确认”。

---

## 应用设置

主窗口「应用设置」区域：

- **开机启动**：登录系统后自动启动应用。
  - macOS 13 及以上调用系统登录项接口 `SMAppService.mainApp` 注册，条目出现在「系统设置 → 通用 → 登录项与扩展」。该接口要求应用带有效代码签名，因此 `tauri.conf.json` 默认配了 ad-hoc 签名（`bundle.macOS.signingIdentity = "-"`）；未签名、未以 `.app` 形式运行或 macOS 12 及以下会自动退回 `~/Library/LaunchAgents/rEFInd Switcher.plist`，界面会提示当前用的是兼容方式。签名版本生效后，早期版本遗留的 LaunchAgent 会在启动时自动清理。
  - Windows 写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`；Linux 写 `~/.config/autostart`（即使以 root 执行也会识别 `SUDO_USER`/`PKEXEC_UID`，写回实际登录用户）。
  - 若系统返回「需要批准」（`requiresApproval`），主窗口会给出提示与「打开登录项与扩展设置」按钮。
- **启动时隐藏主界面**：登录/启动后只在托盘待命。
- **点击窗口 ❌ 时**：`最小化到托盘` 或 `退出程序`。
- **远程控制**（详见 [远程控制](#远程控制http-服务)）：HTTP 服务开关、监听地址/端口、token（显示/复制/重新生成）、远程请求通知开关、两条渠道各自的通知与延迟、远程可触发本机授权框、待执行请求卡片、最近请求列表、可复制的调用示例。

设置持久化在配置目录的 `settings.json`。**主窗口和托盘菜单双向联动**：任何一边改了都会立刻保存、重建托盘菜单，并广播同一份状态快照给主窗口与通知弹窗，两边不会出现“一边勾选、一边没变”。开机启动以系统真实状态为准，在系统设置里手动改过之后重新打开主窗口会自动同步。

---

## 环境要求

- Node.js 18+
- Rust 1.77.2+ 与 Cargo
- 平台构建依赖见 [Tauri 2 Prerequisites](https://v2.tauri.app/start/prerequisites/)

## 开发运行

```bash
npm install
npm run tauri dev
```

构建安装包：

```bash
npm run tauri build
```

平台专用构建：

```bash
# Linux: deb + rpm + AppImage
npm run build:linux

# macOS Intel
npm run build:mac-intel

# macOS Apple Silicon
npm run build:mac-arm

# macOS 通用二进制，同时包含 Intel 和 Apple Silicon
npm run build:mac-universal
```

Linux 包必须在 Linux 环境生成；macOS 包必须在 macOS 环境生成。请先在目标平台安装 Tauri 2 所需的系统依赖（Linux 需要 WebKit/GTK、AppImage 工具链等），再运行对应命令。

## 构建产物

所有构建与打包只使用默认目录 `src-tauri/target`，不要再指定 `--target-dir` 或设置 `CARGO_TARGET_DIR`（此前 Windows 侧产生的 `src-tauri/target-new` 已废弃）。

- Windows：`src-tauri/target/release/bundle/nsis/*.exe`、`src-tauri/target/release/bundle/msi/*.msi`
- macOS：`src-tauri/target/release/bundle/macos/*.app`、`src-tauri/target/release/bundle/dmg/*.dmg`
- Linux：`src-tauri/target/release/bundle/deb/*.deb`、`.../rpm/*.rpm`、`.../appimage/*.AppImage`
- 指定 `--target <triple>` 时位于 `src-tauri/target/<triple>/release/bundle`

---

## 调试环境变量

都在启动应用**之前**设好。除 `REFIND_SWITCHER_POWER_DRY_RUN` 外，其余能覆盖 `settings.json` 里的对应项（但**不会**写进文件，只对这次运行生效）。

| 环境变量 | 作用 |
| --- | --- |
| `REFIND_SWITCHER_POWER_DRY_RUN=1` | 只回显将要执行的电源命令，**不真的**重启/关机/睡眠（遥控调试必用） |
| `REFIND_SWITCHER_TRACE=1` | 打开 trace 日志（写 `trace.log`，同时打到 stderr）：菜单重建、事件、菜单动作、设置读写、扫描/切换耗时、电源动作、HTTP 请求（token 只记前 8 位指纹） |
| `REFIND_SWITCHER_HTTP_ENABLED=0\|1` | 覆盖 HTTP 服务开关 |
| `REFIND_SWITCHER_HTTP_BIND=127.0.0.1` | 覆盖监听地址 |
| `REFIND_SWITCHER_HTTP_PORT=12400` | 覆盖端口（也影响 CLI 找实例时的端口） |
| `REFIND_SWITCHER_VARS_DIR=/path/to/vars` | 直接指定变量目录（跳过自动扫描/挂载，验证用） |
| `REFIND_SWITCHER_CONFIG_DIR=/path/to/dir` | 把配置目录（`settings.json` / `trace.log`）指到别处：跑第二个实例或做验证时不会碰到真实配置 |

```bash
# 例子：不动真实配置、不动真实 ESP、不真的重启，只验证一条远程请求会执行什么命令
REFIND_SWITCHER_CONFIG_DIR=/tmp/rs-sandbox \
REFIND_SWITCHER_VARS_DIR=/tmp/rs-vars \
REFIND_SWITCHER_HTTP_PORT=12400 \
REFIND_SWITCHER_POWER_DRY_RUN=1 \
REFIND_SWITCHER_TRACE=1 \
./refind-switcher
# 然后（token 取沙箱 settings.json 里那个）
curl -s -X POST -H "X-Auth-Token: <token>" \
  "http://127.0.0.1:12400/api/switch-and-power?system=linux&action=reboot"
```

---

## 权限说明

写入 ESP 里的 rEFInd 变量文件需要管理员/root 权限。

- **Windows**：请以管理员身份运行应用（开发时也要在管理员终端执行 `npm run tauri dev`）。
- **Linux**：默认以普通用户运行。ESP 未挂载时会通过 `pkexec` 弹系统授权并临时挂载（挂载选项绑定到实际登录用户）；已挂载且可写时不要求 root。
- **macOS**：ESP 未挂载时需要先用系统工具挂载，或在主窗口指定已挂载的变量目录。
- **远程/CLI**：默认不弹本机授权框，遇到必须授权的情况直接失败（见 [远程需要“本机授权”时怎么办](#远程需要本机授权时怎么办)）。

> Linux GNOME 默认没有传统托盘。托盘图标不显示时需启用支持 StatusNotifierItem 的扩展，或直接用主窗口。Linux AppIndicator 不区分托盘左键/右键：主窗口通过托盘菜单里的「显示主窗口」打开；Windows/macOS 支持左键打开主窗口、右键弹菜单。

---

## 常见问题

### 远程访问不了：按这个顺序查

1. 主窗口「远程控制」里的状态是不是「正在监听 0.0.0.0:12300」？不是就看它给出的原因（常见：token 为空、端口被别的程序占了）。
2. 目标机上 `curl http://127.0.0.1:12300/health` 通不通？通说明服务没问题，问题在网络/防火墙。
3. 从另一台机器 `curl http://<目标IP>:12300/health`：不通就去 [三端各自的防火墙处理](#三端各自的防火墙处理)，并确认 `httpBind` 不是 `127.0.0.1`。
4. `/health` 通但 `/api/status` 返回 401：token 不对（重新复制一次）。
5. 返回 409：有条待执行请求在倒计时，等它执行完或去主窗口/弹窗点「取消」。

### 端口被占用 / 想换端口

应用**不会**自动换端口（避免你以为在 12300 其实是别的）。换端口：主窗口「监听地址 / 端口」改完点「应用」，或 `REFIND_SWITCHER_HTTP_PORT=12400` 启动。谁占了：

```bash
ss -ltnp | grep 12300                 # Linux
lsof -nP -iTCP:12300 -sTCP:LISTEN     # macOS
netstat -ano | findstr 12300          # Windows（再按 PID 查进程）
```

### 睡眠变成了休眠

Windows 上如果启用了休眠，`SetSuspendState 0,1,0` 会进休眠。应用每次都会**只读**预检并告诉你结论；不想变休眠就先关掉休眠（这会改系统设置，自己决定）：

```powershell
powercfg /hibernate off     # 关闭休眠（会删除 hiberfil.sys，需管理员）
```

### AppImage 打包失败

出现 `strip: unknown type [0x13] section '.relr.dyn'` 或 `failed to run linuxdeploy` 时，跳过 `linuxdeploy` 的 strip：

```bash
NO_STRIP=1 npm run build:linux
```

该环境变量只影响本次命令，不会修改项目配置。

### Linux 应用设置保存失败

除“开机启动”外其他设置保存后恢复原值，先检查配置目录属主：

```bash
ls -ld ~/.config/com.example.refind-switcher
```

属主显示为 `root`（此前用 `sudo`/`pkexec` 跑过应用）就改回来：

```bash
sudo chown -R "$(id -un):$(id -gn)" ~/.config/com.example.refind-switcher
```

开机启动写的是 `~/.config/autostart`，在这个问题下仍可能正常工作。

### 想看“到底发生了什么”

主窗口「远程控制 → 最近请求」或 `GET /api/requests` 看最近 20 条；更细的过程用 `REFIND_SWITCHER_TRACE=1`（或设置里打开 `debugTrace`）看配置目录下的 `trace.log`——里面**不会有**完整 token。

---

## 注意

- 应用只修改 rEFInd 的 `PreviousBoot` 嗅探状态，不直接改引导项配置；三个 `PreviousBoot-*` 模板文件始终只读。
- HTTP 是**明文**（没有 HTTPS），token 必填但会以明文过网：只用在可信局域网或 SSH 隧道里，并保持 `settings.json` 的正常读权限（别 `chmod 777`）。
- 默认 `0.0.0.0` 是为了“装好就能被远程调用”；不需要就改成 `127.0.0.1` 并走隧道。
- 正式发布前建议替换品牌图标，并把 `bundle.macOS.signingIdentity` 换成 Apple Developer ID 证书（或用 `APPLE_SIGNING_IDENTITY` 覆盖）。ad-hoc 签名只保证系统登录项接口和本地运行可用，未做公证，首次打开仍需在「隐私与安全性」中放行。