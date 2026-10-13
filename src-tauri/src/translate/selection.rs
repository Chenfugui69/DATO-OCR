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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppResult;
use crate::platform::{
    self, FloatingKind, PhysicalRect, SelectionEvent, SelectionWatchGuard, WindowHandle,
};
use crate::state::state;
use crate::{clipboard, events, wm};

pub const WINDOW: &str = "translate";
pub const BUTTON_WINDOW: &str = "selbtn";
/// 悬浮按钮的逻辑尺寸
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ButtonShow {
    generation: u64,
    /// 按钮底下的画面是暗的（None = 不知道，页面按应用的深浅色画）
    dark: Option<bool>,
    /// 按钮窗口盖住的那块画面（PNG 的 data URL）；None = 抓不到，页面画一个不透底的
    backdrop: Option<String>,
}

/// 一张图的平均亮度，0（黑）到 1（白）。
fn brightness(image: &image::RgbaImage) -> f64 {
    let mut sum = 0.0;
    for px in image.pixels() {
        let [r, g, b, _] = px.0;
        sum += 0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
    }
    sum / (255.0 * f64::from(image.width() * image.height()).max(1.0))
}

fn png_data_url(image: &image::RgbaImage) -> Option<String> {
    use base64::Engine;
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

/// 按钮的窗口比圆点大一圈（四周各留这么多），给玻璃的投影和悬停放大留地方：投影要完整地落在窗口里，
/// 不然被窗口的边切掉一截，看着是一块方的灰影。页面里的样式按同一个数留边。圆点多大在设置里（button_size）
const BUTTON_INSET: f64 = 12.0;
/// 没去碰它的话，按钮显示这么久后自己收起
const BUTTON_TTL: Duration = Duration::from_millis(3500);

/// 最近一次要翻译的文字。气泡页面刚加载完时主动来取（事件可能在页面就绪前就发出了）。
static CURRENT: Mutex<Option<String>> = Mutex::new(None);
/// 面板上方预留的高度（逻辑像素，见 `reserve_above`）
static RESERVE: Mutex<f64> = Mutex::new(0.0);

/// 面板现在上方预留了多高（页面加载晚了、错过事件时用）。
pub fn current_reserve() -> f64 {
    *RESERVE.lock()
}
static WATCH: Mutex<Option<SelectionWatchGuard>> = Mutex::new(None);
/// 悬浮按钮那颗圆点当前的位置（屏幕物理坐标，不含窗口四周的留边）；None = 没显示
static BUTTON: Mutex<Option<PhysicalRect>> = Mutex::new(None);
/// 按钮的窗口这会儿是不是真的摆在屏幕上（藏起来是排到主线程上做的，比 BUTTON 清空晚一点）
static BUTTON_ON_SCREEN: AtomicBool = AtomicBool::new(false);
/// 按钮窗口的句柄：抓它底下的画面时用（0 = 还没建）
static BUTTON_HANDLE: AtomicU64 = AtomicU64::new(0);
/// 每显示一次按钮加一，自动收起的计时器靠它判断按钮是不是已经换了一次
static BUTTON_GEN: AtomicU64 = AtomicU64::new(0);

pub fn current() -> Option<String> {
    CURRENT.lock().clone()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslatePopup {
    pub text: String,
    /// 面板上方预留的高度（逻辑像素）
    pub reserve: f64,
    /// 设置页里调外观时的预览：摆在主窗口旁边，不抢焦点，点外面也不收起
    pub preview: bool,
}

/// 面板这会儿是不是设置页的预览
static PREVIEW: AtomicBool = AtomicBool::new(false);
/// 预览时面板里翻译的例句
const PREVIEW_TEXT: &str = "The quick brown fox jumps over the lazy dog. Adjust the settings and this panel follows, at exactly the size you will see when translating.";

/// 设置页调划词面板的外观时，在主窗口旁边把真的面板亮出来（就是平时弹出来的那个窗口，大小、材质、圆角都是实际的）；
/// 改了设置再调一次就按新的重摆。`show = false` 收起。
pub fn preview_popup(app: &AppHandle, show: bool) {
    if show {
        open_popup(app, PREVIEW_TEXT.into(), None, true);
    } else if PREVIEW.swap(false, Ordering::SeqCst) {
        let ui = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(window) = ui.get_webview_window(WINDOW) {
                let _ = window.hide();
            }
        });
    }
}

