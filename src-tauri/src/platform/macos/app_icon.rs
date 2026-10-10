//! 取应用图标（剪贴板来源应用显示用，规格 05 §1.6）。

use std::path::Path;

use image::RgbaImage;
use objc2::rc::Retained;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSString;

use super::{geometry, util};

/// 剪贴板卡片标题栏里的图标有 40 多点，Retina 屏上就是 80–100 像素；按 256 取，缩小显示才清楚
const ICON_SIZE: u32 = 256;

/// `path` 是 .app 的路径（见 `window_enum::app_info_of`）。
pub fn extract(path: &Path) -> Option<RgbaImage> {
    let icon =
        NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&path.to_string_lossy()));
    let mut rect = geometry::rect(0.0, 0.0, f64::from(ICON_SIZE), f64::from(ICON_SIZE));
    // SAFETY: 建议矩形是栈上的变量；图标里有多种尺寸，系统按这个矩形挑最接近的一份。
    let cg = unsafe { icon.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    util::rgba_from_cgimage(
        Retained::as_ptr(&cg).cast_mut().cast(),
        ICON_SIZE,
        ICON_SIZE,
        false,
    )
}
