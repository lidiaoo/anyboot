# 窗口行为细节（贴边自动隐藏 / GRUB 独立窗 / 复现台 / 诊断日志）

> MEMORY.md 只放「每次都用得上」的规则与命令；这份放**细节与实测数字**，
> 需要时读它（按需，不要每次都读）。历史沿革见每日日志 `YYYY-MM-DD.md`。

## 主窗口贴边自动隐藏
机制：守护线程每 120ms 轮询 `AppHandle::cursor_position()`（这个 tao 版本没有
CursorEntered/Left）；光标离开 350ms（`AUTO_HIDE_DELAY_MS`/`TICK_MS`，`dock_delay_ticks=350/120=2`）
→ `dock_main_window` 缩成 40px 缝（`DOCK_STRIP_SLOT_PX`）贴最近边；光标压到缝上 → undock。
`set_decorations(false)` 去装饰（带标题栏的窗口有最小高度钳制，缩不成条）+ `set_shadow(false)`。
**Wayland 做不了**（`cursor_position` 恒 (0,0)，客户端不能自定位）——用户机是 Cinnamon **X11**。

### 坐标系语义（读 tao 0.35.3 源码 + 实测，别记反）
| API | 坐标系 |
| --- | --- |
| `set_position` / `set_size`（→`gtk_window_move`/resize） | **客户区** |
| `inner_position()` / `inner_size()`（configure 事件，去装饰后仍准） | **客户区** |
| `outer_position()` / `outer_size()`（`root_origin()`+`frame_extents()`） | **窗框** |
| `WindowEvent::Moved` 的载荷（同为 `root_origin()`） | **窗框**（≠客户区！） |
实测：带装饰 inner=100/outer=63；停靠条 inner=960/outer=923（恒差一个标题栏 37px）。
`set_outer_position` 只往主线程投一条异步请求 → **发完请求 ≠ 几何已落位**。
去装饰后 GDK 的 frame extents 缓存**不再刷新**，`outer_position()`/`Moved` 载荷持续偏一个
标题栏；`_NET_FRAME_EXTENTS` Muffin 也不更新（判「去装饰生效没」要用 `_MOTIF_WM_HINTS`）。
**定则：贴边逻辑判位一律用 `inner_position()`（`main_client_position()` 是唯一入口）。**
混坐标系曾造成三处 Linux 独有 bug（已修，均有实测）：① 停靠态判据用 Moved 载荷恒偏 37px>
容差 24 → 任何 Moved 都被当成「用户拖走白条」；② `undock_target` 按窗框算却喂给客户区语义的
`set_position` → 展开偏高一个标题栏（抽成纯函数 + 单测 `undock_target_sticks_client_area_to_work_edge`）；
③ `strip_geometry` 的「另一轴保持原位」传的是 `outer_position()` → 每贴一次漂一截
（复现台 `LEDGE=1` 钉它）。Windows 上 `outer == inner`（SetWindowPos 就是窗口矩形）所以看不见。
`xdotool windowmove` 是**窗框**语义（请求 `4 50` → 窗框 50/客户区 87），做实验要拿日志的 `inner=` 当真值。

### 其它已定案的坑
- 启动后 4~5s 窗口自己动一下 = **Muffin 按自己的记录重摆**（窗框 y 31→63；`moved diag` 前后
  没有任何应用侧落位行，也没装 window-state 插件）→ 排查时别再顺着我们的 set_position 找。
- 在**可见**窗口上做「去装饰+改尺寸+落位」会被吃掉 position（重父化让同批 set_position 失效，
  窗口停在旧窗框原点约 420ms 才被回钉兜底）→ **正确姿势 `hide()` → 改几何 → `show()`**，
  且 monitor/target 必须在 hide 前算完（不可见时 `current_monitor()` 退化成主显示器）。
- 排查「一闪而过」必须 ≥60ms 细采样：250ms 采样正好跳过 400ms 量级的错位窗口，看起来「一切正常」。
- `geometry_busy`（`mark_geometry_busy`，400ms，**必须在发几何请求之前**立起来）
  = 「我们自己刚编程过几何，读到的坐标/Moved 是中间态」。
- 启动哑火：`show_main_window()` 原先无条件挂 `pending_enter=true`，而**启动首次显示也走它**
  → 用户碰过窗口之前一律拒绝贴边（Windows 上用户总先拖窗口所以看不出）。修法：
  `ShowOrigin::{Startup,Recall}` 枚举（裸 bool 在 6 个调用点太容易传反），只有呼出路径挂保护；
  两种来源都要 800ms 宽限（`AUTO_HIDE_SETTLE_MS`）。
