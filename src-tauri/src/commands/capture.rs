//! 截图 / 长截图 / 贴图 / 编辑器命令。

use tauri::ipc::Request;
use tauri::AppHandle;

use super::{blocking, raw_request};
use crate::capture::{self, CaptureIntent, FinishMeta, SessionInfo};
use crate::editor::{self, EditorDoc, EditorFinish};
use crate::error::{AppError, AppResult};
use crate::pin::{self, PinInfo};
use crate::platform::{self, ClipboardPayload, MonitorId, PhysicalRect, ScannedCode, WindowHandle};
use crate::state::state;
use crate::{library, longshot, ocr, wm};

#[tauri::command]
pub async fn capture_start(app: AppHandle, intent: CaptureIntent) -> AppResult<()> {
    // 从主窗口按钮触发：主窗口别拍进去，截图结束时 `capture::end_session` 再放回来
    capture::stash_main(&app);
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

/// 截图界面里把选区识别成表格 / 公式（AI 看图），返回模型的回答（表格是 HTML，公式是 LaTeX + MathML）。
#[tauri::command]
pub async fn capture_structure(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
    rect: PhysicalRect,
    kind: String,
) -> AppResult<String> {
    let image = {
        let app = app.clone();
        blocking(move || {
            let session = state(&app)
                .capture
                .current()
                .filter(|s| s.id == session_id)
                .ok_or_else(|| AppError::msg("截图会话已结束"))?;
            let shot = session
                .monitor(MonitorId(monitor_id))
                .ok_or_else(|| AppError::msg("显示器不存在"))?;
            crate::imaging::crop_opaque(&shot.image, rect)
        })
        .await?
    };
    crate::ocr::structure_image(&app, &image, &kind).await
}

/// 截图时把鼠标下的颜色贴到屏幕上（颜色贴图）。`x`、`y` 是这块屏的局部物理坐标。
#[tauri::command]
pub async fn capture_pin_color(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
    x: i32,
    y: i32,
    rgb: [u8; 3],
) -> AppResult<()> {
    let session = state(&app)
        .capture
        .current()
        .filter(|s| s.id == session_id)
        .ok_or_else(|| AppError::msg("截图会话已结束"))?;
    let shot = session
        .monitor(MonitorId(monitor_id))
        .ok_or_else(|| AppError::msg("显示器不存在"))?;
    let at = (shot.info.bounds.x + x, shot.info.bounds.y + y);
    pin::create_color(&app, rgb, at, shot.info.scale_factor)?;
    capture::end_session(&app, session_id, true);
    Ok(())
}

/// 识别选区里的二维码 / 条码。位置换成这块屏的局部坐标（和选区一样）。
#[tauri::command]
pub async fn capture_scan_codes(
    app: AppHandle,
    session_id: u64,
    monitor_id: u64,
    rect: PhysicalRect,
) -> AppResult<Vec<ScannedCode>> {
    blocking(move || {
        let session = state(&app)
            .capture
            .current()
            .filter(|s| s.id == session_id)
            .ok_or_else(|| AppError::msg("截图会话已结束"))?;
        let shot = session
            .monitor(MonitorId(monitor_id))
            .ok_or_else(|| AppError::msg("显示器不存在"))?;
        let image = crate::imaging::crop_opaque(&shot.image, rect)?;
        let mut codes = platform::scan_codes(&image)?;
        for c in &mut codes {
            c.x += f64::from(rect.x);
            c.y += f64::from(rect.y);
        }
        Ok(codes)
    })
    .await
}

/// 截图原位翻译：识别选区里的文字并整批翻译，返回每段的位置和译文。
#[tauri::command]
pub async fn capture_translate(
    app: AppHandle,
    request: capture::region_translate::RegionRequest,
    on_lines: tauri::ipc::Channel<Vec<[f64; 4]>>,
) -> AppResult<capture::region_translate::RegionTranslation> {
    capture::region_translate::translate_region(&app, request, Some(on_lines)).await
}

#[tauri::command]
pub async fn capture_cancel(app: AppHandle, session_id: u64) -> AppResult<()> {
    capture::end_session(&app, session_id, true);
    Ok(())
}

/// 截图工具条的磨砂玻璃（设置里选了这个样式时）：在遮罩窗口里工具条的位置垫一块系统的毛玻璃。
/// 遮罩窗口是透明的，底下冻结的画面在另一个原生窗口里，网页的模糊滤镜够不着，只能用系统的。
/// `rect`：工具条在窗口里的位置（物理像素 x, y, 宽, 高），None = 撤掉；`radius`：圆角（物理像素）。
#[tauri::command]
pub async fn capture_toolbar_glass(
    window: tauri::WebviewWindow,
    rect: Option<(f32, f32, f32, f32)>,
    radius: u32,
) -> AppResult<()> {
    let target = window.clone();
    window.run_on_main_thread(move || match rect {
        Some(rect) => {
            platform::set_backdrop(&target, Some(radius));
            platform::set_backdrop_rect(&target, Some(rect), 0);
        }
        None => {
            platform::set_backdrop(&target, None);
        }
    })?;
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

/// 同时写纯文本和 HTML（表格：粘贴到 Excel / Word 时合并单元格、表头都还在）
#[tauri::command]
pub async fn clipboard_write_rich(text: String, html: String) -> AppResult<()> {
    platform::clipboard_write(&ClipboardPayload::Text {
        text,
        html: Some(html),
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

/// 贴图页面把图画好了，可以显示窗口了。
#[tauri::command]
pub async fn pin_ready(app: AppHandle, label: String) {
    pin::ready(&app, &label);
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

/// 旋转 / 翻转贴图（op：rotateCw、rotateCcw、flipH、flipV），返回新的尺寸
#[tauri::command]
pub async fn pin_transform(app: AppHandle, label: String, op: String) -> AppResult<PinInfo> {
    blocking(move || pin::transform(&app, &label, &op)).await
}

/// 恢复最近关掉的一张贴图，没有可恢复的返回 false
#[tauri::command]
pub async fn pin_restore_last(app: AppHandle) -> AppResult<bool> {
    blocking(move || pin::restore_last(&app)).await
}

/// 把贴图送进编辑器画标注
#[tauri::command]
pub async fn pin_edit(app: AppHandle, label: String) -> AppResult<()> {
    blocking(move || pin::edit(&app, &label)).await
}

/// 贴图里的二维码 / 条码
#[tauri::command]
pub async fn pin_scan_codes(app: AppHandle, label: String) -> AppResult<Vec<ScannedCode>> {
    blocking(move || platform::scan_codes(&pin_image(&app, &label)?)).await
}

/// 贴图里的文字和位置（鼠标直接选字用）
#[tauri::command]
pub async fn pin_text(app: AppHandle, label: String) -> AppResult<Vec<pin::PinText>> {
    blocking(move || pin::text(&app, &label).map(|t| (*t).clone())).await
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

// ───────────────────────── 录完剪辑 ─────────────────────────

/// 剪辑窗口要的信息：待剪辑的那段录制（时长、尺寸、缩略图、临时文件）
#[tauri::command]
pub async fn record_edit_info() -> AppResult<crate::record_edit::EditInfo> {
    crate::record_edit::info()
}

/// 按剪辑保存：`start_ms`..`end_ms` 一段、`speed` 倍速（导出要一会儿，放后台线程）
#[tauri::command]
pub async fn record_edit_save(
    app: AppHandle,
    id: u64,
    start_ms: u64,
    end_ms: u64,
    speed: f64,
) -> AppResult<()> {
    blocking(move || {
        crate::record_edit::save(
            &app,
            id,
            Some(platform::VideoEdit {
                start_ms,
                end_ms,
                speed,
            }),
        )
    })
    .await
}

#[tauri::command]
pub async fn record_edit_discard(app: AppHandle, id: u64) -> AppResult<()> {
    crate::record_edit::discard(&app, id);
    Ok(())
}

/// 麦克风 / 摄像头的授权状态
#[tauri::command]
pub async fn media_access(kind: platform::MediaKind) -> AppResult<platform::MediaAccess> {
    Ok(platform::media_access(kind))
}

/// 还没问过就弹系统的授权框，等用户点完
#[tauri::command]
pub async fn media_request(kind: platform::MediaKind) -> AppResult<platform::MediaAccess> {
    blocking(move || Ok(platform::request_media_access(kind))).await
}
