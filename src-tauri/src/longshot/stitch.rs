//! 长截图拼接算法（规格 03 §4）。
//!
//! # 模型
//!
//! 每一帧 = 固定头部（悬浮导航栏）+ 可滚动内容区 + 固定尾部（底部输入框）。
//! 头尾高度在第一次检测到滚动时，用"两帧顶部/底部逐行相同"测出来，之后固定。
//!
//! 拼好的长图 = 第一帧的头部 + 内容切片序列 + 最后一帧的尾部。头尾只出现一次。
//!
//! 记录"当前视口在长图内容里的位置" `view_top`：向下滚 dy 行 `view_top += dy`，
//! 超出已有内容的部分从新帧底部追加；向上滚则 `view_top -= dy`，越过顶部的部分从新帧
//! 顶部插到最前。回滚到已经拍过的位置什么都不加 —— 所以往返滚动、从中间往两头滚都对。
//!
//! # 匹配
//!
//! 灰度 + 横向降采样到 ≤ 400 列（纵向保持全分辨率，位移精确到 1 行）。
//! 上一帧内容区底部取 40 行做模板（找"向下滚"），顶部再取一条（找"向上滚"）；
//! 模板方差太小（纯色）或没对上就换位置再试，最多 8 个位置。新帧里逐行滑窗：SAD 粗筛
//! 前 5 → NCC 精比，NCC ≥ 0.92 的候选再用**整段重叠区逐行比对**复核，挡掉重复图案（列表里
//! 一模一样的行）造成的错位。
//!
//! # 会变的内容
//!
//! 网页滚动时图片是懒加载的：上一帧还是灰色占位块，这一帧已经变成封面图。所以复核不要求
//! 重叠区处处相同，只要求**大部分行**对得上（`VERIFY_GOOD_FRACTION`）。并入时以新帧为准：
//! 新帧覆盖它和已有内容重叠的那一段，长图里留下的是最后加载好的样子；视口里固定位置的
//! 悬浮按钮（"回到顶部"之类）也因此只在最后一帧出现一次。
//!
//! # 连续跟拍
//!
//! 采集端在用户滚动的过程中不停地抓帧（不等滚轮停下），每帧都 `feed` 进来，和上一张
//! **已并入**的帧比位移。位移不到内容区一半的先记成候选不并入；位移够大就并入，成为新的
//! 比对基准（关键帧）。某一帧已经对不上了（一下滚出去太远），先把手上的候选并入再拿它
//! 重新比 —— 候选离当前画面更近。这样一口气从头滚到尾也能接上，帧数却只有每半屏一张。
//! 滚动停下时 `settle` 把候选并入，最后一屏不会漏。

use std::collections::{HashSet, VecDeque};

use image::RgbaImage;

pub const TEMPLATE_H: usize = 40;
pub const NCC_THRESHOLD: f64 = 0.92;
pub const DOWNSAMPLE_WIDTH: usize = 400;
/// 重叠区复核（见 `verify`）：分块大小
const VERIFY_BLOCK_H: usize = 8;
const VERIFY_BLOCKS_X: usize = 24;
/// 块内灰度极差达到这个值才算"有纹理"
const VERIFY_TEXTURE_RANGE: u8 = 24;
/// 一块的平均绝对灰度差不超过它就算对上
const VERIFY_BLOCK_MAD: f64 = 6.0;
/// 有纹理的块里至少这么多比例对上才算匹配（其余允许是懒加载、动图等变化的内容）
const VERIFY_GOOD_FRACTION: f64 = 0.6;
/// 有纹理的块太少时不按块判断
const VERIFY_MIN_TEXTURED: usize = 12;
/// 两帧逐行比较时"同一行"的阈值
const SAME_ROW_MAD: f64 = 2.0;
/// 模板方差下限（低于此视为纯色，没有特征）
const MIN_VARIANCE: f64 = 25.0;
const MAX_TEMPLATE_TRIES: usize = 8;
const MIN_BAND: usize = 16;
/// 连续跟拍时，位移达到内容区高度的这个比例就并入成关键帧。留一半余量：下一帧就算比
/// 这一帧又滚出去大半屏，也还能对上。
const KEYFRAME_FRACTION: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slice {
    pub frame: usize,
    pub src_y: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// 第一帧
    First,
    /// 新内容已并入
    Added { rows: u32 },
    /// 对上了，但落在已拍过的范围内
    Revisited,
    /// 和上一帧一样（没滚动 / 滚到底了）
    NoChange,
    /// 对不上（滚太快、动态内容、换了窗口…）
    Failed,
}

/// 连续跟拍时这一帧画面的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// 和上一次看到的画面一样（滚动已停 / 没在动）
    Still,
    /// 画面在动，且和已拍内容对得上
    Moving,
    /// 对不上：一下滚出去超过一屏，或画面里有大片动态内容
    Lost,
}

/// 并入长图的一帧。调用方按 `index` 保存原图，合成时 `compose` 按它取回。
pub struct Committed {
    pub index: usize,
    pub frame: RgbaImage,
    pub step: Step,
}

pub struct Fed {
    pub motion: Motion,
    pub committed: Vec<Committed>,
}

/// 一帧和上一张已并入帧的比对结果，并入时直接用，不再重算。
pub struct Prepared {
    gray: Gray,
    kind: Kind,
}

