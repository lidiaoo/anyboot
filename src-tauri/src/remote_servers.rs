//! 远程管理 · 「电脑」页：AnyBoot 服务端档案。
//!
//! 一条记录 = **一台 AnyBoot 电脑**（不是一个 IP）：多网卡候选地址列表 + token，
//! 语义对齐手机端 `ServerProfile`/`ServerStore`/`HealthProbe`：
//! - 探测协议：`GET http://addr/health`（免鉴权），响应 `{ok,service,machineId,machineName}`；
//! - 同一台电脑的多个地址靠 `machineId` 归并（本轮先存不消费，字段对齐）；
//! - `connect_all` = 手机端「启动时连接全部服务端」的全局开关。
//!
//! 与 hub（SSH 服务器）完全解耦：配置目录下自己的 JSON（`remote-servers.json`），
//! 命令只服务 hub 窗口的「电脑」页，不碰 UiState。

use crate::hub::{normalize_mac, normalize_route_target, PowerRoute};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// 档案文件名（放配置目录，随 `ANYBOOT_CONFIG_DIR` 走）
pub(crate) const REMOTE_SERVERS_FILE: &str = "remote-servers.json";

/// 电脑端「远程控制」的默认端口（地址不带端口时探活兜底，对齐手机 DEFAULT_PORT）
pub(crate) const DEFAULT_PORT: u16 = 12300;

/// 单次探活的连接+响应超时
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// 连接优先级的六个通道 id（route_order / route_disabled 里只认这些）。
/// **与手机端 ServerProfile.ROUTE_IDS / hub.html ROUTE_IDS 必须一致**：两端是同一套档案
/// 的不同样式，通道表不一样就会出现「电脑端能选、手机端看不见」这种对不上的情况。
pub(crate) const ROUTE_IDS: [&str; 6] = ["lan", "jump", "tunnel", "ts", "funnel", "tsnet"];

/// 默认优先级：直连 → 跳板 → 隧道 → Tailscale 直连 → 系统 Funnel → 内置 tsnet
pub(crate) fn default_route_order() -> Vec<String> {
    ROUTE_IDS.iter().map(|id| id.to_string()).collect()
}

/// 通道显示名（错误聚合 / 探测行用）
pub(crate) fn route_label(kind: &str) -> &'static str {
    match kind {
        "lan" => "直连",
        "jump" => "跳板机",
        "tunnel" => "Cloudflare 隧道",
        "ts" => "Tailscale 直连",
        "funnel" => "Tailscale Funnel",
        "tsnet" => "Tailscale Funnel（内建）",
        _ => "未知通道",
    }
}

/// Funnel 判型：https + 域名含 .ts.net（与 Tailscale 100.x 直连区分）
fn is_funnel_origin(origin: &str) -> bool {
    let lower = origin.trim().to_ascii_lowercase();
    lower.starts_with("https://") && lower["https://".len()..].contains(".ts.net")
}

/// 三个配对区块（`RemotePair.kind` 的取值）。区块是探测策略与超时的分组单位，
/// 与 `route_order` 里的通道 id（lan/jump/tunnel/ts/funnel/tsnet）是**两套 id**：
/// 区块是存储层，通道 id 是「谁在跑」的上报层，两者靠 [`block_of_route`] /
/// [`route_kind_of`] 互转。别把两套混成一个 —— `ts` 区块同时含 direct 与 funnel
/// 两种 lane，它们必须是同一个区块、却是不同的通道 id。
pub(crate) const BLOCK_IDS: [&str; 3] = ["cloudflare", "ts", "tsnet"];

/// 通道 id → 区块 id（lan / jump 不属于任何配对区块，返回 None）
pub(crate) fn block_of_route(kind: &str) -> Option<&'static str> {
    match kind.trim() {
        "tunnel" => Some("cloudflare"),
        "ts" | "funnel" => Some("ts"),
        "tsnet" => Some("tsnet"),
        _ => None,
    }
}

/// `RemotePair.kind` 的显示名（区块标题用）
pub(crate) fn block_label(kind: &str) -> &'static str {
    match kind.trim() {
        "cloudflare" => "Cloudflare",
        "ts" => "Tailscale",
        "tsnet" => "Tailscale（内建）",
        _ => "未知区块",
    }
}

/// 一条远程接入配对：一个系统的一套通道。独立保存、独立探测、独立删除。
///
/// 取代原来的四个单值槽（`remote_tunnel_addr` / `remote_ts_addr` /
/// `remote_funnel_addr` / `remote_tsnet_addr`）+ 单值 `remote_token`：
/// 槽是有穷枚举，而「同一台机器的几个系统各配一条」是变量，加槽加到几个都会塌。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RemotePair {
    /// 本地唯一 id（配对/手填时生成，永不复用）。探测结果按它回填
    #[serde(default)]
    pub id: String,
    /// 所属区块：cloudflare / ts / tsnet
    #[serde(default)]
    pub kind: String,
    /// 仅 kind=ts 有意义：direct（100.64/10 明文 http）/ funnel（*.ts.net https）。
    /// 保存时由地址形态推导，不靠用户填
    #[serde(default)]
    pub lane: String,
    /// 生效 origin：`https://xxx.trycloudflare.com` 或 `http://100.x.x.x:12300`
    #[serde(default)]
    pub addr: String,
    /// 该条目的网关客户端令牌（对方签发；空 = 该通道不需要）
    #[serde(default)]
    pub token: String,
    /// 本条目的探测超时（毫秒）。0 = 继承 `block_timeouts_ms[kind]`，再继承全局
    /// `probe_timeout_ms` —— 三层继承见文档 §5.3
    #[serde(default)]
    pub probe_timeout_ms: u64,
    /// 是否参与探测（单条粒度）。取代原来的 `route_disabled`（区块内禁用）
    #[serde(default = "serde_default_true")]
    pub enabled: bool,
    /// 对方系统自报的 machine_id —— 幂等键的一半（另一半是 kind + lane）
    #[serde(default)]
    pub mid: String,
    /// 对方自报的系统族（windows / macos / linux）
    #[serde(default)]
    pub os: String,
    /// 对方自报的主机名（列表标题用）
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

impl Default for RemotePair {
    /// 手写而不是 derive：`enabled` 的默认值必须是 **true**（derive 会给 bool 的 false，
    /// 于是任何 `..Default::default()` 造出来的新配对都是「禁用」的，加了一条却不探测）。
    /// 这与 serde 的 `#[serde(default = "serde_default_true")]` 保持一致。
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: String::new(),
            lane: String::new(),
            addr: String::new(),
            token: String::new(),
            probe_timeout_ms: 0,
            enabled: true,
            mid: String::new(),
            os: String::new(),
            host: String::new(),
            tunnel_id: String::new(),
            note: String::new(),
            created_at: 0,
        }
    }
}

impl RemotePair {
    /// 上报/展示用的通道 id（`route_order` / `ProbeOutcome.kind` / [`route_label`] 那一套）。
    /// 空 = 这条配对不可用（kind 非法）。
    pub(crate) fn route_kind(&self) -> &'static str {
        match self.kind.trim() {
            "cloudflare" => "tunnel",
            "tsnet" => "tsnet",
            // ts 区块按 lane 分：直连与 Funnel 的展示/超时语义不同，通道 id 也不同
            "ts" => {
                if self.effective_lane() == "funnel" {
                    "funnel"
                } else {
                    "ts"
                }
            }
            _ => "",
        }
    }

    /// 这条配对的 lane（写空了的按地址形态补：100.64/10 → direct，*.ts.net → funnel）
    pub(crate) fn effective_lane(&self) -> &'static str {
        match self.lane.trim() {
            "direct" => "direct",
            "funnel" => "funnel",
            _ => {
                if is_funnel_origin(&self.addr) {
                    "funnel"
                } else {
                    "direct"
                }
            }
        }
    }

    /// 归一：trim / id 补发 / 地址判型（顺手挖 ab_token）/ lane 推导 / 超时夹紧。
    /// `fallback_kind` = 地址形态推不出区块时的归属（用户从哪个区块点的「添加」）。
    fn normalize(mut self, fallback_kind: &str) -> Option<Self> {
        // 地址栏里粘的是整条配对码？就地升级：区块类型与来源（mid / os / host /
        // tunnel_id）全从码里取，用户不用再手填。**只覆盖写空的字段** —— 用户手填
        // 过的值优先。老串（没有 v=2）解析失败 → 不删数据，按普通地址继续走下面的
        // `split_remote_address`；「拒收 + 提示重新生成」由「识别并加入」的 UI 负责，
        // 存储层不静默丢用户的条目。
        if let Ok(parsed) = parse_pair_string(&self.addr) {
            if !parsed.tunnel.is_empty() {
                self.addr = parsed.tunnel;
                if self.kind.trim().is_empty() {
                    self.kind = parsed.kind;
                }
                if self.mid.trim().is_empty() {
                    self.mid = parsed.mid;
                }
                if self.os.trim().is_empty() {
                    self.os = parsed.os;
                }
                if self.host.trim().is_empty() {
                    self.host = parsed.host;
                }
                if self.token.trim().is_empty() {
                    self.token = parsed.token;
                }
                if self.tunnel_id.trim().is_empty() {
                    self.tunnel_id = parsed.tunnel_id;
                }
            }
        }
        self.kind = self.kind.trim().to_lowercase();
        if !BLOCK_IDS.contains(&self.kind.as_str()) {
            self.kind = fallback_kind.trim().to_lowercase();
        }
        if !BLOCK_IDS.contains(&self.kind.as_str()) {
            return None;
        }
        let (origin, token_in_url) = split_remote_address(&self.addr);
        if origin.is_empty() {
            return None;
        }
        self.addr = origin;
        self.token = self.token.trim().to_string();
        if self.token.is_empty() {
            if let Some(token) = token_in_url {
                self.token = token;
            }
        }
        // lane 只在 ts 区块有意义；其余清空（cloudflare 的地址形态与 lane 无关）
        self.lane = if self.kind == "ts" {
            self.effective_lane().to_string()
        } else {
            String::new()
        };
        if self.probe_timeout_ms > 0 {
            self.probe_timeout_ms = clamp_probe_timeout_ms(self.probe_timeout_ms);
        }
        self.mid = self.mid.trim().to_string();
        self.os = match self.os.trim().to_lowercase().as_str() {
            "windows" => "windows".to_string(),
            "macos" | "darwin" | "osx" => "macos".to_string(),
            "linux" => "linux".to_string(),
            _ => String::new(),
        };
        self.host = self.host.trim().to_string();
        self.tunnel_id = self.tunnel_id.trim().to_string();
        self.note = self.note.trim().to_string();
        if self.id.trim().is_empty() {
            self.id = new_pair_id();
        } else {
            self.id = self.id.trim().to_string();
        }
        Some(self)
    }
}

/// 幂等键：有 `mid` 时按 `(mid, kind, lane)`；`mid` 为空（手填地址）时按 `(addr, kind)`。
///
/// **不能只用 `mid`**：同一个系统可以同时挂 cloudflare + ts + tsnet 三条，
/// 只按 mid 去重会让它们互顶，等于回到原点。
/// **kind 也不能只有区块**：ts 区块合并了「直连」与「系统 Funnel」，同一台电脑的
/// 这两条是**并存**的配对（lane 区分，见文档 §三模型图），只按 `(mid, kind)` 去重
/// 会让扫 funnel 码顶掉 direct 码 —— 这正是合并区块想消灭的互顶，从区块间挪进了区块内。
/// **也不能让空 mid 全撞在一起**：那样手填的第二条就会被吞掉。
/// lane 只对 ts 区块有区分度（其余区块恒空，键只是多了一节空串）；用
/// `effective_lane()` 让 lane 写空的历史数据与新数据的键保持一致。
fn pair_key(pair: &RemotePair) -> String {
    if pair.mid.is_empty() {
        // 手填路径：地址本身已区分 direct / funnel（形态不同），不必再带 lane
        format!("addr|{}|{}", pair.kind, pair.addr)
    } else {
        let lane = if pair.kind.trim() == "ts" {
            pair.effective_lane()
        } else {
            ""
        };
        format!("mid|{}|{}|{}", pair.kind, lane, pair.mid)
    }
}

fn serde_default_true() -> bool {
    true
}

/// 每档案的配对数软上限：再多列表就没法看了，也说明用错了（一条档案 = 一台电脑）
pub(crate) const MAX_PAIRS: usize = 32;

/// 配对 id 序号：一次保存可能同时补发多条 id，纳秒时间戳不足以区分，加个进程内计数
static PAIR_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn new_pair_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = PAIR_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("pair-{nanos:016x}-{seq:04x}")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RemoteServer {
    pub id: String,
    /// 服务端名；空 = 用地址兜底
    #[serde(default)]
    pub name: String,
    /// 候选地址（有序，第 0 个是首选）。`host:port` 或整条带 token 的 URL（保存时归一）
    #[serde(default)]
    pub addresses: Vec<String>,
    /// 访问令牌；空 = 服务端未设 token
    #[serde(default)]
    pub token: String,
    /// `/health` 给的机器标识（同一台电脑多地址归并用）；未知为空
    #[serde(default)]
    pub machine_id: String,
    /// `/health` 给的机器名；未知为空
    #[serde(default)]
    pub machine_name: String,
    /// `/health` 给的系统族（windows / macos / linux）；未知为空。
    /// 客户端用它画系统徽标 —— 探活成功时顺手记下来，省一次额外请求。
    #[serde(default)]
    pub machine_os: String,
    /// **多网卡探测**（同一通道内的多个地址之间怎么探）：
    /// parallel_first（默认，并发取首个成功）/ parallel_all（并发等全部）/ serial（按顺序取第一个通）。
    /// 注意它与 [`Self::route_probe_mode`] 是两个不同粒度：这个只管「一台电脑的多块网卡/多个 IP」，
    /// 那个管「六个连接通道之间」。
    #[serde(default)]
    pub probe_mode: String,
    /// **连接优先级的探测方式**（六个通道之间怎么探，作用于所有通道元素）：
    /// parallel_first（默认，全部通道同时发出、谁先成功用谁）/ parallel_all（等全部返回，逐行报）/
    /// serial（按优先级逐个通道试，第一个通就停）。
    /// 与 [`Self::probe_mode`] 组合成两层策略：外层决定通道之间，内层决定同一通道内多个地址。
    #[serde(default = "serde_default_route_probe_mode")]
    pub route_probe_mode: String,
    /// 探测超时（毫秒，默认 2000）：**通道自己没单独设时的默认值**。
    /// 0 / 缺省 = 用默认值；实际使用前会夹到 [200, 30000]。
    #[serde(default = "serde_default_probe_timeout_ms")]
    pub probe_timeout_ms: u64,
    /// **每个通道自己的探测超时**（毫秒，key = 通道 id：lan / jump / tunnel / ts / funnel / tsnet）。
    /// 缺 key（或值为 0）= 该通道用 `probe_timeout_ms` 这个默认值。
    /// 为什么按通道分开：局域网 RTT <10ms，2 秒都嫌多；而 Funnel / tsnet 走中继时
    /// 首次 TLS 握手可能要好几秒 —— 一个全局值必然一边偏松一边偏紧。
    #[serde(default)]
    pub route_timeouts_ms: std::collections::BTreeMap<String, u64>,
    /// 经由哪台跳板机（hub SSH 服务器 id）；本轮仅存储校验，转发链路后续接入
    #[serde(default)]
    pub jump_id: String,
    /// 每个地址对应的网卡 MAC（局域网电源包要用）：地址 → MAC 列表（小写、冒号分隔）。
    /// 只影响电源 / 唤醒，不参与连接。字段语义与手机端 `ServerProfile.macs` 一致。
    #[serde(default)]
    pub macs: std::collections::BTreeMap<String, Vec<String>>,
    /// 局域网电源路由（唤醒 / 重启 / 关机 / 睡眠 各发到哪个广播地址、哪个端口、
    /// 带哪块网卡的 MAC、发几个包、重试几次）。空 = 没配，走原来的老路。
    #[serde(default)]
    pub power_routes: Vec<PowerRoute>,
    /// **远程接入配对列表**（`pairs` 内的相对顺序 = 同区块内的探测顺序）。
    ///
    /// 取代四个单值槽与单值令牌：一个系统一套通道 = 一条 `RemotePair`。
    /// 幂等键 `(mid, kind, lane)`；「顶替」只剩一个合法场景 —— 同一系统同一区块
    /// 同一 lane 重新配对（ts 区块直连与 Funnel 是两条，互不顶替）。
    #[serde(default)]
    pub pairs: Vec<RemotePair>,
    /// **每个区块内部的探测策略**：parallel_first（缺项 = 它）/ parallel_all / serial。
    /// key ∈ cloudflare / ts / tsnet。与 [`Self::probe_mode`]（区块内多地址）、
    /// [`Self::route_probe_mode`]（区块之间）构成三级策略。
    #[serde(default)]
    pub block_probe_mode: std::collections::BTreeMap<String, String>,
    /// **每个区块的默认探测超时**（毫秒），被 `pairs[].probe_timeout_ms` 覆盖。
    /// key = 区块 id（cloudflare / ts / tsnet）—— 与 [`Self::route_timeouts_ms`]
    /// 的通道 id key **分属两个 key 空间**，同一个表里混两种粒度必然取值歧义。
    #[serde(default)]
    pub block_timeouts_ms: std::collections::BTreeMap<String, u64>,
    /// 连接优先：`remote_first` = 远程优先（隧道通了就走隧道，局域网只做兜底）；
    /// 空 / 其它 = 自动（局域网优先，远程兜底 —— 默认）。
    /// **已废弃**：被 route_order（五通道显式排序）取代，字段保留只为旧档案兼容
    /// （读取时迁移：remote_first → 远程通道在前、LAN 殿后），不再被消费逻辑读取。
    #[serde(default)]
    pub conn_mode: String,
    /// 连接优先级（有序）：id ∈ lan / jump / tunnel / ts / funnel / tsnet。
    /// 空 / 缺项 = 老档案，按默认序（lan→jump→tunnel→ts→funnel）或 conn_mode 迁移补全。
    /// **结构不动**：`lan` / `jump` 仍靠它排序（LAN 选路消费它），配对区块的顺序
    /// 从它的配对通道项推导（见 [`Self::pair_block_order`]）。
    #[serde(default)]
    pub route_order: Vec<String>,
    /// 禁用的**非配对通道** id（lan / jump）—— 配对区块的禁用走 `RemotePair.enabled`。
    ///
    /// 与设计稿的差异：删除整个 `route_disabled` 会让「禁用局域网直连 / 禁用跳板机」
    /// 无处表达（这两条没有配对条目可挂），所以保留字段、语义收窄到 lan / jump。
    #[serde(default)]
    pub route_disabled: Vec<String>,
    #[serde(default)]
    pub created_at: u64,
}

/// 落盘结构：档案列表 + 全局开关
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct RemoteStore {
    #[serde(default)]
    pub connect_all: bool,
    #[serde(default)]
    pub servers: Vec<RemoteServer>,
}

/// 单个地址的探测结果（results 列表用：每个候选地址一条，成败与耗时都在）
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ProbeOutcome {
    pub address: String,
    pub ok: bool,
    pub ms: u64,
    pub error: String,
    /// true = 这一行是远程接入地址（https 隧道），不是局域网候选
    #[serde(default)]
    pub remote: bool,
    /// 通道类型（lan/jump/tunnel/ts/funnel/tsnet）；老结果行缺省 = lan / 按 remote 区分
    #[serde(default)]
    pub kind: String,
    /// 这条结果对应的配对 id（[`RemotePair::id`]）；lan / jump 行为空。
    /// 前端按它回填「哪一行的状态」，不再靠地址字符串匹配
    #[serde(default)]
    pub pair_id: String,
}

/// remote_test 的显式候选（五通道优先级探测用）：kind ∈ lan/jump/tunnel/ts/funnel，
/// origin = 局域网 host:port（lan/jump，jump 由前端先解析好转发端点）或远程 https origin
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RouteTestCandidate {
    pub kind: String,
    pub origin: String,
    /// 这条候选自己的探测超时（毫秒）。0 / 缺省 = 用通道表（`route_timeouts_ms`）或全局默认。
    /// 配对的超时已经按「条目 → 区块 → 全局」算好塞在这里，引擎照办即可
    #[serde(default)]
    pub timeout_ms: u64,
    /// 配对 id（结果行按它回填）；lan / jump 为空
    #[serde(default)]
    pub pair_id: String,
}

/// 一个连接候选（[`RemoteServer::route_candidates`] 展开的结果）。
///
/// 带 `token` 是因为多配对之后「令牌」不再挂在档案上：同一档案里不同配对各带各的
/// 网关客户端令牌（对方系统签发），消费方必须用赢家那一条的令牌。
/// 带 `timeout_ms` 同理：超时三层继承（条目 → 区块 → 全局）在展开时就算好，
/// 消费方不用重复实现继承逻辑、也不会漏掉区块那一层。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RouteCandidate {
    /// 通道 id（lan / jump / tunnel / ts / funnel / tsnet）
    pub kind: String,
    /// 局域网 `host:port`，或远程 origin；jump 为空（由消费方解析转发端点）
    pub origin: String,
    /// 这条候选要用的令牌（lan/jump = 档案 token；配对 = 该配对 token）
    pub token: String,
    /// 这条候选的探测超时（毫秒，已按三层继承算好并夹到 [200, 30000]）
    pub timeout_ms: u64,
}

/// remote_test 的结果：addr/ms 是选中的赢家；results 是全部候选地址的逐行结果
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct RemoteTestResult {
    pub ok: bool,
    pub addr: String,
    pub ms: u64,
    pub machine_id: String,
    pub machine_name: String,
    pub error: String,
    #[serde(default)]
    pub results: Vec<ProbeOutcome>,
    /// true = 赢家是远程接入地址（前端要用 https origin + ab_token 拼 URL）
    #[serde(default)]
    pub remote: bool,
    /// 赢家走的通道 id（lan/jump/tunnel/ts/funnel/tsnet）；前端据此显示「隧道 / 直连 …」
    #[serde(default)]
    pub kind: String,
    /// 赢家报告的系统族（windows / macos / linux）；未知为空
    #[serde(default)]
    pub os: String,
}

impl RemoteServer {
    /// 连接优先是否「远程优先」。空 / auto / 未知值都按自动（局域网优先）处理。
    /// 只剩迁移语义：route_order 为空的老档案读序时用它决定迁移方向。
    pub(crate) fn conn_remote_first(&self) -> bool {
        self.conn_mode.trim() == "remote_first"
    }

    /// 有序五通道 id（自愈）：清洗非法 id / 去重，缺的按默认序补到尾部；
    /// 全空 = 老档案 —— conn_mode=remote_first 迁移为「远程在前、LAN 殿后」，否则默认序。
    pub(crate) fn route_order_sorted(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut order: Vec<String> = self
            .route_order
            .iter()
            .map(|raw| raw.trim().to_string())
            .filter(|id| ROUTE_IDS.contains(&id.as_str()) && seen.insert(id.clone()))
            .collect();
        if order.is_empty() && self.conn_remote_first() {
            // 迁移：远程通道在前（保持 隧道→ts→funnel→内置 的相对序），跳板/直连殿后
            return ["tunnel", "ts", "funnel", "tsnet", "jump", "lan"]
                .iter()
                .map(|id| id.to_string())
                .collect();
        }
        for id in ROUTE_IDS {
            let id = id.to_string();
            if !order.contains(&id) {
                order.push(id);
            }
        }
        order
    }

    /// 区块顺序：从 `route_order` 的配对通道项推导（去重保序），缺的按默认序补齐。
    ///
    /// **不新增 `block_order` 字段**：`route_order` 已经表达了「用户想把哪种接入排前面」，
    /// 再加一个真相源必然出现「两个表说的不一样」。`ts` 与 `funnel` 在这里映射到同一个
    /// `ts` 区块 —— 取先出现者定位即可，两个通道 id 本来就指向同一个区块。
    pub(crate) fn pair_block_order(&self) -> Vec<String> {
        let mut order: Vec<String> = Vec::new();
        for kind in self.route_order_sorted() {
            if let Some(block) = block_of_route(&kind) {
                if !order.iter().any(|item| item == block) {
                    order.push(block.to_string());
                }
            }
        }
        for block in BLOCK_IDS {
            if !order.iter().any(|item| item == block) {
                order.push(block.to_string());
            }
        }
        order
    }

    /// 某区块内的配对（按 `pairs` 里的相对顺序；`enabled_only` 过滤禁用条目）
    pub(crate) fn pairs_of_block(&self, block: &str, enabled_only: bool) -> Vec<&RemotePair> {
        self.pairs
            .iter()
            .filter(|pair| pair.kind == block && (!enabled_only || pair.enabled))
            .collect()
    }

    /// **生效配对**：按区块顺序 + 区块内顺序，第一条 `enabled` 的条目。
    /// 取代原来的 `remote_addr` 存储字段 —— 它不再是配置，而是算出来的结果。
    pub(crate) fn effective_pair(&self) -> Option<&RemotePair> {
        for block in self.pair_block_order() {
            if let Some(pair) = self.pairs_of_block(&block, true).into_iter().next() {
                return Some(pair);
            }
        }
        None
    }

    /// 生效的远程 origin（空 = 没有任何可用配对）。探测 / HTTP / iframe / 终端统一读它
    pub(crate) fn effective_remote_addr(&self) -> String {
        self.effective_pair()
            .map(|pair| pair.addr.clone())
            .unwrap_or_default()
    }

    /// 生效配对的令牌（空 = 不需要 / 没有配对）
    pub(crate) fn effective_remote_token(&self) -> String {
        self.effective_pair()
            .map(|pair| pair.token.clone())
            .unwrap_or_default()
    }

    /// 这条配对实际生效的探测超时：条目 → 区块 → 全局（默认 2s）三层继承。
    pub(crate) fn probe_timeout_for_pair(&self, pair: &RemotePair) -> Duration {
        if pair.probe_timeout_ms > 0 {
            return Duration::from_millis(clamp_probe_timeout_ms(pair.probe_timeout_ms));
        }
        if let Some(raw) = self.block_timeouts_ms.get(pair.kind.trim()) {
            if *raw > 0 {
                return Duration::from_millis(clamp_probe_timeout_ms(*raw));
            }
        }
        self.probe_timeout()
    }

