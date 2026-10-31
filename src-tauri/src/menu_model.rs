//! MenuNode 模型、菜单构建（grub_menu_nodes 等）与菜单指纹。
//! linux_sni.rs 也消费这里的构建函数。

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

/// 托盘顶部那行状态文案：只在**和「下次启动：X」不一样**的时候才显示。
///
/// 扫描成功之后 `status.message` 本身就是「下次启动：X」，和顶部那行一字不差；
/// 老实现照单再摆一行，于是菜单里出现两条「下次启动：…」（用户报的重复）。
/// 只有错误提示、临时操作结果这类「额外信息」才值得单独一行 —— 抽成纯函数方便单测。
pub(crate) fn menu_status_line<'a>(message: &'a str, next_boot_label: &str) -> Option<&'a str> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    // 顶部那行现在可能是**多行**（每个引导器一行，见 next_boot_rows），跟扫描成功
    // 那句「下次启动：X」永远不可能整串相等了。扫描成功的文案恒以「下次启动」开头
    // （`scan` / `grub_esp` 两处都是），按前缀吞掉即可 —— 否则菜单里会同时出现
    // 「下次启动(rEFInd…)」和一条一模一样的「下次启动：X」（用户报过的那条重复）。
    if message.starts_with("下次启动") || message == next_boot_label {
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

// ───────────────────── 引导器下拉框 / 下次启动多行（2026-10-30 定案）─────────────────────
//
// 托盘原来只有两块：「rEFInd 系统列表」（Windows / Linux / macOS 三个固定项）和
// 「GRUB 启动项」。那三个固定项是模板快照（`PreviousBoot-<system>`），**不是**
// rEFInd 菜单里的真实条目 —— 主窗早就按 rEFInd 自己的菜单显示了，托盘却还是老三样，
// 于是用户要求：每个引导器一块下拉框（跟 GRUB 一样），rEFInd 那三个固定项删掉，
// 并且托盘的「下次启动」要与主窗那几行同源。
//
// 现在引导区 = 若干子菜单，每个引导器一块，内容与主窗「系统引导」页的卡片一一对应
// （同一份快照字段、同一套选中项判定）。没内容的引导器整块不出现 —— 菜单里不摆空壳。

/// 通道条目的「当前项」判定：与主窗 `boot-list-view.js` 的 `isCurrentDefault`
/// 同一条规则（`currentDefault` 有时是条目 id、有时是标题，只比其中一个会漏）。
pub(crate) fn channel_entry_is_default(current: &str, entry: &channels::ChannelEntry) -> bool {
    if current.is_empty() {
        return false;
    }
    current == entry.id
        || current == entry.title
        // 空标题不能让 contains 恒真（JS 那边用 \u0000 兜的同一个坑）
        || (!entry.title.is_empty() && current.contains(entry.title.as_str()))
}

/// 通道条目的切换值域：与 `ui/boot-list-view.js` 的 `bootChannelSpec` **同一条规则**
/// （`sdboot:<id>` / opencore 的 id 本身就是值域 / clover 目前只有一个一次性动作）。
pub(crate) fn channel_entry_spec(channel_id: &str, entry: &channels::ChannelEntry) -> String {
    match channel_id {
        "sdboot" => format!("sdboot:{}", entry.id),
        "clover" => "clover-oneshot".to_string(),
        _ => entry.id.clone(),
    }
}

/// 通道里「真正能点的行」：`entries` + OpenCore 的菜单预演合并去重。
///
/// 为什么必须合并：OpenCore 的顶层 `entries` 只放「写入固件启动项」这类修复动作，
/// 真正的菜单项（macOS / Windows / 各卷兜底项）**全在 `menu_preview` 里**，能不能
/// 设默认由 `spec` 有没有值决定。只读 `entries` 的话托盘那块永远退化成一句
/// 「没有可切换的菜单项」，与主窗卡片上明明列着的 5 个卷对不上
/// （用户 2026-10-30 报的「托盘为什么没有 OpenCore 列表」就是这条）。
///
/// 托盘下拉、下次启动多行、远程电脑表三处共用这一份 —— 别再各写一遍判定。
pub(crate) struct ChannelRow {
    pub(crate) title: String,
    /// 原样回传给 `switch_boot_channel` 的值域
    pub(crate) spec: String,
    /// 条目类型（通道用 `icon`、预演用 `kind`），只为挑图标/配色
    pub(crate) class: String,
    pub(crate) is_default: bool,
}

pub(crate) fn channel_switch_rows(channel: &channels::ChannelView) -> Vec<ChannelRow> {
    let current = channel.current_default.as_deref().unwrap_or("").trim();
    // 与 `channel_entry_is_default` 同一条规则，但这里只有 title/spec 两个字符串
    let hit = |spec: &str, title: &str| {
        !current.is_empty()
            && (current == spec || current == title || (!title.is_empty() && current.contains(title)))
    };
    let mut rows: Vec<ChannelRow> = Vec::new();
    for entry in &channel.entries {
        let title = entry.title.trim();
        if title.is_empty() {
            continue;
        }
        let spec = channel_entry_spec(channel.id, entry);
        rows.push(ChannelRow {
            title: title.to_string(),
            is_default: hit(&spec, title),
            spec,
            class: entry.icon.clone(),
        });
    }
    if let Some(preview) = &channel.menu_preview {
        for entry in &preview.entries {
            let title = entry.title.trim();
            let Some(spec) = entry
                .spec
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
            else {
                // 协议驱动项 / 工具项 / 已经是默认项：spec 为空，点了也没有值域可发
                continue;
            };
            if title.is_empty() || rows.iter().any(|row| row.spec == spec) {
                continue;
            }
            rows.push(ChannelRow {
                title: title.to_string(),
                spec: spec.to_string(),
                class: entry.kind.clone(),
                // 预演的 `default` 就是 OC 启动顺序表（OCBtOrder）指的那一项
                is_default: entry.default || hit(spec, title),
            });
        }
    }
    rows
}

/// 通道的「当前默认项」标题 —— **故意与「可切换值域」分开算**。
///
/// `channel_switch_rows` 只收有 `spec` 的条目（点了能发值域的），但默认项不一定
/// 在那一集合里：OpenCore 的默认项来自固件启动顺序表（`OCBtOrder` /
/// `defaultSlot`），而 macOS 那条的引导文件在 APFS Preboot 卷里，没有
/// `opencore-default:<卷>\x01<路径>` 这个值域可用 —— `spec` 是 `None`，于是它既
/// 进不了切换下拉，也判不出默认，主窗/托盘「下次启动」就显示「未选中条目」，
/// 可下面列表里它明明是绿框选中态。这里按优先级补一份：先看 `entries`
/// （sdboot / clover 的常规情形），再看预演里后端打的 `default` 标记。
pub(crate) fn channel_default_title(channel: &channels::ChannelView) -> Option<String> {
    let current = channel.current_default.as_deref().unwrap_or("").trim();
    let from_entries = channel
        .entries
        .iter()
        .find(|entry| channel_entry_is_default(current, entry))
        .map(|entry| entry.title.trim().to_string())
        .filter(|title| !title.is_empty());
    if from_entries.is_some() {
        return from_entries;
    }
    channel
        .menu_preview
        .as_ref()
        .and_then(|preview| {
            preview
                .entries
                .iter()
                .find(|entry| entry.default)
                .map(|entry| entry.title.trim().to_string())
        })
        .filter(|title| !title.is_empty())
}

/// 「下次启动」的多行：每个引导器一行，右边是它当前选中的条目。
///
/// 与主窗那几行（`ui/boot-adv.js` 的 `paintNextBootRows`）**同一条规则**：
/// * 只列「检测到 + 有条目」的引导器（探测到却一条都没读到的不占行）；
/// * 选中项各按各的判据 —— rEFInd 看 `vars/PreviousBoot` 命中的标题，
///   GRUB 看 grubenv 的 next/saved entry，通道看 `current_default`。
pub(crate) fn next_boot_rows(state: &UiState) -> Vec<(String, String)> {
    let grub = effective_grub(state);
    next_boot_rows_from(refind_source(state), &grub, &state.channels)
}

/// rEFInd 的数据源（按设置里的读取方式选）。
pub(crate) fn refind_source(state: &UiState) -> RefindSource<'_> {
    RefindSource {
        mode: state.settings.settings.refind_mode,
        sim: state.refind_sim.as_ref(),
        variables: &state.status.variables,
    }
}

