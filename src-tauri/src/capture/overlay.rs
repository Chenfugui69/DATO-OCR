//! 截图遮罩窗口的生命周期。
//!
//! 每块显示器一个窗口，标签 `capture-{monitor_id}`。遮罩是**透明的，只画压暗层、选区
//! 和工具条**；冻结的屏幕画面在它正下方的原生底图窗口里（`platform::backdrop`）。
//!
//! 遮罩窗口必须**启动时建好并隐藏**，热键路径上只 show：WebView2 建一个窗口要
//! 400–1000ms，放在热键之后 150ms 的预算直接爆掉。

use tauri::{AppHandle, Manager, WebviewWindow};

use crate::error::{AppError, AppResult};
use crate::platform::{self, FloatingKind, MonitorId, MonitorInfo};
use crate::wm;

pub const LABEL_PREFIX: &str = "capture-";

pub fn label_for(monitor: MonitorId) -> String {
    format!("{LABEL_PREFIX}{}", monitor.0)
}

/// 启动时预建所有遮罩窗口和底图窗口。**必须在 UI 线程调用。**
pub fn prewarm(app: &AppHandle) {
    let started = std::time::Instant::now();
    let monitors = match platform::list_monitors() {
        Ok(m) => m,
        Err(err) => {
            tracing::warn!("预建遮罩时枚举显示器失败：{err}");
            return;
        }
    };
    for info in &monitors {
        if let Err(err) = ensure_one(app, info) {
            tracing::warn!(monitor = %info.id, "预建遮罩窗口失败：{err}");
        }
        if let Err(err) = platform::backdrop::ensure(info.id) {
            tracing::warn!(monitor = %info.id, "预建底图窗口失败：{err}");
        }
    }
    tracing::info!(
        monitors = monitors.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "遮罩窗口预建完成"
    );
}

/// 热键路径：把窗口摆到当前显示器几何上，清掉已拔掉的显示器的窗口。**UI 线程。**
pub fn arrange(app: &AppHandle, monitors: &[MonitorInfo]) -> AppResult<()> {
    for (label, window) in app.webview_windows() {
        if label.starts_with(LABEL_PREFIX) && !monitors.iter().any(|m| label_for(m.id) == label) {
            let _ = window.destroy();
        }
    }
    let ids: Vec<MonitorId> = monitors.iter().map(|m| m.id).collect();
    platform::backdrop::retain(&ids);

    let mut usable = 0;
    for info in monitors {
        match ensure_one(app, info) {
            Ok(_) => usable += 1,
            Err(err) => tracing::error!(monitor = %info.id, "准备遮罩窗口失败：{err}"),
        }
    }
    if usable == 0 {
        return Err(AppError::msg("没有可用的遮罩窗口"));
    }
    Ok(())
}

fn ensure_one(app: &AppHandle, info: &MonitorInfo) -> AppResult<WebviewWindow> {
    let label = label_for(info.id);
    if let Some(existing) = app.get_webview_window(&label) {
        place(&existing, info)?;
        return Ok(existing);
    }
    // 构建器只收逻辑单位，这里给的是"落在目标屏、尺寸大致对"的提示：窗口一出生就在
    // 目标屏上，WebView2 才能在建立时拿到该屏 DPI，少一次 WM_DPICHANGED 重排。
    // 精确定位交给下面 `place` 的物理像素 API。
    let s = if info.scale_factor > 0.0 {
        info.scale_factor
    } else {
        1.0
    };
    let overlay = wm::builder(app, &label)
        .position(f64::from(info.bounds.x) / s, f64::from(info.bounds.y) / s)
        .inner_size(
            f64::from(info.bounds.width) / s,
            f64::from(info.bounds.height) / s,
        )
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .shadow(false)
        // 焦点在 show 时再抢。预建阶段抢焦点会把用户正在打字的窗口顶掉
        .focused(false);
    let window = platform::build_floating(overlay, FloatingKind::Overlay)?;
    place(&window, info)?;
    platform::set_exclude_from_capture(&window, true);
    Ok(window)
}

/// 用物理像素精确摆放，避免逻辑换算带来的 1px 错位和模糊。
fn place(window: &WebviewWindow, info: &MonitorInfo) -> AppResult<()> {
    let b = info.bounds;
    platform::place_window(window, b.x, b.y, b.width, b.height)
}

/// 底图与遮罩同一帧显示。**UI 线程。**
///
/// 批次之后还要补一次 Tauri 的 `show()`：tao 自己记着一份可见性标志，原生
/// `SWP_SHOWWINDOW` 绕过了它，不同步的话 `set_focus()` 会以为窗口隐藏着而什么都不做
/// —— 表现是遮罩在屏幕上，Esc 和 Enter 却全没反应。顺序不能反，放到批次前会真的
/// 把遮罩提前显示出来，原子性就白做了。
pub fn show_with_backdrop(app: &AppHandle, monitor: MonitorId, focus: bool) -> AppResult<()> {
    let label = label_for(monitor);
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| AppError::msg(format!("遮罩窗口 {label} 不存在")))?;
    let handle = platform::native_handle(&window)?;
    platform::backdrop::show_below(monitor, handle)?;
    window.show()?;
    platform::reveal_for_tests(&window, true);
    if focus {
        platform::take_focus(&window)?;
    }
    Ok(())
}

/// 把键盘焦点还给这块屏的遮罩。
pub fn refocus(app: &AppHandle, monitor: MonitorId) {
    if let Some(window) = app.get_webview_window(&label_for(monitor)) {
        if let Err(err) = platform::take_focus(&window) {
            tracing::debug!(%monitor, "还焦点给遮罩失败：{err}");
        }
    }
}

/// 只显示遮罩（长截图模式：底图隐藏，露出真实桌面）。
pub fn for_each(app: &AppHandle, mut f: impl FnMut(&WebviewWindow)) {
    for (label, window) in app.webview_windows() {
        if label.starts_with(LABEL_PREFIX) {
            f(&window);
        }
    }
}

/// 收工：藏起所有遮罩和底图。刻意 hide 不 close，窗口留给下一次热键复用。
pub fn hide_all(app: &AppHandle) {
    for_each(app, |w| {
        let _ = w.set_ignore_cursor_events(false);
        let _ = w.hide();
        platform::reveal_for_tests(w, false);
    });
    platform::backdrop::release_all();
}
