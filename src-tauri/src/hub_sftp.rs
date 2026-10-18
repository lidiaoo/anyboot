//! server-hub M5：SFTP 文件浏览 / 传输（独立窗 `server-files`）的后端。
//!
//! 哲学与终端/隧道同源：**不引 SSH 库，直接复用系统 `sftp` CLI**。
//! - 认证：统一走 `hub_tunnel::apply_auth` —— 密钥 `-i`，或存档密码经
//!   `SSH_ASKPASS`（指向本程序自身）免输；Unix 额外用 `ControlPath` 复用
//!   终端建立的 ControlMaster 连接（master 不在时 `ensure_control` 先拨通一次），
//!   Windows 的 Win32-OpenSSH 不支持多路复用，改成每次自己认证。
//!   没存凭据（只在终端里手输过密码）无法非交互认证，维持明确报错。
//! - 批命令走 `sftp -b -`（stdin），出错即中止 —— stderr 尾巴就是给用户的
//!   错误信息。
//! - 列表解析 `ls -la` 输出（LC_ALL=C.UTF-8：月份名仍是英文、且 OpenSSH 客户端不会把非 ASCII 文件名转义成 \303\251 八进制），日期归一成
//!   `MM-DD HH:MM` / `YYYY-MM-DD`。
//! - 传输（下载/上传）支持**暂停 / 继续 / 放弃（断点续传）**：暂停=杀掉
//!   sftp 子进程留下半截文件并记入断点清单（transfer-queue.json，落配置
//!   目录、跨重启存活）；继续=新起子进程用 `reget`/`reput` 从本地/远端已传
//!   字节接着传（sftp -a 语义，见 OpenSSH 手册），续传前校验远端/源文件
//!   尺寸没变（变了就拒绝续传防出损坏文件）；放弃=清半截文件+删记录。
//!   网络中断、窗口重开（自动重挂）、应用重启后（传输页手动续传）都走
//!   同一条续传路径 —— 与手机端 TransferManager 的暂停/续传/放弃三键对齐。

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::hub::HubServer;

/// 进度事件（下载/上传推给 server-files 窗口）
const EVENT_PROGRESS: &str = "hub-sftp-progress";

/// 断点清单文件名（配置目录下，与 hub-servers.json 同级）
const TRANSFER_QUEUE_FILE: &str = "transfer-queue.json";

// ==================== 数据 ====================

/// 一条还没传完的传输记录（暂停/中断后留在断点清单里，等继续/放弃）。
/// 前端展示与 `hub_sftp_transfer_list` 返回、transfer-queue.json 落盘共用。
#[derive(Clone, Serialize, Deserialize)]
pub struct XferTask {
    /// 稳定键：dir|remote_path|local_path（续传/放弃都按它定位）
    pub key: String,
    /// "down" | "up"
    pub dir: String,
    /// 显示名（文件名）
    pub name: String,
    pub remote_path: String,
    pub local_path: String,
    pub total: u64,
    /// 已传字节：down=本地半截文件大小、up=远端半截文件大小
    pub bytes: u64,
    /// 服务器档案 id（续传时按它重新加载档案）
    pub server_id: String,
    /// 服务器显示名（前端展示用）
    pub server_name: String,
    /// 是否处于暂停/中断态（清单接口输出用；落盘的都是 true）
    #[serde(default)]
    pub paused: bool,
    /// 任务状态：running / paused / done / failed / abandoned。
    /// 传输页与文件窗都按它渲染 —— 之前只有 paused/done/failed 三个布尔，
    /// 「放弃」根本没有事件，两端只能靠 3s 轮询猜，这就是状态不同步的根。
    #[serde(default = "state_running")]
    pub state: String,
    /// 这条任务最初是从哪个窗口发起的（续传时沿用它去发事件、关窗时按它收进程）。
    /// 旧版落盘文件里没有这个字段 → 空串，读回时回退到文件窗。
    #[serde(default)]
    pub window: String,
    /// 最后活动时间（毫秒）
    pub at: u64,
}

fn state_running() -> String {
    "running".to_string()
}

#[derive(Clone, Serialize)]
pub struct SftpEntry {
    pub name: String,
    pub dir: bool,
    pub link: bool,
    /// 软链接指向（`ls` 行里 `名字 -> 指向` 的后半段；非软链接为 None）
    pub target: Option<String>,
    pub size: u64,
    /// 展示用："10-16 09:12" / "2025-03-01"
    pub mtime: String,
    pub perms: String,
}

#[derive(Default)]
pub struct HubSftpState {
    /// hub「浏览文件」排队的服务器（窗口首次加载时取走）
    pending: Mutex<Option<HubServer>>,
    /// 还在跑的传输子进程（窗口销毁时按窗口收掉）。Arc 共享给进度轮询线程。
    running: Mutex<Vec<RunningXfer>>,
    /// 暂停/中断的传输（断点续传清单）。窗口重开自动重挂、应用重启后落盘。
    paused: Mutex<Vec<XferTask>>,
    /// 最近的终态任务（完成/失败/放弃）。传输页可能比任务晚打开（事件早就发过了），
    /// 只靠 running+paused 快照会让「刚才那个下载」凭空消失 —— 这份短期历史
    /// 让清单接口能补齐终态，双向对账才完整。
    recent: Mutex<VecDeque<XferTask>>,
}

/// 终态历史容量（够覆盖传输页后开的场景，又不至于让清单膨胀）
const RECENT_LIMIT: usize = 30;

// ==================== 断点清单（落盘） ====================

/// 断点清单路径（配置目录下 transfer-queue.json）
fn transfer_queue_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("无法定位应用配置目录: {error}"))?;
    Ok(dir.join(TRANSFER_QUEUE_FILE))
}

/// 断点清单落盘（临时文件 + 改名，与 hub-servers.json 同款防半截写）
fn save_queue(path: &std::path::Path, tasks: &[XferTask]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录: {error}"))?;
    }
    let temporary = path.with_extension("json.tmp");
    let content =
        serde_json::to_string_pretty(tasks).map_err(|error| format!("序列化失败: {error}"))?;
    std::fs::write(&temporary, content.as_bytes())
        .map_err(|error| format!("无法写入 {temporary:?}: {error}"))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("无法落盘 {path:?}: {error}"))?;
    Ok(())
}

/// 断点清单读回（不存在 = 空；坏 JSON 按空处理，别让一份坏文件卡死传输页）
fn load_queue(path: &std::path::Path) -> Vec<XferTask> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&content).unwrap_or_else(|error| {
        eprintln!("transfer-queue.json 解析失败（按空表处理）: {error}");
        Vec::new()
    })
}

/// 把当前断点清单写回磁盘（内存为准，全量覆盖）
fn persist_queue(state: &HubSftpState, app: &AppHandle) {
    let tasks = state
        .paused
        .lock()
        .map(|paused| paused.clone())
        .unwrap_or_default();
    if let Ok(path) = transfer_queue_path(app) {
        if let Err(error) = save_queue(&path, &tasks) {
            eprintln!("断点清单落盘失败: {error}");
        }
    }
}

/// 应用启动时把磁盘清单读回内存（setup 里调用）
pub fn load_queue_at_startup(app: &AppHandle) {
    if let Some(state) = app.try_state::<HubSftpState>() {
        if let Ok(path) = transfer_queue_path(app) {
            let tasks = load_queue(&path);
            if let Ok(mut paused) = state.paused.lock() {
                *paused = tasks;
            }
        }
    }
}

/// 生成传输记录的稳定键
fn xfer_key(dir: &str, remote_path: &str, local_path: &str) -> String {
    format!("{dir}|{remote_path}|{local_path}")
}

