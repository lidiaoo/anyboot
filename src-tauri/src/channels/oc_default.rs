//! OpenCore「设为默认」——复刻 OC 菜单里对某个系统按 **Ctrl+Enter** 的效果。
//!
//! ## 为什么不能按原样实现
//!
//! OC 的 `OcSetDefaultBootEntry`（`DefaultEntryChoice.c:772`，调用点
//! `OcBootManagementLib.c:457`）只做一件事：改固件里的 `BootOrder` + `Boot0080`。
//! 而 macOS 的 EFI 运行时对 `Boot*` 有变量白名单——实测 **root + Apple 签名的
//! `bless` 都被拒**（`(iokit/common) not permitted` / `kIOReturnNotPermitted`）。
//! 所以主系统里没法直接复刻。
//!
//! ## OC 自己留的那条路
//!
//! `UEFI→Quirks→RequestBootVarRouting=true` 时，`Application/OpenCore/OpenCore.c:183`
//! 把它当 `CustomBootGuid` 传给 `OcLoadBootEntry`，于是
//! `OcGetBootOrder` / `OcSetDefaultBootEntry` 都改用 **OC 私有命名空间**
//! `4D1FDA02-38C7-4A6A-9CC6-4BCCA8B30102`，变量名带 `OCBt` 前缀
//! （`Include/Acidanthera/Guid/OcVariable.h:93`）：
//!
//! ```text
//! OCBtOrder   顺序（UINT16 LE 序列）
//! OCBt0080    某个启动项的 EFI_LOAD_OPTION
//! OCBtNext    一次性默认（OC 用完即删）
//! ```
//!
//! 这组变量 Mac 上 root 可写可回读（实测），于是「设默认项」在 macOS 侧重新成立。
//! 实机现状可直接对照：`OCBtOrder=%80%00`、`OCBt0080` 描述 `MAC`——就是用户在
//! OC 菜单里按过 Ctrl+Enter 留下的痕迹。
//!
//! ## 与 OC 的差异（有意的）
//!
//! OC 自己**永远只用 `0x0080` 这一个槽**（`DefaultEntryChoice.c:844`），设一次默认
//! 就覆盖上一条默认项的 load option。这里不复用那个槽：目标没有现成槽时另取一个
//! 空闲编号，原默认项原样保留。语义相同（`OCBtOrder[0]` 决定默认项），但不会
//! 让上一个系统「从菜单里消失」。

use super::*;

/// 值域里「卷标」与「分区内路径」之间的分隔符。
///
/// 用不可打印的 0x01：卷标可能有空格、连字符、中文，路径里有 `\`，
/// 都不适合当分隔符（而且 `:` 已经被值域本身的 `opencore-default:` 占用了）。
pub(crate) const SEP: char = '\u{1}';

/// OC 私有变量命名空间（`gOcVendorVariableGuid`）。
pub(crate) const GUID_OC_VENDOR: &str = "4D1FDA02-38C7-4A6A-9CC6-4BCCA8B30102";

/// 启动项变量落在哪个命名空间。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Namespace {
    /// true = OC 私有（`OCBt*`）；false = 固件全局（`Boot*`）
    pub(crate) custom: bool,
}

impl Namespace {
    /// 由 `UEFI→Quirks→RequestBootVarRouting` 决定（就是 OC 的 `CustomBootGuid` 来源）。
    pub(crate) fn from_config(config: &str) -> Self {
        Self {
            custom: matches!(
                plist_lite::read_scalar(config, &["UEFI", "Quirks", "RequestBootVarRouting"]),
                Some(plist_lite::PlistScalar::Bool(true))
            ),
        }
    }

    pub(crate) fn guid(&self) -> &'static str {
        if self.custom {
            GUID_OC_VENDOR
        } else {
            nvram::GUID_GLOBAL
        }
    }

    pub(crate) fn order_name(&self) -> &'static str {
        if self.custom {
            "OCBtOrder"
        } else {
            "BootOrder"
        }
    }

    /// 启动项变量名：`OCBt0080` / `Boot0080`（`VariableManagement.c:66-75` 同一规则）。
    pub(crate) fn boot_name(&self, number: u16) -> String {
        if self.custom {
            format!("OCBt{number:04X}")
        } else {
            nvram::boot_var_name(number)
        }
    }

    pub(crate) fn read_order(&self) -> Vec<u16> {
        nvram::read_var(self.order_name(), self.guid())
            .map(|value| nvram::parse_boot_order(&value))
            .unwrap_or_default()
    }

    pub(crate) fn read_option(&self, number: u16) -> Option<Vec<u8>> {
        nvram::read_var(&self.boot_name(number), self.guid()).ok()
    }

    /// 本系统能不能写这个命名空间（写私有命名空间也要一次管理员授权，但允许写）。
    ///
    /// `Boot*` 在 macOS 上恒不可写；`OCBt*` 只是别的 GUID 下的普通变量，
    /// root 可以写（实测写入 + 回读都成功）。
    pub(crate) fn writable(&self) -> bool {
        self.custom || nvram::can_write_boot_vars()
    }
}

