//! 划词翻译（规格 04 §8.2 及其扩展）。三种触发方式：
//!
//! 1. 快捷键 Ctrl+Alt+T：翻译当前选中的文字（始终可用）
//! 2. 选中文字后旁边出现一个小悬浮按钮，点它翻译（设置可关、可改位置）
//! 3. 按住 Alt / Ctrl 选文字，松开鼠标直接弹翻译（设置里选用哪个键，默认关闭）
//!
//! 2、3 靠常驻的全局鼠标钩子（`platform::start_selection_watch`）认出选字手势；两个都关掉
//! 时钩子卸载，不常驻。
//!
//! 读"当前选中的文字"没有通用 API，只能模拟 Ctrl+C 再读剪贴板。这会污染剪贴板，
//! 所以：先整份备份 → 模拟复制 → 读文字 → **原样恢复** → 两次变化都不进历史。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppResult;
use crate::platform::{self, PhysicalRect, SelectionEvent, SelectionWatchGuard};
use crate::state::state;
use crate::{clipboard, events, wm};

pub const WINDOW: &str = "translate";
pub const BUTTON_WINDOW: &str = "selbtn";
/// 悬浮按钮的逻辑尺寸
const BUTTON_SIZE: f64 = 30.0;
/// 没去碰它的话，按钮显示这么久后自己收起
const BUTTON_TTL: Duration = Duration::from_millis(3500);

/// 最近一次要翻译的文字。气泡页面刚加载完时主动来取（事件可能在页面就绪前就发出了）。
static CURRENT: Mutex<Option<String>> = Mutex::new(None);
static WATCH: Mutex<Option<SelectionWatchGuard>> = Mutex::new(None);
/// 悬浮按钮当前的位置（屏幕物理坐标）；None = 没显示
static BUTTON: Mutex<Option<PhysicalRect>> = Mutex::new(None);
/// 每显示一次按钮加一，自动收起的计时器靠它判断按钮是不是已经换了一次
static BUTTON_GEN: AtomicU64 = AtomicU64::new(0);

