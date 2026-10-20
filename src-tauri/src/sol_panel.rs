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

use std::io::BufReader;

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

/// 配置文件等路径非 root 读不到时的提权读取。
///
/// Linux：pkexec cat；macOS：osascript 以 root cat（弹一次管理员授权框）。
/// 只用于读；内容原样返回。
///
/// macOS 的授权框在同一 sudo ticket 窗口（约 5 分钟）内复用 —— 连续读取不会
/// 每次都问密码。
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
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "do shell script \"cat '{}'\" with administrator privileges",
            path.display().to_string().replace('\\', "\\\\").replace('"', "\\\"")
        );
        let output = command("osascript")
            .args(["-e", &script])
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let canceled = stderr.contains("-128") || stderr.contains("User canceled");
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                if canceled {
                    "提权读取被取消".to_string()
                } else {
                    format!(
                        "提权读取失败：{}",
                        stderr.trim().chars().take(160).collect::<String>()
                    )
                },
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
    #[cfg(target_os = "windows")]
    {
        // Windows：以管理员身份跑 powershell 读文件（UAC 提权；已提权则直接读）。
        // 用 -EncodedCommand 避免引号地狱。
        // 先试直接读（普通权限能读就别弹 UAC）
        if let Ok(content) = fs::read_to_string(path) {
            return Ok(content);
        }
        // 读不到：以管理员身份读（UAC 一次）。Start-Process -Verb RunAs 起子进程，
        // 结果写临时文件再取回 —— 因为 RunAs 的 stdout 拿不到。
        let result_file = std::env::temp_dir().join(format!(
            "anyboot-sol-read-{}.txt",
            std::process::id()
        ));
        let script = format!(
            "$ErrorActionPreference='Stop'; Copy-Item -LiteralPath '{p}' -Destination '{r}' -Force",
            p = path.display(),
            r = result_file.display()
        );
        let encoded = windows_utf16_encoded(&script);
        let output = command("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "Start-Process powershell -Verb RunAs -Wait -ArgumentList '-NoProfile','-EncodedCommand','{encoded}'"
                ),
            ])
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "提权读取失败：{}",
                    stderr.trim().chars().take(160).collect::<String>()
                ),
            ));
        }
        let content = fs::read_to_string(&result_file).map_err(|error| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("提权读取未产生结果文件：{error}"),
            )
        });
        let _ = fs::remove_file(&result_file);
        content
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "该平台暂无提权读取方案（可手工把文件权限放开后重试）",
        ))
    }
}

/// Windows：把 PowerShell 脚本编码成 -EncodedCommand 需要的 UTF-16LE base64。
#[cfg(target_os = "windows")]
fn windows_utf16_encoded(script: &str) -> String {
    use base64::Engine as _;
    let bytes: Vec<u8> = script
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// 提权写文件（三平台）：内容经临时文件交给 root/管理员落位。
///
/// 落位后**顺手把权限放宽**：Linux/macOS chmod 0644、Windows 继承目录 ACL ——
/// 这样写入之后，后续的「读取」就再不需要提权了（用户定案的免密体验）。
/// Linux 走 pkexec、macOS 走 osascript、Windows 走 PowerShell RunAs（UAC）。
fn privileged_write(path: &Path, content: &str) -> Result<(), String> {
    let path_str = path.display().to_string();

    // 直接写得了就直接写（用户自己拥有的文件，如 ~/sol/sol.yaml）
    if fs::write(path, content.as_bytes()).is_ok() {
        return Ok(());
    }

    let staging = std::env::temp_dir().join(format!(
        "anyboot-sol-write-{}.yaml",
        std::process::id()
    ));
    fs::write(&staging, content.as_bytes())
        .map_err(|error| format!("写暂存文件失败：{error}"))?;
    let staging_str = staging.display().to_string();

    #[cfg(target_os = "linux")]
    let outcome: Result<(), String> = {
        // 备份 + 落位 + 放宽读权限（0644），一条脚本一次 pkexec
        let script = format!(
            "set -e\ncp -f '{p}' '{p}.bak-anyboot' 2>/dev/null || true\ninstall -m 0644 '{s}' '{p}'\nrm -f '{s}'\n",
            p = path_str,
            s = staging_str,
        );
        let script_file = std::env::temp_dir().join(format!("anyboot-sol-w-{}.sh", std::process::id()));
        fs::write(&script_file, script).map_err(|e| format!("写提权脚本失败：{e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&script_file, fs::Permissions::from_mode(0o755));
        }
        let result = command("pkexec")
            .arg("sh")
            .arg(&script_file)
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("启动 pkexec 失败：{error}"))
            .and_then(|output| {
                if output.status.success() {
                    Ok(())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!(
                        "写入失败（pkexec 被拒或出错）：{}",
                        stderr.trim().chars().take(200).collect::<String>()
                    ))
                }
            });
        let _ = fs::remove_file(&script_file);
        result
    };

    #[cfg(target_os = "macos")]
    let outcome: Result<(), String> = {
        let script = format!(
            "set -e\ncp -f '{p}' '{p}.bak-anyboot' 2>/dev/null || true\ninstall -m 0644 '{s}' '{p}'\nrm -f '{s}'\n",
            p = path_str,
            s = staging_str,
        );
        let escaped = script.replace('\\', "\\\\").replace('"', "\\\"");
        let wrapped = format!("do shell script \"{}\" with administrator privileges", escaped);
        command("osascript")
            .args(["-e", &wrapped])
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("启动 osascript 失败：{error}"))
            .and_then(|output| {
                if output.status.success() {
                    Ok(())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let canceled = stderr.contains("-128") || stderr.contains("User canceled");
                    Err(if canceled {
                        "提权写入被取消".to_string()
                    } else {
                        format!(
                            "写入失败：{}",
                            stderr.trim().chars().take(200).collect::<String>()
                        )
                    })
                }
            })
    };

    #[cfg(target_os = "windows")]
    let outcome: Result<(), String> = {
        // PowerShell：复制备份 → 覆盖 → 删除暂存。以管理员跑（UAC 一次）。
        // Copy-Item 到用户目录后文件属主变成本次操作的账户，之后可读可写。
        let script = format!(
            "$ErrorActionPreference='Stop'; \
             if (Test-Path -LiteralPath '{p}') {{ Copy-Item -LiteralPath '{p}' -Destination '{p}.bak-anyboot' -Force }}; \
             Copy-Item -LiteralPath '{s}' -Destination '{p}' -Force; \
             Remove-Item -LiteralPath '{s}' -Force",
            p = path_str,
            s = staging_str,
        );
        let encoded = windows_utf16_encoded(&script);
        command("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!(
                    "Start-Process powershell -Verb RunAs -Wait -ArgumentList '-NoProfile','-EncodedCommand','{encoded}'"
                ),
            ])
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("启动 PowerShell 失败：{error}"))
            .and_then(|output| {
                if output.status.success() {
                    Ok(())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!(
                        "写入失败（UAC 被拒或出错）：{}",
                        stderr.trim().chars().take(200).collect::<String>()
                    ))
                }
            })
    };

    let _ = fs::remove_file(&staging);
    outcome?;
    // 兜底：让之后的读取免提权（写成功但权限没跟着变的场景）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o644));
    }
    Ok(())
}

/// 以管理员权限重新探测一次服务状态（仅当状态被权限挡住时才需要）。
///
/// 用户定案：状态查看**优先走免提权路径**；实在读不到时，由用户显式点按钮
/// 触发一次授权弹窗（而不是自动弹、也不是武断显示「未注册」）。
#[tauri::command]
pub(crate) async fn sol_status_privileged(app: AppHandle) -> Result<SolStatus, String> {
    trace(&app, "command", "sol_status_privileged");
    let mut status = tauri::async_runtime::spawn_blocking(sol_status_snapshot)
        .await
        .map_err(|error| format!("探测失败：{error}"))?;

    #[cfg(target_os = "macos")]
    {
        // 已经能看到服务就不用提权了
        if !status.status_needs_privilege && (status.service_loaded || !status.installed) {
            return Ok(status);
        }
        let unit = format!("system/{}", SOL_LAUNCHD_LABEL);
        let script = format!(
            "do shell script \"launchctl print {}\" with administrator privileges",
            unit
        );
        let output = command("osascript")
            .args(["-e", &script])
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("启动 osascript 失败：{error}"))?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            status.service_loaded = true;
            status.service_active = text
                .lines()
                .any(|line| line.trim().starts_with("state = running"));
            status.status_needs_privilege = false;
            if status.service_active {
                status.process_running = true;
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let canceled = stderr.contains("-128") || stderr.contains("User canceled");
            return Err(if canceled {
                "已取消授权".to_string()
            } else {
                format!(
                    "提权查询失败：{}",
                    stderr.trim().chars().take(200).collect::<String>()
                )
            });
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux/Windows：状态探测本来就是免提权的，这里只刷新一次
        let _ = &mut status;
    }
    Ok(status)
}

/// 服务名：Linux systemd 单元名（Windows 计划任务 / macOS 服务同名）。
const SOL_SERVICE: &str = "sol";
/// 三端落点（官方 install.sh / install.ps1 的真实布局，2026-10 实测）：
///
/// | 平台 | 二进制 | 台账 | 运行配置 | 单元 |
/// |---|---|---|---|---|
/// | Linux/macOS | `~/sol/bin/sol` | `~/sol/install.json` | `~/sol/sol.yaml` | `~/sol/sol.service` / `~/sol/com.lidiaoo.sol.plist` |
/// | Windows | `%ProgramData%\sol\sol.exe` | `%ProgramData%\sol\install.json` | `%USERPROFILE%\sol\sol.yaml` | 计划任务 `sol` |
///
/// **不再兼容旧路径**（/usr/local/bin/sol、/etc/sol/sol.yaml 等）：三端已统一收进
/// 用户目录（Windows 二进制因需 SYSTEM 计划任务仍在 ProgramData），旧路径一律不查。
/// 从安装目录的 `install.yaml` 里读出 `run.args` 的 `--config` 取值。
///
/// 这份文件是 sol 官方脚本生成的「sol 怎么跑」的唯一权威来源（三端一致）：
/// 用户改了 `--config` 就是真的改了，anyboot 不该再用「惯例路径」去猜。
/// 返回文件名与路径都不做存在性假设 —— 文件可能还不存在（没装过），此时返回 None 让调用方走兜底。
///
/// 只认脚本自己写出的那种写法（`args: [listen, --config, <path>]`，一行、逗号分隔），
/// 与 install.sh 的 parse_args 保持同一套约定，不去猜别的 YAML 形态。
fn sol_install_yaml_config(dir: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(dir.join("install.yaml")).ok()?;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("args:") {
            continue;
        }
        let value = trimmed["args:".len()..].trim();
        let inner = value
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or(value);
        let mut tokens = inner.split(',').map(|token| token.trim());
        let mut prev: Option<&str> = None;
        while let Some(token) = tokens.next() {
            // 同时认 `--config X` 与 `--config=X`
            if let Some(inline) = token.strip_prefix("--config=") {
                let inline = inline.trim().trim_matches('"').trim_matches('\'');
                if !inline.is_empty() {
                    return Some(PathBuf::from(inline));
                }
            }
            if prev == Some("--config") {
                let value = token.trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return Some(PathBuf::from(value));
                }
            }
            prev = Some(token);
        }
    }
    None
}

