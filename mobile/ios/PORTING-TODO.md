# iOS 端「一比一移植 Android」完整 TODO

> 基准：`mobile/android/app/src/main/java/com/anyboot/remote/`（21,490 行）
> 目标：`mobile/ios/AnyBootRemote/`（当前 9,950 行）
>
> **铁律**
> 1. **样式一比一**：颜色、圆角、字号、间距、控件顺序、文案全部照 Android 抄，不自行设计。
> 2. **布局用 flex + 自适应**：`ABFlow`(=Android FlowLayout/flex-wrap)、`ABGrid`(=adaptive)、`ViewThatFits`(=weight 换行)，不写死宽高。
> 3. **不产生额外想法**：Android 没有的交互、页面、动效、文案，一律不加。发现 iOS 已有但 Android 没有的（例如我上一轮自造的色板），改回 Android 的值或删掉。
> 4. 每项都必须能通过「Android 截图 vs iOS 截图」并排比对。

---

## 阶段 0 · 设计基准层（先做，否则后面全部返工）

### T0.1 色板逐值对齐
Android `Ui.java:81-141` 是唯一来源。当前 iOS `DesignSystem/ABTheme.swift` 的色值**全部是我自造的，必须替换**：

| token | Android dark | Android light | iOS 现值（错） |
|---|---|---|---|
| base | `#020617` | `#F4F6FA` | `#0D1015` / `#F4F5F8` |
| card | `#0E1223` | `#FFFFFF` | `#161A21` / `#FFFFFF` |
| elevate | `#141A2E` | `#F1F4F9` | `#1D232C` / `#F7F8FA` |
| line | `#242B41` | `#E2E8F0` | `#2A303A` / `#E2E5EA` |
| lineStrong | `#334155` | `#CBD5E1` | `#3A424F` / `#CDD2DA` |
| fg | `#F8FAFC` | `#0F172A` | `#E9EDF3` / `#14181F` |
| fgDim | `#94A3B8` | `#5A6476` | `#9AA5B4` / `#5A6473` |
| fgFaint | `#64748B` | `#8A94A6` | `#69737F` / `#8A94A2` |
| ok | `#22C55E` | `#16A34A` | `#3FD98A` / `#0E9F5A` |
| accent | `#6366F1` | `#4F46E5` | `#5B93FF` / `#2563EB` |
| warn | `#F59E0B` | `#B45309` | `#FFB43D` / `#B86A00` |
| bad | `#EF4444` | `#DC2626` | `#FF6B6B` / `#D32F2F` |
| onAccent | `#FFFFFF` | `#FFFFFF` | 缺 |

另需 `TerminalDarkBg = #04070F`、`TerminalLightBg = #FFFFFF`（`Theme.kt:49-50`）。

### T0.2 主题三挡（auto / dark / light）
`Ui.java:35-74`。键名 `ui_theme`，默认 `auto`，循环 auto→dark→light。iOS 当前只有系统跟随，需要自建 `ThemeMode` 并 `.preferredColorScheme()` 注入。抽屉里显示「暗色 / 亮色 / 跟随系统」。

### T0.3 宽度自适应因子
`Ui.java:151-162`：`scale = clamp(0.85, 1.25, widthDp / 360)`，**所有 dp 都乘它**。iOS 用 `GeometryReader` 取宽度实现同名 `ABMetric.scaled(_:)`，全部尺寸常量改为函数调用。这是"自适应"的真机制，不能用固定 pt 糊过去。

### T0.4 尺寸/字号对照表（照抄，不取整）
- 卡片圆角 `15`，卡片内边距 `start13/end13/top12/bottom13`
- 启动项圆角 `11`、高 `42`；电源键圆角 `11`、高 `54`；chip 圆角 `5`；键帽圆角 `10`、高 `36`；抽屉头部图标块 `44`、圆角 `16`；抽屉宽 `min(290, 屏宽*85%)`
- 字号：卡片名 `15 SemiBold`；状态 `10.5`；地址/MAC `11 Mono`；配对状态 `10 SemiBold`；通道徽章 `10 SemiBold`；启动项标题 `10.5 SemiBold Mono`；启动项值 `12.5 Medium`；电源键字 `11 SemiBold`；电源 hint `8.5 / 行高 10`；键帽 `12 Mono`；顶栏 chip `13`
- 结构色：状态竖条宽 `3`、高 `卡片高-28`；虚线 `4,3`

**验收**：把 Android 与 iOS 的电脑列表截图并排，色值用取色器抽样一致。

---

## 阶段 0.5 · 通用组件层 + 图标层（审计补漏 · 此前整块漏掉）

### T0.5.1 `Common.kt` 563 行 → 通用组件（`DesignSystem/ABComponents.swift` 补全）
iOS 只做了 `ABBadge/ABDot/ABChip/ABButton/ABProgressBar/ABSectionHeader/ABSectionCard/ABEmptyState/ABStat/ABKeyValue`，Android 还有：

- [ ] `SectionTitle` / `EmptyHint` / `FieldLabel` / `SecondaryText` / `ListDivider`（各自的字号色值照 `Common.kt:199-317`）
- [ ] `ChipButton(text, selected)` —— 与 `ABChip` 的选中态对照（`Common.kt:242`）
- [ ] `BarIcon(text, description, tint)` —— 顶栏图标按钮（38×38 / 圆角 10），`TopBarIcon` 专用
- [ ] `disabledFieldColors(fg, lineStrong, fgDim)` —— **readOnly 的 TextField 会吃掉点击**，必须同时 `enabled=false` 才能让外层 clickable 收到（`ServerEditForm.kt:1095` 的坑）
- [ ] `humanSize(bytes)` / `humanDuration(ms)` —— 格式化口径逐字照抄（`Common.kt:319-336`）
- [ ] `osShowName(os)` —— Windows / macOS / Linux 等显示名（`Common.kt:338`）
- [ ] `OsBadge(os, onDark, onClick)` —— 徽标配色与点击（`Common.kt:357`）
- [ ] `ConfirmBus` / `ConfirmDialog(title, message): Boolean` —— 统一确认框，替代散落的 `.alert`（`Common.kt:156-197`）
- [ ] `copyRemoteSlot(label, value)` —— 复制 + toast「已复制（label）」（`Common.kt:132`）
- [ ] `SessionEvents` / `TransferEvents` —— 事件总线（`Common.kt:79-115`）；iOS 现有 `@Published` 可替代，但**通知时机要与 Android 一致**

### T0.5.2 系统信息（`Common.kt:377-563`）→ 独立弹层，不是"逐行展示"一句
- [ ] `SysInfoRow(label, value)` 结构
- [ ] `fetchSysInfoRows(endpoint, token, timeoutMs = 5000)` —— GET `/api/system`
- [ ] `parseSysInfo(json)` —— 字段顺序与标签逐条照抄（`Common.kt:411-457`）
- [ ] `loadSysInfoRows(profile, lanEndpoint)` —— 端点点选择逻辑
- [ ] `sysInfoText(title, rows)` —— **可整段复制**的纯文本版
- [ ] `SystemInfoDialog` —— 加载中 / 错误 / 空 三态 + 底部分享与复制

### T0.5.3 `Icons.kt` 426 行 → 手绘矢量图标
Android **不是**用系统图标，是 `Canvas` 手绘：`DesktopIcon`(显示器+底座) / `ServerStackIcon`(三层机架) / `TermIcon`(终端框+光标) / `KeyIcon` / `FolderIcon`(带折角) / `DocFileIcon` / `BoltIcon` / `ClockIcon` / `CodeIcon` / `DownloadIcon` / `PencilIcon` / `CopyIcon` / `TrashIcon`(带盖) / `ChevronLeft` / `MenuIcon`(三横) / `ChevronDown` / `RefreshIcon` / `HelpIcon` 等。

- [ ] 逐个用 SwiftUI `Path` 重绘（对照 `Icons.kt` 的 `Path` 坐标，按 `size/24` 比例缩放）
- [ ] **不允许用 SF Symbols 顶替** —— 形状不一致就不叫一比一
- [ ] 统一 `DrawnIcon(size, tint)` 容器，`viewBox` 按 24×24

### T0.5.4 `FlowLayout.java` 98 行
- [ ] iOS 用 `HFlow` 已是等价能力，但要核对 **折行后的行间距** 与 Android 一致（Android 是固定 `spacedBy` 的横纵间距）

### T0.5.5 行为契约测试（`app/src/androidTest/`）
- [ ] `PairUpsertTest.java` → XCTest：`upsertPair` 的 `(mid, kind, lane)` 幂等
- [ ] `ApplyRemoteHintTest.kt` → XCTest，两个用例逐条照搬：
  - `scanDirectThenFunnelAddsTwoRows`：先扫直连再扫 Funnel → **2 条**（不能顶掉）
  - `rescanSameLaneUpdatesNotDuplicates`：重扫同 lane → 仍 2 条，token 被更新
- [ ] 跑通后接入 CI（`xcodebuild test`）

---

## 阶段 1 · 电脑列表页（对齐 `ComputerListActivity.kt` 3286 行）

已有：卡片骨架、六通道行、探测状态、电源菜单、测试/编辑入口。
缺口按 HostCard（839-1198）逐段补：

