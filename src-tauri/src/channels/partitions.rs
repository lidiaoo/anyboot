//! Windows 分区表：**一次** `Get-Partition` + 60 秒缓存。
//!
//! 起因（2026-10-28 分段埋点实测）：一轮扫描里 `Get-Partition` 被要了 **4 次**——
//! ESP 列表、APFS 容器坐标、卷的 GUID，以及 `oc.spec` 里每个 generic 条目又把
//! `volumes()` 整个重算一遍。每次都是一条 PowerShell 冷启动，1.0~1.1 s，合计
//! 3.7 s，占总耗时近一半。而这三处要的都只是同一张表的不同切片，没有任何理由
//! 问三遍。
//!
//! 缓存失效点严格限定在「盘符可能变了」的时刻：
//! * `win_esp::mount()` 分配完盘符（列表真的变了）；
//! * 用户主动点「重新识别」（见 `invalidate()`）。
//!
//! 其余情况 60 秒足够：插拔硬盘在 60 秒内最多是「新插的盘还看不到」，不会让
//! 已缓存的数据变错。
//!
//! 非 Windows 上是空实现 —— 那两端的等价物是 `lsblk`，仍各走各的。

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use super::*;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 缓存有效期。太短等于没缓存（每次刷新都重跑），太长会让刚插的盘看不到。
pub(crate) const TTL: Duration = Duration::from_secs(60);

/// ESP（EFI 系统分区）的 GPT 类型。
pub(crate) const ESP_GPT: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
/// APFS 容器的 GPT 类型（跨平台同一把尺子，见 `apfs_reader::CONTAINER_GPT_TYPE`）。
pub(crate) const APFS_GPT: &str = "7c3457ef-0000-11aa-aa11-00306543ecac";
/// Linux 文件系统数据分区的 GPT 类型（根分区 / `/boot` 都是它）。
/// 只认这一类：LVM（`e6d6d379…`）与 RAID（`a19d880f…`）里的 ext4 我们不解析。
pub(crate) const LINUX_GPT: &str = "0fc63daf-8483-4772-8e79-3d69d8477de4";

/// 一条分区。只留三处调用方真正要的字段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Partition {
    pub(crate) disk: u32,
    pub(crate) partition: u32,
    /// 有盘符才能被普通 Win32 文件 API 访问；`None` = 系统没给它分配
    pub(crate) letter: Option<char>,
    /// 分区字节数（给 APFS 容器算读取范围用）
    pub(crate) size: u64,
    /// 分区在大盘上的字节偏移（APFS 没有卷路径，只能开整盘按偏移读）
    pub(crate) offset: u64,
    /// GPT 类型 GUID，**小写、无花括号**（`-like '*c12a7328*'` 那种比较在这里做）
    pub(crate) gpt_type: String,
    /// 分区 GUID，小写无花括号；MBR 盘上是空串
    pub(crate) guid: String,
}

impl Partition {
    /// 按 GUID 前 8 位判类型：不同 PowerShell 版本对 GptType 是否带花括号不一致，
    /// 只比 GUID 本体最稳（沿用原来 `-like '*c12a7328*'` 的思路）。
    pub(crate) fn is_esp(&self) -> bool {
        // 只比 GUID 前 8 位：命令宿主的不同版本对 GptType 的括号/大小写不一致
        self.gpt_type.starts_with(&ESP_GPT[..8])
    }

    pub(crate) fn is_apfs(&self) -> bool {
        self.gpt_type.starts_with(&APFS_GPT[..8])
    }

    /// Linux 文件系统分区。Windows 读 ext4 靠它筛候选：这些分区没有盘符，
    /// 唯一入口是「开整盘 + `offset`」。
    pub(crate) fn is_linux(&self) -> bool {
        self.gpt_type.starts_with(&LINUX_GPT[..8])
    }

