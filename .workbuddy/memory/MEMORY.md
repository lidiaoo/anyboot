# AnyBoot 项目长期备忘

## 构建与验证（照抄这串，别漏）
- `export HOME=/home/jianghh CARGO_HOME=/home/jianghh/.cargo`（本机 `$HOME=/root` 但 /root
  属主 nobody+750 → cargo fmt/任何读 `~/.rustfmt.toml` 的都 Permission denied）。
- **cargo 必须非沙箱跑**（沙箱里 tauri-codegen 读 `target/release/build/anyboot-*/out/`
  下资产报 `拒绝访问 (os error 5)`）→ 加 `dangerouslyDisableSandbox`。
- `cargo test --release` **只编译测试二进制，不产出可交付 exe**（曾连吃三轮：修复全没进
  用户实测的 exe）。交付流程：`cargo test --release` 验测试 **+ `cargo build --release`
  产出 `target/release/anyboot`**，再 `strings -a <exe> | grep <新加的日志关键字>` 自证新码进没进。
- 只改前端（`ui/*.js|html|css`）时增量编译**不重跑 tauri 打包脚本**，前端改动不会进 exe
  （用户反复反馈「还在/不起作用」多半是这个）。强制重打包：
  `rm -rf target/release/build/anyboot-*/out && touch build.rs && cargo build --release`。
  核对法：`out/tauri-codegen-assets/*.css` 是 brotli，用 node `zlib.brotliDecompressSync` 解开 grep。
  前端语法检查：`node --check ui/<每个>.js`（新增页面别漏）。

## 结构要点
- 后端单文件 `src-tauri/src/lib.rs`（很大）+ `http.rs`（内置 HTTP，tiny_http 0.12）。
- 前端 `ui/`：`index.html/app.js/styles.css` 主窗；`notify.*` 通知弹窗；`grublist.*` GRUB 独立窗
  （与主窗共用 `grub-list-view.js` 树形渲染）。
- 多窗口同源：`publish_ui` 里 `app.emit("ui-state-changed")` 广播；新窗口进场先
  `invoke("get_ui_state")` 再监听同一事件，**不需要新通道**。
- 新窗口 `.visible(false)` 建好、摆好再 show（否则 Windows 先闪一帧白底）；主窗靠
  tauri.conf.json 的 `"visible": false`。
- 托盘菜单没有分组概念，分区只能靠「灰掉的标题项 + 两条分隔线」（`menu_section_header`）。
- `UiState` 是托盘与主窗的**唯一**事实来源；明文 token 不进快照（只有打码版）。
- 起子进程必须走 `fn command(program)`（Windows 带 `CREATE_NO_WINDOW`），否则弹黑框。

## tiny_http 的坑（Windows 上致命）
- `Server::drop` 靠 `TcpStream::connect(listening_addr)` 唤醒 accept 线程；绑 `0.0.0.0`
  时那是通配地址，Windows 上 connect 必失败(10049) → accept 永远卡住、端口永不释放(10048)。
- 所以 `RunningServer::stop()` 必须**自己补一次 127.0.0.1 自连接**并轮询到端口真能 bind；
  它的契约就是「返回时端口一定已放开」，别再往调用方塞重试兜底。

## Linux 写 ESP / 同步 grub.cfg 的权限模型
- 两个动作在标准发行版上**只有 root 能做**（不是用户挂载配错）：① 读 `/boot/grub/grub.cfg`
  ——`grub-mkconfig` 自己 `umask 077` 写成 `0600 root:root`（跑一次就重置，chmod 没用）；
  ② 写 `/boot/efi`——fstab 的 vfat 无 `uid=` → root 属主、`dmask=0022` → 普通用户建不了文件。
  报错 `权限不够 (os error 13)` 发生在**开源源文件**那步（`fs::copy` 先开源再建目标）。
- 修法：`copy_into_esp`/`write_esp_bytes` 先 `access(2)` 看真权限，能写就原子写，写不动就
  `pkexec install -p -m 0644`（`mount_command` 自动套 pkexec，与装 09_anyboot 同路）。
  `install -p` 带源 mtime + `same_size_and_time`（FAT 只有 2s 精度）→ 同一份 grub.cfg 不重复弹框。
- 远程请求不能弹框：`execute_remote_command` 用 `RemoteNoGuiAuth::enter(allow)` 圈住整次执行。
- 同类隐患（未改，rEFInd 链路）：ESP 被系统以 rw + root 属主挂着时，`mounted_scan_candidates`
  只在 `ro || owned` 给 ActiveMount → 写 `PreviousBoot` 会 EACCES。


## 窗口行为（贴边自动隐藏 / GRUB 独立窗）—— 每次必查的定则
细节与实测数字见 `MEMORY-detail-windowing.md`（坐标系表格、复现台开关、诊断行清单都在那），
历史沿革见每日日志。这里只留**动手前必须知道**的几条：
- 判位**只用客户区坐标**：`main_client_position()` / `inner_position()`（`set_position`/`set_size`
  也是客户区语义）。`Moved` 载荷与 `outer_position()` 是**窗框**坐标，Linux 下去装饰后恒偏一个
  标题栏（37px）→ 拿它比 `dock_pos` 必错。Windows 上 `outer == inner`，所以那边看不出这类 bug。
- `geometry_busy()`（`mark_geometry_busy(400)`，**必须在发几何请求之前**立起来）= 「我们刚编程过
  几何，现在读到的坐标/Moved 是中间态」。**dock 与 undock 两侧都要挡**（漏一侧就是
  176ms 内「展开→缩回条子」的闪跳，见 `undocked_counts_ticks` 的 5 个否决条件）。
- 「离窗时钟」不许在**窗口还在动**时累计：`main_moved_recently()`（`LAST_MAIN_MOVED_MS` 在
  `handle_main_moved` 入口盖戳，静默期 `AUTO_HIDE_MOVE_QUIET_MS = 300`）。用户拖窗口/拖白条时
  Moved 连发，那一刻的「光标在窗外」是拖动中间态，不是用户离开了。
- 在**可见**窗口上做「去装饰+改尺寸+落位」会被重父化吃掉 position → **`hide()` → 改几何 →
  `show()`**，且 monitor/target 要在 hide 前算完（不可见时 `current_monitor()` 退化成主显示器）。
- 「自己造成的回声」哨兵必须**持久**（一次 `set_position` 会产生多条同坐标 Moved），
  判据 = 「与最后一次要求它去的位置一致(±8px)」（`grub_move_is_our_echo`）。
- tao 0.35.3 里**未映射窗口的 `outer_size()` 返回 `0x0`**（inner 可读）→ 凡拿窗口尺寸当基准，
  先问「这个读数在未映射时可信吗」（`grub_align_target` / `grub_initial_size_basis` 就是为它写的）。
- 主窗太大（最大化/半屏）时列表**摆不下** → `grub_placement` 返回 `None` → 调用方把独立窗
  藏起来（别压在主窗上，也别每 120ms 开→藏循环）。
- 复现台：`/home/jianghh/.cache/ab-test/run.sh`（副本在 skill `linux-x11-window-repro`）。
  **判「乱闪乱跳」只认振荡判据**：状态 <600ms 内回到两拍之前的值（`A→B→A`）——
  段数和「相邻切换 <300ms」都是错的量具（正常 dock 节奏、拖窗时的正常跟随都会被误判）。
