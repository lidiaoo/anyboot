//! 挂载辅助：ActiveMount 生命周期、临时挂载、原子写。ActiveMount 被
//! AppState 直接持有（state.rs），字段可见性不要收窄。

use super::*;

#[derive(Clone)]
pub(crate) struct ActiveMount {
    pub(crate) device: PathBuf,
    pub(crate) mountpoint: PathBuf,
    pub(crate) owned: bool,
}

impl ActiveMount {
    pub(crate) fn unmount(self) -> io::Result<()> {
        if !self.owned {
            return Ok(());
        }
        // macOS 用 diskutil 卸（它自己挂的，也只能它自己卸干净）；其余用 umount
        #[cfg(target_os = "macos")]
        let result = {
            let mut cmd = command("diskutil");
            cmd.arg("unmount");
            if self.device.as_os_str().is_empty() {
                cmd.arg(&self.mountpoint);
            } else {
                cmd.arg(&self.device);
            }
            cmd.status()
        };
        #[cfg(not(target_os = "macos"))]
        let result = mount_command("umount").arg(&self.mountpoint).status();
        result.and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(io::Error::other("umount failed"))
            }
        })?;
        let _ = fs::remove_dir(&self.mountpoint);
        Ok(())
    }

    pub(crate) fn remount(&self, mode: &str) -> io::Result<()> {
        let output = mount_command("mount")
            .arg("-o")
            .arg(remount_options(mode))
            .arg(&self.device)
            .arg(&self.mountpoint)
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(
                String::from_utf8_lossy(&output.stderr).trim(),
            ))
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn mount_command(program: &str) -> Command {
    if is_root() {
        command(program)
    } else {
        let mut escalated = command("pkexec");
        escalated.arg(program);
        escalated
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn mount_command(program: &str) -> Command {
    command(program)
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_user_home_by_name(username: &str) -> Option<PathBuf> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.first() != Some(&username) {
            return None;
        }
        Some(PathBuf::from(fields.get(5)?.to_string()))
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_user_home_by_uid(uid: u32) -> Option<PathBuf> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.get(2).and_then(|value| value.parse::<u32>().ok()) != Some(uid) {
            return None;
        }
        Some(PathBuf::from(fields.get(5)?.to_string()))
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_invoking_user_home() -> Option<PathBuf> {
    if let Some(username) = std::env::var_os("SUDO_USER") {
        return linux_user_home_by_name(&username.to_string_lossy());
    }
    if let Ok(uid) = std::env::var("PKEXEC_UID") {
        return uid.parse::<u32>().ok().and_then(linux_user_home_by_uid);
    }
    None
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_mount_user_ids() -> Option<(u32, u32)> {
    if is_root() {
        return linux_invoking_user_home()
            .as_deref()
            .and_then(linux_user_ids_from_home);
    }
    Some(unsafe { (libc::getuid(), libc::getgid()) })
}

#[cfg(target_os = "linux")]
pub(crate) fn mount_options(mode: &str) -> String {
    match linux_mount_user_ids() {
        Some((uid, gid)) => format!("{mode},uid={uid},gid={gid}"),
        None => mode.to_string(),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn remount_options(mode: &str) -> String {
    format!("remount,{}", mount_options(mode))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn remount_options(mode: &str) -> String {
    format!("remount,{mode}")
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_user_ids_from_home(home: &Path) -> Option<(u32, u32)> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields = line.split(':').collect::<Vec<_>>();
        if fields.get(5) != Some(&home.to_string_lossy().as_ref()) {
            return None;
        }
        Some((fields.get(2)?.parse().ok()?, fields.get(3)?.parse().ok()?))
    })
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn mount_options(mode: &str) -> String {
    mode.to_string()
}

#[cfg(target_os = "linux")]
pub(crate) fn is_owned_mountpoint(mountpoint: &Path) -> bool {
    let temp_dir = std::env::temp_dir();
    mountpoint.starts_with(&temp_dir)
        && mountpoint
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("anyboot-"))
}

#[cfg(target_os = "linux")]
pub(crate) fn mounted_scan_candidates() -> Vec<MountedScanCandidate> {
    let Ok(mountinfo) = fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    for line in mountinfo.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let Some(mountpoint) = fields.get(4) else {
            continue;
        };
        let Some(options) = fields.get(5) else {
            continue;
        };
        let Some(separator) = fields.iter().position(|field| *field == "-") else {
            continue;
        };
        let Some(fstype) = fields.get(separator + 1) else {
            continue;
        };
        let Some(device) = fields.get(separator + 2) else {
            continue;
        };
        if !FAT_FILESYSTEMS.contains(fstype) {
            continue;
        }

        let mountpoint = decode_mountinfo_path(mountpoint);
        let vars_dir = mountpoint.join("EFI/refind/vars");
        let Ok(resolved) = vars_dir.canonicalize() else {
            continue;
        };
        if !resolved.is_dir()
            || candidates
                .iter()
                .any(|candidate: &MountedScanCandidate| candidate.vars_dir == resolved)
        {
            continue;
        }

        let read_only = options.split(',').any(|option| option == "ro");
        let owned = is_owned_mountpoint(&mountpoint);
        let active_mount = if owned || read_only {
            Some(ActiveMount {
                device: decode_mountinfo_path(device),
                mountpoint,
                owned,
            })
        } else {
            None
        };
        candidates.push(MountedScanCandidate {
            vars_dir: resolved,
            active_mount,
        });
    }
    candidates
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn mounted_scan_candidates() -> Vec<MountedScanCandidate> {
    mounted_candidate_directories()
        .into_iter()
        .map(|vars_dir| MountedScanCandidate {
            vars_dir,
            active_mount: None,
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub(crate) fn active_mount_for_directory(directory: &Path, resolved_directory: &Path) -> Option<ActiveMount> {
    mounted_scan_candidates()
        .into_iter()
        .filter_map(|candidate| candidate.active_mount)
        .find(|mount| {
            resolved_directory.starts_with(&mount.mountpoint)
                || directory.starts_with(&mount.mountpoint)
        })
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn active_mount_for_directory(
    _directory: &Path,
    _resolved_directory: &Path,
) -> Option<ActiveMount> {
    None
}

pub(crate) fn write_with_mount<F>(active_mount: Option<&ActiveMount>, operation: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    if let Some(mount) = active_mount.as_ref() {
        mount
            .remount("rw")
            .map_err(|error| format!("无法将分区重新挂载为可写: {error}"))?;
    }
    let result = operation();
    if let Some(mount) = active_mount.as_ref() {
        let _ = mount.remount("ro");
    }
    result
}

pub(crate) fn temporary_target(target: &Path) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    target
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".{}.{}.{}.tmp",
            target
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("target"),
            std::process::id(),
            unique
        ))
}

pub(crate) fn atomic_copy(source: &Path, target: &Path) -> io::Result<()> {
    let temporary = temporary_target(target);
    let result = (|| -> io::Result<()> {
        fs::copy(source, &temporary)?;
        let file = fs::OpenOptions::new().write(true).open(&temporary)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[tauri::command]
pub(crate) fn unmount_temporary(app: AppHandle) -> AppStatus {
    set_busy(&app, |busy| busy.unmounting = true);
    let status = unmount_temporary_inner(&app);
    set_busy(&app, |busy| busy.unmounting = false);
    status
}

pub(crate) fn unmount_temporary_inner(app: &AppHandle) -> AppStatus {
    let mount = {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state
            .active_mount
            .take_if(|mount| mount.owned)
            .map(|mount| mount.clone())
    };

    let Some(mount) = mount else {
        {
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            state.message = "没有应用创建的临时挂载".to_string();
            state.error = None;
        }
        let status = state_snapshot(app);
        publish_status(app, &status);
        return status;
    };

    let mountpoint = mount.mountpoint.clone();

    let status = match mount.clone().unmount() {
        Ok(()) => {
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            state.vars_dir = None;
            state.variables = Vec::new();
            state.message = "已卸载应用创建的临时挂载".to_string();
            state.error = None;
            AppStatus {
                vars_dir: None,
                temporary_mount: false,
                variables: Vec::new(),
                message: state.message.clone(),
                error: None,
            }
        }
        Err(error) => {
            let message = "卸载临时挂载失败".to_string();
            let detail = format!("无法卸载 {}: {error}", mountpoint.display());
            {
                // 失败要把挂载状态放回去，同时把提示写进 AppState，
                // 否则主窗口读到的还是上一次的成功文案（阶段 1 修同类不一致）。
                let state = app.state::<Mutex<AppState>>();
                let mut state = state.lock().expect("state mutex poisoned");
                state.active_mount = Some(mount);
                state.message = message.clone();
                state.error = Some(detail.clone());
            }
            let mut status = state_snapshot(app);
            status.message = message;
            status.error = Some(detail);
            status
        }
    };
    publish_status(app, &status);
    status
}

pub(crate) fn mounted_candidate_directories() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let root = format!("{}:\\", letter as char);
            push_unique(&mut candidates, PathBuf::from(root).join("EFI/refind/vars"));
        }
    } else {
        for path in [
            "/Volumes/REFIND",
            "/boot/efi",
            "/efi",
            "/boot",
            "/mnt/REFIND",
            "/media/REFIND",
        ] {
            push_unique(&mut candidates, PathBuf::from(path).join("EFI/refind/vars"));
        }
        for base in ["/Volumes", "/mnt", "/media", "/run/media"] {
            let base = PathBuf::from(base);
            let Ok(entries) = fs::read_dir(&base) else {
                continue;
            };
            for entry in entries.flatten() {
                let root = entry.path();
                push_unique(&mut candidates, root.join("EFI/refind/vars"));
                let Ok(nested) = fs::read_dir(&root) else {
                    continue;
                };
                for nested_entry in nested.flatten() {
                    push_unique(&mut candidates, nested_entry.path().join("EFI/refind/vars"));
                }
            }
        }
    }
    candidates
}

#[cfg(target_os = "linux")]
pub(crate) fn decode_mountinfo_path(path: &str) -> PathBuf {
    let mut decoded = String::with_capacity(path.len());
    let mut characters = path.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            let mut escape = String::new();
            for _ in 0..3 {
                if let Some(digit) = characters.next() {
                    escape.push(digit);
                }
            }
            if let Ok(byte) = u8::from_str_radix(&escape, 8) {
                decoded.push(byte as char);
                continue;
            }
            decoded.push(character);
            decoded.push_str(&escape);
        } else {
            decoded.push(character);
        }
    }
    PathBuf::from(decoded)
}
