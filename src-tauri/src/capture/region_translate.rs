//! 截图原位翻译：选区里识别出文字块 → 整批翻译 → 把每块的位置和译文交回遮罩，
//! 由前端在原位置铺底色、写译文（颜色从原图取，见前端 `translateLayer.ts`）。
//!
//! 分块不直接用识字窗口的段落重组（`reflow`）：那边为了复制成文本，会把同一水平线上
//! 左右两栏的字拼成一行（中间用制表符隔开）。原位翻译要的是"版面上的一块块文字"，
//! 所以这里自己分：识字框先按图上的竖线、大空白切开（识字常把表格一整行认成一个框），
//! 再按行聚，行里隔得远的字拆开，再把上下挨着、左边对齐、字号相近、上一行像是一句话写到头
//! 折过来的拼成一块（表格一列里的短格子不拼）；字的颜色差得多的不拼（蓝色标题、灰色作者行、
//! 绿色日期各是各的）。只有数字的格子不翻也不重画。粗体的字译文也用粗体。
//!
//! 免费翻译源有 3 秒一次的限流，一块一块翻会很慢。所以所有块用空行连起来一次发出去，
//! 回来再按空行拆开；拆出来的数目对不上（个别服务会吞掉空行）就退回按单个换行拆，
//! 还对不上才一块一块补翻（最多补几块，免得等太久）。

use image::RgbaImage;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
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
    /// 原文是粗体（标题、表头）：译文也用粗体
    pub bold: bool,
    /// 按墨的上下范围估出来的字号（物理像素），0 = 估不出来（前端退回按行高算）
    pub font_size: f64,
    /// 原文在按钮里居中（四周被一块底色围着、左右留白差不多）：译文也居中排
    pub centered: bool,
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
    /// 笔画粗细 / 字高（各行的中位数；量不出来是 None），和底色深浅（深底上的字显得粗）
    stroke: Option<f64>,
    dark: bool,
    bold: bool,
    /// 各行估出来的字号
    fonts: Vec<f64>,
    centered: bool,
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

/// 框在图上的整数范围（裁到图里）；太小返回 None
fn bounds(img: &RgbaImage, r: &Rect) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = img.dimensions();
    let x0 = r.x.max(0.0) as u32;
    let y0 = r.y.max(0.0) as u32;
    let x1 = (r.right().ceil() as u32).min(w);
    let y1 = (r.bottom().ceil() as u32).min(h);
    (x1 > x0 + 2 && y1 > y0 + 2).then_some((x0, y0, x1, y1))
}

/// 框里哪些像素算"墨"（字、表格线）：离底色（框里最多的颜色）超过最远那个的一半。对比太弱返回 None
struct Ink {
    bg: [f64; 3],
    cut: f64,
}

impl Ink {
    fn of(img: &RgbaImage, r: &Rect) -> Option<Self> {
        let (x0, y0, x1, y1) = bounds(img, r)?;
        let mut counts = std::collections::HashMap::<u32, (usize, [f64; 3])>::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let p = img.get_pixel(x, y).0;
                let key = (u32::from(p[0]) >> 4) << 8 | (u32::from(p[1]) >> 4) << 4 | u32::from(p[2]) >> 4;
                let e = counts.entry(key).or_insert((0, [0.0; 3]));
                e.0 += 1;
                for (sum, &c) in e.1.iter_mut().zip(&p[..3]) {
                    *sum += f64::from(c);
                }
            }
        }
        let (n, sum) = counts.values().max_by_key(|(n, _)| *n)?;
        let bg = [sum[0] / *n as f64, sum[1] / *n as f64, sum[2] / *n as f64];
        let mut far: f64 = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                far = far.max(color_dist(px(img, x, y), bg));
            }
        }
        (far >= 48.0).then_some(Self { bg, cut: far * 0.5 })
    }

    fn at(&self, img: &RgbaImage, x: u32, y: u32) -> bool {
        color_dist(px(img, x, y), self.bg) > self.cut
    }

    fn dark(&self) -> bool {
        self.bg[0] * 0.299 + self.bg[1] * 0.587 + self.bg[2] * 0.114 < 128.0
    }
}

fn px(img: &RgbaImage, x: u32, y: u32) -> [f64; 3] {
    let p = img.get_pixel(x, y).0;
    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
}

