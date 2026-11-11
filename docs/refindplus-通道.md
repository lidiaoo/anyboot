# RefindPlus 独立通道 + rEFInd 引擎 SelfDir 化（2026-11-11）

## 背景

本机现实：p4（REFIND）跑 rEFInd 0.14.2、p20（REFIND_PLUS）跑 RefindPlus 0.14.2.AF，
**两台都装在 `EFI/BOOT`**（fallback 布局，官方 `refind-install --usedefault` /
RefindPlus 官方支持，靠固件扫描 `\EFI\BOOT\BOOTX64.EFI` 启动、不写 NVRAM）。
而 anyboot 的 rEFInd 通道把「装在 `EFI/refind`」写死成了发现锚点，两台都扫不到。

## 引擎层（SelfDir 语义）

rEFInd / RefindPlus 一切相对路径的基准是 **SelfDir = 二进制所在目录**（upstream
`config.c` 的 `FileExists(SelfDir,…)`、RefindPlus `screenmgt.c:1647` 同语义）。
conf / 主题 / 图标 / vars 全跟着它走：

| 项 | rEFInd 0.14.2 | RefindPlus 0.14.2.AF | 源码依据 |
|---|---|---|---|
| `use_nvram` 默认 | `true` | **`false`**（文件式变量） | RefindPlus `main.c:113` |
| 配置文件 | `refind.conf` | `config.conf` → `refindplus.conf` → `refind.conf` | RefindPlus `main.c:4422-4445` |
| vars 目录 | `<SelfDir>/vars`（建不出回落 ESP 根 `refind-vars`） | 先读 `<SelfDir>/refind-vars`，再建 `<SelfDir>/vars`，最后 ESP 根 | upstream `lib.c:403-412` / RefindPlus `lib.c:739-790` |

改动（`scan.rs` / `mount.rs` / `channels/refind_menu.rs`）：

- **vars 候选**：`VARS_CANDIDATE_RELS` = `EFI/{refind,refindplus,BOOT}` ×
  `{vars,refind-vars}`，`inspect_vars_under` / `mounted_scan_candidates` /
  `mounted_candidate_directories` 三处统一走这张表；
- **conf/flavor 解析**：`resolve_conf(install_dir)` 纯函数，按 RefindPlus 官方
  搜索顺序判 flavor；`RefindConfig::for_flavor` 分派 `use_nvram` 缺省值；
- **SelfDir 黑名单**：默认 `dont_scan_dirs` 并入自身安装目录（对齐上游
  `config.c:529` 的 SelfDirPath 默认），用户显式配置则整体替换默认；
- `SimPreview` 新增 `installDir` / `flavor` 字段。

## refindplus 通道壳

- AppState/UiState/Settings 各一套并列槽位（`refindplus_{vars_dir,active_mount,
  variables,sim}` + `refindplusMode`）；
- 命令全部通道化（参数可选、向后兼容）：`refind_sim_scan{channel?}`、
  `refind_sim_switch{title,channel?}`、`switch_system{system,channel?}`、
  `set_loader_mode{loader}` 支持 `refindplus`；
- 刷新链路：`refresh_and_update` 的并行段加 refindplus 扫描（只扫已挂载卷、
  绝不弹授权；没装按「未安装」静默清槽位）；
- 远程白名单新增 `refindplus_sim_scan` / `refindplus_sim_switch`（也支持在
  `refind_sim_*` 上带 `channel` 参数）。

## 真机验证（`cargo test --lib real_machine -- --ignored`）

- `real_machine_refindplus_discovery_and_preview`：p20 发现 RefindPlus（flavor
  由 config.conf 判定、EFI/BOOT 布局、`var_mode=file`、PreviousBoot 有值）✅
- `real_machine_coexisting_installs_require_explicit_choice`：p4+p20 并存时
  rEFInd 全局扫描如实报「找到多个」而非悄悄选一个 ✅

## 已知边界（2026-11-11 第二轮已补齐大部分）

本轮把上一轮的边界全部补齐：

- **托盘** ✅：`next_boot_rows` / `loader_menu_nodes` 加 `refindplus_source`；
  菜单 id 前缀 `refindplus-switch:` / `refindplus-sys:` → `SwitchRefindPlus` /
  `SwitchSystemPlus`，与 rEFInd 并列出块。
- **电脑列表（PC hub）** ✅：`RemoteBootInfo` 加 `refindplus: BootGroup`（本地
  进程内 + 对端 `/api/ui-state` 的 refindplusMode/refindplusSim/
  refindplusVariables 两条链；老对端 available=false + note）；前端
  ComputerRow/remote.ts/ComputerList 同款「RefindPlus 启动项」列；切换走
  `remote_boot_switch {kind:"refindplus"}` → 本机 `SwitchRefindPlus` /
  远端 `/api/switch?system=…&channel=refindplus`。`remote_boot_switch_blocking`
  顺带修了 `value.trim()` 吃掉标题末尾空格的契约冲突（对 refind 也生效）。
- **移动端** ✅：`ComputerListActivity.kt` 卡片按钮 / PickSheet / 解析
  （parseRefindSim/parseRefind 参数化）/ 切换（`/api/switch?…&channel=refindplus`）
  / 离线与老对端占位全链路；`compileDebugKotlin` 通过。
- **引导高级设置** ✅：`bootAdv.ts` LOADER_IDS 加 refindplus（第 2 位）+
  `ensureOrder()` 把新 id 补进存量编排，老用户也能看到卡片。

仍然保留的边界：

- 模拟精度：RefindPlus 是 rEFInd 的 fork，引擎按 rEFInd 0.14.2 规则复算；
  RefindPlus 私有行为（`direct_boot`、`badram_tag`、私有 NVRAM GUID）未逐条对齐。
- refindplus 通道无 `set_vars_dir` 手动指定目录 UI（后端自动定位已覆盖
  EFI/BOOT / EFI/refindplus / EFI/refind 三种布局）。
- 混版本部署：新 PC + 旧被控机时，refindplus 组显示「对端不支持」占位，
  不影响其余组。
