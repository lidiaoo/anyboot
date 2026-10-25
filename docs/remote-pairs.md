# 远程接入配对模型（多条目）

> 状态：设计稿（未实现）
> 涉及：`remote_servers.rs` / `http_server.rs` / `hub_gateway.rs` / `ui/hub.html` / `mobile/{android,ios}`
> 范围：**只做配对**。`cloudflare` / `ts` / `tsnet` 三个区块各自可挂多条配对，全部保留、全部独立探测。
> LAN 直连（`addresses` / `probe_mode`）与跳板机（`jump_id`）不在范围内，结构与探测代码保持原样。
> **不做向后兼容**：旧的单值槽位直接删除，不迁移、不读取、不双写。

## 一、现状

### 1.1 结构：按类型固定命名的单值槽

`RemoteServer`（`remote_servers.rs`）与移动端 `ServerProfile` 的远程通道都是
**一个类型一个 String 槽**：

```
remote_tunnel_addr   ← cloudflare
remote_ts_addr       ← tailscale 直连（100.64/10）
remote_funnel_addr   ← 系统 tailscale funnel（*.ts.net）
remote_tsnet_addr    ← 内置 tsnet（*.ts.net）
remote_token         ← 单值令牌
```

一个槽只能放一条。同一类型第二条配对**无处可放**，只能覆盖第一条。

### 1.2 历史解法是「再加一个槽」，已到极限

代码注释里记着两次同类事故：

- `remote_ts_addr`：曾兼收 `*.ts.net`，系统 Funnel 与 100.x 直连抢一个槽 → 拆出 `remote_funnel_addr`
- `remote_tsnet_addr`：内置 Funnel 与系统 Funnel 域名形态相同，混存必然互相顶替 → 再拆一槽

拆槽能解决「两个通道类型撞车」，解决不了「同一类型有多条」。**槽位是有穷枚举，而系统数量是变量**
—— 加到八个槽，遇到「同一台机器的两个系统各配一条 cloudflare」照样塌。

### 1.3 配对串不携带来源身份

`hub_gateway.rs` 现在签发：

```
anyboot://pair?gateway=<主机名>&tunnel=<url>&token=<token>&type=<kind>
```

移动端 `ServerProfile.parsePair` 只取 `tunnel` / `token` / `type`，`gateway` 被忽略，
**串里没有对方系统标识**。所以两张来自不同系统的配对码，客户端无法判断是否同一来源，
重复扫同一张码也无从去重。这是「顶替」之外的另一半问题：缺一个幂等依据。

## 二、目标与非目标

**目标**

1. 三个区块各自可挂多条配对，全部保留、全部独立探测、每条各带自己的超时。
2. 每条配对能显示「来自哪台机器、哪个系统」（`mid` / `os` / `host`）。
3. 重复配对同一系统时不产生重复行（按 `(mid, kind)` 幂等）。
4. PC / 移动端同一套字段契约、同一套平铺列表。
5. **「顶替」只剩一个合法场景**：同一系统、同一区块重新配对 = 更新那一行。

**非目标**

- **不做向后兼容**。见 §2.1。
- **不做分组**。没有「电脑」这一层，档案靠备注与 `host · os` 区分，PC / 移动端统一平铺。
  「配对后选分组、组内顶替」的方案在「一台机装 Win10 + Win11 + macOS 12/13/15 + Arch + Fedora」
  下会把六个系统削成三个 —— 分组粒度恰好等于顶替力度。
  移动端现有的 `group` 字段与分组折叠 UI（`ServerProfile.java` 的 `groupColor` / 分组排序前缀 /
  带组名的 `displayName`）**一并删除**。
- **不做 MAC 推导 / 机器指纹**，不猜「这两条档案是不是同一台机器」。
- **不做自动建档**。配对落到用户当前打开的档案，由用户决定放哪里（§4.6）。
- **不碰 LAN 与跳板机**：`addresses` / `probe_mode` / `jump_id` 保持原样；
  `route_order` / `route_timeouts_ms` 同时被 LAN 选路消费，本次不改其结构（§5.1）。
- 不做跨系统配置同步，不引入中心服务或账号体系。

### 2.1 破坏性变更（一次性，已确认接受）

