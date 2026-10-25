//! 本地识别表格（不用 AI）：截图界面里点「识别表格」先走这里，几百毫秒出结果，不联网。
//!
//! 1. 找表格线：每一行 / 每一列像素里，"比上下（左右）两边都明显深或浅"的细线像素连成的最长一段，
//!    长过选区宽（高）的三分之一就算一条横（竖）线。挨着的几行合成一条（线有一两个像素粗）
//! 2. 识字一次，得到一行行文字和它们的框
//! 3. 有竖线就按竖线分列，没有就看文字：所有文字框在横向上投影，空出来的竖条就是列与列的缝
//! 4. 有横线就按横线分行，没有就把纵向上重叠的文字框归成一行
//! 5. 一行文字跨了好几列（系统识字常把一整行认成一段）：按空格切开，按字在框里的位置估出每个词落在哪列
//! 6. 合并单元格：两格之间那段竖线（横线）没有，就是合并的
//!
//! 也给截图工具条用：选区里有没有表格（`looks_like_table`），有的话「识别」按钮换成表格图标。

use image::RgbaImage;
use serde::Serialize;

use super::reflow::OcrBlock;
use super::xlsx::TableCell;

/// 线像素要比两边亮度差多少（0 ~ 255）
const CONTRAST: i32 = 22;
/// 和两边比较时隔几个像素（线最粗按 2 像素算）
const SIDE: u32 = 3;
/// 一条线至少占选区宽（高）的多少
const MIN_SPAN: f64 = 0.33;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lines {
    /// 横线的 y（从上到下）
    pub rows: Vec<u32>,
    /// 竖线的 x（从左到右）
    pub cols: Vec<u32>,
}

fn luminance(img: &RgbaImage) -> Vec<i32> {
    img.pixels()
        .map(|p| (i32::from(p[0]) * 299 + i32::from(p[1]) * 587 + i32::from(p[2]) * 114) / 1000)
        .collect()
}

/// 一串"是不是线像素"里最长的一段（中间断开不超过 5 个像素的接起来算：竖线和横线交叉的地方，
/// 两边也是深色、不算线像素，线粗到 4 像素（Retina 屏上 2pt 的线）时一断就是 4 个像素）
fn longest_run(marks: impl Iterator<Item = bool>) -> u32 {
    let (mut best, mut cur, mut gap) = (0u32, 0u32, 0u32);
    for m in marks {
        if m {
            cur += 1 + gap;
            gap = 0;
            best = best.max(cur);
        } else if cur > 0 && gap < 5 {
            gap += 1;
        } else {
            cur = 0;
            gap = 0;
        }
    }
    best
}

/// 挨着的几个位置合成一条线（取中间）
fn merge(positions: Vec<u32>) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let mut group: Vec<u32> = Vec::new();
    for p in positions {
        if group.last().is_some_and(|&l| p > l + 2) {
            out.push(group[group.len() / 2]);
            group.clear();
        }
        group.push(p);
    }
    if !group.is_empty() {
        out.push(group[group.len() / 2]);
    }
    out
}

/// 找表格线
pub fn ruling_lines(img: &RgbaImage) -> Lines {
    let (w, h) = img.dimensions();
    if w < SIDE * 2 + 4 || h < SIDE * 2 + 4 {
        return Lines::default();
    }
    let lum = luminance(img);
    let at = |x: u32, y: u32| lum[(y * w + x) as usize];
    let is_h = |x: u32, y: u32| {
        let v = at(x, y);
        (v - at(x, y - SIDE)).abs() > CONTRAST && (v - at(x, y + SIDE)).abs() > CONTRAST
    };
    let is_v = |x: u32, y: u32| {
        let v = at(x, y);
        (v - at(x - SIDE, y)).abs() > CONTRAST && (v - at(x + SIDE, y)).abs() > CONTRAST
    };
    let min_h = ((f64::from(w) * MIN_SPAN) as u32).max(24);
    let min_v = ((f64::from(h) * MIN_SPAN) as u32).max(16);
    let rows = (SIDE..h - SIDE)
        .filter(|&y| longest_run((0..w).map(|x| is_h(x, y))) >= min_h)
        .collect();
    let cols = (SIDE..w - SIDE)
        .filter(|&x| longest_run((0..h).map(|y| is_v(x, y))) >= min_v)
        .collect();
    Lines {
        rows: merge(rows),
        cols: merge(cols),
    }
}

