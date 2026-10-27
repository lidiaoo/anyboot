//! 直接写固件启动项（`Boot####` + `BootOrder`）——引导器「自注册」靠不住时的硬路径。
//!
//! 为什么需要它：OC / Clover 的启动项本应由它们自己在 `BOOT_SERVICES` 阶段写进固件
//! （OC 的 `Misc→Boot→LauncherOption`、Clover 的 `bless`），但那条链路依赖固件的
//! NVRAM 写入策略、OC 的 `RequestBootVarRouting` 代理和 OC 版本，实测会**静默失败**：
//! 配置明明改成了 `LauncherOption=Full`、也重启进过 OC，固件 `BootOrder` 里却始终只有
//! macOS 那一项（`Boot0080`），`BootNext` 无处可指，通道卡片永远报「未注册」。
//!
//! 这里把 EFI_LOAD_OPTION 自己构造出来直接写：同一条写入路径 macOS 的 `bless` 一直在用
//! （`Boot0080` 就是它建的），所以可行性不依赖任何引导器配置或版本。
//!
//! 值布局（UEFI 2.x §3.1.3 EFI_LOAD_OPTION）：
//!
//! ```text
//! Attributes(u32=LOAD_OPTION_ACTIVE) | FilePathListLength(u16)
//! | Description(UTF-16LE + NUL) | HardDrive 节点 | FilePath 节点 | End 节点
//! ```
//!
//! 两个 DevicePath 节点的字节形态（长度 42 / 变长）：

use super::*;

/// `EFI_LOAD_OPTION.Attributes` 的 `LOAD_OPTION_ACTIVE`：不置位的话固件会跳过这一项。
const LOAD_OPTION_ACTIVE: u32 = 1;
/// HardDrive 节点：Type 4 / SubType 1 / Length 0x2A（4 头 + 4 分区号 + 8 起始 LBA
/// + 8 扇区数 + 16 GUID + 2 类型字节）。
const HD_NODE_LEN: u16 = 0x2a;
/// DevicePath 结束节点：Type 0x7F / SubType 0xFF / Length 4。
const END_NODE: [u8; 4] = [0x7f, 0xff, 0x04, 0x00];
/// MBRType / SignatureType = GPT。ESP 一定是 GPT 分区。
const GPT_TYPE: u8 = 0x02;

/// HardDrive 节点（启动项所指的分区）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HdNode {
    /// GPT 分区号（`disk0s5` 里的 5）
    pub(crate) partition: u32,
    pub(crate) start_lba: u64,
    pub(crate) size_sectors: u64,
    /// 分区 GUID，UEFI 的混合字节序（前三段小端）
    pub(crate) guid: [u8; 16],
}

/// GUID 文本 → UEFI 的 16 字节混合字节序。
///
/// UEFI 的 GUID 前 4 / 后 2 / 再后 2 字节按小端存放，最后 8 字节原样。
/// 用实测值钉住：`D5E0D7F6-2193-43F7-A6BE-383FF9EC6C59` 在 `Boot0080` 里是
/// `F6 D7 E0 D5 93 21 F7 43 A6 BE 38 3F F9 EC 6C 59`。
pub(crate) fn guid_bytes(text: &str) -> Option<[u8; 16]> {
    let hex: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 32 {
        return None;
    }
    let mut raw = [0u8; 16];
    for (index, byte) in raw.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&[raw[3], raw[2], raw[1], raw[0]]);
    out[4..6].copy_from_slice(&[raw[5], raw[4]]);
    out[6..8].copy_from_slice(&[raw[7], raw[6]]);
    out[8..16].copy_from_slice(&raw[8..16]);
    Some(out)
}