#[derive(Clone, Copy)]
enum Kind {
    First,
    NoChange,
    Failed,
    Shift {
        dy: i64,
        header: usize,
        footer: usize,
    },
}

/// 降采样灰度图，行主序。
#[derive(Clone)]
struct Gray {
    w: usize,
    h: usize,
    data: Vec<u8>,
}

impl Gray {
    fn from_rgba(img: &RgbaImage) -> Self {
        let (iw, ih) = (img.width() as usize, img.height() as usize);
        let w = iw.clamp(1, DOWNSAMPLE_WIDTH);
        let raw = img.as_raw();
        let mut data = vec![0u8; w * ih];
        for y in 0..ih {
            let row = &raw[y * iw * 4..(y + 1) * iw * 4];
            for x in 0..w {
                let x0 = x * iw / w;
                let x1 = ((x + 1) * iw / w).max(x0 + 1);
                let mut sum = 0u32;
                for px in row[x0 * 4..x1 * 4].chunks_exact(4) {
                    sum += (u32::from(px[0]) * 77 + u32::from(px[1]) * 150 + u32::from(px[2]) * 29)
                        >> 8;
                }
                data[y * w + x] = (sum / (x1 - x0) as u32) as u8;
            }
        }
        Self { w, h: ih, data }
    }

    fn row(&self, y: usize) -> &[u8] {
        &self.data[y * self.w..(y + 1) * self.w]
    }
}

fn row_mad(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    sum as f64 / a.len().max(1) as f64
}

fn region_variance(g: &Gray, y0: usize, rows: usize) -> f64 {
    let px = &g.data[y0 * g.w..(y0 + rows) * g.w];
    let n = px.len().max(1) as f64;
    let mean = px.iter().map(|&v| f64::from(v)).sum::<f64>() / n;
    px.iter()
        .map(|&v| (f64::from(v) - mean).powi(2))
        .sum::<f64>()
        / n
}

/// 一段列范围 `[x0, x1)`：模板可以只取半边（另半边压在正在变化的图片上时）。
#[derive(Clone, Copy)]
struct Cols {
    x0: usize,
    x1: usize,
}

/// 模板（a 的 [ta, ta+t) 行、cols 列）与 b 的 [tb, tb+t) 的归一化互相关。
fn ncc(a: &Gray, ta: usize, b: &Gray, tb: usize, t: usize, cols: Cols) -> f64 {
    let n = (t * (cols.x1 - cols.x0)) as f64;
    let (mut sa, mut sb) = (0.0, 0.0);
    for r in 0..t {
        sa += a.row(ta + r)[cols.x0..cols.x1]
            .iter()
            .map(|&v| f64::from(v))
            .sum::<f64>();
        sb += b.row(tb + r)[cols.x0..cols.x1]
            .iter()
            .map(|&v| f64::from(v))
            .sum::<f64>();
    }
    let (ma, mb) = (sa / n, sb / n);
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for r in 0..t {
        let ra = &a.row(ta + r)[cols.x0..cols.x1];
        let rb = &b.row(tb + r)[cols.x0..cols.x1];
        for (&x, &y) in ra.iter().zip(rb) {
            let (x, y) = (f64::from(x) - ma, f64::from(y) - mb);
            num += x * y;
            da += x * x;
            db += y * y;
        }
    }
    if da < 1e-9 || db < 1e-9 {
        return 0.0;
    }
    num / (da.sqrt() * db.sqrt())
}

/// 隔列采样的 SAD，用于粗筛。
fn sad(a: &Gray, ta: usize, b: &Gray, tb: usize, t: usize, cols: Cols) -> u64 {
    let mut s = 0u64;
    for r in 0..t {
        let ra = a.row(ta + r);
        let rb = b.row(tb + r);
        for x in (cols.x0..cols.x1).step_by(2) {
            s += u64::from(ra[x].abs_diff(rb[x]));
        }
    }
    s
}

fn cols_variance(g: &Gray, y0: usize, rows: usize, cols: Cols) -> f64 {
    let n = (rows * (cols.x1 - cols.x0)).max(1) as f64;
    let mut sum = 0.0;
    let mut sq = 0.0;
    for r in y0..y0 + rows {
        for &v in &g.row(r)[cols.x0..cols.x1] {
            let v = f64::from(v);
            sum += v;
            sq += v * v;
        }
    }
    let mean = sum / n;
    (sq / n - mean * mean).max(0.0)
}

struct Snapshot {
    header: usize,
    footer: usize,
    bands_known: bool,
    slices: VecDeque<Slice>,
    content_len: u32,
    view_top: i64,
    prev: Option<Gray>,
    last_frame: Option<usize>,
    frames: usize,
}

pub struct Stitcher {
    width: u32,
    height: u32,
    header: usize,
    footer: usize,
    bands_known: bool,
    slices: VecDeque<Slice>,
    content_len: u32,
    view_top: i64,
    prev: Option<Gray>,
    last_frame: Option<usize>,
    /// 已并入的帧数
    frames: usize,
    /// 撤销点：每个元素是一次撤销要回到的状态
    history: Vec<Snapshot>,
    /// 成组撤销：`begin_group` 之后的多次并入只留一个撤销点（一次滚动算一组）
    grouping: bool,
    group_saved: bool,
    /// 连续跟拍：还没并入的候选帧（相对 `prev` 已经比好了）
    pending: Option<(RgbaImage, Prepared)>,
    /// 连续跟拍：上一次看到的画面相对 `prev` 的位移（None = 对不上），用来判断画面停没停
    observed: Option<i64>,
    next_index: usize,
    /// 最近一次并入的接缝位置（长图坐标），预览条上画一条线方便发现拼歪
    pub last_seam: Option<u32>,
}

