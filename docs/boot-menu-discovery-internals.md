# AnyBoot · OpenCore 启动菜单发现机制（源码级原理与探索过程）

> 上游文档：[`boot-menu-discovery.md`](boot-menu-discovery.md)（面向使用者的原理与功能说明）。
> 本文是它的底稿：逐函数源码追踪、实机验证记录、以及**被推翻的几版错误结论**。
>
> 阅读对象：要改 AnyBoot 的 `oc_menu` 预演器、或要判断「这条差异到底是 OC 的问题还是预演器的问题」的人。

---

## 0. 阅读范围

| 项目 | 值 |
|---|---|
| 仓库 | `acidanthera/OpenCorePkg`（`git clone --depth 1`） |
| 主要文件 | `Library/OcBootManagementLib/*`、`Library/OcAppleBootPolicyLib/*`、`Platform/OpenRuntime/UefiRuntimeServices.c`、`Platform/OpenLinuxBoot/OpenLinuxBoot.c`、`Library/OcLogAggregatorLib/OcLog.c` |
| 实机 | AMD 黑苹果（`MacPro7,1` 机型标识），OC 在 `disk0s5`，ESP 卷名 `MacOS-efi` |

行号一律写 `文件:行`，供直接跳转核对。

---

## 1. 全景：一次「进 OC」里发生了什么

```
固件
 └─ 取 BootOrder[0]（或 BootNext 优先）→ 加载 OpenCore.efi
     ├─ OcMainBoot → OcMiscBoot(..., CustomBootGuid = UEFI→Quirks→RequestBootVarRouting)   OpenCore.c:178-186
     ├─ OcLoadDrivers（OpenRuntime.efi / OpenCanopy.efi / OpenLinuxBoot.efi …）
     └─ OcBootManagementLib 主循环
         ├─ OcScanForBootEntries(Context)                    BootEntryManagement.c:2103
         │   ├─ BuildFileSystemList(Context)                 BootEntryManagement.c:1945
         │   │   └─ LocateHandleBuffer(ByProtocol, SimpleFileSystem)   :1977
         │   │       └─ 逐句柄 AddFileSystemEntry            :1741（只查 ScanPolicy）
         │   ├─ 遍历 BootOrder → AddBootEntryFromBootOption  :1224 / 调用点 :2164
         │   ├─ 遍历每个卷 → AddBootEntryFromBless           :912  / 调用点 :2213（另 :1703 单卷）
         │   └─ 遍历协议句柄 → OcAddEntriesFromBootEntryProtocol  BootEntryProtocol.c:239
         ├─ OcEnumerateEntries(Context)                      :2507   ← 菜单渲染顺序
         └─ Picker（BuiltinPicker / OpenCanopy）
             └─ 按键 → OcModifiers → 选中条目 SetDefault → OcSetDefaultBootEntry   DefaultEntryChoice.c:772
```

三个关键事实：

1. **`OcScanForBootEntries` 不遍历目录**，只在「每个已枚举的卷」上试固定几个路径。
2. 扫描结果是一张内存链表，`OcEnumerateEntries` 消费完即释放，**没有落盘、没有导出接口**。
3. 菜单的**默认项**与**第一项**是两个独立概念（见 §6）。

---

## 2. 卷枚举：`BuildFileSystemList`

```c
// BootEntryManagement.c:1977
Status = gBS->LocateHandleBuffer (
                ByProtocol,
                &gEfiSimpleFileSystemProtocolGuid,
                NULL,
                &HandleCount,
                &Handles
                );
```

拿到所有挂载了 SimpleFileSystem 的句柄后，逐个 `AddFileSystemEntry`，其中**只做一件事**：

```c
// BootEntryManagement.c:1741
if (!InternalCheckScanPolicy (Context->PickerContext, DevicePath)) {
  return EFI_SECURITY_VIOLATION;   // 跳过该卷
}
```

`InternalCheckScanPolicy`（`PolicyManagement.c:312`）的实际判定：

```c
if (((Policy & OC_SCAN_DEVICE_LOCK) == 0) && ((Policy & OC_SCAN_FILE_SYSTEM_LOCK) == 0)) {
  return TRUE;      // 两个 LOCK 位都没置 → 全部放行
}
```

**结论**：`ScanPolicy=0` 时，任何 FAT/NTFS/APFS 卷都进候选，包括类型为 `Microsoft Basic Data` 的 ESP。这一条对预演器很重要——**不要把「卷类型」当成过滤依据**。

---

## 3. 来源 ①：`BootOrder` 里的 `Boot####`

