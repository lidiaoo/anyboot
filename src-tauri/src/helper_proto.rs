//! 免密写 NVRAM 的请求协议（App ↔ root 助手）。
//!
//! 为什么要有一层协议，而不是 App 直接调提权命令：
//! macOS 上非 root 写 NVRAM 在内核层就被拒（实测 `nvram` 返回
//! `(iokit/common) general error`，那是被伪装成通用错误的权限拒绝），
//! 而 `osascript … with administrator privileges` 每写一次弹一次密码。
//! 想「装一次、之后免密」，只能让一个 **root 进程**代写 —— 那就是这里的助手。
//!
//! 走的是**文件信箱**而不是 socket：
//! - launchd 用 `WatchPaths` 盯着 [`inbox_dir`]，有文件就唤起助手（没有常驻进程、不占端口）；
//! - 请求内容与结果都是 JSON，落盘即「已送达」，不需要处理连接断裂、半包、重连。
//!
//! # 协议 v2：从「只写 NVRAM」扩到「读 Linux 分区 + 写 grubenv」
//!
//! v1 只会写 `OCBt*`。GRUB 自动读取模式要在 macOS 上读 ext4 分区（内核不让非 root
//! 打开 `/dev/rdisk*`），要么每次弹密码，要么让同一个 root 助手代读 —— 选了后者。
//!
//! **放宽了什么，一句话说完**：
//! 助手现在能读**所有 ext4 分区里的三个固定相对路径**（`etc/fstab`、
//! `boot/grub{,2}/grub.cfg`、`boot/grub{,2}/grubenv`），
//! 并能把其中一个（`grubenv`）的 1024 字节原地覆盖。
//!
//! **没有放宽的是什么**（这几条是边界，不是「暂时」）：
//! - 请求里**没有路径参数**。读什么由助手自己枚举分区再走固定相对路径决定；
//!   写什么由 [`LINUX_ENV_RELS`] 两个常量之一决定。想读 `/etc/shadow` 也没有入口。
//! - 设备名只认 `diskNsM` 形状（[`valid_disk_dev`]），**不接受 `/dev/...`** ——
//!   `r` 前缀由助手自己加，杜绝「拼一个别的设备节点」。
//! - 环境块必须**正好** 1024 字节；尺寸不对说明拿到的不是 GRUB 环境块，直接拒。
//! - 仍然不执行 shell、不接受任意变量名、不跟软链接。
//!
//! 安全边界写在这里，别在别处放宽：
//! 1. NVRAM 只接受 OpenCore 私有命名空间（[`OC_GUID`]）下的 `OCBtOrder` / `OCBtNext` /
//!    `OCBt####`。值只允许十六进制串，长度上限 [`MAX_VALUE`] 字节，单次最多 [`MAX_OPS`] 条。
//! 2. Linux 分区只认 [`LINUX_ENV_RELS`] 里的相对路径，读侧由助手自己枚举。
//! 3. 助手只处理 inbox 里**普通文件**（不跟软链接），文件名必须匹配 [`valid_request_name`]。
//!
//! 仍然存在的代价（必须如实写在 UI 与文档里）：装好之后，**任何以当前用户身份运行的
//! 进程**都能通过它改 OpenCore 的默认启动项、改任意一块 ext4 分区上的 GRUB 默认启动项，
//! 以及读那三个固定文件。最坏后果是「启动项被改 / `/etc/fstab` 被读走」，拿不到 root、
//! 也执行不了任意命令。不装则每一次读写都要密码 —— 二选一。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// OpenCore 私有命名空间的 GUID（`OCBt*` 都挂在这里）。
/// 与 `channels::nvram` 里 `GUID_OC` 同一个值，两处必须一致。
pub const OC_GUID: &str = "4D1FDA02-38C7-4A6A-9CC6-4BCCA8B30102";

/// 默认信箱根目录（root 拥有；`in` 用户名可写、只能新建不能列举）
pub const DEFAULT_SPOOL: &str = "/var/spool/anyboot";

/// 覆盖信箱位置的**仅测试用**环境变量。生产环境的 launchd 作业不设它，
/// 所以它不会成为「把助手指向别处」的入口。
pub const SPOOL_ENV: &str = "ANYBOOT_HELPER_SPOOL";

/// 单次最多写几条（OC 设默认项最多两条：`OCBtOrder` + 一条被复制的 `OCBt####`）
pub const MAX_OPS: usize = 8;

/// 单条变量值的字节上限（一条 `EFI_LOAD_OPTION` 实测 ~100 字节，512 足够宽松）
pub const MAX_VALUE: usize = 512;

/// 请求文件的内容上限：超过这个大小的文件一律不读（信箱目录是用户可写的，
/// 不能让助手把一块巨大的文件读进内存）。
pub const MAX_REQUEST_BYTES: u64 = 64 * 1024;

/// 结果载荷（Linux 探测的整包 JSON）上限。十块分区、每块几十 KB 的 `grub.cfg`，
/// 实测 2 MB 以内；给到 8 MB 是留余量，不是「允许这么大」。
pub const MAX_BLOB_BYTES: u64 = 8 * 1024 * 1024;

pub const PROTOCOL_VERSION: u32 = 2;

/// GRUB 环境块固定 1024 字节。和 `grub::ENV_BLOCK_SIZE` 必须一致（有单测锁）。
pub const GRUB_ENV_SIZE: usize = 1024;

