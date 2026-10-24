//! 录视频（MP4）：Media Foundation 的 Sink Writer，H.264 / H.265 交给系统的编码器（有显卡编码就用显卡）。
//!
//! 帧只在画面变了的时候送进来（可变帧率），每帧带自己的时间（从开始录起的毫秒数）。手里压一帧：
//! 下一帧来了才知道这一帧持续多久，再写进去；最后一帧一直持续到 `finish` 给的结束时间。
//!
//! 声音：系统声音、麦克风各是一条 AAC 音轨，在各自的采集线程上写（写入器加锁）。没声音的时候补静音，
//! 音轨不断档（断档的话写入器会等这条轨，画面也跟着卡）。两条音轨大多数播放器只放第一条，所以录完
//! 要是有两条（又没开"独立音轨"），`export` 再导出一遍混成一条（剪辑时顺带完成）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use image::RgbaImage;
use parking_lot::Mutex;
use windows::core::{Interface, GUID, HSTRING};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

use super::audio::{self, AudioSink, Capture, Resampler, Source, HNS, RATE};
use super::stretch::Stretcher;
use super::{init, mf_err, pack, unpack, Mt};
use crate::error::{AppError, AppResult};
use crate::platform::types::{VideoEdit, VideoEncoding, VideoOptions};

/// AAC 的码率（字节每秒）：20000 = 160kbps，编码器只认 12000 / 16000 / 20000 / 24000
const AAC_BYTES: u32 = 20_000;
/// 系统声音没来的时候，补静音补到"现在"之前多久（给晚到的包留余地）
const AUDIO_LAG: i64 = 1_500_000;
/// 两个包之间空出多少才算断了、要补静音（小于这个当作时钟抖动，直接接上）
const AUDIO_GAP: i64 = (RATE / 50) as i64;
/// 一次写进去的静音最长多少帧
const SILENCE_CHUNK: i64 = RATE as i64 / 2;

/// 画面送进编码器的格式：BGRA（编码器自己转），不行就自己转成 NV12
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    Rgb32,
    Nv12,
}

fn attrs(n: u32) -> windows::core::Result<IMFAttributes> {
    let mut a = None;
    // SAFETY: 输出参数是本地变量
    unsafe { MFCreateAttributes(&mut a, n)? };
    a.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))
}

fn video_out_type(
    w: u32,
    h: u32,
    enc: VideoEncoding,
    hevc: bool,
) -> windows::core::Result<IMFMediaType> {
    // SAFETY: 新建的媒体类型，只设属性
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(
            &MF_MT_SUBTYPE,
            if hevc {
                &MFVideoFormat_HEVC
            } else {
                &MFVideoFormat_H264
            },
        )?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, enc.bitrate(w, h))?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, pack(enc.fps.max(1), 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        if !hevc {
            t.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
        }
        Ok(t)
    }
}

fn video_in_type(w: u32, h: u32, fps: u32, input: Input) -> windows::core::Result<IMFMediaType> {
    // SAFETY: 同上
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        match input {
            Input::Rgb32 => {
                t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
                // 正的行宽 = 从上往下存（RGB 默认是从下往上）
                t.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4)?;
            }
            Input::Nv12 => {
                t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
                t.SetUINT32(&MF_MT_DEFAULT_STRIDE, w)?;
            }
        }
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps.max(1), 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        Ok(t)
    }
}

fn audio_type(aac: bool) -> windows::core::Result<IMFMediaType> {
    // SAFETY: 同上
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
        t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        if aac {
            t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
            t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES)?;
            t.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        } else {
            t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
            t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * 4)?;
            t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        }
        Ok(t)
    }
}

/// 一个样本：缓冲区由 `fill` 填（给它整块可写的内存），带时间和时长（100 纳秒）
fn make_sample(
    len: usize,
    time: i64,
    duration: i64,
    fill: impl FnOnce(&mut [u8]),
) -> windows::core::Result<IMFSample> {
    // SAFETY: 锁住缓冲区写满再解锁；长度是自己申请的
    unsafe {
        let buf = MFCreateMemoryBuffer(len as u32)?;
        let mut ptr = std::ptr::null_mut();
        buf.Lock(&mut ptr, None, None)?;
        fill(std::slice::from_raw_parts_mut(ptr, len));
        buf.Unlock()?;
        buf.SetCurrentLength(len as u32)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(time)?;
        s.SetSampleDuration(duration.max(1))?;
        Ok(s)
    }
}

/// 打开的写入器：画面一路（`video`），声音若干路
struct Opened {
    writer: IMFSinkWriter,
    video: u32,
    input: Input,
    audio: Vec<u32>,
    hevc: bool,
}

