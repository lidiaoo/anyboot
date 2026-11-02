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
| **当前阶段** | 阶段 0 未开始（方案已评审通过） |
| **已完成阶段** | 无 |
| **代码改动** | 仅新增 `docs/`（方案 + 本台账），`ui/` 与 `src-tauri/` 未动 |
| **风险评估** | 迁移未开始，现有 App 完全可用 |
| **最后更新** | 2026-11-02 |

---

## 二、阶段 0 · 脚手架（不碰现有 ui/）

**目标**：`ui-vue/` 能跑起来，Tauri 能加载它，打包能通过。

- [ ] **0.1** 在 `package.json` 加依赖
  - `dependencies`: `vue@^3.5`
  - `devDependencies`: `vite@^6`, `@vitejs/plugin-vue`, `typescript`, `vue-tsc`, `pinia`
  - ⚠️ 保持 `@tauri-apps/cli` 原版本不动
- [ ] **0.2** 建 `ui-vue/` 目录骨架
  - `index.html` / `hub.html` / `files.html` / `backup.html` / `term.html` / `keys.html`（6 个 Vite 入口）
  - `vite.config.ts`（`rollupOptions.input` 多入口 + `build.outDir`）
  - `tsconfig.json`、`src/main.ts`、`src/App.vue`（空白页）
- [ ] **0.3** `vite build` 产出能直接放进 Tauri
  - 产物必须**不含** import/export 语法残留（`type="module"` 规则与现有 `<script>` 不同，需验证）
- [ ] **0.4** 临时把 `tauri.conf.json` 的 `frontendDist` 指向 `../ui-vue/dist`，打包一次
  - 验证：`AnyBoot.app` 启动、`tauri://localhost` 路径正确、IPC `invoke` 能通
- [ ] **0.5** 改回 `../ui`（提交时保持旧版可用），把切换动作留到阶段 5
- [ ] **0.6** 加 npm scripts：`dev:vue` / `build:vue` / `typecheck`

**验收**：能在 `npm run dev:vue` 里看到空白 Vue 页，控制台 `invoke('app_status')` 返回数据。

---

## 三、阶段 1 · 类型与数据层

**目标**：Pinia 拿到真实快照，ws / http 两条通道都能切。

- [ ] **1.1** 从 Rust DTO 生成 `src/api/types.ts`
  - 源：`src-tauri/src/ui_state.rs`、`remote_servers.rs` 的 `#[derive(Serialize)]` 结构
  - 至少覆盖：`UiState`、`BootGroup`、`BootOption`、`ChannelGroup`、`StatusView`、`GrubView`、`Settings`
- [ ] **1.2** `src/api/tauri.ts`：`invoke` 的类型化封装（每个命令一个函数）
- [ ] **1.3** `src/api/http.ts`：`/api/*` 封装（带 `X-Auth-Token`、`cache: no-store`）
- [ ] **1.4** `src/stores/uiState.ts`
  - 持有快照、`revision`、`activeTransport`
  - 监听 Tauri `ui-state-changed` 事件
- [ ] **1.5** `src/composables/useTransport.ts`
  - `?transport=auto|ws|http` URL 参数优先
  - ws 断线指数退避 1s→2s→4s→…→30s，连上复位
  - http 轮询间隔读 `pcDataRefreshSec`（0 = 关闭）
  - 401 → 触发 token 输入
- [ ] **1.6** `src/stores/settings.ts`（`settings` 段 + 本地偏好 localStorage）

**验收**：控制台打印真实快照；断网后 ws 自动重连并恢复；`transport=http` 时按设置间隔轮询。

---

## 四、阶段 2 · 引导页 + 11 套主题（最难，建议优先暴露问题）

**目标**：`index.html` + `app.js` + `boot-adv.js` + 两个 `*-list-view.js` 全量 Vue 化。

### 2A 公共组件
- [ ] **2.1** `components/boot/GrubEntry.vue`（条目行：图标 / 名称 / 标签 / 副文案 / 右侧小字）
- [ ] **2.2** `components/boot/RefindList.vue`（原 `boot-list-view.js`）
- [ ] **2.3** `components/boot/GrubList.vue`（原 `grub-list-view.js`）
- [ ] **2.4** `components/boot/ChannelList.vue`（sdboot / OC / Clover 三通道）
- [ ] **2.5** 置顶逻辑组件化：选中项 sticky（**注意**：必须在真实滚动容器内，且父容器有 `contain: paint`）

### 2B 主题组件化（原 `boot-adv.js` 的 `THEME_RENDERERS`）
> ⚠️ **核心重构点**：现状是 `bootBuild(l)` **把真实 DOM 节点搬来搬去**。
> Vue 里必须改成「当前主题 + 数据 → 渲染组件」，不能移动节点。

