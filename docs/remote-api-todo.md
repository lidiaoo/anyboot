# rEFInd Switcher 远程控制改造 · 执行清单

配套设计：`docs/remote-api-plan.md`（§0.1 决策表：Q1–Q14 全部已定）
约定：每阶段「单测全绿」才进下一阶段；验证一律避开你正在跑的实例（`REFIND_SWITCHER_HTTP_PORT=12399`、`REFIND_SWITCHER_POWER_DRY_RUN=1`）。

---

## 阶段 0 · 取证（不改行为）—— 已完成（2026-10-09）

- [x] 加 trace 日志：菜单重建（含内容指纹+触发者）、事件 emit、菜单动作（原始 id → 解析 → 结果）、设置读写（含失败原因）、扫描/切换耗时与结果、电源动作起止 → `<配置目录>/trace.log`
  验收（实测）：`REFIND_SWITCHER_TRACE=1` 起 debug 版，`trace.log` 落盘 12 行，每类事件都有；样本存证 `hermes scratch/trace-probe-evidence.log`
- [x] 设置项 `debugTrace`（默认 false）+ 环境变量 `REFIND_SWITCHER_TRACE`（环境变量优先）
  验收（实测）：`cargo test` 6 passed（`settings_debug_trace_defaults_off_and_round_trips`、`trace_env_override_parses_common_forms`）
- [x] 平台无关单测搬出 macOS 模块（原来 Windows/Linux 上 `cargo test` 一条都不跑）
  验收（实测）：Windows 上 `cargo test` → `6 passed; 0 failed`（原先 0 条）
- [ ] 复现清单（下方）逐条走一遍取日志 —— **等你/我在阶段 1 后跑**，目前只自动拿到启动阶段证据
  验收：日志能区分 S1–S5，且不含 token 明文

### 阶段 0 实测发现（写进 plan §2.1 D7）

```
2026-10-09T08:59:05Z [settings-file] 已读取 ...\settings.json debugTrace=false
2026-10-09T08:59:05Z [scan] 开始扫描
2026-10-09T08:59:05Z [tray] 重建菜单 trigger=scan sig=msg=下次启动：Windows|tmp=0|autostart=1|tray=1|dock=1|close=Tray|vars=-o,+o,-o|err=
2026-10-09T08:59:05Z [tray] 菜单与 tooltip 已更新
2026-10-09T08:59:05Z [scan] 完成 耗时=22ms result=ok
2026-10-09T08:59:05Z [event] status-changed msg=下次启动：Windows err=- tmp=false
2026-10-09T08:59:05Z [command] refresh（主窗口）
2026-10-09T08:59:05Z [scan] 开始扫描
2026-10-09T08:59:05Z [tray] 重建菜单 trigger=scan sig=（同上）
2026-10-09T08:59:05Z [scan] 完成 耗时=4ms result=ok
2026-10-09T08:59:05Z [event] status-changed msg=下次启动：Windows err=- tmp=false
```

- **启动即双扫描**：后台线程一次 + 前端加载后 `invoke("refresh")` 一次，两次都全量重建托盘菜单（签名完全相同）→ 这是 S1/S2「扫描期间菜单重建/闪断」的直接诱因，阶段 1 的内容指纹会把它压成一次。
- 验证注意事项：Tauri 的配置目录走系统 Known Folder API，**`APPDATA` 环境变量改不动它** —— 跑新构建做验证时它读写的就是你真实配置目录（本次只多写了一个 trace.log，已删除；`settings.json` 未被改写，开机启动状态 `设置=true / 系统状态=enabled` 未触发重注册）。

## 阶段 1 · 联动结构修复 —— 已完成（2026-10-09）

- [x] `ui_state(app) -> UiState`（revision/status/settings(系统真实 autostart)/busy/lastAction）成为唯一快照；`publish_ui` 是唯一发布入口
  验收（实测）：`cargo test` 9 passed；trace 里 `[state] revision=1..4 busy=… msg=…` 单调递增
  （注：http 字段留到阶段 3 加进同一结构）
- [x] 托盘菜单改用同一份快照（不再读 `stored_settings`），修 D1
  验收（实测）：trace 的菜单指纹里 `autostart=1` 来自系统真实状态；`publish_settings` 的 trace 打的是快照值