/// 允许助手读写的两个 GRUB 目录 —— 这是 Linux 上仅有的两种布局：
/// Debian/Arch 系用 `boot/grub`，Fedora 系用 `boot/grub2`。
///
/// **相对路径写死成常量**，请求里只能指定「是哪一个」，不能自己拼路径。
pub const LINUX_ENV_RELS: [&str; 2] = ["boot/grub/grubenv", "boot/grub2/grubenv"];

/// 一次请求要做什么。用扁平结构 + `kind` 字段而不是 serde 的 internally-tagged enum：
/// `deny_unknown_fields` 对 internally-tagged 枚举不生效（serde 的已知限制），
/// 而「请求格式固定、多一个字段就拒」正是这一层要守住的东西。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// v1 的行为：写 `OCBt*` 变量
    Nvram,
    /// 枚举本机 ext4 分区，读 fstab / grub.cfg / grubenv，整包结果走 blob
    LinuxProbe,
    /// 把 1024 字节环境块原地覆盖回某块分区的 `grubenv`
    LinuxWriteEnv,
}

/// 一次变量写入：值已编码成十六进制串（JSON 里没法放裸字节）。
///
/// 用 hex 而不是 base64：`nvram` 本来就吃 `%XX` 串，十六进制在这里是「可肉眼核对」的
/// 表示 —— 出问题时用户能直接把 `hex` 与 `nvram -x OCBtOrder` 的输出对上。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Op {
    pub guid: String,
    pub name: String,
    pub hex: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub v: u32,
    pub kind: Kind,
    /// `kind == Nvram`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ops: Vec<Op>,
    /// `kind == LinuxProbe`：所选 ESP 的 PARTUUID（空串 = 不识别 ESP）。
    /// 只是**查询条件**，不是设备名 —— 助手拿它去自己的分区表里比对。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub esp: Option<String>,
    /// `kind == LinuxWriteEnv`：目标分区，形如 `disk0s9`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<String>,
    /// `kind == LinuxWriteEnv`：只能是 [`LINUX_ENV_RELS`] 里的一个
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    /// `kind == LinuxWriteEnv`：1024 字节环境块（hex）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
}

/// 校验通过、可以直接执行的作业。和 [`Request`] 分开是为了让**校验先于副作用**
/// 这件事在类型上成立：没通过 `validate` 就拿不到 `Job`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    Nvram(Vec<(String, String, Vec<u8>)>),
    LinuxProbe { esp: String },
    LinuxWriteEnv { dev: String, rel: String, block: Vec<u8> },
}

/// 助手写回的结果。`ok=false` 时 `error` 一定非空（前端要能原样显示）。
///
/// `blob` 是「大载荷另存」：Linux 探测的整包 JSON 有几百 KB，塞进回执会让每条回执
/// 都变成几 MB 的读写。写了 blob 时它已经**先于**回执落盘 —— 回执出现在 outbox
/// 就是「blob 已经完整」的信号（顺序在 [`write_blob`] 里是硬要求）。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

impl Reply {
    pub fn ok(detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            detail: Some(detail.into()),
            error: None,
            blob: None,
        }
    }

    /// 带载荷的成功回执：`blob` 是文件名（不含目录），调用方去 outbox 里读。
    pub fn ok_with_blob(detail: impl Into<String>, blob: impl Into<String>) -> Self {
        Self {
            ok: true,
            detail: Some(detail.into()),
            error: None,
            blob: Some(blob.into()),
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: None,
            error: Some(error.into()),
            blob: None,
        }
    }
}

// ─────────────────────────── 路径 ───────────────────────────

/// 信箱根目录。测试可以用 [`SPOOL_ENV`] 指到临时目录 —— 这是唯一让助手逻辑
/// 能在非 root 下跑通（含成功路径）的办法。
pub fn spool_root() -> PathBuf {
    match std::env::var(SPOOL_ENV) {
        Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => PathBuf::from(DEFAULT_SPOOL),
    }
}

impl Request {
    /// 请求落盘的目录：`<spool>/in`
    pub fn inbox_dir() -> PathBuf {
        spool_root().join("in")
    }

    /// 结果回写的目录：`<spool>/out`
    pub fn outbox_dir() -> PathBuf {
        spool_root().join("out")
    }

    /// 暂存目录：`<spool>/tmp`（先写这里再 rename 进 inbox，避免助手读到半个文件）
    pub fn staging_dir() -> PathBuf {
        spool_root().join("tmp")
    }

    /// 新建一个 NVRAM 请求（文件名带 uid/pid/随机串，多用户、多实例同时写也不会撞名）。
    pub fn new(ops: Vec<Op>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            kind: Kind::Nvram,
            ops,
            esp: None,
            dev: None,
            rel: None,
            hex: None,
        }
    }

    /// 枚举本机 ext4 分区。`esp` 只是查询条件（所选 ESP 的 PARTUUID），空串表示不识别。
    pub fn linux_probe(esp: impl Into<String>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            kind: Kind::LinuxProbe,
            ops: Vec::new(),
            esp: Some(esp.into()),
            dev: None,
            rel: None,
            hex: None,
        }
    }

    /// 把 1024 字节环境块覆盖回 `dev` 上的 `rel`。
    pub fn linux_write_env(dev: impl Into<String>, rel: impl Into<String>, block: &[u8]) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            kind: Kind::LinuxWriteEnv,
            ops: Vec::new(),
            esp: None,
            dev: Some(dev.into()),
            rel: Some(rel.into()),
            hex: Some(hex::encode(block)),
        }
    }

    /// 请求文件名。**必须**先用 [`valid_request_name`] 校验才能落盘 ——
    /// 助手那边只认这个形状，形状不对的文件会被无视（不删、不报错）。
    pub fn file_name(&self) -> String {
        let mut nonce = [0u8; 8];
        // 熵源不可用时退回时间戳：文件名只是防撞，不是安全边界
        if getrandom::fill(&mut nonce).is_err() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos() as u64)
                .unwrap_or(0);
            nonce.copy_from_slice(&stamp.to_le_bytes());
        }
        format!(
            "{}-{}-{}.json",
            current_uid(),
            std::process::id(),
            hex::encode(nonce)
        )
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| format!("请求序列化失败: {error}"))
    }
}

