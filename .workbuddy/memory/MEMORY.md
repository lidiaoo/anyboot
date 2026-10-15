# AnyBoot 项目长期备忘（细则查当日日志 2026-10-10~14.md 与 MEMORY-detail-windowing.md）

## 构建 / 打包
- **cargo 位置（2026-10-14 晚起）**：只在**沙箱内**有（/usr/bin/cargo，Arch rust 1.99，HOME=/root）；宿主机全盘无 cargo。构建走沙箱 + `export HOME=/home/jianghh CARGO_HOME=/home/jianghh/.cargo`；X11/dbus 探针才需要非沙箱。「cargo 必须非沙箱跑」已作废。2026-10-15 观察到用户终端可直接 `npm run build:linux` 完整编译——宿主机已有可用 cargo（rustup/PATH），以实况为准。
- 前端改动必须强制 codegen：`/usr/bin/rm -rf src-tauri/target/release/build/anyboot-*/out && touch src-tauri/build.rs && cargo build --release --manifest-path src-tauri/Cargo.toml`（out 在 src-tauri/target 下；zsh glob 无匹配会中断命令串）。核对用 node `zlib.brotliDecompressSync` 解包 `out/tauri-codegen-assets/*`；**二进制里搜中文要 `buf.includes(串,"utf8")`**（latin1 永远匹配不上）。
- 交付 = 测试 + release 构建 + 实机验证；`cargo test` 不产出 exe；构建前退出运行中的 anyboot。
- 打包：`TMPDIR=~/.cache/ab-tmp npm run build:linux`（deb+rpm）；`build:linux-portable` = no-bundle + `scripts/make-appimage-jammy.sh`（jammy 基座 + zig 重链 gnu.2.35，脚本开头自动重链；ELF 改字符串必须等长替换）。appimage（7ce6504 已加回 build:linux）：真机链路 2026-10-15 跑通——**~/.cache/tauri/linuxdeploy-x86_64.AppImage 必须用新版**（continuous build 369+，内置 binutils 2.47 认 RELR；旧 2024 版 strip 全挂 `.relr.dyn` → 整链挂，NO_STRIP 无效），备份 .bak-2024；runtime 下载偶发挂 → `LDAI_RUNTIME_FILE=~/.cache/tauri/runtime-x86_64`。沙箱内打包仍走 jammy 脚本。产物 md5 必核（Text file busy 会静默失败）。文档路径写 ~，不写死 /home/jianghh。
- `ui/index.html` 的 data-page-node-id 属性必须原样保留；两区同构样式走共用选择器；标题字号别挂在会搬家的容器下（`.grub-head h2` 单列）。

## 沙箱验证环境
- Xvfb 三件套：①`mkdir -p /tmp/.X11-unix && chmod 1777`；②Xvfb 本体加 `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json`（只给它加）；③HOME/XDG 指可写路径。模拟用户屏 1920x1080（+GDK_SCALE=2 时 3840x2160）。
- 窗口行为复现台（首选）：Xvfb 对齐真机分辨率/GDK_SCALE + 真 WM（mutter；kwin_x11 乱改尺寸、xfwm4 缺 xfconfd）+ app 本尊，配置走 XDG_CONFIG_HOME 副本照抄用户 settings.json；xdotool 驱动、xwininfo/xprop 读几何。样板 `~/.cache/ab-dock-min/`，收进 skill linux-x11-window-repro。
- 脚本放 ~/.cache/（/tmp 跨沙箱隔离）；冒烟塞同一 Bash 前台串完；判活看「应用日志已监听」+端口（别用 ps）；每次验证换端口。沙箱 http_proxy 常开：curl 加 `--noproxy '*'`，构建/浏览器清代理。
- `xdotool search --name` 中文正则不可靠（枚举窗口逐字比较）；同进程有 20x20/32x32 辅助窗，取面积最大的；xwininfo 搜得到隐藏窗，判「在不在」看 Map State。
- 探针脚本两坑：①`grep -c` 无匹配输出 0 且退出码 1，`|| echo 0` 变两行——写 `n=$(grep -c …); n=${n:-0}`；②函数里 `… | while … return 0` 的 return 退出的是子壳，改 for 循环。
- 取证两通道：trace() 默认关；startup_debug() → `<config>/startup-debug.log` 常开。**用户可见动作必须走常开通道或 record_action**。
- 托盘假宿主探针：probe27-dbusmenu.py/run-probe27.sh（GNOME 流程：Menu 属性→GetLayout→Event）、probe-sni-click.py（Cinnamon 菜单点击）。RegisterStatusNotifierItem 参数以 / 开头时服务名=sender 唯一名；Event 不检查 enabled；Event 的 id 兼容 i32/u32。
- 读用户运行中应用实时状态：`curl --noproxy '*' -H "X-Auth-Token: <settings.json httpToken>" http://127.0.0.1:<httpPort>/api/ui-state`。冒烟库 `~/.cache/ab-verify/`、`ab-win/`、`ab-test/`。

## WebKitGTK × NVIDIA
- X11 唯一正解 `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`（apply_linux_gpu_workarounds，早于建 WebView）；WEBKIT_DISABLE_DMABUF_RENDERER / COMPOSITING_MODE 已失效勿回用。用户设过任一 WEBKIT 环境开关就不插手。

