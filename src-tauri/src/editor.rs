//! 图片编辑窗口：长截图完成后、或从截图库里打开一张图时用（规格 03 §7）。
//!
//! 复用截图遮罩里那套标注工具，但工具条固定在窗口底部，图片可滚动查看。

use std::sync::Arc;

use image::RgbaImage;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, AppResult};
use crate::platform::ClipboardPayload;
use crate::state::state;
use crate::{capture, events, imaging, ocr, pin, platform, wm};

pub const WINDOW: &str = "editor";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorDoc {
    pub id: String,
    pub image_id: String,
    pub width: u32,
    pub height: u32,
    pub screenshot_id: Option<i64>,
}

static CURRENT: Mutex<Option<EditorDoc>> = Mutex::new(None);

pub fn current() -> Option<EditorDoc> {
    CURRENT.lock().clone()
}

/// 图片入库晚于打开窗口时（长截图：先让用户看到图，再慢慢编码入库），补上库记录 id。
pub fn attach_screenshot(doc_id: &str, screenshot_id: i64) {
    if let Some(doc) = CURRENT.lock().as_mut().filter(|d| d.id == doc_id) {
        doc.screenshot_id = Some(screenshot_id);
    }
}

/// 打开编辑窗口，返回文档 id。
pub fn open(app: &AppHandle, image: RgbaImage, screenshot_id: Option<i64>) -> AppResult<String> {
    let st = state(app);
    let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let image_id = format!("editor-{id}");
    let (w, h) = image.dimensions();
    st.images.remove_prefix("editor-");
    st.images.put(image_id.clone(), Arc::new(image));
    let doc = EditorDoc {
        id,
        image_id,
        width: w,
        height: h,
        screenshot_id,
    };
    *CURRENT.lock() = Some(doc.clone());
    let doc_id = doc.id.clone();

    let ui_app = app.clone();
    app.run_on_main_thread(move || {
        let window = match ui_app.get_webview_window(WINDOW) {
            Some(w) => w,
            None => match wm::builder(&ui_app, WINDOW)
                .title("编辑图片 - DATO OCR")
                .resizable(true)
                .transparent(true)
                .shadow(true)
                .min_inner_size(640.0, 420.0)
                .inner_size(960.0, 720.0)
                .build()
            {
                Ok(w) => {
                    wm::apply_window_effects(&ui_app, &w);
                    w
                }
                Err(err) => {
                    tracing::error!("创建编辑窗口失败：{err}");
                    return;
                }
            },
        };
        if let Some(monitor) = wm::monitor_under_cursor() {
            let s = monitor.scale_factor.max(0.5);
            let max_w = f64::from(monitor.work_area.width) / s * 0.8;
            let max_h = f64::from(monitor.work_area.height) / s * 0.88;
            let ww = (f64::from(w) / s + 64.0).clamp(640.0, max_w.max(640.0));
            let wh = (f64::from(h) / s + 160.0).clamp(420.0, max_h.max(420.0));
            let _ = wm::place_on_monitor(&window, &monitor, ww, wh, wm::Anchor::Center);
        }
        let _ = ui_app.emit_to(WINDOW, events::EDITOR_OPEN, &doc);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    })?;
    Ok(doc_id)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EditorAction {
    Copy,
    Save,
    Pin,
    Ocr,
    Translate,
    Ai,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorFinish {
    pub doc_id: String,
    pub action: EditorAction,
    /// 标注层 PNG 左上角（图片像素坐标）
    pub annotation_at: Option<(i32, i32)>,
}

pub fn finish(app: &AppHandle, meta: EditorFinish, annotation_png: Vec<u8>) -> AppResult<()> {
    let doc = current()
        .filter(|d| d.id == meta.doc_id)
        .ok_or_else(|| AppError::msg("图片已关闭"))?;
    let st = state(app);
    let base = st
        .images
        .get(&doc.image_id)
        .ok_or_else(|| AppError::msg("图片已释放"))?;
    let mut image = (*base).clone();
    if let Some((x, y)) = meta.annotation_at.filter(|_| !annotation_png.is_empty()) {
        imaging::composite_png(&mut image, &annotation_png, i64::from(x), i64::from(y))?;
    }
    let settings = st.settings.read().capture.clone();
    match meta.action {
        EditorAction::Copy => {
            if image.height() > 8000 {
                // 超大图很多程序粘贴不了，提示一下但照样复制
                wm::toast(app, "info", "图片很长，部分程序可能无法粘贴，建议保存");
            }
            platform::clipboard_write(&ClipboardPayload::Image { image, png: None })?;
            wm::toast(app, "success", "已复制到剪贴板");
        }
        EditorAction::Save => {
            if let Some(path) = capture::ask_save_path(app, &settings)? {
                capture::write_image_file(&image, &path, &settings)?;
                wm::toast(app, "success", "已保存");
            }
        }
        EditorAction::Pin => {
            let monitor = wm::monitor_under_cursor().ok_or_else(|| AppError::msg("没有显示器"))?;
            let wa = monitor.work_area;
            let x = wa.x + (wa.width as i32 - image.width() as i32).max(0) / 2;
            let y = wa.y + (wa.height as i32 - image.height() as i32).max(0) / 2;
            pin::create(app, Arc::new(image), (x, y), monitor.scale_factor)?;
        }
        EditorAction::Ai => {
            let attached = crate::ai::store_image(app, Arc::new(image));
            crate::ai::open_window(
                app,
                crate::ai::AiContext {
                    images: vec![attached],
                    source: "capture".into(),
                    ..Default::default()
                },
            );
        }
        EditorAction::Ocr | EditorAction::Translate => {
            ocr::open_job(
                app,
                image,
                doc.screenshot_id,
                meta.action == EditorAction::Translate,
            )?;
        }
    }
    Ok(())
}

pub fn close(app: &AppHandle) {
    if let Some(doc) = CURRENT.lock().take() {
        state(app).images.remove(&doc.image_id);
    }
    if let Some(w) = app.get_webview_window(WINDOW) {
        let _ = w.hide();
    }
}
