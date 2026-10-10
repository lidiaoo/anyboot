//! GRUB 启动项解析 + 环境块（grubenv）读写 —— 三端共用，零外部依赖。
//!
//! 移植自 `linux/else/system-switch.sh` 的 `grub_self_check`：
//!
//! * 逐行解析 grub.cfg，按花括号深度判断层级；`submenu` 内的 `menuentry`
//!   归属该 submenu（原脚本用同样的思路，但只在独立成行的 `}` 上减深度、
//!   且不为 `function xxx {` 加深度，本模块改为**统计每一行的花括号净变化**，
//!   顺手修掉了那个偏差）。
//! * `extract_entry_id()` 的取值顺序：标题（第一个引号串）→
//!   `$menuentry_id_option` / `--id` / `--unrestricted` 后的引号串 →
//!   取不到就**退化为标题本身**。这三条与原始脚本一字不差。
//! * 写进 grubenv 的值：顶层条目用 id；submenu 内的条目用
//!   `submenu_id>entry_id`（这正是原脚本 `GRUB_ENTRY_PATH` 的格式）。
//! * `root=UUID=...` 只用于**归属判断**，不参与取值 —— 界面里用户直接点条目，
//!   不需要再按 UUID 反推。
//!
//! 环境块（grubenv）是一块**预分配的 1024 字节纯文本**：
//! 头 25 字节固定 `# GRUB Environment Block\n`，其后是 `key=value\n` 若干行，
//! 剩余空间用 `#` 补满。**写入必须原地覆盖、字节数恒为 1024** —— 尺寸一变，
//! GRUB 就会在开机时报 `error: invalid environment block`。

use serde::Serialize;

/// grubenv 在 ESP 上的相对路径（用户定案：直接放 `/EFI` 下，不建发行版子目录）。
pub const ENV_REL_PATH: &str = "/EFI/grubenv";
/// grub.cfg 在 ESP 上的相对路径（同上）。
pub const CFG_REL_PATH: &str = "/EFI/grub.cfg";

/// 环境块固定长度。
pub const ENV_BLOCK_SIZE: usize = 1024;
/// 环境块头（含换行，共 25 字节）。
pub const ENV_BLOCK_HEADER: &str = "# GRUB Environment Block\n";

/// grub.cfg 里的一个可启动项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrubEntry {
    /// 写进 grubenv 的值：顶层 = `id`，submenu 内 = `submenu_id>id`。
    pub selector: String,
    /// `--id` 一类标记取到的 id；取不到时等于 `name`（与原始脚本一致）。
    pub id: String,
    /// 菜单标题原文。
    pub name: String,
    /// 直接父级 submenu 的 id；顶层条目为 `None`。
    pub submenu: Option<String>,
    /// `--class` 的第一个值，前端用来区分 windows / memtest / os。
    pub class: Option<String>,
    /// 条目体里有 `chainloader`（Windows / OpenCore 这类转交引导）。
    pub chainloader: bool,
    /// 条目体里 `linux` 行的内核文件名（去掉路径），用于展示。
    pub kernels: Vec<String>,
    /// 折叠进来的**同名变体**数量。
    ///
    /// grub.cfg 里同一组内经常出现标题一模一样、只有 selector 不同的条目
    /// （os-prober 的 memtest86+ 就有 `memtest.bin` / `memtest.efi` 两条；
    /// 31_efi_bootnext 生成的 `UEFI OS (EFI BootNext)` 也有 0009 / 000A 两条）。
    /// 界面上连着出现两行一样的文字，用户只会当成「重复了」——所以展示时合并成
    /// 一条，这里记下它背后有几个变体（>1 时前端标出来）。
    /// 展示层恒为 1 或更大；后端解析出来的原始条目此字段为 1。
    pub variants: usize,
}

/// grub.cfg 里的一个 submenu（分组）。
///
/// 解析器**不把 submenu 当可选条目**（GRUB 里它确实不是 menuentry，写进 grubenv
/// 也切不过去），但界面要做树形展示，需要知道「这个父 id 的标题叫什么」。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrubGroup {
    /// submenu 的 id（子条目的 `submenu` 字段就指向它）
    pub id: String,
    /// 分组标题原文
    pub name: String,
}

