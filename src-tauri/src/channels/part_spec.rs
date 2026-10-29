//! 分区标识的统一解析：`UUID=` / `PARTUUID=` / `LABEL=` / `/dev/sda1` …
//!
//! 为什么要有这一层：同一个 ESP 在不同的地方有四种写法 ——
//! * Linux 的 `fstab` 里写 `UUID=E537-7F31`、`PARTUUID=b3fbe669-…`、`LABEL=ARCH-EFI`；
//! * 有的机器（尤其手动分区、换过 ESP 的）fstab 里就写死 `/dev/sda1`；
//! * 用户在我们的设置里填的是 `diskutil` 报的分区 UUID；
//! * `grub.cfg` 里又是 `root=UUID=84d0b19d-…`。
//!
//! 这些其实是同一样东西的不同投影，所以只做一个解析器，两处（设置项与 fstab）共用。
//!
//! **归一化规则**（只对 UUID 类做，卷标类保持原样）：
//! 去 `-`、转大写。`E537-7F31` ≡ `e5377f31`；ext4 的 36 位 UUID 与 FAT 的 8 位卷序列
//! 走同一套比较 —— 它们本来就不会同时挂在同一个键上，混在一起比不会误判。

use std::fmt;

/// 一个分区标识的解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PartSpec {
    /// 文件系统 UUID：ext4 超级块里的 16 字节 UUID，或 FAT 的 4 字节卷序列
    FsUuid(String),
    /// GPT 分区 UUID（Linux 的 PARTUUID / macOS `diskutil` 的 `Disk / Partition UUID`）
    PartUuid(String),
    /// 文件系统卷标（`LABEL=`）
    Label(String),
    /// GPT 分区名（`PARTLABEL=`）
    PartLabel(String),
    /// 设备节点：`/dev/sda1`、`/dev/nvme0n1p1` 这类。**不带任何标识**，只能靠推断。
    Device { disk: DiskHint, part: u32 },
}

/// 设备节点里的「盘」部分。Linux 的字母顺序按驱动探测顺序排，**不是稳定标识**，
/// 所以这里只记「用户写的是什么」，不假装能还原成 `disk0`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiskHint {
    /// `sda` / `vda` / `hda` / `xvda`：字母序（a=1, b=2 …）
    Letter(u8),
    /// `nvme0n1` 的控制器号
    Nvme(u32),
    /// `mmcblk0` 的控制器号
    Mmc(u32),
}

impl fmt::Display for DiskHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // 存的是 1 基序号（a=1），这里换回字母
            Self::Letter(index) => write!(
                f,
                "sd{}",
                (b'a' + index.saturating_sub(1)) as char
            ),
            Self::Nvme(index) => write!(f, "nvme{index}n1"),
            Self::Mmc(index) => write!(f, "mmcblk{index}"),
        }
    }
}

/// 一块分区能提供的全部标识（取不到的字段是 `None`，绝不编）。
#[derive(Clone, Debug, Default)]
pub(crate) struct PartIdentity {
    /// ext4 超级块 UUID（`84d0b19d-4ee2-…`）或 FAT 卷序列（`E537-7F31`）
    pub(crate) fs_uuid: Option<String>,
    /// GPT 分区 UUID
    pub(crate) part_uuid: Option<String>,
    /// 文件系统卷标（`arch-gui`、`ARCH-EFI`）
    pub(crate) label: Option<String>,
    /// GPT 分区名
    pub(crate) part_label: Option<String>,
    /// 本机第几块物理盘（`disk0` → 0）。跨平台能拿到，所以留着。
    pub(crate) disk_index: u32,
    /// 分区号（`disk0s9` → 9）
    pub(crate) part_number: u32,
}

/// 命中强度。`Inferred` 的命中必须让用户看得见 —— 它是「按设备名/卷标猜的」，
/// 猜错了就会把 grubenv 写到另一块盘上，所以界面不能把它当确定结果静默使用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Strength {
    None,
    Inferred,
    Exact,
}

/// `UUID=` 一类的值归一化：去 `-`、转大写。
///
/// 只对「十六进制标识」用。卷标不能这样处理（`ARCH-EFI` 的 `-` 是有意义的字符）。
pub(crate) fn normalize_uuid(raw: &str) -> String {
    raw.chars()
        .filter(|character| *character != '-')
        .flat_map(char::to_uppercase)
        .collect()
}

