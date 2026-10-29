//! lsblk / 变量目录扫描。依赖 state.rs 的 VariableInfo / ScanResult 共享结构。

use super::*;

pub(crate) fn system_display_name(system: &str) -> String {
    SOURCE_FILES
        .iter()
        .find(|(candidate, _, _)| *candidate == system)
        .map(|(_, display_name, _)| (*display_name).to_string())
        .unwrap_or_else(|| system.to_string())
}

#[derive(Deserialize)]
pub(crate) struct LsblkOutput {
    pub(crate) blockdevices: Vec<LsblkDevice>,
}

#[derive(Deserialize)]
pub(crate) struct LsblkDevice {
    pub(crate) path: Option<String>,
    pub(crate) fstype: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) partlabel: Option<String>,
    pub(crate) parttype: Option<String>,
    pub(crate) mountpoints: Option<Vec<Option<String>>>,
    /// lsblk 的 SIZE 列（`2.9G`）—— 只有「枚举未挂载 ESP」那条查询会要它，
    /// 其它查询缺这列也得能反序列化，所以是 Option。
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) size: Option<String>,
    /// 文件系统 UUID —— GRUB `search --fs-uuid` 比的那串（**不是**识别键）。
    /// 老代码的 lsblk 调用没要这一列，`Option` 让缺失列也能反序列化。
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) uuid: Option<String>,
    /// 分区 UUID（GPT PARTUUID）—— 识别 ESP 的权威键，三端同值。
    /// 只有 Linux 的定位函数读它（lsblk 输出列），其余平台缺列也能反序列化。
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) partuuid: Option<String>,
}

pub(crate) struct ScanResult {
    pub(crate) vars_dir: PathBuf,
    pub(crate) active_mount: Option<ActiveMount>,
    pub(crate) variables: Vec<VariableInfo>,
}

#[derive(Clone)]
pub(crate) struct MountedScanCandidate {
    pub(crate) vars_dir: PathBuf,
    pub(crate) active_mount: Option<ActiveMount>,
}

pub(crate) fn file_hash(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) fn hashes_match(left: &Path, right: &Path) -> bool {
    let (Ok(left), Ok(right)) = (file_hash(left), file_hash(right)) else {
        return false;
    };
    left == right
}

pub(crate) fn display_path(path: &Path) -> PathBuf {
    let path_text = path.as_os_str().to_string_lossy();
    if let Some(unc_path) = path_text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc_path}"))
    } else if let Some(local_path) = path_text.strip_prefix(r"\\?\") {
        PathBuf::from(local_path)
    } else {
        path.to_path_buf()
    }
}

pub(crate) fn inspect_variables(directory: &Path) -> io::Result<Vec<VariableInfo>> {
    let target = directory.join("PreviousBoot");
    let mut variables = Vec::new();
    for (system, display_name, source_name) in SOURCE_FILES {
        let source = directory.join(source_name);
        let exists = source.is_file();
        variables.push(VariableInfo {
            system: system.to_string(),
            display_name: display_name.to_string(),
            source_name: source_name.to_string(),
            source: source.clone(),
            exists,
            active: exists && hashes_match(&source, &target),
        });
    }
    Ok(variables)
}

pub(crate) fn push_unique(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if let Ok(resolved) = candidate.canonicalize() {
        if resolved.is_dir() && !candidates.contains(&resolved) {
            candidates.push(resolved);
        }
    }
}

