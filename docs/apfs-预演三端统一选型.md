# APFS 只读解析：让 OC 预演的 macOS / Preboot 条目三端统一（技术选型）

> 状态：**已实现并在真机验证通过**（2026-10-28）
> 关联：[`boot-menu-discovery.md`](boot-menu-discovery.md)（预演原理）、[`boot-menu-discovery-internals.md`](boot-menu-discovery-internals.md)（源码追踪）、`src-tauri/src/channels/apfs_reader.rs`（实现）

## 1. 背景：预演结果两端不一样

OpenCore 菜单预演（`oc_menu.rs::preview`）在 macOS 上能完整复算出 OC 菜单，但在 Windows / Linux 上**缺所有 macOS 条目**。根因不在扫描逻辑，在数据源：

- macOS 的引导文件 `boot.efi` 不在系统卷上，而在 APFS 容器的 **Preboot 卷**里，按系统卷 UUID 分目录（`Preboot/<UUID>/System/Library/CoreServices/boot.efi`）。OC 靠自带的 `apfs.efi` 驱动读它（`OcAppleBootPolicyLib.c:987-1053`）。
- **Windows 没有 APFS 驱动，Linux 只有 staging 质量的内核驱动**——`/System/Volumes/Preboot` 这一层在两个平台上物理不可见，`oc_menu.rs` 的 Preboot 分支因此是 `#[cfg(target_os = "macos")]` 独占。
- NVRAM（BootOrder / OCBt*）是另一处差异，但那是**提权问题**（Windows 管理员、Linux root 后即与 macOS 等价），不在本文范围。

目标：让 Windows / Linux 也能算出 Preboot 条目，三端预演收敛到同一份结果。

## 2. 需求边界（刻意收得很小）

只要三样东西，**全部只读**：

1. Preboot 卷根目录下 UUID 形态的目录列表；
2. 每个目录里 `System/Library/CoreServices/boot.efi` 的**存在性**（不用读内容）；
3. 目录里 `.contentDetails` 这个小文本（菜单显示名，如 "MAC"）。

明确**不做**：写盘、FileVault 加密卷解密（Preboot 常规不加密，系统卷内容我们不读）、全量文件遍历。

## 3. 候选方案对比与淘汰理由

| 方案 | 结论 | 理由 |
|---|---|---|
| A. 自写 APFS 解析器 | 淘汰 | 容器超块/检查点/对象映射/B-tree 全套，工程量大；开源 crate 已现成，重复造轮子无意义 |
| B. 捆绑商业驱动（Paragon APFS for Windows 等） | **不可行** | 商业授权，不能随 Apache-2.0 项目分发 |
| C. 解析 OC 自己的启动日志（ESP 上 `opencore-*.txt`） | 备选，不作为主路 | 两端读同一文件天然一致、且是 OC 亲算的 100% 结果；但要求用户开 `Misc→Debug` 文件日志、且读到的是**上一次启动**的历史数据，不是实时复算。保留为「想要 100% 精度」的增强项 |
| D. 系统级原生方案 | 淘汰 | linux-apfs-rw 在 mainline staging（`drivers/staging/apfs`），发行版内核未必编译、质量不受控；apfs-fuse 是 **GPL-2.0**、libfsapfs 是 **LGPL-3.0**，与主仓库 Apache-2.0 的二进制分发有授权传染问题 |
| **E. 纯 Rust 只读 crate（选定）** | ✅ | 零系统依赖、零挂载、三端同一份代码、许可证干净 |

## 4. Rust crate 选型（2026-10 调研）

| | `apfs` | `apfs-core` | `hadris-apfs` |
|---|---|---|---|
| 仓库 | Dil4rd/dpp | SecurityRonin/apfs-forensic | hxyulin/hadris |
| 许可证 | MIT | **Apache-2.0** | MIT |
| 规模 | ~3K SLoC | 取证向，测试覆盖最全 | ~17K SLoC |
| 活跃度 | 高（0.4.x，2026-10，月下载 2.6K） | 持续更新 | experimental，API 不承诺稳定 |
| 容器/对象映射/B-tree | ✓ | ✓ | ✓ |
| decmpfs 压缩 | ✓ 透明解压 | ✓（lzfse/lzvn） | ✗ 报 Unsupported |
| 加密卷 | ✗ | ✗ | ✓（单密钥，3.x） |
| 依赖 | 仅 byteorder + thiserror | flate2、lzfse_rust 等 | 自带 hadris 生态 |

**决策（spike 后定稿）：`hadris-apfs 3.0.0-rc.2`（MIT），版本锁死。** spike 结论推翻了初版「首选 apfs-core」：

- `apfs-core` 是**底层积木**：只给 `open` + 卷超块地址，目录导航要自己拼 btree/dir/inode/extent 四个模块，等于重写半个高层 API —— 放弃；
- `apfs`（Dil4rd）有现成高层 API，但 `ApfsVolume::open` **只会挂载容器里第一个卷**，而 Preboot 通常排第三 —— 致命，放弃；
- `hadris-apfs` 有 `VolumeSelector::Name("Preboot")` 按卷名选卷，正中需求。缺点是 API 不稳定 → 对策：`Cargo.toml` 锁死 `=3.0.0-rc.2`，升级必须人工过一遍调用点。

> 「不支持加密卷」对需求边界无影响：我们只碰 Preboot 卷（常规不加密）的存在性判断与一个小文本文件。FileVault 用户数据卷本来就不在读取范围内。

## 5. 架构要点：全程零挂载

