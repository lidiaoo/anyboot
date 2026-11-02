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
- [ ] **3.4.1** `.gw-tabs` / `.gw-tab` 分页（Cloudflare / Tailscale）
- [ ] **3.4.2** Cloudflare：二进制状态/下载/手动指定/卸载 + 隧道启停 + 日志
- [ ] **3.4.3** Tailscale：安装/登录/开关/Funnel/日志/扩展设置/退出登录
- [ ] **3.4.4** 配对管理：`#gwPairOverlay`（签发/收回/重签/删除/复制令牌）
- [ ] **3.4.5** 统一开关 `#gwUniOverlay`
- [ ] **3.4.6** 日志弹层 `#gwLogOverlay` / `#tsLogOverlay` / `#tsnLogOverlay`
- [ ] **3.4.7** 孤儿隧道清理
- [ ] **3.4.8** 窄窗自适应（`@media max-width:720px` 已在 hub-inline.css）

### 3.5 `page-term` 终端（10 行 / 5 id / 1 按钮）
- [ ] **3.5.1** `.act-card`（图标 + 标题 + 说明 + 打开按钮）
- [ ] **3.5.2** `#termPageHint` 文案随是否选中 SSH 服务器变化
- [ ] **3.5.3** 打开独立终端窗

### 3.6 `page-sessions` 会话（6 行 / 3 id / 0 按钮）
- [ ] **3.6.1** `.page-head` 标题 + 副标题
- [ ] **3.6.2** `.session-list`：进行中终端会话 + 运行中隧道
- [ ] **3.6.3** 空态 `.act-card` 文案原文

### 3.7 `page-transfers` 传输（16 行 / 7 id / 4 按钮）
- [ ] **3.7.1** `.page-head` + 4 个批量按钮（暂停/继续/放弃/清空已完成）
- [ ] **3.7.2** `.tx-list` 队列：方向 / 名称 / 进度条 / 百分比 / 状态 / 单个操作
- [ ] **3.7.3** `hub-sftp-progress` 事件 + 兜底轮询

### 3.8 `page-files` 文件（59 行 / 28 id / 9 按钮）
- [ ] **3.8.1** `.fb-topbar`：`#fbServer` 选择 + `#fbPill` 连接状态 + `#fbOpenWin`
- [ ] **3.8.2** `.fb-pathbar`：`#fbUp` / `#fbRefresh` / 路径 + 新建 / 上传
- [ ] **3.8.3** `.fb-main` 文件列表（图标 / 名称 / 大小 / 时间）
- [ ] **3.8.4** 双击进目录、右键菜单（下载/重命名/删除/复制路径/属性）
- [ ] **3.8.5** `.fb-empty` 空态
- [ ] **3.8.6** 5 个 `fb-modal-*` 弹层（新建/重命名/删除确认/属性/上传）

### 3.9 `page-keys` 密钥（13 行 / 5 id / 1 按钮）
- [ ] **3.9.1** `#count` / `#hubBtn` / `#themeBtn` / `#genBtn` / `#importBtn` / `#list` / `#emptyTip`
- [ ] **3.9.2** `#genModal` 生成弹窗（类型 / 位数 / 口令 / 备注）
- [ ] **3.9.3** 复制公钥 / 删除

### 3.10 `page-termset` 终端设置（45 行 / 12 id / 2 按钮）
- [ ] **3.10.1** `.ts-wrap` / `.ts-card` / `.ts-row` / `.ts-range` / `.ts-val` / `.ts-toggle-row`
- [ ] **3.10.2** 字号 / 光标闪烁 / 光标样式 / 回滚行数 / 选中即复制
- [ ] **3.10.3** `localStorage` 持久化 + `ab-term-settings-changed` 热更新事件

### 3.11 `page-backup` 备份与恢复（948 行 / 62 id / 19 按钮）
- [ ] **3.11.1** `.bk-*` 全套结构（导出 / 导入 / 预览）
- [ ] **3.11.2** `bk-modal-foot` 弹层
- [ ] **3.11.3** WebDAV 同步：地址/账号/密码/目录 + 测试 + 立即同步 + 开关
- [ ] **3.11.4** 自动补 `https://`（历史坑：没写协议被判未配置）
- [ ] **3.11.5** 同步状态展示（上次同步 / 错误）

### 3.12 `page-about` 关于（477 行 / **209 id** / 51 按钮）
- [ ] **3.12.1** `.about-wrap` / `.about-card` / `.about-hero` / `.about-logo` / `.about-title` / `.about-ver` / `.about-desc`
- [ ] **3.12.2** `.about-pill`「检查更新」
- [ ] **3.12.3** `#update-section` 全套：`.update-head` / `.update-version` / `.update-meta` / `.update-new` / `.update-notes` / `.update-progress`（`.update-track` + `.update-fill`）/ `.update-buttons`（下载/安装并重启/跳过）/ `.update-error` / `.update-opts`（自动检查/下载源）
- [ ] **3.12.4** `update:progress` 事件流
- [ ] **3.12.5** 仓库信息 / 外链 / 快捷键列表

