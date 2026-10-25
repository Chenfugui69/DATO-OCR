//! 录屏（GIF / APNG / MP4）：截图框好区域后点工具条上的「录制」，在录制栏里选好格式、效果，点开始。
//!
//! 和长截图一样：遮罩变成鼠标穿透、底图隐藏，露出真实桌面，用户在选区里正常操作；
//! 选区外面一圈红框和录制工具条（计时、停止、取消）。工具条要能点：录制线程每一拍顺便
//! 看鼠标在不在它上面，在就临时关掉遮罩的穿透。键盘不拦 —— 录的就是用户的操作，
//! 回车、Esc 都得照常给目标程序（开了按键显示时只是旁听，见 `platform::start_record_input`）。
//! 再按一次截图热键也能结束（前端收到 capture-hotkey 转发）。录制中能拖选区右下角的把手挪录制区域。
//!
//! 鼠标高亮、轨迹、点击涟漪、按着的键、滚动箭头、摄像头画面都直接画进每一帧（`record_fx`）。
//! 按键显示由页面画在选区角落：开了它时遮罩允许被抓屏，选区里除了按键显示什么都不画，
//! 红框和工具条都在选区外面。
//!
//! 两个线程：
//! - 录制：按帧率抓最新画面，裁出选区、画上效果和鼠标指针。画面、指针、效果都没变就不出帧
//!   （编码那边自动把上一帧拉长）
//! - 编码：边录边写进临时文件（GIF 或 APNG）。只编码和正在显示的画面相比变了的那一块（抓屏带的 ±1 抖动不算变），
//!   块里没变的像素设成透明（之前的内容留着），文件小、量化也快。编码跟不上时录制那边丢帧，不会越积越多
//!
//! 录完存到截图保存目录，并把文件放进剪贴板（能直接粘贴到聊天软件里）。
//!
//! 同一套流程也录视频（MP4，工具条上的"录制视频"）：录制线程完全一样，编码线程换成
//! `platform::VideoWriter`（macOS 是系统的硬件 H.264 / H.265 编码）。视频的分辨率、帧率、画质、
//! 编码器、最长时长、系统声音 / 麦克风、摄像头画面（圆形叠在角落）按设置里的"录制"来。
//!
//! 文件大小：GIF 默认装进 720p、10 帧，只编码变了的块；视频默认 1080p，码率按画质给（屏幕内容
//! 大块不动，码率可以给得很低），只在画面变了的时候出帧。
//!
//! 两个编码线程都顺手存一串小缩略图（每 250ms 一张），录完要剪辑时剪辑窗口拿来当时间轴。
//! 录完：设置里开了"录完先剪辑"就交给 `record_edit`（打开剪辑窗口），否则直接保存；
//! 视频有两条音轨（系统声音 + 麦克风）时先混成一条再保存。

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::RgbaImage;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::capture::{self, overlay, Session};
use crate::error::{AppError, AppResult};
use crate::platform::{self, ClipboardPayload, MonitorInfo, PhysicalRect, ScreenRecorder};
use crate::state::state;
use crate::{events, imaging, record_fx, wm};

/// GIF 最长录多久，到点自动结束（视频的按设置）
pub const MAX_SECONDS: u64 = 60;
/// GIF"原始"分辨率时的最长边（物理像素）。选区更大就等比缩小，GIF 太大哪儿都发不出去
const MAX_SIDE: u32 = 1600;
/// 编码队列长度。编码跟不上时多出来的帧直接丢掉
const QUEUE: usize = 6;
/// 视频的最长边（"原始"分辨率时再大的选区也等比缩到这么大）
const VIDEO_MAX_SIDE: u32 = 3840;
/// 缩略图（剪辑窗口的时间轴和预览）：多久存一张、最长边
const THUMB_EVERY_MS: u64 = 250;
const THUMB_SIDE: u32 = 360;

/// 录成什么
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordFormat {
    Gif,
    Mp4,
    /// 会动的 PNG（全彩）
    Apng,
}

impl RecordFormat {
    pub fn ext(self) -> &'static str {
        match self {
            Self::Gif => "gif",
            Self::Mp4 => "mp4",
            Self::Apng => "png",
        }
    }

    /// 设置里的写法；不认识的、这个系统上录不了的按 GIF
    pub fn parse(s: &str) -> Self {
        match s {
            "mp4" if cfg!(any(target_os = "macos", windows)) => Self::Mp4,
            "apng" => Self::Apng,
            _ => Self::Gif,
        }
    }

    /// 动图（GIF / APNG）：剪辑、进度条是重新编码一遍
    pub fn is_animation(self) -> bool {
        self != Self::Mp4
    }

    /// 给用户看的叫法
    pub fn label(self) -> &'static str {
        match self {
            Self::Gif => "GIF",
            Self::Mp4 => "视频",
            Self::Apng => "APNG",
        }
    }
}

const RUNNING: u8 = 0;
const FINISH: u8 = 1;
const CANCEL: u8 = 2;