pub(crate) fn scan_unmounted_partition() -> Result<ScanResult, String> {
    if !cfg!(target_os = "linux") {
        return Err("未找到已挂载的 EFI/refind/vars 目录".to_string());
    }

    let _span = crate::perf::span("cmd.lsblk_scan");
    let output = command("lsblk")
        .args([
            "--json",
            "--output",
            "PATH,TYPE,FSTYPE,LABEL,PARTLABEL,PARTTYPE,MOUNTPOINTS",
        ])
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法执行 lsblk: {error}"))?;
    let devices: LsblkOutput = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("无法解析 lsblk 输出: {error}"))?;

    let mut candidates = Vec::new();
    for device in devices.blockdevices {
        let Some(device_path) = device.path else {
            continue;
        };
        let Some(filesystem) = device.fstype else {
            continue;
        };
        if !FAT_FILESYSTEMS.contains(&filesystem.as_str()) {
            continue;
        }
        let mounted = device
            .mountpoints
            .unwrap_or_default()
            .iter()
            .flatten()
            .any(|mountpoint| !mountpoint.is_empty());
        if mounted {
            continue;
        }
        let label = device.label.unwrap_or_default().to_lowercase();
        let partlabel = device.partlabel.unwrap_or_default().to_lowercase();
        let parttype = device.parttype.unwrap_or_default().to_lowercase();
        let hints = format!("{label} {partlabel}");
        let priority = if hints.contains("refind") {
            0
        } else if parttype == ESP_PARTTYPE || hints.contains("efi system partition") {
            1
        } else {
            2
        };
        candidates.push((priority, device_path, filesystem));
    }
    candidates.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));

    let mountpoint = std::env::temp_dir().join(format!("anyboot-{}", std::process::id()));
    let _ = fs::remove_dir(&mountpoint);
    fs::create_dir(&mountpoint).map_err(|error| format!("无法创建临时挂载点: {error}"))?;

    for (_, device, filesystem) in candidates {
        let result = mount_command("mount")
            .args(["-o", &mount_options("ro"), "-t", &filesystem])
            .arg(&device)
            .arg(&mountpoint)
            .output();
        match result {
            Ok(output) if output.status.success() => {
                let vars_dir = mountpoint.join("EFI/refind/vars");
                if vars_dir.is_dir() {
                    let resolved = vars_dir
                        .canonicalize()
                        .map_err(|error| format!("无法解析变量目录: {error}"))?;
                    let variables = inspect_variables(&resolved)
                        .map_err(|error| format!("无法读取变量目录: {error}"))?;
                    return Ok(ScanResult {
                        vars_dir: resolved,
                        active_mount: Some(ActiveMount {
                            device: PathBuf::from(device),
                            mountpoint,
                            owned: true,
                        }),
                        variables,
                    });
                }
                let _ = mount_command("umount").arg(&mountpoint).status();
            }
            Ok(output) => {
                let _ = format!(
                    "mount {device}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Err(_) => {}
        }
    }
    let _ = fs::remove_dir(&mountpoint);

    let root_hint = if cfg!(target_os = "linux") {
        "；应用已尝试通过 pkexec 临时提权挂载，若取消授权或 pkexec 不可用，请手动挂载后再扫描"
    } else {
        ""
    };
    Err(format!(
        "已扫描未挂载的 FAT/ESP 分区，但未找到 EFI/refind/vars 目录{root_hint}"
    ))
}

pub(crate) fn scan_vars_directory(explicit_dir: Option<&Path>) -> Result<ScanResult, String> {
    if let Some(directory) = explicit_dir {
        if !directory.is_dir() {
            return Err(format!("rEFInd 变量目录不存在: {}", directory.display()));
        }
        let resolved_directory = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.to_path_buf());
        let variables = inspect_variables(&resolved_directory)
            .map_err(|error| format!("无法读取变量目录: {error}"))?;
        return Ok(ScanResult {
            vars_dir: resolved_directory.clone(),
            active_mount: active_mount_for_directory(directory, &resolved_directory),
            variables,
        });
    }

    let mut candidates = mounted_scan_candidates();
    candidates.sort_by_key(|candidate| {
        candidate
            .active_mount
            .as_ref()
            .map(|mount| !mount.owned)
            .unwrap_or(true)
    });
    if candidates.len() > 1 {
        let names = candidates
            .iter()
            .map(|candidate| format!("  {}", candidate.vars_dir.display()))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "找到多个 rEFInd 变量目录，请在主窗口指定其中一个：\n{names}"
        ));
    }
    if let Some(directory) = candidates.first() {
        let variables = inspect_variables(&directory.vars_dir)
            .map_err(|error| format!("无法读取变量目录: {error}"))?;
        return Ok(ScanResult {
            vars_dir: directory.vars_dir.clone(),
            active_mount: directory.active_mount.clone(),
            variables,
        });
    }
    scan_unmounted_partition()
}

pub(crate) const ESP_PARTTYPE: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

/// 一块「看着是 ESP、但当前没挂载」的分区（Linux）。
///
/// 这是通道检测的**盲区补丁**：`/proc/self/mounts` 只看得到已经挂上的卷，
/// 而 Linux 上只有 `/etc/fstab` 里那一块系统 ESP 会被自动挂载。双系统黑苹果的
/// OpenCore 通常待在**另一块盘的 ESP** 里（本机 `nvme0n1p5`，GPT 名 `MacOS-efi`），
/// 那块盘从开机到用户手动挂它之前，对所有候选集都不存在 —— 卡片只能说
/// 「未检测到」，而用户手里明明有。
#[cfg(target_os = "linux")]
#[derive(Clone, Debug)]
pub(crate) struct UnmountedEsp {
    /// 设备节点，`/dev/nvme0n1p5`
    pub(crate) device: String,
    /// 给人看的大小，`2.9 GiB`
    pub(crate) size: String,
    /// 给人看的名字：GPT 分区名 → 卷标 → 文件系统类型，取第一个非空的
    pub(crate) label: String,
    /// 已知的文件系统类型（挂载时 `-t`）。udev 缺失时是 None —— 那就让
    /// `mount` 自己探，别猜一个 `vfat` 上去把能挂的分区挂失败。
    pub(crate) fstype: Option<String>,
}

#[cfg(target_os = "linux")]
impl UnmountedEsp {
    pub(crate) fn describe(&self) -> String {
        format!(
            "{}（{}，{}）",
            self.device.trim_start_matches("/dev/"),
            self.label,
            self.size
        )
    }
}