    /// 有序连接候选（已滤掉禁用与不可用项）。
    /// - lan：首个非空局域网地址（补默认端口）；
    /// - jump：经跳板机转发到同一局域网地址（没配跳板机 = 不可用，origin 留空由消费方解析端点）；
    /// - 配对区块：按区块顺序 + 区块内顺序展开**每一条** enabled 配对（不再一区块一条）。
    pub(crate) fn route_candidates(&self) -> Vec<RouteCandidate> {
        let mut out: Vec<RouteCandidate> = Vec::new();
        let has_lan_addr = self.addresses.iter().any(|raw| !raw.trim().is_empty());
        let mut done_blocks: Vec<String> = Vec::new();
        for kind in self.route_order_sorted() {
            if let Some(block) = block_of_route(&kind) {
                // 区块只展开一次（ts 与 funnel 都映射到 ts，第二次跳过）
                if done_blocks.iter().any(|item| item == block) {
                    continue;
                }
                done_blocks.push(block.to_string());
                for pair in self.pairs_of_block(block, true) {
                    let route_kind = pair.route_kind();
                    if route_kind.is_empty() {
                        continue;
                    }
                    out.push(RouteCandidate {
                        kind: route_kind.to_string(),
                        origin: pair.addr.clone(),
                        token: pair.token.clone(),
                        timeout_ms: self.probe_timeout_for_pair(pair).as_millis() as u64,
                    });
                }
                continue;
            }
            if self.route_disabled.iter().any(|raw| raw.trim() == kind) {
                continue;
            }
            match kind.as_str() {
                "lan" => {
                    if let Some(addr) = self
                        .addresses
                        .iter()
                        .map(|raw| raw.trim())
                        .find(|raw| !raw.is_empty())
                    {
                        // 局域网用档案自己的 token（与配对令牌是两套）
                        out.push(RouteCandidate {
                            kind: "lan".to_string(),
                            origin: with_default_port(addr),
                            token: self.token.clone(),
                            timeout_ms: self.probe_timeout_for("lan").as_millis() as u64,
                        });
                    }
                }
                "jump" => {
                    if !self.jump_id.trim().is_empty() && has_lan_addr {
                        out.push(RouteCandidate {
                            kind: "jump".to_string(),
                            origin: String::new(),
                            token: self.token.clone(),
                            timeout_ms: self.probe_timeout_for("jump").as_millis() as u64,
                        });
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// 这台电脑的**默认**探测超时（配对与通道都没单独设时用它；默认 2 秒）
    pub(crate) fn probe_timeout(&self) -> Duration {
        Duration::from_millis(clamp_probe_timeout_ms(self.probe_timeout_ms))
    }

    /// 某个通道自己的探测超时：单独设过就用它，否则回落默认值。
    ///
    /// 「每个通道单独设」是有意义的：局域网 2 秒都算宽，而 Funnel / 内置 tsnet
    /// 走中继时首次 TLS 握手可能好几秒，一个全局值必然一边偏松一边偏紧。
    /// **配对区块**走 `block_timeouts_ms`（见 [`Self::probe_timeout_for_pair`]），
    /// 这张表只剩 `lan` / `jump` 在用。
    pub(crate) fn probe_timeout_for(&self, kind: &str) -> Duration {
        match self.route_timeouts_ms.get(kind.trim()) {
            Some(raw) if *raw > 0 => Duration::from_millis(clamp_probe_timeout_ms(*raw)),
            _ => self.probe_timeout(),
        }
    }

    pub(crate) fn normalize(mut self) -> Self {
        self.id = self.id.trim().to_string();
        self.name = self.name.trim().to_string();
        self.token = self.token.trim().to_string();
        self.machine_id = self.machine_id.trim().to_string();
        self.machine_name = self.machine_name.trim().to_string();
        // 系统族只认三个徽标值；别的（比如老客户端传来的 "Windows"）归空，
        // 前端据此不画徽标而不是画个空白框
        self.machine_os = match self.machine_os.trim().to_lowercase().as_str() {
            "windows" => "windows".to_string(),
            "macos" | "darwin" | "osx" => "macos".to_string(),
            "linux" => "linux".to_string(),
            _ => String::new(),
        };
        self.probe_timeout_ms = clamp_probe_timeout_ms(self.probe_timeout_ms);
        self.jump_id = self.jump_id.trim().to_string();
        self.probe_mode = match self.probe_mode.as_str() {
            "serial" => "serial".to_string(),
            "parallel_all" => "parallel_all".to_string(),
            // parallel_first 是默认；旧值 parallel 与空值都归到它（兼容老配置）
            _ => "parallel_first".to_string(),
        };
        // 连接优先级的探测方式（通道之间）：与上面那个内层策略同一套取值
        self.route_probe_mode = normalize_probe_mode(&self.route_probe_mode).to_string();
        // 连接优先：只认 remote_first，其余一律归空（= 自动，局域网优先）
        self.conn_mode = if self.conn_remote_first() {
            "remote_first".to_string()
        } else {
            String::new()
        };
        // 五通道优先级：清洗（只认五个 id、去重、缺项按默认序补尾）+ 禁用表清洗。
        // 全空 = 老档案：conn_mode=remote_first 迁移为「远程在前、LAN 殿后」，否则默认序；
        // route_order_sorted() 是读路径的自愈版，这里负责把迁移结果落到盘上。
        if self.route_order.iter().all(|raw| raw.trim().is_empty()) {
            self.route_order = if self.conn_remote_first() {
                ["tunnel", "ts", "funnel", "jump", "lan"]
                    .iter()
                    .map(|id| id.to_string())
                    .collect()
            } else {
                default_route_order()
            };
        } else {
            self.route_order = self.route_order_sorted();
        }
        // 禁用表只服务非配对通道（lan / jump）：配对区块的禁用是 `RemotePair.enabled`
        self.route_disabled = self
            .route_disabled
            .iter()
            .map(|raw| raw.trim().to_lowercase())
            .filter(|id| id == "lan" || id == "jump")
            .collect();
        // 每通道探测超时：只留合法通道 id，值 0 等于「没单独设」（删掉，回落默认值），越界夹紧
        self.route_timeouts_ms = normalize_route_timeouts(&self.route_timeouts_ms);
        // 地址逐条归一：挖 URL 形态里的 token 与裸地址；trim、去空、去重（保序）
        let mut token_from_url: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        let mut addresses = Vec::new();
        for raw in &self.addresses {
            let (address, token) = split_address(raw);
            if address.is_empty() || !seen.insert(address.clone()) {
                continue;
            }
            if token.is_some() && self.token.is_empty() && token_from_url.is_none() {
                token_from_url = token;
            }
            addresses.push(address);
        }
        if let Some(token) = token_from_url {
            self.token = token;
        }
        self.addresses = addresses;
        // MAC：只留还存在的地址（地址删了 MAC 跟着丢），MAC 统一小写冒号、去重
        let mut macs: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
        for (address, values) in std::mem::take(&mut self.macs) {
            let (address, _) = split_address(&address);
            if !self.addresses.contains(&address) {
                continue;
            }
            let mut list: Vec<String> = Vec::new();
            for value in values {
                if let Some(mac) = normalize_mac(&value) {
                    if !list.contains(&mac) {
                        list.push(mac);
                    }
                }
            }
            if !list.is_empty() {
                macs.insert(address, list);
            }
        }
        self.macs = macs;
        // 电源路由：动作认成四个之一、端口 0 补 9、广播地址去掉 scheme/路径、
        // MAC 归一去重；burst/retry 保留 0 = 「按动作默认」的语义
        let mut routes: Vec<PowerRoute> = Vec::new();
        for raw in std::mem::take(&mut self.power_routes) {
            let mut list: Vec<String> = Vec::new();
            for value in &raw.macs {
                if let Some(mac) = normalize_mac(value) {
                    if !list.contains(&mac) {
                        list.push(mac);
                    }
                }
            }
            routes.push(PowerRoute {
                action: raw.action_or_wake().to_string(),
                target: normalize_route_target(&raw.target),
                port: raw.effective_port(),
                macs: list,
                enabled: raw.enabled,
                burst: if raw.burst == 0 { 0 } else { raw.effective_burst() },
                retry: if raw.retry == 0 { 0 } else { raw.effective_retry() },
                // mode 只认四个合法值；http_fallback 按 mode 派生（与 hub.rs 同一套）
                mode: match raw.mode.as_str() {
                    "auto" | "http_first" | "http_only" | "packet_only" => raw.mode.clone(),
                    _ => String::new(),
                },
                http_fallback: matches!(raw.effective_mode(), "auto" | "http_first"),
            });
        }
        self.power_routes = routes;
        // 远程接入配对归一：每条 trim / 判型 / 挖 token / 补 id，再按幂等键去重、截到软上限
        self.pairs = normalize_pairs(std::mem::take(&mut self.pairs));
        // 区块策略：只留三个合法区块，值归一到三种取值；= 默认值（parallel_first）就不存，
        // 缺项即默认，存了反而多一份「看起来配过其实没配」的噪音
        let mut block_modes = std::collections::BTreeMap::new();
        for (key, value) in std::mem::take(&mut self.block_probe_mode) {
            let key = key.trim().to_lowercase();
            if !BLOCK_IDS.contains(&key.as_str()) {
                continue;
            }
            let mode = normalize_probe_mode(&value);
            if mode != "parallel_first" {
                block_modes.insert(key, mode.to_string());
            }
        }
        self.block_probe_mode = block_modes;
        // 区块默认超时：只留合法区块 id，值 0 = 没设（删掉回落全局），越界夹紧
        let mut block_timeouts = std::collections::BTreeMap::new();
        for (key, value) in std::mem::take(&mut self.block_timeouts_ms) {
            let key = key.trim().to_lowercase();
            if !BLOCK_IDS.contains(&key.as_str()) || value == 0 {
                continue;
            }
            block_timeouts.insert(key, clamp_probe_timeout_ms(value));
        }
        self.block_timeouts_ms = block_timeouts;
        self
    }

    /// 全部远程 origin（按区块顺序 + 区块内顺序，含 disabled）。
    /// 给「展示所有配过的地址」用；连接与探测请用 [`Self::route_candidates`]。
    pub(crate) fn remote_origins(&self) -> Vec<String> {
        let mut list = Vec::new();
        for block in self.pair_block_order() {
            for pair in self.pairs_of_block(&block, false) {
                if !pair.addr.is_empty() && !list.iter().any(|item| item == &pair.addr) {
                    list.push(pair.addr.clone());
                }
            }
        }
        list
    }

    /// 这台电脑配过的所有 MAC（去重、按地址顺序）—— 电源包要用
    pub fn all_macs(&self) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        for address in &self.addresses {
            if let Some(values) = self.macs.get(address) {
                for mac in values {
                    if !result.contains(mac) {
                        result.push(mac.clone());
                    }
                }
            }
        }
        for values in self.macs.values() {
            for mac in values {
                if !result.contains(mac) {
                    result.push(mac.clone());
                }
            }
        }
        result
    }
}

// ==================== 纯函数（单测覆盖） ====================

/// 地址归一：接受 `host:port`、裸 `host`（探活时补默认端口）或整条
/// `http://host:port/?token=x` URL —— 剥出裸 `host:port` 与 token。
/// token 只在显式带时返回；没有就是 None。
fn split_address(raw: &str) -> (String, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    // 非URL 形态原样（trim 过）
    let lower = raw.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return (raw.to_string(), None);
    }
    let rest = &raw[if lower.starts_with("https://") { 8 } else { 7 }..];
    let (authority, query) = match rest.find(['?', '/']) {
        Some(index) => {
            let tail = &rest[index..];
            let query = match tail.find('?') {
                Some(q) => &tail[q + 1..],
                None => "",
            };
            (&rest[..index], query)
        }
        None => (rest, ""),
    };
    let authority = authority.trim().to_string();
    // 挖 token= / ab_token=（query 里第一个命中；大小写不敏感，对齐 http.rs 的宽松风格）
    let mut token = None;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if key == "token" || key == "ab_token" {
            let value = parts.next().unwrap_or("").trim().to_string();
            if !value.is_empty() {
                token = Some(value);
            }
            break;
        }
    }
    (authority, token)
}

/// 配对串（v2）解析结果。
///
/// `kind` 是**区块** id（`cloudflare` / `ts` / `tsnet`），不是通道 id —— 与
/// `RemotePair.kind` 同一套取值，见 `docs/remote-pairs.md` §4.5。
/// `lane` 不占串参数：由地址形态推导（100.64/10 → direct，`*.ts.net` → funnel）。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ParsedPair {
    pub mid: String,
    pub os: String,
    pub host: String,
    pub kind: String,
    pub tunnel: String,
    pub token: String,
    pub tunnel_id: String,
}

/// 解析配对串 `anyboot://pair?v=2&mid=…&os=…&host=…&type=…&tunnel=…&token=…&tunnel_id=…`
///
/// **只认 `v=2`**：老串没有 `mid` / `type`，硬解出来的条目既无法去重也判不出区块，
/// 结果是「配一个顶掉另一个」。所以 `v != 2` 一律拒收并让用户回电脑上重新生成
/// （`docs/remote-pairs.md` §4.5「不解析老串」）。
///
/// `type` 非法（含整个缺失）同样拒收：猜类型是历史 bug 的源头 —— 三套 `*.ts.net`
/// 长得一模一样，靠地址猜必然把「内建 tsnet」和「系统 Funnel」混成一个。
pub(crate) fn parse_pair_string(raw: &str) -> Result<ParsedPair, String> {
    let raw = raw.trim();
    if !raw.to_ascii_lowercase().starts_with("anyboot://pair?") {
        return Err("不是配对码（要 anyboot://pair?v=2&… 开头）".to_string());
    }
    let query = &raw["anyboot://pair?".len()..];
    let (mut version, mut mid, mut os, mut host) = (String::new(), String::new(), String::new(), String::new());
    let (mut kind, mut tunnel, mut token, mut tunnel_id) =
        (String::new(), String::new(), String::new(), String::new());
    for item in query.split('&') {
        let mut parts = item.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let value = percent_decode(parts.next().unwrap_or("").trim());
        match key.as_str() {
            "v" | "ver" | "version" => version = value,
            "mid" => mid = value,
            "os" => os = value,
            // `gateway` 是 v1 遗留的显示名，v2 里与 `host` 同值；`host` 优先。
            "host" => host = value,
            "gateway" if host.is_empty() => host = value,
            "type" => kind = value,
            "tunnel" => tunnel = value,
            "token" | "ab_token" => token = value,
            "tunnel_id" => tunnel_id = value,
            _ => {}
        }
    }
    if version != "2" {
        return Err(if version.is_empty() {
            "这是旧版配对码（缺 v=2），请在电脑上重新生成一个".to_string()
        } else {
            format!("配对码版本不支持（v={version}），请在电脑上重新生成一个")
        });
    }
    let kind = match kind.trim().to_ascii_lowercase().as_str() {
        "cloudflare" => "cloudflare".to_string(),
        "ts" => "ts".to_string(),
        "tsnet" => "tsnet".to_string(),
        other => {
            return Err(format!(
                "配对码里的 type 非法（{}），要 cloudflare / ts / tsnet",
                if other.is_empty() { "缺失" } else { other }
            ))
        }
    };
    Ok(ParsedPair {
        mid: mid.trim().to_string(),
        os: match os.trim().to_lowercase().as_str() {
            "windows" => "windows".to_string(),
            "macos" | "darwin" | "osx" => "macos".to_string(),
            "linux" => "linux".to_string(),
            _ => String::new(),
        },
        host: host.trim().to_string(),
        kind,
        tunnel: tunnel.trim().to_string(),
        token: token.trim().to_string(),
        tunnel_id: tunnel_id.trim().to_string(),
    })
}

/// 远程接入地址归一：`https://xxx.trycloudflare.com/?ab_token=t` 这类整条 URL、
/// 裸 `xxx.trycloudflare.com` 都吃。返回 (origin, token)：
/// - origin = `scheme://authority`（无 scheme 默认 `https://`；隧道与 ts.net 都是 https；
///   路径剥掉 —— 隧道地址没有路径语义）；
/// - token 从 query 的 `ab_token=` / `token=` 挖出（没有为 None）。
fn split_remote_address(raw: &str) -> (String, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    // 配对串形态：anyboot://pair?gateway=…&tunnel=https://…&token=… ——
    // 拆出 tunnel 与 token（值是 encode_query 转义过的，先解码），tunnel 再走一遍归一
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("anyboot://pair?") {
        let query = &raw[15..];
        let mut tunnel = None;
        let mut token = None;
        for pair in query.split('&') {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
            let value = percent_decode(parts.next().unwrap_or("").trim());
            match key.as_str() {
                "tunnel" if !value.is_empty() => tunnel = Some(value),
                "token" | "ab_token" if !value.is_empty() => token = Some(value),
                _ => {}
            }
        }
        return match tunnel {
            Some(t) => {
                let (origin, token_in_url) = split_remote_address(&t);
                (origin, token.or(token_in_url))
            }
            None => (String::new(), token),
        };
    }
    let lower = raw.to_ascii_lowercase();
    // 裸 IP/主机名的兜底 scheme：100.64.0.0/10（Tailscale CGNAT 段）补 `http://` ——
    // AnyBoot 本机服务是明文 HTTP，按旧逻辑一律补 https 会永远连不上（审计 P0）。
    let (scheme, rest) = if lower.starts_with("https://") {
        ("https://", &raw[8..])
    } else if lower.starts_with("http://") {
        ("http://", &raw[7..])
    } else if is_tailscale_host(&raw) {
        ("http://", raw.as_ref())
    } else {
        ("https://", raw)
    };
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = rest[..end].trim();
    if authority.is_empty() {
        return (String::new(), None);
    }
    let query = match rest.find('?') {
        Some(index) => &rest[index + 1..],
        None => "",
    };
    let mut token = None;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if key == "token" || key == "ab_token" {
            let value = parts.next().unwrap_or("").trim().to_string();
            if !value.is_empty() {
                token = Some(value);
            }
            break;
        }
    }
    // Tailscale 裸地址没带端口：补默认端口（对齐 with_default_port 的局域网惯例）
    let origin = if scheme == "http://" && is_tailscale_host(authority) && !authority.contains(':') {
        format!("{scheme}{}", with_default_port(authority))
    } else {
        format!("{scheme}{authority}")
    };
    (origin, token)
}

/// 远程 origin 判型：Tailscale 槽 = 100.64/10 直连 或 `*.ts.net`（Funnel）。
/// 隧道槽 = 其余 https 公网地址（trycloudflare / cpolar 等）。
/// 与前端 isTailscaleAddr / 移动端 ServerProfile.isTailscaleAddr 同语义，改这里三处对齐。
pub(crate) fn is_tailscale_origin(origin: &str) -> bool {
    let host = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .unwrap_or(origin);
    let host = host.split('/').next().unwrap_or(host);
    // 剥端口（100.x:12300 / ts.net 一般不带端口；is_tailscale_host 自己也能处理端口）
    let bare = host.split(':').next().unwrap_or(host);
    is_tailscale_host(host) || bare.to_ascii_lowercase().ends_with(".ts.net")
}

/// 主机名是否落在 Tailscale CGNAT 段 100.64.0.0/10（100.64.0.0 ~ 100.127.255.255）。
/// 只认点分 IPv4；带端口（`100.x:12300`）取冒号前段；IPv6 字面量（多个冒号）返回 false。
/// 前端（hub.html pcConnLabel）与移动端各有一份同语义实现，改这里记得三处对齐。
pub(crate) fn is_tailscale_host(host: &str) -> bool {
    let host = host.trim();
    if host.contains(':') {
        // 唯一一个冒号当端口分隔，多个冒号是 IPv6
        let parts: Vec<&str> = host.split(':').collect();
        if parts.len() != 2 {
            return false;
        }
        return is_tailscale_host(parts[0]);
    }
    let octets: Vec<&str> = host.split('.').collect();
    if octets.len() < 2 {
        return false;
    }
    match (octets[0].parse::<u8>(), octets[1].parse::<u8>()) {
        (Ok(a), Ok(b)) => a == 100 && (64..=127).contains(&b),
        _ => false,
    }
}

/// 探活地址兜底默认端口：不带 `:` 的补 `:12300`（对齐手机 DEFAULT_PORT）。
/// 只看最后一个冒号后面是否全数字，避免误伤 IPv6 字面量（本地局域网场景 v4 为主）。
fn with_default_port(addr: &str) -> String {
    if addr.contains(':') {
        let port = addr.rsplit(':').next().unwrap_or("");
        if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
            return addr.to_string();
        }
    }
    format!("{addr}:{DEFAULT_PORT}")
}

/// 从 `/health` 响应体里取 machineId/machineName（解析失败按空串，探活不因它失败）
fn parse_health(body: &str) -> (String, String) {
    let (machine_id, machine_name, _) = parse_health_detail(body);
    (machine_id, machine_name)
}

/// 同上，额外取 `os`（windows / macos / linux）—— 客户端画系统徽标用。
/// 老版本服务端的 `/health` 没有这个字段，取不到就是空串（不显示徽标）。
fn parse_health_detail(body: &str) -> (String, String, String) {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let os = match text("os").to_lowercase().as_str() {
        "windows" => "windows".to_string(),
        "macos" | "darwin" | "osx" => "macos".to_string(),
        "linux" => "linux".to_string(),
        _ => String::new(),
    };
    (text("machineId"), text("machineName"), os)
}

/// 对一台电脑的一个地址做 `GET /health` 探活：成功返回 (耗时毫秒, machineId, machineName)。
fn probe_health(addr: &str, timeout: Duration) -> Result<(u64, String, String), String> {
    let (ms, machine_id, machine_name, _) = probe_health_detail(addr, timeout)?;
    Ok((ms, machine_id, machine_name))
}

/// 探活并带回系统族（新引擎用）：(耗时, machineId, machineName, os)
fn probe_health_detail(addr: &str, timeout: Duration) -> Result<(u64, String, String, String), String> {
    let target = with_default_port(addr);
    let started = Instant::now();
    let mut stream = TcpStream::connect(&target)
        .map_err(|error| format!("连接 {target} 失败: {error}"))
        .and_then(|stream| {
            stream
                .set_read_timeout(Some(timeout))
                .map_err(|error| format!("设置读超时失败: {error}"))?;
            stream
                .set_write_timeout(Some(timeout))
                .map_err(|error| format!("设置写超时失败: {error}"))?;
            Ok(stream)
        })?;
    let request = format!("GET /health HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("发送请求失败: {error}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("读取响应失败: {error}"))?;
    let text = String::from_utf8_lossy(&raw);
    let body = match text.find("\r\n\r\n") {
        Some(index) => &text[index + 4..],
        None => "",
    };
    let (machine_id, machine_name, os) = parse_health_detail(body);
    let ms = started.elapsed().as_millis() as u64;
    Ok((ms, machine_id, machine_name, os))
}

/// 远程地址（公网隧道 / ts.net）的探测与请求都走 ureq：它们是 https，
/// 手写 TCP 客户端做不了 TLS（本地局域网场景才用 probe_health）。
/// 超时放宽到 8s —— 公网 RTT 大，按局域网 3s 判会大量误报。
const REMOTE_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// ureq GET：返回 (状态码, 响应体)。4xx/5xx 也带回状态码由调用方解读。
fn ureq_get(url: &str, timeout: Duration) -> Result<(u16, String), String> {
    let response = ureq::get(url)
        .timeout(timeout)
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(code, response) => {
                let body = response.into_string().unwrap_or_default();
                format!("HTTP {code}: {}", body.trim().chars().take(200).collect::<String>())
            }
            other => format!("{other}"),
        })?;
    let status = response.status();
    let body = response.into_string().map_err(|error| format!("读响应失败: {error}"))?;
    Ok((status, body))
}

/// 对远程地址做 `GET {origin}/health`（免鉴权，与局域网探测同一协议）。
/// 成功返回 (耗时毫秒, machineId, machineName)。
fn probe_remote_health(origin: &str, timeout: Duration) -> Result<(u64, String, String), String> {
    let (ms, machine_id, machine_name, _) = probe_remote_health_detail(origin, timeout)?;
    Ok((ms, machine_id, machine_name))
}

/// 远程探活并带回系统族（新引擎用）：(耗时, machineId, machineName, os)
fn probe_remote_health_detail(
    origin: &str,
    timeout: Duration,
) -> Result<(u64, String, String, String), String> {
    let url = format!("{origin}/health");
    let started = Instant::now();
    let (_status, body) = ureq_get(&url, timeout)?;
    let (machine_id, machine_name, os) = parse_health_detail(&body);
    let ms = started.elapsed().as_millis() as u64;
    Ok((ms, machine_id, machine_name, os))
}

/// 远程 HTTP 请求（带 ab_token 的完整 GET，走 ureq）：电源兜底 / 引导项读取
/// 出门时的那一条路。`path_with_query` 形如 `/api/power?action=…&token=…`
/// —— ab_token 由这里统一追加，调用方不用管。
fn remote_https_get(origin: &str, ab_token: &str, path_with_query: &str) -> Result<(u16, String), String> {
    let mut query = path_with_query.to_string();
    let ab_token = ab_token.trim();
    if !ab_token.is_empty() {
        query.push_str(if query.contains('?') { "&" } else { "?" });
        query.push_str("ab_token=");
        query.push_str(&encode_query(ab_token));
    }
    ureq_get(&format!("{origin}{query}"), Duration::from_secs(12))
}

// ==================== 局域网电源（路由 sol + HTTP 兜底） ====================

/// URL 查询串转义：保留字母数字和 -_.~，其余转 %XX（token 里出现 & %= 都不怕）
pub(crate) fn encode_query_pub(value: &str) -> String {
    encode_query(value)
}

/// encode_query 的逆：%XX 解回字节；坏转义原样保留。
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = bytes.get(index + 1..index + 3);
            match hex.and_then(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|text| u8::from_str_radix(text, 16).ok())
            }) {
                Some(byte) => {
                    out.push(byte);
                    index += 3;
                }
                None => {
                    out.push(bytes[index]);
                    index += 1;
                }
            }
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 探活地址兜底默认端口（跨模块复用：网关在线拉取要对 peer 地址补端口）。
pub(crate) fn with_default_port_pub(addr: &str) -> String {
    with_default_port(addr)
}

fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 调这台电脑自带的 AnyBoot 接口（`GET /api/power?action=..&token=..`）。
///
/// 前提是这台电脑还在线 —— 睡了 / 关了它的 HTTP 服务也没了。对「唤醒」这反而是
/// 有效的探测：请求能送达就说明机器醒着（服务端回 WakeAck，不做任何动作）。
/// 局域网地址全连不上时自动兜底远程接入地址（https 隧道 + ab_token）——
/// 出门在外时电源指令照样能发。
fn remote_http_power(server: &RemoteServer, action: &str) -> Result<String, String> {
    let path = format!("/api/power?action={action}");
    // 按用户优先级逐通道试（route_candidates 已滤禁用与不可用项）；
    // 电源路径没有 AppHandle：jump 候选解析不了转发端点，这里跳过（引导项/JSON 路径支持）
    let candidates = server.route_candidates();
    let mut last_error = String::from("没有可用连接通道");
    let mut tried = 0usize;
    for cand in &candidates {
        if cand.kind == "jump" {
            continue;
        }
        tried += 1;
        let result = if cand.kind == "lan" {
            lan_http_get(
                &cand.origin,
                &power_query_path(action, server.token.trim()),
                15,
            )
            .and_then(|(status, body)| parse_power_reply(status, &body))
        } else {
            // 令牌用**这条候选自己的**：不同配对各带各的网关客户端令牌
            remote_https_get(&cand.origin, &cand.token, &path)
                .and_then(|(status, body)| parse_power_reply(status, &body))
        };
        match result {
            Ok(message) => {
                return Ok(if cand.kind == "lan" {
                    message
                } else {
                    format!("（经{}）{message}", route_label(&cand.kind))
                });
            }
            Err(error) => last_error = error,
        }
    }
    if tried == 0 {
        return Err(
            "没有可用连接通道（局域网地址为空，或优先级里所有通道都被禁用/不可用）".to_string(),
        );
    }
    Err(last_error)
}

/// 电源接口的查询串（`?action=..&token=..`），LAN 直连用
fn power_query_path(action: &str, token: &str) -> String {
    let mut query = format!("action={action}");
    if !token.is_empty() {
        query.push_str("&token=");
        query.push_str(&encode_query(token));
    }
    format!("/api/power?{query}")
}

/// 手写 TCP 的单次 GET（LAN 直连 / 跳板端点共用）：返回 (status, body)。
/// 跳板链路电源动作服务端要等机器响应，读超时放宽（调用方传）。
fn lan_http_get(target: &str, path_and_query: &str, read_timeout_secs: u64) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect(target)
        .map_err(|error| format!("连接 {target} 失败: {error}"))
        .and_then(|stream| {
            stream
                .set_read_timeout(Some(Duration::from_secs(read_timeout_secs)))
                .map_err(|error| format!("设置读超时失败: {error}"))?;
            stream
                .set_write_timeout(Some(Duration::from_secs(4)))
                .map_err(|error| format!("设置写超时失败: {error}"))?;
            Ok(stream)
        })?;
    let request = format!(
        "GET {path_and_query} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("发送请求失败: {error}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("读取响应失败: {error}"))?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = match text.find("\r\n\r\n") {
        Some(index) => text[index + 4..].to_string(),
        None => String::new(),
    };
    Ok((status, body))
}

/// 电源接口的局域网那一路（手写 TCP；route_candidates 之外保留给单测与直调）。
fn remote_http_power_lan(server: &RemoteServer, action: &str) -> Result<String, String> {
    let address = server
        .addresses
        .iter()
        .map(|raw| raw.trim())
        .find(|raw| !raw.is_empty())
        .ok_or_else(|| "这台电脑还没有可用地址".to_string())?;
    let target = with_default_port(address);
    lan_http_get(&target, &power_query_path(action, server.token.trim()), 15)
        .and_then(|(status, body)| parse_power_reply(status, &body))
}

/// 电源接口回执解析（LAN / 远程共用）：401/4xx/非 JSON 的分支都收在这。
fn parse_power_reply(status: u16, body: &str) -> Result<String, String> {
    if status == 401 {
        return Err("这台电脑不认这个 token（401）—— 到「编辑」里核对访问令牌".to_string());
    }
    if status >= 400 {
        return Err(format!("这台电脑返回 {status}：{}", body.trim()));
    }
    if status == 0 {
        return Err("这台电脑的响应不是有效的 HTTP".to_string());
    }
    // 优先 message，其次 error，都不是 JSON 就把原文带回去
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body.trim()) {
        for key in ["message", "error"] {
            if let Some(text) = value.get(key).and_then(|item| item.as_str()) {
                if !text.trim().is_empty() {
                    return Ok(text.to_string());
                }
            }
        }
    }
    let fallback = body.trim();
    Ok(if fallback.is_empty() {
        "已提交".to_string()
    } else {
        fallback.to_string()
    })
}

/// 电脑档案的电源动作：按「局域网电源路由」发 sol 包；唤醒没配路由就按默认
/// （每块网卡 10 包 / 重试 10 次）广播魔术包，其余动作没配路由就调这台电脑
/// 自带的 AnyBoot 接口；配了但一个包都没发出去、且路由勾了「HTTP 兜底」
/// → 也改走它的接口（唤醒时 HTTP 能通 = 机器本来就醒着）。语义与手机端
/// `runPowerAction` 一致。
#[tauri::command]
pub async fn remote_power(
    app: AppHandle,
    server_id: String,
    action: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = load_store(&app)
            .ok()
            .and_then(|store| {
                store
                    .servers
                    .iter()
                    .find(|server| server.id == server_id)
                    .map(|server| server.name.clone())
            })
            .unwrap_or_else(|| "?".to_string());
        crate::hub_power::power_log(
            &app,
            &format!("remote_power 入口：电脑「{name}」({server_id}) 动作 {action}"),
        );
        let result = remote_power_blocking(&app, &server_id, &action);
        match &result {
            Ok(message) => crate::hub_power::power_log(&app, &format!("remote_power 完成：{message}")),
            Err(message) => crate::hub_power::power_log(&app, &format!("remote_power 失败：{message}")),
        }
        result
    })
    .await
    .map_err(|error| format!("电源操作任务失败：{error}"))?
}

fn remote_power_blocking(app: &AppHandle, id: &str, action: &str) -> Result<String, String> {
    let action = crate::hub::normalize_power_action(action)
        .ok_or_else(|| format!("未知电源动作：{action}"))?;
    let label = crate::hub_power::action_label(action);
    // 本机行：直连 IPC —— 电源动作直接走托盘/主窗口同一套进程内执行（spawn_power，
    // 忙闸/埋点/结果推送全都在），不回环 HTTP。唤醒对发号施令的这台没有意义，直接说明。
    if id == "local" {
        return match action {
            crate::hub::POWER_WAKE => Ok("本机正在运行，无需唤醒".to_string()),
            other => {
                let parsed = crate::power::PowerAction::parse(other)
                    .ok_or_else(|| format!("未知电源动作：{other}"))?;
                crate::power::spawn_power(app, parsed, true);
                Ok(format!("{}（已提交）", parsed.label()))
            }
        };
    }
    let server = load_store(app)?
        .servers
        .into_iter()
        .find(|candidate| candidate.id == id)
        .ok_or_else(|| format!("这台电脑不存在（可能已被删除）：{id}"))?;
    let jump = if server.jump_id.is_empty() {
        None
    } else {
        Some(
            crate::hub::load_servers_pub(app)?
                .into_iter()
                .find(|candidate| candidate.id == server.jump_id)
                .ok_or_else(|| "跳板机不存在（可能已被删除），请到服务器列表检查".to_string())?,
        )
    };
    let all_macs = server.all_macs();
    let routes: Vec<PowerRoute> = server
        .power_routes
        .iter()
        .filter(|route| {
            route.enabled && crate::hub::normalize_power_action(&route.action) == Some(action)
        })
        .cloned()
        .collect();
    if routes.is_empty() {
        if action == crate::hub::POWER_WAKE {
            // 没配唤醒路由：不调 HTTP（机器关着接口是死的），按默认广播魔术包 ——
            // 每块网卡 10 个 / 重试 10 次，目标 = 各地址的 /24 广播 + 全局广播
            // （与手机端 runPowerAction 的回落一致）。
            let default_route = PowerRoute {
                action: crate::hub::POWER_WAKE.to_string(),
                target: String::new(),
                port: 9,
                macs: Vec::new(),
                enabled: true,
                burst: 0,
                retry: 0,
                mode: String::new(),
                http_fallback: false,
            };
            let plan = crate::hub_power::plan_routes(
                crate::hub::POWER_WAKE,
                &server.addresses,
                &all_macs,
                std::slice::from_ref(&default_route),
            )?;
            let (_sent, where_from) = crate::hub_power::send_plan(app, jump.as_ref(), &plan)?;
            return Ok(format!(
                "（这台电脑没配「唤醒」路由，按默认每块网卡 10 个魔术包 / 重试 10 次广播）\
                 已从{where_from}发送：{}",
                plan.described.join(" · ")
            ));
        }
        let message = remote_http_power(&server, action)?;
        return Ok(format!(
            "（这台电脑没配「{label}」路由，走的是它自己的 AnyBoot 接口）{message}"
        ));
    }

    let mode = crate::hub::routes_mode(&routes);

    // 仅 HTTP：一个广播包都不发，直接调它自己的接口
    if mode == "http_only" {
        let message = remote_http_power(&server, action)?;
        return Ok(format!("（仅 HTTP 模式）已调它自己的 AnyBoot 接口：{message}"));
    }

    // 优先 HTTP：先调接口，通了就完事（一个广播包都不发）；没通退回广播包
    let mut http_first_error: Option<String> = None;
    if mode == "http_first" {
        match remote_http_power(&server, action) {
            Ok(message) => {
                return Ok(format!("（优先 HTTP 模式）已调它自己的 AnyBoot 接口：{message}"));
            }
            Err(http_error) => {
                http_first_error = Some(http_error);
            }
        }
    }

    // 混合：HTTP 接口与广播包**同时**发 —— HTTP 起独立线程，广播包走当前线程，
    // 互不等待；最后把两边结果合并汇报（哪边成了哪边败都说清楚）。
    if mode == "hybrid" {
        let plan = crate::hub_power::plan_routes(action, &server.addresses, &all_macs, &routes)?;
        let http_server = server.clone();
        let http_action = action.to_string();
        let http = std::thread::spawn(move || remote_http_power(&http_server, &http_action));
        let send_result = crate::hub_power::send_plan(app, jump.as_ref(), &plan);
        let http_result =
            http.join()
                .unwrap_or_else(|_| Err("HTTP 线程异常退出".to_string()));
        let packet_text = match &send_result {
            Ok((_sent, where_from)) => format!(
                "已从{where_from}按路由发送{label}：{}",
                plan.described.join(" · ")
            ),
            Err(error) => format!("广播包没发出去（{error}）"),
        };
        return match (&send_result, &http_result) {
            (Ok(_), Ok(http_message)) => Ok(format!(
                "（混合模式）HTTP 与广播包已同时发送 —— HTTP：{http_message}；广播包：{packet_text}"
            )),
            (Ok(_), Err(http_error)) => Ok(format!(
                "（混合模式）广播包已发出：{packet_text}；HTTP 接口失败：{http_error}"
            )),
            (Err(_), Ok(http_message)) => Ok(format!(
                "（混合模式）HTTP 接口已执行：{http_message}；{packet_text}"
            )),
            (Err(_), Err(http_error)) => Err(format!(
                "（混合模式）两边都失败 —— {packet_text}；HTTP：{http_error}"
            )),
        };
    }

    let plan = crate::hub_power::plan_routes(action, &server.addresses, &all_macs, &routes)?;
    match crate::hub_power::send_plan(app, jump.as_ref(), &plan) {
        Ok((_sent, where_from)) => {
            let mut message =
                format!("已从{where_from}按路由发送{label}：{}", plan.described.join(" · "));
            if let Some(http_error) = &http_first_error {
                message =
                    format!("（优先 HTTP 模式）接口没通（{http_error}），改发广播包 —— {message}");
            }
            if action != crate::hub::POWER_WAKE {
                message.push_str("。这个动作靠目标机上的 sol 监听该端口执行 —— 没装 sol 不会有反应");
            }
            if !plan.problems.is_empty() {
                message.push_str(&format!(
                    "（另有 {} 条路由被跳过：{}）",
                    plan.problems.len(),
                    plan.problems.join("；")
                ));
            }
            Ok(message)
        }
        Err(send_error) => {
            // 兜底条件：只有 auto（优先广播包）才回头调 HTTP。packet_only 明确不要；
            // http_first 开头已经试过一次 HTTP 了，不再重复，把两边错误一起报。
            let prefix = match &http_first_error {
                Some(http_error) => format!("（优先 HTTP 模式）接口没通（{http_error}）；"),
                None => String::new(),
            };
            if mode != "auto" {
                return Err(format!("{prefix}{send_error}"));
            }
            match remote_http_power(&server, action) {
                Ok(message) => Ok(format!(
                    "sol 包没发出去（{send_error}），已改走它自己的 AnyBoot 接口：{message}"
                )),
                Err(http_error) => {
                    Err(format!("sol 包没发出去（{send_error}）；HTTP 兜底也失败：{http_error}"))
                }
            }
        }
    }
}

// ==================== 存储 ====================

fn store_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::config_dir(app)?.join(REMOTE_SERVERS_FILE))
}

fn load_store_from(path: &Path) -> RemoteStore {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return RemoteStore::default(), // 不存在 = 还没存过
    };
    serde_json::from_str(&content).unwrap_or_else(|error| {
        eprintln!("remote-servers.json 解析失败（按空表处理）: {error}");
        RemoteStore::default()
    })
}

fn save_store_to(path: &Path, store: &RemoteStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录: {error}"))?;
    }
    // 临时文件 + 改名：写一半被杀也不会留下半截 JSON
    let temporary = path.with_extension("json.tmp");
    let content =
        serde_json::to_string_pretty(store).map_err(|error| format!("序列化失败: {error}"))?;
    std::fs::write(&temporary, content.as_bytes())
        .map_err(|error| format!("无法写入 {temporary:?}: {error}"))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("无法落盘 {path:?}: {error}"))?;
    Ok(())
}

fn load_store(app: &AppHandle) -> Result<RemoteStore, String> {
    Ok(load_store_from(&store_path(app)?))
}

fn save_store(app: &AppHandle, store: &RemoteStore) -> Result<(), String> {
    save_store_to(&store_path(app)?, store)
}

/// 后续「启动时连接全部 / 心跳」复用（跨模块只读）
pub(crate) fn load_store_pub(app: &AppHandle) -> Result<RemoteStore, String> {
    load_store(app)
}

fn new_remote_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("rmt-{nanos:016x}")
}

// ==================== 命令 ====================

/// 「电脑」页 iframe 的本机入口：本机 AnyBoot 服务端的 http 端口与明文 token。
/// 本机标签与远端标签同一路径（`http://127.0.0.1:port/?token=…`），页面行为
/// 与手机 WebView 加载的完全一致；仅 hub 窗口本地使用，不出本机。
#[tauri::command]
pub fn local_http_endpoint(app: AppHandle) -> Result<serde_json::Value, String> {
    let settings = crate::read_settings_file(&app);
    // listening / error：本机标签要能区分「服务没开」和「开了但端口绑不上」
    // （Windows 上 12300 随时可能落进 Hyper-V/WSL 的动态保留段，此时 bind 直接 WSAEACCES）
    let (listening, addr, error) = {
        let runtime = crate::http_server::http_lock();
        let listening = runtime.server.is_some();
        let addr = runtime
            .server
            .as_ref()
            .map(|server| server.addr.clone())
            .unwrap_or_default();
        let error = runtime.last_error.clone().unwrap_or_default();
        (listening, addr, error)
    };
    Ok(serde_json::json!({
        "port": settings.http_port,
        "token": settings.http_token,
        "enabled": settings.http_enabled,
        "listening": listening,
        "addr": addr,
        "error": error,
    }))
}

/// 电脑页初始化数据：全部档案 + 全局开关。token 明文回传（iframe 拼 URL 要用，对齐手机）。
///
/// 返回 `serde_json::Value` 而不是 `RemoteStore`：要给前端补两个**算出来的**字段
/// （`effective_addr` / `effective_token`）—— 它们是「区块顺序 + 区块内顺序的第一条
/// enabled 配对」，不是配置，不能落盘。前端拿到就能直接用，不必自己实现一遍排序规则
/// （三处实现必然走样，这是老四槽时代的教训）。
#[tauri::command]
pub fn remote_list(app: AppHandle) -> Result<serde_json::Value, String> {
    let store = load_store(&app)?;
    let servers: Vec<serde_json::Value> = store
        .servers
        .iter()
        .cloned()
        // 读路径也归一一次：手改过 JSON / 老档案塞进非法条目时，前端不该看到脏数据
        .map(|server| server_json(server.normalize()))
        .collect();
    Ok(serde_json::json!({
        "connect_all": store.connect_all,
        "servers": servers,
    }))
}

/// 档案 → JSON，并补上两个计算字段（见 [`remote_list`] 的说明）
fn server_json(server: RemoteServer) -> serde_json::Value {
    let (addr, token) = (server.effective_remote_addr(), server.effective_remote_token());
    let mut value = serde_json::to_value(&server).unwrap_or(serde_json::Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("effective_addr".to_string(), serde_json::json!(addr));
        object.insert("effective_token".to_string(), serde_json::json!(token));
    }
    value
}

/// 新建 / 编辑。id 空 = 新建；否则按 id 更新（created_at 保留）。
/// 返回值带计算字段（与 [`remote_list`] 同一形态），前端保存后可直接替换本地那一行。
#[tauri::command]
pub fn remote_save(app: AppHandle, server: RemoteServer) -> Result<serde_json::Value, String> {
    let mut server = server.normalize();
    if server.addresses.is_empty() {
        return Err("至少要填一个服务端地址".to_string());
    }
    // 跳板机只存引用：必须是已存在的 SSH 服务器（转发链路后续接入，先保证引用有效）
    if !server.jump_id.is_empty() {
        let hub_servers = crate::hub::load_servers_pub(&app)?;
        match hub_servers.iter().find(|s| s.id == server.jump_id) {
            Some(hub) if hub.protocol == "ssh" => {}
            Some(_) => return Err("经由跳板机只支持 SSH 服务器".to_string()),
            None => return Err("所选跳板机不存在，请重新选择".to_string()),
        }
    }
    let mut store = load_store(&app)?;
    if server.id.is_empty() {
        server.id = new_remote_id();
        server.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        store.servers.push(server.clone());
    } else if let Some(existing) = store.servers.iter_mut().find(|s| s.id == server.id) {
        // 编辑态 token 留空 = 保持已存 token（前端不回显 token，同 hub 密码语义）；
        // 新建态空 token 合法（服务端可能没设 token）
        if server.token.is_empty() {
            server.token = existing.token.clone();
        }
        // 远程配对令牌同语义：编辑弹窗不回显令牌，某条配对留空 = 保持同 id 已存的令牌。
        // 按 id 对而不是按 (mid,kind)：用户可能刚把手填地址的 mid 补上，
        // 这时幂等键变了但确实是同一行，按 id 才对得上
        let saved: Vec<(String, String)> = existing
            .pairs
            .iter()
            .map(|pair| (pair.id.clone(), pair.token.clone()))
            .collect();
        for pair in server.pairs.iter_mut() {
            if !pair.token.is_empty() {
                continue;
            }
            if let Some((_, token)) = saved.iter().find(|(id, _)| *id == pair.id) {
                pair.token = token.clone();
            }
        }
        // 地址没变才保留探测学到的机器标识（变了可能已是另一台电脑，作废）
        let same_machine_set = existing.addresses == server.addresses;
        let machine = (existing.machine_id.clone(), existing.machine_name.clone());
        let created_at = existing.created_at;
        *existing = server.clone();
        existing.created_at = created_at;
        if same_machine_set {
            existing.machine_id = machine.0;
            existing.machine_name = machine.1;
        }
    } else {
        return Err("服务端不存在（可能已被删除）".to_string());
    }
    save_store(&app, &store)?;
    Ok(server_json(server))
}

/// 删除服务端（顺带收掉它的隧道子进程）。
/// **必须 async + spawn_blocking**：`kill_forward` 要 kill 再 wait 子进程。
#[tauri::command]
pub async fn remote_delete(app: AppHandle, id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || remote_delete_blocking(app, id))
        .await
        .map_err(|error| format!("删除服务端任务失败: {error}"))?
}

/// 把探测成功的地址提到候选列表最前并落盘（下次 iframe / 页面加载优先用它）。
/// 地址本来就在首位时是空操作。
#[tauri::command]
pub fn remote_promote_address(
    app: AppHandle,
    server_id: String,
    address: String,
) -> Result<(), String> {
    let mut store = load_store(&app)?;
    let Some(server) = store.servers.iter_mut().find(|s| s.id == server_id) else {
        return Err("服务端不存在（可能已被删除）".to_string());
    };
    let Some(index) = server.addresses.iter().position(|a| *a == address) else {
        return Err(format!("地址 {address} 不在这台服务端的候选列表里"));
    };
    if index > 0 {
        let picked = server.addresses.remove(index);
        server.addresses.insert(0, picked);
        save_store(&app, &store)?;
    }
    Ok(())
}

/// 探活成功后把机器身份记到档案上（machineId / machineName / os）。
///
/// 为什么要落盘：客户端要画「这台是 Windows / macOS / Linux」的系统徽标，
/// 每次开界面都等一轮探测才显示会闪一下。探到了就存下来，下次直接画。
/// **只有真有变化才写盘** —— 心跳每 10 秒一轮，不能每次都碰磁盘。
#[tauri::command]
pub fn remote_note_machine(
    app: AppHandle,
    server_id: String,
    machine_id: String,
    machine_name: String,
    os: String,
) -> Result<(), String> {
    let mut store = load_store(&app)?;
    let Some(server) = store.servers.iter_mut().find(|s| s.id == server_id) else {
        return Err("服务端不存在（可能已被删除）".to_string());
    };
    let probe = RemoteServer {
        machine_id: machine_id.clone(),
        machine_name: machine_name.clone(),
        machine_os: os.clone(),
        ..Default::default()
    };
    let os_normalized = probe.normalize().machine_os;
    let changed = server.machine_id != machine_id.trim()
        || server.machine_name != machine_name.trim()
        || server.machine_os != os_normalized;
    if !changed {
        return Ok(());
    }
    server.machine_id = machine_id.trim().to_string();
    server.machine_name = machine_name.trim().to_string();
    server.machine_os = os_normalized;
    save_store(&app, &store)
}

/// 一台电脑的系统信息（「系统信息」弹层的数据源）。
///
/// - `server_id` 为空 / `"local"` → 本机直调（不经 HTTP，服务没起也能看）；
/// - 其它 → 按连接策略逐通道请求对方 `/api/system`（局域网 → 跳板 → 隧道…），
///   拿到的是**那台机器**的系统（版本 / CPU / 内存 / 运行时长）。
#[tauri::command]
pub async fn remote_system_info(
    app: AppHandle,
    server_id: String,
) -> Result<crate::sysinfo_lite::SystemInfo, String> {
    let id = server_id.trim().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        if id.is_empty() || id == "local" {
            return Ok(local_system_info(&app));
        }
        let store = load_store(&app)?;
        let server = store
            .servers
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(|| "服务端不存在（可能已被删除）".to_string())?;
        let value = remote_http_json(&app, server, "/api/system")?;
        serde_json::from_value::<crate::sysinfo_lite::SystemInfo>(value)
            .map_err(|error| format!("对方返回的系统信息格式不对：{error}"))
    })
    .await
    .map_err(|error| format!("读取系统信息任务失败: {error}"))?
}

/// 本机系统信息（含真实监听端口；服务没起时端口记 0）
pub(crate) fn local_system_info(app: &AppHandle) -> crate::sysinfo_lite::SystemInfo {
    let settings = crate::stored_settings(app);
    let listening = {
        let runtime = crate::http_server::http_lock();
        runtime.server.is_some()
    };
    let port = if listening { settings.http_port } else { 0 };
    let mut ips: Vec<String> = Vec::new();
    for ip in [crate::http_server::local_ipv4()] {
        if !ip.is_empty() && !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    crate::sysinfo_lite::collect(port, ips)
}

fn remote_delete_blocking(app: AppHandle, id: String) -> Result<(), String> {    let state = app.state::<RemoteJumpState>();
    kill_forward(state.inner(), &id);
    let mut store = load_store(&app)?;
    let before = store.servers.len();
    store.servers.retain(|s| s.id != id);
    if store.servers.len() == before {
        return Err("服务端不存在（可能已被删除）".to_string());
    }
    save_store(&app, &store)
}

/// 「✓ 启动时连接全部服务端」全局开关（对齐手机 setConnectAll）
#[tauri::command]
pub fn remote_set_connect_all(app: AppHandle, on: bool) -> Result<(), String> {
    let mut store = load_store(&app)?;
    store.connect_all = on;
    save_store(&app, &store)
}

/// 探测模式归一：serial（按顺序）/ parallel_all（并发等全部）/ parallel_first
/// （并发取首个成功；旧值 parallel 与空值都算它）
pub(crate) fn normalize_probe_mode(mode: &str) -> &'static str {
    match mode.trim() {
        "serial" => "serial",
        "parallel_all" => "parallel_all",
        _ => "parallel_first",
    }
}

