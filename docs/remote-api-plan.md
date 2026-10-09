# rEFInd Switcher：界面联动修复 + 远程控制（HTTP/Token/SSH）+ 电源动作

状态：**待你确认**。本文档分「已定稿」与「待确认/开放项」（§10 的 Q1–Q10）两部分；
动手前请先过一遍 §10，尤其 Q1/Q2/Q3。

日期：2026-10-09　目标分支：master　当前 HEAD：735a2aa（工作区干净，仅若干历史残留未跟踪文件）

---

## 0. 我理解的需求（复述，请核对）

1. **修 bug**：托盘右键菜单 ↔ 主窗口的「联动性还是不够」，仍有不一致/不即时的毛病。
2. **新增内置 HTTP 服务**：**默认开启**，端口 **12300**，让远程可以用 webhook 调用。
3. **新增 SSH 远程切换**：人能 ssh 到这台机器上完成「下次启动切换」这件事。
4. **HTTP 服务加 token**：token 可在**主窗口**或**托盘右键菜单**里设置；带上 token 后远程也能切换系统。
- **电源动作参数化**：除重启外还要能**关机**、**睡眠**，由**调用方传参**决定执行哪个动作，**不传默认重启**。
6. **通知弹窗 + 延迟执行**：有远程请求时弹窗提醒；主窗口（及托盘「应用设置」）可配置「通知弹窗开关」和「几秒后执行远程命令」，倒计时窗口内可取消（详见 §4.7）。**ssh/CLI 与 http webhook 两个渠道可分别配置**（Q14）。

---

### 0.1 决策表（Q1–Q14 全部已定）

已定（按你的回答，实现就按这个来；明细见 §10）：

| 编号 | 决定 |
| --- | --- |
| Q1 | 默认监听 `0.0.0.0:12300`，token 必填 |
| Q2 | **两条路都要**：HTTP（curl / webhook）+ **CLI 子命令**（ssh 调用带参数），CLI 设计见 §5 |
| Q3 | 需要本机授权框的操作**不弹窗**：远程/CLI 直接拒绝并给免密/预挂载指引（`remoteAllowGuiAuth=false`） |
| Q4 | 主窗口三个按钮：重启系统 / 关机 / 睡眠 |
| Q11 | 通知默认**开** + 延迟默认 **5 秒**，弹窗里可手动取消；主窗口可改成 0 = 立即执行 |
| Q12 | **自绘**置顶小窗（不引原生通知插件） |
| Q13 | 本机点击的动作**不**倒计时 |
| Q10′ | Windows 睡眠用 **C**：不做特殊处理（休眠开着时会变休眠），但**必须只读预检并说清楚**。你这台机器实测 `HibernateEnabled=0x0` + `powercfg /a`「休眠：尚未启用休眠」→ 会真睡眠（S3） |
| Q14 | ssh/CLI **也走**通知 + 延迟，但**按渠道分开配置**：`channels.http` 与 `channels.cli` 各自的 通知开关 + 延迟秒数（见 §4.7） |
| Q5/Q6/Q7/Q8/Q9 | 你没反对，按建议执行：不做 HTTPS、`tiny_http`、token 自动生成（空 token 不启动服务）、端口占用报错不换端口、不做 IP 白名单 |

没有任何待你拍板的了——阶段 0 起可以开工。

## 1. 现状盘点（先确认我读的是同一份代码）

| 关注点 | 位置 |
| --- | --- |
| 托盘菜单重建（每次全量重建） | `src-tauri/src/lib.rs:1084` `update_tray_menu` |
| 初始托盘菜单（只有 status/show/refresh/quit 四项） | `src-tauri/src/lib.rs:2100-2104`（`setup` 内） |
| 菜单事件分发（字符串前缀/后缀匹配） | `src-tauri/src/lib.rs:1188` `spawn_menu_action` |
| 菜单 id 生成（`setting-close-action:{Debug}` → lowercase） | `src-tauri/src/lib.rs:1163-1170` |
| 设置结构 / 存盘 / 主窗口命令 | `lib.rs:229` / `lib.rs:1236` / `lib.rs:1987-2036` |
| 设置的「两份真相」 | 托盘读 `stored_settings`（`lib.rs:1091`），主窗口读 `view_settings`（`lib.rs:1539`） |
| 状态快照与事件 | `AppState`/`AppStatus` `lib.rs:37`/`lib.rs:204`，`publish_status` `lib.rs:881` |
| 电源动作（仅重启） | 候选表 `lib.rs:1770-1793`、提权 `lib.rs:1797`、`run_restart` `lib.rs:1845`、`spawn_restart` `lib.rs:1896`、`RESTARTING` 原子标志 `lib.rs:1897` |
| 前端事件监听 / 按钮禁用 | `ui/app.js:209-227` / `ui/app.js:196-207` |
| 前端契约单测（断言事件名与命令名） | `lib.rs:2321` `frontend_listens_to_backend_events` |
| 打包配置（identifier / 图标 / 无 CSP） | `src-tauri/tauri.conf.json` |
| 现有依赖 | `Cargo.toml`：tauri 2(tray-icon)、tauri-plugin-autostart、serde、serde_json、sha2、hex（Linux 加 libc；macOS 加 objc2-*） |

现状机制一句话：**主窗口与托盘是两份独立 UI，靠「改设置 → 重建托盘菜单 + emit `settings-changed`」维持一致；状态靠 `status-changed` 事件补丁式更新。**

本机注意：现在有一个 `refind-switcher.exe`（PID 25304）正在运行——是你正在用的实例，本方案的所有验证都不会碰它（见 §8 的端口/沙箱约定）。

---

## 2. 「联动性不够」的定位结论

