//! MenuNode 模型 + 菜单构建 + 指纹

use super::*;

pub(crate) fn menu_system_rows(variables: &[VariableInfo]) -> Vec<VariableInfo> {
    if !variables.is_empty() {
        return variables.to_vec();
    }
    SOURCE_FILES
        .iter()
        .map(|(system, display_name, source_name)| VariableInfo {
            system: (*system).to_string(),
            display_name: (*display_name).to_string(),
            source_name: (*source_name).to_string(),
            source: PathBuf::new(),
            exists: false,
            active: false,
        })
        .collect()
}

/// 开关项标签：`☑ 开机启动` / `☐ 开机启动`。
pub(crate) fn toggle_label(selected: bool, text: &str) -> String {
    format!("{} {text}", if selected { TOGGLE_ON } else { TOGGLE_OFF })
}

/// 单选项标签：`◉ 屏幕中间` / `○ 自由摆放`。
pub(crate) fn radio_label(selected: bool, text: &str) -> String {
    format!("{} {text}", if selected { RADIO_ON } else { RADIO_OFF })
}

/// GRUB 条目的展示文字：随列表格式变（与 `ui/grub-list-view.js` 的 `rowLabel`
/// **同一条规则** —— 极简/简易 = 去前缀内核名；中等 = 完整内核名；其余 = 菜单标题）。
pub(crate) fn grub_entry_label(entry: &grub::GrubEntry, list_format: &str) -> String {
    // JS 的 `(entry.kernels && entry.kernels[0]) || entry.name`：空内核名也回落到菜单标题
    let kernel = entry
        .kernels
        .first()
        .filter(|kernel| !kernel.is_empty())
        .map(String::as_str)
        .unwrap_or(&entry.name);
    match list_format {
        "minimal" | "simple" => {
            let stripped = kernel
                .strip_prefix("vmlinuz-")
                .or_else(|| kernel.strip_prefix("linux-"))
                .unwrap_or(kernel);
            stripped.to_string()
        }
        "medium" => kernel.to_string(),
        _ => entry.name.clone(),
    }
}

/// 「下次启动」的级联文字：rEFInd 层 → GRUB 层。
///
/// 与主窗口顶部那行大字（`ui/app.js` 的 `nextBootText`）**同一条规则**：
/// rEFInd 选中的不是 Linux（Windows/macOS 是直接引导），或者 GRUB 侧还没选过
/// → 只显示 rEFInd 这一层；选中 Linux 且能手拿把掐找到那个 GRUB 条目 →
/// 「Linux › 组名 › 条目文字」。托盘顶部、主窗口大字说的是同一个东西，
/// **改这里的规则请同步改 `ui/app.js`**（两处一字不差是用户的要求）。
pub(crate) fn next_boot_cascade(variables: &[VariableInfo], grub: &GrubView) -> String {
    let Some(active) = variables.iter().find(|variable| variable.active) else {
        return "未知（没有选中的系统）".to_string();
    };
    let mut text = active.display_name.clone();
    if active.system != "linux" {
        return text;
    }
    let selector = grub
        .next_entry
        .as_deref()
        .or(grub.saved_entry.as_deref())
        .unwrap_or("");
    let Some(entry) = grub.entries.iter().find(|entry| entry.selector == selector) else {
        return text;
    };
    if let Some(submenu) = &entry.submenu {
        let group = grub
            .groups
            .iter()
            .find(|group| &group.id == submenu)
            .map(|group| group.name.as_str())
            .unwrap_or(submenu.as_str());
        text.push_str(&format!(" › {group}"));
    }
    text.push_str(&format!(" › {}", grub_entry_label(entry, &grub.list_format)));
    text
}

/// 托盘顶部那行状态文案：只在**和「下次启动：X」不一样**的时候才显示。
///
/// 扫描成功之后 `status.message` 本身就是「下次启动：X」，和顶部那行一字不差；
/// 老实现照单再摆一行，于是菜单里出现两条「下次启动：…」（用户报的重复）。
/// 只有错误提示、临时操作结果这类「额外信息」才值得单独一行 —— 抽成纯函数方便单测。
pub(crate) fn menu_status_line<'a>(message: &'a str, next_boot_label: &str) -> Option<&'a str> {
    let message = message.trim();
    if message.is_empty() || message == next_boot_label {
        return None;
    }
    Some(message)
}

/// —— 托盘菜单的**数据模型**（三端统一的地基）——
///
/// 菜单内容只在这里生成一次，而且是**纯数据**：不碰 GTK / Win32 / AppKit 任何控件。
/// 三端各自把这份模型渲染出来 —— 因此「同一份菜单」在三端必然一致。
/// 顺带解决一个老隐患：构造原生菜单会真的去创建工具包控件，而重建菜单的调用点
/// 可能在非主线程（HTTP 线程拿着菜单点击 → publish → 重建），GTK 那边并不欢迎。
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(crate) struct MenuNode {
    /// 点击时回传给应用的动作 id；分隔线与表头没有 id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
    /// 纯文本。开关/单选的勾已经画在里面（见 toggle_label / radio_label）
    pub(crate) label: String,
    pub(crate) enabled: bool,
    /// 结构：`item`（可点）/ `header`（分区标题）/ `sep`（分隔线）/ `sub`（子菜单）
    pub(crate) kind: &'static str,
    /// 列表类条目的「当前项」标记（系统列表、GRUB 条目、顶部「下次启动」那行）；
    /// 不是这类条目时是 None（勾在 label 里，不重复表达）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) checked: Option<bool>,
    /// 点完这一项**要不要留着菜单**（用户诉求③：涉及选择框的都不要关）。
    ///
    /// 这条规则**只在这里算一次**，随模型一起发给前端，前端照字段办事、不重算一遍 ——
    /// 否则「哪些算选择框」就有了两份实现，迟早漂移（第一版就是这么写的，已改掉）。
    #[serde(rename = "keepsOpen")]
    pub(crate) keeps_open: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) children: Vec<MenuNode>,
}

