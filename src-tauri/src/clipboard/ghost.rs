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
/// 窗口大小（逻辑像素）：一张卡片（236×240）四周留出叠放和投影的地方（投影要整个落在窗口里，
/// 不然被窗口边切出一道直边）
pub const SIZE: (f64, f64) = (236.0 + 40.0 + 60.0, 240.0 + 40.0 + 64.0);
/// 卡片在窗口里离左上角多远（逻辑像素，和页面 ghost.css 的 --pad 一致）
const PAD: f64 = 40.0;

/// 第几次拖：结束时加一，跟随线程看到变了就停
static GEN: AtomicU64 = AtomicU64::new(0);

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

/// 开始拖：`items` 是拖着的那几条（页面直接拿来画），`grab` 是鼠标按在卡片上的位置。主线程上调用
pub fn start(app: &AppHandle, items: Value, grab: (f64, f64)) -> AppResult<()> {
    let w = window(app)?;
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(c) = platform::cursor_position() {
        let _ = w.set_position(origin_for(c, grab));
    }
    // 鼠标按在卡片上的位置一起给页面：缩小（经过输入框时）以那个点为中心，卡片缩向鼠标
    let _ = app.emit_to(LABEL, crate::events::CLIP_GHOST, serde_json::json!({ "items": items, "grab": [grab.0, grab.1] }));
    platform::show_without_activate(&w)?;
    std::thread::spawn(move || {
        let mut last = None;
        while GEN.load(Ordering::SeqCst) == gen {
            if let Some(c) = platform::cursor_position() {
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
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = app.emit_to(LABEL, crate::events::CLIP_GHOST, serde_json::json!({ "items": [], "grab": [0, 0] }));
        let _ = app.emit_to(LABEL, crate::events::CLIP_GHOST_TARGET, false);
        let _ = w.hide();
    }
}