impl Default for Stitcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Stitcher {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            header: 0,
            footer: 0,
            bands_known: false,
            slices: VecDeque::new(),
            content_len: 0,
            view_top: 0,
            prev: None,
            last_frame: None,
            frames: 0,
            history: Vec::new(),
            grouping: false,
            group_saved: false,
            pending: None,
            observed: None,
            next_index: 0,
            last_seam: None,
        }
    }

    pub fn frame_count(&self) -> usize {
        self.frames
    }

    pub fn last_frame(&self) -> Option<usize> {
        self.last_frame
    }

    pub fn total_height(&self) -> u32 {
        if self.slices.is_empty() {
            return 0;
        }
        self.header as u32 + self.content_len + self.footer as u32
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            header: self.header,
            footer: self.footer,
            bands_known: self.bands_known,
            slices: self.slices.clone(),
            content_len: self.content_len,
            view_top: self.view_top,
            prev: self.prev.clone(),
            last_frame: self.last_frame,
            frames: self.frames,
        }
    }

    fn save_undo_point(&mut self, before: Snapshot) {
        if self.grouping && self.group_saved {
            return;
        }
        self.history.push(before);
        self.group_saved = true;
    }

    /// 开始新的一组：之后的并入在撤销时算一步，直到下一次 `begin_group`。
    pub fn begin_group(&mut self) {
        self.grouping = true;
        self.group_saved = false;
    }

    /// 还有没有可撤销的（第一帧不撤）。
    pub fn can_undo(&self) -> bool {
        self.history.len() > 1
    }

    /// 撤销最后一次并入（成组时撤销最后一组，Backspace）。返回是否还有帧。
    pub fn undo(&mut self) -> bool {
        self.pending = None;
        self.observed = None;
        self.group_saved = false;
        if let Some(s) = self.history.pop() {
            self.header = s.header;
            self.footer = s.footer;
            self.bands_known = s.bands_known;
            self.slices = s.slices;
            self.content_len = s.content_len;
            self.view_top = s.view_top;
            self.prev = s.prev;
            self.last_frame = s.last_frame;
            self.frames = s.frames;
            self.last_seam = None;
        }
        !self.history.is_empty()
    }

    /// 当前状态和所有撤销点用到的帧。其余的帧可以丢掉了。
    pub fn referenced_frames(&self) -> HashSet<usize> {
        let mut set: HashSet<usize> = self.slices.iter().map(|s| s.frame).collect();
        set.extend(self.last_frame);
        for snap in &self.history {
            set.extend(snap.slices.iter().map(|s| s.frame));
            set.extend(snap.last_frame);
        }
        set
    }

    /// 并入一帧。`index` 是调用方保存这帧所用的编号，合成时按它取回原图。
    #[cfg(test)]
    pub fn push(&mut self, frame: &RgbaImage, index: usize) -> Step {
        let p = self.prepare(frame);
        self.commit(frame, p, index)
    }

    /// 和上一张已并入的帧比位移，不改状态。
    pub fn prepare(&self, frame: &RgbaImage) -> Prepared {
        let gray = Gray::from_rgba(frame);
        let Some(prev) = self.prev.as_ref() else {
            return Prepared {
                gray,
                kind: Kind::First,
            };
        };
        let kind = if frame.width() != self.width || frame.height() != self.height {
            Kind::Failed
        } else {
            analyze(
                prev,
                &gray,
                self.bands_known.then_some((self.header, self.footer)),
            )
        };
        Prepared { gray, kind }
    }

    /// 按 `prepare` 的结果并入。`p` 必须是对着当前 `prev` 算的。
    fn commit(&mut self, frame: &RgbaImage, p: Prepared, index: usize) -> Step {
        self.pending = None;
        self.next_index = self.next_index.max(index + 1);
        let Prepared { gray, kind } = p;
        let (dy, header, footer) = match kind {
            Kind::NoChange => return Step::NoChange,
            Kind::Failed => return Step::Failed,
            Kind::First => {
                self.width = frame.width();
                self.height = frame.height();
                let before = self.snapshot();
                self.save_undo_point(before);
                self.slices.push_back(Slice {
                    frame: index,
                    src_y: 0,
                    height: frame.height(),
                });
                self.content_len = frame.height();
                self.view_top = 0;
                self.prev = Some(gray);
                self.last_frame = Some(index);
                self.frames += 1;
                return Step::First;
            }
            Kind::Shift { dy, header, footer } => (dy, header, footer),
        };
        let content = gray.h.saturating_sub(header + footer);

        let before = self.snapshot();
        if !self.bands_known {
            // 第一次确定头尾：把第一帧那整条切片裁成纯内容区
            self.header = header;
            self.footer = footer;
            self.bands_known = true;
            if let Some(first) = self.slices.front_mut() {
                first.src_y = header as u32;
                first.height = content as u32;
            }
            self.content_len = content as u32;
            self.view_top = 0;
        }

        let c = content as i64;
        let mut added = 0u32;
        self.view_top += dy;
        if dy > 0 {
            let overflow = self.view_top + c - i64::from(self.content_len);
            if overflow > 0 {
                let rows = overflow.min(c) as u32;
                // 以新帧为准：已有内容截到新帧视口顶部，整个视口都用新帧的（见模块文档）
                let keep = self.view_top.clamp(0, i64::from(self.content_len)) as u32;
                self.truncate_content(keep);
                let take = (i64::from(self.content_len) + c - self.view_top.max(0)).min(c) as u32;
                self.last_seam = Some(self.header as u32 + self.content_len);
                self.slices.push_back(Slice {
                    frame: index,
                    src_y: (self.header + content) as u32 - take,
                    height: take,
                });
                self.content_len += take;
                added = rows;
            }
        } else if self.view_top < 0 {
            let rows = (-self.view_top).min(c) as u32;
            self.slices.push_front(Slice {
                frame: index,
                src_y: self.header as u32,
                height: rows,
            });
            self.content_len += rows;
            self.view_top = 0;
            self.last_seam = Some(self.header as u32 + rows);
            added = rows;
        }
        self.save_undo_point(before);
        self.prev = Some(gray);
        self.last_frame = Some(index);
        self.frames += 1;
        if added > 0 {
            Step::Added { rows: added }
        } else {
            Step::Revisited
        }
    }

    /// 连续跟拍：滚动过程中抓到的每一帧都喂进来，按需并入（见模块文档）。
    pub fn feed(&mut self, frame: RgbaImage) -> Fed {
        let mut committed = Vec::new();
        let p = self.prepare(&frame);
        let motion = self.follow(frame, p, &mut committed, true);
        Fed { motion, committed }
    }

    /// 滚动停了：把候选帧并入。
    pub fn settle(&mut self) -> Option<Committed> {
        let (frame, p) = self.pending.take()?;
        let mut out = Vec::new();
        self.commit_new(frame, p, &mut out);
        self.observed = Some(0);
        out.pop()
    }

    fn commit_new(&mut self, frame: RgbaImage, p: Prepared, out: &mut Vec<Committed>) {
        let index = self.next_index;
        let step = self.commit(&frame, p, index);
        out.push(Committed { index, frame, step });
    }

    fn follow(
        &mut self,
        frame: RgbaImage,
        p: Prepared,
        out: &mut Vec<Committed>,
        retry: bool,
    ) -> Motion {
        match p.kind {
            Kind::First => {
                self.commit_new(frame, p, out);
                self.observed = Some(0);
                Motion::Moving
            }
            Kind::NoChange => {
                // 回到了上一张已并入帧的位置，候选作废
                self.pending = None;
                let moved = self.observed != Some(0);
                self.observed = Some(0);
                if moved {
                    Motion::Moving
                } else {
                    Motion::Still
                }
            }
            Kind::Shift { dy, header, footer } => {
                let moved = self.observed != Some(dy);
                let content = p.gray.h.saturating_sub(header + footer) as f64;
                if dy.unsigned_abs() as f64 >= content * KEYFRAME_FRACTION {
                    self.commit_new(frame, p, out);
                    self.observed = Some(0);
                } else {
                    self.pending = Some((frame, p));
                    self.observed = Some(dy);
                }
                if moved {
                    Motion::Moving
                } else {
                    Motion::Still
                }
            }
            Kind::Failed => {
                // 一下滚太远了：先并入候选（它离现在的画面更近），再拿它比一次
                if retry {
                    if let Some((pf, pp)) = self.pending.take() {
                        self.commit_new(pf, pp, out);
                        let p = self.prepare(&frame);
                        return self.follow(frame, p, out, false);
                    }
                }
                self.observed = None;
                Motion::Lost
            }
        }
    }

    /// 把内容区截短到 `len` 行（从尾部去掉切片）。
    fn truncate_content(&mut self, len: u32) {
        while self.content_len > len {
            let Some(last) = self.slices.back_mut() else {
                break;
            };
            let excess = self.content_len - len;
            if last.height > excess {
                last.height -= excess;
                self.content_len = len;
            } else {
                self.content_len -= last.height;
                self.slices.pop_back();
            }
        }
    }

    /// 按切片顺序合成完整长图。`load(index)` 取回保存的原始帧。
    pub fn compose(&self, mut load: impl FnMut(usize) -> Option<RgbaImage>) -> Option<RgbaImage> {
        let first = self.slices.iter().map(|s| s.frame).min()?;
        let last = self.last_frame?;
        let mut out = RgbaImage::new(self.width, self.total_height());
        let stride = self.width as usize * 4;
        let mut cursor = 0usize;
        let copy_rows =
            |out: &mut RgbaImage, src: &RgbaImage, y0: u32, rows: u32, cursor: &mut usize| {
                let raw = src.as_raw();
                let from = y0 as usize * stride;
                let len = rows as usize * stride;
                out.as_mut()[*cursor * stride..*cursor * stride + len]
                    .copy_from_slice(&raw[from..from + len]);
                *cursor += rows as usize;
            };
        // 头部取第一帧（最上面那张的原貌），尾部取最后一帧
        if self.header > 0 {
            let img = load(first)?;
            copy_rows(&mut out, &img, 0, self.header as u32, &mut cursor);
        }
        let mut cache: Option<(usize, RgbaImage)> = None;
        for s in &self.slices {
            if cache.as_ref().map(|(i, _)| *i) != Some(s.frame) {
                cache = Some((s.frame, load(s.frame)?));
            }
            let (_, img) = cache.as_ref()?;
            copy_rows(&mut out, img, s.src_y, s.height, &mut cursor);
        }
        if self.footer > 0 {
            let img = load(last)?;
            copy_rows(
                &mut out,
                &img,
                self.height - self.footer as u32,
                self.footer as u32,
                &mut cursor,
            );
        }
        Some(out)
    }

    pub fn slices(&self) -> impl Iterator<Item = &Slice> {
        self.slices.iter()
    }

    pub fn bands(&self) -> (u32, u32) {
        (self.header as u32, self.footer as u32)
    }
}

