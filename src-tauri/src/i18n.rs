//! 后端文案的语言。
//!
//! 界面上的字都在前端的 i18n 里；后端自己产生的那些（提示条、报错、托盘菜单、窗口标题）源码里直接写的
//! 中文。别的界面语言下，在它们送到用户眼前的那一刻（`AppError` 序列化、`wm::toast`、建菜单、建窗口）
//! 用 `localize` 照 `i18n_en.rs` 的表换掉：带占位的句子（"截图失败：{err}"）按模板匹配，
//! 占位里的内容再递归换一遍（错误常常一层套一层）。表里没有的原样返回，所以漏翻只是那一句还是中文。
//!
//! 界面语言：简体中文、繁体中文、英文、日文、韩文。后端只有英文一张译表：日文、韩文界面的提示条用英文；
//! 繁体界面把认出来的句子转成繁体（`i18n_tw.rs`）。

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use crate::i18n_en::TABLE;

/// 实际用的界面语言（`resolve` 之后的）
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ui {
    ZhCn = 0,
    En = 1,
    ZhTw = 2,
    Ja = 3,
    Ko = 4,
}

static LANG: AtomicU8 = AtomicU8::new(Ui::ZhCn as u8);

fn ui() -> Ui {
    match LANG.load(Ordering::Relaxed) {
        1 => Ui::En,
        2 => Ui::ZhTw,
        3 => Ui::Ja,
        4 => Ui::Ko,
        _ => Ui::ZhCn,
    }
}

/// 设置里的界面语言（`system` | `zh-CN` | `zh-TW` | `en-US` | `ja-JP` | `ko-KR`）。启动时和改设置时调用。
pub fn set_language(language: &str) {
    let ui = match resolve(language) {
        "en-US" => Ui::En,
        "zh-TW" => Ui::ZhTw,
        "ja-JP" => Ui::Ja,
        "ko-KR" => Ui::Ko,
        _ => Ui::ZhCn,
    };
    LANG.store(ui as u8, Ordering::Relaxed);
}

/// 设置值 → 实际用的语言。「跟随系统」：按系统的首选语言，不认识的语言一律英文
pub fn resolve(language: &str) -> &'static str {
    match language {
        "zh-CN" => "zh-CN",
        "zh-TW" => "zh-TW",
        "en-US" => "en-US",
        "ja-JP" => "ja-JP",
        "ko-KR" => "ko-KR",
        _ => sys_locale::get_locale().map_or("zh-CN", |l| from_locale(&l)),
    }
}

/// 系统语言（zh-Hans-CN、zh-TW、ja-JP……）→ 界面语言
fn from_locale(locale: &str) -> &'static str {
    let l = locale.to_ascii_lowercase().replace('_', "-");
    if l.starts_with("zh") {
        if l.contains("hant") || ["-tw", "-hk", "-mo"].iter().any(|s| l.ends_with(s)) {
            "zh-TW"
        } else {
            "zh-CN"
        }
    } else if l.starts_with("ja") {
        "ja-JP"
    } else if l.starts_with("ko") {
        "ko-KR"
    } else {
        "en-US"
    }
}

/// 现在是不是非中文界面（英文、日文、韩文）：AI 的默认提示词这些按英文给
pub fn english() -> bool {
    matches!(ui(), Ui::En | Ui::Ja | Ui::Ko)
}

/// 翻译成界面语言时用的目标语言代码（截图翻译"自动"时译成它）
pub fn target_language() -> &'static str {
    match ui() {
        Ui::ZhCn => "zh",
        Ui::ZhTw => "zh-TW",
        Ui::En => "en",
        Ui::Ja => "ja",
        Ui::Ko => "ko",
    }
}

/// 界面语言的英文名（给 AI 的提示里说"用哪种语言回答"）
pub fn language_name() -> &'static str {
    match ui() {
        Ui::ZhCn => "Simplified Chinese",
        Ui::ZhTw => "Traditional Chinese",
        Ui::En => "English",
        Ui::Ja => "Japanese",
        Ui::Ko => "Korean",
    }
}

/// 把一句后端文案换成当前界面语言。
pub fn localize(text: &str) -> Cow<'_, str> {
    let tw = match ui() {
        Ui::ZhCn => return Cow::Borrowed(text),
        Ui::ZhTw => true,
        Ui::En | Ui::Ja | Ui::Ko => false,
    };
    match translate(text, 0, tw) {
        Some(out) => Cow::Owned(out),
        None => Cow::Borrowed(text),
    }
}

/// 简体 → 繁体（台湾用语）：先整词，再逐字
fn to_traditional(text: &str) -> String {
    static CHARS: OnceLock<HashMap<char, char>> = OnceLock::new();
    let chars = CHARS.get_or_init(|| {
        crate::i18n_tw::SIMPLIFIED
            .chars()
            .zip(crate::i18n_tw::TRADITIONAL.chars())
            .collect()
    });
    let mut s = text.to_string();
    for (from, to) in crate::i18n_tw::PHRASES {
        if s.contains(from) {
            s = s.replace(from, to);
        }
    }
    s.chars().map(|c| chars.get(&c).copied().unwrap_or(c)).collect()
}

/// 同上，给要存进状态、发给界面的错误信息用。
pub fn text(message: impl std::fmt::Display) -> String {
    let message = message.to_string();
    localize(&message).into_owned()
}

