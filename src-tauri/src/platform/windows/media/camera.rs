//! 摄像头画面（录视频时叠在角落）：Media Foundation 的采集源 + Source Reader，在自己的线程上一帧帧读，
//! 中间裁成正方形、缩到要的大小、左右镜像（像照镜子，和视频会议里看自己一样），留着最新的一帧。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use image::RgbaImage;
use parking_lot::Mutex;
use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

use super::{init, mf_err, pack, unpack, Mt};
use crate::error::{AppError, AppResult};

pub struct Camera {
    latest: Arc<Mutex<Option<RgbaImage>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Camera {
    pub fn start(size: u32) -> AppResult<Self> {
        let size = size.clamp(16, 1024);
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (frame, flag) = (latest.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("camera".into())
            .spawn(move || {
                let (source, reader) = match open(size) {
                    Ok(v) => {
                        let _ = ready_tx.send(Ok(()));
                        v
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                while !flag.load(Ordering::Acquire) {
                    let (mut flags, mut sample) = (0u32, None::<IMFSample>);
                    // SAFETY: 同步读一帧（摄像头按自己的帧率给）
                    let read = unsafe {
                        reader.0.ReadSample(
                            MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                            0,
                            None,
                            Some(&mut flags),
                            None,
                            Some(&mut sample),
                        )
                    };
                    if read.is_err()
                        || flags
                            & (MF_SOURCE_READERF_ERROR.0 | MF_SOURCE_READERF_ENDOFSTREAM.0) as u32
                            != 0
                    {
                        tracing::warn!("摄像头断开了");
                        break;
                    }
                    let Some(sample) = sample else { continue };
                    // SAFETY: 读当前的输出格式
                    let dims = unsafe {
                        reader
                            .0
                            .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                    }
                    .and_then(|t| unsafe { t.GetUINT64(&MF_MT_FRAME_SIZE) })
                    .map(unpack);
                    if let Ok((w, h)) = dims {
                        if let Some(img) = square_frame(&sample, w, h, size) {
                            *frame.lock() = Some(img);
                        }
                    }
                }
                // SAFETY: 不再读了，关掉摄像头（灯灭）
                unsafe {
                    let _ = source.0.Shutdown();
                }
            })?;
        match ready_rx.recv_timeout(Duration::from_secs(8)) {
            Ok(Ok(())) => Ok(Self {
                latest,
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => Err(AppError::Msg(e)),
            Err(_) => Err(AppError::msg("摄像头用不了")),
        }
    }

    pub fn latest(&self) -> Option<RgbaImage> {
        self.latest.lock().clone()
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 打开第一个摄像头：挑一个够大（短边不小于要的大小）又不太大的格式，输出 BGRA
fn open(size: u32) -> AppResult<(Mt<IMFMediaSource>, Mt<IMFSourceReader>)> {
    init()?;
    // SAFETY: 标准的枚举 / 激活流程；设备数组用 CoTaskMemFree 释放，里面的对象随数组一起 Release
    unsafe {
        let mut a = None;
        MFCreateAttributes(&mut a, 1).map_err(|e| mf_err("打不开摄像头", e))?;
        let a = a.ok_or_else(|| AppError::msg("打不开摄像头"))?;
        a.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )
        .map_err(|e| mf_err("打不开摄像头", e))?;
        let (mut list, mut count) = (std::ptr::null_mut(), 0u32);
        MFEnumDeviceSources(&a, &mut list, &mut count).map_err(|e| mf_err("打不开摄像头", e))?;
        if count == 0 || list.is_null() {
            if !list.is_null() {
                CoTaskMemFree(Some(list.cast()));
            }
            return Err(AppError::msg("找不到摄像头"));
        }
        let devices = std::slice::from_raw_parts_mut(list, count as usize);
        let first = devices[0].take();
        for d in devices.iter_mut() {
            drop(d.take());
        }
        CoTaskMemFree(Some(list.cast()));
        let activate = first.ok_or_else(|| AppError::msg("找不到摄像头"))?;
        let source: IMFMediaSource = activate.ActivateObject().map_err(|e| {
            // 被别的程序占着、隐私设置里关了都会在这里失败
            if super::media_access(crate::platform::types::MediaKind::Camera)
                == crate::platform::types::MediaAccess::Denied
            {
                AppError::msg("没有摄像头权限")
            } else {
                mf_err("打不开摄像头", e)
            }
        })?;
        let ra = {
            let mut r = None;
            MFCreateAttributes(&mut r, 1).map_err(|e| mf_err("打不开摄像头", e))?;
            r.ok_or_else(|| AppError::msg("打不开摄像头"))?
        };
        // 让 Source Reader 自己把摄像头的格式（MJPG、YUY2、NV12……）转成 BGRA
        ra.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
            .map_err(|e| mf_err("打不开摄像头", e))?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, &ra)
            .map_err(|e| mf_err("打不开摄像头", e))?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        // 挑格式：短边不小于要的，面积最小的；帧率太低（< 15）的不要
        let mut best: Option<(u64, u32, u32)> = None;
        for i in 0..200u32 {
            let Ok(t) = reader.GetNativeMediaType(stream, i) else {
                break;
            };
            let Ok((w, h)) = t.GetUINT64(&MF_MT_FRAME_SIZE).map(unpack) else {
                continue;
            };
            let fps = t
                .GetUINT64(&MF_MT_FRAME_RATE)
                .map(unpack)
                .map_or(30.0, |(n, d)| f64::from(n) / f64::from(d.max(1)));
            if w.min(h) < size.min(480) || fps < 15.0 {
                continue;
            }
            let area = u64::from(w) * u64::from(h);
            if best.is_none_or(|b| area < b.0) {
                best = Some((area, w, h));
            }
        }
        let out = MFCreateMediaType().map_err(|e| mf_err("打不开摄像头", e))?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| mf_err("打不开摄像头", e))?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(|e| mf_err("打不开摄像头", e))?;
        if let Some((_, w, h)) = best {
            let _ = out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h));
        }
        if reader.SetCurrentMediaType(stream, None, &out).is_err() {
            // 指定的大小转不了：让它用默认的
            let _ = out.DeleteItem(&MF_MT_FRAME_SIZE);
            reader
                .SetCurrentMediaType(stream, None, &out)
                .map_err(|e| mf_err("打不开摄像头", e))?;
        }
        Ok((Mt(source), Mt(reader)))
    }
}

/// 一帧 BGRA → 中间裁正方形、缩到 size × size、镜像的 RGBA
fn square_frame(sample: &IMFSample, w: u32, h: u32, size: u32) -> Option<RgbaImage> {
    let side = w.min(h);
    if side == 0 {
        return None;
    }
    let (x0, y0) = ((w - side) / 2, (h - side) / 2);
    let mut square = RgbaImage::new(side, side);
    // SAFETY: 锁住缓冲区读，行宽按它自己报的（负的是从下往上存）
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer().ok()?;
        let (base, pitch, b2) = match buffer.cast::<IMF2DBuffer>() {
            Ok(b2) => {
                let (mut p, mut pitch) = (std::ptr::null_mut(), 0i32);
                b2.Lock2D(&mut p, &mut pitch).ok()?;
                (p, pitch as isize, Some(b2))
            }
            Err(_) => {
                let mut p = std::ptr::null_mut();
                buffer.Lock(&mut p, None, None).ok()?;
                (p, (w * 4) as isize, None)
            }
        };
        for y in 0..side {
            let row = base.offset((y + y0) as isize * pitch).add(x0 as usize * 4);
            for x in 0..side {
                let p = row.add(x as usize * 4);
                // 镜像：右边的像素放到左边
                square.put_pixel(
                    side - 1 - x,
                    y,
                    image::Rgba([*p.add(2), *p.add(1), *p, 255]),
                );
            }
        }
        match b2 {
            Some(b2) => {
                let _ = b2.Unlock2D();
            }
            None => {
                let _ = buffer.Unlock();
            }
        }
    }
    Some(if side == size {
        square
    } else {
        image::imageops::resize(&square, size, size, image::imageops::FilterType::Triangle)
    })
}

#[cfg(test)]
mod tests {
    /// 真机：开摄像头 2 秒，存一帧：`CHENOCR_SMOKE_OUT=目录 cargo test --lib -- --ignored smoke_camera --nocapture`
    #[test]
    #[ignore]
    fn smoke_camera() {
        let dir = std::path::PathBuf::from(std::env::var("CHENOCR_SMOKE_OUT").unwrap());
        let cam = super::Camera::start(240).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let frame = cam.latest().expect("没有画面");
        frame.save(dir.join("camera.png")).unwrap();
        println!("ok {}x{}", frame.width(), frame.height());
    }
}
