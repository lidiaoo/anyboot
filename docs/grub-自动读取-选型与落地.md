# GRUB 自动读取（不装 09_anyboot）：需求、技术选型与落地 TODO

> 状态：**需求已定稿，尚未写代码**（2026-10-29）。工作区停在 `f04266e`。
> 关联：[`boot-channel-技术细节设计.md`](boot-channel-技术细节设计.md)（现有三引导器通道）、
> `src-tauri/src/grub.rs` / `grub_esp.rs` / `grub_install.rs`（手动解析链路）、
> `linux/09_anyboot`（GRUB 脚本）、`channels/apfs_reader.rs`（零挂载直读的先例）

---

## 1. 两种模式的定位

| | 手动解析（已有） | 自动读取（本文） |
|---|---|---|
| 前提 | Linux 端装了 `/etc/grub.d/09_anyboot` | **没装** 09_anyboot，GRUB 保持默认布局 |
| 数据源 | ESP(FAT) 上的 `/EFI/grub.cfg`、`/EFI/grubenv` | Linux 分区自己的 `/boot/grub/grub.cfg`、`grubenv` |
| 数据怎么来的 | Linux 端 `grub_collect` 主动覆盖搬运 | 没人搬运，三端自己去读 |
| 读盘能力 | 无要求（FAT 三端原生可读） | macOS/Windows 需要自己解析 ext4 |
| 提权 | 不需要 | macOS 走 helper；Windows 走 UAC |
| 语义 | 写 ESP 上的 env，由 09 脚本 `load_env -f` 生效 | 写 Linux 磁盘上的 env，等价于 `grub-editenv` / `grub-reboot` |
| 新鲜度 | 带 `synced_at` 时间戳 | 读到的是上次 `grub-mkconfig` 的产物，**要显示 mtime** |

两者**并列存在，不互相替换**；同一时刻只有一个在界面上生效（由用户选中的数据源决定）。

## 2. 需求边界（刻意收得很小）

**做**：

1. 读 `grub.cfg` → 条目 + submenu 分组 → 复用现有渲染；
2. 写 `grubenv`：`saved_entry` + `next_entry`，**恒 1024 字节原地覆盖**；
3. 定位「用户选的那个 ESP，是哪块 Linux 分区在用它」。

**不做**：

- 不写 `grub.cfg`（它是 `grub-mkconfig` 的产物，长度每次都变，写坏了直接进不去系统）；
- 不代建 `grubenv`（创建文件要分配块、改 inode，是最危险的一条路；改为提示在 Arch 里跑一次 `grub-editenv /boot/grub/grubenv create`）；
- 不支持 LUKS / LVM（里面根本不是 ext4 字节流）；
- btrfs 本期不做（另找实现，待定）；
- 不动手动解析那条链路的任何一个字节。

> 写入面窄到只有「已存在文件的定长覆盖」这一件事，这是整个方案能成立的根基：不分配块、不改 inode、不动位图/块组/超级块。ext4 库的选型风险因此降了一档。

## 3. 五条定案（用户拍板 2026-10-29）

| # | 定案 | 含义 |
|---|---|---|
| 1 | Linux 本机**不经过** ext4 驱动 | 同一套解析器，Linux 端直接 `fs::read` 文件 |
| 2 | 选 ESP → 遍历 ext4 分区，看谁的 `fstab` 挂载了它 | 并**兼容只写路径**（`/dev/sda1`）而非 UUID 的写法，见 §5 |
| 3 | macOS 提权走 helper | 扩展协议 v1 → v2，不每次弹密码 |
| 4 | btrfs 另找实现，**待定** | 本期只支持 ext4；识别到 btrfs/LUKS/LVM 明确报「不支持」 |
| 5 | UI **并排两块** | 左「手动解析（ESP）」，右「自动读取（Linux 分区）」，共用行渲染 |

## 4. 现状盘点：现有链路是「路径式」的，撑不住自动模式

| 现有事实 | 位置 | 对自动模式的影响 |
|---|---|---|
| `ENV_REL_PATH = "/EFI/grubenv"`、`CFG_REL_PATH = "/EFI/grub.cfg"` | `grub.rs:25,27` | 路径写死在 ESP 命名空间里，自动模式的路径是分区内的 `/boot/grub/…` |
| `ENV_BLOCK_SIZE = 1024`、`ENV_BLOCK_HEADER = "# GRUB Environment Block\n"` | `grub.rs:30,32` | **可直接复用**，写盘格式完全一致 |
| `parse_grub_cfg` / `parse_submenus` / `dedupe_display_entries` | `grub.rs:146/86/117` | 纯字符串处理，**一行都不用改**；selector 规则（`submenu_id>entry_id`）照旧 |
| `grub_rel(dir, rel)` 拼 `PathBuf` | `grub_esp.rs:36` | 自动模式没有挂载点，只有「设备 + 分区内路径」，这层断了 |
| `esp_probe` 要求 FAT32 | `esp.rs` | 自动模式的目标是 ext4，判据要换 |
| 已认 `/boot/grub` 与 `/boot/grub2` | `grub_esp.rs:625,636` | Fedora 这类用 `grub2` 的，自动模式照抄这套双路径 |
| `atomic_write_bytes`（临时文件 + rename） | `grub_esp.rs:41` | **不能复用**：ext4 上 rename 要改目录项，正是我们要避开的那类元数据写入 |
| `elevate.rs` 只覆盖 Windows(UAC)/Linux(pkexec) | `channels/elevate.rs:12` | macOS 没有一次性提权路径，只能 helper |
| helper 是同一个 app 二进制带 `--run-inbox` 以 root 运行，文件信箱协议 | `helper_proto.rs` | 机制可复用，但协议形状要改，见 §8 |

**结论**：解析器与格式常量全部复用；要新写的是「按分区内路径读文件 / 定点写文件」这一层抽象，以及跨平台裸设备访问。

## 5. 目标分区怎么定位

### 5.1 统一抽象 `PartSpec`

「用户选的 ESP」和「Linux 侧 fstab 里写的 ESP」本质是同一样东西的两种写法，只做一个解析器，两处共用：

| 写法 | 怎么认 | 置信度 |
|---|---|---|
| `UUID=84d0b19d-…` | ext4 超级块 UUID / FAT 卷序列（boot sector `0x43`） | **精确** |
| `PARTUUID=b3fbe669-…` | GPT 分区 UUID（= `diskutil` 的 `Disk / Partition UUID`） | **精确** |
| `/dev/disk/by-uuid/E537-7F31` | 本质是软链，拆出键值按 UUID 比 | **精确** |
| `/dev/disk/by-partuuid/…`、`by-label/…` | 同上 | 精确 / 推断 |
| `LABEL=ARCH-EFI`、`PARTLABEL=…` | 卷标 / 分区名，可能重名 | 推断 |
| `/dev/sda1`、`/dev/vda2`、`/dev/nvme0n1p1`、`/dev/mmcblk0p1` | 盘 + 分区号，见 §5.2 | 推断 |

比较前统一归一化：去 `-`、转大写（`E537-7F31` ≡ `e5377f31`）；ext4 UUID 与 FAT 序列走同一套比较。

### 5.2 `/dev/xxx` 怎么落到本机的 `diskNsM`

Linux 设备名与 macOS `disk0s9` 之间没有直接对应，做法是「拆出分区号 → 推断盘 → 校验」：

1. **分区号**无歧义：`sda1`→1、`nvme0n1p1`→1、`mmcblk0p1`→1、`vda2`→2。
2. **盘**：本机只有一块含 Linux 分区的物理盘（`disk0`），一律映射到它 → `/dev/sda14` → `disk0s14`。
3. **校验**：目标分区必须真存在且是 FAT（有 `/EFI` 目录）。若 `/dev/sda1` 指到 `disk0s1` 而它不是 ESP，就当**没匹配上**，不硬凑。

多盘机器上 Linux 的字母顺序按驱动探测顺序排，不可靠 → 降级为「按 `diskutil` 列举顺序对应」，标低置信度。原则：**能对上算红利，对不上不制造假阳性。**

### 5.3 三级定位 + 安全网

1. **精确**：`UUID` / `PARTUUID` 命中 → 直接选中；
2. **推断**：`LABEL` 或 `/dev/xxx` 命中 → 选中，但界面标「按设备名/卷标推断」+「换一个」；
3. **自证**：读候选分区 `grub.cfg` 里的 `linux … root=UUID=xxx`，与该分区超级块 UUID 比对（`grub-mkconfig` 每次都写，不依赖任何配置约定）；
4. **兜底**：以上都不中 → 列出所有有 `grub.cfg` 的分区（卷标 / 大小 / mtime）让用户点。

安全网的意义：即使 `/dev/sda1` 解析错，结果也只是「没自动选中」，**不会把 grubenv 写到另一块盘上**。

## 6. 技术选型过程

### 6.1 先把要求分档

自动模式真正需要的只有两件事：**读一个文件** + **对一个已存在的 1024 字节文件做定点覆盖**。这跟「通用读写文件系统」差一个量级——尤其不需要 create / truncate / allocate 任何一种能力。

> 之所以强调「不长于旧内容」，实测边界不是字节数而是**块边界**：`grubenv` 1024 字节 < 4 KiB 块，定点覆盖只碰文件第一个数据块。而 `grub.cfg` 每次长度都变（本机实测 2,455 ~ 52,760 字节），一旦要重写就必须走分配路径 —— 所以它只读。这条判断来自对 `ext4_rs` 写路径的逐行核对，见 §6.3。

### 6.2 crates.io 候选（2026-10 实测值）

