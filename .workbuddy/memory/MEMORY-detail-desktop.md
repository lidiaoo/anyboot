# AnyBoot 桌面端细节备忘：构建 / 打包 / 沙箱验证 / WebKitGTK

> 本文件是 `MEMORY.md` 的细节延伸（桌面侧）。改动桌面端前读这份。

## 构建 / 打包
- 前端改动必须强制 codegen：
  `rm -rf src-tauri/target/release/build/anyboot-*/out && touch src-tauri/build.rs && cargo build --release --manifest-path src-tauri/Cargo.toml`
  （out 在 `src-tauri/target` 下；zsh 无匹配 glob 会中断命令串，别写裸通配）。
  核对用 node `zlib.brotliDecompressSync` 解 `out/tauri-codegen-assets/*`；**二进制里搜中文要 `buf.includes(串,"utf8")`**（latin1 永远匹配不上）。
- 交付 = 测试 + release 构建 + 实机验证；`cargo test` 不产出 exe；**构建前先退出运行中的 anyboot**；产物 md5 必核（Text file busy 会静默失败）。
- cargo 位置：2026-10-14 晚起沙箱内亦有 `/usr/bin/cargo`（Arch rust，HOME=/root），构建走沙箱并 `export HOME=/home/jianghh CARGO_HOME=/home/jianghh/.cargo`；X11/dbus 探针才需要非沙箱。用户终端也能直接 `npm run build:linux`（rustup/PATH）——以实况为准。
- 打包：`TMPDIR=~/.cache/ab-tmp npm run build:linux`（deb+rpm）；`build:linux-portable` = no-bundle + `scripts/make-appimage-jammy.sh`（jammy 基座 + zig 重链 gnu.2.35，脚本开头自动重链；**ELF 改字符串必须等长替换**）。沙箱内打包仍走 jammy 脚本。
- `~/.cache/tauri/linuxdeploy-x86_64.AppImage` **必须新版**（continuous build 369+，内置 binutils 2.47 认 RELR；旧 2024 版 strip 全挂 `.relr.dyn` → 整链挂，`NO_STRIP` 无效），备份 `.bak-2024`；runtime 偶发下载挂 → `LDAI_RUNTIME_FILE=~/.cache/tauri/runtime-x86_64`。
- `ui/index.html` 的 `data-page-node-id` 属性必须原样保留；两区同构样式走共用选择器；标题字号别挂在会搬家的容器下（如 `.grub-head h2` 单列）。
- 文档里路径写 `~`，**不写死 `/home/jianghh`**。

## 沙箱验证环境
- Xvfb 三件套：①`mkdir -p /tmp/.X11-unix && chmod 1777`；②Xvfb 本体加 `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json`（只给它加）；③HOME/XDG 指可写路径。模拟用户屏 1920x1080（+`GDK_SCALE=2` 时 3840x2160）。
- 窗口行为复现台（首选）：Xvfb 对齐真机分辨率/GDK_SCALE + **真 WM（mutter；kwin_x11 乱改尺寸、xfwm4 缺 xfconfd）+ app 本尊**，配置走 `XDG_CONFIG_HOME` 副本照抄用户 settings.json；xdotool 驱动、xwininfo/xprop 读几何。样板 `~/.cache/ab-dock-min/`（已收进 skill `linux-x11-window-repro`）。
- 脚本放 `~/.cache/`（**/tmp 跨沙箱隔离**）；冒烟塞同一 Bash 前台串完；判活看「应用日志已监听」+端口（**别用 ps**）；每次验证换端口。沙箱 `http_proxy` 常开：curl 加 `--noproxy '*'`，构建/浏览器清代理。
- `xdotool search --name` 中文正则不可靠（枚举窗口逐字比较）；同进程有 20x20/32x32 辅助窗，取面积最大的；`xwininfo` 搜得到隐藏窗，判「在不在」看 Map State。
- 探针脚本两坑：①`grep -c` 无匹配输出 0 且退出码 1，`|| echo 0` 会变两行 —— 写 `n=$(grep -c …); n=${n:-0}`；②函数里 `… | while … return 0` 的 return 退出的是子壳，改 for 循环。
- 取证两通道：`trace()` 默认关；`startup_debug()` → `<config>/startup-debug.log` 常开。**用户可见动作必须走常开通道或 `record_action`**。
- 读用户运行中应用实时状态：
  `curl --noproxy '*' -H "X-Auth-Token: <settings.json httpToken>" http://127.0.0.1:<httpPort>/api/ui-state`
