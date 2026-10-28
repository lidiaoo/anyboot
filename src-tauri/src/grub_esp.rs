//! GRUB/ESP 链路：定位、提权写入、扫描切换与命令面。依赖 mount.rs 与
//! grub_install.rs；grubenv 同步语义见 docs。

use super::*;

/// 切换 GRUB 区块的展示方式（内嵌 / 加宽 / 列表独立 / GRUB 独立）：
/// 落盘 + 按新模式调窗口宽度（460 ⇄ 932）。布局搬移由前端按快照里的
/// grubListView 自己做 —— 后端只管设置与窗口尺寸这两件它该管的事。
#[tauri::command]
pub(crate) fn set_grub_list_view(app: AppHandle, mode: String) -> Result<SettingsView, String> {
    let mode = normalize_grub_list_view(&mode).to_string();
    trace(&app, "command", &format!("set_grub_list_view mode={mode}"));
    let mut settings = stored_settings(&app);
    settings.grub_list_view = mode;
    save_settings(&app, &settings)?;
    apply_grub_layout_size(&app, true);
    Ok(publish_settings(&app))
}

/// GRUB 列表展示格式（原始/极简/简易/中等/复杂）：纯展示偏好，落盘 + 广播，
/// 渲染完全由前端按快照里的 grubListFormat 做 —— 后端不碰数据。
#[tauri::command]
pub(crate) fn set_grub_list_format(app: AppHandle, format: String) -> Result<SettingsView, String> {
    let format = normalize_grub_list_format(&format).to_string();
    trace(&app, "command", &format!("set_grub_list_format format={format}"));
    let mut settings = stored_settings(&app);
    settings.grub_list_format = format;
    save_settings(&app, &settings)?;
    Ok(publish_settings(&app))
}

/// 把 ESP 上的相对路径拼到本端 ESP 根目录下。
///
/// 不能直接用 `Path::join`：`ENV_REL_PATH` 以 `/` 开头，Windows 会把它当绝对
/// 路径从而丢掉盘符（`H:\`.join("/EFI/grubenv") 会变成「当前盘」的 `\EFI\grubenv`）。
pub(crate) fn grub_rel(dir: &Path, rel: &str) -> PathBuf {
    dir.join(rel.trim_start_matches('/'))
}