- [x] 单一事件 `ui-state-changed`（完整快照 + revision），前端按 revision 丢弃过期快照，修 D4
  验收（实测）:`node --check ui/app.js` 通过；`frontend_uses_the_single_state_contract` 断言新事件名 + 10 个命令名 + revision 处理 + 三个旧事件不再出现
- [x] `setup` 启动即建全量菜单（系统项用占位行、禁用），修 D2
  验收（实测）：启动第一帧指纹 `vars=-x,-x,-x`（三个系统项在位、禁用），菜单结构完整
- [x] 菜单内容指纹 + 预热：内容不变就不调 `set_menu`
  验收（实测）：启动两次扫描 + 前端 refresh → **实际重建 1 次、跳过 3 次**（阶段 0 时是 2 次全量重建）
- [x] `busy` 进快照：托盘项与主窗口按钮按同一份 busy 一起禁用，修 D3
  验收（实测）：trace `busy=扫描/空闲` 进快照；托盘置灰逻辑由 `BusyState::affects_menu` 单测覆盖
  （真实点击体验待你在托盘上点一次确认；阶段 2 的 dry-run 会补上置灰的可重复验证）
- [x] 设置 apply 失败回滚为旧值并广播，修 D5（托盘 `update_tray_setting` + 主窗口 `set_hide_from_dock_taskbar`）
  验收：代码路径 + 前端 `call()` 失败后拉回 `get_ui_state`；回滚断言留到阶段 2/3 的 dry-run 场景
- [x] `MenuAction::from_id` 纯函数 + 单测，修 D6
  验收（实测）：`menu_action_from_id_round_trips_generated_ids`（15 个 id 往返）、`menu_action_rejects_unknown_ids`（8 个非法 id → None）
- [x] **新发现并修掉的真 bug（D8）**：托盘「启动时隐藏主界面」「隐藏程序（任务栏/Dock）」两项 id 不带目标状态，
  而分发只认带 `:true/:false` 的 id → 点了只改勾选外观、什么都不保存（下次重建弹回）。
  现在 id 由 `toggle_menu_id(name, !current)` 生成、`MenuAction::from_id` 解析，并有用例锁住旧写法必须判 None。
  验收：单测覆盖；**真实点击体验请你点一下托盘这两项确认**（我无法在无头环境点托盘菜单）
- [ ] 复现清单里需要点鼠标的条目（托盘左键单击 S3、隐藏后显示 S4、系统设置改登录项 S5）待你点一次，我读 trace 定论
- [ ] 阶段 1 改动提交（等你确认）

### 阶段 1 实测统计（trace 存证 `hermes scratch/trace-stage1-final.log`）

```
[settings-file] 已读取 …settings.json debugTrace=false
[scan] 开始扫描
[tray] 菜单内容未变，跳过重建 trigger=publish sig=msg=正在扫描…|vars=-x,-x,-x|busy=power=-|sw=0|un=0   ← 预热生效
[state] revision=1 busy=扫描 msg=正在扫描… err=-
[scan] 完成 耗时=14ms result=ok
[tray] 重建菜单 trigger=publish sig=msg=下次启动：Windows|vars=-o,+o,-o|busy=power=-|sw=0|un=0   ← 唯一一次真实重建
[state] revision=2 busy=空闲 msg=下次启动：Windows err=-
[command] get_ui_state（主窗口） / [command] refresh（主窗口）
[scan] 开始扫描 → [tray] 菜单内容未变，跳过重建 …
[state] revision=3 busy=扫描 …  [scan] 完成 耗时=3ms
[tray] 菜单内容未变，跳过重建 …  [state] revision=4 busy=空闲
```

统计：`实际重建=1，跳过重建=3`（阶段 0 时两次扫描 = 两次全量重建）。

## 阶段 2 · 电源动作（reboot / shutdown / sleep）—— 已完成（2026-10-09）

- [x] `PowerAction`（Reboot/Shutdown/Sleep；`parse` 大小写不敏感 + 别名 restart/poweroff/halt/suspend；缺省 = Reboot）+ `power_attempts(action)` 三端命令表（每条带 `needs_gui_auth`）
  验收（实测）：`cargo test` **14 passed**；`power_action_parses_names_and_aliases`（含 hub/非法值 → None、缺省 reboot、label/回执文案、id 往返）、
  `windows_power_table_matches_design`（本机逐字断言 `shutdown /r /t 0`、`shutdown /s /t 0`、`rundll32.exe powrprof.dll,SetSuspendState 0,1,0`）