/// 要开一个什么样的写入器
#[derive(Clone, Copy)]
struct Spec {
    w: u32,
    h: u32,
    enc: VideoEncoding,
    audio_tracks: usize,
    /// 录制（边录边写）：写入器不许堵住写的线程
    realtime: bool,
}

fn try_open(path: &Path, spec: Spec, hevc: bool, input: Input) -> windows::core::Result<Opened> {
    let Spec {
        w,
        h,
        enc,
        audio_tracks,
        realtime,
    } = spec;
    let _ = std::fs::remove_file(path);
    // SAFETY: 属性、媒体类型都是新建的；写入器在本函数里配好才交出去
    unsafe {
        let a = attrs(2)?;
        a.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        if realtime {
            // 录制时写入器不许因为编码跟不上就把写帧的线程堵住（声音线程也在写）
            a.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        }
        let writer = MFCreateSinkWriterFromURL(
            &HSTRING::from(path.as_os_str()),
            None::<&IMFByteStream>,
            &a,
        )?;
        let video = writer.AddStream(&video_out_type(w, h, enc, hevc)?)?;
        // 关键帧最多隔 5 秒（拖进度条时找得快）
        let params = attrs(1)?;
        params.SetUINT32(&CODECAPI_AVEncMPVGOPSize, enc.fps.max(1) * 5)?;
        writer.SetInputMediaType(video, &video_in_type(w, h, enc.fps, input)?, &params)?;
        let mut audio = Vec::new();
        for _ in 0..audio_tracks {
            let s = writer.AddStream(&audio_type(true)?)?;
            writer.SetInputMediaType(s, &audio_type(false)?, None::<&IMFAttributes>)?;
            audio.push(s);
        }
        writer.BeginWriting()?;
        Ok(Opened {
            writer,
            video,
            input,
            audio,
            hevc,
        })
    }
}

/// 按设置的编码开；H.265 这台电脑编不了就换 H.264，BGRA 送不进去就自己转 NV12。返回要提示用户的话
fn open(path: &Path, spec: Spec, inputs: &[Input]) -> AppResult<(Opened, Option<String>)> {
    let mut last = None;
    let codecs: &[bool] = if spec.enc.hevc {
        &[true, false]
    } else {
        &[false]
    };
    for &hevc in codecs {
        for &input in inputs {
            match try_open(path, spec, hevc, input) {
                Ok(o) => {
                    let note = (spec.enc.hevc && !o.hevc)
                        .then(|| "这台电脑编不了 H.265，已改用 H.264".to_string());
                    tracing::info!(hevc = o.hevc, input = ?o.input, audio_tracks = spec.audio_tracks, "视频写入器已打开");
                    return Ok((o, note));
                }
                Err(e) => {
                    tracing::debug!(hevc, ?input, "打开视频写入器失败：{}", e.message());
                    last = Some(e);
                }
            }
        }
    }
    let _ = std::fs::remove_file(path);
    Err(last.map_or_else(
        || AppError::msg("创建视频文件失败"),
        |e| mf_err("创建视频文件失败", e),
    ))
}

/// 写入器加一把锁：画面线程、几个声音线程都往里写
struct Sink {
    writer: Mutex<Mt<IMFSinkWriter>>,
}

impl Sink {
    fn write(&self, stream: u32, sample: &IMFSample) -> windows::core::Result<()> {
        let w = self.writer.lock();
        // SAFETY: 锁里写
        unsafe { w.0.WriteSample(stream, sample) }
    }
}

// ───────────────────────── 音轨 ─────────────────────────

struct TrackState {
    /// 已经写了多少帧（48k）
    pos: i64,
    open: bool,
}

/// 一条音轨：采集线程把声音交给它，它对好时间写进写入器
struct Track {
    out: OnceLock<(Arc<Sink>, u32)>,
    state: Mutex<TrackState>,
    name: &'static str,
}

impl Track {
    fn new(name: &'static str) -> Self {
        Self {
            out: OnceLock::new(),
            state: Mutex::new(TrackState { pos: 0, open: true }),
            name,
        }
    }

    fn write(&self, st: &mut TrackState, data: &[i16]) {
        let Some((sink, stream)) = self.out.get() else {
            return;
        };
        let frames = (data.len() / 2) as i64;
        if frames == 0 {
            return;
        }
        let time = st.pos * HNS / i64::from(RATE);
        let duration = (st.pos + frames) * HNS / i64::from(RATE) - time;
        let sample = make_sample(data.len() * 2, time, duration, |buf| {
            for (d, s) in buf.chunks_exact_mut(2).zip(data) {
                d.copy_from_slice(&s.to_le_bytes());
            }
        });
        match sample.and_then(|s| sink.write(*stream, &s)) {
            Ok(()) => st.pos += frames,
            Err(e) => tracing::debug!(track = self.name, "写声音失败：{}", e.message()),
        }
    }

