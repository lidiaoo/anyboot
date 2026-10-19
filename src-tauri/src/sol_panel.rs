//! # sol_panel.rs —— SOL 远程唤醒标签页（sol 接收器管理）
//!
//! sol 是独立的 Wake-on-LAN 魔法包**接收器**（本项目 fork 自 bavix/sol 并扩展），
//! 以 systemd 服务（Windows 计划任务 / macOS LaunchDaemon）常驻。本模块只做
//! 「面向人的探针 + 维护动作」，不把 sol 拉进 AnyBoot 进程内：
//!
//! - 运行状态：服务单元是否 loaded/active、进程是否在跑；
//! - 监听端口：sol 只监听 UDP，读 `ss -ulnp`（Linux）/ `netstat -ano -p UDP`
//!   （Windows）里属于 sol 进程的行；
//! - 配置文件：以 systemd ExecStart 里 `--config` 指向的为准（再退到惯例路径），
//!   读写都走这里 —— 界面上改完写回原文件，sol 自己 watch 秒级生效；
//! - 下载安装包：sol_install_download 用 curl/wget 拉 GitHub release 的对应
//!   平台压缩包（Windows 用 PowerShell Invoke-WebRequest，其余平台 curl → wget
//!   依次退），落盘到用户设置的安装目录，不引 HTTP 客户端 crate；
//! - 解压：Linux/macOS tar -xzf（官方 release 是 tar.gz，系统自带 tar），
//!   Windows tar.exe（Win10+ 内置）同样处理，zip 包退 unzip / Expand-Archive；
//! - 安装 / 卸载：写好二进制 + 配置 + 服务单元一条龙（见 sol_install 脚本），
//!   非 root 时经 pkexec / UAC 提权（与 grub_install.rs 同一条路）；
//! - 打开安装目录：open_directory_in_explorer（lib.rs 的跨平台入口）。

use std::io::{BufReader, Read as _};

use super::*;

/// 当前进程是否已有管理员/	root 权限（三平台）：
/// Linux/macOS 看 euid；Windows 看 TokenElevation。
fn sol_is_elevated() -> bool {
    #[cfg(target_os = "linux")]
    {
        is_root()
    }
    #[cfg(target_os = "macos")]
    {
        // macOS 上 sol 的落点（/usr/local）当前用户通常可写；需要真正提权的
        // 动作（写 /etc、launchctl）走 osascript administrator privileges，
        // 这里保守返回 false 让调用方走提权分支。
        false
    }
    #[cfg(target_os = "windows")]
    {
        // 提权判定交给调用方的 Windows 分支（UAC 由官方 install.cmd 承担），
        // 未接 TokenElevation：一律按非提权处理。
        false
    }
}

/// 配置文件等路径非 root 读不到时的提权读取（Linux：pkexec cat；macOS/Windows
/// 当前无提权读方案，直读失败就报原文错误）。只用于读；内容原样返回。
fn privileged_read(path: &Path) -> io::Result<String> {
    if sol_is_elevated() {
        return fs::read_to_string(path);
    }
    #[cfg(target_os = "linux")]
    {
        let output = command("pkexec")
            .arg("cat")
            .arg(path)
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("提权读取被拒：{}", stderr.trim().chars().take(120).collect::<String>()),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "该平台暂无提权读取方案（可手工把文件权限放开后重试）",
        ))
    }
}

/// 服务名：Linux systemd 单元名（Windows 计划任务 / macOS 服务同名）。
const SOL_SERVICE: &str = "sol";
/// sol 二进制的惯例安装位置（install.sh 的默认落点）。
const SOL_BIN: &str = "/usr/local/bin/sol";
/// 配置文件的惯例搜索路径（与 sol 自身发现顺序一致，ExecStart 优先）。
const SOL_CONFIG_CANDIDATES: [&str; 2] = ["/etc/sol/sol.yaml", "/etc/sol/sol.yml"];
/// sol 官方 release 下载基址（fork 仓库的 releases 下载路径，资产名带版本号）。
/// 注意：`releases/latest/download/sol-<platform>.tar.gz` 这种不带版本号的
/// 直链在本仓库是 404 —— 资产命名是 sol-vX.Y.Z-<platform>（见 expanded_assets）。
const SOL_RELEASE_URL: &str = "https://github.com/lidiaoo/sol/releases/download";
/// 当前代码支持的 release 平台目录名（与官方包名 sol-vX.Y.Z-<平台> 一致）。
const SOL_RELEASE_TARGET: &str = if cfg!(target_os = "windows") {
    "windows-amd64"
} else if cfg!(target_os = "macos") {
    if cfg!(target_arch = "aarch64") {
        "darwin-arm64"
    } else {
        "darwin-amd64"
    }
} else if cfg!(target_arch = "aarch64") {
    "linux-arm64"
} else {
    "linux-amd64"
};
/// 版本号（不带 v 前缀）：发布 sol 面板时跟着仓库 tag 更新这一行。
const SOL_RELEASE_VERSION: &str = "0.1.0";

/// release 资产文件名：Windows 官方发 .zip，其余平台 .tar.gz（release.sh 命名）。
fn sol_asset_name() -> String {
    if cfg!(target_os = "windows") {
        format!("sol-v{SOL_RELEASE_VERSION}-{SOL_RELEASE_TARGET}.zip")
    } else {
        format!("sol-v{SOL_RELEASE_VERSION}-{SOL_RELEASE_TARGET}.tar.gz")
    }
}

/// 一条监听端口信息。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SolListenEntry {
    /// UDP 端口号
    pub(crate) port: u16,
    /// 来源行（ss / netstat 的原始行，留作证据）
    pub(crate) source: String,
}

/// sol 面板的完整状态快照（sol_status 命令返回值）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SolStatus {
    /// 二进制是否存在
    pub(crate) installed: bool,
    /// 二进制路径（存在才有）
    pub(crate) bin_path: Option<String>,
    /// `sol --version` 的第一行（存在且能跑才有）
    pub(crate) version: Option<String>,
    /// 服务单元是否被 systemd 加载（enabled/active 与否都算「有这个服务」）
    pub(crate) service_loaded: bool,
    /// 服务是否 active(running)
    pub(crate) service_active: bool,
    /// 服务自述（systemctl show 的 ExecStart 摘要 / 直跑进程命令行）
    pub(crate) service_detail: Option<String>,
    /// 是否探测到 sol 进程在跑（与 service_active 互补：手动直跑也算）
    pub(crate) process_running: bool,
    /// 监听中的 UDP 端口
    pub(crate) listening: Vec<SolListenEntry>,
    /// 正在生效的配置文件路径（ExecStart 指向的优先）
    pub(crate) config_path: Option<String>,
    /// 配置文件是否存在
    pub(crate) config_exists: bool,
    /// 最近的日志行（journalctl -u sol -n 15，取不到就为空）
    pub(crate) recent_logs: Vec<String>,
}

