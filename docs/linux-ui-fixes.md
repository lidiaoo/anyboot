# Linux 真机 8 项界面与体验修复（进行中）

在 Linux 真机（Arch + Cinnamon/Muffin，X11，有合成器）上提出的 8 个问题。
逐项记录**现象 → 根因 → 方案 → 验证**，便于后续复查。

环境事实（实测）：

- `DISPLAY=:0`、`XDG_SESSION_TYPE=x11`、WM = **Muffin**（Cinnamon，带合成器）
- 系统中文字体齐全（Noto Sans CJK / Source Han Sans），所以「不显示字」**不是缺字体**
- 音频后端：`paplay` / `aplay` / `ffplay` / `mpg123` / `cvlc` 都在
- `ffmpeg` 在（用于把内置提示音转成 WAV）
- 构建：`cargo 1.99.0`、`webkit2gtk 2.54.1`、`npm 10.5.0`

---

## 问题 1：主界面「远程怎么调」区块默认展开 ✅

**用户澄清**：「问题1区域是指的主界面里面远程怎么调这个区块」。

**现象**：那个区块原来只显示一行占位文字（「点「复制命令」拿含 token 的可直接执行示例」），
必须点一次「复制命令」才看得见内容 —— 对刚上手的人等于「默认折叠着」。

**方案（已实现）**：`app.js` 初始化时直接调 `http_access_hint` 把含 token 的完整示例
填进 `#http-hint`，进界面就是展开的；「复制命令」按钮保留（点了复制并刷新文本）。
示例块上方加了一行小字提醒「下面这段默认就展开、含明文 token（注意旁人视线）」。

---

## 问题 2：支持自定义 token

**现象**：token 只能自动生成（32 位 hex）或点「重新生成」，用户无法自己指定。

**根因**：`Settings.http_token` 已有字段、`ensure_settings_complete` 会自动补一个随机值，
但**没有写入入口**（只有 `regenerate_http_token` / `reveal_http_token`）。

**方案**：

- 新增命令 `set_http_token(token: String)`：trim → 校验 → 保存 → `apply_http_config`
  （token 变了要重启监听，让新 token 立即生效）。
- 校验规则（纯函数，可单测）：非空时长度 4–128；只允许可打印 ASCII，禁止空白字符
  （避免复制粘贴带换行/空格导致 401 难排查）；空 = 等于关闭服务（现有语义：空 token 不启服），
  界面必须直接说清这一点。
- UI：token 行加输入框 + 「保存」，与「显示 / 复制 / 重新生成」并排。

---

## 问题 3：倒计时弹窗在 Linux 下不显示字 + 渲染异常

**现象**：Linux 下 notify 窗口（`notify.html`）文字不显示、渲染坏。

**根因分析**（待真机截图确认）：候选原因

1. `notify.css` 里 `:root { font-family: "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif }`
   —— 这三种字体在 Linux 都不存在，回落 `sans-serif` 由 fontconfig 决定；系统字体齐全，
   所以**大概率不是它**，但补上 Linux 字体名（Noto Sans CJK SC / Source Han Sans SC）无害且更稳。
2. `body` 没有背景色，只有 `.panel` 有 → 窗口尺寸 420x260 大于内容高度时，
   下半部分露出 webview 默认底色（白/透明），观感像「渲染坏了」。
3. 窗口用 `.decorations(false)` + `.always_on_top(true)` + `.focused(false)` 创建，
   X11 下无 WM 协作时可能拿到异常尺寸/位置。
4. `notify.js` 若在 `window.__TAURI__` 未就绪时抛错，动态文字全空
   （静态文字仍应在，所以不是「完全白屏」的唯一解释）。

**方案**：字体链补 Linux 字体 + `body` 加背景与 `min-height: 100vh` + `.panel` 铺满 +
窗口高度按内容自适应（`inner_size` 加大到 300 或运行时 `set_size`）；
保留 `decorations(false)`，但去掉 `focused(false)`（Linux 下可能导致窗口不映射）。

**验证**：真机跑沙箱实例 → curl 触发远程请求 → `import -window` 截图 → 目视检查。

