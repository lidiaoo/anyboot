# AnyBoot `lib.rs` 拆分迁移方案（AI 可执行版）

> 生成时间：2026-10-18，基于 `lidiaoo/anyboot` HEAD（commit 844a4ac 附近）的 `src-tauri/src/lib.rs`。
> 该文件当时为 **20,455 行 / 892,946 字节**。如果你的工作区版本不同，**以符号锚点为准，行号仅作参考**。

---

## 0. 给执行 AI 的指令（必读，优先级最高）

1. **只搬移，不重写**。逐行剪切粘贴，禁止"顺手优化"、改名、改签名、调整语句顺序。
2. **cfg 属性原样保留**。本文件存在大量同名函数的三平台 cfg 副本（如 `esp_probe`、`power_attempts`、`mount_command`、`autostart_state`），必须随所在分区一起搬，一个都不能漏。
3. **一刀一 commit**。每完成一个分区：`cargo check --all-features` + `cargo test` 全绿后独立提交，message 格式 `refactor: extract <file> from lib.rs (part N/8)`。
4. **定位靠符号不靠行号**。每完成一刀，后续所有行号都会漂移。开始新一刀前，先 grep 锚点符号确认当前位置。
5. **可见性最小化**。搬出去的符号默认保持私有，只给被跨模块引用的项加 `pub(crate)`。禁止 `pub`。
6. **共享状态不动**。`AppState` / `Settings` / `BusyState` / `Channels` / `UI_REVISION` / `WS_CLIENTS` 等被多个分区使用的结构，第一步先落进 `state.rs`，之后任何刀都不再动它们。
7. 每刀结束前 grep 自检：新文件内 `crate::` 引用数 ≤ 搬移前该区块的引用数；没有新增跨模块依赖。
8. 测试不搬（`core_tests`/`linux_gpu_tests`/`tests` 留在 lib.rs）。若搬移导致测试编译错误，给被测项加 `pub(crate)` 即可，不移动测试代码。

---

## 1. 现状

```
src-tauri/src/
├── lib.rs          20,455 行（本方案要拆的对象）
├── cli.rs / grub.rs / http.rs / hub.rs / remote_servers.rs / blocking_guard.rs   ← 已拆出，不动
├── hub_term.rs / hub_sftp.rs / hub_tunnel.rs / hub_keys.rs / hub_vault.rs / hub_health.rs  ← 已拆出，不动
```

lib.rs 结构：主体（1–10357）+ 五个内联模块：

| 内联模块 | 行区间 | 真实内容（注意：login_item 名不副实） |
|---|---|---|
| `mod login_item` | 10358–15426 | 开机自启仅 ~340 行；其余是电源动作（~620 行）和 GRUB/ESP 链路（~3,700 行） |
| `mod tests` | 15427–15663 | 测试 |
| `mod linux_sni` | 15664–17374 | Linux 托盘 SNI 协议 |
| `mod core_tests` | 17375–20289 | 测试 |
| `mod linux_gpu_tests` | 20290–20455 | 测试 |

---

## 2. 目标文件树（最终形态）

