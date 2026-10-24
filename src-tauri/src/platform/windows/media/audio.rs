//! 录视频时的声音：WASAPI 共享模式采集。
//!
//! - 系统声音：默认播放设备的回环（loopback），听到什么录什么。没有声音在放的时候系统一个包都不给，
//!   所以采集线程每一拍还会按墙上时钟告诉接收方"现在到哪了"，那边补上静音，音轨不断档
//! - 麦克风：默认录音设备
//!
//! 每个包都带系统给的时间（QPC，100 纳秒），减去录制的第 0 毫秒就是它在视频里的位置；格式统一转成
//! 48kHz 立体声 16 位（AAC 编码器要的），设备是别的采样率就线性插值重采样。

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::error::{AppError, AppResult};

/// 输出的采样率（AAC 编码器认 44.1k / 48k）
pub const RATE: u32 = 48_000;
/// 一秒多少个 100 纳秒
pub const HNS: i64 = 10_000_000;

/// 采到的声音交给谁
pub trait AudioSink: Send + Sync {
    /// 一段声音：`at` 是它开头在视频里的时间（100 纳秒，可能是负的：录制开始之前），48k 立体声交错的样本
    fn packet(&self, at: i64, samples: &[i16]);
    /// 现在到视频的什么时候了（100 纳秒）：这之前一直没来声音的话补静音
    fn tick(&self, now: i64);
}

/// 现在的 QPC 时间（100 纳秒），和 WASAPI 给包打的时间戳同一个钟
pub fn qpc_now() -> i64 {
    let (mut count, mut freq) = (0i64, 0i64);
    // SAFETY: 只读系统计数器
    unsafe {
        let _ = QueryPerformanceCounter(&mut count);
        let _ = QueryPerformanceFrequency(&mut freq);
    }
    if freq <= 0 {
        return 0;
    }
    ((count as i128) * i128::from(HNS) / i128::from(freq)) as i64
}

/// 一个 `Instant` 换成 QPC 时间（100 纳秒）
pub fn qpc_of(at: std::time::Instant) -> i64 {
    let now = std::time::Instant::now();
    let back = now.saturating_duration_since(at).as_nanos() as i64 / 100;
    qpc_now() - back
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    System,
    Microphone,
}

pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// 开始采：设备打不开就报错（给用户看的原因）。`origin`：录制第 0 毫秒的 QPC 时间，没定下来之前是 i64::MAX
    pub fn start(
        source: Source,
        sink: Arc<dyn AudioSink>,
        origin: Arc<AtomicI64>,
    ) -> AppResult<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name(match source {
                Source::System => "audio-system".into(),
                Source::Microphone => "audio-mic".into(),
            })
            .spawn(move || {
                let stream = match Stream::open(source) {
                    Ok(s) => {
                        let _ = ready_tx.send(Ok(()));
                        s
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                stream.run(&*sink, &origin, &flag);
            })?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => Err(AppError::Msg(e)),
            Err(_) => Err(AppError::msg(match source {
                Source::System => "系统声音打不开（设备没有响应）",
                Source::Microphone => "麦克风打不开（设备没有响应）",
            })),
        }
    }

    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.halt();
    }
}

/// 设备给的样本格式
#[derive(Clone, Copy)]
enum Sample {
    F32,
    I16,
    I24,
    I32,
}

struct Stream {
    source: Source,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    rate: u32,
    channels: usize,
    sample: Sample,
    /// 每个样本几个字节（一个声道）
    bytes: usize,
}

