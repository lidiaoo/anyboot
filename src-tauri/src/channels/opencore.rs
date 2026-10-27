//! OpenCore 通道：检测 + config.plist 健康检查 + BootNext 切换 + 自注册修复。
//!
//! OC 的子系统条目由 OC picker 自己管，AnyBoot 不重复枚举；本通道的动作为：
//!   * 「进入 OpenCore 菜单」= 写 NVRAM `BootNext` 指向 OpenCore.efi 的启动项；
//!   * 「修复并注册」= 改 ESP 上的 config.plist 让 OC 自己把启动项写进固件
//!     （`LauncherOption=Short/Full`）——用于固件里根本没有 OC 项、BootNext
//!     无处可指的机器（典型是靠 `\EFI\BOOT\BOOTX64.EFI` 的 Bootstrap 兜底进 OC）。
//!
//! Vault=Secure 时**绝不改 OC 目录里的任何文件**（自检会失败、直接进不了系统）。

use super::*;

const OC_EFI: &str = "EFI/OC/OpenCore.efi";
const OC_CONFIG: &str = "EFI/OC/config.plist";
pub(crate) const OC_MARKER: &str = "OC\\OpenCore.efi";

/// `LauncherOption` 的两个「OC 会自注册启动项」取值。
/// Short：注册但插到 BootOrder 末尾（不抢默认，AnyBoot 默认选它）；
/// Full：注册并把自己设为第一默认项（OC 官方推荐的黑苹果用法）。
const LAUNCHER_SHORT: &str = "Short";
const LAUNCHER_FULL: &str = "Full";

/// config.plist 健康检查（规则见技术细节设计 §5.1，全部 warn 级）。
pub(crate) fn health_checks(config_xml: &str) -> Vec<HealthIssue> {
    use plist_lite::PlistScalar as Scalar;
    let mut issues = Vec::new();
    let show_picker =
        plist_lite::read_scalar(config_xml, &["Misc", "Boot", "ShowPicker"]);
    if show_picker == Some(Scalar::Bool(false)) {
        issues.push(HealthIssue {
            severity: "warn".to_string(),
            text: "Misc→Boot→ShowPicker=false：OC 不弹菜单，BootNext 指过去也看不到选择界面，建议改 true".to_string(),
        });
    }
    if plist_lite::read_scalar(config_xml, &["Misc", "Security", "AllowSetDefault"])
        == Some(Scalar::Bool(false))
    {
        issues.push(HealthIssue {
            severity: "warn".to_string(),
            text: "Misc→Security→AllowSetDefault=false：无法在 OC 菜单里用 Ctrl+Enter 记默认项"
                .to_string(),
        });
    }
    if plist_lite::read_scalar(config_xml, &["Misc", "Security", "Vault"])
        == Some(Scalar::String("Secure".to_string()))
    {
        issues.push(HealthIssue {
            severity: "warn".to_string(),
            text: "Misc→Security→Vault=Secure：外部改动 OC 目录会自检失败，AnyBoot 不会改 OC 的任何文件"
                .to_string(),
        });
    }
    // `LauncherOption` 不在这里判：它的健康与否取决于「固件里到底有没有 OC 项」，
    // 必须结合 BootOrder 一起看（见 `view`）。Full 本身不是问题——实测它是
    // 靠 Bootstrap 兜底启动的机器唯一能自注册的方式。
    issues
}

/// 读 `Misc→Boot→LauncherOption` 的字符串值（缺失/类型不符 → None）。
fn launcher_option(config_xml: &str) -> Option<String> {
    match plist_lite::read_scalar(config_xml, &["Misc", "Boot", "LauncherOption"]) {
        Some(plist_lite::PlistScalar::String(value)) => Some(value),
        _ => None,
    }
}

/// 该值是否意味着「OC 会自己注册启动项」。
fn self_registers(launcher: Option<&str>) -> bool {
    matches!(launcher, Some(LAUNCHER_SHORT) | Some(LAUNCHER_FULL))
}


/// OC/Clover 共用：在 BootOrder 里搜路径含 `marker` 的启动项编号。
/// 找不到 → None（调用方报「未注册，请先在固件里注册引导项」）。
pub(crate) fn find_boot_number(marker: &str) -> Option<u16> {
    for number in nvram::read_boot_order() {
        if let Some(value) = nvram::read_boot_option(number) {
            if nvram::load_option_matches(&value, marker) {
                return Some(number);
            }
        }
    }
    None
}

