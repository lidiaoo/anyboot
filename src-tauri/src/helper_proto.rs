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
//! 安全边界写在这里，别在别处放宽：
//! 1. 只接受 OpenCore 私有命名空间（[`OC_GUID`]）下的 `OCBtOrder` / `OCBtNext` /
//!    `OCBt####` —— 这是本功能唯一需要写的东西。**不执行 shell，不接受任意路径**。
//! 2. 值只允许十六进制串，长度上限 [`MAX_VALUE`] 字节，单次最多 [`MAX_OPS`] 条。
//! 3. 助手只处理 inbox 里**普通文件**（不跟软链接），文件名必须匹配 [`valid_request_name`]。
//!
//! 仍然存在的代价（必须如实写在 UI 与文档里）：装好之后，**任何以当前用户身份运行的
//! 进程**都能通过它改 OpenCore 的默认启动项。最坏后果是「默认启动项被改」，拿不到
//! root、也执行不了任意命令。不装则每一次写都要密码 —— 二选一。

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

pub const PROTOCOL_VERSION: u32 = 1;

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
    pub ops: Vec<Op>,
}

/// 助手写回的结果。`ok=false` 时 `error` 一定非空（前端要能原样显示）。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    pub fn ok(detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            detail: Some(detail.into()),
            error: None,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: None,
            error: Some(error.into()),
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

    /// 新建一个请求（文件名带 uid/pid/随机串，多用户、多实例同时写也不会撞名）。
    pub fn new(ops: Vec<Op>) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            ops,
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

/// 允许写入的变量名。OC 自己只用这三种形状：
/// `OCBtOrder`（默认启动顺序）、`OCBtNext`（一次性）、`OCBt####`（单个启动项，4 位大写十六进制编号）。
pub fn valid_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("OCBt") else {
        return false;
    };
    if rest == "Order" || rest == "Next" {
        return true;
    }
    rest.len() == 4 && rest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// 校验并解码一个请求：返回可直接交给 `nvram` 的三元组。
///
/// 校验**先于**任何副作用，且是纯函数（单测覆盖）：坏请求只回一条错误，什么都不写。
pub fn validate(request: &Request) -> Result<Vec<(String, String, Vec<u8>)>, String> {
    if request.v != PROTOCOL_VERSION {
        return Err(format!(
            "协议版本不支持：{0}（本助手只认 {PROTOCOL_VERSION}）",
            request.v
        ));
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
        // 同一个名字出现两次：按谁先谁后生效全靠运气，直接拒
        let name = op.name.to_ascii_uppercase();
        if seen.contains(&name) {
            return Err(format!("同一个变量被写了两次：{name}"));
        }
        seen.push(name.clone());
        if op.hex.len() % 2 != 0 || !op.hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("{} 的值不是合法十六进制串", op.name));
        }
        if op.hex.len() > MAX_VALUE * 2 {
            return Err(format!("{} 的值超过 {MAX_VALUE} 字节", op.name));
        }
        let bytes = hex::decode(&op.hex)
            .map_err(|error| format!("{} 的值解码失败: {error}", op.name))?;
        decoded.push((name, OC_GUID.to_string(), bytes));
    }
    Ok(decoded)
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

/// 助手启动入口：把 inbox 里能处理的请求全部处理掉，然后退出。
///
/// 返回进程退出码（0 = 至少跑完了一轮；非 0 = 信箱都打不开，属于安装坏了）。
/// 必须由 **root** 运行 —— launchd 以 root 起它，不是因为「我们觉得安全」，
/// 而是因为写 NVRAM 只有 root 能做。
pub fn run_inbox() -> i32 {
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
        handle_one(&path, &name, &outbox);
        let _ = std::fs::remove_file(&path);
    }
    trim_log();
    0
}

fn handle_one(path: &Path, name: &str, outbox: &Path) {
    let stem = name.trim_end_matches(".json");
    let reply = match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_REQUEST_BYTES => Reply::failed(format!(
            "请求文件超过 {} KB",
            MAX_REQUEST_BYTES / 1024
        )),
        Ok(_) => match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Request>(&bytes) {
                Ok(request) => match validate(&request).and_then(|ops| apply(&ops)) {
                    Ok(detail) => Reply::ok(detail),
                    Err(error) => Reply::failed(error),
                },
                Err(error) => Reply::failed(format!("请求不是合法 JSON：{error}")),
            },
            Err(error) => Reply::failed(format!("读请求失败：{error}")),
        },
        Err(error) => Reply::failed(format!("请求文件读不到：{error}")),
    };
    append_log(&format!("{stem} -> {}", if reply.ok { "ok" } else { "failed" }));
    write_reply(outbox, stem, &reply);
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

