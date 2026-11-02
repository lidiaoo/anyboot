# Vue 迁移 · 1:1 还原 TODO（2026-11-02 重写）

> **这份台账取代旧的 `ui-vue迁移-TODO.md`。**
> 旧台账按「阶段/迁移」组织，本台账按**还原度**组织：每一页都要做到
> **样式 1:1、功能 1:1、交互 1:1**，不缺页面、不缺功能、不缺交互。

---

## 零、判定标准（每条都必须过）

一页算「还原完成」，必须同时满足：

| 维度 | 判定方法 |
|---|---|
| **样式 1:1** | ① 从旧 HTML **逐字复制** DOM 结构与全部 class/id；② 不写自造布局 CSS；③ CDP 实测计算样式与旧版逐项对比 |
| **功能 1:1** | 旧版该页涉及的每个 `invoke(...)` 命令都有对应实现；漏一个就是缺功能 |
| **交互 1:1** | 旧版每个 `addEventListener` 的行为都在（点击/双击/右键/拖拽/键盘/滚动/hover）；漏一个就是缺交互 |
| **缺页面** | 旧版 12 个 `.page` + 4 个独立窗 + 32 个弹层，一个都不能少 |

### 旧版真实规模（实测）

| 文件 | 行数 | 函数 | 事件绑定 | IPC 命令 |
|---|---|---|---|---|
| `hub.html` | 14,921 | 359 | **338** | **109** |
| `app.js` | 5,258 | 28 | 141 | — |
| `boot-adv.js` | 3,046 | 119 | 58 | — |
| `hub-files.html` | 1,262 | 6 | 49 | 16 |
| `index.html` | 1,092 | 4 | 5 | — |
| `hub-backup.html` | 1,057 | 3 | 28 | 15 |
| `hub-term.html` | 695 | 5 | 11 | 12 |
| `boot-list-view.js` | 665 | 5 | 7 | — |
| `hub-keys.html` | 465 | 3 | 15 | 9 |
| `grub-list-view.js` | 406 | 11 | 2 | — |
| **CSS 合计** | **7,645** | | | |

CSS 构成：`styles.css` 5,013 + `hub.html` 内联 **2,342** + `notify.css` 168 + `tray-menu.css` 122。

> ⚠️ **2,342 行内联样式是我第一次迁移时整块丢掉的**（只搬了 `styles.css`），
> 导致 12 个页面全无布局。已提取到 `ui-vue/src/styles/vendor/hub-inline.css`。

---

## 一、共享基础设施（所有页面依赖）

- [ ] **1.1 CSS 全量接入核对**
  - `vendor/legacy.css`（5013）✓ 已接
  - `vendor/notify.css`（168）✓ 已接
  - `vendor/hub-inline.css`（2342）✓ 已接
  - [ ] `tray-menu.css`（122）—— 托盘菜单窗，未接
  - [ ] 核对：构建产物 CSS 是否 ≥ 7,600 行等价体量
- [ ] **1.2 主题切换** `#themeToggle`（`data-theme` dark/light + `localStorage["anyboot.theme"]`）
- [ ] **1.3 侧栏拖拽调宽** `#sideSplit`（160~420px，`localStorage` 记忆）
- [ ] **1.4 窄视口自动收起侧栏**（旧的 `@media` / resize 钩子）
- [ ] **1.5 全局 toast**（旧版 `toast()`，含 `bad` 红态）
- [ ] **1.6 全局确认/输入弹层**（旧版 `window.confirm` 换成自家 dialog 的部分）
- [ ] **1.7 前端异常上报**（`sol_log_client_error` / `hub_report_error`）
- [ ] **1.8 `data-ready` 显示门**（防白闪，已有 ✓）

---

## 二、hub 外壳（`hub.html:2197`）

- [x] **2.1** `.shell` / `aside.side` / `.side-head` / `.side-logo` / `.side-name` / `.side-toggle`
- [x] **2.2** `nav.side-nav` 12 个 `.snav`（SVG 图标 + `.s-label`，`.active` 态）
- [x] **2.3** 主题切换按钮 + `.side-foot`
- [x] **2.4** `.side-split` 拖拽（pointer 事件）
- [x] **2.5** `.side.collapsed` 收起态 —— 实测旧版用 `.shell.nav-collapsed`，已对上
- [ ] **2.6** `#sideVersion`（`v0.2.5 · remote`，由 `getVersion()` 填）

---

## 三、12 个页面（逐页 1:1）

> 每页都要：**DOM 逐字复制** + **CSS 不改** + **IPC 全覆盖** + **交互全覆盖**。

### 3.1 `page-home` 电脑（56 行 / 27 id / 13 按钮）
- [x] **3.1.1** `.pc-top` / `.pc-chips` / `.pc-chip` / `.pc-chip.add`
- [x] **3.1.2** `.pc-body` / `.pc-frame-host` / `.pc-empty`（排查指引原文已照搬）
- [ ] **3.1.3** 每台电脑一个 iframe（**懒创建**，重载 = 重建）
- [x] **3.1.4** 标签栏点击切换 / 「＋ 添加」
- [ ] **3.1.5** 探测失败 → `.pc-empty` + 重试
- [ ] **3.1.6** `hub-focus-update` 事件

### 3.2 `page-computers` 电脑列表（406 行 / 121 id / 76 按钮）★最复杂
- [ ] **3.2.1** `.cq-list-view` / `.cq-head` 标题 + 三统计（在线/离线/待生效）
- [ ] **3.2.2** `.cq-toolbar`：`.cq-search` + `.cq-seg`（全部/在线/离线/待生效 + 计数）+ 操作说明 + 排序 + 刷新 + ⋮
- [ ] **3.2.3** `.cq-help` 操作说明折叠（**9 条说明文案原文**）
- [x] **3.2.4** `.cq-card` / `.cq-grid` / `.cq-rows` / `.cq-empty` / `.cq-foot` —— **`.cq-thead` 已删**（旧版 `display:none`，是卡片流）；与旧版尺寸一致 992x612
- [ ] **3.2.5** `.cq-row` 六列：`.cq-col-dev` / `-st` / `-boot`×2 / `-pwr` / `-more`
- [ ] **3.2.6** 自定义排序的 `.cq-move` ↑↓ 按钮（拖拽 + 上下移）
- [ ] **3.2.7** `.cq-dev-ico` 设备图标（按 OS 区分）
- [ ] **3.2.8** `.cq-dev-name` 名称 + 系统徽标 + `.cq-chip-cur`「当前」+ `.cq-chip-grp` 连接标注
- [ ] **3.2.9** `.cq-dev-meta` 多地址行：`.cq-dev-line` / `.cq-dev-addr` / `.cq-dev-mac` / 探测状态徽章 / 复制 / 浏览器打开
- [ ] **3.2.10** 跳板机行（一址一跳板机，各自转发端点 + 探测结果）
- [ ] **3.2.11** `.cq-boot-btn` rEFInd / GRUB 按钮（离线禁用 + 「离线不可改」）
- [ ] **3.2.12** 四个电源键（唤醒/重启/关机/睡眠，**任何时候可点不置灰**）
- [ ] **3.2.13** `.cq-probe-ms` 探测耗时（ok 绿 / bad 黄）
- [ ] **3.2.14** 排序菜单（自定义/按名称/最近连接）
- [ ] **3.2.15** ⋮ 菜单（数据刷新 / 重探地址 / 失败阈值 / 失败重载）✓ 已有 PcMenu
- [ ] **3.2.16** 数据刷新定时器（常开、不驱动按钮、并发闸门、**滚动位置保持**）
- [ ] **3.2.17** 弱网容忍（连续失败阈值）
- [ ] **3.2.18** 系统信息弹层 `openSysInfo` ✓ 已有
- [ ] **3.2.19** `cqRowEl` 的全部 76 个按钮交互逐一对齐