---

## 四、32 个全局弹层与菜单（**不在任何 page 里**）

> 旧版这些放在 `.content` 之外，任何页面都能打开。**不能漏**。

- [ ] **4.1** `#pcMenu` 电脑页 ⋮ 菜单（多组：添加/重载/传输方式/主题/排序）
- [ ] **4.2** `#cqMore` 电脑列表 ⋮ 菜单
- [ ] **4.3** 排序菜单 `#cqSort`
- [ ] **4.4** 服务器添加/编辑弹窗（`#rsXxx` 一整套，约 200 行 DOM）
- [ ] **4.5** 电脑添加/编辑弹窗（`#reXxx` 一整套）
- [ ] **4.6** `#gwPairOverlay` / `#gwUniOverlay` / `#gwLogOverlay` / `#tsLogOverlay` / `#tsnLogOverlay`
- [ ] **4.7** 文件窗 5 个 `fb-modal-*`
- [ ] **4.8** 密钥 `#genModal`
- [ ] **4.9** 备份 `bk-modal-foot`
- [ ] **4.10** 更新弹层
- [ ] **4.11** 系统信息弹层 ✓ 已有
- [ ] **4.12** 引导器设置面板 `.ba-set-body`（主题里那块）

---

## 五、主窗「系统引导」（`index.html` + `app.js` + `boot-adv.js`）

### 5.1 页面骨架（`index.html` 1,092 行）
- [ ] **5.1.1** `.app` / `.app-columns` / `main` / `.side-*` 全部结构
- [ ] **5.1.2** 页签（系统引导 / 引导高级设置 / 基本设置 / SOL 远程唤醒）
- [ ] **5.1.3** `#next-boot` 下次启动区（三档：off/fold/show）
- [ ] **5.1.4** `#privCard` 权限卡
- [ ] **5.1.5** `#refind` / `#grub` / `#channel-*` 区块（含 sim-scan / grub-auto / oc-menu）
- [ ] **5.1.6** `#boot-channels-mount` 动态挂载 + `#boot-channel-collapsed`

### 5.2 app.js 逻辑（5,258 行 / 141 事件）
- [ ] **5.2.1** 快照 `applySnapshot` + revision 去重 ✓
- [ ] **5.2.2** 传输通道 ws/http/auto ✓
- [ ] **5.2.3** rEFInd 区：点条目切换 / 手动处理 / 模拟扫描
- [ ] **5.2.4** GRUB 区：自动读取 / 手动解析 / 四个列表格式
- [ ] **5.2.5** 三通道：sdboot / OC / Clover（OC 有菜单预演）
- [ ] **5.2.6** Linux GRUB 自动读取链路
- [ ] **5.2.7** 设置页全部表单
- [ ] **5.2.8** 基本设置（主题 / 不透明度 / 窗口 / 托盘 / 自启）
- [ ] **5.2.9** SOL 远程唤醒页
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

- [ ] **6.1 `hub-term.html`**（695 行 / 11 事件 / 12 IPC）
  - `.titlebar` / `#connPill` / `#connText` / `#tabbar` / `#termHost` / `#empty` / `.foot`
  - xterm + FitAddon、多标签、字号、主题、hub 按钮
- [ ] **6.2 `hub-files.html`**（1,262 行 / 49 事件 / 16 IPC）
  - 与 `page-files` 共用后端，但**独立窗有自己的完整 UI**（含 5 个弹层）
- [ ] **6.3 `hub-backup.html`**（1,057 行 / 28 事件 / 15 IPC）
  - 独立窗版本，与 `page-backup` 内容不完全相同
- [ ] **6.4 `hub-keys.html`**（465 行 / 15 事件 / 9 IPC）
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

### 2026-11-02 进度快照

已完成 1:1 的页面/区块：
- **hub 外壳**（含底部状态栏）—— 尺寸像素级一致
- **电脑列表**（卡片流）—— 尺寸像素级一致
- **电脑页**（标签栏 + iframe + 重载悬浮 + 电源悬浮球 + 标签菜单）
- **服务器页**（.toolbar + .cols 两栏结构）

下一步：远程接入（563 行 / 4 个分页：cf / ts / tsn / pair）—— 旧版有
`.gw-ar` 自动刷新三合一控件、`.gw-tabs` 分页、`.gw-card` × N，
需要逐块照搬。

## 九、当前进度

| 分类 | 已完成 | 总数 | 完成度 |
|---|---|---|---|
| 共享基础设施 | 4 | 8 | 50% |
| hub 外壳 | 6 | 6 | **100%** |
| 12 个页面 | **1**（电脑列表） | 12 | 8% |
| 32 个弹层 | 2 | 32 | 6% |
| 主窗引导页 | 部分 | 26 | ~25% |
| 4 个独立窗 | 0 完整 | 4 | 0% |

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
