//! Windows 自带 OCR（Windows.Media.Ocr）。
//!
//! 零体积、零下载，作为 RapidOCR 不可用时的后备引擎。准确率取决于系统装了哪些
//! 语言的 OCR 组件（中文系统一般自带简体中文）。

use image::RgbaImage;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

use crate::error::{AppError, AppResult};
use crate::platform::SysOcrLine;

fn engine() -> AppResult<OcrEngine> {
    // 中文优先：用户界面是英文时 TryCreateFromUserProfileLanguages 会选英文引擎，
    // 认不出中文。系统装了中文 OCR 组件就用中文（它同时认英文）。
    for tag in ["zh-Hans-CN", "zh-Hans", "zh-CN"] {
        if let Ok(lang) = Language::CreateLanguage(&HSTRING::from(tag)) {
            if OcrEngine::IsLanguageSupported(&lang).unwrap_or(false) {
                if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang) {
                    return Ok(engine);
                }
            }
        }
    }
    OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|_| AppError::msg("系统没有可用的 OCR 语言包"))
}

pub fn available() -> bool {
    engine().is_ok()
}

pub fn recognize(image: &RgbaImage) -> AppResult<Vec<SysOcrLine>> {
    let engine = engine()?;
    let max = OcrEngine::MaxImageDimension().unwrap_or(10_000).max(1);
    let (w, h) = image.dimensions();
    let (scaled, scale) = if w > max || h > max {
        let s = f64::from(max) / f64::from(w.max(h));
        let img = image::imageops::resize(
            image,
            ((f64::from(w) * s) as u32).max(1),
            ((f64::from(h) * s) as u32).max(1),
            image::imageops::FilterType::Triangle,
        );
        (img, s)
    } else {
        (image.clone(), 1.0)
    };

    let mut bgra = scaled.into_raw();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let (sw, sh) = (
        (f64::from(w) * scale).max(1.0) as i32,
        (f64::from(h) * scale).max(1.0) as i32,
    );
    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let buffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, sw, sh)?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;

    let mut lines = Vec::new();
    for line in result.Lines()? {
        let mut text = String::new();
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let mut prev_tight = false;
        for (i, word) in line.Words()?.into_iter().enumerate() {
            let t = word.Text()?.to_string_lossy();
            let r = word.BoundingRect()?;
            x0 = x0.min(f64::from(r.X));
            y0 = y0.min(f64::from(r.Y));
            x1 = x1.max(f64::from(r.X + r.Width));
            y1 = y1.max(f64::from(r.Y + r.Height));
            // Windows OCR 把中文拆成单字"词"，词间不该加空格；西文词之间要加
            // CJK 和紧贴型标点（. : , - 等）两侧不加空格：Windows OCR 会把 "10:08" 拆成三个词
            let tight_start = t.chars().next().is_some_and(is_tight);
            if i > 0 && !tight_start && !prev_tight {
                text.push(' ');
            }
            prev_tight = t.chars().last().is_some_and(is_tight);
            text.push_str(&t);
        }
        if text.trim().is_empty() || x1 <= x0 {
            continue;
        }
        lines.push(SysOcrLine {
            text,
            x: x0 / scale,
            y: y0 / scale,
            width: (x1 - x0) / scale,
            height: (y1 - y0) / scale,
        });
    }
    Ok(lines)
}

fn is_tight(c: char) -> bool {
    crate::storage::tokenize::is_cjk(c)
        || matches!(
            c,
            '.' | ':'
                | ','
                | ';'
                | '/'
                | '-'
                | '('
                | ')'
                | '%'
                | '，'
                | '。'
                | '：'
                | '、'
                | '（'
                | '）'
        )
}
