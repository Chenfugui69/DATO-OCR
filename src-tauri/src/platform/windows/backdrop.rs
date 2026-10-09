//! 冻结底图的原生显示层。
//!
//! 每块屏一个 `WS_POPUP` 分层窗口，内容用 `UpdateLayeredWindow` 直接交给 DWM，
//! 不经过 Chromium（4K 传输解码 170ms + 色彩管理会改像素值，两条都不能接受）。
//!
//! 为什么是分层窗口：普通窗口要等 `ShowWindow` 后的 `WM_PAINT` 才出画面，之前 DWM
//! 拿到的是未初始化表面，有概率闪一帧垃圾。`UpdateLayeredWindow` 可以在窗口**隐藏
//! 时**就交付整幅内容，show 出来的第一帧就是最终画面。
//!
//! | 样式 | 为什么 |
//! |---|---|
//! | `WS_EX_LAYERED` | `UpdateLayeredWindow` 的前提 |
//! | `WS_EX_TRANSPARENT` | 鼠标穿透，绝不能吃掉本该给遮罩的点击 |
//! | `WS_EX_NOACTIVATE` | 永不获得焦点，否则抢走遮罩的键盘焦点 |
//! | `WS_EX_TOOLWINDOW` | 不进任务栏 / Alt+Tab |
//! | `WS_EX_TOPMOST` | 必须和遮罩同在 topmost 波段：否则常驻置顶窗口（音量 OSD、画中画、
//! |                 | 录屏工具条…）会夹在"遮罩之下、底图之上"，从镂空的选区里活着浮出来 |
//!
//! 窗口和 GDI 对象归创建线程所有，所以状态放 `thread_local`，**只能在 UI 线程调用**。
//! 从别的线程调用会看到一张空表；`check_thread` 让这种误用在日志里炸出来。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use image::RgbaImage;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, CreateWindowExW, DefWindowProcW, DeferWindowPos, DestroyWindow,
    EndDeferWindowPos, RegisterClassExW, SetWindowDisplayAffinity, ShowWindow, UpdateLayeredWindow,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, ULW_OPAQUE,
    WDA_EXCLUDEFROMCAPTURE, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::util::hwnd as to_hwnd;
use crate::error::{AppError, AppResult};
use crate::platform::{MonitorId, PhysicalRect};

const CLASS_NAME: PCWSTR = w!("ChenocrBackdrop");

struct Surface {
    hwnd: HWND,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    /// DIB 像素内存（GDI 分配，与 bitmap 同生命周期），自上而下 32bpp BGRA
    bits: *mut u8,
    width: u32,
    height: u32,
}

impl Surface {
    fn empty(hwnd: HWND) -> Self {
        Self {
            hwnd,
            dc: HDC::default(),
            bitmap: HBITMAP::default(),
            previous: HGDIOBJ::default(),
            bits: std::ptr::null_mut(),
            width: 0,
            height: 0,
        }
    }