```
src-tauri/src/
├── lib.rs               ~800   run() 装配 + mod 声明 + 命令注册表 + hub 开窗命令
├── state.rs             ~900   共享结构与共享 static
├── scan.rs              ~400   lsblk / 变量目录扫描
├── esp.rs               ~700   ESP 识别（三平台 probe）
├── mount.rs             ~500   挂载辅助
├── switch_flow.rs       ~350   切换状态流
├── power.rs             ~850   电源动作 + 护栏
├── login_item.rs        ~400   开机自启（瘦身后名副其实）
├── grub_esp.rs          ~1100  ESP 定位/提权写入/GRUB 扫描切换
├── grub_install.rs      ~250   mkconfig / install / uninstall 脚本
├── gpu_compat.rs        ~230   NVIDIA / WebKitGTK workaround
├── trace.rs             ~120   trace / 时间工具
├── ui_state.rs          ~210   UI 状态发布 + WebSocket
├── window_geometry.rs   ~530   窗口尺寸/居中/固定显示器
├── http_server.rs       ~1000  HTTP 服务 + token + 端口诊断
├── remote_routing.rs    ~1100  远程命令路由 + 延迟确认
├── notify.rs            ~450   提示音 + 通知窗 + 调度器
├── tray_menu_window.rs  ~480   自绘托盘菜单窗 runtime
├── dock_autohide.rs     ~1800  dock/undock/hotzone/watcher
├── dock_commands.rs     ~470   set_main_dock_* 命令
├── settings_commands.rs ~200   settings 更新/归一化/发布
├── menu_model.rs        ~1400  MenuNode 模型 + 菜单指纹
├── tray_classic.rs      ~570   classic 原生托盘菜单 + 托盘位图
├── linux_sni.rs         ~1700  （第 2 刀整体搬出）
└── （既有模块不动）
```

---

## 3. 执行顺序（8 步）与每刀卡片

### 第 1 步：`state.rs` —— 共享结构落地（其余各步的前置）

来源（主体区内分散的纯定义，逐一搬）：
- 锚点起点 `static UI_REVISION`（~329）→ `Settings`/`impl Default`（~1066–1245）
- 内容：`AppState`、`DockEdge`、`VariableInfo`、`AppStatus`、`GrubView`、`GrubRuntime`、`Channel`、`ChannelNotify`、`Channels`、`RemoteCommand`、`PendingRequest`、`RequestLogEntry`、`NotifyView`/`NotifyState`/`notify_due`、`HttpRuntime`、`HttpView`、`BusyState`(+impl)、`ActionResult`、`UiState`、`CloseAction`、`Settings`(+Default)、`SettingsView`
- 同时搬状态辅助函数：`busy_snapshot`、`set_busy`、`last_action_snapshot`、`record_action`（~2547–2595）
- 加 `pub(crate)`：所有被跨分区使用的 struct 字段与构造
- **不搬**：挂载相关（`ActiveMount`→mount.rs）、`AutostartState`（→login_item.rs）

### 第 2 步：`linux_sni.rs` —— 解包内联模块（热身）

- 来源：内联 `mod linux_sni`（锚点 `use dbus::arg::messageitem`）
- 操作：`mod linux_sni { … }` → 新文件 + `mod linux_sni;`
- 对外暴露：`menu_layout_changed` 加 `pub(crate)`（主体有 2 处调用，grep `linux_sni::` 确认）
- 文件头补 `#![cfg(target_os = "linux")]`

### 第 3 步：解包 `mod login_item` 为 4 个文件（本方案核心）

内联 `mod login_item`（10358–15426）按符号边界切成四份：