/// 读 sol 状态快照（不改任何东西）。任何一步失败都降级成「未知」，绝不让整条命令报错 ——
/// sol 没装、systemctl 不在、ss 不在，界面都还要能渲染出「未安装 / 未知」。
pub(crate) fn sol_status_snapshot() -> SolStatus {
    let mut status = SolStatus {
        installed: false,
        bin_path: None,
        version: None,
        service_loaded: false,
        service_active: false,
        service_detail: None,
        process_running: false,
        listening: Vec::new(),
        config_path: None,
        config_exists: false,
        recent_logs: Vec::new(),
    };

    // 1) 二进制：惯例位置 → PATH。找到了就顺带问一次 --version。
    let bin = [PathBuf::from(SOL_BIN)]
        .into_iter()
        .chain(
            std::env::var("PATH")
                .unwrap_or_default()
                .split(':')
                .filter(|part| !part.is_empty())
                .map(PathBuf::from)
                .map(|dir| dir.join(SOL_SERVICE)),
        )
        .find(|candidate| candidate.is_file());
    if let Some(bin) = bin {
        status.installed = true;
        status.bin_path = Some(bin.display().to_string());
        if let Ok(output) = command(bin.to_str().unwrap_or(SOL_BIN))
            .arg("--version")
            .stderr(Stdio::piped())
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if let Some(first) = text.lines().next() {
                    status.version = Some(first.trim().to_string());
                }
            }
        }
    }

    // 2) 服务状态：systemctl is-active / is-enabled（Linux 专属；Windows/macOS
    //    计划任务与 LaunchDaemon 的探针各平台后续补，这里先按 Linux 走）。
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = command("systemctl")
            .args(["is-active", SOL_SERVICE])
            .stderr(Stdio::piped())
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            status.service_loaded = !text.is_empty() && text != "inactive";
            status.service_active = text == "active";
            if status.service_active {
                status.process_running = true;
            }
        }
        if let Ok(output) = command("systemctl")
            .args(["show", SOL_SERVICE, "--property=ExecStart", "--no-pager"])
            .stderr(Stdio::piped())
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = text.lines().next() {
                // ExecStart={ path=/usr/local/bin/sol ; argv[]=/usr/local/bin/sol listen --config /path/sol.yaml ; ... }
                status.service_detail = Some(line.trim().to_string());
                if let Some(config) = exec_start_config(line.trim()) {
                    status.config_path = Some(config);
                }
            }
        }
        status.recent_logs = journal_tail();
    }

    // 3) 进程兜底：服务没算 active 的话，再看有没有叫 sol 的进程（手动直跑）。
    if !status.process_running {
        status.process_running = sol_process_alive();
    }

    // 4) 监听端口
    status.listening = sol_listening_ports(status.process_running);

    // 5) 配置文件：ExecStart 指向的 → 惯例路径 → 均无
    if status.config_path.is_none() {
        if let Ok(env_path) = std::env::var("SOL_CONFIG") {
            if !env_path.trim().is_empty() {
                status.config_path = Some(env_path);
            }
        }
    }
    if status.config_path.is_none() {
        for candidate in SOL_CONFIG_CANDIDATES {
            if Path::new(candidate).is_file() {
                status.config_path = Some(candidate.to_string());
                break;
            }
        }
    }
    if let Some(path) = status.config_path.as_deref() {
        status.config_exists = Path::new(path).is_file();
    }
    status
}

/// 从 systemctl show 的 ExecStart 行里抠出 `--config <path>`。
fn exec_start_config(line: &str) -> Option<String> {
    // argv[]=/usr/local/bin/sol listen --config /path/sol.yaml
    let argv = line.split("argv[]=").nth(1)?;
    let mut parts = argv.split_whitespace().peekable();
    while let Some(part) = parts.next() {
        let part = part.trim_end_matches(';');
        if part == "--config" {
            if let Some(value) = parts.next() {
                return Some(value.trim_end_matches(';').to_string());
            }
        }
        // 也支持 --config=path 形态
        if let Some(rest) = part.strip_prefix("--config=") {
            return Some(rest.trim_end_matches(';').to_string());
        }
    }
    None
}

/// journalctl 最近 15 行（`-o cat` 只取正文，不要时间戳前缀 —— 界面窄）。
#[cfg(target_os = "linux")]
fn journal_tail() -> Vec<String> {
    let Ok(output) = command("journalctl")
        .args(["-u", SOL_SERVICE, "-n", "15", "-o", "cat", "--no-pager"])
        .stderr(Stdio::piped())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take(160).collect())
        .collect()
}

/// 是否有名为 sol 的进程在跑（pgrep -x sol）。
fn sol_process_alive() -> bool {
    command("pgrep")
        .args(["-x", SOL_SERVICE])
        .stderr(Stdio::piped())
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 读 sol 进程正在监听的 UDP 端口。
///
/// Linux 走 `ss -ulnp`：匹配行里 `pid=1252` 或进程名 `sol`；Windows 走
/// `netstat -ano -p UDP`：先拿 sol 的 PID 再按最后一列 PID 匹配。
fn sol_listening_ports(process_running: bool) -> Vec<SolListenEntry> {
    let mut entries = Vec::new();
    let _ = process_running; // 只决定要不要白跑；端口读不出来就空着

    // 本机 sol 进程 PID（pgrep 一次，两个平台都用）
    let pid: Option<String> = command("pgrep")
        .args(["-x", SOL_SERVICE])
        .stderr(Stdio::piped())
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|text| !text.is_empty());

    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = command("ss").args(["-ulnp"]).stderr(Stdio::piped()).output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines().skip(1) {
                let is_sol = line.contains("sol")
                    || pid.as_deref().map(|pid| line.contains(pid)).unwrap_or(false);
                if !is_sol {
                    continue;
                }
                // Local Address:Port 列形如 *:11 或 0.0.0.0:8080
                let port = line.split_whitespace().nth(4).and_then(|addr| {
                    addr.rsplit(':')
                        .next()
                        .and_then(|port| port.parse::<u16>().ok())
                });
                if let Some(port) = port {
                    if !entries.iter().any(|entry: &SolListenEntry| entry.port == port) {
                        entries.push(SolListenEntry {
                            port,
                            source: line.chars().take(120).collect(),
                        });
                    }
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(pid) = pid {
            if let Ok(output) = command("netstat")
                .args(["-ano", "-p", "UDP"])
                .stderr(Stdio::piped())
                .output()
            {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    let cols: Vec<&str> = line.split_whitespace().collect();
                    // Proto Local-Address Foreign-Address PID
                    if cols.len() >= 4 && cols[0].eq_ignore_ascii_case("UDP") && cols[3] == pid {
                        if let Some(port) = cols[1].rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
                            entries.push(SolListenEntry {
                                port,
                                source: line.chars().take(120).collect(),
                            });
                        }
                    }
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.port);
    entries.dedup_by(|a, b| a.port == b.port);
    entries
}

/// 读配置文件正文。默认读「正在生效的那份」；body 传 true 时直接读。
#[tauri::command]
pub(crate) fn sol_status() -> SolStatus {
    sol_status_snapshot()
}

/// 读配置文件正文（path 为空 = 读正在生效的那份）。
///
/// /etc/sol/sol.yaml 是 root 的 0600（实测），非 root 直读必败：先直读，
/// 权限类失败走 `pkexec cat` 提权读（用户会看到 polkit 弹窗）。
#[tauri::command]
pub(crate) fn sol_config_read(path: Option<String>) -> Result<SolConfigRead, String> {
    let resolved = match path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(path) => path.to_string(),
        None => sol_status_snapshot()
            .config_path
            .ok_or_else(|| "找不到 sol 配置文件（服务没在跑，/etc/sol/ 下也没有）".to_string())?,
    };
    let target = PathBuf::from(&resolved);
    let content = match fs::read_to_string(&target) {
        Ok(content) => content,
        Err(error) if matches!(
            error.kind(),
            io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
        ) =>
        {
            // 权限不够（0600 root）→ 提权读；NotFound 也走一遍（可能是目录层
            // 没有执行位，直读的 errno 一样报 perm）
            privileged_read(&target).map_err(|inner| {
                format!(
                    "读取 {resolved} 失败：{error}；提权读取也失败：{inner}"
                )
            })?
        }
        Err(error) => return Err(format!("读取 {resolved} 失败：{error}")),
    };
    Ok(SolConfigRead {
        path: resolved,
        content,
    })
}

/// 配置读结果。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SolConfigRead {
    pub(crate) path: String,
    pub(crate) content: String,
}

