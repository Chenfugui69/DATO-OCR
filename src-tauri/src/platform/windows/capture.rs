//! 抓屏：GDI 从屏幕 DC 拷（见 `grab`）。xcap（Apache-2.0）只用来枚举显示器，录屏用它的 WGC 帧池。
//!
//! xcap 的 `x/y/width/height` 取自 `DEVMODEW`，是**物理像素**，正是我们要的坐标系。
//! 这些访问器每次都会走一遍 `EnumDisplaySettingsW`，所以只枚举一次就组装好 `MonitorInfo`。
//! 工作区（排除任务栏）xcap 不提供，用 `GetMonitorInfoW` 补。

use image::RgbaImage;
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC,
    GetMonitorInfoW, MonitorFromPoint, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, MONITORINFO, MONITOR_DEFAULTTONULL, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowDisplayAffinity, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    WINDOW_DISPLAY_AFFINITY,
};
use xcap::Monitor;

use super::util::hwnd;
use crate::error::{AppError, AppResult};
use crate::platform::{MonitorId, MonitorInfo, PhysicalRect, WindowHandle};

/// 屏幕上这一块现在的画面（划词按钮的玻璃珠透过去看到的那块）。
///
/// 只有几十个像素见方，用 GDI 直接从屏幕 DC 拷这一块，不到 1ms；WGC 只能整屏抓，4K 一次 60ms 太重。
/// `below`：上一颗按钮还摆在这儿的话，先把它排除出抓屏（WDA_EXCLUDEFROMCAPTURE 对 GDI 拷屏也生效），
/// 拷完恢复原来的设置，免得旧珠子连同投影被拍进新珠子里。
pub fn patch(rect: PhysicalRect, below: Option<WindowHandle>) -> Option<RgbaImage> {
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let restore = below.and_then(|w| {
        let h = hwnd(w.0);
        let mut old = 0u32;
        // SAFETY: h 是本进程的按钮窗口；只读、改它的显示亲和性。
        unsafe {
            GetWindowDisplayAffinity(h, &mut old).ok()?;
            if old == WDA_EXCLUDEFROMCAPTURE.0 {
                return None;
            }
            SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE).ok()?;
        }
        // 亲和性在 DWM 合成下一帧才生效
        std::thread::sleep(std::time::Duration::from_millis(20));
        Some((h, WINDOW_DISPLAY_AFFINITY(old)))
    });
    let image = grab(rect);
    if let Some((h, old)) = restore {
        // SAFETY: 同上。
        let _ = unsafe { SetWindowDisplayAffinity(h, old) };
    }
    match image {
        Ok(image) => Some(image),
        Err(err) => {
            tracing::debug!(?rect, "抓按钮底下的画面失败：{err}");
            None
        }
    }
}

/// 从屏幕 DC 拷出一块（虚拟桌面坐标，物理像素），转成不透明 RGBA。
///
/// 整屏截图也走这里，不用 WGC：WGC 要一个 D3D 设备，xcap 把它放在进程级的静态变量里，
/// 第一次抓屏之后主进程就一直多占 80～170MB（显卡驱动的用户态内存），待机也不释放。
/// GDI 抓 4K 一屏 30ms 左右，比 WGC 慢十来毫秒，画面一样。
fn grab(rect: PhysicalRect) -> windows::core::Result<RgbaImage> {
    let (w, h) = (rect.width as i32, rect.height as i32);
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: 所有 GDI 对象都在本函数内创建并按相反顺序释放；DIB 是 32 位自上而下，
    // `bits` 指向 w*h*4 字节，在 DeleteObject 之前读完。
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let mut bits = std::ptr::null_mut();
        let dib = match CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(dib) if !bits.is_null() => dib,
            other => {
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                return Err(other.err().unwrap_or_else(windows::core::Error::empty));
            }
        };
        let old = SelectObject(mem, dib.into());
        // 直接拷进 DIB，少一次 GetDIBits 的整屏拷贝。CAPTUREBLT：分层窗口（半透明的浮窗、菜单）也拷进来
        let copied = BitBlt(mem, 0, 0, w, h, Some(screen), rect.x, rect.y, SRCCOPY | CAPTUREBLT);
        let _ = GdiFlush();
        let image = copied.map(|()| {
            let src = std::slice::from_raw_parts(bits.cast::<u8>(), (w * h * 4) as usize);
            // BGRA → RGBA，alpha 没有语义，补成不透明
            let mut buf = vec![0u8; src.len()];
            crate::imaging::swap_rb_opaque(src, &mut buf);
            buf
        });
        SelectObject(mem, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        RgbaImage::from_raw(rect.width, rect.height, image?).ok_or_else(windows::core::Error::empty)
    }
}