/// 真正落笔的地方：逐条调 `nvram`。
///
/// 一次请求里可能有多条（`OCBtOrder` + 一条 `OCBt####`），顺序由调用方决定：
/// 先把单项写好、最后写 Order，中途失败时启动顺序表还是旧值，不会出现
/// 「Order 指向一条还没写成功的项」。
#[cfg(target_os = "macos")]
fn apply(ops: &[(String, String, Vec<u8>)]) -> Result<String, String> {
    // 测试用：把信箱指到临时目录时允许 dry-run，这样成功路径也能在非 root 下验证。
    // 生产安装的 launchd 作业不设 ANYBOOT_HELPER_SPOOL，这条分支进不来。
    let dry_run = std::env::var(SPOOL_ENV).is_ok()
        && std::env::var("ANYBOOT_HELPER_DRY_RUN").as_deref() == Ok("1");

    for (name, guid, value) in ops {
        if dry_run {
            continue;
        }
        let arg = nvram_arg(name, guid, value);
        let output = std::process::Command::new("/usr/sbin/nvram")
            .arg(&arg)
            .output()
            .map_err(|error| format!("无法执行 nvram: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                format!("写 {name} 失败（nvram 退出码非 0）")
            } else {
                format!("写 {name} 失败：{stderr}")
            });
        }
    }
    Ok(if dry_run {
        format!("已处理 {} 条变量（dry-run，未真写）", ops.len())
    } else {
        format!("已写 {} 条变量：{}", ops.len(), join_names(ops))
    })
}

#[cfg(not(target_os = "macos"))]
fn apply(_ops: &[(String, String, Vec<u8>)]) -> Result<String, String> {
    Err("这个助手只在 macOS 上使用".to_string())
}

fn join_names(ops: &[(String, String, Vec<u8>)]) -> String {
    ops.iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>()
        .join("、")
}

/// `nvram` 写入参数的唯一正确形态：`guid:name=%XX%XX…` **一个**参数。
///
/// 拆成两个参数时 `nvram` 会把它当「读取 + 设置空名变量」：退出码 0、什么都没写。
/// （这条坑在 `channels::nvram` 里已经踩过一次，这里必须用同一套拼法。）
fn nvram_arg(name: &str, guid: &str, value: &[u8]) -> String {
    let mut encoded = String::with_capacity(value.len() * 3);
    for byte in value {
        encoded.push_str(&format!("%{byte:02X}"));
    }
    format!("{guid}:{name}={encoded}")
}

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
        assert!(valid_name("OCBt00ab"), "小写十六进制也认（回写时统一大写）");
        // 下面这些都不属于 OC 的命名空间，助手一律不碰
        assert!(!valid_name("BootOrder"));
        assert!(!valid_name("OCBt"));
        assert!(!valid_name("OCBt00800"));
        assert!(!valid_name("OCBtOrderX"));
        assert!(!valid_name("ocbt0080"));
        assert!(!valid_name(""));
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
        let decoded = validate(&request).expect("合法请求");
        assert_eq!(decoded[0].0, "OCBT0082");
        assert_eq!(decoded[0].2, vec![0x82, 0x00]);
        assert_eq!(decoded[1].0, "OCBTORDER");
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

        let version = Request {
            v: 99,
            ops: vec![op("OCBtOrder", "8200")],
        };
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
        assert!(serde_json::from_str::<Request>(r#"{"v":1,"ops":[],"extra":1}"#).is_err());
    }

    #[test]
    fn nvram_arg_is_a_single_argument() {
        // 这条断言是防回归的：拆成两个参数会「退出码 0 但什么都没写」
        assert_eq!(
            nvram_arg("OCBtOrder", OC_GUID, &[0x82, 0x00, 0x81, 0x00]),
            "4D1FDA02-38C7-4A6A-9CC6-4BCCA8B30102:OCBtOrder=%82%00%81%00"
        );
        assert!(!nvram_arg("OCBtOrder", OC_GUID, &[0x82]).contains(' '));
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
    }

    /// 端到端：临时信箱 + dry-run，把「收请求 → 校验 → 回结果 → 删请求」整条走通。
    /// 生产路径唯一不同的是 `apply` 真的去调 `nvram`。
    #[test]
    fn inbox_end_to_end_in_dry_run() {
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
        std::env::set_var("ANYBOOT_HELPER_DRY_RUN", "1");

        let good = Request::new(vec![op("OCBtOrder", "82008100")]);
        let good_name = good.file_name();
        assert!(valid_request_name(&good_name));
        std::fs::write(inbox.join(&good_name), good.to_json().unwrap()).unwrap();

        // 坏请求：写 Boot*（不该被任何路径接受）
        let bad = Request::new(vec![op("Boot0082", "8200")]);
        let bad_name = bad.file_name();
        std::fs::write(inbox.join(&bad_name), bad.to_json().unwrap()).unwrap();

        // 杂物：不是给助手的，处理完必须原样留着
        std::fs::write(inbox.join("notes.txt"), "hello").unwrap();

        assert_eq!(run_inbox(), 0);

        let good_reply: Reply = serde_json::from_slice(
            &std::fs::read(root.join("out").join(format!(
                "{}.json",
                good_name.trim_end_matches(".json")
            )))
            .expect("好请求必须有结果"),
        )
        .unwrap();
        assert!(good_reply.ok);
        assert!(good_reply.detail.unwrap().contains("dry-run"));

        let bad_reply: Reply = serde_json::from_slice(
            &std::fs::read(root.join("out").join(format!(
                "{}.json",
                bad_name.trim_end_matches(".json")
            )))
            .expect("坏请求也要有结果"),
        )
        .unwrap();
        assert!(!bad_reply.ok);
        assert!(bad_reply.error.unwrap().contains("允许范围"));

        assert!(!inbox.join(&good_name).exists(), "处理完要删掉请求");
        assert!(inbox.join("notes.txt").exists(), "杂物不能动");

        std::env::remove_var("ANYBOOT_HELPER_DRY_RUN");
        std::env::remove_var(SPOOL_ENV);
        let _ = std::fs::remove_dir_all(&root);
    }
}
