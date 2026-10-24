# SOL 面板细则

- **`~/sol` 是上游压缩包解压出来的，禁止改里面的文件**（`install.ps1` / `install.cmd` / `install.sh` /
  `sol.exe` / `README*` / `sol.schema.json` / `sol-example.yaml` / `install-windows.yaml`）。
  那是上游产物，本项目只做**调用方**：改脚本 = 下次升级覆盖 + 排查时分不清是谁的问题。
  要改行为只能改 `src-tauri/src/sol_panel.rs` 的调用方式（环境变量、stdin 喂菜单、参数）。
- 区分「我改的」和「脚本自己写的」：运行 `/sol` 安装脚本会重写 `install.json`（台账）、`install.yaml`、
  `sol.yaml`、`install.log`、`elevated-install.progress`、`skills/` —— 这些是运行产物，正常。
  核验有没有手改过源文件：`ls -la --time-style=+%m-%d\ %H:%M:%S ~/sol`，源文件时间戳应全部等于
  解压时间（本机 = `10-21 09:03:30`）。
- Windows 提权不闪窗的实现细节在 `sol_panel.rs::run_elevated_hidden`
  （`ShellExecuteExW` + `SEE_MASK_NO_CONSOLE`）；改完必须实测
  （`cargo test --lib elevated_hidden_no_console -- --ignored` + 窗口采样器）。
