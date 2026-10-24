//! 录完先剪辑（设置 → 录制 → 录完先剪辑）：录好的 GIF / 视频先放在临时目录，打开剪辑窗口，
//! 用户掐头去尾、选倍速后再保存。
//!
//! - 视频：取一段、变速（声音保持音调），按设置里的画质 / 编码器重新编码；两条音轨顺带混成一条
//!   （设置里开了"独立音轨"就各自保留）
//! - GIF：解码、只留那一段、按倍速改帧间隔，重新编码
//!
//! 剪辑窗口直接关掉 = 按原样保存（录好的东西不会因为手滑丢掉）；要扔掉得点"不要了"。
//! 同一时间只有一段待剪辑的录制；剪辑窗口还开着又录了一段，前一段按原样保存。

use std::path::{Path, PathBuf};

use base64::Engine;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use crate::error::{AppError, AppResult};
use crate::gif_record::{self, RecordFormat, Stats};
use crate::platform::{self, VideoEdit};
use crate::state::state;
use crate::{events, wm};

pub const WINDOW: &str = "recedit";

struct Pending {
    id: u64,
    tmp: PathBuf,
    format: RecordFormat,
    stats: Stats,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditThumb {
    pub ms: u64,
    /// data:image/jpeg;base64,…
    pub url: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditInfo {
    pub id: u64,
    pub format: RecordFormat,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    /// 临时文件（页面用 asset 协议播放视频）
    pub path: String,
    pub thumbs: Vec<EditThumb>,
}

/// 录完、要剪辑：记下这一段，打开剪辑窗口
pub fn open(app: &AppHandle, tmp: PathBuf, format: RecordFormat, stats: Stats) {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    // 上一段还没处理：按原样保存
    if let Some(prev) = PENDING.lock().replace(Pending {
        id,
        tmp,
        format,
        stats,
    }) {
        let app = app.clone();
        std::thread::spawn(move || finalize(&app, &prev.tmp, prev.format, &prev.stats, None));
    }
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        let window = match ui.get_webview_window(WINDOW) {
            Some(w) => w,
            None => match wm::framed_builder(&ui, WINDOW)
                .title(crate::i18n::localize("剪辑录制 - DATO OCR"))
                .resizable(true)
                .transparent(true)
                .shadow(true)
                .min_inner_size(640.0, 480.0)
                .inner_size(860.0, 640.0)
                .build()
            {
                Ok(w) => {
                    wm::apply_window_effects(&ui, &w);
                    let closing = ui.clone();
                    let raised = w.clone();
                    w.on_window_event(move |event| match event {
                        WindowEvent::CloseRequested { .. } => keep_pending(&closing),
                        // 用户点进窗口了：不用再浮在最上面
                        WindowEvent::Focused(true) => {
                            let _ = raised.set_always_on_top(false);
                        }
                        _ => {}
                    });
                    w
                }
                Err(err) => {
                    tracing::error!("创建剪辑窗口失败：{err}");
                    keep_pending(&ui);
                    return;
                }
            },
        };
        if let Some(monitor) = wm::monitor_under_cursor() {
            let _ = wm::place_on_monitor(&window, &monitor, 860.0, 640.0, wm::Anchor::Center);
        }
        let _ = ui.emit_to(WINDOW, events::RECORD_EDIT, id);
        let _ = window.unminimize();
        // 录完时焦点在用户刚才录的那个应用上，我们不是前台应用，普通窗口只会出现在它后面
        // （系统不让后台应用抢到最前）：先浮在最上面，用户点进来以后再变回普通窗口
        let _ = window.set_always_on_top(true);
        let _ = window.show();
        let _ = window.set_focus();
    });
}

/// 剪辑窗口没点保存就关了：按原样保存
fn keep_pending(app: &AppHandle) {
    if let Some(p) = PENDING.lock().take() {
        let app = app.clone();
        std::thread::spawn(move || finalize(&app, &p.tmp, p.format, &p.stats, None));
    }
}

pub fn info() -> AppResult<EditInfo> {
    let guard = PENDING.lock();
    let p = guard
        .as_ref()
        .ok_or_else(|| AppError::NotFound("待剪辑的录制".into()))?;
    let b64 = base64::engine::general_purpose::STANDARD;
    Ok(EditInfo {
        id: p.id,
        format: p.format,
        duration_ms: p.stats.duration_ms,
        width: p.stats.width,
        height: p.stats.height,
        path: p.tmp.to_string_lossy().into_owned(),
        thumbs: p
            .stats
            .thumbs
            .iter()
            .map(|t| EditThumb {
                ms: t.ms,
                url: format!("data:image/jpeg;base64,{}", b64.encode(&t.jpeg)),
            })
            .collect(),
    })
}

