//! 三引导器通道（systemd-boot / OpenCore / Clover）的聚合层。
//!
//! 与 GRUB/rEFInd 那两条链路并列的第三、四、五条切换通道。设计哲学一致：
//! **状态尽量外置到 ESP（FAT32），任何系统免提权读写**——
//! * systemd-boot：`loader/loader.conf` 的 default 行（文件）；
//! * OpenCore：NVRAM `BootNext`（必经提权），状态导出到 `EFI/anyboot/oc/state.json`；
//! * Clover：`config.plist` 的 DefaultVolume（文件）/ BootNext（实验性）。
//!
//! 切换值域（`switch_system` 与远程命令共用）：`sdboot:<id>` /
//! `sdboot-oneshot:<id>` / `opencore` / `opencore-register[:short|:full]` /
//! `clover-default:<volume>` / `clover-oneshot`。

use super::*;

pub(crate) mod boot_entry;
pub(crate) mod clover;
pub(crate) mod nvram;
pub(crate) mod opencore;
pub(crate) mod plist_lite;
pub(crate) mod sdboot;

// ─────────────────────────── 快照结构 ───────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HealthIssue {
    /// warn（目前只有这一级；预留 error/info）
    pub(crate) severity: String,
    pub(crate) text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelEntry {
    /// sdboot: entry 文件名去 .conf；oc/clover: 动作 id
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) detail: String,
    /// 前端选图标用：linux/windows/macos/menu
    pub(crate) icon: String,
    pub(crate) requires_elevation: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelView {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) detected: bool,
    /// "file" = 免提权可切换；"nvram" = 切换必经 NVRAM 写（UI 渲染 ⤴）
    pub(crate) permission: &'static str,
    /// ok / warn / off —— 对应 UI 状态 chip
    pub(crate) chip: &'static str,
    pub(crate) detail: Option<String>,
    pub(crate) health: Vec<HealthIssue>,
    pub(crate) entries: Vec<ChannelEntry>,
    pub(crate) current_default: Option<String>,
    /// 记忆回读（「上次启动」）
    pub(crate) last_boot: Option<String>,
    pub(crate) last_boot_at: Option<String>,
    pub(crate) error: Option<String>,
}

/// channels 链路的本端运行时状态（与 `GrubRuntime` 完全并列）。
#[derive(Clone, Default)]
pub(crate) struct ChannelsRuntime {
    /// 复用 GRUB 链路已解析的 ESP（三通道共用同一块）。目前视图里用不到它，
    /// 留着给后续「分区信息展示 / 跨通道写盘」用。
    #[allow(dead_code)]
    pub(crate) esp: Option<EspInfo>,
    pub(crate) views: Vec<ChannelView>,
}

// ─────────────────────────── 时间工具 ───────────────────────────

