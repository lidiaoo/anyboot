//! ESP 分区识别（esp_probe 三平台副本成组）。产出 EspInfo / ActiveMount。

use super::*;

/// 一块分区的识别结果（取不到的字段就是 None，绝不编）。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EspInfo {
    /// 用户给的路径（原样回显）
    pub(crate) path: String,
    /// 规范化后的分区根目录（挂载点 / 盘符根）
    pub(crate) root: Option<String>,
    /// 设备节点（Linux `/dev/sda1`、macOS `/dev/disk0s1`、Windows `\\.\H:`）
    pub(crate) device: Option<String>,
    /// 文件系统类型原文（FAT32 / vfat / msdos / NTFS / APFS …）
    pub(crate) fstype: Option<String>,
    /// 是不是 FAT 家族（FAT12/16/32）—— ESP 规范要求用它
    pub(crate) fat: bool,
    /// 是不是正好 FAT32
    pub(crate) fat32: bool,
    /// fs-uuid（GRUB `search --fs-uuid` 那一串，形如 `E537-7F31`）。
    /// 注意这是**文件系统**的标识（FAT 卷序列号），不是识别键 —— macOS
    /// 用户态取不到它（diskutil 报的是 MD5 派生的 Volume UUID），所以 None 很正常。
    pub(crate) uuid: Option<String>,
    /// **分区 UUID**（GPT PARTUUID，三端同值）—— 识别 ESP 的权威键。
    /// Linux 取自 lsblk 的 PARTUUID 列，macOS 取自 diskutil 的
    /// `Disk / Partition UUID`，Windows 取自 Get-Partition 的 Guid。
    pub(crate) partuuid: Option<String>,
    /// 当前是否已挂载 / 可访问
    pub(crate) mounted: bool,
    /// 这个挂载点是不是本程序挂上的（只有自己挂上的才允许自己卸）
    pub(crate) owned_mount: bool,
    /// fs-uuid 的来路，或者取不到时的原话（展示给人看）
    pub(crate) note: Option<String>,
    /// 这块分区**当前挂载在哪里**（用户可能自己先挂上了，或系统自动挂的）。
    ///
    /// Windows 上没盘符的分区 user mode 摸不到 → 这里就是 None，
    /// 界面照实说「没有盘符」，而不是装作认得。
    pub(crate) mount_point: Option<String>,
    /// 这个 mount_point 是不是系统/用户已有的挂载（不是本程序挂的）。
    /// 是的话「打开文件管理器」直接跳过去，也不需要再挂一次。
    pub(crate) preexisting: bool,
}

/// Linux：ESP 挂到哪儿。放 `$XDG_RUNTIME_DIR`（没有就 /tmp）下的**固定名字**，
/// 这样「打开文件管理器」看到的路径每次一样，用户还能把它加进书签。
#[cfg(target_os = "linux")]
pub(crate) fn esp_mountpoint() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(std::env::temp_dir)
        .join("anyboot-esp")
}

/// 本端能不能「按 fs-uuid 自己挂上来」。
///
/// Windows 不行：盘符是系统分配的，没盘符的分区用户态根本摸不到 ——
/// 所以界面把勾选框置灰，并提示去「磁盘管理」里分配盘符。
#[cfg(target_os = "windows")]
pub(crate) fn esp_mount_supported() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn esp_mount_supported() -> bool {
    true
}

