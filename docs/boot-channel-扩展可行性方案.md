# AnyBoot · 新增 OpenCore / Clover / systemd-boot 三条切换通道 · 可行性方案

> 需求：在现有「rEFInd + GRUB」双通道体系上，新增三条通道，把 **OpenCore、Clover、systemd-boot**
> 也纳入 AnyBoot 的「下次启动切换 + 记住上次选择」能力，体验与现有两条通道一致（UI 列表、一键切换、远端可操作）。

---

## 1. 现有架构回顾（新通道必须对齐的约束）

AnyBoot 的核心哲学是 **「状态外置到 ESP（FAT32），跨系统零依赖读写」**：

| 通道 | 状态载体 | 切换方式 | 记住上次 | 提权要求 |
|---|---|---|---|---|
| rEFInd | `ESP:/EFI/refind/vars/PreviousBoot*`（`use_nvram false` 的文件式变量） | 模板文件 `PreviousBoot-{linux,windows,mac}` 原子复制 → `PreviousBoot`（`switch_flow.rs` / `switch-system-by-refind.py`） | rEFInd 自己读写该文件 | ❌ 免提权 |
| GRUB | `ESP:/<SUBDIR>/grubenv` + `anyboot-grub.json` 清单 | 写 `next_entry`（一次性）/ `saved_entry`；Linux 端由 `/etc/grub.d/09_anyboot` 保证 grubenv 落在 ESP，systemd path 单元自动同步 grub.cfg | GRUB 的 `savedefault` 写回 ESP | ❌ 免提权 |

代码落点：`switch_flow.rs`（切换核心）、`state.rs`（状态读取）、`scan.rs`（ESP 发现）、
`lib.rs`（命令注册 + PreviousBoot 模板）、`ui/index.html` + `ui/grub-list-view.js`（两块 UI）、
`remote_routing.rs`（`switch_system` 远端命令 + `/api/grub`）。

**关键差异预告**：rEFInd/GRUB 能做到免提权，是因为它们自己会把状态写成文件；而 **systemd-boot 的状态在 NVRAM、OpenCore 的默认项在 NVRAM（BootOrder/BootNext）**。所以三条新通道会分成两类：
- **systemd-boot**：接近全文件式（loader.conf 在 ESP），最能对齐现有哲学；
- **OpenCore / Clover**：完整能力必然要写 NVRAM（各 OS 提权），文件（config.plist）只能覆盖「持久默认项」部分。

---

## 2. 三条通道逐一设计

### 2.1 systemd-boot 通道（对齐度最高）

**发现**（`scan.rs` 扩展）：
- 探测 `ESP:/loader/loader.conf` 与 `ESP:/loader/entries/*.conf` → 判定「装了 systemd-boot」；
- 解析 `loader.conf` 的 `default` / `timeout`，`entries/*.conf` 的每条 `title` + 文件名即 entry-id。

**切换下次启动**（两级策略，对齐 GRUB 的 `next_entry`/`saved_entry` 语义）：
| 策略 | 载体 | 权限 | 语义 |
|---|---|---|---|
| 一次性（首选） | NVRAM 变量 `LoaderEntryOneShot`（GUID `4a67b082-0a4c-41cf-b6c7-440b29bb8c4f`） | 提权（Linux: efivarfs/`bootctl set-oneshot`；Windows: `SetFirmwareEnvironmentVariable` API；macOS: `nvram`） | 等价 `grub-reboot`，用完即清 |
| 持久（免提权回退） | 直接改 `ESP:/loader/loader.conf` 的 `default <entry-id>` | ❌ 免提权 | 持久覆盖；AnyBoot 记录原值到 `ESP:/EFI/anyboot/loader.conf.bak` 以便「恢复原默认」 |

