//! OpenCore 菜单预演（**纯读**：不写盘、不碰 NVRAM、不需要提权）。
//!
//! 为什么需要它：OC 的菜单是 UEFI 运行期在内存里算出来的一张链表，
//! `OcScanForBootEntries`（`BootEntryManagement.c:2103`）消费完就释放，OC 也没有
//! 任何「导出菜单」的接口。主系统里能拿到的是它的**输入**（卷、文件、NVRAM），
//! 不是**结果**。
//!
//! 所以这里按 OC 的规则复算一遍，规则来源逐条对应源码：
//!
//! | 规则 | 源码 |
//! |---|---|
//! | 卷能否入候选只看 `ScanPolicy` | `BootEntryManagement.c:1741` / `PolicyManagement.c:312` |
//! | `BootOrder` 里每条 `Boot####` 取设备路径、丢弃描述 | `:1224`、`:1262-1265` |
//! | 每卷 bless 顺序：blessed 文件 → blessed 文件夹 → 预定义路径表 | `:912`、`:241`、`:278`、`:943` |
//! | 预定义路径表只有 3 条（boot.efi / bootmgfw.efi / BOOTX64.EFI） | `OcAppleBootPolicyLib.c:74-78` |
//! | APFS Preboot 按容器内每个 UUID 试 `Preboot:\{UUID}\...\boot.efi` | 同文件 `:987-1053` |
//! | `OpenLinuxBoot` 读 `\loader\entries\*.conf`，没有则 autodetect `vmlinuz*` | `OpenLinuxBoot.c:234`、`:251` |
//! | 名字：`.contentDetails` → `.disk_label.contentDetails` → 卷标 | `BootEntryInfo.c:728/730/774` |
//! | `HideAuxiliary` 丢弃 Recovery / TimeMachine / auxiliary | `:460`、`:472`、`:497` |
//! | 默认项 = BootOrder 里第一条注册成功的非工具项 | `:363`、`:380` |
//!
//! **已知误差**（不改代码也能说清的不准）：固件到底枚举得到哪些卷（APFS 驱动没加载、
//! USB 未初始化）、卷的枚举顺序、`Misc→Entries` 自定义条目、协议驱动的内部语义。
//! 要 100% 只能让 OC 自己把菜单写进日志（见 `docs/boot-menu-discovery.md` 第 8 节）。

use super::*;

/// 预定义路径表（顺序有意义：OC 每卷只取**第一个命中**的）。
///
/// 兜底的 `BOOT*.EFI` 要按架构各列一条：x64 机器上是 `BOOTX64.EFI`，ARM64 上是
/// `BOOTAA64.EFI`（OC 自己的 `BOOTAA64.EFI`、Windows on ARM 的引导器都在这个位置）。
/// 排在同一档、x64 在前：某卷上两个都在时行为与以前完全一致。
pub(crate) const PREDEFINED: [(&str, &str); 4] = [
    ("System/Library/CoreServices/boot.efi", "macos"),
    ("EFI/Microsoft/Boot/bootmgfw.efi", "windows"),
    ("EFI/BOOT/BOOTX64.EFI", "generic"),
    ("EFI/BOOT/BOOTAA64.EFI", "generic"),
];

/// 一条预演出来的菜单项。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MenuEntry {
    pub(crate) title: String,
    /// macos / windows / generic / linux / tool
    pub(crate) kind: String,
    /// bootorder / bless / preboot / protocol
    pub(crate) source: String,
    /// 卷标（或挂载点名）
    pub(crate) volume: String,
    /// 分区内路径（`\EFI\...` 形式，跟 OC 日志一致）
    pub(crate) path: String,
    /// 会不会被 `Misc→Boot→HideAuxiliary` 藏掉
    pub(crate) auxiliary: bool,
    /// 是不是默认项（BootOrder 里第一条能用的）
    pub(crate) default: bool,
    pub(crate) note: Option<String>,
    /// 前端原样回传的值域（「设为默认」用，`opencore-default:<卷>\u{1}<路径>`）。
    /// `None` = 这一项不能设默认（协议驱动条目、工具项、已经是默认项……）。
    pub(crate) spec: Option<String>,
}

/// 预演结果（挂在 OpenCore 的 `ChannelView` 上一起下发）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MenuPreview {
    /// `HideAuxiliary=false` 时会显示出来的
    pub(crate) entries: Vec<MenuEntry>,
    /// 会被 `HideAuxiliary=true` 藏掉的
    pub(crate) hidden: Vec<MenuEntry>,
    pub(crate) issues: Vec<HealthIssue>,
    /// 输入摘要（扫了几个卷 / 读到几条 BootOrder / 关键配置值）
    pub(crate) summary: String,
}

// ─────────────────────────── EFI_LOAD_OPTION 解析 ───────────────────────────