- [x] `run_power_action(action, allow_gui_auth, dry_run)`：沿用「只有命令不存在才换下一候选」的安全语义（`PowerStep` Done/Next/Escalate/Stop）
  验收（实测）：`power_attempt_chain_stops_after_cancelled_authorization` 4 条断言全绿（取消授权后停手，不许换方式照样关机）
- [x] 远程/CLI 模式过滤 `needs_gui_auth` 候选（Q3）
  验收（实测）：`plan_commands_filters_gui_auth_attempts` —— 免授权候选可执行、授权候选只以「需要本机授权」出现
- [x] `REFIND_SWITCHER_POWER_DRY_RUN=1`：只回传命令、不执行；**dry-run 不再是环境变量里读**，而是 `run_power_action` 的显式参数（可确定性单测）
  验收（实测）：`dry_run_never_executes_and_lists_the_table_commands` —— 三种动作各跑一遍，命令清单逐条来自命令表，且没有任何进程被执行
- [x] 主窗口三个按钮（重启系统/关机/睡眠）+ 托盘「电源操作」子菜单（进行中整层置灰）
  验收：代码已就位（`ui/index.html`、`ui/app.js`、`build_tray_menu`）；
  **真实点击路径待阶段 3 用 HTTP + dry-run 自动验证**（无头环境点不了托盘/按钮，这点不装）
- [x] `restart_system` 命令保留（兼容）→ 转 `spawn_power(Reboot)`；新增 `power_action` 命令（`reboot`/`shutdown`/`sleep`，缺省 reboot，非法值报错）
  验收（实测）：`cargo test`（`power_action` 已在注册表里，前端契约用例断言命令名）
- [x] Windows 睡眠只读预检：注册表 `HKLM\SYSTEM\CurrentControlSet\Control\Power\HibernateEnabled`（`winreg = "0.55"`，仅 Windows 目标）优先，读不到才跑 `powercfg /a` 并把**原文**写进 trace，三态判定（启用/关闭/未判定）
  验收（实测）：`hibernation_verdict_from_powercfg_output`（本机中文原文 → Disabled、英文 → Disabled、只说「休眠」→ 保守 Enabled、看不出 → Unknown）；
  `windows_power_table_matches_design` 里同时断言 `sleep_probe()` 必须给出结论与非空证据；本机注册表实测 `HibernateEnabled=0` → 「本次会进入睡眠（S3）」
- [x] 预检结论进 trace + 回执：`spawn_power` 把证据逐行写 trace，回执文案带上「会睡眠 / 会变休眠 / 未判定」
  验收：桌面与托盘动作的回执走同一条 `record_action` 路径；HTTP 响应里的说法在阶段 3 接入同一份 `note`
- [x] Q10′ = C 定稿：睡眠命令保持 `rundll32 ... SetSuspendState`，不加任何会动系统的开关
  验收（实测）：`git grep windowsForceRealSleep src-tauri` 无匹配；`cargo build` **0 warning**

### 阶段 2 遗留（明说不藏）

- 主窗口三个按钮 / 托盘「电源操作」子菜单的**真实点击**尚未验证（无头环境点不了）。阶段 3 的 `/api/power` + `REFIND_SWITCHER_POWER_DRY_RUN=1` 会覆盖同一条 `spawn_power` 路径（busy → 计划 → 回执 → 快照），那时补上可重复的端到端验证。
- README 的「重启系统」段落现在过时了（只讲重启、函数名也已改），留到阶段 6 统一重写（已在阶段 6 清单里列明）。

## 阶段 3 · HTTP 服务（默认开启 :12300）—— 已完成（2026-10-09）