/// 把 `number` 挪到顺序最前：其余项保持相对顺序，重复项只留一个。
///
/// 顺带去重是有意的：OC 读 BootOrder 时自己也会去重（`DefaultEntryChoice.c:690-720`），
/// 而重复编号在顺序里没有任何意义。
pub(crate) fn move_to_front(order: &[u16], number: u16) -> Vec<u16> {
    let mut out = Vec::with_capacity(order.len() + 1);
    out.push(number);
    for item in order {
        if !out.contains(item) {
            out.push(*item);
        }
    }
    out
}

/// 一条 `EFI_LOAD_OPTION` 是不是指向 `path`（分区内路径，`\EFI\...` 写法）。
fn option_targets(value: &[u8], path: &str) -> bool {
    match oc_menu::parse_load_option(value) {
        Some((_, file_path)) => oc_menu::same_target(&oc_menu::normalized(&file_path), path),
        None => false,
    }
}

/// 一条 `EFI_LOAD_OPTION` 的描述与条目名相等 —— **preboot 目标的槽匹配判据**。
///
/// 为什么不用路径：Preboot 目标的固件设备路径经过 apfs.efi 虚拟卷，AnyBoot 目前
/// 造不出来、也没真实样本可解析比对；而 OC 写 `OCBt####` 时的描述与 OC 菜单
/// 条目名同源（`.disk_label` / `.contentDetails`），预演条目的 title 也是它 ——
/// 描述相等 + 槽已在 OC 私有命名空间里，误配面很小（再错也只是改顺序，不写新值）。
fn option_titled(value: &[u8], title: &str) -> bool {
    matches!(
        oc_menu::parse_load_option(value),
        Some((description, _)) if !description.is_empty() && description.eq_ignore_ascii_case(title)
    )
}

/// 卷上有个明确的 `.efi` 文件 → 按该分区的 HardDrive 节点自己造一条启动项。
fn build_from_volume(volume: &str, path: &str) -> Result<Vec<u8>, String> {
    if !path.to_ascii_lowercase().ends_with(".efi") {
        return Err(format!(
            "{path} 不是 EFI 可执行文件（这是协议驱动生成的条目），没法直接写成固件启动项"
        ));
    }
    let root = oc_menu::volume_root(volume)
        .ok_or_else(|| format!("找不到卷「{volume}」（可能已经卸载），刷新后再试"))?;
    if !grub_rel(&root, &oc_menu::relative_under_mount(path)).is_file() {
        return Err(format!("{volume} 上已经没有 {path}，没法据此写启动项"));
    }
    let partition = boot_entry::partition_info(&root)?;
    Ok(boot_entry::build_load_option(volume, &partition, path))
}

