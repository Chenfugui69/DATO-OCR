//! 把当前鼠标指针画进截图（瞬间截屏用）。
//!
//! 系统截屏不带光标，要自己画。光标可能是带透明阴影的 32 位图，也可能是黑白的"反色"光标
//! （I 形光标就是），没法直接读出透明度。办法：在黑底和白底上各画一次，
//! 两次结果的差就是透明度；白底比黑底还暗的像素是反色像素，画的时候把底下的像素取反。

use image::RgbaImage;
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetObjectW, ReleaseDC,
    SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CopyIcon, DestroyIcon, DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING,
    DI_NORMAL, HICON, ICONINFO,
};

/// 把光标画到 `image` 上。`origin` 是这张图左上角在虚拟桌面里的物理坐标。
/// 光标不在这张图范围内、或被隐藏时什么都不做。
pub fn draw_cursor(image: &mut RgbaImage, origin: (i32, i32)) {
    // SAFETY: 只读查询；拿到的光标句柄先复制一份再用，用完销毁。
    unsafe {
        let mut info = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        if GetCursorInfo(&mut info).is_err() || info.flags.0 & CURSOR_SHOWING.0 == 0 {
            return;
        }
        let Ok(icon) = CopyIcon(HICON(info.hCursor.0)) else {
            return;
        };
        if let Some(sprite) = render(icon) {
            let x = info.ptScreenPos.x - sprite.hotspot.x - origin.0;
            let y = info.ptScreenPos.y - sprite.hotspot.y - origin.1;
            composite(image, &sprite, x, y);
        }
        let _ = DestroyIcon(icon);
    }
}

struct Sprite {
    w: u32,
    h: u32,
    hotspot: POINT,
    /// 黑底、白底各画一次的结果（BGRA）
    on_black: Vec<u8>,
    on_white: Vec<u8>,
}

/// SAFETY: `icon` 必须是有效的图标 / 光标句柄。
unsafe fn render(icon: HICON) -> Option<Sprite> {
    let mut ii = ICONINFO::default();
    unsafe { GetIconInfo(icon, &mut ii).ok()? };
    // 黑白光标没有彩色位图，掩码位图上下两半分别是 AND / XOR 掩码
    let (w, h) = unsafe {
        let probe = if ii.hbmColor.is_invalid() {
            ii.hbmMask
        } else {
            ii.hbmColor
        };
        let mut bm = BITMAP::default();
        GetObjectW(
            HGDIOBJ(probe.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some((&mut bm as *mut BITMAP).cast()),
        );
        let h = if ii.hbmColor.is_invalid() {
            bm.bmHeight / 2
        } else {
            bm.bmHeight
        };
        (bm.bmWidth.max(1) as u32, h.max(1) as u32)
    };
    let hotspot = POINT {
        x: ii.xHotspot as i32,
        y: ii.yHotspot as i32,
    };
    unsafe {
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
        }
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
        }
    }
    let on_black = unsafe { draw_on(icon, w, h, 0x00)? };
    let on_white = unsafe { draw_on(icon, w, h, 0xFF)? };
    Some(Sprite {
        w,
        h,
        hotspot,
        on_black,
        on_white,
    })
}

/// 在纯色底的 32 位 DIB 上画一次光标，返回 BGRA 像素。
unsafe fn draw_on(icon: HICON, w: u32, h: u32, fill: u8) -> Option<Vec<u8>> {
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // 自上而下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib: Option<HBITMAP> =
            CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok();
        let out = dib.filter(|_| !bits.is_null()).and_then(|dib| {
            let old = SelectObject(dc, HGDIOBJ(dib.0));
            let len = (w * h * 4) as usize;
            let px = std::slice::from_raw_parts_mut(bits.cast::<u8>(), len);
            px.fill(fill);
            let drawn = DrawIconEx(dc, 0, 0, icon, w as i32, h as i32, 0, None, DI_NORMAL).is_ok();
            let copy = drawn.then(|| px.to_vec());
            SelectObject(dc, old);
            let _ = DeleteObject(HGDIOBJ(dib.0));
            copy
        });
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        out
    }
}

fn composite(image: &mut RgbaImage, s: &Sprite, x0: i32, y0: i32) {
    let (iw, ih) = (image.width() as i32, image.height() as i32);
    for sy in 0..s.h as i32 {
        let y = y0 + sy;
        if y < 0 || y >= ih {
            continue;
        }
        for sx in 0..s.w as i32 {
            let x = x0 + sx;
            if x < 0 || x >= iw {
                continue;
            }
            let i = ((sy as u32 * s.w + sx as u32) * 4) as usize;
            let (b, w) = (&s.on_black[i..i + 3], &s.on_white[i..i + 3]);
            let dst = image.get_pixel_mut(x as u32, y as u32);
            // 白底比黑底还暗：XOR 反色像素（I 形光标的竖线）
            if w.iter().zip(b).all(|(w, b)| w < b) {
                for c in 0..3 {
                    dst.0[c] = 255 - dst.0[c];
                }
                continue;
            }
            // 透明度 = 255 - (白底 - 黑底)，三个通道取平均
            let diff: i32 = w
                .iter()
                .zip(b)
                .map(|(&w, &b)| i32::from(w) - i32::from(b))
                .sum::<i32>()
                / 3;
            let alpha = (255 - diff).clamp(0, 255) as u32;
            if alpha == 0 {
                continue;
            }
            for c in 0..3 {
                // 黑底上的结果是预乘过透明度的颜色；BGRA → RGBA
                let src = u32::from(b[2 - c]);
                let under = u32::from(dst.0[c]);
                dst.0[c] = (src + under * (255 - alpha) / 255).min(255) as u8;
            }
        }
    }
}
