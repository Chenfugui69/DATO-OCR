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
use crate::platform::{self, WindowHandle};
use crate::state::state;
use crate::{events, wm};

pub const LABEL: &str = "clipboard";
pub const BOTTOM_HEIGHT: f64 = 320.0;
pub const VERTICAL_SIZE: (f64, f64) = (380.0, 620.0);

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
    let built = wm::builder(app, LABEL)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .inner_size(1200.0, BOTTOM_HEIGHT)
        .build();
    match built {
        Ok(window) => platform::set_exclude_from_capture(&window, true),
        Err(err) => tracing::warn!("创建剪贴板面板失败：{err}"),
    }
}

pub fn toggle(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let Some(window) = app.get_webview_window(LABEL) else {
            prewarm(&app);
            return;
        };
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
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

    let style = state(app).settings.read().clipboard.panel_style.clone();
    if let Some(monitor) = wm::monitor_under_cursor() {
        if style == "vertical" {
            wm::place_on_monitor(
                &window,
                &monitor,
                VERTICAL_SIZE.0,
                VERTICAL_SIZE.1,
                wm::Anchor::Cursor,
            )?;
        } else {
            let width = f64::from(monitor.work_area.width) / monitor.scale_factor.max(0.5);
            wm::place_on_monitor(
                &window,
                &monitor,
                width,
                BOTTOM_HEIGHT,
                wm::Anchor::BottomFull,
            )?;
        }
    }
    let _ = app.emit_to(LABEL, events::CLIPBOARD_PANEL_SHOW, PanelShow { style });
    window.show()?;
    window.set_focus()?;
    platform::reveal_for_tests(&window, true);
    Ok(())
}

pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
        platform::reveal_for_tests(&window, false);
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
