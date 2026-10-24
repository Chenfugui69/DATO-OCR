//! 录视频（MP4）：系统的 AVAssetWriter，H.264 交给 VideoToolbox 硬件编码，不用带 FFmpeg。
//!
//! 帧只在画面变了的时候送进来（可变帧率），每帧带自己的时间（从开始录起的毫秒数）；最后一帧
//! 一直持续到 `finish` 给的结束时间。整个写入器只在一个线程（编码线程）上用。
//!
//! 声音：系统声音、麦克风各是一条 AAC 音轨，样本在各自的队列上直接写进文件（时间戳是主机时钟，
//! 和画面的时间对齐：`start` 时告诉写入器"第 0 毫秒"是主机时钟的哪一刻）。两条音轨大多数播放器
//! 只放第一条，所以录完要是有两条（又没开"独立音轨"），`export` 再导出一遍混成一条（剪辑时顺带完成）。
//!
//! 画面编码（H.264 / H.265、码率）按设置里的画质来，导出时也用同一套：`video_settings`。

use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use image::RgbaImage;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetReader, AVAssetReaderAudioMixOutput, AVAssetReaderOutput, AVAssetReaderStatus,
    AVAssetReaderTrackOutput, AVAssetTrack, AVAssetWriter, AVAssetWriterInput,
    AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus, AVAudioTimePitchAlgorithmSpectral,
    AVFileTypeMPEG4, AVMediaTypeAudio, AVMediaTypeVideo, AVMutableComposition, AVURLAsset,
    AVVideoAverageBitRateKey, AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoCodecTypeHEVC,
    AVVideoCompressionPropertiesKey, AVVideoExpectedSourceFrameRateKey, AVVideoHeightKey,
    AVVideoMaxKeyFrameIntervalDurationKey, AVVideoProfileLevelH264HighAutoLevel,
    AVVideoProfileLevelKey, AVVideoWidthKey,
};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMSampleBuffer, CMTimeRange};
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSURL};
use parking_lot::Mutex;

use super::media::{self, CaptureSource, SampleSink, SystemAudio};
use crate::error::{AppError, AppResult};
use crate::platform::types::{VideoEdit, VideoEncoding, VideoOptions};

/// 编码器暂时收不下（`isReadyForMoreMediaData` 为假）时最多等多久
const READY_WAIT: Duration = Duration::from_secs(2);
/// 'aac '
const AAC: u32 = 0x6161_6320;
/// 'lpcm'
const LPCM: u32 = 0x6c70_636d;
/// '420v'：解码出来直接是编码器要的格式，省一次转换
const YUV420V: u32 = 0x3432_3076;
/// 导出时编码器多久没收下一个样本就算卡死
const STALL: Duration = Duration::from_secs(20);

/// 一条音轨。样本在声音的队列上写，结束时在编码线程上收尾，所以包一层锁
struct AudioTrack {
    input: Mutex<Option<Retained<AVAssetWriterInput>>>,
    /// 第一段声音的时间戳和主机时钟差多少（纳秒），只记日志，看时钟对不对得上
    logged: Mutex<bool>,
    name: &'static str,
}

/// 声音那边要看的状态：会话从主机时钟的哪一刻开始（开始之前的声音丢掉）
struct AudioShared {
    start_ns: AtomicI64,
    tracks: Vec<AudioTrack>,
}

// SAFETY: AVAssetWriterInput 只在锁里用（同一时刻只有一个线程碰它）；AVAssetWriter 允许
// 不同的输入在不同的线程上追加样本
unsafe impl Send for AudioShared {}
unsafe impl Sync for AudioShared {}

