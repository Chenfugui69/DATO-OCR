//! Windows 抓屏实现，基于 xcap（Apache-2.0）。
//!
//! xcap 的 `Monitor::x/y` 取自 `DEVMODEW.dmPosition`，`width/height` 取自
//! `dmPelsWidth/dmPelsHeight` —— 都是**物理像素**，正是我们要的坐标系。
//! 工作区（排除任务栏）xcap 不提供，这里自己用 `GetMonitorInfoW` 补。
//!
//! 注意 xcap 的属性访问器不是字段读取：`x()` / `y()` / `width()` / `height()`
//! 每次都会走一遍 `EnumDisplaySettingsW`。所以这里只枚举一次、一次性把
//! `MonitorInfo` 组装好，不在热路径上反复问。

use image::RgbaImage;
use xcap::Monitor;

use crate::error::{AppError, AppResult};
use crate::platform::{MonitorId, MonitorInfo, PhysicalRect, ScreenCapture};

pub struct WindowsScreenCapture;

impl ScreenCapture for WindowsScreenCapture {
    fn list_monitors(&self) -> AppResult<Vec<MonitorInfo>> {
        Ok(enumerate()?.into_iter().map(|(_, info)| info).collect())
    }

    fn capture_all(&self) -> AppResult<Vec<(MonitorInfo, RgbaImage)>> {
        let monitors = enumerate()?;
        let mut shots = Vec::with_capacity(monitors.len());

        for (monitor, info) in monitors {
            let image = capture_one(&monitor)?;

            // 旋转过的屏幕上 dmPelsWidth/Height 与实际抓到的图可能不一致。
            // 遮罩层的像素映射以图像真实尺寸为准，这里只是留个痕迹。
            if image.width() != info.bounds.width || image.height() != info.bounds.height {
                tracing::warn!(
                    monitor = %info.id,
                    declared = %format!("{}x{}", info.bounds.width, info.bounds.height),
                    captured = %format!("{}x{}", image.width(), image.height()),
                    "抓屏尺寸与显示器声明尺寸不一致"
                );
            }

            shots.push((info, image));
        }

        Ok(shots)
    }

    fn warm_up(&self) {
        let started = std::time::Instant::now();

        // 只抓主屏就够：D3D 设备和 WGC 的初始化是进程级的，不是每块屏一份。
        let result = enumerate().and_then(|monitors| {
            let (monitor, _) = monitors
                .first()
                .ok_or_else(|| AppError::Capture("没有可预热的显示器".into()))?;
            capture_one(monitor).map(|image| image.width())
        });

        match result {
            Ok(width) => tracing::debug!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                width,
                "抓屏预热完成"
            ),
            Err(err) => tracing::warn!("抓屏预热失败，首次截图会慢一些: {err}"),
        }
    }
}

/// 枚举一次显示器，主屏排第一、其余按 x 再按 y。
///
/// 前端的显示顺序和 tab 顺序依赖这个稳定次序。
fn enumerate() -> AppResult<Vec<(Monitor, MonitorInfo)>> {
    let mut monitors = Vec::new();

    for monitor in Monitor::all()? {
        match describe(&monitor) {
            Ok(info) => monitors.push((monitor, info)),
            // 单块屏读失败不该让整次截图失败（比如枚举过程中被拔掉的外接屏）
            Err(err) => tracing::warn!("跳过一块无法读取的显示器: {err}"),
        }
    }

    if monitors.is_empty() {
        return Err(AppError::Capture("没有枚举到任何可用显示器".into()));
    }

    monitors.sort_by(|(_, a), (_, b)| {
        b.is_primary
            .cmp(&a.is_primary)
            .then(a.bounds.x.cmp(&b.bounds.x))
            .then(a.bounds.y.cmp(&b.bounds.y))
    });

    Ok(monitors)
}

fn capture_one(monitor: &Monitor) -> AppResult<RgbaImage> {
    Ok(monitor.capture_image()?)
}

