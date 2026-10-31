# GRUB 数据口径规范：严格按模式、零回落

> **定案时间**：2026-10-31，用户原话「grub 万万不要有这个回落逻辑」。
> **本文档目的**：把这条铁律和它的全部落地点写清楚，避免以后任何人（包括未来的自己）在改代码时又"好心"加回落，导致错误数据被展示出来。
>
> **一句话规则：自动模式只认 Linux 分区扫描那份（`linux_grub`），手动模式只认 ESP 那份（`grub`）。没扫到就是空 —— 绝不拿另一份垫底。**

---

## 1. 为什么绝对禁止回落

回落（fallback）指的是：当前模式对应的数据源为空/缺失时，"退而求其次"去用另一份 GRUB 数据展示。这看起来"健壮"，实际上是**给用户看错误数据**：

| 场景 | 回落导致的错误展示 |
|---|---|
| 自动模式 + 从未点过「重新读取」 | 「下次启动」大字凭空出现 ESP 里的 GRUB 条目，但引导列表却是空的 → 两处互相矛盾 |
| 自动模式 + Linux 分区没扫到项 | 列表计数显示 ESP 的 N 项，用户以为"自动读取扫到了"，实际一份都没读到 |
| 手动模式 + ESP 还没解析 | 展示了自动扫描那份旧数据，用户以为手动解析已完成 |
| 移动端电脑列表 | 手机和 PC 主窗各说各话：PC 显示空、手机显示有项（或反之） |
| 主题页 GRUB 标签计数 | 计数与当前模式下的列表对不上，出现"列表空但计数 5" |

历史上这个回落逻辑至少被实现过一次、又被用户明确否决过一次（2026-10-31，提交 `3ac39b8` 撤回落改零回落）。**以后任何"数据为空时用另一份补"的想法，默认都是错的。**

---

## 2. 唯一正确的口径

```
grub_mode == manual  →  view = state.grub        （ESP / grubenv 快照，原样）
grub_mode == auto    →  view = state.linux_grub  （Linux 分区自动扫描那份）
                        · Some(l) → 整块换上（空也换）
                        · None    → 等于空：entries=[], next/saved=None
                        · 空列表时 error 必须带真实提示（见 §3.1）
```

要点：

1. **整块替换，不是逐字段合并**。entries / groups / next_entry / saved_entry / error 五个字段一起换，不留一半旧一半新。
2. **空也是数据**。`None`（还没读过）和 `Some(空列表)`（读了没扫到）都忠实展示为空，并配对应的 error 提示文案。
3. **模式一切，视图立刻跟着切**。不存在"模式切了但展示还停留在旧模式数据"的中间态。

---

## 3. 全部落地点（五处消费点 + 一条字段规则）

改任何一处，先对照这张表检查其余各处是否仍然同口径：

| # | 消费点 | 文件 / 函数 | 规则 |
|---|---|---|---|
| 1 | 后端托盘 + 主窗「下次启动」 | `src-tauri/src/menu_model.rs` → `effective_grub` / `overlay_linux_grub` | §3.1 |
| 2 | 远程接口 | `src-tauri/src/remote_routing.rs` → `remote_grub_json` | §3.2 |
| 3 | 主窗「下次启动」大字 | `ui/app.js` → `effectiveGrubForNextBoot` | §3.3 |
| 4 | 主题页引导列表 / GRUB 计数 | `ui/boot-adv.js` → `grubLoader`（`demo/boot-adv.js` 是其同步副本） | §3.4 |
| 5 | 移动端电脑列表 GRUB 启动项 | `mobile/android/app/src/main/java/com/anyboot/remote/ComputerListActivity.kt` → `parseGrub` | §3.5 |

另外两份"影子副本"必须同步：`demo/hub.html` ↔ `ui/hub.html`、`demo/boot-adv.js` ↔ `ui/boot-adv.js`。

### 3.1 `overlay_linux_grub`（Rust，托盘 / 下次启动 / /api/grub 的共同内核）

```rust
if manual { return; }                    // 手动：ESP 原样，一个字都不动
match linux {
    Some(l) => {
        // entries / groups / next_entry / saved_entry / error 整块换上
        // 空列表且 error 为空时 → 补真实口径的 error 文案
        if view.entries.is_empty() && view.error...is_empty() {
            view.error = Some("自动读取没扫到 GRUB 启动项（在电脑上点「重新读取」）");
        }
    }
    None => {
        // 还没读过：等于空 —— 绝不回落 ESP
        view.entries = Vec::new(); view.groups = Vec::new();
        view.next_entry = None; view.saved_entry = None;
        view.error = Some("自动读取还没执行（在电脑上点「重新读取」）");
    }
}
```

- error 文案是**零回落的配套**：自动模式空列表必须告诉用户"为什么空、下一步点哪"，否则消费方（尤其移动端）会在 error 为空时退回「这台电脑没部署 GRUB」——在自动模式下这是误导。
- 只覆盖上述字段，**不整份重建** `GrubView`（它有三十多个字段：挂载状态、脚本日志、安装状态…），挂载信息（`efi_dir`）按 `target.dev` 非空才覆盖。
- 测试锚点：`menu_model.rs` 的 `overlay_linux_grub_follows_the_read_mode`（manual 原样 / auto+有条目整块换 / auto+空→空+error / auto+None→空 四组断言）。**改这里的逻辑必须同步改测试。**

