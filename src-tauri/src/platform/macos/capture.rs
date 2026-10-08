//! 抓屏。
//!
//! 用 `CGWindowListCreateImage` 抓一块屏的范围：实测（macOS 26，3420×2224）热态 ~15ms，
//! 比 `CGDisplayCreateImage` 快，而且设了 `sharingType = none` 的窗口（我们自己的遮罩、面板）
//! 不会被拍进去。这个接口在新 SDK 里标了废弃，但系统里一直还在；哪天真没了，换
//! ScreenCaptureKit 的 `SCScreenshotManager` 即可，上层不用动。
//!
//! 抓回来的像素在显示器自己的色彩空间里，统一转成 sRGB 再交给上层（见 `util::rgba_from_cgimage`）。

use image::RgbaImage;

use super::geometry::{self, Screen};
use super::{ffi, permissions, util};
use crate::error::{AppError, AppResult};
use crate::platform::{MonitorId, MonitorInfo, PhysicalRect};

pub fn list_monitors() -> AppResult<Vec<MonitorInfo>> {
    Ok(enumerate()?.into_iter().map(|(_, info)| info).collect())
}

pub fn capture_all() -> AppResult<Vec<(MonitorInfo, RgbaImage)>> {
    permissions::ensure_screen_capture()?;
    // 每块屏一个线程同时抓：一块接一块抓的话，两块屏（尤其带一块 5K 的）热键按下去要等一百多毫秒
    let screens = enumerate()?;
    std::thread::scope(|scope| {
        let jobs: Vec<_> = screens
            .into_iter()
            .map(|(screen, info)| {
                scope.spawn(move || capture_one(&screen).map(|image| (info, image)))
            })
            .collect();
        jobs.into_iter()
            .map(|job| {
                job.join()
                    .unwrap_or_else(|_| Err(AppError::Capture("抓屏线程异常退出".into())))
            })
            .collect()
    })
}

pub fn capture_monitor(id: MonitorId) -> AppResult<RgbaImage> {
    let (screen, _) = enumerate()?
        .into_iter()
        .find(|(_, info)| info.id == id)
        .ok_or_else(|| AppError::Capture(format!("显示器 {id} 已不存在")))?;
    capture_one(&screen)
}

/// 连续录一块屏里的一个区域（GIF 用）。没有用 ScreenCaptureKit 的推流：GIF 一秒最多 30 帧，按拍子抓
/// 一张就够，实现也简单得多。只抓要录的那一块 —— 正式版实测整屏（3420×2224）一帧 64ms，5K 屏上还要翻倍，
/// 帧率就上不去了；1201×721 的选区一帧 21ms。画面没变化时照样返回一帧，要不要出帧由上层比较（编码时相同的帧不占体积）。
pub struct ScreenRecorder {
    /// 抓屏范围：全局点坐标，对齐到整点（把要录的区域包在里面）
    points: ffi::CGRect,
    /// 抓回来的图的像素尺寸
    size: (u32, u32),
    /// 要录的区域在抓回来的图里的位置
    crop: PhysicalRect,
}

impl ScreenRecorder {
    /// `region`：这块屏里要录的那一块（屏内物理像素）。
    pub fn start(id: MonitorId, region: PhysicalRect) -> AppResult<Self> {
        permissions::ensure_screen_capture()?;
        let (screen, _) = enumerate()?
            .into_iter()
            .find(|(_, info)| info.id == id)
            .ok_or_else(|| AppError::Capture(format!("显示器 {id} 已不存在")))?;
        let whole = PhysicalRect::new(0, 0, screen.physical.width, screen.physical.height);
        let region = region
            .intersect(&whole)
            .ok_or_else(|| AppError::Capture("要录的区域不在这块屏上".into()))?;
        let s = screen.scale.max(1.0);
        // 区域的边不一定落在整点上（2x 屏上奇数像素就是半个点），往外扩到整点再抓，抓回来再裁
        let left = (f64::from(region.x) / s).floor();
        let top = (f64::from(region.y) / s).floor();
        let right = (f64::from(region.right()) / s).ceil();
        let bottom = (f64::from(region.bottom()) / s).ceil();
        let origin = screen.points.origin;
        let points = geometry::rect(origin.x + left, origin.y + top, right - left, bottom - top);
        let size = (
            ((right - left) * s).round() as u32,
            ((bottom - top) * s).round() as u32,
        );
        let crop = PhysicalRect::new(
            region.x - (left * s).round() as i32,
            region.y - (top * s).round() as i32,
            region.width,
            region.height,
        );
        Ok(Self { points, size, crop })
    }

    /// 等 `timeout`（上层给的是离下一拍还有多久）再抓一帧（只有要录的那一块，不透明 RGBA）。
    pub fn next(&self, timeout: std::time::Duration) -> Option<RgbaImage> {
        std::thread::sleep(timeout);
        let image = capture_rect(self.points, self.size).ok()?;
        crate::imaging::crop_opaque(&image, self.crop).ok()
    }
}

