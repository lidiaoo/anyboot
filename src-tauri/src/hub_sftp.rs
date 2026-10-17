//! server-hub M5：SFTP 文件浏览 / 传输（独立窗 `server-files`）的后端。
//!
//! 哲学与终端/隧道同源：**不引 SSH 库，直接复用系统 `sftp` CLI**。
//! - 认证不归这里管：sftp 用 `BatchMode=yes` + `ControlPath` 复用终端建立的
//!   ControlMaster 连接（见 hub_tunnel.rs 的 control_path）。所以规则同样是
//!   **先在该服务器开过一次终端完成认证**，文件浏览才能用；没有 master 时
//!   给明确报错，而不是无声挂死。
//! - 批命令走 `sftp -b -`（stdin），出错即中止 —— stderr 尾巴就是给用户的
//!   错误信息。
//! - 列表解析 `ls -la` 输出（LC_ALL=C 固定月份名为英文），日期归一成
//!   `MM-DD HH:MM` / `YYYY-MM-DD`。

use serde::Serialize;
use serde_json::json;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::hub::HubServer;

/// 进度事件（下载/上传推给 server-files 窗口）
const EVENT_PROGRESS: &str = "hub-sftp-progress";

// ==================== 数据 ====================

#[derive(Clone, Serialize)]
pub struct SftpEntry {
    pub name: String,
    pub dir: bool,
    pub link: bool,
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
    running: Mutex<Vec<(String, Arc<Mutex<std::process::Child>>)>>,
}

// ==================== sftp 批命令底座 ====================