/// 现在的毫秒时间戳
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 半截文件大小：down 看本地、up 看远端（远端由调用方探好传入）
fn partial_size(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// 任务当前已传字节：down=本地半截文件（实时、准），up=轮询线程最后一次
/// 摸到的远端尺寸（可能有 ~1.5s 滞后；本地路径是源文件，不能拿它当进度）。
fn xfer_bytes_now(task: &XferTask) -> u64 {
    if task.dir == "down" {
        partial_size(&task.local_path)
    } else {
        task.bytes
    }
}

/// 传输是否已经传完：total 已知且已传 ≥ total。
/// 杀进程/断线都可能发生在"最后字节已落盘、sftp 还没来得及干净退出"的窗口
/// 里 —— 那一刻文件是完整的，绝不能记成断点（会卡出「100% 已暂停」的死卡）。
fn xfer_is_complete(task: &XferTask) -> bool {
    task.total > 0 && xfer_bytes_now(task) >= task.total
}

/// upsert 一条断点记录（同键更新，否则追加）并落盘。落盘的记录都是暂停态。
fn upsert_paused(state: &HubSftpState, app: &AppHandle, mut task: XferTask) {
    task.paused = true;
    if let Ok(mut paused) = state.paused.lock() {
        match paused.iter().position(|t| t.key == task.key) {
            Some(idx) => paused[idx] = task,
            None => paused.push(task),
        }
    }
    persist_queue(state, app);
}

/// 从断点清单里摘掉一条（放弃/完成时）并落盘
fn remove_paused(state: &HubSftpState, app: &AppHandle, key: &str) -> Option<XferTask> {
    let removed = state
        .paused
        .lock()
        .ok()
        .and_then(|mut paused| {
            let idx = paused.iter().position(|t| t.key == key)?;
            Some(paused.remove(idx))
        });
    if removed.is_some() {
        persist_queue(state, app);
    }
    removed
}

// ==================== sftp 批命令底座 ====================

fn sftp_quote(path: &str) -> String {
    format!(
        "\"{}\"",
        path.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// 远端 shell 引用（ssh 命令串走远端 sh）：单引号包裹，内部 `'` 转义为 `'\''`。
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

pub(crate) fn stderr_tail(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .unwrap_or("")
        .trim()
        .to_string()
}

// ==================== 认证 ====================

/// 文件浏览 / 传输共用的一句「没凭据」提示（apply_auth 原样抛给前端）
const NO_CRED_MESSAGE: &str =
    "这台服务器还没有保存凭据，文件功能无法自动认证：请在「编辑」里改用密钥或保存密码";

/// 跑一批 sftp 命令（-b -），返回 stdout。批里任何一条失败都会整体失败。
fn run_sftp_batch(app: &AppHandle, server: &HubServer, commands: &str) -> Result<String, String> {
    if server.id.is_empty() {
        return Err("该服务器还没保存档案，无法自动认证".to_string());
    }
    let target = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };
    let mut command = crate::command("sftp");
    // 认证参数必须排在 target 之前（sftp 把 target 之后的一切当远端路径）
    crate::hub_tunnel::apply_auth(app, server, &mut command, NO_CRED_MESSAGE)?;
    command
        .arg("-P")
        .arg(server.effective_port().to_string())
        .arg("-b")
        .arg("-")
        .arg(&target);
    command
        .env("LC_ALL", "C.UTF-8")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 sftp: {error}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("拿不到 sftp stdin")?;
        stdin
            .write_all(commands.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("写 sftp 批命令失败: {error}"))?;
    } // stdin 落下后 sftp 才开始逐条执行
    let output = child
        .wait_with_output()
        .map_err(|error| format!("等待 sftp 失败: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = stderr_tail(&output.stderr);
        let stdout_tail = stderr_tail(&output.stdout);
        Err(if !stderr.is_empty() {
            stderr
        } else if !stdout_tail.is_empty() {
            stdout_tail
        } else {
            format!("sftp 失败（退出码 {:?}）", output.status.code())
        })
    }
}

/// 还在跑的传输子进程登记项：窗口、任务信息（暂停时要用）、子进程把。
/// 任务快照在 spawn 时拷好 —— pause 拿它直接入断点清单，不必反查。
#[derive(Clone)]
struct RunningXfer {
    window: String,
    task: XferTask,
    child: Arc<Mutex<std::process::Child>>,
}

/// 起一个不等待的 sftp 传输子进程（下载/上传）：登记进 running 表（按窗口），
/// 返回 Arc 把手给进度轮询线程。
fn spawn_sftp_transfer(
    app: &AppHandle,
    state: &HubSftpState,
    window: &str,
    server: &HubServer,
    commands: &str,
    task: XferTask,
) -> Result<Arc<Mutex<std::process::Child>>, String> {
    let target = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };
    let mut command = crate::command("sftp");
    // 认证参数必须排在 target 之前（sftp 把 target 之后的一切当远端路径）
    crate::hub_tunnel::apply_auth(app, server, &mut command, NO_CRED_MESSAGE)?;
    command
        .arg("-P")
        .arg(server.effective_port().to_string())
        .arg("-b")
        .arg("-")
        .arg(&target);
    command
        .env("LC_ALL", "C.UTF-8")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 sftp: {error}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("拿不到 sftp stdin")?;
        stdin
            .write_all(commands.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("写 sftp 批命令失败: {error}"))?;
    }
    let handle = Arc::new(Mutex::new(child));
    let mut started = task;
    started.state = "running".to_string();
    started.paused = false;
    started.window = window.to_string();
    state
        .running
        .lock()
        .map_err(|_| "传输进程表已被锁死")?
        .push(RunningXfer {
            window: window.to_string(),
            task: started.clone(),
            child: handle.clone(),
        });
    // 入表即广播「开跑」。下载过去完全没有初始事件，传输列表要等第一次
    // 字节变化才出现卡片 —— 小文件/秒传甚至一条都不出，只剩最后的 done。
    emit_xfer(app, &started, "running", started.bytes, started.total);
    reap_finished(state);
    Ok(handle)
}

/// 把已经退出的传输子进程从 running 表摘掉（防止长会话里越积越多）
fn reap_finished(state: &HubSftpState) {
    if let Ok(mut list) = state.running.lock() {
        list.retain(|running| {
            running
                .child
                .lock()
                .map(|mut child| child.try_wait().map_or(true, |s| s.is_none()))
                .unwrap_or(true)
        });
    }
}

// ==================== 列表解析 ====================

const MONTHS: [(&str, &str); 12] = [
    ("Jan", "01"), ("Feb", "02"), ("Mar", "03"), ("Apr", "04"),
    ("May", "05"), ("Jun", "06"), ("Jul", "07"), ("Aug", "08"),
    ("Sep", "09"), ("Oct", "10"), ("Nov", "11"), ("Dec", "12"),
];

fn month_num(name: &str) -> Option<&'static str> {
    MONTHS.iter().find(|(m, _)| *m == name).map(|(_, n)| *n)
}

/// 把 ls -la 的日期段归一：`Oct 16 09:12` → `10-16 09:12`；`Oct 16 2025` → `2025-10-16`
fn normalize_mtime(mon: &str, day: &str, tail: &str) -> String {
    let month = month_num(mon).unwrap_or("??");
    let day = format!("{:0>2}", day);
    if tail.contains(':') {
        format!("{month}-{day} {tail}")
    } else {
        format!("{tail}-{month}-{day}")
    }
}

/// 解析一行 `ls -la`。解析不了的行（错误提示等）返回 None。
fn parse_ls_line(line: &str) -> Option<SftpEntry> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 9 {
        return None;
    }
    let perms = tokens[0].to_string();
    if perms.len() < 10 {
        return None;
    }
    let size: u64 = tokens[4].parse().ok()?;
    // 日期段两种流派都认：
    //   GNU ls       ：Oct 17 05:47（月 日 时/年）
    //   paramiko 等  ：17 Oct 01:46（日 月 时/年）
    let (mon, day, tail) = if month_num(tokens[5]).is_some() {
        (tokens[5], tokens[6], tokens[7])
    } else if month_num(tokens[6]).is_some() {
        (tokens[6], tokens[5], tokens[7])
    } else {
        return None;
    };
    let day = day.to_string();
    let tail = tail.to_string();
    // 名字 = 第 9 个字段起直到行尾（文件名可带空格；软链接的 " -> target" 保留可读）。
    // 手工推进字节偏移找第 9 个字段的起点，不依赖 SplitWhitespace::as_str。
    let bytes = line.as_bytes();
    let mut offset = 0usize;
    for _ in 0..8 {
        while offset < bytes.len() && bytes[offset].is_ascii_whitespace() {
            offset += 1;
        }
        while offset < bytes.len() && !bytes[offset].is_ascii_whitespace() {
            offset += 1;
        }
    }
    while offset < bytes.len() && bytes[offset].is_ascii_whitespace() {
        offset += 1;
    }
    let name = line[offset..].trim().to_string();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let link = perms.starts_with('l');
    // 软链接行是 "名字 -> 指向"：拆开，名字保持纯路径（否则 rename/rm/download
    // 拼出 "dir/foo -> /target" 必错）；指向单独带给前端详情。
    let (name, target) = if link {
        match name.find(" -> ") {
            Some(idx) => (name[..idx].to_string(), Some(name[idx + 4..].to_string())),
            None => (name, None),
        }
    } else {
        (name, None)
    };
    if name.is_empty() {
        return None;
    }
    Some(SftpEntry {
        name,
        dir: perms.starts_with('d'),
        link,
        target,
        size,
        mtime: normalize_mtime(mon, &day, &tail),
        perms,
    })
}

/// OpenSSH sftp 客户端对 `ls -la <绝对路径>` 的长格式输出自带完整路径前缀
/// （`ls -la /root` → `/root/.cache`、`/root/.`、`/root/..`）。
/// 剥掉查询路径前缀还原裸名字；`.` / `..` 行返回 None（前端有自己的面包屑/上一级）。
/// 服务器本来就输出裸名字时（strip_prefix 不中）原样保留。
fn bare_name(name: &str, path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let prefix = if trimmed.is_empty() { "/" } else { trimmed };
    let rest = match name.strip_prefix(prefix) {
        Some(rest) => rest.trim_start_matches('/'),
        None => name,
    };
    // 个别服务器目录名带尾斜杠（手机端 SftpOps 已实机踩过），剥掉防排序/拼路径带错
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() || rest == "." || rest == ".." {
        return None;
    }
    Some(rest.to_string())
}

// ==================== 命令 ====================