/// 请求文件名的合法形状：`<uid>-<pid>-<16 位十六进制>.json`。
///
/// 助手只看文件名就决定「这是不是给我的」，所以这里必须严格：
/// 反过来，任何用户都能往 inbox 扔文件，助手不能因为文件名奇怪就 panic 或越界写。
pub fn valid_request_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    let mut parts = stem.split('-');
    let (Some(uid), Some(pid), Some(nonce), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let numeric = |text: &str, max_len: usize| {
        !text.is_empty()
            && text.len() <= max_len
            && text.bytes().all(|byte| byte.is_ascii_digit())
    };
    numeric(uid, 7)
        && numeric(pid, 7)
        && nonce.len() == 16
        // 只认小写：`hex::encode` 产出小写，接受大写等于把「同一个名字两种拼法」
        // 也当成合法输入，没必要给自己多一种形态
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// 允许写入的变量名，且**必须是 OC 自己的拼法**（大小写完全一致）。
/// OC 只用这三种形状：`OCBtOrder`（默认启动顺序）、`OCBtNext`（一次性）、
/// `OCBt####`（单个启动项，4 位大写十六进制编号）。
///
/// 为什么一个字母的大小写都要卡死：UEFI 变量名是 UCS-2 字符串，**区分大小写**，
/// `OCBtOrder` 与 `OCBTORDER` 是两个互不相干的变量。这里曾经把名字统一转成大写
/// （本意只是「去重时好比较」），后果是助手把顺序写进了 `OCBTORDER`——
/// `nvram` 退出码 0、回执 `ok`，可 OC 读的 `OCBtOrder` 一个字节都没变，
/// 界面上只剩下「写进去了，但回读时不在首位」。实机上两个变量确实同时存在
/// （真名里还是旧值），才定位到这里。所以现在：拼法不对一律拒，**不做任何大小写变换**。
pub fn valid_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("OCBt") else {
        return false;
    };
    if rest == "Order" || rest == "Next" {
        return true;
    }
    rest.len() == 4
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
}

/// 设备名只认 macOS 的分区形状：`disk<盘号>s<分区号>`。
///
/// 三条拒绝理由都是实的：
/// - `/dev/disk0s9`：带前缀就意味着「路径由请求方决定」，那就是任意设备读写；
/// - `rdisk0s9`：`r` 必须由助手自己加（加了 `r` 才是裸设备，绕开缓存）；
/// - `disk0`：整盘不是分区，`disk0s0` 也不存在 —— 只接受 `s` + 正整数。
pub fn valid_disk_dev(dev: &str) -> bool {
    let Some(rest) = dev.strip_prefix("disk") else {
        return false;
    };
    let Some((disk, part)) = rest.split_once('s') else {
        return false;
    };
    // 前导零会让「同一个设备两种拼法」出现（`disk01s9` ≡ `disk1s9`），直接拒；
    // 但盘号 `0` 本身合法 —— 它是第一块盘，不是「前导零」。
    let digits = |text: &str, max_len: usize| {
        !text.is_empty()
            && text.len() <= max_len
            && text.bytes().all(|byte| byte.is_ascii_digit())
            && (text.len() == 1 || !text.starts_with('0'))
    };
    // 分区号从 1 开始：`disk0s0` 在 macOS 上不存在
    digits(disk, 3) && digits(part, 3) && part != "0"
}

/// 校验并解码一个请求。校验**先于**任何副作用，且是纯函数（单测覆盖）：
/// 坏请求只回一条错误，什么都不读、什么都不写。
pub fn validate(request: &Request) -> Result<Job, String> {
    if request.v != PROTOCOL_VERSION {
        return Err(format!(
            "协议版本不支持：{0}（本助手只认 {PROTOCOL_VERSION}）",
            request.v
        ));
    }
    match request.kind {
        Kind::Nvram => validate_nvram(request).map(Job::Nvram),
        Kind::LinuxProbe => validate_probe(request),
        Kind::LinuxWriteEnv => validate_write_env(request),
    }
}

