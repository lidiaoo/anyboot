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
    if let Some(text) = tag("string") {
        // 最小反转义（读展示用，config.plist 的目标键不会出现生僻实体）
        return Some(PlistScalar::String(
            text.replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&"),
        ));
    }
    if let Some(text) = tag("integer") {
        return text.trim().parse::<i64>().ok().map(PlistScalar::Integer);
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

/// 沿路径改标量：返回**整份改写后的 XML**（除目标值节点外逐字节不变）。
/// 路径定位失败 / 中间层不是 dict / 目标不存在 → Err，绝不插入猜出来的结构。
pub(crate) fn rewrite_scalar(xml: &str, path: &[&str], new_value: PlistScalar) -> Result<String, String> {
    let (last, parents) = path.split_last().ok_or("空路径")?;
    // 一层层剥到最内层 dict，记下每层 dict 的内层区间
    let mut spans: Vec<(usize, usize)> = Vec::new(); // (dict 内层起点, dict 内层终点) 相对上一层
    let mut scope = xml;
    for parent in parents {
        let (start, end) = locate_pair(scope, parent)
            .ok_or_else(|| format!("定位失败：键 {parent} 不存在"))?;
        let node = &scope[start..end];
        let head_len = "<dict>".len();
        if !node.starts_with("<dict>") {
            return Err(format!("键 {parent} 的值不是 dict"));
        }
        let inner_start = start + head_len;
        let inner_end = start + node.len() - "</dict>".len();
        if inner_end < inner_start {
            return Err(format!("键 {parent} 的 dict 结构残缺"));
        }
        spans.push((inner_start, inner_end));
        scope = &node[head_len..node.len() - "</dict>".len()];
    }
    let (start, end) = locate_pair(scope, last)
        .ok_or_else(|| format!("定位失败：键 {last} 不存在"))?;
    // 把最内层区间换算回全文坐标：逐层累加
    let mut offset = 0usize;
    for (inner_start, _inner_end) in &spans {
        // spans 是相对各层 scope 的，逐层换算：
        // 全文坐标 = 上一层坐标 + inner_start
        offset += inner_start;
    }
    // 注意：spans 里存的是「dict 内层在当层 scope 里的区间」，而每层的 scope
    // 起点本身就是上一层换算后的位置 —— 累加即为全文坐标。
    let value_start = offset + start;
    let value_end = offset + end;
    if value_end > xml.len() || xml[value_start..value_end] != scope[start..end] {
        return Err("改写定位校验失败（文件在解析期间被改动？）".to_string());
    }
    let mut out = String::with_capacity(xml.len() + 32);
    out.push_str(&xml[..value_start]);
    out.push_str(&new_value.to_xml());
    out.push_str(&xml[value_end..]);
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
}
