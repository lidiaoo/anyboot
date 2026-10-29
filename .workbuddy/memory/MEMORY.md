# AnyBoot 项目长期备忘

**这里只放铁律和索引，细则一律进 detail 文件 / 当日日志**（本文件超 15KB 会被注入截断，别再往回堆长段）：
桌面打包与 Linux 验证 → `MEMORY-detail-desktop.md`；托盘 → `MEMORY-detail-tray.md`；窗口行为 → `MEMORY-detail-windowing.md`；移动端 → `MEMORY-detail-mobile.md`；SOL 面板 → `MEMORY-detail-sol.md`；Tailscale/cloudflared/tsnet → `MEMORY-detail-tailscale.md`。

## git
- **commit 我做、push 用户做**；只提交本轮相关文件。`hub*.rs`、`ui/hub.html`、`docs/client-ui/`、`debx/` 属 PC 端 server-hub 线，别混进移动端提交。
- `.workbuddy/`：`memory/` + `probes/` 入库；`artifacts/` 已 gitignore（旧二进制备份放这里）。
- **Rust 改动提交前必须 `cargo check`**。子模块不继承父模块作用域，crate 根私有 item 要 `use crate::XXX;` 引入。
- **`cfg(target_os)` 分支必须逐平台 check**：在 mac 上编不到 linux/windows 分支（027f1fc 就这么坏的 —— `channels/elevate.rs` 的 linux 分支少了 `command`/`Stdio` 导入、`linux_grub.rs` 调了 mac-only 的 `run_privileged`，`cargo build` 直接 E0425）。Linux 侧做法：rsync 到 `/home/disk1/anyboot-check`，跑 `/usr/bin/cargo check --lib` **再加 `cargo test --lib`**（只在 windows 成立的探针测试没设 cfg 会让 Linux 上 test 编译失败，而 `--release` build 看不见）。
- 改 server-hub 线要跑 Windows 目标 check（Linux 过 ≠ Win 过）。
- 沙箱里 HOME 读不到 global git config → `git -c user.name=jianghuihua -c user.email=2503338074@qq.com`，提交后 `git log --oneline -3` 核对（偶发一次写入双提交）。

## 交付铁律
- **⛔ 永不跑 `connectedDebugAndroidTest` / 任何 `connected*` gradle 任务**（AGP 前置 `uninstallDebug` 会卸载用户手机上的 app，SharedPreferences 档案全部丢失，2026-10-26 已造成一次真实数据事故）。instrumented test 只走手动流程：`assembleDebug assembleDebugAndroidTest` → `adb install -t -r` 两包 → `adb shell am instrument -w`（realme 真机上 gradle 直跑 instrumentation 会 "failed to attach"，手动跑正常）。
- 交付 = 测试 + release 构建 + 实机验证；构建前先退出运行中的 anyboot，产物 md5 必核。
- 前端编译期嵌入二进制：改完必须强制 codegen 重编，并用 brotli 解 `tauri-codegen-assets/*` 搜新函数名确认真的进去了（别靠 `strings` 搜中文）。
- 忙闸必须有放闸路径（`POWER_RELEASE_MS=15s`）；`restart_app` 顺序 stop_http_server → release_instance_lock → app.restart()。
- 替换已安装应用：先杀主进程（mac 上 `osascript quit` 常被拒，直接 `kill -TERM`）→ 清残留子进程（tsnet/cloudflared 可能成孤儿）→ 备份旧 .app → 暂存新 .app + `codesign -v` → `mv` 交换 → md5 与构建产物比对 → 启动并查 `/health`。

## 性能（OC 扫描，2026-10-28）
- **Windows 分区表一律走 `channels::partitions::all()`**（一次全量 `Get-Partition` + 60s 缓存 + 持锁 single-flight）。禁止再新起分区查询：一轮扫描原先 7 次冷启动共 7.3s，合并后 1 次，扫描 **8.3s → 1.0s**（同进程热刷新 25ms）。`win_esp::mount()` 成功后必须 `invalidate()`。
- **先埋点再动手**：`perf::begin/span/flush` 把每段耗时写进 trace.log 的 `[perf]` 表（thread_local 隔离、含子口径）。凭直觉估「APFS 解析最慢」是错的，真凶是外部命令冷启动；`oc.spec` 那 1.9s 来自每个 generic 条目重算 `volumes()`。
- 本机 ESP 的 GPT 类型是 `ebd0a0a2`（基本数据），`win_esp::list()` 恒空、走 26 盘符兜底——真机事实，别当 bug 修。`apfs.scan` 0ms 也正常（非提权开物理盘立刻 PermissionDenied）。

