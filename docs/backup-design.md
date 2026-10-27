# 备份与恢复（导入/导出）+ WebDAV 云同步 设计文档

状态：设计定稿，待实现。决策已确认：① 本文档先行，实现另开一轮；② 导入时每次让用户选合并策略（记住上次选择为默认）；③ 备份文件加密可选；④ WebDAV 多端同步与文件导入导出**同一版实现**，冲突按条目合并，同步文件加密可选，自动同步=周期+变化双模式；⑤ **全平台支持**：桌面端（Tauri：Linux/Windows/macOS 同码一份）与移动端（Android/iOS）互通导入导出与同步。

## 1. 目标与范围

- 用户可把 App 内数据导出为一个文件，用于换机迁移、多机同步、灾备。
- 导出/导入时按**分类**勾选：电脑列表、SSH 服务器（跳板机）、SSH 密钥、偏好设置、连接记录。
- **五平台互通**（Linux/Windows/macOS/Android/iOS）：同一份 `.anyboot` 文件任何端可导入。桌面端一次实现（Tauri 一份代码三 OS 生效）。
- **WebDAV 多端同步**：多台设备经由用户自己的 WebDAV（坚果云等兼容服务商均可）保持所选分类数据一致（§10）。
- 明确不做：字段级增量协议（采用「整文件拉取 + 按条目合并」）、非 WebDAV 协议（SMB/S3 暂不支持）、自动发现冲突后的人工比对界面。

## 2. 数据清单与分类

两端持久化同构：全部业务数据在同一个 prefs 文件（iOS UserDefaults / Android SharedPreferences，键名一致），敏感材料在系统安全存储（iOS Keychain / Android Keystore，经 KeyVault）。

| # | 分类名（UI 文案） | 来源键 | 内容 | 敏感度 | 默认勾选 | 需加密 |
|---|---|---|---|---|---|---|
| 1 | 电脑列表 | `servers`、`active_id` | ServerProfile 数组：名称/分组/候选地址/MAC/电源路由/局域网 token/远程配对(RemotePair，含隧道 token)/探测与连接策略/关联 jumpServerId | 中（含 token） | ✅ | 勾选时强制可加密（见 §4） |
| 2 | SSH 服务器 | `jump_servers` | JumpServer 数组：协议/主机:端口/用户/认证方式/关联 keyId/级联 jumpServerId/终端路径与 shell | 中（不含密码本体） | ✅ | 同上 |
| 3 | SSH 密钥 | `ssh_keys` + KeyVault | SshKeyEntry 元数据 + **私钥 PEM / 密钥口令 / 服务器登录密码**（导出时从 Keychain/Keystore 解出） | **高** | ❌ | **勾选即必须加密** |
| 4 | 偏好设置 | `terminal_*` 5 项、`transport`、`connect_all`、`failReloadSec`(iOS) | 终端偏好 + 全局行为偏好 | 低 | ✅ | 可不加密 |
| 5 | 连接记录 | `session_history`、`tofu_<host>`（iOS）/`trusted_hosts`（Android） | 会话历史（最近 50 条）+ TOFU 主机指纹 | 低~中 | ❌ | 可不加密 |

规则：
- 分类 1、2 内嵌了 token/密码引用，勾选任一即建议加密（黄色警示条），**不强制**（用户可能只是想同步到自己的另一台手机，接受风险）。
- 分类 3 **强制加密**：未设置口令时导出按钮禁用，提示「含私钥的备份必须设置口令加密」。
- 其余分类可不加密（明文 JSON，方便检查/手工编辑）。

### 引用完整性

- 电脑档案 `jumpServerId` → 跳板机；跳板机 `keyId` → 密钥；`passwordRef`/`keyPassphraseRef`/`privateRef` → KeyVault account 键。
- **导出时**：所选分类未覆盖被引用对象 → 顶部信息条提示「N 台电脑关联的跳板机未包含在本次导出」（照常导出）。
- **导入时**：同上，缺失引用的条目照常导入，报告里列「跳板机待配置」清单；**不做静默修复**。

### 2.1 全量数据审计（两端逐项盘点，2026-10-27 实测代码）

**能被备份的（进入分类）**

| 存储 | iOS 键 | Android 键（prefs 文件 `anyboot_remote`） | 归属分类 |
|---|---|---|---|
| 电脑档案 | `servers` | `servers` | 电脑列表 |
| 当前电脑 id | `active_id` | `active_id` | 电脑列表 |
| 自动连接全部 | `connect_all` | `connect_all` | 偏好设置 |
| 传输方式 | `transport` | `transport` | 偏好设置 |
| 失败自动重载秒数 | `failReloadSec`（iOS 独有） | —（Android 无此功能） | 偏好设置（仅 iOS 读写） |
| 跳板机 | `jump_servers` | `jump_servers` | SSH 服务器 |
| SSH 密钥 | `ssh_keys` + Keychain | `ssh_keys` + Keystore 加密（见 2.2） | SSH 密钥 |
| 终端偏好 5 项 | `terminal_dark` / `terminal_font_size` / `terminal_font_family` / `terminal_auto_keyboard` / `terminal_key_bar` | 同左（`KEY_TERMINAL_*`，键名一致） | 偏好设置 |
| 会话历史 | `session_history` | `session_history` | 连接记录 |
| TOFU 主机指纹 | `tofu_<host:port>`（每主机一个键） | `trusted_hosts`（单键聚合） | 连接记录 |

