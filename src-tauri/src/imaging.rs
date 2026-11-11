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

/// 红蓝通道对调、alpha 补成 255：BGRA → RGBA 和 RGBA → BGRA 都是这一步。`src` 和 `dst` 等长。
///
/// 发布版按体积优化，编译器不会把逐像素的循环向量化，4K 一屏要 15ms（截图时还在 UI 线程上）；
/// SSSE3 一次重排 16 字节，几毫秒。
pub fn swap_rb_opaque(src: &[u8], dst: &mut [u8]) {
    assert_eq!(src.len(), dst.len());
    #[cfg(target_arch = "x86_64")]
    let done = if std::arch::is_x86_feature_detected!("ssse3") {
        // SAFETY: 刚确认 CPU 支持 SSSE3；两段等长，只读写前 `len / 16 * 16` 字节。
        unsafe { swap_rb_ssse3(src, dst) }
    } else {
        0
    };
    #[cfg(not(target_arch = "x86_64"))]
    let done = 0;
    for (d, s) in dst[done..]
        .chunks_exact_mut(4)
        .zip(src[done..].chunks_exact(4))
    {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = 255;
    }
}

/// 处理掉 16 字节的整块，返回处理了多少字节。
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn swap_rb_ssse3(src: &[u8], dst: &mut [u8]) -> usize {
    use std::arch::x86_64::{
        __m128i, _mm_loadu_si128, _mm_or_si128, _mm_set1_epi32, _mm_setr_epi8, _mm_shuffle_epi8,
        _mm_storeu_si128,
    };
    let order = _mm_setr_epi8(2, 1, 0, 3, 6, 5, 4, 7, 10, 9, 8, 11, 14, 13, 12, 15);
    let alpha = _mm_set1_epi32(0xff00_0000_u32 as i32);
    let blocks = src.len() / 16;
    for i in 0..blocks {
        // SAFETY: i * 16 + 16 <= len，两段都够长；未对齐读写
        unsafe {
            let v = _mm_loadu_si128(src.as_ptr().add(i * 16).cast::<__m128i>());
            let v = _mm_or_si128(_mm_shuffle_epi8(v, order), alpha);
            _mm_storeu_si128(dst.as_mut_ptr().add(i * 16).cast::<__m128i>(), v);
        }
    }
    blocks * 16
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

/// 自由选区：多边形（相对图片左上角的物理像素，首尾自动相连）以外的像素变成白色、完全透明。
/// 存成 PNG 是透明的，存成 JPG 是白底（JPG 没有透明，只设透明的话原来的画面会露出来）。
/// 按扫描线做奇偶填充：每一行算出和多边形各边的交点，成对的交点之间算在形状里。
pub fn mask_polygon(image: &mut RgbaImage, points: &[[f64; 2]]) {
    if points.len() < 3 {
        return;
    }
    let (w, h) = image.dimensions();
    let mut xs: Vec<f64> = Vec::with_capacity(16);
    for y in 0..h {
        // 取像素中心那条线
        let cy = f64::from(y) + 0.5;
        xs.clear();
        for i in 0..points.len() {
            let [x1, y1] = points[i];
            let [x2, y2] = points[(i + 1) % points.len()];
            if (y1 <= cy && cy < y2) || (y2 <= cy && cy < y1) {
                xs.push(x1 + (cy - y1) / (y2 - y1) * (x2 - x1));
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        let mut inside_spans = xs.chunks_exact(2).map(|p| (p[0], p[1])).peekable();
        for x in 0..w {
            let cx = f64::from(x) + 0.5;
            while inside_spans.peek().is_some_and(|&(_, end)| end < cx) {
                inside_spans.next();
            }
            let inside = inside_spans.peek().is_some_and(|&(start, end)| start <= cx && cx <= end);
            if !inside {
                image.put_pixel(x, y, image::Rgba([255, 255, 255, 0]));
            }
        }
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

/// 剪贴板卡片标题栏的颜色（照 Paste 量出来的做法）：取应用图标"底色"——图标方块边缘一圈的颜色，
/// 色相、亮度不变，饱和度往上推，越暗的推得越多（深灰带点蓝的图标出来是藏蓝，不是一块死灰；
/// 本来就鲜艳的只再艳一点），亮度封顶 0.95。底色是白的 / 浅灰的（Edge、很多 Windows 图标）没法代表这个应用，
/// 退回按图标里的主色算（`dominant_color`）；底色是黑 / 灰的（终端）就用中性色。
pub fn header_color(img: &RgbaImage) -> Option<[u8; 3]> {
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    let step = (w.max(h) / 128).max(1) as usize;
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            if img.get_pixel(x, y)[3] > 200 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (bw, bh) = (f64::from(x1 - x0), f64::from(y1 - y0));
    let (mut n, mut sum) = (0f64, [0f64; 3]);
    for y in (y0..=y1).step_by(step) {
        for x in (x0..=x1).step_by(step) {
            let p = img.get_pixel(x, y);
            if p[3] < 230 {
                continue;
            }
            let fx = f64::from(x - x0) / bw;
            let fy = f64::from(y - y0) / bh;
            // 边上那一圈（避开最外面抗锯齿 / 阴影的一两像素）
            let edge = fx.min(1.0 - fx).min(fy).min(1.0 - fy);
            if !(0.025..0.14).contains(&edge) {
                continue;
            }
            let [r, g, b] = [p[0], p[1], p[2]].map(f64::from);
            // 图案伸到边上的白色笔画（Claude 的星芒）不算底色
            let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
            if mn > 225.0 && mx - mn < 20.0 {
                continue;
            }
            n += 1.0;
            for (acc, c) in sum.iter_mut().zip([r, g, b]) {
                *acc += c;
            }
        }
    }
    if n < 8.0 {
        return dominant_color(img);
    }
    let [r, g, b] = sum.map(|c| c / n / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
    // 白 / 浅灰底：看图案的颜色
    if sat < 0.12 && max > 0.75 {
        return dominant_color(img);
    }
    // 黑 / 灰底：中性色
    if sat < 0.1 {
        return Some(if max < 0.35 { [0x1c, 0x1c, 0x1e] } else { [r, g, b].map(|c| (c * 255.0).round() as u8) });
    }
    let hue = {
        let d = max - min;
        if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        }
    };
    let s2 = (sat + (1.0 - sat) * (1.0 - 0.83 * max)).clamp(0.0, 0.97);
    // 亮的再提一点（Paste 的 Claude 橙比图标底色略亮），暗的保持
    let v2 = if max > 0.6 { max + 0.04 } else { max }.clamp(0.14, 0.95);
    Some(hsv_to_rgb(hue, s2, v2))
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r, g, b].map(|t| ((t + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// 高质量缩小（Lanczos3）。先预乘透明度再缩、缩完再还原：直接缩，透明处的黑色会渗进图标边缘，
/// 圆角外面一圈发暗
pub fn resize_sharp(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let mut pre = img.clone();
    for p in pre.pixels_mut() {
        let a = u16::from(p[3]);
        for c in &mut p.0[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
    let mut out = image::imageops::resize(&pre, w, h, image::imageops::FilterType::Lanczos3);
    for p in out.pixels_mut() {
        let a = u16::from(p[3]);
        if a == 0 {
            p.0 = [0, 0, 0, 0];
            continue;
        }
        for c in &mut p.0[..3] {
            *c = ((u16::from(*c) * 255 + a / 2) / a).min(255) as u8;
        }
    }
    out
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
    fn header_color_follows_icon_background() {
        // 带一点蓝的深灰底 → 很深的藏蓝（Paste 给 DATO OCR 的就是这样）
        let navy = header_color(&RgbaImage::from_pixel(64, 64, image::Rgba([34, 37, 46, 255]))).unwrap();
        assert!(navy[2] > navy[0] + 25 && navy[2] < 70, "{navy:?}");
        // 鲜艳的橙底：还是橙，只是更艳一点
        let orange = header_color(&RgbaImage::from_pixel(64, 64, image::Rgba([240, 124, 80, 255]))).unwrap();
        assert!(orange[0] > 230 && orange[2] < 75, "{orange:?}");
        // 白底：看图案（这里是一块蓝）
        let mut white = RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 255]));
        for x in 16..48 {
            for y in 16..48 {
                white.put_pixel(x, y, image::Rgba([20, 110, 230, 255]));
            }
        }
        let blue = header_color(&white).unwrap();
        assert!(blue[2] > 180 && blue[0] < 80, "{blue:?}");
    }

    /// SSSE3 整块和收尾的逐像素两段结果一样（长度故意不是 16 的倍数）
    #[test]
    fn swap_rb_matches_scalar() {
        let src: Vec<u8> = (0..4 * 37).map(|i| (i * 31 % 251) as u8).collect();
        let mut dst = vec![0; src.len()];
        swap_rb_opaque(&src, &mut dst);
        for (d, s) in dst.chunks_exact(4).zip(src.chunks_exact(4)) {
            assert_eq!(d, [s[2], s[1], s[0], 255]);
        }
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

#[cfg(test)]
mod polygon_tests {
    use super::*;

    #[test]
    fn masks_outside_a_triangle() {
        let mut img = RgbaImage::from_pixel(10, 10, image::Rgba([10, 20, 30, 255]));
        // 左上角的直角三角形：(0,0) (10,0) (0,10)
        mask_polygon(&mut img, &[[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]]);
        assert_eq!(img.get_pixel(1, 1).0, [10, 20, 30, 255]);
        assert_eq!(img.get_pixel(8, 8).0, [255, 255, 255, 0]);
        assert_eq!(img.get_pixel(9, 0).0, [10, 20, 30, 255]);
        assert_eq!(img.get_pixel(9, 9).0, [255, 255, 255, 0]);
    }

    #[test]
    fn concave_shape_keeps_both_lobes() {
        // U 形：中间那一列凹进去
        let mut img = RgbaImage::from_pixel(9, 9, image::Rgba([0, 0, 0, 255]));
        let u = [[0.0, 0.0], [3.0, 0.0], [3.0, 6.0], [6.0, 6.0], [6.0, 0.0], [9.0, 0.0], [9.0, 9.0], [0.0, 9.0]];
        mask_polygon(&mut img, &u);
        assert_eq!(img.get_pixel(1, 1).0[3], 255);
        assert_eq!(img.get_pixel(7, 1).0[3], 255);
        assert_eq!(img.get_pixel(4, 2).0[3], 0);
        assert_eq!(img.get_pixel(4, 8).0[3], 255);
    }

    #[test]
    fn fewer_than_three_points_is_a_noop() {
        let mut img = RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        mask_polygon(&mut img, &[[0.0, 0.0], [4.0, 4.0]]);
        assert!(img.pixels().all(|p| p.0[3] == 255));
    }
}
