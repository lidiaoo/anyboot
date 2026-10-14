# AnyBoot 项目长期备忘（细则查当日日志 2026-10-10~13.md 与 MEMORY-detail-windowing.md）

## 构建 / 打包（照抄这串）
- `export HOME=/home/jianghh CARGO_HOME=/home/jianghh/.cargo`（$HOME=/root 不可写）；**cargo 必须非沙箱跑**。
- 前端改动必须强制 codegen：`/usr/bin/rm -rf src-tauri/target/release/build/anyboot-*/out && touch src-tauri/build.rs && cargo build --release --manifest-path src-tauri/Cargo.toml`（out 在 **src-tauri/target** 下，项目根没有 target/；zsh glob 无匹配会中断命令串）。核对：node `zlib.brotliDecompressSync` 解包 `out/tauri-codegen-assets/*` grep；**在二进制里搜中文要 `buf.includes(串,"utf8")`**，latin1 解码后永远匹配不上（ASCII 不受影响）。
- 交付 = 测试 + `cargo build --release` + 实机验证；`cargo test` 不产出 exe；构建前退出运行中的 anyboot。
- **打包命令（2026-10-14 改道）**：`TMPDIR=~/.cache/ab-tmp npm run build:linux` = deb+rpm；`... npm run build:linux-portable` = `tauri build --no-bundle` + jammy 脚本（**appimage 已从 tauri 链路移除**：本容器无 /dev/fuse 且无 CAP_MKNOD、userns 下 chown 被拒 → linuxdeploy 必挂，救不了；/tmp 是 10M tmpfs，打包临时文件必换 TMPDIR，mkdir 先建；**文档/README 路径一律写 ~，不写死 /home/jianghh**）。jammy 脚本素材来自 `~/.cache/ab-appimage` 缓存的 orig-backup，不需要现打原版 AppImage。
- Linux 全兼容包：`npm run build:linux-portable` → `scripts/make-appimage-jammy.sh`（jammy 基座 + zig 重链 x86_64-gnu.2.35；cargo 会覆盖 zig 产物，脚本开头自动重链；ELF 内改字符串必须**等长替换**+orig_size 断言）。产物 md5 必核（Text file busy 会静默打包失败、旧包冒充；脚本有三道防线）。
- `ui/index.html` 含设计工具注入的 **data-page-node-id 属性——编辑必须原样保留**，删改前先核对。
- 两区同构的样式规则（.grub/.refind 背板、两区 h2）必须走**共用选择器**，别只挂单一容器（踩过：删 .systems-head 后 rEFInd h2 回落浏览器默认 1.5em）。

