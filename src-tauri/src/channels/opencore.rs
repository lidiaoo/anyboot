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
pub(crate) const OC_CONFIG: &str = "EFI/OC/config.plist";
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
            // 只管 OC 自己那套热键：AnyBoot 的「设为默认」是直接写 NVRAM（OCBt*），
            // 不经过这个开关，所以别写成「无法设默认项」，那会与卡片上的按钮打架。
            text: "Misc→Security→AllowSetDefault=false：OC 菜单里的 Ctrl+Enter 不生效，记不住默认项（建议改 true；AnyBoot 的「设为默认」直接写 NVRAM，不受它限制）"
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
///
/// `RequestBootVarRouting=true` 时 OC 的启动项存在私有命名空间（`OCBtOrder` /
/// `OCBt####`），「设默认项」动的是那一组，所以两套都备一份 —— 否则出问题时
/// 拿不到可回滚的原值。
pub(crate) fn backup_nvram(esp_root: &Path) -> Result<(), String> {
    let mut snapshot = serde_json::json!({
        "format": "anyboot-nvram-backup/2",
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
    let custom = oc_default::Namespace { custom: true };
    let mut has_custom = false;
    for number in custom.read_order() {
        if let Some(value) = custom.read_option(number) {
            let name = custom.boot_name(number);
            snapshot[name.as_str()] = serde_json::Value::String(hex::encode(value));
            has_custom = true;
        }
    }
    if let Ok(order) = nvram::read_var(custom.order_name(), custom.guid()) {
        snapshot[custom.order_name()] = serde_json::Value::String(hex::encode(order));
        has_custom = true;
    }
    if has_custom {
        snapshot["namespaces"] = serde_json::Value::String("global + oc-vendor".to_string());
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

    // 幂等：目标值与硬前提都已就位时，不备份、不写盘。
    // 否则连点两次「立即修复」就会在 ESP 上堆两份一模一样的 config 备份，
    // 看得出来用户会以为"每次点都在改东西"。
    let routing_ok =
        plist_lite::read_scalar(&original, &["UEFI", "Quirks", "RequestBootVarRouting"])
            == Some(Scalar::Bool(true));
    let flash_ok =
        plist_lite::read_scalar(&original, &["NVRAM", "WriteFlash"]) != Some(Scalar::Bool(false));
    if previous.as_deref() == Some(target) && routing_ok && flash_ok {
        return Ok(format!(
            "LauncherOption 已经是 {target}，硬前提也都在位——无需改动（没有写盘、也没有新增备份）"
        ));
    }

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
    message.push_str("。重启一次进入 OpenCore 后它才会把启动项写进固件");
    if cfg!(target_os = "macos") {
        // macOS 侧既读不到也写不了固件启动列表（Apple 的变量白名单，root 与 bless 都一样），
        // 承诺「下次启动可用」是空头支票，不如直接给可行的做法。
        message.push_str(
            "。macOS 侧看不到、也改不了固件启动列表——想设默认项，请在 OC 菜单里对目标系统按 Ctrl+Enter",
        );
    } else {
        message.push_str("，之后「下次启动」就能直接指向 OpenCore");
    }
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

/// 「直接写固件启动项」：AnyBoot 自己构造 `EFI_LOAD_OPTION` 写 `Boot####` + `BootOrder`。
///
/// 为什么需要这条路：`LauncherOption` 只在 **OC 自己正在运行**的那一瞬间生效，实测在
/// 靠 `\EFI\BOOT\BOOTX64.EFI`（Bootstrap）兜底的机器上，即使配成 `Full` 也什么都没写进
/// 固件 —— `BootOrder` 里始终只有 macOS 那一项。直接写不依赖 OC 的版本、不依赖
/// `RequestBootVarRouting` 的代理，改完立刻生效（与 macOS `bless` 建 `Boot0080` 是同一条
/// NVRAM 写入路径，所以可行性已经被实机证明）。
///
/// `mode=full` → 插到 BootOrder 最前（抢默认启动）；缺省 / `short` → 追加到末尾
/// （固件启动菜单里能看到它，但默认启动项仍由 AnyBoot 决定）。
pub(crate) fn register_boot_entry(esp_root: &Path, mode: &str) -> Result<String, String> {
    if !nvram::can_write_boot_vars() {
        return Err(
            "本系统不允许用户态写 NVRAM 的 Boot* 变量（macOS 的 EFI 运行时白名单，管理员权限也一样），\
             没法由 AnyBoot 直接建启动项"
                .to_string(),
        );
    }
    if let Some(number) = find_boot_number(OC_MARKER) {
        return Ok(format!(
            "固件启动列表里已经有 OpenCore（Boot{}），不需要重复写入",
            nvram::boot_var_name(number)
        ));
    }
    if !grub_rel(esp_root, OC_EFI).is_file() {
        return Err(format!("{OC_EFI} 不存在，这块 ESP 上没有 OpenCore"));
    }
    let partition = boot_entry::partition_info(esp_root)?;
    let order = nvram::read_boot_order();
    let number = boot_entry::next_free_number(&order, |number| {
        nvram::read_boot_option(number).is_some()
    })
    .ok_or_else(|| "固件里找不到空闲的 Boot#### 编号".to_string())?;
    let value = boot_entry::build_load_option("OpenCore", &partition, "\\EFI\\OC\\OpenCore.efi");
    let grab_default = mode.trim().to_ascii_lowercase() == "full";
    let new_order = boot_entry::boot_order_with(&order, number, grab_default);

    // 动 NVRAM 之前先整份备份（BootOrder + 全部 Boot####，留最近 10 份）
    backup_nvram(esp_root)?;
    let boot_name = nvram::boot_var_name(number);
    let order_value = boot_entry::encode_boot_order(&new_order);
    nvram::write_vars(&[
        (boot_name.as_str(), nvram::GUID_GLOBAL, value.as_slice()),
        ("BootOrder", nvram::GUID_GLOBAL, order_value.as_slice()),
    ])
    .map_err(|error| error.message())?;

    // 回读校验：固件可能静默丢弃写入（某些主板的 NVRAM 满了、或策略禁止改 Boot*）
    let written = nvram::read_boot_option(number)
        .ok_or_else(|| format!("写完之后读不回 {boot_name}，固件拒绝了这次写入"))?;
    if !nvram::load_option_matches(&written, OC_MARKER) {
        return Err(format!(
            "{boot_name} 回读内容不含 OpenCore 路径，写入没有落到固件"
        ));
    }
    if !nvram::read_boot_order().contains(&number) {
        return Err(format!(
            "{boot_name} 写进去了，但 BootOrder 里没有它（固件拒绝改启动顺序）"
        ));
    }

    let _ = update_channel_state(
        esp_root,
        "opencore",
        &[
            ("registeredBoot", serde_json::Value::String(boot_name.clone())),
            (
                "registeredAt",
                serde_json::Value::String(anyboot_now_iso()),
            ),
        ],
    );

    Ok(format!(
        "已把 OpenCore 写进固件启动列表（{boot_name}{}），现在「下次启动进入 OpenCore」可以用了",
        if grab_default {
            "，并设为首选启动项"
        } else {
            ""
        }
    ))
}

/// 「修复并注册」的统一入口。
///
/// 顺序是「先动手、再改配置」：能直接写进固件就立刻生效（卡片当场变绿）；
/// 写不进去（固件拒绝 / 拿不到分区信息）才退回让 OC 自己注册 —— 那条路要重启一次
/// 才可能生效，且实测不一定成，只能兜底。
pub(crate) fn register(esp_root: &Path, mode: &str) -> Result<String, String> {
    match register_boot_entry(esp_root, mode) {
        Ok(message) => Ok(message),
        Err(error) => {
            let fallback = fix_launcher_option(esp_root, mode)?;
            Ok(format!(
                "直接写固件启动项没成功（{error}）。已改为让 OpenCore 自己注册：{fallback}"
            ))
        }
    }
}

/// 写 BootNext 指向 `marker`（OC/Clover 共用）对应的启动项。找到 → 写；没找到 →
/// 报错提示（创建 Boot#### 的 DevicePath 构造在后续版本补，先不赌固件兼容性）。
pub(crate) fn set_boot_next(esp_root: &Path, marker: &str, channel: &str) -> Result<u16, String> {
    // 先判平台能力：macOS 上写 Boot* 会被系统白名单直接拒绝（root 也一样），
    // 与其写完报 `not permitted`，不如在这里说清原因和替代方案。
    if !nvram::can_write_boot_vars() {
        return Err(format!(
            "本系统不允许用户态写 NVRAM 的 Boot* 变量（macOS 的 EFI 运行时白名单，管理员权限也一样），\
             没法用 BootNext 让下次启动进入 {channel}。请在引导器菜单里对目标系统按 Ctrl+Enter 设成默认，\
             或在 Linux / Windows 侧用同样的切换"
        ));
    }
    let Some(number) = find_boot_number(marker) else {
        return Err(format!(
            "固件启动列表里没有引导项（Boot#### 路径不含 {marker}）。\
             请先点「写入固件启动项」把它写进固件，再切通道"
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
            menu_preview: None,
        };
    }
    let config = config_text.unwrap_or_default();
    let mut health = health_checks(&config);
    let launcher = launcher_option(&config);
    let can_self_register = self_registers(launcher.as_deref());
    // 本平台能不能写 Boot*（macOS 恒不能：Apple 的 NVRAM 白名单，root 也被拒）
    let can_write = nvram::can_write_boot_vars();
    // 「设默认项」能不能做：写不了 Boot* 时，OC 的私有命名空间（OCBt*）仍是可写的，
    // 那条路就是预演行里的「设为默认」按钮（= OC 菜单的 Ctrl+Enter）。
    let can_set_default = oc_default::Namespace::from_config(&config).writable();
    // OS 侧看得见真实启动列表吗（OpenCore 的 RequestBootVarRouting 会把 Boot* 藏起来）
    let hidden = nvram::boot_vars_hidden();
    let found = find_boot_number(OC_MARKER);
    // 判定"固件里有没有 OC 项"：
    //   * 看得见列表 → 以 BootOrder 为准；
    //   * 看不见（BootCurrent 都没有）→ **不能**断定没有。实机就是这样：BIOS 启动
    //     菜单里明明有 OpenCore 项、也确实能从它启动，而 macOS 侧只剩 macOS 那一条。
    //     此时以「OC 被配置成自注册」为依据，并把「不可见」这件事如实写出来。
    let registered = found.is_some() || (hidden && can_self_register);
    if hidden || !can_write {
        // 这几件事对用户是同一个问题——「为什么在这台机器上只能看不能改」。
        // 但要拆成多条短句：原来拼成一整段，卡片里就是一坨灰字，重点全埋掉了。
        let mut reasons: Vec<String> = Vec::new();
        if hidden {
            reasons.push(
                "启动列表在本系统不可见（读不到 BootCurrent，Boot* 被引导器托管了）".to_string(),
            );
        }
        if !can_write {
            reasons.push(
                "本系统不允许用户态写 Boot* 变量（macOS 的 EFI 运行时白名单，管理员权限也一样）"
                    .to_string(),
            );
        }
        health.push(HealthIssue {
            severity: "info".to_string(),
            text: format!("{}。", reasons.join("；")),
        });
        health.push(HealthIssue {
            severity: "info".to_string(),
            text: format!(
                "所以 OpenCore 在固件里的注册状态以 BIOS / 引导器菜单为准{}",
                if can_self_register {
                    "（LauncherOption 已开，OC 会自己注册）"
                } else {
                    "（LauncherOption 没开，OC 不会自己注册）"
                }
            ),
        });
        // 「默认启动项」是这里唯一还能改的东西：OC 把启动项托管到私有命名空间
        // （OCBt*），那组变量不归 macOS 的 Boot* 白名单管（实测 root 可写、可回读）。
        // 只有在 RequestBootVarRouting 关着、写不了私有命名空间时，才退回 Ctrl+Enter。
        health.push(HealthIssue {
            severity: "info".to_string(),
            text: if can_set_default {
                "默认启动项仍然可以在这里直接设（等价于 OC 菜单里的 Ctrl+Enter）：\
                 点下方预演行末尾的「设为默认」"
                    .to_string()
            } else {
                "想固定默认启动项，只能在 OC 菜单里对目标按 Ctrl+Enter；\
                 把 UEFI→Quirks→RequestBootVarRouting 改成 true 后，OC 会改用私有命名空间\
                 （OCBt*），这里就能直接设"
                    .to_string()
            },
        });
    }
    if !registered {
        let launcher_text = launcher.as_deref().unwrap_or("未设置");
        health.push(HealthIssue {
            severity: "warn".to_string(),
            text: format!(
                "固件启动列表里没有 OpenCore 项，且 LauncherOption={launcher_text}（OC 不自注册）——\
                 进 OC 菜单把 Misc→Boot→LauncherOption 改成 Short/Full，重启一次后它自己会写进去"
            ),
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
    // OC 的子系统条目由 OC picker 自己管，AnyBoot 固定一条「进菜单」动作。
    // 两个动作都要写 Boot* 变量，所以只有能写的平台才给按钮——
    // macOS 上给了也只会让人点了收到 `not permitted`。
    let mut entries = Vec::new();
    if can_write {
        if !registered {
            entries.push(ChannelEntry {
                id: "opencore-register".to_string(),
                title: "写入固件启动项".to_string(),
                detail: "由 AnyBoot 构造 EFI 启动项写进 NVRAM（HardDrive 节点取本 ESP 的分区号/\
                         起始 LBA/GPT GUID），写完立刻生效；写前备份 BootOrder 与全部 Boot####"
                    .to_string(),
                icon: "menu".to_string(),
                requires_elevation: true,
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
    }
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
        // 菜单预演：纯读（卷 + 预定义路径 + Preboot + BootOrder），不写盘、不碰 NVRAM
        menu_preview: Some(oc_menu::preview(esp_root)),
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

    /// 真机自检（`cargo test --lib -- --ignored real_esp_view --nocapture`）。
    /// 钉住 macOS 上这张卡片的形态：列表不可见 + 写不了 Boot* 时，应当是「绿 chip +
    /// 三条 info 说明（原因 / 注册状态 / 还能设默认项）+ 零个按钮」，而不是把残缺的
    /// BootOrder 当成「固件里没有 OC 项」。
    #[test]
    #[ignore]
    fn real_esp_view() {
        let root = Path::new("/Volumes/MacOS-efi");
        if !root.is_dir() {
            eprintln!("没有 {}，跳过", root.display());
            return;
        }
        let view = super::view(root);
        println!("chip={}", view.chip);
        println!(
            "entries={:?}",
            view.entries.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>()
        );
        for issue in &view.health {
            println!("[{}] {}", issue.severity, issue.text);
        }
        println!(
            "can_write={} hidden={}",
            nvram::can_write_boot_vars(),
            nvram::boot_vars_hidden()
        );
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
        // 再跑一次：已经是目标值 → 成功、但不写盘、不新增备份、不重复插键
        let again_message = fix_launcher_option(&root, "full").expect("幂等重跑应成功");
        assert!(
            again_message.contains("无需改动"),
            "幂等重跑该明说没动东西：{again_message}"
        );
        let again = fs::read_to_string(root.join("EFI/OC/config.plist")).unwrap();
        assert_eq!(again, after, "幂等重跑不应改动文件内容");
        assert_eq!(again.matches("<key>LauncherOption</key>").count(), 1);
        assert_eq!(again.matches("<key>RequestBootVarRouting</key>").count(), 1);
        let backups = fs::read_dir(root.join("EFI/anyboot/backup"))
            .unwrap()
            .flatten()
            .count();
        assert_eq!(backups, 1, "幂等重跑不该堆备份（连点两次只该有一份）");
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