impl Stream {
    fn open(source: Source) -> AppResult<Self> {
        super::init()?;
        // 整句写出来（英文界面按整句换）
        let fail = |e: windows::core::Error| {
            AppError::msg(match source {
                Source::System => format!("打不开系统声音：{}", e.message()),
                Source::Microphone => format!("打不开麦克风：{}", e.message()),
            })
        };
        // SAFETY: 标准的 WASAPI 打开流程；混音格式由系统分配，用完 CoTaskMemFree
        unsafe {
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(fail)?;
            let device = devices
                .GetDefaultAudioEndpoint(
                    if source == Source::System {
                        eRender
                    } else {
                        eCapture
                    },
                    eConsole,
                )
                .map_err(|_| {
                    AppError::msg(match source {
                        Source::System => "没有找到播放声音的设备",
                        Source::Microphone => "没有找到麦克风",
                    })
                })?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(fail)?;
            let format = client.GetMixFormat().map_err(fail)?;
            let parsed = parse_format(&*format);
            let flags = if source == Source::System {
                AUDCLNT_STREAMFLAGS_LOOPBACK
            } else {
                0
            };
            // 缓冲 200ms，每 10ms 取一次
            let init =
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, format, None);
            CoTaskMemFree(Some(format.cast()));
            init.map_err(fail)?;
            let (rate, channels, sample, bytes) =
                parsed.ok_or_else(|| {
                    AppError::msg(match source {
                        Source::System => "系统声音的格式不支持",
                        Source::Microphone => "麦克风的格式不支持",
                    })
                })?;
            let capture: IAudioCaptureClient = client.GetService().map_err(fail)?;
            client.Start().map_err(fail)?;
            Ok(Self {
                source,
                client,
                capture,
                rate,
                channels,
                sample,
                bytes,
            })
        }
    }

    fn run(&self, sink: &dyn AudioSink, origin: &AtomicI64, stop: &AtomicBool) {
        let mut resampler = Resampler::new(self.rate);
        // 下一个包按样本数接着上一个包的时间（设备时间戳有抖动，连着的包别一个个对时间）
        let mut expect: Option<i64> = None;
        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(10));
            let zero = origin.load(Ordering::Acquire);
            loop {
                // SAFETY: 取下一个包的大小
                let Ok(size) = (unsafe { self.capture.GetNextPacketSize() }) else {
                    // 设备没了（拔了耳机之类）：不录了，后面靠收尾补静音
                    tracing::warn!(source = ?self.source, "录音设备断开了");
                    return;
                };
                if size == 0 {
                    break;
                }
                let (mut data, mut frames, mut flags, mut qpc) =
                    (std::ptr::null_mut(), 0u32, 0u32, 0u64);
                // SAFETY: 系统给的缓冲区，ReleaseBuffer 之前有效
                if unsafe {
                    self.capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))
                }
                .is_err()
                {
                    break;
                }
                let n = frames as usize;
                let stereo = if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null()
                {
                    vec![0f32; n * 2]
                } else {
                    // SAFETY: 缓冲区是 n 帧 × 声道数 × 每样本字节数
                    let raw =
                        unsafe { std::slice::from_raw_parts(data, n * self.channels * self.bytes) };
                    to_stereo(raw, self.channels, self.sample, self.bytes)
                };
                // SAFETY: 和 GetBuffer 配对
                let _ = unsafe { self.capture.ReleaseBuffer(frames) };
                if zero == i64::MAX {
                    // 还没开始录：丢掉，下一个包重新对时间
                    expect = None;
                    resampler.reset();
                    continue;
                }
                let stamped = qpc as i64 - zero;
                let at = match expect {
                    // 和按样本数推出来的时间差不到 30ms：接着上一个包
                    Some(e) if (stamped - e).abs() < 300_000 => e,
                    _ => {
                        resampler.reset();
                        stamped
                    }
                };
                expect = Some(at + n as i64 * HNS / i64::from(self.rate));
                let out = resampler.run(&stereo);
                if !out.is_empty() {
                    sink.packet(at, &out);
                }
            }
            if zero != i64::MAX {
                sink.tick(qpc_now() - zero);
            }
        }
        // SAFETY: 停流
        let _ = unsafe { self.client.Stop() };
    }
}

/// 混音格式 → (采样率, 声道数, 样本格式, 每样本字节数)
fn parse_format(f: &WAVEFORMATEX) -> Option<(u32, usize, Sample, usize)> {
    const PCM: u16 = 1;
    const FLOAT: u16 = 3;
    const EXTENSIBLE: u16 = 0xFFFE;
    let tag = match f.wFormatTag {
        EXTENSIBLE => {
            // SAFETY: 标记是 EXTENSIBLE 时结构体就是 WAVEFORMATEXTENSIBLE
            let ext = unsafe { &*(f as *const WAVEFORMATEX).cast::<WAVEFORMATEXTENSIBLE>() };
            let sub = ext.SubFormat;
            // KSDATAFORMAT_SUBTYPE_PCM / IEEE_FLOAT 的前 4 字节就是格式号
            sub.data1 as u16
        }
        t => t,
    };
    let bits = f.wBitsPerSample;
    let sample = match (tag, bits) {
        (FLOAT, 32) => Sample::F32,
        (PCM, 16) => Sample::I16,
        (PCM, 24) => Sample::I24,
        (PCM, 32) => Sample::I32,
        _ => return None,
    };
    Some((
        f.nSamplesPerSec,
        usize::from(f.nChannels.max(1)),
        sample,
        usize::from(bits / 8),
    ))
}