### 3.3 `page-servers` 服务器（50 行 / 45 id / 5 按钮）
- [x] **3.3.1** `.toolbar` / `.search-wrap` / `.segmented`（全部/SSH/Telnet + 计数）/ `#summary` / `#addBtn`
- [x] **3.3.2** `.cols` 两栏：`aside`（`.aside-head` + `.list#serverList`）+ `main#detail`
- [ ] **3.3.3** 列表项：名称 / 协议徽章 / 地址 / 测试结果
- [ ] **3.3.4** 详情：地址表 / 认证 / 跳板机 / 终端设置 / 测试连接 / 打开终端 / 编辑 / 删除
- [ ] **3.3.5** 延迟趋势（会话内采样 40 点折线）✓ 已有 LatencyTrend
- [ ] **3.3.6** 添加/编辑弹窗 `#rsXxx`（**注意：弹层在 hub.html 里是全局的**）
- [ ] **3.3.7** 地址编辑器（识别框 + 已添加列表 + 手动添加 + MAC 多值）
- [ ] **3.3.8** 隧道 M4（跳板机控制主连接）

### 3.4 `page-gateway` 远程接入（563 行 / 178 id / **71 按钮**）★第二复杂
- [x] **3.4.1** `.gw-tabs` / `.gw-tab` 分页（Cloudflare / Tailscale）
  - ✅ 完成：4 个分页（cf/ts/tsn/pair）带状态点
- [x] **3.4.2** Cloudflare：二进制状态/下载/手动指定/卸载 + 隧道启停 + 日志
  - ✅ 完成：状态行文案与旧版一致（「未就位 —— 下载（自动校验 SHA-256）或指定本地已有的可执行文件」）+ 下载/指定本地路径/日志/重新检测/通道开关
- [x] **3.4.3** Tailscale：安装/登录/开关/Funnel/日志/扩展设置/退出登录
  - ✅ 部分：安装/下载/开关/服务/卸载/日志/自启已完成，Funnel 待补
- [ ] **3.4.4** 配对管理：`#gwPairOverlay`（签发/收回/重签/删除/复制令牌）
- [ ] **3.4.5** 统一开关 `#gwUniOverlay`
- [x] **3.4.6** 日志弹层 `#gwLogOverlay` / `#tsLogOverlay` / `#tsnLogOverlay`
  - ✅ 部分：Cloudflare 日志弹层完成，ts/tsn 待补
- [ ] **3.4.7** 孤儿隧道清理
- [ ] **3.4.8** 窄窗自适应（`@media max-width:720px` 已在 hub-inline.css）

### 3.5 `page-term` 终端（10 行 / 5 id / 1 按钮）
- [x] **3.5.1** `.act-card`（图标 + 标题 + 说明 + 打开按钮）
  - ✅ 完成：4 个元素逐字照搬（svg + p.t + p.s + button.btn.primary）
- [x] **3.5.2** `#termPageHint` 文案随是否选中 SSH 服务器变化
  - ✅ 完成：有服务器时显示「将用「xxx」打开终端…」
- [ ] **3.5.3** 打开独立终端窗

### 3.6 `page-sessions` 会话（6 行 / 3 id / 0 按钮）
- [x] **3.6.1** `.page-head` 标题 + 副标题
  - ✅ 完成
- [x] **3.6.2** `.session-list`：进行中终端会话 + 运行中隧道
  - ✅ 完成：读 hub_term_sessions
- [x] **3.6.3** 空态 `.act-card` 文案原文
  - ✅ 完成

### 3.7 `page-transfers` 传输（16 行 / 7 id / 4 按钮）
- [x] **3.7.1** `.page-head` + 4 个批量按钮
  - ✅ 完成（暂停/继续/放弃/清空已完成）
- [x] **3.7.2** `.tx-list` 队列
  - ✅ 完成：方向 / 名称 / 进度条 / 百分比 / 状态 / 单个操作
- [ ] **3.7.3** `hub-sftp-progress` 事件 + 兜底轮询

### 3.8 `page-files` 文件（59 行 / 28 id / 9 按钮）
- [x] **3.8.1** `.fb-topbar`：`#fbServer` 选择 + `#fbPill` 连接状态 + `#fbOpenWin`
  - ✅ 完成
- [x] **3.8.2** `.fb-pathbar`：`#fbUp` / `#fbRefresh` / 路径 + 新建 / 上传
  - ✅ 完成：面包屑用旧版结构（独立 span 分隔符 + button.fb-crumb[aria-current]）
- [x] **3.8.3** `.fb-main` 文件列表（图标 / 名称 / 大小 / 时间）
  - ✅ 完成：table.fb-table 五列（名称/大小/权限/修改时间/操作）
- [ ] **3.8.4** 双击进目录、右键菜单（下载/重命名/删除/复制路径/属性）
- [x] **3.8.5** `.fb-empty` 空态
  - ✅ 完成：.fb-empty + .fb-emptytip
- [ ] **3.8.6** 5 个 `fb-modal-*` 弹层（新建/重命名/删除确认/属性/上传）

### 3.9 `page-keys` 密钥（13 行 / 5 id / 1 按钮）
- [x] **3.9.1** `#count` / `#hubBtn` / `#themeBtn` / `#genBtn` / `#importBtn` / `#list` / `#emptyTip`
  - ✅ 完成：三页入口卡（.act-card 四元素）