/// 配置文件候选路径：**安装目录（~/sol）优先** —— 「配置 sol.yaml 就在安装目录」
/// 是既定布局（下载/解压/安装都在这里）。
///
/// 顺序：安装目录的 `install.yaml` 里 `--config` 指哪儿就是哪儿（官方脚本写的
/// 「怎么跑」，唯一权威）；没有才用同目录的 `sol.yaml`（官方默认生成位置）。
/// 没有 `sol.yml` 兜底、没有 /usr/local/bin、没有 /etc/sol —— 官方只会写 `sol.yaml`。
fn sol_config_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = sol_install_dir_from_env() {
        if let Some(declared) = sol_install_yaml_config(&dir) {
            candidates.push(declared);
        }
        candidates.push(dir.join("sol.yaml"));
    }
    candidates
}

/// 默认安装目录（= 下载/解压目录）：`~/sol`。
///
/// Windows 家目录 = `%USERPROFILE%`（官方 install.ps1 的 RunDir）；
/// Linux/macOS 用 `$HOME`。三端唯一落点。
fn sol_install_dir_from_env() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let home = std::env::var("USERPROFILE");
    #[cfg(not(target_os = "windows"))]
    let home = std::env::var("HOME");
    home.ok()
        .filter(|home| !home.trim().is_empty())
        .map(|home| PathBuf::from(home).join(SOL_SERVICE))
}
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

/// 官方包目录名前缀：release 压缩包内唯一顶层目录就是 `sol-vX.Y.Z-<平台>/`。
/// （实际解压出来的目录，安装包里的二进制/脚本都在这层下面。）
const SOL_PACKAGE_DIR_PREFIX: &str = "sol-v";

/// 安装包里的二进制文件名：Windows 是 sol.exe，其它平台是不带后缀的 sol。
fn sol_package_bin() -> &'static str {
    if cfg!(target_os = "windows") {
        "sol.exe"
    } else {
        "sol"
    }
}

/// 安装包里的安装脚本：Windows 官方发 install.cmd / install.ps1，其它平台 install.sh。
/// 「一键安装」要跑的就是它 —— 以前在所有平台都写死了 install.sh，Windows 上必然找不到。
fn sol_package_script() -> &'static str {
    if cfg!(target_os = "windows") {
        "install.cmd"
    } else {
        "install.sh"
    }
}

/// 判断 dir 是不是**完整**的官方安装包：二进制 + 安装脚本都在。
///
/// 以前只认 `sol` + `install.sh` 两个固定名字，Windows 包（sol.exe / install.cmd）永远
/// 匹配不上 —— 一键安装在解压后的校验那一步就报「包内容与预期不符」。
fn sol_package_complete(dir: &Path) -> bool {
    dir.join(sol_package_bin()).is_file() && dir.join(sol_package_script()).is_file()
}

/// 从父目录里挑出官方包目录：先看 `sol-v*` 命名的子目录（release 包内唯一的那个），
/// 再看解压目录本身（--strip-components=1 / 归拢之后的状态）。多个候选时取版本最高的。
fn sol_find_package_dir(parent: &Path) -> Option<PathBuf> {
    if sol_package_complete(parent) {
        return Some(parent.to_path_buf());
    }
    let mut candidates: Vec<PathBuf> = fs::read_dir(parent)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.starts_with(SOL_PACKAGE_DIR_PREFIX))
                        .unwrap_or(false)
                })
                .filter(|path| sol_package_complete(path))
                .collect()
        })
        .unwrap_or_default();
    // 目录名 sol-vX.Y.Z-<平台> 按字典序 == 按版本序（0.9.0 < 0.10.0 时 0.10 排在前面，
    // 这里只是兜底多版本并存的场景，取最后一个即可）
    candidates.sort();
    candidates.pop()
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
    /// 服务状态探测**可能因权限不足而不准**（launchctl/systemctl 被拒）。
    /// 前端据此提示「需要授权查看」而不是武断地显示「未注册」。
    pub(crate) status_needs_privilege: bool,
}

impl Default for SolStatus {
    /// 全「未安装 / 未知」的兜底快照：spawn_blocking 异常（池关闭等）时命令
    /// 不报错，界面照样能渲染（与 sol_status_snapshot 的降级语义一致）。
    fn default() -> Self {
        Self {
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
            status_needs_privilege: false,
        }
    }
}

/// 读 sol 状态快照（不改任何东西）。任何一步失败都降级成「未知」，绝不让整条命令报错 ——
/// sol 没装、systemctl 不在、ss 不在，界面都还要能渲染出「未安装 / 未知」。
///
/// 快照要跑 systemctl/ss 等子进程（同步 `.output()`），命令走 async + spawn_blocking
/// 把它挪进阻塞池（blocking_guard 守卫：同步命令体里不许有阻塞调用）。
/// 官方台账 install.json 里的 **binary** 字段（「已安装」的唯一依据是台账，
/// 但二进制路径是台账里记的，按它去找最准）。
///
/// 台账位置（官方脚本实测）：
/// - Linux/macOS：`~/sol/install.json`
/// - Windows：`%ProgramData%\sol\install.json`
fn sol_ledger_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let root = std::env::var("ProgramData")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        vec![root.join("sol").join("install.json")]
    }
    #[cfg(not(target_os = "windows"))]
    {
        sol_install_dir_from_env()
            .map(|dir| vec![dir.join("install.json")])
            .unwrap_or_default()
    }
}

