//! Windows 特权助手服务（`AnyBootHelper`）：让特权操作**不再需要 UAC**。
//!
//! # 为什么需要它
//!
//! 无人值守 / 远程（手机上的 hub 页面）场景里，UAC 对话框弹在**主机的交互桌面**
//! 上：人看不到，也就永远点不到；机器没人登录时连桌面都没有，
//! `ShellExecuteExW(runas)` 直接失败。而读固件变量（1314）、打开
//! `\\.\PhysicalDriveN` 都被「管理员令牌」卡着。
//!
//! 唯一的出路是**装一次、之后免提权**：常驻一个 LocalSystem 服务，App 把请求发给
//! 它，由它代做。这与 macOS 那边的 launchd 助手是同一个思路（见 `helper_proto`），
//! 区别只是传输层：macOS 用文件信箱，Windows 用命名管道。
//!
//! # 协议
//!
//! 一条连接一次请求，字节流 + `\n` 分帧：
//!
//! ```text
//! 请求：{"v":1,"op":"oc_set_default","args":{"target":"...","esp":"..."}}\n
//! 应答：{"ok":true,"body":"<与 --out 文件内容完全一致的 JSON 文本>"}\n
//!       {"ok":false,"error":"人话原因"}\n
//! ```
//!
//! `body` 是**字符串**而不是嵌套对象：这样服务回的就是「原本要写进 `--out` 的那段
//! 文本」，客户端原样落盘，下游解析器一个字都不用改（也不会出现「双重编码后字段
//! 对不上」这种最难查的错误）。
//!
//! # 授权边界（这段是设计，不是「暂时」）
//!
//! - **op 白名单只有 9 个**（读 / 写固件变量、设 OC 默认项、GRUB 探测与写 grubenv、
//!   systemd-boot one-shot、注册固件启动项、给 ESP 分配盘符），写死在 [`dispatch`]
//!   里；不接受任意命令行、不执行 shell。**这也是 `--privileged-op` 子命令的实现**，
//!   所以「走服务」和「弹 UAC 兜底」跑的是同一份代码。
//! - `linux_grub_write` 的 `rel` 只认 `boot/grub{,2}/grubenv` 两个常量，不接受任意
//!   路径 —— 否则「写 1024 字节」就变成「能改 ext4 里任意文件的前 1024 字节」。
//! - 请求里没有「读/写哪块盘」的自由：分区由服务自己按分区表枚举。
//! - 管道 DACL 只给 SYSTEM / Administrators / Authenticated Users（`PIPE_REJECT_REMOTE_CLIENTS`
//!   挡掉远程 SMB），并**校验调用方映像路径必须等于本服务自己的 exe** —— 也就是说
//!   只有装在本程序安装目录里的 anyboot.exe 能调它，别的进程（哪怕是同用户）不行。
//!
//! 仍然存在的风险：**同用户**的任何进程只要有办法把代码注入进 anyboot.exe，就等于
//! 拿到这些能力。这与 macOS 助手文档里承认的那条是同一类（能力有限、不给 shell），
//! 不打算假装能消除。
//!
//! # 安装 / 卸载
//!
//! Windows 的服务注册和安装包之间**没有任何关联**：卸载程序不会自动删服务。所以
//! 注册与删除都由我们自己负责三处——安装器（NSIS 钩子 / MSI）、`--install-service`
//! 与 `--uninstall-service` 子命令、以及卸载时「先停再删再删文件」的顺序。
#![cfg(target_os = "windows")]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FlushFileBuffers, ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    WaitNamedPipeW,
};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfig2W, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, RegisterServiceCtrlHandlerW,
    SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW, SC_ACTION, SERVICE_DESCRIPTIONW,
    SERVICE_FAILURE_ACTIONSW, SERVICE_STATUS, SERVICE_STATUS_PROCESS, SERVICE_TABLE_ENTRYW,
};
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW};

pub(crate) const SERVICE_NAME: &str = "AnyBootHelper";
const SERVICE_DISPLAY: &str = "AnyBoot 特权助手";
const SERVICE_DESC: &str = "为 AnyBoot 提供免 UAC 的固件变量与磁盘读写能力（供远程 / 无人值守使用）";
pub(crate) const PIPE_NAME: &str = r"\\.\pipe\anyboot-helper";

// ── 常量：宁可本地定义，也不猜模块路径 ──────────────────────────────────────
// windows-sys 把 winerror.h 的符号分散在几个模块里，版本间搬来搬去；这里只用到
// 十来个数字，写死并注明来源比「编译报错再猜」省事，也避免升级时静默改语义。
const ERR_ACCESS_DENIED: u32 = 5;
const ERR_FILE_NOT_FOUND: u32 = 2;
const ERR_BROKEN_PIPE: u32 = 109;
const ERR_PIPE_BUSY: u32 = 231;
const ERR_PIPE_CONNECTED: u32 = 535;
const ERR_SERVICE_DOES_NOT_EXIST: u32 = 1060;
const ERR_SERVICE_EXISTS: u32 = 1073;
const ERR_SERVICE_ALREADY_RUNNING: u32 = 1056;
/// 服务已标记删除（`DeleteService` 之后、SCM 真正移除之前）。
const ERR_SERVICE_MARKED_FOR_DELETE: u32 = 1072;

