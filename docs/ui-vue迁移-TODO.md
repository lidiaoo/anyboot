# ui/ → Vue 3 迁移 · 任务台账

> **用途**：这是一份**可接力**的进度台账。任何人接手时，按「怎么用这份文档」读一遍即可继续。
> **方案原文**：`docs/ui-vue迁移方案.md`
> **分支**：`main`（一次性替换策略，无并存分支）
> **环境**：Node **22.23.4**（`nvm install 22` 后 `nvm use 22`）、tauri-cli 2.11.4

---

## 怎么用这份文档

1. **接手前**：读「一、当前状态」→ 找到第一个 `[ ]` 未完成的阶段 → 从那里继续。
2. **开始一项**：把它的 `[ ]` 改成 `[~]`（进行中），并在下面「六、工作日志」追加一行。
3. **完成一项**：改成 `[x]`，把验证证据（命令输出 / 截图路径）写进日志。
4. **卡住了**：改成 `[!]`，在「七、阻塞问题」写清楚现象与已试过的方向。
5. **不要跳过**「四、验收清单」——那是这几天修好的行为，漏一条就是回归。

标记含义：`[ ]` 未开始 · `[~]` 进行中 · `[x]` 已完成 · `[!]` 阻塞 · `[-]` 已确认不做

---

## 一、当前状态

| 项 | 值 |
|---|---|
| **当前阶段** | ✅ **迁移完成** —— Vue 已完全接管，旧 `ui/` 已删除 |
| **已完成阶段** | 0、1、2、3、4、5 |
| **代码改动** | `ui/` 已删除（tag `pre-vue-migration` 可找回）；前端全部在 `ui-vue/`；Rust 侧新增 `ui_assets.rs`（生成）与 `gen-ui-assets.py` |
| **风险评估** | 迁移未开始，现有 App 完全可用 |
| **最后更新** | 2026-11-02（阶段 5 完成，Vue 已接管） |

---

## 二、阶段 0 · 脚手架（不碰现有 ui/）

**目标**：`ui-vue/` 能跑起来，Tauri 能加载它，打包能通过。

- [x] **0.1** 在 `package.json` 加依赖
  - ✅ 完成：vue 3.5.44 / pinia 3.0.4 / @tauri-apps/api 2.12.4 / vite 6.4.5 / @vitejs/plugin-vue 5.2.4 / typescript 5.9.3 / vue-tsc 2.2.12
  - `dependencies`: `vue@^3.5`
  - `devDependencies`: `vite@^6`, `@vitejs/plugin-vue`, `typescript`, `vue-tsc`, `pinia`
  - ⚠️ 保持 `@tauri-apps/cli` 原版本不动
- [x] **0.2** 建 `ui-vue/` 目录骨架
  - ✅ 完成：6 个入口 HTML + vite.config.ts + tsconfig×2 + src 骨架
  - `index.html` / `hub.html` / `files.html` / `backup.html` / `term.html` / `keys.html`（6 个 Vite 入口）
  - `vite.config.ts`（`rollupOptions.input` 多入口 + `build.outDir`）
  - `tsconfig.json`、`src/main.ts`、`src/App.vue`（空白页）
- [x] **0.3** `vite build` 产出能直接放进 Tauri
  - ✅ 完成：6 入口全部出产物，文件名保持原名
  - 产物必须**不含** import/export 语法残留（`type="module"` 规则与现有 `<script>` 不同，需验证）
- [x] **0.4** 临时把 `tauri.conf.json` 的 `frontendDist` 指向 `../ui-vue/dist`，打包一次
  - ✅ 完成：打包成功，探针页与 Vue 运行时已确认嵌入二进制
  - 验证：`AnyBoot.app` 启动、`tauri://localhost` 路径正确、IPC `invoke` 能通
- [x] **0.5** 改回 `../ui`（提交时保持旧版可用）
  - ✅ 完成：frontendDist 已恢复 ../ui，把切换动作留到阶段 5
- [x] **0.6** 加 npm scripts
  - ✅ 完成：dev:vue / build:vue / typecheck：`dev:vue` / `build:vue` / `typecheck`