/// 识字框按图上的分隔切开：识字常把表格的一整行（"Mobile App | Alice Chen | 45,000"）认成一个框，
/// 整行拿去翻译、整行重画，中英文的格子混在一起、也对不上原来的格子。
/// 分隔是框里够宽的空白（比字间距宽得多），或者两边留着空白、几乎贯穿整个框高的竖线（表格线）。
/// 文字按各段墨的宽度比例分给各段，切在最近的空格上（中文没有空格就切在字之间）
fn split_cells(img: &RgbaImage, r: &Rect, text: &str) -> Vec<(Rect, String)> {
    let whole = || vec![(*r, text.to_string())];
    let (Some((x0, y0, x1, y1)), Some(ink)) = (bounds(img, r), Ink::of(img, r)) else {
        return whole();
    };
    let h = (y1 - y0) as usize;
    if h < 8 {
        return whole();
    }
    // 每一列：多少像素是字；多少像素和底色有一点差别（浅灰的表格线比字淡得多，单独数）
    let cols: Vec<usize> = (x0..x1)
        .map(|x| (y0..y1).filter(|&y| ink.at(img, x, y)).count())
        .collect();
    let lines: Vec<usize> = (x0..x1)
        .map(|x| (y0..y1).filter(|&y| color_dist(px(img, x, y), ink.bg) > 30.0).count())
        .collect();
    let n = cols.len();
    let rule = |k: usize| lines[k] * 10 >= h * 9;
    let gap = |k: usize| cols[k] == 0 || rule(k);
    let min_blank = (h as f64 * 0.9).ceil() as usize;
    let side = (h as f64 * 0.15).ceil().max(2.0) as usize;
    // 各段（列范围）：分隔之间、去掉两头空白
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < n {
        if !gap(i) {
            i += 1;
            continue;
        }
        let s = i;
        while i < n && gap(i) {
            i += 1;
        }
        let has_rule = (s..i).any(rule);
        let blank = (s..i).filter(|&k| !rule(k)).count();
        if s > 0 && i < n && (blank >= min_blank || (has_rule && blank >= side * 2)) {
            pieces.push((start, s));
            start = i;
        }
    }
    pieces.push((start, n));
    let trim = |(a, b): (usize, usize)| {
        let a = (a..b).find(|&k| cols[k] > 0 && !rule(k))?;
        let b = (a..b).rev().find(|&k| cols[k] > 0 && !rule(k))? + 1;
        Some((a, b))
    };
    let mut pieces: Vec<(usize, usize)> = pieces.into_iter().filter_map(trim).collect();
    // 贴着框边、不到半个字宽的一小条墨是旁边格子的字伸进来了（识字框互相压着一点），不算一格
    let sliver = |(a, b): (usize, usize)| ((b - a) as f64) < h as f64 * 0.6;
    if pieces.len() >= 2 && pieces.last().is_some_and(|&p| p.1 + 3 >= n && sliver(p)) {
        pieces.pop();
    }
    if pieces.len() >= 2 && pieces.first().is_some_and(|&p| p.0 <= 2 && sliver(p)) {
        pieces.remove(0);
    }
    if pieces.len() < 2 {
        return whole();
    }
    // 文字切点：按各段墨宽的累计比例，找文字里宽度累计比例最近的空格（没有空格就是字和字之间）
    let chars: Vec<char> = text.trim().chars().collect();
    let unit = |c: char| {
        if c.is_whitespace() {
            0.0
        } else if is_wide(c) {
            2.0
        } else {
            1.0
        }
    };
    let total_units: f64 = chars.iter().map(|&c| unit(c)).sum();
    let total_ink: f64 = pieces.iter().map(|(a, b)| (b - a) as f64).sum();
    let has_space = chars.iter().any(|c| c.is_whitespace());
    let mut cum = vec![0.0; chars.len() + 1];
    for (k, &c) in chars.iter().enumerate() {
        cum[k + 1] = cum[k] + unit(c);
    }
    let mut cuts: Vec<usize> = Vec::new();
    let mut acc = 0.0;
    for (a, b) in &pieces[..pieces.len() - 1] {
        acc += (b - a) as f64;
        let want = acc / total_ink * total_units;
        let from = cuts.last().map_or(1, |&c| c + 1);
        let best = (from..chars.len())
            .filter(|&k| !has_space || chars[k].is_whitespace())
            .min_by(|&p, &q| (cum[p] - want).abs().total_cmp(&(cum[q] - want).abs()));
        match best {
            Some(k) => cuts.push(k),
            None => return whole(),
        }
    }
    let mut out = Vec::with_capacity(pieces.len());
    let mut from = 0;
    for (k, (a, b)) in pieces.iter().enumerate() {
        let to = cuts.get(k).copied().unwrap_or(chars.len());
        let t: String = chars[from..to].iter().collect::<String>().trim().to_string();
        if t.is_empty() {
            return whole();
        }
        out.push((
            Rect {
                x: f64::from(x0) + *a as f64,
                y: r.y,
                w: (b - a) as f64,
                h: r.h,
            },
            t,
        ));
        from = to;
    }
    out
}

/// 一行字量出来的：笔画粗细（像素）、是不是深底、墨的上下范围（像素）
struct Metrics {
    stroke: f64,
    dark: bool,
    extent: f64,
}

