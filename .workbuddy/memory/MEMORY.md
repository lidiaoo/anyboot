# AnyBoot 项目长期备忘（细则查当日日志 2026-10-10~15.md 与 MEMORY-detail-windowing.md）

## 构建 / 打包
- cargo 在**沙箱内**（/usr/bin/cargo，Arch rust 1.99）；构建走 `export HOME=/home/jianghh CARGO_HOME=/home/jianghh/.cargo`；X11/dbus 探针才需非沙箱。用户宿主机也能直跑 `npm run build:linux`。
- 前端改动必须强制 codegen：`/usr/bin/rm -rf src-tauri/target/release/build/anyboot-*/out && touch src-tauri/build.rs && cargo build --release --manifest-path src-tauri/Cargo.toml`；核对用 node `zlib.brotliDecompressSync` 解 `out/tauri-codegen-assets/*`；**二进制里搜中文要 `buf.includes(串,"utf8")`**。只改 Rust 不需要 codegen。
- 打包 `TMPDIR=~/.cache/ab-tmp npm run build:linux`（deb+rpm+appimage）。appimage 依赖 **~/.cache/tauri/linuxdeploy-x86_64.AppImage 必须新版**（continuous 369+，内置 binutils 2.47 认 RELR；旧 2024 版 strip 全挂 `.relr.dyn`，NO_STRIP 无效），备份 .bak-2024；runtime 下载挂时用 `LDAI_RUNTIME_FILE=~/.cache/tauri/runtime-x86_64`；沙箱内仍走 `build:linux-portable`。**产物 md5 必核**（Text file busy 会静默失败）。
- 交付 = 单测 + release 构建 + **载荷（deb 解包出来的二进制）复跑验证** + 用户实机验证；构建前确认没有运行中的实例占着目标二进制。
- ui/index.html 的 data-page-node-id 属性原样保留；两区同构样式走共用选择器。文档路径写 ~。

## 沙箱验证环境
- Xvfb 三件套：`mkdir -p /tmp/.X11-unix && chmod 1777`；Xvfb 本体加 `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json`（只给它）；HOME/XDG 指可写路径。脚本放 ~/.cache/（/tmp 跨沙箱隔离），冒烟塞同一 Bash 前台串完；每次验证换端口。
- 沙箱 http_proxy 常开：curl 加 `--noproxy '*'`，构建/浏览器清代理。
- `ps` 看不到宿主进程；**但能读宿主文件系统、也能直连宿主 localhost** —— 判断用户应用死活/读实时状态：`curl --noproxy '*' -H "X-Auth-Token: <settings.json httpToken>" http://127.0.0.1:<httpPort>/api/ui-state`；也能直接读用户的 `~/.config/com.example.anyboot/startup-debug.log`（真机取证第一手，最省事）。
- 探针两坑：`grep -c` 无匹配输出 0 且退出码 1（写 `n=$(grep -c …); n=${n:-0}`）；函数里 `… | while … return 0` 的 return 退的是子壳（改 for）。
- 取证两通道：trace() 默认关；startup_debug() → `<config>/startup-debug.log` 常开。**用户可见动作必须走常开通道或 record_action**。

## WebKitGTK × NVIDIA
- X11 唯一正解 `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`（apply_linux_gpu_workarounds，早于建 WebView）；WEBKIT_DISABLE_DMABUF_RENDERER / COMPOSITING_MODE 已失效勿回用。用户设过任一 WEBKIT 开关就不插手。

## 托盘（classic 原生菜单 / unified 自绘+SNI）——基线 440189c 线（9310876）
- **trayMenuStyle 启动时定接线，切换重启生效**：
  - classic：Win/mac = tauri 托盘 + muda 原生菜单；**Linux = 不建 tauri 托盘，走自研 linux_sni 全量 dbusmenu**（muda 0.20 GTK3 子菜单渲染回归绕道；备选方案在 backup/old-stack-0d43626，别混）。绿勾位图 check_mark_*、menu_signature 链是 classic 依赖，别删。
  - unified：Win/mac 右键直开自绘窗；Linux 自研 SNI —— **GNOME 壳菜单只 [显示主窗口, 退出程序] 两条**（非空弹窗才发 opened），opened→弹自绘窗 +150ms 合成 Esc 释 grab；Cinnamon/KDE 全量 dbusmenu 由宿主渲染。