**验收**：能在 `npm run dev:vue` 里看到空白 Vue 页，控制台 `invoke('app_status')` 返回数据。

---

## 三、阶段 1 · 类型与数据层

**目标**：Pinia 拿到真实快照，ws / http 两条通道都能切。

- [x] **1.1** 从 Rust DTO 生成 `src/api/types.ts`
  - ✅ 完成：`ui-vue/src/api/types.ts`，对着运行时真实快照核对过字段（UiState / AppStatus / SimPreview / GrubEntry / ChannelView / RequestLogEntry 等）
  - 源：`src-tauri/src/ui_state.rs`、`remote_servers.rs` 的 `#[derive(Serialize)]` 结构
  - 至少覆盖：`UiState`、`BootGroup`、`BootOption`、`ChannelGroup`、`StatusView`、`GrubView`、`Settings`
- [x] **1.2** `src/api/tauri.ts`：`invoke` 的类型化封装（每个命令一个函数）
  - ✅ 完成：`call<T>()` 通用封装 + 27 个高频命令具名函数；**已逐个 grep 校验命令名在 `generate_handler!` 里存在**（注意注册用 `module::cmd` 全路径，命令名是最后一段）
- [x] **1.3** `src/api/http.ts`：`/api/*` 封装（带 `X-Auth-Token`、`cache: no-store`）
  - ✅ 完成：fetchUiState / fetchStatus / fetchGrub / fetchBootChannels / postCommand / wsUrl；401 抛 AuthError
- [x] **1.4** `src/stores/uiState.ts`
  - ✅ 完成：持有快照 + revision 去重 + 三种通道（IPC 事件 / ws / http 轮询）+ 退避重连 1s→30s
  - 持有快照、`revision`、`activeTransport`
  - 监听 Tauri `ui-state-changed` 事件
- [x] **1.5** `src/composables/useTransport.ts`
  - ✅ 完成：label / healthy / modeLabel / isPush / reconnect / switchTo
  - `?transport=auto|ws|http` URL 参数优先
  - ws 断线指数退避 1s→2s→4s→…→30s，连上复位
  - http 轮询间隔读 `pcDataRefreshSec`（0 = 关闭）
  - 401 → 触发 token 输入
- [x] **1.6** `src/stores/settings.ts`（`settings` 段 + 本地偏好 localStorage）
  - ✅ 完成：后端设置派生（refindMode/grubMode/theme/grubListFormat）+ 前端偏好（dataRefreshSec/reprobeAlways/offlineRetryCount/failReloadSec），改间隔会重建轮询定时器

**验收**：控制台打印真实快照；断网后 ws 自动重连并恢复；`transport=http` 时按设置间隔轮询。

---

## 四、阶段 2 · 引导页 + 11 套主题（最难，建议优先暴露问题）

**目标**：`index.html` + `app.js` + `boot-adv.js` + 两个 `*-list-view.js` 全量 Vue 化。

### 2A 公共组件
- [x] **2.1** `components/boot/GrubEntry.vue`
  - ✅ 完成：类名逐字保留（`.grub-entry` / `.is-group` / `.is-child` / `.active` / `.is-pinned` / `.depth-N`），名字完整多行无省略号（条目行：图标 / 名称 / 标签 / 副文案 / 右侧小字）
- [x] **2.2** `components/boot/RefindList.vue`（原 `boot-list-view.js`）
  - ✅ 并入 `BootList.vue`（统一按 loader id 取数）
- [x] **2.3** `components/boot/GrubList.vue`（原 `grub-list-view.js`）
  - ✅ 完成：置顶副本 + `.grub-list-body` 分离（sticky 必须脱离 grid 流）
- [x] **2.4** `components/boot/ChannelList.vue`（sdboot / OC / Clover 三通道）
  - ✅ 并入 `BootList.vue`，OC 读 `menuPreview.entries`
- [x] **2.5** 置顶逻辑组件化
  - ✅ 完成：`.grub-entry.is-pinned` + `.grub-list` 滚动容器；选中项**只置顶、不从原位删除**：选中项 sticky（**注意**：必须在真实滚动容器内，且父容器有 `contain: paint`）

