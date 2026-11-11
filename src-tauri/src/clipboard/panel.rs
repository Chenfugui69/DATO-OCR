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
/// 横向面板窗口高：面板 324（顶栏 60 + 卡片上方 8 + 卡片 232 + 下方 24，同 Paste）+ 上下透明边各 8
pub const BOTTOM_HEIGHT: f64 = 340.0;
pub const VERTICAL_SIZE: (f64, f64) = (380.0, 620.0);
/// 页面里面板四周留给阴影的透明边（和 panel.css 一致）：底部样式 8，竖版 6
const BOTTOM_PAD: f64 = 8.0;
const VERTICAL_PAD: f64 = 6.0;
/// 悬浮样式的圆角：竖版和 panel.css 的 --cn-radius-2xl 一致，横向那一条大一些（panel.css 里同样是 26，和 Paste 一样）
const RADIUS: f64 = 16.0;
const BOTTOM_RADIUS: f64 = 26.0;

static PREVIOUS: Mutex<Option<WindowHandle>> = Mutex::new(None);

/// 打开面板之前用户在用的那个窗口（粘贴回填、智能推荐看"你在做什么"都靠它）。
pub fn previous_window() -> Option<WindowHandle> {
    *PREVIOUS.lock()
}
/// 面板这次停在屏幕上的位置（物理像素）：面板开着时允许被截屏，智能推荐截"你在做什么"的窗口时要把这块涂掉
static SHOWN_AT: Mutex<Option<PhysicalRect>> = Mutex::new(None);

pub fn shown_rect() -> Option<PhysicalRect> {
    *SHOWN_AT.lock()
}
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
        // 卡片要能拖到分组标签上、拖进别的程序：Tauri 默认接管拖放（Windows 上网页里的拖放就失灵了），面板不需要接外面拖进来的文件
        .disable_drag_drop_handler()
        // 卡片（图片 / 文件）拖回面板里松手时 WebKit 会打开那个文件，页面就被导航走了：不许打开本地文件
        .on_navigation(|url| url.scheme() != "file")
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
    let radius = match (docked, cb.panel_style.as_str()) {
        (true, _) => 0.0,
        (false, "vertical") => RADIUS,
        _ => BOTTOM_RADIUS,
    };
    wm::set_glass(&window, cb.panel_blur, radius, 0.0);
}

pub fn toggle(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if app
            .get_webview_window(LABEL)
            .is_some_and(|w| w.is_visible().unwrap_or(false))
        {
            dismiss(&app);
            return;
        }
        let started = std::time::Instant::now();
        let cold = app.get_webview_window(LABEL).is_none();
        // 面板启动时就预建好、常驻（现建要两三百毫秒，快捷键按下去会顿一下）；万一不在了再现建
        crate::lifecycle::when_ready(&app, LABEL, prewarm, move |app, _| {
            if let Err(err) = show(app) {
                tracing::error!("打开剪贴板面板失败：{err}");
            }
            let elapsed_ms = started.elapsed().as_millis() as u64;
            tracing::info!(elapsed_ms, cold, "剪贴板面板可见");
        });
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
    let mut placed = None;
    // 上一次滑出去时窗口被裁成了一条看不见的缝（Windows 的滑动是边挪边裁），这次不管滑不滑都先放开
    platform::set_rect_region(&window, None);
    // 上一次筛选面板开着时被收起：窗口往上长过、背板只铺在下面一块。这次窗口重新摆，背板也恢复铺满
    platform::set_backdrop_rect(&window, None, 0);
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
            // 悬浮面板离底边的距离：平时是 BOTTOM_PAD，盖住程序坞 / 任务栏时按平台来
            let gap = if over_dock {
                platform::over_dock_gap()
            } else {
                BOTTOM_PAD
            };
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
                        wa.bottom() - (gap * s).round() as i32 - h,
                        w.max(1) as u32,
                        h.max(1) as u32,
                    )
                }
                (true, false) => {
                    wm::rect_on_monitor(&monitor, width, BOTTOM_HEIGHT, wm::Anchor::BottomFull)
                }
                // 悬浮、不开毛玻璃：页面自己在窗口里留了 BOTTOM_PAD 的边，要贴得更近就把整个窗口往下挪
                (false, false) => {
                    let mut r =
                        wm::rect_on_monitor(&monitor, width, BOTTOM_HEIGHT, wm::Anchor::BottomFull);
                    r.y += ((BOTTOM_PAD - gap) * s).round() as i32;
                    r
                }
            };
            placed = Some(rect);
            if cb.panel_animation && platform::slides_windows(blur) {
                // 先摆在滑入的起点（看不见），显示出来之后再滑上去
                let start = platform::slide_start(rect, &monitor);
                platform::set_bounds(&window, start.x, start.y, start.width, start.height)?;
                slide = Some((rect, start));
            } else {
                platform::place_window(&window, rect.x, rect.y, rect.width, rect.height)?;
            }
        }
    }
    SHOW_GEN.fetch_add(1, Ordering::SeqCst);
    *SLID.lock() = slide;
    *SHOWN_AT.lock() = placed.or_else(|| {
        let p = window.outer_position().ok()?;
        let s = window.outer_size().ok()?;
        Some(PhysicalRect::new(p.x, p.y, s.width, s.height))
    });
    let _ = app.emit_to(LABEL, events::CLIPBOARD_PANEL_SHOW, PanelShow { style });
    window.show()?;
    platform::take_focus(&window)?;
    // 面板开着的时候允许被截图（用户就是想截它）；藏起来时再排除 —— 不排除的隐藏窗口会被
    // WGC 画成一块带标题栏的白块
    platform::set_exclude_from_capture(&window, false);
    if let Some((rect, _)) = slide {
        platform::slide_window(&window, rect, SLIDE_IN_MS);
    }
    // 趁用户还在看面板，后台先把"你在做什么"读好，点 ✨ 时就不用等了
    super::smart::prefetch(app);
    Ok(())
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
    super::peek::hide(app);
    super::ghost::end(app);
    *SLID.lock() = None;
    *SHOWN_AT.lock() = None;
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

