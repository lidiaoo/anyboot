# AnyBoot 项目长期备忘

## 构建与验证
- 本机 **debug** 配置编译会挂在依赖 `schemars 0.8.22`（`error[E0107]`，既有环境问题）。
  一律用 `cargo test --release` 验证；前端用 `node --check ui/app.js ui/notify.js`。
- 重新构建前要先退出正在跑的 `anyboot.exe`，否则 `target/release/anyboot.exe` 写不进去。

## 结构要点
- Rust 后端单文件 `src-tauri/src/lib.rs`（很大）+ `http.rs`（内置 HTTP 服务，tiny_http 0.12）。
- 前端 `ui/`：`index.html` / `app.js` / `styles.css` 主窗口；`notify.html/js/css` 通知弹窗。
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