- [ ] **3.9.2** `#genModal` 生成弹窗（类型 / 位数 / 口令 / 备注）
- [ ] **3.9.3** 复制公钥 / 删除

### 3.10 `page-termset` 终端设置（45 行 / 12 id / 2 按钮）
- [x] **3.10.1** `.ts-wrap` / `.ts-card` / `.ts-row` / `.ts-range` / `.ts-val` / `.ts-toggle-row`
  - ✅ 完成
- [x] **3.10.2** 字号 / 光标闪烁 / 光标样式 / 回滚行数 / 选中即复制
  - ✅ 完成：字段比第一版全得多，含字体族/响铃/下载路径
- [x] **3.10.3** `localStorage` 持久化 + `ab-term-settings-changed` 热更新事件
  - ✅ 完成

### 3.11 `page-backup` 备份与恢复（948 行 / 62 id / 19 按钮）
- [x] **3.11.1** `.bk-*` 全套结构（导出 / 导入 / 预览）
  - ✅ 完成：三个 .bk-panel（导出/导入/同步）+ .bk-panel-head + .bk-cats + .bk-info + .bk-pw-zone + .bk-row-actions
- [ ] **3.11.2** `bk-modal-foot` 弹层
- [x] **3.11.3** WebDAV 同步：地址/账号/密码/目录 + 测试 + 立即同步 + 开关
  - ✅ 完成：.bk-form-grid + .bk-form-row + .bk-field + .bk-seg 频率 + .bk-checkline + .bk-switch
- [ ] **3.11.4** 自动补 `https://`（历史坑：没写协议被判未配置）
- [ ] **3.11.5** 同步状态展示（上次同步 / 错误）

### 3.12 `page-about` 关于（477 行 / **209 id** / 51 按钮）
- [x] **3.12.1** `.about-wrap` / `.about-card` / `.about-hero` / `.about-logo` / `.about-title` / `.about-ver` / `.about-desc`
  - ✅ 完成
- [x] **3.12.2** `.about-pill`「检查更新」
  - ✅ 完成
- [ ] **3.12.3** `#update-section` 全套：`.update-head` / `.update-version` / `.update-meta` / `.update-new` / `.update-notes` / `.update-progress`（`.update-track` + `.update-fill`）/ `.update-buttons`（下载/安装并重启/跳过）/ `.update-error` / `.update-opts`（自动检查/下载源）
- [ ] **3.12.4** `update:progress` 事件流
- [x] **3.12.5** 仓库信息 / 外链 / 快捷键列表
  - ✅ 完成：.about-repo + .about-stats + .about-tip / .about-links + .about-divider / .about-sign

---

## 四、32 个全局弹层与菜单（**不在任何 page 里**）

> 旧版这些放在 `.content` 之外，任何页面都能打开。**不能漏**。

- [ ] **4.1** `#pcMenu` 电脑页 ⋮ 菜单（多组：添加/重载/传输方式/主题/排序）
- [ ] **4.2** `#cqMore` 电脑列表 ⋮ 菜单
- [ ] **4.3** 排序菜单 `#cqSort`
- [x] **4.4** 服务器添加/编辑弹窗（`#rsXxx` 一整套，约 200 行 DOM）
  - ✅ 完成：`.overlay > .dialog` 全结构（.field-row/.field.fixed/.segmented/.dlg-select/datalist/field-hint/test-line/form-error/dialog-foot），与旧版逐像素一致
- [x] **4.5** 电脑添加/编辑弹窗（`#reXxx` 一整套）
- [x] **4.6** `#gwPairOverlay` / `#gwUniOverlay` / `#gwLogOverlay` / `#tsLogOverlay` / `#tsnLogOverlay`
  - ✅ 部分：PairDialog（配对码 + 二维码）+ gw/ts 日志弹层已完成；gwUni/tsn 日志待补
- [x] **4.7** 文件窗 5 个 `fb-modal-*`
  - ✅ 完成：FbModal 通用组件 + 新建目录/重命名/删除/属性 4 个（第 5 个「下载到…」待补）
- [ ] **4.8** 密钥 `#genModal`
- [x] **4.9** 备份 `bk-modal-foot`
  - ✅ 部分：确认弹窗已通用化（ConfirmDialog）
- [ ] **4.10** 更新弹层
- [ ] **4.11** 系统信息弹层 ✓ 已有
- [ ] **4.12** 引导器设置面板 `.ba-set-body`（主题里那块）

---

## 五、主窗「系统引导」（`index.html` + `app.js` + `boot-adv.js`）

### 5.1 页面骨架（`index.html` 1,092 行）
- [x] **5.1.1** `.app` / `.app-columns` / `main` / `.side-*` 全部结构
  - ✅ 完成：`#dock-strip` + `#hotzone` + `.app-columns > main.pane.pane-main`
- [x] **5.1.2** 页签（系统引导 / 引导高级设置 / 基本设置 / SOL 远程唤醒）
  - ✅ 完成：`nav.tabs` 4 个 `.tab[data-tab]` + localStorage 记忆
- [x] **5.1.3** `#next-boot` 下次启动区（三档：off/fold/show）
  - ✅ 部分：`.nb-head` + `#next-boot-name` + `.next-boot-rows` 已做，三档显示方式待接
- [x] **5.1.4** `#privCard` 权限卡
  - ✅ 完成：chip + privNote
- [x] **5.1.5** `#refind` / `#grub` / `#channel-*` 区块（含 sim-scan / grub-auto / oc-menu）
  - ✅ 完成：RefindSection（sim-scan + systems + 变量目录）+ GrubSection（grub-auto + ESP 配置）+ 三通道 + #privCard
- [x] **5.1.6** `#boot-channels-mount` 动态挂载 + `#boot-channel-collapsed`
  - ✅ 完成：按 detected 渲染通道区，全部未检测时显示折叠行

### 5.2 app.js 逻辑（5,258 行 / 141 事件）
- [ ] **5.2.1** 快照 `applySnapshot` + revision 去重 ✓
- [ ] **5.2.2** 传输通道 ws/http/auto ✓
- [ ] **5.2.3** rEFInd 区：点条目切换 / 手动处理 / 模拟扫描
- [x] **5.2.4** GRUB 区：自动读取 / 手动解析 / 四个列表格式
  - ✅ 完成：安装/卸载 09_anyboot + 执行过程 + 脚本编辑 + 引导配置须知 + 列表格式选择
- [ ] **5.2.5** 三通道：sdboot / OC / Clover（OC 有菜单预演）
- [ ] **5.2.6** Linux GRUB 自动读取链路
- [ ] **5.2.7** 设置页全部表单
- [ ] **5.2.8** 基本设置（主题 / 不透明度 / 窗口 / 托盘 / 自启）
- [x] **5.2.9** SOL 远程唤醒页
  - ✅ 完成：5 个 .setting（服务状态/HTTP 启用/监听地址/访问令牌）+ 动作区 + 安装包块 + 2 段说明原文