const SC_MANAGER_CONNECT: u32 = 0x0001;
const SC_MANAGER_CREATE_SERVICE: u32 = 0x0002;
const SERVICE_QUERY_STATUS: u32 = 0x0004;
const SERVICE_STOP: u32 = 0x0020;
const SERVICE_DELETE: u32 = 0x0001_0000;
const SERVICE_ALL_ACCESS: u32 = 0xF01FF;
const SERVICE_WIN32_OWN_PROCESS: u32 = 0x0000_0010;
const SERVICE_AUTO_START: u32 = 0x0000_0002;
const SERVICE_ERROR_NORMAL: u32 = 0x0000_0001;
const SERVICE_CONTROL_STOP: u32 = 0x0000_0001;
const SERVICE_STOPPED: u32 = 0x0000_0001;
const SERVICE_START_PENDING: u32 = 0x0000_0002;
const SERVICE_RUNNING: u32 = 0x0000_0004;
const SERVICE_ACCEPT_STOP: u32 = 0x0000_0001;
const SERVICE_CONFIG_DESCRIPTION: u32 = 1;
const SERVICE_CONFIG_FAILURE_ACTIONS: u32 = 2;
const SC_ACTION_RESTART: i32 = 0x0000_0001;
const SC_STATUS_PROCESS_INFO: i32 = 0;

const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
const PIPE_WAIT: u32 = 0x0000_0000;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;
const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

/// 单条请求的上限：最大的是 grub 写入（1024 字节 → 2048 个 hex 字符），
/// 1 MiB 已经宽到不可能被正常请求碰到，但能挡住「灌一个无限长的行」。
const MAX_REQUEST_BYTES: usize = 1 << 20;
/// 客户端等应答的上限。**必须是「还没回就是坏了」而不是「慢」**：扫描十块
/// ext4 分区实测 2.3 秒（冷缓存十几秒），30 秒是上限。
const CLIENT_TIMEOUT_MS: u64 = 30_000;

fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
}

fn log_line(message: &str) {
    log_line_to("service.log", message);
}

/// 往 `%ProgramData%\AnyBoot\<file_name>` 追加一行。服务与安装器共用：
/// 服务写 `service.log`，安装器写 `install.log`（分开放，否则常驻服务的
/// 心跳会把安装失败的记录冲得看不见）。
fn log_line_to(file_name: &str, message: &str) {
    // 服务在 Session 0，没有任何前台可写；排障只能靠文件。失败就静默，
    // 日志写不进去绝不能让业务失败。
    let Ok(program_data) = std::env::var("ProgramData") else {
        return;
    };
    let dir = std::path::Path::new(&program_data).join("AnyBoot");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(file_name);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(file, "[{stamp}] {message}");
    }
}

/// 安装器里的自定义动作是 `Return="ignore"`：失败既不弹框也不阻断安装，
/// 用户在「装完了但免 UAC 不生效」时没有任何线索。所以安装/卸载的**每次**
/// 结果（连同是哪个 exe 在尝试）都记一行 —— 排查时先看这个文件。
fn log_installer_result(tag: &str, result: &Result<String, String>) {
    let exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    let line = match result {
        Ok(detail) => format!("{tag} ok: {detail} [exe: {exe}]"),
        Err(error) => format!("{tag} FAILED: {error} [exe: {exe}]"),
    };
    log_line_to("install.log", &line);
}


// ─────────────────────────────── 服务端 ───────────────────────────────

/// 停止标志。控制处理函数（SCM 线程）写，管道线程读。
static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// 正在处理中的请求数。停止服务时要等它们跑完再报 STOPPED ——
/// 半途把一个正在写 NVRAM / 写 ESP 的请求砍断，比「多等半秒」糟得多。
static INFLIGHT: AtomicUsize = AtomicUsize::new(0);

/// 归零计数用：**用 Drop 而不是在末尾手动减**，否则一个 panic 就把计数永久顶住，
/// 之后每次停服务都要白等满 5 秒。
struct InflightGuard;
impl InflightGuard {
    fn enter() -> Self {
        INFLIGHT.fetch_add(1, Ordering::Relaxed);
        InflightGuard
    }
}
impl Drop for InflightGuard {
    fn drop(&mut self) {
        INFLIGHT.fetch_sub(1, Ordering::Relaxed);
    }
}

/// `--run-service` 子命令：进 SCM 分发器，**绝不能**再往下走到 Tauri
/// （那会开窗口、抢单实例锁、监听端口，服务里全是灾难）。
pub(crate) fn run_service() -> i32 {
    let name = wide(SERVICE_NAME);
    let mut table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_ptr() as *mut u16,
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: std::ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    // SAFETY：表以全零项结尾；`name` 活到调用结束。
    let ok = unsafe { StartServiceCtrlDispatcherW(table.as_mut_ptr()) };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        return if code == ERR_ACCESS_DENIED { 2 } else { 1 };
    }
    0
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    let name = wide(SERVICE_NAME);
    let handle = unsafe { RegisterServiceCtrlHandlerW(name.as_ptr(), Some(control_handler)) };
    if handle.is_null() {
        return;
    }
    report(handle, SERVICE_START_PENDING, 2000);
    let stop = Arc::new(AtomicBool::new(false));
    let _ = STOP.set(stop.clone());
    log_line("service starting");
    report(handle, SERVICE_RUNNING, 0);
    std::thread::spawn(move || serve_loop(stop));
    // 主线程只做「等服务结束」：SCM 要求 stop 后尽快报 STOPPED，
    // 所以在控制回调里置标志，这里 200ms 轮询。
    while !STOP.get().map(|flag| flag.load(Ordering::Relaxed)).unwrap_or(true) {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    log_line("service stopping");
    // 等处理中的请求跑完再报 STOPPED（最多 5 秒）：切系统这类写入被半途砍断，
    // 留下的是「NVRAM 写了一半」或「state.json 与固件不一致」，比慢一点严重得多。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while INFLIGHT.load(Ordering::Relaxed) > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    report(handle, SERVICE_STOPPED, 0);
}

