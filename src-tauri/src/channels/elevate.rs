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
#![cfg(any(target_os = "windows", target_os = "linux", test))]

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

/// 一次特权操作的完整投递：**先问常驻服务，服务不在才弹授权**。
///
/// 顺序不能反。远程 / 无人值守时 UAC 对话框弹在主机的交互桌面上（手机上永远
/// 看不到），反过来的话每一次特权操作都会先卡 30 秒再失败。而服务不可用
/// （没装、被删、被策略禁）时必须能退回弹出授权 —— 有人在机器前的时候，
/// 「装个服务」不该是使用前提。
///
/// 三种结果的处置不同，别合并：
/// - 服务做完了 → 直接返回，**不要再弹 UAC**；
/// - 服务明确失败 → 立刻返回错误。再弹一次 UAC 只会把同一个错误再报一遍，
///   还要用户多点一次确认；
/// - 服务不可用 → 走 `fallback_args` 那条老路（Windows 弹 UAC / Linux pkexec）。
pub(crate) fn run_privileged_op(
    op: &str,
    service_args: serde_json::Value,
    fallback_args: &[String],
    out_path: &std::path::Path,
) -> Result<(), String> {
    match try_service(op, service_args, out_path) {
        ServiceOutcome::Served => return Ok(()),
        ServiceOutcome::Failed(reason) => return Err(reason),
        ServiceOutcome::Unavailable(_) => {}
    }
    run_elevated(fallback_args, out_path)
}

enum ServiceOutcome {
    Served,
    Unavailable(String),
    Failed(String),
}

#[cfg(target_os = "windows")]
fn try_service(op: &str, args: serde_json::Value, out_path: &std::path::Path) -> ServiceOutcome {
    // 上一次残留不能被当成新结果（与 run_elevated 同一条纪律）
    let _ = std::fs::remove_file(out_path);
    match crate::service_win::call(op, args) {
        crate::service_win::ServiceCall::Ok(body) => {
            let path = out_path.to_string_lossy().to_string();
            if super::write_result_file(&path, &body) == 0 {
                ServiceOutcome::Served
            } else {
                ServiceOutcome::Failed("特权服务算完了，但结果落盘失败（磁盘满 / 目录只读？）".to_string())
            }
        }
        crate::service_win::ServiceCall::Unavailable(why) => ServiceOutcome::Unavailable(why),
        crate::service_win::ServiceCall::Failed(why) => ServiceOutcome::Failed(why),
    }
}

#[cfg(not(target_os = "windows"))]
fn try_service(_op: &str, _args: serde_json::Value, _out_path: &std::path::Path) -> ServiceOutcome {
    // 只有 Windows 有这个常驻服务（macOS 走 launchd 助手，Linux 走 pkexec）
    ServiceOutcome::Unavailable("这个平台没有特权服务".to_string())
}

/// 把参数拼成 `lpParameters` 里的那一条命令行（Windows 规则）。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn build_params(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Windows 命令行的单个参数（`CommandLineToArgvW` 的规则）。
///
/// **光给每个参数套引号是不够的**：参数以反斜杠结尾时（`S:\`、`\EFI\`），
/// 结尾那个引号会被前一个反斜杠转义掉（`\"` 在 argv 里是字面引号），于是它
/// 与后面的参数黏成一段，子进程拿到的每个值都错位 —— 而且通常不会报错，
/// 只会「设默认项失败」或「路径不存在」。规则：
/// * 反斜杠只有「紧贴引号」或「贴末尾」时才需要翻倍；
/// * 引号本身写成 `\"`。
///
/// 现有调用方传的路径都不以反斜杠结尾，但 `S:\` 这种写法迟早会进来（卷根就是
/// 标准形态），所以这里按规则做对，而不是靠「恰好没踩到」。
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn quote_arg(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                for _ in 0..backslashes * 2 + 1 {
                    out.push('\\');
                }
                out.push('"');
                backslashes = 0;
            }
            other => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                out.push(other);
                backslashes = 0;
            }
        }
    }
    // 结尾的反斜杠必须翻倍，否则会把我们补上的收尾引号转义掉
    for _ in 0..backslashes * 2 {
        out.push('\\');
    }
    out.push('"');
    out
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
    let params = build_params(args);
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

/// 只在 `cfg(test)` 下需要存在：让整个模块能在开发机（macOS）上编译，
/// 好让 [`quote_arg`] 这种纯字符串逻辑跑单测。真机没有这条路。
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn spawn_and_wait(_args: &[String], _out_path: &str) -> Result<(), String> {
    Err("这个平台没有提权通道".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 命令行的引号规则。错在这里的样子是「子进程收到的参数整体串位」，
    /// 只会表现为真机上「写入失败 / 路径不存在」，肉眼 review 看不出来。
    #[test]
    fn argv_quoting_survives_trailing_backslash() {
        assert_eq!(quote_arg("--out"), r#""--out""#);
        assert_eq!(quote_arg(r#"C:\Temp\a b.json"#), r#""C:\Temp\a b.json""#);
        // 结尾反斜杠：不翻倍就会把收尾引号吃掉（卷根 `S:\` 正是这个形状）
        assert_eq!(quote_arg(r#"S:\"#), r#""S:\\""#);
        assert_eq!(quote_arg(r#"\EFI\OC\"#), r#""\EFI\OC\\""#);
        // 中间的反斜杠不翻倍
        assert_eq!(quote_arg(r#"\\?\C:\x"#), r#""\\?\C:\x""#);
        // 引号本身要转义，且它前面的反斜杠要翻倍
        assert_eq!(quote_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_arg(r#"a\"b"#), r#""a\\\"b""#);
        // 空参数也要成立（否则会与下一个参数黏在一起）
        assert_eq!(quote_arg(""), r#""""#);
    }

    #[test]
    fn params_keep_one_token_per_arg() {
        let params = build_params(&[
            "--oc-set-default".to_string(),
            "--esp".to_string(),
            r#"S:\"#.to_string(),
            "--out".to_string(),
            r#"C:\Temp\x.json"#.to_string(),
        ]);
        assert_eq!(
            params,
            r#""--oc-set-default" "--esp" "S:\\" "--out" "C:\Temp\x.json""#
        );
    }
}
