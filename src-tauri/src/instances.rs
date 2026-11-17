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

fn rel_of(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| dir.to_string_lossy().to_string())
}

/// 枚举本机所有候选根上的所有引导器实例。
pub(crate) fn scan_instances(app: &tauri::AppHandle) -> Vec<BootInstanceRef> {
    scan_instances_opts(app, false)
}

/// 深度扫描：枚举未挂载 ESP 并尝试只读/系统挂载（可能慢/弹授权）。
pub(crate) fn scan_instances_deep(app: &tauri::AppHandle) -> Vec<BootInstanceRef> {
    scan_instances_opts(app, true)
}

fn scan_instances_opts(app: &tauri::AppHandle, deep: bool) -> Vec<BootInstanceRef> {
    // 缓存命中（15s）；深度扫描强制刷新并回填缓存。
    if !deep {
        if let Ok(state) = app.state::<std::sync::Mutex<crate::state::AppState>>().lock() {
            if let Some((at, cached)) = &state.instances_cache {
                if at.elapsed() < std::time::Duration::from_secs(15) {
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
        // 稳定键：优先 PARTUUID（三端同值，跨挂载点不变）；探测失败才退回挂载根。
        let part = crate::esp_probe(&root)
            .ok()
            .and_then(|info| info.partuuid.or(info.uuid))
            .unwrap_or_else(|| root.to_string_lossy().to_string());
        let short_part = root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.to_string())
            .unwrap_or_else(|| part.chars().take(12).collect::<String>());
        for kind in KINDS {
            // 只在 ESP 的 EFI 子树里找（避免回收站/备份目录里的假 EFI）
            let efi = root.join("EFI");
            let dirs: Vec<PathBuf> = match kind {
                "refind" => {
                    let mut dirs = dirs_containing(&efi, "refind.conf");
                    dirs.extend(
                        dirs_with_efi(&efi, "refind")
                            .into_iter()
                            .filter(|dir| !dir.to_string_lossy().to_ascii_lowercase().contains("refindplus")),
                    );
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
    crate::grub::parse_grub_cfg(&text)
        .into_iter()
        .map(|entry| crate::remote_servers::BootOption {
            value: entry.selector.clone(),
            label: entry.name.clone(),
            class: entry.class.clone().unwrap_or_else(|| "linux".to_string()),
            child: entry.submenu.is_some(),
            tag: if entry.variants > 1 {
                format!("{} 个变体", entry.variants)
            } else {
                String::new()
            },
            ..Default::default()
        })
        .collect()
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
    std::fs::write(vars_dir.join("PreviousBoot"), bytes)
        .map_err(|error| format!("写入 PreviousBoot 失败: {error}"))?;
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
        "grub" => {}
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

/// 额外实例 → BootGroup（真实 options）。失败返回 None，由调用方给只读占位。
pub(crate) fn extra_instance_group(instance: &BootInstanceRef) -> Option<crate::remote_servers::BootGroup> {
    let root = std::path::Path::new(&instance.root_path);
    match instance.kind.as_str() {
        "grub" => Some(crate::remote_servers::BootGroup {
            kind: instance.kind.clone(),
            title: instance.title.clone(),
            available: instance.available,
            current: grub_instance_current(instance),
            note: String::new(),
            options: grub_instance_options(instance),
            ..Default::default()
        }),
        "refind" | "refindplus" => {
            let vars = root.join(&instance.install_dir).join("vars");
            let sim = if instance.kind == "refind" {
                crate::channels::refind_menu::preview(&vars, &[], None)
            } else {
                crate::channels::refind_menu::preview_with_default_use_nvram(&vars, &[], None, false)
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
        if !content.to_ascii_uppercase().contains("EFI") {
            continue;
        }
        let Some(device) = value_of(segment, "DeviceIdentifier") else {
            continue;
        };
        if let Some(mount) = mountpoint_of(&mount_text, &device) {
            roots.push(PathBuf::from(mount));
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
                roots.push(PathBuf::from(mount));
            }
        }
    }
    roots
}

#[cfg(target_os = "linux")]
fn extra_esp_roots() -> Vec<PathBuf> {
    let output = match std::process::Command::new("lsblk")
        .args(["--json", "--output", "PATH,FSTYPE,PARTTYPE,PARTUUID,MOUNTPOINTS"])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    let value: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let mut roots = Vec::new();
    fn walk(node: &serde_json::Value, roots: &mut Vec<PathBuf>) {
        if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
            for child in children {
                walk(child, roots);
            }
        }
        let fstype = node.get("fstype").and_then(|v| v.as_str()).unwrap_or("").to_ascii_lowercase();
        let parttype = node.get("parttype").and_then(|v| v.as_str()).unwrap_or("").to_ascii_uppercase();
        // 只认 EFI System Partition（GPT EF00）；避免把普通 FAT 数据盘当 ESP。
        if parttype != "EF00" || !(fstype.contains("fat") || fstype == "vfat") {
            return;
        }
        let uuid = node.get("partuuid").and_then(|v| v.as_str()).unwrap_or("");
        if uuid.is_empty() {
            return;
        }
        if let Some(mounts) = node.get("mountpoints").and_then(|v| v.as_array()) {
            if let Some(mount) = mounts.iter().filter_map(|v| v.as_str()).find(|m| !m.is_empty()) {
                roots.push(PathBuf::from(mount));
                return;
            }
        }
        // 未挂载的 ESP 不在默认扫描里自动挂载（会阻塞/弹授权）。
        let _ = uuid;
    }
    if let Some(devices) = value.get("blockdevices").and_then(|v| v.as_array()) {
        for device in devices {
            walk(device, &mut roots);
        }
    }
    roots
}

#[cfg(target_os = "windows")]
fn extra_esp_roots() -> Vec<PathBuf> {
    Vec::new()
}
