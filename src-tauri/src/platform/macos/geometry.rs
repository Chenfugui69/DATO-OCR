//! 屏幕几何与坐标换算。
//!
//! macOS 的全局坐标是"点"（逻辑像素，原点在主屏左上角、y 向下），每块屏有自己的缩放；
//! 上层要的是一整张物理像素的虚拟桌面。约定：
//!
//! - 每块屏的物理原点 = 点原点 × 所有屏里最大的缩放
//! - 每块屏的物理尺寸 = 点尺寸 × 本屏缩放（也就是抓屏得到的图片尺寸）
//!
//! 单屏、或各屏缩放相同时，这就是"点 × 缩放"，和 tao 的物理坐标一致。缩放不同的多屏下
//! （2x 的内置屏 + 1x 的外接屏）各屏的物理矩形之间会有空隙，但**不会重叠**，
//! "这个点在哪块屏上"始终只有一个答案 —— 直接按"点 × 本屏缩放"算的话是会重叠的。
//!
//! AppKit 自己的坐标（NSWindow.frame、NSEvent.mouseLocation）原点在主屏**左下角**、y 向上，
//! 进出 AppKit 时要用 [`flip`] 翻一下。

use std::collections::HashMap;

use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_foundation::{NSNumber, NSString};
use parking_lot::Mutex;

use super::ffi::{self, CGPoint, CGRect, CGSize};
use crate::platform::PhysicalRect;

#[derive(Clone, Debug)]
pub struct Screen {
    pub id: u32,
    /// 全局点坐标
    pub points: CGRect,
    pub scale: f64,
    pub physical: PhysicalRect,
    pub is_primary: bool,
}

/// 只有 NSScreen 给得出的信息：扣掉菜单栏和程序坞的可用区域、显示器名字。
/// NSScreen 只能在主线程读，所以在主线程上顺手缓存，别的线程读缓存。
#[derive(Clone, Debug)]
pub struct ScreenExtra {
    /// 全局点坐标（已翻成 y 向下）
    pub visible: CGRect,
    pub name: String,
}

static EXTRAS: Mutex<Option<HashMap<u32, ScreenExtra>>> = Mutex::new(None);

pub fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}

/// 当前所有显示器。只用 CoreGraphics，任何线程都能调，每次现查（几十微秒）。
pub fn screens() -> Vec<Screen> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    // SAFETY: 缓冲区长度如实传入。
    let (err, main) = unsafe {
        (
            ffi::CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count),
            ffi::CGMainDisplayID(),
        )
    };
    if err != 0 {
        return Vec::new();
    }
    let raw: Vec<(u32, CGRect, f64)> = ids[..count as usize]
        .iter()
        .map(|&id| {
            // SAFETY: 纯查询；显示模式用完释放。
            let (bounds, scale) = unsafe {
                let bounds = ffi::CGDisplayBounds(id);
                let mode = ffi::CGDisplayCopyDisplayMode(id);
                let scale = if mode.is_null() {
                    1.0
                } else {
                    let (px, pt) = (
                        ffi::CGDisplayModeGetPixelWidth(mode),
                        ffi::CGDisplayModeGetWidth(mode),
                    );
                    ffi::CGDisplayModeRelease(mode);
                    if pt == 0 {
                        1.0
                    } else {
                        px as f64 / pt as f64
                    }
                };
                (bounds, scale)
            };
            (id, bounds, if scale > 0.0 { scale } else { 1.0 })
        })
        .filter(|(_, b, _)| b.size.width > 0.0 && b.size.height > 0.0)
        .collect();
    let mut raw = raw;
    if cfg!(debug_assertions) {
        if let Some(flag) = std::env::var_os("CHENOCR_FAKE_MONITOR") {
            if std::path::Path::new(&flag).exists() {
                let right = raw
                    .iter()
                    .map(|r| r.1.origin.x + r.1.size.width)
                    .fold(0.0, f64::max);
                raw.push((99, rect(right, 0.0, 1280.0, 720.0), 1.0));
            }
        }
    }
    let max_scale = raw.iter().map(|r| r.2).fold(1.0, f64::max);
    raw.into_iter()
        .map(|(id, points, scale)| Screen {
            id,
            points,
            scale,
            physical: PhysicalRect::new(
                (points.origin.x * max_scale).round() as i32,
                (points.origin.y * max_scale).round() as i32,
                (points.size.width * scale).round() as u32,
                (points.size.height * scale).round() as u32,
            ),
            is_primary: id == main,
        })
        .collect()
}

