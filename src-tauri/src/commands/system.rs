//! 系统信息相关 command。

/// 把前端的错误写进 Rust 日志。
///
/// 遮罩窗口是隐藏的、也没有 devtools，里面抛出来的异常在界面上完全不可见 ——
/// 表现只是"按 F1 没反应"。没有这条通道就只能靠改代码二分查找。
///
/// 只记消息文本，不记任何用户内容（规格 00 §6.3）。
#[tauri::command]
pub fn report_error(scope: String, message: String) {
    tracing::error!(scope = %scope, "前端报错: {message}");
}


use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};
use crate::platform::{self, MonitorInfo};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemDiagnostics {
    pub app_version: String,
    pub data_dir: String,
    pub dark_mode: bool,
    pub transparency_enabled: bool,
    pub monitors: Vec<MonitorInfo>,
}

/// M0 验证页用的自检信息：显示器几何、缩放比例、数据目录。
///
/// 双屏混合 DPI 环境下最先要确认的就是这里的数字对不对 —— 如果 `scaleFactor`
/// 全是 1.0 或者坐标没有负值，说明 DPI 声明没生效。
#[tauri::command]
pub fn get_system_diagnostics(app: AppHandle) -> AppResult<SystemDiagnostics> {
    let state = app.state::<AppState>();

    Ok(SystemDiagnostics {
        app_version: app.package_info().version.to_string(),
        data_dir: state.paths.root().display().to_string(),
        dark_mode: platform::system_info().is_dark_mode(),
        transparency_enabled: platform::system_info().is_transparency_enabled(),
        monitors: platform::screen_capture().list_monitors()?,
    })
}

#[tauri::command]
pub fn open_data_folder(app: AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;

    let state = app.state::<AppState>();
    let path = state.paths.root().display().to_string();

    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|err| AppError::Io(format!("打开数据目录失败: {err}")))
}