/// 把卡片拖出面板、在别的地方松手：粘贴到松手的那个位置。先收起面板，在松手的地方点一下
/// （那个窗口拿到焦点、光标落到那儿），再粘贴。`ids` 可以是多选的几张
pub fn drop_paste(app: &AppHandle, ids: &[i64]) -> AppResult<()> {
    tracing::debug!(count = ids.len(), "拖放粘贴：开始");
    let at = super::ghost::take_release_point().or_else(platform::cursor_position);
    let touch = (ids.len() == 1).then(|| ids[0]);
    let mut runs = super::paste_runs(app, ids)?;
    if runs.len() <= 1 {
        let payload = runs.pop().ok_or_else(|| crate::error::AppError::msg("没有选中的记录"))?;
        return paste_at(app, &payload, touch, at);
    }
    // 文字和图片混着：在松手的地方点一下，然后一段一段写进剪贴板、粘贴
    let ui_app = app.clone();
    app.run_on_main_thread(move || hide(&ui_app))?;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        click_drop_point(at);
        let count = runs.len();
        for (i, payload) in runs.iter().enumerate() {
            if let Err(err) = super::write_own(&app, payload, None) {
                tracing::warn!("多选粘贴：写剪贴板失败：{err}");
                break;
            }
            // 剪贴板换了内容，目标程序要一点时间才认得到
            std::thread::sleep(Duration::from_millis(40));
            if let Err(err) = platform::send_paste() {
                tracing::warn!("模拟粘贴失败：{err}");
                break;
            }
            // 等它读完这一段再换下一段，不然它读到的是下一段（图片读得慢）
            if i + 1 < count {
                std::thread::sleep(Duration::from_millis(350));
            }
        }
        tracing::info!(runs = count, "多选粘贴：文字和图片分段粘完");
    });
    Ok(())
}

