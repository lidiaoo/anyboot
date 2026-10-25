# 一台电脑多系统：身份分层与远程接入隔离

> 状态：设计稿（未实现）
> 涉及：`remote_servers.rs` / `http_server.rs` / `hub_gateway.rs` / `hub_tsnet.rs` / `ui/hub.html` / `mobile/{android,ios}`
> 背景：一台物理机（固定 IP + MAC）装多个系统，每个系统各有一套 AnyBoot。当前远程接入是「顶替制」。

## 一、问题：顶替发生在四层

模型缺一层。现在 `RemoteServer` 同时扮演两个角色：「一台电脑」和「一个系统」，而物理事实是
**一台电脑 → N 个系统实例**。四个位置各自顶替，互相叠加：

### 1. 连接层：LAN 地址是机器级，不区分系统

`192.168.1.10:12300` 指向「当前正在运行的那个系统」，不是「某条档案」。
三条代表不同系统的档案共用这个地址 → 探活全部成功 → 三个都显示在线 → 点哪行都连到碰巧在跑的那个。

更关键的是**探活结果没被校验**：`/health` 已经返回 `machineId`/`machineOs`，但
`ui/hub.html:5347` 只回写 `machine_name`/`machine_os`，从不比对 `machine_id` 是否等于本档案的。
于是「这条档案是不是当前系统」这个事实被丢掉了。

### 2. 配对层：配对码不带身份

`hub_gateway.rs:1546`：

```
anyboot://pair?gateway=<主机名>&tunnel=<url>&token=<token>&type=<kind>
```

移动端 `ServerProfile.parsePair`（1081-1126 行）只取 `tunnel`/`token`/`type`，
**`gateway` 被完全忽略，也没有 machineId**。三个系统各签一张码，扫进来说到底只是「一个地址」，
只能落到用户此刻正在编辑的那条档案的同一个槽 → 后配的顶掉先配的。

### 3. tailnet 层：节点自己没区分

- `hub_tsnet.rs:475` 内置引擎 `--hostname anyboot` 是**写死**的 → 三系统在 tailnet 里同名，
  Tailscale 自动变 `anyboot` / `anyboot-1` / `anyboot-2`，看不出是哪个系统。
- Cloudflare named 模式更直接：同一个 Tunnel Token 在三个系统上跑 = **一个 tunnel 三个 connector**，
  Cloudflare 把请求随机分给其中一个 —— 想连 Windows，结果进了 macOS。

### 4. 展示层：两端不一致

- PC 端「电脑」页 `cqRenderRows`（`ui/hub.html:6309`）是平铺列表，没有分组概念。
- 移动端 `ServerProfile` 反而已有 `group` + `groupColor` + `groupSortKey` + `groupDisplayName`。
- 移动端比 PC 端领先一代，字段语义也不完全对齐。

## 二、目标与非目标

**目标**

1. 一台物理机的多个系统在数据模型里显式分层，档案不再互相覆盖。
2. 配对码携带身份，客户端按身份归档，不再「落进当前正在编辑的那条」。
3. 连接时能判断「对面是不是本条档案那个系统」，判断结果对用户可见。
4. PC / 移动端同一套字段契约、同一套分组呈现。

**非目标（明确不做）**

- **不解决 LAN 通道的系统定位**。同一时刻只有一个系统在跑，`IP:12300` 永远指向它。
  这是物理事实，不靠软件绕。LAN 通道只做「校验 + 提示」，不做「路由到指定系统」。
- 不做跨系统的配置同步（各系统各存各的，不共享 tailnet/cloudflare 凭据）。
- 不引入新的中心服务/账号体系。

## 三、身份模型

两级，key 与显示名分离（key 稳定，显示名可改）：

| 级别 | key | 来源 | 稳定性 | 用途 |
|---|---|---|---|---|
| 机器 | `machine_key` = `mac:aa:bb:cc:dd:ee:ff` | 物理网卡 MAC 推导 | 装机级，跨系统一致 | 分组、电源/唤醒、rEFInd/GRUB |
| 系统实例 | `machine_id`（现有字段） | 首次运行随机生成，存 `settings.json` | 每系统唯一，重装即变 | 连接、配对归档、四通道归属 |

