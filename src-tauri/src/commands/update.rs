//! 检查更新命令。

use tauri::AppHandle;

use crate::error::AppResult;
use crate::update::{self, UpdateStatus};

#[tauri::command]
pub async fn update_status(app: AppHandle) -> AppResult<UpdateStatus> {
    Ok(update::status(&app))
}

/// 手动检查
#[tauri::command]
pub async fn update_check(app: AppHandle) -> AppResult<UpdateStatus> {
    update::check(&app, true).await
}

/// 下载、验签、安装（Windows 上随后退出，装完自动重新打开）
#[tauri::command]
pub async fn update_install(app: AppHandle) -> AppResult<()> {
    update::install(&app).await
}

/// "不显示更新提示"：这个版本不再提示
#[tauri::command]
pub async fn update_skip(app: AppHandle, version: String) -> AppResult<()> {
    update::skip(&app, &version)
}