impl PartSpec {
    /// 解析一处标识。认得的写法见模块注释；认不出就返回 `Err`，
    /// 由调用方决定「是忽略这一行，还是报给用户」。
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let text = raw.trim();
        if text.is_empty() {
            return Err("标识是空的".to_string());
        }
        // 先处理 `KEY=value` 形式（fstab 与 grub.cfg 都用它）
        if let Some((key, value)) = text.split_once('=') {
            let value = value.trim();
            if value.is_empty() {
                return Err(format!("{} 后面没有值", key.trim()));
            }
            return match key.trim().to_ascii_uppercase().as_str() {
                "UUID" => Ok(Self::FsUuid(normalize_uuid(value))),
                "PARTUUID" => Ok(Self::PartUuid(normalize_uuid(value))),
                "LABEL" => Ok(Self::Label(value.to_string())),
                "PARTLABEL" => Ok(Self::PartLabel(value.to_string())),
                other => Err(format!("不认识的标识类型：{other}")),
            };
        }
        // 再处理设备节点：`/dev/…`，以及 udev 的 `by-*` 软链（它们是「路径」，
        // 但语义上就是某一种标识，拆出来按标识比才准）
        if let Some(rest) = text.strip_prefix("/dev/") {
            return Self::parse_device(rest).ok_or_else(|| format!("不认识的设备节点：{text}"));
        }
        Err(format!("不认识的标识：{text}"))
    }

    /// `parse` 的宽松版：认不出来就返回 `None`（fstab 里会有 `tmpfs`、`cifs` 这类
    /// 根本不是分区的行，那些不该报错）。
    pub(crate) fn parse_opt(raw: &str) -> Option<Self> {
        Self::parse(raw).ok()
    }

    fn parse_device(rest: &str) -> Option<Self> {
        // udev 的软链：`disk/by-uuid/E537-7F31`。语义上等价于 `UUID=`，
        // 所以直接折回去 —— 一条 `/dev/disk/by-*` 的路径不该被当成「设备名」推断。
        if let Some(tail) = rest.strip_prefix("disk/by-uuid/") {
            return Some(Self::FsUuid(normalize_uuid(tail)));
        }
        if let Some(tail) = rest.strip_prefix("disk/by-partuuid/") {
            return Some(Self::PartUuid(normalize_uuid(tail)));
        }
        if let Some(tail) = rest.strip_prefix("disk/by-label/") {
            return Some(Self::Label(fstab_unescape(tail)));
        }
        if let Some(tail) = rest.strip_prefix("disk/by-partlabel/") {
            return Some(Self::PartLabel(fstab_unescape(tail)));
        }
        // 其余 `disk/by-*`（by-id、by-path）与 mapper（LVM/dm-crypt）都不认：
        // 它们不是分区标识，硬猜只会造成假阳性
        if rest.starts_with("disk/") || rest.contains("mapper") {
            return None;
        }

        // nvme0n1p1 / mmcblk0p1：盘名里带数字，分区号必须靠 `p` 分隔
        if let Some(index) = rest.strip_prefix("nvme") {
            let (disk, part) = split_p_partition(index)?;
            return Some(Self::Device {
                disk: DiskHint::Nvme(disk),
                part,
            });
        }
        if let Some(index) = rest.strip_prefix("mmcblk") {
            let (disk, part) = split_p_partition(index)?;
            return Some(Self::Device {
                disk: DiskHint::Mmc(disk),
                part,
            });
        }

        // sd[a-z]1 / vda2 / hda1 / xvda1：字母 + 数字直接拼接
        for prefix in ["xvd", "sd", "vd", "hd"] {
            if let Some(tail) = rest.strip_prefix(prefix) {
                let (letter, digits) = split_alpha_digits(tail)?;
                if letter.len() != 1 {
                    return None;
                }
                let index = (letter.as_bytes()[0].to_ascii_lowercase() as u8)
                    .checked_sub(b'a')
                    .map(|value| value + 1)?;
                let part = digits.parse::<u32>().ok()?;
                if part == 0 {
                    return None;
                }
                return Some(Self::Device {
                    disk: DiskHint::Letter(index),
                    part,
                });
            }
        }
        None
    }

    /// 这条标识命中这块分区吗？
    pub(crate) fn matches(&self, identity: &PartIdentity) -> Strength {
        match self {
            Self::FsUuid(wanted) => match &identity.fs_uuid {
                Some(value) if normalize_uuid(value) == *wanted => Strength::Exact,
                _ => Strength::None,
            },
            Self::PartUuid(wanted) => match &identity.part_uuid {
                Some(value) if normalize_uuid(value) == *wanted => Strength::Exact,
                _ => Strength::None,
            },
            Self::Label(wanted) => match &identity.label {
                // 卷标区分大小写（Linux 的 LABEL= 也是精确比较）
                Some(value) if value == wanted => Strength::Inferred,
                _ => Strength::None,
            },
            Self::PartLabel(wanted) => match &identity.part_label {
                Some(value) if value == wanted => Strength::Inferred,
                _ => Strength::None,
            },
            // 设备名只能对上「分区号」这一半：Linux 的 `/dev/sda` 是哪块盘，
            // 取决于驱动探测顺序，跨机器/加硬盘都会变。所以恒为 Inferred，
            // 调用方还必须自己再校验「这块分区真的是 ESP」。
            Self::Device { part, .. } => {
                if *part == identity.part_number {
                    Strength::Inferred
                } else {
                    Strength::None
                }
            }
        }
    }

    /// 展示用：`UUID=E537-7F31`、`/dev/sda1`
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::FsUuid(value) => format!("UUID={value}"),
            Self::PartUuid(value) => format!("PARTUUID={value}"),
            Self::Label(value) => format!("LABEL={value}"),
            Self::PartLabel(value) => format!("PARTLABEL={value}"),
            Self::Device { disk, part } => match disk {
                // `sd`/`vd`/`hd` 的盘名以字母结尾，分区号直接拼
                DiskHint::Letter(_) => format!("/dev/{disk}{part}"),
                // nvme / mmcblk 的盘名以数字结尾，必须用 `p` 分隔，否则
                // `nvme0n1` + `9` 会拼成 `nvme0n19`（单测抓到的坑）
                DiskHint::Nvme(_) | DiskHint::Mmc(_) => format!("/dev/{disk}p{part}"),
            },
        }
    }
}

