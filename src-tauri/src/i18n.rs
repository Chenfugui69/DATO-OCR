//! 后端文案的语言。
//!
//! 界面上的字都在前端的 i18n 里；后端自己产生的那些（提示条、报错、托盘菜单、窗口标题）源码里直接写的
//! 中文。英文界面下，在它们送到用户眼前的那一刻（`AppError` 序列化、`wm::toast`、建菜单、建窗口）
//! 用 `localize` 照 `i18n_en.rs` 的表换成英文：带占位的句子（"截图失败：{err}"）按模板匹配，
//! 占位里的内容再递归换一遍（错误常常一层套一层）。表里没有的原样返回，所以漏翻只是那一句还是中文。

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crate::i18n_en::TABLE;

static ENGLISH: AtomicBool = AtomicBool::new(false);

/// 设置里的界面语言（`system` | `zh-CN` | `en-US`）。启动时和改设置时调用。
pub fn set_language(language: &str) {
    ENGLISH.store(resolve(language) == "en-US", Ordering::Relaxed);
}

/// 设置值 → 实际用的语言。「跟随系统」：系统是中文就用中文，其余一律英文。
pub fn resolve(language: &str) -> &'static str {
    let chinese = match language {
        "zh-CN" => true,
        "en-US" => false,
        _ => sys_locale::get_locale().is_none_or(|l| l.to_ascii_lowercase().starts_with("zh")),
    };
    if chinese {
        "zh-CN"
    } else {
        "en-US"
    }
}

/// 现在是不是英文界面。
pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// 把一句后端文案换成当前界面语言。
pub fn localize(text: &str) -> Cow<'_, str> {
    if !ENGLISH.load(Ordering::Relaxed) {
        return Cow::Borrowed(text);
    }
    match translate(text, 0) {
        Some(en) => Cow::Owned(en),
        None => Cow::Borrowed(text),
    }
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

fn translate(text: &str, depth: u8) -> Option<String> {
    if depth > 4 || !has_cjk(text) {
        return None;
    }
    let catalog = catalog();
    if let Some(en) = catalog.exact.get(text) {
        return Some((*en).to_string());
    }
    for template in &catalog.templates {
        let Some(holes) = capture(text, &template.parts) else {
            continue;
        };
        let mut out = String::with_capacity(template.en.len() + text.len());
        let mut holes = holes.into_iter();
        for (i, piece) in template.en.split("{}").enumerate() {
            if i > 0 {
                let hole = holes.next().unwrap_or("");
                match translate(hole, depth + 1) {
                    Some(en) => out.push_str(&en),
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
        assert_eq!(translate("已保存", 0).as_deref(), Some("Saved"));
        assert_eq!(
            translate("截图失败：抓屏失败：系统没有返回画面", 0).as_deref(),
            Some("Capture failed: Screen capture failed: The system returned no image")
        );
        assert_eq!(
            translate("上传失败：HTTP 507", 0).as_deref(),
            Some("Upload failed: HTTP 507")
        );
        // 占位里不是表里的句子（系统给的英文、路径）：原样留着
        assert_eq!(
            translate("文件读写失败：No such file or directory (os error 2)", 0).as_deref(),
            Some("File error: No such file or directory (os error 2)")
        );
        assert_eq!(translate("表里没有的一句话", 0), None);
        assert_eq!(translate("already English", 0), None);
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
