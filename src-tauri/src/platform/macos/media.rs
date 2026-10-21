//! 录视频时的声音和摄像头画面。
//!
//! - 系统声音：ScreenCaptureKit 开一个只要音频的流（画面给 2×2、一秒一帧，不加画面输出），
//!   排除自己进程的声音。用的是已有的「屏幕录制」权限，不用另外授权
//! - 麦克风、摄像头：AVCaptureSession。第一次用系统会问用户要不要允许；没允许就不录这一路
//!
//! 回调在各自的串行队列上来，给的是带主机时钟（mach_absolute_time）时间戳的 CMSampleBuffer，
//! 和 `Instant` 是同一个钟，录制那边按这个对齐画面和声音。

use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use image::RgbaImage;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureAudioDataOutput, AVCaptureAudioDataOutputSampleBufferDelegate,
    AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput, AVCaptureOutput, AVCaptureSession,
    AVCaptureSessionPreset640x480, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaType, AVMediaTypeAudio, AVMediaTypeVideo,
};
use objc2_core_media::{CMClock, CMSampleBuffer, CMTime};
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput,
    SCStreamOutputType,
};
use parking_lot::Mutex;

use crate::error::{AppError, AppResult};
use crate::platform::types::{MediaAccess, MediaKind};

/// 收到一段声音（或一帧摄像头画面）时调的回调
pub type SampleSink = Arc<dyn Fn(&CMSampleBuffer) + Send + Sync>;

/// 等系统的异步回调最多等多久
const WAIT: Duration = Duration::from_secs(5);

// ───────────────────────── 时间 ─────────────────────────

/// 主机时钟现在的时间（纳秒）
pub fn host_now_ns() -> i64 {
    // SAFETY: 系统的主机时钟一直有效，取时间没有副作用
    to_ns(unsafe { CMClock::host_time_clock().time() })
}

/// `Instant` 换成主机时钟的纳秒数（两者都是 mach_absolute_time，只差一个起点）
pub fn instant_to_host_ns(at: Instant) -> i64 {
    let now = Instant::now();
    let host = host_now_ns();
    if at <= now {
        host - (now - at).as_nanos() as i64
    } else {
        host + (at - now).as_nanos() as i64
    }
}

pub fn to_ns(t: CMTime) -> i64 {
    if t.timescale <= 0 {
        return 0;
    }
    (i128::from(t.value) * 1_000_000_000 / i128::from(t.timescale)) as i64
}

pub fn ns_time(ns: i64) -> CMTime {
    // SAFETY: 纯值构造
    unsafe { CMTime::new(ns, 1_000_000_000) }
}

// ───────────────────────── 回调对象 ─────────────────────────

pub struct SinkIvars {
    sink: SampleSink,
}

define_class!(
    /// ScreenCaptureKit 的输出：只转发音频
    #[unsafe(super(NSObject))]
    #[name = "DatoRecordAudioOutput"]
    #[ivars = SinkIvars]
    struct StreamAudioOutput;

    unsafe impl NSObjectProtocol for StreamAudioOutput {}

    unsafe impl SCStreamOutput for StreamAudioOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(
            &self,
            _stream: &SCStream,
            buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind == SCStreamOutputType::Audio {
                (self.ivars().sink)(buffer);
            }
        }
    }
);

define_class!(
    /// AVCaptureSession 的输出（麦克风、摄像头共用）：原样转发
    #[unsafe(super(NSObject))]
    #[name = "DatoRecordCaptureOutput"]
    #[ivars = SinkIvars]
    struct CaptureOutput;

    unsafe impl NSObjectProtocol for CaptureOutput {}

    unsafe impl AVCaptureAudioDataOutputSampleBufferDelegate for CaptureOutput {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn capture_did_output(
            &self,
            _output: &AVCaptureOutput,
            buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            (self.ivars().sink)(buffer);
        }
    }

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CaptureOutput {}
);

impl StreamAudioOutput {
    fn new(sink: SampleSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(SinkIvars { sink });
        // SAFETY: NSObject 的 init
        unsafe { msg_send![super(this), init] }
    }
}

impl CaptureOutput {
    fn new(sink: SampleSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(SinkIvars { sink });
        // SAFETY: NSObject 的 init
        unsafe { msg_send![super(this), init] }
    }
}