/// 写配置文件：先备份成 `<原名>.bak-anyboot`（存在就覆盖旧备份），再原子写。
///
/// /etc/sol/ 是 root 的：非 root 时整条（备份+写入+落位+chmod）包进一个
/// `/bin/sh -c` 经 `pkexec` 提权执行，一次弹窗完成全部落盘。写完不用动
/// sol —— 它自己 watch 配置变化，秒级生效。
#[tauri::command]
pub(crate) fn sol_config_write(path: String, content: String) -> Result<String, String> {
    let path = path.trim().to_string();
    if path.is_empty() {
        return Err("配置文件路径为空".to_string());
    }
    let target = PathBuf::from(&path);
    if !target.is_file() {
        return Err(format!("{path} 不存在，先在 sol 侧安装好再编辑"));
    }

    // 非 root：内容经临时文件（用户可写目录）交给提权 shell 去落位
    if !sol_is_elevated() {
        let staging = std::env::temp_dir().join(format!(
            "anyboot-sol-config-{}.yaml",
            std::process::id()
        ));
        fs::write(&staging, content.as_bytes())
            .map_err(|error| format!("写暂存文件失败：{error}"))?;
        let script = format!(
            "set -e\ninstall -m 600 '{p}' '{p}.bak-anyboot'\ninstall -m 600 '{s}' '{p}'\nrm -f '{s}'\n",
            p = path,
            s = staging.display(),
        );
        let script_file = std::env::temp_dir().join(format!("anyboot-sol-cfg-{}.sh", std::process::id()));
        fs::write(&script_file, &script).map_err(|error| format!("写提权脚本失败：{error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&script_file, fs::Permissions::from_mode(0o755));
        }
        let output = command("pkexec")
            .arg("sh")
            .arg(&script_file)
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("启动提权写入失败：{error}"))?;
        let _ = fs::remove_file(&script_file);
        let _ = fs::remove_file(&staging);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "写入 {path} 失败（提权被拒或出错）：{}",
                stderr.trim().chars().take(200).collect::<String>()
            ));
        }
        return Ok(format!(
            "已写入 {path}（原文件备份为 {path}.bak-anyboot）。sol 会自动热重载，无需重启服务"
        ));
    }

    // root：本进程直接备份 + 原子写
    let backup = target.with_extension("yaml.bak-anyboot");
    fs::copy(&target, &backup)
        .map_err(|error| format!("备份到 {} 失败：{error}", backup.display()))?;
    // 原子写：写临时文件再 rename，写一半断电也不会把配置留成半截
    let tmp = target.with_extension("yaml.tmp-anyboot");
    fs::write(&tmp, content.as_bytes())
        .map_err(|error| format!("写入临时文件失败：{error}"))?;
    fs::rename(&tmp, &target).map_err(|error| format!("落盘失败：{error}"))?;
    Ok(format!(
        "已写入 {path}（原文件备份为 {}）。sol 会自动热重载，无需重启服务",
        backup.display()
    ))
}

/// 安装 sol 的默认下载目录（用户没设置时）：三平台统一 **用户主目录下的 sol**
/// （Linux/macOS ~/sol，Windows %USERPROFILE%\sol）。用户目录一定可写，下载/
/// 解压不需要提权；提权只发生在「安装」（写 /usr/local/bin、/etc、注册服务）
/// 那一步 —— 权限边界与动作语义一一对应。
pub(crate) fn sol_download_dir_default() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        // %USERPROFILE% = C:\Users\<name>
        std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("LOCALAPPDATA"))
            .map(|home| PathBuf::from(home).join(SOL_SERVICE))
            .unwrap_or_else(|_| PathBuf::from(r"C:\sol"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        // HOME 在 Linux/macOS 都是主目录；兜底 /tmp/sol 保底不炸（仅极端无 HOME 环境）
        match std::env::var("HOME") {
            Ok(home) if !home.trim().is_empty() => {
                PathBuf::from(home).join(SOL_SERVICE)
            }
            _ => PathBuf::from("/tmp/sol"),
        }
    }
}

/// 下载 sol 安装包到指定目录（dir 为空 = 默认目录）。
///
/// 流程：目录不存在就建 → 落盘名为 sol-<platform>.tar.gz（Windows 同样是 tar.gz，
/// 官方 release 的 Windows 包也是 tar.gz；zip 只是历史兼容）→ 用系统自带的
/// curl → wget（Windows 走 PowerShell Invoke-WebRequest）下载，带
/// `-L` 跟随 GitHub 的 302 重定向 → 校验文件非空 → 返回落盘绝对路径。
/// 不做断点续传（安装包就几 MB，失败重下比续传省心）。
#[tauri::command]
pub(crate) async fn sol_install_download(
    app: AppHandle,
    dir: Option<String>,
) -> Result<String, String> {
    trace(&app, "command", "sol_install_download");
    let dir = match dir.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => expand_home_dir(value),
        None => sol_download_dir_default(),
    };
    fs::create_dir_all(&dir).map_err(|error| format!("创建下载目录 {} 失败：{error}", dir.display()))?;

    // 官方资产名带版本号（sol-v0.1.0-linux-amd64.tar.gz），Windows 是 zip：
    // 后缀由 sol_asset_name() 按平台给出，与 scripts/release.sh 的命名一致。
    let asset = sol_asset_name();
    let url = format!("{SOL_RELEASE_URL}/v{SOL_RELEASE_VERSION}/{asset}");
    let target = dir.join(&asset);
    sol_log(&app, format!("开始下载 {url}"));

    // 已有同名包就先删（避免旧的半截包被误当新包用）
    let _ = fs::remove_file(&target);

    // ===== 进度化下载 =====
    // GitHub 会 302 到 release-assets CDN：先 HEAD 拿最终 Content-Length（总字节），
    // 再用 curl 的进度回调（--progress-bar 输出在 stderr，% 形如 "## 34.5%"）。
    // 解析 stderr 里的百分比推事件，前端画进度条；解析不出来就按「已落盘字节数
    // / 总字节」兜底轮询。
    let total = sol_head_content_length(&url);
    if let Some(total) = total {
        sol_log(&app, format!("包大小 {total} 字节（HEAD 探测）"));
    } else {
        sol_log(&app, "未能从 HEAD 拿到包大小，进度条按已落盘字节显示".to_string());
    }

    let (child, _is_curl) = spawn_download_child(&url, &target)?;

    // 轮询收集：curl 的 stderr 流（百分比）+ 落盘文件大小，推 sol-download-progress
    let app_for_watch = app.clone();
    let target_for_watch = target.clone();
    let progress_handle = std::thread::spawn(move || {
        watch_download_progress(app_for_watch, child, _is_curl, target_for_watch, total)
    });

    let output = progress_handle
        .join()
        .map_err(|_| "下载监视线程崩溃".to_string())?;

    if !output.ok {
        let _ = fs::remove_file(&target);
        sol_log(&app, format!("下载失败：{}", output.message));
        return Err(output.message);
    }
    // 空包也是失败（GitHub 偶发 404 时空文件 + exit 0 的组合见过）
    let size = fs::metadata(&target)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if size == 0 {
        let _ = fs::remove_file(&target);
        let message = format!(
            "下载得到的 {asset} 是空文件（release 里可能没有 {SOL_RELEASE_TARGET} 的包），请确认 GitHub release 资产"
        );
        sol_log(&app, message.clone());
        return Err(message);
    }
    sol_log(&app, format!("下载完成：{}（{} 字节）", target.display(), size));
    Ok(target.display().to_string())
}

/// 下载结果（监视线程 → 命令体）。
struct DownloadOutput {
    ok: bool,
    message: String,
}

/// HEAD 请求拿 Content-Length（跟随重定向后的最终大小）。失败返回 None ——
/// 进度条退化为「已落盘字节」显示，不阻塞下载。
fn sol_head_content_length(url: &str) -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        // PowerShell HEAD：Content-Type 判空返回长度
        let output = command("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "[Net.ServicePointManager]::SecurityProtocol='Tls12'; \
                     $r=[System.Net.HttpWebRequest]::Create('{url}'); \
                     $r.Method='HEAD'; $r.UserAgent='anyboot'; \
                     $resp=$r.GetResponse(); $resp.ContentLength; $resp.Close()"
                ),
            ])
            .stderr(Stdio::piped())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout).trim().parse::<u64>().ok()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let curl = command("curl")
            .args(["-sIL", "-o", "/dev/null", "-w", "%{size_download}"])
            .arg(url)
            .stderr(Stdio::piped())
            .output()
            .ok()?;
        if !curl.status.success() {
            return None;
        }
        // -w size_download 对 HEAD 给 0：改用解析头。curl -sIL 输出里取最后一个
        // content-length（重定向链的最后一跳才是 CDN 上的真实大小）
        let output = command("curl")
            .args(["-sIL"])
            .arg(url)
            .stderr(Stdio::piped())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.to_ascii_lowercase().starts_with("content-length:"))
            .filter_map(|line| {
                line.split(':')
                    .nth(1)
                    .and_then(|value| value.trim().parse::<u64>().ok())
            })
            .last()
            .filter(|size| *size > 0)
    }
}