/// `Boot####` 值 → (描述, 分区内文件路径)。
///
/// `value` 是 `read_boot_option` 返回的**已剥掉 efivarfs 属性头**的裸值。
/// 布局见 UEFI 2.x §3.1.3：`Attributes(4) | FilePathListLength(2) | 描述(UTF-16 NUL) | 设备路径`。
pub(crate) fn parse_load_option(value: &[u8]) -> Option<(String, String)> {
    if value.len() < 8 {
        return None;
    }
    let mut pos = 6usize;
    let mut units: Vec<u16> = Vec::new();
    while pos + 1 < value.len() {
        let unit = u16::from_le_bytes([value[pos], value[pos + 1]]);
        pos += 2;
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    let description = String::from_utf16_lossy(&units);
    let path = device_path_file(&value[pos..]).unwrap_or_default();
    Some((description, path))
}

/// 设备路径列表 → 第一个 `MEDIA_FILEPATH`（Type 4 / SubType 4）节点里的路径。
fn device_path_file(raw: &[u8]) -> Option<String> {
    let mut pos = 0usize;
    while pos + 4 <= raw.len() {
        let kind = raw[pos];
        let subtype = raw[pos + 1];
        let length = u16::from_le_bytes([raw[pos + 2], raw[pos + 3]]) as usize;
        if length < 4 || pos + length > raw.len() {
            return None;
        }
        // 0x7F = End of Device Path
        if kind == 0x7f {
            return None;
        }
        if kind == 0x04 && subtype == 0x04 {
            let units: Vec<u16> = raw[pos + 4..pos + length]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .take_while(|unit| *unit != 0)
                .collect();
            return Some(String::from_utf16_lossy(&units));
        }
        pos += length;
    }
    None
}

/// 分区内路径统一成 `\EFI\...`（OC 日志里的写法，也便于比对）。
pub(crate) fn normalized(path: &str) -> String {
    // 先统一分隔符再削前导 `\`：`/EFI/x` 与 `\EFI\x` 必须归一到同一个写法，
    // 否则「BootOrder 里的路径」和「卷扫描出的路径」永远对不上（单测钉住）。
    let body = path.trim().replace('/', "\\");
    let body = body.trim_start_matches('\\');
    format!("\\{body}")
}

/// 分区内路径（`\EFI\x.efi`）→ 挂载点下的相对路径（`EFI/x.efi`）。
///
/// `grub_rel` 只是 `join`，把带反斜杠的串丢给它会得到一个**不存在**的文件名
/// （真机踩到：WINDOWS-EFI 上明明有 bootmgfw.efi，却被判成「文件不存在」）。
pub(crate) fn relative_under_mount(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_string()
}

fn is_uuid_segment(segment: &str) -> bool {
    segment.len() == 36 && segment.chars().filter(|c| *c == '-').count() == 4
}

fn file_name_of(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// 去掉 UUID 段与 `Preboot` 段之后的路径（全小写、`\` 连接），用于「不同写法是不是同一个文件」。
fn canonical_path(path: &str) -> String {
    path.split(['\\', '/'])
        .filter(|segment| {
            !segment.is_empty()
                && !is_uuid_segment(segment)
                && !segment.eq_ignore_ascii_case("preboot")
        })
        .map(|segment| segment.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("\\")
}

/// 两条路径是不是指向**同一个引导文件**。
///
/// 实机上有两种写法指同一个东西：
/// * `Boot0080` 的路径是 `\2D8A2ED9-…\System\Library\CoreServices\boot.efi`（Preboot 写法，带 UUID）；
/// * 卷扫描得到的是 `\System\Library\CoreServices\boot.efi`（或带 `Preboot\{uuid}\` 前缀）。
///
/// 规则：**双方都带 UUID 时必须 UUID 相同**（否则是同一容器里的两个 macOS），
/// 只要有一方不带，就按去掉 UUID/`Preboot` 的规范路径比较。
pub(crate) fn same_target(left: &str, right: &str) -> bool {
    if left.eq_ignore_ascii_case(right) {
        return true;
    }
    let uuid_of = |path: &str| {
        path.split(['\\', '/'])
            .find(|segment| is_uuid_segment(segment))
            .map(|segment| segment.to_ascii_lowercase())
    };
    match (uuid_of(left), uuid_of(right)) {
        (Some(a), Some(b)) => a == b && file_name_of(left) == file_name_of(right),
        _ => canonical_path(left) == canonical_path(right),
    }
}

// ─────────────────────────── 卷枚举 ───────────────────────────

struct Volume {
    label: String,
    root: PathBuf,
}

/// 主系统里看得见的卷。
///
/// macOS 的根卷不在 `/Volumes` 下（那是 APFS 的 Data/System 合体），要单独加；
/// 其余平台 `candidate_roots` 已经覆盖了常见挂载点。
fn volumes() -> Vec<Volume> {
    let mut out: Vec<Volume> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        // 根卷不在 `/Volumes` 下（那是 APFS 的 System/Data 合体），单独加。
        // 卷标只能问 diskutil：`/.contentDetails` 在封存的系统卷上不存在，
        // 而它正是 OC 菜单里显示的那个名字（实测就是 "MAC"）。
        out.push(Volume {
            label: root_volume_label(),
            root: PathBuf::from("/"),
        });
    }
    for root in super::candidate_roots() {
        if !root.is_dir() {
            continue;
        }
        // 只有「带 EFI 目录」或「带 macOS 引导文件」的卷才可能被 OC 扫出条目：
        // UEFI 里没有 NTFS/ext4 驱动，OC 看不到它们的目录结构。
        let has_efi = grub_rel(&root, "EFI").is_dir();
        let has_macos = grub_rel(&root, "System/Library/CoreServices").is_dir();
        if !has_efi && !has_macos {
            continue;
        }
        if out.iter().any(|known| same_path(&known.root, &root)) {
            continue;
        }
        let fallback = root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| root.display().to_string());
        out.push(Volume {
            label: volume_label(&root, &fallback),
            root,
        });
    }
    out
}

#[cfg(target_os = "macos")]
pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    // `/Volumes/X` 与 `/` 不会重复，但 `/` 会被 read_dir 结果里的符号链接绕进来
    fs::canonicalize(left).ok() == fs::canonicalize(right).ok()
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    left == right
}