/// 把 BootOrder + 全部 Boot#### 原值备份到 ESP（写任何 NVRAM 前调用；保留最近 10 份）。
pub(crate) fn backup_nvram(esp_root: &Path) -> Result<(), String> {
    let mut snapshot = serde_json::json!({
        "format": "anyboot-nvram-backup/1",
        "at": anyboot_now_iso(),
    });
    for number in nvram::read_boot_order() {
        if let Some(value) = nvram::read_boot_option(number) {
            snapshot[nvram::boot_var_name(number)] =
                serde_json::Value::String(hex::encode(value));
        }
    }
    if let Ok(order) = nvram::read_var("BootOrder", nvram::GUID_GLOBAL) {
        snapshot["BootOrder"] = serde_json::Value::String(hex::encode(order));
    }
    let dir = grub_rel(esp_root, "EFI/anyboot/nvram-backup");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(format!("{}.json", anyboot_now_iso().replace(':', "-")));
    write_esp_bytes(&path, snapshot.to_string().as_bytes())?;
    prune_backups(&dir, 10, ".json")
}

/// 只保留最近 `keep` 份备份（按文件名 = ISO 时间排序；只清 `suffix` 结尾的）。
fn prune_backups(dir: &Path, keep: usize, suffix: &str) -> Result<(), String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map_err(|error| format!("读备份目录失败: {error}"))?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(suffix))
        .collect();
    names.sort();
    while names.len() > keep {
        let victim = names.remove(0);
        let _ = fs::remove_file(dir.join(victim));
    }
    Ok(())
}

