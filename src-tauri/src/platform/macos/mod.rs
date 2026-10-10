//! macOS 实现。函数签名与 `windows/` 一一对应，上层零改动。
//!
//! 和 Windows 最大的不同是**权限**：抓屏要「屏幕录制」，模拟按键（粘贴、划词取字）和全局
//! 吞键要「辅助功能」，都得用户在系统设置里手动打开，见 `permissions`。
//! 另一个不同是**坐标**：系统给的是逻辑"点"，这一层负责和上层的物理像素互相换算，见 `geometry`。

pub mod app_icon;
pub mod backdrop;
pub mod capture;
pub mod clipboard;
pub mod cursor;
pub mod effects;
pub mod hook;
pub mod input;
pub mod messages;
pub mod ocr;
pub mod permissions;
pub mod process;
pub mod secret;
pub mod selection_hook;
pub mod system_events;
pub mod system_info;
pub mod window_enum;

mod ffi;
mod geometry;
mod util;

use super::types::{DefaultHotkeys, Permission, Permissions};

/// Mac 笔记本的 F1–F3 默认是亮度和调度中心，得按住 fn 才是功能键；
/// 换成 ⌥1 / ⌥2 / ⌥3，位置和 Windows 的 F1 / F2 / F3 对应。
pub const DEFAULT_HOTKEYS: DefaultHotkeys = DefaultHotkeys {
    capture: "Alt+1",
    longshot: "Alt+2",
    ocr: "Alt+3",
    clipboard: "Alt+V",
    translate: "Ctrl+Alt+T",
    instant: "Alt+Shift+1",
};
pub const SYSTEM_OCR_NAME: &str = "Apple Vision";
/// 可下载的 PaddleOCR 引擎（PaddleOCR-json）只有 Windows 版
pub const PADDLE_OCR_SUPPORTED: bool = false;
/// 菜单栏图标的习惯是左键就弹菜单
pub const TRAY_MENU_ON_LEFT_CLICK: bool = true;
pub const TRAY_ICON_IS_TEMPLATE: bool = true;

/// `Ctrl+Alt+T` → `⌃⌥T`
pub fn accelerator_symbols(accelerator: &str) -> String {
    let mut modifiers = [false; 4];
    let mut key = String::new();
    for part in accelerator.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers[0] = true,
            "alt" | "option" => modifiers[1] = true,
            "shift" => modifiers[2] = true,
            "super" | "cmd" | "command" | "meta" | "cmdorctrl" | "commandorcontrol" => {
                modifiers[3] = true;
            }
            _ => key = part.to_string(),
        }
    }
    let mut out = String::new();
    for (on, symbol) in modifiers.iter().zip(["⌃", "⌥", "⇧", "⌘"]) {
        if *on {
            out.push_str(symbol);
        }
    }
    out.push_str(&key);
    out
}

/// macOS 的菜单不认 `\t`，快捷键用符号写在文字后面。
pub fn menu_label(text: &str, accelerator: &str) -> String {
    if accelerator.is_empty() {
        text.to_string()
    } else {
        format!("{text}　{}", accelerator_symbols(accelerator))
    }
}

pub fn is_reopen_event(event: &tauri::RunEvent) -> bool {
    matches!(event, tauri::RunEvent::Reopen { .. })
}

pub fn permissions() -> Permissions {
    Permissions {
        screen_capture: Some(permissions::screen_capture_granted()),
        accessibility: Some(permissions::accessibility_granted()),
        full_disk_access: Some(messages::full_disk_access_granted()),
    }
}

pub fn sms_supported() -> bool {
    true
}

pub fn start_sms_watcher(
    tx: std::sync::mpsc::Sender<super::types::SmsMessage>,
    enabled: Box<dyn Fn() -> bool + Send>,
) {
    messages::start(tx, enabled);
}

pub fn request_permission(which: Permission) {
    permissions::request(which);
}

/// 启动前在前台的那个应用（进程号）。
static LAUNCHED_OVER: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

pub fn init_process() {
    // 显示器的可用区域和名字只能在主线程读，趁现在（进程入口，主线程）先记一份
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        geometry::refresh_extras(mtm);
    }
    if let Some(front) = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication() {
        LAUNCHED_OVER.store(
            front.processIdentifier(),
            std::sync::atomic::Ordering::SeqCst,
        );
    }
}

/// 静默启动（开机自启）时不显示任何窗口，但 macOS 照样会把新启动的应用切到前台，
/// 用户正在用的程序就丢了焦点。把前台还给启动前的那个应用。
pub fn after_silent_start() {
    let pid = LAUNCHED_OVER.load(std::sync::atomic::Ordering::SeqCst);
    if pid == 0 || pid == std::process::id() as i32 {
        return;
    }
    if let Some(app) = window_enum::running_app(pid) {
        #[allow(deprecated)]
        app.activateWithOptions(
            objc2_app_kit::NSApplicationActivationOptions::ActivateIgnoringOtherApps,
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn accelerator_symbols_follow_mac_order() {
        assert_eq!(super::accelerator_symbols("Alt+1"), "⌥1");
        assert_eq!(super::accelerator_symbols("Ctrl+Alt+T"), "⌃⌥T");
        assert_eq!(super::accelerator_symbols("Super+Shift+S"), "⇧⌘S");
        assert_eq!(super::accelerator_symbols("F1"), "F1");
    }
}
