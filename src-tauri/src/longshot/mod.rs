//! 长截图（规格 03）：**只做手动滚动**（铁律 5）。
//!
//! 框好区域后，遮罩变成鼠标穿透、底图隐藏，露出真实桌面；用户自己滚动滚轮。全局低级
//! 钩子听到滚轮就开始**连续拍**（一轮滚动），每一帧都交给拼接器跟踪位移，所以可以一口气
//! 从头滚到底，不用滚一下停一下。滚轮停了、画面也不动了，再按当前画面补一帧收尾，然后
//! 歇着等下一次滚轮。
//!
//! 遮罩穿透后拿不到键盘焦点，所以 Enter / Esc / Backspace 由钩子截获。提示条和预览条
//! 上的按钮要能点：鼠标移进它们的区域时临时关掉穿透，移出再打开。
//!
//! 连续拍下来的帧绝大多数只用来跟踪，拼接器说要留的（长图用得上的）才存：原图存在临时
//! 目录（每帧几 MB，全放内存会超过 200MB 的增长上限），只在最终合成时读回；预览用的是
//! 这些帧的缩略图。

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
use crate::platform::{
    self, HookEvent, HookKey, InputHookGuard, MonitorInfo, PhysicalRect, ScreenRecorder,
};
use crate::state::state;
use crate::{editor, events, imaging, library, wm};
use stitch::{Motion, Stitcher};

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
    /// 最近跟踪上、还没存盘的那一帧：拼接器之后说要留（`Fed::keep_prev`）才存
    held: Option<(usize, RgbaImage)>,
    /// 连续几轮滚动都是跟丢收场
    failures: u32,
    /// 连续几轮滚轮后画面都没动（无限滚动的页面在加载下一批时也会这样）
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
    /// 第一帧算 1，之后每轮带来新内容的滚动加 1（一轮 = 一个撤销点）
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    /// first | added | revisited | bottom | failed | toolong | undone
    /// （added / failed 在一轮滚动途中也会发）
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
        held: None,
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
            // 采集期间遮罩一直显示着：提示条、预览条不能被拍进长图
            platform::set_overlay_capturable(w, false);
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
    let quiet = Duration::from_millis(settings.scroll_debounce_ms);
    std::thread::Builder::new()
        .name("longshot".into())
        .spawn(move || worker(worker_app, rx, quiet))?;
    Ok(())
}

/// 收尾时最多补拍几次。很多控件（浏览器、RichEdit、WinUI）是平滑滚动，滚轮事件停了
/// 画面还在动；补拍到画面不再变化为止，最后一屏才不会漏。
const MAX_SETTLE_STEPS: u8 = 8;
/// 一轮滚动里等下一帧画面最多等多久
const FRAME_WAIT: Duration = Duration::from_millis(30);
/// 滚轮停了这么久画面还在变（视频、动图），就不等它停了
const ROUND_CAP: Duration = Duration::from_millis(1500);
/// 一轮滚动途中多久刷新一次预览条
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

#[derive(PartialEq)]
enum Flow {
    Continue,
    Stop,
}