/// 台账里的 binary 字段（文件存在才算命中）。
fn sol_binary_from_ledger() -> Option<PathBuf> {
    for ledger in sol_ledger_paths() {
        let Ok(content) = fs::read_to_string(&ledger) else {
            continue;
        };
        // 不引 JSON crate 依赖：binary 字段一行正则级提取足够
        for line in content.lines() {
            let trimmed = line.trim().trim_end_matches(',');
            if let Some(rest) = trimmed.strip_prefix("\"binary\"") {
                let rest = rest.trim().strip_prefix(':')?.trim();
                let value = rest.trim_matches(|c| c == '"' || c == ',');
                let path = PathBuf::from(value);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// systemd ExecStart 里跑的那个 sol（服务在跑，二进制必然存在且就是它在用）。
fn sol_binary_from_service_unit() -> Option<PathBuf> {
    let output = command("systemctl")
        .args([
            "show",
            SOL_SERVICE,
            "--property=ExecStart",
            "--no-pager",
        ])
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    // ExecStart={ path=/home/x/sol/bin/sol ; argv[]=/home/x/sol/bin/sol listen ... }
    let path_part = line.split("path=").nth(1)?;
    let path = path_part.split(';').next()?.trim();
    let path = PathBuf::from(path);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

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
        status_needs_privilege: false,
    };

    // 1) 二进制：官方台账 install.json 的 binary 字段（权威）→ ExecStart 里
    //    跑的那个（服务在跑时必然命中）→ 平台惯例落点。
    //    旧路径（/usr/local/bin/sol、PATH 扫描）不再兼容 —— 三端已统一。
    let bin = sol_binary_from_ledger()
        .or_else(sol_binary_from_service_unit)
        .or_else(|| {
            let expected = sol_installed_bin_path();
            expected.is_file().then_some(expected)
        });
    if let Some(bin) = bin {
        status.installed = true;
        status.bin_path = Some(bin.display().to_string());
        if let Ok(output) = command(bin.to_str().unwrap_or_default())
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

    // 2) 服务状态：Linux 走 systemctl；macOS 走 launchctl（LaunchDaemon）。
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
                // ExecStart={ path=/home/u/sol/bin/sol ; argv[]=/home/u/sol/bin/sol listen --config /home/u/sol/sol.yaml ; ... }
                status.service_detail = Some(line.trim().to_string());
                if let Some(config) = exec_start_config(line.trim()) {
                    status.config_path = Some(config);
                }
            }
        }
        status.recent_logs = journal_tail();
    }
    #[cfg(target_os = "macos")]
    {
        // LaunchDaemon 在不在 + 起没起来：launchctl print（不存在则非零退出）
        // 服务状态探测**不需要任何提权**（用户定案：状态查看不要密码框）：
        // - plist 是 0644（官方脚本 put_root 0644），直接读得到 → 注册与否 + 配置路径
        // - `launchctl print system/<label>` 普通用户可执行，不需要 sudo
        // - 进程表 pgrep 可读 → 运行与否（最可靠的「服务在跑」证据）
        // - sol.log 是 0644 → 日志尾部
        let plist_path = sol_launchd_plist_path();
        if let Ok(plist) = fs::read_to_string(&plist_path) {
            status.service_loaded = true;
            // 配置路径：plist 里 `--config` 的**下一个** <string> 才是路径。
            // plist 的 ProgramArguments 每项一行：
            //   <string>listen</string>
            //   <string>--config</string>
            //   <string>/Users/x/sol/sol.yaml</string>
            // 之前在同一行 split 拿到的是 "--config</string>"，导致前端拿垃圾路径去读，
            // 读不到就触发提权读取 → 弹密码框（用户报的「进 SOL 页面要密码」根因）。
            let config_from_plist = {
                let mut lines = plist.lines().peekable();
                let mut found = None;
                while let Some(line) = lines.next() {
                    if line.contains("--config") {
                        // 本行可能自带值（`--config=/path`），否则看下一行
                        if let Some(rest) = line.split("--config").nth(1) {
                            let rest = rest.trim_start_matches('=').trim();
                            if let Some(value) = rest.split("<string>").nth(1) {
                                found = Some(value.split("</string>").next().unwrap_or("").trim().to_string());
                            }
                        }
                        if found.is_none() {
                            if let Some(next) = lines.peek() {
                                if let Some(value) = next.split("<string>").nth(1) {
                                    found = Some(
                                        value.split("</string>").next().unwrap_or("").trim().to_string(),
                                    );
                                }
                            }
                        }
                        break;
                    }
                }
                found.filter(|value| !value.is_empty())
            };
            if let Some(value) = config_from_plist {
                status.config_path = Some(value.clone());
                status.service_detail = Some(format!("launchd: {value}"));
            }
        }
        // 运行态：优先看 launchd（不需要提权），拿不到再靠进程表兜底
        let unit = format!("system/{}", SOL_LAUNCHD_LABEL);
        if let Ok(output) = command("launchctl")
            .args(["print", &unit])
            .stderr(Stdio::piped())
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                status.service_loaded = true;
                status.service_active = text
                    .lines()
                    .any(|line| line.trim().starts_with("state = running"));
            } else {
                // 失败有两种：服务真不存在 / 权限不足看不到。后者不能让界面
                // 武断说「未注册」—— 标出来，前端提示「需要授权查看」。
                let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
                let permission_like = stderr.contains("permission")
                    || stderr.contains("denied")
                    || stderr.contains("not privileged")
                    || stderr.contains("operation not permitted")
                    || stderr.contains("bad request");
                if permission_like && !plist_path.exists() {
                    status.status_needs_privilege = true;
                } else if permission_like {
                    // plist 在但 print 被拒：服务大概率是注册着的，只是看不到细节
                    status.service_loaded = true;
                    status.status_needs_privilege = true;
                }
            }
        }
        if status.service_active || status.process_running {
            status.process_running = true;
        }
        status.recent_logs = macos_sol_log_tail();
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
        for candidate in sol_config_candidates() {
            if candidate.is_file() {
                status.config_path = Some(candidate.display().to_string());
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
/// Linux 快照用（macOS 的配置路径从 LaunchDaemon plist 里抠）；测试在
/// Windows/macOS 上也要跑，所以这里不挂 cfg，函数本身很小。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn exec_start_config(line: &str) -> Option<String> {
    // argv[]=/home/u/sol/bin/sol listen --config /home/u/sol/sol.yaml
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

// ---- macOS：launchd（LaunchDaemon）那条线 ----
// 与 Linux 的 systemd 一一对应：官方脚本把 plist 写在 ~/sol/com.lidiaoo.sol.plist，
// 用 launchctl bootstrap system 注册（system 域）；日志 plist 指到 ~/sol/sol.log。

/// macOS LaunchDaemon 的 label（与官方 install.sh 的默认一致）。
#[cfg(target_os = "macos")]
const SOL_LAUNCHD_LABEL: &str = "com.lidiaoo.sol";

/// macOS 服务定义 plist 路径：官方脚本写在 `~/sol/com.lidiaoo.sol.plist`，
/// 再用 `launchctl bootstrap system <该文件>` 注册（system 域，但文件在用户目录）。
#[cfg(target_os = "macos")]
fn sol_launchd_plist_path() -> PathBuf {
    sol_install_dir_from_env()
        .unwrap_or_else(|| PathBuf::from("/tmp/sol"))
        .join(format!("{}.plist", SOL_LAUNCHD_LABEL))
}

/// macOS sol 日志最近 15 行：官方 plist 把 StandardOutPath 指到 `~/sol/sol.log`。
#[cfg(target_os = "macos")]
fn macos_sol_log_tail() -> Vec<String> {
    let log_path = sol_install_dir_from_env()
        .unwrap_or_else(|| PathBuf::from("/tmp/sol"))
        .join("sol.log");
    let Ok(text) = fs::read_to_string(&log_path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take(160).collect())
        .rev()
        .take(15)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}


/// 读 sol 进程正在监听的 UDP 端口。
///
/// Linux 走 `ss -ulnp`：匹配行里 `pid=1252` 或进程名 `sol`；Windows 走
/// `netstat -ano -p UDP`：先拿 sol 的 PID 再按最后一列 PID 匹配。
fn sol_listening_ports(process_running: bool) -> Vec<SolListenEntry> {
    // 显式标注元素类型：macOS 上没有任何 cfg 分支会 push，若靠推断 entries 是
    // Vec<_>，下面的 sort_by_key 就报 E0282（Linux/Windows 构建恰好推得出来）。
    let mut entries: Vec<SolListenEntry> = Vec::new();
    let _ = process_running; // 只决定要不要白跑；端口读不出来就空着

    // 本机 sol 进程 PID（pgrep 一次，两个平台都用；macOS 无对应分支，只用于
    // Linux ss / Windows netstat 的行匹配，下划线前缀免 macOS unused 警告）
    #[cfg_attr(target_os = "macos", allow(unused_variables))]
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
pub(crate) async fn sol_status(app: AppHandle) -> SolStatus {
    trace(&app, "command", "sol_status");
    match tauri::async_runtime::spawn_blocking(sol_status_snapshot).await {
        Ok(status) => status,
        Err(error) => {
            // 阻塞池异常（关闭/恐慌）要留痕 —— 否则面板永远停在「正在读取…」
            trace(&app, "sol", &format!("sol_status 任务失败：{error}"));
            SolStatus::default()
        }
    }
}

/// 读配置文件正文（path 为空 = 读正在生效的那份）。
///
/// 配置在安装目录里，可能归 root 管（0600），非 root 直读必败：先直读，
/// 权限类失败走 `pkexec cat` 提权读（用户会看到 polkit 弹窗）。
///
/// 命令体要跑 pkexec 等子进程，走 async + spawn_blocking（blocking_guard 守卫）。
#[tauri::command]
pub(crate) async fn sol_config_read(path: Option<String>) -> Result<SolConfigRead, String> {
    tauri::async_runtime::spawn_blocking(move || sol_config_read_blocking(path))
        .await
        .map_err(|error| format!("sol_config_read 任务失败：{error}"))?
}

/// 只在阻塞池里调用（sol_config_read 的命令体）。
fn sol_config_read_blocking(path: Option<String>) -> Result<SolConfigRead, String> {
    let resolved = match path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(path) => path.to_string(),
        None => sol_status_snapshot()
            .config_path
            .ok_or_else(|| "找不到 sol 配置文件（服务没在跑，安装目录里也没有 sol.yaml）".to_string())?,
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
/// 配置在安装目录里，可能归 root 管：非 root 时整条（备份+写入+落位+chmod）包进一个
/// `/bin/sh -c` 经 `pkexec` 提权执行，一次弹窗完成全部落盘。写完不用动
/// sol —— 它自己 watch 配置变化，秒级生效。
///
/// 命令体要跑 pkexec 等子进程，走 async + spawn_blocking（blocking_guard 守卫）。
#[tauri::command]
pub(crate) async fn sol_config_write(path: String, content: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || sol_config_write_blocking(path, content))
        .await
        .map_err(|error| format!("sol_config_write 任务失败：{error}"))?
}

/// 只在阻塞池里调用（sol_config_write 的命令体）。
fn sol_config_write_blocking(path: String, content: String) -> Result<String, String> {
    let path = path.trim().to_string();
    if path.is_empty() {
        return Err("配置文件路径为空".to_string());
    }
    let target = PathBuf::from(&path);
    if !target.is_file() {
        return Err(format!("{path} 不存在，先在 sol 侧安装好再编辑"));
    }

    // 统一走 privileged_write：能直写就直写，否则提权（pkexec / osascript / UAC），
    // 成功后再放宽权限，保证之后「读」不再需要提权。
    privileged_write(&target, &content)?;
    Ok(format!(
        "已写入 {path}（原文件备份为 {path}.bak-anyboot）。sol 会自动热重载，无需重启服务"
    ))
}

/// 安装 sol 的默认下载目录（用户没设置时）：三平台统一 **用户主目录下的 sol**
/// （Linux/macOS ~/sol，Windows %USERPROFILE%\sol）。用户目录一定可写，下载/
/// 解压不需要提权；提权只发生在「安装」（写 ~/sol/bin、~/sol/sol.service、注册服务）
/// 那一步 —— 权限边界与动作语义一一对应。
pub(crate) fn sol_download_dir_default() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        // %USERPROFILE% = C:\Users\<name>（官方 install.ps1 的 RunDir 就是这个）
        std::env::var("USERPROFILE")
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
/// 流程：目录不存在就建 → 落盘名按平台（Linux/macOS: .tar.gz，Windows: .zip，
/// 与官方 release 一致）→ 用系统自带的 curl → wget（Windows 走 PowerShell
/// Invoke-WebRequest）下载，带
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

    // **已有包且校验一致就复用**（用户定案）：不重复下载。
    // 校验 = 与 release 端的 md5 清单比对（sol-vX.Y.Z-SHA256SUMS / md5 资产）；
    // 拿不到官方校验值时退化为「同名包大小与 HEAD Content-Length 一致即复用」。
    if target.is_file() {
        let local_size = fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0);
        if local_size > 0 {
            // ① 有 md5 旁车：本地 md5 == 旁车记录的 md5 → 就是上次下载的那份，复用
            let sidecar = sol_md5_sidecar(&target);
            if let Ok(recorded) = fs::read_to_string(&sidecar) {
                let recorded = recorded
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if recorded.len() == 32 {
                    match file_md5(&target) {
                        Some(actual) if actual == recorded => {
                            sol_log(
                                &app,
                                format!(
                                    "已有安装包 md5 一致（{}），直接复用：{}",
                                    actual,
                                    target.display()
                                ),
                            );
                            return Ok(target.display().to_string());
                        }
                        Some(actual) => sol_log(
                            &app,
                            format!("已有安装包 md5 不一致（记录 {recorded} / 实际 {actual}），重新下载"),
                        ),
                        None => sol_log(&app, "计算已有安装包 md5 失败，重新下载".to_string()),
                    }
                }
            }
            // ② 没有旁车（旧包/手选的包）：按大小与远端比对
            if let Some(remote_size) = sol_head_content_length(&url) {
                if local_size == remote_size {
                    sol_log(
                        &app,
                        format!(
                            "已有安装包大小与远端一致（{} 字节），直接复用：{}",
                            local_size,
                            target.display()
                        ),
                    );
                    // 补写 md5 旁车，下次走快速路径
                    if let Some(sum) = file_md5(&target) {
                        let _ = fs::write(&sidecar, format!("{sum}  {}\n", asset));
                    }
                    return Ok(target.display().to_string());
                }
                sol_log(
                    &app,
                    format!(
                        "已有安装包大小不一致（本地 {local_size} / 远端 {remote_size}），重新下载"
                    ),
                );
            } else {
                sol_log(
                    &app,
                    format!(
                        "已有安装包（{} 字节）且无法探测远端大小，直接复用：{}",
                        local_size,
                        target.display()
                    ),
                );
                return Ok(target.display().to_string());
            }
        }
    }

    // 已有同名包但校验没过：先清理（避免半截包被误当新包用）
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
    // 记下 md5 旁车，下次「已有包」时用它判断能否复用（用户要求：md5 一致就不重复下）
    if let Some(sum) = file_md5(&target) {
        let _ = fs::write(sol_md5_sidecar(&target), format!("{sum}  {}\n", asset));
        trace(&app, "sol", &format!("安装包 md5={sum}（旁车已写）"));
    }
    Ok(target.display().to_string())
}

/// sol 项目在 GitHub 上的信息（界面展示 + 打开链接用，白名单限定，不接受任意 URL）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SolProjectInfo {
    /// 仓库地址
    pub(crate) repo: String,
    /// releases 页
    pub(crate) releases: String,
    /// 当前跟随的版本
    pub(crate) version: String,
    /// 当前平台资产名
    pub(crate) asset: String,
}

/// 取 sol 项目的 GitHub 信息（纯常量，不联网）。
#[tauri::command]
pub(crate) fn sol_project_info() -> SolProjectInfo {
    SolProjectInfo {
        repo: "https://github.com/lidiaoo/sol".to_string(),
        releases: "https://github.com/lidiaoo/sol/releases".to_string(),
        version: SOL_RELEASE_VERSION.to_string(),
        asset: sol_asset_name(),
    }
}

/// 打开 sol 项目的 GitHub 页面（repo / releases，白名单）。
#[tauri::command]
pub(crate) fn sol_project_open(page: String) -> Result<(), String> {
    let url = match page.as_str() {
        "repo" => "https://github.com/lidiaoo/sol",
        "releases" => "https://github.com/lidiaoo/sol/releases",
        _ => return Err(format!("未知页面：{page}")),
    };
    crate::open_url_in_browser(url)
}

/// 本地安装包现状（进 SOL 页面时回填「安装包」一栏用）。
///
/// 返回：包路径、大小、md5、是否与旁车校验一致、以及同目录下是否已解压好
/// （有 sol-v*/ 官方包：sol(.exe) + install.sh/.cmd）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SolLocalPackage {
    /// 安装包路径（不存在则 None）
    pub(crate) archive: Option<String>,
    /// 包大小（字节）
    pub(crate) size: u64,
    /// md5（计算失败则 None）
    pub(crate) md5: Option<String>,
    /// md5 与旁车记录一致（= 可用、不会被重新下载）
    pub(crate) verified: bool,
    /// 安装目录里是否已解压出完整包（sol(.exe) + install.sh/.cmd）
    pub(crate) extracted: bool,
    /// 安装目录
    pub(crate) dir: String,
}