/// 笔画粗细：横竖两个方向墨的游程长度的中位数。除以字号（`font_from_extent`）以后正文大约 0.09，
/// 粗体 0.15 左右（深底浅字整体更粗，比较时分开看，见 `mark_bold`）。不能除以墨的上下范围：
/// 没有下伸字母的词（"Done"）范围小一截，正文也会量得像粗体。几乎横贯整行的（表头下面的粗线）不算字
fn ink_metrics(img: &RgbaImage, r: &Rect) -> Option<Metrics> {
    let ink = Ink::of(img, r)?;
    let (x0, y0, x1, y1) = bounds(img, r)?;
    let width = (x1 - x0) as usize;
    let rows: Vec<u32> = (y0..y1)
        .filter(|&y| {
            let n = (x0..x1).filter(|&x| ink.at(img, x, y)).count();
            n > 0 && n * 100 < width * 85
        })
        .collect();
    let extent = f64::from(rows.last()? - rows.first()? + 1);
    // 只量字所在的那几行：表头下面的粗线不算（不然竖着量出一堆 2 像素的游程，粗体量成正文）
    let (top, bottom) = (*rows.first()?, *rows.last()? + 1);
    let text_row = |y: u32| rows.binary_search(&y).is_ok();
    let runs = |outer: std::ops::Range<u32>, inner: std::ops::Range<u32>, horizontal: bool| {
        let mut v: Vec<f64> = Vec::new();
        for o in outer {
            let mut run = 0;
            for i in inner.clone() {
                let (x, y) = if horizontal { (i, o) } else { (o, i) };
                if text_row(y) && ink.at(img, x, y) {
                    run += 1;
                } else if run > 0 {
                    v.push(f64::from(run));
                    run = 0;
                }
            }
            if run > 0 {
                v.push(f64::from(run));
            }
        }
        (!v.is_empty()).then(|| median(v))
    };
    let across = runs(top..bottom, x0..x1, true)?;
    let down = runs(x0..x1, top..bottom, false)?;
    (extent >= 6.0).then_some(Metrics {
        stroke: (across + down) / 2.0,
        dark: ink.dark(),
        extent,
    })
}

/// 从墨的上下范围估字号（像素）。识字框的高矮不准（同样大的字，框能差三成），按这一行里有没有
/// 大写 / 数字 / 上伸的字母（b d f h k l t，i j 的点也差不多到那么高）、下伸的字母（g j p q y）、汉字，换算成字号
fn font_from_extent(text: &str, extent: f64) -> f64 {
    let wide = text.chars().any(is_wide);
    let up = text
        .chars()
        .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || "bdfhklijt()[]{}|/".contains(c));
    let down = text.chars().any(|c| "gjpqy()[]{}|".contains(c));
    // 系数在 Windows（Segoe UI，150%）和 Mac（SF，2x）上量过：同样 15 号字，
    // 只有上伸的一行（"Save to Downloads"）墨高 0.76 个字号，上下都伸的（"Open in new window"）0.92
    let k = if wide {
        0.9
    } else if up && down {
        0.92
    } else if up {
        0.76
    } else if down {
        0.74
    } else {
        0.53
    };
    extent / k
}

/// 哪些块是粗体：和同一选区里底色深浅相同的其他块比，笔画明显粗（多半是正文，粗的是标题、表头）；
/// 同类的块太少比不了时按经验值
fn mark_bold(blocks: &mut [TextBlock]) {
    for dark in [false, true] {
        let ratios: Vec<f64> = blocks
            .iter()
            .filter(|b| b.dark == dark)
            .filter_map(|b| b.stroke)
            .collect();
        let base = (ratios.len() >= 3).then(|| median(ratios));
        for b in blocks.iter_mut().filter(|b| b.dark == dark) {
            let Some(s) = b.stroke else { continue };
            b.bold = match base {
                Some(m) => s > m * 1.45,
                None => s > if dark { 0.2 } else { 0.125 },
            };
        }
    }
}

/// 同一行相邻的两段是不是两个格子：中间有竖线（表格线），或者空白比一个半字宽（按墨估的字号）还宽。
/// 识字框的高矮不准，只按框高判断时，表格里隔着一条线、两个字宽的两格也会被拼成一句
fn separated(img: &RgbaImage, a: &Rect, a_text: &str, b: &Rect) -> bool {
    let gap = b.x - a.right();
    let y = a.y.max(b.y);
    let bottom = a.bottom().min(b.bottom());
    if bottom - y < 4.0 {
        return false;
    }
    if gap >= 3.0 {
        let band = Rect { x: a.right(), y, w: gap, h: bottom - y };
        if let (Some(ink), Some((x0, y0, x1, y1))) = (Ink::of(img, &a.union(b)), bounds(img, &band)) {
            let h = (y1 - y0) as usize;
            let rule = (x0..x1).any(|x| (y0..y1).filter(|&y| color_dist(px(img, x, y), ink.bg) > 30.0).count() * 10 >= h * 9);
            if rule {
                return true;
            }
        }
    }
    // 空白按墨量（识字框四周留的边不一样宽，还常常交叠）：比一个字宽（字号）还宽就是两格，
    // 句子里词和词之间只有四分之一个字宽
    let Some(font) = ink_metrics(img, a).map(|m| font_from_extent(a_text, m.extent)) else {
        return false;
    };
    ink_gap(img, a, b).is_some_and(|g| g >= font)
}

/// 同一行左右两个框之间，墨真正空着的最宽一段（像素）：从左框中间量到右框中间，取最长的一段空列。
/// 两框上下重叠的那几行里数
fn ink_gap(img: &RgbaImage, a: &Rect, b: &Rect) -> Option<f64> {
    blank_run(img, a, b).map(|(_, len)| len)
}

