# AnyBoot · OpenCore 菜单里的系统列表是怎么来的

> 一句话：**OpenCore 不遍历硬盘找系统。** 它把三个固定来源并进一张表，过滤掉一部分，再按 NVRAM 里的顺序排一遍，就成了你按 F12 进去看到的那一排图标。
>
> 本文讲「是什么、怎么用、边界在哪」。
> 逐函数源码追踪、实机验证过程、以及被推翻的几版错误结论，都在
> [`boot-menu-discovery-internals.md`](boot-menu-discovery-internals.md)。

---

## 1. 三个来源

`OcScanForBootEntries()`（`BootEntryManagement.c:2103`）干的只有一件事：**先列出所有文件系统，再对每个卷跑固定几步**。

### ① 固件的 `BootOrder` 里的每一条 `Boot####`

`AddBootEntryFromBootOption`（`:1224`）把 `EFI_LOAD_OPTION` 读出来，**丢掉描述、丢掉 OptionalData**，只留设备路径，再按 HardDrive 节点反查是哪个卷，挂上文件路径。

这就是 macOS 那一项（描述 `MAC`）的来源——它本来就躺在 NVRAM 里，OC 只是照着念。

### ② 每个卷上的 Apple bless 扫描

`AddBootEntryFromBless`（`:912`）对每个卷按固定顺序试三件事，**前一步有了就停**：

| 顺序 | 试什么 | 存在哪 |
|---|---|---|
| 1 | 被 bless 过的**文件** | 卷上的 `gAppleBlessedSystemFileInfoGuid` 变量 |
| 2 | 被 bless 过的**文件夹** | `gAppleBlessedSystemFolderInfoGuid` |
| 3 | **预定义路径表** | 见下表 |

预定义路径表只有 **3 条**（`OcAppleBootPolicyLib.c:74-78`，源码注释在 `:60-72`）：

```c
\System\Library\CoreServices\boot.efi    // macOS
\EFI\Microsoft\Boot\bootmgfw.efi         // Windows
\EFI\BOOT\BOOTX64.EFI                    // 通用兜底
```

兜底那一条是**按架构**的：x64 是 `BOOTX64.EFI`，ARM64 是 `BOOTAA64.EFI`（同一位置）。
AnyBoot 的表里两条都列、x64 在前 —— 同一个卷上两者都存在的可能性极低，但少了 AA64
那一条，Apple Silicon / Windows on ARM 上整条兜底会凭空消失。

**Linux 不在表里，这是设计选择不是 bug。** 源码原话（`:71-72`）：

> *Linux and related entries are not present here, because they have fine working software for boot management and do not use BOOTx64.efi in the first place.*

所以：没装 `OpenLinuxBoot.efi` 的 OC，**只会把你的 Linux 认成「某个卷上的 BOOTX64.EFI」**，图标是灰的问号，点进去才进 GRUB。

**APFS 多一层**（`:987-1053`）：卷的角色是 **Preboot** 时，先取 blessed，再对**同一容器里每个卷 UUID** 试
`Preboot:\{UUID}\System\Library\CoreServices\boot.efi`。这是「一个 APFS 容器装了多个 macOS，菜单里能全列出来」的原因。

### ③ 第三方「条目协议」驱动

实现 `gOcBootEntryProtocolGuid` 的驱动会自己往菜单里塞条目，由 `OcAddEntriesFromBootEntryProtocol`（`BootEntryProtocol.c:239`）收集。真正遍历目录的只有它们：

| 驱动 | 干什么 |
|---|---|
| `OpenLinuxBoot.efi` | 读 `\loader\entries\*.conf`；没有就 autodetect `vmlinuz*` / `initrd*` |
| `OpenLegacyBoot.efi` | 传统 MBR 引导 |
| `OpenNetworkBoot.efi` | 网络启动 |
| `ResetNvramEntry.efi` / `ToggleSipEntry.efi` | 纯工具项（`NVRAM 重置` / `切换 SIP`） |

**想让 Arch 在 OC 菜单里变成独立内核条目，唯一的办法就是加 `OpenLinuxBoot.efi`。** 改 `ScanPolicy`、改 `HideAuxiliary` 都没用。

---

## 2. 一个卷能不能进候选：只看 `ScanPolicy`