fn validate_nvram(request: &Request) -> Result<Vec<(String, String, Vec<u8>)>, String> {
    // 交叉字段一律拒：`ops` 请求里出现 `dev`，说明调用方拿错了结构 ——
    // 静默忽略等于让「你以为写了 A，其实写了 B」有机会发生。
    if request.esp.is_some() || request.dev.is_some() || request.rel.is_some() {
        return Err("Nvram 请求不接受 esp / dev / rel 字段".to_string());
    }
    if request.hex.is_some() {
        return Err("Nvram 请求的值必须写在 ops 里".to_string());
    }
    if request.ops.is_empty() {
        return Err("请求里没有任何写入项".to_string());
    }
    if request.ops.len() > MAX_OPS {
        return Err(format!("一次最多写 {MAX_OPS} 条变量"));
    }
    let mut seen: Vec<String> = Vec::new();
    let mut decoded = Vec::with_capacity(request.ops.len());
    for op in &request.ops {
        if !op.guid.eq_ignore_ascii_case(OC_GUID) {
            return Err(format!(
                "只允许写 OpenCore 私有命名空间 {}，请求里是 {}",
                OC_GUID, op.guid
            ));
        }
        if !valid_name(&op.name) {
            return Err(format!(
                "变量名不在允许范围：{}（只允许 OCBtOrder / OCBtNext / OCBt####）",
                op.name
            ));
        }
        // 同一个名字出现两次：按谁先谁后生效全靠运气，直接拒。
        // 名字**原样**保留（不能为了去重改大小写——那会把写入引到另一个变量上）
        if seen.contains(&op.name) {
            return Err(format!("同一个变量被写了两次：{}", op.name));
        }
        seen.push(op.name.clone());
        if op.hex.len() % 2 != 0 || !op.hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("{} 的值不是合法十六进制串", op.name));
        }
        if op.hex.len() > MAX_VALUE * 2 {
            return Err(format!("{} 的值超过 {MAX_VALUE} 字节", op.name));
        }
        let bytes =
            hex::decode(&op.hex).map_err(|error| format!("{} 的值解码失败: {error}", op.name))?;
        decoded.push((op.name.clone(), OC_GUID.to_string(), bytes));
    }
    Ok(decoded)
}

fn validate_probe(request: &Request) -> Result<Job, String> {
    if !request.ops.is_empty() {
        return Err("LinuxProbe 不接受 ops 字段".to_string());
    }
    if request.dev.is_some() || request.rel.is_some() || request.hex.is_some() {
        return Err("LinuxProbe 不接受 dev / rel / hex 字段（读什么由助手自己枚举）".to_string());
    }
    let esp = request.esp.clone().unwrap_or_default();
    // `esp` 只是拿去和自己的分区表比对，永远不参与路径拼接；仍然限制形状，
    // 免得一个 10 KB 的字符串被当成「PARTUUID」在分区表里做几十次比较。
    if esp.len() > 64
        || !esp
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("esp 不是合法的分区标识".to_string());
    }
    Ok(Job::LinuxProbe { esp })
}

fn validate_write_env(request: &Request) -> Result<Job, String> {
    if !request.ops.is_empty() {
        return Err("LinuxWriteEnv 不接受 ops 字段".to_string());
    }
    if request.esp.is_some() {
        return Err("LinuxWriteEnv 不接受 esp 字段".to_string());
    }
    let dev = request
        .dev
        .as_deref()
        .ok_or_else(|| "缺少 dev 字段".to_string())?;
    if !valid_disk_dev(dev) {
        return Err(format!(
            "设备名必须形如 disk0s9（不接受 /dev/ 前缀或 rdisk 前缀）：{dev}"
        ));
    }
    let rel = request
        .rel
        .as_deref()
        .ok_or_else(|| "缺少 rel 字段".to_string())?;
    if !LINUX_ENV_RELS.contains(&rel) {
        return Err(format!(
            "只允许写 {}，请求里是 {rel}",
            LINUX_ENV_RELS.join(" / ")
        ));
    }
    let hex = request
        .hex
        .as_deref()
        .ok_or_else(|| "缺少 hex 字段".to_string())?;
    if hex.len() != GRUB_ENV_SIZE * 2 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "环境块必须正好 {GRUB_ENV_SIZE} 字节（hex {} 字符），这次是 {} 字符",
            GRUB_ENV_SIZE * 2,
            hex.len()
        ));
    }
    let block = hex::decode(hex).map_err(|error| format!("环境块解码失败: {error}"))?;
    Ok(Job::LinuxWriteEnv {
        dev: dev.to_string(),
        rel: rel.to_string(),
        block,
    })
}

/// 当前进程的 uid（文件名用；拿来当安全边界是不够的，见模块注释）。
pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        // 不引 libc：读 /proc 只在 Linux 有，macOS 上走 geteuid 才准。
        // `id -u` 是一次 exec，只在「发起请求」时调一次，代价可接受。
        if let Ok(output) = std::process::Command::new("/usr/bin/id").arg("-u").output() {
            if let Ok(text) = String::from_utf8(output.stdout) {
                if let Ok(uid) = text.trim().parse::<u32>() {
                    return uid;
                }
            }
        }
        0
    }
    #[cfg(not(unix))]
    {
        0
    }
}

// ─────────────────────────── 助手一侧 ───────────────────────────

/// 真正干活的那一半。协议层只管「收、校验、回执」，**不碰任何设备与变量** ——
/// 这样校验逻辑可以完整单测（注入假执行器），而真执行器只有一处（[`crate::helper_exec`]）。
pub trait Executor {
    /// 写 `OCBt*`。返回给人看的一句话。
    fn nvram(&self, ops: &[(String, String, Vec<u8>)]) -> Result<String, String>;
    /// 枚举 ext4 分区并读三家文件，返回要写进 blob 的整包 JSON。
    fn linux_probe(&self, esp: &str) -> Result<Vec<u8>, String>;
    /// 把环境块覆盖回 `dev` 的 `rel`。
    fn linux_write_env(&self, dev: &str, rel: &str, block: &[u8]) -> Result<String, String>;
}

