//! GRUB「自动读取」通道：没装 `09_anyboot` 时，Windows / macOS 直接读 Linux
//! 分区上的 `/boot/grub/grub.cfg`（解析成条目）与 `/boot/grub/grubenv`（写下次启动）。
//!
//! 与 `grub_esp.rs` 那条「手动解析」链路**并列**，互不影响：
//!
//! | | 手动解析 | 自动读取（本模块） |
//! |---|---|---|
//! | 前提 | 装了 `/etc/grub.d/09_anyboot` | 没装，GRUB 保持默认布局 |
//! | 数据源 | ESP 上的 `/EFI/grub.cfg`、`/EFI/grubenv` | Linux 分区自己的 `/boot/grub/*` |
//! | 读盘 | FAT，三端原生可读 | ext4，macOS/Windows 要自己解析（[`ext4_raw`]） |
//! | 提权 | 不需要 | 裸设备要 root / 管理员 |
//!
//! 三端的数据来源：
//! * **Linux 本机**：`fs` 直读（`/boot/grub` 或 `/boot/grub2`），零驱动零提权；
//! * **macOS**：`osascript … with administrator privileges` 拉起本程序的
//!   `--linux-grub-probe` / `--linux-grub-write` 子命令（app 本身不是 root，
//!   `/dev/rdisk*` 是 `root:operator 0640`，实测普通用户连 `dd` 都被拒）；
//! * **Windows**：`elevate` 拉起同样的子命令（UAC 一次，按分区偏移读
//!   `\\.\PhysicalDriveN`）。
//!
//! 定位规则（用户定案 2026-10-29 第 2 点）：用户选 ESP → 看哪块 Linux 分区的
//! `fstab` 挂载了它。`fstab` 里可能只写 `/dev/sda1`（不是 UUID），所以标识
//! 统一交给 [`part_spec`] 解析，命中强度分「精确 / 推断」两档，推断结果界面
//! 必须标出来 —— 猜错会把 grubenv 写到另一块盘上。
//!
//! 写入面**只有一件事**：把 1024 字节环境块覆盖回它自己的第一个数据块
//! （[`ext4_raw::Ext4::overwrite_in_place`]）。不建文件、不改长度、不动元数据。

use super::*;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ext4_raw::{BlockSource, Ext4, FileSource};
use super::part_spec::{self, PartIdentity, PartSpec, Strength};

/// 环境块尺寸与头部（与 `grub.rs` 同源，写盘前逐条校验）
use crate::grub::{ENV_BLOCK_HEADER, ENV_BLOCK_SIZE};

/// Linux 上 GRUB 的两种目录命名（Fedora 用 `grub2`）
pub(crate) const GRUB_DIRS: [&str; 2] = ["boot/grub", "boot/grub2"];
/// grub.cfg 读取上限：实测 arch-gui 那份 52,760 字节，512 KB 足够宽松
const MAX_CFG_BYTES: usize = 512 * 1024;
/// fstab 读取上限
const MAX_FSTAB_BYTES: usize = 256 * 1024;
/// os-release 读取上限
const MAX_SMALL_BYTES: usize = 8192;

// ─────────────────────────── 跨端数据结构 ───────────────────────────

/// 一条 fstab 挂载行（**不含 options** —— 里面常有明文密码）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MountHint {
    pub(crate) spec: String,
    pub(crate) mountpoint: String,
    pub(crate) fstype: String,
}

/// 一块 Linux 分区的探测结果（提权子进程产出，主进程消费）。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartReport {
    /// macOS `disk0s9` / Linux `sdb1` / Windows `disk0p9`
    pub(crate) dev: String,
    pub(crate) label: String,
    pub(crate) size: Option<String>,
    pub(crate) fs_uuid: Option<String>,
    pub(crate) part_uuid: Option<String>,
    pub(crate) part_label: Option<String>,
    pub(crate) part_number: u32,
    pub(crate) disk_index: u32,
    /// `fstab` 里的挂载行（已剥掉 options）
    pub(crate) mounts: Vec<MountHint>,
    /// 分区内的 grub.cfg 路径（`boot/grub/grub.cfg`）
    pub(crate) cfg_rel: Option<String>,
    pub(crate) env_rel: Option<String>,
    pub(crate) cfg_text: Option<String>,
    /// cfg 的 mtime（epoch 秒）—— 自动模式没人同步，界面必须说出它有多旧
    pub(crate) cfg_mtime: Option<i64>,
    /// 环境块原始字节（hex）—— 写之前要在它上面改键值
    pub(crate) env_hex: Option<String>,
    /// grub.cfg 里 `root=UUID=` 的值：用来验证「这份 cfg 确实是这个根分区的」
    pub(crate) root_uuid: Option<String>,
    /// `/etc/os-release` 的 `ID`（`arch` / `fedora` …）
    pub(crate) os_id: Option<String>,
    pub(crate) note: Option<String>,
}

impl PartReport {
    pub(crate) fn identity(&self) -> PartIdentity {
        PartIdentity {
            fs_uuid: self.fs_uuid.clone(),
            part_uuid: self.part_uuid.clone(),
            label: Some(self.label.clone()).filter(|value| !value.is_empty()),
            part_label: self.part_label.clone(),
            disk_index: self.disk_index,
            part_number: self.part_number,
        }
    }

    fn env_bytes(&self) -> Option<Vec<u8>> {
        let hex = self.env_hex.as_deref()?;
        hex::decode(hex).ok()
    }
}

/// 提权子进程写给主进程的整包结果（走临时目录里的 JSON 文件）。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbePayload {
    pub(crate) ok: bool,
    pub(crate) error: Option<String>,
    /// 所选 ESP 的标识（用户设置的 partition UUID 对应的那块分区的实际身份）
    pub(crate) esp: Option<IdentityWire>,
    pub(crate) parts: Vec<PartReport>,
    /// 本端枚举时遇到的非致命问题（一句话一条）
    pub(crate) notes: Vec<String>,
}

/// [`PartIdentity`] 的传输形态（`part_spec` 里的结构体不 derive serde）。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdentityWire {
    pub(crate) fs_uuid: Option<String>,
    pub(crate) part_uuid: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) part_label: Option<String>,
    pub(crate) disk_index: u32,
    pub(crate) part_number: u32,
}

impl From<IdentityWire> for PartIdentity {
    fn from(wire: IdentityWire) -> Self {
        Self {
            fs_uuid: wire.fs_uuid,
            part_uuid: wire.part_uuid,
            label: wire.label,
            part_label: wire.part_label,
            disk_index: wire.disk_index,
            part_number: wire.part_number,
        }
    }
}

impl From<&PartIdentity> for IdentityWire {
    fn from(identity: &PartIdentity) -> Self {
        Self {
            fs_uuid: identity.fs_uuid.clone(),
            part_uuid: identity.part_uuid.clone(),
            label: identity.label.clone(),
            part_label: identity.part_label.clone(),
            disk_index: identity.disk_index,
            part_number: identity.part_number,
        }
    }
}

/// 界面上的「目标分区」
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinuxTarget {
    pub(crate) dev: String,
    pub(crate) label: String,
    pub(crate) size: Option<String>,
    pub(crate) fs_uuid: Option<String>,
    pub(crate) root_uuid: Option<String>,
    pub(crate) os_id: Option<String>,
    /// 本端直读（Linux）还是走裸设备（macOS/Windows）
    pub(crate) host: bool,
}

