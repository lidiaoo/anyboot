# i18n 实施 TODO（拆分到文件级，可直接勾选跟踪）

> 配套方案：[i18n-方案.md](./i18n-方案.md)。PC 端**必选 vue-i18n**；移动端各自原生资源。
> 编号：`P-` PC / `A-` Android / `I-` iOS / `R-` Rust / `T-` 工具链。
> **每项的完成定义（DoD）**：①该文件内无中文字面量（注释/品牌词白名单除外）；
> ②`npm run i18n:lint` 通过；③中英切换后该页面文案完整、无换行/溢出错乱；④截图留档。
> 勾选：`[ ]` → `[x]`；完成时在行尾补提交号。

---

## 里程碑

- **M1 = PC 可切换**：T0–T2 + P0–P2（PC 主链路中英可切，lint 生效）
- **M2 = 三端可切换**：M1 + A0–A3 + I0–I3
- **M3 = 全量收口**：P3–P6 + A4–A5 + I4–I5 + R0–R4，lint 对三端零残留

---

## 一、PC 端（Vue 3 + Tauri / vue-i18n）

### P0 基础设施
- [ ] **P0.1** 安装依赖：`vue-i18n@9` + `@intlify/unplugin-vue-i18n`；vite 配置接入插件（`include` 指向 `src/i18n/messages/**`）
- [ ] **P0.2** 建 `ui-vue/src/i18n/index.ts`（createI18n：legacy:false、globalInjection、fallbackLocale zh-CN、numberFormats/datetimeFormats）
- [ ] **P0.3** 建 `messages/zh-CN.ts`（仅骨架 + `common.*`）+ `messages/en-US.ts`（同结构）+ `d.ts`（DefineLocaleMessage）
- [ ] **P0.4** `main.ts` 挂载 `i18n`；`setLocale()` 实现（locale/<html lang>/localStorage/set_language 调用）
- [ ] **P0.5** Tauri 命令 `set_language` + 前端封装（持久化 `settings.language`；触发托盘菜单重建、通知窗刷新）→ 见 R0
- [ ] **P0.6** 语言选择 UI：`components/boot/SettingsPanel.vue` 基础设置加「中文 / English / 跟随系统」；`stores/settings.ts` 增加 `language` 字段
- [ ] **P0.7** 远程页面语言透传：iframe/blob URL 追加 `&abLang={locale}`（HomePage/ComputerList 弹层 两处 URL 构造）

### P1 导航与通用组件
- [ ] **P1.1** `pages/hub/HubApp.vue`：侧栏菜单项、顶部工具栏
- [ ] **P1.2** `pages/boot/BootApp.vue`：引导页顶部标签/返回/标题
- [ ] **P1.3** 通用组件：`components/hub/ConfirmDialog.vue`、`FbModal.vue`、`LogDialog.vue`、`WindowNavMenu.vue`、`PcMenu.vue`
- [ ] **P1.4** `stores/toast.ts` 调用方改造为传 key（`toast.show(t("..."))`）；toast 组件本身无文案
- [ ] **P1.5** `api/tauri.ts` 中面向用户的错误文案（`reportIpcFailure` 等）

### P2 电脑列表 + 电脑页（最高频）
- [ ] **P2.1** `pages/hub/ComputerList.vue`：标题/统计 chips（全部/在线/离线/待生效）/搜索占位/工具栏（操作说明/排序/刷新）/空态/**待生效**/右键菜单动作
- [ ] **P2.2** `components/hub/ComputerRow.vue`：通道列名（rEFInd/RefindPlus/GRUB/OpenCore 启动项）、状态语（在线/未连接/离线不可改/读取中…/未检测到）、回显占位（未设置/未选择）、连通行（直通/不通/复制/浏览器打开/探测超时）、电 key 文案、待生效条（含**撤销挂起**倒计时）
- [ ] **P2.3** 弹窗（`ComputerRow` 内 picker）：head（`{loader} · 选择下次启动`）、格式 chips（跟随电脑/极简/简易/中等/原始/复杂 + `条目文字格式（当前：…）`）、置顶「下次启动」、组头「{n} 项」、来源标签（默认项/折叠内核/macOS 诊断/辅助）、sub/note、取消
- [ ] **P2.4** 通道弹窗（OC 行）：tag（类型 · 来源）词表、「辅助」、只读提示（`这一项不能直接启动（由 OC/固件接管）`）
- [ ] **P2.5** `pages/hub/HomePage.vue`：tabs（本机/未连接）、空态提示（选择上方标签…）、⋮ 菜单
- [ ] **P2.6** `components/hub/RemoteListDialog.vue`、`RemoteEditDialog.vue`、`ServerEditDialog.vue`、`PairDialog.vue`、`TunnelDialog.vue`、`SystemInfoDialog.vue`、`LatencyTrend.vue`