- [ ] **1.1 当前选中态**：`selectedId` + `ServerStore.setActiveId` 落盘；选中卡片 = accent 7% 底 + 1.5pt accent 边框 + 左侧 3pt 状态竖条 + 「当前」chip（10 SemiBold，accent 16% 底 + 30% 边框，圆角 5）
- [ ] **1.2 卡片头行**：名称（15 SemiBold，weight 占满剩余）→「当前」→ OS 徽标（可点开系统信息）→ 连接标注 chip（10 SemiBold，限宽 110，自身省略）→ ⋮
- [ ] **1.3 OS 徽标**：`machineOs` → Windows/macOS/Linux，图标 + 着色（对齐 `ServerProfile+Presentation.osSymbol`，已存在，接线即可）
- [ ] **1.4 状态线**：在线/离线/读取中/唤醒中/重启中/关机中/睡眠中 七态 + 各自颜色（`stateColor` 3045-3055）
- [ ] **1.5 多地址逐行**：`hostAt(i)` 等宽 11 + `LanHitBadge`（该网卡通=延迟绿、不通=红、未测=灰）+ `macsForHost` 大写 MAC 逐行；无地址显示「(未配地址)」
- [ ] **1.6 远程配对逐条**：状态（10 SemiBold，宽 72）+ 通道徽章 + 地址（11 Mono，weight 自适应）+ lane + 来源 + 「复制」「打开」；停用整行 fgFaint
- [ ] **1.7 tunnelNote**：⚠ 图标 + warn 色 10 号提示
- [ ] **1.8 引导两项**：`BootRow` rEFInd / GRUB（见阶段 2）
- [ ] **1.9 待生效条**：「引导项已修改，重启后生效」+「撤销」（pendBg/pendBorder/pendText，圆角 9）
- [ ] **1.10 电源四键**：唤醒/睡眠/重启/关机，圆角 11、高 54，每键两行 hint；离线时唤醒给实心 ok 色；执行中显示 14pt 进度环
- [ ] **1.11 卡片 ⋮ 菜单**：编辑 / 删除 / 浏览器打开 / 复制地址 / 系统信息 / 测试
- [ ] **1.12 顶栏**（对齐 `TopBar` 329-405，**此前写错过，以本行为准**）：
  - 高 54，底色 card，`start6 / end8` 内边距
  - 左：☰（38×38 方角按钮，圆角 10，透明底，图标 20）
  - 中：标题「电脑列表」`16.5 Bold` + 副标题「远程管理 - 电脑 · 点卡片即可发电源指令或切引导项」`11 fgFaint`，均单行
  - 右：刷新（38×38；**刷新中换成 16pt 圆形进度、stroke 2**）/ 帮助（「操作说明」弹层）/ ⋮（溢出菜单）
  - 没有「＋」按钮 —— 添加电脑在 ⋮ 里
- [ ] **1.13 搜索框**：第二行 `SearchBox`，按名称/地址/分组过滤（`matches` 2062-2093）
- [ ] **1.14 统计筛选条**（对齐 384-401，**既是统计也是筛选器，此前写漏**）：
  - 四段：`全部 n` / `在线 n` / `离线 n` / `待生效 n`，间距 7，可横向滚动
  - **数字必须基于全量 rows**，不受搜索与当前筛选影响（否则点进「离线」后「在线」变 0）
  - 点击切换 `filter`；「待生效」用 pend 色，其余按选中态着色（`StatChip`）
- [ ] **1.15 ToastStack**：底部堆叠轻提示，替代 alert
- [ ] **1.16 自动探测**：前台每 10s 静默刷新一轮（`AUTO_PROBE_INTERVAL_MS`），手动刷新时跳过；进入后台停表，回前台先补一轮
- [ ] **1.17 空态**：无电脑（引导加第一台）/ 有电脑但都离线，两套文案
- [ ] **1.18 操作说明弹层**（`PickSheet` 的 help 分支，`ComputerListActivity.kt:1975-1984` + `HELP_LINES:3217`）：
  标题「操作说明」14.5 Bold，五条 `·` 起头的说明（字号 12.5、行高 22、fgDim），末尾一行
  「电源操作不会弹出确认框」11 fgFaint。**文案逐字照抄**：
  1. 唤醒：发标准魔术包（UDP 9）。睡眠 / 重启 / 关机按每台电脑「电源路由」里配的通道发送，可以是局域网广播包，也可以走它自己的 AnyBoot 接口
  2. 四个电源键任何时候都能按，不会因为在线/离线变灰；关机与重启立即执行，不会二次确认，请先保存工作
  3. 引导项（rEFInd / GRUB）在下次重启时生效，可点「撤销」恢复成原来那项
  4. 电脑离线时读不到引导项
  5. 显示「未检测到」= 这台电脑没部署对应的引导管理器（rEFInd / GRUB），不是读取失败

---

## 阶段 1.5 · 电脑编辑表单（`ServerEditForm.kt` 1891 行 · 审计补漏，此前只写了"换皮"）

**这是 Android 第三大文件，比整个电脑列表页的卡片逻辑还长。iOS 现在 481 行，覆盖不到一半。**

### 1.5.1 表单状态机 `ServerEditFormState`（1-630）
- [ ] `AddrEntry(address, macs)` + `addressEntries` 列表；`upsertAddress(address, macs)` 已存在只并 MAC 并返回是否新增
- [ ] `macPool` —— 从已添加地址汇总的可选 MAC 池
- [ ] `recognizeQuick()` —— **快速识别**：整段文字粘进来，`parseAddressInput(raw, strict=true)` 拆出多地址多 MAC；备注自动忽略；顺手用 `tokenFromUrl` 收 token；回填 `pastedRaw`
- [ ] `addManualAddress()` —— 地址 + 多 MAC（`[\s,;，；]+` 分隔）
- [ ] `maskToken(value)` —— 前 4 后 4 可见，中间 10 个 `•`；`≤12` 位整串打码（最少 6 个 `•`）
- [ ] `tokenFromUrl(raw)` —— 从整条 URL 里挖 token（**iOS 目前没有这个方法**）
- [ ] `applyRemoteHint()` —— 粘贴/扫码分拣，`(mid, kind, lane)` 幂等；无 mid 时按 `(addr, kind)`；满 `MAX_PAIRS=32` 时提示「先删掉几条再加」
- [ ] `resolvedToken()` —— 填了用填的，没填从原文再挖
- [ ] `probeTimeoutText` —— **秒文本**（支持一位小数，如 `2.5`），落盘换算毫秒并 `clamp(200, 30000)`
- [ ] `routeTimeoutTexts` —— 只对 `lan` / `jump` 有效；`routeTimeoutsMs()` 落盘时**只收单独设过的通道**（配对通道走区块超时，两套 key 空间不能混）
- [ ] `blockTimeoutTexts` + `blockModes` —— 区块默认超时与区块内策略
- [ ] `toParsed()` / `resolvedJumpId()` / `jumpLabel(jump)`（含「（仅 Telnet，不能做隧道）」）
- [ ] `routeMove(kind, delta)` / `routeAvailable` / `routeSub(kind)`

### 1.5.2 表单 UI（644-1486），分区顺序不许变
1. [ ] 「电脑名称（同一台电脑的不同网卡/地址，用名字区分）」+ 输入框，占位「例如：有线网卡 / 5G WiFi（可留空，用地址兜底）」
2. [ ] 「快速识别（整段粘进来，自动拆出地址 / 端口 / MAC 加进下面）」+ 多行框 + 「识别并加入」按钮 + 右侧提示（默认「换行 / 空格 / 逗号都能分开；认不出的备注自动忽略」）
3. [ ] 「已添加的地址（第一个用来连接，其余参与测试连接和唤醒）」：
   - 空态「还没加地址 —— 上面粘一段，或者下面手动填一个。」
   - 每项小卡（圆角 10、card 底、内边距 8）：地址 + 首项「连接用」（10 accent）+ ✕ 删除；MAC 逐行可 ✕；无 MAC 时「没配 MAC —— 局域网电源发不出去」
4. [ ] 「手动添加（地址 + 多个 MAC；地址已存在时只追加 MAC）」两框 + 「添加」
5. [ ] 「访问令牌 token（服务端设了才要填）」：**已存令牌打码回显 + 「显示/隐藏」切换**（重开表单回到打码）+ 新令牌框（占位「留空保持不变；要替换就填新令牌」）
6. [ ] 「远程配对（三个区块，每个区块可以挂多条；同类型配一条不会顶掉另一条）」：
   - 粘贴框（占位含 `anyboot://pair?v=2&…` 与两个 http 例子）+ onValueChange 即时 `applyRemoteHint`
   - 行：「📷 扫码读码」/「全部清空」（无配对时文案是「清空」）/ 右侧 hint（默认「扫码或粘贴后自动加入对应区块；同一条会更新而不是重复」）
   - 三区块（圆角 11、card 底、`h9 v7`）：标题 + 「未配 / n 条」+「＋ 添加」；空态「这个区块还没有配对 —— 上面粘一条配对码，或点「＋ 添加」手填」
   - `PairEditRow`（1627-1766）：地址 / lane / 来源 / 令牌 / **该条自己的超时** / 启用开关 / 复制 / 删除
   - 底部说明「保存后每个区块里的配对各自独立探测、各自计时；清空全部保存 = 解除配对。配对码等同密码，别转发给别人。」
7. [ ] `FoldHeader`「连接策略」（**可折叠**，summary 一格塞五项）：`优先级 n/5 · 通道间 X · 多网卡 Y · 默认 2.0s（n 个通道单独设）· 跳板机`
8. [ ] 折叠内 → 连接优先级逐行：☰ 图标 + 中文名 + `routeSub` 子行 + **内联超时框（仅 lan/jump）+「超时见区块设置」占位（其余四个通道）** + ↑ / ↓ + Switch（不可用通道整行 fgFaint 且 Switch 禁用）
9. [ ] 折叠内 → 「连接优先级的探测方式」（三选一 `ProbeModeChoice` 均分）+ 说明
10. [ ] 折叠内 → 「多网卡探测」（三选一）+ 说明
11. [ ] 折叠内 → 「区块内部策略与区块默认超时」：每区块**两行**（第一行 名称 + 「默认超时」框；第二行 三个策略按钮）—— 一行塞不下六个元素是 Android 踩过的坑
12. [ ] 折叠内 → 「经由跳板机」下拉（**必须 disabled 而非 readOnly** 才能点开）+「直连（默认）」
13. [ ] 折叠内 → 默认探测超时（秒）单框 + 说明（上限 30 秒）
14. [ ] `FoldHeader`「局域网电源路由」（summary：未配置 / n 条）→ 每路由卡：
    - 「路由 i / n」小字
    - 动作下拉（唤醒/重启/关机/睡眠）+ 地址 + 端口
    - 模式下拉（优先广播包 / 优先 HTTP / 仅 HTTP / 仅广播包 / 混合）+ 包数 + 重试 + 启用 + 删除
    - **切换动作时自动重置 burst/retry 为该动作默认值**（唤醒 10/10，其余 1/1）
    - 「加一条电源路由」
15. [ ] 底栏：取消 / 保存 / （编辑态）删除
16. [ ] `applyServerEdit` / `saveServerProfile`：保存后按 id 回查共享列表（对象会被重建）

