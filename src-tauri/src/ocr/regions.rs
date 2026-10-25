//! 一张图里有好几样东西（几张表、表和公式、表上面的标题）时，先按空白切成一块块，再一块块认：
//! 点「识别表格」只认是表格的那几块，点「识别公式」只把像公式的那几块交给 AI。整张图一起认的话，
//! 几张表的线连不成一张网格、本地认不出，只能整张交给 AI，又慢又容易认错。
//!
//! 1. 底色：四条边上最多的颜色。和底色差得多的像素算"有东西"
//! 2. 横着切：一整行都是底色的地方是缝，缝和缝之间是一条（有字、有线、有底色块的一段）
//! 3. 一条条并成块：
//!    - 有横有竖的格子线的一条（带框的表）自成一块，不和上下并
//!    - 只有线、没有字的一条（表头下面那条粗线、分数线）和挨着的并
//!    - 上下两条都是好几段字、而且一列列对得上（没有框的表、斑马纹的表一行行之间有缝）就并
//!    - 挨得很近的（公式的上下标、分子分母）也并
//! 4. 带框的表并排放着的，按竖着的缝再切开
//!
//! 每块给出在图里的范围（四周留几个像素，线不会贴着边）和落在里面的字（坐标换成块里的）。

use image::RgbaImage;

use super::reflow::OcrBlock;
use super::table::{self, Box2};

/// 和底色差多少算有东西（RGB 三个通道里差得最多的那个，0 ~ 255）
const INK: i16 = 28;
/// 每块四周留的像素（找线时要和两边比，线贴着边就认不出来）
const PAD: f64 = 6.0;

/// 切出来的一块：在图里的范围（像素，含四周留的边），里面的字（坐标是块里的）
#[derive(Clone, Debug)]
pub struct Region {
    pub rect: Box2,
    pub blocks: Vec<OcrBlock>,
    /// 有横有竖的格子线（带框的表）
    pub ruled: bool,
}

impl Region {
    /// 这块在原图里的样子
    pub fn crop(&self, img: &RgbaImage) -> RgbaImage {
        let (x, y, w, h) = pixel_rect(&self.rect);
        image::imageops::crop_imm(img, x, y, w, h).to_image()
    }
}

/// 范围换成整数像素（左上角、宽高）
pub fn pixel_rect(b: &Box2) -> (u32, u32, u32, u32) {
    let (x0, y0) = (b.x0.max(0.0).floor() as u32, b.y0.max(0.0).floor() as u32);
    let (x1, y1) = (b.x1.ceil() as u32, b.y1.ceil() as u32);
    (
        x0,
        y0,
        x1.saturating_sub(x0).max(1),
        y1.saturating_sub(y0).max(1),
    )
}

/// 底色：四条边上最多的颜色（每通道 8 级一桶）
fn background(img: &RgbaImage) -> [i16; 3] {
    let (w, h) = img.dimensions();
    let mut counts = std::collections::HashMap::<[u8; 3], (usize, [u32; 3])>::new();
    let mut add = |x: u32, y: u32| {
        let p = img.get_pixel(x, y).0;
        let e = counts
            .entry([p[0] >> 3, p[1] >> 3, p[2] >> 3])
            .or_insert((0, [0; 3]));
        e.0 += 1;
        for (s, v) in e.1.iter_mut().zip(p) {
            *s += u32::from(v);
        }
    };
    for x in 0..w {
        add(x, 0);
        add(x, h - 1);
    }
    for y in 0..h {
        add(0, y);
        add(w - 1, y);
    }
    let (n, sum) = counts
        .into_values()
        .max_by_key(|(n, _)| *n)
        .unwrap_or((1, [255; 3]));
    let n = n as u32;
    [
        (sum[0] / n) as i16,
        (sum[1] / n) as i16,
        (sum[2] / n) as i16,
    ]
}