/// 原子写字节：先写同目录临时文件再改名（与 `atomic_copy` 同一套思路）。
pub(crate) fn atomic_write_bytes(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = temporary_target(target);
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

/// 09_anyboot 正文：只把占位符换成真值，其余一字不改（用户定案 1）。
///
/// `template` 正常就是内置的 `GRUB_SCRIPT_TEMPLATE`；用户在界面上改过脚本，就用
/// 他改的那份（设置项 `grub_script_override`）—— 「可展示 + 可修改」是明确要求。
pub(crate) fn grub_script_content(template: &str, uuid: &str) -> String {
    template.replace("@@ESP_UUID@@", uuid)
}

/// 当前生效的 09_anyboot 模板（内置 / 用户改过的）。
pub(crate) fn grub_template(settings: &Settings) -> &str {
    if settings.grub_script_override.trim().is_empty() {
        GRUB_SCRIPT_TEMPLATE
    } else {
        settings.grub_script_override.as_str()
    }
}

/// 09_anyboot 是否已安装。这个功能只在 Linux 有意义，所以非 Linux 返回 None
/// —— 界面据此**只在 Linux 展示**安装/卸载按钮。
#[cfg(target_os = "linux")]
pub(crate) fn grub_installed() -> Option<bool> {
    Some(Path::new(GRUB_SCRIPT_PATH).is_file())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn grub_installed() -> Option<bool> {
    None
}

/// 「上次同步」用的时间串：沿用项目既有的 UTC 记法（`utc_timestamp`），
/// 三端互相比对时不会因为本地时区而错位。
pub(crate) fn grub_synced_stamp() -> String {
    utc_timestamp(now_epoch_seconds())
}

/// 安装 / 卸载 09_anyboot 的**执行过程日志**：每跑一条外部命令就记一行，
/// 连同 stdout / stderr 与退出码一起。用户要知道「你到底做了什么」，光给一句
/// 「已安装」是不够的（一个 pkexec 弹窗背后的 `install` + `grub-mkconfig` 全看不见）。
///
/// 容量固定：这是给人看的最近一次操作的流水，不是审计日志，超过就从头丢。
pub(crate) const SCRIPT_LOG_LIMIT: usize = 200;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScriptLogView {
    /// 最近一次操作的名字（安装 09_anyboot / 卸载 09_anyboot）
    pub(crate) title: Option<String>,
    /// 逐行记录：命令行、stdout/stderr、退出码
    pub(crate) lines: Vec<String>,
    /// 这次操作整体成没成；None = 还没跑过
    pub(crate) ok: Option<bool>,
}

/// 往日志里追一行；超出上限就丢掉最老的。
pub(crate) fn script_log_push(lines: &mut Vec<String>, line: &str) {
    for part in line.lines() {
        lines.push(part.to_string());
    }
    if lines.len() > SCRIPT_LOG_LIMIT {
        let excess = lines.len() - SCRIPT_LOG_LIMIT;
        lines.drain(..excess);
    }
}

/// 把一次命令执行的结果按「退出码 → stdout → stderr」记进日志。
///
/// 只有 Linux 的安装 / 卸载会真正跑到它；其余平台留着是为了同一份代码不按平台分叉。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn script_log_output(lines: &mut Vec<String>, label: &str, output: &Output) {
    script_log_push(lines, &format!("[{label}] {}", output.status));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();
    if !stdout.is_empty() {
        script_log_push(lines, stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        script_log_push(lines, stderr);
    }
}

/// 记一条「命令根本没跑起来」的失败。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn script_log_error(lines: &mut Vec<String>, label: &str, error: &io::Error) {
    script_log_push(lines, &format!("[{label}] 无法执行: {error}"));
}

/// Windows：按分区 UUID 认盘（有盘符 = 可访问，没有就如实说）。
#[cfg(target_os = "windows")]
pub(crate) fn grub_esp_dir(partuuid: &str) -> Result<(PathBuf, Option<ActiveMount>), String> {
    let guids = windows_partition_guids();
    if guids.is_empty() {
        return Err(
            "读不到分区的 GUID（PowerShell Get-Partition 失败）：确认这是 GPT 磁盘"
                .to_string(),
        );
    }
    for (letter, guid) in guids {
        if grub::uuid_matches(&guid, partuuid) {
            return Ok((PathBuf::from(format!("{letter}:\\")), None));
        }
    }
    Err(format!(
        "没找到分区 UUID 为 {partuuid} 的分区：确认它已在「磁盘管理」里分配了盘符"
    ))
}

/// Windows 上「已挂载 = 有盘符」，所以 `grub_esp_dir` 本身就是挂载扫描 —— 不用再来一遍。
#[cfg(target_os = "windows")]
pub(crate) fn find_mounted_by_partuuid(partuuid: &str) -> Option<PathBuf> {
    grub_esp_dir(partuuid).ok().map(|(dir, _)| dir)
}

/// Linux：`lsblk --json` 里按**分区 UUID**（PARTUUID 列）认盘。已挂载就用现成
/// 挂载点；没挂载才由我们用 `mount`（必要时 pkexec 提权）以 **rw** 挂上。
#[cfg(target_os = "linux")]
pub(crate) fn grub_esp_dir(partuuid: &str) -> Result<(PathBuf, Option<ActiveMount>), String> {
    let _span = crate::perf::span("cmd.lsblk_grub_esp");
    let output = command("lsblk")
        .args(["--json", "--output", "PATH,FSTYPE,UUID,PARTUUID,MOUNTPOINTS"])
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("无法执行 lsblk: {error}"))?;
    let devices: LsblkOutput = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("无法解析 lsblk 输出: {error}"))?;

    for device in devices.blockdevices {
        let (Some(path), Some(candidate)) = (device.path.clone(), device.partuuid.clone()) else {
            continue;
        };
        if !grub::uuid_matches(&candidate, partuuid) {
            continue;
        }
        // 已经挂载：直接用用户的挂载点，绝不擅自改动别人的挂载
        if let Some(mountpoint) = device
            .mountpoints
            .clone()
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .find(|mountpoint| !mountpoint.is_empty())
        {
            return Ok((PathBuf::from(mountpoint), None));
        }

        let mountpoint = esp_mountpoint();
        // 只建目录、不先 remove_dir：这个路径可能是上一次自己挂上的活挂载点，
        // 删不掉也没关系 —— mount(8) 允许挂到非空目录。
        if !mountpoint.is_dir() {
            fs::create_dir_all(&mountpoint)
                .map_err(|error| format!("无法创建临时挂载点: {error}"))?;
        }
        let filesystem = device.fstype.clone().unwrap_or_else(|| "vfat".to_string());
        match mount_command("mount")
            .args(["-o", &mount_options("rw"), "-t", &filesystem])
            .arg(&path)
            .arg(&mountpoint)
            .output()
        {
            Ok(result) if result.status.success() => {
                return Ok((
                    mountpoint.clone(),
                    Some(ActiveMount {
                        device: PathBuf::from(path),
                        mountpoint,
                        owned: true,
                    }),
                ));
            }
            Ok(result) => {
                let _ = fs::remove_dir(&mountpoint);
                return Err(format!(
                    "挂载 ESP 失败: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ));
            }
            Err(error) => {
                let _ = fs::remove_dir(&mountpoint);
                return Err(format!("无法执行 mount: {error}"));
            }
        }
    }
    Err(format!("没找到分区 UUID 为 {partuuid} 的分区（lsblk 里没有对得上的 PARTUUID）"))
}

/// Linux：**用户已经挂上的**那块分区，按分区 UUID 找它的挂载点。
///
/// 和 `grub_esp_dir` 的区别：这个**绝不挂载**，只用 `lsblk` 已经报出来的
/// MOUNTPOINTS —— 用户自己挂好之后，我们直接把那个路径拿来用。
#[cfg(target_os = "linux")]
pub(crate) fn find_mounted_by_partuuid(partuuid: &str) -> Option<PathBuf> {
    let _span = crate::perf::span("cmd.lsblk_mounted");
    let output = command("lsblk")
        .args(["--json", "--output", "PARTUUID,MOUNTPOINTS"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let devices: LsblkOutput = serde_json::from_slice(&output.stdout).ok()?;
    for device in devices.blockdevices {
        let Some(candidate) = device.partuuid.as_deref() else {
            continue;
        };
        if !grub::uuid_matches(candidate, partuuid) {
            continue;
        }
        let mountpoints: Vec<String> = device
            .mountpoints
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .filter(|mountpoint| !mountpoint.is_empty())
            .collect();
        if mountpoints.is_empty() {
            continue;
        }
        // 一块分区可以同时挂在多处（fstab 的 /boot/efi + 桌面的自动挂载）。
        // 优先取 /boot 下的挂载点 —— 那是 fstab 方案里「系统已经挂好的引导
        // 数据」；/media 之类的自动挂载只是顺手，不该被当成首选。
        return mountpoints
            .iter()
            .find(|mountpoint| under_boot(mountpoint))
            .or_else(|| mountpoints.first())
            .map(PathBuf::from);
    }
    None
}

/// 挂载点是否在 `/boot` 下（fstab 统一方案的约定位置）。纯函数，便于单测。
#[cfg(target_os = "linux")]
pub(crate) fn under_boot(mountpoint: &str) -> bool {
    mountpoint == "/boot" || mountpoint.starts_with("/boot/")
}

/// 「同步」按钮的硬性条件（用户定案）：当前系统把这个分区挂在了 **`/boot` 或
/// `/boot/efi`** —— 两个位置都算数。挂别处（/media 之类）或没挂都不同步。
/// 纯函数，便于单测。
#[cfg(target_os = "linux")]
pub(crate) fn sync_mount_ok(mountpoint: &str) -> bool {
    mountpoint == "/boot" || mountpoint == "/boot/efi"
}

/// 启动/刷新链路的同步硬性条件（用户定案）：ESP 必须已经由**当前系统**
/// 挂载好（fstab 挂在 /boot 下），启动同步才会进行；否则**一个字节都不动**
/// —— 不自动挂载、不 pkexec、不写盘。判据：
/// * 填了分区 UUID → `lsblk` 里它有挂载点且在 /boot 下；
/// * 只填了目录 → `findmnt --target` 报的挂载点在 /boot 下（目录没挂上时
///   报的是根挂载 `/`，直接不通过 —— 防止把 grub.cfg 写进根文件系统的
///   普通目录）。
#[cfg(target_os = "linux")]
pub(crate) fn grub_esp_ready(settings: &Settings) -> bool {
    let uuid = settings.grub_efi_partuuid.trim();
    let dir = settings.grub_efi_dir.trim();
    if !uuid.is_empty() {
        return find_mounted_by_partuuid(uuid)
            .is_some_and(|mount| under_boot(&mount.display().to_string()));
    }
    if !dir.is_empty() {
        let path = Path::new(dir);
        if !path.is_dir() {
            return false;
        }
        return esp_probe(path)
            .ok()
            .and_then(|info| info.mount_point)
            .is_some_and(|target| under_boot(&target));
    }
    false
}

/// 非 Linux：`grub_collect` 是空操作，启动同步没有副作用，硬性条件恒满足。
#[cfg(not(target_os = "linux"))]
pub(crate) fn grub_esp_ready(_settings: &Settings) -> bool {
    true
}

/// 启动/刷新链路的「启动识别」（用户定案 2026-10-13）：**打开软件就调用一次
/// 识别**，UUID 优先、目录兜底。识别只解析不写盘（sync=false），解析出的挂载点
/// 经 `discovered` 回填设置里的「ESP 目录」并显示到目录框。
///
/// * UUID 填了就直接走 `grub_scan` —— 与「识别」按钮同一条链路：先看已挂载的，
///   没挂载按需挂载（Linux 上 mount 失败/pkexec 被取消 → Err，回落目录分支）。
///   这取代了早先「启动绝不挂载」的定案：UUID 的意义就在于不用管它挂在哪。
/// * 目录兜底：清掉 UUID 再扫（识别链路不许再触发第二次按需挂载），分区
///   UUID 取到了回填（仅在 UUID 本来为空时 —— 用户填了的 UUID 不被覆盖）。
/// 两个都无效 → 返回 None，调用方维持现状。
/// 返回 `(scan, 落选时待归还的挂载具柄)`。
pub(crate) fn startup_identify_scan(
    app: &AppHandle,
    settings: &Settings,
    reusable: Option<ActiveMount>,
) -> (Option<GrubScan>, Option<ActiveMount>) {
    let uuid = settings.grub_efi_partuuid.trim();
    let dir = settings.grub_efi_dir.trim();

    // ① UUID 优先：grub_locate 先看已挂载的，没挂载按需挂载（= 识别按钮行为）
    if !uuid.is_empty() {
        match grub_scan(settings, reusable, false) {
            Ok(scan) => {
                trace(app, "grub", "startup identify: via partuuid");
                return (scan, None);
            }
            // 定位失败（没这分区 / 挂载被取消）：reusable 已被 grub_scan 收尾，
            // 回落目录分支再试一次 —— 目录可能指向手动挂载的位置。
            Err(error) => {
                trace(app, "grub", &format!("startup identify: partuuid failed: {error}"));
                return fall_back_to_dir(app, settings, None, uuid, dir);
            }
        }
    }

    fall_back_to_dir(app, settings, reusable, uuid, dir)
}

/// 目录兜底分支（UUID 缺失或定位失败时）：清掉 UUID 再扫，杜绝按需挂载。
pub(crate) fn fall_back_to_dir(
    app: &AppHandle,
    settings: &Settings,
    reusable: Option<ActiveMount>,
    uuid: &str,
    dir: &str,
) -> (Option<GrubScan>, Option<ActiveMount>) {
    if !dir.is_empty() && Path::new(dir).is_dir() {
        let mut scan_settings = settings.clone();
        scan_settings.grub_efi_partuuid = String::new();
        match grub_scan(&scan_settings, reusable, false) {
            Ok(Some(scan)) => {
                if !scan.esp.fat {
                    // 目录不是 FAT32：指错了地方，识别作废；具柄按 merge 结果归还
                    trace(app, "grub", "startup identify: dir is not FAT32, abort");
                    return (None, scan.mount);
                }
                trace(app, "grub", "startup identify: via dir");
                if uuid.is_empty() {
                    if let Some(partuuid) = scan
                        .esp
                        .partuuid
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let mut new_settings = settings.clone();
                        new_settings.grub_efi_partuuid = partuuid.to_string();
                        let _ = save_settings(app, &new_settings);
                    }
                }
                return (Some(scan), None);
            }
            // Ok(None) 到不了（dir 非空）；Err 时 grub_scan 已把具柄收尾
            _ => return (None, None),
        }
    }

    (None, reusable)
}

/// macOS：遍历已挂载的卷，按**分区 UUID**（`Disk / Partition UUID`）定位。
///
/// 键就是 GPT 分区表里的 GUID，Linux/Windows 报的是同一个值 —— 直接归一化
/// 比对即可，不需要任何派生换算。命不中就如实报错并提示先手动挂载。
#[cfg(target_os = "macos")]
pub(crate) fn grub_esp_dir(partuuid: &str) -> Result<(PathBuf, Option<ActiveMount>), String> {
    let entries =
        fs::read_dir("/Volumes").map_err(|error| format!("无法读取 /Volumes: {error}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(info) = macos_disk_info(&path) else {
            continue;
        };
        if info
            .partition_uuid
            .as_deref()
            .is_some_and(|value| grub::uuid_matches(value, partuuid))
        {
            return Ok((path, None));
        }
    }
    Err(format!(
        "没在 /Volumes 下找到分区 UUID 为 {partuuid} 的 ESP；请先在 macOS 上手动挂载该分区"
    ))
}

/// macOS：已挂载的卷里按分区 UUID 找（和 `grub_esp_dir` 同一套比对）。
#[cfg(target_os = "macos")]
pub(crate) fn find_mounted_by_partuuid(partuuid: &str) -> Option<PathBuf> {
    grub_esp_dir(partuuid).ok().map(|(dir, _)| dir)
}

/// 当前用户对 `path` 有没有 `mode` 权限（Linux 的 `access(2)`；`libc::R_OK` / `W_OK`）。
#[cfg(target_os = "linux")]
pub(crate) fn user_can(path: &Path, mode: libc::c_int) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(raw) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(raw.as_ptr(), mode) == 0 }
}

