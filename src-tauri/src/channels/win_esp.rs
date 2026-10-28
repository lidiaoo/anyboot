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
    // 挂载动作可能真的改了分区表（也可能被用户拒绝），先作废缓存再回读 ——
    // 不作废就会读到 60 秒前那张「还没有盘符」的旧表，回读永远失败
    super::partitions::invalidate();
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
