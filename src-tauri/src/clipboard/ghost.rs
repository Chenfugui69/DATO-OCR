//! 拖剪贴板卡片时跟着鼠标走的"影子"：一张（多选时是一叠）卡片，拖出面板窗口也看得见。
//!
//! 页面里画的东西出不了窗口，所以单开一个透明、不挡鼠标、不抢焦点的小窗口，拖动期间后台线程每 8ms
//! 按鼠标位置挪它一次（页面那边拖到窗口外面时也收得到移动，但每次移动都过一趟 IPC 不如这里直接跟）。
//! 第一次用时才建，面板打开时会先建好（`prepare`），之后藏起来复用。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition};

use crate::error::AppResult;
use crate::platform::{self, FloatingKind};
use crate::wm;

pub const LABEL: &str = "clip-ghost";
/// 窗口大小（逻辑像素）：一张卡片（232×232）四周留出叠放和投影的地方（投影要整个落在窗口里，
/// 不然被窗口边切出一道直边）
pub const SIZE: (f64, f64) = (232.0 + 40.0 + 60.0, 232.0 + 40.0 + 64.0);
/// 卡片在窗口里离左上角多远（逻辑像素，和页面 ghost.css 的 --pad 一致）
const PAD: f64 = 40.0;

/// 第几次拖：结束时加一，跟随线程看到变了就停
static GEN: AtomicU64 = AtomicU64::new(0);
/// 最近一次拖动松手时鼠标在哪（物理像素）、什么时候。拖放粘贴要点在这里，而不是点的那一刻
/// 鼠标在的地方：松手后到点下去有零点几秒，这期间用户顺手一挪鼠标就点偏了，粘贴落了空
static RELEASED_AT: parking_lot::Mutex<Option<((i32, i32), std::time::Instant)>> = parking_lot::Mutex::new(None);

/// 刚才那次拖动松手的位置（几秒之内的才算）
pub fn take_release_point() -> Option<(i32, i32)> {
    RELEASED_AT
        .lock()
        .take()
        .filter(|(_, at)| at.elapsed() < Duration::from_secs(3))
        .map(|(p, _)| p)
}

fn window(app: &AppHandle) -> AppResult<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window(LABEL) {
        return Ok(w);
    }
    let builder = wm::builder(app, LABEL)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .focused(false)
        .inner_size(SIZE.0, SIZE.1);
    let w = platform::build_floating(builder, FloatingKind::Panel)?;
    let _ = w.set_ignore_cursor_events(true);
    platform::set_exclude_from_capture(&w, true);
    Ok(w)
}

/// 先把窗口建好（面板打开时），第一次拖的时候影子不用等窗口加载。主线程上调用
pub fn prepare(app: &AppHandle) {
    if let Err(err) = window(app) {
        tracing::debug!("拖动影子窗口建不出来：{err}");
    }
}

/// 窗口左上角该在哪（物理像素）：鼠标按在卡片上 `grab`（逻辑像素）那个点，影子里的卡片也是那个点在鼠标下
fn origin_for(cursor: (i32, i32), grab: (f64, f64)) -> PhysicalPosition<i32> {
    let s = wm::monitor_at(cursor.0, cursor.1).map_or(1.0, |m| m.scale_factor.max(0.5));
    PhysicalPosition::new(
        cursor.0 - ((PAD + grab.0) * s).round() as i32,
        cursor.1 - ((PAD + grab.1) * s).round() as i32,
    )
}