/// 这次写 ESP 要不要 root：目标目录写不进去（系统挂的 ESP），或源文件读不了（0600）。
#[cfg(target_os = "linux")]
pub(crate) fn esp_write_needs_root(source: Option<&Path>, target: &Path) -> bool {
    let dir_writable = target.parent().is_some_and(|dir| user_can(dir, libc::W_OK));
    let source_readable = source.is_none_or(|path| user_can(path, libc::R_OK));
    !dir_writable || !source_readable
}

/// ESP 上的副本是不是已经是源的这一份 —— 只看 size/mtime，**不读内容**
/// （源 0600，普通用户根本读不了）。
///
/// 为什么要判：每次写都要 root、每次 root 都要弹一次授权框，而 `/boot/grub/grub.cfg`
/// 只在用户跑 grub-mkconfig 时才变。ESP 是 FAT，mtime 只有 **2 秒**精度
/// （`install -p` 落盘时秒以下会被截掉），所以留 2 秒容差。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn same_size_and_time(source: &Path, target: &Path) -> bool {
    let (Ok(source_meta), Ok(target_meta)) = (fs::metadata(source), fs::metadata(target)) else {
        return false;
    };
    if source_meta.len() != target_meta.len() {
        return false;
    }
    let Ok(source_time) = source_meta.modified() else {
        return false;
    };
    let Ok(target_time) = target_meta.modified() else {
        return false;
    };
    let delta = match source_time.duration_since(target_time) {
        Ok(delta) => delta,
        Err(error) => error.duration(),
    };
    delta <= Duration::from_secs(2)
}

/// 以 root 把 `source` 写到 `target`：`pkexec install -p -m 0644`（本来就是 root 就直连）。
///
/// * 读源和写目标必须是**同一条命令**：源 0600，普通用户连读都读不到，没法先在用户态
///   复制一份再提权写。
/// * `install -p` 把源的 mtime 一起带过去 —— 下一次 `same_size_and_time` 才认得出
///   「已同步」，否则每次扫描都要再授权一次。
#[cfg(target_os = "linux")]
pub(crate) fn privileged_install(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(error) = gui_auth_forbidden() {
        return Err(error);
    }
    let output = mount_command("install")
        .args(["-p", "-m", "0644"])
        .arg(source)
        .arg(target)
        .output()
        .map_err(|error| format!("无法执行 install（经 pkexec 提权）: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    Err(format!(
        "root 侧没写完：{}",
        if stderr.is_empty() {
            "install 退出码非 0".to_string()
        } else {
            format!("{stderr}（授权被取消？本机没有 polkit 授权代理？）")
        }
    ))
}