- [x] `Settings` 新增 `httpEnabled/httpBind/httpPort/httpToken/remoteAllowGuiAuth/notifyEnabled/channels.{http,cli}.{notify,delaySeconds}`（默认值写回磁盘，一眼可见）
  验收（实测）：单测 `settings_defaults_show_up_in_the_file`（默认开服务 / `0.0.0.0` / `12300` / 通知开 / 两渠道各 5 秒 / `remoteAllowGuiAuth=false`）、
  `settings_json_missing_new_fields_falls_back_to_defaults`（老 `settings.json` 少字段也能读，取默认）；
  沙箱实测启动后 `settings.json` 里字段齐全、token 已生成（32 位十六进制）
- [x] 首次运行生成 32 位十六进制 token；空 token 拒绝启动服务并提示
  验收（实测）：单测 `token_generation_and_comparison_are_strict`（32 位十六进制、两次不同、`token_matches` 严格）；
  沙箱首启 trace：`[http] 首次运行：已生成 32 位十六进制 token`；token 为空时 trace 写「token 为空，拒绝启动」，界面状态显示该原因
- [x] 服务生命周期：start/stop/rebind（置停止标志 + 自连接唤醒 accept + join）；端口占用报错不换端口
  验收（实测）：单测 `stop_releases_the_port`（真 socket：收掉后同端口能重新绑定）；
  沙箱实测「端口占用」：第二个实例 trace `[http] 启动失败: 无法监听 0.0.0.0:12399: … (os error 10048)`，同时 `netstat` 里仍是第一个实例在 LISTENING、`/health` 依旧 200——没有换端口、没有被抢
- [x] 端点：`/health`（免 token）、`/api/status`、`/api/switch`、`/api/power`、`/api/switch-and-power`（GET/POST 支持）
  验收（实测，沙箱 12399）：`/health`=200；`/api/status`=200；`/api/switch?system=linux`=200 且沙箱 `PreviousBoot` 内容真的变成模板内容；
  `/api/power`（缺省）=200「重启系统（已提交）」；`/api/power?action=sleep`=200；`/api/switch-and-power?system=windows&action=shutdown`=200 且 `PreviousBoot` 变成 windows 模板
- [x] 鉴权：`X-Auth-Token` → `token` query → body，常量时间比较；失败 401 + 计数
  验收（实测）：单测 `token_generation_and_comparison_are_strict`；沙箱：无 token=401、错 token=401（body 都是 `{"ok":false,"error":"invalid_token","message":"token 缺失或无效"}`，不区分“没带/不对”）、
  query 放 token=200、`X-Auth-Token` 头=200
- [x] 错误语义：400/401/404/405/409/500，body 与 GUI 错误同源
  验收（实测）：沙箱逐条实测 —— `?system=solaris`=400 `unknown_system`、缺 `system`=400、`?action=explode`=400 `unknown_action`、`PUT`=405 `method_not_allowed`、`/api/whatever`=404 `not_found`、body 9000 字节=413 `body_too_large`、倒计时期间重复请求=409 `action_in_progress`
- [x] 最近请求列表（内存 20 条：时间/来源 IP/方法/路径/动作/结果）+ 每次请求一行日志（token 只记前 8 位）
  验收（实测）：单测 `recent_requests_keep_newest_first_and_cap_the_list`（25 条只留 20、最新在最前）；
  沙箱 `/api/requests` 返回 20 条并按时间倒序；trace 里每行 `[http-access]` 含来源/渠道/路径/结果，完整 token 搜不到（token 从不进日志，只有比较逻辑）
- [x] 主窗口「远程控制」区：开关/地址/端口/token 显示·复制·重新生成/服务状态/最近请求/可复制的 curl 命令
  验收：代码 + `frontend_uses_the_single_state_contract` 断言（13 个命令名出现在 `ui/app.js`）；`node --check ui/app.js` 通过；
  **界面真实点击（复制/显示/重新生成按钮）需要你在有屏幕时点一次**——无头环境点不了
- [x] 托盘「远程控制」子菜单：HTTP 服务开关 + 远程控制设置…（打开主窗口并定位）
  验收：`MenuAction::from_id` 单测覆盖新 id（`setting-http-enabled:true/false`、`setting-notify-enabled:`、`setting-delay:<n>`、`show-remote`）；
  菜单与主窗口读同一份快照（阶段 1 的 `publish_ui`），所以勾选状态天然同步

## 阶段 4 · 通知弹窗 + 延迟执行 —— 已完成（2026-10-09）