关系：**一个 `machine_key` 下挂多个 `machine_id`**。档案仍然是 `machine_id` 粒度，机器只是分组容器。

`group` 字段保留，语义从「用户手填的分组名」**降级为机器显示名**
（默认由 `machine_key` 推导，用户可改名）。分组键一律用 `machine_key`，绝不用 `group` 字符串
——字符串会被用户改乱，key 不会。

## 四、`machine_key` 的推导规则（三端一致）

这是整套方案唯一有技术风险的部分，规则必须确定、可复现、失败可观测。

### 4.1 只认物理网卡

| 平台 | 取法 | 天然排除虚拟网卡 |
|---|---|---|
| Linux | `/sys/class/net/*/address`，要求 `/sys/class/net/<if>/device` 存在 | veth / docker0 / br- / virbr / tailscale0 都没有 `device` 目录 |
| Windows | `Get-NetAdapter -Physical`（PowerShell，必须 `CREATE_NO_WINDOW`） | `-Physical` 直接过滤 Hyper-V / WSL / VPN 虚拟适配器 |
| macOS | `networksetup -listallhardwareports` | 输出的是硬件端口（Wi-Fi / Ethernet / Thunderbolt Bridge），虚拟接口不在其中 |

三端都必须额外排除：回环、隧道类（`utun`/`tun`/`tap`/`tailscale*`/`wg*`）、
MAC 全零（`00:00:00:00:00:00`）、**本地管理位为 1 的 MAC**。

最后一条有实测依据（本机 macOS 2026-10-26）：`ifconfig` 全扫会看到

```
en0     ether 58:11:22:bc:78:66     ← 真实有线
en1     ether f0:d4:15:57:9c:c5     ← 真实无线
awdl0   ether 5e:d3:b7:bf:7d:67     ← AirDrop 派生接口，首字节 0x5e 的 bit1 = 1（本地管理位）
utun0-4 （无 ether）
gif0 / stf0 （无 ether）
```

`awdl0` 这类派生地址每个系统各不相同、且会重生成，一旦被当成机器 key 就永远归不了组。
判据：首字节 `& 0x02 != 0` → 本地管理位 → 排除（同时天然排除掉随机化的 Wi-Fi 地址）。

注意**不能拿 `ifconfig` 全扫本地 MAC 集合**：`networksetup -listallhardwareports` 只列真实硬件端口
（实测只给 `Ethernet / en0` 与 `Wi-Fi / en1`），是 macOS 上更省事的入口。

### 4.2 选「主 MAC」的确定性顺序

1. **有默认路由的物理网卡**（三端都能取：Linux `ip route get 1.1.1.1`；Windows `Get-NetRoute -DestinationPrefix 0.0.0.0/0`；macOS `route -n get default`）
2. 否则：所有物理**有线**网卡按接口名升序取第一个
3. 否则：物理**无线**网卡，且必须读 **burned-in MAC**，不能读当前地址（见 4.3）
4. 都没有 → `machine_key` 为空串

第 4 步是**明确失败**，不是兜底：空 key 表示「这台机器无法自动归组」，
UI 直接让用户手动指定机器分组，不猜。

### 4.3 无线网卡必须读出厂地址

三端默认的「当前 MAC」可能是随机化地址：

- macOS「私有 Wi-Fi 地址」（Sequoia 起默认开）
- Windows 11「随机硬件地址」
- Linux NetworkManager `wifi.cloned-mac-address=random`

随机地址按 SSID 变化，拿它当机器 key 会导致同一台笔记本在不同网络下变成不同的机器。
读出厂地址的方式：

- macOS：`ioreg -l | grep IOMACAddress` —— 实测能读到全部出厂地址（本机三个：
  `f0d415579cc5` Wi-Fi、`5ed3b7bf7d67` awdl0、`581122bc7866` Ethernet），
  但**输出不含接口名**。做法：把 ioreg 的地址集合当「出厂地址白名单」，
  `networksetup` 给的 Wi-Fi 值若不在白名单里 → 说明是随机化地址，
  此时取 ioreg 里剩下的那个非本地管理位地址。
- Linux：`iw dev <if> info` 的 `addr`（需 `iw`）或 `ethtool -P <if>` 的 `Permanent address`
- Windows：`Get-NetAdapter -Physical` 的 `PermanentAddress` 字段