/// 助手启动入口：把 inbox 里能处理的请求全部处理掉，然后退出。
///
/// 返回进程退出码（0 = 至少跑完了一轮；非 0 = 信箱都打不开，属于安装坏了）。
/// 必须由 **root** 运行 —— launchd 以 root 起它，不是因为「我们觉得安全」，
/// 而是因为写 NVRAM、读 `/dev/rdisk*` 只有 root 能做。
pub fn run_inbox(executor: &dyn Executor) -> i32 {
    let inbox = Request::inbox_dir();
    let outbox = Request::outbox_dir();
    if !inbox.is_dir() {
        eprintln!("inbox 不存在：{}（助手没装好）", inbox.display());
        return 1;
    }
    let _ = std::fs::create_dir_all(&outbox);

    let mut names: Vec<String> = match std::fs::read_dir(&inbox) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect(),
        Err(error) => {
            eprintln!("读不了 inbox：{}（{error}）", inbox.display());
            return 1;
        }
    };
    names.sort();
    for name in names {
        let path = inbox.join(&name);
        // 只处理普通文件：不跟软链接（否则可以把助手当成「读任意文件」的探针）
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => {}
            _ => continue,
        }
        if !valid_request_name(&name) {
            // 用户自己扔进来的杂物：不删、不报错，只跳过
            continue;
        }
        handle_one(&path, &name, &outbox, executor);
        let _ = std::fs::remove_file(&path);
    }
    trim_log();
    0
}

fn handle_one(path: &Path, name: &str, outbox: &Path, executor: &dyn Executor) {
    let stem = name.trim_end_matches(".json");
    let reply = match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_REQUEST_BYTES => Reply::failed(format!(
            "请求文件超过 {} KB",
            MAX_REQUEST_BYTES / 1024
        )),
        Ok(_) => match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Request>(&bytes) {
                // 校验先于副作用：坏请求在这里就结束，执行器一次都不会被调用
                Ok(request) => match validate(&request) {
                    Ok(job) => run_job(&job, stem, outbox, executor),
                    Err(error) => Reply::failed(error),
                },
                Err(error) => Reply::failed(format!("请求不是合法 JSON：{error}")),
            },
            Err(error) => Reply::failed(format!("读请求失败：{error}")),
        },
        Err(error) => Reply::failed(format!("请求文件读不到：{error}")),
    };
    append_log(&format!(
        "{stem} {} -> {}",
        kind_tag(&reply),
        if reply.ok { "ok" } else { "failed" }
    ));
    write_reply(outbox, stem, &reply);
}

fn kind_tag(reply: &Reply) -> &'static str {
    if reply.blob.is_some() {
        "blob"
    } else {
        ""
    }
}

fn run_job(job: &Job, stem: &str, outbox: &Path, executor: &dyn Executor) -> Reply {
    match job {
        Job::Nvram(ops) => match executor.nvram(ops) {
            Ok(detail) => Reply::ok(detail),
            Err(error) => Reply::failed(error),
        },
        Job::LinuxProbe { esp } => match executor.linux_probe(esp) {
            Ok(bytes) => {
                if bytes.len() as u64 > MAX_BLOB_BYTES {
                    return Reply::failed(format!(
                        "探测结果 {} 字节，超过 {} MB 上限",
                        bytes.len(),
                        MAX_BLOB_BYTES / 1024 / 1024
                    ));
                }
                let blob = format!("{stem}.blob");
                match write_blob(outbox, &blob, &bytes) {
                    Ok(()) => Reply::ok_with_blob(format!("读取完成（{} 字节）", bytes.len()), blob),
                    Err(error) => Reply::failed(error),
                }
            }
            Err(error) => Reply::failed(error),
        },
        Job::LinuxWriteEnv { dev, rel, block } => {
            match executor.linux_write_env(dev, rel, block) {
                Ok(detail) => Reply::ok(detail),
                Err(error) => Reply::failed(error),
            }
        }
    }
}

/// 写大载荷：同样先写 `.part` 再 rename。
///
/// **必须在回执之前调用**：App 侧看到回执就去读 blob，所以 blob 完整落盘是
/// 回执出现的前提。反过来的顺序会让 App 偶尔读到半个文件 —— 那种错只在
/// 快机器上偶发，最难查。
fn write_blob(outbox: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let target = outbox.join(name);
    let temp = outbox.join(format!("{name}.part"));
    std::fs::write(&temp, bytes).map_err(|error| format!("写结果载荷失败：{error}"))?;
    make_readable(&temp);
    std::fs::rename(&temp, &target).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("结果载荷就位失败：{error}")
    })
}

/// 写结果：先写 `<stem>.part` 再 rename。App 侧轮询时不会读到半个 JSON。
fn write_reply(outbox: &Path, stem: &str, reply: &Reply) {
    let target = outbox.join(format!("{stem}.json"));
    let temp = outbox.join(format!("{stem}.part"));
    let body = match serde_json::to_vec(reply) {
        Ok(body) => body,
        // 兜底串必须是 ASCII：`br"…"` 不接受非 ASCII 字符（这里是最后一道保险，
        // 真走到这一步说明 serde 序列化一个两个字段的结构体都失败了）
        Err(_) => br#"{"ok":false,"error":"reply serialization failed"}"#.to_vec(),
    };
    if std::fs::write(&temp, &body).is_ok() {
        make_readable(&temp);
        if std::fs::rename(&temp, &target).is_err() {
            let _ = std::fs::remove_file(&temp);
        }
    }
}

/// 结果文件要对发起请求的用户可读（root 写、用户读），所以得显式放开 0644：
/// 助手自己按 umask 落盘时可能只给 root 可读，那 App 就永远等不到结果。
fn make_readable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn append_log(line: &str) {
    let path = spool_root().join("helper.log");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| {
            use std::io::Write;
            writeln!(file, "{stamp} {line}")
        });
}