---

## 问题 4：主窗口呼出应置顶

**现象**：主窗口（托盘「显示主窗口」/ 左键点击）呼出后会被其他窗口盖住。

**根因**：`show_main_window()`（lib.rs:3672）只做 `unminimize + show + set_focus`；
Linux 下 WM 的 focus-stealing-prevention 会让 `set_focus` 无效。

**方案**：呼出时 `set_always_on_top(true)`，并在窗口失焦时恢复 `false`
（既能压在别人上面，又不会永久霸屏）。

---

## 问题 5：各平台都没有弹窗提示音（需内置 KakaoTalk 提示音）

**现象**：远程请求弹窗时三平台都没有声音。

**根因**（已定位）：

1. `notify.js::playSoundNotification` 在前端用 `new Audio(path)` 播放：
   - 自定义路径是**本地文件系统路径**，webview 不能直接播（需要 asset 协议转换），永远失败；
   - 默认分支的 `data:audio/wav;base64,...` 是一段**几乎没有采样数据**的伪 WAV
     （`UklGRigAAABXQVZFZm10IBAAAAABAAEARKEAACAhIQMgFAABOUZpbZM=` 解码后仅 ~40 字节），
     即使能解析也是无声；
   - webkit2gtk 播放还需要 gstreamer 插件，缺一个就静音。
2. 后端 `play_sound` / `sound_file_path` **只存设置，从不播放**（grep 证实无任何播放调用）。

**方案**：

- 内置一份 **KakaoTalk 风格提示音**（WAV，单声道，~0.5s），用 `include_bytes!` 编进二进制，
  运行时写到配置目录（`sounds/notify.wav`）再播放 —— 不依赖打包资源路径。
- 播放走**后端系统命令**（比前端 Audio 可靠）：
  - Linux：`paplay` → `aplay` → `ffplay -nodisp -autoexit`
  - macOS：`afplay`
  - Windows：`powershell -c (New-Object Media.SoundPlayer '<wav>').PlaySync()`
- 自定义路径优先；自定义不存在或播放器都失败 → 回落内置音，并把原因写进 trace。
- 前端不再播声音（去掉那段伪 WAV）。

**验证**：真机 `paplay` 播一次（能听到），并在 trace 里看到「已播放提示音」记录。

---

## 问题 6：「打开变量目录」按钮移到变量目录那一栏下方

**现象**：现在两个「打开目录」按钮在「远程控制」区底部（`#open-dir-actions`），位置离谱；
「打开变量目录」应该紧挨着变量目录输入框。

**方案**：

- `ui/index.html`：把「打开变量目录」按钮从 `#open-dir-actions` 移到 `.directory` 区的
  `.actions`（与「使用此目录」「扫描」并排）；「打开配置目录」留在原地（它是设置/日志相关）。
- `app.js` 里 `#open-vars-dir` 的事件绑定不变（只挪位置）。

---

## 问题 7：美化系统列表

**需求**：

1. 选中的系统打**绿色勾**；
2. 顶部用**醒目大字体 + 颜色**说明「下次启动的系统是 X」；
3. 系统列表**左对齐**；
4. 勾在**额外的一列**，不影响其他列的对齐。

**方案**：

- `renderSystems`：每张卡改为 `grid` 三列：`[勾 26px] [名称/来源 1fr] [按钮 auto]`，
  勾列固定宽度 → 所有行的名称与按钮天然对齐。
- 选中项：勾列显示绿色 ✓（`✓` + `color: #46c46e`），卡片边框/背景加绿调。
- 列表上方新增一行「下次启动：**Windows**」（`#next-boot`），字号 ~20px、鲜艳色，
  数据取 `status.variables` 里 `active` 的那一项；没有选中项时显示「下次启动：未知」。

---

## 问题 8：主题选择（暗色 / 亮色 / 跟随系统）+ 透明度

**需求**：界面支持主题切换 + 可调不透明度。

**方案**：