/// 探测一台电脑的所有候选地址。三种模式：
/// - **并发（首个）**：每个地址一个线程，**任一响应立即胜出**（done 旗标），
///   其余线程探完作废 —— 用户要的是「最快哪个能用」；
/// - **并发（全部）**：等全部线程结束，取最快的成功者，逐行结果随 results 返回；
/// - **串行**：按顺序逐个试，第一个成功就停。
/// results 里每个候选地址一条（成败 + 耗时），前端逐行展示。
/// `GET /health` 免鉴权。**必须 async + spawn_blocking**：要 join 并发线程 / 等 TCP 超时。
///
/// `remote_addrs`（可选，远程接入候选列表 = 隧道槽 + Tailscale 槽）：**局域网优先、
/// 远程兜底** —— 局域网有赢家就不探远程（results 里每槽补一行「未探测」说明）；
/// 局域网全挂才按顺序逐槽探，第一个通的远程槽就是赢家（结果带 `remote: true`）。
/// 没有任何远程候选时行为与老版本完全一致。
///
/// `probe_timeout_ms`：**默认**探测超时（通道没单独设时用它，默认 2000，
/// 见 [`clamp_probe_timeout_ms`]）。
///
/// `route_timeouts_ms`：每个通道自己的超时（key = 通道 id）。编辑弹窗里点了「测试」
/// 但还没保存时，前端把界面上那张表原样传过来，测出来的结果才和保存后一致。
///
/// `probe_mode` = **多网卡探测**（同一通道内多个地址之间）；`route_probe_mode` =
/// **连接优先级的探测方式**（六个通道之间）。两个粒度各一个参数，别混。
///
/// `block_probe_modes`：**区块内部**策略（key = 区块 id）。配对通道（tunnel / ts /
/// funnel / tsnet）的组内策略优先听它，没配才落 `probe_mode` —— 用户在前端把
/// 「ts 区块串行」调了，后端必须照做，否则那条设置等于不存在。
#[tauri::command]
pub async fn remote_test(
    addresses: Vec<String>,
    probe_mode: String,
    route_probe_mode: Option<String>,
    remote_addrs: Option<Vec<String>>,
    conn_mode: Option<String>,
    candidates: Option<Vec<RouteTestCandidate>>,
    probe_timeout_ms: Option<u64>,
    route_timeouts_ms: Option<std::collections::BTreeMap<String, u64>>,
    block_probe_modes: Option<std::collections::BTreeMap<String, String>>,
) -> RemoteTestResult {
    let default_ms = clamp_probe_timeout_ms(probe_timeout_ms.unwrap_or(0));
    let timeout = Duration::from_millis(default_ms);
    let timeouts = normalize_route_timeouts(&route_timeouts_ms.unwrap_or_default());
    let block_modes = normalize_block_probe_modes(&block_probe_modes.unwrap_or_default());
    // 主路径：按连接策略给出的显式候选（六通道，顺序 = 用户排的优先级）。
    // 外层（通道之间）听 route_probe_mode，内层（同通道多地址）听 probe_mode —— 全部通道都参与。
    if let Some(list) = candidates.filter(|items| !items.is_empty()) {
        let outer = normalize_probe_mode(route_probe_mode.as_deref().unwrap_or(""));
        let inner = normalize_probe_mode(&probe_mode);
        return tauri::async_runtime::spawn_blocking(move || {
            probe_candidates_blocking(list, outer, inner, &timeouts, default_ms, &block_modes)
        })
        .await
        .unwrap_or_else(|error| RemoteTestResult {
            error: format!("探测任务失败: {error}"),
            ..Default::default()
        });
    }
    let remote_first = conn_mode
        .as_deref()
        .map(str::trim)
        .map(|mode| mode == "remote_first")
        .unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        remote_test_blocking(addresses, probe_mode, remote_addrs, remote_first, timeout)
    })
    .await
    .unwrap_or_else(|error| RemoteTestResult {
        error: format!("探测任务失败: {error}"),
        ..Default::default()
    })
}

/// 探测超时（毫秒）的合法区间与默认值。
///
/// 默认 2 秒：局域网 RTT 通常 <10ms，2 秒足够；公网隧道首次 TLS 握手可能要 1~2 秒，
/// 觉得偏紧可以在「连接策略」里调大（上限 30 秒，防止手滑填 99999 把界面卡住）。
pub(crate) const DEFAULT_PROBE_TIMEOUT_MS: u64 = 2000;
const MIN_PROBE_TIMEOUT_MS: u64 = 200;
const MAX_PROBE_TIMEOUT_MS: u64 = 30_000;

/// 用户填的超时归一：0 / 缺省 → 默认 2 秒；越界夹到 [200ms, 30s]
pub(crate) fn clamp_probe_timeout_ms(raw: u64) -> u64 {
    if raw == 0 {
        return DEFAULT_PROBE_TIMEOUT_MS;
    }
    raw.clamp(MIN_PROBE_TIMEOUT_MS, MAX_PROBE_TIMEOUT_MS)
}

/// 每通道超时表的清洗：非法通道 id 丢掉；值 0 视作「没单独设」直接删（回落默认）；
/// 越界值夹到 [200ms, 30s]（宁可替用户改，也别让一个手滑的 999999 卡住界面）。
pub(crate) fn normalize_route_timeouts(
    raw: &std::collections::BTreeMap<String, u64>,
) -> std::collections::BTreeMap<String, u64> {
    let mut out = std::collections::BTreeMap::new();
    for (id, value) in raw {
        let key = id.trim().to_lowercase();
        if !ROUTE_IDS.contains(&key.as_str()) || *value == 0 {
            continue;
        }
        out.insert(key, clamp_probe_timeout_ms(*value));
    }
    out
}

fn serde_default_probe_timeout_ms() -> u64 {
    DEFAULT_PROBE_TIMEOUT_MS
}

/// 区块内部策略表的清洗：key 只留三个区块 id（通道 id 传进来也没用，直接丢），
/// 值归一到三种合法取值（非法值当没设，回落外层/多网卡策略）。
///
/// 与 `route_timeouts_ms`（通道 id）/ `block_timeouts_ms`（区块 id）分属三个 key 空间，
/// 别混在一张表里 —— 同一个字符串在两套空间里含义不同。
fn normalize_block_probe_modes(
    raw: &std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for (id, value) in raw {
        let key = id.trim().to_lowercase();
        if !BLOCK_IDS.contains(&key.as_str()) {
            continue;
        }
        // 只有三个合法取值才算「设了」；空串 / 拼错的值一律当没设（回落多网卡策略），
        // 别把拼错的值悄悄变成 parallel_first 存进去
        let text = value.trim();
        if text != "parallel_first" && text != "parallel_all" && text != "serial" {
            continue;
        }
        out.insert(key, text.to_string());
    }
    out
}

/// 配对列表归一：逐条 trim / 判型 / 挖 token / 补 id → 按幂等键去重（**后来的覆盖前面的**，
/// 也就是「同一系统同一区块重新配对 = 更新那一行」）→ 截到软上限。
///
/// 去重时保留**先出现的位置**而不是把更新项挪到末尾：用户排好的区块内顺序不该因为
/// 重扫一次码就被打乱。
fn normalize_pairs(raw: Vec<RemotePair>) -> Vec<RemotePair> {
    let mut out: Vec<RemotePair> = Vec::new();
    let mut index_of: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for pair in raw {
        // kind 非法/为空时按地址形态兜底判型：100.x 与 *.ts.net 都归 ts 区块，
        // 其余（trycloudflare / 自建域名）归 cloudflare。不猜 tsnet —— 内置引擎的域名
        // 与系统 Funnel 同形，猜错了会把系统那条塞进内置区块，只能由配对串显式指定
        let fallback = if is_tailscale_origin(&pair.addr) {
            "ts"
        } else {
            "cloudflare"
        };
        let Some(pair) = pair.normalize(fallback) else {
            continue;
        };
        let key = pair_key(&pair);
        match index_of.get(&key) {
            Some(&index) => {
                // 更新：保留原 id（探测结果还能对上）与原位置
                let kept_id = out[index].id.clone();
                out[index] = RemotePair {
                    id: kept_id,
                    created_at: if out[index].created_at != 0 {
                        out[index].created_at
                    } else {
                        pair.created_at
                    },
                    ..pair
                };
            }
            None => {
                index_of.insert(key, out.len());
                out.push(pair);
            }
        }
        if out.len() >= MAX_PAIRS {
            break;
        }
    }
    out
}

fn serde_default_route_probe_mode() -> String {
    "parallel_first".to_string()
}

/// 一次探测的原始结果（新引擎内部用；比 ProbeOutcome 多带 machineId/名/系统）
#[derive(Clone, Debug)]
struct ProbeHit {
    kind: String,
    origin: String,
    /// 配对 id（前端按它回填到具体那一行）；lan / jump 为空
    pair_id: String,
    remote: bool,
    outcome: Result<ProbePayload, String>,
}

/// 探活拿到的机器信息
#[derive(Clone, Debug, Default)]
struct ProbePayload {
    ms: u64,
    machine_id: String,
    machine_name: String,
    os: String,
}

/// 探一个候选（在探测线程池里跑）：lan 走手写 TCP，远程走 ureq（TLS）
fn probe_one(kind: &str, origin: &str, timeout: Duration) -> Result<ProbePayload, String> {
    let remote = matches!(kind, "tunnel" | "ts" | "funnel" | "tsnet");
    if !remote && kind != "lan" {
        // jump 需要 AppHandle 解析 SSH 转发端口，探测线程池里没有 —— 交给使用方
        return Err("此通道请用「经跳板机测试」（先保存档案）".to_string());
    }
    let (ms, machine_id, machine_name, os) = if remote {
        let (origin, _) = split_remote_address(origin);
        if origin.is_empty() {
            return Err("地址为空".to_string());
        }
        probe_remote_health_detail(&origin, timeout)?
    } else {
        probe_health_detail(origin, timeout)?
    };
    Ok(ProbePayload {
        ms,
        machine_id,
        machine_name,
        os,
    })
}

/// **按连接策略探测**（电脑页 / 电脑列表 / 编辑弹窗「测试」共用）。**三级策略**：
///
/// - 外层 `route_mode`：**连接优先级整体的探测方式**，管「六个通道之间」怎么试 ——
///   `parallel_first`（全部通道同时发出，谁先成功用谁）/ `parallel_all`（等全部返回，
///   逐行报每个通道）/ `serial`（按优先级逐个通道试，第一个通就停）；
/// - 区块内层 `block_modes`：**区块内部策略**（key = cloudflare / ts / tsnet），管「同一
///   区块里的多条配对之间」怎么试；没配就落 `address_mode`；
/// - `address_mode`：**多网卡探测**，管「同一个通道里的多个地址」（实际只有局域网
///   会配多个）—— 同样是上面三种取值。
///
/// 三层是独立开关（用户在编辑框里分别选），组合起来的意思很清楚：
/// 「通道之间并发、每个区块内部按顺序试」这种搭配是合法且常用的。
///
/// 所有探测都在 [`crate::probe_pool`] 里跑（专用固定线程池），不再占用 Tauri 的
/// 全局阻塞池 —— 用户把超时调大 + 并发全部时，界面其它命令不会被拖住。
fn probe_candidates_blocking(
    candidates: Vec<RouteTestCandidate>,
    route_mode: &str,
    address_mode: &str,
    timeouts: &std::collections::BTreeMap<String, u64>,
    default_timeout_ms: u64,
    block_modes: &std::collections::BTreeMap<String, String>,
) -> RemoteTestResult {
    let list: Vec<RouteTestCandidate> = candidates
        .into_iter()
        .map(|cand| RouteTestCandidate {
            kind: cand.kind.trim().to_lowercase(),
            origin: cand.origin.trim().to_string(),
            // 条目自带超时跟着走（配对算好的三层继承结果），没带就落通道表/全局默认
            timeout_ms: cand.timeout_ms,
            pair_id: cand.pair_id.trim().to_string(),
        })
        .filter(|cand| !cand.kind.is_empty() && !cand.origin.is_empty())
        .collect();
    if list.is_empty() {
        return RemoteTestResult {
            error: "没有可探测的通道（局域网地址为空，或所有通道都被禁用/不可用）".to_string(),
            ..Default::default()
        };
    }

    // 每条候选的实际超时在这里一次算清：**条目自带 → 通道表 → 全局默认**（三层继承）。
    // 配对已经把「条目 → 区块 → 全局」算好塞进 timeout_ms，局域网候选则由通道表决定。
    let list: Vec<RouteTestCandidate> = list
        .into_iter()
        .map(|mut cand| {
            if cand.timeout_ms == 0 {
                cand.timeout_ms = timeouts
                    .get(&cand.kind)
                    .copied()
                    .filter(|value| *value > 0)
                    .unwrap_or(default_timeout_ms);
            }
            cand.timeout_ms = clamp_probe_timeout_ms(cand.timeout_ms);
            cand
        })
        .collect();
    let groups = group_candidates(&list, block_modes, address_mode);
    let route_mode = normalize_probe_mode(route_mode);
    // 兜底时限：按**最坏路径**估（内层按各区块自己的策略算，外层串行就累加、并发就取最大），
    // 每个探活自己已有 IO 超时，这里只防线程池排队，宽一点不会误杀
    let group_worst = |group: &ProbeGroup| {
        let secs: Vec<Duration> = group.items.iter()
            .map(|item| Duration::from_millis(item.timeout_ms))
            .collect();
        if group.inner_mode == "serial" {
            secs.iter().copied().sum()
        } else {
            secs.iter().copied().max().unwrap_or_default()
        }
    };
    let worst = if route_mode == "serial" {
        groups.iter().map(group_worst).sum::<Duration>()
    } else {
        groups
            .iter()
            .map(group_worst)
            .max()
            .unwrap_or_else(|| Duration::from_millis(clamp_probe_timeout_ms(default_timeout_ms)))
    };

    if route_mode == "serial" {
        // 外层串行：一个通道一个通道地试，某个通道命中就不再往下走
        let mut hits: Vec<ProbeHit> = Vec::new();
        for group in &groups {
            let group_hits = probe_group_blocking(group);
            let ok = group_hits.iter().any(|hit| hit.outcome.is_ok());
            hits.extend(group_hits);
            if ok {
                break;
            }
        }
        // 没轮到的通道如实标「没探」，别让用户以为漏了（逐行结果要和优先级表对得上）
        for group in &groups {
            for item in &group.items {
                if hits.iter().any(|hit| hit.kind == group.kind && hit.origin == item.origin) {
                    continue;
                }
                hits.push(ProbeHit {
                    kind: group.kind.clone(),
                    origin: item.origin.clone(),
                    pair_id: item.pair_id.clone(),
                    remote: is_remote_kind(&group.kind),
                    outcome: Err("前面已有通道命中，按串行策略未探测".to_string()),
                });
            }
        }
        return finish_probe(hits);
    }

    // 外层并发：一个通道一个任务，组内再按内层策略处理它的多个地址
    let (sender, receiver) = std::sync::mpsc::channel::<Vec<ProbeHit>>();
    let mut pending = groups.len();
    for group in &groups {
        let sender = sender.clone();
        let kind = group.kind.clone();
        let items = group.items.clone();
        let inner = group.inner_mode.clone();
        crate::probe_pool::pool().execute(Box::new(move || {
            let group = ProbeGroup { kind, inner_mode: inner, items };
            let hits = probe_group_blocking(&group);
            // 接收端可能已经不要了（并发首个已经胜出）——发送失败静默丢弃
            let _ = sender.send(hits);
        }));
    }
    drop(sender);

    // 整轮硬顶 30 秒：超时下放到条目后最坏耗时是「乘」出来的（区块间串行 × 区块内串行），
    // 3 区块 × 4 条 × 3s = 36 秒，界面会像卡死 —— 到点就把剩下的标「已跳过」
    let deadline = Instant::now()
        + (worst + Duration::from_millis(500)).min(Duration::from_millis(MAX_PROBE_TIMEOUT_MS));
    let mut collected: Vec<ProbeHit> = Vec::new();
    while pending > 0 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match receiver.recv_timeout(remaining) {
            Ok(group_hits) => {
                pending -= 1;
                let ok = group_hits.iter().any(|hit| hit.outcome.is_ok());
                collected.extend(group_hits);
                // 并发（首个）：第一个成功的通道立刻收工，剩下的在途任务结果无人接收
                if ok && route_mode == "parallel_first" {
                    break;
                }
            }
            Err(_) => break, // 超时：把已拿到的收束掉
        }
    }
    // 没回来的通道 / 地址如实标出来（带上它自己的超时值），别让用户以为漏了通道
    for group in &groups {
        for item in &group.items {
            if collected
                .iter()
                .any(|hit| hit.kind == group.kind && hit.origin == item.origin)
            {
                continue;
            }
            collected.push(ProbeHit {
                kind: group.kind.clone(),
                origin: item.origin.clone(),
                pair_id: item.pair_id.clone(),
                remote: is_remote_kind(&group.kind),
                outcome: Err(format!("探测超时（>{}ms）", item.timeout_ms)),
            });
        }
    }
    finish_probe(collected)
}

/// 一个通道的候选项分组：同一个 kind 的多条归一组（配对多条目之后，远程通道也会有多条）
struct ProbeGroup {
    kind: String,
    /// 组内策略：配对通道听 `block_probe_mode[区块]`，没配（或 lan/jump）落多网卡策略。
    /// 在这一层定好，后面每个探测任务只认自己的那一份 —— 免得一个全局变量管所有区块。
    inner_mode: String,
    items: Vec<RouteTestCandidate>,
}

/// 按 kind 分组，**保持候选出现顺序**（组顺序 = 用户在优先级表里排的顺序；
/// 组内顺序 = 配对在区块里的顺序）。同 kind 同 origin 只留第一条。
///
/// 组内策略在这里定：`block_probe_mode` 是**区块**粒度的设置，而分组是**通道**粒度的，
/// 所以要按 `block_of_route(kind)` 回查 —— ts 区块配的「串行」对 ts 与 funnel 两个
/// 通道组都生效（它们本来就是同一区块）。
fn group_candidates(
    list: &[RouteTestCandidate],
    block_modes: &std::collections::BTreeMap<String, String>,
    address_mode: &str,
) -> Vec<ProbeGroup> {
    let mut groups: Vec<ProbeGroup> = Vec::new();
    for cand in list {
        match groups.iter_mut().find(|group| group.kind == cand.kind) {
            Some(group) => {
                if !group.items.iter().any(|item| item.origin == cand.origin) {
                    group.items.push(cand.clone());
                }
            }
            None => {
                let inner = block_of_route(&cand.kind)
                    .and_then(|block| block_modes.get(block).cloned())
                    .unwrap_or_else(|| address_mode.to_string());
                groups.push(ProbeGroup {
                    kind: cand.kind.clone(),
                    inner_mode: normalize_probe_mode(&inner).to_string(),
                    items: vec![cand.clone()],
                });
            }
        }
    }
    groups
}