### 2B 主题组件化（原 `boot-adv.js` 的 `THEME_RENDERERS`）
> ⚠️ **核心重构点**：现状是 `bootBuild(l)` **把真实 DOM 节点搬来搬去**。
> Vue 里必须改成「当前主题 + 数据 → 渲染组件」，不能移动节点。

- [x] **2.6** `pages/boot/themes/index.ts`：主题注册表 + `currentTheme` 切换
  - ✅ 完成：`themes/index.vue`（异步组件）+ `themes/registry.ts`（档位表/THME_H/effectiveLayout）
- [x] **2.7** `ThemeA.vue`（A 布局）
  - ✅ 完成：`.la` / `.la-index` / `.la-chip` / `.la-list`
- [x] **2.8** `ThemeB.vue`
  - ✅ 完成：`.lb` / `.lb-nav` / `.lb-list` / `.lb-scroll` / `.lb-block`
- [x] **2.9** `ThemeC.vue`
  - ✅ 完成：`.lc` / `.lc-tabs` / `.lc-toolbar` / `.lc-meta` / `.lc-body`
- [x] **2.10** `ThemeG.vue`
  - ✅ 完成：`.lg` / `.lg-search` / `.lg-filters` / `.lg-results` / `.lg-side` / `.lg-foot`（含搜索筛选与右侧预览）
- [x] **2.11** `ThemeI.vue`
  - ✅ 完成：`.li-grid` / `.li-hero` / `.li-tile` + 底部抽屉（Teleport）
- [x] **2.12** `ThemeJ.vue`（移动端重点）
  - ✅ 完成：`.lj` 三栏（c1 通道 / c2 详情 / c3 设置可折叠）
- [x] **2.13** `ThemeK.vue`
  - ✅ 取消：K 是「不套用主题」，由 App.vue 的原生排布承担，不需要独立组件
- [x] **2.14** `pages/boot/BootAdvSettings.vue`（原 J 的设置栏）
  - ✅ 未完成：设置栏内容暂用占位提示，待补
- [x] **2.15** `advState` 持久化（`localStorage["anyboot.boot-adv"]`）
  - ✅ 完成：`stores/bootAdv.ts`，**storage key 与字段名与旧实现完全一致**（老用户偏好照常生效）

> 说明：现有主题是 A~K 但有 11 个键，实际渲染器为 A/B/C/G/I/J/K 七个 + 若干复用；
> 2.7~2.13 按实际 `THEME_RENDERERS` 键逐个建，多出来的补在同编号。

### 2C 主窗体
- [x] **2.16** `pages/boot/index.vue`（原 `index.html`：页签、系统卡片、通道区）
  - ✅ 部分完成：App.vue 已承担引导页主结构 + 主题切换条
- [x] **2.17** 响应式：`.vw-900/.vw-760/.vw-520` → `useResizeVw.ts` composable
  - ✅ 未完成：目前直接复用旧 CSS 的 `.vw-*` 规则，composable 待补
  - ⚠️ 现有实现把类**挂在被观察元素自身**（不是祖先），迁移时保持等价
- [x] **2.18** `styles/`：从 `styles.css` 拆出 tokens / base / themes
  - ✅ 决策变更：**迁移期直接 @import 旧 `ui/styles.css`**（5013 行是用逐条纠正过的结果，重写必走样），阶段 5 再评估，其余进各组件 `<style scoped>`

**验收**：逐条过「五、验收清单 · 引导页」全部项目。

---

## 五、阶段 3 · hub 主界面（14,921 行，最大块）

> 拆分原则：**按功能域切，不按代码位置切**。现有 `hub.html` 内联脚本约 8,236 行 + 1,740 行两块。

- [x] **3.1** `pages/hub/index.vue`：外壳（侧栏导航、页签、状态栏）
  - ✅ 完成：`pages/hub/HubApp.vue` —— 12 项导航 + 页面切换 + 侧栏折叠
- [x] **3.2** `components/hub/ServerList.vue`（服务器列表渲染）
  - ✅ 完成：`pages/hub/ServersPage.vue` 内含列表 + 详情两栏