/// 横着切出来的一条：y 范围（含头不含尾）、横向上有东西的范围、里面的字（序号）
#[derive(Clone, Debug)]
struct Strip {
    y0: u32,
    y1: u32,
    x0: u32,
    x1: u32,
    boxes: Vec<usize>,
    ruled: bool,
}

/// 一组字框：横向上分开的有几段（好几段 = 像表格的一行）
fn columns_in(boxes: &[Box2]) -> usize {
    let mut spans: Vec<(f64, f64)> = boxes.iter().map(|b| (b.x0, b.x1)).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut n = 0;
    let mut right = f64::NEG_INFINITY;
    for (a, z) in spans {
        if a > right {
            n += 1;
        }
        right = right.max(z);
    }
    n
}

/// 新的一条字和块里已有的字是不是一列列对得上：大多数字框的左边、右边或中间和块里某个字框差不多
fn aligned(block: &[Box2], strip: &[Box2], tol: f64) -> bool {
    let hits = strip
        .iter()
        .filter(|b| {
            block.iter().any(|a| {
                (b.x0 - a.x0).abs() <= tol
                    || (b.x1 - a.x1).abs() <= tol
                    || ((b.x0 + b.x1) - (a.x0 + a.x1)).abs() / 2.0 <= tol
            })
        })
        .count();
    hits >= 2 && hits * 5 >= strip.len() * 3
}

