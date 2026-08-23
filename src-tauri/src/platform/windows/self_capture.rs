//! 「把自己排除在别人的抓屏之外」这件事的总开关。
//!
//! 默认开着（规格 07 §4.4）：遮罩窗口和底图窗口都设 `WDA_EXCLUDEFROMCAPTURE`，
//! 这样长截图采集时不会把自己的提示条拍进去，用户用别的截图工具时也拍不到我们。
//!
//! 但这条规则让截图界面变得**没法用截图验证**：`WDA_EXCLUDEFROMCAPTURE` 是 DWM
//! 层面强制的，GDI、WGC、Desktop Duplication 全都拿不到内容，想截个图看看遮罩长
//! 什么样只会得到底下的真实桌面。所以留一个环境变量把它关掉：
//!
//! ```text
//! CHENOCR_ALLOW_SELF_CAPTURE=1
//! ```
//!
//! 只用于开发和验收（"混合 DPI 下背景像素级一致"这类验收项要靠眼睛看）。
//! 正常运行时不要开 —— 开了之后长截图会把自己拍进去。

use std::sync::OnceLock;

pub fn excluded() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        let allow = matches!(
            std::env::var("CHENOCR_ALLOW_SELF_CAPTURE")
                .unwrap_or_default()
                .as_str(),
            "1" | "true"
        );
        if allow {
            tracing::warn!("CHENOCR_ALLOW_SELF_CAPTURE 已开启：本进程的窗口会被别人抓到");
        }
        !allow
    })
}
