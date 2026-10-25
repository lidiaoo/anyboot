# 多配对与多系统：远程接入的隔离模型

> 状态：设计稿（未实现）
> 涉及：`remote_servers.rs` / `http_server.rs` / `hub_gateway.rs` / `hub_tsnet.rs` / `ui/hub.html` / `mobile/{android,ios}`
> 背景：一台物理机（固定 IP + MAC）装多个系统，每个系统各有一套 AnyBoot；每个系统上又可能同时开
> cloudflare / tailscale 直连 / funnel / tsnet 多条通道。当前架构在这两个维度上都只允许一条，新配的顶掉旧的。

## 一、问题：「顶替」是两个维度叠加的结果

### 1.1 数据结构上就没有「多」的位置

`RemoteServer`（`remote_servers.rs:155-178`）与移动端 `ServerProfile` 的远程通道是
**按类型固定命名的 String 槽位**：

```
remote_tunnel_addr   ← cloudflare
remote_ts_addr       ← tailscale 直连（100.64/10）
remote_funnel_addr   ← 系统 tailscale funnel（*.ts.net）
remote_tsnet_addr    ← 内置 tsnet（*.ts.net）
remote_token         ← 单值令牌
```

一个类型一个槽，同类型第二条配对**无处可放**，只能覆盖。

这个坑项目已经撞过两次，历史解法都是「再加一个槽」：

- `remote_ts_addr` 注释：以前兼收 `*.ts.net`，结果 Funnel 和 100.x 直连抢一个槽，**配一个顶掉另一个** → 拆槽
- `remote_tsnet_addr` 注释：和系统 Funnel 分开存，两者域名长得一样，**混在一个槽里必然互相顶替** → 再拆槽

拆槽能解决「两种通道类型撞车」，解决不了「同一类型有多条」。槽位是有穷枚举，而系统数量、通道数量都是变量
—— 再加到八个槽，遇到「两个 Windows 各配一条 cloudflare」照样塌。

### 1.2 配对码不带身份，只能落进「当前正在编辑的那条档案」

`hub_gateway.rs:1546` 签发：

```
anyboot://pair?gateway=<主机名>&tunnel=<url>&token=<token>&type=<kind>
```

移动端 `ServerProfile.parsePair`（1081-1126 行）只取 `tunnel`/`token`/`type`，
**`gateway` 被忽略，也没有 machineId**。所以三台机器的配对码进来，客户端无从判断「这是哪个系统」，
只能落进用户此刻正在编辑的那条档案的同一个槽 → **后配的顶掉先配的**。

这才是「顶替制」的根源：不是策略问题，是**配对串里缺一个归档依据**。

### 1.3 探活拿到了身份，但丢掉了

`/health` 早已返回 `machineId` / `machineOs` / `machineName`，但 `ui/hub.html:5347`
只回写 `machine_name` / `machine_os`，**从不比对 `machine_id` 是否等于本档案的**。
于是「对面这个系统是不是这条档案那个」这个事实被丢弃，探活成功就一律显示在线。

### 1.4 LAN 地址是机器级的，天然无法定位系统

`192.168.1.10:12300` 指向「当前正在运行的那个系统」，不是「某条档案」。
同一台机器的三条档案共用这个地址 → 探活全部成功 → 三个都显示在线。
这是物理事实，不是 bug（见 §2 非目标）。

### 1.5 展示层两端不一致

- PC 端「电脑」页 `cqRenderRows`（`ui/hub.html:6309`）平铺，无分组概念。
- 移动端 `ServerProfile` 已有 `group` / `groupColor` / `groupSortKey` / `groupDisplayName`。
- 移动端领先一代，字段语义未对齐。

## 二、目标与非目标

**目标**