/// 卷标 → 挂载根（「设为默认」要按该分区自己造启动项时用）。
///
/// 重新枚举一遍而不是缓存：值域里只带了卷标，点击与渲染之间可能已经卸载/重挂。
pub(crate) fn volume_root(label: &str) -> Option<PathBuf> {
    volumes()
        .into_iter()
        .find(|volume| volume.label == label)
        .map(|volume| volume.root)
}

/// 卷标：OC 的取名顺序（`.contentDetails` → `.disk_label.contentDetails` → 卷标）。
///
/// 实测这两个隐藏文件在 OC 的 ESP 上都可能不存在，读不到就退回挂载点名。
fn volume_label(root: &Path, fallback: &str) -> String {
    for rel in [".contentDetails", ".disk_label/contentDetails"] {
        if let Ok(text) = fs::read_to_string(grub_rel(root, rel)) {
            let text = text.trim();
            if !text.is_empty() && text.len() < 80 {
                return text.to_string();
            }
        }
    }
    fallback.to_string()
}

/// 从 `diskutil info -plist` 的某台设备上取一个字符串字段。
#[cfg(target_os = "macos")]
fn diskutil_field(device: &str, key: &str) -> Option<String> {
    let output = command("diskutil")
        .args(["info", "-plist"])
        .arg(device)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    match plist_lite::read_scalar(&String::from_utf8_lossy(&output.stdout), &[key])? {
        plist_lite::PlistScalar::String(text) if !text.trim().is_empty() => {
            Some(text.trim().to_string())
        }
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn root_volume_label() -> String {
    // 根卷一定同时挂在 `/Volumes/<卷标>` 下（实测同一 dev），直接读那个目录名，
    // 比每次调 diskutil（~130ms）划算。读不到才落到 diskutil。
    use std::os::unix::fs::MetadataExt;
    if let Ok(root) = fs::metadata("/") {
        if let Ok(dir) = fs::read_dir("/Volumes") {
            for item in dir.flatten() {
                let Ok(meta) = fs::metadata(item.path()) else {
                    continue;
                };
                if meta.dev() == root.dev() {
                    if let Some(name) = item.file_name().to_str() {
                        if !name.is_empty() {
                            return name.to_string();
                        }
                    }
                }
            }
        }
    }
    diskutil_field("/", "VolumeName").unwrap_or_else(|| "启动卷".to_string())
}

// ─────────────────────────── 预演主流程 ───────────────────────────

/// 按 OC 的规则复算一遍菜单。只读 ESP 与系统卷。
pub(crate) fn preview(esp_root: &Path) -> MenuPreview {
    let config = fs::read_to_string(grub_rel(esp_root, "EFI/OC/config.plist")).unwrap_or_default();
    let hide_auxiliary = matches!(
        plist_lite::read_scalar(&config, &["Misc", "Boot", "HideAuxiliary"]),
        Some(plist_lite::PlistScalar::Bool(true))
    );
    let scan_policy = match plist_lite::read_scalar(&config, &["Kernel", "Quirks", "ScanPolicy"]) {
        Some(plist_lite::PlistScalar::Integer(value)) => value,
        _ => 0,
    };
    let drivers = plist_lite::read_string_array(&config, &["UEFI", "Drivers"]).unwrap_or_default();
    let loaded = |name: &str| {
        drivers
            .iter()
            .any(|driver| driver.to_ascii_lowercase().contains(name))
    };

    let volumes = volumes();
    let mut entries: Vec<MenuEntry> = Vec::new();
    let mut issues: Vec<HealthIssue> = Vec::new();

    // ⓪ BootOrder 先解析成表：它既是 OC 的第一来源，也是「默认项」的唯一依据，
    // Preboot 那步还要用它的 UUID 判断「哪条 Preboot 是当前系统」。
    //
    // 读的是**OC 实际会读的那一组变量**：`RequestBootVarRouting=true` 时 OC 用私有
    // 命名空间的 `OCBtOrder` / `OCBt####`（`OcGetBootOrder` 按 `BootVariableGuid` 选名字，
    // `DefaultEntryChoice.c:611-617`），固件全局 `BootOrder` 只是旁证。
    let namespace = oc_default::Namespace::from_config(&config);
    let mut order = namespace.read_order();
    let mut namespace_fallback = false;
    if order.is_empty() && namespace.custom {
        let global = nvram::read_boot_order();
        if !global.is_empty() {
            order = global;
            namespace_fallback = true;
        }
    }
    let boot_option = |number: u16| -> Option<Vec<u8>> {
        namespace.read_option(number).or_else(|| {
            if namespace_fallback {
                nvram::read_boot_option(number)
            } else {
                None
            }
        })
    };
    let order_items: Vec<(u16, String, String)> = order
        .iter()
        .filter_map(|number| {
            let value = boot_option(*number)?;
            let (description, file_path) = parse_load_option(&value)?;
            Some((*number, description, normalized(&file_path)))
        })
        .collect();
    let order_uuids: Vec<String> = order_items
        .iter()
        .filter_map(|(_, _, path)| {
            path.split(['\\', '/'])
                .find(|segment| is_uuid_segment(segment))
                .map(|segment| segment.to_ascii_uppercase())
        })
        .collect();

    // ① 每个卷上的 bless 扫描（每卷只取第一个命中，与 OC 一致）
    let mut all_roots: Vec<(&str, &PathBuf)> = Vec::new();
    for volume in &volumes {
        all_roots.push((volume.label.as_str(), &volume.root));
        for (relative, kind) in PREDEFINED {
            if !grub_rel(&volume.root, relative).is_file() {
                continue;
            }
            let path = normalized(relative);
            let note = match kind {
                // 这一条最容易被误会：它就是 OC 自己，选中会再进一次 OC 菜单。
                // 路径用命中的那一条（x64 是 BOOTX64.EFI，ARM64 是 BOOTAA64.EFI），别写死。
                "generic" if same_path(&volume.root, esp_root) => Some(format!(
                    "这就是 OpenCore 自己的 Bootstrap 入口（{relative}）：\
                     选中它只会再进一次 OC 菜单，不是启动别的系统"
                )),
                "generic" => Some(
                    "预定义路径里的兜底项（IsGeneric）：OC 不认它是哪个系统，点进去由它自己接管"
                        .to_string(),
                ),
                _ => None,
            };
            entries.push(MenuEntry {
                title: volume.label.clone(),
                kind: kind.to_string(),
                source: "bless".to_string(),
                volume: volume.label.clone(),
                path,
                auxiliary: false,
                default: false,
                note,
                spec: None,
            });
            break;
        }
    }

    // ② APFS Preboot：OC 对容器内每个卷 UUID 都试一次
    #[cfg(target_os = "macos")]
    {
        let primary = volumes
            .first()
            .map(|volume| volume.label.clone())
            .unwrap_or_default();
        preboot_entries(&mut entries, &order_uuids, &primary);
    }

    // ③ 协议驱动：OpenLinuxBoot 是真遍历目录的那个
    let linux_boot_loaded = loaded("openlinuxboot");
    if linux_boot_loaded {
        for volume in &volumes {
            for base in ["loader/entries", "boot/loader/entries"] {
                let Ok(dir) = fs::read_dir(grub_rel(&volume.root, base)) else {
                    continue;
                };
                for item in dir.flatten() {
                    let path = item.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("conf") {
                        continue;
                    }
                    let title = fs::read_to_string(&path)
                        .ok()
                        .and_then(|text| {
                            text.lines()
                                .find_map(|line| line.trim().strip_prefix("title ").map(str::to_string))
                        })
                        .unwrap_or_else(|| {
                            path.file_stem()
                                .map(|stem| stem.to_string_lossy().to_string())
                                .unwrap_or_default()
                        });
                    entries.push(MenuEntry {
                        title,
                        kind: "linux".to_string(),
                        source: "protocol".to_string(),
                        volume: volume.label.clone(),
                        path: normalized(&format!("{base}/{}", item.file_name().to_string_lossy())),
                        auxiliary: false,
                        default: false,
                        note: Some("OpenLinuxBoot.efi 从 loader/entries 读出来的".to_string()),
                        // 这类条目的设备路径里含 OC 自己的自定义节点，凭路径造不出来
                        spec: None,
                    });
                }
            }
        }
    }
    for (driver, title) in [
        ("resetnvramentry", "Reset NVRAM"),
        ("togglesipentry", "切换 SIP"),
    ] {
        if loaded(driver) {
            entries.push(MenuEntry {
                title: title.to_string(),
                kind: "tool".to_string(),
                source: "protocol".to_string(),
                volume: "—".to_string(),
                path: format!("{driver}.efi"),
                // OC 把工具项算 auxiliary（HideAuxiliary 时会收进二级菜单）
                auxiliary: true,
                default: false,
                note: Some("协议驱动提供的工具项，不是可启动的系统".to_string()),
                spec: None,
            });
        }
    }

    // ④ BootOrder 落条目（表已经在 ⓪ 解析好）
    let mut order_notes: Vec<String> = Vec::new();
    for (number, description, wanted) in &order_items {
        // 与卷扫描结果对上 → 只标来源与默认，不重复列一条。
        // 用 same_target 而不是字符串全等：Preboot 形态的路径差一个 UUID 前缀。
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| same_target(&entry.path, wanted))
        {
            existing.source = "bootorder".to_string();
            if description.trim().is_empty() {
                continue;
            }
            // OC 丢弃 BootOrder 的描述、自己重新取名；这里留一句对照，方便核实
            existing.note = Some(format!(
                "{}固件里的描述是「{}」，OC 会丢掉它、按路径重新取名",
                existing
                    .note
                    .as_ref()
                    .map(|note| format!("{note}；"))
                    .unwrap_or_default(),
                description
            ));
            continue;
        }
        // 对不上：卷没挂载 / 文件已被删 / 路径不在我们扫的卷里
        let exists_anywhere = volumes.iter().any(|volume| {
            grub_rel(&volume.root, relative_under_mount(wanted).as_str()).is_file()
        });
        // 同一容器里别的 macOS（Preboot 有、但本系统没挂载它的根卷）也会走到这里，
        // 那种情况下不算「死项」，只是这边看不到文件。
        entries.push(MenuEntry {
            title: if description.trim().is_empty() {
                format!("Boot{number:04X}")
            } else {
                description.clone()
            },
            kind: "unknown".to_string(),
            source: "bootorder".to_string(),
            volume: "—".to_string(),
            path: wanted.clone(),
            auxiliary: false,
            default: false,
            note: Some(if exists_anywhere {
                "固件里有这一项，但不在上面扫到的卷/路径里".to_string()
            } else {
                "固件里有这一项，但对应的文件在本系统看不到（卷没挂载，或已被删除）"
                    .to_string()
            }),
            // 卷标是「—」：既没挂载点也没有可抄的路径，设默认时给出的只会是一句报错
            spec: None,
        });
        if !exists_anywhere {
            order_notes.push(format!("Boot{number:04X}"));
        }
    }

    // ⑤ 默认项：BootOrder 里第一条能用的（OC 用它，与菜单顺序无关）
    if !order_items.is_empty() {
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.source == "bootorder" && !entry.default && entry.kind != "tool")
        {
            entry.default = true;
        }
    }

    // ⑥ 过滤：HideAuxiliary 只影响 auxiliary 项
    let (mut visible, mut hidden): (Vec<MenuEntry>, Vec<MenuEntry>) = entries
        .into_iter()
        .partition(|entry| !(hide_auxiliary && entry.auxiliary));
    // 去重：OC 按 **DevicePath 全等** 去重（DevicePath 里含 HardDrive 节点 = 卷），
    // 所以键必须带上卷 —— 只按路径会把「三个 ESP 各自的 \EFI\BOOT\BOOTX64.EFI」
    // 合并成一条（真机实测踩到：ARCH-EFI 的泛型项因此消失）。
    let mut seen: Vec<String> = Vec::new();
    visible.retain(|entry| {
        let key = format!(
            "{}\u{1}{}",
            entry.volume.to_ascii_lowercase(),
            entry.path.to_ascii_lowercase()
        );
        if seen.contains(&key) {
            return false;
        }
        seen.push(key);
        true
    });

    // ⑥' 「设为默认」的值域（= OC 菜单里的 Ctrl+Enter）。放在最后统一算：
    // 它依赖「是不是默认项」「source 是 BootOrder 还是 bless」，两者到这里才定。
    let writable = namespace.writable();
    for entry in visible.iter_mut().chain(hidden.iter_mut()) {
        entry.spec = oc_default::entry_spec(entry, writable, esp_root);
    }

    // ⑦ 诊断
    if namespace_fallback {
        issues.push(HealthIssue {
            severity: "info".to_string(),
            text: format!(
                "OC 私有命名空间里还没有 `{}`（OC 没在里面记过默认项），下面按固件 `BootOrder` 展示；\
                 在这里设一次默认项就会把它建起来",
                namespace.order_name()
            ),
        });
    }
    if !writable {
        issues.push(HealthIssue {
            severity: "info".to_string(),
            text: "不能在这里设默认项：OC 读的是固件全局 `BootOrder`（`UEFI→Quirks→RequestBootVarRouting=false`），\
                   而本系统不允许写 `Boot*` 变量。把 `UEFI→Quirks→RequestBootVarRouting` 改成 true 后，\
                   OC 会改用私有命名空间（`OCBtOrder` / `OCBt0080`），这个能力就会亮起来"
                .to_string(),
        });
    }
    if scan_policy != 0 {
        issues.push(HealthIssue {
            severity: "info".to_string(),
            text: format!(
                "`ScanPolicy={scan_policy}`（非 0）：置了 LOCK 位，某些卷/文件系统会被直接跳过，\
                 下面的预演按「全部放行」算，可能与实际菜单有出入"
            ),
        });
    }
    match nvram::boot_vars_view() {
        nvram::BootVarsView::Visible => {}
        // 权限不足 ≠ 被托管：给的修法不一样，文案必须分开
        nvram::BootVarsView::NeedElevation => issues.push(HealthIssue {
            severity: "info".to_string(),
            text: "当前进程没权限读 EFI 变量（`BootCurrent` 读不到）：固件启动列表在这里不完整，\
                   「默认项」标记仅供参考；以管理员身份 / root 重跑即可看到完整列表"
                .to_string(),
        }),
        nvram::BootVarsView::Hidden => issues.push(HealthIssue {
            severity: "info".to_string(),
            text: "本系统读不到 `BootCurrent`：固件的启动列表在这里不完整，「默认项」标记仅供参考"
                .to_string(),
        }),
    }
    if hide_auxiliary && !hidden.is_empty() {
        issues.push(HealthIssue {
            severity: "info".to_string(),
            text: format!(
                "`HideAuxiliary=true`（`Misc→Boot`）：下面 {} 项会被收进二级菜单，首屏看不到",
                hidden.len()
            ),
        });
    }
    if !order_notes.is_empty() {
        issues.push(HealthIssue {
            severity: "warn".to_string(),
            text: format!(
                "`BootOrder` 里有 {} 项找不到对应文件（{}）——它们进了菜单也是点不动的死项",
                order_notes.len(),
                order_notes.join("、")
            ),
        });
    }

    // rEFInd 不存在于 OC 菜单：这是最常见也最费解的困惑，单独点出来
    let refind_volumes: Vec<&str> = all_roots
        .iter()
        .filter(|(_, root)| {
            grub_rel(root, "EFI/refind/refind_x64.efi").is_file()
                || grub_rel(root, "EFI/BOOT/refind_x64.efi").is_file()
                || grub_rel(root, "EFI/refind/refind.conf").is_file()
        })
        .map(|(label, _)| *label)
        .collect();
    if !refind_volumes.is_empty()
        && !visible
            .iter()
            .any(|entry| entry.path.to_ascii_lowercase().contains("refind"))
    {
        issues.push(HealthIssue {
            severity: "warn".to_string(),
            text: format!(
                "rEFInd 在 {} 上，但不会出现在 OC 菜单里：OC 的预定义路径只有 `boot.efi` / \
                 `bootmgfw.efi` / `BOOTX64.EFI`，rEFInd 三条都不占。要它出现，把 `refind_x64.efi` \
                 复制一份到该卷的 `\\EFI\\BOOT\\BOOTX64.EFI`，或让固件 `BootOrder` 里有一条指向它",
                refind_volumes.join("、")
            ),
        });
    }

    // Linux 只被认成泛型项
    let has_linux_entry = visible.iter().any(|entry| entry.kind == "linux")
        || visible
            .iter()
            .any(|entry| entry.path.to_ascii_lowercase().contains("grub"));
    let linux_volumes: Vec<&str> = all_roots
        .iter()
        .filter(|(_, root)| {
            grub_rel(root, "EFI/BOOT/grubx64.efi").is_file()
                || grub_rel(root, "EFI/arch/grubx64.efi").is_file()
                || grub_rel(root, "grub/grub.cfg").is_file()
                || grub_rel(root, "boot/grub/grub.cfg").is_file()
        })
        .map(|(label, _)| *label)
        .collect();
    if !linux_volumes.is_empty() && !has_linux_entry {
        let driver_file = grub_rel(esp_root, "EFI/OC/Drivers/OpenLinuxBoot.efi").is_file();
        issues.push(HealthIssue {
            severity: "warn".to_string(),
            text: format!(
                "Linux 在 {} 上只会显示成一个泛型项（图标是问号，点进去才进 GRUB）——\
                 Linux 不在 OC 的预定义路径表里。要得到独立内核条目，把 `OpenLinuxBoot.efi` \
                 放进 `EFI/OC/Drivers/` 并加到 `UEFI→Drivers`；它读 `/loader/entries/*.conf`，\
                 没有就自动找 `vmlinuz`{}",
                linux_volumes.join("、"),
                if driver_file {
                    "（文件已在 Drivers 目录，只是没被 `UEFI→Drivers` 加载）"
                } else {
                    "（当前 Drivers 目录里还没有这个文件）"
                }
            ),
        });
    }

    let summary = format!(
        "扫了 {} 个卷 / {} {} 项 / ScanPolicy={} / HideAuxiliary={}{}",
        volumes.len(),
        order.len(),
        namespace.order_name(),
        scan_policy,
        if hide_auxiliary { "true" } else { "false" },
        if namespace.custom {
            "（OC 私有命名空间，RequestBootVarRouting=true）"
        } else {
            ""
        }
    );

    MenuPreview {
        entries: visible,
        hidden,
        issues,
        summary,
    }
}