**明确不备份的（审计排除，附理由）**

| 存储 | 两端 | 排除理由 |
|---|---|---|
| `migrated_multi_server` | 两端同键 | 一次性迁移标记。**恢复了反而有害**：新设备会被误判「已迁移」而跳过 1.1→1.2 档案迁移 |
| `firstRunDone` | iOS | 首启标记。新设备应保留首启「连接 AnyBoot 服务端」引导，恢复了就没有引导 |
| `github_cache` | 两端同键 | 关于页版本检查缓存，纯 cache |
| `ABSeedTransfers` 等 `ABSeed*` | iOS | 测试注入标记（探针工程用），非用户数据 |
| `transfers.json`（Documents/filesDir） | 两端同文件名 | 传输任务清单：进行中/暂停任务的本地状态，恢复到新设备无意义（localPath 全部失效）；v1 不备，v1.1 可考虑只备「已完成历史」 |
| `Documents/Downloads/`（iOS）/ `getExternalFilesDir(Download)`（Android） | — | 用户传输的文件本体，属用户文件不属 App 配置，体积不可控 |

### 2.2 审计关键发现：两端 JSON 键并不完全一致（推翻此前假设）

JumpStore 代码注释「字段名与 JSON 键完全一致，互导不失真」**只对 iOS↔iOS 成立**。跨端实差：

| 语义 | iOS 键 | Android 键 | 差异性质 |
|---|---|---|---|
| 协议 | `protocolRaw` | `protocol` | 改名 |
| 登录密码 | `passwordRef`（Keychain account 键） | `passwordEnc`（Keystore AES-GCM 密文 base64，内嵌 JSON） | **存储形态不同** |
| 密钥口令 | `keyPassphraseRef` | `keyPassphraseEnc` | 同上 |
| 私钥 PEM | `privateRef`（SshKeyEntry 内） | `privateEnc` | 同上 |
| 终端路径/Shell | `termPath: String?` / `termShell: String?` | `termPath: String` / `termShell: String`（空串） | nil vs "" 容错即可 |
| lastSeen | `Date?`（Codable 默认纪元秒） | `long`（epoch 毫秒） | 单位不同，需归一 |
| 指纹存储 | `tofu_<host>` 动态键 | `trusted_hosts` 单键 | 结构不同，备份统一为数组 |

**推论（改变实现方案）**：
1. 备份 schema 必须定义**规范键名**（canonical keys），两端各写一层薄适配器（encode/decode 时映射），不能直接把本端 JSON 塞进备份。规范键名的全平台选择见 §2.3（camelCase，桌面 snake→camel 映射）。
2. **Android 的密文离开本机就是废数据**（Keystore 主密钥不可导出）：`passwordEnc` 换台设备解不开。所以两端都必须在导出时把密文/引用**解密成明文**放进 `keys.secrets`（§3.1 已有此设计，现在成为硬要求而非优化）——这也再次印证「勾 SSH 密钥必须加密」和「含密码的分类建议加密」。
3. 导入端拿到明文 secrets 后各自落本端安全存储：iOS 写 Keychain 生成新 `passwordRef`；Android `KeyVault.encrypt` 写回 `passwordEnc`。**备份内的 ref/enc 字段两端都直接丢弃重生成**。

### 2.3 全平台审计（桌面端 Tauri：Linux/Windows/macOS）

桌面端审计结论（代码位置见下）：

**存储点**（配置目录 `app_config_dir`，可用 `ANYBOOT_CONFIG_DIR` 重定向）：

| 文件 | 格式 | 内容 | 对应移动端域 |
|---|---|---|---|
| `remote-servers.json` | JSON（snake_case） | `RemoteStore{connect_all, servers: [RemoteServer]}` | `servers` 电脑列表 ✅ |
| `hub-servers.json` | JSON 数组 | `HubServer`（SSH/Telnet 服务器，密码 AES-GCM 密文） | `jump_servers` ✅ |
| `keys/` 目录 | OpenSSH PEM 文件（0600） | 私钥本体；**无元数据 JSON**，清单运行时派生 | `ssh_keys` ✅（形态不同） |
| `hub-vault.key` | 32B 裸密钥（0600） | 解密 `password_enc` 的主密钥 | iOS Keychain / Android Keystore 的对应物，**但是文件、可拷贝** |
| localStorage `ab-term-settings` | Webview 存储 | 终端偏好（fontSize/cursorBlink/copyOnSelect/bell/scrollback/fontFamily） | `terminal_*`（仅部分对应） |
| localStorage `anyboot.transport` | Webview 存储 | auto/ws/http | `transport`（**值域不同**） |
| `settings.json` | JSON | 本机服务端配置（token/machine_id/窗口/GRUB/通知/主题/authkey…） | 无对应（桌面专属） |
| `hub-tunnels.json` / `transfer-queue.json` / `gateway/` / `tsnet/state/` / 日志 / 锁文件 | — | 端口转发档案 / 断点续传 / 二进制缓存 / 节点凭据 | 桌面专属 |