我没有 GUI 点击能力（托盘菜单无法在无头环境里点），所以分两类：**能直接从代码断定的缺陷**（已定稿，阶段 1 直接修）与**必须用现场日志定论的怀疑点**（阶段 0 取证）。

### 2.1 已能从代码断定（D1–D6）

- **D1 两份真相（一致性根因）**：托盘勾选状态取 `stored_settings`（`lib.rs:1091`），主窗口取 `view_settings`（系统真实状态，并把结果回写，`lib.rs:1539-1547`）。在系统设置里手动关掉登录项后，出现「主窗口已关闭 / 托盘仍勾着」的窗口期，直到下一次 `publish_settings` 或扫描。
- **D2 首次右键菜单是残缺的**：`setup` 先挂一个只有 4 项的菜单（`lib.rs:2100-2104`），完整菜单要等后台线程跑完 `refresh_and_update` → `update_tray_menu`（`lib.rs:2131-2132`）。冷启动/Linux 上 ESP 未挂载时扫描要 `lsblk` 甚至等 `pkexec` 授权，这段时间右键看到的**没有系统切换项、没有「重启系统」、没有「应用设置」**。这是最常见、最容易复现的「联动不够」。
- **D3 电源动作的状态不共享**：`RESTARTING` 只在 Rust 侧（`lib.rs:1897`），主窗口按钮的 disabled 只由本窗口自己的点击控制（`app.js:196-207`）。托盘触发重启时主窗口按钮照旧可点；主窗口触发时托盘「重启系统」照旧可点，第二次点击被静默丢弃（只 `eprintln!`，`lib.rs:1898-1900`），用户毫无反馈。
- **D4 事件是补丁式、无版本号**：`status-changed`/`settings-changed`/`restart-result` 三个事件各自更新 UI 的一部分，没有单调序号，也没有「同一份快照」概念；前端每次 `render` 都要重新拼状态，任何一次漏发/乱序都会留下不刷新的一角。
- **D5 设置应用失败不回滚**：`set_hide_from_dock_taskbar`（`lib.rs:2030`）先 `save_settings` 再 `apply_dock_taskbar_visibility`，apply 失败时返回 Err 给前端，但设置文件里已经是新值 → 前端 `loadSettings()` 读到新值、勾选框显示已生效，**系统实际状态没变，界面在说假话**（`ui/app.js:143-154` 的 catch 分支）。
- **D6 菜单 id 靠字符串前后缀解析**：id 生成（`lib.rs:1165`）与解析（`lib.rs:1199-1210`）两处手工同步，新增设置项极易漏；没有任何单测覆盖这条契约。
- **D7 启动即双扫描（阶段 0 trace 实测，见 todo 阶段 0 记录）**：启动时后台线程扫一次、前端加载后又 `invoke("refresh")` 扫一次，两次都全量重建托盘菜单（指纹完全相同）→ 直接对应 S1/S2 的菜单重建/闪断。阶段 1 用内容指纹 + 单一快照把它压成一次。
- **D8 托盘两个勾选项是死的（阶段 1 实施时发现）**：`setting-start-in-tray` / `setting-hide-from-dock-taskbar` 两项的 id 不带目标状态（`lib.rs` 旧 1315/1324 行），而分发只认带 `:true`/`:false` 的 id（旧 1378-1383 行）→ 在托盘里点这两项只改勾选外观、不保存任何东西，下次重建菜单就弹回去。id 生成与解析现在都由 `toggle_menu_id`/`MenuAction::from_id` 负责，并有单测锁住旧写法必须判为无法识别。

### 2.2 需要现场证据才能定论的怀疑点（不猜，取证）

只列怀疑 + 取证手段，**不当作结论**：

- **S1** 扫描期间重建菜单，导致正在打开的托盘菜单消失/闪断（Windows 上常见「要再点一次才行」）。
- **S2** 主窗口改设置触发的菜单重建，与用户正在操作菜单撞车。
- **S3** 托盘左键单击（`show_menu_on_left_click(false)` + `TrayIconEvent::Click`，`lib.rs:2106-2120`）：点一次显示窗口、再点一次是否仍能显示（窗口已在前台时的行为）。
- **S4** 隐藏到托盘后（`on_window_event`，`lib.rs:2054`）再「显示主窗口」，标题栏/焦点/激活策略是否有异常。
- **S5** 你在系统设置里改过登录项后，界面哪一边先错、错多久。

**取证方式（阶段 0）**：加一个只写不读的 trace 日志（`<应用配置目录>/trace.log`，随进程追加），记录：每次菜单重建（含内容指纹与触发者）、每次 `emit` 的事件名+revision、每次菜单动作分发（**原始 id → 解析结果 → 结果**）、每次设置读/写（含失败与回滚）、每次扫描耗时与失败原因；**不改任何现有行为**。你复现一次现象，把日志给我，我按证据定 S1–S5，再修。日志开关：设置里 `debugTrace`（默认关）或环境变量 `REFIND_SWITCHER_TRACE=1`。

### 2.3 结构性修法（阶段 1，已定稿）