`BuildFileSystemList()`（`:1945`）用 `LocateHandleBuffer(ByProtocol, SimpleFileSystem)`（`:1977`）拿**所有**文件系统句柄，然后逐个 `AddFileSystemEntry`。
那里**不检查目录结构**，只检查 `ScanPolicy`（`:1741`）。

`InternalCheckScanPolicy`（`PolicyManagement.c:312`）的规则很简单：**只有当 `OC_SCAN_DEVICE_LOCK` / `OC_SCAN_FILE_SYSTEM_LOCK` 位置位时才校验**。

- `ScanPolicy = 0`（多数黑苹果配置）→ **全部放行**，连 `Microsoft Basic Data` 类型的分区也进候选
- 位被置上 → 只放行符合 `OC_SCAN_ALLOW_FS_*` / `OC_SCAN_ALLOW_DEVICE_*` 的卷

---

## 3. 名字和图标从哪来

`InternalDescribeBootEntry`（`BootEntryInfo.c:692`）按这个顺序取名，取到就停：

| 顺序 | 来源 | 位置 |
|---|---|---|
| 1 | `.contentDetails` 文件 | `:728` |
| 2 | `.disk_label.contentDetails` | `:730` |
| 3 | 卷标 | `:774` |
| 4 | 固定串 `Windows` / `macOS Installer` | `:759` / `:772` |

实测：macOS 根卷上**前两个文件都不存在**，所以菜单里真正显示的是**卷标**（`diskutil info /` 的 `Volume Name`，这台机器就是 `MAC`）。

条目**类型**（图标靠它）由**路径字符串**判定（`PolicyManagement.c:329` `OcGetBootDevicePathType`）：

| 路径特征 | 类型 |
|---|---|
| 含 `com.apple.recovery.boot` | Recovery（auxiliary，默认隐藏） |
| 含 `com.apple.recovery.boot` + Time Machine 标记 / `tmbootpicker.efi` | Time Machine（auxiliary） |
| 结尾 `boot.efi` | macOS |
| 结尾 `bootmgfw.efi` | Windows |
| 结尾 `BOOTX64.EFI` | **generic**（`IsGeneric=TRUE`，`:427-432`） |

---

## 4. 过滤与排序

**过滤**只有三类，全在 `AddBootEntry`（`:396` 起）里：

| 规则 | 效果 | 你的机器 |
|---|---|---|
| `Misc→Boot→HideAuxiliary` | 丢弃 Recovery / Time Machine / `Auxiliary` 标记项（`:460`、`:472`、`:497`） | `false` → **都显示** |
| 卷上 `.contentVisibility` 文件写 `Disabled` / `Auxiliary`（`:202`） | 单卷级隐藏，**不改 config** | 未使用 |
| DevicePath 全等去重（`:522`） | 同一条目出现两次只留一个 | — |

**排序**：`OcEnumerateEntries`（`:2507`）按「先卷链表、再卷内链表」两层嵌套展开。所以菜单顺序 = **卷的枚举顺序**（大致是固件返回句柄的顺序），不是按名字、不是按时间。

**默认项和菜单第一项无关。** 默认项 = `BootOrder` 里**第一条成功注册且非工具**的条目（`:363`、`:380`）。

---

## 5. 拿这套规则解释一台真实的机器

一台 `Misc→Entries` 和 `Misc→Tools` 都是空数组、`ScanPolicy=0`、`HideAuxiliary=false` 的 AMD 黑苹果，菜单是**纯自动扫**出来的：

| 菜单项 | 来源 | 说明 |
|---|---|---|
| `MAC` | `BootOrder` 的 `Boot0080` | APFS Preboot 里的 `boot.efi` |
| `Recovery` | APFS 容器内 Recovery 卷 | `com.apple.recovery.boot` |
| `Windows` | WINDOWS-EFI 卷 | `\EFI\Microsoft\Boot\bootmgfw.efi` |
| `ARCH-EFI`（generic） | ARCH-EFI 卷 | `\EFI\BOOT\BOOTX64.EFI` → GRUB |
| `MacOS-efi`（generic） | MacOS-efi 卷 | 就是 OC Bootstrap 自己，**选中会再进一次 OC** |
| `Reset NVRAM` | `ResetNvramEntry.efi` | 协议驱动提供 |

**rEFInd 不会出现。** 因为它的卷上既没有 `bootmgfw.efi`，也没有 `\EFI\BOOT\BOOTX64.EFI`，更不在 `BootOrder` 里 —— 三条路全断。

想让 rEFInd 出现在 OC 菜单里，只有两条：