```c
// BootEntryManagement.c:1224  AddBootEntryFromBootOption
OcGetBootOptionData  →  解析 EFI_LOAD_OPTION
```

要点：

- **描述字符串被丢弃**（`:1262-1265`），`OptionalData` 也丢弃 → OC 菜单里的名字不是从 NVRAM 描述来的，而是后面重新算（§5）。
- 短格式 Boot 项（`ShortFormDevicePath`）先展开：`ExpandShortFormBootPath`（`:51`）。
- 按 HardDrive 节点（PartitionNumber + PartitionGuid）在卷链表里找对应卷，找不到 → 该条目**丢弃**（卷不在了）。
- 找到卷后把 FilePath 节点拼上，得到完整设备路径。

---

## 4. 来源 ②：Apple bless 扫描

```c
// BootEntryManagement.c:912  AddBootEntryFromBless
```

固定三级降级，**命中即停**：

| 级 | 读什么 | GUID / 常量 | 行 |
|---|---|---|---|
| 1 | blessed **文件** | `gAppleBlessedSystemFileInfoGuid` | `:241` |
| 2 | blessed **文件夹** | `gAppleBlessedSystemFolderInfoGuid` | `:278` |
| 3 | 预定义路径表 | `gAppleBootPolicyPredefinedPaths` | `:943-944` |

### 4.1 预定义路径表只有 3 条

```c
// OcAppleBootPolicyLib.c:74-78
GLOBAL_REMOVE_IF_UNREFERENCED CONST CHAR16  *gAppleBootPolicyPredefinedPaths[] = {
  APPLE_BOOTER_DEFAULT_FILE_NAME,     // \System\Library\CoreServices\boot.efi
  MS_BOOTER_DEFAULT_FILE_NAME,        // \EFI\Microsoft\Boot\bootmgfw.efi
  EFI_REMOVABLE_MEDIA_FILE_NAME       // \EFI\BOOT\BOOTX64.EFI
};
```

还有一行容易被忽略但很重要的常量：

```c
// OcAppleBootPolicyLib.c:83
GLOBAL_REMOVE_IF_UNREFERENCED CONST UINTN gAppleBootPolicyCoreNumPredefinedPaths = 2;
```

即：**前 2 条是「核心」，第 3 条 `BOOTX64.EFI` 是兜底**。源码注释（`:60-72`）解释了为什么不放 Linux：

> *Linux and related entries are not present here, because they have fine working software for boot management and do not use BOOTx64.efi in the first place.*

并且说明了为什么要加 `BOOTX64` 这条：

> *This resolves a problem when Windows installer does not replace our BOOTx64.efi file with Windows file and then NVRAM reset or Boot Camp software reboot to macOS results in the removal of the Windows boot entry from NVRAM making Windows disappear from the list of OpenCore entries without BlessOverride.*

**推论（对预演器是硬规则）**：卷上只要存在 `\EFI\BOOT\BOOTX64.EFI`，就一定会出现一条 `generic` 条目 —— 哪怕它是 OC Bootstrap 自己。

### 4.2 APFS：Preboot 与容器内多卷

`OcAppleBootPolicyLib.c:987-1011` 起：

1. 若卷角色是 **Preboot**，先按普通流程取 blessed；
2. 再对**同一 APFS 容器内每个卷 UUID** 试
   `Preboot:\{UUID}\System\Library\CoreServices\boot.efi`
   （`InternalGetBooterFromApfsPredefinedPathList`，实现在 `:565` 起，调用点 `:1052-1053`、`:1210-1211`、`:1462-1463`）

这就是「一个 APFS 容器里装了多个 macOS，OC 菜单能全部列出来」的机制。
对预演器的影响：macOS 条目**不能只按当前启动卷找**，要按容器内 UUID 全扫。

---

## 5. 名字、类型、图标

### 5.1 名字

```c
// BootEntryInfo.c:692  InternalDescribeBootEntry
```

| 顺序 | 来源 | 行 |
|---|---|---|
| 1 | `.contentDetails` | `:728` |
| 2 | `.disk_label.contentDetails` | `:730` |
| 3 | 固定串（`Windows` / `macOS Installer`） | `:759` / `:772` |
| 4 | 卷标 | `:774` |

### 5.2 类型（决定图标）

```c
// PolicyManagement.c:329  OcGetBootDevicePathType
```

路径字符串匹配，**只看文件名，不看卷**：