/// 查询本地是否已有安装包（不下载、不提权）。
#[tauri::command]
pub(crate) async fn sol_local_package(
    app: AppHandle,
    dir: Option<String>,
) -> Result<SolLocalPackage, String> {
    trace(&app, "command", "sol_local_package");
    tauri::async_runtime::spawn_blocking(move || {
        let target_dir = resolve_sol_dir_raw(&dir);
        let asset = sol_asset_name();
        let archive = target_dir.join(&asset);
        let exists = archive.is_file();
        let size = if exists {
            fs::metadata(&archive).map(|meta| meta.len()).unwrap_or(0)
        } else {
            0
        };
        let md5 = if exists { file_md5(&archive) } else { None };
        let verified = match (&md5, fs::read_to_string(sol_md5_sidecar(&archive))) {
            (Some(actual), Ok(recorded)) => recorded
                .split_whitespace()
                .next()
                .map(|value| value.eq_ignore_ascii_case(actual))
                .unwrap_or(false),
            _ => false,
        };
        // 解压状态 = 安装目录下有官方包（sol-v*/ 里的 sol(.exe) + install.sh/.cmd）。
        // Windows 包是 sol.exe + install.cmd，以前写死的 `sol` + `install.sh` 永远为假，
        // 面板上因此始终显示「未解压」。
        let extracted = sol_find_package_dir(&target_dir).is_some();
        Ok(SolLocalPackage {
            archive: exists.then(|| archive.display().to_string()),
            size,
            md5,
            verified,
            extracted,
            dir: target_dir.display().to_string(),
        })
    })
    .await
    .map_err(|error| format!("sol_local_package 任务失败：{error}"))?
}

/// 安装包 md5 旁车文件路径：`<包>.md5`。
fn sol_md5_sidecar(archive: &Path) -> PathBuf {
    let mut path = archive.as_os_str().to_os_string();
    path.push(".md5");
    PathBuf::from(path)
}

/// 计算文件 md5（`md5sum`/`md5`/Windows `certutil` 三平台统一）。
fn file_md5(path: &Path) -> Option<String> {
    #[cfg(target_os = "windows")]
    let (program, args): (&str, Vec<String>) = (
        "certutil",
        vec!["-hashfile".to_string(), path.display().to_string(), "MD5".to_string()],
    );
    #[cfg(target_os = "macos")]
    let (program, args): (&str, Vec<String>) = ("md5", vec!["-q".to_string(), path.display().to_string()]);
    #[cfg(target_os = "linux")]
    let (program, args): (&str, Vec<String>) = ("md5sum", vec![path.display().to_string()]);
    output_md5(program, &args)
}

