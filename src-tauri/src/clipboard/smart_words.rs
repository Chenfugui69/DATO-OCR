//! 智能推荐本机打分用的提示词：上下文里出现这些词，说明要贴的多半是哪一类。
//!
//! 这里的中文是用来匹配窗口文字的关键词，不是界面文案（`scripts/backend-strings.mjs` 跳过这个文件）。

/// 上下文里提到了要贴哪类东西（"附上链接""这是截图"）
pub fn wanted_kind(ctx_lower: &str) -> Option<&'static str> {
    let has = |words: &[&str]| words.iter().any(|w| ctx_lower.contains(w));
    if has(&[
        "链接",
        "网址",
        "地址",
        "link",
        "url",
        "here's the",
        "here is the",
    ]) {
        Some("link")
    } else if has(&[
        "图片",
        "截图",
        "照片",
        "image",
        "photo",
        "screenshot",
        "picture",
    ]) {
        Some("image")
    } else if has(&["附件", "文件", "attach", "file"]) {
        Some("files")
    } else {
        None
    }
}