/// 在松手的地方点一下，让那里的输入框拿到焦点。鼠标下面的程序不在前台时先把它叫到前台再点：
/// 面板打开时焦点在我们这儿，对方不在前台，Electron / Chromium 系程序（Claude、VS Code、浏览器）
/// 不在前台时的第一下点击只用来激活窗口、不传给网页 —— 焦点还留在它原来的输入框（比如分屏里的另一栏），
/// 粘贴就进了那里
fn click_drop_point(at: Option<(i32, i32)>) {
    let app_of = |w| platform::app_info_of(w).map(|a| (a.name, a.exe_path));
    let cursor = at.or_else(platform::cursor_position);
    let target = cursor
        .and_then(|(x, y)| platform::window_at(x, y))
        .filter(|w| !platform::is_own_window(*w));
    let front = platform::foreground_window().and_then(app_of).map(|a| a.0);
    tracing::debug!(
        ?cursor,
        target = ?target.and_then(app_of).map(|a| a.0),
        ?front,
        own_active = platform::own_app_active(),
        focus = ?target.and_then(platform::focused_element),
        "拖放粘贴：松手时"
    );
    if let Some(target) = target {
        let want = app_of(target);
        let ready = || !platform::own_app_active() && platform::foreground_window().and_then(app_of) == want;
        if !ready() {
            platform::activate_app(target);
            // 等它真的到前台、我们交出键盘（快慢不定，固定等一下有时候还没切过去，点击、粘贴就被吞掉）
            let started = std::time::Instant::now();
            while !ready() && started.elapsed() < Duration::from_millis(800) {
                std::thread::sleep(Duration::from_millis(20));
            }
            tracing::debug!(
                waited_ms = started.elapsed().as_millis() as u64,
                ready = ready(),
                "拖放粘贴：叫目标程序到前台"
            );
            // 到了之后再等一拍，让它的窗口变成接收键盘的那个
            std::thread::sleep(Duration::from_millis(80));
        }
    }
    let clicked = match cursor {
        Some((x, y)) => platform::click_at(x, y),
        None => platform::click_here(),
    };
    if let Err(err) = clicked {
        tracing::warn!("拖放粘贴：点一下失败：{err}");
    }
    // 等光标落到点的那个输入框（网页里换焦点要走一趟渲染进程）
    std::thread::sleep(Duration::from_millis(200));
    if platform::own_app_active() {
        tracing::warn!("拖放粘贴：要粘贴时键盘还在我们这儿，再把目标程序叫到前台");
        if let Some(target) = target {
            platform::activate_app(target);
            std::thread::sleep(Duration::from_millis(150));
        }
    }
    tracing::debug!(
        cursor = ?platform::cursor_position(),
        own_active = platform::own_app_active(),
        focus = ?target.and_then(platform::focused_element),
        "拖放粘贴：点完、要粘贴时"
    );
}

/// 把 `payload` 粘贴到鼠标现在的位置：收起面板，在那儿点一下，再粘贴（主窗口截图库、识字记录拖出去也走这里）
pub fn paste_at_cursor(app: &AppHandle, payload: &platform::ClipboardPayload, touch: Option<i64>) -> AppResult<()> {
    let at = super::ghost::take_release_point().or_else(platform::cursor_position);
    paste_at(app, payload, touch, at)
}

/// 粘贴到屏幕物理坐标 `at`（松手的地方）
fn paste_at(
    app: &AppHandle,
    payload: &platform::ClipboardPayload,
    touch: Option<i64>,
    at: Option<(i32, i32)>,
) -> AppResult<()> {
    super::write_own(app, payload, touch)?;
    let ui_app = app.clone();
    app.run_on_main_thread(move || hide(&ui_app))?;
    std::thread::spawn(move || {
        // 等面板藏好，点下去才落在后面的窗口上
        std::thread::sleep(Duration::from_millis(60));
        click_drop_point(at);
        if let Err(err) = platform::send_paste() {
            tracing::warn!("模拟粘贴失败：{err}");
        }
        tracing::debug!("拖放粘贴：已发出粘贴");
    });
    Ok(())
}

/// 多选复制：几条合成一份写进剪贴板（见 `payload_of_many`）
pub fn copy_many(app: &AppHandle, ids: &[i64]) -> AppResult<()> {
    let payload = super::payload_of_many(app, ids, false)?;
    let touch = (ids.len() == 1).then(|| ids[0]);
    super::write_own(app, &payload, touch)
}

/// 只复制，不粘贴（Shift+Enter / 右键菜单"复制"）。
pub fn copy(app: &AppHandle, id: i64, plain: bool) -> AppResult<()> {
    let payload = super::payload_of(app, id, plain)?;
    super::write_own(app, &payload, Some(id))
}

/// 拖卡片结束（系统原生拖拽，macOS）：`accepted` 有程序收下了，`inside` 松手时还在面板里
#[cfg(target_os = "macos")]
#[derive(Clone, serde::Serialize)]
pub struct NativeDragEnd {
    pub accepted: bool,
    pub inside: bool,
    /// 松手的位置（面板页面里的 CSS 像素）：松在分组标签上由页面自己归组
    pub x: f64,
    pub y: f64,
}