/// 扫一遍 grub.cfg，取出所有 submenu 的 `(id, 标题)`，按出现顺序。
pub fn parse_submenus(text: &str) -> Vec<GrubGroup> {
    let mut groups = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_start();
        let Some(rest) = keyword_tail(line, "submenu") else {
            continue;
        };
        let (id, name) = heading_identity(rest);
        if !groups.iter().any(|group: &GrubGroup| group.id == id) {
            groups.push(GrubGroup { id, name });
        }
    }
    groups
}

/// 把「同名条目」折叠成一条（展示用），并数出它背后有几个变体。
///
/// grub.cfg 里同一个 submenu 内经常躺着标题**逐字相同**、只有 selector 不同的条目：
///
/// * os-prober 的 memtest86+：`…（位于 /dev/nvme0n1p14）` 有 `memtest.bin` 与
///   `memtest.efi` 两条；
/// * `31_efi_bootnext` 生成的 `UEFI OS (EFI BootNext)` 有 `0009` / `000A` 两条。
///
/// 界面上连着出现两行完全一样的文字，用户只会认为「重复了」。所以这里按
/// **(父 submenu, 标题)** 归并：只保留第一条（它的 selector 照样能切过去），
/// `variants` 记下折叠了几个。
///
/// 归并的 key 必须带父级：不同发行版的「高级选项」里都可能有叫「Arch Linux」的条目，
/// 那是真不同的东西，不能跨组消掉。顶层条目（无 submenu）按标题归并。
///
/// 返回值与入参**顺序一致**（按首次出现的时机定位），条目字段除 `variants` 外不变。
pub fn dedupe_display_entries(entries: &[GrubEntry]) -> Vec<GrubEntry> {
    let mut out: Vec<GrubEntry> = Vec::with_capacity(entries.len());
    // key → out 里的下标。key 用 (submenu | "", name)。
    let mut seen: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();

    for entry in entries {
        let key = (
            entry.submenu.clone().unwrap_or_default(),
            entry.name.clone(),
        );
        match seen.get(&key) {
            Some(&index) => {
                // 已经有一条同名的了：只记数，不再往外铺一行
                out[index].variants += 1;
            }
            None => {
                seen.insert(key, out.len());
                out.push(entry.clone());
            }
        }
    }
    out
}

/// 解析 grub.cfg 全文，按出现顺序返回所有 menuentry。
///
/// 注意：**不做任何过滤**（第一版按用户要求把 os-prober 出来的 Windows /
/// OpenCore 也一并列出），调用方若要过滤再自行处理。
pub fn parse_grub_cfg(text: &str) -> Vec<GrubEntry> {
    let mut entries: Vec<GrubEntry> = Vec::new();
    // (submenu 打开时的深度, submenu 的 id)
    let mut submenu_stack: Vec<(i32, String)> = Vec::new();
    let mut depth: i32 = 0;
    // 当前正在解析的 menuentry 索引 + 它的**行所在深度**（条目体在其 +1 层）
    let mut current: Option<(usize, i32)> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim_start();
        let delta = brace_delta(line);

        if let Some(rest) = keyword_tail(line, "submenu") {
            let (id, _name) = heading_identity(rest);
            submenu_stack.push((depth, id));
        } else if let Some(rest) = keyword_tail(line, "menuentry")
            .or_else(|| keyword_tail(line, "submenuentry"))
        {
            let (id, name) = heading_identity(rest);
            let parent = submenu_stack.last().map(|(_, id)| id.clone());
            let selector = match &parent {
                Some(sub) => format!("{sub}>{id}"),
                None => id.clone(),
            };
            entries.push(GrubEntry {
                selector,
                id,
                name,
                submenu: parent,
                class: first_class(rest),
                chainloader: false,
                kernels: Vec::new(),
                variants: 1,
            });
            current = Some((entries.len() - 1, depth));
        } else if let Some((index, _)) = current {
            // 条目体内部：`linux` 行拿内核名，`chainloader` 标记转交引导。
            let entry = &mut entries[index];
            let trimmed = line.trim_end();
            if let Some(kernel) = linux_kernel(trimmed) {
                if !entry.kernels.contains(&kernel) {
                    entry.kernels.push(kernel);
                }
            } else if trimmed.starts_with("chainloader") {
                entry.chainloader = true;
            }
        }

        depth += delta;
        // 花括号回到「打开 submenu 时的那一层」说明这个 submenu 已经结束。
        while let Some((open_depth, _)) = submenu_stack.last() {
            if depth <= *open_depth {
                submenu_stack.pop();
            } else {
                break;
            }
        }
        if let Some((index, entry_depth)) = current {
            if depth <= entry_depth {
                current = None;
                let _ = index;
            }
        }
    }

    entries
}