/// `$TMPDIR` 下一个本次调用专用的路径（与 `temporary_target` 同一套去重思路）。
#[cfg(target_os = "linux")]
pub(crate) fn unique_temp_path(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("{prefix}-{}-{unique}", std::process::id()))
}

/// 把内存里的字节落进 ESP：先写用户态暂存文件，再让 root `install` 过去。
///
/// 必须暂存：`pkexec` 只跑得动一条命令，而内容是我们现算出来的（ESP 上的环境块就是
/// 这样）。暂存文件 0600 —— 里面没有秘密，但没必要给它更宽的权限。
#[cfg(target_os = "linux")]
pub(crate) fn stage_and_install(bytes: &[u8], target: &Path) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let staging = unique_temp_path("anyboot-esp-stage");
    let staged = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = staged {
        let _ = fs::remove_file(&staging);
        return Err(format!("无法创建暂存文件 {}: {error}", staging.display()));
    }
    let result = privileged_install(&staging, target);
    let _ = fs::remove_file(&staging);
    result
}

/// 需要 root 才能把这份文件写进 ESP 时返回 `Some(结果)`；自己写得动就是 `None`。
#[cfg(target_os = "linux")]
pub(crate) fn esp_file_via_root(source: &Path, target: &Path) -> Option<Result<(), String>> {
    if !esp_write_needs_root(Some(source), target) {
        return None;
    }
    Some(privileged_install(source, target).and_then(|()| {
        // 提权这条路读不了源（0600），核对只能看 `install -p` 带过来的 size/mtime
        if same_size_and_time(source, target) {
            Ok(())
        } else {
            Err("写入后核对不一致（副本的 size/mtime 与源对不上）".to_string())
        }
    }))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn esp_file_via_root(_source: &Path, _target: &Path) -> Option<Result<(), String>> {
    None
}

/// 同上，但内容在内存里（先落暂存文件，见 `stage_and_install`）。
#[cfg(target_os = "linux")]
pub(crate) fn esp_bytes_via_root(target: &Path, bytes: &[u8]) -> Option<Result<(), String>> {
    esp_write_needs_root(None, target).then(|| stage_and_install(bytes, target))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn esp_bytes_via_root(_target: &Path, _bytes: &[u8]) -> Option<Result<(), String>> {
    None
}

/// 把一份**用户态已经算好的字节**写进 ESP（ESP 上的环境块走这条）。
pub(crate) fn write_esp_bytes(target: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(result) = esp_bytes_via_root(target, bytes) {
        return result;
    }
    atomic_write_bytes(target, bytes).map_err(|error| error.to_string())
}

/// 把系统里另一个文件同步进 ESP（`/boot/grub/grub.cfg` 走这条）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn copy_into_esp(source: &Path, target: &Path) -> Result<(), String> {
    // 已经是源的这一份就别再写：写一次就要一次 root 授权
    if same_size_and_time(source, target) {
        return Ok(());
    }
    if let Some(result) = esp_file_via_root(source, target) {
        return result;
    }
    atomic_copy(source, target).map_err(|error| error.to_string())?;
    // 副本带上源的 mtime：下一次 `same_size_and_time` 才认得出「已同步」
    if let Ok(modified) = fs::metadata(source).and_then(|meta| meta.modified()) {
        let _ = fs::OpenOptions::new()
            .write(true)
            .open(target)
            .and_then(|file| file.set_modified(modified));
    }
    if !hashes_match(source, target) {
        return Err("写入后校验不一致".to_string());
    }
    Ok(())
}

/// Linux：把 `/boot/grub` 的两份数据搬进 ESP。
///
/// * `grub.cfg` **每次都覆盖** —— 它就是 `update-grub` 的产物，内容必须跟得上。
/// * `grubenv` **只在 ESP 上还没有时才搬**（首次迁移）。装上 09_anyboot 之后
///   GRUB 只写 ESP 那一份，ext4 那份是旧的；每次覆盖会把 GRUB 刚记住的选择冲掉。
#[cfg(target_os = "linux")]
pub(crate) fn grub_collect(efi_dir: &Path) -> Result<(), String> {
    let source = ["/boot/grub/grub.cfg", "/boot/grub2/grub.cfg"]
        .iter()
        .map(Path::new)
        .find(|path| path.is_file())
        .ok_or_else(|| "没找到 /boot/grub/grub.cfg（或 /boot/grub2/grub.cfg）".to_string())?;
    let target = grub_rel(efi_dir, grub::CFG_REL_PATH);
    copy_into_esp(source, &target)
        .map_err(|error| format!("同步 grub.cfg 到 ESP 失败: {error}"))?;

    let env_target = grub_rel(efi_dir, grub::ENV_REL_PATH);
    if !env_target.is_file() {
        let existing = ["/boot/grub/grubenv", "/boot/grub2/grubenv"]
            .iter()
            .map(Path::new)
            .find(|path| path.is_file());
        let block = match existing.and_then(|path| fs::read(path).ok()) {
            // ext4 上那份是合法环境块就原样迁过去（保住当前的 saved_entry）
            Some(bytes) if grub::parse_env_block(&bytes).is_ok() => bytes,
            // 否则造一块干净的空块 —— GRUB 自己不会创建这个文件
            _ => grub::empty_env_block(),
        };
        write_esp_bytes(&env_target, &block)
            .map_err(|error| format!("在 ESP 上创建环境块失败: {error}"))?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn grub_collect(_efi_dir: &Path) -> Result<(), String> {
    // Windows / macOS 没有 /boot/grub 可搬：ESP 上的副本由 Linux 端负责更新
    Ok(())
}

/// GRUB 数据的一次读取结果（**不碰 AppState**）：GUI 与 CLI 共用同一份实现。
pub(crate) struct GrubScan {
    pub(crate) efi_dir: PathBuf,
    /// ESP 分区识别结果（是不是 FAT32 / fs-uuid 从哪来）
    pub(crate) esp: EspInfo,
    /// 我们自己挂上的挂载（用完要 `unmount`；用户已有的挂载是 None）
    pub(crate) mount: Option<ActiveMount>,
    pub(crate) entries: Vec<grub::GrubEntry>,
    /// submenu 分组的 (id, 标题) —— 前端做树形层级用
    pub(crate) groups: Vec<grub::GrubGroup>,
    pub(crate) env_path: PathBuf,
    pub(crate) cfg_path: PathBuf,
    pub(crate) env_exists: bool,
    pub(crate) saved_entry: Option<String>,
    pub(crate) next_entry: Option<String>,
    /// 这个目录是**按 fs-uuid 现找出来的**（不是用户填的那一行）——
    /// 调用方据此把找到的挂载点回填进设置，界面上「ESP 目录」那一行就会跟着变。
    pub(crate) discovered: bool,
}

impl GrubScan {
    pub(crate) fn release(self) {
        release_mount(self.mount);
    }
}

/// 把一次挂载交回去卸掉（`None` 就是什么都不做）。
pub(crate) fn release_mount(mount: Option<ActiveMount>) {
    if let Some(mount) = mount {
        let _ = mount.unmount();
    }
}

/// 决定这次该拿哪个挂载具柄：新挂的优先；否则复用上一次自己挂的那份
/// （**挂载点必须还是同一个**），其余的卸掉 —— 绝不能留下没人管得着的挂载。
pub(crate) fn merge_mount(
    fresh: Option<ActiveMount>,
    reusable: Option<ActiveMount>,
    dir: &Path,
) -> Option<ActiveMount> {
    match (fresh, reusable) {
        (Some(fresh), other) => {
            release_mount(other);
            Some(fresh)
        }
        (None, Some(old)) if old.mountpoint.as_path() == dir => Some(old),
        (None, other) => {
            release_mount(other);
            None
        }
    }
}

/// 按 fs-uuid 定位 ESP。
///
/// 顺序刻意是「**先看已经挂上的**，再考虑自己挂」：
/// 用户可能已经在系统里（或上次）把 ESP 挂好了 —— 那种情况下直接用它现成的挂载点，
/// 既不重复挂载，也顺手把这个路径回填到「ESP 目录」那一行给人看。
///
/// Linux 的 `grub_esp_dir` 本身就负责「没挂就挂上」；macOS 的只认已挂载的卷，
/// 所以这里给 macOS 补一次 `diskutil mount`（只在勾了「保持挂载」时做 ——
/// 用户没说要挂，就不该悄悄动他的磁盘）。
pub(crate) fn grub_locate(partuuid: &str, allow_mount: bool) -> Result<(PathBuf, Option<ActiveMount>), String> {
    // ① 已经挂着的：直接用（owned = false，不归我们管，也不会被我们卸掉）
    if let Some(dir) = find_mounted_by_partuuid(partuuid) {
        return Ok((dir, None));
    }
    // ② 没挂上：Linux 走 grub_esp_dir（会按需 mount），macOS 按需 diskutil mount
    match grub_esp_dir(partuuid) {
        Ok(found) => Ok(found),
        Err(error) => {
            if cfg!(target_os = "macos") && allow_mount {
                #[cfg(target_os = "macos")]
                match esp_mount(partuuid) {
                    Ok(Some(mount)) => return Ok((mount.mountpoint.clone(), Some(mount))),
                    Ok(None) => {}
                    Err(mount_error) => return Err(format!("{error}；{mount_error}")),
                }
            }
            Err(error)
        }
    }
}

/// 只读 ESP 上的 grub.cfg 与 grubenv。
///
/// * `Ok(Some(scan))` —— 定位到了 ESP（按分区 UUID，或按用户填的目录）
/// * `Ok(None)` —— 既没填分区 UUID 也没填目录，什么都不做
///
/// `reusable` 是上一次自己挂上的那份（勾了「保持挂载」才会传进来）。
/// `sync` 才会把 /boot/grub 的数据搬进 ESP（`grub_collect`）——**识别只解析，
/// 不写盘**（用户定案：同步只发生在启动自动链路和「同步」按钮，见 rescan_grub）。
pub(crate) fn grub_scan(
    settings: &Settings,
    reusable: Option<ActiveMount>,
    sync: bool,
) -> Result<Option<GrubScan>, String> {
    let uuid = settings.grub_efi_partuuid.trim().to_string();
    let dir = settings.grub_efi_dir.trim().to_string();
    if uuid.is_empty() && dir.is_empty() {
        release_mount(reusable);
        return Ok(None);
    }

    // ① 定位：填了分区 UUID 就按它找（先看已挂载的，再按需挂载）；只有目录就直接用目录
    let efi_dir_from_uuid = !uuid.is_empty();
    let located = if uuid.is_empty() {
        None
    } else {
        let Some(digits) = grub::partuuid_digits(&uuid) else {
            release_mount(reusable);
            return Err(format!(
                "分区 UUID 格式不对：{uuid}（GPT 分区 GUID，32 位标准 UUID，\
                 如 F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3）"
            ));
        };
        match grub_locate(&digits, settings.grub_keep_mounted) {
            Ok(found) => Some(found),
            Err(error) => {
                release_mount(reusable);
                return Err(error);
            }
        }
    };
    let (efi_dir, mount) = match located {
        Some((efi_dir, fresh)) => {
            let mount = merge_mount(fresh, reusable, &efi_dir);
            (efi_dir, mount)
        }
        None => {
            // 目录分支同样要按「同目录才复用，否则释放」收好手上的具柄 ——
            // 直接丢掉的话，勾了「保持挂载」的旧挂载会从此没人管得着。
            let mount = merge_mount(None, reusable, &PathBuf::from(&dir));
            (PathBuf::from(dir), mount)
        }
    };
    let discovered = efi_dir_from_uuid;

    // ② 挂上之后任何一步失败都要把挂载收掉，否则挂载点会越堆越多
    if sync {
        if let Err(error) = grub_collect(&efi_dir) {
            release_mount(mount);
            return Err(error);
        }
    }

    let cfg_path = grub_rel(&efi_dir, grub::CFG_REL_PATH);
    let env_path = grub_rel(&efi_dir, grub::ENV_REL_PATH);
    // grub.cfg 还没同步过来不算致命：先把状态摆出来，界面上会提示去点同步
    let cfg_text = fs::read_to_string(&cfg_path).ok();
    let raw_entries = cfg_text
        .as_deref()
        .map(grub::parse_grub_cfg)
        .unwrap_or_default();
    // 展示前折叠同名条目：os-prober 的 memtest86+（.bin/.efi）、31_efi_bootnext 的
    // `UEFI OS (EFI BootNext)`（0009/000A）标题逐字相同，铺两行只会让人以为重复了。
    let entries = grub::dedupe_display_entries(&raw_entries);
    let groups = cfg_text
        .as_deref()
        .map(grub::parse_submenus)
        .unwrap_or_default();
    let (env_exists, saved_entry, next_entry) = match fs::read(&env_path) {
        Ok(bytes) => (
            true,
            grub::env_value(&bytes, "saved_entry").filter(|value| !value.is_empty()),
            grub::env_value(&bytes, "next_entry").filter(|value| !value.is_empty()),
        ),
        Err(_) => (false, None, None),
    };

    // ③ 顺手把分区认一遍（探测失败**不算**致命：条目照样能用，只是分区信息缺一块）
    let mut esp = esp_probe(&efi_dir).unwrap_or_else(|error| EspInfo {
        path: efi_dir.display().to_string(),
        root: Some(efi_dir.display().to_string()),
        note: Some(error),
        ..EspInfo::default()
    });
    esp.owned_mount = mount.as_ref().is_some_and(|mount| mount.owned);

    Ok(Some(GrubScan {
        efi_dir,
        esp,
        mount,
        entries,
        groups,
        env_path,
        cfg_path,
        env_exists,
        saved_entry,
        next_entry,
        discovered,
    }))
}

/// 唯一的 GRUB 数据读取入口：只读 ESP 上的 grub.cfg 与 grubenv。
///
/// `sync=true` 才把 /boot/grub 的数据搬进 ESP。三条链路的分工（用户定案）：
/// * 启动/刷新/切换后（`refresh_grub`）：`(false, true)` —— 硬性条件满足才同步；
///   不满足时仍做一次「启动识别」（只解析 + 回填，UUID 优先、绝不挂载，
///   见 `startup_identify_scan`）；
/// * 识别（`detect_esp` / `detect_grub_by_partuuid`）与挂载勾选：`(true, false)`
///   —— 只解析，不写盘；
/// * 「同步」按钮（`sync_grub`）：`(true, true)` —— 自己先验挂载条件。
pub(crate) fn rescan_grub(app: &AppHandle, allow_mount: bool, sync: bool) -> Result<(), String> {
    let settings = stored_settings(app);
    // 上一次自己挂的那份：勾了「保持挂载」就留着复用（否则每次扫描都要重新
    // mount/umount 一遍，挂载点还会越堆越多）。
    let previous = {
        let state = app.state::<Mutex<AppState>>();
        let mut state = state.lock().expect("state mutex poisoned");
        state.grub.mount.take()
    };
    let reusable = if settings.grub_keep_mounted {
        previous
    } else {
        release_mount(previous);
        None
    };

    // 启动/刷新链路（`refresh_grub`，allow_mount=false）的硬性条件（用户定案）：
    // ESP 必须已经由**当前系统**挂载在 /boot 下，**同步**才进行 —— 否则不挂载、
    // 不写盘，绝不在后台偷偷 pkexec。但硬性条件不满足 ≠ 连解析都不做：填了
    // UUID 或目录就仍做一次「启动识别」（只解析 + 双向回填，UUID 优先，
    // 见 startup_identify_scan）。
    let scan = if !allow_mount && !grub_esp_ready(&settings) {
        match startup_identify_scan(app, &settings, reusable) {
            (Some(scan), _) => Some(scan),
            (None, reusable) => {
                if settings.grub_keep_mounted {
                    let state = app.state::<Mutex<AppState>>();
                    let mut state = state.lock().expect("state mutex poisoned");
                    state.grub.mount = reusable;
                }
                return Ok(());
            }
        }
    } else {
        grub_scan(&settings, reusable, sync)?
    };

    // 这次是「按 fs-uuid 现找出来的挂载点」——把它回填到设置里的 ESP 目录，
    // 界面上那一行就会显示真实位置（用户填了 fs-uuid 却看不到任何反馈，就是这个问题）。
    if let Some(scan) = &scan {
        if scan.discovered {
            let root = scan
                .esp
                .mount_point
                .clone()
                .or_else(|| scan.esp.root.clone())
                .unwrap_or_else(|| display_path(&scan.efi_dir).display().to_string());
            if settings.grub_efi_dir.trim() != root.trim() {
                let mut settings = stored_settings(app);
                settings.grub_efi_dir = root;
                let _ = save_settings(app, &settings);
            }
        }
    }

    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().expect("state mutex poisoned");
    state.grub = match scan {
        Some(scan) => GrubRuntime {
            efi_dir: Some(scan.efi_dir),
            esp: Some(scan.esp),
            mount: scan.mount,
            entries: scan.entries,
            groups: scan.groups,
            env_path: Some(scan.env_path),
            cfg_path: Some(scan.cfg_path),
            env_exists: scan.env_exists,
            saved_entry: scan.saved_entry,
            next_entry: scan.next_entry,
            installed: grub_installed(),
            synced_at: Some(grub_synced_stamp()),
            error: None,
        },
        None => GrubRuntime {
            installed: grub_installed(),
            ..GrubRuntime::default()
        },
    };
    Ok(())
}

/// 把一条 GRUB 侧的错误记进快照（保留「装没装 09_anyboot」这个仍可判定的信息）。
///
/// 注意别把挂载具柄顺手丢了：`GrubRuntime` 一被整体替换，旧的 `mount` 就没人管
/// 得着了（`ActiveMount` 没有 Drop），挂载点会一直留在那里。
pub(crate) fn apply_grub_error(app: &AppHandle, error: &str) {
    let keep = stored_settings(app).grub_keep_mounted;
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().expect("state mutex poisoned");
    let mount = state.grub.mount.take();
    let mount = if keep {
        mount
    } else {
        release_mount(mount);
        None
    };
    state.grub = GrubRuntime {
        installed: grub_installed(),
        mount,
        error: Some(error.to_string()),
        ..GrubRuntime::default()
    };
}

/// 重新解析 GRUB 数据，失败只记进快照（不打断 rEFInd 那条链路）。
///
/// **不发布**快照：调用它的 `refresh_and_update` 末尾会统一发布。
pub(crate) fn refresh_grub(app: &AppHandle) {
    // 自动链路：硬性条件（/boot 下已挂）满足才同步；识别链路不同步，见 rescan_grub。
    if let Err(error) = rescan_grub(app, false, true) {
        apply_grub_error(app, &error);
    }
}

/// 组装主窗口要的 GRUB 视图。
pub(crate) fn grub_snapshot(app: &AppHandle, settings: &Settings) -> GrubView {
    let (runtime, script_log) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (state.grub.clone(), state.script_log.clone())
    };
    let uuid = settings.grub_efi_partuuid.trim().to_string();
    let hint = if uuid.is_empty() {
        Some(
            "填 ESP 目录后点「识别」（自动判断是不是 FAT32 并取出分区 UUID），\
             或者直接手工填分区 UUID（三端都报同一个值：Linux lsblk -o PARTUUID / \
             macOS diskutil 的 Disk / Partition UUID / Windows Get-Partition 的 Guid）。\
             目录和 UUID 填一个即可，识别后二者双向同步"
                .to_string(),
        )
    } else if cfg!(target_os = "linux") {
        Some(
            "执行过 update-grub / grub-mkconfig 之后，点一次「同步」把新 grub.cfg \
             搬进 ESP（需已把该分区挂载到 /boot 或 /boot/efi）"
                .to_string(),
        )
    } else {
        Some("列表读的是 ESP 上的 grub.cfg 副本；Linux 端更新过 GRUB 后需重新同步".to_string())
    };
    GrubView {
        partuuid: uuid,
        dir: settings.grub_efi_dir.trim().to_string(),
        esp: runtime.esp.clone(),
        keep_mounted: settings.grub_keep_mounted,
        mount_supported: esp_mount_supported(),
        efi_dir: runtime
            .efi_dir
            .as_deref()
            .map(|path| display_path(path).display().to_string()),
        env_path: runtime
            .env_path
            .as_deref()
            .map(|path| display_path(path).display().to_string()),
        cfg_path: runtime
            .cfg_path
            .as_deref()
            .map(|path| display_path(path).display().to_string()),
        env_exists: runtime.env_exists,
        installed: runtime.installed,
        synced_at: runtime.synced_at,
        script_log,
        entries: runtime.entries,
        groups: runtime.groups,
        list_format: settings.grub_list_format.clone(),
        saved_entry: runtime.saved_entry,
        next_entry: runtime.next_entry,
        error: runtime.error,
        hint,
    }
}

/// 把 selector 写进指定的环境块文件（**不碰 AppState**，GUI 与 CLI 共用）。
///
/// 内容规则完全沿用原脚本：写 `saved_entry`（永久默认）**和** `next_entry`
/// （一次性），值取 grub.cfg 里解析出来的原文 —— **绝不自己拼 id**。
pub(crate) fn grub_write_env(env_path: &Path, selector: &str) -> Result<(), String> {
    let existing = fs::read(env_path).unwrap_or_else(|_| grub::empty_env_block());
    let updates = vec![
        ("saved_entry".to_string(), selector.to_string()),
        ("next_entry".to_string(), selector.to_string()),
    ];
    let block = grub::set_env_vars(&existing, &updates)?;
    write_esp_bytes(env_path, &block).map_err(|error| format!("写入环境块失败: {error}"))?;
    // 回读校验：写不进去必须当场发现，而不是等到开机才报 invalid environment block
    let written = fs::read(env_path).map_err(|error| format!("回读环境块失败: {error}"))?;
    if grub::env_value(&written, "next_entry").as_deref() != Some(selector) {
        return Err("环境块写入校验失败（回读的 next_entry 与预期不一致）".to_string());
    }
    Ok(())
}

/// 把用户点选的条目写进 ESP 上的环境块（先校验它在当前 grub.cfg 里确实存在）。
pub(crate) fn write_grub_entry(app: &AppHandle, selector: &str) -> Result<(), String> {
    let env_path = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        let env_path = state
            .grub
            .env_path
            .clone()
            .ok_or_else(|| "GRUB 环境块路径尚未解析出来".to_string())?;
        if !state
            .grub
            .entries
            .iter()
            .any(|entry| entry.selector == selector)
        {
            return Err(format!("grub.cfg 里没有这一项：{selector}"));
        }
        env_path
    };
    grub_write_env(&env_path, selector)
}

/// 切换下次启动的 GRUB 条目。只写环境块，**不重启**（与 rEFInd 那条链路一致）。
pub(crate) fn switch_grub_and_update(app: &AppHandle, selector: &str) -> AppStatus {
    trace(app, "grub", &format!("切换 entry={selector}"));
    set_busy(app, |busy| busy.switching = true);
    let outcome = write_grub_entry(app, selector);
    set_busy(app, |busy| busy.switching = false);

    match outcome {
        Ok(()) => {
            refresh_grub(app);
            let message = format!("下次启动：{selector}");
            record_action(app, "切换 GRUB 启动项", true, &message);
            publish_ui(app);
            state_snapshot(app)
        }
        Err(error) => {
            record_action(app, "切换 GRUB 启动项", false, &error);
            update_failure_status(app, "切换 GRUB 启动项失败", error)
        }
    }
}

/// 保存 ESP 的**分区 UUID**。填了它，主窗口才会出现「GRUB 启动项」区块。
#[tauri::command]
pub(crate) fn set_grub_partuuid(app: AppHandle, partuuid: String) -> Result<(), String> {
    trace(&app, "command", &format!("set_grub_partuuid partuuid={partuuid}"));
    let trimmed = partuuid.trim().to_string();
    if !trimmed.is_empty() && grub::partuuid_digits(&trimmed).is_none() {
        return Err(format!(
            "分区 UUID 不像有效值：{trimmed}（GPT 分区 GUID，32 位标准 UUID，\
             如 F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3）"
        ));
    }
    let mut settings = stored_settings(&app);
    settings.grub_efi_partuuid = trimmed;
    save_settings(&app, &settings)?;
    refresh_grub(&app);
    publish_ui(&app);
    Ok(())
}

/// 保存用户改过的 09_anyboot 正文；传空串 = 恢复内置模板。
/// 保存用户填的 ESP 目录**原文**；传空串 = 清空（分区 UUID 不动，它才是权威键）。
///
/// 与「识别」（detect_esp）的区别：这里不做 FAT32 校验、不取分区 UUID，
/// 纯落盘 —— 用户就是想改目录原文（包括清成空），识别才是校验+取键的入口。
#[tauri::command]
pub(crate) fn set_grub_dir(app: AppHandle, dir: String) -> Result<(), String> {
    trace(&app, "command", &format!("set_grub_dir dir={dir}"));
    let mut settings = stored_settings(&app);
    settings.grub_efi_dir = dir.trim().to_string();
    save_settings(&app, &settings)?;
    refresh_grub(&app);
    publish_ui(&app);
    Ok(())
}

/// 呼出**系统原生文件夹选择框**，让用户挑 ESP 分区里的目录去识别。
///
/// 返回选中的路径（取消返回 None）；识别本身走同一条 `detect_esp` ——
/// 成功后分区 UUID 与 ESP 目录一起落盘，「选文件夹」和「填分区 UUID」两个
/// 入口自动对上（分区 UUID 是权威键，目录只是入口）。
///
/// 走 tauri-plugin-dialog 的 Rust API（macOS NSOpenPanel / Windows
/// 文件夹对话框 / Linux GTK 选择器），不暴露前端 JS 插件层，所以也不需要
/// capabilities 权限配置。回调是异步回来的：阻塞等结果的活儿丢进
/// spawn_blocking，别占 async runtime 的工作线程。
#[tauri::command]
pub(crate) async fn pick_grub_dir(app: AppHandle) -> Result<Option<String>, String> {
    trace(&app, "command", "pick_grub_dir");
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        app.dialog()
            .file()
            .set_title("选择 ESP 分区里的文件夹")
            .pick_folder(move |path| {
                let picked = path.and_then(|p| p.into_path().ok()).map(|p| {
                    display_path(&p).display().to_string()
                });
                let _ = tx.send(picked);
            });
        rx.recv().map_err(|_| "文件夹选择框被关闭".to_string())
    })
    .await
    .map_err(|error| format!("文件夹选择框执行失败: {error}"))?
}