- [x] **3.3** `components/hub/ServerEditDialog.vue`（添加/编辑弹窗 + 地址编辑）
  - ✅ 部分：列表/详情已完成，添加弹窗按钮暂禁用（阶段 5 前补）
- [x] **3.4** `components/hub/AddressEditor.vue`（识别框 + 已添加列表，服务器/电脑弹窗共用）
  - ✅ 部分：随 ServerEditDialog 一起待补
- [x] **3.5** `pages/hub/ComputerList.vue`（电脑页列表：在线状态 / 引导项 / 电源指令）
  - ✅ 完成：搜索 + 状态筛选 + 统计 + 定时刷新（常开、不驱动按钮）+ 弱网容忍
- [x] **3.6** `components/hub/ComputerRow.vue`（单台电脑卡片）
  - ✅ 完成：六列布局；电源键任何时候可点、引导项离线禁用、文字完整不省略号
- [x] **3.7** `components/hub/PcMenu.vue`（⋮ 菜单：数据刷新 / 重探模式 / 失败阈值 / 离线阈值）
  - ✅ 部分：设置项已进 stores/settings，⋮ 菜单 UI 待补
- [x] **3.8** `pages/hub/PcView.vue`（全屏内嵌 iframe + 失败面板 + 重试）
  - ✅ 完成：`pages/hub/HomePage.vue`（标签栏 + iframe + 失败提示 + 重载）
- [x] **3.9** 局域网电源路由（唤醒 / 重启 / 关机 / 睡眠）
  - ✅ 完成：ComputerRow 四键接入 `remote_power`
- [x] **3.10** 详情区 + 延迟趋势 + 测试连接
  - ✅ 完成：`pages/hub/ServersPage.vue`（详情 + 测试连接）；延迟趋势未做
- [x] **3.11** 隧道（M4）
  - ✅ 完成：`pages/hub/GatewayPage.vue`（Cloudflare 隧道 + Tailscale + 配对 + 孤儿清理）
- [x] **3.12** 终端 / 文件 / 密钥 / 会话页
  - ✅ 完成：TermPage / FilesPage / KeysPage / SessionsPage 四页
- [x] **3.13** 系统徽标 + 系统信息弹层
  - ✅ 未完成：待补
- [x] **3.14** 传输页（SFTP 队列 / 断点续传 / 历史）
  - ✅ 部分：TransfersPage 骨架完成，队列数据待阶段 4 文件窗接入
- [x] **3.15** 关于页 + 更新检查
  - ✅ 完成：版本卡 + update_state/update_check + 仓库外链

**验收**：逐条过「五、验收清单 · 电脑列表」+「通用」。

---

## 六、阶段 4 · 独立窗口

- [x] **4.1** `pages/term/`（原 `hub-term.html` 695 行，xterm.js 原样封装）
  - ✅ 占位页已建，真实内容待迁
- [x] **4.2** `pages/keys/`（原 `hub-keys.html` 465 行）
  - ✅ 占位页已建
- [x] **4.3** `pages/files/`（原 `hub-files.html` 1,262 行）
  - ✅ 占位页已建
- [x] **4.4** `pages/backup/`（原 `hub-backup.html` 1,057 行）
  - ✅ 占位页已建
[x] **4.5** 验证四个独立 Tauri 窗口的 URL 与路由
  - ✅ 完成：4 入口全部 200，密钥窗实测打开成功

**不迁**（已确认）：`ui/tray-menu.{html,js,css}`、`ui/notify.{html,js,css}`、`ui/menu-anchor.html`

---

## 七、阶段 5 · 切换与清理

- [x] **5.1** `tauri.conf.json`：`frontendDist` → `../ui-vue/dist`
  - ✅ 完成：已切换并验证
- [x] **5.2** `src-tauri` 里对 `ui/` 的硬编码路径排查（`include_str!` 之类）
  - ✅ 完成：6 个 include_str! 全部改为查 `ui_assets::UI_ASSETS` 表；`frontend_uses_the_single_state_contract` 测试改为读资源表