unsafe extern "system" fn control_handler(control: u32) {
    if control == SERVICE_CONTROL_STOP {
        if let Some(flag) = STOP.get() {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

unsafe fn report(handle: HANDLE, state: u32, wait_hint: u32) {
    let mut status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP } else { 0 },
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    unsafe { SetServiceStatus(handle, &mut status) };
}

/// 接受连接的主循环：**连上就交给独立线程，自己立刻回去等下一个**。
///
/// 曾经的写法是 `handle_client(pipe)` 同步跑完再 accept 下一个 —— 于是服务是串行的：
/// 一次 APFS 扫描（十块盘，冷缓存十几秒）期间，任何并发请求都撞上 `ERROR_PIPE_BUSY`；
/// 客户端那边只等 5 秒，等不到就报「特权服务忙」且**不回退 UAC**（那是 `Failed` 不是
/// `Unavailable`）。现在「切系统」这种必须秒回的操作和「扫描」这种慢操作能并存了。
///
/// 停止服务时不 join 这些线程：`service_main` 报完 STOPPED 就返回，进程退出把它们
/// 一起带走；在此之前 `INFLIGHT` 那道闸已经保证没有写到一半的请求。
fn serve_loop(stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        let pipe = match create_pipe_instance() {
            Ok(pipe) => pipe,
            Err(error) => {
                // 拿不到管道（极少见：句柄耗尽）。别空转把 CPU 打满。
                log_line(&format!("create pipe failed: {error}"));
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };
        // 在这里（而不是工作线程里）等连接：一个实例只服务一个客户端，等连接占住
        // 主循环没关系（下一个实例建出来就有了并行度）；反过来「先 spawn 再等连接」
        // 会在没有客户端时无限堆线程。
        let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) };
        if connected == 0 {
            let code = unsafe { GetLastError() };
            if code != ERR_PIPE_CONNECTED {
                log_line(&format!("ConnectNamedPipe failed: {code}"));
                unsafe { CloseHandle(pipe) };
                continue;
            }
        }
        // 句柄以 `usize` 形式搬进线程：`HANDLE` 是裸指针、没有 `Send`，而闭包的
        // 精确捕获会直接抓到那个字段（连包裹类型都救不了）。转成整数只是绕过
        // 类型系统这道闸，语义上没问题 —— 管道句柄是内核对象，在哪个线程用都行。
        let raw = pipe as usize;
        std::thread::spawn(move || {
            let pipe = raw as HANDLE;
            let guard = InflightGuard::enter();
            handle_client(pipe);
            // 先归零再断开：断开那一刻已经不需要再拦停服务了
            drop(guard);
            unsafe {
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
        });
    }
}

fn create_pipe_instance() -> Result<HANDLE, String> {
    let name = wide(PIPE_NAME);
    // DACL：SYSTEM 与管理員全权，Authenticated Users 只读写本管道。
    // GA = GENERIC_ALL，GR/GW = GENERIC_READ/WRITE。
    let sddl = wide("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)");
    let mut descriptor: *mut std::ffi::c_void = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut descriptor, std::ptr::null_mut())
    };
    if ok == 0 {
        return Err(format!("构造管道 DACL 失败（{}）", unsafe { GetLastError() }));
    }
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            &mut attributes,
        )
    };
    unsafe { LocalFree(descriptor) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!("CreateNamedPipe 失败（{}）", unsafe { GetLastError() }));
    }
    Ok(handle)
}

fn read_request(pipe: HANDLE) -> Result<String, String> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = vec![0u8; 4096];
    loop {
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                pipe,
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let code = unsafe { GetLastError() };
            // 客户端写一半就关（崩溃）：把已有内容当请求处理，让错误信息更准
            if code == ERR_BROKEN_PIPE && !buffer.is_empty() {
                break;
            }
            return Err(format!("读请求失败（{code}）"));
        }
        if read == 0 {
            break; // 对端半关
        }
        buffer.extend_from_slice(&chunk[..read as usize]);
        if buffer.len() > MAX_REQUEST_BYTES {
            return Err("请求超过 1 MiB 上限".to_string());
        }
        if buffer.contains(&b'\n') {
            break;
        }
    }
    let text = String::from_utf8(buffer).map_err(|error| format!("请求不是 UTF-8：{error}"))?;
    Ok(text.trim().to_string())
}

fn write_reply(pipe: HANDLE, payload: &Value) -> Result<(), String> {
    let mut text = serde_json::to_string(payload).map_err(|error| format!("序列化应答失败：{error}"))?;
    text.push('\n');
    let bytes = text.as_bytes();
    let mut written = 0usize;
    while written < bytes.len() {
        let mut count = 0u32;
        let ok = unsafe {
            WriteFile(
                pipe,
                bytes[written..].as_ptr(),
                (bytes.len() - written) as u32,
                &mut count,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || count == 0 {
            return Err(format!("写应答失败（{}）", unsafe { GetLastError() }));
        }
        written += count as usize;
    }
    unsafe { FlushFileBuffers(pipe) };
    Ok(())
}

fn handle_client(pipe: HANDLE) {
    // 顺序是「先收请求、再认人、最后回话」，不能先认人再收：
    // 调用方是先写完请求才开始读应答的，我们在它写的过程中就拒连会导致
    // 它那边收到的是 EPIPE（报「发请求失败 109」），真正的原因（调用方不是
    // 本程序）就永远传不回去 —— 安全拒绝路径上最不该丢的就是原因。
    let reply = match read_request(pipe) {
        Ok(text) => match caller_is_our_exe(pipe) {
            // 带上 code：调用方不是「安装目录里那份」（便携副本、旧副本）是
            // **可用性问题**，不是操作失败 —— 客户端要据此改走 UAC 兜底，
            // 否则「本机装了服务」反而把便携版的特权操作全堵死。
            Err(reason) => json!({ "ok": false, "code": CODE_NOT_OUR_EXE, "error": reason }),
            Ok(()) => match serde_json::from_str::<Value>(&text) {
                Ok(request) => match dispatch(&request) {
                    Ok(body) => json!({ "ok": true, "body": body }),
                    Err(error) => {
                        log_line(&format!("op failed: {error}"));
                        let mut reply = json!({ "ok": false, "error": error });
                        if error.starts_with(UNKNOWN_OP_PREFIX) {
                            reply["code"] = json!(CODE_UNKNOWN_OP);
                        }
                        reply
                    }
                },
                Err(error) => json!({ "ok": false, "error": format!("请求不是合法 JSON：{error}") }),
            },
        },
        Err(error) => json!({ "ok": false, "error": error }),
    };
    let _ = write_reply(pipe, &reply);
}

/// 调用方必须就是本服务这个 exe。
///
/// 这条检查挡的是「同机器上别的程序直接连管道让 SYSTEM 替它干活」。它不是万能的
/// （能注入本进程的代码仍可绕过），但把「随便写个脚本就能改默认启动项」降到
/// 「得先能改安装目录里的文件或注入进程」，而安装目录是管理员独占可写的。
fn caller_is_our_exe(pipe: HANDLE) -> Result<(), String> {
    let mut pid = 0u32;
    if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) } == 0 {
        return Err("拿不到调用方进程（拒绝执行）".to_string());
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err("打不开调用方进程（拒绝执行）".to_string());
    }
    let mut buffer = vec![0u16; 1024];
    let mut size = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(process) };
    if ok == 0 {
        return Err("读不到调用方映像路径（拒绝执行）".to_string());
    }
    let caller = String::from_utf16_lossy(&buffer[..size as usize]);
    let ours = std::env::current_exe()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    if !same_path(&caller, &ours) {
        log_line(&format!("rejected caller: {caller} (ours: {ours})"));
        return Err(format!(
            "调用方不是本程序的安装副本（{caller}），拒绝执行。请用安装目录里的 anyboot.exe"
        ));
    }
    Ok(())
}