| 变更 | 后果 |
|---|---|
| 删除 `remote_tunnel_addr` / `remote_ts_addr` / `remote_funnel_addr` / `remote_tsnet_addr` / `remote_token` / `remote_addr` | 老档案里的这些内容**直接丢弃**。serde 默认忽略未知字段，读档不报错，只是 `pairs` 为空 |
| 删除 `route_disabled` | 被 `pair.enabled` 取代，老值丢弃 |
| 删除移动端 `group` 及分组折叠 UI | 列表改平铺，老分组名丢弃（档案本就靠备注与 `host · os` 区分） |
| 配对串升级到 `v=2` | **老串（无 `v=2`）拒收**，提示「这是旧版配对码，请在新版重新生成」 |
| 探测结果增加 `pair_id` | 老结果行不再按 `kind` 匹配，升级后首次探测整轮重算 |

**用户可见影响：升级后每台电脑的远程接入都是未配置状态，需要重新扫一次码（或手填地址）。**
LAN 直连不受影响，仍在原处可用。

不做迁移的理由：四个老槽位能表达的只是「每类一条」，把它搬进 `pairs` 会得到四条 `mid` 为空、
来源未知的条目 —— 既不能参与幂等去重，也不能显示来源，还得为它们保留一套「无 mid」的特判路径。
一次性重配比长期背着这套特判干净得多。

## 三、模型

```
档案（一套 AnyBoot 配置，一个 machine_id）
├── 区块 cloudflare   pair#1  https://a-1.trycloudflare.com
├── 区块 ts           pair#2  http://100.118.110.36:12300      (lane=direct)
│                     pair#3  https://desktop-8f3a.xxx.ts.net  (lane=funnel)
└── 区块 tsnet        pair#4  https://anyboot-win.xxx.ts.net
```

| 层 | 载体 | 说明 |
|---|---|---|
| 档案 | `RemoteServer.id` | 一条档案 = 一套配置 |
| 区块 | `kind` ∈ `cloudflare` / `ts` / `tsnet` | 固定三个，是探测策略与超时的一级分组 |
| 配对条目 | `RemotePair.id` | 探测与连接的最小单元 |

**`ts` 区块合并了原来的「直连」与「系统 Funnel」**，用 `lane` 区分：

| lane | 地址形态 | 典型超时 |
|---|---|---|
| `direct` | `http://100.x.x.x:12300` | 1.5s |
| `funnel` | `https://<node>.<tailnet>.ts.net` | 5s |

合并的依据是两者本来就同属「电脑上装的那个 Tailscale」，原先拆槽只是为了避免互顶；
多条目之后互顶消失，拆槽的理由也不存在了。合并后同区块内两种形态各设超时，
这正是条目级超时存在的意义。

**顶替的唯一合法场景**：同一 `(mid, kind)` 重新配对 → 更新那一行的 `addr` / `token` / `tunnel_id`。
注意键是 `(mid, kind)` 而不是 `mid`：同一个系统可以同时有 cloudflare + ts + tsnet 三条，
只按 `mid` 去重会让它们互顶，等于回到原点。

## 四、数据契约

### 4.1 `RemotePair`（`remote_servers.rs`）

```rust
/// 一条远程接入配对：一个系统的一套通道。独立保存、独立探测、独立删除。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RemotePair {
    /// 本地唯一 id（配对/手填时生成，永不复用）。探测结果按它回填
    pub id: String,
    /// 所属区块：cloudflare / ts / tsnet
    pub kind: String,
    /// 仅 kind=ts 有意义：direct / funnel。展示与默认超时用
    #[serde(default)]
    pub lane: String,
    /// 生效 origin：https://xxx.trycloudflare.com 或 http://100.x.x.x:12300
    pub addr: String,
    /// 该条目的网关客户端令牌（对方签发；空 = 该通道不需要）
    #[serde(default)]
    pub token: String,
    /// 本条目的探测超时（毫秒）。0 = 继承 block_timeouts_ms[kind]，再继承全局 probe_timeout_ms
    #[serde(default)]
    pub probe_timeout_ms: u64,
    /// 是否参与探测（单条粒度）
    #[serde(default = "serde_default_true")]
    pub enabled: bool,
    /// 对方系统自报的 machine_id —— 幂等键的一半
    #[serde(default)]
    pub mid: String,
    /// 对方自报的系统族（windows / macos / linux）
    #[serde(default)]
    pub os: String,
    /// 对方自报的主机名（列表显示用）
    #[serde(default)]
    pub host: String,
    /// named tunnel 的 id：识别「同一个隧道被多套系统共用」
    #[serde(default)]
    pub tunnel_id: String,
    /// 用户备注（纯显示）
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub created_at: u64,
}
```

