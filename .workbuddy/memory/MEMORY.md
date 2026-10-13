# AnyBoot 项目长期备忘（细则查当日日志 2026-10-10~13.md 与 MEMORY-detail-windowing.md）

## 构建 / 打包（照抄这串）
- `export HOME=/home/jianghh CARGO_HOME=/home/jianghh/.cargo`（$HOME=/root 不可写）；**cargo 必须非沙箱跑**。
- 前端改动必须强制 codegen：`/usr/bin/rm -rf src-tauri/target/release/build/anyboot-*/out && touch src-tauri/build.rs && cargo build --release --manifest-path src-tauri/Cargo.toml`（out 在 **src-tauri/target** 下，项目根没有 target/；zsh glob 无匹配会中断命令串）。核对：node `zlib.brotliDecompressSync` 解包 `out/tauri-codegen-assets/*` grep；**在二进制里搜中文要 `buf.includes(串,"utf8")`**，latin1 解码后永远匹配不上（ASCII 不受影响）。
- 交付 = 测试 + `cargo build --release` + 实机验证；`cargo test` 不产出 exe；构建前退出运行中的 anyboot。
- Linux 全兼容包：`npm run build:linux-portable` → `scripts/make-appimage-jammy.sh`（jammy 基座 + zig 重链 x86_64-gnu.2.35；cargo 会覆盖 zig 产物，脚本开头自动重链；ELF 内改字符串必须**等长替换**+orig_size 断言）。产物 md5 必核（Text file busy 会静默打包失败、旧包冒充；脚本有三道防线）。
- `ui/index.html` 含设计工具注入的 **data-page-node-id 属性——编辑必须原样保留**，删改前先核对。
- 两区同构的样式规则（.grub/.refind 背板、两区 h2）必须走**共用选择器**，别只挂单一容器（踩过：删 .systems-head 后 rEFInd h2 回落浏览器默认 1.5em）。

## 沙箱验证环境
- Xvfb 三件套缺一即败：① `mkdir -p /tmp/.X11-unix && chmod 1777`；② **Xvfb 本体**加 `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json`（只给它加，别 export）；③ `HOME=/home/jianghh` + XDG 三目录指可写路径。模拟用户屏：1920x1080。
- **WebKit 渲染在沙箱必崩**（MiniBrowser/anyboot navigate 即 page crash，DMABUF/COMPOSITING 兜底开关也救不了）→ 截图、DOM 校验都过不去，观感验证只能实机。
- WebKitWebDriver：**只认 `-p <port>`**（`--port 4444` 打 Usage）；**MiniBrowser 继承 http_proxy** → 先 unset 全部代理变量；4444 被代理占，换高位端口。脚手架：`~/.cache/ab-ui-dom/wd_ui_batch.py`（UI 批次 14 断言）、`ab-verify/wd_defaults.py`/`wd_pending.py`；`execute/sync` 的 script 要 `return (()=>{...})()`。agent-browser + Chrome 在 `~/.cache/ab-browser`（open+eval 必须同一次 Bash 调用；eval 读不到 let）。
- 脚本放 `~/.cache/`（/tmp 跨沙箱隔离）；`rm -rf` 用 `/usr/bin/rm`；冒烟塞同一个 Bash 调用前台串完，落盘 `echo >>file`；后台进程用 run_in_background 托管、收摊 TaskStop；判活看「应用日志已监听」+端口，**别用 ps/单测端口**（旧实例会骗过）；每次验证换端口。
- 沙箱 http_proxy 常开：curl 加 `--noproxy '*'`；构建/浏览器全要清代理。
- `xdotool search --name` 对中文正则不可靠（先枚举窗口再 shell 逐字比较）；`xprop` 是空壳（读 X 属性用 ctypes 调 libX11.so.6）；点击用 `getwindowgeometry --shell` 算偏移，别写绝对坐标。
- 取证两通道：`trace()` → trace.log；`startup_debug()` → `<config>/startup-debug.log`（落位/贴边只认后者）。
- 冒烟脚本库：`~/.cache/ab-verify/`（run/real/auth/exec/resize/localclickswitch/explore/recall）、`ab-win/windows.sh`（三窗截图）、`ab-test/`（ws-live/ws-probe）。

## WebKitGTK × NVIDIA（Linux 白屏/局部重绘）
- X11 唯一正解 `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`（`apply_linux_gpu_workarounds()`，必须早于建 WebView）；`WEBKIT_DISABLE_DMABUF_RENDERER`（2.52+）与 `WEBKIT_DISABLE_COMPOSITING_MODE`（2.54+）已失效**勿回用**（单测钉死）。用户设过任一 WEBKIT_ENV_SWITCHES（含=0）就完全不插手（也是 A/B 开关）。NVIDIA 真机效果待用户确认。