1. **通道维度**：一个档案内每种接入类型可挂多条配对，全部保留、全部独立探测。
2. **系统维度**：同一台电脑的多个系统各成一条档案，配对时按对方自报的 `machine_id` 归档，互不覆盖。
3. 连接时能判断「对面是不是本条档案那个系统」，判断结果对用户可见。
4. PC / 移动端同一套字段契约、同一套列表与分组呈现。
5. **「顶替」只剩一个合法场景**：同一个系统、同一种通道重新配对 → 更新那一行（幂等，不是策略）。

**非目标（明确不做）**

- **不解决 LAN 通道的系统定位**。同一时刻只有一个系统在跑，`IP:12300` 永远指向它。
  这是物理事实，不靠软件绕。LAN 通道只做「校验 + 提示」，不做「路由到指定系统」。
- **不做 MAC 推导 / 机器指纹**。不猜「这两条档案是不是同一台机器」，把判定权交给
  `machine_id`（系统自报）与用户手填的分组。
- **不按「系统安装」区分共享 home 的多个发行版**。`mid` 的粒度是「一套 AnyBoot 配置」，
  共享 home 即同一个接入身份（§4.7）；要区分必须用 `ANYBOOT_CONFIG_DIR` 把配置分开（§4.9），
  只分 `mid` 不分 tsnet 状态会制造假的多身份（§4.8）。
- 不做跨系统的配置同步（各系统各存各的，不共享 tailnet/cloudflare 凭据）。
- 不引入新的中心服务 / 账号体系。

## 三、模型：两个维度的「多」

```
一台电脑（group 手填，纯展示）
├── 系统 A · machine_id=midA · host=DESKTOP-8F3A · os=windows
│   ├── pair#1  cloudflare   https://a-1.trycloudflare.com
│   ├── pair#2  ts           http://100.118.110.36:12300
│   └── pair#3  tsnet        https://desktop-8f3a.xxx.ts.net
├── 系统 B · machine_id=midB · host=arch-pc · os=linux
│   ├── pair#4  cloudflare   https://b-1.trycloudflare.com
│   └── pair#5  tsnet        https://anyboot-linux.xxx.ts.net
└── 系统 C · machine_id=midC · host=MacBook-Pro · os=macos
    └── pair#6  funnel       https://macbook-pro.xxx.ts.net
```

三层各司其职：

| 层 | 载体 | 唯一性 | 是否参与连接判定 |
|---|---|---|---|
| 电脑 | `group` 字符串（用户手填，可空） | **不唯一、不约束**，只是折叠标签 | 否 |
| 系统档案 | `RemoteServer.machine_id` | 每套 AnyBoot 配置唯一（共享 home 的多发行版共用一个，见 §4.7） | 是（探活比对） |
| 配对条目 | `RemotePair.id` + `(mid, kind)` | 条目 id 唯一 | 是（这就是连接目标） |

**关键规则**：`group` 不参与任何唯一性判断，不触发顶替。两个系统都叫「windows」只是两个同标签的行，
都能留着。顶替只按 `(machine_id, kind)` 发生。

### 3.1 为什么不用「配对后选分组，组内顶替」

这个方案（用户提出的备选 B）在「一台机器装 Win10 + Win11 + macOS 12/13/15 + Arch + Fedora」的场景下会丢数据：

| 配对来源 | 落到哪个组 | 结果 |
|---|---|---|
| Win10 + Win11 | 都选 `windows` | 只剩 Win11 |
| macOS 12 + 13 + 15 | 都选 `macos` | 只剩 15 |
| Arch + Fedora | 都选 `linux` | 只剩 Fedora |

按 OS 类型分组，粒度恰好等于顶替的力度 —— 六个系统最终只剩三个。
要救它只能把分组细到「每台机器的每个系统」，那组名就得手填六个（`DESKTOP-8F3A`
/ `DESKTOP-A11C` / `arch-pc` / `fedora-pc` …），比 `machine_id` 累得多，而 `machine_id`
是配对时对方自带的，零输入成本。

结论：**分组保留，但降级为纯展示折叠；归档依据用 `machine_id`。**

## 四、数据契约

