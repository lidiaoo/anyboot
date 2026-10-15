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

## 托盘（双形态：classic 原生菜单 / unified 自绘+SNI）——当前基线=440189c 线（9310876）
- **trayMenuStyle 二选一，启动时定接线，切换重启生效**：
  - classic（默认）= Win/mac：tauri 托盘 + muda 原生菜单（Windows 摘菜单+锚点窗 popup_tray_menu 现建现弹）；**Linux：不建 tauri 托盘，走自研 linux_sni 全量 dbusmenu**（8c96da4 起，muda 0.20 GTK3 子菜单渲染回归绕道；注意与 backup/old-stack-0d43626 分支的「GNOME 回 libayatana」方案相反——那是备选方案，main 走 linux_sni）。绿勾位图 check_mark_*、菜单指纹 menu_signature 链是 classic 依赖，别再删。
  - unified = Win/mac 右键直开自绘窗（tray-menu）；Linux 自研 SNI 导出 dbusmenu——**GNOME：壳菜单只有[显示主窗口, 退出程序]两条**（非空弹窗才发 opened），opened→弹自绘窗+150ms 合成 Esc 释 grab；Cinnamon/KDE：全量 dbusmenu 宿主渲染。**AboutToShow=真脏标记（MENU_DIRTY 只在 GetLayout 应答清零，440189c）**——恒 true 会把展开瞬间打回；LayoutUpdated 防重发靠 MENU_SIGNAL_REVISION。
  - **自绘窗落位（9310876 定案）**：unified 优先贴托盘图标**正下方**（水平对图标居中），放不下翻**正上方**，绝不盖图标；classic 上方优先+左对齐。图标矩形用标称 32 物理px（TRAY_MENU_ICON_NOMINAL）。**TrayCenter 已弃用**（固定往上摆、Linux 不翻面）；positioner 插件与喂点保留但落位全手算。
  - **失焦看门狗（9310876）**：收起主路径 Focused(false) 有两漏点（假失焦被宽限吞掉后焦点已丢不再来事件；set_focus 偶发失败从未有焦点）→ spawn_tray_menu_watchdog 250ms 轮询：拿到过焦点又稳定 400ms 无→收；从未拿到→宽限后重试 set_focus、4s 仍无→收（GNOME 点击必聚焦，点在菜单上必有焦点）。runtime 焦点账本 open/close 清零。
  - **GRUB 区（9310876 + c522443）**：grub_menu_nodes 对齐主窗列表逻辑（置顶当前项+原位保留、组头▸+系统图标+N 项[+ESP 位置]、条目文字随 grub.listFormat、非原始格式按渲染结果折叠「N 个变体」）——**同一份函数喂三处：主窗、unified 自绘窗、Linux classic dbusmenu，以及 Win/mac classic 原生菜单**（c522443 起 build_tray_menu 只做「节点→原生控件」翻译，不再有第二份实现）。改它三头都变。行文字顺序 = **箭头在前、图标在后**（↳ 🐧 linux），与主窗 makeRow 逐字对齐；`menu_signature_for` 的 GRUB 段直接用 grub_menu_nodes 的 JSON，别再自己数。
  - **macOS 原生对话框不可用（c522443 实测）**：rfd 无 parent 时走 `CFUserNotificationDisplayAlert`，本机只登记不显示（UserNotificationCenter 多一个 4×3/alpha=0 空窗，调用方阻塞 ~30s 才返回）。要弹提示走 `osascript -e 'display dialog … buttons {"好"} default button 1 with icon note giving up after 60'`（实测 420×132 可见、无权限弹框），与电源动作同源。