/// `line` 是否以 `keyword` 开头且后面紧跟空白；是则返回其后的内容。
fn keyword_tail<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(keyword)?;
    if rest.is_empty() {
        return None;
    }
    if rest.starts_with(|c: char| c.is_whitespace()) {
        Some(rest.trim_start())
    } else {
        None
    }
}

/// 统计一行里花括号的净变化，**忽略引号串内部的括号**。
fn brace_delta(line: &str) -> i32 {
    let mut delta = 0;
    let mut quote: Option<char> = None;
    for ch in line.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                }
            }
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '{' => delta += 1,
                '}' => delta -= 1,
                '#' => break, // 注释之后的内容 GRUB 不解析
                _ => {}
            },
        }
    }
    delta
}

/// 从 `menuentry`/`submenu` 行「关键字之后的部分」取出 (id, 标题)。
///
/// 取值顺序与原始脚本的 `extract_entry_id()` 一致：
/// 标题 = 第一个引号串；id = 最后一个 id 标记之后的引号串；都没有则 id = 标题。
fn heading_identity(rest: &str) -> (String, String) {
    let spans = quoted_spans(rest);
    // 标题就是第一个引号串；`menuentry "X" {` 这种双引号写法也能覆盖。
    let name = spans
        .first()
        .map(|(_, text)| (*text).to_string())
        .unwrap_or_else(|| rest.trim_end_matches('{').trim().to_string());

    // 标记必须取「最后一个」：os-prober 会把 `$menuentry_id_option "…"` 这段
    // 文本本身写进标题里（见本模块单测里的 memtest 用例），取第一个会解析错。
    let marker_end = MARKERS
        .iter()
        .filter_map(|marker| last_marker_end(rest, marker))
        .max();
    let id = marker_end
        .and_then(|end| {
            spans
                .iter()
                .find(|(start, _)| *start >= end)
                .map(|(_, text)| (*text).to_string())
        })
        .filter(|value| !value.is_empty())
        // 取不到 id 时**退化为标题**，与原始脚本 `_id="$_name"` 完全一致。
        .unwrap_or_else(|| name.clone());

    (id, name)
}

/// id 标记。`$menuentry_id_option` 是 grub-mkconfig 的模板变量，
/// `--id` / `--unrestricted` 是手写 40_custom 与 ostree 发行版常见的写法。
const MARKERS: [&str; 3] = ["$menuentry_id_option", "--id", "--unrestricted"];

/// `marker` 在 `text` 中最后一次出现的位置（返回其结束下标）。
fn last_marker_end(text: &str, marker: &str) -> Option<usize> {
    text.rfind(marker).map(|start| start + marker.len())
}

/// 取出文本里所有引号串（单双引号各自成对），返回 (起始字节, 内容)。
fn quoted_spans(text: &str) -> Vec<(usize, &str)> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\'' || byte == b'"' {
            let quote = byte;
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != quote {
                end += 1;
            }
            if end < bytes.len() {
                if let Some(text) = text.get(start..end) {
                    spans.push((start, text));
                }
                index = end + 1;
                continue;
            }
            break;
        }
        index += 1;
    }
    spans
}

/// `--class` 的第一个值（GRUB 用它决定菜单图标/配色，前端拿它做分组）。
///
/// 必须先把引号串挖空再分词：os-prober 可能把 `--class memtest` 这串文本
/// 写进标题里（memtest 那几条），直接分词会把标题里的假标记当成真参数。
fn first_class(rest: &str) -> Option<String> {
    let masked = unquoted(rest);
    let mut tokens = masked.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == "--class" {
            return tokens.next().map(str::to_string);
        }
    }
    None
}

/// 把文本里的引号串替换成单个空格，只保留真正的参数部分。
fn unquoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut quote: Option<char> = None;
    for ch in text.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                }
            }
            None => {
                if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                    out.push(' ');
                } else {
                    out.push(ch);
                }
            }
        }
    }
    out
}