impl AudioShared {
    fn append(&self, i: usize, buffer: &CMSampleBuffer) {
        let start = self.start_ns.load(Ordering::Acquire);
        if start == i64::MAX {
            return;
        }
        let Some(track) = self.tracks.get(i) else {
            return;
        };
        // SAFETY: 回调期间缓冲区有效，只读时间戳
        let pts = media::to_ns(unsafe { buffer.presentation_time_stamp() });
        {
            let mut logged = track.logged.lock();
            if !*logged {
                *logged = true;
                tracing::debug!(
                    track = track.name,
                    lag_ms = (media::host_now_ns() - pts) / 1_000_000,
                    "第一段声音"
                );
            }
        }
        if pts < start {
            return;
        }
        let guard = track.input.lock();
        let Some(input) = guard.as_ref() else { return };
        // SAFETY: 输入还没结束（结束时会先把它从锁里拿走）；样本是系统给的有效音频
        unsafe {
            if input.isReadyForMoreMediaData() && !input.appendSampleBuffer(buffer) {
                tracing::debug!(track = track.name, "写声音失败");
            }
        }
    }
}

pub struct VideoWriter {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    width: u32,
    height: u32,
    last_ms: Option<u64>,
    /// 第 0 毫秒对应的主机时钟（纳秒）；`start` 之前是 None
    origin_ns: Option<i64>,
    audio: Arc<AudioShared>,
    system: Option<SystemAudio>,
    mic: Option<CaptureSource>,
    /// 没开成的声音（没权限之类），录完提示用户
    warnings: Vec<String>,
}

fn av_err(what: &str, err: Option<Retained<objc2_foundation::NSError>>) -> AppError {
    match err {
        Some(e) => AppError::msg(format!("{what}：{}", e.localizedDescription())),
        None => AppError::msg(what.to_string()),
    }
}

fn number(v: u32) -> Retained<NSNumber> {
    NSNumber::new_u32(v)
}

fn audio_input(channels: u32, realtime: bool) -> Option<Retained<AVAssetWriterInput>> {
    // SAFETY: AVFoundation 导出的常量
    let audio = unsafe { AVMediaTypeAudio }?;
    let settings = NSDictionary::<NSString, AnyObject>::from_slices(
        &[
            &*NSString::from_str("AVFormatIDKey"),
            &*NSString::from_str("AVSampleRateKey"),
            &*NSString::from_str("AVNumberOfChannelsKey"),
            &*NSString::from_str("AVEncoderBitRateKey"),
        ],
        &[
            number(AAC).as_ref(),
            NSNumber::new_f64(48_000.0).as_ref(),
            number(channels).as_ref(),
            number(if channels > 1 { 160_000 } else { 96_000 }).as_ref(),
        ],
    );
    // SAFETY: 设置字典的键值都是 AVFoundation 认的
    unsafe {
        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            audio,
            Some(&settings),
        );
        input.setExpectsMediaDataInRealTime(realtime);
        Some(input)
    }
}