/// 看这块像素里有没有有横有竖的格子线
fn has_grid(img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> bool {
    let b = Box2 {
        x0: f64::from(x0) - PAD,
        y0: f64::from(y0) - PAD,
        x1: f64::from(x1) + PAD,
        y1: f64::from(y1) + PAD,
    };
    let (x, y, w, h) = pixel_rect(&clamp(img, b));
    let crop = image::imageops::crop_imm(img, x, y, w, h).to_image();
    let l = table::ruling_lines(&crop);
    l.rows.len() >= 2 && l.cols.len() >= 2
}

fn clamp(img: &RgbaImage, b: Box2) -> Box2 {
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    Box2 {
        x0: b.x0.clamp(0.0, w),
        y0: b.y0.clamp(0.0, h),
        x1: b.x1.clamp(0.0, w),
        y1: b.y1.clamp(0.0, h),
    }
}

/// 把图切成一块块（见模块说明）。图里什么都没有时返回空
pub fn split(img: &RgbaImage, blocks: &[OcrBlock]) -> Vec<Region> {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return Vec::new();
    }
    let bg = background(img);
    let differs = |p: &image::Rgba<u8>| (0..3).any(|i| (i16::from(p.0[i]) - bg[i]).abs() > INK);
    // 每一行有东西的横向范围（None = 整行是底色）
    let rows: Vec<Option<(u32, u32)>> = (0..h)
        .map(|y| {
            let mut first = None;
            let mut last = 0;
            for x in 0..w {
                if differs(img.get_pixel(x, y)) {
                    first.get_or_insert(x);
                    last = x;
                }
            }
            first.map(|f| (f, last + 1))
        })
        .collect();
    let boxes: Vec<Box2> = blocks.iter().map(table::bounds).collect();
    let mut strips: Vec<Strip> = Vec::new();
    let mut y = 0;
    while y < h {
        let Some((mut x0, mut x1)) = rows[y as usize] else {
            y += 1;
            continue;
        };
        let y0 = y;
        while y < h {
            match rows[y as usize] {
                Some((a, z)) => {
                    x0 = x0.min(a);
                    x1 = x1.max(z);
                    y += 1;
                }
                None => break,
            }
        }
        strips.push(Strip {
            y0,
            y1: y,
            x0,
            x1,
            boxes: Vec::new(),
            ruled: false,
        });
    }
    if strips.is_empty() {
        return Vec::new();
    }
    // 字归到它中线所在（或最近）的那一条
    for (i, b) in boxes.iter().enumerate() {
        if blocks[i].text.trim().is_empty() {
            continue;
        }
        let cy = (b.y0 + b.y1) / 2.0;
        let near = strips
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, z)| {
                let d = |s: &Strip| {
                    if cy < f64::from(s.y0) {
                        f64::from(s.y0) - cy
                    } else if cy >= f64::from(s.y1) {
                        cy - f64::from(s.y1)
                    } else {
                        0.0
                    }
                };
                d(a).total_cmp(&d(z))
            })
            .map(|(k, _)| k);
        if let Some(k) = near {
            strips[k].boxes.push(i);
        }
    }
    for s in &mut strips {
        s.ruled = has_grid(img, s.x0, s.y0, s.x1, s.y1);
    }
    let mut heights: Vec<f64> = boxes
        .iter()
        .zip(blocks)
        .filter(|(_, b)| !b.text.trim().is_empty())
        .map(|(b, _)| b.height())
        .collect();
    heights.sort_by(f64::total_cmp);
    let lh = heights
        .get(heights.len() / 2)
        .copied()
        .unwrap_or(16.0)
        .max(6.0);

    // 一条条并成块
    let mut groups: Vec<Vec<Strip>> = Vec::new();
    for s in strips {
        let Some(group) = groups.last_mut() else {
            groups.push(vec![s]);
            continue;
        };
        let (last_y1, last_textless) = group
            .last()
            .map_or((s.y0, true), |g| (g.y1, g.boxes.is_empty()));
        let gap = f64::from(s.y0.saturating_sub(last_y1));
        let ruled = s.ruled || group.iter().any(|g| g.ruled);
        let group_boxes: Vec<Box2> = group
            .iter()
            .flat_map(|g| g.boxes.iter().map(|&i| boxes[i]))
            .collect();
        let strip_boxes: Vec<Box2> = s.boxes.iter().map(|&i| boxes[i]).collect();
        let lines_only = s.boxes.is_empty() || last_textless;
        let multi = columns_in(&strip_boxes) >= 2
            && group
                .iter()
                .any(|g| columns_in(&g.boxes.iter().map(|&i| boxes[i]).collect::<Vec<_>>()) >= 2);
        let merge = !ruled
            && ((lines_only && gap <= lh * 1.2)
                || gap <= lh * 0.35
                || (multi && gap <= lh * 2.0 && aligned(&group_boxes, &strip_boxes, lh * 0.9)));
        if merge {
            group.push(s);
        } else {
            groups.push(vec![s]);
        }
    }

    let mut out = Vec::new();
    for group in groups {
        let x0 = group.iter().map(|s| s.x0).min().unwrap_or(0);
        let x1 = group.iter().map(|s| s.x1).max().unwrap_or(w);
        let y0 = group.first().map_or(0, |s| s.y0);
        let y1 = group.last().map_or(h, |s| s.y1);
        let ruled = group.iter().any(|s| s.ruled);
        let ids: Vec<usize> = group.iter().flat_map(|s| s.boxes.iter().copied()).collect();
        let pieces = if ruled {
            side_by_side(
                img,
                &column_mask(img, &differs, x0, y0, x1, y1),
                x0,
                y0,
                y1,
                lh,
            )
        } else {
            vec![(x0, x1)]
        };
        for (px0, px1) in pieces {
            let mut rect = Box2 {
                x0: f64::from(px0),
                y0: f64::from(y0),
                x1: f64::from(px1),
                y1: f64::from(y1),
            };
            let inside: Vec<usize> = ids
                .iter()
                .copied()
                .filter(|&i| {
                    let cx = (boxes[i].x0 + boxes[i].x1) / 2.0;
                    cx >= rect.x0 && cx < rect.x1
                })
                .collect();
            for &i in &inside {
                rect = rect.union(&boxes[i]);
            }
            let rect = clamp(
                img,
                Box2 {
                    x0: rect.x0 - PAD,
                    y0: rect.y0 - PAD,
                    x1: rect.x1 + PAD,
                    y1: rect.y1 + PAD,
                },
            );
            let (ox, oy, _, _) = pixel_rect(&rect);
            let rect = {
                let (x, y, w, h) = pixel_rect(&rect);
                Box2 {
                    x0: f64::from(x),
                    y0: f64::from(y),
                    x1: f64::from(x + w),
                    y1: f64::from(y + h),
                }
            };
            let shifted = inside
                .iter()
                .map(|&i| {
                    let mut b = blocks[i].clone();
                    for p in &mut b.quad {
                        p[0] -= f64::from(ox);
                        p[1] -= f64::from(oy);
                    }
                    b
                })
                .collect();
            // 并排切开的每块都有格子线（`side_by_side` 只在这时才切）
            out.push(Region {
                ruled,
                rect,
                blocks: shifted,
            });
        }
    }
    out
}

