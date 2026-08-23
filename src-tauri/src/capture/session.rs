//! 一次截图会话：按下热键那一刻冻结下来的全部屏幕画面。

use std::time::Instant;

use image::RgbaImage;

use crate::error::{AppError, AppResult};
use crate::platform::{MonitorId, MonitorInfo, PhysicalRect};

pub struct MonitorShot {
    pub info: MonitorInfo,
    /// 抓到的图在虚拟桌面上占据的真实矩形。
    ///
    /// 尺寸取自图像本身而不是 `info.bounds` —— 旋转过的屏幕上两者可能不一致，
    /// 而像素映射必须以真实图像为准，否则遮罩底图会错位。
    pub bounds: PhysicalRect,
    pub image: RgbaImage,
}

pub struct CaptureSession {
    pub shots: Vec<MonitorShot>,
    pub captured_at: i64,
    /// 热键按下的时刻，用于埋点核对 150ms 红线（规格 00 §6.4）。
    pub started_at: Instant,
}

impl CaptureSession {
    pub fn new(shots: Vec<MonitorShot>, captured_at: i64, started_at: Instant) -> Self {
        Self {
            shots,
            captured_at,
            started_at,
        }
    }

    pub fn shot(&self, monitor: MonitorId) -> AppResult<&MonitorShot> {
        self.shots
            .iter()
            .find(|shot| shot.info.id == monitor)
            .ok_or(AppError::MonitorNotFound(monitor.0))
    }

    /// 虚拟桌面外接矩形。副屏在主屏左侧/上方时 x/y 为负。
    pub fn virtual_bounds(&self) -> PhysicalRect {
        let mut iter = self.shots.iter().map(|shot| shot.bounds);
        let Some(first) = iter.next() else {
            return PhysicalRect::new(0, 0, 0, 0);
        };

        let (mut left, mut top) = (first.x, first.y);
        let (mut right, mut bottom) = (first.right(), first.bottom());

        for rect in iter {
            left = left.min(rect.x);
            top = top.min(rect.y);
            right = right.max(rect.right());
            bottom = bottom.max(rect.bottom());
        }

        PhysicalRect::new(left, top, (right - left) as u32, (bottom - top) as u32)
    }

