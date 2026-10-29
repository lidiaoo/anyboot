; AnyBoot 的 NSIS 安装钩子（Tauri 的 bundle.windows.nsis.installerHooks 指向本文件）。
;
; 这里只做一件安装包必须做的事：**注册 / 删除特权助手服务**。
; 为什么非要放在安装器里 —— Windows 的服务注册信息和安装包之间没有任何关联，
; 安装器不做，卸载后就会留下一个指向「已经不存在的 exe」的服务：之后每次开机
; SCM 都报 7000/7009，事件日志里挂着一个用户看不懂的失败项。
;
; 顺序上的两个硬要求：
; 1. 卸载必须「先停再删」，`--uninstall-service` 内部已经按这个顺序做了。
;    服务在运行时 exe 被占用，先删文件会失败（NSIS 报「文件正在使用」）。
; 2. 升级时 Tauri 会先跑旧版卸载器（触发下面 PREUNINSTALL 删掉服务），
;    再装新文件（POSTINSTALL 重新注册）—— 所以服务里存的绝对路径总是指向
;    当前安装位置，不会残留旧路径。这一点很重要：`currentUser` → `perMachine`
;    换过目录之后，不重注册就会指向一个已经不存在的 exe。

!macro NSIS_HOOK_POSTINSTALL
  ; 安装器此时已经是管理员（perMachine），注册服务不需要再弹 UAC。
  ; 失败不阻断安装：服务装不上只影响「免 UAC」，功能本身仍可走弹出授权那条路。
  DetailPrint "注册 AnyBoot 特权助手服务…"
  ExecWait '"$INSTDIR\anyboot.exe" --install-service'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; 停 → 删 → 之后才轮到 NSIS 删文件。反过来会因 exe 被占用而卸载失败。
  DetailPrint "停止并删除 AnyBoot 特权助手服务…"
  ExecWait '"$INSTDIR\anyboot.exe" --uninstall-service'
!macroend
