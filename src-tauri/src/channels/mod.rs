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

/// 三张通道卡的身份。
///
/// 定位层给出**候选集**，每张卡只认自己那份签名 —— 这是「一块 ESP 服务三通道」
/// 拆开后的调用约定：`systemd-boot` 要 `loader/loader.conf`，OpenCore 要
/// `EFI/OC/OpenCore.efi`，Clover 要 `EFI/CLOVER/CLOVERX64.efi`，三者互不代劳。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChannelKind {
    Sdboot,
    OpenCore,
    Clover,
}

impl ChannelKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Sdboot => "systemd-boot",
            Self::OpenCore => "OpenCore",
            Self::Clover => "Clover",
        }
    }

    /// 这块盘上有没有**这张卡自己**那套引导器。
    ///
    /// 刻意不含 GRUB：GRUB 不是「通道卡」（它是当前系统的引导器，身份由
    /// `state.grub` 给出），而且它的判据里认 `EFI/BOOT/BOOTX64.EFI` —— 那个文件
    /// Windows ESP、黑苹果的 OC Bootstrap、几乎所有装过系统的 ESP 上都有。
    /// 一旦把它混进逐卡判据，OpenCore 卡就会在「只有 GRUB 的 ESP」上误报已检测，
    /// 然后在错误的盘上设启动项。
    pub(crate) fn has_signature(self, root: &Path) -> bool {
        match self {
            Self::Sdboot => grub_rel(root, "loader/loader.conf").is_file(),
            Self::OpenCore => grub_rel(root, "EFI/OC/OpenCore.efi").is_file(),
            Self::Clover => grub_rel(root, "EFI/CLOVER/CLOVERX64.efi").is_file(),
        }
    }

    /// 判据文件在 ESP 上的相对路径（报错文案用，别再抄一遍字面量）。
    pub(crate) fn signature_hint(self) -> &'static str {
        match self {
            Self::Sdboot => "loader/loader.conf",
            Self::OpenCore => "EFI/OC/OpenCore.efi",
            Self::Clover => "EFI/CLOVER/CLOVERX64.efi",
        }
    }
}

/// 目录上有没有**任一**引导器的标志性文件（是块挂载着的 ESP 且确实在用）。
///
/// 只留两个用途：判断「这块盘算不算 ESP」（特权写的准入门）与「值不值得进候选」。
/// **不要**拿它当某张卡的检测判据 —— 那正是「双 ESP 机器上 OC 永远看不见」的成因。
fn has_channel_signature(root: &Path) -> bool {
    ChannelKind::Sdboot.has_signature(root)
        || ChannelKind::OpenCore.has_signature(root)
        || ChannelKind::Clover.has_signature(root)
        || has_grub_signature(root)
}

/// GRUB 也是「这块 ESP 上确实装了引导器」的一种。
///
/// 原先只认 systemd-boot / OpenCore / Clover 三个签名，于是纯 GRUB 的 Linux
/// 机器三个全落空 → [`resolve_esp_root`] 判「这台机器没有 ESP」→ 三张通道卡
/// 一起变「未检测到」。macOS 上永远撞不到这件事（黑苹果的前置就是装 OC，
/// 判据必然成立），Linux 上却是绝大多数机器的常态 —— 同一个判据在两端
/// 语义完全相反，这是 Linux 端通道整块失灵的根因。
///
/// 只认「引导器文件」而**不是**「有 EFI 目录就好」：后者会把签名校验降级成
/// 「任意一个 FAT 卷」，而这个函数同时是特权路径的准入条件
/// （见 [`resolve_esp_root_arg`]，特权进程会往它下面写 `EFI/anyboot/...`）。
fn has_grub_signature(root: &Path) -> bool {
    // AnyBoot 自己往 ESP 上写的那份 GRUB 配置（装过 09_anyboot 就一定有）
    if grub_rel(root, "EFI/grub.cfg").is_file() {
        return true;
    }
    // 兜底引导路径：任何装过系统的 ESP 都有它。GRUB、Windows、以及 OC 的
    // Bootstrap 方案都会把各自的 loader 丢在这儿。
    if grub_rel(root, "EFI/BOOT/BOOTX64.EFI").is_file()
        || grub_rel(root, "EFI/BOOT/BOOTIA32.EFI").is_file()
        || grub_rel(root, "EFI/BOOT/BOOTAA64.EFI").is_file()
    {
        return true;
    }
    // 发行版自己的目录（`EFI/arch`、`EFI/ubuntu`、`EFI/fedora`…）里的 grub 本体。
    // 走到这里说明连 EFI/BOOT 都被人删了，但 grub 还在，仍然算一块能用的 ESP。
    let Ok(entries) = std::fs::read_dir(grub_rel(root, "EFI")) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let dir = entry.path();
        if !dir.is_dir() {
            return false;
        }
        ["grubx64.efi", "grubia32.efi", "grubaa64.efi"]
            .iter()
            .any(|name| dir.join(name).is_file())
    })
}