pub fn current() -> Option<String> {
    CURRENT.lock().clone()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslatePopup {
    pub text: String,
}

/// 快捷键入口：翻译当前选中的文字，气泡出现在鼠标旁边。
pub fn trigger(app: &AppHandle) {
    translate_selection(app, None);
}

/// 取选中文字并弹出翻译气泡。`at` 为气泡左上角附近的点（屏幕物理坐标），None = 鼠标旁边。
fn translate_selection(app: &AppHandle, at: Option<(i32, i32)>) {
    let app = app.clone();
    std::thread::spawn(move || match grab_selection(&app) {
        Ok(Some(text)) => {
            tracing::debug!(chars = text.chars().count(), ?at, "划词取到文字");
            show_popup(&app, text, at);
        }
        Ok(None) => wm::toast(&app, "info", "没有选中文字"),
        Err(err) => wm::toast(&app, "error", format!("读取选中文字失败：{err}")),
    });
}

fn grab_selection(app: &AppHandle) -> AppResult<Option<String>> {
    let backup = platform::clipboard_backup();
    let before = platform::clipboard_sequence();
    clipboard::begin_ignore(app);
    platform::send_copy()?;

    let deadline = Instant::now() + Duration::from_millis(500);
    let mut changed = false;
    while Instant::now() < deadline {
        if platform::clipboard_sequence() != before {
            changed = true;
            // 有的程序分几步写剪贴板，稍等它写完
            std::thread::sleep(Duration::from_millis(40));
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    let text = if changed {
        platform::clipboard_read_text()
    } else {
        None
    };
    clipboard::end_ignore(app);

    if changed {
        if let Some(backup) = backup.clone() {
            clipboard::begin_ignore(app);
            platform::clipboard_restore(backup);
            clipboard::end_ignore(app);
        }
        if let Some(text) = &text {
            clipboard::expect_echo(text, backup);
        }
    }
    Ok(text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
}

/// 面板大小：按设置；高度设成"自动"时，同时显示多个翻译源就按源的个数加高。
fn popup_size(app: &AppHandle) -> (f64, f64) {
    let translate = state(app).settings.read().translate.clone();
    let (multi, popup) = (translate.show_all_providers, translate.popup);
    let width = f64::from(popup.width);
    if popup.height > 0 {
        return (width, f64::from(popup.height));
    }
    let sources = super::list_providers(app)
        .iter()
        .filter(|p| p.enabled && p.configured)
        .count();
    // 底部的"问 AI"输入框 44
    let extra = if popup.show_source { 64.0 } else { 0.0 } + 44.0;
    let height = if multi && sources > 1 {
        (100.0 + 96.0 * sources as f64).clamp(260.0, 560.0)
    } else {
        250.0
    };
    (width, height + extra)
}

/// 气泡、悬浮按钮常驻但隐藏（同剪贴板面板），用的时候只是摆位 + show，瞬间出现。
pub fn prewarm(app: &AppHandle) {
    if app.get_webview_window(WINDOW).is_none() {
        let built = wm::builder(app, WINDOW)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            // 右下角有拖拽手柄可以改大小，改完的尺寸记进设置
            .resizable(true)
            .min_inner_size(320.0, 200.0)
            .shadow(false)
            .focused(false)
            .inner_size(460.0, 300.0)
            .build();
        match built {
            Ok(window) => {
                wm::track_glass(&window);
                apply_popup_material(app);
            }
            Err(err) => tracing::warn!("创建翻译气泡失败：{err}"),
        }
    }
}

/// 面板材质（毛玻璃、圆角），见 `wm::set_glass`。只能在 UI 线程调用。
pub fn apply_popup_material(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW) else {
        return;
    };
    let popup = state(app).settings.read().translate.popup.clone();
    wm::set_glass(&window, popup.blur, f64::from(popup.radius));
}

fn prewarm_button(app: &AppHandle) {
    if app.get_webview_window(BUTTON_WINDOW).is_some() {
        return;
    }
    let built = wm::builder(app, BUTTON_WINDOW)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .inner_size(BUTTON_SIZE, BUTTON_SIZE)
        .build();
    match built {
        Ok(window) => {
            // 点它、显示它都不能抢走用户正在选字的那个窗口的焦点
            platform::set_no_activate(&window);
            platform::set_exclude_from_capture(&window, true);
        }
        Err(err) => tracing::warn!("创建划词按钮失败：{err}"),
    }
}

pub fn show_popup(app: &AppHandle, text: String, at: Option<(i32, i32)>) {
    *CURRENT.lock() = Some(text.clone());
    let (w, h) = popup_size(app);
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        prewarm(&app);
        let Some(window) = app.get_webview_window(WINDOW) else {
            return;
        };
        let (anchor, monitor) = match at {
            Some((x, y)) => (wm::Anchor::Point(x, y), wm::monitor_at(x, y)),
            None => (wm::Anchor::Cursor, wm::monitor_under_cursor()),
        };
        if let Some(monitor) = monitor {
            let _ = wm::place_on_monitor(&window, &monitor, w, h, anchor);
        }
        let _ = app.emit_to(WINDOW, events::TRANSLATE_REQUEST, TranslatePopup { text });
        let _ = window.show();
        let _ = window.set_focus();
    });
}

// ───────────────────────── 选字手势：悬浮按钮 / 按住修饰键 ─────────────────────────

/// 按当前设置装上或卸下划词钩子。启动时和设置变更时调用。
pub fn reconfigure(app: &AppHandle) {
    let sel = state(app).settings.read().translate.selection.clone();
    let wanted = sel.show_button || sel.modifier != "none";
    let mut watch = WATCH.lock();
    if wanted && watch.is_none() {
        let (tx, rx) = std::sync::mpsc::channel();
        match platform::start_selection_watch(tx) {
            Ok(guard) => {
                *watch = Some(guard);
                let worker = app.clone();
                let _ = std::thread::Builder::new()
                    .name("selection".into())
                    .spawn(move || {
                        // 钩子卸载时发送端被丢弃，这里自然结束
                        for event in rx {
                            on_event(&worker, event);
                        }
                    });
                tracing::info!("划词监听已开启");
            }
            Err(err) => tracing::warn!("划词监听启动失败：{err}"),
        }
    } else if !wanted && watch.is_some() {
        *watch = None;
        tracing::info!("划词监听已关闭");
    }
    drop(watch);
    if sel.show_button {
        let ui = app.clone();
        let _ = app.run_on_main_thread(move || prewarm_button(&ui));
    }
    if !sel.show_button {
        hide_button(app);
    }
}

fn on_event(app: &AppHandle, event: SelectionEvent) {
    tracing::debug!(?event, "划词事件");
    match event {
        SelectionEvent::Selected {
            anchor,
            end,
            alt,
            ctrl,
        } => on_selected(app, anchor, end, alt, ctrl),
        SelectionEvent::ButtonClicked => {
            let rect = *BUTTON.lock();
            hide_button(app);
            // 气泡出现在按钮下方
            let at = rect.map(|r| (r.x, r.bottom()));
            translate_selection(app, at);
        }
        SelectionEvent::Dismiss => hide_button(app),
    }
}

fn on_selected(app: &AppHandle, anchor: PhysicalRect, end: (i32, i32), alt: bool, ctrl: bool) {
    let st = state(app);
    let (sel, offline) = {
        let s = st.settings.read();
        (s.translate.selection.clone(), s.general.offline_mode)
    };
    if offline || st.capture.is_busy() || st.longshot.is_active() {
        return;
    }
    // 在 DATO COR 自己的窗口里选字（识字结果、翻译气泡）不弹
    if platform::foreground_window().is_some_and(platform::is_own_window) {
        return;
    }
    let held = match sel.modifier.as_str() {
        "alt" => alt,
        "ctrl" => ctrl,
        _ => false,
    };
    if held {
        hide_button(app);
        translate_selection(app, Some(end));
    } else if sel.show_button {
        show_button(app, anchor, &sel.button_position);
    }
}

fn show_button(app: &AppHandle, anchor: PhysicalRect, position: &str) {
    let Some(monitor) = wm::monitor_at(anchor.right(), anchor.bottom()) else {
        return;
    };
    let s = monitor.scale_factor.max(0.5);
    let size = (BUTTON_SIZE * s).round() as i32;
    // 鼠标一般在文字行的中间，往下 / 往上让开半行多一点，免得盖住字
    let (dx, dy) = ((4.0 * s) as i32, (14.0 * s) as i32);
    let (x, y) = match position {
        "topRight" => (anchor.right() + dx, anchor.y - dy - size),
        "bottomLeft" => (anchor.x - dx - size, anchor.bottom() + dy),
        "topLeft" => (anchor.x - dx - size, anchor.y - dy - size),
        _ => (anchor.right() + dx, anchor.bottom() + dy),
    };
    let wa = monitor.work_area;
    let x = x.clamp(wa.x, (wa.right() - size).max(wa.x));
    let y = y.clamp(wa.y, (wa.bottom() - size).max(wa.y));
    let rect = PhysicalRect::new(x, y, size as u32, size as u32);
    let generation = BUTTON_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    *BUTTON.lock() = Some(rect);
    platform::set_selection_button_rect(Some(rect));

    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        prewarm_button(&ui);
        let Some(window) = ui.get_webview_window(BUTTON_WINDOW) else {
            return;
        };
        let _ = window.set_size(tauri::PhysicalSize::new(rect.width, rect.height));
        let _ = window.set_position(tauri::PhysicalPosition::new(rect.x, rect.y));
        let _ = ui.emit_to(BUTTON_WINDOW, events::SELECTION_BUTTON_SHOW, generation);
        let _ = platform::show_without_activate(&window);
        platform::reveal_for_tests(&window, true);
    });

    // 没人理它就自己收起；鼠标停在按钮上时不收
    let timer = app.clone();
    std::thread::spawn(move || {
        let shown = Instant::now();
        let mut hovered_at: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            if BUTTON_GEN.load(Ordering::SeqCst) != generation {
                return;
            }
            let hover =
                platform::cursor_position().is_some_and(|(cx, cy)| rect.contains_point(cx, cy));
            if hover {
                hovered_at = Some(Instant::now());
                continue;
            }
            let idle_since = hovered_at.unwrap_or(shown);
            let ttl = if hovered_at.is_some() {
                Duration::from_millis(1200)
            } else {
                BUTTON_TTL
            };
            if idle_since.elapsed() >= ttl {
                hide_button(&timer);
                return;
            }
        }
    });
}

fn hide_button(app: &AppHandle) {
    if BUTTON.lock().take().is_none() {
        return;
    }
    BUTTON_GEN.fetch_add(1, Ordering::SeqCst);
    platform::set_selection_button_rect(None);
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = ui.get_webview_window(BUTTON_WINDOW) {
            // 按钮是用原生调用"显示但不激活"弹出来的，Tauri 记的可见状态可能没跟上，
            // 隐藏也走原生调用，保证真的藏起来
            platform::hide_window(&window);
            let _ = window.hide();
            platform::reveal_for_tests(&window, false);
        }
    });
}