### 4.2 `RemoteServer` 字段变更

**新增**

```rust
/// 远程接入配对列表（数组顺序 = 区块内探测顺序）
#[serde(default)]
pub pairs: Vec<RemotePair>,
/// 每个区块内部的探测策略：parallel_first / parallel_all / serial。缺项 = parallel_first
#[serde(default)]
pub block_probe_mode: std::collections::BTreeMap<String, String>,
/// 每个区块的默认探测超时（毫秒），被 pairs[].probe_timeout_ms 覆盖。缺项 = 全局默认
#[serde(default)]
pub block_timeouts_ms: std::collections::BTreeMap<String, u64>,
```

**删除**（连同所有消费点一起改）

| 字段 | 替代 |
|---|---|
| `remote_tunnel_addr` / `remote_ts_addr` / `remote_funnel_addr` / `remote_tsnet_addr` | `pairs` 内对应 `kind` / `lane` 的条目 |
| `remote_token` | `RemotePair.token`（每条自带） |
| `remote_addr` | 计算函数 `effective_remote_addr()`：按区块顺序 + 区块内顺序，取第一条 `enabled` 的 `addr`。**它不再是存储字段**，探测 / HTTP / iframe / 终端等消费点统一改读它 |
| `route_disabled` | `RemotePair.enabled` |

**保留不动**（同时被 LAN / jump 消费，重构会越界）

| 字段 | 处置 |
|---|---|
| `probe_mode` | 原样。**不能**并入 `block_probe_mode`：LAN 地址是自动收集、随 DHCP 漂移、按 MAC 归一；配对是手填、稳定、按条目禁用。合成一个字段会让「DHCP 换了 IP 该替换还是新增」失去判断依据 |
| `route_probe_mode` | 原样，语义自然收窄为「三个配对区块之间」 |
| `route_order` | **结构不动**（仍含 `lan` / `jump` / 四个配对 kind），区块顺序从它的配对项推导，见 §5.1 |
| `route_timeouts_ms` | 结构不动，配对侧改读 `block_timeouts_ms`；剩下的 key 只服务 `lan` / `jump`（§5.3） |
| `addresses` / `token` / `machine_id` / `machine_name` / `machine_os` / `jump_id` / `macs` / `power_routes` | 不动 |

**软上限**：每档案 32 条，超出时 UI 拒绝新增并提示。

### 4.3 移动端 `ServerProfile`

`pairs` / `block_probe_mode` / `block_timeouts_ms` 字段名与语义与 PC 端逐字一致；
老的 `remoteTunnel` / `remoteTs` / `remoteFunnel` / `remoteTsnet` / `remoteToken` 一并删除，
`group` 字段与其分组折叠 UI（`groupColor` / 排序前缀 / 带组名的 `displayName`）一并删除，
列表改平铺。

`Task.resume()` 的逐字段拷贝**必须同步带上 `pairs`** —— 历史坑：`Task` 加字段不同步会导致状态丢失。

### 4.4 什么不进 `pairs`

| 项 | 为什么 |
|---|---|
| LAN（`addresses`） | 机器级地址，不属于任何系统；同一时刻只有正在运行的系统响应 |
| 跳板机（`jump_id`） | 档案级的转发路径，不是「某系统的一条接入」 |
| `group` | 不做分组；字段与分组 UI 一并删除，不进档案也不进配对 |

### 4.5 配对串 v2

```
anyboot://pair?v=2&mid=<machineId>&os=<os>&host=<hostname>&type=<kind>&tunnel=<url>&token=<token>&tunnel_id=<id>
```

- `v` 必须等于 `2`，否则拒收并提示重新生成。**不解析老串**。
- `type` 取值限定 `cloudflare | ts | tsnet`，非法值拒收。
- `lane` 由地址形态推导（`100.x` → direct，`*.ts.net` → funnel），不占串参数。
- `gateway` 保留（显示用，不是归档依据）。

### 4.6 归档规则（客户端收到配对串后）

```
1. 目标档案 = 用户当前打开/选中的那条（没有则先引导新建）
2. 在该档案内按 (mid, kind) 找 pair
   ↳ 找到 → 更新 addr / token / tunnel_id（唯一合法的「顶替」）
   ↳ 没找到 → 追加到对应区块末尾
3. mid 与档案的 machine_id 不一致
   → 只提示「这张码来自 <host>（<os>），与档案记录不同」，由用户确认后添加；
     不拦截、不自动改档案的 machine_id
```

### 4.7 `mid` 的语义