/// APFS Preboot 里的引导文件（OC 对同一容器内每个卷 UUID 都会试一遍）。
///
/// 有一条要**与卷扫描出来的「启动卷」合并**，否则菜单里会出现两个一模一样的 macOS 图标。
/// 判断哪条是「当前系统」：只有一个 Preboot 目录时必然是它；有多个时用 BootOrder 里
/// 出现过的 UUID（默认启动项通常指向自己），都没有就取第一条 —— 全程不调外部命令。
#[cfg(target_os = "macos")]
fn preboot_entries(out: &mut Vec<MenuEntry>, order_uuids: &[String], primary_label: &str) {
    let base = Path::new("/System/Volumes/Preboot");
    let Ok(dir) = fs::read_dir(base) else {
        return;
    };
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for item in dir.flatten() {
        let uuid = item.file_name().to_string_lossy().to_string();
        // 只认 UUID 形态的目录（跳过 `cryptex1`、`Cryptexes`、`.` 之类）
        if !is_uuid_segment(&uuid) {
            continue;
        }
        if item.path().join("System/Library/CoreServices/boot.efi").is_file() {
            found.push((uuid, item.path()));
        }
    }
    let current = if found.len() == 1 {
        found.first().map(|(uuid, _)| uuid.to_ascii_uppercase())
    } else {
        found
            .iter()
            .map(|(uuid, _)| uuid.to_ascii_uppercase())
            .find(|uuid| order_uuids.iter().any(|known| known.eq_ignore_ascii_case(uuid)))
            .or_else(|| found.first().map(|(uuid, _)| uuid.to_ascii_uppercase()))
    };
    for (uuid, path) in &found {
        let short = &uuid[..8.min(uuid.len())];
        let is_current = current
            .as_deref()
            .map(|known| known.eq_ignore_ascii_case(uuid))
            .unwrap_or(false);
        if is_current {
            // 与「启动卷」那条是同一个系统：合并，别在菜单里重复一次
            if let Some(host) = out
                .iter_mut()
                .find(|entry| entry.kind == "macos" && entry.volume == primary_label)
            {
                let extra = format!(
                    "引导文件实际在 APFS Preboot（{short}…）；OC 也可能把同一系统按两条列出来"
                );
                host.note = Some(match host.note.take() {
                    Some(old) => format!("{old}；{extra}"),
                    None => extra,
                });
                continue;
            }
        }
        let title = fs::read_to_string(
            path.join("System/Library/CoreServices/.contentDetails"),
        )
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| {
            if is_current {
                primary_label.to_string()
            } else {
                format!("macOS（Preboot {short}）")
            }
        });
        out.push(MenuEntry {
            title,
            kind: "macos".to_string(),
            source: "preboot".to_string(),
            volume: format!("Preboot {short}"),
            path: normalized(&format!("Preboot/{uuid}/System/Library/CoreServices/boot.efi")),
            auxiliary: false,
            default: false,
            note: Some(if is_current {
                "当前系统的引导文件就在 APFS Preboot 里（同一容器里还有别的 macOS 时会各列一条）"
                    .to_string()
            } else {
                "同一 APFS 容器里的另一个 macOS：根卷没挂载，但 Preboot 里能取到 boot.efi"
                    .to_string()
            }),
            spec: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_option_parsing() {
        // 实测 Boot0080：描述 "MAC"，路径 \System\Library\CoreServices\boot.efi
        let mut value: Vec<u8> = vec![0x01, 0x00, 0x00, 0x00];
        let path_units: Vec<u8> = "\\EFI\\OC\\OpenCore.efi"
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .chain([0, 0])
            .collect();
        let list_len = 42 + 4 + path_units.len() + 4;
        value.extend_from_slice(&(list_len as u16).to_le_bytes());
        for unit in "OpenCore".encode_utf16() {
            value.extend_from_slice(&unit.to_le_bytes());
        }
        value.extend_from_slice(&[0, 0]);
        // HardDrive 节点（内容不参与解析，只占位）
        value.extend_from_slice(&[0x04, 0x01, 0x2a, 0x00]);
        value.extend(std::iter::repeat_n(0, 38));
        // FilePath 节点
        value.extend_from_slice(&[0x04, 0x04]);
        value.extend_from_slice(&((4 + path_units.len()) as u16).to_le_bytes());
        value.extend_from_slice(&path_units);
        value.extend_from_slice(&[0x7f, 0xff, 0x04, 0x00]);

        let (description, path) = parse_load_option(&value).expect("解析失败");
        assert_eq!(description, "OpenCore");
        assert_eq!(path, "\\EFI\\OC\\OpenCore.efi");
        assert_eq!(normalized(&path), "\\EFI\\OC\\OpenCore.efi");
        // 太短 / 没有 FilePath 节点 → 不 panic，路径为空
        assert_eq!(parse_load_option(&[1, 0, 0]), None);
    }

    #[test]
    fn normalized_paths_are_comparable() {
        assert_eq!(normalized("/EFI/OC/OpenCore.efi"), "\\EFI\\OC\\OpenCore.efi");
        assert_eq!(normalized("\\System\\boot.efi"), "\\System\\boot.efi");
        assert_eq!(normalized("  EFI\\BOOT\\BOOTX64.EFI "), "\\EFI\\BOOT\\BOOTX64.EFI");
    }

    #[test]
    fn same_target_handles_preboot_form() {
        // 实测：Boot0080 的路径是 Preboot 写法（带容器 UUID），卷扫描出来的带 `Preboot\` 前缀
        let from_order =
            "\\2D8A2ED9-B942-494D-9220-59187BC83F94\\System\\Library\\CoreServices\\boot.efi";
        let from_scan =
            "\\Preboot\\2D8A2ED9-B942-494D-9220-59187BC83F94\\System\\Library\\CoreServices\\boot.efi";
        assert!(same_target(from_order, from_scan), "同一个引导文件不能判成死项");
        // UUID 不同 → 不是同一个系统
        assert!(!same_target(
            "\\11111111-2222-3333-4444-555555555555\\System\\Library\\CoreServices\\boot.efi",
            from_scan
        ));
        // 全等（大小写不敏感）
        assert!(same_target("\\EFI\\BOOT\\BOOTX64.EFI", "\\efi\\boot\\bootx64.efi"));
        // 文件名不同 → 不合并（避免把不同卷上的同名路径混成一个）
        assert!(!same_target(
            "\\EFI\\BOOT\\BOOTX64.EFI",
            "\\EFI\\Microsoft\\Boot\\bootmgfw.efi"
        ));
    }

    #[test]
    fn predefined_order_matches_opencore() {
        // 顺序有意义：OC 每卷只取第一个命中的，boot.efi 优先于 BOOT*.EFI
        assert_eq!(PREDEFINED[0].1, "macos");
        assert_eq!(PREDEFINED[1].1, "windows");
        assert_eq!(PREDEFINED[2].1, "generic");
        // 兜底项按架构各一条：x64 在前（某卷上两个都在时命中顺序与以前一致），
        // ARM64 的 BOOTAA64.EFI 也要能认出来，否则 Apple Silicon / WoA 上整条兜底丢失。
        assert_eq!(PREDEFINED[2].0, "EFI/BOOT/BOOTX64.EFI");
        assert_eq!(PREDEFINED[3].0, "EFI/BOOT/BOOTAA64.EFI");
        assert_eq!(PREDEFINED[3].1, "generic");
        assert_eq!(PREDEFINED.len(), 4, "预定义路径表：3 类系统 + 2 种架构的兜底");
    }

    /// 真机探针：`cargo test --lib -- --ignored real_esp_preview --nocapture`
    #[test]
    #[ignore = "真机：需要 ESP 挂载着"]
    fn real_esp_preview() {
        let root = Path::new("/Volumes/MacOS-efi");
        if !grub_rel(root, "EFI/OC/config.plist").is_file() {
            println!("跳过：{root:?} 上没有 OC");
            return;
        }
        let preview = preview(root);
        println!("summary: {}", preview.summary);
        println!("—— 会显示 ——");
        for entry in &preview.entries {
            println!(
                "[{}] {} / {} / {}{}\n    可设默认: {}\n    {}",
                entry.source,
                entry.kind,
                entry.volume,
                entry.path,
                if entry.default { "   << 默认 >>" } else { "" },
                entry.spec.is_some(),
                entry.note.as_deref().unwrap_or("-")
            );
        }
        println!("—— 被 HideAuxiliary 藏掉 ——");
        for entry in &preview.hidden {
            println!("[{}] {} / {}", entry.source, entry.kind, entry.path);
        }
        println!("—— 诊断 ——");
        for issue in &preview.issues {
            println!("({}) {}", issue.severity, issue.text);
        }
    }
}