/// 传输状态事件：把一条任务的当前状态**广播**给所有窗口。
///
/// 为什么不用 `emit_to(files_label)`：传输页开在 hub 主窗里，主窗没打开时
/// emit_to 直接失败（返回值过去被丢弃），用户之后打开传输页就什么都看不到 ——
/// 这是「下载进度没同步到传输列表」的一半原因。广播是谁在听谁收；
/// 另一半由 `recent` 快照补齐（见 `hub_sftp_transfer_list`），事件丢了也能对账。
fn emit_xfer(app: &AppHandle, task: &XferTask, state: &str, bytes: u64, total: u64) {
    let payload = json!({
        "key": task.key,
        "name": task.name,
        "dir": task.dir,
        "state": state,
        "bytes": bytes,
        "total": total,
        "server_id": task.server_id,
        "server_name": task.server_name,
        // 旧布尔字段继续带上：文件窗/主窗里还有按它们分支的老逻辑
        "paused": state == "paused",
        "done": state == "done",
        "failed": state == "failed",
        "abandoned": state == "abandoned",
    });
    let _ = app.emit(EVENT_PROGRESS, payload);
}

/// 记一条终态进短期历史（同 key 去重、最新的在前）。
/// 传输页后开时靠它补上「刚才完成/放弃的那条」。
fn push_recent(state: &HubSftpState, task: &XferTask, status: &str) {
    let mut entry = task.clone();
    entry.state = status.to_string();
    entry.paused = false;
    entry.at = now_ms();
    if let Ok(mut recent) = state.recent.lock() {
        recent.retain(|t| t.key != entry.key);
        recent.push_front(entry);
        while recent.len() > RECENT_LIMIT {
            recent.pop_back();
        }
    }
}

/// 传输控制：pause=杀掉 sftp 子进程留下半截文件记入断点清单（真正可续传的
/// 暂停）、resume=按断点用 `reget`/`reput` 从已传字节接着传、abandon=删半截
/// 文件+删记录（放弃）、cancel 兼容旧前端=等价 abandon。返回作用的任务数。
///
/// （旧实现是 SIGSTOP/SIGCONT 冻结进程：Windows 没有 POSIX 信号直接编译不过，
/// 且应用重启后进程没了传输就永远卡死 —— 换成「杀进程+reget/reput」的
/// 断点续传语义后三端一致、跨重启也成立。）
/// **必须 async + spawn_blocking**：abandon 要 kill 子进程再等它退出
/// （`child.wait()`），同步跑在 UI 线程上就是「点放弃 → 界面僵住」。
#[tauri::command]
pub async fn hub_sftp_transfer_ctl(
    app: AppHandle,
    action: String,
    key: Option<String>,
) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        hub_sftp_transfer_ctl_blocking(app, action, key)
    })
    .await
    .map_err(|error| format!("传输控制任务失败: {error}"))?
}

fn hub_sftp_transfer_ctl_blocking(
    app: AppHandle,
    action: String,
    key: Option<String>,
) -> Result<usize, String> {
    let state = app.state::<HubSftpState>();
    match action.as_str() {
        "pause" => pause_transfers(&app, &state, key.as_deref()),
        "resume" => resume_transfer(&app, &state, key.as_deref()),
        "abandon" | "cancel" => abandon_transfer(&app, &state, key.as_deref()),
        other => Err(format!("未知传输控制动作: {other}")),
    }
}

/// 传输页实时清单：跑着的 + 断点清单 + 最近终态（完成/失败/放弃）。
///
/// 前端把它当**对账快照**用：事件是主路径，事件丢了（传输页后开、主窗没开时
/// 广播没人收）就靠这份快照把卡片补回来或清掉。所以三类都必须给：
/// 只给 running+paused 时，「刚才下载完的」在传输页里根本不存在，
/// 「刚才放弃的」卡片也永远消不掉。
#[tauri::command]
pub fn hub_sftp_transfer_list(
    state: tauri::State<'_, HubSftpState>,
) -> Result<Vec<XferTask>, String> {
    let mut tasks: Vec<XferTask> = Vec::new();
    if let Ok(list) = state.running.lock() {
        for running in list.iter() {
            let mut task = running.task.clone();
            // 进度以最新事实为准（down 看本地半截文件；up 保持轮询值，
            // 本地路径是源文件不能当进度）；已传满的按完成口径报。
            task.bytes = xfer_bytes_now(&task);
            if xfer_is_complete(&task) {
                task.bytes = task.total;
            }
            task.paused = false;
            task.state = "running".to_string();
            tasks.push(task);
        }
    }
    if let Ok(paused) = state.paused.lock() {
        tasks.extend(paused.iter().map(|t| {
            let mut task = t.clone();
            task.paused = true;
            task.state = "paused".to_string();
            task
        }));
    }
    // 终态补录：同 key 已经在 running/paused 里的跳过（续传中的任务不该
    // 被上一次的 done/abandoned 顶掉）
    if let Ok(recent) = state.recent.lock() {
        let live: std::collections::HashSet<String> =
            tasks.iter().map(|t| t.key.clone()).collect();
        tasks.extend(
            recent
                .iter()
                .filter(|t| !live.contains(&t.key))
                .cloned(),
        );
    }
    Ok(tasks)
}

/// 暂停：杀掉 sftp 子进程（本地留半截文件）、任务入断点清单。
/// key=None 时作用于全部跑着的下载/上传（「全部暂停」按钮）。
/// 收尾事件的**所有权**归摘除方：从 running 表摘走后由这里直接发 paused/done
/// 事件，轮询线程发现自己已不持有该条目时保持沉默（见 finish_transfer）。
fn pause_transfers(
    app: &AppHandle,
    state: &HubSftpState,
    key: Option<&str>,
) -> Result<usize, String> {
    // 先摘出来再杀：摘出去的就不在表里了，轮询线程对它们只会看到进程退出。
    let taken: Vec<RunningXfer> = match state.running.lock() {
        Ok(mut list) => {
            let (taken, kept): (Vec<_>, Vec<_>) = list
                .drain(..)
                .partition(|running| key.map_or(true, |k| running.task.key == k));
            *list = kept;
            taken
        }
        Err(_) => return Err("传输进程表已被锁死".to_string()),
    };
    let mut acted = 0usize;
    for running in taken {
        // 杀子进程：sftp 已写的字节留在半截文件里，这就是续传的断点
        if let Ok(mut child) = running.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let mut task = running.task;
        task.bytes = xfer_bytes_now(&task);
        task.at = now_ms();
        if xfer_is_complete(&task) {
            // 竞态：点暂停那一刻其实已经传完了（最后字节已落盘、sftp 还没
            // 干净退出）。文件是完整的 → 直接按完成收尾，绝不记断点，
            // 否则传输页会卡出「100% 已暂停」永远按不出结果。
            let total = task.total;
            push_recent(state, &task, "done");
            emit_xfer(app, &task, "done", total, total);
        } else {
            task.state = "paused".to_string();
            // 记住来源窗口：续传时按它发事件、关窗时按它收进程
            task.window = running.window.clone();
            upsert_paused(state, app, task.clone());
            let (bytes, total) = (task.bytes, task.total);
            emit_xfer(app, &task, "paused", bytes, total);
        }
        acted += 1;
    }
    Ok(acted)
}

/// 继续：按断点记录重起 sftp 子进程。down 用 `reget`（从本地半截文件大小
/// 接着拉）、up 用 `reput`（往远端半截文件接着推）。远端文件尺寸变了就
/// 拒绝续传（内容对不上传出来是损坏文件）。
///
/// `key=None`（「全部继续」）时把断点清单里的**每一条**都续起来 —— 旧实现
/// 走 `pick_paused(None)` 只取最近一条，于是「全部继续」名义上成功、实际
/// 只续了一个，其余卡片留在原地不动。
fn resume_transfer(
    app: &AppHandle,
    state: &HubSftpState,
    key: Option<&str>,
) -> Result<usize, String> {
    let targets: Vec<XferTask> = match key {
        Some(k) => pick_paused(state, Some(k)).into_iter().collect(),
        None => state.paused.lock().map(|p| p.clone()).unwrap_or_default(),
    };
    if targets.is_empty() {
        return Err("没有可续传的任务".to_string());
    }
    let mut done = 0usize;
    let mut last_error: Option<String> = None;
    for task in targets {
        match resume_one(app, state, task) {
            Ok(()) => done += 1,
            Err(error) => last_error = Some(error),
        }
    }
    if done == 0 {
        return Err(last_error.unwrap_or_else(|| "没有可续传的任务".to_string()));
    }
    Ok(done)
}

