//! macOS 实现（第一版留桩）。
//!
//! 每个函数都写明了移植时用什么 API —— 这是给未来移植者的地图。
//! 签名必须与 `windows/` 完全一致，上层代码零改动。
//!
//! ⚠ macOS 的权限申请（屏幕录制、辅助功能）是移植时最大的工作量，不是 API 本身。

#![allow(unused_variables)]

use crate::error::{AppError, AppResult};

fn todo(api: &str) -> AppError {
    AppError::msg(format!("macOS 尚未实现：{api}"))
}

pub fn init_process() {}

pub mod capture {
    use super::*;
    use crate::platform::{MonitorId, MonitorInfo};
    use image::RgbaImage;

    /// NSScreen.screens + CGDisplayBounds，backingScaleFactor 即 scale_factor
    pub fn list_monitors() -> AppResult<Vec<MonitorInfo>> {
        Err(todo("NSScreen"))
    }
    /// ScreenCaptureKit（12.3+）SCScreenshotManager，需「屏幕录制」权限
    pub fn capture_all() -> AppResult<Vec<(MonitorInfo, RgbaImage)>> {
        Err(todo("ScreenCaptureKit"))
    }
    pub fn capture_monitor(id: MonitorId) -> AppResult<RgbaImage> {
        Err(todo("ScreenCaptureKit"))
    }
    pub fn warm_up() {}
}

pub mod system_events {
    use crate::platform::SystemEvent;
    /// NSApplicationDidChangeScreenParametersNotification / NSWorkspaceDidWakeNotification
    pub fn install(handler: fn(SystemEvent)) {}
}

pub mod window_enum {
    use super::*;
    use crate::platform::{AppInfo, PhysicalRect, WindowHandle, WindowInfo};

    /// CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly)
    pub fn enumerate_top_level() -> AppResult<Vec<WindowInfo>> {
        Ok(Vec::new())
    }
    /// Accessibility API（AXUIElement），需「辅助功能」权限
    pub fn enumerate_children(window: WindowHandle) -> AppResult<Vec<PhysicalRect>> {
        Ok(Vec::new())
    }
    /// CGWindowListCopyWindowInfo 按 Z 序找第一个包含该点的窗口
    pub fn window_at(x: i32, y: i32) -> Option<WindowHandle> {
        None
    }
    /// NSWorkspace.frontmostApplication
    /// NSRunningApplication.processIdentifier 对比 getpid()
    pub fn is_own_window(window: WindowHandle) -> bool {
        false
    }
    pub fn foreground_window() -> Option<WindowHandle> {
        None
    }
    pub fn app_info_of(window: WindowHandle) -> Option<AppInfo> {
        None
    }
}

pub mod input {
    use super::*;
    use crate::platform::WindowHandle;

    /// NSRunningApplication.activate
    pub fn focus_window(window: WindowHandle) -> AppResult<()> {
        Err(todo("NSRunningApplication.activate"))
    }
    /// NSEvent.mouseLocation（注意 macOS 坐标原点在左下角）
    pub fn cursor_position() -> Option<(i32, i32)> {
        None
    }
    /// CGEventCreateKeyboardEvent(kVK_ANSI_V, cmd)，需「辅助功能」权限
    pub fn send_paste() -> AppResult<()> {
        Err(todo("CGEvent"))
    }
    pub fn send_copy() -> AppResult<()> {
        Err(todo("CGEvent"))
    }
}

pub mod effects {
    use super::*;
    use tauri::WebviewWindow;

    /// ns_window() 指针
    pub fn native_handle(window: &WebviewWindow) -> AppResult<u64> {
        Err(todo("ns_window"))
    }
    /// NSWindow.sharingType = .none
    pub fn set_exclude_from_capture(window: &WebviewWindow, exclude: bool) {}
    pub fn reveal_for_tests(window: &WebviewWindow, visible: bool) {}
    /// NSPanel + .nonactivatingPanel 样式
    pub fn set_no_activate(window: &WebviewWindow) {}
    /// orderOut
    pub fn hide_window(window: &WebviewWindow) {
        let _ = window.hide();
    }
    /// orderFrontRegardless
    pub fn show_without_activate(window: &WebviewWindow) -> AppResult<()> {
        window.show()?;
        Ok(())
    }
}

pub mod backdrop {
    use super::*;
    use crate::platform::{MonitorId, PhysicalRect};
    use image::RgbaImage;