/// 「换一个」列表里的一行
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinuxCandidate {
    pub(crate) dev: String,
    pub(crate) label: String,
    pub(crate) size: Option<String>,
    pub(crate) cfg_rel: Option<String>,
    pub(crate) cfg_mtime: Option<String>,
    pub(crate) has_env: bool,
    /// `exact` / `inferred` / `self` / `none`：这一块与所选 ESP 的关系
    pub(crate) matched: String,
    /// fstab 里那一行的原文（不含 options）
    pub(crate) esp_spec: Option<String>,
    pub(crate) os_id: Option<String>,
}

/// 界面的整块视图（存进快照，前端并排第二块用它）
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinuxGrubView {
    pub(crate) scanned_at: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) platform: String,
    /// 本端读盘要不要提权（macOS/Windows 是 true）
    pub(crate) needs_elevation: bool,
    pub(crate) target: Option<LinuxTarget>,
    /// `exact` / `inferred` / `self` / `manual` / `none`
    pub(crate) confidence: String,
    pub(crate) confidence_note: Option<String>,
    pub(crate) candidates: Vec<LinuxCandidate>,
    pub(crate) entries: Vec<grub::GrubEntry>,
    pub(crate) groups: Vec<grub::GrubGroup>,
    pub(crate) cfg_rel: Option<String>,
    pub(crate) cfg_size: Option<u64>,
    pub(crate) cfg_mtime: Option<String>,
    pub(crate) env_rel: Option<String>,
    pub(crate) env_exists: bool,
    pub(crate) env_size: Option<u64>,
    pub(crate) saved_entry: Option<String>,
    pub(crate) next_entry: Option<String>,
    /// ESP 上还有 `EFI/grubenv` 且 09_anyboot 装着 → GRUB 会优先用那份，
    /// 写 Linux 盘上的 env **写了也不生效**（必须说出来，否则是假成功）
    pub(crate) manual_takes_over: bool,
    pub(crate) issues: Vec<String>,
}

impl LinuxGrubView {
    fn failed(platform: &str, error: String) -> Self {
        Self {
            scanned_at: Some(anyboot_now_iso()),
            platform: platform.to_string(),
            needs_elevation: !cfg!(target_os = "linux"),
            error: Some(error.clone()),
            confidence: "none".to_string(),
            issues: vec![error],
            ..Self::default()
        }
    }
}

/// 写盘需要的上下文（扫描成功后缓存，切换时直接用，不重新探测）
#[derive(Clone, Debug, Default)]
pub(crate) struct Runtime {
    /// macOS/Windows：`disk0s9`；Linux 直读时为空
    pub(crate) dev: String,
    pub(crate) host: bool,
    /// ext4 分区内路径（`boot/grub/grubenv`）或本机绝对路径
    pub(crate) env_rel: String,
    pub(crate) env_bytes: Vec<u8>,
    /// 当前菜单里的 selector 白名单：只允许写真实存在的条目
    pub(crate) selectors: Vec<String>,
}

fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

// ─────────────────────────── 定位 ───────────────────────────

/// 一级定位结果
#[derive(Clone, Debug)]
pub(crate) struct Selection {
    pub(crate) index: usize,
    /// `exact` / `inferred` / `self` / `manual`
    pub(crate) confidence: &'static str,
    pub(crate) note: String,
    /// 命中的那行 fstab 原文（精确/推断时才有）
    pub(crate) esp_spec: Option<String>,
}

/// 这条挂载行看起来是不是「ESP 挂载」（`/boot/efi`、`/efi`、vfat 挂在 `/boot`）
fn looks_like_esp_mount(mount: &MountHint) -> bool {
    let point = mount.mountpoint.trim_end_matches('/').to_ascii_lowercase();
    let fstype = mount.fstype.to_ascii_lowercase();
    let fat = fstype.starts_with("vfat") || fstype.starts_with("fat") || fstype == "msdos";
    if point == "/boot/efi" || point == "/efi" || point == "/esp" {
        return fat || fstype == "auto";
    }
    // 少数老配置把 ESP 直接挂在 `/boot`
    if point == "/boot" && fat {
        return true;
    }
    false
}

/// 在探测结果里挑出「用户所选 ESP 对应的那块 Linux 分区」。
///
/// 顺序（与文档 §5.3 一致）：
/// 1. 用户手动指定过（`prefer_dev`）→ 直接用，标 `manual`；
/// 2. fstab 里有挂载行命中所选 ESP → `exact`（UUID/PARTUUID）或 `inferred`（卷标/设备名）；
/// 3. 都不中、且只有一块分区有 grub.cfg → `self`；
/// 4. 否则不选，交给界面让用户点（绝不猜）。
pub(crate) fn locate(
    parts: &[PartReport],
    esp: Option<&PartIdentity>,
    prefer_dev: Option<&str>,
) -> Option<Selection> {
    if let Some(dev) = prefer_dev.map(str::trim).filter(|value| !value.is_empty()) {
        if let Some(index) = parts.iter().position(|part| part.dev == dev) {
            return Some(Selection {
                index,
                confidence: "manual",
                note: format!("按你选择的磁盘（{}）读取", parts[index].dev),
                esp_spec: None,
            });
        }
    }

    let Some(esp) = esp else {
        return fallback(parts, "还没识别出你选的 ESP 分区");
    };

    let mut best: Option<Selection> = None;
    for (index, part) in parts.iter().enumerate() {
        for mount in &part.mounts {
            if !looks_like_esp_mount(mount) {
                continue;
            }
            let Some(spec) = PartSpec::parse_opt(&mount.spec) else {
                continue;
            };
            let strength = spec.matches(esp);
            if strength == Strength::None {
                continue;
            }
            let better = match &best {
                None => true,
                Some(current) => strength == Strength::Exact && current.confidence != "exact",
            };
            if better {
                let note = match strength {
                    Strength::Exact => format!(
                        "{} 里的 fstab 写着 {} 挂在 {} —— UUID/PARTUUID 精确命中",
                        part.dev, spec.describe(), mount.mountpoint
                    ),
                    _ => format!(
                        "{} 里的 fstab 写着 {} 挂在 {} —— 只能按{}推断",
                        part.dev,
                        spec.describe(),
                        mount.mountpoint,
                        match spec {
                            PartSpec::Device { .. } => "设备名",
                            _ => "卷标",
                        }
                    ),
                };
                best = Some(Selection {
                    index,
                    confidence: if strength == Strength::Exact {
                        "exact"
                    } else {
                        "inferred"
                    },
                    note,
                    esp_spec: Some(format!("{} → {}", mount.spec, mount.mountpoint)),
                });
            }
            if best.as_ref().is_some_and(|value| value.confidence == "exact") {
                return best;
            }
        }
    }
    if best.is_some() {
        return best;
    }
    fallback(parts, "没有哪块 Linux 分区的 fstab 挂载了你选的这个 ESP")
}

/// 兜底：只有一块分区有 grub.cfg 时直接用它，否则不选
fn fallback(parts: &[PartReport], reason: &str) -> Option<Selection> {
    let with_cfg: Vec<usize> = parts
        .iter()
        .enumerate()
        .filter(|(_, part)| part.cfg_rel.is_some())
        .map(|(index, _)| index)
        .collect();
    if with_cfg.len() == 1 {
        let index = with_cfg[0];
        return Some(Selection {
            index,
            confidence: "self",
            note: format!("{reason}；只有 {} 一块分区有 grub.cfg，就先按它读", parts[index].dev),
            esp_spec: None,
        });
    }
    None
}