/// 预演菜单里某一项能不能「设为默认」，能则给出前端原样回传的值域。
///
/// 只放三类：**已经在固件启动项里**（只改顺序，零风险）、**卷上有明确 .efi 文件**
/// （自己造一条）、**APFS Preboot 条目**（靠描述匹配 OC 亲写的槽，只重排不新建；
/// 见 [`set_default`] 里 ③ 段的限制说明）。协议驱动生成的条目（`OpenLinuxBoot`
/// 的 `.conf`）内嵌 OC 的自定义设备路径节点，凭路径造不出来，不给按钮而不是
/// 给一个点了必然失败的按钮。
pub(crate) fn entry_spec(
    entry: &oc_menu::MenuEntry,
    writable: bool,
    esp_root: &Path,
) -> Option<String> {
    if !writable || entry.default || entry.kind == "tool" || entry.volume == "—" {
        return None;
    }
    let supported = entry.source == "bootorder"
        || (entry.source == "bless"
            && matches!(entry.kind.as_str(), "macos" | "windows" | "generic"))
        || (entry.source == "preboot" && entry.kind == "macos");
    if !supported {
        return None;
    }
    // OC 自己的 Bootstrap 入口（这块 ESP 上的 \EFI\BOOT\BOOTX64.EFI）不提供：
    // 把它设成默认 = 每次启动都先回 OC 菜单，「默认项」就失去意义了。
    if entry.kind == "generic" {
        if let Some(root) = oc_menu::volume_root(&entry.volume) {
            if oc_menu::same_path(&root, esp_root) {
                return None;
            }
        }
    }
    // Preboot 条目额外带上条目名（第三段）：这类目标的固件设备路径要经过
    // apfs.efi 虚拟卷，AnyBoot 还造不出来 —— 匹配已有槽全靠「描述 == 条目名」
    // （OC 写 OCBt#### 时的描述与菜单条目名同源，都是 .disk_label/.contentDetails）。
    if entry.source == "preboot" {
        return Some(format!(
            "opencore-default:{}{SEP}{}{SEP}{}",
            entry.volume, entry.path, entry.title
        ));
    }
    Some(format!(
        "opencore-default:{}{SEP}{}",
        entry.volume, entry.path
    ))
}