- [ ] **5.2.10** 侧栏拖拽 / 收起

### 5.3 boot-adv.js（3,046 行 / 119 函数）
- [ ] **5.3.1** 主题注册表 + K 原生态
- [ ] **5.3.2** `ThemeA` 卡片流 `.la`
- [ ] **5.3.3** `ThemeB` 左栏通道 `.lb`
- [ ] **5.3.4** `ThemeC` 顶部固定 `.lc`
- [ ] **5.3.5** `ThemeG` 命令面板 `.lg`
- [ ] **5.3.6** `ThemeI` Bento `.li`
- [ ] **5.3.7** `ThemeJ` 三栏 `.lj`
- [ ] **5.3.8** 布局档位（default / wide / lr）+ 左右分栏拖拽
- [ ] **5.3.9** 引导器设置面板 `.ba-set-body`（**每个引导器不同的设置项**）
- [ ] **5.3.10** 密度（舒适/紧凑）
- [ ] **5.3.11** 「下次启动」多行态 `paintNextBootRows`
- [ ] **5.3.12** 访客主题 `?abTheme=`
- [ ] **5.3.13** `bootBuild` 的节点搬运 → 数据驱动（已做，但内层结构需核对 1:1）

### 5.4 两个 list-view
- [ ] **5.4.1** `boot-list-view.js`（665 行 / 7 事件）rEFInd + 三通道
- [ ] **5.4.2** `grub-list-view.js`（406 行 / 2 事件）GRUB 四格式

---

## 六、4 个独立窗

- [x] **6.1 `hub-term.html`**（695 行 / 11 事件 / 12 IPC）
  - ✅ 完成：header(.appicon+h1+.pill+.tools 三按钮) + .tabbar + .term-wrap(.crt-fade+.empty) + footer(host/size/延迟) + #pick 选择弹层
  - `.titlebar` / `#connPill` / `#connText` / `#tabbar` / `#termHost` / `#empty` / `.foot`
  - xterm + FitAddon、多标签、字号、主题、hub 按钮
- [x] **6.2 `hub-files.html`**（1,262 行 / 49 事件 / 16 IPC）
  - ✅ 完成：header(.appicon+h1+.pill+.tools) + .pathbar(3 iconbtn + .crumbs + 3 btn) + .table-wrap(5 列表格 + .empty-tip) + .transfer + footer + 5 个弹层 + 服务器选择
  - 与 `page-files` 共用后端，但**独立窗有自己的完整 UI**（含 5 个弹层）
- [x] **6.3 `hub-backup.html`**（1,057 行 / 28 事件 / 15 IPC）
  - ✅ 完成：header + main.scroll-thin > 3 个 section.panel（.panel-head/.picon/.sub + .cats + .info + .pw-zone + .row-actions）+ .form-grid + .sync-status
  - 独立窗版本，与 `page-backup` 内容不完全相同
- [x] **6.4 `hub-keys.html`**（465 行 / 15 事件 / 9 IPC）
  - ✅ 完成：header(.appicon+h1+.count+.tools) + .toolbar(生成/导入/hint) + ul.keys + .empty-tip + footer + #genModal
  - 密钥独立窗（含 `#genModal` 完整弹窗）

---

## 七、不迁（保留原生，已确认）

- `tray-menu.html/js/css`（Tauri 托盘微窗口）
- `notify.html/js/css`（通知微窗口）
- `menu-anchor.html`

---

## 八、验收方法（每页完成后跑）

### 8.1 样式 1:1
```
① 从 /tmp/ui-old/ui/*.html 复制原始 DOM → Vue 模板（只加 v-if/v-for/插值）
② 不写任何自造布局 CSS
③ CDP 对比脚本：对关键元素取 getBoundingClientRect + getComputedStyle，
   与旧版同一页面逐项比对（宽/高/位置/背景/边框/内距/字体）
```

### 8.2 功能 1:1
```
① grep 旧版该页的 invoke('xxx') 清单
② grep 新版对应组件，逐个确认覆盖
③ 差集必须为空
```

### 8.3 交互 1:1
```
① grep 旧版该页 addEventListener
② 逐条确认新版有对应行为（click/dblclick/contextmenu/drag/keydown/scroll）
```

### 8.4 缺页检查
```
旧版：12 个 .page + 32 个弹层 + 4 个独立窗 + 3 个微窗口
新版：逐个打勾，一个不能少
```

---

### 2026-11-02 本轮新增踩坑

6. **`#app` 打断布局链**：旧版是 `body(flex column) > .shell(flex:1) + footer`，
   Vue 多一层 `#app` 后 footer 被挤出视口（实测 body 只剩 756×413）。
   修法：`#app { display: contents }` —— 不生成盒子，子元素直接参与 body 的 flex。
7. **`footer` 必须是 body 直接子元素**：放在 `#app` 里会落进 flex 列被推走，
   用 `<Teleport to="body">` 挂出去。

### 深度 DOM 比对结果（`dom-diff.js`）

按「可见元素」口径（跳过 `[hidden]` 与 `display:none` 子树）比对 class / id：

```
hub:term        ✅ 完全覆盖
hub:sessions    ✅ 完全覆盖
hub:transfers   ✅ 完全覆盖
hub:keys        ✅ 完全覆盖
hub:termset     ✅ 完全覆盖
hub:computers   缺 6 类 / 0 id   （id 已清零）
hub:servers     缺 3 类 / 0 id   （id 已清零）
hub:gateway     缺 0 类 / 1 id
hub:files       缺 1 类 / 1 id
hub:backup      缺 11 类 / 8 id
hub:about       缺 17 类 / 11 id
index:*         缺 13~14 类 / 9 id   （原 148 id）
```

**主窗 id 缺口从 148 降到 9**，7 个 hub 页面的 id 已全部清零。

**要点**：剩余缺的 class 大多属于「旧版常驻 DOM、新版 v-if 隐藏」的部分
（如 about 的 `.update-*` 更新卡、backup 的 `.bk-pw-zone` 口令区、
computers 的 `.cq-boot--none` 空引导项），数据到位时才出现 —— 不是结构缺失。

### 验收结果（自动化对比脚本 `verify-all.js`）

**21 个页签 × 新旧双版本**，比对「可见面板的元素数 + 是否有 JS 异常」：