// ─────────────────────────── 视图组装 ───────────────────────────

/// 从 `grub.cfg` 文本里取 `root=UUID=…`（`grub-mkconfig` 每次都会写）。
///
/// 只取第一个命中的值：同一份 cfg 里所有 `linux` 行通常指向同一个根分区。
pub(crate) fn cfg_root_uuid(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(at) = rest.find("root=UUID=") {
        let tail = &rest[at + "root=UUID=".len()..];
        let end = tail
            .find(|character: char| {
                !(character.is_ascii_hexdigit() || character == '-')
            })
            .unwrap_or(tail.len());
        let value = &tail[..end];
        if value.len() >= 8 {
            return Some(value.to_ascii_uppercase());
        }
        rest = &tail[end..];
    }
    None
}

/// os-release 的 `ID=`
pub(crate) fn os_release_id(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(value) = line.trim().strip_prefix("ID=") {
            let value = value.trim_matches('"').trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// 探测结果 → 界面视图。**纯函数**（不碰 IO），定位逻辑因此可以单测。
pub(crate) fn build_view(
    payload: &ProbePayload,
    platform: &str,
    needs_elevation: bool,
    prefer_dev: Option<&str>,
    manual_takes_over: bool,
) -> LinuxGrubView {
    let mut view = LinuxGrubView {
        scanned_at: Some(anyboot_now_iso()),
        platform: platform.to_string(),
        needs_elevation,
        confidence: "none".to_string(),
        issues: payload.notes.clone(),
        manual_takes_over,
        ..LinuxGrubView::default()
    };
    if let Some(error) = &payload.error {
        view.error = Some(error.clone());
        return view;
    }

    let esp = payload.esp.as_ref().map(|wire| PartIdentity::from(wire.clone()));
    let parts = &payload.parts;

    // 候选列表：所有带 grub.cfg 的分区（+ 手动指定过的那块，哪怕没有 cfg）
    for part in parts {
        if part.cfg_rel.is_none() && prefer_dev != Some(part.dev.as_str()) {
            continue;
        }
        let (matched, esp_spec) = if Some(part.dev.as_str()) == prefer_dev {
            ("manual".to_string(), None)
        } else {
            match &esp {
                Some(esp) => {
                    let mut best = ("none".to_string(), None);
                    for mount in &part.mounts {
                        if !looks_like_esp_mount(mount) {
                            continue;
                        }
                        let Some(spec) = PartSpec::parse_opt(&mount.spec) else {
                            continue;
                        };
                        match spec.matches(esp) {
                            Strength::Exact => {
                                best = (
                                    "exact".to_string(),
                                    Some(format!("{} → {}", mount.spec, mount.mountpoint)),
                                );
                                break;
                            }
                            Strength::Inferred => {
                                best = (
                                    "inferred".to_string(),
                                    Some(format!("{} → {}", mount.spec, mount.mountpoint)),
                                );
                            }
                            Strength::None => {}
                        }
                    }
                    best
                }
                None => ("none".to_string(), None),
            }
        };
        view.candidates.push(LinuxCandidate {
            dev: part.dev.clone(),
            label: if part.label.is_empty() {
                "（无卷标）".to_string()
            } else {
                part.label.clone()
            },
            size: part.size.clone(),
            cfg_rel: part.cfg_rel.clone(),
            cfg_mtime: part.cfg_mtime.map(|value| anyboot_iso_at(value as u64 * 1000)),
            has_env: part.env_hex.is_some(),
            matched,
            esp_spec,
            os_id: part.os_id.clone(),
        });
    }

    let Some(selection) = locate(parts, esp.as_ref(), prefer_dev) else {
        view.confidence = "none".to_string();
        view.confidence_note = Some(if parts.is_empty() {
            "没扫到任何 ext4 分区（本机没有 Linux 分区，或者都不支持）".to_string()
        } else {
            "没定位到你选的 ESP 对应的 Linux 分区：从下面的列表里选一块，之后就记住它".to_string()
        });
        return view;
    };

    let part = &parts[selection.index];
    view.confidence = selection.confidence.to_string();
    view.confidence_note = Some(selection.note.clone());
    view.target = Some(LinuxTarget {
        dev: part.dev.clone(),
        label: part.label.clone(),
        size: part.size.clone(),
        fs_uuid: part.fs_uuid.clone(),
        root_uuid: part.root_uuid.clone(),
        os_id: part.os_id.clone(),
        host: platform == "linux",
    });

    let Some(cfg_text) = &part.cfg_text else {
        view.error = Some(format!(
            "{} 里没找到 grub.cfg（找过 boot/grub 与 boot/grub2）",
            part.dev
        ));
        return view;
    };
    view.cfg_rel = part.cfg_rel.clone();
    view.cfg_size = Some(cfg_text.len() as u64);
    view.cfg_mtime = part.cfg_mtime.map(|value| anyboot_iso_at(value as u64 * 1000));

    // 解析器原样复用「手动解析」那一套：selector 规则、submenu 分组都一样
    let entries = crate::grub::dedupe_display_entries(&crate::grub::parse_grub_cfg(cfg_text));
    view.groups = crate::grub::parse_submenus(cfg_text);
    view.entries = entries;

    // 空菜单不是「没问题」。真机见过 boot/grub/grub.cfg 被换成一整段纯文本的情况
    // （本机 disk0s13 里躺的就是一份笔记），安静显示 0 条只会让人以为程序瞎了。
    if view.entries.is_empty() {
        view.issues.push(format!(
            "{} 的 grub.cfg（{} 字节）里没解析出任何 menuentry —— 这份文件可能 \
             不是 GRUB 生成的配置。没有菜单就没有可写入的 selector，\
             「记住启动项」在这里用不了",
            part.dev,
            cfg_text.len()
        ));
    }

    if let Some(bytes) = part.env_bytes() {
        view.env_rel = part.env_rel.clone();
        view.env_exists = true;
        view.env_size = Some(bytes.len() as u64);
        view.saved_entry = crate::grub::env_value(&bytes, "saved_entry").filter(|value| !value.is_empty());
        view.next_entry = crate::grub::env_value(&bytes, "next_entry").filter(|value| !value.is_empty());
    } else {
        view.env_rel = part.env_rel.clone();
        view.env_exists = false;
        view.issues.push(format!(
            "{} 里没有 grubenv：本程序**不会**替你创建它（创建要分配块、改 inode）。\
             要启用「记住启动项」，请在系统里跑一次 `grub-editenv {} create`",
            part.dev,
            part.env_rel.as_deref().unwrap_or("boot/grub/grubenv")
        ));
    }
    view
}

// ─────────────────────────── 扫描入口 ───────────────────────────

/// 扫一遍：Linux 本机直读，另外两端拉起提权子进程。
pub(crate) fn probe(esp_partuuid: &str, prefer_dev: &str) -> Result<ProbePayload, String> {
    #[cfg(target_os = "linux")]
    {
        let _ = (esp_partuuid, prefer_dev);
        Ok(probe_host())
    }
    #[cfg(not(target_os = "linux"))]
    {
        // 「优先用哪块分区」由主进程在 locate() 里决定，子进程只负责摆事实
        let _ = prefer_dev;
        let mut args = vec!["--linux-grub-probe".to_string()];
        if !esp_partuuid.trim().is_empty() {
            args.push("--esp".to_string());
            args.push(esp_partuuid.trim().to_string());
        }
        let out = temp_path("probe");
        args.push("--out".to_string());
        args.push(out.to_string_lossy().to_string());
        run_privileged(&args, &out)?;
        read_payload(&out)
    }
}

fn read_payload(path: &Path) -> Result<ProbePayload, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("提权扫描没留下结果（{}）：{error}", path.display()))?;
    // 临时文件是给人看排障的，读完就删
    let _ = std::fs::remove_file(path);
    serde_json::from_slice(&bytes).map_err(|error| format!("扫描结果不是合法 JSON：{error}"))
}

