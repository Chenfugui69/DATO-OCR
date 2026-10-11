//! 剪贴板图片的大预览（像「快速查看」）：鼠标所在那块屏正中间一个和主窗口差不多大的窗口，
//! 图片能缩放、拖动（前端 PreviewView）。剪贴板面板里那一小块放不下，所以单开一个窗口。
//!
//! 窗口第一次用时才建，之后藏起来复用。页面可能还没加载完事件就发了，所以当前看哪一条也记在这里，
//! 页面加载完自己来取（`current`）。

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, AppResult};
use crate::platform::{self, FloatingKind};
use crate::wm;

pub const LABEL: &str = "preview";
/// 窗口大小（逻辑像素）：和主窗口（1120×720）差不多，屏幕小的话缩到可用区域的九成
const SIZE: (f64, f64) = (1040.0, 700.0);

static CURRENT: Mutex<Option<i64>> = Mutex::new(None);

pub fn current() -> Option<i64> {
    *CURRENT.lock()
}

pub fn open(app: &AppHandle, id: i64) -> AppResult<()> {
    *CURRENT.lock() = Some(id);
    let window = match app.get_webview_window(LABEL) {
        Some(w) => w,
        None => {
            let builder = wm::builder(app, LABEL)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .shadow(true)
                .inner_size(SIZE.0, SIZE.1);
            platform::build_floating(builder, FloatingKind::Panel)?
        }
    };
    let monitor = wm::monitor_under_cursor().ok_or_else(|| AppError::msg("显示器不存在"))?;
    let s = monitor.scale_factor.max(0.5);
    let w = SIZE.0.min(f64::from(monitor.work_area.width) / s * 0.9);
    let h = SIZE.1.min(f64::from(monitor.work_area.height) / s * 0.9);
    wm::place_on_monitor(&window, &monitor, w, h, wm::Anchor::Center)?;
    let _ = app.emit_to(LABEL, crate::events::CLIP_PREVIEW, id);
    window.show()?;
    let _ = window.set_focus();
    Ok(())
}

pub fn close(app: &AppHandle) {
    *CURRENT.lock() = None;
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}