**桌面端排除项（v1 不备）**：`settings.json` 主体（`machine_id` 恢复会让两台真机被当成一台；窗口几何/显示器名/ESP UUID/本机路径不可迁移；`http_token`/`gateway_named_token`/`tsnet_authkey` 明文高敏且属本机服务端配置）、`hub-tunnels.json`（`local_port` 设备相关）、`transfer-queue.json`（`local_path` 失效）、tsnet 状态（节点凭据天然设备绑定）、cookie/缓存/日志。**`hub-vault.key` 不进备份**——secrets 走明文化路线（下述），密钥文件留本机。

**关键差异与推论**：
1. **命名风格**：移动端 camelCase，桌面端 snake_case。canonical 取 camelCase（三端里两端已是），桌面适配器 snake→camel 一层映射搞定。
2. **桌面能解密自己的密文**（`hub-vault.key` 是随目录走的文件），导出时解 `password_enc` 成明文 → 与移动端同一条 `keys.secrets` 路线，三端统一。
3. **桌面私钥口令不归 App 管**（ssh-keygen 自身保护），`keyPassphrase` 桌面侧导出恒空、导入时忽略——三端中桌面在这一点上是「只出不进」。

**三端字段级映射（canonical 键 = 备份内的键名）**：

电脑档案（canonical ← iOS `ServerProfile` / Android JSON / 桌面 `RemoteServer`）：

| canonical | iOS | Android | 桌面 | 备注 |
|---|---|---|---|---|
| `id`/`name`/`addresses`/`token`/`macs`/`pairs` | 同名 | 同名 | 同名 | ✅ 三端一致 |
| `machineId`/`machineName`/`machineOs` | 同名 | 同名 | `machine_id`/`machine_name`/`machine_os` | 桌面改名校验 |
| `probeMode`/`routeProbeMode`/`probeTimeoutMs` | 同名 | 同名 | `probe_mode`/`route_probe_mode`/`probe_timeout_ms` | 同上 |
| `routeTimeoutsMs`/`blockProbeMode`/`blockTimeoutsMs` | 同名 | 同名 | `route_timeouts_ms`/`block_probe_mode`/`block_timeouts_ms` | 同上 |
| `routeOrder`/`routeDisabled` | 同名 | 同名 | `route_order`/`route_disabled` | 同上 |
| `jumpServerId` | 同名 | 同名 | `jump_id` | 改名 |
| `powerRoutes` | 同名 | 同名 | `power_routes` | 子字段：两端 `httpFallback` vs 桌面 `http_fallback`（旧字段，桌面读取时按 mode 派生，照抄即可） |
| `connMode`（废弃） | 有 | 有 | `conn_mode` | 三端都只读迁移，canonical 保留用于旧备份兼容 |
| `group` | 有 | **无** | **无** | iOS 独有，跨端导入时他端忽略 |
| `created_at` | **无** | **无** | 有 | 桌面导入移动端备份时补当前时间 |
| `pairs[]` 内部 | camelCase 全对齐 | 同 | snake_case + 多 `note`/`created_at` | 幂等键 `(mid, kind, lane)` 三端一致 ✅ |

SSH 服务器（canonical ← iOS `JumpServer` / Android `JumpServer` / 桌面 `HubServer`）：

| canonical | iOS | Android | 桌面 | 备注 |
|---|---|---|---|---|
| `id`/`name`/`host`/`username`/`note`/`termPath`/`termShell`/`lastRttMs` | 同名 | 同名 | 同名/`notes`/`term_path`/`term_shell`/无 | 桌面 `note`→`notes` 改名；`lastRttMs` 桌面无（导入忽略） |
| `protocol` | **`protocolRaw`** | `protocol` | `protocol` | iOS 适配改名 |
| `port`/`addresses` | 无（host 内嵌 :port） | 无 | 有（结构化） | canonical 增加 `addresses`（桌面原生）；移动端导出时从 host 拆出单元素数组，导入时若 `addresses` 非空则回填 host |
| `authType`/`keyId` | 同名 | 同名 | `authType`/`key_id` | 桌面 `keyId`=keys/ 文件基名 |
| `jumpServerId` | 同名 | 同名 | `jump_id` | 改名 |
| 密码/口令 | `passwordRef`/`keyPassphraseRef`（Keychain 键） | `passwordEnc`/`keyPassphraseEnc`（密文） | `password_enc`（密文，vault 可解） | **全部不入 canonical**，明文进 `keys.secrets`（见 §3.1） |

