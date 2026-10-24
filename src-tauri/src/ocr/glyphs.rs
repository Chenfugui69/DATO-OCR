//! 一段字的字形细节（原位显示表格时用）：按墨迹把字切成一个个字形，量出基线、x 高、大写高，
//! - 纠正大小写：c o s v w x z 这几个字母大小写只差高矮，识字常认错（"City" 认成 "city"）。
//!   字形的个数和字数对得上时，按每个字形顶到哪（x 高还是大写高）定大小写
//! - 斜体：字形的笔画整体往右上斜
//! - 等宽：每个字（连 i、l 这种窄字母）占的宽度都一样（代码、终端里的表）
//!
//! 只看一行、没有中日韩字的一段字；量不准（字形太少、个数对不上）就什么都不改。

use image::RgbaImage;

/// 大小写只差高矮的字母
const SIZE_ONLY: &str = "cosvwxzCOSVWXZ";
/// 只有 x 高、没有上伸下伸的小写字母（量 x 高用）
const X_HEIGHT: &str = "acemnorsuvwxz";

#[derive(Clone, Copy, Debug)]
struct Glyph {
    x0: u32,
    x1: u32,
    top: u32,
    bottom: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    /// 纠正过大小写的字（没改就是 None）
    pub text: Option<String>,
    pub italic: bool,
    pub mono: bool,
}