/// 从 FAT 引导扇区里读出 `(FAT 类型, 卷序列号)` —— 纯函数，好单测。
///
/// 类型按簇数判定（<4085 = FAT12、<65525 = FAT16、否则 FAT32），不认那 8 字节的
/// 文件系统标识串（FAT16 卷上它常常写的是 `MSDOS5.0`）。序列号就是 GRUB 的
/// fs-uuid：FAT12/16 在偏移 39，FAT32 在偏移 67。
///
/// 只有 Linux 会用它去补 blkid 分不出来的 FAT16/FAT32（要走单测，所以不按平台裁掉）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn fat_boot_sector(sector: &[u8]) -> Option<(String, String)> {
    if sector.len() < 512 {
        return None;
    }
    let bytes_per_sector = u16::from_le_bytes([sector[11], sector[12]]) as u64;
    if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096) {
        return None;
    }
    let sectors_per_cluster = sector[13] as u64;
    let reserved = u16::from_le_bytes([sector[14], sector[15]]) as u64;
    let fat_count = sector[16] as u64;
    let root_entries = u16::from_le_bytes([sector[17], sector[18]]) as u64;
    let total16 = u16::from_le_bytes([sector[19], sector[20]]) as u64;
    let fat_size16 = u16::from_le_bytes([sector[22], sector[23]]) as u64;
    let total32 = u32::from_le_bytes([sector[32], sector[33], sector[34], sector[35]]) as u64;
    let fat_size32 = u32::from_le_bytes([sector[36], sector[37], sector[38], sector[39]]) as u64;
    let total = if total16 != 0 { total16 } else { total32 };
    let fat_size = if fat_size16 != 0 {
        fat_size16
    } else {
        fat_size32
    };
    if total == 0 || fat_size == 0 || fat_count == 0 || sectors_per_cluster == 0 {
        return None;
    }
    let root_dir_sectors = (root_entries * 32).div_ceil(bytes_per_sector);
    let overhead = reserved
        .saturating_add(fat_count.saturating_mul(fat_size))
        .saturating_add(root_dir_sectors);
    let clusters = total.saturating_sub(overhead) / sectors_per_cluster;
    let (kind, serial_at) = if clusters < 4085 {
        ("FAT12", 39usize)
    } else if clusters < 65525 {
        ("FAT16", 39usize)
    } else {
        ("FAT32", 67usize)
    };
    if sector.len() < serial_at + 4 {
        return None;
    }
    let serial = u32::from_le_bytes([
        sector[serial_at],
        sector[serial_at + 1],
        sector[serial_at + 2],
        sector[serial_at + 3],
    ]);
    Some((kind.to_string(), format!("{serial:08X}")))
}

/// 读设备的前 512 字节（Linux 上普通用户读 `/dev/sdX1` 多半没权限 ——
/// 那是预期的失败，调用方退回到 blkid 报的信息，这里不报错）。
#[cfg(target_os = "linux")]
pub(crate) fn read_first_sector(device: &Path) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = fs::File::open(device)?;
    let mut buffer = vec![0u8; 512];
    file.read_exact(&mut buffer)?;
    Ok(buffer)
}

/// Windows：盘符路径 → 卷信息。
#[cfg(target_os = "windows")]
pub(crate) fn esp_probe(path: &Path) -> Result<EspInfo, String> {
    let text = path.to_string_lossy().to_string();
    let letter = text
        .trim()
        .chars()
        .next()
        .filter(char::is_ascii_alphabetic)
        .map(|letter| letter.to_ascii_uppercase())
        .ok_or_else(|| format!("请填盘符路径，例如 H:\\（当前填的是 {text}）"))?;
    let (filesystem, serial) = windows_volume(letter).ok_or_else(|| {
        format!("读不到 {letter}: 的卷信息：盘符不存在，或该分区还没在「磁盘管理」里分配盘符")
    })?;
    let name = filesystem.to_ascii_uppercase();
    let fat32 = name == "FAT32";
    let root = format!("{letter}:\\");
    // 识别键 = 分区 UUID（Get-Partition 的 Guid），一次 PowerShell 枚举全量再查表
    let partuuid = windows_partition_guids()
        .into_iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(&letter))
        .map(|(_, guid)| guid)
        .filter(|guid| grub::partuuid_digits(guid).is_some());
    Ok(EspInfo {
        path: text,
        root: Some(root.clone()),
        device: Some(format!("\\\\.\\{letter}:")),
        fstype: Some(filesystem),
        fat: fat32 || name == "FAT" || name == "FAT16",
        fat32,
        uuid: Some(grub::uuid_pretty(&serial)),
        partuuid,
        mounted: true,
        owned_mount: false,
        note: Some(
            "分区 UUID 取自 Get-Partition 的 Guid（识别键，三端同值）；fs-uuid 取自 \
             Windows 卷序列号 —— GRUB 的 search --fs-uuid 用它"
                .to_string(),
        ),
        // 有盘符就是「挂着的」，而且是系统给的挂载（不是我们挂的）
        mount_point: Some(root),
        preexisting: true,
    })
}

#[cfg(target_os = "linux")]
#[derive(Deserialize)]
pub(crate) struct FindmntOutput {
    #[serde(default)]
    pub(crate) filesystems: Vec<FindmntEntry>,
}

#[cfg(target_os = "linux")]
#[derive(Deserialize)]
pub(crate) struct FindmntEntry {
    #[serde(default)]
    pub(crate) source: Option<String>,
    #[serde(default)]
    pub(crate) fstype: Option<String>,
    #[serde(default)]
    pub(crate) uuid: Option<String>,
    #[serde(default)]
    pub(crate) target: Option<String>,
}