| 条件 | 类型 | 行 |
|---|---|---|
| 设备路径是 `MEDIA_PIWG_FW_FILE_DP` | `OC_BOOT_UNKNOWN`（直接返回，不探测） | `:353-356` |
| 含 `com.apple.recovery.boot` | `OC_BOOT_APPLE_RECOVERY` | `:373-376` |
| 含 `tmbootpicker.efi` / Time Machine 标记 | `OC_BOOT_APPLE_TIME_MACHINE` | 后续分支 |
| 结尾 `boot.efi` | `OC_BOOT_APPLE_OS` | 后续分支 |
| 结尾 `bootmgfw.efi` | `OC_BOOT_WINDOWS` | 后续分支 |
| 结尾 `BOOTX64.EFI` | `IsGeneric = TRUE` | `:427-432` |

---

## 6. 过滤、排序、默认项

### 6.1 过滤（`AddBootEntry`，`:396` 起）

```c
// BootEntryManagement.c:460
if (BootContext->PickerContext->HideAuxiliary && (EntryType == OC_BOOT_APPLE_RECOVERY)) { … }
// :472
if (BootContext->PickerContext->HideAuxiliary && (EntryType == OC_BOOT_APPLE_TIME_MACHINE)) { … }
// :497
if ((Visibility == BootEntryAuxiliary) && BootContext->PickerContext->HideAuxiliary) { … }
```

`.contentVisibility` 文件（`ReadEntryVisibility`，`:202`）可写 `Disabled` / `Auxiliary`，是**唯一不改 config 就能藏单个条目的手段**。

去重：DevicePath 全等（`:522`）。

### 6.2 排序

`OcEnumerateEntries`（`:2507`）双层嵌套：先按卷链表顺序，再按卷内条目链表顺序。

**卷的顺序不是按名字排的**，取决于固件返回句柄的顺序 —— 这就是为什么预演器能算「大概有哪些」，但**算不出确定的显示次序**。

### 6.3 默认项

```c
// BootEntryManagement.c:363 / :380
BootContext->DefaultEntry = BootEntry;
```

条件是「按 BootOrder 顺序遍历中，第一条注册成功且不是工具/system 类型的条目」。
**与菜单里的位置无关**，所以「默认项不在第一行」是正常现象。

---

## 7. 「记住上次选择」的真实机制：`OcSetDefaultBootEntry`

Ctrl+Enter 背后没有任何「记忆文件」，**就是改固件 Boot 变量**。

### 7.1 按键 → 标记

```c
// HotKeySupport.c:318
if (Context->AllowSetDefault) {
  ValidBootModifiers |= APPLE_MODIFIERS_CONTROL;
}
// :341
PickerKeyInfo->OcModifiers |= OC_MODIFIERS_SET_DEFAULT;
// :348-351  （'=' 键为备用，适合修饰键失灵的场景）
```

`Misc→Security→AllowSetDefault=false` 时 Ctrl 不在白名单，**整个功能等于关闭**。

### 7.2 标记 → 条目

```c
// BuiltinPicker.c:797 / :863（文本 picker）
(*ChosenBootEntry)->SetDefault = ((PickerKeyInfo.OcModifiers & OC_MODIFIERS_SET_DEFAULT) != 0);
// OpenCanopy/Views/BootPicker.c:458（图形 picker）同一行逻辑
```

文本 picker 里它显示成 `+`（`BuiltinPicker.c:247`）。

### 7.3 条目 → 写变量

`OcSetDefaultBootEntry`（`DefaultEntryChoice.c:772`）全程约 400 行，分四段：

| 阶段 | 行 | 做什么 |
|---|---|---|
| 门控 | `:816` | `!AllowSetDefault` → 返回 `EFI_SECURITY_VIOLATION`，**静默失败** |
| 选命名空间 | `:841-849` | `CustomBootGuid` 真 → `4D1FDA02-38C7-4A6A-9CC6-4BCCA8B30102` + `OCBt0080` / `OCBtOrder`；假 → 全局 GUID + `Boot0080` / `BootOrder` |
| 匹配已有项 | `:862-934` | 遍历 BootOrder，按设备路径逐条比对（HD+FilePath / OC 自定义节点 / entry-protocol 节点） |
| 没匹配上 | `:936-1103` | **现场构造 `EFI_LOAD_OPTION`**（属性 `NV\|BS\|RT`，描述=条目名，FilePath=条目设备路径）写进 **`0080` 这一个槽** |
| 重排 | `:1112-1147` | 把选中编号与 `BootOrder[0]` 交换；原本不在列表则新建 `[0x80, …]` 插最前 |
| 写回 | `:1149` | `SetVariable(BootOrderName, …)` |

`CustomBootGuid` 的实参来自配置：