## 验证 PC UI（Windows，四条铁律）
① 用计划任务跑 .bat 启动（沙箱会杀 WebView2）；② 抓屏用 `GetDC(NULL)+BitBlt`（`PrintWindow` 恒黑）；③ webview 不吃注入点击，走 `ANYBOOT_TEST_OPEN_HUB` / `ANYBOOT_TEST_OPEN_TERM=1` 钩子；④ 本机无 `strings`，用 Python 提串。
- CDP：`ANYBOOT_CDP_PORT=9223` + **所有 webview 都得过 `apply_browser_args`**（参数不一致 = 0x8007139F）；杀旧实例等 5s。
- `ui/*.html` 里元素 id **严禁与 `window.xxx` 同名**（named access 会覆盖 → 判空失效，hub-term 黑屏教训）。
- 验证闭环技能：`~/.workbuddy/skills/anyboot-pc-ui-verify/SKILL.md`。
- **mac 上验证不了 webview DOM**（CDP 是 `cfg(windows)` 实现，`ANYBOOT_CDP_PORT` 无效；`ps`/`screencapture` 被沙箱禁）→ 用 Rust `#[ignore]` 诊断测试 + jsdom 代替，或把界面验收留给 Windows/真机。

## 产品定则
- 默认值钉死常量并配单测：亮色 / 90% / `remoteAllowGuiAuth=true` / 默认高度 = 整屏 2/3。
- 下次启动大字 = 级联全链；Rust 文案与前端 `nextBootText` / `rowLabel` 一字不差。
- 新增托盘菜单项：`grub_menu_nodes`（主窗/自绘窗/Linux dbusmenu/Win-mac classic 四处共用）与 `build_tray_menu` 两边都要加。

## 远端管理（server-hub）
- **产品定则：连接策略机制只属于「电脑列表」**——优先级/停用/探测赢家都由它算，「电脑」页（PC hub 与移动端）只是另一展示主题、样式不同、逻辑分支不许另写。`route_disabled` 认全部六通道 id（别再只认 lan/jump）；电脑页标注只认探测赢家 kind（`pcWinnerKind` / HeartbeatManager `State.kind`）。
- **配对多条目模型已实现**（`docs/remote-pairs.md`，S1–S5 完成）：三区块 `cloudflare|ts|tsnet` 各挂多条 `RemotePair`，**不许再退回固定 String 槽**；顶替只允许 `(mid,kind)` 幂等更新、跨系统永不覆盖；`ts` 区块用 `lane=direct|funnel` 合一直连与 Funnel；两层策略 `route_probe_mode`（区块间）+ `block_probe_mode[kind]`（区块内），三层超时 `pairs[].probe_timeout_ms → block_timeouts_ms[kind] → probe_timeout_ms`，「测试」固定 `parallel_all`；不做向后兼容 / 分组 / MAC 推导 / 自动建档。
- 探测引擎的「区块策略」在 Rust 端由 `ProbeGroup.inner_mode`（建组时按 `block_of_route` 回查）承载，**不是一个全局 address_mode**；清洗策略表不能直接用 `normalize_probe_mode`（它把非法值悄悄变 `parallel_first`）。PC 端三处接线缺一即设置无效：`pcRouteCandidates`（三层超时）、`pcProbe`（传 `blockProbeModes`）、编辑弹窗「测试」（强制 `parallel_all`）。
- `machine_id` = 「一套 AnyBoot 配置」的身份（存 `settings.json`，非系统安装 id）。**铁律：mid 与 tsnet 状态必须同源**，只分 mid 不分 state = 两个身份抢一个节点；Linux 共享 home 是同一身份，靠 `/health` 的 `systemLabel`（`PRETTY_NAME`）区分发行版；tsnet `--hostname` 取配置不取系统 hostname。
- **唯一 SSH 入口 `hub_tunnel::apply_auth`**（askpass = 本程序自身，分支排在 `main.rs` 的 `cli_requested` 之前）。Windows 无 ControlMaster；`sftp -b` 自开 BatchMode → 有密码没密钥时补 `-o BatchMode=no`。
- **同步 tauri command 跑在 UI 线程**：阻塞操作走 async + `spawn_blocking`（守卫单测 `blocking_guard.rs`）；IPC 回调里建 webview 必须 async（否则白屏卡死）。子进程一律 `crate::command()`（防闪黑框）。
- 传输真相源 = `XferTask.state` 五态，变更一律 `app.emit` 广播；退出必须 `detach_running`，中断报 paused。
- 服务器多地址：连接只用 host/port，探活按序试全部地址；下载目录空值 = 系统下载目录（`expand_home` 读 HOME **和** USERPROFILE）；sol 路由语义改了要同步 `hub.html`、`ServerProfile.java`、`MainActivity.kt` 三处。