/// 画面的编码设置：H.264（High）或 H.265，码率按画质算，关键帧最多隔 5 秒
fn video_settings(
    width: u32,
    height: u32,
    enc: VideoEncoding,
) -> AppResult<Retained<NSDictionary<NSString, AnyObject>>> {
    let bitrate = enc.bitrate(width, height);
    // SAFETY: 都是 AVFoundation 导出的键和值，进程里一直有效
    unsafe {
        let (
            Some(codec_key),
            Some(width_key),
            Some(height_key),
            Some(props_key),
            Some(bitrate_key),
            Some(fps_key),
            Some(keyframe_key),
            Some(h264),
            Some(hevc),
        ) = (
            AVVideoCodecKey,
            AVVideoWidthKey,
            AVVideoHeightKey,
            AVVideoCompressionPropertiesKey,
            AVVideoAverageBitRateKey,
            AVVideoExpectedSourceFrameRateKey,
            AVVideoMaxKeyFrameIntervalDurationKey,
            AVVideoCodecTypeH264,
            AVVideoCodecTypeHEVC,
        )
        else {
            return Err(AppError::msg("系统不支持 H.264 编码"));
        };
        let rate = number(bitrate);
        let fps = number(enc.fps.max(1));
        let keyframe = NSNumber::new_f64(5.0);
        let mut keys: Vec<&NSString> = vec![bitrate_key, fps_key, keyframe_key];
        let mut values: Vec<&AnyObject> = vec![rate.as_ref(), fps.as_ref(), keyframe.as_ref()];
        if !enc.hevc {
            if let (Some(profile_key), Some(high)) =
                (AVVideoProfileLevelKey, AVVideoProfileLevelH264HighAutoLevel)
            {
                keys.push(profile_key);
                values.push(high.as_ref());
            }
        }
        let props = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &values);
        let codec: &NSString = if enc.hevc { hevc } else { h264 };
        let w = number(width);
        let h = number(height);
        Ok(NSDictionary::<NSString, AnyObject>::from_slices(
            &[codec_key, width_key, height_key, props_key],
            &[codec.as_ref(), w.as_ref(), h.as_ref(), props.as_ref()],
        ))
    }
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
        // 文件已存在的话 AVAssetWriter 直接失败
        let _ = std::fs::remove_file(path);
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        // SAFETY: 这些常量是 AVFoundation 导出的字符串，进程里一直有效
        let (file_type, media_type) = unsafe { (AVFileTypeMPEG4, AVMediaTypeVideo) };
        let (Some(file_type), Some(media_type)) = (file_type, media_type) else {
            return Err(AppError::msg("系统不支持写 MP4"));
        };
        // SAFETY: url、文件类型都有效
        let writer = unsafe { AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type) }
            .map_err(|e| av_err("创建视频文件失败", Some(e)))?;

        let settings = video_settings(
            width,
            height,
            VideoEncoding {
                fps,
                ..options.encoding
            },
        )?;
        // SAFETY: 设置字典的键值都是 AVFoundation 认的类型
        let input = unsafe {
            AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                media_type,
                Some(&settings),
            )
        };
        // SAFETY: 普通的属性设置和对象创建
        let adaptor = unsafe {
            input.setExpectsMediaDataInRealTime(true);
            AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(&input, None)
        };
        // SAFETY: 还没开始写，可以加输入
        unsafe { writer.addInput(&input) };

        // 声音：先把音轨加上（开始写之后就不能加了），再开声音源
        let mut tracks = Vec::new();
        let mut kinds = Vec::new();
        for (on, name, channels) in [
            (options.system_audio, "system", 2),
            (options.microphone, "mic", 1),
        ] {
            if !on {
                continue;
            }
            let Some(track) = audio_input(channels, true) else {
                continue;
            };
            // SAFETY: 还没开始写
            if unsafe { writer.canAddInput(&track) } {
                unsafe { writer.addInput(&track) };
                tracks.push(AudioTrack {
                    input: Mutex::new(Some(track)),
                    logged: Mutex::new(false),
                    name,
                });
                kinds.push(name);
            }
        }
        let audio = Arc::new(AudioShared {
            start_ns: AtomicI64::new(i64::MAX),
            tracks,
        });
        // SAFETY: 输入都加好了
        if !unsafe { writer.startWriting() } {
            return Err(av_err("开始写视频失败", unsafe { writer.error() }));
        }

        let mut warnings = Vec::new();
        let mut system = None;
        let mut mic = None;
        for (i, kind) in kinds.iter().enumerate() {
            let shared = audio.clone();
            let sink: SampleSink =
                Arc::new(move |buffer: &CMSampleBuffer| shared.append(i, buffer));
            match *kind {
                "system" => match SystemAudio::start(sink) {
                    Ok(s) => system = Some(s),
                    Err(err) => warnings.push(err.to_string()),
                },
                _ => match CaptureSource::microphone(sink) {
                    Ok(s) => mic = Some(s),
                    Err(err) => warnings.push(err.to_string()),
                },
            }
        }
        for w in &warnings {
            tracing::warn!("录视频的声音没开成：{w}");
        }
        Ok(Self {
            writer,
            input,
            adaptor,
            width,
            height,
            last_ms: None,
            origin_ns: None,
            audio,
            system,
            mic,
            warnings,
        })
    }

    /// 开始计时：`origin` 是第 0 毫秒（录制线程开始数时间的那一刻）。之前到的声音丢掉
    pub fn start(&mut self, origin: Instant) {
        if self.origin_ns.is_some() {
            return;
        }
        let ns = media::instant_to_host_ns(origin);
        // SAFETY: 已经开始写了，会话只开一次
        unsafe { self.writer.startSessionAtSourceTime(media::ns_time(ns)) };
        self.origin_ns = Some(ns);
        self.audio.start_ns.store(ns, Ordering::Release);
    }

    /// 没开成的声音（给用户看的原因）
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
        // 录制线程没给起点（不会发生）：就拿第一帧当起点
        if self.origin_ns.is_none() {
            self.start(Instant::now() - Duration::from_millis(ms));
        }
        let origin = self.origin_ns.unwrap_or_default();
        let started = Instant::now();
        // SAFETY: 只读状态
        while !unsafe { self.input.isReadyForMoreMediaData() } {
            if started.elapsed() > READY_WAIT {
                // 编码器卡住了：丢这一帧，别把录制拖死
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let buffer = self.pixel_buffer(frame)?;
        let at = media::ns_time(origin + ms as i64 * 1_000_000);
        // SAFETY: buffer 是刚建好、填好的 BGRA 像素缓冲；时间是递增的
        let ok = unsafe {
            self.adaptor
                .appendPixelBuffer_withPresentationTime(&buffer, at)
        };
        if !ok {
            return Err(av_err("写视频帧失败", unsafe { self.writer.error() }));
        }
        self.last_ms = Some(ms);
        Ok(())
    }

    /// RGBA → BGRA，放进一个新的 CVPixelBuffer
    fn pixel_buffer(&self, frame: &RgbaImage) -> AppResult<CFRetained<CVPixelBuffer>> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: 输出指针指向本地变量；成功时得到一个 +1 引用计数的缓冲
        let status = unsafe {
            CVPixelBufferCreate(
                None,
                w,
                h,
                kCVPixelFormatType_32BGRA,
                None,
                NonNull::from(&mut out),
            )
        };
        let Some(raw) = NonNull::new(out).filter(|_| status == 0) else {
            return Err(AppError::msg(format!("创建视频帧缓冲失败（{status}）")));
        };
        // SAFETY: CVPixelBufferCreate 返回的是自己持有的引用
        let buffer = unsafe { CFRetained::from_raw(raw) };
        // SAFETY: 锁住再写，写完解锁；每行按缓冲自己的行宽走
        unsafe {
            CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
            let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
            let stride = CVPixelBufferGetBytesPerRow(&buffer);
            if !base.is_null() {
                let src = frame.as_raw();
                let src_stride = frame.width() as usize * 4;
                for y in 0..h {
                    let row = std::slice::from_raw_parts_mut(base.add(y * stride), w * 4);
                    let from = &src[y * src_stride..y * src_stride + w * 4];
                    for (d, s) in row.chunks_exact_mut(4).zip(from.chunks_exact(4)) {
                        d[0] = s[2];
                        d[1] = s[1];
                        d[2] = s[0];
                        d[3] = 255;
                    }
                }
            }
            CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
        }
        Ok(buffer)
    }

    /// 先停声音源，再把音轨从锁里拿出来收尾（之后再来的样本直接丢掉）
    fn close_audio(&mut self) {
        if let Some(s) = self.system.take() {
            s.stop();
        }
        if let Some(m) = self.mic.take() {
            m.stop();
        }
        for track in &self.audio.tracks {
            if let Some(input) = track.input.lock().take() {
                // SAFETY: 这条音轨不会再写了
                unsafe { input.markAsFinished() };
            }
        }
    }

    /// 写完：最后一帧持续到 `end_ms`，等文件落盘。一帧都没有的话报错（文件删掉）。
    /// 返回文件里有几条音轨。
    pub fn finish(mut self, end_ms: u64) -> AppResult<usize> {
        self.close_audio();
        let (Some(last), Some(origin)) = (self.last_ms, self.origin_ns) else {
            // SAFETY: 放弃写入
            unsafe { self.writer.cancelWriting() };
            return Err(AppError::msg("没有录到画面"));
        };
        let (tx, rx) = mpsc::channel::<()>();
        let done = RcBlock::new(move || {
            let _ = tx.send(());
        });
        let end = origin + end_ms.max(last + 1) as i64 * 1_000_000;
        // SAFETY: 结束时间不早于最后一帧；回调在别的线程上调，只发个信号
        unsafe {
            self.input.markAsFinished();
            self.writer.endSessionAtSourceTime(media::ns_time(end));
            self.writer.finishWritingWithCompletionHandler(&done);
        }
        if rx.recv_timeout(Duration::from_secs(30)).is_err() {
            return Err(AppError::msg("写视频文件超时"));
        }
        // SAFETY: 只读状态
        match unsafe { self.writer.status() } {
            AVAssetWriterStatus::Completed => Ok(self.audio.tracks.len()),
            _ => Err(av_err("写视频文件失败", unsafe {
                self.writer.error()
            })),
        }
    }
}

