//! Windows：EFI 系统分区的只读枚举（+ 一次提权挂载）。
//!
//! 为什么需要它：Windows 默认**不给 ESP 分配盘符**（它是隐藏的 EFI 系统分区），
//! 而 `channels::candidate_roots` 原来只扫 `A:`–`Z:` —— ESP 在 Win32 文件 API 眼里
//! 根本不存在，于是三张引导通道卡片一起变成「未检测到」，且**一句原因都没有**。
//! `Get-Partition` 能按 GPT 类型把它列出来，不需要盘符。
//!
//! 解析做成纯函数 [`parse_json`]，不依赖 Windows —— 开发机上没有 Windows 可用，
//! 只能靠它把 PowerShell 的输出形状钉住（与 `parse_diskutil_info` / `lsblk` 同一套做法）。
//! 真正跑命令的 [`list`] / [`mount`] 只在 `target_os = "windows"` 下编译。
//!
//! 非 Windows 上这套东西没有调用者（`esp_hint` / `candidate_roots` 都按平台分叉），
//! 所以整模块放行 dead_code —— 否则每次在 macOS 上编译都会刷一片「从未使用」。
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use super::*;

/// 一块 EFI 系统分区（GPT 类型 `c12a7328-f81f-11d2-ba4b-00a0c93ec93b`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WinEsp {
    pub(crate) disk: u32,
    pub(crate) partition: u32,
    /// 有盘符才能被普通 Win32 文件 API 访问；`None` = 系统没给它分配
    pub(crate) letter: Option<char>,
    /// 分区大小（MB）；取不到时 0
    pub(crate) size_mb: u64,
}

impl WinEsp {
    /// 坐标，`磁盘 0 分区 1`。窄屏（420px）下条目标题只能放十几个字，
    /// 大小放进副标题 —— 塞在标题里会被省略号吃掉，而它正是选哪块 ESP 的依据。
    pub(crate) fn coords(&self) -> String {
        format!("磁盘 {} 分区 {}", self.disk, self.partition)
    }

    /// 人话描述，用在长提示里：`磁盘 0 分区 1（260 MB）`
    pub(crate) fn describe(&self) -> String {
        if self.size_mb == 0 {
            self.coords()
        } else {
            format!("{}（{} MB）", self.coords(), self.size_mb)
        }
    }

    /// 可访问的挂载根（`S:\`）；没盘符就是 `None`。
    pub(crate) fn root(&self) -> Option<PathBuf> {
        self.letter.map(|letter| PathBuf::from(format!("{letter}:\\")))
    }
}

/// `Get-Partition | ... | ConvertTo-Json -Compress` 的输出 → 分区表（纯函数）。
///
/// 两处形状差异要吸收：
/// * **只有一个结果时** `ConvertTo-Json` 出对象而不是数组；
/// * `DriveLetter` 是 `[char]`，`ConvertTo-Json` 序列化成 `"S"`，但个别版本会写成
///   数字码点（`83`）—— 两种都认。
pub(crate) fn parse_json(text: &str) -> Vec<WinEsp> {
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
            let disk = row.get("DiskNumber")?.as_u64()? as u32;
            let partition = row.get("PartitionNumber")?.as_u64()? as u32;
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
            let size_mb = match row.get("Size") {
                Some(serde_json::Value::Number(number)) => number.as_u64(),
                Some(serde_json::Value::String(text)) => text.trim().parse::<u64>().ok(),
                _ => None,
            }
            .map(|bytes| bytes / (1024 * 1024))
            .unwrap_or(0);
            Some(WinEsp {
                disk,
                partition,
                letter,
                size_mb,
            })
        })
        .collect()
}

