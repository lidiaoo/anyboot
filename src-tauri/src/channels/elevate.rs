//! 按需提权：弹一次 UAC（Windows）/ pkexec（Linux），让**本程序自己**带参数跑
//! 一个子命令，结果经 JSON 文件回传。目前两个用户：
//!
//! * APFS Preboot 直读（`--apfs-scan-privileged`，见 `apfs_reader.rs`）；
//! * OC 设默认项（`--oc-set-default`，见 `oc_default.rs`）。
//!
//! 为什么不做成 macOS 那样的常驻助手：这两件事都是「偶尔来一次」，一次性的
//! ShellExecute/pkexec 没有安装步骤、没有守护项、用完即退，安全面小得多。
//!
//! 与 blocking_guard 的关系：**调用方必须在 async 命令的工作线程上**——
//! Windows 的 ShellExecuteExW 会同步等 UAC 对话框，用户可能想几分钟再点。
#![cfg(any(target_os = "windows", target_os = "linux"))]

/// 回传文件放 %TEMP%：主进程与提权子进程是同一个用户（令牌只提权不改身份），
/// %TEMP% 同目录同权限；用完留在原地由系统清理。
///
/// `tag` 区分用途，避免扫描和设默认互相吃到对方的残留文件。
pub(crate) fn result_file_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("anyboot-{tag}-{}.json", std::process::id()))
}

/// 拉起提权子进程并**等它退出**。`args` 是完整参数（不含程序自身路径）；
/// 结果由子进程原子写进 `out_path`，调用方负责读取与解析。
pub(crate) fn run_elevated(args: &[String], out_path: &std::path::Path) -> Result<(), String> {
    // 上一次残留（崩溃/强杀）不能被当成新结果
    let _ = std::fs::remove_file(out_path);
    let out_text = out_path.to_string_lossy().to_string();
    spawn_and_wait(args, &out_text)
}

#[cfg(target_os = "windows")]
fn spawn_and_wait(args: &[String], _out_path: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_CANCELLED, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let exe = std::env::current_exe().map_err(|error| format!("拿不到自身路径：{error}"))?;
    // 每个参数都套引号：目标串里可能有空格（卷标）、反斜杠（路径），引号最省心
    let params = args
        .iter()
        .map(|arg| format!("\"{arg}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let to_wide = |text: &str| -> Vec<u16> {
        std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let verb = to_wide("runas");
    let file = to_wide(&exe.as_os_str().to_string_lossy());
    let args_wide = to_wide(&params);
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    // NOASYNC：调用线程等到 UAC 对话框有了结果；NOCLOSEPROCESS：回填 hProcess 好等子进程
    info.fMask = SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = args_wide.as_ptr();
    info.nShow = SW_HIDE;
    // SAFETY：四个 wide 串在本函数栈上活到调用结束；结构体按 API 要求完整初始化。
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let code = unsafe { GetLastError() };
        return Err(if code == ERROR_CANCELLED {
            "已取消授权 —— 操作没有执行，需要时再试一次".to_string()
        } else {
            format!("弹授权失败（错误码 {code}）")
        });
    }
    let wait = unsafe { WaitForSingleObject(info.hProcess, 120_000) };
    unsafe { CloseHandle(info.hProcess) };
    match wait {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err("提权子进程 120 秒没退出（被卡住了？）".to_string()),
        code => Err(format!("等待提权子进程异常（0x{code:08X}）")),
    }
}

#[cfg(target_os = "linux")]
fn spawn_and_wait(args: &[String], _out_path: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| format!("拿不到自身路径：{error}"))?;
    // pkexec 会一直等到目标程序退出（取消授权则立刻失败），正好当同步等待用
    let status = command("pkexec")
        .arg(exe)
        .args(args)
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("无法调起 pkexec（装 polkit 了吗）：{error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("授权被取消或子命令失败（pkexec 非零退出）".to_string())
    }
}