/// 开始拖：`items` 是拖着的那几条（页面直接拿来画），`grab` 是鼠标按在卡片上的位置，`origin` 是开始拖的
/// 那个窗口（松手时告诉它）。主线程上调用
pub fn start(app: &AppHandle, items: Value, grab: (f64, f64), origin: String) -> AppResult<()> {
    let w = window(app)?;
    RELEASED_AT.lock().take();
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(c) = platform::cursor_position() {
        let _ = w.set_position(origin_for(c, grab));
    }
    // 鼠标按在卡片上的位置一起给页面：缩小（经过输入框时）以那个点为中心，卡片缩向鼠标
    let _ = app.emit_to(LABEL, crate::events::CLIP_GHOST, serde_json::json!({ "items": items, "grab": [grab.0, grab.1] }));
    // 藏着时排除在抓屏之外，拖动时放开：远程控制看到的是抓屏画面，排除着的话对面看不到影子卡片
    // （和剪贴板面板一样，见 `panel.rs`）
    platform::set_exclude_from_capture(&w, false);
    platform::show_without_activate(&w)?;
    let follow_app = app.clone();
    // 开始拖的那个窗口占的地方（物理像素）。拖动期间它不动
    let origin_rect = app.get_webview_window(&origin).and_then(|o| {
        let p = o.outer_position().ok()?;
        let s = o.outer_size().ok()?;
        Some(platform::PhysicalRect::new(p.x, p.y, s.width, s.height))
    });
    std::thread::spawn(move || {
        let mut last = None;
        let mut released = false;
        let mut left = false;
        while GEN.load(Ordering::SeqCst) == gen {
            let cursor = platform::cursor_position();
            let outside = match (cursor, origin_rect) {
                (Some((x, y)), Some(r)) => !r.contains_point(x, y),
                _ => false,
            };
            // 鼠标出了开始拖的窗口：告诉页面（面板滑走让开）。macOS 上鼠标拖出窗口后页面收不到移动，
            // 自己发现不了（Windows 上靠鼠标捕获收得到，两边都发也不碍事，页面只认第一次）
            if outside && !left {
                left = true;
                let _ = follow_app.emit_to(origin.as_str(), crate::events::CLIP_GHOST_LEFT, ());
            }
            // 松手了：同样告诉页面，带上是不是松在窗口外面（穿透的窗口连松开都收不到）
            if !released && platform::mouse_buttons() & 1 == 0 {
                released = true;
                if let Some(c) = cursor {
                    *RELEASED_AT.lock() = Some((c, std::time::Instant::now()));
                }
                let _ = follow_app.emit_to(origin.as_str(), crate::events::CLIP_GHOST_RELEASED, outside);
            }
            if let Some(c) = cursor {
                if last != Some(c) {
                    last = Some(c);
                    let _ = w.set_position(origin_for(c, grab));
                }
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    });
    // 另一条线程隔一会儿问一下鼠标下面是不是输入框：是的话影子缩小，表示松手就贴在这儿
    // （跨进程问 UIA 要几毫秒到几十毫秒，不能放在跟鼠标的那条线程里）
    let app = app.clone();
    std::thread::spawn(move || {
        let mut over = false;
        let mut last: Option<((i32, i32), std::time::Instant)> = None;
        while GEN.load(Ordering::SeqCst) == gen {
            std::thread::sleep(Duration::from_millis(90));
            let Some(c) = platform::cursor_position() else { continue };
            // 鼠标停着不动也隔一会儿再问一次：浏览器类程序头一次被问时无障碍树还没建好，
            // 只问一次就一直当成"不是输入框"
            if last.is_some_and(|(p, t)| p == c && t.elapsed() < Duration::from_millis(300)) {
                continue;
            }
            last = Some((c, std::time::Instant::now()));
            let now = platform::editable_at(c.0, c.1).unwrap_or(false);
            if now != over && GEN.load(Ordering::SeqCst) == gen {
                over = now;
                let _ = app.emit_to(LABEL, crate::events::CLIP_GHOST_TARGET, over);
            }
        }
    });
    Ok(())
}

/// 拖完了（松手、取消）：影子收起
pub fn end(app: &AppHandle) {
    GEN.fetch_add(1, Ordering::SeqCst);
    {
        // 页面先收到了松开（Windows 上常这样），跟随线程还没来得及记：这时候的鼠标位置就是松手的地方
        let mut released = RELEASED_AT.lock();
        if released.is_none() {
            *released = platform::cursor_position().map(|c| (c, std::time::Instant::now()));
        }
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = app.emit_to(LABEL, crate::events::CLIP_GHOST, serde_json::json!({ "items": [], "grab": [0, 0] }));
        let _ = app.emit_to(LABEL, crate::events::CLIP_GHOST_TARGET, false);
        let _ = w.hide();
        platform::set_exclude_from_capture(&w, true);
    }
}