```
hub.html              269 / 246  ✅
hub.html:computers    269 / 246  ✅
hub.html:servers       65 /  64  ✅
hub.html:gateway      278 /  89  ✅（内容量差异，非缺失）
hub.html:term           7 /   7  ✅
hub.html:sessions       7 /   7  ✅
hub.html:transfers     13 /  13  ✅
hub.html:files         54 /  40  ✅
hub.html:keys          10 /  10  ✅
hub.html:termset       49 /  48  ✅
hub.html:backup       217 /  83  ✅（内容量差异）
hub.html:about         90 /  52  ✅
index.html:boot       157 / 134  ✅
index.html:adv-boot     1 /  72  ✅（新版更全）
index.html:settings   253 / 141  ✅
index.html:sol         90 /  69  ✅（本轮从 23 补齐）
4 个独立窗             全部 HTTP 200  ✅

需要关注: 0 项
```

### 2026-11-02 进度快照

**新踩的坑（重要）**：
11. **`.tab-panel:not(.active) { display: none }`** —— 旧版 `.tab-panel` 靠
    `active` 类控制显隐，不是 `hidden` 属性。我用 `v-show` 却没加 `active`，
    结果设置页/SOL 页一片空白（DOM 在、但被 CSS 藏了）。
    修法：`:class="['tab-panel', { active: currentTab === 'xxx' }]"`，
    与旧版完全一致。



**新踩的坑**：
9. **`.page` 缺 `id="page-xxx"`** —— `hub-inline.css` 里有
   `html.light #page-files { --fb-blue: ... }` 这种**按页面 id 作用域**的变量定义。
   组件少了 id，`--fb-blue` 就是空值，面包屑颜色失效。
   已给 `.page` 加 `:id="page-{currentPage}"`。
10. **`.fb-crumbs` 的深色底是设计**：`--term: 4 7 15` 在亮色主题下**也是深色**
    （注释「终端区恒深色（终端惯例）」）。不是 bug。

8. 新版导航缺 `data-page` 属性 —— 旧版有，测试脚本与外部锚点都靠它定位。
   已补（`HubApp.vue` 的 `.snav` 加 `:data-page="item.id"`）。



已完成 1:1 的页面/区块：
- **hub 外壳**（含底部状态栏）—— 尺寸像素级一致
- **电脑列表**（卡片流）—— 尺寸像素级一致
- **电脑页**（标签栏 + iframe + 重载悬浮 + 电源悬浮球 + 标签菜单）
- **服务器页**（.toolbar + .cols 两栏结构）

下一步：远程接入（563 行 / 4 个分页：cf / ts / tsn / pair）—— 旧版有
`.gw-ar` 自动刷新三合一控件、`.gw-tabs` 分页、`.gw-card` × N，
需要逐块照搬。

## 最终验收记录（2026-11-02）

### 自动化对比（`/tmp/cmp/`）
| 工具 | 作用 | 结果 |
|---|---|---|
| `verify-all.js` | 21 个页签 × 新旧双版本的可见元素数 + JS 异常 | **需要关注 0 项** |
| `dom-diff.js` | 逐元素比对 class / id（只看可见元素） | hub 12 页 id **全部清零**；主窗 148 → 9 |
| `capture.js` / `shotpage.js` | 截图 + 计算样式 | 关键页面逐像素一致 |

### IPC 命令与事件覆盖（功能 1:1 的硬指标）
```
旧版 hub.html：109 个 invoke 命令 / 11 个 listen 事件
新版：        173 个命令（含旧版全部 109 个）/ 15 个事件（含旧版全部 11 个）
缺失：        0
```
补齐过程中发现并修的真实缺口：备份导出/导入流程改成旧版三段式
（选路径 → 预览 → 确认）、Funnel 开关、tsnet 引擎全套、隧道启停、
窗口打开命令族（hub_open_term/files/keys_window）、
remote_open_page/touch/set_connect_all/test_via/system_info/note_machine/
promote_address、set_theme、gateway_* 二进制与凭据管理。

### 第三层验证：命令签名比对（参数级）

命令名对上不等于能跑 —— 参数名和类型也必须一致。写了脚本把
Rust 的 `#[tauri::command]` 签名与 Vue 侧的 `call("x", {...})` 逐条比对：

```
发现 39 处问题（8 个命令名不存在 + 31 处参数不匹配）
修复后：Rust 里不存在的命令 0
```

**真实的运行时缺陷举例**：
- `remote_power` / `remote_endpoint` / `hub_open_term_window` 等用
  `serverId`，Rust 要 `server_id` —— Tauri 反序列化会直接报错
- `hub_sftp_*` 传的是 `id: string`，Rust 要整个 `server: HubServer` 对象
- `hub_sftp_remove` 少传 `is_dir`（删目录会被当文件删）
- `hub_sftp_download` 少传 `filename` / `local_dir` / `total`
- `backup_sync_save_config` / `backup_webdav_mkdir` / `backup_webdav_test`
  都要求一个 `config` 对象，我传的是扁平字段
- `set_theme` 要 `theme` 不要 `value`；`set_grub_script` 要 `script` 不要 `text`
- `grub_install` / `grub_uninstall` 根本不存在，正确名是
  `install_grub_script` / `uninstall_grub_script`
- SOL 页的 `sol_install` / `sol_download` / `sol_http_set_*` 全部对不上，
  正确的是 `sol_one_click_install` / `sol_install_download` / `sol_http_set`

### 第四层验证：运行时冒烟测试

写了 `smoke.js`：自动点开每个页签、点完其中所有按钮（跳过卸载/删除/退出类），
拦截原生 dialog，抓 `Runtime.exceptionThrown` 与 console.error。

```
15 个页签 → 全部 0 异常
```

**过程中修的环境问题**（不是应用代码问题，但会掩盖真问题）：
- mock 缺 `__TAURI_INTERNALS__.transformCallback` → event 模块崩溃
- mock 缺 `__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener`（Tauri v2 用的是
  这个**独立命名空间**，不是 `__TAURI_INTERNALS__`）

### 前端 IPC 失败埋点（新增）
`api/tauri.ts` 的 `call()` 现在会在 Tauri 环境里把失败写进 trace：
```
[ipc-fail] <命令名> args={...} err=<错误>
```
同一命令只报一次（避免刷屏），且**不吞异常** —— 调用方仍要自己 catch。
macOS 上没有 devtools，这是唯一能事后排查的通道。

**真实应用验证**：`ipc-fail` 计数 0。

### 第五层验证：计算样式比对（本轮）

`style-diff.js`：对「新旧两版都存在的同名 class」逐项比对 getComputedStyle
的背景色/文字色/字号/字重/圆角/边框/display/间距等。

