//! 底图的编码格式。
//!
//! BMP 和 PNG 两条路都留着，因为它们在延迟预算里的取舍完全相反，而"哪个更快"
//! 只能靠实测（4K 单屏实测数据见 `protocol` 的文件头）：
//!
//! - **BMP**：编码就是搬一遍内存（4K 约 30ms），但字节多（33MB），传输慢。
//! - **PNG**：字节少（截图内容压缩比很高），传输快，但编码要压缩、
//!   解码也比 BMP 贵，两头都加在关键路径上。
//!
//! 用环境变量选，默认 BMP。这样 A/B 对比不用重新编译 —— release 构建一次要
//! 四分钟，靠改代码来回切会把时间全耗在编译上。

use std::sync::OnceLock;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, RgbaImage};

use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bmp,
    Png,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Bmp => "bmp",
            Format::Png => "png",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Format::Bmp => "image/bmp",
            Format::Png => "image/png",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "bmp" => Some(Format::Bmp),
            "png" => Some(Format::Png),
            _ => None,
        }
    }
}

/// `CHENOCR_BACKDROP_FORMAT=png` 切到 PNG，其余情况用 BMP。
pub fn configured() -> Format {
    static CACHED: OnceLock<Format> = OnceLock::new();
    *CACHED.get_or_init(|| {
        match std::env::var("CHENOCR_BACKDROP_FORMAT")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => Format::Png,
            _ => Format::Bmp,
        }
    })
}

pub fn encode(image: &RgbaImage, format: Format) -> AppResult<Vec<u8>> {
    match format {
        Format::Bmp => Ok(super::bmp::encode(image)),
        Format::Png => encode_png(image),
    }
}

/// PNG 编码，刻意用最快的档位。
///
/// `CompressionType::Fast` + `FilterType::NoFilter`：这里要的是"尽快把字节数
/// 降下来"，不是最小体积。默认档位（zlib 级别高 + 自适应滤波）在 4K 全屏上要
/// 好几百毫秒，而整条链路的预算只有 150ms，光编码就超了。
///
/// 不做滤波还有个附带好处：滤波要逐行回看上一行，是随机访问；关掉之后
/// zlib 拿到的就是顺序流。
fn encode_png(image: &RgbaImage) -> AppResult<Vec<u8>> {
    // 截图内容压缩比通常在 5–10 倍，先按 1/6 预留，省掉几次扩容重分配。
    let mut out = Vec::with_capacity(image.as_raw().len() / 6);

    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::NoFilter)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|err| AppError::Image(format!("PNG 编码失败: {err}")))?;

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 扩展名与格式一一对应() {
        for format in [Format::Bmp, Format::Png] {
            assert_eq!(Format::from_extension(format.extension()), Some(format));
        }
        assert_eq!(Format::from_extension("jpg"), None);
    }

    #[test]
    fn png_编码出的字节带正确的魔数() {
        let image = RgbaImage::from_pixel(4, 4, image::Rgba([10, 20, 30, 255]));
        let bytes = encode(&image, Format::Png).expect("PNG 编码应当成功");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn png_能被解回原始像素() {
        // 无损是硬要求：遮罩底图必须与真实桌面像素级一致（规格 08 M0 验收项）。
        let mut image = RgbaImage::new(8, 8);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Rgba([x as u8 * 7, y as u8 * 11, 200, 255]);
        }

        let bytes = encode(&image, Format::Png).expect("PNG 编码应当成功");
        let decoded = image::load_from_memory(&bytes)
            .expect("PNG 应当能解回来")
            .to_rgba8();

        assert_eq!(decoded.dimensions(), image.dimensions());
        assert_eq!(decoded.as_raw(), image.as_raw());
    }
}