/// `0n1p1` → `(0, 1)`（nvme 控制器号 + 分区号）；`0p1` → `(0, 1)`（mmcblk）
fn split_p_partition(raw: &str) -> Option<(u32, u32)> {
    let (head, part) = raw.rsplit_once('p')?;
    // `nvme0n1` 里的 `n1` 是命名空间；这里只取控制器号，命名空间恒为 1（消费级都是）
    let disk = head.strip_suffix("n1").unwrap_or(head);
    let disk = disk.parse::<u32>().ok()?;
    let part = part.parse::<u32>().ok()?;
    if part == 0 {
        return None;
    }
    Some((disk, part))
}

/// `a1` → `("a", "1")`：字母前缀与数字后缀分开
fn split_alpha_digits(raw: &str) -> Option<(&str, &str)> {
    let cut = raw
        .find(|character: char| character.is_ascii_digit())
        .unwrap_or(raw.len());
    if cut == 0 || cut == raw.len() {
        return None;
    }
    Some((&raw[..cut], &raw[cut..]))
}

// ─────────────────────────── fstab ───────────────────────────

/// `/etc/fstab` 里我们关心的三列。`options` 一律不保留 ——
/// 里面常有 `password=a00000000` 这类明文，进内存以外的任何地方都是事故。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FstabEntry {
    /// 第一列原文（`UUID=…` / `/dev/sda1` / `//192.168.0.110/ntfs-slow`）
    pub(crate) spec: String,
    /// 解析出来的分区标识（`tmpfs`/`cifs` 这类不是分区的行是 `None`）
    pub(crate) part: Option<PartSpec>,
    /// 挂载点（已反转义）
    pub(crate) mountpoint: String,
    /// 文件系统类型（`vfat` / `ext4` / `cifs` …）
    pub(crate) fstype: String,
}