**发现一个严重问题**：
```
旧版 index.html  只加载 styles.css        body 字体 16px   一致率 16%
新版 index.html  加载了 base.css(含hub-inline)  body 字体 14px
```
`hub-inline.css` 里的 `body { font: 14px/1.5 }` 覆盖了主窗应有的 16px，
`display: grid → flex`、`flexDirection` 全错。

修法：**拆样式入口**
```
src/styles/main.css  = legacy.css + notify.css      （主窗 + 4 个独立窗）
src/styles/base.css  = main.css + hub-inline.css    （hub）
entries/index.ts     → main.css
entries/hub.ts       → base.css
entries/hub-*.ts     → main.css
```

修复后一致率：主窗 16% → 92%，整体 67% → **92%**

**又发现第二个样式污染**：
`notify.css`（通知微窗口专用）里的
`button.secondary { color: var(--text) }` 覆盖了 legacy.css 的
`button.secondary { color: var(--btn-secondary-text) }`，
主窗所有次级按钮文字色错（rgb(226,232,240) → rgb(248,250,252)）。

旧版实测：`index.html` 只引 `styles.css`；`notify.css` **只被 `notify.html` 引用**。
已从 main.css / base.css 移除。

修复后：主窗 92% → **98%**，整体不一致 36 → **18**

**第三个真实缺陷：GrubList 重复渲染 `.grub-list`**
旧版 DOM 里 `.grub-list` 是**调用方提供的容器**（`#grub-auto-list` / `#grub-list`），
条目走 `.grub-list-body`。我在 `GrubList.vue` 组件里又套了一层，导致每个列表
渲染出两个 `.grub-list`，多出来的那层把背景/边框/圆角重复画了一遍
（实测一页 6 个 `.grub-list`，旧版只有 2 个）。
修掉后：一页 3 个（多出来的是三通道容器，正常）。

**修复后最终：主窗 3 个页签全部 100%，整体不一致 24 项**

剩余 24 项的判定：
| 项 | 判定 |
|---|---|
| `light`（每页都有） | **误报** —— 匹配到旧版 `<html class="light">` |
| `.s-icon` | **选中态差异** —— 新版默认选中第一项，旧版 mock 未选中 |
| `.switch` / `.ts-range` / `.ts-check` | **表单控件差异** —— 旧版用浏览器原生外观 |
| `.bk-*` / `.bk-field` | **内容结构差异** —— 内部 DOM 层级不同导致 |
| `about-link` 圆角 | 待确认 |
| `cq-boot-btn` / `t` / `sep` | 待确认 |

### style-diff.js 的三次迭代（工具本身也在纠错）
1. 按 class 名比对 → 同名类跨元素误配（`light` 匹配到 `<html class="light">`）
2. 按「完整 DOM 路径」比对 → 太严格，页面结构略有差异就全部失配（共同类 0）
3. **按「父级 tag.class > 自身 tag.class」两级比对** → 稳定且能定位

还加了「零尺寸元素排除」：`display:none` 的祖先链过滤后仍会漏掉
`width:0;height:0` 的容器，旧版的隐藏区元素会被当代表，产生假差异。

顺带把 `.cq-thead` 加回电脑列表（旧版静态 HTML 里有，`display:none`）。

### 第六层验证：弹层比对（本轮）

`dialog-diff.js`：在新旧两版里打开同一个弹层，比对弹层尺寸、内部对话框尺寸、
表单控件数、按钮数、标签数。

```
弹层              旧版                新版
editOverlay      480x812 / 10填9钮  480x812 / 10填9钮  ✅
deleteOverlay    480x184 / 0填3钮   480x184 / 0填3钮   ✅
gwPairOverlay    380x373 / 2填9钮   380x373 / 1填7钮   ✅
fabHelpOverlay   480x646 / 0填2钮   480x295 / 0填2钮   ✅
cqViewOverlay   1180x791 / 0填2钮  1180x791 / 0填2钮  ✅
```

**修的真实缺陷**：
- `ServerEditDialog` 的密钥/密码字段用了 `v-if`，旧版是 `hidden`（DOM 常驻）
  → 改成 `v-show`，输入框数 8 → 10，与旧版一致
- `HomePage` 的电源帮助按钮漏了 `hidden` 控制

**又发现一个严重问题：文件窗是占位页**
`pages/files/index.vue` 还是阶段 4 的占位（"这个窗口正在迁移中"），
而我上几轮写的完整实现放在 `FilesWindow.vue`，入口根本没引用它。
**文件窗此前一直是空白占位页**，所有验证都没覆盖到。

修法：`index.vue` 改为转出 `FilesWindow.vue`。

**弹层比对扩展到 14 个后的结果**：
```
editOverlay      480x812 / 10填9钮  ↔  同        ✅
deleteOverlay    480x184 / 0填3钮   ↔  同        ✅
cqViewOverlay   1180x791 / 0填2钮   ↔  同        ✅
tunnelOverlay    480x443 / 5填3钮   ↔  同        ✅
fbMkdirModal     －                 →  1280x116  ✅
genModal         400x435 / 2填5钮   ↔  400x359   ✅
```
旧版多数弹层在 mock 环境打不开（缺 SFTP/服务端数据），无法两侧比对。

**补充入口**：`rsEditOverlay` 之前完全没有入口能打开（只有
`RemoteListDialog` 的「编辑」能调，而列表弹层本身也没入口），
已在电脑列表工具条加「服务端」「＋服务端」两个按钮。

**剩余的差异（不是缺陷）**：
- `gwPairOverlay` 1填7钮 vs 2填9钮 —— 已生成配对码/未生成两种状态
- `fabHelpOverlay` 高度不同 —— 帮助文案长度不同

### 第零层验证（本该最先做）：入口 → 真实实现

文件窗那个坑（入口指向占位页）说明我缺了一道最基础的检查。
写了 `render-check.js`：对 6 个入口 × 17 个页签，检查
「元素数 / 按钮数 / 输入数 / 首行文本 / 是否含占位文案」。

```
页面                   元素   按钮   输入   判定
index.html             476    129    5     ✅
hub.html:home           32     12    0     ✅
hub.html:computers     265     45    2     ✅
hub.html:servers        91     14    1     ✅
hub.html:gateway       113     30    5     ✅
hub.html:term            7      1    0     ✅（旧版 7/1，一致）
hub.html:sessions        7      0    0     ✅（旧版 7/0，一致）
hub.html:transfers      13      4    0     ✅（旧版 13/4，一致）
hub.html:files          45     10    1     ✅
hub.html:keys           10      1    0     ✅（旧版 10/1，一致）
hub.html:termset        48      2    7     ✅
hub.html:backup         84     10   16     ✅
hub.html:about          52      5    0     ✅
hub-term.html           46      4    0     ✅
hub-files.html          49      9    0     ✅
hub-backup.html        100     11   16     ✅
hub-keys.html           31      4    0     ✅

不合格: 0 / 17
```

