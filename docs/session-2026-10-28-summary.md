# 2026-10-28 会话总结：Windows 审计、免密助手修复与 GRUB 诊断

## 一、OCBtOrder 写不生效（免密模式根因）

**现象**：界面提示「OCBtOrder 写进去了，但回读时 OCBt0082 不在首位——这次写入没有生效」。

**实机证据**：NVRAM 中 `OCBtOrder`（旧值 `80 82 81`）与 `OCBTORDER`（新值 `82 80 81`）并存。

**根因**：`helper_proto.rs` 的 `validate()` 对变量名做了 `to_ascii_uppercase()`，
助手把顺序写进了 `OCBTORDER`——UEFI 变量名区分大小写，这是两个互不相干的变量。
助手回执照样 `ok`，属于「静默写错」。只有免密模式暴露（`csascript` 老路径不经 `validate`）。

**修复**（`helper_proto.rs`）：
- 变量名原样传递到底，不做任何大小写转换；
- `valid_name()` 只认 OC 精确拼法（`OCBtOrder` / `OCBtNext` / `OCBt####`，四位必须大写十六进制）；
- 回归测试断言 `nvram_arg` 输出 `{OC_GUID}:OCBtOrder=%82%00%81%00`。

**已端到端验证**：手工构造请求投入 spool 信箱，回执
`{"ok":true,"detail":"已写 1 条变量：OCBtOrder"}`，变量名原样到达，`OCBTORDER` 未复活。

## 二、Windows 相关代码审计（commit `97cbf1d`，8 文件 +259/−51）

| # | 缺陷 | 文件 | 修复 |
|---|------|------|------|
| 1 | 「设为默认」提权写成功后不 return，落回非提权路径再写一遍 → 用户看到失败，NVRAM 其实已改好 | `channels/mod.rs` | 提权分支就地 `return set_default_elevated(...)` |
| 2 | 提权子进程自己扫 ESP，多块盘时可能挑错 → 读错 OC config、命名空间翻转 | `oc_default.rs` / `main.rs` / `lib.rs` | 父进程传 `--esp`，子进程校验签名后优先采用 |
| 3 | `GetFirmwareEnvironmentVariable` 返回 122（缓冲区不足）被当「变量不存在」 | `nvram.rs` | 先读 1 KB，122 再试 8 KB |
| 4 | `AdjustTokenPrivileges` 特权未授予时也返回非零，真实结果在 `ERROR_NOT_ALL_ASSIGNED`；且漏 `CloseHandle` | `nvram.rs` | 补 1300 判定 + 两处 `CloseHandle(token)` |
| 5 | 命令行「每参数套引号」遇 `S:\` 结尾反斜杠吃掉收尾引号 → 子进程收到 `S:"` | `elevate.rs` | 按 `CommandLineToArgvW` 规则重写 `quote_arg`/`build_params` + 2 单测 |
| 6 | `powercfg /a` 英文环境判不出休眠（词形是 Hibernation） | `lib.rs` | 改词根 `hibernat`，补英文真机原文测试 |
| 7 | PowerShell `DriveLetter` 三态（`"S"` / `null` / 码点 83）只处理了一态 | `esp.rs` | 补 Number 分支 + `to_ascii_uppercase()` |
| 8 | 提示音路径含 `'` 破坏 PowerShell 单引号串 | `notify.rs` | `''` 转义 |

**验证边界**：本机无法交叉编译 Windows 目标（Homebrew rustc 与 rustup 双工具链冲突；
`ring` 的 C 交叉编译卡死）。Windows 侧仅做静态核对：`ERROR_INSUFFICIENT_BUFFER`=122、
`ERROR_NOT_ALL_ASSIGNED`=1300、相关 API 均在 `Win32::Foundation` / `WindowsProgramming`。
**Windows 分支未经编译验证**，下次在 Windows 构建时留意。

## 三、顺带修复

- **macOS 从 `f613178` 起编译不过**：`lib.rs` 无条件引用 `#[cfg(not(macos))]` 的 `apfs_reader`。
  给两个入口加 macOS 占位版（保持参数名 `force` 不变，避免 Tauri 按名匹配静默失配）。
