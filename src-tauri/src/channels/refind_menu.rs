//! rEFInd 模拟引导项扫描（扫描本身**纯读**：不写盘、不碰 NVRAM、不需要提权；
//! 只有用户在一个条目上点「下次启动」时才写 `vars/PreviousBoot`，见
//! [`refind_sim_switch`]）。
//!
//! 与 `scan.rs` 那条「手动获取」链路**完全并列**，互不影响：
//! * 手动获取 —— 只认 `vars/PreviousBoot-*` 快照（用户自己维护的三份文件），
//!   切换 = 复制其中一份到 `PreviousBoot`；
//! * 模拟引导项扫描（本模块）—— 按 rEFInd 自己的菜单扫描规则复算一遍，
//!   回答「rEFInd 下次开机到底会列出哪些条目、各叫什么名字」。
//!
//! 为什么能做「一比一」：OC 的菜单是运行期内存链表，而 rEFInd 的菜单**纯由
//! 文件系统和 refind.conf 决定**（`scan.c` 里没有隐藏状态），只要把 ESP 挂上来
//! 就能逐条复算。规则来源（rEFInd 0.14.2，逐条对应源码）：
//!
//! | 规则 | 源码 |
//! |---|---|
//! | 扫哪些介质看 `scanfor`（默认 `ieom`） | `scan.c:1371` |
//! | 每卷扫 `EFI\<子目录>\*.efi` + `also_scan_dirs` | `scan.c:1133-1155` |
//! | 跳过 `EFI\tools`、`.` 开头的子目录 | `scan.c:1136` |
//! | Windows 专扫 `bootmgfw.efi`（标题固定 `Microsoft EFI boot`） | `scan.c:1116-1122` |
//! | macOS 专扫 `System\Library\CoreServices\boot.efi` | `scan.c:1049`、`1073` |
//! | fallback `EFI\BOOT\BOOTX64.EFI` 单列一条 `Fallback boot loader` | `scan.c:1170-1178` |
//! | 过滤：`.` 开头文件、`.icns`/`.png`、`.efi.signed` 同名、`DontScanFiles` | `scan.c:927-933` |
//! | loader 必须是有效 PE（`IsValidLoader`） | `scan.c:649` |
//! | 同一目录内按 mtime **降序**（最新在前） | `scan.c:680` |
//! | 标题 = `Boot <路径> from <卷标> `（**末尾带一个空格**） | `scan.c:653` |
//! | 默认不扫 `EFI\refind` 与 `EFI\tools` | `config.c:519-524` |
//! | DEL 隐藏写 `HiddenTags`，元素形如 `<分区GUID>:<路径>` | `menu.c:1657` |
//! | 记住上次启动 = `PreviousBoot`（UTF-16LE 标题串） | `main.c:230`、`349` |
//!
//! **已知误差**（说清楚，不装作一模一样）：固件到底枚举得到哪些卷（没有 NTFS/ext4
//! 驱动就看不到它们）、卷的枚举顺序、`ScanFirmwareDefined` 的 NVRAM Boot#### 条目
//! （非提权读不到）、macOS 的 APFS 快照 bless 路径（主系统里多是别名的形态）。
//! 这些都在 `issues` / `summary` 里如实交代。

use super::*;

/// `scanfor` 缺省值（`main.c:488`；Mac 固件上是 `ihebocm`，但文件扫描部分一致）。
const DEFAULT_SCANFOR: &str = "ieom";
/// `also_scan_dirs` 缺省值（`config.h:83`）。
const DEFAULT_ALSO_SCAN: &[&str] = &["boot", "@/boot"];
/// `dont_scan_volumes` 缺省值（`config.h:82`）。
const DEFAULT_DONT_SCAN_VOLUMES: &[&str] = &["LRS_ESP"];
/// `dont_scan_files` 缺省值：`config.h:74` + `global.h:168-171`（MOK/FWUPDATE/MEMTEST/SHELL，x64 名单）。
/// rEFInd 会把这几组并进默认黑名单，所以它们默认根本不会出现在菜单里。
const DEFAULT_DONT_SCAN_FILES: &[&str] = &[
    "shim.efi",
    "shim-fedora.efi",
    "shim-centos.efi",
    "shimx64.efi",
    "PreLoader.efi",
    "fbx64.efi",
    "fbia32.efi",
    "fbaa64.efi",
    "fb.efi",
    "fwupx64.efi",
    "fwupia32.efi",
    "fwupaa64.efi",
    "memtest86.efi",
    "memtest86_x64.efi",
    "memtest86x64.efi",
    "memtest86+x64.efi",
    "memtest86_ia32.efi",
    "memtest86ia32.efi",
    "memtest86+ia32.efi",
    "memtest86_aa64.efi",
    "memtest86aa64.efi",
    "MokManager.efi",
    "HashTool.efi",
    "HashTool-signed.efi",
    "KeyTool.efi",
    "KeyTool-signed.efi",
    "mmx64.efi",
    "mmia32.efi",
    "mmaa64.efi",
    "shell.efi",
    "shellx64.efi",
    "shellia32.efi",
];
/// 默认不进菜单的目录（`config.c:519-524`：自身目录 + `TOOL_LOCATIONS`）。
/// 注意 `EFI\BOOT` **不在**这里 —— 它只把 fallback 文件名摘出来单列，别的 .efi 照扫。
const DEFAULT_DONT_SCAN_DIRS: &[&str] = &["EFI/refind", "EFI/tools"];

/// 子菜单里的一项（rEFInd 的 `SubScreen` 条目，对应界面上「按 Tab/`+`/F2 进去」
/// 看到的第二层）。
///
/// 来源只有三类（`scan.c GenerateSubScreen` / `linux.c AddKernelToSubmenu` /
/// `config.c AddSubmenu`）：默认项、refind_linux.conf 的 options 变体、手写
/// `submenuentry`。macOS 还有一组写死的诊断项。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SimChild {
    /// 子项标题**原文**（`GenerateSubScreen` 里就是菜单上显示的那一行）
    pub(crate) title: String,
    /// 传给 loader 的参数（`refind_linux.conf` 第二列起）；空串表示用默认
    pub(crate) options: String,
    /// default / options / kernel / macos / stanza
    pub(crate) source: String,
    pub(crate) note: Option<String>,
}

/// 一条模拟出来的引导项（要不要隐藏由 `hidden` 表形状决定，见 [`SimPreview`]）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SimEntry {
    /// rEFInd 的 `me.Title` **原文**（末尾可能带一个空格，前端展示时 trim）
    pub(crate) title: String,
    /// windows / macos / linux / opencore / clover / unknown
    pub(crate) kind: String,
    /// internal / external / optical / macos / windows / fallback / user / firmware
    pub(crate) source: String,
    /// 卷标（rEFInd 的 `VolName`）
    pub(crate) volume: String,
    /// 分区内路径（无前导反斜杠，与 rEFInd 日志一致）
    pub(crate) path: String,
    /// 命中 `HiddenTags` / `DontScanFiles` —— 这一项 rEFInd 菜单里**看不到**
    pub(crate) hidden: bool,
    /// 与 `vars/PreviousBoot` 的标题一致 = 上次启动的那一项
    pub(crate) last_boot: bool,
    /// 命中隐藏规则的原因（排查「为什么少了一项」用）
    pub(crate) hidden_by: Option<String>,
    pub(crate) note: Option<String>,
    /// 子屏标题（`Boot Options for <标题> on <卷标>`，`scan.c:245`）；
    /// 没有子屏时为 `None`
    pub(crate) subscreen: Option<String>,
    /// 第二层条目。rEFInd 给几乎每个 loader 都建了子屏，所以「只有一条
    /// `Boot using default options`」是常态 —— 前端只在一个以上时才展示
    pub(crate) children: Vec<SimChild>,
}