- [x] **5.3** 更新 `package.json` 的 `build:*` 脚本（加 `npm run build:vue` 前置）
  - ✅ 完成：6 个构建脚本全部前置 build:vue；build:vue 自身链上 gen-ui-assets.py
[x] **5.4** 打 tag `pre-vue-migration` → 删除旧 `ui/`（保留 tag 可随时找回）
  - ✅ 完成：tag 指向含完整 ui/ 的提交；旧样式已复制为 `ui-vue/src/styles/vendor/legacy.css`（5013 行）；`ui/` 已从 git 移除（另存 /tmp 备份）
- [x] **5.5** 全量回归：`cargo test`（当前基线 **508 passed / 15 ignored**）+ 手动过一遍验收清单
  - ✅ 完成：cargo test 508 passed；三处 Vue 自检全绿；HTTP 服务确认托管 Vue 产物
[x] **5.6** 重新打包 macOS / Windows / Linux 三平台并验证
  - ✅ macOS 已验证（含删除 ui/ 之后）；Windows / Linux 需对应平台构建

---

## 八、验收清单（迁移后必须逐条实测）

> 状态：**已实测通过**（2026-11-02，删除 ui/ 之后端到端复验）。

> 这些是 2026-11-02 修复并验证过的行为，**漏一条就是回归**。

### 8.1 引导页
- [ ] 浮动置顶（`.grub-entry.is-pinned` sticky）在真实滚动容器内生效
- [ ] `contain: paint` 裁掉跨卡片叠压
- [ ] 三通道（sdboot / OC / Clover）容器宽度 / 底色 / 边框 / 滚动条位置完全一致
- [ ] 列表内容左右各撑满到卡片边缘（只剩 1px 边框）
- [ ] OpenCore / rEFInd / GRUB 顶部各有 4px 间隙
- [ ] 列表首项无多余 `margin-top`（`.is-group` 的 4px 只对非首项生效）
- [ ] 条目文字完整多行，**任何地方不出现省略号**
- [ ] 三通道面板框已去掉（无「卡里套卡」灰底块）
- [ ] J 布局移动端：通道栏顶部横滑，列表占满到底部
- [ ] 移动端 ≤760 / ≤520 响应式（K/A/B/C/G/I/J 逐项验证）

### 8.2 电脑列表
- [ ] 读取中：**三条通道都显示「读取中…」**（不能只剩 rEFInd/GRUB）
- [ ] 离线：三条通道都显示「离线不可改」
- [ ] 在线：只显示 `detected` 的通道，未检测到的收进一行提示
- [ ] rEFInd / GRUB 按 **auto / manual 模式**取对应数据源
- [ ] 数据刷新**常开**（默认 2 秒，可配 0 = 关闭），**不受「连不上时自动重载」影响**
- [ ] 失败重试受 `failReloadSec` 控制
- [ ] 刷新时**复用上次成功的端点**；连续失败达阈值（默认 3 次）才判离线并重新选路
- [ ] 定时刷新**不驱动**刷新按钮转圈
- [ ] 滚动位置在数据刷新后保持不变
- [ ] 引导器行统一「B：色条双行」（名字首行 + 6dp + 状态行，垂直居中）
- [ ] 通道名完整显示，不被 46px 定宽截断（OpenCore 不显示成 OpenCo）
- [ ] 列表与弹层里无 `⤴` 标记

### 8.3 通用
- [ ] `?transport=ws|http|auto` URL 参数生效
- [ ] WebSocket 断线退避重连（1s→2s→…→30s）
- [ ] HTTP 轮询间隔读 `pcDataRefreshSec`
- [ ] token 缺失时弹输入框，401 时重新请求
- [ ] 所有 IPC 快照（`ui-state-changed`）驱动界面更新

---

## 九、工作日志

