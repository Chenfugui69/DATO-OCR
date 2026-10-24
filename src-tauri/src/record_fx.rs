//! 录屏的效果：指针高亮、鼠标轨迹、点击涟漪、按着的鼠标键、滚动箭头、摄像头画面，
//! 以及动图（GIF / APNG）底下的进度条 / 播放时间。
//!
//! 鼠标的效果直接画进每一帧（录制线程抓到画面的那一刻按那一刻的指针画），位置和画面对得上，不会慢一拍；
//! 录像里有、屏幕上不画（录的人自己知道鼠标在哪）。进度条和播放时间要知道总长，录完重新编码时才画。
//! 播放时间的数字用页面画好的一排字形（`set_glyphs`），这里只是挑出来缩放贴上。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use image::RgbaImage;
use parking_lot::Mutex;

use crate::settings::{parse_color, RecordSettings};

// ───────────────────────── 画图的小工具（都带一像素的抗锯齿） ─────────────────────────

/// `color` 按 `coverage`（0 ~ 1）叠到这个像素上
fn blend(img: &mut RgbaImage, x: i64, y: i64, color: [u8; 4], coverage: f64) {
    if x < 0 || y < 0 || x >= i64::from(img.width()) || y >= i64::from(img.height()) {
        return;
    }
    let a = f64::from(color[3]) / 255.0 * coverage.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let p = img.get_pixel_mut(x as u32, y as u32);
    for (d, s) in p.0.iter_mut().zip(color).take(3) {
        *d = (f64::from(s) * a + f64::from(*d) * (1.0 - a)).round() as u8;
    }
}

/// 在 [x0, x1) × [y0, y1) 里逐个像素算覆盖率
fn paint(
    img: &mut RgbaImage,
    bounds: (f64, f64, f64, f64),
    color: [u8; 4],
    coverage: impl Fn(f64, f64) -> f64,
) {
    let (x0, y0, x1, y1) = bounds;
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    let (xa, ya) = (x0.floor().max(0.0) as i64, y0.floor().max(0.0) as i64);
    let (xb, yb) = (x1.ceil().min(w) as i64, y1.ceil().min(h) as i64);
    for y in ya..yb {
        for x in xa..xb {
            let c = coverage(x as f64 + 0.5, y as f64 + 0.5);
            if c > 0.0 {
                blend(img, x, y, color, c);
            }
        }
    }
}

fn fill_circle(img: &mut RgbaImage, cx: f64, cy: f64, r: f64, color: [u8; 4]) {
    paint(
        img,
        (cx - r - 1.0, cy - r - 1.0, cx + r + 1.0, cy + r + 1.0),
        color,
        |x, y| 0.5 - ((x - cx).hypot(y - cy) - r),
    );
}

fn ring(img: &mut RgbaImage, cx: f64, cy: f64, r: f64, width: f64, color: [u8; 4]) {
    let o = r + width;
    paint(
        img,
        (cx - o - 1.0, cy - o - 1.0, cx + o + 1.0, cy + o + 1.0),
        color,
        |x, y| width / 2.0 + 0.5 - ((x - cx).hypot(y - cy) - r).abs(),
    );
}

/// 点到线段的距离
fn seg_dist(px: f64, py: f64, (ax, ay): (f64, f64), (bx, by): (f64, f64)) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (px - ax - t * dx).hypot(py - ay - t * dy)
}

fn line(img: &mut RgbaImage, a: (f64, f64), b: (f64, f64), width: f64, color: [u8; 4]) {
    let pad = width / 2.0 + 1.0;
    let bounds = (
        a.0.min(b.0) - pad,
        a.1.min(b.1) - pad,
        a.0.max(b.0) + pad,
        a.1.max(b.1) + pad,
    );
    paint(img, bounds, color, |x, y| {
        width / 2.0 + 0.5 - seg_dist(x, y, a, b)
    });
}

