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

/// 本机所有 EFI 系统分区（按 GPT 类型认，不需要盘符）。
///
/// 数据来自 [`super::partitions`] 的共享分区表（一次 `Get-Partition` + 60s 缓存），
/// 不再自己起 PowerShell —— 一轮扫描里这张表会被要 3 次（ESP 列表、候选卷、
/// 若干 generic 条目的卷反查），每次都起进程曾是 3.7s 的来源。
///
/// 失败一律返回空表：这条只是「让提示说得准」，不该因为读表失败把整条链路弄挂。
pub(crate) fn list() -> Vec<WinEsp> {
    super::partitions::all()
        .into_iter()
        .filter(super::partitions::Partition::is_esp)
        .map(|row| WinEsp {
            disk: row.disk,
            partition: row.partition,
            letter: row.letter,
            size_mb: row.size_mb(),
        })
        .collect()
}


/// 给一块没有盘符的 ESP 分配盘符（`Add-PartitionAccessPath -AssignDriveLetter`）。
///
/// **走统一的提权通道**：特权服务优先（常驻、免 UAC），服务不可用才弹一次授权。
/// 之前这里是裸 `Start-Process -Verb RunAs`，等于把「ESP 没盘符」这类机器上的
/// 所有通道动作都锁死在「必须有人守在屏幕前」—— 而挂载恰恰是那些机器的前置条件，
/// 前置条件卡住，后面全部免谈。
///
/// 回读校验保留：盘符是「分配完才能知道结果」的操作，真拿到 `X:\` 才算成。
#[cfg(target_os = "windows")]
pub(crate) fn mount(disk: u32, partition: u32) -> Result<String, String> {
    let body = super::elevate::run_op(
        "esp_assign_letter",
        serde_json::json!({ "disk": disk, "partition": partition }),
    )?;
    let reply: LetterReply =
        serde_json::from_str(&body).map_err(|error| format!("特权侧返回的不是合法 JSON：{error}"))?;
    // 父进程的分区表也有 60 秒缓存：「还没有盘符」是挂载之前的事实，不作废就永远
    // 回读失败（哪怕盘符已经真的分配好了）。
    super::partitions::invalidate();
    if !reply.ok {
        return Err(reply
            .error
            .unwrap_or_else(|| "分配盘符失败（没给原因）".to_string()));
    }
    for esp in list() {
        if esp.disk == disk && esp.partition == partition {
            if let Some(root) = esp.root() {
                return Ok(root.display().to_string());
            }
            return Err(
                "授权成功但系统没能给这块 EFI 分区分配盘符（盘符可能已经用完了）".to_string(),
            );
        }
    }
    Err("挂载后没再找到这块 EFI 分区：点「重新识别」再看一次".to_string())
}

/// 特权侧的回传体。
#[derive(serde::Deserialize)]
struct LetterReply {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

/// 跑一次「分配盘符」并序列化成 JSON 文本（服务与提权子进程共用）。
#[cfg(target_os = "windows")]
pub(crate) fn assign_letter_json(disk: u32, partition: u32) -> Result<String, String> {
    let payload = match assign_letter_direct(disk, partition) {
        Ok(root) => serde_json::json!({ "ok": true, "root": root }),
        Err(error) => serde_json::json!({ "ok": false, "error": error }),
    };
    serde_json::to_string(&payload).map_err(|error| format!("序列化结果失败：{error}"))
}

/// 直接调 `Add-PartitionAccessPath`。**不要再套一层 RunAs**：这条路本身就跑在
/// 特权服务 / 提权子进程里，里面再 `Start-Process -Verb RunAs` 只会又弹一次 UAC
/// 然后失败（Session 0 里连弹都弹不出来）。
#[cfg(target_os = "windows")]
fn assign_letter_direct(disk: u32, partition: u32) -> Result<String, String> {
    // 只用两个数字参数拼脚本，不存在引号/注入面
    let script = format!(
        "Add-PartitionAccessPath -DiskNumber {disk} -PartitionNumber {partition} -AssignDriveLetter"
    );
    let output = command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法调用 PowerShell 分配盘符：{error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(if detail.is_empty() {
            "分配盘符的 PowerShell 返回失败（多半是没有空闲盘符了）".to_string()
        } else {
            format!("分配盘符失败：{detail}")
        });
    }
    // 刚刚可能真的改了盘符分配，先作废分区表缓存再回读。
    // **要重试几次**：`Add-PartitionAccessPath` 返回成功时盘符已经定了，但
    // `Get-Partition` 反映出来可能晚一拍（卷管理器的挂载通知是异步的）。
    // 只试一次会把「其实成功了」报成失败，用户照着提示再点一次，又得到
    // 「这块分区已经有盘符了」—— 最容易让人怀疑程序坏了的那种来回。
    for attempt in 0..3 {
        super::partitions::invalidate();
        for esp in list() {
            if esp.disk == disk && esp.partition == partition {
                if let Some(root) = esp.root() {
                    return Ok(root.display().to_string());
                }
            }
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }
    Err("分配盘符的请求成功了，但重新枚举三次都没看到这块 EFI 分区有盘符\
         （盘符可能已经用完了；也可能需要点「重新识别」再看一次）"
        .to_string())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn mount(_disk: u32, _partition: u32) -> Result<String, String> {
    Err("只有 Windows 需要给 EFI 分区分配盘符".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