## 托盘（双形态：classic 原生菜单 / unified 自绘+SNI）
- **trayMenuStyle 二选一，启动时定接线，切换重启生效**（2026-10-15 用户定案）：
  - classic（默认）= 三端自绘之前那代：tauri 托盘 + **系统原生菜单**（6b7ef6c^ 恢复）——macOS 菜单挂托盘；Windows 摘菜单+锚点窗 popup_tray_menu 现建现弹（ui/menu-anchor.html，托盘不挂菜单右键仍发事件）；Linux libappindicator 宿主渲染（GNOME 右键也能弹）。绿勾位图 check_mark_*、菜单指纹 menu_signature/remember/invalidate（manage 里 Mutex<Option<String>>）、busy.menu_describe 都是 classic 专用，别再删。
  - unified = Win/mac 右键直开自绘窗（tray-menu，positioner TrayCenter 落位，**窗口高度封顶=锚点显示器工作区-2×GAP，超出靠 .menu 自滚**）；Linux 自研 SNI（linux_sni，unified 专用）导出**build_menu_nodes 全量内容的动态 dbusmenu** @ /StatusNotifierMenu——右键直接出完整菜单（无「打开面板」中转），点击 run_on_main_thread 派发 handle_menu_event。**AboutToShow=真脏标记（MENU_DIRTY：初始 true、变更置位、只在 GetLayout 清零）**——恒回 true 会把子菜单展开瞬间打回（扩展客户端开子菜单先调 AboutToShow(子id)，true→整树重拉重建=展不开，GNOME 实锤）；LayoutUpdated 防重发靠 MENU_SIGNAL_REVISION（已广播代次）。
- keepsOpen 只在后端算，前端照字段办事；分发统一走 handle_menu_event。
- positioner：插件 Tray* 位置只认原生托盘事件 → `on_tray_event(app,&TrayIconEvent)` 喂入口：Win/mac 在 on_tray_icon_event 喂真实 rect；Linux SNI 在 activate/dbusmenu click 喂 32×32 标称矩形。capabilities 加 positioner:default。
- Linux SNI 定则：ItemIsMenu=false + Menu=object path（类型错 GNOME 静默丢属性）。GNOME 扩展点击只认 dbusmenu；Cinnamon/KDE 见有效 Menu 会自建宿主菜单消费右键（ContextMenu 回调不来了）。
- dbus 0.9 坑：MessageItemArray::new 要整条数组签名（带前导 a）；Message::signal(path,iface,name) 直接返 Message（失败 panic）、Connection::send 返 Result<u32,()>；GetLayout 回执 (u(ia{sv}av)) 有单测钉死；快照父节点先 push 再递归（先序，否则 id 错位）。Tauri 2 ACL：新窗口 label 必须进 capabilities/default.json 的 windows，否则 invoke/listen 静默拒。
- 回归探针：`~/.cache/ab-dock-min/cin/probe40.sh`——假 watcher 总线级回归（不依赖真面板），classic 验 libayatana 全量菜单、unified 验自研 dbusmenu+点击派发；libayatana 不暴露 ItemIsMenu 属性，探针要容错。
- 待办：本机无 win/mac target，上 Windows 先 build 再实机点托盘复验（两形态）→ push → 打包。

## 产品定则（细节见日志）
- 默认值常量钉着（改值同步单测）：亮色/90%/remoteAllowGuiAuth=true/默认高度=整屏高 2/3（基准 monitor.size()）；window_height==0 是「没记过尺寸」哨兵。
- 落位 free/center/last：center=禁拖；锁定窗口大小只禁手拖缩放不拦落宽。
- 下次启动大字=级联全链；Rust 侧级联文案必须与前端 nextBootText/rowLabel 一字不差；托盘指纹含所有可见字段。
- 忙闸必须有放闸路径（判据不能是「命令返回成功」——systemctl suspend 排队即返回 0）；POWER_RELEASE_MS=15s 世代号放闸；被闸点击必须 record_action 留痕。
- 远程动作=本机点头（pending+授权窗）；灰掉而不藏按钮。
- UI_REVISION 起点跟时间挂钩；split_url() 先剥代理绝对 URI；/api/ui-command 回执放 result 里。

## 结构 / 窗口
- 后端 src-tauri/src/lib.rs（单文件大）+ http.rs（tiny_http）；前端 ui/（index/app.js/styles.css/grub-list-view.js/tray-menu.js）。UiState 唯一事实源。
- tiny_http Windows：stop() 必须 127.0.0.1 自连接轮询到端口真能 bind。Linux 写 ESP：access(2) 探测，写不动 pkexec install。
- 窗口定则：判位只用客户区坐标；改几何=hide→set→show；隐藏窗口视口 0x0；改尺寸后等落地再落位。GTK3：set_resizable(false) 把尺寸钉在子控件最小（WebKitWebView≈200x200），要缩到 200 以下先临时 set_resizable(true)、回锁排在几何请求之后（细则见 MEMORY-detail-windowing.md）。

## 移动端（mobile/，WebView 客户端）
- 只加载电脑端内置 HTTP 托管的 ui/，别交叉编译 Rust 核心。多电脑=每台独立 WebView，addresses 有序 + machineId 归并。
- Linux 出 APK：`~/.cache/ab-android/build.sh`（SDK ~/android-sdk，JDK 21 /home/disk2/software/jdk/jdk-21，ANDROID_USER_HOME/GRADLE_USER_HOME 显式隔离，清代理）。Windows 出包走 E:\Android。产物核对 classes2.dex。文档三份同步（ui/mobile/根 README）。

## git 推送（本机限制）
- 本机无任何 GitHub 凭据，git push 必失败。**commit 由我做，push 由用户在 IDE 做**（提醒本地领先 N 个提交即可）。远端 https://github.com/lidiaoo/anyboot.git。