/// 圆角矩形（`r` 圆角半径）的有向距离：里面是负的
fn rounded_box(x: f64, y: f64, (cx, cy): (f64, f64), (hw, hh): (f64, f64), r: f64) -> f64 {
    let qx = (x - cx).abs() - hw + r;
    let qy = (y - cy).abs() - hh + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn with_alpha(mut c: [u8; 4], k: f64) -> [u8; 4] {
    c[3] = (f64::from(c[3]) * k.clamp(0.0, 1.0)).round() as u8;
    c
}

// ───────────────────────── 鼠标的效果 ─────────────────────────

/// 涟漪多久散完
const RIPPLE: Duration = Duration::from_millis(450);
/// 滚动箭头亮多久
const SCROLL_SHOW: Duration = Duration::from_millis(450);

pub struct MouseFx {
    /// 物理像素 / 逻辑像素
    scale: f64,
    highlight: Option<(f64, [u8; 4])>,
    trail: Option<([u8; 4], Duration)>,
    click: Option<[u8; 4]>,
    buttons: bool,
    scroll_arrows: bool,
    points: VecDeque<(Instant, f64, f64)>,
    ripples: Vec<(Instant, f64, f64)>,
    held: u8,
    last: Option<(f64, f64)>,
    scrolled: Option<(Instant, f64)>,
}

impl MouseFx {
    pub fn new(rec: &RecordSettings, scale: f64) -> Self {
        let e = &rec.effects;
        let color = |s: &str| (!s.trim().is_empty()).then(|| parse_color(s, [255, 59, 48, 255]));
        Self {
            scale,
            highlight: rec.mouse_highlight.then(|| {
                (
                    f64::from(e.highlight_radius) * scale,
                    parse_color(&e.highlight_color, [255, 255, 0, 128]),
                )
            }),
            trail: color(&rec.trail_color)
                .map(|c| (c, Duration::from_millis(u64::from(e.trail_ms)))),
            click: color(&rec.click_color),
            buttons: rec.mouse_buttons,
            scroll_arrows: rec.mouse_scroll,
            points: VecDeque::new(),
            ripples: Vec::new(),
            held: 0,
            last: None,
            scrolled: None,
        }
    }

    /// 一个效果都没开
    pub fn is_off(&self) -> bool {
        self.highlight.is_none()
            && self.trail.is_none()
            && self.click.is_none()
            && !self.buttons
            && !self.scroll_arrows
    }

    /// 每一拍：指针在选区里的位置（物理像素，不在就是 None）、按着哪几个键（新按下的出一圈涟漪）
    pub fn tick(&mut self, now: Instant, cursor: Option<(f64, f64)>, buttons: u8) {
        if let Some((x, y)) = cursor {
            if let Some((_, ttl)) = self.trail {
                if self.last != Some((x, y)) {
                    self.points.push_back((now, x, y));
                }
                while self
                    .points
                    .front()
                    .is_some_and(|(t, _, _)| now.duration_since(*t) > ttl)
                {
                    self.points.pop_front();
                }
            }
            if self.click.is_some() && buttons & !self.held != 0 {
                self.ripples.push((now, x, y));
            }
        }
        self.ripples
            .retain(|(t, _, _)| now.duration_since(*t) < RIPPLE);
        self.held = buttons;
        self.last = cursor;
    }

    pub fn scroll(&mut self, now: Instant, dy: f64) {
        if self.scroll_arrows {
            self.scrolled = Some((now, dy));
        }
    }

    /// 还在动：涟漪没散完、轨迹没消完、滚动箭头还亮着（画面不动也得接着出帧）
    pub fn animating(&self, now: Instant) -> bool {
        !self.ripples.is_empty()
            || self.points.len() > 1
            || self
                .scrolled
                .is_some_and(|(t, _)| now.duration_since(t) < SCROLL_SHOW)
    }

    /// 指针底下那一层：高亮、轨迹、涟漪
    pub fn draw_under(&self, img: &mut RgbaImage, now: Instant) {
        let s = self.scale;
        if let (Some((r, color)), Some((x, y))) = (self.highlight, self.last) {
            fill_circle(img, x, y, r, color);
        }
        if let Some((color, ttl)) = self.trail {
            let width = (4.0 * s).max(2.0);
            let pts: Vec<_> = self.points.iter().collect();
            for w in pts.windows(2) {
                let age = now.duration_since(w[1].0).as_secs_f64() / ttl.as_secs_f64().max(0.01);
                line(
                    img,
                    (w[0].1, w[0].2),
                    (w[1].1, w[1].2),
                    width,
                    with_alpha(color, 1.0 - age),
                );
            }
        }
        if let Some(color) = self.click {
            for (t, x, y) in &self.ripples {
                let k = now.duration_since(*t).as_secs_f64() / RIPPLE.as_secs_f64();
                let r = (6.0 + 26.0 * k.sqrt()) * s;
                ring(img, *x, *y, r, 3.0 * s, with_alpha(color, 1.0 - k));
            }
        }
    }

    /// 指针上面那一层：按着的鼠标键（指针右下一个小鼠标，按着的那半边上色）、滚动箭头
    pub fn draw_over(&self, img: &mut RgbaImage, now: Instant) {
        let s = self.scale;
        let Some((x, y)) = self.last else { return };
        let accent = self.click.unwrap_or([22, 119, 255, 255]);
        if self.buttons && self.held != 0 {
            let (cx, cy) = (x + 22.0 * s, y + 26.0 * s);
            let (hw, hh, r) = (7.0 * s, 10.0 * s, 6.0 * s);
            let bounds = (cx - hw - 2.0, cy - hh - 2.0, cx + hw + 2.0, cy + hh + 2.0);
            // 白底黑边的鼠标
            paint(img, bounds, [255, 255, 255, 235], |px, py| {
                0.5 - rounded_box(px, py, (cx, cy), (hw, hh), r)
            });
            paint(img, bounds, [30, 30, 30, 230], |px, py| {
                s * 0.6 + 0.5 - rounded_box(px, py, (cx, cy), (hw, hh), r).abs()
            });
            // 按着的那半边（左 / 右 / 中）上色：上半截
            let left = self.held & 1 != 0;
            let right = self.held & 2 != 0;
            let middle = self.held & 4 != 0;
            paint(img, bounds, accent, |px, py| {
                let inside = 0.5 - rounded_box(px, py, (cx, cy), (hw - s, hh - s), r - s);
                let top = py < cy - 1.0 * s;
                let side = (left && px < cx - 0.5 * s)
                    || (right && px > cx + 0.5 * s)
                    || (middle && (px - cx).abs() < 1.5 * s);
                if top && side {
                    inside
                } else {
                    0.0
                }
            });
        }
        if let Some((t, dy)) = self.scrolled {
            let age = now.duration_since(t);
            if age < SCROLL_SHOW {
                let fade = 1.0 - age.as_secs_f64() / SCROLL_SHOW.as_secs_f64();
                let color = with_alpha(accent, fade);
                let dir = if dy > 0.0 { 1.0 } else { -1.0 };
                let (cx, cy) = (x + 24.0 * s, y);
                // 两道人字形箭头
                for k in [0.0, 7.0] {
                    let tip = cy + dir * (6.0 + k) * s;
                    let back = tip - dir * 6.0 * s;
                    line(img, (cx - 6.0 * s, back), (cx, tip), 2.5 * s, color);
                    line(img, (cx + 6.0 * s, back), (cx, tip), 2.5 * s, color);
                }
            }
        }
    }
}

// ───────────────────────── 摄像头 ─────────────────────────

pub struct CameraSpec {
    /// 画面边长（录制区域的物理像素）
    pub size: u32,
    /// 左上角在选区里的位置（占选区宽高的比例）；None = 左下角
    pub at: Option<[f64; 2]>,
    /// circle | square | rounded
    pub shape: String,
}

impl CameraSpec {
    pub fn from_settings(rec: &RecordSettings, scale: f64) -> Self {
        Self {
            size: ((f64::from(rec.camera_px) * scale).round() as u32).clamp(48, 1200),
            at: rec.camera_at,
            shape: rec.camera_shape.clone(),
        }
    }
}

/// 摄像头画面贴到帧上：按设置的位置和形状，一圈白边
pub fn draw_camera(frame: &mut RgbaImage, cam: &RgbaImage, spec: &CameraSpec) {
    let d = f64::from(cam.width().min(cam.height()));
    let (fw, fh) = (f64::from(frame.width()), f64::from(frame.height()));
    if d < 8.0 || d + 8.0 > fw.min(fh) {
        return;
    }
    let margin = (d / 10.0).clamp(8.0, 32.0);
    let (x0, y0) = match spec.at {
        Some([ax, ay]) => ((ax * fw).clamp(0.0, fw - d), (ay * fh).clamp(0.0, fh - d)),
        None => (margin, fh - margin - d),
    };
    let r = d / 2.0;
    let corner = match spec.shape.as_str() {
        "square" => 0.0,
        "rounded" => d * 0.18,
        _ => r,
    };
    let border = (r * 0.04).clamp(2.0, 5.0);
    let center = (x0 + r, y0 + r);
    for y in 0..cam.height().min(d as u32) {
        for x in 0..cam.width().min(d as u32) {
            let (px, py) = (x0 + f64::from(x) + 0.5, y0 + f64::from(y) + 0.5);
            let sd = rounded_box(px, py, center, (r, r), corner);
            if sd > 0.5 {
                continue;
            }
            let src = if sd > -border {
                [255u8, 255, 255, 255]
            } else {
                let p = cam.get_pixel(x, y);
                [p[0], p[1], p[2], 255]
            };
            blend(
                frame,
                (x0 + f64::from(x)) as i64,
                (y0 + f64::from(y)) as i64,
                src,
                0.5 - sd,
            );
        }
    }
}

// ───────────────────────── 动图的进度条 / 播放时间 ─────────────────────────

/// 页面画好的一排字形：`0123456789:/` 这些，每个字在图里的横向位置和宽度
pub struct Glyphs {
    pub atlas: RgbaImage,
    pub chars: Vec<(char, u32, u32)>,
}

static GLYPHS: Mutex<Option<Glyphs>> = Mutex::new(None);

pub fn set_glyphs(glyphs: Glyphs) {
    *GLYPHS.lock() = Some(glyphs);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DecorKind {
    Bar,
    Time,
}

#[derive(Clone, Copy, Debug)]
pub struct Decor {
    pub kind: DecorKind,
    pub color: [u8; 4],
}

impl Decor {
    pub fn from_settings(rec: &RecordSettings) -> Option<Self> {
        let kind = match rec.progress.as_str() {
            "bar" => DecorKind::Bar,
            "time" => DecorKind::Time,
            _ => return None,
        };
        Some(Self {
            kind,
            color: parse_color(&rec.effects.progress_color, [22, 119, 255, 255]),
        })
    }
}

fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// 一个字形缩到 `height` 高，按字形的灰度当覆盖率、用 `color` 画到 (x, y)。返回画了多宽
fn draw_glyph(
    img: &mut RgbaImage,
    g: &Glyphs,
    (gx, gw): (u32, u32),
    x: f64,
    y: f64,
    height: f64,
    color: [u8; 4],
) -> f64 {
    let ah = f64::from(g.atlas.height());
    let k = height / ah.max(1.0);
    let w = f64::from(gw) * k;
    let (xa, ya) = (x.floor() as i64, y.floor() as i64);
    for oy in 0..height.ceil() as i64 {
        for ox in 0..w.ceil() as i64 {
            // 目标像素对应字形里的一块，取那块的平均（缩小时不糊成锯齿）
            let (sx0, sy0) = (f64::from(gx) + ox as f64 / k, oy as f64 / k);
            let (sx1, sy1) = (sx0 + 1.0 / k, sy0 + 1.0 / k);
            let (mut sum, mut n) = (0.0, 0.0);
            let mut sy = sy0.floor() as i64;
            while (sy as f64) < sy1 && sy < ah as i64 {
                let mut sx = sx0.floor() as i64;
                while (sx as f64) < sx1 && sx < i64::from(gx + gw) {
                    if sx >= 0 && sy >= 0 {
                        sum += f64::from(g.atlas.get_pixel(sx as u32, sy as u32)[3]) / 255.0;
                        n += 1.0;
                    }
                    sx += 1;
                }
                sy += 1;
            }
            if n > 0.0 {
                blend(img, xa + ox, ya + oy, color, sum / n);
            }
        }
    }
    w
}

/// 画到动图的一帧上：`t` 是这一帧的时间、`total` 是整段多长（毫秒）
pub fn draw_decor(img: &mut RgbaImage, decor: &Decor, t: u64, total: u64) {
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    match decor.kind {
        DecorKind::Bar => {
            let bh = (h / 120.0).clamp(3.0, 8.0);
            let done = w * (t as f64 / total.max(1) as f64).clamp(0.0, 1.0);
            // 没走到的那截淡淡的一条
            paint(img, (0.0, h - bh, w, h), [0, 0, 0, 60], |_, _| 1.0);
            paint(img, (0.0, h - bh, done, h), decor.color, |x, _| {
                (done - x + 0.5).clamp(0.0, 1.0)
            });
        }
        DecorKind::Time => {
            let guard = GLYPHS.lock();
            let Some(g) = guard.as_ref() else { return };
            let text = format!("{} / {}", clock(t), clock(total));
            let th = (h / 18.0).clamp(12.0, 40.0);
            let pad = th * 0.35;
            let width: f64 = text
                .chars()
                .map(|c| {
                    g.chars
                        .iter()
                        .find(|(ch, _, _)| *ch == c)
                        .map_or(th * 0.3, |(_, _, gw)| {
                            f64::from(*gw) * th / f64::from(g.atlas.height().max(1))
                        })
                })
                .sum();
            let (bx1, by1) = (w - pad, h - pad);
            let (bx0, by0) = (bx1 - width - pad * 2.0, by1 - th - pad * 1.2);
            // 半透明的黑底、圆角
            let r = (th * 0.35).min((by1 - by0) / 2.0);
            let (cx, cy, hw, hh) = (
                (bx0 + bx1) / 2.0,
                (by0 + by1) / 2.0,
                (bx1 - bx0) / 2.0,
                (by1 - by0) / 2.0,
            );
            paint(
                img,
                (bx0 - 1.0, by0 - 1.0, bx1 + 1.0, by1 + 1.0),
                [0, 0, 0, 150],
                |x, y| 0.5 - rounded_box(x, y, (cx, cy), (hw, hh), r),
            );
            let mut x = bx0 + pad;
            for c in text.chars() {
                match g.chars.iter().find(|(ch, _, _)| *ch == c) {
                    Some(&(_, gx, gw)) => {
                        x += draw_glyph(img, g, (gx, gw), x, by0 + pad * 0.6, th, decor.color)
                    }
                    None => x += th * 0.3,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ripple_appears_on_press_and_fades() {
        let rec = RecordSettings {
            click_color: "#ff0000".into(),
            ..Default::default()
        };
        let mut fx = MouseFx::new(&rec, 1.0);
        let t0 = Instant::now();
        fx.tick(t0, Some((50.0, 50.0)), 0);
        assert!(!fx.animating(t0));
        fx.tick(t0, Some((50.0, 50.0)), 1);
        assert!(fx.animating(t0));
        let mut img = RgbaImage::from_pixel(100, 100, image::Rgba([255, 255, 255, 255]));
        fx.draw_under(&mut img, t0 + Duration::from_millis(100));
        // 涟漪那一圈变红了，圆心不动
        let ring_px = (0..100).any(|x| img.get_pixel(x, 50)[1] < 200);
        assert!(ring_px);
        assert_eq!(img.get_pixel(50, 50).0, [255, 255, 255, 255]);
        fx.tick(t0 + RIPPLE, Some((50.0, 50.0)), 1);
        assert!(!fx.animating(t0 + RIPPLE));
    }

    #[test]
    fn camera_lands_where_it_was_dragged() {
        let mut frame = RgbaImage::from_pixel(400, 300, image::Rgba([0, 0, 255, 255]));
        let cam = RgbaImage::from_pixel(100, 100, image::Rgba([255, 0, 0, 255]));
        let spec = CameraSpec {
            size: 100,
            at: Some([0.5, 0.0]),
            shape: "square".into(),
        };
        draw_camera(&mut frame, &cam, &spec);
        assert_eq!(frame.get_pixel(250, 50).0, [255, 0, 0, 255]);
        assert_eq!(frame.get_pixel(50, 250).0, [0, 0, 255, 255]);
    }

    #[test]
    fn progress_bar_grows_with_time() {
        let mut img = RgbaImage::from_pixel(200, 120, image::Rgba([255, 255, 255, 255]));
        let decor = Decor {
            kind: DecorKind::Bar,
            color: [255, 0, 0, 255],
        };
        draw_decor(&mut img, &decor, 500, 1000);
        assert_eq!(img.get_pixel(50, 119).0, [255, 0, 0, 255]);
        assert_ne!(img.get_pixel(150, 119).0, [255, 0, 0, 255]);
    }
}