struct Active {
    monitor: MonitorInfo,
    /// 录制区域现在在哪（本屏局部物理坐标）：开始时的选区，拖把手挪过就是挪到的地方
    rect: PhysicalRect,
    /// 控制栏等可点击区域（本屏局部物理坐标）
    regions: Vec<PhysicalRect>,
    interactive: bool,
    /// 遮罩现在能不能被录进去（按键显示、演示标注要能）
    capturable: bool,
    /// 录制中开了一次截图：遮罩归截图用，不切穿透（见 `suspend_ui`）
    suspended: bool,
    stop: Arc<AtomicU8>,
    /// 用户拖了把手、录制区域挪到哪（本屏局部物理坐标）；录制线程下一拍取走
    moved: Option<PhysicalRect>,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

pub fn is_active() -> bool {
    ACTIVE.lock().is_some()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GifStateEvent {
    pub active: bool,
    pub monitor_id: u64,
    /// 本屏局部物理坐标
    pub rect: PhysicalRect,
    /// 录完回到录制准备（`active` = false 时）：录制框留在 `rect`，接着录下一段 / 打开文件夹
    pub prep: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    elapsed_ms: u64,
    max_ms: u64,
    frames: u32,
}

enum Msg {
    /// 开始计时的那一刻（第 0 毫秒）。视频拿它对齐声音
    Origin(Instant),
    /// 一帧画面和它的时间（从开始录起的毫秒数）
    Frame(RgbaImage, u64),
    /// 录完了，把最后一帧写完（结束时间在这之前用一个 0×0 的空帧带过来）
    End,
}

/// 剪辑窗口的一张缩略图：JPEG
#[derive(Clone)]
pub struct Thumb {
    pub ms: u64,
    pub jpeg: Vec<u8>,
}

#[derive(Clone, Default)]
pub struct Stats {
    pub frames: u32,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    /// 视频里的音轨数（两条要混成一条）
    pub audio_tracks: usize,
    /// 没开成的声音（没权限之类）
    pub warnings: Vec<String>,
    pub thumbs: Vec<Thumb>,
}

/// 每 THUMB_EVERY_MS 存一张小缩略图
#[derive(Default)]
struct Thumbs {
    list: Vec<Thumb>,
    next_ms: u64,
}

impl Thumbs {
    fn offer(&mut self, img: &RgbaImage, ms: u64) {
        if ms < self.next_ms && !self.list.is_empty() {
            return;
        }
        self.next_ms = ms + THUMB_EVERY_MS;
        let (w, h) = img.dimensions();
        let k = (f64::from(THUMB_SIDE) / f64::from(w.max(h))).min(1.0);
        let (tw, th) = (
            ((f64::from(w) * k).round() as u32).max(1),
            ((f64::from(h) * k).round() as u32).max(1),
        );
        let small = image::imageops::thumbnail(img, tw, th);
        let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
        let mut jpeg = Vec::new();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 72);
        if rgb.write_with_encoder(enc).is_ok() {
            self.list.push(Thumb { ms, jpeg });
        }
    }
}

pub fn start(
    app: &AppHandle,
    session: Arc<Session>,
    monitor: MonitorInfo,
    global: PhysicalRect,
    format: RecordFormat,
) -> AppResult<()> {
    if is_active() {
        return Err(AppError::msg("已经在录制 GIF 了"));
    }
    // 准备阶段的穿透和键盘监听先当场收掉，下面换录制自己的
    crate::capture::record_prep::leave(app, false);
    let local = PhysicalRect::new(
        global.x - monitor.bounds.x,
        global.y - monitor.bounds.y,
        global.width,
        global.height,
    );
    if local.width < 16 || local.height < 16 {
        return Err(AppError::msg("区域太小，无法录制"));
    }
    let st = state(app);
    let settings = st.settings.read().capture.clone();
    let rec = st.settings.read().record.clone();
    let stop = Arc::new(AtomicU8::new(RUNNING));
    *ACTIVE.lock() = Some(Active {
        monitor: monitor.clone(),
        rect: local,
        regions: Vec::new(),
        interactive: false,
        capturable: rec.keystrokes,
        suspended: false,
        stop: stop.clone(),
        moved: None,
    });
    tracing::info!(session = session.id, ?local, ?format, "开始录制");

    let ui_app = app.clone();
    let event = GifStateEvent {
        active: true,
        monitor_id: monitor.id.0,
        rect: local,
        prep: false,
    };
    let (cx, cy) = (
        global.x + global.width as i32 / 2,
        global.y + global.height as i32 / 2,
    );
    let fallback = session.previous;
    // 开了按键显示：页面把按键画在选区角落，遮罩得能被抓进去（选区里别的什么都不画，红框、工具条在选区外面）
    let capturable = rec.keystrokes;
    app.run_on_main_thread(move || {
        // 底图藏起来露出真实桌面，遮罩全部鼠标穿透；焦点交给选区下面那个窗口，用户直接接着操作
        platform::backdrop::hide_all();
        overlay::for_each(&ui_app, |w| {
            let _ = w.set_ignore_cursor_events(true);
            // 平时录制期间遮罩不能被录进去（控制条、选区框）
            platform::set_overlay_capturable(w, capturable);
        });
        let _ = ui_app.emit(events::GIF_STATE, event);
        if let Some(target) = platform::window_at(cx, cy).or(fallback) {
            let _ = platform::focus_window(target);
        }
    })?;

    // 输出尺寸：太大就等比缩小；视频的宽高还得是偶数
    // 动图（GIF / APNG）用动图的清晰度
    let resolution = if format.is_animation() {
        &rec.gif_resolution
    } else {
        &rec.resolution
    };
    let k = output_scale(format, resolution, local.width, local.height);
    let mut out = (
        ((f64::from(local.width) * k).round() as u32).max(1),
        ((f64::from(local.height) * k).round() as u32).max(1),
    );
    if format == RecordFormat::Mp4 {
        out = ((out.0 & !1).max(2), (out.1 & !1).max(2));
    }
    // 录完回到录制准备，同一次截图里能接着录好几段（上一段可能还在编码）：文件名带上开始的时间
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let tmp = st
        .paths
        .temp()
        .join(format!("rec-{}-{stamp}.{}", session.id, format.ext()));
    let (tx, rx) = sync_channel::<Msg>(QUEUE);
    let enc_path = tmp.clone();
    let video_fps = rec.fps.clamp(5, 60);
    let options = platform::VideoOptions {
        system_audio: rec.system_audio,
        microphone: rec.microphone,
        encoding: video_encoding(&rec),
    };
    let looping = rec.gif_loop;
    let encoder = std::thread::Builder::new()
        .name("rec-encode".into())
        .spawn(move || match format {
            RecordFormat::Mp4 => encode_video(rx, &enc_path, out, video_fps, options),
            _ => encode(rx, &enc_path, out, format, looping),
        })?;
    // 摄像头画面：大小、形状、位置按设置（录制栏里拖过的位置）
    let camera = rec
        .camera
        .then(|| record_fx::CameraSpec::from_settings(&rec, monitor.scale_factor));

    let fx = record_fx::MouseFx::new(&rec, monitor.scale_factor);
    let ctx = RecordCtx {
        app: app.clone(),
        monitor,
        local,
        global,
        fps: if format.is_animation() {
            settings.gif_fps.clamp(5, 30)
        } else {
            video_fps
        },
        max: Duration::from_secs(if format.is_animation() {
            MAX_SECONDS
        } else {
            u64::from(rec.max_seconds.clamp(10, 3600))
        }),
        camera,
        fx,
        keystrokes: rec.keystrokes,
        edit: rec.edit_before_save,
        format,
        cursor: settings.gif_cursor,
        stop,
        tx,
        tmp,
        session_id: session.id,
    };
    std::thread::Builder::new()
        .name("gif-record".into())
        .spawn(move || record(ctx, encoder))?;
    Ok(())
}

/// 录制中拖了选区右下角的把手：录制区域挪到 `rect`（本屏局部物理坐标，大小不变）
pub fn move_to(rect: PhysicalRect) {
    if let Some(a) = ACTIVE.lock().as_mut() {
        let b = a.monitor.bounds;
        let x = rect.x.clamp(0, (b.width as i32 - rect.width as i32).max(0));
        let y = rect
            .y
            .clamp(0, (b.height as i32 - rect.height as i32).max(0));
        a.moved = Some(PhysicalRect::new(x, y, rect.width, rect.height));
        a.rect = PhysicalRect::new(x, y, rect.width, rect.height);
    }
}

pub fn set_regions(regions: Vec<PhysicalRect>) {
    if let Some(a) = ACTIVE.lock().as_mut() {
        let origin = a.monitor.bounds;
        a.regions = regions
            .into_iter()
            .map(|r| PhysicalRect::new(r.x + origin.x, r.y + origin.y, r.width, r.height))
            .collect();
    }
}

/// 录制中遮罩能不能被录进去：选区里有演示用的标注、或者开了按键显示时要能（不然录不到），
/// 都没有的时候不能（全屏录制时工具条压在选区里，别把它录进去）
pub fn set_capturable(app: &AppHandle, on: bool) {
    {
        let mut guard = ACTIVE.lock();
        let Some(a) = guard.as_mut() else { return };
        a.capturable = on;
        // 截图占着遮罩：截完 `resume_ui` 再按这个设
        if a.suspended {
            return;
        }
    }
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        overlay::for_each(&ui, |w| platform::set_overlay_capturable(w, on));
    });
}