### 4.1 PC 端：新增 `RemotePair`（`remote_servers.rs`）

```rust
/// 一条远程接入配对：一个系统的一套通道。独立保存、独立探测、独立删除。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RemotePair {
    /// 本地唯一 id（配对/手填时生成，永不复用）。探测结果与 route_order 都引用它
    pub id: String,
    /// 通道类型：cloudflare / ts / funnel / tsnet（LAN 与 jump 不进这里，见 §4.4）
    pub kind: String,
    /// 生效 origin：https://xxx.trycloudflare.com 或 http://100.x.x.x:12300
    pub addr: String,
    /// 该条目的网关客户端令牌（对方签发；空 = 该通道不需要）
    pub token: String,
    /// 对方系统自报的 machine_id —— 幂等键的一半；老档案迁移进来时为空
    #[serde(default)]
    pub mid: String,
    /// 对方自报的系统族（windows / macos / linux）
    #[serde(default)]
    pub os: String,
    /// 对方自报的主机名（列表显示用）
    #[serde(default)]
    pub host: String,
    /// named tunnel 的 id：识别「同一个隧道被多个系统共用」
    #[serde(default)]
    pub tunnel_id: String,
    /// 用户备注（可改，纯显示）
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub created_at: u64,
}
```

`RemoteServer` 新增：

```rust
/// 远程接入配对列表（有序 = 探测优先级）。空 = 老档案，读取时从旧槽位迁移
#[serde(default)]
pub pairs: Vec<RemotePair>,
```

保留不变（只读兼容）：`remote_addr`（由 pairs 计算出的「生效地址」镜像）、
`remote_tunnel_addr` / `remote_ts_addr` / `remote_funnel_addr` / `remote_tsnet_addr` / `remote_token`
（迁移后不再写入，仅用于读老档案）。

**软上限**：每档案 32 条（防误操作刷屏；超了 UI 拒绝新增并提示）。

### 4.2 移动端 `ServerProfile`

新增等价的 JSON 数组字段 `pairs`，元素字段名与 PC 端完全一致（`id` / `kind` / `addr` /
`token` / `mid` / `os` / `host` / `tunnel_id` / `note`）。

迁移时 `Task.resume()` 的逐字段拷贝**必须同步带上 `pairs`**（历史坑：`Task` 加字段不同步会导致状态丢失）。

### 4.3 `/health`：只加一个显示用字段

已有 `machineId` / `machineOs` / `machineName`，够做归档与校验。
新增 `systemLabel`（**纯显示，不参与任何判定**）：

```json
{
  "ok": true,
  "service": "anyboot",
  "machineId": "<一套配置的 id>",
  "machineName": "<主机名>",
  "machineOs": "windows|macos|linux",
  "systemLabel": "Fedora Linux 41 | Ubuntu 24.04 | Windows 11 24H2 | macOS 15.2"
}
```

取值：Linux 读 `/etc/os-release` 的 `PRETTY_NAME`；Windows 读
`HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` 的 `ProductName` + `DisplayVersion`；
macOS 读 `sw_vers -productName` + `-productVersion`。

**为什么必须有**：共享 home 的多发行版共用一个 `mid`，`machineOs` 都是 `linux`，
没有 `systemLabel` 用户就看不出当前连的是 Arch 还是 Fedora。这是共享 home 方案的必备配套，
不是可选装饰。

**不引入 `machineKey` / `machineMacs`**（上一版设计的 MAC 推导方案已废弃）。
老客户端读新字段会忽略，无影响。

### 4.4 什么不进 `RemotePair`

| 项 | 为什么 |
|---|---|
| LAN（`addresses`） | 机器级地址，不属于任何系统；同一时刻只有正在运行的系统响应 |
| 跳板机（`jump_id`） | 档案级的转发路径，不是「某系统的一条接入」 |
| `group` | 放在档案级（这台电脑归哪堆），不是配对的属性 |

### 4.5 配对串 v2