/// 续传单条（resume_transfer 的实现体）
fn resume_one(app: &AppHandle, state: &HubSftpState, task: XferTask) -> Result<(), String> {
    let server = load_server_by_id(app, &task.server_id)?;
    let commands = match task.dir.as_str() {
        "down" => {
            let have = partial_size(&task.local_path);
            // 本地半截文件没了（用户手动删过/放弃竞态）：没有断点可续
            if have == 0 {
                return Err("本地半截文件已不在，没有断点可续；请放弃这个任务后重新下载".to_string());
            }
            // 远端文件比断点小 = 服务器上文件换过了，续出来必然是错的
            let remote_size = match remote_size_of(app, &server, &task.remote_path)? {
                Some(size) => size,
                // 部分服务端 ls -l 也不给 size：不拦着续传（sftp 自己按本地
                // 大小续），只是少了"文件被换过"的防线
                None => have,
            };
            if remote_size < have {
                return Err(format!(
                    "远端文件比断点小（已传 {}，远端现在 {}），可能被替换过；请放弃后重新下载",
                    human_bytes(have),
                    human_bytes(remote_size),
                ));
            }
            format!("reget {} {}\n", sftp_quote(&task.remote_path), sftp_quote(&task.local_path))
        }
        "up" => {
            let have = remote_size_of(app, &server, &task.remote_path)?.unwrap_or(0);
            let src_size = partial_size(&task.local_path);
            if src_size == 0 {
                return Err("本地源文件已不存在，无法续传；请放弃这个任务".to_string());
            }
            if src_size < have {
                return Err(format!(
                    "本地文件比断点小（远端已有 {}，本地现在 {}），可能被替换过；请放弃后重新上传",
                    human_bytes(have),
                    human_bytes(src_size),
                ));
            }
            format!("reput {} {}\n", sftp_quote(&task.local_path), sftp_quote(&task.remote_path))
        }
        other => return Err(format!("未知传输方向: {other}")),
    };
    // 事件与进程归属沿用原任务的来源窗口（旧任务可能来自主窗内嵌文件浏览）
    let window = if task.window.is_empty() {
        crate::HUB_FILES_WINDOW_LABEL.to_string()
    } else {
        task.window.clone()
    };
    let mut resumed = task.clone();
    resumed.state = "running".to_string();
    resumed.paused = false;
    resumed.window = window.clone();
    // 先起新子进程再摘记录：spawn 失败（连不上/断点校验不过）时记录还在清单
    // 里，用户可以重试或放弃 —— 先摘就会把任务弄丢。
    // spawn_sftp_transfer 内部会立刻广播一条 running 事件（卡片从"已暂停"
    // 立刻切回"下载中"，不必等第一次进度）。
    let child = spawn_sftp_transfer(app, state, &window, &server, &commands, resumed.clone())?;
    remove_paused(state, app, &task.key);
    spawn_progress_poller(app.clone(), child, resumed, server);
    Ok(())
}

/// 放弃（cancel 是它的旧名）：删半截文件（down 删本地、up 删远端）+ 删记录。
///
/// `key=None`（「全部放弃」）时清掉**跑着的全部 + 断点清单的全部**。旧实现
/// 只 `pick_paused(None)` 取最近一条删，于是「全部放弃」之后清单里还躺着
/// 其余暂停任务 —— 卡片永远消不掉。
///
/// 每个真正被清掉的任务都广播一条 `abandoned`：这是过去完全没有事件的动作，
/// 所以「文件窗点放弃、传输页卡片还在」只能等 3s 轮询甚至永远不消失。
/// 三个位置都兜：断点清单、跑着的任务（先杀再清）、都没有时也返回成功 ——
/// 前端幽灵卡（清单已清但 UI 没同步掉）点放弃必须能消卡，而不是报错卡死。
fn abandon_transfer(
    app: &AppHandle,
    state: &HubSftpState,
    key: Option<&str>,
) -> Result<usize, String> {
    let mut acted = 0usize;

    // ① 跑着的同键任务先杀掉（放弃 running 里的任务同样合法）
    let victims: Vec<RunningXfer> = {
        let Ok(mut list) = state.running.lock() else {
            return Err("传输进程表已被锁死".to_string());
        };
        let victims: Vec<RunningXfer> = match key {
            Some(k) => list.iter().filter(|r| r.task.key == k).cloned().collect(),
            None => list.clone(),
        };
        for running in &victims {
            list.retain(|r| r.task.key != running.task.key);
        }
        victims
    };
    for running in victims {
        if let Ok(mut child) = running.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // 半截文件顺手清（清单路径里没有这条时也不留垃圾）
        if running.task.dir == "down" {
            let _ = std::fs::remove_file(&running.task.local_path);
        }
        acted += 1;
        push_recent(state, &running.task, "abandoned");
        emit_xfer(app, &running.task, "abandoned", 0, running.task.total);
    }

    // ② 断点清单：key 给定删一条，没给就整清
    let mut queue_dirty = false;
    let tasks: Vec<XferTask> = match state.paused.lock() {
        Ok(mut paused) => {
            let before = paused.len();
            let taken: Vec<XferTask> = match key {
                Some(k) => {
                    let found = paused.iter().find(|t| t.key == k).cloned();
                    paused.retain(|t| t.key != k);
                    found.into_iter().collect()
                }
                None => paused.drain(..).collect(),
            };
            queue_dirty = paused.len() != before;
            taken
        }
        Err(_) => Vec::new(),
    };
    for task in tasks {
        // 半截文件先清掉再摘记录：清失败不拦着放弃，记录也照样走。
        // 已传满的记录（旧版本竞态留下的 100% 断点）文件是完整的，绝不能删。
        let complete = {
            let mut probe = task.clone();
            probe.bytes = xfer_bytes_now(&probe);
            xfer_is_complete(&probe)
        };
        if !complete {
            match task.dir.as_str() {
                "down" => {
                    let _ = std::fs::remove_file(&task.local_path);
                }
                "up" => {
                    if let Ok(server) = load_server_by_id(app, &task.server_id) {
                        let _ = run_sftp_batch(
                            app,
                            &server,
                            &format!("rm {}\n", sftp_quote(&task.remote_path)),
                        );
                    }
                }
                _ => {}
            }
        }
        acted += 1;
        push_recent(state, &task, "abandoned");
        emit_xfer(app, &task, "abandoned", 0, task.total);
    }
    if queue_dirty {
        persist_queue(state, app);
    }
    // 一条都没清到（幽灵卡）：按「无事可做但成功」处理，前端据此消卡
    Ok(acted.max(1))
}

/// 挑一条要操作的断点记录：key 给定就精确找，没给就取最近活动的一条。
/// 精确找不中（比如应用重启后键没变但清单丢了）比静默续错任务强，直接报错。
fn pick_paused(state: &HubSftpState, key: Option<&str>) -> Option<XferTask> {
    state.paused.lock().ok().and_then(|paused| match key {
        Some(k) => paused.iter().find(|t| t.key == k).cloned(),
        None => paused.iter().max_by_key(|t| t.at).cloned(),
    })
}

/// 按档案 id 重新加载服务器档案（续传/放弃要用完整档案去连）
fn load_server_by_id(app: &AppHandle, server_id: &str) -> Result<HubServer, String> {
    let servers = crate::hub::load_servers_pub(app)?;
    servers
        .into_iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| "该服务器档案已删除，无法继续这个传输".to_string())
}

/// 远端文件大小（`ls -l` 的 size 列；解析不出返回 None）。
/// 注意必须 `ls -l`：真机 E2E 实锤部分服务端（paramiko）的裸 `ls /file`
/// 只回文件名不带任何元数据。size 永远在权限列后第 4 个字段
/// （perms nlink uid gid size），用权限列锚定 —— 别掐最后一个数字
/// （文件名叫 "12345" 时会掐到文件名）。
fn remote_size_of(
    app: &AppHandle,
    server: &HubServer,
    remote_path: &str,
) -> Result<Option<u64>, String> {
    let out = run_sftp_batch(app, server, &format!("ls -l {}\n", sftp_quote(remote_path)))?;
    for line in out.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 5 {
            continue;
        }
        // 权限列：≥10 字符、字符都在权限符号集里、首字符是 -/d/l
        let perms_idx = tokens.iter().position(|t| {
            t.len() >= 10
                && t.chars().all(|c| "-rwxstTdlbcps".contains(c))
                && matches!(t.chars().next(), Some('-') | Some('d') | Some('l'))
        });
        let size_idx = match perms_idx {
            Some(i) if i + 4 < tokens.len() => i + 4,
            // 没锚到权限列就按标准 ls 顺序赌第 5 个字段（perms nlink uid gid size）
            _ => 4,
        };
        if let Ok(size) = tokens[size_idx].parse::<u64>() {
            return Ok(Some(size));
        }
    }
    Ok(None)
}

/// 字节数的人话展示（报错文案用，不依赖前端）
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    if n == 0 {
        return "0B".to_string();
    }
    let mut value = n as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 || unit == 0 {
        format!("{:.0}{}", value, UNITS[unit])
    } else {
        format!("{:.1}{}", value, UNITS[unit])
    }
}

/// 把跑着的任务的最新进度刷回 running 表（传输页清单读它）
fn update_running_bytes(state: &HubSftpState, key: &str, bytes: u64) {
    if let Ok(mut list) = state.running.lock() {
        if let Some(running) = list.iter_mut().find(|r| r.task.key == key) {
            running.task.bytes = bytes;
        }
    }
}

/// 断点清单里有没有这个键
fn is_paused(state: &HubSftpState, key: &str) -> bool {
    state
        .paused
        .lock()
        .map(|paused| paused.iter().any(|t| t.key == key))
        .unwrap_or(false)
}