/// 同 `ink_gap`，另外给出那段空白从哪儿开始（图上 x）
fn blank_run(img: &RgbaImage, a: &Rect, b: &Rect) -> Option<(f64, f64)> {
    let y = a.y.max(b.y);
    let bottom = a.bottom().min(b.bottom());
    let from = a.x + a.w / 2.0;
    let to = b.x + b.w / 2.0;
    if bottom - y < 4.0 || to <= from {
        return None;
    }
    let ink = Ink::of(img, &a.union(b))?;
    let (x0, y0, x1, y1) = bounds(img, &Rect { x: from, y, w: to - from, h: bottom - y })?;
    let (mut best, mut best_end, mut run) = (0u32, x0, 0u32);
    for x in x0..x1 {
        if (y0..y1).any(|y| ink.at(img, x, y)) {
            run = 0;
        } else {
            run += 1;
            if run > best {
                best = run;
                best_end = x + 1;
            }
        }
    }
    Some((f64::from(best_end - best), f64::from(best)))
}

/// 去掉行首行尾的图标以后，框是按字数比例缩的，可能切在图标上（译文底色盖住半个图标）或字上。
/// 把切口挪到离它最近的一段空白（至少五分之一个字高宽）：右边切口挪到空白起点，左边挪到空白终点
fn snap_cut(img: &RgbaImage, orig: &Rect, r: &mut Rect) {
    let (Some(ink), Some((x0, y0, x1, y1))) = (Ink::of(img, orig), bounds(img, orig)) else {
        return;
    };
    let h = f64::from(y1 - y0);
    let min_run = (h * 0.2).max(2.0) as u32;
    let mut runs: Vec<(f64, f64)> = Vec::new();
    let mut start: Option<u32> = None;
    for x in x0..=x1 {
        let blank = x < x1 && !(y0..y1).any(|y| ink.at(img, x, y));
        match (blank, start) {
            (true, None) => start = Some(x),
            (false, Some(s0)) => {
                if x - s0 >= min_run {
                    runs.push((f64::from(s0), f64::from(x)));
                }
                start = None;
            }
            _ => {}
        }
    }
    // 框本身的两头也算切口：格子是按墨切出来的，去掉的图标可能根本不在这一格里（"▸" 的字给了前一格，
    // 墨在后一格开头），这时按比例缩就会切掉最后一个字母
    runs.push((orig.right(), orig.right()));
    runs.push((orig.x, orig.x));
    let right = r.right();
    if right < orig.right() - 0.5 {
        if let Some(&(s0, _)) = runs
            .iter()
            .filter(|(s0, _)| *s0 > r.x + h && (s0 - right).abs() <= h * 2.0)
            .min_by(|a, b| (a.0 - right).abs().total_cmp(&(b.0 - right).abs()))
        {
            r.w = s0 - r.x;
        }
    }
    let right = r.right();
    if r.x > orig.x + 0.5 {
        if let Some(&(_, e)) = runs
            .iter()
            .filter(|(_, e)| *e < right - h && (e - r.x).abs() <= h * 2.0)
            .min_by(|a, b| (a.1 - r.x).abs().total_cmp(&(b.1 - r.x).abs()))
        {
            r.w = right - e;
            r.x = e;
        }
    }
}

/// 按钮里的字：字的左右、上下不远处都碰到另一种底色（按钮的边），而且左右留白差不多
fn in_button(img: &RgbaImage, r: &Rect) -> bool {
    let (Some(ink), Some((x0, y0, x1, y1))) = (Ink::of(img, r), bounds(img, r)) else {
        return false;
    };
    let (w, h) = img.dimensions();
    let has_ink = |x: u32| (y0..y1).any(|y| ink.at(img, x, y));
    let (Some(ix0), Some(ix1)) = ((x0..x1).find(|&x| has_ink(x)), (x0..x1).rev().find(|&x| has_ink(x))) else {
        return false;
    };
    let lh = f64::from(y1 - y0);
    let edge = |x: u32, y: u32| color_dist(px(img, x, y), ink.bg) > 40.0;
    let reach = (lh * 4.0) as u32;
    let cy = (y0 + y1) / 2;
    let cx = (ix0 + ix1) / 2;
    let left = (1..=reach.min(ix0)).find(|&d| edge(ix0 - d, cy));
    let right = (1..=reach.min(w - 1 - ix1)).find(|&d| edge(ix1 + d, cy));
    let up_reach = ((lh * 1.5) as u32).max(y1 - y0);
    let up = (1..=up_reach.min(cy)).find(|&d| edge(cx, cy - d));
    let down = (1..=up_reach.min(h - 1 - cy)).find(|&d| edge(cx, cy + d));
    let (Some(l), Some(r), Some(_), Some(_)) = (left, right, up, down) else {
        return false;
    };
    let (l, r) = (f64::from(l), f64::from(r));
    // 左右留白差三成多以内都算居中（按钮的字常常为了视觉居中偏一点）；输入框、列表里靠左的字两边能差好几倍
    l >= lh * 0.2 && r >= lh * 0.2 && (l - r).abs() <= (l.max(r) * 0.35).max(lh * 0.3)
}