/// 枚举「没挂载的 EFI 系统分区」——**只读元数据，不挂载、不提权**。
///
/// 两条来源合并，缺一条就会在真机上枚举不到东西：
/// * `lsblk --json`：带 FSTYPE / PARTTYPE，最准，但这两列来自 udev 数据库；
/// * sysfs 的 `PARTNAME`：GPT 分区名由内核直接给出，**不依赖 udev**。
///   这台机器上 `lsblk -f` 的 FSTYPE 整列是空的（`/run/udev` 都不存在），
///   只靠 lsblk 一块 ESP 都找不出来 —— 而 `nvme0n1p5` 的 `PARTNAME` 正是
///   `MacOS-efi`。
///
/// 只挑「看起来就是 ESP」的：GPT 类型是 ESP，或者分区名/卷标里带 efi/esp。
/// 不按 `vfat` 一刀切 —— 那样每插一个 U 盘都会多出一行「要不要挂载」的噪声。
#[cfg(target_os = "linux")]
pub(crate) fn unmounted_esp_partitions() -> Vec<UnmountedEsp> {
    let _span = crate::perf::span("cmd.lsblk_esp");
    // 已经挂着的设备节点：sysfs 那条路只能靠它排除，lsblk 自带 MOUNTPOINTS
    let mounted: Vec<String> = std::fs::read_to_string("/proc/self/mounts")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split(' ').next().map(str::to_string))
        .collect();

    let mut found: Vec<(u8, UnmountedEsp)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    let output = command("lsblk")
        .args([
            "--json",
            "--output",
            "PATH,TYPE,FSTYPE,LABEL,PARTLABEL,PARTTYPE,MOUNTPOINTS,SIZE",
        ])
        .stderr(Stdio::null())
        .output();
    if let Ok(output) = output {
        if let Ok(devices) = serde_json::from_slice::<LsblkOutput>(&output.stdout) {
            for device in devices.blockdevices {
                let Some(path) = device.path else { continue };
                let partlabel = device.partlabel.unwrap_or_default();
                let label_raw = device.label.unwrap_or_default();
                let hints = format!("{partlabel} {label_raw}").to_lowercase();
                let is_esp_type = device
                    .parttype
                    .as_deref()
                    .is_some_and(|parttype| parttype.eq_ignore_ascii_case(ESP_PARTTYPE));
                let named = hints.contains("efi") || hints.contains("esp");
                if !is_esp_type && !named {
                    continue;
                }
                if device
                    .mountpoints
                    .unwrap_or_default()
                    .iter()
                    .flatten()
                    .any(|mountpoint| !mountpoint.is_empty())
                {
                    continue;
                }
                // 没 udev 时 FSTYPE 是空的：这时候别拿「不是 FAT」把 ESP 否掉
                let fstype = device.fstype.filter(|value| !value.trim().is_empty());
                let label = if !partlabel.trim().is_empty() {
                    partlabel
                } else if !label_raw.trim().is_empty() {
                    label_raw
                } else {
                    fstype.clone().unwrap_or_else(|| "未知文件系统".to_string())
                };
                seen.push(path.clone());
                found.push((
                    if is_esp_type { 0 } else { 1 },
                    UnmountedEsp {
                        device: path,
                        size: device.size.unwrap_or_default(),
                        label,
                        fstype,
                    },
                ));
            }
        }
    }

    for (device, name, bytes) in sysfs_partitions() {
        if seen.iter().any(|known| known == &device) || mounted.iter().any(|used| used == &device) {
            continue;
        }
        let hints = name.to_lowercase();
        if !(hints.contains("efi") || hints.contains("esp")) {
            continue;
        }
        found.push((
            1,
            UnmountedEsp {
                device,
                size: human_mib(bytes),
                label: name,
                fstype: None,
            },
        ));
    }

    found.sort_by(|left, right| (left.0, &left.1.device).cmp(&(right.0, &right.1.device)));
    found.into_iter().map(|(_, part)| part).collect()
}

/// sysfs 里的 `(设备节点, GPT 分区名, 字节数)`，只列分区（不列整盘）。
///
/// 读 `<name>/uevent` 的 `PARTNAME` —— 内核直接给，不经过 udev。
#[cfg(target_os = "linux")]
fn sysfs_partitions() -> Vec<(String, String, u64)> {
    let Ok(entries) = std::fs::read_dir("/sys/class/block") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let dir = entry.path();
        // 只有分区才有 `partition` 文件；整盘没有
        if !dir.join("partition").is_file() {
            continue;
        }
        let Ok(uevent) = std::fs::read_to_string(dir.join("uevent")) else {
            continue;
        };
        let partname = uevent
            .lines()
            .find_map(|line| line.strip_prefix("PARTNAME="))
            .unwrap_or_default()
            .trim()
            .to_string();
        if partname.is_empty() {
            continue;
        }
        let bytes = std::fs::read_to_string(dir.join("size"))
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .map(|sectors| sectors.saturating_mul(512))
            .unwrap_or(0);
        out.push((format!("/dev/{name}"), partname, bytes));
    }
    out
}

/// `2.9 GiB` / `900 MiB`：分区大小给人看，别丢一堆字节数。
#[cfg(target_os = "linux")]
fn human_mib(bytes: u64) -> String {
    let mib = bytes as f64 / (1024.0 * 1024.0);
    if mib >= 1024.0 {
        format!("{:.1} GiB", mib / 1024.0)
    } else {
        format!("{mib:.0} MiB")
    }
}