/// 识别 ESP 分区：判断是不是 FAT32，并把分区 UUID（识别键）填进设置。
///
/// 填目录是**可选的入口**：分区 UUID 才是权威键。识别成功后存的是**分区根目录**，
/// 所以用户填 `H:\EFI` 或 `/boot/efi/EFI/grub.cfg` 都会归一化成 `H:\` / `/boot/efi`。
#[tauri::command]
pub(crate) async fn detect_esp(app: AppHandle, path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || detect_esp_blocking(app, path))
        .await
        .map_err(|error| format!("detect_esp 任务失败: {error}"))?
}

/// 按分区 UUID 识别（UUID 行的「识别」按钮）：没挂载就先按需挂载 ——
/// 手动按钮，pkexec 授权是明说的行为（`grub_locate`：先看已挂的，没挂自己挂）。
/// 解析结果（挂载点）经由 `discovered` 回填进设置里的「ESP 目录」并保存；
/// 与目录识别同一条解析链路（sync=false），**识别不同步**。
#[tauri::command]
pub(crate) fn detect_grub_by_partuuid(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "detect_grub_by_partuuid");
    let mut settings = stored_settings(&app);
    let uuid = settings.grub_efi_partuuid.trim().to_string();
    if uuid.is_empty() {
        return Err("先填 ESP 的分区 UUID（或填目录点「识别」自动取）".to_string());
    }
    // 格式校验用归一化 digits，落盘存**用户填的原文**（trim 过）—— 否则识别
    // 一次就把 B3FBE669-D3B9-… 改写成 32 位无连字符全大写，看着像被改了值。
    // 匹配环节（uuid_matches / partuuid_digits）两边都归一化，原格式照样命中。
    if grub::partuuid_digits(&uuid).is_none() {
        return Err(format!(
            "分区 UUID 格式不对：{uuid}（GPT 分区 GUID，32 位标准 UUID，\
             如 F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3）"
        ));
    }
    settings.grub_efi_partuuid = uuid;
    save_settings(&app, &settings)?;

    if let Err(error) = rescan_grub(&app, true, false) {
        apply_grub_error(&app, &error);
        record_action(&app, "识别 ESP 分区", false, &error);
        publish_ui(&app);
        return Err(error);
    }
    let message = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        let dir = state
            .grub
            .efi_dir
            .as_ref()
            .map(|path| display_path(path).display().to_string())
            .unwrap_or_default();
        let parsed = state
            .grub
            .cfg_path
            .as_ref()
            .is_some_and(|path| path.is_file());
        if parsed {
            format!(
                "{dir}：已解析 grub.cfg（{} 个启动项），挂载点已回填到「ESP 目录」。\
                 目录和 UUID 填一个即可，二者已双向同步",
                state.grub.entries.len()
            )
        } else if dir.is_empty() {
            "按分区 UUID 识别完成，但还没定位到挂载点".to_string()
        } else {
            format!(
                "{dir}：分区上还没有 grub.cfg —— 点「同步」可从当前系统 /boot/grub 搬一份过来。\
                 目录和 UUID 填一个即可，二者已双向同步"
            )
        }
    };
    record_action(&app, "识别 ESP 分区", true, &message);
    publish_ui(&app);
    Ok(message)
}

