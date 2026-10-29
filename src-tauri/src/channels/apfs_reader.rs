//! Windows / Linux：零挂载只读解析 APFS 容器，翻出 Preboot 卷里的 macOS 引导条目。
//!
//! macOS **不编译本模块**（原生就能读 `/System/Volumes/Preboot`，见
//! `oc_menu::preboot_entries`）；这里是另外两端补齐预演 macOS 条目的路。
//! 方案选型与风险兜底见 `docs/apfs-预演三端统一选型.md`，要点：
//!
//! - **Preboot 不是分区**，是 APFS 容器内部的一个卷 —— GPT 里只有一个容器分区
//!   （类型 `7c3457ef-…`），把它当原始字节流喂给 `hadris-apfs`，卷/目录全部在
//!   内存里解析，不涉及任何挂载、卸载、盘符。
//! - **只需要三样东西**：UUID 目录列表、`boot.efi` 存在性、`.contentDetails`
//!   小文本 —— 都在 Preboot 卷（常规不加密），FileVault 数据卷根本不碰。
//! - **失败必须 loudly**：任何一步失败都返回带人话原因的 `Err`，由
//!   `oc_menu::preview` 补成一行 issue；绝不静默吞成空结果。
//!
//! 解析用 [`hadris_apfs`]（纯 Rust 只读，MIT，见 THIRD_PARTY_NOTICES）。
//! 为什么不用更活跃的 `apfs` crate：它的 `ApfsVolume::open` 只会挂载容器里
//! **第一个**卷，而 Preboot 通常排第三；`hadris_apfs::VolumeSelector::Name`
//! 能按卷名选卷，正中需求。

use super::*;

/// APFS 容器分区的 GPT 类型 UUID（小写）。全平台同一把尺子。
/// Windows 分支用 PowerShell 按 GUID 文本匹配，不引用本常量 —— 只在 Linux 编译目标下有调用者。
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) const CONTAINER_GPT_TYPE: &str = "7c3457ef-0000-11aa-aa11-00306543ecac";

/// Preboot 卷里的一条候选：一个 UUID 目录。
///
/// serde 只为跨进程边界：提权子进程把结果序列化进 JSON 文件、主进程反序列化读回。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PrebootItem {
    /// 目录名，全大写（`C8B0D4AC-…`），与 macOS 侧 `preboot_entries` 同一形态
    pub(crate) uuid: String,
    /// `<uuid>/System/Library/CoreServices/boot.efi` 是否存在
    pub(crate) has_boot_efi: bool,
    /// `.contentDetails` 的内容（菜单显示名）；读不到就是 `None`
    pub(crate) label: Option<String>,
}

// ─────────────────────────── 容器分区定位（纯函数 + 平台探测） ───────────────────────────

/// Linux：`lsblk -rno PATH,PARTTYPE` 的输出 → 容器分区设备路径。
///
/// `-r` 紧凑格式每行 `PATH PARTTYPE`；PARTTYPE 大小写不定（旧版 lsblk 小写），
/// 比较时统一小写。找不到返回 `None`，由调用方报「没有 APFS 容器分区」。
#[cfg(target_os = "linux")]
pub(crate) fn partition_path_from_lsblk(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let path = parts.next()?;
            let part_type = parts.next()?;
            (part_type.eq_ignore_ascii_case(CONTAINER_GPT_TYPE)).then(|| path.to_string())
        })
        .next()
}

/// Windows：APFS 容器分区的坐标。为什么只取坐标：Windows 不给未知文件系统的
/// 分区建卷路径（实测 `AccessPaths=null`），`\\?\Volume{…}` 这条路对 APFS
/// 分区根本不存在 —— 只能开整盘 `\\.\PhysicalDriveN` 再按偏移读。
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContainerLocation {
    pub(crate) disk: u32,
    pub(crate) offset: u64,
    pub(crate) size: u64,
}

/// Windows：容器分区的坐标（开整盘 + 按偏移读）。
///
/// 与 ESP 列表同源 —— 都来自 [`super::partitions`] 那张共享表。原来这里自己起
/// 一次全量分区查询，只为拿一个容器的偏移（一轮扫描第 4 次，约 1s）。
#[cfg(target_os = "windows")]
fn container_location() -> Result<ContainerLocation, String> {
    let rows = super::partitions::all();
    if rows.is_empty() {
        return Err("读不到分区表（枚举分区失败，可能是分区服务异常）".to_string());
    }
    // 不碰盘符：Windows 不给未知文件系统的分区建卷路径，只能开整盘按偏移读
    rows.into_iter()
        .find(|row| row.is_apfs() && row.size > 0)
        .map(|row| ContainerLocation {
            disk: row.disk,
            offset: row.offset,
            size: row.size,
        })
        .ok_or_else(|| "这台机器上没有 APFS 容器分区（没装 macOS，Preboot 自然无从谈起）".to_string())
}