/// 主要是西文的行里，识字给的全角标点换回半角（"（now from" 的全角括号会被当成图标剥掉），
/// 词尾大小写同形的字母认成大写的（"trackS"）改回小写
fn normalize_latin(text: &str) -> String {
    let alnum: Vec<char> = text.chars().filter(|c| c.is_alphanumeric()).collect();
    let latin = alnum.iter().filter(|c| c.is_ascii_alphanumeric()).count();
    if alnum.is_empty() || (latin as f64) < alnum.len() as f64 * 0.7 {
        return text.to_string();
    }
    let mapped: String = text
        .chars()
        .map(|c| match c {
            '（' => '(',
            '）' => ')',
            '，' => ',',
            '：' => ':',
            '；' => ';',
            '！' => '!',
            '？' => '?',
            c => c,
        })
        .collect();
    let mut out = String::with_capacity(mapped.len());
    for (k, word) in mapped.split(' ').enumerate() {
        if k > 0 {
            out.push(' ');
        }
        let cs: Vec<char> = word.chars().collect();
        // 词里最后一个字母（后面可能跟着标点）
        let last = cs.iter().rposition(|c| c.is_alphabetic());
        match last {
            Some(i) if i >= 3
                && "COSVWXZ".contains(cs[i])
                && cs[..i].iter().rev().take(3).all(|c| c.is_ascii_lowercase()) =>
            {
                let fixed: String = cs.iter().enumerate().map(|(j, &c)| if j == i { c.to_ascii_lowercase() } else { c }).collect();
                out.push_str(&fixed);
            }
            _ => out.push_str(word),
        }
    }
    out
}

/// 一行像是一句话写到头折下来的（段落的一行），不是表格里的短格子：四个词以上，或者十个以上的中文字
fn wraps(text: &str) -> bool {
    let words = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphabetic))
        .count();
    words >= 4 || text.chars().filter(|&c| is_wide(c)).count() >= 10
}

/// 行尾是连字符（前面是字母）：下一行接着这个词写，拼的时候不加空格（"win/glass-" + "shader."）
fn hyphen_end(s: &str) -> bool {
    let mut cs = s.chars().rev();
    cs.next() == Some('-') && cs.next().is_some_and(char::is_alphanumeric)
}

/// 下一行开头那个词，在上一行剩下的空里放不下：上一行是被迫折下来的。右边不齐的段落（聊天、说明文字）
/// 每行右边都空着一截，光看"写没写到头"会把一段拆成好几块各翻各的，意思就翻错了
fn next_word_wont_fit(last: &Rect, last_text: &str, column_right: f64, next: &str) -> bool {
    let cw = char_width(last_text, last);
    // 中文没有空格，"第一个词"按第一个字算
    let mut first = 0.0;
    for c in next.trim_start().chars() {
        if c.is_whitespace() {
            break;
        }
        if is_wide(c) {
            if first == 0.0 {
                first = cw;
            }
            break;
        }
        first += cw * 0.55;
    }
    first > 0.0 && last.right() + cw * 0.55 + first > column_right + cw * 0.3
}