    /// 可访问的挂载根（`S:\`）；没盘符就是 `None`。
    pub(crate) fn root(&self) -> Option<PathBuf> {
        self.letter
            .map(|letter| PathBuf::from(format!("{letter}:\\")))
    }

    /// 分区大小（MB，向下取整）。0 = 取不到。
    pub(crate) fn size_mb(&self) -> u64 {
        self.size / (1024 * 1024)
    }
}

// ─────────────────────────── 解析（纯函数，可单测） ───────────────────────────

/// GUID 文本归一：去花括号、转小写。`Get-Partition` 的 Guid 常带花括号。
fn normalize_guid(text: &str) -> String {
    text.trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .to_ascii_lowercase()
}

fn as_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    match value? {
        serde_json::Value::Number(number) => number.as_u64(),
        // 个别环境把大整数序列化成字符串（Size 超过 2^53 时也不该用 f64）
        serde_json::Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
}

/// `Get-Partition | ... | ConvertTo-Json -Compress` 的输出 → 分区表。
///
/// 三处形状差异要吸收（都是真机见过的）：
/// * **只有一个结果时** `ConvertTo-Json` 出对象而不是数组；
/// * `DriveLetter` 是 `[char]`，通常序列化成 `"S"`，个别版本写成数字码点 `83`；
/// * `Size` / `Offset` 可能被写成字符串。
///
/// 缺 `DiskNumber` / `PartitionNumber` 的行直接跳过 —— 没有坐标的行没法用来读盘。
pub(crate) fn parse_json(text: &str) -> Vec<Partition> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return Vec::new();
    };
    let rows = match parsed {
        serde_json::Value::Array(rows) => rows,
        row @ serde_json::Value::Object(_) => vec![row],
        _ => return Vec::new(),
    };
    rows.into_iter()
        .filter_map(|row| {
            let disk = as_u64(row.get("DiskNumber"))? as u32;
            let partition = as_u64(row.get("PartitionNumber"))? as u32;
            let letter = match row.get("DriveLetter") {
                Some(serde_json::Value::String(text)) => text
                    .trim()
                    .chars()
                    .next()
                    .filter(char::is_ascii_alphabetic)
                    .map(|letter| letter.to_ascii_uppercase()),
                Some(serde_json::Value::Number(number)) => number
                    .as_u64()
                    .and_then(|code| u8::try_from(code).ok())
                    .map(char::from)
                    .filter(char::is_ascii_alphabetic)
                    .map(|letter| letter.to_ascii_uppercase()),
                _ => None,
            };
            let gpt_type = row
                .get("GptType")
                .and_then(|value| value.as_str())
                .map(normalize_guid)
                .unwrap_or_default();
            let guid = row
                .get("Guid")
                .and_then(|value| value.as_str())
                .map(normalize_guid)
                .unwrap_or_default();
            Some(Partition {
                disk,
                partition,
                letter,
                size: as_u64(row.get("Size")).unwrap_or(0),
                offset: as_u64(row.get("Offset")).unwrap_or(0),
                gpt_type,
                guid,
            })
        })
        .collect()
}

// ─────────────────────────── 缓存 ───────────────────────────

static CACHE: Mutex<Option<(Instant, Vec<Partition>)>> = Mutex::new(None);

/// 清掉缓存。`win_esp::mount()` 成功之后必须调 —— 刚分配的盘符不在旧表里。
pub(crate) fn invalidate() {
    if let Ok(mut cache) = CACHE.lock() {
        *cache = None;
    }
}