    /// 无边框 NSWindow + CALayer.contents = CGImage，level 比遮罩低一级
    pub fn ensure(monitor: MonitorId) -> AppResult<()> {
        Err(todo("NSWindow backdrop"))
    }
    pub fn load(monitor: MonitorId, image: &RgbaImage, at: PhysicalRect) -> AppResult<()> {
        Err(todo("NSWindow backdrop"))
    }
    pub fn show_below(monitor: MonitorId, overlay: u64) -> AppResult<()> {
        Err(todo("NSWindow backdrop"))
    }
    pub fn hide_all() {}
    pub fn release_all() {}
    pub fn retain(keep: &[MonitorId]) {}
}

pub mod clipboard {
    use super::*;
    use crate::platform::{ClipboardPayload, ClipboardSnapshot};
    use std::sync::mpsc::Sender;

    pub type Backup = ();

    /// NSPasteboard.general.changeCount 200ms 轮询（macOS 没有变化通知）
    pub fn start_listener(tx: Sender<ClipboardSnapshot>) -> AppResult<()> {
        Err(todo("NSPasteboard"))
    }
    pub fn write(payload: &ClipboardPayload) -> AppResult<()> {
        Err(todo("NSPasteboard"))
    }
    pub fn read_text() -> Option<String> {
        None
    }
    pub fn backup() -> Option<Backup> {
        None
    }
    pub fn restore(backup: Backup) {}
    pub fn sequence_number() -> u32 {
        0
    }
}

pub mod hook {
    use super::*;
    use crate::platform::HookEvent;
    use std::sync::mpsc::Sender;

    pub struct Guard;

    /// CGEventTapCreate(kCGHIDEventTap)，需「辅助功能」权限
    pub fn install(tx: Sender<HookEvent>) -> AppResult<Guard> {
        Err(todo("CGEventTap"))
    }
}

pub mod cursor {
    /// NSCursor.currentSystem 的 image + hotSpot，画到截图上
    pub fn draw_cursor(image: &mut image::RgbaImage, origin: (i32, i32)) {}
}

pub mod selection_hook {
    use super::*;
    use crate::platform::{PhysicalRect, SelectionEvent};
    use std::sync::mpsc::Sender;

    pub struct Guard;

    /// NSEvent.addGlobalMonitorForEvents(leftMouseUp)，需「辅助功能」权限；
    /// 选中文字优先用 AXSelectedText 读，读不到再模拟 ⌘C
    pub fn install(tx: Sender<SelectionEvent>) -> AppResult<Guard> {
        Err(todo("NSEvent global monitor"))
    }
    pub fn set_button_rect(rect: Option<PhysicalRect>) {}
}

pub mod secret {
    use super::*;
    /// Keychain Services：SecItemAdd / SecItemCopyMatching
    pub fn protect(plain: &[u8]) -> AppResult<Vec<u8>> {
        Err(todo("Keychain"))
    }
    pub fn unprotect(cipher: &[u8]) -> AppResult<Vec<u8>> {
        Err(todo("Keychain"))
    }
}

pub mod system_info {
    use crate::platform::SystemVisuals;
    use std::path::PathBuf;

    /// NSApp.effectiveAppearance / NSWorkspace.accessibilityDisplayShouldReduceTransparency
    pub fn visuals() -> SystemVisuals {
        SystemVisuals {
            transparency_enabled: true,
            ..Default::default()
        }
    }
    pub fn pictures_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures"))
    }
}

pub mod app_icon {
    use image::RgbaImage;
    use std::path::Path;
    /// NSWorkspace.icon(forFile:)
    pub fn extract(exe: &Path) -> Option<RgbaImage> {
        None
    }
}

pub mod process {
    use std::path::Path;
    use std::process::Command;
    pub fn hidden_command(program: &Path) -> Command {
        Command::new(program)
    }
    /// macOS 没有作业对象：子进程里轮询 getppid() 或用 kqueue 监听父进程 NOTE_EXIT
    pub fn tie_to_current_process(child: &std::process::Child) {}
}

pub mod ocr {
    use super::*;
    use crate::platform::SysOcrLine;
    use image::RgbaImage;

    /// Vision.framework VNRecognizeTextRequest
    pub fn available() -> bool {
        false
    }
    pub fn recognize(image: &RgbaImage) -> AppResult<Vec<SysOcrLine>> {
        Err(todo("Vision"))
    }
}
