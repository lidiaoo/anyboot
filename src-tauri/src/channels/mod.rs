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
/// ext4 极简解析：自动读取模式（不装 09_anyboot）直读 Linux 分区的底层
pub(crate) mod ext4_raw;
/// GRUB「自动读取」通道：读 Linux 分区里的 grub.cfg / grubenv，与手动解析并列
pub(crate) mod linux_grub;
/// 分区标识（UUID / PARTUUID / LABEL / /dev/sda1）的统一解析
pub(crate) mod part_spec;
// macOS 原生读得到 /System/Volumes/Preboot，不需要这个模块（依赖也只装在另两端）
#[cfg(not(target_os = "macos"))]
pub(crate) mod apfs_reader;
// `test` 也放进来：命令行拼接（`quote_arg`）是纯字符串逻辑，得能在开发机上单测 ——
// 它出错的样子只会在真机上表现为「参数串位」，那是没法靠肉眼 review 兜住的。
#[cfg(any(target_os = "windows", target_os = "linux", test))]
pub(crate) mod elevate;
pub(crate) mod nvram;
pub(crate) mod oc_default;
pub(crate) mod oc_menu;
pub(crate) mod opencore;
pub(crate) mod partitions;
pub(crate) mod plist_lite;
/// rEFInd「模拟引导项扫描」：与 `scan.rs` 的「手动获取」（vars 快照）并列的另一条只读链路
pub(crate) mod refind_menu;
pub(crate) mod sdboot;
pub(crate) mod win_esp;

// ─────────────────────────── 提权方式 ───────────────────────────

/// 这次提权是**谁发起的** —— 决定「能不能弹授权框」。两个调用方要的语义完全不同，
/// 用一个 bool（`allow_fallback`）传会一路读成「要不要回落」，实际约束是：
///
/// * [`Elevation::Allow`]：用户亲手点的按钮。服务优先，服务不可用就弹一次授权；
/// * [`Elevation::Silent`]：开机 / 进页面**自动**扫。只允许静默通道
///   （Windows 特权服务、macOS 免密助手），拿不到就报错 —— 绝不弹授权。
///
/// 为什么自动扫必须这么严：开机那一拍没人盯着屏幕，UAC 框要么被无视（30 秒后失败），
/// 要么在最不该打扰的时候弹出来；远程/无人值守更是永远点不到。代价是服务/助手不在位
/// 时自动扫一定失败，所以界面上始终留着会弹授权的手动按钮。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Elevation {
    Allow,
    Silent,
}

impl Elevation {
    /// 前端传的 `auto`（true = 开机自动扫那些调用）→ 提权方式。
    pub(crate) fn from_auto(auto: bool) -> Self {
        if auto {
            Self::Silent
        } else {
            Self::Allow
        }
    }

    pub(crate) fn is_silent(self) -> bool {
        self == Self::Silent
    }
}

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
    /// OpenCore 专有：按 OC 自己的规则复算出来的菜单（纯读）。
    /// 其他通道恒 `None`。
    pub(crate) menu_preview: Option<oc_menu::MenuPreview>,
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

/// 把特权子进程（或特权服务）算出来的那段 JSON **原子**落盘：临时文件 + rename。
///
/// 为什么共用一份：五个特权子命令原来各写一遍「tmp + rename + 失败清理」，
/// 任何一份写成直接 `write` 都会让父进程有机会读到半截 JSON —— 那种失败只在
/// 机器卡顿时出现，属于最难复现的一类。现在文件路径与管道路径都产同一段文本，
/// 落盘也只该有一处。
///
/// `#[cfg(unix)]` 那段权限是给 macOS 的：子进程是 root，写出来的文件属主也是
/// root，不放开读权限的话发起请求的用户读不到自己的结果。
///
/// 写临时文件时带 `O_NOFOLLOW | O_EXCL`：临时文件名是「结果路径 + pid」，可预测，
/// 而结果路径通常落在世界可写的 `/tmp`。不带这两个标志的话，别人可以先建一个
/// 同名符号链接，让我们以 **root** 身份往他指定的文件里写这段 JSON。
pub(crate) fn write_result_file(out_path: &str, text: &str) -> i32 {
    let tmp = std::path::PathBuf::from(format!("{out_path}.tmp-{}", std::process::id()));
    if let Err(error) = write_new_file(&tmp, text) {
        eprintln!("写结果失败：{error}");
        return 2;
    }
    if let Err(error) = std::fs::rename(&tmp, out_path) {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("落盘结果失败：{error}");
        return 2;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(out_path, std::fs::Permissions::from_mode(0o644));
    }
    0
}