/// 执行哈希命令并从输出里抠出 32 位十六进制 md5。
fn output_md5(program: &str, args: &[String]) -> Option<String> {
    let output = command(program).args(args).stderr(Stdio::piped()).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .find(|token| token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|token| token.to_ascii_lowercase())
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
    let _reader_handle = stderr.map(|stream| {
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

/// 需要跨「清空 + 解压」保留下来的运行时文件（相对安装目录）。
///
/// 用户定案：解压采用「先清空安装目录再解压」，但**配置文件不能跟着被清掉** ——
/// 那是用户手改过的 sol.yaml（监听端口、token、动作白名单）。日志/界面都要提前说清楚。
const SOL_PRESERVED_FILES: &[&str] = &["sol.yaml"];

/// 安装包文件名（小写比较）：`sol-vX.Y.Z-windows-amd64.zip` 之类。
///
/// 安装目录同时是**下载目录**，包和它的 `.md5` 旁车就躺在里面。清空前绝不能连包一起删 ——
/// 否则紧接着的解压无包可解，用户想重解一次也得重新下一遍。三端共用同一规则。
fn sol_is_archive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // 先剥掉校验旁车后缀，`sol-v0.1.0-windows-amd64.zip.md5` 和包本体同样保留
    let base = lower
        .strip_suffix(".md5")
        .or_else(|| lower.strip_suffix(".sha256"))
        .or_else(|| lower.strip_suffix(".sha1"))
        .unwrap_or(&lower);
    base.starts_with("sol-v")
        && (base.ends_with(".zip")
            || base.ends_with(".tar.gz")
            || base.ends_with(".tgz")
            || base.ends_with(".tar.xz")
            || base.ends_with(".txz"))
}

/// 解压 sol 安装包（tar.gz / zip）到指定目录，返回**安装目录**（包根目录）。
///
/// 三端统一行为（用户定案）：
///   1. 先**清空**安装目录（旧版本残留、上一次解压的 sol-v*/ 目录），避免新旧文件混在一起；
///   2. 保留 `sol.yaml`（用户配置）—— 清空后原样放回；
///   3. 解压后如果唯一顶层是 `sol-vX.Y.Z-<平台>/`，把它里面的**内容上移到安装目录**，
///      再删掉这层空壳。这样 `~/sol/` 下直接就是 sol(.exe)、install.sh(.cmd)、skills/，
///      不再是 `~/sol/sol-v0.1.0-windows-amd64/sol.exe`。
///
/// 解压**写入阶段**仍按包原样展开（不用 tar 的 --strip-components）：Windows 上那是
/// GNU tar 专有参数，bsdtar/libarchive 不认；而且先完整落地再上移，中途失败时目录里
/// 至少还有一份完整包可查。上移是解压成功之后的独立一步，三平台走同一段代码。
///
/// 官方脚本以「自己所在目录」为基准找二进制和 skills/，上移之后这一基准变成安装目录
/// 本身，脚本照常工作（install.sh 用 $0 所在目录，install.ps1 用 $PSScriptRoot）。
/// 清空安装目录，但把用户配置（`SOL_PRESERVED_FILES`）和**安装包本身**留下。
///
/// 安装目录同时是下载目录：`sol-vX.Y.Z-<平台>.zip` 和它的 `.md5` 旁车就躺在这里。
/// 清空时如果把包一起删了，紧接着的解压就无包可解（以前就是这么自毁的），
/// 而且用户想「重解一次」也得重新下一遍 —— 所以包 + 旁车必须在保留清单里。
///
/// 返回 (清掉的条目名, 保留的条目名)。目录本身不存在时返回空清单（不需要清）。
/// 只删目录/文件的**内容**，不删目录本身 —— 后面的解压还要往这里写。
fn sol_clear_install_dir(dir: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    if !dir.is_dir() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut removed: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();
    let entries = fs::read_dir(dir).map_err(|error| format!("读取安装目录 {} 失败：{error}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_file = path.is_file();
        // 用户配置：sol.yaml
        let preserved_config = is_file
            && SOL_PRESERVED_FILES
                .iter()
                .any(|preserved| name.eq_ignore_ascii_case(preserved));
        // 「怎么跑」：由 anyboot 在解压后重新生成，清空时先留着，生成失败也不至于丢
        let preserved_run = is_file && name.eq_ignore_ascii_case("install.yaml");
        // 安装包本体：sol-v*.zip / .tar.gz / .tar.xz，以及它们的 .md5 旁车
        let preserved_archive = is_file && sol_is_archive_name(&name);
        if preserved_config || preserved_run || preserved_archive {
            kept.push(name);
            continue;
        }
        let outcome = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match outcome {
            Ok(()) => removed.push(name),
            // 某个条目删不掉**不立刻中止**：继续清剩下的，最后统一汇总报告。
            // 这样用户一次就能看到「到底哪些东西挡住了」，而不是修一个再跑一次、
            // 一次只暴露一个问题（error 32 = 共享冲突，通常是进程把目录当 cwd 钉住了）。
            Err(error) => blocked.push(format!("{name}（{error}）")),
        }
    }
    if !blocked.is_empty() {
        return Err(format!(
            "清空安装目录 {} 失败，有 {} 项删不掉：{}\n\
             常见原因是进程正占用这些文件或把它们所在目录当工作目录：\n\
             · sol 服务/终端里还开着 install 脚本 —— 先停服务、关掉那个终端窗口\n\
             · 查占用进程：Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -like '*sol-v0.1.0*' }}\n\
             处理后重新点「解压到安装目录」即可（已删除的部分不用管）。",
            dir.display(),
            blocked.len(),
            blocked.join("；")
        ));
    }
    removed.sort();
    kept.sort();
    Ok((removed, kept))
}

/// 目录里是否「只有」唯一顶层目录 `sol-v*`，且它本身是个完整包。
/// 是的话返回这个子目录路径 —— 解压后要把它的内容上移到安装目录。
///
/// 注意：清空时**保留**下来的 `sol.yaml` 会留在同层，它不算障碍
/// （上移时逐项 rename，只会碰包里自己的文件，配置文件不受影响）；除此之外
/// 顶层不能有任何其它文件或第二个目录，否则不动 —— 避免误删/误盖别的东西。
fn sol_single_package_child(dir: &Path) -> Option<PathBuf> {
    // 安装目录本身已经是完整包（包直接铺在这一层）就不用再动
    if sol_package_complete(dir) {
        return None;
    }
    let entries: Vec<PathBuf> = fs::read_dir(dir).ok()?.flatten().map(|entry| entry.path()).collect();
    let mut package_dirs: Vec<PathBuf> = Vec::new();
    for path in &entries {
        if path.is_dir() {
            package_dirs.push(path.clone());
            continue;
        }
        // 文件：保留下来的配置文件、install.yaml、以及安装包本体 + .md5 旁车（安装目录也是下载目录）都是允许的
        let allowed = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| {
                SOL_PRESERVED_FILES
                    .iter()
                    .any(|preserved| name.eq_ignore_ascii_case(preserved))
                    || name.eq_ignore_ascii_case("install.yaml")
                    || sol_is_archive_name(name)
            })
            .unwrap_or(false);
        if !allowed {
            return None;
        }
    }
    if package_dirs.len() != 1 {
        return None;
    }
    let child = &package_dirs[0];
    let named_like_package = child
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with(SOL_PACKAGE_DIR_PREFIX))
        .unwrap_or(false);
    (named_like_package && sol_package_complete(child)).then(|| child.clone())
}

/// 把 `child` 的内容逐个移到 `parent`，成功后删掉空的 `child`。
///
/// 同名冲突时**直接替换**（`rename` 在 Unix 上语义如此；Windows 先删再移）。
/// 返回被上移的条目名。
fn sol_flatten_package_dir(parent: &Path, child: &Path) -> Result<Vec<String>, String> {
    let mut moved: Vec<String> = Vec::new();
    let entries = fs::read_dir(child).map_err(|error| format!("读取 {} 失败：{error}", child.display()))?;
    for entry in entries.flatten() {
        let from = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let to = parent.join(&name);
        // 上一轮留下的同名项先清掉，保证是「解压出来的新版本」而不是叠加
        if to.exists() {
            let _ = if to.is_dir() {
                fs::remove_dir_all(&to)
            } else {
                fs::remove_file(&to)
            };
        }
        fs::rename(&from, &to)
            .map_err(|error| format!("上移 {} → {} 失败：{error}", from.display(), to.display()))?;
        moved.push(name);
    }
    // 壳已经空了，删掉（删不掉不算失败：内容已经到位，只是留了个空目录）
    let _ = fs::remove_dir(child);
    moved.sort();
    Ok(moved)
}

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

    // 先清空安装目录（保留 sol.yaml + 安装包本身），再解压 —— 三端同一条路径。
    let (removed, kept) = sol_clear_install_dir(&dir)?;
    if removed.is_empty() && kept.is_empty() {
        sol_log(&app, "① 安装目录是空的，无需清理".to_string());
    } else {
        sol_log(
            &app,
            format!(
                "① 已清空安装目录{}（清掉 {} 项：{}）",
                dir.display(),
                removed.len(),
                if removed.is_empty() {
                    "无".to_string()
                } else {
                    removed.join("、")
                }
            ),
        );
        if !kept.is_empty() {
            // 分成两类说清楚：用户手改的配置 vs 安装包（包留着才能「重解一次」而不必重新下载）
            let (configs, archives): (Vec<&String>, Vec<&String>) = kept
                .iter()
                .partition(|name| sol_is_archive_name(name).eq(&false));
            if !configs.is_empty() {
                sol_log(
                    &app,
                    format!(
                        "① 已保留配置文件（不会被覆盖）：{}",
                        configs.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("、")
                    ),
                );
            }
            if !archives.is_empty() {
                sol_log(
                    &app,
                    format!(
                        "① 已保留安装包（可重复解压，无需重新下载）：{}",
                        archives.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("、")
                    ),
                );
            }
        }
    }

    // tar.gz：Windows 10+ 自带的 bsdtar 与 GNU tar 都认 -xzf（**不带** --strip-components，
    // 那是 GNU 专有参数，bsdtar 会报 unrecognized option）。
    // zip：Windows 走 Expand-Archive，其它平台 unzip。
    let output = if archive
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
    {
        if cfg!(target_os = "windows") {
            // 路径里的单引号按 PowerShell 规则翻倍，避免拼坏命令
            let quote = |path: &Path| path.display().to_string().replace('\'', "''");
            command("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &format!(
                        "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                        quote(&archive),
                        quote(&dir)
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
            .arg("-xzf")
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

    // 解压成功：如果唯一顶层是 sol-v*/，把内容上移到安装目录（三端同一条逻辑），
    // 得到「~/sol/ 下直接是 sol(.exe)、install.sh(.cmd)、skills/」的布局。
    if let Some(child) = sol_single_package_child(&dir) {
        let moved = sol_flatten_package_dir(&dir, &child)?;
        sol_log(
            &app,
            format!(
                "② 已把 {} 的内容上移到安装目录（{} 项：{}）",
                child.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
                moved.len(),
                moved.join("、")
            ),
        );
    }

    // 官方包根目录 = 安装目录本身，或（保底）唯一的顶层 sol-v*/。
    // 找不到就把实际解出来的东西列出来，报错信息直接指路而不是只说「不符」。
    if sol_package_complete(&dir) {
        sol_log(&app, format!("解压完成：安装目录 {}", dir.display()));
        return Ok(dir.display().to_string());
    }
    if let Some(root) = sol_find_package_dir(&dir) {
        sol_log(&app, format!("解压完成：包根目录 {}", root.display()));
        return Ok(root.display().to_string());
    }
    let mut listing: Vec<String> = fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if entry.path().is_dir() {
                        format!("{name}/")
                    } else {
                        name
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    listing.sort();
    Err(format!(
        "{} 里没有找到官方包（需要 {} + {}，通常在 sol-v*/ 目录下）。实际解出来的内容：{}",
        dir.display(),
        sol_package_bin(),
        sol_package_script(),
        if listing.is_empty() {
            "（空目录）".to_string()
        } else {
            listing.join("、")
        }
    ))
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
pub(crate) fn sol_http_get(app: AppHandle, path: Option<String>) -> Result<SolHttp, String> {
    trace(&app, "command", &format!("sol_http_get path={path:?}"));
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
            .ok_or_else(|| "找不到 sol 配置文件（服务没在跑，安装目录里也没有 sol.yaml）".to_string()),
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
///
/// 落盘经 sol_config_write（pkexec 子进程），走 async + spawn_blocking（blocking_guard 守卫）。
#[tauri::command]
pub(crate) async fn sol_http_set(
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
    // 复用 sol_config_write 的落盘与备份逻辑（含 pkexec 提权）；落盘在阻塞池里做
    let _ = &app;
    tauri::async_runtime::spawn_blocking(move || sol_config_write_blocking(resolved, updated))
        .await
        .map_err(|error| format!("sol_http_set 任务失败：{error}"))?
}

/// 生成一串随机 token（64 位十六进制，与 getrandom 复用同一来源）。
#[tauri::command]
pub(crate) fn sol_token_generate() -> String {
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

/// 一键下载安装（静默）：下载 → 清空安装目录（保留 sol.yaml）→ 解压铺平 → 官方脚本无人值守安装。
/// 中途任何一步失败即停（已下载的包保留，重试时解压可直接复用）。
///
/// 「一键安装（调终端）」的流程 = 前两步一样，第三步换成 sol_install_terminal，
/// 由前端分两个按钮各自串。
#[tauri::command]
pub(crate) async fn sol_one_click_install(app: AppHandle, dir: Option<String>) -> Result<String, String> {
    trace(&app, "command", "sol_one_click_install");
    let _guard = SolOpGuard::acquire()?;
    clean_orphan_install_lock(&app);
    sol_log(&app, "＝＝＝ 一键下载安装开始 ＝＝＝".to_string());

    // 1) 下载（进度条独立走事件）
    let archive = sol_install_download(app.clone(), dir.clone()).await.map_err(|error| {
        sol_log(&app, format!("一键安装中止（下载失败）：{error}"));
        error
    })?;
    sol_log(&app, format!("① 下载完成 {archive}"));

    // 2) 清空 + 解压：sol_extract 内部先清空安装目录（保留 sol.yaml），
    //    再把包内容上移铺平 —— 最终 ~/sol/ 下直接是 sol(.exe)、install.sh(.cmd)、skills/
    sol_log(&app, format!("② 开始解压到 {}", resolve_sol_dir_raw(&dir).display()));
    let root = sol_extract(app.clone(), archive, Some(resolve_sol_dir_raw(&dir).display().to_string()))
        .await
        .map_err(|error| {
            sol_log(&app, format!("一键安装中止（解压失败）：{error}"));
            error
        })?;
    sol_log(&app, format!("② 解压完成 {root}"));

    // 3) 静默安装（官方脚本无人值守）
    sol_log(&app, "③ 静默安装（官方 install.sh，可能弹一次管理员授权框）".to_string());
    let message = sol_install_silent_inner(app.clone(), Some(root), _guard).await.map_err(|error| {
        sol_log(&app, format!("一键安装中止（安装失败）：{error}"));
        error
    })?;
    sol_log(&app, "＝＝＝ 一键下载安装完成 ＝＝＝".to_string());
    Ok(message)
}

/// 一键安装（调终端）：下载 → 清空 + 解压铺平 → 呼出系统终端跑官方脚本。
/// 第三步由用户在终端里回答脚本问句（菜单选 1 = apply）。
#[tauri::command]
pub(crate) async fn sol_one_click_install_terminal(
    app: AppHandle,
    dir: Option<String>,
) -> Result<String, String> {
    trace(&app, "command", "sol_one_click_install_terminal");
    let _guard = SolOpGuard::acquire()?;
    clean_orphan_install_lock(&app);
    sol_log(&app, "＝＝＝ 一键安装（调终端）开始 ＝＝＝".to_string());

    let archive = sol_install_download(app.clone(), dir.clone()).await.map_err(|error| {
        sol_log(&app, format!("一键安装中止（下载失败）：{error}"));
        error
    })?;
    sol_log(&app, format!("① 下载完成 {archive}"));

    let target = resolve_sol_dir_raw(&dir);
    sol_log(&app, format!("② 开始解压到 {}", target.display()));
    let root = sol_extract(app.clone(), archive, Some(target.display().to_string()))
        .await
        .map_err(|error| {
            sol_log(&app, format!("一键安装中止（解压失败）：{error}"));
            error
        })?;
    sol_log(&app, format!("② 解压完成 {root}"));

    sol_log(&app, "③ 呼出终端跑官方 install.sh（请在终端里确认）".to_string());
    let message = sol_install_terminal_inner(app.clone(), Some(root), _guard).await?;
    sol_log(&app, "＝＝＝ 一键安装（调终端）完成 ＝＝＝".to_string());
    Ok(message)
}

/// 安装目录的原始解析（不做存在性校验）：一键下载/解压时目录还没有内容，
/// 允许不存在；安装/卸载前由 resolve_sol_dir 做完整校验。
fn resolve_sol_dir_raw(dir: &Option<String>) -> PathBuf {
    let raw = dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| sol_download_dir_default().display().to_string());
    expand_home_dir(&raw)
}

/// 解析安装目录：输入框的值（支持 ~ 展开与相对路径）；留空 = 默认 ~/sol。
/// **不做任何自动探测**（/usr/local/software、/opt 那套已删）：用户填哪儿就是哪儿。
fn resolve_sol_dir(dir: Option<String>) -> Result<PathBuf, String> {
    let raw = dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| {
            sol_download_dir_default()
                .display()
                .to_string()
        });
    let path = expand_home_dir(&raw);
    if !path.is_dir() {
        return Err(format!(
            "安装目录 {} 不存在（先下载/解压，或检查目录填写）",
            path.display()
        ));
    }
    Ok(path)
}

/// 确认安装目录里有平台对应的官方脚本与二进制（安装/卸载全靠脚本）。
/// 传进来的是「解压后返回的包根目录」，不是父目录：`sol-vX.Y.Z-<平台>/` 本身。
fn require_official_package(dir: &Path) -> Result<(), String> {
    for name in [sol_package_script(), sol_package_bin()] {
        if !dir.join(name).is_file() {
            return Err(format!(
                "{} 里没有 {}（官方包必须完整：{} + {}，通常在 sol-v*/ 目录下）",
                dir.display(),
                name,
                sol_package_bin(),
                sol_package_script()
            ));
        }
    }
    Ok(())
}

/// 官方安装脚本的完整路径（Windows = install.cmd，其它 = install.sh）。
fn sol_install_script_path(dir: &Path) -> PathBuf {
    dir.join(sol_package_script())
}

/// 在系统终端里呼出一条命令（用户能看到输出、能回答脚本的交互问答）。
///
/// - macOS：AppleScript 让 Terminal 新开窗口执行（`open -a Terminal` 无法带命令）
/// - Windows：`start` 一个新的 cmd 窗口，`/k` 保持窗口不关
/// - Linux：按常见的终端模拟器依次尝试
#[cfg(unix)]
fn spawn_terminal(dir: &Path, command_line: &str) -> Result<(), String> {
    let dir_display = dir.display().to_string();
    #[cfg(target_os = "macos")]
    {
        // AppleScript 字符串层转义：\ 和 " 之外原样（路径与命令由我们拼，不含怪字符）
        let escaped = command_line.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "tell application \"Terminal\"\n  activate\n  do script \"cd '{}' && {}\"\nend tell",
            dir_display.replace('"', "\\\""),
            escaped
        );
        let status = command("osascript")
            .args(["-e", &script])
            .status()
            .map_err(|error| format!("呼出 Terminal 失败：{error}"))?;
        if !status.success() {
            return Err("呼出 Terminal 失败（osascript 非零退出）".to_string());
        }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let full = format!("cd '{dir_display}' && {command_line}; echo '（脚本已退出，可关闭此窗口）'; exec $SHELL");
        let xfce = format!("bash -c \"{full}\"");
        for terminal in [
            ["gnome-terminal", "--", "bash", "-c", &full],
            ["konsole", "-e", "bash", "-c", &full],
            ["xfce4-terminal", "-e", &xfce, "", ""],
            ["x-terminal-emulator", "-e", &xfce, "", ""],
        ] {
            let program = terminal[0];
            if command(program).arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
                let status = command(program)
                    .args(&terminal[1..])
                    .spawn()
                    .map_err(|error| format!("呼出 {program} 失败：{error}"))?;
                let _ = status;
                return Ok(());
            }
        }
        Err("没找到可用的终端模拟器（gnome-terminal / konsole / xfce4-terminal）".to_string())
    }
}

/// 跑官方安装脚本时用的工作目录：安装目录的**父目录**（不是安装目录本身）。
///
/// 为什么不能用安装目录本身：官方 `install.ps1:512-516` 有一段 legacy 配置迁移判定，
/// 把 `Join-Path (Get-Location).Path 'sol.yaml'`（也就是 `cwd\sol.yaml`）也列进了
/// 「老位置」清单，一旦命中就执行 `install.ps1:537`：
///
/// ```powershell
/// Say "运行配置   改到用户目录：$RunConfig$(if (...) {...} else { '' }；你另外指定的路径不受影响)"
/// ```
///
/// 全角 `；你另外指定的路径不受影响` 落在 `$( ... )` 子表达式**内部**，PowerShell 会把它
/// 当成命令名去执行，于是整个安装以
/// `无法将"；你另外指定的路径不受影响"项识别为 cmdlet ...` + exit 1 崩掉。
///
/// 默认安装目录正好是 `%USERPROFILE%\sol`，而 `$RunConfig` 也是
/// `%USERPROFILE%\sol\sol.yaml` —— cwd 设成安装目录时 `cwd\sol.yaml` 恰好命中，
/// 所以「在安装目录里跑安装脚本」是必崩的（官方双击 install.cmd 同样会踩到）。
///
/// 这是上游脚本的 bug，我们不改脚本内容（用户定案），改用 cwd 绕开：父目录
/// `%USERPROFILE%` 下没有同名 `sol.yaml`，判定不命中，脚本正常跑完。脚本自己用
/// `$PSScriptRoot` 找 `sol.exe`（`install.ps1:151-157`），install.yaml / sol.yaml 的
/// 位置也都由 `$RunDir` 决定，与 cwd 无关，所以换 cwd 不影响它认出安装包。
#[cfg(target_os = "windows")]
fn sol_installer_cwd(dir: &Path) -> PathBuf {
    match dir.parent() {
        Some(parent) if !parent.as_os_str().is_empty() && parent != dir => parent.to_path_buf(),
        // 安装目录是盘符根（没有可用的父目录）时的兜底：退回系统临时目录，
        // 只要不是安装目录本身，就不会命中那段 legacy 判定。
        _ => std::env::temp_dir(),
    }
}

/// Windows 呼出 cmd 窗口跑 install.cmd（官方双击入口，它自己调 install.ps1）。
/// 交互模式不给环境变量：脚本走正常交互流程，用户在窗口里按菜单选。
///
/// cwd 用 `sol_installer_cwd`（父目录）而不是安装目录 —— 见那里的注释；
/// 也正因为 cwd 不在安装目录，command_line 必须是脚本的**绝对路径**。
#[cfg(target_os = "windows")]
fn spawn_terminal(dir: &Path, command_line: &str) -> Result<(), String> {
    // 与上午可用的基线同构：`start "" cmd /K cd /d <cwd> && <脚本>`。
    // 与基线唯一的不同是 cwd = 安装目录的**父目录**（sol_installer_cwd，绕
    // install.ps1:537 的 legacy 判定崩溃），因此 command_line 必须是绝对路径。
    // 注意不要在这里加引号/改用 /D：嵌套引号会被 cmd 剥错层（实测报
    // `'"cd /d "C:\..."' 不是内部或外部命令`），基线的不带引号形式反而能跑
    // —— 默认路径 C:\Users\<user>\sol 都不含空格。
    let cwd = sol_installer_cwd(dir);
    let status = command("cmd")
        .args([
            "/C",
            "start",
            "",
            "cmd",
            "/K",
            &format!("cd /d {} && {}", cwd.display(), command_line),
        ])
        .status()
        .map_err(|error| format!("呼出 cmd 失败：{error}"))?;
    if !status.success() {
        return Err("呼出 cmd 失败（start 非零退出）".to_string());
    }
    Ok(())
}

/// 官方 install.sh 的互斥锁路径（脚本自己创建：进锁的 pid 写在目录里）。
const SOL_INSTALL_LOCK: &str = "/tmp/sol-install.lock";

/// 进程内安装/卸载互斥：多个终端/静默安装并发时只放一个进去（官方脚本
/// 自己还有文件锁兜底，这里管的是「本面板别自己打自己」）。AtomicBool
/// compare_exchange：跨 await 持有安全（MutexGuard 不是 Send 不能跨 await）。
static SOL_OP_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 尝试占住安装标志。true = 占到（用完必须 release）；false = 已有操作在跑。
fn sol_op_try_acquire() -> bool {
    SOL_OP_RUNNING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_ok()
}

/// 释放安装标志（Drop 守卫，panic 路径也不漏）。
struct SolOpGuard;
impl SolOpGuard {
    fn acquire() -> Result<Self, String> {
        if sol_op_try_acquire() {
            Ok(SolOpGuard)
        } else {
            Err("另一个安装/卸载操作正在进行中（刚点过？），等它完成或关闭已弹出的终端窗口再试".to_string())
        }
    }
}
impl Drop for SolOpGuard {
    fn drop(&mut self) {
        SOL_OP_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 清理官方脚本留下的孤儿锁：锁目录存在但 pid 已死 → 删掉（脚本上次被
/// 强杀/终端直接关闭时残留，不清掉后续安装永远报「另一个安装/卸载正在进行」）。
fn clean_orphan_install_lock(app: &AppHandle) -> bool {
    let lock_dir = std::path::Path::new(SOL_INSTALL_LOCK);
    if !lock_dir.exists() {
        return false;
    }
    let pid_alive = fs::read_to_string(lock_dir.join("pid"))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .map(|pid| {
            // pid 存活 = /proc/<pid> 存在（Linux）；其它平台保守当作存活
            #[cfg(target_os = "linux")]
            {
                std::path::Path::new(&format!("/proc/{pid}")).exists()
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = pid;
                true
            }
        })
        // 读不到 pid：当存活处理（宁可让用户手动 rm，不误删别人的锁）
        .unwrap_or(true);
    if !pid_alive {
        let _ = fs::remove_dir_all(lock_dir);
        sol_log(app, format!("检测到上次安装的孤儿锁（进程已退出），已自动清理 {SOL_INSTALL_LOCK}"));
        return true;
    }
    false
}

/// 呼出系统终端，在安装目录里跑官方安装脚本（交互模式）。
/// 用户在弹出的终端里按脚本提示确认（菜单选 1 = apply）；脚本管提权、装服务、
/// 生成配置 —— AnyBoot 只负责「把终端开到正确目录并带上命令」。
#[tauri::command]
pub(crate) async fn sol_install_terminal(
    app: AppHandle,
    install_dir: Option<String>,
) -> Result<String, String> {
    trace(&app, "command", "sol_install_terminal");
    let _guard = SolOpGuard::acquire()?;
    clean_orphan_install_lock(&app);
    sol_install_terminal_inner(app, install_dir, _guard).await
}

/// 实体（不带锁）：一键终端流程已持锁时直接调这里。
pub(crate) async fn sol_install_terminal_inner(
    app: AppHandle,
    install_dir: Option<String>,
    _guard: SolOpGuard,
) -> Result<String, String> {
    let dir: PathBuf = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        let dir = resolve_sol_dir(install_dir.clone())?;
        require_official_package(&dir)?;
        Ok(dir)
    })
    .await
    .map_err(|error| format!("sol_install_terminal 任务失败：{error}"))??;

    // Windows：走 install.cmd 的**绝对路径** —— spawn_terminal 把 cwd 放在安装目录的
    // 父目录（绕开 install.ps1 的 legacy 判定，见 sol_installer_cwd），相对名会找不到文件。
    #[cfg(target_os = "windows")]
    let command_line = sol_install_script_path(&dir).display().to_string();
    #[cfg(not(target_os = "windows"))]
    let command_line = format!("sh {}", sol_package_script());

    spawn_terminal(&dir, &command_line)?;
    let message = format!(
        "已在系统终端打开官方安装入口 install.cmd（{}）—— 按脚本提示确认即可（菜单选 1 = 安装/升级）",
        dir.display()
    );
    sol_log(&app, message.clone());
    record_action(&app, "sol_install_terminal", true, &message);
    Ok(message)
}

/// 静默安装：官方 install.sh 的无人值守模式（官方注释明确支持 `printf 'y\n' | sh`）。
///
/// 关键细节：脚本内部需要 root 的动作走 **sudo**（要 tty 问密码）—— 无终端的
/// 静默模式下 sudo 必炸（实测 `sudo: a terminal is required`）。macOS 上改为
/// 先经 osascript 弹一次管理员授权框、**以 root 身份**跑整条安装命令：
/// root 下脚本检测 `UID_NOW=0` 自行跳过 sudo，stdin 答案照常管道喂入。
/// Linux 有 pkexec/终端 sudo 的场景不受影响，仍按原路径跑。
#[tauri::command]
pub(crate) async fn sol_install_silent(
    app: AppHandle,
    install_dir: Option<String>,
) -> Result<String, String> {
    trace(&app, "command", "sol_install_silent");
    let _guard = SolOpGuard::acquire()?;
    clean_orphan_install_lock(&app);
    sol_install_silent_inner(app, install_dir, _guard).await
}

/// 实体（不带锁）：一键静默流程已持锁时直接调这里。
pub(crate) async fn sol_install_silent_inner(
    app: AppHandle,
    install_dir: Option<String>,
    _guard: SolOpGuard,
) -> Result<String, String> {
    sol_log(&app, "＝＝＝ 静默安装开始 ＝＝＝".to_string());
    #[cfg(target_os = "macos")]
    let app_handle = app.clone();
    let message = tauri::async_runtime::spawn_blocking(move || {
        let dir = resolve_sol_dir(install_dir.clone())?;
        require_official_package(&dir)?;
        // Windows 是 install.cmd，其它平台是 install.sh
        let script = sol_install_script_path(&dir);

        // 答案流：菜单选 1（apply）；脚本遇到其它问句会再读 stdin —— 多喂几个 y
        const ANSWERS: &[u8] = b"1\ny\ny\ny\n";

        // Windows：走官方 install.cmd 入口（环境变量传动作：无人值守、脚本自己提权）
        #[cfg(target_os = "windows")]
        let stdout = run_windows_installer(&dir, &script, "install", "yes")?;

        // 两个平台各产出 stdout：macOS = osascript 提权跑（root 下脚本跳过 sudo）；
        // 其它 = 直接跑（root 由 pkexec/终端 sudo 场景自理）。
        #[cfg(target_os = "macos")]
        let stdout = {
            // root 跑脚本 + cd 到安装目录（sol.yaml / install.yaml 生成在 cwd）+
            // printf 答案流管道喂给 sh。整条经 osascript administrator privileges。
            // 路径不含单引号（resolve_sol_dir 未校验这点，含则明确拒绝）。
            let dir_display = dir.display().to_string();
            if dir_display.contains('\'') {
                return Err("安装目录里包含单引号，无法提权执行（换个不含 ' 的目录）".to_string());
            }
            // 安装完顺手把 sol.yaml 放宽到 0644（同一次 root 授权内完成，不额外弹框）：
            // 官方脚本以 root 生成 0600 配置，不放开的话之后每次读取都要弹密码。
            // 用户定案：安装时直接生成可读配置（权衡：本机其它登录用户也能读到 token）。
            let config_path = dir.join("sol.yaml");
            let inner = format!(
                "cd '{}' && printf '1\\ny\\ny\\ny\\n' | sh '{}'; rc=$?; [ -f '{}' ] && chmod 0644 '{}' || true; exit $rc",
                dir_display,
                script.display(),
                config_path.display(),
                config_path.display()
            );
            // do shell script 的 AppleScript 字符串层只需转义 \ 和 "
            let escaped = inner.replace('\\', "\\\\").replace('"', "\\\"");
            let wrapped = format!(
                "do shell script \"{}\" with administrator privileges",
                escaped
            );
            let output = command("osascript")
                .args(["-e", &wrapped])
                .stderr(Stdio::piped())
                .output()
                .map_err(|error| format!("启动 osascript 失败：{error}"))?;
            // 脚本的完整输出落到 trace：安装失败/服务没注册时，这是唯一能查的现场
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            for line in stdout.lines().chain(stderr.lines()) {
                let line = line.trim();
                if !line.is_empty() {
                    trace(&app_handle, "sol-install", &line.chars().take(300).collect::<String>());
                }
            }
            if !output.status.success() {
                let canceled = stderr.contains("-128") || stderr.contains("User canceled");
                return Err(if canceled {
                    "提权被取消（点了取消），未做任何修改".to_string()
                } else {
                    format!(
                        "静默安装失败：{}",
                        stderr.trim().chars().take(400).collect::<String>()
                    )
                });
            }
            stdout
        };
        // 非 macOS / 非 Windows：直接跑（root 由 pkexec/终端 sudo 场景自理）
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let stdout = {
            let output = command("sh")
                .arg(&script)
                .current_dir(&dir)
                .stdin(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write as _;
                    if let Some(stdin) = child.stdin.as_mut() {
                        let _ = stdin.write_all(ANSWERS);
                    }
                    child.wait_with_output()
                })
                .map_err(|error| format!("运行 {} 失败：{error}", sol_package_script()))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "静默安装失败（exit {}）：{}",
                    output.status.code().unwrap_or(-1),
                    stderr.trim().chars().take(400).collect::<String>()
                ));
            }
            String::from_utf8_lossy(&output.stdout).into_owned()
        };

        // 脚本成功收尾（打二维码/完成提示）或至少把二进制装上了就算成。
        // 三个分支各自产出 stdout（macOS = osascript，Windows = install.cmd，其它 = 直接跑）。
        let bin = sol_installed_bin_path();
        if !bin.is_file() {
            let tail: String = stdout
                .lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            return Err(format!(
                "脚本退出但 {} 不在 —— 安装可能被取消。输出末尾：{tail}",
                bin.display()
            ));
        }
        // Windows 上官方 install.ps1 用 PS 5.1 默认 ANSI 写 sol.yaml，中文注释会
        // 双重编码乱码 —— 上游 bug，等上游修复（anyboot 不做编码兜底）。
        #[cfg(target_os = "windows")]
        let _ = &dir;
        Ok(format!(
            "已安装：{}（配置 {}）",
            bin.display(),
            sol_installed_config_path(&dir, &bin).display()
        ))
    })
    .await
    .map_err(|error| format!("sol_install_silent 任务失败：{error}"))??;
    sol_log(&app, format!("＝＝＝ 静默安装完成 ＝＝＝ {message}"));
    record_action(&app, "sol_install_silent", true, &message);
    Ok(message)
}

