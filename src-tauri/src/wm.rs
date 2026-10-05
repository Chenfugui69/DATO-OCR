//! 窗口管理：主窗口、toast、通用建窗工具、系统材质。
//!
//! 所有窗口共用一个 `index.html`，前端按窗口标签决定渲染哪个视图
//! （`src/main.tsx`），所以这里建窗只需要给标签。

use serde::Serialize;
use tauri::window::{Effect, EffectsBuilder};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::error::AppResult;
use crate::events;
use crate::platform::{self, MonitorInfo};
use crate::state::state;

pub const MAIN: &str = "main";
pub const TOAST: &str = "toast";

/// 新建一个无边框窗口的通用构建器（默认隐藏）。
pub fn builder<'a>(
    app: &'a AppHandle,
    label: &'a str,
) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title("DATO COR")
        .decorations(false)
        .visible(false)
}

pub fn show_main(app: &AppHandle, page: Option<&str>) {
    let Some(window) = app.get_webview_window(MAIN) else {
        tracing::error!("主窗口不存在");
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    if let Some(page) = page {
        let _ = app.emit_to(MAIN, events::NAVIGATE, page);
    }
}

/// 鼠标所在的显示器（取不到就主屏）。
pub fn monitor_under_cursor() -> Option<MonitorInfo> {
    let monitors = platform::list_monitors().ok()?;
    let cursor = platform::cursor_position();
    cursor
        .and_then(|(x, y)| {
            monitors
                .iter()
                .find(|m| m.bounds.contains_point(x, y))
                .cloned()
        })
        .or_else(|| monitors.iter().find(|m| m.is_primary).cloned())
        .or_else(|| monitors.into_iter().next())
}

/// 包含某个屏幕点的显示器（取不到就主屏）。
pub fn monitor_at(x: i32, y: i32) -> Option<MonitorInfo> {
    let monitors = platform::list_monitors().ok()?;
    monitors
        .iter()
        .find(|m| m.bounds.contains_point(x, y))
        .cloned()
        .or_else(|| monitors.iter().find(|m| m.is_primary).cloned())
        .or_else(|| monitors.into_iter().next())
}

/// 把窗口放到某块屏上，`(w, h)` 是逻辑尺寸，`anchor` 决定对齐方式。
pub fn place_on_monitor(
    window: &WebviewWindow,
    monitor: &MonitorInfo,
    w: f64,
    h: f64,
    anchor: Anchor,
) -> AppResult<()> {
    let s = monitor.scale_factor.max(0.5);
    let (pw, ph) = ((w * s).round() as i32, (h * s).round() as i32);
    let wa = monitor.work_area;
    let margin = (20.0 * s).round() as i32;
    let (x, y) = match anchor {
        Anchor::Center => (
            wa.x + (wa.width as i32 - pw) / 2,
            wa.y + (wa.height as i32 - ph) / 2,
        ),
        Anchor::BottomRight => (wa.right() - pw - margin, wa.bottom() - ph - margin),
        Anchor::BottomFull => (wa.x, wa.bottom() - ph),
        Anchor::Cursor | Anchor::Point(..) => {
            let (cx, cy) = match anchor {
                Anchor::Point(x, y) => (x, y),
                _ => platform::cursor_position().unwrap_or((wa.x, wa.y)),
            };
            let mut x = cx + (12.0 * s) as i32;
            let mut y = cy + (16.0 * s) as i32;
            if x + pw > wa.right() {
                x = (cx - pw - (12.0 * s) as i32).max(wa.x);
            }
            if y + ph > wa.bottom() {
                y = (cy - ph - (12.0 * s) as i32).max(wa.y);
            }
            (x, y)
        }
    };
    window.set_size(PhysicalSize::new(pw.max(1) as u32, ph.max(1) as u32))?;
    window.set_position(PhysicalPosition::new(x, y))?;
    Ok(())
}

#[derive(Clone, Copy)]
pub enum Anchor {
    Center,
    BottomRight,
    BottomFull,
    Cursor,
    /// 和 Cursor 一样贴着一个点放（右下方，放不下就翻到左 / 上），点由调用方给
    Point(i32, i32),
}

// ───────────────────────── 系统材质 ─────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualCapabilities {
    /// 主窗口是否启用了系统材质（Mica）。false 时前端必须画不透明底色。
    pub glass: bool,
    pub backdrop: &'static str,
    pub dark_mode: bool,
    pub reduced_motion: bool,
    pub transparency_enabled: bool,
    pub power_saver: bool,
}

pub fn visual_capabilities(app: &AppHandle) -> VisualCapabilities {
    let st = state(app);
    let v = *st.visuals.read();
    let pref = st.settings.read().appearance.glass_effect.clone();
    let glass = match pref.as_str() {
        "off" => false,
        "on" => v.mica_supported,
        _ => v.mica_supported && v.transparency_enabled && !v.power_saver,
    };
    VisualCapabilities {
        glass,
        backdrop: if glass { "mica" } else { "none" },
        dark_mode: v.dark_mode,
        reduced_motion: v.reduced_motion,
        transparency_enabled: v.transparency_enabled,
        power_saver: v.power_saver,
    }
}

