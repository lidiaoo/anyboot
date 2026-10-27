//! OpenCore 通道：检测 + config.plist 健康检查 + BootNext 切换。
//!
//! OC 的子系统条目由 OC picker 自己管，AnyBoot 不重复枚举；本通道固定两个动作：
//!   * 「进入 OpenCore 菜单」= 写 NVRAM `BootNext` 指向 OpenCore.efi 的启动项；
//!   * 健康检查只提示不擅改（ShowPicker/AllowSetDefault/Vault/LauncherOption）。
//!
//! Vault=Secure 时**绝不改 OC 目录里的任何文件**（自检会失败、直接进不了系统）。

use super::*;

const OC_EFI: &str = "EFI/OC/OpenCore.efi";
const OC_CONFIG: &str = "EFI/OC/config.plist";
pub(crate) const OC_MARKER: &str = "OC\\OpenCore.efi";

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
    if plist_lite::read_scalar(config_xml, &["Misc", "Boot", "LauncherOption"])
        == Some(Scalar::String("Full".to_string()))
    {
        issues.push(HealthIssue {
            severity: "warn".to_string(),
            text: "Misc→Boot→LauncherOption=Full：OC 每次启动都会抢占 BootOrder（AnyBoot 写的引导顺序会被它改回去），建议 System 或 Disabled".to_string(),
        });
    }
    issues
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
    prune_backups(&dir, 10)
}

/// 只保留最近 `keep` 份备份（按文件名 = ISO 时间排序）。
fn prune_backups(dir: &Path, keep: usize) -> Result<(), String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map_err(|error| format!("读备份目录失败: {error}"))?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();
    while names.len() > keep {
        let victim = names.remove(0);
        let _ = fs::remove_file(dir.join(victim));
    }
    Ok(())
}

/// 写 BootNext 指向 `marker`（OC/Clover 共用）对应的启动项。找到 → 写；没找到 →
/// 报错提示（创建 Boot#### 的 DevicePath 构造在后续版本补，先不赌固件兼容性）。
pub(crate) fn set_boot_next(esp_root: &Path, marker: &str, channel: &str) -> Result<u16, String> {
    let Some(number) = find_boot_number(marker) else {
        return Err(format!(
            "固件启动列表里没有引导项（Boot#### 路径不含 {marker}）。\
             请先进一次该引导器的菜单让固件注册，或用「跳板机」方案"
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
    if !registered {
        health.push(HealthIssue {
            severity: "warn".to_string(),
            text: "固件启动列表里没有 OpenCore 项（BootNext 无法写入）——请先进一次 OC 菜单让固件注册"
                .to_string(),
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
        // OC 的子系统条目由 OC picker 自己管，AnyBoot 固定一条「进菜单」动作
        entries: vec![ChannelEntry {
            id: "opencore".to_string(),
            title: "进入 OpenCore 菜单".to_string(),
            detail: "下次启动经 NVRAM BootNext → OpenCore.efi（一次性，用后自动失效）"
                .to_string(),
            icon: "macos".to_string(),
            requires_elevation: true,
        }],
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
        assert_eq!(issues.len(), 4, "四条规则全中：{issues:?}");
        assert!(issues.iter().all(|issue| issue.severity == "warn"));
        // 修好之后全绿
        xml = plist_lite::rewrite_scalar(&xml, &["Misc", "Boot", "ShowPicker"], Scalar::Bool(true)).unwrap();
        xml = plist_lite::rewrite_scalar(&xml, &["Misc", "Security", "AllowSetDefault"], Scalar::Bool(true)).unwrap();
        xml = plist_lite::rewrite_scalar(&xml, &["Misc", "Security", "Vault"], Scalar::String("Basic".into())).unwrap();
        xml = plist_lite::rewrite_scalar(&xml, &["Misc", "Boot", "LauncherOption"], Scalar::String("Disabled".into())).unwrap();
        assert!(health_checks(&xml).is_empty());
        // 缺键不算问题（OC 老版本没有这些键很正常）
        assert!(health_checks("<plist><dict/></plist>").is_empty());
    }
}
