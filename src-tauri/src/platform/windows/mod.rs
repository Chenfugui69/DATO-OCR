//! Windows 平台实现。

pub mod app_icon;
pub mod backdrop;
pub mod capture;
pub mod clipboard;
pub mod codes;
pub mod cursor;
pub mod effects;
pub mod elements;
pub mod hook;
pub mod input;
pub mod media;
pub mod ocr;
pub mod process;
pub mod record_input;
pub mod secret;
pub mod selection_hook;
pub mod system_events;
pub mod system_info;
pub mod webview_idle;
pub mod window_enum;

mod util;

pub use super::generic::{
    after_silent_start, is_reopen_event, is_speaking, menu_label, permissions, request_permission,
    sms_supported, speak, start_sms_watcher, stop_speaking, voices,
};
pub use codes::scan as scan_codes;
pub use media::{export_video, media_access, request_media_access, Camera, VideoWriter};
pub use elements::{elements_at, elements_near};
use super::types::DefaultHotkeys;

pub const DEFAULT_HOTKEYS: DefaultHotkeys = DefaultHotkeys {
    capture: "F1",
    longshot: "F2",
    ocr: "F3",
    clipboard: "Alt+V",
    translate: "Ctrl+Alt+T",
    instant: "Shift+F1",
};
pub const SYSTEM_OCR_NAME: &str = "Windows OCR";
pub const PADDLE_OCR_SUPPORTED: bool = true;
pub const TRAY_MENU_ON_LEFT_CLICK: bool = false;
pub const TRAY_ICON_IS_TEMPLATE: bool = false;

use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

pub fn init_process() {
    // 必须在任何窗口、任何抓屏 API 之前声明。Tauri 的清单通常已经声明过，
    // 那时这里会返回"已设置"错误，忽略即可。没有它，混合 DPI 下所有坐标都是错的。
    // SAFETY: 无参数副作用，只影响本进程。
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}