/// Linux：路径 → 它所在的那块分区。
///
/// `findmnt --target` 取的是「包含这个路径的那一次挂载」，所以用户填
/// `/boot/efi/EFI/` 还是 `/run/media/me/ARCH-EFI/EFI/grub.cfg`，都会落到同一块 FAT 上。
#[cfg(target_os = "linux")]
pub(crate) fn esp_probe(path: &Path) -> Result<EspInfo, String> {
    let root = fs::canonicalize(path)
        .map_err(|error| format!("路径打不开：{}（{error}）", path.display()))?;
    let _span = crate::perf::span("cmd.findmnt");
    let output = command("findmnt")
        .args([
            "--json",
            "--output",
            "SOURCE,FSTYPE,UUID,TARGET",
            "--target",
        ])
        .arg(&root)
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法执行 findmnt: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{} 不在任何已挂载的分区上（ESP 还没挂上来？）",
            root.display()
        ));
    }
    let parsed: FindmntOutput = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("无法解析 findmnt 输出: {error}"))?;
    let entry = parsed
        .filesystems
        .into_iter()
        .next()
        .ok_or_else(|| format!("{} 不在任何已挂载的分区上", root.display()))?;

    let mut fstype = entry.fstype.clone().unwrap_or_else(|| "未知".to_string());
    let mut uuid = entry
        .uuid
        .clone()
        .filter(|value| !value.trim().is_empty())
        .map(|value| match grub::uuid_digits(&value) {
            Some(digits) if digits.len() == 8 => grub::uuid_pretty(&digits),
            _ => value,
        });
    let mut note = "fs-uuid 取自 findmnt 报的 UUID（与 GRUB 同源）".to_string();
    // 能直接读引导扇区就自己算一遍：blkid 的 `vfat` 分不出 FAT16/FAT32，
    // 引导扇区里的簇数分得出来，序列号也读得到。读不了（普通用户没权限）就算了。
    if let Some(device) = entry.source.as_deref() {
        if let Ok(sector) = read_first_sector(Path::new(device)) {
            if let Some((kind, serial)) = fat_boot_sector(&sector) {
                fstype = kind;
                uuid = Some(grub::uuid_pretty(&serial));
                note = format!("fs-uuid 取自 {device} 引导扇区里的 BS_VolID（最权威）");
            }
        }
    }
    let name = fstype.to_ascii_uppercase();
    let fat32 = name == "FAT32";
    // Linux 内核里这文件系统就叫 vfat（fat32 只是 mount(8) 的别名），
    // blkid/findmnt/lsblk 报的都是 vfat —— 引导扇区读不到（没权限）时
    // 走到这里，别把它漏出 FAT 家族。
    let fat = fat32
        || matches!(
            name.as_str(),
            "FAT" | "FAT12" | "FAT16" | "VFAT" | "MSDOS"
        );
    if matches!(name.as_str(), "VFAT" | "MSDOS") {
        note = format!("Linux 把 FAT12/16/32 都报成 {fstype}；ESP 一般是 FAT32（{note}）");
    }
    // 识别键 = 分区 UUID：对 findmnt 报的 SOURCE 设备查 lsblk 的 PARTUUID 列
    //（udev 数据库，普通用户可读）。查不到就如实留空，note 里说去哪补。
    let partuuid = entry.source.as_deref().and_then(|device| {
        let output = command("lsblk")
            .args(["--noheadings", "--output", "PARTUUID"])
            .arg(device)
            .stderr(Stdio::null())
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    let partuuid = partuuid.filter(|value| grub::partuuid_digits(value).is_some());
    Ok(EspInfo {
        path: path.to_string_lossy().to_string(),
        root: Some(root.display().to_string()),
        device: entry.source,
        fstype: Some(fstype),
        fat,
        fat32,
        uuid,
        partuuid,
        mounted: true,
        owned_mount: false,
        note: Some(note),
        // findmnt --target 报的 TARGET 就是这块分区的**挂载点本身**
        mount_point: entry.target,
        preexisting: true,
    })
}

/// macOS：`diskutil info` 里我们要的那几行。
#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Default)]
pub(crate) struct DiskInfo {
    pub(crate) node: Option<String>,
    pub(crate) identifier: Option<String>,
    pub(crate) personality: Option<String>,
    pub(crate) uuid: Option<String>,
    pub(crate) mount_point: Option<String>,
    /// 分区 UUID（GPT PARTGUID，diskutil 报作 `Disk / Partition UUID`）。
    /// 与 Linux blkid/lsblk 的 PARTUUID、Windows Get-Partition 的 Guid 同值，
    /// 是识别 ESP 的权威键。
    pub(crate) partition_uuid: Option<String>,
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_read_line(info: &mut DiskInfo, line: &str) {
    let Some((key, value)) = line.split_once(':') else {
        return;
    };
    let value = value.trim();
    match key.trim() {
        "Device Node" => info.node = Some(value.to_string()),
        "Device Identifier" => info.identifier = Some(value.to_string()),
        "File System Personality" => info.personality = Some(value.to_string()),
        "Volume UUID" => info.uuid = Some(value.to_string()),
        "Disk / Partition UUID" => info.partition_uuid = Some(value.to_string()),
        // 未挂载时 diskutil 打的就是 "Mount Point: None"
        "Mount Point" if value != "None" => info.mount_point = Some(value.to_string()),
        _ => {}
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_disk_info(path: &Path) -> Option<DiskInfo> {
    let output = command("diskutil").arg("info").arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut info = DiskInfo::default();
    for line in text.lines() {
        macos_read_line(&mut info, line);
    }
    Some(info)
}

/// `diskutil info -all`：每块盘一段，段之间是一整行星号。未挂载的卷也能从这里看到。
#[cfg(target_os = "macos")]
pub(crate) fn macos_all_disks() -> Vec<DiskInfo> {
    let Ok(output) = command("diskutil").arg("info").arg("-all").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut disks = Vec::new();
    let mut current = DiskInfo::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.len() >= 8 && trimmed.chars().all(|character| character == '*') {
            if current.node.is_some() || current.identifier.is_some() {
                disks.push(std::mem::take(&mut current));
            }
            continue;
        }
        macos_read_line(&mut current, line);
    }
    if current.node.is_some() || current.identifier.is_some() {
        disks.push(current);
    }
    disks
}

/// macOS：路径 → 它所在的那块分区。
#[cfg(target_os = "macos")]
pub(crate) fn esp_probe(path: &Path) -> Result<EspInfo, String> {
    let root = fs::canonicalize(path)
        .map_err(|error| format!("路径打不开：{}（{error}）", path.display()))?;
    let info = macos_disk_info(&root)
        .ok_or_else(|| format!("diskutil 读不到 {}（分区已卸载？）", root.display()))?;
    let personality = info
        .personality
        .clone()
        .unwrap_or_else(|| "未知".to_string());
    let upper = personality.to_ascii_uppercase();
    let fat32 = upper.contains("FAT32");
    let fat = fat32 || upper.contains("FAT");
    // 识别键 = 分区 UUID（diskutil 的 `Disk / Partition UUID`，GPT PARTGUID）
    let partuuid = info
        .partition_uuid
        .clone()
        .filter(|value| grub::partuuid_digits(value).is_some());
    let note = match (&info.uuid, partuuid.is_some()) {
        (_, true) => {
            "分区 UUID 取自 diskutil 报的 Disk / Partition UUID（识别键，三端同值）".to_string()
        }
        (Some(raw), false) => format!(
            "diskutil 报的 Volume UUID {raw} 是 MD5 派生值（FAT 卷没有原生 UUID），\
             且没报出分区 UUID —— 请在 Linux（lsblk -o PARTUUID）或 Windows \
             （Get-Partition）里读同一分区的分区 UUID"
        ),
        (None, false) => "diskutil 没报分区 UUID —— 请在 Linux（lsblk -o PARTUUID）\
             或 Windows（Get-Partition）里读同一分区的分区 UUID"
            .to_string(),
    };
    Ok(EspInfo {
        path: path.to_string_lossy().to_string(),
        root: Some(
            info.mount_point
                .clone()
                .unwrap_or_else(|| root.display().to_string()),
        ),
        device: info
            .node
            .clone()
            .or_else(|| info.identifier.as_deref().map(|id| format!("/dev/{id}"))),
        fstype: Some(personality),
        fat,
        fat32,
        // fs-uuid：macOS 用户态拿不到（引导扇区要特权、Volume UUID 是派生值
        // 反推不回序列号）—— 如实留空，note 里说清楚。识别键用上面的 partuuid。
        uuid: None,
        partuuid,
        mounted: info.mount_point.is_some(),
        owned_mount: false,
        note: Some(note),
        mount_point: info.mount_point.clone(),
        preexisting: info.mount_point.is_some(),
    })
}

/// macOS：按分区 UUID 找到那块盘并 `diskutil mount`。
#[cfg(target_os = "macos")]
pub(crate) fn esp_mount(partuuid: &str) -> Result<Option<ActiveMount>, String> {
    // 已经挂着了（能在 /Volumes 下按分区 UUID 认出来）就不用动，owned=false 表示不归我们管
    if let Ok((dir, _)) = grub_esp_dir(partuuid) {
        return Ok(Some(ActiveMount {
            device: PathBuf::new(),
            mountpoint: dir,
            owned: false,
        }));
    }
    let node = macos_all_disks()
        .into_iter()
        .find(|info| {
            info.partition_uuid
                .as_deref()
                .is_some_and(|value| grub::uuid_matches(value, partuuid))
        })
        .and_then(|info| {
            info.node
                .or_else(|| info.identifier.map(|id| format!("/dev/{id}")))
        })
        .ok_or_else(|| {
            format!("没找到分区 UUID 为 {partuuid} 的分区（diskutil info -all 里没有对得上的）")
        })?;
    let output = command("diskutil")
        .arg("mount")
        .arg(&node)
        .output()
        .map_err(|error| format!("无法执行 diskutil mount: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "diskutil mount {node} 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mountpoint = macos_disk_info(Path::new(&node))
        .and_then(|info| info.mount_point)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{node} 挂上了，但 diskutil 没报出挂载点"))?;
    Ok(Some(ActiveMount {
        device: PathBuf::from(node),
        mountpoint,
        owned: true,
    }))
}

/// 盘符 → `(文件系统名, 卷序列号十六进制)`；盘符不存在或没有介质时返回 None。
///
/// 卷序列号是**文件系统**的 fs-uuid（GRUB `search --fs-uuid` 比的那串，
/// `E5377F31` 形态）—— 仅作展示；识别键是分区 UUID，走下面的 PowerShell。
#[cfg(target_os = "windows")]
pub(crate) fn windows_volume(letter: char) -> Option<(String, String)> {
    let root: Vec<u16> = format!("{}:\\", letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut serial: u32 = 0;
    let mut filesystem = vec![0u16; 32];
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    let end = filesystem
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(filesystem.len());
    Some((
        String::from_utf16_lossy(&filesystem[..end]),
        format!("{serial:08X}"),
    ))
}

/// 一次 PowerShell 调用枚举所有带盘符分区的 GUID：`盘符 → 分区 UUID`。
///
/// GPT 分区 GUID 与 Linux lsblk 的 PARTUUID、macOS diskutil 的
/// `Disk / Partition UUID` 是同一个值（UEFI 标准渲染，都是 8-4-4-4-12）。
/// Guid 属性经 ConvertTo-Json 序列化成 `"{…}"` 带花括号的串，要剥掉。
/// MBR 盘没有 GUID（报告全零），比不上就如实命不中。
#[cfg(target_os = "windows")]
pub(crate) fn windows_partition_guids() -> Vec<(char, String)> {
    // 第三次 PowerShell 冷启动（另外两次在 win_esp::list）。它只为了把
    // `grub_efi_partuuid` 跟盘符对上，没有缓存就是纯浪费 —— 埋点把它记下来，
    // 后面合并成一次 Get-Partition 时能直接对比。
    let _span = crate::perf::span("cmd.partition_guids");
    let output = match command("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Partition | Where-Object DriveLetter | Select-Object DriveLetter,Guid | ConvertTo-Json -Compress",
        ])
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    let parsed: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    // 单个分区时 ConvertTo-Json 出对象而不是数组 —— 统一当数组处理
    let rows = match parsed {
        serde_json::Value::Array(rows) => rows,
        row @ serde_json::Value::Object(_) => vec![row],
        _ => return Vec::new(),
    };
    rows.into_iter()
        .filter_map(|row| {
            // DriveLetter 是 `[char]`：ConvertTo-Json 通常出 "S"，个别 PowerShell
            // 版本会写成码点 83 —— `win_esp::parse_json` 已经为这个形状做过容错，
            // 这里是同一套处理（否则那条分区只是「查不到 partuuid」，不会报错，
            // 也很难发现）。
            let letter = match row.get("DriveLetter")? {
                serde_json::Value::String(text) => text.trim().chars().next()?,
                serde_json::Value::Number(number) => number
                    .as_u64()
                    .and_then(|code| u8::try_from(code).ok())
                    .map(char::from)?,
                _ => return None,
            }
            .to_ascii_uppercase();
            let guid = row.get("Guid")?.as_str()?;
            Some((letter, guid.trim_matches(['{', '}']).to_string()))
        })
        .collect()
}
