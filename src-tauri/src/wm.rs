//! 窗口管理：主窗口、toast、通用建窗工具、系统材质。
//!
//! 所有窗口共用一个 `index.html`，前端按窗口标签决定渲染哪个视图
//! （`src/main.tsx`），所以这里建窗只需要给标签。

use serde::Serialize;
use tauri::window::{Effect, EffectsBuilder};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::error::AppResult;
use crate::events;
use crate::platform::{self, FloatingKind, MonitorInfo, PhysicalRect};
use crate::state::state;

pub const MAIN: &str = "main";
pub const TOAST: &str = "toast";

/// 新建一个无边框窗口的通用构建器（默认隐藏）。
pub fn builder<'a>(
    app: &'a AppHandle,
    label: &'a str,
) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title("DATO OCR")
        .decorations(false)
        .visible(false)
}

/// 带标题栏的"正经窗口"（识字、编辑、AI）的构建器：外框按平台的习惯来
/// （Windows 无边框、页面自己画标题栏；macOS 用系统的红黄绿灯）。
pub fn framed_builder<'a>(
    app: &'a AppHandle,
    label: &'a str,
) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    platform::frame_window(builder(app, label))
}

pub fn show_main(app: &AppHandle, page: Option<&str>) {
    let page = page.map(str::to_string);
    let ui = app.clone();
    // 主窗口不预建（藏着也要一百多 MB），用到才建；藏起来一阵后释放，见 lifecycle.rs
    let _ = app.run_on_main_thread(move || {
        crate::lifecycle::when_ready(&ui, MAIN, create_main, move |app, window| {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
            if let Some(page) = page {
                let _ = app.emit_to(MAIN, events::NAVIGATE, page);
            }
        });
    });
}

/// 按配置文件里的样子建主窗口（`tauri.conf.json` 里 `create: false`，不让它启动时自己建）。
fn create_main(app: &AppHandle) {
    let Some(config) = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN)
        .cloned()
    else {
        tracing::error!("配置里没有主窗口");
        return;
    };
    match WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build()) {
        Ok(window) => apply_window_effects(app, &window),
        Err(err) => tracing::error!("创建主窗口失败：{err}"),
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
    let r = rect_on_monitor(monitor, w, h, anchor);
    platform::place_window(window, r.x, r.y, r.width, r.height)
}

