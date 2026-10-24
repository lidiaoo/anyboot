//! 轻量系统信息采集（不引 sysinfo 之类的大依赖）。
//!
//! 用途：客户端在「系统信息」里看到对方电脑到底是什么系统、什么版本、几核几 G，
//! 排障时一眼能对上（比如「明明是 Windows 11 却连不上」）。
//!
//! 设计取舍：
//! - **静态部分只采一次**（`OnceLock`）：OS 名 / 版本 / 内核 / CPU 型号 / 内存总量。
//!   这些在进程生命周期内不会变，反复读注册表或开子进程纯属浪费；
//! - **动态部分每次现算**：可用内存、系统运行时长；
//! - `/health` 只用 `os_family()` / `arch()` 两个常量（零开销），详细项走鉴权后的
//!   `/api/system`，避免探活接口变成信息泄露面；
//! - 只用 std + 已存在的 winreg / windows-sys，不新增 crate。

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// 给客户端看的系统信息快照（字段一律 camelCase，前端直接读）
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemInfo {
    /// 系统族：windows / macos / linux（客户端据此显示系统徽标）
    pub os: String,
    /// 系统名（Windows 11 / Ubuntu 24.04 LTS / macOS）
    pub os_name: String,
    /// 系统版本号（10.0.26200 / 24.04 / 15.1）
    pub os_version: String,
    /// 内核 / 构建号
    pub kernel: String,
    pub arch: String,
    pub hostname: String,
    /// CPU 型号（取不到为空）
    pub cpu_model: String,
    /// 逻辑核心数
    pub cpu_cores: usize,
    pub mem_total_mb: u64,
    /// 可用内存（每次采集现算；取不到为 0）
    pub mem_free_mb: u64,
    /// 系统已运行秒数（取不到为 0）
    pub uptime_secs: u64,
    /// AnyBoot 版本
    pub app_version: String,
    /// HTTP 端口（0 = 服务没起）
    pub http_port: u16,
    /// 本机局域网 IPv4 列表（客户端「这台机器有哪些地址」用）
    pub lan_ips: Vec<String>,
    /// 采集时刻（Unix 秒）
    pub collected_at: u64,
}

/// 系统族常量：windows / macos / linux（与客户端徽标一一对应）
pub(crate) fn os_family() -> &'static str {
    std::env::consts::OS
}

pub(crate) fn arch() -> &'static str {
    std::env::consts::ARCH
}

/// 静态部分：进程内只需采一次
struct StaticParts {
    os_name: String,
    os_version: String,
    kernel: String,
    cpu_model: String,
    mem_total_mb: u64,
}

fn static_parts() -> &'static StaticParts {
    static PARTS: OnceLock<StaticParts> = OnceLock::new();
    PARTS.get_or_init(|| {
        let (os_name, os_version, kernel) = os_release();
        StaticParts {
            os_name,
            os_version,
            kernel,
            cpu_model: cpu_model(),
            mem_total_mb: mem_total_mb(),
        }
    })
}

/// 采集完整快照；`http_port` 由调用方给（本机 = 监听端口，远端档案 = 0）
pub(crate) fn collect(http_port: u16, lan_ips: Vec<String>) -> SystemInfo {
    let parts = static_parts();
    SystemInfo {
        os: os_family().to_string(),
        os_name: parts.os_name.clone(),
        os_version: parts.os_version.clone(),
        kernel: parts.kernel.clone(),
        arch: arch().to_string(),
        hostname: crate::http_server::machine_name(),
        cpu_model: parts.cpu_model.clone(),
        cpu_cores: std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(0),
        mem_total_mb: parts.mem_total_mb,
        mem_free_mb: mem_free_mb(),
        uptime_secs: uptime_secs(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        http_port,
        lan_ips,
        collected_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0),
    }
}

// ==================== 各平台实现 ====================