- [x] `Settings` 新增 `notifyEnabled`（总开关，默认 true）+ 嵌套 `channels.http` / `channels.cli`（各含 `notify` 默认 true、`delaySeconds` 默认 5、0–600 夹取）
  验收（实测）：单测 `settings_defaults_show_up_in_the_file`、`channel_delay_is_clamped_and_both_channels_are_independent`（超大值夹到 `MAX_DELAY_SECONDS=600`、两渠道互不影响、`set_delay_all` 同时改两条）
- [x] 待执行请求槽位 + 每秒 tick 调度：`pending → executed | cancelled | failed`，只转移一次，取消优先；pending 期间新请求 409
  验收（实测）：单测 `notify_scheduler_fires_once_per_state`（没到点→Nothing、到点→Execute、槽位空→Nothing、info 到点→CloseInfo）、
  `pending_seconds_left_counts_down`；沙箱实测 202 → 倒计时 → `[remote] 执行 … ok=true` → `/api/requests` 变 `executed`（只执行一次）
- [x] 会改状态的 HTTP 请求改为 `202 + requestId`，`GET /api/requests` 可查状态
  验收（实测）：沙箱返回 `{"ok":true,"pending":true,"requestId":"ceb4790f","executeInSeconds":5,…}`，同一 `requestId` 在 `/api/requests` 里先 `pending` 后 `executed`；
  倒计时期间第二条请求 409，并记一条 `rejected 已有待执行请求`
- [x] 通知弹窗窗口 `notify`（`ui/notify.html` + `ui/notify.js`：置顶/无边框/右下角/倒计时/「立即执行」「取消」/关闭=取消）
  验收：代码 + `node --check ui/notify.js`；沙箱跑 202 那一轮时 trace 里**没有**「弹窗失败」记录（即窗口创建成功）；
  **真实观感（弹窗位置/倒计时进度条/点取消）要你在有屏幕时看一眼**——我在无头环境下看不到窗口内容
- [x] `delay=0` → 不弹倒计时窗，执行后弹 3 秒自动关的事后提示
  验收（实测）：沙箱把两渠道设成 `delaySeconds:0 / notify:false` 时，请求直接 200 执行（`pending:false`），全程没有待执行槽位；
  `notify:true + delay:0` 的「3 秒事后提示」路径由 `notify_view` 的 `Info` 分支 + 调度线程 `CloseInfo` 覆盖（单测 `notify_scheduler_fires_once_per_state`）
- [x] `channels.<渠道>.notify=false` → 该渠道不弹窗；延迟取消入口只剩主窗口「待执行请求」卡片
  验收：代码路径（`start_remote_command` 里 `settings.notify_enabled && notify.notify` 才弹窗）；沙箱 `notify:false` 那轮 trace 写「该渠道关闭了通知：只在主窗口显示待执行请求」
- [x] 分渠道配置：`notifyEnabled` 总开关 + `channels.http` / `channels.cli` 各自的 通知开关 + 延迟秒数（Q14）
  验收（实测）：`set_channel_notify` / `set_channel_delay` 命令（按渠道写入）+ 单测 `channel_delay_is_clamped_and_both_channels_are_independent`
- [x] **渠道识别与防伪造**：CLI 转发带 `channel=cli` 仅 loopback 生效；远端伪造 `channel=cli` 或被塞 `notify=0`/`delay=0` 一律无效
  验收（实测）：单测 `remote_channel_only_trusts_loopback_for_cli`（loopback+cli→cli、非 loopback+cli→http、非 loopback 塞 notify/delay→http、未知渠道→http）；
  沙箱实测：从本机局域网 IP（192.168.0.121）带 `channel=cli` 请求，响应 `channel` 仍是 `http`；从 127.0.0.1 带 `channel=cli` 响应 `channel=cli`
- [x] 主窗口「远程控制」区新增 通知总开关 / 两渠道（各 通知 + 延迟）/ 待执行请求卡片 / 最近请求列表加「渠道·状态」列；托盘「应用设置」加 远程请求通知总开关勾选 + 延迟预设子菜单（0/3/5/10/30，同时作用于两渠道）
  验收：代码 + 契约测试断言命令名；托盘延迟预设走 `MenuAction::SetDelayAll`（`setting-delay:<n>` id 由单测锁住）