/// 安装完成后真正的配置文件路径。
///
/// 唯一权威：安装目录 `install.yaml` 里的 `--config`（官方脚本写的「怎么跑」）。
/// 没有 install.yaml 时：Linux/macOS 用 `<安装目录>/sol.yaml`，
/// Windows 用 `%USERPROFILE%\sol\sol.yaml`（官方分工：二进制进 ProgramData、
/// 配置进用户目录 —— 绝不能拿 bin.parent() 猜，那会指向 ProgramData）。
fn sol_installed_config_path(install_dir: &Path, bin: &Path) -> PathBuf {
    if let Some(config) = sol_install_yaml_config(install_dir) {
        return config;
    }
    #[cfg(target_os = "windows")]
    {
        let _ = bin;
        sol_install_dir_from_env()
            .unwrap_or_else(|| install_dir.to_path_buf())
            .join("sol.yaml")
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = bin;
        install_dir.join("sol.yaml")
    }
}

/// 安装完成后 sol 二进制的落点（官方布局）。
/// Linux/macOS：`~/sol/bin/sol`；Windows：`%ProgramData%\sol\sol.exe`
/// （可用 SOL_INSTALL_ROOT 改根，官方 ps1 也认这个变量）。
fn sol_installed_bin_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        // Windows 二进制仍在 ProgramData（SYSTEM 计划任务需要），不在用户目录。
        // SOL_INSTALL_ROOT 是官方 ps1 自己认的沙箱变量（多实例测试用），不是旧版兼容。
        let root = std::env::var("SOL_INSTALL_ROOT")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var("ProgramData").ok().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        root.join("sol").join("sol.exe")
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Linux/macOS：~/sol/bin/sol
        sol_install_dir_from_env()
            .unwrap_or_else(|| PathBuf::from("/tmp/sol"))
            .join("bin")
            .join("sol")
    }
}