1. **单一真相**：新增 `ui_state(app) -> UiState`（含 `revision`、`status`、`settings`(含 autostart 真实状态)、`http`、`busy`(正在进行的动作)）。`rebuild_tray_menu` 与「给前端的事件」都用它；托盘不再直接读 `stored_settings`（修 D1）。
2. **完整快照 + 版本号**：一个事件 `ui-state-changed` 携带完整快照 + 单调递增 `revision`，前端只在 revision 变大时整页重绘（修 D4）；保留 `restart-result` 语义并入快照（用 `lastActionResult` 字段表达）。
3. **启动即完整菜单**：`setup` 就构建含系统项、设置项、电源项、远程控制项的全量菜单（内容来自 `ui_state`），后台扫描只做「更新内容」而不是「从残缺变完整」（修 D2）。
4. **菜单只在内容变化时重建**：对菜单内容算指纹（`fingerprint`），指纹不变就不调 `tray.set_menu`，减少 S1/S2 的撞车概率（修 S1/S2 的诱因）。
5. **动作状态共享**：`busy` 进入快照（进行中的电源动作/切换/扫描），托盘对应项 `enabled=false`、主窗口对应按钮 `disabled`，两边同步（修 D3）。
6. **设置应用失败回滚**：apply 失败则把设置回滚为旧值并广播，界面显示真实状态（修 D5）。
7. **菜单动作解析改纯函数**：`enum MenuAction` + `MenuAction::from_id(&str) -> Option<MenuAction>` + 单测（每个 id 往返、非法 id 返回 None）（修 D6）。
8. **前端契约单测更新**：`lib.rs:2321` 的断言同步到新事件名/命令名。

### 2.4 交付这一阶段时我会证明什么（不是口头说的）

- `cargo test` 全绿（含新增的解析往返、指纹、回滚、快照字段单测）；
- trace 日志里能读到「菜单重建只发生在内容变化时」「动作进行中两边都置灰」「apply 失败后设置回到旧值」三条证据；
- 你按 §8 的复现清单跑一遍，症状消失（或没消失时日志能定位）。

### 2.5 阶段 1 实施与验证记录（2026-10-09）

已落地：`UiState` 单一快照 + `publish_ui` 唯一发布入口（含 `revision`）、`ui-state-changed` 单一事件、前端按 revision 丢弃过期快照、
`build_tray_menu` 启动第一帧即全量（系统项用占位行禁用）、菜单内容指纹 + 预热（内容不变不重建）、
`BusyState` 双 UI 同步置灰、设置 apply 失败回滚（托盘与主窗口两条路径）、`MenuAction::from_id` 纯函数解析、`ActionResult` 取代 `restart-result`。

验证（本机真实执行）：`cargo test` **9 passed / 0 failed**、`cargo build` **0 warning**、`cargo fmt --check` 通过、
`node --check ui/app.js` 通过；trace 实测启动两次扫描只触发 **1 次真实菜单重建 + 3 次跳过**（阶段 0 为 2 次全量重建），
快照 `revision` 单调递增且 `busy` 随扫描进出。

未覆盖：托盘菜单的**真实点击**（左键单击、两个勾选项、动作中置灰）——无头环境点不了托盘，需你点一次并读 trace；阶段 2 的 dry-run 会把「电源动作置灰」变成可重复的自动验证。

---

## 3. 电源动作：reboot / shutdown / sleep

### 3.1 抽象（已定稿）

把 `run_restart`/`restart_attempts` 泛化为：

```rust
enum PowerAction { Reboot, Shutdown, Sleep }   // 解析："reboot"|"shutdown"|"sleep"，未知/缺省 → Reboot
struct Attempt { program: &'static str, args: Vec<&'static str>, needs_gui_auth: bool }
fn power_attempts(action: PowerAction) -> Vec<Attempt>
fn run_power_action(action: PowerAction, allow_gui_auth: bool) -> Result<(), String>
```

沿用现有安全约定（README §重启系统 已承诺）：**只有「命令不存在」才换下一条候选**；「命令能跑但失败」时若还有提权方案就走提权、否则停手并回报原因——绝不「取消授权后换个方式照样把机器关掉」。

`needs_gui_auth=true` 的候选（macOS 的管理员授权框、Linux 的 pkexec）在**远程触发**时默认整条跳过（见 §10 Q3、§4.5）。

### 3.2 三平台命令表（已定稿）

| 动作 | macOS | Linux | Windows |
| --- | --- | --- | --- |
| reboot（默认） | `osascript -e 'do shell script "/sbin/shutdown -r now" with administrator privileges'`〔GUI 授权〕 | `systemctl reboot` → `loginctl reboot`；可提权：`pkexec systemctl reboot`〔GUI 授权〕 | `shutdown /r /t 0` |
| shutdown | `osascript -e 'do shell script "/sbin/shutdown -h now" with administrator privileges'`〔GUI 授权〕 | `systemctl poweroff` → `loginctl poweroff`；可提权同上〔GUI 授权〕 | `shutdown /s /t 0` |
| sleep | ①`osascript -e 'tell application "System Events" to sleep'`（免 root，需「自动化」授权，**远程不弹**）②`sudo -n /usr/bin/pmset sleepnow`（配了免密 sudo 时可用）③`osascript ... "/usr/bin/pmset sleepnow" with administrator privileges`〔GUI 授权〕 | `systemctl suspend` → `loginctl suspend`；可提权 `pkexec`〔GUI 授权〕 | `rundll32.exe powrprof.dll,SetSuspendState 0,1,0`（★ 休眠开启时会变成休眠，见 §3.4） |

要点/坑：

- **Windows 睡眠**：`SetSuspendState` 在系统**启用了休眠**时会变成「休眠」而不是「睡眠」——平台行为，代码绕不开。你要的是**睡眠**，所以这块单独走 §3.4（只读预检 + 三选一，待你定 Q10′）。
- **Windows 关机/重启**：普通用户默认有 `SeShutdownPrivilege`，不需要提权；但 `shutdown` 与其它 GUI 程序一样需要能访问会话（服务会话下不可用）——远程场景由 ssh 进去执行本应用属于用户会话，正常。
- **macOS sleep 的 ①** 需要用户的「自动化」权限（首次会弹一次同意框，和密码框不同），且 ad-hoc 签名每次更新会重新弹——README 写清。
- **Windows/macOS 写 ESP 的前置条件**（远程切换时同样适用）：Windows 上 ESP 必须已经被挂载（`mountvol` 需要管理员），macOS 上 ESP 未挂载要先挂载；Linux 上未挂载时应用会用 `pkexec` 提权临时挂载〔GUI 授权〕——这些在远程场景下的处理见 §4.5。