1. 在 REFIND 卷放一份 `\EFI\BOOT\BOOTX64.EFI`
2. 让固件 `BootOrder` 里有一条指向 `\EFI\refind\refind_x64.efi`（这条在 macOS 侧写不了，见下）

---

## 6. 为什么不能直接读 OC 算出来的那份菜单

**能算的那一步只发生在 UEFI 里。** `OcScanForBootEntries` 依赖 `LocateHandleBuffer` 和 Apple Boot Policy protocol，结果是一张**内存链表**，`OcEnumerateEntries` 吃完就释放，重启即丢。

OC 没有 IPC、没有共享内存、没有「导出菜单」接口，也**从不把启动项写文件**（整个仓库 grep `SaveBootEntry` / `ExportMenu` / `OcStorageWriteFile` 在启动管理里零命中）。它的持久化只有一条路：写 NVRAM。

所以在 macOS / Linux / Windows 里，你看到的只能是**输入**，不是**结果**：

| OC 的依赖 | 主系统里能不能读 | 依据 |
|---|---|---|
| 有哪些卷 | ✅ 更准 | `diskutil list` / `lsblk`，还带 `Internal` 标志 |
| 卷上的 `boot.efi` / `bootmgfw.efi` / `BOOTX64.EFI` | ✅ 直接 stat | ESP 挂上就能看 |
| APFS Preboot 的 blessed 位置 | ✅ | `/System/Volumes/Preboot/<UUID>/System/Library/CoreServices/` |
| APFS 卷角色（Preboot / Recovery） | ✅ | `diskutil apfs list` |
| `BootOrder` / `Boot####` | ✅ 读（❌ 写） | macOS 侧读得到，写被系统白名单挡 |
| **固件到底枚举到哪几个卷** | ❌ | 只能 UEFI 里知道（APFS 驱动没加载、USB 未初始化都会差） |
| 协议驱动动态产生的条目 | ⚠️ 可推算 | `OpenLinuxBoot` 的规则是确定性的 |

这就是为什么「预演」的准确率天花板在 **90% 左右**：文件、卷、NVRAM 都能读准，**唯一猜不准的是这块固件在那一刻能不能真的枚举到某个卷**。

---

## 7. AnyBoot 的做法：「预演 OC 菜单」

OpenCore 卡片上常驻一块 **预演 OC 菜单**（默认展开，标题可点收起，收起状态会记住）。它按上面第 1–5 节的规则，在**不重启、不写盘、不碰 NVRAM** 的前提下，把「下次进 OC 大概会看到什么」先算出来：

每条会给出：

| 字段 | 含义 |
|---|---|
| 标题 | 按第 3 节的取名规则算出来的名字 |
| 类型 | macOS / Recovery / Windows / generic / 工具 |
| 来源 | `BootOrder` / bless 文件 / bless 文件夹 / 预定义路径 / Preboot / 协议驱动 |
| 所在卷与文件路径 | 便于你直接去核对 |
| 会不会显示 | `HideAuxiliary` 会不会把它藏掉、`.contentVisibility` 有没有禁用 |
| 默认项标记 | 是不是**启动顺序表里第一条非工具项**（读的是 `OCBtOrder` 还是固件 `BootOrder`，由 `RequestBootVarRouting` 决定，行上的来源标签照实写） |

三项判定细节值得单独说，因为这里的错法都很隐蔽：

- **默认项按启动顺序走，不按扫描顺序走。** 预演先枚举卷（顺序与启动无关），再拿 `BootOrder` 去对上号；默认项必须是后者里的第一条。曾经写成「在枚举结果里找第一条来源是 bootorder 的」，等于拿卷的遍历顺序当启动顺序——实测 `OCBtOrder = [0x82 WIN, 0x81 ARCH, 0x80 MAC]` 时界面把 MAC 标成了默认项，而真默认项那一行反而挂着「设为默认」按钮，点下去只是把已经在最前的项再挪一次。
- **同名文件要靠分区 GUID 区分。** 多块 ESP 上都有 `\EFI\BOOT\BOOTX64.EFI` 是常态，只比路径会把 A 卷的启动项算到 B 卷头上。因此对不上号时会读 `Boot####` 里的 HardDrive 节点（分区 GUID）再确认一次。但它只用来「挑」不用来「否」：APFS 卷的 `DiskUUID` 是卷 UUID，与启动项里记的物理分区 GUID 天生不等，拿它当必要条件会把正确的匹配也否掉（真踩过，`OCBt0080` 因此变成「文件看不到」的孤儿项）。
- **已经是默认项的那一行不给「设为默认」按钮**，而是行尾显示「✓ 已是默认」。缺按钮容易被读成「这一行选不了」或「按钮丢了」，而真相是它就在顺序最前面。

