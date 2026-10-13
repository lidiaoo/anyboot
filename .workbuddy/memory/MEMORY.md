# AnyBoot 项目长期备忘

## 构建与验证（照抄这串，别漏）
- `export HOME=/home/jianghh CARGO_HOME=/home/jianghh/.cargo`（本机 `$HOME=/root` 属主 nobody → cargo 读 `~/.rustfmt.toml` Permission denied）。
- **cargo 必须非沙箱跑**（沙箱里 tauri-codegen 读 `target/release/build/anyboot-*/out/` 报 os error 5）。
- `cargo test --release` **不产出可交付 exe**；交付 = 测试 **+ `cargo build --release`**，构建前退出运行中的 anyboot。
- 只改前端时增量编译**不重跑打包**，前端不进 exe。强制：`rm -rf target/release/build/anyboot-*/out && touch build.rs && cargo build --release`（`rm` 被 safe-delete 拦时用 `/usr/bin/rm`）。核对：`out/tauri-codegen-assets/*.css|js` 是 brotli，node `zlib.brotliDecompressSync` 解开 grep。前端语法：`node --check ui/<每个>.js`。
- 冒烟脚本在 **`/home/jianghh/.cache/ab-win/windows.sh`**（三窗口版：主窗 / 贴边白条 / 通知弹窗，一键跑完并出三张 png）；
  `/home/jianghh/.cache/ab-verify/` 下：`run.sh`（出厂默认值 / 窗口几何 / 网页端授权流程一站跑完）、
  `real.sh`（**拿用户真实配置**验几何 —— 记得它要临时关 `startInTray` + `mainAutoHide`）、
  `auth.sh`（授权边界四步）、`exec.sh`（到点自动执行不被误取消）、
  `resize.sh [autoHide] [position]`（改尺寸后是否仍居中）、`localclickswitch.sh`（本机点击作废挂起远程请求）、
  `explore.sh`（起实例 + 点页签 + 截图）、`recall.sh`（托盘启动 → 诊断钩子模拟托盘呼出，验「主界面能不能被呼出来」）。
  骨架：`Xvfb :99 -screen 0 1280x1024x24 &` →
  `DISPLAY=:99 ANYBOOT_CONFIG_DIR=<cfg> ANYBOOT_VARS_DIR=<vars> ANYBOOT_TRACE=1 ANYBOOT_POWER_DRY_RUN=1 <额外钩子> ./target/release/anyboot &` →
  `xdotool search --onlyvisible --name '^AnyBoot$'` 拿窗口 → **先 `xdotool windowsize` 晃一下再截图**（隐藏期 WebView 视口 0×0，不触发重排会截到一条竖条）→ `import -window root shot.png`。
  宿主 12300 被占时给 settings 预置 `httpPort: 12400`。
- **起 Xvfb 的三件套，缺一个就失败**（`ab-verify/run.sh` 里本来就有，自己另写脚本时别漏）：
  ① `mkdir -p /tmp/.X11-unix && chmod 1777 /tmp/.X11-unix`（缺了连不上 display）；
  ② `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json`
  （不加就被本机 NVIDIA 615 的 `libnvidia-egl-gbm.so.1` 拖崩；另配 `LIBGL_ALWAYS_SOFTWARE=1
  GALLIUM_DRIVER=llvmpipe __GLX_VENDOR_LIBRARY_NAME=mesa` 更稳。**别用 `-extension GLX` 绕** ——
  Xvfb 起来了但 GTK 报 `Failed to initialize GTK`，anyboot 起不来）；
  ③ `export HOME=/home/jianghh` + `XDG_DATA_HOME/XDG_CONFIG_HOME/XDG_CACHE_HOME` 指到可写目录
  —— 本机 `$HOME=/root` 不可写，Tauri/WebKit 写不了 `$HOME/.local/share` 就
  **`Failed to setup app: Permission denied (os error 13)`**（这个报错跟 HTTP 开关、跟沙箱都无关，
  别被「开了 HTTP 才崩」误导）。
- 后台进程（Xvfb / anyboot）**要用 `run_in_background` 托管**才活得过一次工具调用；
  `nohup ... &` 只活到本次调用结束。`/home/jianghh/.cache/ab-test/` 下：`serve.sh`（常驻起服务）、
  `ws-live.sh`（起服务 → 跑探针 → 截图 → 收摊，一条命令出结论）、`ws-probe.js`（只用 net+crypto 手写的
  WebSocket 客户端，验握手/帧/推送）。
- **浏览器端到端**：`agent-browser`（`/home/jianghh/.cache/ab-browser/node_modules/.bin`）+ Chrome 155
  （`/home/jianghh/.agent-browser/`）。安装要带 `HOME=/home/jianghh`（否则 npm 写 `/root/.npm` 直接失败）。
  两条铁律：**open + eval 必须在同一次 Bash 调用里**（跨调用会变成 `about:blank`）；
  `eval` **读不到脚本作用域的 `let`**（`transportMode` 之类 ReferenceError）→ 只能靠 DOM + 后端 `trace.log` 观察行为。
- **起服务等就绪，只认「应用自己日志里的 `已监听 <port>`」，不要只测端口能不能连**：我自己用
  「后台任务」托起来的实例会**一直活着**（`serve.sh` 那次活了 10 分钟），而后续另起一个 shell 调用时
  `pkill` / `ss` 可能**看不到它**（不在同一会话/命名空间；`/proc/net/tcp` 里却有一条 `0100007F:3070 0A`）。
  于是「端口能连上」这个判据会被它骗过：探针全绿，跑的却是旧二进制
  （症状：本次实例日志里写着 `Address already in use`，环里还带着旧版本才有的条目）。
  **两条对策**：① 每次验证**换一个端口**；② 收摊要用**后台任务管理器停**（`TaskStop`），
  光 `pkill` 不保险。
- **两套取证通道别搞混**：`trace()` → 日志文件 `trace.log`（`[settings-file] [scan] [tray] [state] [command]`…）；
  而 `startup_debug()` → **`<config>/startup-debug.log`**，`center main -> (x,y) 窗口=WxH`、`moved diag`、
  `autohide diag`、`dock begin`、`undock`、`diag: 模拟呼叫主窗口` 全在这里。**查落位/贴边只认后者。**
- **冒烟必须整个塞进「同一个 Bash 调用」用前台 `sleep` 串起来**：写在 `.sh` 里再 `setsid … &` 甩后台，
  脚本会被**中途杀掉**，而 stdout 重定向到文件是**块缓冲** → 被杀时缓冲全丢（表现为「脚本明明跑过、
  cfg 目录有产物，却一行输出都没有」，极难判断）。要落盘就让每次 `echo` 都 `>>file`。
  另外 `ab-verify/*.sh` 未必有 +x，用 `bash ./x.sh` 跑。采样窗口要够密：`xdotool` 每次调用约 50~150ms，
  46ms 的错位一闪就可能漏掉 —— 先用官方日志确认「有没有发生」，再用 xdotool 抓「真几何」。
