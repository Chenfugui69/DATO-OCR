//! 文字识别（规格 04 第一部分）。
//!
//! 引擎生命周期：**懒启动**（第一次识别才拉起）→ **常驻复用** → **空闲回收**（默认 5
//! 分钟）→ **崩溃自愈**（下次请求自动重启，连续失败 3 次后报错）→ 应用退出时杀掉。
//! RapidOCR 不可用时退回 Windows 自带 OCR。可选下载 PaddleOCR（paddle.rs），选了它但失败时退回 RapidOCR。

pub mod paddle;
pub mod rapid;
pub mod reflow;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use image::RgbaImage;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, AppResult};
use crate::state::state;
use crate::storage::{ocr as repo, screenshots};
use crate::{events, imaging, platform, wm};
use rapid::RapidEngine;
use reflow::{OcrBlock, Paragraph};

pub const WINDOW: &str = "ocr";
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

#[derive(Default)]
pub struct OcrService {
    rapid: Mutex<Option<RapidEngine>>,
    paddle: Mutex<Option<RapidEngine>>,
    paddle_download: Mutex<paddle::DownloadStatus>,
    last_used: Mutex<Option<Instant>>,
    failures: AtomicU32,
    paddle_failures: AtomicU32,
    reaper: AtomicBool,
    jobs: Mutex<HashMap<String, OcrJob>>,
    current: Mutex<Option<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub blocks: Vec<OcrBlock>,
    pub paragraphs: Vec<Paragraph>,
    pub plain_text: String,
    pub raw_text: String,
    pub engine: String,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrJob {
    pub id: String,
    /// running | done | error
    pub status: String,
    pub image_path: String,
    pub width: u32,
    pub height: u32,
    pub translate: bool,
    pub record_id: Option<i64>,
    pub result: Option<OcrResult>,
    pub error: Option<String>,
    /// 识别完已经自动复制到剪贴板
    #[serde(default)]
    pub copied: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub rapid: bool,
    pub rapid_running: bool,
    pub system: bool,
    pub avx: bool,
    /// PaddleOCR 已下载
    pub paddle: bool,
    pub paddle_download: paddle::DownloadStatus,
}

impl OcrService {
    pub fn shutdown(&self) {
        for slot in [&self.rapid, &self.paddle] {
            if let Some(mut engine) = slot.lock().take() {
                engine.kill();
            }
        }
    }
}

pub fn status(app: &AppHandle) -> EngineStatus {
    let st = state(app);
    let rapid_running = st.ocr.rapid.lock().is_some();
    let paddle_download = st.ocr.paddle_download.lock().clone();
    EngineStatus {
        rapid: rapid::engine_dir(&st.paths).is_some(),
        rapid_running,
        system: platform::system_ocr_available(),
        avx: supports_avx(),
        paddle: paddle::engine_dir(&st.paths).is_some(),
        paddle_download,
    }
}

/// PaddleOCR（可选高精度引擎）要求 AVX。
pub(crate) fn supports_avx() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        std::arch::is_x86_feature_detected!("avx")
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        false
    }
}

/// 后台回收线程：空闲超时就杀掉引擎进程释放内存（RapidOCR 峰值 ~500MB）。
fn ensure_reaper(app: &AppHandle) {
    let st = state(app);
    if st.ocr.reaper.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(30));
        let st = state(&app);
        let minutes = st.settings.read().ocr.idle_timeout_minutes;
        if minutes == 0 {
            continue;
        }
        let idle = st
            .ocr
            .last_used
            .lock()
            .is_some_and(|t| t.elapsed() > Duration::from_secs(u64::from(minutes) * 60));
        if idle {
            for slot in [&st.ocr.rapid, &st.ocr.paddle] {
                if let Some(mut engine) = slot.lock().take() {
                    engine.kill();
                    tracing::info!("识字引擎空闲超时，已回收");
                }
            }
        }
    });
}

#[derive(Clone, Copy, PartialEq)]
enum Engine {
    Rapid,
    Paddle,
}