/// Linux：容器分区设备路径（分区设备从分区第一个字节起，无需偏移换算）。
#[cfg(target_os = "linux")]
fn container_location() -> Result<String, String> {
    let _span = perf::span("cmd.lsblk_parttype");
    let output = command("lsblk")
        .args(["-rno", "PATH,PARTTYPE"])
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法调用 lsblk 枚举分区: {error}"))?;
    if !output.status.success() {
        return Err("lsblk 枚举失败（发行版太老没有 PARTTYPE 列？）".to_string());
    }
    partition_path_from_lsblk(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| "这台机器上没有 APFS 容器分区（没装 macOS，Preboot 自然无从谈起）".to_string())
}

// ─────────────────────────── 打开与扫描 ───────────────────────────

/// 打开原始设备。Windows 上卷设备读必须带共享模式，否则和系统自己的句柄打架。
#[cfg(target_os = "windows")]
fn open_raw(path: &str) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ | FILE_SHARE_WRITE：分区上可能挂着别的读句柄（如 win_esp 的探测）
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(path)
}

#[cfg(target_os = "linux")]
fn open_raw(path: &str) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// 把 IO 错误翻译成人话。权限问题单独点名 —— 这是两端最常见的失败原因。
fn io_reason(error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        "需要管理员/root 权限：读原始设备要提权".to_string()
    } else {
        format!("{error}")
    }
}

/// 扫 Preboot 卷，列出全部 UUID 目录。
///
/// 只做三件事：枚举根目录、试开 `boot.efi`、读 `.contentDetails`。整条链路
/// 只读，任何失败都向上报原因，不返回半真半假的空结果。
pub(crate) fn scan_preboot() -> Result<Vec<PrebootItem>, String> {
    // 也埋一层：提权重扫（scan_preboot_elevated）走的是同一个函数，但那条路
    // 不经过 oc_menu::preview，不埋就没有数字
    let _span = perf::span("apfs.scan");
    use hadris_apfs::{sync::ApfsFs, VolumeSelector};
    use hadris_fs::{sync::Volume, MountOptions, OpenOptions};
    use hadris_storage::host::FileDevice;

    // APFS 容器逻辑块固定 4096（NXSB 的 nx_block_size；不是 4096 的容器极罕见，
    // mount 阶段就会校验失败并被兜住）。FileDevice 的读全部按块对齐，原始设备读
    // 要求的扇区对齐自动满足。
    let block_size = hadris_storage::BlockSize::new(4096).expect("4096 是合法块大小");

    // Windows：开整盘 + Partition 偏移（APFS 分区没有卷路径，坐标由 container_location 给）；
    // Linux：直接开分区设备（分区设备天生从分区头开始，不用偏移）。
    #[cfg(target_os = "windows")]
    let device = {
        let location = container_location()?;
        let disk_path = format!(r"\\.\PhysicalDrive{}", location.disk);
        let file = open_raw(&disk_path)
            .map_err(|error| format!("打不开整盘 {disk_path}：{}", io_reason(&error)))?;
        let disk = FileDevice::with_block_size(file, block_size)
            .map_err(|error| format!("整盘设备 {disk_path} 初始化失败：{}", io_reason(&error)))?;
        hadris_storage::Partition::new(disk, location.offset, location.size)
    };
    #[cfg(target_os = "linux")]
    let device = {
        let path = container_location()?;
        let file = open_raw(&path)
            .map_err(|error| format!("打不开 APFS 容器分区 {path}：{}", io_reason(&error)))?;
        FileDevice::with_block_size(file, block_size)
            .map_err(|error| format!("APFS 容器设备 {path} 初始化失败：{}", io_reason(&error)))?
    };

    let fs = ApfsFs::mount_volume(device, MountOptions::new(), VolumeSelector::Name("Preboot"))
        .map_err(|mount| {
            let (error, _device) = mount.into_parts();
            match error.kind() {
                // NotFound = 容器里没有叫 Preboot 的卷；其余如实带上游信息
                hadris_fs::ErrorKind::NotFound => {
                    "APFS 容器里没有叫 Preboot 的卷（这台机器的容器结构不一般？）".to_string()
                }
                _ => format!("APFS 容器能打开但挂不上 Preboot 卷：{error}"),
            }
        })?;
    let volume = Volume::new(fs);

    // Preboot 根目录 = 一堆 UUID 目录（跳过 cryptexes 之类非 UUID 名）
    let listing = volume
        .read_dir("/")
        .map_err(|error| format!("读 Preboot 卷根目录失败：{error}"))?;
    let mut items: Vec<PrebootItem> = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|error| format!("遍历 Preboot 根目录中断：{error}"))?;
        let name = entry.name().to_string();
        if !is_uuid_dir(&name) || items.len() >= 32 {
            continue;
        }
        let boot_path = format!(
            "/{name}/System/Library/CoreServices/boot.efi"
        );
        let has_boot_efi = volume
            .open(boot_path.as_bytes(), OpenOptions::new().read())
            .is_ok();
        if !has_boot_efi {
            // 与 macOS 侧同语义：没有 boot.efi 的目录 OC 也不会列条目
            continue;
        }
        // 菜单显示名按 OC 的取名顺序试（BootEntryInfo.c:728/730）：
        // `.contentDetails` → `.disk_label.contentDetails`（实测 Preboot 卷上是后者，
        // 它是个文件，不是 ESP 上那种 `.disk_label/` 目录）→ 都没有就回退 UUID 短名
        let label = ["/.contentDetails", "/.disk_label.contentDetails"]
            .iter()
            .find_map(|tail| {
                let details_path = format!("/{name}/System/Library/CoreServices{tail}");
                let file = volume
                    .open(details_path.as_bytes(), OpenOptions::new().read())
                    .ok()?;
                let mut file = file;
                let mut buffer = Vec::new();
                std::io::Read::read_to_end(&mut file, &mut buffer).ok()?;
                let text = String::from_utf8_lossy(&buffer).trim().to_string();
                (!text.is_empty() && text.len() < 80).then_some(text)
            });
        items.push(PrebootItem {
            uuid: name.to_ascii_uppercase(),
            has_boot_efi,
            label,
        });
    }
    Ok(items)
}