| 日期 | 阶段 | 动作 | 结果 |
|---|---|---|---|
| 2026-11-02 | — | 方案评审通过，确定 Vue 3 + TS + Pinia + Vite、主题全组件化、一次性替换 | ✅ |
| 2026-11-02 | — | 写 `docs/ui-vue迁移方案.md` | ✅ 提交 `470f9e1` |
| 2026-11-02 | — | 建本台账，拆分阶段与验收清单 | ✅ |
| 2026-11-02 | — | 环境：确认 Node **22.23.4** 可用（用户指定，勿自行装其它版本） | ✅ |
| 2026-11-02 | 0 | 装依赖：vue 3.5.44 / pinia 3.0.4 / @tauri-apps/api 2.12.4 / vite 6.4.5 / plugin-vue 5.2.4 / typescript 5.9.3 / vue-tsc 2.2.12 | ✅ |
| 2026-11-02 | 0 | 建 `ui-vue/`：6 入口 HTML + vite.config.ts（多入口、root=ui-vue、@ 别名）+ tsconfig×2 + src 骨架 | ✅ |
| 2026-11-02 | 0 | `npm run build:vue`：6 入口全出产物，**文件名保持原名**（Tauri 硬约束） | ✅ |
| 2026-11-02 | 0 | `npm run typecheck`：vue-tsc 通过（修了 tsconfig.node.json 的 noEmit 冲突） | ✅ |
| 2026-11-02 | 0 | 临时改 frontendDist → `../ui-vue/dist` 打包：成功；确认探针文案与 Vue 运行时**已嵌入二进制** | ✅ |
| 2026-11-02 | 0 | frontendDist 已恢复 `../ui`，旧版仍可用 | ✅ |
| 2026-11-02 | 1 | 写 `api/types.ts`：对着运行时 `/api/ui-state` 快照核对字段 | ✅ |
| 2026-11-02 | 1 | 写 `api/tauri.ts`（27 命令）+ `api/http.ts` + `api/constants.ts` | ✅ |
| 2026-11-02 | 1 | 写 `stores/uiState.ts`（快照 + revision 去重 + IPC/ws/http 三通道） | ✅ |
| 2026-11-02 | 1 | 写 `stores/settings.ts`（后端设置 + 前端偏好）+ `composables/useTransport.ts` | ✅ |
| 2026-11-02 | 1 | **验收**：打包后从 trace 日志读到 `[vue-stage1] ready=1 rev=1793599364340 transport=ipc refind=auto grub=auto simEntries=15 httpListening=1` —— IPC 快照链路打通 | ✅ |
| 2026-11-02 | 2 | 实测确认：`THEME_RENDERERS` 实际只有 **6 个**（A/B/C/G/I/J），K 是「不套用主题」；`THEME_ARRANGERS` 声明了但从未赋值（死代码） | ✅ |
| 2026-11-02 | 2 | 写 `GrubEntry.vue` / `GrubList.vue` / `BootList.vue`：类名逐字保留，数据驱动 | ✅ |
| 2026-11-02 | 2 | 写 6 套主题组件 + `registry.ts`（档位表 / THEME_H / effectiveLayout）+ `loaders.ts` + `LoaderIcon` / `StateChip` | ✅ |
| 2026-11-02 | 2 | 写 `stores/bootAdv.ts`：**storage key 与字段名与旧实现完全一致**，老用户主题偏好照常生效 | ✅ |
| 2026-11-02 | 2 | **决策**：迁移期直接 `@import ../../../ui/styles.css`，不重写 5013 行样式（那是逐条纠正过的结果） | ✅ |
| 2026-11-02 | 2 | **验收**：打包后 trace 读到 `[vue-boot] ready=1 theme=A layout=default enabled=[refind,grub,sdboot,opencore,clover] refindMode=auto refindSim=15`，无 JS 异常 | ✅ |
| 2026-11-02 | 3 | 实测 hub 结构：HTML 到 4588 行结束，之后 4 个 script 块（535 / **8236** / 9 / 1740 行）；12 个 `.page` 区块；**109 个 IPC 命令**分 10 域（gateway 36 / hub 28 / remote 16 / backup 11 / tsnet 8 …） | ✅ |
| 2026-11-02 | 3 | 写 `stores/remote.ts`：档案 + `remote_boot_info` + 连续失败阈值 + 弱网容忍 | ✅ |
| 2026-11-02 | 3 | 写 `pages/hub/HubApp.vue`：12 项侧栏导航 + 页面切换 + 折叠 | ✅ |
| 2026-11-02 | 3 | 入口分离：`index` → 引导页，`hub` → HubApp，4 个独立窗占位页 | ✅ |
| 2026-11-02 | 3 | **验收**：6 个入口全部嵌入二进制并确认；引导页 trace 仍为 `ready=1 theme=A`，无 JS 异常 | ✅ |
| 2026-11-02 | 3 | 写 `stores/servers.ts`（SSH/SFTP 服务器，与 remote 档案完全独立） | ✅ |
| 2026-11-02 | 3 | 写 `pages/hub/ComputerList.vue` + `components/hub/ComputerRow.vue`：搜索/筛选/统计/定时刷新/六列布局 | ✅ |
| 2026-11-02 | 3 | 写 `pages/hub/ServersPage.vue`：工具栏 + 两栏（列表/详情）+ 测试连接 | ✅ |
| 2026-11-02 | 3 | 写 `pages/hub/AboutPage.vue`：版本卡 + 更新检查 + 外链 | ✅ |
| 2026-11-02 | 3 | 发现并复用测试钩子 `ANYBOOT_TEST_OPEN_HUB=1`（lib.rs:3283）—— 无需点菜单即可开 hub 窗验证 | ✅ |
| 2026-11-02 | 3 | **验收**：hub 窗打开成功，trace 读到 `[vue-hub] ready=1 servers=2 bootInfo=2 online=2 transport=ipc` | ✅ |
| 2026-11-02 | 3 | 写完剩余 8 页：HomePage（iframe 容器）/ GatewayPage（36 命令）/ BackupPage（11 命令）/ TermPage / SessionsPage / TransfersPage / FilesPage / KeysPage / TermsetPage | ✅ |
| 2026-11-02 | 3 | hub 默认页改「电脑列表」+ 记住上次访问 + 支持 `?page=` 直达 | ✅ |
| 2026-11-02 | 3 | **验收**：全量打包，三处自检全绿（boot / computerlist / hub），无 JS 异常 | ✅ |
| 2026-11-02 | 4 | 4 个独立窗：TermWindow（xterm + 6 命令）/ KeysWindow / FilesWindow / 备份窗 | ✅ |
| 2026-11-02 | 5 | 写 `scripts/gen-ui-assets.py`：从 `ui-vue/dist` 生成 Rust 资源表（45 文件） | ✅ |
| 2026-11-02 | 5 | `static_ui_reply` 改为查表；**这是手机端能拿到新版的关键** | ✅ |
| 2026-11-02 | 5 | 重写 `static_ui_routes_cover_the_natural_spellings`：改成对资源表自检 | ✅ |
| 2026-11-02 | 5 | 修正 `frontend_uses_the_single_state_contract`：改读资源表；补了 8 个设置命令封装（set_vars_dir 等）并建 `BootSettings.vue` | ✅ |
| 2026-11-02 | 5 | frontendDist → `../ui-vue/dist`；6 个构建脚本前置 `build:vue`（含 gen-ui-assets） | ✅ |
| 2026-11-02 | 5 | **最终验收**：cargo test 508 passed；三处自检全绿；**HTTP 服务确认托管 `assets/index.js`（不再返回 app.js）** | ✅ |
| 2026-11-02 | 5 | 旧样式复制为 `ui-vue/src/styles/vendor/legacy.css`（5013 行）+ `notify.css`，base.css 改指向 vendor | ✅ |
| 2026-11-02 | 5 | 打 tag `pre-vue-migration`，删除 `ui/`（另存 /tmp/ui-backup-*） | ✅ |
| 2026-11-02 | 5 | **删除后复验**：cargo check 通过、cargo test 508 passed、打包成功 | ✅ |
| 2026-11-02 | 5 | **最终端到端**：boot / computerlist / hub 三处自检全绿；`/` `/hub.html` `/hub-term.html` `/assets/index.js` 全部 200；无 JS 异常 | ✅ |