- `Settings` 加 `theme`（`dark` / `light` / `system`）与 `opacity_percent`（50–100，默认 100）。
- CSS 改用变量：`:root` 定义暗色；`[data-theme="light"]` 覆盖为亮色；
  `theme=system` 时用 `prefers-color-scheme` 媒体查询决定（`app.js` 里监听变化）。
- 不透明度：主窗口 `transparent: true` + `body` 背景用 `rgba(..., var(--ui-alpha))`，
  默认 100% 时与现在观感一致（纯不透明）。**若某些环境透明导致异常，把滑块拉回 100% 即恢复。**
- UI：新增「外观」区：主题下拉 + 不透明度滑块（拖动即生效，松手保存）。
- 命令：`set_theme(theme)`、`set_opacity(percent)`。

---

## 落地顺序（全部完成）

1. ✅ 基线实测：真机跑沙箱实例 + xwd 截图，确认主窗口渲染正常、问题 3 是弹窗自身
2. ✅ 后端：`set_http_token` / `set_theme` / `set_opacity` + `play_notify_sound` +
   `show_main_window` 置顶 + 失焦取消置顶 + 3 条单测
3. ✅ 前端：弹窗字体与铺满修复、按钮归位、系统列表三列 + 绿勾 + 大字标题、主题与
   不透明度、示例默认展开
4. ✅ 问题 1 由用户澄清后实现
5. ✅ 验证见下
6. ✅ 提交（英文 + 中文）

---

## 实机验证记录（Arch + Cinnamon/Muffin，X11，DISPLAY=:0）

跑法：`ANYBOOT_CONFIG_DIR`/`ANYBOOT_VARS_DIR`/`ANYBOOT_HTTP_PORT=12399` 沙箱，
**不碰真实 ESP 与配置**；截图用 `xwd -id <win>` + ImageMagick。

| 项 | 证据 |
| --- | --- |
| 问题 1 | 截图里「远程怎么调」进界面就填好了含 token 的 curl 示例（含新增的提醒小字） |
| 问题 2 | `cargo test` 的 `custom_token_validation_*` 覆盖长度/空白/中文/换行；命令已注册 |
| 问题 3 | 弹窗截图：中文全部正常（无方块/乱码）、进度条正常、双按钮正常、背景整块铺满无未渲染空白 |
| 问题 4 | `xprop _NET_WM_STATE` → `_NET_WM_STATE_ABOVE`（呼出即置顶）；对照用户侧旧实例无该状态 |
| 问题 5 | trace `[sound] 已播放内置提示音（paplay）：<配置目录>/sounds/notify.wav` |
| 问题 6 | 截图：变量目录那栏按钮为「使用此目录 / 打开变量目录 / 扫描」 |
| 问题 7 | 截图：顶部绿色左边框 + 大号绿字「Windows」；Windows 卡片左侧绿勾 ✓，另两张无勾但三行文字仍对齐 |
| 问题 8 | 把 `theme=light, opacityPercent=70` 写进沙箱设置后重启：界面变浅色（白卡片、选中卡淡绿、深色文字），背景呈中灰 —— 正是 alpha 0.7 叠加桌面后的结果 |
| 回归 | `cargo test` 42 passed；`node --check ui/app.js ui/notify.js` 通过 |

### 顺带修掉的两个既有问题

1. **`.hidden` 失效**：`styles.css` 里 `.hidden{display:none}` 写在 `.pending{display:grid}`
   与 `.port-occupant{display:grid}` **之前**且优先级相同 → 被后者覆盖，
   于是「远程请求待执行」卡片与端口占用提示**一直显示**（基线截图里能看到那个橙色卡片）。
   现在 `.hidden` 用 `display: none !important`。
2. **提示音输入框太窄**：placeholder 被 116px 宽度裁掉 —— 输入框加宽到 200px、文案改短。

### 已知限制

- 不透明度调低时的「透出桌面」效果依赖桌面合成器；无合成器的裸 X11 可能显示黑底，
  拉回 100% 即恢复正常（README 已写明）。
- Windows 的 `Media.SoundPlayer` 只认 WAV PCM：自定义提示音请用 WAV，其他格式会
  回落内置音（trace 里有记录）。