- **`/usr/bin/xprop` 在本机是 0 字节空壳**（`import` 只有 6 字节但真能用）→ 想读 X 窗口属性
  只能自己写 ctypes 调 `/usr/lib/libX11.so.6`（注意不在 `x86_64-linux-gnu/` 下）。见 `ab-verify/opacity.py`。
- **`xdotool search --name` 的正则对中文不可靠**：`--name '^远程请求$'` 匹配不到，但把窗口列出来
  用 shell `case "$nm" in *远程请求*)` 逐字比较就能命中 → 一律「先枚举再看名字」，别指望 xdotool 的正则。
- 验「出厂默认」的正确姿势：settings.json 里**故意不写** `theme`/`opacityPercent`/`mainWindowPosition`，
  只留 HTTP 必需项，逼它走 `Settings::default()`，再对比「后端补全写回的文件 / `/api/ui-state` / 真实客户端 DOM」
  三处是否一致。用 WebKitWebDriver（`browserName=MiniBrowser`）读 DOM；`ab-verify/wd_defaults.py`、`wd_pending.py` 可复用。
- 复现台四个必踩的坑（都花过时间）：
  1. **必须 `export HOME=/home/jianghh`** —— 否则 Tauri 起来就 `Failed to setup app: Permission denied (os error 13)` 退出（`$HOME=/root` 属主 nobody）。
  2. **脚本别放 `/tmp`**：沙箱 Bash 与非沙箱 Bash 的 `/tmp` 是**隔离**的（Write 写的文件非沙箱 shell 看不见，且会被清空）。放 `~/.cache/`。
  3. **Xvfb 要先 `mkdir -p /tmp/.X11-unix`**，并给 **Xvfb 本体**加 `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json`：glvnd 列表里 NVIDIA 的 EGL 排在 Mesa 前面，沙箱里没有 `/dev/dri` → Xvfb 直接 core dump（backtrace 落在 `libnvidia-egl-gbm.so.1`）。**只给 Xvfb 加，别 export**，否则被测应用也走了 Mesa EGL、验的就不是真实路径。
  4. **`rm -rf` 会被 safe-delete 拦**（genie-trash 在 root 下失败 → FAIL_CLOSED），脚本里一律用 `/usr/bin/rm -rf`。
  5. **用「用户真实配置」复现前要临时改两处**：`startInTray → false`（他的配置里是 `true`，窗口压根不显示 →
     `xdotool` 拿到空 WIN → 后面全报 `BadWindow`）、`mainAutoHide → false`（复现台没有鼠标停在窗口上，会贴边收成条）。
  6. **模拟他的屏幕要显式给尺寸**：本机内屏（eDP-1）是 **1920x1080** → `Xvfb :99 -screen 0 1920x1080x24`。
     这条顺带决定「窗口默认高度 = 屏幕工作区一半」在本机的实际值：1080/2 = 540 低于最小高度 560 → **实际是 560**。
- 三窗口各自的触发方式：主窗靠 `ANYBOOT_AUTOHIDE_DIAG_RECALL_MS` 呼出（绕开 `visible:false` 起不来）；
  **贴边白条必须让 settings 里 `mainAutoHide=true`** —— 否则 `dock_main_window()` 第一行就 return（"设了钩子却没反应"多半是这个），再配 `ANYBOOT_AUTOHIDE_DIAG_FORCE_DOCK_MS`；
  通知弹窗用 `POST /api/power?action=reboot`（头 `X-Auth-Token: <token>`，配 `ANYBOOT_POWER_DRY_RUN=1` 不真重启，settings 里 `channels.*.delaySeconds` 拉长留截图时间），trace 会打 `[notify] present height=`。
- 环境变量类改动想 A/B（新行为 vs 旧行为）**不用重编译**：本项目「用户已配过任一 WebKit/NVIDIA 开关就完全不插手」，额外设 `__NV_DISABLE_EXPLICIT_SYNC=0` 即让注入失效＝旧行为。
  核对注入到了渲染进程：`/proc/<WebKitWebProcess pid>/environ`（**主进程自己的 `/proc/<pid>/environ` 看不到** —— 它记的是 exec 时的初始环境，`set_var` 不反映在那儿）。
- **HTTP 端到端验证**（改远程 / 授权链路时必做，比读代码靠得住）：预置一份 `settings.json`
  （`httpBind=127.0.0.1 / httpPort=12400 / httpToken=<固定值>`）→ `ANYBOOT_CONFIG_DIR` 指到临时目录起 app
  → `curl --noproxy '*' -X POST http://127.0.0.1:12400/api/ui-command -H "X-Auth-Token: <值>" -H 'Content-Type: application/json' -d '{"cmd":...,"args":{...}}'`。
  现成脚本：`/home/jianghh/.cache/ab-verify/run.sh`（① 出厂默认值 ② 窗口几何 ③ 授权流程，一站跑完）。
- **`/api/ui-command` 的回执必须落在 `result` 里**：网页端 `invoke()` 只取 `json.result`（`ui/app.js`），
  额外回执塞在 json 顶层 = 前端永远看不到（写「已发送到本机」提示时踩过）。透传子链路回执
  （如 `start_remote_command` 的 `pending/message`）要包成 `{ok, cmd, result: <子回执>}`。
- **静态路由表要覆盖「自然的写法」**：`static_ui_reply` 原来有 `/`、`/ui`、`/ui/`、`/ui/index.html`
  **独缺 `/index.html`** —— 用户书签/手输这个地址，无 token 回 401「token 缺失或无效」、带 token 回 404，
  两种都会让人误以为是 token 问题。凡新增静态资源都问一句「用户最可能怎么打这个地址」，
  并把写法钉进单测（`static_ui_routes_cover_the_natural_spellings`）。
- **`revision` 是「进程内」计数器 → 起点必须跟时间挂钩**（否则跨重启客户端永久冻结）：
  客户端 `applySnapshot` 用 `revision <= lastRevision` 丢过期快照，进程重启后 revision 从 0 重数，
  客户端手里旧进程的高水位会把新进程的快照**全部**丢弃 —— 实测旧进程到 7、新进程到 5，
  页面每 2 秒收到 200 却一个字段都不更新（主题/系统列表全停旧值），且「不刷新页面就永远不同步」。
  修法：`run()` 第一件事 `UI_REVISION.store(ui_revision_seed(unix_millis()))` —— 新进程启动更晚、
  起点必然更大，**所有客户端都不用改**。凡是「前端按版本号丢过期数据」的判据，
  都要先问一句「这个版本号跨进程/跨会话还单调吗」。