/// 预览摆在哪：主窗口右边，放不下就左边，再不行就压在主窗口右侧里面。
fn preview_anchor(app: &AppHandle, w: f64) -> Option<(i32, i32)> {
    let main = app.get_webview_window(wm::MAIN)?;
    let (pos, size) = (main.outer_position().ok()?, main.outer_size().ok()?);
    let (left, right) = (pos.x, pos.x + size.width as i32);
    let monitor = wm::monitor_at(left + size.width as i32 / 2, pos.y + 40)?;
    let s = monitor.scale_factor.max(0.5);
    let (gap, need) = ((16.0 * s) as i32, (w * s) as i32);
    let wa = monitor.work_area;
    let x = if right + gap + need <= wa.right() {
        right + gap
    } else if left - gap - need >= wa.x {
        left - gap - need
    } else {
        right - gap - need
    };
    Some((x, pos.y + (72.0 * s) as i32))
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
        let popup = wm::builder(app, WINDOW)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            // 右下角有拖拽手柄可以改大小，改完的尺寸记进设置
            .resizable(true)
            .min_inner_size(320.0, 200.0)
            .shadow(false)
            .focused(false)
            .inner_size(460.0, 300.0);
        match platform::build_floating(popup, FloatingKind::Panel) {
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
    let button = wm::builder(app, BUTTON_WINDOW)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        // 每次显示时按设置里的大小重新摆，这里的尺寸只是个初值
        .inner_size(42.0, 42.0);
    match platform::build_floating(button, FloatingKind::Panel) {
        Ok(window) => {
            // 点它、显示它都不能抢走用户正在选字的那个窗口的焦点
            platform::set_no_activate(&window);
            platform::set_exclude_from_capture(&window, true);
            if let Ok(handle) = platform::native_handle(&window) {
                BUTTON_HANDLE.store(handle, Ordering::SeqCst);
            }
        }
        Err(err) => tracing::warn!("创建划词按钮失败：{err}"),
    }
}

pub fn show_popup(app: &AppHandle, text: String, at: Option<(i32, i32)>) {
    open_popup(app, text, at, false);
}

fn open_popup(app: &AppHandle, text: String, at: Option<(i32, i32)>, preview: bool) {
    // 预览已经开着、只是设置又变了：只按新的大小、材质重摆窗口，不再让页面重新翻译一遍
    // （每动一下滑块面板就整个刷新一次，根本没法调）
    let was_preview = PREVIEW.swap(preview, Ordering::SeqCst);
    let refresh_only = preview && was_preview;
    if !refresh_only {
        *CURRENT.lock() = Some(text.clone());
    }
    let (w, h) = popup_size(app);
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        prewarm(&app);
        let Some(window) = app.get_webview_window(WINDOW) else {
            return;
        };
        if preview {
            // 材质、圆角按最新的设置来（平时是改设置时另外应用的，这里顺手对齐一次）
            apply_popup_material(&app);
        }
        let at = if preview { preview_anchor(&app, w) } else { at };
        let (anchor, monitor) = match at {
            Some((x, y)) => (wm::Anchor::Point(x, y), wm::monitor_at(x, y)),
            None => (wm::Anchor::Cursor, wm::monitor_under_cursor()),
        };
        let mut reserve = 0.0;
        if let Some(monitor) = monitor {
            let placed = wm::rect_on_monitor(&monitor, w, h, anchor);
            let _ =
                platform::place_window(&window, placed.x, placed.y, placed.width, placed.height);
            reserve = reserve_above(&app, &window, &monitor, placed, h);
        }
        *RESERVE.lock() = reserve;
        if refresh_only && window.is_visible().unwrap_or(false) {
            return;
        }
        let _ = app.emit_to(
            WINDOW,
            events::TRANSLATE_REQUEST,
            TranslatePopup {
                text,
                reserve,
                preview,
            },
        );
        if preview {
            // 用户正在主窗口里拖滑块，不能把焦点抢走
            let _ = platform::show_without_activate(&window);
            return;
        }
        let _ = window.show();
        let _ = platform::take_focus(&window);
    });
}