pub fn list_monitors() -> AppResult<Vec<MonitorInfo>> {
    Ok(enumerate()?.into_iter().map(|(_, info)| info).collect())
}

pub fn capture_all() -> AppResult<Vec<(MonitorInfo, RgbaImage)>> {
    let monitors = enumerate()?;
    let mut shots = Vec::with_capacity(monitors.len());
    for (_, info) in monitors {
        let image = capture_one(&info)?;
        shots.push((info, image));
    }
    Ok(shots)
}

pub fn capture_monitor(id: MonitorId) -> AppResult<RgbaImage> {
    let (_, info) = enumerate()?
        .into_iter()
        .find(|(_, info)| info.id == id)
        .ok_or_else(|| AppError::Capture(format!("显示器 {id} 已不存在")))?;
    capture_one(&info)
}

pub fn warm_up() {
    let started = std::time::Instant::now();
    // 第一次整屏 BitBlt 要多十几毫秒（DWM 那边准备回读），抓一块屏就够；抓完的图随即释放
    let result = enumerate().and_then(|monitors| {
        let (_, info) = monitors
            .first()
            .ok_or_else(|| AppError::Capture("没有显示器".into()))?;
        capture_one(info).map(|img| img.width())
    });
    match result {
        Ok(_) => tracing::debug!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "抓屏预热完成"
        ),
        Err(err) => tracing::warn!("抓屏预热失败，首次截图会慢一些：{err}"),
    }
}

/// 主屏排第一，其余按 x 再按 y。前端的显示顺序依赖这个稳定次序。
fn enumerate() -> AppResult<Vec<(Monitor, MonitorInfo)>> {
    let mut monitors = Vec::new();
    for monitor in Monitor::all()? {
        match describe(&monitor) {
            Ok(info) => monitors.push((monitor, info)),
            // 枚举过程中被拔掉的外接屏之类，跳过而不是整体失败
            Err(err) => tracing::warn!("跳过一块无法读取的显示器：{err}"),
        }
    }
    if monitors.is_empty() {
        return Err(AppError::Capture("没有可用的显示器".into()));
    }
    monitors.sort_by(|(_, a), (_, b)| {
        b.is_primary
            .cmp(&a.is_primary)
            .then(a.bounds.x.cmp(&b.bounds.x))
            .then(a.bounds.y.cmp(&b.bounds.y))
    });
    Ok(monitors)
}

/// 连续录屏（GIF 用）：xcap 的 WGC 帧池录制，画面有变化才来新帧，不用每帧新建一次抓屏会话
/// （单次抓屏 4K 要 60～70ms，连续录制只要拷贝一帧的时间）。drop 时停止。
pub struct ScreenRecorder {
    recorder: xcap::VideoRecorder,
    rx: std::sync::mpsc::Receiver<xcap::Frame>,
    region: PhysicalRect,
}

impl ScreenRecorder {
    /// `region`：这块屏里要录的那一块（屏内物理像素）。
    pub fn start(id: MonitorId, region: PhysicalRect) -> AppResult<Self> {
        let (monitor, _) = enumerate()?
            .into_iter()
            .find(|(_, info)| info.id == id)
            .ok_or_else(|| AppError::Capture(format!("显示器 {id} 已不存在")))?;
        let (recorder, rx) = monitor.video_recorder()?;
        recorder.start()?;
        Ok(Self {
            recorder,
            rx,
            region,
        })
    }

