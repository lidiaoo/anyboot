//! 守卫：同步 `#[tauri::command]` 里不许出现会阻塞的调用。
//!
//! 背景（两次线上事故，同一个病根）：
//! - 第一次：在 IPC 回调里 `WebviewWindowBuilder::build()` —— 重入 WebView2
//!   消息循环，窗口建得出来但页面永远停在白底首帧。
//! - 第二次：`hub_term_open` 里 `openpty` + `spawn_command` —— Windows 上
//!   是「起 conhost 并等命名管道握手」＋同步 CreateProcess，几十毫秒到数秒。
//!   同步命令跑在 UI 线程上，这段时间整个窗口不绘制、不吃输入。
//!
//! 两次都表现为「点了就卡死」，两次都是同一类错误：**在 UI 线程上做阻塞的事**。
//! 靠人肉 review 记不住，所以钉成测试：谁再往同步命令里塞阻塞调用，`cargo test`
//! 直接红，报出 文件:行 函数名 和命中的调用。
//!
//! 判定范围（保守，只拦确定会阻塞的）：
//! - 直接命中：命令体内出现 `FORBIDDEN` 里的调用；
//! - 间接命中：命令体调用了同文件里另一个**同步** fn，而那个 fn 命中（深度 ≤ 3）。
//!
//! 逃生口：函数体里写一行 `// blocking-ok: <理由>` 显式豁免。豁免是可见的，
//! review 时能看见，不会变成静默的口子。

use std::path::{Path, PathBuf};

/// (token, 为什么它不能待在 UI 线程上)
const FORBIDDEN: &[(&str, &str)] = &[
    ("Command::new", "起子进程"),
    ("openpty", "建 PTY（Windows 上是同步的 ConPTY 握手）"),
    ("spawn_command", "启动 PTY 子进程"),
    ("TcpStream::connect", "TCP 连接"),
    ("connect_timeout", "TCP 连接（要等满超时）"),
    ("thread::sleep", "睡眠"),
    (".wait()", "等子进程退出"),
    (".output()", "等子进程跑完"),
    (".dialog()", "系统模态对话框"),
    ("reqwest::", "网络请求"),
];

/// 显式豁免标记（必须带理由，写法见模块注释）
const ESCAPE: &str = "// blocking-ok";

#[test]
fn sync_commands_never_block() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = rs_files(&src_dir);
    assert!(
        files.len() > 5,
        "没扫到源码文件（{}）—— 测试自身的路径解析坏了",
        src_dir.display()
    );

    let mut violations: Vec<String> = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        violations.extend(scan_file(file, &text));
    }

    assert!(
        violations.is_empty(),
        "同步 #[tauri::command] 里出现了会阻塞的调用 —— 它们跑在 WebView2 的 IPC \
         回调线程（UI 线程）上，一阻塞就是整个窗口停止绘制和响应（用户看到「卡死」）。\n\
         改法：把命令改成 `async fn`，函数体塞进 \
         `tauri::async_runtime::spawn_blocking(move || …).await`。\n\
         确有必要保留同步的，在函数体里写 `// blocking-ok: 理由`。\n\n{}",
        violations.join("\n")
    );
}

fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rs_files(&path));
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
    out
}

/// 一个函数在源码里的位置与属性
struct FnDef {
    name: String,
    is_async: bool,
    body_start: usize,
    body_end: usize,
}

