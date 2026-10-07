//! 剪贴板面板窗口（Alt+V）与"选中即粘贴"流程（规格 05 §3）。
//!
//! 面板**常驻但隐藏**，打开只是摆位 + show + 通知前端刷新，做到瞬间响应。
//!
//! 粘贴流程：面板打开**之前**记下前台窗口 → 用户选中 → 写剪贴板（不进历史，
//! 让该条冒泡）→ 藏面板 → 焦点还给记下的窗口 → 等焦点稳定 → 模拟 Ctrl+V。

use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppResult;
use crate::platform::{self, FloatingKind, WindowHandle};
use crate::state::state;
use crate::{events, wm};

pub const LABEL: &str = "clipboard";
pub const BOTTOM_HEIGHT: f64 = 346.0;
pub const VERTICAL_SIZE: (f64, f64) = (380.0, 620.0);
/// 页面里面板四周留给阴影的透明边（和 panel.css 一致）：底部样式 8，竖版 6
const BOTTOM_PAD: f64 = 8.0;
const VERTICAL_PAD: f64 = 6.0;
/// 悬浮样式的圆角（和 panel.css 的 --cn-radius-2xl 一致）
const RADIUS: f64 = 16.0;

static PREVIOUS: Mutex<Option<WindowHandle>> = Mutex::new(None);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PanelShow {
    style: String,
}

pub fn prewarm(app: &AppHandle) {
    if app.get_webview_window(LABEL).is_some() {
        return;
    }
    let panel = wm::builder(app, LABEL)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .inner_size(1200.0, BOTTOM_HEIGHT);
    match platform::build_floating(panel, FloatingKind::Panel) {
        Ok(window) => {
            platform::set_exclude_from_capture(&window, true);
            wm::track_glass(&window);
            apply_material(app);
        }
        Err(err) => tracing::warn!("创建剪贴板面板失败：{err}"),
    }
}

/// 面板材质：毛玻璃时窗口就是面板本身（不留透明边），悬浮样式裁成圆角，贴边是直角。
/// 只能在 UI 线程调用。
pub fn apply_material(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let cb = state(app).settings.read().clipboard.clone();
    let docked = cb.panel_style != "vertical" && cb.panel_docked;
    wm::set_glass(&window, cb.panel_blur, if docked { 0.0 } else { RADIUS });
}

pub fn toggle(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let Some(window) = app.get_webview_window(LABEL) else {
            prewarm(&app);
            return;
        };
        if window.is_visible().unwrap_or(false) {
            hide(&app);
        } else if let Err(err) = show(&app) {
            tracing::error!("打开剪贴板面板失败：{err}");
        }
    });
}

fn show(app: &AppHandle) -> AppResult<()> {
    let window = app
        .get_webview_window(LABEL)
        .ok_or_else(|| crate::error::AppError::msg("剪贴板面板不存在"))?;
    // 必须在面板显示之前记，之后前台就是面板自己了
    let own = platform::native_handle(&window).ok();
    *PREVIOUS.lock() = platform::foreground_window().filter(|h| Some(h.0) != own);

    let cb = state(app).settings.read().clipboard.clone();
    let style = cb.panel_style.clone();
    if let Some(monitor) = wm::monitor_under_cursor() {
        // 毛玻璃时窗口就是面板（材质铺满整个窗口），页面里那圈透明边要从窗口尺寸里扣掉
        let blur = cb.panel_blur;
        if style == "vertical" {
            let inset = if blur { VERTICAL_PAD * 2.0 } else { 0.0 };
            wm::place_on_monitor(
                &window,
                &monitor,
                VERTICAL_SIZE.0 - inset,
                VERTICAL_SIZE.1 - inset,
                wm::Anchor::Cursor,
            )?;
        } else {
            let s = monitor.scale_factor.max(0.5);
            let width = f64::from(monitor.work_area.width) / s;
            match (cb.panel_docked, blur) {
                // 贴边：紧贴工作区底边、整屏宽；不开毛玻璃时页面只在上面留阴影
                (true, true) => wm::place_on_monitor(
                    &window,
                    &monitor,
                    width,
                    BOTTOM_HEIGHT - BOTTOM_PAD,
                    wm::Anchor::BottomFull,
                )?,
                // 悬浮 + 毛玻璃：窗口本身四周缩进，和不开毛玻璃时的卡片位置一样
                (false, true) => {
                    let wa = monitor.work_area;
                    let pad = (BOTTOM_PAD * s).round() as i32;
                    let w = ((width - BOTTOM_PAD * 2.0) * s).round() as i32;
                    let h = ((BOTTOM_HEIGHT - BOTTOM_PAD * 2.0) * s).round() as i32;
                    platform::place_window(
                        &window,
                        wa.x + pad,
                        wa.bottom() - pad - h,
                        w.max(1) as u32,
                        h.max(1) as u32,
                    )?;
                }
                _ => wm::place_on_monitor(
                    &window,
                    &monitor,
                    width,
                    BOTTOM_HEIGHT,
                    wm::Anchor::BottomFull,
                )?,
            }
        }
    }
    let _ = app.emit_to(LABEL, events::CLIPBOARD_PANEL_SHOW, PanelShow { style });
    window.show()?;
    platform::take_focus(&window)?;
    // 面板开着的时候允许被截图（用户就是想截它）；藏起来时再排除 —— 不排除的隐藏窗口会被
    // WGC 画成一块带标题栏的白块
    platform::set_exclude_from_capture(&window, false);
    Ok(())
}

pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
        platform::set_exclude_from_capture(&window, true);
    }
}

/// 粘贴到面板打开前的那个窗口。
pub fn paste(app: &AppHandle, id: i64, plain: bool) -> AppResult<()> {
    let payload = super::payload_of(app, id, plain)?;
    super::write_own(app, &payload, Some(id))?;
    let target = *PREVIOUS.lock();
    let ui_app = app.clone();
    app.run_on_main_thread(move || hide(&ui_app))?;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        if let Some(target) = target {
            if let Err(err) = platform::focus_window(target) {
                tracing::debug!("还原焦点失败：{err}");
            }
        }
        // 等焦点切换完成，否则 Ctrl+V 会发给还没激活的窗口
        std::thread::sleep(Duration::from_millis(70));
        if let Err(err) = platform::send_paste() {
            tracing::warn!("模拟粘贴失败：{err}");
        }
    });
    Ok(())
}

/// 只复制，不粘贴（Shift+Enter / 右键菜单"复制"）。
pub fn copy(app: &AppHandle, id: i64, plain: bool) -> AppResult<()> {
    let payload = super::payload_of(app, id, plain)?;
    super::write_own(app, &payload, Some(id))
}
