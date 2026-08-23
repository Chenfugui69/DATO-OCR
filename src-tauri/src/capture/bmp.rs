//! 把 RGBA 位图包成 BMP。
//!
//! # 为什么是 BMP
//!
//! 遮罩窗口要在 150ms 内把"按下热键那一刻的屏幕"显示出来（规格 00 §6.4）。
//! BMP 的编码就是加个 54 字节文件头 + 一趟通道重排，4K 实测 8ms；WebView2
//! （Chromium）原生就能解，前端拿到就能画。
//!
//! PNG 试过，是条死路：`CompressionType::Fast` + `FilterType::NoFilter` 在 4K
//! 桌面上编码要 59ms，而字节数是 33.18MB —— 比 BMP 的 33.17MB 还大。桌面上摊着
//! 照片壁纸，不做滤波的话 zlib 一级压缩基本压不动。换自适应滤波能压下来，但编码
//! 时间又要翻几倍，两头都在关键路径上，怎么摆都是亏。见 `backdrop` 模块。
//!
//! # 为什么是 24bpp
//!
//! 底图是屏幕内容，本来就不透明，alpha 通道纯属白搭 —— 而它占了 25% 的字节数
//! （4K 下 33MB 里的 8MB）。实测传输是整条链路最大的一段，省这 8MB 直接就是
//! 省时间。
//!
//! 还有个附带好处：32bpp 那版必须逐像素把 alpha 强写成 255，否则整块底图会透明
//! （抓屏给回来的 alpha 字节是没有语义的垃圾，GDI 的 `GetDIBits` 在 32bpp
//! `BI_RGB` 下明确说这个字节"未使用"，实测常年是 0；WGC 那条路一样）。24bpp
//! 压根没有 alpha 通道，这个坑从结构上就不存在了。
//!
//! 用 `BI_RGB` + 负高度（自上而下），这样行序和 `RgbaImage` 一致，不用翻转。

use image::RgbaImage;

const FILE_HEADER_LEN: usize = 14;
const INFO_HEADER_LEN: usize = 40;
pub const HEADER_LEN: usize = FILE_HEADER_LEN + INFO_HEADER_LEN;

/// BMP 每行必须按 4 字节对齐，24bpp 下不是天然满足的，要补 0–3 个填充字节。
fn stride(width: u32) -> usize {
    (width as usize * 3).next_multiple_of(4)
}

pub fn encode(image: &RgbaImage) -> Vec<u8> {
    let width = image.width();
    let height = image.height();
    let stride = stride(width);
    let pixel_bytes = stride * height as usize;

    let mut out = Vec::with_capacity(HEADER_LEN + pixel_bytes);

    // ── BITMAPFILEHEADER ────────────────────────────────────────────────
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((HEADER_LEN + pixel_bytes) as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // bfReserved1
    out.extend_from_slice(&0u16.to_le_bytes()); // bfReserved2
    out.extend_from_slice(&(HEADER_LEN as u32).to_le_bytes()); // bfOffBits

    // ── BITMAPINFOHEADER ────────────────────────────────────────────────
    out.extend_from_slice(&(INFO_HEADER_LEN as u32).to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    // 负高度 = 像素自上而下排列，与 RgbaImage 的行序相同
    out.extend_from_slice(&(height as i32).wrapping_neg().to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&24u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    out.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

    // ── 像素 RGBA → BGR ─────────────────────────────────────────────────
    // 先 resize 再按行按块写。resize 填的 0 顺带就是行尾的填充字节，不用单独补。
    // 逐像素 `extend_from_slice` 不行 —— 4K 有 830 万像素，光函数调用开销就够慢。
    out.resize(HEADER_LEN + pixel_bytes, 0);

    let src_stride = width as usize * 4;
    for (dst_row, src_row) in out[HEADER_LEN..]
        .chunks_exact_mut(stride)
        .zip(image.as_raw().chunks_exact(src_stride))
    {
        // 只遍历有像素的那一段，行尾填充留着 resize 填的 0。
        for (dst, src) in dst_row[..width as usize * 3]
            .chunks_exact_mut(3)
            .zip(src_row.chunks_exact(4))
        {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn header_declares_top_down_24bpp() {
        let image = RgbaImage::from_pixel(2, 3, Rgba([1, 2, 3, 255]));
        let bmp = encode(&image);

        assert_eq!(&bmp[0..2], b"BM");
        // 宽 2 → 每行 6 字节，补到 8
        assert_eq!(bmp.len(), HEADER_LEN + 8 * 3);
        assert_eq!(u32::from_le_bytes(bmp[2..6].try_into().unwrap()), bmp.len() as u32);
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), HEADER_LEN as u32);
        assert_eq!(i32::from_le_bytes(bmp[18..22].try_into().unwrap()), 2);
        // 负高度表示自上而下
        assert_eq!(i32::from_le_bytes(bmp[22..26].try_into().unwrap()), -3);
        assert_eq!(u16::from_le_bytes(bmp[28..30].try_into().unwrap()), 24);
    }

    #[test]
    fn pixels_are_written_as_bgr() {
        let image = RgbaImage::from_pixel(1, 1, Rgba([10, 20, 30, 255]));
        let bmp = encode(&image);
        // 宽 1 → 3 字节补到 4，最后一个是填充
        assert_eq!(&bmp[HEADER_LEN..], &[30, 20, 10, 0]);
    }

    #[test]
    fn 抓屏带回来的垃圾_alpha_不影响输出() {
        // 抓屏给回来的 alpha 常年是 0。24bpp 根本不存这个通道，所以它进不来。
        let image = RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 0]));
        let bmp = encode(&image);
        for row in bmp[HEADER_LEN..].chunks_exact(stride(2)) {
            assert_eq!(&row[..6], &[30, 20, 10, 30, 20, 10]);
        }
    }

    #[test]
    fn 行填充不会把像素挤错位() {
        // 宽 3 → 每行 9 字节补到 12。行填充算错的话第二行会整体偏移，
        // 表现是底图斜着扭一下 —— 只有非 4 整数倍的宽度才暴露。
        let mut image = RgbaImage::new(3, 2);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgba([x as u8, y as u8, 200, 255]);
        }

        let bmp = encode(&image);
        let rows: Vec<_> = bmp[HEADER_LEN..].chunks_exact(12).collect();
        assert_eq!(rows.len(), 2);

        for (y, row) in rows.iter().enumerate() {
            for x in 0..3usize {
                let px = &row[x * 3..x * 3 + 3];
                assert_eq!(px, &[200, y as u8, x as u8], "行 {y} 像素 {x}");
            }
            assert_eq!(&row[9..], &[0, 0, 0], "行 {y} 的填充字节");
        }
    }

    #[test]
    fn 无损_解回来要和原图一致() {
        // 底图必须与真实桌面像素级一致（规格 08 M0 验收项）。
        let mut image = RgbaImage::new(7, 5);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgba([x as u8 * 31, y as u8 * 47, 123, 0]);
        }

        let decoded = image::load_from_memory(&encode(&image))
            .expect("BMP 应当能解回来")
            .to_rgb8();

        assert_eq!(decoded.dimensions(), image.dimensions());
        for (x, y, pixel) in decoded.enumerate_pixels() {
            let src = image.get_pixel(x, y);
            assert_eq!(pixel.0, [src[0], src[1], src[2]], "像素 ({x}, {y})");
        }
    }
}