| 目标文件 | 来源区间（锚点） | 关键符号 |
|---|---|---|
| `login_item.rs` | `mod login_item {` 起 → `fn show_main_window` 前 | `macos_autostart_state/set`、`legacy_autostart_state/set`、`autostart_state/set`（三 cfg 副本）、`linux_autostart_file`、`shell_quote`、`remove_legacy_launch_agents`、`cleanup_legacy_autostart`、`startup_autostart_sync`、`toggle_autostart_setting`；从主体并入：`AutostartState`/`AutostartBackend`/`app_bundle_path`/`running_from_app_bundle`/`app_bundle_is_signed`/`login_item_failure`/`note_login_item_failure`/`clear_login_item_failure`/`launch_agent_hint`/`autostart_hint`(cfg ×2)/`macos_autostart_hint` |
| `power.rs` | `enum PowerAction`（~11039）→ `fn apply_dock_taskbar_visibility` 前；再并入主体 `POWER_RELEASE_GENERATION`/`try_begin_power`/`finish_power`/`schedule_power_release`（~2596–2680） | `PowerAction`、`PowerAttempt`、`attempt`、`PowerAttemptOutcome`、`PowerStep`、`next_power_step`、`power_attempts`(cfg ×3)、`escalate_power`、`sudo_without_password`、`apple_events_denied`、`run_power_command`、`PowerReport`、`power_dry_run`、`plan_commands`、`run_power_action`、`HibernationState`、`read_hibernate_enabled_registry`、`sleep_probe`、`spawn_power`、`render_command`；再并入内联区的 `REMOTE_NO_GUI_AUTH`/`gui_auth_forbidden`（~12731–12763） |
| `mount.rs` | 内联区 `fn unmount_temporary`（~10846）→ `unmount_temporary_inner` 结束；再并入主体 `ActiveMount`/impl（~459）、`mount_command`(cfg ×2)、`linux_user_home_by_name/by_uid`、`linux_invoking_user_home`、`linux_mount_user_ids`、`mount_options`(cfg ×2)、`remount_options`(cfg ×2)、`linux_user_ids_from_home`、`is_owned_mountpoint`、`mounted_scan_candidates`(cfg ×2)、`active_mount_for_directory`(cfg ×2)、`write_with_mount`、`temporary_target`、`atomic_copy`（~2085–2161） | 见符号列表 |
| 剩余并入其他刀 | `ShowOrigin`/`show_main_window`（→lib.rs）、`TraySetting`/`update_tray_setting`/`update_close_action_setting`（→settings_commands.rs）、`view_settings`/`publish_settings`（→settings_commands.rs）、GRUB/ESP 大区（→第 4 步） | |

⚠️ `apply_dock_taskbar_visibility`（~11656）在电源区之后，归 `dock_autohide.rs`。

### 第 4 步：`esp.rs` + `grub_esp.rs` + `grub_install.rs` —— 拆 GRUB/ESP 链路

| 文件 | 锚点区间 | 关键符号 |
|---|---|---|
| `esp.rs` | 注释"ESP 分区识别"（~11820）→ `fn esp_mount`（~12269）结束 | `EspInfo`、`esp_mountpoint`、`esp_mount_supported`(cfg ×2)、`fat_boot_sector`、`read_first_sector`、`esp_probe`(cfg ×3)、`FindmntOutput/Entry`、`DiskInfo`、`macos_read_line`、`macos_disk_info`、`macos_all_disks`、`esp_mount` |
| `grub_esp.rs` | `grub_rel`（~11715）→ 识别注释前；再从 ~12339（"本端按分区 UUID 定位 ESP"）到 `switch_grub_and_update`（~13402）结束 | `grub_rel`、`atomic_write_bytes`、`grub_script_content`、`grub_template`、`grub_installed`(cfg ×2)、`grub_synced_stamp`、`ScriptLogView`、`script_log_*`、`grub_esp_dir`(cfg ×3)、`find_mounted_by_partuuid`(cfg ×3)、`under_boot`、`sync_mount_ok`、`grub_esp_ready`(cfg ×2)、`startup_identify_scan`、`fall_back_to_dir`、`user_can`、`esp_write_needs_root`、`same_size_and_time`、`privileged_install`、`unique_temp_path`、`stage_and_install`、`esp_file_via_root`(cfg ×2)、`esp_bytes_via_root`(cfg ×2)、`write_esp_bytes`、`copy_into_esp`、`grub_collect`(cfg ×2)、`GrubScan`、`release_mount`、`merge_mount`、`grub_locate`、`grub_scan`、`rescan_grub`、`apply_grub_error`、`refresh_grub`、`grub_snapshot`、`grub_write_env`、`write_grub_entry`、`switch_grub_and_update`、命令面 `set_grub_partuuid`/`set_grub_dir`/`set_grub_list_view`/`set_grub_list_format` |
| `grub_install.rs` | "安装/卸载脚本"注释区（~13428–13608） | `binary_path`、`grub_run_mkconfig`、`linux_fsuuid_of_partuuid`、`grub_install_script`(cfg ×2)、`grub_uninstall_script`(cfg ×2) |

### 第 5 步：HTTP 与远程路由