fn sftp_quote(path: &str) -> String {
    format!(
        "\"{}\"",
        path.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

fn stderr_tail(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .last()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 跑一批 sftp 命令（-b -），返回 stdout。批里任何一条失败都会整体失败。
fn run_sftp_batch(app: &AppHandle, server: &HubServer, commands: &str) -> Result<String, String> {
    if server.id.is_empty() {
        return Err("该服务器还没保存档案，无法复用认证连接".to_string());
    }
    let control = crate::hub_tunnel::control_path(app, &server.id)?;
    if !control.exists() {
        return Err(
            "该服务器还没有可复用的认证连接：先打开一次终端（输完密码/通过认证）再用文件浏览"
                .to_string(),
        );
    }
    let target = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };
    let mut command = std::process::Command::new("sftp");
    command
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg(format!("ControlPath={}", control.display()))
        .arg("-P")
        .arg(server.effective_port().to_string())
        .arg("-b")
        .arg("-")
        .arg(&target);
    command
        .env("LC_ALL", "C")
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

/// 起一个不等待的 sftp 传输子进程（下载/上传）：登记进 running 表（按窗口），
/// 返回 Arc 把手给进度轮询线程。
fn spawn_sftp_transfer(
    state: &HubSftpState,
    window: &str,
    server: &HubServer,
    control: &std::path::Path,
    commands: &str,
) -> Result<Arc<Mutex<std::process::Child>>, String> {
    let target = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };
    let mut command = std::process::Command::new("sftp");
    command
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg(format!("ControlPath={}", control.display()))
        .arg("-P")
        .arg(server.effective_port().to_string())
        .arg("-b")
        .arg("-")
        .arg(&target);
    command
        .env("LC_ALL", "C")
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
    state
        .running
        .lock()
        .map_err(|_| "传输进程表已被锁死")?
        .push((window.to_string(), handle.clone()));
    reap_finished(state);
    Ok(handle)
}

/// 把已经退出的传输子进程从 running 表摘掉（防止长会话里越积越多）
fn reap_finished(state: &HubSftpState) {
    if let Ok(mut list) = state.running.lock() {
        list.retain(|(_, handle)| {
            handle
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
    Some(SftpEntry {
        name,
        dir: perms.starts_with('d'),
        link: perms.starts_with('l'),
        size,
        mtime: normalize_mtime(mon, &day, &tail),
        perms,
    })
}

// ==================== 命令 ====================

/// 传输进度事件：发给文件窗，同时抄送 hub 主窗（传输页跨窗口汇总）。
fn emit_progress(app: &AppHandle, files_label: &str, payload: serde_json::Value) {
    let _ = app.emit_to(files_label, EVENT_PROGRESS, payload.clone());
    let _ = app.emit_to(crate::HUB_WINDOW_LABEL, EVENT_PROGRESS, payload);
}

/// 传输控制：对文件窗（server-files）里还在跑的 sftp 下载子进程发信号。
/// pause=SIGSTOP 挂起（本地文件大小停住，进度自然停）、resume=SIGCONT 继续、
/// cancel=SIGTERM 终止（轮询线程会以 failed 收尾，前端清队列）。返回作用的进程数。
#[tauri::command]
pub fn hub_sftp_transfer_ctl(
    state: tauri::State<'_, HubSftpState>,
    action: String,
) -> Result<usize, String> {
    let sig: i32 = match action.as_str() {
        "pause" => libc::SIGSTOP,
        "resume" => libc::SIGCONT,
        "cancel" => libc::SIGTERM,
        other => return Err(format!("未知传输控制动作: {other}")),
    };
    let mut acted = 0usize;
    if let Ok(list) = state.running.lock() {
        for (window, handle) in list.iter() {
            if window != crate::HUB_FILES_WINDOW_LABEL {
                continue;
            }
            if let Ok(mut child) = handle.lock() {
                // 只对还没退出的进程发信号（stopped 状态 try_wait 仍是 None，不误伤）
                if child.try_wait().ok().flatten().is_none() {
                    // SAFETY: libc::kill 传子进程 pid，仅发送信号
                    if unsafe { libc::kill(child.id() as i32, sig) } == 0 {
                        acted += 1;
                    }
                }
            }
        }
    }
    Ok(acted)
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
pub fn hub_sftp_home(app: AppHandle, server: HubServer) -> Result<String, String> {
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
pub fn hub_sftp_list(
    app: AppHandle,
    server: HubServer,
    path: String,
) -> Result<Vec<SftpEntry>, String> {
    let server = server.normalize();
    let out = run_sftp_batch(&app, &server, &format!("ls -la {}\n", sftp_quote(&path)))?;
    let mut entries: Vec<SftpEntry> = out.lines().filter_map(parse_ls_line).collect();
    entries.sort_by(|a, b| {
        b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

#[tauri::command]
pub fn hub_sftp_mkdir(app: AppHandle, server: HubServer, path: String) -> Result<(), String> {
    let server = server.normalize();
    run_sftp_batch(&app, &server, &format!("mkdir {}\n", sftp_quote(&path))).map(|_| ())
}

#[tauri::command]
pub fn hub_sftp_remove(
    app: AppHandle,
    server: HubServer,
    path: String,
    is_dir: bool,
) -> Result<(), String> {
    let server = server.normalize();
    let verb = if is_dir { "rmdir" } else { "rm" };
    run_sftp_batch(&app, &server, &format!("{verb} {}\n", sftp_quote(&path))).map(|_| ())
}

#[tauri::command]
pub fn hub_sftp_rename(
    app: AppHandle,
    server: HubServer,
    from: String,
    to: String,
) -> Result<(), String> {
    let server = server.normalize();
    run_sftp_batch(
        &app,
        &server,
        &format!("rename {} {}\n", sftp_quote(&from), sftp_quote(&to)),
    )
    .map(|_| ())
}

/// 下载单个文件：系统存盘对话框选本地路径 → sftp get → 轮询本地文件大小推进度。
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
) -> Result<String, String> {
    let server = server.normalize();
    if server.id.is_empty() {
        return Err("该服务器还没保存档案，无法复用认证连接".to_string());
    }
    let control = crate::hub_tunnel::control_path(&app, &server.id)?;
    if !control.exists() {
        return Err("先打开一次终端完成认证，再用文件浏览".to_string());
    }
    // 存盘对话框（阻塞等结果，丢 spawn_blocking）
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
    let local = picked.ok_or_else(|| "未选择保存位置".to_string())?;
    let local_display = local.display().to_string();

    let commands = format!(
        "get {} {}\n",
        sftp_quote(&remote_path),
        sftp_quote(&local_display)
    );
    let child = spawn_sftp_transfer(state.inner(), window.label(), &server, &control, &commands)?;

    // 轮询本地文件大小 → 进度事件；传输完成后再核 sftp 退出码
    let emit_app = app.clone();
    let window_label = window.label().to_string();
    let poll_name = filename.clone();
    std::thread::spawn(move || {
        let mut last_bytes: u64 = 0;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let bytes = std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0);
            if bytes != last_bytes {
                last_bytes = bytes;
                emit_progress(
                    &emit_app,
                    &window_label,
                    json!({ "name": poll_name, "dir": "down", "bytes": bytes, "total": total }),
                );
            }
            let status = child.lock().ok().and_then(|mut c| c.try_wait().ok().flatten());
            match status {
                Some(status) => {
                    if status.success() {
                        emit_progress(
                            &emit_app,
                            &window_label,
                            json!({ "name": poll_name, "dir": "down", "bytes": total, "total": total, "done": true }),
                        );
                    } else {
                        emit_progress(
                            &emit_app,
                            &window_label,
                            json!({ "name": poll_name, "dir": "down", "failed": true }),
                        );
                    }
                    return;
                }
                None => continue,
            }
        }
    });
    Ok(local_display)
}

/// 上传：系统多选对话框选本机文件 → 逐个 put 到远端目录。进度为不确定态
/// （bytes=0,total=0，前端画旋转），完成/失败各发一条事件。
#[tauri::command]
pub async fn hub_sftp_upload(
    app: AppHandle,
    window: tauri::WebviewWindow,
    server: HubServer,
    remote_dir: String,
) -> Result<Vec<String>, String> {
    let server = server.normalize();
    if server.id.is_empty() {
        return Err("该服务器还没保存档案，无法复用认证连接".to_string());
    }
    let control = crate::hub_tunnel::control_path(&app, &server.id)?;
    if !control.exists() {
        return Err("先打开一次终端完成认证，再用文件浏览".to_string());
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
        emit_progress(
            &app,
            &window_label,
            json!({ "name": name, "dir": "up", "bytes": 0, "total": 0 }),
        );
        let result = run_sftp_batch(
            &app,
            &server,
            &format!("put {} {}\n", sftp_quote(&local), sftp_quote(&remote)),
        );
        match result {
            Ok(_) => {
                uploaded.push(name.clone());
                emit_progress(
                    &app,
                    &window_label,
                    json!({ "name": name, "dir": "up", "done": true }),
                );
            }
            Err(error) => {
                emit_progress(
                    &app,
                    &window_label,
                    json!({ "name": name, "dir": "up", "failed": true }),
                );
                return Err(error);
            }
        }
    }
    Ok(uploaded)
}

/// 应用退出：收掉所有还在跑的传输子进程（不分窗口）
pub fn kill_all(app: &AppHandle) {
    if let Some(state) = app.try_state::<HubSftpState>() {
        if let Ok(mut list) = state.running.lock() {
            for (_, handle) in list.drain(..) {
                if let Ok(mut child) = handle.lock() {
                    let _ = child.kill();
                }
            }
        }
    }
}

/// server-files 窗口销毁：收掉它名下还在跑的传输子进程
pub fn kill_window(app: &AppHandle, label: &str) {
    if let Some(state) = app.try_state::<HubSftpState>() {
        if let Ok(mut list) = state.running.lock() {
            list.retain(|(w, handle)| {
                if w != label {
                    return true;
                }
                if let Ok(mut child) = handle.lock() {
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

        // 软链接行（名字含 " -> target"，保留可读）
        let link =
            parse_ls_line("lrwxrwxrwx 1 root root 11 Oct  2 10:00 latest -> v1.2.3").unwrap();
        assert!(link.link);
        assert_eq!(link.name, "latest -> v1.2.3");

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
    fn month_map() {
        assert_eq!(month_num("Oct"), Some("10"));
        assert_eq!(month_num("oct"), None);
    }
}
