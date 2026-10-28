//! Apple XML plist 的最小读写（OC/Clover 的 config.plist）。
//!
//! 不引 `plist` crate（连坐 quick-xml 一串依赖，且全量反序列化再序列化会
//! 重排键、丢注释、改缩进格式）。config.plist 是**纯文本**，AnyBoot 只做两件事：
//!
//! * `read_scalar` —— 沿 `["Misc","Boot","ShowPicker"]` 的字典路径读标量
//!   （string/integer/true/false），读不到/类型不符一律 `None`，绝不猜；
//! * `rewrite_scalar` —— 字节级定位该 key 的值节点，**只替换那一小段**，
//!   其余内容（注释、缩进、键序、BOM）逐字节保留。
//!
//! 解析器是手写的受限下降：只认 `<dict><key>…</key>…</dict>` 嵌套与四种标量，
//! 碰到 array/data/date 等一律当「不可读」。够 OC/Clover 健康检查与
//! DefaultVolume/DefaultPickerValue 定点改写用。

/// 标量值（plist XML 的四种 AnyBoot 关心的类型）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlistScalar {
    String(String),
    Integer(i64),
    Bool(bool),
}

impl PlistScalar {
    /// 序列化成 XML 文本片段（rewrite 用）。
    pub(crate) fn to_xml(&self) -> String {
        match self {
            Self::String(value) => {
                // 只转义写 XML 会坏的三样；config.plist 里的卷名/路径不会出现别的
                format!(
                    "<string>{}</string>",
                    value
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;")
                )
            }
            Self::Integer(value) => format!("<integer>{value}</integer>"),
            Self::Bool(true) => "<true/>".to_string(),
            Self::Bool(false) => "<false/>".to_string(),
        }
    }
}

/// 从 `<string>…</string>` 等值节点原文里解出标量；`raw` 不带外层标签则返回 None。
///
/// **只认节点自身就是这个标签**：早期实现用 `find`/`rfind` 直接抓内部
/// `<string>`，遇到 `<array><string>a</string></array>` 会把数组首元素当成
/// 该键的值（has_key 单测抓到的真实缺陷）。
fn parse_scalar_node(node: &str) -> Option<PlistScalar> {
    let node = node.trim();
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = node.find(&open)? + open.len();
        let end = node.rfind(&close)?;
        if end < start {
            return None;
        }
        Some(node[start..end].to_string())
    };
    if node.starts_with("<true/>") || node.starts_with("<true ") {
        return Some(PlistScalar::Bool(true));
    }
    if node.starts_with("<false/>") || node.starts_with("<false ") {
        return Some(PlistScalar::Bool(false));
    }
    if node.starts_with("<string>") || node.starts_with("<string ") {
        // 最小反转义（读展示用，config.plist 的目标键不会出现生僻实体）
        return tag("string").map(|text| {
            PlistScalar::String(
                text.replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&"),
            )
        });
    }
    if node.starts_with("<integer>") || node.starts_with("<integer ") {
        return tag("integer")
            .and_then(|text| text.trim().parse::<i64>().ok())
            .map(PlistScalar::Integer);
    }
    None
}

/// 在 dict 原文里定位 key 对应值节点的字节区间 `[value_start, value_end)`。
///
/// 算法：顺序扫 `<key>Name</key>`，取它后面第一个非空白节点开标签，
/// 再找该标签的闭合（自闭合 `<true/>` 直接结束）。dict 里 key 与值严格成对，
/// 所以「下一个 key 之前」就是值的边界。
fn locate_pair(dict: &str, key: &str) -> Option<(usize, usize)> {
    let needle = format!("<key>{key}</key>");
    let mut search_from = 0;
    while let Some(key_start) = dict[search_from..].find(&needle) {
        let key_start = search_from + key_start;
        let after_key = key_start + needle.len();
        // 跳过 key 与值之间的空白
        let rest = &dict[after_key..];
        let ws = rest.len() - rest.trim_start().len();
        let value_start = after_key + ws;
        let rest = &dict[value_start..];
        if !rest.starts_with('<') {
            // 不是节点（残缺文件）：跳过这处 key 继续找
            search_from = key_start + needle.len();
            continue;
        }
        let value_end = node_end(rest)?;
        return Some((value_start, value_start + value_end));
    }
    None
}