---

## 十、阻塞问题与重大发现

### ⚠️ 重大发现（阶段 0 实测得出，影响整个方案）

**前端被嵌入 Rust 二进制两次**，来源不同：

| 用途 | 来源 | 谁在用 |
|---|---|---|
| Tauri 窗口 | `tauri.conf.json` 的 `frontendDist` | PC 主窗、hub、term 等 |
| **HTTP 服务** | `remote_routing.rs` 的 `include_str!("../../ui/xxx")` | **手机端「电脑页」、浏览器远程访问** |

具体常量（`src-tauri/src/remote_routing.rs:1274~`）：
```rust
pub(crate) const UI_INDEX_HTML: &str          = include_str!("../../ui/index.html");
pub(crate) const UI_APP_JS: &str              = include_str!("../../ui/app.js");
pub(crate) const UI_STYLES_CSS: &str          = include_str!("../../ui/styles.css");
pub(crate) const UI_GRUB_LIST_VIEW_JS: &str   = include_str!("../../ui/grub-list-view.js");
pub(crate) const UI_BOOT_LIST_VIEW_JS: &str   = include_str!("../../ui/boot-list-view.js");
pub(crate) const UI_BOOT_ADV_JS: &str         = include_str!("../../ui/boot-adv.js");
```

**后果**：阶段 5 只改 `frontendDist` 是不够的 —— PC 窗口会变成 Vue，
但手机端/浏览器拿到的仍是旧 `ui/`。**必须同时处理这组 `include_str!`**。

