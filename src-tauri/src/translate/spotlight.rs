//! 输入翻译：像 macOS 的聚焦搜索那样，屏幕中间偏上弹出一条输入框，边打字下面边出各个源的译文。
//!
//! 单独一个窗口（不借用划词翻译的气泡：那个是跟着选区摆的，样子和用法都对不上）。窗口宽度固定，
//! 高度由页面按内容量好了自己改（`SpotlightView` 里的 ResizeObserver），顶边不动、往下长。
//! 点别处（失焦）或按 Esc 收起；第一次用时才建，之后藏起来复用。

use tauri::{AppHandle, Emitter};

use crate::platform::{self, FloatingKind};
use crate::{lifecycle, wm};

pub const WINDOW: &str = "spotlight";
/// 窗口宽度（逻辑像素，含四周给阴影留的边，和页面的 PAD 一致）
pub const WIDTH: f64 = 720.0;
/// 刚弹出来时只有输入框那一条的高度（含阴影边）
const HEIGHT: f64 = 120.0;
/// 输入框的顶边在屏幕可用区域从上往下多少（比例）
const TOP_RATIO: f64 = 0.22;

fn create(app: &AppHandle) {
    let builder = wm::builder(app, WINDOW)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .inner_size(WIDTH, HEIGHT);
    if let Err(err) = platform::build_floating(builder, FloatingKind::Panel) {
        tracing::warn!("创建输入翻译窗口失败：{err}");
    }
}

/// 弹出（已经开着就再拿一次焦点、选中输入框里的字）
pub fn show(app: &AppHandle) {
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        lifecycle::when_ready(&ui, WINDOW, create, |app, window| {
            if let Some(m) = wm::monitor_under_cursor() {
                let s = m.scale_factor.max(0.5);
                let wa = m.work_area;
                let w = (WIDTH * s).round() as i32;
                let x = wa.x + (wa.width as i32 - w) / 2;
                let y = wa.y + (f64::from(wa.height) * TOP_RATIO).round() as i32;
                // 高度先按一条输入框；页面量好内容后自己往下长
                let _ = platform::set_bounds(&window, x, y, w as u32, (HEIGHT * s).round() as u32);
            }
            let _ = window.show();
            let _ = platform::take_focus(&window);
            let _ = app.emit_to(WINDOW, "spotlight-show", ());
        });
    });
}
