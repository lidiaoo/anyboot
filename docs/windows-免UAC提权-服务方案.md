# Windows 免 UAC 提权：特权助手服务

> 目的：远程 / 无人值守下也能做特权操作（读固件变量、打开 `\\.\PhysicalDriveN`、
> 写 `grubenv`）。当前实现：常驻 Windows 服务 `AnyBootHelper`（LocalSystem）+
> 命名管道。代码：`src-tauri/src/service_win.rs`。

## 为什么必须换机制

UAC 是**会话级**的东西：对话框弹在主机当前交互桌面上。

- 走 AnyBoot 的 HTTP 页面（手机浏览器）时，按钮在浏览器里，UAC 在主机屏幕上——
  人看不到，就永远点不到，卡到超时。
- 机器无人登录时连桌面都没有，`ShellExecuteExW(runas)` 直接失败。

所以「装一次、之后免提权」不是优化，是这条链路能不能成立的前提。

## 结构

```
App（普通权限）
  └─ elevate::run_privileged_op(op, args, fallback_args, out)
       ├─ 1. 命名管道 → AnyBootHelper（LocalSystem）
       │       └─ dispatch(op) → 复用同一批 privileged_* 函数 → 返回 JSON 文本
       └─ 2. 服务不可用才回退：ShellExecuteExW runas → 子进程写 --out 文件
```

两代通道产出的**是同一段 JSON 文本**（就是原本要写进 `--out` 的东西），所以
下游解析器完全不知道这次走的是哪条路。这也是把五个 `*_main` 拆成
`*_json()` + `write_result_file()` 的原因。

| op | 对应能力 |
|---|---|
| `apfs_scan` | 读 APFS Preboot（OpenCore 预演） |
| `nvram_dump` | 读 `OCBt*` / `Boot*` 全量 |
| `oc_set_default` | 写 OC 默认启动项（``--target``/``esp``） |
| `linux_grub_probe` | 枚举 ext4 分区、读 fstab/grub.cfg/grubenv |
| `linux_grub_write` | 覆盖 1024 字节 grubenv |

## 协议

一条连接一次请求，字节流 + `\n` 分帧：

```text
请求 {"v":1,"op":"oc_set_default","args":{"target":"...","esp":"..."}}\n
应答 {"ok":true,"body":"<与 --out 文件逐字节一致的 JSON 文本>"}\n
     {"ok":false,"error":"人话原因"}\n
```

`body` 是字符串而不是嵌套对象：服务回的就是「原本要落盘的那段文本」，客户端
原样写文件即可，不会出现双重编码后字段对不上。

## 安全边界（是设计，不是临时）

1. **op 白名单只有 5 个**，写死在 `dispatch()`；不接受任意命令行、不执行 shell。
2. `linux_grub_write` 的 `rel` 只认 `boot/grub{,2}/grubenv`。放开成任意相对路径，
   「写 1024 字节」就变成「能改 ext4 里任意文件的前 1024 字节」。
3. 请求里没有「读/写哪块盘」的自由，分区由服务自己按分区表枚举。
4. 管道 DACL：SYSTEM / Administrators 全权，Authenticated Users 只读写本管道，
   且 `PIPE_REJECT_REMOTE_CLIENTS` 挡掉远程 SMB 访问。
5. **调用方映像路径必须等于本服务自己的 exe**（`GetNamedPipeClientProcessId` +
   `QueryFullProcessImageNameW`）。别的程序直接连管道会被拒。
6. **exe 必须在管理员独占可写的目录**。`--install-service` 会检查：路径落在
   `%LOCALAPPDATA%`、用户目录、Temp、Downloads、Desktop 下一律拒绝注册。
   原因很直接：服务以 SYSTEM 运行，若它指向用户可以改写的 exe，任何本地用户
   改一下文件就是 SYSTEM —— 那是免费提权后门，比「没有这个功能」糟糕得多。

仍然存在、并且不打算假装消除的风险：**同用户**的代码若能注入本进程，就能用这
五个能力。这与 macOS 助手文档里承认的那条同类（能力有限、不给 shell）。

## 安装与卸载

Windows 的服务注册和安装包之间**没有任何关联**，卸载程序默认什么都不做。

- **NSIS（`npm run build` 的主产物）**：`bundle.windows.nsis.installMode = perMachine`
  → 装到 `Program Files`，安装器带管理员权限；钩子文件
  `src-tauri/windows/installer-hooks.nsh`：
  - `POSTINSTALL` → `anyboot.exe --install-service`
  - `PREUNINSTALL` → `anyboot.exe --uninstall-service`（内部**先停再删**，
    因为服务运行时 exe 被锁，先删文件会让卸载失败）
  - 升级时 Tauri 先跑旧卸载器（删服务）再装新文件（重注册），服务里存的绝对
    路径始终指向当前安装位置。
- **MSI**：不经过 NSIS 钩子，需要手工执行一次（管理员）：
  `"%ProgramFiles%\AnyBoot\anyboot.exe" --install-service`。
- **查询状态**：`anyboot.exe --service-status` → `Missing|Stopped|Running|Unknown`。
- `--install-service` 幂等：服务已存在会先停再删再建（换路径的场景必须重建，
  否则服务里留着旧绝对路径，开机报 7000/7009）。

## 排障

| 现象 | 原因 / 处理 |
|---|---|
| 权限操作仍然弹 UAC | 服务没注册或没起。`--service-status` 应是 `Running` |
| 「调用方不是本程序的安装副本」 | 你跑的是另一份 anyboot.exe（比如 Dev 产物），服务只认安装目录那份 |
| 注册报「拒绝在用户可写目录…」 | 当前 exe 在 `%LOCALAPPDATA%`；改用 perMachine 安装 |
| 开机有 7000/7009 | 服务指向的 exe 被删/移动了。`--install-service` 重建即可 |
| 服务启动失败 1053 | 看 `%ProgramData%\AnyBoot\service.log`，或 eventvwr → 服务控制管理器 |
| `--run-service` 手跑报错 | 正常：SCM 才有分发器上下文（1063） |

## 尚未做

- 设置页里的「安装 / 修复 / 卸载特权助手」按钮（现在只有安装器与 CLI）。
- MSI 的 WiX 片段（`ServiceInstall`/`ServiceControl`），让 MSI 也能自动管理服务。
- 客户端连接复用与分区表缓存落盘：服务进程是常驻的，`partitions::all()` 的
  60 秒进程内缓存天然有效；但 App 侧每次请求仍是新连接，暂未池化。