/// GRUB 的数据源：自动 = 只认 Linux 分区那份（`linux_grub`），手动 = 只认 ESP 那份。
///
/// **严格按模式、零回落**（用户 2026-10-31 定案）：自动模式没扫到就是空列表 /
/// 无选中项，绝不拿 ESP 那份垫底 —— 托盘、主窗「下次启动」、主题页计数、
/// 电脑列表 `/api/grub` 四处同口径，模式切了列表必须跟着换。
///
/// 只覆盖若干字段而不是整份重建 —— `GrubView` 有三十多个字段（挂载状态、脚本日志、
/// 安装状态…），托盘与「下次启动」只用得到 entries / groups / next·saved。
pub(crate) fn effective_grub(state: &UiState) -> GrubView {
    let mut view = state.grub.clone();
    overlay_linux_grub(
        &mut view,
        state.linux_grub.as_ref(),
        state.settings.settings.grub_mode.is_manual(),
    );
    view
}

/// `effective_grub` 的可测内核：自动模式把 `linux_grub` 的 entries / groups /
/// next·saved / error **整块换上**（空也换，None 也是空）—— 没有任何回落。
///
/// 除了托盘 /「下次启动」，电脑列表的 `/api/grub` 也吃这一份 —— 手机与 PC hub
/// 看到的 GRUB 列表必须和本机主窗同一个来源，否则「自动模式」下两边各说各话。
pub(crate) fn overlay_linux_grub(
    view: &mut GrubView,
    linux: Option<&channels::linux_grub::LinuxGrubView>,
    manual: bool,
) {
    if manual {
        return;
    }
    match linux {
        Some(l) => {
            view.entries = l.entries.clone();
            view.groups = l.groups.clone();
            view.next_entry = l.next_entry.clone();
            view.saved_entry = l.saved_entry.clone();
            view.error = l.error.clone();
            // 挂载点跟着数据源走：自动那份定位到了哪个分区就报哪个；
            // 没定位到保持原值（这是挂载信息，不是列表内容）
            if let Some(target) = l.target.as_ref() {
                if !target.dev.is_empty() {
                    view.efi_dir = Some(target.dev.clone());
                }
            }
        }
        // 还没读过（None）：列表与选中项就是空 —— 不回落 ESP
        None => {
            view.entries = Vec::new();
            view.groups = Vec::new();
            view.next_entry = None;
            view.saved_entry = None;
            view.error = None;
        }
    }
}