| crate | 版本 / 更新 | 许可 | 读写 | 数据来源 | ⭐ | commits | 下载 |
|---|---|---|---|---|---|---|---|
| **ext4-view** | 1.0.0 / 2026-09 | MIT OR Apache | **只读** | `Ext4Read` trait / 路径 | 22 | **590** | **181.8k** |
| **rsext4** | 0.9.2 / 2026-09 | **Apache-2.0** | **读写 + jbd2** | `BlockIo`（按扇区） | 8 | 37 | 14.7k |
| ext4_rs | 1.3.3 / 2026-01 | MIT | 读写（不完整） | `BlockDevice` | **72** | 232 | 11.6k |
| ext4 | 0.9.0 / **2021-01** | MIT | 只读（仅超级块信息） | 镜像 | 57 | — | 31.9k |
| arcbox-ext4 | 0.1.2 / 2026-04 | MIT/Apache | 格式化 + 读 | 容器镜像 | 8 | 10 | 60.1k |
| ext4plus | 0.1.0-rc.2 / 2026-05 | MIT/Apache | 只读 | 同 ext4-view | 11 | — | 5.0k |
| ferrosys | 0.6.0 / 2026-10 | MIT/Apache | 读 + 写 | 镜像 | 2 | — | 744 |
| ext4-lwext4 | 0.1.2 / 2026-03 | MIT/Apache 包 **GPLv2** | 读写 | — | — | — | — |
| unearth | 0.4.0 / 2026-07 | MIT | 只读（取证） | 取证 | 0 | — | 58 |

### 6.3 逐个体检与被淘汰的理由

- **`ext4-view`（1.0.0）** — 定位就是只读，写是明确的 non-goal；`no_std + alloc`，主包无 `unsafe`；设计目标第一条「All valid ext2/ext3/ext4 filesystems should be readable」，extents / 64bit / metadata_csum / bigalloc / inline_data 都覆盖；数据源是 `Ext4Read { fn read(&self, start_byte, buf) }` trait，开 `std` 还有 `load_from_path`；API 是 `std::fs` 风格（`read_dir` / `metadata` / `exists`）；加密文件返回显式 `Ext4Error::Encrypted` 而不是垃圾数据。
  → **读取首选**。
- **`rsext4`（0.9.2）** — 出自 Starry OS 生态（`rcore-os/tgoskits` 的内核文件系统实现），模块完整：`jbd2`、`extents_tree`、`hashtree`、`checksum/crc32c`、`bmalloc`、`indirect`、`blockgroup_description`；测试里有 `linux_image_repro`、`metadata_lifecycle`、`unwritten_extents`；CHANGELOG 明写「与 Linux 7.1 语义对齐」「e2fsck 覆盖」。零依赖，edition 2024，本机 rustc 1.96 **stable 直接编过**。API 是块设备驱动式：`Jbd2Dev::new(dev, bs)` → `Ext4Filesystem::mount(&mut dev)`（还有 `mount_with_readonly_fallback`）→ 自由函数；`read_file` / `write_file(offset)` / `sync_filesystem` 都在。
  → **唯一能「挂载现有分区并修改」的 Rust 实现**；代价是 0.9.x 未到 1.0、API 会变（应锁 `=0.9.x`）、`USE_MULTILEVEL_CACHE` 默认开（落盘前必须 `sync_filesystem()`）。
- **`ext4_rs`（1.3.3，star 最高 72）** — 淘汰。逐行核对结论：
  - `src/lib.rs:1` 是 `#![feature(error_in_core)]`，stable 上 `error[E0554]` 直接编不过（删掉那两行即可，`error_in_core` 从 1.81 起已稳定；代价小但需要 vendor 一份打补丁的 fork，或整个应用切 nightly —— 后者免谈）；
  - `"w"` 解析成 `O_WRONLY|O_CREAT|O_TRUNC`，但 `O_TRUNC` 全仓只在 `consts.rs:52` 定义、**零处使用** → 打开不截断；要截断得自己调 `truncate_inode`（`file.rs:544`），它第一行 `assert!(old_size > new_size)`；长度相同就 panic；
  - 无 jbd2（只有 `super_block.rs:52` 的字段声明，没有回放/提交）；`s_state` 全仓零引用 → 写崩了文件系统对外仍显示 clean，Linux 开机跳过 journal 回放，静默不一致；
  - **纠正一个我先前说错的判断**：22 处 `todo!()` 全在 `fuse_interface/mod.rs`，核心写路径并不残缺；块位图、位图校验和、块组描述符校验和、超级块校验和、`i_blocks`、inode 校验和**全都写了**（`balloc.rs:408-421`、`block_group.rs:168`、`file.rs:441` 写完还回读校验）；写入顺序是「位图先、extent 后」（`inode.rs:254`），所以不会出现双重分配（最坏是块泄漏，`e2fsck` 一修即可）；
  - 但 `file.rs:311` 把 `start_bgid` 硬编码成 1，不看 inode 所在块组 → 新块大概率落在第 1 块组，文件变碎片。
  → 结论：**能写，但要走「不超过旧块边界」那条路**（`blocks_to_allocate == 0`，不动 inode、不走 balloc）。这条路恰好也是我们唯一需要的那条，所以它留在参考清单里，但不作为主路径（stable 编译 + 无日志两项缺点无法回避）。
- **`ferrosys`** — 淘汰。README 说每类都有 reader 和 writer，但 `open()` 返回只读的 `FsReader`；writer 是「从目录树/归档**造一整块新镜像**」，改不了现有文件系统。
- **`ext4-lwext4`** — 淘汰。包 MIT/Apache，但底层 `lwext4` 是 GPLv2，feature 名就叫 `gpl` / `gpl-extents`（**extents 支持在 GPL 那半边**），商用/二进制分发有授权风险。
- **`ext4`（0.9.0）/ `ext2` / `unearth` / `arcbox-ext4` / `ext4plus`** — 淘汰或备选：停更、只支持 ext2、取证向下载量极小、只做格式化+读。

### 6.4 三个反直觉的结论

1. **按 star 排序会选错。** 读写方向 star 最高的是 `ext4_rs`（72），但它 stable 编不过、没日志、不置脏；唯一真正合格的 `rsext4` 只有 8 ⭐。Rust 的 ext4 方向本身小众，star 中位数是个位数，没有区分度。有区分度的是 **commits**（590 vs 37 vs 10）、**下载量**（181.8k vs 14.7k）、**写路径有没有 journal**。
2. **`todo!()` 的数量会误导。** 22 处全在 FUSE 回调里，核心写路径是完整的——我最初据此给 `ext4_rs` 下的「写路径不完整」结论是错的。
3. **「新内容不能太长」是块边界的说法，不是字节数的说法。** 旧文件 7,900 字节占 2 个块（到 8,192），新内容写到 8,100 也不分配块，只改 `i_size`。

### 6.5 本期选型结论

| 用途 | 选定 | 版本策略 |
|---|---|---|
| 读 `grub.cfg` / `fstab` / 目录 | **`ext4-view`** | 锁 `=1.0.0` |
| 写 `grubenv`（1024B 定点覆盖） | **待 spike**：优先 `rsext4`（有 jbd2）；若它的 mount 开销/稳定性不达标，退到「`ext4-view` 只读 + 自写单块覆盖」（不动元数据，但绕过库） | 锁 `=0.9.x` |

spike 通过标准：① 能读出本机 `disk0s9` 的 52,760 字节 `grub.cfg`；② 写 `grubenv` 后进 Arch 跑 `e2fsck -fn` 干净；③ 只碰 1 个数据块。

## 7. 真机探测记录（2026-10-29，只读，脚本 `/tmp/ext4_probe.py`）

**结论：ext4 这条路完全可行，本机 10 个 Linux 分区全部 ext4（`magic=0xEF53`）。**

| 分区 | 卷标 | 大小 | `/boot/grub/grub.cfg` | `grubenv` |
|---|---|---|---|---|
| disk0s2 | （空） | 54.0G | 13,648 B · 09-03 | 1024 B |
| disk0s7 | ubuntu | 52.3G | 10,606 B · 08-31 | 1024 B |
| disk0s9 | arch-gui | 60.2G | **52,760 B** · 10-23 | 1024 B |
| disk0s10 | cachyos | 39.2G | 8,293 B · 10-25 | 1024 B |
| disk0s11 | fedora | 39.4G | **没有**（在 `grub2/`） | — |
| disk0s12 | mint | 39.5G | 9,495 B · 08-31 | 1024 B |
| disk0s13 | manjaro | 78.8G | 2,455 B · 10-12 | 1024 B |

`disk0s8`(disk2) / `disk0s16`(disk1) / `disk0s18`(home) 是纯数据盘（无 `/etc/fstab`），已排除。

**现存 ESP 的 FAT 卷序列（boot sector `0x43`）：**

```
REFIND      (s4)  = 4754-DCAD
ARCH-EFI    (s14) = 317F-37E5
WINDOWS-EFI (s3)  = 5652-564F
MacOS-efi   (s5)  = 3A57-EBA5
```

**定案 2 在本机的命中率是 0/7**：七个安装里只有 arch-gui 的 fstab 写了 `/boot/efi vfat UUID=E537-7F31`，而这个值不对应上面任何一个 ESP。原因是这些系统多数手动分区装的，Arch/CachyOS/Manjaro 用 systemd-boot 或直接 UKI，ESP 根本没进 fstab。**这正是要加 §5.3 第 3、4 级定位的原因。**

