# ui/ 全量迁移到 Vue 3 方案（2026-11-02 立项）

> 决策人：用户
> 技术选型：**Vue 3 + TypeScript + Pinia + Vite**
> 主题策略：**11 套引导页主题全部组件化**
> 过渡策略：**一次性替换**（不分新旧共存）

---

## 一、为什么要迁

现状是 **28,238 行手写 DOM 操作**，近几天排查的故障几乎都能追溯到这一点：

| 故障 | 根因 | Vue 下 |
|---|---|---|
| 快照刷新弹回顶部 | `replaceChildren` / `textContent=''` 全量重建 | keyed diff 自动保持 |
| 列表闪烁 | 每拍全量重绘 | 只更新变化节点 |
| 定时器堆叠 | 手写 `setInterval` 无生命周期 | `onUnmounted` 自动清理 |
| 状态分散 | 全局变量散落（`pcAutoReloadSec`、`cqFailCount`…） | Pinia 集中管理 |
| 选择器失效 | `.vw-760 .foo` 不匹配、行内样式压过 CSS | 组件内 scoped 样式 |

---

## 二、迁移范围（完整清单）

| 文件 | 行数 | 目标 | 难度 |
|---|---|---|---|
| `ui/hub.html` | 14,921 | `src/pages/hub/` 多组件拆分 | 🔴 极高 |
| `ui/app.js` | 5,258 | `src/pages/boot/` + `src/stores/` | 🔴 高 |
| `ui/styles.css` | 5,013 | 拆进各组件 `<style scoped>` + `src/styles/` 全局 | 🟡 中 |
| `ui/boot-adv.js` | 3,046 | `src/pages/boot/themes/` 11 个组件 | 🔴 **最高** |
| `ui/qrcode.min.js` | 2,297 | 改用 `qrcode` npm 包 | 🟢 低 |
| `ui/hub-files.html` | 1,262 | `src/pages/files/` | 🟡 中 |
| `ui/index.html` | 1,092 | `src/pages/boot/index.vue` | 🟡 中 |
| `ui/hub-backup.html` | 1,057 | `src/pages/backup/` | 🟡 中 |
| `ui/hub-term.html` | 695 | `src/pages/term/`（xterm 原样封装） | 🟢 低 |
| `ui/boot-list-view.js` | 665 | `src/components/boot/` | 🟡 中 |
| `ui/hub-keys.html` | 465 | `src/pages/keys/` | 🟢 低 |
| `ui/grub-list-view.js` | 406 | `src/components/boot/` | 🟡 中 |
| `ui/tray-menu.js` + `.html` + `.css` | 337 | 保留原生（极小，收益低） | ⚪ 不迁 |
| `ui/notify.js` + `.html` + `.css` | 393 | 保留原生 | ⚪ 不迁 |
| `ui/menu-anchor.html` | 22 | 保留原生 | ⚪ 不迁 |

**不迁**：`tray-menu` / `notify` / `menu-anchor` —— 它们是 Tauri 的托盘/通知微窗口，
只有几十行，改成 Vue 反而增加构建产物体积和复杂度。

---

## 三、目标结构

```
ui-vue/
├── index.html                 # Vite 入口（主窗「系统引导」）
├── hub.html                   # Vite 多入口（管理界面）
├── files.html                 # 独立窗
├── backup.html
├── term.html
├── keys.html
├── vite.config.ts             # rollupOptions.input 多入口
├── src/
│   ├── main.ts
│   ├── styles/
│   │   ├── tokens.css         # CSS 变量（从 styles.css 抽 :root）
│   │   ├── base.css
│   │   └── themes.css
│   ├── stores/                # Pinia
│   │   ├── uiState.ts         # /api/ui-state 快照 + 传输通道
│   │   ├── servers.ts
│   │   ├── boot.ts
│   │   ├── channels.ts
│   │   └── settings.ts
│   ├── composables/
│   │   ├── useTransport.ts    # ws / http 轮询切换
│   │   ├── useScrollKeep.ts   # 滚动保持（Vue 下大多不需要，留作兜底）
│   │   └── useResizeVw.ts     # .vw-900/.vw-760/.vw-520 替代
│   ├── components/
│   │   ├── boot/              # 引导列表公共组件
│   │   │   ├── GrubEntry.vue
│   │   │   ├── RefindList.vue
│   │   │   ├── GrubList.vue
│   │   │   └── ChannelList.vue
│   │   └── common/
│   ├── pages/
│   │   ├── boot/
│   │   │   ├── index.vue      # 原 index.html
│   │   │   ├── themes/        # 原 boot-adv.js 的 THEME_RENDERERS
│   │   │   │   ├── ThemeA.vue
│   │   │   │   ├── ThemeB.vue
│   │   │   │   ├── ThemeC.vue
│   │   │   │   ├── ThemeG.vue
│   │   │   │   ├── ThemeI.vue
│   │   │   │   ├── ThemeJ.vue
│   │   │   │   ├── ThemeK.vue
│   │   │   │   └── index.ts   # 注册表 + 当前主题切换
│   │   │   └── BootAdvSettings.vue
│   │   ├── hub/               # 原 hub.html（最大块，需再拆）
│   │   │   ├── index.vue
│   │   │   ├── ServerList.vue
│   │   │   ├── ServerEditDialog.vue
│   │   │   ├── ComputerList.vue
│   │   │   ├── ComputerRow.vue
│   │   │   ├── PcMenu.vue
│   │   │   └── ...
│   │   ├── files/
│   │   ├── backup/
│   │   ├── term/
│   │   └── keys/
│   ├── api/
│   │   ├── tauri.ts           # invoke 封装
│   │   ├── http.ts            # /api/* 封装
│   │   └── types.ts           # 后端 DTO 的 TS 类型
│   └── router/                # 每个 HTML 入口一个独立 router 或不用 router
└── public/                    # 不参与打包的静态资源
```