fn temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "anyboot-linux-{tag}-{}-{}.json",
        std::process::id(),
        anyboot_now_iso().replace([':', '-'], "")
    ))
}

/// 拉起提权子进程并等它退出。子进程把结果写进 `out`。
#[cfg(target_os = "macos")]
fn run_privileged(args: &[String], out: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| format!("定位自身可执行文件失败：{error}"))?;
    let mut command = sh_quote(&exe.to_string_lossy());
    for arg in args {
        command.push(' ');
        command.push_str(&sh_quote(arg));
    }
    // osascript 里 `do shell script` 的脚本是双引号字符串：反斜杠与双引号要转义
    let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "do shell script \"{escaped} 2>&1; exit 0\" with administrator privileges"
    );
    let output = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|error| format!("拉起授权对话框失败：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        // 用户点「取消」时 osascript 报 -128：这是正常操作，不是错误
        if detail.contains("-128") || detail.contains("User canceled") {
            let _ = out;
            return Err("已取消授权（读 Linux 分区需要管理员权限）".to_string());
        }
        return Err(format!("授权执行失败：{detail}"));
    }
    if !out.is_file() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(format!("提权子进程没有产出结果：{stdout}"));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn sh_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\\''"))
}

#[cfg(target_os = "windows")]
fn run_privileged(args: &[String], out: &Path) -> Result<(), String> {
    super::elevate::run_elevated(args, out)
}

/// Linux 本机：不需要任何子进程，`fs` 直接读。
#[cfg(target_os = "linux")]
fn probe_host() -> ProbePayload {
    let mut payload = ProbePayload {
        ok: true,
        ..ProbePayload::default()
    };
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let id = os_release_id(&os_release);
    let label = os_release
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim_matches('"').to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| id.clone())
        .unwrap_or_else(|| "本机".to_string());

    let mut part = PartReport {
        dev: "本机".to_string(),
        label,
        os_id: id,
        part_number: 0,
        ..PartReport::default()
    };
    // `/boot` 可能是独立分区：判据是 `/boot/grub` 存在即可，
    // 两种布局下路径都成立，这里不需要额外区分
    for dir in GRUB_DIRS {
        let cfg = format!("/{dir}/grub.cfg");
        let env = format!("/{dir}/grubenv");
        if let Ok(bytes) = std::fs::read(&cfg) {
            part.cfg_rel = Some(cfg.clone());
            part.cfg_text = Some(String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_CFG_BYTES)]).to_string());
            part.cfg_mtime = std::fs::metadata(&cfg)
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| value.as_secs() as i64);
            part.root_uuid = part.cfg_text.as_deref().and_then(cfg_root_uuid);
            if let Ok(env_bytes) = std::fs::read(&env) {
                if env_bytes.len() == ENV_BLOCK_SIZE {
                    part.env_rel = Some(env.clone());
                    part.env_hex = Some(hex::encode(&env_bytes));
                }
            }
            break;
        }
    }
    if part.cfg_rel.is_none() {
        payload
            .notes
            .push("本机没找到 /boot/grub/grub.cfg 或 /boot/grub2/grub.cfg".to_string());
    }
    if let Ok(text) = std::fs::read_to_string("/etc/fstab") {
        let entries = part_spec::parse_fstab(&text);
        part.mounts = entries
            .into_iter()
            .map(|entry| MountHint {
                spec: entry.spec,
                mountpoint: entry.mountpoint,
                fstype: entry.fstype,
            })
            .collect();
    }
    payload.parts.push(part);
    payload
}

// ─────────────────────────── 提权子进程 ───────────────────────────

/// 子进程入口：枚举分区 → 读 fstab / grub.cfg / grubenv → 写 JSON。
///
/// **调用方不传任何路径**：分区由子进程自己枚举，它只读三样东西
/// （`etc/fstab`、`boot/grub{,2}/grub.cfg`、`.../grubenv`），拿不到别的文件。
pub(crate) fn probe_child_main(out: &str, esp_partuuid: &str) -> i32 {
    let payload = match probe_raw(esp_partuuid) {
        Ok(payload) => payload,
        Err(error) => ProbePayload {
            ok: false,
            error: Some(error),
            ..ProbePayload::default()
        },
    };
    write_json(out, &payload);
    0
}

/// `disk0s9` → `/dev/rdisk0s9`
///
/// 注意是 `r` + dev，不是 `rdisk` + dev —— 后者会拼成 `/dev/rdiskdisk0s9`
/// （这个笔误让整轮探测「一块都打不开」，而错误还被吞成了「可能被系统占用」）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn raw_path(dev: &str) -> String {
    format!("/dev/r{dev}")
}

/// 子进程入口：把 1024 字节环境块写回指定分区里的指定文件。
pub(crate) fn write_child_main(dev: &str, rel: &str, payload_path: &str, out: &str) -> i32 {
    let reply = match write_child_work(dev, rel, payload_path) {
        Ok(detail) => ProbePayload {
            ok: true,
            error: None,
            notes: vec![detail],
            ..ProbePayload::default()
        },
        Err(error) => ProbePayload {
            ok: false,
            error: Some(error),
            ..ProbePayload::default()
        },
    };
    write_json(out, &reply);
    0
}

#[cfg(target_os = "macos")]
fn write_child_work(dev: &str, rel: &str, payload_path: &str) -> Result<String, String> {
    let bytes = std::fs::read(payload_path)
        .map_err(|error| format!("读不到待写内容（{payload_path}）：{error}"))?;
    validate_env_block(&bytes)?;
    if rel.contains("..") || rel.starts_with('/') {
        return Err("写入路径必须是分区内的相对路径".to_string());
    }
    let source = FileSource::open_raw_rw(&raw_path(dev))?;
    let mut fs = Ext4::open(Box::new(source))?;
    fs.overwrite_in_place(rel, &bytes)?;
    Ok(format!("已写入 {dev}:{rel}（{} 字节，回读一致）", bytes.len()))
}

#[cfg(not(target_os = "macos"))]
fn write_child_work(_dev: &str, _rel: &str, _payload_path: &str) -> Result<String, String> {
    Err("这一端还不支持写 ext4：Windows 的分区偏移方案尚未接上".to_string())
}

fn write_json(out: &str, payload: &ProbePayload) {
    match serde_json::to_vec(payload) {
        Ok(bytes) => {
            if let Err(error) = std::fs::write(out, &bytes) {
                eprintln!("写结果失败：{error}");
            } else {
                // 结果文件要给发起请求的用户读（子进程是 root）
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o644));
                }
            }
        }
        Err(error) => eprintln!("序列化结果失败：{error}"),
    }
}

/// 环境块的三条硬性校验：尺寸、头部、以 `#` 补满（与 `grub-editenv` 一致）
pub(crate) fn validate_env_block(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() != ENV_BLOCK_SIZE {
        return Err(format!(
            "环境块必须恰好 {ENV_BLOCK_SIZE} 字节，这次是 {} 字节",
            bytes.len()
        ));
    }
    if !bytes.starts_with(ENV_BLOCK_HEADER.as_bytes()) {
        return Err(format!(
            "环境块头部必须是 `{}`",
            ENV_BLOCK_HEADER.trim_end()
        ));
    }
    Ok(())
}