/// 一个节点（从 `<tag>` 起）到它同名闭合标签（含）的长度。自闭合直接结束。
///
/// 按同名标签**深度计数**（嵌套 `<dict>` 是常态），并跳过 `<!-- -->` 注释 ——
/// config.plist 里注释是合法且常见的，注释里出现 `</dict>` 字样不能算数。
fn node_end(node: &str) -> Option<usize> {
    let tag_end = node.find('>')? + 1;
    let head = &node[..tag_end];
    // 自闭合：<true/> <false/> <integer/>（残缺）
    if head.ends_with("/>") {
        return Some(tag_end);
    }
    // 取开标签名
    let name = head
        .strip_prefix('<')?
        .split(|c: char| c.is_whitespace() || c == '>')
        .next()?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut depth = 1usize;
    let mut pos = tag_end;
    while pos < node.len() {
        if node[pos..].starts_with("<!--") {
            // 注释整段跳过（没有闭合的注释算文件残缺）
            let end = node[pos..].find("-->")? + pos + 3;
            pos = end;
            continue;
        }
        if node.as_bytes()[pos] == b'<' {
            let gt = node[pos..].find('>')? + pos;
            let inner = node[pos + 1..gt].trim();
            if let Some(closing) = inner.strip_prefix('/') {
                if closing.trim() == name {
                    depth -= 1;
                    if depth == 0 {
                        return Some(gt + 1);
                    }
                }
            } else if !inner.ends_with('/')
                && inner.split(|c: char| c.is_whitespace()).next() == Some(name)
            {
                depth += 1;
            }
            pos = gt + 1;
            continue;
        }
        pos += 1;
    }
    None
}

/// 沿路径读标量。`path` 最后一层是 key，前面每层都是 dict。
///
/// ```ignore
/// read_scalar(xml, &["Misc", "Boot", "ShowPicker"]) // → Some(Bool(false))
/// ```
pub(crate) fn read_scalar(xml: &str, path: &[&str]) -> Option<PlistScalar> {
    let (last, parents) = path.split_last()?;
    let mut scope = xml;
    for parent in parents {
        let (start, end) = locate_pair(scope, parent)?;
        let node = &scope[start..end];
        // 中间层必须是 dict：取 <dict> 的内层继续
        let inner = node.strip_prefix("<dict>")?;
        let inner_end = inner.rfind("</dict>")?;
        scope = &inner[..inner_end];
    }
    let (start, end) = locate_pair(scope, last)?;
    parse_scalar_node(&scope[start..end])
}

/// 沿 `parents` 逐层下钻到最内层 dict，返回它的**内层区间**（全文坐标）：
/// 即 `<dict>` 之后、`</dict>` 之前的那一段。`parents` 为空表示根 dict。
///
/// 一次把坐标算成全文绝对值，避免上层 `rewrite`/`insert` 各自再做偏移累加。
fn innermost_dict_span(xml: &str, parents: &[&str]) -> Result<(usize, usize), String> {
    let mut scope_start = 0usize;
    let mut scope_end = xml.len();
    for parent in parents {
        let (start, end) = locate_pair(&xml[scope_start..scope_end], parent)
            .ok_or_else(|| format!("定位失败：键 {parent} 不存在"))?;
        let node_start = scope_start + start;
        let node_len = end - start;
        let node = &xml[node_start..node_start + node_len];
        if !node.starts_with("<dict>") {
            return Err(format!("键 {parent} 的值不是 dict"));
        }
        let inner_start = node_start + "<dict>".len();
        let inner_end = node_start + node_len - "</dict>".len();
        if inner_end < inner_start {
            return Err(format!("键 {parent} 的 dict 结构残缺"));
        }
        scope_start = inner_start;
        scope_end = inner_end;
    }
    Ok((scope_start, scope_end))
}

/// 路径上的 key 是否存在（**不看值类型**：array/data 也算存在，避免把可疑结构
/// 当成缺失再插一个同名键）。
pub(crate) fn has_key(xml: &str, path: &[&str]) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    match innermost_dict_span(xml, parents) {
        Ok((start, end)) => locate_pair(&xml[start..end], last).is_some(),
        Err(_) => false,
    }
}

