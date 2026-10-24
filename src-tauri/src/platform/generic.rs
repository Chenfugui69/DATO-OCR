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

/// Windows 上藏起来的遮罩不按整屏留画面缓冲，直接藏就行。
pub fn park_overlay(window: &WebviewWindow) {
    let _ = window.hide();
}

pub fn unpark_overlay(_window: &WebviewWindow) {}

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

/// Windows 上 `window.screenX` / `screen.avail*` 是准的，页面自己读。
pub fn window_placement(_window: &WebviewWindow) -> Option<super::types::WindowPlacement> {
    None
}

/// Windows 上用 Tauri 自带的 `startResizeDragging`。
pub fn start_resize_drag(
    _window: &WebviewWindow,
    _direction: &str,
    _min: (f64, f64),
    _top_inset: f64,
) -> bool {
    false
}

/// Windows 上面板靠失焦收起，用不到这个；说"在"最保险（不会误收）。
pub fn cursor_in_window(_window: &WebviewWindow) -> bool {
    true
}

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

/// 录视频的写入器：Windows 还没做（下一步用 Media Foundation），先报不支持。
pub struct VideoWriter;

impl VideoWriter {
    pub fn create(
        _path: &std::path::Path,
        _width: u32,
        _height: u32,
        _fps: u32,
        _options: super::types::VideoOptions,
    ) -> AppResult<Self> {
        Err(crate::error::AppError::msg("这个系统上暂时还不能录视频"))
    }

    pub fn start(&mut self, _origin: std::time::Instant) {}

    pub fn warnings(&self) -> &[String] {
        &[]
    }

    pub fn push(&mut self, _frame: &image::RgbaImage, _ms: u64) -> AppResult<()> {
        Ok(())
    }

    pub fn finish(self, _end_ms: u64) -> AppResult<usize> {
        Ok(0)
    }
}

/// 剪辑 / 混音录好的视频：没有视频编码的平台不支持
pub fn export_video(
    _src: &std::path::Path,
    _dst: &std::path::Path,
    _edit: super::types::VideoEdit,
    _encoding: super::types::VideoEncoding,
    _mix: bool,
) -> AppResult<()> {
    Err(crate::error::AppError::msg("这个系统上暂时还不能剪辑视频"))
}

/// 朗读：没做系统语音的平台报不支持，页面改用浏览器自带的语音合成
pub fn speak(_text: &str, _lang: &str, _options: &super::types::SpeakOptions) -> AppResult<()> {
    Err(crate::error::AppError::msg("unsupported"))
}

pub fn stop_speaking() {}

pub fn is_speaking() -> bool {
    false
}

/// 系统的声音列表：没做的平台给空，设置页改用浏览器的语音列表
pub fn voices() -> Vec<super::types::Voice> {
    Vec::new()
}

/// 鼠标下面的界面元素（由里到外）：还没做，截图照旧按窗口 / 子窗口框选
pub fn elements_at(
    _window: super::types::WindowHandle,
    _x: i32,
    _y: i32,
) -> AppResult<Vec<super::types::UiElement>> {
    Ok(Vec::new())
}

pub fn elements_near(
    _window: super::types::WindowHandle,
    _x: i32,
    _y: i32,
) -> AppResult<Vec<super::types::UiElement>> {
    Ok(Vec::new())
}

/// 摄像头画面：还没做
pub struct Camera;

impl Camera {
    pub fn start(_size: u32) -> AppResult<Self> {
        Err(crate::error::AppError::msg("这个系统上暂时还不能录摄像头"))
    }

    pub fn latest(&self) -> Option<image::RgbaImage> {
        None
    }
}

pub fn media_access(_kind: super::types::MediaKind) -> super::types::MediaAccess {
    super::types::MediaAccess::Unsupported
}

pub fn request_media_access(kind: super::types::MediaKind) -> super::types::MediaAccess {
    media_access(kind)
}

/// 纯 Rust 的二维码识别（rqrr），只认二维码。macOS 在 Vision 没认出来时也拿它再试一遍。
pub fn scan_codes(image: &image::RgbaImage) -> AppResult<Vec<super::types::ScannedCode>> {
    let (w, h) = image.dimensions();
    let mut prepared =
        rqrr::PreparedImage::prepare_from_greyscale(w as usize, h as usize, |x, y| {
            let p = image.get_pixel(x as u32, y as u32).0;
            // 和常见的灰度公式一致（整数版，免得每个像素都做浮点）
            ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u8
        });
    let mut out = Vec::new();
    for grid in prepared.detect_grids() {
        let Ok((_, text)) = grid.decode() else {
            continue;
        };
        let xs = grid.bounds.map(|p| f64::from(p.x));
        let ys = grid.bounds.map(|p| f64::from(p.y));
        let x = xs.iter().copied().fold(f64::INFINITY, f64::min);
        let y = ys.iter().copied().fold(f64::INFINITY, f64::min);
        out.push(super::types::ScannedCode {
            kind: "QR".into(),
            text,
            x,
            y,
            width: xs.iter().copied().fold(f64::NEG_INFINITY, f64::max) - x,
            height: ys.iter().copied().fold(f64::NEG_INFINITY, f64::max) - y,
        });
    }
    Ok(out)
}

pub fn is_reopen_event(_event: &tauri::RunEvent) -> bool {
    false
}

pub fn permissions() -> Permissions {
    Permissions::default()
}

pub fn request_permission(_which: Permission) {}

/// 这个平台读不到手机短信。
pub fn sms_supported() -> bool {
    false
}

pub fn start_sms_watcher(
    _tx: std::sync::mpsc::Sender<super::types::SmsMessage>,
    _enabled: Box<dyn Fn() -> bool + Send>,
) {
}

#[cfg(test)]
mod tests {
    /// 真机烟雾测试（Windows 上二维码只靠它）：
    /// `CHENOCR_SMOKE_QR=二维码图片 cargo test -- --ignored smoke_scan_codes --nocapture`
    #[test]
    #[ignore]
    fn smoke_scan_codes() {
        let path = std::env::var("CHENOCR_SMOKE_QR").unwrap();
        let image = image::open(&path).unwrap().to_rgba8();
        let codes = super::scan_codes(&image).unwrap();
        println!("{codes:?}");
        assert!(!codes.is_empty());
    }
}

/// 录屏时的键盘、滚轮监听：没做的平台什么都不报
pub mod record_input {
    pub struct Guard;

    pub fn install(
        _tx: std::sync::mpsc::Sender<crate::platform::types::RecordInput>,
    ) -> crate::error::AppResult<Guard> {
        Ok(Guard)
    }

    /// 现在按着哪几个鼠标键（没做的平台一直是没按）
    pub fn mouse_buttons() -> u8 {
        0
    }
}
