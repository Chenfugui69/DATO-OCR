//! 输入模拟与焦点还原。
//!
//! 浮层在 macOS 上都是"不激活应用"的面板（见 `effects::build_floating`）：面板出现时原来的
//! 应用一直是激活状态，面板一收起键盘焦点自己回到它那里。所以这里的"还原焦点"多数时候
//! 什么都不用做，只有目标应用确实不在前台时才去激活它。

use std::time::Duration;

use objc2_app_kit::{NSApplication, NSApplicationActivationOptions, NSWorkspace};

use super::ffi;
use super::util::{handle_pid, handle_window, on_main, own_pid};
use super::{geometry, permissions, window_enum};
use crate::error::{AppError, AppResult};
use crate::platform::WindowHandle;

pub fn cursor_position() -> Option<(i32, i32)> {
    window_enum::cursor_point().map(geometry::point_to_physical)
}

pub fn focus_window(window: WindowHandle) -> AppResult<()> {
    let pid = handle_pid(window.0);
    if pid == own_pid() {
        let number = handle_window(window.0) as isize;
        on_main(move |mtm| {
            let app = NSApplication::sharedApplication(mtm);
            if let Some(w) = app.windowWithWindowNumber(number) {
                w.makeKeyAndOrderFront(None);
            }
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        });
        return Ok(());
    }
    let front = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|a| a.processIdentifier());
    if front == Some(pid) {
        return Ok(());
    }
    let app = window_enum::running_app(pid).ok_or_else(|| AppError::msg("目标应用已退出"))?;
    #[allow(deprecated)]
    let ok = app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    if !ok {
        tracing::debug!("激活目标应用未成功，它可能拿不到焦点");
    }
    Ok(())
}

/// 长截图时滚轮该发给谁。macOS 的滚轮永远发给鼠标底下的窗口，和焦点无关，不用做任何事；
/// 遮罩面板还握着键盘焦点，正好用来收 Enter / Esc / Backspace。
pub fn route_scroll_to(_window: WindowHandle) -> AppResult<()> {
    Ok(())
}

/// 发 ⌘+某键。事件上只带 ⌘ 这一个修饰键标志，用户手上还按着的 ⌥（刚按完 ⌥V）不会混进去。
fn send_command(key: u16) -> AppResult<()> {
    permissions::ensure_accessibility()?;
    // SAFETY: 事件来源和事件都在本函数内创建并释放。
    unsafe {
        let source = ffi::CGEventSourceCreate(ffi::kCGEventSourceStateCombinedSessionState);
        if !source.is_null() {
            // 合成事件之后的一小段时间里系统默认会压住真实键盘事件；只放行鼠标和系统事件，
            // 免得用户手上的修饰键在这期间搅进来
            ffi::CGEventSourceSetLocalEventsFilterDuringSuppressionState(
                source,
                ffi::kCGEventFilterMaskPermitLocalMouseEvents
                    | ffi::kCGEventFilterMaskPermitSystemDefinedEvents,
                ffi::kCGEventSuppressionStateSuppressionInterval,
            );
        }
        let down = ffi::CGEventCreateKeyboardEvent(source, key, true);
        let up = ffi::CGEventCreateKeyboardEvent(source, key, false);
        let ok = !down.is_null() && !up.is_null();
        if ok {
            ffi::CGEventSetFlags(down, ffi::kCGEventFlagMaskCommand);
            ffi::CGEventSetFlags(up, ffi::kCGEventFlagMaskCommand);
            ffi::CGEventPost(ffi::kCGSessionEventTap, down);
            std::thread::sleep(Duration::from_millis(8));
            ffi::CGEventPost(ffi::kCGSessionEventTap, up);
        }
        for obj in [down, up, source] {
            if !obj.is_null() {
                ffi::CFRelease(obj.cast_const());
            }
        }
        if !ok {
            return Err(AppError::msg("创建按键事件失败"));
        }
    }
    Ok(())
}

pub fn send_paste() -> AppResult<()> {
    send_command(ffi::kVK_ANSI_V)
}

pub fn send_copy() -> AppResult<()> {
    send_command(ffi::kVK_ANSI_C)
}