/// 起下载子进程（不等待）：Linux/macOS curl → wget；Windows PowerShell。
/// 返回子进程 + 是不是 curl（决定 stderr 里百分比行的解析方式）。
fn spawn_download_child(url: &str, target: &Path) -> Result<(std::process::Child, bool), String> {
    #[cfg(target_os = "windows")]
    {
        let child = command("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "[Net.ServicePointManager]::SecurityProtocol='Tls12'; Invoke-WebRequest -Uri '{url}' -OutFile '{}' -UserAgent 'anyboot'",
                    target.display()
                ),
            ])
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("启动 PowerShell 下载失败：{error}"))?;
        Ok((child, false))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let curl = command("curl")
            .args(["-fSL", "--connect-timeout", "15", "--max-time", "600", "-o"])
            .arg(target)
            .arg(url)
            .stderr(Stdio::piped())
            .spawn();
        match curl {
            Ok(child) => Ok((child, true)),
            Err(_) => {
                let child = command("wget")
                    .args(["-q", "-O"])
                    .arg(target)
                    .arg(url)
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|error| format!("curl 与 wget 都不可用：{error}"))?;
                Ok((child, false))
            }
        }
    }
}

/// 盯着下载子进程：读 curl 的 stderr（百分比行），同时每 500ms 量一次落盘
/// 文件大小，推 `sol-download-progress` 事件；进程退出后返回结果。
fn watch_download_progress(
    app: AppHandle,
    mut child: std::process::Child,
    _is_curl: bool,
    target: PathBuf,
    total: Option<u64>,
) -> DownloadOutput {
    // stderr 读挪进独立线程：reader.read() 是阻塞读，curl 没输出时会挂死整个
    // 轮询循环（子进程退出检测、文件量尺寸全停）——一键下载「卡死」的根因。
    // 线程只管往共享 buffer 里塞字节，主循环照常每 500ms 醒来。
    let stderr = child.stderr.take();
    let reader_handle = stderr.map(|stream| {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut collected: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        collected.extend_from_slice(&chunk[..read]);
                        if collected.len() > 256 * 1024 {
                            collected.drain(..collected.len() - 8192);
                        }
                    }
                }
            }
            collected
        })
    });

    let mut last_bytes: u64 = 0;
    loop {
        // 1) 子进程还活着吗
        match child.try_wait() {
            Ok(Some(status)) => {
                push_progress(&app, last_bytes, total, true, status.success());
                if status.success() {
                    return DownloadOutput { ok: true, message: String::new() };
                }
                // curl 失败 → 前端体感是「下载中断」；wget 退路在 spawn 阶段已经走过
                return DownloadOutput {
                    ok: false,
                    message: format!(
                        "下载子进程退出（exit {}）",
                        status.code().unwrap_or(-1)
                    ),
                };
            }
            Ok(None) => {}
            Err(error) => {
                return DownloadOutput { ok: false, message: format!("等待下载进程失败：{error}") };
            }
        }

        // 2) 落盘字节数（进度事件的主数据源；curl 的百分比解析不再依赖——
        //    文件大小 + total 就是完整的进度信息，stderr 线程只为保管道不堵）
        if let Ok(meta) = fs::metadata(&target) {
            last_bytes = meta.len();
        }
        push_progress(&app, last_bytes, total, false, false);

        // 3) 睡一拍再来
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// 推一条下载进度事件（前端 sol-download-progress 监听）。
fn push_progress(app: &AppHandle, bytes: u64, total: Option<u64>, done: bool, ok: bool) {
    let payload = serde_json::json!({
        "bytes": bytes,
        "total": total,
        "done": done,
        "ok": ok,
    });
    let _ = app.emit("sol-download-progress", payload);
}

/// 追加一条 sol 面板日志（事件推前端 + trace 落盘）。
fn sol_log(app: &AppHandle, message: impl Into<String>) {
    let message = message.into();
    trace(app, "sol", &message);
    let _ = app.emit(
        "sol-log",
        serde_json::json!({
            "time": now_string(),
            "message": message,
        }),
    );
}

/// 本地时间串（HH:MM:SS）：日志行的前缀。
fn now_string() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    // UTC +8 当本地（部署环境是上海时区；日志用途，不引入 chrono）
    let seconds = seconds + 8 * 3600;
    let h = (seconds / 3600) % 24;
    let m = (seconds / 60) % 60;
    let s = seconds % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// 解压 sol 安装包（tar.gz / zip）到指定目录，返回解出来的根目录。
///
/// 根目录的判定：tar 包里顶层只有一个 `sol-vX.Y.Z-<platform>/` 目录，解完
/// 直接取它；zip 同理。解出来的目录里必须能找到 sol 二进制，否则报错。
#[tauri::command]
pub(crate) async fn sol_extract(app: AppHandle, archive: String, dir: Option<String>) -> Result<String, String> {
    trace(&app, "command", "sol_extract");
    sol_log(&app, format!("解压 {} → {}", archive, dir.clone().unwrap_or_default()));
    let archive = PathBuf::from(archive.trim());
    if !archive.is_file() {
        return Err(format!("{} 不存在，先下载安装包", archive.display()));
    }
    let dir = match dir.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => expand_home_dir(value),
        None => archive
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(sol_download_dir_default),
    };
    fs::create_dir_all(&dir).map_err(|error| format!("创建解压目录失败：{error}"))?;

    // tar.exe（Win10+ 内置）与 GNU tar 都认 -xzf；zip 退 unzip / Expand-Archive
    let output = if archive
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
    {
        if cfg!(target_os = "windows") {
            command("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    &format!(
                        "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                        archive.display(),
                        dir.display()
                    ),
                ])
                .stderr(Stdio::piped())
                .output()
                .map_err(|error| format!("启动 PowerShell 解压失败：{error}"))?
        } else {
            command("unzip")
                .args(["-o"])
                .arg(&archive)
                .arg("-d")
                .arg(&dir)
                .stderr(Stdio::piped())
                .output()
                .map_err(|error| format!("unzip 不可用：{error}"))?
        }
    } else {
        command("tar")
            .args(["-xzf"])
            .arg(&archive)
            .arg("-C")
            .arg(&dir)
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("tar 不可用：{error}"))?
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "解压 {} 失败：{}",
            archive.display(),
            stderr.trim().chars().take(300).collect::<String>()
        ));
    }

    // 找解出来的根目录：直接命中 / 顶层唯一子目录 / 解压目录本身
    let root = sol_extracted_root(&dir).ok_or_else(|| {
        format!(
            "解压完成，但 {} 下找不到含 sol 二进制的目录",
            dir.display()
        )
    })?;
    Ok(root.display().to_string())
}

/// 在解压目录里找「根」：先看 dir 本身有没有 sol 二进制，再看唯一子目录。
fn sol_extracted_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("sol").is_file() {
        return Some(dir.to_path_buf());
    }
    let mut subdirs: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    subdirs.sort();
    // 顶层唯一子目录 → 就是官方包的 sol-vX.Y.Z-<platform>/
    if subdirs.len() == 1 && subdirs[0].join("sol").is_file() {
        return Some(subdirs.pop().expect("len checked above"));
    }
    // 目录名带 sol 前缀的优先（多包共存时不抓错）
    subdirs
        .into_iter()
        .find(|path| {
            path.file_name()
                .map(|name| {
                    let name = name.to_string_lossy().to_lowercase();
                    name.starts_with("sol")
                })
                .unwrap_or(false)
                && path.join("sol").is_file()
        })
        .or_else(|| {
            subdirs_last(dir)
        })
}

/// 最后的兜底：任意子目录里含 sol 二进制的第一个。
fn subdirs_last(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("sol").is_file())
        .collect();
    entries.sort();
    entries.into_iter().next()
}

/// 当前默认安装目录（~/sol）：前端「安装目录」输入框的初始回显来源。
#[tauri::command]
pub(crate) fn sol_default_dir() -> String {
    sol_download_dir_default().display().to_string()
}