/// UUID 形态的目录名（8-4-4-4-12 十六进制）。`cryptexes`、`.` 之类会被挡掉。
fn is_uuid_dir(name: &str) -> bool {
    let segments = [8usize, 4, 4, 4, 12];
    if name.len() != 36 {
        return false;
    }
    let bytes = name.as_bytes();
    let mut offset = 0;
    for (index, want) in segments.iter().enumerate() {
        if index > 0 {
            if bytes[offset] != b'-' {
                return false;
            }
            offset += 1;
        }
        if !bytes[offset..offset + want].iter().all(u8::is_ascii_hexdigit) {
            return false;
        }
        offset += want;
    }
    true
}

// ─────────────────────────── 提权重扫（按需 UAC / pkexec） ───────────────────────────
//
// 读 APFS 容器整盘在 Windows 恒要管理员（os error 5，只读也不例外）。不想让用户
// 「管理员身份重启整个 App」，就按需提权：弹一次授权，让**本程序自己**带
// `--apfs-scan-privileged` 参数跑一次扫描，结果经 JSON 文件回传、缓存在内存里 ——
// 之后预演每次重建都吃缓存，授权弹窗只在第一次（或用户点「重扫」）出现。
//
// 为什么不做成 macOS 那样的常驻助手：这里只是「偶尔读一次盘」，一次性的
// ShellExecute/pkexec 没有安装步骤、没有守护项、用完即退，安全面小得多。

/// 缓存有效期。Preboot 的 UUID 目录与 `.contentDetails` 几乎不变，
/// 30 分钟只是「理论上卷变了还拿旧数据」的护栏。
const ELEVATED_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// 提权扫描子进程的回传文件内容：`{ ok, items | error }`。
#[derive(serde::Deserialize, serde::Serialize)]
struct PrivilegedScanFile {
    ok: bool,
    #[serde(default)]
    items: Vec<PrebootItem>,
    #[serde(default)]
    error: Option<String>,
}

static ELEVATED_CACHE: std::sync::OnceLock<
    Mutex<Option<(std::time::Instant, Vec<PrebootItem>)>>,
> = std::sync::OnceLock::new();

/// 进程内提权缓存（预演重建时读；TTL 内不再弹授权）。
pub(crate) fn cached_preboot() -> Option<Vec<PrebootItem>> {
    let cell = ELEVATED_CACHE.get_or_init(|| Mutex::new(None));
    let guard = cell.lock().ok()?;
    let (at, items) = guard.as_ref()?;
    (at.elapsed() < ELEVATED_CACHE_TTL).then(|| items.clone())
}

fn store_cache(items: &[PrebootItem]) {
    let cell = ELEVATED_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = cell.lock() {
        *guard = Some((std::time::Instant::now(), items.to_vec()));
    }
}