读不到时**不要退回当前 MAC**，直接跳过这块网卡（宁可交给用户手动分组，也不要给出会漂移的 key）。

### 4.4 已知限制（必须写进 UI 提示）

- 笔记本只用 Wi-Fi 且系统把出厂地址也隐藏（少数机型）→ 推导不出，落手动分组。
- 三系统看不到同一块网卡（比如某系统禁用了有线网卡）→ key 不同，用户手动归组。
- 换主板 / 换网卡 → key 变化，等于新机器。这与「换机器」语义一致，无需特殊处理。

### 4.5 同一台机器的判定

**只比 `machine_key` 是否相等**，不做「MAC 集合求交集」这类模糊匹配。
单一真相：两个档案同组 ⟺ `machine_key` 相同且非空。空 key 的档案永不自动归组。

## 五、数据契约

### 5.1 PC 端 `RemoteServer`（`remote_servers.rs`）

新增：

```rust
/// 机器标识 `mac:aa:bb:cc:dd:ee:ff`（由 /health 回填；空 = 无法推导，不参与自动分组）
#[serde(default)]
pub machine_key: String,
/// 该机器的全部物理 MAC（小写冒号分隔，升序）。电源/唤醒可复用，
/// 也是排查「为什么归不到一台」的证据。
#[serde(default)]
pub machine_macs: Vec<String>,
```

保留不变：`machine_id`（系统实例 id）、`machine_os`、`machine_name`。
`group` 改为机器显示名，不再是分组键。

### 5.2 移动端 `ServerProfile`

新增 `machineKey`（`String`）、`machineMacs`（`List<String>`），JSON key 与 PC 端一致
（`machine_key` / `machine_macs`）。`group` 语义同步调整。
`sameMachine()` 改为：`machineKey` 非空且相等 → 同机器；否则退回 `group` 相等（兼容老档案）。

### 5.3 `/health` 响应（`http_server.rs:78`）

```json
{
  "ok": true,
  "service": "anyboot",
  "machineId": "<系统实例 id>",
  "machineName": "<主机名>",
  "machineOs": "windows|macos|linux",
  "machineKey": "mac:aa:bb:cc:dd:ee:ff",
  "machineMacs": ["aa:bb:cc:dd:ee:ff", "..."]
}
```

老客户端读新字段：忽略，无影响。新客户端读老服务端：`machineKey` 缺省为空 → 不自动分组。

### 5.4 配对串 v2

```
anyboot://pair?v=2&mid=<machineId>&os=<os>&host=<hostname>&tunnel=<url>&token=<token>&type=<kind>&tunnel_id=<id>
```

- `v=2` 显式版本；老客户端只认已知参数，**天然兼容**。
- `mid` 是归档依据：客户端扫到后
  - 已有档案 `machine_id == mid` → 只更新它的对应槽（`type` 决定哪个槽）
  - 没有 → 新建一条档案：`name = host`、`machine_os = os`、`machine_id = mid`，
    机器分组按 `machineKey`（拿到 `/health` 后再回填）
- `tunnel_id` 用于识别「同一个 named tunnel 被多个系统共用」：档案上显示来源，
  两个档案 `tunnel_id` 相同 → UI 提示「同一个隧道被两套系统抢占，请求会随机落到其中一个」。

## 六、连接与探活语义

`/health` 返回的 `machineId` 与本档案比对，三种结果：

| 情况 | 判定 | UI | 行为 |
|---|---|---|---|
| `machineId` 相等 | 命中本系统 | 正常 | 正常连接 |
| `machineId` 不等 | 是**同机器的另一个系统**在跑 | 该行标「当前运行的是 <os>」并灰化 | 禁止该行走 LAN 直连；远程通道照常试（远程地址本来就按系统分） |
| `machineId` 空（老服务端） | 无法判定 | 不标注 | 维持现状 |

注意：这里**不是**「探测失败」。探测是成功的，只是对面不是本条档案那个系统。
所以不能复用 `ok:false`，要单列一个标记（建议 `foreign_system: bool` + `foreign_os: String`），
否则用户会以为网络有问题。

## 七、列表呈现（PC / 移动端一致）