    fn silence_to(&self, st: &mut TrackState, frame: i64) {
        while st.pos < frame {
            let n = (frame - st.pos).min(SILENCE_CHUNK) as usize;
            let before = st.pos;
            self.write(st, &vec![0i16; n * 2]);
            if st.pos == before {
                break;
            }
        }
    }

    /// 收尾：静音补到 `end`（帧），之后再来的丢掉
    fn close(&self, end: i64) {
        let mut st = self.state.lock();
        if st.open {
            self.silence_to(&mut st, end);
            st.open = false;
        }
    }
}

impl AudioSink for Track {
    fn packet(&self, at: i64, samples: &[i16]) {
        let mut st = self.state.lock();
        if !st.open || self.out.get().is_none() {
            return;
        }
        let mut start = at * i64::from(RATE) / HNS;
        let mut data = samples;
        if start < st.pos {
            // 和写过的重叠（时钟抖动、补静音补过头了）：重叠的部分丢掉
            let skip = ((st.pos - start) * 2) as usize;
            if skip >= data.len() {
                return;
            }
            data = &data[skip..];
            start = st.pos;
        }
        if start - st.pos > AUDIO_GAP {
            self.silence_to(&mut st, start);
        }
        self.write(&mut st, data);
    }

    fn tick(&self, now: i64) {
        let mut st = self.state.lock();
        if !st.open || self.out.get().is_none() {
            return;
        }
        let target = (now - AUDIO_LAG) * i64::from(RATE) / HNS;
        if target - st.pos > AUDIO_GAP * 5 {
            self.silence_to(&mut st, target);
        }
    }
}

// ───────────────────────── 录制 ─────────────────────────

pub struct VideoWriter {
    sink: Arc<Sink>,
    video: u32,
    input: Input,
    width: u32,
    height: u32,
    path: PathBuf,
    /// 压在手里的上一帧（下一帧来了才知道它持续多久）
    held: Option<(IMFSample, u64)>,
    last_ms: Option<u64>,
    /// 第 0 毫秒的 QPC 时间（100 纳秒）；`start` 之前是 i64::MAX
    origin: Arc<AtomicI64>,
    tracks: Vec<Arc<Track>>,
    captures: Vec<Capture>,
    warnings: Vec<String>,
}