SSH 密钥（canonical ← iOS `SshKeyEntry` / Android `SshKeyEntry` / 桌面 `keys/` 目录）：

| canonical | iOS | Android | 桌面 |
|---|---|---|---|
| `id`/`name`/`algorithm`/`fingerprint`/`publicKey` | 同名 | 同名 | 桌面运行时派生（`kind`→`algorithm`、fingerprint 同算法）；`name` 无落盘（用 id 兜底） |
| 私钥 | `privateRef`→Keychain | `privateEnc`→解密 | `keys/<id>` 文件直读 |
| 口令 | `keyPassphraseRef`→Keychain | `keyPassphraseEnc`→解密 | **无**（ssh-keygen 管），导出空、导入忽略 |

偏好/其他：终端偏好桌面仅 `fontSize`/`fontFamily` 可映射（`terminalFontSize`/`terminalFontFamily`），桌面专属 4 项（cursorBlink/copyOnSelect/bell/scrollback）v1 不跨端；`transport` **值域不同**（移动 auto/push/poll vs 桌面 auto/ws/http），跨端导入只迁 `auto`，平台特有值过滤为默认；`connect_all` 三端对齐 ✅；会话历史/TOFU 桌面**无此域**（勾选时桌面导出为空表、导入忽略）。

## 3. 备份文件格式

- 扩展名 `.anyboot`（明文与加密同扩展名，靠 envelope 字段区分）；同时接受 `.json`。
- 注册 UTI（iOS：`Exported Type UTI`，`com.anyboot.backup`，conforms-to `public.json`）/ IntentFilter（Android `application/anyboot` + json 兜底），支持「用其他应用打开 → AnyBoot」直接进入导入流程。

### 3.1 明文结构

```json
{
  "format": "anyboot-backup",
  "version": 1,
  "app": "1.2.0",
  "platform": "ios",            // ios | android
  "exportedAt": "2026-10-27T11:30:00Z",
  "includes": ["servers", "jumps", "keys", "terminal", "history"],
  "sync": {                       // 仅云同步文件携带；手工导出为 null
    "deviceId": "<每台设备一个 UUID>",
    "rev": 3,                     // 每次上传 +1，配合 If-Match 防丢更新
    "tombstones": [               // 删除墓碑：{ "cat": "servers", "id": "…" }
      { "cat": "jumps", "id": "A1B2" }
    ]
  },
  "data": {
    "servers":   { "items": [ /* ServerProfile，字段=现有 JSON 键 */ ], "activeId": "…" },
    "jumps":     { "items": [ /* JumpServer */ ] },
    "keys":      { "items": [ /* SshKeyEntry 元数据（无 ref 字段）*/ ],
                   "secrets": { "key:<id>:private": "PEM", "key:<id>:passphrase": "…",
                                "jump:<id>:password": "…", "jump:<id>:keyPassphrase": "…" } },
    "terminal":  { "terminalDark": true, "terminalFontSize": 14, "terminalFontFamily": "",
                   "terminalAutoKeyboard": false, "terminalKeyBar": true },
    "history":   { "sessions": [ /* HistoryEntry */ ],
                   "trustedHosts": [ { "host": "…", "fingerprint": "SHA256:…" } ] }
  }
}
```

要点：
- **内层记录字段用规范键名（canonical keys，见 §2.2）**：审计发现两端 JSON 键在敏感字段与个别命名上并不一致（`protocolRaw` vs `protocol`、`passwordRef` vs `passwordEnc`），所以备份内统一用规范键，两端各写一层薄适配器；**ref/enc 等安全存储字段不入备份**，统一由 `keys.secrets` 明文（仅在加密备份内）+ 导入端重生成。
- `keys.secrets`：**稳定键**（`key:<id>:private` / `jump:<id>:password` 等，与平台各自的 ref/enc 机制解耦）的明文表，只存在于加密备份中（明文备份若勾了密钥，导入端给一次性强警示；见 §4 决策）。导入端按稳定键回填本端安全存储并重生成 ref/enc。
- 日期统一 **ISO8601 字符串**（新格式新规则；导入端把老备份/两端差异统一转成本端 Date 表示）。`exportedAt`、`createdAt`、`startedAt` 等。
- `version` 兼容策略：导入端遇到 `version > 本端支持` → 友好报错「备份来自更新版本，请先升级 App」；`version ≤` → 永远可导（沿用现有「1.1 单 host 档案兼容」的 decode 容错思路）。
- **同步元数据（§10 用）**：`sync.deviceId` 标识来源设备；`sync.rev` 单调递增配合 WebDAV `If-Match` 防丢更新；`sync.tombstones` 记录已删除条目（墓碑），防止「A 端删除、B 端又同步回来」。手工导出的文件 `sync` 恒为 `null`。
- **记录级 `updatedAt`（同步前置改造）**：`ServerProfile`/`JumpServer`/`SshKeyEntry` 本轮各加一个 `updatedAt` 字段（缺省可 decode，老档案自动补当前时间），两端所有 upsert 路径统一 touch。这是按条目 LWW 合并的前提。