| 文件 | 锚点区间（主体区） | 关键符号 |
|---|---|---|
| `http_server.rs` | `validate_http_token`（~2901）起的归一化中，仅取 token 相关；主体 `generate_token`（~3554）→ `stop_http_server`/`apply_http_config`（~3928–4010）前 | `validate_http_token`、`generate_token`、`short_request_id`、`machine_name`、`first_nonempty`、`health_json`、`token_hint`、`token_matches`、`local_ipv4`、`HTTP_RUNTIME`、`http_lock`、`PortOccupantView`、`PortOccupant`、`parse_netstat_listening_port`、`process_name_for_pid`、`diagnose_port_occupant`、`kill_process`、`ensure_settings_complete`、`stop_http_server`、`apply_http_config` |
| `remote_routing.rs` | `record_request_log`（~4010）→ `execute_remote_command` 结束（~4955）；再并入 `pending_snapshot`/`confirm_pending_now`/`cancel_pending_now`/`run_pending_now`（~8128–8190） | `record_request_log`、`push_recent`、`RemoteRequest`、`RemoteReply`、`remote_channel`、`route_remote_command`、`remote_status_json`、`remote_requests_json`、`remote_grub_json`、`static_ui_reply`、`handle_ui_command_request`、`ui_command_requires_local_auth`、`ui_command_is_local_only`、`ui_str`/`ui_bool`/`ui_u32`/`ui_i32`/`ui_opt_str`、`status_value`、`settings_value`、`res_unit`、`remote_ui_command`、`remote_ui_command_inner`、`start_remote_command`、`execute_remote_command`、pending 四件套 |
| `settings_commands.rs` | 归一化剩余 + web 面板 URL 区（~8212–8450）+ 内联区 settings 视图 | `normalize_theme`、`normalize_window_position`、`normalize_tray_menu_style`、`normalize_fixed_monitor`、`fixed_monitor_index`、`normalize_grub_list_view`、`normalize_grub_list_format`、`grub_layout_width`、`percent_encode_component`、`http_access_hint_text`、`web_page_url`、`current_web_page_url`、`open_web_page_now`、`update_settings_and_publish`、`update_settings_and_restart_http`、`set_http_enabled_setting`、`set_notify_enabled_setting`、`set_delay_all_setting`、`delay_submenu_title`、`view_settings`、`publish_settings`、`TraySetting`、`update_tray_setting`、`update_close_action_setting`、`set_accent_color`、`set_tray_menu_style` |

### 第 6 步：窗口与托盘