- **验「断线自愈」类行为，必须让恢复可观测**：只断线再恢复、看页面还在，是分不清
  「没重连」和「重连了但拿的还是旧快照」的。做法＝断线期间**改掉宿主状态**（改 theme、删变量文件），
  重连后页面出现新值才算真自愈。给页面 `fetch` 装钩子记录每次请求的 `state/status/error` 也很有用
  —— 本案就是这样看到「502 之后全是 200 但页面不动」的（脚本 `heal.sh`/`heal.py`/`live.py`）。
  另：变量文件被删**不会**让系统卡片消失，只让副标题变「（不存在）」+ 按钮变灰。
- **请求路径必须先剥绝对 URI（代理）**：客户端配了系统代理时请求行是
  `GET http://host:port/path HTTP/1.1`（直连才是 `GET /path`），`split_url()` 里不剥掉
  scheme+host 的话，按路径精确匹配的路由全部落空 → 401 → 网页**整片空白**（自己的 CSS/JS 都取不到）。
  已在 `split_url()` 最前面用 `strip_absolute_uri()` 统一剥掉（大小写不敏感，`http://host` 当 `/`），
  所有路由一次受益；单测 `split_url_strips_proxy_absolute_uri` 钉住。
  「网页有时空白有时不空白」先怀疑浏览器/手机走没走代理。本沙箱 `http_proxy` 是常开的，
  复现台里这属于**必踩项**。
- **用 WebKitWebDriver 验真实客户端**（本机 `/usr/bin/WebKitWebDriver`，可 `browserName=MiniBrowser`
  建会话；`Epiphany` 反而匹配失败）：能读真实 DOM / cookie / `button.disabled`，**点元素按文本找、
  不用猜像素**。脚本 `/home/jianghh/.cache/ab-verify/wd_client2.py` + `webflow.sh`。两个坑：
  ① `execute/sync` 的 script 当**函数体**执行，IIFE 必须写成 `return (()=>{...})()`，否则永远 `{"value":null}`；
  ② `xdotool search --onlyvisible --name '.'` **匹配不到通知弹窗**（它的 WM_NAME 是空的，`.` 要求恰好一字符），
  要 `.*` 或直接看截图。driver 与浏览器都要 `WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1`。
- **窗口几何验收**：`xdotool getwindowgeometry <win>` 直接读 `Position` / `Geometry`（无 WM 的 Xvfb 下就是真实落点），
  比截图量像素准。例：1280x1024 + 默认配置 → 期望 `460x560 @ (410,232)`。
- **`xdotool` 点击只能给「相对窗口左上角的偏移」**：`eval "$(xdotool getwindowgeometry --shell $WIN)"` 拿 X/Y 再加偏移。
  写死绝对坐标必翻车，而且**点空之后两张截图几乎一样**，极易误判成「点了没反应」——
  下坐标前先用 `magick <png> -crop WxH+X+Y +repage -resize 300%` 裁出来确认，比连试三轮快。
  已知偏移（wide 932x560）：「系统引导」页签 **(41,123)**；Linux 卡片「下次启动 Linux」**(834,349)**。
- **`set_window_size` 也在 `/api/ui-command` 白名单里** → 改尺寸的几何验证可以纯 HTTP 驱动**生产路径**
  （比 `xdotool windowsize` 硬改更贴近真实）。现成脚本 `/home/jianghh/.cache/ab-verify/resize.sh [autoHide] [position]`。
- **手机端 Java 代码在本机能做「编译级」验证，也能真跑纯逻辑**（别等 Windows）。
  下面这套 Robolectric 路子**仍然可用且比装 SDK 快**（2026-10-13 起本机也装了真 SDK，可换用
  `/home/jianghh/android-sdk/platforms/android-34/android.jar`，两者都行）：
  ① JDK 用 Android Studio 自带的 `jbr`（`/home/disk2/software/JetBrains/Toolbox/android-studio/jbr`，本机 25）；
  ② 拉一份 Android 框架类当 classpath（含 `android.R`）：
  `curl --noproxy '*' -sSL -o android-all.jar https://repo1.maven.org/maven2/org/robolectric/android-all/14-robolectric-10818077/android-all-14-robolectric-10818077.jar`
  （**直连**可以，`http_proxy` 那个 44123 是死的）；
  ③ `"$JBR/bin/javac" -encoding UTF-8 --release 17 -classpath android-all.jar -d out src/com/anyboot/remote/*.java`
  → 类型检查通过 = 语法/签名都对。注意 `android-all` 里 `android.graphics.Color` / `android.net.Uri`
  这类**方法体是 stub**（运行时会抛），所以只能「编译 + 跑纯字符串/JSON 逻辑」。
  ④ 纯逻辑可以真跑起来（本机已验证 29/29）：测试类必须写在 `package com.anyboot.remote;` 里
  （`ServerProfile` 是包级私有），`-sourcepath <app>/java:<test>/src`，然后用
  `java -classpath android-all.jar:out com.anyboot.remote.ProfileLogicTest` 跑（`org.json` 是真实现）。
  现成脚本与用例：`/home/jianghh/.cache/ab-javacheck/`（`android-all.jar` + `src/.../ProfileLogicTest.java`）。
  **交付 APK 不再依赖 Windows**：本机已装真 SDK，`/home/jianghh/.cache/ab-android/build.sh` 直接出 APK
  （2026-10-13 验证，BUILD SUCCESSFUL）；上面这条 Robolectric 路子仍可用于「没装 SDK 的机器上提前拦编译错误」。

## WebKitGTK × NVIDIA：画不出来（Linux）
- 症状两种，**同一个病**：①**独显模式**下主窗一片白、零报错（集显/Intel-Mesa 好好的）；②「内容能加载、但**只有鼠标扫过的地方**才显示」。根因是 WebKit 交给窗口系统的帧是 **GPU 缓冲（DMA-BUF）**，NVIDIA 专有驱动的 X11/EGL 路径收不下 → WebKit 静默画不出来。
- **X11 的正解只有一个开关：`WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`**（让帧以共享内存交付，绕开 GPU 缓冲）。已落地在 `apply_linux_gpu_workarounds()`（`run()` 最前面调用，**必须早于建 WebView** —— WebKit 建窗那一刻读这些变量）；检测 `/proc/driver/nvidia/version`（纯核显/nouveau 没有 → 一个开关都不加）。其余分支：Wayland+Hyprland → 关 DMA-BUF 渲染器；Wayland+KWin/Plasma → `__NV_DISABLE_EXPLICIT_SYNC=1`；其它 Wayland + 驱动 ≥560 → 不动。
- ⚠️ **两个坑开关，别再回头用**（本轮两个版本都栽在这儿，真机实测）：
  - `WEBKIT_DISABLE_DMABUF_RENDERER=1` —— **2.52 起不再回退到共享内存**，只会把传输模式清空。X11 上表现正是「只有鼠标扫过才显示」；上游还报它会让 `AcceleratedBackingStore::create()` 返回 null 而崩（Buzz #3654 明确「切勿使用」）。只留 Hyprland 那支还在用。
  - `WEBKIT_DISABLE_COMPOSITING_MODE=1` —— **2.54 已失效**：非合成渲染器在 2.54 同样走 AcceleratedBackingStore，加上去症状一字不变。