/// 传输子进程的进度轮询 + 收尾（down 用本地文件大小、up 定期摸远端大小）。
/// 退出收尾三态：成功 → done 事件（顺手清可能的竞态断点记录）；
/// 被 resume 接管（新进程同键）→ 静默退出；被暂停 → paused 事件；
/// 其余 → failed 事件。事件只发窗口（传输页走轮询清单，不依赖事件）。
fn spawn_progress_poller(
    app: AppHandle,
    child: Arc<Mutex<std::process::Child>>,
    mut task: XferTask,
    server: HubServer,
) {
    std::thread::spawn(move || {
        let state = app.state::<HubSftpState>();
        let is_up = task.dir == "up";
        let mut last_bytes: u64 = if is_up { 0 } else { partial_size(&task.local_path) };
        let mut tick = 0u32;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let status = child
                .lock()
                .ok()
                .and_then(|mut c| c.try_wait().ok().flatten());
            if is_up {
                // 上传的进度在远端：每 ~1.5s 摸一次（复用 ControlMaster，开销可忽略）
                tick += 1;
                if tick % 5 == 0 && status.is_none() {
                    if let Ok(Some(bytes)) = remote_size_of(&app, &server, &task.remote_path) {
                        task.bytes = bytes;
                        emit_xfer(&app, &task, "running", bytes, task.total);
                        update_running_bytes(&state, &task.key, bytes);
                    }
                }
            } else {
                let bytes = partial_size(&task.local_path);
                if bytes != last_bytes {
                    last_bytes = bytes;
                    emit_xfer(&app, &task, "running", bytes, task.total);
                    update_running_bytes(&state, &task.key, bytes);
                }
            }
            if let Some(status) = status {
                finish_transfer(&app, &state, &child, &task, status);
                return;
            }
        }
    });
}

/// 从 running 表摘掉「本进程」这一条。
/// 同 key 可能已被 resume 换成新进程（Arc 不同），按指针比才不会误摘新任务。
fn detach_running(state: &HubSftpState, key: &str, child: &Arc<Mutex<std::process::Child>>) {
    if let Ok(mut list) = state.running.lock() {
        list.retain(|r| !(r.task.key == key && Arc::ptr_eq(&r.child, child)));
    }
}

/// 子进程退出后的收尾（见 spawn_progress_poller 的三态注释）
fn finish_transfer(
    app: &AppHandle,
    state: &tauri::State<'_, HubSftpState>,
    child: &Arc<Mutex<std::process::Child>>,
    task: &XferTask,
    status: std::process::ExitStatus,
) {
    if status.success() {
        // 防竞态：完成前瞬间被点过暂停的话会留下一条"全量"断点记录，顺手清掉
        remove_paused(state, app, &task.key);
        let bytes = if task.total > 0 {
            task.total
        } else {
            partial_size(&task.local_path)
        };
        push_recent(state, task, "done");
        emit_xfer(app, task, "done", bytes, task.total);
        return;
    }
    // resume 已经用新进程接管（同键不同把）→ 这个老轮询的退出事件别发，
    // 免得把新任务从队列里吓掉
    let successor = state
        .running
        .lock()
        .map(|list| {
            list.iter()
                .any(|r| r.task.key == task.key && !Arc::ptr_eq(&r.child, child))
        })
        .unwrap_or(false);
    if successor {
        return;
    }
    // 收尾事件的所有权：只有 running 表还认得这个把（=这条目没被别人摘走）
    // 时，本线程才负责发终态事件；已被摘走（暂停/接管）的不重复发。
    let owned = state
        .running
        .lock()
        .map(|list| list.iter().any(|r| r.task.key == task.key && Arc::ptr_eq(&r.child, child)))
        .unwrap_or(false);
    if !owned {
        return;
    }
    // 杀进程/断线可能正好落在"最后字节已落盘、退出码还没来得及是 0"的窗口：
    // 文件已完整 → 按完成收尾（清掉竞态断点记录），不进任何暂停/失败分支。
    let mut task = task.clone();
    task.bytes = xfer_bytes_now(&task);
    if xfer_is_complete(&task) {
        // 摘除 + 发终态一把抓：先从 running 表摘掉自己，再发 done ——
        // 别让并发的第二个查询看到"已完成却还在跑"。
        detach_running(state, &task.key, child);
        remove_paused(state, app, &task.key);
        push_recent(state, &task, "done");
        emit_xfer(app, &task, "done", task.total, task.total);
        return;
    }
    // 无论暂停还是中断，进程都已经退出 —— 必须先从 running 表摘掉自己。
    // 过去这两个分支没摘，于是 transfer_list 一直把死进程报成 running，
    // 前端卡片永远停在「下载中」（实测复现：sftp 失败 4s 后清单里还挂着 running）。
    detach_running(state, &task.key, child);
    if is_paused(state, &task.key) {
        // 被暂停：down 的断点=本地半截文件；up 的断点=轮询刷进来的远端尺寸
        emit_xfer(app, &task, "paused", task.bytes, task.total);
    } else {
        // 非用户暂停的退出（网络断、连接被重置等）= 中断：自动挂进断点清单，
        // 半截文件留着，传输页/文件窗都能从断点继续 —— 与手机端行为对齐。
        // 状态统一报 paused 而不是 failed：它确实还能续传，报 failed 会让前端
        // 把卡片删掉，而清单里又留着记录 —— 两边对不上账就是「状态不同步」。
        let mut interrupted = task.clone();
        interrupted.at = now_ms();
        upsert_paused(state, app, interrupted.clone());
        emit_xfer(app, &interrupted, "paused", interrupted.bytes, interrupted.total);
    }
}

/// hub 传输页「聚焦」独立窗口（前端 window API 无 ACL 权限，走后端）。
#[tauri::command]
pub fn hub_focus_window(app: AppHandle, label: String) -> Result<(), String> {
    match app.get_webview_window(&label) {
        Some(window) => {
            let _ = window.show();
            let _ = window.set_focus();
            Ok(())
        }
        None => Err(format!("窗口不存在（{label}）")),
    }
}

/// hub「浏览文件」：确保 server-files 窗口存在并聚焦；窗口新开 → pending 队列，
/// 已存在 → 给页面发 hub-files-open-server 事件（页面换服务器并回到家目录）。
pub fn open_files_window(app: &AppHandle, server: HubServer) -> Result<(), String> {
    const LABEL: &str = crate::HUB_FILES_WINDOW_LABEL;
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        tauri::async_runtime::spawn(async move {
            let _ = tauri::Emitter::emit(
                &window,
                "hub-files-open-server",
                serde_json::to_value(&server).unwrap_or_default(),
            );
        });
        return Ok(());
    }
    {
        let state = app.state::<HubSftpState>();
        // ① match 而不是 if let、② 带分号（块尾表达式）：if let 的 Result 临时值
        // 析构晚于 state 会报 E0597，官方建议就是补分号让临时值先死
        match state.pending.lock() {
            Ok(mut pending) => *pending = Some(server),
            Err(_) => {}
        };
    }
    tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        tauri::WebviewUrl::App("hub-files.html".into()),
    )
    .title("AnyBoot 文件")
    .inner_size(980.0, 660.0)
    .min_inner_size(760.0, 480.0)
    .resizable(true)
    .center()
    .build()
    .map_err(|error| format!("无法创建文件浏览窗口: {error}"))?;
    Ok(())
}

/// server-files 页面加载后取走排队的启动服务器（取走即清空）。
#[tauri::command]
pub fn hub_sftp_take_pending(state: tauri::State<'_, HubSftpState>) -> Option<HubServer> {
    state.pending.lock().ok().and_then(|mut p| p.take())
}

/// 远端家目录（`pwd` → "Remote working directory: /home/arch"）。
/// 注意别用 `pwd -P`：老版 OpenSSH 的 sftp 不认这个旗子。
#[tauri::command]
pub async fn hub_sftp_home(app: AppHandle, server: HubServer) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || hub_sftp_home_blocking(app, server))
        .await
        .map_err(|error| format!("hub_sftp_home 任务失败: {error}"))?
}

/// 原实现：同步起 sftp/ssh 子进程（会阻塞到子进程收工），
/// **只允许在阻塞线程池里调用** —— 同步命令跑在 UI 线程上，一阻塞就是整窗无响应。
fn hub_sftp_home_blocking(app: AppHandle, server: HubServer) -> Result<String, String> {
    let server = server.normalize();
    let out = run_sftp_batch(&app, &server, "pwd\n")?;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("Remote working directory: ") {
            return Ok(rest.trim().to_string());
        }
    }
    Err("无法确定远端家目录".to_string())
}

#[tauri::command]
pub async fn hub_sftp_list(app: AppHandle, server: HubServer, path: String) -> Result<Vec<SftpEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || hub_sftp_list_blocking(app, server, path))
        .await
        .map_err(|error| format!("hub_sftp_list 任务失败: {error}"))?
}

