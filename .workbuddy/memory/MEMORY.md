# AnyBoot 项目长期备忘

## 构建与验证
- 本机 **debug** 配置编译会挂在依赖 `schemars 0.8.22`（`error[E0107]`，既有环境问题）。
  **`cargo test --release` 只编译测试二进制，不更新 target/release/anyboot.exe**
  （回合 20–22 三轮的修复因此全部没进用户实测的 exe）。正确流程：
  `cargo test --release` 验证测试 + **`cargo build --release` 产出可交付 exe**，
  两者都要跑；构建前先退出 anyboot.exe。前端用 `node --check ui/app.js ui/notify.js`
  （新增页面时也要 check，如 grublist.js / grub-list-view.js）。
- 重新构建前要先退出正在跑的 `anyboot.exe`，否则 `target/release/anyboot.exe` 写不进去
  （改名成 `*.pending-*` 腾位也行，用户退出后手动删）。
- **cargo 必须非沙箱跑**：沙箱里 tauri-codegen（proc-macro）读
  `target/release/build/anyboot-*/out/tauri-codegen-assets/` 下的资产会报
  `拒绝访问 (os error 5)`（shell 里同文件能读）；加 `dangerouslyDisableSandbox` 即可。
- **只改前端（ui/*.html/.js/.css）时，cargo 增量编译不重跑 tauri 打包脚本**，
  前端改动不会进 exe（用户反复反馈「还在/不起作用」多半是这个）。强制重打包：
  `rm -rf target/release/build/anyboot-*/out && touch build.rs && cargo build --release`。
  判断前端是否进 exe：`out/tauri-codegen-assets/*.css` 是 brotli 压缩，
  用 node `zlib.brotliDecompressSync` 解压后 grep 关键字核对。

## 结构要点
- Rust 后端单文件 `src-tauri/src/lib.rs`（很大）+ `http.rs`（内置 HTTP 服务，tiny_http 0.12）。
- 前端 `ui/`：`index.html` / `app.js` / `styles.css` 主窗口；`notify.html/js/css` 通知弹窗；
  `grublist.html/js/css` GRUB 独立窗口（与主窗口共用 `grub-list-view.js` 树形渲染器）。
- 多窗口数据同源：`publish_ui` 的 `app.emit("ui-state-changed")` 广播到所有窗口，
  新窗口进场先 `invoke("get_ui_state")`、再监听同一事件即可，不需要新通道。
- 新建窗口一律 `.visible(false)` 先建、摆好位置再 show（否则 Windows 上在默认位置
  闪一帧白底）；主窗口靠 tauri.conf.json 的 `"visible": false` 达成同样效果。
- 托盘菜单：原生菜单没有分组概念，分区只能靠「灰掉的标题项 + 两条分隔线」表达
  （见 `menu_section_header`）。
- 运行态快照 `UiState` 是托盘与主窗口的**唯一**事实来源；明文 token **不进快照**（只有打码版），
  所以前端只能靠派生字段判断 token 是否变化。
- 起子进程必须走 `fn command(program)`（Windows 带 `CREATE_NO_WINDOW`），否则 GUI 程序会弹出蓝色控制台窗口。

## tiny_http 的坑（Windows 上致命）
- `Server::drop` 用 `TcpStream::connect(listening_addr)` 唤醒自己的 accept 线程；
  绑 `0.0.0.0` 时那是**通配地址**，Windows 上 connect 它必失败（10049），
  accept 线程永远卡在 `accept()`，监听 socket 永不释放 → 本进程再也绑不上该端口（10048）。
- 所以收服务不能只 `join` 自己的 worker：必须**自己补一次可达的自连接（127.0.0.1）**
  并轮询到端口真的能 bind 为止。`http::RunningServer::stop()` 的契约就是
  「返回时端口一定已放开」，别再往调用方塞重试兜底。

## 用户偏好（本项目的做法）
- 要求精准的根因分析 + 精准修改，不接受猜测性代码。
- 主方法足够时**主动删掉冗余兜底**（例如「勾选时自动重试一次」那种猜 TIME_WAIT 的补丁）。
- 改动要能用测试钉住；涉及平台差异的地方写明「为什么只在 Windows 上炸」。