### 3.2 `remote_grub_json`（Rust，`/api/grub`）

`installed` 字段**按模式如实上报**（零回落配套）：

```rust
"installed": if settings.grub_mode.is_manual() {
    view.installed            // 手动：才是「ESP 上装没装 09_anyboot」
} else {
    Some(!view.entries.is_empty())   // 自动：列表只认扫描那份，ESP 安装态与列表无关
},
```

原因：移动端用 `available = installed || entries 非空` 判可用性。自动模式下若把 ESP 的 `installed=true` 原样透传，空列表会被误判成"GRUB 可用"。

### 3.3 `effectiveGrubForNextBoot`（`ui/app.js`，主窗下次启动大字）

```js
if (state.settings && state.settings.grubMode === "manual") return grub;   // 手动：ESP 原样
const linux = state.linuxGrub || {};
return Object.assign({}, grub, {
  entries: Array.isArray(linux.entries) ? linux.entries : [],   // 缺失/非数组 → 空，不回落
  groups: linux.groups || [],
  nextEntry: linux.nextEntry || "",
  savedEntry: linux.savedEntry || "",
});
```

### 3.4 `grubLoader`（`ui/boot-adv.js` / `demo/boot-adv.js`，主题页引导列表与计数）

```js
const useLinux = mode !== "manual" && linuxHasEntries;   // 严格：不满足就用 ESP 视图但
const view = useLinux ? linux : esp;                     // 自动模式空时 detected=false、
                                                          // healthText 报「重新读取」
```

- `detected = entries.length > 0` —— 自动模式没扫到就是"未检测到"，计数 0。
- `healthText` 四种口径：自动+有项 → 分区名 + N 项；自动+空 → 「还没读过：点重新读取」；手动+有项 → `ESP · N 个启动项`；手动+空 → 「还没解析：点重新解析」。**绝不在自动空时拿 ESP 的 N 项来报。**

### 3.5 移动端 `parseGrub`（Android，电脑列表 GRUB 启动项）

- 客户端**零回落**：每次刷新用 `/api/grub` 响应**整块替换**本地缓存，不保留上一轮数据，也不做"空时沿用旧数据"的兜底。
- `available = installed || options 非空`（服务端已按模式上报正确的 installed，见 §3.2，客户端不再自行判断）。
- `options` 为空时 note 优先显示服务端 `error` 字段（即 §3.1 里服务端给的真实口径文案），仅在 error 也为空时退「这台电脑没部署 GRUB（或还没同步 grub.cfg）」——这不是数据回落，是文案兜底，是允许且必要的。

---

## 4. 禁止的写法（反例集合）

看到下面任何一种代码出现在 GRUB 相关路径里，直接视为 bug：

```js
// 反例 1：空时回落另一份数据
const view = (useLinux && linux.entries.length) ? linux : (linux.entries.length ? linux : esp);
//                                   ↑ esp 出现在自动模式的数据链路里 = 回落

// 反例 2：entries 用 || 合并两份
entries: linux.entries?.length ? linux.entries : (grub.entries || [])

// 反例 3：available / installed 判断把另一份的安装状态混进来（自动模式下）
available = installed || entries.length > 0   // 当 installed 来自 ESP 时

// 反例 4："数据没到先展示旧模式的"，缓存跨模式复用
if (!newData) return cachedOldModeView
```

同理 Rust 侧：

```rust
// 反例：auto 分支在 linux 为 None/空时 return state.grub.clone()（原 ESP）—— 这正是被否决的回落
```

---

## 5. 改动自查清单

凡是动到 GRUB 数据流（新增消费点、改字段、改模式语义），逐项打勾：

- [ ] 托盘「下次启动」在自动模式空数据下不出现 GRUB 条目？
- [ ] 主窗「下次启动」大字同上？
- [ ] 主题页引导列表：自动空 → 空列表 + 计数 0 + healthText 提示「重新读取」？手动空 → 提示「重新解析」？
- [ ] `/api/grub` 的 `installed` 是否按模式上报（auto → `entries 非空`）？
- [ ] 移动端电脑列表：刷新后整块替换、空列表 note 显示服务端 error？
- [ ] `demo/hub.html`、`demo/boot-adv.js` 两份影子副本是否同步？
- [ ] `overlay_linux_grub_follows_the_read_mode` 测试是否仍覆盖新行为？
- [ ] 新增消费点是否也走了 `effective_grub` / `effectiveGrubForNextBoot`，而不是直接读 `state.grub`？

**最容易犯的错**：新增 UI 直接 `state.grub` / `state.grub.entries` 取数 —— 必须走 `effective_grub`（Rust）或 `effectiveGrubForNextBoot(state)`（JS），否则模式语义丢失。

---

## 6. 历史教训（防重演时间线）

1. 首版实现"自动模式空时回落 ESP"，用户看图发现下次启动区与列表矛盾 → 否决。
2. 修"下次启动大字"时只修了主窗一处、没审主题页 grubLoader → 用户截图抓到主题页计数仍是 ESP 口径（"你到底审查代码没有"）。**教训：改动必须全链路五处一起审，包括影子副本与移动端。**
3. 定案零回落后又发现两处配套缺口：`/api/grub` 空列表没带 error、`installed` 未按模式上报 → 移动端会误判"没部署 GRUB"。**教训：零回落不只是"不展示"，还包括"空时必须给真实口径的提示"。**
