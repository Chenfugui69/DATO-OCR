//! 长截图（规格 03）：**只做手动滚动**（铁律 5）。
//!
//! 框好区域后，遮罩变成鼠标穿透、底图隐藏，露出真实桌面；用户自己滚动滚轮，全局低级
//! 钩子听到滚轮后防抖 160ms（等目标程序滚动动画结束）抓一帧、拼接、刷新预览。
//!
//! 遮罩穿透后拿不到键盘焦点，所以 Enter / Esc / Backspace 由钩子截获。提示条和预览条
//! 上的按钮要能点：鼠标移进它们的区域时临时关掉穿透，移出再打开。
//!
//! 帧原图存在临时目录（每帧几 MB，50 帧全放内存会超过 200MB 的增长上限），只在最终
//! 合成时读回；预览用的是每帧的缩略图。

pub mod stitch;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::RgbaImage;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::capture::{self, overlay, Session};
use crate::error::{AppError, AppResult};
use crate::platform::{self, HookEvent, HookKey, InputHookGuard, MonitorInfo, PhysicalRect};
use crate::state::state;
use crate::{editor, events, imaging, library, wm};
use stitch::{Step, Stitcher};

pub const PREVIEW_ID: &str = "longshot-preview";
/// 预览条宽度（逻辑像素）
const PREVIEW_WIDTH: f64 = 120.0;

#[derive(Default)]
pub struct LongshotState {
    active: Mutex<Option<Active>>,
}

impl LongshotState {
    pub fn is_active(&self) -> bool {
        self.active.lock().is_some()
    }
}