### 3.3 dry-run 探针（已定稿，验证用）

环境变量 `REFIND_SWITCHER_POWER_DRY_RUN=1`：**只打印将执行的命令行，不真的执行**（`run_power_action` 直接返回 Ok 并把命令清单回传）；HTTP 响应里带上 `dryRun:true` 与 `commands:[...]`。这样本机验证不会真的关掉你的机器。

---

### 3.4 Windows 睡眠：接受「可能变休眠」，但必须说清楚（Q10′ 已定：方案 C）

事实（平台行为，不是实现取舍）：Windows 上 `rundll32 powrprof.dll,SetSuspendState` 和 PowerShell/Add-Type 调的是同一个 `SetSuspendState`，**系统启用休眠时会休眠而不是睡眠**，官方没有「强制睡眠」的公开参数。你选的 **C**：不做特殊处理（点「睡眠」就是这条命令），但应用必须把「这次是真睡眠还是会变休眠」讲清楚，不加 `windowsForceRealSleep` 那种会动系统的开关。

**只读预检（每次执行睡眠前跑，不写入任何东西）**：

1. 首选读注册表 `HKLM\SYSTEM\CurrentControlSet\Control\Power` 的 `HibernateEnabled`（`REG_DWORD`，语言无关；`winreg` 0.55.0 已在 `Cargo.lock` 里 → 只加 Windows 目标的直接依赖，离线可构建）：`0` → 「本次会进入睡眠（S3）」；`1` → 「本次会把机器休眠（不是睡眠）」。
2. 读不到注册表 → 跑 `powercfg /a`，把**原文整段**写进 trace（注意本机是 GBK 输出），再用关键词给出三态判定；仍不确定 → 结论写「未判定（见 trace 里的 `powercfg /a` 原文）」，**不猜**。

这句话同时出现在三处：trace 日志、主窗口状态 / HTTP 响应里的 `message`、通知弹窗（这次走弹窗时）。

**本机实测（2026-10-09，两个只读探针）**：`HibernateEnabled = 0x0`；`powercfg /a` →「待机 (S3) 可用 / 休眠：尚未启用休眠 / 混合睡眠与快速启动因休眠不可用而不可用」。结论：**这台机器上「睡眠」会真睡眠（S3），不会变休眠**，所以 C 在你机器上没有副作用；换机器时预检会如实告诉使用者。

## 4. HTTP 服务

### 4.1 配置（已定稿，默认值**必须一眼可见**：首次运行就把它们写进 `settings.json`）

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `httpEnabled` | `true` | 你要的「默认开启」 |
| `httpBind` | `"0.0.0.0"` | 见 Q1；填 `127.0.0.1` 则只允许本机/ssh 隧道 |
| `httpPort` | `12300` | |
| `httpToken` | 首次运行自动生成的 32 位十六进制 | 必填；为空则服务拒绝启动（不允许无鉴权公网监听） |
| `remoteAllowGuiAuth` | `false` | 远程/CLI 请求是否允许触发本机授权弹窗（**Q3 已定：false，不弹**） |
| `notifyEnabled` | `true` | 弹窗通知**总开关**：关掉后任何渠道都不弹窗（延迟照旧，见 §4.7 / Q11） |
| `channels.http.notify` | `true` | HTTP 渠道（webhook / curl / 面板）是否弹窗（Q14） |
| `channels.http.delaySeconds` | `5` | HTTP 渠道收到请求后延迟多少秒执行，`0`=立即 |
| `channels.cli.notify` | `true` | ssh / CLI 渠道是否弹窗（Q14） |
| `channels.cli.delaySeconds` | `5` | ssh / CLI 渠道延迟秒数，`0`=立即 |
| `debugTrace` | `false` | 阶段 0 的 trace 日志开关 |

环境变量覆盖（**调试/沙箱用**，优先级高于设置文件）：`REFIND_SWITCHER_HTTP_ENABLED`、`REFIND_SWITCHER_HTTP_BIND`、`REFIND_SWITCHER_HTTP_PORT`、`REFIND_SWITCHER_POWER_DRY_RUN`、`REFIND_SWITCHER_TRACE`、`REFIND_SWITCHER_VARS_DIR`、`REFIND_SWITCHER_CONFIG_DIR`。

其中 `REFIND_SWITCHER_CONFIG_DIR` 是实施时补上的（阶段 7 验证时发现必需）：Tauri 的配置目录走系统 Known Folder API，**`APPDATA` 环境变量改不动它**，第一次沙箱验证就这样误写了真实 `settings.json`；现在 GUI 与 CLI 都用这个变量定位配置目录，验证/多实例可以完全隔离。

### 4.1.1 实施中补定的细节（都已落地 + 有测试）

- **413 而不是静默空 body**：请求体超过 `MAX_REQUEST_BODY`（8192）时明确回 `413 body_too_large`；否则 body 被丢掉会变成看不懂的 401。
- **HTTP 层不持有 token**：token 比较只在 `handle_remote_request` 一处（`http.rs` 不接收 token，避免出现第二处鉴权逻辑）。
- **CLI 端口优先级**：`--port` > `REFIND_SWITCHER_HTTP_PORT` > 设置文件；CLI 与 GUI 因此能在同一沙箱里互相找到。
- **CLI 本地分支也支持 `--json`**，字段名与 HTTP 响应一致（`ok/pending/channel/message/varsDir/variables`，dry-run 时多一个 `dryRunCommands`）。
- **通知弹窗关闭语义**：只有槽位里确实还有待执行请求时才记「本机用户取消」；执行完成/到点自动关窗不算用户操作（不写这条日志）。
- **端口被占用**：报错留在界面上、**不换端口**；已实测（第二个实例报 `os error 10048`，第一个实例继续服务）。