/// 选区看起来是不是一张表：有横有竖的格子，或者一排横线隔开的好几行
pub fn looks_like_table(img: &RgbaImage) -> bool {
    let l = ruling_lines(img);
    (l.rows.len() >= 3 && l.cols.len() >= 2) || l.rows.len() >= 5
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box2 {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Box2 {
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    pub fn union(&self, o: &Box2) -> Box2 {
        Box2 {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

pub(super) fn bounds(b: &OcrBlock) -> Box2 {
    let xs = b.quad.iter().map(|p| p[0]);
    let ys = b.quad.iter().map(|p| p[1]);
    Box2 {
        x0: xs.clone().fold(f64::INFINITY, f64::min),
        x1: xs.fold(f64::NEG_INFINITY, f64::max),
        y0: ys.clone().fold(f64::INFINITY, f64::min),
        y1: ys.fold(f64::NEG_INFINITY, f64::max),
    }
}

/// 一段区间列表里，`v` 落在第几段
fn band(edges: &[f64], v: f64) -> Option<usize> {
    edges.windows(2).position(|w| v >= w[0] && v < w[1])
}

/// 文字框在横向上的投影里空出来的缝 → 列的分界（不含两头）
fn gutters(boxes: &[Box2], width: f64) -> Vec<f64> {
    let n = width.ceil().max(1.0) as usize;
    let mut covered = vec![false; n + 1];
    for b in boxes {
        let (a, z) = (b.x0.max(0.0) as usize, (b.x1.min(width) as usize).min(n));
        if a <= z {
            covered[a..=z].fill(true);
        }
    }
    let mut cuts = Vec::new();
    let mut start: Option<usize> = None;
    let first = covered.iter().position(|&c| c).unwrap_or(0);
    let last = covered.iter().rposition(|&c| c).unwrap_or(n);
    for (x, &c) in covered.iter().enumerate().take(last + 1).skip(first) {
        match (c, start) {
            (false, None) => start = Some(x),
            (true, Some(s)) => {
                // 太窄的缝是字之间的空格，不是列缝
                if x - s >= 6 {
                    cuts.push((s + x) as f64 / 2.0);
                }
                start = None;
            }
            _ => {}
        }
    }
    cuts
}

/// 纵向上重叠的文字框归成一行 → 行的分界（不含两头）
fn row_cuts(boxes: &[Box2]) -> Vec<f64> {
    let mut sorted: Vec<Box2> = boxes.to_vec();
    sorted.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let mut cuts = Vec::new();
    let mut bottom: Option<f64> = None;
    for b in sorted {
        let center = (b.y0 + b.y1) / 2.0;
        match bottom {
            Some(bot) if center > bot => {
                cuts.push((bot + b.y0) / 2.0);
                bottom = Some(b.y1);
            }
            Some(bot) => bottom = Some(bot.max(b.y1)),
            None => bottom = Some(b.y1),
        }
    }
    cuts
}

/// 一段竖线在 [y0, y1) 这段里在不在（合并单元格时这段线是没有的）
fn has_v_segment(img: &RgbaImage, x: u32, y0: u32, y1: u32) -> bool {
    let (w, h) = img.dimensions();
    if x < SIDE || x + SIDE >= w || y1 <= y0 + 4 {
        return true;
    }
    let lum = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        (i32::from(p[0]) * 299 + i32::from(p[1]) * 587 + i32::from(p[2]) * 114) / 1000
    };
    let (a, b) = (y0 + 2, y1.min(h) - 2);
    let hits = (a..b)
        .filter(|&y| {
            (x.saturating_sub(1)..=x + 1).any(|xx| {
                let v = lum(xx, y);
                (v - lum(xx - SIDE, y)).abs() > CONTRAST && (v - lum(xx + SIDE, y)).abs() > CONTRAST
            })
        })
        .count();
    hits as f64 >= f64::from(b - a) * 0.6
}

fn has_h_segment(img: &RgbaImage, y: u32, x0: u32, x1: u32) -> bool {
    let (w, h) = img.dimensions();
    if y < SIDE || y + SIDE >= h || x1 <= x0 + 4 {
        return true;
    }
    let lum = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        (i32::from(p[0]) * 299 + i32::from(p[1]) * 587 + i32::from(p[2]) * 114) / 1000
    };
    let (a, b) = (x0 + 2, x1.min(w) - 2);
    let hits = (a..b)
        .filter(|&x| {
            (y.saturating_sub(1)..=y + 1).any(|yy| {
                let v = lum(x, yy);
                (v - lum(x, yy - SIDE)).abs() > CONTRAST && (v - lum(x, yy + SIDE)).abs() > CONTRAST
            })
        })
        .count();
    hits as f64 >= f64::from(b - a) * 0.6
}

/// 认出来的网格：行、列的分界（选区图的像素坐标，含两头）和合并后的每一格
#[derive(Clone, Debug)]
pub struct Grid {
    pub row_edges: Vec<f64>,
    pub col_edges: Vec<f64>,
    pub cells: Vec<GridCell>,
}

#[derive(Clone, Debug)]
pub struct GridCell {
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub text: String,
    /// 落进这格的文字框（一行字按词拆到几列时，是估出来的那个词的框）
    pub boxes: Vec<Box2>,
}

impl Grid {
    pub fn rows(&self) -> usize {
        self.row_edges.len().saturating_sub(1)
    }

    pub fn cols(&self) -> usize {
        self.col_edges.len().saturating_sub(1)
    }

    /// 这一格的范围
    pub fn rect(&self, c: &GridCell) -> Box2 {
        Box2 {
            x0: self.col_edges[c.col],
            x1: self.col_edges[c.col + c.col_span],
            y0: self.row_edges[c.row],
            y1: self.row_edges[c.row + c.row_span],
        }
    }

    /// 按行排好的单元格（导出、转 HTML 用）。第一行当表头
    pub fn table(&self) -> Vec<Vec<TableCell>> {
        let mut out: Vec<Vec<TableCell>> = vec![Vec::new(); self.rows()];
        for c in &self.cells {
            out[c.row].push(TableCell {
                text: c.text.clone(),
                header: c.row == 0,
                row_span: c.row_span as u32,
                col_span: c.col_span as u32,
            });
        }
        out.retain(|r| !r.is_empty());
        out
    }
}

/// 一段文字框按纵向重叠分成一行行，每行的上下沿（从上到下）
fn text_lines(boxes: &[Box2]) -> Vec<(f64, f64)> {
    let mut sorted: Vec<Box2> = boxes.to_vec();
    sorted.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for b in sorted {
        let center = (b.y0 + b.y1) / 2.0;
        match out.last_mut() {
            Some(last) if center <= last.1 => last.1 = last.1.max(b.y1),
            _ => out.push((b.y0, b.y1)),
        }
    }
    out
}

/// 两条横线之间的一段里有好几行字、而且每行都跨好几列：这其实是好几行表格，只是行与行之间的线太淡、
/// 没认出来（常见：只有表头上下有深色的线，下面的行靠浅浅的底色区分）。按字行把这段切开。
/// 只有一列有第二行字的不切：那是一格里的字换行了
fn split_bands(row_edges: &mut Vec<f64>, boxes: &[Box2], col_edges: &[f64]) {
    if col_edges.len() < 3 {
        return;
    }
    let mut extra = Vec::new();
    for band in row_edges.windows(2) {
        let inside: Vec<Box2> = boxes
            .iter()
            .copied()
            .filter(|b| (b.y0 + b.y1) / 2.0 >= band[0] && (b.y0 + b.y1) / 2.0 < band[1])
            .collect();
        let lines = text_lines(&inside);
        if lines.len() < 2 {
            continue;
        }
        let wide = lines
            .iter()
            .filter(|(top, bottom)| {
                let cols = col_edges
                    .windows(2)
                    .filter(|c| {
                        inside.iter().any(|b| {
                            let y = (b.y0 + b.y1) / 2.0;
                            y >= *top && y <= *bottom && b.x1 > c[0] && b.x0 < c[1]
                        })
                    })
                    .count();
                cols >= 2
            })
            .count();
        if wide >= 2 && wide as f64 >= lines.len() as f64 * 0.6 {
            extra.extend(lines.windows(2).map(|w| (w[0].1 + w[1].0) / 2.0));
        }
    }
    row_edges.extend(extra);
    row_edges.sort_by(f64::total_cmp);
}

/// 文字落格：按中心找行；一段跨了几列的按空格切开，每个词按它在框里的位置估列（也估出词的框）
fn assign(
    boxes: &[(Box2, &OcrBlock)],
    row_edges: &[f64],
    col_edges: &[f64],
) -> Vec<Vec<Vec<(f64, String, Box2)>>> {
    let (rows, cols) = (row_edges.len() - 1, col_edges.len() - 1);
    let mut cells: Vec<Vec<Vec<(f64, String, Box2)>>> = vec![vec![Vec::new(); cols]; rows];
    for (b, block) in boxes {
        let Some(r) = band(row_edges, (b.y0 + b.y1) / 2.0) else {
            continue;
        };
        let text = block.text.trim();
        let total = text.chars().count().max(1) as f64;
        let mut start = 0usize;
        // 一段整个落在一列里：不拆（框就是原来的框，词之间的空格原样保留）
        let whole = band(col_edges, b.x0 + 1.0).filter(|&c| band(col_edges, b.x1 - 1.0) == Some(c));
        if let Some(c) = whole {
            cells[r][c].push(((b.x0 + b.x1) / 2.0, text.to_string(), *b));
            continue;
        }
        for word in text.split_whitespace() {
            let idx = text[start..].find(word).map_or(start, |i| start + i);
            let char_idx = text[..idx].chars().count() as f64;
            let len = word.chars().count() as f64;
            let wx0 = b.x0 + (b.x1 - b.x0) * char_idx / total;
            let wx1 = b.x0 + (b.x1 - b.x0) * (char_idx + len) / total;
            let center = (wx0 + wx1) / 2.0;
            start = idx + word.len();
            let c =
                band(col_edges, center).unwrap_or(if center < col_edges[0] { 0 } else { cols - 1 });
            cells[r][c].push((
                center,
                word.to_string(),
                Box2 {
                    x0: wx0,
                    x1: wx1,
                    ..*b
                },
            ));
        }
    }
    cells
}

/// 没有一个字的行 / 列（两条线挨太近、线和字之间的空白）并进旁边那一行 / 列：去掉它和下一行（列）之间的分界，
/// 最后一行（列）就去掉它上面的分界。返回去掉了几条
fn drop_empty(edges: &mut Vec<f64>, empty: impl Fn(usize) -> bool) -> usize {
    let n = edges.len() - 1;
    let gone: Vec<usize> = (0..n)
        .filter(|&i| empty(i))
        .map(|i| if i + 1 < n { i + 1 } else { i })
        .collect();
    let mut removed = 0;
    for &e in gone.iter().rev() {
        // 只剩一行 / 一列时不再并
        if edges.len() > 2 && e > 0 && e < edges.len() - 1 {
            edges.remove(e);
            removed += 1;
        }
    }
    removed
}

/// 把识字结果排进表格网格。没认出至少 2×2 的表时返回 None
pub fn grid(img: &RgbaImage, blocks: &[OcrBlock]) -> Option<Grid> {
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    let lines = ruling_lines(img);
    let boxes: Vec<(Box2, &OcrBlock)> = blocks
        .iter()
        .filter(|b| !b.text.trim().is_empty())
        .map(|b| (bounds(b), b))
        .collect();
    if boxes.is_empty() {
        return None;
    }
    let plain: Vec<Box2> = boxes.iter().map(|(b, _)| *b).collect();
    // 列：竖线够用就按竖线，否则按文字间的缝
    let ruled_cols = lines.cols.len() >= 2;
    let mut col_edges: Vec<f64> = if ruled_cols {
        lines.cols.iter().map(|&x| f64::from(x)).collect()
    } else {
        gutters(&plain, w)
    };
    if !ruled_cols
        || col_edges
            .first()
            .is_some_and(|&x| plain.iter().any(|b| (b.x0 + b.x1) / 2.0 < x))
    {
        col_edges.insert(0, 0.0);
    }
    if !ruled_cols
        || col_edges
            .last()
            .is_some_and(|&x| plain.iter().any(|b| (b.x0 + b.x1) / 2.0 > x))
    {
        col_edges.push(w);
    }
    // 行：横线够用就按横线（线之间夹着好几行字的再按字行切开），否则按字行
    let ruled_rows = lines.rows.len() >= 2;
    let mut row_edges: Vec<f64> = if ruled_rows {
        lines.rows.iter().map(|&y| f64::from(y)).collect()
    } else {
        row_cuts(&plain)
    };
    if !ruled_rows
        || row_edges
            .first()
            .is_some_and(|&y| plain.iter().any(|b| (b.y0 + b.y1) / 2.0 < y))
    {
        row_edges.insert(0, 0.0);
    }
    if !ruled_rows
        || row_edges
            .last()
            .is_some_and(|&y| plain.iter().any(|b| (b.y0 + b.y1) / 2.0 > y))
    {
        row_edges.push(h);
    }
    if ruled_rows {
        split_bands(&mut row_edges, &plain, &col_edges);
    }
    if row_edges.len() < 3 || col_edges.len() < 3 {
        return None;
    }
    let mut cells = assign(&boxes, &row_edges, &col_edges);
    let removed = drop_empty(&mut row_edges, |r| cells[r].iter().all(Vec::is_empty))
        + drop_empty(&mut col_edges, |c| {
            cells.iter().all(|row| row[c].is_empty())
        });
    if removed > 0 {
        cells = assign(&boxes, &row_edges, &col_edges);
    }
    let (rows, cols) = (row_edges.len() - 1, col_edges.len() - 1);
    if rows < 2 || cols < 2 {
        return None;
    }
    // 没有竖线的"表"靠文字对齐分格：真正的表格大多数行都有两格以上有字。每行只有一段字的
    // （公式的分子分母、几行说明文字）不是表
    if !ruled_cols {
        let multi = cells
            .iter()
            .filter(|row| row.iter().filter(|c| !c.is_empty()).count() >= 2)
            .count();
        if multi < 2 || multi * 2 < rows {
            return None;
        }
    }
    // 合并单元格（只在有线的表里判断）：两格之间那段竖线（横线）没有，就是合并的。
    // 两格都有字的不算：线太淡没认出来而已，真合并的格子里字只有一份。
    // 例外：同一段字跨过了这条分界（"2025 年" 被按空格拆进了两格），那两格本来就是一格
    let empty = |r: usize, c: usize| cells[r][c].is_empty();
    let mut crossed = std::collections::HashSet::new();
    for (b, _) in &boxes {
        let Some(r) = band(&row_edges, (b.y0 + b.y1) / 2.0) else {
            continue;
        };
        if let (Some(c0), Some(c1)) = (band(&col_edges, b.x0 + 1.0), band(&col_edges, b.x1 - 1.0)) {
            for c in c0..c1 {
                crossed.insert((r, c));
            }
        }
    }
    let mut span_right = vec![vec![false; cols]; rows];
    let mut span_down = vec![vec![false; cols]; rows];
    if ruled_cols && ruled_rows {
        for r in 0..rows {
            let (y0, y1) = (row_edges[r] as u32, row_edges[r + 1] as u32);
            for c in 0..cols - 1 {
                span_right[r][c] = (empty(r, c) || empty(r, c + 1) || crossed.contains(&(r, c)))
                    && !has_v_segment(img, col_edges[c + 1] as u32, y0, y1);
            }
        }
        for c in 0..cols {
            let (x0, x1) = (col_edges[c] as u32, col_edges[c + 1] as u32);
            for r in 0..rows - 1 {
                span_down[r][c] = (empty(r, c) || empty(r + 1, c))
                    && !has_h_segment(img, row_edges[r + 1] as u32, x0, x1);
            }
        }
    }
    let mut covered = vec![vec![false; cols]; rows];
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            if covered[r][c] {
                continue;
            }
            let mut cs = 1;
            while c + cs < cols && span_right[r][c + cs - 1] {
                cs += 1;
            }
            let mut rs = 1;
            while r + rs < rows && (c..c + cs).all(|cc| span_down[r + rs - 1][cc]) {
                rs += 1;
            }
            let mut words: Vec<(f64, String, Box2)> = Vec::new();
            for row in covered.iter_mut().skip(r).take(rs) {
                row[c..c + cs].fill(true);
            }
            for row in cells.iter().skip(r).take(rs) {
                for cell in &row[c..c + cs] {
                    words.extend(cell.iter().cloned());
                }
            }
            // 先按行、再按左右排：一格里换了行的字按阅读顺序连起来
            let line_of = |b: &Box2| {
                text_lines(&words.iter().map(|w| w.2).collect::<Vec<_>>())
                    .iter()
                    .position(|l| (b.y0 + b.y1) / 2.0 <= l.1)
                    .unwrap_or(0)
            };
            let mut keyed: Vec<(usize, f64, String, Box2)> = words
                .iter()
                .map(|(x, t, b)| (line_of(b), *x, t.clone(), *b))
                .collect();
            keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            let text = join_words(keyed.iter().map(|k| k.2.clone()));
            out.push(GridCell {
                row: r,
                col: c,
                row_span: rs,
                col_span: cs,
                text,
                boxes: keyed.into_iter().map(|k| k.3).collect(),
            });
        }
    }
    Some(Grid {
        row_edges,
        col_edges,
        cells: out,
    })
}

/// 把识字结果排进表格。没认出至少 2×2 的表时返回 None
pub fn build(img: &RgbaImage, blocks: &[OcrBlock]) -> Option<Vec<Vec<TableCell>>> {
    let t = grid(img, blocks)?.table();
    (t.len() >= 2).then_some(t)
}

fn is_cjk(c: char) -> bool {
    ('\u{3040}'..='\u{9FFF}').contains(&c) || ('\u{AC00}'..='\u{D7AF}').contains(&c)
}

/// 词连回一句：中日韩的字之间不加空格，其他加。整句没有中日韩的字时，系统识字认成全角的括号、冒号、
/// 逗号换回半角（"Budgeted ($)" 常被认成 "Budgeted （$）"）
fn join_words(words: impl Iterator<Item = String>) -> String {
    let mut out = String::new();
    for w in words {
        // 全角标点挨着中文也不加空格
        let tight = |c: char| {
            is_cjk(c)
                || ('\u{3000}'..='\u{303F}').contains(&c)
                || ('\u{FF00}'..='\u{FFEF}').contains(&c)
        };
        if !out.is_empty()
            && !(out.chars().last().is_some_and(tight) && w.chars().next().is_some_and(tight))
        {
            out.push(' ');
        }
        out.push_str(&w);
    }
    if out.chars().any(is_cjk) {
        return ascii_brackets(&out);
    }
    out.chars()
        .map(|c| match c {
            '（' => '(',
            '）' => ')',
            '：' => ':',
            '，' => ',',
            '；' => ';',
            _ => c,
        })
        .collect()
}

/// 中文里括着的全是半角字（"预算（$）"）：原文多半是半角括号，换回 "预算 ($)"
fn ascii_brackets(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '（' {
            if let Some(len) = chars[i + 1..].iter().position(|&c| c == '）') {
                let inner = &chars[i + 1..i + 1 + len];
                if len > 0 && inner.iter().all(|c| c.is_ascii()) {
                    if out.chars().last().is_some_and(|c| !c.is_whitespace()) {
                        out.push(' ');
                    }
                    out.push('(');
                    out.extend(inner);
                    out.push(')');
                    i += len + 2;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 表格转成 HTML（和 AI 回答的格式一样，页面同一套代码显示）
pub fn to_html(rows: &[Vec<TableCell>]) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut html = String::from("<table>");
    for row in rows {
        html.push_str("<tr>");
        for c in row {
            let tag = if c.header { "th" } else { "td" };
            html.push('<');
            html.push_str(tag);
            if c.row_span > 1 {
                html.push_str(&format!(" rowspan=\"{}\"", c.row_span));
            }
            if c.col_span > 1 {
                html.push_str(&format!(" colspan=\"{}\"", c.col_span));
            }
            html.push('>');
            html.push_str(&esc(&c.text));
            html.push_str(&format!("</{tag}>"));
        }
        html.push_str("</tr>");
    }
    html.push_str("</table>");
    html
}

// ───────────────────────── 原位显示：每格的位置、样式和抹掉字的底图 ─────────────────────────
//
// 截图界面里认出的表格直接盖在原来的表格上：底图是选区原图把字抹掉（线、底色、边框都还是原来的），
// 上面按原来的位置、字号、颜色、粗细、对齐摆上认出来的字，能直接改。

/// 原位显示的一格。坐标都是选区图的像素
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaidCell {
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub header: bool,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// 字所在的范围 [x, y, 宽, 高]（空格子没有）
    pub text_box: Option<[f64; 4]>,
    /// 字有几行
    pub lines: usize,
    /// 字号（像素）：按字的墨迹高和这段字有没有大写 / 下伸部分换算出来的
    pub font_px: f64,
    /// 墨迹中心比行框中心低多少（字号的倍数）：页面按墨迹中心摆字时用（有下伸部分的字墨迹偏下）
    pub center_shift: f64,
    /// 字色 #rrggbb
    pub color: String,
    /// 这格的底色 #rrggbb（编辑时用来盖住底图上的字迹残留）
    pub background: String,
    pub bold: bool,
    /// left | center | right
    pub align: &'static str,
    /// 位置、字号是按墨迹量的（不是的话是按识字框估的，不太准）。页面按墨迹的高重新量字号
    pub by_ink: bool,
    /// 笔画宽 ÷ 字号（判断粗体用；页面按它在半粗 / 全粗之间选）
    pub stroke: Option<f64>,
    /// 斜体
    pub italic: bool,
    /// 等宽字体（代码、终端里的表）
    pub mono: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaidTable {
    pub width: u32,
    pub height: u32,
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<LaidCell>,
}

type Rgb = [f64; 3];

fn rgb(img: &RgbaImage, x: u32, y: u32) -> Rgb {
    let p = img.get_pixel(x, y).0;
    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
}

fn dist(a: Rgb, b: Rgb) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn hex(c: Rgb) -> String {
    let v = |x: f64| x.round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", v(c[0]), v(c[1]), v(c[2]))
}

/// 一堆颜色里最多的那种（按每通道 16 级分桶，取桶里的平均）
fn dominant(colors: &[Rgb]) -> Option<Rgb> {
    let key = |c: &Rgb| (c[0] as u32 >> 4) << 8 | (c[1] as u32 >> 4) << 4 | c[2] as u32 >> 4;
    let mut counts = std::collections::HashMap::<u32, (usize, Rgb)>::new();
    for c in colors {
        let e = counts.entry(key(c)).or_insert((0, [0.0; 3]));
        e.0 += 1;
        for (sum, v) in e.1.iter_mut().zip(c) {
            *sum += v;
        }
    }
    let (n, sum) = counts.into_values().max_by_key(|(n, _)| *n)?;
    Some([sum[0] / n as f64, sum[1] / n as f64, sum[2] / n as f64])
}

fn clamp_box(img: &RgbaImage, b: &Box2) -> (u32, u32, u32, u32) {
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    (
        b.x0.clamp(0.0, w) as u32,
        b.y0.clamp(0.0, h) as u32,
        b.x1.ceil().clamp(0.0, w) as u32,
        b.y1.ceil().clamp(0.0, h) as u32,
    )
}

fn inflate(b: &Box2, pad: f64) -> Box2 {
    Box2 {
        x0: b.x0 - pad,
        y0: b.y0 - pad,
        x1: b.x1 + pad,
        y1: b.y1 + pad,
    }
}

/// 字框往外留多少才盖得住字的边缘（识字的框常常贴着字、甚至切掉一点下伸部分）
fn erase_pad(b: &Box2) -> f64 {
    (b.height() * 0.18).clamp(2.0, 6.0)
}

/// 格子的底色：格子里避开字框、避开边上两三个像素（线）取样，最多的颜色
fn cell_background(img: &RgbaImage, cell: &Box2, text: &[Box2]) -> Rgb {
    let inner = inflate(cell, -3.0);
    let (x0, y0, x1, y1) = clamp_box(img, &inner);
    let step = ((x1.saturating_sub(x0)).min(y1.saturating_sub(y0)) / 24).max(1) as usize;
    let mut px = Vec::new();
    for y in (y0..y1).step_by(step) {
        for x in (x0..x1).step_by(step) {
            let (fx, fy) = (f64::from(x), f64::from(y));
            let in_text = text.iter().any(|b| {
                let p = erase_pad(b);
                fx >= b.x0 - p && fx <= b.x1 + p && fy >= b.y0 - p && fy <= b.y1 + p
            });
            if !in_text {
                px.push(rgb(img, x, y));
            }
        }
    }
    if px.is_empty() {
        // 字把格子塞满了：拿整格取
        for y in (y0..y1).step_by(step) {
            for x in (x0..x1).step_by(step) {
                px.push(rgb(img, x, y));
            }
        }
    }
    dominant(&px).unwrap_or([255.0; 3])
}

/// 一段字的颜色和笔画粗细（笔画横向宽多少像素）。离底色最远的那批像素的平均当字色
fn ink(img: &RgbaImage, b: &Box2, bg: Rgb) -> Option<(Rgb, f64)> {
    let (x0, y0, x1, y1) = clamp_box(img, b);
    if x1 <= x0 + 1 || y1 <= y0 + 1 {
        return None;
    }
    let far = (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .map(|(x, y)| dist(rgb(img, x, y), bg))
        .fold(0.0, f64::max);
    if far < 40.0 {
        return None;
    }
    let mut sum = [0.0; 3];
    let mut n = 0.0;
    let mut runs: Vec<u32> = Vec::new();
    for y in y0..y1 {
        let mut run = 0u32;
        for x in x0..x1 {
            let c = rgb(img, x, y);
            let d = dist(c, bg);
            if d >= far * 0.8 {
                for k in 0..3 {
                    sum[k] += c[k];
                }
                n += 1.0;
            }
            if d >= far * 0.45 {
                run += 1;
            } else if run > 0 {
                runs.push(run);
                run = 0;
            }
        }
        if run > 0 {
            runs.push(run);
        }
    }
    if n == 0.0 || runs.is_empty() {
        return None;
    }
    runs.sort_unstable();
    let stroke = f64::from(runs[runs.len() / 2]);
    Some(([sum[0] / n, sum[1] / n, sum[2] / n], stroke))
}

/// 一段字的墨迹从哪到哪（字号的倍数，以基线为 0、往上为负）：有大写、数字或带上伸部分的小写
/// （b d f h k l t）顶到大写高（约 0.73），只有 x 高的小写字母顶到 0.53；有下伸部分（g j p q y、括号）
/// 往下 0.21，只有逗号、分号往下 0.12；中日韩的字从 -0.80 到 0.08
pub(super) fn ink_extent(text: &str) -> (f64, f64) {
    if text.chars().any(is_cjk) {
        return (0.80, 0.08);
    }
    let top = if text.chars().any(|c| {
        c.is_ascii_uppercase() || c.is_ascii_digit() || "bdfhklt$()/[]{}|!?&%#@'\"\\".contains(c)
    }) {
        0.73
    } else {
        0.53
    };
    let down = if text.chars().any(|c| "gjpqy()[]{}|$@".contains(c)) {
        0.21
    } else if text.chars().any(|c| ",;".contains(c)) {
        0.12
    } else {
        0.0
    };
    (top, down)
}

/// 系统界面字体（苹方、SF）行框中心在基线上面多高（字号的倍数）
const LINE_CENTER: f64 = 0.355;

/// 同一列（表头和表身分开）字号差不多的统一成一样：原表格一列里通常是一个字号，量出来的会有一两个像素的出入
fn snap_sizes(cells: &mut [LaidCell]) {
    let groups: std::collections::BTreeSet<(usize, bool)> = cells
        .iter()
        .filter(|c| c.text_box.is_some() && c.col_span == 1)
        .map(|c| (c.col, c.header))
        .collect();
    for (col, header) in groups {
        let mut sizes: Vec<f64> = cells
            .iter()
            .filter(|c| {
                c.text_box.is_some() && c.col_span == 1 && c.col == col && c.header == header
            })
            .map(|c| c.font_px)
            .collect();
        sizes.sort_by(f64::total_cmp);
        let Some(&m) = sizes.get(sizes.len() / 2) else {
            continue;
        };
        for c in cells.iter_mut() {
            if c.text_box.is_none() || c.col_span != 1 || c.col != col || c.header != header {
                continue;
            }
            // 差不多的统一；差得太多的多半是量错了（字上压着东西），也按这列一般的来；按识字框估的本来就不准，
            // 也按这列一般的来。只有一两格的列不好说谁对，不动
            let off = (c.font_px - m).abs() / m;
            if off < 0.2 || (sizes.len() >= 3 && (!c.by_ink || off > 0.35)) {
                c.font_px = m;
            }
        }
    }
}

/// 认出来的网格 → 原位显示用的每一格
pub fn layout(img: &RgbaImage, grid: &Grid) -> LaidTable {
    // 先量：底色、字色、笔画宽（像素）、字的范围、字号
    struct Measured {
        bg: Rgb,
        fg: Rgb,
        stroke_px: Option<f64>,
        text_box: Option<Box2>,
        by_ink: bool,
        font_px: f64,
    }
    let measured: Vec<Measured> = grid
        .cells
        .iter()
        .map(|c| {
            let rect = grid.rect(c);
            let bg = cell_background(img, &rect, &c.boxes);
            let inks: Vec<(Rgb, f64)> = c.boxes.iter().filter_map(|b| ink(img, b, bg)).collect();
            let fg = if inks.is_empty() {
                // 没字的格子：字色按底色深浅给（改格子时新打的字要看得见）
                if bg.iter().sum::<f64>() / 3.0 > 128.0 {
                    [20.0; 3]
                } else {
                    [245.0; 3]
                }
            } else {
                let n = inks.len() as f64;
                inks.iter().fold([0.0; 3], |a, (c, _)| {
                    [a[0] + c[0] / n, a[1] + c[1] / n, a[2] + c[2] / n]
                })
            };
            let mut strokes: Vec<f64> = inks.iter().map(|(_, s)| *s).collect();
            strokes.sort_by(f64::total_cmp);
            let stroke_px = strokes.get(strokes.len() / 2).copied();
            let glyphs: Vec<Box2> = c
                .boxes
                .iter()
                .filter_map(|b| glyph_ink(img, b, bg))
                .map(|(x0, y0, x1, y1)| Box2 {
                    x0: f64::from(x0),
                    y0: f64::from(y0),
                    x1: f64::from(x1),
                    y1: f64::from(y1),
                })
                .collect();
            // 字的范围按墨迹算（识字的框有高有矮）。量不到墨迹、或者墨迹比识字框大出一大截
            // （字上压着图标、挨着别的图形，连成了一片）的，用识字框
            let ocr_box = c.boxes.iter().copied().reduce(|a, b| a.union(&b));
            let ink_box = glyphs.iter().copied().reduce(|a, b| a.union(&b));
            let by_ink = match (ink_box, ocr_box) {
                (Some(i), Some(o)) => {
                    i.height() <= o.height() * 1.6 && i.width() <= o.width() * 1.25 + 4.0
                }
                _ => false,
            };
            let (top, down) = ink_extent(&c.text);
            let mut heights: Vec<f64> = if by_ink {
                glyphs.iter().map(Box2::height).collect()
            } else {
                // 识字框的高大约是字号的 1.25 倍
                c.boxes
                    .iter()
                    .map(|b| b.height() * 0.8 * (top + down))
                    .collect()
            };
            heights.sort_by(f64::total_cmp);
            let ink_h = heights
                .get(heights.len() / 2)
                .copied()
                .unwrap_or(rect.height() * 0.4);
            Measured {
                bg,
                fg,
                stroke_px,
                text_box: if by_ink { ink_box } else { ocr_box },
                by_ink,
                font_px: (ink_h / (top + down).max(0.3)).max(6.0),
            }
        })
        .collect();
    let mut cells: Vec<LaidCell> = grid
        .cells
        .iter()
        .zip(&measured)
        .map(|(c, m)| {
            let rect = grid.rect(c);
            let (top, down) = ink_extent(&c.text);
            let align = match m.text_box {
                Some(t) => {
                    let (gl, gr) = (t.x0 - rect.x0, rect.x1 - t.x1);
                    if (gl - gr).abs() <= (rect.width() * 0.12).max(4.0) {
                        "center"
                    } else if gl < gr {
                        "left"
                    } else {
                        "right"
                    }
                }
                None => "left",
            };
            LaidCell {
                row: c.row,
                col: c.col,
                row_span: c.row_span,
                col_span: c.col_span,
                header: c.row == 0,
                text: c.text.clone(),
                x: rect.x0,
                y: rect.y0,
                width: rect.width(),
                height: rect.height(),
                text_box: m.text_box.map(|t| [t.x0, t.y0, t.width(), t.height()]),
                lines: text_lines(&c.boxes).len().max(1),
                font_px: m.font_px,
                center_shift: (down - top) / 2.0 + LINE_CENTER,
                color: hex(m.fg),
                background: hex(m.bg),
                bold: false,
                align,
                by_ink: m.by_ink,
                stroke: None,
                italic: false,
                mono: false,
            }
        })
        .collect();
    // 大小写、斜体、等宽：按字形量（只看一行、按墨迹量的）
    for (c, m) in cells.iter_mut().zip(&measured) {
        let (Some(t), true, 1) = (m.text_box, m.by_ink, c.lines) else {
            continue;
        };
        let b = clamp_box(img, &t);
        let style = super::glyphs::analyze(img, b, m.bg, &c.text);
        if let Some(text) = style.text {
            c.text = text;
        }
        c.italic = style.italic;
        c.mono = style.mono;
    }
    snap_sizes(&mut cells);
    // 笔画宽 ÷ 字号：粗体的明显比一般的粗
    for (c, m) in cells.iter_mut().zip(&measured) {
        c.stroke = m.stroke_px.map(|s| s / c.font_px.max(1.0));
    }
    let mut all: Vec<f64> = cells.iter().filter_map(|c| c.stroke).collect();
    all.sort_by(f64::total_cmp);
    let typical = all.get(all.len() / 2).copied().unwrap_or(0.0);
    for c in cells.iter_mut() {
        c.bold = typical > 0.0 && c.stroke.is_some_and(|s| s > typical * 1.3);
    }
    // 一列（表身）里绝大多数是一种粗细：个别格子量出来不一样、笔画又和这列一般的差不多，就当量错了
    for col in 0..grid.cols() {
        let idx: Vec<usize> = (0..cells.len())
            .filter(|&i| cells[i].col == col && cells[i].row > 0 && cells[i].stroke.is_some())
            .collect();
        if idx.len() < 3 {
            continue;
        }
        let bolds = idx.iter().filter(|&&i| cells[i].bold).count();
        let majority = bolds * 2 > idx.len();
        let minority = if majority { idx.len() - bolds } else { bolds };
        // 和大多数不一样的只有零星几格（不到一成半）：除非笔画粗细差得非常明显，都按大多数来
        let few = minority as f64 <= (idx.len() as f64 * 0.15).max(1.0);
        let mut strokes: Vec<f64> = idx.iter().filter_map(|&i| cells[i].stroke).collect();
        strokes.sort_by(f64::total_cmp);
        let m = strokes[strokes.len() / 2];
        for &i in &idx {
            let k = cells[i].stroke.unwrap_or(m) / m;
            let close = k > 0.75 && k < 1.35;
            let not_obvious = k > 0.55 && k < 1.8;
            if cells[i].bold != majority && (close || (few && not_obvious)) {
                cells[i].bold = majority;
            }
        }
    }
    LaidTable {
        width: img.width(),
        height: img.height(),
        rows: grid.rows(),
        cols: grid.cols(),
        cells,
    }
}

/// 擦字的范围：字的墨迹再往外留一点（字边上那圈淡淡的抗锯齿，大字宽一些）；找不到笔画就用识字框
fn glyph_area(img: &RgbaImage, b: &Box2, bg: Rgb) -> (u32, u32, u32, u32) {
    let (iw, ih) = img.dimensions();
    let m = (b.height() * 0.06).round().max(1.0) as u32;
    match glyph_ink(img, b, bg) {
        Some((x0, y0, x1, y1)) => (
            x0.saturating_sub(m),
            y0.saturating_sub(m),
            (x1 + m).min(iw),
            (y1 + m).min(ih),
        ),
        None => clamp_box(img, &inflate(b, 2.0)),
    }
}

/// 字真正占的范围（墨迹）：在识字框中间那部分找笔画，顺着连在一起的像素找全（框常常比字矮，或者高到碰着行线），
/// 细长的线（表格线）和横穿整个范围的不算
fn glyph_ink(img: &RgbaImage, b: &Box2, bg: Rgb) -> Option<(u32, u32, u32, u32)> {
    let h = b.height().max(1.0);
    let window = Box2 {
        x0: b.x0 - h * 0.4,
        x1: b.x1 + h * 0.4,
        y0: b.y0 - h * 0.6,
        y1: b.y1 + h * 0.6,
    };
    let (wx0, wy0, wx1, wy1) = clamp_box(img, &window);
    let core = Box2 {
        y0: b.y0 + h * 0.15,
        y1: b.y1 - h * 0.15,
        ..*b
    };
    let (cx0, cy0, cx1, cy1) = clamp_box(img, &core);
    if wx1 <= wx0 || wy1 <= wy0 || cx1 <= cx0 || cy1 <= cy0 {
        return None;
    }
    let (ww, wh) = ((wx1 - wx0) as usize, (wy1 - wy0) as usize);
    let ink: Vec<bool> = (wy0..wy1)
        .flat_map(|y| (wx0..wx1).map(move |x| (x, y)))
        .map(|(x, y)| dist(rgb(img, x, y), bg) > 45.0)
        .collect();
    let mut seen = vec![false; ww * wh];
    // 从 (sx, sy) 起顺着连在一起的墨迹找完一整笔，给它的范围（含两头）；这里不是墨迹、或者已经找过了给 None
    let mut fill = |sx: u32, sy: u32| -> Option<(u32, u32, u32, u32)> {
        let start = (sy - wy0) as usize * ww + (sx - wx0) as usize;
        if !ink[start] || seen[start] {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (sx, sy, sx, sy);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % ww) as u32 + wx0, (i / ww) as u32 + wy0);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (i64::from(x) + dx, i64::from(y) + dy);
                    if nx < i64::from(wx0)
                        || ny < i64::from(wy0)
                        || nx >= i64::from(wx1)
                        || ny >= i64::from(wy1)
                    {
                        continue;
                    }
                    let j = (ny - i64::from(wy0)) as usize * ww + (nx - i64::from(wx0)) as usize;
                    if ink[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        Some((x0, y0, x1, y1))
    };
    // 细长的（表格线、下划线以外的长横线）和横穿整个范围的：不是字
    let not_glyph = |(x0, y0, x1, y1): (u32, u32, u32, u32)| {
        let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
        let thin_line =
            (cw >= ch * 4 && ch <= 4) || (ch >= cw * 4 && cw <= 4 && f64::from(ch) > h * 1.1);
        let spans = (x0 == wx0 && x1 + 1 == wx1) || (y0 == wy0 && y1 + 1 == wy1);
        thin_line || spans
    };
    let mut area: Option<(u32, u32, u32, u32)> = None;
    for sy in cy0..cy1 {
        for sx in cx0..cx1 {
            let Some(c) = fill(sx, sy) else { continue };
            if not_glyph(c) {
                continue;
            }
            let (x0, y0, x1, y1) = c;
            area = Some(match area {
                None => (x0, y0, x1 + 1, y1 + 1),
                Some((a, b2, c, d)) => (a.min(x0), b2.min(y0), c.max(x1 + 1), d.max(y1 + 1)),
            });
        }
    }
    // 和字身分开、浮在上面的小块也是字的一部分：i / j 的点、重音符号、汉字顶上单独的一点。
    // 不算进来的话字的高度量矮了（字号偏小），抹字时也会漏掉它们。只认紧挨着字顶、横向落在字的范围里的小块
    let (ax0, ay0, ax1, ay1) = area?;
    let ah = f64::from(ay1 - ay0);
    let reach = ay0.saturating_sub((ah * 0.35).ceil() as u32).max(wy0);
    let mut top = ay0;
    for sy in reach..ay0 {
        for sx in ax0..ax1 {
            let Some(c) = fill(sx, sy) else { continue };
            let (x0, y0, x1, y1) = c;
            let small = f64::from(y1 - y0 + 1) <= ah * 0.4;
            let attached = y1 + 1 >= ay0 || f64::from(ay0 - (y1 + 1)) <= ah * 0.3;
            if small && attached && x0 >= ax0.saturating_sub(2) && x1 < ax1 + 2 && !not_glyph(c) {
                top = top.min(y0);
            }
        }
    }
    Some((ax0, top, ax1, ay1))
}

/// 一块区域（公式）的底色和字色 #rrggbb：原位显示公式时铺底、上字
pub fn region_colors(img: &RgbaImage) -> (String, String) {
    let (w, h) = img.dimensions();
    let step = (w.min(h) / 48).max(1) as usize;
    let px: Vec<Rgb> = (0..h)
        .step_by(step)
        .flat_map(|y| (0..w).step_by(step).map(move |x| (x, y)))
        .map(|(x, y)| rgb(img, x, y))
        .collect();
    let bg = dominant(&px).unwrap_or([255.0; 3]);
    let whole = Box2 {
        x0: 0.0,
        y0: 0.0,
        x1: f64::from(w),
        y1: f64::from(h),
    };
    let fg =
        ink(img, &whole, bg)
            .map(|(c, _)| c)
            .unwrap_or(if bg.iter().sum::<f64>() / 3.0 > 128.0 {
                [20.0; 3]
            } else {
                [245.0; 3]
            });
    (hex(bg), hex(fg))
}

/// 把表格里的字都抹掉（原位显示的底图）：每个字框按它那一格的底色擦
pub fn erase_grid(img: &RgbaImage, grid: &Grid) -> RgbaImage {
    let items: Vec<(Box2, Rgb)> = grid
        .cells
        .iter()
        .flat_map(|c| {
            let bg = cell_background(img, &grid.rect(c), &c.boxes);
            c.boxes.iter().map(move |b| (*b, bg))
        })
        .collect();
    erase_text(img, &items)
}

/// 把字抹掉：按字真正占的范围（`glyph_area`），每一列从上沿往下沿插值填色，范围外的像素不动。
/// 竖着穿过范围的线（上下沿颜色一样）会留下来；上下沿取样碰到了别的字（离底色远）就改用底色 `bg`
fn erase_text(img: &RgbaImage, items: &[(Box2, Rgb)]) -> RgbaImage {
    let mut out = img.clone();
    let (w, h) = img.dimensions();
    for &(b, bg) in items {
        let b = &b;
        let (x0, y0, x1, y1) = glyph_area(img, b, bg);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let sample = |x: u32, ys: &[i64]| -> Option<Rgb> {
            let picks: Vec<Rgb> = ys
                .iter()
                .filter(|&&y| y >= 0 && y < i64::from(h))
                .map(|&y| rgb(img, x, y as u32))
                .collect();
            dominant(&picks)
        };
        let fix = |c: Option<Rgb>, other: Option<Rgb>| -> Rgb {
            match (c, other) {
                (Some(c), _) if dist(c, bg) <= 50.0 => c,
                // 离底色远、但上下一样：竖线，留着
                (Some(c), Some(o)) if dist(c, o) <= 30.0 => c,
                _ => bg,
            }
        };
        let (top_ys, bottom_ys): (Vec<i64>, Vec<i64>) = (
            (1..=2).map(|d| i64::from(y0) - d).collect(),
            (0..2).map(|d| i64::from(y1) + d).collect(),
        );
        let mut tops = Vec::with_capacity((x1 - x0) as usize);
        let mut bottoms = Vec::with_capacity((x1 - x0) as usize);
        for x in x0..x1 {
            let (t, bt) = (sample(x, &top_ys), sample(x, &bottom_ys));
            tops.push(fix(t, bt));
            bottoms.push(fix(bt, t));
        }
        let span = f64::from((y1 - y0).max(1));
        for (i, x) in (x0..x1).enumerate() {
            for y in y0..y1 {
                let k = f64::from(y - y0) / span;
                let c: Vec<u8> = (0..3)
                    .map(|j| {
                        (tops[i][j] * (1.0 - k) + bottoms[i][j] * k)
                            .round()
                            .clamp(0.0, 255.0) as u8
                    })
                    .collect();
                if x < w && y < h {
                    out.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 画一张白底、灰线的表：`rows` × `cols` 格，每格 cw × ch
    fn grid(rows: u32, cols: u32, cw: u32, ch: u32) -> RgbaImage {
        let (w, h) = (cols * cw + 21, rows * ch + 21);
        let mut img = RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
        for r in 0..=rows {
            for x in 10..=10 + cols * cw {
                img.put_pixel(x, 10 + r * ch, image::Rgba([150, 150, 150, 255]));
            }
        }
        for c in 0..=cols {
            for y in 10..=10 + rows * ch {
                img.put_pixel(10 + c * cw, y, image::Rgba([150, 150, 150, 255]));
            }
        }
        img
    }

    #[test]
    fn glyph_ink_counts_the_dot_above_an_i() {
        // 一竖（字身）加上面隔开两像素的一个点；识字框只框住字身
        let mut img = RgbaImage::from_pixel(60, 50, image::Rgba([255, 255, 255, 255]));
        let black = image::Rgba([20, 20, 20, 255]);
        for y in 18..34 {
            for x in 28..32 {
                img.put_pixel(x, y, black);
            }
        }
        for y in 12..16 {
            for x in 28..32 {
                img.put_pixel(x, y, black);
            }
        }
        // 离得远的、别的行的字不算
        for y in 1..4 {
            for x in 28..32 {
                img.put_pixel(x, y, black);
            }
        }
        let b = Box2 {
            x0: 26.0,
            y0: 18.0,
            x1: 34.0,
            y1: 34.0,
        };
        let (_, top, _, bottom) = glyph_ink(&img, &b, [255.0; 3]).unwrap();
        assert_eq!((top, bottom), (12, 34));
    }

    fn block(text: &str, x: f64, y: f64, w: f64, h: f64) -> OcrBlock {
        OcrBlock {
            text: text.into(),
            quad: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            score: 1.0,
        }
    }

    #[test]
    fn finds_grid_lines() {
        let l = ruling_lines(&grid(4, 3, 100, 30));
        assert_eq!(l.rows, vec![10, 40, 70, 100, 130]);
        assert_eq!(l.cols, vec![10, 110, 210, 310]);
        assert!(looks_like_table(&grid(4, 3, 100, 30)));
        let blank = RgbaImage::from_pixel(300, 200, image::Rgba([255, 255, 255, 255]));
        assert!(!looks_like_table(&blank));
    }

    #[test]
    fn places_text_into_cells_and_splits_lines_across_columns() {
        let img = grid(3, 3, 100, 30);
        let blocks = [
            block("序号", 20.0, 18.0, 30.0, 14.0),
            block("IP", 120.0, 18.0, 20.0, 14.0),
            block("速度", 220.0, 18.0, 30.0, 14.0),
            // 系统识字把一整行认成了一段：按词的位置分到三列
            block("1 139.203.170.244 170.07", 20.0, 48.0, 270.0, 14.0),
            block("2", 20.0, 78.0, 10.0, 14.0),
            block("183.93.93.125", 120.0, 78.0, 80.0, 14.0),
            block("91.52", 220.0, 78.0, 40.0, 14.0),
        ];
        let t = build(&img, &blocks).unwrap();
        let texts: Vec<Vec<&str>> = t
            .iter()
            .map(|r| r.iter().map(|c| c.text.as_str()).collect())
            .collect();
        assert_eq!(
            texts,
            vec![
                vec!["序号", "IP", "速度"],
                vec!["1", "139.203.170.244", "170.07"],
                vec!["2", "183.93.93.125", "91.52"],
            ]
        );
        assert!(t[0].iter().all(|c| c.header));
    }

    #[test]
    fn missing_line_segment_means_merged_cell() {
        let mut img = grid(3, 3, 100, 30);
        // 第一行第一、二格之间的竖线擦掉 → 这两格合并
        for y in 11..40 {
            img.put_pixel(110, y, image::Rgba([255, 255, 255, 255]));
        }
        let blocks = [
            block("标题", 60.0, 18.0, 40.0, 14.0),
            block("C", 220.0, 18.0, 10.0, 14.0),
            block("a", 20.0, 48.0, 10.0, 14.0),
            block("b", 120.0, 48.0, 10.0, 14.0),
            block("c", 220.0, 48.0, 10.0, 14.0),
            block("d", 20.0, 78.0, 10.0, 14.0),
        ];
        let t = build(&img, &blocks).unwrap();
        assert_eq!(t[0].len(), 2);
        assert_eq!(t[0][0].col_span, 2);
        assert_eq!(t[0][0].text, "标题");
    }

    #[test]
    fn text_straddling_a_missing_line_is_one_merged_cell() {
        let mut img = grid(3, 3, 100, 30);
        // 第一行第二、三格之间没有竖线，"2025 年" 横跨这条分界：按空格拆开会落进两格，其实是一格
        for y in 11..40 {
            img.put_pixel(210, y, image::Rgba([255, 255, 255, 255]));
        }
        let blocks = [
            block("地区", 30.0, 18.0, 40.0, 14.0),
            block("2025 年", 175.0, 18.0, 70.0, 14.0),
            block("a", 20.0, 48.0, 10.0, 14.0),
            block("b", 120.0, 48.0, 10.0, 14.0),
            block("c", 220.0, 48.0, 10.0, 14.0),
            block("d", 20.0, 78.0, 10.0, 14.0),
        ];
        let t = build(&img, &blocks).unwrap();
        assert_eq!(t[0].len(), 2);
        assert_eq!(t[0][1].col_span, 2);
        assert_eq!(t[0][1].text, "2025 年");
    }

    #[test]
    fn two_lines_without_vertical_rules_are_not_a_table() {
        // 公式：分子、分数线、分母，左边一个 "x ="
        let mut img = RgbaImage::from_pixel(300, 80, image::Rgba([255, 255, 255, 255]));
        for x in 60..290 {
            img.put_pixel(x, 40, image::Rgba([20, 20, 20, 255]));
        }
        let blocks = [
            block("x =", 10.0, 32.0, 30.0, 14.0),
            block("-b ± √b² - 4ac", 70.0, 15.0, 200.0, 18.0),
            block("2a", 150.0, 50.0, 30.0, 18.0),
        ];
        assert!(build(&img, &blocks).is_none());
    }

    #[test]
    fn borderless_table_from_text_alignment() {
        let img = RgbaImage::from_pixel(400, 100, image::Rgba([255, 255, 255, 255]));
        let blocks = [
            block("Name", 10.0, 10.0, 40.0, 12.0),
            block("Score", 200.0, 10.0, 40.0, 12.0),
            block("Alice", 10.0, 40.0, 40.0, 12.0),
            block("90", 200.0, 40.0, 20.0, 12.0),
            block("Bob", 10.0, 70.0, 30.0, 12.0),
            block("85", 200.0, 70.0, 20.0, 12.0),
        ];
        let t = build(&img, &blocks).unwrap();
        let texts: Vec<Vec<&str>> = t
            .iter()
            .map(|r| r.iter().map(|c| c.text.as_str()).collect())
            .collect();
        assert_eq!(
            texts,
            vec![
                vec!["Name", "Score"],
                vec!["Alice", "90"],
                vec!["Bob", "85"]
            ]
        );
    }

    #[test]
    fn rows_between_header_lines_are_split_by_text() {
        // 只有表头上下两条线，下面三行字靠底色区分：按字行切成三行，不能当成一行（也不能当成合并单元格）
        let mut img = RgbaImage::from_pixel(400, 160, image::Rgba([255, 255, 255, 255]));
        for y in [10u32, 40] {
            for x in 0..400 {
                img.put_pixel(x, y, image::Rgba([20, 30, 60, 255]));
            }
        }
        let blocks = [
            block("Name", 10.0, 18.0, 40.0, 14.0),
            block("Score", 250.0, 18.0, 40.0, 14.0),
            block("Alice", 10.0, 55.0, 40.0, 14.0),
            block("90", 250.0, 55.0, 20.0, 14.0),
            block("Bob", 10.0, 85.0, 30.0, 14.0),
            block("85", 250.0, 85.0, 20.0, 14.0),
            block("Cara", 10.0, 115.0, 30.0, 14.0),
            block("77", 250.0, 115.0, 20.0, 14.0),
        ];
        let t = build(&img, &blocks).unwrap();
        let texts: Vec<Vec<&str>> = t
            .iter()
            .map(|r| r.iter().map(|c| c.text.as_str()).collect())
            .collect();
        assert_eq!(
            texts,
            vec![
                vec!["Name", "Score"],
                vec!["Alice", "90"],
                vec!["Bob", "85"],
                vec!["Cara", "77"]
            ]
        );
    }

    #[test]
    fn wrapped_text_in_one_cell_is_not_split_into_rows() {
        let mut img = RgbaImage::from_pixel(400, 120, image::Rgba([255, 255, 255, 255]));
        for y in [10u32, 40, 110] {
            for x in 0..400 {
                img.put_pixel(x, y, image::Rgba([120, 120, 120, 255]));
            }
        }
        let blocks = [
            block("Item", 10.0, 18.0, 40.0, 14.0),
            block("Note", 250.0, 18.0, 40.0, 14.0),
            block("Apple", 10.0, 55.0, 40.0, 14.0),
            block("first line", 250.0, 55.0, 80.0, 14.0),
            block("second line", 250.0, 80.0, 90.0, 14.0),
        ];
        let t = build(&img, &blocks).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[1][1].text, "first line second line");
    }

    #[test]
    fn fullwidth_brackets_around_latin_in_chinese_become_ascii() {
        assert_eq!(super::ascii_brackets("预算（$）"), "预算 ($)");
        assert_eq!(super::ascii_brackets("面积（平方公里）"), "面积（平方公里）");
        assert_eq!(super::ascii_brackets("单位 （km）"), "单位 (km)");
    }

    #[test]
    fn fullwidth_brackets_become_ascii_in_latin_text() {
        assert_eq!(
            join_words(["Budgeted", "（$）"].into_iter().map(String::from)),
            "Budgeted ($)"
        );
        assert_eq!(
            join_words(["金额", "（元）"].into_iter().map(String::from)),
            "金额（元）"
        );
    }

    #[test]
    fn erasing_text_keeps_background_and_vertical_lines() {
        let mut img = RgbaImage::from_pixel(60, 30, image::Rgba([240, 240, 250, 255]));
        // 一条竖线穿过字框
        for y in 0..30 {
            img.put_pixel(30, y, image::Rgba([100, 100, 100, 255]));
        }
        // 字
        for x in 10..25 {
            for y in 12..18 {
                img.put_pixel(x, y, image::Rgba([10, 10, 10, 255]));
            }
        }
        let out = erase_text(
            &img,
            &[(
                Box2 {
                    x0: 10.0,
                    y0: 12.0,
                    x1: 40.0,
                    y1: 18.0,
                },
                [240.0, 240.0, 250.0],
            )],
        );
        assert_eq!(out.get_pixel(15, 15).0, [240, 240, 250, 255]);
        assert_eq!(out.get_pixel(30, 15).0, [100, 100, 100, 255]);
    }

    #[test]
    fn layout_reads_colors_bold_and_alignment() {
        // 深底白字的表头 + 白底黑字；右边一列数字靠右
        let mut img = RgbaImage::from_pixel(300, 90, image::Rgba([255, 255, 255, 255]));
        for y in 0..30 {
            for x in 0..300 {
                img.put_pixel(x, y, image::Rgba([20, 34, 61, 255]));
            }
        }
        for y in [30u32, 60] {
            for x in 0..300 {
                img.put_pixel(x, y, image::Rgba([200, 200, 200, 255]));
            }
        }
        let mut glyph = |x0: u32, y0: u32, w: u32, stroke: u32, c: [u8; 4]| {
            for x in x0..x0 + w {
                if (x - x0) % 6 < stroke {
                    for y in y0..y0 + 12 {
                        img.put_pixel(x, y, image::Rgba(c));
                    }
                }
            }
        };
        glyph(10, 9, 40, 2, [255, 255, 255, 255]);
        glyph(240, 9, 40, 2, [255, 255, 255, 255]);
        glyph(10, 39, 40, 1, [0, 0, 0, 255]);
        glyph(250, 39, 40, 3, [0, 0, 0, 255]);
        glyph(10, 69, 40, 1, [0, 0, 0, 255]);
        glyph(250, 69, 40, 3, [0, 0, 0, 255]);
        let blocks = [
            block("Name", 10.0, 9.0, 40.0, 12.0),
            block("Total", 240.0, 9.0, 40.0, 12.0),
            block("a", 10.0, 39.0, 40.0, 12.0),
            block("1.00", 250.0, 39.0, 40.0, 12.0),
            block("b", 10.0, 69.0, 40.0, 12.0),
            block("2.00", 250.0, 69.0, 40.0, 12.0),
        ];
        let g = super::grid(&img, &blocks).unwrap();
        let t = layout(&img, &g);
        let at = |r: usize, c: usize| t.cells.iter().find(|x| x.row == r && x.col == c).unwrap();
        assert_eq!(at(0, 0).background, "#14223d");
        assert_eq!(at(0, 0).color, "#ffffff");
        assert_eq!(at(1, 0).color, "#000000");
        assert_eq!(at(1, 1).align, "right");
        assert!(at(1, 1).bold && !at(1, 0).bold);
    }

    #[test]
    fn html_escapes_and_spans() {
        let rows = vec![vec![TableCell {
            text: "a<b".into(),
            header: true,
            row_span: 1,
            col_span: 2,
        }]];
        assert_eq!(
            to_html(&rows),
            "<table><tr><th colspan=\"2\">a&lt;b</th></tr></table>"
        );
    }
}

#[cfg(test)]
mod smoke {
    /// 真图 + 系统识字：`CHENOCR_SMOKE_TABLE=表格截图.png cargo test --lib -- --ignored smoke_table --nocapture`
    #[test]
    #[ignore]
    fn smoke_table() {
        let path = std::env::var("CHENOCR_SMOKE_TABLE").unwrap();
        let img = image::open(path).unwrap().to_rgba8();
        let started = std::time::Instant::now();
        let lines = super::ruling_lines(&img);
        println!(
            "线：{} 横 {} 竖，{}ms",
            lines.rows.len(),
            lines.cols.len(),
            started.elapsed().as_millis()
        );
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
        for b in &blocks {
            println!(
                "  [{:.0},{:.0} {:.0}] {}",
                b.quad[0][0],
                b.quad[0][1],
                b.quad[1][0] - b.quad[0][0],
                b.text
            );
        }
        let g = super::grid(&img, &blocks).unwrap();
        let laid = super::layout(&img, &g);
        for c in laid.cells.iter().take(40) {
            println!(
                "  格 {},{} 底 {} 字 {} 粗 {} 笔画 {:.3} {} 字号 {:.1} 墨迹 {} 「{}」",
                c.row,
                c.col,
                c.background,
                c.color,
                c.bold,
                c.stroke.unwrap_or(0.0),
                c.align,
                c.font_px,
                c.by_ink,
                c.text
            );
        }
        if let Ok(out) = std::env::var("CHENOCR_SMOKE_OUT") {
            super::erase_grid(&img, &g).save(out).unwrap();
        }
        let t = g.table();
        for row in &t {
            println!(
                "{}",
                row.iter()
                    .map(|c| format!(
                        "{}{}",
                        c.text,
                        if c.col_span > 1 {
                            format!("(×{})", c.col_span)
                        } else {
                            String::new()
                        }
                    ))
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
        }
        println!("共 {}ms", started.elapsed().as_millis());
    }
}