## 构建
- **Windows**（2026-10-28 起定案：**不再往 J 盘写任何东西**，用户明确要求）：仓库在 `E:\Common\Project\IdeaProjects\anyboot`，产物用默认 `src-tauri/target`（已 gitignore），只把临时目录指到项目内 `$env:TMP=$env:TEMP='E:\...\anyboot\.build\tmp'`（WiX/NSIS 要几 GB，落 C: 会报 os error 112）。`npm run build` 出 msi+nsis，**管道退出码常为 1 但「Finished 2 bundles」即成功**。部署：先 `taskkill /IM anyboot.exe /F`，备份 `AnyBoot-bak-YYYYMMDD<x>.exe` 再复制 exe。12300 落保留段 bind 报 10013 → `netsh int ipv4 set dynamicport tcp start=49152 num=16384` 后重启 winnat。
- **mac**（本机是 Hackintosh x86_64、无 arm64）：`npm run build:mac-intel`；tauri 的 `bundle_dmg.sh` 在本机会失败（要 GUI 自动化摆窗口）→ 卸残留挂载 + 删 `rw.*.dmg` + `hdiutil create -ov -format UDZO` 补包。Windows 版在这里编不出来（无 MSVC）。配方见 `2026-10-26.md`。
- **Linux**：系统 `/usr/bin/rustc` 已坏（要 libLLVM.so.23.1）→ 用自装工具链 `/home/disk1/{cargo,rustup}-home`，release 偶发 SIGABRT 用 `CARGO_BUILD_JOBS=2`；**仓库目录跑不了 check**（指纹扫描读 .git exclude 报 EACCES）→ rsync `src-tauri` + 同级 `ui/` 到仓库外跑。细则见 `2026-10-25.md`。
- **Android**：无 gradlew、无全局 gradle → 直接用 `~/.gradle/wrapper/dists/gradle-8.14.3-bin/*/gradle-8.14.3/bin/gradle`；必须给 JDK17（`/usr/local/software/jdk/jdk-17`）+ `ANDROID_HOME`，并**在命令行覆盖 `-Duser.home`**（`gradle.properties` 里写着 Linux 的 `/home/jianghh`，不覆盖连 debug keystore 都生不出）。产物核验：类不止在 classes.dex（本工程 4 个 dex，`RemotePair` 在 classes3.dex）→ 逐 dex 搜符号，别拿体积当判据。
- 内置 tsnet sidecar 一律跑 `scripts/build-sidecar.sh`，详见 `MEMORY-detail-tailscale.md`。

## 沙箱 / 环境坑
- **safe-delete 垫片把 `rm` 转投回收站且不释放空间**（曾 19G 塞满盘）→ 清缓存用 `/bin/rm` 全路径直删；回收站只清自己当天条目（查 `info/*.trashinfo` 原路径）。
- **沙箱 FS 视图会瞬时劣化**：cargo 工具链整棵消失、`systemctl` 报 not booted、写入丢失或段落重复 → 写完随手核对，工具链没了按 `2026-10-25.md` 用 USTC 镜像重装。
- 沙箱里对该仓库用 shell `grep` 有时静默返回空/失败 → 改用 Grep 工具。
- 验证脚本别用带中文注释的 `.bat`；启动用纯 ASCII 启动器。禁用 `pkill -f <路径>`（会连通自己的 shell 一起杀），按 `/proc/PID/exe` 比对。