impl VideoWriter {
    /// `width`、`height` 必须是偶数（H.264 的要求，调用方负责裁）。
    pub fn create(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        options: VideoOptions,
    ) -> AppResult<Self> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(AppError::msg("视频尺寸必须是正的偶数"));
        }
        init()?;
        // 先开声音（开不了的不加音轨，录完提示用户），再开写入器
        let origin = Arc::new(AtomicI64::new(i64::MAX));
        let mut warnings = Vec::new();
        let mut tracks = Vec::new();
        let mut captures = Vec::new();
        for (on, source, name) in [
            (options.system_audio, Source::System, "system"),
            (options.microphone, Source::Microphone, "mic"),
        ] {
            if !on {
                continue;
            }
            let track = Arc::new(Track::new(name));
            match Capture::start(source, track.clone(), origin.clone()) {
                Ok(c) => {
                    captures.push(c);
                    tracks.push(track);
                }
                Err(e) => {
                    tracing::warn!("录视频的声音没开成：{e}");
                    warnings.push(e.to_string());
                }
            }
        }
        let enc = VideoEncoding {
            fps,
            ..options.encoding
        };
        let spec = Spec {
            w: width,
            h: height,
            enc,
            audio_tracks: tracks.len(),
            realtime: true,
        };
        let (opened, note) = open(path, spec, &[Input::Rgb32, Input::Nv12])?;
        warnings.extend(note);
        let sink = Arc::new(Sink {
            writer: Mutex::new(Mt(opened.writer)),
        });
        for (track, stream) in tracks.iter().zip(&opened.audio) {
            let _ = track.out.set((sink.clone(), *stream));
        }
        Ok(Self {
            sink,
            video: opened.video,
            input: opened.input,
            width,
            height,
            path: path.to_path_buf(),
            held: None,
            last_ms: None,
            origin,
            tracks,
            captures,
            warnings,
        })
    }

    /// 开始计时：`origin` 是第 0 毫秒（录制线程开始数时间的那一刻）。之前到的声音丢掉
    pub fn start(&mut self, origin: Instant) {
        if self.origin.load(Ordering::Acquire) != i64::MAX {
            return;
        }
        self.origin.store(audio::qpc_of(origin), Ordering::Release);
    }

    /// 没开成的声音、换了编码（给用户看的原因）
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 写一帧。`frame` 至少 `width × height`（多出来的右边、下边裁掉）；时间不比上一帧晚的丢掉。
    pub fn push(&mut self, frame: &RgbaImage, ms: u64) -> AppResult<()> {
        if self.last_ms.is_some_and(|last| ms <= last) {
            return Ok(());
        }
        if frame.width() < self.width || frame.height() < self.height {
            return Err(AppError::msg("视频帧尺寸不对"));
        }
        if self.origin.load(Ordering::Acquire) == i64::MAX {
            self.start(Instant::now() - Duration::from_millis(ms));
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let time = ms as i64 * 10_000;
        let sample = match self.input {
            Input::Rgb32 => make_sample(w * h * 4, time, 1, |buf| rgba_to_bgra(frame, w, h, buf)),
            Input::Nv12 => {
                make_sample(w * h * 3 / 2, time, 1, |buf| rgba_to_nv12(frame, w, h, buf))
            }
        }
        .map_err(|e| mf_err("写视频帧失败", e))?;
        if let Some((prev, prev_ms)) = self.held.take() {
            self.write_video(&prev, prev_ms, ms)?;
        }
        self.held = Some((sample, ms));
        self.last_ms = Some(ms);
        Ok(())
    }

    fn write_video(&self, sample: &IMFSample, from_ms: u64, to_ms: u64) -> AppResult<()> {
        // SAFETY: 样本是自己建的
        unsafe { sample.SetSampleDuration((to_ms.saturating_sub(from_ms).max(1) as i64) * 10_000) }
            .and_then(|()| self.sink.write(self.video, sample))
            .map_err(|e| mf_err("写视频帧失败", e))
    }

    fn stop_audio(&mut self) {
        for c in self.captures.drain(..) {
            c.stop();
        }
    }

    /// 写完：最后一帧持续到 `end_ms`，声音补齐到同样长，等文件落盘。一帧都没有的话报错（文件删掉）。
    /// 返回文件里有几条音轨。
    pub fn finish(mut self, end_ms: u64) -> AppResult<usize> {
        self.stop_audio();
        let Some((last, last_ms)) = self.held.take() else {
            for t in &self.tracks {
                t.state.lock().open = false;
            }
            let _ = std::fs::remove_file(&self.path);
            return Err(AppError::msg("没有录到画面"));
        };
        let end_ms = end_ms.max(last_ms + 1);
        self.write_video(&last, last_ms, end_ms)?;
        let end_frame = end_ms as i64 * i64::from(RATE) / 1000;
        for t in &self.tracks {
            t.close(end_frame);
        }
        let w = self.sink.writer.lock();
        // SAFETY: 所有流都写完了
        unsafe { w.0.Finalize() }.map_err(|e| mf_err("写视频文件失败", e))?;
        Ok(self.tracks.len())
    }
}

impl Drop for VideoWriter {
    fn drop(&mut self) {
        self.stop_audio();
    }
}

/// RGBA → 从上往下存的 BGRA（编码器的 RGB32）
fn rgba_to_bgra(frame: &RgbaImage, w: usize, h: usize, out: &mut [u8]) {
    let src = frame.as_raw();
    let stride = frame.width() as usize * 4;
    for y in 0..h {
        let from = &src[y * stride..y * stride + w * 4];
        let to = &mut out[y * w * 4..(y + 1) * w * 4];
        for (d, s) in to.chunks_exact_mut(4).zip(from.chunks_exact(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = 255;
        }
    }
}

/// RGBA → NV12（BT.709，有限范围；色度取 2×2 的平均）
fn rgba_to_nv12(frame: &RgbaImage, w: usize, h: usize, out: &mut [u8]) {
    let src = frame.as_raw();
    let stride = frame.width() as usize * 4;
    let px = |x: usize, y: usize| {
        let i = y * stride + x * 4;
        (
            i32::from(src[i]),
            i32::from(src[i + 1]),
            i32::from(src[i + 2]),
        )
    };
    let (luma, chroma) = out.split_at_mut(w * h);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = px(x, y);
            luma[y * w + x] = ((47 * r + 157 * g + 16 * b + 128) / 256 + 16).clamp(16, 235) as u8;
        }
    }
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let (mut r, mut g, mut b) = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = px(x * 2 + dx, y * 2 + dy);
                r += p.0;
                g += p.1;
                b += p.2;
            }
            let (r, g, b) = (r / 4, g / 4, b / 4);
            let u = (-26 * r - 87 * g + 112 * b + 128) / 256 + 128;
            let v = (112 * r - 102 * g - 10 * b + 128) / 256 + 128;
            chroma[y * w + x * 2] = u.clamp(16, 240) as u8;
            chroma[y * w + x * 2 + 1] = v.clamp(16, 240) as u8;
        }
    }
}

// ───────────────────────── 剪辑 / 变速 / 混音 ─────────────────────────