### P3 系统引导页
- [ ] **P3.1** `components/boot/BootList.vue`：区块标题、空态、`showGroups`
- [ ] **P3.2** `components/boot/GrubEntry.vue`：`tagText`（组头 N 项 / 变体 / 只读）、`displayName` 分隔符保持代码
- [ ] **P3.3** `components/boot/GrubList.vue`、`GrubSection.vue`、`RefindSection.vue`、`bootRows.ts` 的 `CHILD_SOURCE` 改 key
- [ ] **P3.4** `components/boot/GrubEsp.vue`：ESP 配置表单（分区/UUID/目录/挂载/保持挂载说明）
- [ ] **P3.5** `components/boot/{LoaderSettings,ChannelSettings,SolPanel,HelperPrompt,TokenPrompt}.vue`
- [ ] **P3.6** `pages/boot/themes/Theme{A,B,C,G,I,J}.vue` + `LoaderIcon/StateChip`：仅状态语与按钮（其余是图标/布局）
- [ ] **P3.7** `pages/boot/BootSettings.vue`

### P4 其余功能页
- [ ] **P4.1** `pages/hub/ServersPage.vue` + `stores/servers.ts` 用户可见文案
- [ ] **P4.2** `pages/hub/BackupPage.vue`、`pages/backup/index.vue`
- [ ] **P4.3** `pages/hub/TermPage.vue`、`TermsetPage.vue`、`term/*`、`components/boot/SettingsPanel.vue`（终端设置段落，语言项已在 P0.6）
- [ ] **P4.4** `pages/hub/TransfersPage.vue`
- [ ] **P4.5** `pages/hub/SessionsPage.vue`
- [ ] **P4.6** `pages/hub/GatewayPage.vue`（远程接入/隧道/配对）
- [ ] **P4.7** `pages/hub/KeysPage.vue`、`keys/*`
- [ ] **P4.8** `pages/hub/FilesPage.vue`、`files/*`
- [ ] **P4.9** `pages/hub/AboutPage.vue`（版本/更新/检查更新/仓库链接）
- [ ] **P4.10** 更新流程文案：`stores/update.ts`（或等价）+ 更新面板按钮/进度/错误

### P5 PC 收尾
- [ ] **P5.1** 全量扫尾：`grep` 残留中文，按白名单归类修正
- [ ] **P5.2** 英文排版复核：长英文在 chips/弹窗 head/取消按钮/卡片回显/表格头 不溢出（截图 6 张）
- [ ] **P5.3** 本地化细节：数字/时间用 `n()`/`d()`；「N 秒」「N 项」统一走插值
- [ ] **P5.4** 语言切换回归：切 en → 刷新 → 保持；切回 zh；远程 iframe 跟随 `abLang`

---

## 二、Android（Compose / strings.xml）

### A0 基础设施
- [ ] **A0.1** `res/values/strings.xml`（默认中文）与 `res/values-en/strings.xml` 建骨架（先只放 `common_*`、`app_name`）
- [ ] **A0.2** `Theme.kt`/`Common.kt`：加 `AppLocale` 管理（SharedPreferences `anyboot.lang`；`setApplicationLocales`（API33+）/ 自管 `attachBaseContext` 包裹）
- [ ] **A0.3** `Drawer.kt`：抽屉菜单项与底部版本号
- [ ] **A0.4** 语言设置入口：放在「关于」或设置面板（`AboutActivity.kt` 或 `TerminalSettingsActivity.kt` 旁的通用设置）

### A1 电脑列表（主界面）
- [ ] **A1.1** `ComputerListActivity.kt`：顶栏（电脑列表/远程管理…点卡片即可发电源/搜索占位）、统计 chips（全部/在线/离线/待生效）、空态/加载/错误
- [ ] **A1.2** 卡片：通道列名、状态语（在线/离线/未连接/读取中…/未检测到/离线不可改）、回显占位（未设置/未选择）
- [ ] **A1.3** 连通行：直通/不通/复制/浏览器打开/探测超时/N ms/魔法包→HTTP→自动广播
- [ ] **A1.4** 电源四键 + 提示（本机无需唤醒/挂起到内存/HTTP）
- [ ] **A1.5** 待生效条：`引导项已修改，重启后生效` / `撤销` / `切换挂起中，%1$d 秒后在本机执行：%2$s` / `撤销挂起`
- [ ] **A1.6** 引导器弹窗（PickSheet）：head、格式 chips 与「条目文字格式（当前：…）」、置顶「下次启动」、组头「%1$d 项」、来源标签、sub/note、取消、只读提示
- [ ] **A1.7** `switchBoot` / `cancelPendingSwitch` / `channelSwitchRequest` / `switchChannel` 的 toast 文案改 `getString`
- [ ] **A1.8** `childSourceLabel()` 改返回 `@StringRes`