/// `curr` 相对 `prev` 的变化。`bands`：已经测出来的固定头尾。
fn analyze(prev: &Gray, curr: &Gray, bands: Option<(usize, usize)>) -> Kind {
    let h = curr.h;
    // 整帧几乎一样 → 没滚动
    if (0..h).all(|y| row_mad(prev.row(y), curr.row(y)) < SAME_ROW_MAD) {
        return Kind::NoChange;
    }
    let (header, footer) = bands.unwrap_or_else(|| detect_bands(prev, curr));
    let content = h.saturating_sub(header + footer);
    if content < TEMPLATE_H * 2 {
        return Kind::Failed;
    }
    match find_shift(prev, curr, header, content) {
        None => Kind::Failed,
        Some(0) => Kind::NoChange,
        Some(dy) => Kind::Shift { dy, header, footer },
    }
}

/// 两帧顶部/底部逐行相同的高度 = 固定头部/尾部。要求有纹理且 ≥ 16 行，否则不认
/// （页面顶部本来就是一大片空白时，不能误判成固定头部）。
fn detect_bands(a: &Gray, b: &Gray) -> (usize, usize) {
    let h = a.h;
    let cap = h / 3;
    let mut header = 0;
    while header < cap && row_mad(a.row(header), b.row(header)) < SAME_ROW_MAD {
        header += 1;
    }
    let mut footer = 0;
    while footer < cap && row_mad(a.row(h - 1 - footer), b.row(h - 1 - footer)) < SAME_ROW_MAD {
        footer += 1;
    }
    let header = if header >= MIN_BAND && region_variance(a, 0, header) > MIN_VARIANCE {
        header
    } else {
        0
    };
    let footer = if footer >= MIN_BAND && region_variance(a, h - footer, footer) > MIN_VARIANCE {
        footer
    } else {
        0
    };
    (header, footer)
}