/// `place_on_monitor` 会把窗口摆在哪（屏幕物理像素）。
pub fn rect_on_monitor(monitor: &MonitorInfo, w: f64, h: f64, anchor: Anchor) -> PhysicalRect {
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
    PhysicalRect::new(x, y, pw.max(1) as u32, ph.max(1) as u32)
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
    /// 主窗口是否启用了系统材质（Windows 的 Mica、macOS 的毛玻璃）。false 时前端必须画不透明底色。
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

/// 按当前能力给窗口加/去系统材质（主窗口、识字窗口这类"正经窗口"）。
pub fn apply_window_effects(app: &AppHandle, window: &WebviewWindow) {
    let caps = visual_capabilities(app);
    if let Err(err) = window.set_effects(platform::window_effects(caps.glass)) {
        tracing::warn!(label = window.label(), "设置窗口材质失败：{err}");
    }
    // Win11：无边框 + 透明的窗口系统默认不给圆角，四个角是直的，明确要一下（最大化时系统自己去掉）。
    // macOS 上是空操作
    platform::set_rounded(window, true);
}

/// 毛玻璃面板的圆角半径（逻辑像素），按窗口标签存；窗口大小一变就按它重新裁。
static GLASS: parking_lot::Mutex<Option<std::collections::HashMap<String, f64>>> =
    parking_lot::Mutex::new(None);

/// 浮动面板的材质：
/// - 毛玻璃：窗口最底下挂一层系统模糊背板，按用户要的半径（`radius`，0 = 直角）裁成圆角，
///   网页盖在上面只铺一层淡色调（见 `platform::set_backdrop`）
/// - 不开毛玻璃：窗口全透明，圆角和阴影由页面自己画（四周留了透明边）
///
/// 不能用 Tauri 的 `set_effects`：Acrylic 在 Win11 上铺满整个矩形窗口、裁不掉，圆角外面露出方块；
/// Blur（老的 ACCENT_ENABLE_BLURBEHIND）配 WebView2 背后是黑的。也不能开系统阴影（会把窗框延伸
/// 进来、窗口变大）。只能在 UI 线程调用。
pub fn set_glass(window: &WebviewWindow, blur: bool, radius: f64) {
    // 以前的版本用过系统亚克力和窗口裁剪，先都清掉
    let _ = window.set_effects(None::<tauri::utils::config::WindowEffectsConfig>);
    platform::set_rounded(window, false);
    platform::set_round_region(window, 0);
    let r = blur.then(|| radius.max(0.0));
    {
        let mut glass = GLASS.lock();
        let map = glass.get_or_insert_with(Default::default);
        match r {
            Some(r) => map.insert(window.label().to_string(), r),
            None => map.remove(window.label()),
        };
    }
    let s = window.scale_factor().unwrap_or(1.0);
    let px = r.map(|r| (r * s).round() as u32);
    if !platform::set_backdrop(window, px) {
        // Win10 挂不上背板：退回系统亚克力，再把窗口裁成圆角（Win10 的亚克力走窗口合成属性，跟着窗口区域走）
        let effects = EffectsBuilder::new().effect(Effect::Acrylic).build();
        if let Err(err) = window.set_effects(Some(effects)) {
            tracing::warn!(label = window.label(), "设置面板材质失败：{err}");
        }
        platform::set_round_region(window, px.unwrap_or(0));
    }
}

/// 窗口大小变了：背板裁剪（Win10 是窗口的圆角裁剪）按新大小重来。只能在 UI 线程调用。
/// 我们自己改了窗口大小时直接调这个，别等 `Resized` 事件 —— 那要晚一两帧，这期间圆角外面露出方角。
pub fn refit_glass(window: &WebviewWindow) {
    // Win10 退回窗口裁剪的情况：窗口一变大小就得按新大小重新裁
    if platform::resize_backdrop(window) {
        return;
    }
    let r = GLASS
        .lock()
        .as_ref()
        .and_then(|m| m.get(window.label()).copied());
    if let Some(r) = r {
        let s = window.scale_factor().unwrap_or(1.0);
        platform::set_round_region(window, (r * s).round() as u32);
    }
}

/// 面板窗口创建时调一次：窗口大小变了背板裁剪跟着变，缩放变了圆角半径按新缩放重算。
pub fn track_glass(window: &WebviewWindow) {
    let w = window.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Resized(_) => refit_glass(&w),
        tauri::WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
            // 先把半径取出来再设：设的时候系统可能同步发回窗口消息
            let r = GLASS
                .lock()
                .as_ref()
                .and_then(|m| m.get(w.label()).copied());
            if let Some(r) = r {
                let px = (r * scale_factor).round() as u32;
                if !platform::set_backdrop(&w, Some(px)) {
                    platform::set_round_region(&w, px);
                }
            }
        }
        _ => {}
    });
}

/// 本程序自己截图时，开着毛玻璃的面板（划词面板、剪贴板面板）换成页面自己画的实心底。
///
/// Windows 的抓屏（WGC）里，系统模糊背板会无视圆角裁剪、画成整块矩形，冻结的画面里面板四角露出方块；
/// 窗口区域裁圆角又只能整像素地裁，四角是一级一级的台阶。所以抓屏前：通知面板把色调调成不透明
/// （`glass-snapshot` 事件，页面换好、画完了回 `window_glass_snapshot_ready`），再把背板藏起来，
/// 抓到的面板就全是页面画的，圆角带抗锯齿。抓完（这个值丢掉时）换回去。
/// macOS 的系统材质抓屏里是对的，什么都不做。
pub struct GlassSnapshot {
    app: AppHandle,
    windows: Vec<WebviewWindow>,
}

/// 页面回了"已经换好"的窗口
static SNAPSHOT_READY: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());
static SNAPSHOT_WAKE: parking_lot::Condvar = parking_lot::Condvar::new();

/// 页面换好实心底、画完了一帧（`window_glass_snapshot_ready`）
pub fn glass_snapshot_ready(label: &str) {
    SNAPSHOT_READY.lock().push(label.to_string());
    SNAPSHOT_WAKE.notify_all();
}