/// 一块里每一列有没有东西（并排的表之间是一整列底色）
fn column_mask(
    img: &RgbaImage,
    differs: &impl Fn(&image::Rgba<u8>) -> bool,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) -> Vec<bool> {
    (x0..x1)
        .map(|x| (y0..y1).any(|y| differs(img.get_pixel(x, y))))
        .collect()
}

/// 带框的表并排放着：按够宽的竖缝切开，切出来的每块都有格子线才算数；否则整块不切
fn side_by_side(
    img: &RgbaImage,
    cols: &[bool],
    x0: u32,
    y0: u32,
    y1: u32,
    lh: f64,
) -> Vec<(u32, u32)> {
    let min_gap = (lh * 1.5).max(12.0) as usize;
    let mut pieces: Vec<(u32, u32)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut blank = 0usize;
    for (i, &c) in cols.iter().enumerate() {
        if c {
            match start {
                None => start = Some(i),
                Some(s) if blank >= min_gap => {
                    pieces.push((x0 + s as u32, x0 + (i - blank) as u32));
                    start = Some(i);
                }
                Some(_) => {}
            }
            blank = 0;
        } else if start.is_some() {
            blank += 1;
        }
    }
    if let Some(s) = start {
        pieces.push((x0 + s as u32, x0 + (cols.len() - blank) as u32));
    }
    let whole = vec![(
        x0 + cols.iter().position(|&c| c).unwrap_or(0) as u32,
        x0 + cols.iter().rposition(|&c| c).map_or(cols.len(), |i| i + 1) as u32,
    )];
    if pieces.len() < 2 || pieces.iter().any(|&(a, z)| !has_grid(img, a, y0, z, y1)) {
        return whole;
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]))
    }

    fn rect(img: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 3]) {
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
    }

    /// 画一张带框的表：rows × cols 格，每格 cw × ch，左上角在 (x, y)
    fn ruled(img: &mut RgbaImage, x: u32, y: u32, rows: u32, cols: u32, cw: u32, ch: u32) {
        for r in 0..=rows {
            rect(
                img,
                x,
                y + r * ch,
                x + cols * cw + 1,
                y + r * ch + 1,
                [120, 120, 120],
            );
        }
        for c in 0..=cols {
            rect(
                img,
                x + c * cw,
                y,
                x + c * cw + 1,
                y + rows * ch + 1,
                [120, 120, 120],
            );
        }
    }

    /// 一段"字"：画一块深色，再给一个同样范围的识字结果
    fn word(img: &mut RgbaImage, out: &mut Vec<OcrBlock>, text: &str, x: u32, y: u32, w: u32) {
        rect(img, x, y + 3, x + w, y + 13, [30, 30, 30]);
        let (x, y, w) = (f64::from(x), f64::from(y), f64::from(w));
        out.push(OcrBlock {
            text: text.into(),
            quad: [[x, y], [x + w, y], [x + w, y + 16.0], [x, y + 16.0]],
            score: 1.0,
        });
    }

    #[test]
    fn stacked_tables_and_a_formula_become_separate_regions() {
        let mut img = white(400, 420);
        let mut words = Vec::new();
        // 带框的表：3 行 3 列
        ruled(&mut img, 20, 20, 3, 3, 100, 30);
        for r in 0..3 {
            for c in 0..3 {
                word(&mut img, &mut words, "格", 30 + c * 100, 27 + r * 30, 40);
            }
        }
        // 中间一行公式（只有一段字）
        word(&mut img, &mut words, "x = a + b", 120, 150, 120);
        // 没有框的表：表头 + 粗线 + 三行，行间有空
        for c in 0..3 {
            word(&mut img, &mut words, "Head", 30 + c * 110, 200, 50);
        }
        rect(&mut img, 20, 222, 360, 225, [50, 50, 50]);
        for r in 0..3 {
            for c in 0..3 {
                word(
                    &mut img,
                    &mut words,
                    "12",
                    30 + c * 110,
                    236 + r * 34,
                    30 + c * 6,
                );
            }
        }
        let regions = split(&img, &words);
        assert_eq!(regions.len(), 3, "{regions:#?}");
        assert!(regions[0].ruled);
        assert_eq!(regions[0].blocks.len(), 9);
        assert_eq!(regions[1].blocks.len(), 1);
        assert!(!regions[2].ruled);
        assert_eq!(regions[2].blocks.len(), 12);
        // 字的坐标换成了块里的
        let first = &regions[0].blocks[0];
        assert!((first.quad[0][0] - (30.0 - regions[0].rect.x0)).abs() < 0.01);
    }

    #[test]
    fn ruled_tables_side_by_side_are_split() {
        let mut img = white(500, 160);
        ruled(&mut img, 20, 20, 3, 2, 100, 30);
        ruled(&mut img, 270, 20, 3, 2, 100, 30);
        let regions = split(&img, &[]);
        assert_eq!(regions.len(), 2, "{regions:#?}");
        assert!(regions.iter().all(|r| r.ruled));
        assert!(regions[0].rect.x1 < regions[1].rect.x0);
    }
}