外加三类可以直接行动的问题项：

- **rEFInd 不在这张表里** → 提示「要让 rEFInd 出现在 OC 菜单，需要放 `BOOTX64.EFI` 或改固件顺序」，并给出具体卷
- **Linux 只是泛型项** → 提示「装 `OpenLinuxBoot.efi` 才能得到独立内核条目」，并检查 `UEFI→Drivers` 里到底有没有它
- **配置项自相矛盾** → 例如 `HideAuxiliary=true` 但你在找 Recovery；`ScanPolicy` 置了位导致某类卷被跳过

它是**纯读**的：只读 ESP 上的文件和 `diskutil` 的输出，不会改任何东西，也不需要管理员密码。

### 7.1 顺手把它设成默认项（= 进 OC 菜单按 Ctrl+Enter）

预演出来的每一行右侧还有一个 **设为默认** 按钮。它做的事和你在 OC 菜单里对某个系统按 <kbd>Ctrl</kbd>+<kbd>Enter</kbd> 完全等价：

| | OC 自己的做法 | AnyBoot 的做法 |
|---|---|---|
| 容器 | `BootOrder` + `Boot0080`（固件全局变量） | `OCBtOrder` + `OCBt0080`（OC 私有命名空间） |
| macOS 能不能写 | ❌ 系统白名单拒绝，root 和 Apple 签名的 `bless` 都不行 | ✅ 换个 GUID 的普通变量，一次管理员授权就能写 |
| 槽位 | 永远复用 `0x0080`，**上一条默认项会被覆盖** | 已有槽只调顺序；没有就新建一个空闲编号，原默认项不动 |
| 生效时机 | 下次启动 | 下次启动 |

为什么这条路成立：`UEFI→Quirks→RequestBootVarRouting=true` 时，OC 把「启动变量」整体搬到自己的 GUID
（`4D1FDA02-38C7-4A6A-9CC6-4BCCA8B30102`）下，变量名加 `OCBt` 前缀（`OCBtOrder` / `OCBt0080` / `OCBtNext`）。
OC 读菜单、读默认项读的都是这一组，而 macOS 只拦名字叫 `Boot*` 的变量 —— 于是这一组可以写。

按钮出现的条件（写死的规则，列表里没有的一律不给按钮，避免点了必然失败）：

- 这一项**已经在固件启动项里**（只改顺序，零风险）；
- 或者这一项是**卷上一个明确的 `.efi` 文件**（Windows 的 `bootmgfw.efi`、泛型 `BOOTX64.EFI`，按该分区的
  HardDrive 节点自己造一条）；
- **不给**：工具项（`Reset NVRAM`）、协议驱动生成的条目（`OpenLinuxBoot` 的 `.conf` 内嵌 OC 自定义设备路径，
  凭路径造不出来）、OC 自己的 Bootstrap 入口（设成默认等于每次启动都先回菜单）、已经是默认项的那一条。

写法上有三道保险：动 NVRAM 前把两套命名空间整份备份到 `EFI/anyboot/nvram-backup/`（留 10 份）；写完**回读校验**
（顺序首位 + 槽位内容），固件或系统静默丢弃会当场报出来；目标已经是默认项时直接返回「无需改动」，不备份、不写盘。

> ⚠️ 前提是 `RequestBootVarRouting=true`。它为 `false` 时 OC 读的是固件全局 `BootOrder`，而 macOS 不允许写，
> 卡片上会写明这一点并让按钮不出现 —— 改这一个 quirk 就能启用。

---

## 8. 需要 100% 准确时：让 OC 自己把菜单写进日志

OC 有一份结构化日志，每行格式固定，能直接解析：

```
OCB: Found 8 potentially bootable filesystems
OCB: Adding fs 67A1... (E:0|L:0|P:Success) - PciRoot(0x0)/.../HD(5,GPT,...)
OCB: Building entry from Boot0080
OCB: Adding entry type (T:1|F:0|G:0) - ...\System\Library\CoreServices\boot.efi
```