`machine_id` 存在 `settings.json` 里，因此它的粒度是「**一套 AnyBoot 配置**」，不是「一次操作系统安装」：

| 部署形态 | `settings.json` | `mid` | 结果 |
|---|---|---|---|
| Win + Linux + macOS（各自 home） | 三份 | 三个 | 三条档案，互不覆盖 |
| 双 Windows（各自系统盘） | 两份 | 两个 | 两条档案 |
| 多发行版**共享 home** | 一份 | 一个 | 一个接入身份、一个固定域名，永远连当前在跑的系统 |

共享 home 合并成一个身份与 LAN 地址同构：同一时刻只有一个系统在运行，所以「一个地址永远指向当前系统」
是可接受的。要区分共享 home 的发行版，只能用 `ANYBOOT_CONFIG_DIR` 把配置（含 tsnet 状态）一起分开。

**铁律：`mid` 与 tsnet 状态目录必须同源。** 只把 `mid` 换成系统级 id（如 `/etc/machine-id`）
而共享 tsnet 状态，会得到两个 `mid` 指向同一个 tailnet 节点 —— 两条档案、同一个域名、都显示在线，
点哪条走的都是同一条路。这比顶替更糟：它制造了假的多身份。

`mid` 在配对里只做两件事：幂等键（§4.6）、显示来源（§六）。**不比对探活返回的 `machineId`
去禁用通道** —— 一条档案里本来就允许并存多个系统的配对，任何「当前运行的不是这条记录」式判定都会误报。

## 五、探测

### 5.1 遍历与排序

- 每次探活遍历 `pairs` 全集（跳过 `enabled = false`），**每条独立上报**。
- `ProbeOutcome` 增加 `pair_id: String`，与 `RemotePair.id` 一一对应。
- **区块顺序从 `route_order` 推导，不新增排序字段**：过滤出配对 kind
  （`tunnel / funnel / ts / tsnet`）→ 映射为区块 id（`tunnel→cloudflare`、`funnel|ts→ts`、`tsnet→tsnet`）
  → 去重保序；缺失的区块按默认序（cloudflare → ts → tsnet）追加在后。
  区块内顺序 = `pairs` 数组里同 kind 的相对顺序。

  `route_order` 本身**不改结构**：`lan` / `jump` 仍在其中并被 LAN 选路消费，收窄它就会把 LAN 的排序一起改掉。
  代价是合并后 `ts` 与 `funnel` 在 `route_order` 里各占一个位置 —— 取先出现者定位 `ts` 区块即可，
  两个 kind 本来就指向同一个区块。

### 5.2 两层策略

| 层 | 字段 | 作用域 | 默认 |
|---|---|---|---|
| 区块之间 | `route_probe_mode`（已有） | cloudflare → ts → tsnet 的先后 | `parallel_first` |
| 区块内部 | `block_probe_mode[kind]`（新增） | 同一区块内多条配对之间 | `parallel_first` |

两种策略取值相同：

| 值 | 行为 | 适用 |
|---|---|---|
| `parallel_first` | 同层全部并发，第一个成功即收工，其余标「未探测」 | 日常选路要快 |
| `parallel_all` | 同层全部并发，等全部返回、逐条报状态 | 体检、确认每条是否还活着 |
| `serial` | 按序逐个试，第一个成功即停 | 严格尊重优先级，避免同时打多个通道 |

**`parallel_first` 下顺序不保证生效**：全部并发发出，`route_order` 只影响发起先后与平手判定，
先排的不一定赢。要严格按优先级必须 `serial`。UI 必须把这个后果写出来，否则用户会以为顺序填了没生效。

### 5.3 超时三层继承

```
pairs[].probe_timeout_ms   （条目，0 = 未设）
  → block_timeouts_ms[kind] （区块，缺项 = 未设）
    → probe_timeout_ms       （全局，默认 2000，夹到 [200, 30000]）
```

同一区块内不同形态的超时需求差别很大（`100.x` 直连 1.5s、`*.ts.net` 走公网可能 5s），所以下放到条目。
`block_timeouts_ms` 用区块 id 作 key，与 `route_timeouts_ms`（通道 id，剩 `lan` / `jump`）
**分属两个 key 空间**，避免同一个表里混两种粒度的 key。

### 5.4 最坏耗时与硬顶

超时下放到条目后，最坏耗时是**乘出来的**：区块间串行 × 区块内串行 = Σ(Σ timeout)。
3 区块 × 4 条 × 3s = 36 秒，界面会像卡死。两条措施：

