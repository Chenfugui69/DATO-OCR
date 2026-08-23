//! 冻结底图的原生显示层（Windows）。
//!
//! 每块屏一个 `WS_POPUP` 顶层窗口，内容用 `UpdateLayeredWindow` 直接推给 DWM。
//!
//! # 为什么是分层窗口而不是普通窗口 + WM_PAINT
//!
//! 普通窗口在 `ShowWindow` 之后才收到 `WM_PAINT`，在那之前 DWM 拿到的是未初始化
//! 的重定向表面 —— 也就是有概率闪一帧垃圾内容。`UpdateLayeredWindow` 可以在窗口
//! **还隐藏着**的时候就把整幅内容交给 DWM，之后 show 出来的第一帧就是最终画面。
//! 这是防闪烁启动画面的老办法，正好也是我们要的：底图绝不能先亮一帧未压暗的原图。
//!
//! 附带好处是这些窗口不需要消息泵来出画面。但窗口的所有权仍然属于创建它的线程，
//! 所以本模块的所有函数**必须在 UI 线程调用**（见 [`surfaces`]）。
//!
//! # 窗口样式
//!
//! | 样式 | 为什么 |
//! |---|---|
//! | `WS_EX_LAYERED` | `UpdateLayeredWindow` 的前提 |
//! | `WS_EX_TRANSPARENT` | 鼠标穿透。底图窗口绝不能吃掉本该给遮罩的点击 |
//! | `WS_EX_NOACTIVATE` | 永不获得焦点，否则会把遮罩的键盘焦点抢走 |
//! | `WS_EX_TOOLWINDOW` | 不进任务栏、不进 Alt+Tab |
//! | `WS_EX_TOPMOST` | **见下**，少了它会有窗口从选区里钻出来 |
//! | `WDA_EXCLUDEFROMCAPTURE` | 别把自己拍进别人的抓屏结果（规格 07 §4.4） |
//!
//! # 为什么显式加 `WS_EX_TOPMOST`
//!
//! z 序分两个波段：topmost 的一律在非 topmost 的之上。底图**必须**和遮罩同在
//! topmost 波段，否则系统里任何常驻置顶窗口（音量 OSD、画中画、Teams 悬浮条、
//! PowerToys 置顶窗、录屏工具条、输入法候选窗、任务管理器的"置于顶层"）都会卡在
//! "遮罩之下、底图之上"的夹层里。遮罩的选区是真镂空的，那个窗口就会**活着浮在
//! 冻结画面上** —— 既破坏"画面是冻结的"这个前提，也会被一起截进去。
//!
//! 实测结论（`scripts/probe-zorder.ps1`，可用 `CHENOCR_BACKDROP_NO_TOPMOST=1` 复现）：
//! 即便创建时**不**加这个样式，运行时读回来的 `GWL_EXSTYLE` 仍然是 `0x080800A8`
//! —— 含 `WS_EX_TOPMOST`。因为 `DeferWindowPos(hWndInsertAfter = 遮罩)` 把它插到了
//! 一个 topmost 窗口之后，`SetWindowPos` 的文档明确说这会让被摆的窗口自己也变成
//! topmost。同一个脚本还能看到底图正上方紧贴着的就是遮罩那个 `Tauri Window`。
//!
//! 所以这条样式在**当前**的调用方式下是冗余的。留着它是因为波段归属是这个模块的
//! 硬需求，不该靠"插入锚点恰好是个 topmost 窗口"这种副作用来保证 —— 哪天
//! [`BackdropLayer::show_above`] 改成用 `HWND_TOP` 或别的锚点，冗余就变成缺陷，
//! 而且症状是间歇性的（要恰好有置顶窗口在场），极难查。
//!
//! 注意它**替代不了**显式插入：同一波段内的先后仍然由 `DeferWindowPos` 决定。
//! 真正能消掉那条调用约定的是让底图成为遮罩的 owner（对顶层窗口设
//! `GWLP_HWNDPARENT`），OS 保证 owned 窗口恒在 owner 之上；但改 Tauri/WebView2
//! 窗口的 owner 有没有副作用尚未验证，没验之前不动。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use image::RgbaImage;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, CreateWindowExW, DefWindowProcW, DeferWindowPos, DestroyWindow,
    EndDeferWindowPos, RegisterClassExW, SetWindowDisplayAffinity, ShowWindow, UpdateLayeredWindow,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE,
    WDA_EXCLUDEFROMCAPTURE, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::Win32::UI::WindowsAndMessaging::ULW_OPAQUE;