/// 原实现：同步起 sftp/ssh 子进程（会阻塞到子进程收工），
/// **只允许在阻塞线程池里调用** —— 同步命令跑在 UI 线程上，一阻塞就是整窗无响应。
fn hub_sftp_list_blocking(app: AppHandle, server: HubServer, path: String) -> Result<Vec<SftpEntry>, String> {
    let server = server.normalize();
    let out = run_sftp_batch(&app, &server, &format!("ls -la {}\n", sftp_quote(&path)))?;
    let mut entries: Vec<SftpEntry> = out
        .lines()
        .filter_map(parse_ls_line)
        .filter_map(|mut entry| match bare_name(&entry.name, &path) {
            Some(name) => {
                entry.name = name;
                Some(entry)
            }
            None => None,
        })
        .collect();
    entries.sort_by(|a, b| {
        b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

#[tauri::command]
pub async fn hub_sftp_mkdir(app: AppHandle, server: HubServer, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || hub_sftp_mkdir_blocking(app, server, path))
        .await
        .map_err(|error| format!("hub_sftp_mkdir 任务失败: {error}"))?
}

/// 原实现：同步起 sftp/ssh 子进程（会阻塞到子进程收工），
/// **只允许在阻塞线程池里调用** —— 同步命令跑在 UI 线程上，一阻塞就是整窗无响应。
fn hub_sftp_mkdir_blocking(app: AppHandle, server: HubServer, path: String) -> Result<(), String> {
    let server = server.normalize();
    run_sftp_batch(&app, &server, &format!("mkdir {}\n", sftp_quote(&path))).map(|_| ())
}

#[tauri::command]
pub async fn hub_sftp_remove(app: AppHandle, server: HubServer, path: String, is_dir: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || hub_sftp_remove_blocking(app, server, path, is_dir))
        .await
        .map_err(|error| format!("hub_sftp_remove 任务失败: {error}"))?
}

/// 原实现：同步起 sftp/ssh 子进程（会阻塞到子进程收工），
/// **只允许在阻塞线程池里调用** —— 同步命令跑在 UI 线程上，一阻塞就是整窗无响应。
fn hub_sftp_remove_blocking(app: AppHandle, server: HubServer, path: String, is_dir: bool) -> Result<(), String> {
    let server = server.normalize();
    let verb = if is_dir { "rmdir" } else { "rm" };
    run_sftp_batch(&app, &server, &format!("{verb} {}\n", sftp_quote(&path))).map(|_| ())
}

#[tauri::command]
pub async fn hub_sftp_rename(app: AppHandle, server: HubServer, from: String, to: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || hub_sftp_rename_blocking(app, server, from, to))
        .await
        .map_err(|error| format!("hub_sftp_rename 任务失败: {error}"))?
}

/// 原实现：同步起 sftp/ssh 子进程（会阻塞到子进程收工），
/// **只允许在阻塞线程池里调用** —— 同步命令跑在 UI 线程上，一阻塞就是整窗无响应。
fn hub_sftp_rename_blocking(app: AppHandle, server: HubServer, from: String, to: String) -> Result<(), String> {
    let server = server.normalize();
    run_sftp_batch(
        &app,
        &server,
        &format!("rename {} {}\n", sftp_quote(&from), sftp_quote(&to)),
    )
    .map(|_| ())
}

/// 远端复制（操作面板「复制到...」）：sftp 协议没有 copy，走远端 shell 的
/// `cp -a` 一次到位（同盘远端复制不过本机中转）。目标已存在则覆盖。
#[tauri::command]
pub async fn hub_sftp_copy(app: AppHandle, server: HubServer, from: String, to: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || hub_sftp_copy_blocking(app, server, from, to))
        .await
        .map_err(|error| format!("hub_sftp_copy 任务失败: {error}"))?
}

/// 原实现：同步起 sftp/ssh 子进程（会阻塞到子进程收工），
/// **只允许在阻塞线程池里调用** —— 同步命令跑在 UI 线程上，一阻塞就是整窗无响应。
fn hub_sftp_copy_blocking(app: AppHandle, server: HubServer, from: String, to: String) -> Result<(), String> {
    let server = server.normalize();
    if server.id.is_empty() {
        return Err("该服务器还没保存档案，无法复用认证连接".to_string());
    }
    let target = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };
    let mut command = crate::command("ssh");
    crate::hub_tunnel::apply_auth(&app, &server, &mut command, NO_CRED_MESSAGE)?;
    command
        .arg("-p")
        .arg(server.effective_port().to_string())
        .arg(&target)
        .arg("--")
        .arg(format!("cp -a -- {} {}", shell_quote(&from), shell_quote(&to)));
    command.env("LC_ALL", "C.UTF-8");
    let output = command
        .output()
        .map_err(|error| format!("无法启动 ssh: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let tail = stderr_tail(&output.stderr);
        Err(if tail.is_empty() {
            format!("远端复制失败（退出码 {:?}）", output.status.code())
        } else {
            tail
        })
    }
}

/// 软链接指向（详情弹层按需拉取）：sftp CLI 没暴露协议级 readlink（真机验证
/// "Invalid command."），且真 sshd 的 `ls -la` longname 不带 ` -> target`——
/// 走 exec 通道 `readlink -f`（与 cp 同一远端 shell 依赖），返回绝对指向。
#[tauri::command]
pub async fn hub_sftp_readlink(app: AppHandle, server: HubServer, path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || hub_sftp_readlink_blocking(app, server, path))
        .await
        .map_err(|error| format!("hub_sftp_readlink 任务失败: {error}"))?
}

/// 原实现：同步起 sftp/ssh 子进程（会阻塞到子进程收工），
/// **只允许在阻塞线程池里调用** —— 同步命令跑在 UI 线程上，一阻塞就是整窗无响应。
fn hub_sftp_readlink_blocking(app: AppHandle, server: HubServer, path: String) -> Result<String, String> {
    let server = server.normalize();
    if server.id.is_empty() {
        return Err("该服务器还没保存档案，无法复用认证连接".to_string());
    }
    let target = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };
    let mut command = crate::command("ssh");
    crate::hub_tunnel::apply_auth(&app, &server, &mut command, NO_CRED_MESSAGE)?;
    command
        .arg("-p")
        .arg(server.effective_port().to_string())
        .arg(&target)
        .arg("--")
        .arg(format!("readlink -f -- {}", shell_quote(&path)));
    command.env("LC_ALL", "C.UTF-8");
    let output = command
        .output()
        .map_err(|error| format!("无法启动 ssh: {error}"))?;
    if output.status.success() {
        let target = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if target.is_empty() {
            Err("远端未返回指向".to_string())
        } else {
            Ok(target)
        }
    } else {
        let tail = stderr_tail(&output.stderr);
        Err(if tail.is_empty() {
            format!("读取指向失败（退出码 {:?}）", output.status.code())
        } else {
            tail
        })
    }
}

/// 用户主目录：Windows 没有 POSIX 的 `HOME` 变量（只有 `USERPROFILE`），
/// 只读 `HOME` 会让 `~` 展开失败，落成相对路径 `~\文件`（跑到进程工作目录）。
fn user_home() -> Option<String> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

/// 展开 `~` 前缀为用户主目录（`HOME` / `USERPROFILE`；都取不到时原样返回）。
/// 拼接统一用 `/`：Windows 的 Win32-OpenSSH 同样接受正斜杠，且不依赖平台分隔符。
fn expand_home(dir: &str) -> String {
    let trimmed = dir.trim();
    let home = user_home();
    let rest = if trimmed == "~" {
        Some("")
    } else {
        trimmed
            .strip_prefix("~/")
            .or_else(|| trimmed.strip_prefix("~\\"))
    };
    match (rest, home) {
        (Some(""), Some(home)) => home.trim_end_matches(['/', '\\']).to_string(),
        (Some(rest), Some(home)) => {
            let rest = rest.trim_start_matches(['/', '\\']);
            format!("{}/{}", home.trim_end_matches(['/', '\\']), rest)
        }
        _ => trimmed.to_string(),
    }
}

/// 默认下载目录：系统「下载」目录（Windows 已知文件夹 / macOS `~/Downloads` /
/// Linux 的 XDG 下载目录），取不到再退回用户主目录。
///
/// 前端设置页与新装用户的 `downDir` 初始值都来自这里 —— 不再写死 `~`
/// （Windows 上 `~` 曾展开失败，文件直接落到程序工作目录）。
#[tauri::command]
pub fn hub_download_dir_default(app: AppHandle) -> Result<String, String> {
    if let Ok(dir) = app.path().download_dir() {
        return Ok(dir.display().to_string());
    }
    app.path()
        .home_dir()
        .map(|dir| dir.display().to_string())
        .map_err(|error| format!("取不到用户目录: {error}"))
}

/// 目录选择框（设置页「下载路径 → 选择…」）：返回选中的绝对路径，取消返回 `None`。
#[tauri::command]
pub async fn hub_pick_directory(
    app: AppHandle,
    current: Option<String>,
) -> Result<Option<String>, String> {
    let start = current
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(expand_home)
        .filter(|value| std::path::Path::new(value).is_dir());
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut dialog = app.dialog().file().set_title("选择下载目录");
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_folder(move |path| {
            let _ = tx.send(path.and_then(|p| p.into_path().ok()));
        });
        rx.recv().map_err(|_| "目录选择框被关闭".to_string())
    })
    .await
    .map_err(|error| format!("目录选择框执行失败: {error}"))?
    .map(|path| path.map(|path| path.display().to_string()))
}