struct Active {
    session_id: u64,
    monitor: MonitorInfo,
    local: PhysicalRect,
    stitcher: Stitcher,
    dir: PathBuf,
    next_index: usize,
    thumbs: std::collections::HashMap<usize, RgbaImage>,
    thumb_scale: f64,
    failures: u32,
    /// 连续几次滚轮后画面都没动（无限滚动的页面在加载下一批时也会这样）
    still: u32,
    preview_version: u64,
    max_height: u32,
    /// 可点击的界面区域（屏幕物理坐标）
    regions: Vec<PhysicalRect>,
    interactive: bool,
    /// 被滚动的目标窗口：点过提示条按钮（焦点跑到遮罩上）之后要把焦点还给它
    target: Option<crate::platform::WindowHandle>,
    /// 截图时前台程序名（只采到一帧、按普通截图出图时记进截图库）
    source_app: Option<String>,
    _hook: InputHookGuard,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LongshotStateEvent {
    pub active: bool,
    pub monitor_id: u64,
    /// 本屏局部物理坐标
    pub rect: PhysicalRect,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    /// first | added | revisited | bottom | failed | toolong | undone
    pub status: &'static str,
    pub failures: u32,
    pub preview_version: u64,
    /// 最近接缝在预览图里的 y（预览像素）
    pub seam: Option<f64>,
}

pub fn start(
    app: &AppHandle,
    session: Arc<Session>,
    monitor: MonitorInfo,
    global: PhysicalRect,
) -> AppResult<()> {
    let st = state(app);
    if st.longshot.is_active() {
        return Err(AppError::msg("长截图已在进行中"));
    }
    let local = PhysicalRect::new(
        global.x - monitor.bounds.x,
        global.y - monitor.bounds.y,
        global.width,
        global.height,
    );
    if local.width < 40 || local.height < 80 {
        return Err(AppError::msg("区域太小，无法长截图"));
    }
    let dir = st.paths.temp().join(format!("longshot-{}", session.id));
    std::fs::create_dir_all(&dir)?;
    let (tx, rx) = std::sync::mpsc::channel();
    let hook = platform::install_input_hook(tx)?;
    let settings = st.settings.read().longshot.clone();
    let thumb_scale = (PREVIEW_WIDTH * monitor.scale_factor) / f64::from(local.width);
    // 选区中心的屏幕点：进入采集态后，鼠标穿透的遮罩下面那个窗口就是要滚动的目标
    let (cx, cy) = (
        global.x + global.width as i32 / 2,
        global.y + global.height as i32 / 2,
    );
    let fallback = session.previous;
    *st.longshot.active.lock() = Some(Active {
        session_id: session.id,
        monitor: monitor.clone(),
        local,
        stitcher: Stitcher::new(),
        dir,
        next_index: 0,
        thumbs: Default::default(),
        thumb_scale: thumb_scale.min(1.0),
        failures: 0,
        still: 0,
        preview_version: 0,
        max_height: settings.max_height,
        regions: Vec::new(),
        interactive: false,
        target: None,
        source_app: session.source_app.clone(),
        _hook: hook,
    });
    tracing::info!(session = session.id, ?local, "长截图开始");

    let ui_app = app.clone();
    let event = LongshotStateEvent {
        active: true,
        monitor_id: monitor.id.0,
        rect: local,
    };
    app.run_on_main_thread(move || {
        // 底图藏起来，露出真实桌面让用户滚动；遮罩全部鼠标穿透
        platform::backdrop::hide_all();
        overlay::for_each(&ui_app, |w| {
            let _ = w.set_ignore_cursor_events(true);
        });
        let _ = ui_app.emit(events::LONGSHOT_STATE, event);
        // 焦点必须交给被滚动的窗口：遮罩虽然鼠标穿透了，但还握着键盘焦点的话，"滚动非活动
        // 窗口"关闭的系统上滚轮会发给遮罩，页面纹丝不动。Enter/Esc/Backspace 由全局钩子
        // 截获，遮罩用不着焦点。目标用 WindowFromPoint 现取（遮罩此时已穿透、底图已隐藏），
        // 这样选区落在 DATO COR 自己的窗口上也能找对。
        let target = platform::window_at(cx, cy).or(fallback);
        if let Some(target) = target {
            if let Err(err) = platform::focus_window(target) {
                tracing::debug!("长截图：把焦点交给目标窗口失败：{err}");
            }
        }
        if let Some(active) = state(&ui_app).longshot.active.lock().as_mut() {
            active.target = target;
        }
    })?;

    let worker_app = app.clone();
    let debounce = Duration::from_millis(settings.scroll_debounce_ms);
    std::thread::Builder::new()
        .name("longshot".into())
        .spawn(move || worker(worker_app, rx, debounce))?;
    Ok(())
}

/// 滚轮停下后最多补拍几次。很多控件（浏览器、RichEdit、WinUI）是平滑滚动，
/// 滚轮事件停了画面还在动；补拍到画面不再变化为止，最后一屏才不会漏。
const MAX_SETTLE_STEPS: u8 = 8;

fn worker(app: AppHandle, rx: Receiver<HookEvent>, debounce: Duration) {
    // 等底图隐藏、遮罩换好界面再拍第一帧
    std::thread::sleep(Duration::from_millis(180));
    step(&app, false);
    let mut pending: Option<Instant> = None;
    // (上次拍摄时间, 已补拍次数)
    let mut settling: Option<(Instant, u8)> = None;
    loop {
        if !state(&app).longshot.is_active() {
            break;
        }
        let due = pending.or(settling.map(|(t, _)| t));
        let timeout = match due {
            Some(t) => debounce
                .saturating_sub(t.elapsed())
                .max(Duration::from_millis(1)),
            None => Duration::from_millis(250),
        };
        match rx.recv_timeout(timeout) {
            Ok(HookEvent::Wheel { .. }) => {
                pending = Some(Instant::now());
                settling = None;
            }
            Ok(HookEvent::MouseMove { x, y }) => update_interactive(&app, x, y),
            Ok(HookEvent::Key(HookKey::Enter)) => {
                let app = app.clone();
                std::thread::spawn(move || finish(&app));
                break;
            }
            Ok(HookEvent::Key(HookKey::Escape)) => {
                abort(&app);
                break;
            }
            Ok(HookEvent::Key(HookKey::Backspace)) => undo(&app),
            Err(RecvTimeoutError::Timeout) => {
                if pending.is_some_and(|t| t.elapsed() >= debounce) {
                    pending = None;
                    settling = step(&app, false).then(|| (Instant::now(), 0));
                } else if let Some((t, n)) = settling {
                    if t.elapsed() >= debounce {
                        let moved = step(&app, true);
                        settling =
                            (moved && n + 1 < MAX_SETTLE_STEPS).then(|| (Instant::now(), n + 1));
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn update_interactive(app: &AppHandle, x: i32, y: i32) {
    let st = state(app);
    let mut guard = st.longshot.active.lock();
    let Some(active) = guard.as_mut() else { return };
    let inside = active.regions.iter().any(|r| r.contains_point(x, y));
    if inside != active.interactive {
        active.interactive = inside;
        let label = overlay::label_for(active.monitor.id);
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.set_ignore_cursor_events(!inside);
        }
        // 离开按钮区域时把焦点还给被滚动的窗口（点过按钮后焦点会落在遮罩上）
        if !inside {
            if let Some(target) = active.target {
                let _ = platform::focus_window(target);
            }
        }
    }
}

pub fn set_regions(app: &AppHandle, regions: Vec<PhysicalRect>) {
    let st = state(app);
    if let Some(active) = st.longshot.active.lock().as_mut() {
        let origin = active.monitor.bounds;
        active.regions = regions
            .into_iter()
            .map(|r| PhysicalRect::new(r.x + origin.x, r.y + origin.y, r.width, r.height))
            .collect();
    };
}

fn frame_path(dir: &std::path::Path, index: usize) -> PathBuf {
    dir.join(format!("{index}.rgba"))
}

/// 抓一帧 → 拼接 → 刷新预览。返回画面是否有新内容（补拍据此决定要不要继续）。
/// quiet：补拍时"没变化 / 没对上"不算用户可见的状态，不刷新提示。
fn step(app: &AppHandle, quiet: bool) -> bool {
    let (monitor, local) = {
        let st = state(app);
        let guard = st.longshot.active.lock();
        let Some(a) = guard.as_ref() else {
            return false;
        };
        if a.stitcher.total_height() >= a.max_height {
            drop(guard);
            emit_progress(app, "toolong");
            return false;
        }
        (a.monitor.id, a.local)
    };
    let frame = match platform::capture_monitor(monitor)
        .and_then(|img| imaging::crop_opaque(&img, local))
    {
        Ok(f) => f,
        Err(err) => {
            tracing::warn!("长截图抓帧失败：{err}");
            return false;
        }
    };
    let st = state(app);
    let status = {
        let mut guard = st.longshot.active.lock();
        let Some(a) = guard.as_mut() else {
            return false;
        };
        let index = a.next_index;
        a.next_index += 1;
        let outcome = a.stitcher.push(&frame, index);
        match outcome {
            Step::First | Step::Added { .. } | Step::Revisited => {
                if let Err(err) = std::fs::write(frame_path(&a.dir, index), frame.as_raw()) {
                    tracing::warn!("保存长截图帧失败：{err}");
                }
                let (tw, th) = (
                    ((f64::from(frame.width()) * a.thumb_scale).round() as u32).max(1),
                    ((f64::from(frame.height()) * a.thumb_scale).round() as u32).max(1),
                );
                a.thumbs
                    .insert(index, image::imageops::thumbnail(&frame, tw, th));
                a.failures = 0;
                a.still = 0;
                if a.stitcher.total_height() >= a.max_height {
                    "toolong"
                } else {
                    match outcome {
                        Step::First => "first",
                        Step::Added { .. } => "added",
                        _ => "revisited",
                    }
                }
            }
            Step::NoChange if quiet => return false,
            Step::NoChange => {
                // 一次没动可能只是页面在加载下一批内容，连续两次才提示"到底了"
                a.still += 1;
                if a.still < 2 {
                    return false;
                }
                "bottom"
            }
            Step::Failed if quiet => return false,
            Step::Failed => {
                a.failures += 1;
                "failed"
            }
        }
    };
    emit_progress(app, status);
    matches!(status, "first" | "added" | "revisited")
}

fn build_preview(a: &Active) -> Option<RgbaImage> {
    let s = a.thumb_scale;
    let (header, footer) = a.stitcher.bands();
    let total = a.stitcher.total_height();
    let width = ((f64::from(a.stitcher.width()) * s).round() as u32).max(1);
    let height = ((f64::from(total) * s).round() as u32).max(1);
    let mut out = RgbaImage::new(width, height);
    let mut cursor = 0u32;
    let paste =
        |out: &mut RgbaImage, thumb: &RgbaImage, src_y: u32, rows: u32, cursor: &mut u32| {
            let ty = ((f64::from(src_y) * s).round() as u32).min(thumb.height());
            let th = ((f64::from(rows) * s).round() as u32).min(thumb.height() - ty);
            if th == 0 {
                return;
            }
            let part =
                image::imageops::crop_imm(thumb, 0, ty, thumb.width().min(width), th).to_image();
            image::imageops::replace(out, &part, 0, i64::from(*cursor));
            *cursor += th;
        };
    let first = a.stitcher.slices().map(|s| s.frame).min()?;
    if header > 0 {
        paste(&mut out, a.thumbs.get(&first)?, 0, header, &mut cursor);
    }
    for slice in a.stitcher.slices() {
        if let Some(t) = a.thumbs.get(&slice.frame) {
            paste(&mut out, t, slice.src_y, slice.height, &mut cursor);
        }
    }
    if footer > 0 {
        let last = a.thumbs.keys().max()?;
        let t = a.thumbs.get(last)?;
        let h = t.height();
        let fy = ((f64::from(footer) * s).round() as u32).min(h);
        let part = image::imageops::crop_imm(t, 0, h - fy, t.width(), fy).to_image();
        image::imageops::replace(
            &mut out,
            &part,
            0,
            i64::from(cursor.min(height.saturating_sub(fy))),
        );
    }
    Some(out)
}

fn emit_progress(app: &AppHandle, status: &'static str) {
    let st = state(app);
    let progress = {
        let mut guard = st.longshot.active.lock();
        let Some(a) = guard.as_mut() else { return };
        if let Some(preview) = build_preview(a) {
            st.images.put(PREVIEW_ID, Arc::new(preview));
            a.preview_version += 1;
        }
        Progress {
            frames: a.stitcher.frame_count(),
            width: a.stitcher.width(),
            height: a.stitcher.total_height(),
            status,
            failures: a.failures,
            preview_version: a.preview_version,
            seam: a.stitcher.last_seam.map(|y| f64::from(y) * a.thumb_scale),
        }
    };
    let _ = app.emit(events::LONGSHOT_PROGRESS, progress);
}

pub fn undo(app: &AppHandle) {
    {
        let st = state(app);
        let mut guard = st.longshot.active.lock();
        let Some(a) = guard.as_mut() else { return };
        if a.stitcher.frame_count() <= 1 {
            return;
        }
        a.stitcher.undo();
    }
    emit_progress(app, "undone");
}

fn take(app: &AppHandle) -> Option<Active> {
    let st = state(app);
    let active = st.longshot.active.lock().take();
    st.images.remove(PREVIEW_ID);
    active
}

pub fn abort(app: &AppHandle) {
    let Some(active) = take(app) else { return };
    let _ = std::fs::remove_dir_all(&active.dir);
    let session = active.session_id;
    drop(active);
    stop_ui(app);
    capture::end_session(app, session, true);
}

fn stop_ui(app: &AppHandle) {
    let _ = app.emit(
        events::LONGSHOT_STATE,
        LongshotStateEvent {
            active: false,
            monitor_id: 0,
            rect: PhysicalRect::default(),
        },
    );
}

pub fn finish(app: &AppHandle) {
    // 收尾前按当前画面补拍，直到画面停稳：用户滚完立刻按 Enter 时，平滑滚动可能还在动，
    // 最后一屏还没被拍到
    for _ in 0..MAX_SETTLE_STEPS {
        if !step(app, true) {
            break;
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    let Some(active) = take(app) else { return };
    let session = active.session_id;
    let (w, h) = (active.local.width, active.local.height);
    let dir = active.dir.clone();
    let single = active.stitcher.frame_count() == 1;
    let global = PhysicalRect::new(
        active.monitor.bounds.x + active.local.x,
        active.monitor.bounds.y + active.local.y,
        w,
        h,
    );
    let (scale, source_app) = (active.monitor.scale_factor, active.source_app.clone());
    let composed = active.stitcher.compose(|i| {
        let bytes = std::fs::read(frame_path(&dir, i)).ok()?;
        RgbaImage::from_raw(w, h, bytes)
    });
    drop(active);
    let _ = std::fs::remove_dir_all(&dir);
    stop_ui(app);
    capture::end_session(app, session, false);

    let Some(image) = composed else {
        wm::toast(app, "error", "还没有采集内容");
        return;
    };
    tracing::info!(width = image.width(), height = image.height(), "长截图完成");
    if single {
        // 只采到一帧（没滚动就按了 Enter）：等同普通截图，直接复制出图（规格 03 §6）
        let result = capture::perform(
            app,
            capture::FinishAction::Copy,
            Arc::new(image),
            global,
            scale,
            source_app,
            false,
        );
        if let Err(err) = result {
            wm::toast(app, "error", err.to_string());
        }
        return;
    }
    // 先开编辑窗口让用户看到图，再入库（长图编码 PNG 要一会儿）
    let to_library = state(app)
        .settings
        .read()
        .capture
        .save_to_library
        .then(|| image.clone());
    match editor::open(app, image, None) {
        Ok(doc_id) => {
            if let Some(image) = to_library {
                if let Ok(id) = library::add(app, &image, "longshot", None, false) {
                    editor::attach_screenshot(&doc_id, id);
                }
            }
        }
        Err(err) => wm::toast(app, "error", format!("打开长图失败：{err}")),
    }
}