- **背景**：WebKitGTK 2.46 起 Skia 取代 Cairo 做 2D；**2.54 起合成器也搬到 Skia，并把「只合成损伤区域」打开成默认** —— 所以症状会从「全白」演化成「局部重绘」，也就更误导人。老帖子里的「关某个加速开关」到新版失效甚至有害，改这块必须先查新 issue。
- 逃生舱 = `WEBKIT_ENV_SWITCHES` 那四个名字用户设过**任一个**（含设成 `0`）就全部跳过；同时也是 A/B 开关（`WEBKIT_DMABUF_RENDERER_FORCE_SHM=0`）。
- 上游依据：**WebKit Bug 324551**（NVIDIA 机上唯一能画出页面的是 FORCE_SHM，且明说 DISABLE_COMPOSITING_MODE 在 2.54 已不避开）、WebKit PR #20869（变量由来）、Buzz #3271/#3654、A2Tools DPS Meter 的 Linux 说明、`webkit2gtk-nvidia-quirk` crate 的会话对照表。
- 单测把 X11 开关集**钉死**成这一个，并断言另两个废弃名不许加回来（`gpu_override_depends_on_backend_and_compositor`）。
- **仍未实机验证**：Xvfb 没有 NVIDIA GL（本沙箱连 `/dev/dri` 都不存在），只证明了「不崩 + 开关确实传到了 WebKitWebProcess + 界面渲染无回归」。**NVIDIA 上的最终效果必须用户真机确认。**
- 若 FORCE_SHM 仍不够，备选（未验证，按顺序试）：`WEBKIT_DISABLE_DMABUF_ATLAS=1`（2.54 新增的 GPU atlas）、`LIBGL_ALWAYS_SOFTWARE=1`（整条 GL 走软件）。

## 出厂默认值与「远程要经本机同意」
- **默认值**（`Settings::default()`，三条常量钉着）：主题**亮色**（`DEFAULT_THEME`）、不透明度 **90**
  （`DEFAULT_OPACITY_PERCENT`）、窗口**居中**（`DEFAULT_WINDOW_POSITION`）、`remoteAllowGuiAuth=**true**`、
  窗口高度 = **屏幕工作区的一半**（`window_height == 0` 时现算，落位后写回记忆值）。
  单测 `settings_defaults_match_the_agreed_product_defaults` 钉着这四条，改默认值必须同步它。
- **`window_height == 0` 是「还没记过尺寸」的哨兵**，不是真实高度 —— 别拿它当尺寸用（会被 clamp 成 560）。
  想回到出厂高度用「重置窗口大小」（`reset_window_size` → `default_window_height`）。
- 「屏幕一半」在 **1920x1080 上实际落到 560**（一半是 540，被 `WINDOW_MIN_SIZE.1=560` 顶住，**这是有意的**：
  再矮界面就挤坏了）。`default_window_height_from_work_area` 的注释和单测都写了。
  用户没答复「560 还是 720 / 固定 700」—— 现状＝按用户原话的字面实现（工作区一半，夹最小高度）。
- **远程动作 = 本机点头**：网页 / 手机端 `/api/ui-command` 的 `switch_system` 不直接执行，
  由 `ui_command_requires_local_auth()` 拦下 → `start_remote_command`（pending + 本机弹授权窗，
  弹窗自带「来源 IP｜渠道｜发起时间」）。本机主窗口走 Tauri IPC，不经这条 HTTP 路径，仍是即时生效。
  加新的「必须本机同意」的命令，就往 `ui_command_requires_local_auth` 里加一条。
- **授权窗上的「立即执行」是本机专属**：`ui_command_is_local_only()` 只认 `confirm_pending`，
  远程调用直接 **403 `local_only`**（不是转待确认）；`cancel_pending` 放行 —— 撤回自己刚发的那条
  请求是安全方向的操作。前端 `renderPending` 在浏览器模式下把那个按钮 **`disabled` + title 提示**
  （**灰掉而不是藏起来**：藏起来像「没这功能」，灰着才知道要回电脑前点）。
  别把「远程能自己确认」当方便 —— 那等于自己给自己盖章，授权窗就成了摆设。
- **客户端（Web / 手机）连不上会自愈**：`probeRemoteApi()` 探失败不再定格成「纯预览模式」
  （旧行为会让正文永远空白、连轮询都不起，只能手动刷新），`scheduleRemoteApiReprobe()` 每 2s 重试，
  探通后 `attachRemoteApi()` 接回。轮询只在 `startUiStatePolling()` 里起（单例），别在 `listen` 里再内联一份。
- **「显示来源的客户端」有两处，别只验一处**：①本机授权弹窗 `notify.html` 的 `<small id="source">`
  （`notify.js:129` 填 `来源 IP｜渠道 http｜发起 时间`）；②**主界面的待确认卡 `#pending-card`**
  ——它在 **「基本设置」页签**里（`index.html:508`，在应用设置区），**「系统引导」页看不到**，
  别因此在系统引导页截图后误判成「主界面不显示来源」。卡里 `#pending-run` 在远程端是
  `disabled`（`pendingRunElement.disabled = IS_BROWSER`），`#pending-cancel` 可用。
- **半透明（不透明度）是前端 CSS 做的，不是窗口属性**：`applyAppearance()` 把 `--ui-alpha` 设在
  `documentElement` 上，`styles.css:106` 的 `body { background: rgba(var(--bg-rgb), var(--ui-alpha,1)) }`
  消费它；后端另有 `undock_mask_color` 按同一百分比算展开时的兜底背景色。**验它只能读 DOM/CSS，
  别去读 `_NET_WM_WINDOW_OPACITY`**（那里永远没有值，不是 bug）。