### 4.2 技术选型（定稿）

**在应用进程内起一个后台线程的 HTTP 服务**（共享 `AppHandle`，直接调用与 GUI 完全相同的核心函数），用 **`tiny_http 0.12`**（纯线程、无 async 运行时、依赖极小；已实测本机能解析到该 crate）。备选：手写 `std::net::TcpListener`（零依赖，但要自己处理报文边界/超时，安全面更大）——不推荐但可行。

服务生命周期：设置变更/开关切换时 **stop → rebind → start**（旧监听用「置停止标志 + 自连接唤醒 accept + join」确定性地收掉）；端口被占用 → 报错并在主窗口提示，**不自动换端口**（避免远程地址漂移）。多实例：第二个实例绑定失败即提示，不影响主实例。

### 4.3 端点（已定稿）

统一：`GET` 与 `POST` 都接受；参数可从 query、`x-www-form-urlencoded` 或 JSON body 取；所有响应 `application/json; charset=utf-8`。

| 端点 | 参数 | 作用 |
| --- | --- | --- |
| `GET /health` | — | **无需 token**，`{"ok":true}`（探活用，不泄露任何信息） |
| `GET|POST /api/status` | — | 当前状态快照（变量目录、三个系统及其 active、message、error、busy） |
| `GET|POST /api/switch` | `system=linux\|windows\|macos` | 只切换下次启动系统（写 `PreviousBoot`，原子替换 + SHA-256 校验，与 GUI 同一条路径） |
| `GET|POST /api/power` | `action=reboot\|shutdown\|sleep`（缺省 `reboot`） | 只执行电源动作 |
| `GET|POST /api/switch-and-power` | `system=...` + `action=...`（缺省 `reboot`） | **「远程切换系统」的主用法**：先切换，成功后立刻执行电源动作 |
| `GET|POST /api/requests` | `limit`（默认 20） | 最近请求及其状态（`pending`/`executed`/`cancelled`/`failed`），供远程侧确认「到底执行了没有」 |

响应示例：

```json
{"ok":true,"revision":42,"message":"已提交重启，系统即将重启","data":{"action":"reboot","system":"windows"}}
{"ok":false,"error":"invalid_token","message":"token 无效"}
{"ok":false,"error":"action_in_progress","message":"已有动作在执行中（reboot），请稍后重试"}
{"ok":false,"error":"needs_local_authorization","message":"Linux 上挂载 ESP 需要本机授权，远程已跳过；请配置免密 sudo 或预先挂载（见 README）"}
{"ok":true,"pending":true,"requestId":"7f3c9a21","executeInSeconds":5,"message":"已收到远程请求，本机确认后 5 秒执行"}
```

HTTP 状态码：`200` 成功；`400` 参数非法；`401` token 缺失/错误；`404` 未知路径；`405` 方法不支持；`409` 已有动作在执行；`500` 动作失败（body 带原因，与 GUI 显示的原因同源）。

### 4.4 鉴权与安全（已定稿）

- token 顺序：请求头 `X-Auth-Token` → query `token=` → body `token`；**常量时间比较**；失败一律 `401` 并计数。
- 不做 HTTPS（见 Q5）：**明文 HTTP 只用于局域网或 SSH 隧道**，README 三端并列写清风险与 `ssh -L 12300:127.0.0.1:12300` 用法。
- 不做 IP 白名单/限速（见 Q9），但记录**最近请求**（内存保留 20 条：时间/来源 IP/方法/路径/动作/结果），主窗口「远程控制」区可看——这就是排查「谁在打我的端口」的现场证据。
- **不改系统防火墙**（需要管理员，且属于用户环境）：README 给出各平台放行/查询命令，由你手动执行。
- 每次请求写一行日志（stderr + trace 文件），不记录 token 明文（只记 token 指纹前 8 位）。

### 4.5 远程触发时的授权策略（已定稿）

远程请求**默认不允许**触发需要本机 GUI 交互的授权框（macOS 管理员框 / Linux pkexec），因为那会挂住 HTTP 请求且需要有人坐在机器前点确认。命中这种候选时：整条跳过，直接返回 `needs_local_authorization` 并附上可照做的建议（README 里给出具体配置：Linux 的 `sudoers` 免密行、把 ESP 写进 `/etc/fstab` 预先挂载、macOS 的 `sudo -n` 免密；Windows 预先 `mountvol` 挂载 ESP）。`remoteAllowGuiAuth=true` 时行为回到「和本机点击一样」（Q3）。

### 4.6 主窗口「远程控制」区（已定稿）

- 服务开关、监听地址、端口、**token 显示/复制/重新生成**、服务状态（正在监听 `0.0.0.0:12300` / 失败原因）、最近请求列表；
- **一键复制可直接用的命令**（带本机 IP 与 token 的 `curl`，见 §5），这是「远程能立刻用起来」的关键；
- 托盘菜单不放 token（截图/旁人视线风险）：托盘只放「HTTP 服务」开关与「远程控制设置…」（打开主窗口并定位到该区）。
- token 显示默认打码，点「显示」才明文。

---

### 4.7 通知弹窗 + 延迟执行（只针对远程请求）

这是「远程命令不能悄无声息地把机器关掉」的兜底：远程请求先变成**待执行请求**，弹窗 + 倒计时，本机可取消。

流程（已定稿，默认值见 Q11）：