按 `machine_key` 分组折叠，组内按 `machine_os` 排序：

```
▾ 书房台式机  mac:…3f:2a            [在线 2/3]
    ● Windows 11 · DESKTOP-8K2      lan ✓  tunnel ✓  ts ✓  funnel –
    ○ Ubuntu 24.04 · anyboot-linux  lan –  tunnel ✓  ts ✓  funnel –
    ○ macOS 15 · mac-pro            lan –  tunnel –  ts –  funnel –
```

- 机器行：显示名（可改）+ key 短码 + 在线系统计数。电源/唤醒/rEFInd/GRUB 挂这里
  （它们本来就是机器级的：一台机器只有一个 rEFInd）。
- 系统行：os 徽标 + 主机名 + 四通道状态；「当前在跑」的系统加亮点，其余显示「离线」。
- 未识别 key 的档案进「未分组」组，保持平铺（旧行为）。
- 移动端已有 `groupColor`/`groupSortKey`/`groupDisplayName`，改为以 `machine_key` 为分组键即可复用；
  PC 端按同一套排序/配色规则实现。

## 八、通道侧命名

| 通道 | 改动 |
|---|---|
| 内置 tsnet | `--hostname` 从写死 `anyboot` 改为 `anyboot-<os>`（`anyboot-win` / `anyboot-linux` / `anyboot-mac`），设置项 `tsnet_hostname` 可覆盖 |
| 系统 Tailscale | 不动（hostname 由系统自己决定）；远程接入页显示当前节点名，便于对账 |
| Cloudflare named | 无法自动隔离：**一台机器多系统必须各建一个 tunnel**（同 token 必然互抢 connector）。UI 显示 tunnel id，配对串带 `tunnel_id`，重复时提示 |
| Cloudflare quick | 天然隔离（每次随机域名），但地址会变，保持现状 |

## 九、兼容与迁移

- **老档案**（无 `machine_key`）：首次探活成功时用 `/health` 的 `machineKey` 回填；回填后自动归组。
- **老配对串**（无 `mid`）：落槽逻辑完全不变（当前行为）。UI 上加一句提示
  「这条配对码不含系统标识，可能落到别的系统上，建议重新生成」。
- **老服务端**（`/health` 无 `machineKey`）：档案不进自动分组，不报错。
- 字段全部 `#[serde(default)]`，读写老档案不丢数据。

## 十、实施阶段与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| S1 | 三端 `machine_key` 推导 + `/health` 新字段 + 档案字段与归一化 | 单测：虚拟网卡排除、无线读出厂地址、主 MAC 顺序、空 key 语义；本机实测三种网络形态（有线/无线/仅虚拟） |
| S2 | 配对串 v2（签发 + PC/移动端解析 + 按 mid 归档） | 单测：v2 全参数、缺 `mid` 的老串、`type` 非法；两端解析结果一致 |
| S3 | 探活 `machineId` 校验 + `foreign_system` 标记 | 单测 + 模拟：同 IP 两条不同 `machine_id` 的档案，只有匹配的那条可连 LAN |
| S4 | PC 端分组折叠 UI + 移动端改分组键 | 真机：一台机器的两条档案（不同 `machine_id`）归到一个组下；移动端同样归组 |
| S5 | tsnet hostname 带系统后缀 + tunnel id 展示与重复提示 | 真机：tailnet 里看到 `anyboot-mac`；两个档案同 `tunnel_id` 时提示出现 |

S1/S2 可以并行；S3 依赖 S2；S4 依赖 S1；S5 独立。

## 十一、待确认

1. **机器显示名默认格式**：建议 `<主机名> (<MAC 后 6 位>)`，还是 `<厂商型号>`（要额外读硬件信息，跨平台成本高）。
2. **`machine_macs` 是否与现有 `macs`（地址→MAC 映射，电源包用）合并**：建议保持两份，
   一份是「机器身份证据」，一份是「某地址该发 WOL 到哪个 MAC」，语义不同，混用会误发。
3. **Wi-Fi 出厂地址读不到时**：建议跳过该网卡（宁可手动分组），而不是退回可能漂移的当前 MAC
   —— 但这条会让部分纯无线笔记本永远落「未分组」，需要确认可接受。