/// 下载单个文件：`local_dir` 给定时直接落到该目录（`~` 展开，默认主目录），
/// 未给定时退回系统存盘对话框 → sftp get → 轮询本地文件大小推进度。
/// 返回落盘路径。
#[tauri::command]
pub async fn hub_sftp_download(
    app: AppHandle,
    state: tauri::State<'_, HubSftpState>,
    window: tauri::WebviewWindow,
    server: HubServer,
    remote_path: String,
    total: u64,
    filename: String,
    local_dir: Option<String>,
) -> Result<String, String> {
    let server = server.normalize();
    if server.id.is_empty() {
        return Err("该服务器还没保存档案，无法自动认证".to_string());
    }
    let local = match local_dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(dir) => {
            let dir = expand_home(dir);
            let mut p = std::path::PathBuf::from(&dir);
            p.push(&filename);
            p
        }
        // 存盘对话框（阻塞等结果，丢 spawn_blocking）
        None => {
            let dialog_app = app.clone();
            let suggested = filename.clone();
            let picked = tauri::async_runtime::spawn_blocking(move || {
                let (tx, rx) = std::sync::mpsc::channel();
                dialog_app
                    .dialog()
                    .file()
                    .set_title("保存到本机")
                    .set_file_name(&suggested)
                    .pick_file(move |path| {
                        let _ = tx.send(path.and_then(|p| p.into_path().ok()));
                    });
                rx.recv().map_err(|_| "存盘对话框被关闭".to_string())
            })
            .await
            .map_err(|error| format!("存盘对话框执行失败: {error}"))??;
            picked.ok_or_else(|| "未选择保存位置".to_string())?
        }
    };
    let local_display = local.display().to_string();

    let commands = format!(
        "get {} {}\n",
        sftp_quote(&remote_path),
        sftp_quote(&local_display)
    );
    let task = XferTask {
        key: xfer_key("down", &remote_path, &local_display),
        dir: "down".to_string(),
        name: filename.clone(),
        remote_path: remote_path.clone(),
        local_path: local_display.clone(),
        total,
        bytes: 0,
        server_id: server.id.clone(),
        server_name: server.name.clone(),
        paused: false,
        state: "running".to_string(),
        window: window.label().to_string(),
        at: now_ms(),
    };
    let child = spawn_sftp_transfer(&app, state.inner(), window.label(), &server, &commands, task.clone())?;

    // 轮询本地文件大小 → 进度事件；传输完成后再核 sftp 退出码
    spawn_progress_poller(app, child, task, server);
    Ok(local_display)
}

/// 上传：系统多选对话框选本机文件 → 逐个 put 到远端目录。每个文件一个
/// sftp 子进程（可独立暂停/续传/放弃），进度靠轮询远端半截文件尺寸。
#[tauri::command]
pub async fn hub_sftp_upload(
    app: AppHandle,
    state: tauri::State<'_, HubSftpState>,
    window: tauri::WebviewWindow,
    server: HubServer,
    remote_dir: String,
) -> Result<Vec<String>, String> {
    let server = server.normalize();
    if server.id.is_empty() {
        return Err("该服务器还没保存档案，无法自动认证".to_string());
    }
    let dialog_app = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        dialog_app
            .dialog()
            .file()
            .set_title("选择要上传的文件")
            .pick_files(move |paths| {
                let picked: Vec<String> = paths
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|p| p.into_path().ok())
                    .map(|p| p.display().to_string())
                    .collect();
                let _ = tx.send(picked);
            });
        rx.recv().map_err(|_| "上传对话框被关闭".to_string())
    })
    .await
    .map_err(|error| format!("上传对话框执行失败: {error}"))??;
    if picked.is_empty() {
        return Err("未选择文件".to_string());
    }

    let window_label = window.label().to_string();
    let mut uploaded: Vec<String> = Vec::new();
    for local in picked {
        let name = std::path::Path::new(&local)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| format!("路径没有文件名（{local}）"))?;
        let remote = format!("{}/{}", remote_dir.trim_end_matches('/'), name);
        let total = std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0);
        let task = XferTask {
            key: xfer_key("up", &remote, &local),
            dir: "up".to_string(),
            name: name.clone(),
            remote_path: remote.clone(),
            local_path: local.clone(),
            total,
            bytes: 0,
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            paused: false,
            state: "running".to_string(),
            window: window_label.clone(),
            at: now_ms(),
        };
        // 每个文件一个子进程：暂停/放弃能精确到单个文件，上传失败也不连坐
        let child = spawn_sftp_transfer(
            &app,
            state.inner(),
            &window_label,
            &server,
            &format!("put {} {}\n", sftp_quote(&local), sftp_quote(&remote)),
            task.clone(),
        )?;
        // 不等它：起轮询线程接管进度与收尾，主循环继续排下一个文件。
        // 顺序语义由 picked 的迭代顺序保证（多文件仍是一个接一个传）。
        spawn_progress_poller(app.clone(), child, task, server.clone());
        uploaded.push(name);
    }
    Ok(uploaded)
}

/// 应用退出：收掉所有还在跑的传输子进程（不分窗口）。
/// 注意 kill 是硬杀不是暂停 —— 半截文件留着，但记录不进断点清单
/// （退出场景没法区分用户意图；断点清单只记用户明确暂停的任务）。
pub fn kill_all(app: &AppHandle) {
    if let Some(state) = app.try_state::<HubSftpState>() {
        if let Ok(mut list) = state.running.lock() {
            list.drain(..).for_each(|running| {
                if let Ok(mut child) = running.child.lock() {
                    let _ = child.kill();
                }
            });
        }
    }
}

