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
/// 弹按钮时系统告诉我们的选中文字（问得到时才有）。点按钮时模拟 ⌘C 没取到字（有的程序 ⌘C 不响应），就用它
static BUTTON_TEXT: Mutex<Option<String>> = Mutex::new(None);
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
    /// 输入翻译：没有原文，面板上显示输入框让用户自己打字
    pub input: bool,
}

/// 面板这会儿是不是输入翻译（页面刚加载完主动来问时用）
static INPUT: AtomicBool = AtomicBool::new(false);

pub fn is_input() -> bool {
    INPUT.load(Ordering::SeqCst)
}

/// 面板这会儿是不是设置页的预览
static PREVIEW: AtomicBool = AtomicBool::new(false);
/// 预览上次套用的材质（毛玻璃开关、圆角），没变就不重设
static PREVIEW_MATERIAL: Mutex<Option<(bool, u32)>> = Mutex::new(None);
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
                // 预览是原生"显示但不激活"亮出来的，藏也要走原生的：Tauri 的 hide() 以为它本来就藏着，什么都不做
                platform::hide_window(&window);
                // 反过来，预览之前面板要是正常开着（钉住、对话模式），Tauri 记着"显示中"；原生藏了它不知道，
                // 下次点划词按钮 show() 以为已经显示、什么都不做，面板就弹不出来。这里把它记的状态也改成藏着
                let _ = window.hide();
            }
            let _ = ui.emit_to(wm::MAIN, events::TRANSLATE_PREVIEW_CLOSED, ());
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
    translate_selection_or(app, at, None);
}