```
anyboot://pair?v=2&mid=<machineId>&os=<os>&host=<hostname>&type=<kind>&tunnel=<url>&token=<token>&tunnel_id=<id>
```

- `v=2` 显式版本；老客户端只认已知参数，**天然兼容**。
- 新增 `mid` / `os` / `host` / `tunnel_id`；`gateway` 保留（显示用，不再当归档依据）。
- `type` 取值限定 `cloudflare|ts|funnel|tsnet`，非法值拒收并提示。

### 4.6 归档规则（客户端收到配对串后）

```
1. 按 mid 在本机全部档案里找 machine_id == mid 的那条
2. 找不到 → 新建档案：name = host, machine_os = os, machine_id = mid
   ↳ 此时问用户「归到哪个分组」：列已有 group + 「新建分组」+「暂不分组」
3. 在目标档案内按 (mid, kind) 找 pair
   ↳ 找到 → 更新 addr / token / tunnel_id（这就是唯一合法的「顶替」）
   ↳ 没找到 → 追加新 pair
4. mid 为空（老服务端签的码）→ 退回旧行为：落到当前编辑的档案，
   UI 提示「这张码不含系统标识，可能落到别的系统上，建议重新生成」
```

注意第 3 步的键是 `(mid, kind)` 而不是 `mid`：**同一个系统可以同时有 cloudflare + tsnet + funnel
三条配对**，只用 `mid` 去重会让它们互相顶替，等于回到原点。

### 4.7 身份语义：一套配置 = 一个身份

`machine_id` 存在 `settings.json` 里（`lib.rs:1351`），所以它的语义天然是
「**一套 AnyBoot 配置的身份**」，不是「一次操作系统安装的身份」。这个定义必须显式写下来，
因为它决定了共享 home 场景下的行为：

| 部署形态 | `settings.json` | `mid` | tsnet 节点 | 结果 |
|---|---|---|---|---|
| Win + Linux + macOS（各自 home） | 三份 | 三个 | 三个 | 三条档案、三个域名、可同时在线 |
| 双 Windows（各自系统盘） | 两份 | 两个 | 两个 | 两条档案 |
| **多发行版共享 home** | **一份** | **一个** | **一个** | **一条档案、一个域名**，永远连当前在跑的系统 |
| 两台机器 | 两份 | 两个 | 两个 | 两条档案 |

共享 home 合并成一个身份与 LAN 地址同构：同一时刻只有一个系统在运行，所以
「一个地址永远指向当前系统」是可接受的，而且是优点 —— 用户只需记一个域名，
不用管这次启动的是 Arch 还是 Fedora。

**代价**：共用 `mid` 后 `machineOs` 都是 `linux`，看不出当前是哪个发行版。
所以 `/health` 必须带 `systemLabel`（见 §4.3），列表上显示「当前运行：Fedora 41」。

### 4.8 铁律：`mid` 与 tsnet 状态必须同源

`machine_id` 来自 `app_config_dir()`（`lib.rs:1351`），tsnet 状态来自
`app_data_dir()/tsnet/state`（`hub_tsnet.rs:461`）。二者都随配置目录走，所以**天然同源**。
绝不能只把其中一个改成「系统级 id」：

```
用 /etc/machine-id 当 mid  →  Arch 与 Fedora 得到两个 mid
但 tsnet state 仍共享     →  两个 mid 指向同一个 tailnet 节点
结果：手机端两条档案、同一个域名、都显示在线，点哪条走过的都是同一条路
```

这比顶替更糟 —— 它制造了一个**假的多身份**。`mid` 与节点身份必须一起分或一起合。

### 4.9 真要在共享 home 下区分发行版

必须两样一起分开：给每个发行版设不同的 `ANYBOOT_CONFIG_DIR`（`lib.rs:1337` 已有的逃生门），
于是各自有独立的 `settings.json` + tsnet 状态 + `mid` + 端口。代价是配置不再共享，
每个发行版要各自配一遍远程接入。