fn scan_file(file: &Path, text: &str) -> Vec<String> {
    let clean = sanitize(text);
    let defs = collect_fns(&clean);
    let mut violations = Vec::new();
    let needle = b"#[tauri::command]";
    let mut from = 0usize;
    while let Some(rel) = find(&clean[from..], needle) {
        let attr_start = from + rel;
        let attr_end = attr_start + needle.len();
        let Some(brace_rel) = clean[attr_end..].iter().position(|&c| c == b'{') else {
            break;
        };
        let brace = attr_end + brace_rel;
        let head = &clean[attr_end..brace];
        // 属性到第一个 `{` 之间若含 `;`，说明这是声明而非常规命令（不该发生），跳过
        if head.contains(&b';') {
            from = brace + 1;
            continue;
        }
        let is_async = contains(head, b"async");
        let name = fn_name(head).unwrap_or_else(|| "<anon>".to_string());
        let body_end = match_body(&clean, brace);
        if !is_async {
            let line = line_of(&clean, attr_start);
            let body_raw = slice_str(&text, brace, body_end);
            if !body_raw.contains(ESCAPE) {
                let scope = &clean[brace..=body_end.min(clean.len() - 1)];
                let mut hits = forbidden_in(scope);
                // 间接：调用了同文件里另一个**同步** fn，而那个 fn 自己也命中
                for other in &defs {
                    if other.is_async || other.name == name {
                        continue;
                    }
                    // 用 `name(` 精确匹配，避免 `foo` 命中 `foobar`
                    if find(scope, format!("{}(", other.name).as_bytes()).is_none() {
                        continue;
                    }
                    if other.body_start >= brace && other.body_end <= body_end {
                        continue; // 嵌套定义，不追
                    }
                    let other_raw = slice_str(&text, other.body_start, other.body_end);
                    if other_raw.contains(ESCAPE) {
                        continue;
                    }
                    let other_scope = &clean[other.body_start..=other.body_end.min(clean.len() - 1)];
                    for (token, why, _) in forbidden_in(other_scope) {
                        hits.push((token, why, format!("（经 `{}`）", other.name)));
                    }
                }
                if !hits.is_empty() {
                    let mut detail = String::new();
                    for (token, why, via) in hits {
                        detail.push_str(&format!("\n      · `{token}`{via} —— {why}"));
                    }
                    violations.push(format!(
                        "  {}:{}  fn {}{}",
                        file.display(),
                        line,
                        name,
                        detail
                    ));
                }
            }
        }
        from = brace + 1;
    }
    violations
}

fn forbidden_in(body: &[u8]) -> Vec<(&'static str, &'static str, String)> {
    let mut out = Vec::new();
    for (token, why) in FORBIDDEN {
        if contains(body, token.as_bytes()) {
            out.push((*token, *why, String::new()));
        }
    }
    out
}

/// 收集文件里所有 `fn`（含 async），带函数体字节区间
fn collect_fns(clean: &[u8]) -> Vec<FnDef> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = find(&clean[from..], b"fn ") {
        let start = from + rel;
        // `fn ` 前面必须是行首/空白/`>`/`&`，避免命中 `pub fn` 里的部分（其实是同一个）
        let prev = if start == 0 { b'\n' } else { clean[start - 1] };
        if !(prev == b'\n' || prev == b' ' || prev == b'\t' || prev == b'>') {
            from = start + 3;
            continue;
        }
        let Some(paren_rel) = clean[start..].iter().position(|&c| c == b'(') else {
            break;
        };
        let paren = start + paren_rel;
        if paren <= start + 3 {
            from = start + 3;
            continue;
        }
        let mut name = String::from_utf8_lossy(&clean[start + 3..paren]).trim().to_string();
        // 泛型参数 `fn foo<T>(` 之类的极少见，去掉 `<...>`
        if let Some(idx) = name.find('<') {
            name.truncate(idx);
        }
        if name.is_empty() || name.contains(' ') {
            from = start + 3;
            continue;
        }
        let Some(brace_rel) = clean[paren..].iter().position(|&c| c == b'{' || c == b';') else {
            break;
        };
        let brace = paren + brace_rel;
        if clean[brace] == b';' {
            from = brace + 1;
            continue; // trait 方法声明，无函数体
        }
        let end = match_body(clean, brace);
        // async 判定：往回找最近的 `fn` 之前的 12 字节内是否含 async
        let head_start = start.saturating_sub(24);
        let is_async = contains(&clean[head_start..start], b"async");
        out.push(FnDef { name, is_async, body_start: brace, body_end: end });
        from = brace + 1;
    }
    out
}

fn match_body(clean: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    for (offset, byte) in clean.iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return offset;
                }
            }
            _ => {}
        }
    }
    clean.len().saturating_sub(1)
}

fn fn_name(head: &[u8]) -> Option<String> {
    let rel = find(head, b"fn ")?;
    let rest = &head[rel + 3..];
    let paren = rest.iter().position(|&c| c == b'(')?;
    let mut name = String::from_utf8_lossy(&rest[..paren]).trim().to_string();
    if let Some(idx) = name.find('<') {
        name.truncate(idx);
    }
    Some(name)
}