/// 动作条目（扫描 / 显示主窗口 / 电源 / 退出…）：点完关菜单。
pub(crate) fn menu_item(id: impl Into<String>, label: impl Into<String>, enabled: bool) -> MenuNode {
    MenuNode {
        id: Some(id.into()),
        label: label.into(),
        enabled,
        kind: "item",
        checked: None,
        keeps_open: false,
        children: Vec::new(),
    }
}

/// **选择条目**：开关（☑/☐）或单选（◉/○）。点完**不关菜单** —— 可以连着勾几个。
///
/// 勾/单选标记必须已经画在 label 里（toggle_label / radio_label），这是本项成立的前提：
/// 标记是文字，与工具包无关，谁渲染都一样。
pub(crate) fn menu_choice_item(id: impl Into<String>, label: impl Into<String>, enabled: bool) -> MenuNode {
    MenuNode {
        keeps_open: true,
        ..menu_item(id, label, enabled)
    }
}

/// 列表类**动作**条目：带「当前项」标记，但点它是动作（顶部「下次启动」→ 打开主窗口）。
pub(crate) fn menu_marked_item(
    id: impl Into<String>,
    label: impl Into<String>,
    enabled: bool,
    marked: bool,
) -> MenuNode {
    MenuNode {
        checked: Some(marked),
        ..menu_item(id, label, enabled)
    }
}

/// 列表类**选择**条目：带「当前项」标记，点它就是把「下次启动」换成它 —— 属单选组，点完留着菜单。
pub(crate) fn menu_marked_choice_item(
    id: impl Into<String>,
    label: impl Into<String>,
    enabled: bool,
    marked: bool,
) -> MenuNode {
    MenuNode {
        keeps_open: true,
        ..menu_marked_item(id, label, enabled, marked)
    }
}

/// 分区标题：禁用项，配合上下分隔线把菜单切成独立的几块（原生菜单唯一的表达能力）。
pub(crate) fn menu_header(id: impl Into<String>, label: impl Into<String>) -> MenuNode {
    MenuNode {
        id: Some(id.into()),
        label: label.into(),
        enabled: false,
        kind: "header",
        checked: None,
        keeps_open: false,
        children: Vec::new(),
    }
}

/// 分隔线。
pub(crate) fn menu_separator() -> MenuNode {
    MenuNode {
        id: None,
        label: String::new(),
        enabled: false,
        kind: "sep",
        checked: None,
        keeps_open: false,
        children: Vec::new(),
    }
}

/// 子菜单。
pub(crate) fn menu_submenu(
    id: impl Into<String>,
    label: impl Into<String>,
    children: Vec<MenuNode>,
) -> MenuNode {
    MenuNode {
        id: Some(id.into()),
        label: label.into(),
        enabled: true,
        kind: "sub",
        checked: None,
        keeps_open: false,
        children,
    }
}