/// 「修复并注册」：让 OC 自己把启动项写进固件。
///
/// 固件 BootOrder 里没有 OC 项时 `BootNext` 无处可指。根因几乎总是
/// `Misc→Boot→LauncherOption=Disabled/System`——OC 被配置成不注册自己。典型症状：
/// 机器靠 `\EFI\BOOT\BOOTX64.EFI` 的 Bootstrap 兜底进 OC，系统能进，但固件
/// 启动列表里从来没有 OC 项。
///
/// 只动 ESP 上的 config.plist（不碰 NVRAM，免提权）：
/// 1. 备份原文件到 `EFI/anyboot/backup/config.plist.<时间>.bak`（留 10 份）；
/// 2. `LauncherOption` → `Short`（默认，不抢 BootOrder 默认位）/ `Full`；
/// 3. 补齐硬前提 `UEFI→Quirks→RequestBootVarRouting=true`（OC 0.7+ 该 quirk 从
///    Booter 移到 UEFI；不为 true 时 LauncherOption 不生效）；
/// 4. 纠正 `NVRAM→WriteFlash=false`（否则 OC 不落 NVRAM）；
/// 5. 写盘**前先回读校验**，不过就中止，绝不落半成品。
///
/// 生效时机：重启一次进 OC —— OC 在引导时写 Boot#### 并维护 BootOrder。
pub(crate) fn fix_launcher_option(esp_root: &Path, mode: &str) -> Result<String, String> {
    use plist_lite::PlistScalar as Scalar;

    let target = match mode.trim().to_ascii_lowercase().as_str() {
        "" | "short" => LAUNCHER_SHORT,
        "full" => LAUNCHER_FULL,
        other => {
            return Err(format!(
                "不支持的 LauncherOption 目标值 {other}（只接受 short / full）"
            ))
        }
    };
    let config_path = grub_rel(esp_root, OC_CONFIG);
    let original = fs::read_to_string(&config_path).map_err(|error| {
        format!(
            "读 {} 失败（ESP 没挂载或没有读权限？）: {error}",
            config_path.display()
        )
    })?;
    if plist_lite::read_scalar(&original, &["Misc", "Security", "Vault"])
        == Some(Scalar::String("Secure".to_string()))
    {
        return Err(
            "Misc→Security→Vault=Secure：OC 会校验 EFI 目录哈希，外部改动会导致自检失败、进不了系统。\
             请先在 OC 里把 Vault 改成 Optional 再回来修复"
                .to_string(),
        );
    }
    let previous = launcher_option(&original);

    // 先整份备份（与 NVRAM 备份同策略：留最近 10 份）
    let backup_dir = grub_rel(esp_root, "EFI/anyboot/backup");
    fs::create_dir_all(&backup_dir).map_err(|error| format!("建备份目录失败: {error}"))?;
    let backup_path = backup_dir.join(format!(
        "config.plist.{}.bak",
        anyboot_now_iso().replace(':', "-")
    ));
    fs::copy(&config_path, &backup_path)
        .map_err(|error| format!("备份 config.plist 失败: {error}"))?;
    prune_backups(&backup_dir, 10, ".bak")?;

    let mut edited = original;
    let mut notes: Vec<String> = Vec::new();

    // ① LauncherOption（缺失则插到 Misc→Boot 末尾）
    edited = upsert(
        &edited,
        &["Misc", "Boot"],
        "LauncherOption",
        Scalar::String(target.to_string()),
    )?;

    // ② RequestBootVarRouting 硬前提
    if plist_lite::read_scalar(&edited, &["UEFI", "Quirks", "RequestBootVarRouting"])
        != Some(Scalar::Bool(true))
    {
        edited = upsert(
            &edited,
            &["UEFI", "Quirks"],
            "RequestBootVarRouting",
            Scalar::Bool(true),
        )?;
        notes.push(
            "已把 UEFI→Quirks→RequestBootVarRouting 设为 true（LauncherOption 生效的硬前提）"
                .to_string(),
        );
    }
    // ③ WriteFlash 不能是 false
    if plist_lite::read_scalar(&edited, &["NVRAM", "WriteFlash"]) == Some(Scalar::Bool(false)) {
        edited = upsert(&edited, &["NVRAM"], "WriteFlash", Scalar::Bool(true))?;
        notes.push("已把 NVRAM→WriteFlash 设为 true（否则 OC 不写 NVRAM）".to_string());
    }

    // 写盘前回读校验：目标值必须都对得上，否则一个字节都不写
    if launcher_option(&edited).as_deref() != Some(target)
        || plist_lite::read_scalar(&edited, &["UEFI", "Quirks", "RequestBootVarRouting"])
            != Some(Scalar::Bool(true))
    {
        return Err(format!(
            "改写后回读校验失败，已中止（原文件未改动，备份在 {}）",
            backup_path.display()
        ));
    }
    write_esp_bytes(&config_path, edited.as_bytes())?;

    let _ = update_channel_state(
        esp_root,
        "opencore",
        &[
            (
                "launcherOption",
                serde_json::Value::String(target.to_string()),
            ),
            (
                "launcherRepairedAt",
                serde_json::Value::String(anyboot_now_iso()),
            ),
        ],
    );

    let mut message = format!(
        "已把 Misc→Boot→LauncherOption 从 {} 改成 {target}",
        previous.as_deref().unwrap_or("未设置")
    );
    if target == LAUNCHER_SHORT {
        message.push_str("（OC 会注册启动项，但不抢 BootOrder 默认位）");
    } else {
        message.push_str("（OC 会注册，并把自己设为默认启动项）");
    }
    for note in &notes {
        message.push('；');
        message.push_str(note);
    }
    message.push_str(
        "。重启一次进入 OpenCore 后它才会写进固件，到那时 AnyBoot 的「下次启动」即可使用",
    );
    Ok(message)
}

/// 有则改、无则插（`insert_scalar` 自己不做存在性判断，必须先 `has_key`）。
fn upsert(
    xml: &str,
    parents: &[&str],
    key: &str,
    value: plist_lite::PlistScalar,
) -> Result<String, String> {
    let mut path: Vec<&str> = parents.to_vec();
    path.push(key);
    if plist_lite::has_key(xml, &path) {
        plist_lite::rewrite_scalar(xml, &path, value)
    } else {
        plist_lite::insert_scalar(xml, parents, key, value)
    }
}