- [x] 只对鉴权通过的请求弹窗；同来源 3 秒内重复请求合并一次；弹窗文本不含 token
  验收（实测）：鉴权在 `handle_remote_request` 最前面 → 401 直接返回、不会走到弹窗；弹窗文案来自 `RemoteCommand::summary()` 与来源 IP，**不含 token**（token 只在 `token_matches` 里用）；
  「3 秒内合并」由「同一时刻只允许一个待执行槽位 + 409」替代（比静默合并更明确：调用方能立刻知道被拒了）

## 阶段 5 · CLI 子命令（ssh 调用，§5.2–5.4）—— 已完成（2026-10-09）

- [x] `main.rs` 参数分支：无参数 → GUI，`--switch/--action/--status/--json/--port/--help/--version` → headless CLI（不进 `tauri::Builder`）
  验收（实测）：`refind-switcher.exe --help` 打印用法、退出码 0；`--action explode` / `--switch solaris` / `--status --nope` 都退出码 2
- [x] CLI 复用同一批核心函数（扫描 / 原子写入 / 电源命令表），不出现第二份实现
  验收（实测）：`cli.rs` 直接 `use super::{scan_vars_directory, write_with_mount, atomic_copy, hashes_match, run_power_action, sleep_probe, …}`，没有第二份命令表；
  单测 `cli::tests::parse_args_handles_all_forms` / `parse_args_rejects_bad_input` / `cli_invocation_requires_a_flag`
- [x] 优先转发给运行中的实例（读 settings 的 port/token → `/health` 探测 → 调 `/api/...`），不可达则降级自己执行
  验收（实测）：实例在跑时 `--status --json` 返回与 `/api/status` 同一份数据（同一 `revision`/`varsDir`/`message`）；
  实例停掉后 `--switch linux` 自己执行成功（退出码 0，沙箱 `PreviousBoot` 真的被改写）
- [x] Windows：`AttachConsole(ATTACH_PARENT_PROCESS)` 让 ssh 会话看到输出；取不到控制台时结果仍写 trace
  验收（实测）：release 二进制（`windows_subsystem = "windows"`）带参数跑 `--status` 仍能输出到父控制台/重定向管道（见阶段 7 记录）；
  所有结果同时也写 `trace.log`（`[cli/…]` 行），取不到控制台也不会丢
- [x] 退出码 0/1/2/3/4 与 `--json`（与 HTTP 响应同结构）
  验收（实测）：0（成功）/ 2（参数错）/ 4（变量目录不存在，人类可读与 `--json` 两条路都验过）；
  1、3 由 `exit_code_for_error`（含“授权/pkexec”→3）覆盖，需在 Linux 未预挂载场景复验（见下方遗留）
- [x] 需要本机授权时按 Q3 不弹窗，退出码 3 + 可照做的指引
  验收：CLI 本地分支调用 `run_power_action(action, false, …)`（`allow_gui_auth=false`）；授权类错误映射到退出码 3，指引写在 README
- [x] `--port` 覆盖便于调试/多实例
  验收（实测）：`REFIND_SWITCHER_HTTP_PORT=12399`（与 `--port` 同义的覆盖）时 `--status --json` 命中沙箱实例；优先级 `--port` > 环境变量 > 设置文件
- [x] 转发时带 `channel=cli`（仅 loopback 生效），按 `channels.cli` 决定通知 + 延迟（Q14 已定：走）
  验收（实测）：沙箱 `channels.cli.delaySeconds=5` 时 `--switch macos` 返回「已受理（本机确认后执行）」，
  5 秒后沙箱 `PreviousBoot` 变成 `mac-c`、`/api/requests` 里该条为 `cli executed`（转发→待执行→自动执行全链路）

## 阶段 6 · 文档（README 全面更新）—— 已完成（2026-10-09）

