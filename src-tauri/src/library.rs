//! 截图库：每次截图完成都存一份（文件 + 数据库），主窗口「截图库」页查看（规格 02 §3.8.5）。

use std::path::Path;

use image::RgbaImage;
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};
use crate::paths::thumb_rel;
use crate::state::state;
use crate::storage::screenshots::{self, NewShot};
use crate::{events, imaging};

/// 截图库里 GIF 的 kind（另外两种是 normal、longshot）
pub const KIND_GIF: &str = "gif";

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

/// 录好的 GIF 也进截图库：在数据目录里留一份（保存目录里那份用户可能挪走、删掉），缩略图用第一帧。
pub fn add_gif(app: &AppHandle, path: &Path) -> AppResult<i64> {
    let st = state(app);
    let first = gif_first_frame(path)?;
    let rel = st.paths.new_rel_file("screenshots", "gif")?;
    let size = std::fs::copy(path, st.paths.abs(&rel))?;
    let thumb = thumb_rel(&rel);
    let thumb = match imaging::write_card_thumbnail(&first, &st.paths.abs(&thumb)) {
        Ok(()) => Some(thumb),
        Err(err) => {
            tracing::warn!("生成 GIF 缩略图失败：{err}");
            None
        }
    };
    let shot = NewShot {
        file_path: rel,
        thumb_path: thumb,
        width: first.width(),
        height: first.height(),
        size_bytes: size,
        kind: KIND_GIF.to_string(),
        source_app: None,
        has_annotations: false,
    };
    let id = st.db.with(|c| screenshots::insert(c, &shot))?;
    let _ = app.emit(events::LIBRARY_CHANGED, ());
    Ok(id)
}

/// GIF 的第一帧（我们自己录的 GIF 第一帧总是整幅画面）。
fn gif_first_frame(path: &Path) -> AppResult<RgbaImage> {
    let bad = |e: &dyn std::fmt::Display| AppError::msg(format!("GIF 读不出来：{e}"));
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options
        .read_info(std::io::BufReader::new(std::fs::File::open(path)?))
        .map_err(|e| bad(&e))?;
    let (w, h) = (u32::from(decoder.width()), u32::from(decoder.height()));
    let frame = decoder
        .read_next_frame()
        .map_err(|e| bad(&e))?
        .ok_or_else(|| bad(&"没有画面"))?;
    // 第一帧不是整幅的（别处来的 GIF）：贴到一张透明底上
    let mut canvas = RgbaImage::new(w, h);
    if let Some(part) = RgbaImage::from_raw(
        u32::from(frame.width),
        u32::from(frame.height),
        frame.buffer.to_vec(),
    ) {
        image::imageops::replace(
            &mut canvas,
            &part,
            i64::from(frame.left),
            i64::from(frame.top),
        );
    }
    Ok(canvas)
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
    let path = st.paths.abs(&shot.file_path);
    // GIF 当图片用（贴图、识字）时取第一帧
    if shot.kind == KIND_GIF {
        return gif_first_frame(&path);
    }
    imaging::load(&path)
}