/// `next_boot_rows` 的可测内核：只认三份数据，不碰 AppHandle/设置。
///
/// 拆两层不是为了好看：托盘顶部那行是三端共用的一句话，构造一个完整 `UiState`
/// 需要 Layout/HTTP/助手状态一大堆无关字段，测试里根本写不出来 —— 而「哪一行该
/// 出现、选中项算不算命中」正是最容易写错的地方。
///
/// rEFInd 与 GRUB 传进来的**必须是已经按读取方式选好的那份**（见
/// `refind_source` / `effective_grub`）—— 两个模式各自有各的「选中项」，混着比
/// 就会像之前那样「主窗列表选了 A、下次启动说未选中」。
pub(crate) fn next_boot_rows_from(
    refind: RefindSource<'_>,
    grub: &GrubView,
    channels: &[channels::ChannelView],
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    // rEFInd：自动看 `PreviousBoot` 命中的那条，手动看快照里标「已选择」的那条
    let refind_rows = refind.rows();
    if !refind_rows.is_empty() {
        let picked = refind_rows
            .iter()
            .find(|entry| entry.is_default)
            .map(|entry| entry.label.clone());
        rows.push(("rEFInd".to_string(), picked.unwrap_or_default()));
    }
    // GRUB：选中项 = next_entry 优先、退 saved_entry，在**当前这份**的条目里找
    if !grub.entries.is_empty() {
        let selector = grub
            .next_entry
            .as_deref()
            .or(grub.saved_entry.as_deref())
            .unwrap_or("");
        let picked = grub
            .entries
            .iter()
            .find(|entry| !selector.is_empty() && entry.selector == selector)
            .map(|entry| entry.name.clone());
        rows.push(("GRUB".to_string(), picked.unwrap_or_default()));
    }
    // 三引导器通道（systemd-boot / OpenCore / Clover）：行与托盘下拉同源
    // （`channel_switch_rows` 会带上 OpenCore 的菜单预演，否则 OC 那行永远不出现）
    for channel in channels.iter().filter(|channel| channel.detected) {
        let switchable = channel_switch_rows(channel);
        // 一条可切换项都没有、也判不出默认项：整行不出现（不摆「未选中」噪音）；
        // 只有默认项没有切换值域（OC 全是 Preboot 条目）时仍要出这一行。
        if switchable.is_empty() && channel_default_title(channel).is_none() {
            continue;
        }
        // 默认项优先在可切换行里找；找不到（OC 的 macOS 在 Preboot 卷上、
        // 没有可发的值域）就用 `channel_default_title` 补 —— 「下次启动」要回答
        // 的是「下次会起谁」，不是「能不能在这里改它」。
        let picked = switchable
            .iter()
            .find(|row| row.is_default)
            .map(|row| row.title.clone())
            .or_else(|| channel_default_title(channel))
            .unwrap_or_default();
        rows.push((channel.label.to_string(), picked));
    }
    rows
}