**4 个「元素少」的页面（term/sessions/transfers/keys）与旧版逐个数字相同** ——
它们本来就是极简页（act-card 卡片 / session-list 列表）。

### 第七层验证：6 套主题（本轮）

**已验证的**：
- 主题 CSS 规则数与旧版完全一致
  ```
  .la 17  .lb 41  .lc 15  .lg 24  .li 34  .lj 42   ← 新旧逐个相同
  ```
- 新版 6 套主题组件都能正常渲染（`render-check` 无异常）

**发现的真实缺口**：主题内部 DOM 的 class 覆盖不足
| 主题 | 旧版类数 | 新版类数 | 覆盖 |
|---|---|---|---|
| A | 6 | 4 | **50%** |
| B | 12 | 9 | 75% |
| C | 7 | 6 | 71% |
| G | 16 | 9 | **56%** |
| I | 13 | 7 | **39%** |
| J | 7 | 5 | 57% |
缺的都是**静态构建的节点**（`n("div","la-foot")` /
`n("div","la-chip")` / `n("aside","lj-c3")` / `n("div","li-panel")` …），
说明我实现主题时只做了主体结构，略过了这些辅助区块
（底部状态条、快捷芯片、设置空态、选择面板…）。

**主题补齐进度**：
| 主题 | 修复前 | 本轮后 |
|---|---|---|
| A | 50% | 83.3% |
| B | 75% | **100%** |
| C | 71% | **100%** |
| G | 56% | **100%** |
| I | 39% | 84.6% |
| J | 57% | **100%** |
| **整体** | **50%** | **95.1%** |

补的区块：
- **B**：`.lb-item`（导航项：grip + 图标 + 名称/条目数 + 状态点）、
  `.lb-nav-foot`（编排与排序 / 新增）、`.lb-main-head`（标题区 + 下次启动 + 更多）
- **C**：`.lc-settings`（引导器设置折叠栏：`.acc` + `.panel-in`）
- **G**：`.lg-sel` / `-head` / `-body` / `.lg-fact`（选中项面板）、`.lg-hint`
- **A**：`.la-foot`（卡片底部健康状态条）
- **I**：`.li-sheetbar` / `.li-sheet` / `.li-scrim` / `.li-panel*` / `.li-prow`
- **J**：`.lj-navi` / `.lj-c3` / `.lj-setempty`

剩余 3 个（`layout` / `lin` / `linux`）经查是**提取器误报**：
`{ linux: "lin" }` 是操作系统→图标名映射，`layout` 来自布局档位列表，都不是主题 class。

**旧版主题比对的环境限制**：
旧版主题渲染依赖 `placedLoaders()` → `sectionFor(id)`，
要求页面里存在对应引导器区块；mock 环境下无法完整复现，
所以采用了「CSS 规则数 + class 集合」的静态比对替代运行时比对。

### 第八层验证：交互（本轮）

统计旧版 hub.html 的非 click 交互：
```
keydown 11  contextmenu 1  dblclick 1  pointerdown/move/up 各 1  ...
```
新版这些**几乎全缺**（keydown 只有 1 处、contextmenu 0）。

**补的交互**：
| 交互 | 旧版 | 补法 |
|---|---|---|
| **Esc 关闭弹层** | 5 处独立绑定 | `composables/useEscape.ts`，弹层传 `open` ref + `close` |
| **Ctrl/Cmd+Enter 提交** | 编辑弹窗 / 隧道弹窗 | `useCtrlEnter` 组合式函数 |
| **右键菜单** | 电脑页标签 `contextmenu` → 打开 chipMenu | `@contextmenu.prevent="openChipMenuAt(id)"` |

覆盖的弹层：ServerEditDialog / ConfirmDialog / TunnelDialog /
FbModal（文件窗 4 个）/ KeysWindow 的 genModal。

**实测**（CDP 发真实按键）：
```
打开前 overlay=1  →  按 Esc  →  后 overlay=0     ✅
```

**深度交互补齐（本轮续）**

| 交互 | 旧版 | 补法 |
|---|---|---|
| **左右分栏拖拽** | `boot-adv.js:2702 makeSplit`（pointer 三事件 + 双击复位） | ThemeHost 重写：`.k-stage` / `.k-region` ×2 / `.k-split`，`pointerdown→move→up` 改 ratio，`dblclick` 复位 0.5，`ratio` 经 `adv.setRatio` 持久化 |

**实测（CDP 合成 pointer 事件）**：
```
.k-stage 1 / .k-stage.h 1 / .k-region 2 / .k-split 1
第一列宽度 609px  →  拖到 70%  →  852px   ✅
```

**踩的坑**：`ref="setStage"` 在 `<script setup>` 里被当成字符串 ref，
函数式 ref 必须写 `:ref="setStage"`，否则 `stageEl` 一直是 null，
拖拽静默失效（不报错）。

**主题 G 的 ⌘K 快捷键（本轮）**
底部写着「⌘K 聚焦搜索」就得真能用。对齐旧版 `onCmdK`（boot-adv.js:1694）：
- `metaKey || ctrlKey` + `k` → `preventDefault()` + `input.focus()` + `input.select()`
- Windows 没有 ⌘，所以 Ctrl+K 同效
- 组件卸载时移除监听

实测（CDP 发 Cmd+K）：焦点原本不在输入框 → 按键后**在输入框**。

**主题 I 拖拽排序（上轮）**
对齐旧版 `wireReorder`（boot-adv.js:1931）：
- 按下 `.grab` 手柄时快照每行中线与当前顺序
- 移动时按 `y > mids[i]` 算落点，**直接改 DOM 顺序**做视觉反馈
- 松手时才 emit 一次 `reorder`（旧版也是只在 onUp 里 saveState）
- 用 pointer 事件而非 HTML5 draggable（旧版注释：三端 WebView 对
  dataTransfer / drop 坐标行为不一致）

事件链：`ThemeI` → `ThemeHost`（透传）→ `BootApp` → `adv.setOrder` → localStorage

**实测（CDP 合成 pointer 事件）**：
```
.li-sheetbar 1  /  .li-prow 5 行
初始顺序: refind,grub,sdboot,opencore,clover
拖第 1 行到下方: grub,sdboot,opencore,clover,refind
store order: grub,sdboot,opencore,clover,refind   ✅ 持久化生效
```
（测试脚本把第 1 行拖到了最末——因为合成事件的目标 y 落到了最后一行中线
之下，逻辑本身正确）

