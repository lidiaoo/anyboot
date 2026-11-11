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

#[derive(Debug)]
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

/// 一个 FAT 分区（Linux「未挂载 ESP 扫描」用的候选）。
#[cfg(target_os = "linux")]
#[derive(Clone, Debug)]
pub(crate) struct FatPartition {
    pub(crate) device: String,
    pub(crate) fstype: String,
    /// 已经挂着的挂载点（None = 没挂载，扫描时临时挂）。
    pub(crate) mountpoint: Option<PathBuf>,
    pub(crate) label: String,
    pub(crate) partlabel: String,
    pub(crate) parttype: String,
}

/// 从 `lsblk --json` 的结果里挑出「该纳入考虑」的 FAT 分区。
///
/// 判定（纯函数，好单测；真机 lsblk 输出灌进来也能跑）：
/// * 只留 FAT 家族（vfat/fat/fat32/…/exfat）—— rEFInd 只能待在 FAT 上；
/// * **已挂载、以及系统自挂的 ESP 一律纳入**（用户定案：已挂载的「只扫描不卸载」）；
/// * 未挂载的：只有「看着就是 ESP」的才纳入 —— GPT 类型是 ESP，或分区名/卷标里带
///   efi/esp，或卷标带 refind。不按 vfat 一刀切，免得每插个 U 盘就多挂一次。
///
/// 排序：refind 命中 → ESP 类型 → 其余；同级按设备名，结果稳定可复现。
#[cfg(target_os = "linux")]
pub(crate) fn select_fat_candidates(devices: Vec<LsblkDevice>) -> Vec<FatPartition> {
    let mut out: Vec<(u8, FatPartition)> = Vec::new();
    for device in devices {
        let Some(path) = device.path else { continue };
        let Some(fstype) = device.fstype else { continue };
        if !FAT_FILESYSTEMS.contains(&fstype.to_lowercase().as_str()) {
            continue;
        }
        let mountpoint = device
            .mountpoints
            .unwrap_or_default()
            .iter()
            .flatten()
            .find(|mountpoint| !mountpoint.is_empty())
            .map(PathBuf::from);
        let label_raw = device.label.unwrap_or_default();
        let partlabel = device.partlabel.unwrap_or_default();
        let parttype = device.parttype.unwrap_or_default();
        let hints = format!("{label_raw} {partlabel}").to_lowercase();
        let is_esp_type = parttype.eq_ignore_ascii_case(ESP_PARTTYPE);
        let priority = if hints.contains("refind") {
            0
        } else if is_esp_type || hints.contains("efi") || hints.contains("esp") {
            1
        } else {
            2
        };
        // 没挂载的、又不是 ESP 长相的 FAT 分区（U 盘、数据盘）不纳入，免噪声
        if mountpoint.is_none() && priority == 2 {
            continue;
        }
        out.push((
            priority,
            FatPartition {
                device: path,
                fstype,
                mountpoint,
                label: label_raw,
                partlabel,
                parttype,
            },
        ));
    }
    out.sort_by(|left, right| (left.0, &left.1.device).cmp(&(right.0, &right.1.device)));
    out.into_iter().map(|(_, part)| part).collect()
}

/// rEFInd 家族「文件式变量目录」的候选位置（相对卷根，按优先级排）。
///
/// rEFInd 的 `use_nvram false` 时落在 **SelfDir（二进制所在目录）** 下：先试
/// `vars`，建不出来才回落 ESP 根的 `refind-vars`（upstream `lib.c:403-412`）；
/// RefindPlus 顺序相反：先读 `<SelfDir>/refind-vars`，没有才建
/// `<SelfDir>/vars`（RefindPlus `lib.c:739-790` `FindVarsDir`）。
/// SelfDir 惯例有三处：`EFI/refind`（官方默认）、`EFI/refindplus`（RefindPlus
/// 默认）、`EFI/BOOT`（fallback / `refind-install --usedefault` / RefindPlus
/// 手动布局）。两个目录名 × 三个安装位，全试，命中即用。
pub(crate) const VARS_CANDIDATE_RELS: &[&str] = &[
    "EFI/refind/vars",
    "EFI/refind/refind-vars",
    "EFI/refindplus/vars",
    "EFI/refindplus/refind-vars",
    "EFI/BOOT/vars",
    "EFI/BOOT/refind-vars",
];