---

## 四、必须保留的既有行为（迁移验收清单）

这些是近几天修好、**必须在新实现里重新验证**的点：

### 引导页
- [ ] 浮动置顶（`.grub-entry.is-pinned` sticky）在真实滚动容器内生效
- [ ] `contain: paint` 裁掉跨卡片叠压
- [ ] 三通道（sdboot/OC/Clover）容器宽度 / 底色 / 边框 / 滚动条位置完全一致
- [ ] 左右内容撑满卡片（各留 1px 边框）
- [ ] OpenCore / rEFInd / GRUB 顶部各有 4px 间隙
- [ ] 列表首项无多余 margin-top（`.is-group` 的 4px 只对非首项生效）
- [ ] 条目文字完整多行，**任何地方不出现省略号**
- [ ] J 布局移动端：通道栏顶部横滑，列表占满到底部
- [ ] 移动端 ≤760 / ≤520 的响应式（K/A/B/C/G/I/J 逐项验证）

### 电脑列表
- [ ] 读取中：**三条通道都显示「读取中…」**（不能只剩 rEFInd/GRUB）
- [ ] 离线：三条通道都显示「离线不可改」
- [ ] 在线：只显示 detected 的通道，未检测到的收进一行提示
- [ ] rEFInd / GRUB 按 **auto/manual 模式**取对应数据源
- [ ] 数据刷新：**常开**（默认 2 秒，可配 0=关闭），**不受「连不上时自动重载」影响**
- [ ] 失败重试：受 `failReloadSec` 控制
- [ ] 刷新时**复用上次成功的端点**；失败达阈值（默认 3 次）才判离线并重新选路
- [ ] 定时刷新**不驱动**刷新按钮转圈
- [ ] 滚动位置在数据刷新后保持不变
- [ ] 引导器行统一「B：色条双行」（名字首行 + 6dp + 状态行，垂直居中）

### 通用
- [ ] `?transport=ws|http|auto` URL 参数生效
- [ ] WebSocket 断线自动退避重连（1s→2s→…→30s）
- [ ] HTTP 轮询间隔读 `pcDataRefreshSec`
- [ ] token 缺失时弹输入框，401 时重新请求
- [ ] 所有 IPC 快照（`ui-state-changed`）驱动界面更新

---

## 五、分阶段执行计划

### 阶段 0：脚手架（不碰现有 ui/）
1. `npm i -D vue@3 typescript vite @vitejs/plugin-vue vue-tsc`
2. `npm i vue@3 pinia`
3. 建 `ui-vue/` 目录，配 `vite.config.ts` 多入口
4. 跑通一个空白 Vue 页面 → `tauri.conf.json` 指向它 → 验证打包

**验收**：`AnyBoot.app` 能启动，显示空白 Vue 页，IPC 能通。

### 阶段 1：类型与数据层
1. 从 Rust 的 DTO 生成 `src/api/types.ts`
2. 写 `stores/uiState.ts`（快照 + 传输通道 + 轮询）
3. 写 `composables/useTransport.ts`

**验收**：控制台能打出真实快照，ws/http 都能切。

### 阶段 2：引导页（含 11 套主题）
1. 拆 `GrubEntry` 等公共组件
2. 逐个移植 `THEME_RENDERERS.A~K`
3. 逐条过「四、验收清单 → 引导页」

**验收**：11 套主题逐一实测，对照现版本截图。

### 阶段 3：hub 主界面
最大块，建议按功能切：
1. 服务器列表 / 编辑弹窗
2. 电脑列表（卡片、数据刷新、探测）
3. 备份、文件、密钥、终端入口

### 阶段 4：独立窗口
`files` / `backup` / `term` / `keys` 四个 Vite 入口。

### 阶段 5：切换
1. `tauri.conf.json` 的 `frontendDist` 从 `../ui` 改为 `../ui-vue/dist`
2. 删除旧 `ui/`（先打 tag 保底）
3. 全量回归测试

---

## 六、已知风险

| 风险 | 说明 | 缓解 |
|---|---|---|
| **主题 DOM 移动** | `bootBuild(l)` 现在是**把真实 `#refind` / `#grub` 节点搬来搬去**。Vue 里不能用这种手法，必须改成「数据 → 组件渲染」。这是整个迁移最大的重构点 | 阶段 2 先做，暴露问题最早 |
| **14,921 行的 hub.html** | 单文件含 6 个内联 `<script>`，需要仔细拆 | 按功能域切，不按代码位置切 |
| **Tauri 资源路径** | `frontendDist: "../ui"` 改成 Vite 产物后，`tauri://localhost` 下的路径规则要重验 | 阶段 0 就验证 |
| **构建时间** | 从「改完刷新即生效」变成等 Vite 编译 | 用 `npm run dev` + HMR |
| **回归范围大** | 上面验收清单所有项都要重测 | 每阶段结束跑一遍 |

---

## 七、环境前置

- **Node 20.20.2 已装**（`nvm install 20`）；Vite 5 需要 ≥18
- Tauri CLI 2.11.4 在新 Node 下可用
- 注意：`npm run build:mac-intel` 等脚本用 `env -u CARGO`，迁移后要确认 Node 版本仍对