/// 构造一条完整的 `EFI_LOAD_OPTION` 值（写 `Boot####` 用）。
///
/// `file_path` 是分区内的 EFI 路径，必须以 `\` 开头（如 `\EFI\OC\OpenCore.efi`）。
pub(crate) fn build_load_option(description: &str, hd: &HdNode, file_path: &str) -> Vec<u8> {
    let mut hard_drive = Vec::with_capacity(HD_NODE_LEN as usize);
    hard_drive.extend_from_slice(&[0x04, 0x01]);
    hard_drive.extend_from_slice(&HD_NODE_LEN.to_le_bytes());
    hard_drive.extend_from_slice(&hd.partition.to_le_bytes());
    hard_drive.extend_from_slice(&hd.start_lba.to_le_bytes());
    hard_drive.extend_from_slice(&hd.size_sectors.to_le_bytes());
    hard_drive.extend_from_slice(&hd.guid);
    hard_drive.push(GPT_TYPE);
    hard_drive.push(GPT_TYPE);

    let mut path_units: Vec<u8> = Vec::new();
    for unit in file_path.encode_utf16() {
        path_units.extend_from_slice(&unit.to_le_bytes());
    }
    path_units.extend_from_slice(&[0, 0]); // UTF-16 双 NUL 结尾
    let file_node_len = (4 + path_units.len()) as u16;
    let mut file_node = Vec::with_capacity(file_node_len as usize);
    file_node.extend_from_slice(&[0x04, 0x04]);
    file_node.extend_from_slice(&file_node_len.to_le_bytes());
    file_node.extend_from_slice(&path_units);

    let list_len = hard_drive.len() + file_node.len() + END_NODE.len();

    let mut value = Vec::with_capacity(6 + description.len() * 2 + 2 + list_len);
    value.extend_from_slice(&LOAD_OPTION_ACTIVE.to_le_bytes());
    value.extend_from_slice(&(list_len as u16).to_le_bytes());
    for unit in description.encode_utf16() {
        value.extend_from_slice(&unit.to_le_bytes());
    }
    value.extend_from_slice(&[0, 0]);
    value.extend_from_slice(&hard_drive);
    value.extend_from_slice(&file_node);
    value.extend_from_slice(&END_NODE);
    value
}

/// BootOrder 里加一项：已存在 → 原样；`first=true` 插到最前（抢默认启动），
/// 否则追加到末尾（不抢默认，符合 AnyBoot「默认仍由我自己控制」的约定）。
pub(crate) fn boot_order_with(order: &[u16], number: u16, first: bool) -> Vec<u16> {
    if order.contains(&number) {
        return order.to_vec();
    }
    let mut out = Vec::with_capacity(order.len() + 1);
    if first {
        out.push(number);
        out.extend_from_slice(order);
    } else {
        out.extend_from_slice(order);
        out.push(number);
    }
    out
}

/// BootOrder 值本体（UINT16 LE 序列）。
pub(crate) fn encode_boot_order(order: &[u16]) -> Vec<u8> {
    let mut value = Vec::with_capacity(order.len() * 2);
    for number in order {
        value.extend_from_slice(&number.to_le_bytes());
    }
    value
}

/// 找一个能用的启动项编号：从 `0x0080` 起（macOS bless 也用这一段），
/// 跳过 BootOrder 里已有的、以及变量已经存在的（孤儿项）。
pub(crate) fn next_free_number(
    order: &[u16],
    exists: impl Fn(u16) -> bool,
) -> Option<u16> {
    (0x0080u16..=0xFFFF).find(|number| !order.contains(number) && !exists(*number))
}

// ─────────────────────────── 平台分区信息 ───────────────────────────

/// 探出 ESP 所在分区的 HardDrive 节点信息。
pub(crate) fn partition_info(esp_root: &Path) -> Result<HdNode, String> {
    partition_info_platform(esp_root)
}

