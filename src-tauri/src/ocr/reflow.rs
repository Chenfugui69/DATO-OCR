//! 段落重组（规格 04 §3）—— 决定识字体验好坏的关键一步。
//!
//! 引擎给的是一堆零散文本框，直接拼起来断行混乱。这里按几何关系还原：
//!
//! 1. 归一化：四点框 → 轴对齐包围盒
//! 2. 行聚类：**垂直重叠率**（重叠高度 / 较矮者高度）> 0.5 视为同一行。
//!    不用"中心 y 差"，因为同一行里字号可能不同
//! 3. 行内拼接：按间距决定"直接连 / 空格 / 制表符"，中文之间不加空格
//! 4. 段落聚类：行距不大、上一行被撑满（换行是被动的）、左边缘对齐或首行缩进 → 同段
//!
//! TODO(双栏)：对 block 的 x 中心做一维聚类，若明显两簇且中间有纵向空白带，拆成左右
//! 两栏分别处理（左栏在前）。第一版未做，双栏排版会被逐行横向合并。

use serde::{Deserialize, Serialize};

use crate::storage::tokenize::is_cjk;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OcrBlock {
    pub text: String,
    /// 四个角点（左上、右上、右下、左下），物理像素
    #[serde(rename = "box")]
    pub quad: [[f64; 2]; 4],
    pub score: f64,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn right(&self) -> f64 {
        self.x + self.w
    }
    fn bottom(&self) -> f64 {
        self.y + self.h
    }
    fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect {
            x,
            y,
            w: self.right().max(o.right()) - x,
            h: self.bottom().max(o.bottom()) - y,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub text: String,
    pub bbox: Rect,
    pub block_indices: Vec<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paragraph {
    pub text: String,
    pub lines: Vec<Line>,
    pub bbox: Rect,
    pub block_indices: Vec<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reflowed {
    pub paragraphs: Vec<Paragraph>,
    /// 段落间空行、段内不换行。默认复制的内容
    pub plain_text: String,
    /// 保留原始每行换行
    pub raw_text: String,
}

fn aabb(quad: &[[f64; 2]; 4]) -> Rect {
    let xs = quad.iter().map(|p| p[0]);
    let ys = quad.iter().map(|p| p[1]);
    let x0 = xs.clone().fold(f64::MAX, f64::min);
    let x1 = xs.fold(f64::MIN, f64::max);
    let y0 = ys.clone().fold(f64::MAX, f64::min);
    let y1 = ys.fold(f64::MIN, f64::max);
    Rect {
        x: x0,
        y: y0,
        w: (x1 - x0).max(1.0),
        h: (y1 - y0).max(1.0),
    }
}

fn vertical_overlap(a: &Rect, b: &Rect) -> f64 {
    let overlap = a.bottom().min(b.bottom()) - a.y.max(b.y);
    overlap.max(0.0) / a.h.min(b.h).max(1.0)
}

fn char_width(text: &str, rect: &Rect) -> f64 {
    // CJK 字符按 1 个字宽、拉丁字符按半个估算，避免英文行的平均字宽被低估一半
    let units: f64 = text
        .chars()
        .map(|c| {
            if is_cjk(c) || !c.is_ascii() {
                1.0
            } else {
                0.55
            }
        })
        .sum();
    rect.w / units.max(1.0)
}

fn needs_space(prev: &str, next: &str) -> bool {
    let a = prev.chars().last();
    let b = next.chars().next();
    match (a, b) {
        (Some(a), Some(b)) => {
            !(is_cjk(a) || is_cjk(b) || !a.is_ascii() || !b.is_ascii()) && !a.is_whitespace()
        }
        _ => false,
    }
}

pub fn reflow(blocks: &[OcrBlock]) -> Reflowed {
    let rects: Vec<Rect> = blocks.iter().map(|b| aabb(&b.quad)).collect();
    let mut order: Vec<usize> = (0..blocks.len())
        .filter(|&i| !blocks[i].text.trim().is_empty())
        .collect();
    order.sort_by(|&a, &b| rects[a].y.total_cmp(&rects[b].y));

    // ── 行聚类
    let mut lines: Vec<(Rect, Vec<usize>)> = Vec::new();
    for i in order {
        let r = rects[i];
        // 只和最近几行比较：行按 y 递增生成，更早的行不可能重叠
        let found = lines
            .iter_mut()
            .rev()
            .take(4)
            .find(|(lr, _)| vertical_overlap(lr, &r) > 0.5);
        match found {
            Some((lr, members)) => {
                *lr = lr.union(&r);
                members.push(i);
            }
            None => lines.push((r, vec![i])),
        }
    }
    lines.sort_by(|a, b| a.0.y.total_cmp(&b.0.y));

    // ── 行内拼接
    let built: Vec<Line> = lines
        .into_iter()
        .map(|(bbox, mut members)| {
            members.sort_by(|&a, &b| rects[a].x.total_cmp(&rects[b].x));
            let mut text = String::new();
            for (k, &i) in members.iter().enumerate() {
                let t = blocks[i].text.trim();
                if k > 0 {
                    let prev = members[k - 1];
                    let gap = rects[i].x - rects[prev].right();
                    let cw = char_width(&blocks[prev].text, &rects[prev]);
                    if gap >= cw * 2.0 {
                        text.push('\t');
                    } else if gap >= cw * 0.3 && needs_space(&text, t) {
                        text.push(' ');
                    }
                }
                text.push_str(t);
            }
            Line {
                text,
                bbox,
                block_indices: members,
            }
        })
        .collect();

    // ── 段落聚类
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    for line in built {
        let merge = paragraphs.last().is_some_and(|p| {
            let Some(prev) = p.lines.last() else {
                return false;
            };
            let h = prev.bbox.h;
            let cw = char_width(&prev.text, &prev.bbox);
            let close = line.bbox.y - prev.bbox.y <= h * 1.7 && line.bbox.y >= prev.bbox.y;
            let region_right = p.bbox.right().max(line.bbox.right());
            let filled = region_right - prev.bbox.right() < h * 2.0;
            let aligned = (line.bbox.x - prev.bbox.x).abs() < cw * 4.0;
            // 制表符分隔的是表格行，不合并成段
            let tabular = prev.text.contains('\t') || line.text.contains('\t');
            close && filled && aligned && !tabular
        });
        if merge {
            if let Some(p) = paragraphs.last_mut() {
                if needs_space(&p.text, &line.text) {
                    p.text.push(' ');
                }
                p.text.push_str(&line.text);
                p.bbox = p.bbox.union(&line.bbox);
                p.block_indices.extend(&line.block_indices);
                p.lines.push(line);
            }
        } else {
            paragraphs.push(Paragraph {
                text: line.text.clone(),
                bbox: line.bbox,
                block_indices: line.block_indices.clone(),
                lines: vec![line],
            });
        }
    }

    // 全是单行段落（界面截图、聊天记录）时段间只换一行，否则空一行
    let sep = if paragraphs.iter().all(|p| p.lines.len() == 1) {
        "\n"
    } else {
        "\n\n"
    };
    let plain_text = paragraphs
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join(sep);
    let raw_text = paragraphs
        .iter()
        .flat_map(|p| p.lines.iter().map(|l| l.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    Reflowed {
        paragraphs,
        plain_text,
        raw_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str, x: f64, y: f64, w: f64, h: f64) -> OcrBlock {
        OcrBlock {
            text: text.into(),
            quad: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            score: 0.99,
        }
    }

    #[test]
    fn multi_line_chinese_paragraph_has_no_inner_breaks() {
        let blocks = vec![
            block(
                "这是一段很长的中文文字，它在截图里",
                10.0,
                10.0,
                400.0,
                20.0,
            ),
            block("被自动换行成了三行，但其实属于同", 10.0, 38.0, 398.0, 20.0),
            block("一个段落。", 10.0, 66.0, 100.0, 20.0),
        ];
        let r = reflow(&blocks);
        assert_eq!(r.paragraphs.len(), 1);
        assert_eq!(
            r.plain_text,
            "这是一段很长的中文文字，它在截图里被自动换行成了三行，但其实属于同一个段落。"
        );
        assert_eq!(r.raw_text.lines().count(), 3);
    }

    #[test]
    fn two_paragraphs_are_split() {
        let blocks = vec![
            block("第一段第一行写满了整整一行的内容", 10.0, 10.0, 400.0, 20.0),
            block("第一段结束。", 10.0, 38.0, 120.0, 20.0),
            block("第二段从这里开始，也写满了一整行", 10.0, 90.0, 400.0, 20.0),
            block("第二段结束。", 10.0, 118.0, 120.0, 20.0),
        ];
        let r = reflow(&blocks);
        assert_eq!(r.paragraphs.len(), 2);
        assert_eq!(r.plain_text, "第一段第一行写满了整整一行的内容第一段结束。\n\n第二段从这里开始，也写满了一整行第二段结束。");
    }

    #[test]
    fn spaces_only_between_latin_words() {
        let blocks = vec![
            block("Hello", 10.0, 10.0, 50.0, 20.0),
            block("world", 66.0, 10.0, 50.0, 20.0),
            block("你好", 200.0, 50.0, 40.0, 20.0),
            block("世界", 244.0, 50.0, 40.0, 20.0),
        ];
        let r = reflow(&blocks);
        assert_eq!(r.paragraphs[0].text, "Hello world");
        assert_eq!(r.paragraphs[1].text, "你好世界");
    }

    #[test]
    fn mixed_font_sizes_on_one_line() {
        // 同一行里一个大字号块、一个小字号块，垂直重叠率仍 > 0.5
        let blocks = vec![
            block("标题", 10.0, 10.0, 60.0, 30.0),
            block("副标题", 74.0, 18.0, 54.0, 16.0),
        ];
        let r = reflow(&blocks);
        assert_eq!(r.paragraphs.len(), 1);
        assert_eq!(r.paragraphs[0].lines.len(), 1);
    }

    #[test]
    fn table_columns_use_tabs() {
        let blocks = vec![
            block("名称", 10.0, 10.0, 40.0, 20.0),
            block("数量", 200.0, 10.0, 40.0, 20.0),
            block("苹果", 10.0, 40.0, 40.0, 20.0),
            block("12", 200.0, 40.0, 20.0, 20.0),
        ];
        let r = reflow(&blocks);
        assert_eq!(r.raw_text, "名称\t数量\n苹果\t12");
        assert_eq!(r.paragraphs.len(), 2, "表格行不合并成段");
    }

    #[test]
    fn empty_input() {
        let r = reflow(&[]);
        assert!(r.paragraphs.is_empty());
        assert_eq!(r.plain_text, "");
    }
}
