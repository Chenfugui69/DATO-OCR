//! 只靠 Tauri 接口就能做的默认实现。
//!
//! Windows 直接用这些；macOS 上同名的函数换成了原生实现（`macos/effects.rs` 等）。
//! 这个文件在两个平台上都参与编译，改它的时候两边都会过一遍类型检查。

#![cfg_attr(not(windows), allow(dead_code))]

use tauri::utils::config::WindowEffectsConfig;
use tauri::window::{Effect, EffectsBuilder};
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow,
    WebviewWindowBuilder,
};

use super::types::{FloatingKind, Permission, Permissions};
use crate::error::AppResult;

/// Windows 上置顶窗口本来就能浮在别的程序上面、收键盘也不用特殊处理，照常建就行。
pub fn build_floating(
    builder: WebviewWindowBuilder<'_, tauri::Wry, AppHandle>,
    _kind: FloatingKind,
) -> AppResult<WebviewWindow> {
    Ok(builder.build()?)
}

pub fn place_window(
    window: &WebviewWindow,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> AppResult<()> {
    window.set_position(PhysicalPosition::new(x, y))?;
    window.set_size(PhysicalSize::new(width, height))?;
    // 先设尺寸再设位置偶尔会被 DWM 挪回去，再确认一次
    window.set_position(PhysicalPosition::new(x, y))?;
    Ok(())
}

/// Windows 上拿着键盘的窗口就是系统的前台窗口。
pub fn is_foreground(window: &WebviewWindow) -> bool {
    super::native_handle(window)
        .is_ok_and(|own| super::foreground_window().is_some_and(|h| h.0 == own))
}

/// 应用里选了浅色 / 深色时，窗口的系统材质（Mica）也得是那个外观：材质的深浅是按窗口的主题画的，
/// 不同步的话系统是深色、应用选了浅色，侧边栏背后还是深色的 Mica，上面压着浅色主题的深色字，看不清
/// （和 macOS 上是同一个问题）。选"跟随系统"就交还给系统。
pub fn apply_theme(app: &AppHandle, theme: &str) {
    app.set_theme(match theme {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => None,
    });
}

/// 盖住任务栏时面板贴到屏幕最底下，不留缝：任务栏最底下那一排是"程序开着"的小点，留一条缝的话
/// 它们正好从缝里露出来（用户报过）。
pub const OVER_DOCK_GAP: f64 = 0.0;

/// Windows 的置顶窗口本来就在任务栏上面。
pub fn set_above_dock(_window: &WebviewWindow, _above: bool) {}

pub fn take_focus(window: &WebviewWindow) -> AppResult<()> {
    window.set_focus()?;
    Ok(())
}

/// Windows 上所有窗口都是无边框的，标题栏由页面自己画。
pub fn frame_window<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    builder
}

pub fn window_effects(glass: bool) -> Option<WindowEffectsConfig> {
    glass.then(|| EffectsBuilder::new().effect(Effect::Mica).build())
}

/// 菜单项文字后面带上快捷键。`\t` 之后的部分 Windows 会靠右对齐显示。
pub fn menu_label(text: &str, accelerator: &str) -> String {
    if accelerator.is_empty() {
        text.to_string()
    } else {
        format!("{text}\t{accelerator}")
    }
}

pub fn after_silent_start() {}

pub fn is_reopen_event(_event: &tauri::RunEvent) -> bool {
    false
}

pub fn permissions() -> Permissions {
    Permissions::default()
}

pub fn request_permission(_which: Permission) {}
