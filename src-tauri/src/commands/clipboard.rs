//! 剪贴板历史命令。

use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use super::blocking;
use crate::clipboard::{self, panel};
use crate::error::{AppError, AppResult};
use crate::state::state;
use crate::storage::clipboard::{
    self as repo, ClipDetail, ClipGroup, ClipPage, ClipQuery, ClipStats,
};

#[tauri::command]
pub async fn clipboard_query(app: AppHandle, query: ClipQuery) -> AppResult<ClipPage> {
    state(&app).db.with(|c| repo::query(c, &query))
}

#[tauri::command]
pub async fn clipboard_get(app: AppHandle, id: i64) -> AppResult<ClipDetail> {
    state(&app).db.with(|c| repo::get_detail(c, id))
}

/// GIF 卡片的预览：返回数据目录里那份副本的相对路径，早先的记录没有就现在补一份。
#[tauri::command]
pub async fn clipboard_gif_preview(app: AppHandle, id: i64) -> AppResult<Option<String>> {
    tokio::task::spawn_blocking(move || crate::clipboard::ensure_gif_preview(&app, id))
        .await
        .map_err(|e| crate::error::AppError::msg(e.to_string()))?
}

#[tauri::command]
pub async fn clipboard_stats(app: AppHandle) -> AppResult<ClipStats> {
    state(&app).db.with(|c| repo::stats(c))
}

#[tauri::command]
pub async fn clipboard_paste(app: AppHandle, id: i64, plain: bool) -> AppResult<()> {
    blocking(move || panel::paste(&app, id, plain)).await
}

#[tauri::command]
pub async fn clipboard_copy(app: AppHandle, id: i64, plain: bool) -> AppResult<()> {
    blocking(move || panel::copy(&app, id, plain)).await
}

#[tauri::command]
pub async fn clipboard_delete(app: AppHandle, ids: Vec<i64>) -> AppResult<()> {
    blocking(move || clipboard::delete(&app, &ids)).await
}

#[tauri::command]
pub async fn clipboard_set_pinned(app: AppHandle, id: i64, value: bool) -> AppResult<()> {
    state(&app)
        .db
        .with(|c| repo::set_flag(c, id, "pinned", value))
}

#[tauri::command]
pub async fn clipboard_set_favorite(app: AppHandle, id: i64, value: bool) -> AppResult<()> {
    state(&app)
        .db
        .with(|c| repo::set_flag(c, id, "favorite", value))
}

#[tauri::command]
pub async fn clipboard_set_note(app: AppHandle, id: i64, note: Option<String>) -> AppResult<()> {
    state(&app)
        .db
        .with(|c| repo::set_note(c, id, note.as_deref()))
}

#[tauri::command]
pub async fn clipboard_set_group(
    app: AppHandle,
    ids: Vec<i64>,
    group_id: Option<i64>,
) -> AppResult<()> {
    state(&app).db.with(|c| repo::set_group(c, &ids, group_id))
}

#[tauri::command]
pub async fn clipboard_groups(app: AppHandle) -> AppResult<Vec<ClipGroup>> {
    state(&app).db.with(|c| repo::list_groups(c))
}

#[tauri::command]
pub async fn clipboard_create_group(app: AppHandle, name: String) -> AppResult<i64> {
    state(&app).db.with(|c| repo::create_group(c, &name))
}

#[tauri::command]
pub async fn clipboard_rename_group(app: AppHandle, id: i64, name: String) -> AppResult<()> {
    state(&app).db.with(|c| repo::rename_group(c, id, &name))
}

#[tauri::command]
pub async fn clipboard_delete_group(app: AppHandle, id: i64) -> AppResult<()> {
    state(&app).db.with(|c| repo::delete_group(c, id))
}

/// scope：`unpinned` = 保留置顶/收藏/有备注/已分组的；`everything` = 全部清空
#[tauri::command]
pub async fn clipboard_clear(app: AppHandle, scope: String) -> AppResult<usize> {
    blocking(move || {
        let ids = state(&app).db.with(|c| repo::ids_for_clear(c, &scope))?;
        let n = ids.len();
        clipboard::delete(&app, &ids)?;
        Ok(n)
    })
    .await
}

#[tauri::command]
pub async fn clipboard_panel_hide(app: AppHandle) -> AppResult<()> {
    let ui = app.clone();
    app.run_on_main_thread(move || panel::hide(&ui))?;
    Ok(())
}

/// 文件 → 在资源管理器中显示；链接 → 用浏览器打开。
#[tauri::command]
pub async fn clipboard_open(app: AppHandle, id: i64) -> AppResult<()> {
    let detail = state(&app).db.with(|c| repo::get_detail(c, id))?;
    match detail.item.kind.as_str() {
        "link" => {
            let url = detail.content_text.unwrap_or_default();
            app.opener()
                .open_url(url.trim(), None::<&str>)
                .map_err(|e| AppError::msg(e.to_string()))
        }
        "files" => {
            let first = detail
                .item
                .files
                .first()
                .cloned()
                .ok_or_else(|| AppError::msg("没有文件"))?;
            app.opener()
                .reveal_item_in_dir(first)
                .map_err(|e| AppError::msg(e.to_string()))
        }
        "image" => {
            let rel = detail
                .item
                .file_path
                .ok_or_else(|| AppError::msg("图片丢失"))?;
            let path = state(&app).paths.abs(&rel);
            app.opener()
                .reveal_item_in_dir(path)
                .map_err(|e| AppError::msg(e.to_string()))
        }
        _ => Ok(()),
    }
}