/// 本机所有 EFI 系统分区（按 GPT 类型认，不需要盘符）。
///
/// 失败一律返回空表：这条只是「让提示说得准」，不该因为 PowerShell 抽风把
/// 整条链路弄挂（真读不到时界面还是原来的「未检测到」）。
#[cfg(target_os = "windows")]
pub(crate) fn list() -> Vec<WinEsp> {
    // `-like '*c12a7328*'` 而不是 `-eq '{…}'`：不同 PowerShell 版本对 GptType
    // 是否带花括号不一致，只比 GUID 本体最稳。
    const SCRIPT: &str = "Get-Partition | Where-Object { $_.GptType -like '*c12a7328*' } \
                          | Select-Object DiskNumber,PartitionNumber,DriveLetter,Size \
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
pub(crate) fn list() -> Vec<WinEsp> {
    Vec::new()
}

/// 给一块没有盘符的 ESP 分配盘符（`Add-PartitionAccessPath -AssignDriveLetter`）。
///
/// 这一步必须要管理员权限，所以用 `Start-Process -Verb RunAs` 弹一次 UAC。
/// **新进程的退出码拿不到**，因此不靠返回码判断成败 —— 挂完重新枚举一遍，
/// 真拿到盘符才算成（与 macOS 用 osascript 提权后回读校验是同一思路）。
/// 用户点了「否」时枚举不到盘符，如实报出来。
#[cfg(target_os = "windows")]
pub(crate) fn mount(disk: u32, partition: u32) -> Result<String, String> {
    // 只用两个数字参数拼脚本，不存在引号/注入面
    let inner = format!(
        "Add-PartitionAccessPath -DiskNumber {disk} -PartitionNumber {partition} -AssignDriveLetter"
    );
    let outer = format!(
        "Start-Process powershell -Verb RunAs -Wait -ArgumentList '-NoProfile','-NonInteractive','-Command','{inner}'"
    );
    let launched = command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &outer])
        .stderr(Stdio::null())
        .output();
    if let Err(error) = launched {
        return Err(format!("无法调用 PowerShell 提权: {error}"));
    }
    // 回读：分配成功的话，这块分区现在应该有盘符了
    for esp in list() {
        if esp.disk == disk && esp.partition == partition {
            if let Some(root) = esp.root() {
                return Ok(root.display().to_string());
            }
            return Err(
                "授权被取消，或系统没能给这块 EFI 分区分配盘符（盘符可能已经用完了）".to_string(),
            );
        }
    }
    Err("挂载后没再找到这块 EFI 分区：点「重新识别」再看一次".to_string())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn mount(_disk: u32, _partition: u32) -> Result<String, String> {
    Err("只有 Windows 需要给 EFI 分区分配盘符".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机形状（`Get-Partition | ConvertTo-Json -Compress`，两块盘各一块 ESP，
    /// 一块有盘符一块没有）。DriveLetter 为 null 是常见情况。
    #[test]
    fn parses_real_converttojson_shape() {
        let text = r#"[
            {"DiskNumber":0,"PartitionNumber":1,"DriveLetter":null,"Size":272629760},
            {"DiskNumber":1,"PartitionNumber":2,"DriveLetter":"S","Size":104857600}
        ]"#;
        let esp = parse_json(text);
        assert_eq!(esp.len(), 2);
        assert_eq!(esp[0].disk, 0);
        assert_eq!(esp[0].partition, 1);
        assert_eq!(esp[0].letter, None, "没有盘符要留成 None，不能当成 0 或空字符");
        assert_eq!(esp[0].size_mb, 260, "272629760 字节 = 260 MB");
        assert_eq!(esp[1].letter, Some('S'));
        assert_eq!(esp[1].root().unwrap().display().to_string(), "S:\\");
    }

    /// 单个结果时 ConvertTo-Json 出对象而不是数组 —— 不能因此整条丢掉。
    #[test]
    fn parses_single_object() {
        let esp = parse_json(r#"{"DiskNumber":2,"PartitionNumber":3,"DriveLetter":null,"Size":0}"#);
        assert_eq!(esp.len(), 1);
        assert_eq!(esp[0].describe(), "磁盘 2 分区 3", "大小 0 时不摆「0 MB」");
    }

    /// 盘符可能被序列化成码点（个别 PowerShell 版本）；大小也可能带引号。
    #[test]
    fn tolerates_alternate_scalar_shapes() {
        let esp = parse_json(r#"[{"DiskNumber":0,"PartitionNumber":1,"DriveLetter":83,"Size":"104857600"}]"#);
        assert_eq!(esp.len(), 1);
        assert_eq!(esp[0].letter, Some('S'));
        assert_eq!(esp[0].size_mb, 100);
    }

    /// 非数组、空串、坏 JSON 都只返回空表（这条路径失败不该让界面报错）。
    #[test]
    fn bad_input_yields_empty() {
        assert!(parse_json("").is_empty());
        assert!(parse_json("   ").is_empty());
        assert!(parse_json("not json").is_empty());
        assert!(parse_json("null").is_empty());
        assert!(parse_json(r#"{"DiskNumber":"x"}"#).is_empty(), "字段类型不对就跳过这一行");
    }

    /// 非 Windows 上 mount 只是占位，必须给出能读懂的理由而不是静默 Ok。
    #[test]
    fn mount_is_rejected_off_windows() {
        if cfg!(target_os = "windows") {
            return;
        }
        let error = mount(0, 1).expect_err("非 Windows 上不该成功");
        assert!(error.contains("Windows"), "{error}");
    }
}