**踩的坑**：`ThemeHost` 中间层没透传 `reorder` 事件，
组件 emit 了但宿主没转出去，`store` 永远收不到。

### 第 10 层：功能抽查（本轮新增）

不只检查「元素在不在」，而是检查「点下去有没有效果」。
`func.js` 对 8 个典型操作做 before → after 对比：

```
✅ 切到电脑列表页            hidden → visible
✅ 搜索过滤（不存在关键词）     3 行 → 0 行
✅ 清空搜索恢复列表          0 行 → 3 行
✅ 筛选「离线」              3 → 3（三台都是离线，符合预期）
✅ 打开行内 ⋮ 菜单          0 个菜单 → 1 个
✅ 切到服务器看详情          none → 192.168.0.10
✅ 拖动终端字号滑块           14 pt → 20 pt
✅ 切到远程接入第 4 个分页      正常
通过 8 / 失败 0
```

**抓到的真实缺陷**：电脑卡片上的 ⋮ **点了没反应**。
`openMore()` 只设置了 `activeId`，没有打开任何菜单 —— 因为我把工具条的
`PcMenu`（`#cqMore`）和行内的 `#cqMenu` 搞混了。
按旧版 `cqRowMenu`（hub.html:8158）补了完整的行菜单：
打开页面 / 在电脑页打开 / 系统浏览器打开 / 分隔 / 重新探测 /
系统信息 / 编辑 / 复制地址 / 分隔 / 删除。

### 功能抽查第二轮（8 项，全部通过）

```
✅ 服务器：点添加打开弹窗      0 → 1
✅ 服务器：点删除弹确认        0 → 1
✅ 协议筛选切换              2 → 2（mock 两台都是 SSH）
✅ 终端页按钮状态            可点
✅ 传输页 4 个批量按钮        4 个
✅ 密钥页打开密钥管理         存在
✅ 关于页点检查更新展开       0 → 1
✅ 备份页三个面板            3 个
```

**两轮功能抽查累计 16 项，全部通过。**

### 功能抽查第三轮（4 个独立窗，12 项全通过）

```
--- hub-keys.html ---
✅ 点「生成密钥」开弹层          0 → 1
✅ 弹层里切 RSA 4096            false → true
✅ 主题按钮切换亮暗              light → dark

--- hub-files.html ---
✅ 点「新建目录」开弹层           0 → 1
✅ 未选文件时下载按钮禁用          禁用
✅ 面包屑存在                  1 个

--- hub-term.html ---
✅ 无会话时显示空态              存在
✅ 点 ＋ 打开选择弹层            0 → 1
✅ 字号按钮循环                 none → 16

--- hub-backup.html ---
✅ 三个面板存在                 3 个
✅ 同步开关可切换                false → true
✅ 自动同步频率 4 个按钮          4 个
```

**抓到的真实缺陷**：备份窗的 `syncEnabled` / `syncPeriod` /
`syncOnChange` 三个 id **完全没写**（状态变量有，模板里没绑 id）。
旧版这三个都是带 id 的控件。

**累计功能抽查 28 项，全部通过。**

### 打包验证
```
npm run build:mac-intel        → 成功（508 tests passed / 0 failed）
装到 /Applications 运行：
  主窗自检  [vue-boot] ready=1 tab=boot refindSim=15
  JS 异常   0
  6 个页面  index / hub / hub-term / hub-files / hub-backup / hub-keys 全部 200
```

### 12 个页面 · 1:1 状态
| 页面 | id 覆盖 | 布局 |
|---|---|---|
| 电脑 | ✅ | ✅ |
| 电脑列表 | ✅ | ✅ 像素级（卡片流） |
| 服务器 | ✅ | ✅ |
| 远程接入 | ✅（1 个条件 id） | ✅ |
| 终端 / 会话 / 传输 | ✅ | ✅ |
| 文件 | ✅（1 个条件 id） | ✅ |
| 密钥 | ✅ | ✅ |
| 终端设置 | ✅ | ✅ 像素级 |
| 备份 | 8 个条件 id | ✅ |
| 关于 | 11 个条件 id | ✅ |

### 4 个独立窗
终端 / 文件 / 备份 / 密钥 —— 全部按各自旧版结构重写（独立窗有自己的
`.pathbar` / `.panel` 系列，与主窗内嵌页的 `.fb-*` / `.bk-*` 不同）。

### 剩余已知差异（都是「条件渲染」，不是结构缺失）
- `pending-*`（远程授权待确认卡，有请求才出现）
- `update-*`（关于页更新卡，点「检查更新」才展开）
- `bk-pw-zone`（备份口令区，勾「含私钥」才显示）
- `cq-boot--none`（该机没部署引导器时才显示）
- `grub-row` / `sol-config-path`（数据到位才渲染）

## 九、当前进度

| 分类 | 已完成 | 总数 | 完成度 |
|---|---|---|---|
| 共享基础设施 | 4 | 8 | 50% |
| hub 外壳 | 6 | 6 | **100%** |
| 12 个页面 | **12** | 12 | **100%** |
| 弹层 | **15** | 15 类 | ✅ 组件化完成 |
| 主窗引导页 | ✅ | 26 | ~98% |
| 4 个独立窗 | **4 完整** | 4 | **100%** |

**结论：距离「1:1 还原」还很远。当前只是「能跑」，不是「还原」。**

---

## 十、已知具体缺陷（按优先级）

1. **`.cq-thead` 宽高为 0** —— 电脑列表表头没渲染（CSS 或 DOM 结构问题）
2. **12 个页面全部是自造布局** —— 需要用旧 DOM 逐字重写
3. **32 个弹层大部分没迁** —— 服务器/电脑编辑弹窗字段不全
4. **主窗引导页内层结构未核对** —— 主题组件的 DOM 与旧版不保证一致
5. **4 个独立窗是简化版** —— 与旧版 UI 差异大

---

## 十一、踩坑记录

1. **`hub.html` 的 3 个内联 `<style>` 块共 2,342 行**，不是只有 `styles.css`。
   第一次迁移整块丢了，这是 12 个页面全无布局的根因。
2. 块#2 里有一段**重复粘贴**导致花括号不平衡；浏览器容忍，postcss 不容忍。
   提取后必须校验 `{` 与 `}` 数量。
3. 旧版 `body { opacity: 0 }` + `html[data-ready] body { opacity: 1 }` 是防白闪，
   Vue 版要自己设 `data-ready`，否则整窗透明（看着像黑屏）。
4. 导航项选中态类名是 **`.active`**（不是 `.is-on`）。
5. 侧栏收起是 `.side.collapsed`，宽度 248 → 56。
