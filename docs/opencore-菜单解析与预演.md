# OpenCore 菜单解析与预演

> 原 README「OpenCore 菜单里的系统列表是怎么来的」章节归档。对应功能截图见 [README](../README.md#截图)。
> 原理与用法：[`boot-menu-discovery.md`](boot-menu-discovery.md) · 详细源码追踪：[`boot-menu-discovery-internals.md`](boot-menu-discovery-internals.md) · APFS 选型：[`apfs-预演三端统一选型.md`](apfs-预演三端统一选型.md)

开机按 F12 进 OpenCore，那一排系统图标看着像「它把硬盘扫了一遍」。**其实不是。**
OC 从不遍历目录找系统，它是把三个固定来源并成一张表，过滤掉一部分，再按 NVRAM 里的顺序排一遍：

| 来源 | 怎么来的 | 典型条目 |
|---|---|---|
| ① 固件 `BootOrder` 里的每条 `Boot####` | 读出 `EFI_LOAD_OPTION`，**丢掉描述和 OptionalData**，只留设备路径，再反查是哪个卷 | macOS（描述 `MAC`） |
| ② 每个卷上的 Apple bless 扫描 | 依次试：blessed 文件 → blessed 文件夹 → **预定义路径表**（`\System\Library\CoreServices\boot.efi`、`\EFI\Microsoft\Boot\bootmgfw.efi`、`\EFI\BOOT\BOOTX64.EFI` / `BOOTAA64.EFI` 按架构取其一） | Windows、`MacOS-efi`（generic） |
| ③ 第三方「条目协议」驱动 | `OpenLinuxBoot.efi` / `OpenLegacyBoot.efi` / `ResetNvramEntry.efi` 等自己往里塞 | `Reset NVRAM`、独立内核条目 |

由此有三个容易踩的结论：

- **Linux 不在预定义路径表里**（源码注释写明是设计选择）。所以没装 `OpenLinuxBoot.efi` 时，你的 Arch 在 OC 菜单里只是一个泛型的 `ARCH-EFI`（点进去才是 GRUB）。改 `ScanPolicy`、改 `HideAuxiliary` 都帮不上。
- **rEFInd 默认不会出现在 OC 菜单里**——它的卷上既没有 `bootmgfw.efi` 也没有 `\EFI\BOOT\BOOTX64.EFI`，也不在 `BootOrder` 里，三条路全断。
- **卷本身是否入候选只看 `ScanPolicy`**：`ScanPolicy=0` 时所有文件系统全放行，连 `Microsoft Basic Data` 类型的 ESP 也算。OC 在这个阶段不检查目录结构。

另外两件事跟直觉相反：

- **菜单顺序 ≠ 默认项**。顺序取决于固件返回文件系统句柄的次序；默认项是 `BootOrder` 里**第一条注册成功的非工具条目**。所以「默认的不在第一行」是正常现象。
- **Ctrl+Enter 不改任何文件**，它调 `OcSetDefaultBootEntry` 去改 NVRAM：匹配已有 `Boot####` → 没有就现场造一个 → 写进 `0080` 这一个槽 → 和 `BootOrder[0]` 交换位置。OC 只用 `0x80` 一个槽，所以设新默认会覆盖上一条的 load option。

**为什么不能在 macOS 里直接读 OC 算出来的那份菜单**：`OcScanForBootEntries` 是 UEFI 运行期函数，结果是内存链表，菜单渲染完就释放，重启即丢；OC 没有 IPC、没有导出接口，也从不把启动项写文件。主系统里能读到的是**输入**（卷、文件、NVRAM），不是**结果**。

所以 AnyBoot 的做法是**按同一套规则预演**（纯读 ESP + `diskutil`，不写盘、不碰 NVRAM、不需要密码）：

- 列出「下次进 OC 大概会看到哪几项」，每项标出**来源**（`BootOrder` / bless / Preboot / 协议驱动 / 自定义）、**类型**、**所在卷与文件路径**、**会不会被 `HideAuxiliary` 藏掉**
- 标出**默认项**
- 每一行右侧可以点 **设为默认**：等价于进 OC 菜单对它按 `Ctrl+Enter`。它写的是 OC **私有命名空间**的
  `OCBtOrder` / `OCBt0080`（`RequestBootVarRouting=true` 时 OC 读的就是这一组，而 macOS 只拦名字叫 `Boot*` 的变量），
  所以 macOS 侧也能用；动 NVRAM 前整份备份两套命名空间，写完回读校验
- 直接点出上面那三类问题：「rEFInd 不会出现」「Linux 只是泛型项，缺 `OpenLinuxBoot.efi`」「配置自相矛盾」

准确率天花板在 **90% 左右**——卷、文件、NVRAM 都能读准，唯一猜不准的是「这块固件在那一刻能不能真的枚举到某个卷」（APFS 驱动没加载、USB 未初始化都会差）。想要 100%，可以让 OC 自己把菜单写进日志（改 `Misc→Debug` 三项后重启一次），代价是之后不再保存 `boot.efi` 的日志。

## 三个平台上差在哪

预演规则、`EFI_LOAD_OPTION` 的构造、`BootOrder` 的解析是**同一份代码**；真正分平台的只有「怎么读 NVRAM」和「怎么找到那块 ESP」：

| | macOS | Linux | Windows |
|---|---|---|---|
| 读 NVRAM | `nvram` 命令 | efivarfs（`/sys/firmware/efi/efivars`） | `GetFirmwareEnvironmentVariableW` |
| 写 NVRAM | 只能写 OC 私有 `OCBt*`（全局 `Boot*` 被 Apple 白名单挡住，root 也不行） | 直写 + `pkexec` 提权 | `SetFirmwareEnvironmentVariableW`（要管理员） |
| 找 ESP | `/Volumes` + `/` | `/efi`、`/boot/efi`、`/boot`、`/esp` | `Get-Partition` 按 GPT 类型列出 ESP |
| 默认项写到哪 | `OCBtOrder` / `OCBt####` | `BootOrder` / `Boot####` | `BootOrder` / `Boot####` |

**Windows 的坑**：系统默认不给 EFI 分区分配盘符，而那个分区在资源管理器里是隐藏的。所以 AnyBoot 不用盘符扫描，而是按 GPT 类型 `c12a7328-…` 把 ESP 列出来；一块都没盘符时，OpenCore 卡片会显示成「未检测到」并**说明原因**（哪块磁盘哪个分区、多大），带一个「分配盘符」按钮 —— 点它会弹一次管理员授权，完成后自动重新识别。Linux 上则要求 ESP 是**已挂载**的（`/boot/efi` 或 `/efi`），没挂载同样扫不到。

**macOS / Preboot 条目的三端差异**：目前预演里所有 macOS 条目（含 Preboot）只在 macOS 上算得出来——Windows / Linux 没有 APFS 驱动，`Preboot/<UUID>/…/boot.efi` 这一层物理不可见。已选定「纯 Rust 只读 APFS crate」方案补齐这条缺口（零挂载、直接解析容器分区，与 NVRAM 合并一次提权），
选型过程与架构见 [`docs/apfs-预演三端统一选型.md`](apfs-预演三端统一选型.md)。

## Linux 端补充

- **efivarfs 直读预演**：Linux 上直接读 `/sys/firmware/efi/efivars` 复算 OC 菜单。
- **非白名单变量写入专道**：写入不在固件白名单里的变量（如 `OCBt*`）时自动清不可变位（`S_IMMUTABLE`）。
- 修过「设为默认要两次才回显」——根因是提权 dump 早于写入、带回旧表。