/// 用系统原生拖拽把卡片拖出面板（macOS，参照 Paste）：拖动图像由系统画、跟手；松手由目标程序自己收
/// （文字插在松手处、图片 / 文件按它的方式收），不碰剪贴板、不模拟按键。
/// 鼠标出了面板发 `CLIP_GHOST_LEFT`（面板让开）；结束发 `CLIP_NATIVE_DRAG_END`，被面板外的程序收下时
/// 先收起面板、单条的冒到最前
#[cfg(target_os = "macos")]
pub fn native_drag(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    ids: Vec<i64>,
    cards: Vec<Option<[f64; 4]>>,
    grabbed: usize,
    grab: (f64, f64),
) -> AppResult<()> {
    use platform::native_drag::{DragCard, DragEntry, DragEvents};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let st = super::state(app);
    let details = st
        .db
        .with(|c| ids.iter().map(|id| super::repo::get_detail(c, *id)).collect::<AppResult<Vec<_>>>())?;
    let mut drag_cards = Vec::new();
    let mut images = 0;
    for (n, d) in details.into_iter().enumerate() {
        let mut entries = Vec::new();
        match d.item.kind.as_str() {
            "image" => {
                if let Some(rel) = d.item.file_path.as_deref() {
                    let src = st.paths.abs(rel);
                    let file = drag_copy(&src, images).unwrap_or_else(|| src.clone());
                    images += 1;
                    let png = src
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
                        .then(|| std::fs::read(&src).ok())
                        .flatten();
                    entries.push(DragEntry::Image { file, png });
                }
            }
            "files" => entries.extend(d.item.files.iter().map(|f| DragEntry::File(f.into()))),
            kind => {
                let text = d.content_text.unwrap_or_default();
                let url = (kind == "link").then(|| text.trim().to_string());
                entries.push(DragEntry::Text {
                    text,
                    html: d.content_html,
                    rtf: d.content_rtf,
                    url,
                });
            }
        }
        let rect = cards.get(n).copied().flatten().map(|r| (r[0], r[1], r[2], r[3]));
        drag_cards.push(DragCard { entries, rect });
    }

    let label = window.label().to_string();
    let left = Arc::new(AtomicBool::new(false));
    let (moved_app, moved_label, moved_left) = (app.clone(), label.clone(), left.clone());
    let ended_app = app.clone();
    let touch = (ids.len() == 1).then(|| ids[0]);
    let (began_app, began_label) = (app.clone(), label.clone());
    let events = DragEvents {
        began: Box::new(move || {
            let _ = began_app.emit_to(began_label.as_str(), events::CLIP_NATIVE_DRAG_BEGIN, ());
        }),
        moved: Box::new(move |inside| {
            if !inside && !moved_left.swap(true, Ordering::SeqCst) {
                let _ = moved_app.emit_to(moved_label.as_str(), events::CLIP_GHOST_LEFT, ());
            }
        }),
        ended: Box::new(move |accepted, inside, (x, y)| {
            tracing::debug!(accepted, inside, "原生拖拽结束");
            if accepted && !inside {
                if let Some(id) = touch {
                    if super::state(&ended_app).db.with(|c| super::repo::touch(c, id)).is_ok() {
                        let _ = ended_app.emit(events::CLIPBOARD_CHANGED, super::ClipChanged { id, is_new: false });
                    }
                }
                hide(&ended_app);
            }
            let _ = ended_app.emit_to(label.as_str(), events::CLIP_NATIVE_DRAG_END, NativeDragEnd { accepted, inside, x, y });
        }),
    };
    platform::native_drag::start(window, drag_cards, grabbed, grab, events)
}

/// 拖出去的图片给一份好认的文件名（拖进访达 / 聊天程序时看到的名字），放在临时目录里
#[cfg(target_os = "macos")]
fn drag_copy(src: &std::path::Path, index: usize) -> Option<std::path::PathBuf> {
    let dir = std::env::temp_dir().join("DATO OCR 拖出");
    std::fs::create_dir_all(&dir).ok()?;
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("png");
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S");
    let name = if index == 0 { format!("图片 {stamp}.{ext}") } else { format!("图片 {stamp} ({}).{ext}", index + 1) };
    let dest = dir.join(name);
    std::fs::copy(src, &dest).ok()?;
    Some(dest)
}