---

## 阶段 2 · 引导管理（当前完全缺失 · AnyBoot 核心）

### 2.1 数据层（新建 `Model/BootGroup.swift`、`Net/RemoteHttp.swift`）
- [ ] `BootOption` / `BootGroup` / `BootInfo` / `GrubRow` 四结构
- [ ] `parseRefind`：`/api/status` → `variables[]`（exists/system/displayName/active）
- [ ] `parseGrub`：`/api/grub` → `entries[]`（selector/name/submenu/class/chainloader/kernels/variants）、`groups[]`、`installed`、`listFormat`、`efiDir`、`partuuid`、`nextEntry`、`savedEntry`
- [ ] `httpJson(endpoint,token,path)`：GET 带 token；401→「这台电脑不认这个 token（401）」；非 JSON→明确报错；≥400→取 `message`
- [ ] `chooseEndpoint`：按 `ProbeEngine` 赢家选点；lan/jump 用 `http://`+局域网令牌；远程用该配对令牌；全不通回退 `/  局域网端点（alive=false）`
- [ ] `switchBoot`：`/api/switch?system=` 与 `/api/grub-switch?entry=`，回传 message

### 2.2 GRUB 拼装（纯函数，逐条照抄 2690-2821）
- [ ] **六档格式**（`grubEffectiveFormat`，**此前写成"五档"是错的**）：
  chip 六个 =「跟随电脑」`""` / 极简 `minimal` / 简易 `simple` / 中等 `medium` / 原始 `raw` / 复杂 `detailed`
  - 用户自选 > 服务端 `listFormat` > 回退 `minimal`
  - **落盘 key = `grubListFormat`，命名空间 `anyboot_ui`**（不是 profile 字段，与 Android 同 key）
  - 切换条：标题「条目文字格式（当前：跟随电脑 · minimal）」11 fgFaint；chip 圆角 8、`h9 v5`、字号 11.5，
    选中 accent 底 + 白字 Bold，未选 elevate 底 + fgDim；横向滚动、间距 6
- [ ] `grubOsIcon` 🪟/🍎/🐧、`grubStripKernel`、`grubKernelOf`、`grubRowLabel`、`grubCollapse`
- [ ] `grubDisplayRows`：当前项置顶（子项带「组名 › 」）→ 顶层项 → 组头「▸ 图标 组名（N 项 · 位于 ESP · UUID=…）」→ 子项「↳ 」；raw 用 variants；detailed/raw 附内核名/「转交引导」/class
- [ ] `refindClass` / `grubClass`

### 2.3 UI
- [ ] `BootRow`：圆角 11、高 42、虚线边框（不可用态）、左侧 46 宽标题、BootDot（7pt；auto 为空心环）、值、待重启 chip、chevron
- [ ] 三档不可用文案：读取中… / 离线不可改 / 未检测到
- [ ] `PickSheet` 容器（`ComputerListActivity.kt:1907-1967`，**三种内容共用一个底部弹层**）：
  - `ModalBottomSheet` + `skipPartiallyExpanded`，`card` 底
  - 拖拽手柄：36×4、圆角 2、`lineStrong`、上下各 8
  - 标题行：`start18 / end8 / bottom10`；标题 14.5 Bold（「操作说明」/「rEFInd 启动项」/「GRUB 启动项」）；
    右侧电脑名 11 fgFaint 可省；关闭钮 30 圆形 `elevate` 底 + CloseIcon 14
  - 内容区 `horizontal 18` 可滚动，底部 `navigationBarsPadding` + 12
  - GRUB 分支顶部先放格式 chips（见 2.2），rEFInd 分支**直接列条目不拼装**
- [ ] `PickSheet` 内容：当前项打勾、分组缩进、raw 行尾内核名、点选即切换
- [ ] 切换后自动重读引导项

---

## 阶段 3 · 电脑页（对齐 `MainActivity.kt` 2074 行）

### 3.0 WebView 壳层配置（**审计发现三个真 bug，现状是错的**）
- [ ] **UA 标记**：`userAgentString += " AnyBootRemote/1.2"`（`MainActivity.kt:1956`）。
  前端 `ui/app.js:193` 用 `/AnyBootRemote\//` 判断是不是 App 壳，**决定下拉刷新是否交给原生整页重载**。
  iOS 现在没设 UA → 网页认为自己是浏览器 → 刷新行为与 Android 不一致。iOS 用
  `configuration.applicationNameForUserAgent = "AnyBootRemote/1.2"` 或在 `didFinish` 前用
  `WKUserScript` 覆盖 `navigator.userAgent`。
- [ ] **`anyboot://reload` 拦截**：网页下拉刷新会跳 `anyboot://reload`，Android 在
  `shouldOverrideUrlLoading` 里拦成 `reloadActive()`（先探全部候选再重载，换 IP 也跟得上）。
  iOS 的 `decidePolicyFor` 现在把非 http(s) **全部交给 `UIApplication.shared.open`** ——
  `anyboot://reload` 会被当成 deeplink 打开，等于点了没反应。必须显式拦这个 scheme。
- [ ] **`isCurrentAddress` 守卫**：`onPageStarted` / `onPageFinished` / `onReceivedError` 都要先确认
  回调的 URL 还是当前在试的地址，否则**换地址后旧地址的迟到回调会把状态改回去**（`MainActivity.kt:1984`）。
- [ ] 错误判定只看主文档：`request.isForMainFrame`，子资源失败不盖页面（iOS `didFail` 无此区分）
- [ ] 混合内容放行（`MIXED_CONTENT_ALWAYS_ALLOW`）、DOM storage、禁用缩放、`LOAD_DEFAULT` 缓存
- [ ] 站内 http(s) 跳转留在 WebView；`mailto:`/`tel:` 交给系统且**失败要静默忽略**

### 3.1 起
- [ ] **3.1 多标签**：每台电脑一个保活 WebView，切走隐藏、切回不重载（`ensureTab` / `connect(forceReload=false)`）
- [ ] **3.2 标签条**（对齐 `TopBar` 403-495）：
  - 容器 `card` 底，`start4 / top6 / end4 / bottom6`
  - ☰ + 横向 `LazyRow`（间距 6，占满剩余）+ ＋（添加电脑）+ ⋮
  - chip：**圆角 16**、高 22、`start10 / top5 / end12 / bottom5`；选中 accent 底，未选 elevate 底
  - chip 内：状态点 **8pt** → 间距 6 → 名称 **13**（`maxWidth 132` 自身省略）→ 间距 5 → OS 徽标（`onDark = 选中`）→ 间距 5 → 连接标注 **9**（选中时白 75%，否则 fgFaint）
  - 状态点色序：**loading 警告 > 心跳在线 ok > 心跳离线 faint > 标签 ok 绿 > 标签 fail 红 > faint**
  - 点 chip 切换、**长按编辑**
- [ ] **3.3 连接标注规则**：没连上写「未连接」，不许默认「直连」；连上取心跳赢家通道；赢家未出按实际路径兜底（远程/跳板机/直连），跳板机被停用不许出现（`connLabelOf` 505-519）
- [ ] **3.4 切标签自动滚动到可视区**
- [ ] **3.5 连接流程**：`defaultLocalKind`（优先级 lan/jump 谁前走谁）→ `loadCurrentAddress`（赢家优先，正在试的不算试过）→ `connectThroughTunnel`（隧道成功加载 127.0.0.1:port，失败换地址）→ `advanceAddress`（多地址轮换）→ `connectRemote`（远程兜底）→ `rememberWorkingAddress`（连上后提到最前）
- [ ] **3.6 ErrorPanel 全文**：标题「连不上服务端」+ detail + 「顺手检查这几点」四条 + 自动重试状态 + 重试 + 修改这个服务端
- [ ] **3.7 溢出菜单九项**：添加电脑 / 编辑当前电脑 / 电脑列表 / 当前电脑的系统信息 / 重载当前页面 / 全部重载 / 启动时连接全部服务端（开√）/ 连不上时自动重载（每 N 秒）/ 传输方式（当前值）
- [ ] **3.8 failReloadSec**：落盘，0=关闭；失败后按间隔自动重载
- [ ] **3.9 transport**：push / poll 落盘，随 URL 带进网页
- [ ] **3.10 connectAll**：落盘 + 启动时批量连接
- [ ] **3.11 电源悬浮球**：右下角，展开四 chip + 顶部「电源走局域网 · 需 sol ?」说明条，底部抬高 48
- [ ] **3.12 电源帮助弹层**：睡眠/关机/重启语义、广播到哪第几包、BIOS 开 WOL、必须有线网卡 MAC、没装 sol 的后果（原文照抄 1065-1130）
- [ ] **3.12b `powerHint` 文案规则**（`PowerHint.kt` 62 行，**此前 TODO 写错了**）：
  - badge 六档由「该动作启用中的路由」的 `routesMode` 决定：
    `http_only`→`HTTP` / `http_first`→`HTTP→魔法包`（远程时降级 `HTTP`）/ `hybrid`→`HTTP+魔法包` /
    `auto`→`魔法包→HTTP` / 无路由且唤醒→`魔法包`（远程时 `发不出`）或 `未配 MAC` / 无路由非唤醒→`HTTP`
  - **有路由**：`badge · → 目标:端口`，`target` 为空显示「自动广播」；多条追加「 等 N 条」
  - **无路由且是唤醒**：无 MAC→`未配 MAC`；远程通道中→`发不出 · 魔术包仅局域网有效`；否则`魔法包`
  - **无路由非唤醒**：`badge · fallback`（fallback 由调用方给，如「挂起到内存」）
- [ ] **3.13 系统信息**：`/api/system` 逐行展示，加载/错误/空三态

---

## 阶段 4 · 抽屉导航（对齐 `Drawer.kt` 213 行 · 当前缺失）

- [ ] 盖层 + 左抽屉，宽 `min(290, 屏宽*85%)`，遮罩 `#55000000`
- [ ] 头部：44pt 图标块（圆角 16、accent 15% 底）+「AnyBoot」16 Bold +「v版本 · remote」10
- [ ] 导航五项：电脑 / 电脑列表 / 服务器 / 关于 / 主题：当前值（点击循环）
- [ ] 行高：内边距 `14/12`，圆角 12，选中 accent 12% 底 + accent 字 SemiBold
- [ ] 底部：心跳 10s · 并发探测 / GitHub · lidiaoo/anyboot / 外网连家里说明（10 号，行高 16）