/// 日志超过 128 KB 就清空：这份日志只用来解释「为什么没生效」，
/// 不是审计记录，没必要无限增长。
fn trim_log() {
    let path = spool_root().join("helper.log");
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 128 * 1024 {
            let _ = std::fs::write(&path, b"");
        }
    }
}

/// 真正落笔的实现已经搬到 [`crate::helper_exec`]：这一层只管协议，不碰设备。
///
/// 分层的理由写在 `helper_exec` 头部；一句话是「校验要能在任何权限下单测，
/// 执行只能在 root 下跑」。
#[cfg(test)]
mod tests {
    use super::*;

    fn op(name: &str, hex: &str) -> Op {
        Op {
            guid: OC_GUID.to_string(),
            name: name.to_string(),
            hex: hex.to_string(),
        }
    }

    #[test]
    fn name_rules_match_opencore() {
        assert!(valid_name("OCBtOrder"));
        assert!(valid_name("OCBtNext"));
        assert!(valid_name("OCBt0080"));
        assert!(valid_name("OCBt00AB"));
        // 下面这些都不属于 OC 的命名空间，助手一律不碰
        assert!(!valid_name("BootOrder"));
        assert!(!valid_name("OCBt"));
        assert!(!valid_name("OCBt00800"));
        assert!(!valid_name("OCBtOrderX"));
        assert!(!valid_name("ocbt0080"));
        assert!(!valid_name(""));
        // 大小写不同 = 另一个变量（实机踩过：顺序写进了 OCBTORDER，OC 读不到）
        assert!(!valid_name("OCBTORDER"));
        assert!(!valid_name("OCBT0080"));
        assert!(!valid_name("OCBt00ab"), "小写编号也是另一个变量名");
    }

    #[test]
    fn request_name_shape_is_strict() {
        assert!(valid_request_name("501-1234-0123456789abcdef.json"));
        assert!(valid_request_name("0-0-0000000000000000.json"));
        // 目录穿越 / 绝对路径 / 多余字段全部拒（助手只按名字决定读不读）
        assert!(!valid_request_name("../501-1-0123456789abcdef.json"));
        assert!(!valid_request_name("501-1234-0123456789abcdef.json.bak"));
        assert!(!valid_request_name("501-1234-0123456789ABCDEF.json"), "只认小写");
        assert!(!valid_request_name("501-1234-0123456789abcde.json"));
        assert!(!valid_request_name("a-1234-0123456789abcdef.json"));
        assert!(!valid_request_name("501-1234-0123456789abcdef.txt"));
    }

    #[test]
    fn validate_accepts_the_real_shape() {
        let request = Request::new(vec![
            op("OCBt0082", "8200"),
            op("OCBtOrder", "82008100"),
        ]);
        match validate(&request).expect("合法请求") {
            Job::Nvram(decoded) => {
                assert_eq!(decoded[0].0, "OCBt0082");
                assert_eq!(decoded[0].2, vec![0x82, 0x00]);
                assert_eq!(decoded[1].0, "OCBtOrder");
            }
            other => panic!("应该是 Nvram 作业，实际是 {other:?}"),
        }
    }

    /// 回归（实机踩到）：`validate` 曾把变量名统一转大写，于是助手把顺序写进了
    /// `OCBTORDER` 这个「同名的另一个变量」，`nvram` 却回 ok —— 表现为
    /// 「写进去了，但回读时 OCBt0082 不在首位」。名字必须一个字母都不改地到达 nvram。
    #[test]
    fn names_reach_nvram_verbatim() {
        let request = Request::new(vec![op("OCBtOrder", "82008100")]);
        match validate(&request).expect("合法请求") {
            Job::Nvram(decoded) => assert_eq!(decoded[0].0, "OCBtOrder"),
            other => panic!("应该是 Nvram 作业，实际是 {other:?}"),
        }
        // 大写形态不是 OC 的变量：必须拒，而不是「帮你改成大写再写」
        assert!(validate(&Request::new(vec![op("OCBTORDER", "8200")])).is_err());
        assert!(validate(&Request::new(vec![op("OCBT0082", "8200")])).is_err());
    }

    #[test]
    fn validate_rejects_everything_outside_the_namespace() {
        // 换 GUID = 想写别的命名空间（例如被 macOS 拦的 Boot*）：必须拒
        let mut foreign = Request::new(vec![op("OCBtOrder", "8200")]);
        foreign.ops[0].guid = "8BE4DF61-93CA-11D2-AA0D-00E098032B8C".to_string();
        assert!(validate(&foreign).unwrap_err().contains("私有命名空间"));

        let boot = Request::new(vec![op("Boot0082", "8200")]);
        assert!(validate(&boot).unwrap_err().contains("允许范围"));

        let odd = Request::new(vec![op("OCBtOrder", "820")]);
        assert!(validate(&odd).unwrap_err().contains("十六进制"));

        let not_hex = Request::new(vec![op("OCBtOrder", "8z")]);
        assert!(validate(&not_hex).is_err());

        let huge = Request::new(vec![op("OCBtOrder", &"00".repeat(MAX_VALUE + 1))]);
        assert!(validate(&huge).unwrap_err().contains("字节"));

        let empty = Request::new(Vec::new());
        assert!(validate(&empty).unwrap_err().contains("没有任何写入项"));

        let mut version = Request::new(vec![op("OCBtOrder", "8200")]);
        version.v = 99;
        assert!(validate(&version).unwrap_err().contains("协议版本"));
    }