/// 探一个通道（组内多条按**这条通道所在区块的策略**处理），返回按 items 顺序排列的命中列表。
///
/// **每条用自己的超时**：同一个区块里 `100.x` 直连几十毫秒就回、`*.ts.net` 走中继
/// 可能要好几秒，共用一个超时必然一边误判失败一边白等。
/// 未探测的条目也会补一行（「同通道已命中/未探测」），这样逐行结果永远和配对列表对得上。
fn probe_group_blocking(group: &ProbeGroup) -> Vec<ProbeHit> {
    let remote = is_remote_kind(&group.kind);
    let single = |item: &RouteTestCandidate, outcome: Result<ProbePayload, String>| ProbeHit {
        kind: group.kind.clone(),
        origin: item.origin.clone(),
        pair_id: item.pair_id.clone(),
        remote,
        outcome,
    };
    let timeout_of = |item: &RouteTestCandidate| Duration::from_millis(item.timeout_ms);
    if group.items.len() == 1 {
        let item = &group.items[0];
        return vec![single(item, probe_one(&group.kind, &item.origin, timeout_of(item)))];
    }
    // 组内策略在分组时就定好了（见 `group_candidates`）：配对通道听区块设置，
    // lan / jump 听多网卡设置
    let mode = group.inner_mode.as_str();
    let mut hits: Vec<ProbeHit> = Vec::with_capacity(group.items.len());
    if mode == "serial" {
        // 内层串行：按用户排的顺序逐个试，第一个通就停
        for item in &group.items {
            let outcome = probe_one(&group.kind, &item.origin, timeout_of(item));
            let ok = outcome.is_ok();
            hits.push(single(item, outcome));
            if ok {
                break;
            }
        }
        // 命中后剩下的条目补一行说明
        for item in group.items.iter().skip(hits.len()) {
            hits.push(single(item, Err("同通道前面已命中，未探测".to_string())));
        }
        return hits;
    }
    // 内层并发：全部同时发出；parallel_first 谁先成功用谁，parallel_all 等全部
    let (sender, receiver) = std::sync::mpsc::channel::<(usize, ProbeHit)>();
    let mut pending = group.items.len();
    for (index, item) in group.items.iter().enumerate() {
        let sender = sender.clone();
        let kind = group.kind.clone();
        let origin = item.origin.clone();
        let pair_id = item.pair_id.clone();
        let timeout = timeout_of(item);
        crate::probe_pool::pool().execute(Box::new(move || {
            let outcome = probe_one(&kind, &origin, timeout);
            let _ = sender.send((
                index,
                ProbeHit {
                    kind,
                    origin,
                    pair_id,
                    remote,
                    outcome,
                },
            ));
        }));
    }
    drop(sender);
    let mut slots: Vec<Option<ProbeHit>> = (0..group.items.len()).map(|_| None).collect();
    // 兜底时限按组内**最长**那条算（各自的 IO 超时已在 probe_one 里生效）
    let longest = group
        .items
        .iter()
        .map(|item| item.timeout_ms)
        .max()
        .unwrap_or(0);
    // 组内兜底同样夹 30 秒（条目的 IO 超时已各自生效，这里只防线程池排队）
    let deadline = Instant::now()
        + (Duration::from_millis(longest) + Duration::from_millis(500))
            .min(Duration::from_millis(MAX_PROBE_TIMEOUT_MS));
    while pending > 0 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match receiver.recv_timeout(remaining) {
            Ok((index, hit)) => {
                pending -= 1;
                let ok = hit.outcome.is_ok();
                slots[index] = Some(hit);
                if ok && mode == "parallel_first" {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    for (index, slot) in slots.into_iter().enumerate() {
        hits.push(slot.unwrap_or_else(|| {
            single(&group.items[index], Err("同通道已命中，未再等待".to_string()))
        }));
    }
    hits
}

fn is_remote_kind(kind: &str) -> bool {
    matches!(kind, "tunnel" | "ts" | "funnel" | "tsnet")
}

/// 把原始命中结果收束成 `RemoteTestResult`：赢家 = 并发全部里最快的成功者；
/// 逐行结果按**候选原顺序**排（用户对着优先级表看，别让顺序乱跳）。
fn finish_probe(mut hits: Vec<ProbeHit>) -> RemoteTestResult {
    // 赢家：先看有没有成功的，取延迟最低的（serial / parallel_first 只会有一个成功）
    let mut winner: Option<(usize, ProbePayload)> = None;
    for (index, hit) in hits.iter().enumerate() {
        if let Ok(payload) = &hit.outcome {
            let better = winner
                .as_ref()
                .map(|(_, current)| payload.ms < current.ms)
                .unwrap_or(true);
            if better {
                winner = Some((index, payload.clone()));
            }
        }
    }
    let results: Vec<ProbeOutcome> = hits
        .drain(..)
        .map(|hit| match hit.outcome {
            Ok(payload) => ProbeOutcome {
                address: hit.origin,
                ok: true,
                ms: payload.ms,
                error: String::new(),
                remote: hit.remote,
                kind: hit.kind,
                pair_id: hit.pair_id,
            },
            Err(error) => ProbeOutcome {
                address: hit.origin,
                ok: false,
                ms: 0,
                error,
                remote: hit.remote,
                kind: hit.kind,
                pair_id: hit.pair_id,
            },
        })
        .collect();
    match winner {
        Some((index, payload)) => {
            let row = &results[index];
            RemoteTestResult {
                ok: true,
                addr: row.address.clone(),
                ms: payload.ms,
                machine_id: payload.machine_id,
                machine_name: payload.machine_name,
                os: payload.os,
                remote: row.remote,
                kind: row.kind.clone(),
                results,
                ..Default::default()
            }
        }
        None => {
            let last_error = results
                .iter()
                .map(|row| row.error.clone())
                .find(|error| !error.is_empty())
                .unwrap_or_else(|| "全部通道都连不上".to_string());
            RemoteTestResult {
                error: format!("所有通道都不通：{last_error}"),
                results,
                ..Default::default()
            }
        }
    }
}

fn remote_test_blocking(
    addresses: Vec<String>,
    probe_mode: String,
    remote_addrs: Option<Vec<String>>,
    remote_first: bool,
    timeout: Duration,
) -> RemoteTestResult {
    let mode = normalize_probe_mode(&probe_mode);
    // 远程候选归一：每条 split_remote_address（兼容粘成配对串/带 ab_token 的 URL），
    // 去重保序（隧道在前、Tailscale 在后 —— 调用方负责顺序）
    let remote_origins: Vec<String> = remote_addrs
        .unwrap_or_default()
        .iter()
        .filter_map(|raw| {
            let (origin, _) = split_remote_address(raw);
            if origin.is_empty() { None } else { Some(origin) }
        })
        .collect();
    let remote_origin = remote_origins.first().cloned();
    let candidates: Vec<String> = addresses
        .iter()
        .filter_map(|raw| {
            let (address, _) = split_address(raw);
            if address.is_empty() {
                None
            } else {
                Some(address)
            }
        })
        .collect();
    if candidates.is_empty() && remote_origin.is_none() {
        return RemoteTestResult {
            error: "没有可探测的地址".to_string(),
            ..Default::default()
        };
    }

    // 远程优先：按双槽候选逐个探，谁先通用谁直接用（局域网整轮不探，省 3 秒级等待）
    let mut remote_first_errors: Vec<(String, String)> = Vec::new(); // (origin, error)
    if remote_first {
        for origin in &remote_origins {
            match probe_remote_health(origin, timeout) {
                Ok((ms, machine_id, machine_name)) => {
                    let mut rows: Vec<ProbeOutcome> = candidates
                        .iter()
                        .map(|addr| ProbeOutcome {
                            address: addr.clone(),
                            ok: false,
                            ms: 0,
                            error: "远程优先，局域网未探测".to_string(),
                            remote: false,
                            kind: "lan".to_string(),
                            pair_id: String::new(),
                        })
                        .collect();
                    // 排在赢家前面的远程候选都失败过：如实逐行补失败原因
                    for (failed_origin, error) in &remote_first_errors {
                        rows.push(ProbeOutcome {
                            address: failed_origin.clone(),
                            ok: false,
                            ms: 0,
                            error: error.clone(),
                            remote: true,
                            kind: String::new(),
                            pair_id: String::new(),
                        });
                    }
                    rows.push(ProbeOutcome {
                        address: origin.clone(),
                        ok: true,
                        ms,
                        error: String::new(),
                        remote: true,
                            kind: String::new(),
                            pair_id: String::new(),
                    });
                    return RemoteTestResult {
                        ok: true,
                        addr: origin.clone(),
                        ms,
                        machine_id,
                        machine_name,
                        remote: true,
                        results: rows,
                        ..Default::default()
                    };
                }
                Err(remote_error) => {
                    // 这个远程槽没通：记下来，试下一个槽；全挂退局域网链路
                    remote_first_errors.push((origin.clone(), remote_error));
                }
            }
        }
    }

    let mut results: Vec<ProbeOutcome> = Vec::new();
    let mut best: Option<RemoteTestResult> = None;
    if mode == "serial" {
        // 串行：按顺序逐个试，第一个成功就停（剩下的地址不探）
        for addr in &candidates {
            match probe_health(addr, timeout) {
                Ok((ms, machine_id, machine_name)) => {
                    results.push(ProbeOutcome {
                        address: addr.clone(),
                        ok: true,
                        ms,
                        error: String::new(),
                        remote: false,
                            kind: "lan".to_string(),
                            pair_id: String::new(),
                    });
                    best = Some(RemoteTestResult {
                        ok: true,
                        addr: addr.clone(),
                        ms,
                        machine_id,
                        machine_name,
                        ..Default::default()
                    });
                    break;
                }
                Err(error) => results.push(ProbeOutcome {
                    address: addr.clone(),
                    ok: false,
                    ms: 0,
                    error,
                    remote: false,
                            kind: "lan".to_string(),
                            pair_id: String::new(),
                }),
            }
        }
    } else {
        // 并发：每个地址一个线程。首个 = 谁先成功谁赢（done 旗标，后到的成功作废）；
        // 全部 = 等所有线程结束，取最快的成功者。
        let first_only = mode == "parallel_first";
        let done = Arc::new(AtomicBool::new(false));
        let winner: Arc<Mutex<Option<RemoteTestResult>>> = Arc::new(Mutex::new(None));
        let outcomes: Arc<Mutex<Vec<ProbeOutcome>>> = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for addr in &candidates {
            let addr = addr.clone();
            let (done, winner, outcomes) =
                (Arc::clone(&done), Arc::clone(&winner), Arc::clone(&outcomes));
            handles.push(std::thread::spawn(move || {
                if first_only && done.load(Ordering::Relaxed) {
                    return; // 已经有赢家：不再白探（在途的等它自己超时收尾）
                }
                let outcome = match probe_health(&addr, timeout) {
                    Ok((ms, machine_id, machine_name)) => {
                        if first_only {
                            if !done.swap(true, Ordering::SeqCst) {
                                *winner.lock().unwrap() = Some(RemoteTestResult {
                                    ok: true,
                                    addr: addr.clone(),
                                    ms,
                                    machine_id,
                                    machine_name,
                                    ..Default::default()
                                });
                            }
                        } else {
                            let mut guard = winner.lock().unwrap();
                            let better = match guard.as_ref() {
                                Some(current) => ms < current.ms,
                                None => true,
                            };
                            if better {
                                *guard = Some(RemoteTestResult {
                                    ok: true,
                                    addr: addr.clone(),
                                    ms,
                                    machine_id,
                                    machine_name,
                                    ..Default::default()
                                });
                            }
                        }
                        ProbeOutcome {
                            address: addr.clone(),
                            ok: true,
                            ms,
                            error: String::new(),
                            remote: false,
                            kind: "lan".to_string(),
                            pair_id: String::new(),
                        }
                    }
                    Err(error) => ProbeOutcome {
                        address: addr.clone(),
                        ok: false,
                        ms: 0,
                        error,
                        remote: false,
                            kind: "lan".to_string(),
                            pair_id: String::new(),
                    },
                };
                outcomes.lock().unwrap().push(outcome);
            }));
        }
        for handle in handles {
            let _ = handle.join();
        }
        results = outcomes.lock().unwrap().clone();
        // 按候选顺序排回，方便用户逐行对照地址表
        results.sort_by_key(|o| {
            candidates
                .iter()
                .position(|a| *a == o.address)
                .unwrap_or(usize::MAX)
        });
        best = winner.lock().unwrap().clone();
    }
    let mut final_result = best.unwrap_or(RemoteTestResult {
        error: results
            .iter()
            .find(|o| !o.error.is_empty())
            .map(|o| o.error.clone())
            .unwrap_or_else(|| "全部地址都连不上".to_string()),
        ..Default::default()
    });
    // 局域网有赢家：远程不探（省一次 8s 级公网等待），results 里每槽补一行说明
    if !remote_origins.is_empty() {
        if final_result.ok {
            if !remote_first_errors.is_empty() {
                // 远程优先没走通、局域网救场成功：如实逐行补远程失败
                for (failed_origin, error) in &remote_first_errors {
                    results.push(ProbeOutcome {
                        address: failed_origin.clone(),
                        ok: false,
                        ms: 0,
                        error: error.clone(),
                        remote: true,
                            kind: String::new(),
                            pair_id: String::new(),
                    });
                }
            } else {
                for origin in &remote_origins {
                    results.push(ProbeOutcome {
                        address: origin.clone(),
                        ok: false,
                        ms: 0,
                        error: "局域网可用，远程地址未探测".to_string(),
                        remote: true,
                            kind: String::new(),
                            pair_id: String::new(),
                    });
                }
            }
        } else if !remote_first_errors.is_empty() {
            // 双失败：远程在上面已探过，不再重复打 8 秒级公网，把原因补进逐行结果
            for (failed_origin, error) in &remote_first_errors {
                results.push(ProbeOutcome {
                    address: failed_origin.clone(),
                    ok: false,
                    ms: 0,
                    error: error.clone(),
                    remote: true,
                            kind: String::new(),
                            pair_id: String::new(),
                });
            }
        } else {
            // 局域网全挂：按双槽顺序逐个兜底，第一个通的远程槽就是赢家
            for origin in &remote_origins {
                match probe_remote_health(origin, timeout) {
                    Ok((ms, machine_id, machine_name)) => {
                        results.push(ProbeOutcome {
                            address: origin.clone(),
                            ok: true,
                            ms,
                            error: String::new(),
                            remote: true,
                            kind: String::new(),
                            pair_id: String::new(),
                        });
                        final_result = RemoteTestResult {
                            ok: true,
                            addr: origin.clone(),
                            ms,
                            machine_id,
                            machine_name,
                            remote: true,
                            ..Default::default()
                        };
                        break;
                    }
                    Err(error) => results.push(ProbeOutcome {
                        address: origin.clone(),
                        ok: false,
                        ms: 0,
                        error,
                        remote: true,
                            kind: String::new(),
                            pair_id: String::new(),
                    }),
                }
            }
        }
    }
    // 逐行结果无条件带回：前端并发（全部）模式要逐行展示每个地址的成败
    final_result.results = results;
    final_result
}

// ==================== 经跳板机连接远端 AnyBoot ====================

/// 远端端点：直连 = 目标地址原样；经跳板 = 复用跳板机 ControlMaster 的本地转发端口。
#[derive(Clone, Debug, Serialize)]
pub struct RemoteEndpoint {
    pub host: String,
    pub port: u16,
    pub via_jump: bool,
}

/// 经跳板机的本地转发子进程（按远端档案 id 记，应用退出统一收掉）。
pub struct JumpForward {
    pub child: std::process::Child,
    pub local_port: u16,
    pub target: String,
}

/// 经跳板转发进程表
#[derive(Default)]
pub struct RemoteJumpState {
    pub forwards: std::sync::Mutex<std::collections::HashMap<String, JumpForward>>,
}

/// 地址 → 转发目标 (host, port)：URL / host:port / 裸 host 都吃，缺省端口 12300。
fn remote_target(raw: &str) -> (String, u16) {
    let (host, port) = split_address(raw);
    if let Some(p) = port.and_then(|x| x.parse::<u16>().ok()) {
        return (host, p);
    }
    // 非 URL 且自带端口（split_address 对 host:port 原样返回）
    if let Some(idx) = host.rfind(':') {
        if let Ok(p) = host[idx + 1..].parse::<u16>() {
            return (host[..idx].to_string(), p);
        }
    }
    (host, DEFAULT_PORT)
}

/// `-L` 转发参数（纯函数，单测）
fn remote_forward_arg(local_port: u16, host: &str, port: u16) -> String {
    format!("127.0.0.1:{local_port}:{host}:{port}")
}

fn kill_forward(state: &RemoteJumpState, id: &str) {
    if let Ok(mut map) = state.forwards.lock() {
        if let Some(mut f) = map.remove(id) {
            let _ = f.child.kill();
            let _ = f.child.wait();
        }
    }
}

/// 端点解析（命令体）：直连返回首个地址；经跳板确保本地转发活着并返回 127.0.0.1:port。
fn remote_endpoint_inner(
    app: &AppHandle,
    state: &RemoteJumpState,
    server_id: &str,
) -> Result<RemoteEndpoint, String> {
    let store = load_store(app)?;
    let server = store
        .servers
        .iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| format!("服务端不存在（{server_id}）"))?
        .clone();
    let raw = server
        .addresses
        .iter()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .ok_or_else(|| "该服务端没有可连的地址".to_string())?
        .to_string();
    let (host, port) = remote_target(&raw);
    if server.jump_id.is_empty() {
        return Ok(RemoteEndpoint { host, port, via_jump: false });
    }

    // 已有活转发且目标一致 → 直接复用（地址改了就走重建）
    let target = remote_forward_arg(0, &host, port);
    if let Ok(mut map) = state.forwards.lock() {
        if let Some(f) = map.get_mut(server_id) {
            if f.target == target && f.child.try_wait().ok().flatten().is_none() {
                return Ok(RemoteEndpoint {
                    host: "127.0.0.1".into(),
                    port: f.local_port,
                    via_jump: true,
                });
            }
        }
    }
    kill_forward(state, server_id);

    let jump = crate::hub::load_servers_pub(app)?
        .into_iter()
        .find(|s| s.id == server.jump_id)
        .ok_or_else(|| "跳板机已被删除，请重新选择".to_string())?;
    if jump.protocol != "ssh" {
        return Err("跳板机必须是 SSH 服务器".to_string());
    }
    // 找一个空闲本地端口
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("拿不到空闲本地端口: {e}"))?;
    let local_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);

    let jump_user = if jump.username.is_empty() {
        String::new()
    } else {
        format!("{}@", jump.username)
    };
    let mut command = crate::command("ssh");
    command
        .arg("-N")
        .arg("-T")
        .arg("-o")
        .arg("ExitOnForwardFailure=yes");
    // 认证统一走 apply_auth：Unix 复用 ControlMaster（终端建过就直接用），
    // Windows 没有多路复用，改由每次自己认证（密钥 / askpass）。
    crate::hub_tunnel::apply_auth(
        app,
        &jump,
        &mut command,
        "跳板机还没有保存凭据，无法自动建立转发：请改用密钥或保存密码",
    )?;
    command
        .arg("-L")
        .arg(remote_forward_arg(local_port, &host, port))
        .arg("-p")
        .arg(jump.effective_port().to_string())
        .arg(format!("{jump_user}{}", jump.host));
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 ssh: {error}"))?;
    // 1.2s 内就退出 = 转发没建立（跳板连接断了等）
    std::thread::sleep(Duration::from_millis(1200));
    if let Ok(Some(status)) = child.try_wait() {
        let _ = child.wait();
        return Err(format!("跳板转发建立失败（exit {status}）"));
    }
    if let Ok(mut map) = state.forwards.lock() {
        map.insert(
            server_id.to_string(),
            JumpForward { child, local_port, target },
        );
    }
    Ok(RemoteEndpoint { host: "127.0.0.1".into(), port: local_port, via_jump: true })
}

/// 电脑页 iframe / 健康检查要连的端点。
/// **必须 async + spawn_blocking**：没现成转发时要起 ssh 子进程并等它就绪。
#[tauri::command]
pub async fn remote_endpoint(app: AppHandle, server_id: String) -> Result<RemoteEndpoint, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<RemoteJumpState>();
        remote_endpoint_inner(&app, state.inner(), &server_id)
    })
    .await
    .map_err(|error| format!("获取端点任务失败: {error}"))?
}

/// 经跳板机探测：先确保本地转发，再对 127.0.0.1:port 做 /health 探测。
#[tauri::command]
pub async fn remote_test_via(app: AppHandle, server_id: String) -> RemoteTestResult {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<RemoteJumpState>();
        remote_test_via_blocking(&app, state.inner(), &server_id)
    })
    .await
    .unwrap_or_else(|error| RemoteTestResult {
        error: format!("探测任务失败: {error}"),
        ..Default::default()
    })
}

fn remote_test_via_blocking(
    app: &AppHandle,
    state: &RemoteJumpState,
    server_id: &str,
) -> RemoteTestResult {
    let endpoint = match remote_endpoint_inner(app, state, server_id) {
        Ok(ep) => ep,
        Err(error) => {
            return RemoteTestResult {
                error,
                ..Default::default()
            }
        }
    };
    // 经跳板机的转发端点在本机 127.0.0.1：局域网级 RTT；超时仍尊重档案里
    // 「跳板机」通道自己设的值（没设就是默认 2 秒），别再写死 3 秒
    let timeout = load_store(app)
        .ok()
        .and_then(|store| {
            store
                .servers
                .iter()
                .find(|item| item.id == server_id)
                .map(|server| server.probe_timeout_for("jump"))
        })
        .unwrap_or(PROBE_TIMEOUT);
    match probe_health(&format!("{}:{}", endpoint.host, endpoint.port), timeout) {
        Ok((ms, machine_id, machine_name)) => RemoteTestResult {
            ok: true,
            addr: format!("127.0.0.1:{}（经跳板机）", endpoint.port),
            ms,
            machine_id,
            machine_name,
            ..Default::default()
        },
        Err(error) => RemoteTestResult {
            error,
            ..Default::default()
        },
    }
}

// ==================== 引导项（rEFInd / GRUB 远程读取与切换） ====================

/// 一个可选的引导项。`value` 是回传给服务端的标识：
/// rEFInd 用 `system`（linux/windows/macos），GRUB 用 `selector`（可能含空格/`>`）。
#[derive(Clone, Debug, Serialize, Default, PartialEq)]
pub struct BootOption {
    pub value: String,
    pub label: String,
    /// 图标分类：mac / win / linux / auto（前端取色点用）
    pub class: String,
    /// 组头行（`value` 恒空、不可选）：GRUB 下拉的「▸ 组名（N 项…）」，
    /// 与托盘菜单的组头同一规则 —— 前端渲染成小标题而不是选项。
    #[serde(default)]
    pub header: bool,
}

/// 一台电脑上的一种引导管理器（rEFInd 或 GRUB）的现状 + 可选项。
#[derive(Clone, Debug, Serialize, Default, PartialEq)]
pub struct BootGroup {
    /// refind / grub
    pub kind: String,
    /// 界面上显示的名字：rEFInd / GRUB
    pub title: String,
    /// 这台电脑上是否真的部署了它（false = 显示「未检测到」）
    pub available: bool,
    /// 下次启动会进哪个（rEFInd = active 的 system；GRUB = nextEntry，缺省回 savedEntry）
    pub current: String,
    /// 下次启动的值（GRUB 独有：与入口菜单里那个「已保存」区分开，用来判断待生效）
    pub next: String,
    /// 当前生效/上次生效的值
    pub saved: String,
    /// 不可用或读取失败时的说明（前端挂在 title 上）
    pub note: String,
    pub options: Vec<BootOption>,
}

/// 电脑页「引导项」列的数据：一次请求拿齐 rEFInd + GRUB。
#[derive(Clone, Debug, Serialize, Default, PartialEq)]
pub struct RemoteBootInfo {
    pub ok: bool,
    /// 这台电脑当前探得通吗（离线时两个引导项一律不可改）
    pub online: bool,
    /// 探活赢家地址（展示用）
    pub addr: String,
    pub error: String,
    pub refind: BootGroup,
    pub grub: BootGroup,
}

/// GRUB 条目名 → 图标分类（纯函数，便于单测）。
fn grub_class(name: &str, class: &str, chainloader: bool) -> String {
    let haystack = format!("{name} {class}").to_ascii_lowercase();
    if haystack.contains("windows") || haystack.contains("win") {
        return "win".to_string();
    }
    if haystack.contains("osx") || haystack.contains("mac") || haystack.contains("apple") {
        return "mac".to_string();
    }
    if chainloader && haystack.contains("efi") {
        return "auto".to_string();
    }
    "linux".to_string()
}

/// rEFInd 的 system → 图标分类
fn refind_class(system: &str) -> String {
    match system {
        "windows" => "win".to_string(),
        "macos" => "mac".to_string(),
        _ => "linux".to_string(),
    }
}

/// 这台电脑的 HTTP 端点（直连取首个非空地址；经跳板机复用本地转发）。
fn server_endpoint(app: &AppHandle, server: &RemoteServer) -> Result<String, String> {
    if server.jump_id.trim().is_empty() {
        let address = server
            .addresses
            .iter()
            .map(|raw| raw.trim())
            .find(|raw| !raw.is_empty())
            .ok_or_else(|| "这台电脑还没有可用地址".to_string())?;
        return Ok(with_default_port(address));
    }
    let state = app.state::<RemoteJumpState>();
    let endpoint = remote_endpoint_inner(app, state.inner(), &server.id)?;
    Ok(format!("{}:{}", endpoint.host, endpoint.port))
}

/// 对这台电脑发一条 `GET {path}?token=…`，返回解析后的 JSON。
///
/// 与 `remote_http_power` 同一套手写 HTTP（不引 reqwest）：局域网场景只需要
/// 一条短请求，多一个依赖不划算。**必须跑在 spawn_blocking 里** —— 会阻塞在 TCP 上。
/// 局域网失败时兜底远程接入地址（https + ab_token）：出门时引导项读取/切换照常可用。
fn remote_http_json(
    app: &AppHandle,
    server: &RemoteServer,
    path: &str,
) -> Result<serde_json::Value, String> {
    // 按用户优先级逐通道试（route_candidates 已滤禁用与不可用项）；
    // jump 候选在这里支持：AppHandle 在手，能解析 ssh -L 转发端点
    let candidates = server.route_candidates();
    let mut last_error = String::from("没有可用连接通道");
    let mut tried = 0usize;
    for cand in &candidates {
        tried += 1;
        let result = match cand.kind.as_str() {
            "lan" => lan_json_get(&cand.origin, server.token.trim(), path),
            "jump" => {
                let state = app.state::<RemoteJumpState>();
                match remote_endpoint_inner(app, state.inner(), &server.id) {
                    Ok(endpoint) => lan_json_get(
                        &format!("{}:{}", endpoint.host, endpoint.port),
                        server.token.trim(),
                        path,
                    ),
                    Err(error) => Err(error),
                }
            }
            _ => remote_https_get(&cand.origin, &cand.token, path)
                .and_then(|(status, body)| parse_remote_json_reply(status, &body)),
        };
        match result {
            Ok(value) => return Ok(value),
            Err(error) => last_error = error,
        }
    }
    if tried == 0 {
        return Err(
            "没有可用连接通道（局域网地址为空，或优先级里所有通道都被禁用/不可用）".to_string(),
        );
    }
    Err(last_error)
}

/// 远程 JSON 回执解析（401 文案按「远程令牌」口径说，与局域网区分开）。
fn parse_remote_json_reply(status: u16, body: &str) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = serde_json::from_str(body.trim())
        .map_err(|error| format!("这台电脑返回的不是合法 JSON（{status}）：{error}"))?;
    if status == 401 {
        return Err("远程通道令牌无效（401）—— 请重新配对或到「编辑」里核对远程接入令牌".to_string());
    }
    if status >= 400 {
        let message = value
            .get("message")
            .and_then(|item| item.as_str())
            .unwrap_or("请求被拒绝");
        return Err(message.to_string());
    }
    Ok(value)
}

/// LAN 直连（或跳板端点）的 JSON GET：`GET {path}?token=…`，手写 TCP。
fn lan_json_get(target: &str, token: &str, path: &str) -> Result<serde_json::Value, String> {
    let mut query = String::new();
    if !token.is_empty() {
        query.push_str(if path.contains('?') { "&" } else { "?" });
        query.push_str("token=");
        query.push_str(&encode_query(token));
    }
    let (status, body) = lan_http_get(target, &format!("{path}{query}"), 8)?;
    if status == 401 {
        return Err("这台电脑不认这个 token（401）—— 到「编辑」里核对访问令牌".to_string());
    }
    if status == 0 {
        return Err("这台电脑的响应不是有效的 HTTP".to_string());
    }
    let value: serde_json::Value = serde_json::from_str(body.trim())
        .map_err(|error| format!("这台电脑返回的不是合法 JSON（{status}）：{error}"))?;
    if status >= 400 {
        let message = value
            .get("message")
            .and_then(|item| item.as_str())
            .unwrap_or("请求被拒绝");
        return Err(message.to_string());
    }
    Ok(value)
}

/// remote_http_json 的局域网那一路（LAN 直连 / 跳板二选一；route_candidates 之外保留给单测与直调）。
fn remote_http_json_lan(
    app: &AppHandle,
    server: &RemoteServer,
    path: &str,
) -> Result<serde_json::Value, String> {
    let target = server_endpoint(app, server)?;
    lan_json_get(&target, server.token.trim(), path)
}

/// 命令通道（/api/grub-switch、/api/power…）的回执解析：{ok, pending, channel, message}。
fn command_reply_message(reply: &serde_json::Value, fallback: &str) -> Result<String, String> {
    if let Some(message) = reply
        .get("message")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Ok(message.to_string());
    }
    let ok = reply.get("ok").and_then(|item| item.as_bool()).unwrap_or(true);
    if !ok {
        return Err(reply
            .get("error")
            .and_then(|item| item.as_str())
            .unwrap_or(fallback)
            .to_string());
    }
    Ok(fallback.to_string())
}

/// 把 `/api/status` 的 variables 收成 rEFInd 的可选组。
fn refind_from_status(value: &serde_json::Value) -> BootGroup {
    let mut options = Vec::new();
    let mut current = String::new();
    for variable in value
        .get("variables")
        .and_then(|item| item.as_array())
        .map(|list| list.as_slice())
        .unwrap_or_default()
    {
        let exists = variable
            .get("exists")
            .and_then(|item| item.as_bool())
            .unwrap_or(false);
        if !exists {
            continue;
        }
        let system = variable
            .get("system")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if system.is_empty() {
            continue;
        }
        let label = variable
            .get("displayName")
            .and_then(|item| item.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or(&system)
            .to_string();
        let active = variable
            .get("active")
            .and_then(|item| item.as_bool())
            .unwrap_or(false);
        if active {
            current = system.clone();
        }
        options.push(BootOption {
            value: system.clone(),
            label,
            class: refind_class(&system),
            header: false,
        });
    }
    BootGroup {
        kind: "refind".to_string(),
        title: "rEFInd".to_string(),
        available: !options.is_empty(),
        current: current.clone(),
        next: current.clone(),
        saved: current,
        note: if options.is_empty() {
            "这台电脑没部署 rEFInd（或还没设置过下次启动项）".to_string()
        } else {
            String::new()
        },
        options,
    }
}

/// 出一条 GRUB 可选项 —— 与托盘菜单的 `menu_model::grub_push_entry` **同一条拼装
/// 规则**，只是产物从菜单节点换成 BootOption：
/// * 文字随格式变（`grub_row_label`：极简/简易=去前缀内核名、中等=完整内核名、
///   原始/复杂=标题原文）；内核为空时退回标题原文（不猜）；
/// * 非原始格式按渲染结果折叠同名条目（首个出面、其余并成「· N 个变体」）；
///   原始格式不折叠，变体数用解析期并进的 `entry.variants`；
/// * 原始/复杂在行尾附内核名（或「转交引导」/class）；
/// * 行文字 = 箭头（子项 ↳）+ 系统图标 +（组前缀）+ 格式化文字 + 附注。
#[allow(clippy::too_many_arguments)]
fn push_grub_option(
    options: &mut Vec<BootOption>,
    format: &str,
    entry: &crate::grub::GrubEntry,
    top: bool,
    group_prefix: &str,
    counts: &std::collections::HashMap<String, usize>,
    emitted: &mut std::collections::HashSet<String>,
) {
    let label = crate::menu_model::grub_row_label(format, entry);
    let mut parts: Vec<String> = Vec::new();
    if format == "raw" {
        // 原始格式不折叠；变体数在解析期并进了 entry.variants
        let variants = entry.variants.max(1);
        if variants > 1 {
            parts.push(format!("{variants} 个变体"));
        }
    } else {
        if !emitted.insert(label.clone()) {
            return;
        }
        let variants = counts.get(&label).copied().unwrap_or(1);
        if variants > 1 {
            parts.push(format!("{variants} 个变体"));
        }
    }
    if format == "raw" || format == "detailed" {
        let tag = if !entry.kernels.is_empty() {
            entry.kernels[0].clone()
        } else if entry.chainloader {
            "转交引导".to_string()
        } else {
            entry.class.clone().unwrap_or_default()
        };
        if !tag.is_empty() {
            parts.push(tag);
        }
    }
    let suffix = if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    };
    let icon = crate::menu_model::grub_os_icon(&format!("{group_prefix}{}", entry.name));
    let arrow = if top { "" } else { "↳ " };
    options.push(BootOption {
        value: entry.selector.clone(),
        label: format!("{arrow}{icon}{group_prefix}{label}{suffix}"),
        class: grub_class(
            &entry.name,
            entry.class.as_deref().unwrap_or(""),
            entry.chainloader,
        ),
        header: false,
    });
}

/// GRUB 子菜单的 id → 组名。
///
/// `/api/grub` 的 `submenu` 字段是 `gnulinux-advanced-9f1bae08-…` 这种内部 id，
/// 真正的组名（「Advanced options for …」）在 `groups` 里。GRUB 列表页与托盘菜单
/// 都会把 id 翻成组名 —— 电脑列表这里也必须翻，否则下拉里一排 uuid 前缀。
fn grub_submenu_titles(value: &serde_json::Value) -> std::collections::HashMap<String, String> {
    let mut titles = std::collections::HashMap::new();
    for group in value
        .get("groups")
        .and_then(|item| item.as_array())
        .map(|list| list.as_slice())
        .unwrap_or_default()
    {
        let id = group
            .get("id")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim();
        let name = group
            .get("name")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim();
        if !id.is_empty() || !name.is_empty() {
            titles.insert(id.to_string(), name.to_string());
        }
    }
    titles
}