/// 顶部那一项的完整文字：首行「下次启动」，其后每行「引导器 · 条目」。
///
/// 宿主对多行 label 的处理各不相同（自绘窗 `white-space: pre-line`、原生菜单
/// 直接把 `\n` 当回车），所以这里只给文本，不掺任何排版符号。
pub(crate) fn next_boot_label(state: &UiState) -> String {
    next_boot_label_from_rows(&next_boot_rows(state))
}

pub(crate) fn next_boot_label_from_rows(rows: &[(String, String)]) -> String {
    if rows.is_empty() {
        return "下次启动\n未知（没有选中的系统）".to_string();
    }
    let mut text = String::from("下次启动");
    for (name, entry) in rows {
        text.push('\n');
        text.push_str(name);
        text.push_str(" · ");
        text.push_str(if entry.is_empty() {
            "未选中条目"
        } else {
            entry.as_str()
        });
    }
    text
}

/// rEFInd 那块下拉框的内容：条目与主窗 rEFInd 卡同一份（顺序即扫描顺序）。
///
/// 被隐藏规则挡掉的、标题空的不列。选中项（`vars/PreviousBoot` 命中的那条）
/// 置顶一份，原位不再重复 —— 通道/GRUB 那套「置顶 + 原位」在条目多时会把菜单
/// 撑成两倍长，这里只置顶一次。
/// rEFInd 那块的「一行」—— 两种读取方式共用一个形态，动作靠 `system` 区分：
/// `None` = 自动扫描链路（点 = 把标题原文写进 `vars/PreviousBoot`）；
/// `Some(system)` = 手动处理链路（点 = 写 vars 快照，值域是 `windows`/`macos`/`linux`）。
pub(crate) struct RefindRow {
    /// 菜单 id 里带的值：自动 = 标题**原文**（末尾空格要留着，写 PreviousBoot 要
    /// 原样匹配），手动 = system 名。
    pub(crate) value: String,
    /// 展示用（已 trim）
    pub(crate) label: String,
    /// windows / macos / linux …（只用来挑图标）
    pub(crate) kind: String,
    pub(crate) system: Option<String>,
    pub(crate) is_default: bool,
}