use crate::error::{AppError, AppResult};
use crate::platform::{BackdropLayer, MonitorId, PhysicalRect};

const CLASS_NAME: PCWSTR = w!("ChenocrBackdrop");

pub struct WindowsBackdropLayer;

/// 一块屏的底图窗口及其像素缓冲。
struct Surface {
    hwnd: HWND,
    /// 内存 DC，`bitmap` 已经选进去了。`UpdateLayeredWindow` 要的源就是它。
    dc: HDC,
    bitmap: HBITMAP,
    /// `SelectObject` 换出来的旧对象。销毁前要选回去，否则 `DeleteObject`
    /// 删不掉还在被 DC 选中的位图。
    previous: HGDIOBJ,
    /// DIB 的像素内存，由 GDI 分配、和 `bitmap` 同生命周期。自上而下 32bpp BGRA。
    bits: *mut u8,
    width: u32,
    height: u32,
}

impl Surface {
    fn byte_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    /// SAFETY: 调用方保证 `self.bits` 仍然有效（即本 `Surface` 尚未 destroy）。
    unsafe fn pixels(&mut self) -> &mut [u8] {
        std::slice::from_raw_parts_mut(self.bits, self.byte_len())
    }

    fn destroy(self) {
        // SAFETY: 这些句柄都由本模块创建且只在这里销毁；`self` 按值传入，
        // 保证不会有人再拿到它。
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteDC(self.dc);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

thread_local! {
    /// 底图窗口表。
    ///
    /// 用 `thread_local` 而不是全局锁是刻意的：Windows 的窗口归创建它的线程所有，
    /// GDI 对象也最好别跨线程用。把状态绑在线程上，"只能在 UI 线程访问"这条约束
    /// 就是结构性的，而不是靠注释提醒 —— 顺带也不用为一堆裸指针写 `unsafe impl Send`。
    ///
    /// 代价是从别的线程调用会看到一张空表、静默什么都不做。[`check_thread`] 就是
    /// 为了让这种情况在日志里炸出来，而不是表现为"按 F1 没有底图"。
    static SURFACES: RefCell<HashMap<u64, Surface>> = RefCell::new(HashMap::new());
}

/// 第一次建窗时所在的线程。之后所有调用都必须来自同一个线程。
static OWNER_THREAD: OnceLock<u32> = OnceLock::new();

fn check_thread(what: &str) {
    // SAFETY: GetCurrentThreadId 无参数、无失败模式。
    let current = unsafe { GetCurrentThreadId() };
    let owner = *OWNER_THREAD.get_or_init(|| current);
    if current != owner {
        tracing::error!(
            what,
            current,
            owner,
            "底图层被非 UI 线程调用 —— 窗口表按线程隔离，这次调用不会有任何效果"
        );
    }
}

impl BackdropLayer for WindowsBackdropLayer {
    fn ensure(&self, monitor: MonitorId) -> AppResult<()> {
        check_thread("ensure");
        SURFACES.with_borrow_mut(|surfaces| {
            if surfaces.contains_key(&monitor.0) {
                return Ok(());
            }
            let hwnd = create_window()?;
            surfaces.insert(
                monitor.0,
                Surface {
                    hwnd,
                    dc: HDC::default(),
                    bitmap: HBITMAP::default(),
                    previous: HGDIOBJ::default(),
                    bits: std::ptr::null_mut(),
                    width: 0,
                    height: 0,
                },
            );
            Ok(())
        })
    }

    fn load(&self, monitor: MonitorId, image: &RgbaImage, at: PhysicalRect) -> AppResult<()> {
        check_thread("load");
        self.ensure(monitor)?;

        SURFACES.with_borrow_mut(|surfaces| {
            let surface = surfaces
                .get_mut(&monitor.0)
                .ok_or_else(|| AppError::Window(format!("底图窗口 {monitor} 不存在")))?;

            // 分辨率变了（或第一次装载）就重建 DIB。尺寸没变时复用 —— 4K 一张
            // 33MB，反复分配会白白吃掉一堆首次触碰的缺页开销。
            if surface.width != image.width() || surface.height != image.height() {
                rebuild_dib(surface, image.width(), image.height())?;
            }

            // SAFETY: 上面刚保证过 DIB 尺寸和 image 一致，缓冲有效。
            let dst = unsafe { surface.pixels() };
            copy_rgba_to_bgra(image.as_raw(), dst);

            push_to_dwm(surface, at)
        })
    }

    fn show_above(&self, monitor: MonitorId, overlay: u64) -> AppResult<()> {
        check_thread("show_above");

        let backdrop = SURFACES.with_borrow(|surfaces| surfaces.get(&monitor.0).map(|s| s.hwnd));
        let Some(backdrop) = backdrop else {
            return Err(AppError::Window(format!("底图窗口 {monitor} 不存在")));
        };
        let overlay = HWND(overlay as *mut _);

        // 一次 DeferWindowPos 批次里的窗口在同一帧生效。分两次 ShowWindow 的话，
        // 中间会有一帧只有底图、没有压暗遮罩 —— 那是一道很显眼的闪光。
        //
        // SAFETY: 两个句柄都指向存活的窗口（backdrop 由本模块持有，overlay 由
        // 调用方从 Tauri 现取），且都归当前线程所有。
        unsafe {
            let batch = BeginDeferWindowPos(2)
                .map_err(|err| AppError::Window(format!("BeginDeferWindowPos 失败: {err}")))?;

            // 先摆遮罩：它是 topmost 的锚。
            let batch = DeferWindowPos(
                batch,
                overlay,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .map_err(|err| AppError::Window(format!("DeferWindowPos(遮罩) 失败: {err}")))?;

            // 底图插在遮罩之后 = z 序更低。位置和尺寸已经由
            // UpdateLayeredWindow 设好，这里只动 z 序和可见性。
            //
            // 顺带一提：hWndInsertAfter 指的是"排在谁后面"，而不是"谁排在它后面"
            // —— 这两个方向搞反的话底图会盖住遮罩，画面上就是压暗层整个消失。
            let batch = DeferWindowPos(
                batch,
                backdrop,
                Some(overlay),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .map_err(|err| AppError::Window(format!("DeferWindowPos(底图) 失败: {err}")))?;

            EndDeferWindowPos(batch)
                .map_err(|err| AppError::Window(format!("EndDeferWindowPos 失败: {err}")))?;
        }

        Ok(())
    }

    fn hide_all(&self) {
        check_thread("hide_all");
        SURFACES.with_borrow(|surfaces| {
            for surface in surfaces.values() {
                // SAFETY: 句柄由本模块持有，尚未销毁。
                let _ = unsafe { ShowWindow(surface.hwnd, SW_HIDE) };
            }
        });
    }

    fn retain(&self, keep: &[MonitorId]) {
        check_thread("retain");
        SURFACES.with_borrow_mut(|surfaces| {
            let doomed: Vec<u64> = surfaces
                .keys()
                .copied()
                .filter(|id| !keep.iter().any(|m| m.0 == *id))
                .collect();

            for id in doomed {
                if let Some(surface) = surfaces.remove(&id) {
                    tracing::info!(monitor = id, "显示器已移除，销毁对应底图窗口");
                    surface.destroy();
                }
            }
        });
    }
}

fn create_window() -> AppResult<HWND> {
    register_class()?;

    // SAFETY: 类已注册；所有参数都是常量或空。失败会返回错误的 HWND，下面查了。
    let hwnd = unsafe {
        CreateWindowExW(
            if std::env::var("CHENOCR_BACKDROP_NO_TOPMOST").is_ok() {
                // 诊断用：证明上面那段说明里的 z-band 结论不是空话。
                // 去掉 TOPMOST 之后 verify-backdrop.ps1 必须失败。
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW
            } else {
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
            },
            CLASS_NAME,
            w!("CHENOCR Backdrop"),
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
    }
    .map_err(|err| AppError::Window(format!("创建底图窗口失败: {err}")))?;

    if super::self_capture::excluded() {
        // SAFETY: hwnd 刚创建成功。
        if let Err(err) = unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) } {
            // Windows 10 2004 以下不支持，静默降级（规格 07 §4.4）。
            tracing::debug!("底图窗口排除抓屏失败: {err}");
        }
    }

    Ok(hwnd)
}

fn register_class() -> AppResult<()> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();

    REGISTERED
        .get_or_init(|| {
            // SAFETY: GetModuleHandleW(None) 取当前进程模块，不会失败到需要处理；
            // RegisterClassExW 的入参是一个完整初始化的结构体。
            let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }
                .map_err(|err| format!("取模块句柄失败: {err}"))?
                .into();

            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance,
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };

            if unsafe { RegisterClassExW(&class) } == 0 {
                return Err(format!(
                    "注册窗口类失败: {}",
                    windows::core::Error::from_thread()
                ));
            }
            Ok(())
        })
        .clone()
        .map_err(AppError::Window)
}