#[cfg(target_os = "linux")]
fn os_release() -> (String, String, String) {
    let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let field = |key: &str| -> String {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(|value| value.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    let name = {
        let pretty = field("PRETTY_NAME");
        if pretty.is_empty() {
            "Linux".to_string()
        } else {
            pretty
        }
    };
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    (name, field("VERSION_ID"), kernel)
}

#[cfg(target_os = "linux")]
fn cpu_model() -> String {
    let text = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    for key in ["model name", "Hardware", "Processor"] {
        if let Some(line) = text.lines().find(|line| line.starts_with(key)) {
            if let Some((_, value)) = line.split_once(':') {
                let value = value.trim();
                if !value.is_empty() {
                    return value.to_string();
                }
            }
        }
    }
    String::new()
}

#[cfg(target_os = "linux")]
fn meminfo() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let read = |key: &str| -> u64 {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
    };
    // /proc/meminfo 单位是 kB
    (read("MemTotal:").div_ceil(1024), read("MemAvailable:").div_ceil(1024))
}

#[cfg(target_os = "linux")]
fn mem_total_mb() -> u64 {
    meminfo().0
}

#[cfg(target_os = "linux")]
fn mem_free_mb() -> u64 {
    meminfo().1
}

#[cfg(target_os = "linux")]
fn uptime_secs() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| text.split_whitespace().next().map(|v| v.to_string()))
        .and_then(|value| value.parse::<f64>().ok())
        .map(|value| value as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
fn os_release() -> (String, String, String) {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok();
    let read = |name: &str| -> String {
        key.as_ref()
            .and_then(|key| key.get_value::<String, _>(name).ok())
            .unwrap_or_default()
    };
    let product = read("ProductName"); // Windows 10 Pro / Windows 11 Pro（老版本一律写 10）
    let build = read("CurrentBuild");
    let display = read("DisplayVersion"); // 23H2
    let ubr = key
        .as_ref()
        .and_then(|key| key.get_value::<u32, _>("UBR").ok())
        .unwrap_or(0);
    // 微软的坑：Windows 11 的 ProductName 仍然写 "Windows 10"。构建号 >= 22000 才是 11。
    let major = build.parse::<u32>().unwrap_or(0);
    let family = if major >= 22000 {
        "Windows 11"
    } else if major > 0 {
        "Windows 10"
    } else {
        "Windows"
    };
    let edition = product
        .split("Windows 10")
        .nth(1)
        .or_else(|| product.split("Windows 11").nth(1))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("");
    let name = if edition.is_empty() {
        family.to_string()
    } else {
        format!("{family} {edition}")
    };
    let version = if display.is_empty() {
        format!("10.0.{build}")
    } else {
        format!("{display}（10.0.{build}.{ubr}）")
    };
    let kernel = format!("10.0.{build}.{ubr}");
    (name, version, kernel)
}

#[cfg(target_os = "windows")]
fn cpu_model() -> String {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
        .ok()
        .and_then(|key| key.get_value::<String, _>("ProcessorNameString").ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn memory_status() -> (u64, u64) {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // 失败就返回 0（调用方按「取不到」处理），不为它 panic
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return (0, 0);
    }
    const MB: u64 = 1024 * 1024;
    (
        status.ullTotalPhys / MB,
        status.ullAvailPhys / MB,
    )
}

#[cfg(target_os = "windows")]
fn mem_total_mb() -> u64 {
    memory_status().0
}

#[cfg(target_os = "windows")]
fn mem_free_mb() -> u64 {
    memory_status().1
}

#[cfg(target_os = "windows")]
fn uptime_secs() -> u64 {
    use windows_sys::Win32::System::SystemInformation::GetTickCount64;
    unsafe { GetTickCount64() / 1000 }
}

#[cfg(target_os = "macos")]
fn run(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn os_release() -> (String, String, String) {
    let version = run("sw_vers", &["-productVersion"]);
    let kernel = run("uname", &["-r"]);
    let name = match version.split('.').next().and_then(|v| v.parse::<u32>().ok()) {
        Some(major) if major >= 11 => format!("macOS {major}"),
        _ => "macOS".to_string(),
    };
    (name, version, kernel)
}

#[cfg(target_os = "macos")]
fn cpu_model() -> String {
    run("sysctl", &["-n", "machdep.cpu.brand_string"])
}

#[cfg(target_os = "macos")]
fn mem_total_mb() -> u64 {
    run("sysctl", &["-n", "hw.memsize"])
        .parse::<u64>()
        .map(|bytes| bytes / (1024 * 1024))
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn mem_free_mb() -> u64 {
    // vm_stat 的 free + inactive 页数 × 页大小（rough but honest）
    let text = run("vm_stat", &[]);
    let page_size = text
        .lines()
        .find_map(|line| line.split("page size of").nth(1))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(4096);
    let pages = |key: &str| -> u64 {
        text.lines()
            .find(|line| line.starts_with(key))
            .and_then(|line| line.split(':').nth(1))
            .map(|value| value.trim().trim_end_matches('.').to_string())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
    };
    ((pages("Pages free") + pages("Pages inactive")) * page_size) / (1024 * 1024)
}

#[cfg(target_os = "macos")]
fn uptime_secs() -> u64 {
    // kern.boottime 是结构体文本，解析太脆；改用系统启动至今的近似算法：
    // sysctl -n kern.boottime 形如 "{ sec = 1729..., usec = 0 } ..."
    let text = run("sysctl", &["-n", "kern.boottime"]);
    let boot = text
        .split("sec = ")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if boot == 0 {
        return 0;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_secs();
    now.saturating_sub(boot)
}

/// 其它平台（FreeBSD 之类）兜底：给最少的常量，不让编译挂
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn os_release() -> (String, String, String) {
    (std::env::consts::OS.to_string(), String::new(), String::new())
}
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn cpu_model() -> String {
    String::new()
}
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn mem_total_mb() -> u64 {
    0
}
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn mem_free_mb() -> u64 {
    0
}
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn uptime_secs() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_family_is_one_of_the_three_badges() {
        // 客户端只有 windows / macos / linux 三个徽标；别的值会让徽标空白
        let family = os_family();
        assert!(
            ["windows", "macos", "linux"].contains(&family),
            "未预期的系统族：{family}"
        );
    }

    #[test]
    fn collect_fills_the_basics() {
        let info = collect(12300, vec!["192.168.1.10".to_string()]);
        assert_eq!(info.os, os_family());
        assert!(!info.os_name.is_empty(), "系统名不该为空");
        assert_eq!(info.arch, arch());
        assert_eq!(info.http_port, 12300);
        assert_eq!(info.lan_ips, vec!["192.168.1.10".to_string()]);
        assert!(info.collected_at > 0);
        // CPU 核心数：任何支持的平台都该 >= 1
        assert!(info.cpu_cores >= 1, "核心数异常：{}", info.cpu_cores);
    }

    #[test]
    fn serializes_as_camel_case() {
        let value = serde_json::to_value(collect(0, Vec::new())).expect("序列化失败");
        // 前端读 camelCase（Rust 侧是 snake_case 字段名）
        assert!(value.get("osName").is_some());
        assert!(value.get("cpuCores").is_some());
        assert!(value.get("memTotalMb").is_some());
        assert!(value.get("httpPort").is_some());
        assert!(value.get("os_name").is_none(), "不该漏出 snake_case");
    }
}