**`/dev/` 写法本机 0 例**（7 份 fstab 里 `grep -c "/dev/"` = 0，全部是 `UUID=`）。所以「兼容只写路径」这条**只能在单测里造用例覆盖**，真机无法验证。

## 8. 权限与安全边界

**实测**：`/dev/disk0s7` 是 `root:operator 0640`，当前用户不在 `operator` 组，`dd` 直接 `Permission denied`，`sudo -n` 也要密码。所以「枚举分区 → 判 ext4 → 读 fstab → 定位 grub 目录」这一整串**必须在 helper 里做**，App 只收结果。

**helper 现状**（`helper_proto.rs`，669 行，v1）：同一 app 二进制带 `--run-inbox` 以 root 跑，文件信箱在 `/var/spool/anyboot`；请求是 `{v, ops:[{guid,name,hex}]}`，`MAX_OPS=8`、`MAX_VALUE=512`、`MAX_REQUEST=64KB`、`PROTOCOL_VERSION=1`；文件头注释写着「只接受 OC 命名空间的 `OCBt*`，不执行 shell，不接受任意路径，别在别处放宽」。

**v2 要放宽的（用户已选 helper）**：

- `linux_probe`：**调用方不传路径**，helper 自己枚举分区，只回 `/etc/fstab`、`grub.cfg`、`grubenv` 三样（单文件上限 1 MB）；
- `linux_write_env`：**只允许写 grubenv**，必须恰好 1024 字节、必须以 `# GRUB Environment Block\n` 开头，写完立刻读回逐字节比对。

**协议形状必须改一处**：`grub.cfg` 实测最大 52,760 字节，远超 `MAX_VALUE=512`，回包不能走 `detail` 字符串 → **二进制载荷走 outbox 文件**，回包里只给相对路径 + 长度 + 哈希。

**最坏被滥用**：能读到一份 fstab 和一份 GRUB 配置，能改那 1024 字节的启动项。拿不到整盘，也拿不到别的文件。

**回传数据的敏感面**：本机 7 份 fstab 里有 6 条 `cifs` 写着明文 `password=a00000000`。所以 helper 只回**规范化后的挂载行**（`{spec_kind, spec_value, mountpoint, fstype}`），**不带 options**，日志里也不落原文。

**代价**：协议 v1→v2，已装的 helper 不认新 op，会回「协议版本不支持」，需要重装一次（一次密码）。

## 9. 模块结构与数据流（拟）

```
channels/linux_grub.rs        # 自动读取通道
├── PartSpec                  # UUID / PARTUUID / LABEL / device path 统一解析
├── BlockSource trait         # 裸设备：macOS /dev/rdiskNsM、Windows \\.\PhysicalDriveN+offset
├── LinuxRoot trait           # read(rel) / write_fixed(rel, bytes) / describe()
│   ├── HostFs                # Linux 本机：fs::read 直读，零驱动零提权
│   └── Ext4Root              # macOS/Windows：ext4-view 读 + 定点写
├── locate()                  # 三级定位 + grub.cfg root= 自证
└── preview() / switch()      # 产出 SimEntry 同构数据；写 grubenv（复用 grub.rs 的 selector 与 1024B 格式）
```

数据流：`locate()` → `Ext4Root::read("boot/grub/grub.cfg")` → `grub::parse_grub_cfg`（**原样复用**）→ 快照新字段 `linuxGrub` → 前端并排第二块 → 点击 → `write_fixed("boot/grub/grubenv", 1024B)` → 回读校验。

## 10. 落地 TODO

**阶段 0 · 骨架**
- [x] 需求与选型定稿（本文）
- [x] `channels/linux_grub.rs` 骨架 + `BlockSource` trait
- [x] `PartSpec` 解析器；单测覆盖 `UUID` / `PARTUUID` / `LABEL` / `by-uuid` 软链 / `sda1` / `vda2` / `nvme0n1p1` / `mmcblk0p1` / 6 字段带注释 / 挂载点含 `\040`

**阶段 1 · Linux 本机直读（零驱动、零提权，先把语义跑通）**
- [x] `HostFs` 实现：`fs::read("/boot/grub/grub.cfg")`，认 `grub` 与 `grub2` 双路径
- [ ] 独立 `/boot` 分区识别（读 fstab 判 `/boot` 是否单独挂载）
- [x] 写 `grubenv`：1024 字节原地覆盖 + 回读逐字节校验（复用现有 selector 生成逻辑）
- [x] **09_anyboot 冲突检测**：ESP 上存在 `/EFI/grubenv` → 提示「GRUB 会优先 `load_env -f` 那份，写这里不生效」
- [ ] 真机验证：改 `next_entry` → 重启 → 确认进的是那一项

**阶段 2 · macOS / Windows 读 ext4**
- [x] spike ①：读出 `disk0s9` 的 52,760 字节 grub.cfg（自研 `ext4_raw`，非 `ext4-view`，理由见 §13.1）
- [ ] spike ②：`grubenv` 定点写后，Arch 里 `e2fsck -fn` 干净、只碰 1 个数据块
- [~] helper 协议 v2：**这一版先用 `osascript with administrator privileges` 一次性提权子进程**，没有扩 helper，见 §13.2
- [x] macOS `BlockSource`：`/dev/rdiskNsM`
- [x] Windows `BlockSource`：`\\.\PhysicalDriveN` + 分区偏移，提权复用 `elevate.rs`（UAC）；
      偏移取 `Get-Partition` 的 `Offset`（**不是** `DeviceIoControl`），见 `ext4_raw::FileSource::open_physical`
- [ ] helper 重装流程与旧版本回退提示

**阶段 3 · 定位与 UI**
- [x] 三级定位 + `grub.cfg root=UUID=` 自证
- [x] fstab 解析（6 字段、`#` 注释、`\040` 转义、跳过 options）
- [x] UI 并排两块 + 数据源标识 + 置信度徽标（精确/推断）+「换一个」
- [x] `grub.cfg` mtime 展示（自动模式没人同步）
- [ ] 快照字段同步到网页端 / 手机端（`remote_routing.rs` 三条白名单已加，两端界面未做）

## 11. 风险与未定项

| 项 | 状态 | 说明 |
|---|---|---|
| btrfs | **待定** | Rust 侧无成熟只读实现；识别到即明确报不支持 |
| LUKS / LVM | 不支持 | 里面不是 ext4 字节流，本期直接放弃 |
| Windows 侧整体 | 需求存在，方案未细化 | 用户提到「windows/macos 利用这个驱动」；Windows 无 helper，只有 UAC |
| helper 能力放宽 | 已获同意，需落注释 | 「能读整块 Linux 分区」的边界要写进 `helper_proto.rs` 头注释与本文 §8 |
| 写 `grubenv` 前提 | 定案 | 文件必须已存在，**不代建**；不存在时给 `grub-editenv … create` 的命令提示 |
| 数据新鲜度 | 需 UI 体现 | 读到的是上次 `grub-mkconfig` 的产物 |
| `ext4-view` 不回放 journal | 已知限制 | `RECOVERY` 位认得但不 replay；对「看 `/boot` 有哪些条目」影响很小，对写则是前提条件（分区必须干净） |

## 12. 这次踩到的坑（供实现时避让）

1. **ext4 超级块魔数在偏移 `0x438`（1080），值是 `0xEF53`**。我一度写成 `0x53EF`，导致 10 个分区全被判成「不是 ext4」。
2. **macOS 的 `/dev/rdisk*` 要求按扇区对齐读写**，否则 `EINVAL`。读 FAT 序列（`0x43` 起 4 字节）不能直接 seek+read 4 字节，要先读整扇区再切。
3. **`s_last_mounted` 字段不可靠**：只有 rw 挂载时才更新，不能拿来推断「上次挂到哪」。
4. **ESP 不一定有 GPT 类型 `EFI System`**：本机 `REFIND`/`ARCH-EFI`/`WINDOWS-EFI` 的 GPT 类型都是 `Microsoft Basic Data`，靠分区类型判断会全漏；只能靠「FAT + 有 `/EFI` 目录」。
5. **macOS 对 FAT32 的 `Volume UUID` 是长格式**（`XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`），我在一个卷上观察到前 8 位与 FAT 序列相同 —— 但这是 macOS 自己的拼法，不作依赖，直接读 boot sector `0x43`。
6. **`RO_COMPAT` 位表**：`0x0200` 才是 `metadata_csum`，`0x0002` 是 `large_file`；位表写错会把健康分区报成异常特性。
7. **GRUB 里 `#` 是合法注释**，所以「补齐到旧长度」不会破坏语义 —— 这条只在将来若真要重写 `grub.cfg` 时用得上，本期不写 cfg。

## 13. 落地记录（2026-10-29 实装）

代码落在三个新文件上，约 3000 行，**全部只碰块设备：不挂载、不调外部工具、不写 cfg**。

| 文件 | 职责 |
|---|---|
| `src-tauri/src/channels/part_spec.rs` | `PartSpec` 六种写法的统一解析与比较；fstab 六字段解析 |
| `src-tauri/src/channels/ext4_raw.rs` | 极简 ext4 只读解析（超级块 / inode / 目录块 / extent 树）+ 定长原地写 |
| `src-tauri/src/channels/linux_grub.rs` | 通道主体：提权探测、三级定位、视图组装、写 env、三个命令 |

### 13.1 选型改判：没用 `rsext4`，自己写了解析

§6 选的是 `rsext4`（star 最少但唯一合格）。真写的时候改成了自研 `ext4_raw`，理由只有一条：