/// GRUB 启动项那一块（进顶层菜单 = 平铺展开；进子菜单 = 收起）。
///
/// 与主窗 GRUB 区**同一套列表格式逻辑**（`ui/grub-list-view.js` 的 renderGrubEntries）：
/// * 当前选中项置顶（原位照常保留 —— 同一条出现两次是主窗定案的行为）；
/// * 组头 = ▸ + 系统图标 + 组名 +（N 项[ · ESP 位置]），位置按格式带不带（极简/原始不带）；
/// * 条目文字随 `grub.listFormat` 变（极简/简易 = 去前缀内核名、中等 = 完整内核名、
///   原始/复杂 = 完整标题），非原始格式按渲染结果折叠同名条目（首个出面、其余并成
///   「· N 个变体」）；点击写 grubenv（`grub-switch:<selector>`），与主窗完全同源。
pub(crate) fn grub_menu_nodes(grub: &GrubView, busy: bool) -> Vec<MenuNode> {
    let mut items: Vec<MenuNode> = Vec::new();
    if grub.entries.is_empty() {
        items.push(menu_item(
            "grub-empty",
            if grub.partuuid.is_empty() && grub.dir.is_empty() {
                "GRUB：还没配置 ESP"
            } else {
                "GRUB：没解析到启动项"
            },
            false,
        ));
        return items;
    }
    let format = normalize_grub_list_format(&grub.list_format);
    let active = grub
        .next_entry
        .as_deref()
        .or(grub.saved_entry.as_deref())
        .unwrap_or("");
    let group_titles: std::collections::HashMap<&str, &str> = grub
        .groups
        .iter()
        .map(|group| (group.id.as_str(), group.name.as_str()))
        .collect();
    let mut children_of: std::collections::HashMap<&str, Vec<&grub::GrubEntry>> =
        std::collections::HashMap::new();
    for entry in &grub.entries {
        if let Some(submenu) = &entry.submenu {
            children_of.entry(submenu.as_str()).or_default().push(entry);
        }
    }
    // 组头附加的 ESP 位置（简易/中等/复杂带，极简/原始不带 —— 与主窗同一规则）
    let location = if format == "minimal" || format == "raw" {
        String::new()
    } else {
        let mut parts: Vec<String> = Vec::new();
        if let Some(dir) = &grub.efi_dir {
            parts.push(format!("位于{dir}"));
        }
        let uuid = (!grub.partuuid.is_empty())
            .then(|| format!("UUID={}", grub.partuuid))
            .or_else(|| {
                grub.esp
                    .as_ref()
                    .and_then(|esp| esp.partuuid.clone().or_else(|| esp.uuid.clone()))
                    .map(|uuid| format!("UUID={uuid}"))
            });
        if let Some(uuid) = uuid {
            parts.push(uuid);
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" · {}", parts.join(" "))
        }
    };

    // 置顶当前选中项（组内子项补「组名 › 」前缀，与主窗置顶行同一规则）
    if let Some(pinned) = grub.entries.iter().find(|entry| entry.selector == active) {
        let prefix = pinned
            .submenu
            .as_deref()
            .and_then(|id| group_titles.get(id))
            .map(|title| format!("{title} › "))
            .unwrap_or_default();
        grub_push_entry(
            &mut items,
            format,
            pinned,
            true,
            &prefix,
            &std::collections::HashMap::new(),
            &mut std::collections::HashSet::new(),
            busy,
            active,
        );
    }

    // 主体：顶层一套折叠状态，各组独立一套（与主窗一致）；原始格式不折叠
    let mut emitted_groups = std::collections::HashSet::new();
    let top_counts = grub_collapse(
        format,
        grub.entries
            .iter()
            .filter(|entry| entry.submenu.is_none()),
    );
    let mut top_emitted = std::collections::HashSet::new();
    for entry in &grub.entries {
        let Some(submenu_id) = &entry.submenu else {
            grub_push_entry(
                &mut items,
                format,
                entry,
                true,
                "",
                &top_counts,
                &mut top_emitted,
                busy,
                active,
            );
            continue;
        };
        if !emitted_groups.insert(submenu_id.as_str()) {
            continue;
        }
        let title = group_titles
            .get(submenu_id.as_str())
            .copied()
            .unwrap_or(submenu_id.as_str());
        let count = children_of
            .get(submenu_id.as_str())
            .map(Vec::len)
            .unwrap_or(0);
        items.push(menu_header(
            format!("grub-group:{submenu_id}"),
            format!(
                "▸ {}{title}（{count} 项{}）",
                grub_os_icon(title),
                location
            ),
        ));
        let counts = grub_collapse(
            format,
            children_of
                .get(submenu_id.as_str())
                .into_iter()
                .flatten()
                .copied(),
        );
        let mut emitted = std::collections::HashSet::new();
        for child in children_of.get(submenu_id.as_str()).into_iter().flatten() {
            grub_push_entry(
                &mut items,
                format,
                child,
                false,
                "",
                &counts,
                &mut emitted,
                busy,
                active,
            );
        }
    }
    items
}

/// 条目/组名的系统图标：windows → 🪟，mac/apple → 🍎，linux → 🐧，都不中就不加
/// （与主窗 grubOsIcon 同一规则，不猜）。
pub(crate) fn grub_os_icon(text: &str) -> &'static str {
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("windows") {
        "🪟 "
    } else if lowered.contains("mac") || lowered.contains("apple") {
        "🍎 "
    } else if lowered.contains("linux") {
        "🐧 "
    } else {
        ""
    }
}

/// 条目的内核名：优先 `linux` 行解析出的内核文件名，取不到退回标题原文。
pub(crate) fn grub_kernel_of(entry: &grub::GrubEntry) -> String {
    entry
        .kernels
        .first()
        .cloned()
        .unwrap_or_else(|| entry.name.clone())
}

/// 去内核名前缀：先去 `vmlinuz-`，再去 `linux-`（与主窗 grubStripKernel 一致）。
pub(crate) fn grub_strip_kernel(name: &str) -> String {
    let stripped = name.strip_prefix("vmlinuz-").unwrap_or(name);
    stripped.strip_prefix("linux-").unwrap_or(stripped).to_string()
}

/// 行文字随 `grub.listFormat` 变（与主窗 rowLabel 一致）。
pub(crate) fn grub_row_label(format: &str, entry: &grub::GrubEntry) -> String {
    match format {
        "minimal" | "simple" => grub_strip_kernel(&grub_kernel_of(entry)),
        "medium" => grub_kernel_of(entry),
        _ => entry.name.clone(),
    }
}

/// 按渲染结果数一遍变体数（非原始格式用；原始格式不折叠，回空表 —— 变体数
/// 已在解析期并进 `entry.variants`）。
pub(crate) fn grub_collapse<'a>(
    format: &str,
    entries: impl Iterator<Item = &'a grub::GrubEntry>,
) -> std::collections::HashMap<String, usize> {
    let mut counts = std::collections::HashMap::new();
    if format == "raw" {
        return counts;
    }
    for entry in entries {
        *counts.entry(grub_row_label(format, entry)).or_insert(0) += 1;
    }
    counts
}

