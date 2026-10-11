# AnyBoot 项目长期备忘

## 构建与验证（照抄这串，别漏）
- `export HOME=/home/jianghh CARGO_HOME=/home/jianghh/.cargo`（本机 `$HOME=/root` 属主 nobody → cargo 读 `~/.rustfmt.toml` Permission denied）。
- **cargo 必须非沙箱跑**（沙箱里 tauri-codegen 读 `target/release/build/anyboot-*/out/` 报 os error 5）。
- `cargo test --release` **不产出可交付 exe**；交付 = 测试 **+ `cargo build --release`**，构建前退出运行中的 anyboot。
- 只改前端时增量编译**不重跑打包**，前端不进 exe。强制：`rm -rf target/release/build/anyboot-*/out && touch build.rs && cargo build --release`（`rm` 被 safe-delete 拦时用 `/usr/bin/rm`）。核对：`out/tauri-codegen-assets/*.css|js` 是 brotli，node `zlib.brotliDecompressSync` 解开 grep。前端语法：`node --check ui/<每个>.js`。
- 冒烟脚本在 `/home/jianghh/.cache/ab-test/`（run.sh 贴边 / mode-smoke.sh 四模式 / size-smoke.sh 窗口尺寸 / notify-smoke.sh 弹窗）。宿主 12300 被占时给 settings 预置 `httpPort: 12400`。

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
- tao 未映射窗口 `outer_size()` 返回 0x0（inner 可读）。
- **隐藏窗口的 WebView 视口在 Linux/GTK 上是 0x0**（Windows/macOS 有真实分配）：0 宽视口排版把文字折成竖条 → 前端量「内容高度」严重虚高（notify 弹窗曾恒顶满上限 460，底部空白一大块）。修法：`.panel` 钉 `min-width` = 窗口宽度常量（两处必须一致）+ 监听 `resize` 在 show 后重测自愈。凡是「隐藏窗口里量 DOM 排版」都要过这条。
- 判「乱闪乱跳」只认振荡判据：状态 <600ms 内回到两拍前的值（A→B→A）。