```c
// Application/OpenCore/OpenCore.c:178-186
OcMiscBoot (
  &mOpenCoreStorage, &mOpenCoreConfiguration, Privilege, OcStartImage,
  mOpenCoreConfiguration.Uefi.Quirks.RequestBootVarRouting,   // ← CustomBootGuid
  mStorageHandle
  );
```

**设计缺陷（OC 自己的）**：只用 `0x80` 一个槽。所以 Ctrl+Enter 一次，上一条默认项的 load option 就被覆盖 —— 这是「设了默认以后原来那个系统从列表消失」的根因。

### 7.4 开局怎么读回来

```c
// DefaultEntryChoice.c:578  OcGetBootOrder
//   :686-689  有 BootNext → 插到顺序最前
//   :765      InternalClearNextVariables()  用完立刻删
```

所以 `BootNext` 天然是「只下一次」的语义，不需要额外清理。

---

## 8. 实机验证：macOS 侧能看到什么、能写什么

### 8.1 现象

非 root 与 root（`osascript … with administrator privileges`）读到的结果**完全一样**：

```text
BootCurrent        → 读不到（真实 UEFI 不可能没有它）
BootOrder          → %80%00
Boot0080           → %01%00%00%00%14%01M%00A%00C%00…    描述 "MAC"
Boot0081           → 不存在
efi-boot-device    → 不存在
```

### 8.2 先怀疑「OC 重定向」，验证后部分成立

`Platform/OpenRuntime/UefiRuntimeServices.c` 里有 `BootVariableRedirect` 机制：`RequestBootVarRouting=true` 时，运行时对 `Boot*` 的读写被改到 OC 私有前缀 `OCBt`。

实测两个命名空间：

```text
4D1FDA02-…:OCBtOrder = %80%00
4D1FDA02-…:OCBt0080  = %01%00%00%00%14%01M%00A%00C%00…
8be4df61-…:BootOrder = %80%00
8be4df61-…:Boot0080  = %01%00%00%00%14%01M%00A%00C%00…
```

**逐字节相同** —— 但**这不构成「重定向生效」的证据**，因为两边本来就可以恰好一样。

判别性实验：写一个**固件里根本不存在**的 `OCBt0081`，再看全局 `Boot0081` 是否出现。

```text
写 4D1FDA02-…:OCBt0081 → 成功，回读得到
读 8be4df61-…:Boot0081 → 仍然 not found
```

**结论：读取重定向在这台机器上并未生效**（`OpenRuntime.efi` 确实加载了，但重定向没作用到读取路径）。所以 macOS 看到的 `BootOrder` 就是固件真值，只是真值里本来就只有 `0x0080` 一条。

这一条修正了本文档早期版本的说法，也修正了主文档里「列表被引导器托管」的措辞。

### 8.3 写入能力按变量名精确拦截

分类测试（root 执行）：

| 目标 | 结果 |
|---|---|
| `anyboot-probe`（普通变量） | ✅ 写入 / 读回 / 删除 |
| `7C436110-…:anyboot-probe`（Apple 命名空间普通变量） | ✅ 写入成功 |
| `7C436110-…:efi-boot-device` | ❌ `(iokit/common) not permitted` |
| `8be4df61-…:Boot0081` | ❌ `not permitted` |
| `8be4df61-…:BootOrder` | ❌ `not permitted` |
| `4D1FDA02-…:OCBtNext` / `OCBt0081` | ✅ **写入、读回、删除都成功**（非 root 报 `general error`） |

排除的三个常见解释：

| 解释 | 是否成立 | 证据 |
|---|---|---|
| 权限不够 | ❌ | root 同样被拒；普通变量 root 能写 |
| `Kernel→Quirks→DisableVariableWrite` 关了写 | ❌ | 实测 `false` |
| SIP 拦了 | ❌ | `csr-active-config=0x843`，含 `0x40 CSR_ALLOW_UNRESTRICTED_NVRAM` |

**进一步排除「权限模型」**：`bless`（Apple 签名带 entitlement，root 运行）也被拒：

```text
bless --setBoot --mount / --verbose
  Setting EFI NVRAM: efi-boot-device='<array>…'
  Could not set boot device property: 0xe00002e2     ← kIOReturnNotPermitted
bless_exit=3        （NVRAM 一个字节都没变）
```

**结论**：macOS 用户态按**变量名**精确拦截 `Boot*` 与 `efi-boot-device`，没有任何合法路径绕过；`OCBt*` 不受此限制。

### 8.4 由此得到的能力表