/// 出一行 GRUB 条目：非原始格式先按渲染结果折叠（重复的只出面一次，与主窗同一规则），
/// 行文字 = **缩进箭头 + 系统图标** +（组前缀）+ 格式化文字 + 行尾附注（变体数/内核）。
/// 箭头在图标的**前面** —— 与主窗 `makeRow`（`arrow + icon + text`）逐字对齐，
/// 组头那行也是「▸ 图标 组名」同一顺序。
/// 置顶行带 `group_prefix`（组名 › ）、不做缩进箭头；原位行顶层无箭头、组内 ↳。
pub(crate) fn grub_push_entry(
    items: &mut Vec<MenuNode>,
    format: &str,
    entry: &grub::GrubEntry,
    top: bool,
    group_prefix: &str,
    counts: &std::collections::HashMap<String, usize>,
    emitted: &mut std::collections::HashSet<String>,
    busy: bool,
    active: &str,
) {
    let label = grub_row_label(format, entry);
    let variants = if format == "raw" {
        entry.variants.max(1)
    } else if !emitted.insert(label.clone()) {
        return;
    } else {
        counts.get(&label).copied().unwrap_or(1)
    };
    let mut parts: Vec<String> = Vec::new();
    if variants > 1 {
        parts.push(format!("{variants} 个变体"));
    }
    if format == "raw" || format == "detailed" {
        let tag = if !entry.kernels.is_empty() {
            entry.kernels[0].clone()
        } else if entry.chainloader {
            "转交引导".to_string()
        } else {
            entry.class.clone().unwrap_or_default()
        };
        if !tag.is_empty() {
            parts.push(tag);
        }
    }
    let suffix = if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    };
    let icon = grub_os_icon(&format!("{group_prefix}{}", entry.name));
    let arrow = if top { "" } else { "↳ " };
    items.push(menu_marked_choice_item(
        format!("grub-switch:{}", entry.selector),
        format!("{arrow}{icon}{group_prefix}{label}{suffix}"),
        !busy,
        entry.selector == active,
    ));
}

/// 在模型里按 id 找那一条，回答「点完要不要留着菜单」（找不到 = 动作项，关）。
///
/// 前端也会照模型里的 `keepsOpen` 做即时反馈，但**关不关窗由后端说了算** ——
/// 免得菜单窗在「前端认为该关、后端认为不该关」时僵在那里。
pub(crate) fn menu_node_keeps_open(nodes: &[MenuNode], id: &str) -> bool {
    fn find(nodes: &[MenuNode], id: &str) -> Option<bool> {
        for node in nodes {
            if node.id.as_deref() == Some(id) {
                return Some(node.keeps_open);
            }
            if let Some(found) = find(&node.children, id) {
                return Some(found);
            }
        }
        None
    }
    find(nodes, id).unwrap_or(false)
}