## 沙箱验证环境
- Xvfb 三件套缺一即败：① `mkdir -p /tmp/.X11-unix && chmod 1777`；② **Xvfb 本体**加 `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json`（只给它加，别 export）；③ `HOME=/home/jianghh` + XDG 三目录指可写路径。模拟用户屏：1920x1080。
- ~~WebKit 渲染在沙箱必崩~~ → **先跑一次再下结论**：2026-10-14 用 Xvfb(3840x2160)+`mutter`+`GDK_SCALE=2`+`__EGL_VENDOR_LIBRARY_FILENAMES` 跑**真机 app 本尊**，页面正常加载、布局/JS 全活（白条取证值就是页面自己报的）。MiniBrowser/navigate 那轮必崩，怀疑与 GL 路径有关。
- **窗口行为复现台（强烈推荐首选）**：Xvfb 分辨率与 `GDK_SCALE` 对齐真机 + 真 WM（`mutter` 可用；`kwin_x11` 会乱改尺寸、`xfwm4` 缺 xfconfd）+ **app 本尊**，配置走 `XDG_CONFIG_HOME` 副本**照抄用户 settings.json**（设置参数化的 bug 裸探针会全绿）；`xdotool` 摆位/挪光标驱动贴边，`xwininfo -id` 读实际几何，**`xprop -id` 读 WM_NORMAL_HINTS**（本轮实测可用，别再当空壳）。样板 `~/.cache/ab-dock-min/`（probe*.py、run-probe*.sh、run-app*.sh），已收进 skill `linux-x11-window-repro`。
- WebKitWebDriver：**只认 `-p <port>`**（`--port 4444` 打 Usage）；**MiniBrowser 继承 http_proxy** → 先 unset 全部代理变量；4444 被代理占，换高位端口。脚手架：`~/.cache/ab-ui-dom/wd_ui_batch.py`（UI 批次 14 断言）、`ab-verify/wd_defaults.py`/`wd_pending.py`；`execute/sync` 的 script 要 `return (()=>{...})()`。agent-browser + Chrome 在 `~/.cache/ab-browser`（open+eval 必须同一次 Bash 调用；eval 读不到 let）。
- 脚本放 `~/.cache/`（/tmp 跨沙箱隔离）；`rm -rf` 用 `/usr/bin/rm`；冒烟塞同一个 Bash 调用前台串完，落盘 `echo >>file`；后台进程用 run_in_background 托管、收摊 TaskStop；判活看「应用日志已监听」+端口，**别用 ps/单测端口**（旧实例会骗过）；每次验证换端口。
- 沙箱 http_proxy 常开：curl 加 `--noproxy '*'`；构建/浏览器全要清代理。
- `xdotool search --name` 对中文正则不可靠（先枚举窗口再 shell 逐字比较）；**同进程常有 20x20 菜单锚窗 / 32x32 辅助窗，取面积最大的那个才是主窗**；点击用 `getwindowgeometry --shell` 算偏移，别写绝对坐标。
- 取证两通道：`trace()` → trace.log（**默认关**）、`startup_debug()` → `<config>/startup-debug.log`（常开；落位/贴边只认它）。**用户可见动作的失败/被忽略必须走常开通道或 `record_action`**，只写 trace 等于没有（踩过：托盘电源项点击被忙闸吞掉，现场一个字节都没有）。
- **无头环境也能「点」托盘菜单**（SKILL 里记着的盲区已补齐）：假扮 Cinnamon 面板的 `org.kde.StatusNotifierWatcher`（dbus-python，`RegisterStatusNotifierItem` 的参数以 `/` 开头时服务名=**sender 唯一名**）+ SNI `Menu` 属性 → `com.canonical.dbusmenu.GetLayout(0,-1,[])`（回执 `(revision, layout)` 两层）→ `Event(id,"clicked","",ts)`。脚本 `~/.cache/ab-dock-min/probe-sni-click.py` + `run-app-dbusclick.sh` / `run-app-latchcheck.sh`（假 `systemctl` exit 0 伪造「成功但没关机」）。要点：libdbusmenu 的 `Event` **不检查 enabled**（置灰项照样送达应用）。
- **读用户运行中应用的实时状态**（排查「用户报的现象」最快的一手）：`curl --noproxy '*' -H "X-Auth-Token: <settings.json 里的 httpToken>" http://127.0.0.1:<httpPort>/api/ui-state`（只读）→ 直接看 `busy` / `lastAction` / `status`。用户机器上的 AppImage 与沙箱共享网络名空间，所以能连（应用日志/挂在 /tmp 的 AppDir 看不到）。
- 冒烟脚本库：`~/.cache/ab-verify/`（run/real/auth/exec/resize/localclickswitch/explore/recall）、`ab-win/windows.sh`（三窗截图）、`ab-test/`（ws-live/ws-probe）。

## WebKitGTK × NVIDIA（Linux 白屏/局部重绘）
- X11 唯一正解 `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`（`apply_linux_gpu_workarounds()`，必须早于建 WebView）；`WEBKIT_DISABLE_DMABUF_RENDERER`（2.52+）与 `WEBKIT_DISABLE_COMPOSITING_MODE`（2.54+）已失效**勿回用**（单测钉死）。用户设过任一 WEBKIT_ENV_SWITCHES（含=0）就完全不插手（也是 A/B 开关）。NVIDIA 真机效果待用户确认。