> 这一期只需要读三个文件（`etc/fstab`、`boot/grub/grub.cfg`、`boot/grub/grubenv`）
> 和覆盖一个 1024 字节的环境块。为这点需求引一个通用文件系统库，等于把
> 「我只碰这 3 个文件」这条安全边界交给别人的代码去守。

自研版的能力边界正好等于需求边界：只认 `extent`（不认 `indirect block`）、只读不解压、
不碰 journal，所以 `RECOVER` 位挂着时**直接拒绝写**（只读照常）。约 1000 行，7 项单测。

### 13.2 提权路径：先 osascript 落地，当日收口到 helper（v2）

定案第 3 条是「macOS 走 helper」。第一版为了先把语义跑通，走的是
`osascript -e 'do shell script "…" with administrator privileges'`：每次读盘弹一次授权。

当时的判断「`helper_proto.rs` 不存在」**是错的** —— 它一直在，只是协议 v1 只能写
`OCBt*`（OpenCore 的默认启动项），没有「读块设备」这种任务。所以不是从零做助手，
而是**扩协议**。同一轮就收口了，见 §14。

两版共有的代价与真相：
- 一次性提权只拿得到一次结果 → `refresh()` 里**只探一次**，视图与写盘上下文
  （`runtime_from`）从同一份 payload 出，切启动项时不二次弹框；
- 用户点「取消」时 `osascript` 返回 `-128`，已单独识别成「已取消授权」，不当报错；
- 两种路径**共用同一个写入函数** `write_env_block` —— 免得「免密模式干净、
  弹密码模式不干净」这种差异随时间去漂。

### 13.3 真机数据（本机 disk0 · 2TB · 10 块 ext4）

一次探测 2.3 秒读完 10 块分区，其中 7 块有 grub.cfg：

| 分区 | 卷标 | 发行版 | grub.cfg | env | root=UUID 自证 |
|---|---|---|---|---|---|
| disk0s2 | （无卷标） | endeavouros | `boot/grub/grub.cfg` 13,648 | 1024 | ✓ |
| disk0s7 | ubuntu | ubuntu | `boot/grub/grub.cfg` 10,606 | 1024 | ✓ |
| disk0s9 | arch-gui | arch | `boot/grub/grub.cfg` **52,760** | 1024 | ✓ |
| disk0s10 | cachyos | cachyos | `boot/grub/grub.cfg` 8,293 | 1024 | ✓ |
| disk0s11 | fedora | fedora | **`boot/grub2/grub.cfg`** 9,547 | 1024 | ✓ |
| disk0s12 | mint | linuxmint | `boot/grub/grub.cfg` 9,495 | 1024 | ✓ |
| disk0s13 | manjaro | manjaro | 2,455（**不是 grub 配置**，见 13.5） | 1024 | ✗ |

`disk0s8` / `disk0s16` / `disk0s18` 是纯数据分区，没有 cfg，正确跳过。

**fedora 那一行的 `grub2` 路径**就是双路径分支存在的唯一理由 —— 少认这一个目录，
七块分区里就有一块永远是空的。

### 13.4 「按 fstab 找 ESP」的真实命中率：7 块里只有 1 块

选 `ARCH-EFI`（FAT 序列 `E537-7F31`）→

```
定位：confidence=exact target=disk0s9
说明：disk0s9 里的 fstab 写着 UUID=E5377F31 挂在 /boot/efi —— UUID/PARTUUID 精确命中
```

其余 6 块分区的 fstab 里**根本没有 ESP 这一行**（`/boot` 就在根分区里，ESP 只在安装
时挂过一次、没写进 fstab）。选 `WINDOWS-EFI` / `REFIND` 时同样无人匹配。

这不是 bug，是定案的直接后果：「按 fstab 找」只在「这个发行版把 ESP 挂进了 fstab」
时成立。所以三级定位的第三级（唯一有 cfg 就用它）和「列候选让人点」必须是正经功能，
不能当兜底敷衍 —— 本机就是靠它才能读另外 6 块。

### 13.5 真机发现：`grub.cfg` 存在 ≠ 它是 grub 配置

`disk0s13`（manjaro）的 `boot/grub/grub.cfg` 里躺的是一段**中文笔记**
（「主菜单中grub解析的列表增加一项…原始-简易-中等-详细」），2,455 字节纯文本，零个 `menuentry`。

如果只是安静地渲染 0 个条目，用户看到的就是「这程序瞎了」。现在 `build_view`
解析不出任何 `menuentry` 时会往 `issues` 推一条明说此事，
单测 `cfg_without_menuentry_is_reported` 锁住这条行为。

### 13.6 UI 落地：并排两块的响应式边界

`#grub` 里 `#grub-list`（手动解析）与 `#grub-auto`（自动读取）被包进 `#grub-dual`，
`grid-template-columns: repeat(auto-fit, minmax(320px, 1fr))`：

- **加宽 / 列表独立 / GRUB 独立**：宽度够，两块真正左右并排；
- **内嵌（默认 460px 列）**：两块各需 320px，装不下，自动上下堆叠。

这是有意的：硬挤两列会把内核名压成三行，比堆叠更难读。要强制并排把 320 改成 220 即可
（代价是每列约 220px）。

另有三处配套：
- 模式 ③「列表独立」会把 `#grub-list` 搬进右栏，`applyGrubLayout` 里补了一句
  「非 list 模式把它插回 `#grub-dual`」——否则搬出去就再也回不来；
- 「列表格式」下拉共用同一份偏好，右块用缓存的上次结果立刻重画（不重扫、不重弹授权）；
- 右块**不做开机自动扫**：macOS 每次都要弹密码框，属于骚扰。

### 13.7 本轮没做的事

- ~~**Windows 侧**：`\\.\PhysicalDriveN` + 分区偏移~~ —— **2026-10-29 补上了**：
  `probe_raw` 枚举 `partitions::all()` 里的分区，按 `Offset` 开整盘读，解析与 macOS
  共用同一份 `scan_ext4`；写 `grubenv` 走同一条路（`write_env_block` 把 `disk1p3`
  反查回偏移）。偏移来自 `Get-Partition`，**不需要 `DeviceIoControl`** —— 当初的
  方案里这句是多余的，分区表缓存已经有了。
- **ESP 身份必须包含 FAT 卷序列**（同日修正）：「所选 ESP → 哪块 Linux 分区」这一步
  靠的是 fstab 里那行 `UUID=E537-7F31 /boot/efi vfat …`，而这个 UUID 是 **FAT 卷序列**，
  不是分区 GUID。Windows 版最初只填了 `part_uuid`，`fs_uuid` / `label` 留 `None`，
  于是精确匹配永远不中 —— 界面只会说「没定位到你选的 ESP 对应的 Linux 分区」，
  而真相是「没有钥匙」。现在与 macOS 同路：在提权子进程里读 ESP 首扇区，
  按簇数判 FAT12/16/32（序列号 12/16 在 `0x27`、32 在 `0x43`，卷标随之在 `0x2B`/`0x47`），
  解析直接复用 `esp::fat_boot_sector`，不另写一份。
- **枚举口径也向 macOS 看齐**：不再只筛 `0fc63daf`。XBOOTLDR（`bc13c2ff…`）与发行版
  自定义类型里同样可能是 ext4 的 `/boot`，硬筛会出现「磁盘管理里看得到、程序里扫不到」。
  现在按 `Linux 类型优先 + 其余兜底` 的顺序逐块试，靠 `Ext4::open` 判定，
  非 Linux 类型打不开时**不刷问题列表**（MSR / 恢复分区本来就没有文件系统）。
- **btrfs / LUKS / LVM**：识别到即明确报不支持，不静默跳过。
- **写 env 的 e2fsck 复验**：代码里有回读逐字节比对，但没有跑过 `e2fsck -fn`；
  Arch 里的验证留待下次重启时一起做（改 `next_entry` → 重启 → 确认进的是那一项）。
- **Windows 裸写吃 os error 5**（2026-10-30 真机）：读探测全绿、一写环境块就
  「写设备失败：拒绝访问」。根因是 MSDN「Blocking Direct Write Operations」那条
  规矩 —— 对 `\\.\PhysicalDriveN` 的裸写，凡落在**已挂载卷**扇区上的都是
  `ERROR_ACCESS_DENIED(5)`；「已挂载」与盘符无关，ESP 的 FAT 卷没分配字母也是
  mounted。解法是官方流程：`volume_lock::lock_extent` 用
  `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` 逐卷比对盖住目标偏移的卷，
  `FSCTL_DISMOUNT_VOLUME`（必要）+ `FSCTL_LOCK_VOLUME`（尽力）后写，
  RAII 放开，系统随后自动重新挂载。只挂在写路径（`write_env_block` Windows 版），
  只读探测不锁卷。

## 14. 收口：helper 协议 v2 与内嵌并排（2026-10-29 同日晚）

用户追加「都做了」= ① 补 helper ② 内嵌模式也强制并排。

### 14.1 先纠正一条事实：helper 一直存在

上一节写的「`helper_proto.rs` 不存在」是错的。真实情况：

| 件 | 状态 |
|---|---|
| `helper_proto.rs`（669 行） | 在。协议 v1 + 文件信箱 + `run_inbox` |
| `helper_install.rs`（692 行） | 在。装 / 卸 / `launchd WatchPaths` / sha256 判「是不是当前构建」 |
| `/Library/PrivilegedHelperTools/com.example.anyboot.helper` | **装着的**（10-28 装过一次） |

缺的只是「助手能做的事」：v1 的 `validate` 只放行 OpenCore 私有命名空间下的
`OCBt*`，一条 `Op` 就是「写一个 NVRAM 变量」。所以这一轮不是从零做助手，是**扩协议**。