/// 同上；`known` 是弹按钮时就问到的选中文字：模拟 ⌘C 没取到（或者出错）时用它
fn translate_selection_or(app: &AppHandle, at: Option<(i32, i32)>, known: Option<String>) {
    let app = app.clone();
    std::thread::spawn(move || {
        let grabbed = grab_selection(&app);
        match (grabbed, known) {
            (Ok(Some(text)), _) | (Ok(None) | Err(_), Some(text)) => {
                tracing::debug!(chars = text.chars().count(), ?at, "划词取到文字");
                show_popup(&app, text, at);
            }
            (Ok(None), None) => wm::toast(&app, "info", "没有选中文字"),
            (Err(err), None) => wm::toast(&app, "error", format!("读取选中文字失败：{err}")),
        }
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

/// 翻译面板窗口四周给阴影留的透明边（逻辑像素，和页面 TranslatePopup 的 SHADOW_PAD 一致）。
/// macOS 毛玻璃是系统材质铺满窗口、阴影是系统的，不留；Windows 毛玻璃背板往里缩，阴影页面自己画
fn popup_pad(blur: bool) -> f64 {
    if blur && cfg!(target_os = "macos") {
        0.0
    } else {
        16.0
    }
}

/// 面板材质（毛玻璃、圆角），见 `wm::set_glass`。只能在 UI 线程调用。
pub fn apply_popup_material(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW) else {
        return;
    };
    let popup = state(app).settings.read().translate.popup.clone();
    wm::set_glass(&window, popup.blur, f64::from(popup.radius), popup_pad(popup.blur));
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
    INPUT.store(false, Ordering::SeqCst);
    open_popup(app, text, at, false);
}

/// 输入翻译（热键 / 托盘菜单）：在鼠标旁边弹出空的翻译面板，光标在输入框里
/// 输入翻译：屏幕中间那条输入框（见 `spotlight`）。以前借用划词翻译的气泡，在鼠标那里弹、样子也不对
pub fn show_input(app: &AppHandle) {
    super::spotlight::show(app);
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
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        // 面板不预建，用到才建（等页面加载完再显示），闲置一阵后释放，见 lifecycle.rs
        crate::lifecycle::when_ready(&ui, WINDOW, prewarm, move |app, window| {
            if preview {
                // 材质、圆角按最新的设置来（平时是改设置时另外应用的，这里顺手对齐一次）。
                // 只在毛玻璃、圆角真的变了时重设：重设要先拆掉背板和圆角再挂上，拖宽高滑块时每一下都重来，面板一直闪
                let popup = state(app).settings.read().translate.popup.clone();
                let material = (popup.blur, popup.radius);
                let mut last = PREVIEW_MATERIAL.lock();
                if !refresh_only || *last != Some(material) {
                    *last = Some(material);
                    apply_popup_material(app);
                }
            }
            let at = if preview { preview_anchor(app, w) } else { at };
            let (anchor, monitor) = match at {
                Some((x, y)) => (wm::Anchor::Point(x, y), wm::monitor_at(x, y)),
                None => (wm::Anchor::Cursor, wm::monitor_under_cursor()),
            };
            let mut reserve = 0.0;
            if let Some(monitor) = monitor {
                let placed = wm::rect_on_monitor(&monitor, w, h, anchor);
                if preview && cfg!(windows) {
                    // Windows 的预览不给"问 AI"抽屉往上预留：预留要先摆好、再把窗口往上扩、再裁掉扩出来的那截，
                    // 拖滑块时每一下都走一遍，中间那几帧面板忽高忽低；`place_window` 自己也要挪、改大小、再挪三步。
                    // 这里一步摆到位。真要展开抽屉，页面会自己挪窗口
                    platform::set_rect_region(&window, None);
                    platform::set_backdrop_rect(&window, None, 0);
                    let _ = platform::set_bounds(
                        &window,
                        placed.x,
                        placed.y,
                        placed.width,
                        placed.height,
                    );
                    // 背板 / 圆角立刻按新大小裁，别等 Resized 事件
                    wm::refit_glass(&window);
                } else {
                    let _ = platform::place_window(
                        &window,
                        placed.x,
                        placed.y,
                        placed.width,
                        placed.height,
                    );
                    reserve = reserve_above(app, &window, &monitor, placed, h);
                }
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
                    input: !preview && INPUT.load(Ordering::SeqCst),
                },
            );
            if preview {
                // 用户正在主窗口里拖滑块，不能把焦点抢走
                let _ = platform::show_without_activate(&window);
                return;
            }
            // Tauri 的 show() 只在它记的状态从"藏"变"显示"时才真的显示。窗口被原生调用藏过的话，
            // 它可能还记着"显示中"，先对齐一下
            if !window.is_visible().unwrap_or(false) {
                let _ = window.hide();
            }
            let _ = window.show();
            let _ = platform::take_focus(&window);
        });
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
    let pad = popup_pad(popup.blur);
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
        // 背板停在面板上（窗口顶上预留的那截不铺，四周留给阴影的边也不铺）
        let p = (pad * s) as f32;
        platform::set_backdrop_rect(
            window,
            Some((p, rp as f32 + p, size.width as f32 - 2.0 * p, size.height as f32 - 2.0 * p)),
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
    let ui = app.clone();
    if sel.show_button {
        // 小圆点常驻：选中文字后它得马上出来，等现建窗口、加载页面要两三百毫秒
        let _ = app.run_on_main_thread(move || prewarm_button(&ui));
    } else {
        hide_button(app);
        // 关掉了就不用留着它占内存
        let _ = app.run_on_main_thread(move || {
            if let Some(window) = ui.get_webview_window(BUTTON_WINDOW) {
                let _ = window.destroy();
            }
        });
    }
}

fn on_event(app: &AppHandle, event: SelectionEvent) {
    // 选中的字不写进日志
    match &event {
        SelectionEvent::Selected { anchor, end, text_rect, .. } => {
            tracing::debug!(?anchor, ?end, ?text_rect, "划词事件：选了字");
        }
        // 每次按鼠标都有一条，不记
        SelectionEvent::PointerDown => {}
        other => tracing::debug!(event = ?other, "划词事件"),
    }
    match event {
        SelectionEvent::Selected {
            anchor,
            end,
            alt,
            ctrl,
            text,
            text_rect,
        } => on_selected(app, anchor, end, alt, ctrl, text.as_deref(), text_rect),
        SelectionEvent::ButtonClicked => button_clicked(app),
        SelectionEvent::Dismiss => {
            hide_button(app);
            reconcile_button(app);
        }
        SelectionEvent::PointerDown => {
            outside_click(app);
            reconcile_button(app);
        }
    }
}

/// 系统的语言（zh、en、ja……，取系统首选语言的语言部分；繁体中文的系统是 zh-TW，和检测结果对得上）
pub fn system_language() -> String {
    let Some(locale) = sys_locale::get_locale() else {
        return "en".into();
    };
    let l = locale.to_ascii_lowercase().replace('_', "-");
    if l.starts_with("zh") && (l.contains("hant") || ["-tw", "-hk", "-mo"].iter().any(|s| l.ends_with(s))) {
        return "zh-TW".into();
    }
    l.split('-').next().unwrap_or("en").to_string()
}

/// 选中的文字是这种语言时要不要不弹按钮
fn skip_language(sel: &crate::settings::SelectionTranslateSettings, lang: &str) -> bool {
    (sel.skip_system_language && system_language() == lang)
        || sel.skip_languages.iter().any(|l| l == lang)
}

/// 点了圆点：收起圆点，在它下面弹翻译面板
fn button_clicked(app: &AppHandle) {
    let rect = *BUTTON.lock();
    hide_button(app);
    // 气泡出现在按钮下方
    let at = rect.map(|r| (r.x, r.bottom()));
    let known = BUTTON_TEXT.lock().take();
    translate_selection_or(app, at, known);
}

/// 对一下账：圆点已经收起了（`BUTTON` 是空的），窗口却还亮着 —— 就再藏一次。
///
/// 正常不会这样，但用户遇到过：圆点一直留在桌面上，点它没反应（监听那边已经当它收起了，不认这次点击），
/// 点别处也收不掉。每次鼠标按下都对一下，残留的圆点在下一次点击时就消失。
fn reconcile_button(app: &AppHandle) {
    if BUTTON.lock().is_some() {
        return;
    }
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        if BUTTON.lock().is_some() {
            return;
        }
        let Some(window) = ui.get_webview_window(BUTTON_WINDOW) else {
            return;
        };
        // 不看"窗口显示着没有"：那个状态有时和屏幕上看到的对不上（用户遇到过它说藏了、圆点却还在）。
        // 藏一个已经藏起来的窗口什么也不会发生，直接再藏一次最稳
        if BUTTON_ON_SCREEN.swap(false, Ordering::SeqCst) || window.is_visible().unwrap_or(false) {
            tracing::info!("划词按钮已经收起，窗口却还在，再藏一次");
        }
        wm::conceal_floating(&window);
        platform::hide_window(&window);
        let _ = window.hide();
    });
}

