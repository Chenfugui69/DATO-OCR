//! 多端同步命令（规格 09）。

use tauri::AppHandle;

use super::blocking;
use crate::error::AppResult;
use crate::sync::{self, webdav::DavDevice, SyncStatus};

#[tauri::command]
pub async fn sync_status(app: AppHandle) -> AppResult<SyncStatus> {
    blocking(move || sync::status(&app)).await
}

/// 同意 / 拒绝一个加入申请
#[tauri::command]
pub async fn sync_approve(app: AppHandle, id: String, approve: bool) -> AppResult<()> {
    blocking(move || sync::approve(&app, &id, approve)).await
}

/// 移除一台设备（加入了本机的成员，或者本机加入的主机）
#[tauri::command]
pub async fn sync_remove_peer(app: AppHandle, device_id: String) -> AppResult<()> {
    blocking(move || sync::remove_peer(&app, &device_id)).await
}

#[tauri::command]
pub async fn sync_join(app: AppHandle, code: String, address: Option<String>) -> AppResult<()> {
    sync::join(&app, code, address)
}

#[tauri::command]
pub async fn sync_cancel_join(app: AppHandle) -> AppResult<()> {
    sync::cancel_join(&app);
    Ok(())
}

#[tauri::command]
pub async fn sync_leave(app: AppHandle) -> AppResult<()> {
    blocking(move || sync::leave(&app)).await
}

#[tauri::command]
pub async fn sync_regenerate_code(app: AppHandle) -> AppResult<String> {
    blocking(move || sync::regenerate_code(&app)).await
}

#[tauri::command]
pub async fn sync_qr(text: String) -> AppResult<String> {
    sync::qr_svg(&text)
}

#[tauri::command]
pub async fn sync_webdav_connect(
    app: AppHandle,
    url: String,
    user: String,
    password: String,
    folder: String,
    sync_password: String,
) -> AppResult<()> {
    sync::webdav_connect(&app, url, user, password, folder, sync_password).await
}

#[tauri::command]
pub async fn sync_webdav_disconnect(app: AppHandle) -> AppResult<()> {
    blocking(move || sync::webdav_disconnect(&app)).await
}

#[tauri::command]
pub async fn sync_webdav_now(app: AppHandle) -> AppResult<()> {
    sync::webdav_now(&app);
    Ok(())
}

#[tauri::command]
pub async fn sync_webdav_devices(app: AppHandle) -> AppResult<Vec<DavDevice>> {
    sync::webdav_devices(&app).await
}