fn run_engine(app: &AppHandle, kind: Engine, image: &RgbaImage) -> AppResult<Vec<OcrBlock>> {
    let st = state(app);
    let (slot, failures, dir) = match kind {
        Engine::Rapid => (
            &st.ocr.rapid,
            &st.ocr.failures,
            rapid::engine_dir(&st.paths).ok_or_else(|| AppError::msg("没有找到 RapidOCR 引擎"))?,
        ),
        Engine::Paddle => (
            &st.ocr.paddle,
            &st.ocr.paddle_failures,
            paddle::engine_dir(&st.paths)
                .ok_or_else(|| AppError::msg("还没有下载 PaddleOCR 引擎"))?,
        ),
    };
    if failures.load(Ordering::SeqCst) >= MAX_CONSECUTIVE_FAILURES {
        return Err(AppError::msg(
            "识字引擎连续启动失败，已停用（重启 DATO OCR 可重试）",
        ));
    }
    let mut guard = slot.lock();
    if guard.as_mut().is_some_and(|e| !e.alive()) {
        tracing::warn!("识字引擎进程已退出，重新启动");
        *guard = None;
    }
    if guard.is_none() {
        let started = Instant::now();
        let spawned = match kind {
            Engine::Rapid => RapidEngine::spawn(&dir),
            Engine::Paddle => RapidEngine::spawn_paddle(&dir),
        };
        match spawned {
            Ok(engine) => {
                tracing::info!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "识字引擎已启动"
                );
                *guard = Some(engine);
            }
            Err(err) => {
                failures.fetch_add(1, Ordering::SeqCst);
                return Err(err);
            }
        }
    }
    let engine = guard
        .as_mut()
        .ok_or_else(|| AppError::msg("识字引擎不可用"))?;
    match engine.recognize(image, &st.paths.temp_file("bmp")) {
        Ok(blocks) => {
            failures.store(0, Ordering::SeqCst);
            Ok(blocks)
        }
        Err(err) => {
            // 超时或协议错乱：杀掉，下次请求自动重启
            if let Some(mut e) = guard.take() {
                e.kill();
            }
            failures.fetch_add(1, Ordering::SeqCst);
            Err(err)
        }
    }
}

fn run_system(image: &RgbaImage) -> AppResult<Vec<OcrBlock>> {
    let lines = platform::system_ocr(image)?;
    Ok(lines
        .into_iter()
        .map(|l| OcrBlock {
            text: l.text,
            quad: [
                [l.x, l.y],
                [l.x + l.width, l.y],
                [l.x + l.width, l.y + l.height],
                [l.x, l.y + l.height],
            ],
            score: 1.0,
        })
        .collect())
}