/// 按剪辑保存（`edit` 为 None = 原样）。阻塞，调用方放在后台线程
pub fn save(app: &AppHandle, id: u64, edit: Option<VideoEdit>) -> AppResult<()> {
    let p = {
        let mut guard = PENDING.lock();
        match guard.take() {
            Some(p) if p.id == id => p,
            other => {
                *guard = other;
                return Err(AppError::msg("这段录制已经处理过了"));
            }
        }
    };
    close_window(app);
    finalize(app, &p.tmp, p.format, &p.stats, edit);
    Ok(())
}

/// 不要了：删掉临时文件
pub fn discard(app: &AppHandle, id: u64) {
    let p = {
        let mut guard = PENDING.lock();
        if guard.as_ref().map(|p| p.id) != Some(id) {
            return;
        }
        guard.take()
    };
    if let Some(p) = p {
        let _ = std::fs::remove_file(&p.tmp);
    }
    close_window(app);
}

fn close_window(app: &AppHandle) {
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = ui.get_webview_window(WINDOW) {
            let _ = w.destroy();
        }
    });
}

/// 收尾：要剪辑的先剪，视频有两条音轨的混成一条（开了"独立音轨"就不混），动图要画进度条 / 播放时间的
/// 重新编码一遍，然后保存。出错就提示，原文件尽量照样保存。
pub fn finalize(
    app: &AppHandle,
    tmp: &Path,
    format: RecordFormat,
    stats: &Stats,
    edit: Option<VideoEdit>,
) {
    // 剪辑范围是整段、1 倍速：当成没剪
    let edit = edit.filter(|e| {
        e.start_ms > 0 || e.end_ms + 50 < stats.duration_ms || (e.speed - 1.0).abs() > 0.001
    });
    let rec = state(app).settings.read().record.clone();
    let mix = !rec.separate_audio_tracks;
    let decor = format
        .is_animation()
        .then(|| crate::record_fx::Decor::from_settings(&rec))
        .flatten();
    let needs_export = edit.is_some()
        || decor.is_some()
        || (format == RecordFormat::Mp4 && stats.audio_tracks > 1 && mix);
    if !needs_export {
        report(app, gif_record::save(app, tmp, stats, format), format);
        return;
    }
    let edit = edit.unwrap_or(VideoEdit {
        start_ms: 0,
        end_ms: stats.duration_ms.max(1),
        speed: 1.0,
    });
    let out = tmp.with_file_name(format!(
        "{}-edit.{}",
        tmp.file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default(),
        format.ext()
    ));
    if edit.start_ms > 0
        || (edit.speed - 1.0).abs() > 0.001
        || edit.end_ms + 50 < stats.duration_ms
        || format == RecordFormat::Mp4
    {
        wm::toast(app, "info", "正在导出…");
    }
    let started = std::time::Instant::now();
    let result = match format {
        RecordFormat::Mp4 => {
            platform::export_video(tmp, &out, edit, gif_record::video_encoding(&rec), mix).map(
                |()| Stats {
                    duration_ms: ((edit.end_ms.min(stats.duration_ms)
                        - edit.start_ms.min(stats.duration_ms))
                        as f64
                        / edit.speed.clamp(0.25, 4.0)) as u64,
                    ..stats.clone()
                },
            )
        }
        _ => gif_record::rework_animation(
            tmp,
            &out,
            format,
            edit,
            stats.duration_ms,
            rec.gif_loop,
            decor,
        ),
    };
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        ?format,
        ok = result.is_ok(),
        "导出剪辑"
    );
    match result {
        Ok(new_stats) => {
            let _ = std::fs::remove_file(tmp);
            report(app, gif_record::save(app, &out, &new_stats, format), format);
        }
        Err(err) => {
            let _ = std::fs::remove_file(&out);
            tracing::error!("导出剪辑失败：{err}");
            wm::toast(app, "error", format!("剪辑失败，按原样保存：{err}"));
            report(app, gif_record::save(app, tmp, stats, format), format);
        }
    }
}

fn report(app: &AppHandle, result: AppResult<()>, format: RecordFormat) {
    if let Err(err) = result {
        tracing::error!(?format, "保存录制失败：{err}");
        wm::toast(app, "error", format!("保存失败：{err}"));
    }
}