/// 底图窗口不处理任何消息。
///
/// 内容是 `UpdateLayeredWindow` 推的，不走 `WM_PAINT`；输入靠 `WS_EX_TRANSPARENT`
/// 穿透，压根收不到鼠标消息。
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: 转发给系统默认处理，参数原样传递。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn rebuild_dib(surface: &mut Surface, width: u32, height: u32) -> AppResult<()> {
    // SAFETY: 先把旧的拆干净，再建新的。旧句柄若是默认值（第一次装载），
    // 这些调用都是无害的 no-op。
    unsafe {
        if !surface.dc.is_invalid() {
            SelectObject(surface.dc, surface.previous);
            let _ = DeleteDC(surface.dc);
        }
        if !surface.bitmap.is_invalid() {
            let _ = DeleteObject(surface.bitmap.into());
        }
    }
    surface.bits = std::ptr::null_mut();
    surface.width = 0;
    surface.height = 0;

    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            // 负高度 = 自上而下，和 RgbaImage 的行序一致，不用翻转
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();

    // SAFETY: `info` 描述的是一张合法的 32bpp DIB；`bits` 接收 GDI 分配的
    // 像素指针，其生命周期与返回的 HBITMAP 绑定。
    let bitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }
        .map_err(|err| AppError::Window(format!("创建底图 DIB 失败: {err}")))?;

    if bits.is_null() {
        // SAFETY: bitmap 刚创建成功，这里把它删掉避免泄漏。
        let _ = unsafe { DeleteObject(bitmap.into()) };
        return Err(AppError::Window("底图 DIB 没有返回像素指针".into()));
    }

    // SAFETY: 内存 DC 不依赖任何设备；把 DIB 选进去之后它就是 blit 的源。
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        let _ = unsafe { DeleteObject(bitmap.into()) };
        return Err(AppError::Window("创建底图内存 DC 失败".into()));
    }
    let previous = unsafe { SelectObject(dc, bitmap.into()) };

    surface.dc = dc;
    surface.bitmap = bitmap;
    surface.previous = previous;
    surface.bits = bits.cast::<u8>();
    surface.width = width;
    surface.height = height;

    Ok(())
}

