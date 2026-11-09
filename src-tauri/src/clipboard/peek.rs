//! 剪贴板面板里鼠标停在卡片上、按住 Shift：卡片正上方浮出一张大一点的预览（松开 Shift、鼠标移开就收）。
//!
//! 面板窗口只有卡片那么高，放不下，所以单开一个小窗口：不抢焦点（面板失焦会收起）、不挡鼠标。
//! 第一次用时才建，之后藏起来复用；页面加载完自己来取当前看哪一条（`current`）。

use parking_lot::Mutex;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, AppResult};
use crate::platform::{self, FloatingKind, PhysicalRect};
use crate::state::state;
use crate::storage::clipboard as repo;
use crate::wm;

pub const LABEL: &str = "clip-peek";
/// 预览最大多大（逻辑像素）。图片按比例缩进去，文字、文件用固定大小
const MAX: (f64, f64) = (520.0, 380.0);
const TEXT: (f64, f64) = (440.0, 280.0);
/// 和卡片之间留多少（逻辑像素）
const GAP: f64 = 10.0;

static CURRENT: Mutex<Option<i64>> = Mutex::new(None);

pub fn current() -> Option<i64> {
    *CURRENT.lock()
}

/// 卡片在面板页面里的位置（CSS 像素，相对页面左上角）
#[derive(Clone, Copy, Deserialize)]
pub struct CardRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
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
        .inner_size(TEXT.0, TEXT.1);
    let w = platform::build_floating(builder, FloatingKind::Panel)?;
    let _ = w.set_ignore_cursor_events(true);
    Ok(w)
}

/// 在面板里那张卡片的正上方显示第 `id` 条的预览。主线程上调用
pub fn show(app: &AppHandle, id: i64, card: CardRect) -> AppResult<()> {
    let panel = app
        .get_webview_window(super::panel::LABEL)
        .ok_or_else(|| AppError::msg("剪贴板面板不在"))?;
    let item = state(app).db.with(|c| repo::get_detail(c, id))?.item;
    let origin = panel.inner_position()?;
    let ps = panel.scale_factor().unwrap_or(1.0);
    // 卡片在屏幕上的位置（物理像素）
    let cx = f64::from(origin.x) + card.x * ps;
    let cy = f64::from(origin.y) + card.y * ps;
    let (cw, ch) = (card.width * ps, card.height * ps);
    let monitor = wm::monitor_at((cx + cw / 2.0) as i32, (cy + ch / 2.0) as i32)
        .ok_or_else(|| AppError::msg("显示器不存在"))?;
    let s = monitor.scale_factor.max(0.5);
    let wa = monitor.work_area;
    // 设置里的大小：小 = 原来那么大，中、大依次放大；再大也不超过屏幕可用区域的八成五
    let zoom = match state(app).settings.read().clipboard.peek_size.as_str() {
        "small" => 1.0,
        "large" => 2.0,
        _ => 1.45,
    };
    let room = (f64::from(wa.width) / s * 0.85, (cy - f64::from(wa.y)) / s - GAP * 2.0);
    let fit = |w: f64, h: f64| {
        let k = (room.0 / w).min(room.1.max(f64::from(wa.height) / s * 0.5) / h).min(1.0);
        (w * k, h * k)
    };
    let (max, text) = ((MAX.0 * zoom, MAX.1 * zoom), (TEXT.0 * zoom, TEXT.1 * zoom));
    // 窗口大小：图片按比例放进 max 里（不放大），别的固定大小
    let (lw, lh) = match (item.kind.as_str(), item.width, item.height) {
        ("image", Some(w), Some(h)) if w > 0 && h > 0 => {
            let (w, h) = (w as f64 / s, h as f64 / s);
            let k = (max.0 / w).min(max.1 / h).min(1.0);
            fit((w * k).max(160.0), (h * k).max(120.0))
        }
        ("image", ..) => fit(max.0, max.1),
        _ => fit(text.0, text.1),
    };
    // 卡片上方放不下（竖版面板在屏幕上沿）就放到旁边
    let (pw, ph) = (lw * s, lh * s);
    let gap = GAP * s;
    let above = cy - gap - ph >= f64::from(wa.y);
    let (x, y) = if above {
        (cx + cw / 2.0 - pw / 2.0, cy - gap - ph)
    } else if cx + cw + gap + pw <= f64::from(wa.right()) {
        (cx + cw + gap, cy)
    } else {
        (cx - gap - pw, cy)
    };
    let x = (x.round() as i32).clamp(wa.x, (wa.right() - pw as i32).max(wa.x));
    let y = (y.round() as i32).clamp(wa.y, (wa.bottom() - ph as i32).max(wa.y));
    let rect = PhysicalRect::new(x, y, pw.round() as u32, ph.round() as u32);

    *CURRENT.lock() = Some(id);
    let w = window(app)?;
    platform::place_window(&w, rect.x, rect.y, rect.width, rect.height)?;
    let _ = app.emit_to(LABEL, crate::events::CLIP_PEEK, id);
    platform::show_without_activate(&w)?;
    Ok(())
}

pub fn hide(app: &AppHandle) {
    if CURRENT.lock().take().is_none() {
        return;
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = app.emit_to(LABEL, crate::events::CLIP_PEEK, None::<i64>);
        let _ = w.hide();
    }
}