/// 「提权重扫」命令的返回（camelCase 直达前端）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ElevatedScanOutcome {
    pub(crate) items: Vec<PrebootItem>,
    pub(crate) from_cache: bool,
    /// 给状态栏的一句人话（成功/取消/失败原因都在这）
    pub(crate) note: String,
}

/// 「提权重扫」入口。**必须从 async 命令调**：Windows 的 ShellExecuteExW 会同步等
/// UAC 对话框，放在同步命令里就是又一次「点了就卡死」。
///
/// `force=false` 先吃缓存；当前进程本来就有权限（用户用管理员跑的 App）就直接扫，
/// 不折腾授权。`force=true` 无条件重弹授权重扫。
///
/// `elevation` 见 [`super::Elevation`]：`Silent`（开机自动扫）只走特权服务，
/// 服务不可用就报错、绝不弹 UAC —— 开机时没人盯着屏幕，也没人点得到那口授权框。
pub(crate) fn scan_preboot_elevated(
    force: bool,
    elevation: super::Elevation,
) -> Result<ElevatedScanOutcome, String> {
    if !force {
        if let Some(items) = cached_preboot() {
            let count = items.len();
            return Ok(ElevatedScanOutcome {
                items,
                from_cache: true,
                note: format!("读的是提权扫描缓存（{count} 条，30 分钟内有效）——卷有变动请点「重扫」"),
            });
        }
        match scan_preboot() {
            Ok(items) => {
                let count = items.len();
                store_cache(&items);
                return Ok(ElevatedScanOutcome {
                    items,
                    from_cache: false,
                    note: format!("当前权限直接扫描成功（{count} 条），没弹授权"),
                });
            }
            // 权限（或环境）问题 → 走提权。失败原因不用带出来，提权流程会给自己的。
            Err(_) => {}
        }
    }
    let items = run_privileged_scan(elevation)?;
    store_cache(&items);
    Ok(ElevatedScanOutcome {
        items,
        from_cache: false,
        note: if elevation.is_silent() {
            "已自动提权扫描（特权服务，没弹授权），本页已刷新".to_string()
        } else {
            "提权扫描完成，本页已刷新".to_string()
        },
    })
}

/// 走特权服务（若已安装），否则弹一次授权 → 等子进程退出 → 读回结果文件。
/// `elevation=Silent` 时只剩前半句：服务不在就直接失败。
fn run_privileged_scan(elevation: super::Elevation) -> Result<Vec<PrebootItem>, String> {
    // 拉起与等待的机制在 elevate.rs（与「OC 设默认项」共用同一套自拉起通道）
    let out_path = super::elevate::result_file_path("apfs-scan");
    let args = vec![
        "--apfs-scan-privileged".to_string(),
        "--out".to_string(),
        out_path.to_string_lossy().to_string(),
    ];
    if elevation.is_silent() {
        super::elevate::run_silent_op("apfs_scan", serde_json::json!({}), &out_path)?;
    } else {
        super::elevate::run_privileged_op("apfs_scan", serde_json::json!({}), &args, &out_path)?;
    }
    let text = std::fs::read_to_string(&out_path)
        .map_err(|error| format!("提权子进程退出了但结果文件读不到：{error}"))?;
    let payload: PrivilegedScanFile = serde_json::from_str(&text)
        .map_err(|error| format!("提权扫描结果不是合法 JSON：{error}"))?;
    if payload.ok {
        Ok(payload.items)
    } else {
        Err(payload
            .error
            .unwrap_or_else(|| "提权扫描失败（子进程没给原因）".to_string()))
    }
}

/// 跑一次 Preboot 扫描并序列化成 JSON 文本。
///
/// 返回值就是「原本要写进 `--out` 的那段文本」：一次性授权子进程把它落盘，
/// 特权服务直接经管道把它送回主进程 —— 两条路产出的字节完全一致，下游解析器
/// 不需要知道这次是弹了 UAC 还是走了服务。
pub(crate) fn privileged_scan_json() -> Result<String, String> {
    let payload = match scan_preboot() {
        Ok(items) => PrivilegedScanFile {
            ok: true,
            items,
            error: None,
        },
        Err(reason) => PrivilegedScanFile {
            ok: false,
            items: Vec::new(),
            error: Some(reason),
        },
    };
    serde_json::to_string(&payload).map_err(|error| format!("序列化扫描结果失败：{error}"))
}