    /// 从虚拟桌面坐标裁一块出来，跨显示器时自动拼接。
    ///
    /// 显示器不是矩形排列时中间会有空洞，空洞填不透明黑 —— 比留透明像素好，
    /// 粘到不支持透明的地方不会变成一团花。
    pub fn crop(&self, rect: PhysicalRect) -> AppResult<RgbaImage> {
        if rect.is_empty() {
            return Err(AppError::InvalidSelection(format!(
                "选区宽或高为 0: {}x{}",
                rect.width, rect.height
            )));
        }

        let desktop = self.virtual_bounds();
        let rect = rect.intersection(&desktop).ok_or_else(|| {
            AppError::InvalidSelection(format!(
                "选区 ({}, {}, {}x{}) 完全落在虚拟桌面 ({}, {}, {}x{}) 之外",
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                desktop.x,
                desktop.y,
                desktop.width,
                desktop.height
            ))
        })?;

        let dst_stride = rect.width as usize * 4;
        // 显示器不是矩形排列时中间会有空洞，这些像素保持全 0（黑），
        // alpha 由函数末尾统一刷成 255。
        let mut buffer = vec![0u8; dst_stride * rect.height as usize];

        for shot in &self.shots {
            let Some(overlap) = shot.bounds.intersection(&rect) else {
                continue;
            };

            let src_stride = shot.bounds.width as usize * 4;
            let src = shot.image.as_raw();
            let row_bytes = overlap.width as usize * 4;

            let src_x = (overlap.x - shot.bounds.x) as usize * 4;
            let src_y = (overlap.y - shot.bounds.y) as usize;
            let dst_x = (overlap.x - rect.x) as usize * 4;
            let dst_y = (overlap.y - rect.y) as usize;

            for row in 0..overlap.height as usize {
                let src_start = (src_y + row) * src_stride + src_x;
                let dst_start = (dst_y + row) * dst_stride + dst_x;

                // 越界只可能来自几何计算错误。宁可少拷一行也不要 panic，
                // 但要留下日志好排查。
                let Some(src_row) = src.get(src_start..src_start + row_bytes) else {
                    tracing::error!(monitor = %shot.info.id, row, "裁剪时源行越界");
                    break;
                };
                let Some(dst_row) = buffer.get_mut(dst_start..dst_start + row_bytes) else {
                    tracing::error!(monitor = %shot.info.id, row, "裁剪时目标行越界");
                    break;
                };

                dst_row.copy_from_slice(src_row);
            }
        }

        // 抓屏结果的 alpha 字节没有语义（GDI 的 GetDIBits 在 32bpp BI_RGB 下明确
        // 说这个字节"未使用"，实测常年是 0），刚才逐行拷贝把它一起搬过来了。
        // 屏幕内容本来就不透明，不修的话粘到画图里是一片空白 —— 属于 P0。
        //
        // 刻意只对裁剪结果做这一遍，而不是抓完就刷整屏：选区通常只有屏幕的
        // 几十分之一，整屏刷一遍 4K 要 33ms，白白占掉六分之一的延迟预算。
        for pixel in buffer.chunks_exact_mut(4) {
            pixel[3] = 255;
        }

        RgbaImage::from_raw(rect.width, rect.height, buffer)
            .ok_or_else(|| AppError::Image("裁剪结果的缓冲区尺寸不匹配".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn monitor(id: u64, rect: PhysicalRect, scale: f64) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId(id),
            name: format!("显示器 {id}"),
            bounds: rect,
            work_area: rect,
            scale_factor: scale,
            is_primary: id == 1,
            refresh_rate: Some(60),
        }
    }

    fn shot(id: u64, rect: PhysicalRect, scale: f64, fill: [u8; 4]) -> MonitorShot {
        MonitorShot {
            info: monitor(id, rect, scale),
            bounds: rect,
            image: RgbaImage::from_pixel(rect.width, rect.height, Rgba(fill)),
        }
    }

    /// 主屏 150%（2560×1440 物理），副屏 100% 摆在**左边**，产生负坐标。
    /// 这是规格 08 §0 点名最容易出问题的场景。
    fn mixed_dpi_session() -> CaptureSession {
        CaptureSession::new(
            vec![
                shot(1, PhysicalRect::new(0, 0, 2560, 1440), 1.5, [10, 10, 10, 255]),
                shot(
                    2,
                    PhysicalRect::new(-1920, 0, 1920, 1080),
                    1.0,
                    [200, 200, 200, 255],
                ),
            ],
            0,
            Instant::now(),
        )
    }

    #[test]
    fn virtual_bounds_covers_negative_coordinates() {
        let bounds = mixed_dpi_session().virtual_bounds();
        assert_eq!(bounds, PhysicalRect::new(-1920, 0, 4480, 1440));
    }

    #[test]
    fn crop_inside_secondary_monitor_uses_that_monitors_pixels() {
        let session = mixed_dpi_session();
        let image = session.crop(PhysicalRect::new(-1900, 10, 100, 50)).unwrap();

        assert_eq!(image.dimensions(), (100, 50));
        assert_eq!(*image.get_pixel(0, 0), Rgba([200, 200, 200, 255]));
        assert_eq!(*image.get_pixel(99, 49), Rgba([200, 200, 200, 255]));
    }

    #[test]
    fn crop_across_the_seam_stitches_both_monitors() {
        let session = mixed_dpi_session();
        // 跨过 x=0 的接缝：左 20px 来自副屏，右 30px 来自主屏
        let image = session.crop(PhysicalRect::new(-20, 0, 50, 10)).unwrap();

        assert_eq!(image.dimensions(), (50, 10));
        assert_eq!(*image.get_pixel(19, 5), Rgba([200, 200, 200, 255]));
        assert_eq!(*image.get_pixel(20, 5), Rgba([10, 10, 10, 255]));
    }

    #[test]
    fn crop_is_clamped_to_the_desktop() {
        let session = mixed_dpi_session();
        // 右边超出主屏 500px，应该被裁到桌面边界而不是报错或越界
        let image = session.crop(PhysicalRect::new(2400, 0, 660, 10)).unwrap();
        assert_eq!(image.dimensions(), (160, 10));
    }

    #[test]
    fn crop_rejects_empty_and_off_desktop_selections() {
        let session = mixed_dpi_session();
        assert!(session.crop(PhysicalRect::new(0, 0, 0, 100)).is_err());
        assert!(session.crop(PhysicalRect::new(9000, 9000, 10, 10)).is_err());
    }

    #[test]
    fn gap_between_monitors_is_filled_opaque() {
        // 两块屏之间留 100px 空洞的排布
        let session = CaptureSession::new(
            vec![
                shot(1, PhysicalRect::new(0, 0, 100, 100), 1.0, [1, 2, 3, 255]),
                shot(2, PhysicalRect::new(200, 0, 100, 100), 1.0, [4, 5, 6, 255]),
            ],
            0,
            Instant::now(),
        );

        let image = session.crop(PhysicalRect::new(0, 0, 300, 10)).unwrap();
        assert_eq!(*image.get_pixel(150, 5), Rgba([0, 0, 0, 255]));
    }
}
