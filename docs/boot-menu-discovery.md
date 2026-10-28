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
| 默认项标记 | 是不是 `BootOrder` 里第一条成功注册的 |

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
| 设置 OC 默认项 | ✅ 走 OC 私有命名空间 `OCBt*`（见 7.1）；`RequestBootVarRouting=false` 时退回 ❌ | ✅ |

macOS 上「读不全 / 写不了 `Boot*`」不是权限不够，是系统按变量名精确拦截 `Boot*` 与 `efi-boot-device`。
而 `OCBt*` 只是另一个 GUID 下的普通变量，能写 —— 这正是「设默认项」在 macOS 侧仍然可用的原因。
实测证据（`bless` 一样被拒、`OCBt*` 反而可写）见
[`boot-menu-discovery-internals.md`](boot-menu-discovery-internals.md) 第 6 节。

---

## 相关文档

- 详细原理与探索过程（逐函数源码、实机验证、被推翻的结论）：[`boot-menu-discovery-internals.md`](boot-menu-discovery-internals.md)
- 三引导器通道的模块设计与数据结构：[`boot-channel-技术细节设计.md`](boot-channel-技术细节设计.md)
- 通道扩展的机制论证：[`boot-channel-扩展可行性方案.md`](boot-channel-扩展可行性方案.md)