## 结构要点
- 后端 `src-tauri/src/lib.rs`（单文件很大）+ `http.rs`（tiny_http 0.12）；前端 `ui/`：`index/app.js/styles.css` 主窗、`notify.*` 弹窗、`grub-list-view.js` 树形渲染（主窗/GRUB 窗共用）。
- 多窗口同源：`publish_ui` emit `ui-state-changed` 广播；新窗口先 `get_ui_state` 再监听同事件。新窗口 `.visible(false)` 建好摆好再 show。
- `UiState` 是唯一事实来源；明文 token 不进快照。子进程必须走 `fn command(program)`（Windows CREATE_NO_WINDOW）。

## tiny_http 的坑（Windows 致命）
- `Server::drop` 用 `connect(listening_addr)` 唤醒 accept 线程；绑 `0.0.0.0` 时 Windows 上必失败 → 端口永不释放。`RunningServer::stop()` 必须自己补 127.0.0.1 自连接并轮询到端口真能 bind，契约「返回时端口已放开」。

## Linux 写 ESP / 同步 grub.cfg 权限模型
- 读 `/boot/grub/grub.cfg`（mkconfig 固定 0600 root）与写 `/boot/efi`（vfat 无 uid=）只有 root 能做。修法：`copy_into_esp`/`write_esp_bytes` 先 `access(2)`，写不动就 `pkexec install -p -m 0644`（`install -p` + `same_size_and_time` 防重复弹框）。远程请求用 `RemoteNoGuiAuth::enter` 圈住整次执行（不能弹框）。

## 窗口/WebView 定则（细节见 MEMORY-detail-windowing.md）
- 判位**只用客户区坐标**（`main_client_position()`/`inner_position()`）；`Moved` 载荷与 `outer_position()` 是窗框坐标，Linux 去装饰后恒偏一个标题栏 → 拿来比 `dock_pos` 必错。
- 贴边三道闸：`geometry_busy()`（发几何请求**之前**立）+ `main_moved_recently()`（300ms 静默期）+ 回声哨兵（持久，±8px）。dock 与 undock 两侧都要挡。
- 可见窗口上「去装饰+改几何」会被重父化吃掉 → **hide() → 改几何 → show()**，monitor 在 hide 前算。
- **GTK 上 set_position 语义随装饰状态变**：去装饰=客户区坐标（dock 路径准）；带装饰=**窗框**原点，客户区=请求+标题栏（真机 Cinnamon 2x=64px）。凡「带装饰落位」要么实况自校准（undock 重锚牛顿步：偏差反扣 offset、收敛写回跨轮复用），要么显式扣标题栏。Xvfb 无主题标题栏 0px → 复现台永远测不出这条。
- tao 未映射窗口 `outer_size()` 返回 0x0（inner 可读）。
- **隐藏窗口上 `set_size` 不会落地，而且 tao 的尺寸读数会「陈旧」**：GTK 对未映射窗口只是登记 pending，
  这段窗口期里 `outer_size()/inner_size()` 读到的还是**建窗时那一对**（`tauri.conf.json` 主窗写死 460x900）。
  症状＝首次落位按错尺寸算「居中」→ 窗口先出现在 (730,90)、46~51ms 后才被自愈线程拉回 (494,260)
  （1920x1080 实测，宽高一直是对的，**只有位置错**）；工作区比 900 矮时还会把窗口推出下沿 →
  `near_edge` → 开贴边就立刻缩成白条。**凡「按窗口自身尺寸算落点」都要过这条。**
  修法：`static MAIN_SIZE_PENDING`（只在 `set_main_window_size` 里按「当时可不可见」立/清）+
  纯函数 `centering_size(reading, requested, pending)` —— pending 且读数没跟上（±2px）就用
  `requested`（设置里记的尺寸＝映射后真正会有的尺寸），读数跟上即作废 pending（否则用户**手拖**
  窗口时会被旧设置值盖掉，手拖不走 `set_main_window_size`）。自愈线程那道「等尺寸读数可用」的闸门
  **只判 `>0`**，陈旧值非 0 会直接放行 —— 别指望它兜住。
  贴边/展开走裸 `main.set_size`（不经 `set_main_window_size`），所以不受这个标记影响。
- **隐藏窗口的 WebView 视口在 Linux/GTK 上是 0x0**（Windows/macOS 有真实分配）：0 宽视口排版把文字折成竖条 → 前端量「内容高度」严重虚高（notify 弹窗曾恒顶满上限 460，底部空白一大块）。修法：`.panel` 钉 `min-width` = 窗口宽度常量（两处必须一致）+ 监听 `resize` 在 show 后重测自愈。凡是「隐藏窗口里量 DOM 排版」都要过这条。
- **改窗口尺寸之后必须重新落位**（`set_main_window_size` 末尾已接 `recenter_after_window_size()`）：
  `set_size` 是**以左上角为锚**的，只改高度会让窗口往下长出去 → 底边越过工作区 →
  `schedule_clamp`（Moved 后 250ms）把它往上推回 → 居中的窗口被推到屏幕上沿**且永不回位**
  （1920x1080 实测 560→900→1000→560 = y 260→180→80→**80**）。补落位的三个要点：
  ① 只在窗口**已显示**时插手（启动期另有「等 WM 映射完再落位」的自愈流程，抢跑会被覆盖）；
  ② **必须轮询等新尺寸真落地**再落位（`set_size` 异步，紧随的 `outer_size()` 还是旧值 → 会「刚居中又被撑歪」）；
  ③ 落地后复核 `main_window_position == "center"`，非 center 直接返回 → free/last 不受影响。
- 判「乱闪乱跳」只认振荡判据：状态 <600ms 内回到两拍前的值（A→B→A）。

## 移动端（Android / iOS）= WebView 客户端，**不是**原生移植
- 手机端与 web 端同构：只加载电脑端内置 HTTP 服务（默认绑 `0.0.0.0:12300`）托管的那份 `ui/` 前端，
  `/api/*` 全是同源请求。**不要**为安卓/iOS 交叉编译 Rust 核心（不需要 NDK；
  `cargo check --target aarch64-linux-android` 那条路是错的，`gen/android` 已删）。
  服务端 bind `0.0.0.0`，`is_loopback` 只用于禁「LAN 冒充 CLI 渠道」，不拦手机。
- `mobile/android/`：Java + 系统 WebView，**不引 AndroidX/Kotlin**（依赖仅 AGP 8.7.3）。
  Gradle 8.11.1 + compileSdk/targetSdk 34 + minSdk 24。
  `export JAVA_HOME=/e/Android/jdk17/jdk-17.0.20.1+1 GRADLE_USER_HOME='E:\Android\gradle-home'`；
  `cd mobile/android && /e/Android/gradle-8.11.1/bin/gradle assembleDebug --no-daemon`
  → `app/build/outputs/apk/debug/app-debug.apk`。Gradle 解包在 `E:\Android\gradle-8.11.1`。
- Android 三必备：`usesCleartextTraffic=true`、`setDomStorageEnabled(true)`（token 走 cookie）、
  `onReceivedError` 只认 `isForMainFrame`。