// ==================== HTTP 控制面：配置与选项双向联动 ====================
//
// sol.yaml 的 server.http 段：
//   http:
//     enabled: true
//     listen: 127.0.0.1:8080
//     auth: { type: bearer, token: "<随机串>" }
//
// 面板上拆成三个独立选项（HTTP 服务监听开关 / 监听地址端口 / Token），改动
// 写回 YAML、YAML 变化也读回面板 —— 单向真相都是配置文件本身（sol 自己
// watch 秒级热重载）。解析用「行级锚点 + 缩进匹配」而不是引 YAML crate：
// 项目里没有 serde_yaml 依赖，这份文件的结构也足够浅。

/// HTTP 控制面解析结果（sol_http_get 返回值）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SolHttp {
    pub(crate) enabled: bool,
    /// host:port（原样展示，如 127.0.0.1:8080）
    pub(crate) listen: String,
    /// bearer token 原文（token_file / token_env 模式下为空，前端提示改用手填）
    pub(crate) token: String,
    /// auth 的书写形态：token（内联）/ token_env / token_file / none
    pub(crate) auth_kind: String,
}

/// 找到 `http:` 块的起止行号（缩进 4 的 `  http:`，到下一个同级或更浅键为止）。
fn sol_yaml_http_span(content: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut start = None;
    let mut indent_of_http = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("http:") && trimmed.len() > 4 && !trimmed.starts_with("#") {
            indent_of_http = line.len() - trimmed.len();
            start = Some(index);
            break;
        }
    }
    let start = start?;
    let mut end = lines.len();
    for (offset, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent <= indent_of_http {
            end = offset;
            break;
        }
    }
    Some((start, end))
}

/// 在 http 块内找 `key: value` 行（缩进 = http 缩进 + 2）。
fn sol_yaml_http_value(content: &str, key: &str) -> Option<String> {
    let (start, end) = sol_yaml_http_span(content)?;
    let prefix = format!("{key}:");
    for line in content.lines().take(end).skip(start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue; // 注释里的示例键不算
        }
        if trimmed.starts_with(&prefix) {
            return Some(trimmed[prefix.len()..].trim().to_string());
        }
    }
    None
}

/// 解析 sol.yaml 的 http 段（没有 http 段 = 默认值 enabled=false）。
fn sol_parse_http(content: &str) -> SolHttp {
    let enabled = sol_yaml_http_value(content, "enabled")
        .map(|value| value == "true")
        .unwrap_or(false);
    let listen = sol_yaml_http_value(content, "listen").unwrap_or_default();
    // auth 行可能是 flow map：auth: { type: bearer, token: "..." }
    let mut token = String::new();
    let mut auth_kind = "none".to_string();
    if let Some(auth_line) = sol_yaml_http_value(content, "auth") {
        if let Some(token_value) = extract_flow_value(&auth_line, "token") {
            token = token_value;
            auth_kind = "token".to_string();
        } else if extract_flow_key(&auth_line, "token_env").is_some() {
            auth_kind = "token_env".to_string();
        } else if extract_flow_key(&auth_line, "token_file").is_some() {
            auth_kind = "token_file".to_string();
        }
    }
    SolHttp {
        enabled,
        listen,
        token,
        auth_kind,
    }
}

/// 从 flow map 原文 `{ type: bearer, token: "abc" }` 里取 `key:` 的值
/// （带引号会被剥掉）。
fn extract_flow_value(flow: &str, key: &str) -> Option<String> {
    let inner = flow.trim().trim_start_matches('{').trim_end_matches('}');
    for part in inner.split(',') {
        let part = part.trim();
        let prefix = format!("{key}:");
        if let Some(rest) = part.strip_prefix(&prefix) {
            let value = rest.trim();
            let value = value.trim_matches(|c| c == '"' || c == '\'');
            return Some(value.to_string());
        }
    }
    None
}

/// 只判断 flow map 里有没有某个键（token_env / token_file 探测用）。
fn extract_flow_key(flow: &str, key: &str) -> Option<()> {
    extract_flow_value(flow, key).map(|_| ())
}

/// 写回 http 段的单个键（enabled / listen），保持其它行不动。
/// 没有该键就在块尾补一行；没有 http 块就整个补一段。
fn sol_yaml_set_http_key(content: &str, key: &str, value: &str) -> String {
    let lines: Vec<String> = content.lines().map(String::from).collect();
    match sol_yaml_http_span(content) {
        Some((start, end)) => {
            // 缩进跟随文件现状：取 http 块内第一个真实键行的缩进，不写死 ——
            // 之前写死 6 空格把原本 4 空格的文件越写越歪（同级键缩进不一致 yaml
            // 直接拒收，sol 解析失败反复重启）。块内没有键行时退到 http: 缩进 + 2。
            let http_line = &lines[start];
            let http_indent_len = http_line.len() - http_line.trim_start().len();
            let indent = lines[start + 1..end]
                .iter()
                .find(|line| {
                    let trimmed = line.trim_start();
                    !trimmed.is_empty()
                        && !trimmed.starts_with('#')
                        && (line.len() - trimmed.len()) > http_indent_len
                })
                .map(|line| line[..line.len() - line.trim_start().len()].to_string())
                .unwrap_or_else(|| format!("{}  ", &http_line[..http_indent_len]));
            let prefix = format!("{key}:");
            let mut out: Vec<String> = Vec::new();
            let mut replaced = false;
            for (index, line) in lines.iter().enumerate() {
                let trimmed = line.trim_start();
                if index > start && index < end && trimmed.starts_with(&prefix) && !trimmed.starts_with("#") {
                    if !replaced {
                        out.push(format!("{indent}{key}: {value}"));
                        replaced = true;
                    }
                    // 重复键行删掉（理论不会有）
                    continue;
                }
                out.push(line.clone());
            }
            if !replaced {
                // 插到 http 块尾（end 之前的最后一个非空行后）
                let insert_at = out
                    .iter()
                    .rposition(|line| {
                        let index = out
                            .iter()
                            .position(|l| std::ptr::eq(l, line))
                            .unwrap_or(0);
                        index > start && index < end && !line.trim().is_empty()
                    })
                    .unwrap_or(start);
                out.insert(insert_at + 1, format!("{indent}{key}: {value}"));
            }
            let mut text = out.join("\n");
            if content.ends_with('\n') {
                text.push('\n');
            }
            text
        }
        None => {
            // 没有 http 块：插到 server: 下（server 段一定有 —— rules 之前）
            let mut out: Vec<String> = Vec::new();
            let mut inserted = false;
            for line in &lines {
                out.push(line.clone());
                if !inserted && line.trim() == "server:" {
                    out.push(format!("  http:"));
                    out.push(format!("    {key}: {value}"));
                    inserted = true;
                }
            }
            if !inserted {
                out.push("server:".to_string());
                out.push(format!("  http:"));
                out.push(format!("    {key}: {value}"));
            }
            let mut text = out.join("\n");
            if content.ends_with('\n') {
                text.push('\n');
            }
            text
        }
    }
}