fn contains(r: &CGRect, p: CGPoint) -> bool {
    p.x >= r.origin.x
        && p.x < r.origin.x + r.size.width
        && p.y >= r.origin.y
        && p.y < r.origin.y + r.size.height
}

fn distance(r: &CGRect, p: CGPoint) -> f64 {
    let dx = (r.origin.x - p.x)
        .max(p.x - (r.origin.x + r.size.width))
        .max(0.0);
    let dy = (r.origin.y - p.y)
        .max(p.y - (r.origin.y + r.size.height))
        .max(0.0);
    dx * dx + dy * dy
}

/// 包含这个点的屏；点落在所有屏之外（窗口拖出去一半）就取最近的。
fn screen_for_point(screens: &[Screen], p: CGPoint) -> Option<&Screen> {
    screens.iter().find(|s| contains(&s.points, p)).or_else(|| {
        screens
            .iter()
            .min_by(|a, b| distance(&a.points, p).total_cmp(&distance(&b.points, p)))
    })
}

fn physical_as_points(s: &Screen) -> CGRect {
    rect(
        f64::from(s.physical.x),
        f64::from(s.physical.y),
        f64::from(s.physical.width),
        f64::from(s.physical.height),
    )
}

fn screen_for_physical(screens: &[Screen], x: i32, y: i32) -> Option<&Screen> {
    let p = CGPoint::new(f64::from(x), f64::from(y));
    screens
        .iter()
        .find(|s| s.physical.contains_point(x, y))
        .or_else(|| {
            screens.iter().min_by(|a, b| {
                distance(&physical_as_points(a), p).total_cmp(&distance(&physical_as_points(b), p))
            })
        })
}

pub fn point_to_physical_in(screens: &[Screen], p: CGPoint) -> (i32, i32) {
    match screen_for_point(screens, p) {
        Some(s) => (
            s.physical.x + ((p.x - s.points.origin.x) * s.scale).round() as i32,
            s.physical.y + ((p.y - s.points.origin.y) * s.scale).round() as i32,
        ),
        None => (p.x.round() as i32, p.y.round() as i32),
    }
}

pub fn point_to_physical(p: CGPoint) -> (i32, i32) {
    point_to_physical_in(&screens(), p)
}

/// 矩形按它中心所在的屏换算（窗口跨屏时归中心那块屏）。
pub fn rect_to_physical_in(screens: &[Screen], r: CGRect) -> PhysicalRect {
    let center = CGPoint::new(
        r.origin.x + r.size.width / 2.0,
        r.origin.y + r.size.height / 2.0,
    );
    match screen_for_point(screens, center) {
        Some(s) => PhysicalRect::new(
            s.physical.x + ((r.origin.x - s.points.origin.x) * s.scale).round() as i32,
            s.physical.y + ((r.origin.y - s.points.origin.y) * s.scale).round() as i32,
            (r.size.width * s.scale).round().max(0.0) as u32,
            (r.size.height * s.scale).round().max(0.0) as u32,
        ),
        None => PhysicalRect::new(
            r.origin.x.round() as i32,
            r.origin.y.round() as i32,
            r.size.width.round().max(0.0) as u32,
            r.size.height.round().max(0.0) as u32,
        ),
    }
}

pub fn rect_to_points_in(screens: &[Screen], r: PhysicalRect) -> CGRect {
    let (cx, cy) = (r.x + r.width as i32 / 2, r.y + r.height as i32 / 2);
    match screen_for_physical(screens, cx, cy) {
        Some(s) => rect(
            s.points.origin.x + f64::from(r.x - s.physical.x) / s.scale,
            s.points.origin.y + f64::from(r.y - s.physical.y) / s.scale,
            f64::from(r.width) / s.scale,
            f64::from(r.height) / s.scale,
        ),
        None => rect(
            f64::from(r.x),
            f64::from(r.y),
            f64::from(r.width),
            f64::from(r.height),
        ),
    }
}

pub fn rect_to_points(r: PhysicalRect) -> CGRect {
    rect_to_points_in(&screens(), r)
}