fn same_path(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let normalize = |text: &str| {
        std::fs::canonicalize(text)
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|_| text.to_string())
            .to_lowercase()
    };
    normalize(left) == normalize(right)
}

/// op 白名单。**新增能力只能加在这里**，不要开放「任意命令行」——
/// 那就等于给同用户一个 SYSTEM shell。
///
/// 也是「弹 UAC 那条路」的同一份实现：`--privileged-op` 子命令直接调本函数，
/// 所以服务与兜底两条路的语义永远一致（参数校验、错误文案、返回体形状）。
pub(crate) fn dispatch(request: &Value) -> Result<String, String> {
    let op = request
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| "请求缺少 op".to_string())?;
    let args = request.get("args").cloned().unwrap_or_else(|| json!({}));
    let text_arg = |name: &str| -> Result<String, String> {
        args.get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("{op} 缺少参数 {name}"))
    };
    // 数字参数一律走 JSON number：字符串数字会被 serde 当成 `null`，这里不接受
    // 「看起来像数字的字符串」—— 少一条隐式转换就少一种歧义。
    let num_arg = |name: &str| -> Result<u32, String> {
        args.get(name)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| format!("{op} 缺少数字参数 {name}"))
    };
    match op {
        "apfs_scan" => crate::channels::apfs_reader::privileged_scan_json(),
        "nvram_dump" => crate::channels::nvram::privileged_dump_json(),
        "oc_set_default" => {
            let target = text_arg("target")?;
            let esp = args.get("esp").and_then(Value::as_str);
            crate::channels::oc_default::set_default_json(&target, esp)
        }
        "linux_grub_probe" => {
            let esp = args.get("esp").and_then(Value::as_str).unwrap_or("");
            crate::channels::linux_grub::probe_child_json(esp)
        }
        "linux_grub_write" => {
            let dev = text_arg("dev")?;
            let rel = text_arg("rel")?;
            // 只认这两个常量。放开成任意相对路径 = 「能覆盖 ext4 里任意文件的
            // 前 1024 字节」，那是另一回事了。
            if rel != "boot/grub/grubenv" && rel != "boot/grub2/grubenv" {
                return Err(format!("不接受这个写入路径（只允许 grubenv）：{rel}"));
            }
            let payload_hex = text_arg("payload_hex")?;
            let bytes = hex::decode(&payload_hex).map_err(|error| format!("payload 不是合法 hex：{error}"))?;
            crate::channels::linux_grub::write_child_json(&dev, &rel, &bytes)
        }
        // ── 固件启动项写入 ───────────────────────────────────────────────
        // 为什么放在白名单里：「下次启动进 OpenCore」「注册固件启动项」是远程
        // **最常用**的动作，而写 Boot* 的天生门槛就是管理员令牌。参数一律收
        // 「通道名 / 注册方式 / 条目 id」，不收变量名也不收原始字节 —— 服务端自己
        // 按分区与分区表算出该写什么。
        "nvram_boot_next" => {
            let channel = text_arg("channel")?;
            crate::channels::opencore::boot_next_json(&channel, args.get("esp").and_then(Value::as_str))
        }
        "nvram_register_boot" => {
            let mode = args.get("mode").and_then(Value::as_str).unwrap_or("");
            crate::channels::opencore::register_boot_entry_json(
                mode,
                args.get("esp").and_then(Value::as_str),
            )
        }
        "sdboot_one_shot" => {
            let entry = text_arg("entry")?;
            crate::channels::sdboot::one_shot_json(args.get("esp").and_then(Value::as_str), &entry)
        }
        // ── 给 ESP 分配盘符 ─────────────────────────────────────────────
        // 只收两个数字：磁盘号、分区号。脚本由服务端拼，调用方碰不到命令行。
        "esp_assign_letter" => {
            let disk = num_arg("disk")?;
            let partition = num_arg("partition")?;
            crate::channels::win_esp::assign_letter_json(disk, partition)
        }
        other => Err(format!("{UNKNOWN_OP_PREFIX} {other}（白名单只有 9 个）")),
    }
}

/// 「这个 op 我不认识」的固定前缀。
///
/// 它不只用来写错误文案：**升级期的版本错配全靠它识别**。装了新程序、服务却还是
/// 老进程（升级后没重启）时，服务会回这一条；客户端据此改判成「服务不可用」→
/// 走 UAC 兜底，而不是硬失败。所以这条文案是协议的一部分，别随手改字。
const UNKNOWN_OP_PREFIX: &str = "不认识的操作";