/// 一次模拟扫描的结果（挂在 `UiState.refindSim` 上下发）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SimPreview {
    /// rEFInd 菜单会显示出来的（顺序即扫描顺序）
    pub(crate) entries: Vec<SimEntry>,
    /// 被隐藏规则挡掉的（rEFInd 菜单里看不到，但确实被扫到了）
    pub(crate) hidden: Vec<SimEntry>,
    pub(crate) issues: Vec<HealthIssue>,
    pub(crate) summary: String,
    /// 变量目录（`EFI/refind/vars`）
    pub(crate) vars_dir: String,
    /// 读到的配置文件（没有就是 `None`，此时全套用默认值）
    pub(crate) config_path: Option<String>,
    /// 生效的 `scanfor`（默认 `ieom`）
    pub(crate) scanfor: String,
    /// 变量存放位置：`file`（vars 目录）/ `nvram`（`use_nvram true`）
    pub(crate) var_mode: String,
    /// `vars/PreviousBoot` 解出来的标题（「记住启动项」的值）
    pub(crate) previous_boot: Option<String>,
    /// `default_selection` 原文（`+` 会被 PreviousBoot 替换）
    pub(crate) default_selection: String,
}

/// 重算并写回 `AppState.refind_sim`（扫描结束、手动切换成功之后各调一次）。
///
/// 「手动获取」的切换只改 `PreviousBoot` 文件，模拟列表里的 `lastBoot` 标记
/// 必须跟着重算 —— 否则用户刚切完，卡片上「上次启动」还停在旧的那一项。
pub(crate) fn refresh_sim(app: &tauri::AppHandle) -> Option<SimPreview> {
    let vars_dir = {
        let state = app.state::<Mutex<AppState>>();
        state
            .lock()
            .ok()
            .and_then(|state| state.vars_dir.clone())
    };
    let sim = vars_dir.map(|dir| preview(&dir));
    let state = app.state::<Mutex<AppState>>();
    if let Ok(mut state) = state.lock() {
        state.refind_sim = sim.clone();
    }
    sim
}

/// 手动「重新模拟」：只读重算一遍，不写盘。
#[tauri::command]
pub(crate) fn refind_sim_scan(app: tauri::AppHandle) -> Option<SimPreview> {
    let sim = refresh_sim(&app);
    publish_ui(&app);
    sim
}