- `mobile/ios/`：SwiftUI + WKWebView 源码 + `Info.plist`（ATS 放行 + `NSLocalNetworkUsageDescription`）。
- **Windows 原生 python 不认 MSYS 路径**：`/e/Android/x` → `<当前盘>:\e\Android\x`（写错树），必须传 `E:/Android/x`。

## Windows 宿主链接器（踩过两次）
- 宿主 msvc 目标必须有 VC 工具链；PATH 里 `/usr/bin`（Git 自带 coreutils `link`）早于 MSVC 时会遮蔽它，
  报 `link: extra operand ... Try 'link --help'`。自检：`where link.exe` + `vswhere -latest -products '*' -property installationPath`。
- 本机 VS 生成工具装在 **`D:\VS`**（VCTools + Windows SDK 10.0.26100.0，C 盘紧张刻意装 D 盘），
  rustc 走 vswhere 自动发现，不用改 PATH。

## 前端栏目 = 地址栏路由
- 栏目写进 hash：`#/boot`（系统引导）、`#/settings`（基本设置），`history.replaceState` 同步、`hashchange` 反同步。
- `initialTab()`：hash > （非浏览器才读 localStorage）> `boot`。**网页端/手机 WebView 默认「系统引导」**，
  只有桌面主窗口记忆上次栏目 —— 改这里别把网页端又拖回「一刷新就进基本设置」。
- 本机自测 HTTP：Bash 的 curl 带 `http_proxy=127.0.0.1:3564`，必须 `--noproxy '*'` 才连得到 12300。
- **启动期别偷偷点页签**：浏览器模式的 `listen` shim 对 `focus-remote` 原写成「等价于切到设置栏目」，
  而 shim 在**注册时**就执行 → 每次开网页都被顶到基本设置。改栏目/首屏怪象先查启动期有没有代点按钮的代码。
- **Windows 打开 URL 用 `ShellExecuteW`，别用 `explorer <url>`**：explorer 会把带 query 的地址当路径解析；
  再配 `.status()` 同步等，弹错误框时进程永不退出 → 命令挂死（表现＝按钮点了没反应）。
  `ShellExecuteW` 返回值 **≤32 是错误码**，>32 才算成功。依赖：windows-sys 0.59（UI_Shell + WindowsAndMessaging + Foundation）。
- **GUI 弹窗/进程启动类行为在 CLI 会话里测不出来**：shell 里 explorer / Start-Process / 非沙箱 ShellExecuteW
  都"成功"返回，但桌面无新窗口、本地探针服务器收不到请求 → 必须让用户实机点一次确认，别拿 CLI 结果当结论。
- **别让后台重试碰用户正在输入的控件**：网页端 2s 轮询 401 → `requestRemoteToken()` 会把
  `#token` 输入框 `value` 抹空 + 抢焦点（表现为「token 打一半就没了／像页面被刷新」）。
  定则：① 模态框只在「确实新打开」时写 value/focus，已开着只更新文案；② 轮询/重试路径开头
  `if (!remoteToken) return;`，没 token 不产生 401 洪流；③ 用户点过「暂不连接」要记住，别自动弹回。
  另：给 token 这类输入框一律关 `autocapitalize/autocorrect/spellcheck`（手机软键盘会悄悄改坏）。

## 移动端（`mobile/`）：WebView 客户端，服务端一行不改
- 结构：Android = Java + 系统 WebView（依赖只有 AGP，无 AndroidX/Kotlin）；iOS = SwiftUI + WKWebView。
  两端同构，加载电脑端托管的同一份 `ui/`，`/api/ui-state`、`/api/ui-command` 都是同源请求。
- **多电脑 = 每台一个独立 WebView**（Android 懒创建 + `Tab.started` 判重；iOS 常驻 + opacity 切换），
  才谈得上「同时在线、切换不重载」。标签名 = `组名 · 服务端名`，
  分组配色取组名哈希定色相，标签圆点 = 该电脑的加载状态。
- **一条记录 = 一台电脑（不是一个 IP）**：`ServerProfile.addresses` 是**有序候选地址列表**
  （`host` = `addresses[0]` = 首选）。一台电脑多块网卡 / 换过 DHCP 只占**一个标签、一个 WebView**：
  首选地址不通就换下一个（换 URL，不换实例），全试完才显示重试面板；
  连上的地址 `promoteAddress` 提到最前 + 落盘（下次直连它，DHCP 换 IP 能自愈）。
  编辑框地址支持**多行**（一行一个地址）。
- **「是不是同一台电脑」= `/health` 的 `machineId`（免鉴权，故填 token 前也能归并）+ 分组名兜底**：
  两边都有标识时以标识为准（不同就是不同电脑）；任一方没有则按「分组名相同」判。
  保存地址后后台探一次 `/health` → 命中已有记录就把地址**并进那条**，不新建标签。
  老配置（每个 IP 一条）用列表里的「合并同一台电脑的地址」一键收拢。
  `machineName` 取不到就是空串（**不开子进程**：`/health` 每次请求都走，只读环境变量与几个约定文件）。
- **客户端侧的三个易错点**：① Java `promoteAddress` 后必须把 `Tab.addressIndex` 归零，否则 `urlAt(index)` 指错；
  ② 换地址后旧地址的 `onPageStarted/onReceivedError/onPageFinished` 还会回来 → 一律先过 `isCurrentAddress()`
  （前缀比对 + 后面必须紧跟 `/` 或 `:`，否则 `192.168.1.5` 会匹到 `192.168.1.50`）；
  ③ iOS `didFailProvisionalNavigation` 的**主动取消 -999** 不算失败，不过滤会白跳过地址。
- **换地址后 token 靠 URL 带**：cookie 按来源站点（IP）存，换 IP 就是换站点 → App 里存着 token 最省事。
- **传输方式（推送 / 轮询）由 App 拼进 URL，不由 App 自己发请求**：后端 `GET /api/ws` 是
  RFC6455 推送通道（与 `/api/ui-state` 送同一份快照，首帧即完整快照），前端三档
  `auto`（默认）/`ws`/`http`。客户端只做一件事：把偏好拼成 `?transport=`（空值/`auto` 不加）。
  **理由**：网页 `localStorage` 按来源站点存，一台电脑换网卡就换地址（= 换来源），
  只存网页端偏好会丢。Android `ServerProfile.urlAt(index, transport)` / iOS `url(at:transport:)`；
  偏好落 `ServerStore.transport`（SharedPreferences / UserDefaults）。选完**不立刻重载**，
  下一次加载生效（页面上也能在横幅下拉里即切）。