/// 应答里的 `code`：**这个服务帮不了你，请换条路**（客户端据此回退 UAC）。
///
/// 两种都算「可用性问题」，绝不能当 `Failed`：
/// * `unknown_op`：服务是旧版本（升级后没重启），不认识新 op；
/// * `not_our_exe`：调用方不是安装目录里那份（便携副本）。若不回退，
///   「本机装了服务」反而会让便携版的每一个特权操作都报「拒绝执行」。
const CODE_UNKNOWN_OP: &str = "unknown_op";
const CODE_NOT_OUR_EXE: &str = "not_our_exe";

// ─────────────────────────────── 客户端 ───────────────────────────────

/// 服务侧的调用结果。三态是必须的：
/// - `Ok(body)`：做完了，`body` 就是要落盘的那段 JSON；
/// - `Unavailable`：服务没装 / 没起 / 版本不认（→ 调用方回退到 UAC）；
/// - `Failed`：服务明确回了失败（→ 不要再弹 UAC，那是同一个错误再报一次）。
pub(crate) enum ServiceCall {
    Ok(String),
    Unavailable(String),
    Failed(String),
}

/// 走服务执行一个 op。整通调用放在线程里 + `recv_timeout`：命名管道的同步读没有
/// 超时参数，而「服务卡住」必须能被感知（否则界面就一直转圈）。
pub(crate) fn call(op: &str, args: Value) -> ServiceCall {
    let request = json!({ "v": 1, "op": op, "args": args });
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(round_trip(&request));
    });
    match receiver.recv_timeout(std::time::Duration::from_millis(CLIENT_TIMEOUT_MS)) {
        Ok(result) => result,
        Err(_) => ServiceCall::Failed(format!(
            "特权服务 {} 秒没有应答（多半卡在一次读写上），已放弃等待",
            CLIENT_TIMEOUT_MS / 1000
        )),
    }
}

fn round_trip(request: &Value) -> ServiceCall {
    let name = wide(PIPE_NAME);
    let mut handle = INVALID_HANDLE_VALUE;
    for attempt in 0..2 {
        // SAFETY：name 是本函数栈上的宽串；其余参数按文档传空/零。
        handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            break;
        }
        let code = unsafe { GetLastError() };
        match code {
            // 服务没装 / 没起：这两种都是「换条路走」，不是错误
            ERR_FILE_NOT_FOUND | ERR_ACCESS_DENIED => {
                return ServiceCall::Unavailable(format!("特权服务不可用（{code}）"))
            }
            // 有名无空位（别的请求正在处理）：等一个实例，还不行就明确失败
            ERR_PIPE_BUSY if attempt == 0 => {
                unsafe { WaitNamedPipeW(name.as_ptr(), 5000) };
            }
            _ => return ServiceCall::Unavailable(format!("连不上特权服务（{code}）")),
        }
    }
    if handle == INVALID_HANDLE_VALUE {
        return ServiceCall::Failed("特权服务忙（5 秒内拿不到实例）".to_string());
    }
    let mut text = serde_json::to_string(request).unwrap_or_default();
    text.push('\n');
    let bytes = text.as_bytes();
    let mut written = 0usize;
    while written < bytes.len() {
        let mut count = 0u32;
        let ok = unsafe {
            WriteFile(
                handle,
                bytes[written..].as_ptr(),
                (bytes.len() - written) as u32,
                &mut count,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || count == 0 {
            let code = unsafe { GetLastError() };
            unsafe { CloseHandle(handle) };
            return ServiceCall::Failed(format!("发请求失败（{code}）"));
        }
        written += count as usize;
    }
    unsafe { FlushFileBuffers(handle) };

    let mut raw = Vec::new();
    let mut chunk = vec![0u8; 4096];
    loop {
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                handle,
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let code = unsafe { GetLastError() };
            unsafe { CloseHandle(handle) };
            return ServiceCall::Failed(format!("读应答失败（{code}）"));
        }
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read as usize]);
        if raw.contains(&b'\n') {
            break;
        }
        if raw.len() > MAX_REQUEST_BYTES {
            unsafe { CloseHandle(handle) };
            return ServiceCall::Failed("应答超过 1 MiB（协议错乱？）".to_string());
        }
    }
    unsafe { CloseHandle(handle) };

    let text = String::from_utf8_lossy(&raw);
    classify_reply(text.trim())
}

/// 把服务的应答文本分类成 [`ServiceCall`]（弄成纯函数是为了能单测 ——
/// 「旧服务不认识新 op」这条版本错配契约必须有测试钉住）。
fn classify_reply(text: &str) -> ServiceCall {
    let Ok(reply) = serde_json::from_str::<Value>(text) else {
        return ServiceCall::Failed("特权服务回了非 JSON 应答".to_string());
    };
    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
        return match reply.get("body").and_then(Value::as_str) {
            Some(body) => ServiceCall::Ok(body.to_string()),
            None => ServiceCall::Failed("特权服务应答缺少 body".to_string()),
        };
    }
    // 版本错配 / 调用方不是安装副本：都当成「不可用」而不是「失败」——
    // 否则新程序 + 旧服务、或便携版 + 已装服务，会表现成「操作直接报错且
    // 不弹授权」，用户完全没有出路。
    let code = reply.get("code").and_then(Value::as_str).unwrap_or("");
    let message = reply.get("error").and_then(Value::as_str).unwrap_or("");
    if code == CODE_UNKNOWN_OP || message.starts_with(UNKNOWN_OP_PREFIX) {
        return ServiceCall::Unavailable(format!("特权服务是旧版本（{message}），改用授权弹窗"));
    }
    if code == CODE_NOT_OUR_EXE {
        return ServiceCall::Unavailable(format!("{message}；改用授权弹窗"));
    }
    ServiceCall::Failed(if message.is_empty() {
        "特权服务没说原因".to_string()
    } else {
        message.to_string()
    })
}