/// 勾选「挂载 ESP」：勾上就按分区 UUID 挂上来并保持，取消就卸掉**自己挂的那份**。
///
/// 用户自己挂的（`owned = false`）绝不去卸 —— 那是别人的挂载。
#[tauri::command]
pub(crate) fn set_grub_mount(app: AppHandle, enabled: bool) -> Result<(), String> {
    trace(
        &app,
        "command",
        &format!("set_grub_mount enabled={enabled}"),
    );
    if enabled && !esp_mount_supported() {
        return Err(
            "本端不能按分区 UUID 挂载：请先在「磁盘管理」里给这个分区分配盘符，再填盘符路径"
                .to_string(),
        );
    }
    let mut settings = stored_settings(&app);
    if enabled && grub::partuuid_digits(&settings.grub_efi_partuuid).is_none() {
        return Err("先填好 ESP 的分区 UUID（或在上面填目录点「识别」自动取）".to_string());
    }
    settings.grub_keep_mounted = enabled;
    save_settings(&app, &settings)?;

    if !enabled {
        let mount = {
            let state = app.state::<Mutex<AppState>>();
            let mut state = state.lock().expect("state mutex poisoned");
            state.grub.mount.take()
        };
        release_mount(mount);
    }

    // 勾上 = 走一次扫描（里面会按分区 UUID 挂载并把具柄留下）；取消 = 上面已经卸了
    if enabled {
        set_busy(&app, |busy| busy.scanning = true);
        // 挂载勾选只保证「挂上 + 解析」，不同步 —— 同步是「同步」按钮的职责。
        let outcome = rescan_grub(&app, true, false);
        set_busy(&app, |busy| busy.scanning = false);
        if let Err(error) = outcome {
            // 挂不上（比如 pkexec 被取消）—— 把勾选退回未勾，别让界面骗人
            let mut settings = stored_settings(&app);
            settings.grub_keep_mounted = false;
            let _ = save_settings(&app, &settings);
            apply_grub_error(&app, &error);
            record_action(&app, "挂载 ESP", false, &error);
            publish_ui(&app);
            return Err(error);
        }
    }

    let message = if enabled {
        "ESP 已挂载，可以直接读写（取消勾选即卸载）"
    } else {
        "已卸载本程序挂上的 ESP"
    };
    record_action(&app, "挂载 ESP", true, message);
    publish_ui(&app);
    Ok(())
}