- **宿主客户端三家矩阵（决定一切「点不动」类问题，2026-10-15 实测）**：GNOME appindicators 扩展走 GIO、点条目发 `Event`、按**信号名**匹配；KDE libdbusmenu-qt5 发 `Event`、按**签名精确匹配**；**Cinnamon/MATE/indicator 系（libdbusmenu-gtk3←glib，桥梁 xapp-sn-watcher）点条目发 `EventGroup(a(isvu))`**。两条通道都必须实现 —— 老实现只认 Event，导致 Cinnamon 上「菜单能弹、点任何条目毫无反应」；写探针时更要记得探针几乎都用 Event，天然测不到 EventGroup。
- Linux SNI 定则：ItemIsMenu=false + Menu=object path（类型错 GNOME 静默丢属性）。GNOME 扩展只认 dbusmenu；Cinnamon/KDE 见有效 Menu 会自建宿主菜单消费右键（ContextMenu 回调不来）。RegisterStatusNotifierItem 参数以 / 开头时服务名=sender 唯一名。
- **dbusmenu 导出禁带 toggle-type/toggle-state（17baf39）**：宿主对 checkable 条目先本地翻转再通知、协议又没有单选组 → 点几个勾几个。勾选标记一律 radio_label 画进 label 文字（◉/○）。
- **dbusmenu 信号定则（cea8908+42b9539+3a39e22）**：信号必须从 **Item.Menu 指向的路径**（MENU_PATH）发（从 ITEM_PATH 发被所有宿主静默丢）。回显走 **ItemsPropertiesUpdated 属性差分**：只属性变→只推属性；只结构变→只发 LayoutUpdated；**都没变→什么都不发**。**信号体签名必须 `a(ia{sv})a(ias)`**（removedProps 是 a(ias)；写成 a(iias) 被 Qt 静默丢，GNOME 按名字匹配所以不报错）。
- **AboutToShow = 真脏标记**（MENU_DIRTY 只在 GetLayout 应答清零；恒 true 会把子菜单展开瞬间打回）；LayoutUpdated 防重发靠 MENU_SIGNAL_REVISION。
- **自绘窗落位（9310876）**：unified 优先贴托盘图标**正下方**（水平居中），放不下翻正上方，绝不盖图标；classic 上方优先+左对齐。图标矩形用标称 32 物理px。**TrayCenter 已弃用**，positioner 只留喂点。**失焦看门狗** 250ms 轮询：拿到过焦点又稳定 400ms 无→收；从未拿到→宽限后重试 set_focus、4s 仍无→收。
- **GRUB 区（9310876）**：grub_menu_nodes 同一份喂 unified 自绘窗与 Linux classic dbusmenu，改它两头都变。keepsOpen 只在后端算，分发统一走 handle_menu_event。
- dbus 0.9 坑：MessageItemArray::new 要**整条数组签名**（带前导 a，否则构造即 panic、SNI 线程当场死）；Message::signal 直接返 Message（失败 panic）、Connection::send 返 Result<u32,()>；快照父节点先 push 再递归（先序，否则 id 错位）。Tauri 2 ACL：新窗口 label 必须进 capabilities/default.json 的 windows，否则 invoke/listen 静默拒。
- 复现台（都在 ~/.cache/，程序+脚本副本已进对应技能 assets）：
  - `ab-cin-repro/` probe-cin.sh + cinmenu_client.c —— **Cinnamon 侧真 libdbusmenu-gtk3**（`dbusmenu_gtkmenu_new`，与 xapp-sn-watcher 同款调用）+ 私有总线/Xvfb/应用本尊/dbus-monitor，末尾同时回归单条 `Event`。「点不动」类改动必跑（技能 sni-dbusmenu-debug）。
  - `ab-kde-repro/` probe-kde.sh + kdemenu_client.cpp（真 libdbusmenu-qt5）。回显/签名类改动必跑。
  - `ab-headerbtn/` 主窗口标题栏按钮自校准点击台；`ab-dock-min/` 窗口行为/贴边台（+cin/probe40.sh）；假宿主探针 probe27-dbusmenu.py、probe-sni-click.py。