fn describe(monitor: &Monitor) -> AppResult<MonitorInfo> {
    let bounds = PhysicalRect::new(
        monitor.x()?,
        monitor.y()?,
        monitor.width()?,
        monitor.height()?,
    );

    let refresh_rate = monitor.frequency().ok().and_then(|hz| {
        let hz = hz.round();
        (hz > 0.0).then_some(hz as u32)
    });

    Ok(MonitorInfo {
        // 这里用 name()（`\\.\DISPLAY1`，一次 GetMonitorInfoW）而不是
        // friendly_name()（"DELL U2720Q"，要走 QueryDisplayConfig 全量查询）。
        // 热键路径上不值得为一个人类可读名字付这个开销；等 M6 设置界面真要
        // 展示显示器名字时，在那边单独查一次并缓存。
        id: MonitorId(u64::from(monitor.id()?)),
        name: monitor.name().unwrap_or_else(|_| "未知显示器".to_owned()),
        work_area: work_area(&bounds).unwrap_or(bounds),
        bounds,
        scale_factor: f64::from(monitor.scale_factor()?),
        is_primary: monitor.is_primary()?,
        refresh_rate,
    })
}

/// 排除任务栏后的可用区域。
fn work_area(bounds: &PhysicalRect) -> Option<PhysicalRect> {
    use std::mem;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONULL,
    };

    // 用 bounds 内部一点反查 HMONITOR，避免依赖 xcap 内部的句柄表示。
    let probe = POINT {
        x: bounds.x + 1,
        y: bounds.y + 1,
    };

    // SAFETY: `probe` 按值传入；`info` 是栈上的合法可写指针，且 cbSize 已按
    // MONITORINFO 的真实大小填好，符合 GetMonitorInfoW 的契约。
    unsafe {
        let handle = MonitorFromPoint(probe, MONITOR_DEFAULTTONULL);
        if handle.is_invalid() {
            return None;
        }

        let mut info = MONITORINFO {
            cbSize: mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };

        if GetMonitorInfoW(handle, &mut info).ok().is_err() {
            return None;
        }

        let work = info.rcWork;
        let width = work.right.checked_sub(work.left)?;
        let height = work.bottom.checked_sub(work.top)?;
        if width <= 0 || height <= 0 {
            return None;
        }

        Some(PhysicalRect::new(
            work.left,
            work.top,
            width as u32,
            height as u32,
        ))
    }
}

/// 真机抓屏烟雾测试。
///
/// 需要真实的显示器，所以默认跳过（CI 里没有桌面会话）。手动跑：
///
/// ```text
/// cargo test --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture
/// ```
///
/// 它验证的是几件只有在真硬件上才暴露的事：DPI 声明有没有生效、多屏坐标对不对、
/// alpha 通道有没有被 GDI 留成 0。
#[cfg(test)]
mod smoke {
    use super::*;
    use crate::capture::bmp::HEADER_LEN as HEADER_OFFSET;
    use crate::platform::ScreenCapture;