/// 只新建、不跟随符号链接地写一个文件（跨平台：非 unix 退化成普通写）。
fn write_new_file(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644).custom_flags(libc::O_NOFOLLOW);
    }
    // create_new 遇到同名残留会失败；那是上一次崩溃留下的，删掉再来一次
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(path)?;
            options.open(path)?
        }
        Err(error) => return Err(error),
    };
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// UTC ISO8601（无 chrono 依赖，civil-from-days；与 backup::iso_now 同算法）。
pub(crate) fn anyboot_now_iso() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    if ms == 0 {
        return String::new();
    }
    anyboot_iso_at(ms)
}

/// 把「毫秒时间戳」格式化成同一个 UTC ISO8601。
///
/// 分开一个函数是为了展示**文件修改时间**（`grub.cfg` 的 mtime）——自动读取
/// 模式没有同步动作，用户必须能看出这份菜单是哪天生成的。
pub(crate) fn anyboot_iso_at(ms: u64) -> String {
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

/// 解析「这次特权操作作用于哪块 ESP」：优先用调用方点名的那块（**必须真的带通道
/// 签名**），拿不到才按候选列表挑第一块带签名的。
///
/// 为什么特权路径上必须过这一关：`esp` 是请求里唯一一个「路径」形态的参数，而
/// 特权进程（SYSTEM 服务 / 提权子进程）会往它下面写 `EFI/anyboot/...` 与 NVRAM
/// 备份。不校验的话，一个路径参数就成了「让 SYSTEM 在任意目录建文件」。签名校验
/// 把可写范围压回「确实装过引导器的卷」；Windows 上再叠一条「必须是卷根」
/// （见 [`esp_root_is_volume_root`]），因为那里的 ESP 根永远是 `X:\` 这种形状。
pub(crate) fn resolve_esp_root_arg(esp: Option<&str>) -> Result<PathBuf, String> {
    let root = esp
        .map(PathBuf::from)
        .filter(|root| has_channel_signature(root))
        .or_else(|| candidate_roots().into_iter().find(|root| has_channel_signature(root)))
        .ok_or_else(|| "没有找到可用的 ESP（三条通道都需要它）".to_string())?;
    #[cfg(target_os = "windows")]
    if !esp_root_is_volume_root(&root) {
        return Err(format!(
            "只接受卷根形态的 ESP 路径（{}），不接受它下面的子目录",
            root.display()
        ));
    }
    Ok(root)
}

/// ESP 根在 Windows 上必须是卷根（`H:\`）：`H:\EFI` 这种子目录不是「一块 ESP」，
/// 拿它当根会让 `EFI/anyboot/...` 落到 `H:\EFI\EFI\anyboot\...`，而且更糟的是
/// 给了「随便挑个深层目录当根」的可能。
#[cfg(target_os = "windows")]
fn esp_root_is_volume_root(root: &Path) -> bool {
    // `Path::new("H:\\").parent()` 是 None（只有前缀 + 根，没有名字分量）
    root.parent().is_none() && root.is_absolute()
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
    // 先按 GPT 类型直接把 ESP 列出来：Windows 默认**不给 ESP 分配盘符**，只扫
    // `A:`–`Z:` 会一块都扫不到，三张卡片全变「未检测到」且不说原因。
    let esp: Vec<PathBuf> = win_esp::list()
        .into_iter()
        .filter_map(|partition| partition.root())
        .collect();
    if !esp.is_empty() {
        return esp;
    }
    // 拿不到 GPT 信息（MBR 盘、老系统、PowerShell 被裁剪）→ 退回原来的盘符扫描。
    // 这条兜底只在真的枚举不出 ESP 时才走，不会掩盖上面那条更准的路。
    // 单独埋点：盲扫 26 个盘符时，一块**断连的网络盘 / 空光驱**能同步等到超时
    // （SMB 断连十几秒），是「有时特别慢」的长尾来源。
    let _span = perf::span("esp.drive_letters");
    (b'A'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn candidate_roots() -> Vec<PathBuf> {
    Vec::new()
}

/// 找不到 ESP 时的「原因 + 可点的修复动作」。目前只有 Windows 给得出。
///
/// 别小看这一句：Windows 上 ESP 天生没有盘符，`candidate_roots` 扫不到，通道就
/// 一起缩进折叠条变成一行「OpenCore（未检测到）」。用户看到的是「AnyBoot 不支持
/// 我这台机器」，真相却是「给那块 260 MB 的隐藏分区分配一个盘符就行」。
///
/// 一次调用把两件事都算了（提示文案 + 按钮），因为 `win_esp::list()` 要起一次
/// PowerShell，分开算会白跑两遍。
fn offline_extras() -> (Option<String>, Vec<ChannelEntry>) {
    #[cfg(target_os = "windows")]
    {
        // 又一次 win_esp::list()（= 又一次 PowerShell）。它在「没找到 ESP」时才走，
        // 但那种情况恰恰是用户最需要看到解释的时候，慢也得付。
        let _span = perf::span("esp.offline_extras");
        // 只看「没有盘符」的：有盘符却没扫到，说明是「这块 ESP 上没有引导器」，
        // 跟盘符无关 —— 那时候给这条提示就是纯误导（用户会去挂一块早就挂好的分区）。
        let unlettered: Vec<win_esp::WinEsp> = win_esp::list()
            .into_iter()
            .filter(|partition| partition.letter.is_none())
            .collect();
        if unlettered.is_empty() {
            return (None, Vec::new());
        }
        let names = unlettered
            .iter()
            .map(win_esp::WinEsp::describe)
            .collect::<Vec<_>>()
            .join("、");
        let hint = format!(
            "这台机器上有 {} 块 EFI 系统分区没有盘符（{names}）——ESP 是隐藏分区，\
             没有盘符就读不到里面的文件。用下面的按钮给其中一块分配盘符\
             （会弹一次管理员授权），再点「重新识别」即可",
            unlettered.len()
        );
        let entries = unlettered
            .into_iter()
            .map(|partition| ChannelEntry {
                id: format!("mount-esp:{}:{}", partition.disk, partition.partition),
                // 标题只放坐标：420px 下十几个字就到头了，大小得放副标题
                title: partition.coords(),
                detail: format!(
                    "{}ESP 没有盘符时 Windows 读不到它的文件；这一步要一次管理员授权",
                    if partition.size_mb > 0 {
                        format!("{} MB · ", partition.size_mb)
                    } else {
                        String::new()
                    }
                ),
                icon: "menu".to_string(),
                requires_elevation: true,
            })
            .collect();
        (Some(hint), entries)
    }
    #[cfg(not(target_os = "windows"))]
    {
        (None, Vec::new())
    }
}

/// 三通道共用的 ESP 根目录。优先级：GRUB 链路已解析的 → 常见挂载点探测。
/// （不引入第二个 ESP 探测实现：识别信息照抄 grub 链路的 EspInfo。）
pub(crate) fn resolve_esp_root(app: &AppHandle) -> Option<(Option<EspInfo>, PathBuf)> {
    let _span = perf::span("esp.resolve");
    {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        if let Some(dir) = &state.grub.efi_dir {
            if has_channel_signature(dir) {
                return Some((state.grub.esp.clone(), dir.clone()));
            }
        }
    }
    let roots = {
        // Windows 上这一步 = 一次 PowerShell 冷启动（Get-Partition），是整条链路
        // 最贵的单点；单独埋一段，才能一眼看出它到底占了多少
        let _span = perf::span("esp.candidates");
        candidate_roots()
    };
    roots
        .into_iter()
        .find(|root| has_channel_signature(root))
        .map(|root| {
            let info = {
                let _span = perf::span("esp.probe");
                esp_probe(&root).ok()
            };
            (info, root)
        })
}

// ─────────────────────────── 刷新与快照 ───────────────────────────

/// 没找到 ESP 时的全 off 视图（与 GRUB 空态同风格：照实说原因）。
///
/// `explain` = 要尽力解释「为什么没找到」（目前只有 Windows 查得到，见
/// [`offline_extras`]）。解释得出来时还会带一个可点的修复动作。
/// 前端只在 `health` 非空时才把未检测的通道渲染成卡片 —— 没有原因可说的
/// 仍然收进折叠条，不占版面。
fn offline_view(
    id: &'static str,
    label: &'static str,
    permission: &'static str,
    explain: bool,
) -> ChannelView {
    let (hint, entries) = if explain {
        offline_extras()
    } else {
        (None, Vec::new())
    };
    let health: Vec<HealthIssue> = hint
        .map(|text| HealthIssue {
            severity: "info".to_string(),
            text,
        })
        .into_iter()
        .collect();
    ChannelView {
        id,
        label,
        detected: false,
        permission,
        chip: "off",
        detail: Some("没有找到挂载着的 ESP（或上面没有这个引导器）".to_string()),
        health,
        entries,
        current_default: None,
        last_boot: None,
        last_boot_at: None,
        error: None,
        menu_preview: None,
    }
}

/// 重算三通道视图并写回 AppState.channels（refresh_and_update 里 GRUB 刷新之后调）。
/// 叫 refresh_channels 是因为 switch_flow::refresh（tauri 命令）经 glob 会撞名。
pub(crate) fn refresh_channels(app: &AppHandle) {
    let _span = perf::span("channels.all");
    let (esp, views) = match resolve_esp_root(app) {
        Some((esp, root)) => {
            let views = vec![
                {
                    let _span = perf::span("channels.sdboot");
                    sdboot::view(&root)
                },
                {
                    let _span = perf::span("channels.opencore");
                    opencore::view(&root)
                },
                {
                    let _span = perf::span("channels.clover");
                    clover::view(&root)
                },
            ];
            (esp, views)
        }
        None => {
            // 原因只往 OpenCore 这张卡片上写：它是这里的主角（有预演列表、有「设为默认」），
            // 三张卡各说一遍同一句话只会把版面撑乱。
            let views = vec![
                offline_view("sdboot", "systemd-boot", "file", false),
                offline_view("opencore", "OpenCore", "nvram", true),
                offline_view("clover", "Clover", "file", false),
            ];
            (None, views)
        }
    };
    let state = app.state::<Mutex<AppState>>();
    state.lock().expect("state mutex poisoned").channels = ChannelsRuntime { esp, views };
}

// ─────────────────── 开机自动「提权扫」（进 App 就有数据） ───────────────────

/// App 一起来就在后台补两份数据，用户的体感是「一进去就有」：
///
/// 1. **OC 启动顺序**（`OCBtOrder` / `BootOrder`）—— 固件变量对非管理员一律
///    `1314`，读不到就标不出「默认项」：用户看到列表却看不出哪个是默认；
/// 2. **APFS Preboot**（macOS 条目）—— 零挂载直读 APFS 容器要开整盘，EACCES；
///    读不到时 OC 预演里 macOS 条目凭空消失。
///
/// 两条都**只走静默通道**（[`Elevation::Silent`] → Windows 特权服务）：
/// 服务没在跑就整段跳过并留一行 trace，绝不在开机那一拍弹 UAC。
/// 跳过的代价只是「要手动点卡片上的按钮」，而不是「开机弹一个没人看着的授权框」。
///
/// 并发关系：这段和 `refresh_and_update` 的首次扫描是两条独立线程，谁先跑完都行 ——
/// 两份数据都在进程内缓存里，`refresh_channels` 读的是缓存，不看扫描先后。
pub(crate) fn spawn_boot_privileged_scan(app: AppHandle) {
    #[cfg(target_os = "windows")]
    std::thread::spawn(move || {
        if !crate::service_win::usable_from_here() {
            crate::trace(
                &app,
                "boot-scan",
                "跳过开机自动提权扫描：特权服务没在运行，或当前不是安装目录里那份 exe\
                 （卡片上的「重扫 macOS 条目 / 重读启动顺序」仍可手动授权）",
            );
            return;
        }
        let mut touched = false;
        match nvram::refresh_boot_vars_elevated(Elevation::Silent) {
            Ok(message) => {
                crate::trace(&app, "boot-scan", &format!("启动顺序：{message}"));
                touched = true;
            }
            // 读不到不影响页面：预演照旧，只是「默认项」标不出来
            Err(error) => crate::trace(&app, "boot-scan", &format!("启动顺序没读到：{error}")),
        }
        match apfs_reader::scan_preboot_elevated(false, Elevation::Silent) {
            Ok(outcome) => {
                crate::trace(
                    &app,
                    "boot-scan",
                    &format!("APFS Preboot：{} 条｜{}", outcome.items.len(), outcome.note),
                );
                touched = true;
            }
            Err(error) => crate::trace(&app, "boot-scan", &format!("APFS Preboot 没扫到：{error}")),
        }
        if touched {
            refresh_channels(&app);
            // 一次性推送：窗口已经把首帧画完了，这一拍只补上刚读到的默认项/macOS 条目
            crate::publish_ui(&app);
        }
    });
    // 别的平台没有静默提权通道（macOS 本来就能直读，Linux 走 pkexec 必弹授权）
    #[cfg(not(target_os = "windows"))]
    let _ = app;
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
    /// 把预演菜单里的某一项设为 OC 默认项（= OC 菜单里的 Ctrl+Enter）：
    /// `<卷标>\u{1}<分区内路径>`
    OpenCoreDefault(String),
    /// 给一块没有盘符的 ESP 分配盘符（Windows 专属的前置动作）：
    /// 必须在解析 ESP **之前**执行，见 [`switch`]。
    MountEsp { disk: u32, partition: u32 },
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
        // 设默认项：`<卷标>\u{1}<路径>`（路径里可能有 `:`，所以不能再切一次冒号）
        ("opencore-default", Some(rest))
            if rest.contains(oc_default::SEP) && !rest.starts_with(oc_default::SEP) =>
        {
            Some(SwitchSpec::OpenCoreDefault(rest.to_string()))
        }
        // 挂载 ESP：`<磁盘号>:<分区号>`（Windows 上 ESP 没盘符时的前置动作）
        ("mount-esp", Some(rest)) => {
            let (disk, partition) = rest.split_once(':')?;
            Some(SwitchSpec::MountEsp {
                disk: disk.trim().parse().ok()?,
                partition: partition.trim().parse().ok()?,
            })
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
            Self::OpenCoreDefault(target) => {
                let volume = target.split(oc_default::SEP).next().unwrap_or_default();
                format!("把 {volume} 设为 OpenCore 默认启动项")
            }
            Self::CloverDefault(volume) => format!("Clover 默认卷设为 {volume}"),
            Self::CloverOneShot => "下次启动进入 Clover（一次性）".to_string(),
            Self::MountEsp { disk, partition } => {
                format!("给 EFI 分区分配盘符（磁盘 {disk} 分区 {partition}）")
            }
        }
    }

    /// 动作名（写操作记录用）：「注册」与「切换」不是一回事。
    pub(crate) fn action_label(&self) -> &'static str {
        match self {
            Self::OpenCoreRegister(_) => "写入固件启动项",
            Self::OpenCoreDefault(_) => "设置 OC 默认项",
            Self::MountEsp { .. } => "挂载 ESP",
            _ => "切换引导通道",
        }
    }
}

/// 执行一次通道切换（不动 busy / 不发布快照，调用方管）。
pub(crate) fn switch(app: &AppHandle, spec: &str) -> Result<String, String> {
    let parsed =
        parse_switch_spec(spec).ok_or_else(|| format!("未知的引导通道值域: {spec}"))?;
    // 「给 ESP 分配盘符」是在**修前置条件**：它必须跑在 resolve_esp_root 之前，
    // 否则永远是「没有找到可用的 ESP」——正是因为没盘符才找不到的。
    if let SwitchSpec::MountEsp { disk, partition } = &parsed {
        let root = win_esp::mount(*disk, *partition)?;
        return Ok(format!(
            "已给磁盘 {disk} 分区 {partition} 分配盘符 {root}，正在重新识别 ESP"
        ));
    }
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
        // 「设默认项」= OC 菜单里的 Ctrl+Enter（写 OC 私有命名空间，绕过 macOS 白名单）。
        // Windows 非提权进程读写 OCBt* 直接被拒（1314/5）—— 与其让用户看一条失败，
        // 不如按需弹一次授权，让本程序的提权子进程跑同一套 set_default。
        // macOS 走免密助手信箱（helper_install），不经过这条路。
        SwitchSpec::OpenCoreDefault(target) => {
            // 提权子进程做完就必须**就地返回**。落回下面那条非提权路径的后果：
            // 它读 OCBt* 会失败（读不到被 `read_order` 当成「表是空的」），于是
            // 重新算出一个槽号、再写一次 —— 写又被拒，用户看到失败，而 NVRAM
            // 其实早就改好了；这种「成功报成失败」比直接失败更难排查。
            #[cfg(target_os = "windows")]
            {
                if nvram::boot_vars_view() == nvram::BootVarsView::NeedElevation {
                    return oc_default::set_default_elevated(&esp_root, target);
                }
            }
            oc_default::set_default(&esp_root, target)?
        }
        SwitchSpec::CloverDefault(volume) => {
            clover::set_default_volume(&esp_root, volume, None)?;
            parsed.describe()
        }
        SwitchSpec::CloverOneShot => {
            let number = opencore::set_boot_next(&esp_root, clover::CLOVER_MARKER, "clover")?;
            format!("{}（Boot{}）", parsed.describe(), nvram::boot_var_name(number))
        }
        // 上面已经提前返回；留着是为了 match 穷尽（也防以后有人挪走那段而不知道）
        SwitchSpec::MountEsp { .. } => {
            return Err("挂载 ESP 必须在解析 ESP 之前处理".to_string())
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
pub(crate) async fn switch_boot_channel(app: AppHandle, target: String) -> AppStatus {
    trace(&app, "command", &format!("switch_boot_channel target={target}"));
    // 提权（Windows 上「设为默认」弹 UAC）可能等几分钟，绝不能占 UI 线程 ——
    // 这就是 blocking_guard 的存在理由。整条链路挪到工作线程，本函数只 await。
    let app_for_task = app.clone();
    let join =
        tauri::async_runtime::spawn_blocking(move || switch_channel_and_update(&app_for_task, &target));
    match join.await {
        Ok(status) => status,
        Err(error) => update_failure_status(&app, "切换失败", format!("任务异常退出：{error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 特权路径只接受「卷根」形态的 ESP。放行 `H:\EFI` 这种子目录会让
    /// `EFI/anyboot/...` 落成 `H:\EFI\EFI\anyboot\...`，更糟的是把「随便挑个
    /// 深层目录当根」变成了可能 —— 而写它的是 SYSTEM。
    #[cfg(target_os = "windows")]
    #[test]
    fn esp_root_must_be_a_volume_root() {
        assert!(esp_root_is_volume_root(Path::new(r"H:\")));
        assert!(esp_root_is_volume_root(Path::new(r"C:\")));
        assert!(!esp_root_is_volume_root(Path::new(r"H:\EFI")));
        assert!(!esp_root_is_volume_root(Path::new(r"H:\EFI\OC")));
        // 盘符相对路径（`H:`）既不是绝对路径也不是卷根
        assert!(!esp_root_is_volume_root(Path::new(r"H:")));
    }

    /// 找不到带通道签名的 ESP 时，**绝不能**把调用方点名的那个路径当结果用：
    /// `esp` 是特权请求里唯一的路径参数，宽松处理等于让调用方指定写哪儿。
    ///
    /// 注意这里不能断言「一定报错」：本机真有一块带签名的 ESP 时会退回到它，
    /// 那是对的（候选列表从来是可信来源）。要钉住的是「点名 + 无签名 → 不用它」。
    #[test]
    fn resolve_esp_root_never_trusts_an_unsigned_path() {
        let missing = std::env::temp_dir().join("anyboot-no-such-esp");
        match resolve_esp_root_arg(Some(&missing.to_string_lossy())) {
            Ok(root) => assert_ne!(root, missing, "不该把没有通道签名的路径当 ESP 根"),
            Err(error) => assert!(error.contains("没有找到可用的 ESP"), "{error}"),
        }
    }

    /// 「自动扫不弹授权」这条规矩的唯一入口：前端的 `auto=true` 必须落成 `Silent`。
    /// 一旦有人把它接反，开机就会弹 UAC —— 而这在没有人的机器上等于功能彻底失效。
    #[test]
    fn auto_scan_never_allows_a_prompt() {
        assert_eq!(Elevation::from_auto(true), Elevation::Silent);
        assert!(Elevation::from_auto(true).is_silent());
        assert_eq!(Elevation::from_auto(false), Elevation::Allow);
        assert!(!Elevation::from_auto(false).is_silent());
    }

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
        // 设默认项：卷标与路径用 0x01 分隔（路径里可能有 `:`，所以不能再切一次冒号）
        let target = "Macintosh HD\u{1}\\EFI\\Microsoft\\Boot\\bootmgfw.efi";
        assert_eq!(
            parse_switch_spec(&format!("opencore-default:{target}")),
            Some(SwitchSpec::OpenCoreDefault(target.to_string()))
        );
        let with_colon = "VOL\u{1}\\EFI\\x:y.efi";
        assert_eq!(
            parse_switch_spec(&format!("opencore-default:{with_colon}")),
            Some(SwitchSpec::OpenCoreDefault(with_colon.to_string()))
        );
        // 缺卷标 / 缺路径 → 拒绝（别把半截参数当成合法动作）
        assert_eq!(parse_switch_spec("opencore-default:Macintosh HD"), None);
        assert_eq!(parse_switch_spec("opencore-default:\u{1}\\EFI\\x.efi"), None);
        assert_eq!(parse_switch_spec("opencore-default:"), None);
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
        assert_eq!(
            SwitchSpec::OpenCoreDefault("Macintosh HD\u{1}\\System\\boot.efi".into()).action_label(),
            "设置 OC 默认项"
        );
        assert!(SwitchSpec::OpenCoreDefault("Macintosh HD\u{1}\\System\\boot.efi".into())
            .describe()
            .contains("Macintosh HD"));
        // describe 要说清插到 BootOrder 哪儿（用户能预期默认启动项会不会变）
        assert!(SwitchSpec::OpenCoreRegister("full".into())
            .describe()
            .contains("首位"));
        assert!(SwitchSpec::OpenCoreRegister("short".into())
            .describe()
            .contains("末尾"));
    }
}