/// 从 `linux /vmlinuz-linux root=UUID=…` 之类的行里取出内核文件名。
fn linux_kernel(line: &str) -> Option<String> {
    let rest = keyword_tail(line, "linux")
        .or_else(|| keyword_tail(line, "linux16"))
        .or_else(|| keyword_tail(line, "linuxefi"))?;
    let path = rest.split_whitespace().next()?;
    if path.is_empty() {
        return None;
    }
    Some(
        path.rsplit('/')
            .next()
            .unwrap_or(path)
            .trim_matches(|c| c == '\'' || c == '"')
            .to_string(),
    )
}

// ---------------------------------------------------------------------------
// 环境块（grubenv）
// ---------------------------------------------------------------------------

/// 生成一块全新的空环境块：头 + `#` 补满，**恰好 1024 字节**。
///
/// 与 `grub-editenv <file> create` 的产物逐字节一致。GRUB 自己**不会创建**
/// 这个文件，所以第一次必须由外部先建好，否则 `save_env -f` 会报
/// `invalid environment block`。
pub fn empty_env_block() -> Vec<u8> {
    let mut block = vec![b'#'; ENV_BLOCK_SIZE];
    block[..ENV_BLOCK_HEADER.len()].copy_from_slice(ENV_BLOCK_HEADER.as_bytes());
    block
}

/// 把环境块解析成有序的 `key=value` 列表。
///
/// 尺寸不是 1024 或头部不符时返回 Err —— 这正是 GRUB 判定
/// `invalid environment block` 的两个条件。
pub fn parse_env_block(block: &[u8]) -> Result<Vec<(String, String)>, String> {
    if block.len() != ENV_BLOCK_SIZE {
        return Err(format!(
            "环境块尺寸应为 {ENV_BLOCK_SIZE} 字节，实际 {} 字节",
            block.len()
        ));
    }
    if !block.starts_with(ENV_BLOCK_HEADER.as_bytes()) {
        return Err("环境块头部不是 `# GRUB Environment Block`".to_string());
    }
    let text = String::from_utf8_lossy(&block[ENV_BLOCK_HEADER.len()..]);
    let mut pairs = Vec::new();
    for line in text.split('\n') {
        // `#` 开始的行是补位（或注释），到此为止。
        if line.is_empty() || line.starts_with('#') {
            break;
        }
        if let Some((key, value)) = line.split_once('=') {
            pairs.push((key.to_string(), value.to_string()));
        }
    }
    Ok(pairs)
}