- 「自己造成的回声」哨兵必须**持久**：一次 `set_position` 在 X 上产生**多条同坐标** Moved，
  「匹配一次就清空」的哨兵必漏第二条 → 被当成用户动了窗口 → 反拽另一个窗口（实测一次拖动反拽
  900+ 次，主窗被从 x=1136 拽到 192）。判据 = 「与最后一次要求它去的位置一致(±8px) 就一直算回声」
  （`grub_move_is_our_echo` / `grub_resized_by_user`，取代早先的 600ms 时间窗）。
  `MAIN_FOLLOW_TARGET` 是一次性写法但**不动**（fall through 后走幂等的 place）。
- 呼出（点托盘）路径靠诊断钩子可自动化：`ANYBOOT_AUTOHIDE_DIAG=1` +
  `ANYBOOT_AUTOHIDE_DIAG_RECALL_MS=<ms>` → 启动 N ms 后走真正的呼出（不设变量直接 return）。
  实测：贴边→呼出展开→光标仍在窗外 4s 也不贴回→进过窗口再离开→250ms 内重新贴边。

### 贴边/展开的「拖动保护」（回合 31 修，用户报「从左右缘左右拖动 → 主界面变长 + GRUB 窗乱闪乱跳」）
- 复现台 `DOCKPOKE=1`（光标沿缘来回扫 + 抓边条往工作区拖）实测根因：抓白条拖离 →
  `restore_main_dock_size` 展开成 460x900（用户看到的「变长」）→ **176ms 后** watcher 又按
  拖动途中的「光标在窗外」把它缩回条子，GRUB 独立窗 129ms 内翻两次（「乱闪乱跳」）。
- 根因：**undock 分支漏了 dock 分支早有的「几何在飞就放手」**（dock 侧是 `docked_poll_action`
  的 `busy → Hold`）；且「离窗时钟」在拖动途中照常累计（WM 拖窗口/白条时 Moved 连发，
  那一刻的「光标在窗外」是拖动中间态，不是用户离开了）。
- 修法：`undocked_counts_ticks` 从 3 个否决条件扩到 5 个，新增
  `geometry_in_flight = geometry_busy()` 与 `move_quiet = main_moved_recently()`
  （`LAST_MAIN_MOVED_MS` 在 `handle_main_moved` 入口每次盖戳；静默期
  `AUTO_HIDE_MOVE_QUIET_MS = 300`）。诊断行 `undocked: left window … ` 增打 `geo_busy= move_quiet=`。
- 验收（同一判据，见下）：主窗尺寸振荡 1→**0** 次；GRUB 振荡 1→**0** 次；`DRAG=1` 回归 0 振荡、
  条子/展开尺寸全程稳定 40x900 / 460x900。

## GRUB 独立窗的尺寸/位置双向绑定
- 两个方向都是「事件回调只报名、80ms 去抖线程真正 set_size/set_position」（主窗变 →
  `schedule_grub_align`；用户拉 GRUB 边框 → `schedule_main_size_from_grub`；位置同理
  `place_grub_list_window` / `follow_main_to_grub`）。**绝不在事件回调里同步 set_size**（重入消息循环挂死）。
- **tao 0.35.3 的 `outer_size()` 对「还没映射过」的窗口返回 `0x0`**（初值取的是坐标）。
  GRUB 窗 `.visible(false)` 建出来 → 边距 = 外框−内框 = 负数 → 目标尺寸算成 `900x1257` → 被 WM 钳住
  的 configure 回声又被当成「用户拉了边框」→ 反推主窗尺寸 = 用户报的「主界面变长」。
  修：所有「由读数算目标尺寸」都走纯函数（`grub_align_target`/`main_size_from_grub`），
  读到 0/负边距就 `None` 不改尺寸（位置照摆）；第三处同根因是建窗初始尺寸
  （`grub_initial_size_basis` 优先取可用读数：**内尺寸未映射时就可读**）。
  **凡是拿窗口尺寸当基准，都要问「这个读数在未映射时可信吗」。**
- 主窗最大化/撑得太宽时**摆不下**（两侧都放不进工作区）→ 纯函数 `grub_placement` 返回 `None` →
  调用方**藏起来**（别再压在主窗上，也别每 120ms 开→藏循环）。`snap-4` 实测：1600 宽屏上
  主窗 800 宽时右侧 `800+800+12>1600`、左侧 `<0` → 藏；恢复正常尺寸后自动回来。
- 残留（已上报未改）：启动时主窗被拽 ~132px —— WM **忽略未映射窗口的位置请求**，
  那条首帧 Moved 被当成「用户拖 GRUB」。干净修法要引入「落点被观测到之前不认用户操作」的新状态，
  有吞掉真拖动的风险，按纪律先上报。只出现在「刚建/刚 show 的窗口」。