| 文件 | 锚点区间 | 关键符号 |
|---|---|---|
| `window_geometry.rs` | `default_window_height_from_screen`（~3019）→ `clamp_opacity`（~3549）前；再并入 `set_window_size` 家族（~8051–8127） | `default_window_height_from_screen`、`default_window_height`、`clamp_window_size`、`set_main_window_size`、`recenter_after_window_size`、`apply_window_size_lock`、`apply_grub_layout_size`、`grub_layout_target_width`、`main_window_monitor`、`center_main_window`、`docked_center_target`、`apply_main_window_position`、`enforce_fixed_monitor`、`clamp_opacity`、`set_window_size`、`set_window_size_locked`、`reset_window_size_inner`、`reset_window_size`、`reset_main_window_position`、`kick_webview_repaint`(cfg ×2) |
| `dock_autohide.rs` | `geometry_busy`（~5826）→ `clamp_main_window` 结束（~7658）；再并入 `apply_dock_taskbar_visibility` | `geometry_busy`、`mark_geometry_busy`、`LAST_MAIN_MOVED_MS`、`CENTER_*`(4 个 static)、`main_moved_recently`、`main_auto_hide_enabled`、`auto_hide_should_run`、`auto_hide_active`、`ensure_auto_hide_watcher`、`undock_target`、`redock_edge`、`dock_axis_position`、`AutoHideReanchor`、`UndockReanchor`、`strip_geometry`、`hotzone_hit`、`dock_main_window`、`undock_main_window`、`undock_mask_color`、`restore_main_dock_size`、`pin_main_topmost`、`unix_millis`、`webview_browser_args`、`auto_hide_diag_enabled`、`start_auto_hide_watcher`、`undocked_counts_ticks`、`auto_hide_near_edge`、`DockedMove`/`CenterMove`/`DockedPoll` 及 *_action、`hover_delay_ticks`、`window_client_position`、`main_client_position`、`spawn_diag_*`(3 个)、`handle_main_moved`、`schedule_clamp`、`schedule_center_snap`、`clamp_main_window`、`apply_dock_taskbar_visibility` |
| `dock_commands.rs` | `set_main_auto_hide`（~7659）→ `set_main_fixed_monitor` 结束（~8027 前） | `set_main_auto_hide`、`set_main_always_on_top`、`set_main_auto_hide_edge_distance`、`set_main_dock_strip_*`(6 个)、`normalize_hex_color`、`state_auto_hide_docked`、`set_main_hotzone_*`(4 个)、`set_main_window_position`、`report_dock_strip_rect`、`set_main_fixed_monitor` |
| `tray_classic.rs` | 注释"classic（各自方式）：三端原生托盘菜单"（~9221）→ 指纹注释前（~9704）；再并入 `create_tray_image`/`check_mark_image`/`build_check_mark_image`/`check_mark_rgba`/`distance_to_segment`（~2162–2247） | 见锚点 |
| `menu_model.rs` | `menu_system_rows`（~8451）→ classic 注释前（~9221）；再并入指纹区（~9704–10357） | `menu_system_rows`、`toggle_label`、`radio_label`、`grub_entry_label`、`next_boot_cascade`、`menu_status_line`、`MenuNode`、`menu_item`/`menu_choice_item`/`menu_marked_item`/`menu_marked_choice_item`/`menu_header`/`menu_separator`/`menu_submenu`、`grub_menu_nodes`、`grub_os_icon`、指纹区全部（`menu_signature_changed` 等） |
| `tray_menu_window.rs` | `TrayMenuRuntime`（~5343）→ `spawn_diag_recall` 前（~7263）中自绘菜单相关 + `tray_menu_present/click/close/nodes` 命令（~5675–5745） | `TrayMenuRuntime`、`with_tray_menu_runtime`、`TrayMenuPayload`、`tray_menu_height`、`tray_menu_max_height`、`clamp_tray_menu_height`、`open_tray_menu`、`build_tray_menu_window`、`push_tray_menu_nodes`、`emit_tray_menu_nodes`、`close_tray_menu`、`tray_menu_blur_is_fake`、`spawn_tray_menu_watchdog`、命令 `tray_menu_present`/`tray_menu_click`/`tray_menu_close`/`tray_menu_nodes` |
| `notify.rs` | `set_notify_state`（~4956）→ `open_hub_window` 前（~5140）；再并入 `spawn_notify_scheduler`（~8191–8211） | `set_notify_state`、`notify_view`、`builtin_sound_path`、`sound_player_candidates`、`play_sound_file`、`play_notify_sound`、`notify_height`、`show_notify_window`、`present_notify_window`、`place_notify_window`、`close_notify_window`、`spawn_notify_scheduler` |

### 第 7 步：剩余小文件

