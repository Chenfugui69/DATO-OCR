//! 截图 / 长截图 / 贴图 / 编辑器命令。

use tauri::ipc::Request;
use tauri::{AppHandle, Manager};

use super::{blocking, raw_request};
use crate::capture::{self, CaptureIntent, FinishMeta, SessionInfo};
use crate::editor::{self, EditorDoc, EditorFinish};
use crate::error::{AppError, AppResult};
use crate::pin::{self, PinInfo};
use crate::platform::{self, ClipboardPayload, MonitorId, PhysicalRect, WindowHandle};
use crate::state::state;
use crate::{library, longshot, ocr, wm};

#[tauri::command]
pub async fn capture_start(app: AppHandle, intent: CaptureIntent) -> AppResult<()> {
    // 从主窗口按钮触发时先把主窗口藏起来，别拍进去
    if let Some(main) = app.get_webview_window(wm::MAIN) {
        if main.is_visible().unwrap_or(false) {
            let _ = main.hide();
            std::thread::sleep(std::time::Duration::from_millis(220));
        }
    }
    capture::trigger(&app, intent);
    Ok(())
}

/// 遮罩页面加载完成时调一次：如果热键早于页面加载到达，补上会话。
#[tauri::command]
pub async fn capture_session_info(
    app: AppHandle,
    monitor_id: u64,
) -> AppResult<Option<SessionInfo>> {
    capture::session_info(&app, MonitorId(monitor_id))
}

#[tauri::command]
pub async fn capture_overlay_ready(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
) -> AppResult<()> {
    capture::overlay_ready(&app, session_id, MonitorId(monitor_id))
}

#[tauri::command]
pub async fn capture_window_children(handle: u64) -> AppResult<Vec<PhysicalRect>> {
    platform::enumerate_children(WindowHandle(handle))
}

#[tauri::command]
pub async fn capture_finish(app: AppHandle, request: Request<'_>) -> AppResult<()> {
    let (meta, body): (FinishMeta, Vec<u8>) = raw_request(&request)?;
    blocking(move || capture::finish(&app, meta, body)).await
}

/// 截图原位翻译：识别选区里的文字并整批翻译，返回每段的位置和译文。
#[tauri::command]
pub async fn capture_translate(
    app: AppHandle,
    request: capture::region_translate::RegionRequest,
) -> AppResult<capture::region_translate::RegionTranslation> {
    capture::region_translate::translate_region(&app, request).await
}

#[tauri::command]
pub async fn capture_cancel(app: AppHandle, session_id: u64) -> AppResult<()> {
    capture::end_session(&app, session_id, true);
    Ok(())
}

/// 取色器复制颜色值、识字结果复制等：写一段纯文本到剪贴板（会进历史）。
#[tauri::command]
pub async fn clipboard_write_text(text: String) -> AppResult<()> {
    platform::clipboard_write(&ClipboardPayload::Text {
        text,
        html: None,
        rtf: None,
    })
}

// ───────────────────────── 长截图 ─────────────────────────

#[tauri::command]
pub async fn longshot_set_regions(app: AppHandle, regions: Vec<PhysicalRect>) -> AppResult<()> {
    longshot::set_regions(&app, regions);
    Ok(())
}

#[tauri::command]
pub async fn gif_set_regions(regions: Vec<PhysicalRect>) -> AppResult<()> {
    crate::gif_record::set_regions(regions);
    Ok(())
}

#[tauri::command]
pub async fn gif_finish() -> AppResult<()> {
    crate::gif_record::finish();
    Ok(())
}

#[tauri::command]
pub async fn gif_cancel() -> AppResult<()> {
    crate::gif_record::cancel();
    Ok(())
}

#[tauri::command]
pub async fn longshot_finish(app: AppHandle) -> AppResult<()> {
    blocking(move || {
        longshot::finish(&app);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn longshot_abort(app: AppHandle) -> AppResult<()> {
    longshot::abort(&app);
    Ok(())
}

#[tauri::command]
pub async fn longshot_undo(app: AppHandle) -> AppResult<()> {
    longshot::undo(&app);
    Ok(())
}

// ───────────────────────── 贴图 ─────────────────────────

#[tauri::command]
pub async fn pin_info(app: AppHandle, label: String) -> AppResult<PinInfo> {
    pin::info(&app, &label)
}

#[tauri::command]
pub async fn pin_close(app: AppHandle, label: String) -> AppResult<()> {
    let ui = app.clone();
    app.run_on_main_thread(move || pin::close(&ui, &label))?;
    Ok(())
}

#[tauri::command]
pub async fn pin_close_all(app: AppHandle) -> AppResult<()> {
    let ui = app.clone();
    app.run_on_main_thread(move || pin::close_all(&ui))?;
    Ok(())
}

fn pin_image(app: &AppHandle, label: &str) -> AppResult<image::RgbaImage> {
    state(app)
        .pins
        .image(label)
        .map(|img| (*img).clone())
        .ok_or_else(|| AppError::NotFound("贴图".into()))
}

#[tauri::command]
pub async fn pin_copy(app: AppHandle, label: String) -> AppResult<()> {
    blocking(move || {
        let image = pin_image(&app, &label)?;
        platform::clipboard_write(&ClipboardPayload::Image { image, png: None })?;
        wm::toast(&app, "success", "已复制到剪贴板");
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn pin_save(app: AppHandle, label: String) -> AppResult<()> {
    blocking(move || {
        let image = pin_image(&app, &label)?;
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
pub async fn pin_ocr(app: AppHandle, label: String, translate: bool) -> AppResult<()> {
    blocking(move || {
        let image = pin_image(&app, &label)?;
        ocr::open_job(&app, image, None, translate)
    })
    .await
}

// ───────────────────────── 编辑器 ─────────────────────────

#[tauri::command]
pub async fn editor_current() -> AppResult<Option<EditorDoc>> {
    Ok(editor::current())
}

#[tauri::command]
pub async fn editor_finish(app: AppHandle, request: Request<'_>) -> AppResult<()> {
    let (meta, body): (EditorFinish, Vec<u8>) = raw_request(&request)?;
    blocking(move || editor::finish(&app, meta, body)).await
}

#[tauri::command]
pub async fn editor_close(app: AppHandle) -> AppResult<()> {
    let ui = app.clone();
    app.run_on_main_thread(move || editor::close(&ui))?;
    Ok(())
}

#[tauri::command]
pub async fn editor_open_shot(app: AppHandle, id: i64) -> AppResult<()> {
    blocking(move || {
        let image = library::load_image(&app, id)?;
        editor::open(&app, image, Some(id)).map(drop)
    })
    .await
}