/// `PreviousBoot` 文件的字节形态：标题原文 UTF-16LE + 2 字节 NUL。
///
/// 对应 `StoreLoaderName()` 里的 `StrLen(Name) * 2 + 2`（`main.c:230`）——
/// 请**注意**这里不带 Linux efivarfs 那 4 字节属性头，和 `vars/` 里现存文件的
/// 形态完全一致（可拿 `PreviousBoot-linux` 这类快照对照）。
fn previous_boot_bytes(title: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = title
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

/// 选中某个模拟条目 → 写 `vars/PreviousBoot`（「下次启动就是它」）。
///
/// 这正是 rEFInd 自己 `StoreLoaderName()` 做的事（`main.c:230`）：
/// `EfivarSetRaw(&RefindGuid, L"PreviousBoot", Name, StrLen(Name)*2+2, TRUE)`；
/// `use_nvram false` 时 `EfivarSetRaw` 把它落到 `EFI/refind/vars/PreviousBoot`，
/// 下次开机 `AdjustDefaultSelection` 把 `default_selection` 里的 `+` 换成它。
///
/// 所以内容必须逐字节等于「标题原文的 UTF-16LE + 2 字节 NUL」：少一个末尾空格、
/// 卷标大小写不同都会让 rEFInd 匹配不上，表现是「设置了却没生效、开机还进第一项」。
/// 因此这里**只接受当前模拟列表里真实存在的可见项标题**（前端从 `SimEntry.title`
/// 原样带回，不是用户手输的字符串）：
/// * 手输 / 已过期的标题 → 拒绝，提示先重新模拟；
/// * 被隐藏规则挡掉的标题 → 拒绝（菜单里没这一项，写了也匹配不上）。
///
/// 与「手动获取」的关系：两者写的是同一个 `PreviousBoot`，所以模拟这边选中后，
/// 上面那三张快照卡会因为匹配不上而显示「未匹配模板」—— 这是如实的，不是坏了。
#[tauri::command]
pub(crate) fn refind_sim_switch(app: tauri::AppHandle, title: String) -> AppStatus {
    let started = Instant::now();
    let (vars_dir, active_mount, cached) = {
        let state = app.state::<Mutex<AppState>>();
        let state = state.lock().expect("state mutex poisoned");
        (
            state.vars_dir.clone(),
            state.active_mount.clone(),
            state.refind_sim.clone(),
        )
    };
    let Some(vars_dir) = vars_dir else {
        return update_failure_status(
            &app,
            "写入 PreviousBoot 失败",
            "变量目录尚未就绪：先让上一轮扫描成功跑完".to_string(),
        );
    };
    // 缓存没有（或过期）就先算一遍，保证校验用的是当前真实菜单
    let sim = cached.unwrap_or_else(|| preview(&vars_dir));

    // `use_nvram true` 时 rEFInd 从固件 NVRAM 读 PreviousBoot，`vars/` 里的文件它根本不看。
    // 与其写一个「看着成功、开机没反应」的文件，不如拦住并说清改哪里。
    if sim.var_mode == "nvram" {
        return update_failure_status(
            &app,
            "写入 PreviousBoot 失败",
            "refind.conf 里是 use_nvram true：rEFInd 从固件 NVRAM 读 PreviousBoot，\
             不读 vars 目录里的文件。先把 refind.conf 改成 use_nvram false（或删掉那一行）"
                .to_string(),
        );
    }

    if let Some(blocked) = sim.hidden.iter().find(|entry| entry.title == title) {
        let why = blocked
            .hidden_by
            .clone()
            .unwrap_or_else(|| "隐藏规则".to_string());
        return update_failure_status(
            &app,
            "写入 PreviousBoot 失败",
            format!(
                "「{}」被隐藏规则挡掉了（{why}）：rEFInd 菜单里没有这一项，\
                 写进 PreviousBoot 也匹配不上，开机会退回第一项",
                title.trim()
            ),
        );
    }
    if !sim.entries.iter().any(|entry| entry.title == title) {
        return update_failure_status(
            &app,
            "写入 PreviousBoot 失败",
            format!(
                "「{}」不在当前模拟菜单里（多半是重新扫描后菜单变了）：先点「重新模拟」再选",
                title.trim()
            ),
        );
    }

    // 内容 = 标题原文 UTF-16LE + 2 字节 NUL（对应 `StrLen(Name)*2+2`）。
    // rEFInd 自己写这个文件不截断（`egSaveFile` 用 CREATE 打开，短串覆盖长串会留尾巴），
    // 这里用「临时文件 + rename」整文件替换：等价但干净，也顺带清掉历史残留字节。
    let bytes = previous_boot_bytes(&title);
    let target = vars_dir.join("PreviousBoot");

    let result = write_with_mount(active_mount.as_ref(), || {
        let temporary = temporary_target(&target);
        let written = (|| -> io::Result<()> {
            fs::write(&temporary, &bytes)?;
            let file = fs::OpenOptions::new().write(true).open(&temporary)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &target)
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        written.map_err(|error| format!("写入 {} 失败: {error}", target.display()))?;
        // 读回校验：多一个字节少一个字节都算没写对（rEFInd 只按 NUL 截断读，
        // 但它比对的是整串标题，差一个字符就匹配不上）
        let read_back =
            fs::read(&target).map_err(|error| format!("写入后读回失败: {error}"))?;
        if utf16le_to_string(&read_back) != title {
            return Err(format!(
                "写入校验失败：{} 读回的标题与目标不一致",
                target.display()
            ));
        }
        Ok(())
    });

    match result {
        Ok(()) => {
            trace(
                &app,
                "switch",
                &format!(
                    "模拟项写入 PreviousBoot 完成 title={} 耗时={}ms",
                    title.trim(),
                    started.elapsed().as_millis()
                ),
            );
            let mut status = refresh_and_update(&app);
            // refresh 会按「三份快照」重算 message（这次写入对不上任何快照），
            // 这里换成用户真正做的事，别让他以为切换失败
            status.message = format!("下次启动：{}", title.trim());
            record_action(&app, "切换系统（模拟扫描）", true, &status.message);
            status
        }
        Err(error) => {
            trace(
                &app,
                "switch",
                &format!(
                    "模拟项写入 PreviousBoot 失败 title={} 错误={error}",
                    title.trim()
                ),
            );
            record_action(&app, "切换系统（模拟扫描）", false, &error);
            update_failure_status(&app, "写入 PreviousBoot 失败", error)
        }
    }
}

// ─────────────────────────── refind.conf 解析 ───────────────────────────

/// `refind.conf` 里的一只手写 stanza（`ScanUserConfigured`，`config.c:1062`）。
///
/// `disabled` 那一行是关键：rEFInd 见到它就 `Entry->Enabled = FALSE`，
/// `ScanUserConfigured` 随后把这个条目**直接丢弃**（连隐藏都不算，菜单里没有）。
/// refind.conf-sample 里那批示例 stanza 全都带 `disabled`，所以照抄进来也不会
/// 出现在菜单上 —— 不做这一步，模拟结果会比真实菜单多出七八条假条目。
#[derive(Clone, Debug)]
struct MenuStanza {
    title: String,
    loader: String,
    volume: String,
    disabled: bool,
    /// `submenuentry "标题" { options … }` —— 手写 stanza 的第二层（`config.c:1034`）
    subs: Vec<SubStanza>,
}

/// 手写 stanza 里的一个子项。
#[derive(Clone, Debug, Default)]
struct SubStanza {
    title: String,
    options: String,
    loader: String,
}

/// 本模块关心的配置项（其余几十个选项与菜单扫描无关，不解析）。
#[derive(Clone, Debug)]
struct RefindConfig {
    scanfor: String,
    also_scan: Vec<String>,
    dont_scan_dirs: Vec<String>,
    dont_scan_files: Vec<String>,
    dont_scan_volumes: Vec<String>,
    fold_linux_kernels: bool,
    use_nvram: bool,
    default_selection: String,
    /// `menuentry "标题" { ... }` 块（`ScanUserConfigured`）
    menu_entries: Vec<MenuStanza>,
}

impl Default for RefindConfig {
    fn default() -> Self {
        Self {
            scanfor: DEFAULT_SCANFOR.to_string(),
            also_scan: DEFAULT_ALSO_SCAN.iter().map(|s| s.to_string()).collect(),
            dont_scan_dirs: DEFAULT_DONT_SCAN_DIRS.iter().map(|s| s.to_string()).collect(),
            dont_scan_files: DEFAULT_DONT_SCAN_FILES.iter().map(|s| s.to_string()).collect(),
            dont_scan_volumes: DEFAULT_DONT_SCAN_VOLUMES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            fold_linux_kernels: true,
            use_nvram: true,
            default_selection: String::new(),
            menu_entries: Vec::new(),
        }
    }
}

/// 把一行拆成 token：去 `#` 注释、去引号、按空白分。
fn tokens_of(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            '#' if !quoted => break,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn csv(list: &str) -> Vec<String> {
    list.split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// 读 `refind.conf`（支持 `include`，最多 8 层防环）。
///
/// 顶层关键字逐行读；`menuentry "标题" {` 之后进入**块模式**，收块内的
/// `loader` / `volume` / `disabled`，块结束（花括号回到 0）才落一条 stanza。
/// `submenuentry` 里的花括号也算深度，但只影响配对，不进主菜单。
fn read_config(path: &Path, config: &mut RefindConfig, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut stanza: Option<MenuStanza> = None;
    let mut braces: i32 = 0;
    // 进入某个 `submenuentry` 时的花括号深度；回到这个深度就是子项结束
    let mut sub_base: Option<i32> = None;
    for line in text.lines() {
        let tokens = tokens_of(line);
        if let Some(current) = stanza.as_mut() {
            let key = tokens.first().map(|token| token.to_ascii_lowercase());
            // `submenuentry "标题" {`：开一个子项（`config.c:1034`）
            if matches!(key.as_deref(), Some("submenuentry")) {
                if let Some(title) = tokens.get(1) {
                    current.subs.push(SubStanza {
                        title: title.clone(),
                        options: String::new(),
                        loader: String::new(),
                    });
                    sub_base = Some(braces);
                    braces += brace_delta(line);
                    if braces <= sub_base.unwrap_or(0) {
                        sub_base = None;
                    }
                    continue;
                }
            }
            if let Some(base) = sub_base {
                // 子项内部：只收它自己的 options / loader，别污染父 stanza
                if let Some(sub) = current.subs.last_mut() {
                    match key.as_deref() {
                        Some("options") => {
                            if let Some(value) = tokens.get(1) {
                                sub.options = value.clone();
                            }
                        }
                        Some("loader") => {
                            if let Some(value) = tokens.get(1) {
                                sub.loader = value.clone();
                            }
                        }
                        _ => {}
                    }
                }
                braces += brace_delta(line);
                if braces <= base {
                    sub_base = None;
                }
                if braces <= 0 {
                    if let Some(done) = stanza.take() {
                        config.menu_entries.push(done);
                    }
                    sub_base = None;
                }
                continue;
            }
            if let Some(key) = key {
                match key.as_str() {
                    "loader" => {
                        if let Some(value) = tokens.get(1) {
                            current.loader = value.clone();
                        }
                    }
                    "volume" => {
                        if let Some(value) = tokens.get(1) {
                            current.volume = value.clone();
                        }
                    }
                    "disabled" => current.disabled = true,
                    _ => {}
                }
            }
            braces += brace_delta(line);
            if braces <= 0 {
                if let Some(done) = stanza.take() {
                    config.menu_entries.push(done);
                }
            }
            continue;
        }
        let Some(key) = tokens.first().map(|token| token.to_ascii_lowercase()) else {
            continue;
        };
        let args = &tokens[1..];
        match key.as_str() {
            "menuentry" => {
                if let Some(title) = args.first() {
                    stanza = Some(MenuStanza {
                        title: title.clone(),
                        loader: String::new(),
                        volume: String::new(),
                        disabled: false,
                        subs: Vec::new(),
                    });
                    braces = brace_delta(line);
                    if braces <= 0 {
                        // `menuentry X` 独占一行、`{` 在下一行（少见）：按已开启处理，
                        // 由后续第一行 `{` 补上深度
                        braces = 1;
                    }
                }
            }
            "scanfor" => {
                if let Some(value) = args.first() {
                    config.scanfor = value.clone();
                }
            }
            "also_scan_dirs" => {
                // 该选项是「追加」语义：用户写了就在默认之上再加
                for item in args {
                    config.also_scan.extend(csv(item));
                }
            }
            "dont_scan_dirs" => config.dont_scan_dirs.extend(args.iter().flat_map(|a| csv(a))),
            "dont_scan_files" => config.dont_scan_files.extend(args.iter().flat_map(|a| csv(a))),
            "dont_scan_volumes" => {
                config
                    .dont_scan_volumes
                    .extend(args.iter().flat_map(|a| csv(a)))
            }
            "fold_linux_kernels" => {
                config.fold_linux_kernels = args
                    .first()
                    .map(|value| !matches!(value.to_ascii_lowercase().as_str(), "false" | "0" | "off"))
                    .unwrap_or(true);
            }
            "use_nvram" => {
                config.use_nvram = args
                    .first()
                    .map(|value| !matches!(value.to_ascii_lowercase().as_str(), "false" | "0" | "off"))
                    .unwrap_or(true);
            }
            "default_selection" => {
                if let Some(value) = args.first() {
                    config.default_selection = value.clone();
                }
            }
            "include" => {
                if let Some(name) = args.first() {
                    read_config(&dir.join(name), config, depth + 1);
                }
            }
            _ => {}
        }
    }
}

/// 一行里花括号的净增量（去注释后数 `{` 减 `}`）。
fn brace_delta(line: &str) -> i32 {
    let mut quoted = false;
    let mut delta = 0;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            '#' if !quoted => break,
            '{' if !quoted => delta += 1,
            '}' if !quoted => delta -= 1,
            _ => {}
        }
    }
    delta
}

// ─────────────────────────── 小工具 ───────────────────────────

/// UTF-16LE 字节 → 字符串（按第一个 NUL 截断）。
///
/// rEFInd 往 vars 目录写的是「原始变量值」= 标题的 UTF-16LE + 结尾的 0x0000，
/// 而且 `egSaveFile` **不截断**，旧的长标题会把尾巴留下来 —— 不按 NUL 截断就会
/// 读到 `...partition\0lume ` 这种脏串。
fn utf16le_to_string(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// 有效 EFI 可执行的最低校验：`MZ` 头 + 合理的 `e_lfanew` 处有 `PE\0\0`。
/// 对应 `scan.c:649` 的 `IsValidLoader`（它做得更细，但对「是不是可执行」的
/// 判断等价；读错文件头就跳过，绝不把所有 `.efi` 都当系统）。
fn is_valid_loader(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 0x40];
    if file.read_exact(&mut header).is_err() {
        return false;
    }
    if &header[0..2] != b"MZ" {
        return false;
    }
    let lfanew = u32::from_le_bytes([header[0x3c], header[0x3d], header[0x3e], header[0x3f]]) as u64;
    if lfanew == 0 || lfanew > 2 * 1024 * 1024 {
        // 太小/离谱的偏移：仍然认为是可执行（有些精简 loader 只有 MZ），
        // 但不再往下读 —— 免得为了一个坏文件卡住整轮扫描
        return true;
    }
    use std::io::{Seek, SeekFrom};
    if file.seek(SeekFrom::Start(lfanew)).is_err() {
        return true;
    }
    let mut signature = [0u8; 4];
    match file.read_exact(&mut signature) {
        Ok(()) => &signature == b"PE\0\0",
        Err(_) => true,
    }
}

/// 路径比较用：去掉前导反斜杠、统一用 `\`、小写。
fn norm_path(path: &str) -> String {
    path.replace('/', "\\")
        .trim_start_matches('\\')
        .to_ascii_lowercase()
}

/// `FilenameIn` 的等义实现（`lib.c:1901`）。
///
/// rEFInd 的规则：黑名单元素可以是 `卷标:路径`、纯路径、或纯文件名，
/// 三段**分别精确比对**（大小写不敏感）。我们拿不到分区 GUID（那需要
/// diskutil/lsblk），所以带卷限定的元素按「只看路径部分」宽松匹配 ——
/// 只会多隐藏、不会漏隐藏，方向是安全的。
fn filename_in(volume: &str, dir_path: &str, filename: &str, list: &[String]) -> Option<String> {
    for element in list {
        let (vol_part, path_part) = match element.split_once(':') {
            // `C:` 这种盘符形式不是卷限定（rEFInd 的卷限定是 GUID/卷标）
            Some((head, rest)) if head.len() > 1 && !head.ends_with('\\') => (Some(head), rest),
            _ => (None, element.as_str()),
        };
        if let Some(vol) = vol_part {
            if !vol.eq_ignore_ascii_case(volume) {
                // 卷名对不上：GUID 形式我们认不出来，退化成「只看路径」
                if vol.len() > 2 && vol.contains('-') {
                    // 当作 GUID 处理：忽略卷限定
                } else {
                    continue;
                }
            }
        }
        let target = norm_path(path_part);
        let dir = norm_path(dir_path);
        let file = filename.to_ascii_lowercase();
        let (target_dir, target_file) = match target.rsplit_once('\\') {
            Some((parent, name)) => (parent.to_string(), name.to_string()),
            None => (String::new(), target.clone()),
        };
        let dir_matches = target_dir.is_empty() || target_dir == dir;
        let file_matches = target_file.is_empty() || target_file == file;
        if dir_matches && file_matches && (!target.is_empty()) {
            return Some(element.clone());
        }
    }
    None
}

/// 「上次启动」判定：rEFInd 用 `FindMenuShortcutEntry` 找**包含** DefaultSelection
/// 的条目（`menu.c:1812` 的注释原文就是 "includes *DefaultSelection as a substring"），
/// 是子串/通配语义，不是精确相等。
///
/// 卷标改过名时（例如 PreviousBoot 里还留着 "Basic data partition"，而卷现在是
/// "WINDOWS-EFI"）匹配不上是**正常结果**：rEFInd 自己也会因此丢掉 `+`、退回默认
/// 第一项。`preview` 末尾会给一条 issue 把这事说清楚，不假装标上了。
fn matches_previous(previous: Option<&str>, title: &str) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    let pattern = previous.trim().to_ascii_lowercase();
    let target = title.trim().to_ascii_lowercase();
    !pattern.is_empty() && (target.contains(&pattern) || pattern.contains(&target))
}

/// 文件名线索 → 图标 kind（`SetLoaderDefaults`，`scan.c:520-586` 的简化版）。
fn kind_of(path: &str) -> &'static str {
    let lower = norm_path(path);
    let name = lower.rsplit('\\').next().unwrap_or(&lower).to_string();
    if name == "bootmgfw.efi" || name == "bkpbootmgfw.efi" {
        return "windows";
    }
    if lower.contains("\\oc\\") || name == "opencore.efi" {
        return "opencore";
    }
    if lower.contains("\\clover\\") || name == "cloverx64.efi" {
        return "clover";
    }
    if name == "boot.efi" {
        return "macos";
    }
    if lower.contains("grub")
        || lower.contains("shim")
        || lower.contains("vmlinuz")
        || lower.contains("\\systemd\\")
        || lower.contains("\\arch\\")
        || lower.contains("\\ubuntu\\")
        || lower.contains("\\fedora\\")
        || lower.contains("\\cachyos\\")
        || lower.contains("\\endeavouros\\")
        || lower.contains("\\manjaro\\")
        || lower.contains("\\linux")
    {
        return "linux";
    }
    "unknown"
}