#[cfg(target_os = "windows")]
fn query() -> Vec<Partition> {
    // 真实命令（命中缓存时这条不会出现）。埋点名与 all() 分开，日志里一眼能看出
    // 「这轮问了几次表 / 其中几次真起了进程」
    let _span = perf::span("cmd.get_partition");
    // 一条脚本把三处要的字段全带上：GptType/Guid 给 ESP 和卷 GUID 用，
    // Offset/Size 给 APFS 容器用（它没有卷路径，只能开整盘按偏移读）。
    const SCRIPT: &str = "Get-Partition \
                          | Select-Object DiskNumber,PartitionNumber,DriveLetter,Size,Offset,GptType,Guid \
                          | ConvertTo-Json -Compress";
    let output = command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            parse_json(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

#[cfg(not(target_os = "windows"))]
fn query() -> Vec<Partition> {
    Vec::new()
}

/// 全部分区（命中缓存就直接返回）。
///
/// **查询是持锁做的**（single-flight）：启动时 `thread::spawn(refresh_and_update)`
/// 与前端触发的刷新几乎同时跑，如果只在锁内读缓存、锁外查询，两个线程都会看到
/// 「缓存空」而各起一次进程 —— 实测两轮各 1000ms，合并失败。持锁查询让后来的
/// 那个排在门外等着，等第一个写回缓存后直接命中。
///
/// 查询失败返回空表且**不写缓存**：偶发失败不该让整个 60 秒都看到一张空表
/// （用户会以为盘没了）。
pub(crate) fn all() -> Vec<Partition> {
    let _span = perf::span("partitions.all");
    let mut guard = match CACHE.lock() {
        Ok(guard) => guard,
        // 锁被别人 panic 污染：宁可重查一次也不能返回空表
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some((taken_at, rows)) = guard.as_ref() {
        if taken_at.elapsed() < TTL && !rows.is_empty() {
            return rows.clone();
        }
    }
    let rows = query();
    if !rows.is_empty() {
        *guard = Some((Instant::now(), rows.clone()));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linux 分区按类型认出来 —— Windows 读 ext4 的唯一入口就是它。
    #[test]
    fn recognises_linux_partitions() {
        let rows = parse_json(
            r#"[{"DiskNumber":1,"PartitionNumber":3,"DriveLetter":null,"Size":107374182400,
                 "Offset":1048576000,"GptType":"{0fc63daf-8483-4772-8e79-3d69d8477de4}"},
                {"DiskNumber":1,"PartitionNumber":4,"DriveLetter":null,"Size":1073741824,
                 "Offset":2118123520,"GptType":"{e6d6d379-f507-44c2-a23c-238f2a3a45e5}"}]"#,
        );
        assert!(rows[0].is_linux(), "根分区必须认出来");
        assert!(!rows[0].is_esp() && !rows[0].is_apfs(), "三种类型不能串");
        assert!(!rows[1].is_linux(), "LVM 成员不是我们能解析的 ext4，别当候选");
    }

    /// 真机形状：一块盘一块 ESP（无盘符）、一块有盘符，外加一块 APFS 容器。
    #[test]
    fn parses_real_converttojson_shape() {
        let text = r#"[
            {"DiskNumber":0,"PartitionNumber":1,"DriveLetter":null,"Size":272629760,"Offset":1048576,
             "GptType":"{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}","Guid":"{11111111-2222-3333-4444-555555555555}"},
            {"DiskNumber":1,"PartitionNumber":2,"DriveLetter":"S","Size":104857600,"Offset":20971520,
             "GptType":"{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}","Guid":"{66666666-7777-8888-9999-aaaaaaaaaaaa}"},
            {"DiskNumber":2,"PartitionNumber":3,"DriveLetter":null,"Size":107374182400,"Offset":314572800,
             "GptType":"{7c3457ef-0000-11aa-aa11-00306543ecac}","Guid":"{bbbbbbbb-cccc-dddd-eeee-ffffffffffff}"}
        ]"#;
        let rows = parse_json(text);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].letter, None, "没有盘符要留成 None，不能当成 0 或空字符");
        assert_eq!(rows[0].size_mb(), 260, "272629760 字节 = 260 MB");
        assert!(rows[0].is_esp());
        assert_eq!(rows[1].root().unwrap().display().to_string(), "S:\\");
        assert!(rows[2].is_apfs(), "APFS 容器按类型认");
        assert_eq!(rows[2].offset, 314572800, "APFS 靠 Offset 定位，不能丢");
        assert_eq!(rows[0].guid, "11111111-2222-3333-4444-555555555555", "花括号要去掉");
    }

    /// 单个结果时 ConvertTo-Json 出对象而不是数组 —— 不能因此整条丢掉。
    #[test]
    fn parses_single_object() {
        let rows =
            parse_json(r#"{"DiskNumber":2,"PartitionNumber":3,"DriveLetter":null,"Size":0}"#);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].size_mb(), 0, "大小 0 时调用方自己决定怎么显示");
        assert!(rows[0].gpt_type.is_empty(), "缺字段留空串，不 panic");
    }

    /// 盘符可能被序列化成码点（个别 PowerShell 版本）；数字也可能带引号。
    #[test]
    fn tolerates_alternate_scalar_shapes() {
        let rows = parse_json(
            r#"[{"DiskNumber":0,"PartitionNumber":1,"DriveLetter":83,"Size":"104857600","Offset":"4096"}]"#,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].letter, Some('S'));
        assert_eq!(rows[0].size_mb(), 100);
        assert_eq!(rows[0].offset, 4096, "字符串形式的 Offset 也要认");
    }

    /// 真机验证（默认 ignore，只在本机手动跑）：
    /// `cargo test --release --lib real_partition_table -- --ignored --nocapture`
    ///
    /// 单测只能钉住解析形状，钉不住「真机那条脚本的输出能不能被解析」——
    /// 这两件事差一个字段名就会全空，而空表在界面上表现为「三张卡全未检测到」，
    /// 没有任何报错。这条把整条链路（起命令 → 解析 → 筛选）跑一遍并打出来。
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore]
    fn real_partition_table() {
        let started = std::time::Instant::now();
        let rows = all();
        let first = started.elapsed().as_millis();
        let started = std::time::Instant::now();
        let again = all();
        let second = started.elapsed().as_millis();
        println!("首次 {} ms / {} 行；命中缓存 {} ms / {} 行", first, rows.len(), second, again.len());
        for row in &rows {
            let tag = if row.is_esp() {
                "ESP"
            } else if row.is_apfs() {
                "APFS"
            } else {
                "  "
            };
            println!(
                "{tag} disk={} part={} letter={:?} size={}MB offset={} guid={}",
                row.disk,
                row.partition,
                row.letter,
                row.size_mb(),
                row.offset,
                row.guid
            );
        }
        assert!(!rows.is_empty(), "真机分区表不该是空的");
        // **不能断言「必有 ESP 类型的行」**：本机（开发机）的 ESP GPT 类型被写成了
        // `ebd0a0a2…`（微软基本数据分区），`win_esp::list()` 一直是空表，靠
        // `candidate_roots` 的 26 盘符兜底在工作。这是真机事实，不是 bug ——
        // 断言写死会让这条诊断测试变成噪音。
        assert!(
            rows.iter().any(Partition::is_apfs),
            "本机有 APFS 容器（7c3457ef），按类型必须认出来"
        );
        assert!(second + 5 < first.max(50), "第二次应命中缓存（{second}ms vs {first}ms）");
    }

    /// 非数组、空串、坏 JSON 都只返回空表（这条路径失败不该让界面报错）。
    #[test]
    fn bad_input_yields_empty() {
        assert!(parse_json("").is_empty());
        assert!(parse_json("   ").is_empty());
        assert!(parse_json("not json").is_empty());
        assert!(parse_json("null").is_empty());
        // 没有坐标的行没法用来读盘，跳过
        assert!(parse_json(r#"{"DiskNumber":"x","PartitionNumber":1}"#).is_empty());
        assert!(parse_json(r#"[{"PartitionNumber":1}]"#).is_empty());
    }
}