/// `--apfs-scan-privileged` 子命令体：跑一次扫描，把结果**原子**写进 `--out` 文件。
/// 只在提权子进程里被调（main.rs 把它排在所有分支最前，绝不进 GUI）。
pub(crate) fn privileged_scan_main(out_path: &str) -> i32 {
    match privileged_scan_json() {
        Ok(text) => super::write_result_file(out_path, &text),
        Err(error) => {
            eprintln!("{error}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_shape_gate() {
        assert!(is_uuid_dir("C8B0D4AC-1234-5678-9ABC-DEF012345678"));
        assert!(is_uuid_dir("c8b0d4ac-1234-5678-9abc-def012345678"));
        assert!(!is_uuid_dir("cryptexes"));
        assert!(!is_uuid_dir("."));
        assert!(!is_uuid_dir("C8B0D4AC-1234-5678-9ABC-DEF01234567")); // 少一位
        assert!(!is_uuid_dir("C8B0D4AC_1234-5678-9ABC-DEF012345678"));
    }

    /// lsblk 紧凑格式：PATH 在前 PARTTYPE 在后，大小写不敏感地认容器类型。
    #[cfg(target_os = "linux")]
    #[test]
    fn lsblk_lines_pick_container() {
        let text = "/dev/nvme0n1p1 c12a7328-f81f-11d2-ba4b-00a0c93ec93b\n\
                    /dev/nvme0n1p2 0fc63daf-8483-4772-8e79-3d69d8477de4\n\
                    /dev/nvme0n1p3 7C3457EF-0000-11AA-AA11-00306543ECAC\n";
        assert_eq!(
            partition_path_from_lsblk(text).as_deref(),
            Some("/dev/nvme0n1p3")
        );
        assert_eq!(partition_path_from_lsblk("/dev/sda1 c12a7328-f81f-11d2-ba4b-00a0c93ec93b"), None);
    }

    /// 真机集成验证（`cargo test real_machine_scan -- --ignored --nocapture`）：
    /// 只在装了 macOS 的机器上手动跑。非提权终端里应当给出人话的权限报错，
    /// 而不是 panic 或静默空结果。
    #[test]
    #[ignore = "碰真实磁盘，手动跑"]
    fn real_machine_scan() {
        match scan_preboot() {
            Ok(items) => {
                assert!(!items.is_empty(), "本机有 APFS 容器，Preboot 不该是空的");
                for item in &items {
                    println!("{}  boot.efi={}  label={:?}", item.uuid, item.has_boot_efi, item.label);
                }
            }
            Err(reason) => println!("扫描失败（非提权终端的预期形态）：{reason}"),
        }
    }

    /// 临时探针：看 Preboot/<uuid>/System/Library/CoreServices 里到底有什么、
    /// .contentDetails 为什么读不到（`--ignored --nocapture` 手动跑）。
    #[test]
    #[ignore = "碰真实磁盘，手动跑"]
    fn probe_core_services() {
        use hadris_apfs::{sync::ApfsFs, VolumeSelector};
        use hadris_fs::{sync::Volume, MountOptions};
        use hadris_storage::host::FileDevice;
        let location = container_location().expect("定位容器");
        let file = open_raw(&format!(r"\\.\PhysicalDrive{}", location.disk)).expect("开整盘");
        let block_size = hadris_storage::BlockSize::new(4096).unwrap();
        let disk = FileDevice::with_block_size(file, block_size).expect("块设备");
        let device = hadris_storage::Partition::new(disk, location.offset, location.size);
        let fs = ApfsFs::mount_volume(device, MountOptions::new(), VolumeSelector::Name("Preboot"))
            .expect("挂 Preboot");
        let volume = Volume::new(fs);
        let listing = volume
            .read_dir("/")
            .expect("列根目录");
        for entry in listing.flatten() {
            let name = entry.name().to_string();
            if !is_uuid_dir(&name) {
                continue;
            }
            let base = format!("/{name}/System/Library/CoreServices");
            println!("== {base}");
            match volume.read_dir(base.as_bytes()) {
                Ok(rows) => {
                    for row in rows.flatten() {
                        println!("   {:?} {}", row.file_type(), row.name());
                    }
                }
                Err(error) => println!("   列目录失败：{error}"),
            }
            let details = format!("{base}/.disk_label.contentDetails");
            match volume.open(details.as_bytes(), hadris_fs::OpenOptions::new().read()) {
                Ok(mut file) => {
                    let mut buffer = Vec::new();
                    match std::io::Read::read_to_end(&mut file, &mut buffer) {
                        Ok(_) => println!("   .contentDetails = {buffer:?}"),
                        Err(error) => println!("   .contentDetails 读失败：{error}"),
                    }
                }
                Err(error) => println!("   .contentDetails 打不开：{error}"),
            }
        }
    }
}