### 14.2 协议 v2：三种任务，一个扁平结构

```rust
pub enum Kind { Nvram, LinuxProbe, LinuxWriteEnv }

pub struct Request {
    v: u32, kind: Kind,
    ops: Vec<Op>,        // Nvram
    esp: Option<String>, // LinuxProbe：所选 ESP 的 PARTUUID（只是查询条件）
    dev: Option<String>, // LinuxWriteEnv：disk0s9
    rel: Option<String>, // LinuxWriteEnv：只能是两个常量之一
    hex: Option<String>, // LinuxWriteEnv：1024 字节
}
```

**为什么不用 serde 的 internally-tagged enum**：`deny_unknown_fields` 对
internally/adjacently tagged 枚举不生效（serde 的已知限制）。而「请求格式固定、
多一个字段就拒」正是这一层要守住的东西 —— 静默忽略一个 `dev` 字段，等于让
「你以为写了 A，其实写了 B」有机会发生。扁平结构 + 显式交叉校验把这件事做死了。

**大载荷走 blob，不进回执**：Linux 探测一次 135 KB（十块分区、七份 grub.cfg）。
塞进回执等于每条回执都几百 KB。现在：

```
助手：执行 → 写 outbox/<stem>.blob（先 .part 再 rename）→ 写 outbox/<stem>.json（带 blob 名）
App ：轮询到 .json → 按名字读 .blob → 删掉
```

顺序是硬要求：**回执出现就是「blob 已完整」的信号**，反过来的顺序会让 App 偶尔
读到半个文件 —— 那种错只在快机器上偶发，最难查。

### 14.3 放宽了什么 / 没放宽什么

放宽（这是 ack 过的边界，写在 `helper_proto.rs` 头注释里）：

- 能读**所有 ext4 分区里的三个固定相对路径**：`etc/fstab`、`boot/grub{,2}/grub.cfg`、
  `boot/grub{,2}/grubenv`；
- 能把 `grubenv` 的 1024 字节原地覆盖。

**没有**放宽（这几条是边界，不是「暂时」）：

| 约束 | 实现 |
|---|---|
| 请求里没有路径参数 | `LinuxProbe` 不接受 `dev/rel/hex`；读什么由助手自己枚举 |
| 写路径只有两个常量 | `LINUX_ENV_RELS = ["boot/grub/grubenv", "boot/grub2/grubenv"]` |
| 设备名只认 `diskNsM` | `valid_disk_dev`：拒 `/dev/` 前缀、拒 `rdisk` 前缀、拒整盘、拒 `s0`、拒前导零 |
| 环境块必须正好 1024 字节 | 1023 / 1025 都拒 |
| 交叉字段一律拒 | `Nvram` 带 `dev`、`LinuxWriteEnv` 带 `esp`、`LinuxProbe` 带 `dev` 全部报错 |
| 不执行 shell、不跟软链接 | 沿用 v1 |

`valid_disk_dev` 里有一条值得记：**盘号 `0` 本身合法**。第一版写「不以前导零开头」，
把 `disk0s9` 一起拒了（单测当场抓到）。

### 14.4 分层：协议不碰设备

```
helper_proto.rs   收请求 / 校验 / 回执    —— 纯逻辑，任何权限任何平台都能单测
   ↓ trait Executor
helper_exec.rs    真正碰设备              —— 只在 root 助手进程里跑，一行单测都没有
```

`run_inbox(&dyn Executor)` 收一个执行器；生产传 `helper_exec::System`，
单测传 `FakeExecutor`。端到端测试因此能同时覆盖三条任务 + 两个坏请求，
而**不需要 root、不需要真设备**，还能断言「坏请求一次都没到达执行层」
（`executor.calls == 3`）。

`apply_nvram` 从 `helper_proto` 搬到 `helper_exec` 是这次分层的直接结果：
协议层一行设备代码都不该有。

### 14.5 超时：探测不能和写变量用同一个值

| 任务 | 超时 | 理由 |
|---|---|---|
| `Nvram` / `LinuxWriteEnv` | 4 s | launchd 唤起 + 一次 `nvram`，实测 <300 ms |
| `LinuxProbe` | **25 s** | 真机暖缓存 2.3 s，冷启动 + 十块盘寻道可到十几秒 |

用 4 s 的话，一块大分区就能让整个扫描被判成「助手没响应」，然后回落去弹密码 ——
用户看到的是「免密又坏了」。

### 14.6 旧助手：必须说出来，不能装作免密

App 升级后 `/Library` 里还是上一版助手（协议 v1，不认 `kind`）。处理：

```rust
if helper.installed && !helper.stale { 走 helper }
else { 走 osascript；若 installed && stale → issues 里推一条 }
```

那条 issue 写的是「免密助手还是上一个版本的：这次用了一次性授权。在 OpenCore
卡片里把「免密模式」关掉再开一次，就能恢复免密。」—— 不说明的话，用户看到的就是
「明明开着免密，怎么又弹密码了」。

### 14.7 真机验证（v2 全链路）

用临时信箱（`ANYBOOT_HELPER_SPOOL`）+ root 跑 `--run-inbox`，不碰
`/var/spool/anyboot`，也不碰已装的那份旧助手：

```
=== LinuxProbe ===
回执 {"ok":true,"detail":"读取完成（134979 字节）","blob":"0-1-....blob"}
blob 134,979 字节；disk0s9 envHex 2048 字符；cfgText 51,075 字节

=== LinuxWriteEnv（把 disk0s9 的 grubenv 原样写回）===
回执 {"ok":true,"detail":"已写入 disk0s9:boot/grub/grubenv（1024 字节，回读一致）"}

=== 写后重新探测的对比 ===
envHex 逐字节一致: True
grub.cfg 也一致: True
分区数: 10 -> 10，清单一致: True
环境块头部: b'# GRUB Environme'
```

写的是**与原来完全相同的内容**，所以这条验证的是：能定位 inode、能原地覆盖、
回读一致、并且除那 1024 字节外什么都没动。

### 14.8 UI：内嵌模式也并排

原来 `repeat(auto-fit, minmax(320px, 1fr))` 在内嵌 460px 列里装不下两块，自动堆成上下
—— 等于「并排」只在加宽模式存在。改成：

```css
#grub { container-type: inline-size; }          /* 内嵌列宽与视口无关，只能按容器判 */

.grub-dual {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));  /* 默认就并排 */
}

@container (max-width: 420px) {                   /* 真的窄才堆叠 */
  .grub-dual { grid-template-columns: minmax(0, 1fr); }
}
```

- 内嵌 460px 列下每块约 210px：挤，但比上下堆叠少滚一屏；
- `minmax(0, 1fr)` 而不是 `1fr`：长内核名换行而不是撑破列宽（`1fr` 的隐含下限是
  `min-content`，一个长名字就能把列顶宽）；
- 脚注按 `view.via` 如实说：`helper` → 「本次由免密助手读取，没有弹密码」；
  `elevated` → 「本次是一次性授权读取（弹了密码）。开启免密模式后不再弹」；
  空 → 「每读一次都要管理员授权」。

### 14.9 还没做的

- ~~**Windows 侧仍不支持写 ext4**~~ —— 2026-10-29 已接上：`disk1p3` → `Offset` → 整盘读写，
  与 macOS 共用 `overwrite_in_place`。**真机验证仍缺一台双系统机器**（开发机上没有
  ext4 分区，这条路一次都跑不到，现阶段只有 `parse_win_dev` 的形状测试兜着）；
- **没跑 e2fsck**：macOS 上没有 ext4 工具，一致性靠助手自己的「写完整块回读逐字节比对」；
- **没重启验证 `next_entry` 真的生效**：这一步只能人工重启确认；
- **装到 `/Library` 的新助手还要用户点一次**：升级后 `stale=true`，需在 OpenCore 卡片
  里关一次再开；之后新装机器上第一次点也会弹一次密码（写 `/Library` 必须 root）。

### 14.10 收口时抓到的两个真问题

**① `linux_grub_switch` 返回了 `AppStatus`（rEFInd 的状态结构）**

实测返回：

```json
{"cmd":"linux_grub_switch","ok":true,"result":{
  "message":"已连接 rEFInd；PreviousBoot 未匹配模板",
  "variables":[{"displayName":"Linux",...},{"displayName":"Windows",...},...],
  "varsDir":"/Volumes/REFIND/EFI/refind/vars"}}
```

`switch` 内部写 grubenv 是成功的（随后重扫确认 `saved_entry`/`next_entry` 都变了），
但返回值是 rEFInd 的状态对象，`message` 是**上一次 rEFInd 操作**留下的那句。

为什么一直没暴露：前端 `switchGrubAuto` 忽略了返回值，只把摘要清空。但网页端
`/api/ui-command` 会把它原样吐出去 —— 任何一个走 HTTP 的调用方都会读到一句
完全无关的话。

改成返回 `LinuxGrubView`（和 `scan` / `pick` 一致），前端直接用它重画。

**② 「切完再扫一次」在 osascript 模式下 = 再弹一次密码框**

原实现成功分支里有 `let _ = refresh(&app, None)`，注释写的是「不弹新授权：
这次走的是同一份缓存路径」。**注释是错的**：`refresh()` 第一件事就是
`probe()`，在没装助手的机器上那次 probe 会再拉起一次授权框。

现在改成不重扫：只写了 1024 字节 env，`grub.cfg` 没变，没必要再枚举分区。
`saved_entry` / `next_entry` 直接用**刚写进去的那块**回填，并把新块存回
`Runtime.env_bytes` —— 这一条也是必须的：连续切两次时，第二次要是不基于新块算，
第一次写的 `saved_entry` 会被旧块覆盖回去。

