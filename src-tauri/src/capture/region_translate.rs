//! 截图原位翻译：选区里识别出文字块 → 整批翻译 → 把每块的位置和译文交回遮罩，
//! 由前端在原位置铺底色、写译文（颜色从原图取，见前端 `translateLayer.ts`）。
//!
//! 分块不直接用识字窗口的段落重组（`reflow`）：那边为了复制成文本，会把同一水平线上
//! 左右两栏的字拼成一行（中间用制表符隔开）。原位翻译要的是"版面上的一块块文字"，
//! 所以这里自己分：先按行聚，行里隔得远的字拆开，再把上下挨着、左边对齐、字号相近的
//! 拼成一块；字的颜色差得多的不拼（蓝色标题、灰色作者行、绿色日期各是各的）。
//!
//! 免费翻译源有 3 秒一次的限流，一块一块翻会很慢。所以所有块用空行连起来一次发出去，
//! 回来再按空行拆开；拆出来的数目对不上（个别服务会吞掉空行）就退回按单个换行拆，
//! 还对不上才一块一块补翻（最多补几块，免得等太久）。

use image::RgbaImage;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::error::{AppError, AppResult};
use crate::imaging;
use crate::ocr::{
    self,
    reflow::{aabb, char_width, needs_space, vertical_overlap, OcrBlock, Rect},
};
use crate::platform::{MonitorId, PhysicalRect};
use crate::state::state;
use crate::translate::{self, TranslateRequest};

