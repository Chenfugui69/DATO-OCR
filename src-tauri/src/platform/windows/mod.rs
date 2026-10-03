//! Windows 平台实现。

pub mod app_icon;
pub mod backdrop;
pub mod capture;
pub mod clipboard;
pub mod cursor;
pub mod effects;
pub mod hook;
pub mod input;
pub mod ocr;
pub mod process;
pub mod secret;
pub mod selection_hook;
pub mod system_events;
pub mod system_info;
pub mod window_enum;

mod util;

use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

pub fn init_process() {
    // 必须在任何窗口、任何抓屏 API 之前声明。Tauri 的清单通常已经声明过，
    // 那时这里会返回"已设置"错误，忽略即可。没有它，混合 DPI 下所有坐标都是错的。
    // SAFETY: 无参数副作用，只影响本进程。
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}