struct Template {
    /// 占位之间的固定文字，比占位多一段（首尾可以是空串）
    parts: Vec<&'static str>,
    en: &'static str,
}

struct Catalog {
    exact: HashMap<&'static str, &'static str>,
    /// 固定文字长的排前面：更具体的模板先试
    templates: Vec<Template>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut exact = HashMap::new();
        let mut templates = Vec::new();
        for &(zh, en) in TABLE {
            if zh.contains("{}") {
                templates.push(Template {
                    parts: zh.split("{}").collect(),
                    en,
                });
            } else {
                exact.insert(zh, en);
            }
        }
        templates
            .sort_by_key(|t| std::cmp::Reverse(t.parts.iter().map(|p| p.len()).sum::<usize>()));
        Catalog { exact, templates }
    })
}

fn has_cjk(text: &str) -> bool {
    text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// 认出是表里的哪一句，换成英文（`tw` = false）或繁体（`tw` = true，固定的文字转繁体，占位里的照原样递归）
fn translate(text: &str, depth: u8, tw: bool) -> Option<String> {
    if depth > 4 || !has_cjk(text) {
        return None;
    }
    let catalog = catalog();
    if let Some(en) = catalog.exact.get(text) {
        return Some(if tw { to_traditional(text) } else { (*en).to_string() });
    }
    for template in &catalog.templates {
        let Some(holes) = capture(text, &template.parts) else {
            continue;
        };
        let pieces: Vec<Cow<'_, str>> = if tw {
            template.parts.iter().map(|p| Cow::Owned(to_traditional(p))).collect()
        } else {
            template.en.split("{}").map(Cow::Borrowed).collect()
        };
        let mut out = String::with_capacity(template.en.len() + text.len());
        let mut holes = holes.into_iter();
        for (i, piece) in pieces.iter().enumerate() {
            if i > 0 {
                let hole = holes.next().unwrap_or("");
                match translate(hole, depth + 1, tw) {
                    Some(t) => out.push_str(&t),
                    None => out.push_str(hole),
                }
            }
            out.push_str(piece);
        }
        return Some(out);
    }
    None
}

/// `text` 符合"固定文字 + 占位 + 固定文字…"的话，取出每个占位里的内容。
fn capture<'a>(text: &'a str, parts: &[&str]) -> Option<Vec<&'a str>> {
    let (first, rest) = parts.split_first()?;
    let (last, middle) = rest.split_last()?;
    let mut remaining = text.strip_prefix(first)?.strip_suffix(last)?;
    // 只有首尾两段固定文字都是空的模板（"{}"）没有意义，不收
    if first.is_empty() && last.is_empty() && middle.is_empty() {
        return None;
    }
    let mut holes = Vec::with_capacity(parts.len() - 1);
    for part in middle {
        let at = remaining.find(part)?;
        holes.push(&remaining[..at]);
        remaining = &remaining[at + part.len()..];
    }
    holes.push(remaining);
    Some(holes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_errors_are_translated_layer_by_layer() {
        assert_eq!(translate("已保存", 0, false).as_deref(), Some("Saved"));
        assert_eq!(
            translate("截图失败：抓屏失败：系统没有返回画面", 0, false).as_deref(),
            Some("Capture failed: Screen capture failed: The system returned no image")
        );
        assert_eq!(
            translate("上传失败：HTTP 507", 0, false).as_deref(),
            Some("Upload failed: HTTP 507")
        );
        // 占位里不是表里的句子（系统给的英文、路径）：原样留着
        assert_eq!(
            translate("文件读写失败：No such file or directory (os error 2)", 0, false).as_deref(),
            Some("File error: No such file or directory (os error 2)")
        );
        assert_eq!(translate("表里没有的一句话", 0, false), None);
        assert_eq!(translate("already English", 0, false), None);
    }

    #[test]
    fn traditional_converts_the_sentence_but_not_what_is_inside_it() {
        assert_eq!(translate("已保存", 0, true).as_deref(), Some("已儲存"));
        assert_eq!(
            translate("截图失败：抓屏失败：系统没有返回画面", 0, true).as_deref(),
            Some("截圖失敗：抓屏失敗：系統沒有返回畫面")
        );
        // 占位里的路径是用户自己的，不转
        let out = translate("文件读写失败：D:\\文件\\截图.png", 0, true).unwrap();
        assert!(out.ends_with("D:\\文件\\截图.png"), "{out}");
        assert_eq!(from_locale("zh-Hant-TW"), "zh-TW");
        assert_eq!(from_locale("zh_HK"), "zh-TW");
        assert_eq!(from_locale("zh-CN"), "zh-CN");
        assert_eq!(from_locale("ja-JP"), "ja-JP");
        assert_eq!(from_locale("ko-KR"), "ko-KR");
        assert_eq!(from_locale("fr-FR"), "en-US");
    }

    #[test]
    fn simplified_and_traditional_tables_line_up() {
        assert_eq!(
            crate::i18n_tw::SIMPLIFIED.chars().count(),
            crate::i18n_tw::TRADITIONAL.chars().count()
        );
    }

    #[test]
    fn every_entry_keeps_its_placeholders() {
        for (zh, en) in TABLE {
            assert_eq!(
                zh.matches("{}").count(),
                en.matches("{}").count(),
                "占位个数对不上：{zh}"
            );
        }
    }
}
