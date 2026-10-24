//! 自己画的右键菜单（贴图用）：和程序别的地方一样的样子（图标、快捷键、深浅色），不是系统那种灰色的菜单。
//!
//! 贴图窗口只有图那么大，菜单画在贴图里会被裁掉，所以菜单是一个单独的小浮层窗口：
//! 贴图把菜单项交过来（`popmenu_open`），菜单窗口在鼠标那里弹出、量好自己多大再摆（`popmenu_place`，
//! 靠屏幕边时往里收 / 往上翻），点了哪项把编号发回给贴图（`popmenu-select` 事件），点别处或按 Esc 就收起。
//! 菜单项的内容 Rust 不管，原样转给菜单页面。

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppResult;
use crate::platform::{self, FloatingKind};
use crate::wm;

pub const WINDOW: &str = "popmenu";

/// 菜单项、谁要的菜单、在哪弹（屏幕物理坐标）
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub owner: String,
    pub items: serde_json::Value,
    #[serde(skip)]
    at: (i32, i32),
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

fn ensure(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window(WINDOW) {
        return Some(w);
    }
    let builder = wm::builder(app, WINDOW)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .inner_size(260.0, 360.0);
    match platform::build_floating(builder, FloatingKind::Panel) {
        Ok(w) => Some(w),
        Err(err) => {
            tracing::warn!("创建菜单窗口失败：{err}");
            None
        }
    }
}

/// 弹出菜单：`owner` 是要菜单的窗口（选了哪项发给它）
pub fn open(app: &AppHandle, owner: String, items: serde_json::Value) -> AppResult<()> {
    let at = platform::cursor_position().unwrap_or((0, 0));
    let pending = Pending { owner, items, at };
    *PENDING.lock() = Some(pending.clone());
    let ui = app.clone();
    app.run_on_main_thread(move || {
        if let Some(w) = ensure(&ui) {
            // 页面已经开着：直接把新的菜单项给它；刚建的窗口页面加载好后自己来取
            let _ = w.emit_to(WINDOW, "popmenu-items", pending);
        }
    })?;
    Ok(())
}

/// 菜单页面加载好了来取菜单项
pub fn items() -> Option<Pending> {
    PENDING.lock().clone()
}

/// 页面量好了自己多大（CSS 像素）：在鼠标那里摆好、显示、拿焦点（点别处时失焦收起）
pub fn place(app: &AppHandle, width: f64, height: f64) -> AppResult<()> {
    let Some((x, y)) = PENDING.lock().as_ref().map(|p| p.at) else {
        return Ok(());
    };
    let ui = app.clone();
    app.run_on_main_thread(move || {
        let Some(w) = ui.get_webview_window(WINDOW) else { return };
        let monitor = wm::monitor_at(x, y);
        let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor);
        let (pw, ph) = ((width * scale).ceil() as i32, (height * scale).ceil() as i32);
        let area = monitor.map(|m| m.work_area);
        // 默认在鼠标右下；右边放不下往左翻，下面放不下往上翻
        let (mut left, mut top) = (x, y);
        if let Some(a) = area {
            if left + pw > a.right() {
                left = (x - pw).max(a.x);
            }
            if top + ph > a.bottom() {
                top = (y - ph).max(a.y);
            }
        }
        let _ = platform::set_bounds(&w, left, top, pw as u32, ph as u32);
        let _ = w.show();
        let _ = platform::take_focus(&w);
    })?;
    Ok(())
}

/// 选了一项：收起菜单，焦点还给要菜单的窗口（贴图里的表格：选完接着用键盘操作，选区也要画出来），
/// 再告诉它选了哪项。点别处收起的不走这里，不抢焦点
pub fn pick(app: &AppHandle, id: String) {
    let owner = PENDING.lock().take().map(|p| p.owner);
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        hide(&ui);
        let Some(owner) = owner else { return };
        if let Some(w) = ui.get_webview_window(&owner) {
            let _ = platform::take_focus(&w);
        }
        let _ = ui.emit_to(owner.as_str(), "popmenu-select", id);
    });
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.hide();
    }
}
