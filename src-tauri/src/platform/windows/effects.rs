//! Tauri 窗口上的原生小动作。

use tauri::WebviewWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE,
    SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_EX_NOACTIVATE,
};

use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
    DWM_WINDOW_CORNER_PREFERENCE,
};

use super::util::{handle_of, hwnd};
use crate::error::AppResult;

pub fn native_handle(window: &WebviewWindow) -> AppResult<u64> {
    Ok(handle_of(window.hwnd()?))
}

/// 诊断开关：`CHENOCR_ALLOW_SELF_CAPTURE=1` 时，浮动窗口**显示期间**不排除抓屏。
/// 正常必须排除（否则连续截图会拍到上一次的遮罩），但排除后任何脚本化的视觉验证都
/// 截不到遮罩 —— 这个开关是自动化截屏检查的唯一手段。
///
/// 隐藏期间仍然排除：实测在本进程内用 WGC 抓屏时，没被排除的**隐藏**窗口会被画成
/// 带标题栏的白块盖在画面上（独立进程抓同一时刻的屏幕则正常）。所以诊断模式只在
/// 显示时放开，见 [`reveal_for_tests`]。
pub fn self_capture_allowed() -> bool {
    std::env::var("CHENOCR_ALLOW_SELF_CAPTURE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn set_exclude_from_capture(window: &WebviewWindow, exclude: bool) {
    let Ok(h) = window.hwnd() else { return };
    let affinity = if exclude {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // SAFETY: h 是存活的 Tauri 窗口。Win10 2004 以下不支持 EXCLUDE，静默降级。
    if let Err(err) = unsafe { SetWindowDisplayAffinity(h, affinity) } {
        tracing::debug!(label = window.label(), "设置抓屏排除失败：{err}");
    }
}

/// Win11 系统圆角（无边框窗口默认是直角）。毛玻璃材质会跟着圆角裁，阴影也是系统画的。
/// Win10 没有这个属性，调用失败就算了。
pub fn set_rounded(window: &WebviewWindow, rounded: bool) {
    let Ok(h) = window.hwnd() else { return };
    let pref: DWM_WINDOW_CORNER_PREFERENCE = if rounded {
        DWMWCP_ROUND
    } else {
        DWMWCP_DONOTROUND
    };
    // SAFETY: h 是存活的 Tauri 窗口；传入的是一个 4 字节枚举值及其大小。
    let result = unsafe {
        DwmSetWindowAttribute(
            h,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&pref).cast(),
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        )
    };
    if let Err(err) = result {
        tracing::debug!(label = window.label(), "设置窗口圆角失败：{err}");
    }
}

pub fn hide_window(window: &WebviewWindow) {
    let Ok(h) = window.hwnd() else { return };
    // SAFETY: h 是存活的 Tauri 窗口。
    unsafe {
        let _ = ShowWindow(h, SW_HIDE);
    }
}

/// 加上 `WS_EX_NOACTIVATE`：点击、显示都不会把它变成前台窗口。
pub fn set_no_activate(window: &WebviewWindow) {
    let Ok(h) = window.hwnd() else { return };
    // SAFETY: h 是存活的 Tauri 窗口；只追加一个扩展样式位。
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
    }
}

/// 置顶显示但不激活（toast 不能抢走用户正在打字的窗口的焦点）。
pub fn show_without_activate(window: &WebviewWindow) -> AppResult<()> {
    let h = hwnd(native_handle(window)?);
    // SAFETY: h 是存活的 Tauri 窗口。
    unsafe {
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        SetWindowPos(
            h,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )?;
    }
    Ok(())
}

/// 诊断模式下，窗口显示时放开抓屏、隐藏时恢复排除。非诊断模式什么都不做。
pub fn reveal_for_tests(window: &WebviewWindow, visible: bool) {
    if self_capture_allowed() {
        set_exclude_from_capture(window, !visible);
    }
}

pub fn reveal_hwnd_for_tests(handle: u64, visible: bool) {
    if self_capture_allowed() {
        let affinity = if visible {
            WDA_NONE
        } else {
            WDA_EXCLUDEFROMCAPTURE
        };
        // SAFETY: 句柄来自本进程创建的窗口。
        let _ = unsafe { SetWindowDisplayAffinity(hwnd(handle), affinity) };
    }
}