### 14.11 实测：为什么 4 秒超时必须改

| 场景 | 耗时 |
|---|---|
| 临时信箱 + root 直接跑 `--run-inbox`（首轮） | 2.3 s |
| 同一份请求，经 launchd 冷起动 | 4.50 s |
| 界面「重新读取」（HTTP → helper） | 4.59 s / 3.42 s（暖缓存） |

4.50 s 已经越过原来的 `REPLY_TIMEOUT = 4s`。如果没把探测超时单独放宽到 25 s，
第一次扫描就会被判「助手没响应」→ 回落弹密码 → 用户以为免密坏了。
这个数字不是估的，是收口当天真机跑出来的。

## 15. 再改：ESP 提为公共区 + 手动/自动改成横向标签（2026-10-29）

用户一句话定的版式：

> grub 中的 efi 相关是公共区域 自动和手动分为两个横向标签 默认进自动标签

### 15.1 为什么要「公共区」这层结构

上一版把两块**并排**（§14），隐含了一个错误假设：手动和自动是两条平行链路、
各自带自己的配置。实际上「哪块 ESP」是**两条链路共同的前提**：

- 手动解析读的是这块 ESP 上的 `/EFI/grub.cfg` 与 `/EFI/grubenv`；
- 自动读取拿这块 ESP 的 FAT 序列（或 `/dev/xxx`）去各 Linux 分区的 fstab 里反查。

把 ESP 目录 / 挂载 / 分区 UUID / 同步放进任何一个面板，都会让另一种模式
找不到配置入口。所以它是 `#grub-esp`，站在标签之外，对两边同时可见。

### 15.2 分界线：什么归手动标签

| 内容 | 归属 | 理由 |
|---|---|---|
| ESP 目录 / 挂载 / 分区 UUID / 同步 / 空状态 | **公共** | 两条链路都要先答「哪块 ESP」 |
| 列表（`#grub-list`） | 手动 | 数据源是 ESP 上的 cfg |
| 安装 / 卸载 `09_anyboot`、内置脚本、执行过程 | 手动 | 只有手动链路需要这个东西 |
| 引导配置须知（os-prober / EFI 顺序） | 手动 | 讲的是「怎么让 ESP 上那份 cfg 有别的系统」 |
| 自动读取面板整体 | 自动 | — |
| 展示方式 / 列表格式 / 提示与错误 | **公共** | 展示方式挪的是面板本身，与选了哪个标签无关 |

### 15.3 默认进自动，而且**不记住**上次选择

`grubTab` 是纯前端状态，不落盘。要是按「记住上次」做，第二次打开就落在手动，
正好违背「默认进自动」。代价是每次启动都回到自动 —— 这是用户要的。

### 15.4 搬 DOM 的两阶段（这版最容易写错的地方）

`applyGrubLayout` 原来是一趟 `appendChild`，现在必须分两阶段：

```text
① 该离开 #grub 的先搬走（pane：全部；list：只搬手动面板）
② 留下的按出厂顺序 insertBefore 回去（倒序插，anchor = 已就位的下一个兄弟）
```

一趟 `appendChild` 会把节点全堆到末尾 —— 手动面板会跑到「执行过程」「提示行」
后面。这个坑上一版靠一句特判（`grubDualElement.insertBefore`）绕过去了，
标签化之后特判失效，必须换成正经的顺序复原。

`#grub-panel-manual` 现在自己就是被搬的单位（列表在它里面），
所以 `grubCanonicalOrder` 只认面板、不再认 `#grub-list`。

### 15.5 「列表独立」模式下标签与右栏的关系

list 模式下把**整个手动面板**搬进右栏。此时：

- 手动面板不在 `#grub` 里 → `syncGrubTabs` **不给它设 hidden**，
  否则右栏会被清成白板；
- 切到「自动」标签时，左栏显示自动面板、右栏仍显示手动列表。
  这是有意的：list 模式的语义就是「列表独占右栏」。

### 15.6 `[hidden]` 必须手写一条

`.grub-panel { display: grid }` 是作者样式，会盖掉 UA 的 `[hidden] { display: none }`
—— 不补 `.grub-panel[hidden] { display: none }`，切标签时两个面板会**同时显示**。
（`.grub-empty-state[hidden]` 早就吃过这个亏，抄了同一条。）

### 15.7 顺手修掉一个一直存在的排版 bug

`.grub-advice strong { display: block }` 本意是让标题「引导配置须知」独占一行，
但它同时把条目里的**句中强调**也块级化了 —— `os-prober`、`一个`、`boot, efi`、
`EFI 第一启动项` 各自被挤成单独一行，一句话断成三截。

这段文字以前没人正眼看过（须知在页面底部，且改版前也没有截图机会）。
改成 `.grub-advice > strong`（只认标题）+ `.grub-advice li strong`（句中强调）后，
三条须知各自连贯成句。

### 15.8 视觉验证：无头 Chrome（这一轮终于有截图）

`screencapture` 在本会话一直报 `could not create image from display`（拿不到显示权限）。
改用 **Chrome headless 软件渲染**，不需要显示权限：

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless=new --disable-gpu --hide-scrollbars \
  --window-size=1280,1800 --virtual-time-budget=4000 \
  --screenshot=/tmp/grub_tabs.png \
  "file:///Users/jianghh/IdeaProjects/anyboot/ui/index.html"
```

配合一份临时页（复制 index.html、尾部加 1.2 s 后 `.click()`）验证了切标签后的状态，
临时页验完即删。实际确认到的：

- ESP 公共区在标签之上、带左侧竖线，两条读取方式共用；
- 标签默认选中「自动」（浅底 + 加粗 + 强调色下划线）；
- 点「手动解析」后：说明文字 → 空列表 → 内置脚本折叠 → 引导配置须知，
  上方 ESP 公共区不动；
- 窄窗（520px）下标签与面板都不溢出错位。

**注意**：`file://` 打开时走的是「纯预览」分支（无 Tauri、无后端），
列表里没有真实数据；布局与配色是真的，条目内容是假的。真机数据靠 HTTP 接口验（§14）。

### 15.9 真实数据下的交互断言（CDP）

光有静态截图不够 —— 标签能不能点、点了会不会两个面板一起露出来，得真点一次。
用 Chrome 的调试端口 + Node 22 内置的 `WebSocket`（零依赖）直接驱动页面：

```text
初始：        visible=[grub-panel-auto]     selected=[auto]   手动列表已渲染 60 条
点「手动解析」：visible=[grub-panel-manual]   selected=[manual]
点回「自动」：  visible=[grub-panel-auto]    selected=[auto]
按 →（键盘）： visible=[grub-panel-manual]   selected=[manual]
```

「任意时刻有且只有一个面板可见」这条一直是成立的，`manualHidden=true`。
顺带确认了两件容易被忽略的事：

- **手动面板 hidden 时内容也已经渲染好了**（60 个条目），切过去是瞬间的，
  不用等第二次拉快照；
- `#grub` 的直接子节点顺序与 §15.2 的分工完全一致：
  `head → esp → tabs → panel-auto → panel-manual → mode-row → format-row → hint → error`。

中间踩过一次假警报：用 `chrome --dump-dom` 抓页面时看到手动面板**没有** hidden 属性，
一度以为 `syncGrubTabs` 判断错了。改用 CDP 在当前页面求值，得到的是 `manualHidden: true`
—— `--dump-dom` 抓的是渲染过程中的某一帧，不是稳定态。**结论：验证 DOM 状态要用
CDP 求值，不要用一次性 dump。**

## 16. 自动读取合成一个框（用户定案 2026-10-29）

用户截图指出「为啥会有两个框」并给出做法：**自动模式的数据都放进第一个框**。

### 16.1 原来那两个框是什么

不是「标签 + 面板」，是自动面板内部两块各自带 `1px` 边框的兄弟块：

```text
① .grub-auto-target   分区明细（发行版 · UUID · 容量 / cfg 路径 · 大小 · 改于 / env …）
② .grub-list          启动项列表（本身也是滚动容器，inline 模式限高 45vh）
```

两块上下叠着、都带圆角和 `--input-bg` 底色，读起来像框里套框，纵向还被
两圈边框 + 两个 8px 间距切掉约 30px —— 在 460px 宽的小窗里很值钱。

### 16.2 改法

结构上加一层 `.grub-auto-card`，把 ①② 收进同一个框：明细当**框头**（`border-bottom`
分隔线），列表当**框身**（边框/底色/圆角全部交出去）。

```text
#grub-auto
├── .grub-auto-summary      一句话结论（也在出错时当报错出口）
├── .grub-auto-issues       问题列表（空菜单 / 没有 grubenv …）
├── .grub-auto-card         ← 唯一的框
│   ├── .grub-auto-target   框头（hidden 时连分隔线一起消失）
│   └── #grub-auto-list     框身（滚动容器，限高原样保留）
├── .grub-auto-candidates   候选分区（点一块切过去，仍是行按钮）
└── .grub-auto-foot         脚注
```

两个必须写下来的坑：

1. **`.grub-auto-card` 不能加 `overflow: hidden`。** 想用它裁圆角很自然，但
   `overflow:hidden` 会把「最近滚动容器」抢成自己，③④（列表独立 / GRUB 独立右栏）
   模式下 `.grub-entry.is-pinned` 的 sticky 就钉不住 `.pane-body` 了 —— 那正是
   §13 专门修过的「选中项置顶」。框内 4px 内边距已经让行不贴角，不需要裁。