### 3.2 加密信封（encryption 可选）

勾选加密时，上面的明文 JSON 整体作为 `data` 被加密，envelope 变为：

```json
{
  "format": "anyboot-backup",
  "version": 1,
  "encryption": {
    "scheme": "PBKDF2-SHA256-310000 + AES-256-GCM",
    "salt": "<base64, 16B>",
    "iv": "<base64, 12B>",
    "rounds": 310000
  },
  "payload": "<base64( ciphertext + 16B GCM tag )>"
}
```

- KDF：PBKDF2-HMAC-SHA256，310000 轮（OWASP 2023 推荐），盐 16B 随机。
- 对称：AES-256-GCM，IV 12B 随机，附加数据 = envelope 里除 `payload` 外的字节（防篡改 version/rounds）。
- 实现落点：iOS `CryptoKit`（`HKDF`不用，用 `CommonCrypto CCKeyDerivationPBKDF` + `AES.GCM`）；Android 沿用 `KeyVault.java` 已有的 AES-GCM + KeyStore 思路，但这里**不用 Keystore**（口令派生密钥，跨设备可解），用 `javax.crypto` 的 PBKDF2WithHmacSHA256 + AES/GCM/NoPadding。
- 口令规则：最少 6 位；两次输入确认；**口令不落盘、不进备份文件**；导入时输错 → GCM 校验失败 → 统一报「口令错误或文件已损坏」，不区分两种情况（安全语义）。

## 4. 导出流程

```
抽屉「备份与恢复」→ 导出页
  ├─ 分类清单（5 行，checkbox + 计数「电脑列表（3 台）」+ 敏感徽标）
  │    └─ 勾「SSH 密钥」→ 黄色警示 + 口令设置区展开
  │    └─ 勾 1/2 且未加密 → 灰色信息条「建议设置口令加密」
  ├─ （可选）口令设置：输入 + 确认 + 明文强度提示
  ├─ 引用完整性信息条（如「2 台电脑关联的跳板机未勾选」）
  └─ [导出到文件]
       → 组包（收集 + 序列化 + 可选加密，全部内存完成）
       → 系统分享面板（文件名 anyboot-backup-20261027-1130.anyboot）
       → 分享完成即删临时文件
```

细节：
- 计数实时反映勾选状态；空分类（如没有密钥）显示「SSH 密钥（0）」且可勾但导出后该项为空表。
- 文件名：`anyboot-backup-YYYYMMDD-HHmmss.anyboot`。
- 导出成功 toast：「已导出 5 类数据（电脑 3 · 服务器 2 · 密钥 1 …）」。

## 5. 导入流程

```
入口三路：① 备份页 [从文件导入]（系统文件选择器）
         ② 系统分享/「打开方式」→ AnyBoot（深链进备份页并带文件）
         ③ 文件 App 里点 .anyboot → 用其他应用打开
        ↓
解析 envelope → 是加密的 → [口令输入页]（GCM 校验失败重输，3 次后提示检查来源）
        ↓
摘要预览页（核心，导入前必看）：
  ┌────────────────────────────────────────────┐
  │ 备份来自 iOS · 2026-10-27 · App 1.2.0      │
  ├────────────────────────────────────────────┤
  │ ☑ 电脑列表    备份 3 条 · 本机 2 条（新增2/覆盖1）│
  │ ☑ SSH 服务器  备份 2 条 · 本机 1 条           │
  │ ☐ SSH 密钥    备份 1 把 · 本机 0 把           │
  │ ☐ 偏好设置    将覆盖本机当前设置              │
  │ ☐ 连接记录    备份 37 条 · 本机 12 条          │
  ├────────────────────────────────────────────┤
  │ 合并策略  (•) 跳过本机已有   ( ) 备份覆盖本机  │ ← 默认=上次选择
  │ 警示：1 台电脑关联的跳板机不在本备份中         │
  ├────────────────────────────────────────────┤
  │           [取消]      [导入所选]              │
  └────────────────────────────────────────────┘
        ↓
执行（逐条独立处理，单条失败不中断）→ 结果报告页
  「导入完成：新增电脑 2 · 覆盖电脑 1 · 新增服务器 2 · 跳过密钥 0 · 失败 0」
  失败项逐行列原因（如「第 3 台电脑：地址字段不合法」）
```