/// 托盘菜单的全部内容。**纯数据**，与任何工具包无关。
pub(crate) fn build_menu_nodes(state: &UiState) -> Vec<MenuNode> {
    let busy = state.busy.affects_menu();
    let message = &state.status.message;
    let settings = &state.settings.settings;
    let variables = menu_system_rows(&state.status.variables);
    let next_boot = variables.iter().find(|variable| variable.active);
    // 扫描成功后 `status.message` 就是这一行的样子（rEFInd 那一层）—— 去重拿它比对，
    // 与顶部那行改成什么排版无关（见 menu_status_line）。
    let next_boot_scan_line = next_boot.map_or_else(
        || "下次启动：未知".to_string(),
        |variable| format!("下次启动：{}", variable.display_name),
    );
    // 顶部那行：**与主窗口那行大字同一份文字**（级联到 GRUB 条目），并按用户定案
    // 排成两行「下次启动 / Linux › OpenCore (macOS)」。级联规则见 next_boot_cascade。
    let next_boot_label = format!(
        "下次启动\n{}",
        next_boot_cascade(&state.status.variables, &state.grub)
    );

    let mut nodes: Vec<MenuNode> = Vec::new();
    // 第一行就把「下次启动哪个系统」摆出来。原生菜单改不了字号/字重，所以靠**位置**
    // 和标记让它够醒目；点它 = 打开主窗口去改。
    nodes.push(menu_marked_item(
        "next-boot",
        next_boot_label,
        true,
        next_boot.is_some(),
    ));
    // 状态行只在**跟上面那行不一样**的时候才出现（见 menu_status_line）：
    // 扫描成功后 message 本身就是「下次启动：X」，再摆一行就是「下次启动重复了」，
    // 只有错误提示/临时操作结果才需要单独一行。
    if let Some(line) = menu_status_line(message, &next_boot_scan_line) {
        nodes.push(menu_item("status", line, false));
    }
    nodes.push(menu_separator());

    // rEFInd 扫到了系统 = 「展开」平铺（现状不变）；没扫到 = 收进子菜单不展开，
    // 同时 GRUB 区平铺展开顶上 —— 保证总有一块能直接点到。反过来 rEFInd 在时
    // GRUB 收进子菜单（默认展开 rEFInd，用户定案）。
    let refind_alive = variables.iter().any(|variable| variable.exists);
    // —— 分区：系统列表（选中项带标记）——
    if refind_alive {
        for variable in &variables {
            nodes.push(menu_marked_choice_item(
                variable.system.clone(),
                variable.display_name.clone(),
                variable.exists && !busy,
                variable.active,
            ));
        }
    } else {
        let children = variables
            .iter()
            .map(|variable| {
                menu_marked_choice_item(
                    variable.system.clone(),
                    variable.display_name.clone(),
                    variable.exists && !busy,
                    variable.active,
                )
            })
            .collect();
        nodes.push(menu_submenu("refind-systems", "rEFInd 系统列表", children));
    }
    nodes.push(menu_separator());
    // —— 分区：GRUB 启动项（与 rEFInd 系统列表并列的第二块）——
    if refind_alive {
        nodes.push(menu_submenu(
            "grub-entries",
            "GRUB 启动项",
            grub_menu_nodes(&state.grub, busy),
        ));
    } else {
        nodes.extend(grub_menu_nodes(&state.grub, busy));
    }
    nodes.push(menu_separator());
    if state.status.temporary_mount {
        nodes.push(menu_item("unmount-temporary", "卸载临时挂载", !busy));
    }
    nodes.push(menu_item("show", "显示主窗口", true));
    // 远程管理（server-hub）独立窗口的入口：托盘是唯一新增入口，主窗口零改动。
    nodes.push(menu_item("open-hub", "远程管理…", true));

    // —— 分区：应用设置 ——
    nodes.push(menu_separator());
    nodes.push(menu_header("settings-label", "应用设置"));
    nodes.push(menu_choice_item(
        "setting-autostart",
        toggle_label(settings.autostart, "开机启动"),
        true,
    ));
    // 开关项的 id 带「点下去要变成的状态」。原生勾选控件会先把勾翻转再通知我们，
    // 所以分发时必须知道用户想要哪个值；现在是纯文本标记（见 toggle_label），
    // 这个后缀同时也让「重复点同一项」是幂等的（不会来回翻）。
    nodes.push(menu_choice_item(
        toggle_menu_id("setting-start-in-tray", !settings.start_in_tray),
        toggle_label(settings.start_in_tray, "启动时隐藏主界面"),
        true,
    ));
    nodes.push(menu_choice_item(
        toggle_menu_id(
            "setting-hide-from-dock-taskbar",
            !settings.hide_from_dock_taskbar,
        ),
        toggle_label(settings.hide_from_dock_taskbar, "隐藏程序（任务栏/Dock）"),
        true,
    ));
    // 贴边自动隐藏是高频功能开关，托盘给个快捷入口（与 HTTP 服务/远程请求通知同级）。
    // 「屏幕中间」模式下两者互斥，这里整体置灰（勾选态保持设置原值，
    // 切回其他落位方式即可用）。
    nodes.push(menu_choice_item(
        toggle_menu_id("setting-main-auto-hide", !settings.main_auto_hide),
        toggle_label(settings.main_auto_hide, "贴边自动隐藏"),
        normalize_window_position(&settings.main_window_position) != "center",
    ));
    // 窗口大小在设置页能随意调（滑条 / 拖边框），托盘里放一个「重置」入口：
    // 窗口被拖到怪尺寸（或锁定态想恢复标称）时，不用打开主窗口去找
    nodes.push(menu_item("reset-window-size", "重置窗口大小", !busy));
    // 与设置页「重置位置」同源：窗口被拖丢/飞出屏幕时一键回正中
    nodes.push(menu_item("reset-window-position", "重置窗口位置", !busy));
    // 固定显示位置（与设置页「固定显示位置」下拉同源）：三种模式各一个单选项
    let position_now = normalize_window_position(&settings.main_window_position);
    nodes.push(menu_submenu(
        "setting-window-position",
        "固定显示位置",
        [("free", "自由摆放"), ("center", "屏幕中间"), ("last", "上次位置")]
            .into_iter()
            .map(|(mode, label)| {
                menu_choice_item(
                    format!("setting-main-window-position:{mode}"),
                    radio_label(position_now == mode, label),
                    true,
                )
            })
            .collect(),
    ));
    // 固定显示器（与设置页「固定显示器」下拉同源）：auto + 当前所有显示器。
    // 列表来自快照里的 mainMonitorOptions（与主菜单同源），显示器热插拔后
    // 下一次快照刷新菜单就会跟着变。
    let fixed_now = normalize_fixed_monitor(&settings.main_fixed_monitor);
    let mut monitors = vec![menu_choice_item(
        "setting-main-fixed-monitor:auto",
        radio_label(fixed_now == "auto", "自动（不固定）"),
        true,
    )];
    monitors.extend(state.settings.main_monitor_options.iter().map(|name| {
        menu_choice_item(
            format!("setting-main-fixed-monitor:{name}"),
            radio_label(fixed_now == *name, name),
            true,
        )
    }));
    nodes.push(menu_submenu(
        "setting-main-fixed-monitor",
        "固定显示器",
        monitors,
    ));
    nodes.push(menu_submenu(
        "setting-close-action",
        "点击窗口 ❌ 时",
        [(CloseAction::Tray, "最小化到托盘"), (CloseAction::Quit, "退出程序")]
            .into_iter()
            .map(|(action, label)| {
                menu_choice_item(
                    close_action_menu_id(action),
                    radio_label(settings.close_action == action, label),
                    true,
                )
            })
            .collect(),
    ));

    // —— 分区：扫描 ——
    nodes.push(menu_separator());
    nodes.push(menu_item("refresh", "扫描", !busy));

    // —— 分区：电源操作 ——
    // 铺平在主菜单里（不折叠成子菜单：关机/重启是高频动作，多一层展开纯属添堵），
    // 但上下各有一条分隔线 + 一个标题，这样它是一块独立分区，而不是粘在「扫描」下面。
    nodes.push(menu_separator());
    nodes.push(menu_header("power-label", "电源操作"));
    for action in [
        PowerAction::Reboot,
        PowerAction::Shutdown,
        PowerAction::Sleep,
    ] {
        nodes.push(menu_item(power_menu_id(action), action.label(), !busy));
    }

    // —— 分区：远程控制 ——
    // 同样铺平在主菜单里；监听状态是这一区的**第一行**，不展开任何子菜单就能看到在不在听。
    nodes.push(menu_separator());
    nodes.push(menu_header("remote-label", "远程控制"));
    nodes.push(menu_item(
        "remote-status",
        format!("监听状态：{}", menu_status_text(&state.http.state_text, 48)),
        false,
    ));
    nodes.push(menu_choice_item(
        toggle_menu_id("setting-http-enabled", !settings.http_enabled),
        toggle_label(settings.http_enabled, "HTTP 服务"),
        true,
    ));
    nodes.push(menu_choice_item(
        toggle_menu_id("setting-notify-enabled", !settings.notify_enabled),
        toggle_label(settings.notify_enabled, "远程请求通知"),
        true,
    ));
    let delay_now = settings.channels.http.delay_seconds;
    let delay_same = delay_now == settings.channels.cli.delay_seconds;
    nodes.push(menu_submenu(
        "setting-delay",
        delay_submenu_title(settings),
        [0u32, 3, 5, 10, 30]
            .into_iter()
            .map(|seconds| {
                let label = if seconds == 0 {
                    "立即执行（0 秒）".to_string()
                } else {
                    format!("{seconds} 秒")
                };
                menu_choice_item(
                    format!("setting-delay:{seconds}"),
                    radio_label(delay_same && delay_now == seconds, &label),
                    true,
                )
            })
            .collect(),
    ));
    nodes.push(menu_item("show-remote", "远程控制设置…", true));
    // 用系统浏览器打开 Web 控制页（带 token，点开即用）。
    // 没在监听时置灰：没有页面可开，免得点了弹一个必然失败的错误。
    nodes.push(menu_item("open-web", "打开 Web 页面", state.http.listening));

    nodes.push(menu_separator());
    nodes.push(menu_item("quit", "退出程序", true));
    nodes
}