| 能力 | macOS | Linux (`efivarfs`) / Windows (`GetFirmwareEnvironmentVariableW`) |
|---|---|---|
| 读 `BootOrder` / `Boot####` | ✅（就是真值） | ✅ |
| 读 `OCBt*` | ✅ | ✅ |
| 写 `Boot*` | ❌ | ✅ |
| 写 `OCBt*`（root） | ✅ | ✅ |
| 复刻 Ctrl+Enter（写 `OCBt0080` + 重排 `OCBtOrder`） | ✅ 已真机跑通（见 §8.5） | ✅ |

### 8.5 复刻 Ctrl+Enter：实现与两个实测结论

`OcSetDefaultBootEntry` 只在「条目已在 `BootOrder` 里匹配上」时做一件事：把它的编号挪到顺序首位
（`DefaultEntryChoice.c:1112-1147`）；没匹配上就现场造一条 `EFI_LOAD_OPTION` 塞进 `0x0080`
（`:936-1103`，**永远只用这一个槽**，所以会覆盖上一条默认项的 load option）。

AnyBoot 的 `channels/oc_default.rs` 照这个语义做，但避开了覆盖：

```text
① 目标在 OCBtOrder 里已有槽  → 只写 OCBtOrder（把编号挪到首位）
② 固件全局 Boot0080 里有它   → 把那条的值原样抄进一个空闲 OCBt00XX，再排首位
③ 卷上有明确的 .efi 文件     → 按该分区的 HardDrive 节点自己造一条（boot_entry 的布局代码）
```

真机验证（`oc_default::tests::real_esp_set_default`，root 跑，跑完自动还原）：

```text
原始 OCBtOrder: [0080]
① 设 Windows 为默认 → 在 OCBtOrder 里新建 OCBt0081 并放到首位
   写后顺序: [0081, 0080]        ← 回读校验通过
② 设回 macOS → 把 OCBt0080 挪到 OCBtOrder 首位
   还原后: [0080]  ✓ 与原始一致
```

**同一轮验证里抓到一个真 bug**（既有代码，不在这次新增里）：macOS 的 `nvram` 收两个参数
`nvram <guid:name> =%XX%XX` 时，会把第一个当「读取」（照原值打印）、第二个当「设置一个空名字的变量」，
**退出码 0，却什么都没写**。所以 `direct_write` 一直是"假成功"：

| 场景 | 结果 |
|---|---|
| 变量**不存在** | GET 返回非 0 → 走提权回退（`elevated_nvram_script` 用的是**单个** argv，写法正确）→ 真的写进去了 |
| 变量**已存在** | GET 返回 0 → 被当成"直写成功"→ **静默 no-op**，回读还是老值 |

这个坑之所以一直没暴露：`Boot*` 在 macOS 上本来就不可写（`can_write_boot_vars()=false`，走不到这条路径），
`OCBt*` 又是刚接进来的新路径。修法是一行：整条 `guid:name=%XX…` 必须是**同一个 argv**
（`nvram::nvram_write_arg`，单测钉住"不含空格、只有一个等号"）。

---

## 9. 日志渠道：为什么你机器上 170 份日志里只有 1 行 `OCB:`

`OcLog.c:120` 的文件名生成只有「前缀 + 时间戳」，**OC 与 `boot.efi` 共用同一个文件名**：

```text
opencore-<YYYY-MM-DD-HHMMSS>.txt
```

而 `Misc→Debug→AppleDebug=true` 会把 `boot.efi` 的 256KB 固定缓冲灌进同一个文件，**循环覆盖**，把前面 OC 的 `OCB:` 段冲掉。

实测：ESP 根目录 170 份 `opencore-*.txt`，含 `OCB:` 的只有 **1 份**（且只有一行 `OCB: StartImage failed`，属 `WARN` 级，所以能活下来）。

另外两个门：

| 项 | 这台机器的值 | 含义 |
|---|---|---|
| `Misc→Debug→Target` | `67` = `0x43` | `BIT0` 使能 + `BIT1` 控制台 + `BIT6` 文件（`OcLog.h:29/35`）→ **文件日志本来就是开的** |
| `Misc→Debug→DisplayLevel` | `2147483650` = `0x80000002` | 只含 `DEBUG_ERROR(0x80000000)` + `DEBUG_WARN(0x02)`。`OCB: Adding entry type…` 属 `DEBUG_INFO = 0x40` → **根本不输出** |

所以要拿到可解析的菜单，需要：

```
Target       = 67            （不变）
DisplayLevel = 2147483714    （0x80000042，加 DEBUG_INFO）
AppleDebug   = false         （否则 boot.efi 覆盖）
LogModules   = OCB,OCBP,OCBT,LNX   （可选，缩小体积）
```

预期输出（格式稳定，可直接正则）：