### 5.1 合并语义（逐分类）

| 分类 | 合并（默认） | 覆盖 |
|---|---|---|
| 电脑列表 | 按 `id` 冲突判断：不存在→新增；已存在→按策略跳过/覆盖 | `servers` 整表替换 + `activeId` 用备份的 |
| SSH 服务器 | 同上（`id`） | `jump_servers` 整表替换 |
| SSH 密钥 | 同上（`id`）；导入时把 `secrets` 写回 KeyVault（沿用备份里的 ref 作为 account 键，ref 是 UUID 无碰撞风险） | `ssh_keys` 整表替换 + 清掉本机不在备份内的密钥材料 |
| 偏好设置 | 无「跳过」语义：勾选即覆盖全部偏好项（terminal 5 项 + transport + connect_all） | 同左 |
| 连接记录 | 历史**追加**按 `(serverId, startedAt)` 去重；指纹按 host 覆盖写 | 同左（历史本来就是追加型） |

- 「记住上次选择」：`UserDefaults/prefs` 存 `backup_import_strategy`，预览页默认选中上次的。
- id 冲突时预览行内展开明细（可选，v1 可只给汇总计数）：长按/点行看「将覆盖：黑苹果（127.0.0.1:12300）」。

## 6. 文案清单（两端逐字一致）

- 页面标题：`备份与恢复`
- 导出区标题：`导出`；按钮：`导出到文件`
- 导入区标题：`导入`；按钮：`从文件导入`
- 分类名（§2 表列 2 原文）
- 警示：`此分类包含私钥，必须设置口令加密`、`建议设置口令加密以保护令牌`、`N 台电脑关联的跳板机未包含在本次导出`
- 合并策略：`合并策略`、`跳过本机已有`、`备份覆盖本机`
- 口令页：`输入备份口令`、`口令错误或文件已损坏`
- 结果：`导入完成`、`失败 N 项`、`该分类在本备份中为空`
- 版本错误：`备份来自更新版本，请先升级 App`
- 同步分组：`启用多端同步`、`通过 WebDAV 在多台设备间保持所选数据一致`、`WebDAV 与同步内容`、`立即同步`、`上次同步：…`、`请先配置 WebDAV 服务器和同步内容`
- 配置页：`服务器地址`、`用户名`、`密码`、`远端同步目录`、`同步周期`、`记录变化时就同步`、`建议只在自有 WebDAV 服务器时开启，防止快速占用服务商限额。`、`同步的数据类型`、`包含私钥与登录密码，请勿同步到不可信 WebDAV`、`建议开启加密，私钥将以明文存放于 WebDAV 服务器`、`测试连接`
- 同步报告：`同步完成`、`有 N 处修改被另一台设备覆盖`、`同步失败：…`

## 7. 实现拆分

### iOS（预计 4 个新文件 + 少量接线）

| 文件 | 职责 |
|---|---|
| `Backup/BackupModels.swift` | envelope struct、分类 enum、各分类 payload 的 Codable 壳（内层直接复用 ServerProfile/JumpServer/SshKeyEntry） |
| `Backup/BackupCrypto.swift` | PBKDF2 + AES-GCM 封装（纯函数，单测友好） |
| `Backup/BackupStore.swift` | collect(categories) -> Data；parse(data) -> BackupSummary；apply(summary, strategy) -> ImportReport。全部纯函数 + 现有 Store 注入，不碰 UI |
| `UI/BackupView.swift` | 导出页 + 导入预览页 + 口令页 + 结果页（沿用 token/手绘图标体系，全屏页形态同终端设置页） |

接线点：`ABDrawer` 加菜单项 `备份与恢复` → `nav.go(.backup)`；`ABPage` 加 `.backup`；分享入口（`onOpenURL`）带文件进导入预览。

### Android（对称）

`BackupManager.java`（collect/parse/apply，操作 ServerStore/JumpStore/KeyVault）+ `BackupCrypto.java`（PBKDF2/AES-GCM）+ `BackupActivity.kt`（同四页流程，UI 按 Android 现行规范）+ Manifest 注册 `.anyboot` IntentFilter + 抽屉入口。

### 关键实现注意

- KeyVault 读密码失败（如 iOS 换机后 Keychain 项丢失）→ 导出前逐条预检，失败的条目**跳过并在导出结果里列明**，不静默导出空值。
- 导入密钥时 KeyVault 写入失败的兜底：条目跳过、报告失败原因。
- 大文件防御：> 10 MB 直接拒收（现数据量远小于此，防恶意文件）。
- 解析全程 try-per-item：**任何一条坏数据不影响其余**。

### 7.1 桌面端（Tauri，一份代码覆盖 Linux/Windows/macOS）

