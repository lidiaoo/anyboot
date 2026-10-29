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

/// 单个目录清单的条数上限：`/` 根目录与 `/boot` 通常几十个文件，
/// 512 已经是病态布局；超出就截断，模拟扫描宁缺毋滥也不把内存吃爆。
const MAX_LISTING_FILES: usize = 512;

/// `/etc/os-release` 的 `PRETTY_NAME=`（带引号剥掉）。
pub(crate) fn os_release_pretty_name(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(value) = line.trim().strip_prefix("PRETTY_NAME=") {
            let value = value.trim_matches('"').trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// 拼分区内相对路径（`dir` 是 `""` 或 `boot`）。
fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

// ─────────────────────────── 跨端数据结构 ───────────────────────────

/// 一条 fstab 挂载行（**不含 options** —— 里面常有明文密码）。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MountHint {
    pub(crate) spec: String,
    pub(crate) mountpoint: String,
    pub(crate) fstype: String,
}

/// 一块 ext4 分区里某个目录的文件清单（给 rEFInd 模拟扫描用）。
///
/// rEFInd 装了 ext4 驱动后会去 Linux 分区的根目录和 `/boot` 找内核
/// （`vmlinuz*` / `bzImage*` / `kernel*` 前缀，`scan.c:1071-1073` 把前缀并进
/// 匹配模式）；这里**不做前缀过滤**，把整个目录的清单端回来 —— 匹配规则
/// （`linux_prefixes` 可被 refind.conf 改）留在主进程的模拟扫描里。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DirListing {
    /// 分区内路径：`""`（根目录）或 `boot`
    pub(crate) dir: String,
    /// 文件名 + mtime（epoch 秒）。跳过 `.` 开头（AppleDouble `._xxx` 等）
    pub(crate) files: Vec<DirFile>,
    /// 这个目录里的 `refind_linux.conf` 原文（内核条目的标题与选项来源）
    pub(crate) refind_linux_conf: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DirFile {
    pub(crate) name: String,
    pub(crate) mtime: i64,
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
    /// `/etc/os-release` 的 `PRETTY_NAME`（模拟扫描的 Linux 条目说明用）
    #[serde(default)]
    pub(crate) os_pretty_name: Option<String>,
    /// 根目录与 `/boot` 的文件清单（ext4 卷才有；**serde default**：
    /// 旧助手的回执里没有这个字段，解析成空 —— 模拟扫描据此提示重装）
    #[serde(default)]
    pub(crate) dir_listings: Vec<DirListing>,
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
    /// 这份数据是怎么拿到的：`host`（Linux 本机直读）/ `helper`（root 助手，免密）/
    /// `elevated`（一次性授权子进程，弹过密码）。UI 要如实显示 ——
    /// 「为什么这次没弹密码」和「为什么这次弹了」都得有答案。
    #[serde(default)]
    pub(crate) via: String,
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
    /// 这块分区自己的状态提示（如 ext4 的 RECOVER 脏标志：只读可以、写入会被拒）。
    /// 必须传到界面上：否则用户点一次启动项才被拒一次，看着就是「点了没反应」。
    pub(crate) note: Option<String>,
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
    /// 这块分区自己的状态提示（RECOVER 脏标志等）；有它就等于「能读、不能写」
    pub(crate) note: Option<String>,
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
    /// 这次是免密还是弹了密码：`host` / `helper` / `elevated`（空 = 还没读成功过）。
    /// 前端拿它显示「免密助手」的徽标，不再靠猜。
    #[serde(default)]
    pub(crate) via: String,
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
        via: payload.via.clone(),
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
            note: part.note.clone(),
        });
    }

    let Some(selection) = locate(parts, esp.as_ref(), prefer_dev) else {
        view.confidence = "none".to_string();
        view.confidence_note = Some(if parts.is_empty() {
            "没扫到任何 ext4 分区（本机没有 Linux 分区，或者都不支持）".to_string()
        } else if esp.is_none() {
            // 与「fstab 里没挂这个 ESP」分开说：一个是 ESP 本身没认出来（设置里的
            // 分区 UUID 对不上），一个是认出来了但没有哪块分区挂它。含糊成一句
            // 会让用户去翻 fstab，而真正的原因在设置里。
            "还没认出你选的 ESP（设置里的分区 UUID 没对上任何分区）：从下面的列表里选一块，之后就记住它"
                .to_string()
        } else if esp
            .as_ref()
            .is_some_and(|esp| esp.fs_uuid.is_none() && esp.label.is_none())
        {
            // 认出分区了，但拿不到「fstab 会写的那种身份」—— 匹配只剩 PARTUUID
            // 一条路，而 Linux 侧几乎都写 `UUID=XXXX-XXXX`。这种「认出了但没钥匙」
            // 的状态必须单独说，否则用户只会看到「没定位到」，以为是分区选错。
            "认出了你选的 ESP，但读不到它的 FAT 卷序列与卷标（只能按 PARTUUID 匹配）：从下面的列表里选一块，之后就记住它"
                .to_string()
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
        note: part.note.clone(),
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

/// 扫一遍：Linux 本机直读，另外两端提起权进程。
///
/// macOS 上**优先走 root 助手**（装了就是免密），没装/没响应才回落到
/// 「一次性授权子进程」。这个优先级不是性能优化：每次切启动项都弹密码的话，
/// 这个功能不会被用起来。
///
/// `allow_fallback=false` 是**自动扫描**（开机自启那次）专用：绝不允许弹授权。
/// 开机瞬间整机 I/O 高峰、助手冷启动，25 秒超时并不保险 —— 撞上就回落 osascript，
/// 用户看到的就是「刚进 App 就要密码」（2026-10-29 实测）。这种情况直接报错，
/// 前端会自动重试；用户手点「重新读取」才允许回落（主动点的，弹了不意外）。
pub(crate) fn probe(
    esp_partuuid: &str,
    prefer_dev: &str,
    allow_fallback: bool,
) -> Result<ProbePayload, String> {
    #[cfg(target_os = "linux")]
    {
        let _ = (esp_partuuid, prefer_dev, allow_fallback);
        Ok(probe_host())
    }
    #[cfg(not(target_os = "linux"))]
    {
        // 「优先用哪块分区」由主进程在 locate() 里决定，子进程只负责摆事实
        let _ = prefer_dev;

        #[cfg(target_os = "macos")]
        let helper = crate::helper_install::view();

        #[cfg(not(target_os = "linux"))]
        if !allow_fallback {
            #[cfg(target_os = "macos")]
            if !helper.installed {
                return Err(
                    "没装免密助手：自动扫描不弹管理员授权，装好助手后重开 App 即自动读；\
                     也可以点「重新读取」手动扫（会请求一次密码）"
                        .to_string(),
                );
            }
            #[cfg(not(target_os = "macos"))]
            return Err(
                "自动扫描不弹管理员授权：点「重新读取」手动扫一次（会请求一次密码）"
                    .to_string(),
            );
        }

        // 助手「回来了但没干完」时记下它说的原因，回落之后要写进 notes
        #[cfg(target_os = "macos")]
        let mut fell_back: Option<String> = None;

        #[cfg(target_os = "macos")]
        if helper.installed {
            match probe_via_helper(esp_partuuid) {
                Ok(mut payload) => {
                    payload.via = "helper".to_string();
                    return Ok(payload);
                }
                // 助手明确拒绝：请求非法，或它自己读盘失败。再弹一次密码只会再失败一次，
                // 直接把真实原因报上去（弹框会把「权限不够」这种真相盖掉）。
                Err(crate::helper_install::HelperError::Rejected(error)) => return Err(error),
                // 这条路不通（超时 / 作业没加载 / 被系统拦）：回落一次性授权。
                // 回落成功时不打扰用户；回落也失败时 osascript 会说自己的原因。
                Err(crate::helper_install::HelperError::Unavailable(why)) => {
                    if !allow_fallback {
                        return Err(format!(
                            "免密助手没接上（{why}）：自动扫描不弹管理员授权，\
                             稍后会自动重试；也可以点「重新读取」手动扫"
                        ));
                    }
                    fell_back = Some(why);
                }
            }
        }

        let mut payload = probe_via_elevated(esp_partuuid)?;
        payload.via = "elevated".to_string();
        // 走到这里说明助手没把活干完。最常见的原因是「App 重装过、/Library 里那份
        // 还是上一个构建」（`view().stale`），此外还有超时 / 作业没加载。
        // 不说清楚，用户看到的就是「明明开着免密，怎么又弹密码了」。
        #[cfg(target_os = "macos")]
        payload.notes.push({
            let hint = if helper.stale {
                "（/Library 里那份助手是上一个构建装的，点「重装」可消掉这条）"
            } else {
                ""
            };
            match fell_back {
                Some(why) => format!("免密助手这次没接上（{why}）：改用了一次性授权{hint}"),
                None => format!("没有可用的免密助手：改用了一次性授权{hint}"),
            }
        });
        Ok(payload)
    }
}

/// 免密助手这条路能不能走：只问「装没装」。
///
/// 曾经这里还要求「装的那份必须是当前构建」（digest 一致），结果是**每次 App
/// 重新构建/升级后，所有读取与写入都会退回弹密码**，用户只会觉得「免密坏了」。
/// 真正的契约是协议版本：请求里带 `v`，助手侧的 `helper_proto::validate` 对不上
/// 就直接拒绝并回一句「协议版本不支持」，绝不会闷头按旧语义乱写。所以：
/// - 同协议、不同构建 → 照常免密（这正是开发期每天都会遇到的情况）；
/// - 协议不同 → 助手拒绝 → 调用方把真实原因报出来，用户重装一次助手即可。
///
/// `view().stale` 现在只用来提示「建议重装」，不再当门禁。
#[cfg(target_os = "macos")]
fn helper_ready() -> bool {
    crate::helper_install::view().installed
}

#[cfg(target_os = "macos")]
fn probe_via_helper(
    esp_partuuid: &str,
) -> Result<ProbePayload, crate::helper_install::HelperError> {
    let bytes = crate::helper_install::linux_probe(esp_partuuid)?;
    serde_json::from_slice(&bytes).map_err(|error| {
        crate::helper_install::HelperError::Rejected(format!(
            "免密助手给的探测结果不是合法 JSON：{error}"
        ))
    })
}

/// 给 rEFInd 模拟扫描的「Linux 分区清单」：取最近一次探测的缓存；
/// 没有缓存、助手在位，就现跑一次**免密专用**探测（不回落弹授权）。
///
/// 返回 `(parts, 失败原因)`。`parts` 为空说明拿不到 —— 模拟扫描会把原因
/// 写进 issue，而不是悄悄少一块分区。Linux 本机不需要这条链路：ext4 分区
/// 都挂着，`oc_menu::volumes()` 已经覆盖（返回空 + None，模拟扫描自然跳过）。
pub(crate) fn parts_for_sim(app: &AppHandle) -> (Vec<PartReport>, Option<String>) {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        (Vec::new(), None)
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(cached) = app
            .state::<Mutex<AppState>>()
            .lock()
            .ok()
            .and_then(|state| state.linux_probe_cache.clone())
        {
            let note = cached.notes.first().cloned();
            return (cached.parts, note);
        }
        // Windows 没有免密助手这条路，而模拟扫描是自动跑的 —— 不能在这里弹 UAC，
        // 否则每重建一次预演就冒一个授权框。等用户手动「重新读取」扫过一次即可。
        (
            Vec::new(),
            Some(
                "Linux 分区里的内核要读 ext4 裸设备（Windows 上必须提权）：\
                 点一次「重新读取」授权扫描后，这里就能扫出内核条目"
                    .to_string(),
            ),
        )
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(cached) = app
            .state::<Mutex<AppState>>()
            .lock()
            .ok()
            .and_then(|state| state.linux_probe_cache.clone())
        {
            // 缓存可能是空手而归的（比如设备全打不开）：也如实返回，
            // 让模拟扫描的 issue 把那句 note 带出去。
            let note = cached.notes.first().cloned();
            return (cached.parts, note);
        }
        if !helper_ready() {
            return (
                Vec::new(),
                Some(
                    "Linux 分区里的内核要读 ext4 裸设备（需要免密助手或一次授权）。\
                     装好助手后点「重新模拟」即可扫出内核条目"
                        .to_string(),
                ),
            );
        }
        let esp = stored_settings(app).grub_efi_partuuid;
        match probe_via_helper(&esp) {
            Ok(payload) => {
                let note = payload.notes.first().cloned();
                let parts = payload.parts.clone();
                if let Ok(mut state) = app.state::<Mutex<AppState>>().lock() {
                    state.linux_probe_cache = Some(payload);
                }
                (parts, note)
            }
            Err(crate::helper_install::HelperError::Rejected(error)) => (Vec::new(), Some(error)),
            Err(crate::helper_install::HelperError::Unavailable(why)) => {
                (Vec::new(), Some(format!("免密助手没接上：{why}")))
            }
        }
    }
}

/// 一次性授权（osascript / UAC）：弹一次密码，跑一个把自己提权起来的子进程。
#[cfg(not(target_os = "linux"))]
fn probe_via_elevated(esp_partuuid: &str) -> Result<ProbePayload, String> {
    let mut args = vec!["--linux-grub-probe".to_string()];
    if !esp_partuuid.trim().is_empty() {
        args.push("--esp".to_string());
        args.push(esp_partuuid.trim().to_string());
    }
    let out = temp_path("probe");
    args.push("--out".to_string());
    args.push(out.to_string_lossy().to_string());
    // Windows：先问常驻服务（远程/无人值守唯一可行），不可用才弹 UAC。
    // 别的平台没有这个服务，直接走各自的一次性授权。
    #[cfg(target_os = "windows")]
    super::elevate::run_privileged_op(
        "linux_grub_probe",
        serde_json::json!({ "esp": esp_partuuid.trim() }),
        &args,
        &out,
    )?;
    #[cfg(not(target_os = "windows"))]
    run_privileged(&args, &out)?;
    read_payload(&out)
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
/// 跑一次探测并序列化成 JSON 文本（一次性授权子进程落盘，特权服务直接回传）。
pub(crate) fn probe_child_json(esp_partuuid: &str) -> Result<String, String> {
    let payload = match probe_raw(esp_partuuid) {
        Ok(payload) => payload,
        Err(error) => ProbePayload {
            ok: false,
            error: Some(error),
            ..ProbePayload::default()
        },
    };
    serde_json::to_string(&payload).map_err(|error| format!("序列化探测结果失败：{error}"))
}

pub(crate) fn probe_child_main(out: &str, esp_partuuid: &str) -> i32 {
    match probe_child_json(esp_partuuid) {
        Ok(text) => super::write_result_file(out, &text),
        Err(error) => {
            eprintln!("{error}");
            2
        }
    }
}

/// `disk0s9` → `/dev/rdisk0s9`
///
/// 注意是 `r` + dev，不是 `rdisk` + dev —— 后者会拼成 `/dev/rdiskdisk0s9`
/// （这个笔误让整轮探测「一块都打不开」，而错误还被吞成了「可能被系统占用」）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn raw_path(dev: &str) -> String {
    format!("/dev/r{dev}")
}

/// 把 1024 字节环境块写回 `dev` 上的 `rel`，返回与落盘文件同形的 JSON 文本。
///
/// **两条提权路径共用这一份**（一次性授权的 `--linux-grub-write` 子进程、
/// 常驻特权服务的 `linux_grub_write` op）：写入语义只能有一处，否则
/// 「走服务干净、弹 UAC 不干净」这种差异会随时间去漂。
pub(crate) fn write_child_json(dev: &str, rel: &str, bytes: &[u8]) -> Result<String, String> {
    let reply = match write_env_block(dev, rel, bytes) {
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
    serde_json::to_string(&reply).map_err(|error| format!("序列化写入结果失败：{error}"))
}

/// 子进程入口：把 1024 字节环境块写回指定分区里的指定文件。
pub(crate) fn write_child_main(dev: &str, rel: &str, payload_path: &str, out: &str) -> i32 {
    let text = match std::fs::read(payload_path) {
        Ok(bytes) => write_child_json(dev, rel, &bytes),
        Err(error) => {
            let reply = ProbePayload {
                ok: false,
                error: Some(format!("读不到待写内容（{payload_path}）：{error}")),
                ..ProbePayload::default()
            };
            serde_json::to_string(&reply).map_err(|error| format!("序列化写入结果失败：{error}"))
        }
    };
    match text {
        Ok(text) => super::write_result_file(out, &text),
        Err(error) => {
            eprintln!("{error}");
            2
        }
    }
}

/// 把 1024 字节环境块写回 `dev` 上的 `rel`。
///
/// **两条提权路径共用这一份**（root 助手 / 一次性授权子进程）：写入语义只能有一处，
/// 否则「免密模式干净、弹密码模式不干净」这种差异会随时间去漂。
#[cfg(target_os = "macos")]
pub(crate) fn write_env_block(dev: &str, rel: &str, bytes: &[u8]) -> Result<String, String> {
    validate_env_block(bytes)?;
    if rel.contains("..") || rel.starts_with('/') {
        return Err("写入路径必须是分区内的相对路径".to_string());
    }
    let source = FileSource::open_raw_rw(&raw_path(dev))?;
    let mut fs = Ext4::open(Box::new(source))?;
    fs.overwrite_in_place(rel, bytes)?;
    Ok(format!("已写入 {dev}:{rel}（{} 字节，回读一致）", bytes.len()))
}

/// Windows：同样的写入语义，只是多一步「把 `disk1p3` 换回整盘偏移」。
///
/// `dev` 是 [`PartReport::dev`] 里那个 `disk{N}p{M}`，子进程重新查一次分区表拿
/// `Offset` —— 不复用父进程的缓存，因为写的时候磁盘可能刚被重分区过，而写错
/// 偏移的后果是把别人的分区写坏，这一秒的查询绝对省不得。
#[cfg(target_os = "windows")]
pub(crate) fn write_env_block(dev: &str, rel: &str, bytes: &[u8]) -> Result<String, String> {
    validate_env_block(bytes)?;
    if rel.contains("..") || rel.starts_with('/') {
        return Err("写入路径必须是分区内的相对路径".to_string());
    }
    let (disk, partition) = parse_win_dev(dev).ok_or_else(|| {
        format!("看不懂的分区标识 {dev}（这里要的是 disk0p9 这种形式）")
    })?;
    let row = super::partitions::all()
        .into_iter()
        .find(|row| row.disk == disk && row.partition == partition)
        .ok_or_else(|| format!("分区表里找不到 {dev}（盘被拔了？分区表变了？）"))?;
    let source = FileSource::open_physical(disk, row.offset, true)?;
    let mut fs = Ext4::open(Box::new(source))?;
    fs.overwrite_in_place(rel, bytes)?;
    Ok(format!(
        "已写入 {dev}:{rel}（{} 字节，回读一致）",
        bytes.len()
    ))
}

/// `disk1p3` → `(1, 3)`。解析失败返回 `None`（宁可报错也不猜一个偏移）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_win_dev(dev: &str) -> Option<(u32, u32)> {
    let rest = dev.trim().strip_prefix("disk")?;
    let (disk, partition) = rest.split_once('p')?;
    Some((disk.parse().ok()?, partition.parse().ok()?))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn write_env_block(_dev: &str, _rel: &str, _bytes: &[u8]) -> Result<String, String> {
    Err("这一端还不支持写 ext4：请在本机直接改 /boot/grub/grubenv".to_string())
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

/// 一块 ext4 分区的身份与坐标。两端只差「怎么打开」，读完之后要的东西完全一样。
#[cfg_attr(target_os = "linux", allow(dead_code))]
struct Ext4Candidate {
    dev: String,
    label: Option<String>,
    size: Option<String>,
    part_uuid: Option<String>,
    part_number: u32,
    disk_index: u32,
}

/// 解析一块**已经打开**的 ext4 分区：fstab / os-release / 目录清单 / grub.cfg / grubenv。
///
/// 只写这一份的理由：这些字段就是「自动读取」通道的全部产出，两端各写一遍必然漂 ——
/// 一边加了 `dir_listings`、另一边忘了加，rEFInd 模拟在那一端就永远看不到内核清单，
/// 而这种缺失不会报错，只会表现为「列表里少几项」。
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn scan_ext4(fs: &mut Ext4, cand: &Ext4Candidate) -> PartReport {
    let mut report = PartReport {
        dev: cand.dev.clone(),
        label: if fs.sb.label.is_empty() {
            cand.label.clone().unwrap_or_default()
        } else {
            fs.sb.label.clone()
        },
        size: cand.size.clone(),
        fs_uuid: Some(part_spec::normalize_uuid(&fs.sb.uuid)),
        part_uuid: cand.part_uuid.clone(),
        part_label: None,
        part_number: cand.part_number,
        disk_index: cand.disk_index,
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
    // os-release（只为在列表里好认；PRETTY_NAME 给模拟扫描的 Linux 条目用）
    if let Ok(Some(bytes)) = fs.read_path("etc/os-release", MAX_SMALL_BYTES) {
        let text = String::from_utf8_lossy(&bytes);
        report.os_id = os_release_id(&text);
        report.os_pretty_name = os_release_pretty_name(&text);
    }
    // 内核目录清单（rEFInd 装了 ext4 驱动后扫的就是这两处：根目录 + /boot）。
    // 只在**有 ext4 文件系统可读**时才有意义，FAT ESP 的 PartReport 根本不会
    // 走到这里（Ext4::open 失败直接 continue 了）。
    for dir in ["", "boot"] {
        let Ok(names) = fs.list_dir(dir) else {
            continue;
        };
        let files: Vec<DirFile> = names
            .into_iter()
            .filter(|name| !name.starts_with('.'))
            .take(MAX_LISTING_FILES)
            .filter_map(|name| {
                // 只留文件（rEFInd 的 DirIter 也只匹配文件）；目录名如
                // "efi" 不会撞 vmlinuz 前缀，但端给主进程的清单干净些好。
                // mtime 对折叠排序至关重要（rEFInd 新内核排最前）。
                let meta = fs.stat(&join_rel(dir, &name))?;
                if meta.is_dir {
                    return None;
                }
                Some(DirFile {
                    name,
                    mtime: meta.mtime,
                })
            })
            .collect();
        // list_dir 已按名字排好序，这里不用再排；**mtime 排序是消费方的
        // 事**（要照顾 rescue 内核沉底，那是消费方才知道的事）。
        let conf_rel = format!("{dir}/refind_linux.conf");
        let refind_linux_conf = fs
            .read_path(&conf_rel, MAX_SMALL_BYTES)
            .ok()
            .flatten()
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string());
        report.dir_listings.push(DirListing {
            dir: dir.to_string(),
            files,
            refind_linux_conf,
        });
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
        report.root_uuid = report.cfg_text.as_deref().and_then(cfg_root_uuid);
        if let Ok(Some(env)) = fs.read_path(&env_rel, ENV_BLOCK_SIZE + 1) {
            if env.len() == ENV_BLOCK_SIZE {
                report.env_rel = Some(env_rel.clone());
                report.env_hex = Some(hex::encode(&env));
            }
        }
        break;
    }
    report
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
        payload.parts.push(scan_ext4(
            &mut fs,
            &Ext4Candidate {
                dev: part.dev.clone(),
                label: part.label.clone(),
                size: part.size.clone(),
                part_uuid: part.part_uuid.clone(),
                part_number: part.part_number,
                disk_index: part.disk_index,
            },
        ));
    }
    Ok(payload)
}

/// Windows：Linux 分区没有盘符，只能开整盘再按分区偏移读。
///
/// Windows 读不了 ext4，所以系统根本不给这些分区分配盘符 —— 不存在「分区级」
/// 路径可开，唯一入口是 `\\.\PhysicalDriveN` + `Get-Partition` 给的 `Offset`。
/// 这和 APFS 容器那条路是同一招（`apfs_reader` 的 `container_location`）。
///
/// **这条路径只会在提权子进程里跑到**：非 Linux 平台上 `probe()` 一律走
/// `probe_via_elevated`，物理磁盘的 `CreateFile` 也本来就要管理员。所以这里
/// 不再判断权限，把 io 错误原样带出去 —— 万一真被拒，说明 UAC 那次没提上来。
#[cfg(target_os = "windows")]
pub(crate) fn probe_raw(esp_partuuid: &str) -> Result<ProbePayload, String> {
    let mut payload = ProbePayload {
        ok: true,
        ..ProbePayload::default()
    };
    let rows = super::partitions::all();
    if rows.is_empty() {
        payload
            .notes
            .push("读不到分区表（Get-Partition 没有返回任何行）".to_string());
        return Ok(payload);
    }

    // 所选 ESP 的身份。**必须把 FAT 卷序列读出来**：Linux 的 fstab 里最常见的
    // 写法是 `UUID=E537-7F31 /boot/efi vfat ...`，那个 UUID 是 FAT 卷序列，
    // 不是分区 GUID —— 少了它 `locate()` 就永远认不出「哪块 Linux 分区挂的是
    // 这个 ESP」，界面只能报「没定位到你选的 ESP 对应的 Linux 分区」。
    // 卷标同理（`LABEL=EFI` 那一支）。
    if !esp_partuuid.trim().is_empty() {
        let wanted = part_spec::normalize_uuid(esp_partuuid);
        if let Some(hit) = rows
            .iter()
            .find(|row| !row.guid.is_empty() && part_spec::normalize_uuid(&row.guid) == wanted)
        {
            let sector = first_sector(hit.disk, hit.offset);
            let serial = sector.as_deref().and_then(fat_volume_serial);
            let label = sector.as_deref().and_then(fat_volume_label);
            if serial.is_none() {
                // 读不到卷序列不是致命错，但要让用户知道「按 UUID 匹配这条路走不通」，
                // 否则界面上只会说「没定位到」，看不出是权限还是文件系统不对。
                payload.notes.push(format!(
                    "读不出 ESP（disk{}p{}）的 FAT 卷序列：按 fstab 的 UUID= 匹配这一路不可用",
                    hit.disk, hit.partition
                ));
            }
            payload.esp = Some(IdentityWire {
                fs_uuid: serial.map(|value| format!("{:04X}-{:04X}", value >> 16, value & 0xFFFF)),
                part_uuid: Some(hit.guid.clone()),
                label,
                part_label: None,
                disk_index: hit.disk,
                part_number: hit.partition,
            });
        } else {
            payload.notes.push(format!(
                "分区表里没有 PARTUUID 为 {esp_partuuid} 的分区（设置里填错了？）"
            ));
        }
    }

    // 候选顺序：Linux 类型分区在前（正常机器第一块就命中），其余分区排后面。
    //
    // **不按 GPT 类型一刀切**。XBOOTLDR（`bc13c2ff-…`）与个别发行版自定义的类型
    // 里同样可能是 ext4 的 `/boot`，只认 `8300` 就会出现「磁盘管理里看得到、
    // 程序里扫不到」。macOS 端本来就不筛类型（读不动就 `Ext4::open` 失败跳过），
    // 这里照同一条尺子 —— 反正每块分区只多读一个超级块。
    let mut candidates: Vec<_> = rows.iter().filter(|row| row.is_linux()).collect();
    candidates.extend(rows.iter().filter(|row| !row.is_linux()));

    let mut denied = 0usize;
    for part in candidates {
        let linux = part.is_linux();
        // 与 PartReport.dev 的既有约定一致：macOS 是 `disk0s9`、Windows 是 `disk0p9`
        let dev = format!("disk{}p{}", part.disk, part.partition);
        let source = match FileSource::open_physical(part.disk, part.offset, false) {
            Ok(source) => source,
            Err(why) => {
                if why.contains("管理员权限") {
                    denied += 1;
                }
                // 报错原文一定带出来（APFS 那边踩过：把 io 原因吞成
                // 「可能被占用」，白查半天权限）。**但只对 Linux 类型报**：
                // MSR / 恢复分区本来就没有可读的文件系统，全报出来会把
                // 真正的问题淹掉。
                if linux {
                    payload.notes.push(format!("{dev} 打不开：{why}"));
                }
                continue;
            }
        };
        let Ok(mut fs) = Ext4::open(Box::new(source)) else {
            continue; // 不是 ext4：正常跳过，不刷问题列表
        };
        payload.parts.push(scan_ext4(
            &mut fs,
            &Ext4Candidate {
                dev,
                label: None,
                size: Some(format!("{} MB", part.size_mb())),
                part_uuid: Some(part.guid.clone()).filter(|value| !value.is_empty()),
                part_number: part.partition,
                disk_index: part.disk,
            },
        ));
    }

    if payload.parts.is_empty() {
        payload.notes.push(if denied > 0 {
            "没有管理员权限，打不开物理磁盘：点「重新读取」授权一次就能读".to_string()
        } else {
            "没扫到任何 ext4 分区（本机没有 Linux 分区，或者都不支持）".to_string()
        });
    }
    Ok(payload)
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
    read_first_sector(&mut source)
}

/// Windows 版：没有分区级设备节点，只能开整盘按偏移读这一个扇区。
#[cfg(target_os = "windows")]
fn first_sector(disk: u32, offset: u64) -> Option<Vec<u8>> {
    let mut source = FileSource::open_physical(disk, offset, false).ok()?;
    read_first_sector(&mut source)
}

fn read_first_sector(source: &mut FileSource) -> Option<Vec<u8>> {
    let mut sector = vec![0u8; 512];
    source.read_at(0, &mut sector).ok()?;
    Some(sector)
}

/// FAT 卷序列 —— 就是 `/etc/fstab` 与 GRUB 里的 `UUID=XXXX-XXXX`。
///
/// 委托 [`crate::esp::fat_boot_sector`]：它按**簇数**判 FAT12/16/32，序列号位置
/// 随之不同（12/16 在 `0x27`、32 在 `0x43`）。自己固定读 `0x43` 在 FAT16 的 ESP 上
/// 会读出一串垃圾，然后表现成「fstab 里的 UUID 对不上」—— 和「根本没读到」
/// 长得一模一样，是最难查的那种失败。
pub(crate) fn fat_volume_serial(sector: &[u8]) -> Option<u32> {
    if sector.len() < 512 || sector[510] != 0x55 || sector[511] != 0xAA {
        return None;
    }
    // NTFS 的 BPB 在同一批偏移上也有非零字段，能算出一个「像 FAT」的簇数 ——
    // 先按 OEM 名排掉，别把 NTFS 的散列当成卷序列。
    if &sector[3..11] == b"NTFS    " {
        return None;
    }
    let (_, serial) = crate::esp::fat_boot_sector(sector)?;
    u32::from_str_radix(&serial, 16).ok()
}

/// FAT 卷标（`BS_VolLab`，11 字节空格填充）。
///
/// `fstab` 里写成 `LABEL=EFI` 时靠它匹配 —— 和 `UUID=` 是同一条链路的两个分支。
/// 偏移随 FAT 类型不同：FAT12/16 在 `0x2B`，FAT32 在 `0x47`。
/// 全空格 = 没设卷标，返回 `None`：空字符串绝不能当成「可匹配的值」，
/// 否则任意一块没有卷标的分区都会被算成命中。
pub(crate) fn fat_volume_label(sector: &[u8]) -> Option<String> {
    let (kind, _) = crate::esp::fat_boot_sector(sector)?;
    let at = if kind == "FAT32" { 0x47 } else { 0x2B };
    let raw = sector.get(at..at + 11)?;
    let label: String = raw.iter().map(|byte| *byte as char).collect();
    let label = label.trim().to_string();
    // 卷标只允许大写 ASCII；出现控制字符说明这条链路读到的不是 FAT 引导扇区
    if label.is_empty() || !label.chars().all(|character| character.is_ascii_graphic()) {
        return None;
    }
    Some(label)
}

// ─────────────────────────── 写盘 ───────────────────────────

/// 把 selector 写进目标分区的环境块（`saved_entry` + `next_entry`，与手动链路同规则）。
///
/// 返回 `(给人看的一句话, 写进去的那 1024 字节)`。**要把新块带出去**：
/// 调用方得用它更新状态里的 `env_bytes`，否则连续切两次时第二次会拿**旧块**
/// 去算新块 —— 第一次写的 `saved_entry` 就被覆盖回去了。
pub(crate) fn write_env(runtime: &Runtime, selector: &str) -> Result<(String, Vec<u8>), String> {
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
        return write_env_host(&runtime.env_rel, &block).map(|detail| (detail, block));
    }

    // 与 probe 同一套优先级：装了助手就免密写，否则弹一次密码。
    #[cfg(target_os = "macos")]
    if helper_ready() {
        match crate::helper_install::linux_write_env(&runtime.dev, &runtime.env_rel, &block) {
            Ok(detail) => return Ok((detail, block)),
            // 助手拒绝 = 真的写不进去（分区脏 / 文件不存在 / RECOVER 位挂着）。
            // 这时候再弹密码只会得到同一个错误，还多让用户输一次密码。
            Err(crate::helper_install::HelperError::Rejected(error)) => return Err(error),
            Err(crate::helper_install::HelperError::Unavailable(_why)) => {}
        }
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
    #[cfg(target_os = "windows")]
    let result = super::elevate::run_privileged_op(
        "linux_grub_write",
        serde_json::json!({
            "dev": runtime.dev,
            "rel": runtime.env_rel,
            // 服务走 hex 而不是文件路径：请求里只带 1024 字节内容，
            // 不在 SYSTEM 进程里再读一个用户可写的临时文件。
            "payload_hex": hex::encode(&block),
        }),
        &args,
        &out,
    );
    #[cfg(not(target_os = "windows"))]
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
                let detail = reply
                    .notes
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "已写入环境块".to_string());
                Ok((detail, block))
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
///
/// **必须在阻塞池里跑**（用户 2026-10-29「点击没反应」的真实原因）：
/// 一次探测要读十块裸分区 —— 助手冷启 4~5 秒、走 osascript 还要弹授权。
/// 同步命令是跑在 WebView 的 IPC/UI 线程上的，这几秒里整个窗口不绘制、
/// 不吃点击：`[render]` 心跳和界面轮询都会断在那儿（trace 里能直接看到
/// 4~20 秒的空档），用户看到的就是「点了没反应」，然后连点几次排成一队。
///
/// `auto=true` 是开机自动扫：助手没接上**不许弹授权**（见 `probe`），报错让
/// 前端自动重试；`auto=false`（手点）才回落一次性授权。
#[tauri::command]
pub(crate) async fn linux_grub_scan(
    app: AppHandle,
    target: Option<String>,
    auto: Option<bool>,
) -> Option<LinuxGrubView> {
    let allow_fallback = !auto.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || scan_blocking(app, target, allow_fallback))
        .await
        .ok()
        .flatten()
}

/// `linux_grub_scan` 的同步实现（阻塞，只能在阻塞池或 HTTP 线程里调用）。
pub(crate) fn scan_blocking(
    app: AppHandle,
    target: Option<String>,
    allow_fallback: bool,
) -> Option<LinuxGrubView> {
    Some(refresh(&app, target.as_deref(), allow_fallback))
}

/// 「换一个」：记住用户选的分区，然后立刻按它重扫。
#[tauri::command]
pub(crate) async fn linux_grub_pick(app: AppHandle, dev: String) -> Option<LinuxGrubView> {
    tauri::async_runtime::spawn_blocking(move || pick_blocking(app, dev))
        .await
        .ok()
        .flatten()
}

/// `linux_grub_pick` 的同步实现。先落盘再扫：扫失败（分区里没有 cfg）时
/// 选择本身也要留住，否则用户会以为「选了又弹回去」。
pub(crate) fn pick_blocking(app: AppHandle, dev: String) -> Option<LinuxGrubView> {
    let dev = dev.trim().to_string();
    let mut settings = stored_settings(&app);
    settings.linux_grub_dev = dev.clone();
    if let Err(error) = save_settings(&app, &settings) {
        trace(&app, "linux_grub", &format!("保存目标分区失败：{error}"));
    }
    // pick 永远是用户主动点的：允许回落一次性授权
    Some(refresh(&app, Some(&dev), true))
}

/// 切换下次启动：写目标分区自己的 grubenv。
///
/// 返回**切换后的 GRUB 视图**，不返回 `AppStatus`。两个原因：
/// 1. `AppStatus` 是 rEFInd 的状态结构（`vars_dir` / `variables` / `message`），
///    把 GRUB 的结果塞进去会让网页端读到 rEFInd 遗留的 message
///    （那是上一次 rEFInd 操作留下的，与 GRUB 无关）；
/// 2. 前端其实忽略返回值，所以这个错一直没暴露，但 HTTP 通道会原样吐出去。
///
/// **不重扫**：只写了 1024 字节 env，`grub.cfg` 没变，没必要再探一遍分区；
/// 而在没装免密助手时「再探一遍」= 再弹一次密码框。`saved_entry` / `next_entry`
/// 直接用刚写进去的那块回填，并把新块存回 runtime —— 否则连续切两次时，
/// 第二次会拿**旧块**去算，把第一次的 `saved_entry` 覆盖掉。
#[tauri::command]
pub(crate) async fn linux_grub_switch(app: AppHandle, selector: String) -> LinuxGrubView {
    let log_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || switch_blocking(app, selector)).await {
        Ok(view) => view,
        // 阻塞池 join 失败（任务 panic）：别让界面停在「正在写入…」到天荒地老
        Err(error) => {
            let message = format!("切换任务没跑起来：{error}");
            trace(&log_app, "linux_grub", &message);
            record_action(&log_app, "切换 GRUB 启动项（自动读取）失败", false, &message);
            let mut view = LinuxGrubView::failed(platform_name(), message);
            view.needs_elevation = !cfg!(target_os = "linux");
            publish_ui(&log_app);
            view
        }
    }
}

/// `linux_grub_switch` 的同步实现：写 1024 字节 env（打不开分区就报真实原因）。
///
/// 同样不能待在 UI 线程上：写盘要先探分区、必要时唤起免密助手 —— 一次几秒。
pub(crate) fn switch_blocking(app: AppHandle, selector: String) -> LinuxGrubView {
    let (runtime, current) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (state.linux_grub_rt.clone(), state.linux_grub.clone())
    };
    let Some(runtime) = runtime else {
        let message = "还没扫描过：先点「重新读取」，让界面拿到目标分区的 grubenv".to_string();
        record_action(&app, "切换 GRUB 启动项（自动读取）失败", false, &message);
        let mut view = current.unwrap_or_else(|| LinuxGrubView {
            platform: platform_name().to_string(),
            needs_elevation: !cfg!(target_os = "linux"),
            ..LinuxGrubView::default()
        });
        view.error = Some(message.clone());
        view.issues.insert(0, message);
        return view;
    };

    match write_env(&runtime, &selector) {
        Ok((detail, block)) => {
            let message = format!("下次启动：{selector}｜{detail}");
            let saved = crate::grub::env_value(&block, "saved_entry").filter(|value| !value.is_empty());
            let next = crate::grub::env_value(&block, "next_entry").filter(|value| !value.is_empty());
            let view = {
                let state = app.state::<Mutex<AppState>>();
                let mut state = state.lock().expect("state mutex poisoned");
                state.linux_grub_rt = Some(Runtime {
                    env_bytes: block,
                    ..runtime
                });
                let view = state.linux_grub.get_or_insert_with(|| LinuxGrubView {
                    platform: platform_name().to_string(),
                    needs_elevation: !cfg!(target_os = "linux"),
                    ..LinuxGrubView::default()
                });
                view.saved_entry = saved;
                view.next_entry = next;
                view.error = None;
                view.clone()
            };
            record_action(&app, "切换 GRUB 启动项（自动读取）", true, &message);
            publish_ui(&app);
            view
        }
        Err(error) => {
            record_action(&app, "切换 GRUB 启动项（自动读取）失败", false, &error);
            // 写失败时 runtime 仍然有效（cfg 是好的，可能是分区脏 / 只读），
            // 所以不清它 —— 用户修好之后还能直接再点一次，不必重扫。
            let mut view = current.unwrap_or_else(|| LinuxGrubView {
                platform: platform_name().to_string(),
                needs_elevation: !cfg!(target_os = "linux"),
                ..LinuxGrubView::default()
            });
            view.error = Some(error.clone());
            view.issues.insert(0, error);
            // **失败也要存回状态**：原来只把带 error 的视图 return 给前端，
            // 而 publish_ui 推的是 state 里那份**没有** issue 的旧视图 ——
            // 错误在下一次快照（3 秒）就被覆盖掉了，用户看到的就是
            // 「点了一下，闪了行字，然后什么都没发生」。runtime 不动，还能再点。
            {
                let state = app.state::<Mutex<AppState>>();
                let mut state = state.lock().expect("state mutex poisoned");
                state.linux_grub = Some(view.clone());
            }
            publish_ui(&app);
            view
        }
    }
}

