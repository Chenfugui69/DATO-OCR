//! 系统权限：屏幕录制、辅助功能、完全磁盘访问。
//!
//! macOS 把这几项权限记在"应用"头上。开发时从终端跑的是裸可执行文件，权限算在终端程序
//! 头上；打包后的 .app 才算在 DATO OCR 自己头上。授权后一般要重启应用才生效。

use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSString};

use super::ffi;
use crate::error::{AppError, AppResult};
use crate::platform::Permission;

/// 设置页上"去授权"按钮：先让系统弹它自己的授权框（同时把应用登记进列表），
/// 再打开系统设置里对应的那一页。
pub fn request(which: Permission) {
    let pane = match which {
        Permission::ScreenCapture => {
            // SAFETY: 无参数。
            unsafe { ffi::CGRequestScreenCaptureAccess() };
            "Privacy_ScreenCapture"
        }
        Permission::Accessibility => {
            request_accessibility();
            "Privacy_Accessibility"
        }
        // 没有弹框的接口：只能打开那一页，让用户点 + 把 DATO OCR 加进去
        Permission::FullDiskAccess => "Privacy_AllFiles",
    };
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
    if let Err(err) = std::process::Command::new("open").arg(url).spawn() {
        tracing::warn!("打开系统设置失败：{err}");
    }
}

pub fn screen_capture_granted() -> bool {
    // SAFETY: 无参数的查询。
    unsafe { ffi::CGPreflightScreenCaptureAccess() }
}

/// 没有屏幕录制权限时系统只给桌面背景和自己的窗口，看着像截到了、其实是空的，所以直接报错。
/// 第一次会弹系统的授权框，并把应用登记进"录屏与系统录音"列表。
pub fn ensure_screen_capture() -> AppResult<()> {
    if screen_capture_granted() {
        return Ok(());
    }
    // SAFETY: 无参数。已经问过一次之后这个调用不再弹框，只返回当前状态。
    if unsafe { ffi::CGRequestScreenCaptureAccess() } {
        return Ok(());
    }
    Err(AppError::Capture(
        "需要「屏幕录制」权限：系统设置 → 隐私与安全性 → 录屏与系统录音，打开 DATO OCR 后重启应用"
            .into(),
    ))
}

pub fn accessibility_granted() -> bool {
    // SAFETY: 无参数的查询。
    unsafe { ffi::AXIsProcessTrusted() }
}

/// 模拟按键、拦截按键都要「辅助功能」权限。没有的话弹系统的授权引导。
pub fn ensure_accessibility() -> AppResult<()> {
    if accessibility_granted() {
        return Ok(());
    }
    request_accessibility();
    Err(AppError::msg(
        "需要「辅助功能」权限：系统设置 → 隐私与安全性 → 辅助功能，打开 DATO OCR 后重试",
    ))
}

pub fn request_accessibility() -> bool {
    // SAFETY: 键是系统常量，值是 kCFBooleanTrue；NSDictionary 与 CFDictionary 可直接互换。
    unsafe {
        let key = &*ffi::kAXTrustedCheckOptionPrompt.cast::<NSString>();
        let value = &*ffi::kCFBooleanTrue.cast::<AnyObject>();
        let options = NSDictionary::from_slices(&[key], &[value]);
        ffi::AXIsProcessTrustedWithOptions(std::ptr::from_ref(&*options).cast())
    }
}