impl GlassSnapshot {
    /// 抓屏之前调；没有开着的毛玻璃面板（或者不是 Windows）时返回 None，不耽误时间
    pub fn begin(app: &AppHandle) -> Option<Self> {
        if !cfg!(windows) {
            return None;
        }
        let labels: Vec<String> = GLASS
            .lock()
            .as_ref()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let windows: Vec<WebviewWindow> = labels
            .iter()
            .filter_map(|l| app.get_webview_window(l))
            .filter(|w| w.is_visible().unwrap_or(false))
            .collect();
        if windows.is_empty() {
            return None;
        }
        SNAPSHOT_READY.lock().clear();
        for w in &windows {
            let _ = app.emit_to(w.label(), events::GLASS_SNAPSHOT, true);
        }
        // 等页面换好（最多 150ms：页面卡住了也不能让截图一直等）
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
        let mut ready = SNAPSHOT_READY.lock();
        while !windows.iter().all(|w| ready.iter().any(|l| l == w.label())) {
            if SNAPSHOT_WAKE.wait_until(&mut ready, deadline).timed_out() {
                tracing::debug!("面板没及时换成实心底，照常截图");
                break;
            }
        }
        drop(ready);
        // 背板藏起来（要在 UI 线程上动）；合成器下一帧才生效，再等两帧
        let (tx, rx) = std::sync::mpsc::channel();
        let ws = windows.clone();
        let _ = app.run_on_main_thread(move || {
            for w in &ws {
                platform::fade_backdrop(w, false, 0);
            }
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(std::time::Duration::from_millis(100));
        std::thread::sleep(std::time::Duration::from_millis(34));
        Some(Self {
            app: app.clone(),
            windows,
        })
    }
}

impl Drop for GlassSnapshot {
    fn drop(&mut self) {
        let ws = std::mem::take(&mut self.windows);
        for w in &ws {
            let _ = self.app.emit_to(w.label(), events::GLASS_SNAPSHOT, false);
        }
        let _ = self.app.run_on_main_thread(move || {
            for w in &ws {
                platform::fade_backdrop(w, true, 0);
            }
        });
    }
}

/// 提示条、划词按钮这类小浮层显示出来时让抓屏看得到：远程控制、屏幕共享的对面看到的画面是抓屏
/// 抓来的，一直排除的话对面根本看不见它们（远程用划词时按钮"不出现"就是这个原因）。
/// 正在截图 / 长截图 / 录 GIF 时不放开，免得被拍进去；收起时调 `conceal_floating`。
pub fn reveal_floating(app: &AppHandle, window: &WebviewWindow) {
    let st = state(app);
    let capturing =
        st.capture.is_busy() || st.longshot.is_active() || crate::gif_record::is_active();
    platform::set_exclude_from_capture(window, capturing);
}

/// 小浮层收起、或者马上要抓屏时：重新排除。要在藏窗口 / 抓屏之前调。
pub fn conceal_floating(window: &WebviewWindow) {
    platform::set_exclude_from_capture(window, true);
}

/// 马上要抓屏了：还显示着的提示条、划词按钮先排除掉，别拍进截图里。
pub fn conceal_all_floating(app: &AppHandle) {
    for label in [TOAST, crate::translate::selection::BUTTON_WINDOW] {
        if let Some(window) = app.get_webview_window(label) {
            conceal_floating(&window);
        }
    }
}

/// 改了界面语言：已经开着的窗口，标题跟着换。
pub fn retitle(app: &AppHandle) {
    for (label, title) in [
        (crate::ai::WINDOW, "AI 对话 - DATO OCR"),
        (crate::editor::WINDOW, "编辑图片 - DATO OCR"),
        (crate::ocr::WINDOW, "文字识别 - DATO OCR"),
    ] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.set_title(&crate::i18n::localize(title));
        }
    }
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

fn create_toast(app: &AppHandle) {
    if app.get_webview_window(TOAST).is_some() {
        return;
    }
    let toast = builder(app, TOAST)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .inner_size(360.0, 220.0);
    match platform::build_floating(toast, FloatingKind::Panel) {
        Ok(window) => {
            let _ = window.set_ignore_cursor_events(true);
            platform::set_exclude_from_capture(&window, true);
        }
        Err(err) => tracing::warn!("创建 toast 窗口失败：{err}"),
    }
}

/// 屏幕右下角的轻提示（规格 06 §4.7）。不是系统通知：系统通知有延迟还会进通知中心。
pub fn toast(app: &AppHandle, kind: &'static str, message: impl Into<String>) {
    let message = crate::i18n::text(message.into());
    let app = app.clone();
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        crate::lifecycle::when_ready(&ui, TOAST, create_toast, move |app, window| {
            if let Some(monitor) = monitor_under_cursor() {
                let _ = place_on_monitor(&window, &monitor, 360.0, 220.0, Anchor::BottomRight);
            }
            let _ = app.emit_to(TOAST, events::TOAST, ToastPayload { kind, message });
            if let Err(err) = platform::show_without_activate(&window) {
                tracing::debug!("显示 toast 失败：{err}");
            }
            reveal_floating(app, &window);
        });
    });
}