/// 主屏高度（点）。AppKit 坐标和全局坐标互相翻转时用。
pub fn primary_height() -> f64 {
    // SAFETY: 纯查询。
    unsafe { ffi::CGDisplayBounds(ffi::CGMainDisplayID()) }
        .size
        .height
}

/// AppKit 的矩形（原点左下、y 向上）↔ 全局矩形（原点左上、y 向下）。两个方向是同一个算式。
pub fn flip(r: CGRect) -> CGRect {
    rect(
        r.origin.x,
        primary_height() - (r.origin.y + r.size.height),
        r.size.width,
        r.size.height,
    )
}

pub fn flip_point(p: CGPoint) -> CGPoint {
    CGPoint::new(p.x, primary_height() - p.y)
}

/// 在主线程上重读 NSScreen 的可用区域和名字。
pub fn refresh_extras(mtm: MainThreadMarker) {
    let key = NSString::from_str("NSScreenNumber");
    let mut map = HashMap::new();
    for screen in NSScreen::screens(mtm).iter() {
        let Some(id) = screen
            .deviceDescription()
            .objectForKey(&key)
            .and_then(|n| n.downcast::<NSNumber>().ok())
            .map(|n| n.unsignedIntValue())
        else {
            continue;
        };
        map.insert(
            id,
            ScreenExtra {
                visible: flip(screen.visibleFrame()),
                name: screen.localizedName().to_string(),
            },
        );
    }
    *EXTRAS.lock() = Some(map);
}

/// 各屏的可用区域和名字。在主线程上调用时现读；别的线程读上次的缓存，并让主线程稍后刷新一次
/// （程序坞自动隐藏、挪位置都不会有通知，只能这样跟上）。
pub fn extras() -> HashMap<u32, ScreenExtra> {
    if let Some(mtm) = MainThreadMarker::new() {
        refresh_extras(mtm);
    } else {
        super::util::on_main_async(refresh_extras);
    }
    EXTRAS.lock().clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(id: u32, x: f64, y: f64, w: f64, h: f64, scale: f64, max: f64) -> Screen {
        Screen {
            id,
            points: rect(x, y, w, h),
            scale,
            physical: PhysicalRect::new(
                (x * max).round() as i32,
                (y * max).round() as i32,
                (w * scale).round() as u32,
                (h * scale).round() as u32,
            ),
            is_primary: x == 0.0 && y == 0.0,
        }
    }

    #[test]
    fn single_retina_screen_is_points_times_scale() {
        let s = [screen(1, 0.0, 0.0, 1710.0, 1112.0, 2.0, 2.0)];
        assert_eq!(
            point_to_physical_in(&s, CGPoint::new(100.5, 20.0)),
            (201, 40)
        );
        let r = rect_to_physical_in(&s, rect(10.0, 20.0, 300.0, 200.0));
        assert_eq!(r, PhysicalRect::new(20, 40, 600, 400));
        let back = rect_to_points_in(&s, r);
        assert_eq!((back.origin.x, back.size.height), (10.0, 200.0));
    }

    #[test]
    fn mixed_scale_screens_never_overlap() {
        // 2x 内置屏在右，1x 外接屏在左（负坐标）
        let s = [
            screen(1, 0.0, 0.0, 1440.0, 900.0, 2.0, 2.0),
            screen(2, -1920.0, 0.0, 1920.0, 1080.0, 1.0, 2.0),
        ];
        assert!(s[0].physical.intersect(&s[1].physical).is_none());
        // 外接屏上的点：原点按最大缩放放大，屏内偏移按本屏缩放
        assert_eq!(
            point_to_physical_in(&s, CGPoint::new(-1920.0 + 100.0, 50.0)),
            (-3840 + 100, 50)
        );
        // 来回换算不走样
        let p = rect_to_physical_in(&s, rect(-1000.0, 100.0, 400.0, 300.0));
        assert_eq!(p, PhysicalRect::new(-3840 + 920, 100, 400, 300));
        let back = rect_to_points_in(&s, p);
        assert_eq!((back.origin.x, back.origin.y), (-1000.0, 100.0));
    }

    #[test]
    fn point_outside_all_screens_uses_nearest() {
        let s = [screen(1, 0.0, 0.0, 1000.0, 800.0, 2.0, 2.0)];
        assert_eq!(
            point_to_physical_in(&s, CGPoint::new(-10.0, 810.0)),
            (-20, 1620)
        );
    }
}