/// 把预演菜单里的某一项设为 OC 默认项（= 在 OC 菜单里对它按 Ctrl+Enter）。
///
/// `target` 形如 `<卷标>\u{1}<分区内路径>`（就是 [`entry_spec`] 生成的那串）；
/// Preboot 条目多一段：`<卷标>\u{1}<路径>\u{1}<条目名>`（描述匹配用）。
pub(crate) fn set_default(esp_root: &Path, target: &str) -> Result<String, String> {
    let (volume, rest) = target
        .split_once(SEP)
        .ok_or_else(|| "默认项参数里缺少卷或路径".to_string())?;
    let volume = volume.trim();
    let (raw_path, title) = match rest.split_once(SEP) {
        Some((path, title)) => (path, Some(title.trim()).filter(|t| !t.is_empty())),
        None => (rest, None),
    };
    let path = oc_menu::normalized(raw_path);
    let config = fs::read_to_string(grub_rel(esp_root, opencore::OC_CONFIG)).unwrap_or_default();
    let namespace = Namespace::from_config(&config);
    if !namespace.writable() {
        return Err(
            "本机读的是固件全局 BootOrder（UEFI→Quirks→RequestBootVarRouting=false），\
             而 macOS 不允许用户态写 Boot* 变量（root 也一样）——\
             把 RequestBootVarRouting 改成 true 之后，OC 会改用私有命名空间（OCBt*），\
             AnyBoot 就能在这里设默认项了"
                .to_string(),
        );
    }
    let order = namespace.read_order();

    // ① 私有命名空间里已经有指向这个文件的启动项 → 只改顺序（与 OC 匹配到已有项的分支一致）。
    //    Preboot 目标没有路径可比，靠「描述 == 条目名」认槽。
    let mut slot: Option<u16> = None;
    for number in &order {
        if let Some(value) = namespace.read_option(*number) {
            if option_targets(&value, &path) || title.is_some_and(|t| option_titled(&value, t)) {
                slot = Some(*number);
                break;
            }
        }
    }
    // ② 固件全局里有（macOS 的 bless 建的那条）→ 把它的值原样抄进私有命名空间
    let mut copied: Option<Vec<u8>> = None;
    if slot.is_none() {
        for number in nvram::read_boot_order() {
            if let Some(value) = nvram::read_boot_option(number) {
                if option_targets(&value, &path) || title.is_some_and(|t| option_titled(&value, t)) {
                    copied = Some(value);
                    break;
                }
            }
        }
    }
    // ③ 都没有 → 自己按分区信息造一条
    let (number, writing) = match slot {
        Some(number) => (number, None),
        None => {
            // Preboot 目标造不出新条目：固件设备路径要经过 apfs.efi 虚拟卷，凭
            // 「分区 GUID + 文件路径」构造出来的 HD 节点固件不认。这条路留给
            // 「拿到 OC 亲写字节样本后按字节复刻」（docs/apfs-预演三端统一选型.md §8）。
            if volume.starts_with("Preboot") {
                return Err(format!(
                    "「{volume}」指向 APFS 容器里的 boot.efi，AnyBoot 还不能凭路径构造新启动项。\
                     在 OC 菜单里对这项按一次 Ctrl+Enter 之后，AnyBoot 就能识别这个槽、\
                     一键把它设回默认（只重排 {}，不写新值）",
                    namespace.order_name()
                ));
            }
            let number = boot_entry::next_free_number(&order, |candidate| {
                namespace.read_option(candidate).is_some()
            })
            .ok_or_else(|| "NVRAM 里找不到空闲的启动项编号".to_string())?;
            let value = match copied {
                Some(value) => value,
                None => build_from_volume(volume, &path)?,
            };
            (number, Some(value))
        }
    };

    let new_order = move_to_front(&order, number);
    let boot_name = namespace.boot_name(number);
    // 幂等：已经是默认项且不需要新建槽 → 不备份、不写盘
    if writing.is_none() && new_order == order {
        return Ok(format!(
            "「{volume}」已经是 OpenCore 的默认项（{} 首位就是 {boot_name}），没有改动",
            namespace.order_name()
        ));
    }

    opencore::backup_nvram(esp_root)?;
    let order_value = boot_entry::encode_boot_order(&new_order);
    let mut writes: Vec<(&str, &str, &[u8])> = Vec::new();
    if let Some(value) = &writing {
        writes.push((boot_name.as_str(), namespace.guid(), value.as_slice()));
    }
    writes.push((
        namespace.order_name(),
        namespace.guid(),
        order_value.as_slice(),
    ));
    nvram::write_vars(&writes).map_err(|error| error.message())?;

    // 回读校验：固件/系统可能静默丢弃写入（macOS 对私有命名空间也是要 root 的）
    if namespace.read_order().first() != Some(&number) {
        // 两种可能：固件把写入丢了；或者 `/Library` 里那份免密助手是旧版本
        // （旧版会把变量名转大写，写进 `OCBTORDER` 这个「同名的另一个变量」，
        // `nvram` 照样回 ok）。后者只要重装助手就好，所以直接给出下一步。
        let helper = crate::helper_install::view();
        let hint = if helper.stale {
            "（/Library 里那份免密助手是旧版本：点卡片上的「重装」换成当前版本再试）"
        } else {
            ""
        };
        return Err(format!(
            "{} 写进去了，但回读时 {} 不在首位——这次写入没有生效{hint}",
            namespace.order_name(),
            boot_name
        ));
    }
    if let Some(value) = &writing {
        let stored = namespace
            .read_option(number)
            .ok_or_else(|| format!("写完读不回 {boot_name}，NVRAM 拒绝了这次写入"))?;
        if &stored != value {
            return Err(format!("{boot_name} 回读内容与写入不一致，写入没有落到 NVRAM"));
        }
    }

    let _ = opencore::update_channel_state(
        esp_root,
        "opencore",
        &[
            (
                "defaultEntry",
                serde_json::Value::String(volume.to_string()),
            ),
            ("defaultBoot", serde_json::Value::String(boot_name.clone())),
            (
                "defaultAt",
                serde_json::Value::String(anyboot_now_iso()),
            ),
        ],
    );

    Ok(format!(
        "已把「{volume}」设为 OpenCore 默认项（等价于在 OC 菜单里按 Ctrl+Enter）：{}。\
         重启后 OC 直接进它，不用再选",
        if writing.is_some() {
            format!("在 {} 里新建 {boot_name} 并放到首位", namespace.order_name())
        } else {
            format!("把 {boot_name} 挪到 {} 首位", namespace.order_name())
        }
    ))
}

// ─────────────────────────── 按需提权（Windows） ───────────────────────────

