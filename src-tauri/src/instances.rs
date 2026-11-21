//! 引导器实例发现层（多实例模型）。
//!
//! 一个「实例」= 某块 ESP（或 Linux /boot）上的一套引导器安装：
//! `instanceId = "<kind>@<PARTUUID或根路径>/<安装相对目录>"`，稳定、跨重启不变。
//! 旧的单实例 `kind` 只作为 primary 的兼容映射。

use serde::Serialize;
use tauri::AppHandle;
use tauri::Manager;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootInstanceRef {
    pub kind: String,
    /// 唯一且稳定：kind@分区标识/安装目录（小写归一）
    pub instance_id: String,
    /// 展示名：类型 · 分区标识（可被设置里的别名覆盖）
    pub title: String,
    /// 分区标识（PARTUUID 优先）
    pub partition: String,
    /// esp / linux-boot
    pub origin: String,
    /// ESP 内相对安装目录（或 Linux /boot 下的路径）
    pub install_dir: String,
    pub available: bool,
    /// 是否被设置隐藏（不删除，只是不在主列表展示）
    pub hidden: bool,
    /// 候选根（内部用，不序列化）
    #[serde(skip)]
    pub root_path: String,
}

const KINDS: [&str; 6] = ["refind", "refindplus", "grub", "sdboot", "opencore", "clover"];

fn kind_title(kind: &str) -> &'static str {
    match kind {
        "refind" => "rEFInd",
        "refindplus" => "RefindPlus",
        "grub" => "GRUB",
        "sdboot" => "systemd-boot",
        "opencore" => "OpenCore",
        "clover" => "Clover",
        _ => "引导器",
    }
}