/// 按当前能力给窗口加/去 Mica（主窗口、识字窗口这类"正经窗口"）。
pub fn apply_window_effects(app: &AppHandle, window: &WebviewWindow) {
    let caps = visual_capabilities(app);
    let result = if caps.glass {
        window.set_effects(EffectsBuilder::new().effect(Effect::Mica).build())
    } else {
        window.set_effects(None)
    };
    if let Err(err) = result {
        tracing::warn!(label = window.label(), "设置窗口材质失败：{err}");
    }
}

/// 毛玻璃面板的圆角半径（逻辑像素），按窗口标签存；窗口大小一变就按它重新裁。
static GLASS: parking_lot::Mutex<Option<std::collections::HashMap<String, f64>>> =
    parking_lot::Mutex::new(None);

/// 浮动面板的材质：
/// - 毛玻璃：系统亚克力（背后内容实时模糊）。系统圆角只有 8px 一档，所以关掉系统圆角、自己把窗口
///   裁成用户要的半径（`radius`，0 = 直角）。裁出来的边没有抗锯齿，页面在边上画一圈细线盖一盖
/// - 不开毛玻璃：窗口全透明，圆角和阴影由页面自己画（四周留了透明边）
///
/// 实测（Win11 24H2）不能用 Blur（老的 ACCENT_ENABLE_BLURBEHIND，配 WebView2 背后是黑的），
/// 也不能开系统阴影（会把窗框延伸进来、窗口变大）。亚克力自带色调，页面色调要淡；窗口失去焦点时
/// 系统会把亚克力换成纯色，这是系统行为。只能在 UI 线程调用。
pub fn set_glass(window: &WebviewWindow, blur: bool, radius: f64) {
    let effects = blur.then(|| EffectsBuilder::new().effect(Effect::Acrylic).build());
    if let Err(err) = window.set_effects(effects) {
        tracing::warn!(label = window.label(), "设置面板材质失败：{err}");
    }
    platform::set_rounded(window, false);
    let r = if blur { radius.max(0.0) } else { 0.0 };
    GLASS
        .lock()
        .get_or_insert_with(Default::default)
        .insert(window.label().to_string(), r);
    apply_region(window, r);
}

fn apply_region(window: &WebviewWindow, radius: f64) {
    let s = window.scale_factor().unwrap_or(1.0);
    platform::set_round_region(window, (radius * s).round() as u32);
}

/// 面板窗口创建时调一次：窗口大小、缩放一变就按记下的半径重新裁圆角。
pub fn track_glass(window: &WebviewWindow) {
    let w = window.clone();
    window.on_window_event(move |event| {
        if matches!(
            event,
            tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. }
        ) {
            // 先把半径取出来再裁：裁的时候系统可能同步发回窗口消息
            let r = GLASS
                .lock()
                .as_ref()
                .and_then(|m| m.get(w.label()).copied())
                .unwrap_or(0.0);
            if r > 0.0 {
                apply_region(&w, r);
            }
        }
    });
}

/// 带系统材质的窗口标签。
const GLASS_WINDOWS: &[&str] = &[MAIN, "ocr", "editor"];

pub fn refresh_visuals(app: &AppHandle) {
    *state(app).visuals.write() = platform::system_visuals();
    for label in GLASS_WINDOWS {
        if let Some(window) = app.get_webview_window(label) {
            apply_window_effects(app, &window);
        }
    }
    let _ = app.emit(events::VISUALS_CHANGED, visual_capabilities(app));
}

// ───────────────────────── Toast ─────────────────────────

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToastPayload {
    pub kind: &'static str,
    pub message: String,
}

pub fn prewarm_toast(app: &AppHandle) {
    if app.get_webview_window(TOAST).is_some() {
        return;
    }
    let built = builder(app, TOAST)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .inner_size(360.0, 220.0)
        .build();
    match built {
        Ok(window) => {
            let _ = window.set_ignore_cursor_events(true);
            platform::set_exclude_from_capture(&window, true);
        }
        Err(err) => tracing::warn!("创建 toast 窗口失败：{err}"),
    }
}

/// 屏幕右下角的轻提示（规格 06 §4.7）。不是系统通知：系统通知有延迟还会进通知中心。
pub fn toast(app: &AppHandle, kind: &'static str, message: impl Into<String>) {
    let message = message.into();
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let Some(window) = app.get_webview_window(TOAST) else {
            return;
        };
        if let Some(monitor) = monitor_under_cursor() {
            let _ = place_on_monitor(&window, &monitor, 360.0, 220.0, Anchor::BottomRight);
        }
        let _ = app.emit_to(TOAST, events::TOAST, ToastPayload { kind, message });
        if let Err(err) = platform::show_without_activate(&window) {
            tracing::debug!("显示 toast 失败：{err}");
        }
        platform::reveal_for_tests(&window, true);
    });
}