/// RGBA → BGRA。alpha 一律写 255。
///
/// 抓屏结果的 alpha 是没有语义的垃圾（WGC 那条路给回来常年是 0），照抄的话
/// `ULW_OPAQUE` 虽然会忽略它，但一旦以后改成按像素混合就会整块透明。写死 255
/// 在这里是零成本 —— 反正已经在逐像素搬了。
fn copy_rgba_to_bgra(src: &[u8], dst: &mut [u8]) {
    for (out, inp) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        out[0] = inp[2];
        out[1] = inp[1];
        out[2] = inp[0];
        out[3] = 255;
    }
}

/// 把像素交给 DWM，同时设定窗口位置和尺寸。窗口保持当前可见性。
fn push_to_dwm(surface: &Surface, at: PhysicalRect) -> AppResult<()> {
    let position = POINT { x: at.x, y: at.y };
    let size = SIZE {
        cx: surface.width as i32,
        cy: surface.height as i32,
    };
    let source = POINT { x: 0, y: 0 };

    // SAFETY: hwnd 是本模块创建的 WS_EX_LAYERED 窗口；`surface.dc` 里选中的
    // DIB 尺寸就是 `size`；hdcDst 传 None 表示用屏幕 DC。
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
            // 不透明：底图是屏幕内容，没有透明的道理。传 None 的 BLENDFUNCTION
            // 配 ULW_OPAQUE 就是"整幅照搬"，DIB 里的 alpha 字节不参与运算。
            ULW_OPAQUE,
        )
    }
    .map_err(|err| AppError::Window(format!("UpdateLayeredWindow 失败: {err}")))?;

    Ok(())
}