#[cfg(test)]
mod smoke {
    /// 真图 + 系统识字：`CHENOCR_SMOKE_REGIONS=截图.png cargo test --lib -- --ignored smoke_regions --nocapture`
    #[test]
    #[ignore]
    fn smoke_regions() {
        let path = std::env::var("CHENOCR_SMOKE_REGIONS").unwrap();
        let img = image::open(path).unwrap().to_rgba8();
        let started = std::time::Instant::now();
        let blocks: Vec<super::OcrBlock> = crate::platform::system_ocr(&img)
            .unwrap()
            .into_iter()
            .map(|l| super::OcrBlock {
                text: l.text,
                quad: [
                    [l.x, l.y],
                    [l.x + l.width, l.y],
                    [l.x + l.width, l.y + l.height],
                    [l.x, l.y + l.height],
                ],
                score: 1.0,
            })
            .collect();
        println!(
            "识字 {} 段，{}ms",
            blocks.len(),
            started.elapsed().as_millis()
        );
        let t = std::time::Instant::now();
        for r in super::split(&img, &blocks) {
            println!(
                "块 [{:.0},{:.0} {:.0}×{:.0}] 有框 {} 字 {}：{}",
                r.rect.x0,
                r.rect.y0,
                r.rect.width(),
                r.rect.height(),
                r.ruled,
                r.blocks.len(),
                r.blocks
                    .iter()
                    .map(|b| b.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            );
        }
        println!("切块 {}ms", t.elapsed().as_millis());
        let t = std::time::Instant::now();
        for p in crate::ocr::find_tables(&img, &blocks) {
            println!(
                "表 [{:.0},{:.0} {:.0}×{:.0}] {} 行 {} 列",
                p.rect.x0,
                p.rect.y0,
                p.rect.width(),
                p.rect.height(),
                p.laid.rows,
                p.laid.cols
            );
            for c in &p.laid.cells {
                print!("「{}」", c.text);
            }
            println!();
        }
        println!("认表 {}ms", t.elapsed().as_millis());
        for f in crate::ocr::find_formulas(&img, &blocks) {
            println!(
                "公式块 [{:.0},{:.0} {:.0}×{:.0}]",
                f.x0,
                f.y0,
                f.width(),
                f.height()
            );
        }
    }
}