`Found N potentially bootable filesystems` 这一行，正是主系统里永远猜不准的那个数。**要拿到它，得改 3 项配置然后重启一次**：

| 项 | 要改成 | 为什么 |
|---|---|---|
| `Misc→Debug→Target` | `67` | 已含 `BIT6 = 文件日志`，一般不用动 |
| `Misc→Debug→DisplayLevel` | `2147483714`（`0x80000042`） | 默认只含 `ERROR\|WARN`，而 `OCB:` 全是 `DEBUG_INFO(0x40)` → **根本没输出** |
| `Misc→Debug→AppleDebug` | `false` | 否则 `boot.efi` 的 256KB 输出会灌进**同一个文件**，把 OC 段冲掉 |

代价：之后不再保存 `boot.efi` 的日志，排查 macOS 启动问题时少一样工具。所以建议只在想核对时开一次，对完关掉。

完整推导、位定义、实测日志证据见
[`boot-menu-discovery-internals.md`](boot-menu-discovery-internals.md) 第 7 节。

---

## 9. 能力边界

| | macOS | Linux / Windows |
|---|---|---|
| 预演 OC 菜单 | ✅ 纯读 | ✅ 纯读 |
| 读真实 `BootOrder` | ⚠️ 只读得到固件真值里的一小部分 | ✅ |
| 写 `BootNext` 让下次进 OC | ❌ 系统白名单拒绝（root 也不行） | ✅ |
| 设置 OC 默认项 | ✅ 走 OC 私有命名空间 `OCBt*`（见 7.1）；`RequestBootVarRouting=false` 时退回 ❌ | ✅（Windows 还要求该项已在 `BootOrder` 里，见 9.1 ②） |

macOS 上「读不全 / 写不了 `Boot*`」不是权限不够，是系统按变量名精确拦截 `Boot*` 与 `efi-boot-device`。
而 `OCBt*` 只是另一个 GUID 下的普通变量，能写 —— 这正是「设默认项」在 macOS 侧仍然可用的原因。
实测证据（`bless` 一样被拒、`OCBt*` 反而可写）见
[`boot-menu-discovery-internals.md`](boot-menu-discovery-internals.md) 第 6 节。

### 9.1 实现层：同一套规则，四处分叉

预演规则、`EFI_LOAD_OPTION` 的构造（`boot_entry.rs`）、`BootOrder` 的解析全是平台无关的纯函数，
三端共用一份、单测直接覆盖。真正按 `target_os` 分叉的只有四件事：

| | macOS | Linux | Windows |
|---|---|---|---|
| 读 NVRAM | `/usr/sbin/nvram guid:name` | efivarfs 文件（4 字节属性头 + 值） | `GetFirmwareEnvironmentVariableW` |
| 写 NVRAM | `nvram`（`osascript` 提权），只写得动 `OCBt*` | 直写，失败转 `pkexec` 暂存安装 | `SetFirmwareEnvironmentVariableW`（**要进程本身是管理员**） |
| 找 ESP | `/Volumes` + `/` | `/efi`、`/boot/efi`、`/boot`、`/esp` | `Get-Partition` 按 GPT 类型 `c12a7328-…` 过滤 |
| 取分区号 / 起始 LBA / GPT GUID | `diskutil info -plist` | `findmnt` + `lsblk` | ❌ 未实现（见下） |

三处差异会带来实际后果，用之前得知道：

**① Windows 的 ESP 天生没有盘符。** 不分配盘符的话，它在 Win32 文件 API 里根本不存在，
按 `A:`–`Z:` 扫一个都扫不到。所以 `candidate_roots` 改成先用 `Get-Partition` 列出所有
`GptType` 为 ESP 的分区，再取其挂载根。一块都没有盘符时，OpenCore 卡片会**照实说明**
（「磁盘 0 分区 1（260 MB）没有盘符」）并给一个「分配盘符」按钮 —— 它调
`Add-PartitionAccessPath -AssignDriveLetter`，经 `Start-Process -Verb RunAs` 弹一次 UAC。
提权子进程的退出码拿不到，所以成败不靠返回码判断：**挂完重新枚举一遍，真拿到盘符才算成**
（与 macOS 用 `osascript` 提权后回读校验同一思路）。