/// UTC ISO8601（无 chrono 依赖，civil-from-days；与 backup::iso_now 同算法）。
pub(crate) fn anyboot_now_iso() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    if ms == 0 {
        return String::new();
    }
    let days = (ms / 86_400_000) as i64;
    let rem = ms % 86_400_000;
    let (h, m, s) = (rem / 3_600_000, (rem / 60_000) % 60, (rem / 1000) % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

// ─────────────────────────── ESP 定位 ───────────────────────────

/// 目录上有没有任一通道的标志性文件（是块挂载着的 ESP 且确实在用）。
fn has_channel_signature(root: &Path) -> bool {
    grub_rel(root, "loader/loader.conf").is_file()
        || grub_rel(root, "EFI/OC/OpenCore.efi").is_file()
        || grub_rel(root, "EFI/CLOVER/CLOVERX64.efi").is_file()
}

/// 平台各自的常见挂载点（GRUB 链路没解析出 ESP 时的兜底探测）。
#[cfg(target_os = "linux")]
fn candidate_roots() -> Vec<PathBuf> {
    ["/efi", "/boot/efi", "/boot", "/esp"]
        .iter()
        .map(PathBuf::from)
        .collect()
}

#[cfg(target_os = "macos")]
fn candidate_roots() -> Vec<PathBuf> {
    fs::read_dir("/Volumes")
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn candidate_roots() -> Vec<PathBuf> {
    (b'A'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn candidate_roots() -> Vec<PathBuf> {
    Vec::new()
}

/// 三通道共用的 ESP 根目录。优先级：GRUB 链路已解析的 → 常见挂载点探测。
/// （不引入第二个 ESP 探测实现：识别信息照抄 grub 链路的 EspInfo。）
pub(crate) fn resolve_esp_root(app: &AppHandle) -> Option<(Option<EspInfo>, PathBuf)> {
    {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        if let Some(dir) = &state.grub.efi_dir {
            if has_channel_signature(dir) {
                return Some((state.grub.esp.clone(), dir.clone()));
            }
        }
    }
    candidate_roots()
        .into_iter()
        .find(|root| has_channel_signature(root))
        .map(|root| {
            let info = esp_probe(&root).ok();
            (info, root)
        })
}

// ─────────────────────────── 刷新与快照 ───────────────────────────

/// 没找到 ESP 时的全 off 视图（与 GRUB 空态同风格：照实说原因）。
fn offline_view(id: &'static str, label: &'static str, permission: &'static str) -> ChannelView {
    ChannelView {
        id,
        label,
        detected: false,
        permission,
        chip: "off",
        detail: Some("没有找到挂载着的 ESP（或上面没有这个引导器）".to_string()),
        health: Vec::new(),
        entries: Vec::new(),
        current_default: None,
        last_boot: None,
        last_boot_at: None,
        error: None,
    }
}

/// 重算三通道视图并写回 AppState.channels（refresh_and_update 里 GRUB 刷新之后调）。
/// 叫 refresh_channels 是因为 switch_flow::refresh（tauri 命令）经 glob 会撞名。
pub(crate) fn refresh_channels(app: &AppHandle) {
    let (esp, views) = match resolve_esp_root(app) {
        Some((esp, root)) => {
            let views = vec![
                sdboot::view(&root),
                opencore::view(&root),
                clover::view(&root),
            ];
            (esp, views)
        }
        None => {
            let views = vec![
                offline_view("sdboot", "systemd-boot", "file"),
                offline_view("opencore", "OpenCore", "nvram"),
                offline_view("clover", "Clover", "file"),
            ];
            (None, views)
        }
    };
    let state = app.state::<Mutex<AppState>>();
    state.lock().expect("state mutex poisoned").channels = ChannelsRuntime { esp, views };
}

// ─────────────────────────── 切换 ───────────────────────────

/// 切换值域的解析（纯函数，好单测）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SwitchSpec {
    Sdboot(String),
    SdbootOneShot(String),
    OpenCore,
    /// 修复 OC 的自注册配置：`short`（默认，不抢默认位）/ `full`
    OpenCoreRegister(String),
    CloverDefault(String),
    CloverOneShot,
}

pub(crate) fn parse_switch_spec(spec: &str) -> Option<SwitchSpec> {
    let (head, rest) = match spec.split_once(':') {
        Some((head, rest)) => (head, Some(rest)),
        None => (spec, None),
    };
    match (head, rest) {
        ("sdboot", Some(id)) if !id.trim().is_empty() => {
            Some(SwitchSpec::Sdboot(id.trim().to_string()))
        }
        ("sdboot-oneshot", Some(id)) if !id.trim().is_empty() => {
            Some(SwitchSpec::SdbootOneShot(id.trim().to_string()))
        }
        ("opencore", None | Some("")) => Some(SwitchSpec::OpenCore),
        // 缺省 = short：不抢 BootOrder 默认位，比 Full 温和
        ("opencore-register", None | Some("")) => {
            Some(SwitchSpec::OpenCoreRegister("short".to_string()))
        }
        ("opencore-register", Some(mode))
            if matches!(mode.trim().to_ascii_lowercase().as_str(), "short" | "full") =>
        {
            Some(SwitchSpec::OpenCoreRegister(
                mode.trim().to_ascii_lowercase(),
            ))
        }
        ("clover-default", Some(volume)) if !volume.trim().is_empty() => {
            Some(SwitchSpec::CloverDefault(volume.trim().to_string()))
        }
        ("clover-oneshot", None | Some("")) => Some(SwitchSpec::CloverOneShot),
        _ => None,
    }
}

impl SwitchSpec {
    pub(crate) fn is_channel_spec(spec: &str) -> bool {
        parse_switch_spec(spec).is_some()
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Sdboot(id) => format!("systemd-boot 下次启动：{id}"),
            Self::SdbootOneShot(id) => format!("systemd-boot 一次性启动：{id}"),
            Self::OpenCore => "下次启动进入 OpenCore 菜单".to_string(),
            Self::OpenCoreRegister(mode) => format!(
                "把 OpenCore 写进固件启动项（BootOrder {}）",
                if mode == "full" { "首位" } else { "末尾" }
            ),
            Self::CloverDefault(volume) => format!("Clover 默认卷设为 {volume}"),
            Self::CloverOneShot => "下次启动进入 Clover（一次性）".to_string(),
        }
    }

    /// 动作名（写操作记录用）：「注册」与「切换」不是一回事。
    pub(crate) fn action_label(&self) -> &'static str {
        match self {
            Self::OpenCoreRegister(_) => "写入固件启动项",
            _ => "切换引导通道",
        }
    }
}

/// 执行一次通道切换（不动 busy / 不发布快照，调用方管）。
pub(crate) fn switch(app: &AppHandle, spec: &str) -> Result<String, String> {
    let parsed =
        parse_switch_spec(spec).ok_or_else(|| format!("未知的引导通道值域: {spec}"))?;
    let (_esp, esp_root) = resolve_esp_root(app)
        .ok_or_else(|| "没有找到可用的 ESP（三条通道都需要它）".to_string())?;
    let message = match &parsed {
        SwitchSpec::Sdboot(id) => {
            sdboot::set_default(&esp_root, id)?;
            parsed.describe()
        }
        SwitchSpec::SdbootOneShot(id) => {
            sdboot::one_shot(&esp_root, id)?;
            parsed.describe()
        }
        SwitchSpec::OpenCore => {
            let number = opencore::set_boot_next(&esp_root, opencore::OC_MARKER, "opencore")?;
            format!("{}（Boot{}，用后自动失效）", parsed.describe(), nvram::boot_var_name(number))
        }
        // 「注册」不是切换：直接把注册结果的完整说明当消息
        SwitchSpec::OpenCoreRegister(mode) => opencore::register(&esp_root, mode)?,
        SwitchSpec::CloverDefault(volume) => {
            clover::set_default_volume(&esp_root, volume, None)?;
            parsed.describe()
        }
        SwitchSpec::CloverOneShot => {
            let number = opencore::set_boot_next(&esp_root, clover::CLOVER_MARKER, "clover")?;
            format!("{}（Boot{}）", parsed.describe(), nvram::boot_var_name(number))
        }
    };
    Ok(message)
}

/// 切换引导通道的统一入口（镜像 switch_grub_and_update：busy + record + publish）。
pub(crate) fn switch_channel_and_update(app: &AppHandle, spec: &str) -> AppStatus {
    trace(app, "channel", &format!("执行通道动作 spec={spec}"));
    let label = parse_switch_spec(spec)
        .map(|parsed| parsed.action_label())
        .unwrap_or("切换引导通道");
    set_busy(app, |busy| busy.switching = true);
    let outcome = switch(app, spec);
    set_busy(app, |busy| busy.switching = false);
    match outcome {
        Ok(message) => {
            refresh_channels(app);
            record_action(app, label, true, &message);
            publish_ui(app);
            state_snapshot(app)
        }
        Err(error) => {
            record_action(app, label, false, &error);
            update_failure_status(app, &format!("{label}失败"), error)
        }
    }
}

// ─────────────────────────── 命令面 ───────────────────────────

#[tauri::command]
pub(crate) fn boot_channels(app: AppHandle) -> Vec<ChannelView> {
    app.state::<Mutex<AppState>>()
        .lock()
        .expect("state mutex poisoned")
        .channels
        .views
        .clone()
}

#[tauri::command]
pub(crate) fn switch_boot_channel(app: AppHandle, target: String) -> AppStatus {
    trace(&app, "command", &format!("switch_boot_channel target={target}"));
    switch_channel_and_update(&app, &target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_spec_parsing() {
        assert_eq!(
            parse_switch_spec("sdboot:arch.conf"),
            Some(SwitchSpec::Sdboot("arch.conf".into()))
        );
        assert_eq!(
            parse_switch_spec("sdboot-oneshot:cachyos.conf"),
            Some(SwitchSpec::SdbootOneShot("cachyos.conf".into()))
        );
        assert_eq!(parse_switch_spec("opencore"), Some(SwitchSpec::OpenCore));
        // 修复动作：缺省 short（温和，不抢默认位）
        assert_eq!(
            parse_switch_spec("opencore-register"),
            Some(SwitchSpec::OpenCoreRegister("short".into()))
        );
        assert_eq!(
            parse_switch_spec("opencore-register:full"),
            Some(SwitchSpec::OpenCoreRegister("full".into()))
        );
        assert_eq!(
            parse_switch_spec("opencore-register:SHORT"),
            Some(SwitchSpec::OpenCoreRegister("short".into())),
            "模式名大小写不敏感"
        );
        assert_eq!(parse_switch_spec("opencore-register:disabled"), None);
        assert_eq!(
            parse_switch_spec("clover-default:Macintosh HD"),
            Some(SwitchSpec::CloverDefault("Macintosh HD".into()))
        );
        assert_eq!(
            parse_switch_spec("clover-oneshot"),
            Some(SwitchSpec::CloverOneShot)
        );
        // 旧值域与新值域不冲突；空 id / 未知前缀拒绝
        assert_eq!(parse_switch_spec("linux"), None);
        assert_eq!(parse_switch_spec("sdboot:"), None);
        assert_eq!(parse_switch_spec("sdboot"), None);
        assert_eq!(parse_switch_spec("clover-default:"), None);
        assert_eq!(parse_switch_spec("opencore:extra"), None);
        assert_eq!(parse_switch_spec("windows"), None);
        assert!(SwitchSpec::is_channel_spec("sdboot:arch.conf"));
        assert!(SwitchSpec::is_channel_spec("opencore-register"));
        assert!(!SwitchSpec::is_channel_spec("macos"));
        // 注册类动作写操作记录时用「写入固件启动项」而不是「切换」
        assert_eq!(
            SwitchSpec::OpenCoreRegister("short".into()).action_label(),
            "写入固件启动项"
        );
        assert_eq!(SwitchSpec::OpenCore.action_label(), "切换引导通道");
        // describe 要说清插到 BootOrder 哪儿（用户能预期默认启动项会不会变）
        assert!(SwitchSpec::OpenCoreRegister("full".into())
            .describe()
            .contains("首位"));
        assert!(SwitchSpec::OpenCoreRegister("short".into())
            .describe()
            .contains("末尾"));
    }
}