/// 几路声音按时间对齐加起来（只一路就是原样）。位置都是"从剪出来的片段开头起第几帧"（48k）
struct Mixer {
    inputs: Vec<MixInput>,
    pos: i64,
}

struct MixInput {
    start: i64,
    data: Vec<f32>,
    done: bool,
}

impl MixInput {
    fn end(&self) -> i64 {
        self.start + (self.data.len() / 2) as i64
    }
}

impl Mixer {
    fn new(n: usize) -> Self {
        Self {
            inputs: (0..n)
                .map(|_| MixInput {
                    start: 0,
                    data: Vec::new(),
                    done: false,
                })
                .collect(),
            pos: 0,
        }
    }

    fn push(&mut self, i: usize, at: i64, samples: &[f32]) {
        let inp = &mut self.inputs[i];
        let end = inp.end();
        let mut data = samples;
        let mut at = at;
        if at < end {
            let skip = ((end - at) * 2) as usize;
            if skip >= data.len() {
                return;
            }
            data = &data[skip..];
            at = end;
        }
        if at > end {
            inp.data
                .resize(inp.data.len() + ((at - end) * 2) as usize, 0.0);
        }
        inp.data.extend_from_slice(data);
    }

    fn finish(&mut self, i: usize) {
        self.inputs[i].done = true;
    }

    /// 所有路都到了的那部分（没结束的路里最短的）加起来取走；全结束了就取到最长的
    fn drain(&mut self) -> Vec<f32> {
        let open: Vec<i64> = self
            .inputs
            .iter()
            .filter(|i| !i.done)
            .map(MixInput::end)
            .collect();
        let upto = if open.is_empty() {
            self.inputs
                .iter()
                .map(MixInput::end)
                .max()
                .unwrap_or(self.pos)
        } else {
            open.into_iter().min().unwrap_or(self.pos)
        };
        if upto <= self.pos {
            return Vec::new();
        }
        let n = (upto - self.pos) as usize;
        let mut out = vec![0f32; n * 2];
        for inp in &mut self.inputs {
            for f in 0..n {
                let abs = self.pos + f as i64;
                if abs >= inp.start && abs < inp.end() {
                    let k = ((abs - inp.start) * 2) as usize;
                    out[f * 2] += inp.data[k];
                    out[f * 2 + 1] += inp.data[k + 1];
                }
            }
            // 用掉的扔掉
            let used = (upto.min(inp.end()) - inp.start).max(0);
            inp.data.drain(..(used * 2) as usize);
            inp.start += used;
        }
        self.pos = upto;
        if self.inputs.len() > 1 {
            for v in &mut out {
                *v = v.clamp(-1.0, 1.0);
            }
        }
        out
    }
}

/// 输出的一条音轨：混好的声音 → （变速）→ AAC
struct OutTrack {
    stream: u32,
    stretch: Option<Stretcher>,
    written: i64,
}

impl OutTrack {
    fn write(&mut self, writer: &IMFSinkWriter, samples: &[f32]) -> AppResult<()> {
        let frames = (samples.len() / 2) as i64;
        if frames == 0 {
            return Ok(());
        }
        let time = self.written * HNS / i64::from(RATE);
        let duration = (self.written + frames) * HNS / i64::from(RATE) - time;
        let sample = make_sample(samples.len() * 2, time, duration, |buf| {
            for (d, s) in buf.chunks_exact_mut(2).zip(samples) {
                d.copy_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
            }
        })
        .map_err(|e| mf_err("导出视频失败", e))?;
        // SAFETY: 本线程独用的写入器
        unsafe { writer.WriteSample(self.stream, &sample) }
            .map_err(|e| mf_err("导出视频失败", e))?;
        self.written += frames;
        Ok(())
    }

    fn feed(&mut self, writer: &IMFSinkWriter, samples: Vec<f32>) -> AppResult<()> {
        let out = match self.stretch.as_mut() {
            Some(s) => s.push(&samples),
            None => samples,
        };
        self.write(writer, &out)
    }

    fn flush(&mut self, writer: &IMFSinkWriter) -> AppResult<()> {
        if let Some(s) = self.stretch.take() {
            let rest = s.finish();
            self.write(writer, &rest)?;
        }
        Ok(())
    }
}