### A2 其余页面
- [ ] **A2.1** `ServersActivity.kt` + `ServerEditForm.kt`（服务器增删改/地址/令牌/测试连接）
- [ ] **A2.2** `BackupActivity.kt`
- [ ] **A2.3** `TerminalActivity.kt`、`TerminalSettingsActivity.kt`
- [ ] **A2.4** `MainActivity.kt`（电脑页容器/错误页/加载页）
- [ ] **A2.5** `AboutActivity.kt` + `UpdatePanel.kt` + `AppUpdate.kt`/`UpdateCore.kt` 用户可见错误
- [ ] **A2.6** `ScanPairActivity.kt`（扫码配对）、`PowerHint.kt`
- [ ] **A2.7** `DemoSeed.kt`（演示数据里的中文若会展示则改为资源；否则保留）

### A3 请求语言与后端
- [ ] **A3.1** `httpJson`/`httpJsonArray` 加 `Accept-Language: <locale>` 头
- [ ] **A3.2** 后端 `note`/`message` 已是本地化结果（依赖 R3）→ 端到端验证一条切换提示

### A4 复检
- [ ] **A4.1** 全量扫 `[一-龥]` 残留（白名单：品牌/日志/Demo）
- [ ] **A4.2** 英文长文本复核：卡片回显、弹窗 head、chips 换行、按钮宽度（截图 6 张）
- [ ] **A4.3** 语言切换回归（系统语言切换 + 应用内切换；重启保持）

---

## 三、iOS（SwiftUI / Localizable.xcstrings）

### I0 基础设施
- [ ] **I0.1** 建 `Resources/Localizable.xcstrings`（zh-Hans 为 Source Language；加 en）
- [ ] **I0.2** 语言管理：跟随系统（默认）；应用内切换用自管 `Bundle` 包装（`LocalizedBundle.swift`），Preferences 存 `anyboot.lang`
- [ ] **I0.3** `ABDrawer.swift`：抽屉菜单项
- [ ] **I0.4** 语言设置入口（AboutView 或设置页）

### I1 电脑列表
- [ ] **I1.1** `ComputerListView.swift`：顶栏/统计 chips/搜索占位/空态
- [ ] **I1.2** 卡片：通道名、状态语、回显占位、连通行、电源键与提示
- [ ] **I1.3** `pendingBar`：已修改/撤销/挂起倒计时/撤销挂起
- [ ] **I1.4** 引导器弹窗（`pickSheetOverlay`/`bootPickerContent`/`channelPickerContent`）：head、chips、置顶、组头、来源标签、取消
- [ ] **I1.5** `ComputerChannel.swift`、`ComputerTabs.swift`、`ComputerTabsView.swift` 文案
- [ ] **I1.6** `GrubFormat.childSourceLabel`/`osIcon`（图标不动，标签改 key）→ `NSLocalizedString`
- [ ] **I1.7** `toast(...)`/错误提示统一 `NSLocalizedString`

### I2 其余页面
- [ ] **I2.1** `ComputerEditorView.swift`（添加/编辑电脑）
- [ ] **I2.2** `SystemInfoView.swift`、`QRScannerView.swift`、`TofuPrompt.swift`
- [ ] **I2.3** `AboutView.swift` + 更新相关文案
- [ ] **I2.4** `DemoSeed.swift`（同 Android 规则）

### I3 请求语言
- [ ] **I3.1** `RemoteHttp.swift` 加 `Accept-Language` 头
- [ ] **I3.2** 端到端验证一条后端 message 的本地化

### I4 复检
- [ ] **I4.1** 全量扫中文残留（白名单）
- [ ] **I4.2** 英文长文本复核（截图 6 张）
- [ ] **I4.3** 切换回归

---

## 四、Rust（托盘 / 通知窗 / 状态 message）

- [ ] **R0** 新增 `src/i18n.rs`：`Lang { ZhCn, EnUs }` + `t(lang, key, &[args])`；
      `settings.language` 读写（与前端 `set_language` 命令共享）
