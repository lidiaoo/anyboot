# AnyBoot 托盘细节备忘（classic / unified 双形态 + Linux dbusmenu）

> 本文件是 `MEMORY.md` 的细节延伸（托盘侧）。当前基线 = `440189c` 线（`9310876`）。
> 注意：与 `backup/old-stack-0d43626` 分支的「GNOME 回 libayatana」方案相反 —— 那是备选，main 走自研 `linux_sni`。

## 形态选择
`trayMenuStyle` 二选一，**启动时定接线、切换需重启**：

- **classic（默认）= Win/mac 风格**：tauri 托盘 + muda 原生菜单。
  Windows：摘掉菜单 + 锚点窗 `popup_tray_menu` 现建现弹。
  **Linux：不建 tauri 托盘，走自研 `linux_sni` 全量 dbusmenu**（`8c96da4` 起，绕开 muda 0.20 的 GTK3 子菜单渲染回归）。
  `check_mark_*` 绿勾位图、`menu_signature` 菜单指纹链是 classic 依赖，**别再删**。
- **unified = Win/mac 风格**：右键直开自绘窗（`tray-menu`）。
  Linux：自研 SNI 导出 dbusmenu ——
  - **GNOME**：壳菜单只有 `[显示主窗口, 退出程序]` 两条（非空弹窗才发 `opened`）；`opened` → 弹自绘窗 + 150ms 合成 Esc 释 grab。
  - **Cinnamon / KDE**：全量 dbusmenu 由宿主渲染。

## 自绘窗落位（`9310876` 定案）
- **unified**：优先贴托盘图标**正下方**（水平对图标居中）；放不下翻**正上方**；**绝不盖图标**。
- **classic**：上方优先 + 左对齐。
- 图标矩形用**标称 32 物理 px**（`TRAY_MENU_ICON_NOMINAL`）。
- **`TrayCenter` 已弃用**（固定往上摆、Linux 不翻面）；positioner 插件与喂点（`on_tray_event`）保留但仅作插件状态维护，**落位全手算**。capabilities 需加 `positioner:default`。

## 失焦看门狗（`9310876`）
收起的主路径 `Focused(false)` 有两个漏点：假失焦被宽限吞掉后焦点已丢、不再来事件；`set_focus` 偶发失败从未有过焦点。
→ `spawn_tray_menu_watchdog` 250ms 轮询：拿到过焦点又稳定 400ms 无 → 收；从未拿到 → 宽限后重试 `set_focus`，4s 仍无 → 收（GNOME 点击必聚焦，点在菜单上必有焦点）。runtime 焦点账本 open/close 清零。

## GRUB 区
`grub_menu_nodes` 对齐主窗列表逻辑（置顶当前项 + 原位保留、组头 ▸ + 系统图标 + N 项 [+ESP 位置]、条目文字随 `grub.listFormat`、非原始格式按渲染结果折叠「N 个变体」）。
**同一份函数喂四处**：主窗、unified 自绘窗、Linux classic dbusmenu、Win/mac classic 原生菜单（`c522443` 起 `build_tray_menu` 只做「节点 → 原生控件」翻译，不再有第二份实现）。**改它三头都变**。
行文字顺序 = **箭头在前、图标在后**（`↳ 🐧 linux`），与主窗 `makeRow` 逐字对齐；`menu_signature_for` 的 GRUB 段直接用 `grub_menu_nodes` 的 JSON，别再自己数。
`keepsOpen` 只在后端算，前端照字段办事；分发统一走 `handle_menu_event`。

## Linux SNI / dbusmenu 定则
- `ItemIsMenu=false` + `Menu=<object path>`（**类型错 GNOME 静默丢属性**，二者缺一菜单展不开）。
- **信号必须从 `Item.Menu` 指向的对象路径（`MENU_PATH`）发** —— 从 `ITEM_PATH` 发会被所有宿主按路径匹配静默丢弃。
- 回显走 **`ItemsPropertiesUpdated` 属性差分推送**（`menu_property_diff` 回「结构变?, 属性更新」按结果分流）：只属性变 → 只推属性；只结构变 → 只发 `LayoutUpdated`；**都没变 → 什么都不发**（`invalidate_menu_signature` 每次点击强制换代；无差别广播 = 更新风暴）。
- **信号体签名必须是 `a(ia{sv})a(ias)`** —— `removedProps` 是 `a(ias)`（规范 / Chromium / libdbusmenu-qt / Go systray 一致）。写成 `a(iias)` 会被 Qt 宿主按签名精确匹配丢弃（GNOME 走 GDBus 按名字匹配，所以不报错、只表现为「点了没反应」）。
- **导出禁带原生勾选（`17baf39`）**：`toggle-type` / `toggle-state` 一律不下发 —— 宿主（KDE/Qt）对 checkable 条目先本地翻转再通知，协议又没有单选组概念 → 点几个勾几个（KDE 实测「单选点不动 / GRUB 变多选」）。勾选标记一律用 `radio_label` 画进 label 文字（`◉`/`○`）；快照单测钉死 label 断言 + 无 `toggle-*`。
- **`AboutToShow` = 真脏标记**（`MENU_DIRTY` 只在 `GetLayout` 应答清零，`440189c`）—— 恒 `true` 会把展开瞬间打回；`LayoutUpdated` 防重发靠 `MENU_SIGNAL_REVISION`。
- 宿主行为差异：GNOME 扩展点击只认 dbusmenu；Cinnamon/KDE 见有效 `Menu` 会自建宿主菜单消费右键（**`ContextMenu` 回调不来了**）。
- 假宿主探针：`probe27-dbusmenu.py` / `run-probe27.sh`（GNOME 流程：Menu 属性 → GetLayout → Event）、`probe-sni-click.py`（Cinnamon 菜单点击）。`RegisterStatusNotifierItem` 参数以 `/` 开头时服务名 = sender 唯一名；`Event` 不检查 `enabled`；`Event` 的 id 兼容 i32/u32。
- **`libayatana` 不暴露 `ItemIsMenu` 属性**，探针要容错。
- dbus 0.9 坑：`MessageItemArray::new` 要整条数组签名（带前导 `a`）；`Message::signal(path,iface,name)` 直接返 `Message`（失败 panic）、`Connection::send` 返 `Result<u32,()>`；`GetLayout` 回执 `(u(ia{sv})av)` 有单测钉死；快照父节点先 push 再递归（先序，否则 id 错位）。
- Tauri 2 ACL：新窗口 label 必须进 `capabilities/default.json` 的 `windows`，否则 invoke/listen 静默拒。

## macOS 原生对话框不可用（`c522443` 实测）
rfd 无 parent 时走 `CFUserNotificationDisplayAlert`，本机只登记不显示（UserNotificationCenter 多一个 4×3 / alpha=0 空窗，调用方阻塞 ~30s 才返回）。
要弹提示走：`osascript -e 'display dialog … buttons {"好"} default button 1 with icon note giving up after 60'`（实测 420×132 可见、无权限弹框），与电源动作同源。

## 待办
本机无 win/mac target → **上 Windows 先 build 再实机点托盘复验（两形态）**，然后 push、打包。