- [ ] **2.6** `pages/boot/themes/index.ts`：主题注册表 + `currentTheme` 切换
- [ ] **2.7** `ThemeA.vue`（A 布局）
- [ ] **2.8** `ThemeB.vue`
- [ ] **2.9** `ThemeC.vue`
- [ ] **2.10** `ThemeG.vue`
- [ ] **2.11** `ThemeI.vue`
- [ ] **2.12** `ThemeJ.vue`（移动端重点）
- [ ] **2.13** `ThemeK.vue`
- [ ] **2.14** `pages/boot/BootAdvSettings.vue`（原 J 的设置栏）
- [ ] **2.15** `advState` 持久化（`localStorage["anyboot.boot-adv"]`）

> 说明：现有主题是 A~K 但有 11 个键，实际渲染器为 A/B/C/G/I/J/K 七个 + 若干复用；
> 2.7~2.13 按实际 `THEME_RENDERERS` 键逐个建，多出来的补在同编号。

### 2C 主窗体
- [ ] **2.16** `pages/boot/index.vue`（原 `index.html`：页签、系统卡片、通道区）
- [ ] **2.17** 响应式：`.vw-900/.vw-760/.vw-520` → `useResizeVw.ts` composable
  - ⚠️ 现有实现把类**挂在被观察元素自身**（不是祖先），迁移时保持等价
- [ ] **2.18** `styles/`：从 `styles.css` 拆出 tokens / base / themes，其余进各组件 `<style scoped>`

**验收**：逐条过「五、验收清单 · 引导页」全部项目。

---

## 五、阶段 3 · hub 主界面（14,921 行，最大块）

> 拆分原则：**按功能域切，不按代码位置切**。现有 `hub.html` 内联脚本约 8,236 行 + 1,740 行两块。

- [ ] **3.1** `pages/hub/index.vue`：外壳（侧栏导航、页签、状态栏）
- [ ] **3.2** `components/hub/ServerList.vue`（服务器列表渲染）
- [ ] **3.3** `components/hub/ServerEditDialog.vue`（添加/编辑弹窗 + 地址编辑）
- [ ] **3.4** `components/hub/AddressEditor.vue`（识别框 + 已添加列表，服务器/电脑弹窗共用）
- [ ] **3.5** `pages/hub/ComputerList.vue`（电脑页列表：在线状态 / 引导项 / 电源指令）
  - 含：`cqLoadBoot` / `cqRenderRows` / 数据刷新定时器 / 连续失败阈值 / 滚动保持
- [ ] **3.6** `components/hub/ComputerRow.vue`（单台电脑卡片）
- [ ] **3.7** `components/hub/PcMenu.vue`（⋮ 菜单：数据刷新 / 重探模式 / 失败阈值 / 离线阈值）
- [ ] **3.8** `pages/hub/PcView.vue`（全屏内嵌 iframe + 失败面板 + 重试）
- [ ] **3.9** 局域网电源路由（唤醒 / 重启 / 关机 / 睡眠）
- [ ] **3.10** 详情区 + 延迟趋势 + 测试连接
- [ ] **3.11** 隧道（M4）
- [ ] **3.12** 终端 / 文件 / 密钥 / 会话页
- [ ] **3.13** 系统徽标 + 系统信息弹层
- [ ] **3.14** 传输页（SFTP 队列 / 断点续传 / 历史）
- [ ] **3.15** 关于页 + 更新检查

**验收**：逐条过「五、验收清单 · 电脑列表」+「通用」。

---

## 六、阶段 4 · 独立窗口

- [ ] **4.1** `pages/term/`（原 `hub-term.html` 695 行，xterm.js 原样封装）
- [ ] **4.2** `pages/keys/`（原 `hub-keys.html` 465 行）
- [ ] **4.3** `pages/files/`（原 `hub-files.html` 1,262 行）
- [ ] **4.4** `pages/backup/`（原 `hub-backup.html` 1,057 行）
- [ ] **4.5** 验证四个独立 Tauri 窗口的 URL 与路由

**不迁**（已确认）：`ui/tray-menu.{html,js,css}`、`ui/notify.{html,js,css}`、`ui/menu-anchor.html`

---

## 七、阶段 5 · 切换与清理

- [ ] **5.1** `tauri.conf.json`：`frontendDist` → `../ui-vue/dist`
- [ ] **5.2** `src-tauri` 里对 `ui/` 的硬编码路径排查（`include_str!` 之类）
- [ ] **5.3** 更新 `package.json` 的 `build:*` 脚本（加 `npm run build:vue` 前置）
- [ ] **5.4** 打 tag `pre-vue-migration` → 删除旧 `ui/`（保留 tag 可随时找回）
- [ ] **5.5** 全量回归：`cargo test`（当前基线 **508 passed / 15 ignored**）+ 手动过一遍验收清单
- [ ] **5.6** 重新打包 macOS / Windows / Linux 三平台并验证

---

## 八、验收清单（迁移后必须逐条实测）

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

---

## 十、阻塞问题

> 当前无。

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
