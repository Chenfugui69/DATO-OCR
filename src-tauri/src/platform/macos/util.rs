//! 小工具：句柄编码、主线程调度、取 NSWindow、位图互转。

use std::ffi::c_void;

use image::RgbaImage;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::NSWindow;
use tauri::WebviewWindow;

use super::ffi;

/// 窗口句柄 = 进程号 << 32 | CGWindowID。带上进程号，激活应用、查应用信息都不用再翻一遍窗口表；
/// 没有窗口的应用（比如只剩桌面的访达）窗口号是 0。
pub fn encode_handle(pid: i32, window: u32) -> u64 {
    (u64::from(pid as u32) << 32) | u64::from(window)
}

pub fn handle_pid(handle: u64) -> i32 {
    (handle >> 32) as u32 as i32
}

pub fn handle_window(handle: u64) -> u32 {
    handle as u32
}

pub fn own_pid() -> i32 {
    std::process::id() as i32
}

/// 在主线程上同步执行，已经在主线程就直接跑。AppKit 的窗口操作只能在主线程做，
/// 而上层的 command 多半跑在工作线程上。
pub fn on_main<R: Send>(f: impl FnOnce(MainThreadMarker) -> R + Send) -> R {
    dispatch2::run_on_main(f)
}

pub fn on_main_async(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        // SAFETY: 主队列上的任务在主线程执行。
        f(unsafe { MainThreadMarker::new_unchecked() });
    });
}

/// Tauri 窗口对应的 NSWindow。
pub fn ns_window(window: &WebviewWindow, _mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    let ptr = window.ns_window().ok()?.cast::<NSWindow>();
    // SAFETY: 指针来自存活的 Tauri 窗口；多 retain 一次，用的过程中不会被释放。
    unsafe { Retained::retain(ptr) }
}

// ───────────────────────── 位图 ─────────────────────────

/// sRGB 色彩空间。整个进程共用一份，不释放。
pub fn srgb() -> ffi::CGColorSpaceRef {
    static SPACE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    // SAFETY: 常量名由系统提供；返回值我们一直持有。
    *SPACE
        .get_or_init(|| unsafe { ffi::CGColorSpaceCreateWithName(ffi::kCGColorSpaceSRGB) } as usize)
        as ffi::CGColorSpaceRef
}

/// 持有一个 CGImage，drop 时释放。
pub struct CgImage(pub ffi::CGImageRef);

impl Drop for CgImage {
    fn drop(&mut self) {
        // SAFETY: 只在指针非空时构造。
        unsafe { ffi::CGImageRelease(self.0) };
    }
}

unsafe extern "C" fn release_pixels(info: *mut c_void, _data: *const c_void, _size: usize) {
    // SAFETY: info 是 `cgimage_from_rgba` 里 Box::into_raw 出来的。
    drop(unsafe { Box::from_raw(info.cast::<Vec<u8>>()) });
}

/// RGBA（不预乘）→ CGImage，标成 sRGB。像素拷一份交给 CGImage 管。
/// `opaque`：忽略 alpha（屏幕截图的 alpha 没有意义）。
pub fn cgimage_from_rgba(image: &RgbaImage, opaque: bool) -> Option<CgImage> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let pixels = Box::new(image.as_raw().clone());
    let (ptr, len) = (pixels.as_ptr(), pixels.len());
    let info = Box::into_raw(pixels);
    let alpha = if opaque {
        ffi::kCGImageAlphaNoneSkipLast
    } else {
        ffi::kCGImageAlphaLast
    };
    // SAFETY: 数据指针在 provider 释放前一直有效（由 release_pixels 回收）；尺寸与缓冲一致。
    unsafe {
        let provider =
            ffi::CGDataProviderCreateWithData(info.cast(), ptr.cast(), len, Some(release_pixels));
        if provider.is_null() {
            drop(Box::from_raw(info));
            return None;
        }
        let cg = ffi::CGImageCreate(
            w,
            h,
            8,
            32,
            w * 4,
            srgb(),
            alpha,
            provider,
            std::ptr::null(),
            false,
            0,
        );
        ffi::CGDataProviderRelease(provider);
        (!cg.is_null()).then_some(CgImage(cg))
    }
}