/// 目录内的文件按 mtime 降序（`scan.c:680` TimeComp —— 最新编译的排最前）。
fn sort_by_mtime_desc(files: &mut [(String, std::time::SystemTime)]) {
    files.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
}

// ─────────────────────────── 主流程 ───────────────────────────

/// 把 `EFI/refind/vars` 反推到 ESP 根（`vars` → `refind` → `EFI` → 根）。
fn esp_root_of(vars_dir: &Path) -> Option<PathBuf> {
    let refind_dir = vars_dir.parent()?;
    let efi_dir = refind_dir.parent()?;
    let root = efi_dir.parent()?;
    Some(root.to_path_buf())
}

/// 模拟一次完整扫描（纯读）。
pub(crate) fn preview(vars_dir: &Path) -> SimPreview {
    let _span = perf::span("refind.sim");
    let vars_dir_text = vars_dir.display().to_string();
    let mut issues: Vec<HealthIssue> = Vec::new();
    let mut config = RefindConfig::default();
    let mut config_path = None;

    let Some(esp_root) = esp_root_of(vars_dir) else {
        return SimPreview {
            entries: Vec::new(),
            hidden: Vec::new(),
            issues: vec![HealthIssue {
                severity: "warn".to_string(),
                text: format!(
                    "变量目录 {vars_dir_text} 不在 <ESP>/EFI/refind/vars 的位置上，无法反推 ESP 根，模拟扫描跳过"
                ),
            }],
            summary: "未扫描".to_string(),
            vars_dir: vars_dir_text,
            config_path: None,
            scanfor: DEFAULT_SCANFOR.to_string(),
            var_mode: "file".to_string(),
            previous_boot: None,
            default_selection: String::new(),
        };
    };

    let conf = esp_root.join("EFI").join("refind").join("refind.conf");
    if conf.is_file() {
        read_config(&conf, &mut config, 0);
        config_path = Some(conf.display().to_string());
    } else {
        issues.push(HealthIssue {
            severity: "info".to_string(),
            text: format!(
                "没读到 {}，下面全部按 rEFInd 默认配置（scanfor=ieom、fold_linux_kernels、默认黑名单）复算",
                conf.display()
            ),
        });
    }

    // 「记住启动项」的当前值：use_nvram=false 时就在 vars 目录里
    let previous_boot = fs::read(vars_dir.join("PreviousBoot"))
        .ok()
        .map(|bytes| utf16le_to_string(&bytes))
        .filter(|text| !text.is_empty());

    let var_mode = if config.use_nvram {
        "nvram".to_string()
    } else {
        "file".to_string()
    };
    if config.use_nvram {
        issues.push(HealthIssue {
            severity: "info".to_string(),
            text: "use_nvram=true：隐藏项（HiddenTags）和 PreviousBoot 都存在固件 NVRAM 里。\
                   非提权读不到，因此下面的隐藏过滤只用了 refind.conf 的 dont_scan_files；\
                   PreviousBoot 一栏显示的是 vars 目录里的旧副本（可能不是最新）"
                .to_string(),
        });
    }

    // 隐藏名单：HiddenTags 文件（use_nvram=false 时 rEFInd 就写这里）
    let hidden_tags: Vec<String> = fs::read(vars_dir.join("HiddenTags"))
        .ok()
        .map(|bytes| utf16le_to_string(&bytes))
        .map(|text| csv(&text))
        .unwrap_or_default();

    let volumes = {
        let _span = perf::span("refind.sim_volumes");
        oc_menu::volumes()
    };
    if volumes.is_empty() {
        issues.push(HealthIssue {
            severity: "warn".to_string(),
            text: "一块能读到 EFI 目录的卷都没有（UEFI 没有 NTFS/ext4 驱动，这两类分区上的 \
                   引导器 rEFInd 自己也看不到），菜单会是空的"
                .to_string(),
        });
    }

    let mut entries: Vec<SimEntry> = Vec::new();
    let scan_efi = config.scanfor.to_ascii_lowercase().contains('i')
        || config.scanfor.to_ascii_lowercase().contains('e')
        || config.scanfor.to_ascii_lowercase().contains('o');

    if scan_efi {
        for volume in &volumes {
            if config
                .dont_scan_volumes
                .iter()
                .any(|blocked| blocked.eq_ignore_ascii_case(&volume.label))
            {
                continue;
            }
            scan_volume(
                volume,
                &config,
                &hidden_tags,
                previous_boot.as_deref(),
                &mut entries,
            );
        }
    }

    // ScanUserConfigured：refind.conf 里手写的 menuentry。`disabled` 的条目
    // rEFInd 直接丢弃（连隐藏都不算），这里放进 hidden 表并写明原因 ——
    // 用户看到「为什么我写了 stanza 却没出现在菜单里」时，答案就在这一行。
    for stanza in &config.menu_entries {
        let kind = if stanza.loader.is_empty() {
            "unknown".to_string()
        } else {
            kind_of(&stanza.loader).to_string()
        };
        let path = if stanza.loader.is_empty() {
            "（stanza 没写 loader）".to_string()
        } else if stanza.volume.is_empty() {
            stanza.loader.clone()
        } else {
            format!("{} · {}", stanza.volume, stanza.loader)
        };
        let note = if stanza.disabled {
            "stanza 里写了 disabled：rEFInd 会直接丢掉这一条，菜单上看不到".to_string()
        } else {
            "手写 stanza：由 refind.conf 的 menuentry 定义，不来自文件扫描".to_string()
        };
        // 子屏：默认项 + 手写的 submenuentry（`config.c:880-1036`）
        let mut children = vec![default_child()];
        for sub in &stanza.subs {
            children.push(SimChild {
                title: sub.title.clone(),
                options: sub.options.clone(),
                source: "stanza".to_string(),
                note: (!sub.loader.is_empty()).then(|| format!("loader: {}", sub.loader)),
            });
        }
        let subscreen = (!stanza.subs.is_empty()).then(|| {
            format!(
                "Boot Options for {} on {}",
                stanza
                    .loader
                    .rsplit(['\\', '/'])
                    .next()
                    .unwrap_or(stanza.loader.as_str()),
                stanza.volume
            )
        });
        entries.push(SimEntry {
            title: stanza.title.clone(),
            kind,
            source: "user".to_string(),
            volume: stanza.volume.clone(),
            path,
            hidden: stanza.disabled,
            last_boot: matches_previous(previous_boot.as_deref(), &stanza.title),
            hidden_by: stanza.disabled.then(|| "stanza 里写了 disabled".to_string()),
            note: Some(note),
            children,
            subscreen,
        });
    }

    // `scanfor` 里的 f（固件 Boot#### 项）—— 非提权读不到，如实交代
    if config.scanfor.to_ascii_lowercase().contains('f') {
        issues.push(HealthIssue {
            severity: "info".to_string(),
            text: "scanfor 含 f：rEFInd 还会把固件 NVRAM 里的 Boot#### 列成「Reboot to …」条目。\
                   那些在 NVRAM 里，非提权读不到，这里不列（切换它们请用 OC 卡片或固件启动菜单）"
                .to_string(),
        });
    }

    // PreviousBoot 有值、但菜单里一项都没标上 —— 大概率是卷标改过名（历史值）。
    // rEFInd 在这种情况下的真实行为：`default_selection` 的 `+` 被丢掉、退回默认第一项。
    // 与其假装标上了，不如把这句说出来。
    if let Some(previous) = &previous_boot {
        if !entries.iter().any(|entry| entry.last_boot) {
            issues.push(HealthIssue {
                severity: "info".to_string(),
                text: format!(
                    "PreviousBoot 里的标题「{}」在当前菜单里找不到对应项（通常是卷标改过名）：\
                     rEFInd 会把 default_selection 里的 + 丢掉、退回默认第一项",
                    previous.trim()
                ),
            });
        }
    }

    let (visible, hidden): (Vec<SimEntry>, Vec<SimEntry>) =
        entries.into_iter().partition(|entry| !entry.hidden);
    // 第二层统计：只有一条「用默认选项启动」的子屏没意义，不计数
    let with_subscreen = visible
        .iter()
        .filter(|entry| entry.children.len() > 1)
        .count();
    let variants: usize = visible
        .iter()
        .filter(|entry| entry.children.len() > 1)
        .map(|entry| entry.children.len())
        .sum();
    let summary = format!(
        "扫描 {} 个卷 · scanfor={} · 菜单 {} 项{}{}",
        volumes.len(),
        config.scanfor,
        visible.len(),
        if hidden.is_empty() {
            String::new()
        } else {
            format!("（另有 {} 项被隐藏规则挡掉）", hidden.len())
        },
        if with_subscreen > 0 {
            format!("（{with_subscreen} 项有第二层，共 {variants} 个启动变体）")
        } else {
            String::new()
        }
    );

    SimPreview {
        entries: visible,
        hidden,
        issues,
        summary,
        vars_dir: vars_dir_text,
        config_path,
        scanfor: config.scanfor,
        var_mode,
        previous_boot,
        default_selection: config.default_selection,
    }
}

