//! 图像编解码与合成的小工具。

use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, RgbaImage};

use crate::error::{AppError, AppResult};
use crate::platform::PhysicalRect;

/// PNG：快速压缩 + 自适应滤波。截图库和"另存为"都在后台线程，但 4K 图用默认
/// 压缩级别要 300ms+，fdeflate 的 Fast 档十几毫秒，体积只大一点。
pub fn encode_png(img: &RgbaImage) -> AppResult<Vec<u8>> {
    let mut out = Vec::with_capacity((img.width() * img.height()) as usize);
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgba8,
        )?;
    Ok(out)
}

pub fn encode_jpeg(img: &RgbaImage, quality: u8) -> AppResult<Vec<u8>> {
    let rgb: Vec<u8> = img
        .as_raw()
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality).write_image(
        &rgb,
        img.width(),
        img.height(),
        ExtendedColorType::Rgb8,
    )?;
    Ok(out)
}

/// 24bpp 自下而上 BMP。只用来把像素喂给 WebView（`shot:` 协议）：
/// 编码约等于一次 memcpy，Chromium 原生解码。PNG 在桌面截图上又慢又大。
pub fn encode_bmp24(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let row = (w as usize * 3 + 3) & !3;
    let data_len = row * h as usize;
    let mut out = Vec::with_capacity(54 + data_len);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    let raw = img.as_raw();
    let stride = w as usize * 4;
    let pad = row - w as usize * 3;
    for y in (0..h as usize).rev() {
        for px in raw[y * stride..(y + 1) * stride].chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        out.extend(std::iter::repeat_n(0, pad));
    }
    out
}

pub fn has_alpha(img: &RgbaImage) -> bool {
    img.as_raw().chunks_exact(4).any(|p| p[3] != 255)
}

/// 给 WebView 显示用的编码：不透明走 BMP（快），带透明走 PNG。
pub fn encode_for_webview(img: &RgbaImage) -> AppResult<(Vec<u8>, &'static str)> {
    if has_alpha(img) {
        Ok((encode_png(img)?, "image/png"))
    } else {
        Ok((encode_bmp24(img), "image/bmp"))
    }
}

pub fn crop(img: &RgbaImage, rect: PhysicalRect) -> AppResult<RgbaImage> {
    let bounds = PhysicalRect::new(0, 0, img.width(), img.height());
    let r = rect
        .intersect(&bounds)
        .ok_or_else(|| AppError::msg("选区在画面之外"))?;
    Ok(image::imageops::crop_imm(img, r.x as u32, r.y as u32, r.width, r.height).to_image())
}

/// 屏幕画面裁剪：顺带把 alpha 统一写成 255（抓屏给的 alpha 是垃圾值）。
pub fn crop_opaque(img: &RgbaImage, rect: PhysicalRect) -> AppResult<RgbaImage> {
    let mut out = crop(img, rect)?;
    for px in out.as_mut().chunks_exact_mut(4) {
        px[3] = 255;
    }
    Ok(out)
}

/// 把标注层（带透明度的 PNG）按偏移叠加到底图上。
pub fn composite_png(base: &mut RgbaImage, png: &[u8], x: i64, y: i64) -> AppResult<()> {
    let layer = image::load_from_memory_with_format(png, image::ImageFormat::Png)?.to_rgba8();
    image::imageops::overlay(base, &layer, x, y);
    Ok(())
}

/// 卡片缩略图的短边下限、长边上限（物理像素）。
///
/// 剪贴板、截图库的卡片都按"铺满裁切"（object-fit: cover）显示，看得清不清楚取决于**短边**：
/// 以前按长边 320 缩，一张 2335×497 的宽截图缩出来只有 320×68，铺满 4K 屏上二百多像素高的卡片
/// 要放大三倍多，糊成一片。卡片图片区在 4K@150% 上大约 234×225 物理像素，200% 屏 300 左右，
/// 短边留 400 够用；超长图再按长边封顶，免得缩略图比原图还占地方。
pub const CARD_THUMB_SHORT: u32 = 400;
pub const CARD_THUMB_LONG: u32 = 1600;

/// 按短边缩（至少 `min_short`），长边超过 `max_long` 再按长边缩。原图本来就小就不放大。
pub fn card_thumbnail(img: &RgbaImage, min_short: u32, max_long: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let (short, long) = (w.min(h).max(1), w.max(h).max(1));
    let mut scale = f64::from(min_short) / f64::from(short);
    if f64::from(long) * scale > f64::from(max_long) {
        scale = f64::from(max_long) / f64::from(long);
    }
    if scale >= 1.0 {
        return img.clone();
    }
    let tw = ((f64::from(w) * scale).round() as u32).max(1);
    let th = ((f64::from(h) * scale).round() as u32).max(1);
    image::imageops::thumbnail(img, tw, th)
}

/// 卡片缩略图（见 `card_thumbnail`），JPEG 质量 90（截图里的字压得太狠会起毛边）。
pub fn write_card_thumbnail(img: &RgbaImage, path: &Path) -> AppResult<()> {
    write_jpeg_flat(
        card_thumbnail(img, CARD_THUMB_SHORT, CARD_THUMB_LONG),
        path,
        90,
    )
}

/// 透明处垫白底后存 JPEG。
fn write_jpeg_flat(mut thumb: RgbaImage, path: &Path, quality: u8) -> AppResult<()> {
    for p in thumb.pixels_mut() {
        if p[3] != 255 {
            let a = u32::from(p[3]);
            for c in 0..3 {
                p[c] = ((u32::from(p[c]) * a + 255 * (255 - a)) / 255) as u8;
            }
            p[3] = 255;
        }
    }
    std::fs::write(path, encode_jpeg(&thumb, quality)?)?;
    Ok(())
}

pub fn decode(bytes: &[u8]) -> AppResult<RgbaImage> {
    Ok(image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()?
        .to_rgba8())
}

pub fn load(path: &Path) -> AppResult<RgbaImage> {
    Ok(image::open(path)?.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmp24_decodes_back() {
        let mut img = RgbaImage::new(5, 3);
        img.put_pixel(4, 2, image::Rgba([10, 20, 30, 255]));
        let bmp = encode_bmp24(&img);
        let back = image::load_from_memory(&bmp).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(4, 2), &image::Rgba([10, 20, 30, 255]));
        assert_eq!(back.dimensions(), (5, 3));
    }

    #[test]
    fn crop_clamps_to_image() {
        let img = RgbaImage::new(100, 50);
        let c = crop(&img, PhysicalRect::new(90, 40, 50, 50)).unwrap();
        assert_eq!(c.dimensions(), (10, 10));
        assert!(crop(&img, PhysicalRect::new(200, 0, 10, 10)).is_err());
    }
}

#[cfg(test)]
mod card_thumb_tests {
    use super::*;

    #[test]
    fn wide_screenshot_keeps_enough_height() {
        let img = RgbaImage::new(2335, 497);
        let t = card_thumbnail(&img, CARD_THUMB_SHORT, CARD_THUMB_LONG);
        // 按长边封顶：1600 × 341（以前按长边 320 缩只剩 68 像素高）
        assert_eq!(t.dimensions(), (1600, 341));
        let img = RgbaImage::new(1600, 900);
        assert_eq!(card_thumbnail(&img, 400, 1600).dimensions(), (711, 400));
        let small = RgbaImage::new(300, 200);
        assert_eq!(card_thumbnail(&small, 400, 1600).dimensions(), (300, 200));
    }
}