/// 找内容区的位移：正数 = 向下滚了这么多行，负数 = 向上滚。
fn find_shift(prev: &Gray, curr: &Gray, header: usize, content: usize) -> Option<i64> {
    let t = TEMPLATE_H.min(content / 3).max(8);
    let lo = header;
    let hi = header + content; // 不含
    let w = prev.w;
    // 整行模板优先；它压在正在变化的内容上对不上时，再试左半边、右半边
    let bands = [
        Cols { x0: 0, x1: w },
        Cols {
            x0: 0,
            x1: (w / 2).max(1),
        },
        Cols { x0: w / 2, x1: w },
    ];

    let mut best: Option<(f64, i64)> = None;
    // 底部模板找"向下滚"，顶部模板找"向上滚"
    for down in [true, false] {
        'attempts: for attempt in 0..MAX_TEMPLATE_TRIES {
            let offset = 4 + attempt * t;
            if offset + t > content {
                break;
            }
            let ty = if down { hi - offset - t } else { lo + offset };
            for cols in bands {
                if cols.x1 - cols.x0 < 8 || cols_variance(prev, ty, t, cols) < MIN_VARIANCE {
                    continue;
                }
                if let Some((score, dy)) = search(prev, curr, ty, t, lo, hi, down, cols) {
                    if best.is_none_or(|(s, _)| score > s) {
                        best = Some((score, dy));
                    }
                    break 'attempts;
                }
            }
        }
    }
    best.map(|(_, dy)| dy)
}

#[allow(clippy::too_many_arguments)]
fn search(
    prev: &Gray,
    curr: &Gray,
    ty: usize,
    t: usize,
    lo: usize,
    hi: usize,
    down: bool,
    cols: Cols,
) -> Option<(f64, i64)> {
    // 向下滚：内容上移，匹配位置 y ≤ ty；向上滚：y ≥ ty
    let (y0, y1) = if down { (lo, ty) } else { (ty, hi - t) };
    if y1 < y0 {
        return None;
    }
    let mut coarse: Vec<(u64, usize)> = (y0..=y1)
        .map(|y| (sad(prev, ty, curr, y, t, cols), y))
        .collect();
    coarse.sort_unstable_by_key(|(s, _)| *s);
    let mut fine: Vec<(f64, usize)> = coarse
        .iter()
        .take(5)
        .map(|&(_, y)| (ncc(prev, ty, curr, y, t, cols), y))
        .filter(|(score, _)| *score >= NCC_THRESHOLD)
        .collect();
    fine.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (score, y) in fine {
        let dy = ty as i64 - y as i64;
        if verify(prev, curr, lo, hi, dy) {
            return Some((score, dy));
        }
    }
    None
}