// ─────────────────────────── 安装 / 卸载 / 状态 ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Missing,
    Stopped,
    Running,
    Unknown,
}

impl State {
    #[allow(dead_code)] // 给设置页/日志用，当前只有 --service-status 的机器可读输出
    pub(crate) fn label(self) -> &'static str {
        match self {
            State::Missing => "未安装",
            State::Stopped => "已安装未运行",
            State::Running => "运行中",
            State::Unknown => "状态未知",
        }
    }
}

/// 现在这份 exe 能不能用这台机器上的特权服务 —— 也就是「值不值得去问它」。
///
/// 两个条件缺一不可：
/// 1. 服务在跑（`state() == Running`）；
/// 2. **我们自己就是安装目录那份**。服务端只接受与它同路径的调用方
///    （[`caller_is_our_exe`]），便携副本 / 解压到别处的副本去问必定被拒。
///
/// 第 2 条是给「自动提权扫」用的：不问就不会留下一条 `not_our_exe` 的错误，
/// 便携版也不会每次开机都在界面上刷一行红字 —— 它本来就走 UAC 那条路。
pub(crate) fn usable_from_here() -> bool {
    state() == State::Running && is_installed_copy()
}

/// 安装目录固定 `%ProgramFiles%\AnyBoot`（NSIS 与 MSI 两边都钉死了这个路径，
/// 不是默认值碰巧一致 —— 服务以 SYSTEM 跑，安装目录必须不可被用户改写）。
fn installed_exe_path() -> Option<std::path::PathBuf> {
    std::env::var_os("ProgramFiles")
        .map(|root| std::path::PathBuf::from(root).join("AnyBoot").join("anyboot.exe"))
}

fn is_installed_copy() -> bool {
    let Ok(ours) = std::env::current_exe() else {
        return false;
    };
    let Some(expected) = installed_exe_path() else {
        return false;
    };
    same_path(
        &ours.to_string_lossy(),
        &expected.to_string_lossy(),
    )
}

pub(crate) fn state() -> State {
    let manager = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return State::Unknown;
    }
    let name = wide(SERVICE_NAME);
    let service = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS) };
    if service.is_null() {
        unsafe { CloseServiceHandle(manager) };
        return State::Missing;
    }
    let mut info: SERVICE_STATUS_PROCESS = unsafe { std::mem::zeroed() };
    let mut size = 0u32;
    let ok = unsafe {
        QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            &mut info as *mut _ as *mut u8,
            std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut size,
        )
    };
    unsafe {
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
    }
    if ok == 0 {
        return State::Unknown;
    }
    match info.dwCurrentState {
        SERVICE_RUNNING => State::Running,
        SERVICE_STOPPED => State::Stopped,
        _ => State::Unknown,
    }
}

/// 安装目录是否「管理员独占可写」。
///
/// 这是整套设计唯一的硬性前提：服务以 SYSTEM 运行，如果它指向的 exe 躺在用户
/// 可写的目录里（当前 NSIS 就是 `currentUser` 装到 `%LOCALAPPDATA%`），那任何
/// 本地用户改一下 exe 就白拿 SYSTEM —— 免费提权后门。宁可在安装时直接拒绝，
/// 也不要装出一个「能提权但更不安全」的东西。
fn install_dir_is_protected(exe: &std::path::Path) -> bool {
    let path = exe.to_string_lossy().to_lowercase();
    let suspicious: Vec<String> = ["LOCALAPPDATA", "APPDATA", "USERPROFILE", "TEMP", "TMP"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| value.to_lowercase())
        .filter(|value| !value.is_empty())
        .collect();
    if suspicious.iter().any(|root| path.starts_with(root.as_str())) {
        return false;
    }
    if path.contains("\\downloads\\") || path.contains("\\desktop\\") || path.contains("\\temp\\") {
        return false;
    }
    true
}

/// `--install-service`：注册并启动服务。**必须管理员**（SCM 会直接拒绝）。
///
/// 幂等：已经存在就更新 `binPath` 再启动，不报「已存在」——升级后 exe 路径可能
/// 变过（`currentUser` → `perMachine`），而任务/服务里存的是**绝对路径**，
/// 不更新就会指向一个不存在的文件（开机报 7000/7009，用户看不懂）。
pub(crate) fn install() -> Result<String, String> {
    let result = install_inner();
    log_installer_result("install", &result);
    result
}