/// 写 BootNext 指向 `marker`（OC/Clover 共用）对应的启动项。找到 → 写；没找到 →
/// 报错提示（创建 Boot#### 的 DevicePath 构造在后续版本补，先不赌固件兼容性）。
pub(crate) fn set_boot_next(esp_root: &Path, marker: &str, channel: &str) -> Result<u16, String> {
    let Some(number) = find_boot_number(marker) else {
        return Err(format!(
            "固件启动列表里没有引导项（Boot#### 路径不含 {marker}）。\
             请先用「修复并注册」让 OC 自己写进固件，或按提示用 bless 手动注册"
        ));
    };
    backup_nvram(esp_root)?;
    nvram::write_var("BootNext", nvram::GUID_GLOBAL, &nvram::boot_next_value(number))
        .map_err(|error| error.message())?;
    update_channel_state(esp_root, channel, &[("bootNext", number.into())])?;
    Ok(number)
}

/// 通用通道状态写盘（EFI/anyboot/<channel>/state.json）。
pub(crate) fn update_channel_state(
    esp_root: &Path,
    channel: &str,
    update: &[(&str, serde_json::Value)],
) -> Result<(), String> {
    let dir = grub_rel(esp_root, &format!("EFI/anyboot/{channel}"));
    let path = dir.join("state.json");
    let mut parsed: serde_json::Value = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| serde_json::json!({"format": format!("anyboot-{channel}/1")}));
    let object = parsed
        .as_object_mut()
        .ok_or_else(|| format!("{} 不是 JSON 对象，拒绝改写", path.display()))?;
    for (key, value) in update {
        object.insert((*key).to_string(), value.clone());
    }
    object.insert(
        "updatedAt".to_string(),
        serde_json::Value::String(anyboot_now_iso()),
    );
    write_esp_bytes(&path, parsed.to_string().as_bytes())
}

/// 读通道 state.json 里的字符串键。
pub(crate) fn channel_state_key(esp_root: &Path, channel: &str, key: &str) -> Option<String> {
    let path = grub_rel(esp_root, &format!("EFI/anyboot/{channel}/state.json"));
    let text = fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
    parsed.get(key)?.as_str().map(str::to_string)
}