impl TextBlock {
    fn text(&self) -> String {
        let mut out = String::new();
        for (_, t) in &self.lines {
            if !out.is_empty() && needs_space(&out, t) && !hyphen_end(&out) {
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
    // 行尾的 "-" 是折行的连字符（"win/glass-" 下一行接 "shader"）：去掉的话原图上那个 "-" 不在译文底色里，会露出来
    let keep_end = ".,!?:;)]}'\"%…-";
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
    let mut rects: Vec<Rect> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    // 从同一个识字框切出来的几格：记下是哪个框，下面按"隔得远不远"拼的时候不再拼回去
    let mut cut_from: Vec<Option<usize>> = Vec::new();
    for (k, b) in blocks.iter().enumerate() {
        let orig = aabb(&b.quad);
        let mut r = orig;
        let t = strip_icon_glyphs(&normalize_latin(&b.text), &mut r);
        if let Some(img) = image {
            snap_cut(img, &orig, &mut r);
        }
        let mut cells = match image {
            Some(img) => split_cells(img, &r, &t),
            None => vec![(r, t)],
        };
        // 切开以后格子两头又露出图标（"★ Favorites ▸ Recent files" 切成 "Favorites▸"）
        if cells.len() > 1 {
            for (r, t) in &mut cells {
                let before = *r;
                *t = strip_icon_glyphs(t, r);
                if let Some(img) = image {
                    snap_cut(img, &before, r);
                }
            }
        }
        let cut = (cells.len() > 1).then_some(k);
        for (r, t) in cells {
            rects.push(r);
            texts.push(t);
            cut_from.push(cut);
        }
    }
    let mut order: Vec<usize> = (0..rects.len())
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
        let mut last: Option<usize> = None;
        for &i in &members {
            let mut r = rects[i];
            let t = texts[i].trim();
            let was_cut = cut_from[i].is_some() && last.is_some_and(|l| cut_from[l] == cut_from[i]);
            last = Some(i);
            if let Some((cr, ct)) = cur.as_mut() {
                let gap = r.x - cr.right();
                let far = was_cut
                    || gap >= (char_width(ct, cr) * 2.5).max(cr.h * 1.2)
                    || image.is_some_and(|img| separated(img, cr, ct, &r));
                if !far {
                    // 识字框常常互相压着一点（"Holiday photos" 和 "118 MB" 的框交叠），按墨量真正的空白
                    let gap = image.and_then(|img| ink_gap(img, cr, &r)).unwrap_or(gap);
                    if gap >= char_width(ct, cr) * 0.3 && needs_space(ct, t) {
                        ct.push(' ');
                    }
                    ct.push_str(t);
                    *cr = cr.union(&r);
                    continue;
                }
                // 两格的识字框交叠：分界放在中间那段空白的正中，不然前一格的译文底色会盖住后一格开头的字
                // （"Holiday photos" 的底色盖掉 "118 MB" 的 "1"）
                if r.x < cr.right() {
                    if let Some((s0, len)) = image.and_then(|img| blank_run(img, cr, &r)) {
                        let mid = s0 + len / 2.0;
                        let right = r.right();
                        cr.w = (mid - cr.x).max(1.0);
                        r.x = mid;
                        r.w = (right - mid).max(1.0);
                    }
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
            // 上一行没写满就换行了 → 那是段落结尾（比如标题下面接正文）；
            // 上一行太短（两三个词的格子）→ 表格里同一列的格子，各翻各的
            let column = b.rect.right().max(r.right());
            let filled = (column - last.right() < h * 2.5 || next_word_wont_fit(last, last_text, column, &t)) && wraps(last_text);
            let same_color = match (b.color, color) {
                (Some(a), Some(c)) => color_dist(a, c) < 70.0,
                _ => true,
            };
            // 语言不同的不拼：英文标题下面一行"Bash 已完成 12分钟"拼进去，整段被翻译服务当成中文原样退回，
            // 标题也跟着没翻
            let same_lang = translate::detect::detect(last_text) == translate::detect::detect(&t);
            below && similar && aligned && filled && same_color && same_lang
        });
        let m = image.and_then(|img| ink_metrics(img, &r));
        let font = m.as_ref().map(|m| font_from_extent(&t, m.extent));
        // 笔画粗细 / 字号
        let stroke = m.as_ref().zip(font).map(|(m, f)| m.stroke / f.max(1.0));
        match target {
            Some(b) => {
                b.rect = b.rect.union(&r);
                b.lines.push((r, t));
                b.color = color.or(b.color);
                b.stroke = b.stroke.or(stroke);
                b.fonts.extend(font);
            }
            None => out.push(TextBlock {
                rect: r,
                lines: vec![(r, t)],
                color,
                stroke,
                dark: m.as_ref().is_some_and(|m| m.dark),
                bold: false,
                fonts: font.into_iter().collect(),
                centered: false,
            }),
        }
    }
    // 不到两个字母 / 字的不是要翻的正文：关闭按钮的"x"、图标认成的"口"；只有数字的格子（金额、
    // 日期、"30s"）翻了也一样，重画只会换掉原来的字体、对齐
    out.retain(|b| b.text().chars().filter(|c| c.is_alphabetic()).count() >= 2);
    mark_bold(&mut out);
    if let Some(img) = image {
        for b in out.iter_mut().filter(|b| b.lines.len() == 1) {
            b.centered = in_button(img, &b.rect);
        }
    }
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

/// 识别完、开始翻译前先把要翻的每一行的位置（本屏局部物理坐标 x, y, 宽, 高）交给前端：
/// 翻译要等网络，这段时间遮罩在这些行上放"正在加载"的占位动画，而不是整块干等。
pub async fn translate_region(
    app: &AppHandle,
    req: RegionRequest,
    on_lines: Option<Channel<Vec<[f64; 4]>>>,
) -> AppResult<RegionTranslation> {
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
    let (target, provider) = {
        let tr = state(app).settings.read().translate.clone();
        let provider = Some(tr.region_provider).filter(|p| !p.is_empty() && p != "auto");
        let configured = tr.target_language;
        let target = if configured != "auto" {
            configured
        } else {
            crate::i18n::target_language().to_string()
        };
        (target, provider)
    };
    // 本来就是目标语言的块不翻也不重画：重画只会把识别丢了空格、认错字的原文盖在原图上
    blocks.retain(|b| !same_language(translate::detect::detect(&b.text()), &target));
    if blocks.is_empty() {
        return Err(AppError::msg("选区里的文字已经是目标语言"));
    }
    let sources: Vec<String> = blocks.iter().map(TextBlock::text).collect();
    if let Some(ch) = &on_lines {
        let (ox, oy) = (f64::from(req.rect.x), f64::from(req.rect.y));
        let lines = blocks
            .iter()
            .flat_map(|b| b.lines.iter().map(|(r, _)| [r.x + ox, r.y + oy, r.w, r.h]))
            .collect();
        let _ = ch.send(lines);
    }

    let request = |text: String, provider: Option<String>| TranslateRequest {
        text,
        to: Some(target.clone()),
        provider,
        ..Default::default()
    };
    // 设置里指定了截图翻译用的源：先用它，失败了再按顺序降级（总得翻出来）
    let joined = sources.join("\n\n");
    let whole = match &provider {
        Some(p) => match translate::translate(app, request(joined.clone(), Some(p.clone()))).await {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(provider = %p, "截图翻译指定的翻译源失败，按顺序换别的：{err}");
                translate::translate(app, request(joined, None)).await?
            }
        },
        None => translate::translate(app, request(joined, None)).await?,
    };
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
                // 逐块补翻用整批翻成功的那个源
                let r = translate::translate(app, request(src.clone(), Some(whole.provider.clone()))).await;
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
            bold: b.bold,
            font_size: median(b.fonts.clone()),
            centered: b.centered,
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

    /// 右边不齐的一段（聊天里的说明文字）：每行右边都空着，但下一行开头的词放不进上一行 → 还是同一段；
    /// 行尾的连字符和下一行拼在一起不加空格
    #[test]
    fn ragged_paragraph_is_one_block() {
        let lines = [
            "With Emby running, all Mac",
            "server self-tests pass on the",
            "Windows branch: tracks, server",
            "preview images (now from real",
            "BIF data) and the three",
            "reconnect scenarios. Found and",
            "patched an mpv audio crash.",
            "These Mac-side fixes are",
            "uncommitted on local win/glass-",
            "shader.",
        ];
        let blocks: Vec<OcrBlock> = lines
            .iter()
            .enumerate()
            .map(|(i, t)| block(t, 10.0, 10.0 + i as f64 * 40.0, t.len() as f64 * 15.4, 28.0))
            .collect();
        let out = layout_blocks(&blocks, None);
        assert_eq!(out.len(), 1, "{:?}", out.iter().map(TextBlock::text).collect::<Vec<_>>());
        let text = out[0].text();
        assert!(text.starts_with("With Emby running, all Mac server self-tests"), "{text}");
        assert!(text.ends_with("local win/glass-shader."), "{text}");
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
    fn a_table_row_read_as_one_box_is_cut_at_the_cell_lines() {
        // 白底黑字三格："Mobile App | Alice Chen | Done"，格子之间是竖线，识字认成了一个框
        let mut img = RgbaImage::from_pixel(600, 40, image::Rgba([255, 255, 255, 255]));
        let mut word = |x0: u32, x1: u32| {
            for y in 12..28 {
                for x in (x0..x1).filter(|x| x % 4 != 3) {
                    img.put_pixel(x, y, image::Rgba([30, 30, 30, 255]));
                }
            }
        };
        word(10, 150);
        word(230, 360);
        word(440, 520);
        for x in [190, 400] {
            for y in 0..40 {
                img.put_pixel(x, y, image::Rgba([200, 200, 200, 255]));
            }
        }
        let r = Rect { x: 0.0, y: 0.0, w: 600.0, h: 40.0 };
        let cells = split_cells(&img, &r, "Mobile App Alice Chen Done");
        let texts: Vec<&str> = cells.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, vec!["Mobile App", "Alice Chen", "Done"]);
        assert!(cells[1].0.x >= 225.0 && cells[1].0.right() <= 365.0);
        // 普通的一句话（字间距窄）不切
        let mut img = RgbaImage::from_pixel(600, 40, image::Rgba([255, 255, 255, 255]));
        for x in (10..590).filter(|x| x % 40 < 34) {
            for y in 12..28 {
                img.put_pixel(x, y, image::Rgba([30, 30, 30, 255]));
            }
        }
        assert_eq!(split_cells(&img, &r, "one two three four").len(), 1);
    }

    #[test]
    fn short_cells_in_a_column_and_numbers_are_left_alone() {
        let blocks = vec![
            block("City", 0.0, 0.0, 60.0, 26.0),
            block("Shanghai", 0.0, 34.0, 100.0, 26.0),
            block("Beijing", 0.0, 68.0, 90.0, 26.0),
            block("24,870,895", 200.0, 34.0, 120.0, 26.0),
            block("2025", 200.0, 68.0, 60.0, 26.0),
        ];
        let texts: Vec<String> = layout_blocks(&blocks, None).iter().map(TextBlock::text).collect();
        assert_eq!(texts, vec!["City", "Shanghai", "Beijing"]);
    }

    #[test]
    fn neighbouring_cells_with_a_line_between_stay_apart() {
        // 识字给了两个框，中间 40 像素（比框高的 1.2 倍还窄），但隔着一条表格线
        let mut img = RgbaImage::from_pixel(400, 50, image::Rgba([255, 255, 255, 255]));
        for (x0, x1) in [(10u32, 120u32), (160, 260)] {
            for y in 16..34 {
                for x in (x0..x1).filter(|x| x % 4 != 3) {
                    img.put_pixel(x, y, image::Rgba([30, 30, 30, 255]));
                }
            }
        }
        for y in 0..50 {
            img.put_pixel(140, y, image::Rgba([210, 210, 210, 255]));
        }
        let blocks = vec![block("Mobile App", 8.0, 2.0, 114.0, 46.0), block("Alice Chen", 160.0, 2.0, 102.0, 46.0)];
        let texts: Vec<String> = layout_blocks(&blocks, Some(&img)).iter().map(TextBlock::text).collect();
        assert_eq!(texts, vec!["Mobile App", "Alice Chen"]);
        // 没有图（不知道中间有没有线）时照旧按距离拼
        assert_eq!(layout_blocks(&blocks, None).len(), 1);
    }

    #[test]
    fn overlapping_boxes_with_a_wide_ink_gap_are_two_cells() {
        // 表格一行识字给了三个框，框互相压着一点；墨之间空着两个字宽 → 三格，不拼成 "photos118 MBLast"
        let mut img = RgbaImage::from_pixel(600, 40, image::Rgba([30, 30, 30, 255]));
        let words = [(10u32, 80u32), (90, 160), (210, 260), (270, 300), (350, 400), (410, 470)];
        for (x0, x1) in words {
            for y in 10..30 {
                for x in (x0..x1).filter(|x| x % 4 != 3) {
                    img.put_pixel(x, y, image::Rgba([200, 200, 200, 255]));
                }
            }
        }
        let blocks = vec![
            block("Holiday photos", 6.0, 6.0, 200.0, 28.0),
            block("118 MB", 200.0, 6.0, 104.0, 28.0),
            block("Last week", 302.0, 6.0, 172.0, 28.0),
        ];
        let texts: Vec<String> = layout_blocks(&blocks, Some(&img)).iter().map(TextBlock::text).collect();
        assert_eq!(texts, vec!["Holiday photos", "118 MB", "Last week"]);
        // 框压进了旁边格子的字里（"Holiday photos" 的框右边盖住了 "1" 的一小条）：那一小条不算一格
        let blocks = vec![
            block("Holiday photos", 6.0, 6.0, 210.0, 28.0),
            block("118 MB", 200.0, 6.0, 104.0, 28.0),
            block("Last week", 302.0, 6.0, 172.0, 28.0),
        ];
        let mut out = layout_blocks(&blocks, Some(&img));
        out.sort_by(|a, b| a.rect.x.total_cmp(&b.rect.x));
        let texts: Vec<String> = out.iter().map(TextBlock::text).collect();
        assert_eq!(texts, vec!["Holiday photos", "118 MB", "Last week"]);
        // 分界在空白中间：前一格的底色不盖到 "118" 的 "1"（墨从 210 起）
        assert!(out[0].rect.right() <= 200.0 && out[1].rect.x >= 170.0, "{:?} {:?}", out[0].rect, out[1].rect);
    }

    #[test]
    fn text_centred_in_a_button_is_marked() {
        let mut img = RgbaImage::from_pixel(400, 80, image::Rgba([30, 30, 30, 255]));
        for y in 20..60 {
            for x in 40..240 {
                img.put_pixel(x, y, image::Rgba([10, 120, 250, 255]));
            }
        }
        for y in 32..48 {
            for x in (80..200).filter(|x| x % 4 != 3) {
                img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        // 同样的字直接写在底上（列表项）：不居中
        for y in 32..48 {
            for x in (280..380).filter(|x| x % 4 != 3) {
                img.put_pixel(x, y, image::Rgba([220, 220, 220, 255]));
            }
        }
        let blocks = vec![block("Download now", 76.0, 28.0, 128.0, 24.0), block("Save to Downloads", 276.0, 28.0, 108.0, 24.0)];
        let out = layout_blocks(&blocks, Some(&img));
        let centred: Vec<(String, bool)> = out.iter().map(|b| (b.text(), b.centered)).collect();
        assert!(centred.contains(&("Download now".into(), true)), "{centred:?}");
        assert!(centred.contains(&("Save to Downloads".into(), false)), "{centred:?}");
    }

    #[test]
    fn latin_lines_get_half_width_punctuation_and_lowercase_word_ends() {
        assert_eq!(normalize_latin("（now from real BIF data） and the three"), "(now from real BIF data) and the three");
        assert_eq!(normalize_latin("branch: trackS, server"), "branch: tracks, server");
        // 真的大写不动：macOS、iOS、全大写的词
        assert_eq!(normalize_latin("Requires macOS and iOS, see BIOS"), "Requires macOS and iOS, see BIOS");
        // 中文为主的行不动
        assert_eq!(normalize_latin("把 API key 填到（设置）里"), "把 API key 填到（设置）里");
        let mut r = Rect { x: 0.0, y: 0.0, w: 300.0, h: 20.0 };
        assert_eq!(strip_icon_glyphs(&normalize_latin("（now from real BIF data）"), &mut r), "(now from real BIF data)");
    }

    #[test]
    fn font_size_comes_from_the_ink_not_the_box() {
        // 15px 的字（150% 缩放 22.5 像素）：带大写和下伸的一行、汉字一行，墨的范围都在 21 左右
        for (text, extent) in [("City", 21.0), ("Mobile App", 21.0), ("官网改版", 20.0), ("region", 21.0)] {
            let f = font_from_extent(text, extent);
            assert!((20.5..24.0).contains(&f), "{text}: {f}");
        }
    }

    #[test]
    fn bold_is_judged_against_the_other_text() {
        let mut blocks: Vec<TextBlock> = [0.08, 0.09, 0.085, 0.16]
            .iter()
            .map(|&s| TextBlock {
                rect: Rect::default(),
                lines: Vec::new(),
                color: None,
                stroke: Some(s),
                dark: false,
                bold: false,
                fonts: Vec::new(),
                centered: false,
            })
            .collect();
        mark_bold(&mut blocks);
        assert_eq!(blocks.iter().map(|b| b.bold).collect::<Vec<_>>(), vec![false, false, false, true]);
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