/// 沿路径读 `<array>` 里的字符串元素（`UEFI→Drivers`、`BlessOverride` 这类）。
///
/// 只摘 `<string>`：新版 config 的 `UEFI→Drivers` 是 `<dict>` 数组、路径在
/// `Path` 键里也是 `<string>`，一并摘出来正合「里面有没有某个驱动」的用途。
/// 值不是 array → `None`（不猜、不把单值当数组）。
pub(crate) fn read_string_array(xml: &str, path: &[&str]) -> Option<Vec<String>> {
    let (last, parents) = path.split_last()?;
    let (scope_start, scope_end) = innermost_dict_span(xml, parents).ok()?;
    let (start, end) = locate_pair(&xml[scope_start..scope_end], last)?;
    let node = xml[scope_start + start..scope_start + end].trim();
    let inner = node.strip_prefix("<array>")?.strip_suffix("</array>")?;
    Some(
        inner
            .split("<string>")
            .skip(1)
            .filter_map(|chunk| chunk.split("</string>").next())
            .map(|text| {
                text.replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&")
            })
            .collect(),
    )
}

/// 沿路径改标量：返回**整份改写后的 XML**（除目标值节点外逐字节不变）。
/// 路径定位失败 / 中间层不是 dict / 目标不存在 → Err，绝不插入猜出来的结构。
pub(crate) fn rewrite_scalar(xml: &str, path: &[&str], new_value: PlistScalar) -> Result<String, String> {
    let (last, parents) = path.split_last().ok_or("空路径")?;
    let (scope_start, scope_end) = innermost_dict_span(xml, parents)?;
    let scope = &xml[scope_start..scope_end];
    let (start, end) = locate_pair(scope, last)
        .ok_or_else(|| format!("定位失败：键 {last} 不存在"))?;
    let value_start = scope_start + start;
    let value_end = scope_start + end;
    if value_end > xml.len() || xml[value_start..value_end] != scope[start..end] {
        return Err("改写定位校验失败（文件在解析期间被改动？）".to_string());
    }
    let mut out = String::with_capacity(xml.len() + 32);
    out.push_str(&xml[..value_start]);
    out.push_str(&new_value.to_xml());
    out.push_str(&xml[value_end..]);
    Ok(out)
}

/// dict 内层里同级键的缩进（取第一条非空行的前导空白）。空 dict 用单个 TAB。
fn dict_indent(inner: &str) -> String {
    inner
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line[..line.len() - line.trim_start().len()].to_string())
        .unwrap_or_else(|| "\t".to_string())
}