/// 抽屉式"问 AI"是从输入框的位置往上长的。要是展开时才把窗口往上挪，窗口挪动和网页重画对不上同一帧，
/// 面板会跳一下。所以面板弹出时（窗口还没显示）就把窗口往上多留出抽屉要长的那么高：留出来的部分
/// 用窗口区域裁掉（不显示、点击穿透），毛玻璃背板也只铺在面板上。展开时面板在窗口里往上长，窗口不动。
/// 上面空间不够（面板贴着屏幕顶）就少留或不留，展开时页面再挪窗口。返回留了多高（逻辑像素）。
fn reserve_above(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    monitor: &platform::MonitorInfo,
    placed: PhysicalRect,
    h: f64,
) -> f64 {
    platform::set_rect_region(window, None);
    platform::set_backdrop_rect(window, None, 0);
    let popup = state(app).settings.read().translate.popup.clone();
    if popup.ai_layout != "drawer" {
        return 0.0;
    }
    let s = monitor.scale_factor.max(0.5);
    let pad = if popup.blur { 0.0 } else { 16.0 };
    // 和页面里展开抽屉的算法一致：翻译区露出一半，加上抽屉的高度
    let trans = ((h - pad * 2.0) / 2.0).round().max(110.0);
    let want = (pad * 2.0 + trans + f64::from(popup.drawer_height) - h).max(0.0);
    let room = (f64::from(placed.y - monitor.work_area.y) / s).max(0.0);
    let reserve = want.min(room).floor();
    if reserve < 1.0 {
        return 0.0;
    }
    let rp = (reserve * s).round() as i32;
    let size = placed;
    if platform::set_bounds(
        window,
        placed.x,
        placed.y - rp,
        size.width,
        size.height + rp as u32,
    )
    .is_err()
    {
        return 0.0;
    }
    platform::set_rect_region(window, Some((0, rp, size.width as i32, size.height as i32)));
    if popup.blur {
        platform::set_backdrop_rect(
            window,
            Some((0.0, rp as f32, size.width as f32, size.height as f32)),
            0,
        );
    }
    reserve
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
    // 选中的字不写进日志
    match &event {
        SelectionEvent::Selected { anchor, end, .. } => {
            tracing::debug!(?anchor, ?end, "划词事件：选了字");
        }
        other => tracing::debug!(event = ?other, "划词事件"),
    }
    match event {
        SelectionEvent::Selected {
            anchor,
            end,
            alt,
            ctrl,
            text,
        } => on_selected(app, anchor, end, alt, ctrl, text.as_deref()),
        SelectionEvent::ButtonClicked => {
            let rect = *BUTTON.lock();
            hide_button(app);
            // 气泡出现在按钮下方
            let at = rect.map(|r| (r.x, r.bottom()));
            translate_selection(app, at);
        }
        SelectionEvent::Dismiss => hide_button(app),
        SelectionEvent::PointerDown => outside_click(app),
    }
}

/// 鼠标按下了：翻译面板开着、而且不是按在面板上，就告诉面板"点了外面"，由它决定收不收（钉住、对话模式不收）。
/// 面板平时靠失焦收起，但它有时拿不到键盘焦点（那就永远等不到失焦），这条路不依赖焦点。
fn outside_click(app: &AppHandle) {
    if PREVIEW.load(Ordering::SeqCst) {
        return;
    }
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(window) = ui.get_webview_window(WINDOW) else {
            return;
        };
        if window.is_visible().unwrap_or(false) && !platform::cursor_in_window(&window) {
            let _ = ui.emit_to(WINDOW, events::TRANSLATE_OUTSIDE, ());
        }
    });
}

fn on_selected(
    app: &AppHandle,
    anchor: PhysicalRect,
    end: (i32, i32),
    alt: bool,
    ctrl: bool,
    text: Option<&str>,
) {
    let st = state(app);
    let (sel, offline) = {
        let s = st.settings.read();
        (s.translate.selection.clone(), s.general.offline_mode)
    };
    if offline || st.capture.is_busy() || st.longshot.is_active() {
        tracing::debug!(offline, "划词：离线模式或正在截图，不弹按钮");
        return;
    }
    // 在 DATO OCR 自己的窗口里选字（识字结果、翻译气泡）不弹
    if platform::foreground_window().is_some_and(platform::is_own_window) {
        tracing::debug!("划词：前台是 DATO OCR 自己的窗口，不弹按钮");
        return;
    }
    tracing::debug!(show_button = sel.show_button, modifier = %sel.modifier, "划词：弹按钮 / 直接翻译");
    let held = match sel.modifier.as_str() {
        "alt" => alt,
        "ctrl" => ctrl,
        _ => false,
    };
    if held {
        hide_button(app);
        translate_selection(app, Some(end));
    } else if sel.show_button {
        // 网址、路径、代码这些用不着翻译的不弹（按住修饰键、快捷键是明说要翻，不管这个）
        if text.is_some_and(|t| !super::worth::worth_translating(t)) {
            tracing::debug!("划词：选中的是网址 / 路径 / 代码之类，不弹按钮");
            hide_button(app);
            return;
        }
        // 按下的那个点：包围盒上和松开点相对的那个角
        let start = (
            if end.0 == anchor.x {
                anchor.right()
            } else {
                anchor.x
            },
            if end.1 == anchor.y {
                anchor.bottom()
            } else {
                anchor.y
            },
        );
        show_button(app, start, end, &sel);
    }
}

