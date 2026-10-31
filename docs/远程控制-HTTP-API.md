# 远程控制（HTTP 服务）— 完整参考

> 本文是 AnyBoot 远程控制能力的完整参考（原 README「远程控制」章节归档）。
> 快速上手见 [README](../README.md#远程控制)。

## 默认行为与安全默认值

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
| `remoteAllowGuiAuth` | `true` | 远程/CLI 请求**可以**弹本机授权框 |

必须知道的三件事：

1. **是明文 HTTP，不是 HTTPS**。token 会以明文经过网络，请只在可信局域网或 SSH 隧道里用。
2. **token 必填**。服务启动的前提是 token 非空；日志里只记 token 前 8 位指纹，完整值不会写进日志，也不会出现在托盘菜单和通知弹窗里（防截图）。
3. **默认监听 `0.0.0.0`**。不希望局域网能访问就把监听地址改成 `127.0.0.1`，然后走 SSH 隧道。

## token 去哪看、怎么换

三个入口，改哪个都一样（同一份 `settings.json`）：

- **主窗口 → 远程控制**：有「显示当前 / 复制 / 重新生成」按钮，还有**自己填一个 token** 的输入框（4–128 位可见 ASCII，字母/数字/符号都行，不能有空格和中文）。点「保存」或「重新生成」都会**关掉旧监听、按新 token 重新绑定端口**，旧 token 立即失效；状态栏会如实报出重启结果（`监听已重启：0.0.0.0:12300`，或者绑不上的原因）。旁边是监听地址/端口输入框和「复制命令」。
- **托盘右键 → 远程控制区**：紧跟一个灰标题「远程控制」，下面是**监听状态**一行（`正在监听 0.0.0.0:12300` / 失败原因），再往下是 HTTP 服务开关、远程请求通知开关、远程执行延迟预设（立即 / 3 / 5 / 10 / 30 秒），以及「远程控制设置…」（打开主窗口并滚到该区）。
- **直接改文件**：`settings.json` 里的 `httpToken`。改完在界面上再动一下任意远程设置，或在主窗口开关一次服务即可生效。

> 自定义 token 的建议：别用 `1234` 这种 —— 这个端口默认监听 `0.0.0.0`，能拿到 token 的人就能切你的启动项、重启/关机你的机器。用 `openssl rand -hex 16` 这类命令生成。
> 清空 token = 服务不再启动（这是刻意的安全默认：没有 token 就没有鉴权）。

配置目录（放 `settings.json`，日志 `trace.log` 也在同一目录）：

| 平台 | 路径 |
| --- | --- |
| Windows | `%APPDATA%\com.example.anyboot\`（即 `C:\Users\<你>\AppData\Roaming\com.example.anyboot\`） |
| macOS | `~/Library/Application Support/com.example.anyboot/` |
| Linux | `~/.config/com.example.anyboot/` |

忘了 token：主窗口「显示」按钮，或直接读文件。

```bash
# Linux / macOS
python3 -c "import json,os;print(json.load(open(os.path.expanduser('~/.config/com.example.anyboot/settings.json')))['httpToken'])"
```

```powershell
# Windows PowerShell
(Get-Content "$env:APPDATA\com.example.anyboot\settings.json" | ConvertFrom-Json).httpToken
```

换 token 后旧 token **立刻失效**（服务会用新 token 重建）。

## 三端各自的防火墙处理

应用能监听，但系统防火墙可能挡着外部访问。

```powershell
# Windows（管理员 PowerShell）：放行 12300/TCP
New-NetFirewallRule -DisplayName "AnyBoot HTTP" -Direction Inbound -Protocol TCP -LocalPort 12300 -Action Allow
# 撤销
Remove-NetFirewallRule -DisplayName "AnyBoot HTTP"
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

## 端点表（每条都给了可复制的 curl）

除 `/health` 外**所有端点都要 token**。token 三种放法任选：请求头 `X-Auth-Token: <token>`（推荐）、query `?token=<token>`、表单/JSON body 里的 `token` 字段。

```bash
# 先把这两个变量设好，后面直接复制
TOKEN='把主窗口里复制的 token 贴这里'
HOST='192.168.1.50'      # 目标机的局域网 IP；本机测试用 127.0.0.1
```

| 端点 | 干什么 | 会改状态吗 |
| --- | --- | --- |
| `GET /health` | 服务活着吗 + **这台机器是谁**（`machineId` / `machineName`，免 token） | 不会 |
| `GET /api/ws` | 状态快照的**推送通道**（WebSocket；握手用 `?token=` 或 cookie，连上即得一帧完整快照，之后每次变更推一帧） | 不会 |
| `GET /api/ui-state` | 当前状态快照（轮询兜底 / 手动刷新 / 下拉刷新用） | 不会 |
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
{"ok":true,"service":"anyboot","machineId":"9f2c…（32 位十六进制）","machineName":"书房台式机"}
```

`machineId` 是**本机标识**：首次运行生成一次、存在 `settings.json` 里，重启不变。
手机 / 网页客户端拿它认出「这几条 IP 其实是同一台电脑」—— 一台电脑多块网卡、
或者 DHCP 每次给不同 IP，客户端只保留**一份页面**、自动挑能通的那个地址
（详见 [`mobile/README.md`](../mobile/README.md)）。它免 token 返回，因为客户端在
填 token 之前就要用它归并；不含任何机密，`machineName` 取不到时是空串。

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

## 电源动作：重启 / 关机 / 睡眠

主窗口三个按钮、托盘上的三个电源菜单项、HTTP `/api/power`、CLI `--action` 走的是**同一套命令表**：

| 平台 | 重启 | 关机 | 睡眠 |
| --- | --- | --- | --- |
| Windows | `shutdown /r /t 0` | `shutdown /s /t 0` | `rundll32.exe powrprof.dll,SetSuspendState 0,1,0` |
| Linux | `systemctl reboot`（失败退 `loginctl reboot` → `pkexec systemctl reboot`） | `systemctl poweroff`（同样三级退让） | `systemctl suspend`（退 `loginctl suspend` → `pkexec`） |
| macOS | 先走 AppleEvent → `System Events`（和苹果菜单的「重新启动」同一条路，**任何用户都不用管理员密码、不用改配置**；首次弹一次「自动化」授权，允许后永久免框）→ 用不了才试 `sudo -n /sbin/shutdown -r now`（配了免密 sudo 的机器）→ 最后才是 `osascript` 管理员授权框 | 同上（`shut down` / `-h now`） | 同上（`sleep`，再 `sudo -n pmset sleepnow`，最后管理员授权） |

两条安全约定（一直有效）：

- **只有「这条候选在本机用不了」才换下一条**（命令不存在、`sudo -n` 没配免密、macOS 的「自动化」未授权）。“命令能执行但失败”（没权限、用户当场取消了授权框）就立刻停手并把原因显示出来——不会“取消授权之后换种方式照样关机”。
- **提权弹窗可能停留很久**，动作都在后台线程执行；重复点击会被忽略（按钮和托盘项同时置灰）。

### macOS：一次授权，永久免密

macOS 的重启 / 关机 / 睡眠**不需要管理员密码**：走的是 `System Events` 的 AppleEvent（系统内部转成 `loginwindow` 的 `aevt`/`rst`/`sdn`），
也就是苹果菜单里那三个菜单项用的同一条链路。代价只是**首次**会弹一次系统的「自动化」授权框：

> “AnyBoot”想要控制“System Events”。

点「允许」后写进当前用户的 TCC 库（`~/Library/Application Support/com.apple.TCC/TCC.db`），此后永久免框、不再询问，也不需要 root、不需要 `sudoers`、不需要任何配置文件。
换个用户登录时，该用户要自己点一次（这是 macOS 的按用户授权模型，应用无法代劳）。

- 忘了点过什么：**系统设置 → 隐私与安全性 → 自动化** 里能看到并随时改。
- 想重新弹一次授权框：`tccutil reset AppleEvents com.example.anyboot`。
- **开发期注意**：TCC 记的是应用的代码签名要求。当前构建是 ad-hoc 签名（`tauri.conf.json` 里 `signingIdentity: "-"`），
  签名随构建内容变化（cdhash 变），所以**每次重新构建并覆盖 /Applications 里的 app，才会再弹一次**（一次点击，不用密码）。
  正式发布用固定的 Developer ID 证书签名后，授权对同一份签名永久有效。

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

## 通知弹窗与延迟执行（可配 0 秒立即执行）

默认：**任何**远程请求（HTTP 或 ssh）都先弹一个置顶小窗，倒计时 5 秒后自动执行。

- 弹窗有「立即执行」和「取消」；**关掉窗口就等于取消**（误关不会把机器关掉）。
- 弹窗的高度按内容**自适应**（前端量出「刚好兜住最后一行文字」的高度再报给后端），位置固定在工作区右下角 —— 用**工作区**而不是屏幕，所以不会压住任务栏。
- 主窗口「远程控制」区有「待执行请求」卡片（含倒计时、来源、渠道），也能在这里取消/立即执行。
- 想不打扰：把该渠道 `delaySeconds` 设成 `0`（立即执行，不再弹倒计时窗，只弹一条 3 秒自动消失的结果提示），或把 `notify` 关掉（不弹窗但延迟照旧，取消入口只剩主窗口）。
- **两条渠道分开配**：`channels.http`（webhook / curl / 面板）与 `channels.cli`（ssh / CLI 转发）。典型用法：让 ssh 立即执行（`delaySeconds:0`），让对外的 webhook 留 5 秒倒计时给自己留后路。
- 本机自己点按钮/托盘项**不会**弹窗、也不进待执行队列。
- 弹窗出现时会**响一声内置的 KakaoTalk 风格提示音**（可换自定义音频、也可关掉）。

## 常见客户端与面板

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

## 给每台机器起个名字（多机管理）

一台机器管多台时，建议按「设备名 + 动作」组织，别让一堆 IP 散在脚本里：

| 设备名 | 干什么 | 调用 |
| --- | --- | --- |
| `nas` | 切到 Linux 并重启 | `curl -s -X POST "http://nas:12300/api/switch-and-power?system=linux&action=reboot" -H "X-Auth-Token: $NAS_TOKEN"` |
| `nas` | 只关机 | `curl -s -X POST "http://nas:12300/api/power?action=shutdown" -H "X-Auth-Token: $NAS_TOKEN"` |
| `nas` | 睡眠 | `curl -s -X POST "http://nas:12300/api/power?action=sleep" -H "X-Auth-Token: $NAS_TOKEN"` |
| `desk` | 切到 Windows 并重启 | `curl -s -X POST "http://desk:12300/api/switch-and-power?system=windows&action=reboot" -H "X-Auth-Token: $DESK_TOKEN"` |
| `desk` | 看状态 | `curl -s -H "X-Auth-Token: $DESK_TOKEN" "http://desk:12300/api/status"` |

一台设备的所有动作（状态 / 切换 / 重启 / 关机 / 睡眠）放一组，token 变量名用 `设备名_TOKEN`（Windows 上写 `$env:NAS_TOKEN`）。给每台机器在路由器里配静态 IP 或 DHCP 保留，名字才稳。

## ssh 远程调用两种用法

**用法一：ssh 直连（推荐，不额外暴露端口）**

同一个可执行文件带参数就是 CLI；应用正在运行时它会自动把命令转发给 GUI 执行（于是与 webhook 走同一套通知/延迟，渠道算 `cli`）；应用没在运行时它自己执行（无界面，相当于立即执行）。

```bash
# Linux / macOS 客户端
ssh user@192.168.1.50 'anyboot --status'
ssh user@192.168.1.50 'anyboot --switch windows'                    # 只切换
ssh user@192.168.1.50 'anyboot --switch windows --action reboot'    # 切换并重启
ssh user@192.168.1.50 'anyboot --action sleep'                      # 只睡眠
ssh user@192.168.1.50 'anyboot --status --json'                     # 脚本友好的 JSON
```

```powershell
# Windows 客户端（win10/11 自带 OpenSSH 客户端）
ssh user@192.168.1.50 "anyboot.exe --status"
ssh user@192.168.1.50 "anyboot.exe --switch windows --action reboot"
ssh user@192.168.1.50 "anyboot.exe --action sleep"
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
if ssh user@nas 'anyboot --switch linux --action reboot'; then
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

## 远程需要“本机授权”时怎么办

默认 `remoteAllowGuiAuth = false`：远程/CLI 请求**不会**在目标机弹授权框，遇到必须授权的操作会直接失败并说明原因（HTTP 里是 `ok:false` + `message`，CLI 退出码 `3`）。这样远程请求不会“挂在那里等有人去点确认”。

想让远程一次成功，就在**目标机**上提前把权限配好：

```bash
# Linux：免密 sudo（把 <user> 换成实际用户；只放开这三个命令，别写 ALL）
#   sudo visudo -f /etc/sudoers.d/anyboot
<user> ALL=(ALL) NOPASSWD: /usr/bin/systemctl reboot, /usr/bin/systemctl poweroff, /usr/bin/systemctl suspend

# Linux：顺手把 ESP 预挂载好（fstab），应用就不需要临时提权挂载
#   /etc/fstab 里加一行（UUID 用 blkid 查）
UUID=1234-ABCD  /boot/efi  vfat  defaults,nofail,umask=0022  0  2
sudo mount -a
```

```bash
# macOS：不需要配任何东西。首次在本机点一次「重启系统」/「关机」/「睡眠」时
# 会弹一次系统授权框「“AnyBoot”想要控制“System Events”」——点「允许」即可，
# 之后永久免框（按用户记在 TCC 库里）。不想点也可以什么都不做：
# 应用会退回系统管理员授权框，功能不受影响。
# 想重新弹一次：tccutil reset AppleEvents com.example.anyboot
```

macOS 这条授权不是「给 root 权限」，而是**自动化（AppleEvents）**：应用通过 `System Events` 发
重启/关机/睡眠事件，和苹果菜单里那三个菜单项用的是同一条链路，所以本来就不需要管理员密码——
也不需要 `sudoers`。`/etc/sudoers.d/anyboot` 那条免密规则现在只是**可选**的中间候选（配了就在
自动化被拒时也免框），不配完全不影响使用。

```powershell
# Windows：让 ESP 一直挂着（管理员 PowerShell / diskpart，一次就够）
diskpart
  select disk 0
  select partition 1
  assign letter=S
```

如果你确实想让远程请求在目标机弹授权框（有人在场），把 `remoteAllowGuiAuth` 打开即可——但那意味着必须有人在机器前点“确认”。