/// 写回 auth 行（整体替换，因为 flow map 是一整行）。缩进跟随现有 auth 行。
fn sol_yaml_set_auth(content: &str, token: &str) -> String {
    let lines: Vec<String> = content.lines().map(String::from).collect();
    // 先找到现有 auth 行的缩进（替换行必须与文件其它键同级，写死会把文件写歪）
    let existing_indent = sol_yaml_http_span(content)
        .and_then(|(start, end)| {
            lines[start + 1..end]
                .iter()
                .find(|line| {
                    let trimmed = line.trim_start();
                    trimmed.starts_with("auth:") && !trimmed.starts_with("#")
                })
                .map(|line| line[..line.len() - line.trim_start().len()].to_string())
        })
        // 没有 auth 行时用与 sol_yaml_set_http_key 相同的推导（http 块内第一个键行）
        .or_else(|| {
            sol_yaml_http_span(content).and_then(|(start, end)| {
                let http_line = &lines[start];
                let http_indent_len = http_line.len() - http_line.trim_start().len();
                lines[start + 1..end]
                    .iter()
                    .find(|line| {
                        let trimmed = line.trim_start();
                        !trimmed.is_empty()
                            && !trimmed.starts_with('#')
                            && (line.len() - trimmed.len()) > http_indent_len
                    })
                    .map(|line| line[..line.len() - line.trim_start().len()].to_string())
            })
        })
        .unwrap_or_else(|| "    ".to_string());
    let auth_line = format!(
        "{indent}auth: {{ type: bearer, token: \"{token}\" }}",
        indent = existing_indent
    );
    match sol_yaml_http_span(content) {
        Some((start, end)) => {
            let mut out: Vec<String> = Vec::new();
            let mut replaced = false;
            for (index, line) in lines.iter().enumerate() {
                // 只认非注释的 auth: 行 —— 注释掉的示例行（# auth: {...}）要留着
                let trimmed = line.trim_start();
                if index > start
                    && index < end
                    && trimmed.starts_with("auth:")
                    && !trimmed.starts_with("#")
                {
                    if !replaced {
                        out.push(auth_line.clone());
                        replaced = true;
                    }
                    continue;
                }
                out.push(line.clone());
            }
            if !replaced {
                // 没有 auth 行：插到 http 块尾（end 之前的最后一个非空行之后），
                // 不能推到文件末尾 —— 那会落在 http 块外面，等于没配
                let insert_at = ((start + 1)..end)
                    .rev()
                    .find(|&index| !lines[index].trim().is_empty())
                    .unwrap_or(start);
                out.insert(insert_at + 1, auth_line);
            }
            let mut text = out.join("\n");
            if content.ends_with('\n') {
                text.push('\n');
            }
            text
        }
        None => sol_yaml_set_http_key(content, "auth", &format!("{{ type: bearer, token: \"{token}\" }}")),
    }
}

/// 读 HTTP 控制面配置（path 为空 = 读正在生效的那份）。
#[tauri::command]
pub(crate) fn sol_http_get(path: Option<String>) -> Result<SolHttp, String> {
    let resolved = sol_config_path_for(path)?;
    let content = read_sol_config(&resolved)?;
    Ok(sol_parse_http(&content))
}

/// 配置文件定位：显式路径 → 正在生效的那份。
fn sol_config_path_for(path: Option<String>) -> Result<String, String> {
    match path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(path) => Ok(path.to_string()),
        None => sol_status_snapshot()
            .config_path
            .ok_or_else(|| "找不到 sol 配置文件（服务没在跑，/etc/sol/ 下也没有）".to_string()),
    }
}

/// 读配置正文（自动提权降级）。
fn read_sol_config(resolved: &str) -> Result<String, String> {
    let target = PathBuf::from(resolved);
    match fs::read_to_string(&target) {
        Ok(content) => Ok(content),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
            ) =>
        {
            privileged_read(&target)
                .map_err(|inner| format!("读取 {resolved} 失败：{error}；提权读取也失败：{inner}"))
        }
        Err(error) => Err(format!("读取 {resolved} 失败：{error}")),
    }
}

/// 写 HTTP 控制面单项（option = enabled | listen | token；value 为新值）。
/// 写前重读文件 —— 两个选项连续改不互相覆盖（每次都以磁盘上的最新内容为底）。
#[tauri::command]
pub(crate) fn sol_http_set(
    app: AppHandle,
    path: Option<String>,
    option: String,
    value: String,
) -> Result<String, String> {
    trace(&app, "command", &format!("sol_http_set {option}"));
    let resolved = sol_config_path_for(path)?;
    let content = read_sol_config(&resolved)?;
    let updated = match option.as_str() {
        "enabled" => {
            let flag = matches!(value.as_str(), "true" | "1" | "on" | "yes");
            sol_yaml_set_http_key(&content, "enabled", if flag { "true" } else { "false" })
        }
        "listen" => {
            let value = value.trim();
            if !value.contains(':') {
                return Err(format!("监听地址要写 host:port（如 127.0.0.1:8080），收到「{value}」"));
            }
            sol_yaml_set_http_key(&content, "listen", value)
        }
        "token" => {
            let token = value.trim();
            if token.len() < 8 {
                return Err("token 至少 8 个字符（建议用「生成」出一串随机值）".to_string());
            }
            sol_yaml_set_auth(&content, token)
        }
        other => return Err(format!("不支持的选项：{other}（只认 enabled / listen / token）")),
    };
    // 复用 sol_config_write 的落盘与备份逻辑（含 pkexec 提权）
    sol_config_write(resolved, updated)
}

/// 生成一串随机 token（64 位十六进制，与 getrandom 复用同一来源）。
#[tauri::command]
pub(crate) fn sol_token_generate() -> String {
    use sha2::Digest as _;
    // 32 字节随机 → 64 hex；混入纳秒时间防极端重复
    let mut bytes = [0u8; 32];
    let _ = getrandom::fill(&mut bytes);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    bytes[0] ^= nanos as u8;
    bytes[1] ^= (nanos >> 8) as u8;
    hex::encode(bytes)
}