- [ ] **R1** `src/menu_model.rs`：托盘菜单全部 label 走 `t()`（含 `▸ 组名（N 项）`、`下次启动`、`↳`不变、通道块标题、电源项、`显示主窗口/退出`）
- [ ] **R2** `src/notify.rs` + `src/state.rs`：通知窗标题/按钮/倒计时文案、`PendingRequest.summary` 生成处
- [ ] **R3** `src/remote_servers.rs`：`BootGroup.note`、`info.refindplus.note`（对端版本较旧…）、`command_reply_message` 等**返回客户端的**文案按请求方语言输出（HTTP 取 `Accept-Language`，IPC 取 `settings.language`）
- [ ] **R4** `src/lib.rs`：`/api/status` 的 `message`/`error`、`remote_status_json` 相关文案
- [ ] **R5** 单测：`t()` 双语快照 + 托盘菜单中英各一份快照（防回归）

---

## 五、工具链与治理

- [ ] **T0** `scripts/i18n-scan.mjs`：扫源码残留中文字面量（支持白名单文件/行注释、品牌词表），PC/Android/iOS 三模式
- [ ] **T1** `docs/i18n-keys.csv`：三端 key 对照表（生成 + 校验）；`scripts/i18n-check.mjs` 比对
      csv ↔ `messages/zh-CN.ts` ↔ `strings.xml` ↔ `xcstrings`，缺失/多余即非零退出
- [ ] **T2** CI 接入：`.github/workflows/build-all.yml` 的 `build-vue` job 增加 `i18n-scan` + `i18n-check`
      （Android/iOS 的 scan 也在对应 job 跑；失败即阻断）
- [ ] **T3** 文档：`docs/i18n-方案.md` 补充「新增文案流程」小节（改 zh → 同步 en → `npm run i18n:check` → 提交）
- [ ] **T4** 白名单初版：`scripts/i18n-whitelist.json`（品牌词：GRUB/rEFInd/RefindPlus/OpenCore/systemd-boot/Clover/ESP/NVRAM/APFS/OCBtOrder/Token…；文件名；日志 trace）

---

## 六、依赖关系与并行

```
T0/T1 ─┬─> P0（可先行，不阻塞）
       └─> CI(T2) 最后接
P0 ─> P1 ─> P2 ─> P3 ─> P4 ─> P5      （PC 串行，P2 完成即 M1）
A0 ─> A1 ─> A2 ─> A3 ─> A4            （可与 PC 并行；A0 依赖 R0 的语言读取可选）
I0 ─> I1 ─> I2 ─> I3 ─> I4
R0 ─> R1/R2/R3/R4 （R3 需在 A3/I3 前完成后端 Accept-Language）
```
- **PC 与移动端可完全并行**（不同 skill）。
- **R3 必须早于 A3/I3**（否则端到端验证没有本地化的后端 message）。
- **P5.2 / A4.2 / I4.2 的英文排版复核**建议在同一轮做（统一截图规范：手机宽度 + 桌面宽度各一组）。

---

## 七、风险登记

| 风险 | 影响 | 缓解 |
|---|---|---|
| 767+1004+1153 条词条量大，易漏 | 中英混杂 | T0/T1 自动扫 + CI 阻断；按页面批次提交 |
| 英文更长导致 chips/按钮溢出 | UI 破版 | 每阶段英文截图复核；必要处 `flex-wrap`/`ellipsis` 已有 |
| `{n} 项` 等插值在 vue-i18n 与 Android `%1$d` 语法不同 | 翻译不一致 | 统一以 `{name}` 为**语义源**，Android 侧转换脚本生成 `%1$d` |
| 后端文案遗留中文 | 远程视角仍中文 | R3 前置；A3/I3 带 Accept-Language；P0.7 带 abLang |
| 托盘菜单语言不跟随 | 主窗英文/托盘中文 | R1 + `set_language` 触发重建；P0.5 验收项 |
| 语言切换后缓存旧页面 | 需刷新 | PC 响应式即时；iframe 追加 abLang 并强制重载 |

---

## 八、总验收清单（M3 通过条件）

- [ ] 三端在中/英下各截 10 张关键页（电脑列表、引导器弹窗、电脑页、设置、关于），无溢出/错位/残缺
- [ ] `scripts/i18n-scan.mjs` 三端零残留（白名单外）
- [ ] `scripts/i18n-check.mjs` key 三端一致，无缺失/多余
- [ ] CI 的 i18n 检查为必需项（红灯阻断合并）
- [ ] 语言切换后：主窗、托盘、通知窗、远程 iframe 同一语言
- [ ] 版本发版（v0.2.x）包含语言切换说明