1. 通过鉴权的**会改状态**的请求（`/api/switch`、`/api/power`、`/api/switch-and-power`）不立即执行，先生成一条**待执行请求**（`requestId`），并弹窗 + 在主窗口显示。
2. 弹窗显示：来源 IP、动作摘要（例：「切换下次启动为 Windows，5 秒后重启」）、倒计时进度、「立即执行」「取消」两个按钮。**关掉窗口（X / Esc）＝取消**（安全默认）。
3. 倒计时到点或点「立即执行」→ 走与 GUI 完全相同的核心逻辑；点「取消」→ 记录 `cancelled`，日志与最近请求列表写明「本机用户取消」，**不执行**。
4. HTTP 立刻回 `202`：`{"ok":true,"pending":true,"requestId":"...","executeInSeconds":5}`；`GET /api/requests` 可查最近请求状态（`pending`/`executed`/`cancelled`/`failed`）——远程侧不用猜结果。
5. **渠道生效规则**（`channels.<渠道>.delaySeconds` / `.notify`，Q14 已定：http 与 cli 各自可配）：
   - `delaySeconds=0` → 不弹倒计时窗口，执行后弹一个 **3 秒自动关闭的事后提示**（「远程请求已执行：…（来源 IP）」）。
   - `notify=false` 且有延迟 → 该渠道不弹窗，取消入口只剩主窗口「远程控制」区的**待执行请求卡片**（倒计时 + 立即执行 + 取消）。
   - `notify=false` 且 `delaySeconds=0` → 静默立即执行，只记日志。
   - **总开关 `notifyEnabled=false`** → 任何渠道都不弹窗（延迟照旧，取消只能从主窗口卡片）。
6. **渠道识别**：CLI 转发时带 `channel=cli`，该参数**只在请求来源为 127.0.0.1（loopback）时生效**；远端请求即使伪造 `channel=cli` 也一律按 `channels.http` 执行。
7. 处于 pending 期间，任何新的会改状态请求 → `409 action_in_progress`（pending 也算忙）。
8. 只有**鉴权通过**的请求才弹窗；`/health`、`/api/status` 不弹（避免被刷屏）；同一来源 3 秒内的重复请求合并成一次提示。
9. 弹窗窗口：label `notify`，无边框、置顶、不可缩放、约 380×200、屏幕右下角、`skipTaskbar`，倒计时结束自动关闭；不抢主窗口焦点，也不受「启动时隐藏主界面/隐藏任务栏」影响（它必须能被看见）。
10. 本地（托盘 / 主窗口）触发的动作**不弹**这个窗，走原有回执（见 Q13）。
11. 实现：一个待执行槽位 + 每秒 tick 的调度线程（不引入 async 运行时）；状态机 `pending → executed | cancelled | failed` 只转移一次，取消优先于到点。

**弹窗与配置的落点**：主窗口「远程控制」区放 **通知总开关** + **两条渠道各自的通知开关 / 延迟秒数**（HTTP webhook、SSH·CLI，各 0–600 秒）+ 待执行请求卡片 + 最近请求列表（含「渠道」与「状态」两列）；托盘「应用设置」子菜单放 **远程请求通知（总开关）** + **远程执行延迟**（预设 0/3/5/10/30 秒，**同时作用于两个渠道**，细分到渠道的配置去主窗口改）。两处共享同一份快照，双向一致。

安全：弹窗只显示来源 IP 与动作，**不显示 token**；未通过鉴权的请求不弹窗、只计失败次数；远程请求**不能**用参数覆盖 `notify`/`delay`（带了也一律忽略），否则「本机可取消」这层保护会被远程自己关掉。

## 5. 远程调用的两条路：HTTP（curl / webhook）与 CLI（ssh）

### 5.1 方式 A：HTTP（curl / webhook，零新增依赖）

ssh 上去或走隧道，然后 curl（`httpBind=0.0.0.0` 时也可从任意可达地址直连，不必隧道）：

```bash
# 1) 直接 ssh 到目标机执行
ssh user@192.168.1.50 'curl -s -X POST "http://127.0.0.1:12300/api/switch-and-power?system=windows&action=reboot" -H "X-Auth-Token: <token>"'

# 2) 本机开隧道，用同样的 URL（httpBind=0.0.0.0 时也可直连，不必隧道）
ssh -N -L 12300:127.0.0.1:12300 user@192.168.1.50 &
curl -s -X POST "http://127.0.0.1:12300/api/power?action=sleep" -H "X-Auth-Token: <token>"
```

### 5.2 方式 B：CLI 子命令（定稿——按 Q2「要参数、ssh 更方便」）

同一个二进制，无参数照旧启动 GUI；带参数走**无 GUI 的 CLI 分支**（不进 `tauri::Builder`）：

```bash
# ssh 上去一条命令搞定（Windows / Linux / macOS 同一个二进制）
ssh user@192.168.1.50 'refind-switcher --switch windows --action reboot'
ssh user@192.168.1.50 '/Applications/rEFInd\ Switcher.app/Contents/MacOS/refind-switcher --status --json'
ssh user@192.168.1.50 'rEFInd\ Switcher.exe --action sleep'
```

| 参数 | 含义 |
| --- | --- |
| `--status [--json]` | 看当前状态（变量目录、三个系统是否 active、服务是否在监听） |
| `--switch <linux\|windows\|macos>` | 只切换下次启动系统（不带 `--action`） |
| `--switch <系统> --action <reboot\|shutdown\|sleep>` | **先切换，成功后立刻执行电源动作**（等价 HTTP 的 `/api/switch-and-power`） |
| `--action <reboot\|shutdown\|sleep>` | 只做电源动作；缺省 = `reboot`；非法值 → 退出码 2 |
| `--port <n>` | 覆盖探测端口（调试/多实例用） |
| `--json` | 输出与 HTTP 响应同结构的 JSON，方便脚本解析 |
| `--help` / `--version` | 用法 / 版本 |