/// 解码出来的一帧 NV12（可能带对齐用的空白行列）→ 紧凑的 w × h NV12
fn copy_nv12(
    sample: &IMFSample,
    w: u32,
    h: u32,
    padded_h: u32,
    default_stride: u32,
    out: &mut [u8],
) -> windows::core::Result<()> {
    let (w, h) = (w as usize, h as usize);
    // SAFETY: 锁住解码器的缓冲区读，读完解锁；行宽按缓冲区自己报的走
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let (base, pitch, locked2d) = if let Ok(b2) = buffer.cast::<IMF2DBuffer>() {
            let (mut p, mut pitch) = (std::ptr::null_mut(), 0i32);
            b2.Lock2D(&mut p, &mut pitch)?;
            (p, pitch as isize, Some(b2))
        } else {
            let mut p = std::ptr::null_mut();
            buffer.Lock(&mut p, None, None)?;
            (p, default_stride.max(w as u32) as isize, None)
        };
        for y in 0..h {
            let row = base.offset(y as isize * pitch);
            std::ptr::copy_nonoverlapping(row, out.as_mut_ptr().add(y * w), w);
        }
        let uv = base.offset(pitch * padded_h as isize);
        for y in 0..h / 2 {
            let row = uv.offset(y as isize * pitch);
            std::ptr::copy_nonoverlapping(row, out.as_mut_ptr().add(w * h + y * w), w);
        }
        match locked2d {
            Some(b2) => b2.Unlock2D()?,
            None => buffer.Unlock()?,
        }
    }
    Ok(())
}