### 4.10 同时运行的冲突（虚拟机 / 容器）

共享 home 的两个系统同时跑会撞两处：`http_port`（12300）与 tsnet 节点（同节点两连接互踢）。
启动时 bind 失败必须给明确文案「另有一套 AnyBoot 在运行，或是共享 home 的另一个系统」，
而不是笼统的「端口占用」—— 否则用户会去查防火墙。

## 五、探测语义

### 5.1 全量探测

`route_order` 从「通道类型枚举」演进为「pair id 有序列表」：

```rust
/// 探测/连接优先级（pair id 有序）。空 = 按 pairs 数组顺序
pub route_order: Vec<String>,
/// 禁用的 pair id（在位次里但跳过探测）
pub route_disabled: Vec<String>,
```

- 每次探活遍历 `pairs` 全集（跳过 `route_disabled`），**每个 pair 独立上报**。
- `ProbeOutcome` 增加 `pair_id: String`（空 = 老结果行，按 `kind` 兼容）。
- 探测策略沿用现有三层：`route_probe_mode`（pair 之间）/ `probe_mode`（同一 pair 多地址之间）/
  `route_timeouts_ms`（按 kind 设超时，key 仍是 kind，因为超时特性跟通道类型走而不是跟条目走）。

### 5.2 系统校验

LAN 探活成功后，用 `/health` 的 `machineId` 与本档案比对：

| 情况 | 判定 | UI | 行为 |
|---|---|---|---|
| `machineId` 相等 | 命中本系统 | 正常 | 正常连接 |
| `machineId` 不等 | 同机器的**另一个系统**在跑 | 该行标「当前运行的是 <os>」并灰化 | 禁止 LAN 直连；远程 pair 照常试（远程地址本来就按系统分） |
| `machineId` 空（老服务端） | 无法判定 | 不标注 | 维持现状 |

**这不是探测失败**：探测成功，只是对面不是本条档案那个系统。所以不能复用 `ok:false`，
要单列 `foreign_system: bool` + `foreign_os: String`，否则用户会以为网络坏了。

## 六、列表呈现（PC / 移动端一致）

平铺为主，分组可选折叠：

```
▾ 书房台式机                        [在线 2/3]
    ● Windows 11 · DESKTOP-8F3A
        cloudflare  a-1.trycloudflare.com     ✓ 42ms
        ts          100.118.110.36:12300      ✓ 8ms
        tsnet       desktop-8f3a.xxx.ts.net   – 未连接
    ○ Arch Linux · arch-pc  （当前运行的是 Windows 11）
        cloudflare  b-1.trycloudflare.com     – 离线
    ○ macOS 15 · MacBook-Pro
        funnel      macbook-pro.xxx.ts.net    – 离线
```

- 电脑行（`group`）：显示名可改，含在线计数。**电源 / 唤醒 / rEFInd / GRUB 挂这里**
  （本来就是机器级：一台机器只有一个 rEFInd）。
- 系统行：os 徽标 + 主机名；「当前在跑」加亮点，其余标离线原因。
- 配对行：kind 徽标 + 地址 + 独立探测状态 + 备注；行内可改备注 / 删除 / 调整优先级。
- 未分组档案进「未分组」平铺（旧行为）。
- 移动端已有 `groupColor` / `groupSortKey` / `groupDisplayName`，改为按 `group` 折叠即可复用；
  PC 端按同一套排序 / 配色实现。

## 七、通道侧命名（辅助隔离）

