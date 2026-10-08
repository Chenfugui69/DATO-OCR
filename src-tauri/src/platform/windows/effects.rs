//! Tauri 窗口上的原生小动作。

use tauri::WebviewWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_EX_NOACTIVATE,
};

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
    DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    CreateRectRgn, CreateRoundRectRgn, DeleteObject, SetWindowRgn,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use super::util::{handle_of, hwnd};
use crate::error::AppResult;
pub use crate::platform::generic::{
    apply_theme, build_floating, frame_window, is_foreground, place_window, set_above_dock,
    slide_window, take_focus, window_effects, SLIDES_WINDOWS,
};

// ───────────────────────── 毛玻璃背板 ─────────────────────────
//
// 为什么不用 Tauri 的 `set_effects(Acrylic)`：Win11 上它走 DWM 的系统背板（DWMSBT），背板永远铺满
// 整个矩形窗口，`SetWindowRgn` 也裁不掉。面板是圆角的，圆角外面就露出一块方的毛玻璃，看着像一圈
// 方方的阴影（用户报过）。系统圆角又只有 8px 一档。
//
// 改成自己在窗口最底下挂一层 Windows.UI.Composition 的视觉对象：画刷用系统给的"宿主背板"
// （就是亚克力用的那张已经模糊好的桌面），再用圆角矩形裁剪。半径随便设，边缘有抗锯齿，
// 网页内容盖在它上面（窗口本身是透明的）。

use std::cell::RefCell;
use std::collections::HashMap;

use windows::core::Interface;
use windows::core::BOOL;
use windows::System::DispatcherQueueController;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::DWMWA_USE_HOSTBACKDROPBRUSH;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{CompositionRoundedRectangleGeometry, Compositor};
use windows_numerics::Vector2;

struct Backdrop {
    // 目标一释放，挂在窗口上的视觉树就没了，所以得一直拿着
    _target: DesktopWindowTarget,
    geometry: CompositionRoundedRectangleGeometry,
    /// 背板的范围由页面指定（展开 / 收起动画期间），窗口变大小时不自动铺满
    manual: bool,
}

thread_local! {
    /// 合成器要求当前线程有 DispatcherQueue；两个都只建一次（都在 UI 线程上）
    static COMPOSITOR: RefCell<Option<(DispatcherQueueController, Compositor)>> = const { RefCell::new(None) };
    static BACKDROPS: RefCell<HashMap<isize, Backdrop>> = RefCell::new(HashMap::new());
}

fn compositor() -> windows::core::Result<Compositor> {
    COMPOSITOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some((_, c)) = slot.as_ref() {
            return Ok(c.clone());
        }
        // SAFETY: 结构体大小如实填写；控制器存进线程局部变量，和线程同生命周期。
        let controller = unsafe {
            CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            })?
        };
        let compositor = Compositor::new()?;
        *slot = Some((controller, compositor.clone()));
        Ok(compositor)
    })
}

fn client_size(h: HWND) -> Vector2 {
    let mut rc = RECT::default();
    // SAFETY: h 是存活的窗口。
    let _ = unsafe { GetClientRect(h, &mut rc) };
    Vector2 {
        X: (rc.right - rc.left).max(1) as f32,
        Y: (rc.bottom - rc.top).max(1) as f32,
    }
}