/// 提权子进程的回传文件：`{ ok, message | error }`。
#[derive(serde::Deserialize)]
struct SetDefaultReply {
    ok: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// 「设为默认」的提权版：弹一次 UAC，让本程序自己（提权子进程）跑同一套
/// [`set_default`]，结果经 JSON 文件回传。Windows 专用 —— macOS 走免密助手
/// 信箱（helper_install），Linux 的 efivars 写路径尚未接提权（遗留）。
///
/// **必须在 async 命令的工作线程上调**：ShellExecuteExW 同步等 UAC 对话框。
#[cfg(target_os = "windows")]
pub(crate) fn set_default_elevated(target: &str) -> Result<String, String> {
    let out_path = super::elevate::result_file_path("oc-set-default");
    super::elevate::run_elevated(
        &[
            "--oc-set-default".to_string(),
            "--target".to_string(),
            target.to_string(),
            "--out".to_string(),
            out_path.to_string_lossy().to_string(),
        ],
        &out_path,
    )?;
    let text = std::fs::read_to_string(&out_path)
        .map_err(|error| format!("提权子进程退出了但结果文件读不到：{error}"))?;
    let reply: SetDefaultReply = serde_json::from_str(&text)
        .map_err(|error| format!("提权设默认的结果不是合法 JSON：{error}"))?;
    if reply.ok {
        Ok(reply
            .message
            .unwrap_or_else(|| "已提权完成 OpenCore 默认项设置".to_string()))
    } else {
        Err(reply
            .error
            .unwrap_or_else(|| "提权设默认失败（子进程没给原因）".to_string()))
    }
}

/// `--oc-set-default` 子命令体：提权子进程里跑 [`set_default`]，结果原子写进
/// `--out` 文件。ESP 自行定位（子进程没有 AppHandle，走不了 resolve_esp_root
/// 的用户目录偏好，直接扫候选根）。
pub(crate) fn privileged_set_default_main(target: &str, out_path: &str) -> i32 {
    // 提权 ≠ 特权已启用：Windows 上 SeSystemEnvironmentPrivilege 在管理员令牌里
    // 通常处于 disabled 态，不显式启用的话读写 OCBt*/Boot* 报 5/1314 —— 和
    // 「没提权」同症状，会被误诊成「OC 没写过槽」（真机踩过：OCBt0080 明明在）。
    let _ = super::nvram::enable_system_environment_privilege();
    let payload = match (|| -> Result<String, String> {
        let esp_root = super::candidate_roots()
            .into_iter()
            .find(|root| super::has_channel_signature(root))
            .ok_or_else(|| "没有找到可用的 ESP（三条通道都需要它）".to_string())?;
        set_default(&esp_root, target)
    })() {
        Ok(message) => serde_json::json!({ "ok": true, "message": message }),
        Err(error) => serde_json::json!({ "ok": false, "error": error }),
    };
    write_reply(out_path, &payload)
}

/// 结果原子落盘（临时文件 + rename，父进程读到的一定是完整 JSON）。
fn write_reply(out_path: &str, payload: &serde_json::Value) -> i32 {
    let text = match serde_json::to_string(payload) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("序列化结果失败：{error}");
            return 2;
        }
    };
    let tmp = std::path::PathBuf::from(format!("{out_path}.tmp-{}", std::process::id()));
    if let Err(error) = std::fs::write(&tmp, text) {
        eprintln!("写结果失败：{error}");
        return 2;
    }
    if let Err(error) = std::fs::rename(&tmp, out_path) {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("落盘结果失败：{error}");
        return 2;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(volume: &str, path: &str, kind: &str, source: &str, default: bool) -> oc_menu::MenuEntry {
        oc_menu::MenuEntry {
            title: volume.to_string(),
            kind: kind.to_string(),
            source: source.to_string(),
            volume: volume.to_string(),
            path: path.to_string(),
            auxiliary: false,
            default,
            note: None,
            spec: None,
        }
    }

    #[test]
    fn namespace_follows_request_boot_var_routing() {
        let on = r#"<plist><dict><key>UEFI</key><dict><key>Quirks</key><dict>
            <key>RequestBootVarRouting</key><true/></dict></dict></dict></plist>"#;
        let off = r#"<plist><dict><key>UEFI</key><dict><key>Quirks</key><dict>
            <key>RequestBootVarRouting</key><false/></dict></dict></dict></plist>"#;
        let custom = Namespace::from_config(on);
        assert!(custom.custom);
        assert_eq!(custom.guid(), GUID_OC_VENDOR);
        assert_eq!(custom.order_name(), "OCBtOrder");
        assert_eq!(custom.boot_name(0x80), "OCBt0080");
        // 缺键按 false 处理（OC 的默认值），走固件全局变量
        let global = Namespace::from_config(off);
        assert!(!global.custom);
        assert_eq!(global.guid(), nvram::GUID_GLOBAL);
        assert_eq!(global.order_name(), "BootOrder");
        assert_eq!(global.boot_name(0x0080), "Boot0080");
        assert!(!Namespace::from_config("").custom);
    }

    #[test]
    fn move_to_front_keeps_relative_order() {
        assert_eq!(move_to_front(&[0x80, 0x81, 0x82], 0x81), vec![0x81, 0x80, 0x82]);
        assert_eq!(move_to_front(&[0x80], 0x81), vec![0x81, 0x80]);
        assert_eq!(move_to_front(&[], 0x80), vec![0x80]);
        // 已在首位 → 原样（幂等判断靠它）
        assert_eq!(move_to_front(&[0x81, 0x80], 0x81), vec![0x81, 0x80]);
        // 重复项只留一个
        assert_eq!(move_to_front(&[0x80, 0x81, 0x80], 0x81), vec![0x81, 0x80]);
    }

    #[test]
    fn spec_gating() {
        let esp = Path::new("/");
        // 已有固件启动项（bootorder）→ 可设
        assert!(entry_spec(&entry("MAC", "\\System\\boot.efi", "macos", "bootorder", false), true, esp).is_some());
        // 卷上的 windows 引导文件（bless）→ 可设
        assert!(
            entry_spec(&entry("WINDOWS-EFI", "\\EFI\\Microsoft\\Boot\\bootmgfw.efi", "windows", "bless", false), true, esp)
                .is_some()
        );
        // 已经是默认项 / 平台不可写 / 工具项 / 无卷 → 不给按钮
        assert!(entry_spec(&entry("MAC", "\\System\\boot.efi", "macos", "bootorder", true), true, esp).is_none());
        assert!(entry_spec(&entry("MAC", "\\System\\boot.efi", "macos", "bootorder", false), false, esp).is_none());
        assert!(entry_spec(&entry("—", "resetnvramentry.efi", "tool", "protocol", false), true, esp).is_none());
        // 协议驱动条目（OpenLinuxBoot 的 .conf）造不出设备路径 → 不给
        assert!(
            entry_spec(&entry("ARCH", "\\loader\\entries\\arch.conf", "linux", "protocol", false), true, esp)
                .is_none()
        );
        // Preboot 条目（APFS 容器里的 macOS）→ 可设，但值域带第三段条目名：
        // 这类目标只走「描述匹配 OC 亲写的槽、重排顺序」，不新建启动项（见 set_default ③）
        let preboot_spec = entry_spec(
            &entry("Preboot 2D8A2ED9", "\\Preboot\\2D8A2ED9-x\\boot.efi", "macos", "preboot", false),
            true,
            esp,
        )
        .expect("preboot 条目现在给按钮（重排已有槽）");
        assert!(preboot_spec.starts_with("opencore-default:"));
        assert_eq!(preboot_spec.matches(SEP).count(), 2, "第三段是条目名");
    }

    #[test]
    fn spec_value_round_trips_the_path() {
        // 值域用 0x01 分隔：卷标里的空格/中文、路径里的反斜杠都不会撞
        let spec = entry_spec(
            &entry("Macintosh HD", "\\EFI\\Microsoft\\Boot\\bootmgfw.efi", "windows", "bless", false),
            true,
            Path::new("/Volumes/Other"),
        )
        .expect("应可设");
        assert!(spec.starts_with("opencore-default:"));
        let (volume, path) = spec
            .trim_start_matches("opencore-default:")
            .split_once(SEP)
            .expect("分隔符");
        assert_eq!(volume, "Macintosh HD");
        assert_eq!(path, "\\EFI\\Microsoft\\Boot\\bootmgfw.efi");
        // 路径里的 `:` 不会破坏解析（值域只 split 一次）
        let with_colon = "opencore-default:VOL\u{1}\\EFI\\x:y.efi";
        let (_, rest) = with_colon.split_once(':').expect("冒号");
        assert!(rest.contains(':'));
    }

    #[test]
    fn option_target_matching_handles_preboot_forms() {
        let hd = boot_entry::HdNode {
            partition: 3,
            start_lba: 40,
            size_sectors: 4096,
            guid: [7u8; 16],
        };
        let value = boot_entry::build_load_option("MAC", &hd, "\\System\\Library\\CoreServices\\boot.efi");
        assert!(option_targets(&value, "\\System\\Library\\CoreServices\\boot.efi"));
        // 大小写不敏感
        assert!(option_targets(&value, "\\system\\library\\coreservices\\BOOT.EFI"));
        // 别的文件不算
        assert!(!option_targets(&value, "\\EFI\\Microsoft\\Boot\\bootmgfw.efi"));
        // 残缺值不 panic
        assert!(!option_targets(&[], "\\EFI\\BOOT\\BOOTX64.EFI"));
    }

    /// 真机探针（**只读**）：把「设默认项」要写的那份值算出来并打印，不碰 NVRAM。
    ///
    /// `cargo test --lib -- --ignored real_esp_plan --nocapture`
    #[test]
    #[ignore = "真机：需要 ESP 挂载着"]
    fn real_esp_plan() {
        let root = Path::new("/Volumes/MacOS-efi");
        let config = fs::read_to_string(grub_rel(root, opencore::OC_CONFIG)).unwrap_or_default();
        if config.is_empty() {
            println!("跳过：{root:?} 上没有 OC 配置");
            return;
        }
        let namespace = Namespace::from_config(&config);
        println!(
            "命名空间: {} / {} / 可写={}",
            namespace.order_name(),
            namespace.guid(),
            namespace.writable()
        );
        let order = namespace.read_order();
        println!("当前顺序: {order:04X?}");
        for number in &order {
            let Some(value) = namespace.read_option(*number) else {
                continue;
            };
            println!(
                "  {} → {:?}",
                namespace.boot_name(*number),
                oc_menu::parse_load_option(&value)
            );
        }
        let path = "\\EFI\\Microsoft\\Boot\\bootmgfw.efi";
        println!(
            "目标 {path}: 已有槽={:?} 固件里可抄={}",
            order.iter().find(|number| namespace
                .read_option(**number)
                .map(|value| option_targets(&value, path))
                .unwrap_or(false)),
            nvram::read_boot_order().iter().any(|number| nvram::read_boot_option(*number)
                .map(|value| option_targets(&value, path))
                .unwrap_or(false))
        );
        match build_from_volume("WINDOWS-EFI", path) {
            Ok(value) => {
                println!(
                    "将新建的值: {} 字节 / 解析回来 {:?}",
                    value.len(),
                    oc_menu::parse_load_option(&value)
                );
                println!("HEX: {}", hex::encode(&value));
                let number = boot_entry::next_free_number(&order, |n| namespace.read_option(n).is_some());
                println!(
                    "预计槽位: {:?}，顺序变成 {:04X?}",
                    number.map(|n| namespace.boot_name(n)),
                    number.map(|n| move_to_front(&order, n)).unwrap_or_default()
                );
            }
            Err(error) => println!("造不出来: {error}"),
        }
    }

    /// 真机探针（**会写 OCBt* 变量，跑完立刻还原**）：完整走一遍「设默认项」。
    ///
    /// 需要 root（否则 `nvram` 写入会弹一次管理员授权框），所以直接用测试二进制：
    /// `sudo -S <target/debug/deps/anyboot_lib-xxx> --ignored real_esp_set_default --nocapture`
    #[test]
    #[ignore = "真机：会写 OCBt* 变量（自己还原）"]
    fn real_esp_set_default() {
        let root = Path::new("/Volumes/MacOS-efi");
        let config = fs::read_to_string(grub_rel(root, opencore::OC_CONFIG)).unwrap_or_default();
        let namespace = Namespace::from_config(&config);
        if !namespace.custom {
            println!("跳过：这台机器的 OC 没开 RequestBootVarRouting");
            return;
        }
        let before = namespace.read_order();
        println!("原始 {}: {before:04X?}", namespace.order_name());

        let windows = format!("WINDOWS-EFI{SEP}\\EFI\\Microsoft\\Boot\\bootmgfw.efi");
        println!(
            "① 设 Windows 为默认 → {}",
            set_default(root, &windows).unwrap_or_else(|error| format!("失败: {error}"))
        );
        let after = namespace.read_order();
        println!("   写后顺序: {after:04X?} / 槽位内容: {:?}", after.first().map(|n| namespace.read_option(*n)));

        let mac = format!("MAC{SEP}\\System\\Library\\CoreServices\\boot.efi");
        println!(
            "② 设回 macOS → {}",
            set_default(root, &mac).unwrap_or_else(|error| format!("失败: {error}"))
        );
        // 顺序按原值精确还原（不依赖 set_default 的语义）
        let _ = nvram::write_var(
            namespace.order_name(),
            namespace.guid(),
            &boot_entry::encode_boot_order(&before),
        );
        let restored = namespace.read_order();
        println!("   还原后: {restored:04X?}  {}", if restored == before { "✓ 与原始一致" } else { "✗ 不一致，请看这里！" });
    }
}