/// 整段重叠区复核：curr[k] 应等于 prev[k + dy]。
///
/// 把重叠区切成小块，只看上一帧里**有纹理**的块（文字、边框、图片），纯色块不算 ——
/// 懒加载前的灰色占位块就是纯色的，自然被排除。位移对的时候，没变的内容逐像素一致；
/// 位移错了，有纹理的块几乎全对不上。要求大部分有纹理的块对上，允许局部内容变化。
fn verify(prev: &Gray, curr: &Gray, lo: usize, hi: usize, dy: i64) -> bool {
    // curr 的 [k0, k1) 行对应 prev 的 [k0 + dy, k1 + dy)
    let k0 = (lo as i64).max(lo as i64 - dy) as usize;
    let k1 = (hi as i64).min(hi as i64 - dy) as usize;
    if k1 <= k0 + VERIFY_BLOCK_H {
        return false;
    }
    let bw = (prev.w / VERIFY_BLOCKS_X).max(4);
    let (mut textured, mut matched) = (0usize, 0usize);
    let mut k = k0;
    while k + VERIFY_BLOCK_H <= k1 {
        let src = (k as i64 + dy) as usize;
        let mut x = 0;
        while x + bw <= prev.w {
            let (mut lo_v, mut hi_v, mut diff) = (u8::MAX, 0u8, 0u32);
            for r in 0..VERIFY_BLOCK_H {
                let a = &prev.row(src + r)[x..x + bw];
                let b = &curr.row(k + r)[x..x + bw];
                for (&pa, &pb) in a.iter().zip(b) {
                    lo_v = lo_v.min(pa);
                    hi_v = hi_v.max(pa);
                    diff += u32::from(pa.abs_diff(pb));
                }
            }
            if hi_v - lo_v >= VERIFY_TEXTURE_RANGE {
                textured += 1;
                if f64::from(diff) / (VERIFY_BLOCK_H * bw) as f64 <= VERIFY_BLOCK_MAD {
                    matched += 1;
                }
            }
            x += bw;
        }
        k += VERIFY_BLOCK_H;
    }
    if textured < VERIFY_MIN_TEXTURED {
        // 重叠区几乎全是纯色（大片空白页面）：退回逐行比对
        return verify_rows(prev, curr, lo, hi, dy);
    }
    matched as f64 >= textured as f64 * VERIFY_GOOD_FRACTION
}