fn attach_backdrop(h: HWND, radius: f32) -> windows::core::Result<()> {
    let key = h.0 as isize;
    let size = client_size(h);
    let corner = Vector2 {
        X: radius,
        Y: radius,
    };
    let existing = BACKDROPS.with(|b| b.borrow().get(&key).map(|bd| bd.geometry.clone()));
    if let Some(geometry) = existing {
        geometry.SetCornerRadius(corner)?;
        geometry.SetSize(size)?;
        return Ok(());
    }
    let on = BOOL(1);
    // SAFETY: 传入的指针和大小匹配一个 BOOL。
    unsafe {
        DwmSetWindowAttribute(
            h,
            DWMWA_USE_HOSTBACKDROPBRUSH,
            (&on as *const BOOL).cast(),
            std::mem::size_of::<BOOL>() as u32,
        )?;
    }
    let compositor = compositor()?;
    let interop: ICompositorDesktopInterop = compositor.cast()?;
    // SAFETY: h 是本线程创建的存活窗口。isTopmost = false：挂在网页内容下面。
    let target = unsafe { interop.CreateDesktopWindowTarget(h, false)? };
    let visual = compositor.CreateSpriteVisual()?;
    visual.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
    visual.SetBrush(&compositor.CreateHostBackdropBrush()?)?;
    let geometry = compositor.CreateRoundedRectangleGeometry()?;
    geometry.SetCornerRadius(corner)?;
    geometry.SetSize(size)?;
    visual.SetClip(&compositor.CreateGeometricClipWithGeometry(&geometry)?)?;
    target.SetRoot(&visual)?;
    BACKDROPS.with(|b| {
        b.borrow_mut().insert(
            key,
            Backdrop {
                _target: target,
                geometry,
                manual: false,
            },
        )
    });
    Ok(())
}

/// 毛玻璃背板：`Some(半径)`（物理像素）挂上或者更新半径和大小，`None` 去掉。只能在 UI 线程调用。
/// 挂不上返回 false（Win10 没有 `DWMWA_USE_HOSTBACKDROPBRUSH`，调用方退回系统亚克力）。
pub fn set_backdrop(window: &WebviewWindow, radius: Option<u32>) -> bool {
    let Ok(h) = window.hwnd() else { return false };
    match radius {
        Some(r) => match attach_backdrop(h, r as f32) {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(label = window.label(), "毛玻璃背板设置失败：{err}");
                false
            }
        },
        None => {
            let removed = BACKDROPS.with(|b| b.borrow_mut().remove(&(h.0 as isize)));
            if removed.is_some() {
                let off = BOOL(0);
                // SAFETY: 同上。
                unsafe {
                    let _ = DwmSetWindowAttribute(
                        h,
                        DWMWA_USE_HOSTBACKDROPBRUSH,
                        (&off as *const BOOL).cast(),
                        std::mem::size_of::<BOOL>() as u32,
                    );
                }
            }
            true
        }
    }
}

/// 窗口大小变了：背板的裁剪跟着变（视觉对象本身按比例铺满，不用管）。没挂背板返回 false。
/// 页面正在指定背板范围（动画期间）时不动它。
pub fn resize_backdrop(window: &WebviewWindow) -> bool {
    let Ok(h) = window.hwnd() else { return false };
    let state = BACKDROPS.with(|b| {
        b.borrow()
            .get(&(h.0 as isize))
            .map(|bd| (bd.geometry.clone(), bd.manual))
    });
    match state {
        Some((geometry, false)) => {
            let _ = geometry.SetSize(client_size(h));
            true
        }
        Some((_, true)) => true,
        None => false,
    }
}