/// 递归找指定文件名（最多 5 层，跳过符号链接），返回所在目录列表。
fn dirs_containing(root: &Path, file_name: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, name: &str, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 5 {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                walk(&path, name, depth + 1, out);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                if let Some(parent) = path.parent() {
                    if !out.iter().any(|known| known == parent) {
                        out.push(parent.to_path_buf());
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, file_name, 0, &mut out);
    out
}

/// 递归找文件名匹配前缀/后缀的第一个目录（用于 *.efi 系列）。
fn dirs_with_efi(root: &Path, contains: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, needle: &str, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 5 {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                walk(&path, needle, depth + 1, out);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".efi") && lower.contains(needle) {
                    if let Some(parent) = path.parent() {
                        if !out.iter().any(|known| known == parent) {
                            out.push(parent.to_path_buf());
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, contains, 0, &mut out);
    out
}

/// 与 dirs_with_efi 相同，但排除文件名含 `exclude` 的（如 RefindPlus 的 x64_RefindPlus_REL.efi 不能被当成 rEFInd）。
fn dirs_with_efi_excluding(root: &Path, needle: &str, exclude: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, needle: &str, exclude: &str, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 5 { return; }
        let entries = match fs::read_dir(dir) { Ok(v) => v, Err(_) => return };
        for entry in entries.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() { Ok(v) => v, Err(_) => continue };
            if ft.is_symlink() { continue; }
            if ft.is_dir() {
                walk(&path, needle, exclude, depth + 1, out);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".efi") && lower.contains(needle) && !lower.contains(exclude) {
                    if let Some(parent) = path.parent() {
                        if !out.iter().any(|known| known == parent) {
                            out.push(parent.to_path_buf());
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, needle, exclude, 0, &mut out);
    out
}

fn rel_of(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| dir.to_string_lossy().to_string())
}

/// 枚举本机所有候选根上的所有引导器实例。
/// Windows 上盘符根（`H:\`）没有文件名，用「磁盘X分区Y」代替 GUID 前缀作为别名。
#[cfg(target_os = "windows")]
fn volume_coords(root: &std::path::Path) -> Option<String> {
    let target = root.to_string_lossy().trim_end_matches('\\').to_ascii_lowercase();
    for partition in crate::channels::partitions::all() {
        let letter_match = partition
            .root()
            .map(|candidate| candidate.to_string_lossy().trim_end_matches('\\').to_ascii_lowercase() == target)
            .unwrap_or(false);
        let volume_match = partition
            .volume
            .as_deref()
            .map(|candidate| candidate.trim_end_matches('\\').to_ascii_lowercase() == target)
            .unwrap_or(false);
        if letter_match || volume_match {
            return Some(format!("磁盘{}分区{}", partition.disk, partition.partition));
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn volume_coords(_root: &std::path::Path) -> Option<String> {
    None
}

/// 兼容旧逻辑的「分区短标识」：取候选根路径最后一段（自动挂载时通常就是设备名，
/// 如 `nvme0n1p8`），否则退到卷坐标，再否则用分区标识（PARTUUID）前 12 位。
fn fallback_part_name(partition: &str, root_path: &str) -> String {
    std::path::Path::new(root_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| {
            volume_coords(std::path::Path::new(root_path))
                .unwrap_or_else(|| partition.chars().take(12).collect::<String>())
        })
}

/// 卷标重名时的区分后缀：取分区标识前 6 位（PARTUUID / 卷 GUID 三端同值、
/// 跨挂载点与重启都稳定）。分区标识不是 GUID（退回挂载根）时用挂载点末段。
fn part_suffix(partition: &str, root_path: &str) -> String {
    let head: String = partition
        .chars()
        .filter(char::is_ascii_hexdigit)
        .take(6)
        .collect::<String>()
        .to_ascii_lowercase();
    if head.len() == 6 {
        head
    } else {
        fallback_part_name(partition, root_path)
    }
}

/// 多实例标题里用来区分分区的「友好名」：**三端一律优先卷标**
/// （`MACOS-EFI` / `OC` / `EFI`），取不到才退回设备名 / 分区坐标 / 标识前缀。
/// 仅在同类型多实例时才会用到。
///
/// 为什么必须是卷标而不是「磁盘0分区5」这类坐标：卷标是唯一三端同值、且和
/// OpenCore 菜单、macOS `/Volumes` 目录名、Linux `blkid` 对得上的字符串 ——
/// 用户切到某块 ESP 的引导器时，标题能直接和他在引导菜单里看到的名字对应上。
/// 之前只有 Linux 走卷标，Windows 印成「磁盘0分区5」、macOS 靠挂载点末段碰巧对上，
/// 同一次升级三端名字不同（用户 2026-11-20 报）。
fn friendly_part_name(partition: &str, root_path: &str) -> String {
    // Linux：PARTUUID 直接问 blkid 最稳（ESP 未必挂在我们手里这个路径上）。
    #[cfg(target_os = "linux")]
    if let Some(label) = linux_volume_label(partition) {
        return label;
    }
    // 三端同源的原生卷标：Windows `GetVolumeInformationW`（认盘符也认
    // `\\?\Volume{guid}\`）、macOS 取 `/Volumes/<卷标>` 末段、Linux 走 findmnt+lsblk。
    // 与 OC 菜单里的卷标是同一个字符串。
    if let Some(label) = crate::channels::oc_menu::native_volume_label(Path::new(root_path)) {
        let label = label.trim();
        if !label.is_empty() {
            return label.to_string();
        }
    }
    fallback_part_name(partition, root_path)
}

/// Linux：`lsblk` 不暴露 FAT 的 `LABEL_FATBOOT`，所以按 PARTUUID 用 `blkid` 查卷标。
/// 先看 FAT 引导标签（`LABEL_FATBOOT`），再看通用 `LABEL`；都没有就返回 None。
#[cfg(target_os = "linux")]
fn linux_volume_label(partition: &str) -> Option<String> {
    let part = partition.trim();
    if part.is_empty() {
        return None;
    }
    for key in ["LABEL_FATBOOT", "LABEL"] {
        let output = std::process::Command::new("blkid")
            .args(["-t", &format!("PARTUUID={part}"), "-o", "value", "-s", key])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let label = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !label.is_empty() {
            return Some(label);
        }
    }
    None
}

pub(crate) fn scan_instances(app: &tauri::AppHandle) -> Vec<BootInstanceRef> {
    scan_instances_opts(app, false)
}

/// 深度扫描：枚举未挂载 ESP 并尝试只读/系统挂载（可能慢/弹授权）。
pub(crate) fn scan_instances_deep(app: &tauri::AppHandle) -> Vec<BootInstanceRef> {
    scan_instances_opts(app, true)
}

/// 分区的稳定键：优先 PARTUUID/GUID。Windows 无盘符卷不能走 `esp_probe`（它只认盘符），
/// 直接查 `partitions::all()` 的共享缓存（60s）取 GUID。
fn partition_key_for_root(root: &Path) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let target = root.to_string_lossy().trim_end_matches('\\').to_ascii_lowercase();
        for partition in crate::channels::partitions::all() {
            let letter_match = partition
                .root()
                .map(|candidate| candidate.to_string_lossy().trim_end_matches('\\').to_ascii_lowercase() == target)
                .unwrap_or(false);
            let volume_match = partition
                .volume
                .as_deref()
                .map(|candidate| candidate.trim_end_matches('\\').to_ascii_lowercase() == target)
                .unwrap_or(false);
            if (letter_match || volume_match) && !partition.guid.is_empty() {
                return Some(partition.guid.clone());
            }
        }
        return None;
    }
    #[cfg(not(target_os = "windows"))]
    {
        crate::esp_probe(root)
            .ok()
            .and_then(|info| info.partuuid.or(info.uuid))
    }
}

fn scan_instances_opts(app: &tauri::AppHandle, deep: bool) -> Vec<BootInstanceRef> {
    // 缓存命中（15s）；深度扫描强制刷新并回填缓存。
    if !deep {
        if let Ok(state) = app.state::<std::sync::Mutex<crate::state::AppState>>().lock() {
            if let Some((at, cached)) = &state.instances_cache {
                if at.elapsed() < std::time::Duration::from_secs(60) {
                    return cached.clone();
                }
            }
        }
    }
    let settings = crate::stored_settings(app);
    let mut roots = crate::channels::esp_candidates(app);
    // 默认全盘枚举：已挂载 + 未挂载 EFI 分区（diskutil list -plist / lsblk 全盘）。
    // `deep` 参数保留兼容；两种模式都做全盘，不再只扫已挂载卷。
    let _ = deep;
    for extra in extra_esp_roots() {
        if !roots.iter().any(|known| known == &extra) {
            roots.push(extra);
        }
    }
    let mut out: Vec<BootInstanceRef> = Vec::new();
    for root in roots {
        // 只扫真正的 ESP（根下有 EFI 目录）；避免对数据盘/网络卷递归整盘。
        if !root.join("EFI").is_dir() {
            continue;
        }
        // 稳定键：优先 PARTUUID/GUID（三端同值，跨挂载点不变）；探测失败才退回挂载根。
        let part = partition_key_for_root(&root)
            .unwrap_or_else(|| root.to_string_lossy().to_string());
        let short_part = fallback_part_name(&part, &root.to_string_lossy());
        for kind in KINDS {
            // 只在 ESP 的 EFI 子树里找（避免回收站/备份目录里的假 EFI）
            let efi = root.join("EFI");
            let dirs: Vec<PathBuf> = match kind {
                "refind" => {
                    let mut dirs = dirs_containing(&efi, "refind.conf");
                    dirs.extend(dirs_with_efi_excluding(&efi, "refind", "refindplus"));
                    dirs
                }
                "refindplus" => {
                    let mut dirs = dirs_containing(&efi, "refindplus.conf");
                    dirs.extend(dirs_with_efi(&efi, "refindplus"));
                    dirs
                }
                "grub" => dirs_containing(&efi, "grub.cfg"),
                "sdboot" => {
                    let mut dirs = dirs_containing(&root.join("loader"), "loader.conf");
                    dirs.extend(dirs_with_efi(&efi, "systemd-boot"));
                    dirs
                }
                "opencore" => dirs_with_efi(&efi, "opencore"),
                "clover" => dirs_with_efi(&efi, "cloverx64"),
                _ => Vec::new(),
            };
            for dir in dirs {
                let rel = rel_of(&root, &dir);
                let instance_id = format!("{}@{}/{}", kind, part, rel.to_lowercase());
                if out.iter().any(|known| known.instance_id == instance_id) {
                    continue;
                }
                let display = settings
                    .boot_instance_aliases
                    .get(&instance_id)
                    .cloned()
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| format!("{} · {}", kind_title(kind), short_part));
                let hidden = settings.boot_instance_hidden.iter().any(|id| id == &instance_id);
                out.push(BootInstanceRef {
                    kind: kind.to_string(),
                    instance_id,
                    title: display,
                    partition: part.clone(),
                    origin: "esp".to_string(),
                    install_dir: rel,
                    available: true,
                    hidden,
                    root_path: root.to_string_lossy().to_string(),
                });
            }
        }
    }
    out.sort_by(|a, b| (a.kind.clone(), a.partition.clone(), a.install_dir.clone())
        .cmp(&(b.kind.clone(), b.partition.clone(), b.install_dir.clone())));
    // 同一分区同类型多份安装（如 EFI/OC 与 EFI/OC2）：标题追加安装目录名以便区分
    let mut counts: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
    let mut kind_totals: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for instance in &out {
        *counts.entry((instance.kind.clone(), instance.partition.clone())).or_insert(0) += 1;
        *kind_totals.entry(instance.kind.clone()).or_insert(0) += 1;
    }
    // 同一个 kind 下卷标重名（两块 ESP 都叫 `EFI` / `SYSTEM` 很常见）：卷标本身
    // 区分不了，补一段分区标识尾巴，否则两个 tab 看起来一模一样、点了也不知道
    // 切到哪块。别名（用户手工起的）不参与，那是用户自己的选择。
    let mut label_of: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut label_seen: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
    for instance in &out {
        if settings.boot_instance_aliases.contains_key(&instance.instance_id) {
            continue;
        }
        let friendly = friendly_part_name(&instance.partition, &instance.root_path);
        *label_seen.entry((instance.kind.clone(), friendly.clone())).or_insert(0) += 1;
        label_of.insert(instance.instance_id.clone(), friendly);
    }
    for instance in &mut out {
        let aliased = settings.boot_instance_aliases.contains_key(&instance.instance_id);
        // 单实例类型：标题恢复 v0.2.12 的纯类型名（GRUB / rEFInd …）；
        // 只有同类型多实例时才保留「类型 · 分区名」区分。
        if kind_totals.get(&instance.kind).copied().unwrap_or(0) == 1 && !aliased {
            instance.title = kind_title(&instance.kind).to_string();
        } else if kind_totals.get(&instance.kind).copied().unwrap_or(0) > 1 && !aliased {
            // 多实例：用分区卷标区分（如 `MacOS-efi` / `OC`），三端同源；
            // 读不到卷标才退回设备名 / 分区坐标。
            let mut friendly = label_of.get(&instance.instance_id).cloned().unwrap_or_default();
            if friendly.trim().is_empty() {
                friendly = fallback_part_name(&instance.partition, &instance.root_path);
            }
            if label_seen
                .get(&(instance.kind.clone(), friendly.clone()))
                .copied()
                .unwrap_or(0)
                > 1
            {
                friendly = format!("{} ({})", friendly, part_suffix(&instance.partition, &instance.root_path));
            }
            instance.title = format!("{} · {}", kind_title(&instance.kind), friendly);
        }
        if counts.get(&(instance.kind.clone(), instance.partition.clone())).copied().unwrap_or(0) > 1 {
            let leaf = instance
                .install_dir
                .rsplit('/')
                .next()
                .unwrap_or("")
                .trim();
            if !leaf.is_empty() && !instance.title.ends_with(leaf) {
                instance.title = format!("{} / {}", instance.title, leaf);
            }
        }
    }
    if let Ok(mut state) = app.state::<std::sync::Mutex<crate::state::AppState>>().lock() {
        state.instances_cache = Some((std::time::Instant::now(), out.clone()));
    }
    out
}

/// Tauri 命令：所有引导器实例（平铺列表用）。
#[tauri::command]
pub(crate) fn boot_instances(app: AppHandle) -> Vec<BootInstanceRef> {
    scan_instances(&app)
}

/// GRUB 实例的真实条目：读该实例自己的 grub.cfg 解析成 BootOption。
pub(crate) fn grub_instance_options(instance: &BootInstanceRef) -> Vec<crate::remote_servers::BootOption> {
    let cfg = std::path::Path::new(&instance.root_path)
        .join(&instance.install_dir)
        .join("grub.cfg");
    let text = match fs::read_to_string(&cfg) {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };
    let entries = crate::grub::parse_grub_cfg(&text);
    let group_names: std::collections::HashMap<String, String> = crate::grub::parse_submenus(&text)
        .into_iter()
        .map(|group| (group.id, group.name))
        .collect();
    let mut group_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for entry in &entries {
        if let Some(submenu) = &entry.submenu {
            *group_counts.entry(submenu.clone()).or_insert(0) += 1;
        }
    }
    let mut options = Vec::new();
    let mut emitted_groups: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in entries {
        if let Some(submenu) = entry.submenu.clone() {
            // 一级（组头）必须保留：含子节点的这一行要按组头样式渲染
            if emitted_groups.insert(submenu.clone()) {
                let title = group_names.get(&submenu).cloned().unwrap_or_else(|| submenu.clone());
                let count = group_counts.get(&submenu).copied().unwrap_or(0);
                options.push(crate::remote_servers::BootOption {
                    value: String::new(),
                    label: title,
                    class: String::new(),
                    header: true,
                    fixed: true,
                    tag: format!("{count} 项"),
                    ..Default::default()
                });
            }
            options.push(crate::remote_servers::BootOption {
                value: entry.selector.clone(),
                label: entry.name.clone(),
                class: entry.class.clone().unwrap_or_else(|| "linux".to_string()),
                kernels: entry.kernels.clone(),
                child: true,
                tag: if entry.variants > 1 {
                    format!("{} 个变体", entry.variants)
                } else {
                    String::new()
                },
                ..Default::default()
            });
        } else {
            options.push(crate::remote_servers::BootOption {
                value: entry.selector.clone(),
                label: entry.name.clone(),
                class: entry.class.clone().unwrap_or_else(|| "linux".to_string()),
                kernels: entry.kernels.clone(),
                child: false,
                tag: if entry.variants > 1 {
                    format!("{} 个变体", entry.variants)
                } else {
                    String::new()
                },
                ..Default::default()
            });
        }
    }
    options
}

/// 切换某个 GRUB 实例：只写它自己安装目录下的 grubenv（多实例互不影响）。
fn switch_refind_instance(app: &tauri::AppHandle, instance: &BootInstanceRef, title: &str) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("没有选择引导项".to_string());
    }
    let vars_dir = std::path::Path::new(&instance.root_path)
        .join(&instance.install_dir)
        .join("vars");
    if !vars_dir.is_dir() {
        return Err(format!("该实例的 vars 目录不存在：{}", vars_dir.display()));
    }
    let sim = if instance.kind == "refind" {
        crate::channels::refind_menu::preview(&vars_dir, &[], None)
    } else {
        crate::channels::refind_menu::preview_with_default_use_nvram(&vars_dir, &[], None, false)
    };
    if sim.var_mode == "nvram" {
        return Err(format!("{} 使用 NVRAM 保存 PreviousBoot，暂不支持按实例切换", instance.title));
    }
    let hit = sim
        .entries
        .iter()
        .find(|entry| !entry.hidden && entry.title.trim() == title)
        .ok_or_else(|| format!("当前实例菜单里没有这一项：{title}"))?;
    let bytes = crate::channels::refind_menu::previous_boot_bytes(&hit.title);
    let target = vars_dir.join("PreviousBoot");
    // 该实例的 ESP 多半是被我们**只读**挂到 anyboot-esp 上的（启动扫描用），
    // 直接写会 EROFS。与扫描 / 单实例切换同一套：临时转可写、写完立刻转回只读。
    let readonly_mount = crate::mount::readonly_mount_of(&vars_dir);
    crate::mount::with_mount(readonly_mount.as_ref(), || {
        std::fs::write(&target, &bytes)
            .map_err(|error| format!("写入 PreviousBoot 失败: {error}"))
    })?;
    Ok(format!("下次启动：{}", hit.title.trim()))
}

pub(crate) fn switch_instance(app: &tauri::AppHandle, instance_id: &str, target: &str) -> Result<String, String> {
    let selector = target.trim();
    if selector.is_empty() {
        return Err("没有选择引导项".to_string());
    }
    let instances = scan_instances(app);
    let instance = instances
        .into_iter()
        .find(|item| item.instance_id == instance_id)
        .ok_or_else(|| format!("找不到引导器实例：{instance_id}"))?;
    match instance.kind.as_str() {
        "refind" | "refindplus" => return switch_refind_instance(app, &instance, target),
        "sdboot" | "opencore" | "clover" => {
            return crate::channels::apply_channel_spec_at(std::path::Path::new(&instance.root_path), target)
        }
        "grub" => {
            // **读取方式决定写哪份 grubenv**（与回显同源，2026-11-21 用户实测：
            // 自动模式下点了没反应 —— 这里写的是 ESP 那份 EFI/grubenv，而真实
            // grubenv 在 Linux 分区的 /boot/grub，两份不同步）。
            if !crate::stored_settings(app).grub_mode.is_manual() {
                let view = crate::channels::linux_grub::switch_blocking(
                    app.clone(),
                    selector.to_string(),
                );
                if let Some(error) = view.error.clone() {
                    return Err(error);
                }
                let saved = view.saved_entry.clone().unwrap_or_default();
                let label = view
                    .entries
                    .iter()
                    .find(|entry| entry.selector == saved)
                    .map(|entry| entry.name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(saved);
                return Ok(format!("下次启动：{label}"));
            }
        }
        other => return Err(format!("暂不支持该类型的实例切换：{other}")),
    }
    let dir = std::path::Path::new(&instance.root_path).join(&instance.install_dir);
    let mut env_path = dir.join("grubenv");
    if !env_path.is_file() {
        let shared = std::path::Path::new(&instance.root_path).join("EFI/grubenv");
        if shared.is_file() {
            env_path = shared;
        }
    }
    crate::grub_esp::grub_write_env(&env_path, selector)?;
    Ok(format!("下次启动：{selector}"))
}

/// 实例切换的统一入口：写入 + 局部更新实例缓存 + 发布 UI（让三端都刷新/出提示）。
pub(crate) fn switch_instance_and_update(
    app: &tauri::AppHandle,
    instance_id: &str,
    target: &str,
) -> Result<String, String> {
    let message = switch_instance(app, instance_id, target)?;
    // 先回执成功（切换命令不再同步跑全量 channels 刷新——那是几秒级延迟的来源）。
    // 复用实例缓存：旧代码切换只读当前状态即可，不能每次切换都全盘扫一遍；
    // 后台刷新/下一次 60s 缓存过期时自然会更新 current。
    if let Ok(mut state) = app.state::<std::sync::Mutex<crate::state::AppState>>().lock() {
        state.message = if message.starts_with("下次启动：") {
            message.clone()
        } else {
            String::new()
        };
        state.error = None;
    }
    crate::ui_state::publish_ui(app);
    Ok(message)
}

/// 额外实例 → BootGroup（真实 options）。失败返回 None，由调用方给只读占位。
pub(crate) fn extra_instance_group(app: &tauri::AppHandle, instance: &BootInstanceRef) -> Option<crate::remote_servers::BootGroup> {
    let _span = match instance.kind.as_str() {
        "grub" => crate::perf::span("inst.grub"),
        "refind" => crate::perf::span("inst.refind"),
        "refindplus" => crate::perf::span("inst.refindplus"),
        "opencore" => crate::perf::span("inst.opencore"),
        "sdboot" => crate::perf::span("inst.sdboot"),
        _ => crate::perf::span("inst.clover"),
    };
    let root = std::path::Path::new(&instance.root_path);
    match instance.kind.as_str() {
        "grub" => {
            // **读取方式决定数据源**（与系统引导页 / 电脑列表本机行 / 托盘同一口径）：
            // · 自动（默认）：GRUB 列表看 Linux 分区里那份 grub.cfg + grubenv
            //   （`remote_grub_json` 已经把 linuxGrub overlay 上去）；
            // · 手动：才解析 ESP 上那份副本（现状行为）。
            // 以前一律读「实例目录下的 grubenv」（= ESP 那份），与自动模式的真值
            // 不同步 → 电脑列表/电脑页的 GRUB 回显显示成另一颗内核（实测：真值
            // saved_entry=linux-zen，界面却显示 mainline）。2026-11-21 用户实测。
            let settings = crate::stored_settings(app);
            if !settings.grub_mode.is_manual() {
                let json = crate::remote_routing::remote_grub_json(app);
                let mut group = crate::remote_servers::grub_from_json(
                    &json,
                    &settings.grub_list_format,
                );
                group.kind = instance.kind.clone();
                group.title = instance.title.clone();
                group.available = instance.available || group.available;
                Some(group)
            } else {
                Some(crate::remote_servers::BootGroup {
                    kind: instance.kind.clone(),
                    title: instance.title.clone(),
                    available: instance.available,
                    current: grub_instance_current(instance),
                    note: String::new(),
                    options: grub_instance_options(instance),
                    ..Default::default()
                })
            }
        }
        "refind" | "refindplus" => {
            let vars = root.join(&instance.install_dir).join("vars");
            // 复用 T7 的 linux_probe_cache（内核/分区清单），不重新全盘扫 ext4
            let (parts, note) = crate::channels::refind_menu::linux_grub_parts_from_cache(app);
            let sim = if instance.kind == "refind" {
                crate::channels::refind_menu::preview(&vars, &parts, note.clone())
            } else {
                crate::channels::refind_menu::preview_with_default_use_nvram(&vars, &parts, note, false)
            };
            let json = serde_json::to_value(sim).ok()?;
            let mut group = if instance.kind == "refind" {
                crate::remote_servers::refind_from_sim(&json)
            } else {
                crate::remote_servers::refindplus_from_sim(&json)
            };
            group.kind = instance.kind.clone();
            group.title = instance.title.clone();
            Some(group)
        }
        "sdboot" | "opencore" | "clover" => {
            let view = match instance.kind.as_str() {
                "sdboot" => crate::channels::sdboot::view(root),
                "opencore" => crate::channels::opencore::view(root),
                _ => crate::channels::clover::view(root),
            };
            let mut group = crate::remote_servers::channel_from_view(&view);
            group.kind = instance.kind.clone();
            group.title = instance.title.clone();
            Some(group)
        }
        _ => None,
    }
}

fn grub_instance_current(instance: &BootInstanceRef) -> String {
    let dir = std::path::Path::new(&instance.root_path).join(&instance.install_dir);
    let mut env = dir.join("grubenv");
    if !env.is_file() {
        let shared = std::path::Path::new(&instance.root_path).join("EFI/grubenv");
        if shared.is_file() {
            env = shared;
        }
    }
    let Ok(bytes) = fs::read(env) else { return String::new() };
    crate::grub::env_value(&bytes, "next_entry")
        .filter(|value| !value.trim().is_empty())
        .or_else(|| crate::grub::env_value(&bytes, "saved_entry"))
        .unwrap_or_default()
}

/// 补充枚举「没挂载」的 ESP 根：多硬盘多 ESP 时，仅扫已挂载卷会漏实例。
#[cfg(target_os = "macos")]
fn extra_esp_roots() -> Vec<PathBuf> {
    // diskutil list -plist：一次拿到所有磁盘/分区（含未挂载 EFI），比 diskutil info -all 快得多。
    let Ok(list) = std::process::Command::new("diskutil").args(["list", "-plist"]).output() else {
        return Vec::new();
    };
    if !list.status.success() {
        return Vec::new();
    }
    let xml = String::from_utf8_lossy(&list.stdout).to_string();
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut mount_text = std::process::Command::new("mount")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
        .unwrap_or_default();

    fn int_of(segment: &str, key: &str) -> Option<u64> {
        let needle = format!("<key>{key}</key>");
        let index = segment.find(&needle)? + needle.len();
        let rest = &segment[index..];
        let start = rest.find("<integer>")? + "<integer>".len();
        let end = rest[start..].find("</integer>")? + start;
        rest[start..end].trim().parse().ok()
    }
    fn value_of(segment: &str, key: &str) -> Option<String> {
        let needle = format!("<key>{key}</key>");
        let index = segment.find(&needle)? + needle.len();
        let rest = &segment[index..];
        let start = rest.find("<string>")? + "<string>".len();
        let end = rest[start..].find("</string>")? + start;
        Some(rest[start..end].to_string())
    }
    fn mountpoint_of(mount_text: &str, device: &str) -> Option<String> {
        let target = format!("/dev/{device}");
        for line in mount_text.lines() {
            if line.starts_with(&target) {
                if let Some(rest) = line.split(" on ").nth(1) {
                    let mount = rest.split(" (").next().unwrap_or("").trim();
                    if !mount.is_empty() {
                        return Some(mount.to_string());
                    }
                }
            }
        }
        None
    }

    for segment in xml.split("<dict>").skip(1) {
        let segment = segment.split("</dict>").next().unwrap_or("");
        let content = value_of(segment, "Content").unwrap_or_default();
        let size = int_of(segment, "Size").unwrap_or(0);
        // ESP 在 diskutil 里可能被标成 Microsoft Basic Data（尤其黑苹果），不能只看 Content=EFI。
        // 取「Content=EFI」或「≤4GB 的 Microsoft Basic Data」作为候选，挂载后必须存在 EFI 目录才算。
        let candidate = content.to_ascii_uppercase().contains("EFI")
            || (content.eq_ignore_ascii_case("Microsoft Basic Data") && size > 0 && size <= 4 * 1024 * 1024 * 1024);
        if !candidate {
            continue;
        }
        let Some(device) = value_of(segment, "DeviceIdentifier") else {
            continue;
        };
        if let Some(mount) = mountpoint_of(&mount_text, &device) {
            if PathBuf::from(&mount).join("EFI").is_dir() {
                roots.push(PathBuf::from(mount));
            }
            continue;
        }
        // 未挂载：直接 diskutil mount <device>，再查挂载点（失败就跳过，不阻塞整体）。
        let mounted = std::process::Command::new("diskutil")
            .args(["mount", &device])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        if mounted {
            if let Ok(out) = std::process::Command::new("mount").output() {
                mount_text = String::from_utf8_lossy(&out.stdout).to_string();
            }
            if let Some(mount) = mountpoint_of(&mount_text, &device) {
                if PathBuf::from(&mount).join("EFI").is_dir() {
                    roots.push(PathBuf::from(mount));
                } else {
                    // 挂上了但不是 ESP（普通 FAT 数据盘）→ 立刻卸载，避免留下挂载
                    let _ = std::process::Command::new("diskutil")
                        .args(["unmount", &device])
                        .output();
                }
            }
        }
    }
    roots
}

#[cfg(target_os = "linux")]
fn extra_esp_roots() -> Vec<PathBuf> {
    // 复用单实例时代那条已经验证过的「未挂载 ESP 枚举 + 只读挂载」链路：
    // `scan::unmounted_esp_partitions()` 现在也认「≤4GB Microsoft Basic Data」的
    // 黑苹果 ESP（与 Windows/macOS 端 is_esp_candidate 对齐），
    // `channels::linux_mount_esp_ro()` 走免密助手静默挂载（allow_pkexec=false，
    // 绝不弹授权框）。挂不上的（没装助手/非 FAT）静默跳过，不影响其它实例。
    let mut roots = Vec::new();
    for part in crate::scan::unmounted_esp_partitions() {
        if let Ok(mountpoint) = crate::channels::linux_mount_esp_ro(&part.device, false) {
            roots.push(mountpoint);
        }
    }
    roots
}

#[cfg(target_os = "windows")]
fn extra_esp_roots() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PARTUUID / 卷 GUID 前 6 位做重名后缀；非 GUID 退回挂载点名。
    #[test]
    fn part_suffix_prefers_guid_head() {
        assert_eq!(
            part_suffix("f1d02f84-6406-4b8f-8430-f8ab38d41b67", "/Volumes/MacOS-efi-t"),
            "f1d02f"
        );
        assert_eq!(part_suffix("42074C1D-E063-431C-AAC4-4FFBE441298E", "R://"), "42074c");
        // 不是 GUID（Linux 拿不到 PARTUUID 时会把挂载根当标识）→ 用挂载点末段
        assert_eq!(part_suffix("/boot/efi", "/boot/efi"), "efi");
    }

    /// 真机探针：`cargo test --lib -- --ignored real_instance_labels --nocapture`
    ///
    /// 打出每块 ESP 的候选根与读到的卷标 —— 多实例标题「三端优先卷标」就靠这一列
    /// 对齐（Windows 上必须看到 `MacOS-efi` 这类卷标，不是 `磁盘0分区5`）。
    #[test]
    #[ignore = "真机：读本机真实 ESP 卷标"]
    fn real_instance_labels() {
        #[cfg(target_os = "windows")]
        for esp in crate::channels::win_esp::list() {
            let root = esp.access_root();
            let label = root
                .as_deref()
                .and_then(crate::channels::oc_menu::native_volume_label);
            println!(
                "磁盘{}分区{:<3} root={:<32} label={:?}",
                esp.disk,
                esp.partition,
                root.map(|path| path.display().to_string()).unwrap_or_else(|| "-".into()),
                label
            );
        }
        #[cfg(target_os = "macos")]
        for entry in std::fs::read_dir("/Volumes").into_iter().flatten().flatten() {
            let path = entry.path();
            println!("{:<40} label={:?}", path.display(), crate::channels::oc_menu::native_volume_label(&path));
        }
        #[cfg(target_os = "linux")]
        for root in ["/efi", "/boot/efi"] {
            let path = Path::new(root);
            if path.is_dir() {
                println!("{:<40} label={:?}", root, crate::channels::oc_menu::native_volume_label(path));
            }
        }
    }
}