**② Windows 上不能用「自己造一条启动项」这条兜底。** `boot_entry` 需要分区的起始 LBA 与
GPT GUID，Windows 侧还没写这段（`partition_info_platform` 直接返回错误）。所以 Windows 上
「设为默认」只有两条路能用：目标已在 `BootOrder` 里（只重排），或者能原样抄一条现成的值。
路都能走通的前提是**以管理员身份运行**——非管理员进程连 `BootCurrent` 都读不到
（`GetFirmwareEnvironmentVariableW` 要 `SeSystemEnvironmentPrivilege`）。

> 这一点以前会被错判成「启动列表被引导器托管了」。现在 `nvram::BootVarsView` 把
> 「变量不存在」和「没权限读」分开，文案也分开：前者让你去看 OC 配置，后者让你提权重开。
> 混在一起的后果是——按提示查半天 OC，而真因是权限。

**③ 卷标在不同平台不一样。** OC 显示的卷名来自 macOS 写在卷上的 `.contentDetails`；
Linux / Windows 上这两个文件通常不存在，于是回退成挂载点名（`efi`）或盘符（`S:\`）。
功能不受影响（「设为默认」按卷标反查时会重新枚举），但卡片里显示的名字会和 OC 菜单里的对不上。

### 9.2 免密模式：装一次，之后不再弹密码

「设为默认」要写 `OCBtOrder` + 一条 `OCBt####`。macOS 上**非 root 写 NVRAM 在 iokit 层就被拒**
（实测 `nvram` 报 `(iokit/common) general error`，那是被伪装成通用错误的权限拒绝），
所以默认走的是 `osascript … with administrator privileges`：每点一次，输一次密码。

要去掉这个密码框，只能让一个 **root 进程**替 App 写。实现是那张卡片里的「免密模式 / 开启」：

```
App  ──写 JSON──▶  /var/spool/anyboot/in/   (0733，用户可新建、不可列举)
                        │  kqueue 触发 WatchPaths
                        ▼
/ Library/LaunchDaemons/com.example.anyboot.helper.plist
     └─ 唤起 /Library/PrivilegedHelperTools/com.example.anyboot.helper --run-inbox
                        │  校验 → nvram guid:name=%XX…
                        ▼
              /var/spool/anyboot/out/<同名>.json   → App 轮询取结果
```

几个刻意的选择：

- **用文件信箱，不用 socket。** launchd 的 `WatchPaths` 盯着 `in/` 目录就够了：
  没有常驻进程、不占端口，也不存在「连接断了 / 半包 / 重连」这些问题。请求先写进
  `tmp/` 再 `rename` 进 `in/`，助手永远读不到写了一半的 JSON。
- **助手只在有请求时被唤起**（`RunAtLoad=false`），处理完就退。
- **只认 OpenCore 私有命名空间**：`OCBtOrder` / `OCBtNext` / `OCBt####`，GUID 必须是
  `4D1FDA02-…`；值是十六进制串、≤512 字节、单次 ≤8 条；**不执行 shell、不接受路径**。
  越界的请求直接回错误，不会「先试试看」。
- **失败分两类**。`Unavailable`（没装、launchd 没加载、4 秒没响应）→ 退回密码框，功能
  不受影响；`Rejected`（变量名越界、`nvram` 报错）→ **直接报错，不再弹密码**。
  搞混这两个的后果是「输一次密码、再失败一次」，而真正的原因被埋掉。
- **超时要撤回请求**。助手 4 秒没响应就把 `in/` 里的请求删掉，否则它十分钟后才醒来，
  会在用户完全不知情的情况下把启动项改掉 —— 那种「鬼写」比失败更难排查。

代价必须一起说清楚：装上以后，**任何以当前用户身份运行的进程**都可以让它改 OpenCore 的
默认启动项（最坏后果就是「默认项被改」，拿不到 root、执行不了任意命令）；`/Library` 里还会
多一份 ~25 MB 的副本（装的是本程序的拷贝，用 `--run-inbox` 进入助手模式，所以不存在版本错配）。
不想承担这个交换，就保持关闭 —— 关着的时候功能和以前完全一样，只是每次要密码。

- **变量名一个字母都不许改**。UEFI 变量名区分大小写，`OCBTORDER` 是**另一个变量**：
  助手早期把名字统一转成大写，于是 `OCBtOrder` 一个字节没动、`nvram` 却回 `ok`，
  界面只剩「写进去了，但回读不在首位」。现在拼法不符直接拒，不做任何大小写归一化。