    #[test]
    fn validate_rejects_duplicate_names() {
        let duplicated = Request::new(vec![op("OCBtOrder", "8200"), op("OCBtOrder", "8100")]);
        assert!(validate(&duplicated).unwrap_err().contains("两次"));
    }

    #[test]
    fn request_round_trips_through_json() {
        let request = Request::new(vec![op("OCBtOrder", "820081008000")]);
        let json = request.to_json().expect("序列化");
        let back: Request = serde_json::from_str(&json).expect("反序列化");
        assert_eq!(back.ops.len(), 1);
        assert_eq!(back.ops[0].hex, "820081008000");
        // 多字段必须拒：请求格式是固定的，不给自己留猜测空间
        assert!(
            serde_json::from_str::<Request>(r#"{"v":2,"kind":"nvram","ops":[],"extra":1}"#).is_err()
        );
        // 老版本（v1）必须在版本检查那一关被拒，而不是被当成 nvram 猜测执行
        let old: Request = serde_json::from_str(r#"{"v":1,"kind":"nvram","ops":[]}"#).unwrap();
        assert!(validate(&old).unwrap_err().contains("协议版本"));
    }

    /// v2 的核心安全断言：助手**拿不到**路径参数、也拿不到任意设备节点。
    /// 这几条一旦有一条能过，就等于给当前用户开了一个「以 root 读任意块设备」的口子。
    #[test]
    fn linux_jobs_only_accept_the_fixed_shapes() {
        // 设备名：只认 diskNsM，不接受 /dev 前缀、rdisk 前缀、整盘、前导零
        assert!(valid_disk_dev("disk0s9"));
        assert!(valid_disk_dev("disk2s14"));
        for bad in [
            "/dev/disk0s9",
            "/dev/rdisk0s9",
            "rdisk0s9",
            "disk0",
            "disk0s0",
            "disk00s9",
            "disk0s09",
            "disk0s",
            "disks9",
            "",
            "disk0s9 ",
        ] {
            assert!(!valid_disk_dev(bad), "不该接受 {bad:?}");
        }

        // 相对路径：只有两个常量，其他一律拒（含目录穿越与绝对路径）
        let block = vec![b'#'; GRUB_ENV_SIZE];
        for bad_rel in [
            "/boot/grub/grubenv",
            "../../etc/passwd",
            "boot/grub/grub.cfg",
            "boot/grub2/grubenv ",
            "BOOT/GRUB/GRUBENV",
        ] {
            let request = Request::linux_write_env("disk0s9", bad_rel, &block);
            assert!(
                validate(&request).is_err(),
                "不该接受路径 {bad_rel:?}"
            );
        }

        // 尺寸：必须正好 1024 字节，1023/1025 都拒
        for size in [0usize, 1023, 1025] {
            let request = Request::linux_write_env("disk0s9", LINUX_ENV_RELS[0], &vec![b'#'; size]);
            assert!(validate(&request).is_err(), "{size} 字节不该被当成环境块");
        }

        // 交叉字段：writeEnv 带 esp、probe 带 dev，都拒（不静默忽略）
        let mut crossed = Request::linux_write_env("disk0s9", LINUX_ENV_RELS[0], &block);
        crossed.esp = Some("E537-7F31".to_string());
        assert!(validate(&crossed).is_err());
        let mut crossed = Request::linux_probe("");
        crossed.dev = Some("disk0s9".to_string());
        assert!(validate(&crossed).is_err());
        // nvram 请求里出现 dev，也拒
        let mut crossed = Request::new(vec![op("OCBtOrder", "8200")]);
        crossed.dev = Some("disk0s9".to_string());
        assert!(validate(&crossed).is_err());
    }

    #[test]
    fn linux_jobs_round_trip_and_decode() {
        let block: Vec<u8> = (0..GRUB_ENV_SIZE).map(|index| index as u8).collect();
        let request = Request::linux_write_env("disk2s14", LINUX_ENV_RELS[1], &block);
        let json = request.to_json().expect("序列化");
        let back: Request = serde_json::from_str(&json).expect("反序列化");
        match validate(&back).expect("合法请求") {
            Job::LinuxWriteEnv { dev, rel, block: decoded } => {
                assert_eq!(dev, "disk2s14");
                assert_eq!(rel, "boot/grub2/grubenv");
                assert_eq!(decoded, block, "环境块必须逐字节还原");
            }
            other => panic!("应该是 LinuxWriteEnv，实际是 {other:?}"),
        }

        let probe: Request = serde_json::from_str(r#"{"v":2,"kind":"linuxProbe","esp":""}"#).unwrap();
        assert_eq!(
            validate(&probe).expect("合法探测"),
            Job::LinuxProbe { esp: String::new() }
        );

        // 这个常量必须和 grub 里的尺寸一致，否则写进去的就是坏块
        assert_eq!(GRUB_ENV_SIZE, crate::grub::ENV_BLOCK_SIZE);
    }

    #[test]
    fn reply_shape_is_explicit() {
        assert_eq!(
            serde_json::to_string(&Reply::ok("已写 2 条")).unwrap(),
            r#"{"ok":true,"detail":"已写 2 条"}"#
        );
        assert_eq!(
            serde_json::to_string(&Reply::failed("拒绝")).unwrap(),
            r#"{"ok":false,"error":"拒绝"}"#
        );
        // 载荷字段只在有载荷时出现：没有 blob 的回执要和不带这个字段的时代逐字节一致，
        // 否则老 App 读新助手的回执会因为 deny_unknown_fields 直接报「看不懂」
        assert_eq!(
            serde_json::to_string(&Reply::ok_with_blob("读取完成", "1-2-ab.blob")).unwrap(),
            r#"{"ok":true,"detail":"读取完成","blob":"1-2-ab.blob"}"#
        );
    }

    /// 端到端：临时信箱 + 假执行器，把「收请求 → 校验 → 执行 → 回结果 → 删请求」整条走通。
    ///
    /// 用假执行器而不是 [`crate::helper_exec::System`]：这条测试要在任何权限、任何平台
    /// 都能跑，真执行器要 root 与真设备。两个方向各有单测，接缝就是 [`Executor`]。
    #[test]
    fn inbox_end_to_end_with_all_three_kinds() {
        let root = std::env::temp_dir().join(format!(
            "anyboot-helper-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let inbox = root.join("in");
        std::fs::create_dir_all(&inbox).unwrap();
        std::env::set_var(SPOOL_ENV, &root);

        let good = Request::new(vec![op("OCBtOrder", "82008100")]);
        let good_name = good.file_name();
        assert!(valid_request_name(&good_name));
        std::fs::write(inbox.join(&good_name), good.to_json().unwrap()).unwrap();

        // 坏请求：写 Boot*（不该被任何路径接受）
        let bad = Request::new(vec![op("Boot0082", "8200")]);
        let bad_name = bad.file_name();
        std::fs::write(inbox.join(&bad_name), bad.to_json().unwrap()).unwrap();

        // 探测：结果必须走 blob，且 blob 内容就是执行器返回的字节
        let probe = Request::linux_probe("E537-7F31");
        let probe_name = probe.file_name();
        std::fs::write(inbox.join(&probe_name), probe.to_json().unwrap()).unwrap();

        // 写环境块：尺寸不对必须在校验关就拒，执行器一次都不会被调用
        let block: Vec<u8> = (0..GRUB_ENV_SIZE).map(|index| index as u8).collect();
        let write = Request::linux_write_env("disk0s9", LINUX_ENV_RELS[0], &block);
        let write_name = write.file_name();
        std::fs::write(inbox.join(&write_name), write.to_json().unwrap()).unwrap();
        let short = Request::linux_write_env("disk0s9", LINUX_ENV_RELS[0], b"too short");
        let short_name = short.file_name();
        std::fs::write(inbox.join(&short_name), short.to_json().unwrap()).unwrap();

        // 杂物：不是给助手的，处理完必须原样留着
        std::fs::write(inbox.join("notes.txt"), "hello").unwrap();

        let executor = FakeExecutor::default();
        assert_eq!(run_inbox(&executor), 0);

        let read_reply = |name: &str| -> Reply {
            serde_json::from_slice(
                &std::fs::read(root.join("out").join(format!(
                    "{}.json",
                    name.trim_end_matches(".json")
                )))
                .unwrap_or_else(|_| panic!("{name} 必须有结果")),
            )
            .unwrap()
        };

        let good_reply = read_reply(&good_name);
        assert!(good_reply.ok);
        assert!(good_reply.detail.unwrap().contains("nvram"));

        let bad_reply = read_reply(&bad_name);
        assert!(!bad_reply.ok);
        assert!(bad_reply.error.unwrap().contains("允许范围"));

        let probe_reply = read_reply(&probe_name);
        assert!(probe_reply.ok);
        let blob = probe_reply.blob.expect("探测结果必须是 blob 回执");
        let blob_bytes = std::fs::read(root.join("out").join(&blob)).expect("blob 必须已落盘");
        assert!(
            String::from_utf8_lossy(&blob_bytes).contains("E537-7F31"),
            "blob 里要能看出探的是哪个 ESP"
        );

        let write_reply = read_reply(&write_name);
        assert!(write_reply.ok, "{:?}", write_reply.error);
        assert!(write_reply.detail.unwrap().contains("boot/grub/grubenv"));

        let short_reply = read_reply(&short_name);
        assert!(!short_reply.ok);
        assert!(short_reply.error.unwrap().contains("1024"));

        // 校验关之前就结束的两个请求（Boot* 名字不对、环境块尺寸不对）执行器一次都不该碰到：
        // 三次 = nvram + probe + writeEnv，多一次就说明校验漏了。
        assert_eq!(
            executor.calls.get(),
            3,
            "只有 nvram / probe / writeEnv 三个合法请求该被执行"
        );

        assert!(!inbox.join(&good_name).exists(), "处理完要删掉请求");
        assert!(inbox.join("notes.txt").exists(), "杂物不能动");

        std::env::remove_var(SPOOL_ENV);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 只记录「被调用了几次」的假执行器：用来证明坏请求根本到不了执行层。
    #[derive(Default)]
    struct FakeExecutor {
        calls: std::cell::Cell<usize>,
    }

    impl Executor for FakeExecutor {
        fn nvram(&self, ops: &[(String, String, Vec<u8>)]) -> Result<String, String> {
            self.calls.set(self.calls.get() + 1);
            Ok(format!("已写 {} 条 nvram 变量", ops.len()))
        }

        fn linux_probe(&self, esp: &str) -> Result<Vec<u8>, String> {
            self.calls.set(self.calls.get() + 1);
            Ok(format!(r#"{{"ok":true,"esp":"{esp}","parts":[]}}"#).into_bytes())
        }

        fn linux_write_env(&self, dev: &str, rel: &str, block: &[u8]) -> Result<String, String> {
            self.calls.set(self.calls.get() + 1);
            Ok(format!("已写入 {dev}:{rel}（{} 字节）", block.len()))
        }
    }
}