/// 剪辑 / 混音：从 `src` 取 `[start, end)` 一段、按倍速，重新编码到 `dst`。
///
/// 画面解码成 NV12、按 `encoding` 重新编码（时间除以倍速）；声音解码成 48k 立体声，`mix` 时几条混成一条，
/// 否则每条各自保留；变速时声音保持原来的音调（WSOLA）。
pub fn export(
    src: &Path,
    dst: &Path,
    edit: VideoEdit,
    encoding: VideoEncoding,
    mix: bool,
) -> AppResult<()> {
    init()?;
    let start = edit.start_ms as i64 * 10_000;
    let end = edit.end_ms as i64 * 10_000;
    if end - start < 1_000_000 {
        return Err(AppError::msg("剪出来的片段太短了"));
    }
    let speed = edit.speed.clamp(0.25, 4.0);
    let stretch = (speed - 1.0).abs() > 0.001;
    let rescale = |t: i64| ((t - start).max(0) as f64 / speed) as i64;
    let read_err = |e: windows::core::Error| mf_err("读取视频失败", e);

    // SAFETY: 读写器都在这个线程上建和用；样本只在这一轮里用
    unsafe {
        let a = attrs(1).map_err(read_err)?;
        a.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
            .map_err(read_err)?;
        let reader =
            MFCreateSourceReaderFromURL(&HSTRING::from(src.as_os_str()), &a).map_err(read_err)?;
        // 找出画面和声音是第几路
        let mut video_idx = None;
        let mut audio_idx = Vec::new();
        for i in 0..32u32 {
            let Ok(t) = reader.GetNativeMediaType(i, 0) else {
                break;
            };
            match t.GetGUID(&MF_MT_MAJOR_TYPE) {
                Ok(g) if g == MFMediaType_Video && video_idx.is_none() => video_idx = Some(i),
                Ok(g) if g == MFMediaType_Audio => audio_idx.push(i),
                _ => {}
            }
        }
        let video_idx = video_idx.ok_or_else(|| AppError::msg("视频里没有画面"))?;
        let _ = reader.SetStreamSelection(MF_SOURCE_READER_ANY_STREAM.0 as u32, false);
        reader
            .SetStreamSelection(video_idx, true)
            .map_err(read_err)?;
        let native = reader.GetNativeMediaType(video_idx, 0).map_err(read_err)?;
        let (w, h) = unpack(native.GetUINT64(&MF_MT_FRAME_SIZE).map_err(read_err)?);
        let (w, h) = (w & !1, h & !1);
        let vt = MFCreateMediaType().map_err(read_err)?;
        vt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(read_err)?;
        vt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(read_err)?;
        reader
            .SetCurrentMediaType(video_idx, None, &vt)
            .map_err(read_err)?;
        // 解码出来的帧可能有对齐用的空白行（1080 → 1088）
        let decoded = |r: &IMFSourceReader| -> (u32, u32) {
            r.GetCurrentMediaType(video_idx)
                .ok()
                .map(|t| {
                    (
                        unpack(t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(pack(w, h))).1,
                        t.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(w),
                    )
                })
                .unwrap_or((h, w))
        };
        let (mut padded_h, mut stride) = decoded(&reader);

        // 声音：解码成 PCM 16 位；采样率、声道数要是不是 48k 立体声，下面自己转
        let mut audio_fmt = Vec::new();
        for &i in &audio_idx {
            reader.SetStreamSelection(i, true).map_err(read_err)?;
            let pcm = audio_type(false).map_err(read_err)?;
            if reader.SetCurrentMediaType(i, None, &pcm).is_err() {
                let t = MFCreateMediaType().map_err(read_err)?;
                t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                    .map_err(read_err)?;
                t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                    .map_err(read_err)?;
                t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                    .map_err(read_err)?;
                reader.SetCurrentMediaType(i, None, &t).map_err(read_err)?;
            }
            let cur = reader.GetCurrentMediaType(i).map_err(read_err)?;
            let rate = cur
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .unwrap_or(RATE);
            let channels = cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2).max(1) as usize;
            audio_fmt.push((rate, channels));
        }
        if start > 0 {
            reader
                .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(start))
                .map_err(read_err)?;
        }

        let outs = if audio_idx.is_empty() {
            0
        } else if mix {
            1
        } else {
            audio_idx.len()
        };
        let spec = Spec {
            w,
            h,
            enc: encoding,
            audio_tracks: outs,
            realtime: false,
        };
        let (opened, note) = open(dst, spec, &[Input::Nv12])?;
        if let Some(n) = note {
            tracing::info!("{n}");
        }
        let writer = opened.writer;
        let mut mixers: Vec<Mixer> = if mix {
            vec![Mixer::new(audio_idx.len())]
        } else {
            (0..audio_idx.len()).map(|_| Mixer::new(1)).collect()
        };
        let mut tracks: Vec<OutTrack> = opened
            .audio
            .iter()
            .map(|&s| OutTrack {
                stream: s,
                stretch: stretch.then(|| Stretcher::new(speed)),
                written: 0,
            })
            .collect();
        let mut resamplers: Vec<Resampler> = audio_fmt
            .iter()
            .map(|&(rate, _)| Resampler::new(rate))
            .collect();
        // 哪一路读完了：画面 + 每路声音
        let mut done = vec![false; 1 + audio_idx.len()];
        let mut frame = vec![0u8; (w * h * 3 / 2) as usize];
        let cut_frames = (end - start) * i64::from(RATE) / HNS;
        let mut wrote_video = false;
        let started = Instant::now();

        while done.iter().any(|d| !d) {
            if started.elapsed() > Duration::from_secs(60 * 30) {
                return Err(AppError::msg("导出视频超时"));
            }
            let (mut idx, mut flags, mut ts) = (0u32, 0u32, 0i64);
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(
                    MF_SOURCE_READER_ANY_STREAM.0 as u32,
                    0,
                    Some(&mut idx),
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(read_err)?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(AppError::msg("读取视频失败"));
            }
            let slot = if idx == video_idx {
                Some(0)
            } else {
                audio_idx.iter().position(|&i| i == idx).map(|k| k + 1)
            };
            let Some(slot) = slot else { continue };
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 && slot == 0 {
                (padded_h, stride) = decoded(&reader);
            }
            let eos = flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0;
            if let Some(sample) = sample.as_ref().filter(|_| !done[slot]) {
                let dur = sample.GetSampleDuration().unwrap_or(333_333).max(1);
                if ts >= end {
                    done[slot] = true;
                } else if slot == 0 {
                    if ts + dur > start {
                        copy_nv12(sample, w, h, padded_h, stride, &mut frame).map_err(read_err)?;
                        let t0 = rescale(ts.max(start));
                        let t1 = rescale((ts + dur).min(end));
                        let out =
                            make_sample(frame.len(), t0, t1 - t0, |b| b.copy_from_slice(&frame))
                                .map_err(read_err)?;
                        writer
                            .WriteSample(opened.video, &out)
                            .map_err(|e| mf_err("导出视频失败", e))?;
                        wrote_video = true;
                    }
                } else {
                    let k = slot - 1;
                    let (_, channels) = audio_fmt[k];
                    let buffer = sample.ConvertToContiguousBuffer().map_err(read_err)?;
                    let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
                    buffer
                        .Lock(&mut p, None, Some(&mut len))
                        .map_err(read_err)?;
                    let raw = std::slice::from_raw_parts(p, len as usize);
                    let mut stereo = Vec::with_capacity(raw.len() / channels);
                    for f in raw.chunks_exact(2 * channels) {
                        let l = f32::from(i16::from_le_bytes([f[0], f[1]])) / 32768.0;
                        let r = if channels > 1 {
                            f32::from(i16::from_le_bytes([f[2], f[3]])) / 32768.0
                        } else {
                            l
                        };
                        stereo.push(l);
                        stereo.push(r);
                    }
                    buffer.Unlock().map_err(read_err)?;
                    let pcm: Vec<f32> = resamplers[k]
                        .run(&stereo)
                        .into_iter()
                        .map(|v| f32::from(v) / 32768.0)
                        .collect();
                    let at = (ts - start) * i64::from(RATE) / HNS;
                    // 片段外面的部分裁掉
                    let lo = (-at).max(0) as usize * 2;
                    let hi = ((cut_frames - at).max(0) as usize * 2).min(pcm.len());
                    if lo < hi {
                        let (m, input) = if mix { (0, k) } else { (k, 0) };
                        mixers[m].push(input, at.max(0), &pcm[lo..hi]);
                    }
                }
            }
            if eos {
                done[slot] = true;
            }
            if done[slot] && slot > 0 {
                let k = slot - 1;
                let (m, input) = if mix { (0, k) } else { (k, 0) };
                mixers[m].finish(input);
                let _ = reader.SetStreamSelection(idx, false);
            } else if done[slot] {
                let _ = reader.SetStreamSelection(idx, false);
            }
            for (m, t) in mixers.iter_mut().zip(tracks.iter_mut()) {
                let chunk = m.drain();
                t.feed(&writer, chunk)?;
            }
        }
        for (m, t) in mixers.iter_mut().zip(tracks.iter_mut()) {
            for i in 0..m.inputs.len() {
                m.finish(i);
            }
            let chunk = m.drain();
            t.feed(&writer, chunk)?;
            t.flush(&writer)?;
            // 声音比画面短（片段后面没声音）：补静音，有的播放器音轨一完就不往下放了
            let want = rescale(end) * i64::from(RATE) / HNS;
            if t.written < want {
                let pad = vec![0f32; ((want - t.written) * 2) as usize];
                t.write(&writer, &pad)?;
            }
        }
        if !wrote_video {
            let _ = std::fs::remove_file(dst);
            return Err(AppError::msg("剪出来的片段里没有画面"));
        }
        writer.Finalize().map_err(|e| mf_err("导出视频失败", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_aligns_and_waits_for_every_input() {
        let mut m = Mixer::new(2);
        m.push(0, 0, &[0.5; 8]);
        // 第二路还没来：先不出
        assert!(m.drain().is_empty());
        m.push(1, 2, &[0.25; 4]);
        // 两路都到第 4 帧：第二路前两帧没声音
        assert_eq!(m.drain(), vec![0.5, 0.5, 0.5, 0.5, 0.75, 0.75, 0.75, 0.75]);
        m.finish(1);
        m.finish(0);
        assert!(m.drain().is_empty());
    }

    #[test]
    fn nv12_of_white_and_black() {
        let img = RgbaImage::from_fn(4, 2, |x, _| {
            if x < 2 {
                image::Rgba([255, 255, 255, 255])
            } else {
                image::Rgba([0, 0, 0, 255])
            }
        });
        let mut out = vec![0u8; 12];
        rgba_to_nv12(&img, 4, 2, &mut out);
        assert_eq!(&out[..4], &[235, 235, 16, 16]);
        // 灰色没有色度
        assert!(out[8..].iter().all(|&c| (126..=130).contains(&c)));
    }

    /// 真录一段（2 秒画面 + 系统声音 + 麦克风），再剪 0.5s..1.5s、2 倍速混音导出，再 H.265 导出：
    /// `CHENOCR_SMOKE_OUT=某个目录 cargo test --lib -- --ignored smoke_windows_video --nocapture`
    #[test]
    #[ignore]
    fn smoke_windows_video() {
        let dir = PathBuf::from(std::env::var("CHENOCR_SMOKE_OUT").unwrap());
        let (raw, cut, hevc) = (
            dir.join("raw.mp4"),
            dir.join("cut.mp4"),
            dir.join("cut-hevc.mp4"),
        );
        let mut w = VideoWriter::create(
            &raw,
            320,
            240,
            30,
            VideoOptions {
                system_audio: true,
                microphone: true,
                encoding: VideoEncoding::default(),
            },
        )
        .unwrap();
        println!("warnings: {:?}", w.warnings());
        let origin = Instant::now();
        w.start(origin);
        for i in 0..60u64 {
            let shade = (i * 4) as u8;
            let frame = RgbaImage::from_pixel(320, 240, image::Rgba([shade, 0, 255 - shade, 255]));
            let target = origin + Duration::from_millis(i * 33);
            if let Some(wait) = target.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            w.push(&frame, origin.elapsed().as_millis() as u64).unwrap();
        }
        let tracks = w.finish(2000).unwrap();
        println!("audio tracks: {tracks}");
        export(
            &raw,
            &cut,
            VideoEdit {
                start_ms: 500,
                end_ms: 1500,
                speed: 2.0,
            },
            VideoEncoding::default(),
            true,
        )
        .unwrap();
        export(
            &raw,
            &hevc,
            VideoEdit {
                start_ms: 0,
                end_ms: 2000,
                speed: 1.0,
            },
            VideoEncoding {
                hevc: true,
                ..VideoEncoding::default()
            },
            false,
        )
        .unwrap();
        println!("ok: {} {} {}", raw.display(), cut.display(), hevc.display());
    }
}
