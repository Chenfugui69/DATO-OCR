//! 窗口特效。M0 只需要"把自己从别人的抓屏结果里排除"。
//!
//! 规格 07 §4.4：`WDA_EXCLUDEFROMCAPTURE` 需要 Windows 10 2004+。更老的系统上
//! 调用会失败，此时静默降级 —— 后果只是长截图可能拍到自己的提示条，不值得
//! 让整个功能不可用。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
};

use crate::error::{AppError, AppResult};
use crate::platform::WindowEffects;

pub struct WindowsWindowEffects;

impl WindowEffects for WindowsWindowEffects {
    fn native_handle(&self, window: &tauri::WebviewWindow) -> AppResult<u64> {
        let handle = window
            .hwnd()
            .map_err(|err| AppError::Window(format!("取窗口句柄失败: {err}")))?;
        Ok(handle.0 as u64)
    }

    fn exclude_from_capture(&self, window: &tauri::WebviewWindow, enabled: bool) -> AppResult<()> {
        let handle = window
            .hwnd()
            .map_err(|err| AppError::Window(format!("取窗口句柄失败: {err}")))?;

        let affinity = if enabled && super::self_capture::excluded() {
            WDA_EXCLUDEFROMCAPTURE
        } else {
            WDA_NONE
        };

        // SAFETY: `handle` 来自 Tauri 的存活窗口，调用期间窗口不会被销毁
        // （本函数只在主线程、窗口创建之后调用）。
        let result = unsafe { SetWindowDisplayAffinity(HWND(handle.0), affinity) };

        if let Err(err) = result {
            tracing::debug!("SetWindowDisplayAffinity 不可用，跳过抓屏排除: {err}");
        }

        Ok(())
    }
}