2. **`display:grid` 会盖掉 UA 的 `[hidden]{display:none}`。** `.grub-auto-target`
   是 grid 容器，`hidden` 一直没生效过：没有目标分区时会留一条空的带边框细条
   （14px 高）。这次顺手补了 `.grub-auto-target[hidden]{display:none}`，分隔线用
   `:not([hidden])` 才画。

### 16.3 验证

临时页（复制新结构 + 22 条假条目 + 7 个候选分区）跑无头 Chrome，再按应用窗口宽度
（476px）截图核对：**带完整边框的卡片只剩一个**，框头三行明细、下面一条分隔线、
再往下是启动项列表，列表右边缘贴框沿但不越界。

CDP 求值（inline 模式）：

```text
cardBorder=1px  cardRadius=8px
listBorder=0px  listBg=transparent   listOverflowY=auto
listScrollH=615 > listClientH=411    → 列表仍自己滚（限高还在）
滚动 120px 后 pinned 行 top 仍等于「列表顶 + 4px」 → sticky 置顶没被破坏
targetBottomBorder=1px               → 有明细时才有那条分隔线
```

高度与改前一致：框头原本就占那么多，只是从「框外」挪到「框内」，宽排（wide）
的 `calc(100vh - 380px)` 不需要跟着调。

### 16.4 没动的地方

- **候选分区列表仍留在框外**（一块块窄行按钮，`精确/推断` 标记 + 当前项描边）。
  它是「换一块分区读」的操作入口，不是数据；框里只放「这块分区的数据」。
  真机 7 个发行版里只有一个把 ESP 写进了 fstab，自动定位常常判成「不唯一」，
  这个入口不能藏。
- 手动解析面板不动：它读的是 ESP 上那一份 `/EFI/grub.cfg`，没有「分区明细」这种东西。

## 17. 候选分区折成一行小字 + 手选的回头路（用户定案 2026-10-29）

用户要「把它折叠成一行小字」，同时问「选择框可以选择了吗 / 选择之后的逻辑」。

### 17.1 折叠

`<details>` 包住原来那串候选分区行，收起时只剩一行 11px 灰字：

```text
▸ 换一块分区读 · 7 块有 grub.cfg            ← 收起
▾ 换一块分区读 · 7 块有 grub.cfg · 当前手选 disk0s9   ← 展开（手选时多一段）
```

- `summary` 里的「N 块有 grub.cfg」用 `candidates.filter(c => c.cfgRel)` 现算 ——
  真机上有些分区（manjaro 的 disk0s13 就是）里面是段中文笔记，不是 cfg。
- **`confidence === "none"` 时自动 `open = true`**：一块都定位不到的时候，
  这行是唯一出路，不能还让人先点一下。
- 其余情况只在 `renderGrubAuto` 里改文案，**不碰 `open`** —— 否则每次重画
  （切条目、写 env 回来）都会把用户展开的列表又收起来。
- 有个 CSS 坑：`<details>` 的 `[hidden]` 也一样被作者样式压不住，需要显式
  `.grub-auto-picker[hidden]{display:none}`。

### 17.2 选择框能选吗：能，链路是这样

```text
点分区行
  → invoke("linux_grub_pick", { dev })
  → 后端 settings.linux_grub_dev = dev（落盘）+ refresh(Some(dev))
  → locate() 第一分支命中 prefer_dev → confidence = "manual"
      备注「按你选择的磁盘（disk0s9）读取」
  → 视图重画：摘要显示该分区 / 框头换成它的明细 / 列表换成它的菜单
```

选了之后**不是一次性的**：`refresh()` 在 `target` 为空时读的就是
`settings.linux_grub_dev`，所以「重新读取」、切标签、重启 App 全都按这块分区走。
这是有意的 —— 用户点它就是在说「以后就读这块」。

副作用是「按 ESP 的 fstab 自动定位」被长期顶掉，而界面上原本**没有任何入口能
清掉** `linux_grub_dev`（只能改配置文件或重装）。所以这次补了一条：

```text
手选状态下，候选列表末尾多一行虚线按钮：
「恢复自动定位（按 ESP 的 fstab 重新找）」→ pickGrubAuto("") → dev 存空串
```

`locate()` 里 `prefer_dev.trim()` 为空的判定本来就有（`filter(|v| !v.trim().is_empty())`），
空串天然等于「没手选过」，后端不需要新命令。补了一条单测锁住这个语义：
`locate_ignores_blank_manual_choice`（`""` 与 `"   "` 都必须落回 `exact`）。

### 17.3 顺手对齐的细节

- 候选行左侧标记原来只有「精确 / 推断 / —」。手选中的那块后端给的是
  `matched = "manual"`，前端原来把它显示成「—」，看着像「啥也不是」。现在显示
  **手选**（链接色，与绿色的精确、灰色的推断区分开）。
- 选中行的描边（`is-current`）与标记是两个维度：手选那块也照样描边，
  一眼能看出「现在读的就是它」。
- 手选后若那块分区没有 grub.cfg，走的是原有的 `error` 分支（摘要变红 + 写明
  「没找到 grub.cfg（找过 boot/grub 与 boot/grub2）」），不会静默给个空列表。

### 17.4 验证

无头 Chrome 按 476px 窗宽截图，两种态都核对过：收起时是卡片下面一行带三角的
小字，与卡片左对齐；展开后 7 行候选 + `手选` 标记 + 底部虚线按钮，
`is-current` 那行描边正常。`node --check ui/app.js` 通过；
`cargo test --lib channels::linux_grub` 14 passed（新增 1 条）。

## 18. 「免密怎么又弹密码」与「选了没回显」（用户报障 2026-10-29）

两条报障其实是两个独立的坑，一条后端一条前端。

### 18.1 为什么扫描 / 选分区又弹密码了

不是助手坏了，是**门禁判错了**。原来的优先级判断是：

```rust
if helper.installed && !helper.stale { 走助手 }   // 否则回落 osascript
```

`stale` 是拿 `/Library/PrivilegedHelperTools/com.example.anyboot.helper` 的 sha256
和**当前 App 可执行文件**比。而装过去的助手就是 App 二进制的副本 —— 所以
**每次重新构建/升级 App，助手立刻被判成「旧版本」，之后所有读取与写入都会弹密码**。
今天这次报障就是这么来的：12:04 装了助手，12:13 我又构建了一版，摘要一比对就
对不上（两者文件长度还都一样，25407424 字节 —— 正是 `helper_is_stale` 注释里
担心的那种「长度一样、内容不同」）。

真机实测证明助手本身完全正常（同一台机器，绕过门禁直连）：

```text
live_helper_probe：回包 129326 字节，耗时 4.18s，10 块 ext4 / 7 块有 grub.cfg
```

**改法：把门禁换成协议版本，而不是 digest。**

- `helper_ready()` 只看 `view().installed`；
- 真正的契约是请求里的 `v`：助手侧 `helper_proto::validate()` 一上来就比
  `request.v != PROTOCOL_VERSION`，对不上直接拒绝并回「协议版本不支持」，
  **不会闷头按旧语义乱写**（这正是当初加 digest 门禁想防的事）；
- 于是：同协议、不同构建 → 照常免密；协议不同 → 助手明确拒绝 → 调用方把真实
  原因报出来，用户重装一次即可。开发期每天重构建几十次也不会再退化成弹密码。
- `view().stale` 保留，但**只用来提示**「建议重装」（OpenCore 卡片那句文案也改了：
  「协议一致，照常免密。想消掉这条提示就点「重装」」）。
- 回落时不再说「免密助手还是上一个版本」这种猜原因的话，改成带上真实原因：
  `免密助手这次没接上（超时 / 作业没加载 …）：改用了一次性授权`。

### 18.2 为什么「选了没回显」

三个原因叠在一起，都在前端：

1. `pickGrubAuto` 一点就把整块候选列表 **`hidden = true` 并且 `open = false`**；
   而探测一次要 4~5 秒（免密），弹密码时更久。这段时间界面上等于什么都没发生，
   回来之后列表还是收起的 —— 回显全被自己藏了。
2. 点的那块如果**本来就是当前块**，摘要、框头、列表全都一模一样，
   唯一的变化是那句被藏起来的「当前手选 …」。
3. 行右侧只有 cfg 修改时间，看不出「现在读的是哪一块」。

对应改法：

- 不再隐藏、不再收起：行原地变禁用（其它行 55% 透明），被点的那行加
  `.is-picking`（描边转绿、保持不透明），读完由重画把状态落定；
- 右侧加**当前 · 时间**（`.mark.is-current-mark`，绿色），左侧标记手选时显示
  **手选**（链接色）—— 三个信号叠加，换没换过去一眼可见；
- 加 `grubAutoPicking` 闸门：探测期间重复点击直接忽略，否则一次点击会发两个请求、
  连弹两次密码框。

### 18.3 验证

- `cargo check --lib` 干净；`cargo test --lib -- --ignored live_helper_probe` 通过
  （真机 129326 字节 / 4.18s，免密）；
- 476px 窗宽截图核对：候选行 `手选 disk0s9 · arch-gui · arch` + 绿色
  `当前 · 2026/10/29 11:41` + 绿色描边；
- `node --check ui/app.js` 通过。

## 19. 「下拉框选择还是没用、点击没反应」根因（用户报障 2026-10-29）

用户在 App 里点候选分区毫无反应。HTTP 直连同一命令是通的（~5s 返回视图），
说明后端没问题；真机 WebKit 命中测试也正常。最后从 trace 日志的轮询空档
里看出真相：**三个命令都是同步 `#[tauri::command]`，跑在主线程上**。