/// 圆点页面自己收到了点击：正常情况下圆点上的点击在监听那边就被拦下了，到不了页面；到了说明两边
/// 对不上（监听不知道圆点在这）。圆点该在就照常弹翻译，不该在就把它收起来
pub fn button_pressed_in_page(app: &AppHandle) {
    if BUTTON.lock().is_some() {
        tracing::warn!("划词按钮的点击没被监听认出来，按点了按钮处理");
        button_clicked(app);
    } else {
        reconcile_button(app);
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
        if !window.is_visible().unwrap_or(false) {
            return;
        }
        // Windows：鼠标下面就是面板窗口时，把按下的点（相对窗口、CSS 像素）交给页面，由它看是不是按在
        // 面板上 —— 窗口四周有给阴影留的透明边、顶上还可能有给展开留的空白，按在那些地方也算外面。
        // 下面是别的窗口就是 null（肯定在外面）
        #[cfg(windows)]
        {
            let on_popup = platform::cursor_position().and_then(|c| {
                let own = platform::native_handle(&window).ok()?;
                (platform::window_at(c.0, c.1)?.0 == own).then_some(c)
            });
            let point = on_popup.and_then(|c| {
                let pos = window.inner_position().ok()?;
                let s = window.scale_factor().ok()?.max(0.5);
                Some([f64::from(c.0 - pos.x) / s, f64::from(c.1 - pos.y) / s])
            });
            let _ = ui.emit_to(WINDOW, events::TRANSLATE_OUTSIDE, point);
        }
        #[cfg(not(windows))]
        if !platform::cursor_in_window(&window) {
            let _ = ui.emit_to(WINDOW, events::TRANSLATE_OUTSIDE, None::<[f64; 2]>);
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
    text_rect: Option<crate::platform::TextBounds>,
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
        // 设置里说了这种语言不弹（问不到选中的字时不知道是什么语言，照弹）
        if let Some(t) = text {
            let lang = super::detect::detect(t);
            if skip_language(&sel, lang) {
                tracing::debug!(lang, "划词：这种语言设置成不弹");
                hide_button(app);
                return;
            }
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
        *BUTTON_TEXT.lock() = text.map(str::to_string);
        // Windows 的划词监听只知道鼠标按下、松开的点：选了"跟着文段"时再问一下系统选中的字在哪
        // （松开鼠标后程序要一小会儿才更新选区）。问不到照旧跟着鼠标
        let text_rect = text_rect.or_else(|| {
            if sel.button_follow != "text" {
                return None;
            }
            // 浏览器类程序的无障碍树刚被叫醒时头一两次还问不到：隔一小会儿再问，最多多等两百来毫秒
            let started = std::time::Instant::now();
            let mut b = None;
            for wait in [40, 90, 130] {
                std::thread::sleep(std::time::Duration::from_millis(wait));
                b = platform::selection_text_bounds(end.0, end.1);
                if b.is_some() {
                    break;
                }
            }
            tracing::debug!(?b, elapsed_ms = started.elapsed().as_millis() as u64, "划词：问到的选中文字位置");
            b
        });
        show_button(app, start, end, text_rect, &sel);
    }
}

/// 两个点按阅读顺序排（先上后下，同一行先左后右）：(选区开头, 选区结尾)。`line` 以内的上下差算同一行
fn reading_order(a: (i32, i32), b: (i32, i32), line: i32) -> ((i32, i32), (i32, i32)) {
    let a_first = if (a.1 - b.1).abs() > line { a.1 < b.1 } else { a.0 <= b.0 };
    if a_first {
        (a, b)
    } else {
        (b, a)
    }
}

/// 点 `p` 离矩形 `r` 不超过 `max`（在里面算 0）
fn near(r: PhysicalRect, p: (i32, i32), max: i32) -> bool {
    let dx = (r.x - p.0).max(p.0 - r.right()).max(0);
    let dy = (r.y - p.1).max(p.1 - r.bottom()).max(0);
    dx <= max && dy <= max
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
    text_rect: Option<crate::platform::TextBounds>,
    sel: &crate::settings::SelectionTranslateSettings,
) {
    let Some(monitor) = wm::monitor_at(end.0, end.1) else {
        return;
    };
    let s = monitor.scale_factor.max(0.5);
    let px = |logical: f64| (logical * s).round() as i32;
    // 小圆点按点本身的大小摆（窗口小一圈），这样才能紧贴着字
    let pip = sel.button_style == "dot";
    let size_pt = if pip { sel.plain_dot.size } else { sel.button_size };
    let (dot, inset) = (px(f64::from(size_pt)), px(BUTTON_INSET));
    let (mut right, mut below) = match sel.button_position.as_str() {
        "topRight" => (true, false),
        "bottomLeft" => (false, true),
        "topLeft" => (false, false),
        _ => (true, true),
    };
    // "跟着文段"：贴在选中那段字的角上（设置里的哪个角就是哪个角，从哪头开始选都一样）。
    // 问不到文字范围、或者问到的范围离按下、松开鼠标的两个点都很远（有的程序报的不准）时照旧跟着鼠标。
    // 只看松开点不行：拖过了头、在字外面的空白处松手很常见，那时选区离松开点可能很远，但按下点一定在字上
    let text_rect = text_rect
        .filter(|_| sel.button_follow == "text")
        .filter(|b| near(b.whole, end, px(160.0)) || near(b.whole, start, px(160.0)));
    let (x, y) = if let Some(b) = text_rect {
        // 靠右的位置贴在选区最后一个字后面，靠左的贴在第一个字前面；上下按设置，贴着那个字所在的行。
        // 倒着选（从下往上、从右往左）时松开鼠标的地方在选区开头：位置整个反过来（右下变左上），
        // 按钮出现在眼睛和鼠标所在的那一头
        let backward = reading_order(start, end, px(12.0)).0 == end && start != end;
        if backward {
            right = !right;
            below = !below;
        }
        let gap = px(3.0);
        let edge = if right { b.last } else { b.first };
        if let Some(r) = edge {
            if pip {
                // 小圆点紧贴着那个字：横向挨着字的边，纵向压在那一行的底边 / 顶边上
                let x = if right { r.right() + px(1.0) } else { r.x - px(1.0) - dot };
                let y = if below { r.bottom() - dot / 2 } else { r.y - dot / 2 };
                (x, y)
            } else {
                let x = if right { r.right() + gap } else { r.x - gap - dot };
                let y = if below { r.bottom() + gap } else { r.y - gap - dot };
                (x, y)
            }
        } else {
            // 问不到单个字（Chrome、Edge、Electron 程序就不答）：选区的头尾就是按下、松开鼠标的那两个点
            // （按阅读顺序排，从下往上选时结尾是按下的那个点）。整段外框它们报得也不准（常常连后面没选的
            // 段落一起算进去），只拿来别让按钮跑出文字的左右边
            let (head, tail) = reading_order(start, end, px(12.0));
            let p = if right { tail } else { head };
            let x = if right {
                p.0.min(b.whole.right()) + px(4.0)
            } else {
                p.0.max(b.whole.x) - px(4.0) - dot
            };
            // 鼠标一般在文字行的中间，往下 / 往上让开半行多一点（拖到字外面的空白处才松手时先拉回选区里）
            let py = p.1.clamp(b.whole.y, b.whole.bottom().max(b.whole.y));
            let y = if below { py + px(14.0) } else { py - px(14.0) - dot };
            (x, y)
        }
    } else {
        // 上下差出大半行就算换行了
        let rows = end.1 - start.1;
        if rows.abs() > px(12.0) {
            below = rows > 0;
            right = rows > 0;
        } else if end.0 < start.0 {
            right = !right;
        }
        // 鼠标一般在文字行的中间，往下 / 往上让开半行多一点，免得盖住字（小圆点小，让开得少一些）
        let (dx, dy) = if pip { (px(3.0), px(8.0)) } else { (px(4.0), px(14.0)) };
        let x = if right { end.0 + dx } else { end.0 - dx - dot };
        let y = if below { end.1 + dy } else { end.1 - dy - dot };
        (x, y)
    };
    let wa = monitor.work_area;
    let x = x.clamp(wa.x, (wa.right() - dot).max(wa.x));
    let y = y.clamp(wa.y, (wa.bottom() - dot).max(wa.y));
    let size = (dot + inset * 2) as u32;
    let rect = PhysicalRect::new(x - inset, y - inset, size, size);
    // 能点的范围：小圆点太小不好点中，四周各放宽一点（还在窗口留边以内）
    let grow = if pip { px(5.0) } else { 0 };
    let dot_rect = PhysicalRect::new(x - grow, y - grow, (dot + grow * 2) as u32, (dot + grow * 2) as u32);
    let generation = BUTTON_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    // 按钮还没显示，这时候把它要盖住的那块画面抓下来：玻璃按它的明暗用浅色或深色，
    // 页面再把这块画面放进珠子里（放大一点、模糊一点），看着像透过玻璃看到的
    // 上一颗按钮还摆着（比如双击之后紧接着三击）：只抓它底下的，别把旧的那颗连同投影抓进新的珠子里
    let below = Some(BUTTON_HANDLE.load(Ordering::SeqCst))
        .filter(|&h| h != 0 && BUTTON_ON_SCREEN.load(Ordering::SeqCst))
        .map(WindowHandle);
    // 纯色、小圆点不透出底下的画面，也就不用抓（抓屏是弹按钮最费时的一步）
    let patch = if matches!(sel.button_style.as_str(), "solid" | "dot") {
        None
    } else {
        platform::screen_patch(rect, below)
    };
    let dark = patch.as_ref().map(|p| brightness(p) < 0.5);
    let backdrop = patch.as_ref().and_then(png_data_url);
    // 能点的、算"鼠标停在上面"的只有圆点，四周的留边不算
    *BUTTON.lock() = Some(dot_rect);
    platform::set_selection_button_rect(Some(dot_rect));

    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        // 按钮不预建，第一次选字时才建（之后常驻一阵，闲置后释放，见 lifecycle.rs）
        crate::lifecycle::when_ready(&ui, BUTTON_WINDOW, prewarm_button, move |ui, window| {
            // 第一次要等窗口建好，这期间又选了别的、或者按钮已经被收起了：这一颗作废
            if BUTTON_GEN.load(Ordering::SeqCst) != generation {
                return;
            }
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
            wm::reveal_floating(ui, &window);
            tracing::info!(generation, "划词按钮显示");
        });
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
    show_button(app, (x, y), (x, y), None, &sel);
}

fn hide_button(app: &AppHandle) {
    if BUTTON.lock().take().is_none() {
        return;
    }
    let generation = BUTTON_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    tracing::info!(generation, "划词按钮收起");
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
    // 稍后再对一次账：万一有还没执行的显示排在了这次隐藏后面
    let later = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        reconcile_button(&later);
    });
}

#[cfg(test)]
mod tests {
    use super::reading_order;

    #[test]
    fn head_and_tail_follow_reading_order() {
        // 往下选：按下点在前
        assert_eq!(reading_order((100, 10), (50, 200), 12), ((100, 10), (50, 200)));
        // 从下往上选：松开点在前，结尾是按下的那个点
        assert_eq!(reading_order((50, 200), (100, 10), 12), ((100, 10), (50, 200)));
        // 同一行从右往左
        assert_eq!(reading_order((300, 100), (80, 104), 12), ((80, 104), (300, 100)));
    }
}