/// 把 `/api/grub` 的 entries 收成 GRUB 的可选组 —— **结构与拼装规则和托盘菜单
/// （`menu_model::grub_menu_nodes`）逐条对齐**：
/// * 当前选中项置顶（组内子项带「组名 › 」前缀），原位照常保留；
/// * 每个 submenu 一行组头「▸ 图标 组名（N 项[ · ESP 位置]）」，位置按格式带不带
///   （极简/原始不带）；组名翻自 `groups`（老服务端没有 groups 时退回内部 id）；
/// * 子项「↳ 」缩进；顶层与各组各有一套折叠状态（非原始格式折叠同名变体）；
/// * 条目文字随格式变（`push_grub_option`，与 `grub_row_label` 同一规则）。
fn grub_from_json(value: &serde_json::Value, format: &str) -> BootGroup {
    let next = value
        .get("nextEntry")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let saved = value
        .get("savedEntry")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let installed = value
        .get("installed")
        .and_then(|item| item.as_bool())
        .unwrap_or(false);
    let format = crate::settings_commands::normalize_grub_list_format(format);
    let active = if next.is_empty() {
        saved.clone()
    } else {
        next.clone()
    };

    // 条目直接反序列化成 GrubEntry —— 拼装助手（grub_row_label / grub_collapse）
    // 只吃这个类型，反序列化回来就能与托盘菜单共用同一套代码。老服务端不发
    // variants（甚至 kernels），serde(default) 兜住；坏条目跳过。
    // 内核名里的空白项清掉 —— grub_kernel_of 只对「空 Vec」回落标题，
    // 空串内核会渲染出一行空白。
    let mut entries: Vec<crate::grub::GrubEntry> = Vec::new();
    if let Some(list) = value.get("entries").and_then(|item| item.as_array()) {
        for item in list {
            let Ok(mut entry) = serde_json::from_value::<crate::grub::GrubEntry>(item.clone())
            else {
                continue;
            };
            if entry.selector.trim().is_empty() {
                continue;
            }
            entry.kernels.retain(|kernel| !kernel.trim().is_empty());
            entries.push(entry);
        }
    }

    let group_titles = grub_submenu_titles(value);
    let mut options: Vec<BootOption> = Vec::new();

    // 置顶当前选中项（组内子项补「组名 › 」前缀 —— 与托盘/主窗置顶行同一规则）
    if let Some(pinned) = entries.iter().find(|entry| entry.selector == active) {
        let prefix = pinned
            .submenu
            .as_deref()
            .and_then(|id| group_titles.get(id))
            .map(|title| format!("{title} › "))
            .unwrap_or_default();
        push_grub_option(
            &mut options,
            format,
            pinned,
            true,
            &prefix,
            &std::collections::HashMap::new(),
            &mut std::collections::HashSet::new(),
        );
    }

    // 组内子项索引（与托盘同一份 children_of）；空串 submenu（老数据/手写 cfg）
    // 按「没有父级」算 —— 真服务端顶层条目的 submenu 是 null。
    let mut children_of: std::collections::HashMap<&str, Vec<&crate::grub::GrubEntry>> =
        std::collections::HashMap::new();
    for entry in &entries {
        if let Some(submenu) = entry.submenu.as_deref().filter(|id| !id.is_empty()) {
            children_of.entry(submenu).or_default().push(entry);
        }
    }

    // 组头附加的 ESP 位置（简易/中等/复杂带，极简/原始不带 —— 与托盘同一规则）
    let location = if format == "minimal" || format == "raw" {
        String::new()
    } else {
        let mut parts: Vec<String> = Vec::new();
        let dir = value
            .get("efiDir")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim();
        if !dir.is_empty() {
            parts.push(format!("位于{dir}"));
        }
        let uuid = value
            .get("partuuid")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim();
        if !uuid.is_empty() {
            parts.push(format!("UUID={uuid}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" · {}", parts.join(" "))
        }
    };

    // 主体：顶层一套折叠状态，各组独立一套（与托盘一致）；原始格式不折叠
    let mut emitted_groups = std::collections::HashSet::new();
    let top_counts = crate::menu_model::grub_collapse(
        format,
        entries.iter().filter(|entry| entry.submenu.is_none()),
    );
    let mut top_emitted = std::collections::HashSet::new();
    for entry in &entries {
        match entry.submenu.as_deref().filter(|id| !id.is_empty()) {
            None => {
                push_grub_option(
                    &mut options,
                    format,
                    entry,
                    true,
                    "",
                    &top_counts,
                    &mut top_emitted,
                );
            }
            Some(submenu_id) => {
                if !emitted_groups.insert(submenu_id) {
                    continue;
                }
                let title = group_titles
                    .get(submenu_id)
                    .cloned()
                    .unwrap_or_else(|| submenu_id.to_string());
                let count = children_of.get(submenu_id).map(Vec::len).unwrap_or(0);
                options.push(BootOption {
                    value: String::new(),
                    label: format!(
                        "▸ {}{title}（{count} 项{location}）",
                        crate::menu_model::grub_os_icon(&title)
                    ),
                    class: String::new(),
                    header: true,
                });
                let counts = crate::menu_model::grub_collapse(
                    format,
                    children_of.get(submenu_id).into_iter().flatten().copied(),
                );
                let mut emitted = std::collections::HashSet::new();
                for child in children_of.get(submenu_id).into_iter().flatten() {
                    push_grub_option(&mut options, format, child, false, "", &counts, &mut emitted);
                }
            }
        }
    }
    let note = if options.is_empty() {
        let raw = value
            .get("error")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim();
        if raw.is_empty() {
            "这台电脑没部署 GRUB（或还没同步 grub.cfg）".to_string()
        } else {
            raw.to_string()
        }
    } else {
        String::new()
    };
    BootGroup {
        kind: "grub".to_string(),
        title: "GRUB".to_string(),
        available: installed || !options.is_empty(),
        current: if next.is_empty() {
            saved.clone()
        } else {
            next.clone()
        },
        next,
        saved,
        note,
        options,
    }
}

/// 电脑页「引导项」列：读这台电脑的 rEFInd 与 GRUB 现状。
///
/// **必须 async + spawn_blocking**：探活 + 两次 HTTP 都要等网络。
#[tauri::command]
pub async fn remote_boot_info(
    app: AppHandle,
    server_id: String,
) -> Result<RemoteBootInfo, String> {
    tauri::async_runtime::spawn_blocking(move || remote_boot_info_blocking(&app, &server_id))
        .await
        .map_err(|error| format!("读取引导项任务失败：{error}"))?
}

fn remote_boot_info_blocking(app: &AppHandle, server_id: &str) -> Result<RemoteBootInfo, String> {
    // 本机行（电脑列表的「本机」固定行）：它就在本进程里，直连 IPC ——
    // 直接调用 /api/status、/api/grub 的进程内生成函数（remote_status_json /
    // remote_grub_json），不走回环 HTTP：服务端没起（端口被保留段盖住）也照读不误，
    // 解析逻辑（refind_from_status / grub_from_json）原样复用。
    if server_id == "local" {
        let status = crate::remote_routing::remote_status_json(app);
        let grub_json = crate::remote_routing::remote_grub_json(app);
        let format = crate::read_settings_file(app).grub_list_format;
        let mut info = RemoteBootInfo {
            refind: refind_from_status(&status),
            grub: grub_from_json(&grub_json, &format),
            ..Default::default()
        };
        info.ok = true;
        info.online = true;
        info.addr = "127.0.0.1".to_string();
        return Ok(info);
    }
    let server = load_store(app)?
        .servers
        .into_iter()
        .find(|candidate| candidate.id == server_id)
        .ok_or_else(|| format!("这台电脑不存在（可能已被删除）：{server_id}"))?;
    let mut info = RemoteBootInfo {
        refind: BootGroup {
            kind: "refind".to_string(),
            title: "rEFInd".to_string(),
            ..Default::default()
        },
        grub: BootGroup {
            kind: "grub".to_string(),
            title: "GRUB".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    // 先探活：离线时没必要再发两条注定超时的请求（等待体验差 3 倍）
    // 自动模式：局域网探不通时兜底远程接入地址（出门场景：引导项照常可读）；
    // 远程优先：先探隧道，通了直接用，局域网只做兜底。
    let probe_addr = server
        .addresses
        .iter()
        .map(|raw| raw.trim())
        .find(|raw| !raw.is_empty())
        .unwrap_or("")
        .to_string();
    // 探活按用户优先级逐通道试（route_candidates 已滤禁用与不可用项）；
    // 赢家地址原样给前端（远程 origin 由它自己拼 https）。
    //
    // **探测方式也照「连接策略」走**：没有跳板机通道时交给统一探测引擎
    // （并发首个 / 并发全部 / 串行 + 档案里的超时，跑在独立探测线程池里）；
    // 有跳板机时保留原来的串行循环 —— 跳板机端点要 AppHandle 解析，引擎里没有。
    let candidates = server.route_candidates();
    let has_jump = candidates.iter().any(|cand| cand.kind == "jump");
    let mut last_error = String::from("没有可用连接通道");
    let mut winner: Option<(u64, String, String, String)> = None;
    if has_jump {
        for cand in &candidates {
            // 超时按候选自己算（配对走条目→区块→全局三层继承，lan/jump 走通道表）
            let per_kind = Duration::from_millis(cand.timeout_ms);
            let result = match cand.kind.as_str() {
                "lan" => probe_health(&cand.origin, per_kind),
                "jump" => {
                    let state = app.state::<RemoteJumpState>();
                    match remote_endpoint_inner(app, state.inner(), server_id) {
                        Ok(endpoint) => probe_health(
                            &format!("{}:{}", endpoint.host, endpoint.port),
                            per_kind,
                        ),
                        Err(error) => Err(error),
                    }
                }
                _ => probe_remote_health(&cand.origin, per_kind),
            };
            match result {
                Ok((ms, machine_id, machine_name)) => {
                    winner = Some((ms, machine_id, machine_name, cand.origin.clone()));
                    break;
                }
                Err(error) => last_error = error,
            }
        }
    } else {
        // lan 在 route_candidates 里只占一条（它给请求路径用），探测要逐块网卡都试
        let mut engine_candidates: Vec<RouteTestCandidate> = Vec::new();
        for cand in &candidates {
            if cand.kind == "lan" {
                for raw in &server.addresses {
                    let (address, _) = split_address(raw);
                    if !address.is_empty() {
                        engine_candidates.push(RouteTestCandidate {
                            kind: "lan".to_string(),
                            origin: address,
                            timeout_ms: server.probe_timeout_for("lan").as_millis() as u64,
                            ..Default::default()
                        });
                    }
                }
            } else {
                engine_candidates.push(RouteTestCandidate {
                    kind: cand.kind.clone(),
                    origin: cand.origin.clone(),
                    timeout_ms: cand.timeout_ms,
                    pair_id: String::new(),
                });
            }
        }
        let outcome = probe_candidates_blocking(
            engine_candidates,
            &normalize_probe_mode(&server.route_probe_mode),
            &normalize_probe_mode(&server.probe_mode),
            &server.route_timeouts_ms,
            server.probe_timeout_ms,
            &server.block_probe_mode,
        );
        if outcome.ok {
            winner = Some((
                outcome.ms,
                outcome.machine_id.clone(),
                outcome.machine_name.clone(),
                outcome.addr.clone(),
            ));
        } else {
            last_error = outcome.error;
        }
    }
    let (probe, winner_addr) = match winner {
        Some((ms, machine_id, machine_name, addr)) => (Ok((ms, machine_id, machine_name)), addr),
        None => (Err(last_error), probe_addr),
    };
    match probe {
        Ok(_) => {
            info.online = true;
            info.addr = winner_addr;
        }
        Err(error) => {
            info.online = false;
            info.error = error;
            info.refind.note = "电脑离线，无法读取引导项".to_string();
            info.grub.note = "电脑离线，无法读取引导项".to_string();
            return Ok(info);
        }
    }
    // 两个接口各自失败互不影响：一边能读就先把那边显示出来
    let mut errors: Vec<String> = Vec::new();
    match remote_http_json(app, &server, "/api/status") {
        Ok(value) => info.refind = refind_from_status(&value),
        Err(error) => {
            info.refind.note = error.clone();
            errors.push(format!("rEFInd：{error}"));
        }
    }
    match remote_http_json(app, &server, "/api/grub") {
        // 条目文字按本机设置里的「GRUB 列表格式」生成（与 GRUB 列表页/托盘菜单一致）
        Ok(value) => {
            let format = crate::read_settings_file(app).grub_list_format;
            info.grub = grub_from_json(&value, &format);
            // 排查用（受 debugTrace 开关控制）：这次按哪档生成、前三条长什么样
            crate::trace(
                app,
                "boot",
                &format!(
                    "grub format={format} server={server_id} labels={:?}",
                    info.grub
                        .options
                        .iter()
                        .take(3)
                        .map(|option| option.label.as_str())
                        .collect::<Vec<_>>()
                ),
            );
        }
        Err(error) => {
            info.grub.note = error.clone();
            errors.push(format!("GRUB：{error}"));
        }
    }
    info.ok = errors.len() < 2;
    if !errors.is_empty() {
        info.error = errors.join("；");
    }
    Ok(info)
}

/// 切换引导项：rEFInd 走 `/api/switch?system=`，GRUB 走 `/api/grub-switch?entry=`。
///
/// **必须 async + spawn_blocking**：同上（等网络）。
#[tauri::command]
pub async fn remote_boot_switch(
    app: AppHandle,
    server_id: String,
    kind: String,
    value: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        remote_boot_switch_blocking(&app, &server_id, &kind, &value)
    })
    .await
    .map_err(|error| format!("切换引导项任务失败：{error}"))?
}

fn remote_boot_switch_blocking(
    app: &AppHandle,
    server_id: &str,
    kind: &str,
    value: &str,
) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("没有选择引导项".to_string());
    }
    let kind = kind.trim();
    // 本机行：直连 IPC —— 直接调远程命令的进程内执行入口（与 CLI/HTTP 走的
    // 是同一个 execute_remote_command，落到 switch_and_update / switch_grub_and_update）
    if server_id == "local" {
        let command = match kind {
            "refind" => {
                if !crate::SOURCE_FILES
                    .iter()
                    .any(|(system, _, _)| *system == value)
                {
                    return Err(format!("未知的系统：{value}"));
                }
                crate::state::RemoteCommand::Switch(value.to_string())
            }
            "grub" => crate::state::RemoteCommand::SwitchGrub(value.to_string()),
            other => return Err(format!("未知的引导管理器：{other}")),
        };
        let (ok, message) = crate::remote_routing::execute_remote_command(
            app,
            crate::state::Channel::Http,
            "hub-电脑列表-本机行",
            &command,
        );
        return if ok { Ok(message) } else { Err(message) };
    }
    let path = match kind {
        // system 只有 linux/windows/macos 三个值，不需要转义
        "refind" => {
            if !crate::SOURCE_FILES
                .iter()
                .any(|(system, _, _)| *system == value)
            {
                return Err(format!("未知的系统：{value}"));
            }
            format!("/api/switch?system={value}")
        }
        // selector 里可能有空格、`>`、引号 —— 必须转义
        "grub" => format!("/api/grub-switch?entry={}", encode_query(value)),
        other => return Err(format!("未知的引导管理器：{other}")),
    };
    let server = load_store(app)?
        .servers
        .into_iter()
        .find(|candidate| candidate.id == server_id)
        .ok_or_else(|| format!("这台电脑不存在（可能已被删除）：{server_id}"))?;
    let reply = remote_http_json(app, &server, &path)?;
    command_reply_message(&reply, &format!("已提交切换：{value}"))
}

/// 用系统浏览器打开这台电脑的 AnyBoot 页面。
///
/// 为什么不让前端 `window.open`：hub 是 Tauri webview，`window.open` 只会开出
/// 一个新的 webview（还带着 tauri:// 的源），行为随平台不一致；电源页那种
/// 「用系统浏览器打开」的语义必须落到 `open_url_in_browser` 上。
/// 只放行 http/https —— 别让任意 scheme 穿过命令边界。
#[tauri::command]
pub fn remote_open_page(url: String) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("地址是空的".to_string());
    }
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!("只支持 http/https 地址：{url}"));
    }
    // 空白/控制字符会把 URL 拼坏（也会让 ShellExecuteW 收到半截命令）
    if url.chars().any(|item| item.is_whitespace() || item.is_control()) {
        return Err("地址里不能有空白或控制字符".to_string());
    }
    crate::open_url_in_browser(url)
}