/// `SetLoaderDefaults`（`scan.c:478`、`519-568`）里的 OSType 判定。
///
/// 子菜单内容完全由它决定：`L` 读 `refind_linux.conf`、`M` 是 macOS 那组写死的
/// 诊断项、其余类型 rEFInd 只给一条「用默认选项启动」。判定次序与源码一致
/// （Linux 最先，命名线索只取文件名 `Basename`）。
fn os_type_of(loader_path: &str) -> char {
    let lower = loader_path.to_ascii_lowercase();
    let name = lower.rsplit(['\\', '/']).next().unwrap_or(lower.as_str());
    // LINUX_PREFIXES：x64 是 `vmlinuz,bzImage,kernel`（大小写不敏感的子串匹配）
    if ["vmlinuz", "bzimage", "kernel"].iter().any(|hint| name.contains(hint)) {
        return 'L';
    }
    if name.contains("refit") || name.contains("refind") {
        return 'R';
    }
    if lower.contains("system\\library\\coreservices\\boot.efi")
        || lower.contains("system/library/coreservices/boot.efi")
    {
        return 'M';
    }
    if name.contains("elilo") || name == "e.efi" {
        return 'E';
    }
    if name.contains("grub") {
        return 'G';
    }
    if matches!(
        name,
        "cdboot.efi" | "bootmgr.efi" | "bootmgfw.efi" | "bkpbootmgfw.efi"
    ) {
        return 'W';
    }
    if name == "xom.efi" {
        return 'X';
    }
    if name.contains("ipxe") {
        return 'N';
    }
    '\0'
}