    #[test]
    #[ignore = "需要真实显示器"]
    fn capture_all_monitors_on_real_hardware() {
        crate::platform::init_process();

        let monitors = WindowsScreenCapture.list_monitors().expect("枚举显示器");
        println!("\n枚举到 {} 块显示器：", monitors.len());
        for info in &monitors {
            println!(
                "  {:<16} 位置 ({:>6}, {:>6})  尺寸 {:>5}x{:<5}  工作区 {:>5}x{:<5}  缩放 {:.2}  {}",
                info.name,
                info.bounds.x,
                info.bounds.y,
                info.bounds.width,
                info.bounds.height,
                info.work_area.width,
                info.work_area.height,
                info.scale_factor,
                if info.is_primary { "主屏" } else { "" }
            );
        }

        // 分段计时，找出 316ms 到底花在哪一步（M0 的预算只有 200ms）
        let t = std::time::Instant::now();
        let enumerated = enumerate().expect("枚举");
        println!("\n[计时] enumerate            {:>6} ms", t.elapsed().as_millis());

        for (monitor, info) in &enumerated {
            // 抓两次：WGC 第一次要建 GraphicsCaptureItem 和 D3D 设备，
            // 之后 xcap 会缓存下来。用户连续截图走的是第二次那条路。
            let t = std::time::Instant::now();
            let _warmup = monitor.capture_image().expect("抓屏（首次）");
            let cold_ms = t.elapsed().as_millis();

            let t = std::time::Instant::now();
            let image = monitor.capture_image().expect("抓屏（复用）");
            let warm_ms = t.elapsed().as_millis();

            let t = std::time::Instant::now();
            let bmp = crate::capture::bmp::encode(&image);
            let bmp_ms = t.elapsed().as_millis();

            println!(
                "[计时] {:<14} 抓屏 首次 {:>4} ms / 复用 {:>4} ms | BMP {:>3} ms | {:.1} MB",
                info.name,
                cold_ms,
                warm_ms,
                bmp_ms,
                bmp.len() as f64 / 1024.0 / 1024.0
            );

            // BMP 是 24bpp，字节数应当正好是"行按 4 字节对齐 × 行数"。
            // 对不上就说明行填充算错了，底图会斜着扭。
            let expected = (image.width() as usize * 3).next_multiple_of(4)
                * image.height() as usize
                + HEADER_OFFSET;
            assert_eq!(bmp.len(), expected, "{} 的 BMP 字节数不对", info.name);
        }

        let started = std::time::Instant::now();
        let shots = WindowsScreenCapture.capture_all().expect("抓取所有显示器");
        let elapsed = started.elapsed();
        println!("[计时] capture_all 合计      {:>6} ms（{} 块屏）\n", elapsed.as_millis(), shots.len());

        assert_eq!(shots.len(), monitors.len(), "抓到的屏数应与枚举数一致");

        for (info, image) in &shots {
            assert!(image.width() > 0 && image.height() > 0, "抓到空图像");

            // 全屏同色说明抓到的是黑屏/空缓冲，不是真实桌面
            let first = &image.as_raw()[0..4];
            let uniform = image
                .as_raw()
                .chunks_exact(4)
                .all(|pixel| pixel[0..3] == first[0..3]);
            assert!(!uniform, "{} 抓到的画面是纯色，疑似没抓到真实内容", info.name);

            println!(
                "  {:<16} 抓到 {}x{}，与声明尺寸{}",
                info.name,
                image.width(),
                image.height(),
                if image.width() == info.bounds.width && image.height() == info.bounds.height {
                    "一致"
                } else {
                    "不一致（旋转屏？）"
                }
            );
        }

        // 多屏时至少确认一次跨屏裁剪能拼起来
        if shots.len() > 1 {
            let session = crate::capture::CaptureSession::new(
                shots
                    .into_iter()
                    .map(|(info, image)| crate::capture::MonitorShot {
                        bounds: PhysicalRect::new(
                            info.bounds.x,
                            info.bounds.y,
                            image.width(),
                            image.height(),
                        ),
                        info,
                        image,
                    })
                    .collect(),
                0,
                std::time::Instant::now(),
            );

            let desktop = session.virtual_bounds();
            println!(
                "\n虚拟桌面 ({}, {}) {}x{}",
                desktop.x, desktop.y, desktop.width, desktop.height
            );

            let strip = PhysicalRect::new(desktop.x, desktop.y, desktop.width, 8.min(desktop.height));
            let stitched = session.crop(strip).expect("跨屏裁剪");
            assert_eq!(stitched.width(), desktop.width);
            assert!(
                stitched.as_raw().chunks_exact(4).all(|px| px[3] == 255),
                "跨屏裁剪结果里有非不透明像素，粘到剪贴板会是空白"
            );
        }
    }
}