    /// 取一帧新画面（只有 `region` 那一块，不透明 RGBA）。画面没变化时等到超时返回 None。
    pub fn next(&self, timeout: std::time::Duration) -> Option<RgbaImage> {
        let frame = self.rx.recv_timeout(timeout).ok()?;
        // WGC 给的是整块屏，alpha 没有语义；裁出区域时顺手补成不透明
        let full = RgbaImage::from_raw(frame.width, frame.height, frame.raw)?;
        match crate::imaging::crop_opaque(&full, self.region) {
            Ok(image) => Some(image),
            Err(err) => {
                tracing::debug!("录屏裁帧失败：{err}");
                None
            }
        }
    }
}

impl Drop for ScreenRecorder {
    fn drop(&mut self) {
        if let Err(err) = self.recorder.stop() {
            tracing::debug!("停止录屏失败：{err}");
        }
    }
}

/// 注意：WGC 给回来的 alpha 没有语义（常年是 0）。热路径上不逐像素修它 —— 底图层和
/// BMP 编码都忽略 alpha；真正要输出的裁剪结果由 `imaging::crop_opaque` 统一补成不透明。
fn capture_one(info: &MonitorInfo) -> AppResult<RgbaImage> {
    grab(info.bounds).map_err(|err| AppError::Capture(format!("抓屏失败：{err}")))
}

fn describe(monitor: &Monitor) -> AppResult<MonitorInfo> {
    let bounds = PhysicalRect::new(
        monitor.x()?,
        monitor.y()?,
        monitor.width()?,
        monitor.height()?,
    );
    Ok(MonitorInfo {
        id: MonitorId(u64::from(monitor.id()?)),
        name: monitor.name().unwrap_or_else(|_| "显示器".to_owned()),
        work_area: work_area(&bounds).unwrap_or(bounds),
        bounds,
        scale_factor: f64::from(monitor.scale_factor()?),
        is_primary: monitor.is_primary()?,
    })
}

fn work_area(bounds: &PhysicalRect) -> Option<PhysicalRect> {
    let probe = POINT {
        x: bounds.x + 1,
        y: bounds.y + 1,
    };
    // SAFETY: 纯查询，结构体按文档初始化了 cbSize。
    unsafe {
        let hmon = MonitorFromPoint(probe, MONITOR_DEFAULTTONULL);
        if hmon.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(hmon, &mut info).as_bool() {
            return None;
        }
        let r = info.rcWork;
        Some(PhysicalRect::new(
            r.left,
            r.top,
            (r.right - r.left).max(0) as u32,
            (r.bottom - r.top).max(0) as u32,
        ))
    }
}

#[cfg(test)]
mod tests {
    /// 真机烟雾测试：需要物理显示器，默认忽略。
    /// `cargo test -- --ignored --nocapture` 打印各屏几何、缩放与抓屏耗时。
    /// 把抓到的画面存成 PNG 人眼看：`CHENOCR_SMOKE_OUT=路径 cargo test -- --ignored smoke_save`
    #[test]
    #[ignore]
    fn smoke_save() {
        crate::platform::init_process();
        let out = std::env::var("CHENOCR_SMOKE_OUT").unwrap();
        let shots = super::capture_all().unwrap();
        let (_, img) = &shots[0];
        let small = image::imageops::thumbnail(img, img.width() / 4, img.height() / 4);
        small.save(&out).unwrap();
        println!("saved {out}");
    }


    #[test]
    #[ignore]
    fn smoke_capture_all() {
        crate::platform::init_process();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let shots = super::capture_all().unwrap();
            for (info, img) in &shots {
                println!(
                    "{} {:?} scale={} img={}x{} work={:?}",
                    info.name,
                    info.bounds,
                    info.scale_factor,
                    img.width(),
                    img.height(),
                    info.work_area
                );
            }
            println!("capture_all {}ms", started.elapsed().as_millis());
        }
    }
}