**退出码**：`0` 成功；`1` 动作失败；`2` 参数错误；`3` 需要本机授权被跳过（按 Q3 不弹框，附 README 里的免密/预挂载方案）；`4` 变量目录未就绪。输出同时写进 trace 日志（stdout 拿不到也不丢证据）。

### 5.3 执行路径：优先转发给运行中的实例，不可达才降级自己干（定稿）

1. 读设置里的 `httpPort`/`httpToken`，探测 `http://127.0.0.1:<port>/health`：
   - **可达** → 把这次调用**转发给正在运行的实例**（自动带 token + `channel=cli`）→ 行为与 webhook 一致（同样有通知弹窗、延迟执行、最近请求记录），只是按 `channels.cli` 的配置生效。好处：ssh 远程操作也能被本机看见、否决。
   - **不可达**（应用没跑 / 服务关了）→ **降级为 CLI 自己执行**：复用同一批核心函数（扫描、原子写入、电源命令表），**不是第二份实现**；没有 UI 进程，因此无弹窗、相当于「立即执行」。
2. 降级且需要 GUI 授权时（Linux pkexec、macOS 管理员框）：按 Q3 **不弹**，退出码 3 + 可照做的指引。

**渠道与 Q14（已定）**：ssh/CLI 调用**也走**通知弹窗 + 延迟（ssh 也是远程操作，本机该知道、也该能否决），但**按渠道分开配置**（Q14）：转发带 `channel=cli`（仅 loopback 生效，见 §4.7），实例按 `channels.cli.notify` / `channels.cli.delaySeconds` 执行，webhook 走 `channels.http`。于是可以做到「ssh 允许立即执行、webhook 一律 5 秒可取消」这种组合。

### 5.4 Windows 上的 stdout（定稿）

release 版是 GUI 子系统（`src-tauri/src/main.rs:1` 的 `windows_subsystem = "windows"`），ssh 会话里默认**没有** stdout。做法：CLI 分支开头调 `AttachConsole(ATTACH_PARENT_PROCESS)`（自己 `extern` 声明，不新增依赖），把输出接到 ssh 终端；取不到控制台时结果仍写 trace 日志。macOS/Linux 无此问题。

---

## 6. UI 改动清单

**托盘右键菜单**（全量，按顺序）：

1. 状态行（不可点，`下次启动：Windows`）
2. 三个系统项（active 打 ●）
3. `电源操作` 子菜单：重启系统（默认）/ 关机 / 睡眠 —— 进行中置灰
4. `远程控制` 子菜单：HTTP 服务（勾选开关）/ 远程控制设置…
5. 卸载临时挂载（仅存在时）
6. 显示主窗口
7. 应用设置：开机启动 / 启动时隐藏主界面 / 隐藏程序（任务栏·Dock）/ 点击 ❌ 时 / 远程请求通知（总开关勾选）/ 远程执行延迟（子菜单 0·3·5·10·30 秒，同时作用于 http 与 cli 两条渠道）
8. 扫描 / 退出

**主窗口**：

- 顶部状态区不变；
- 系统卡片区不变（点击切换，进行中禁用）；
- 电源动作区：三个按钮「重启系统 / 关机 / 睡眠」（见 Q4），进行中全部禁用并显示原因；
- 「应用设置」不变 + 新增「远程控制」区（§4.6）；
- 错误显示统一：动作失败的原因与 HTTP 响应里的 `message` 同源。

**通知弹窗（独立窗口，label `notify`）**：

- 新增 `ui/notify.html` + `ui/notify.js`：无边框、置顶、约 380×200、右下角、`skipTaskbar`，显示 来源 IP / 动作摘要 / 倒计时 / 「立即执行」「取消」；关闭窗口＝取消；倒计时结束自动关闭。
- 主窗口「远程控制」区新增：**通知总开关**、**两条渠道各一行**（HTTP webhook 与 SSH·CLI，各有 通知开关 + 延迟秒数 0–600）、`待执行请求` 卡片（倒计时 + 立即执行 + 取消）、最近请求列表加「渠道」「状态」两列。
- 主窗口电源动作区：本机点击**不**走倒计时（见 Q13）。

---

## 7. 配置兼容

- `Settings` 新增字段全部 `#[serde(default)]`：老 `settings.json` 缺字段 → 取默认（HTTP 默认仍为开启，token 缺失则首次启动生成并写回）。
- 首次运行/升级后**主动写回完整配置**（含 token），保证「默认值一眼可见」。
- 不迁移、不删除任何现有字段；新增的 `notifyEnabled` 与嵌套的 `channels.{http,cli}.{notify,delaySeconds}` 全部 `#[serde(default)]`（嵌套结构也带默认），老配置升级后即为「总开关开 + 两条渠道都弹窗、都延迟 5 秒」。

---

## 8. 测试与验收（本机验证不碰你正在跑的实例）

沙箱约定：验证时始终带环境变量起 dev/独立构建，端口避开 12300、电源动作走 dry-run：

```bash
# Windows（本机，git-bash）
REFIND_SWITCHER_HTTP_PORT=12399 REFIND_SWITCHER_POWER_DRY_RUN=1 REFIND_SWITCHER_TRACE=1 npm run tauri dev
```

固定检查项（每阶段都要过）：

```bash
cd src-tauri && cargo fmt --check && cargo test && cargo check
node --check ui/app.js
git diff --check
```

HTTP 手测清单（阶段 3 验收，全部走 12399 + dry-run）：