- **免密助手旧版判定不可靠**（`helper_install.rs`）：从「比文件大小」改为「比 sha256 内容」，
  按 (len, mtime) 缓存；spool `out/` 权限 0755 → 0733。
- **「接管上一个实例」在 macOS 从未成功过**（commit `4cc610d`）：
  `ps -o ucomm=` 读内核 `p_comm`，上限 16 字节；测试二进制 `anyboot_lib-<hash>`（24 字节）
  被截断 → `is_same_app_process` 永远判否 → 误报 `AlreadyRunning`。改用 `comm=` 取 basename，
  加回归测试 `own_image_name_is_not_truncated`。生产二进制 `anyboot`（7 字节）不受影响。

## 四、免密助手重装（macOS）

- 重新构建 app（x86_64-apple-darwin），旧包留作 `/Applications/AnyBoot.app.old-20261028`；
- 一次授权完成：装助手 → 重建信箱目录 → 重载 launchd → 删除 `OCBTORDER`；
- 装后 `/Library/PrivilegedHelperTools/…` 与新 app 的 sha256 一致（`419dd64…`），`helper_is_stale` 不再提示；
- NVRAM 现状：`OCBtOrder = 80 82 81`（默认项 MAC），`OCBTORDER = 不存在`。

## 五、GRUB rescue 问题诊断（ARCH-EFI 分区）

**现象**：OC 扫描 ARCH-EFI 分区的 grub 报错，需手工
`set root` / `set prefix` / `insmod normal` / `normal` 才能进系统。

**根因**：grub 核心镜像把 `/boot/grub` 位置以 `(,gptN)` 分区号形式焊死在二进制里，
分区表变动后全部指向错误分区：

| 文件 | 内嵌 prefix | 状态 |
|---|---|---|
| `EFI/BOOT/BOOTX64.EFI`（gpt14） | `(,gpt8)/boot/grub` | ❌ |
| `EFI/arch-efi/grubx64.efi` | `(,gpt10)/boot/grub` | ❌ |

手工那四行只是每次开机临时扶起，不是修复。

**修复路线**（推荐 A）：
- **A. 退役 grub，走 systemd UKI**：`EFI/ArchLinux/linux-zen.efi` 是完整 UKI
  （`.linux`/`.initrd`/`.cmdline`/`.osrel` 齐全，`root=UUID=…`），纯 UUID 定位，免疫分区表变化。
  在 Arch 里给 `/etc/mkinitcpio.d/linux-zen.preset` 加 `default_uki="…"`，
  cmdline 放 `/etc/kernel/cmdline`，内核更新自动重建；之后删掉两份坏 grub。
- B. 保留 grub：grub-mkimage 嵌 UUID 早期配置
  （`search.fs_uuid <UUID> root` + `set prefix=($root)'/boot/grub'`，模块含 `search_fs_uuid`
  + 对应 fs 驱动），UUID 用 `grub-probe --target=fs_uuid /boot/grub` 获取。
- C. 每次手工 rescue 命令（仅过渡用）。

**生态参考**：systemd UKI 工具链（ukify + mkinitcpio/dracut）> systemd-boot（与 OC 菜单角色重复，不必装）
> sbctl（Secure Boot 辅助）> rEFInd / Limine。

## 六、遗留事项

1. **Windows 分支未编译验证**——下次 Windows 构建时确认 `97cbf1d` 全部改动可编译、行为符合预期。
2. **默认启动项仍是 MAC**——机制已修好，用户在卡片上点「设为默认」即可真正生效（未代改）。
3. **GRUB → UKI 迁移需在 Arch 里操作**（挂 ESP、配 preset、删旧 grub），macOS 侧只能诊断无法代做。
4. Windows 的 `boot_entry::partition_info_platform` 仍是「未实现」——Windows 上不能自造启动项，
   只能用已在 `BootOrder` 里的项；补齐需 PowerShell 取 Offset/Size/GptGuid 拼 HdNode，真机验证前不盲写。

## 提交记录

- `97cbf1d` — windows 审计 8 处修复 + 恢复 mac 端构建
- `4cc610d` — macOS 进程名 ucomm 16 字节截断修复
- 测试：`cargo test --lib` **409 passed / 0 failed**（首次全绿）
