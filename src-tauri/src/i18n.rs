//! Rust 端 i18n（R0 基础设施）。
//!
//! PC 桌面 UI 由前端（ui-vue）承载、已 100% i18n；Rust 这边只有**少量系统级文案**需要本地化：
//! 托盘菜单、系统通知、HTTP API 返回的 `message`/`note`、`/api/status`。
//!
//! 语言标签与前端/移动端一致：zh-CN/zh-TW/en-US/ja-JP/ko-KR/de-DE/ru-RU/pt-BR/es-ES。
//! 这里用编译期 `t!` 宏把 9 种译文内联进 crate，无运行时 IO。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    ZhCn,
    ZhTw,
    En,
    Ja,
    Ko,
    De,
    Ru,
    PtBr,
    Es,
}

impl Lang {
    /// 从 BCP-47 标签（大小写不敏感，取主语言）解析；未知回落到中文。
    pub fn from_tag(tag: &str) -> Lang {
        let t = tag.to_ascii_lowercase();
        if t.starts_with("zh") {
            if t.contains("tw") || t.contains("hant") || t.contains("hk") {
                Lang::ZhTw
            } else {
                Lang::ZhCn
            }
        } else if t.starts_with("en") {
            Lang::En
        } else if t.starts_with("ja") {
            Lang::Ja
        } else if t.starts_with("ko") {
            Lang::Ko
        } else if t.starts_with("de") {
            Lang::De
        } else if t.starts_with("ru") {
            Lang::Ru
        } else if t.starts_with("pt") {
            Lang::PtBr
        } else if t.starts_with("es") {
            Lang::Es
        } else {
            Lang::ZhCn
        }
    }

    /// 解析 `Accept-Language` 头（取第一个 q>0 的标签）。
    pub fn from_accept_language(header: Option<&str>) -> Lang {
        let Some(h) = header else { return Lang::ZhCn };
        // 取第一个标签；忽略 q 值权重（够用）
        let first = h.split(',').next().unwrap_or("").split(';').next().unwrap_or("");
        Lang::from_tag(first.trim())
    }
}

/// 从 9 元组中按语言取一条。
pub fn pick(lang: Lang, v: [&'static str; 9]) -> &'static str {
    match lang {
        Lang::ZhCn => v[0],
        Lang::ZhTw => v[1],
        Lang::En => v[2],
        Lang::Ja => v[3],
        Lang::Ko => v[4],
        Lang::De => v[5],
        Lang::Ru => v[6],
        Lang::PtBr => v[7],
        Lang::Es => v[8],
    }
}

/// `t!(lang, zh, tw, en, ja, ko, de, ru, pt, es)` → 当前语言的静态串。
#[macro_export]
macro_rules! t {
    ($lang:expr, $zh:expr, $tw:expr, $en:expr, $ja:expr, $ko:expr, $de:expr, $ru:expr, $pt:expr, $es:expr) => {
        $crate::i18n::pick($lang, [$zh, $tw, $en, $ja, $ko, $de, $ru, $pt, $es])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tags() {
        assert_eq!(Lang::from_tag("zh-CN"), Lang::ZhCn);
        assert_eq!(Lang::from_tag("zh-TW"), Lang::ZhTw);
        assert_eq!(Lang::from_tag("en-US"), Lang::En);
        assert_eq!(Lang::from_tag("pt-BR"), Lang::PtBr);
        assert_eq!(Lang::from_tag("fr"), Lang::ZhCn);
    }

    #[test]
    fn accept_language() {
        assert_eq!(Lang::from_accept_language(Some("en-US,en;q=0.9")), Lang::En);
        assert_eq!(Lang::from_accept_language(Some("ja")), Lang::Ja);
        assert_eq!(Lang::from_accept_language(None), Lang::ZhCn);
    }
}