## e2e 复现台（Linux/X11）
`/home/jianghh/.cache/ab-test/run.sh`（副本 = skill `linux-x11-window-repro/scripts/repro-edge-hide.sh`）：
`dbus-run-session -- bash run.sh <anyboot 路径> [标签]`。**新增场景后记得同步回 skill 副本。**
- 环境：Xvfb `-extension GLX`；WM 用 `muffin --x11 --sm-disable --replace`（用户桌面本尊）；
  HOME 指向可写目录；**`export LANG=C.UTF-8 LC_ALL=C.UTF-8`（必须）**：POSIX locale 下 X 无法把
  中文标题转成 WM_NAME → `xdotool search --name GRUB` 一条都搜不到 → 采样全是「无 GRUB 窗口」→
  段数 0/1 看着像「全程没闪」= 最好的结果，实际什么都没测到（漏了几轮）；
  每行打 `date +%s%3N` 与日志对齐；几何一律 `xwininfo` 解析（`xdotool getwindowgeometry` 的
  前导空格让 `awk -F'[ x]'` 拆出空字段 → 面积恒 0）；截图 `import -window root`。
- 场景开关：`ENTER=0/1`、`NUDGE=1`（停靠态挪 10px 验回声判据）、`LEDGE=1`（贴左边验「另一轴」）、
  `RECALL=1`（模拟呼出）、`GRUBVIEW=window|inline`（**默认 inline！**涉及 GRUB 独立窗的 bug 必须
  传 `window`）、`DRAG=1`、`GRUBDRAG=1`、`MAXIMIZE=1`、`SNAP=1`、`DOCKPOKE=1`、
  `POS=center|free`（验呼出保护必须 `free`）。
- **判「乱闪乱跳」只认振荡判据**（`report_flicker`）：状态在 <600ms 内回到两拍之前的值
  （`A→B→A`）。段数（正常一轮 dock/undock 也切 1 次 → 曾 17 段全是正常节奏）与「相邻切换
  <300ms」（拖窗平移时跟随窗每 70~80ms 一条同方向新位置 → 26 条）都是**错的量具**。
  实现坑：`A→B→A` 的时长取 **B 的存活时间**（`$1 - pt`），不是 `t2`（A 开始的时间）——
  写错过一次，修复前的 bug 因此被判成「无振荡」。
- **一次都没采到窗口时必须显式判「本场景无效」**，不能把「切换 0 次」当通过。
- 场景动作必须真能进目标分支：光标扫出要**跨出「主窗 ∪ 跟随窗」并集**（只扫到 700px 还在
  跟随窗矩形里 → `inside` 恒 true → 贴边一次没触发 → 「通过」其实是没测）；抓边条要先确认
  **真的成条**再抓（否则抓的是整窗客户区，什么都不会发生）。

## 诊断日志约定（`startup-debug.log`）
- `startup_debug()` **无条件写文件**，「默认关」全靠调用方套 `if auto_hide_diag_enabled()`
  （环境变量 `ANYBOOT_AUTOHIDE_DIAG=1`）。新增诊断行必须审两件事：① 有没有漏套开关
  （污染用户日志）；② **有没有把功能代码误关进开关块**（=「只在设了环境变量时才正常」的隐形 bug）。
  审法：按花括号配平抓出每个开关块，确认块里只有 `startup_debug`；再实跑一次
  `ANYBOOT_AUTOHIDE_DIAG=0`，确认新诊断行 0 次、对照组日志仍在写。
- 关键行：`dock begin edge=… from=… size=… target=…` / `undock edge=… restore=… target=…` /
  `docked: snap back (obs) -> (target)` / `docked: cursor on strip -> undock` /
  `moved: docked + geometry busy -> hold` / `moved: docked but off dock_pos … -> release` /
  `restore dock size -> WxH` / `undocked: left window … near_edge= grace_left_ms= pending_enter= geo_busy= move_quiet=` /
  `autohide diag: docked= pos= size= inner= cursor= inside= near_edge= outside_ticks= …`（每秒一行心跳，
  定位「为什么没贴边」全看它）/ `moved diag: event=(…) outer=(…) inner=(…)`（每条 Moved，
  用来分清「这条位移是谁发的」）/ `place grub -> (x,y) main=(…)` 或 `place grub: 摆不下（…），藏起来`。
- 日志是给用户回贴的，别塞逐拍噪声（心跳行只在开关打开时写）。


## 回合 32：GRUB 展示四模式（单窗口）取代三模式（含 OS 独立窗）
- 四模式全在主窗口里：inline(460)/wide(932,列表两列)/list(932,仅列表进右栏)/pane(932,GRUB 全在右栏)。
- 老配置值 `window` 由 `pane` 承接（normalize_grub_list_view）。
- 右栏 .pane-grub 是空壳；app.js applyGrubLayout 按模式搬 #grub 子节点
  （出厂序 grubCanonicalOrder 捕获一次，回搬复原）；body[data-grub-mode] 驱动 CSS。
- 后端只管：设置落盘 + 窗口宽度（apply_grub_layout_size：inline→460 其余→932），
  setup 启动也调。回合 31 删掉的 OS 独立窗绑定代码不要找回。
- Xvfb 截图全白 = WebKitGTK 不上屏（改前就白），别拿截图当渲染证据。