/// 在目录里找 rEFInd 家族的文件式变量目录（见 [`VARS_CANDIDATE_RELS`]），
/// 命中就读出变量快照。
#[cfg(target_os = "linux")]
fn inspect_vars_under(root: &Path) -> Result<Option<(PathBuf, Vec<VariableInfo>)>, String> {
    for candidate in VARS_CANDIDATE_RELS {
        let vars_dir = root.join(candidate);
        if !vars_dir.is_dir() {
            continue;
        }
        let resolved = vars_dir
            .canonicalize()
            .map_err(|error| format!("无法解析变量目录: {error}"))?;
        let variables =
            inspect_variables(&resolved).map_err(|error| format!("无法读取变量目录: {error}"))?;
        return Ok(Some((resolved, variables)));
    }
    Ok(None)
}

#[cfg(target_os = "linux")]
pub(crate) fn scan_unmounted_partition() -> Result<ScanResult, String> {
    let _span = crate::perf::span("cmd.lsblk_scan");
    // 要 MOUNTPOINTS：已挂载的 ESP 直接在其挂载点上找 vars（不卸载、不重挂）
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

    // 纯函数挑选：已挂载的 FAT + 未挂载的 ESP 长相分区，全都纳入
    let candidates = select_fat_candidates(devices.blockdevices);

    // ---- 第一轮：已挂载的直接扫（只读，绝不卸载）----
    // 用户的定案：已被挂载的「只扫描不卸载」—— 挂载点是人家的，动它就是越界。
    for part in &candidates {
        let Some(mountpoint) = part.mountpoint.as_ref() else {
            continue;
        };
        match inspect_vars_under(mountpoint) {
            Ok(Some((vars_dir, variables))) => {
                // owned=false：这不是我们挂的，退出/切通道时也绝不能卸
                return Ok(ScanResult {
                    vars_dir,
                    active_mount: Some(ActiveMount {
                        device: PathBuf::from(&part.device),
                        mountpoint: mountpoint.clone(),
                        owned: false,
                    }),
                    variables,
                });
            }
            Ok(None) => {}
            Err(_) => {
                // 挂载点没权限读（少见）：当作没找到，继续试别的
            }
        }
    }

    // ---- 第二轮：未挂载的逐个临时挂载扫描（用完立刻卸）----
    // 只挂「未挂载」的；每次挂到独立临时点，扫完不管成功失败都卸掉，绝不残留。
    let mut last_mount_error: Option<String> = None;
    for part in &candidates {
        if part.mountpoint.is_some() {
            continue; // 已挂载的在第一轮处理过
        }
        let mountpoint = std::env::temp_dir().join(format!(
            "anyboot-{}-{}",
            std::process::id(),
            part.device
                .trim_start_matches("/dev/")
                .replace(['/', ' '], "_")
        ));
        let _ = fs::remove_dir(&mountpoint);
        if let Err(error) = fs::create_dir(&mountpoint) {
            last_mount_error = Some(format!("无法创建临时挂载点: {error}"));
            continue;
        }
        let result = {
            let mut cmd = mount_command("mount");
            cmd.args(["-o", &mount_options("ro"), "-t", &part.fstype]);
            cmd.arg(&part.device).arg(&mountpoint);
            cmd.output()
        };
        match result {
            Ok(output) if output.status.success() => {
                let inspected = inspect_vars_under(&mountpoint);
                if let Ok(Some((vars_dir, variables))) = inspected {
                    // 命中：保留这份临时挂载（owned=true，交给 ActiveMount 生命周期卸载）
                    return Ok(ScanResult {
                        vars_dir,
                        active_mount: Some(ActiveMount {
                            device: PathBuf::from(&part.device),
                            mountpoint,
                            owned: true,
                        }),
                        variables,
                    });
                }
                // 没命中或读失败：立刻卸掉这个临时挂载，继续下一块
                let _ = mount_command("umount").arg(&mountpoint).status();
                let _ = fs::remove_dir(&mountpoint);
                if let Err(error) = inspected {
                    last_mount_error = Some(error);
                }
            }
            Ok(output) => {
                last_mount_error = Some(format!(
                    "mount {}: {}",
                    part.device,
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
                let _ = fs::remove_dir(&mountpoint);
            }
            Err(error) => {
                last_mount_error = Some(format!("无法执行 mount: {error}"));
                let _ = fs::remove_dir(&mountpoint);
            }
        }
    }

    let _ = last_mount_error; // 只做诊断用，能挂的都试过了，统一给下面这句
    let root_hint = if cfg!(target_os = "linux") {
        "；应用已尝试通过 pkexec 临时提权挂载所有未挂载的 FAT/ESP 分区（已挂载的只读扫描不卸载），若取消授权或 pkexec 不可用，请手动挂载后再扫描"
    } else {
        ""
    };
    Err(format!(
        "已扫描全部已挂载与临时挂载的 FAT/ESP 分区，但未找到 rEFInd 家族的变量目录（EFI/refind、EFI/refindplus、EFI/BOOT 下的 vars / refind-vars）{root_hint}"
    ))
}

/// 定位 RefindPlus 安装（refindplus 通道「自动模式」的数据源）。
///
/// 与 [`scan_vars_directory`] 的差别在**认谁**：rEFInd 通道认第一个带 vars 的
/// 候选；这里只认「安装目录下有 `config.conf` / `refindplus.conf`」的候选 ——
/// 这是 RefindPlus 的配置文件搜索顺序（RefindPlus `main.c:4422-4445`），也是
/// 它与同机共存 rEFInd（`refind.conf`）的唯一可靠区分。两台安装并存时各回
/// 各的通道，互不抢。
///
/// 只扫已挂载卷（mountinfo / 盘符枚举）；未挂载的 ESP 与 rEFInd 通道一样，
/// 请先在主窗口点「只读挂载」再扫。
pub(crate) fn scan_refindplus_install() -> Result<ScanResult, String> {
    let _sp_c = crate::perf::span("vars.candidates.rp");
    let mut tier_refindplus: Vec<MountedScanCandidate> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for candidate in mounted_scan_candidates() {
        if seen.contains(&candidate.vars_dir) {
            continue;
        }
        seen.push(candidate.vars_dir.clone());
        let Some(install_dir) = candidate.vars_dir.parent() else {
            continue;
        };
        let (_, flavor) = crate::channels::refind_menu::resolve_conf(install_dir);
        if flavor != "refindplus" {
            continue;
        }
        tier_refindplus.push(candidate);
    }
    // 与 rEFInd 通道对称：同层（都是 RefindPlus 安装）多个才报错列出，不猜
    if tier_refindplus.len() > 1 {
        let names = tier_refindplus
            .iter()
            .map(|candidate| format!("  {}", candidate.vars_dir.display()))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "找到多个 RefindPlus 安装（都有 config.conf），请指定其中一个：\n{names}"
        ));
    }
    if let Some(candidate) = tier_refindplus.first() {
        let variables = inspect_variables(&candidate.vars_dir)
            .map_err(|error| format!("无法读取变量目录: {error}"))?;
        return Ok(ScanResult {
            vars_dir: candidate.vars_dir.clone(),
            active_mount: candidate.active_mount.clone(),
            variables,
        });
    }
    Err(
        "已扫描全部已挂载的 FAT/ESP 分区，但没找到 RefindPlus 安装：其安装目录下应有 \
         config.conf（或 refindplus.conf）并带 vars 目录；未挂载的分区请先在主窗口点「只读挂载」"
            .to_string(),
    )
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

    let _sp_c = crate::perf::span("vars.candidates");
    let mut candidates = mounted_scan_candidates();
    drop(_sp_c);
    candidates.sort_by_key(|candidate| {
        candidate
            .active_mount
            .as_ref()
            .map(|mount| !mount.owned)
            .unwrap_or(true)
    });

    // rEFInd 与 RefindPlus 是**并存的两台引导器**，不是二选一：refindplus 通道
    // 凭 config.conf（[`scan_refindplus_install`]）认领自己的安装；这里 rEFInd
    // 通道在**剩下的**候选里做 flavor 分层选择 ——
    //   1. 有 refind.conf 的（flavor=refind）优先；
    //   2. conf 一个都没有的（unknown，比如只有 BOOTx64+vars 的裸布局）次之；
    //   3. flavor=refindplus 的整层让给 refindplus 通道，绝不抢。
    // 同层出现多个才报「找到多个」（且只列同层候选 —— 被让出去的不算数）。
    let mut tier_refind = Vec::new();
    let mut tier_unknown = Vec::new();
    for candidate in candidates {
        let flavor = candidate
            .vars_dir
            .parent()
            .map(crate::channels::refind_menu::resolve_conf)
            .map(|(_, flavor)| flavor)
            .unwrap_or("unknown");
        match flavor {
            "refind" => tier_refind.push(candidate),
            "refindplus" => {} // refindplus 通道的，让出去
            _ => tier_unknown.push(candidate),
        }
    }
    for tier in [&tier_refind, &tier_unknown] {
        if tier.is_empty() {
            continue;
        }
        if tier.len() > 1 {
            let names = tier
                .iter()
                .map(|candidate| format!("  {}", candidate.vars_dir.display()))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(format!(
                "找到多个 rEFInd 变量目录，请在主窗口指定其中一个：\n{names}"
            ));
        }
        let directory = &tier[0];
        let _sp_i = crate::perf::span("vars.inspect");
        let variables = inspect_variables(&directory.vars_dir)
            .map_err(|error| format!("无法读取变量目录: {error}"))?;
        drop(_sp_i);
        return Ok(ScanResult {
            vars_dir: directory.vars_dir.clone(),
            active_mount: directory.active_mount.clone(),
            variables,
        });
    }
    #[cfg(target_os = "linux")]
    {
        return scan_unmounted_partition();
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("未找到已挂载的 rEFInd 家族变量目录（EFI/refind、EFI/refindplus、EFI/BOOT 下的 vars / refind-vars）".to_string())
    }
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn dev(
        path: &str,
        fstype: Option<&str>,
        label: Option<&str>,
        partlabel: Option<&str>,
        parttype: Option<&str>,
        mountpoints: Vec<Option<String>>,
    ) -> LsblkDevice {
        LsblkDevice {
            path: Some(path.to_string()),
            fstype: fstype.map(str::to_string),
            label: label.map(str::to_string),
            partlabel: partlabel.map(str::to_string),
            parttype: parttype.map(str::to_string),
            mountpoints: Some(mountpoints),
            size: None,
            uuid: None,
            partuuid: None,
        }
    }

    #[test]
    fn select_skips_non_fat() {
        let out = select_fat_candidates(vec![
            dev("/dev/sda1", Some("ext4"), None, None, None, vec![None]),
            dev("/dev/sda2", Some("ntfs"), None, None, None, vec![None]),
        ]);
        assert!(out.is_empty(), "非 FAT 分区一律不纳入");
    }

    #[test]
    fn select_skips_plain_unmounted_fat_usb() {
        // 未挂载、又不是 ESP 长相的 FAT（U 盘/数据盘）不该纳入
        let out = select_fat_candidates(vec![dev(
            "/dev/sdb1",
            Some("vfat"),
            Some("MYUSB"),
            None,
            Some("ebd0a0a2-b9e5-4433-87c0-68b6b72699c7"),
            vec![None],
        )]);
        assert!(out.is_empty(), "普通未挂载 FAT 不纳入（避免噪声）");
    }

    #[test]
    fn select_keeps_unmounted_esp_by_type() {
        let out = select_fat_candidates(vec![dev(
            "/dev/nvme0n1p5",
            Some("vfat"),
            Some("MacOS-efi"),
            Some("MacOS-efi"),
            Some(ESP_PARTTYPE),
            vec![None],
        )]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].device, "/dev/nvme0n1p5");
        assert!(out[0].mountpoint.is_none());
    }

    #[test]
    fn select_keeps_unmounted_by_efi_name() {
        // 类型列不靠谱（无 udev）时，靠分区名带 efi 也认
        let out = select_fat_candidates(vec![dev(
            "/dev/nvme0n1p3",
            Some("vfat"),
            Some("WINDOWS-EFI"),
            Some("windows-efi"),
            None,
            vec![None],
        )]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn select_keeps_mounted_fat_even_if_not_esp() {
        // 已挂载的 FAT 一律纳入（只扫描不卸载）——即便名字里没有 efi
        let out = select_fat_candidates(vec![dev(
            "/dev/sdc1",
            Some("vfat"),
            Some("DATA"),
            None,
            None,
            vec![Some("/mnt/data".to_string())],
        )]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].mountpoint.as_deref(), Some(Path::new("/mnt/data")));
    }

    #[test]
    fn select_orders_refind_and_esp_first_then_mounted() {
        let out = select_fat_candidates(vec![
            dev("/dev/z1", Some("vfat"), Some("DATA"), None, None, vec![Some("/m/z".to_string())]),
            dev("/dev/a2", Some("vfat"), Some("REFIND"), None, None, vec![None]),
            dev("/dev/a1", Some("vfat"), Some("EFI"), None, Some(ESP_PARTTYPE), vec![None]),
        ]);
        let devices: Vec<&str> = out.iter().map(|p| p.device.as_str()).collect();
        assert_eq!(devices, vec!["/dev/a2", "/dev/a1", "/dev/z1"],
            "refind 命中优先，其次 ESP，最后普通已挂载");
    }

    #[test]
    fn select_dedups_mountpoint_takes_first_non_empty() {
        // MOUNTPOINTS 可能是 [null, "/boot/efi"]，取第一个非空
        let out = select_fat_candidates(vec![dev(
            "/dev/nvme0n1p14",
            Some("vfat"),
            Some("ARCH-EFI"),
            Some("arch-efi"),
            Some(ESP_PARTTYPE),
            vec![None, Some("/boot/efi".to_string())],
        )]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].mountpoint.as_deref(), Some(Path::new("/boot/efi")));
    }
}