fn line_of(clean: &[u8], offset: usize) -> usize {
    1 + clean[..offset.min(clean.len())].iter().filter(|&&c| c == b'\n').count()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

/// 取原始文本的某个字节区间（只用于找 `// blocking-ok`，不会切坏 UTF-8 之外的东西）
fn slice_str(text: &str, start: usize, end: usize) -> String {
    let bytes = text.as_bytes();
    let end = end.min(bytes.len().saturating_sub(1));
    if start >= end {
        return String::new();
    }
    String::from_utf8_lossy(&bytes[start..=end]).to_string()
}

/// 把注释与字符串/字符字面量内容抹成空格（保留字节偏移与换行）。
///
/// 目的：`format!("{error}")` 这种字符串里的花括号、以及注释里的 `fn ` / `{`
/// 不能干扰结构扫描。非 ASCII 字节（中文）原样保留 —— 它的每个字节都 >= 0x80，
/// 不会和任何 ASCII token 撞上。
fn sanitize(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                out[i] = b' ';
                out[i + 1] = b' ';
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    if bytes[i] != b'\n' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
                if i + 1 < bytes.len() {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    i += 2;
                }
            }
            b'"' => {
                out[i] = b' ';
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        out[i] = b' ';
                        if i + 1 < bytes.len() && bytes[i + 1] != b'\n' {
                            out[i + 1] = b' ';
                        }
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        out[i] = b' ';
                        i += 1;
                        break;
                    }
                    if bytes[i] != b'\n' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
            }
            // 字符字面量（'x' / '\n'）；生命周期（'a）保持原样
            b'\'' if i + 2 < bytes.len()
                && (bytes[i + 2] == b'\''
                    || (bytes[i + 1] == b'\\' && i + 3 < bytes.len() && bytes[i + 3] == b'\'')) =>
            {
                let end = if bytes[i + 1] == b'\\' { i + 4 } else { i + 3 };
                for byte in out.iter_mut().take(end.min(bytes.len())).skip(i) {
                    if *byte != b'\n' {
                        *byte = b' ';
                    }
                }
                i = end;
            }
            _ => i += 1,
        }
    }
    out
}

// ==================== 守卫自身的单测 ====================

#[test]
fn sanitize_ignores_braces_in_strings_and_comments() {
    let src = r#"
fn a() {
    let s = format!("{error}");
    // 这里有个 } 注释
    /* 这里也有 { */
}
fn b() { }
"#;
    let clean = sanitize(src);
    let defs = collect_fns(&clean);
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b"], "函数体配对被字符串/注释里的括号带偏了");
}

#[test]
fn detects_blocking_inside_sync_command() {
    let src = r#"
#[tauri::command]
pub fn bad(app: AppHandle) -> Result<(), String> {
    let mut cmd = std::process::Command::new("ssh");
    cmd.output().map(|_| ()).map_err(|e| e.to_string())
}
"#;
    let hits = scan_file(Path::new("mem.rs"), src);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].contains("fn bad"), "{hits:?}");
    assert!(hits[0].contains("Command::new"), "{hits:?}");
}

#[test]
fn allows_async_command_and_escape_hatch() {
    let src = r#"
#[tauri::command]
pub async fn good(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _ = std::process::Command::new("ssh");
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn excused() -> Result<(), String> {
    // blocking-ok: 只是拿一下系统默认目录，微秒级
    let _ = std::env::temp_dir();
    Ok(())
}
"#;
    let hits = scan_file(Path::new("mem.rs"), src);
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn detects_blocking_one_hop_away() {
    let src = r#"
#[tauri::command]
pub fn wrapper(app: AppHandle) -> Result<(), String> {
    inner(app).map(|_| ())
}

fn inner(app: AppHandle) -> Result<String, String> {
    let out = std::process::Command::new("ls").output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}
"#;
    let hits = scan_file(Path::new("mem.rs"), src);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].contains("fn wrapper"), "{hits:?}");
    assert!(hits[0].contains("inner"), "{hits:?}");
}