```
OCB: Found 8 potentially bootable filesystems
OCB: Adding fs 67A1… (E:0|L:0|P:Success) - PciRoot(0x0)/…/HD(5,GPT,…)
OCB: Building entry from Boot0080
OCB: Adding entry type (T:1|F:0|G:0) - …\System\Library\CoreServices\boot.efi
```

`Found N potentially bootable filesystems` 这一行是预演器唯一无法自己算出来的量。
`SysReport`（`Misc→Debug→SysReport`）dump 的是 ACPI 表，与启动项无关，已排除。

---

## 10. 被推翻的结论（探索过程，按时间顺序）

留在这里是为了避免以后重复踩。

### ① 用本地时间找引导日志 → 结论「用户没进过 OC」

- 证据是 `ls -lat` 里最新日志 14:35，而用户 22:18 才重启。
- **反证**：日志名是 **UTC**。`opencore-2026-10-27-151056.txt` = 本地 23:10，正是那次重启。
- **教训**：ESP 根目录日志名的时间戳是 UTC，比较前先 `date -u`。

### ② 把 macOS 侧 `BootOrder` 当作固件真值 → 结论「固件里没有 OC 项，卡片该报错」

- **反证**：`BootCurrent` 都读不到；而用户 BIOS 里确实有 OpenCore 项。
- **修正**：macOS 侧读到的是**部分真值**（§8.2 已证明不是重定向，就是真值本身残缺 —— 固件只登记过 `0x0080`）。
- **代码影响**：判定不能只靠 `BootOrder`，要结合 `LauncherOption` 与「`BootCurrent` 是否可读」来判断「本系统视图是否可信」。

### ③ 以为「自己构造 `EFI_LOAD_OPTION` 写进固件」能解决

- 方案：按 UEFI 布局拼 HardDrive + FilePath + End 节点，写 `Boot0081` + `BootOrder`。
- **反证**：root 写入直接被拒 `(iokit/common) not permitted`；`bless` 也一样。
- **结论**：macOS 侧这条路彻底不通，代码里对应按钮已撤（留着只会让人点了收到 `not permitted`）。

### ④ 以为「改 `LauncherOption` 就能让 OC 自注册」

- **事实**：`LauncherOption` 的自注册（`InternalRegisterBootstrapBootOption`，`DefaultEntryChoice.c:1246`）在**经 `\EFI\BOOT\BOOTX64.EFI`（Bootstrap）兜底启动**的机器上实测不写任何东西。
- 用户 BIOS 里那个 OpenCore 项是自注册成功的（用户明确「我完全通过 bios 新引导项 OpenCore 进来的」），但**后续重启不再重写**，且 macOS 侧读不到它。
- **结论**：以 BIOS 菜单为准；AnyBoot 只做配置自检，不承诺「改完就变绿」。

---

## 11. 预演器的算法与误差来源

### 11.1 输入（全部只读）

| 输入 | 来源 |
|---|---|
| 卷列表与角色 | `diskutil list -plist` / `diskutil apfs list -plist`（macOS）、`lsblk -J`（Linux） |
| 卷上的引导文件 | 直接 `stat` 候选路径 |
| blessed 状态 | `/System/Volumes/Preboot/<UUID>/System/Library/CoreServices/`、卷上 `gAppleBlessedSystemFileInfoGuid` var |
| `BootOrder` / `Boot####` | `nvram`（macOS）/ efivarfs（Linux） |
| OC 配置 | `EFI/OC/config.plist`：`ScanPolicy`、`HideAuxiliary`、`BlessOverride`、`Misc→Entries`、`Misc→Tools`、`UEFI→Drivers` |

### 11.2 算法（与 §2–§6 一一对应）

```
entries = []

# ① BootOrder
for n in BootOrder:
    dp = parse(Boot####[n])
    vol = match_harddrive_node(dp)        # 找不到卷 → 丢弃
    if vol: entries += {source: BootOrder, path: dp.filepath, number: n}

# ② 每卷 bless
for vol in 所有卷:
    if blessed_file(vol):    entries += {…}
    elif blessed_folder(vol): entries += {…}
    else:
        for p in [boot.efi, bootmgfw.efi, BOOTX64.EFI]:
            if exists(vol + p): entries += {…}; break
    if vol.role == Preboot:
        for uuid in same_container(vol): entries += Preboot:uuid\…\boot.efi

# ③ 协议驱动（能静态推算的部分）
if OpenLinuxBoot 在 UEFI→Drivers:
    entries += loader/entries/*.conf，无则 autodetect vmlinuz*/initrd*
for d in [ResetNvramEntry, ToggleSipEntry]: if loaded: entries += 工具项
entries += Misc→Tools 里的每一项
entries += Misc→Entries 里的每一项（Custom 条目）

# 过滤 / 命名 / 排序
drop 重复 DevicePath
if HideAuxiliary: drop Recovery / TimeMachine / Auxiliary
title   = contentDetails > disk_label > 卷标 > 固定串
default = BootOrder 里第一条成功注册的非工具项
```

