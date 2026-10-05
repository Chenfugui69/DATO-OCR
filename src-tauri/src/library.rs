//! 截图库：每次截图完成都存一份（文件 + 数据库），主窗口「截图库」页查看（规格 02 §3.8.5）。

use image::RgbaImage;
use tauri::{AppHandle, Emitter};

use crate::error::AppResult;
use crate::paths::thumb_rel;
use crate::state::state;
use crate::storage::screenshots::{self, NewShot};
use crate::{events, imaging};

pub fn add(
    app: &AppHandle,
    image: &RgbaImage,
    kind: &str,
    source_app: Option<String>,
    annotated: bool,
) -> AppResult<i64> {
    let st = state(app);
    let rel = st.paths.new_rel_file("screenshots", "png")?;
    let bytes = imaging::encode_png(image)?;
    let size = bytes.len() as u64;
    std::fs::write(st.paths.abs(&rel), &bytes)?;
    let thumb = thumb_rel(&rel);
    let thumb = match imaging::write_card_thumbnail(image, &st.paths.abs(&thumb)) {
        Ok(()) => Some(thumb),
        Err(err) => {
            tracing::warn!("生成截图缩略图失败：{err}");
            None
        }
    };
    let shot = NewShot {
        file_path: rel,
        thumb_path: thumb,
        width: image.width(),
        height: image.height(),
        size_bytes: size,
        kind: kind.to_string(),
        source_app,
        has_annotations: annotated,
    };
    let id = st.db.with(|c| screenshots::insert(c, &shot))?;
    let _ = app.emit(events::LIBRARY_CHANGED, ());
    Ok(id)
}

pub fn delete(app: &AppHandle, id: i64) -> AppResult<()> {
    let st = state(app);
    let files = st.db.with(|c| screenshots::delete(c, id))?;
    for f in files {
        let _ = std::fs::remove_file(st.paths.abs(&f));
    }
    let _ = app.emit(events::LIBRARY_CHANGED, ());
    Ok(())
}

pub fn load_image(app: &AppHandle, id: i64) -> AppResult<RgbaImage> {
    let st = state(app);
    let shot = st.db.with(|c| screenshots::get(c, id))?;
    imaging::load(&st.paths.abs(&shot.file_path))
}
