//! 系统自带的二维码 / 条码识别（Vision 框架的 VNDetectBarcodesRequest）。
//! 二维码、Data Matrix、PDF417、Aztec 和常见的一维条码（EAN、UPC、Code 128…）都认。

use image::RgbaImage;
use objc2::rc::{autoreleasepool, Retained};
use objc2::AnyThread;
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{VNDetectBarcodesRequest, VNImageRequestHandler, VNRequest};

use super::util;
use crate::error::{AppError, AppResult};
use crate::platform::ScannedCode;

pub fn scan(image: &RgbaImage) -> AppResult<Vec<ScannedCode>> {
    let (w, h) = (f64::from(image.width()), f64::from(image.height()));
    let cg = util::cgimage_from_rgba(image, true).ok_or_else(|| AppError::msg("图片转换失败"))?;
    autoreleasepool(|_| {
        // SAFETY: 普通的 Vision 请求对象，没有前置条件。
        let request = unsafe { VNDetectBarcodesRequest::new() };
        // SAFETY: cg 是有效的 CGImage，处理器创建时会持有它。
        let handler = unsafe {
            VNImageRequestHandler::initWithCGImage_options(
                VNImageRequestHandler::alloc(),
                &*cg.0.cast_const().cast(),
                &NSDictionary::new(),
            )
        };
        let requests: Retained<NSArray<VNRequest>> =
            NSArray::from_slice(&[&**request as &VNRequest]);
        handler
            .performRequests_error(&requests)
            .map_err(|err| AppError::msg(format!("识别失败：{}", err.localizedDescription())))?;

        let mut out: Vec<ScannedCode> = Vec::new();
        // SAFETY: 请求刚执行完，结果数组由它持有。
        let results = unsafe { request.results() };
        for observation in results.iter().flat_map(|r| r.iter()) {
            // SAFETY: 结果对象由请求持有，在这个自动释放池里一直有效。
            let Some(text) = (unsafe { observation.payloadStringValue() }) else {
                continue;
            };
            let text = text.to_string();
            if text.is_empty() {
                continue;
            }
            // SAFETY: 同上。
            let kind = symbology_name(&unsafe { observation.symbology() }.to_string());
            // 包围盒是 0–1 的比例坐标，原点在左下角
            let b = unsafe { observation.boundingBox() };
            // 同一个码 Vision 偶尔会报两次（不同符号体系各认一遍），按内容去重
            if out.iter().any(|c| c.text == text) {
                continue;
            }
            out.push(ScannedCode {
                kind,
                text,
                x: b.origin.x * w,
                y: (1.0 - b.origin.y - b.size.height) * h,
                width: b.size.width * w,
                height: b.size.height * h,
            });
        }
        Ok(out)
    })
}

/// `VNBarcodeSymbologyQR` → `QR`，`VNBarcodeSymbologyEAN13` → `EAN-13`
fn symbology_name(raw: &str) -> String {
    let name = raw.strip_prefix("VNBarcodeSymbology").unwrap_or(raw);
    match name {
        "QR" | "MicroQR" => "QR".into(),
        "EAN13" => "EAN-13".into(),
        "EAN8" => "EAN-8".into(),
        "UPCE" => "UPC-E".into(),
        "Code128" => "Code 128".into(),
        "Code39" | "Code39Checksum" | "Code39FullASCII" | "Code39FullASCIIChecksum" => {
            "Code 39".into()
        }
        "Code93" | "Code93i" => "Code 93".into(),
        "DataMatrix" => "Data Matrix".into(),
        "PDF417" | "MicroPDF417" => "PDF417".into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn symbology_names() {
        assert_eq!(super::symbology_name("VNBarcodeSymbologyQR"), "QR");
        assert_eq!(super::symbology_name("VNBarcodeSymbologyEAN13"), "EAN-13");
        assert_eq!(super::symbology_name("VNBarcodeSymbologyAztec"), "Aztec");
    }
}