/// 读同目录的 `refind_linux.conf` / `refind-linux.conf`（`config.c:74`）。
///
/// 每行 = `<标签> <options...>`，标签通常带引号。返回 `None` 表示没有这个文件 ——
/// 注意 rEFInd 还会退到 `/etc/fstab` 与分区类型推测（`config.c:1252-1256`），
/// 那两条我们没法复算，所以这里只认文件，没有就如实说「读不到」。
fn read_linux_options(dir: &Path) -> Option<Vec<(String, String)>> {
    for name in ["refind_linux.conf", "refind-linux.conf"] {
        let Ok(text) = fs::read_to_string(dir.join(name)) else {
            continue;
        };
        let rows = parse_options_text(&text);
        if !rows.is_empty() {
            return Some(rows);
        }
    }
    None
}

/// `refind_linux.conf` 的行解析：引号标签 + 其余 options（`ReadTokenLine` 的语义）。
fn parse_options_text(text: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        rows.push(split_option_line(line));
    }
    rows
}

fn split_option_line(line: &str) -> (String, String) {
    if let Some(rest) = line.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (
                rest[..end].to_string(),
                rest[end + 1..].trim().to_string(),
            );
        }
    }
    match line.find(char::is_whitespace) {
        Some(index) => (line[..index].to_string(), line[index..].trim().to_string()),
        None => (String::new(), line.to_string()),
    }
}

/// `FindNumbers()`（`mystrings.c:245`）：文件名里**第一个数字**到**最后一个数字**
/// 之间的整段（含点、连字符），例如 `vmlinuz-linux-6.9.7-arch1-1` → `6.9.7-arch1-1`。
/// 用于把 options 里的 `%v`（`KERNEL_VERSION`）换成真版本号 —— 界面上该显示实际
/// 参数，而不是一个占位符。
fn kernel_version_of(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let start = chars.iter().position(|c| c.is_ascii_digit());
    let end = chars.iter().rposition(|c| c.is_ascii_digit());
    match (start, end) {
        (Some(start), Some(end)) if end >= start => chars[start..=end].iter().collect(),
        _ => String::new(),
    }
}

fn replace_kernel_version(options: &str, kernel_name: &str) -> String {
    let version = kernel_version_of(kernel_name);
    if version.is_empty() || !options.contains("%v") {
        return options.to_string();
    }
    options.replace("%v", &version)
}

fn sub_child(title: &str, options: &str, source: &str) -> SimChild {
    SimChild {
        title: title.to_string(),
        options: options.to_string(),
        source: source.to_string(),
        note: None,
    }
}

/// `InitializeSubScreen`（`scan.c:236`）无条件塞进来的第一项。
fn default_child() -> SimChild {
    SimChild {
        title: "Boot using default options".to_string(),
        options: String::new(),
        source: "default".to_string(),
        note: Some(
            "rEFInd 给每个 loader 都建了子屏，这一项就是用它自己的默认参数启动".to_string(),
        ),
    }
}

/// macOS 条目的子屏（`scan.c:291-368`）：一组建死的诊断项，
/// 外加 `.diagnostics/diags.efi` 存在时的 Apple Hardware Test。
fn macos_children(root: &Path) -> Vec<SimChild> {
    let mut rows = vec![
        sub_child("Boot macOS with a 64-bit kernel", "arch=x86_64", "macos"),
        sub_child("Boot macOS with a 32-bit kernel", "arch=i386", "macos"),
        sub_child("Boot macOS in verbose mode", "-v", "macos"),
        sub_child("Boot macOS in verbose mode (64-bit)", "-v arch=x86_64", "macos"),
        sub_child("Boot macOS in verbose mode (32-bit)", "-v arch=i386", "macos"),
        sub_child("Boot macOS in single user mode", "-v -s", "macos"),
        sub_child("Boot macOS in safe mode", "-v -x", "macos"),
    ];
    if grub_rel(root, "System/Library/CoreServices/.diagnostics/diags.efi").is_file() {
        let mut entry = sub_child("Run Apple Hardware Test", "", "macos");
        entry.note = Some("loader 换成 .diagnostics/diags.efi".to_string());
        rows.push(entry);
    }
    rows
}