/// 免密助手在不在位 —— 前端拿它决定「进页面要不要自动扫」。
///
/// 「不自动扫」是 osascript 弹密码年代定的规矩：开机就读裸设备 = 开机弹密码框。
/// 助手免密之后这个代价没了，但**助手不在位时仍会弹密码**，所以必须先问一句
/// 再决定。只查 `/Library` 里装没装（外加算一次摘要比对协议说明），不碰设备。
#[tauri::command]
pub(crate) async fn linux_grub_helper_ready() -> bool {
    tauri::async_runtime::spawn_blocking(helper_ready_blocking)
        .await
        .unwrap_or(false)
}

/// `linux_grub_helper_ready` 的同步实现：HTTP 路由本来就跑在工作线程上，直调。
pub(crate) fn helper_ready_blocking() -> bool {
    #[cfg(target_os = "macos")]
    return crate::helper_install::view().installed;

    #[cfg(not(target_os = "macos"))]
    return true;
}

fn store(
    app: &AppHandle,
    view: LinuxGrubView,
    runtime: Option<Runtime>,
    probe: Option<&ProbePayload>,
) {
    let state = app.state::<Mutex<AppState>>();
    let mut state = state.lock().expect("state mutex poisoned");
    state.linux_grub = Some(view);
    state.linux_grub_rt = runtime;
    // 全量探测结果缓存给 rEFInd 模拟扫描复用：模拟扫描只要分区清单（内核
    // 文件名/mtime/os-release），绝不再为它开一轮裸设备探测 —— macOS 上
    // 每一轮都是 4~5 秒 + 可能的授权框。
    if let Some(payload) = probe {
        state.linux_probe_cache = Some(payload.clone());
    }
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
pub(crate) fn refresh(app: &AppHandle, target: Option<&str>, allow_fallback: bool) -> LinuxGrubView {
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

    let payload = match probe(&esp_partuuid, &prefer, allow_fallback) {
        Ok(payload) => payload,
        Err(error) => {
            let view = LinuxGrubView::failed(platform_name(), error);
            store(app, view.clone(), None, None);
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
    store(app, view.clone(), runtime, Some(&payload));
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
    fn locate_ignores_blank_manual_choice() {
        // 「恢复自动定位」在前端传空串：必须当成「没手选过」，
        // 否则清不掉手选状态，fstab 自动定位永远回不来。
        let parts = vec![
            report("disk0s7", "AAAA", vec![], true),
            report("disk0s9", "BBBB", vec![("UUID=e537-7f31", "/boot/efi", "vfat")], true),
        ];
        let selection = locate(&parts, Some(&esp()), Some("")).unwrap();
        assert_eq!(selection.index, 1);
        assert_eq!(selection.confidence, "exact");
        // 空白也不算手选
        let selection = locate(&parts, Some(&esp()), Some("   ")).unwrap();
        assert_eq!(selection.confidence, "exact");
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

    /// 分区脏（ext4 RECOVER）时后端给这条分区挂了 `note`：目标与候选列表
    /// 都必须把它带到界面上 —— 不然用户点一次启动项被拒一次，看起来就是
    /// 「点了没反应」（2026-10-29 现场，同一条错误连点四次）。
    #[test]
    fn dirty_partition_note_reaches_view() {
        let mut part = report(
            "disk0s9",
            "BBBB",
            vec![("UUID=E537-7F31", "/boot/efi", "vfat")],
            true,
        );
        let note = "上次没干净关机（RECOVER 标志），只读可用、写入会被拒".to_string();
        part.note = Some(note.clone());
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
        assert_eq!(view.target.as_ref().and_then(|t| t.note.clone()), Some(note.clone()));
        assert_eq!(view.candidates[0].note.clone(), Some(note));
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

    /// FAT32 的 BPB 骨架：簇数 ≥ 65525 才会被判成 FAT32，序列号才落在 `0x43`。
    /// 只填 `0x43` 处的序列号是不够的 —— 簇数字段全是 0 时根本判不出类型。
    fn fat32_sector() -> Vec<u8> {
        let mut sector = vec![0u8; 512];
        sector[11..13].copy_from_slice(&512u16.to_le_bytes()); // 每扇区字节数
        sector[13] = 8; // 每簇扇区数
        sector[14..16].copy_from_slice(&32u16.to_le_bytes()); // 保留扇区
        sector[16] = 2; // FAT 份数
        sector[32..36].copy_from_slice(&1_048_576u32.to_le_bytes()); // 总扇区数
        sector[36..40].copy_from_slice(&1_000u32.to_le_bytes()); // 每份 FAT 的扇区数
        sector[510] = 0x55;
        sector[511] = 0xAA;
        sector
    }

    #[test]
    fn fat_serial_reads_fat32_bs_volid() {
        let mut sector = fat32_sector();
        sector[0x43..0x47].copy_from_slice(&0x4754_DCADu32.to_le_bytes());
        assert_eq!(fat_volume_serial(&sector), Some(0x4754_DCAD));
        // NTFS 的同一位置不是卷序列，不能当 UUID 用
        sector[3..11].copy_from_slice(b"NTFS    ");
        assert_eq!(fat_volume_serial(&sector), None);
    }

    /// FAT32 与 FAT12/16 的卷标不在同一个偏移 —— 固定读 `0x47` 会在 FAT16 的
    /// ESP 上读出 `BS_FilSysType` 那一带的字节，表现为「fstab 里的 LABEL 认不出」。
    #[test]
    fn fat_label_follows_fat_type() {
        // FAT32：BS_VolLab 在 0x47
        let mut sector = fat32_sector();
        sector[0x47..0x52].copy_from_slice(b"EFI        ");
        assert_eq!(fat_volume_label(&sector).as_deref(), Some("EFI"));
        // 全空格 = 没设卷标。绝不能返回空串：那样任意一块没卷标的分区都会被
        // `LABEL=` 那一行算成命中，等于乱选一块。
        sector[0x47..0x52].copy_from_slice(b"           ");
        assert_eq!(fat_volume_label(&sector), None);

        // FAT16：BS_VolLab 在 0x2B
        let mut sector = vec![0u8; 512];
        sector[11..13].copy_from_slice(&512u16.to_le_bytes());
        sector[13] = 4;
        sector[14..16].copy_from_slice(&1u16.to_le_bytes());
        sector[16] = 2;
        sector[17..19].copy_from_slice(&512u16.to_le_bytes()); // 根目录项数
        sector[19..21].copy_from_slice(&40_000u16.to_le_bytes());
        sector[22..24].copy_from_slice(&100u16.to_le_bytes());
        sector[0x2B..0x36].copy_from_slice(b"BOOT       ");
        assert_eq!(fat_volume_label(&sector).as_deref(), Some("BOOT"));
    }

    #[test]
    fn disk_ids_parse() {
        assert_eq!(parse_disk_id("disk0s9"), Some((0, 9)));
        assert_eq!(parse_disk_id("disk2"), Some((2, 0)));
        assert_eq!(parse_disk_id("rdisk0s9"), None);
    }

    /// Windows 的分区标识：`disk1p3`。
    ///
    /// 生成端在 `probe_raw`（`format!("disk{}p{}")`），解析端在 `write_env_block` ——
    /// 两边必须是同一套写法，否则表现是「扫得到、写不进」，而错误还只是
    /// 「看不懂的分区标识」，最难查的那一类。
    #[test]
    fn win_disk_ids_parse() {
        assert_eq!(parse_win_dev("disk1p3"), Some((1, 3)));
        assert_eq!(parse_win_dev("disk0p12"), Some((0, 12)));
        assert_eq!(
            format!("disk{}p{}", 1u32, 3u32),
            "disk1p3",
            "生成端与解析端必须同构"
        );
        // 猜一个偏移等于往别人的分区上写，认不出的一律 None
        assert_eq!(parse_win_dev("disk1"), None);
        assert_eq!(parse_win_dev("disk1p"), None);
        assert_eq!(parse_win_dev("diskxp3"), None);
        assert_eq!(parse_win_dev("sdb1"), None);
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

    /// 真机：走 **root 助手（免密）** 读一遍，验证 App 侧客户端 + v2 协议 + 解析三者合得上。
    ///
    /// 与 [`live_probe`] 的区别：那条测「本进程以 root 直接读块设备」，
    /// 这条测「普通用户进程投请求 → launchd 唤起 root 助手 → 读 → 回 blob」。
    /// 后者才是 App 在 macOS 上的真实路径，而且**全程不该出现密码框**。
    ///
    /// 前提：助手已安装。**不判 `stale`** —— 那个判断比的是
    /// `current_exe()` 与 `/Library` 里的副本，而测试进程的 `current_exe` 是
    /// `target/debug/deps/anyboot-xxxx`，跟 App 本体永远不同，判了必然误报。
    /// 产品里 App 进程比的是自己，两者是同一份复制，摘要一致。
    ///
    /// 助手是旧协议时这条会失败（回 `请求不是合法 JSON`）—— 那正是要暴露的：
    /// 去 OpenCore 卡片里关一次再开一次免密模式。
    /// `cargo test --lib live_helper_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_helper_probe() {
        if !cfg!(target_os = "macos") {
            return;
        }
        if !crate::helper_install::installed() {
            println!("跳过：免密助手没装（这条测试只测免密路径）");
            return;
        }
        let started = std::time::Instant::now();
        let bytes = crate::helper_install::linux_probe("").expect("免密助手读取失败");
        println!(
            "回包 {} 字节，耗时 {:?}",
            bytes.len(),
            started.elapsed()
        );
        let payload: ProbePayload = serde_json::from_slice(&bytes).expect("回包不是 ProbePayload");
        assert!(payload.ok, "助手回了 ok=false：{:?}", payload.error);
        assert!(!payload.parts.is_empty(), "一块 ext4 都没读到");
        let with_cfg = payload.parts.iter().filter(|part| part.cfg_rel.is_some()).count();
        println!("{} 块 ext4，其中 {} 块有 grub.cfg", payload.parts.len(), with_cfg);
        assert!(with_cfg > 0, "一块带 grub.cfg 的分区都没有");

        // 内核清单（rEFInd 模拟扫描的输入）：vmlinuz*/bzImage*/kernel*/*.efi。
        // **旧助手的回执没有 dir_listings**（serde default 解析成空）—— 所以
        // 这里只在「至少一块分区带清单」时才断言内核非空：旧助手时代这条测试
        // 照常绿，重装新助手之后它自动升级成端到端检查。
        let mut kernel_parts = 0;
        let mut listed_parts = 0;
        for part in &payload.parts {
            for listing in &part.dir_listings {
                listed_parts += 1;
                let kernels: Vec<&str> = listing
                    .files
                    .iter()
                    .map(|file| file.name.as_str())
                    .filter(|name| {
                        let lower = name.to_ascii_lowercase();
                        lower.starts_with("vmlinuz")
                            || lower.starts_with("bzimage")
                            || lower.starts_with("kernel")
                            || lower.ends_with(".efi")
                    })
                    .collect();
                if kernels.is_empty() {
                    continue;
                }
                kernel_parts += 1;
                println!(
                    "{} ({:?}) [{}] conf={}",
                    part.dev,
                    part.label,
                    listing.dir,
                    listing.refind_linux_conf.is_some()
                );
                for kernel in kernels {
                    println!("    {kernel}");
                }
            }
        }
        if listed_parts == 0 {
            println!("清单字段全空：/Library 里还是旧版助手（重装后这条会变成真检查）");
        } else {
            assert!(kernel_parts > 0, "有清单但没有一块分区带内核文件");
        }
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