| 位置 | 职责 |
|---|---|
| `src-tauri/src/backup.rs`（新） | envelope 读写、canonical ↔ 本端模型适配（snake→camel）、`hub_vault` 解密导密、`keys/` 目录打包/落盘、合并 apply（复用 remote_servers/hub 的 upsert 路径） |
| `src-tauri/src/backup_crypto.rs`（新） | PBKDF2 + AES-256-GCM（RustCrypto：`pbkdf2`/`aes-gcm` crate），与移动端互解密必须逐字节对齐（盐/IV/轮数/tag 位置） |
| WebDAV | `reqwest`（dav 特性或手拼 PROPFIND/PUT）+ `SyncEngine`（pull→merge→push，复用 backup.rs 的 apply） |
| 前端 | hub 页新增「备份与恢复」入口页（复用现有设置页骨架）；导出经 Tauri dialog.save + fs 写文件；导入 dialog.open |
| localStorage 适配 | 终端偏好/transport 从 Webview localStorage 读出并入分类（导出时前端传给后端，导入时后端回传前端写入） |

桌面分类落点：电脑列表=`remote-servers.json`；SSH 服务器=`hub-servers.json`（密码经 vault 解密）；SSH 密钥=`keys/` 目录（含公钥 `.pub`）；偏好设置=`ab-term-settings`（映射 2 项）+`transport`+`connect_all`；连接记录=空域。

## 8. 验收清单（探针化）

1. 全类导出（加密）→ 清空数据 → 导入 → 逐字段断言与导出前一致（含电源路由、RemotePair lane、termShell 等深字段）。
2. 只导电脑列表（有关联跳板机）→ 换库导入 → 断言「跳板机待配置」报告 + 档案完整。
3. iOS 导出 → Android 导入 / 反向 → 同上断言（跨端用同一份 fixture 文件）。
4. 口令错误 3 次、坏 JSON、version+1 文件、10 MB 大文件、空分类勾选。
5. 合并两种策略 × id 冲突/不冲突矩阵；「记住上次策略」断言。
6. 密钥导入后**真实连一次 sshd**（私钥可用性端到端）。
7. **双端同步收敛矩阵**：A 端新增 1 台 / B 端改同一台的名字 / C 端删 1 台 → 三端各自触发同步 → 断言三端最终一致（新增传播、改动 LWW 胜出、删除不复活）。
8. 加密同步文件跨端解密（口令一致可解、不一致报错）；未加密文件两端正读。
9. WebDAV 异常路径：404 首推、`If-Match` 冲突后重拉再推、断网重试退避、配额满报错文案。
10. **桌面 ↔ 移动互导矩阵**（fixture 循环：桌面→iOS、iOS→桌面、桌面→Android、Android→桌面）：断言电脑档案深字段（电源路由/macs/pairs 幂等键）在 snake↔camel 映射后无损；跳板机 protocol 改名映射；密钥三端落库后**真实 SSH 登录成功**（桌面 keys/ 文件 0600 权限断言）；桌面导出的 jump 密码（vault 解密）在 iOS 导入后 Keychain 可读。
11. 跨端过滤断言：iOS `group` 导入 Android/桌面被忽略；`transport=push` 导入桌面回落 `auto`；桌面备份导入移动端时 `created_at`/`note` 被忽略、终端偏好桌面专属 4 项被忽略；会话历史/TOFU 桌面为空域不报错。

## 9. WebDAV 云同步（本轮一并实现）

形态借鉴 NipaPlay（备份页放同步总开关 + 配置入口 + 立即同步；配置页管服务器/目录/周期/数据类型），语义按上面确认的决策落实。

### 9.1 页面结构

备份页（§4/§5 之上）新增「多端同步」分组：

```
┌ 启用多端同步  ──────────────── [开关] ┐
│ 通过 WebDAV 在多台设备间保持所选数据一致 │
├ WebDAV 与同步内容                  > ┤   ← 未配置时「立即同步」置灰并提示
├ 立即同步                            > ┤   ← 行内显示上次同步时间/结果
└─────────────────────────────────────┘
```

WebDAV 配置页（全屏，两端各按自家规范，文案逐字一致）：

- `服务器地址`（https://…）、`用户名`、`密码`（密码存 KeyVault，可明示切换）
- `远端同步目录`：默认 `/AnyBoot/sync`，可改
- `同步周期`：`手动` / `15 分钟` / `30 分钟` / `60 分钟`（下拉；手动=只靠变化触发和立即同步）
- `记录变化时就同步`（开关，去抖 30 秒）+ 说明行：`建议只在自有 WebDAV 服务器时开启，防止快速占用服务商限额。`
- `同步的数据类型`：复用 §2 的 5 分类 checkbox（每项一行说明）；「SSH 密钥」项说明改为 `包含私钥与登录密码，请勿同步到不可信 WebDAV`
- `同步文件加密`（开关，默认关）+ 口令设置；勾选「SSH 密钥」且未开加密时黄条强警示 `建议开启加密，私钥将以明文存放于 WebDAV 服务器`
- 底部：`测试连接`（PROPFIND 远端目录，404 时提示可自动创建）｜`取消`｜`保存`