---

## 阶段 5 · 服务器模块（对齐 `ServersActivity.kt` 3224 行）

结构是「顶栏 + 页面 + 底部四 tab」，iOS 现在**顶栏缺件、底部导航做成了五项且布局错**，需要重做：

### 5.0 顶部栏（对齐 `TopBar` 254-345，**此前完全没写**）
- [ ] 底色 `base`，内边距 `start4 / top10 / end20 / bottom8`
- [ ] ☰（`BarIcon`，进抽屉）
- [ ] 标题 **20 Bold**，随页变：服务器 / 密钥 / 文件 / 会话 / 传输
- [ ] `PagePill`（monospace 10、`elevate` 底、`line` 1px 边、圆角 999、`h8 v3`）：
  - 服务器页 →「n 在线」（n = SSH 会话在线的服务器数）
  - 密钥页 →「n 把」
  - 文件页 → 圆点(6pt) + 「SFTP 就绪」/「未连接」；就绪时 accent 10% 底 + accent 字，否则 elevate 底 + fgDim
  - 会话 / 传输页 → 无 pill
- [ ] 齿轮（36×36，仅**服务器页**显示）→ 进「终端设置」
- [ ] ⋮ 菜单三项：会话（进行中 / 历史）/ 传输管理 / 终端设置
- [ ] FAB（36 圆形 accent，仅服务器页与密钥页）：服务器页 = 添加服务器；密钥页 = 弹出「生成密钥对 / 导入密钥」

### 5.1 底部导航 **只有 4 项**（对齐 `BottomNav` 398-427，**此前写成五项，是错的**）
- [ ] 四项顺序：**服务器 / 终端 / 密钥 / 文件**
- [ ] 每项 = 20pt 图标 + 10pt 标签（选中 accent + Medium；未选 fgFaint + Normal），`top3` 间距
- [ ] 「终端」**不是页面**，是动作项：跳最近一个活会话的终端页；没有活会话时 toast
      「没有进行中的会话 —— 到「服务器」页点「终端」开始」
- [ ] **会话页 / 传输页不在底部导航**，从顶部 ⋮ 进
- [ ] 容器：顶部 1px `line` 分隔线、`card` 底、`top8 / bottom6` + 底部安全区
- [ ] 图标必须是手绘的 `ServerStackIcon` / `TermIcon` / `KeyIcon` / `FolderIcon`（见 T0.5.3）

- [ ] **5.2 传输状态条**：**只在文件页**（`currentPage == 2`）出现，有活动任务时显示
  「spin + 进度 + 百分比」，点一下进传输页；不在全局常驻（**此前写成"底部常驻"，错了**）
- [ ] **5.3 会话页**：进行中（绿点 + 名称 + 已连接时长 + 结束 + 点击进入）/ 历史（名称 + 起止时间 + 时长 + 清空）
- [ ] **5.4 服务器卡片**：首字母图标 / 名称 / 协议徽章（SSH accent、Telnet 橙）/ RTT / 地址；展开显示端口·用户·认证方式、备注、经由此服务器的电脑数、SSH 在线态、动作组（打开终端/浏览文件/测试连接/编辑/删除）
- [ ] **5.5 删除确认**：提示「有 N 台电脑配置了经由它连接，删除后退回直连」
- [ ] **5.6 密钥页**：算法 + 指纹（缩短为前 16 位）+ 关联服务器数 + 复制公钥 + 删除（使用中拦截）；导入文件；生成（Ed25519 不支持时提示改 RSA 4096）；名称输入
- [ ] **5.7 文件页**（对齐 `ServersActivity.kt:1774-2948` + `SftpOps.java:184-435`）：
  - 面包屑分段可点、末段高亮；排序（名称/大小/时间 × 升降序）、隐藏文件开关、刷新、根/家目录快捷键
  - 行 = 图标 + 名称 + 大小 + 时间；目录在前；**软链接用 🔗 单独图标**（`Entry.link`，iOS 现在没解析）
  - 动作面板：详情 / 重命名 / 删除 / **复制 / 剪切 / 粘贴** / 下载 / 上传到此 / 在终端打开
  - 详情：权限（rwx 串）、**uid / gid / atime / linkTarget / typeLabel**（iOS 现在只有 name/size/mtime/perms）
  - **`exists` 重名检测** → 自动 `name (1).ext`，上限 99 报「同名副本太多了」
  - **`removeTree` 递归删除**：iOS 现在 `rmdir` 删非空目录直接失败（**功能 bug**）
  - **`home` 取家目录**、**`resolve` 解析 `~` 与相对路径**（iOS 都没有）
  - 粘贴到自身目录要拦；跨设备移动；空目录 / 加载中 / 错误三态
- [ ] **5.8 传输页**：进行中/已暂停/已收尾三段；进度条（确定/不确定态）、百分比、已传/总量、速率、剩余；暂停/继续/取消/重试/清空
- [ ] **5.9 手动密码对话框**：SSH 与 SFTP 共用，勾「保存密码」默认开

---

## 阶段 6 · 终端（对齐 `TerminalActivity.kt` 868 + `TerminalSettingsActivity.kt` 252）

- [ ] **6.1 独立终端设置页**：配色（暗/亮单选）、字号（A- 数字 A+，8~28）、真实样例行、字体族输入 + 说明、自动弹键盘、显示快捷键栏、恢复默认、底部说明文案
- [ ] **6.2 设置热套用**：所有改动落盘（`terminal_dark` / `terminal_font_size` / `terminal_font_family` / `terminal_auto_keyboard` / `terminal_key_bar`），已开终端返回时 `applyStoredSettings()`
- [ ] **6.3 顶栏 A- / A+**：临时调字号并落盘
- [ ] **6.4 按键栏 8+4**：Esc / Tab / Ctrl / / / - / ↑ / ↓ / | + ← / → / Enter / ⌨
- [ ] **6.5 Ctrl 字母行**：武装后出现 A~Z（横向滚动），点击发 `0x01~0x1A`，发一次解除武装
- [ ] **6.6 多会话**：顶部会话 chip、点击切换、长按结束、结束确认；切换时 `termSync` 重报尺寸
- [ ] **6.7 手动密码对话框**

---

## 阶段 7 · 扫码（对齐 `ScanPairActivity.kt` 296）

- [ ] 相机权限：拒绝时给说明 + 「去设置里授权」，不黑屏
- [ ] 取景框（黑底圆角 16）+ 底部提示「把二维码放进框内即可，稍等一两秒」
- [ ] 只接受首次有效结果（`compareAndSet` 语义）
- [ ] 顶部「‹ 返回」+ 标题「扫码读配对码」+ 两行说明文案

---

## 阶段 8 · 关于页（对齐 `AboutActivity.kt` 354）

- [ ] 版本号 / GitHub 仓库 / Star 引导 / 仓库统计（stars·forks·issues）/ 技术栈说明 / 许可链接 / 文档图标（iOS 已有基础，逐条核对文案与顺序）

---

## 阶段 9 · 收尾验证

- [ ] 全部页面在 iPhone SE（窄）/ 15 Pro（标准）/ iPad（宽）/ 深色 / 浅色 五格截图
- [ ] 与 Android 同分辨率截图并排比对：色值、圆角、字号、间距、控件顺序
- [ ] 真实连接测一遍：局域网直连、跳板机、隧道、远程配对；读引导项、切引导项、电源四键
- [ ] 大文件上传/下载（>1GB）不 OOM、暂停续传正确
- [ ] 深色/浅色/跟随系统三挡切换后无残留颜色

---

## 附录 A · 方法级缺口（审计逐方法比对结果）

### A.1 `ServerProfile.java` → `Model/ServerProfile.swift`
已有大部分，**缺这 7 个**：
- [ ] `tokenFromUrl(raw)` —— 从整条 URL 里挖 `token` 参数（`2046`）；`recognizeQuick` 依赖它
- [ ] `sameConnState(a, b)` —— 连接状态比较（`1446`）；**心跳只在变化时通知 UI**，否则每 10 秒全量重组
- [ ] `routesMode(routes)` —— 一组路由的统一模式（全 http_only / 全 auto / 混合…）（`775`）；`powerHint` 依赖它
- [ ] `modeLabel(mode)` —— 模式中文名（`809`）
- [ ] `blockFromLegacyType(raw)` / `defaultBlockFor(origin)` —— 老 type 迁移 / 从地址推断区块（`1023`、`1048`）
- [ ] `pruneMacs()` —— 把不在地址里的 MAC 清掉（`834`）
- [ ] `parseAddresses(raw)` —— 只要地址不要 MAC 的简版（`1821`）

### A.2 `HeartbeatManager.java` → `Probe/Heartbeat.swift`
- [ ] **远程降频 60 秒**（`REMOTE_FALLBACK_INTERVAL_MS`，`232-242`）：本地优先的档案，本地全挂后**60 秒内只探一次公网**。
      iOS 现在每 10 秒本地挂一次就立刻探全部 6 通道 → **会把隧道打成筛子**（10s × 6 = 每分钟 36 次）
- [ ] 空档案跳过：`addresses.isEmpty && !hasEnabledRemote()` 直接不探（`194`）
- [ ] `lanRecovered` 局域网恢复沿：上轮不通→这轮通，用于**自动切回局域网**（远程兜着时也算离线态，`260-267`）
- [ ] `sameConnState` 变化检测：只在状态真变时发布（iOS 现在每轮都写 `states` 触发全量刷新）

### A.3 `ServerStore.java` / `JumpStore.java`
- [ ] `migrateLegacy` —— 老档案格式迁移（`ServerStore.java:76`）；iOS 需确认是否有等价迁移
- [ ] `sort(list)` 排序口径（`133`）：iOS 现有排序字段与顺序要逐条核对
- [ ] `rememberPassword(jumpId, password)` 的「勾了才存」语义（`JumpStore.java:90`）
- [ ] `trustedHostKey` 的读写与清除（iOS 用 `tofu_<host>` key，功能有，但**缺「重置信任」入口** —— 指纹变更的报错文案指向服务器详情页的按钮，现在页面上没有）

