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

/// 解码并按 EXIF 方向摆正（手机照片常带"旋转 90°"标记，像素本身是横着的）。
/// 第二个值：原图本来就是正的（原字节可以直接存，不用重新编码）。
pub fn decode_upright(bytes: &[u8]) -> AppResult<(RgbaImage, bool)> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok((
        img.to_rgba8(),
        orientation == image::metadata::Orientation::NoTransforms,
    ))
}

/// 应用图标的主色（剪贴板卡片标题栏用，像 Paste 那样跟着来源应用变色）。
///
/// 只看不透明、够鲜艳的像素，按色相分 24 格、按"饱和度² × 亮度"加权（越鲜艳越算数，图标的阴影、
/// 渐变暗部拖不灰它），取相邻三格加起来最重的那一块求平均。鲜艳的像素太少：大半是深色的算黑色图标
/// （Linear、终端之类），否则返回 None（用类型的颜色）。不压暗：黄色压暗就成了橄榄色。
pub fn dominant_color(img: &RgbaImage) -> Option<[u8; 3]> {
    const BINS: usize = 24;
    let mut weight = [0f64; BINS];
    let mut sum = [[0f64; 3]; BINS];
    let (mut opaque, mut dark) = (0f64, 0f64);
    let step = (img.width().max(img.height()) / 64).max(1) as usize;
    for y in (0..img.height()).step_by(step) {
        for x in (0..img.width()).step_by(step) {
            let p = img.get_pixel(x, y);
            if p[3] < 200 {
                continue;
            }
            opaque += 1.0;
            let [r, g, b] = [p[0], p[1], p[2]].map(|c| f64::from(c) / 255.0);
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            if max < 0.25 {
                dark += 1.0;
                continue;
            }
            let sat = (max - min) / max;
            if sat < 0.3 {
                continue;
            }
            let delta = max - min;
            let hue = if max == r {
                60.0 * ((g - b) / delta).rem_euclid(6.0)
            } else if max == g {
                60.0 * ((b - r) / delta + 2.0)
            } else {
                60.0 * ((r - g) / delta + 4.0)
            };
            let bin = ((hue / 360.0 * BINS as f64) as usize).min(BINS - 1);
            let w = sat * sat * max;
            weight[bin] += w;
            for (s, c) in sum[bin].iter_mut().zip([r, g, b]) {
                *s += c * w;
            }
        }
    }
    if opaque == 0.0 {
        return None;
    }
    let colorful: f64 = weight.iter().sum();
    if colorful < opaque * 0.06 {
        return (dark > opaque * 0.4).then_some([0x1c, 0x1c, 0x1e]);
    }
    let window = |i: usize| [(i + BINS - 1) % BINS, i, (i + 1) % BINS];
    let best = (0..BINS)
        .max_by(|&a, &b| {
            let wa: f64 = window(a).iter().map(|&j| weight[j]).sum();
            let wb: f64 = window(b).iter().map(|&j| weight[j]).sum();
            wa.total_cmp(&wb)
        })
        .unwrap_or(0);
    let (mut w, mut rgb) = (0f64, [0f64; 3]);
    for j in window(best) {
        w += weight[j];
        for (acc, s) in rgb.iter_mut().zip(sum[j]) {
            *acc += s;
        }
    }
    Some(rgb.map(|c| (c / w * 255.0).round().clamp(0.0, 255.0) as u8))
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

    /// 带"顺时针转 90°"标记的 JPEG（手机竖着拍的照片）要摆正，而且不能当成正的直接存原字节
    #[test]
    fn decode_upright_applies_exif_orientation() {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&image::RgbImage::new(4, 2))
            .unwrap();
        assert_eq!(decode_upright(&jpeg).unwrap().0.dimensions(), (4, 2));
        assert!(decode_upright(&jpeg).unwrap().1);

        let mut exif = b"Exif\0\0MM\x00\x2a\x00\x00\x00\x08\x00\x01".to_vec();
        exif.extend_from_slice(b"\x01\x12\x00\x03\x00\x00\x00\x01\x00\x06\x00\x00");
        exif.extend_from_slice(&[0, 0, 0, 0]);
        let mut rotated = vec![0xFF, 0xD8, 0xFF, 0xE1];
        rotated.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        rotated.extend_from_slice(&exif);
        rotated.extend_from_slice(&jpeg[2..]);
        let (img, upright) = decode_upright(&rotated).unwrap();
        assert_eq!(img.dimensions(), (2, 4));
        assert!(!upright);
    }

    #[test]
    fn dominant_color_of_icons() {
        let solid = |c: [u8; 4]| RgbaImage::from_pixel(32, 32, image::Rgba(c));
        let red = dominant_color(&solid([230, 40, 50, 255])).unwrap();
        assert!(red[0] > 200 && red[1] < 80 && red[2] < 80, "{red:?}");
        assert_eq!(dominant_color(&solid([0, 0, 0, 0])), None);
        assert_eq!(
            dominant_color(&solid([30, 30, 32, 255])),
            Some([0x1c, 0x1c, 0x1e])
        );
        assert_eq!(dominant_color(&solid([200, 200, 200, 255])), None);
        // 黄色保持黄色，不压成橄榄色
        assert_eq!(
            dominant_color(&solid([255, 204, 0, 255])),
            Some([255, 204, 0])
        );
        // 大片橙色 + 一小块蓝：取橙色
        let mut img = solid([245, 130, 40, 255]);
        for x in 0..6 {
            for y in 0..6 {
                img.put_pixel(x, y, image::Rgba([20, 90, 240, 255]));
            }
        }
        let orange = dominant_color(&img).unwrap();
        assert!(orange[0] > 200 && orange[2] < 100, "{orange:?}");
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
