//! GIF 录制：截图框好区域后点工具条上的"录制 GIF"。
//!
//! 和长截图一样：遮罩变成鼠标穿透、底图隐藏，露出真实桌面，用户在选区里正常操作；
//! 选区外面一圈红框和一条控制栏（计时、完成、取消）。控制栏要能点：录制线程每一拍顺便
//! 看鼠标在不在它上面，在就临时关掉遮罩的穿透。不装全局键盘钩子 —— 录的就是用户的操作，
//! 回车、Esc 都得照常给目标程序。再按一次截图热键也能结束（前端收到 capture-hotkey 转发）。
//!
//! 两个线程：
//! - 录制：xcap 的 WGC 连续录屏，按帧率取最新画面，裁出选区、画上鼠标指针。画面和指针都
//!   没变就不出帧（编码那边自动把上一帧拉长）
//! - 编码：边录边写进临时 GIF 文件。只编码和上一帧相比变了的那一块，块里没变的像素设成
//!   透明（上一帧的内容留着），文件小、量化也快。编码跟不上时录制那边丢帧，不会越积越多
//!
//! 录完存到截图保存目录，并把文件放进剪贴板（能直接粘贴到聊天软件里）。

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
use crate::{events, imaging, wm};

/// 最长录多久，到点自动结束
pub const MAX_SECONDS: u64 = 60;
/// GIF 的最长边（物理像素）。选区更大就等比缩小，GIF 太大哪儿都发不出去
const MAX_SIDE: u32 = 1600;
/// 编码队列长度。编码跟不上时多出来的帧直接丢掉
const QUEUE: usize = 6;

const RUNNING: u8 = 0;
const FINISH: u8 = 1;
const CANCEL: u8 = 2;

struct Active {
    monitor: MonitorInfo,
    /// 控制栏等可点击区域（屏幕物理坐标）
    regions: Vec<PhysicalRect>,
    interactive: bool,
    stop: Arc<AtomicU8>,
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
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    elapsed_ms: u64,
    max_ms: u64,
    frames: u32,
}

enum Msg {
    /// 一帧画面和它的时间（从开始录起的毫秒数）
    Frame(RgbaImage, u64),
    /// 录完了，把最后一帧写完（结束时间在这之前用一个 0×0 的空帧带过来）
    End,
}

struct Stats {
    frames: u32,
    duration_ms: u64,
    width: u32,
    height: u32,
}