/// 切字形：墨迹连在一起的一块是一个字形（只看上下左右相连，斜着挨着的不算连着）。
/// 墨迹按"比底色和最深的字迹的中间还深"算，字与字之间淡淡的抗锯齿连不起来。
/// 浮在另一个字形正上方的小块（i、j 的点、重音）并进下面那个字形。从左到右排好
fn glyphs(img: &RgbaImage, b: (u32, u32, u32, u32), bg: [f64; 3]) -> Vec<Glyph> {
    let (x0, y0, x1, y1) = b;
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let dist: Vec<f64> = (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .map(|(x, y)| {
            let p = img.get_pixel(x, y).0;
            ((f64::from(p[0]) - bg[0]).powi(2) + (f64::from(p[1]) - bg[1]).powi(2) + (f64::from(p[2]) - bg[2]).powi(2)).sqrt()
        })
        .collect();
    let darkest = dist.iter().copied().fold(0.0, f64::max);
    let threshold = (darkest * 0.5).max(60.0);
    let ink: Vec<bool> = dist.iter().map(|&d| d > threshold).collect();
    let mut seen = vec![false; w * h];
    let mut comps: Vec<(Glyph, usize)> = Vec::new();
    for start in 0..w * h {
        if !ink[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let (mut gx0, mut gx1, mut top, mut bottom, mut n) = (usize::MAX, 0, usize::MAX, 0, 0);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            n += 1;
            gx0 = gx0.min(x);
            gx1 = gx1.max(x + 1);
            top = top.min(y);
            bottom = bottom.max(y + 1);
            let mut visit = |j: usize| {
                if ink[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if n < 2 {
            continue;
        }
        let g = Glyph {
            x0: x0 + gx0 as u32,
            x1: x0 + gx1 as u32,
            top: y0 + top as u32,
            bottom: y0 + bottom as u32,
        };
        comps.push((g, n));
    }
    let tallest = comps.iter().map(|(g, _)| g.bottom - g.top).max().unwrap_or(0);
    // 点、重音：矮小、整个在另一个字形上面、横向大半落在它的范围里 → 并进去
    let mut merged: Vec<Glyph> = Vec::new();
    let mut dots: Vec<Glyph> = Vec::new();
    for (g, _) in &comps {
        if (g.bottom - g.top) * 100 < tallest * 40 {
            dots.push(*g);
        } else {
            merged.push(*g);
        }
    }
    for d in dots {
        let host = merged.iter_mut().filter(|m| d.bottom <= m.top + 2).find(|m| {
            let overlap = d.x1.min(m.x1).saturating_sub(d.x0.max(m.x0));
            overlap * 2 >= (d.x1 - d.x0).max(1)
        });
        match host {
            Some(m) => {
                m.top = m.top.min(d.top);
                m.x0 = m.x0.min(d.x0);
                m.x1 = m.x1.max(d.x1);
            }
            // 不在别的字形上面的小块：逗号、句号、连字符，它自己就是一个字
            None => merged.push(d),
        }
    }
    merged.sort_by_key(|g| g.x0);
    merged
}

/// 斜体：把墨迹按不同的斜度"扶正"（每往上一个像素往左挪 θ 个像素），竖着的笔画对得最齐（每列的墨迹最集中）
/// 的那个斜度就是字的斜度。整段一起看，不会被 7、4、A、V 这种本来就有斜笔画的字带偏
fn is_italic(img: &RgbaImage, b: (u32, u32, u32, u32), bg: [f64; 3]) -> bool {
    let (x0, y0, x1, y1) = b;
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y).0;
            let d = (f64::from(p[0]) - bg[0]).powi(2) + (f64::from(p[1]) - bg[1]).powi(2) + (f64::from(p[2]) - bg[2]).powi(2);
            if d.sqrt() > 60.0 {
                pts.push((f64::from(x), f64::from(y)));
            }
        }
    }
    if pts.len() < 40 || y1 - y0 < 8 {
        return false;
    }
    let bottom = f64::from(y1);
    let score = |theta: f64| {
        let mut hist = std::collections::HashMap::<i64, f64>::new();
        for &(x, y) in &pts {
            *hist.entry((x - theta * (bottom - y)).round() as i64).or_default() += 1.0;
        }
        hist.values().map(|c| c * c).sum::<f64>()
    };
    let upright = score(0.0);
    let (best, best_score) = (1..=9)
        .map(|i| f64::from(i) * 0.04)
        .map(|t| (t, score(t)))
        .fold((0.0, upright), |acc, x| if x.1 > acc.1 { x } else { acc });
    best >= 0.12 && best_score >= upright * 1.08
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied()
}

/// 量一段字（`b` 是墨迹的范围 x0, y0, x1, y1，`bg` 底色）
pub fn analyze(img: &RgbaImage, b: (u32, u32, u32, u32), bg: [f64; 3], text: &str) -> Style {
    let mut style = Style::default();
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.len() < 2 || chars.iter().any(|c| ('\u{2e80}'..='\u{9fff}').contains(c)) {
        return style;
    }
    let gs = glyphs(img, b, bg);
    if gs.len() < 2 {
        return style;
    }
    style.italic = is_italic(img, b, bg);
    // 字形和字一个对一个（个数对得上）时每个字都能看；对不上时只看一个词的第一个字母（最常认错的就是它）
    let full = gs.len() == chars.len();
    // 等宽：字形起点之间的距离都差不多，而且窄字母（i、l、1、标点）后面那一步也一样宽 —— 比例字体里窄字母只占一半。
    // 窄字母在最后一个的话量不到它占多宽，不算
    if full && gs.len() >= 4 && !text.trim().contains(' ') {
        let steps: Vec<f64> = gs.windows(2).map(|w| f64::from(w[1].x0) - f64::from(w[0].x0)).collect();
        let mean = steps.iter().sum::<f64>() / steps.len() as f64;
        let narrow: Vec<f64> = chars[..chars.len() - 1]
            .iter()
            .zip(&steps)
            .filter(|(c, _)| "iIl1.,:;|!'".contains(**c))
            .map(|(_, s)| *s)
            .collect();
        let even = |s: f64| (s - mean).abs() <= mean * 0.18;
        style.mono = mean > 2.0 && !narrow.is_empty() && narrow.iter().all(|&s| even(s)) && steps.iter().all(|&s| even(s));
    }
    // 大小写：基线是大多数字形的底；x 高按只有 x 高的小写字母量，大写高按大写字母、数字量
    let baseline = median(gs.iter().map(|g| f64::from(g.bottom)).collect()).unwrap_or(0.0);
    let height = |g: &Glyph| baseline - f64::from(g.top);
    let x_h = median(
        gs.iter()
            .zip(&chars)
            .filter(|(_, c)| X_HEIGHT.contains(**c) && !SIZE_ONLY.contains(**c))
            .map(|(g, _)| height(g))
            .collect(),
    );
    let cap_h = median(
        gs.iter()
            .zip(&chars)
            .filter(|(_, c)| (c.is_ascii_uppercase() && !SIZE_ONLY.contains(**c)) || c.is_ascii_digit() || "bdfhklt".contains(**c))
            .map(|(g, _)| height(g))
            .collect(),
    );
    // 对不上时没法按字母挑参照：按整段墨迹的高和这段字有没有上伸 / 下伸部分推出字号，
    // 再按常见字体的比例算 x 高（0.53）和大写高（0.73），和表格排版用的是同一套比例
    let (x_h, cap_h) = if full {
        (x_h, cap_h)
    } else {
        let top = gs.iter().map(|g| g.top).min().unwrap_or(0);
        let bottom = gs.iter().map(|g| g.bottom).max().unwrap_or(0);
        let (up, down) = super::table::ink_extent(text);
        let font = f64::from(bottom - top) / (up + down).max(0.3);
        (Some(font * 0.53), Some(font * 0.73))
    };
    let (x_h, cap_h) = match (x_h, cap_h) {
        (Some(x), Some(c)) if c > x * 1.15 => (x, c),
        // 只有一种参照：按常见字体的比例推另一个（x 高约是大写高的七成）
        (Some(x), None) => (x, x / 0.7),
        (None, Some(c)) => (c * 0.7, c),
        _ => return style,
    };
    let split = (x_h + cap_h) / 2.0;
    let fix = |ch: char, g: &Glyph| {
        if height(g) > split {
            ch.to_ascii_uppercase()
        } else {
            ch.to_ascii_lowercase()
        }
    };
    let mut fixed = String::with_capacity(text.len());
    let mut changed = false;
    if full {
        let mut k = 0;
        for ch in text.chars() {
            if ch.is_whitespace() {
                fixed.push(ch);
                continue;
            }
            let g = &gs[k];
            k += 1;
            let want = if SIZE_ONLY.contains(ch) { fix(ch, g) } else { ch };
            changed |= want != ch;
            fixed.push(want);
        }
    } else if !text.trim().contains(char::is_whitespace) {
        let mut it = text.chars();
        if let (Some(first), Some(g)) = (it.next(), gs.first()) {
            let want = if SIZE_ONLY.contains(first) { fix(first, g) } else { first };
            changed = want != first;
            fixed.push(want);
            fixed.extend(it);
        }
    }
    if changed {
        style.text = Some(fixed);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 白底上按"字形"画黑块：(宽, 顶到基线的高, 下伸) 一个接一个，中间隔 2 像素；斜体的块往右上斜
    fn draw(glyphs: &[(u32, u32, u32)], italic: bool) -> (RgbaImage, (u32, u32, u32, u32)) {
        let baseline = 40;
        let mut img = RgbaImage::from_pixel(300, 60, image::Rgba([255, 255, 255, 255]));
        let mut x = 10;
        for &(w, h, down) in glyphs {
            for y in baseline - h..baseline + down {
                let shift = if italic { (baseline - y) / 3 } else { 0 };
                for xx in x + shift..x + shift + w {
                    img.put_pixel(xx, y, image::Rgba([0, 0, 0, 255]));
                }
            }
            x += w + 2 + if italic { 8 } else { 0 };
        }
        (img, (5, 5, x + 5, 55))
    }

    #[test]
    fn fixes_case_of_letters_that_only_differ_in_height() {
        // "City"：C 是大写高（14），i 带点也是 14，t 12，y 是 x 高（10）带下伸
        let (img, b) = draw(&[(6, 14, 0), (2, 14, 0), (3, 12, 0), (5, 10, 4)], false);
        let s = analyze(&img, b, [255.0; 3], "city");
        assert_eq!(s.text.as_deref(), Some("City"));
        // 本来就对：不改
        assert_eq!(analyze(&img, b, [255.0; 3], "City").text, None);
        // "SQL" 认成了 "sql"：s 的高和 L（大写高）一样 → 大写
        let (img, b) = draw(&[(6, 14, 0), (6, 14, 2), (5, 14, 0)], false);
        assert_eq!(analyze(&img, b, [255.0; 3], "sqL").text.as_deref(), Some("SqL"));
    }

    #[test]
    fn mismatched_glyph_count_only_judges_the_first_letter_of_one_word() {
        // "Cat" 的 a、t 粘成了一个字形：只看第一个字母（大写高）
        let (img, b) = draw(&[(6, 14, 0), (8, 14, 0)], false);
        assert_eq!(analyze(&img, b, [255.0; 3], "cat").text.as_deref(), Some("Cat"));
        // 好几个词又对不上：哪个字形是哪个字说不准，不改
        assert_eq!(analyze(&img, b, [255.0; 3], "c at").text, None);
    }

    /// 真图：`CHENOCR_SMOKE_GLYPHS=图片 CHENOCR_SMOKE_REGIONS="x0,y0,x1,y1,字;…" cargo test --lib -- --ignored smoke_glyphs --nocapture`
    #[test]
    #[ignore]
    fn smoke_glyphs() {
        let img = image::open(std::env::var("CHENOCR_SMOKE_GLYPHS").unwrap()).unwrap().to_rgba8();
        for spec in std::env::var("CHENOCR_SMOKE_REGIONS").unwrap().split(';') {
            let parts: Vec<&str> = spec.splitn(5, ',').collect();
            let n: Vec<u32> = parts[..4].iter().map(|v| v.parse().unwrap()).collect();
            let text = parts[4];
            let p = img.get_pixel(n[0], n[1]).0;
            let bg = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
            let gs = glyphs(&img, (n[0], n[1], n[2], n[3]), bg);
            println!("{text}: {} glyphs {:?}", gs.len(), gs.iter().map(|g| (g.x0, g.top, g.bottom)).collect::<Vec<_>>());
            println!("  => {:?}", analyze(&img, (n[0], n[1], n[2], n[3]), bg, text));
        }
    }

    #[test]
    fn detects_italic_and_monospace() {
        let (img, b) = draw(&[(3, 14, 0), (3, 14, 0), (3, 14, 0), (3, 14, 0)], true);
        assert!(analyze(&img, b, [255.0; 3], "HHHH").italic);
        let (img, b) = draw(&[(3, 14, 0), (3, 14, 0), (3, 14, 0), (3, 14, 0)], false);
        assert!(!analyze(&img, b, [255.0; 3], "HHHH").italic);
        // 等宽：i 和 m 占的位置一样宽
        let (img, b) = draw(&[(6, 10, 0), (6, 14, 0), (6, 10, 0), (6, 10, 0), (6, 14, 0)], false);
        assert!(analyze(&img, b, [255.0; 3], "aiman").mono);
    }
}
