//! 剪贴板面板窗口（Alt+V）与"选中即粘贴"流程（规格 05 §3）。
//!
//! 面板**常驻但隐藏**，打开只是摆位 + show + 通知前端刷新，做到瞬间响应。
//!
//! 粘贴流程：面板打开**之前**记下前台窗口 → 用户选中 → 写剪贴板（不进历史，
//! 让该条冒泡）→ 藏面板 → 焦点还给记下的窗口 → 等焦点稳定 → 模拟 Ctrl+V。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppResult;
use crate::platform::{self, FloatingKind, PhysicalRect, WindowHandle};
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
/// 这次是滑进来的：（停下的位置，屏幕底边下面的起点）。收起时照原路滑回去
static SLID: Mutex<Option<(PhysicalRect, PhysicalRect)>> = Mutex::new(None);
/// 每显示一次加一，滑出动画结束时用来判断中途有没有又被叫出来
static SHOW_GEN: AtomicU64 = AtomicU64::new(0);
const SLIDE_IN_MS: u32 = 280;
const SLIDE_OUT_MS: u32 = 200;

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
            dismiss(&app);
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
    let mut slide = None;
    // 盖在程序坞上面：底边按整块屏算，不按扣掉程序坞的可用区域；窗口层级也要提到程序坞之上
    let over_dock = style != "vertical" && cb.panel_over_dock;
    platform::set_above_dock(&window, over_dock);
    if let Some(mut monitor) = wm::monitor_under_cursor() {
        if over_dock {
            let bottom = monitor.bounds.bottom();
            monitor.work_area.height = (bottom - monitor.work_area.y).max(1) as u32;
        }
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
            let rect = match (cb.panel_docked, blur) {
                // 贴边：紧贴工作区底边、整屏宽；不开毛玻璃时页面只在上面留阴影
                (true, true) => wm::rect_on_monitor(
                    &monitor,
                    width,
                    BOTTOM_HEIGHT - BOTTOM_PAD,
                    wm::Anchor::BottomFull,
                ),
                // 悬浮 + 毛玻璃：窗口本身四周缩进，和不开毛玻璃时的卡片位置一样
                (false, true) => {
                    let wa = monitor.work_area;
                    let pad = (BOTTOM_PAD * s).round() as i32;
                    let w = ((width - BOTTOM_PAD * 2.0) * s).round() as i32;
                    let h = ((BOTTOM_HEIGHT - BOTTOM_PAD * 2.0) * s).round() as i32;
                    PhysicalRect::new(
                        wa.x + pad,
                        wa.bottom() - pad - h,
                        w.max(1) as u32,
                        h.max(1) as u32,
                    )
                }
                _ => wm::rect_on_monitor(&monitor, width, BOTTOM_HEIGHT, wm::Anchor::BottomFull),
            };
            if cb.panel_animation && platform::slides_windows() {
                // 先摆在屏幕底边下面，显示出来之后再滑上去
                let start = below(rect, &monitor);
                platform::set_bounds(&window, start.x, start.y, start.width, start.height)?;
                slide = Some((rect, start));
            } else {
                platform::place_window(&window, rect.x, rect.y, rect.width, rect.height)?;
            }
        }
    }
    SHOW_GEN.fetch_add(1, Ordering::SeqCst);
    *SLID.lock() = slide;
    let _ = app.emit_to(LABEL, events::CLIPBOARD_PANEL_SHOW, PanelShow { style });
    window.show()?;
    platform::take_focus(&window)?;
    // 面板开着的时候允许被截图（用户就是想截它）；藏起来时再排除 —— 不排除的隐藏窗口会被
    // WGC 画成一块带标题栏的白块
    platform::set_exclude_from_capture(&window, false);
    if let Some((rect, _)) = slide {
        platform::slide_window(&window, rect, SLIDE_IN_MS);
    }
    Ok(())
}

/// 面板滑入前 / 滑出后待的位置：正好在屏幕底边下面。
fn below(rect: PhysicalRect, monitor: &platform::MonitorInfo) -> PhysicalRect {
    PhysicalRect::new(rect.x, monitor.bounds.bottom(), rect.width, rect.height)
}

/// 用户收起面板（点外面、Esc、再按一次热键）：平台自己滑窗口的话先滑下去再藏。
pub fn dismiss(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let Some((_, start)) = SLID.lock().take() else {
        hide(app);
        return;
    };
    platform::slide_window(&window, start, SLIDE_OUT_MS);
    let gen = SHOW_GEN.load(Ordering::SeqCst);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(u64::from(SLIDE_OUT_MS)));
        let ui = app.clone();
        let _ = app.run_on_main_thread(move || {
            // 滑到一半又被叫出来了：那一次已经重新摆好位置，不要藏
            if SHOW_GEN.load(Ordering::SeqCst) == gen {
                hide(&ui);
            }
        });
    });
}

pub fn hide(app: &AppHandle) {
    *SLID.lock() = None;
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