/// `diskutil info -plist` 的输出 → HardDrive 节点（纯函数，单测覆盖）。
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn parse_diskutil_info(xml: &str) -> Result<HdNode, String> {
    use plist_lite::PlistScalar as Scalar;
    let text = |key: &str| match plist_lite::read_scalar(xml, &[key]) {
        Some(Scalar::String(value)) => Some(value),
        _ => None,
    };
    let integer = |key: &str| match plist_lite::read_scalar(xml, &[key]) {
        Some(Scalar::Integer(value)) => Some(value),
        _ => None,
    };
    let identifier = text("DeviceIdentifier")
        .ok_or_else(|| "diskutil 输出里没有 DeviceIdentifier".to_string())?;
    let partition = identifier
        .rsplit('s')
        .next()
        .and_then(|number| number.parse::<u32>().ok())
        .ok_or_else(|| format!("从 {identifier} 里解析不出分区号"))?;
    let offset = integer("PartitionMapPartitionOffset")
        .ok_or_else(|| format!("{identifier} 上没有 PartitionMapPartitionOffset（不是磁盘分区？"))?;
    let size = integer("Size").ok_or_else(|| format!("{identifier} 上没有 Size"))?;
    let block = integer("DeviceBlockSize").filter(|block| *block > 0).unwrap_or(512);
    let guid_text = text("DiskUUID")
        .ok_or_else(|| format!("{identifier} 上没有 DiskUUID（不是 GPT 分区？"))?;
    let guid =
        guid_bytes(&guid_text).ok_or_else(|| format!("分区 GUID {guid_text} 解析失败"))?;
    Ok(HdNode {
        partition,
        start_lba: (offset / block) as u64,
        size_sectors: (size / block) as u64,
        guid,
    })
}

#[cfg(target_os = "macos")]
fn partition_info_platform(esp_root: &Path) -> Result<HdNode, String> {
    let output = command("diskutil")
        .arg("info")
        .arg("-plist")
        .arg(esp_root)
        .output()
        .map_err(|error| format!("执行 diskutil 失败: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "diskutil info {} 失败（ESP 没挂载？）",
            esp_root.display()
        ));
    }
    parse_diskutil_info(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "linux")]
