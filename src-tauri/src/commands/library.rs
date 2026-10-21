//! 截图库与识字记录命令。

use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use super::blocking;
use crate::error::{AppError, AppResult};
use crate::platform::{self, ClipboardPayload};
use crate::state::state;
use crate::storage::clipboard as clip_repo;
use crate::storage::ocr::{self as ocr_repo, OcrPage, OcrQuery};
use crate::storage::screenshots::{self as repo, ShotPage, ShotQuery};
use crate::{capture, imaging, library, ocr, pin, wm};

#[tauri::command]
pub async fn library_query(app: AppHandle, query: ShotQuery) -> AppResult<ShotPage> {
    state(&app).db.with(|c| repo::query(c, &query))
}

#[tauri::command]
pub async fn library_delete(app: AppHandle, ids: Vec<i64>) -> AppResult<()> {
    blocking(move || {
        for id in ids {
            library::delete(&app, id)?;
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn library_set_favorite(app: AppHandle, id: i64, value: bool) -> AppResult<()> {
    state(&app).db.with(|c| repo::set_favorite(c, id, value))
}

#[tauri::command]
pub async fn library_set_note(app: AppHandle, id: i64, note: Option<String>) -> AppResult<()> {
    state(&app)
        .db
        .with(|c| repo::set_note(c, id, note.as_deref()))
}

#[tauri::command]
pub async fn library_copy(app: AppHandle, id: i64) -> AppResult<()> {
    blocking(move || {
        let st = state(&app);
        let shot = st.db.with(|c| repo::get(c, id))?;
        // GIF 按文件复制（按图片的话只剩第一帧）
        if shot.kind == library::KIND_GIF {
            let file = st.paths.abs(&shot.file_path);
            platform::clipboard_write(&ClipboardPayload::Files(vec![file]))?;
            wm::toast(&app, "success", "已复制到剪贴板");
            return Ok(());
        }
        let image = library::load_image(&app, id)?;
        platform::clipboard_write(&ClipboardPayload::Image { image, png: None })?;
        wm::toast(&app, "success", "已复制到剪贴板");
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn library_pin(app: AppHandle, id: i64) -> AppResult<()> {
    blocking(move || {
        let image = library::load_image(&app, id)?;
        let monitor = wm::monitor_under_cursor().ok_or_else(|| AppError::msg("没有显示器"))?;
        let wa = monitor.work_area;
        let x = wa.x + (wa.width as i32 - image.width() as i32).max(0) / 2;
        let y = wa.y + (wa.height as i32 - image.height() as i32).max(0) / 2;
        pin::create(
            &app,
            std::sync::Arc::new(image),
            (x, y),
            monitor.scale_factor,
            None,
        )?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn library_save_as(app: AppHandle, id: i64) -> AppResult<()> {
    blocking(move || {
        let image = library::load_image(&app, id)?;
        let settings = state(&app).settings.read().capture.clone();
        if let Some(path) = capture::ask_save_path(&app, &settings)? {
            capture::write_image_file(&image, &path, &settings)?;
            wm::toast(&app, "success", "已保存");
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn library_reveal(app: AppHandle, id: i64) -> AppResult<()> {
    let st = state(&app);
    let shot = st.db.with(|c| repo::get(c, id))?;
    app.opener()
        .reveal_item_in_dir(st.paths.abs(&shot.file_path))
        .map_err(|e| AppError::msg(e.to_string()))
}

#[tauri::command]
pub async fn library_ocr(app: AppHandle, id: i64, translate: bool) -> AppResult<()> {
    blocking(move || {
        let image = library::load_image(&app, id)?;
        ocr::open_job(&app, image, Some(id), translate)
    })
    .await
}

// ───────────────────────── 识字 ─────────────────────────

#[tauri::command]
pub async fn ocr_current_job(app: AppHandle) -> AppResult<Option<ocr::OcrJob>> {
    Ok(ocr::current_job(&app))
}

#[tauri::command]
pub async fn ocr_rerun(
    app: AppHandle,
    job_id: String,
    engine: Option<String>,
    upscale: bool,
) -> AppResult<()> {
    blocking(move || ocr::rerun(&app, &job_id, engine, upscale)).await
}

/// 识别出来的表格导出成 Excel：先问存在哪，返回存好的路径（取消了返回 None）
#[tauri::command]
pub async fn ocr_export_xlsx(
    app: AppHandle,
    rows: Vec<Vec<ocr::xlsx::TableCell>>,
) -> AppResult<Option<String>> {
    blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let settings = crate::state::state(&app).settings.read().capture.clone();
        let dir = crate::capture::save_directory(&settings);
        let tr = crate::i18n::localize;
        let picked = app
            .dialog()
            .file()
            .set_title(tr("导出表格"))
            .set_directory(&dir)
            .set_file_name(tr("表格.xlsx"))
            .add_filter(tr("Excel 表格"), &["xlsx"])
            .blocking_save_file();
        let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
            return Ok(None);
        };
        let path = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("xlsx")) {
            path
        } else {
            path.with_extension("xlsx")
        };
        ocr::xlsx::write(&rows, &path)?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
}

/// 用 AI 把识字的图识别成表格 / 公式（kind：table | formula）
#[tauri::command]
pub async fn ocr_structure(app: AppHandle, job_id: String, kind: String) -> AppResult<String> {
    ocr::structure(&app, &job_id, &kind).await
}

#[tauri::command]
pub async fn ocr_save_text(app: AppHandle, record_id: i64, text: String) -> AppResult<()> {
    state(&app)
        .db
        .with(|c| ocr_repo::update_text(c, record_id, &text))
}

#[tauri::command]
pub async fn ocr_status(app: AppHandle) -> AppResult<ocr::EngineStatus> {
    blocking(move || Ok(ocr::status(&app))).await
}

/// 下载安装 PaddleOCR 引擎（进度走 `ocr-engine-download` 事件）。
#[tauri::command]
pub async fn ocr_paddle_install(app: AppHandle) -> AppResult<()> {
    ocr::paddle::install(&app).await
}

/// 删掉 PaddleOCR 引擎；正在用它的话改回 RapidOCR。
#[tauri::command]
pub async fn ocr_paddle_remove(app: AppHandle) -> AppResult<()> {
    tokio::task::spawn_blocking({
        let app = app.clone();
        move || ocr::paddle::remove(&app)
    })
    .await
    .map_err(|e| crate::error::AppError::msg(e.to_string()))??;
    if state(&app).settings.read().ocr.engine == "paddle" {
        crate::commands::system::update_settings_with(&app, |s| s.ocr.engine = "rapid".into())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn ocr_history(app: AppHandle, query: OcrQuery) -> AppResult<OcrPage> {
    state(&app).db.with(|c| ocr_repo::query(c, &query))
}

#[tauri::command]
pub async fn ocr_open_record(app: AppHandle, id: i64) -> AppResult<()> {
    blocking(move || ocr::open_record(&app, id)).await
}

#[tauri::command]
pub async fn ocr_delete_record(app: AppHandle, id: i64) -> AppResult<()> {
    blocking(move || ocr::delete_record(&app, id)).await
}

/// 剪贴板里的图片 → 识字。
#[tauri::command]
pub async fn ocr_from_clip(app: AppHandle, id: i64, translate: bool) -> AppResult<()> {
    blocking(move || {
        let st = state(&app);
        let item = st.db.with(|c| clip_repo::get_item(c, id))?;
        let rel = item
            .file_path
            .ok_or_else(|| AppError::msg("这条记录不是图片"))?;
        let image = imaging::load(&st.paths.abs(&rel))?;
        ocr::open_job(&app, image, None, translate)
    })
    .await
}
