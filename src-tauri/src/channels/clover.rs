//! Clover 通道：检测 + config.plist 定点改写（DefaultVolume 文件路线，免提权）
//! + BootNext 一次性（实验性）+ 模拟 NVRAM 状态识别。
//!
//! Clover 与 OC 的关键差异：Clover 的 `config.plist` 支持 `GUI→Boot→DefaultVolume`
//! 持久默认，**改文件就够**（plist_lite 定点改写，不重排不丢注释）；而 Clover 自己
//! 的模拟 NVRAM 落在 `ESP:/nvram.plist`，天然跨系统可读。

use super::*;

const CLOVER_EFI: &str = "EFI/CLOVER/CLOVERX64.efi";
const CLOVER_CONFIG: &str = "EFI/CLOVER/config.plist";
pub(crate) const CLOVER_MARKER: &str = "CLOVER\\CLOVERX64.efi";

/// 文件路线：把 config.plist 的 `GUI→Boot→DefaultVolume`（与可选 DefaultLoader）
/// 改成目标卷名。改前备份整个 config.plist 到 EFI/anyboot/clover/config-backup/。
pub(crate) fn set_default_volume(esp_root: &Path, volume: &str, loader: Option<&str>) -> Result<(), String> {
    let config_path = grub_rel(esp_root, CLOVER_CONFIG);
    let original = fs::read_to_string(&config_path)
        .map_err(|error| format!("读 {CLOVER_CONFIG} 失败（Vault 或权限问题？）: {error}"))?;
    if plist_lite::read_scalar(&original, &["Misc", "Security", "Vault"])
        == Some(plist_lite::PlistScalar::String("Secure".to_string()))
    {
        return Err("config.plist 的 Vault=Secure，任何文件改动都会导致自检失败，拒绝改写".to_string());
    }
    // 备份（保留最近 10 份）
    let backup_dir = grub_rel(esp_root, "EFI/anyboot/clover/config-backup");
    let _ = fs::create_dir_all(&backup_dir);
    let stamp = anyboot_now_iso().replace(':', "-");
    write_esp_bytes(&backup_dir.join(format!("{stamp}.plist")), original.as_bytes())?;
    let mut names: Vec<String> = fs::read_dir(&backup_dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .filter(|name| name.ends_with(".plist"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    while names.len() > 10 {
        let victim = names.remove(0);
        let _ = fs::remove_file(backup_dir.join(victim));
    }

    let original_volume = plist_lite::read_scalar(&original, &["GUI", "Boot", "DefaultVolume"])
        .map(|value| match value {
            plist_lite::PlistScalar::String(text) => text,
            other => other.to_xml(),
        });
    let mut rewritten = plist_lite::rewrite_scalar(
        &original,
        &["GUI", "Boot", "DefaultVolume"],
        plist_lite::PlistScalar::String(volume.to_string()),
    )
    .map_err(|error| format!("改写 DefaultVolume 失败：{error}"))?;
    if let Some(loader) = loader {
        rewritten = plist_lite::rewrite_scalar(
            &rewritten,
            &["GUI", "Boot", "DefaultLoader"],
            plist_lite::PlistScalar::String(loader.to_string()),
        )
        .map_err(|error| format!("改写 DefaultLoader 失败：{error}"))?;
    }
    write_esp_bytes(&config_path, rewritten.as_bytes())?;
    opencore::update_channel_state(
        esp_root,
        "clover",
        &[
            ("defaultVolume", serde_json::Value::String(volume.to_string())),
            ("originalVolume", serde_json::json!(original_volume)),
        ],
    )
}

/// 组本通道的展示视图。
pub(crate) fn view(esp_root: &Path) -> ChannelView {
    let efi_exists = grub_rel(esp_root, CLOVER_EFI).is_file();
    let config_text = fs::read_to_string(grub_rel(esp_root, CLOVER_CONFIG)).ok();
    let detected = efi_exists && config_text.is_some();
    if !detected {
        return ChannelView {
            id: "clover",
            label: "Clover",
            detected: false,
            permission: "file",
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
    let mut health = Vec::new();
    // 模拟 NVRAM：ESP:/nvram.plist 存在 = Clover 会把 NVRAM 落到这个文件（跨系统可读）
    let has_emulated = grub_rel(esp_root, "nvram.plist").is_file();
    if plist_lite::read_scalar(&config, &["GUI", "Boot", "DefaultVolume"]).is_none() {
        health.push(HealthIssue {
            severity: "warn".to_string(),
            text: "config.plist 没有 GUI→Boot→DefaultVolume 键（Clover 不记默认卷），\
                   切换时会先补上这个键"
                .to_string(),
        });
    }
    let current_default = plist_lite::read_scalar(&config, &["GUI", "Boot", "DefaultVolume"])
        .map(|value| match value {
            plist_lite::PlistScalar::String(text) => text,
            other => other.to_xml(),
        });
    if !nvram::can_write_boot_vars() {
        health.push(HealthIssue {
            severity: "info".to_string(),
            text: "本系统不允许用户态写 Boot* 变量（macOS 的 NVRAM 白名单），一次性进入 Clover \
                   这里给不了按钮；Clover 的默认卷仍可用 clover-default:<卷名> 从 CLI / 远程改"
                .to_string(),
        });
    }
    let last_boot = opencore::channel_state_key(esp_root, "clover", "defaultVolume");
    let last_boot_at = opencore::channel_state_key(esp_root, "clover", "updatedAt");
    let mut detail = String::from(if has_emulated {
        "状态来源：模拟 NVRAM（nvram.plist）· "
    } else {
        "状态来源：config.plist · "
    });
    detail.push_str(&format!(
        "DefaultVolume={}",
        current_default.as_deref().unwrap_or("（无）")
    ));
    ChannelView {
        id: "clover",
        label: "Clover",
        detected: true,
        permission: "file",
        chip: if health.iter().any(|issue| issue.severity == "warn") {
            "warn"
        } else {
            "ok"
        },
        detail: Some(detail),
        health,
        // 文件路线（clover-default:<volume>）走远程 API / CLI，UI 上先只放
        // 一次性动作 —— 卷名要打字输入，塞个按钮反而误导。
        // 一次性动作要写 BootNext，macOS 上写不了（Apple 的 NVRAM 白名单）→ 不给按钮。
        entries: if nvram::can_write_boot_vars() {
            vec![ChannelEntry {
                id: "clover-oneshot".to_string(),
                title: "一次性进入 Clover（实验性）".to_string(),
                detail: "NVRAM BootNext；Clover 对 BootNext 的支持随版本有差异".to_string(),
                icon: "menu".to_string(),
                requires_elevation: true,
            }]
        } else {
            Vec::new()
        },
        current_default,
        last_boot,
        last_boot_at,
        error: None,
    }
}