/// 菜单里的「小节标题」：灰色禁用项，纯视觉分区用。
/// 只被手写 build_tray_menu（Win/mac）使用；单测与 Linux 编译下 allow。
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn menu_section_header(
    app: &AppHandle,
    id: &str,
    label: &str,
) -> tauri::Result<MenuItem<tauri::Wry>> {
    MenuItem::with_id(app, id, label, false, None::<&str>)
}

/// 托盘菜单内容摘要：同样内容 → 同样签名。
///
/// 用它做「内容没变就不重建菜单」的指纹（原生菜单重建 + 重挂会闪断）；
/// unified 方式下同一个指纹用来决定要不要给 dbusmenu 发 LayoutUpdated。
pub(crate) fn menu_signature(
    message: &str,
    temporary_mount: bool,
    settings: &Settings,
    variables: &[VariableInfo],
    error: Option<&str>,
    busy: &BusyState,
    http_state: &str,
) -> String {
    // 系统行上每个可见字段都进指纹：系统名 / 显示名 / 选中 / 存在。
    // 少一个就会出现「数据变了但菜单没重建」的假状态。
    let variables = variables
        .iter()
        .map(|variable| {
            format!(
                "{}:{}:{}{}",
                variable.system,
                variable.display_name,
                if variable.active { "+" } else { "-" },
                if variable.exists { "o" } else { "x" }
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "msg={message}|tmp={}|autostart={}|tray={}|dock={}|close={:?}|http={}|notify={}|autohide={}|delay={},{}|vars={variables}|err={}|busy={}|httpState={http_state}",
        temporary_mount as u8,
        settings.autostart as u8,
        settings.start_in_tray as u8,
        settings.hide_from_dock_taskbar as u8,
        settings.close_action,
        // 远程控制子菜单里的勾选与「监听状态」行都从设置/运行态来，
        // 不进指纹的话主窗口一改，托盘那边就停在旧样子
        settings.http_enabled as u8,
        settings.notify_enabled as u8,
        // 贴边自动隐藏的托盘勾选态也要进指纹
        settings.main_auto_hide as u8,
        settings.channels.http.delay_seconds,
        settings.channels.cli.delay_seconds,
        error.unwrap_or(""),
        // 动作进行中菜单项会变灰，所以 busy 必须进指纹，否则置灰不会生效
        busy.menu_describe()
    )
}

pub(crate) fn menu_signature_for(state: &UiState) -> String {
    let base = menu_signature(
        &state.status.message,
        state.status.temporary_mount,
        &state.settings.settings,
        &menu_system_rows(&state.status.variables),
        state.status.error.as_deref(),
        &state.busy,
        &state.http.state_text,
    );
    // 固定显示位置的模式进指纹（托盘子菜单三个勾选项跟着变）
    let position = normalize_window_position(&state.settings.settings.main_window_position);
    // 固定显示器及其选项列表也进指纹：勾选态跟着变，显示器热插拔后
    // 选项集变了也要重建菜单
    let fixed_monitor = normalize_fixed_monitor(&state.settings.settings.main_fixed_monitor);
    let monitor_options = state.settings.main_monitor_options.join(",");
    // GRUB 区进指纹：**直接用托盘菜单真正会显示的那份 GRUB 节点**（grub_menu_nodes，
    // 与主窗 GRUB 区、统一方式同一份渲染逻辑）—— 列表格式、当前项置顶、组头
    // （N 项 · ESP 位置）、变体折叠、哪一条是当前项，全在里面。
    // 以前是自己再数一遍「条目名 + 分组名 + 列表格式」，等于把同一份内容算了两遍：
    // 改渲染规则时极易漏掉指纹，症状就是「显示的名字变了但托盘没重建」的假状态。
    let grub_menu = serde_json::to_string(&grub_menu_nodes(
        &state.grub,
        state.busy.affects_menu(),
    ))
    .expect("GRUB 菜单节点一定能序列化");
    let next_boot = next_boot_cascade(&state.status.variables, &state.grub);
    format!(
        "{base}|wpos={position}|fmon={fixed_monitor}|fmons={monitor_options}|grubmenu={grub_menu}|nb={next_boot}"
    )
}

/// 记住当前菜单内容，让后续相同内容的刷新直接跳过重建。
/// Linux 已不建 tauri 托盘（classic 也走 linux_sni），预热只剩 Win/mac 用。
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn remember_menu_signature(app: &AppHandle, signature: String) {
    *app.state::<Mutex<Option<String>>>()
        .lock()
        .expect("menu signature mutex poisoned") = Some(signature);
}

/// 作废指纹记忆：下一次 publish 一定重建菜单（哪怕内容算出来没变）。
///
/// 托盘菜单的实时性全靠「重建 + 重挂」：勾选标记、置灰、状态行都是重建那一刻才刷新的。
/// 而「用户点了菜单」这件事本身就是一个「显示可能与真实状态不符」的信号 ——
/// 比如单选项点的是已选中的那个（设置没变、指纹没变），但宿主那边的勾选态很可能
/// 已经被工具包翻过了。所以每次菜单点击都廉价地作废一次指纹，让界面马上回到真实状态。
pub(crate) fn invalidate_menu_signature(app: &AppHandle) {
    *app.state::<Mutex<Option<String>>>()
        .lock()
        .expect("menu signature mutex poisoned") = None;
}

/// 指纹比对 + 记忆。返回「这次和上次比内容变了吗」。
///
/// 统一给两种形态用：classic 变了重建原生菜单，unified 变了给 dbusmenu 发更新信号。
pub(crate) fn menu_signature_changed(app: &AppHandle, signature: &str) -> bool {
    let cache = app.state::<Mutex<Option<String>>>();
    let mut cache = cache.lock().expect("menu signature mutex poisoned");
    if cache.as_deref() == Some(signature) {
        return false;
    }
    *cache = Some(signature.to_string());
    true
}

/// 托盘菜单项的语义。把「字符串前后缀匹配」换成可单测的纯解析（阶段 1 修 D6）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MenuAction {
    Show,
    Refresh,
    UnmountTemporary,
    Switch(String),
    /// 电源动作：重启 / 关机 / 睡眠
    Power(PowerAction),
    /// HTTP 服务开关
    SetHttpEnabled(bool),
    /// 远程请求通知总开关
    SetNotifyEnabled(bool),
    /// 贴边自动隐藏总开关（关掉时立即展开）
    SetMainAutoHide(bool),
    /// 远程执行延迟（同时作用于两条渠道）
    SetDelayAll(u32),
    /// 打开主窗口并定位到「远程控制」区
    ShowRemote,
    /// 用系统默认浏览器打开 Web 控制页（带 token）
    OpenWeb,
    /// 打开/唤起远程管理（server-hub）独立窗口
    OpenHub,
    SetAutostart,
    SetStartInTray(bool),
    SetHideFromDockTaskbar(bool),
    SetCloseAction(CloseAction),
    ResetWindowSize,
    /// 主窗口摆回屏幕正中（与设置页「重置位置」同源）
    ResetWindowPosition,
    /// 固定显示位置模式（free / center / last，与设置页下拉同源）
    SetMainWindowPosition(String),
    /// 固定显示器（auto / 显示器名，与设置页下拉同源）
    SetMainFixedMonitor(String),
    /// 切换 GRUB 启动项（写 grubenv 的 selector，与主窗 GRUB 区同源）
    SwitchGrub(String),
    Quit,
}

impl MenuAction {
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        if let Some(rest) = id.strip_prefix("setting-close-action:") {
            return CloseAction::parse(rest.trim()).map(Self::SetCloseAction);
        }
        if let Some(rest) = id.strip_prefix("setting-start-in-tray:") {
            return parse_menu_bool(rest).map(Self::SetStartInTray);
        }
        if let Some(rest) = id.strip_prefix("setting-hide-from-dock-taskbar:") {
            return parse_menu_bool(rest).map(Self::SetHideFromDockTaskbar);
        }
        if let Some(rest) = id.strip_prefix("power:") {
            return PowerAction::parse(rest).map(Self::Power);
        }
        if let Some(rest) = id.strip_prefix("setting-http-enabled:") {
            return parse_menu_bool(rest).map(Self::SetHttpEnabled);
        }
        if let Some(rest) = id.strip_prefix("setting-notify-enabled:") {
            return parse_menu_bool(rest).map(Self::SetNotifyEnabled);
        }
        if let Some(rest) = id.strip_prefix("setting-main-auto-hide:") {
            return parse_menu_bool(rest).map(Self::SetMainAutoHide);
        }
        if let Some(rest) = id.strip_prefix("setting-delay:") {
            return rest.trim().parse::<u32>().ok().map(Self::SetDelayAll);
        }
        if let Some(rest) = id.strip_prefix("setting-main-window-position:") {
            let mode = rest.trim().to_string();
            // 归一兜底：脏值会被 normalize 打回 free，不会落进设置
            return Some(Self::SetMainWindowPosition(mode));
        }
        if let Some(rest) = id.strip_prefix("setting-main-fixed-monitor:") {
            return Some(Self::SetMainFixedMonitor(rest.to_string()));
        }
        if let Some(rest) = id.strip_prefix("grub-switch:") {
            return Some(Self::SwitchGrub(rest.to_string()));
        }
        match id {
            "show" => Some(Self::Show),
            // 顶部那行「下次启动：X」：点它就是把主窗口叫出来改
            "next-boot" => Some(Self::Show),
            "refresh" => Some(Self::Refresh),
            "unmount-temporary" => Some(Self::UnmountTemporary),
            // 旧版 id：只有一个「重启系统」项时的写法，保留兼容
            "restart" => Some(Self::Power(PowerAction::Reboot)),
            "setting-autostart" => Some(Self::SetAutostart),
            "show-remote" => Some(Self::ShowRemote),
            "open-web" => Some(Self::OpenWeb),
            "open-hub" => Some(Self::OpenHub),
            "reset-window-size" => Some(Self::ResetWindowSize),
            "reset-window-position" => Some(Self::ResetWindowPosition),
            "quit" => Some(Self::Quit),
            "linux" | "windows" | "macos" => Some(Self::Switch(id.to_string())),
            _ => None,
        }
    }
}