- keepsOpen 只在后端算，前端照字段办事；分发统一走 handle_menu_event。
- positioner：喂点保留（on_tray_event），仅作插件状态维护；落位不再走插件。capabilities 加 positioner:default。
- Linux SNI 定则：ItemIsMenu=false + Menu=object path（类型错 GNOME 静默丢属性）。GNOME 扩展点击只认 dbusmenu；Cinnamon/KDE 见有效 Menu 会自建宿主菜单消费右键（ContextMenu 回调不来了）。
- **dbusmenu 导出禁带原生勾选（17baf39）**：toggle-type/toggle-state 一律不下发——宿主（KDE/Qt）对 checkable 条目先本地翻转再通知，协议又没有单选组概念 → 点几个勾几个（KDE 实测「单选点不动/GRUB 变多选」）。勾选标记一律 radio_label 画进 label 文字（◉/○），与 8285 行定则同源；快照单测钉死 label 断言 + 无 toggle-*。
- **dbusmenu 信号定则（cea8908+42b9539+3a39e22）**：信号必须从 **Item.Menu 指向的对象路径**（MENU_PATH）发——从 ITEM_PATH 发会被所有宿主按路径匹配静默丢弃。回显走 **ItemsPropertiesUpdated 属性差分推送**（menu_property_diff 回 (结构变?, 属性更新)，按结果分流）：只属性变→只推属性；只结构变→只发 LayoutUpdated；**都没变→什么都不发**（invalidate_menu_signature 每次点击强制换代，无差别广播 = 更新风暴）。**信号体签名必须是 `a(ia{sv})a(ias)`**——removedProps 是 `a(ias)`（规范/Chromium/libdbusmenu-qt/Go systray 一致），写成 a(iias) 会被 Qt 宿主按签名精确匹配丢弃（GNOME 走 GDBus 按名字匹配所以不报错）。勾选标记画进 label 文字、不导出 toggle-type/toggle-state。
- **KDE 侧复现台（3a39e22）**：`~/.cache/ab-kde-repro/`——kdemenu_client.cpp（KDE 面板同一个 libdbusmenu-qt5 的 DBusMenuImporter：updateMenu→dump→QAction::trigger→再 dump，查就地回显）+ probe-kde.sh（TAG/STYLE/APP 同 probe60）。编译 `g++ -fPIC -std=c++17 -o kdemenu_client kdemenu_client.cpp $(pkg-config --cflags --libs dbusmenu-qt5 Qt5Core Qt5Widgets Qt5DBus)`。回显类改动必须过这台（probe60 的 python 客户端是名字匹配，抓不到签名类问题）。
- dbus 0.9 坑：MessageItemArray::new 要整条数组签名（带前导 a）；Message::signal(path,iface,name) 直接返 Message（失败 panic）、Connection::send 返 Result<u32,()>；GetLayout 回执 (u(ia{sv}av)) 有单测钉死；快照父节点先 push 再递归（先序，否则 id 错位）。Tauri 2 ACL：新窗口 label 必须进 capabilities/default.json 的 windows，否则 invoke/listen 静默拒。
- 回归探针：`~/.cache/ab-dock-min/cin/probe40.sh`——假 watcher 总线级回归（不依赖真面板），classic 验 libayatana 全量菜单、unified 验自研 dbusmenu+点击派发；libayatana 不暴露 ItemIsMenu 属性，探针要容错。
- **主窗口 UI 按钮点击台（9bcf278）**：`~/.cache/ab-headerbtn/`（probe-header2.sh + find-buttons.py）——Xvfb 起应用本尊 → `import -window <id>` 截图（透明窗抓 root 全黑）→ 像素自校准定位按钮 → xdotool mousemove+click（无 WM 也能点）→ 用应用自己的 startup-debug/trace 判定。扫描须跳过 h1 文字区（x≥130）并限定左栏（x≤470）；判活/清理按 /proc/PID/exe 比对，**禁用 pkill -f <路径>**（会连自己的 shell 一起杀）。
- 待办：本机无 win/mac target，上 Windows 先 build 再实机点托盘复验（两形态）→ push → 打包。