/// 按钮跟着松开鼠标的那个点走（眼睛和鼠标都在那儿），并且放在选区**外面**：
///
/// - 选了好几行：往下选的放在松开点的右下，往上选的放在左上 —— 都是顺着选的方向再往外一点，不压住选中的字；
/// - 只选了一行（或双击 / 三击）：上下按设置里的位置，往回选（从右往左）时左右反过来。
///
/// 以前是按"按下点和松开点的包围盒"的角来放，往回选、跨行选的时候按钮会跑到离鼠标很远的那个角上。
fn show_button(
    app: &AppHandle,
    start: (i32, i32),
    end: (i32, i32),
    sel: &crate::settings::SelectionTranslateSettings,
) {
    let Some(monitor) = wm::monitor_at(end.0, end.1) else {
        return;
    };
    let s = monitor.scale_factor.max(0.5);
    let px = |logical: f64| (logical * s).round() as i32;
    let (dot, inset) = (px(f64::from(sel.button_size)), px(BUTTON_INSET));
    let (mut right, mut below) = match sel.button_position.as_str() {
        "topRight" => (true, false),
        "bottomLeft" => (false, true),
        "topLeft" => (false, false),
        _ => (true, true),
    };
    // 上下差出大半行就算换行了
    let rows = end.1 - start.1;
    if rows.abs() > px(12.0) {
        below = rows > 0;
        right = rows > 0;
    } else if end.0 < start.0 {
        right = !right;
    }
    // 鼠标一般在文字行的中间，往下 / 往上让开半行多一点，免得盖住字
    let (dx, dy) = (px(4.0), px(14.0));
    let x = if right { end.0 + dx } else { end.0 - dx - dot };
    let y = if below { end.1 + dy } else { end.1 - dy - dot };
    let wa = monitor.work_area;
    let x = x.clamp(wa.x, (wa.right() - dot).max(wa.x));
    let y = y.clamp(wa.y, (wa.bottom() - dot).max(wa.y));
    let dot_rect = PhysicalRect::new(x, y, dot as u32, dot as u32);
    let size = (dot + inset * 2) as u32;
    let rect = PhysicalRect::new(x - inset, y - inset, size, size);
    let generation = BUTTON_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    // 按钮还没显示，这时候把它要盖住的那块画面抓下来：玻璃按它的明暗用浅色或深色，
    // 页面再把这块画面放进珠子里（放大一点、模糊一点），看着像透过玻璃看到的
    // 上一颗按钮还摆着（比如双击之后紧接着三击）：只抓它底下的，别把旧的那颗连同投影抓进新的珠子里
    let below = Some(BUTTON_HANDLE.load(Ordering::SeqCst))
        .filter(|&h| h != 0 && BUTTON_ON_SCREEN.load(Ordering::SeqCst))
        .map(WindowHandle);
    let patch = platform::screen_patch(rect, below);
    let dark = patch.as_ref().map(|p| brightness(p) < 0.5);
    let backdrop = patch.as_ref().and_then(png_data_url);
    // 能点的、算"鼠标停在上面"的只有圆点，四周的留边不算
    *BUTTON.lock() = Some(dot_rect);
    platform::set_selection_button_rect(Some(dot_rect));

    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        prewarm_button(&ui);
        let Some(window) = ui.get_webview_window(BUTTON_WINDOW) else {
            return;
        };
        let _ = platform::place_window(&window, rect.x, rect.y, rect.width, rect.height);
        let _ = ui.emit_to(
            BUTTON_WINDOW,
            events::SELECTION_BUTTON_SHOW,
            ButtonShow {
                generation,
                dark,
                backdrop,
            },
        );
        let _ = platform::show_without_activate(&window);
        BUTTON_ON_SCREEN.store(true, Ordering::SeqCst);
        wm::reveal_floating(&ui, &window);
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
                platform::cursor_position().is_some_and(|(cx, cy)| dot_rect.contains_point(cx, cy));
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

/// 调试用（`--demo=selbtn`）：不用真的选字，直接在鼠标旁边把按钮亮出来看样子。
#[cfg(debug_assertions)]
pub fn demo_button(app: &AppHandle, at: Option<(i32, i32)>) {
    let (x, y) = at.or_else(platform::cursor_position).unwrap_or((400, 400));
    let sel = state(app).settings.read().translate.selection.clone();
    show_button(app, (x, y), (x, y), &sel);
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
            wm::conceal_floating(&window);
            platform::hide_window(&window);
            let _ = window.hide();
            // 这期间又弹出了新的一颗就别动（藏和显示都排在主线程上，顺序和这里看到的一致）
            if BUTTON.lock().is_none() {
                BUTTON_ON_SCREEN.store(false, Ordering::SeqCst);
            }
        }
    });
}