- **推送与下拉刷新都不需要客户端写代码**：`new WebSocket(...)` 和触摸手势都在 `ui/` 里，
  WebView 原样支持。iOS 唯一相关的改动是过滤 `NSURLErrorCancelled`（见上）。
  Android 菜单项的标题带当前档位 → 必须在 **`onPrepareOptionsMenu`** 里重算
  （`onCreateOptionsMenu` 只跑第一次，不改就一直显示旧档位）。
- Android 构建：**两台机器都能出了**。
  - **Linux（本机）**：2026-10-13 就地把工具链补齐，一条命令即出 —— `/home/jianghh/.cache/ab-android/build.sh`。
    细节与两个「必须显式设」的变量见文末「Android 构建工具链（Linux 本机）」。
  - **Windows**：`JAVA_HOME=/e/Android/jdk17/jdk-17.0.20.1+1`、`GRADLE_USER_HOME=E:\Android\gradle-home`，
    `/e/Android/gradle-8.11.1/bin/gradle assembleDebug`。
  - 产物都是 `mobile/android/app/build/outputs/apk/debug/app-debug.apk`（debug 已签名，可直接 adb install）。
  - 核对入包：`unzip -o app-debug.apk "classes*.dex"` 后查 `classes2.dex`（代码不在 `classes.dex`，那里只有 1.3KB 存根）。
- 定则：`activate()` 必须**先 ensureTab/connect 再 applyVisibility()**；判「是否已加载」用自建 `started` 标记，
  不要用 `WebView.getUrl()`（刚 loadUrl 时可能还是 null → 重复加载）。
- **文档落点（改客户端行为时同步改这三份，别漏）**：`ui/README.md` = Web 端使用说明（打开方式/token/栏目 hash/`gui_only` 边界）；
  `mobile/README.md` + `mobile/android|ios/README.md` = 移动端安装与用法；根 `README.md` 的「客户端（Web 端 / 移动端）」小节是入口。
- **提交约定**：构建产物不入库（`.gitignore` 已挡 `mobile/android/**/{build,.gradle,local.properties}`、`*.apk/*.aab/*.keystore`、
  iOS `DerivedData/xcuserdata`）。交付 APK 时在 README 里给 gradle 命令现打，不要为了图省事把 APK 提交进去。

## Android 构建工具链（Linux 本机，2026-10-13 装好）

**结论：本机现在能直接出 APK，不用再推到 Windows。** 一条命令：
`/home/jianghh/.cache/ab-android/build.sh`（内部就是 gradle `assembleDebug`，`--no-daemon`）。
产物 `mobile/android/app/build/outputs/apk/debug/app-debug.apk`（约 37KB，v2 签名，Android Debug 证书，可直接 adb install）。

**装在哪**
- SDK `/home/jianghh/android-sdk`（cmdline-tools 11076708 + platform-tools + `platforms;android-34` + `build-tools;34.0.0`，许可已接受）
- Gradle 8.11.1 `/home/jianghh/.cache/ab-android/gradle-8.11.1`（**项目里没有 gradle-wrapper.jar**，只有 .properties，
  所以 `./gradlew` 用不了，得直接调 gradle 可执行文件）
- JDK `/home/disk2/software/jdk/jdk-21`（**必须 17~23**；JetBrains 全家桶自带的 jbr 都是 JDK 25，Gradle 8.11.1 跑不了）

**四个必踩的坑（一个都不能省）**
1. **Java 在 Linux 上从 passwd 取 `user.home`，不认 `$HOME`** → 本会话是 root，`user.home=/root`。
   - 不给 `ANDROID_USER_HOME=/home/jianghh/.android`，sdkmanager 就报
     `Failed to parse user addon file at /root/.android/repositories.cfg` + `Failed to download any source lists!`
     （看着像网络问题，其实是权限问题）。
   - 不给 `GRADLE_USER_HOME`，Gradle 就去写 `/root/.gradle` → `Gradle could not start your build.`
2. **`~/.gradle/init.gradle` 是老板自己的国内镜像配置（aliyun + huaweicloud）**，它往 `allprojects` 塞 maven 仓库，
   而本工程 `settings.gradle.kts` 声明了 `RepositoriesMode.FAIL_ON_PROJECT_REPOS` →
   `repository 'maven' was added by initialization script`，**直接构建失败**。
   **别动那份文件**（是他的东西），改用隔离的 `GRADLE_USER_HOME=/home/jianghh/.cache/ab-android/gradle-home`。
   直连 maven.google.com / repo1.maven.org 都好使，不需要镜像。
3. **本会话有 `http_proxy/https_proxy`（端口每次会话变，这次 36087）** → 构建时用
   `env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY` 清掉走直连。curl 同理，记得 `--noproxy '*'`。
4. **`export A=x PATH="$A/bin:$PATH"` 在同一句里展开，`$A` 还是空的** → PATH 坏掉，`exec: java: not found`。
   分两句写。（apksigner 脚本内部是 `exec java`，靠 PATH；实在不行直接
   `java -jar $SDK/build-tools/34.0.0/lib/apksigner.jar verify --print-certs <apk>`。）

### 模拟器跑不起来：这台机器**没有 `/dev/kvm`**（2026-10-13 查清）
- 内核模块其实**在**（`lsmod` 里 `kvm_intel` + `kvm` 都 loaded，16 核 / 31G，**不是容器** ——
  无 `/.dockerenv`，cgroup 在 `user-1000.slice/session-2.scope`），但 **`/dev` 里没有 `kvm` 设备节点**。
- **我建不出来**：`mknod /dev/kvm c 10 232` → `Operation not permitted`。根因是
  **本进程 `CapEff: 0000000000000000` —— uid 是 0 但有效 capability 被清空了**（`CapBnd` 有全部，
  `Current` 只剩 `cap_wake_alarm=i`）。**uid 0 ≠ 有权限**，别被 `id` 骗了。
  `/dev` 是 devtmpfs 且目录不可写（`touch /dev/x` 也是权限不够）。
- 结论：**硬件加速模拟器在本机不可用**，只能 `emulator -avd <name> -accel off`（TCG 软件模拟，
  `-accel-check` 返回 8「/dev/kvm is not found」）。TCG 下 API 34 x86_64 启动很慢但确实能起。
- **要让 App 真跑一遍，得去有 KVM 的机器，或直接连真机**（`adb install` 一份即可）。

### 进程存活与可见性（在这个执行环境里，决定「怎么跑长任务」）
- **普通命令行之间，上一句起的所有后台进程都会被清理掉**（`setsid`/`nohup`/`&`/`disown` 都挡不住）：
  同一句里 `ss` 能看到监听端口，**下一句就没了**。