```bash
curl -s http://127.0.0.1:12399/health                                   # {"ok":true}
curl -s -i http://127.0.0.1:12399/api/status                            # 401
curl -s -X POST http://127.0.0.1:12399/api/power -H "X-Auth-Token: $T"  # 缺 action → reboot 且 dryRun
curl -s "http://127.0.0.1:12399/api/power?action=sleep&token=$T"        # shutdown/sleep 各一次
curl -s -i "http://127.0.0.1:12399/api/nope?token=$T"                   # 404
curl -s -X PUT "http://127.0.0.1:12399/api/status?token=$T"             # 405
curl -s -X POST "http://127.0.0.1:12399/api/power?action=reboot&token=$T"  # 202 + pending + executeInSeconds
curl -s "http://127.0.0.1:12399/api/requests?token=$T"                  # pending → executed / cancelled
```

新增 Rust 单测（阶段 1–3）：

- `MenuAction::from_id` 往返 + 非法 id → `None`；
- `PowerAction::parse`（含缺省 → `Reboot`、`SHUTDOWN` 大小写、非法值）；
- 远程模式过滤 `needs_gui_auth` 候选：命令表只留可远程执行项；
- token 比较（正确/错误/空/超长）+ 401 判定；
- 路由解析（path+method+query → 动作，非法参数 → 400）；
- `Settings` 序列化含新字段 + 老 JSON（缺字段）反序列化为默认；
- 菜单指纹：内容不变 → 不重建；
- 设置 apply 失败 → 回滚为旧值；
- dry-run 命令渲染（Windows/macOS/Linux 三条），不真的执行；
- `channels.*.delaySeconds` 夹取（0–600；负数/超大/非数字 → 默认 5）+ `notifyEnabled` 总开关（关掉后任何渠道都不弹窗、延迟照旧）；
- **渠道识别与防伪造**：`channel=cli` + loopback → 用 `channels.cli`；`channel=cli` + 非 loopback → 一律 `channels.http`；
- 远程请求自带 `notify=0` / `delay=0` 参数 → 被忽略（只用本机配置）；
- Windows 睡眠预检三态映射（注册表 0 / 1 / 读不到但 `powercfg /a` 原文可判 / 原文不确定 → 未判定）；
- 待执行请求状态机：`pending → executed | cancelled | failed`，只转移一次，取消优先；pending 期间新请求 → 409；
- 本机（托盘/主窗口）触发的动作不进入待执行、不弹窗。

---

## 9. 阶段划分

| 阶段 | 内容 | 完成判据 |
| --- | --- | --- |
| 0 | trace 日志探针（不改行为） | 你复现一次，日志里能区分 S1–S5 |
| 1 | 联动结构修复（§2.3 的 1–8） | `cargo test` 全绿 + 你的复现清单症状消失 |
| 2 | 电源动作抽象 + 三端命令表 + dry-run + 双 UI 入口 | 单测全绿 + dry-run 命令与预期逐字一致 |
| 3 | HTTP 服务（配置/生命周期/端点/token/界面/日志） | §8 的 curl 清单全部符合预期状态码与 body |
| 4 | 通知弹窗 + 延迟执行（§4.7） | 弹窗倒计时/取消/到点执行三条路径手测通过 + 状态机单测全绿 |
| 5 | CLI 子命令（§5.2–5.4：参数、转发/降级、退出码、Windows console） | §5.2 的 ssh 命令逐条在真实 ssh 会话里跑通 |
| 6 | README 全面更新（三端并列 + 多种使用方式对照表 + 客户端/面板点名 + 防火墙/隧道/授权配置 + 通知与延迟说明 + ssh 两种用法） | 照 README 从上到下能跑通（另一台设备实测一次） |
| 7 | 三端打包回归 | `npm run build:win`；Linux/macOS 交给对应机器构建 |

---

## 10. 决策表（Q1–Q14 全部已定）

已定（你的回答，见 §0.1 决策表）：Q1 `0.0.0.0`；Q2 两条路都要（HTTP + CLI）；Q3 不弹授权框；Q4 三个按钮；Q11 通知开 + 延迟 5 秒（弹窗可取消、可配 0 = 立即，都在主窗口设置里）；Q12 自绘窗口；Q13 本机动作不倒计时。Q5–Q9 你没反对，按建议执行。

补答（都在 §0.1 里）：

- **Q10′**：选 **C** —— Windows 不做特殊处理（不加 `windowsForceRealSleep`），但每次睡眠前做只读预检（注册表优先）并在 trace / 界面 / HTTP 响应三处明说「会睡眠」还是「会变休眠」。你机器实测为「会真睡眠（S3）」。
- **Q14**：ssh/CLI **也走**通知 + 延迟，且**按渠道分开配置**（`channels.http` / `channels.cli`），CLI 转发用 `channel=cli`（仅 loopback 生效，远端伪造无效）；远程请求不能用参数覆盖本机配置。

**结论：Q1–Q14 全部已定，无待确认项，可从阶段 0 开工。**

## 11. 明确不做（范围外）

- 不做 HTTPS/TLS、不做 UPnP/端口映射、不改系统防火墙、不做用户体系（只有单 token）。
- 不改动 rEFInd 本身的配置，不新增对 `PreviousBoot-*` 模板文件的写操作（现有承诺不变）。
- 不重构与本需求无关的扫描/挂载逻辑。
- 不做系统原生通知（Windows Toast / macOS 通知中心，见 Q12；自绘置顶窗口已满足需求）。
- 不给本机点击的动作加倒计时（见 Q13）。
- 不做「远程请求自己关掉通知/延迟」的参数，也不做 Windows 强制真睡眠（见 Q10′、§4.7 安全）。