**记住上次选择**：systemd-boot 本来就自动维护 NVRAM 变量 `LoaderEntryLastBooted`，但只存在 NVRAM 里，跨系统读不到。做法与 GRUB 通道的同步器同构：
- Linux 端新增 `anyboot-sdboot-export`（systemd oneshot 单元，开机后跑一次）：root 读 efivarfs，把 `LoaderEntryLastBooted`、`LoaderEntryDefault`、当前 `loader.conf` 快照导出为 `ESP:/EFI/anyboot/sdboot-state.json`；
- Windows/macOS 端（AnyBoot 本体）读该 JSON 展示「上次启动项 / 当前默认项」。
- 若导出文件不存在（Linux 端没装过同步器），UI 显示「上次未知」而不猜。

### 2.2 OpenCore 通道

**发现**：`ESP:/EFI/OC/OpenCore.efi` 存在 + `EFI/OC/config.plist` 可解析 → 判定装了 OC；
顺手做**健康检查**（复用方案里查证的官方机制）：
- `Misc→Boot→ShowPicker`（false 则 OC 不会弹菜单，AnyBoot 切到它就没意义）→ 提示；
- `Misc→Security→AllowSetDefault`（true 才支持 Ctrl+Enter 记默认）→ 提示；
- `Vault=Secure` 时外部修改 config.plist 会导致 OC 自检失败 → 强提示；
- `LauncherOption=Full` 会抢占 NVRAM BootOrder → 提示改 `System`/`Disabled`。

**切换下次启动 = 进 OC 菜单**：
- 首选：写 UEFI `BootNext` 指向 OC 在 BootOrder 里的 `Boot####` 项（OC picker 遵循 BootNext，官方 DefaultEntryChoice 逻辑：BootNext > BootOrder > Apple 变量）；找不到 Boot#### 项时用 efibootmgr 等价逻辑创建；
- 免提权降级：OC 没有文件式状态可写，**降级只能提示用户提权**，没有文件路线。

**记住上次（OC 侧的两个层面）**：
- OC 自己记默认 = `AllowSetDefault` + 用户在 OC 菜单里 Ctrl+Enter，状态写 NVRAM BootOrder —— 这是 OC 用户已掌握的机制，AnyBoot 不重复造；
- AnyBoot 层面「记住上次选了谁」：与 sd-boot 同构，Linux/macOS 端把 `BootCurrent`/`BootNext` 快照导出 `ESP:/EFI/anyboot/oc-state.json`（macOS 用 `nvram -p`，需要 root，做成 AnyBoot 内的特权辅助操作而非常驻服务）。

### 2.3 Clover 通道

**发现**：`ESP:/EFI/CLOVER/CLOVERX64.efi` + `EFI/CLOVER/config.plist`。

**切换与记忆**（Clover 比 OC 多一条文件路线，对齐度介于 sd-boot 与 OC 之间）：
| 动作 | 载体 | 权限 | 说明 |
|---|---|---|---|
| 持久默认项 | 改 `config.plist` 的 `GUI→Boot→DefaultVolume`（+ `DefaultLoader`） | ❌ 免提权 | Clover 官方支持的默认引导卷配置，文件改完重启即生效 |
| 一次性进 Clover | NVRAM `BootNext` 指向 Clover 的 Boot#### | 提权 | 对 BootNext 的支持随 Clover 版本有差异，实施时以真机实测为准（OVMF 先验证） |
| 状态互通 | ESP 根的 `nvram.plist`（Clover 模拟 NVRAM 载体） | ❌ 免提权 | 若用户启用了 EmuVariableUefi，Clover 相关变量落在此文件，AnyBoot 可读它判断状态；注意与真 NVRAM 并存时的优先级 |

### 2.4 三通道统一抽象（Rust 侧重构点）

把现有 switch_flow/state 里「rEFInd 专用」的逻辑抽成 trait，三条新通道各自实现：

```rust
trait BootChannel {
    fn detect(&self, esp: &Path) -> Option<ChannelInfo>;      // 是否安装 + 版本/健康检查
    fn entries(&self, esp: &Path) -> Vec<BootEntry>;          // 列表（UI/远端共用）
    fn set_next(&self, esp: &Path, id: &str) -> SetNextPlan;  // 一次性或持久，附权限要求
    fn last_boot(&self, esp: &Path) -> Option<String>;        // 记住上次（读导出文件/变量）
}
```