/// 托盘菜单事件入口：原始 id → 解析 → 分发，每一步都写 trace（现场取证）。
///
/// 另外**无条件**往 startup-debug.log 记一行「收到某个菜单 id」：trace 默认是关的，
/// 而「点了菜单没反应」这类报障只有两个可能 —— 事件根本没到应用，或到了但被
/// 某个闸吞了。这一行让两种情形在用户机器上一眼可辨（托盘点击是低频事件，
/// 一行日志的代价可以忽略）。
pub(crate) fn handle_menu_event(app: &AppHandle, id: &str) {
    startup_debug(app, &format!("菜单点击 id={id}"));
    trace(app, "menu-action", &format!("id={id}"));
    let Some(action) = MenuAction::from_id(id) else {
        trace(app, "menu-action", &format!("id={id} 未识别，已忽略"));
        return;
    };
    trace(app, "menu-action", &format!("id={id} → {action:?}"));
    // 用户交互过菜单：先作废指纹（见 invalidate_menu_signature），
    // 让这次动作引发的 publish 一定刷新菜单（classic 重建原生菜单 / unified 发
    // LayoutUpdated），把勾选标记刷成真实状态。
    invalidate_menu_signature(app);
    // 窗口操作留在事件线程上执行；其余动作放工作线程（可能耗时或要授权）。
    if action == MenuAction::Show {
        show_main_window(app, ShowOrigin::Recall);
        return;
    }
    spawn_menu_action(app, action);
}