/// 背板只铺在窗口里的一块（物理像素 x, y, w, h），`ms` > 0 时用和页面一样的缓动曲线动画过去；
/// `None` = 恢复铺满整个窗口。面板在窗口里展开 / 收起时，背板跟着面板外框走，不然毛玻璃
/// 会先铺满整个新窗口，面板还没长到那里就露出一块空的毛玻璃。只能在 UI 线程调用。
pub fn set_backdrop_rect(window: &WebviewWindow, rect: Option<(f32, f32, f32, f32)>, ms: u32) {
    let Ok(h) = window.hwnd() else { return };
    let key = h.0 as isize;
    let geometry = BACKDROPS.with(|b| {
        let mut map = b.borrow_mut();
        let bd = map.get_mut(&key)?;
        bd.manual = rect.is_some();
        Some(bd.geometry.clone())
    });
    let Some(geometry) = geometry else { return };
    let (offset, size) = match rect {
        Some((x, y, w, hh)) => (
            Vector2 { X: x, Y: y },
            Vector2 {
                X: w.max(1.0),
                Y: hh.max(1.0),
            },
        ),
        None => (Vector2 { X: 0.0, Y: 0.0 }, client_size(h)),
    };
    let result = (|| -> windows::core::Result<()> {
        let offset_name = windows::core::HSTRING::from("Offset");
        let size_name = windows::core::HSTRING::from("Size");
        geometry.StopAnimation(&offset_name)?;
        geometry.StopAnimation(&size_name)?;
        if ms == 0 {
            geometry.SetOffset(offset)?;
            geometry.SetSize(size)?;
            return Ok(());
        }
        let compositor = compositor()?;
        // 和页面里的 cubic-bezier(0.32, 0.72, 0, 1) 一样
        let ease = compositor.CreateCubicBezierEasingFunction(
            Vector2 { X: 0.32, Y: 0.72 },
            Vector2 { X: 0.0, Y: 1.0 },
        )?;
        let duration = windows::Foundation::TimeSpan {
            Duration: i64::from(ms) * 10_000,
        };
        for (name, value) in [(&offset_name, offset), (&size_name, size)] {
            let anim = compositor.CreateVector2KeyFrameAnimation()?;
            anim.InsertKeyFrameWithEasingFunction(1.0, value, &ease)?;
            anim.SetDuration(duration)?;
            geometry.StartAnimation(name, &anim)?;
        }
        Ok(())
    })();
    if let Err(err) = result {
        tracing::debug!("毛玻璃背板范围设置失败：{err}");
    }
}

pub fn native_handle(window: &WebviewWindow) -> AppResult<u64> {
    Ok(handle_of(window.hwnd()?))
}

/// 诊断开关：`CHENOCR_ALLOW_SELF_CAPTURE=1` 时，浮动窗口**显示期间**不排除抓屏。
/// 正常必须排除（否则连续截图会拍到上一次的遮罩），但排除后任何脚本化的视觉验证都
/// 截不到遮罩 —— 这个开关是自动化截屏检查的唯一手段。
///
/// 隐藏期间仍然排除：实测在本进程内用 WGC 抓屏时，没被排除的**隐藏**窗口会被画成
/// 带标题栏的白块盖在画面上（独立进程抓同一时刻的屏幕则正常）。所以诊断模式只在
/// 显示时放开，见 [`reveal_for_tests`]。
pub fn self_capture_allowed() -> bool {
    std::env::var("CHENOCR_ALLOW_SELF_CAPTURE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn set_exclude_from_capture(window: &WebviewWindow, exclude: bool) {
    let Ok(h) = window.hwnd() else { return };
    let affinity = if exclude {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // SAFETY: h 是存活的 Tauri 窗口。Win10 2004 以下不支持 EXCLUDE，静默降级。
    if let Err(err) = unsafe { SetWindowDisplayAffinity(h, affinity) } {
        tracing::debug!(label = window.label(), "设置抓屏排除失败：{err}");
    }
}

/// Win11 系统圆角（无边框窗口默认是直角）。毛玻璃材质会跟着圆角裁，阴影也是系统画的。
/// Win10 没有这个属性，调用失败就算了。
pub fn set_rounded(window: &WebviewWindow, rounded: bool) {
    let Ok(h) = window.hwnd() else { return };
    let pref: DWM_WINDOW_CORNER_PREFERENCE = if rounded {
        DWMWCP_ROUND
    } else {
        DWMWCP_DONOTROUND
    };
    // SAFETY: h 是存活的 Tauri 窗口；传入的是一个 4 字节枚举值及其大小。
    let result = unsafe {
        DwmSetWindowAttribute(
            h,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&pref).cast(),
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        )
    };
    if let Err(err) = result {
        tracing::debug!(label = window.label(), "设置窗口圆角失败：{err}");
    }
}

/// 把窗口裁成圆角矩形（`radius` 物理像素，0 = 不裁）。毛玻璃面板用：系统圆角只有 8px 一档，
/// 用户要别的半径只能自己裁。窗口大小变了要重新裁（`wm::track_glass`）。
pub fn set_round_region(window: &WebviewWindow, radius: u32) {
    let Ok(h) = window.hwnd() else { return };
    // SAFETY: h 是存活的 Tauri 窗口。SetWindowRgn 成功后区域归系统所有，失败才由我们释放。
    unsafe {
        if radius == 0 {
            let _ = SetWindowRgn(h, None, true);
            return;
        }
        let mut rc = RECT::default();
        if GetWindowRect(h, &mut rc).is_err() {
            return;
        }
        let d = (radius * 2) as i32;
        let rgn = CreateRoundRectRgn(0, 0, rc.right - rc.left + 1, rc.bottom - rc.top + 1, d, d);
        if SetWindowRgn(h, Some(rgn), true) == 0 {
            let _ = DeleteObject(rgn.into());
        }
    }
}

/// 把窗口限制在一个矩形里（物理像素，相对窗口左上角）：外面的部分不显示、点击直接穿过去。`None` = 整个窗口。
pub fn set_rect_region(window: &WebviewWindow, rect: Option<(i32, i32, i32, i32)>) {
    let Ok(h) = window.hwnd() else { return };
    // SAFETY: h 是存活的 Tauri 窗口。SetWindowRgn 成功后区域归系统所有，失败才由我们释放。
    unsafe {
        match rect {
            None => {
                let _ = SetWindowRgn(h, None, true);
            }
            Some((x, y, w, hh)) => {
                let rgn = CreateRectRgn(x, y, x + w, y + hh);
                if SetWindowRgn(h, Some(rgn), true) == 0 {
                    let _ = DeleteObject(rgn.into());
                }
            }
        }
    }
}

pub fn hide_window(window: &WebviewWindow) {
    let Ok(h) = window.hwnd() else { return };
    // SAFETY: h 是存活的 Tauri 窗口。
    unsafe {
        let _ = ShowWindow(h, SW_HIDE);
    }
}

/// 加上 `WS_EX_NOACTIVATE`：点击、显示都不会把它变成前台窗口。
pub fn set_no_activate(window: &WebviewWindow) {
    let Ok(h) = window.hwnd() else { return };
    // SAFETY: h 是存活的 Tauri 窗口；只追加一个扩展样式位。
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
    }
}