## 产品定则（细节见日志）
- 默认值常量钉着（改值同步单测）：亮色/90%/remoteAllowGuiAuth=true/默认高度=整屏高 2/3（基准 monitor.size()）；window_height==0 是「没记过尺寸」哨兵。
- 落位 free/center/last：center=禁拖；锁定窗口大小只禁手拖缩放不拦落宽。
- 下次启动大字=级联全链；Rust 侧级联文案必须与前端 nextBootText/rowLabel 一字不差；托盘指纹含所有可见字段。
- 忙闸必须有放闸路径（判据不能是「命令返回成功」——systemctl suspend 排队即返回 0）；POWER_RELEASE_MS=15s 世代号放闸；被闸点击必须 record_action 留痕。
- 远程动作=本机点头（pending+授权窗）；灰掉而不藏按钮。
- 主窗口顶部标题栏（9bcf278）= 电源三键 + **重启程序 / 退出程序**（系统动作在前、程序动作在后）。两命令走 tauri IPC；`/api/ui-command` 里归 gui_only（400）且网页端隐藏按钮。**restart_app 必须先 stop_http_server 再 app.restart()**（tauri 的 restart 是先 spawn 再 exit(0)，不放开端口新实例第一屏就是「端口被另一个实例占用」）；AppImage 下 tauri 靠 APPIMAGE 指回包本身。两栏布局下 5 个按钮在 460px 左栏会换行（一行 4 + 「退出程序」单占一行），单栏/加宽一行放得下。
- UI_REVISION 起点跟时间挂钩；split_url() 先剥代理绝对 URI；/api/ui-command 回执放 result 里。

## 单实例（c522443）
- 配置目录下的 `anyboot.lock`，`File::try_lock`（内核锁，进程死/被杀自动放；同进程第二个 fd 也 WouldBlock，故可单测）。守卫放 `setup` 最前面，拿不到锁就**只提示、后面一行都不做**（不起托盘/不监听/不开窗/不写配置）。
- **restart_app 的顺序：stop_http_server → release_instance_lock → app.restart()**（少放锁 = 点重启程序直接没了）。
- 提示：macOS 走 osascript `display dialog`（见上一条 macOS 对话框定则），其余两端走对话框插件（删 DialogExt 里 MessageDialogKind 时要按 cfg 导，macOS 上会变 unused）。`ANYBOOT_CONFIG_DIR` 可跑两套。

## 结构 / 窗口
- 后端 src-tauri/src/lib.rs（单文件大）+ http.rs（tiny_http）；前端 ui/（index/app.js/styles.css/grub-list-view.js/tray-menu.js）。UiState 唯一事实源。
- tiny_http Windows：stop() 必须 127.0.0.1 自连接轮询到端口真能 bind。Linux 写 ESP：access(2) 探测，写不动 pkexec install。
- 窗口定则：判位只用客户区坐标；改几何=hide→set→show；隐藏窗口视口 0x0；改尺寸后等落地再落位。GTK3：set_resizable(false) 把尺寸钉在子控件最小（WebKitWebView≈200x200），要缩到 200 以下先临时 set_resizable(true)、回锁排在几何请求之后（细则见 MEMORY-detail-windowing.md）。

## 移动端（mobile/，WebView 客户端）
- 只加载电脑端内置 HTTP 托管的 ui/，别交叉编译 Rust 核心。多电脑=每台独立 WebView，addresses 有序 + machineId 归并。
- Linux 出 APK：`~/.cache/ab-android/build.sh`（SDK ~/android-sdk，JDK 21 /home/disk2/software/jdk/jdk-21，ANDROID_USER_HOME/GRADLE_USER_HOME 显式隔离，清代理）。Windows 出包走 E:\Android。产物核对 classes2.dex。文档三份同步（ui/mobile/根 README）。

## git 推送（本机限制）
- 本机无任何 GitHub 凭据，git push 必失败。**commit 由我做，push 由用户在 IDE 做**（提醒本地领先 N 个提交即可）。远端 https://github.com/lidiaoo/anyboot.git。
