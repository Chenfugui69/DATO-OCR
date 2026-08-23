//! 截图相关 command。

use tauri::AppHandle;

use crate::capture::{
    self, Bounds, CaptureAction, CaptureFinishResult, CapturePrepareResult, CaptureMode,
};
use crate::error::AppResult;
use crate::platform::MonitorId;

/// 返回当前会话的显示器元信息。
///
/// 抓屏本身由热键回调在 Rust 侧完成（见 `capture` 模块的模块级说明），这里是
/// 幂等的读操作。没有进行中的会话时返回 `no_capture_session`。
#[tauri::command]
pub fn capture_prepare(app: AppHandle) -> AppResult<CapturePrepareResult> {
    capture::prepare_result(&app)
}

/// 遮罩前端已挂载、事件监听已注册，可以接会话通知了。
///
/// 遮罩窗口是启动时预建并隐藏的，Rust 侧无从知道里面的 WebView 加载到哪一步。
/// 没有这个信号的话，热键来得比前端就绪早会导致整次截图静默卡死。
#[tauri::command]
pub fn capture_overlay_boot(app: AppHandle, monitor_id: u64) {
    capture::overlay_boot(&app, MonitorId(monitor_id));
}

/// 遮罩前端已经把压暗层放进 DOM，请求让画面出来。
///
/// 底图在原生层，所以这里**不等**任何像素传输。`prepare_ms` 是前端自量的
/// "收到会话通知 → `capture_prepare` 返回"，只用来写日志：热键到上屏的总时间
/// 必须由 Rust 侧计（前端拿不到按键那一刻的时间戳）。
#[tauri::command]
pub fn capture_overlay_ready(
    app: AppHandle,
    monitor_id: u64,
    prepare_ms: Option<f64>,
) -> AppResult<()> {
    capture::overlay_ready(&app, MonitorId(monitor_id), prepare_ms)
}

/// 底图像素已经送到 WebView 并解码完毕。
///
/// 纯埋点。这条路不在关键路径上，但它决定放大镜/取色/马赛克何时可用。
#[tauri::command]
pub fn capture_overlay_pixels_ready(
    app: AppHandle,
    monitor_id: u64,
    timings: capture::PixelTimings,
) {
    capture::overlay_pixels_ready(&app, MonitorId(monitor_id), timings);
}

#[tauri::command]
pub fn capture_finish(
    app: AppHandle,
    bounds: Bounds,
    action: CaptureAction,
) -> AppResult<CaptureFinishResult> {
    capture::finish(&app, bounds, action)
}

#[tauri::command]
pub fn capture_cancel(app: AppHandle) {
    capture::cancel(&app);
}

/// 手动触发一次截图，等价于按热键。M0 用它在主窗口上做不依赖热键的验证。
#[tauri::command]
pub fn capture_trigger(app: AppHandle, mode: Option<CaptureMode>) -> AppResult<()> {
    capture::start(&app, mode.unwrap_or(CaptureMode::Normal))
}