/// 组本通道的展示视图。
pub(crate) fn view(esp_root: &Path) -> ChannelView {
    let efi_exists = grub_rel(esp_root, OC_EFI).is_file();
    let config_path = grub_rel(esp_root, OC_CONFIG);
    let config_text = fs::read_to_string(&config_path).ok();
    let detected = efi_exists && config_text.is_some();
    if !detected {
        return ChannelView {
            id: "opencore",
            label: "OpenCore",
            detected: false,
            permission: "nvram",
            chip: "off",
            detail: None,
            health: Vec::new(),
            entries: Vec::new(),
            current_default: None,
            last_boot: None,
            last_boot_at: None,
            error: None,
        };
    }
    let config = config_text.unwrap_or_default();
    let mut health = health_checks(&config);
    // OC 引导项是否已在固件里注册（BootOrder 里能搜到）——搜不到时 BootNext 无从写起
    let registered = find_boot_number(OC_MARKER).is_some();
    let launcher = launcher_option(&config);
    let can_self_register = self_registers(launcher.as_deref());
    if !registered {
        let launcher_text = launcher.as_deref().unwrap_or("未设置");
        health.push(HealthIssue {
            severity: "warn".to_string(),
            text: if can_self_register {
                format!(
                    "LauncherOption={launcher_text}（OC 会自注册），但固件里还没有该项——重启一次进入 OpenCore 后才会写入"
                )
            } else {
                format!(
                    "固件启动列表里没有 OpenCore 项，且 LauncherOption={launcher_text}（OC 不自注册）——用下方「修复并注册」一键改掉"
                )
            },
        });
    }
    // 版本探测（有就展示，没有不勉强）
    let version = fs::read_to_string(grub_rel(esp_root, "EFI/OC/Version.plist"))
        .ok()
        .and_then(|text| {
            plist_lite::read_scalar(&text, &["ProductBuildVersion"])
                .map(|value| match value {
                    plist_lite::PlistScalar::String(text) => text,
                    other => other.to_xml(),
                })
        });
    let last_boot = channel_state_key(esp_root, "opencore", "bootNext")
        .map(|number| format!("Boot{number}（AnyBoot 上次写入的 BootNext）"));
    let last_boot_at = channel_state_key(esp_root, "opencore", "updatedAt");
    // OC 的子系统条目由 OC picker 自己管，AnyBoot 固定一条「进菜单」动作；
    // 固件里没有 OC 项、且 OC 又被配成不自注册时，补一条「修复并注册」。
    let mut entries = Vec::new();
    if !registered && !can_self_register {
        entries.push(ChannelEntry {
            id: "opencore-register".to_string(),
            title: "修复并注册 OpenCore 启动项".to_string(),
            detail: "把 Misc→Boot→LauncherOption 改成 Short（OC 自注册、不抢默认顺序），\
                     并补齐 RequestBootVarRouting 等硬前提；重启一次进入 OC 后生效"
                .to_string(),
            icon: "menu".to_string(),
            requires_elevation: false,
        });
    }
    entries.push(ChannelEntry {
        id: "opencore".to_string(),
        title: "进入 OpenCore 菜单".to_string(),
        detail: "下次启动经 NVRAM BootNext → OpenCore.efi（一次性，用后自动失效）"
            .to_string(),
        icon: "macos".to_string(),
        requires_elevation: true,
    });
    ChannelView {
        id: "opencore",
        label: "OpenCore",
        detected: true,
        permission: "nvram",
        chip: if health.iter().all(|issue| issue.severity != "warn") {
            "ok"
        } else {
            "warn"
        },
        detail: version.map(|version| format!("OpenCore {version} · 切换经 NVRAM BootNext")),
        health,
        entries,
        current_default: None,
        last_boot,
        last_boot_at,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist_lite::PlistScalar as Scalar;

    /// 典型「Bootstrap 兜底」配置：LauncherOption=Disabled（OC 不自注册）、
    /// 没有 RequestBootVarRouting 键、WriteFlash=false。
    const CONFIG_DISABLED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>Misc</key>
	<dict>
		<key>Boot</key>
		<dict>
			<key>ShowPicker</key>
			<true/>
			<key>LauncherOption</key>
			<string>Disabled</string>
		</dict>
		<key>Security</key>
		<dict>
			<key>Vault</key>
			<string>Optional</string>
		</dict>
	</dict>
	<key>UEFI</key>
	<dict>
		<key>Quirks</key>
		<dict>
			<key>EnableVmx</key>
			<false/>
		</dict>
	</dict>
	<key>NVRAM</key>
	<dict>
		<key>WriteFlash</key>
		<false/>
	</dict>
</dict>
</plist>
"#;

    const CONFIG_VAULT_SECURE: &str = r#"<plist><dict>
	<key>Misc</key>
	<dict>
		<key>Boot</key>
		<dict>
			<key>LauncherOption</key>
			<string>Disabled</string>
		</dict>
		<key>Security</key>
		<dict>
			<key>Vault</key>
			<string>Secure</string>
		</dict>
	</dict>
</dict></plist>"#;

    /// 造一个最小 ESP 目录（EFI/OC 下放 OpenCore.efi + config.plist）。
    fn mock_esp(config: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "anyboot-oc-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("EFI/OC")).unwrap();
        fs::write(dir.join("EFI/OC/OpenCore.efi"), b"stub").unwrap();
        fs::write(dir.join("EFI/OC/config.plist"), config).unwrap();
        dir
    }

    #[test]
    fn health_check_rules() {
        let mut xml = String::from(
            "<plist><dict><key>Misc</key><dict><key>Boot</key><dict>\
             <key>ShowPicker</key><false/><key>LauncherOption</key><string>Full</string>\
             </dict><key>Security</key><dict>\
             <key>AllowSetDefault</key><false/><key>Vault</key><string>Secure</string>\
             </dict></dict></dict></plist>",
        );
        let issues = health_checks(&xml);
        // ShowPicker / AllowSetDefault / Vault 三条；LauncherOption=Full 不再算问题
        assert_eq!(issues.len(), 3, "三条规则全中：{issues:?}");
        assert!(issues.iter().all(|issue| issue.severity == "warn"));
        assert!(
            !issues.iter().any(|issue| issue.text.contains("LauncherOption")),
            "Full 是有效的自注册方式，不该报 warn：{issues:?}"
        );
        // 修好之后全绿
        xml = plist_lite::rewrite_scalar(&xml, &["Misc", "Boot", "ShowPicker"], Scalar::Bool(true)).unwrap();
        xml = plist_lite::rewrite_scalar(&xml, &["Misc", "Security", "AllowSetDefault"], Scalar::Bool(true)).unwrap();
        xml = plist_lite::rewrite_scalar(&xml, &["Misc", "Security", "Vault"], Scalar::String("Basic".into())).unwrap();
        xml = plist_lite::rewrite_scalar(&xml, &["Misc", "Boot", "LauncherOption"], Scalar::String("Disabled".into())).unwrap();
        assert!(health_checks(&xml).is_empty());
        // 缺键不算问题（OC 老版本没有这些键很正常）
        assert!(health_checks("<plist><dict/></plist>").is_empty());
    }

    #[test]
    fn self_register_predicate() {
        assert!(self_registers(Some("Short")));
        assert!(self_registers(Some("Full")));
        assert!(!self_registers(Some("Disabled")));
        assert!(!self_registers(Some("System")));
        assert!(!self_registers(None));
    }

    #[test]
    fn fix_sets_short_and_prerequisites() {
        let root = mock_esp(CONFIG_DISABLED);
        let message = fix_launcher_option(&root, "").expect("修复应成功");
        assert!(message.contains("Short"), "默认应为 Short：{message}");
        assert!(message.contains("重启一次"), "必须提示需要重启：{message}");

        let after = fs::read_to_string(root.join("EFI/OC/config.plist")).unwrap();
        assert_eq!(launcher_option(&after).as_deref(), Some("Short"));
        // 硬前提：缺失的 RequestBootVarRouting 被插入为 true
        assert_eq!(
            plist_lite::read_scalar(&after, &["UEFI", "Quirks", "RequestBootVarRouting"]),
            Some(Scalar::Bool(true))
        );
        // WriteFlash=false 被纠正
        assert_eq!(
            plist_lite::read_scalar(&after, &["NVRAM", "WriteFlash"]),
            Some(Scalar::Bool(true))
        );
        // 兄弟键没被误伤
        assert_eq!(
            plist_lite::read_scalar(&after, &["UEFI", "Quirks", "EnableVmx"]),
            Some(Scalar::Bool(false))
        );
        assert_eq!(
            plist_lite::read_scalar(&after, &["Misc", "Boot", "ShowPicker"]),
            Some(Scalar::Bool(true))
        );
        // 备份留了一份，且内容 = 原文件
        let backups: Vec<_> = fs::read_dir(root.join("EFI/anyboot/backup"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(backups.len(), 1, "应恰好留一份备份");
        let backup_text = fs::read_to_string(backups[0].path()).unwrap();
        assert_eq!(backup_text, CONFIG_DISABLED);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fix_full_mode_and_idempotent() {
        let root = mock_esp(CONFIG_DISABLED);
        let message = fix_launcher_option(&root, "full").expect("修复应成功");
        assert!(message.contains("Full"), "{message}");
        let after = fs::read_to_string(root.join("EFI/OC/config.plist")).unwrap();
        assert_eq!(launcher_option(&after).as_deref(), Some("Full"));
        // 再跑一次：已经是目标值，仍应成功且不重复插键
        fix_launcher_option(&root, "full").expect("幂等重跑应成功");
        let again = fs::read_to_string(root.join("EFI/OC/config.plist")).unwrap();
        assert_eq!(again.matches("<key>LauncherOption</key>").count(), 1);
        assert_eq!(again.matches("<key>RequestBootVarRouting</key>").count(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fix_refuses_vault_secure_without_touching_file() {
        let root = mock_esp(CONFIG_VAULT_SECURE);
        let error = fix_launcher_option(&root, "short").unwrap_err();
        assert!(error.contains("Vault=Secure"), "{error}");
        // 一个字节都没动，也没留备份
        let after = fs::read_to_string(root.join("EFI/OC/config.plist")).unwrap();
        assert_eq!(after, CONFIG_VAULT_SECURE);
        assert!(!root.join("EFI/anyboot/backup").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fix_rejects_unknown_mode() {
        let root = mock_esp(CONFIG_DISABLED);
        let error = fix_launcher_option(&root, "disabled").unwrap_err();
        assert!(error.contains("只接受 short / full"), "{error}");
        let _ = fs::remove_dir_all(&root);
    }
}