impl Drop for VideoWriter {
    fn drop(&mut self) {
        self.close_audio();
    }
}

/// 剪辑 / 混音：从 `src` 取 `[start, end)` 一段、按倍速，重新编码到 `dst`。
///
/// 不用 AVAssetExportSession：它的预设只有"最高质量"这类，码率很高，剪一刀文件反而比原来大好几倍。
/// 这里自己读（AVAssetReader，读组合好的那一段，变速由组合完成）、自己写（AVAssetWriter，
/// 画面编码和录制时一样按 `encoding`）。`mix` 时几条音轨混成一条，否则每条各自保留；
/// 变速时声音保持原来的音调。
// 同步读轨道的老接口（新接口是异步回调），这里本来就在后台线程上等结果，用老的简单
#[allow(deprecated)]
pub fn export(
    src: &Path,
    dst: &Path,
    edit: VideoEdit,
    encoding: VideoEncoding,
    mix: bool,
) -> AppResult<()> {
    let _ = std::fs::remove_file(dst);
    let src_url = NSURL::fileURLWithPath(&NSString::from_str(&src.to_string_lossy()));
    let dst_url = NSURL::fileURLWithPath(&NSString::from_str(&dst.to_string_lossy()));
    // SAFETY: AVFoundation 导出的常量
    let (Some(video_type), Some(audio_type), Some(file_type)) = (
        unsafe { AVMediaTypeVideo },
        unsafe { AVMediaTypeAudio },
        unsafe { AVFileTypeMPEG4 },
    ) else {
        return Err(AppError::msg("系统不支持导出视频"));
    };
    // SAFETY: 组合、读写器都在这个线程上建和用，参数是刚建好的对象；样本缓冲只在这一轮里用
    unsafe {
        let asset = AVURLAsset::URLAssetWithURL_options(&src_url, None);
        let total = media::to_ns(asset.duration());
        let start = (edit.start_ms as i64 * 1_000_000).clamp(0, total);
        let end = (edit.end_ms as i64 * 1_000_000).clamp(start, total);
        if end - start < 100_000_000 {
            return Err(AppError::msg("剪出来的片段太短了"));
        }
        let comp = AVMutableComposition::composition();
        for media_type in [video_type, audio_type] {
            for track in asset.tracksWithMediaType(media_type).iter() {
                // 这条轨道和要的那一段重叠的部分（声音可能比画面晚开始一点）
                let range = track.timeRange();
                let t0 = media::to_ns(range.start).max(start);
                let t1 = (media::to_ns(range.start) + media::to_ns(range.duration)).min(end);
                if t1 <= t0 {
                    continue;
                }
                let Some(dest) = comp.addMutableTrackWithMediaType_preferredTrackID(media_type, 0)
                else {
                    continue;
                };
                let piece = CMTimeRange {
                    start: media::ns_time(t0),
                    duration: media::ns_time(t1 - t0),
                };
                if let Err(err) = dest.insertTimeRange_ofTrack_atTime_error(
                    piece,
                    &track,
                    media::ns_time(t0 - start),
                ) {
                    return Err(AppError::msg(format!(
                        "剪辑视频失败：{}",
                        err.localizedDescription()
                    )));
                }
            }
        }
        let length = end - start;
        let speed = edit.speed.clamp(0.25, 4.0);
        if (speed - 1.0).abs() > 0.001 {
            comp.scaleTimeRange_toDuration(
                CMTimeRange {
                    start: media::ns_time(0),
                    duration: media::ns_time(length),
                },
                media::ns_time((length as f64 / speed) as i64),
            );
        }

        // ── 读：画面解码成 420v，声音解码成 48k 立体声 PCM（几条混成一条，或各自一路） ──
        let reader = AVAssetReader::assetReaderWithAsset_error(&comp)
            .map_err(|e| av_err("读取视频失败", Some(e)))?;
        let videos = comp.tracksWithMediaType(video_type);
        let Some(vtrack) = videos.iter().next() else {
            return Err(AppError::msg("视频里没有画面"));
        };
        let size = vtrack.naturalSize();
        let (w, h) = (
            ((size.width.round() as u32) & !1).max(2),
            ((size.height.round() as u32) & !1).max(2),
        );
        let pixel = NSDictionary::<NSString, AnyObject>::from_slices(
            &[&*NSString::from_str("PixelFormatType")],
            &[number(YUV420V).as_ref()],
        );
        let vout = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
            &vtrack,
            Some(&pixel),
        );
        vout.setAlwaysCopiesSampleData(false);
        reader.addOutput(&vout);

        let pcm = NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                &*NSString::from_str("AVFormatIDKey"),
                &*NSString::from_str("AVSampleRateKey"),
                &*NSString::from_str("AVNumberOfChannelsKey"),
                &*NSString::from_str("AVLinearPCMBitDepthKey"),
                &*NSString::from_str("AVLinearPCMIsFloatKey"),
                &*NSString::from_str("AVLinearPCMIsBigEndianKey"),
                &*NSString::from_str("AVLinearPCMIsNonInterleaved"),
            ],
            &[
                number(LPCM).as_ref(),
                NSNumber::new_f64(48_000.0).as_ref(),
                number(2).as_ref(),
                number(16).as_ref(),
                NSNumber::new_bool(false).as_ref(),
                NSNumber::new_bool(false).as_ref(),
                NSNumber::new_bool(false).as_ref(),
            ],
        );
        let audios = comp.tracksWithMediaType(audio_type);
        let audio_tracks: Vec<Retained<AVAssetTrack>> = audios
            .iter()
            .map(|t| Retained::into_super(Retained::into_super(t)))
            .collect();
        let groups: Vec<Vec<&AVAssetTrack>> = if audio_tracks.is_empty() {
            Vec::new()
        } else if mix {
            vec![audio_tracks.iter().map(|t| &**t).collect()]
        } else {
            audio_tracks.iter().map(|t| vec![&**t]).collect()
        };
        let mut aouts = Vec::new();
        for group in groups {
            let out =
                AVAssetReaderAudioMixOutput::assetReaderAudioMixOutputWithAudioTracks_audioSettings(
                    &NSArray::from_slice(&group),
                    Some(&pcm),
                );
            out.setAlwaysCopiesSampleData(false);
            if let Some(spectral) = AVAudioTimePitchAlgorithmSpectral {
                out.setAudioTimePitchAlgorithm(spectral);
            }
            if reader.canAddOutput(&out) {
                reader.addOutput(&out);
                aouts.push(Retained::into_super(out));
            }
        }

        // ── 写：画面按 encoding 编码，声音 AAC ──
        let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&dst_url, file_type)
            .map_err(|e| av_err("创建视频文件失败", Some(e)))?;
        let settings = video_settings(w, h, encoding)?;
        let vin = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            video_type,
            Some(&settings),
        );
        vin.setExpectsMediaDataInRealTime(false);
        vin.setTransform(vtrack.preferredTransform());
        writer.addInput(&vin);
        let mut pairs: Vec<(
            Retained<AVAssetReaderOutput>,
            Retained<AVAssetWriterInput>,
            bool,
        )> = vec![(Retained::into_super(vout), vin, false)];
        for out in aouts {
            let Some(ain) = audio_input(2, false) else {
                continue;
            };
            if writer.canAddInput(&ain) {
                writer.addInput(&ain);
                pairs.push((out, ain, false));
            }
        }
        if !writer.startWriting() {
            return Err(av_err("开始写视频失败", writer.error()));
        }
        if !reader.startReading() {
            writer.cancelWriting();
            return Err(av_err("读取视频失败", reader.error()));
        }
        writer.startSessionAtSourceTime(media::ns_time(0));

        // 一个线程轮流喂：哪路编码器收得下就往哪路塞（写入器要画面、声音交错着来，只喂一路会卡住）
        let mut last_progress = Instant::now();
        loop {
            let mut progressed = false;
            let mut pending = false;
            for (out, input, done) in pairs.iter_mut() {
                if *done {
                    continue;
                }
                pending = true;
                while input.isReadyForMoreMediaData() {
                    match out.copyNextSampleBuffer() {
                        Some(buffer) => {
                            if !input.appendSampleBuffer(&buffer) {
                                reader.cancelReading();
                                return Err(av_err("导出视频失败", writer.error()));
                            }
                        }
                        None => {
                            input.markAsFinished();
                            *done = true;
                        }
                    }
                    progressed = true;
                    if *done {
                        break;
                    }
                }
            }
            if !pending {
                break;
            }
            if reader.status() == AVAssetReaderStatus::Failed {
                writer.cancelWriting();
                return Err(av_err("读取视频失败", reader.error()));
            }
            if writer.status() == AVAssetWriterStatus::Failed {
                reader.cancelReading();
                return Err(av_err("导出视频失败", writer.error()));
            }
            if progressed {
                last_progress = Instant::now();
            } else if last_progress.elapsed() > STALL {
                reader.cancelReading();
                writer.cancelWriting();
                return Err(AppError::msg("导出视频卡住了"));
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let (tx, rx) = mpsc::channel::<()>();
        let done = RcBlock::new(move || {
            let _ = tx.send(());
        });
        writer.finishWritingWithCompletionHandler(&done);
        if rx.recv_timeout(Duration::from_secs(60)).is_err() {
            return Err(AppError::msg("写视频文件超时"));
        }
        match writer.status() {
            AVAssetWriterStatus::Completed => Ok(()),
            _ => Err(av_err("导出视频失败", writer.error())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真录一段（2 秒画面 + 系统声音），再剪 0.5s..1.5s、2 倍速导出。要屏幕录制权限：
    /// `CHENOCR_SMOKE_OUT=/某个目录 cargo test --lib -- --ignored smoke_video_audio_export --nocapture`
    #[test]
    #[ignore]
    fn smoke_video_audio_export() {
        let dir = std::path::PathBuf::from(std::env::var("CHENOCR_SMOKE_OUT").unwrap());
        let (raw, cut) = (dir.join("raw.mp4"), dir.join("cut.mp4"));
        let mut w = VideoWriter::create(
            &raw,
            320,
            240,
            30,
            VideoOptions {
                system_audio: true,
                microphone: false,
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
        // H.265、音轨不混
        let hevc = dir.join("cut-hevc.mp4");
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