/// 第一次抓屏要和系统的录屏服务建立连接（实测冷 ~550ms、热 ~15ms），启动时先空抓一次。
/// 没有权限时不抓，免得一启动就弹授权框。
pub fn warm_up() {
    if !permissions::screen_capture_granted() {
        tracing::info!("还没有屏幕录制权限，跳过抓屏预热");
        return;
    }
    let started = std::time::Instant::now();
    let result = enumerate().and_then(|screens| {
        let (screen, _) = screens
            .first()
            .ok_or_else(|| AppError::Capture("没有显示器".into()))?;
        capture_one(screen).map(|img| img.width())
    });
    match result {
        Ok(_) => tracing::debug!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "抓屏预热完成"
        ),
        Err(err) => tracing::warn!("抓屏预热失败，首次截图会慢一些：{err}"),
    }
}

/// 主屏排第一，其余按 x 再按 y。前端的显示顺序依赖这个稳定次序。
fn enumerate() -> AppResult<Vec<(Screen, MonitorInfo)>> {
    let screens = geometry::screens();
    if screens.is_empty() {
        return Err(AppError::Capture("没有可用的显示器".into()));
    }
    let extras = geometry::extras();
    let mut monitors: Vec<(Screen, MonitorInfo)> = screens
        .iter()
        .map(|s| {
            let extra = extras.get(&s.id);
            let work_area = extra
                .map(|e| geometry::rect_to_physical_in(&screens, e.visible))
                .and_then(|r| r.intersect(&s.physical))
                .unwrap_or(s.physical);
            let info = MonitorInfo {
                id: MonitorId(u64::from(s.id)),
                name: extra
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| "显示器".to_owned()),
                bounds: s.physical,
                work_area,
                scale_factor: s.scale,
                is_primary: s.is_primary,
            };
            (s.clone(), info)
        })
        .collect();
    monitors.sort_by(|(_, a), (_, b)| {
        b.is_primary
            .cmp(&a.is_primary)
            .then(a.bounds.x.cmp(&b.bounds.x))
            .then(a.bounds.y.cmp(&b.bounds.y))
    });
    Ok(monitors)
}

fn capture_one(screen: &Screen) -> AppResult<RgbaImage> {
    capture_rect(
        screen.points,
        (screen.physical.width, screen.physical.height),
    )
}

/// 抓屏幕上的一块。`points`：全局点坐标；`size`：这一块的像素尺寸。
fn capture_rect(points: ffi::CGRect, size: (u32, u32)) -> AppResult<RgbaImage> {
    // SAFETY: 范围是全局点坐标；返回的图由 CgImage 负责释放。
    let image = unsafe {
        ffi::CGWindowListCreateImage(
            points,
            ffi::kCGWindowListOptionOnScreenOnly,
            ffi::kCGNullWindowID,
            ffi::kCGWindowImageBestResolution,
        )
    };
    if image.is_null() {
        return Err(AppError::Capture("系统没有返回画面".into()));
    }
    let image = util::CgImage(image);
    util::rgba_from_cgimage(image.0, size.0, size.1, true)
        .ok_or_else(|| AppError::Capture("转换屏幕画面失败".into()))
}

#[cfg(test)]
mod tests {
    use crate::platform::PhysicalRect;

    /// 真机烟雾测试：需要屏幕录制权限，默认忽略。
    /// 把抓到的画面存成 PNG 人眼看：`CHENOCR_SMOKE_OUT=路径 cargo test -- --ignored smoke_save`
    #[test]
    #[ignore]
    fn smoke_save() {
        crate::platform::init_process();
        let out = std::env::var("CHENOCR_SMOKE_OUT").unwrap();
        let shots = super::capture_all().unwrap();
        let (_, img) = &shots[0];
        let small = image::imageops::thumbnail(img, img.width() / 4, img.height() / 4);
        small.save(&out).unwrap();
        println!("saved {out}");
    }

    /// 录屏一帧要多久（决定 GIF 实际能到的帧率）：`cargo test -- --ignored smoke_recorder --nocapture`
    #[test]
    #[ignore]
    fn smoke_recorder() {
        crate::platform::init_process();
        let monitor = super::list_monitors().unwrap().remove(0);
        let whole = PhysicalRect::new(0, 0, monitor.bounds.width, monitor.bounds.height);
        // 整屏，和一块边不落在整点上的区域
        for region in [whole, PhysicalRect::new(601, 441, 1201, 721)] {
            let recorder = super::ScreenRecorder::start(monitor.id, region).unwrap();
            let _ = recorder.next(std::time::Duration::ZERO);
            let started = std::time::Instant::now();
            let n = 10;
            for _ in 0..n {
                let frame = recorder.next(std::time::Duration::ZERO).unwrap();
                assert_eq!(
                    (frame.width(), frame.height()),
                    (region.width, region.height)
                );
            }
            println!(
                "{}×{} 每帧 {}ms",
                region.width,
                region.height,
                started.elapsed().as_millis() / n
            );
            if let Ok(out) = std::env::var("CHENOCR_SMOKE_OUT") {
                recorder
                    .next(std::time::Duration::ZERO)
                    .unwrap()
                    .save(format!("{out}-{}.png", region.width))
                    .unwrap();
            }
        }
    }

    #[test]
    #[ignore]
    fn smoke_capture_all() {
        crate::platform::init_process();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let shots = super::capture_all().unwrap();
            for (info, img) in &shots {
                println!(
                    "{} {:?} scale={} img={}x{} work={:?}",
                    info.name,
                    info.bounds,
                    info.scale_factor,
                    img.width(),
                    img.height(),
                    info.work_area
                );
            }
            println!("capture_all {}ms", started.elapsed().as_millis());
        }
    }
}
