//! 二维码 / 条码识别：rxing（ZXing 的 Rust 移植）。二维码、Data Matrix、PDF417、Aztec 和常见的一维条码
//! （EAN、UPC、Code 128、Code 39…）都认。rxing 没认出二维码时再拿 rqrr 试一遍（它对糊了、缺角的二维码宽容些）。

use image::RgbaImage;
use rxing::{BarcodeFormat, DecodeHints};

use crate::error::AppResult;
use crate::platform::ScannedCode;

/// 比这大的图（整块 4K 屏）不用 rxing 的"尽力找"模式：几百万像素上要好几秒
const TRY_HARDER_MAX_PIXELS: u32 = 2_500_000;

pub fn scan(image: &RgbaImage) -> AppResult<Vec<ScannedCode>> {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return Ok(Vec::new());
    }
    let started = std::time::Instant::now();
    // 和常见的灰度公式一致（整数版）
    let luma: Vec<u8> = image
        .pixels()
        .map(|p| {
            ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u8
        })
        .collect();
    let mut hints = DecodeHints {
        TryHarder: Some(w * h <= TRY_HARDER_MAX_PIXELS),
        ..DecodeHints::default()
    };
    let mut out: Vec<ScannedCode> = Vec::new();
    // 没认出码时 rxing 报 NotFound，不算错
    if let Ok(results) = rxing::helpers::detect_multiple_in_luma_with_hints(luma, w, h, &mut hints)
    {
        for r in results {
            let text = r.getText().to_string();
            if text.is_empty() || out.iter().any(|c| c.text == text) {
                continue;
            }
            let pts = r.getPoints();
            if pts.is_empty() {
                continue;
            }
            let x0 = pts
                .iter()
                .map(|p| f64::from(p.x))
                .fold(f64::INFINITY, f64::min);
            let x1 = pts
                .iter()
                .map(|p| f64::from(p.x))
                .fold(f64::NEG_INFINITY, f64::max);
            let mut y0 = pts
                .iter()
                .map(|p| f64::from(p.y))
                .fold(f64::INFINITY, f64::min);
            let mut y1 = pts
                .iter()
                .map(|p| f64::from(p.y))
                .fold(f64::NEG_INFINITY, f64::max);
            // 一维码只给扫描线的两头：按宽的四分之一往上下撑开，框才看得见
            if y1 - y0 < 4.0 {
                let pad = ((x1 - x0) * 0.12).max(6.0);
                y0 = (y0 - pad).max(0.0);
                y1 = (y1 + pad).min(f64::from(h));
            }
            out.push(ScannedCode {
                kind: format_name(r.getBarcodeFormat()).into(),
                text,
                x: x0,
                y: y0,
                width: (x1 - x0).max(1.0),
                height: (y1 - y0).max(1.0),
            });
        }
    }
    if !out.iter().any(|c| c.kind == "QR") {
        for c in super::super::generic::scan_codes(image)? {
            if !out.iter().any(|o| o.text == c.text) {
                out.push(c);
            }
        }
    }
    tracing::debug!(
        count = out.len(),
        pixels = w * h,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "识别二维码 / 条码"
    );
    Ok(out)
}

/// 和 macOS 那边给的名字一致：QR、EAN-13、Code 128……
fn format_name(f: &BarcodeFormat) -> &'static str {
    match f {
        BarcodeFormat::QR_CODE
        | BarcodeFormat::MICRO_QR_CODE
        | BarcodeFormat::RECTANGULAR_MICRO_QR_CODE => "QR",
        BarcodeFormat::EAN_13 => "EAN-13",
        BarcodeFormat::EAN_8 => "EAN-8",
        BarcodeFormat::UPC_A => "UPC-A",
        BarcodeFormat::UPC_E => "UPC-E",
        BarcodeFormat::CODE_128 => "Code 128",
        BarcodeFormat::CODE_39 => "Code 39",
        BarcodeFormat::CODE_93 => "Code 93",
        BarcodeFormat::CODABAR => "Codabar",
        BarcodeFormat::ITF => "ITF",
        BarcodeFormat::DATA_MATRIX => "Data Matrix",
        BarcodeFormat::PDF_417 => "PDF417",
        BarcodeFormat::AZTEC => "Aztec",
        BarcodeFormat::MAXICODE => "MaxiCode",
        _ => "Barcode",
    }
}

#[cfg(test)]
mod tests {
    /// 真机烟雾测试：`CHENOCR_SMOKE_CODES=条码图片 cargo test -- --ignored smoke_scan_barcodes --nocapture`
    #[test]
    #[ignore]
    fn smoke_scan_barcodes() {
        let path = std::env::var("CHENOCR_SMOKE_CODES").unwrap();
        let image = image::open(&path).unwrap().to_rgba8();
        let codes = super::scan(&image).unwrap();
        println!("{codes:?}");
        assert!(!codes.is_empty());
    }
}