fn worker(app: AppHandle, rx: Receiver<HookEvent>, quiet: Duration) {
    // 等底图隐藏、遮罩换好界面再拍第一帧
    std::thread::sleep(Duration::from_millis(180));
    if let Some(frame) = grab(&app) {
        feed(&app, frame, true);
    }
    emit_progress(&app, "first");
    // 连续抓屏的会话一进来就开好：等第一下滚轮再开的话，开的那一百来毫秒里页面已经滚走了，
    // 用力一拨就直接跟丢
    let recorder = start_recorder(&app);
    loop {
        if !state(&app).longshot.is_active() {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(HookEvent::Wheel { .. }) => {
                if round(&app, &rx, recorder.as_ref(), quiet) == Flow::Stop {
                    break;
                }
            }
            Ok(event) => {
                if handle(&app, event) == Flow::Stop {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn start_recorder(app: &AppHandle) -> Option<ScreenRecorder> {
    let (monitor, local) = {
        let st = state(app);
        let guard = st.longshot.active.lock();
        let a = guard.as_ref()?;
        (a.monitor.id, a.local)
    };
    ScreenRecorder::start(monitor, local)
        // 开不了就一张一张抓，慢一些（滚快了容易跟丢），但能用
        .inspect_err(|err| tracing::warn!("长截图：连续抓屏开不了，改成逐张抓：{err}"))
        .ok()
}

/// 滚轮以外的钩子事件。
fn handle(app: &AppHandle, event: HookEvent) -> Flow {
    match event {
        HookEvent::Wheel { .. } => {}
        HookEvent::MouseMove { x, y } => update_interactive(app, x, y),
        HookEvent::Key(HookKey::Enter) => {
            let app = app.clone();
            std::thread::spawn(move || finish(&app));
            return Flow::Stop;
        }
        HookEvent::Key(HookKey::Escape) => {
            abort(app);
            return Flow::Stop;
        }
        HookEvent::Key(HookKey::Backspace) => undo(app),
    }
    Flow::Continue
}

/// 一轮滚动里发生过什么。
#[derive(Default)]
struct Round {
    moved: bool,
    grew: bool,
    /// 最近一帧没对上
    lost: bool,
}

impl Round {
    fn note(&mut self, fed: Option<Fed>) {
        let Some(fed) = fed else { return };
        match fed.motion {
            Motion::First => {}
            Motion::Moved => {
                self.moved = true;
                self.lost = false;
            }
            Motion::Still => self.lost = false,
            Motion::Lost => self.lost = true,
        }
        self.grew |= fed.stored;
    }
}

/// 一轮滚动：从第一下滚轮开始连续拍，拍到滚轮停了、画面也停了为止。整轮算一个撤销点。
fn round(
    app: &AppHandle,
    rx: &Receiver<HookEvent>,
    recorder: Option<&ScreenRecorder>,
    quiet: Duration,
) -> Flow {
    {
        let st = state(app);
        let mut guard = st.longshot.active.lock();
        let Some(a) = guard.as_mut() else {
            return Flow::Stop;
        };
        a.stitcher.checkpoint();
    }
    let started = Instant::now();
    let (mut frames, mut lost_frames, mut first_lost) = (0u32, 0u32, None);
    let (mut t_wait, mut t_feed, mut t_emit) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let mut seen = Round::default();
    let mut last_wheel = Instant::now();
    let mut last_motion = Instant::now();
    let mut last_emit = Instant::now();
    let mut dirty = false;
    loop {
        while let Ok(event) = rx.try_recv() {
            match event {
                HookEvent::Wheel { .. } => last_wheel = Instant::now(),
                other => {
                    if handle(app, other) == Flow::Stop {
                        return Flow::Stop;
                    }
                }
            }
        }
        if !state(app).longshot.is_active() {
            return Flow::Stop;
        }
        let t0 = Instant::now();
        let frame = match recorder {
            Some(r) => r.next(FRAME_WAIT),
            None => {
                std::thread::sleep(FRAME_WAIT);
                grab(app)
            }
        };
        t_wait += t0.elapsed();
        if let Some(frame) = frame {
            let t0 = Instant::now();
            let fed = feed(app, frame, false);
            t_feed += t0.elapsed();
            if fed.is_some_and(|f| f.motion != Motion::Still) {
                last_motion = Instant::now();
            }
            frames += 1;
            if fed.is_some_and(|f| f.motion == Motion::Lost) {
                lost_frames += 1;
                first_lost.get_or_insert((frames, started.elapsed().as_millis() as u64));
            }
            let was_lost = seen.lost;
            seen.note(fed);
            dirty |= fed.is_some_and(|f| f.stored) || seen.lost != was_lost;
        }
        if dirty && last_emit.elapsed() >= PROGRESS_EVERY {
            let t0 = Instant::now();
            emit_progress(app, if seen.lost { "failed" } else { "added" });
            t_emit += t0.elapsed();
            last_emit = Instant::now();
            dirty = false;
        }
        let wheel_idle = last_wheel.elapsed();
        if wheel_idle >= ROUND_CAP || (wheel_idle >= quiet && last_motion.elapsed() >= quiet) {
            break;
        }
    }
    // 画面停了：按现在的样子补一帧。连续抓屏给的帧可能慢半拍，这里单独抓一张最新的；
    // 途中并入的画面里没加载出来的图，也靠这一帧换成加载好的
    if let Some(frame) = grab(app) {
        seen.note(feed(app, frame, true));
    }
    let status = {
        let st = state(app);
        let mut guard = st.longshot.active.lock();
        let Some(a) = guard.as_mut() else {
            return Flow::Stop;
        };
        tracing::info!(
            frames,
            ms = started.elapsed().as_millis() as u64,
            lost_frames,
            ?first_lost,
            wait_ms = t_wait.as_millis() as u64,
            feed_ms = t_feed.as_millis() as u64,
            emit_ms = t_emit.as_millis() as u64,
            recorder = recorder.is_some(),
            max_step = a.stitcher.max_step,
            content = a.local.height,
            height = a.stitcher.total_height(),
            ended_lost = seen.lost,
            "长截图：一轮滚动结束"
        );
        if a.stitcher.total_height() >= a.max_height {
            "toolong"
        } else if seen.lost {
            a.failures += 1;
            "failed"
        } else if seen.moved {
            a.failures = 0;
            a.still = 0;
            if seen.grew {
                "added"
            } else {
                "revisited"
            }
        } else {
            // 一轮没动可能只是页面在加载下一批内容，连续两轮才提示"到底了"
            a.still += 1;
            if a.still < 2 {
                return Flow::Continue;
            }
            "bottom"
        }
    };
    emit_progress(app, status);
    Flow::Continue
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

/// 单独抓一张选区当前的画面。
fn grab(app: &AppHandle) -> Option<RgbaImage> {
    let (monitor, local) = {
        let st = state(app);
        let guard = st.longshot.active.lock();
        let a = guard.as_ref()?;
        (a.monitor.id, a.local)
    };
    platform::capture_monitor(monitor)
        .and_then(|img| imaging::crop_opaque(&img, local))
        .inspect_err(|err| tracing::warn!("长截图抓帧失败：{err}"))
        .ok()
}

#[derive(Clone, Copy)]
struct Fed {
    motion: Motion,
    /// 长图用上了新的帧（预览要刷新）
    stored: bool,
}

/// 把一帧画面交给拼接器，并把它说要留的帧存下来。会话已结束或长图已到上限时返回 None。
/// `settled`：画面已经停稳（见 `Stitcher::feed`）。
fn feed(app: &AppHandle, frame: RgbaImage, settled: bool) -> Option<Fed> {
    let st = state(app);
    let mut guard = st.longshot.active.lock();
    let a = guard.as_mut()?;
    if a.stitcher.total_height() >= a.max_height {
        return None;
    }
    let index = a.next_index;
    a.next_index += 1;
    let fed = a.stitcher.feed(&frame, index, settled);
    let mut stored = false;
    let held = a.held.take();
    if let (Some(wanted), Some((i, image))) = (fed.keep_prev, held.as_ref()) {
        if wanted == *i {
            store(a, wanted, image);
            stored = true;
        }
    }
    if fed.keep {
        store(a, index, &frame);
        stored = true;
    } else if fed.motion == Motion::Lost {
        // 没对上的帧不要；手里那帧还是"最近跟踪上的"，留着
        a.held = held.filter(|(i, _)| fed.keep_prev != Some(*i));
    } else {
        a.held = Some((index, frame));
    }
    Some(Fed {
        motion: fed.motion,
        stored,
    })
}

fn store(a: &mut Active, index: usize, frame: &RgbaImage) {
    if let Err(err) = std::fs::write(frame_path(&a.dir, index), frame.as_raw()) {
        tracing::warn!("保存长截图帧失败：{err}");
    }
    let (tw, th) = (
        ((f64::from(frame.width()) * a.thumb_scale).round() as u32).max(1),
        ((f64::from(frame.height()) * a.thumb_scale).round() as u32).max(1),
    );
    a.thumbs
        .insert(index, image::imageops::thumbnail(frame, tw, th));
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
        a.held = None;
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
        let moved = grab(app)
            .and_then(|frame| feed(app, frame, true))
            .is_some_and(|fed| fed.motion == Motion::Moved);
        if !moved {
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
    // 截完就已经在剪贴板里了，不用再到编辑窗口里点一次复制（要标注的话标完再复制会盖掉这一份）
    let copied = platform::clipboard_write(&platform::ClipboardPayload::Image {
        image: image.clone(),
        png: None,
    });
    match editor::open(app, image, None) {
        Ok(doc_id) => {
            match copied {
                Ok(()) => wm::toast(app, "success", "长图已复制到剪贴板"),
                Err(err) => tracing::warn!("长图复制到剪贴板失败：{err}"),
            }
            if let Some(image) = to_library {
                if let Ok(id) = library::add(app, &image, "longshot", None, false) {
                    editor::attach_screenshot(&doc_id, id);
                }
            }
        }
        Err(err) => wm::toast(app, "error", format!("打开长图失败：{err}")),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use super::stitch::{Motion, Stitcher};
    use crate::imaging;
    use crate::platform::{self, PhysicalRect, ScreenRecorder};

    /// 真机烟雾测试：连续抓屏 + 拼接，不经过界面。先开一个能滚的窗口（`scripts/test/scrollwin.ps1`），
    /// 跑起来后在选区里一口气往下滚。打印每帧耗时、跟丢几次、存了几帧，长图存成 PNG 人眼看：
    /// `CHENOCR_SMOKE_RECT=x,y,w,h CHENOCR_SMOKE_OUT=路径 cargo test --profile perf -- --ignored smoke_continuous --nocapture`
    /// （选区是主屏内的物理像素；`CHENOCR_SMOKE_SECS` 默认 6 秒）
    #[test]
    #[ignore]
    fn smoke_continuous() {
        platform::init_process();
        let rect: Vec<i32> = std::env::var("CHENOCR_SMOKE_RECT")
            .unwrap()
            .split(',')
            .map(|v| v.trim().parse().unwrap())
            .collect();
        let local = PhysicalRect::new(rect[0], rect[1], rect[2] as u32, rect[3] as u32);
        let secs = std::env::var("CHENOCR_SMOKE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6);
        let monitor = platform::list_monitors().unwrap().remove(0);
        let grab = || {
            let full = platform::capture_monitor(monitor.id).unwrap();
            imaging::crop_opaque(&full, local).unwrap()
        };

        let mut stitcher = Stitcher::new();
        let mut kept = HashMap::new();
        let mut held = None;
        let mut index = 0usize;
        let (mut moved, mut still, mut lost) = (0u32, 0u32, 0u32);
        let mut spent = Duration::ZERO;
        let mut feed = |frame: image::RgbaImage, settled: bool| {
            let started = Instant::now();
            let fed = stitcher.feed(&frame, index, settled);
            spent += started.elapsed();
            match fed.motion {
                Motion::Moved => moved += 1,
                Motion::Still => still += 1,
                Motion::Lost => lost += 1,
                Motion::First => {}
            }
            let prev: Option<(usize, image::RgbaImage)> = held.take();
            if let (Some(want), Some((i, image))) = (fed.keep_prev, prev.as_ref()) {
                assert_eq!(want, *i);
                kept.insert(want, image.clone());
            }
            if fed.keep {
                kept.insert(index, frame);
            } else if fed.motion == Motion::Lost {
                held = prev;
            } else {
                held = Some((index, frame));
            }
            index += 1;
        };

        feed(grab(), true);
        let recorder = ScreenRecorder::start(monitor.id, local).unwrap();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(secs) {
            if let Some(frame) = recorder.next(super::FRAME_WAIT) {
                feed(frame, false);
            }
        }
        feed(grab(), true);
        let frames = index as u32;
        println!(
            "{frames} 帧（{:.0} 帧/秒），动 {moved} / 没动 {still} / 跟丢 {lost}，存了 {} 帧，拼接每帧 {:.1}ms，长图 {}×{}",
            f64::from(frames) / secs as f64,
            kept.len(),
            spent.as_secs_f64() * 1000.0 / f64::from(frames),
            stitcher.width(),
            stitcher.total_height(),
        );
        let image = stitcher.compose(|i| kept.get(&i).cloned()).unwrap();
        if let Ok(out) = std::env::var("CHENOCR_SMOKE_OUT") {
            image.save(&out).unwrap();
            println!("saved {out}");
        }
    }
}