### A.4 `SshSessionManager.java`
- [ ] `terminalStartupLine(server)` 的 `cd` + shell 拼装与 `shQuote` 转义口径（iOS 有，需逐字比对）
- [ ] `releaseVia(entry)` 隧道释放时机（`121`）
- [ ] HostKey 校验：iOS 已实现 TOFU + SHA256 指纹，文案也比 Android 多一句「重置信任」，**保留**

### A.5 `TransferManager.java`
- [ ] 通知/前台服务在 iOS 无对应物 —— 用 `URLSession` 后台会话替代还是纯前台，需明确（Android 有前台通知，iOS 不做是合理降级，但要写进「不做」清单）

---

## 明确「不做」清单（防止再自由发挥）

- 不加 Android 没有的页面、弹层、动效、装饰
- 不改 Android 的文案（中文原文照搬）
- 不重排控件顺序、不改变信息层级
- 不引入 Android 没有的第三方视觉风格（现有 4 个 UI 库只用于 flex 布局与弹层能力，外观必须服从上面的令牌）
- iOS 系统特有的（如安全区域、手势返回）保留，但视觉结果要与 Android 一致

### 平台差异（**允许**不做的，需在代码注释里说明原因）
- Android 的 `configChanges` / `launchMode` / 多 Activity 栈 —— iOS 用 NavigationStack 等价
- Android 前台服务 + 传输通知 —— iOS 没有，传输仅前台运行
- Android `usesCleartextTraffic` —— iOS 用 ATS `NSAllowsArbitraryLoads`（已配）
- Android `ScanPairActivity` 锁竖屏 —— iOS 不锁（相机页全屏自适应）

---

## 附录 B · 审计发现摘要（2026-10-26）

| 级别 | 问题 | 影响 |
|---|---|---|
| A | `ServerEditForm.kt` 1891 行整个没进 TODO | 编辑表单只写了"换皮"，实际缺快速识别 / 地址池 / 令牌打码 / 折叠分区 / 区块超时 / 跳板机下拉 |
| A | `Common.kt` 563 行没进 TODO | 系统信息是完整弹层（含复制/分享/三态），`humanSize` 等格式化全缺 |
| A | `Icons.kt` 426 行没进 TODO | Android 是手绘矢量图标，iOS 用 SF Symbols → 形状不一致 |
| A | 两个 `androidTest` 没进 TODO | `(mid,kind,lane)` 幂等的两个回归用例没移植 |
| B | 顶栏 1.12 写错 | 实际是「标题+副标题+刷新+帮助+⋮」，没有「＋」和标签 chip |
| B | 统计 1.14 写漏 | 是**四段筛选器**（含「待生效」），不是三段统计 |
| B | `powerHint` 写错 | badge 六档规则 + 唤醒三态特判，不是「自动广播:9」 |
| B | 心跳缺远程降频 | 每 10 秒打 6 通道，公网隧道会被打爆 |
| B | WebView 缺 UA 标记 | 前端 `app.js:193` 认不出 App 壳，刷新行为不一致 |
| B | WebView 缺 `anyboot://reload` 拦截 | 下拉刷新点了没反应 |
| B | WebView 缺 `isCurrentAddress` 守卫 | 换地址后旧回调覆盖状态 |
| C | `removeTree` 缺 | 删非空目录直接失败 |
| C | `copy` / `move` / `exists` / `home` / `resolve` 缺 | 文件页复制剪切粘贴、重名策略、家目录全做不了 |
| C | `Entry.link` 软链接未解析 | 软链接被当普通文件，点进去行为错 |
| C | `ServerProfile` 缺 7 个方法 | 详见附录 A.1 |
| C | 「重置信任」入口缺 | 指纹变更后的报错文案指向不存在的按钮 |

---

## 阶段 12 · 收尾验证（2026-10-26）

全部阶段（0 → 11）已完成：`xcodebuild` 通过，契约测试 7/7 通过。

### 12.1 契约测试（替代 androidTest）

Android 的两个 instrumented 回归用例已移植，但**没有**放进 Xcode 的 unit-test target ——
本机 Xcode 26.6 上给工程加「依赖 App 的 test target」会让 SPM 预构建阶段失灵，
应用 target 直接报 `unable to resolve module dependency: 'Flow' / 'Crypto' / 'Citadel'…`
（`build` 与 `test` 都挂）。于是改成独立的纯模型包：

```
mobile/ios/Package.swift                     # AnyBootContracts（不进 App 构建）
mobile/ios/Contracts/Tests/AnyBootModelTests/
    PairUpsertTests.swift                    # ← PairUpsertTest.java（5 例）
    ApplyRemoteHintTests.swift               # ← ApplyRemoteHintTest.kt（2 例）
```

跑法：`cd mobile/ios && swift test` → `Executed 7 tests, with 0 failures`。
包里直接编译 `AnyBootRemote/Model/{RemotePair,ServerProfile}.swift`（不复制，避免口径漂移），
所以测的就是 App 跑的那份代码。

### 12.2 构建注意事项（踩过的坑，别重踩）

1. **每次 `xcodegen generate` 之后换一个干净的 `-derivedDataPath`**。工程 UUID 变了而旧包产物
   还在时，增量构建会在依赖扫描阶段报 `unable to resolve module dependency: 'SwiftASN1'`
   （swift-crypto 的 `_CryptoExtras` 找不到 swift-asn1）。这不是代码问题，是 DerivedData 状态不一致。
2. 验证用的稳定命令（**别加 `CODE_SIGNING_ALLOWED=NO`，原因见下**）：

   ```
   xcodegen generate
   xcodebuild build -project AnyBootRemote.xcodeproj -scheme AnyBootRemote \
     -configuration Debug -destination 'platform=iOS Simulator,name=iPhone 17' \
     -derivedDataPath /tmp/abdd4
   ```

   模拟器上用 Xcode 默认的 ad-hoc 签名（日志里是 `Signing Identity: "Sign to Run Locally"`），
   不需要 development team，也不需要描述文件 —— 但**必须签**。
   加了 `CODE_SIGNING_ALLOWED=NO` 的包「未签名」（`code object is not signed at all`），
   没有任何 entitlement，于是**每一次钥匙串访问都返回 `-34018 errSecMissingEntitlement`**：
   密码 / 私钥 PEM / 密钥口令全都存不进去，表现就是「编辑里存了密码，进终端还是每次弹
   输入框」。这个坑排查了很久：`KeyVault` 当初把 `OSStatus` 丢了，失败无声（已改成返回
   Bool + NSLog，见 12.7）。
3. 若在 Xcode 里报同样的模块错：`File ▸ Packages ▸ Reset Package Caches`，
   或删掉 `~/Library/Developer/Xcode/DerivedData/AnyBootRemote-*`。
4. **`error: Missing package product 'AlertToast'`（Xcode GUI 里 ⌘R 常见）**：
   DerivedData 被清过 / 工程重新生成过之后，Xcode 不会自动重解析 SPM，于是任何一个
   三方包都可能报「Missing package product」（AlertToast / PopupView / Citadel 随机出现）。
   命令行一条修好，不用点 GUI：

   ```
   cd mobile/ios && xcodebuild -resolvePackageDependencies \
     -project AnyBootRemote.xcodeproj -scheme AnyBootRemote
   ```

   真正原因在 `xcactivitylog` 里（`~/Library/Developer/Xcode/DerivedData/AnyBootRemote-*/Logs/Build/`，
   gzip 后 grep `error:` 就能看到），Xcode 弹窗只给一句「Build failed」。
5. 真机不想走 GUI 时（命令行 ≈ ⌘R）：

   ```
   xcodebuild build -project AnyBootRemote.xcodeproj -scheme AnyBootRemote \
     -configuration Debug -destination 'platform=iOS,id=<UDID>' -allowProvisioningUpdates
   xcrun devicectl device install app --device <UDID> <DerivedData>/Build/Products/Debug-iphoneos/AnyBootRemote.app
   xcrun devicectl device process launch --device <UDID> --terminate-existing com.anyboot.AnyBootRemote
   ```

### 12.3 调试直达钩子（仅 Debug，`UI/DemoSeed.swift`）

```
-ABSeedDemo                       # 种样例：1 台电脑（3 条远程配对）+ 2 台服务器 + 1 把密钥
-ABSeedHost / -ABSeedToken        # 电脑地址与令牌
-ABSeedJumpHost                   # 第一台服务器地址（黑洞 IP 可让终端页停在「正在连接 …」）
-ABSeedPwdHost                    # 第二台（密码认证那台）地址；指到 127.0.0.1:22 就能用本机 sshd
                                  # 走真实握手，验收 TOFU 指纹弹窗 / 密码策略
-ABSeedSavedPassword=1            # 给第二台预写钥匙串密码（不写则先进应急密码框）
-ABPage=computer|computers|servers|about
-ABServerPage=servers|keys|files|sessions|transfers
-ABTerminal=<jumpServerId>        # 直接打开终端页
-ABDrawer=1                       # 启动后自动展开侧边栏（查抽屉渲染用，绕过点击）
```

### 12.4 逐格截图比对结果（iPhone 17 模拟器 / iOS 26.5）

| 设计格 | 页面 | 结论 |
|---|---|---|
| servers | 服务器页 | ✅ 顶栏五件套、卡片（图标块 / 名称+协议 / 地址 mono / 右侧 ms + 状态点）、底部四项导航一致 |
| keys | 密钥页 | ✅ 卡片（图标块 / 名称 +「N 台使用」/ 算法·指纹）+ 虚线提示一致 |
| files | 文件页 | ✅ 服务器下拉、路径栏、工具行（flex 换行）、列头、底部传输条一致 |
| file-menu | 文件动作面板 | ⚠️ 未截图：需要真实 SFTP 会话（模拟器没有可连的 SSH 主机），代码路径与 `FileActionSheet` 一致，待真机复核 |
| term | 终端页 | ✅ 顶栏（‹ + 32pt 图标块 + 名称·状态 + 🌓 / Aa / ⌨）、xterm 区、键栏 8+4 一致 |

补充核对：电脑列表（亮 / 暗）、会话页、传输页、关于页 均过了一遍，暗色主题文字全部可读。

### 12.5 顺带修掉的七个 bug