/// 识别一张图。`upscale` 为 true 时先放大两倍（小字更准，规格 04 §4"提高分辨率重试"）。
pub fn recognize(
    app: &AppHandle,
    image: &RgbaImage,
    engine: Option<&str>,
    upscale: bool,
) -> AppResult<OcrResult> {
    ensure_reaper(app);
    let st = state(app);
    *st.ocr.last_used.lock() = Some(Instant::now());
    let preferred = engine
        .map(str::to_string)
        .unwrap_or_else(|| st.settings.read().ocr.engine.clone());

    let scaled;
    let (input, factor) = if upscale && image.width() * image.height() < 4_000_000 {
        scaled = image::imageops::resize(
            image,
            image.width() * 2,
            image.height() * 2,
            image::imageops::FilterType::CatmullRom,
        );
        (&scaled, 2.0)
    } else {
        (image, 1.0)
    };

    let started = Instant::now();
    let paddle_result = (preferred == "paddle").then(|| run_engine(app, Engine::Paddle, input));
    let (mut blocks, used) = if preferred == "system" {
        (run_system(input)?, "Windows OCR")
    } else if let Some(Ok(b)) = paddle_result {
        (b, "PaddleOCR")
    } else {
        if let Some(Err(err)) = &paddle_result {
            tracing::warn!("PaddleOCR 失败，改用 RapidOCR：{err}");
        }
        match run_engine(app, Engine::Rapid, input) {
            Ok(b) => (b, "RapidOCR"),
            Err(err) => {
                tracing::warn!("RapidOCR 失败，改用系统 OCR：{err}");
                (run_system(input).map_err(|_| err)?, "Windows OCR")
            }
        }
    };
    if factor != 1.0 {
        for b in &mut blocks {
            for p in &mut b.quad {
                p[0] /= factor;
                p[1] /= factor;
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;
    tracing::info!(engine = used, blocks = blocks.len(), elapsed_ms, "识字完成");
    let r = reflow::reflow(&blocks);
    Ok(OcrResult {
        blocks,
        paragraphs: r.paragraphs,
        plain_text: r.plain_text,
        raw_text: r.raw_text,
        engine: used.to_string(),
        elapsed_ms,
    })
}

// ───────────────────────── 识字窗口任务 ─────────────────────────

pub fn ensure_window(app: &AppHandle) -> AppResult<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window(WINDOW) {
        return Ok(w);
    }
    let window = wm::builder(app, WINDOW)
        .title("文字识别 - DATO OCR")
        .inner_size(960.0, 640.0)
        .min_inner_size(680.0, 440.0)
        .resizable(true)
        .transparent(true)
        .shadow(true)
        .center()
        .build()?;
    wm::apply_window_effects(app, &window);
    Ok(window)
}

fn show_window(app: &AppHandle) {
    let app = app.clone();
    let _ = app
        .clone()
        .run_on_main_thread(move || match ensure_window(&app) {
            Ok(w) => {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
            Err(err) => tracing::error!("打开识字窗口失败：{err}"),
        });
}

fn publish(app: &AppHandle, job: OcrJob) {
    let st = state(app);
    st.ocr.jobs.lock().insert(job.id.clone(), job.clone());
    *st.ocr.current.lock() = Some(job.id.clone());
    let _ = app.emit_to(WINDOW, events::OCR_JOB, &job);
}

pub fn current_job(app: &AppHandle) -> Option<OcrJob> {
    let st = state(app);
    let id = st.ocr.current.lock().clone()?;
    let job = st.ocr.jobs.lock().get(&id).cloned();
    job
}

/// 截图 / 贴图 / 剪贴板图片 → 打开识字窗口并开始识别。
pub fn open_job(
    app: &AppHandle,
    image: RgbaImage,
    screenshot_id: Option<i64>,
    translate: bool,
) -> AppResult<()> {
    let st = state(app);
    let rel = st.paths.new_rel_file("ocr", "png")?;
    std::fs::write(st.paths.abs(&rel), imaging::encode_png(&image)?)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let job = OcrJob {
        id: id.clone(),
        status: "running".into(),
        image_path: rel.clone(),
        width: image.width(),
        height: image.height(),
        translate,
        record_id: None,
        result: None,
        error: None,
        copied: false,
    };
    st.ocr.jobs.lock().clear();
    publish(app, job.clone());
    show_window(app);
    run_job(app, job, image, screenshot_id, None, false);
    Ok(())
}

/// 重新识别（换引擎 / 放大重试）。
pub fn rerun(
    app: &AppHandle,
    job_id: &str,
    engine: Option<String>,
    upscale: bool,
) -> AppResult<()> {
    let st = state(app);
    let mut job = st
        .ocr
        .jobs
        .lock()
        .get(job_id)
        .cloned()
        .ok_or_else(|| AppError::NotFound("识字任务".into()))?;
    let image = imaging::load(&st.paths.abs(&job.image_path))?;
    job.status = "running".into();
    job.result = None;
    job.error = None;
    publish(app, job.clone());
    run_job(app, job, image, None, engine, upscale);
    Ok(())
}

fn run_job(
    app: &AppHandle,
    mut job: OcrJob,
    image: RgbaImage,
    screenshot_id: Option<i64>,
    engine: Option<String>,
    upscale: bool,
) {
    let app = app.clone();
    std::thread::spawn(move || {
        let st = state(&app);
        match recognize(&app, &image, engine.as_deref(), upscale) {
            Ok(result) => {
                let record = repo::NewOcrRecord {
                    screenshot_id,
                    image_path: Some(job.image_path.clone()),
                    width: image.width(),
                    height: image.height(),
                    engine: result.engine.clone(),
                    raw_blocks: serde_json::to_string(&result.blocks)
                        .unwrap_or_else(|_| "[]".into()),
                    plain_text: result.plain_text.clone(),
                    lang: None,
                    elapsed_ms: result.elapsed_ms,
                };
                let saved = match job.record_id {
                    Some(id) => st
                        .db
                        .with(|c| repo::update_text(c, id, &result.plain_text))
                        .map(|_| id),
                    None => st.db.with(|c| repo::insert(c, &record)),
                };
                match saved {
                    Ok(id) => job.record_id = Some(id),
                    Err(err) => tracing::warn!("保存识字记录失败：{err}"),
                }
                if let Some(sid) = screenshot_id {
                    let _ = st
                        .db
                        .with(|c| screenshots::set_ocr_text(c, sid, &result.plain_text));
                }
                job.copied = auto_copy(&app, &result);
                job.status = "done".into();
                job.result = Some(result);
                let _ = app.emit(events::OCR_HISTORY_CHANGED, ());
            }
            Err(err) => {
                job.status = "error".into();
                job.error = Some(err.to_string());
            }
        }
        // 用户可能已经开了新任务，旧任务的结果只更新缓存不抢显示
        let is_current = st.ocr.current.lock().as_deref() == Some(job.id.as_str());
        st.ocr.jobs.lock().insert(job.id.clone(), job.clone());
        if is_current {
            let _ = app.emit_to(WINDOW, events::OCR_JOB, &job);
        }
    });
}

/// 设置里开着"识别后自动复制"就把文字写进剪贴板（格式和识字窗口里"复制全部"一样）。返回复制了没有。
fn auto_copy(app: &AppHandle, result: &OcrResult) -> bool {
    let (on, keep_breaks) = {
        let st = state(app);
        let s = st.settings.read();
        (s.ocr.auto_copy, s.ocr.keep_line_breaks)
    };
    if !on || std::env::var("CHENOCR_TEST_NO_CLIPBOARD").is_ok_and(|v| v == "1") {
        return false;
    }
    let text = if keep_breaks {
        result
            .paragraphs
            .iter()
            .map(|p| {
                p.lines
                    .iter()
                    .map(|l| l.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        result.plain_text.clone()
    };
    if text.trim().is_empty() {
        return false;
    }
    match platform::clipboard_write(&platform::ClipboardPayload::Text {
        text,
        html: None,
        rtf: None,
    }) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!("识字结果自动复制失败：{err}");
            false
        }
    }
}

/// 从识字记录重新打开（主窗口「识字记录」页）。
pub fn open_record(app: &AppHandle, record_id: i64) -> AppResult<()> {
    let st = state(app);
    let rec = st.db.with(|c| repo::get(c, record_id))?;
    let blocks: Vec<OcrBlock> = serde_json::from_str(&rec.raw_blocks).unwrap_or_default();
    let r = reflow::reflow(&blocks);
    let job = OcrJob {
        id: uuid::Uuid::new_v4().simple().to_string(),
        status: "done".into(),
        image_path: rec.image_path.clone().unwrap_or_default(),
        width: rec.width.unwrap_or(0) as u32,
        height: rec.height.unwrap_or(0) as u32,
        translate: false,
        record_id: Some(rec.id),
        result: Some(OcrResult {
            blocks,
            paragraphs: r.paragraphs,
            // 用户可能编辑过文字，以库里存的为准
            plain_text: rec.plain_text,
            raw_text: r.raw_text,
            engine: rec.engine,
            elapsed_ms: rec.elapsed_ms.unwrap_or(0) as u64,
        }),
        error: None,
        copied: false,
    };
    publish(app, job);
    show_window(app);
    Ok(())
}

pub fn delete_record(app: &AppHandle, id: i64) -> AppResult<()> {
    let st = state(app);
    if let Some(file) = st.db.with(|c| repo::delete(c, id))? {
        let _ = std::fs::remove_file(st.paths.abs(&file));
    }
    let _ = app.emit(events::OCR_HISTORY_CHANGED, ());
    Ok(())
}