`SetNextPlan` 显式携带 `RequiresElevation`——UI 据此决定「直接执行」还是弹「需要管理员/root，点击提权重试」。

---

## 3. UI 与远端 API 扩展

- **主窗**（`index.html`）：在现有 rEFInd 状态区、GRUB 列表区并列新增「systemd-boot / OpenCore / Clover」三个卡片区块，各含：检测状态、条目列表（复用 `grub-list-view.js` 的渲染，抽象成通用 boot-list-view）、上次启动标记、切换按钮；
- **托盘**（`menu_model.rs` / `tray-menu.js`）：检测到哪条通道，托盘菜单就动态加哪组「下次启动到 XXX」；
- **远端**（`remote_routing.rs`）：`switch_system` 的 system 参数扩展枚举值（`sdboot:<entry-id>` / `opencore` / `clover`），新增 `GET /api/boot-channels` 返回全通道检测+列表 JSON（对齐现有 `/api/grub` 模式）；
- **Web/手机端**：与 PC 同一份 JSON 驱动，无需单独开发。

---

## 4. 风险与限制（诚实清单）

| 风险 | 影响 | 对策 |
|---|---|---|
| OC/Clover 通道必须提权 | 与现有两条通道「免提权」体验不一致 | UI 明确分级标识：文件式操作直接做，NVRAM 操作弹提权引导；sd-boot 的持久路线保住免提权底线 |
| `BootNext` 在 Clover 旧版上行为不一 | 一次性切换可能失效 | 检测 Clover 版本，不支持时降级为「改 DefaultVolume（持久）」并告知 |
| OC `Vault=Secure` / `LauncherOption=Full` | 修改文件/引导序失败甚至 OC 拒启 | 健康检查前置拦截，只提示不擅改 |
| 黑苹果 NVRAM 污染（Clover↔OC 共存） | 清 NVRAM 后引导项丢失 | 只增不改 BootOrder 既有项；所有 NVRAM 写操作先备份原值到 `ESP:/EFI/anyboot/nvram-backup/` |
| loader.conf 手改被 `bootctl update` / 重新安装覆盖 | 免提权切换丢失 | 与 GRUB 通道同思路：文档化 + 状态自愈（检测 default 被重置则重放） |
| sd-boot 导出依赖 Linux root 单元 | Win/mac 端看不到「上次」 | 降级显示「上次未知」，引导用户在 Linux 端装同步器 |

## 5. 里程碑

| 阶段 | 内容 | 验收 |
|---|---|---|
| M1 | `BootChannel` trait 抽象 + sd-boot 通道（发现/列表/免提权切换/last-boot 导出单元） | OVMF + 虚拟 ESP 里全流程跑通 |
| M2 | OC 通道（发现/健康检查/BootNext 切换/状态导出） | OVMF 里 OC picker 正确响应 BootNext |
| M3 | Clover 通道（发现/config.plist 修改/BootNext 实测） | 同上 |
| M4 | UI 三卡片 + 托盘 + 远端 API + 提权分级交互 | 三端（PC/Web/手机）可切 |
| M5 | 真机矩阵：黑苹果机（OC+Clover 共存）、多 Linux 发行版（sd-boot）、NVRAM 脆弱机型 | 每条通道至少 2 台真机验证 |

## 6. 结论

**可行，且与现有架构同构**：
- systemd-boot 通道几乎完全复刻 rEFInd/GRUB 的「ESP 文件交换」模式，免提权体验不打折；
- OC/Clover 通道在「文件可改的部分」（Clover DefaultVolume、健康检查、状态导出）保持免提权，只有「NVRAM 必须写的部分」（BootNext）需要提权——这是被引导器自身机制决定的，无法绕过，但 UI 分级后体验可控；
- 工程主体是 `switch_flow.rs`/`state.rs` 的 trait 化重构 + 三个通道实现 + UI/远端扩展，预估 M1-M4 约 1.5-2 周（不含真机矩阵周期）；
- 所有新状态文件统一收口在 `ESP:/EFI/anyboot/` 下，与现有 grubenv 清单、rEFInd vars 互不干扰，卸载路径清晰。