1. **文件页报错文案漏出 Swift 反射**：`"\(error)"` 会把自定义错误打成
   `message("私钥读取失败…")`。`SftpOps.reason/describe`、`FilesStore.message(of:)`、
   `TransferCenter.reason` 统一改为先取 `LocalizedError.errorDescription`。
2. **电脑卡片通道行把尾巴挤掉**：SwiftUI 默认按比例压缩，Android/Compose 是「非 weight 子项先量、
   weight 子项吃剩余」。给通道徽章 / 通道名 / Funnel / 来源 / 复制 / 打开加 `layoutPriority`，
   由地址那一格吸收省略（与 Android 一致）。
3. **终端页首帧呈现不出来**：`fullScreenCover` 在首次 body 之前置位会被丢掉，调试钩子改成
   一拍之后再置位（用户点「终端」本来就晚于首帧，不受影响）。
4. **顶栏图标点不动（☰ 呼不出侧边栏）**：两个原因叠在一起。
   - `ServerModuleView` 的 ☰ 改的是 `ServerModuleStore.drawerOpen`，而抽屉组件读的是
     `ABNavigator.drawerOpen`，两套状态互不相干 —— 点下去只渲染出一个空的 GeometryReader。
     抽屉本来就由 `RootView` 在根上统一挂一份，已删掉服务器页那份和 `store.drawerOpen` 字段。
   - 命中区过小。`ABBarIcon` 只用 padding 撑开，Text 的透明内边距不进命中区；实测
     ☰ 的可点范围：服务器页 **15×20pt**、电脑页 **22×22pt**、电脑列表 42×42pt。
     手指按视觉图标的位置点，稍微偏一点就落空。统一改成 `.frame(44,44) + .contentShape(Rectangle())`
     （对齐 Android `BarIcon` 的 44dp 图标块），电脑页 ☰ 顺带补 `accessibilityLabel("侧边栏")`，
     `＋` 补 `contentShape`。改后三页实测均为 49×49，点框左上角（离字形 4pt）也能打开。

   验证手段：`mobile/ios` 之外的独立 UI 探针工程（不依赖 App 的 SPM 图），用
   `XCUIApplication(bundleIdentifier:)` 驱动已安装的包，按 accessibilityLabel 找按钮、
   按归一化坐标点命中框四角。上面三组数字都是它量出来的。
5. **抽屉里「电脑」和「电脑列表」指反了**。`ABPage` 的 case 名跟直观读法相反
   （`.computers` = 电脑页/多标签、`.computer` = 电脑列表），抽屉照名字填，结果
   「电脑」跳去了列表页、「电脑列表」跳去了电脑页。已按 `Drawer.kt:116-129` 对调：
   「电脑」→ `.computers`（DesktopIcon）、「电脑列表」→ `.computer`（ServerStackIcon）。
   同时修掉两处连带问题：
   - **启动页**：Android 的 launcher 是 `MainActivity`（电脑页），iOS 原来默认
     `.computer` 落在列表页，改成 `.computers`（`previous` 初值同步改）。
   - **列表页 ⋮ →「电脑列表」**：原来写成 `nav.go(.computers)`（跳到电脑页），
     Android 那边是弹一个「服务端列表（点一台进行编辑）」对话框（`:492-556`，
     点一台直接进编辑、每台把所有 IP 都亮出来、底部「添加 / 关闭」）。已补
     `serverListDialog` 并对齐文案。
6. **服务器各页的 ⋮ 菜单被页面内容盖住**（用户反馈「三个点都被挡住」）。菜单是用
   `.overlay(alignment: .topTrailing)` 挂在顶栏 ⋮ 按钮上的，而顶栏是 `VStack` 的
   **第一个**子视图 —— SwiftUI 按书写顺序绘制，后画的页面内容（搜索框、服务器卡片、
   列表）整块画在菜单之上。实测菜单只在顶栏下沿露出 13pt 一条白边，点菜单项等于点在
   下面的列表上（探针里点「传输管理」不翻页）。修法三条：
   - `topBar` 加 `.zIndex(10)`，抬到页面内容之上（这一条修好 90% —— 5 个页面共用同一个顶栏，
     所以「所有页面」一起好）；＋ 的「生成密钥 / 导入私钥」菜单同理受益。
   - 页面区加一层透明遮罩，点空白处收起菜单（对齐 Android `Popup` 的 outside tap 关闭），
     `onChange(of: store.page)` 时也收起，避免切页后菜单还挂着。
   - 顺带把命中区补到 44×44：⋮ 原来是 `Text("⋮") + padding`，实测 accessibility frame
     **仅 7×20pt**（和 12.5.4 的 ☰ 同一个病），改成设计系统手绘 `DrawnIcon.more` + 44 框；
     齿轮 20×20 → 44×44；＋ 可见圆仍 36，外框补到 44。三处都加了 accessibilityLabel。
   验收（探针 + 主机侧连拍）：`PROBE_dot_frame=(283,75.7,44,44)`、
   `PROBE_navigated_to_transfers=true`，截图里菜单完整浮在搜索框和卡片之上。
7. **终端页 Aa 字号菜单同样被终端区盖住**（同一天用户接着反馈）。完全同一个病根：
   `TerminalPage` 的 `topBar` 是 `VStack` 第一个子视图，Aa 的菜单挂在它按钮的 `.overlay` 上，
   被后画的终端 `ZStack`（不透明底色 + xterm WebView）整块盖住。同样三条处理：
   `topBar.zIndex(10)`、终端区加透明遮罩点一下收起（顺带避免这一下漏进 WebView 误敲字）、
   Aa 命中区 `padding` → `.frame(44,44)+contentShape` 并补 `accessibilityLabel("字号")`。
   验收：`PROBE_aa_frame=(300,73.7,44,44)`、菜单开→`放大字号（当前 15）`、点完变 16、
   点终端区菜单收起，`TEST SUCCEEDED`。
   **探针踩坑**：终端的第一次合成点击会被系统吞掉（XCUITest 让 App 变 key window 的那一下），
   表现为「点了没反应」，第二次才生效 —— 探针里要先点一次热身。真机在前台没有这个现象。

### 12.10 跳板机端到端验收 + 编辑回显加固（2026-10-26 晚）

**跳板机此前一直没有端到端测试**，本轮补上（全部真握手，非 mock）：

环境：本机 user-mode sshd（127.0.0.1:2222，公钥认证）当跳板机 +
Python HTTP 服务（127.0.0.1:18081）模拟电脑端 AnyBoot 服务。
调试钩子 `-ABSeedViaJump=1`：种出来的电脑档案关掉局域网通道、清空远程配对、
`jumpServerId=srv-live`，逼探测与加载**只能**走 jump 通道。

```
PROBE_TOFU=确认主机指纹            ← 首连跳板机弹指纹确认
PROBE_TUNNEL_HIT=true             ← 心跳经 direct-tcpip 隧道探到 /health（服务端 access.log 有记录）
Test Case 'test2_jumpTunnelEndToEnd' passed
```

跳板机在 iOS 上的生效链路（本轮逐一核对过代码）：心跳探测
（`ProbeEngine.probeOne` → `tunnelAddress`）→ 电脑页 WebView 加载
（`ComputerTabs.openViaJump`）→ 电源广播（`WakeOnLan` 经跳板机跑 python 发魔术包）→
系统信息（`SysInfo` 走隧道端点）。隧道实现是 NWListener + `createDirectTCPIPChannel`。

**编辑回显**（用户报「电脑/电脑列表点编辑有时候回显为空」）：

- 加固：`ComputerEditorView` 的呈现从 `.sheet(isPresented:)` 改为
  `.sheet(item: $editorTarget) { ComputerEditorView(profile: target.profile).id(target.id) }`
  （两个页面共 7 处调用点）。`@State` 初始值只在视图身份第一次建立时生效，
  旧的 isPresented 写法身份是复用的，存在「沿用上一次表单状态」的理论风险；
  现在每次呈现都发新身份（`ComputerEditorTarget`，每次全新 UUID），init 的回显一定生效。
- 表单字段补 `accessibilityIdentifier`（editor.group / editor.name / editor.address /
  editor.token），以后探针可以直接读字段值。
- 自动化验收（UI 探针 4 条，全过）：
  ① 列表页 ⋮ → 添加(空) → 取消 → 编辑 → 地址回显 ✓
  ② 电脑页长按标签 → 编辑这台电脑 → 地址回显 ✓
  ③ 编辑 → 保存 → 立刻重开 → 回显是保存后的值 ✓
  ④ 跳板机端到端（上面那条）✓
- **诚实备注**：A/B 对照把列表页临时还原成旧写法跑同样的探针，也通过了 ——
  即「常规路径」下新旧写法回显都正确，「有时候为空」在自动化里没能复现。
  加固仍然保留（消除这一类风险）。若真机上再遇到回显为空，记录一下入口
  （长按菜单 / ⋮ / 错误面板「修改这个服务端」）与哪个字段为空，探针可以精确复现。

### 12.6 仍未验证（需要真实环境）

- 真实 SSH / SFTP：终端交互、文件列表与动作面板、传输进度与续传
- 真实局域网：探测赢家判定、引导项读取与切换、电源（魔术包 / HTTP 兜底）
- 抽屉手势与多标签电脑页的 WebView 保活
- 真机：相机扫码页（模拟器没有摄像头）

### 12.7 密码保存策略（与 Android 逐条对齐，2026-10-26）

Android 侧的现状（6786caf 之后）一共四条，iOS 现在完全照此办理：

| # | 策略 | Android | iOS |
|---|---|---|---|
| 1 | 编辑服务器即可存密码，「保存密码 / 凭据」默认勾选，取消勾选 = 清除已存 | `persistJump` → `KeyVault.encrypt` → `passwordEnc` | `JumpEditSheet.persist()` → `KeyVault.save` → `passwordRef` |
| 2 | 只在「密码认证 && 本机拿不到密码」时才弹应急输入框 | `needsManualPassword` | `SshSessionManager.needsManualPassword` |
| 3 | 应急框「保存密码」默认勾选，输一次即落盘，下次免输 | `ManualPasswordDialog` → `JumpStore.rememberPassword` | `ManualPasswordBody` → `JumpStore.rememberPassword`（本轮新增，逐行对齐） |
| 4 | 落盘失败不静默：明确提示，且绝不写明文 | 编辑框 Toast 拒绝保存 | 编辑框 Toast 拒绝保存；应急框 Toast 但仍继续本次连接 |