fn install_inner() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|error| format!("拿不到自身路径：{error}"))?;
    if !install_dir_is_protected(&exe) {
        return Err(format!(
            "拒绝在用户可写目录注册特权服务：{}。\
             服务会以 SYSTEM 运行，而这个目录当前用户（以及任何同用户进程）都能改 —— \
             等于留下一个本地提权后门。安装包默认装到 %ProgramFiles%\\AnyBoot；\
             若你手工移动过程序，把 anyboot.exe 放回 Program Files 再执行本命令。",
            exe.display()
        ));
    }
    let manager = unsafe {
        OpenSCManagerW(
            std::ptr::null(),
            std::ptr::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
        )
    };
    if manager.is_null() {
        return Err(format!(
            "打不开服务管理器（{}）—— 要以管理员身份运行本命令",
            unsafe { GetLastError() }
        ));
    }
    let name = wide(SERVICE_NAME);
    let display = wide(SERVICE_DISPLAY);
    let bin = wide(&format!("\"{}\" --run-service", exe.display()));
    let mut service = unsafe {
        CreateServiceW(
            manager,
            name.as_ptr(),
            display.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            bin.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    let mut created = true;
    let mut recreate_code = 0u32;
    if service.is_null() {
        let code = unsafe { GetLastError() };
        if code != ERR_SERVICE_EXISTS {
            unsafe { CloseServiceHandle(manager) };
            return Err(install_error(code));
        }
        // 已存在：打开并改配置（升级路径）
        created = false;
        service = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_ALL_ACCESS) };
        if service.is_null() {
            unsafe { CloseServiceHandle(manager) };
            return Err(format!("服务已存在但打不开（{}）", unsafe { GetLastError() }));
        }
        // 改 binPath 要用 ChangeServiceConfig；这里直接删了重建更稳（配置项本就少，
        // 而且重建能顺带把 binPath 里旧路径彻底换掉）。删除会失败于「正在运行」，
        // 所以先停。
        stop_service(service);
        unsafe { DeleteService(service) };
        unsafe { CloseServiceHandle(service) };
        service = recreate_service(manager, name.as_ptr(), display.as_ptr(), bin.as_ptr(), &mut recreate_code);
        if service.is_null() {
            unsafe { CloseServiceHandle(manager) };
            return Err(install_error(recreate_code));
        }
    }

    // 描述 + 失败重启：服务是常驻进程，一次 panic 就永久失去提权能力是最难
    // 排查的失败形态（用户只看到「点没反应」）。三次重启够用了。
    let description = wide(SERVICE_DESC);
    let desc = SERVICE_DESCRIPTIONW {
        lpDescription: description.as_ptr() as *mut u16,
    };
    unsafe {
        ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_DESCRIPTION,
            &desc as *const _ as *const std::ffi::c_void,
        );
    }
    let mut actions = vec![
        SC_ACTION { Type: SC_ACTION_RESTART, Delay: 5_000 },
        SC_ACTION { Type: SC_ACTION_RESTART, Delay: 10_000 },
        SC_ACTION { Type: SC_ACTION_RESTART, Delay: 30_000 },
    ];
    let failure = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 3600,
        lpRebootMsg: std::ptr::null_mut(),
        lpCommand: std::ptr::null_mut(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    unsafe {
        ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            &failure as *const _ as *const std::ffi::c_void,
        );
    }

    let started = unsafe { StartServiceW(service, 0, std::ptr::null()) };
    let start_code = unsafe { GetLastError() };
    unsafe {
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
    }
    if started == 0 && start_code != ERR_SERVICE_ALREADY_RUNNING {
        return Err(format!(
            "服务已注册但启动失败（{start_code}）。看 %ProgramData%\\AnyBoot\\service.log，\
             或 eventvwr 里的「服务控制管理器」日志"
        ));
    }
    Ok(format!(
        "{}：{} → {}",
        if created { "已注册特权服务" } else { "已更新特权服务" },
        SERVICE_NAME,
        exe.display()
    ))
}

fn install_error(code: u32) -> String {
    match code {
        ERR_ACCESS_DENIED => "拒绝访问：要以管理员身份运行 --install-service".to_string(),
        ERR_SERVICE_MARKED_FOR_DELETE => {
            "服务已标记删除但还没被 SCM 真正移除（刚才的卸载/升级还没走完）。\
             过几秒重跑一次 --install-service，或用重启系统收尾"
                .to_string()
        }
        _ => format!("注册服务失败（{code}）"),
    }
}