/// 读取某个变量的值（不存在返回 None）。
pub fn env_value(block: &[u8], key: &str) -> Option<String> {
    parse_env_block(block)
        .ok()?
        .into_iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

/// 在已有环境块上原地写若干变量，保持**恰好 1024 字节**。
///
/// 已存在的键按原位替换，新键追加在后面，其余键保持不动（与
/// `grub-editenv set` 的语义一致）。空间不够时返回 Err，绝不截断。
pub fn set_env_vars(block: &[u8], updates: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut pairs = parse_env_block(block)?;
    for (key, value) in updates {
        match pairs.iter_mut().find(|(name, _)| name == key) {
            Some(pair) => pair.1 = value.clone(),
            None => pairs.push((key.clone(), value.clone())),
        }
    }

    let mut body = String::with_capacity(ENV_BLOCK_SIZE);
    body.push_str(ENV_BLOCK_HEADER);
    for (key, value) in &pairs {
        body.push_str(key);
        body.push('=');
        body.push_str(value);
        body.push('\n');
    }
    if body.len() > ENV_BLOCK_SIZE {
        return Err(format!(
            "环境块放不下（需要 {} 字节，上限 {ENV_BLOCK_SIZE}）",
            body.len()
        ));
    }
    let mut out = body.into_bytes();
    out.resize(ENV_BLOCK_SIZE, b'#');
    Ok(out)
}

// ---------------------------------------------------------------------------
// fs-uuid（GRUB `search --fs-uuid` 用的那串）
// ---------------------------------------------------------------------------

/// 归一化 fs-uuid：只保留十六进制数字并大写，便于跨端比对。
///
/// 同一块 FAT，GRUB 写 `E537-7F31`，Windows 报 `E5377F31`，
/// Linux `blkid` 给 `E537-7F31` —— 去掉分隔符就完全一致。
pub fn uuid_digits(raw: &str) -> Option<String> {
    let digits: String = raw
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if digits.len() < 8 || digits.len() > 32 || digits.len() % 2 != 0 {
        return None;
    }
    Some(digits)
}

/// 把归一化后的十六进制串还原成惯用写法（FAT 卷序列号 8 位 → `E537-7F31`）。
pub fn uuid_pretty(digits: &str) -> String {
    match digits.len() {
        8 => format!("{}-{}", &digits[..4], &digits[4..]),
        32 => format!(
            "{}-{}-{}-{}-{}",
            &digits[0..8],
            &digits[8..12],
            &digits[12..16],
            &digits[16..20],
            &digits[20..32]
        ),
        _ => digits.to_string(),
    }
}

/// 两个 fs-uuid 是否指同一块分区（忽略大小写与分隔符）。
pub fn uuid_matches(left: &str, right: &str) -> bool {
    match (uuid_digits(left), uuid_digits(right)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 覆盖真实 grub.cfg 里的全部形态：submenu 内条目、os-prober 名字里
    /// 嵌了 `$menuentry_id_option` 的 memtest、`--unrestricted` 的 ostree、
    /// 40_custom 的双引号无 id 项、chainloader 项。
    const SAMPLE: &str = r#"
set default="${saved_entry}"
function savedefault {
  if [ -z "${boot_once}" ]; then
    saved_entry="${chosen}"
  fi
}

menuentry 'Arch Linux' --class arch --class gnu-linux --class gnu --class os $menuentry_id_option 'gnulinux-simple-84d0b19d-4ee2-4cd4-bc2c-80a77c19c702' {
	linux	/vmlinuz-linux root=UUID=84d0b19d-4ee2-4cd4-bc2c-80a77c19c702 rw
	initrd	/initramfs-linux.img
}
submenu 'Arch Linux 的高级选项' $menuentry_id_option 'gnulinux-advanced-84d0b19d-4ee2-4cd4-bc2c-80a77c19c702' {
	menuentry 'Arch Linux, with Linux linux' --class arch $menuentry_id_option 'gnulinux-linux-advanced-84d0b19d-4ee2-4cd4-bc2c-80a77c19c702' {
		linux	/vmlinuz-linux root=UUID=84d0b19d-4ee2-4cd4-bc2c-80a77c19c702 rw
	}
}
menuentry 'Memory test (mt86+x64)" --class memtest $menuentry_id_option "memtest86+ （位于 /dev/nvme0n1p22）' --class gnu-linux --class gnu --class os $menuentry_id_option 'osprober-gnulinux-/boot/mt86+x64--fd17f5ed-78d4-4ada-932b-bc47ddc538e9' {
	linux	/boot/mt86+x64
}
menuentry 'Deepin 25 (2025/02/16 02:54:51)' --class gnu-linux --class gnu --class os --unrestricted 'ostree-0-07f0f594-6dd5-4263-9486-217bb288fde0' {
	linux	/vmlinuz root=UUID=07f0f594-6dd5-4263-9486-217bb288fde0
}
menuentry "Arch Linux (mainline)" {
	linux	/vmlinuz-linux-mainline root=UUID=84d0b19d-4ee2-4cd4-bc2c-80a77c19c702
}
menuentry "OpenCore (macOS)" {
	insmod fat
	search --no-floppy --fs-uuid --set=root A5EB-573A
	chainloader /EFI/BOOT/BOOTx64.efi
}
"#;

    #[test]
    fn parses_every_entry_with_expected_selector() {
        let entries = parse_grub_cfg(SAMPLE);
        let selectors: Vec<&str> = entries.iter().map(|e| e.selector.as_str()).collect();
        assert_eq!(
            selectors,
            vec![
                "gnulinux-simple-84d0b19d-4ee2-4cd4-bc2c-80a77c19c702",
                "gnulinux-advanced-84d0b19d-4ee2-4cd4-bc2c-80a77c19c702\
                 >gnulinux-linux-advanced-84d0b19d-4ee2-4cd4-bc2c-80a77c19c702",
                // os-prober 把 `$menuentry_id_option` 写进了标题里，必须取**最后一个**
                "osprober-gnulinux-/boot/mt86+x64--fd17f5ed-78d4-4ada-932b-bc47ddc538e9",
                "ostree-0-07f0f594-6dd5-4263-9486-217bb288fde0",
                // 40_custom 没有 id → 退化为标题（原脚本 `_id="$_name"` 的行为）
                "Arch Linux (mainline)",
                "OpenCore (macOS)",
            ]
        );
    }

    #[test]
    fn tracks_submenu_parent_and_depth() {
        let entries = parse_grub_cfg(SAMPLE);
        assert!(entries[0].submenu.is_none(), "顶层条目不该有父级 submenu");
        assert_eq!(
            entries[1].submenu.as_deref(),
            Some("gnulinux-advanced-84d0b19d-4ee2-4cd4-bc2c-80a77c19c702")
        );
        // submenu 结束之后的条目必须回到顶层（花括号深度回落）
        assert!(entries[2].submenu.is_none());
        assert!(entries[3].submenu.is_none());
    }

    #[test]
    fn parses_submenu_titles_for_tree_view() {
        // 前端要做树形层级，必须拿到「父 id → 组标题」的映射；
        // submenu 本身不是 menuentry，所以它只在 parse_submenus 里出现。
        let groups = parse_submenus(SAMPLE);
        assert_eq!(groups.len(), 1, "样例里只有一个 submenu");
        assert_eq!(
            groups[0].id,
            "gnulinux-advanced-84d0b19d-4ee2-4cd4-bc2c-80a77c19c702"
        );
        assert_eq!(groups[0].name, "Arch Linux 的高级选项");
        // 组标题绝不能出现在 menuentry 列表里（它切不过去）
        let entries = parse_grub_cfg(SAMPLE);
        assert!(
            !entries.iter().any(|entry| entry.name == "Arch Linux 的高级选项"),
            "submenu 行不是可选条目"
        );
    }

    #[test]
    fn keeps_raw_title_and_class_and_kernel() {
        let entries = parse_grub_cfg(SAMPLE);
        assert_eq!(entries[0].name, "Arch Linux");
        assert_eq!(entries[0].class.as_deref(), Some("arch"));
        assert_eq!(entries[0].kernels, vec!["vmlinuz-linux".to_string()]);
        // 双引号标题同样能取到
        assert_eq!(entries[4].name, "Arch Linux (mainline)");
        // chainloader 标记
        assert!(entries[5].chainloader);
        assert!(!entries[0].chainloader);
    }

    #[test]
    fn submenu_line_itself_is_not_an_entry() {
        let entries = parse_grub_cfg(SAMPLE);
        assert!(
            !entries.iter().any(|e| e.name.contains("的高级选项")),
            "submenu 行是分组，不是可选条目"
        );
    }

    #[test]
    fn dedupe_folds_same_titled_variants_within_a_group() {
        // os-prober 的 memtest86+ 与 31_efi_bootnext 都会产出标题逐字相同的多条，
        // 只有 selector 不同 —— 界面上必须折叠成一条，否则看起来就是「重复了」。
        const DUPLICATED: &str = r#"
submenu 'Manjaro Linux 的高级选项' $menuentry_id_option 'manjaro-advanced' {
	menuentry 'Memory Tester (memtest86+) （位于 /dev/nvme0n1p14）' --class gnu-linux $menuentry_id_option 'osprober-...memtest.bin--aaa' {
		linux /boot/memtest86+/memtest.bin
	}
	menuentry 'Memory Tester (memtest86+) （位于 /dev/nvme0n1p14）' --class gnu-linux $menuentry_id_option 'osprober-...memtest.efi--aaa' {
		linux /boot/memtest86+/memtest.efi
	}
}
menuentry 'UEFI OS (EFI BootNext)' --class uefi $menuentry_id_option 'efi-bootnext-0009' {
	chainloader /EFI/BOOT/BOOTX64.EFI
}
menuentry 'UEFI OS (EFI BootNext)' --class uefi $menuentry_id_option 'efi-bootnext-000A' {
	chainloader /EFI/BOOT/BOOTX64.EFI
}
"#;
        let raw = parse_grub_cfg(DUPLICATED);
        assert_eq!(raw.len(), 4, "原始解析应保留全部 4 条（不去重）");

        let display = dedupe_display_entries(&raw);
        assert_eq!(display.len(), 2, "两组同名条目各折叠成一条");
        // 保留的是每组里第一次出现的那条（selector 仍可切换）
        assert_eq!(display[0].name, "Memory Tester (memtest86+) （位于 /dev/nvme0n1p14）");
        assert_eq!(display[0].variants, 2);
        assert_eq!(display[1].name, "UEFI OS (EFI BootNext)");
        assert_eq!(display[1].variants, 2);
        // 逐字相同的标题不应该再出现两遍
        assert_ne!(display[0].selector, display[1].selector);
    }

    #[test]
    fn dedupe_keeps_same_title_in_different_groups() {
        // 不同发行版的「高级选项」里都可能有叫「Arch Linux」的条目，那是两个系统，
        // 绝不能跨组消掉 —— 归并 key 必须带父 submenu。
        const TWO_GROUPS: &str = r#"
submenu 'A 的高级选项' $menuentry_id_option 'a-adv' {
	menuentry 'Arch Linux' --class arch $menuentry_id_option 'a-arch' {
		linux /vmlinuz-a
	}
}
submenu 'B 的高级选项' $menuentry_id_option 'b-adv' {
	menuentry 'Arch Linux' --class arch $menuentry_id_option 'b-arch' {
		linux /vmlinuz-b
	}
}
"#;
        let raw = parse_grub_cfg(TWO_GROUPS);
        let display = dedupe_display_entries(&raw);
        assert_eq!(display.len(), 2, "跨组同名不能折叠");
        assert!(display.iter().all(|entry| entry.variants == 1));
    }

    #[test]
    fn empty_env_block_matches_grub_editenv_create() {
        let block = empty_env_block();
        assert_eq!(block.len(), ENV_BLOCK_SIZE);
        assert_eq!(&block[..ENV_BLOCK_HEADER.len()], ENV_BLOCK_HEADER.as_bytes());
        assert!(block[ENV_BLOCK_HEADER.len()..].iter().all(|b| *b == b'#'));
        assert_eq!(parse_env_block(&block).unwrap(), Vec::new());
    }

    #[test]
    fn set_env_vars_keeps_block_exactly_1024_bytes() {
        let block = empty_env_block();
        let block = set_env_vars(
            &block,
            &[
                ("saved_entry".to_string(), "Arch Linux".to_string()),
                ("next_entry".to_string(), "Windows Boot Manager".to_string()),
            ],
        )
        .unwrap();
        assert_eq!(block.len(), ENV_BLOCK_SIZE);
        assert_eq!(
            env_value(&block, "saved_entry").as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(
            env_value(&block, "next_entry").as_deref(),
            Some("Windows Boot Manager")
        );

        // 再次写入只替换原键，不新增、不涨长
        let block = set_env_vars(&block, &[("next_entry".to_string(), String::new())]).unwrap();
        assert_eq!(block.len(), ENV_BLOCK_SIZE);
        assert_eq!(env_value(&block, "next_entry").as_deref(), Some(""));
        assert_eq!(parse_env_block(&block).unwrap().len(), 2);
    }

    #[test]
    fn env_block_size_or_header_mismatch_is_an_error() {
        let short = vec![b'#'; 100];
        assert!(parse_env_block(&short).is_err());
        let mut wrong_header = empty_env_block();
        wrong_header[0] = b'X';
        assert!(parse_env_block(&wrong_header).is_err());
    }

    #[test]
    fn env_block_refuses_to_overflow() {
        let block = empty_env_block();
        let updates = vec![("k".to_string(), "v".repeat(2000))];
        assert!(set_env_vars(&block, &updates).is_err());
    }

    #[test]
    fn uuid_normalisation_matches_across_platforms() {
        // GRUB 里写 E537-7F31，Windows 把卷序列号报成 E5377F31
        assert!(uuid_matches("E537-7F31", "E5377F31"));
        assert!(uuid_matches("e537-7f31", "E5377F31"));
        assert!(!uuid_matches("E537-7F31", "A5EB-573A"));
        assert_eq!(uuid_pretty("E5377F31"), "E537-7F31");
        assert_eq!(
            uuid_pretty("84D0B19D4EE24CD4BC2C80A77C19C702"),
            "84D0B19D-4EE2-4CD4-BC2C-80A77C19C702"
        );
        assert!(uuid_digits("E537-7F31").is_some());
        assert!(uuid_digits("not-a-uuid!").is_none());
    }
}