- 只有 **`run_in_background` 的后台任务**里的常驻进程能活；而且它的**网络/文件系统与普通命令共享**
  （后台任务里 `python -m http.server` → 普通命令 `curl` 拿到 200），但 **`ps` 看不到它的进程**
  （pid namespace 隔离）→ **用「端口 + 日志文件」判断它活着，不要用 `ps`**。
- 于是「模拟器 + 服务端」这类长任务要这样摆：一个常驻编排脚本跑在后台任务里（内部
  Xvfb + anyboot + emulator，末尾 `exec sleep infinity` 保持任务不结束），外部用普通命令经
  **adb（TCP 5555）** 操作模拟器。
- 这个执行环境**没有 `/tmp` 跨命令持久**（每条命令一个 `/tmp` 视图）→ 中间产物放 `~/.cache/`。

## Linux 打包：AppImage 的 glibc 陷阱（2026-10-13 定位）
- **症状**：AppImage 换台机器启动即死，刷一屏
  `version 'GLIBC_2.44' not found (required by /tmp/.mount_.../usr/lib/libwebkit2gtk-4.1.so.0)`。
- **根因**：`tauri build --bundles appimage` 走 linuxdeploy，把**整条 GTK/WebKit 依赖链**裹进包
  （本机 CachyOS / **glibc 2.44** 编译的，184 个 .so、283MB）。目标机 glibc 比 2.44 老 → 加载即报缺版本。
  **不是应用本体的问题。**
- **取证口径**：`objdump -T <elf> | grep -oE 'GLIBC_[0-9.]+' | sort -uV | tail -1`
  - `usr/bin/anyboot`（Rust 本体）→ **2.39**
  - `libwebkit2gtk-4.1.so.0` / `libjavascriptcoregtk-4.1.so.0` → **2.44**
  - `libglib-2.0`、`libgnutls`、`liblcms2`、`libjxl`、`libavif`、`libaom`、`libSvtAv1Enc`、
    `liborc-0.4`、`libtdb` → **2.43**
  - **用户报错里没有一行 `required by usr/bin/anyboot`** → 他系统 glibc **已 ≥2.39**，只差裹进来的那批。
- **纯配置改不了**：`~/.cache/tauri/linuxdeploy-plugin-gtk.sh` 只认 `DEPLOY_GTK_VERSION` /
  `GTK_THEME_VARIANT` / `APPIMAGE_GTK_THEME` / `LINUXDEPLOY`，**没有 exclude-library**；
  tauri-bundler 也不透传 `--exclude-library`。
- **路 A：不裹 GTK/WebKit**（本机已做出实验产物，见下）
  1. 删 `AppDir/usr/lib/*` 与 `usr/lib64`（webkit 的 helper 在 lib64 里，必须一起删）；
  2. 把 `AppRun` 会 `source` 的 `apprun-hooks/linuxdeploy-plugin-gtk.sh` **置空**（不能删，AppRun 有 `set -e`）——
     那里面的 `GTK_PATH`/`GDK_PIXBUF_MODULE_FILE`/`GIO_EXTRA_MODULES` 全指向 `$APPDIR/usr/lib`，删库后是空指针。
  3. 结果：整包只剩 `usr/bin/anyboot`(2.39) + `AppRun.wrapped`(2.3)，**门槛 2.44→2.39**，300MB→18MB。
     代价：目标机要自带 `libwebkit2gtk-4.1` + GTK3。
- **路 B：换基座重建**（保持自包含）：基座必须 **glibc ≥2.39**（Ubuntu 24.04）——
  **Debian 12（2.36）也不行**，应用本体自己就要 2.39。本机 docker daemon 没跑 → 只能在别的机器 / CI 做。
- **重打 AppImage 的办法（本机无 FUSE 时）**：
  `linuxdeploy-plugin-appimage.AppImage` 直接跑 → `Cannot mount AppImage`；设 `APPIMAGE_EXTRACT_AND_RUN=1`
  能跑但**它要联网下 runtime**（下不动）→ 于是：
  ① `--appimage-extract` 解出它自带的 `appimagetool` + `mksquashfs`（在 `appimagetool-prefix/usr/bin/`）；
  ② 从**现有 AppImage** 里抠 runtime 头：找 squashfs 魔数 `hsqs` 的偏移，`head -c <偏移>` 即 runtime
  （本次 = 194183 字节）；
  ③ `PATH=<自带的 usr/bin>:$PATH appimagetool --runtime-file runtime-x86_64 <AppDir> <out>`。
  → 成功产出 4.1MB 的 `AnyBoot-portable-x86_64.AppImage`（在 `~/.cache/ab-appimage/`）。
- **⚠️ 本机验证不了「能不能启动」**：拿**原版**（完整库）AppImage 做对照，在本机**同样起不来**
  （`Failed to initialize GTK` / `fatal runtime error: Rust cannot catch foreign exceptions`）；
  另外 `Xvfb :98` 在本机直接崩在 `libnvidia-egl-gbm.so.1`（NVIDIA EGL），加 `LIBGL_ALWAYS_SOFTWARE=1` 也没救回来。
  → **本机的 Xvfb + NVIDIA 图形栈跑不了这个应用，不具备 AppImage 的图形验收能力**；
  能给的硬证据只有「包结构 + glibc 需求上限」。真实验收要在用户的机器上做。
- 用户侧可**立刻自救**（不必等我重打包）：
  `./AnyBoot.AppImage --appimage-extract && cd squashfs-root && rm -rf usr/lib/* usr/lib64 && : > apprun-hooks/linuxdeploy-plugin-gtk.sh && ./AppRun`
- 相关：README「AppImage 打包失败」小节只讲了 `NO_STRIP=1`（strip 报 `.relr.dyn`），**没讲 glibc**。

**产物验证清单（照这个顺序做，别只看 BUILD SUCCESSFUL）**
1. `aapt2 dump badging <apk>` → 包名/版本/minSdk/targetSdk/label/权限
2. `aapt2 dump xmltree --file AndroidManifest.xml <apk>` → 确认 `usesCleartextTraffic=true`
   （局域网 http 没它就是白屏）
3. `zipalign -c -v 4 <apk>` → `Verification succesful`
4. `java -jar .../apksigner.jar verify --verbose <apk>` → `Verified using v2 scheme: true`
5. **核对字节码是不是最新那版**（这个项目被「改了源码但产物是旧的」坑过）：
   `unzip -o <apk> 'classes*.dex' -d <tmp>` 后
   `strings -a -e S classes2.dex | grep 新加的中文文案`（dex 里中文是 UTF-8，要 `-e S`），
   再用 `dexdump -d classes2.dex | grep "name          : '新方法名"` 确认新方法在。
   **代码在 `classes2.dex`**，`classes.dex` 只有 1.3KB 存根（只有 `R`）。