/// 交错的多声道样本 → 交错的立体声 f32。单声道复制成两路；三路以上的把中置混进左右
fn to_stereo(raw: &[u8], channels: usize, sample: Sample, bytes: usize) -> Vec<f32> {
    let read = |i: usize| -> f32 {
        let b = &raw[i * bytes..i * bytes + bytes];
        match sample {
            Sample::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Sample::I16 => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
            Sample::I24 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            Sample::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        }
    };
    let frames = raw.len() / (channels * bytes).max(1);
    let mut out = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        let base = f * channels;
        let (l, r) = match channels {
            1 => {
                let v = read(base);
                (v, v)
            }
            2 => (read(base), read(base + 1)),
            _ => {
                let c = read(base + 2) * 0.707;
                (read(base) + c, read(base + 1) + c)
            }
        };
        out.push(l);
        out.push(r);
    }
    out
}

/// 立体声 f32 → 48k 立体声 i16（线性插值；跨包接着插）
pub struct Resampler {
    step: f64,
    pos: f64,
    last: [f32; 2],
    primed: bool,
}

impl Resampler {
    pub fn new(rate: u32) -> Self {
        Self {
            step: f64::from(rate) / f64::from(RATE),
            pos: 0.0,
            last: [0.0; 2],
            primed: false,
        }
    }

    pub fn reset(&mut self) {
        self.pos = 0.0;
        self.primed = false;
    }

    pub fn run(&mut self, input: &[f32]) -> Vec<i16> {
        let to_i16 = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        let n = input.len() / 2;
        if (self.step - 1.0).abs() < 1e-9 {
            return input.iter().map(|&v| to_i16(v)).collect();
        }
        if n == 0 {
            return Vec::new();
        }
        if !self.primed {
            self.last = [input[0], input[1]];
            self.primed = true;
        }
        // 位置 -1 是上一包的最后一个样本，0..n 是这一包
        let at = |i: isize, ch: usize| -> f32 {
            if i < 0 {
                self.last[ch]
            } else {
                input[i as usize * 2 + ch]
            }
        };
        let mut out = Vec::with_capacity((n as f64 / self.step) as usize * 2 + 4);
        while self.pos < n as f64 - 1.0 + 1.0 {
            let i = self.pos.floor() as isize - 1;
            let frac = (self.pos - self.pos.floor()) as f32;
            if i + 1 >= n as isize {
                break;
            }
            for ch in 0..2 {
                let a = at(i, ch);
                let b = at(i + 1, ch);
                out.push(to_i16(a + (b - a) * frac));
            }
            self.pos += self.step;
        }
        self.pos -= n as f64;
        self.last = [input[(n - 1) * 2], input[(n - 1) * 2 + 1]];
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_length_and_level() {
        // 44.1k 的一秒正弦 → 48k 大约一秒
        let mut r = Resampler::new(44_100);
        let input: Vec<f32> = (0..44_100)
            .flat_map(|i| {
                let v = (i as f32 * 0.05).sin() * 0.5;
                [v, v]
            })
            .collect();
        let mut out = Vec::new();
        for chunk in input.chunks(882) {
            out.extend(r.run(chunk));
        }
        let frames = out.len() / 2;
        assert!((47_900..=48_100).contains(&frames), "{frames}");
        let peak = out.iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert!((15_000..=17_000).contains(&peak), "{peak}");
    }

    #[test]
    fn mono_is_duplicated_and_surround_folds_center() {
        let mono: Vec<u8> = [0.25f32, -0.5]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(
            to_stereo(&mono, 1, Sample::F32, 4),
            vec![0.25, 0.25, -0.5, -0.5]
        );
        let five: Vec<u8> = [0.1f32, 0.2, 0.5, 0.0, 0.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let s = to_stereo(&five, 5, Sample::F32, 4);
        assert!((s[0] - (0.1 + 0.3535)).abs() < 1e-3 && (s[1] - (0.2 + 0.3535)).abs() < 1e-3);
    }
}
