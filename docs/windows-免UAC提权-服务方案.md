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

## 唯一收口：`elevate::run_op`

Windows 上**任何**需要管理员权限的操作都只有一条路：

```text
调用点 → elevate::run_op(op, args)
           ├─ 命名管道问服务（常驻、免 UAC）           → 完成
           ├─ 服务不可用 → ShellExecuteExW(runas) 本程序
           │                `--privileged-op <op> --args <json> --out <file>`
           └─ 服务明确失败 → 直接返回错误（不再弹 UAC：同一个错误不必报两次）
```

关键点：`--privileged-op` 子命令**直接调用 `dispatch()`**，所以「走服务」和
「弹授权」跑的是同一份实现 —— 参数校验、错误文案、返回体形状永远一致，不会出现
「走服务能用、弹授权报参数错」这种随时间去漂的差异。

因此新增一个可提权能力只有两步：在 `dispatch()` 加一条 match 臂，调用点换成
`run_op`。**不要**再加 argv 分支（`--apfs-scan-privileged` 那 5 条是历史形状，
仅为兼容保留）。

判定「要不要进白名单」的标准不是「需不需要提权」，而是**「远程/无人值守时会不会
被触发」**：会 → 必须进（UAC 对远程等于失败）；不会（一次性维护）→ 只留兜底即可。
白名单每加一项就多一分 SYSTEM 攻击面。

### 版本错配与便携副本

有两类应答必须判成 **`Unavailable`**（→ 改走 UAC 兜底），而不是 `Failed`，
否则会变成「装了服务反而把功能堵死」的死路：

| 应答 `code` | 场景 | 若误判成 Failed 的后果 |
|---|---|---|
| `unknown_op` | 升级后服务还是老进程，不认识新 op | 装了新版本却什么特权操作都做不了，还不弹授权 |
| `not_our_exe` | 便携副本 / 旧副本调用已安装的服务 | 本机装了服务，便携版的每个特权操作都报「拒绝执行」 |

所以这两条文案与 `code` 都是**协议的一部分**（常量 `UNKNOWN_OP_PREFIX` /
`CODE_NOT_OUR_EXE`），别随手改字。老的（没有 `code` 字段的）服务靠前缀识别。

### 并发

服务是**每连接一线程**的（`INFLIGHT` 计数，停服务最多等 5 秒再报 STOPPED）。
不能退回串行：一次 APFS 扫描要几十秒，期间并发请求会撞 `ERROR_PIPE_BUSY`，
客户端只等 5 秒就返回 `Failed`（注意**不是** `Unavailable`，不会回退 UAC）——
表现成「特权服务忙」这种没人看得懂的硬失败。

## 三种提权语义：`Elevation`

同一个能力，**谁发起的**决定了「能不能弹授权框」。这不是礼貌问题，是硬约束：

| | 发起方 | 通道 | 服务不在时 |
|---|---|---|---|
| `Elevation::Allow` | 用户亲手点的按钮 | 服务 → UAC | 弹一次授权（合理：他刚点的） |
| `Elevation::Silent` | 开机 / 进页面**自动**扫 | **只有**服务 | 报错，绝不弹 UAC |

自动扫必须是 `Silent`，两个理由：远程/无人值守时 UAC 弹在主机桌面上，手机端永远
点不到，只会白卡 30 秒；有人在机器前时，开机瞬间弹一个「没人主动要求」的授权框
是最招人烦的打扰，而且拒一次就等于这次扫描彻底失败 —— 用户还不知道自己拒了什么。

对应实现：`elevate::run_silent_op(op, args, out)`（只问服务）与
`elevate::run_privileged_op`（服务 → UAC）。两者成功时**落同一份结果文件**，
下游解析代码不分叉。

前端据此决定「要不要自动扫」：`linux_grub_helper_ready` 在三端统一成
「有静默提权通道吗」——macOS 看 root 助手装没装，Windows 看特权服务在不在跑，
Linux 恒 `true`（本机直读）。没有静默通道时**不自动扫**，免得每次开机都换回一条
「不弹授权」的报错；手动按钮照旧可用。

### 进 App 就自动扫的两份数据

`channels::spawn_boot_privileged_scan` 在后端启动线程里跑，用户体感是「一进去就有」：

| 数据 | 谁在用 | 不提权会怎样 |
|---|---|---|
| `OCBtOrder` / `BootOrder` | OC 预演的「默认项」标记 | 列表能看，但看不出哪个是默认 |
| APFS Preboot（macOS 条目） | OC 预演条目 | macOS 条目凭空消失 |

启动顺序读一次（毫秒级），再扫 APFS 容器（秒级），成功就
`refresh_channels` + `publish_ui` 把首帧之后的那一拍补上。服务没在跑 → 整段跳过并
留一行 `[boot-scan]` trace，卡片上的「重扫 macOS 条目 / 重读启动顺序」仍可手动触发
那一次授权。

**GRUB 自动读取不在这里做**：那条链路由前端页面加载时触发（`linux_grub_scan`
`auto=true` → `Silent`），因为探测本身要读十块裸分区，后端再跑一遍只是重复劳动。

## 安全边界（是设计，不是临时）

1. **op 白名单只有 9 个**，写死在 `dispatch()`；不接受任意命令行、不执行 shell。
   每个 op 的参数都是**枚举化 / 数值化**的：`channel ∈ {opencore,clover}`、
   `mode ∈ {full,short}`、`disk/partition` 是 JSON number、`esp` 必须带通道签名
   且 Windows 上必须是卷根（`H:\`，拒绝 `H:\EFI`）。
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
  只有第一行是机器可读的；服务在跑、但当前这份 exe 不在安装目录时，会多打两行
  提示 —— 那种情况下服务会拒掉所有调用，自动提权扫描也整段跳过。
- `--install-service` 幂等：服务已存在会先停再删再建（换路径的场景必须重建，
  否则服务里留着旧绝对路径，开机报 7000/7009）。

## 排障

| 现象 | 原因 / 处理 |
|---|---|
| 权限操作仍然弹 UAC | 服务没注册或没起。`--service-status` 应是 `Running` |
| 进 App 后 OC 不显示 macOS 条目 / 默认项 | 自动提权扫被跳过：服务没在跑，或当前跑的是便携副本（`--service-status` 第二段会明说）。卡片上「重扫 macOS 条目 / 重读启动顺序」仍可手动补 |
| 「调用方不是本程序的安装副本」 | 你跑的是另一份 anyboot.exe（比如 Dev 产物），服务只认安装目录那份 |
| 注册报「拒绝在用户可写目录…」 | 当前 exe 在 `%LOCALAPPDATA%`；改用 perMachine 安装 |
| 开机有 7000/7009 | 服务指向的 exe 被删/移动了。`--install-service` 重建即可 |
| 服务启动失败 1053 | 看 `%ProgramData%\AnyBoot\service.log`，或 eventvwr → 服务控制管理器 |
| `--run-service` 手跑报错 | 正常：SCM 才有分发器上下文（1063） |

## 尚未做

- 设置页里的「安装 / 修复 / 卸载特权助手」按钮（现在只有安装器与 CLI）。
- 客户端连接复用：App 侧每次请求仍是新连接，暂未池化（服务端已经是并发的，
  所以这不是正确性问题，只是每次多一次 `CreateFileW`）。

已做过、别再当成待办：MSI 的 WiX 片段（`windows/service-fragment.wxs`，
延迟型自定义动作）与服务端并发化。