/// rEFInd 当前的**数据源**（按 `Settings.refind_mode` 选），下次启动行与托盘
/// 下拉都吃它 —— 否则「自动扫描」模式下点了托盘、主窗那行却不动。
pub(crate) struct RefindSource<'a> {
    pub(crate) mode: state::LoaderMode,
    pub(crate) sim: Option<&'a channels::refind_menu::SimPreview>,
    pub(crate) variables: &'a [VariableInfo],
}

impl RefindSource<'_> {
    pub(crate) fn rows(&self) -> Vec<RefindRow> {
        if self.mode.is_manual() {
            // 手动处理：vars 快照那三行系统（不存在的那些不列）
            return self
                .variables
                .iter()
                .filter(|item| item.exists && !item.display_name.trim().is_empty())
                .map(|item| RefindRow {
                    value: item.system.clone(),
                    label: item.display_name.trim().to_string(),
                    kind: item.system.clone(),
                    system: Some(item.system.clone()),
                    is_default: item.active,
                })
                .collect();
        }
        self.sim
            .map(|sim| {
                sim.entries
                    .iter()
                    .filter(|entry| !entry.hidden && !entry.title.trim().is_empty())
                    .map(|entry| RefindRow {
                        value: entry.title.clone(),
                        label: entry.title.trim().to_string(),
                        kind: entry.kind.clone(),
                        system: None,
                        is_default: entry.last_boot,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn links_sim(&self) -> bool {
        !self.mode.is_manual()
    }
}

pub(crate) fn refind_loader_nodes(rows: &[RefindRow], busy: bool) -> Vec<MenuNode> {
    if rows.is_empty() {
        return Vec::new();
    }
    let row = |entry: &RefindRow| {
        let id = match &entry.system {
            Some(system) => format!("refind-sys:{system}"),
            // id 里放**标题原文**（末尾那个空格也留着）：后端要原样写进
            // `vars/PreviousBoot`，trim 过就匹配不上 rEFInd 的菜单项。
            None => format!("refind-switch:{}", entry.value),
        };
        menu_marked_choice_item(
            id,
            format!("{}{}", grub_os_icon(&entry.label), entry.label),
            !busy,
            entry.is_default,
        )
    };
    let mut items: Vec<MenuNode> = Vec::new();
    if let Some(picked) = rows.iter().find(|entry| entry.is_default) {
        items.push(row(picked));
    }
    for entry in rows.iter().filter(|entry| !entry.is_default) {
        items.push(row(entry));
    }
    items
}

/// 三引导器通道那块下拉框的内容（与主窗通道卡同一份数据）。
///
/// 行来源统一走 `channel_switch_rows`：OpenCore 的可切换项在菜单预演里，
/// 不在 `entries` 里（详见那个函数的说明）。
pub(crate) fn channel_loader_nodes(
    channel: &channels::ChannelView,
    busy: bool,
) -> Vec<MenuNode> {
    let node_of = |row: &ChannelRow| {
        menu_marked_choice_item(
            format!("channel-switch:{}", row.spec),
            format!("{}{}", grub_os_icon(&row.title), row.title),
            // 需要提权的条目照样可点：链路里会走服务优先 / 授权兜底（与主窗一致），
            // 不在这里预先拦掉，否则 Windows 上「设为默认」这类条目在托盘里永远点不动。
            !busy,
            row.is_default,
        )
    };
    let rows = channel_switch_rows(channel);
    let mut items: Vec<MenuNode> = Vec::new();
    if let Some(picked) = rows.iter().find(|row| row.is_default) {
        items.push(node_of(picked));
    }
    for row in rows.iter().filter(|row| !row.is_default) {
        items.push(node_of(row));
    }
    // 预演里**没有值域**的行也要列进来（macOS 默认项在 Preboot 卷上没有
    // `opencore-default:` 值域、协议驱动、Reset NVRAM 这类工具）—— 托盘下拉
    // 要与主窗卡片是同一份内容，缺了默认项那一行，用户看到的就是「右键菜单
    // 不对」「两边对不上」。点不了就置灰，是默认项的照样打勾。
    if let Some(preview) = &channel.menu_preview {
        for entry in &preview.entries {
            let title = entry.title.trim();
            if title.is_empty()
                || entry
                    .spec
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .is_some()
                || rows.iter().any(|row| row.title == title)
            {
                continue;
            }
            items.push(menu_marked_choice_item(
                // id 走一个**不解析**的前缀（见 MenuAction::from_id 的白名单）：
                // 这些行没有值域可发，点了也该没动作。
                // **enabled 必须是 true**：Windows 原生菜单把禁用项的位图勾画成
                // 灰色，跟 GRUB/rEFInd 那些可用行的绿勾长得不一样（用户要求对勾
                // 必须一致）。点了没反应是安全的 —— id 不在解析白名单里。
                format!("channel-fixed:{}", title),
                format!("{}{}", grub_os_icon(title), title),
                true,
                entry.default,
            ));
        }
    }
    items
}

/// 托盘引导区：每个引导器一块子菜单（= 下拉框），顺序与主窗卡片一致
/// （rEFInd → GRUB → systemd-boot → OpenCore → Clover）。
pub(crate) fn loader_menu_nodes(state: &UiState) -> Vec<MenuNode> {
    let grub = effective_grub(state);
    loader_menu_nodes_from(
        refind_source(state),
        &grub,
        &state.channels,
        state.busy.affects_menu(),
    )
}

/// `loader_menu_nodes` 的可测内核（理由同 `next_boot_rows_from`）：只认三份数据。
///
/// `refind` / `grub` 都是**已经按读取方式选好的那份**（`refind_source` /
/// `effective_grub`）—— 托盘下拉里勾在哪一行，必须跟主窗「下次启动」同一口径。
pub(crate) fn loader_menu_nodes_from(
    refind: RefindSource<'_>,
    grub: &GrubView,
    channels: &[channels::ChannelView],
    busy: bool,
) -> Vec<MenuNode> {
    let mut nodes: Vec<MenuNode> = Vec::new();
    // rEFInd：当前那份有行才出块（没跑过扫描 / 快照没生成过 = 不该占位）。
    // 自动模式下全被隐藏规则挡掉时给一条说明，别让用户以为菜单坏了。
    {
        let rows = refind.rows();
        let auto = refind.links_sim();
        if auto && refind.sim.is_none() {
            // 自动扫描还没跑：不出块
        } else if rows.is_empty() && !auto {
            // 手动处理但一条快照都没有：同样不出块（多数是压根没装 rEFInd）
        } else {
            let children = refind_loader_nodes(&rows, busy);
            let children = if children.is_empty() {
                vec![menu_item(
                    "refind-empty",
                    "没有可见的菜单项（都被隐藏规则挡掉了）",
                    false,
                )]
            } else {
                children
            };
            nodes.push(menu_submenu("refind-entries", "rEFInd 启动项", children));
        }
    }
    // GRUB：有可点的条目才出块；一条都没解析到但**已经配置过 ESP** 的，也给块
    // （块内是 grub_menu_nodes 自带的那条禁用说明，告诉用户还差什么）。
    let grub_nodes = grub_menu_nodes(grub, busy);
    let grub_configured = !grub.partuuid.is_empty() || !grub.dir.is_empty() || grub.esp.is_some();
    if grub_nodes.iter().any(|node| node.enabled) || grub_configured {
        nodes.push(menu_submenu("grub-entries", "GRUB 启动项", grub_nodes));
    }
    // 通道：检测到就出块（哪怕当前没有可切换的菜单项 —— 主窗能看菜单预览，
    // 托盘至少要让人知道「这台机器上有它」）。未检测到的整块不出现。
    for channel in channels {
        if !channel.detected {
            continue;
        }
        let children = channel_loader_nodes(channel, busy);
        let children = if children.is_empty() {
            vec![menu_item(
                format!("channel-empty:{}", channel.id),
                "没有可切换的菜单项（在主窗看菜单预览）",
                false,
            )]
        } else {
            children
        };
        nodes.push(menu_submenu(
            format!("channel:{}", channel.id),
            format!("{} 启动项", channel.label),
            children,
        ));
    }
    // 没检测到的通道：不摆空壳子菜单，但**必须留一行说明** —— 否则用户看着
    // 「只有 rEFInd / GRUB 两块」会以为是程序漏做了这几个引导器
    // （用户 2026-10-30 的原话就是「为什么没有 opencore clover systemd-boot」）。
    let missing: Vec<&str> = channels
        .iter()
        .filter(|channel| !channel.detected)
        .map(|channel| channel.label)
        .collect();
    if !missing.is_empty() {
        nodes.push(menu_item(
            "loaders-missing",
            format!("未检测到：{}（这台机器上没有）", missing.join("、")),
            false,
        ));
    }
    nodes
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

/// 条目的内核名：优先 `linux` 行解析出的内核文件名，取不到（或是一串空白）退回
/// 标题原文 —— 空白内核按「没有」算，不然极简档会渲染出一行空白。
pub(crate) fn grub_kernel_of(entry: &grub::GrubEntry) -> String {
    entry
        .kernels
        .iter()
        .map(|kernel| kernel.trim())
        .find(|kernel| !kernel.is_empty())
        .map(str::to_string)
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
    // 扫描成功后 `status.message` 就是「下次启动：X」（rEFInd 那一层）—— 去重拿它比对，
    // 与顶部那行改成什么排版无关（见 menu_status_line）。
    let next_boot_scan_line = state
        .status
        .variables
        .iter()
        .find(|variable| variable.active)
        .map_or_else(
            || "下次启动：未知".to_string(),
            |variable| format!("下次启动：{}", variable.display_name),
        );
    // 顶部那行：**与主窗「下次启动」同源** —— 每个引导器一行（见 next_boot_rows）。
    let boot_rows = next_boot_rows(state);
    let boot_label = next_boot_label_from_rows(&boot_rows);

    let mut nodes: Vec<MenuNode> = Vec::new();
    // 第一行就把「下次启动哪些系统」摆出来。原生菜单改不了字号/字重，所以靠**位置**
    // 和标记让它够醒目；点它 = 打开主窗口去改。
    nodes.push(menu_marked_item(
        "next-boot",
        boot_label,
        true,
        !boot_rows.is_empty(),
    ));
    // 状态行只在**跟上面那行不一样**的时候才出现（见 menu_status_line）：
    // 扫描成功后 message 本身就是「下次启动：X」，再摆一行就是「下次启动重复了」，
    // 只有错误提示/临时操作结果才需要单独一行。
    if let Some(line) = menu_status_line(message, &next_boot_scan_line) {
        nodes.push(menu_item("status", line, false));
    }
    nodes.push(menu_separator());

    // —— 分区：引导器（每个引导器一块下拉框）——
    //
    // 与主窗「系统引导」页一一对应。**rEFInd 的 Windows / Linux / macOS 三个固定项
    // 已删除**：那是模板快照，不是 rEFInd 菜单里的真实条目（用户 2026-10-30 定案）。
    // 某块为空 = 该引导器没检测到 / 没读到条目，整块不出现。
    let loaders = loader_menu_nodes(state);
    if loaders.is_empty() {
        nodes.push(menu_item("boot-empty", "没有可用的引导项（先扫描）", false));
    } else {
        nodes.extend(loaders);
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
    // 引导器下拉框进指纹：**直接用托盘菜单真正会显示的那份节点**（loader_menu_nodes，
    // 与主窗引导器卡同一份快照字段）。以前是单独序列化 GRUB 那块，加了 rEFInd /
    // OpenCore / Clover 之后那样会漏 —— 换了条目菜单却不重建，托盘就停在旧勾选上。
    let loaders = serde_json::to_string(&loader_menu_nodes(state))
        .expect("引导器菜单节点一定能序列化");
    // 顶部「下次启动」多行也进指纹：切完条目要立刻看到那行跟着变
    let next_boot = next_boot_label(state);
    format!(
        "{base}|wpos={position}|fmon={fixed_monitor}|fmons={monitor_options}|loaders={loaders}|nb={next_boot}"
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
    /// 切换 rEFInd 启动项（写 `vars/PreviousBoot`，与主窗 rEFInd 列表同源）
    SwitchRefind(String),
    /// 切换 rEFInd「手动处理」那条链路的系统（写 vars 快照，值域 = windows/macos/linux）。
    /// 与 `SwitchRefind` 不是一回事：那个按标题原文写 PreviousBoot，这个按 system 写快照。
    SwitchSystem(String),
    /// 切换三引导器通道的条目（`sdboot:<id>` / `opencore` / `clover-oneshot`…），
    /// 与主窗通道卡同一个值域（见 `channel_entry_spec`）
    SwitchChannel(String),
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
        // rEFInd 条目的 id 里是**标题原文**（末尾那个空格要留着写进 PreviousBoot），
        // 所以这里只判空、绝不 trim。
        if let Some(rest) = id.strip_prefix("refind-switch:") {
            return (!rest.trim().is_empty()).then(|| Self::SwitchRefind(rest.to_string()));
        }
        if let Some(rest) = id.strip_prefix("refind-sys:") {
            return (!rest.trim().is_empty()).then(|| Self::SwitchSystem(rest.trim().to_string()));
        }
        if let Some(rest) = id.strip_prefix("channel-switch:") {
            return (!rest.trim().is_empty()).then(|| Self::SwitchChannel(rest.to_string()));
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
            // 与主窗 GRUB 区同一条链路：写 grubenv → 重扫 → 广播。
            // **写哪份 grubenv 必须跟「读取方式」走**：自动模式的托盘列表是
            // `linux_grub` 那份（`effective_grub` 覆盖过），手动模式才是 ESP 那份。
            // 以前恒写 ESP grubenv —— 自动模式下托盘点完，托盘勾选不动、主窗
            // 列表与「下次启动」也不变（state.grub 更新了但 effective_grub 根本
            // 不读它），看起来就是「点了没反应」，双向同步就是这么丢的。
            let auto = !view_settings(&app).settings.grub_mode.is_manual();
            if auto {
                // 与主窗自动页签（`linux_grub_switch` 命令）同一实现：回填
                // `linux_grub` 的 next/saved → record → publish，托盘指纹随
                // `loaders=`/`nb=` 变化自动重建。
                channels::linux_grub::switch_blocking(app.clone(), selector);
            } else {
                switch_grub_and_update(&app, &selector);
            }
        }
        MenuAction::SwitchRefind(title) => {
            // 与主窗 rEFInd 列表同一条链路（`refind_sim_switch`）：校验标题仍在当前
            // 菜单里 → 写 vars/PreviousBoot → 重算模拟快照 → 广播。
            // 校验失败是「正常返回 + error」，这里补一条动作记录，否则托盘点完没反应。
            let status = channels::refind_menu::refind_sim_switch(app.clone(), title);
            if let Some(error) = status.error {
                record_action(&app, "切换 rEFInd 启动项", false, &error);
            }
        }
        MenuAction::SwitchSystem(system) => {
            // 与主窗「手动处理」列表同一手势：写 vars 快照（Boot* / PreviousBoot），
            // 值域是 system 名 —— 别拿 SwitchRefind 的标题原文去糊这一条。
            let status = switch_system_locally(&app, &system);
            if let Some(error) = status.error {
                record_action(&app, "切换系统", false, &error);
            }
        }
        MenuAction::SwitchChannel(spec) => {
            // 与主窗通道卡同一条链路：解析值域 → 执行（必要时提权）→ 重扫 → 广播
            switch_channel_and_update(&app, &spec);
        }
        MenuAction::Quit => app.exit(0),
    });
}

pub(crate) fn parse_menu_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}