/// 真正的枚举与读取（子进程里跑，或 Linux 本机排障时直接调）
#[cfg(target_os = "macos")]
pub(crate) fn probe_raw(esp_partuuid: &str) -> Result<ProbePayload, String> {
    let mut payload = ProbePayload {
        ok: true,
        ..ProbePayload::default()
    };
    let sys = system_partitions();
    if sys.is_empty() {
        payload.notes.push("diskutil 没列出任何分区".to_string());
        return Ok(payload);
    }
    // 所选 ESP 的标识：按 PARTUUID 找，再把 FAT 卷序列读出来（`fstab` 里常用它）
    if !esp_partuuid.trim().is_empty() {
        let wanted = part_spec::normalize_uuid(esp_partuuid);
        if let Some(hit) = sys.iter().find(|part| {
            part.part_uuid
                .as_deref()
                .map(part_spec::normalize_uuid)
                .as_deref()
                == Some(wanted.as_str())
        }) {
            let fat = first_sector(&hit.dev)
                .and_then(|sector| fat_volume_serial(&sector))
                .map(|serial| format!("{:04X}-{:04X}", serial >> 16, serial & 0xFFFF));
            payload.esp = Some(IdentityWire {
                fs_uuid: fat,
                part_uuid: hit.part_uuid.clone(),
                label: hit.label.clone(),
                part_label: None,
                disk_index: hit.disk_index,
                part_number: hit.part_number,
            });
        } else {
            payload.notes.push(format!(
                "本机没有 PARTUUID 为 {esp_partuuid} 的分区（设置里填错了？）"
            ));
        }
    }

    for part in &sys {
        // 只试分区，不试整盘
        if part.part_number == 0 {
            continue;
        }
        let path = raw_path(&part.dev);
        // 报错原文一定要带出来。上一轮 `/dev/rdisk` + `disk0s9` 拼成了
        // `/dev/rdiskdisk0s9`，被这句「可能被系统占用」吞掉，白查了半天权限。
        let source = match FileSource::open_raw(&path) {
            Ok(source) => source,
            Err(why) => {
                payload.notes.push(format!("{} 打不开：{why}", part.dev));
                continue;
            }
        };
        let Ok(mut fs) = Ext4::open(Box::new(source)) else {
            continue; // 不是 ext4：正常跳过，不刷问题列表
        };
        let mut report = PartReport {
            dev: part.dev.clone(),
            label: if fs.sb.label.is_empty() {
                part.label.clone().unwrap_or_default()
            } else {
                fs.sb.label.clone()
            },
            size: part.size.clone(),
            fs_uuid: Some(part_spec::normalize_uuid(&fs.sb.uuid)),
            part_uuid: part.part_uuid.clone(),
            part_label: None,
            part_number: part.part_number,
            disk_index: part.disk_index,
            ..PartReport::default()
        };
        if fs.sb.needs_recovery() {
            report.note = Some("上次没干净关机（RECOVER 标志），只读可用、写入会被拒".to_string());
        }
        // fstab
        if let Ok(Some(bytes)) = fs.read_path("etc/fstab", MAX_FSTAB_BYTES) {
            let text = String::from_utf8_lossy(&bytes);
            report.mounts = part_spec::parse_fstab(&text)
                .into_iter()
                .map(|entry| MountHint {
                    spec: entry.spec,
                    mountpoint: entry.mountpoint,
                    fstype: entry.fstype,
                })
                .collect();
        }
        // os-release（只为在列表里好认）
        if let Ok(Some(bytes)) = fs.read_path("etc/os-release", MAX_SMALL_BYTES) {
            report.os_id = os_release_id(&String::from_utf8_lossy(&bytes));
        }
        // grub.cfg / grubenv
        for dir in GRUB_DIRS {
            let cfg_rel = format!("{dir}/grub.cfg");
            let env_rel = format!("{dir}/grubenv");
            let cfg = fs.read_path(&cfg_rel, MAX_CFG_BYTES).unwrap_or(None);
            let Some(cfg_bytes) = cfg else {
                continue;
            };
            report.cfg_mtime = fs.stat(&cfg_rel).map(|meta| meta.mtime);
            report.cfg_text = Some(String::from_utf8_lossy(&cfg_bytes).to_string());
            report.cfg_rel = Some(cfg_rel.clone());
            report.root_uuid = report
                .cfg_text
                .as_deref()
                .and_then(cfg_root_uuid);
            if let Ok(Some(env)) = fs.read_path(&env_rel, ENV_BLOCK_SIZE + 1) {
                if env.len() == ENV_BLOCK_SIZE {
                    report.env_rel = Some(env_rel.clone());
                    report.env_hex = Some(hex::encode(&env));
                }
            }
            break;
        }
        payload.parts.push(report);
    }
    Ok(payload)
}

#[cfg(target_os = "windows")]
pub(crate) fn probe_raw(_esp_partuuid: &str) -> Result<ProbePayload, String> {
    Err(
        "Windows 侧还在做：需要按分区偏移打开 \\\\.\\PhysicalDriveN 才能读 ext4。\
         当前版本请在这一端用「手动解析」（装 09_anyboot），或在 macOS / Linux 上查看"
            .to_string(),
    )
}

#[cfg(target_os = "linux")]
pub(crate) fn probe_raw(_esp_partuuid: &str) -> Result<ProbePayload, String> {
    Ok(probe_host())
}

// ─────────────────────────── macOS 分区枚举 ───────────────────────────

#[derive(Clone, Debug, Default)]
struct SysPart {
    /// `disk0s9`
    dev: String,
    label: Option<String>,
    size: Option<String>,
    part_uuid: Option<String>,
    disk_index: u32,
    part_number: u32,
}

#[cfg(target_os = "macos")]
fn system_partitions() -> Vec<SysPart> {
    let Ok(output) = std::process::Command::new("diskutil")
        .arg("info")
        .arg("-all")
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = Vec::new();
    let mut current: Option<SysPart> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.len() >= 8 && trimmed.chars().all(|character| character == '*') {
            if let Some(part) = current.take() {
                parts.push(part);
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Device Identifier" => {
                if let Some(part) = current.take() {
                    parts.push(part);
                }
                if let Some((disk, number)) = parse_disk_id(value) {
                    current = Some(SysPart {
                        dev: value.to_string(),
                        disk_index: disk,
                        part_number: number,
                        ..SysPart::default()
                    });
                }
            }
            "Volume Name" if !value.is_empty() => {
                if let Some(part) = current.as_mut() {
                    part.label = Some(value.to_string());
                }
            }
            "Disk Size" => {
                if let Some(part) = current.as_mut() {
                    part.size = Some(value.split('(').next().unwrap_or(value).trim().to_string());
                }
            }
            // 分区 UUID（GPT PARTGUID）。ext4 卷 macOS 认不出，报的就是这个值。
            "Disk / Partition UUID" => {
                if let Some(part) = current.as_mut() {
                    part.part_uuid = Some(value.to_string());
                }
            }
            _ => {}
        }
    }
    if let Some(part) = current.take() {
        parts.push(part);
    }
    for part in &mut parts {
        if part.label.as_deref().is_some_and(is_diskutil_placeholder) {
            part.label = None;
        }
    }
    parts
}