pub fn start(
    app: &AppHandle,
    session: Arc<Session>,
    monitor: MonitorInfo,
    global: PhysicalRect,
) -> AppResult<()> {
    if is_active() {
        return Err(AppError::msg("已经在录制 GIF 了"));
    }
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
    let stop = Arc::new(AtomicU8::new(RUNNING));
    *ACTIVE.lock() = Some(Active {
        monitor: monitor.clone(),
        regions: Vec::new(),
        interactive: false,
        stop: stop.clone(),
    });
    tracing::info!(
        session = session.id,
        ?local,
        fps = settings.gif_fps,
        "开始录制 GIF"
    );

    let ui_app = app.clone();
    let event = GifStateEvent {
        active: true,
        monitor_id: monitor.id.0,
        rect: local,
    };
    let (cx, cy) = (
        global.x + global.width as i32 / 2,
        global.y + global.height as i32 / 2,
    );
    let fallback = session.previous;
    app.run_on_main_thread(move || {
        // 底图藏起来露出真实桌面，遮罩全部鼠标穿透；焦点交给选区下面那个窗口，用户直接接着操作
        platform::backdrop::hide_all();
        overlay::for_each(&ui_app, |w| {
            let _ = w.set_ignore_cursor_events(true);
        });
        let _ = ui_app.emit(events::GIF_STATE, event);
        if let Some(target) = platform::window_at(cx, cy).or(fallback) {
            let _ = platform::focus_window(target);
        }
    })?;

    // 输出尺寸：太大就等比缩小
    let k = (f64::from(MAX_SIDE) / f64::from(local.width.max(local.height))).min(1.0);
    let out = (
        ((f64::from(local.width) * k).round() as u32).max(1),
        ((f64::from(local.height) * k).round() as u32).max(1),
    );
    let tmp = st.paths.temp().join(format!("gif-{}.gif", session.id));
    let (tx, rx) = sync_channel::<Msg>(QUEUE);
    let enc_path = tmp.clone();
    let encoder = std::thread::Builder::new()
        .name("gif-encode".into())
        .spawn(move || encode(rx, &enc_path, out))?;

    let ctx = RecordCtx {
        app: app.clone(),
        monitor,
        local,
        global,
        fps: settings.gif_fps.clamp(5, 30),
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

pub fn set_regions(regions: Vec<PhysicalRect>) {
    if let Some(a) = ACTIVE.lock().as_mut() {
        let origin = a.monitor.bounds;
        a.regions = regions
            .into_iter()
            .map(|r| PhysicalRect::new(r.x + origin.x, r.y + origin.y, r.width, r.height))
            .collect();
    }
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
}

fn record(ctx: RecordCtx, encoder: std::thread::JoinHandle<AppResult<Stats>>) {
    let app = ctx.app.clone();
    // 等底图隐藏、遮罩换好界面
    std::thread::sleep(Duration::from_millis(150));
    let outcome = match ScreenRecorder::start(ctx.monitor.id) {
        Ok(recorder) => {
            run(&ctx, &recorder);
            drop(recorder);
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
    let result = encoder.join();

    *ACTIVE.lock() = None;
    let _ = app.emit(
        events::GIF_STATE,
        GifStateEvent {
            active: false,
            monitor_id: 0,
            rect: PhysicalRect::default(),
        },
    );
    capture::end_session(&app, ctx.session_id, false);

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
            if let Err(err) = save(&app, &ctx.tmp, &stats) {
                tracing::error!("保存 GIF 失败：{err}");
                wm::toast(&app, "error", format!("保存 GIF 失败：{err}"));
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
fn run(ctx: &RecordCtx, recorder: &ScreenRecorder) {
    let interval = Duration::from_millis(u64::from(1000 / ctx.fps));
    let max = Duration::from_secs(MAX_SECONDS);
    let started = Instant::now();
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
        // 等到下一拍，期间来的新画面只留最新的
        let wait = next_tick.saturating_duration_since(Instant::now());
        if let Some(full) = recorder.next(wait.max(Duration::from_millis(1))) {
            match imaging::crop_opaque(&full, ctx.local) {
                Ok(img) => {
                    clean = Some(img);
                    changed = true;
                }
                Err(err) => tracing::debug!("GIF 裁帧失败：{err}"),
            }
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
        let Some(base) = clean.as_ref() else { continue };
        if changed || (ctx.cursor && cursor != last_cursor) {
            let mut frame = base.clone();
            if ctx.cursor {
                platform::draw_cursor(&mut frame, (ctx.global.x, ctx.global.y));
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
    tracing::info!(frames, dropped, fps = ctx.fps, "GIF 录制结束");
    // 最后一帧持续到按下"完成"的那一刻
    let _ = ctx.tx.send(Msg::Frame(
        RgbaImage::new(0, 0),
        started.elapsed().as_millis() as u64,
    ));
}

/// 鼠标在控制栏上时关掉遮罩的穿透，按钮才点得到；移开再打开。
fn update_interactive(app: &AppHandle, cursor: Option<(i32, i32)>) {
    let mut guard = ACTIVE.lock();
    let Some(a) = guard.as_mut() else { return };
    let inside = cursor.is_some_and(|(x, y)| a.regions.iter().any(|r| r.contains_point(x, y)));
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

/// 边录边编码。收到空帧（0×0）表示录制结束、给出结束时间；随后的 End 才算真的写完。
/// 发送端断开却没收到 End = 取消。
fn encode(rx: Receiver<Msg>, path: &Path, (w, h): (u32, u32)) -> AppResult<Stats> {
    let file = BufWriter::new(File::create(path)?);
    let mut enc = gif::Encoder::new(file, w as u16, h as u16, &[]).map_err(gif_err)?;
    enc.set_repeat(gif::Repeat::Infinite).map_err(gif_err)?;
    let mut prev: Option<RgbaImage> = None;
    // 已经编好、还不知道该持续多久的那一帧，和它的开始时间
    let mut pending: Option<(gif::Frame<'static>, u64)> = None;
    let mut written = 0u32;
    let mut end_ms = 0u64;
    let mut done = false;
    for msg in rx {
        match msg {
            Msg::Frame(img, t) if img.width() == 0 => end_ms = t,
            Msg::Frame(img, t) => {
                let img = if img.dimensions() == (w, h) {
                    img
                } else {
                    image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle)
                };
                let frame = match prev.as_ref() {
                    None => Some(full_frame(&img)),
                    Some(p) => delta_frame(p, &img),
                };
                // 和上一帧一模一样：不出新帧，上一帧自然拉长
                let Some(frame) = frame else { continue };
                if let Some((f, t0)) = pending.take() {
                    write_frame(&mut enc, f, t.saturating_sub(t0))?;
                    written += 1;
                }
                pending = Some((frame, t));
                prev = Some(img);
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
    write_frame(&mut enc, f, end_ms.saturating_sub(t0).max(100))?;
    written += 1;
    let mut file = enc.into_inner().map_err(gif_err)?;
    file.flush()?;
    Ok(Stats {
        frames: written,
        duration_ms: end_ms,
        width: w,
        height: h,
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

fn full_frame(img: &RgbaImage) -> gif::Frame<'static> {
    let (w, h) = img.dimensions();
    let mut buf = img.as_raw().clone();
    for px in buf.chunks_exact_mut(4) {
        px[3] = 255;
    }
    let mut f =
        gif::Frame::from_rgba_speed(w as u16, h as u16, &mut buf, speed_for((w * h) as usize));
    f.dispose = gif::DisposalMethod::Keep;
    f
}

/// 和上一帧比：只取变了的那块矩形，块里没变的像素设成透明（显示的还是上一帧的）。
/// 完全没变返回 None。
fn delta_frame(prev: &RgbaImage, cur: &RgbaImage) -> Option<gif::Frame<'static>> {
    let (w, h) = cur.dimensions();
    let (pw, cw) = (prev.as_raw(), cur.as_raw());
    let row = (w * 4) as usize;
    let same = |i: usize| pw[i] == cw[i] && pw[i + 1] == cw[i + 1] && pw[i + 2] == cw[i + 2];
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for y in 0..h {
        let start = y as usize * row;
        if pw[start..start + row] == cw[start..start + row] {
            continue;
        }
        let first = (0..w).find(|&x| !same(start + x as usize * 4))?;
        let last = (0..w)
            .rev()
            .find(|&x| !same(start + x as usize * 4))
            .unwrap_or(first);
        x0 = x0.min(first);
        x1 = x1.max(last);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    if x1 < x0 || y1 < y0 {
        return None;
    }
    let (sw, sh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut buf = Vec::with_capacity((sw * sh * 4) as usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let i = (y * w + x) as usize * 4;
            if same(i) {
                buf.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                buf.extend_from_slice(&[cw[i], cw[i + 1], cw[i + 2], 255]);
            }
        }
    }
    let mut f = gif::Frame::from_rgba_speed(
        sw as u16,
        sh as u16,
        &mut buf,
        speed_for((sw * sh) as usize),
    );
    f.left = x0 as u16;
    f.top = y0 as u16;
    f.dispose = gif::DisposalMethod::Keep;
    Some(f)
}

// ───────────────────────── 保存 ─────────────────────────

fn save(app: &AppHandle, tmp: &Path, stats: &Stats) -> AppResult<()> {
    let settings = state(app).settings.read().capture.clone();
    let dir = capture::save_directory(&settings);
    std::fs::create_dir_all(&dir)?;
    let stem = Path::new(&capture::file_name(&settings))
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "DATO COR".into());
    let mut path = dir.join(format!("{stem}.gif"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem}_{n}.gif"));
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
        "GIF 已保存"
    );
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
    wm::toast(
        app,
        "success",
        format!("GIF {what}（{secs:.1} 秒，{size}）"),
    );
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
        let a = solid(40, 30, [10, 20, 30, 255]);
        assert!(delta_frame(&a, &a.clone()).is_none());
    }

    #[test]
    fn delta_covers_only_the_changed_box() {
        let a = solid(40, 30, [10, 20, 30, 255]);
        let mut b = a.clone();
        b.put_pixel(5, 7, image::Rgba([200, 0, 0, 255]));
        b.put_pixel(12, 9, image::Rgba([0, 200, 0, 255]));
        let f = delta_frame(&a, &b).unwrap();
        assert_eq!((f.left, f.top, f.width, f.height), (5, 7, 8, 3));
        // 块里没变的像素是透明的
        assert!(f.transparent.is_some());
    }

    #[test]
    fn encodes_a_playable_gif() {
        let dir = std::env::temp_dir().join(format!("dato-gif-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.gif");
        let (tx, rx) = sync_channel(8);
        let p = path.clone();
        let h = std::thread::spawn(move || encode(rx, &p, (40, 30)));
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

    #[test]
    fn dropping_the_sender_without_end_cancels() {
        let path = std::env::temp_dir().join(format!("dato-gif-cancel-{}.gif", std::process::id()));
        let (tx, rx) = sync_channel(8);
        tx.send(Msg::Frame(solid(8, 8, [1, 2, 3, 255]), 0)).unwrap();
        drop(tx);
        assert!(encode(rx, &path, (8, 8)).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