/// Windows：无人值守跑官方 **install.cmd**，返回脚本输出（stdout + stderr 合并）。
///
/// 为什么调 .cmd 而不是 .ps1：.cmd 是官方给用户双击的入口，它自己用
/// `powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"` 去调 .ps1
/// —— PowerShell 版本探测、执行策略、路径引号这些封装在里面。anyboot 走官方入口，
/// 而不是绕过它自己重做一遍。
///
/// **动作用环境变量传，不是命令行参数**：官方 install.cmd 不带 `%*`（它明确写着
/// no arguments to remember），给它加参数会被整段丢掉。install.ps1 自己留了后门
/// （install.ps1:1195-1204）：参数没给时回落到 `$env:SOL_INSTALL_ACTION` /
/// `SOL_INSTALL_SERVICE` / `SOL_INSTALL_DELCONFIGS`。经环境变量进去，.cmd 一个字都不用改，
/// 效果跟 `-SolAction install -SolService yes` 完全一致：走「已升权子进程」分支，
/// 不问、不展示、直接执行，需要管理员时脚本自己弹一次 UAC，结束 exit 0。
///
/// stdin 立刻关掉（EOF）：install.cmd 末尾的 `pause` 收到 EOF 立即返回，不会挂死；
/// 脚本若要提问也只拿到默认值。
#[cfg(target_os = "windows")]
fn run_windows_installer(dir: &Path, script: &Path, action: &str, service: &str) -> Result<String, String> {
    // /U 让 cmd 用 UTF-16 输出，中文不会被控制台代码页吃掉。
    // 动作用环境变量传（见上方说明）：install.cmd 不转交参数，但 install.ps1 认 SOL_INSTALL_*。
    let script_arg = script.display().to_string();
    let output = command("cmd")
        .args(["/U", "/C", script_arg.as_str()])
        .env("SOL_INSTALL_ACTION", action)
        .env("SOL_INSTALL_SERVICE", service)
        // cwd 不放在安装目录：install.ps1 的 legacy 判定会把
        // `cwd\sol.yaml` 与 `%USERPROFILE%\sol\sol.yaml` 撞上，命中后踩
        // install.ps1:537 的坏行直接 exit 1（实测安装动作 exit=1）。父目录
        // 没有同名 sol.yaml，判定不命中。脚本内部路径全用 $PSScriptRoot /
        // $RunDir，与 cwd 无关，换 cwd 无副作用。
        .current_dir(sol_installer_cwd(dir))
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            // stdin 立刻关掉（EOF）：.cmd 的 pause 与 .ps1 的 Read-Host 都立即返回默认值
            drop(child.stdin.take());
            child.wait_with_output()
        })
        .map_err(|error| format!("运行 {} 失败：{error}", sol_package_script()))?;
    // /U 的 stdout 是 UTF-16LE，去掉 BOM 再解码；万一没走 /U 则按 UTF-8 兜底。
    let stdout = decode_cmd_output(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        let detail: &str = if stderr.trim().is_empty() { &stdout } else { &stderr };
        return Err(format!(
            "静默安装失败（exit {}）：{}",
            output.status.code().unwrap_or(-1),
            detail.trim().chars().take(400).collect::<String>()
        ));
    }
    Ok(if stderr.trim().is_empty() {
        stdout
    } else {
        format!("{stdout}\n{stderr}")
    })
}

/// 解出 `cmd /U` 的 UTF-16LE 输出；不是 UTF-16 就按 UTF-8 处理。
#[cfg(target_os = "windows")]
fn decode_cmd_output(bytes: &[u8]) -> String {
    let body = bytes.strip_prefix(&[0xFF, 0xFE]).unwrap_or(bytes);
    let looks_utf16 = body.len() >= 2 && body.len() % 2 == 0 && body.iter().skip(1).step_by(2).all(|b| *b == 0);
    if looks_utf16 {
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(body).into_owned()
    }
}

/// 静默卸载：官方脚本的 uninstall 动作（管道喂「2」选菜单卸载项）。
/// macOS 上同样以 root 跑（osascript 提权，理由同 sol_install_silent 的注释）。
#[tauri::command]
pub(crate) async fn sol_uninstall_silent(
    app: AppHandle,
    install_dir: Option<String>,
) -> Result<String, String> {
    trace(&app, "command", "sol_uninstall_silent");
    let _guard = SolOpGuard::acquire()?;
    clean_orphan_install_lock(&app);
    // 先看安装目录里有没有官方包（install.sh / install.cmd）：可能安装包还没下载/解压，
    // 没有可执行的东西就别往下走（弹终端/跑脚本都会失败）
    let dir = resolve_sol_dir(install_dir.clone())?;
    if !sol_package_complete(&dir) {
        return Err(format!(
            "安装目录 {} 里没有官方包（{} + {}）—— 可能安装包还没下载或没解压，先「一键下载安装」",
            dir.display(),
            sol_package_bin(),
            sol_package_script()
        ));
    }
    sol_log(&app, "＝＝＝ 静默卸载开始 ＝＝＝".to_string());
    let message = tauri::async_runtime::spawn_blocking(move || {
        let dir = resolve_sol_dir(install_dir.clone())?;
        require_official_package(&dir)?;
        let script = sol_install_script_path(&dir);

        // Windows：同一个官方入口跑卸载（SOL_INSTALL_ACTION=uninstall 经环境变量转交 install.ps1）
        #[cfg(target_os = "windows")]
        run_windows_installer(&dir, &script, "uninstall", "yes")?;

        #[cfg(target_os = "macos")]
        {
            let dir_display = dir.display().to_string();
            if dir_display.contains('\'') {
                return Err("安装目录里包含单引号，无法提权执行（换个不含 ' 的目录）".to_string());
            }
            let inner = format!(
                "cd '{}' && printf '2\\ny\\ny\\ny\\n' | sh '{}'",
                dir_display,
                script.display()
            );
            let escaped = inner.replace('\\', "\\\\").replace('"', "\\\"");
            let wrapped = format!(
                "do shell script \"{}\" with administrator privileges",
                escaped
            );
            let output = command("osascript")
                .args(["-e", &wrapped])
                .stderr(Stdio::piped())
                .output()
                .map_err(|error| format!("启动 osascript 失败：{error}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let canceled = stderr.contains("-128") || stderr.contains("User canceled");
                return Err(if canceled {
                    "提权被取消（点了取消），未做任何修改".to_string()
                } else {
                    format!(
                        "静默卸载失败：{}",
                        stderr.trim().chars().take(400).collect::<String>()
                    )
                });
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let output = command("sh")
                .arg(&script)
                .current_dir(&dir)
                .stdin(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    use std::io::Write as _;
                    if let Some(stdin) = child.stdin.as_mut() {
                        // 菜单选 2（uninstall）；之后的确认问句也喂 y
                        let _ = stdin.write_all(b"2\ny\ny\ny\n");
                    }
                    child.wait_with_output()
                })
                .map_err(|error| format!("运行 {} 失败：{error}", sol_package_script()))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "静默卸载失败（exit {}）：{}",
                    output.status.code().unwrap_or(-1),
                    stderr.trim().chars().take(400).collect::<String>()
                ));
            }
        }
        let bin = sol_installed_bin_path();
        if bin.is_file() {
            return Err(format!(
                "脚本退出但 {} 还在 —— 卸载可能没执行（检查脚本输出）",
                bin.display()
            ));
        }
        Ok("已卸载：二进制与服务已由官方脚本移除（安装目录里的配置保留）".to_string())
    })
    .await
    .map_err(|error| format!("sol_uninstall_silent 任务失败：{error}"))??;
    sol_log(&app, format!("＝＝＝ 静默卸载完成 ＝＝＝ {message}"));
    record_action(&app, "sol_uninstall_silent", true, &message);
    Ok(message)
}