/// 系统对象跨线程递回来（只在回调里递一下，拿到后只在一个线程上用）
struct SendBox<T>(T);
// SAFETY: 见上，装的东西不会在两个线程上同时用
unsafe impl<T> Send for SendBox<T> {}

fn error_text(err: *mut NSError) -> Option<String> {
    // SAFETY: 系统给的要么是空指针、要么是有效的 NSError
    unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string())
}

/// 调一个带 `completionHandler:(NSError *)` 的方法并等它回来
fn wait_done(
    what: &str,
    call: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>),
) -> AppResult<()> {
    let (tx, rx) = mpsc::channel::<Option<String>>();
    let block = RcBlock::new(move |err: *mut NSError| {
        let _ = tx.send(error_text(err));
    });
    call(&block);
    match rx.recv_timeout(WAIT) {
        Ok(None) => Ok(()),
        Ok(Some(msg)) => Err(AppError::msg(format!("{what}：{msg}"))),
        Err(_) => Err(AppError::msg(format!("{what}：系统没有响应"))),
    }
}

// ───────────────────────── 系统声音 ─────────────────────────

pub struct SystemAudio {
    stream: Retained<SCStream>,
    _output: Retained<StreamAudioOutput>,
    _queue: DispatchRetained<DispatchQueue>,
}

impl SystemAudio {
    /// 开始录系统声音（48kHz 双声道），每段声音交给 `sink`
    pub fn start(sink: SampleSink) -> AppResult<Self> {
        let (tx, rx) = mpsc::channel::<Result<SendBox<Retained<SCShareableContent>>, String>>();
        let block = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
            // SAFETY: 系统给的指针要么空、要么有效；retain 一下自己持有
            let got = unsafe { Retained::retain(content) };
            let _ = tx.send(match got {
                Some(c) => Ok(SendBox(c)),
                None => Err(error_text(err).unwrap_or_else(|| "拿不到屏幕信息".into())),
            });
        });
        // SAFETY: 回调只往通道里发
        unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
        let content = match rx.recv_timeout(WAIT) {
            Ok(Ok(c)) => c.0,
            Ok(Err(msg)) => return Err(AppError::msg(format!("录系统声音失败：{msg}"))),
            Err(_) => return Err(AppError::msg("录系统声音失败：系统没有响应")),
        };
        // SAFETY: 只读属性
        let display = unsafe { content.displays() }
            .firstObject()
            .ok_or_else(|| AppError::msg("录系统声音失败：没有显示器"))?;
        let empty = NSArray::new();
        // SAFETY: 新建过滤器、配置和流；参数都有效
        let stream = unsafe {
            let filter = SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &empty,
            );
            let config = SCStreamConfiguration::new();
            config.setCapturesAudio(true);
            config.setExcludesCurrentProcessAudio(true);
            config.setSampleRate(48_000);
            config.setChannelCount(2);
            // 画面不要：最小尺寸、一秒一帧，也不加画面输出
            config.setWidth(2);
            config.setHeight(2);
            config.setMinimumFrameInterval(CMTime::new(1, 1));
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                None,
            )
        };
        let output = StreamAudioOutput::new(sink);
        let queue = DispatchQueue::new("com.datocor.record.system-audio", None);
        // SAFETY: 输出对象和队列都由 self 持有，活得比流长
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Audio,
                Some(&queue),
            )
        }
        .map_err(|e| AppError::msg(format!("录系统声音失败：{}", e.localizedDescription())))?;
        // SAFETY: 回调只往通道里发
        wait_done("录系统声音失败", |b| unsafe {
            stream.startCaptureWithCompletionHandler(Some(b));
        })?;
        Ok(Self {
            stream,
            _output: output,
            _queue: queue,
        })
    }

    pub fn stop(&self) {
        // SAFETY: 回调只往通道里发
        let result = wait_done("停止录系统声音", |b| unsafe {
            self.stream.stopCaptureWithCompletionHandler(Some(b));
        });
        if let Err(err) = result {
            tracing::warn!("{err}");
        }
    }
}

// ───────────────────────── 麦克风、摄像头 ─────────────────────────