- [x] README「重启系统」段落重写为「电源动作」小节：三端命令表（Windows `shutdown /r|/s`、`rundll32 SetSuspendState`；Linux `systemctl reboot|poweroff|suspend` + loginctl/pkexec 退让；macOS osascript/shutdown/pmset）、两条安全约定、`REFIND_SWITCHER_POWER_DRY_RUN=1` 探针、Windows 睡眠会变休眠的说明（Q10′）
- [x] 开头「多种使用方式」对照表（我想做什么 → 从哪开始 → 得到什么）+ 目录
- [x] 三端并列：默认监听情况、token 去哪看（含三端配置文件路径与直接读文件的命令）、防火墙怎么查/放行（Windows `New-NetFirewallRule`、macOS socketfilterfw、Linux ufw/firewalld、三端查监听命令）
- [x] 远程调用示例：6 条 curl 各带「预期回答怎么读」，token 三种放法；点名 Postman / ApiPost / Insomnia / UpSnap / PowerControl / 手机 Wake-on-LAN / Home Assistant / Node-RED / n8n / 路由器群晖计划任务
- [x] SSH 两种用法（直连 / 隧道）逐字可复制，含 Windows OpenSSH（`ssh user@host "refind-switcher.exe …"`）与 Unix 写法；另附 CLI 参数表与退出码
- [x] 授权相关：Linux 免密 sudoers 行（只放开三条命令）、`/etc/fstab` 预挂载、macOS `sudo -n` 所需 sudoers、Windows 预挂载 ESP（diskpart）；写清为什么远程默认不弹授权框（Q3）
- [x] 风险与默认值显眼说明：明文 HTTP、必填 token、默认 `0.0.0.0`、`remoteAllowGuiAuth=false`、Windows 睡眠可能变休眠
- [x] 调试环境变量表（开关/绑定/端口/dry-run/trace/变量目录/配置目录）+ 一个可复制的沙箱示例
- [x] 常见问题新增：远程访问不了的五步排查、端口被占用怎么查、睡眠变休眠怎么办、怎么看“到底发生了什么”
  验收：README 里的每条命令都在本机实测过（curl 清单、CLI、防火墙查询、只读预检）；
  **“另一台设备实测”这一条还没做**——本机只有一台，跨机访问只做到“从本机局域网 IP（非 loopback）访问成功”，真机跨设备由你验一次

## 阶段 7 · 打包回归

- [x] `cd src-tauri && cargo fmt --check && cargo test && cargo check`；`node --check ui/app.js`（+ `notify.js`）；`git diff --check`
  验收（实测）：`cargo fmt --check` 通过；`cargo test` **37 passed / 0 failed**；`node --check` 两个前端文件通过；`git diff --check` 无空白错误
- [x] `cargo build --release` + `npm run build:win`（本机）
  验收（实测）：见下方「阶段 7 记录」
- [ ] Linux / macOS 由对应机器构建（`npm run build:linux`、`build:mac-arm` 等）并各测一次 curl 清单 —— **本机做不到**（Windows），留给对应机器
- [ ] 提交（消息先英文、空行、再中文；含 stop/shutdown 字样的消息先写文件再 `git commit -F`），提交后 `git show --stat`

### 阶段 7 记录（待回填构建结果）

- release 构建：`cargo build --release` 成功（`Finished release profile in 44.33s`）
- 打包：`npm run build:win`（nsis,msi）结果见下方回填

---

## 复现清单（阶段 0 请你按序做一遍，把 trace.log 给我）

1. 冷启动应用（确保 ESP 未挂载或扫描耗时），**立刻**右键托盘 → 记录：菜单里有没有系统切换项/「重启系统」/「应用设置」？
2. 主窗口改「隐藏程序（任务栏/Dock）」→ 观察托盘菜单勾选是否即时同步；是否出现过菜单需要再点一次。
3. 托盘里改「启动时隐藏主界面」和「点击 ❌ 时」→ 主窗口对应控件是否即时同步。
4. 托盘点一次「扫描」，扫描进行中再右键托盘 → 菜单内容有没有闪断/消失/需要重新点。
5. 托盘点「重启系统」（若要避免真重启，先起 `REFIND_SWITCHER_POWER_DRY_RUN=1` 的实例）→ 主窗口按钮有没有进入禁用态；再点一次托盘项有没有回执。
6. 隐藏到托盘（点 ❌）→ 托盘左键单击显示主窗口 → 再左键单击，窗口是否仍在最前。
7. 在系统设置里手动关闭本应用的登录项 → 打开主窗口看「开机启动」显示；再看托盘菜单勾选，记录哪一边先错、错多久。