1. 编辑弹窗实时显示「最坏耗时 X 秒」，随策略与超时变化计算。
2. 整轮探测加 30s 硬顶，超时的剩余条目标「已跳过」，不静默等待。

### 5.5 「测试」按钮固定走全部探测

`parallel_first` 与「每条都显示状态」在一次探测里不可兼得：命中即收工，后面的条目只会是「未探测」。
所以两个动作分开：

| 触发 | 策略 |
|---|---|
| 后台轮询 / 打开页面 | 按档案配置的策略（快） |
| 用户点「测试」 | **固定 `parallel_all`**，逐条报 |

否则配了 3 条 cloudflare，永远只看到第 1 条的状态，用户会以为后面两条坏了。

## 六、UI（PC / 移动端一致）

一条档案一个平铺块，块内三个区块，每个区块内多条配对行：

```
● DESKTOP-8F3A · Windows 11                    [区块策略 ▾] [最坏 8.5s]
   cloudflare ────────────────────────────────────────────────
     a-1.trycloudflare.com          ✓ 42ms    超时 3s   [⋯]
     b-2.trycloudflare.com          – 离线    超时 3s   [⋯]
   ts ────────────────────────────────────────────────────────
     100.118.110.36:12300  ·直连    ✓ 8ms     超时 1.5s [⋯]
     desktop-8f3a.xxx.ts.net ·funnel – 未连接  超时 5s   [⋯]
   tsnet ──────────────────────────────────────────────────────
     anyboot-win.xxx.ts.net         ✓ 120ms   超时 3s   [⋯]
   LAN  192.168.1.10:12300          ✓ 3ms     （本次不改，维持现状）
```

- 区块头：区块名 + 该区块策略下拉 + 本块最坏耗时。
- 配对行：地址 + `lane` 徽标（仅 ts）+ 独立状态与耗时 + 该行超时 + 操作（备注 / 禁用 / 删除 / 上移下移）。
- 来源信息（`host · os`）显示在行尾，`mid` 与档案不一致时高亮提示，但**不禁用**。
- 平铺、不分组；档案标题兜底顺序：备注 → `host` → 首个地址。
- 电源 / 唤醒 / rEFInd / GRUB 是机器级，挂在档案标题行，不进任何区块。

## 七、实施阶段与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| S1 | `RemotePair` + 新字段 + **删除六个旧字段与 `route_disabled`** + `effective_remote_addr()` + 全部消费点改读 + 软上限 | `cargo check` 无残留引用（grep 六个旧字段名 = 0 命中）；单测：`effective_remote_addr` 按区块/块内顺序取第一条 enabled、32 条上限 |
| S2 | 探测遍历 `pairs` + `ProbeOutcome.pair_id` + 两层策略 + 条目级超时 + 30s 硬顶 | 单测：3 条同 kind 各自出结果且各用各的超时；`enabled=false` 只跳过它；块内三种策略各一例；硬顶触发后剩目标「已跳过」 |
| S3 | 配对串 v2（签发 + PC/移动端解析 + §4.6 归档 + 拒收老串） | 单测：v2 全参数；`v != 2` 拒收；`type` 非法拒收；同 (mid,kind) 更新；同 mid 不同 kind 新增 |
| S4 | PC 端三区块 UI（多条行 + 块内策略 + 条目超时 + 最坏耗时）；移动端字段对齐 + **删 `group` 与分组折叠 UI、列表改平铺** | 真机：一档案挂 3 条同区块配对，全部显示、各自探测、各自超时；两端列表形态一致、无分组标题 |
| S5 | 来源显示与 `tunnel_id` 重复提示 | 真机：行尾能看出对方 `host · os`；两条同 `tunnel_id` 时提示「同一隧道被多套系统共用，请求会随机落到其中一个」 |

S1 必须先做完（删字段是全仓编译级改动）；S2/S3 依赖 S1；S4 依赖 S2；S5 依赖 S1。

## 八、待确认

1. **配对行默认显示名**：`<host> · <kind>`（如 `DESKTOP-8F3A · cloudflare`）够不够，还是 `note` 必填。
2. **条目超时默认值**：按 `lane` 给不同默认（ts 直连 1.5s、ts funnel 5s、cloudflare / tsnet 3s），
   还是一律继承全局 2s 由用户改。
3. **区块策略下拉的位置**：放区块头右侧（贴合「每区块一个策略」），还是收进统一的「连接策略」折叠区。