/// 用系统文件管理器打开 ESP 目录（三端都走同一条 `open_directory_in_explorer`）。
///
/// 解析顺序：内存里刚扫出来的目录 → 用户填的分区 UUID 现找（**含已挂载的**）→
/// 用户填的目录原文。都不行才报错，并把「该怎么办」说清楚。
#[tauri::command]
pub(crate) fn open_grub_dir(app: AppHandle) -> Result<String, String> {
    trace(&app, "command", "open_grub_dir");
    let resolved = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        state.grub.efi_dir.clone()
    };
    let settings = stored_settings(&app);

    // 已经解析出来、而且真的能打开，就直接用
    if let Some(dir) = resolved.filter(|dir| dir.is_dir()) {
        open_directory_in_explorer(&dir)?;
        return Ok(display_path(&dir).display().to_string());
    }

    // 内存里没有：按用户填的分区 UUID 现找一次（已挂载的优先）
    if let Some(digits) = grub::partuuid_digits(&settings.grub_efi_partuuid) {
        if let Ok((dir, mount)) = grub_locate(&digits, settings.grub_keep_mounted) {
            let result = open_directory_in_explorer(&dir);
            release_mount(mount);
            result?;
            return Ok(display_path(&dir).display().to_string());
        }
    }

    // 退到用户填的目录原文
    let raw = settings.grub_efi_dir.trim();
    if !raw.is_empty() {
        let dir = PathBuf::from(raw);
        if dir.is_dir() {
            open_directory_in_explorer(&dir)?;
            return Ok(display_path(&dir).display().to_string());
        }
        return Err(format!(
            "{raw} 现在打不开（ESP 没挂上？先填分区 UUID 或在上面填目录后点「识别」）"
        ));
    }

    Err("还不知道 ESP 在哪儿：先填分区 UUID，或在上面填目录后点「识别」".to_string())
}

/// 「同步」按钮：把当前系统 /boot/grub（或 /boot/grub2）的 grub.cfg 覆盖搬进
/// ESP，grubenv 没有才搬。硬性条件：该分区已挂载到 /boot 或 /boot/efi，
/// 否则不动（不自动挂载、不 pkexec、不写盘）。
#[tauri::command]
pub(crate) async fn sync_grub(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || sync_grub_blocking(app))
        .await
        .map_err(|error| format!("sync_grub 任务失败: {error}"))?
}

/// 切换下次启动的 GRUB 条目（只写环境块，不重启）。
#[tauri::command]
pub(crate) fn switch_grub(app: AppHandle, selector: String) -> AppStatus {
    trace(&app, "command", &format!("switch_grub selector={selector}"));
    switch_grub_and_update(&app, &selector)
}