/// 把 CGImage 画进 `width`×`height` 的 sRGB 位图，顺带完成色彩转换和缩放。
///
/// 屏幕抓回来的像素在显示器自己的色彩空间里（MacBook 是 Display P3），不转的话取色器
/// 读到的数值、存出去的图在别的机器上都会偏色。
///
/// `opaque`（屏幕画面）：alpha 那一字节原样照抄来源的，没有语义 —— 和 Windows 的抓屏一样，
/// 热路径上不逐像素修它（4K 屏在调试版里要一百多毫秒），真正输出时由 `imaging::crop_opaque`
/// 统一补成不透明。`opaque` 为 false 时返回不预乘的 RGBA。
pub fn rgba_from_cgimage(
    image: ffi::CGImageRef,
    width: u32,
    height: u32,
    opaque: bool,
) -> Option<RgbaImage> {
    if image.is_null() || width == 0 || height == 0 {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let mut buf = vec![0u8; w * h * 4];
    let alpha = if opaque {
        ffi::kCGImageAlphaNoneSkipLast
    } else {
        ffi::kCGImageAlphaPremultipliedLast
    };
    // SAFETY: 位图上下文直接写进 buf，行宽 w*4，绘制期间 buf 不被移动。
    unsafe {
        let ctx =
            ffi::CGBitmapContextCreate(buf.as_mut_ptr().cast(), w, h, 8, w * 4, srgb(), alpha);
        if ctx.is_null() {
            return None;
        }
        let same_size = ffi::CGImageGetWidth(image) == w && ffi::CGImageGetHeight(image) == h;
        ffi::CGContextSetBlendMode(ctx, ffi::kCGBlendModeCopy);
        ffi::CGContextSetInterpolationQuality(
            ctx,
            if same_size {
                ffi::kCGInterpolationNone
            } else {
                ffi::kCGInterpolationHigh
            },
        );
        let rect = ffi::CGRect::new(
            ffi::CGPoint::new(0.0, 0.0),
            ffi::CGSize::new(w as f64, h as f64),
        );
        ffi::CGContextDrawImage(ctx, rect, image);
        ffi::CGContextRelease(ctx);
    }
    if !opaque {
        for px in buf.chunks_exact_mut(4) {
            let a = u32::from(px[3]);
            if a != 0 && a != 255 {
                for c in &mut px[..3] {
                    *c = (u32::from(*c) * 255 / a).min(255) as u8;
                }
            }
        }
    }
    RgbaImage::from_raw(width, height, buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_roundtrip() {
        let h = encode_handle(12345, 678);
        assert_eq!(handle_pid(h), 12345);
        assert_eq!(handle_window(h), 678);
        assert_eq!(handle_window(encode_handle(1, 0)), 0);
    }

    #[test]
    fn rgba_cgimage_roundtrip() {
        let mut img = RgbaImage::new(3, 2);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(2, 1, image::Rgba([0, 0, 255, 255]));
        let cg = cgimage_from_rgba(&img, true).unwrap();
        let back = rgba_from_cgimage(cg.0, 3, 2, true).unwrap();
        // 不透明模式下 alpha 没有语义，只比颜色
        let rgb = |x, y| {
            let p = back.get_pixel(x, y);
            [p[0], p[1], p[2]]
        };
        assert_eq!(rgb(0, 0), [255, 0, 0]);
        assert_eq!(rgb(2, 1), [0, 0, 255]);
        assert_eq!(rgb(1, 0), [0, 0, 0]);

        // 带透明度的图：来回一趟颜色和透明度都不变
        let mut img = RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([0, 255, 0, 255]));
        let cg = cgimage_from_rgba(&img, false).unwrap();
        let back = rgba_from_cgimage(cg.0, 2, 1, false).unwrap();
        assert_eq!(back.get_pixel(0, 0), &image::Rgba([0, 255, 0, 255]));
        assert_eq!(back.get_pixel(1, 0)[3], 0);
    }
}
