# AnyBoot 项目长期备忘（细则查当日日志 2026-10-10~16.md）

## 构建 / 打包（桌面端）
- 前端改动必须强制 codegen：`rm -rf src-tauri/target/release/build/anyboot-*/out && touch src-tauri/build.rs && cargo build --release`；核对用 node `zlib.brotliDecompressSync` 解包 out/tauri-codegen-assets/*；二进制里搜中文要 `buf.includes(串,"utf8")`（latin1 匹配不上）。
- 交付 = 测试 + release 构建 + 实机验证；构建前退出运行中的 anyboot；产物 md5 必核（Text file busy 会静默失败）。
- 打包：`TMPDIR=~/.cache/ab-tmp npm run build:linux`（deb+rpm）；portable 走 `scripts/make-appimage-jammy.sh`（jammy 基座+zig 重链 gnu.2.35；ELF 改字符串必须等长替换）。`~/.cache/tauri/linuxdeploy-x86_64.AppImage` 必须用新版（continuous 369+，旧版 strip 挂 .relr.dyn）；runtime 偶发挂 → `LDAI_RUNTIME_FILE=~/.cache/tauri/runtime-x86_64`。
- `ui/index.html` 的 data-page-node-id 原样保留；标题字号别挂在会搬家的容器下。文档路径写 ~，不写死 /home/jianghh。

## 沙箱验证环境
- Xvfb 三件套：`mkdir /tmp/.X11-unix && chmod 1777`；Xvfb 加 `__EGL_VENDOR_LIBRARY_FILENAMES=…50_mesa.json`（只给它加）；HOME/XDG 指可写路径。模拟用户屏 1920x1080（GDK_SCALE=2 时 3840x2160）。
- 窗口行为复现台首选：Xvfb 对齐真机 + 真 WM（mutter；kwin/xfwm4 有坑）+ app 本尊，配置照抄用户 settings.json；xdotool 驱动、xwininfo 读几何。样板 `~/.cache/ab-dock-min/`，收进 skill linux-x11-window-repro。
- 脚本放 ~/.cache/（/tmp 跨沙箱隔离）；判活看「应用日志已监听」+端口；每次验证换端口；curl 加 `--noproxy '*'`，构建/浏览器清代理。
- `xdotool search --name` 中文正则不可靠（枚举逐字比较）；判「在不在」看 Map State。
- 探针坑：`grep -c` 无匹配退出 1（写 `n=$(grep -c …); n=${n:-0}`）；函数里 `…|while…return 0` 退出的是子壳，改 for。
- 取证：startup_debug() → `<config>/startup-debug.log` 常开；用户可见动作必须走常开通道或 record_action。
- 读运行中应用状态：`curl -H "X-Auth-Token: <httpToken>" http://127.0.0.1:<httpPort>/api/ui-state`。
- **禁用 pkill -f <路径>**（连自己 shell 一起杀），按 /proc/PID/exe 比对。

## WebKitGTK × NVIDIA
- X11 唯一正解 `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`；WEBKIT_DISABLE_DMABUF_RENDERER 已失效勿回用。

## 托盘（双形态）——基线=440189c 线
- trayMenuStyle 启动定接线，切换重启生效。classic（默认）=Win/mac muda 原生菜单；Linux 自研 linux_sni 全量 dbusmenu（muda 0.20 GTK3 子菜单回归绕道）。unified=Win/mac 自绘窗；Linux SNI 导出 dbusmenu（GNOME 壳菜单只有[显示主窗口,退出程序]，opened→弹自绘窗+150ms Esc）。
- 自绘窗落位：unified 贴托盘图标**正下方**居中、放不下翻正上方，绝不盖图标；classic 上方优先+左对齐。TrayCenter 已弃用。
- 失焦看门狗 250ms 轮询；AboutToShow=真脏标记（GetLayout 应答清零）；LayoutUpdated 防重发靠 MENU_SIGNAL_REVISION。
- GRUB 区：grub_menu_nodes 一份函数喂四处（主窗/unified 自绘窗/Linux dbusmenu/Win mac 原生菜单）；行文字箭头在前图标在后。
- dbusmenu 定则：信号从 Item.Menu 指向路径（MENU_PATH）发；回显走 ItemsPropertiesUpdated 属性差分（都没变不发）；信号体签名 `a(ia{sv})a(ias)`；禁带 toggle-type/toggle-state（勾选画进 label ◉/○）。ItemIsMenu=false + Menu=object path。Tauri 2 新窗口 label 必须进 capabilities windows。
- macOS 原生对话框不可用（rfd 阻塞 ~30s）→ `osascript display dialog`。
- 复现台：KDE `~/.cache/ab-kde-repro/`（kdemenu_client.cpp）；回归 `~/.cache/ab-dock-min/cin/probe40.sh`；UI 按钮 `~/.cache/ab-headerbtn/`。

## 产品定则
- 默认值常量钉着（改值同步单测）：亮色/90%/remoteAllowGuiAuth=true/默认高度=整屏 2/3；window_height==0 是「没记过尺寸」哨兵。落位 free/center/last：center=禁拖。
- 下次启动大字=级联全链，Rust 文案与前端一字不差。
- 忙闸必须有放闸路径（判据不能是「命令返回成功」；POWER_RELEASE_MS=15s 世代号）；远程动作=本机点头；灰掉而不藏按钮。
- 顶部标题栏=电源三键+重启程序/退出程序；**restart_app 顺序：stop_http_server → release_instance_lock → app.restart()**。单实例锁 anyboot.lock（File::try_lock），守卫放 setup 最前，拿不到锁只提示一行都不做。
- tiny_http Windows：stop() 必须 127.0.0.1 自连接轮询；Linux 写 ESP 探测失败走 pkexec。

## 移动端（mobile/）
- Android 已迁 **Jetpack Compose**（Kotlin；业务 Java 类保留）。gradle 构建：`GRADLE_USER_HOME=~/.cache/ab-gradle`、`JAVA_HOME=~/.sdkm/links/java`（21.0.11）、`ANDROID_HOME=~/Android/Sdk`；AGP 8.7.3 / Gradle 8.11.1（~/.gradle/wrapper/dists/...）。**本机 shell ulimit -v 4G（硬限）**：daemon jvmargs 必须压小，JdkImageTransform/jlink 子进程要 `_JAVA_OPTIONS="-Xmx768m -Xms64m"` 限制，否则 metaspace 映射崩溃（hs_err）；网络可直连（清代理）。SDK 平台 android-34。
- 轻量链应急打包：`~/.cache/ab-apk/build.sh`（aapt2+javac+d8 BT35+zipalign+apksigner；须非沙箱，沙箱 JVM 假 OOM）。
- 主题：Compose 版 Theme.kt/Common.kt（三挡 auto/dark/light，终端区恒深 OLED）；PC 设计稿 docs/client-ui/*.html 双主题（ab-theme + localStorage）。
- 待办：iOS 同套 token 重构；PC 端 server-hub 独立窗口（设计已定稿）；密码 Keystore 问题定性（应急手输通道已加）。

## git
- 本机无 GitHub 凭据，push 必失败。**commit 由我做，push 由用户在 IDE 做**。远端 https://github.com/lidiaoo/anyboot.git