/// 展开 `~` / `~/xxx` 为绝对路径（用户在输入框里手填时用；后端拿到的
/// 可能就是这样的原文）。展开不了原样返回。
fn expand_home_dir(value: &str) -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .filter(|home| !home.trim().is_empty());
    // Result::filter 不存在：先 ok() 再 filter（上面已处理）
    let _ = &home;
    if value == "~" {
        return home.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        if let Some(home) = home {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(value)
}

/// 选择安装目录（下载/解压落点）：复用 hub_sftp 的文件夹选择框。
#[tauri::command]
pub(crate) async fn sol_pick_dir(
    app: AppHandle,
    current: Option<String>,
) -> Result<Option<String>, String> {
    let start = current
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut dialog = app.dialog().file().set_title("选择 sol 安装目录");
        if let Some(dir) = start {
            dialog = dialog.set_directory(&dir);
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

/// 用户手动选安装包（文件管理器选择框）：只认 .tar.gz / .tgz / .zip。
/// 选完不自动动它 —— 解压/安装由用户点按钮或一键流程继续。
#[tauri::command]
pub(crate) async fn sol_pick_archive(app: AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        app.dialog()
            .file()
            .set_title("选择 sol 安装包（.tar.gz / .zip）")
            .add_filter("sol 安装包", &["tar.gz", "tgz", "zip"])
            .pick_file(move |path| {
                let _ = tx.send(path.and_then(|p| p.into_path().ok()));
            });
        rx.recv().map_err(|_| "文件选择框被关闭".to_string())
    })
    .await
    .map_err(|error| format!("文件选择框执行失败: {error}"))?
    .map(|path| path.map(|path| path.display().to_string()))
}

/// 读取最近一次面板日志（前端进页面时回填日志区）。后端日志同时经
/// sol-log 事件实时推，这条命令只管「进页面时把历史一次取走」。
#[tauri::command]
pub(crate) fn sol_logs(app: AppHandle) -> Vec<String> {
    // 从 trace.log 里捞本进程启动以来的 sol 段（捞不到就空，前端不渲染）
    let Some(path) = trace_file_path(&app) else {
        return Vec::new();
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter(|line| line.contains("[sol]"))
        .map(|line| {
            // trace 行形如 "12:34:56 [sol] 开始下载 …"：去掉时间戳以外的前缀
            match line.split_once("[sol]") {
                Some((_, rest)) => rest.trim().to_string(),
                None => line.to_string(),
            }
        })
        .rev()
        .take(50)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// 一键下载安装：下载 → 解压 → 安装，全流程顺序执行，每一步都推日志。
/// 中途任何一步失败即停（已下载的包保留，重试时解压可直接复用）。
#[tauri::command]
pub(crate) async fn sol_one_click_install(app: AppHandle, dir: Option<String>) -> Result<String, String> {
    trace(&app, "command", "sol_one_click_install");
    sol_log(&app, "＝＝＝ 一键下载安装开始 ＝＝＝".to_string());

    // 1) 下载（进度条独立走事件）
    let archive = sol_install_download(app.clone(), dir).await.map_err(|error| {
        sol_log(&app, format!("一键安装中止（下载失败）：{error}"));
        error
    })?;
    sol_log(&app, format!("① 下载完成 {archive}"));

    // 2) 解压（解到包所在目录）
    let extract_dir = PathBuf::from(&archive)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(sol_download_dir_default);
    sol_log(&app, format!("② 开始解压到 {}", extract_dir.display()));
    let root = sol_extract(app.clone(), archive, Some(extract_dir.display().to_string()))
        .await
        .map_err(|error| {
            sol_log(&app, format!("一键安装中止（解压失败）：{error}"));
            error
        })?;
    sol_log(&app, format!("② 解压完成 {root}"));

    // 3) 安装（内部自带 pkexec 提权与日志）
    sol_log(&app, "③ 开始安装（可能弹出提权窗口）".to_string());
    let message = sol_install(app.clone(), Some(root)).await.map_err(|error| {
        sol_log(&app, format!("一键安装中止（安装失败）：{error}"));
        error
    })?;
    sol_log(&app, "＝＝＝ 一键下载安装完成 ＝＝＝".to_string());
    Ok(message)
}

/// 安装 sol：用官方 install.sh（Linux）——它自带交互问答，这里传「全部接受默认」
/// 是做不到的（脚本要求 tty），所以走**服务级安装**：
///   1. 若解包目录里有 sol 二进制，直接 install 到 /usr/local/bin；
///   2. 写 /etc/systemd/system/sol.service（ExecStart 带配置文件）；
///   3. systemctl daemon-reload + enable --now。
/// 非 root 时整条经 pkexec 提权（与 mount.rs 的 mount_command 同一条路）。
#[tauri::command]
pub(crate) async fn sol_install(app: AppHandle, install_dir: Option<String>) -> Result<String, String> {
    trace(&app, "command", "sol_install");
    let source = install_dir
        .as_deref()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(detect_sol_install_dir)
        .ok_or_else(|| {
            "没找到 sol 的解包目录（里面要有 sol 二进制和 sol.yaml）。请点「打开安装目录」手工选，或先把 sol 包解开".to_string()
        })?;

    let bin_src = source.join("sol");
    let config_src = source.join("sol.yaml");
    if !bin_src.is_file() {
        return Err(format!("{} 里没有 sol 二进制", source.display()));
    }

    // 配置来源：解包目录里的 sol.yaml 优先，没有就退到示例配置改名
    let config_src = if config_src.is_file() {
        config_src
    } else {
        let example = source.join("sol-example.yaml");
        if example.is_file() {
            example
        } else {
            return Err(format!(
                "{} 里既没有 sol.yaml 也没有 sol-example.yaml",
                source.display()
            ));
        }
    };

    // 生成安装脚本（临时文件）：内容固定，免拼接引号地狱
    let script = format!(
        "#!/bin/sh\nset -e\ninstall -m 755 '{bin}' /usr/local/bin/sol\nmkdir -p /etc/sol\ninstall -m 600 '{config}' /etc/sol/sol.yaml\ncat > /etc/systemd/system/{svc}.service <<'UNIT'\n[Unit]\nDescription=SoL listener (installed by AnyBoot)\nAfter=network-online.target\n\n[Service]\nExecStart=/usr/local/bin/sol listen --config /etc/sol/sol.yaml\nRestart=always\n\n[Install]\nWantedBy=multi-user.target\nUNIT\nsystemctl daemon-reload\nsystemctl enable --now {svc}.service\n",
        bin = bin_src.display(),
        config = config_src.display(),
        svc = SOL_SERVICE,
    );
    let tmp_script = std::env::temp_dir().join("anyboot-sol-install.sh");
    fs::write(&tmp_script, &script).map_err(|error| format!("写安装脚本失败：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp_script, fs::Permissions::from_mode(0o755));
    }

    let (program, args): (String, Vec<String>) = if sol_is_elevated() {
        ("sh".to_string(), vec![tmp_script.display().to_string()])
    } else {
        (
            "pkexec".to_string(),
            vec!["sh".to_string(), tmp_script.display().to_string()],
        )
    };
    let output = command(&program)
        .args(&args)
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("运行安装脚本失败：{error}"))?;
    let _ = fs::remove_file(&tmp_script);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "安装失败（exit {}）：{}",
            output.status.code().unwrap_or(-1),
            stderr.trim().chars().take(300).collect::<String>()
        ));
    }
    let message = format!(
        "已安装：二进制 → /usr/local/bin/sol，配置 → /etc/sol/sol.yaml，服务 sol.service 已启用并启动"
    );
    sol_log(&app, message.clone());
    record_action(&app, "sol_install", true, &message);
    Ok(message)
}

/// 卸载 sol：停服务 + disable + 删 unit + 删二进制 + 删 /etc/sol/sol.yaml
/// （配置有 .bak-anyboot 备份就一起留着不删，手工回滚用）。非 root 经 pkexec。
#[tauri::command]
pub(crate) async fn sol_uninstall(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "sol_uninstall");
    let script = format!(
        "#!/bin/sh\nset -e\nsystemctl stop {svc}.service 2>/dev/null || true\nsystemctl disable {svc}.service 2>/dev/null || true\nrm -f /etc/systemd/system/{svc}.service\nsystemctl daemon-reload\nrm -f /usr/local/bin/sol\n# 配置 /etc/sol/sol.yaml 保留：里面有用户改过的规则，手工删\n",
        svc = SOL_SERVICE,
    );
    let tmp_script = std::env::temp_dir().join("anyboot-sol-uninstall.sh");
    fs::write(&tmp_script, &script).map_err(|error| format!("写卸载脚本失败：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp_script, fs::Permissions::from_mode(0o755));
    }

    let (program, args): (String, Vec<String>) = if sol_is_elevated() {
        ("sh".to_string(), vec![tmp_script.display().to_string()])
    } else {
        (
            "pkexec".to_string(),
            vec!["sh".to_string(), tmp_script.display().to_string()],
        )
    };
    let output = command(&program)
        .args(&args)
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("运行卸载脚本失败：{error}"))?;
    let _ = fs::remove_file(&tmp_script);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "卸载失败（exit {}）：{}",
            output.status.code().unwrap_or(-1),
            stderr.trim().chars().take(300).collect::<String>()
        ));
    }
    let message = "已卸载：服务已停止并移除，二进制已删除，配置保留在 /etc/sol/sol.yaml（要彻底清就手工删）";
    sol_log(&app, message.to_string());
    record_action(&app, "sol_uninstall", true, message);
    Ok(message.to_string())
}

/// 启动 / 停止 / 重启 sol 服务（非 root 经 pkexec systemctl）。
#[tauri::command]
pub(crate) async fn sol_service_ctl(
    app: AppHandle,
    action: String,
) -> Result<String, String> {
    trace(&app, "command", &format!("sol_service_ctl {action}"));
    let action = match action.as_str() {
        "start" | "stop" | "restart" => action,
        _ => return Err(format!("不支持的动作：{action}（只认 start / stop / restart）")),
    };
    let verb = format!("systemctl");
    let (program, args): (String, Vec<String>) = if sol_is_elevated() {
        (verb, vec![action.clone(), format!("{}.service", SOL_SERVICE)])
    } else {
        (
            "pkexec".to_string(),
            vec![
                "systemctl".to_string(),
                action.clone(),
                format!("{}.service", SOL_SERVICE),
            ],
        )
    };
    let output = command(&program)
        .args(&args)
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("运行 systemctl 失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "systemctl {action} 失败：{}",
            stderr.trim().chars().take(200).collect::<String>()
        ));
    }
    let message = format!("sol 服务已{action}");
    record_action(&app, "sol_service_ctl", true, &message);
    Ok(message)
}

/// 打开 sol 安装目录（文件管理器）。给定了路径就用它；没给就找检测到的那个。
#[tauri::command]
pub(crate) fn sol_open_dir(app: AppHandle, dir: Option<String>) -> Result<String, String> {
    trace(&app, "command", "sol_open_dir");
    let resolved = match dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => detect_sol_install_dir()
            .or_else(|| PathBuf::from(SOL_BIN).parent().map(Path::to_path_buf))
            .ok_or_else(|| "没找到 sol 安装目录".to_string())?,
    };
    if !resolved.is_dir() {
        return Err(format!("{} 不是目录", resolved.display()));
    }
    open_directory_in_explorer(&resolved)?;
    Ok(resolved.display().to_string())
}