- **禁用 `pkill -f <路径>`**（会连自己的 shell 一起杀），按 `/proc/PID/exe` 比对。
- 冒烟库：`~/.cache/ab-verify/`、`~/.cache/ab-win/`、`~/.cache/ab-test/`。

## WebKitGTK × NVIDIA
- X11 唯一正解 `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`（`apply_linux_gpu_workarounds`，必须早于建 WebView）。
- `WEBKIT_DISABLE_DMABUF_RENDERER` / `COMPOSITING_MODE` 已失效，**勿回用**。用户设过任一 WEBKIT 环境开关就不插手。

## 端到端验证台
- KDE 侧：`~/.cache/ab-kde-repro/`（`kdemenu_client.cpp` + `probe-kde.sh`，同 libdbusmenu-qt5 的 DBusMenuImporter）。
  编译：`g++ -fPIC -std=c++17 -o kdemenu_client kdemenu_client.cpp $(pkg-config --cflags --libs dbusmenu-qt5 Qt5Core Qt5Widgets Qt5DBus)`
- 总线级回归：`~/.cache/ab-dock-min/cin/probe40.sh`（假 watcher，不依赖真面板）。
- 主窗 UI 按钮点击台：`~/.cache/ab-headerbtn/`（`probe-header2.sh` + `find-buttons.py`；Xvfb 起应用 → `import -window <id>` 截图 → 像素自校准定位 → xdotool 点击；透明窗抓 root 全黑，扫描须跳过 h1 文字区 x≥130 并限定左栏 x≤470）。

## 网页控制台（ui/）
- **styles.css 里凡是 class 设了 `display:flex/grid` 的元素，用 `hidden` 属性控制显隐必须显式补 `[hidden]{display:none}`**——作者样式永远压过 UA 的 `[hidden]` 规则。已踩两次：`.grub-empty-state`、`.token-modal-overlay`（后者症状：token 弹窗连接成功/点「暂不连接」后永远盖在屏幕上，手机端整页被锁死；JS 状态其实都对）。排查第一步：CDP 读 `getComputedStyle(el).display` 和 `el.hidden` 对不对得上。
- **PC 端网页 UI 是 `include_str!` 编译期嵌进二进制的**（lib.rs `UI_INDEX_HTML/UI_APP_JS/UI_STYLES_CSS/UI_GRUB_LIST_VIEW_JS`）——改 `ui/*` 必须 `cargo build --release` + 重启 PC 端才到浏览器/手机 WebView；只改前端源码不重编等于没改。
- 鉴权链路排查入口：`curl -H "X-Auth-Token: <token>" http://127.0.0.1:12300/api/requests` 能看每个请求的来源 IP/路径/被拒原因（token 无效）；`/api/ui-state` 成功请求不记日志，401 才记。服务端 token 在 `~/.config/com.example.anyboot/settings.json` 的 `httpToken`。
- **OpenSSH 10 的 ProxyCommand 不吃 `VAR=x cmd` 环境前缀**（2026-10-17 实测 10.6p1：会被当可执行文件名解析直接挂，报错 `<CWD>/SSH_ASKPASS=...: No such file or directory`）→ 必须 `env VAR=x cmd`。hub_term 跳板 ProxyCommand 已用 `env` 形式，字符串组装抽在 `jump_proxy_command` 纯函数（有单测）。
- **ssh 链路 E2E 靶机**：`ab_sftp_fixture.py` 已支持 direct-tcpip（`check_channel_direct_tcpip_request` 放行 + 按 `chan.remote_chanid` 对接 pump 线程）；跑两实例（--port 2323/2324 + 各自 --root），`ssh -o ProxyCommand="ssh -p 2323 abtest@127.0.0.1 -W %h:%p" -p 2324 abtest@127.0.0.1` 即验链。注意：靶机后台进程随 shell 退出，起靶机和 ssh 测试要放同一条命令里。模拟器 E2E 走不通（沙箱无 /dev/kvm）。