/// 单卷扫描（对应 `ScanEfiFiles`，`scan.c:1059`）。
fn scan_volume(
    volume: &oc_menu::Volume,
    config: &RefindConfig,
    hidden_tags: &[String],
    previous_boot: Option<&str>,
    out: &mut Vec<SimEntry>,
) {
    let root = &volume.root;
    let label = volume.label.clone();

    // ① macOS：System\Library\CoreServices\boot.efi（纯 FAT 世界里基本见不到，
    //    见到就照 macOS 记一条 —— 与 rEFInd 一致）
    let macos_loader = "System/Library/CoreServices/boot.efi";
    if grub_rel(root, macos_loader).is_file() {
        let path = "System\\Library\\CoreServices\\boot.efi".to_string();
        let hidden_by = filename_in(&label, "System\\Library\\CoreServices", "boot.efi", &config.dont_scan_files)
            .or_else(|| filename_in(&label, "System\\Library\\CoreServices", "boot.efi", hidden_tags));
        // macOS 的子屏是一组写死的诊断项（`scan.c:291-368`）
        let mut children = vec![default_child()];
        children.extend(macos_children(root));
        out.push(SimEntry {
            title: if label.is_empty() {
                "Boot macOS ".to_string()
            } else {
                format!("Boot macOS from {label} ")
            },
            kind: "macos".to_string(),
            source: "macos".to_string(),
            volume: label.clone(),
            path: path.clone(),
            hidden: hidden_by.is_some(),
            last_boot: matches_previous(previous_boot, &format!("Boot macOS from {label}")),
            hidden_by,
            note: None,
            children,
            subscreen: Some(format!("Boot Options for {path} on {label}")),
        });
    }

    // ② Windows：EFI\Microsoft\Boot\bootmgfw.efi，标题固定 "Microsoft EFI boot"
    //   （`scan.c:1116-1122`）—— 用户 vars 里存的正是这个标题
    for (relative, dir, name, title_text) in [
        (
            "EFI/Microsoft/Boot/bkpbootmgfw.efi",
            "EFI\\Microsoft\\Boot",
            "bkpbootmgfw.efi",
            "Microsoft EFI boot (Boot Repair backup)",
        ),
        (
            "EFI/Microsoft/Boot/bootmgfw.efi",
            "EFI\\Microsoft\\Boot",
            "bootmgfw.efi",
            "Microsoft EFI boot",
        ),
    ] {
        if !grub_rel(root, relative).is_file() {
            continue;
        }
        let hidden_by = filename_in(&label, dir, name, &config.dont_scan_files)
            .or_else(|| filename_in(&label, dir, name, hidden_tags));
        let full = format!("{title_text} from {label} ");
        let windows_path = relative.replace('/', "\\");
        out.push(SimEntry {
            title: if label.is_empty() {
                format!("Boot {title_text} ")
            } else {
                format!("Boot {full}")
            },
            kind: "windows".to_string(),
            source: "windows".to_string(),
            volume: label.clone(),
            path: windows_path.clone(),
            hidden: hidden_by.is_some(),
            last_boot: matches_previous(previous_boot, &full),
            hidden_by,
            note: None,
            // OSType 'W'：子屏里只有「用默认选项启动」这一条
            children: vec![default_child()],
            subscreen: Some(format!("Boot Options for {windows_path} on {label}")),
        });
    }

    // ③ 根目录 + EFI\<子目录> 的 *.efi（scan.c:1129-1155）
    let mut dirs: Vec<(String, PathBuf)> = vec![(String::new(), root.clone())];
    if let Ok(read) = fs::read_dir(grub_rel(root, "EFI")) {
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if !item.path().is_dir() {
                continue;
            }
            let relative = format!("EFI\\{name}");
            if is_blocked_dir(&relative, &config.dont_scan_dirs) {
                continue;
            }
            dirs.push((relative, item.path()));
        }
    }
    // also_scan_dirs（默认 boot、@/boot；`@` 前缀表示「所有卷」）
    for extra in &config.also_scan {
        let clean = extra.trim_start_matches('@').trim_matches('/').trim_matches('\\');
        if clean.is_empty() {
            continue;
        }
        let relative = clean.replace('/', "\\");
        if is_blocked_dir(&relative, &config.dont_scan_dirs) {
            continue;
        }
        let path = grub_rel(root, clean);
        if path.is_dir() {
            dirs.push((relative, path));
        }
    }

    for (relative_dir, absolute_dir) in dirs {
        let display_dir = if relative_dir.is_empty() {
            String::new()
        } else {
            relative_dir.clone()
        };
        let Ok(read) = fs::read_dir(&absolute_dir) else {
            continue;
        };
        let mut candidates: Vec<(String, std::time::SystemTime)> = Vec::new();
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().to_string();
            let path = item.path();
            if !path.is_file() {
                continue;
            }
            // `scan.c:928`：`.` 开头（macOS 在 FAT 上生成的 AppleDouble `._xxx`）
            if name.starts_with('.') {
                continue;
            }
            let lower = name.to_ascii_lowercase();
            // rEFInd 扫的是 `*.efi` **加上** `vmlinuz*,bzImage*,kernel*`
            // （`SetLinuxMatchPatterns`，`config.c:498`）—— 内核不是 .efi 后缀，
            // 漏了这条就看不到「一个 Linux 条目 + 一堆内核」的折叠效果
            let is_efi = lower.ends_with(".efi");
            let is_kernel = ["vmlinuz", "bzimage", "kernel"]
                .iter()
                .any(|prefix| lower.starts_with(prefix));
            if !is_efi && !is_kernel {
                continue;
            }
            // `scan.c:930-931`：fallback 文件在 EFI\BOOT 里跳过（单独一条）
            if display_dir.eq_ignore_ascii_case("EFI\\BOOT")
                && matches!(lower.as_str(), "bootx64.efi" | "bootaa64.efi" | "bootia32.efi")
            {
                continue;
            }
            // `.efi.signed` 存在时跳过未签名的那份（`HasSignedCounterpart`）
            if absolute_dir.join(format!("{name}.signed")).is_file() {
                continue;
            }
            if !is_valid_loader(&path) {
                continue;
            }
            let modified = item
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            candidates.push((name, modified));
        }
        sort_by_mtime_desc(&mut candidates);
        // `fold_linux_kernels`：同一目录里第二个及以后的 Linux 内核不再占主菜单
        // 一行，而是挂进「第一个内核」的子屏（`scan.c:957-971`）。
        let mut first_kernel: Option<usize> = None;
        for (name, _) in candidates {
            let relative = if display_dir.is_empty() {
                name.clone()
            } else {
                format!("{display_dir}\\{name}")
            };
            let os_type = os_type_of(&relative);
            let is_linux = os_type == 'L';

            // 折叠：没有 refind_linux.conf 的内核，rEFInd 会**静默丢掉**
            // （`AddKernelToSubmenu` 见 File == NULL 直接返回）—— 这里照做，
            // 但让调用方知道少了谁，免得以为我们漏扫了
            if is_linux && config.fold_linux_kernels {
                if let Some(index) = first_kernel {
                    let Some(rows) = read_linux_options(&absolute_dir) else {
                        continue;
                    };
                    for (label, options) in rows {
                        out[index].children.push(SimChild {
                            title: format!(
                                "{}: {}",
                                name,
                                if label.is_empty() {
                                    "Boot Linux".to_string()
                                } else {
                                    label
                                }
                            ),
                            options: replace_kernel_version(&options, &name),
                            source: "kernel".to_string(),
                            note: None,
                        });
                    }
                    continue;
                }
            }

            let hidden_by = filename_in(&volume.label, &display_dir, &name, &config.dont_scan_files)
                .or_else(|| filename_in(&volume.label, &display_dir, &name, hidden_tags));
            let title = if volume.label.is_empty() {
                format!("Boot {relative} ")
            } else {
                format!("Boot {relative} from {} ", volume.label)
            };
            let note = if is_linux && config.fold_linux_kernels {
                Some(
                    "Linux 内核：同目录里其它内核会折叠进这一项的子菜单（候选项按 \
                     refind_linux.conf 展开）"
                        .to_string(),
                )
            } else if relative.to_ascii_lowercase().contains("\\oc\\")
                || name.eq_ignore_ascii_case("opencore.efi")
            {
                Some("OpenCore 自己：选中它只会再进一次 OC 菜单".to_string())
            } else {
                None
            };

            // 子屏：rEFInd 给每个条目都建了一条（`InitializeSubScreen`），
            // 内容按 OSType 分发（`GenerateSubScreen`）
            let mut children = vec![default_child()];
            match os_type {
                'L' => {
                    if let Some(rows) = read_linux_options(&absolute_dir) {
                        // 第一行不是新加一项，而是把「默认项」的标题换掉（`scan.c:382-385`）
                        if let Some((label, options)) = rows.first() {
                            let title = if label.is_empty() {
                                "Boot Linux".to_string()
                            } else {
                                label.clone()
                            };
                            children[0].title = title;
                            children[0].options = replace_kernel_version(options, &name);
                            children[0].source = "options".to_string();
                            children[0].note = None;
                        }
                        for (label, options) in rows.iter().skip(1) {
                            children.push(SimChild {
                                title: if label.is_empty() {
                                    "Boot Linux".to_string()
                                } else {
                                    label.clone()
                                },
                                options: replace_kernel_version(options, &name),
                                source: "options".to_string(),
                                note: None,
                            });
                        }
                    }
                }
                'M' => children.extend(macos_children(root)),
                _ => {}
            }

            out.push(SimEntry {
                title: title.clone(),
                kind: kind_of(&relative).to_string(),
                source: "internal".to_string(),
                volume: volume.label.clone(),
                path: relative.clone(),
                hidden: hidden_by.is_some(),
                last_boot: matches_previous(previous_boot, &title),
                hidden_by,
                note,
                children,
                subscreen: Some(format!(
                    "Boot Options for {relative} on {}",
                    volume.label
                )),
            });
            if is_linux && first_kernel.is_none() {
                first_kernel = Some(out.len() - 1);
            }
        }
    }

    // ④ fallback：EFI\BOOT\BOOTX64.EFI → 标题固定 "Fallback boot loader"（scan.c:1176）
    for (relative, name) in [
        ("EFI/BOOT/BOOTX64.EFI", "BOOTX64.EFI"),
        ("EFI/BOOT/BOOTAA64.EFI", "BOOTAA64.EFI"),
        ("EFI/BOOT/BOOTIA32.EFI", "BOOTIA32.EFI"),
    ] {
        if !grub_rel(root, relative).is_file() {
            continue;
        }
        if is_blocked_dir("EFI\\BOOT", &config.dont_scan_dirs) {
            continue;
        }
        let hidden_by = filename_in(&label, "EFI\\BOOT", name, &config.dont_scan_files)
            .or_else(|| filename_in(&label, "EFI\\BOOT", name, hidden_tags));
        let title = if label.is_empty() {
            "Boot Fallback boot loader ".to_string()
        } else {
            format!("Boot Fallback boot loader from {label} ")
        };
        let fallback_path = relative.replace('/', "\\");
        out.push(SimEntry {
            title: title.clone(),
            kind: kind_of(relative).to_string(),
            source: "fallback".to_string(),
            volume: label.clone(),
            path: fallback_path.clone(),
            hidden: hidden_by.is_some(),
            last_boot: matches_previous(previous_boot, &title),
            hidden_by,
            note: Some(
                "EFI\\BOOT 兜底项：rEFInd 找不到别的引导器时固件也会走这里".to_string(),
            ),
            children: vec![default_child()],
            subscreen: Some(format!("Boot Options for {fallback_path} on {label}")),
        });
    }
}

