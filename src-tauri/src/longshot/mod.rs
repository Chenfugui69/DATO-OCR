//! 长截图（规格 03）：**只做手动滚动**（铁律 5）。
//!
//! 框好区域后，遮罩变成鼠标穿透、底图隐藏，露出真实桌面；用户自己滚动滚轮。全局低级
//! 钩子听到滚轮就开始**连续跟拍**：滚动过程中不停抓帧（Windows 用 WGC 连续录屏，只拷贝
//! 最新一帧），交给拼接器挑关键帧并入（见 `stitch` 模块文档）。不用滚一下停一下，
//! 一口气从头滚到尾就行；滚轮停下、画面停稳后这一次滚动才算结束（撤销按一次滚动算）。
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
use stitch::{Committed, Motion, Step, Stitcher};

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
        // 这样选区落在 DATO OCR 自己的窗口上也能找对。
        let target = platform::window_at(cx, cy).or(fallback);
        if let Some(target) = target {
            if let Err(err) = platform::route_scroll_to(target) {
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

/// 收尾时最多补拍几次：用户滚完立刻按 Enter，平滑滚动可能还在动。
const MAX_SETTLE_STEPS: u8 = 8;
/// 跟拍的最短间隔（抓帧 + 比对一次大约 20～40ms，实际帧率由它决定）
const SAMPLE_INTERVAL: Duration = Duration::from_millis(30);
/// 滚轮停了这么久还没停稳（画面里有动画、一直对不上）也结束这一次滚动
const MAX_GESTURE_TAIL: Duration = Duration::from_millis(1500);

/// 一次滚动（从第一下滚轮到画面停稳）。
struct Gesture {
    last_wheel: Instant,
    last_motion: Instant,
    /// 这次滚动里并入过新内容 / 拍到过已拍的位置
    added: bool,
    revisited: bool,
    /// 当前对不上（滚太快）
    lost: bool,
}

/// 抓帧来源：优先连续录屏（只拷贝最新一帧，比每次新建抓屏会话快得多），起不来就退回单次抓屏。
enum Grabber {
    Recorder(platform::ScreenRecorder),
    Snapshot(platform::MonitorId, PhysicalRect),
}

impl Grabber {
    /// 现在的画面；None = 上次取过之后没变化（或抓失败）。
    fn grab(&self) -> Option<RgbaImage> {
        match self {
            Self::Recorder(r) => r.latest(),
            Self::Snapshot(id, local) => grab_snapshot(*id, *local),
        }
    }
}

fn grab_snapshot(monitor: platform::MonitorId, local: PhysicalRect) -> Option<RgbaImage> {
    match platform::capture_monitor(monitor).and_then(|img| imaging::crop_opaque(&img, local)) {
        Ok(f) => Some(f),
        Err(err) => {
            tracing::warn!("长截图抓帧失败：{err}");
            None
        }
    }
}

fn worker(app: AppHandle, rx: Receiver<HookEvent>, debounce: Duration) {
    // 等底图隐藏、遮罩换好界面再拍第一帧
    std::thread::sleep(Duration::from_millis(180));
    let Some((monitor, local)) = state(&app)
        .longshot
        .active
        .lock()
        .as_ref()
        .map(|a| (a.monitor.id, a.local))
    else {
        return;
    };
    let grabber = match platform::ScreenRecorder::start(monitor, local) {
        Ok(r) => Grabber::Recorder(r),
        Err(err) => {
            tracing::warn!("长截图：连续录屏起不来，改用单次抓屏：{err}");
            Grabber::Snapshot(monitor, local)
        }
    };
    // 第一帧单独抓：画面静止时录屏不出帧
    if let Some(frame) = grab_snapshot(monitor, local) {
        feed(&app, frame, None);
    }
    let mut gesture: Option<Gesture> = None;
    let mut last_sample = Instant::now();
    loop {
        if !state(&app).longshot.is_active() {
            break;
        }
        let timeout = if gesture.is_some() {
            SAMPLE_INTERVAL
                .saturating_sub(last_sample.elapsed())
                .max(Duration::from_millis(1))
        } else {
            Duration::from_millis(250)
        };
        match rx.recv_timeout(timeout) {
            Ok(HookEvent::Wheel { .. }) => {
                let now = Instant::now();
                let g = gesture.get_or_insert_with(|| {
                    if let Some(a) = state(&app).longshot.active.lock().as_mut() {
                        a.stitcher.begin_group();
                    }
                    Gesture {
                        last_wheel: now,
                        last_motion: now,
                        added: false,
                        revisited: false,
                        lost: false,
                    }
                });
                g.last_wheel = now;
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
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let Some(g) = gesture.as_mut() else {
            // 没在滚动：把录屏积压的帧清掉
            if let Grabber::Recorder(r) = &grabber {
                r.drain();
            }
            continue;
        };
        // 滚轮事件很密的时候 recv 不会超时，按间隔在这里抓
        if last_sample.elapsed() < SAMPLE_INTERVAL {
            continue;
        }
        last_sample = Instant::now();
        if let Some(frame) = grabber.grab() {
            match feed(&app, frame, Some(&mut *g)) {
                Some(Motion::Still) => {}
                Some(_) => g.last_motion = Instant::now(),
                None => {
                    // 太长了，不再采集
                    gesture = None;
                    continue;
                }
            }
        }
        let quiet = g.last_wheel.elapsed();
        if (quiet >= debounce && g.last_motion.elapsed() >= debounce) || quiet >= MAX_GESTURE_TAIL {
            end_gesture(&app, g);
            gesture = None;
        }
    }
}

/// 喂一帧给拼接器，并入的帧落盘、刷新预览。返回 None 表示长图已经太长、不再采集。
fn feed(app: &AppHandle, frame: RgbaImage, gesture: Option<&mut Gesture>) -> Option<Motion> {
    let st = state(app);
    let (motion, status) = {
        let mut guard = st.longshot.active.lock();
        let a = guard.as_mut()?;
        if a.stitcher.total_height() >= a.max_height {
            drop(guard);
            emit_progress(app, "toolong");
            return None;
        }
        let fed = a.stitcher.feed(frame);
        let mut status = None;
        let mut steps = Vec::new();
        for c in fed.committed {
            steps.push(c.step);
            status = Some(keep(a, c));
        }
        if let Some(g) = gesture {
            for step in steps {
                match step {
                    Step::Added { .. } => g.added = true,
                    Step::Revisited => g.revisited = true,
                    _ => {}
                }
            }
            if fed.motion == Motion::Lost {
                if !g.lost {
                    // 刚对不上时提示一次；往回滚到能接上的位置会自己恢复
                    a.failures += 1;
                    status = Some("failed");
                }
                g.lost = true;
            } else {
                g.lost = false;
            }
        }
        (fed.motion, status)
    };
    if let Some(status) = status {
        emit_progress(app, status);
    }
    Some(motion)
}

/// 并入的帧：原图落盘、做缩略图，丢掉撤销也用不到的旧帧。返回要显示的状态。
fn keep(a: &mut Active, c: Committed) -> &'static str {
    if let Err(err) = std::fs::write(frame_path(&a.dir, c.index), c.frame.as_raw()) {
        tracing::warn!("保存长截图帧失败：{err}");
    }
    let (tw, th) = (
        ((f64::from(c.frame.width()) * a.thumb_scale).round() as u32).max(1),
        ((f64::from(c.frame.height()) * a.thumb_scale).round() as u32).max(1),
    );
    a.thumbs
        .insert(c.index, image::imageops::thumbnail(&c.frame, tw, th));
    let used = a.stitcher.referenced_frames();
    let dir = &a.dir;
    a.thumbs.retain(|i, _| {
        let keep = used.contains(i);
        if !keep {
            let _ = std::fs::remove_file(frame_path(dir, *i));
        }
        keep
    });
    a.failures = 0;
    a.still = 0;
    if a.stitcher.total_height() >= a.max_height {
        return "toolong";
    }
    match c.step {
        Step::First => "first",
        Step::Added { .. } => "added",
        _ => "revisited",
    }
}

/// 一次滚动结束：候选帧并入；什么都没变的话按"到底了 / 没对上"提示。
fn end_gesture(app: &AppHandle, g: &mut Gesture) {
    let st = state(app);
    let status = {
        let mut guard = st.longshot.active.lock();
        let Some(a) = guard.as_mut() else { return };
        if let Some(c) = a.stitcher.settle() {
            match c.step {
                Step::Added { .. } => g.added = true,
                Step::Revisited => g.revisited = true,
                _ => {}
            }
            Some(keep(a, c))
        } else if g.lost {
            // 停在了对不上的位置，提示已经发过了
            None
        } else if !g.added && !g.revisited {
            // 一次没动可能只是页面在加载下一批内容，连续两次才提示"到底了"
            a.still += 1;
            (a.still >= 2).then_some("bottom")
        } else {
            None
        }
    };
    if let Some(status) = status {
        emit_progress(app, status);
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
                let _ = platform::route_scroll_to(target);
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
        let t = a.thumbs.get(&a.stitcher.last_frame()?)?;
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
        if !a.stitcher.can_undo() {
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
    let target = state(app)
        .longshot
        .active
        .lock()
        .as_ref()
        .map(|a| (a.monitor.id, a.local));
    if let Some((monitor, local)) = target {
        for _ in 0..MAX_SETTLE_STEPS {
            let Some(frame) = grab_snapshot(monitor, local) else {
                break;
            };
            if feed(app, frame, None).is_none_or(|m| m == Motion::Still) {
                break;
            }
            std::thread::sleep(Duration::from_millis(120));
        }
        let mut g = Gesture {
            last_wheel: Instant::now(),
            last_motion: Instant::now(),
            added: true,
            revisited: false,
            lost: false,
        };
        end_gesture(app, &mut g);
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