fn verify_rows(prev: &Gray, curr: &Gray, lo: usize, hi: usize, dy: i64) -> bool {
    let mut good = 0usize;
    let mut rows = 0usize;
    for k in lo..hi {
        let src = k as i64 + dy;
        if src < lo as i64 || src >= hi as i64 {
            continue;
        }
        if row_mad(prev.row(src as usize), curr.row(k)) <= VERIFY_BLOCK_MAD {
            good += 1;
        }
        rows += 1;
    }
    rows >= TEMPLATE_H / 2 && good as f64 >= rows as f64 * VERIFY_GOOD_FRACTION
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定性的"文字状"纹理：每行都不一样，横向有块状结构。
    fn page(width: u32, height: u32, seed: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let line = y / 18;
            let in_text = (y % 18) < 12 && ((x / 7 + line * 3 + seed) % 11) < 8;
            let h =
                (x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663) ^ seed.wrapping_mul(83492791))
                    % 40;
            let v = if in_text {
                30 + h as u8
            } else {
                235 - (h as u8 / 4)
            };
            image::Rgba([v, v.wrapping_add((line % 7) as u8 * 3), v, 255])
        })
    }

    fn window(src: &RgbaImage, top: u32, h: u32) -> RgbaImage {
        image::imageops::crop_imm(src, 0, top, src.width(), h).to_image()
    }

    fn stitch_frames(frames: &[RgbaImage]) -> (Stitcher, Vec<Step>) {
        let mut s = Stitcher::new();
        let steps = frames
            .iter()
            .enumerate()
            .map(|(i, f)| s.push(f, i))
            .collect();
        (s, steps)
    }

    fn compose(s: &Stitcher, frames: &[RgbaImage]) -> RgbaImage {
        s.compose(|i| frames.get(i).cloned()).unwrap()
    }

    #[test]
    fn perfect_overlap_reconstructs_original() {
        let full = page(600, 1000, 1);
        let frames = vec![
            window(&full, 0, 400),
            window(&full, 300, 400),
            window(&full, 600, 400),
        ];
        let (s, steps) = stitch_frames(&frames);
        assert_eq!(steps[0], Step::First);
        assert_eq!(steps[1], Step::Added { rows: 300 });
        assert_eq!(steps[2], Step::Added { rows: 300 });
        let out = compose(&s, &frames);
        assert_eq!(out.dimensions(), (600, 1000));
        assert!(out == full, "拼接结果应与原图逐像素一致");
    }

    #[test]
    fn fixed_header_and_footer_appear_once() {
        let content = page(500, 1200, 2);
        let header = page(500, 60, 99);
        let footer = page(500, 50, 77);
        let frame = |top: u32| {
            let mut f = RgbaImage::new(500, 400);
            image::imageops::replace(&mut f, &header, 0, 0);
            image::imageops::replace(&mut f, &window(&content, top, 290), 0, 60);
            image::imageops::replace(&mut f, &footer, 0, 350);
            f
        };
        let frames = vec![frame(0), frame(150), frame(300)];
        let (s, steps) = stitch_frames(&frames);
        assert!(matches!(steps[1], Step::Added { .. }), "{steps:?}");
        assert!(matches!(steps[2], Step::Added { .. }), "{steps:?}");
        assert_eq!(s.bands(), (60, 50));
        let out = compose(&s, &frames);
        // 头 60 + 内容 (300 + 290) + 尾 50
        assert_eq!(out.height(), 60 + 590 + 50);
        assert!(window(&out, 0, 60) == header);
        assert!(window(&out, 60, 590) == window(&content, 0, 590));
        assert!(window(&out, 650, 50) == footer);
    }

    /// 网页懒加载：上一帧里还是灰色占位块的地方，下一帧已经变成了图片。
    #[test]
    fn lazy_loaded_images_do_not_break_matching() {
        let loaded = page(600, 1400, 4);
        // 未加载版本：每隔一段有一块 120 行高、占半宽的灰色占位
        let mut placeholder = loaded.clone();
        for y in 0..1400u32 {
            if (y / 120) % 3 == 1 {
                for x in 50..350u32 {
                    placeholder.put_pixel(x, y, image::Rgba([200, 200, 200, 255]));
                }
            }
        }
        // 第 k 帧拍到时，它视口上半截的图已经加载好、下半截还是占位
        let frame = |top: u32| {
            let mut f = window(&placeholder, top, 400);
            let ready = window(&loaded, top, 220);
            image::imageops::replace(&mut f, &ready, 0, 0);
            f
        };
        let frames = vec![
            frame(0),
            frame(200),
            frame(400),
            frame(600),
            frame(800),
            frame(1000),
        ];
        let (s, steps) = stitch_frames(&frames);
        for (i, st) in steps.iter().enumerate().skip(1) {
            assert!(
                matches!(st, Step::Added { rows: 200 }),
                "第 {i} 帧：{steps:?}"
            );
        }
        let out = compose(&s, &frames);
        assert_eq!(out.height(), 1400);
        // 以新帧为准：除最后一帧下半截外，长图里都是加载好的样子
        assert!(window(&out, 0, 1000 + 220) == window(&loaded, 0, 1000 + 220));
    }

    /// 视口里固定位置的悬浮按钮（不是整行的固定头尾）只在最后一帧出现一次。
    #[test]
    fn floating_button_appears_once() {
        let full = page(500, 1200, 6);
        let button = RgbaImage::from_pixel(60, 60, image::Rgba([20, 120, 240, 255]));
        let frame = |top: u32| {
            let mut f = window(&full, top, 400);
            image::imageops::replace(&mut f, &button, 420, 320);
            f
        };
        let frames = vec![frame(0), frame(150), frame(300), frame(450)];
        let (s, steps) = stitch_frames(&frames);
        assert!(
            steps
                .iter()
                .skip(1)
                .all(|st| matches!(st, Step::Added { .. })),
            "{steps:?}"
        );
        let out = compose(&s, &frames);
        assert_eq!(out.height(), 850);
        // 按钮之外的内容和原页面一致；按钮只在最后一帧的位置（450 + 320）
        assert!(window(&out, 0, 450 + 320) == window(&full, 0, 450 + 320));
        assert_eq!(
            out.get_pixel(450, 450 + 330),
            &image::Rgba([20, 120, 240, 255])
        );
    }

    #[test]
    fn uniform_frames_fail_without_panic() {
        let white = RgbaImage::from_pixel(400, 300, image::Rgba([255, 255, 255, 255]));
        let mut grey = white.clone();
        for p in grey.pixels_mut() {
            *p = image::Rgba([200, 200, 200, 255]);
        }
        let (_, steps) = stitch_frames(&[white.clone(), grey]);
        assert_eq!(steps[1], Step::Failed);
        let (_, steps) = stitch_frames(&[white.clone(), white]);
        assert_eq!(steps[1], Step::NoChange);
    }

    #[test]
    fn identical_frames_mean_reached_bottom() {
        let full = page(400, 600, 3);
        let f = window(&full, 100, 300);
        let (s, steps) = stitch_frames(&[f.clone(), f.clone(), f]);
        assert_eq!(steps[1], Step::NoChange);
        assert_eq!(steps[2], Step::NoChange);
        assert_eq!(s.total_height(), 300);
    }

    #[test]
    fn scrolling_past_a_full_screen_fails() {
        let full = page(400, 2000, 4);
        let frames = vec![window(&full, 0, 300), window(&full, 900, 300)];
        let (_, steps) = stitch_frames(&frames);
        assert_eq!(steps[1], Step::Failed);
    }

    #[test]
    fn wide_frames_are_downsampled_but_shift_is_exact() {
        let full = page(1700, 900, 5);
        let frames = vec![
            window(&full, 0, 500),
            window(&full, 137, 500),
            window(&full, 400, 500),
        ];
        let (s, steps) = stitch_frames(&frames);
        assert_eq!(steps[1], Step::Added { rows: 137 });
        assert_eq!(steps[2], Step::Added { rows: 263 });
        assert!(compose(&s, &frames) == full);
    }

    #[test]
    fn scrolling_up_prepends_and_revisits() {
        let full = page(500, 1100, 6);
        // 从中间开始，先往上滚，再往下滚回已拍区域，再继续往下
        let frames = vec![
            window(&full, 400, 400),
            window(&full, 150, 400),
            window(&full, 0, 400),
            window(&full, 300, 400),
            window(&full, 600, 400),
        ];
        let (s, steps) = stitch_frames(&frames);
        assert_eq!(steps[1], Step::Added { rows: 250 });
        assert_eq!(steps[2], Step::Added { rows: 150 });
        assert_eq!(steps[3], Step::Revisited);
        assert_eq!(steps[4], Step::Added { rows: 200 });
        assert!(compose(&s, &frames) == window(&full, 0, 1000));
    }

    /// 喂一串帧，最后 settle，返回并入的帧（按 index 存，合成时取回）。
    fn follow_frames(
        s: &mut Stitcher,
        frames: &[RgbaImage],
    ) -> (Vec<Motion>, Vec<Option<RgbaImage>>) {
        let mut store: Vec<Option<RgbaImage>> = Vec::new();
        let keep = |c: Committed, store: &mut Vec<Option<RgbaImage>>| {
            assert!(
                matches!(c.step, Step::First | Step::Added { .. } | Step::Revisited),
                "{:?}",
                c.step
            );
            if store.len() <= c.index {
                store.resize(c.index + 1, None);
            }
            store[c.index] = Some(c.frame);
        };
        let mut motions = Vec::new();
        for f in frames {
            let fed = s.feed(f.clone());
            motions.push(fed.motion);
            for c in fed.committed {
                keep(c, &mut store);
            }
        }
        if let Some(c) = s.settle() {
            keep(c, &mut store);
        }
        (motions, store)
    }

    /// 一口气从头滚到尾：每拍只挪几十行，只有关键帧被并入，结果和原页面一致。
    #[test]
    fn continuous_scroll_reconstructs_original() {
        let full = page(600, 2400, 8);
        let tops: Vec<u32> = (0..=2000).step_by(37).chain([2000, 2000]).collect();
        let frames: Vec<_> = tops.iter().map(|&t| window(&full, t, 400)).collect();
        let mut s = Stitcher::new();
        let (motions, store) = follow_frames(&mut s, &frames);
        assert!(!motions.contains(&Motion::Lost), "{motions:?}");
        // 最后一帧和倒数第二帧一样：画面停了
        assert_eq!(motions.last(), Some(&Motion::Still));
        // 每半屏左右一张，远少于抓到的帧数
        assert!(s.frame_count() <= 2000 / 200 + 2, "{}", s.frame_count());
        let out = s.compose(|i| store.get(i).cloned().flatten()).unwrap();
        assert!(out == full, "{:?}", out.dimensions());
    }

    /// 某一拍一下滚出去超过一屏（相对上一张关键帧），退回用候选帧接上。
    #[test]
    fn falls_back_to_pending_when_jump_is_too_far() {
        let full = page(500, 1400, 9);
        let frames: Vec<_> = [0, 150, 480, 820, 1000]
            .iter()
            .map(|&t| window(&full, t, 400))
            .collect();
        let mut s = Stitcher::new();
        let (motions, store) = follow_frames(&mut s, &frames);
        assert!(!motions.contains(&Motion::Lost), "{motions:?}");
        let out = s.compose(|i| store.get(i).cloned().flatten()).unwrap();
        assert!(out == window(&full, 0, 1400), "{:?}", out.dimensions());
    }

    #[test]
    fn too_fast_is_lost_then_recovers() {
        let full = page(500, 2000, 10);
        let frames: Vec<_> = [0, 900, 250, 450]
            .iter()
            .map(|&t| window(&full, t, 400))
            .collect();
        let mut s = Stitcher::new();
        let (motions, store) = follow_frames(&mut s, &frames);
        assert_eq!(motions[1], Motion::Lost);
        assert_eq!(motions[2], Motion::Moving);
        let out = s.compose(|i| store.get(i).cloned().flatten()).unwrap();
        assert!(out == window(&full, 0, 850));
    }

    /// 成组撤销：一次滚动并入的多帧一起撤掉，用不到的帧不再被引用。
    #[test]
    fn grouped_undo_reverts_whole_gesture() {
        let full = page(500, 2000, 11);
        let mut s = Stitcher::new();
        let first = [window(&full, 0, 400), window(&full, 120, 400)];
        follow_frames(&mut s, &first);
        let h = s.total_height();
        let refs = s.referenced_frames();
        s.begin_group();
        let second: Vec<_> = (160..=1200)
            .step_by(60)
            .map(|t| window(&full, t, 400))
            .collect();
        follow_frames(&mut s, &second);
        assert!(s.frame_count() > 3);
        assert!(s.can_undo());
        s.undo();
        assert_eq!(s.total_height(), h);
        assert_eq!(s.referenced_frames(), refs);
    }

    #[test]
    fn undo_restores_previous_state() {
        let full = page(400, 900, 7);
        let frames = [
            window(&full, 0, 300),
            window(&full, 200, 300),
            window(&full, 400, 300),
        ];
        let mut s = Stitcher::new();
        s.push(&frames[0], 0);
        s.push(&frames[1], 1);
        let h2 = s.total_height();
        s.push(&frames[2], 2);
        assert!(s.total_height() > h2);
        assert!(s.undo());
        assert_eq!(s.total_height(), h2);
        // 撤销后从第 2 帧的状态继续
        assert_eq!(s.push(&frames[2], 3), Step::Added { rows: 200 });
    }
}