**老版的「只放内存、不落盘」那个弹窗等价物已彻底删掉**：现在只有一份
`Servers/ManualPasswordBody.swift`，终端页与服务器页共用（此前有两份复制粘贴的实现）。

顺带修的三个真问题：

1. **`KeyVault` 把 `OSStatus` 丢了** —— `save` 不返回结果、失败连日志都没有。现在
   `save` 返回 Bool 并 `NSLog` 失败码，`load` 也记录非 `errSecItemNotFound` 的异常。
   没有这一步，`-34018` 会一直隐身（见 12.2）。
2. **未签名包 = 钥匙串全挂** —— `CODE_SIGNING_ALLOWED=NO` 构建出的模拟器包里
   所有 Keychain 调用返回 `-34018`，保存密码永不生效。给用户的构建命令已改（12.2）；
   若以后再遇到，看控制台的 `[ABVAULT] save 失败 account=… status=…`。
3. **连接失败把 Citadel/NIO 的英文内部报错直接甩给用户** ——
   `The operation couldn't be completed. (Citadel.ClientHandshakeHandler…Disconnected error 1.)`。
   新增 `SshSessionManager.describe(_:)`：先按 `SSHClientError` 枚举判（
   `allAuthenticationOptionsFailed` → 「认证失败：用户名或密码不正确」），再按文本兜底
   （拒绝 / 超时 / 不可达 / 域名解析 / 未建立），原始信息只进 `NSLog`。
   终端页与「测试连接」都改走它。

验收（UI 自动化，`/tmp/abprobe-src` 独立探针工程）：
```
### 第一次进入：弹了输入框 = true          # 没存密码 → 弹，且「保存密码」默认勾选
### 第二次进入：还弹输入框 = false         # 输完密码保存后重开 → 不再弹
### 第二次进入：进到连接流程 = true → 连接失败：…
Test Case 'testSavedPasswordSkipsPrompt' passed
```

调试钩子新增 `-ABSeedSavedPassword=1`：给样例里那台密码认证的服务器预写一条
钥匙串条目，用来验证「存了就不再弹」。

### 12.8 TOFU 主机指纹弹窗（2026-10-26 修）

**症状**：第一次连任何一台服务器（`tofu_<host>` 没有记录）时，弹窗根本不出现，
直接报「用户取消了主机指纹确认」。

**真因**：`SshSessionManager.confirmHandler` 的注释写着「由 UI 层注入」，
但整个工程**没有任何一处赋值**。首连时 `await manager.confirmHandler?("确认主机指纹", msg)`
对 nil 求值得 nil，`== true` 为假，于是走拒绝分支。编译期完全看不出来 —— 这类
「可选闭包 + 可选链」的漏接线，靠读 diff 也容易漏，所以这次直接把它做成默认实现：

1. `SshSessionManager.init()` 里给 `confirmHandler` 兜底接 `ABConfirmBus`，
   不再依赖任何外部注入。
2. `ABConfirmBus` 从「根视图上的 SwiftUI `.alert`」改成 `UIAlertController`
   挂在**当前最上层 view controller** 上。原因：终端页是 `fullScreenCover`，
   挂在根视图的 `.alert` 在盖子之上弹不出来（UIKit 只打 already presenting 的警告，
   SwiftUI 静默失败），弹不出来就没人 resume，握手会永远停在「正在连接 …」。
   现在全屏盖 / sheet 之上都能弹；主题跟 `ui_theme` 三挡（`overrideUserInterfaceStyle`）。
3. `SshError` 新增 `.hostKey(String)`：没确认 / 指纹变了 / 确认框起不来单列一档，
   文案直给，并且**不计入连接退避** —— Android 那边 `JSchException`（含指纹拒绝）
   也不退避；否则点「取消」后 2 秒内重连会先撞「X 秒后再试」。
4. 弹窗文案对齐 Android `TofuRepository.check`：标题「确认主机指纹」、
   正文「首次连接 user@host + 主机指纹（SHA256）：… + 确认是你要连的服务器吗？」、
   按钮「信任并连接 / 取消」，点外面不关。指纹键仍是 `tofu_<host>`（`jump.host` 原样），
   跟 Android `JumpStore.trustedHostKey(server.host)` 同键。

**验收**（独立 UI 探针工程 `/tmp/abprobe`，`XCUIApplication(bundleIdentifier:)` 驱动已装包；
本机 sshd 是真主机，`-ABSeedPwdHost=localhost:22` 走真实握手）：

```
PROBE_ALERT_LABEL=确认主机指纹
PROBE_ALERT_TEXTS=[… "首次连接 jiang@localhost:22\n\n主机指纹（SHA256）：SHA256:/6epp…\n\n确认是你要连的服务器吗？…"]
Test Case 'test1_cancelKeepsNothing' passed      # 取消 → 弹窗消失、退回列表（不卡「正在连接」）、指纹不落盘
Test Case 'test2_acceptThenSilent' passed        # 信任 → 握手继续（演示密码错 → 认证失败）→ 重开不再弹
```

注：探针工程里的 xcodegen 是裸二进制（没有 SettingPresets），`project.yml` 必须显式写
`PRODUCT_NAME` / `PRODUCT_BUNDLE_IDENTIFIER`，否则报 `Multiple commands produce '…/.app'`。

**遗留**：指纹变更的报错文案指向服务器详情的「重置信任」，这个入口仍未做（见附录 B 最后一行）。

### 12.9 终端永远停在「正在连接 …」（2026-10-26 修）

**症状**：首连点了「信任并连接」之后，状态一直停在「正在连接 jianghh@…」，终端区一片空，
不报错、不超时，退出重进也一样。

**真因**：`SshSessionManager.openShell` 直接 `try await client.withPTY(request) { … }`。
Citadel 的 `withPTY` 会一直 `await` 它的 `perform` 闭包，而闭包里是「读到通道关闭」的循环 ——
也就是说 **`withPTY` 要等整段会话结束才返回**。于是 `TerminalSessionRef.startSsh()` 永远不返回，
`TerminalPage.ensureSession` 卡在 `await` 上：`statusMessage` 一直是「正在连接 …」，
`TerminalSessionsRegistry.put()` / `bind()` 一次都没跑过。PTY 其实早就建好了、提示符也吐出来了，
全在 `TerminalSessionRef.backlog` 里没人取。日志上很直观：

```
21:24:47.742 [ABSSH] PTY 已建立 host=127.0.0.1:2222 80x24   ← 通道真的开了
（bind 日志迟迟不出现 —— 修复前压根没有这行）
```

**改法**（`SshSessionManager.openShell`）：

1. `withPTY` 甩进后台 `Task`，句柄先返回；新增 `OneShotGate`（一次性闸门）在「闭包刚开始执行」
   ＝ PTY 已被服务器接受的那一刻 `succeed()` 放行上层。
2. 闸门带 15s 超时（服务器不给 PTY 时不会又变成无限等）。失败走 `SshError.message`，
   文案「打开终端超时：服务器没回应 PTY 请求（可能禁用了 shell）」。
3. 后台任务在闭包返回后 `ready.fail(...)`：闸门没开过 → 报「终端通道刚建立就关闭了」；
   已开过 → 按「会话已结束：<人话原因>」回调，界面不会卡死。
4. `ShellHandle` 多存一份 `lifetime` 任务句柄（`cancel()`），`TerminalView` 侧顺序也对调了：
   **先挂下行监听，再 `attachAndDrain()` 取积攒回显** —— 反过来的话两步之间的输出两头落空。

**验收**（本机起了一个 user-mode sshd：`/usr/sbin/sshd -f /tmp/abssh/sshd_config`，公钥认证 + PTY；
App 侧用新钩子 `-ABSeedKeyPEM=<base64 PEM> -ABSeedKeyHost=127.0.0.1:2222 -ABSeedResetTofu=1`）：

```
PROBE_ALERT=确认主机指纹
PROBE_CLEAR_SECONDS=0.9            # 点「信任并连接」后 0.9s，「正在连接」消失
Test Case 'testLiveTerminalConnects' passed
```

服务端日志同步印证：`Starting session: shell on ttys001 for jianghh`；
设备日志：`[ABSSH] PTY 已建立` 之后 **20ms** 就打出 `[ABTERM] bind jump=srv-live 已连接=true`。
截图里终端正常显示 `Welcome to fish…` 与提示符。

**顺带补的诊断**（都只在 DEBUG 生效）：`TerminalView` 注入 `window.onerror` 把 JS 异常投给原生打
`[ABTERM] JS 异常: …`；`didFinish` 里再打一行 `[ABTERM] JS 状态={"term":"object",…}`。
以后再遇到「终端空白」，先看这两行就知道是 JS 没起来还是数据没进来。

**新增调试钩子**：`-ABSeedKeyHost` / `-ABSeedKeyUser` / `-ABSeedKeyPEM`（base64 私钥）/
`-ABSeedResetTofu=1`（清掉 `tofu_<host>`，强制重走首连确认）。

### 12.11 终端不能用第三方输入法（2026-10-26 根治）

**症状**：终端里只有默认 iOS 输入法能打字；切到搜狗/百度/Gboard 或中文 IME 就
按键没反应、候选词丢失。其他页面（原生 SwiftUI 输入框）都正常 —— 说明不是 App
禁了键盘，是终端的输入通路问题。

**真因**：终端是 WKWebView 跑 xterm.js，xterm 靠一个**屏外隐藏 textarea**
（`.xterm-helper-textarea`）「偷」键盘事件转字节流。系统键盘走 WebKit 的
keydown/composition 事件链恰好能用；第三方输入法依赖真实文本框的几何信息与
marked text 流程，在这种屏外小框里事件序列对不上。这是 WebKit + xterm.js 的
固有缺陷，Android WebView 同病。

**修法**（原生输入中继，Termius 等专业终端同款思路）：

1. 新增 `TerminalInputField.swift`：隐藏 UITextField（alpha 0.01、1×1pt、
   `autocorrection/spellChecking/smartQuotes` 全关），作为**键盘宿主**。任何键盘
   （含第三方 IME）都走 iOS 原生 UITextInput 协议；字段永远为空，只负责收。
