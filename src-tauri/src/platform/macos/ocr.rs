//! 系统自带的文字识别（Vision 框架）。
//!
//! macOS 上这就是主力引擎：离线、零体积、中英文混排的准确率很高，不需要再带一个
//! RapidOCR。每个结果是一行文字和它的包围盒。

use image::RgbaImage;
use objc2::rc::{autoreleasepool, Retained};
use objc2::AnyThread;
use objc2_foundation::{NSArray, NSDictionary, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

use super::util;
use crate::error::{AppError, AppResult};
use crate::platform::SysOcrLine;

/// 识别语言，排前面的优先。简体中文的模型同时认英文和数字。
const LANGUAGES: [&str; 3] = ["zh-Hans", "zh-Hant", "en-US"];

pub fn available() -> bool {
    true
}

pub fn recognize(image: &RgbaImage) -> AppResult<Vec<SysOcrLine>> {
    let (w, h) = (f64::from(image.width()), f64::from(image.height()));
    let cg = util::cgimage_from_rgba(image, true).ok_or_else(|| AppError::msg("图片转换失败"))?;
    autoreleasepool(|_| {
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        let languages: Vec<Retained<NSString>> =
            LANGUAGES.iter().map(|l| NSString::from_str(l)).collect();
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));

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

        let mut lines = Vec::new();
        for observation in request.results().iter().flat_map(|r| r.iter()) {
            let Some(best) = observation.topCandidates(1).firstObject() else {
                continue;
            };
            let text = best.string().to_string();
            if text.trim().is_empty() {
                continue;
            }
            // 包围盒是 0–1 的比例坐标，原点在左下角
            let b = unsafe { observation.boundingBox() };
            lines.push(SysOcrLine {
                text,
                x: b.origin.x * w,
                y: (1.0 - b.origin.y - b.size.height) * h,
                width: b.size.width * w,
                height: b.size.height * h,
            });
        }
        Ok(lines)
    })
}

#[cfg(test)]
mod tests {
    /// 真机烟雾测试：`CHENOCR_SMOKE_IMAGE=图片路径 cargo test -- --ignored smoke_recognize --nocapture`
    #[test]
    #[ignore]
    fn smoke_recognize() {
        let path = std::env::var("CHENOCR_SMOKE_IMAGE").unwrap();
        let image = image::open(&path).unwrap().to_rgba8();
        let started = std::time::Instant::now();
        let lines = super::recognize(&image).unwrap();
        println!(
            "{} lines in {}ms ({}x{})",
            lines.len(),
            started.elapsed().as_millis(),
            image.width(),
            image.height()
        );
        for l in &lines {
            println!(
                "[{:.0},{:.0} {:.0}x{:.0}] {}",
                l.x, l.y, l.width, l.height, l.text
            );
        }
        assert!(!lines.is_empty());
    }
}