/// 一块**已被上游链路确认过身份**的 ESP 长什么样：根下有 `EFI/` 目录。
///
/// 比 [`has_channel_signature`] 宽，因为调用方已经带着上下文来 —— 分区是按
/// fstab 的 UUID/PARTUUID 精确匹配出来的，不必再靠「里面装了哪个引导器」反推。
/// **只给只读探测用**，别拿它当特权写的准入条件（那要看签名）。
fn looks_like_esp(root: &Path) -> bool {
    grub_rel(root, "EFI").is_dir()
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

/// Linux 上「去哪些目录找 ESP」= 四个固定路径 + 挂载表里**所有 FAT 家族卷**。
///
/// 为什么要动挂载表：macOS 的候选根是 `/Volumes` 全量 —— 任何磁盘的挂载点都在
/// 那儿，用户在哪儿挂上 ESP 都躲不掉。Linux 没有这种统一目录，只给
/// `/efi /boot/efi /boot /esp` 四个固定点的话，`/mnt/esp`、`/run/media/me/EFI`、
/// 双系统另一块盘的 ESP（本机 `nvme0n1p4` 就是一例）全在射程外，表现就是
/// 「明明挂着一块带 OpenCore 的分区，卡片却写未检测到」。
///
/// 只取 FAT 家族：ESP 规范上就是 FAT12/16/32。把几百个 tmpfs / ext4 / proc
/// 挂载点都拿去 `is_file()` 一遍是白花 syscall，而且会把整条扫描拖慢 ——
/// 这一步在每次刷新通道时都会跑。
#[cfg(target_os = "linux")]
fn candidate_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = ["/efi", "/boot/efi", "/boot", "/esp"]
        .iter()
        .map(PathBuf::from)
        .collect();
    for root in fat_mount_points() {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// 挂载表里的 FAT 家族挂载点（读 `/proc/self/mounts`，不起外部命令）。
///
/// 别改用 `lsblk`：这一步是「扫候选目录」，只需要已挂载的卷；`lsblk` 多给的是
/// 未挂载分区，而要不要挂载是另一件事（未挂载就挂属于改系统状态，得用户点头）。
///
/// 挂载点里的空格 / 制表符 / 反斜杠被内核转义成 `\040` `\011` `\134`，
/// 不还原就会拿一串带反斜杠的假路径去 `is_file()`。
#[cfg(target_os = "linux")]
fn fat_mount_points() -> Vec<PathBuf> {
    let text = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let mut out = Vec::new();
    for line in text.lines() {
        // 字段序：device mountpoint fstype options dump pass
        let mut fields = line.split(' ');
        let Some(_device) = fields.next() else { continue };
        let Some(mountpoint) = fields.next() else {
            continue;
        };
        let Some(fstype) = fields.next() else {
            continue;
        };
        if !matches!(fstype, "vfat" | "msdos" | "fat" | "exfat") {
            continue;
        }
        out.push(PathBuf::from(unescape_mount_point(mountpoint)));
    }
    out
}

/// 还原 mountinfo 的八进制转义（`\040` = 空格）。
#[cfg(target_os = "linux")]
fn unescape_mount_point(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &raw[index + 1..index + 4];
            if let Ok(byte) = u8::from_str_radix(digits, 8) {
                out.push(byte);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
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

/// 找不到 ESP 时的「原因 +（能给的）修复动作」。
///
/// 别小看这一句：没有它，通道就一起缩进折叠条变成一行「OpenCore（未检测到）」
/// —— 用户看到的是「AnyBoot 不支持我这台机器」，而真相可能只是「给那块 260 MB
/// 的隐藏分区分配一个盘符」（Windows），或者「ESP 压根还没挂载」（Linux）。
///
/// 两端给的东西不同：Windows 能直接给按钮（分配盘符），Linux 没有等价动作，
/// 所以给的是**情况分类** —— 「一块都没挂」和「挂了但上面没有引导器」修法完全不同。
/// 但一条都不给是不行的：那正是「未检测到的通道必须说明为什么」的落点。
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
    // Linux：没有「分配盘符」这种动作，但**有**「挂上来」这个等价动作。
    // 关键在于先把情况说清：「挂载着但没引导器」和「压根没挂载」修法完全不同，
    // 糊成一句「未检测到」等于让用户自己猜 —— 而后者正是双系统黑苹果的常态
    // （OpenCore 在另一块盘的 ESP 里，那块盘开机时不会自动挂）。
    #[cfg(target_os = "linux")]
    {
        let mounted: Vec<String> = candidate_roots()
            .into_iter()
            .filter(|root| looks_like_esp(root))
            .map(|root| root.display().to_string())
            .collect();
        let unmounted = crate::scan::unmounted_esp_partitions();
        if !unmounted.is_empty() {
            let names = unmounted
                .iter()
                .map(crate::scan::UnmountedEsp::describe)
                .collect::<Vec<_>>()
                .join("、");
            let mut hint = format!(
                "检测到 {} 块没有挂载的 EFI 系统分区（{names}）—— ESP 没挂载，\
                 里面的文件就读不到，OpenCore 很可能就在其中。点下面那一项把它\
                 只读挂上来（会弹一次管理员授权），挂好后自动重新识别",
                unmounted.len()
            );
            if !mounted.is_empty() {
                hint.push_str(&format!(
                    "；现在已经挂着的 {} 上都没有 EFI/OC",
                    mounted.join("、")
                ));
            }
            let entries = unmounted
                .into_iter()
                .map(|part| ChannelEntry {
                    id: format!("mount-part:{}", part.device),
                    // 标题只放设备名：420px 下十几个字就到头了，卷标与大小放副标题
                    title: part.device.trim_start_matches("/dev/").to_string(),
                    detail: format!("{} · {}", part.label, part.size),
                    icon: "menu".to_string(),
                    requires_elevation: true,
                })
                .collect();
            return (Some(hint), entries);
        }
        if mounted.is_empty() {
            (
                Some(
                    "没找到任何带 EFI 目录的分区：ESP 可能还没挂载\
                     （`lsblk -f | grep vfat` 看一眼哪块是 EFI 系统分区），\
                     也可能是这台机器走 BIOS 引导、根本没有 ESP"
                        .to_string(),
                ),
                Vec::new(),
            )
        } else {
            (
                Some(format!(
                    "找到了 {} 块带 EFI 目录的分区（{}），但上面都没有 EFI/OC —— \
                     也就是说这块盘上还没装 OpenCore，或者它被放在了非标准目录",
                    mounted.len(),
                    mounted.join("、")
                )),
                Vec::new(),
            )
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        (None, Vec::new())
    }
}

/// 三通道共用的 ESP **候选集**（有序、去重）。
///
/// 为什么返回集合而不是一块盘：三张卡真正要问的是「哪块盘上有**我这套**引导器」，
/// 而定位层能回答的只是「哪块盘上有**任一**引导器」。压成一个答案的后果是：
/// 双 ESP 机器上先命中的那块（通常是系统盘，带 GRUB 或 Windows Boot Manager）
/// 赢，另一块盘上的 OpenCore 永远看不见 —— 卡片报「没有 EFI/OC」，
/// 而它是在**错误的盘**上正确地说这句话。
///
/// 顺序即优先级：GRUB 链路按 fstab 的 UUID/PARTUUID 精确匹配出来的那块放最前
/// （它最权威，但**不再提前返回**），其后是各平台的常规候选。
///
/// 只读探测到此为止；特权写那条路仍然走 [`resolve_esp_root_arg`]（严格签名 +
/// Windows 卷根），两条别混。
pub(crate) fn esp_candidates(app: &AppHandle) -> Vec<PathBuf> {
    let _span = perf::span("esp.resolve");
    let mut roots: Vec<PathBuf> = Vec::new();
    {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        if let Some(dir) = &state.grub.efi_dir {
            // 这块 ESP 是按 fstab 里的 UUID/PARTUUID 精确匹配出来的
            // （见 `linux_grub::locate`），比「目录里有没有某个引导器」更权威。
            // 只看签名的话，一块只装 GRUB 的 ESP 会被自己的签名判据否掉 ——
            // 于是「明明已经定位到 ESP，却报告没有 ESP」。
            // `looks_like_esp` 只对 Linux 放宽：Windows 上「有盘符 + 有 EFI 目录」
            // 太容易命中（随便插个 U 盘就够），而它的 GRUB 链路本来就能给出盘符。
            let trusted =
                has_channel_signature(dir) || (cfg!(target_os = "linux") && looks_like_esp(dir));
            if trusted {
                roots.push(dir.clone());
            }
        }
    }
    let candidates = {
        // Windows 上这一步 = 一次 PowerShell 冷启动（Get-Partition），是整条链路
        // 最贵的单点；单独埋一段，才能一眼看出它到底占了多少
        let _span = perf::span("esp.candidates");
        candidate_roots()
    };
    for root in candidates {
        if !roots.iter().any(|known| same_root(known, &root)) {
            roots.push(root);
        }
    }
    roots
}

/// 两个候选根是不是同一块盘（Windows 上 `H:\` 与 `H:` 是同一个卷，且大小写不敏感；
/// 不去重就会对同一个卷白扫一遍 —— Windows 上那是一次 `statfs` 起步的代价）。
fn same_root(left: &Path, right: &Path) -> bool {
    fn trim(path: &Path) -> String {
        path.to_string_lossy()
            .trim_end_matches(['/', '\\'])
            .to_string()
    }
    let (left, right) = (trim(left), trim(right));
    if cfg!(target_os = "windows") {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

/// 从候选集里挑出这张卡要用的那块盘（第一个带它自己签名的）。
///
/// 找不到就是「这块盘上真没装这个引导器」，该由调用方给原因，而不是退回去
/// 拿别的盘凑数。
fn pick_esp(roots: &[PathBuf], kind: ChannelKind) -> Option<PathBuf> {
    roots
        .iter()
        .find(|root| kind.has_signature(root))
        .cloned()
}

// ─────────────────────────── 刷新与快照 ───────────────────────────

/// 没找到 ESP 时的全 off 视图（与 GRUB 空态同风格：照实说原因）。
///
/// `explain` = 要尽力解释「为什么没找到」（见 [`offline_extras`]：
/// Windows 会带一个可点的修复动作，Linux 给的是情况分类）。
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
    // 候选集算一次，三张卡各自在里面挑带自己签名的那块 —— 不再共用同一个 root。
    let candidates = esp_candidates(app);
    let views = vec![
        match pick_esp(&candidates, ChannelKind::Sdboot) {
            Some(root) => {
                let _span = perf::span("channels.sdboot");
                sdboot::view(&root)
            }
            // 原因只往 OpenCore 这张卡片上写：它是这里的主角（有预演列表、有
            // 「设为默认」），三张卡各说一遍同一句话只会把版面撑乱。
            None => offline_view("sdboot", "systemd-boot", "file", false),
        },
        match pick_esp(&candidates, ChannelKind::OpenCore) {
            Some(root) => {
                let _span = perf::span("channels.opencore");
                opencore::view(&root)
            }
            None => offline_view("opencore", "OpenCore", "nvram", true),
        },
        match pick_esp(&candidates, ChannelKind::Clover) {
            Some(root) => {
                let _span = perf::span("channels.clover");
                clover::view(&root)
            }
            None => offline_view("clover", "Clover", "file", false),
        },
    ];
    // 只给 OC 那块做识别详情（视图里用的 esp 字段目前只作展示，取主角那块）
    let esp = pick_esp(&candidates, ChannelKind::OpenCore).and_then(|root| {
        let _span = perf::span("esp.probe");
        esp_probe(&root).ok()
    });
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
/// 两条都**只走静默通道**（[`Elevation::Silent`] → Windows 特权服务 / Linux 免密助手）：
/// 通道没就位就整段跳过并留一行 trace，绝不在开机那一拍弹授权框。
/// 跳过的代价只是「要手动点卡片上的按钮」，而不是「开机弹一个没人看着的授权框」。
///
/// 并发关系：这段和 `refresh_and_update` 的首次扫描是两条独立线程，谁先跑完都行 ——
/// 两份数据都在进程内缓存里，`refresh_channels` 读的是缓存，不看扫描先后。
pub(crate) fn spawn_boot_privileged_scan(app: AppHandle) {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    std::thread::spawn(move || {
        #[cfg(target_os = "windows")]
        if !crate::service_win::usable_from_here() {
            crate::trace(
                &app,
                "boot-scan",
                "跳过开机自动提权扫描：特权服务没在运行，或当前不是安装目录里那份 exe\
                 （卡片上的「重扫 macOS 条目 / 重读启动顺序」仍可手动授权）",
            );
            return;
        }
        // Linux 的静默通道 = 免密助手。只在「助手 + sudoers 都到位、版本也对得上」
        // 时才往下走：半装状态下再往下一层就退到 pkexec，而开机那一拍没人点得到
        // 授权框（这正是原先整段被跳过的原因 —— 免密组件落地后这个前提已经不成立）。
        #[cfg(target_os = "linux")]
        if crate::linux_priv::status().state != "ready" {
            crate::trace(
                &app,
                "boot-scan",
                "跳过开机自动提权扫描：免密组件没装好（「系统引导页」点一次「安装免密组件」），\
                 卡片上的「重扫 macOS 条目 / 重读启动顺序」仍可手动授权",
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
    // macOS 原生就能读 NVRAM 与 /System/Volumes/Preboot，不需要这条提权链
    #[cfg(target_os = "macos")]
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
    /// 把一块**没挂载**的 ESP 只读挂上来（Linux 的前置动作，与上面那条对称）：
    /// 值是设备节点（`/dev/nvme0n1p5`）。同样必须在解析 ESP 之前执行。
    MountPartition(String),
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
        // 只读挂载一块没挂载的 ESP：`/dev/nvme0n1p5`（Linux）。
        // 只收设备节点形状的字符串 —— 这个值会原样交给 `mount`，
        // 放任何带空格/`..`/多级路径的东西进来都等于让前端指定挂载目标。
        ("mount-part", Some(device)) => {
            let device = device.trim();
            let name = device.strip_prefix("/dev/")?;
            if name.is_empty()
                || name.contains('/')
                || name.contains("..")
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            {
                return None;
            }
            Some(SwitchSpec::MountPartition(device.to_string()))
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
            Self::MountPartition(device) => format!("只读挂载 {device} 后重新识别"),
        }
    }

    /// 动作名（写操作记录用）：「注册」与「切换」不是一回事。
    pub(crate) fn action_label(&self) -> &'static str {
        match self {
            Self::OpenCoreRegister(_) => "写入固件启动项",
            Self::OpenCoreDefault(_) => "设置 OC 默认项",
            Self::MountEsp { .. } | Self::MountPartition(_) => "挂载 ESP",
            _ => "切换引导通道",
        }
    }
}

/// 把一块没挂载的 ESP **只读**挂到本程序自己的目录下。
///
/// 三道闸，缺一条都能变成「让前端指定 root 挂载任意东西」：
/// 1. 设备必须出现在 [`crate::scan::unmounted_esp_partitions`] 里（真实枚举出来的
///    ESP 分区，不是前端说了算）；
/// 2. 挂载点由我们自己拼（`$XDG_RUNTIME_DIR/anyboot-esp/<设备名>`），不接外部路径；
/// 3. 一律 `ro` + `nosuid,nodev`（见 `mount_options`），只读看一眼，不当系统盘用。
///
/// 挂载需要 root。两条路，按 `allow_pkexec` 分：
///
/// - **免密助手**（装了 `/etc/sudoers.d/anyboot` 就是它）：静默，无感，启动自动
///   挂载只认这条路；
/// - **pkexec**：系统授权框，一次一弹。只在用户亲手点按钮（`allow_pkexec=true`）
///   时才允许 —— 启动那一拍没人看着屏幕，弹了等于永远刷不出结果。
#[cfg(target_os = "linux")]
fn linux_mount_esp_ro(device: &str, allow_pkexec: bool) -> Result<PathBuf, String> {
    let _span = perf::span("esp.mount");
    let found = crate::scan::unmounted_esp_partitions()
        .into_iter()
        .find(|part| part.device == device)
        .ok_or_else(|| {
            format!("{device} 不在「未挂载的 EFI 系统分区」列表里（可能已经挂上了，刷新看看）")
        })?;
    let base = crate::esp::esp_mountpoint();
    std::fs::create_dir_all(&base).map_err(|error| format!("无法创建挂载点目录: {error}"))?;
    let name = found
        .device
        .trim_start_matches("/dev/")
        .replace(['/', ' '], "_");
    let mountpoint = base.join(name);
    std::fs::create_dir_all(&mountpoint).map_err(|error| format!("无法创建挂载点: {error}"))?;
    // 上一次点过就已经挂着了：直接复用，别重复挂（重复挂会在同一个点上叠加）
    if mountpoint.join("EFI").is_dir() {
        return Ok(mountpoint);
    }
    let fstype = found.fstype.as_deref();
    // 只认 state == "ready"（= 助手在 + 授权在 + **版本与本程序一致**）。
    // 别用 helper_ready()：那是旧版助手副本也会过的 —— 它不认识 --linux-mount-esp，
    // sudo 拉起来会带着 root 身份一路落进 Tauri，绝不能把新参数喂给它。
    if crate::linux_priv::status().state == "ready" {
        match mount_esp_via_helper(&found.device, &mountpoint, fstype) {
            Ok(()) => {}
            // 免密路挂失败（APFS、内核裁剪了 vfat……）：真实原因优先透出；
            // 但自动挂载绝不能退到弹框那一步
            Err(error) if !allow_pkexec => return Err(error),
            Err(_) => {
                let output = {
                    let mut cmd = crate::mount::mount_command("mount");
                    cmd.args(["-o", &crate::mount::mount_options("ro")]);
                    if let Some(fstype) = fstype {
                        cmd.arg("-t").arg(fstype);
                    }
                    cmd.arg(&found.device)
                        .arg(&mountpoint)
                        .output()
                        .map_err(|error| format!("无法执行 mount: {error}"))?
                };
                if !output.status.success() {
                    return Err(format!(
                        "挂载 {} 失败: {}",
                        found.device,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
            }
        }
    } else if !allow_pkexec {
        return Err(
            "没装免密组件，启动时不自动挂载（「系统引导页」装一次，或点卡片上的按钮手动挂）"
                .to_string(),
        );
    } else {
        let output = {
            let mut cmd = crate::mount::mount_command("mount");
            cmd.args(["-o", &crate::mount::mount_options("ro")]);
            // 知道类型就显式给（省一次探测）；不知道就交给 mount 自己探 ——
            // 猜一个 `vfat` 上去只会把本来能挂的分区挂失败。
            if let Some(fstype) = fstype {
                cmd.arg("-t").arg(fstype);
            }
            cmd.arg(&found.device)
                .arg(&mountpoint)
                .output()
                .map_err(|error| format!("无法执行 mount: {error}"))?
        };
        if !output.status.success() {
            return Err(format!(
                "挂载 {} 失败: {}",
                found.device,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    if !mountpoint.join("EFI").is_dir() {
        return Err(format!(
            "{} 挂上了，但根下没有 EFI 目录 —— 它可能不是 ESP",
            found.device
        ));
    }
    Ok(mountpoint)
}

/// 走免密助手的只读挂载：sudo → `/usr/lib/anyboot/anyboot-priv --linux-mount-esp`，
/// 结果从 JSON 文件读回。成功与否以「ok 字段」为准（mount 本身失败也算跑通）。
#[cfg(target_os = "linux")]
fn mount_esp_via_helper(
    device: &str,
    mountpoint: &Path,
    fstype: Option<&str>,
) -> Result<(), String> {
    let out = crate::grub_esp::unique_temp_path_under(
        crate::linux_priv::SCRATCH_DIR,
        "anyboot-esp-mount-out",
    );
    let args = vec![
        "--linux-mount-esp".to_string(),
        "--device".to_string(),
        device.to_string(),
        "--mountpoint".to_string(),
        mountpoint.display().to_string(),
        "--fstype".to_string(),
        fstype.unwrap_or_default().to_string(),
        "--out".to_string(),
        out.display().to_string(),
    ];
    crate::linux_priv::run_via_helper(&args, &out)?;
    let text = std::fs::read_to_string(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| "免密挂载没写出结果（授权多半失效了，重装一次免密组件）".to_string())?;
    if value.get("ok").and_then(|flag| flag.as_bool()) == Some(true) {
        return Ok(());
    }
    Err(value
        .get("error")
        .and_then(|error| error.as_str())
        .unwrap_or("免密挂载失败")
        .to_string())
}

/// 开机自动「只读挂载」所有未挂载的 ESP —— 进 App 卡片就有数据，不用手点。
///
/// 与 `spawn_boot_privileged_scan` 同一条规矩：**只走免密助手，没装好整段跳过**，
/// 绝不在开机那一拍弹授权框。单块挂不上（APFS、内核裁剪了 vfat……）只留一行
/// trace，不阻塞其余的。挂完至少一块就刷新一次通道，让 OC 卡片当场变绿。
pub(crate) fn spawn_boot_esp_mount(app: AppHandle) {
    #[cfg(target_os = "linux")]
    std::thread::spawn(move || {
        if crate::linux_priv::status().state != "ready" {
            crate::trace(
                &app,
                "boot-esp-mount",
                "跳过开机自动挂载 ESP：免密组件没装好（「系统引导页」点一次「安装免密组件」，\
                 或用卡片上的「只读挂载」手动挂）",
            );
            return;
        }
        let unmounted = crate::scan::unmounted_esp_partitions();
        if unmounted.is_empty() {
            return;
        }
        let mut mounted_any = false;
        for part in &unmounted {
            match linux_mount_esp_ro(&part.device, false) {
                Ok(_) => {
                    crate::trace(
                        &app,
                        "boot-esp-mount",
                        &format!("已只读挂载 {}", part.device),
                    );
                    mounted_any = true;
                }
                Err(error) => crate::trace(
                    &app,
                    "boot-esp-mount",
                    &format!("{} 挂不上：{error}", part.device),
                ),
            }
        }
        if mounted_any {
            refresh_channels(&app);
            // 与 boot-scan 同款一次性推送：首帧已画完，这一拍把「挂上之后
            // 检测到了」补进窗口 —— 不推的话卡片要等用户手动刷新才变绿
            crate::publish_ui(&app);
        }
    });
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
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
    // 「把一块没挂载的 ESP 只读挂上来」同样是在**修前置条件**，也得跑在解析之前：
    // 没挂载的分区对候选集根本不存在，先解析只会得到「没有找到可用的 ESP」。
    // 这是用户亲手点的按钮：免密挂失败允许退到 pkexec 弹一次授权。
    #[cfg(target_os = "linux")]
    if let SwitchSpec::MountPartition(device) = &parsed {
        let mountpoint = linux_mount_esp_ro(device, true)?;
        return Ok(format!(
            "已把 {device} 只读挂载到 {}，正在重新识别 ESP",
            mountpoint.display()
        ));
    }
    // 每张卡去候选集里挑**带自己签名**的那块：切 OpenCore 的启动项却写进
    // systemd-boot 的 ESP，是比「找不到」更坏的失败 —— 它会成功，然后不生效。
    let kind = match &parsed {
        SwitchSpec::Sdboot(_) | SwitchSpec::SdbootOneShot(_) => ChannelKind::Sdboot,
        SwitchSpec::OpenCore
        | SwitchSpec::OpenCoreRegister(_)
        | SwitchSpec::OpenCoreDefault(_) => ChannelKind::OpenCore,
        SwitchSpec::CloverDefault(_) | SwitchSpec::CloverOneShot => ChannelKind::Clover,
        // 只有 Linux 提供「只读挂上来」这个前置动作（macOS 走 diskutil mount，
        // Windows 走分配盘符），走到这里说明是别的平台收到了 Linux 的值域。
        SwitchSpec::MountPartition(_) => {
            return Err("只读挂载 ESP 目前只在 Linux 上提供".to_string())
        }
        // 上面都提前返回了；留着是为了 match 穷尽（也防以后有人挪走那段而不知道）
        SwitchSpec::MountEsp { .. } => {
            return Err("挂载 ESP 必须在解析 ESP 之前处理".to_string())
        }
    };
    let esp_root = pick_esp(&esp_candidates(app), kind).ok_or_else(|| {
        format!(
            "候选的 ESP 里没有一块装着 {}（它要的是 {}）",
            kind.label(),
            kind.signature_hint()
        )
    })?;
    // 这块 ESP 很可能是我们自己「只读挂载」上来的（卡片上那个按钮）。写动作全都要
    // 往它上面落文件（NVRAM 备份、state.json、必要时改 config.plist），只读挂载会
    // 直接 EROFS —— 所以临时转可写、跑完转回只读。与扫描 rEFInd 时同一套 with_mount。
    let readonly_mount = crate::mount::readonly_mount_of(&esp_root);
    let message = crate::mount::with_mount(readonly_mount.as_ref(), || {
        apply_spec(&parsed, &esp_root)
    })?;
    Ok(message)
}

/// 写入类动作的实际执行（切换 / 注册 / 设默认 / 一次性启动）。
///
/// 拆出来是为了让「只读挂载临时转可写」只写一遍：这些动作全都要往 ESP 落文件
/// （NVRAM 备份 `EFI/anyboot/nvram/`、`state.json`、必要时改 `config.plist`）。
fn apply_spec(parsed: &SwitchSpec, esp_root: &Path) -> Result<String, String> {
    let message = match parsed {
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
    SwitchSpec::MountEsp { .. } | SwitchSpec::MountPartition(_) => {
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

    /// Linux：挂载表里的八进制转义必须还原。
    ///
    /// 卷标带空格的 U 盘/分区挂出来就是 `/run/media/me/My\040Disk` 这种形态，
    /// 不还原等于拿一串带反斜杠的假路径去 `is_file()` —— 表现是「明明插着，
    /// 就是检测不到」，而且不会有任何报错。
    #[cfg(target_os = "linux")]
    #[test]
    fn mount_point_escapes_are_unescaped() {
        assert_eq!(unescape_mount_point("/boot/efi"), "/boot/efi");
        assert_eq!(
            unescape_mount_point(r"/run/media/me/My\040Disk"),
            "/run/media/me/My Disk"
        );
        assert_eq!(unescape_mount_point(r"/a\011b"), "/a\tb");
        // 不是合法八进制就原样留着：别把真的反斜杠吃掉
        assert_eq!(unescape_mount_point(r"/a\zzz"), r"/a\zzz");
        // 不足三位的残缺转义同样原样保留
        assert_eq!(unescape_mount_point(r"/tail\04"), r"/tail\04");
    }

    #[cfg(target_os = "linux")]
    fn signature_scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "anyboot-sig-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 纯 GRUB 的 ESP 必须被认出来 —— 这是 Linux 端通道整块失灵的根因：
    /// 原先只认 systemd-boot / OC / Clover，GRUB 机器三个全落空，
    /// `resolve_esp_root` 于是报「这台机器没有 ESP」。
    #[cfg(target_os = "linux")]
    #[test]
    fn grub_signature_covers_the_three_real_layouts() {
        // ① AnyBoot 自己写的那份 EFI/grub.cfg
        let root = signature_scratch("grubcfg");
        std::fs::create_dir_all(root.join("EFI")).unwrap();
        std::fs::write(root.join("EFI/grub.cfg"), "menuentry x {}\n").unwrap();
        assert!(has_channel_signature(&root), "EFI/grub.cfg 没被认出来");
        let _ = std::fs::remove_dir_all(&root);

        // ② 发行版目录里的 grub 本体（arch / ubuntu / fedora 都是这个形状）
        let root = signature_scratch("distro");
        std::fs::create_dir_all(root.join("EFI/arch")).unwrap();
        std::fs::write(root.join("EFI/arch/grubx64.efi"), b"MZ").unwrap();
        assert!(has_channel_signature(&root), "EFI/arch/grubx64.efi 没被认出来");
        let _ = std::fs::remove_dir_all(&root);

        // ③ 兜底引导路径
        let root = signature_scratch("bootpath");
        std::fs::create_dir_all(root.join("EFI/BOOT")).unwrap();
        std::fs::write(root.join("EFI/BOOT/BOOTX64.EFI"), b"MZ").unwrap();
        assert!(has_channel_signature(&root), "EFI/BOOT/BOOTX64.EFI 没被认出来");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 反过来：空 ESP（只有 EFI 目录、里面什么都没有）**不算**签名。
    ///
    /// 这个函数同时是特权写的准入门（`resolve_esp_root_arg`），放宽成
    /// 「有 EFI 目录就行」等于「任意一块 FAT 卷都能被 root 进程写」。
    /// 而 `looks_like_esp` 认得它 —— 那条路只喂只读探测，且有上游 UUID 匹配背书。
    #[cfg(target_os = "linux")]
    #[test]
    fn empty_efi_dir_is_not_a_signature() {
        let root = signature_scratch("empty");
        std::fs::create_dir_all(root.join("EFI")).unwrap();
        assert!(!has_channel_signature(&root), "空 EFI 目录不该算签名");
        assert!(looks_like_esp(&root), "已确认过的 ESP 只要求有 EFI 目录");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 逐卡判据必须互不代劳：一块「只有 GRUB」的盘上，三张卡都得说「没装」。
    ///
    /// 这是「双 ESP 机器上 OpenCore 永远看不见」的回归闸门：一旦有人把
    /// [`has_channel_signature`]（任一签名）塞回逐卡判断里，OC 卡就会在这块
    /// 只有 GRUB 的盘上误报已检测 —— 然后往错误的盘写启动项。
    #[cfg(target_os = "linux")]
    #[test]
    fn channel_kinds_never_borrow_each_others_signature() {
        let root = signature_scratch("kinds");
        std::fs::create_dir_all(root.join("EFI/BOOT")).unwrap();
        std::fs::write(root.join("EFI/BOOT/BOOTX64.EFI"), b"MZ").unwrap();
        std::fs::write(root.join("EFI/grub.cfg"), "menuentry x {}\n").unwrap();
        for kind in [
            ChannelKind::Sdboot,
            ChannelKind::OpenCore,
            ChannelKind::Clover,
        ] {
            assert!(
                !kind.has_signature(&root),
                "只有 GRUB 的盘被判成了 {}",
                kind.label()
            );
        }
        assert!(has_channel_signature(&root), "GRUB 盘必须仍算一块可用的 ESP");

        // 反过来：装了 OC 就只有 OC 认领这份签名
        std::fs::create_dir_all(root.join("EFI/OC")).unwrap();
        std::fs::write(root.join("EFI/OC/OpenCore.efi"), b"MZ").unwrap();
        assert!(ChannelKind::OpenCore.has_signature(&root));
        assert!(!ChannelKind::Sdboot.has_signature(&root), "OC 的盘不该被 sdboot 认领");
        assert!(!ChannelKind::Clover.has_signature(&root), "OC 的盘不该被 Clover 认领");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 候选集里每张卡挑到**自己的**那块，而不是「谁排前面谁通吃」。
    ///
    /// 本机的真实形状：`/boot/efi`（只有 GRUB）排在前面，OpenCore 在另一块盘上。
    /// 旧逻辑 `find(has_channel_signature)` 会永远停在第一块，OC 卡报「没有 EFI/OC」。
    #[cfg(target_os = "linux")]
    #[test]
    fn pick_esp_is_per_channel_not_first_match() {
        let first = signature_scratch("pick-first");
        std::fs::create_dir_all(first.join("EFI/BOOT")).unwrap();
        std::fs::write(first.join("EFI/BOOT/BOOTX64.EFI"), b"MZ").unwrap();
        let second = signature_scratch("pick-second");
        std::fs::create_dir_all(second.join("EFI/OC")).unwrap();
        std::fs::write(second.join("EFI/OC/OpenCore.efi"), b"MZ").unwrap();

        let roots = vec![first.clone(), second.clone()];
        assert_eq!(
            pick_esp(&roots, ChannelKind::OpenCore),
            Some(second.clone()),
            "OC 必须挑到后面那块，而不是排在最前的 GRUB 盘"
        );
        assert_eq!(
            pick_esp(&roots, ChannelKind::Sdboot),
            None,
            "两块盘上都没有 loader/loader.conf，就该如实返回 None"
        );
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    /// 现场诊断：把本机「未挂载的 ESP」原样打出来。
    ///
    /// `cargo test --lib -- --ignored dump_unmounted_esp --nocapture`
    ///
    /// 这条是给人看的，不是断言：换一台机器、换一个发行版，udev 有没有起来、
    /// `lsblk` 的 FSTYPE 是不是空的，只有打出来才知道。
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn dump_unmounted_esp() {
        for part in crate::scan::unmounted_esp_partitions() {
            println!(
                "{} | {} | {} | fstype={:?}",
                part.device, part.label, part.size, part.fstype
            );
        }
    }

    /// `mount-part:` 只收设备节点形状的字符串。
    /// 这个值会原样交给 `mount`（而且是提权跑的），放宽等于让前端指定挂载任何东西；
    /// 带空格能夹参数、带 `/` 能跑出 `/dev` 之外、带 `..` 能离开 `/dev`。
    #[test]
    fn mount_partition_spec_is_sanitized() {
        assert_eq!(
            parse_switch_spec("mount-part:/dev/nvme0n1p5"),
            Some(SwitchSpec::MountPartition("/dev/nvme0n1p5".to_string()))
        );
        assert_eq!(
            parse_switch_spec("mount-part:/dev/sda1"),
            Some(SwitchSpec::MountPartition("/dev/sda1".to_string()))
        );
        for bad in [
            "mount-part:nvme0n1p5",
            "mount-part:/dev/",
            "mount-part:/dev/a/b",
            "mount-part:/dev/../etc/passwd",
            "mount-part:/dev/sda1 -o rw",
            "mount-part:/dev/sda1;reboot",
            "mount-part:",
        ] {
            assert_eq!(parse_switch_spec(bad), None, "{bad} 不该被接受");
        }
    }

    /// 四个固定路径必须永远在候选里（哪怕机器上一块 FAT 都没挂）——
    /// 这条挂了的表现是「所有 Linux 机器一律检测不到」。
    #[cfg(target_os = "linux")]
    #[test]
    fn candidate_roots_always_keep_the_standard_paths() {
        let roots = candidate_roots();
        for want in ["/efi", "/boot/efi", "/boot", "/esp"] {
            assert!(
                roots.iter().any(|root| root.as_path() == Path::new(want)),
                "候选根里缺了 {want}"
            );
        }
        let mut sorted = roots.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), roots.len(), "候选根里有重复项，会让扫描白跑");
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