/// server-files 窗口销毁：收掉它名下还在跑的传输子进程
pub fn kill_window(app: &AppHandle, label: &str) {
    if let Some(state) = app.try_state::<HubSftpState>() {
        if let Ok(mut list) = state.running.lock() {
            list.retain(|running| {
                if running.window != label {
                    return true;
                }
                if let Ok(mut child) = running.child.lock() {
                    let _ = child.kill();
                }
                false
            });
        }
    }
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_home_variants() {
        std::env::set_var("HOME", "/home/tester");
        assert_eq!(expand_home("~"), "/home/tester");
        assert_eq!(expand_home("~/downloads"), "/home/tester/downloads");
        assert_eq!(expand_home(" ~/x "), "/home/tester/x");
        assert_eq!(expand_home("/abs/path"), "/abs/path");
        assert_eq!(expand_home("rel/path"), "rel/path");
    }

    #[test]
    fn bare_name_strips_list_prefix() {
        // OpenSSH sftp 长格式输出：查询路径作前缀
        assert_eq!(bare_name("/root/.cache", "/root").as_deref(), Some(".cache"));
        assert_eq!(bare_name("/root/docker", "/root").as_deref(), Some("docker"));
        // `.` / `..` 行丢弃
        assert_eq!(bare_name("/root/.", "/root"), None);
        assert_eq!(bare_name("/root/..", "/root"), None);
        // 查询路径带尾部斜杠 / 输出双斜杠都能剥干净
        assert_eq!(bare_name("/root//.cache", "/root/").as_deref(), Some(".cache"));
        assert_eq!(bare_name("/root/.cache", "/root/").as_deref(), Some(".cache"));
        // 根目录查询
        assert_eq!(bare_name("/root", "/").as_deref(), Some("root"));
        assert_eq!(bare_name("/.", "/"), None);
        // 服务器输出裸名字时原样保留
        assert_eq!(bare_name(".cache", "/root").as_deref(), Some(".cache"));
        // 目录名带尾斜杠（个别服务器实机场景）剥干净
        assert_eq!(bare_name("/root/docs/", "/root").as_deref(), Some("docs"));
        assert_eq!(bare_name("docs/", "/root").as_deref(), Some("docs"));
        // 软链接名字（parse 已拆掉 " -> target"，此处是纯路径）原样保留
        assert_eq!(bare_name("/root/link", "/root").as_deref(), Some("link"));
    }

    #[test]
    fn parse_real_sftp_output_samples() {
        // 真实 OpenSSH sftp CLI 采集（paramiko 靶机 + 真 sshd 各采过，字段间距勿改）：
        // paramiko 风格 longname：日期「日 月」顺序、é 原样（LC_ALL=C.UTF-8 下客户端不转义）
        let e =
            parse_ls_line("-rw-r--r--   1 0        0              10 17 Oct 19:14 a b'c\"d é.txt")
                .unwrap();
        assert_eq!(e.name, "a b'c\"d é.txt");
        // 单文件 ls：客户端自排版（nlink 为 ?、日期「月 日」顺序、名字带全路径）
        let f =
            parse_ls_line("-rw-r--r--    ? 0        0               1 Oct 17 19:14 /.hidden")
                .unwrap();
        assert_eq!(f.name, "/.hidden");
        assert_eq!(bare_name(&f.name, "/").as_deref(), Some(".hidden"));
        // 真 sshd 的软链接行：箭头前带全路径
        let l = parse_ls_line("lrwxrwxrwx 1 root root 11 Oct  2 10:00 /srv/latest -> v1.2.3")
            .unwrap();
        assert_eq!(l.name, "/srv/latest");
        assert_eq!(l.target.as_deref(), Some("v1.2.3"));
        assert_eq!(bare_name(&l.name, "/srv").as_deref(), Some("latest"));
    }

    #[test]
    fn parse_ls_line_variants() {
        let entry = parse_ls_line(
            "-rw-r--r--    1 root     root      1234567 Oct 16 09:12 backup.tar.gz",
        )
        .expect("文件行");
        assert_eq!(entry.name, "backup.tar.gz");
        assert!(!entry.dir);
        assert_eq!(entry.size, 1234567);
        assert_eq!(entry.mtime, "10-16 09:12");
        assert_eq!(entry.perms, "-rw-r--r--");

        let dir = parse_ls_line("drwxr-xr-x    6 arch     arch         4096 Oct 15 22:40 .config")
            .expect("目录行");
        assert!(dir.dir);
        assert_eq!(dir.name, ".config");

        let old = parse_ls_line("drwxr-xr-x    2 root root 4096 Mar  1 2020 old-dir").unwrap();
        assert_eq!(old.mtime, "2020-03-01");

        // paramiko 流派：日 月 时（日期段顺序与 GNU 相反）
        let pki = parse_ls_line(
            "-rw-r--r--   1 0        0         8388608 17 Oct 01:46 ab-sftp-test.bin",
        )
        .expect("paramiko 行");
        assert_eq!(pki.name, "ab-sftp-test.bin");
        assert_eq!(pki.size, 8388608);
        assert_eq!(pki.mtime, "10-17 01:46");
        // paramiko 老文件：日 月 年
        let pki_old =
            parse_ls_line("drwxr-xr-x   1 0 0 4096  1 Mar 2020 old-dir").unwrap();
        assert_eq!(pki_old.mtime, "2020-03-01");

        // 带空格的文件名
        let spaced =
            parse_ls_line("-rw-r--r-- 1 root root 12 Oct  1 08:00 my file.txt").unwrap();
        assert_eq!(spaced.name, "my file.txt");

        // 软链接行：名字与指向拆开（名字保持纯路径，指向单独带给前端）
        let link =
            parse_ls_line("lrwxrwxrwx 1 root root 11 Oct  2 10:00 latest -> v1.2.3").unwrap();
        assert!(link.link);
        assert_eq!(link.name, "latest");
        assert_eq!(link.target.as_deref(), Some("v1.2.3"));

        // 软链接名字本身带 " ->"（少见但合法）时按第一处拆
        let link2 =
            parse_ls_line("lrwxrwxrwx 1 root root 11 Oct  2 10:00 a -> b -> c").unwrap();
        assert_eq!(link2.name, "a");
        assert_eq!(link2.target.as_deref(), Some("b -> c"));

        // 解析不了的行
        assert!(parse_ls_line("sftp> ls -la").is_none());
        assert!(parse_ls_line(".").is_none());
        assert!(parse_ls_line("drwxr-xr-x 2 root root 4096 Nope 1 10:00 x").is_none());
    }

    #[test]
    fn sftp_quote_escapes() {
        assert_eq!(sftp_quote("/a b/c.txt"), "\"/a b/c.txt\"");
        assert_eq!(sftp_quote("/a\"b"), "\"/a\\\"b\"");
        assert_eq!(sftp_quote("/a\\b"), "\"/a\\\\b\"");
    }

    #[test]
    fn shell_quote_escapes() {
        assert_eq!(shell_quote("/a/b"), "'/a/b'");
        assert_eq!(shell_quote("/a b"), "'/a b'");
        assert_eq!(shell_quote("/a'b"), "'/a'\\''b'");
        // 拼 ssh 远端命令串：cp -a -- 'src' 'dst'
        assert_eq!(
            format!("cp -a -- {} {}", shell_quote("/a b/c"), shell_quote("/d/e")),
            "cp -a -- '/a b/c' '/d/e'"
        );
    }

    #[test]
    fn month_map() {
        assert_eq!(month_num("Oct"), Some("10"));
        assert_eq!(month_num("oct"), None);
    }

    #[test]
    fn xfer_task_reads_old_queue_file() {
        // 旧版 transfer-queue.json 没有 state / window 字段：
        // 升级后必须还能读回（读不回来 = 用户的断点清单全丢）
        let old = r#"[{"key":"down|/a|/b","dir":"down","name":"a","remote_path":"/a",
            "local_path":"/b","total":10,"bytes":5,"server_id":"s","server_name":"n",
            "paused":true,"at":1}]"#;
        let tasks: Vec<XferTask> = serde_json::from_str(old).expect("旧清单必须能读回");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].state, "running");
        assert_eq!(tasks[0].window, "");
        assert!(tasks[0].paused);
    }

    #[test]
    fn xfer_key_format() {
        // 键 = dir|remote|local：三段拼死，前端只透传不解析
        assert_eq!(
            xfer_key("down", "/srv/a.iso", "/home/u/a.iso"),
            "down|/srv/a.iso|/home/u/a.iso"
        );
    }

    #[test]
    fn human_bytes_display() {
        assert_eq!(human_bytes(0), "0B");
        assert_eq!(human_bytes(512), "512B");
        assert_eq!(human_bytes(2048), "2.0K");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0M");
        // ≥100 主单位取整
        assert_eq!(human_bytes(150 * 1024 * 1024), "150M");
    }

    #[test]
    fn remote_size_parses_standard_ls() {
        // 标准七列：perms nlink uid gid size —— size 在权限列后第 4 个字段
        let task = |line: &str| {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            let perms_idx = tokens.iter().position(|t| {
                t.len() >= 10
                    && t.chars().all(|c| "-rwxstTdlbcps".contains(c))
                    && matches!(t.chars().next(), Some('-') | Some('d') | Some('l'))
            });
            match perms_idx {
                Some(i) if i + 4 < tokens.len() => tokens[i + 4].parse::<u64>().ok(),
                _ => tokens.get(4).and_then(|t| t.parse::<u64>().ok()),
            }
        };
        assert_eq!(task("-rw-r--r-- 1 root root 12345 Oct 17 10:00 /srv/a.iso"), Some(12345));
        // 文件名叫纯数字时不能掐错列
        assert_eq!(task("-rw-r--r-- 1 root root 999 Oct 17 10:00 /srv/12345"), Some(999));
        // paramiko 风格（uid/gid 缺省成 ?）
        assert_eq!(task("-rw-r--r-- 1 ? ? 4096 Jan 02 03:04 b.txt"), Some(4096));
        // 部分服务端裸 ls 无元数据（真机 E2E：paramiko 只回文件名）→ 解析不出 size
        assert_eq!(task("/half2.bin"), None);
    }

    #[test]
    fn xfer_is_complete_guard() {
        // 完成判定：total 未知时永不判完成（只能靠成功退出码）
        let base = XferTask {
            key: xfer_key("up", "/srv/a", "/tmp/src-a"),
            dir: "up".into(),
            name: "a".into(),
            remote_path: "/srv/a".into(),
            local_path: "/tmp/src-a".into(),
            total: 100,
            bytes: 100,
            server_id: "s".into(),
            server_name: "n".into(),
            paused: false,
            state: "running".to_string(),
            window: String::new(),
            at: 0,
        };
        assert!(xfer_is_complete(&base), "up：已传=total 判完成");
        let mut t = base.clone();
        t.bytes = 99;
        assert!(!xfer_is_complete(&t), "差一字节不算完");
        t.total = 0;
        t.bytes = 100;
        assert!(!xfer_is_complete(&t), "total 未知不判完成");
        // down 的进度读的是磁盘：拿真实临时文件测
        let dir = std::env::temp_dir().join(format!("ab-xfer-done-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let local = dir.join("a.iso");
        std::fs::write(&local, vec![0u8; 100]).unwrap();
        let mut down = base.clone();
        down.dir = "down".into();
        down.key = xfer_key("down", "/srv/a", &local.display().to_string());
        down.local_path = local.display().to_string();
        assert!(xfer_is_complete(&down), "down：本地文件=total 判完成");
        std::fs::write(&local, vec![0u8; 99]).unwrap();
        assert!(!xfer_is_complete(&down), "down：差一字节不算完");
        let _ = std::fs::remove_file(&local);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn queue_roundtrip_and_bad_json() {        let dir = std::env::temp_dir().join(format!("ab-xfer-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("transfer-queue.json");
        let tasks = vec![XferTask {
            key: xfer_key("down", "/srv/a", "/tmp/a"),
            dir: "down".into(),
            name: "a".into(),
            remote_path: "/srv/a".into(),
            local_path: "/tmp/a".into(),
            total: 100,
            bytes: 40,
            server_id: "srv-1".into(),
            server_name: "测试机".into(),
            paused: true,
            state: "paused".to_string(),
            window: String::new(),
            at: 1700000000000,
        }];
        save_queue(&path, &tasks).unwrap();
        assert_eq!(load_queue(&path).len(), 1);
        assert_eq!(load_queue(&path)[0].server_name, "测试机");
        // 坏 JSON 按空表处理，不 panic
        std::fs::write(&path, b"{not json").unwrap();
        assert!(load_queue(&path).is_empty());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