2. `TerminalView` 改为返回 `TerminalHostView`（WKWebView + 输入域）。焦点全部改道：
   点终端区（tap 手势挂**宿主**上、`cancelsTouchesInView` 防 WebKit 抢焦点、
   delegate 并行识别）、⌨ 按钮（现在是真「弹出/收起」开关，以前只弹不收）、
   「自动弹键盘」设置 —— 三处都把 first responder 交给输入域，不再碰 WebView。
3. 字节转发要覆盖 **三条提交入口**（实测 iOS 26.5 上逐键文本走的是委托路径，
   不经过可重写的 `insertText`！）：
   - `textField(_:shouldChangeCharactersIn:replacementString:)` —— 逐键主通路，
     转发 replacement + 返回 true + 异步清空字段（`markedTextRange == nil` 才清，
     不打断组词）；
   - `insertText(_:)` / `replace(_:withText:)` —— IME 提交、程序化插入；
   - `textFieldShouldReturn` —— 直发 CR；**组词未提交时先把 marked text 原文补发**
     再发 CR（iOS 默认会把组词整段丢掉，实测只发出一个 CR）；
   - `deleteBackward` —— 组词中交给 super（删拼音字母），否则发 DEL(0x7f)。
   字节经串行队列（保键序）→ `TerminalSessionRef.write`（自带锁）→ 会话。
4. 键盘外观跟终端主题走（暗色终端配 `.dark` 键盘）。

**验收**（`/tmp/abprobe` `InputProbe` 5 用例全过；本机 sshd:2222 真连接）：

```
test1 键盘逐键按序到中继（0-9）                    passed
test2 退格=DEL(0x7f)、回车=CR                      passed
test3 IME 提交（insertText 钩子）→ 远端 fish 执行 touch → 宿主机 marker 生成  passed
test4 键盘收起后点终端区再弹出（tap 手势）          passed
test5 字母逐键按序到中继（nihao）                  passed
```

可观测性：DEBUG 下中继经手的每个字节追加到 `/tmp/abterm_input.log`
（模拟器 App 直写宿主机 FS，探针直接断言字节级内容）。
新调试钩子 `-ABTermType=<命令>`：会话就绪后模拟一次 IME 提交（走 insertText 同一入口）。

**探针踩坑**（下次别再踩）：
- XCUITest `typeText` 走硬件直插路径，**不进 IME 组词** —— 模拟「IME 提交」
  只能用 `insertText` 钩子，别指望敲拼音+空格。
- 输入日志每键一行（时间戳前缀），断言要按「字符都出现且次序递增」，
  直接 `contains("0123456789")` 永远 false。
- UITapGestureRecognizer 挂在 WKWebView **上**时合成点击从未触发过；挂到
  宿主容器（祖先视图）+ 并行识别 delegate 就好了。
- 探针工程新增/删除用例后要重新 `xcodegen`，否则 `-only-testing` 空跑还显示 passed。

**真机**：已构建安装（Personal Team 签名链路照旧）。第三方输入法的实际手感
（搜狗候选条、滑行输入）需要真机人工验 —— 自动化只能证到 UITextInput 协议层。

**遗留**：xterm.js 的组词预览没了（原来 WebKit 会在光标处画组词串）—— 现在组词
内容只显示在输入法自己的候选条里，提交后终端才见字。这是字节流终端的固有取舍，
Termius 同款行为。

### 12.12 跳板机「终端能通、探测/通信不通」（2026-10-27 修，真机用户报障）

**症状**：终端能 ping 通/能连上目标地址，但「网络探测走跳板机」与「通信走跳板机」
都不行 —— 跳板机 seemingly 没转发任何请求。

**三个叠着的真因**（模拟器此前验收通过是运气好，没踩到）：

1. **`Heartbeat.tunnelResolver` 从未注入**。探测层设计为「不碰 SSH 栈，由 App
   注入隧道解析器」，但全工程没有任何地方赋值 → resolver 恒 nil →
   `probeOne` 报「没配跳板机解析器」并静默失败。心跳的 10s 探测从来就没走过
   跳板机；而电脑页预览/加载走 `BootService.localEndpoint → tunnelAddress`
   不经 resolver，所以**通信通、探测不通**。修复：`AnyBootRemoteApp.init`
   注入 `tunnelResolver = { try await SshSessionManager.shared.tunnelAddress(for: $0) }`。

2. **`TunnelBridge` 数据方向用反了**。TCP→SSH 必须出站 `context.writeAndFlush`
   （流向 head 的 NIOSSH 才会发 CHANNEL_DATA）；之前用 `fireChannelRead`
   （入站方向）→ 数据从 pipeline 尾部直接漏掉 → 「通道开得开、sshd 也连上
   目标，但一个字节都过不去」。同时 `NWConnection` 必须 **先 start 再开
   SSH 通道**（桥在 handlerAdded 里就发 receive，start 之前 receive 无效）。

3. **`ABConfirmBus` 并发确认互相残杀**。resolver 修好后心跳+预览+终端会**同时**
   连跳板机 → 多个 TOFU 确认并发 → 旧实现「新请求把 in-flight 当取消」→
   连环互相取消，全部报「用户拒绝主机指纹」。重写：同内容共享一次弹窗
   （答一次全体生效），不同内容排队串行弹。

**顺带加固**：
- 隧道缓存带 `clientID`（ObjectIdentifier），SSH 会话重连即作废旧隧道自动重建
  （真机锁屏/切网后旧隧道绑死连接的问题）；
- `openTunnel` 建好后**自检一次 direct-tcpip**：服务器禁转发（AllowTcpForwarding no）
  或目标不可达时当场报真实原因，不让 HTTP 请求静默超时；
- 泵转发失败打 `[ABSSH]` 日志，不再静默吞。

**验收**（真实 sshd + 真实 HTTP，探针 TunnelProbe）：
- test1 基线探通（`/health` 经跳板转发命中）→ 宿主机杀 sshd 重启（模拟掉线）→
  会话重建后新 hit 自动出现 → **通过**；
- test2 2223 端口禁转发 sshd：探测不出 hit、报真实原因 → **通过**；
- 回归 EditorProbe 4/4（含 `PROBE_TUNNEL_HIT=true`）、InputProbe 5/5、FontProbe ✓。

**注**： ATS 拦截 `http://100.64.0.7`（demo TS 直连配对）是 Info.plist 例外
已配仍报错 —— 疑似模拟器行为，待真机复核；不影响跳板链路（隧道地址是
127.0.0.1 回环）。


### 12.13 电脑/电脑列表「添加/编辑」未按安卓复刻（2026-10-27 重写）

用户报「电脑和电脑列表的添加和编辑没有完全按照安卓布局以及样式来复刻」。对照
Android `ServerEditForm.kt`（ServerEditFormBody）与 `ComputerListActivity.EditSheet`
逐段核对，iOS 旧 `ComputerEditorView` 缺一大半能力。整文件重写（`UI/ComputerEditorView.swift`）：

**外壳**（对齐 EditSheet）：底部 sheet（大档位、自绘把手 36×4）→ 标题行
「添加电脑/编辑这台电脑」15pt Bold + ✕ 关闭圆钮（30pt elevate）→ 表单滚动区
（≤62% 屏高，与 Android 同算法）→ 分隔线 → 底部动作栏「删除(红框,仅编辑) + 全宽
保存(40pt accent 圆角11)」。导航栏/系统「取消/保存」文字钮全部移除。

**表单主体**（对齐 ServerEditFormBody，顺序/文案/配色逐项对应）：
- 电脑名称（去掉 iOS 多出来的「分组」字段 —— 两端页面都不消费 group）
- **快速识别**：多行粘贴框 + 「识别并加入」+ 提示语（iOS 原本没有）
- **已添加的地址**：结构化卡片（地址 + 「连接用」徽标 + ✕；MAC 行逐条 ✕ /
  「没配 MAC」提示）（iOS 原本是整段 textarea）
- **手动添加**：地址 + 多 MAC + 添加（原本没有）
- token：**已存令牌回显（打码 + 显示/隐藏）** + 替换框（原本没有回显）
- 远程配对：粘贴即时分拣（onChanged，无独立「添加」钮）+ 📷 扫码 + 全部清空 +
  **三区块常驻卡片**（未配/N 条 + 「＋ 添加」手填空行）+ 条目行
  （开关 · 地址可编辑 · ↑↓✕ 30pt 小钮 · lane 徽标 · 来源 · 行内超时小框 ·
  令牌打码/改看/复制地址）
- **连接策略折叠区**（默认收起 + 摘要行）：通道行（☰ 名称/副标 + 行内超时
  [仅 lan/jump，其余「超时见区块设置」] + ↑↓ + 开关）、两个粒度 + 区块内部策略
  （三选项 radio 行，Android ProbeModeChoice 同款）、经由跳板机下拉（直连默认 +
  全部跳板机，Telnet 带「不能做隧道」后缀）、默认探测超时
- **局域网电源路由折叠区**（默认收起 + 未配置/N 条）：路由卡（动作菜单[安卓同序
  唤醒/重启/关机/睡眠] + 删除、模式菜单、广播包区[端口/广播地址/MAC 选择
  (全部网卡 checkbox)/包数/重试（占位显示动作默认值，0=默认）]、HTTP 说明）
- 换动作时包数/重试跟随默认值重置（0 = 按动作默认）等行为细节同 Android

**保存语义**：结构化地址 → replaceParsed；token 空则从 pastedRaw 再挖；
routeTimeoutsMs 只收 lan/jump 单独设过的；区块默认超时（秒→毫秒）与区块探测
策略（默认值不落盘）；空地址行丢弃；空地址保存 → toast「请填电脑地址」。
兜底：快速识别粘了没点识别 → 保存时自动认一次（兼容老操作习惯与探针）。

**探针更新**（/tmp/abprobe EditorProbe）：取消→关闭圆钮；地址回显断言从
textarea 值改为「地址卡片 staticText 前缀」；test4 增加名称回显断言（含「改名机」
即可 —— 种子名「黑苹果」被追加是正确行为）。4/4 通过（含跳板机端到端回归）。

新包已装真机。待真机人工复核：折叠区观感、地址卡片增删、令牌打码切换、
MAC 选择、电源路由菜单。