fn partition_info_platform(esp_root: &Path) -> Result<HdNode, String> {
    let source = command("findmnt")
        .args(["-no", "SOURCE", "--target"])
        .arg(esp_root)
        .output()
        .map_err(|error| format!("执行 findmnt 失败: {error}"))?;
    let device = String::from_utf8_lossy(&source.stdout).trim().to_string();
    if device.is_empty() {
        return Err(format!("findmnt 找不到 {} 的设备", esp_root.display()));
    }
    let listing = command("lsblk")
        .args(["-no", "PARTN,START,SIZE,PARTUUID,LOG-SEC"])
        .arg(&device)
        .output()
        .map_err(|error| format!("执行 lsblk 失败: {error}"))?;
    let line = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 4 {
        return Err(format!("lsblk 输出无法解析: {line:?}"));
    }
    let partition = fields[0]
        .parse::<u32>()
        .map_err(|error| format!("分区号 {:?} 解析失败: {error}", fields[0]))?;
    let start_lba = fields[1]
        .parse::<u64>()
        .map_err(|error| format!("起始扇区 {:?} 解析失败: {error}", fields[1]))?;
    let size = fields[2]
        .parse::<u64>()
        .map_err(|error| format!("分区大小 {:?} 解析失败: {error}", fields[2]))?;
    let block = fields
        .get(4)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(512);
    let guid = guid_bytes(fields[3])
        .ok_or_else(|| format!("分区 GUID {:?} 解析失败（不是 GPT 分区？）", fields[3]))?;
    Ok(HdNode {
        partition,
        start_lba,
        size_sectors: size / block,
        guid,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn partition_info_platform(_esp_root: &Path) -> Result<HdNode, String> {
    Err("Windows 上还没有实现「直接写固件启动项」，请让引导器自己注册（OC 的 LauncherOption）"
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_matches_firmware_byte_order() {
        // 实测值：Boot0080 里同一分区 GUID 的字节序
        assert_eq!(
            guid_bytes("D5E0D7F6-2193-43F7-A6BE-383FF9EC6C59"),
            Some([
                0xf6, 0xd7, 0xe0, 0xd5, 0x93, 0x21, 0xf7, 0x43, 0xa6, 0xbe, 0x38, 0x3f, 0xf9, 0xec,
                0x6c, 0x59
            ])
        );
        assert_eq!(guid_bytes("不是-GUID"), None);
        // 小写也认
        assert_eq!(
            guid_bytes("d5e0d7f6-2193-43f7-a6be-383ff9ec6c59"),
            guid_bytes("D5E0D7F6-2193-43F7-A6BE-383FF9EC6C59")
        );
    }

    #[test]
    fn load_option_layout_is_self_consistent() {
        let hd = HdNode {
            partition: 5,
            start_lba: 534_679_552,
            size_sectors: 5_988_352,
            guid: guid_bytes("42074C1D-E063-431C-AAC4-4FFBE441298E").expect("guid"),
        };
        let value = build_load_option("OpenCore", &hd, "\\EFI\\OC\\OpenCore.efi");

        // ① 属性 + 路径表长度
        assert_eq!(&value[..4], &[1, 0, 0, 0]);
        let list_len = u16::from_le_bytes([value[4], value[5]]) as usize;
        assert_eq!(list_len, value.len() - 6 - "OpenCore".len() * 2 - 2);

        // ② HardDrive 节点紧跟描述（Type 4 / SubType 1 / 长度 42）
        let hd_start = 6 + "OpenCore".len() * 2 + 2;
        assert_eq!(&value[hd_start..hd_start + 4], &[0x04, 0x01, 0x2a, 0x00]);
        let partition = u32::from_le_bytes(value[hd_start + 4..hd_start + 8].try_into().expect("4B"));
        assert_eq!(partition, 5);
        let start = u64::from_le_bytes(value[hd_start + 8..hd_start + 16].try_into().expect("8B"));
        assert_eq!(start, 534_679_552);

        // ③ 路径节点 + 结束节点
        let file_start = hd_start + HD_NODE_LEN as usize;
        assert_eq!(&value[file_start..file_start + 2], &[0x04, 0x04]);
        let file_len =
            u16::from_le_bytes([value[file_start + 2], value[file_start + 3]]) as usize;
        assert_eq!(file_len % 2, 0, "FilePath 节点长度必须偶数（UTF-16）");
        assert_eq!(&value[value.len() - 4..], &END_NODE);

        // ④ 能被读取侧认出来（回读校验用的就是它）
        assert!(nvram::load_option_matches(&value, opencore::OC_MARKER));
        assert_eq!(
            nvram::load_option_description(&value).as_deref(),
            Some("OpenCore")
        );
    }

    #[test]
    fn boot_order_insert_semantics() {
        assert_eq!(boot_order_with(&[0x80], 0x81, false), vec![0x80, 0x81]);
        assert_eq!(boot_order_with(&[0x80], 0x81, true), vec![0x81, 0x80]);
        // 已存在 → 原样返回（不重复、不重排）
        assert_eq!(boot_order_with(&[0x80, 0x81], 0x81, true), vec![0x80, 0x81]);
        assert_eq!(boot_order_with(&[], 0x81, false), vec![0x81]);
        assert_eq!(encode_boot_order(&[0x80, 0x81]), vec![0x80, 0x00, 0x81, 0x00]);
    }

    #[test]
    fn free_number_skips_used_and_orphans() {
        assert_eq!(next_free_number(&[0x80], |_| false), Some(0x81));
        assert_eq!(next_free_number(&[0x80], |n| n == 0x81), Some(0x82));
        assert_eq!(next_free_number(&[], |n| n < 0x90), Some(0x90));
    }

    #[test]
    fn diskutil_info_parsing() {
        // 真机 `diskutil info -plist /Volumes/MacOS-efi` 的字段（只留用到的）
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>DeviceBlockSize</key>
	<integer>512</integer>
	<key>DeviceIdentifier</key>
	<string>disk0s5</string>
	<key>DiskUUID</key>
	<string>42074C1D-E063-431C-AAC4-4FFBE441298E</string>
	<key>MediaName</key>
	<string></string>
	<key>PartitionMapPartitionOffset</key>
	<integer>273755930624</integer>
	<key>Size</key>
	<integer>3066036224</integer>
</dict>
</plist>"#;
        let hd = parse_diskutil_info(xml).expect("解析失败");
        assert_eq!(hd.partition, 5);
        assert_eq!(hd.start_lba, 534_679_552);
        assert_eq!(hd.size_sectors, 5_988_352);
        assert_eq!(
            hd.guid,
            guid_bytes("42074C1D-E063-431C-AAC4-4FFBE441298E").expect("guid")
        );
        // 缺关键字段时给的是能看懂的错，不是 panic
        assert!(parse_diskutil_info("<plist><dict></dict></plist>").is_err());
    }
}