/// 录完、保存。
pub fn finish() {
    if let Some(a) = ACTIVE.lock().as_ref() {
        let _ = a
            .stop
            .compare_exchange(RUNNING, FINISH, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// 不要了。
pub fn cancel() {
    if let Some(a) = ACTIVE.lock().as_ref() {
        a.stop.store(CANCEL, Ordering::SeqCst);
    }
}

struct RecordCtx {
    app: AppHandle,
    monitor: MonitorInfo,
    local: PhysicalRect,
    global: PhysicalRect,
    fps: u32,
    cursor: bool,
    stop: Arc<AtomicU8>,
    tx: SyncSender<Msg>,
    tmp: PathBuf,
    session_id: u64,
    format: RecordFormat,
    max: Duration,
    camera: Option<record_fx::CameraSpec>,
    /// 鼠标的效果（高亮、轨迹、涟漪……）
    fx: record_fx::MouseFx,
    /// 按键显示：按下的键转给页面
    keystrokes: bool,
    /// 录完先打开剪辑窗口
    edit: bool,
}

/// 设置里的视频编码（录制和剪辑导出都用）
pub fn video_encoding(rec: &crate::settings::RecordSettings) -> platform::VideoEncoding {
    platform::VideoEncoding {
        hevc: rec.codec == "hevc",
        quality: rec.quality,
        fps: rec.fps.clamp(5, 60),
    }
}

/// 输出比例：1080p / 720p / 480p 装进 1920×1080 / 1280×720 / 854×480（竖着的区域就装进 1080×1920）；
/// "原始"时 GIF 最长边 1600、视频最长边 3840。比上限小的不放大
fn output_scale(format: RecordFormat, resolution: &str, w: u32, h: u32) -> f64 {
    let (long, short) = (f64::from(w.max(h)), f64::from(w.min(h).max(1)));
    let k = match resolution {
        "1080p" => (1920.0 / long).min(1080.0 / short),
        "720p" => (1280.0 / long).min(720.0 / short),
        "480p" => (854.0 / long).min(480.0 / short),
        _ if format.is_animation() => f64::from(MAX_SIDE) / long,
        _ => f64::from(VIDEO_MAX_SIDE) / long,
    };
    k.min(1.0)
}

fn record(mut ctx: RecordCtx, encoder: std::thread::JoinHandle<AppResult<Stats>>) {
    let app = ctx.app.clone();
    // 等底图隐藏、遮罩换好界面
    std::thread::sleep(Duration::from_millis(150));
    let outcome = match ScreenRecorder::start(ctx.monitor.id, ctx.local) {
        Ok(recorder) => {
            run(&mut ctx, recorder);
            Ok(())
        }
        Err(err) => Err(err),
    };
    let cancelled = ctx.stop.load(Ordering::SeqCst) == CANCEL || outcome.is_err();
    // 发 End 让编码线程把最后一帧写完；取消时直接断开，编码线程收不到 End 就当作取消
    if !cancelled {
        let _ = ctx.tx.send(Msg::End);
    }
    drop(ctx.tx);

    // 录完（不是丢掉、没出错、不用先剪辑）：录制框留着，回到录制准备 —— 接着录下一段、打开文件夹看刚录的
    // 都方便，Esc / 关闭才收掉。丢掉、出错、要先剪辑（剪辑窗口别被遮罩压着）时整个收掉。
    // 界面先换，编码的收尾（GIF 可能还要一会儿）在后面等：这时已经能开始录下一段
    *ACTIVE.lock() = None;
    let back = !cancelled
        && !ctx.edit
        && state(&app)
            .capture
            .current()
            .is_some_and(|s| s.id == ctx.session_id);
    let _ = app.emit(
        events::GIF_STATE,
        GifStateEvent {
            active: false,
            monitor_id: if back { ctx.monitor.id.0 } else { 0 },
            rect: if back { ctx.local } else { PhysicalRect::default() },
            prep: back,
        },
    );
    if back {
        let ui = app.clone();
        let _ = app.run_on_main_thread(move || {
            // 录制中按设置切过遮罩进不进录像（平时不进）：回到准备，和截图开着时一样
            overlay::for_each(&ui, |w| platform::set_overlay_capturable(w, true));
        });
        if let Err(err) = capture::record_prep::enter(&app, ctx.session_id, ctx.monitor.id) {
            tracing::warn!("录完回到录制准备失败：{err}");
            capture::end_session(&app, ctx.session_id, false);
        }
    } else {
        capture::end_session(&app, ctx.session_id, false);
    }
    let result = encoder.join();

    if let Err(err) = outcome {
        tracing::error!("GIF 录制启动失败：{err}");
        wm::toast(&app, "error", format!("录制失败：{err}"));
        let _ = std::fs::remove_file(&ctx.tmp);
        return;
    }
    if cancelled {
        let _ = std::fs::remove_file(&ctx.tmp);
        return;
    }
    match result {
        Ok(Ok(stats)) => {
            for w in &stats.warnings {
                wm::toast(&app, "error", format!("这次没录到声音：{w}"));
            }
            if ctx.edit {
                crate::record_edit::open(&app, ctx.tmp.clone(), ctx.format, stats);
            } else {
                crate::record_edit::finalize(&app, &ctx.tmp, ctx.format, &stats, None);
            }
        }
        Ok(Err(err)) => {
            tracing::error!("GIF 编码失败：{err}");
            wm::toast(&app, "error", format!("GIF 编码失败：{err}"));
            let _ = std::fs::remove_file(&ctx.tmp);
        }
        Err(_) => {
            wm::toast(&app, "error", "GIF 编码线程异常退出");
            let _ = std::fs::remove_file(&ctx.tmp);
        }
    }
}

/// 录制循环：按帧率取最新画面，有变化才出帧。
fn run(ctx: &mut RecordCtx, mut recorder: ScreenRecorder) {
    let interval = Duration::from_micros(1_000_000 / u64::from(ctx.fps));
    let max = ctx.max;
    let camera = ctx
        .camera
        .as_ref()
        .and_then(|spec| match platform::Camera::start(spec.size) {
            Ok(cam) => Some(cam),
            Err(err) => {
                wm::toast(&ctx.app, "error", format!("这次没录摄像头：{err}"));
                None
            }
        });
    // 按键显示、滚动箭头要旁听键盘和滚轮（守卫丢掉就停）
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let _input = (ctx.keystrokes || !ctx.fx.is_off())
        .then(|| platform::start_record_input(input_tx).ok())
        .flatten();
    let started = Instant::now();
    let _ = ctx.tx.send(Msg::Origin(started));
    let mut next_tick = started;
    let mut clean: Option<RgbaImage> = None;
    let mut changed = false;
    let mut last_cursor: Option<(i32, i32)> = None;
    let mut frames = 0u32;
    let mut dropped = 0u32;
    let mut last_progress: Option<Instant> = None;
    loop {
        if ctx.stop.load(Ordering::SeqCst) != RUNNING {
            break;
        }
        if started.elapsed() >= max {
            let _ = ctx
                .stop
                .compare_exchange(RUNNING, FINISH, Ordering::SeqCst, Ordering::SeqCst);
            break;
        }
        // 用户拖了把手：录制区域换到新位置（大小不变），下一帧从新位置抓
        let moved = ACTIVE.lock().as_mut().and_then(|a| a.moved.take());
        if let Some(local) = moved {
            match ScreenRecorder::start(ctx.monitor.id, local) {
                Ok(r) => {
                    recorder = r;
                    ctx.local = local;
                    ctx.global = PhysicalRect::new(
                        ctx.monitor.bounds.x + local.x,
                        ctx.monitor.bounds.y + local.y,
                        local.width,
                        local.height,
                    );
                    clean = None;
                }
                Err(err) => tracing::warn!("挪录制区域失败：{err}"),
            }
        }
        // 等到下一拍，期间来的新画面只留最新的
        let wait = next_tick.saturating_duration_since(Instant::now());
        if let Some(img) = recorder.next(wait.max(Duration::from_millis(1))) {
            clean = Some(img);
            changed = true;
        }
        let now = Instant::now();
        if now < next_tick {
            continue;
        }
        next_tick += interval;
        if next_tick < now {
            next_tick = now + interval;
        }
        // 刚开始画面一直没变的话 WGC 不出帧，补抓一张当第一帧
        if clean.is_none() && started.elapsed() > Duration::from_millis(300) {
            if let Ok(img) = platform::capture_monitor(ctx.monitor.id)
                .and_then(|full| imaging::crop_opaque(&full, ctx.local))
            {
                clean = Some(img);
                changed = true;
            }
        }
        let cursor = platform::cursor_position();
        update_interactive(&ctx.app, cursor);
        // 键盘、滚轮：按键交给页面显示，滚动给效果画箭头
        for input in input_rx.try_iter() {
            match input {
                platform::RecordInput::Key(label) if ctx.keystrokes => {
                    let _ = ctx.app.emit(events::RECORD_KEY, label);
                }
                platform::RecordInput::Scroll(dy) => ctx.fx.scroll(now, dy),
                platform::RecordInput::Key(_) => {}
            }
        }
        let local_cursor = cursor
            .map(|(x, y)| (f64::from(x - ctx.global.x), f64::from(y - ctx.global.y)))
            .filter(|&(x, y)| {
                x >= 0.0
                    && y >= 0.0
                    && x < f64::from(ctx.local.width)
                    && y < f64::from(ctx.local.height)
            });
        ctx.fx.tick(now, local_cursor, platform::mouse_buttons());
        let Some(base) = clean.as_ref() else { continue };
        // 有摄像头画面时每一拍都出帧（人一直在动）；涟漪、轨迹没消完也是
        if changed
            || camera.is_some()
            || ctx.fx.animating(now)
            || (ctx.cursor && cursor != last_cursor)
        {
            let mut frame = base.clone();
            ctx.fx.draw_under(&mut frame, now);
            if ctx.cursor {
                platform::draw_cursor(&mut frame, (ctx.global.x, ctx.global.y));
            }
            ctx.fx.draw_over(&mut frame, now);
            if let (Some(cam), Some(spec)) = (camera.as_ref(), ctx.camera.as_ref()) {
                if let Some(img) = cam.latest() {
                    record_fx::draw_camera(&mut frame, &img, spec);
                }
            }
            let t = started.elapsed().as_millis() as u64;
            match ctx.tx.try_send(Msg::Frame(frame, t)) {
                Ok(()) => frames += 1,
                // 编码跟不上：丢这一帧，下一拍再说
                Err(TrySendError::Full(_)) => dropped += 1,
                Err(TrySendError::Disconnected(_)) => break,
            }
            changed = false;
            last_cursor = cursor;
        }
        if last_progress.is_none_or(|t| t.elapsed() >= Duration::from_millis(250)) {
            last_progress = Some(Instant::now());
            let _ = ctx.app.emit(
                events::GIF_PROGRESS,
                Progress {
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    max_ms: max.as_millis() as u64,
                    frames,
                },
            );
        }
    }
    tracing::info!(frames, dropped, fps = ctx.fps, format = ?ctx.format, "录制结束");
    // 最后一帧持续到按下"完成"的那一刻
    let _ = ctx.tx.send(Msg::Frame(
        RgbaImage::new(0, 0),
        started.elapsed().as_millis() as u64,
    ));
}

/// 录制中开了一次截图（录 DATO 自己的操作）：录制照常录，控制条先让位，遮罩归截图用
pub fn suspend_ui() {
    if let Some(a) = ACTIVE.lock().as_mut() {
        a.suspended = true;
        a.interactive = false;
    }
}

/// 那次截图结束：遮罩还给录制（穿透、底图藏起来），页面换回录制的界面。**UI 线程。**
pub fn resume_ui(app: &AppHandle) {
    let (event, capturable) = {
        let mut guard = ACTIVE.lock();
        let Some(a) = guard.as_mut() else { return };
        a.suspended = false;
        a.interactive = false;
        (
            GifStateEvent {
                active: true,
                monitor_id: a.monitor.id.0,
                rect: a.rect,
                prep: false,
            },
            a.capturable,
        )
    };
    platform::backdrop::hide_all();
    overlay::for_each(app, |w| {
        let _ = w.set_ignore_cursor_events(true);
        platform::set_overlay_capturable(w, capturable);
    });
    let _ = app.emit(events::GIF_STATE, event);
}

/// 鼠标在控制栏上时关掉遮罩的穿透，按钮才点得到；移开再打开。
fn update_interactive(app: &AppHandle, cursor: Option<(i32, i32)>) {
    let mut guard = ACTIVE.lock();
    let Some(a) = guard.as_mut() else { return };
    if a.suspended {
        return;
    }
    // 区域是本屏局部坐标，鼠标位置是全局的：副屏（原点不在 0,0）上要先换过来
    let b = a.monitor.bounds;
    let inside = cursor.is_some_and(|(x, y)| {
        a.regions
            .iter()
            .any(|r| r.contains_point(x - b.x, y - b.y))
    });
    if inside != a.interactive {
        a.interactive = inside;
        if let Some(w) = app.get_webview_window(&overlay::label_for(a.monitor.id)) {
            let _ = w.set_ignore_cursor_events(!inside);
        }
    }
}

// ───────────────────────── 编码 ─────────────────────────

fn gif_err(err: impl std::fmt::Display) -> AppError {
    AppError::msg(format!("GIF 编码失败：{err}"))
}

/// 动图的写入器：GIF 或 APNG。帧要等下一帧来了才知道该持续多久，所以都是"写上一帧"
enum AnimWriter {
    Gif(gif::Encoder<BufWriter<File>>),
    Apng(crate::apng::ApngWriter),
}

/// 编好、还没写的一帧：GIF 是量化好的帧（量化最费时，收到就做），APNG 是那一块的像素
enum PendingFrame {
    Gif(gif::Frame<'static>),
    Apng(crate::apng::ApngFrame),
}

impl AnimWriter {
    fn create(
        path: &Path,
        (w, h): (u32, u32),
        format: RecordFormat,
        looping: bool,
    ) -> AppResult<Self> {
        if format == RecordFormat::Apng {
            return Ok(Self::Apng(crate::apng::ApngWriter::create(
                path, w, h, looping,
            )?));
        }
        let file = BufWriter::new(File::create(path)?);
        let mut enc = gif::Encoder::new(file, w as u16, h as u16, &[]).map_err(gif_err)?;
        if looping {
            enc.set_repeat(gif::Repeat::Infinite).map_err(gif_err)?;
        }
        Ok(Self::Gif(enc))
    }

    /// 一块变化（`Delta`）做成这个格式的帧
    fn prepare(&self, d: Delta) -> PendingFrame {
        match self {
            Self::Gif(_) => {
                let mut buf = d.rgba;
                let mut f = gif::Frame::from_rgba_speed(
                    d.width as u16,
                    d.height as u16,
                    &mut buf,
                    speed_for((d.width * d.height) as usize),
                );
                f.left = d.x as u16;
                f.top = d.y as u16;
                f.dispose = gif::DisposalMethod::Keep;
                PendingFrame::Gif(f)
            }
            Self::Apng(_) => PendingFrame::Apng(crate::apng::ApngFrame {
                x: d.x,
                y: d.y,
                width: d.width,
                height: d.height,
                rgba: d.rgba,
            }),
        }
    }

    fn write(&mut self, frame: PendingFrame, ms: u64) -> AppResult<()> {
        match (self, frame) {
            (Self::Gif(enc), PendingFrame::Gif(f)) => write_frame(enc, f, ms),
            (Self::Apng(w), PendingFrame::Apng(f)) => w.write(&f, ms),
            _ => Err(AppError::msg("录制编码出错")),
        }
    }

    fn finish(self) -> AppResult<()> {
        match self {
            Self::Gif(enc) => {
                let mut file = enc.into_inner().map_err(gif_err)?;
                file.flush()?;
                Ok(())
            }
            Self::Apng(w) => w.finish().map(|_| ()),
        }
    }
}

/// 边录边编码（GIF / APNG）。收到空帧（0×0）表示录制结束、给出结束时间；随后的 End 才算真的写完。
/// 发送端断开却没收到 End = 取消。`looping`：循环播放（关掉 = 放一遍停在最后一帧）
fn encode(
    rx: Receiver<Msg>,
    path: &Path,
    (w, h): (u32, u32),
    format: RecordFormat,
    looping: bool,
) -> AppResult<Stats> {
    let mut enc = AnimWriter::create(path, (w, h), format, looping)?;
    // 现在显示的画面（下一帧和它比）
    let mut shown: Option<RgbaImage> = None;
    // 已经编好、还不知道该持续多久的那一帧，和它的开始时间
    let mut pending: Option<(PendingFrame, u64)> = None;
    let mut written = 0u32;
    let mut end_ms = 0u64;
    let mut done = false;
    let mut thumbs = Thumbs::default();
    for msg in rx {
        match msg {
            Msg::Origin(_) => {}
            Msg::Frame(img, t) if img.width() == 0 => end_ms = t,
            Msg::Frame(img, t) => {
                let img = if img.dimensions() == (w, h) {
                    img
                } else {
                    image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle)
                };
                let delta = if let Some(s) = shown.as_mut() {
                    delta_rect(s, &img)
                } else {
                    shown = Some(img.clone());
                    Some(full_rect(&img))
                };
                // 和上一帧一模一样：不出新帧，上一帧自然拉长
                let Some(delta) = delta else { continue };
                let frame = enc.prepare(delta);
                thumbs.offer(&img, t);
                if let Some((f, t0)) = pending.take() {
                    enc.write(f, t.saturating_sub(t0))?;
                    written += 1;
                }
                pending = Some((frame, t));
            }
            Msg::End => {
                done = true;
                break;
            }
        }
    }
    if !done {
        return Err(AppError::msg("已取消"));
    }
    let Some((f, t0)) = pending.take() else {
        return Err(AppError::msg("没有录到画面"));
    };
    enc.write(f, end_ms.saturating_sub(t0).max(100))?;
    written += 1;
    enc.finish()?;
    Ok(Stats {
        frames: written,
        duration_ms: end_ms,
        width: w,
        height: h,
        thumbs: thumbs.list,
        ..Default::default()
    })
}

fn write_frame(
    enc: &mut gif::Encoder<BufWriter<File>>,
    mut frame: gif::Frame<'static>,
    ms: u64,
) -> AppResult<()> {
    // GIF 的时间单位是 10ms；小于 2 的浏览器会当成 10，反而变慢
    frame.delay = ((ms + 5) / 10).clamp(2, u64::from(u16::MAX)) as u16;
    enc.write_frame(&frame).map_err(gif_err)
}

/// 大块画面量化慢一点也没关系，但别拖住录制：面积大就用快一档的量化
fn speed_for(pixels: usize) -> i32 {
    if pixels > 800_000 {
        20
    } else {
        10
    }
}

/// 画面里变了的一块：位置、大小、像素（RGBA，没变的地方透明）
struct Delta {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// 第一帧：整张画面（不透明）
fn full_rect(img: &RgbaImage) -> Delta {
    let (w, h) = img.dimensions();
    let mut rgba = img.as_raw().clone();
    for px in rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }
    Delta {
        x: 0,
        y: 0,
        width: w,
        height: h,
        rgba,
    }
}

/// 每个通道差不超过这么多的像素不算变。抓屏时换色彩空间会带一点 ±1 的抖动，静止的画面相邻两帧
/// 也有一两千个像素差 1，散落在整个画面上 —— 不忽略的话每一帧的"变化范围"都被撑成整屏，静止的画面也整块
/// 重编（实测每帧比 Snow Shot 大十几倍）。差 3 以内肉眼看不出来，GIF 只有 256 色也表现不出来
const NOISE: u8 = 3;

/// 这个像素（`i` 是 RGBA 里的下标）和显示着的比算不算变了
fn changed(shown: &[u8], cur: &[u8], i: usize) -> bool {
    (0..3).any(|k| shown[i + k].abs_diff(cur[i + k]) > NOISE)
}

/// 和现在显示的画面（`shown`）比：只取变了的那块矩形，块里没变的像素设成透明（显示的还是之前的）。
/// 变了的像素写回 `shown`，没变的保持原样（慢慢变的颜色攒够了差值照样会更新）。完全没变返回 None。
fn delta_rect(shown: &mut RgbaImage, cur: &RgbaImage) -> Option<Delta> {
    let (w, h) = cur.dimensions();
    let row = (w * 4) as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    {
        let (sw, cw) = (shown.as_raw(), cur.as_raw());
        for y in 0..h {
            let start = y as usize * row;
            if sw[start..start + row] == cw[start..start + row] {
                continue;
            }
            let Some(first) = (0..w).find(|&x| changed(sw, cw, start + x as usize * 4)) else {
                continue;
            };
            let last = (0..w)
                .rev()
                .find(|&x| changed(sw, cw, start + x as usize * 4))
                .unwrap_or(first);
            x0 = x0.min(first);
            x1 = x1.max(last);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
    }
    if x1 < x0 || y1 < y0 {
        return None;
    }
    let (sw, sh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut buf = Vec::with_capacity((sw * sh * 4) as usize);
    let cw = cur.as_raw();
    let shown: &mut [u8] = shown;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let i = (y * w + x) as usize * 4;
            if changed(shown, cw, i) {
                buf.extend_from_slice(&[cw[i], cw[i + 1], cw[i + 2], 255]);
                shown[i..i + 3].copy_from_slice(&cw[i..i + 3]);
            } else {
                buf.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    Some(Delta {
        x: x0,
        y: y0,
        width: sw,
        height: sh,
        rgba: buf,
    })
}

/// 视频编码线程：帧缩到输出尺寸（视频要偶数宽高，多半只是裁掉一个像素），交给 VideoWriter。
/// 和 GIF 一样：空帧（0×0）带结束时间，End 才算写完；没收到 End 就断开 = 取消。
fn encode_video(
    rx: Receiver<Msg>,
    path: &Path,
    (w, h): (u32, u32),
    fps: u32,
    options: platform::VideoOptions,
) -> AppResult<Stats> {
    let mut writer = platform::VideoWriter::create(path, w, h, fps, options)?;
    let warnings = writer.warnings().to_vec();
    let mut frames = 0u32;
    let mut end_ms = 0u64;
    let mut thumbs = Thumbs::default();
    loop {
        match rx.recv() {
            Ok(Msg::Origin(at)) => writer.start(at),
            Ok(Msg::Frame(img, t)) => {
                if img.width() == 0 {
                    end_ms = t;
                    continue;
                }
                // 只差一两个像素（凑偶数）就直接裁，否则等比缩
                let fitted;
                let frame = if img.width() < w + 2 && img.height() < h + 2 {
                    &img
                } else {
                    fitted =
                        image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
                    &fitted
                };
                writer.push(frame, t)?;
                thumbs.offer(frame, t);
                frames += 1;
                end_ms = end_ms.max(t);
            }
            Ok(Msg::End) => break,
            // 取消：文件不要了
            Err(_) => {
                drop(writer);
                let _ = std::fs::remove_file(path);
                return Err(AppError::msg("已取消"));
            }
        }
    }
    let audio_tracks = writer.finish(end_ms)?;
    Ok(Stats {
        frames,
        duration_ms: end_ms,
        width: w,
        height: h,
        audio_tracks,
        warnings,
        thumbs: thumbs.list,
    })
}

// ───────────────────────── 剪辑 / 加进度条（动图） ─────────────────────────

/// 动图多大
fn animation_size(path: &Path, format: RecordFormat) -> AppResult<(u32, u32)> {
    if format == RecordFormat::Apng {
        let reader = png::Decoder::new(std::io::BufReader::new(File::open(path)?))
            .read_info()
            .map_err(|e| AppError::msg(format!("读取 APNG 失败：{e}")))?;
        return Ok((reader.info().width, reader.info().height));
    }
    let dec = gif::DecodeOptions::new()
        .read_info(std::io::BufReader::new(File::open(path)?))
        .map_err(gif_err)?;
    Ok((u32::from(dec.width()), u32::from(dec.height())))
}

/// 一帧帧读动图（GIF / APNG）：每帧叠好之后的整张画面和它显示多久（毫秒）。`f` 返回 false 就不再读了
fn read_animation(
    path: &Path,
    format: RecordFormat,
    mut f: impl FnMut(&RgbaImage, u64) -> bool,
) -> AppResult<()> {
    if format == RecordFormat::Apng {
        return crate::apng::read_frames(path, f);
    }
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut dec = opts
        .read_info(std::io::BufReader::new(File::open(path)?))
        .map_err(gif_err)?;
    let (w, h) = (u32::from(dec.width()), u32::from(dec.height()));
    let mut canvas = RgbaImage::new(w, h);
    while let Some(frame) = dec.read_next_frame().map_err(gif_err)? {
        let (fx, fy) = (u32::from(frame.left), u32::from(frame.top));
        let restore = (frame.dispose == gif::DisposalMethod::Previous).then(|| canvas.clone());
        // 透明像素保留底下的内容（我们自己录的 GIF 每块都是这样叠的）
        for (i, px) in frame.buffer.chunks_exact(4).enumerate() {
            let x = fx + i as u32 % u32::from(frame.width);
            let y = fy + i as u32 / u32::from(frame.width);
            if px[3] != 0 && x < w && y < h {
                canvas.put_pixel(x, y, image::Rgba([px[0], px[1], px[2], 255]));
            }
        }
        if !f(&canvas, u64::from(frame.delay) * 10) {
            break;
        }
        match frame.dispose {
            gif::DisposalMethod::Background => {
                for y in fy..(fy + u32::from(frame.height)).min(h) {
                    for x in fx..(fx + u32::from(frame.width)).min(w) {
                        canvas.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                    }
                }
            }
            gif::DisposalMethod::Previous => {
                if let Some(prev) = restore {
                    canvas = prev;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// 送一帧去编码：给了进度条 / 播放时间就先画上
fn send_frame(
    tx: &SyncSender<Msg>,
    img: &RgbaImage,
    at: u64,
    decor: Option<&record_fx::Decor>,
    total: u64,
) -> bool {
    let mut frame = img.clone();
    if let Some(d) = decor {
        record_fx::draw_decor(&mut frame, d, at, total);
    }
    tx.send(Msg::Frame(frame, at)).is_ok()
}

/// 重新编码录好的动图：只留 `[start, end)` 一段、按倍速；`decor` 给了就每帧画上进度条 / 播放时间
/// （长时间不动的帧拆成一小段一小段，进度条才会走）。`total_ms` 是原来整段多长。
/// 还是只编码变了的块；帧间隔变得比 20ms 还短的帧合并掉（GIF 再短浏览器就当成 100ms 了）。
pub fn rework_animation(
    src: &Path,
    dst: &Path,
    format: RecordFormat,
    edit: platform::VideoEdit,
    total_ms: u64,
    looping: bool,
    decor: Option<record_fx::Decor>,
) -> AppResult<Stats> {
    let (w, h) = animation_size(src, format)?;
    let speed = edit.speed.clamp(0.25, 4.0);
    let (start, end) = (edit.start_ms, edit.end_ms.max(edit.start_ms + 1));
    let out_ms = |ms: u64| ((ms.saturating_sub(start)) as f64 / speed).round() as u64;
    let out_total = out_ms(end.min(total_ms.max(start + 1)));
    // 进度条每 100ms 走一格，播放时间每半秒换一次
    let step = match decor.map(|d| d.kind) {
        Some(record_fx::DecorKind::Bar) => 100,
        Some(record_fx::DecorKind::Time) => 500,
        None => u64::MAX,
    };
    let (tx, rx) = sync_channel::<Msg>(QUEUE);
    let out = dst.to_path_buf();
    let encoder = std::thread::Builder::new()
        .name("anim-edit".into())
        .spawn(move || encode(rx, &out, (w, h), format, looping))?;
    let mut t = 0u64;
    let mut last_sent: Option<u64> = None;
    // 开头之前的最后一帧：剪出来的第一帧要从它开始显示
    let mut before: Option<RgbaImage> = None;
    let deco = decor.as_ref();
    read_animation(src, format, |canvas, delay| {
        let (t0, t1) = (t, t + delay);
        t = t1;
        if t1 <= start {
            before = Some(canvas.clone());
            return true;
        }
        if t0 >= end {
            return false;
        }
        if let Some(first) = before.take() {
            if t0 > start {
                if !send_frame(&tx, &first, 0, deco, out_total) {
                    return false;
                }
                last_sent = Some(0);
            }
        }
        let (a, b) = (out_ms(t0.max(start)), out_ms(t1.min(end)));
        let mut at = a;
        loop {
            if !last_sent.is_some_and(|l| at < l + 20) {
                if !send_frame(&tx, canvas, at, deco, out_total) {
                    return false;
                }
                last_sent = Some(at);
            }
            at = at.saturating_add(step);
            if at >= b {
                break;
            }
        }
        true
    })?;
    if let Some(first) = before.take() {
        let _ = send_frame(&tx, &first, 0, deco, out_total);
    }
    let _ = tx.send(Msg::Frame(RgbaImage::new(0, 0), out_ms(end.min(t))));
    let _ = tx.send(Msg::End);
    drop(tx);
    encoder
        .join()
        .map_err(|_| AppError::msg("动图编码线程异常退出"))?
}

// ───────────────────────── 保存 ─────────────────────────

/// 存到截图保存目录、放进剪贴板、提示一句。`tmp` 会被挪走
pub fn save(app: &AppHandle, tmp: &Path, stats: &Stats, format: RecordFormat) -> AppResult<()> {
    let settings = state(app).settings.read().capture.clone();
    let dir = capture::save_directory(&settings);
    std::fs::create_dir_all(&dir)?;
    let stem = Path::new(&capture::file_name(&settings))
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "DATO OCR".into());
    let ext = format.ext();
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem}_{n}.{ext}"));
        n += 1;
    }
    // 临时目录可能和保存目录不在一个盘上，改名不行就复制
    if std::fs::rename(tmp, &path).is_err() {
        std::fs::copy(tmp, &path)?;
        let _ = std::fs::remove_file(tmp);
    }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    tracing::info!(
        frames = stats.frames,
        duration_ms = stats.duration_ms,
        width = stats.width,
        height = stats.height,
        size,
        ?format,
        "录制已保存"
    );
    // 截图库只认图片（GIF 也是图）；视频不进库
    if format == RecordFormat::Gif {
        if let Err(err) = crate::library::add_gif(app, &path) {
            tracing::warn!("GIF 存进截图库失败：{err}");
        }
    }
    // 诊断开关：自动化测试时不碰剪贴板（测试机的远程控制软件会把剪贴板同步到别的电脑）
    let skip_clipboard = std::env::var("CHENOCR_TEST_NO_CLIPBOARD").is_ok_and(|v| v == "1");
    let copied = !skip_clipboard
        && platform::clipboard_write(&ClipboardPayload::Files(vec![path.clone()])).is_ok();
    let secs = stats.duration_ms as f64 / 1000.0;
    let size = if size >= 1 << 20 {
        format!("{:.1} MB", size as f64 / f64::from(1 << 20))
    } else {
        format!("{} KB", (size >> 10).max(1))
    };
    let what = if copied {
        "已保存并复制"
    } else {
        "已保存"
    };
    // 设置里关了"导出完成后提示"就不提示
    if state(app).settings.read().record.notify_done {
        wm::toast(
            app,
            "success",
            format!("{}{what}（{secs:.1} 秒，{size}）", format.label()),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba(c))
    }

    #[test]
    fn identical_frames_produce_no_delta() {
        let mut a = solid(40, 30, [10, 20, 30, 255]);
        let b = a.clone();
        assert!(delta_rect(&mut a, &b).is_none());
    }

    #[test]
    fn capture_noise_is_not_a_change() {
        // 抓屏换色彩空间带的 ±1 抖动散在各处：不算变，不出帧
        let mut a = solid(40, 30, [10, 20, 30, 255]);
        let mut b = a.clone();
        for (x, y) in [(0, 0), (39, 29), (20, 3), (7, 25)] {
            b.put_pixel(x, y, image::Rgba([11, 19, 31, 255]));
        }
        assert!(delta_rect(&mut a, &b).is_none());
        // 一点点攒起来的变化够大了照样更新
        let c = solid(40, 30, [18, 20, 30, 255]);
        let f = delta_rect(&mut a, &c).unwrap();
        assert_eq!((f.width, f.height), (40, 30));
        assert_eq!(a.get_pixel(0, 0).0, [18, 20, 30, 255]);
    }

    #[test]
    fn delta_covers_only_the_changed_box() {
        let mut a = solid(40, 30, [10, 20, 30, 255]);
        let mut b = a.clone();
        b.put_pixel(5, 7, image::Rgba([200, 0, 0, 255]));
        b.put_pixel(12, 9, image::Rgba([0, 200, 0, 255]));
        let f = delta_rect(&mut a, &b).unwrap();
        assert_eq!((f.x, f.y, f.width, f.height), (5, 7, 8, 3));
        // 块里没变的像素是透明的
        assert!(f.rgba.chunks_exact(4).any(|p| p[3] == 0));
    }

    #[test]
    fn encodes_a_playable_gif() {
        let dir = std::env::temp_dir().join(format!("dato-gif-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.gif");
        let (tx, rx) = sync_channel(8);
        let p = path.clone();
        let h = std::thread::spawn(move || encode(rx, &p, (40, 30), RecordFormat::Gif, true));
        let a = solid(40, 30, [10, 20, 30, 0]);
        let mut b = a.clone();
        b.put_pixel(3, 3, image::Rgba([255, 255, 255, 0]));
        tx.send(Msg::Frame(a.clone(), 0)).unwrap();
        tx.send(Msg::Frame(a, 100)).unwrap(); // 一样的帧，不出新帧
        tx.send(Msg::Frame(b, 200)).unwrap();
        tx.send(Msg::Frame(RgbaImage::new(0, 0), 500)).unwrap();
        tx.send(Msg::End).unwrap();
        let stats = h.join().unwrap().unwrap();
        assert_eq!(stats.frames, 2);
        // 读回来：两帧，第一帧持续 200ms，第二帧 300ms
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let mut dec = opts.read_info(File::open(&path).unwrap()).unwrap();
        let mut delays = Vec::new();
        while let Some(f) = dec.read_next_frame().unwrap() {
            delays.push(f.delay);
        }
        assert_eq!(delays, vec![20, 30]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 录一段 GIF：每 100ms 一帧，第 i 帧左上角那个像素的红色是 i
    fn sample_gif(path: &Path, frames: u8) {
        let (tx, rx) = sync_channel(64);
        let p = path.to_path_buf();
        let h = std::thread::spawn(move || encode(rx, &p, (20, 10), RecordFormat::Gif, true));
        for i in 0..frames {
            let mut img = solid(20, 10, [0, 0, 0, 255]);
            img.put_pixel(0, 0, image::Rgba([i * 10, 0, 0, 255]));
            tx.send(Msg::Frame(img, u64::from(i) * 100)).unwrap();
        }
        tx.send(Msg::Frame(RgbaImage::new(0, 0), u64::from(frames) * 100))
            .unwrap();
        tx.send(Msg::End).unwrap();
        h.join().unwrap().unwrap();
    }

    fn delays_and_reds(path: &Path) -> Vec<(u16, u8)> {
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let mut dec = opts.read_info(File::open(path).unwrap()).unwrap();
        let mut out = Vec::new();
        let mut red = 0;
        while let Some(f) = dec.read_next_frame().unwrap() {
            // 左上角像素在这一块里、不透明时才是新值
            if f.left == 0 && f.top == 0 && f.buffer[3] != 0 {
                red = f.buffer[0];
            }
            out.push((f.delay, red));
        }
        out
    }

    #[test]
    fn edit_gif_trims_and_speeds_up() {
        let dir = std::env::temp_dir().join(format!("dato-gif-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, dst) = (dir.join("a.gif"), dir.join("b.gif"));
        sample_gif(&src, 10);
        // 只要 250ms..650ms（第 2 帧后半到第 6 帧前半），2 倍速
        let stats = rework_animation(
            &src,
            &dst,
            RecordFormat::Gif,
            platform::VideoEdit {
                start_ms: 250,
                end_ms: 650,
                speed: 2.0,
            },
            1000,
            true,
            None,
        )
        .unwrap();
        assert_eq!(stats.duration_ms, 200);
        let got = delays_and_reds(&dst);
        // 第 2 帧剩 50ms → 25ms，第 3~5 帧各 100ms → 50ms；最后一帧编码器至少给 100ms（循环播放时别一闪而过）
        assert_eq!(
            got.iter().map(|g| g.1).collect::<Vec<_>>(),
            vec![20, 30, 40, 50, 60]
        );
        assert_eq!(
            got.iter().map(|g| g.0).collect::<Vec<_>>(),
            vec![3, 5, 5, 5, 10]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apng_records_and_gets_a_progress_bar() {
        let dir = std::env::temp_dir().join(format!("dato-apng-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, dst) = (dir.join("a.png"), dir.join("b.png"));
        let (tx, rx) = sync_channel(8);
        let p = src.clone();
        let h = std::thread::spawn(move || encode(rx, &p, (40, 30), RecordFormat::Apng, true));
        tx.send(Msg::Frame(solid(40, 30, [255, 255, 255, 255]), 0))
            .unwrap();
        tx.send(Msg::Frame(solid(40, 30, [250, 250, 250, 255]), 500))
            .unwrap();
        tx.send(Msg::Frame(RgbaImage::new(0, 0), 1000)).unwrap();
        tx.send(Msg::End).unwrap();
        let stats = h.join().unwrap().unwrap();
        assert_eq!((stats.frames, stats.duration_ms), (2, 1000));
        // 加进度条：一秒拆成十来帧，最后一帧的条几乎走满
        let decor = record_fx::Decor {
            kind: record_fx::DecorKind::Bar,
            color: [255, 0, 0, 255],
        };
        let out = rework_animation(
            &src,
            &dst,
            RecordFormat::Apng,
            platform::VideoEdit {
                start_ms: 0,
                end_ms: 1000,
                speed: 1.0,
            },
            1000,
            true,
            Some(decor),
        )
        .unwrap();
        assert!(out.frames >= 8);
        let mut last = None;
        crate::apng::read_frames(&dst, |img, _| {
            last = Some(img.clone());
            true
        })
        .unwrap();
        let last = last.unwrap();
        assert_eq!(last.get_pixel(30, 29).0, [255, 0, 0, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_scale_fits_the_resolution_box() {
        // 2560×1440 的区域：1080p → 1920×1080，720p → 1280×720，原始不缩
        assert!((output_scale(RecordFormat::Mp4, "1080p", 2560, 1440) - 0.75).abs() < 1e-9);
        assert!((output_scale(RecordFormat::Mp4, "720p", 2560, 1440) - 0.5).abs() < 1e-9);
        assert_eq!(output_scale(RecordFormat::Mp4, "original", 2560, 1440), 1.0);
        // 竖着的区域装进 1080×1920
        assert!((output_scale(RecordFormat::Mp4, "1080p", 1440, 2560) - 0.75).abs() < 1e-9);
        // 比上限小的不放大
        assert_eq!(output_scale(RecordFormat::Mp4, "720p", 800, 600), 1.0);
        assert!((output_scale(RecordFormat::Gif, "original", 3200, 1000) - 0.5).abs() < 1e-9);
        // GIF 默认 720p：retina 上 1600×1000 的选区缩到 1152×720；480p 装进 854×480
        assert!((output_scale(RecordFormat::Gif, "720p", 1600, 1000) - 0.72).abs() < 1e-9);
        assert!((output_scale(RecordFormat::Gif, "480p", 1600, 800) - 854.0 / 1600.0).abs() < 1e-9);
    }

    #[test]
    fn dropping_the_sender_without_end_cancels() {
        let path = std::env::temp_dir().join(format!("dato-gif-cancel-{}.gif", std::process::id()));
        let (tx, rx) = sync_channel(8);
        tx.send(Msg::Frame(solid(8, 8, [1, 2, 3, 255]), 0)).unwrap();
        drop(tx);
        assert!(encode(rx, &path, (8, 8), RecordFormat::Gif, true).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