## 托盘菜单三端自绘（③，2026-10-14 修通 Linux 全链，提交 6b7ef6c）
- 托盘菜单不再交给宿主渲染：`tray-menu` 无边框置顶 webview 窗渲染 `build_menu_nodes` 纯数据；「点完留不留菜单」只在后端算一次 = `MenuNode.keepsOpen`（`menu_choice_item`/`menu_marked_choice_item`=true），前端照字段办事，**不许两处重算**。分发仍走 `handle_menu_event`。
- **dbus 0.9 坑**：`MessageItemArray::new` 要**整条数组签名（带前导 a）**——内层 `"ay"`、外层 `"a(iiay)"`；传元素签名 = `Err(InvalidSignature)`，且崩在**宿主读属性时**（handler 线程死，症状「注册成功但没图标」）。已有单测钉死。
- **Tauri 2 ACL 坑**：新建窗口的 label **必须加进 `capabilities/default.json` 的 `windows`**，否则该窗口所有 invoke/listen 被静默拒（症状「页面加载了、前端就是不调命令」）。
- Linux 托盘图标 = 自写 SNI（`linux_sni` 模块，`dbus::ffidisp` 手工 MsgHandler；`ItemIsMenu=false`+`Menu=""` ⇒ 宿主把点击回调给我们）。断链取证看 `tray-menu` trace：窗已建→页面加载→前端拉内容→报高→呈现。
- **点击语义三端统一**：左键=显示主窗口（Recall），右键=自绘菜单（Linux 中键也是菜单）。原生菜单三端全下线（绿勾位图、菜单指纹、menu-anchor 锚点窗已删，净 -717 行，提交 575e63e）。
- 待办：**本机无 win/mac target**，`cfg(not(linux))` 只过了 Linux 编译+单测 —— 上 Windows 先 build 再实机点托盘复验 → push → 打包。

## 产品定则（细节见日志）
- 默认值常量钉着（改值必须同步单测）：亮色 / 90% / remoteAllowGuiAuth=true / **默认高度=整屏高的 2/3**（1080→720；基准是 `monitor.size()` 不是工作区——工作区基准曾让重置按钮"点了没反应"，2026-10-13）；`window_height==0` 是「没记过尺寸」哨兵，别当尺寸用。
- 落位方式 free/center/last：**center=位置固定禁拖**（拖动弹回居中+提示换方式，节流 4s；回声/WM 抖动过滤见 CENTER_ECHO_* 静态量）。**锁定窗口大小只禁手拖缩放，不拦展示方式落宽**（`grub_layout_target_width` 无 locked 门槛，用户定案 2026-10-13）。
- 下次启动大字=级联全链（rEFInd›GRUB 组名›条目，条目文字与 grub.listFormat 同规则）；状态行「下次启动：」前缀一律去重。两区（rEFInd/GRUB）卡片背板与标题行共用 CSS 规则；**标题字号规则绝不能挂在会搬家的容器下**——`.grub-head` 搬进右栏后脱离 `.grub`（坑过两次），已单列 `.grub-head h2`。③④ 右栏 `.pane-body` 本身就是卡片（同款边框/底色/圆角，24px padding↔margin 挪位）；pane 模式 `.grub-head` 由 applyGrubLayout **搬出滚动容器**钉在卡片上方常驻（**sticky 对 grid/flex 子项受包含块约束，滚出自身轨道就失效——「钉住」需求直接搬出滚动容器**）。
- 远程动作=本机点头：`switch_system` 等走 `ui_command_requires_local_auth` → pending+授权窗；「立即执行」仅本机（远程 403）；灰掉而不藏按钮。
- **托盘顶部「下次启动」= 主窗口那行大字**（两行排版 `下次启动\nLinux › 组名 › 条目`），级联规则由 Rust 侧 `next_boot_cascade`/`grub_entry_label` 重建，**必须与前端 `ui/app.js` 的 `nextBootText`、`ui/grub-list-view.js` 的 `rowLabel` 一字不差**；`menu_signature_for` 里凡是「托盘上看得见的字段」都必须进指纹（含 `entry.name`/`grubgroups`/`grublf`/级联文案 `nb`）。
- **忙闸（busy）类状态必须有放闸路径，判据不能是「命令返回成功」**：`systemctl suspend` 只是排队就返回 0，被 inhibitor 拦下/内核 suspend 失败立刻 resume 都照样「成功」—— 曾因此让 `busy.power` 永久挂住、三个电源项永远置灰且点击被静默吞掉（用户报「三个电源按钮都没用」）。现在 `POWER_RELEASE_MS=15s` 世代号延时放闸（`schedule_power_release`）；新动作推高世代号作废旧放闸。被闸挡掉的点击必须 `record_action` 留痕，`handle_menu_event` 每次点击无条件写一行 `菜单点击 id=…` 到 startup-debug.log。
- `UI_REVISION` 起点跟时间挂钩（`ui_revision_seed`）；凡「按版本号丢过期数据」先问跨进程单调性。
- `split_url()` 先剥代理绝对 URI（否则网页整片空白）；静态路由要覆盖「自然写法」并钉单测；`/api/ui-command` 回执必须放 `result` 里。
- 验「断线自愈」：断线期间改宿主状态，重连见新值才算真自愈。

