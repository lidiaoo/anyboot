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
- [x] `cargo test` 37 passed、`node --check ui/app.js ui/notify.js` 通过

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