/// 往 `parents` 指向的 dict 末尾**新增**一个键值对（缩进照抄同级）。
///
/// 只用于 OC 那种「老配置里可能压根没有这个键」的场景（LauncherOption /
/// RequestBootVarRouting）。键已存在请走 `rewrite_scalar`——这里不做存在性判断，
/// 由调用方用 `has_key` 决定，避免重复插入。
pub(crate) fn insert_scalar(
    xml: &str,
    parents: &[&str],
    key: &str,
    value: PlistScalar,
) -> Result<String, String> {
    let (scope_start, scope_end) = innermost_dict_span(xml, parents)?;
    let inner = &xml[scope_start..scope_end];
    let indent = dict_indent(inner);
    // 插在最后一个有效内容之后（尾部空白/换行原样保留给 `</dict>` 前）
    let body_end = scope_start + inner.trim_end().len();
    let snippet = format!("\n{indent}<key>{key}</key>\n{indent}{}", value.to_xml());
    let mut out = String::with_capacity(xml.len() + snippet.len());
    out.push_str(&xml[..body_end]);
    out.push_str(&snippet);
    out.push_str(&xml[body_end..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<!-- GUI 配置 -->
	<key>GUI</key>
	<dict>
		<key>Mouse</key>
		<false/>
		<key>Boot</key>
		<dict>
			<key>Timeout</key>
			<integer>5</integer>
			<key>DefaultVolume</key>
			<string>Macintosh HD</string>
			<key>ShowPicker</key>
			<true/>
		</dict>
	</dict>
	<key>Misc</key>
	<dict>
		<key>Boot</key>
		<dict>
			<key>ShowPicker</key>
			<false/>
			<key>Timeout</key>
			<integer>10</integer>
			<key>LauncherOption</key>
			<string>Full</string>
		</dict>
		<key>Security</key>
		<dict>
			<key>AllowSetDefault</key>
			<false/>
			<key>Vault</key>
			<string>Basic</string>
		</dict>
	</dict>
</dict>
</plist>
"#;

    #[test]
    fn nested_dict_value_node_end() {
        // 中间层 dict 自身又嵌 dict（Clover 的 GUI 下常见），截断不能丢内容
        let nested = r#"<plist><dict>
	<key>GUI</key>
	<dict>
		<key>Theme</key>
		<dict><key>Embedded</key><true/></dict>
		<key>Boot</key>
		<dict><key>DefaultVolume</key><string>ARCH</string></dict>
	</dict>
</dict></plist>"#;
        assert_eq!(
            read_scalar(nested, &["GUI", "Boot", "DefaultVolume"]),
            Some(PlistScalar::String("ARCH".into()))
        );
        let out = rewrite_scalar(
            nested,
            &["GUI", "Boot", "DefaultVolume"],
            PlistScalar::String("WIN10".into()),
        )
        .unwrap();
        assert_eq!(
            read_scalar(&out, &["GUI", "Boot", "DefaultVolume"]),
            Some(PlistScalar::String("WIN10".into()))
        );
        // 嵌套 dict 里的兄弟键没被误伤
        assert_eq!(
            read_scalar(&out, &["GUI", "Theme", "Embedded"]),
            Some(PlistScalar::Bool(true))
        );
    }

    #[test]
    fn reads_scalars_along_path() {
        assert_eq!(
            read_scalar(SAMPLE, &["Misc", "Boot", "ShowPicker"]),
            Some(PlistScalar::Bool(false))
        );
        assert_eq!(
            read_scalar(SAMPLE, &["Misc", "Boot", "Timeout"]),
            Some(PlistScalar::Integer(10))
        );
        assert_eq!(
            read_scalar(SAMPLE, &["Misc", "Security", "Vault"]),
            Some(PlistScalar::String("Basic".into()))
        );
        assert_eq!(
            read_scalar(SAMPLE, &["GUI", "Boot", "DefaultVolume"]),
            Some(PlistScalar::String("Macintosh HD".into()))
        );
        // 同名 key 在不同层不串（GUI/Boot/ShowPicker=true 而 Misc/Boot/ShowPicker=false）
        assert_eq!(
            read_scalar(SAMPLE, &["GUI", "Boot", "ShowPicker"]),
            Some(PlistScalar::Bool(true))
        );
        // 读不到的返回 None
        assert_eq!(read_scalar(SAMPLE, &["Misc", "Boot", "NotExist"]), None);
        assert_eq!(read_scalar(SAMPLE, &["Nope", "Boot"]), None);
    }

    #[test]
    fn rewrite_keeps_everything_else() {
        let out = rewrite_scalar(
            SAMPLE,
            &["GUI", "Boot", "DefaultVolume"],
            PlistScalar::String("ARCH-EFI".into()),
        )
        .unwrap();
        // 注释、缩进、其它键全部原样
        assert!(out.contains("<!-- GUI 配置 -->"));
        assert!(out.contains("<string>ARCH-EFI</string>"));
        assert!(!out.contains("Macintosh HD"));
        // 其它值没被误伤
        assert_eq!(
            read_scalar(&out, &["Misc", "Boot", "ShowPicker"]),
            Some(PlistScalar::Bool(false))
        );
        assert_eq!(
            read_scalar(&out, &["GUI", "Boot", "Timeout"]),
            Some(PlistScalar::Integer(5))
        );
        // 只替换了一处：ShowPicker 有两处（GUI 与 Misc），都没动
        assert_eq!(out.matches("<false/>").count(), SAMPLE.matches("<false/>").count());
    }

    #[test]
    fn rewrite_bool_and_missing_path_errors() {
        let out =
            rewrite_scalar(SAMPLE, &["Misc", "Security", "AllowSetDefault"], PlistScalar::Bool(true))
                .unwrap();
        assert_eq!(
            read_scalar(&out, &["Misc", "Security", "AllowSetDefault"]),
            Some(PlistScalar::Bool(true))
        );
        assert!(rewrite_scalar(SAMPLE, &["Misc", "Nope", "Vault"], PlistScalar::Bool(true)).is_err());
        assert!(rewrite_scalar(SAMPLE, &["Nothing"], PlistScalar::Bool(true)).is_err());
        // 中间层不是 dict → Err
        assert!(rewrite_scalar(SAMPLE, &["Misc", "Boot", "Timeout", "Deep"], PlistScalar::Bool(true)).is_err());
    }

    #[test]
    fn escaped_string_roundtrip() {
        let out = rewrite_scalar(
            SAMPLE,
            &["GUI", "Boot", "DefaultVolume"],
            PlistScalar::String("A&B<C>".into()),
        )
        .unwrap();
        assert_eq!(
            read_scalar(&out, &["GUI", "Boot", "DefaultVolume"]),
            Some(PlistScalar::String("A&B<C>".into()))
        );
    }

    #[test]
    fn insert_missing_key_keeps_siblings() {
        // OC 老配置里没有 LauncherOption：插到 Misc→Boot 末尾
        let xml = r#"<plist><dict>
	<key>Misc</key>
	<dict>
		<key>Boot</key>
		<dict>
			<key>ShowPicker</key>
			<false/>
			<key>Timeout</key>
			<integer>10</integer>
		</dict>
	</dict>
</dict></plist>"#;
        assert!(!has_key(xml, &["Misc", "Boot", "LauncherOption"]));
        let out = insert_scalar(
            xml,
            &["Misc", "Boot"],
            "LauncherOption",
            PlistScalar::String("Short".into()),
        )
        .unwrap();
        assert_eq!(
            read_scalar(&out, &["Misc", "Boot", "LauncherOption"]),
            Some(PlistScalar::String("Short".into()))
        );
        // 兄弟键原样
        assert_eq!(
            read_scalar(&out, &["Misc", "Boot", "ShowPicker"]),
            Some(PlistScalar::Bool(false))
        );
        assert_eq!(
            read_scalar(&out, &["Misc", "Boot", "Timeout"]),
            Some(PlistScalar::Integer(10))
        );
        // 缩进跟同级一致（三行 TAB），且闭合标签没被挤到值后面
        assert!(
            out.contains("\n\t\t\t<key>LauncherOption</key>\n\t\t\t<string>Short</string>\n\t\t</dict>"),
            "插入片段的缩进/换行不对:\n{out}"
        );
        // 插完仍是合法结构：has_key 为真
        assert!(has_key(&out, &["Misc", "Boot", "LauncherOption"]));
        // 幂等检查：再插一次会变成两个同名键 —— 所以调用方必须先 has_key
    }

    #[test]
    fn insert_into_empty_and_root_dict() {
        // 空 dict：没有同级可抄缩进，用 TAB 兜底，仍要合法
        let xml = "<plist><dict><key>UEFI</key><dict><key>Quirks</key><dict></dict></dict></dict></plist>";
        let out = insert_scalar(
            xml,
            &["UEFI", "Quirks"],
            "RequestBootVarRouting",
            PlistScalar::Bool(true),
        )
        .unwrap();
        assert_eq!(
            read_scalar(&out, &["UEFI", "Quirks", "RequestBootVarRouting"]),
            Some(PlistScalar::Bool(true))
        );
        // 根 dict（parents 为空）
        let out = insert_scalar("<plist><dict></dict></plist>", &[], "Top", PlistScalar::Bool(true))
            .unwrap();
        assert_eq!(read_scalar(&out, &["Top"]), Some(PlistScalar::Bool(true)));
        // 中间层不存在 → Err（不猜结构）
        assert!(insert_scalar(xml, &["Nope"], "X", PlistScalar::Bool(true)).is_err());
        assert!(insert_scalar("<plist><dict><key>A</key><string>x</string></dict></plist>", &["A"], "X", PlistScalar::Bool(true)).is_err());
    }

    #[test]
    fn has_key_sees_non_scalar_values() {
        // read_scalar 读不出 array，但 has_key 必须认（否则会重复插一个同名键）
        let xml = "<plist><dict><key>List</key><array><string>a</string></array><key>B</key><true/></dict></plist>";
        assert_eq!(read_scalar(xml, &["List"]), None);
        assert!(has_key(xml, &["List"]));
        assert!(has_key(xml, &["B"]));
        assert!(!has_key(xml, &["C"]));
    }
}