- 待办：本机无 win/mac target，上 Windows 先 build 再实机点托盘复验（两形态）→ push → 打包。

## 产品定则（细节见日志）
- 默认值常量钉着（改值同步单测）：亮色/90%/remoteAllowGuiAuth=true/默认高度=整屏高 2/3（基准 monitor.size()）；window_height==0 是「没记过尺寸」哨兵。
- 落位 free/center/last：center=禁拖；锁定窗口大小只禁手拖缩放不拦落宽。
- 下次启动大字=级联全链；Rust 侧级联文案必须与前端 nextBootText/rowLabel 一字不差；托盘指纹含所有可见字段。
- 忙闸必须有放闸路径（判据不能是「命令返回成功」——systemctl suspend 排队即返回 0）；POWER_RELEASE_MS=15s 世代号放闸；被闸点击必须 record_action 留痕。
- 远程动作=本机点头（pending+授权窗）；灰掉而不藏按钮。
- 主窗口顶部标题栏（9bcf278）= 电源三键 + **重启程序 / 退出程序**（系统动作在前、程序动作在后）。走 tauri IPC；`/api/ui-command` 归 gui_only（400）且网页端隐藏按钮。**restart_app 必须先 stop_http_server 再 app.restart()**（tauri 的 restart = 先 spawn 再 exit(0)，不放开端口新实例第一屏就是「端口被另一个实例占用」）；AppImage 下 tauri 靠 APPIMAGE 指回包本身。两栏布局下 5 个按钮在 460px 左栏会换行（一行 4 + 「退出程序」单占一行），单栏/加宽一行放得下。
- UI_REVISION 起点跟时间挂钩；split_url() 先剥代理绝对 URI；/api/ui-command 回执放 result 里。

## 结构 / 窗口
- 后端 src-tauri/src/lib.rs（单文件大）+ http.rs（tiny_http）；前端 ui/（index/app.js/styles.css/grub-list-view.js/tray-menu.js）。UiState 唯一事实源。
- tiny_http Windows：stop() 必须 127.0.0.1 自连接轮询到端口真能 bind。Linux 写 ESP：access(2) 探测，写不动 pkexec install。
- 窗口定则：判位只用客户区坐标；改几何=hide→set→show；隐藏窗口视口 0x0；改尺寸后等落地再落位。GTK3 `set_resizable(false)` 会把尺寸钉在子控件最小（≈200x200），要更小得先临时解锁、回锁排在几何请求之后（细则见 MEMORY-detail-windowing.md）。

## 移动端（mobile/，WebView 客户端）
- 只加载电脑端内置 HTTP 托管的 ui/，别交叉编译 Rust 核心。多电脑=每台独立 WebView，addresses 有序 + machineId 归并。
- Linux 出 APK：`~/.cache/ab-android/build.sh`（SDK ~/android-sdk，JDK 21 /home/disk2/software/jdk/jdk-21，ANDROID_USER_HOME/GRADLE_USER_HOME 显式隔离，清代理）。Windows 出包走 E:\Android。产物核对 classes2.dex。文档三份同步。

## git 推送（本机限制）
- 本机无任何 GitHub 凭据，git push 必失败。**commit 由我做，push 由用户在 IDE 做**（提醒本地领先 N 个提交即可）。远端 https://github.com/lidiaoo/anyboot.git