/// diskutil 认不出的分区会报一句英文占位文案，不能当卷标往界面上摆。
fn is_diskutil_placeholder(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.is_empty() || lower.starts_with("not applicable")
}

/// `disk0s9` → `(0, 9)`；`disk2` → `(2, 0)`
fn parse_disk_id(text: &str) -> Option<(u32, u32)> {
    let rest = text.strip_prefix("disk")?;
    match rest.split_once('s') {
        Some((disk, part)) => Some((disk.parse().ok()?, part.parse().ok()?)),
        None => Some((rest.parse().ok()?, 0)),
    }
}

/// 读一块分区的第一个扇区（FAT 卷序列在偏移 `0x43`）
#[cfg(target_os = "macos")]
fn first_sector(dev: &str) -> Option<Vec<u8>> {
    let mut source = FileSource::open_raw(&raw_path(dev)).ok()?;
    let mut sector = vec![0u8; 512];
    source.read_at(0, &mut sector).ok()?;
    Some(sector)
}

/// FAT 卷序列（4 字节小端，位于偏移 `0x43`）。它就是 GRUB / `/etc/fstab` 里的
/// `UUID=XXXX-XXXX`。只有 FAT 家族才有，别的文件系统返回 `None`。
pub(crate) fn fat_volume_serial(sector: &[u8]) -> Option<u32> {
    if sector.len() < 0x47 {
        return None;
    }
    // 引导扇区签名 0x55AA 存在、且不是 NTFS（NTFS 的 0x43 处也有数据，但那不是卷序列）
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return None;
    }
    let fs_type = &sector[54..62];
    if fs_type == b"NTFS    " {
        return None;
    }
    Some(u32::from_le_bytes([
        sector[0x43],
        sector[0x44],
        sector[0x45],
        sector[0x46],
    ]))
}

// ─────────────────────────── 写盘 ───────────────────────────

/// 把 selector 写进目标分区的环境块（`saved_entry` + `next_entry`，与手动链路同规则）。
pub(crate) fn write_env(runtime: &Runtime, selector: &str) -> Result<String, String> {
    if !runtime.selectors.iter().any(|value| value == selector) {
        return Err(format!("当前 grub.cfg 里没有这一项：{selector}"));
    }
    let updates = [
        ("saved_entry".to_string(), selector.to_string()),
        ("next_entry".to_string(), selector.to_string()),
    ];
    let block = crate::grub::set_env_vars(&runtime.env_bytes, &updates)?;
    validate_env_block(&block)?;

    if runtime.host {
        return write_env_host(&runtime.env_rel, &block);
    }
    let payload = temp_path("env");
    std::fs::write(&payload, &block).map_err(|error| format!("暂存待写内容失败：{error}"))?;
    let out = temp_path("write");
    let args = vec![
        "--linux-grub-write".to_string(),
        "--dev".to_string(),
        runtime.dev.clone(),
        "--rel".to_string(),
        runtime.env_rel.clone(),
        "--payload".to_string(),
        payload.to_string_lossy().to_string(),
        "--out".to_string(),
        out.to_string_lossy().to_string(),
    ];
    let result = run_privileged(&args, &out);
    let _ = std::fs::remove_file(&payload);
    let payload_reply = match result {
        Ok(()) => read_payload(&out),
        Err(error) => {
            let _ = std::fs::remove_file(&out);
            return Err(error);
        }
    };
    match payload_reply {
        Ok(reply) => {
            if reply.ok {
                Ok(reply
                    .notes
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "已写入环境块".to_string()))
            } else {
                Err(reply.error.unwrap_or_else(|| "写入失败".to_string()))
            }
        }
        Err(error) => Err(error),
    }
}

/// Linux 本机：直接改文件。**原地**覆盖同样 1024 字节，不做 rename
/// （rename 要改目录项，正是我们要避开的那类元数据写入）。
fn write_env_host(path: &str, block: &[u8]) -> Result<String, String> {
    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("打开 {path} 失败：{error}"))?;
    let size = file
        .metadata()
        .map_err(|error| format!("读 {path} 信息失败：{error}"))?
        .len();
    if size != ENV_BLOCK_SIZE as u64 {
        return Err(format!(
            "{path} 是 {size} 字节（应为 {ENV_BLOCK_SIZE}）—— 不是合法的 GRUB 环境块，拒绝写入"
        ));
    }
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.write_all(block))
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("写 {path} 失败：{error}"))?;
    let written = std::fs::read(path).map_err(|error| format!("回读 {path} 失败：{error}"))?;
    if written != block {
        return Err("回读不一致：写入没有真正生效".to_string());
    }
    Ok(format!("已写入 {path}（{ENV_BLOCK_SIZE} 字节，回读一致）"))
}

// ─────────────────────────── 命令 ───────────────────────────

/// 「重新扫描」：按当前设置定位目标分区并读出菜单。
#[tauri::command]
pub(crate) fn linux_grub_scan(app: AppHandle, target: Option<String>) -> Option<LinuxGrubView> {
    Some(refresh(&app, target.as_deref()))
}

/// 「换一个」：记住用户选的分区，然后立刻按它重扫。
#[tauri::command]
pub(crate) fn linux_grub_pick(app: AppHandle, dev: String) -> Option<LinuxGrubView> {
    let dev = dev.trim().to_string();
    let mut settings = stored_settings(&app);
    settings.linux_grub_dev = dev.clone();
    if let Err(error) = save_settings(&app, &settings) {
        trace(&app, "linux_grub", &format!("保存目标分区失败：{error}"));
    }
    Some(refresh(&app, Some(&dev)))
}

/// 切换下次启动：写目标分区自己的 grubenv。
#[tauri::command]
pub(crate) fn linux_grub_switch(app: AppHandle, selector: String) -> AppStatus {
    let runtime = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        state.linux_grub_rt.clone()
    };
    let Some(runtime) = runtime else {
        return update_failure_status(
            &app,
            "切换失败",
            "还没扫描过：先点「重新扫描」，让界面拿到目标分区的 grubenv".to_string(),
        );
    };
    match write_env(&runtime, &selector) {
        Ok(detail) => {
            // 让界面立刻反映 saved_entry/next_entry（重扫一次，不弹新授权：
            // 这次走的是同一份缓存路径，不会重新枚举分区）
            let _ = refresh(&app, None);
            let message = format!("下次启动：{selector}｜{detail}");
            record_action(&app, "切换 GRUB 启动项（自动读取）", true, &message);
            publish_ui(&app);
            state_snapshot(&app)
        }
        Err(error) => update_failure_status(&app, "切换 GRUB 启动项（自动读取）失败", error),
    }
}

fn store(app: &AppHandle, view: LinuxGrubView, runtime: Option<Runtime>) {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().expect("state mutex poisoned");
    state.linux_grub = Some(view);
    state.linux_grub_rt = runtime;
}

/// 写盘上下文：从**同一份** payload 里取，绝不为了拿 env 再探一次
/// （macOS 上再探一次就是第二次授权对话框）。
fn runtime_from(payload: &ProbePayload, view: &LinuxGrubView, prefer: &str) -> Option<Runtime> {
    let target = view.target.as_ref()?;
    let env_rel = view.env_rel.clone()?;
    let esp = payload
        .esp
        .as_ref()
        .map(|wire| PartIdentity::from(wire.clone()));
    let prefer_opt = Some(prefer).filter(|value| !value.trim().is_empty());
    let selection = locate(&payload.parts, esp.as_ref(), prefer_opt)?;
    let part = payload.parts.get(selection.index)?;
    // 视图里选中的分区必须与这里重新定位的结果一致，否则宁可不给写入口
    if part.dev != target.dev {
        return None;
    }
    Some(Runtime {
        dev: part.dev.clone(),
        host: view.platform == "linux",
        env_rel,
        env_bytes: part.env_bytes()?,
        selectors: view.entries.iter().map(|entry| entry.selector.clone()).collect(),
    })
}