## 结构 / 权限 / 窗口（窗口细则见 MEMORY-detail-windowing.md）
- 后端 `src-tauri/src/lib.rs`（单文件大）+ `http.rs`（tiny_http 0.12）；前端 `ui/`：index/app.js/styles.css + grub-list-view.js（主窗/GRUB 共用）。`UiState` 唯一事实源，明文 token 不进快照。
- tiny_http Windows 致命：`RunningServer::stop()` 必须 127.0.0.1 自连接并轮询到端口真能 bind。
- Linux 写 ESP：先 access(2)，写不动 `pkexec install -p -m 0644`；远程请求圈 `RemoteNoGuiAuth::enter`。
- 窗口定则：判位只用客户区坐标；改几何=hide→set→show；GTK 带装饰 set_position 是窗框原点；隐藏窗口 WebView 视口 0x0；改尺寸后必须等新尺寸落地再重落位。
- **GTK3 定则（必守）**：`set_resizable(false)`（= 「锁定窗口大小」）会把窗口尺寸**钉在子控件最小尺寸**上（WebKitWebView ≈**200x200 逻辑px**），之后 `set_size` 被夹进 `[子控件最小, 几何提示 max]`。任何要缩到 200px 以下的路径（贴边细缝、条形提示）必须先临时 `set_resizable(true)`，落定后再回锁；**回锁必须排在几何请求之后**，否则把尺寸夹回去（全链见 MEMORY-detail-windowing.md「回合 47」）。

## 移动端（mobile/，WebView 客户端，非原生移植）
- 只加载电脑端内置 HTTP 服务托管的 `ui/`，`/api/*` 同源；**别交叉编译 Rust 核心**（无 NDK）。
- 多电脑=每台一个独立 WebView；一条记录=一台电脑（`addresses` 有序列表 + `/health` 的 machineId 归并，promoteAddress 后 Tab.addressIndex 归零）；传输方式由 App 拼 `?transport=`，不立刻重载。
- **Linux 本机出 APK**：`/home/jianghh/.cache/ab-android/build.sh`（SDK `~/android-sdk`；JDK 21 `/home/disk2/software/jdk/jdk-21`；`ANDROID_USER_HOME`/`GRADLE_USER_HOME` 必须显式隔离；清代理；无 /dev/kvm 模拟器只能 TCG）。Windows 出包走 `E:\Android`（JAVA_HOME+gradle-8.11.1）。产物核对：`classes2.dex` 才是代码（`strings -a -e S` 查新文案）。
- 文档三份同步：`ui/README.md`、`mobile/README.md`(+子目录)、根 README 客户端小节；APK/构建产物不入库。

## git 推送（本机限制）
- 本机**没有任何 GitHub 凭据**：无 `credential.helper`、无 `~/.git-credentials`/`.netrc`、无 `~/.ssh` 密钥、无 `gh` CLI，环境里也没有 GH_TOKEN。`git push` 只会得到 `could not read Username for 'https://github.com'`（沙箱里则是直接卡死）。
- 所以**提交（commit）由我做，推送（push）由用户在 IDE 里做**——别反复重试 push 浪费时间；提醒一句「本地领先 origin/main N 个提交，得空 push」即可。
- 远端：`https://github.com/lidiaoo/anyboot.git`。