### 11.3 误差来源（诚实清单）

| 误差 | 影响 | 能不能修 |
|---|---|---|
| 固件能否真的枚举到该卷（APFS 驱动没加载 / USB 未初始化 / 卷损坏） | 预演有、实际没有，或反之 | ❌ 只能靠 OC 日志校准 |
| 卷的枚举顺序（决定菜单先后） | 顺序可能与预演不同 | ❌ 依赖固件句柄顺序 |
| `BlessOverride` 里的自定义路径 | 未实现前会漏 | ✅ 可实现 |
| 协议驱动的内部逻辑（`OpenLegacyBoot` 扫 MBR 等） | 可能漏条目 | ⚠️ 只能实现已知的几种 |
| `.contentVisibility` 的解析 | 隐藏状态判断错 | ✅ 可实现 |
| 加密卷 / FileVault | 主系统能看到、UEFI 侧可能看不到 | ❌ |

结论：**预演器定位是「解释与排查」，不是「精确复刻」**。要 100% 就走 §9 的日志校准。

### 11.4 实现时踩到的六个实机事实

写 `channels/oc_menu.rs` 的过程中，预演结果和真机菜单对不上，逐个查出来的（都已修在代码里）：

**① `Boot0080` 的路径是 Preboot 写法，与卷扫描写不到一起**

实测 `Boot0080` 解出来的 FilePath 是：

```
\2D8A2ED9-B942-494D-9220-59187BC83F94\System\Library\CoreServices\boot.efi
```

而卷扫描得到的是 `\System\Library\CoreServices\boot.efi`（不带 UUID）。字符串全等必然对不上，于是 macOS 那一项被判成「固件里有、文件不存在」的死项 —— 完全是误报。

修法（`same_target`）：**双方都带 UUID 时要求 UUID 相同**（否则是同一容器里的两个 macOS）；只要有一方不带，就按「去掉 UUID 段与 `Preboot` 段」的规范路径比较。

**② 去重键不能只用路径，必须带卷**

第一版按 `path` 去重，结果三个 ESP 各自的 `\EFI\BOOT\BOOTX64.EFI` 被合并成一条 —— 真机上 `ARCH-EFI` 的泛型项直接消失。

OC 的去重条件是 **DevicePath 全等**（`BootEntryManagement.c:522`），而 DevicePath 里含 HardDrive 节点（= 卷）。所以键必须是 `卷 + 路径`。

**③ `.contentDetails` 在 macOS 根卷上不存在**

`cat /.contentDetails` → `No such file or directory`（`/.disk_label/contentDetails` 同样）。所以「名字按 `.contentDetails` → `.disk_label` → 卷标」在这台机器上前两级全落空，**真正显示的是卷标**。

而 OC 菜单里那个 `MAC` 正是根卷的 `Volume Name`（`diskutil info /`）。取法：根卷同时挂在 `/Volumes/MAC` 下，两者 `dev` 相同（实测都是 `16777237`），直接读那个目录名即可，**不必每次 refresh 调 `diskutil`**（一次 ~130ms）。

**④ 无法用 inode 判断「哪条 Preboot 是当前系统」**

原本想用 `boot.efi` 的 `(dev, ino)` 比对 firmlink 的那份：

```
/System/Library/CoreServices/boot.efi                                     dev=16777237 ino=1152921500312105676
/System/Volumes/Preboot/2D8A…/System/Library/CoreServices/boot.efi        dev=16777239 ino=356025
```

**不同** —— firmlink 没有作用到这个文件，系统卷上真有一份。所以 inode 比对不可用。

最终规则（零外部命令）：

- `Preboot` 下只有一个 UUID 目录 → 它就是当前系统，与「启动卷」条目合并；
- 有多个 → 优先取 `BootOrder` 里出现过的 UUID（默认项通常指向自己），都没有就取第一条，其余作为「同一容器里的另一个 macOS」单独列出。

对单系统机器 100% 正确；多系统机器上「额外条目」也照样能列全，只是「哪条是当前」的标注可能不准 —— 这属于 §11.3 里已经声明的误差。

**⑤ 默认项必须按启动顺序表走，不能按扫描顺序走**

第一版是「在枚举出来的条目里，找第一条来源是 bootorder 的」。看着等价，实际上把**卷的遍历顺序**当成了启动顺序。