| 通道 | 改动 |
|---|---|
| 内置 tsnet | `--hostname`（`hub_tsnet.rs:475`，现写死 `anyboot`）改为取**配置项** `tsnet_hostname`，默认 `anyboot-<os>`（`anyboot-win` / `anyboot-linux` / `anyboot-mac`）。**绝不能取系统 hostname**：共享 home 的 Arch(`arch-pc`) 与 Fedora(`fedora`) 会注册成两个节点、两个域名，正好破坏「一个固定域名」。取配置则共享 home 域名固定、独立 home 三系统自动区分 |
| 系统 Tailscale | 不动（hostname 由系统自己决定）；远程接入页显示当前节点名，便于对账 |
| Cloudflare named | 无法自动隔离：**一系统一 tunnel**（同 token 必然互抢 connector）。UI 显示 `tunnel_id`，两条 pair 同 `tunnel_id` 时提示「同一个隧道被多套系统抢占，请求会随机落到其中一个」 |
| Cloudflare quick | 天然隔离（每次随机域名），地址会变，保持现状 |

## 八、兼容与迁移

- **老档案（四个旧槽位）**：读取时迁移成 `pairs`。规则：非空的槽位各生成一条
  `RemotePair { id: "legacy-<kind>", kind, addr, token: remote_token, mid: "" }`，
  顺序按老 `route_order`，老 `route_order`/`route_disabled` 里的 kind 字符串映射到对应 pair id。
- **老 `remote_addr`**：迁移后置空，由 `pairs` 计算（隧道类优先，其余按 route_order）。
  消费点改为读计算值，不再直接读字段。
- **老配对串（无 `mid`）**：落槽行为完全不变，UI 提示建议重新生成。
- **老服务端（`/health` 无字段）**：`machine_id` 留空 → 不做系统校验，不报错。
- 所有新字段 `#[serde(default)]`，读写老档案不丢数据；**旧字段只读不写**，确认无回滚需求后再删。

## 九、实施阶段与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| S1 | `RemotePair` 结构 + 老槽位迁移 + 归一化/裁剪（软上限） | 单测：四槽迁移顺序、空槽不生成、老 `route_order` 映射、32 条上限 |
| S2 | 探测改遍历 `pairs` + `ProbeOutcome.pair_id` + 按 kind 取超时 | 单测：3 条同 kind 各自出结果；禁用某条只跳过它；老结果行兼容 |
| S3 | 配对串 v2（签发 + PC/移动端解析 + §4.6 归档规则） | 单测：v2 全参数、同 (mid,kind) 更新、同 mid 不同 kind 新增、缺 mid 老路 |
| S4 | 探活 `machineId` 校验 + `foreign_system` 标记 | 单测 + 真机：同 IP 两条不同 mid 的档案，只有匹配的能走 LAN |
| S5 | PC 端列表（分组折叠 + 配对行）与移动端对齐 | 真机：一档案挂 3 条配对全部显示并各自探测；两台机器归两组 |
| S6 | `systemLabel`（三端取值 + `/health` + 列表显示）+ tsnet `--hostname` 取配置 + `tunnel_id` 展示与重复提示 + 端口占用文案 | 真机：共享 home 的 Arch/Fedora 显示「当前运行：Fedora 41」且域名不变；tailnet 里看到 `anyboot-linux`；同 tunnel 两条时提示出现 |

S1/S2 可并行；S3 依赖 S1（要建档案就要有 pairs）；S4 依赖 S2；S5 依赖 S1+S2；S6 独立。

## 十、待确认

1. **配对条目的默认显示名**：`<host> · <kind>`（如 `DESKTOP-8F3A · cloudflare`）够不够，
   还是要 `note` 必填。
2. **分组是否要预置空组**：建议**不预置**。预置 `windows/macos/linux` 会诱导用户按 OS 分组，
   而按 OS 分组正是 §3.1 里会丢系统的那个粒度。分组只在配对新建档案时问一次，用户填什么就是什么。
3. **异地/多网卡场景下 `group` 与 `jump_id` 的关系**：目前分组只管显示，跳板机只管转发，
   两者互不引用 —— 确认这个解耦是否可以。
4. **`systemLabel` 显示在哪**：建议挂在档案名后缀（`arch-pc · Fedora 41`），
   与「当前运行」标记分两处，避免同一个位置又要表示身份又要表示实时状态。