探测一次要 4~5 秒（读 10 块裸设备），期间整个 WebView 的事件循环被卡死：
点击丢失、菜单无响应、轮询也停——不是「没反应」，是「整个 App 冻住了 5 秒」。
项目里本来就有 `blocking_guard`（启动时断言命令不能阻塞主线程），但这三个
命令早于该守卫，漏网了。

改法：

- `linux_grub_scan / linux_grub_pick / linux_grub_switch` 全部改成
  `async fn`，实现体挪进 `*_blocking`（`scan_blocking / pick_blocking /
  switch_blocking`），命令侧 `tauri::async_runtime::spawn_blocking` 包装；
  join 失败时返回 `LinuxGrubView::failed`（不再 panic 成前端 undefined）。
- `remote_routing`（HTTP 通道）本来就跑在工作线程上，直接调 `*_blocking`，
  不必过 async。
- 切换失败（写 grubenv 出错）时把错误视图也写回 `publish_ui` 状态——
  原来错误只在返回值里闪 3 秒，轮询一次就把它覆盖没了。
- 候选行点击回调改成显式 `() => pickGrubAuto(candidate.dev)`（原来直接传
  事件对象当 dev，HTML harness 里看不出来，因为 mock 里没触发 invoke）。

验证：`cargo test --lib channels::linux_grub` 16 passed、blocking_guard
测试通过、HTTP 通道 `linux_grub_pick {"dev":""}` 实测恢复自动定位成功、
`node --check` 通过。构建并覆盖安装到 /Applications（sha256 校验一致）。

教训：**「点击没反应」先查命令是不是 sync fn 在主线程上跑**，别急着改
前端；`blocking_guard` 只覆盖它声明之后写的命令，老命令要逐个补。




## 20. Linux 端接入：从「被短路」到全链贯通（2026-10-29/30）

这条线此前是文档里没有的一章：**Linux 上自动读取从来就没跑过遍历逻辑**。
`probe()` 在 Linux 分支被两行短路：

```rust
let _ = (esp_partuuid, prefer_dev, elevation);   // 用户设的 ESP UUID 整个被丢弃
Ok(probe_host())                                  // 只读本机 /boot/grub{,2}/grub.cfg
```

本机是 rEFInd 启动（根分区里没有这两个文件），于是必然报「本机没找到」——
截图报障（2026-10-29 23:59）的根因不是「检测不到」，是「Linux 分支压根没实现」。

### 20.1 用户定案（五步规则，2026-10-30 追加写入三点）

**扫描五步**：

1. 读用户设定的 ESP UUID / 分区路径（`settings.grub_efi_partuuid`）；
2. 遍历磁盘上所有 Linux 分区，看谁的 fstab 挂载了这块 ESP；
3. 挂载了就读**该分区下的** `/boot/grub/grub.cfg`（兼容 `grub2` 路径）；
4. 解析引导条目的逻辑复用（`parse_grub_cfg` 原样）；
5. 用户选择之后，写入**该分区下的** `/boot/grub/grubenv`（兼容 grub/grub2）。

**写入三点**（对第一版理解的纠正）：

- 「写 `/boot/grub/grubenv`」指的是**目标分区里**的那份，不是本机的；
- 要讲清操作文件的风险，**用成熟做法**；
- **必须提权**——包括本机行（`/boot/grub` 归 root，普通身份直写必 EACCES）。

### 20.2 技术选型：为什么全是复用

| 部件 | 选定 | 理由 |
|---|---|---|
| ext4 解析 | 自研 `ext4_raw`（§13.1 已定） | 能力边界 = 需求边界；Linux 上同一份代码直接跑 |
| 分区枚举 | `lsblk -J` + sysfs 兜底 | `unmounted_esp_partitions()` 已有同款双来源；本机 udev 缺失只有 sysfs 管用 |
| ESP 身份（FAT 卷序列） | `esp::fat_boot_sector` | fstab 里 `UUID=E537-7F31` 是 FAT 序列不是 GPT GUID（§13.7 教训照抄） |
| 提权探测 | `--linux-grub-probe` 子命令 + `linux_priv::run_via_helper` | 免密助手三通道（助手→pkexec→密码），`status=="ready"` 才静默 |
| 提权写入 | `--linux-grub-write` 子命令（**早已存在**，macOS/Windows 共用） | root 硬校验 + 参数白名单现成，一行没改 |
| 本机行 | `fs::read` 直读 + 提权写 | 定案 1「Linux 本机不经过 ext4 驱动」照旧 |

**零新依赖、零新子命令**：扫描复用 `scan_ext4`，写入复用 `write_child_main`。

### 20.3 写入的三档成熟度（用户问「风险」后的定案）

`write_env_block` Linux 实现按风险从低到高自动选，**能走内核就绝不裸写**：

| 档 | 条件 | 做法 | 风险控制 |
|---|---|---|---|
| ① | `dev == "本机"` | root 原地覆盖绝对路径文件 | 内核 ext4 驱动 |
| ② | 分区已挂载（`/proc/self/mounts` 认 ext 家族 + rw） | 写挂载点下的文件 | **坚持不裸写已挂载分区**：裸写绕过页缓存，脏页回写会把写入盖掉，ext4 日志还可能把绕过日志的数据当崩溃恢复回滚 |
| ③ | 未挂载 | 裸设备 O_RDWR → `overwrite_in_place` | 官方 `grub-editenv` 同语义：inode→extent 定位它自己的数据块，整块读改写、回读逐字节比对；RECOVER 拒写；不动元数据 |

四道硬闸三档通用：文件必须已存在**不创建**、尺寸必须恰好 1024 字节、
路径 `..`/绝对路径混用直接拒、btrfs/zfs 挂载点不认（布局不同，宁可裸写不猜）。

### 20.4 落地里的两个真 bug

1. **`runtime_from` 的 `host: view.platform == "linux"`** —— Linux 上恒 true，
   helper 扫出的真实分区也被当本机文件写：而分区行的 `env_rel` 是**分区内相对路径**
   （`boot/grub/grubenv`），在主机上按相对路径打开必然失败。改为 `part.dev == "本机"`。
   这条被 `runtime_host_only_for_host_row` 单测锁住。
2. **probe 的 notes 文案指向「关于」页** —— 免密提权卡 2026-10-29 已迁到 index.html
   系统引导页（hub.html 关于页是远程管理窗），4 处文案同步改成「系统引导页」。

### 20.5 本轮提交

| 提交 | 内容 |
|---|---|
| `0d7d59c` | Linux 扫描链：`probe_raw` 从别名短路改为真遍历；`probe()` 免密优先、没装回落本机直读并说明差距 |
| `f30d2bf` | Linux 写入链：三档成熟度、host 判定修正、`ext4_raw` 补 `open_raw_rw`、本机行也走提权 |

### 20.7 回落链修正（2026-10-30，用户定案）

`0d7d59c` 时代 probe() 的 Linux 回落是「本机直读」，真机暴露两点问题：

1. **本机直读必然失败**：`/boot/grub/grub.cfg` 是 `0600 root`（`grub-mkconfig`
   自己 `umask 077`），普通用户读不动 → `if let Ok(read)` 把 EACCES 静默当
   「没这个文件」，报出误导性的「本机没找到 …grub.cfg」；
2. **回落目标本来就该是授权而不是认输**：写入链 `write_env` 的形态是
   免密失败 → `run_privileged`（免密助手 → pkexec → sudo 三条链），扫描没理由更弱。

修正后的 Linux `probe()`：

| 条件 | 行为 |
|---|---|
| 免密组件 `state=="ready"` | `run_via_helper` 静默全量扫（原样） |
| 不就绪 / 没跑通 + `Elevation::Allow`（用户手点） | `probe_via_elevated`（三端共用）：helper 失败自动落 pkexec / sudo，notes 带真实原因 |
| 不就绪 / 没跑通 + `Elevation::Silent`（自动刷新 / 远程） | **报错不弹框**（与 macOS 同一条铁律），错误文案引导装免密组件或手点「重新读取」 |

远程请求天然被拦：`spawn_privileged` 的 `allow_gui=false`（`remoteAllowGuiAuth`）
会拒绝弹框，手机端只能靠免密组件静默完成 —— 与其它提权操作同一策略。

`probe_host()` 不再作为 probe 的回落（Linux `probe_raw` 的本机行仍用它），
「只读了本机」这条 notes 文案随之删除。

### 20.6 TODO 增补（Linux 侧）

- [ ] **真机验证·扫描**：装免密组件 → 自动 tab 应列出本机其它 ext4 的 Linux
      （按各自 fstab 挂载所选 ESP 的那块被 `locate()` 精确选中）；没装组件时
      手点「重新读取」应弹一次 pkexec 授权并扫出全量（§20.7），自动刷新则报错引导；
- [ ] **真机验证·写入**：选条目切换 → 目标分区挂载着应报「已写入 …（挂载点 …，
      回读一致）」；未挂载走裸写；`grub.cfg` 不变、只有 saved_entry/next_entry 生效；
- [ ] **真机验证·重启生效**：改 `next_entry` → 重启 → 确认进的是那一项
      （§10 阶段 1 遗留项，Linux 侧现在才有条件做）；
- [ ] btrfs 分区遇到时明确报「不支持」——本机还没有 btrfs 的 `/boot`，识别分支
      已有（非 ext4 静默跳过 + notes），文案是否要更显眼待真机看效果；
- [ ] 快照字段同步到网页端 / 手机端（§10 阶段 3 遗留，未变）。
