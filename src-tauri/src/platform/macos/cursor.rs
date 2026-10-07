//! 把当前鼠标指针画进截图（瞬间截屏用）。系统抓屏不带光标，要自己画。

use image::RgbaImage;
use objc2::rc::Retained;
use objc2_app_kit::NSCursor;

use super::util::{self, on_main};
use super::{geometry, window_enum};

/// 把光标画到 `image` 上。`origin` 是这张图左上角在虚拟桌面里的物理坐标。
/// 光标不在这张图范围内时什么都不做。
pub fn draw_cursor(image: &mut RgbaImage, origin: (i32, i32)) {
    let Some(position) = window_enum::cursor_point() else {
        return;
    };
    let screens = geometry::screens();
    let (px, py) = geometry::point_to_physical_in(&screens, position);
    let scale = screens
        .iter()
        .find(|s| s.physical.contains_point(px, py))
        .map_or(1.0, |s| s.scale);
    // 光标对象只能在主线程读
    let Some((sprite, hot_x, hot_y)) = on_main(move |_| render(scale)) else {
        return;
    };
    composite(image, &sprite, px - hot_x - origin.0, py - hot_y - origin.1);
}

/// 当前光标按这块屏的缩放渲染成位图，连同热点（物理像素）。
fn render(scale: f64) -> Option<(RgbaImage, i32, i32)> {
    // 系统建议改用 ScreenCaptureKit 连光标一起抓；我们抓的是静态画面，只要这一刻的光标图
    #[allow(deprecated)]
    let cursor = NSCursor::currentSystemCursor().unwrap_or_else(NSCursor::arrowCursor);
    let picture = cursor.image();
    let (size, hot) = (picture.size(), cursor.hotSpot());
    // SAFETY: 不给建议矩形、上下文和提示，让系统挑最合适的一份位图。
    let cg =
        unsafe { picture.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }?;
    let (w, h) = (
        (size.width * scale).round().max(1.0) as u32,
        (size.height * scale).round().max(1.0) as u32,
    );
    let sprite = util::rgba_from_cgimage(Retained::as_ptr(&cg).cast_mut().cast(), w, h, false)?;
    Some((
        sprite,
        (hot.x * scale).round() as i32,
        (hot.y * scale).round() as i32,
    ))
}

fn composite(image: &mut RgbaImage, sprite: &RgbaImage, x0: i32, y0: i32) {
    let (iw, ih) = (image.width() as i32, image.height() as i32);
    for (sx, sy, src) in sprite.enumerate_pixels() {
        let (x, y) = (x0 + sx as i32, y0 + sy as i32);
        let alpha = u32::from(src[3]);
        if alpha == 0 || x < 0 || y < 0 || x >= iw || y >= ih {
            continue;
        }
        let dst = image.get_pixel_mut(x as u32, y as u32);
        for c in 0..3 {
            dst.0[c] =
                ((u32::from(src[c]) * alpha + u32::from(dst.0[c]) * (255 - alpha)) / 255) as u8;
        }
    }
}