实测这台机器：

```
OCBtOrder = %82%00 %81%00 %80%00      # 0x82 WINDOWS-EFI 在前
entries   = [MAC, WINDOWS-EFI, MacOS-efi, ARCH-EFI]   # MAC 先被扫到
```

结果界面把 **MAC** 标成「OC 默认项」，而真默认项 WINDOWS-EFI 那一行还挂着「设为默认」按钮 —— 点下去只是把已经在最前的项再挪一次，回读校验也只看到「顺序没变」，看起来像生效了。

修法：配对时按 `order` 的顺序记下「编号 → 条目下标」，默认项取这张表里的第一条（跳过工具项）。

**⑥ 同名文件要靠分区 GUID 区分，但 GUID 不能当必要条件**

`\EFI\BOOT\BOOTX64.EFI` 在多块 ESP 上都存在是常态（本机 4 块 ESP，其中 MacOS-efi 与 ARCH-EFI 都有）。只比路径会把 `OCBt0081`（ARCH-EFI）算到先扫到的 MacOS-efi 头上，来源标签跟着错。

`Boot####` 的 HardDrive 节点里有分区 GUID，`diskutil` 的 `DiskUUID` 对 ESP 正是同一个值（GPT 分区 GUID，UEFI 混合字节序），所以可以精确配对。

但**只能用来「挑」不能用来「否」**：APFS 卷的 `DiskUUID` 是**卷 UUID**，与启动项里记的物理分区 GUID 天生不等。第一版把它当必要条件，`OCBt0080`（MAC）立刻被判成「文件看不到」的孤儿项 —— 比原问题更糟。

最终规则：路径唯一就直接认；出现同名文件才去问 `diskutil`（按需，命中缓存，路径不重复时零开销）；分不出来就取先命中的那条，与没有 GUID 时一致。

---

## 12. 未验证 / 待确认

1. ~~复刻 Ctrl+Enter 是否真能让 OC 下次默认启动那个系统~~ —— **已真机验证**（§8.5）：
   `OCBtOrder` 从 `[0080]` 改成 `[0081, 0080]`、回读一致，再改回 `[0080]` 与原始一致。
2. `OCBtNext` 能否作为「只下一次」的等效物（OC 启动后 `InternalClearNextVariables` 会删它）—— 未真机验证。
3. Linux 侧 `efivarfs` 写 `Boot*` 在 AMD 固件上的行为（预期可用，因为 Linux 不受 Apple 白名单限制）。
4. `OpenCanopy` 图形 picker 与文本 picker 在辅助项处理上的差异（两者共用过滤逻辑，但图标解析不同）。
5. **Windows 的两条命令**（`Get-Partition` 按 GPT 类型枚举 ESP、`Add-PartitionAccessPath -AssignDriveLetter` 分配盘符）：
   解析层有 5 个单测钉住 `ConvertTo-Json` 的输出形状，**命令层一次都没实机跑过**。
   尤其是 `Start-Process -Verb RunAs -Wait` 拿不到子进程退出码，成败只能靠事后重新枚举判断 ——
   真实 UAC 下这条是否可靠（用户点「否」、盘符用完、策略禁止），需要一台 Windows 机器确认。
6. `win_esp::parse_json` 对 `DriveLetter` 的**码点形式**（`83` 而不是 `"S"`）是防御性处理：
   见过有人报告某些 PowerShell 版本这样序列化 `[char]`，但没拿到真实输出，两种都认着。
7. **免密助手的安装链路没有经过一次真实授权**（见 §9.2 of `boot-menu-discovery.md`）。
   `osascript … with administrator privileges` → `install` 落 `/Library` → `launchctl bootstrap`
   这一段需要人点一次密码，开发环境里做不到，所以代码只做到了「语法与顺序可验证」：
   `sh -n` 检查安装/卸载脚本、`plutil -lint` 检查 launchd plist、断言 `bootout` 在 `bootstrap` 之前。
   **没验证的**：`launchctl bootstrap system` 在真机上是否一次成功、`WatchPaths` 对
   「rename 进目录」是否必然触发、以及助手以 root 执行 `nvram` 的实际耗时。
8. 助手的**超时阈值**（4 秒）是估的：launchd 冷启动一般 <300 ms，但没有在低配机器上量过。
   如果真机上出现「第一次点要等 4 秒才退回密码框」，就把它调大，别调小。

---

## 相关文档

- 面向使用者的原理与功能说明：[`boot-menu-discovery.md`](boot-menu-discovery.md)
- 三引导器通道模块设计：[`boot-channel-技术细节设计.md`](boot-channel-技术细节设计.md)