/// 置顶显示但不激活（toast 不能抢走用户正在打字的窗口的焦点）。
pub fn show_without_activate(window: &WebviewWindow) -> AppResult<()> {
    let h = hwnd(native_handle(window)?);
    // SAFETY: h 是存活的 Tauri 窗口。
    unsafe {
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        SetWindowPos(
            h,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )?;
    }
    Ok(())
}

/// 一次同时改位置和大小（物理像素）。分两次调的话中间会多出一帧"挪了没变大"或"变大了没挪"，
/// 面板展开动画就会跳一下。
pub fn set_bounds(
    window: &WebviewWindow,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> AppResult<()> {
    let h = hwnd(native_handle(window)?);
    // SAFETY: h 是存活的 Tauri 窗口。
    unsafe {
        SetWindowPos(
            h,
            None,
            x,
            y,
            width as i32,
            height as i32,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )?;
    }
    Ok(())
}

/// 同上，单位是逻辑像素。
pub fn set_bounds_logical(
    window: &WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> AppResult<()> {
    let s = window.scale_factor()?;
    set_bounds(
        window,
        (x * s).round() as i32,
        (y * s).round() as i32,
        (width * s).round().max(1.0) as u32,
        (height * s).round().max(1.0) as u32,
    )
}

/// 诊断模式下，窗口显示时放开抓屏、隐藏时恢复排除。非诊断模式什么都不做。
pub fn reveal_for_tests(window: &WebviewWindow, visible: bool) {
    if self_capture_allowed() {
        set_exclude_from_capture(window, !visible);
    }
}

pub fn reveal_hwnd_for_tests(handle: u64, visible: bool) {
    if self_capture_allowed() {
        let affinity = if visible {
            WDA_NONE
        } else {
            WDA_EXCLUDEFROMCAPTURE
        };
        // SAFETY: 句柄来自本进程创建的窗口。
        let _ = unsafe { SetWindowDisplayAffinity(hwnd(handle), affinity) };
    }
}