/// 目录是否在 `dont_scan_dirs` 里（rEFInd 的元素可能是 `卷:GUID` 前缀，这边只比路径）。
fn is_blocked_dir(relative: &str, list: &[String]) -> bool {
    let target = norm_path(relative);
    list.iter().any(|element| {
        let path_part = element
            .split_once(':')
            .filter(|(head, _)| head.len() > 2 && head.contains('-'))
            .map(|(_, rest)| rest)
            .unwrap_or(element.as_str());
        let blocked = norm_path(path_part);
        !blocked.is_empty() && (blocked == target || target.starts_with(&format!("{blocked}\\")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 纯函数：UTF-16LE 必须按第一个 NUL 截断（rEFInd 写文件不截断，尾巴有旧残留）。
    #[test]
    fn utf16_truncates_at_nul() {
        let mut bytes: Vec<u8> = "Boot Microsoft EFI boot".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&"lume ".encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>());
        assert_eq!(utf16le_to_string(&bytes), "Boot Microsoft EFI boot");
    }

    /// 默认值必须与 rEFInd 编译期常量一致（`config.h` / `main.c:488`）。
    #[test]
    fn config_defaults_match_refind() {
        let config = RefindConfig::default();
        assert_eq!(config.scanfor, "ieom");
        assert!(config.dont_scan_files.iter().any(|n| n == "mmx64.efi"));
        assert!(config
            .dont_scan_dirs
            .iter()
            .any(|n| n.eq_ignore_ascii_case("EFI/refind")));
        // rEFInd 默认把 RefindGuid 变量写 NVRAM（这台的 ESP 上配了 use_nvram false，
        // 但那是配置出来的，不是默认值）
        assert!(config.use_nvram);
    }

    /// `FilenameIn` 语义：纯文件名 / 路径 / `卷:路径` 三种元素都能命中（大小写不敏感）。
    #[test]
    fn filename_in_matches_three_forms() {
        let list = vec![
            "mmx64.efi".to_string(),
            "EFI\\Microsoft\\Boot\\bad.efi".to_string(),
            "ARCH-EFI:\\EFI\\OC\\OpenCore.efi".to_string(),
        ];
        // 纯文件名
        assert!(filename_in("ARCH-EFI", "EFI\\BOOT", "MMX64.EFI", &list).is_some());
        // 带目录的路径（目录必须对上）
        assert!(filename_in("ARCH-EFI", "EFI\\Microsoft\\Boot", "bad.efi", &list).is_some());
        assert!(filename_in("ARCH-EFI", "EFI\\Other", "bad.efi", &list).is_none());
        // 卷限定形式：卷名对上才命中
        assert!(filename_in("ARCH-EFI", "EFI\\OC", "OpenCore.efi", &list).is_some());
        assert!(filename_in("MAC", "EFI\\OC", "OpenCore.efi", &list).is_some());
        // 无关文件
        assert!(filename_in("ARCH-EFI", "EFI\\OC", "OpenCore.efi", &["grubx64.efi".to_string()]).is_none());
    }

    /// 写进 `PreviousBoot` 的字节必须与 rEFInd 写的形态一致：
    /// UTF-16LE + 2 字节 NUL，且不带 efivarfs 那 4 字节属性头。
    #[test]
    fn previous_boot_bytes_match_refind_layout() {
        let title = "Boot EFI\\arch-efi\\grubx64.efi from ARCH-EFI ";
        let bytes = previous_boot_bytes(title);
        assert_eq!(bytes.len(), title.chars().count() * 2 + 2);
        assert_eq!(&bytes[bytes.len() - 2..], &[0, 0]);
        // 小端：'B' = 0x42 0x00
        assert_eq!(&bytes[..2], &[0x42, 0x00]);
        assert_eq!(utf16le_to_string(&bytes), title);
        // 末尾那个空格必须留住：rEFInd 的标题就带它，少一个字符就匹配不上
        assert!(title.ends_with(' '));
    }

    /// 与 ESP 上现成的快照对齐（真机验证，默认 ignore）：
    /// 把快照解成标题、再按我们的格式编回去，前缀必须逐字节一致 ——
    /// 说明「标题 + NUL」就是 rEFInd 写文件的形状（尾部是它不截断留下的残留）。
    #[test]
    #[ignore]
    fn previous_boot_bytes_match_snapshot() {
        let dir = Path::new("/Volumes/REFIND/EFI/refind/vars");
        for name in ["PreviousBoot-linux", "PreviousBoot-mac", "PreviousBoot-windows"] {
            let path = dir.join(name);
            let Ok(raw) = fs::read(&path) else { continue };
            let title = utf16le_to_string(&raw);
            let ours = previous_boot_bytes(&title);
            if raw.len() >= ours.len() {
                assert_eq!(&raw[..ours.len()], &ours[..], "{name} 前缀不一致");
            }
            println!(
                "{name}: {} 字符 · ours={} 字节 · raw={} 字节（尾部残留 {}）",
                title.chars().count(),
                ours.len(),
                raw.len(),
                raw.len().saturating_sub(ours.len())
            );
        }
    }

    /// `FindNumbers` 的语义：第一个数字到最后一段数字之间的整段。
    #[test]
    fn kernel_version_slices_between_first_and_last_digit() {
        assert_eq!(kernel_version_of("vmlinuz-linux-6.9.7-arch1-1"), "6.9.7-arch1-1");
        assert_eq!(kernel_version_of("7.0.2.efi"), "7.0.2");
        assert_eq!(kernel_version_of("vmlinuz"), "");
        assert_eq!(replace_kernel_version("root=/dev/sda2 %v rw", "vmlinuz-6.1.0"), "root=/dev/sda2 6.1.0 rw");
    }

    /// `refind_linux.conf`：引号标签 + options；`#` 注释与空行跳过。
    #[test]
    fn parses_refind_linux_conf() {
        let rows = parse_options_text(
            "# 注释\n\"Arch Linux\"\troot=PARTUUID=abc rw add_efi_memmap\n\"Arch fallback\"\troot=PARTUUID=abc rw\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "Arch Linux");
        assert_eq!(rows[0].1, "root=PARTUUID=abc rw add_efi_memmap");
        assert_eq!(rows[1].0, "Arch fallback");
        // 没写引号时第一词当标签（与 ReadTokenLine 一致）
        assert_eq!(split_option_line("root=/dev/sda2 rw"), ("root=/dev/sda2".to_string(), "rw".to_string()));
    }

    /// OSType 判定次序（`scan.c:519`）：Linux 最先，名字只取 Basename。
    #[test]
    fn os_type_matches_refind_order() {
        assert_eq!(os_type_of("EFI\\arch\\vmlinuz-linux"), 'L');
        assert_eq!(os_type_of("vmlinuz-linux-6.9"), 'L');
        assert_eq!(os_type_of("EFI\\arch\\grubx64.efi"), 'G');
        assert_eq!(os_type_of("EFI\\Microsoft\\Boot\\bootmgfw.efi"), 'W');
        assert_eq!(os_type_of("System\\Library\\CoreServices\\boot.efi"), 'M');
        // OpenCore、UKI 这类既不是内核也不是已知类型：子屏只有「默认项」
        assert_eq!(os_type_of("EFI\\OC\\OpenCore.efi"), '\0');
        assert_eq!(os_type_of("EFI\\ArchLinux\\arch-linux-lts.efi"), '\0');
    }

    /// 真机快照：本机 ESP 挂在 /Volumes/REFIND。默认 ignore —— 只用来肉眼核对输出，
    /// 别的机器上这块盘不存在，不能进常规测试。
    #[test]
    #[ignore]
    fn live_dump() {
        let dir = Path::new("/Volumes/REFIND/EFI/refind/vars");
        if !dir.is_dir() {
            println!("没有 {dir:?}，跳过");
            return;
        }
        let sim = preview(dir);
        println!("summary: {}", sim.summary);
        println!("previous_boot: {:?}", sim.previous_boot);
        println!("scanfor: {} var_mode: {}", sim.scanfor, sim.var_mode);
        for (index, entry) in sim.entries.iter().enumerate() {
            println!("  {}. [{}] {} <- {} · {}", index + 1, entry.kind, entry.title, entry.volume, entry.path);
            println!(
                "      子屏: {:?}（{} 项）",
                entry.subscreen,
                entry.children.len()
            );
            for child in &entry.children {
                println!(
                    "        · [{}] {} {}",
                    child.source,
                    child.title,
                    if child.options.is_empty() {
                        String::new()
                    } else {
                        format!("| {}", child.options)
                    }
                );
            }
        }
        for entry in &sim.hidden {
            println!("  (隐藏) [{}] {} <- {}", entry.kind, entry.title, entry.hidden_by.as_deref().unwrap_or("?"));
        }
        for issue in &sim.issues {
            println!("  [{}] {}", issue.severity, issue.text);
        }
    }
}