/// 逐行解析 fstab。注释、空行、字段不足的行都跳过 —— 宁可少认一行，
/// 也不要因为一行写错就让整次定位失败。
pub(crate) fn parse_fstab(text: &str) -> Vec<FstabEntry> {
    let mut entries = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // fstab 的字段分隔是空白，字段内的空格用 `\040` 转义，
        // 所以这里可以放心按空白切
        let mut fields = line.split_whitespace();
        let (Some(spec), Some(mountpoint), Some(fstype)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        // 挂载点写 `none` 的行（swap 之类）对我们没意义
        if mountpoint.eq_ignore_ascii_case("none") {
            continue;
        }
        entries.push(FstabEntry {
            spec: spec.to_string(),
            part: PartSpec::parse_opt(spec),
            mountpoint: fstab_unescape(mountpoint),
            fstype: fstype.to_string(),
        });
    }
    entries
}

/// fstab 字段里的反斜杠转义（`man 5 fstab`）：
/// `\040` 空格、`\011` 制表、`\012` 换行、`\134` 反斜杠。
pub(crate) fn fstab_unescape(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            // `\\` 也是一种写法（虽然不是 fstab 规范里的八进制形式，
            // 但 inode 注释、手写 fstab 里常见），一并还原成一个反斜杠
            if index + 1 < bytes.len() && bytes[index + 1] == b'\\' {
                out.push('\\');
                index += 2;
                continue;
            }
            if index + 3 < bytes.len() {
                let octal = &raw[index + 1..index + 4];
                if octal.bytes().all(|byte| (b'0'..=b'7').contains(&byte)) {
                    if let Ok(value) = u8::from_str_radix(octal, 8) {
                        out.push(value as char);
                        index += 4;
                        continue;
                    }
                }
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

/// fstab 里所有挂载点的极简摘要，用于日志与提示（**不含 options**）。
pub(crate) fn fstab_summary(entries: &[FstabEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("{} → {}", entry.spec, entry.mountpoint))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(part: u32) -> PartIdentity {
        PartIdentity {
            fs_uuid: Some("84d0b19d-4ee2-4cd4-bc2c-80a77c19c702".to_string()),
            part_uuid: Some("B3FBE669-D3B9-4E0F-9A7C-1D2E3F4A5B6C".to_string()),
            label: Some("ARCH-EFI".to_string()),
            part_label: Some("EFI".to_string()),
            disk_index: 0,
            part_number: part,
        }
    }

    #[test]
    fn uuid_forms_are_equivalent() {
        let right = PartSpec::parse("UUID=84D0B19D-4EE2-4CD4-BC2C-80A77C19C702").unwrap();
        let left = PartSpec::parse("uuid=84d0b19d4ee24cd4bc2c80a77c19c702").unwrap();
        assert_eq!(right, left);
        assert_eq!(right.matches(&identity(9)), Strength::Exact);
        // FAT 卷序列：`E537-7F31` 与 `e5377f31` 是同一个
        assert_eq!(
            PartSpec::parse("UUID=E537-7F31").unwrap(),
            PartSpec::parse("/dev/disk/by-uuid/e5377f31").unwrap()
        );
    }

    #[test]
    fn partuuid_and_labels() {
        assert_eq!(
            PartSpec::parse("PARTUUID=b3fbe669-d3b9-4e0f-9a7c-1d2e3f4a5b6c")
                .unwrap()
                .matches(&identity(14)),
            Strength::Exact
        );
        assert_eq!(
            PartSpec::parse("LABEL=ARCH-EFI").unwrap().matches(&identity(14)),
            Strength::Inferred
        );
        assert_eq!(
            PartSpec::parse("LABEL=arch-efi").unwrap().matches(&identity(14)),
            Strength::None
        );
        assert_eq!(
            PartSpec::parse("PARTLABEL=EFI").unwrap().matches(&identity(14)),
            Strength::Inferred
        );
    }

    /// 用户定的第 2 点：fstab 里可能只写设备路径，必须认
    #[test]
    fn device_paths_of_all_shapes() {
        let cases = [
            ("/dev/sda14", 14u32),
            ("/dev/vda1", 1),
            ("/dev/hdb3", 3),
            ("/dev/xvda2", 2),
            ("/dev/nvme0n1p9", 9),
            ("/dev/mmcblk0p2", 2),
        ];
        for (text, part) in cases {
            let spec = PartSpec::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"));
            assert_eq!(
                spec.matches(&identity(part)),
                Strength::Inferred,
                "{text} 应命中分区 {part}"
            );
            // 分区号不对就不算命中 —— 不能靠「有字母」就凑一个
            assert_eq!(spec.matches(&identity(part + 1)), Strength::None);
        }
        assert_eq!(
            PartSpec::parse("/dev/sda1").unwrap(),
            PartSpec::Device {
                disk: DiskHint::Letter(1),
                part: 1
            }
        );
        assert_eq!(
            PartSpec::parse("/dev/nvme0n1p1").unwrap(),
            PartSpec::Device {
                disk: DiskHint::Nvme(0),
                part: 1
            }
        );
        assert_eq!(
            PartSpec::parse("/dev/mmcblk0p1").unwrap(),
            PartSpec::Device {
                disk: DiskHint::Mmc(0),
                part: 1
            }
        );
    }

    #[test]
    fn rejects_what_is_not_a_partition() {
        for text in [
            "tmpfs",
            "cifs",
            "//192.168.0.110/ntfs-slow",
            "/dev/mapper/cryptroot",
            "/dev/disk/by-id/ata-Samsung_SSD",
            "/dev/sda",
            "",
        ] {
            assert!(PartSpec::parse_opt(text).is_none(), "{text} 不该被认出来");
        }
    }

    #[test]
    fn fstab_fields_and_escapes() {
        let text = "\
# 注释行不算
UUID=84d0b19d-4ee2-4cd4-bc2c-80a77c19c702\t/\text4\trw,relatime\t0\t1
UUID=E537-7F31 /boot/efi vfat rw,relatime 0 1
/dev/sda14 /mnt/my\\040disk ext4 defaults 0 2
tmpfs /tmp tmpfs defaults,noatime,mode=1777 0 0
//192.168.0.110/ntfs-slow /home/ntfs-slow cifs username=root,password=a00000000 0 0
swap none swap sw 0 0
这里字段不够
";
        let entries = parse_fstab(text);
        assert_eq!(entries.len(), 5, "{entries:#?}");
        assert_eq!(entries[0].mountpoint, "/");
        assert_eq!(
            entries[0].part,
            Some(PartSpec::FsUuid(
                "84D0B19D4EE24CD4BC2C80A77C19C702".to_string()
            ))
        );
        assert_eq!(entries[1].mountpoint, "/boot/efi");
        assert_eq!(entries[1].fstype, "vfat");
        // `\040` 转义必须还原成空格，否则挂载点比对会永远失败
        assert_eq!(entries[2].mountpoint, "/mnt/my disk");
        assert_eq!(
            entries[2].part,
            Some(PartSpec::Device {
                disk: DiskHint::Letter(1),
                part: 14
            })
        );
        // tmpfs 与 cifs 不是分区：留行、但 part 为 None
        assert!(entries[3].part.is_none());
        assert!(entries[4].part.is_none());
        // options 里那条明文密码不该出现在摘要里（摘要只用 spec 与 mountpoint）
        let summary = fstab_summary(&entries);
        assert!(!summary.contains("a00000000"), "{summary}");
    }

    #[test]
    fn fstab_unescape_only_touches_octal_escapes() {
        assert_eq!(fstab_unescape(r"a\040b"), "a b");
        assert_eq!(fstab_unescape(r"a\\b"), r"a\b");
        assert_eq!(fstab_unescape(r"C:\temp"), r"C:\temp");
        assert_eq!(fstab_unescape(r"trailing\04"), r"trailing\04");
    }

    #[test]
    fn describe_round_trips() {
        for text in ["/dev/sda14", "/dev/nvme0n1p9"] {
            assert_eq!(PartSpec::parse(text).unwrap().describe(), text);
        }
        assert_eq!(
            PartSpec::parse("UUID=e537-7f31").unwrap().describe(),
            "UUID=E5377F31"
        );
    }
}