fn media_type(kind: MediaKind) -> AppResult<&'static AVMediaType> {
    // SAFETY: AVFoundation 导出的常量
    let t = unsafe {
        match kind {
            MediaKind::Microphone => AVMediaTypeAudio,
            MediaKind::Camera => AVMediaTypeVideo,
        }
    };
    t.ok_or_else(|| AppError::msg("系统不支持这种设备"))
}

/// 麦克风 / 摄像头的授权状态
pub fn media_access(kind: MediaKind) -> MediaAccess {
    let Ok(t) = media_type(kind) else {
        return MediaAccess::Unsupported;
    };
    // SAFETY: 只读状态；媒体类型是音频或视频
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(t) } {
        AVAuthorizationStatus::Authorized => MediaAccess::Granted,
        AVAuthorizationStatus::NotDetermined => MediaAccess::NotDetermined,
        _ => MediaAccess::Denied,
    }
}

/// 弹系统的授权框（还没问过时），等用户点完（最多一分钟）返回结果
pub fn request_media_access(kind: MediaKind) -> MediaAccess {
    let Ok(t) = media_type(kind) else {
        return MediaAccess::Unsupported;
    };
    if media_access(kind) != MediaAccess::NotDetermined {
        return media_access(kind);
    }
    let (tx, rx) = mpsc::channel::<bool>();
    let block = RcBlock::new(move |granted: Bool| {
        let _ = tx.send(granted.as_bool());
    });
    // SAFETY: 回调只往通道里发
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(t, &block) };
    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(true) => MediaAccess::Granted,
        Ok(false) => MediaAccess::Denied,
        Err(_) => media_access(kind),
    }
}

/// 开着的一路麦克风 / 摄像头；drop 时停
pub struct CaptureSource {
    session: Retained<AVCaptureSession>,
    _output: Retained<CaptureOutput>,
    _queue: DispatchRetained<DispatchQueue>,
}

impl CaptureSource {
    fn start(kind: MediaKind, sink: SampleSink) -> AppResult<Self> {
        let mic = kind == MediaKind::Microphone;
        // 整句写出来（英文界面按整句翻译）
        let unusable = || {
            AppError::msg(if mic {
                "麦克风用不了"
            } else {
                "摄像头用不了"
            })
        };
        if media_access(kind) != MediaAccess::Granted {
            return Err(AppError::msg(if mic {
                "没有麦克风权限"
            } else {
                "没有摄像头权限"
            }));
        }
        let t = media_type(kind)?;
        // SAFETY: 媒体类型是音频或视频
        let device =
            unsafe { AVCaptureDevice::defaultDeviceWithMediaType(t) }.ok_or_else(|| {
                AppError::msg(if mic {
                    "找不到麦克风"
                } else {
                    "找不到摄像头"
                })
            })?;
        // SAFETY: 设备有效
        let input =
            unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }.map_err(|e| {
                let why = e.localizedDescription();
                AppError::msg(if mic {
                    format!("打不开麦克风：{why}")
                } else {
                    format!("打不开摄像头：{why}")
                })
            })?;
        let output = CaptureOutput::new(sink);
        let queue = DispatchQueue::new("com.datocor.record.capture", None);
        // SAFETY: 会话还没开始，按顺序配好输入输出；回调对象和队列由 self 持有
        let session = unsafe {
            let session = AVCaptureSession::new();
            session.beginConfiguration();
            if !session.canAddInput(&input) {
                return Err(unusable());
            }
            session.addInput(&input);
            match kind {
                MediaKind::Microphone => {
                    let out = AVCaptureAudioDataOutput::new();
                    // 统一成 48kHz 单声道 32 位浮点，写进文件时再压成 AAC
                    let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                        &[
                            &*NSString::from_str("AVFormatIDKey"),
                            &*NSString::from_str("AVSampleRateKey"),
                            &*NSString::from_str("AVNumberOfChannelsKey"),
                            &*NSString::from_str("AVLinearPCMBitDepthKey"),
                            &*NSString::from_str("AVLinearPCMIsFloatKey"),
                            &*NSString::from_str("AVLinearPCMIsNonInterleaved"),
                        ],
                        &[
                            NSNumber::new_u32(LPCM).as_ref(),
                            NSNumber::new_f64(48_000.0).as_ref(),
                            NSNumber::new_u32(1).as_ref(),
                            NSNumber::new_u32(32).as_ref(),
                            NSNumber::new_bool(true).as_ref(),
                            NSNumber::new_bool(false).as_ref(),
                        ],
                    );
                    out.setAudioSettings(Some(&settings));
                    out.setSampleBufferDelegate_queue(
                        Some(ProtocolObject::from_ref(&*output)),
                        Some(&queue),
                    );
                    if !session.canAddOutput(&out) {
                        return Err(unusable());
                    }
                    session.addOutput(&out);
                }
                MediaKind::Camera => {
                    if session.canSetSessionPreset(AVCaptureSessionPreset640x480) {
                        session.setSessionPreset(AVCaptureSessionPreset640x480);
                    }
                    let out = AVCaptureVideoDataOutput::new();
                    let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                        &[&*NSString::from_str("PixelFormatType")],
                        &[NSNumber::new_u32(kCVPixelFormatType_32BGRA).as_ref()],
                    );
                    out.setVideoSettings(Some(&settings));
                    out.setAlwaysDiscardsLateVideoFrames(true);
                    out.setSampleBufferDelegate_queue(
                        Some(ProtocolObject::from_ref(&*output)),
                        Some(&queue),
                    );
                    if !session.canAddOutput(&out) {
                        return Err(unusable());
                    }
                    session.addOutput(&out);
                }
            }
            session.commitConfiguration();
            session.startRunning();
            session
        };
        Ok(Self {
            session,
            _output: output,
            _queue: queue,
        })
    }

    pub fn microphone(sink: SampleSink) -> AppResult<Self> {
        Self::start(MediaKind::Microphone, sink)
    }

    pub fn stop(&self) {
        // SAFETY: 停止会话；之后不会再有回调
        unsafe { self.session.stopRunning() };
    }
}

