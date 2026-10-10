# UI 改进 · 执行清单

## 功能需求

### 1. 倒计时动态更新 ✅
- [x] 弹窗窗口内部用 `setInterval` 每秒刷新一次 `secondsLeft` 与进度条（`notify.js` 本地 tick）

### 2. 提示音功能 ✅
- [x] `Settings` 新增 `play_sound`（默认 true）/ `sound_file_path`（可配置路径）
- [x] 主窗口「提示音」开关 + 路径输入框 + 应用按钮
- [x] 弹窗进入 pending 时播放一次（自定义文件优先，留空用系统默认）
- [x] 单测：设置默认值 + 老配置缺字段回退默认

### 3. 最近请求排序 ✅
- [x] `push_recent` 把最新条目插到最前；`remote_requests_json` / `HttpView` 输出时 `reverse()` 成时间正序（最旧在上、最新在下）
- [x] 前端渲染完自动 `scrollTop = scrollHeight`，最新一条永远落在视口里
- [x] 单测 `recent_requests_are_newest_first_and_reversed_for_json`

### 4. 打开日志目录按钮 ✅
- [x] `open_config_dir` 命令 + 跨平台 `open_directory_in_explorer`（explorer / open / xdg-open）
- [x] 主窗口「远程控制」区底部「打开配置目录（含 trace.log / settings.json）」按钮

### 5. 打开变量目录按钮 ✅
- [x] `open_vars_dir` 命令（读 AppState.vars_dir，未扫描时给明确报错）
- [x] 主窗口「打开变量目录（rEFInd 变量文件）」按钮

### 验证
- [x] `cargo test` 40 passed、`node --check ui/app.js ui/notify.js` 通过
- [x] 端到端取证（沙箱，端口 12399）：
  - 本程序另一个实例占用 → trace `端口占用者 pid=41076 name=anyboot.exe is_self=false same_app=true`
  - 外部进程占用 → trace `端口占用者 pid=31760 name=python.exe is_self=false same_app=false`
  - 结束占用者后释放端口 → 新实例 `已监听 0.0.0.0:12399`，`/health` = 200（链路闭合）

### 端口占用诊断与「结束占用进程」按钮（同批）- [x] 只读诊断：`netstat -ano` 解析出占用端口的 PID（纯函数 `parse_netstat_listening_port`，只认 LISTENING 行、不误匹配前缀/UDP/他人外部地址列 + IPv6 行），再按 PID 查进程名（Windows `wmic`、Unix `ps`）
- [x] 区分三类占用者：**本程序自己**（`is_self`，上一轮监听没放开）、本应用另一个实例（按进程名匹配，升级后旧版驻留的典型场景）、其他程序；文案与建议各不相同
- [x] 界面：`portOccupant` 进快照 → 主窗口显示「端口被占用」块（谁占的 / 等价命令 / 按钮上写明 PID）
- [x] 「结束占用进程」按钮：**只有用户点它才会结束进程**（`kill_port_occupant` 命令）；当前进程自己占用时直接拒绝，且前端把这个按钮藏掉；杀完自动重试监听
- [x] 「重试监听」按钮（`retry_http`）
- [x] 取消勾选「HTTP 服务」立刻停服务并释放端口（`apply_http_config` 的 stop 路径，已有 `stop_releases_the_port` 单测）
- [x] **收服务必须等端口真的放开才返回**：`RunningServer::stop()` 里补一次可达的自连接唤醒 + 轮询试绑（`wait_until_released`），修掉「上轮监听自己占着端口 → 改 token 重绑必报 10048」；三条单测：`stop_releases_the_port`、`wait_until_released_reclaims_a_wildcard_port`、`connecting_to_the_wildcard_address_fails_on_windows`
- [x] README 补「端口被占用 / 想换端口」一节的完整说明

---

## 弹窗/托盘不再压任务栏 + 若干界面细节（第二批）

用户实测反馈（截图见 `system-switch.png` 同目录的记录）：

- [x] **倒计时弹窗高度按内容自适应**：窗口建出来是隐藏的，前端在 `render()` 里量出「刚好兜住最后一行」的高度，调 `present_notify_window(height)` 定尺寸 → 摆位 → 显示；量不出来（返回 null/0）就退回标称高度 240，宁可不紧凑也不裁文字。后端 `notify_height()` 把值夹在 150–460。
- [x] **弹窗不压任务栏**：定位改用 `Monitor::work_area()`（已排除任务栏）而不是 `size()`；尺寸用「正要设成的逻辑高度」换算，不去 `outer_size()` 读回（`set_size` 是异步投递的，读回会拿到旧值 → 位置偏几十像素）。
- [x] **托盘右键菜单不压任务栏（Windows）**：原生实现把菜单锚在**光标**上，而托盘图标的光标在任务栏里，Windows 只保证菜单不越出屏幕、不越出工作区 —— 本机实测（2194×1234、任务栏 40px）菜单底部 = 1214，压住任务栏 20px。改为：Windows 上托盘**不挂菜单**，右键时按当前快照现建现弹，锚点落在工作区下沿（实测 0px 越界）。
  - 锚点窗口 = 一个 8×8、无边框、不可见、创建时就停在 (0,0) 的 webview 窗口（`menu-anchor.html`）；`Menu::popup_at` 会把坐标当它的**客户区坐标**再 `ClientToScreen`，停在原点才好算。
  - 之所以用带 webview 的窗口：`AppHandle::get_window` / `windows()` 被 tauri 的 `unstable` feature 挡着，公开的只有 `get_webview_window`（`Window` 本身也不是 Send/Sync，进不了 app state）。
- [x] **「远程怎么调」与「最近请求」两个框统一质感**：同样的描边、圆角、底色（`--input-bg`）、字色（`--text-dim`）与字号，只有内边距不同（列表要给序号留位置）。
- [x] **不透明度支持 0–100**（原来下限卡在 50）：`MIN_OPACITY` 改 0、滑块 `min="0"`、前端夹取同步；0% = 全透明但窗口还在，托盘照常能操作。
- [x] **「打开变量目录」挪到「使用此目录」左边**（`ui/index.html` 按钮顺序）。

---

## 实施顺序

1. **阶段 A**：后端添加设置项 + 命令
   - `Settings` 新增 `playSound` / `soundFilePath`
   - `open_config_dir` / `open_vars_dir` 命令
   - 调整 `requests` 排序（最新的在后）

2. **阶段 B**：前端 UI 改造
   - `notify.js` 加 `setInterval` 动态倒计时
   - 提示音播放逻辑（HTML5 Audio）
   - 两个「打开目录」按钮

3. **阶段 C**：验证 + 提交
