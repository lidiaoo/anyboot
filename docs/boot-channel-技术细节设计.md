# AnyBoot · 三引导器通道技术细节设计

> 上游文档：`boot-channel-扩展可行性方案.md`（机制论证）、`boot-channel-ui-mockup.html` + `docs/ui-preview/`（UI 定稿）。
> 本文是落地级设计：模块结构、数据结构、文件格式、字节级细节、命令面、测试。实现以此为准。

---

## 1. 代码结构（对齐现有模块划分惯例）

```
src-tauri/src/
├── channels/                 # 新模块目录（与 grub_esp / grub_install 平级的新链路）
│   ├── mod.rs                # BootChannel trait + 注册表 + ChannelsRuntime 聚合
│   ├── sdboot.rs             # systemd-boot 通道
│   ├── opencore.rs           # OpenCore 通道
│   ├── clover.rs             # Clover 通道
│   ├── nvram.rs              # 三平台 EFI 变量读写（BootNext/BootOrder/LoaderEntry*）
│   └── plist_lite.rs         # Apple XML plist 只读/定点改写（OC/Clover config.plist）
└── esp.rs / grub_esp.rs      # 复用：EspInfo / ActiveMount / pkexec 写盘模式
```

- `state.rs`（冻结文件，按其注释约定）：`AppState` 追加一个字段
  `pub(crate) channels: ChannelsRuntime`，与 `grub: GrubRuntime` 完全并列。
- `lib.rs`：`mod channels;` + 快照结构加字段 + 新命令注册。

## 2. 数据结构与快照流

```rust
// channels/mod.rs
pub(crate) const CHANNEL_IDS: [&str; 3] = ["sdboot", "opencore", "clover"];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelEntry {
    pub(crate) id: String,          // sdboot: entry 文件名去 .conf；oc/clover: 描述串
    pub(crate) title: String,       // 展示名（title= / 卷名 / "进入 xxx 菜单"）
    pub(crate) detail: String,      // 小字：loader/entries/arch.conf 等
    pub(crate) icon: &'static str,  // 前端选图标用：linux/windows/macos/menu
    pub(crate) requires_elevation: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelView {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,          // "systemd-boot" / "OpenCore" / "Clover"
    pub(crate) detected: bool,
    /// "file"=免提权可切换；"nvram"=切换必经 NVRAM 写
    pub(crate) permission: &'static str,
    /// ok / warn / off —— 对应 UI 状态 chip
    pub(crate) chip: &'static str,
    pub(crate) detail: Option<String>,       // loader.conf 摘要 / OC 版本探测
    pub(crate) health: Vec<HealthIssue>,     // OC/Clover 健康检查
    pub(crate) entries: Vec<ChannelEntry>,
    pub(crate) current_default: Option<String>,
    pub(crate) last_boot: Option<String>,    // 记忆回读
    pub(crate) last_boot_at: Option<String>,
    pub(crate) error: Option<String>,
}

pub(crate) struct HealthIssue { pub(crate) severity: String, pub(crate) text: String }

/// state.rs 新增字段（与 GrubRuntime 并列）
pub(crate) struct ChannelsRuntime {
    pub(crate) esp: Option<EspInfo>,          // 复用 grub_esp 已解析的 ESP
    pub(crate) views: Vec<ChannelView>,
}
```

- **快照**：`UiState` 加 `pub(crate) channels: Vec<ChannelView>`，与 `grub: GrubView` 并列。
  前端 revision 比对机制照旧生效，无需新事件。
- **刷新时机**：`switch_flow::refresh_and_update` 里 `refresh_grub(app)` 之后追加
  `channels::refresh(app)`——ESP 已由 GRUB 链路解析好（或独立探测一次），三通道只做文件/变量读取，毫秒级。

## 3. ESP 定位（复用，不重造）

直接复用 GRUB 链路的 ESP 解析结果：`grub_esp.rs` 已产出 `EspInfo`（三平台探测、FAT 校验、
PARTUUID 权威键、按需挂载 ActiveMount）。`channels::refresh` 的 ESP 来源优先级：