/// 拆不开时最多一块一块补翻几块
const MAX_SINGLE_RETRIES: usize = 6;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionRequest {
    pub session_id: u64,
    pub monitor_id: MonitorId,
    /// 本屏局部物理坐标
    pub rect: PhysicalRect,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslatedBlock {
    /// 文字块的包围盒（本屏局部物理坐标）
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// 原文一行的高度（中位数），前端据此定字号
    pub line_height: f64,
    pub source: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionTranslation {
    pub blocks: Vec<TranslatedBlock>,
    pub provider: String,
    pub from: String,
    pub to: String,
}

/// 版面上的一块文字（若干行）。
#[derive(Debug)]
struct TextBlock {
    rect: Rect,
    /// 每行一条：(包围盒, 文字)
    lines: Vec<(Rect, String)>,
    /// 最后一行的字色
    color: Option<[f64; 3]>,
}

fn color_dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// 一行字的颜色：框里出现最多的颜色当底色，离底色最远的那批像素的平均当字色。
fn text_color(img: &RgbaImage, r: &Rect) -> Option<[f64; 3]> {
    let (w, h) = img.dimensions();
    let x0 = r.x.max(0.0) as u32;
    let y0 = r.y.max(0.0) as u32;
    let x1 = (r.right().ceil() as u32).min(w);
    let y1 = (r.bottom().ceil() as u32).min(h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let step = ((x1 - x0).min(y1 - y0) / 16).max(1);
    let mut px: Vec<[f64; 3]> = Vec::new();
    let mut counts = std::collections::HashMap::<u32, usize>::new();
    for y in (y0..y1).step_by(step as usize) {
        for x in (x0..x1).step_by(step as usize) {
            let p = img.get_pixel(x, y).0;
            px.push([f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]);
            let key =
                (u32::from(p[0]) >> 4) << 8 | (u32::from(p[1]) >> 4) << 4 | u32::from(p[2]) >> 4;
            *counts.entry(key).or_default() += 1;
        }
    }
    let (&bg_key, _) = counts.iter().max_by_key(|(_, n)| **n)?;
    let in_bucket = |c: &[f64; 3]| {
        let k = (c[0] as u32 >> 4) << 8 | (c[1] as u32 >> 4) << 4 | c[2] as u32 >> 4;
        k == bg_key
    };
    let bg_px: Vec<&[f64; 3]> = px.iter().filter(|c| in_bucket(c)).collect();
    let n = bg_px.len().max(1) as f64;
    let bg = bg_px.iter().fold([0.0; 3], |a, c| {
        [a[0] + c[0] / n, a[1] + c[1] / n, a[2] + c[2] / n]
    });
    let far = px.iter().map(|c| color_dist(*c, bg)).fold(0.0, f64::max);
    if far < 48.0 {
        return None;
    }
    let fg: Vec<&[f64; 3]> = px
        .iter()
        .filter(|c| color_dist(**c, bg) >= far * 0.62)
        .collect();
    let n = fg.len().max(1) as f64;
    Some(fg.iter().fold([0.0; 3], |a, c| {
        [a[0] + c[0] / n, a[1] + c[1] / n, a[2] + c[2] / n]
    }))
}

impl TextBlock {
    fn text(&self) -> String {
        let mut out = String::new();
        for (_, t) in &self.lines {
            if !out.is_empty() && needs_space(&out, t) {
                out.push(' ');
            }
            out.push_str(t);
        }
        out
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// 东亚全角字：排版时大约占两个西文字的宽度
fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6)
}

/// 行首行尾像是图标被认成的字：汉字（停止按钮的方块常被认成"口""回"）和非句读的符号（`>` `×` `□`）
fn icon_like(c: char, at_end: bool) -> bool {
    if is_wide(c) {
        return true;
    }
    if c.is_alphanumeric() || c.is_whitespace() {
        return false;
    }
    // 句末标点、成对符号的后半个是正文的一部分；行首的引号、括号也是
    let keep_end = ".,!?:;)]}'\"%…";
    let keep_start = "([{'\"#@$";
    !(if at_end { keep_end } else { keep_start }).contains(c)
}

/// 西文行首行尾混进来的图标（按钮、箭头被识别成"口"">"之类）去掉，框也按去掉的字宽缩回去，
/// 免得被当成正文拿去翻译、译文底色把按钮盖住。只管主要是西文的行：中文行里的"口"分不清真假。
fn strip_icon_glyphs(text: &str, rect: &mut Rect) -> String {
    let alnum: Vec<char> = text.chars().filter(|c| c.is_alphanumeric()).collect();
    let latin = alnum.iter().filter(|c| c.is_ascii_alphanumeric()).count();
    if alnum.is_empty() || (latin as f64) < alnum.len() as f64 * 0.7 {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    // 一个图标最多被认成一两个汉字；再多就是正文里真的夹了中文（"Hello 世界"），不动
    let strip = |c: char, at_end: bool, wide: &mut u32| {
        if c.is_whitespace() || !icon_like(c, at_end) {
            return c.is_whitespace();
        }
        if is_wide(c) {
            *wide += 1;
        }
        *wide <= 2
    };
    let (mut start, mut wide) = (0, 0);
    while start < chars.len() && strip(chars[start], false, &mut wide) {
        start += 1;
    }
    if wide > 2 {
        start = 0;
    }
    let (mut end, mut wide) = (chars.len(), 0);
    while end > start && strip(chars[end - 1], true, &mut wide) {
        end -= 1;
    }
    if wide > 2 {
        end = chars.len();
    }
    if start == 0 && end == chars.len() {
        return text.to_string();
    }
    let width = |cs: &[char]| {
        cs.iter()
            .map(|&c| if is_wide(c) { 2.0 } else { 1.0 })
            .sum::<f64>()
    };
    let total = width(&chars).max(1.0);
    let lead = width(&chars[..start]) / total;
    let kept = width(&chars[start..end]) / total;
    rect.x += rect.w * lead;
    rect.w *= kept;
    chars[start..end].iter().collect()
}

/// 识别出的零散文字框 → 版面上的文字块。`image` 用来比较字色（单元测试里不给）。
fn layout_blocks(blocks: &[OcrBlock], image: Option<&RgbaImage>) -> Vec<TextBlock> {
    let mut rects: Vec<Rect> = blocks.iter().map(|b| aabb(&b.quad)).collect();
    let texts: Vec<String> = blocks
        .iter()
        .zip(rects.iter_mut())
        .map(|(b, r)| strip_icon_glyphs(&b.text, r))
        .collect();
    let mut order: Vec<usize> = (0..blocks.len())
        .filter(|&i| !texts[i].trim().is_empty())
        .collect();
    order.sort_by(|&a, &b| rects[a].y.total_cmp(&rects[b].y));

    // 1. 按行聚（和 reflow 一样：垂直重叠过半算同一行）
    let mut rows: Vec<(Rect, Vec<usize>)> = Vec::new();
    for i in order {
        let r = rects[i];
        match rows
            .iter_mut()
            .rev()
            .take(4)
            .find(|(lr, _)| vertical_overlap(lr, &r) > 0.5)
        {
            Some((lr, members)) => {
                *lr = lr.union(&r);
                members.push(i);
            }
            None => rows.push((r, vec![i])),
        }
    }

    // 2. 行里隔得远的拆开：两栏、表格的不同格子不是同一句话
    let mut segments: Vec<(Rect, String)> = Vec::new();
    for (_, mut members) in rows {
        members.sort_by(|&a, &b| rects[a].x.total_cmp(&rects[b].x));
        let mut cur: Option<(Rect, String)> = None;
        for &i in &members {
            let r = rects[i];
            let t = texts[i].trim();
            if let Some((cr, ct)) = cur.as_mut() {
                let gap = r.x - cr.right();
                let far = gap >= (char_width(ct, cr) * 2.5).max(cr.h * 1.2);
                if !far {
                    if gap >= char_width(ct, cr) * 0.3 && needs_space(ct, t) {
                        ct.push(' ');
                    }
                    ct.push_str(t);
                    *cr = cr.union(&r);
                    continue;
                }
                segments.push(cur.take().unwrap_or_default());
            }
            cur = Some((r, t.to_string()));
        }
        segments.extend(cur);
    }
    segments.sort_by(|a, b| a.0.y.total_cmp(&b.0.y).then(a.0.x.total_cmp(&b.0.x)));

    // 3. 上下挨着、左边对齐（或大半重叠）、字号相近、上一行写满了 → 同一块
    let mut out: Vec<TextBlock> = Vec::new();
    for (r, t) in segments {
        let color = image.and_then(|img| text_color(img, &r));
        let target = out.iter_mut().rev().take(8).find(|b| {
            let Some((last, last_text)) = b.lines.last() else {
                return false;
            };
            let h = last.h;
            let below = r.y >= last.y + h * 0.5 && r.y - last.bottom() <= h * 0.9;
            let similar = (r.h / h).clamp(0.0, 10.0) > 0.75 && r.h / h < 1.33;
            let overlap = (r.right().min(last.right()) - r.x.max(last.x)).max(0.0);
            let aligned = (r.x - last.x).abs() < char_width(last_text, last) * 3.0
                || overlap >= r.w.min(last.w) * 0.5;
            // 上一行没写满就换行了 → 那是段落结尾（比如标题下面接正文）
            let filled = b.rect.right().max(r.right()) - last.right() < h * 2.5;
            let same_color = match (b.color, color) {
                (Some(a), Some(c)) => color_dist(a, c) < 70.0,
                _ => true,
            };
            // 语言不同的不拼：英文标题下面一行"Bash 已完成 12分钟"拼进去，整段被翻译服务当成中文原样退回，
            // 标题也跟着没翻
            let same_lang = translate::detect::detect(last_text) == translate::detect::detect(&t);
            below && similar && aligned && filled && same_color && same_lang
        });
        match target {
            Some(b) => {
                b.rect = b.rect.union(&r);
                b.lines.push((r, t));
                b.color = color.or(b.color);
            }
            None => out.push(TextBlock {
                rect: r,
                lines: vec![(r, t)],
                color,
            }),
        }
    }
    // 只有一个字母 / 字的（关闭按钮的"x"、图标认成的"口"）不是要翻的正文
    out.retain(|b| b.text().chars().filter(|c| c.is_alphanumeric()).count() >= 2);
    out
}

/// 检测出的语言和目标语言算不算同一种（简繁都算中文）。
fn same_language(detected: &str, target: &str) -> bool {
    let base = |l: &str| l.split(['-', '_']).next().unwrap_or(l).to_ascii_lowercase();
    base(detected) == base(target)
}

/// 把整批译文拆回各块。拆不对返回 None。
fn split_back(text: &str, n: usize) -> Option<Vec<String>> {
    let by_blank: Vec<String> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    if by_blank.len() == n {
        return Some(by_blank);
    }
    let by_line: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    (by_line.len() == n).then_some(by_line)
}

pub async fn translate_region(app: &AppHandle, req: RegionRequest) -> AppResult<RegionTranslation> {
    if req.rect.is_empty() {
        return Err(AppError::msg("选区为空"));
    }
    let image = {
        let session = state(app)
            .capture
            .current()
            .filter(|s| s.id == req.session_id)
            .ok_or_else(|| AppError::msg("截图会话已结束"))?;
        let shot = session
            .monitor(req.monitor_id)
            .ok_or_else(|| AppError::msg("显示器不存在"))?;
        imaging::crop_opaque(&shot.image, req.rect)?
    };
    let ocr_app = app.clone();
    let (result, image) = tauri::async_runtime::spawn_blocking(move || {
        ocr::recognize(&ocr_app, &image, None, false).map(|r| (r, image))
    })
    .await
    .map_err(|e| AppError::msg(e.to_string()))??;
    let mut blocks = layout_blocks(&result.blocks, Some(&image));
    if blocks.is_empty() {
        return Err(AppError::msg("选区里没有识别到文字"));
    }
    // 翻成哪种语言：设置里指定了就按设置；"自动"时翻成界面语言。不能按整块选区检测
    // （"中 ↔ 英"）：界面、网页里常常中英混排，中文一多整批就被当成中文、翻成英文，
    // 英文那几块原样不动，看着像没翻
    let target = {
        let configured = state(app).settings.read().translate.target_language.clone();
        if configured != "auto" {
            configured
        } else if crate::i18n::english() {
            "en".to_string()
        } else {
            "zh".to_string()
        }
    };
    // 本来就是目标语言的块不翻也不重画：重画只会把识别丢了空格、认错字的原文盖在原图上
    blocks.retain(|b| !same_language(translate::detect::detect(&b.text()), &target));
    if blocks.is_empty() {
        return Err(AppError::msg("选区里的文字已经是目标语言"));
    }
    let sources: Vec<String> = blocks.iter().map(TextBlock::text).collect();

    let whole = translate::translate(
        app,
        TranslateRequest {
            text: sources.join(
                "

",
            ),
            to: Some(target.clone()),
            ..Default::default()
        },
    )
    .await?;
    let texts = match split_back(&whole.text, sources.len()) {
        Some(t) => t,
        None if sources.len() == 1 => vec![whole.text.trim().to_string()],
        None => {
            tracing::info!(blocks = sources.len(), "整批译文拆不回各块，改为逐块翻译");
            let mut out = Vec::with_capacity(sources.len());
            for (i, src) in sources.iter().enumerate() {
                if i >= MAX_SINGLE_RETRIES {
                    out.push(src.clone());
                    continue;
                }
                let r = translate::translate(
                    app,
                    TranslateRequest {
                        text: src.clone(),
                        to: Some(target.clone()),
                        ..Default::default()
                    },
                )
                .await;
                out.push(r.map(|r| r.text).unwrap_or_else(|_| src.clone()));
            }
            out
        }
    };

    let ox = f64::from(req.rect.x);
    let oy = f64::from(req.rect.y);
    let out = blocks
        .iter()
        .zip(sources)
        .zip(texts)
        .map(|((b, source), text)| TranslatedBlock {
            x: b.rect.x + ox,
            y: b.rect.y + oy,
            width: b.rect.w,
            height: b.rect.h,
            line_height: median(b.lines.iter().map(|(r, _)| r.h).collect()),
            source,
            text,
        })
        .collect();
    Ok(RegionTranslation {
        blocks: out,
        provider: whole.provider,
        from: whole.from,
        to: whole.to,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str, x: f64, y: f64, w: f64, h: f64) -> OcrBlock {
        OcrBlock {
            text: text.into(),
            quad: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            score: 0.9,
        }
    }

    #[test]
    fn icons_at_the_ends_of_latin_lines_are_dropped() {
        let mut r = Rect {
            x: 0.0,
            y: 0.0,
            w: 300.0,
            h: 20.0,
        };
        assert_eq!(
            strip_icon_glyphs("Wait for user capture session >口", &mut r),
            "Wait for user capture session"
        );
        assert!(r.w < 300.0 && r.x == 0.0);
        let mut r = Rect {
            x: 0.0,
            y: 0.0,
            w: 300.0,
            h: 20.0,
        };
        assert_eq!(
            strip_icon_glyphs("口 Start dev app with WebView2口", &mut r),
            "Start dev app with WebView2"
        );
        assert!(r.x > 0.0);
        // 正常的句末标点、中文行都不动
        let mut r = Rect {
            x: 0.0,
            y: 0.0,
            w: 300.0,
            h: 20.0,
        };
        assert_eq!(
            strip_icon_glyphs("It works (mostly).", &mut r),
            "It works (mostly)."
        );
        assert_eq!(strip_icon_glyphs("后台任务 口", &mut r), "后台任务 口");
        assert_eq!(strip_icon_glyphs("Say hello to everyone in 你好世界", &mut r), "Say hello to everyone in 你好世界");
        assert_eq!(strip_icon_glyphs("你好世界 means hello world in Chinese", &mut r), "你好世界 means hello world in Chinese");
        assert_eq!(r.w, 300.0);
    }

    #[test]
    fn lines_in_different_languages_stay_apart() {
        // 用户截图里识别出来的四行：英文标题，下面一行中文的状态
        let blocks = vec![
            block("Wait for dev apprebuilt afteredit", 42.0, 20.0, 304.0, 25.0),
            block("Bash已完成12分钟17秒", 41.0, 53.0, 227.0, 25.0),
            block("Run clippy in separate target dir", 40.0, 114.0, 295.0, 29.0),
            block("Bash已完成4分钟15秒", 40.0, 146.0, 217.0, 25.0),
        ];
        let texts: Vec<String> = layout_blocks(&blocks, None).iter().map(TextBlock::text).collect();
        assert_eq!(
            texts,
            vec![
                "Wait for dev apprebuilt afteredit",
                "Bash已完成12分钟17秒",
                "Run clippy in separate target dir",
                "Bash已完成4分钟15秒",
            ]
        );
    }

    #[test]
    fn same_language_ignores_region_suffix() {
        assert!(same_language("zh", "zh"));
        assert!(same_language("zh", "zh-TW"));
        assert!(!same_language("en", "zh"));
    }

    #[test]
    fn splits_back_by_blank_lines_then_single_lines() {
        assert_eq!(
            split_back("甲\n\n乙\n\n丙", 3).unwrap(),
            vec!["甲", "乙", "丙"]
        );
        // 服务把空行吞了
        assert_eq!(split_back("甲\n乙\n丙", 3).unwrap(), vec!["甲", "乙", "丙"]);
        assert!(split_back("甲乙丙", 3).is_none());
    }

    #[test]
    fn two_columns_on_the_same_line_stay_apart() {
        // 左栏两行一段，右栏两行一段，两栏第一行在同一水平线上
        let blocks = vec![
            block(
                "Pheochromocytoma Across the Course: Current",
                0.0,
                0.0,
                600.0,
                30.0,
            ),
            block("Article Type Filters Updated to", 800.0, 2.0, 500.0, 30.0),
            block("Concepts and Clinical Management.", 0.0, 36.0, 420.0, 30.0),
            block("Publication Types", 800.0, 38.0, 220.0, 30.0),
        ];
        let out = layout_blocks(&blocks, None);
        let texts: Vec<String> = out.iter().map(TextBlock::text).collect();
        assert_eq!(
            texts,
            vec![
                "Pheochromocytoma Across the Course: Current Concepts and Clinical Management.",
                "Article Type Filters Updated to Publication Types",
            ]
        );
    }

    #[test]
    fn lines_in_different_colors_stay_apart() {
        // 白底：第一行蓝字（链接标题），第二行灰字（作者），位置、字号都能拼
        let mut img = RgbaImage::from_pixel(720, 80, image::Rgba([255, 255, 255, 255]));
        let mut ink = |y0: u32, color: [u8; 3]| {
            for y in y0 + 6..y0 + 24 {
                for x in (4..700).filter(|x| x % 3 == 0) {
                    img.put_pixel(x, y, image::Rgba([color[0], color[1], color[2], 255]));
                }
            }
        };
        ink(0, [20, 90, 220]);
        ink(34, [90, 90, 90]);
        let blocks = vec![
            block(
                "Clinical guidelines for treating caries in adults following a",
                0.0,
                0.0,
                700.0,
                30.0,
            ),
            block(
                "Momoi Y, et al. J Dent. 2012. PMID: 22079371",
                0.0,
                34.0,
                700.0,
                30.0,
            ),
        ];
        assert_eq!(layout_blocks(&blocks, None).len(), 1);
        assert_eq!(layout_blocks(&blocks, Some(&img)).len(), 2);
    }

    #[test]
    fn short_line_ends_a_block_and_smaller_text_starts_another() {
        let blocks = vec![
            block("March 31, 2026", 0.0, 0.0, 180.0, 26.0),
            block(
                "The article type filters in PubMed have been updated",
                0.0,
                34.0,
                700.0,
                26.0,
            ),
            block("MeSH Processing (AMP) updates", 0.0, 66.0, 400.0, 26.0),
        ];
        let out = layout_blocks(&blocks, None);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text(), "March 31, 2026");
    }
}