分层认知是整个方案的根基——**Preboot 不是一个分区，是 APFS 容器内部的一个卷**。GPT 表里只有一个 APFS 容器分区（类型 `7C3457EF-0000-11AA-AA11-00306543ECAC`），System/Data/Preboot/VM 全在其内。

```text
GPT：    1 个 APFS 容器分区 + 1 个 ESP
容器内： System / Data / Preboot / VM   ← crate 在内存里解析出来
Preboot：/<UUID>/System/Library/CoreServices/boot.efi  ← 目录树节点
```

因此**没有任何挂载/卸载/盘符操作**，不与 automount 打架，不留系统状态。各平台只是「给 crate 喂一个能 Seek 的字节流」：

| 平台 | 数据源 | 权限 |
|---|---|---|
| Windows | `\\.\PhysicalDriveN` 整盘 → `hadris_storage::Partition` 按偏移夹取容器分区。**注意不能走 `\\?\Volume{…}` 卷路径**：Windows 不给未知文件系统的分区建卷路径（实测 `AccessPaths=null`），坐标只能从 `Get-Partition` 的 `Offset/Size` 拿 | **必须管理员**：读原始整盘在 Windows 恒为 ERROR_ACCESS_DENIED（os error 5），只读也不例外；失败路径补 issue 行说明原因 |
| Linux | 直接打开容器分区设备（`/dev/nvme0n1p4` 等），无需偏移换算 | root（或 `disk` 组） |
| macOS | 不走此路（原生读 `/System/Volumes/Preboot`，更快更稳） | — |

三端共同点：与 NVRAM 读取是**同一次提权**，UI 上合并成一次授权。

## 6. 实测结论（本机黑苹果双系统真机）

- `scan_preboot()` 真机一次跑通：枚举出 `2D8A2ED9-…` 一个 UUID 目录，`boot.efi` 存在，`.disk_label.contentDetails` 读出 **"MAC"**——与 OC 菜单显示名一致；
- **Preboot 卷上没有 `.contentDetails`，只有 `.disk_label.contentDetails`**（文件，不是 ESP 上那种 `.disk_label/` 目录）——取名回退链必须包含它；
- macOS 侧同一台机器的预演列出的是同一条（`Preboot/2D8A2ED9-…`），两端一致达成。

## 7. 风险与兜底

1. **crate 年轻**（2026 年才成型）：只读不写，最坏情况是解析出错——不会损坏盘。所有解析失败必须**回退现状**（不列 macOS 条目），并在预演 `summary` 里注明「APFS 解析不可用（原因）」，禁止静默吞掉。
2. **读活盘**：另一边 macOS 正在写时拿到的是无一致性保证的瞬时视图。对我们无害——`boot.efi` 存在性与 `.contentDetails` 极少变动，且 crate 逐块 Fletcher-64 校验，坏块报错而非给错数据。
3. **性能**：Preboot 卷很小（只有引导文件），只按需 resolve 两三个路径，不做 walk。
4. **测试 fixture**：取用户黑苹果盘容器分区的头部若干 MiB 做样本（只含 Preboot 结构，不含用户数据），钉住回归。

## 8. 实施记录（2026-10-28 完成）

1. ✅ spike：真机验证「枚举 Preboot 的 UUID 目录 + `boot.efi` 存在性」，Windows 一次跑通；
2. ✅ 新增 `src-tauri/src/channels/apfs_reader.rs`：GPT 类型定位容器分区（Windows 整盘+偏移，Linux 直接开分区）→ `hadris-apfs` 按名挂 Preboot → 返回 `Vec<PrebootItem>`；
3. ✅ `oc_menu.rs` 的 ② Preboot 分支改造：macOS 原生路径不变，Win/Linux 走 `apfs_reader`，输出同一 `MenuEntry`（`source: "preboot"`）；summary 追加「Preboot 直读 N 条」；
4. ✅ 失败兜底：任何失败补一行 `severity=info` 的 issue（人话原因，权限失败单独点名），不静默吞；
5. ✅ `THIRD_PARTY_NOTICES` 登记 hadris 系列许可证（MIT）；
6. ✅ 单元测试（分区 JSON 解析 / lsblk 解析 / UUID 形态门）+ 真机集成测试 `real_machine_scan`（`#[ignore]`，`cargo test real_machine_scan -- --ignored --nocapture` 手动跑）。

遗留事项：
- Linux 端**读链路**已真机验证（2026-10-29，`real_linux_oc_read` 探针）：efivarfs 直读 `BootOrder` / `Boot0080..0082` 与 OC 私有 `OCBtOrder` / `OCBt0080..0082` 全部解析正确（描述 + 分区内路径）。
  仍未验证的两点：① `lsblk -rno PATH,PARTTYPE` 的 PARTTYPE 列覆盖度（沙箱里读不到 `/dev`，值恒空；需要真机桌面会话跑一次）；② `pkexec` 写 efivarfs 那条路（`install -p` 是否会因 efivarfs 不支持 chmod/truncate 被拒）。
- **Linux 只枚举已挂载的卷**（`candidate_roots()` = `/efi`、`/boot/efi`、`/boot`、`/esp`）。OC 装在另一块没挂载的 ESP 上时，通道卡片直接报「未检测到」—— 实测本机就是这样（`/boot/efi` 是 GRUB 那块，OC 那块没挂）。Windows 用 `Get-Partition` 全盘枚举再临时挂载，macOS 靠 `/Volumes` 自动挂载，Linux 缺等价的一步，这是三端里最后一块实质差距；
- 提权合并 UI（非提权失败时引导用户提权重扫）暂未做 —— 现状是补 issue 行说明原因；
- hadris-apfs 升级策略：锁死 `=3.0.0-rc.2`，升级须人工过调用点 + 真机重验。