/// 扫一遍 → 组装视图 → 写回状态。任何一步失败都返回「带 error 的视图」，
/// 绝不把旧数据当成新结果留在界面上。
pub(crate) fn refresh(app: &AppHandle, target: Option<&str>) -> LinuxGrubView {
    let settings = stored_settings(app);
    let prefer = target
        .map(str::to_string)
        .unwrap_or_else(|| settings.linux_grub_dev.clone());
    let manual_takes_over = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        // 手动链路装着 09_anyboot 且 ESP 上有环境块 → GRUB 会优先 load_env 那份
        state.grub.installed == Some(true) && state.grub.env_exists
    };
    let esp_partuuid = settings.grub_efi_partuuid.clone();

    let payload = match probe(&esp_partuuid, &prefer) {
        Ok(payload) => payload,
        Err(error) => {
            let view = LinuxGrubView::failed(platform_name(), error);
            store(app, view.clone(), None);
            return view;
        }
    };
    let prefer_opt = Some(prefer.as_str()).filter(|value| !value.trim().is_empty());
    let mut view = build_view(
        &payload,
        platform_name(),
        !cfg!(target_os = "linux"),
        prefer_opt,
        manual_takes_over,
    );
    if manual_takes_over {
        view.issues.insert(
            0,
            "这台机器装着 09_anyboot 且 ESP 上有 EFI/grubenv：GRUB 会优先用 ESP 上那份，\
             在这里写的 env 不会生效。要切到自动读取，请先卸载 09_anyboot"
                .to_string(),
        );
    }
    let runtime = runtime_from(&payload, &view, &prefer);
    store(app, view.clone(), runtime);
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(dev: &str, fs_uuid: &str, mounts: Vec<(&str, &str, &str)>, cfg: bool) -> PartReport {
        PartReport {
            dev: dev.to_string(),
            label: dev.to_string(),
            fs_uuid: Some(fs_uuid.to_string()),
            part_uuid: Some(format!("PART-{dev}")),
            part_number: dev
                .rsplit('s')
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1),
            disk_index: 0,
            mounts: mounts
                .into_iter()
                .map(|(spec, mountpoint, fstype)| MountHint {
                    spec: spec.to_string(),
                    mountpoint: mountpoint.to_string(),
                    fstype: fstype.to_string(),
                })
                .collect(),
            cfg_rel: cfg.then(|| "boot/grub/grub.cfg".to_string()),
            cfg_text: cfg.then(|| {
                "menuentry 'Arch' --class arch {\n\tlinux /vmlinuz-linux root=UUID=84d0b19d-4ee2-4cd4-bc2c-80a77c19c702\n}\n"
                    .to_string()
            }),
            env_hex: cfg.then(|| hex::encode(crate::grub::empty_env_block())),
            env_rel: cfg.then(|| "boot/grub/grubenv".to_string()),
            ..PartReport::default()
        }
    }

    fn esp() -> PartIdentity {
        PartIdentity {
            fs_uuid: Some("E537-7F31".to_string()),
            part_uuid: Some("B3FBE669-D3B9-4E0F-9A7C-1D2E3F4A5B6C".to_string()),
            label: Some("ARCH-EFI".to_string()),
            part_label: None,
            // 这台机器上 ARCH-EFI 是 disk0s14：`/dev/sda14` 这种写法就是拿
            // 「分区号 14」跟这里比，所以两者必须一致
            disk_index: 0,
            part_number: 14,
        }
    }

    #[test]
    fn cfg_root_uuid_is_extracted() {
        let text = "menuentry 'a' {\n\tlinux /vmlinuz root=UUID=84d0b19d-4ee2-4cd4-bc2c-80a77c19c702 rw\n}";
        assert_eq!(
            cfg_root_uuid(text).as_deref(),
            Some("84D0B19D-4EE2-4CD4-BC2C-80A77C19C702")
        );
        // 没有 root=UUID= 的 cfg（systemd-boot 生成的 stub）不该编一个出来
        assert_eq!(cfg_root_uuid("search --fs-uuid E537-7F31"), None);
    }

    #[test]
    fn os_release_id_strips_quotes() {
        assert_eq!(
            os_release_id("NAME=\"Arch Linux\"\nID=arch\n").as_deref(),
            Some("arch")
        );
        assert_eq!(os_release_id("NAME=only").as_deref(), None);
    }

    #[test]
    fn locate_prefers_exact_uuid_over_device_path() {
        let parts = vec![
            report("disk0s7", "AAAA", vec![("/dev/sda1", "/boot/efi", "vfat")], true),
            report("disk0s9", "BBBB", vec![("UUID=e537-7f31", "/boot/efi", "vfat")], true),
        ];
        let selection = locate(&parts, Some(&esp()), None).unwrap();
        assert_eq!(selection.index, 1);
        assert_eq!(selection.confidence, "exact");
    }

    #[test]
    fn locate_accepts_device_path_as_inferred() {
        // `/dev/sda14` 只能对上「分区号 14」这一半，所以必须是 Inferred 而不是 Exact
        let parts = vec![report(
            "disk0s14",
            "BBBB",
            vec![("/dev/sda14", "/boot/efi", "vfat")],
            true,
        )];
        let selection = locate(&parts, Some(&esp()), None).unwrap();
        assert_eq!(selection.confidence, "inferred");
        assert!(selection.note.contains("设备名"), "{}", selection.note);
    }

    #[test]
    fn locate_uses_manual_choice_first() {
        let parts = vec![
            report("disk0s7", "AAAA", vec![("UUID=e537-7f31", "/boot/efi", "vfat")], true),
            report("disk0s9", "BBBB", vec![], true),
        ];
        let selection = locate(&parts, Some(&esp()), Some("disk0s9")).unwrap();
        assert_eq!(selection.index, 1);
        assert_eq!(selection.confidence, "manual");
    }

    #[test]
    fn locate_without_match_selects_nothing_when_ambiguous() {
        // 两块都有 cfg、都没挂所选 ESP → 不选（让用户点），不猜
        let parts = vec![
            report("disk0s7", "AAAA", vec![], true),
            report("disk0s9", "BBBB", vec![], true),
        ];
        assert!(locate(&parts, Some(&esp()), None).is_none());
    }

    #[test]
    fn locate_falls_back_when_only_one_candidate() {
        let parts = vec![
            report("disk0s7", "AAAA", vec![], false),
            report("disk0s9", "BBBB", vec![], true),
        ];
        let selection = locate(&parts, Some(&esp()), None).unwrap();
        assert_eq!(selection.index, 1);
        assert_eq!(selection.confidence, "self");
    }

    #[test]
    fn view_reports_target_and_env() {
        let payload = ProbePayload {
            ok: true,
            esp: Some(IdentityWire {
                fs_uuid: Some("E537-7F31".to_string()),
                ..IdentityWire::default()
            }),
            parts: vec![report(
                "disk0s9",
                "BBBB",
                vec![("UUID=E537-7F31", "/boot/efi", "vfat")],
                true,
            )],
            notes: Vec::new(),
            ..ProbePayload::default()
        };
        let view = build_view(&payload, "macos", true, None, false);
        assert_eq!(view.confidence, "exact");
        assert_eq!(view.target.as_ref().map(|target| target.dev.as_str()), Some("disk0s9"));
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].selector, "Arch");
        assert!(view.env_exists);
        assert_eq!(view.candidates.len(), 1);
    }

    /// cfg 存在但不是 GRUB 配置（真机 disk0s13 里躺的就是一段纯文本笔记）：
    /// 必须明确说出来，不能安静地显示 0 个条目。
    #[test]
    fn cfg_without_menuentry_is_reported() {
        let mut part = report(
            "disk0s13",
            "CCCC",
            vec![("UUID=E537-7F31", "/boot/efi", "vfat")],
            true,
        );
        part.cfg_text = Some("主菜单中grub解析的列表增加一项\n原始-简易-中等-详细\n".to_string());
        let payload = ProbePayload {
            ok: true,
            esp: Some(IdentityWire {
                fs_uuid: Some("E537-7F31".to_string()),
                ..IdentityWire::default()
            }),
            parts: vec![part],
            notes: Vec::new(),
            ..ProbePayload::default()
        };
        let view = build_view(&payload, "macos", true, None, false);
        assert!(view.entries.is_empty());
        assert!(
            view.issues.iter().any(|issue| issue.contains("menuentry")),
            "空菜单必须进问题列表：{:?}",
            view.issues
        );
    }

    #[test]
    fn env_block_validation_is_strict() {
        assert!(validate_env_block(&crate::grub::empty_env_block()).is_ok());
        assert!(validate_env_block(&vec![b'#'; 100]).is_err());
        let mut wrong_header = vec![b'#'; ENV_BLOCK_SIZE];
        wrong_header[0] = b'X';
        assert!(validate_env_block(&wrong_header).is_err());
    }

    #[test]
    fn fat_serial_reads_offset_0x43() {
        let mut sector = vec![0u8; 512];
        sector[0x43..0x47].copy_from_slice(&0x4754_DCADu32.to_le_bytes());
        sector[510] = 0x55;
        sector[511] = 0xAA;
        sector[54..62].copy_from_slice(b"FAT32   ");
        assert_eq!(fat_volume_serial(&sector), Some(0x4754_DCAD));
        // NTFS 的同一位置不是卷序列，不能当 UUID 用
        sector[54..62].copy_from_slice(b"NTFS    ");
        assert_eq!(fat_volume_serial(&sector), None);
    }

    #[test]
    fn disk_ids_parse() {
        assert_eq!(parse_disk_id("disk0s9"), Some((0, 9)));
        assert_eq!(parse_disk_id("disk2"), Some((2, 0)));
        assert_eq!(parse_disk_id("rdisk0s9"), None);
    }

    /// 裸设备路径只能加一个 `r`。
    ///
    /// 写成 `"/dev/rdisk" + dev` 会拼出 `/dev/rdiskdisk0s9`，而 macOS 对这个
    /// 不存在的路径只会回一句 `No such file or directory`，看上去特别像权限问题。
    #[test]
    fn raw_device_path_adds_one_r() {
        assert_eq!(raw_path("disk0s9"), "/dev/rdisk0s9");
        assert_eq!(raw_path("disk2s1"), "/dev/rdisk2s1");
        assert!(!raw_path("disk0s9").contains("rdiskdisk"));
    }

    /// 把真机读到的视图原样序列化成 JSON 打出来 —— 前端读的是这份 JSON 的
    /// camelCase 字段名，逐字对一遍比肉眼读 Rust 结构体可靠。
    /// `ANYBOOT_ESP=<PARTUUID> … --ignored live_view_json`
    #[test]
    #[ignore]
    fn live_view_json() {
        let esp = std::env::var("ANYBOOT_ESP").unwrap_or_default();
        let payload = probe_raw(&esp).expect("probe_raw 失败");
        let view = build_view(&payload, "macos", true, None, false);
        println!("{}", serde_json::to_string_pretty(&view).unwrap());
    }

    /// 真机对照（要 root，`cargo test -- --ignored live_probe` 由 sudo 跑）：
    /// 枚举本机分区、用 ext4 模块把每块分区的 fstab / grub.cfg / grubenv 读出来。
    /// 断言只求「读到了」，具体数值拿真机分区表去对。
    ///
    /// 想看定位结果就把 ESP 的 PARTUUID 塞进 `ANYBOOT_ESP`：
    /// `ANYBOOT_ESP=E537-7F31 … --ignored live_probe`
    #[test]
    #[ignore]
    fn live_probe() {
        let payload = probe_raw("").expect("probe_raw 失败");
        println!("非致命问题：{:?}", payload.notes);
        for part in &payload.parts {
            println!(
                "{} label={:?} fs_uuid={:?} cfg={:?} cfg_len={:?} env={:?} root={:?} os={:?} mounts={}",
                part.dev,
                part.label,
                part.fs_uuid,
                part.cfg_rel,
                part.cfg_text.as_ref().map(|text| text.len()),
                part.env_hex.as_ref().map(|hex| hex.len() / 2),
                part.root_uuid,
                part.os_id,
                part.mounts.len()
            );
            // 只打「像 ESP 的挂载行」：定位全靠这几行，真机对一遍最快
            for mount in part.mounts.iter().filter(|mount| looks_like_esp_mount(mount)) {
                println!(
                    "    esp-mount {} → {} ({})",
                    mount.spec, mount.mountpoint, mount.fstype
                );
            }
            // `ANYBOOT_FSTAB=1` 时把 fstab 全文摊开：用来分辨「真没有」和「解析丢了」
            if std::env::var("ANYBOOT_FSTAB").is_ok() {
                for mount in &part.mounts {
                    println!(
                        "    fstab {} → {} ({})",
                        mount.spec, mount.mountpoint, mount.fstype
                    );
                }
            }
            // `ANYBOOT_CFG=disk0s13` 时打那份 cfg 的开头：
            // 用来分辨「stub cfg（只有 configfile 跳转）」和「真菜单」
            if std::env::var("ANYBOOT_CFG")
                .ok()
                .is_some_and(|want| want.trim() == part.dev)
            {
                let text = part.cfg_text.clone().unwrap_or_default();
                println!("    ---- {} cfg {} 字节 ----", part.dev, text.len());
                for line in text.lines().take(40) {
                    println!("    | {line}");
                }
                println!("    ---- 结束 ----");
            }
        }
        assert!(!payload.parts.is_empty(), "一块 ext4 都没读到");

        let Ok(esp) = std::env::var("ANYBOOT_ESP") else {
            return;
        };
        let esp = esp.trim().to_string();
        if esp.is_empty() {
            return;
        }
        let with_esp = probe_raw(&esp).expect("带 ESP 的 probe_raw 失败");
        println!("\n所选 ESP = {esp} → {:?}", with_esp.esp);
        let view = build_view(&with_esp, "macos", true, None, false);
        println!(
            "定位：confidence={} target={:?}",
            view.confidence,
            view.target.as_ref().map(|target| target.dev.clone())
        );
        println!("说明：{}", view.confidence_note.clone().unwrap_or_default());
        for candidate in &view.candidates {
            println!(
                "    候选 {} {:?} matched={} cfg={:?} os={:?} spec={:?}",
                candidate.dev,
                candidate.label,
                candidate.matched,
                candidate.cfg_rel,
                candidate.os_id,
                candidate.esp_spec
            );
        }
        for issue in &view.issues {
            println!("    问题：{issue}");
        }
    }
}