### 9.2 同步数据流（按条目合并）

```
触发（周期 / 变化去抖 / 手动）→ 加锁（同步中不重入）
  → GET {dir}/sync.anyboot
      404 → 本地全量组包（含 tombstones）→ PUT（首推）
  → 解密（若开）→ 校验 envelope
  → 逐分类合并到本地（9.3）→ 得到「合并后的全量 + 合并后 tombstones」
  → 组包 rev = 远端 rev + 1 → PUT（If-Match: 远端 ETag；无 ETag 则覆盖，
     下轮 pull 靠条目合并自愈 —— 最终一致）
  → 记录上次同步时间/结果；冲突与失败条目进同步报告
```

- 同步文件复用 §3 envelope（`sync` 非 null），**云端永远只有一份当前态**，不做历史版本。
- `tombstones` 随合并结果累积；v1 不做 GC（每条 60B，万条删除才 600KB），预留 `rev` 重置点。

### 9.3 合并语义（逐条 LWW，两端一致）

| 情形 | 处理 |
|---|---|
| 本地无、远端有（非墓碑） | 新增到本地 |
| 本地有、远端无 | 保留本地（下次上传带出去） |
| 两边都有 | 比 `updatedAt`，新者胜；败者的改动丢弃并记入同步报告（`「黑苹果」在另一台设备上的修改被覆盖`） |
| 远端墓碑命中本地条目 | 删除本地条目（墓碑优先级高于条目本身） |
| 本地删过的 | 写入 tombstones（本地删除登记是同步开关打开后的行为，开启前的删除无法追溯） |
| 偏好设置 / 历史记录 | 终端设置=单值 LWW（updatedAt 语义按最后写入）；历史=按 `(serverId, startedAt)` 去重追加 |

- 文件导入（§5）与同步合并共用同一个 `apply` 核心：导入=用户选策略的一次性 apply；同步=固定 LWW+墓碑的 apply。

### 9.4 状态与可靠性

- 同步中不重入（进程内互斥）；失败指数退避（30s/2m/10m），连续失败在备份页行内显示错误摘要。
- iOS 周期同步用 `BGTaskScheduler`（app 刷新任务）+ 前台 Timer；Android 用 `WorkManager` 周期任务。前台「立即同步」永远可用。
- 流量防御：本地数据无变化且远端 rev/ETag 未变 → 跳过上传（只花一个 GET）。
- 移动网络下周期同步默认仍执行（数据量 KB 级）；后续可加「仅 Wi-Fi」开关（预留）。

### 9.5 实现拆分（在 §7 基础上追加）

| 端 | 新增 | 职责 |
|---|---|---|
| iOS | `Backup/WebDavClient.swift` | 轻量 WebDAV：GET/PUT/PROPFIND/MKCOL + ETag/If-Match（URLSession，Basic Auth） |
| iOS | `Backup/SyncEngine.swift` | pull→merge→push 状态机、去抖、周期调度、tombstone 登记钩子（挂各 Store 的 remove 路径） |
| Android | `WebDavClient.java` + `SyncWorker.java` | 对称实现，WorkManager 周期 + 变化去抖 |
| 两端 | prefs 新键 | `webdav_url/user/ref、sync_enabled、sync_period、sync_on_change、sync_encrypt、sync_categories、sync_last_result` |

前置改造（两端）：模型加 `updatedAt`（见 §3 要点）；`KeyVault` 增加「读出明文导出」通道（备份/同步共用，§7 已列）。

## 10. 未决（实现前需确认）

- 终端设置的「合并」没有跳过语义（勾选即覆盖），是否接受？（建议接受，5 项都是标量；同步侧按最后写入胜出）
- 分组字段 `group`（iOS 已删 UI 但字段还在）随备份走——导入端若也是 iOS 新版无此字段则自动忽略，无需处理。
- 桌面端 `settings.json` 全体排除在备份外（含 http_token/gateway_named_token 等本机服务端凭据）——若将来要支持「整机迁移桌面配置」，需另立「桌面专属备份」格式，与互通备份分开。
- 桌面 SSH 私钥口令不对称（ssh-keygen 自管，App 导不出也导不进）：带口令的移动端密钥导入桌面后，桌面侧使用时 ssh 会现场要口令——文档已注明，是否需要导入时的提示文案？
- 同步的「被覆盖改动报告」v1 只在备份页展示最近一次的条目数，点开看明细；是否需要单独的报告页？（建议 v1 从简）
- 后续扩展位：`data` 下加 `"meta": { "deviceName": … }`、仅 Wi-Fi 同步开关、SMB/S3 协议、墓碑 GC、hub-tunnels 端口转发档案备份——envelope 预留 `version` 递增即可。