/// 关于页：打开项目 GitHub 页面（白名单限定本项目域名，不开放任意 URL）。
#[tauri::command]
pub fn hub_about_open(page: String) -> Result<(), String> {
    let url = match page.as_str() {
        "repo" => "https://github.com/lidiaoo/anyboot",
        "releases" => "https://github.com/lidiaoo/anyboot/releases",
        _ => return Err(format!("未知页面: {page}")),
    };
    crate::open_url_in_browser(url)
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用：一个通道都没单独设超时（所有通道都回落默认值）
    fn no_route_timeouts() -> std::collections::BTreeMap<String, u64> {
        std::collections::BTreeMap::new()
    }

    /// 测试用：三个区块都没单独设策略（全部落多网卡策略）
    fn no_block_modes() -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::new()
    }

    #[test]
    fn split_address_forms() {
        // 裸 host:port 原样
        assert_eq!(split_address("192.168.0.120:12300"), ("192.168.0.120:12300".into(), None));
        // 裸 host 无端口（探活时补默认端口）
        assert_eq!(split_address("192.168.0.120"), ("192.168.0.120".into(), None));
        // 整条 URL：剥 scheme/路径，挖 token
        assert_eq!(
            split_address("http://192.168.0.120:12300/?token=abc123"),
            ("192.168.0.120:12300".into(), Some("abc123".into()))
        );
        // https + 带路径
        assert_eq!(
            split_address("https://host.example.com:12300/remote?token=t+x"),
            ("host.example.com:12300".into(), Some("t+x".into()))
        );
        // URL 无 token
        assert_eq!(
            split_address("http://192.168.0.120:12300"),
            ("192.168.0.120:12300".into(), None)
        );
        // 空 / 空白
        assert_eq!(split_address("  "), (String::new(), None));
    }

    #[test]
    fn with_default_port_rules() {
        assert_eq!(with_default_port("192.168.0.120"), "192.168.0.120:12300");
        assert_eq!(with_default_port("192.168.0.120:12300"), "192.168.0.120:12300");
        assert_eq!(with_default_port("localhost:8080"), "localhost:8080");
    }

    #[test]
    fn parse_health_fields() {
        let body = r#"{"ok":true,"service":"anyboot","machineId":" m-1 ","machineName":"书房台式机"}"#;
        assert_eq!(parse_health(body), ("m-1".into(), "书房台式机".into()));
        // 非法 JSON / 缺字段 → 空串，不报错
        assert_eq!(parse_health("not json"), (String::new(), String::new()));
        assert_eq!(parse_health("{}"), (String::new(), String::new()));
    }

    #[test]
    fn normalize_trims_and_merges_token_from_url() {
        let server = RemoteServer {
            id: String::new(),
            name: " 台式机 ".into(),
            addresses: vec![
                " 192.168.0.120:12300 ".into(),
                "http://192.168.0.121:12300/?token= abc ".into(),
                "192.168.0.120:12300".into(), // 重复，去掉
                " ".into(),                   // 空，去掉
            ],
            token: String::new(),
            machine_id: String::new(),
            machine_name: String::new(),
            machine_os: String::new(),
            probe_timeout_ms: 0,
            route_timeouts_ms: Default::default(),
            probe_mode: " other ".into(),
            route_probe_mode: "parallel_first".into(),
            jump_id: " j1 ".into(),
            macs: Default::default(),
            power_routes: vec![],
            pairs: vec![RemotePair {
                kind: "cloudflare".into(),
                addr: "  https://test-tunnel.trycloudflare.com/?ab_token= gw-tok  ".into(),
                ..Default::default()
            }],
            block_probe_mode: Default::default(),
            block_timeouts_ms: Default::default(),
            conn_mode: String::new(),
            route_order: Vec::new(),
            route_disabled: Vec::new(),
            created_at: 0,
            ..Default::default()
        };
        let normalized = server.normalize();
        assert_eq!(normalized.name, "台式机");
        assert_eq!(normalized.addresses, vec!["192.168.0.120:12300", "192.168.0.121:12300"]);
        // URL 里的 token 被挖出补进 token（原 token 为空时）
        assert_eq!(normalized.token, "abc");
        // 配对：剥路径/query 留 origin；URL 里的 ab_token 挖进这一条自己的 token
        assert_eq!(normalized.pairs.len(), 1);
        assert_eq!(normalized.pairs[0].addr, "https://test-tunnel.trycloudflare.com");
        assert_eq!(normalized.pairs[0].token, "gw-tok");
        assert!(!normalized.pairs[0].id.is_empty(), "空 id 要补发");
        assert_eq!(normalized.effective_remote_addr(), "https://test-tunnel.trycloudflare.com");
        assert_eq!(normalized.effective_remote_token(), "gw-tok");
        // 旧值 / 未知 probe_mode 一律归一为并发（首个）
        assert_eq!(normalized.probe_mode, "parallel_first");
        assert_eq!(normalized.jump_id, "j1");
    }

    // ---- 配对条目（RemotePair） ----

    fn pair(kind: &str, addr: &str, mid: &str) -> RemotePair {
        RemotePair {
            kind: kind.into(),
            addr: addr.into(),
            mid: mid.into(),
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn pair_lane_derived_from_addr() {
        let n = RemoteServer {
            pairs: vec![
                pair("ts", "100.119.234.95:12300", "m1"),
                pair("ts", "https://box.tail1234.ts.net", "m2"),
                pair("cloudflare", "https://a.trycloudflare.com", "m3"),
            ],
            ..Default::default()
        }
        .normalize();
        // 100.64/10 补 http:// 并推导 direct；*.ts.net 推导 funnel；cloudflare 没有 lane
        assert_eq!(n.pairs[0].addr, "http://100.119.234.95:12300");
        assert_eq!(n.pairs[0].lane, "direct");
        assert_eq!(n.pairs[0].route_kind(), "ts");
        assert_eq!(n.pairs[1].lane, "funnel");
        assert_eq!(n.pairs[1].route_kind(), "funnel");
        assert_eq!(n.pairs[2].lane, "");
        assert_eq!(n.pairs[2].route_kind(), "tunnel");
    }

    #[test]
    fn pair_kind_falls_back_by_addr_shape() {
        // kind 非法/为空：100.x / *.ts.net 归 ts，其余归 cloudflare（不猜 tsnet）
        let n = RemoteServer {
            pairs: vec![
                pair("", "https://win-pc.tailabc.ts.net", "m1"),
                pair("bogus", "https://a.trycloudflare.com", "m2"),
                RemotePair { kind: String::new(), addr: String::new(), ..Default::default() },
            ],
            ..Default::default()
        }
        .normalize();
        // 空地址那条被丢掉
        assert_eq!(n.pairs.len(), 2);
        assert_eq!(n.pairs[0].kind, "ts");
        assert_eq!(n.pairs[1].kind, "cloudflare");
    }

    #[test]
    fn pairs_dedup_by_mid_and_kind_keeps_position_and_id() {
        // 同一系统同一区块重新配对 = 更新那一行（保留位置与原 id）；
        // 同一系统不同区块 = 两条都留（只按 mid 去重会让它们互顶，等于回到原点）
        let n = RemoteServer {
            pairs: vec![
                pair("cloudflare", "https://old.trycloudflare.com", "m1"),
                pair("ts", "100.64.0.1:12300", "m1"),
                pair("cloudflare", "https://new.trycloudflare.com", "m1"),
            ],
            ..Default::default()
        }
        .normalize();
        assert_eq!(n.pairs.len(), 2, "cloudflare 被更新、ts 保留");
        assert_eq!(n.pairs[0].addr, "https://new.trycloudflare.com", "更新项留在原位");
        assert_eq!(n.pairs[0].kind, "cloudflare");
        assert_eq!(n.pairs[1].kind, "ts");
    }

    #[test]
    fn pairs_same_mid_ts_direct_and_funnel_coexist() {
        // ts 区块合并了直连与系统 Funnel，lane 是幂等键的一部分：
        // 同一台电脑（同 mid）的 direct 与 funnel **并存**，互不顶替 ——
        // 只按 (mid, kind) 去重会让扫 funnel 码顶掉 direct 码（互顶从区块间挪进了区块内）。
        let n = RemoteServer {
            pairs: vec![
                pair("ts", "100.64.0.1:12300", "m1"),
                pair("ts", "https://box.tailabc.ts.net", "m1"),
                pair("ts", "100.64.0.9:12300", "m1"),
            ],
            ..Default::default()
        }
        .normalize();
        assert_eq!(n.pairs.len(), 2, "direct 与 funnel 并存；第三条与第一条同 lane，顶替它");
        assert_eq!(n.pairs[0].lane, "direct");
        assert_eq!(n.pairs[0].addr, "http://100.64.0.9:12300", "同 lane 重扫更新原行（保留位置）");
        assert_eq!(n.pairs[1].lane, "funnel");
    }

    #[test]
    fn pairs_without_mid_dedup_by_addr() {
        // 手填地址没有 mid：不能按 (空 mid, kind) 去重，否则同区块第二条会被吞掉
        let n = RemoteServer {
            pairs: vec![
                pair("cloudflare", "https://a.trycloudflare.com", ""),
                pair("cloudflare", "https://b.trycloudflare.com", ""),
                pair("cloudflare", "https://a.trycloudflare.com", ""),
            ],
            ..Default::default()
        }
        .normalize();
        assert_eq!(n.pairs.len(), 2);
        assert_eq!(n.pairs[0].addr, "https://a.trycloudflare.com");
        assert_eq!(n.pairs[1].addr, "https://b.trycloudflare.com");
    }

    #[test]
    fn pairs_capped_at_soft_limit() {
        let pairs: Vec<RemotePair> = (0..MAX_PAIRS + 5)
            .map(|index| pair("cloudflare", &format!("https://t{index}.trycloudflare.com"), ""))
            .collect();
        let n = RemoteServer { pairs, ..Default::default() }.normalize();
        assert_eq!(n.pairs.len(), MAX_PAIRS);
    }

    // ---- 配对串 v2 解析（S3：%v=2 收，其余拒收） ----

    #[test]
    fn parse_pair_string_reads_every_v2_field() {
        let parsed = parse_pair_string(
            "anyboot://pair?v=2&mid=m-1&os=windows&host=%E4%B9%A6%E6%88%BF&gateway=%E4%B9%A6%E6%88%BF&type=ts&tunnel=https%3A%2F%2Fwin.tail.ts.net&token=gw1&tunnel_id=tid-1",
        )
        .expect("合法 v2 串");
        assert_eq!(parsed.mid, "m-1");
        assert_eq!(parsed.os, "windows");
        assert_eq!(parsed.host, "书房");
        assert_eq!(parsed.kind, "ts");
        assert_eq!(parsed.tunnel, "https://win.tail.ts.net");
        assert_eq!(parsed.token, "gw1");
        assert_eq!(parsed.tunnel_id, "tid-1");
        // os 只留三个徽标值（其它一律空 → 前端不画徽标）
        assert_eq!(parse_pair_string("anyboot://pair?v=2&type=ts&os=Windows%2011").unwrap().os, "");
    }

    #[test]
    fn parse_pair_string_rejects_missing_or_wrong_version() {
        // 老串：没有 v → 提示重新生成（不是「格式错误」这种没用的说法）
        let err = parse_pair_string("anyboot://pair?gateway=pc&tunnel=https://a.trycloudflare.com").unwrap_err();
        assert!(err.contains("旧版配对码"), "{err}");
        // v=1 / v=3 同样拒收
        for raw in ["anyboot://pair?v=1&type=cloudflare", "anyboot://pair?v=3&type=cloudflare"] {
            let err = parse_pair_string(raw).unwrap_err();
            assert!(err.contains("版本不支持"), "{raw} → {err}");
        }
        // 不是配对码
        assert!(parse_pair_string("https://a.trycloudflare.com").is_err());
        assert!(parse_pair_string("").is_err());
    }

    #[test]
    fn parse_pair_string_rejects_illegal_type() {
        // 通道 id 不是区块 id：过去把 `tunnel` 直接塞进 type，客户端就得靠猜
        for bad in ["tunnel", "funnel", "builtin", "tailscale"] {
            let raw = format!("anyboot://pair?v=2&type={bad}&tunnel=https://a.trycloudflare.com");
            let err = parse_pair_string(&raw).unwrap_err();
            assert!(err.contains("type 非法"), "{bad} → {err}");
        }
        // 整个缺失也算非法（不许按地址猜类型）
        let err = parse_pair_string("anyboot://pair?v=2&tunnel=https://a.trycloudflare.com").unwrap_err();
        assert!(err.contains("type 非法") && err.contains("缺失"), "{err}");
        // 三个合法区块都收
        for good in ["cloudflare", "ts", "tsnet"] {
            let raw = format!("anyboot://pair?v=2&type={good}&tunnel=https://a.trycloudflare.com");
            assert_eq!(parse_pair_string(&raw).unwrap().kind, good);
        }
    }

    #[test]
    fn pair_code_in_addr_is_upgraded_then_deduped_by_mid_and_kind() {
        // 整条配对码粘进「地址」栏：归一后就地补全 type/mid/os/host/token/tunnel_id
        let upgraded = RemoteServer {
            pairs: vec![RemotePair {
                kind: String::new(),
                addr: "anyboot://pair?v=2&mid=m9&os=linux&host=garuda&type=ts&tunnel=http%3A%2F%2F100.64.0.7%3A12300&token=gw9&tunnel_id=".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
        .normalize();
        assert_eq!(upgraded.pairs.len(), 1);
        let one = &upgraded.pairs[0];
        assert_eq!(one.kind, "ts");
        assert_eq!(one.mid, "m9");
        assert_eq!(one.os, "linux");
        assert_eq!(one.host, "garuda");
        assert_eq!(one.addr, "http://100.64.0.7:12300");
        assert_eq!(one.token, "gw9");
        assert_eq!(one.lane, "direct");

        // 同一台机器同一区块再扫一次 → 更新那一行（位置与 id 不变），不新增
        let mut server = upgraded;
        let original_id = server.pairs[0].id.clone();
        let original_at = server.pairs[0].created_at;
        server.pairs.push(RemotePair {
            addr: "anyboot://pair?v=2&mid=m9&os=linux&host=garuda&type=ts&tunnel=http%3A%2F%2F100.64.0.9%3A12300&token=gw10".into(),
            ..Default::default()
        });
        let server = server.normalize();
        assert_eq!(server.pairs.len(), 1, "同 (mid,kind) 只留一条");
        assert_eq!(server.pairs[0].id, original_id, "id 不变（探测结果才对得上）");
        assert_eq!(server.pairs[0].created_at, original_at, "首次配对时间不变");
        assert_eq!(server.pairs[0].addr, "http://100.64.0.9:12300", "地址被后一条更新");

        // 同一台机器换个区块 → 两条都留（只按 mid 去重会让它们互顶）
        let mut server = server;
        server.pairs.push(RemotePair {
            addr: "anyboot://pair?v=2&mid=m9&os=linux&type=tsnet&tunnel=https%3A%2F%2Fgaruda.tail9.ts.net&token=gw11".into(),
            ..Default::default()
        });
        let server = server.normalize();
        assert_eq!(server.pairs.len(), 2);
        assert_eq!(server.pairs[0].kind, "ts");
        assert_eq!(server.pairs[1].kind, "tsnet");
        assert_eq!(server.effective_remote_addr(), "http://100.64.0.9:12300");
    }

    #[test]
    fn old_pair_code_in_addr_is_kept_as_plain_address() {
        // 老串（无 v=2）不解析成条目，但也**不静默删除**用户数据：
        // 退回「当普通地址用」，连不上由探测结果说话；「拒收 + 重新生成」的提示归 UI。
        let n = RemoteServer {
            pairs: vec![RemotePair {
                kind: String::new(),
                addr: "anyboot://pair?gateway=pc&tunnel=https%3A%2F%2Fold.trycloudflare.com&token=gw-old".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
        .normalize();
        assert_eq!(n.pairs.len(), 1, "不能把用户的条目吃掉");
        assert_eq!(n.pairs[0].addr, "https://old.trycloudflare.com");
        assert_eq!(n.pairs[0].token, "gw-old");
        assert_eq!(n.pairs[0].mid, "", "老串没有 mid，不许瞎猜");
        // kind 落回「用户点添加的那个区块」的兜底值
        assert_eq!(n.pairs[0].kind, "cloudflare");
    }

    #[test]
    fn effective_addr_follows_block_order_then_pair_order() {
        // 默认区块序 cloudflare → ts → tsnet：取第一条 cloudflare
        let mut n = RemoteServer {
            pairs: vec![
                pair("tsnet", "https://tsnet.tail.ts.net", "m3"),
                pair("cloudflare", "https://a.trycloudflare.com", "m1"),
                pair("cloudflare", "https://b.trycloudflare.com", "m2"),
            ],
            ..Default::default()
        }
        .normalize();
        assert_eq!(n.effective_remote_addr(), "https://a.trycloudflare.com");
        // 把远程优先级改成 tsnet 领头：区块顺序随之变化，生效地址跟着变
        n.route_order = vec!["tsnet".into(), "tunnel".into(), "ts".into(), "lan".into(), "jump".into(), "funnel".into()];
        assert_eq!(n.pair_block_order(), vec!["tsnet", "cloudflare", "ts"]);
        assert_eq!(n.effective_remote_addr(), "https://tsnet.tail.ts.net");
    }

    #[test]
    fn effective_addr_skips_disabled_pair() {
        let n = RemoteServer {
            pairs: vec![
                RemotePair { enabled: false, ..pair("cloudflare", "https://a.trycloudflare.com", "m1") },
                pair("cloudflare", "https://b.trycloudflare.com", "m2"),
            ],
            ..Default::default()
        }
        .normalize();
        assert_eq!(n.effective_remote_addr(), "https://b.trycloudflare.com");
        // 全禁用 = 没有生效地址
        let mut all_off = n.clone();
        for item in all_off.pairs.iter_mut() {
            item.enabled = false;
        }
        assert_eq!(all_off.effective_remote_addr(), "");
    }

    #[test]
    fn block_settings_are_sanitized() {
        let mut modes = std::collections::BTreeMap::new();
        modes.insert("ts".to_string(), "serial".to_string());
        modes.insert("cloudflare".to_string(), "parallel_first".to_string()); // 默认值不存
        modes.insert("bogus".to_string(), "serial".to_string()); // 非法区块丢掉
        let mut timeouts = std::collections::BTreeMap::new();
        timeouts.insert("ts".to_string(), 5_000u64);
        timeouts.insert("tsnet".to_string(), 0u64); // 0 = 没设，删掉
        timeouts.insert("bogus".to_string(), 3_000); // 非法区块丢掉
        let n = RemoteServer {
            block_probe_mode: modes,
            block_timeouts_ms: timeouts,
            ..Default::default()
        }
        .normalize();
        assert_eq!(n.block_probe_mode.get("ts").map(String::as_str), Some("serial"));
        assert!(!n.block_probe_mode.contains_key("cloudflare"));
        assert!(!n.block_probe_mode.contains_key("bogus"));
        assert_eq!(n.block_timeouts_ms.get("ts").copied(), Some(5_000));
        assert!(!n.block_timeouts_ms.contains_key("tsnet"));
        assert!(!n.block_timeouts_ms.contains_key("bogus"));
    }

    #[test]
    fn probe_timeout_inherits_pair_then_block_then_global() {
        // 三层继承：条目 > 区块 > 全局（默认 2000）
        let mut timeouts = std::collections::BTreeMap::new();
        timeouts.insert("ts".to_string(), 5_000u64);
        let server = RemoteServer {
            probe_timeout_ms: 0, // → 默认 2000
            block_timeouts_ms: timeouts,
            ..Default::default()
        };
        let own = RemotePair { probe_timeout_ms: 1_200, ..pair("ts", "100.64.0.1:12300", "m1") };
        assert_eq!(server.probe_timeout_for_pair(&own).as_millis(), 1_200);
        let inherit_block = pair("ts", "100.64.0.1:12300", "m1");
        assert_eq!(server.probe_timeout_for_pair(&inherit_block).as_millis(), 5_000);
        let inherit_global = pair("cloudflare", "https://a.trycloudflare.com", "m1");
        assert_eq!(server.probe_timeout_for_pair(&inherit_global).as_millis(), 2_000);
    }

    /// 双槽测试共用的最小档案（addresses 非空，过 normalize 前置校验不需要但保持真实形态）
    fn minimal_server() -> RemoteServer {
        RemoteServer {
            id: String::new(),
            name: String::new(),
            addresses: vec!["192.168.0.120:12300".into()],
            token: String::new(),
            machine_id: String::new(),
            machine_name: String::new(),
            machine_os: String::new(),
            probe_timeout_ms: 0,
            route_timeouts_ms: Default::default(),
            probe_mode: String::new(),
            route_probe_mode: "parallel_first".into(),
            jump_id: String::new(),
            macs: Default::default(),
            power_routes: vec![],
            conn_mode: String::new(),
            route_order: Vec::new(),
            route_disabled: Vec::new(),
            created_at: 0,
            pairs: Vec::new(),
            block_probe_mode: Default::default(),
            block_timeouts_ms: Default::default(),
        }
    }

    #[test]
    fn tailscale_bare_ip_gets_http_scheme() {
        // 审计 P0：100.64/10 裸 IP 补 http://（本机服务是明文 HTTP，一律补 https 会连不上）
        assert_eq!(
            split_remote_address("100.119.234.95:12300"),
            ("http://100.119.234.95:12300".into(), None)
        );
        // 没带端口补默认端口
        assert_eq!(
            split_remote_address("100.64.0.1"),
            ("http://100.64.0.1:12300".into(), None)
        );
        // 段边界：100.63 不算、100.128 不算、100.127 算
        assert_eq!(
            split_remote_address("100.63.1.1:12300"),
            ("https://100.63.1.1:12300".into(), None)
        );
        assert_eq!(
            split_remote_address("100.128.1.1:12300"),
            ("https://100.128.1.1:12300".into(), None)
        );
        assert_eq!(
            split_remote_address("100.127.255.255:9"),
            ("http://100.127.255.255:9".into(), None)
        );
        // 显式 http:// 的 100.x 原样保留（不重复补端口）
        assert_eq!(
            split_remote_address("http://100.1.2.3"),
            ("http://100.1.2.3".into(), None)
        );
        // .ts.net 域名不算 CGNAT 裸 IP（有 scheme 或 https 默认）
        assert_eq!(
            split_remote_address("win-pc.tailabc.ts.net"),
            ("https://win-pc.tailabc.ts.net".into(), None)
        );
        // is_tailscale_host 直接断言
        assert!(is_tailscale_host("100.64.0.1"));
        assert!(is_tailscale_host("100.119.234.95:12300"));
        assert!(!is_tailscale_host("100.63.0.1"));
        assert!(!is_tailscale_host("192.168.1.10"));
        assert!(!is_tailscale_host("fd7a::1"));
        assert!(!is_tailscale_host("host.ts.net"));
    }

    #[test]
    fn split_remote_address_forms() {
        // 整条配对串：拆 tunnel + token
        assert_eq!(
            split_remote_address(
                "anyboot://pair?gateway=%E4%B9%A6%E6%88%BF&tunnel=https%3A%2F%2Fabc.trycloudflare.com&token=gw1"
            ),
            ("https://abc.trycloudflare.com".into(), Some("gw1".into()))
        );
        // 配对串 tunnel 未编码的普通形态也认
        assert_eq!(
            split_remote_address("anyboot://pair?tunnel=https://abc.trycloudflare.com&token=gw2"),
            ("https://abc.trycloudflare.com".into(), Some("gw2".into()))
        );
        // 整条 URL + ab_token
        assert_eq!(
            split_remote_address("https://abc-def.trycloudflare.com/?ab_token=t1"),
            ("https://abc-def.trycloudflare.com".into(), Some("t1".into()))
        );
        // 带 token= 旧式也认
        assert_eq!(
            split_remote_address("https://host.ts.net?token=t2"),
            ("https://host.ts.net".into(), Some("t2".into()))
        );
        // 裸 host 默认补 https
        assert_eq!(
            split_remote_address(" my-tunnel.example.com "),
            ("https://my-tunnel.example.com".into(), None)
        );
        // http 保留原 scheme
        assert_eq!(
            split_remote_address("http://1.2.3.4:8080/x"),
            ("http://1.2.3.4:8080".into(), None)
        );
        // 空 / 全是路径 → 空
        assert_eq!(split_remote_address("  "), (String::new(), None));
        assert_eq!(split_remote_address("https:///path"), (String::new(), None));
    }

    /// 三种探测模式的语义：并发首个 = 最快胜出；并发全部 = 逐行全报；
    /// 串行 = 按顺序第一个通的胜出。用一个真 /health 监听 + 一个死端口验证。
    #[test]
    fn probe_modes_pick_live_address_and_report_every_candidate() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("监听回环");
        let port = listener.local_addr().expect("本地地址").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buffer = [0u8; 512];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let body = "{\"ok\":true,\"machineId\":\"m1\",\"machineName\":\"测试机\"}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        let live = format!("127.0.0.1:{port}");
        let dead = "127.0.0.1:1".to_string(); // 端口 1 上不会有服务

        // 并发（全部）：逐行两条都在，赢家是活的那个
        let all = remote_test_blocking(vec![dead.clone(), live.clone()], "parallel_all".into(), None, false, PROBE_TIMEOUT);
        assert!(all.ok, "并发全部应探到活的地址");
        assert_eq!(all.addr, live);
        assert_eq!(all.results.len(), 2);
        assert!(all.results.iter().any(|o| o.address == dead && !o.ok));
        assert!(all.results.iter().any(|o| o.address == live && o.ok));

        // 并发（首个）：赢家也是活的（死的那个直接失败出局）
        let first = remote_test_blocking(vec![dead.clone(), live.clone()], "parallel_first".into(), None, false, PROBE_TIMEOUT);
        assert!(first.ok && first.addr == live);

        // 串行：先死后活，第一个成功的胜出
        let serial = remote_test_blocking(vec![dead, live.clone()], "serial".into(), None, false, PROBE_TIMEOUT);
        assert!(serial.ok && serial.addr == live);
    }

    /// 远程兜底语义：局域网全挂 → 探远程（赢家带 remote 标记）；
    /// 局域网有赢家 → 远程不探（results 里补一行「未探测」）。
    #[test]
    fn remote_test_lan_first_remote_fallback() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("监听回环");
        let port = listener.local_addr().expect("本地地址").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buffer = [0u8; 512];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let body = "{\"ok\":true,\"machineId\":\"m1\",\"machineName\":\"测试机\"}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        // 本地 http 监听充当「远程」（ureq 对 http origin 照常工作；线上远程是 https）
        let origin = format!("http://127.0.0.1:{port}");
        let live = format!("127.0.0.1:{port}");

        // 局域网全挂 → 远程兜底，赢家 = 远程 origin 且 remote: true
        let fallback = remote_test_blocking(
            vec!["127.0.0.1:1".into()],
            "parallel_first".into(),
            Some(vec![origin.clone()]),
            false, PROBE_TIMEOUT);
        assert!(fallback.ok, "局域网全挂时应兜底远程");
        assert!(fallback.remote, "赢家应带远程标记");
        assert_eq!(fallback.addr, origin);
        assert_eq!(fallback.machine_id, "m1");
        assert!(fallback.results.iter().any(|o| o.remote && o.ok));

        // 局域网有赢家 → 远程不探，results 里带一行「未探测」
        let lan_up = remote_test_blocking(vec![live], "parallel_first".into(), Some(vec![origin]), false, PROBE_TIMEOUT);
        assert!(lan_up.ok && !lan_up.remote, "局域网赢家优先");
        assert!(
            lan_up.results.iter().any(|o| o.remote && !o.ok && o.error.contains("未探测")),
            "远程未探测要有说明行"
        );
    }

    /// 远程优先语义：远程通了 → 直接用隧道（局域网整轮不探，逐行补「未探测」说明）；
    /// 远程挂了 → 退回局域网链路（赢家不带 remote 标记，逐行补远程失败行）。
    #[test]
    fn remote_test_remote_first_prefers_tunnel() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("监听回环");
        let port = listener.local_addr().expect("本地地址").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buffer = [0u8; 512];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let body = "{\"ok\":true,\"machineId\":\"m1\",\"machineName\":\"测试机\"}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        let origin = format!("http://127.0.0.1:{port}");
        let live = format!("127.0.0.1:{port}");

        // 远程优先 + 隧道通：赢家就是远程（哪怕局域网也有活地址也不探）
        let first = remote_test_blocking(
            vec![live.clone()],
            "parallel_first".into(),
            Some(vec![origin.clone()]),
            true, PROBE_TIMEOUT);
        assert!(first.ok && first.remote, "远程优先应选隧道");
        assert_eq!(first.addr, origin);
        assert!(
            first.results.iter().any(|o| !o.remote && !o.ok && o.error.contains("未探测")),
            "局域网行要有「未探测」说明"
        );

        // 远程优先 + 隧道挂：退回局域网，赢家不带 remote 标记，逐行带远程失败行
        let down = remote_test_blocking(
            vec![live.clone()],
            "parallel_first".into(),
            Some(vec!["http://127.0.0.1:1".into()]),
            true, PROBE_TIMEOUT);
        assert!(down.ok && !down.remote, "隧道挂了应退回局域网");
        assert_eq!(down.addr, live);
        assert!(
            down.results.iter().any(|o| o.remote && !o.ok),
            "远程失败要有逐行说明"
        );
    }

    #[test]
    fn normalize_keeps_explicit_token_over_url() {
        let server = RemoteServer {
            id: String::new(),
            name: String::new(),
            addresses: vec!["http://192.168.0.120:12300/?token=urltoken".into()],
            token: " explicit ".into(),
            machine_id: String::new(),
            machine_name: String::new(),
            machine_os: String::new(),
            probe_timeout_ms: 0,
            route_timeouts_ms: Default::default(),
            probe_mode: "serial".into(),
            route_probe_mode: "parallel_first".into(),
            jump_id: String::new(),
            macs: Default::default(),
            power_routes: vec![],
            conn_mode: String::new(),
            route_order: Vec::new(),
            route_disabled: Vec::new(),
            created_at: 0,
            pairs: Vec::new(),
            block_probe_mode: Default::default(),
            block_timeouts_ms: Default::default(),
        };
        let normalized = server.normalize();
        assert_eq!(normalized.token, "explicit");
        assert_eq!(normalized.probe_mode, "serial");
    }

    #[test]
    fn store_roundtrip_defaults() {
        let path = std::env::temp_dir().join(format!("ab-remote-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        // 不存在 = 默认空表
        assert!(load_store_from(&path).servers.is_empty());
        assert!(!load_store_from(&path).connect_all);
        // 写入再读回
        let store = RemoteStore {
            connect_all: true,
            servers: vec![RemoteServer {
                id: "rmt-1".into(),
                name: "n".into(),
                addresses: vec!["127.0.0.1:12300".into()],
                token: "t".into(),
                machine_id: String::new(),
                machine_name: String::new(),
                machine_os: String::new(),
                probe_timeout_ms: 0,
                route_timeouts_ms: Default::default(),
                probe_mode: "parallel".into(),
                route_probe_mode: "parallel_first".into(),
                jump_id: String::new(),
                macs: Default::default(),
                power_routes: vec![],
                conn_mode: String::new(),
            route_order: Vec::new(),
            route_disabled: Vec::new(),
                created_at: 42,
                pairs: Vec::new(),
                block_probe_mode: Default::default(),
                block_timeouts_ms: Default::default(),
            }],
        };
        save_store_to(&path, &store).expect("落盘");
        let loaded = load_store_from(&path);
        assert_eq!(loaded, store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remote_target_forms() {
        // 裸 host → 默认端口
        assert_eq!(remote_target("192.168.0.120"), ("192.168.0.120".into(), 12300));
        // host:port
        assert_eq!(remote_target("10.0.0.8:12400"), ("10.0.0.8".into(), 12400));
        // URL 形态（split_address 拆出 authority 与端口）
        assert_eq!(remote_target("http://192.168.0.120:12300/"), ("192.168.0.120".into(), 12300));
        assert_eq!(remote_target("  http://home.example.com  "), ("home.example.com".into(), 12300));
    }

    #[test]
    fn remote_forward_arg_shape() {
        assert_eq!(remote_forward_arg(15300, "192.168.0.120", 12300), "127.0.0.1:15300:192.168.0.120:12300");
    }

    #[test]
    fn remote_test_reports_missing_addresses() {
        let result = remote_test_blocking(vec![" ".into()], "parallel".into(), None, false, PROBE_TIMEOUT);
        assert!(!result.ok);
        assert_eq!(result.error, "没有可探测的地址");
    }

    #[test]
    fn grub_class_prefers_windows_then_mac() {
        assert_eq!(grub_class("Windows Boot Manager", "windows", true), "win");
        // Ubuntu 里带 win 字样的自定义条目（"Windows 11 (on /dev/nvme0n1p1)"）
        assert_eq!(grub_class("Windows 11 (on /dev/nvme0n1p1)", "", false), "win");
        assert_eq!(grub_class("macOS Sequoia", "osx", true), "mac");
        assert_eq!(grub_class("Apple Recovery", "", false), "mac");
        assert_eq!(grub_class("Ubuntu", "ubuntu", false), "linux");
        assert_eq!(grub_class("UEFI Firmware Settings", "", true), "auto");
    }

    #[test]
    fn refind_from_status_picks_active_and_skips_missing() {
        let value = serde_json::json!({
            "ok": true,
            "variables": [
                {"system": "linux", "displayName": "Ubuntu 24.04 LTS", "exists": true, "active": false},
                {"system": "windows", "displayName": "", "exists": true, "active": true},
                // exists=false 的变量不该出现在下拉里（那台系统没装）
                {"system": "macos", "displayName": "macOS Sequoia", "exists": false, "active": false}
            ]
        });
        let group = refind_from_status(&value);
        assert!(group.available);
        assert_eq!(group.kind, "refind");
        assert_eq!(group.current, "windows");
        assert_eq!(group.options.len(), 2);
        // displayName 为空时退回 system，不能出空标签
        assert_eq!(group.options[1].label, "windows");
        assert_eq!(group.options[1].class, "win");
        assert!(group.note.is_empty());
    }

    #[test]
    fn refind_from_status_all_missing_is_unavailable() {
        let group = refind_from_status(&serde_json::json!({"variables": [
            {"system": "linux", "exists": false, "active": false}
        ]}));
        assert!(!group.available);
        assert!(group.options.is_empty());
        assert!(group.note.contains("没部署"));
    }

    #[test]
    fn grub_from_json_escapes_nothing_but_keeps_selector_verbatim() {
        let value = serde_json::json!({
            "installed": true,
            "savedEntry": "0>1",
            "nextEntry": "",
            "entries": [
                {"selector": "0", "name": "Ubuntu", "submenu": "", "class": "ubuntu", "chainloader": false},
                {"selector": "0>1", "name": "Advanced options for Ubuntu", "submenu": "", "class": "", "chainloader": false},
                {"selector": "2", "name": "Windows Boot Manager", "submenu": "", "class": "windows", "chainloader": true}
            ]
        });
        let group = grub_from_json(&value, "raw");
        assert!(group.available);
        // nextEntry 为空 → 用 savedEntry 当「下次启动」
        assert_eq!(group.current, "0>1");
        assert_eq!(group.next, "");
        assert_eq!(group.saved, "0>1");
        // 当前选中项置顶（第一条），原位照常保留 —— 与托盘同一行为
        assert_eq!(group.options[0].value, "0>1");
        assert_eq!(group.options[1].value, "0");
        assert_eq!(group.options[2].value, "0>1");
        assert_eq!(group.options[3].class, "win");
        // selector 原样保留（含 `>`，切换时要转义但不能被改写）
        assert_eq!(group.options[0].value, "0>1");
    }

    #[test]
    fn grub_from_json_submenu_prefixes_label_once() {
        let value = serde_json::json!({
            "installed": true,
            "entries": [
                {"selector": "1>0", "name": "Ubuntu, with Linux 6.8.0", "submenu": "Advanced options for Ubuntu", "class": "", "chainloader": false},
                {"selector": "1>1", "name": "Advanced options for Ubuntu (recovery mode)", "submenu": "Advanced options for Ubuntu", "class": "", "chainloader": false}
            ]
        });
        let group = grub_from_json(&value, "raw");
        // 与托盘同一结构：一行组头（计数）+ 子项 ↳ 缩进
        assert!(group.options[0].header);
        assert!(group.options[0].label.contains("（2 项）"), "{}", group.options[0].label);
        assert!(!group.options[1].header);
        assert_eq!(group.options[1].label, "↳ 🐧 Ubuntu, with Linux 6.8.0");
        assert_eq!(group.options[2].label, "↳ Advanced options for Ubuntu (recovery mode)");
    }

    /// 子菜单的 `submenu` 是内部 id（`gnulinux-advanced-<uuid>`），下拉里必须换成
    /// `groups` 里的组名 —— GRUB 列表页与托盘菜单都这么翻，电脑列表以前漏了这步。
    #[test]
    fn grub_from_json_maps_submenu_id_to_group_name() {
        let value = serde_json::json!({
            "installed": true,
            "groups": [
                {"id": "gnulinux-advanced-9f1bae08-f61b-4e7a-9277-8e00b3970c81",
                 "name": "Advanced options for CachyOS Linux"}
            ],
            "entries": [
                {"selector": "gnulinux-advanced-9f1bae08-f61b-4e7a-9277-8e00b3970c81>0",
                 "name": "CachyOS Linux, with Linux linux-cachyos",
                 "submenu": "gnulinux-advanced-9f1bae08-f61b-4e7a-9277-8e00b3970c81",
                 "class": "cachyos", "chainloader": false, "kernels": ["vmlinuz-linux-cachyos"]}
            ]
        });
        let group = grub_from_json(&value, "minimal");
        assert_eq!(
            group.options[0].label,
            "▸ 🐧 Advanced options for CachyOS Linux（1 项）"
        );
        assert_eq!(group.options[1].label, "↳ 🐧 cachyos");
        assert!(
            !group.options.iter().any(|option| option.label.contains("gnulinux-advanced")),
            "下拉文字里不该出现内部 id"
        );
    }

    /// 内核名是空串时不能出一行空白（要退回菜单标题）。
    #[test]
    fn grub_row_label_falls_back_to_name_when_kernel_is_blank() {
        let entry = crate::grub::GrubEntry {
            selector: "0".to_string(),
            id: "0".to_string(),
            name: "CachyOS Linux".to_string(),
            submenu: None,
            class: None,
            chainloader: false,
            kernels: vec!["   ".to_string()],
            variants: 1,
        };
        assert_eq!(crate::menu_model::grub_row_label("minimal", &entry), "CachyOS Linux");
        assert_eq!(crate::menu_model::grub_row_label("medium", &entry), "CachyOS Linux");
    }

    #[test]
    fn grub_labels_follow_the_configured_list_format() {
        let value = serde_json::json!({
            "installed": true,
            "entries": [
                {"selector": "0", "name": "CachyOS Linux", "submenu": "", "class": "cachyos",
                 "chainloader": false, "kernels": ["vmlinuz-linux-cachyos"]},
                {"selector": "1", "name": "Ubuntu", "submenu": "", "class": "ubuntu",
                 "chainloader": false, "kernels": ["vmlinuz-6.8.0-45-generic"]}
            ]
        });
        // 极简 / 简易：去掉 vmlinuz- 与 linux- 前缀（行首是系统图标，按标题判）
        assert_eq!(grub_from_json(&value, "minimal").options[0].label, "🐧 cachyos");
        assert_eq!(grub_from_json(&value, "simple").options[1].label, "6.8.0-45-generic");
        // 中等：完整内核名
        assert_eq!(
            grub_from_json(&value, "medium").options[1].label,
            "vmlinuz-6.8.0-45-generic"
        );
        // 原始 / 复杂：标题原文 + 行尾附内核名
        assert_eq!(
            grub_from_json(&value, "raw").options[0].label,
            "🐧 CachyOS Linux · vmlinuz-linux-cachyos"
        );
        assert_eq!(
            grub_from_json(&value, "detailed").options[0].label,
            "🐧 CachyOS Linux · vmlinuz-linux-cachyos"
        );
        // 没有 kernels 时退回标题（不猜）
        let plain = serde_json::json!({
            "installed": true,
            "entries": [
                {"selector": "0", "name": "Windows Boot Manager", "class": "windows", "chainloader": true}
            ]
        });
        assert_eq!(
            grub_from_json(&plain, "minimal").options[0].label,
            "🪟 Windows Boot Manager"
        );
    }

    /// 与托盘菜单同款的整体结构：置顶当前项（组名 › 前缀）+ 组头（计数与 ESP
    /// 位置）+ 子项缩进 + 同名折叠。
    #[test]
    fn grub_from_json_matches_tray_composition() {
        let value = serde_json::json!({
            "installed": true,
            "savedEntry": "g>1",
            "efiDir": "H:\\",
            "partuuid": "abcd-1234",
            "groups": [{"id": "g", "name": "Advanced options for CachyOS"}],
            "entries": [
                {"selector": "0", "name": "CachyOS Linux", "class": "cachyos",
                 "chainloader": false, "kernels": ["vmlinuz-linux-cachyos"]},
                {"selector": "g>0", "name": "CachyOS Linux, with Linux linux-cachyos",
                 "submenu": "g", "class": "cachyos", "chainloader": false,
                 "kernels": ["vmlinuz-linux-cachyos"]},
                {"selector": "g>1", "name": "CachyOS Linux, with Linux linux-cachyos (fallback)",
                 "submenu": "g", "class": "cachyos", "chainloader": false,
                 "kernels": ["vmlinuz-linux-cachyos-fallback"]}
            ]
        });
        let group = grub_from_json(&value, "simple");
        // 置顶行在最前，带「组名 › 」前缀
        assert_eq!(group.options[0].value, "g>1");
        assert!(
            group.options[0].label.contains("Advanced options for CachyOS › cachyos-fallback"),
            "{}",
            group.options[0].label
        );
        // 顶层条目照常在原位出现（置顶行之外它还在）
        assert_eq!(group.options[1].value, "0");
        // 组头带计数与 ESP 位置（simple 档带，极简/原始不带）
        assert!(group.options[2].header);
        assert!(group.options[2].label.contains("（2 项"), "{}", group.options[2].label);
        assert!(group.options[2].label.contains("位于H:\\ UUID=abcd-1234"));
        // 子项缩进
        assert!(group.options[3].label.starts_with("↳ "));
        // 极简档组头不带位置
        let minimal = grub_from_json(&value, "minimal");
        assert!(minimal.options[2].label.contains("（2 项）"));
        assert!(!minimal.options[2].label.contains("位于"));

        // 同名条目折叠成一条并标变体数（非原始格式；原始格式不折叠）
        let dup = serde_json::json!({
            "installed": true,
            "entries": [
                {"selector": "m1", "name": "Memory test", "class": "memtest", "chainloader": false},
                {"selector": "m2", "name": "Memory test", "class": "memtest", "chainloader": false}
            ]
        });
        let folded = grub_from_json(&dup, "minimal");
        assert_eq!(folded.options.len(), 1);
        assert!(folded.options[0].label.contains("2 个变体"), "{}", folded.options[0].label);
        let unfolded = grub_from_json(&dup, "raw");
        assert_eq!(unfolded.options.len(), 2);
    }

    #[test]
    fn grub_from_json_without_entries_reports_reason() {
        let group = grub_from_json(
            &serde_json::json!({"installed": false, "error": "没找到 grub.cfg"}),
            "raw",
        );
        assert!(!group.available);
        assert_eq!(group.note, "没找到 grub.cfg");
    }

    #[test]
    fn boot_option_value_is_url_escaped_on_switch() {        // GRUB selector 里的 `>` 与空格必须转义，否则服务端 query 解析会截断
        assert_eq!(encode_query("0>1"), "0%3E1");
        assert_eq!(encode_query("Advanced options"), "Advanced%20options");
    }

    // ---- 五通道优先级（route_order / route_disabled / route_candidates） ----

    /// 起一个只回 /health 的假服务端，返回监听端口
    fn spawn_health_stub(os: &'static str) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("监听回环");
        let port = listener.local_addr().expect("本地地址").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buffer = [0u8; 512];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let body = format!(
                    "{{\"ok\":true,\"machineId\":\"m1\",\"machineName\":\"测试机\",\"os\":\"{os}\"}}"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        port
    }

    #[test]
    fn probe_timeout_is_clamped() {
        // 0 / 缺省 → 默认 2 秒；太小太大都夹回合法区间
        assert_eq!(clamp_probe_timeout_ms(0), DEFAULT_PROBE_TIMEOUT_MS);
        assert_eq!(clamp_probe_timeout_ms(1), MIN_PROBE_TIMEOUT_MS);
        assert_eq!(clamp_probe_timeout_ms(2_000), 2_000);
        assert_eq!(clamp_probe_timeout_ms(999_999), MAX_PROBE_TIMEOUT_MS);
    }

    #[test]
    fn probe_candidates_serial_stops_at_first_success() {
        let port = spawn_health_stub("windows");
        // 三个都是 lan（= 同一通道的多块网卡）→ 走**内层**串行：命中即停
        let cands = vec![
            RouteTestCandidate { kind: "lan".into(), origin: "127.0.0.1:1".into(), ..Default::default() },
            RouteTestCandidate { kind: "lan".into(), origin: format!("127.0.0.1:{port}"), ..Default::default() },
            RouteTestCandidate { kind: "lan".into(), origin: "127.0.0.1:2".into(), ..Default::default() },
        ];
        let result = probe_candidates_blocking(cands, "parallel_first", "serial", &no_route_timeouts(), DEFAULT_PROBE_TIMEOUT_MS, &no_block_modes());
        assert!(result.ok, "串行应探到活着的那条：{}", result.error);
        assert_eq!(result.addr, format!("127.0.0.1:{port}"));
        assert_eq!(result.os, "windows", "赢家要带回系统族");
        // 逐行结果仍和地址表对得上：第三条没探，标「未探测」而不是凭空消失
        assert_eq!(result.results.len(), 3);
        let skipped = result.results.iter().find(|row| row.address == "127.0.0.1:2").expect("第三条");
        assert!(!skipped.ok);
        assert!(skipped.error.contains("未探测"), "未探的地址要说明原因：{}", skipped.error);
    }

    #[test]
    fn route_serial_probes_channels_in_priority_order() {
        // 外层（通道之间）串行：局域网死 → 隧道活 → 后面的通道压根不探
        let lan_port = spawn_health_stub("windows");
        let tunnel_port = spawn_health_stub("linux");
        let cands = vec![
            RouteTestCandidate { kind: "lan".into(), origin: "127.0.0.1:1".into(), ..Default::default() },
            RouteTestCandidate { kind: "tunnel".into(), origin: format!("http://127.0.0.1:{tunnel_port}"), ..Default::default() },
            RouteTestCandidate { kind: "ts".into(), origin: format!("http://127.0.0.1:{lan_port}"), ..Default::default() },
        ];
        let result = probe_candidates_blocking(cands, "serial", "parallel_first", &no_route_timeouts(), DEFAULT_PROBE_TIMEOUT_MS, &no_block_modes());
        assert!(result.ok, "外层串行应停在隧道通道：{}", result.error);
        assert_eq!(result.kind, "tunnel");
        // 三个通道各一行，但第三个（ts）连探都没探
        assert_eq!(result.results.len(), 3);
        let untouched = result.results.iter().find(|row| row.kind == "ts").expect("ts 那行");
        assert!(!untouched.ok);
        assert!(
            untouched.error.contains("串行策略未探测"),
            "串行命中后剩下的通道要说明没探：{}",
            untouched.error
        );
    }

    #[test]
    fn probe_candidates_parallel_all_reports_every_channel() {
        let port = spawn_health_stub("linux");
        let cands = vec![
            RouteTestCandidate { kind: "lan".into(), origin: "127.0.0.1:1".into(), ..Default::default() },
            RouteTestCandidate { kind: "tunnel".into(), origin: format!("http://127.0.0.1:{port}"), ..Default::default() },
        ];
        let result = probe_candidates_blocking(cands, "parallel_all", "parallel_first", &no_route_timeouts(), DEFAULT_PROBE_TIMEOUT_MS, &no_block_modes());
        assert!(result.ok, "并发全部应探到远程通道：{}", result.error);
        assert!(result.remote, "赢家是远程通道");
        assert_eq!(result.kind, "tunnel");
        assert_eq!(result.os, "linux");
        assert_eq!(result.results.len(), 2, "并发全部要逐行给出每个通道");
        assert!(result.results.iter().any(|row| !row.ok), "死通道也要有一行失败");
    }

    #[test]
    fn probe_candidates_parallel_first_returns_a_winner() {
        let port = spawn_health_stub("macos");
        let cands = vec![
            RouteTestCandidate { kind: "lan".into(), origin: "127.0.0.1:1".into(), ..Default::default() },
            RouteTestCandidate { kind: "lan".into(), origin: format!("127.0.0.1:{port}"), ..Default::default() },
        ];
        let result = probe_candidates_blocking(cands, "parallel_first", "parallel_first", &no_route_timeouts(), DEFAULT_PROBE_TIMEOUT_MS, &no_block_modes());
        assert!(result.ok);
        assert_eq!(result.os, "macos");
    }

    #[test]
    fn probe_candidates_all_dead_reports_every_failure() {
        let cands = vec![
            RouteTestCandidate { kind: "lan".into(), origin: "127.0.0.1:1".into(), ..Default::default() },
            RouteTestCandidate { kind: "ts".into(), origin: "http://127.0.0.1:2".into(), ..Default::default() },
        ];
        let result = probe_candidates_blocking(cands, "parallel_all", "parallel_first", &no_route_timeouts(), 300, &no_block_modes());
        assert!(!result.ok);
        assert_eq!(result.results.len(), 2);
        assert!(result.error.contains("所有通道都不通"), "错误文案：{}", result.error);
    }

    #[test]
    fn probe_candidates_empty_is_an_error_not_a_panic() {
        let result = probe_candidates_blocking(Vec::new(), "parallel_first", "parallel_first", &no_route_timeouts(), DEFAULT_PROBE_TIMEOUT_MS, &no_block_modes());
        assert!(!result.ok);
        assert!(result.error.contains("没有可探测的通道"));
    }

    #[test]
    fn route_timeouts_are_cleaned_per_channel() {
        // 非法通道 id 丢掉；值 0 = 「没单独设」→ 删掉（回落默认）；越界值夹紧
        let mut raw = std::collections::BTreeMap::new();
        raw.insert(" LAN ".to_string(), 5_000);
        raw.insert("funnel".to_string(), 0);
        raw.insert("tsnet".to_string(), 999_999);
        raw.insert("bogus".to_string(), 3_000);
        let cleaned = normalize_route_timeouts(&raw);
        assert_eq!(cleaned.get("lan"), Some(&5_000), "通道 id 要 trim + 小写");
        assert!(!cleaned.contains_key("funnel"), "0 = 没单独设，不该留下");
        assert_eq!(cleaned.get("tsnet"), Some(&MAX_PROBE_TIMEOUT_MS));
        assert!(!cleaned.contains_key("bogus"), "非法通道 id 要丢掉");
    }

    #[test]
    fn probe_timeout_for_falls_back_to_the_default() {
        let mut server = route_test_server();
        server.probe_timeout_ms = 2_500;
        server.route_timeouts_ms.insert("funnel".into(), 8_000);
        assert_eq!(
            server.probe_timeout_for("funnel"),
            Duration::from_millis(8_000),
            "单独设过的通道用自己的值"
        );
        assert_eq!(
            server.probe_timeout_for("lan"),
            Duration::from_millis(2_500),
            "没单独设的通道回落默认值"
        );
        // 0 等于没设（normalize 里会被删掉，这里防手写数据绕过 normalize）
        server.route_timeouts_ms.insert("ts".into(), 0);
        assert_eq!(server.probe_timeout_for("ts"), Duration::from_millis(2_500));
    }

    /// 起一个**慢**服务端：先睡 delay 再回 /health（用来验证「各通道用自己的超时」）
    fn spawn_slow_health_stub(delay: Duration, os: &'static str) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("监听回环");
        let port = listener.local_addr().expect("本地地址").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                std::thread::sleep(delay);
                let mut buffer = [0u8; 512];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let body = format!(
                    "{{\"ok\":true,\"machineId\":\"m1\",\"machineName\":\"慢机\",\"os\":\"{os}\"}}"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
            }
        });
        port
    }

    #[test]
    fn probe_uses_each_channels_own_timeout() {
        // 服务端固定慢 700ms：同一台机器上，短超时的通道该失败、长超时的通道该成功。
        // 若引擎还在用「一个全局超时」，两行结果必然一样 —— 这个测试就是钉住这点。
        let port = spawn_slow_health_stub(Duration::from_millis(700), "linux");
        let mut timeouts = std::collections::BTreeMap::new();
        timeouts.insert("lan".to_string(), 200);
        timeouts.insert("ts".to_string(), 3_000);
        let cands = vec![
            RouteTestCandidate { kind: "lan".into(), origin: format!("127.0.0.1:{port}"), ..Default::default() },
            RouteTestCandidate { kind: "ts".into(), origin: format!("http://127.0.0.1:{port}"), ..Default::default() },
        ];
        let result = probe_candidates_blocking(cands, "parallel_all", "parallel_first", &timeouts, 2_000, &no_block_modes());
        assert!(result.ok, "3 秒超时的通道要能等到 700ms 的响应：{}", result.error);
        assert_eq!(result.kind, "ts", "赢家应该是给足超时的那条");
        assert_eq!(result.results.len(), 2);
        let short = result.results.iter().find(|row| row.kind == "lan").expect("短超时那行");
        assert!(!short.ok, "200ms 等不到 700ms 的响应，必须失败");
        let long = result.results.iter().find(|row| row.kind == "ts").expect("长超时那行");
        assert!(long.ok, "3s 应该成功，逐行结果各按自己的超时判定");
    }

    #[test]
    fn probe_uses_each_candidate_own_timeout_and_reports_pair_id() {
        // 同一个通道（tunnel）里的两条：短超时那条失败、长超时那条成功。
        // 若引擎对整个通道只用一个超时，两行结果必然一样 —— 这个测试钉住「条目级超时」。
        let short_port = spawn_slow_health_stub(Duration::from_millis(700), "windows");
        let long_port = spawn_slow_health_stub(Duration::from_millis(700), "windows");
        let cands = vec![
            RouteTestCandidate {
                kind: "tunnel".into(),
                origin: format!("http://127.0.0.1:{short_port}"),
                timeout_ms: 200,
                pair_id: "pair-short".into(),
            },
            RouteTestCandidate {
                kind: "tunnel".into(),
                origin: format!("http://127.0.0.1:{long_port}"),
                timeout_ms: 3_000,
                pair_id: "pair-long".into(),
            },
        ];
        let result = probe_candidates_blocking(
            cands,
            "parallel_all",
            "parallel_first",
            &std::collections::BTreeMap::new(),
            2_000,
            &no_block_modes(),
        );
        assert!(result.ok, "长超时那条要能等到 700ms 的响应：{}", result.error);
        assert_eq!(result.results.len(), 2);
        let short = result.results.iter().find(|row| row.pair_id == "pair-short").expect("短超时那行");
        assert!(!short.ok, "200ms 等不到 700ms 的响应，必须失败");
        let long = result.results.iter().find(|row| row.pair_id == "pair-long").expect("长超时那行");
        assert!(long.ok, "3s 应该成功");
        assert_eq!(result.kind, "tunnel");
        // 赢家 = 成功那条的地址
        assert_eq!(result.addr, long.address);
    }

    #[test]
    fn probe_block_serial_respects_pair_order() {
        // 块内串行：第一条就通 → 后面的标「未探测」（逐行仍要齐全）。
        // 策略来自 **block_probe_mode[cloudflare]**，不是多网卡设置 —— 多网卡这里显式给
        // parallel_first，串行必须照样生效，否则等于「区块策略没接线」。
        let port = spawn_slow_health_stub(Duration::from_millis(0), "macos");
        let dead = "http://127.0.0.1:1";
        let cands = vec![
            RouteTestCandidate {
                kind: "tunnel".into(),
                origin: format!("http://127.0.0.1:{port}"),
                timeout_ms: 2_000,
                pair_id: "p1".into(),
            },
            RouteTestCandidate {
                kind: "tunnel".into(),
                origin: dead.into(),
                timeout_ms: 500,
                pair_id: "p2".into(),
            },
        ];
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert("cloudflare".to_string(), "serial".to_string());
        let result = probe_candidates_blocking(
            cands,
            "parallel_all",
            "parallel_first",
            &std::collections::BTreeMap::new(),
            2_000,
            &blocks,
        );
        assert!(result.ok);
        assert_eq!(result.results.len(), 2);
        assert!(result.results[0].ok);
        assert_eq!(result.results[0].pair_id, "p1");
        assert!(!result.results[1].ok, "第一条已命中，第二条不该被探");
        assert!(result.results[1].error.contains("未探测"));
    }

    #[test]
    fn block_probe_mode_applies_by_block_not_by_channel() {
        // 区块粒度的设置落到通道组上：ts 配串行 → ts 与 funnel 两组都串行；
        // cloudflare 没配 → 落多网卡设置（parallel_all）；lan 永远听多网卡设置。
        let mut blocks = std::collections::BTreeMap::new();
        blocks.insert("ts".to_string(), "serial".to_string());
        let list = vec![
            RouteTestCandidate { kind: "tunnel".into(), origin: "https://a.example.com".into(), timeout_ms: 1_000, pair_id: "p1".into() },
            RouteTestCandidate { kind: "ts".into(), origin: "http://100.64.0.2:12300".into(), timeout_ms: 1_000, pair_id: "p2".into() },
            RouteTestCandidate { kind: "funnel".into(), origin: "https://b.ts.net".into(), timeout_ms: 1_000, pair_id: "p3".into() },
            RouteTestCandidate { kind: "lan".into(), origin: "192.168.1.9:12300".into(), timeout_ms: 1_000, pair_id: String::new() },
        ];
        let groups = group_candidates(&list, &blocks, "parallel_all");
        assert_eq!(groups.len(), 4, "四个通道各一组");
        assert_eq!(groups[0].inner_mode, "parallel_all", "cloudflare 没配 → 多网卡设置");
        assert_eq!(groups[1].inner_mode, "serial", "ts 区块配了串行");
        assert_eq!(groups[2].inner_mode, "serial", "funnel 与 ts 同区块，同一份设置");
        assert_eq!(groups[3].inner_mode, "parallel_all", "lan 不属于任何区块");
    }

    #[test]
    fn block_probe_modes_drop_illegal_keys_and_values() {
        // 通道 id 当区块 key 传进来、拼错的值、空值 —— 全部丢弃（回落多网卡设置），
        // 不能把拼错的写成 parallel_first 悄悄存下来
        let mut raw = std::collections::BTreeMap::new();
        raw.insert("tunnel".to_string(), "serial".to_string());
        raw.insert("TS".to_string(), " parallel_all ".to_string());
        raw.insert("tsnet".to_string(), "paralel_all".to_string());
        raw.insert("cloudflare".to_string(), "".to_string());
        let modes = normalize_block_probe_modes(&raw);
        assert_eq!(modes.len(), 1);
        assert_eq!(modes.get("ts").map(String::as_str), Some("parallel_all"));
    }

    fn route_test_server() -> RemoteServer {
        RemoteServer {
            id: "r1".into(),
            addresses: vec!["192.168.1.50".into()],
            jump_id: "jump-1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn route_order_defaults_when_empty() {
        let mut server = route_test_server();
        server.route_order = Vec::new();
        server.conn_mode = String::new();
        assert_eq!(
            server.route_order_sorted(),
            vec!["lan", "jump", "tunnel", "ts", "funnel", "tsnet"]
        );
    }

    #[test]
    fn route_order_migrates_remote_first() {
        let mut server = route_test_server();
        server.route_order = Vec::new();
        server.conn_mode = "remote_first".into();
        assert_eq!(
            server.route_order_sorted(),
            vec!["tunnel", "ts", "funnel", "tsnet", "jump", "lan"]
        );
    }

    #[test]
    fn route_order_sanitizes_and_appends_missing() {
        let mut server = route_test_server();
        server.route_order = vec!["funnel".into(), "bogus".into(), "funnel".into(), "ts".into()];
        let order = server.route_order_sorted();
        assert_eq!(&order[..2], &["funnel", "ts"]);
        // 其余四个按默认相对序补到尾部
        assert_eq!(&order[2..], &["lan", "jump", "tunnel", "tsnet"]);
        assert_eq!(order.len(), 6);
    }

    #[test]
    fn normalize_materializes_route_fields() {
        let mut server = route_test_server();
        server.route_order = Vec::new();
        // route_disabled 现在只认非配对通道（lan / jump）：区块禁用走 pair.enabled
        server.route_disabled = vec!["bogus".into(), " LAN ".into(), "tunnel".into()];
        let normalized = server.normalize();
        assert_eq!(
            normalized.route_order,
            vec!["lan", "jump", "tunnel", "ts", "funnel", "tsnet"]
        );
        assert_eq!(normalized.route_disabled, vec!["lan"]);
    }

    #[test]
    fn route_candidates_expand_every_pair_in_order() {
        // 一区块多条：全部展开，顺序 = 区块顺序 + 区块内顺序（不再一区块一条）
        let mut server = route_test_server();
        server.pairs = vec![
            pair("cloudflare", "https://a.trycloudflare.com", "m1"),
            pair("cloudflare", "https://b.trycloudflare.com", "m2"),
            pair("ts", "https://box.tail1234.ts.net", "m3"),
            pair("ts", "100.64.0.1:12300", "m4"),
        ];
        server.jump_id = String::new(); // 先把跳板机摘掉，候选里不留空 origin 的干扰项
        let server = server.normalize(); // 真实路径：落盘前会归一（补 scheme / 补 id）
        let candidates = server.route_candidates();
        let kinds: Vec<&str> = candidates.iter().map(|cand| cand.kind.as_str()).collect();
        let origins: Vec<&str> = candidates.iter().map(|cand| cand.origin.as_str()).collect();
        // 默认 route_order 是 lan 领头，配对区块按 tunnel → ts 跟在后面
        assert_eq!(kinds, vec!["lan", "tunnel", "tunnel", "funnel", "ts"]);
        assert_eq!(
            origins,
            vec![
                "192.168.1.50:12300",
                "https://a.trycloudflare.com",
                "https://b.trycloudflare.com",
                "https://box.tail1234.ts.net",
                "http://100.64.0.1:12300",
            ]
        );
        // 每条候选带自己的令牌与超时
        let mut timed = server.clone();
        timed.block_timeouts_ms.insert("cloudflare".to_string(), 4_000);
        let candidates = timed.route_candidates();
        assert_eq!(candidates[1].timeout_ms, 4_000, "cloudflare 区块的默认超时");
        assert_eq!(candidates[3].timeout_ms, DEFAULT_PROBE_TIMEOUT_MS);
    }

    #[test]
    fn route_candidates_skip_disabled_pair_and_channel() {
        let mut server = route_test_server();
        server.jump_id = String::new();
        server.pairs = vec![
            pair("cloudflare", "https://a.trycloudflare.com", "m1"),
            RemotePair { enabled: false, ..pair("cloudflare", "https://b.trycloudflare.com", "m2") },
        ];
        let kinds: Vec<String> = server
            .route_candidates()
            .iter()
            .map(|cand| cand.kind.clone())
            .collect();
        assert_eq!(kinds, vec!["lan", "tunnel"], "禁用的配对不参与");
        // 禁用局域网通道后只剩配对
        server.route_disabled = vec!["lan".into()];
        let kinds: Vec<String> = server
            .route_candidates()
            .iter()
            .map(|cand| cand.kind.clone())
            .collect();
        assert_eq!(kinds, vec!["tunnel"]);
    }

    #[test]
    fn route_candidates_block_order_follows_route_order() {
        // tsnet 排在最前 → 它的配对先被试（区块顺序从 route_order 推导）
        let mut server = route_test_server();
        server.jump_id = String::new();
        server.route_order = vec!["tsnet".into(), "tunnel".into(), "lan".into(), "jump".into(), "ts".into(), "funnel".into()];
        server.pairs = vec![
            pair("cloudflare", "https://a.trycloudflare.com", "m1"),
            pair("tsnet", "https://tsnet.tail.ts.net", "m2"),
        ];
        let kinds: Vec<String> = server
            .route_candidates()
            .iter()
            .map(|cand| cand.kind.clone())
            .collect();
        assert_eq!(kinds, vec!["tsnet", "tunnel", "lan"]);
    }
}