/// 呼出系统终端跑官方脚本的卸载（交互模式：菜单选 2 = uninstall）。
#[tauri::command]
pub(crate) async fn sol_uninstall_terminal(
    app: AppHandle,
    install_dir: Option<String>,
) -> Result<String, String> {
    trace(&app, "command", "sol_uninstall_terminal");
    let _guard = SolOpGuard::acquire()?;
    clean_orphan_install_lock(&app);
    // 先看安装目录里有没有官方包：可能安装包还没下载/解压，
    // 没有可执行的东西就别往下走（弹终端/跑脚本都会失败）
    let dir = resolve_sol_dir(install_dir.clone())?;
    if !sol_package_complete(&dir) {
        return Err(format!(
            "安装目录 {} 里没有官方包（{} + {}）—— 可能安装包还没下载或没解压，先「一键下载安装」",
            dir.display(),
            sol_package_bin(),
            sol_package_script()
        ));
    }
    let dir: PathBuf = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        let dir = resolve_sol_dir(install_dir.clone())?;
        require_official_package(&dir)?;
        Ok(dir)
    })
    .await
    .map_err(|error| format!("sol_uninstall_terminal 任务失败：{error}"))??;

    #[cfg(target_os = "windows")]
    // spawn_terminal 的 cwd 是安装目录的**父目录**（sol_installer_cwd），
    // 相对名 install.cmd 会在父目录里找不到文件 —— 必须给绝对路径。
    let command_line = sol_install_script_path(&dir).display().to_string();
    #[cfg(not(target_os = "windows"))]
    let command_line = format!("sh {}", sol_package_script());

    spawn_terminal(&dir, &command_line)?;
    let message = "已在系统终端打开安装脚本 —— 卸载请在菜单里选 2（uninstall）";
    sol_log(&app, message.to_string());
    record_action(&app, "sol_uninstall_terminal", true, message);
    Ok(message.to_string())
}

/// 服务启停（start/stop/restart）：macOS launchctl、Linux systemctl。
/// 官方脚本管安装期的注册，日常启停走这里（不必每次都开终端）。
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
    let (program, args): (String, Vec<String>) = if cfg!(target_os = "macos") {
        // bootstrap 过的 system 域服务：kickstart（-k = 先杀再拉起 = restart）、
        // kill SIGTERM = stop
        let sub = match action.as_str() {
            "start" => "launchctl kickstart system/com.lidiaoo.sol",
            "restart" => "launchctl kickstart -k system/com.lidiaoo.sol",
            _ => "launchctl kill SIGTERM system/com.lidiaoo.sol",
        };
        (
            "osascript".to_string(),
            vec![
                "-e".to_string(),
                format!("do shell script \"{sub}\" with administrator privileges"),
            ],
        )
    } else {
        (
            "systemctl".to_string(),
            vec![action.clone(), format!("{}.service", SOL_SERVICE)],
        )
    };
    let output = command(&program)
        .args(&args)
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("运行服务命令失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "sol 服务 {action} 失败：{}",
            stderr.trim().chars().take(200).collect::<String>()
        ));
    }
    let message = match action.as_str() {
        "start" => "sol 服务已启动",
        "stop" => "sol 服务已停止",
        _ => "sol 服务已重启",
    };
    record_action(&app, "sol_service_ctl", true, message);
    Ok(message.to_string())
}

/// 前端（主窗 webview）异常上报：WKWebView 的 console 我们看不到，
/// 「数据不加载」类问题必须让 JS 侧异常落到 trace.log 才能定位。
/// 放在 sol_panel 纯粹因为这里已经有 trace 落盘的样板，语义上它是 UI 诊断。
#[tauri::command]
pub(crate) fn sol_log_client_error(app: AppHandle, kind: String, detail: String) {
    trace(
        &app,
        "ui-error",
        &format!("[{kind}] {}", detail.chars().take(500).collect::<String>()),
    );
}

/// 打开 sol 安装目录（文件管理器）。给定了路径就用它；没给就找检测到的那个。
#[tauri::command]
pub(crate) fn sol_open_dir(app: AppHandle, dir: Option<String>) -> Result<String, String> {
    trace(&app, "command", "sol_open_dir");
    // 打开的就是「安装目录输入框」的语义：留空 = 默认 ~/sol。
    // 目录不存在就先建出来（用户就是想看/往里放东西），不再回退 /usr/local/bin。
    let resolved = resolve_sol_dir(dir)?;
    fs::create_dir_all(&resolved).map_err(|error| format!("创建目录失败：{error}"))?;
    open_directory_in_explorer(&resolved)?;
    Ok(resolved.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// install.yaml 是官方脚本写的「sol 怎么跑」的唯一权威来源：它指哪儿，anyboot 就读哪儿。
    #[test]
    fn install_yaml_config_path_is_parsed() {
        // 用工作区里的临时目录而不是系统 TEMP：CI / 沙箱下 TEMP 常被收紧成只读，
        // 同一个测试在别人机器上就红给你看。CARGO_MANIFEST_DIR 一定是可写的。
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("install-yaml-{}", std::process::id()));
        // Windows 的路径可能有多级还不存在：先建全路径，再往里写
        fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("install.yaml");

        // 官方脚本写出的标准形态（一行 args，逗号分隔）
        fs::write(
            &cfg,
            "# 由 install.ps1 生成：sol 怎么跑。改这里，然后重新运行一遍脚本。\nrun:\n  args: [listen, --config, C:\\Users\\x\\sol\\sol.yaml]\n",
        )
        .unwrap();
        assert_eq!(
            sol_install_yaml_config(&dir).as_deref(),
            Some(Path::new("C:\\Users\\x\\sol\\sol.yaml"))
        );

        // Unix 形态
        fs::write(&cfg, "run:\n  args: [listen, --config, /home/u/sol/sol.yaml]\n").unwrap();
        assert_eq!(
            sol_install_yaml_config(&dir).as_deref(),
            Some(Path::new("/home/u/sol/sol.yaml"))
        );

        // --config=X 也要认
        fs::write(&cfg, "run:\n  args: [listen, --config=/home/u/sol/sol.yaml]\n").unwrap();
        assert_eq!(
            sol_install_yaml_config(&dir).as_deref(),
            Some(Path::new("/home/u/sol/sol.yaml"))
        );

        // 没有 --config：返回 None，让调用方走惯例路径
        fs::write(&cfg, "run:\n  args: [listen]\n").unwrap();
        assert_eq!(sol_install_yaml_config(&dir), None);

        // 文件不存在：None 而不是 panic
        let _ = fs::remove_file(&cfg);
        assert_eq!(sol_install_yaml_config(&dir), None);

        let _ = fs::remove_dir_all(&dir);
    }

    /// 安装目录同时是下载目录：清空时安装包和它的校验旁车必须保住，否则解压无包可解。
    #[test]
    fn clear_install_dir_keeps_archive_and_md5() {
        for name in [
            "sol-v0.1.0-windows-amd64.zip",
            "sol-v0.1.0-windows-amd64.zip.md5",
            "sol-v0.1.0-linux-amd64.tar.gz",
            "sol-v0.1.0-linux-arm64.tar.gz.md5",
            "sol-v0.1.0-darwin-arm64.tar.xz",
            "SOL-V0.2.0-WINDOWS-AMD64.ZIP.MD5",
        ] {
            assert!(sol_is_archive_name(name), "应保留安装包：{name}");
        }
        // 下载中断留下的半截文件照删，别让残缺包永远赖在安装目录里
        for name in [
            "sol-v0.1.0-windows-amd64.zip.part",
            "install.ps1",
            "sol.exe",
            "sol.yaml",
            "soldier.zip",
            "contract.txt",
        ] {
            assert!(!sol_is_archive_name(name), "不该按安装包保留：{name}");
        }
    }

    #[test]
    fn exec_start_config_extracts_path() {
        let line = "ExecStart={ path=/home/u/sol/bin/sol ; argv[]=/home/u/sol/bin/sol listen --config /home/u/sol/sol.yaml ; ignore_errors=no ; start_time=[Mon 2026-10-19 10:55:08 CST] }";
        assert_eq!(
            exec_start_config(line).as_deref(),
            Some("/home/u/sol/sol.yaml")
        );
    }

    #[test]
    fn exec_start_config_supports_equals_form() {
        let line = "ExecStart={ path=/home/u/sol/bin/sol ; argv[]=/home/u/sol/bin/sol listen --config=/home/u/sol/sol.yaml }";
        assert_eq!(exec_start_config(line).as_deref(), Some("/home/u/sol/sol.yaml"));
    }

    #[test]
    fn exec_start_config_without_config_is_none() {
        let line = "ExecStart={ path=/home/u/sol/bin/sol ; argv[]=/home/u/sol/bin/sol listen }";
        assert_eq!(exec_start_config(line), None);
    }

    #[test]
    fn plist_config_argument_is_parsed_from_next_string() {
        // 真实 plist 片段：--config 与路径是两个独立的 <string> 行
        let plist = "<?xml version=\"1.0\"?>\n<plist><dict>\n  <key>ProgramArguments</key>\n  <array>\n    <string>/Users/x/sol/bin/sol</string>\n    <string>listen</string>\n    <string>--config</string>\n    <string>/Users/x/sol/sol.yaml</string>\n  </array>\n</dict></plist>";
        // 与 sol_status_snapshot 里同一套解析
        let mut lines = plist.lines().peekable();
        let mut found = None;
        while let Some(line) = lines.next() {
            if line.contains("--config") {
                if found.is_none() {
                    if let Some(next) = lines.peek() {
                        if let Some(value) = next.split("<string>").nth(1) {
                            found = Some(
                                value.split("</string>").next().unwrap_or("").trim().to_string(),
                            );
                        }
                    }
                }
                break;
            }
        }
        assert_eq!(found.as_deref(), Some("/Users/x/sol/sol.yaml"));
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