| 文件 | 来源 | 关键符号 |
|---|---|---|
| `gpu_compat.rs` | `env_flag_from`（~2240）→ `trace_enabled` 前（~2478） | `env_flag_from`、`LinuxDisplayBackend`、`linux_display_backend`、`nvidia_major_from_version_text`、`nvidia_driver_major`、`linux_gpu_env_overrides`、`webkit_env_already_configured`、`apply_linux_gpu_workarounds` |
| `trace.rs` | `trace_enabled`（~2478）→ `busy_snapshot` 前（~2547） | `trace_enabled`、`trace_file_path`、`now_epoch_seconds`、`utc_timestamp`、`civil_from_days` |
| `ui_state.rs` | `ui_state`（~2681）→ 远程控制注释前（~2894） | `ui_state`、`publish_ui`、`WS_CLIENTS`、`WS_NEXT_ID`、`ui_state_json`、`websocket_client_count`、`websocket_publish` |
| `scan.rs` | `LsblkOutput`（~1466）→ `state_snapshot` 前（~1872） | `LsblkOutput`、`LsblkDevice`、`ScanResult`、`MountedScanCandidate`、`file_hash`、`hashes_match`、`display_path`、`inspect_variables`、`push_unique`、`scan_unmounted_partition`、`scan_vars_directory` |
| `switch_flow.rs` | `state_snapshot`（~1872）→ `write_with_mount` 前（~2085）+ 命令面 `current_status`/`refresh`/`set_vars_dir`/`switch_system`（~13609–13643） | `state_snapshot`、`publish_status`、`refresh_and_update`、`switch_and_update`、`switch_system_locally`、`switch_and_update_inner`、`update_failure_status`、四个命令 |
| `system_display_name`（~826）等零散纯函数 | 就近归入使用它的文件 | `system_display_name` |

lib.rs 最终保留：`run()`、`command` 辅助（~55）、hub 开窗命令（`hub_open_window` 系列 ~5140–5237 + `open_hub_window`）、`ShowOrigin`/`show_main_window`、mod 声明、测试。

### 第 8 步：收尾

1. 给每个新文件写 3–5 行 `//!` 文件头：职责、负责的改动场景、依赖的共享状态
2. lib.rs 顶部写"地图注释"：文件 → 职责一览表
3. `cargo check --all-features && cargo test` + 三平台按能力各跑一次冒烟（托盘菜单、切系统、远程触发、自启开关、dock 开关）
4. 最终 commit：`docs: lib.rs 分区地图注释`

---

## 4. 全局共享状态清单（留在 state.rs，任何人不得再动）

```
AppState · Settings(+Default) · BusyState · Channels · ChannelNotify ·
UI_REVISION · WS_CLIENTS · WS_NEXT_ID · SettingsView
```

其余 static 均为单分区使用，随分区迁走：
`HTTP_RUNTIME`→http_server · `POWER_RELEASE_GENERATION`→power ·
`LAST_MAIN_MOVED_MS`/`CENTER_LAST_X/Y`/`CENTER_HAS_POS`/`LAST_CENTER_HINT_MS`→dock_autohide ·
`REMOTE_NO_GUI_AUTH`→power · `TRAY_MENU_RUNTIME 相关`→tray_menu_window

## 5. cfg 三副本对照表（搬移时必须成组）

| 符号 | 副本数 | 归属 |
|---|---|---|
| `esp_probe` | 3（Win/Linux/mac） | esp.rs |
| `power_attempts` | 3 | power.rs |
| `mount_command` | 2 | mount.rs |
| `autostart_state`/`autostart_set` | 各 3 | login_item.rs |
| `grub_esp_dir` / `find_mounted_by_partuuid` | 各 3 | grub_esp.rs |
| `esp_mount_supported` / `grub_installed` / `grub_collect` | 各 2 | esp.rs / grub_esp.rs |
| `esp_file_via_root` / `esp_bytes_via_root` | 各 2 | grub_esp.rs |
| `mounted_scan_candidates` / `active_mount_for_directory` | 各 2 | mount.rs |
| `cleanup_legacy_autostart` / `autostart_hint` | 各 2 | login_item.rs |
| `kick_webview_repaint` / `grub_install_script` / `grub_uninstall_script` | 各 2 | 见各刀 |

## 6. 验收清单（全部满足才算完成）

- [ ] `cargo check --all-features` 零警告新增
- [ ] `cargo test` 全绿（core_tests/linux_gpu_tests 不改断言）
- [ ] `wc -l lib.rs` ≤ 1000
- [ ] 每个新文件 ≤ 2000 行
- [ ] git log 显示 8 个独立 refactor commit，可单独 revert
- [ ] grep 全库无 `#[allow(dead_code)]` 新增