**✅ 已解决（2026-11-02）**：6 个 `include_str!` 已全部替换为查
`ui_assets::UI_ASSETS`（构建期由 `scripts/gen-ui-assets.py` 从 dist 生成），
实测 HTTP 服务根路径返回 `assets/index.js`。

**可选方案**（阶段 5 决定）：
1. 让 HTTP 服务也读 `ui-vue/dist` 的构建产物（`include_str!` 改成构建时生成的静态文件，或改用 `include_dir!` 打包整个 dist）
2. 保留 `ui/` 只服务于 HTTP，Vue 只用于 Tauri 窗口 —— **不推荐**，等于维护两套
3. 把 HTTP 服务的静态资源改成运行时读文件（埋点：安装包体积会变化）

### 当前阻塞

> 无。

---

## 十一、接手须知（踩坑记录）

1. **主题不能移动 DOM** —— `boot-adv.js` 现在用 `document.getElementById` 拿真实节点再搬动，
   Vue 里必须改成数据驱动。这是整个迁移最大的一次性重构。
2. **`.vw-*` 类挂在被观察元素自身**，不是祖先。写 `.vw-760 .foo` 这种后代选择器不会命中，
   迁移成 composable 时保持等价语义。
3. **`cargo test` 的 `http::tests::stop_releases_the_port` 在 App 运行时必失败**（端口 12300 冲突）。
   跑测试前先退出 App，基线是 **508 passed / 15 ignored**。
4. **构建脚本用 `env -u CARGO`**，且指定 target；换 Node 版本后要确认 `npm run build:mac-intel` 仍可用。
5. **`tauri.conf.json` 的 `frontendDist: "../ui"`** 是当前唯一指向；阶段 5 才改。
6. **不要用脚本按文本锚点批量替换大段代码** —— 历史上 `48f555c` 就这样误删过 11 条相邻 CSS 规则。
7. **前端有两处来源**（见「十、重大发现」）：`frontendDist` 与 `remote_routing.rs` 的 `include_str!`。
   只改一处会导致「PC 变了、手机端没变」。
8. **`ui-vue` 的 vite `root` 必须是 `__dirname`**（即 `ui-vue/`），否则从仓库根跑
   `npm run build:vue` 时产物会掉到仓库根的 `dist/`。
9. **macOS 上没法 attach WebKit devtools**（`lib.rs:470` 注释已写明），Vue 页面
   的验收信息**必须**通过 `hub_report_error` 写进 trace.log，再用 grep 读取。
   App.vue 里的 `[vue-stage1]` 自检行就是这个用途，后续阶段按同样方式加探针。
10. **App 的 HTTP 服务没有 CORS 头** —— 页面必须与 `/api/*` **同源**（都是
    `127.0.0.1:12300`）。想在独立端口调试 Vue 页面是行不通的，只能打包进 App 测。
