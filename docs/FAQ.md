# 常见问题（FAQ）

> 原 README「常见问题 / 注意」章节归档。远程访问、端口、构建类问题在这里。

## 远程访问不了：按这个顺序查

1. 主窗口「远程控制」里的状态是不是「正在监听 0.0.0.0:12300」？不是就看它给出的原因（常见：token 为空、端口被别的程序占了）。
2. 目标机上 `curl http://127.0.0.1:12300/health` 通不通？通说明服务没问题，问题在网络/防火墙。
3. 从另一台机器 `curl http://<目标IP>:12300/health`：不通就去 [三端各自的防火墙处理](远程控制-HTTP-API.md#三端各自的防火墙处理)，并确认 `httpBind` 不是 `127.0.0.1`。
4. `/health` 通但 `/api/status` 返回 401：token 不对（重新复制一次）。
5. 返回 409：有条待执行请求在倒计时，等它执行完或去主窗口/弹窗点「取消」。

## 再启动一个 AnyBoot 为什么提示已经启动

AnyBoot 是**单实例**应用：同一时间只允许跑一个。第二次启动（双击图标、命令行又跑一次、脚本里再拉一遍）**会先把上一个实例结束掉，再自己正常启动** —— 不会留下两个托盘图标、不会两扇窗口抢端口、也不会再抢 HTTP 端口。

接管顺序（每一步都为了「不杀错人」）：

1. 抢配置目录下的 `anyboot.lock` 文件锁；抢到 = 本进程就是唯一实例。
2. 抢不到就读 `anyboot.pid`（上一个实例拿到锁时写下的 pid）。
3. **核对那个 pid 确实是 AnyBoot**（按可执行文件名比，AppImage 每次挂载路径不同所以只比 basename）—— 挡的是「PID 被系统回收后撞上别的程序」。
4. 先礼后兵：Unix 发 `SIGTERM`，让它走正常退出流程（只有走 `RunEvent::Exit` 才会收掉 ssh / cloudflared / tsnet 这些子进程）；**3 秒不退才强杀**，并把它当时的子孙进程一起收掉，不留孤儿。Windows 直接用 `taskkill /T /F`（关窗默认是「隐藏到托盘」，没有可用的优雅退出）。

三种情况**不接管**，退回老行为（弹「AnyBoot 已经启动，只能运行一个实例。」然后退出）：`anyboot.pid` 缺失（上一个实例是没这个功能的旧版本）、那个 pid 已经不是 AnyBoot、强杀之后锁还没放开。

- 锁是配置目录下的文件锁，进程一结束就由内核释放 —— 崩溃、强杀都不会留下「进程早没了、锁还在，于是谁都起不来」的假死状态，也不用手工删锁文件。
- 「重启程序」按钮会**先放锁再拉起新进程**（tauri 的重启是「先起新进程、再退旧进程」），所以点它不会把自己锁在门外。
- 那句兜底提示在 macOS 上走 `osascript` 的 `display dialog`（跟电源动作同一条路），Linux / Windows 用应用自带的对话框。
- 想同时跑两套（比如做验证）：`ANYBOOT_CONFIG_DIR=/tmp/ab-test <可执行文件>` —— 锁和 pid 都跟着配置目录走，两套互不干扰，**也不会互相接管**。

## Linux 上每次提权都要输密码

见 [`权限与提权.md`](权限与提权.md) 的「Linux 免密提权」——「关于」页点一次「安装免密组件」即可。

## 端口被占用 / 想换端口

应用**不会**自动换端口（避免你以为在 12300 其实是别的）。如果勾上服务却起不来，主窗口「远程控制」区会直接出现一块**端口被占用**提示，写清是谁占的：

- **是本应用的另一个实例**（旧版本驻留：老版本没有单实例保护，升级后它可能还占着端口）：
  `端口被本应用的另一个实例占用：PID 40048（anyboot.exe）`
  下面有「结束占用进程」按钮，点了就结束那个实例（按钮上写着 PID，不会杀错人）。
- **是别的程序**：
  `端口被 PID 31760（python.exe）占用，请释放它或改用其他端口`
  同样有「结束占用进程」按钮（点了结束那个进程），旁边还有等价命令可复制（Windows `taskkill /PID 31760 /F`，Unix `kill 31760`）。
- **是本程序自己**（正常不该出现，属于上一轮监听没放开）：
  `端口还被本程序自己占着（PID 34792）`
  这一条**没有**「结束占用进程」按钮（不能结束自己），点「重试监听」；仍然不行就重启本程序。

按钮旁边有「重试监听」（端口刚被释放、或你手动处理完之后点一下，不用重启应用）。
**这些动作都要你自己点**：应用只做只读探测（`netstat -ano` 拿 PID + 查进程名），不会自动结束任何进程。

- **取消勾选「HTTP 服务」会立刻停服务并释放端口**（不是等进程退出）。
- **改 token / 监听地址 / 端口 = 先停旧监听、确认端口已放开、再重新绑**，所以换 token 时不会撞上自己占的端口（`set_http_token` / `regenerate_http_token` 返回的是重绑之后的真实状态，不是「应该重启好了」的猜测）。
- 想换端口：主窗口「监听地址 / 端口」改完点「应用」，或 `ANYBOOT_HTTP_PORT=12400` 启动。

自己查也是这几条：

```bash
ss -ltnp | grep 12300                 # Linux
lsof -nP -iTCP:12300 -sTCP:LISTEN     # macOS
netstat -ano | findstr 12300          # Windows（拿到 PID 后：tasklist | findstr <PID>）
```

## 睡眠变成了休眠

Windows 上如果启用了休眠，`SetSuspendState 0,1,0` 会进休眠。应用每次都会**只读**预检并告诉你结论；不想变休眠就先关掉休眠（这会改系统设置，自己决定）：

```powershell
powercfg /hibernate off     # 关闭休眠（会删除 hiberfil.sys，需管理员）
```

详见 [`远程控制-HTTP-API.md`](远程控制-HTTP-API.md) 的「电源动作」。

## Linux 应用设置保存失败

除“开机启动”外其他设置保存后恢复原值，先检查配置目录属主：

```bash
ls -ld ~/.config/com.example.anyboot
```

属主显示为 `root`（此前用 `sudo`/`pkexec` 跑过应用）就改回来：

```bash
sudo chown -R "$(id -un):$(id -gn)" ~/.config/com.example.anyboot
```

开机启动写的是 `~/.config/autostart`，在这个问题下仍可能正常工作。

## 窗口一片空白 / 只显示白色（Linux + NVIDIA 独显）

WebKitGTK 与 NVIDIA 专有驱动在这条渲染路径上谈不拢，会在**不报任何错**的情况下画不出来：窗口、托盘、日志都正常，就是内容全白。Intel/Mesa 走同一条路径没问题，所以现象常是「核显好好的，一切到独显就白」。

同一个毛病还有第二种表现：**内容其实加载了，但只有鼠标扫过的地方才显示出来**。所以别看到「能加载」就以为过去了。

应用启动时会自己处理，不需要你配任何东西：检测到 NVIDIA 驱动已加载（`/proc/driver/nvidia/version` 存在）时，在 GTK / WebKit 起来之前按会话类型设好对应开关：

| 会话 | 设置的开关 |
| --- | --- |
| X11（Cinnamon / KDE / GNOME 等） | `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1` |
| Wayland + Hyprland | `WEBKIT_DISABLE_DMABUF_RENDERER=1` |
| Wayland + KWin 等严格要求 acquire point 的合成器 | `__NV_DISABLE_EXPLICIT_SYNC=1` |
| Wayland + 其它合成器（驱动 ≥560 自带 egl-wayland2） | 不改（这条路径本来是好的） |

X11 用的 `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1` 是让 WebKit 把帧以**共享内存**交给窗口系统，绕开 NVIDIA 收不下的 GPU 缓冲。这是 NVIDIA 上唯一实测能把页面画出来的开关（WebKit Bug 324551）；同类的 `WEBKIT_DISABLE_DMABUF_RENDERER=1` 自 WebKit 2.52 起已不再回退到共享内存、反而会让画面画不全，`WEBKIT_DISABLE_COMPOSITING_MODE=1` 在 2.54 已失效 —— 这两个都别再用了。

- **纯核显模式不加任何开关**：NVIDIA 驱动没加载就什么都不会动，不需要这些开关的机器不会白白掉性能。
- **你自己设过上面任一个开关**（哪怕设成 `0`）应用就完全不动手，按你的配置来。想手动对照，就用 `WEBKIT_DMABUF_RENDERER_FORCE_SHM=0` 之类跑一次。
- 想看它到底做了什么：`ANYBOOT_TRACE=1 ./anyboot`，启动时会往 stderr 打一行 `anyboot: NVIDIA 驱动 615 / X11，已设置 WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`。

## 想看“到底发生了什么”

主窗口「远程控制 → 最近请求」或 `GET /api/requests` 看最近 20 条；更细的过程用 `ANYBOOT_TRACE=1`（或设置里打开 `debugTrace`）看配置目录下的 `trace.log`——里面**不会有**完整 token。

## 注意（安全与边界）

- 应用只修改 rEFInd 的 `PreviousBoot` 嗅探状态，不直接改引导项配置；三个 `PreviousBoot-*` 模板文件始终只读。
- HTTP 是**明文**（没有 HTTPS），token 必填但会以明文过网：只用在可信局域网或 SSH 隧道里，并保持 `settings.json` 的正常读权限（别 `chmod 777`）。
- 默认 `0.0.0.0` 是为了“装好就能被远程调用”；不需要就改成 `127.0.0.1` 并走隧道。
- 正式发布前建议替换品牌图标，并把 `bundle.macOS.signingIdentity` 换成 Apple Developer ID 证书（或用 `APPLE_SIGNING_IDENTITY` 覆盖）。ad-hoc 签名只保证系统登录项接口和本地运行可用，未做公证，首次打开仍需在「隐私与安全性」中放行。