/// 重建服务（升级路径）：`DeleteService` 是**异步**的 —— SCM 要等所有句柄关闭才
/// 真正移除对象，紧接着 `CreateService` 会吃到 1072（已标记删除）。
///
/// 这段重试不是「保险起见」：升级时安装器必须重建服务才能换掉 binPath 里旧的
/// 绝对路径，而安装器那边是 `Return="ignore"`（装不上不阻断安装）。一次 1072
/// 就等于「升级完免 UAC 静默失效，日志里只有一个错误码」—— 真机上最难查的那类。
fn recreate_service(
    manager: HANDLE,
    name: *const u16,
    display: *const u16,
    bin: *const u16,
    last_code: &mut u32,
) -> HANDLE {
    for attempt in 0..20 {
        let handle = unsafe {
            CreateServiceW(
                manager,
                name,
                display,
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                bin,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if !handle.is_null() {
            return handle;
        }
        *last_code = unsafe { GetLastError() };
        if *last_code != ERR_SERVICE_MARKED_FOR_DELETE || attempt == 19 {
            return std::ptr::null_mut();
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    std::ptr::null_mut()
}

fn stop_service(service: HANDLE) -> bool {
    let mut status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: SERVICE_RUNNING,
        dwControlsAccepted: 0,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: 0,
    };
    unsafe { ControlService(service, SERVICE_CONTROL_STOP, &mut status) };
    // 等到真停下：exe 还被占用时删文件/删服务都会失败，这一步不能省。
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let mut info: SERVICE_STATUS_PROCESS = unsafe { std::mem::zeroed() };
        let mut size = 0u32;
        let ok = unsafe {
            QueryServiceStatusEx(
                service,
                SC_STATUS_PROCESS_INFO,
                &mut info as *mut _ as *mut u8,
                std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
                &mut size,
            )
        };
        if ok == 0 || info.dwCurrentState == SERVICE_STOPPED {
            return true;
        }
    }
    false
}

/// `--uninstall-service`：停 → 删。**顺序不能反**：服务在跑时 exe 被锁，
/// 先删文件会失败；而只 `DeleteService` 不停止，服务会一直挂到进程退出。
pub(crate) fn uninstall() -> Result<String, String> {
    let result = uninstall_inner();
    log_installer_result("uninstall", &result);
    result
}

fn uninstall_inner() -> Result<String, String> {
    let manager = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(format!("打不开服务管理器（{}）", unsafe { GetLastError() }));
    }
    let name = wide(SERVICE_NAME);
    let service = unsafe {
        OpenServiceW(
            manager,
            name.as_ptr(),
            SERVICE_STOP | SERVICE_QUERY_STATUS | SERVICE_DELETE,
        )
    };
    if service.is_null() {
        let code = unsafe { GetLastError() };
        unsafe { CloseServiceHandle(manager) };
        return if code == ERR_SERVICE_DOES_NOT_EXIST {
            Ok("特权服务本来就没装".to_string())
        } else {
            Err(format!("打不开服务（{code}）：要以管理员身份运行 --uninstall-service"))
        };
    }
    stop_service(service);
    let deleted = unsafe { DeleteService(service) };
    let code = unsafe { GetLastError() };
    unsafe {
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
    }
    if deleted == 0 && code != ERR_SERVICE_DOES_NOT_EXIST {
        return Err(format!("删除服务失败（{code}）"));
    }
    Ok("已停止并删除特权服务".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个新 op 的**参数校验都必须在碰硬件之前**发生：这些用例在开发机
    /// （没有固件写权限、没有 ESP）上跑得通，正是因为非法输入先被拒了。
    /// 反过来说，哪天真让 `nvram_boot_next` 拿坏参数去写 NVRAM，这几条会
    /// 直接红掉 —— 那正是我们想要的报警方式。
    #[test]
    fn dispatch_rejects_bad_arguments_before_touching_hardware() {
        // 通道名是枚举，不认「别的引导器」
        let error = dispatch(&json!({ "op": "nvram_boot_next", "args": { "channel": "grub" } }))
            .unwrap_err();
        assert!(error.contains("不认识的通道"), "{error}");
        // 注册方式同理
        let error = dispatch(&json!({ "op": "nvram_register_boot", "args": { "mode": "always" } }))
            .unwrap_err();
        assert!(error.contains("不认识的注册方式"), "{error}");
        // one-shot 的条目 id 不能为空
        let error =
            dispatch(&json!({ "op": "sdboot_one_shot", "args": { "entry": "  " } })).unwrap_err();
        assert!(error.contains("缺少条目 id"), "{error}");
        // 盘符分配的磁盘号必须是 JSON 数字：「2」这种字符串不接受
        let error = dispatch(&json!({
            "op": "esp_assign_letter",
            "args": { "disk": "2", "partition": 1 }
        }))
        .unwrap_err();
        assert!(error.contains("缺少数字参数 disk"), "{error}");
        let error =
            dispatch(&json!({ "op": "esp_assign_letter", "args": { "disk": 2 } })).unwrap_err();
        assert!(error.contains("缺少数字参数 partition"), "{error}");
    }

    /// 升级期的版本错配契约：旧服务回「不认识的操作」时，客户端必须判成
    /// **可用性问题**（→ 回退 UAC），而不是「操作失败」（→ 直接报错、不给退路）。
    /// 这条一旦写错，用户装了新版本却什么特权操作都做不了，还看不到原因。
    #[test]
    fn unknown_op_from_old_service_degrades_to_unavailable() {
        let old_service = r#"{"ok":false,"error":"不认识的操作 nvram_boot_next（白名单只有 5 个）"}"#;
        match classify_reply(old_service) {
            ServiceCall::Unavailable(why) => assert!(why.contains("旧版本"), "{why}"),
            ServiceCall::Failed(why) => panic!("旧服务被当成硬失败：{why}"),
            ServiceCall::Ok(_) => panic!("不该成功"),
        }
        // 真正的业务失败仍然是 Failed（不能顺手把它也降级成「换条路」，
        // 否则同一个错误会被弹两次授权再报一次）
        match classify_reply(r#"{"ok":false,"error":"固件拒绝写入"}"#) {
            ServiceCall::Failed(why) => assert_eq!(why, "固件拒绝写入"),
            _ => panic!("业务失败被误判成可用性问题"),
        }
        match classify_reply(r#"{"ok":true,"body":"{\"ok\":true}"}"#) {
            ServiceCall::Ok(body) => assert!(body.contains("\"ok\":true")),
            _ => panic!("正常应答没解出来"),
        }
        // 便携副本调用已安装的服务：必须回退 UAC，不能硬失败
        match classify_reply(
            r#"{"ok":false,"code":"not_our_exe","error":"调用方不是本程序的安装副本"}"#,
        ) {
            ServiceCall::Unavailable(why) => assert!(why.contains("改用授权弹窗"), "{why}"),
            _ => panic!("便携副本被服务拒了却没有退路"),
        }
    }

    /// 白名单之外的操作必须被拒 —— 这是「不给 SYSTEM shell」的唯一闸门。
    #[test]
    fn dispatch_rejects_unknown_ops() {
        let request = json!({ "op": "cmd", "args": { "line": "whoami" } });
        assert!(dispatch(&request).is_err());
        assert!(dispatch(&json!({ "args": {} })).is_err());
    }

    /// grubenv 之外的文件一律不许写。
    #[test]
    fn dispatch_rejects_arbitrary_paths() {
        let request = json!({
            "op": "linux_grub_write",
            "args": { "dev": "disk0p3", "rel": "root/.ssh/authorized_keys", "payload_hex": "00" }
        });
        let error = dispatch(&request).unwrap_err();
        assert!(error.contains("只允许 grubenv"), "{error}");
    }

    #[test]
    fn same_path_is_case_insensitive() {
        assert!(same_path(r"C:\Program Files\AnyBoot\anyboot.exe", r"c:\program files\anyboot\anyboot.exe"));
        assert!(!same_path(r"C:\a\anyboot.exe", r"C:\b\anyboot.exe"));
        assert!(!same_path("", r"C:\b\anyboot.exe"));
    }

    /// 这套机制的硬前提：服务指向的 exe 不能被普通用户改写。
    /// 用户目录里的路径必须被拒 —— 否则「装一次服务」就等于留下一个本地提权后门。
    #[test]
    fn rejects_user_writable_install_dirs() {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
        if !local.is_empty() {
            let under_local = std::path::PathBuf::from(&local).join("AnyBoot").join("anyboot.exe");
            assert!(!install_dir_is_protected(&under_local), "{}", under_local.display());
        }
        let program_files = std::path::PathBuf::from(r"C:\Program Files\AnyBoot\anyboot.exe");
        assert!(install_dir_is_protected(&program_files));
    }
}