impl Drop for CaptureSource {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 'lpcm'
const LPCM: u32 = 0x6C70_636D;

/// 摄像头：一直开着，最新一帧裁成正方形、缩到 `size`、左右镜像（像照镜子），录制那边随时取
pub struct Camera {
    _source: CaptureSource,
    latest: Arc<Mutex<Option<RgbaImage>>>,
}

impl Camera {
    pub fn start(size: u32) -> AppResult<Self> {
        let latest = Arc::new(Mutex::new(None));
        let slot = latest.clone();
        let size = size.clamp(16, 1024);
        let sink: SampleSink = Arc::new(move |buffer: &CMSampleBuffer| {
            if let Some(img) = square_frame(buffer, size) {
                *slot.lock() = Some(img);
            }
        });
        let source = CaptureSource::start(MediaKind::Camera, sink)?;
        Ok(Self {
            _source: source,
            latest,
        })
    }

    pub fn latest(&self) -> Option<RgbaImage> {
        self.latest.lock().clone()
    }
}

/// 摄像头的一帧（BGRA）→ 中间裁正方形、缩到 size × size、镜像的 RGBA
fn square_frame(buffer: &CMSampleBuffer, size: u32) -> Option<RgbaImage> {
    // SAFETY: 回调期间缓冲区有效；锁住再读，读完解锁
    unsafe {
        let image = buffer.image_buffer()?;
        CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly);
        let base = CVPixelBufferGetBaseAddress(&image).cast::<u8>();
        let (w, h) = (
            CVPixelBufferGetWidth(&image),
            CVPixelBufferGetHeight(&image),
        );
        let stride = CVPixelBufferGetBytesPerRow(&image);
        let out = if base.is_null() || w == 0 || h == 0 {
            None
        } else {
            let src = std::slice::from_raw_parts(base, stride * h);
            let side = w.min(h);
            let (x0, y0) = ((w - side) / 2, (h - side) / 2);
            let n = size as usize;
            let mut img = RgbaImage::new(size, size);
            for (y, row) in img.as_mut().chunks_exact_mut(n * 4).enumerate() {
                let sy = y0 + y * side / n;
                for (x, px) in row.chunks_exact_mut(4).enumerate() {
                    // 镜像：输出的左边取原图的右边
                    let sx = x0 + (n - 1 - x) * side / n;
                    let i = sy * stride + sx * 4;
                    px.copy_from_slice(&[src[i + 2], src[i + 1], src[i], 255]);
                }
            }
            Some(img)
        };
        CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly);
        out
    }
}
