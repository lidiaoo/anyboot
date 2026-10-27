//! systemd-boot 通道：loader.conf 解析/定点改写 + 条目列表 + NVRAM one-shot。
//!
//! 切换两条策略（见技术细节设计 §4）：
//! * 策略A（默认，免提权）：改写 `ESP:/loader/loader.conf` 的 `default` 行；
//! * 策略B（one-shot，提权）：写 EFI 变量 `LoaderEntryOneShot`（systemd-boot
//!   GUID，值 = entry-id 的 ASCII 字节）。
//!
//! 「记住上次」读 `ESP:/EFI/anyboot/sdboot/state.json`（Linux 端导出单元写的；
//! 文件不存在 = 上次未知，如实显示）。

use super::*;

/// loader.conf 的 default 行取值（忽略注释行）：`default @saved` / `default arch.conf`。
pub(crate) fn loader_conf_default(text: &str) -> Option<String> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .find_map(|line| {
            let line = line.trim();
            line.strip_prefix("default")
                .filter(|rest| rest.starts_with(char::is_whitespace) || rest.is_empty())
                .map(|rest| rest.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

/// 把 loader.conf 的 default 行改成 `new_value`；没有 default 行就插到**首个
/// 非注释行**之前（systemd-boot 只认键序不认位置，插最前面最显眼）。
/// 注释、空行、其余键逐字节保留；同文件多写几次结果幂等。
pub(crate) fn rewrite_loader_default(text: &str, new_value: &str) -> String {
    // 两遍法：先判断有没有 default 行，再决定「替换」还是「插入」。
    // 单遍会在「先扫到 timeout、后扫到 default」时提前插入，造成重复 default。
    let has_default = loader_conf_default(text).is_some();
    let mut out = String::with_capacity(text.len() + new_value.len() + 16);
    let mut replaced = false;
    let mut inserted = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let is_default_line =
            !trimmed.starts_with('#') && trimmed.strip_prefix("default").is_some_and(|rest| {
                rest.starts_with(char::is_whitespace) || rest.is_empty()
            });
        if is_default_line {
            if !replaced {
                out.push_str(&format!("default {new_value}"));
                out.push('\n');
                replaced = true;
            }
            // 多余的 default 行直接丢弃（loader.conf 不该有两条，清理掉）
            continue;
        }
        if !has_default && !inserted && !trimmed.is_empty() && !trimmed.starts_with('#') {
            out.push_str(&format!("default {new_value}\n"));
            inserted = true;
        }
        out.push_str(line);
        out.push('\n');
    }
    // 全文只有注释/空行（或空文件）：追加到末尾
    if !replaced && !inserted {
        out.push_str(&format!("default {new_value}\n"));
    }
    // 原文末尾没有换行时上面多补了一个 —— lines() 丢失了「无尾换行」信息，
    // 多一个尾换行对 loader.conf 无害，不追这个细节。
    out
}

/// 条目 conf 里的 `title` 行。
pub(crate) fn entry_title(text: &str) -> Option<String> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .find_map(|line| {
            let line = line.trim();
            line.strip_prefix("title")
                .filter(|rest| rest.starts_with(char::is_whitespace) || rest.is_empty())
                .map(|rest| rest.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

/// `*-utils.conf`（UEFI Shell / systemctl 等工具条目）不进切换列表。
pub(crate) fn is_utils_entry(filename: &str) -> bool {
    filename
        .strip_suffix(".conf")
        .is_some_and(|stem| stem.ends_with("-utils"))
}

/// 读 state.json（EFI/anyboot/sdboot/state.json）里的一个字符串键。
fn state_json_key(esp_root: &Path, key: &str) -> Option<String> {
    let path = grub_rel(esp_root, "EFI/anyboot/sdboot/state.json");
    let text = fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
    parsed.get(key)?.as_str().map(str::to_string)
}

/// 把键值并进 state.json（保留其余键；文件不存在就新建）。
pub(crate) fn update_state_json(esp_root: &Path, update: &[(&str, serde_json::Value)]) -> Result<(), String> {
    let dir = grub_rel(esp_root, "EFI/anyboot/sdboot");
    let path = dir.join("state.json");
    let mut parsed: serde_json::Value = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| serde_json::json!({"format": "anyboot-sdboot/1"}));
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
    let body = serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| "{}".to_string());
    write_esp_bytes(&path, body.as_bytes())
}

/// 条目列表（strategy A 校验「目标 id 在列表里」也用它）。
pub(crate) fn list_entries(esp_root: &Path) -> Vec<ChannelEntry> {
    let dir = grub_rel(esp_root, "loader/entries");
    let Ok(names) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<String> = names
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".conf") && !is_utils_entry(name))
        .collect();
    files.sort(); // systemd-boot 自己也按文件名排序
    files
        .iter()
        .filter_map(|name| {
            let text = fs::read_to_string(dir.join(name)).ok()?;
            let title = entry_title(&text)
                .unwrap_or_else(|| name.trim_end_matches(".conf").to_string());
            Some(ChannelEntry {
                id: name.trim_end_matches(".conf").to_string(),
                title,
                detail: format!("loader/entries/{name}"),
                icon: "linux".to_string(),
                requires_elevation: false,
            })
        })
        .collect()
}

/// 组本通道的展示视图（ESP 未挂/没有 sd-boot 时 detected=false）。
pub(crate) fn view(esp_root: &Path) -> ChannelView {
    let loader_conf = grub_rel(esp_root, "loader/loader.conf");
    let conf_text = fs::read_to_string(&loader_conf).ok();
    let entries = list_entries(esp_root);
    let detected = conf_text.is_some() || !entries.is_empty();
    if !detected {
        return ChannelView {
            id: "sdboot",
            label: "systemd-boot",
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
    let default_value = conf_text.as_deref().and_then(loader_conf_default);
    let current_default = default_value.as_ref().map(|value| {
        if value == "@saved" {
            "@saved（记忆上次）".to_string()
        } else {
            value.clone()
        }
    });
    let last_boot = state_json_key(esp_root, "lastBooted");
    let last_boot_at = state_json_key(esp_root, "lastBootedAt");
    let mut health = Vec::new();
    if default_value.is_none() {
        health.push(HealthIssue {
            severity: "warn".to_string(),
            text: "loader.conf 没有 default 行（每次开机都会弹菜单等选择）".to_string(),
        });
    }
    if default_value.as_deref() == Some("@saved") {
        health.push(HealthIssue {
            severity: "warn".to_string(),
            text: "default=@saved：systemd-boot 自己记忆上次启动项，AnyBoot 改写前会先取消它"
                .to_string(),
        });
    }
    ChannelView {
        id: "sdboot",
        label: "systemd-boot",
        detected: true,
        permission: "file",
        chip: if health.is_empty() { "ok" } else { "warn" },
        detail: Some(format!(
            "loader/loader.conf · default={} · {} 个条目",
            default_value.as_deref().unwrap_or("（无）"),
            entries.len()
        )),
        health,
        entries,
        current_default,
        last_boot,
        last_boot_at,
        error: None,
    }
}

/// 策略A：把 default 改成 entry_id（免提权；提权需求由 write_esp_bytes 自己判断）。
pub(crate) fn set_default(esp_root: &Path, entry_id: &str) -> Result<(), String> {
    let entries = list_entries(esp_root);
    if entries.is_empty() {
        return Err("loader/entries/ 下没有可用条目，拒绝改写".to_string());
    }
    if !entries.iter().any(|entry| entry.id == entry_id) {
        return Err(format!(
            "条目 {entry_id} 不在 systemd-boot 的条目列表里（共 {} 个），拒绝改写",
            entries.len()
        ));
    }
    let loader_conf = grub_rel(esp_root, "loader/loader.conf");
    // 没有 loader.conf：视同空文件（新建）
    let original = fs::read_to_string(&loader_conf).unwrap_or_default();
    // 备份原值（含原 default 与时间），再写 state.json 的 originalDefault
    let original_default = loader_conf_default(&original);
    let backup = format!(
        "{}\n（AnyBoot 改写于 {}）\n",
        original_default
            .as_ref()
            .unwrap_or(&"（无 default 行）".to_string()),
        anyboot_now_iso()
    );
    write_esp_bytes(&grub_rel(esp_root, "EFI/anyboot/sdboot/loader-default.bak"), backup.as_bytes())?;
    update_state_json(
        esp_root,
        &[
            ("default", serde_json::Value::String(entry_id.to_string())),
            ("originalDefault", serde_json::json!(original_default)),
        ],
    )?;
    let new_text = rewrite_loader_default(&original, entry_id);
    write_esp_bytes(&loader_conf, new_text.as_bytes())
}

/// 策略B：one-shot —— 写 `LoaderEntryOneShot`，只影响**下一次**启动，过后自动失效。
/// 校验与策略A同一套（id 必须真实存在）。
pub(crate) fn one_shot(esp_root: &Path, entry_id: &str) -> Result<(), String> {
    let entries = list_entries(esp_root);
    if !entries.iter().any(|entry| entry.id == entry_id) {
        return Err(format!("条目 {entry_id} 不在 systemd-boot 的条目列表里，拒绝写 NVRAM"));
    }
    nvram::write_var("LoaderEntryOneShot", nvram::GUID_SYSTEMD, entry_id.as_bytes())
        .map_err(|error| error.message())?;
    update_state_json(
        esp_root,
        &[
            ("oneShot", serde_json::Value::String(entry_id.to_string())),
            (
                "lastBootedAt",
                serde_json::Value::String(anyboot_now_iso()),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOADER_CONF: &str = "# loader.conf 由 systemd-boot 管理
timeout 3
console-mode keep
default arch.conf
";

    #[test]
    fn parses_default_line() {
        assert_eq!(loader_conf_default(LOADER_CONF), Some("arch.conf".into()));
        assert_eq!(loader_conf_default("default @saved"), Some("@saved".into()));
        // 注释里的 default 不算
        assert_eq!(loader_conf_default("#default x\ntimeout 5"), None);
        assert_eq!(loader_conf_default(""), None);
        // default 后必须有空格/换行（defaultx 不是键）
        assert_eq!(loader_conf_default("defaultx 1"), None);
    }

    #[test]
    fn rewrite_replaces_and_is_idempotent() {
        let once = rewrite_loader_default(LOADER_CONF, "cachyos.conf");
        assert!(once.contains("default cachyos.conf"));
        assert!(once.contains("timeout 3"));
        assert!(once.contains("# loader.conf 由 systemd-boot 管理"));
        assert!(!once.contains("arch.conf"));
        // 幂等：再写一次结果不变
        assert_eq!(rewrite_loader_default(&once, "cachyos.conf"), once);
        // 换回去也行
        let back = rewrite_loader_default(&once, "arch.conf");
        assert!(back.contains("default arch.conf"));
    }

    #[test]
    fn rewrite_inserts_when_missing() {
        let text = "# 注释\nconsole-mode keep\n";
        let out = rewrite_loader_default(text, "arch.conf");
        let lines: Vec<&str> = out.lines().collect();
        // 插在首个非注释行前
        assert_eq!(lines[0], "# 注释");
        assert_eq!(lines[1], "default arch.conf");
        assert_eq!(lines[2], "console-mode keep");
        // 全注释文件：追加到末尾
        let out2 = rewrite_loader_default("# only comment\n", "a.conf");
        assert!(out2.ends_with("default a.conf\n"));
        // 空文件
        let out3 = rewrite_loader_default("", "a.conf");
        assert_eq!(out3, "default a.conf\n");
        // 重复 default 行收敛成一条
        let dup = "default old.conf\ntimeout 3\ndefault other.conf\n";
        let out4 = rewrite_loader_default(dup, "new.conf");
        assert_eq!(out4.matches("default ").count(), 1);
        assert!(out4.contains("default new.conf"));
        assert!(!out4.contains("old.conf"));
    }

    #[test]
    fn entry_title_and_utils() {
        assert_eq!(entry_title("## boot\ntitle   Arch Linux\nlinux /vmlinuz"), Some("Arch Linux".into()));
        assert_eq!(entry_title("linux /vmlinuz"), None);
        assert!(is_utils_entry("arch-utils.conf"));
        assert!(!is_utils_entry("arch.conf"));
        assert!(!is_utils_entry("arch-utils.conf.bak"));
    }
}