/// 探测 sol 的解包 / 安装目录（找带有 sol 二进制 + 配置文件的目录）。
///
/// 惯例位置优先：/usr/local/software/sol-*（用户这台机器的落点）、
/// /opt/sol*、~/sol*。返回第一个看起来像 sol 目录的。
fn detect_sol_install_dir() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![
        PathBuf::from("/usr/local/software"),
        PathBuf::from("/opt"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(&home).join("Downloads"));
        roots.push(PathBuf::from(home));
    }
    for root in roots {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|dir| dir.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            let name = dir
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if name.starts_with("sol") && dir.join("sol").is_file() {
                return Some(dir);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_start_config_extracts_path() {
        let line = "ExecStart={ path=/usr/local/bin/sol ; argv[]=/usr/local/bin/sol listen --config /usr/local/software/sol-v0.1.0-linux-amd64/sol.yaml ; ignore_errors=no ; start_time=[Mon 2026-10-19 10:55:08 CST] }";
        assert_eq!(
            exec_start_config(line).as_deref(),
            Some("/usr/local/software/sol-v0.1.0-linux-amd64/sol.yaml")
        );
    }

    #[test]
    fn exec_start_config_supports_equals_form() {
        let line = "ExecStart={ path=/usr/local/bin/sol ; argv[]=/usr/local/bin/sol listen --config=/etc/sol/sol.yaml }";
        assert_eq!(exec_start_config(line).as_deref(), Some("/etc/sol/sol.yaml"));
    }

    #[test]
    fn exec_start_config_without_config_is_none() {
        let line = "ExecStart={ path=/usr/local/bin/sol ; argv[]=/usr/local/bin/sol listen }";
        assert_eq!(exec_start_config(line), None);
    }

    #[test]
    fn sol_extracted_root_prefers_unique_subdir() {
        let tmp = std::env::temp_dir().join(format!("anyboot-sol-root-{}", std::process::id()));
        let root = tmp.join("sol-v0.1.0-linux-amd64");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("sol"), b"fake binary").unwrap();
        // 目录本身没有 sol → 顶层唯一子目录有 → 返回它
        assert_eq!(sol_extracted_root(&tmp), Some(root.clone()));
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn sol_extracted_root_direct_hit() {
        let tmp = std::env::temp_dir().join(format!("anyboot-sol-root2-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("sol"), b"fake binary").unwrap();
        // 解压目录本身就是根（解包时去掉了顶层目录）
        assert_eq!(sol_extracted_root(&tmp), Some(tmp.clone()));
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn sol_extracted_root_none_when_empty() {
        let tmp = std::env::temp_dir().join(format!("anyboot-sol-root3-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        assert_eq!(sol_extracted_root(&tmp), None);
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn expand_home_dir_variants() {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return; // 无 HOME 环境跳过（展开不了是预期）
        }
        // ~/sol → <home>/sol
        assert_eq!(
            expand_home_dir("~/sol"),
            PathBuf::from(&home).join("sol")
        );
        // 裸 ~ → home 本身
        assert_eq!(expand_home_dir("~"), PathBuf::from(&home));
        // 绝对路径不动
        assert_eq!(expand_home_dir("/opt/sol"), PathBuf::from("/opt/sol"));
        // 相对路径不动
        assert_eq!(expand_home_dir("sol"), PathBuf::from("sol"));
    }

    #[test]
    fn default_dir_is_under_home() {
        let dir = sol_download_dir_default();
        // 三平台共同点：默认目录带 sol 名（~/sol / %USERPROFILE%\sol）
        assert_eq!(dir.file_name().map(|n| n.to_string_lossy().to_string()), Some("sol".to_string()));
    }

    const SAMPLE_YAML: &str = "version: 1\nserver:\n  interfaces: []\n  watch: 5s\n  http:\n    enabled: true\n    listen: 127.0.0.1:8080\n    auth: { type: bearer, token: \"abc123456789\" }\n    # auth: { type: bearer, token_env: SOL_TOKEN }\nrules:\n  - match: { ports: [11] }\n    action: power.shutdown\n";

    #[test]
    fn sol_parse_http_reads_flow_auth() {
        let http = sol_parse_http(SAMPLE_YAML);
        assert!(http.enabled);
        assert_eq!(http.listen, "127.0.0.1:8080");
        assert_eq!(http.token, "abc123456789");
        assert_eq!(http.auth_kind, "token");
    }

    #[test]
    fn sol_parse_http_without_block() {
        let http = sol_parse_http("version: 1\nrules: []\n");
        assert!(!http.enabled);
        assert_eq!(http.listen, "");
        assert_eq!(http.auth_kind, "none");
    }

    #[test]
    fn sol_yaml_set_listen_replaces_only_that_key() {
        let updated = sol_yaml_set_http_key(SAMPLE_YAML, "listen", "0.0.0.0:9000");
        let http = sol_parse_http(&updated);
        assert_eq!(http.listen, "0.0.0.0:9000");
        // 其余字段原样
        assert!(http.enabled);
        assert_eq!(http.token, "abc123456789");
        // rules 段不动
        assert!(updated.contains("action: power.shutdown"));
    }

    #[test]
    fn sol_yaml_set_enabled() {
        let updated = sol_yaml_set_http_key(SAMPLE_YAML, "enabled", "false");
        assert!(sol_parse_http(&updated).enabled == false);
    }

    #[test]
    fn set_key_preserves_file_indent_not_hardcoded() {
        // 回归：缩进必须跟随文件（4 空格文件写完还是 4 空格），否则同级键缩进
        // 不一致，yaml 直接拒收、sol 反复重启（14:39 现场踩坑）。
        let four_space = "version: 1\nserver:\n  http:\n    enabled: true\n    listen: 127.0.0.1:8080\nrules: []\n";
        let updated = sol_yaml_set_http_key(four_space, "listen", "0.0.0.0:9000");
        assert!(updated.contains("    listen: 0.0.0.0:9000"), "4-space kept: {updated}");
        assert!(!updated.contains("      listen:"), "no 6-space leak: {updated}");
        // 换行 token 同理
        let auth_updated = sol_yaml_set_auth(four_space, "tok12345678");
        assert!(
            auth_updated.contains("    auth: { type: bearer, token: \"tok12345678\" }"),
            "auth keeps 4-space: {auth_updated}"
        );
        // 整个文件 yaml 结构可被解析（缩进一致）
        let http = sol_parse_http(&auth_updated);
        assert_eq!(http.token, "tok12345678");
    }

    #[test]
    fn sol_yaml_set_auth_replaces_whole_line() {
        let updated = sol_yaml_set_auth(SAMPLE_YAML, "newtoken123456");
        let http = sol_parse_http(&updated);
        assert_eq!(http.token, "newtoken123456");
        assert_eq!(http.auth_kind, "token");
        // 注释掉的 auth 示例行不受影响（含 token_env 的那行还在）
        assert!(updated.contains("token_env"));
    }

    #[test]
    fn http_span_stops_at_rules() {
        let (start, end) = sol_yaml_http_span(SAMPLE_YAML).unwrap();
        let text = SAMPLE_YAML.lines().collect::<Vec<_>>();
        // 块内不含 rules 行
        assert!(text[end].starts_with("rules:"));
        assert!(text[start].trim().starts_with("http:"));
    }

    #[test]
    fn extract_flow_value_strips_quotes() {
        let flow = "{ type: bearer, token: \"xyz987\" }";
        assert_eq!(extract_flow_value(flow, "token").as_deref(), Some("xyz987"));
        assert_eq!(extract_flow_value(flow, "type").as_deref(), Some("bearer"));
        assert_eq!(extract_flow_value(flow, "missing"), None);
    }

    #[test]
    fn set_http_key_without_block_appends() {
        let updated = sol_yaml_set_http_key("version: 1\nserver:\n  watch: 5s\n", "enabled", "true");
        let http = sol_parse_http(&updated);
        assert!(http.enabled);
    }
}