    fn free_dib(&mut self) {
        // SAFETY: 句柄由本模块创建；默认值（从未分配）时这些调用是无害的 no-op。
        unsafe {
            if !self.dc.is_invalid() {
                SelectObject(self.dc, self.previous);
                let _ = DeleteDC(self.dc);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
        }
        self.dc = HDC::default();
        self.bitmap = HBITMAP::default();
        self.previous = HGDIOBJ::default();
        self.bits = std::ptr::null_mut();
        self.width = 0;
        self.height = 0;
    }

    fn destroy(mut self) {
        self.free_dib();
        // SAFETY: 窗口由本模块创建，按值消费保证不会再被用到。
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

thread_local! {
    static SURFACES: RefCell<HashMap<u64, Surface>> = RefCell::new(HashMap::new());
}

static OWNER_THREAD: OnceLock<u32> = OnceLock::new();

fn check_thread(what: &str) {
    // SAFETY: 无参数。
    let current = unsafe { GetCurrentThreadId() };
    let owner = *OWNER_THREAD.get_or_init(|| current);
    if current != owner {
        tracing::error!(
            what,
            current,
            owner,
            "底图层被非 UI 线程调用，这次调用不会生效"
        );
    }
}

pub fn ensure(monitor: MonitorId) -> AppResult<()> {
    check_thread("ensure");
    SURFACES.with_borrow_mut(|surfaces| {
        if let std::collections::hash_map::Entry::Vacant(slot) = surfaces.entry(monitor.0) {
            slot.insert(Surface::empty(create_window()?));
        }
        Ok(())
    })
}

pub fn load(monitor: MonitorId, image: &RgbaImage, at: PhysicalRect) -> AppResult<()> {
    check_thread("load");
    ensure(monitor)?;
    SURFACES.with_borrow_mut(|surfaces| {
        let surface = surfaces
            .get_mut(&monitor.0)
            .ok_or_else(|| AppError::msg(format!("底图窗口 {monitor} 不存在")))?;
        if surface.width != image.width()
            || surface.height != image.height()
            || surface.bits.is_null()
        {
            rebuild_dib(surface, image.width(), image.height())?;
        }
        // SAFETY: 上面刚保证 DIB 尺寸与 image 一致，缓冲有效。
        let dst = unsafe {
            std::slice::from_raw_parts_mut(
                surface.bits,
                surface.width as usize * surface.height as usize * 4,
            )
        };
        for (out, inp) in dst.chunks_exact_mut(4).zip(image.as_raw().chunks_exact(4)) {
            out[0] = inp[2];
            out[1] = inp[1];
            out[2] = inp[0];
            out[3] = 255;
        }
        push_to_dwm(surface, at)
    })
}

pub fn show_below(monitor: MonitorId, overlay: u64) -> AppResult<()> {
    check_thread("show_below");
    let backdrop = SURFACES.with_borrow(|s| s.get(&monitor.0).map(|s| s.hwnd));
    let Some(backdrop) = backdrop else {
        return Err(AppError::msg(format!("底图窗口 {monitor} 不存在")));
    };
    let overlay = to_hwnd(overlay);

    // 同一个 DeferWindowPos 批次在同一帧生效。分两次 ShowWindow 中间会有一帧
    // 只有底图没有压暗层 —— 一道很显眼的闪光。
    // SAFETY: 两个句柄都指向存活窗口且归当前线程所有。
    unsafe {
        let batch = BeginDeferWindowPos(2)?;
        // 先摆遮罩，它是 topmost 的锚
        let batch = DeferWindowPos(
            batch,
            overlay,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )?;
        // hWndInsertAfter = 遮罩：底图排在遮罩"后面"，即 z 序更低
        let batch = DeferWindowPos(
            batch,
            backdrop,
            Some(overlay),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )?;
        EndDeferWindowPos(batch)?;
    }
    // 底图就是冻结的那张屏幕画面：远程控制的对面要看到它，不然选区里是空的
    super::effects::set_hwnd_capturable(super::util::handle_of(backdrop), true);
    Ok(())
}

pub fn hide_all() {
    check_thread("hide_all");
    SURFACES.with_borrow(|surfaces| {
        for surface in surfaces.values() {
            // 先排除再藏：藏起来的窗口在下一帧合成之前还可能被抓到
            super::effects::set_hwnd_capturable(super::util::handle_of(surface.hwnd), false);
            // SAFETY: 句柄由本模块持有。
            let _ = unsafe { ShowWindow(surface.hwnd, SW_HIDE) };
        }
    });
}

/// 隐藏并释放像素缓冲（每块 4K 屏 33MB），窗口本身保留复用。
pub fn release_all() {
    check_thread("release_all");
    SURFACES.with_borrow_mut(|surfaces| {
        for surface in surfaces.values_mut() {
            super::effects::set_hwnd_capturable(super::util::handle_of(surface.hwnd), false);
            // SAFETY: 句柄由本模块持有。
            let _ = unsafe { ShowWindow(surface.hwnd, SW_HIDE) };
            surface.free_dib();
        }
    });
}

pub fn retain(keep: &[MonitorId]) {
    check_thread("retain");
    SURFACES.with_borrow_mut(|surfaces| {
        let doomed: Vec<u64> = surfaces
            .keys()
            .copied()
            .filter(|id| !keep.iter().any(|m| m.0 == *id))
            .collect();
        for id in doomed {
            if let Some(surface) = surfaces.remove(&id) {
                surface.destroy();
            }
        }
    });
}

fn create_window() -> AppResult<HWND> {
    register_class()?;
    // SAFETY: 类已注册，参数都是常量。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS_NAME,
            w!("DATO OCR Backdrop"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
    }?;
    // SAFETY: hwnd 刚创建。Win10 2004 以下不支持，静默降级。
    let _ = unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) };
    Ok(hwnd)
}

fn register_class() -> AppResult<()> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            // SAFETY: 取本进程模块句柄；结构体完整初始化。
            unsafe {
                let instance: HINSTANCE = GetModuleHandleW(None)
                    .map_err(|e| format!("取模块句柄失败：{e}"))?
                    .into();
                let class = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: instance,
                    lpszClassName: CLASS_NAME,
                    ..Default::default()
                };
                if RegisterClassExW(&class) == 0 {
                    return Err(format!(
                        "注册窗口类失败：{}",
                        windows::core::Error::from_thread()
                    ));
                }
            }
            Ok(())
        })
        .clone()
        .map_err(AppError::Msg)
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: 原样转发给系统默认处理。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn rebuild_dib(surface: &mut Surface, width: u32, height: u32) -> AppResult<()> {
    surface.free_dib();
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32), // 负高度 = 自上而下，和 RgbaImage 行序一致
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: info 描述一张合法 32bpp DIB；bits 接收与 HBITMAP 同生命周期的像素指针。
    unsafe {
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        if bits.is_null() {
            let _ = DeleteObject(bitmap.into());
            return Err(AppError::msg("底图 DIB 没有返回像素指针"));
        }
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            let _ = DeleteObject(bitmap.into());
            return Err(AppError::msg("创建底图内存 DC 失败"));
        }
        surface.previous = SelectObject(dc, bitmap.into());
        surface.dc = dc;
        surface.bitmap = bitmap;
    }
    surface.bits = bits.cast();
    surface.width = width;
    surface.height = height;
    Ok(())
}

fn push_to_dwm(surface: &Surface, at: PhysicalRect) -> AppResult<()> {
    let position = POINT { x: at.x, y: at.y };
    let size = SIZE {
        cx: surface.width as i32,
        cy: surface.height as i32,
    };
    let source = POINT { x: 0, y: 0 };
    // SAFETY: hwnd 是本模块创建的分层窗口，dc 里选中的 DIB 尺寸就是 size。
    unsafe {
        UpdateLayeredWindow(
            surface.hwnd,
            None,
            Some(&position),
            Some(&size),
            Some(surface.dc),
            Some(&source),
            COLORREF(0),
            None,
            ULW_OPAQUE,
        )?;
    }
    Ok(())
}