**更新 App 之后**，「免密模式」那一行会自己变成「旧版本」并给出「重装」：`/Library` 里那份是
上次安装时的可执行文件，不会跟着 App 一起更新。判据是两份文件的 **sha256**（按长度 + mtime
缓存，文件没变时每次快照只多一次 `metadata`）。**不能只比大小**：可执行文件有页对齐与代码签名，
改几十字节代码后长度常常一字节不差，而「助手是旧的」这次的后果正是上面那条 ——
静默写进另一个变量，判断不出来就等于没修。

> 为什么不用 `SMAppService` / `SMJobBless`：这两个「正规」通道要求 App 与助手都用**带 Team ID
> 的证书**签名。本项目是 ad-hoc 签名（`Signature=adhoc`、无 `TeamIdentifier`），注册必然失败。
> 别把这段改成 SMAppService 除非先解决签名身份，否则会出现「装不上、但看起来什么都没发生」。

---

## 10. Linux 分区内核扫描（2026-10-29：ext4 裸读接通后补齐的最后一块）

rEFInd 装上 ext4 驱动后，卷循环会进 Linux 分区把内核捡出来（`ScanLoaderDir`，
`scan.c:907`：根目录 + `also_scan_dirs` 默认含 `boot`；前缀 `vmlinuz*/bzImage*/
kernel*` 由 `scan_all_linux_kernels` + `linux_prefixes` 决定；initrd 只认
`init*,booster*` 前缀再按版本号配对，`linux.c:102`）。AnyBoot 的模拟扫描
（refind_menu.rs）一直只有 ESP 路径扫描——macOS 上 ext4 分区没有挂载点，
`oc_menu::volumes()` 根本不返回它们，Linux 内核条目因此名存实亡。

ext4_raw.rs（自研只读 ext4 解析）接通后，这条路补齐了：

- **数据**：`linux_grub::probe_raw`（helper / 提权子进程 / Linux 本机三条路径
  共用）在原有的每分区循环里顺带做 `list_dir("")` + `list_dir("boot")`
  （各 cap 512 文件，只留文件不留目录）、读 `refind_linux.conf`，装进
  `PartReport.dir_listings`（serde default——旧助手回执没有字段就解析成空，
  协议 v2 不用 bump）。
- **缓存**：全量 `ProbePayload` 存 `AppState.linux_probe_cache`（`store()`），
  模拟扫描复用它，绝不为 Linux 内核单独再探一轮（macOS 每轮 4~5 秒）。
- **取数**：`linux_grub::parts_for_sim`（手动「重新模拟」用：缓存 → 没有且
  助手在位就免密现探，**绝不回落弹授权**）与 `refind_menu::
  linux_grub_parts_from_cache`（启动扫描 / 切换前校验用：只读缓存）。
- **消费**：`refind_menu::linux_kernel_entries` —— 与 `scan_volume` ③ 同一套
  语义（前缀匹配、mtime 降序、fold 进第一个内核的子菜单、refind_linux.conf
  展开变体、dont_scan_files / HiddenTags 隐藏过滤），外加两条 rEFInd 细节：
  `vmlinuz-0-rescue` 强制沉底（`scan.c:698`，否则折叠后它变成默认启动）；
  没带 refind_linux.conf 的后续内核 rEFInd 会静默丢弃，这里照做但聚合成一条
  info issue。`source: "ext4"`，前端标签「ext4 分区」。
- **边界**：只扫根目录与 `/boot` 两层（覆盖 Arch/Debian/Fedora 布局）；
  Linux 分区里的 `EFI\*` 子目录引导器不在清单内（那些多半就是 ESP 上已有的
  同一批 loader）。Windows 端仍未接 ext4，issue 会如实说明。

单测 5 项（折叠 / rescue 沉底 / 无 conf 丢弃 / .efi.signed 去重 / 隐藏规则），
`live_helper_probe` 兼容旧助手（清单字段全空时降级为提示，重装助手后自动
升级为端到端检查）。**启用前提：在 OC 卡片把免密助手重装一次**（助手二进制
= App 本体，重装才会带上新的清单采集代码）。

## 相关文档

- 详细原理与探索过程（逐函数源码、实机验证、被推翻的结论）：[`boot-menu-discovery-internals.md`](boot-menu-discovery-internals.md)
- 三引导器通道的模块设计与数据结构：[`boot-channel-技术细节设计.md`](boot-channel-技术细节设计.md)
- 通道扩展的机制论证：[`boot-channel-扩展可行性方案.md`](boot-channel-扩展可行性方案.md)