## 产品定则（细节见日志）
- 默认值常量钉着（改值必须同步单测）：亮色 / 90% / remoteAllowGuiAuth=true / **默认高度=整屏高的 2/3**（1080→720；基准是 `monitor.size()` 不是工作区——工作区基准曾让重置按钮"点了没反应"，2026-10-13）；`window_height==0` 是「没记过尺寸」哨兵，别当尺寸用。
- 落位方式 free/center/last：**center=位置固定禁拖**（拖动弹回居中+提示换方式，节流 4s；回声/WM 抖动过滤见 CENTER_ECHO_* 静态量）。**锁定窗口大小只禁手拖缩放，不拦展示方式落宽**（`grub_layout_target_width` 无 locked 门槛，用户定案 2026-10-13）。
- 下次启动大字=级联全链（rEFInd›GRUB 组名›条目，条目文字与 grub.listFormat 同规则）；状态行「下次启动：」前缀一律去重。两区（rEFInd/GRUB）卡片背板与标题行共用 CSS 规则；**标题字号规则绝不能挂在会搬家的容器下**——`.grub-head` 搬进右栏后脱离 `.grub`（坑过两次），已单列 `.grub-head h2`。③④ 右栏 `.pane-body` 本身就是卡片（同款边框/底色/圆角，24px padding↔margin 挪位）；pane 模式 `.grub-head` 由 applyGrubLayout **搬出滚动容器**钉在卡片上方常驻（**sticky 对 grid/flex 子项受包含块约束，滚出自身轨道就失效——「钉住」需求直接搬出滚动容器**）。
- 远程动作=本机点头：`switch_system` 等走 `ui_command_requires_local_auth` → pending+授权窗；「立即执行」仅本机（远程 403）；灰掉而不藏按钮。
- `UI_REVISION` 起点跟时间挂钩（`ui_revision_seed`）；凡「按版本号丢过期数据」先问跨进程单调性。
- `split_url()` 先剥代理绝对 URI（否则网页整片空白）；静态路由要覆盖「自然写法」并钉单测；`/api/ui-command` 回执必须放 `result` 里。
- 验「断线自愈」：断线期间改宿主状态，重连见新值才算真自愈。

## 结构 / 权限 / 窗口（窗口细则见 MEMORY-detail-windowing.md）
- 后端 `src-tauri/src/lib.rs`（单文件大）+ `http.rs`（tiny_http 0.12）；前端 `ui/`：index/app.js/styles.css + grub-list-view.js（主窗/GRUB 共用）。`UiState` 唯一事实源，明文 token 不进快照。
- tiny_http Windows 致命：`RunningServer::stop()` 必须 127.0.0.1 自连接并轮询到端口真能 bind。
- Linux 写 ESP：先 access(2)，写不动 `pkexec install -p -m 0644`；远程请求圈 `RemoteNoGuiAuth::enter`。
- 窗口定则：判位只用客户区坐标；改几何=hide→set→show；GTK 带装饰 set_position 是窗框原点；隐藏窗口 `set_size` 不落地且读数陈旧（MAIN_SIZE_PENDING 处理）；隐藏窗口 WebView 视口 0x0；改尺寸后必须等新尺寸落地再重落位。

## 移动端（mobile/，WebView 客户端，非原生移植）
- 只加载电脑端内置 HTTP 服务托管的 `ui/`，`/api/*` 同源；**别交叉编译 Rust 核心**（无 NDK）。
- 多电脑=每台一个独立 WebView；一条记录=一台电脑（`addresses` 有序列表 + `/health` 的 machineId 归并，promoteAddress 后 Tab.addressIndex 归零）；传输方式由 App 拼 `?transport=`，不立刻重载。
- **Linux 本机出 APK**：`/home/jianghh/.cache/ab-android/build.sh`（SDK `~/android-sdk`；JDK 21 `/home/disk2/software/jdk/jdk-21`；`ANDROID_USER_HOME`/`GRADLE_USER_HOME` 必须显式隔离；清代理；无 /dev/kvm 模拟器只能 TCG）。Windows 出包走 `E:\Android`（JAVA_HOME+gradle-8.11.1）。产物核对：`classes2.dex` 才是代码（`strings -a -e S` 查新文案）。
- 文档三份同步：`ui/README.md`、`mobile/README.md`(+子目录)、根 README 客户端小节；APK/构建产物不入库。