pub(crate) fn spawn_menu_action(app: &AppHandle, action: MenuAction) {
    let app = app.clone();
    std::thread::spawn(move || match action {
        MenuAction::Show => show_main_window(&app, ShowOrigin::Recall),
        MenuAction::Refresh => {
            refresh_and_update(&app);
        }
        MenuAction::UnmountTemporary => {
            unmount_temporary(app);
        }
        MenuAction::Switch(system) => {
            switch_system_locally(&app, &system);
        }
        MenuAction::Power(action) => spawn_power(&app, action, true),
        MenuAction::SetHttpEnabled(enabled) => set_http_enabled_setting(&app, enabled),
        MenuAction::SetNotifyEnabled(enabled) => set_notify_enabled_setting(&app, enabled),
        MenuAction::SetMainAutoHide(enabled) => match set_main_auto_hide(app.clone(), enabled) {
            Ok(_) => record_action(
                &app,
                "贴边自动隐藏",
                true,
                if enabled {
                    "已开启：鼠标离窗后贴边缩成白条"
                } else {
                    "已关闭：窗口立即展开"
                },
            ),
            Err(error) => record_action(&app, "贴边自动隐藏", false, &error),
        },
        MenuAction::SetDelayAll(seconds) => set_delay_all_setting(&app, seconds),
        MenuAction::ShowRemote => {
            show_main_window(&app, ShowOrigin::Recall);
            if let Err(error) = app.emit("focus-remote", ()) {
                trace(
                    &app,
                    "tray",
                    &format!("通知主窗口定位远程控制失败: {error}"),
                );
            }
        }
        MenuAction::OpenWeb => match open_web_page_now(&app) {
            Ok(url) => record_action(&app, "打开 Web 页面", true, &format!("已打开 {url}")),
            Err(error) => record_action(&app, "打开 Web 页面", false, &error),
        },
        MenuAction::OpenHub => match open_hub_window(&app) {
            Ok(_) => record_action(&app, "打开远程管理", true, "远程管理窗口已打开"),
            Err(error) => record_action(&app, "打开远程管理", false, &error),
        },
        MenuAction::SetAutostart => toggle_autostart_setting(&app),
        MenuAction::SetStartInTray(enabled) => {
            update_tray_setting(&app, TraySetting::StartInTray, enabled)
        }
        MenuAction::SetHideFromDockTaskbar(enabled) => {
            update_tray_setting(&app, TraySetting::HideFromDockTaskbar, enabled)
        }
        MenuAction::SetCloseAction(close_action) => update_close_action_setting(&app, close_action),
        MenuAction::ResetWindowSize => match reset_window_size_inner(&app) {
            Ok(_) => record_action(&app, "重置窗口大小", true, "窗口大小已重置为标称尺寸"),
            Err(error) => record_action(&app, "重置窗口大小", false, &error),
        },
        MenuAction::ResetWindowPosition => {
            // 复用设置页「重置位置」的同一实现：摆回屏幕正中（last 模式顺带记为新落点）
            match reset_main_window_position(app.clone()) {
                Ok(_) => record_action(&app, "重置窗口位置", true, "窗口已摆回屏幕正中"),
                Err(error) => record_action(&app, "重置窗口位置", false, &error),
            }
        }
        MenuAction::SetMainWindowPosition(mode) => {
            // 复用设置页同一条链路：落盘 + 立即按新模式落位
            match set_main_window_position(app.clone(), mode) {
                Ok(_) => record_action(&app, "固定显示位置", true, "显示位置已更新"),
                Err(error) => record_action(&app, "固定显示位置", false, &error),
            }
        }
        MenuAction::SetMainFixedMonitor(name) => {
            // 复用设置页同一条链路：落盘 + 立即把主窗约束到所选显示器
            match set_main_fixed_monitor(app.clone(), name.clone()) {
                Ok(_) => record_action(&app, "固定显示器", true, &format!("固定显示器已设为 {name}")),
                Err(error) => record_action(&app, "固定显示器", false, &error),
            }
        }
        MenuAction::SwitchGrub(selector) => {
            // 与主窗 GRUB 区同一条链路：写 grubenv → 重扫 → 广播
            switch_grub_and_update(&app, &selector);
        }
        MenuAction::Quit => app.exit(0),
    });
}