1. GRUB 已解析的 `efi_dir`（用户已配置并验证过）；
2. 常见挂载点探测（/efi /boot/efi /boot、macOS /Volumes/*、Windows 盘符扫描 `windows_partition_guids()`）；
3. 都没有 → 三通道全部 `chip=off`，`detail` 写明原因（与 GRUB 空态同文案风格）。

**不引入第二个 ESP 探测实现**；Windows 无盘符摸不到 ESP 的限制与 GRUB 通道一致，UI 照实说。

## 4. systemd-boot 通道（sdboot.rs）

### 4.1 检测与列表（纯文件，免提权）
- 检测：`ESP:/loader/loader.conf` 存在，或 `ESP:/loader/entries/` 下有 `.conf`。
- 列表：逐个解析 `entries/*.conf`：`title`（缺失用文件名）、id=文件名主干。
  跳过 `*-utils.conf`（工具条目）；按文件名排序（systemd-boot 同序）。
- 当前默认：`loader.conf` 的 `default` 键。三种值的处理：
  `@saved` → 显示「@saved（记忆上次）」；精确 id / glob → 直接展示。

### 4.2 切换实现
```
策略A（默认，免提权）：改写 loader.conf 的 default 行
策略B（one-shot，需提权）：写 EFI 变量 LoaderEntryOneShot
```

**策略A 算法**（保证幂等 + 可恢复）：
1. 读原文，按行扫描，定位唯一 `default <value>` 行（忽略注释行 `#`）；
2. 替换值前后**先校验**：目标 id 必须在 entries 列表里，否则拒绝（防写坏菜单）；
3. 无 default 行 → 在首个非注释行前插入 `default <id>`；
4. 原值先备份到 `ESP:/EFI/anyboot/sdboot/loader-default.bak`（内容：原值 + ISO 时间）；
5. 原子写：同目录 `.{name}.tmp` + `fsync` + `rename`（与 `atomic_copy` 同模式）；
6. 写前记录「用户原默认」进 `EFI/anyboot/sdboot/state.json`，UI 提供「恢复原默认」。

**策略B 字节细节**（nvram.rs 实现）：
- 变量：`LoaderEntryOneShot`，GUID `4a67b082-0a4c-41cf-b6c7-440b29bb8c4f`，
  值 = entry-id 的 ASCII 字节（无 NUL），efivarfs 文件首 4 字节 = 属性 `0x7`
  （NV|BS+RT，little-endian `07 00 00 00`）。
- Linux：root 直接写 `/sys/firmware/efi/efivars/LoaderEntryOneShot-4a67b082-.../`；
  非 root 走 `pkexec sh -c 'printf ... > ...'`（暂存文件到 /tmp，与 grub_esp.rs 的
  暂存 + pkexec 单命令模式一致）。
- Windows：`SetFirmwareEnvironmentVariableW("LoaderEntryOneShot", "{4a67...}", ...)`，
  需要进程有 `SeSystemEnvironmentPrivilege`（管理员）；未提权 → 走授权弹窗提权重启流程。
- macOS：`nvram 4a67b082-0a4c-41cf-b6c7-440b29bb8c4f:LoaderEntryOneShot=<id>`（root）。
  ⚠ 真机实测项：sd-boot 只认 GUID 前缀变量名，macOS nvram 的写入格式需在 M5 验证。

### 4.3 记住上次（导出同步器）
- systemd-boot 自身维护 NVRAM `LoaderEntryLastBooted`（同 GUID）。
- 新增 Linux 端导出（模式复刻 GRUB 的 `anyboot-grub-export` systemd 单元）：
  `anyboot-sdboot-export`（oneshot，`After=multi-user.target`）：root 读
  `/sys/firmware/efi/efivars/LoaderEntryLastBooted-4a67...`（跳过 4 字节属性头），
  连同 `LoaderEntryDefault`、`loader.conf` 的 default 快照，写成：
  ```json
  // ESP:/EFI/anyboot/sdboot/state.json
  { "format": "anyboot-sdboot/1", "readAt": "...", 
    "lastBooted": "arch.conf", "default": "arch.conf", "source": "nvram" }
  ```
- AnyBoot 三端 UI 都读这个 JSON 展示「上次启动」；文件不存在 → 显示「上次未知」。

## 5. OpenCore 通道（opencore.rs）

### 5.1 检测 / 列表 / 健康检查
- 检测：`EFI/OC/OpenCore.efi` + `EFI/OC/config.plist` 均存在。
- 版本：读 `EFI/OC/Version.plist` 的 `ProductBuildVersion`（有则展示）。
- plist 读取（plist_lite.rs，见 §7）：`Misc→Boot→ShowPicker / Timeout`、
  `Misc→Security→AllowSetDefault / Vault`、`Misc→Boot→LauncherOption`。
- 健康检查规则（severity=warn 全部只提示不擅改）：
  | 字段 | 值 | 结论 |
  |---|---|---|
  | ShowPicker | false | 「OC 不弹菜单，切换看不到选择界面，建议 true」 |
  | AllowSetDefault | false | 「无法在 OC 菜单 Ctrl+Enter 记默认项」 |
  | Vault | Secure | 「外部改动 OC 目录会自检失败，AnyBoot 不会改 OC 文件」 |
  | LauncherOption | Full | 「OC 会抢占 BootOrder，建议 System/Disabled」 |
- 条目列表：OC 的扫描结果动态生成、无法静态枚举 → 固定两条：
  「进入 OpenCore 菜单」（BootNext → OpenCore.efi）+「跳过 OC 直接启动」提示文案。
  （可行性方案已定：OC 子系统条目由 OC picker 自己管，AnyBoot 不重复。）

### 5.2 切换 = 写 BootNext（nvram.rs）
1. **枚举现有项**：读 `BootOrder`（GUID `8be4df61-93ca-11d2-aa0d-00e098032b8c`，
   值 = UINT16 数组），逐个读 `Boot####`；
2. **EFI_LOAD_OPTION 解析**（务实做法）：完整 DevicePath 解析太重，采用
   「结构头（UINT32 Attributes + UINT16 FilePathListLength）之后，在描述符 UTF-16LE
   里搜 `OC\OpenCore.efi`（或 DevicePath 区搜同样 UTF-16 串）」定位 OC 项；
   已知误判风险低（路径含 OpenCore.efi 即 OC 专属），单测覆盖字节样例；
3. **找到** → 写 `BootNext` = 该项编号（UINT16 LE）；
4. **没找到** → 创建：取最小空闲编号 N，写 `Boot%04X`（Attributes=
   `0x1 LOAD_OPTION_ACTIVE`，DevicePath = 该 ESP 分区的 HD() 节点 + 文件路径节点，
   Description="AnyBoot → OpenCore"），并**追加**进 BootOrder（绝不重排既有顺序）。
   DevicePath 构造：HD(1, GPT, PARTUUID, ...) + File(Path) —— PARTUUID 复用
   `EspInfo.partuuid`。⚠ 创建是 M2 后半段，M2 前半段「未注册 → 报错提示用户在
   BIOS 里加一项」兜底；
5. **备份**：写任何 NVRAM 变量前，把 BootOrder + 全部 Boot#### 原值序列化存
   `ESP:/EFI/anyboot/nvram-backup/<ISO时间>.json`（保留最近 10 份）；
6. **状态导出**：读 `BootCurrent` + `BootNext` + BootOrder 首项，写
  `EFI/anyboot/oc/state.json`（macOS 端由 AnyBoot 进程直接写，root 弹授权；
  Linux 端复用 §4.3 单元思路）。

## 6. Clover 通道（clover.rs）

- 检测：`EFI/CLOVER/CLOVERX64.efi` + `EFI/CLOVER/config.plist`。
- **持久默认（文件，免提权）**：改 `config.plist` 的 `GUI→Boot→DefaultVolume`
  （+ 可选 `DefaultLoader`）。plist_lite 定点改写：只动目标 key 的值文本，
  其余字节原样保留（XML plist 是纯文本，用「按 key 定位的受限改写」比全量
  反序列化再序列化安全得多——不会重排/丢注释/改格式）。改前备份到
  `EFI/anyboot/clover/config-backup/<时间>.plist`。
- **一次性（NVRAM BootNext）**：同 §5.2，搜 `CLOVER\CLOVERX64.efi`。
  UI 上标注「实验性：Clover 对 BootNext 的支持随版本有差异」。
- **状态互通**：Clover 模拟 NVRAM 的 `ESP:/nvram.plist` 存在时，读
  `Clover.PickerAttributes` 等键判断模拟 NVRAM 是否启用，展示「状态来源：模拟 NVRAM」。

## 7. plist_lite.rs（只做最小的事）

不引入 `plist` crate（会连坐引 quick-xml 等一串依赖，且全量序列化有格式风险）。
范围严格限定：
- `read_dict_path(xml, &["Misc", "Boot", "ShowPicker"]) -> Option<plist Value>`：
  手写递归下降解析 `<dict><key>…</key>` 结构，支持 string/integer/true/false 四种标量；
- `rewrite_key(xml, path, new_value) -> Result<String>`：字节级定位该 key 的值节点
  替换文本，其余内容逐字节保留；定位失败/嵌套不符 → Err，绝不猜；
- 单测：真实 OC/Clover config.plist 样例文件跑 read + roundtrip。

## 8. nvram.rs（三平台 EFI 变量读写）

```rust
pub(crate) fn read_var(name: &str, guid: &str) -> Result<Vec<u8>, NvramError>;
pub(crate) fn write_var(name: &str, guid: &str, value: &[u8]) -> Result<(), NvramError>;
```
- Linux：efivarfs 直读（root）/ pkexec 暂存写入（非 root，复用 grub_esp.rs 模式）；
- Windows：`Get/SetFirmwareEnvironmentVariableW`，进程需管理员（无则 Err(NeedElevation)）；
- macOS：`/usr/bin/nvram` 子进程（读 `nvram -xp` XML，写 `nvram guid:name=…`，root）；
- 错误统一带「需要提权吗」标记 → UI 据此渲染 ⤴ 按钮与授权弹窗
  （复用现有远程授权基建：`Settings.remote_allow_gui_auth` + cli.rs 的授权弹窗链路）。

## 9. 命令面扩展（对齐现有 RemoteCommand 模式）

- `switch_system` 的 `system` 值域扩展（`switch_and_update_inner` 前置分流）：
  - `"sdboot:<entry_id>"` → sdboot 策略A（免提权）
  - `"sdboot-oneshot:<entry_id>"` → sdboot 策略B
  - `"opencore"` / `"clover"` → BootNext
  - `"clover-default:<volume>"` → 文件模式
  - 原有 `"linux/windows/macos"` 不动（rEFInd 模板复制）
- `state.rs` 的 `RemoteCommand` 新增变体：
  `SwitchChannel { channel: String, target: String }`，`summary()` 输出
  「切换 systemd-boot 启动项为 arch.conf」等；pending/取消/倒计时机制全复用。
- 新命令：`boot_channels() -> Vec<ChannelView>`（Tauri command + HTTP `/api/boot-channels`
  GET，鉴权与 `/api/grub` 同级）。

## 10. 前端（对齐 ui-preview 定稿）

- 新文件 `ui/boot-list-view.js`：泛化 grub-list-view 的行渲染（`.boot-entry` 结构），
  grub-list-view.js 保持不动；
- `app.js`：快照的 `channels` 数组驱动三张卡片（已检测的渲染完整卡片，未检测的进
  底部折叠条——纯前端判断）；`switchSystem(id)` 复用现有 invoke 链路；
- `hub.html` 远端页与手机端：消费同一份 `/api/boot-channels` JSON，卡片渲染器复用。

## 11. 测试与验证

| 层 | 内容 |
|---|---|
| 单测 | loader.conf 解析/改写（含无 default 行、注释混排、非法 id 拒写）；plist_lite read/rewrite roundtrip（真实 config.plist fixture）；BootOrder/Boot#### 字节样例解析（UTF-16 搜索定位 OC/Clover 项）；switch_system 值域分流 |
| 集成（OVMF） | 虚拟 ESP 放入 sd-boot + OC + Clover，AnyBoot 逐通道切换 → QEMU 重启断言实际进入的加载器；BootNext 用后清除语义验证 |
| 真机（M5） | 黑苹果机（OC+Clover 共存）× 1、sd-boot 发行版 × 2、NVRAM 脆弱机型（文件回退）× 1、macOS 端 nvram 写入格式实测 |

## 12. 依赖与风险登记

- 新依赖：**0**（plist_lite 手写、nvram 三平台直调系统接口，不引 crate）；
- 风险：Windows 未提权进程读 efivar 等价接口受限 → 已按 Err(NeedElevation) 建模；
  macOS nvram 写二进制值格式需实测（M5 前sd-boot one-shot 在 macOS 端默认禁用，
  只开放策略A）；
- 兼容：`switch_system` 旧值域行为零变化；`state.rs` 仅追加字段；UI 现有规则零修改。
