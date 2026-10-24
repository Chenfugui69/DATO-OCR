//! 选区看起来是不是数学公式（截图工具条上「识别」按钮换成公式图标、点一下直接识别公式）。
//!
//! 不识别公式本身（那个交给 AI），只判断像不像：
//! 1. 分数线：一段细横线（一两个像素粗，不贯穿整个选区），上下紧挨着都有字
//! 2. 识字的结果里有数学符号（√ ∑ ∫ ≤ 希腊字母……），或者有等号又有运算符
//!
//! 字太多（一段话）、中文太多的不算。表格的判断在前面，是表格就不会走到这里。

use image::RgbaImage;

use super::reflow::OcrBlock;

/// 一看就是数学的符号
const SPECIAL: &str = "√∛∑∏∫∮∂∞≤≥≠≈≡∝∈∉⊂⊃⊆⊇∀∃∇±∓×÷·αβγδεζηθικλμνξπρστυφχψωΓΔΘΛΞΠΣΦΨΩ";
/// 运算符（单独一个不算数，和别的一起看）
const OPERATORS: &str = "=+-−<>^/";

/// 亮度
fn lum(img: &RgbaImage, x: u32, y: u32) -> i32 {
    let p = img.get_pixel(x, y).0;
    (i32::from(p[0]) * 299 + i32::from(p[1]) * 587 + i32::from(p[2]) * 114) / 1000
}

/// 底色的亮度：四条边上像素亮度的中位数
fn background(img: &RgbaImage) -> i32 {
    let (w, h) = img.dimensions();
    let mut v: Vec<i32> = Vec::new();
    for x in (0..w).step_by((w / 64).max(1) as usize) {
        v.push(lum(img, x, 0));
        v.push(lum(img, x, h - 1));
    }
    for y in (0..h).step_by((h / 64).max(1) as usize) {
        v.push(lum(img, 0, y));
        v.push(lum(img, w - 1, y));
    }
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or(255)
}

/// 分数线有几条：细横线（不超过 4 像素粗），长度在选区宽的 8% 到 95% 之间，上下紧挨着都有字
pub fn fraction_bars(img: &RgbaImage) -> usize {
    let (w, h) = img.dimensions();
    if w < 16 || h < 12 {
        return 0;
    }
    let bg = background(img);
    let ink = |x: u32, y: u32| (lum(img, x, y) - bg).abs() > 70;
    let min_len = ((w as f64 * 0.08) as u32).max(12);
    let max_len = (w as f64 * 0.95) as u32;
    let mut found = 0;
    let mut skip_until = 0;
    for y in 2..h.saturating_sub(2) {
        if y < skip_until {
            continue;
        }
        let mut x = 0;
        while x < w {
            if !ink(x, y) {
                x += 1;
                continue;
            }
            let x0 = x;
            while x < w && ink(x, y) {
                x += 1;
            }
            let len = x - x0;
            if len < min_len || len > max_len {
                continue;
            }
            // 粗细：往下数连着几行在这一段上几乎都是墨迹
            let full = |yy: u32| (x0..x).filter(|&xx| ink(xx, yy)).count() as f64 >= len as f64 * 0.9;
            let mut thick = 1;
            while y + thick < h && full(y + thick) && thick <= 5 {
                thick += 1;
            }
            if thick > 4 {
                continue;
            }
            // 线外面紧挨着的一行基本是空的（不是一大块字的一部分）
            let sparse = |yy: u32| (x0..x).filter(|&xx| ink(xx, yy)).count() as f64 <= len as f64 * 0.3;
            if !sparse(y - 1) || (y + thick < h && !sparse(y + thick)) {
                continue;
            }
            // 上下各一段（线长的六成高）里有字
            let reach = ((len as f64 * 0.6) as u32).clamp(6, h);
            let has_ink = |ya: u32, yb: u32| {
                let mut n = 0;
                for yy in ya..yb {
                    for xx in (x0..x).step_by(2) {
                        if ink(xx, yy) {
                            n += 1;
                        }
                    }
                }
                n >= 6
            };
            let above = has_ink(y.saturating_sub(reach), y.saturating_sub(1));
            let below = has_ink((y + thick + 1).min(h), (y + thick + reach).min(h));
            if above && below {
                found += 1;
                skip_until = y + thick + 2;
                break;
            }
        }
    }
    found
}

/// 识字结果里的数学味：(数学符号个数, 运算符个数, 有没有等号, 字数, 中日韩字数)
fn text_signals(text: &str) -> (usize, usize, bool, usize, usize) {
    let special = text.chars().filter(|c| SPECIAL.contains(*c)).count();
    let operators = text.chars().filter(|c| OPERATORS.contains(*c)).count();
    let cjk = text
        .chars()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
        .count();
    let len = text.chars().filter(|c| !c.is_whitespace()).count();
    (special, operators, text.contains('='), len, cjk)
}

/// 选区像不像公式
pub fn looks_like_formula(img: &RgbaImage, blocks: &[OcrBlock]) -> bool {
    let text: String = blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join(" ");
    let (special, operators, equals, len, cjk) = text_signals(&text);
    // 一段话、中文为主的不是公式
    if len == 0 || len > 160 || cjk > 4 {
        return false;
    }
    if special >= 2 || (special >= 1 && (equals || operators >= 2)) {
        return true;
    }
    let bars = fraction_bars(img);
    bars > 0 && (equals || operators >= 1 || special >= 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str) -> OcrBlock {
        OcrBlock {
            text: text.into(),
            quad: [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            score: 1.0,
        }
    }

    /// 白底上画一条分数线，上下各一块"字"
    fn fraction(bar_len: u32, ink_above: bool) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(200, 80, image::Rgba([255, 255, 255, 255]));
        let black = image::Rgba([20, 20, 20, 255]);
        for x in 40..40 + bar_len {
            img.put_pixel(x, 40, black);
            img.put_pixel(x, 41, black);
        }
        for y in 50..62 {
            for x in 80..100 {
                img.put_pixel(x, y, black);
            }
        }
        if ink_above {
            for y in 20..34 {
                for x in 70..120 {
                    if (x + y) % 3 == 0 {
                        img.put_pixel(x, y, black);
                    }
                }
            }
        }
        img
    }

    #[test]
    fn finds_a_fraction_bar_with_text_on_both_sides() {
        assert_eq!(fraction_bars(&fraction(100, true)), 1);
        // 上面没字：只是条下划线
        assert_eq!(fraction_bars(&fraction(100, false)), 0);
    }

    #[test]
    fn math_text_counts_but_prose_does_not() {
        let img = RgbaImage::from_pixel(100, 40, image::Rgba([255, 255, 255, 255]));
        assert!(looks_like_formula(&img, &[block("∑ x_i = n")]));
        assert!(looks_like_formula(&img, &[block("α + β ≤ π")]));
        assert!(!looks_like_formula(&img, &[block("项目进度 = 已完成")]));
        assert!(!looks_like_formula(&img, &[block("Hello world")]));
        // 有分数线又有等号
        assert!(looks_like_formula(&fraction(100, true), &[block("x = -b±Vb2-4ac"), block("2a")]));
        assert!(!looks_like_formula(&fraction(100, true), &[block("Total"), block("2025")]));
    }
}
