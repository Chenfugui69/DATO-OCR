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
    apply_theme, build_floating, cursor_in_window, frame_window, is_foreground, place_window, set_above_dock, start_resize_drag, take_focus, window_effects, window_placement,
    OVER_DOCK_GAP,
};
use crate::platform::PhysicalRect;

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
use windows::UI::Composition::{CompositionRoundedRectangleGeometry, Compositor, SpriteVisual};
use windows_numerics::{Vector2, Vector3};

struct Backdrop {
    // 目标一释放，挂在窗口上的视觉树就没了，所以得一直拿着
    _target: DesktopWindowTarget,
    geometry: CompositionRoundedRectangleGeometry,
    /// 背板本身：出场 / 收起时整体淡入淡出、缩放（`fade_backdrop`）
    visual: SpriteVisual,
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

/// 背板比面板四周各缩进这么多（物理像素）。合成器的几何裁剪**没有抗锯齿**，圆角是一级一级的台阶；
/// 页面画的面板圆角是抗锯齿的。两者一样大的话，面板边缘那圈半透明的像素外面会露出背板的台阶，
/// 圆角看着有锯齿（用户报过）。缩进一点，台阶就整个藏在面板的色调底下了。
const BACKDROP_INSET: f32 = 1.0;

/// 把背板摆在 `offset` 起、`size` 大的范围里（已经是面板的范围），实际裁剪再往里缩一圈。
fn inset_rect(offset: Vector2, size: Vector2) -> (Vector2, Vector2) {
    (
        Vector2 {
            X: offset.X + BACKDROP_INSET,
            Y: offset.Y + BACKDROP_INSET,
        },
        Vector2 {
            X: (size.X - 2.0 * BACKDROP_INSET).max(1.0),
            Y: (size.Y - 2.0 * BACKDROP_INSET).max(1.0),
        },
    )
}

fn attach_backdrop(h: HWND, radius: f32) -> windows::core::Result<()> {
    let key = h.0 as isize;
    let (offset, size) = inset_rect(Vector2 { X: 0.0, Y: 0.0 }, client_size(h));
    let r = (radius - BACKDROP_INSET).max(0.0);
    let corner = Vector2 { X: r, Y: r };
    let existing = BACKDROPS.with(|b| {
        b.borrow()
            .get(&key)
            .map(|bd| (bd.geometry.clone(), bd.manual))
    });
    if let Some((geometry, manual)) = existing {
        geometry.SetCornerRadius(corner)?;
        // 页面正在指定背板范围时只换圆角，范围不动
        if !manual {
            geometry.SetOffset(offset)?;
            geometry.SetSize(size)?;
        }
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
    geometry.SetOffset(offset)?;
    geometry.SetSize(size)?;
    visual.SetClip(&compositor.CreateGeometricClipWithGeometry(&geometry)?)?;
    target.SetRoot(&visual)?;
    BACKDROPS.with(|b| {
        b.borrow_mut().insert(
            key,
            Backdrop {
                _target: target,
                geometry,
                visual,
                manual: false,
            },
        )
    });
    Ok(())
}

// ───────────────────────── 窗口区域 ─────────────────────────
//
// 背板按圆角裁了，但只在屏幕上的合成里管用：抓屏（WGC，截图功能就是用它）抓到的画面里背板是整个
// 矩形，圆角外面露出四个浅色的小方角（用户截图里看到的）。所以挂了背板的窗口，窗口区域本身也裁成
// 圆角：圆角外面根本不属于这个窗口，屏幕上、抓屏里都一样。
//
// 窗口区域可能同时被两边要求：页面 / 后端指定只保留一块（`set_rect_region`，抽屉的预留），背板要圆角。
// 两样都记下来，每次按两者合起来重算一次。命令可能在非 UI 线程调，所以用锁不用 thread_local。

/// 窗口里的一块：x, y, 宽, 高（物理像素，相对窗口左上角）
type LocalRect = (i32, i32, i32, i32);
/// 每个窗口当前要求保留的那一块；没有 = 整个窗口
static REGION_RECT: parking_lot::Mutex<Option<HashMap<isize, LocalRect>>> =
    parking_lot::Mutex::new(None);
/// 挂了背板的窗口：圆角半径（物理像素）、背板是不是只铺在窗口里指定的一块（`set_backdrop_rect`）
static REGION_RADIUS: parking_lot::Mutex<Option<HashMap<isize, (u32, bool)>>> =
    parking_lot::Mutex::new(None);

/// 窗口区域的圆角比面板小这么多（物理像素）：弧线落在面板圆角外面一点，页面画的抗锯齿边缘整个留着，
/// 只切掉角上那块方的。区域本身没有抗锯齿，一样大的话会把边缘那圈半透明像素切成台阶
const REGION_RADIUS_SLACK: u32 = 2;

/// 按记下的"保留哪一块"和"圆角多大"重设窗口区域。
///
/// 背板只铺在窗口里一小块、窗口又没指定保留哪一块（截图遮罩上工具条的毛玻璃）：圆角是那一小块的，
/// 不是窗口的，窗口区域不裁圆角。
fn apply_region(h: HWND) {
    let key = h.0 as isize;
    let rect = REGION_RECT.lock().as_ref().and_then(|m| m.get(&key).copied());
    let radius = REGION_RADIUS
        .lock()
        .as_ref()
        .and_then(|m| m.get(&key).copied())
        .filter(|&(_, manual)| !manual || rect.is_some())
        .map(|(r, _)| r.saturating_sub(REGION_RADIUS_SLACK))
        .filter(|&r| r > 0);
    // SAFETY: h 是存活的本进程窗口。SetWindowRgn 成功后区域归系统所有，失败才由我们释放。
    unsafe {
        let rgn = match (rect, radius) {
            (None, None) => {
                let _ = SetWindowRgn(h, None, true);
                return;
            }
            (Some((x, y, w, hh)), None) => CreateRectRgn(x, y, x + w, y + hh),
            (rect, Some(r)) => {
                let (x, y, w, hh) = rect.unwrap_or_else(|| {
                    let size = client_size(h);
                    (0, 0, size.X as i32, size.Y as i32)
                });
                let d = (r * 2) as i32;
                CreateRoundRectRgn(x, y, x + w + 1, y + hh + 1, d, d)
            }
        };
        if SetWindowRgn(h, Some(rgn), true) == 0 {
            let _ = DeleteObject(rgn.into());
        }
    }
}

/// 每次都重设区域，不管半径变没变：`wm::set_glass` 挂背板之前会先把窗口区域清掉
fn set_region_radius(h: HWND, radius: Option<u32>) {
    // 锁只在这个块里拿着：apply_region 还要再拿一次
    {
        let mut map = REGION_RADIUS.lock();
        let map = map.get_or_insert_with(HashMap::new);
        let key = h.0 as isize;
        match radius {
            Some(r) => {
                let manual = map.get(&key).is_some_and(|&(_, m)| m);
                map.insert(key, (r, manual));
            }
            None => {
                map.remove(&key);
            }
        }
    }
    apply_region(h);
}

/// 背板改成只铺一块 / 恢复铺满：窗口区域要不要裁圆角跟着变。
fn set_region_manual(h: HWND, manual: bool) {
    let changed = {
        let mut map = REGION_RADIUS.lock();
        match map.as_mut().and_then(|m| m.get_mut(&(h.0 as isize))) {
            Some(entry) if entry.1 != manual => {
                entry.1 = manual;
                true
            }
            _ => false,
        }
    };
    if changed {
        apply_region(h);
    }
}

/// 毛玻璃背板：`Some(半径)`（物理像素）挂上或者更新半径和大小，`None` 去掉。只能在 UI 线程调用。
/// 挂不上返回 false（Win10 没有 `DWMWA_USE_HOSTBACKDROPBRUSH`，调用方退回系统亚克力）。
pub fn set_backdrop(window: &WebviewWindow, radius: Option<u32>) -> bool {
    let Ok(h) = window.hwnd() else { return false };
    match radius {
        Some(r) => match attach_backdrop(h, r as f32) {
            Ok(()) => {
                set_region_radius(h, Some(r));
                true
            }
            Err(err) => {
                tracing::warn!(label = window.label(), "毛玻璃背板设置失败：{err}");
                false
            }
        },
        None => {
            set_region_radius(h, None);
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
    // 圆角的窗口区域按新大小重裁（只保留一块的话那块是页面指定的，不跟着窗口变，重算也一样）
    if REGION_RADIUS
        .lock()
        .as_ref()
        .is_some_and(|m| m.contains_key(&(h.0 as isize)))
    {
        apply_region(h);
    }
    let state = BACKDROPS.with(|b| {
        b.borrow()
            .get(&(h.0 as isize))
            .map(|bd| (bd.geometry.clone(), bd.manual))
    });
    match state {
        Some((geometry, false)) => {
            let (_, size) = inset_rect(Vector2 { X: 0.0, Y: 0.0 }, client_size(h));
            let _ = geometry.SetSize(size);
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
    set_region_manual(h, rect.is_some());
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
    let (offset, size) = inset_rect(offset, size);
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

/// 背板整体出场 / 收起：`visible` 时从透明、略小（0.96）放大淡入，否则淡出、略缩（0.98）并停在透明，
/// 下次出场从透明开始。缩放以面板（背板裁剪范围）的中心为原点，和页面里面板自己的出场动画同一套。
/// 系统背板是窗口底下的原生一层，页面的 CSS 动画带不动它，不一起动的话面板淡入时背后的毛玻璃是一下子出现的。
/// 只能在 UI 线程调用；没挂背板什么都不做。
pub fn fade_backdrop(window: &WebviewWindow, visible: bool, ms: u32) {
    let Ok(h) = window.hwnd() else { return };
    let parts = BACKDROPS.with(|b| {
        b.borrow()
            .get(&(h.0 as isize))
            .map(|bd| (bd.visual.clone(), bd.geometry.clone()))
    });
    let Some((visual, geometry)) = parts else { return };
    let result = (|| -> windows::core::Result<()> {
        let at = geometry.Offset()?;
        let size = geometry.Size()?;
        visual.SetCenterPoint(Vector3 {
            X: at.X + size.X / 2.0,
            Y: at.Y + size.Y / 2.0,
            Z: 0.0,
        })?;
        let opacity_name = windows::core::HSTRING::from("Opacity");
        let scale_name = windows::core::HSTRING::from("Scale");
        visual.StopAnimation(&opacity_name)?;
        visual.StopAnimation(&scale_name)?;
        let (from, to) = if visible { ((0.0, 0.96), (1.0, 1.0)) } else { ((visual.Opacity()?, visual.Scale()?.X), (0.0, 0.98)) };
        let scale = |k: f32| Vector3 { X: k, Y: k, Z: 1.0 };
        if ms == 0 {
            visual.SetOpacity(to.0)?;
            visual.SetScale(scale(to.1))?;
            return Ok(());
        }
        let compositor = compositor()?;
        // 出场：cubic-bezier(0.22, 1, 0.36, 1)，快起慢收；收起：cubic-bezier(0.4, 0, 1, 1)，干脆利落
        let ease = if visible {
            compositor.CreateCubicBezierEasingFunction(Vector2 { X: 0.22, Y: 1.0 }, Vector2 { X: 0.36, Y: 1.0 })?
        } else {
            compositor.CreateCubicBezierEasingFunction(Vector2 { X: 0.4, Y: 0.0 }, Vector2 { X: 1.0, Y: 1.0 })?
        };
        let duration = windows::Foundation::TimeSpan {
            Duration: i64::from(ms) * 10_000,
        };
        let fade = compositor.CreateScalarKeyFrameAnimation()?;
        fade.InsertKeyFrame(0.0, from.0)?;
        fade.InsertKeyFrameWithEasingFunction(1.0, to.0, &ease)?;
        fade.SetDuration(duration)?;
        let grow = compositor.CreateVector3KeyFrameAnimation()?;
        grow.InsertKeyFrame(0.0, scale(from.1))?;
        grow.InsertKeyFrameWithEasingFunction(1.0, scale(to.1), &ease)?;
        grow.SetDuration(duration)?;
        // 动画开始前先落到起点：命令到的这一帧不会闪一下终点的样子
        visual.SetOpacity(from.0)?;
        visual.SetScale(scale(from.1))?;
        visual.StartAnimation(&opacity_name, &fade)?;
        visual.StartAnimation(&scale_name, &grow)?;
        Ok(())
    })();
    if let Err(err) = result {
        tracing::debug!("毛玻璃背板淡入淡出失败：{err}");
    }
}

// ───────────────────────── 面板滑入 / 滑出 ─────────────────────────
//
// 不开毛玻璃时面板是页面里的一张卡片，滑入滑出是页面自己的动画。开了毛玻璃，窗口就是面板本身
// （背板是窗口的一部分，页面带不动它），只能挪窗口：一个线程按帧 `SetWindowPos`。
//
// 面板是从任务栏上沿"长"出来的，不是从屏幕外飞进来：置顶窗口会画在任务栏上面，直接从屏幕底下
// 滑上来要先盖住任务栏再离开，很怪。所以起点就在停稳位置的正下方、紧贴工作区底边，滑动时用窗口
// 区域把超出那条线的部分裁掉。

/// 每开始一次滑动加一：旧的那次发现自己过期了就停手
static SLIDE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn slides_windows(glass: bool) -> bool {
    glass
}

/// 滑入前 / 滑出后待的位置：顶边在工作区底边上（整个被裁掉，看不见）。
pub fn slide_start(rect: PhysicalRect, monitor: &crate::platform::MonitorInfo) -> PhysicalRect {
    PhysicalRect::new(rect.x, monitor.work_area.bottom(), rect.width, rect.height)
}

/// cubic-bezier(0.32, 0.72, 0, 1)，和页面里面板用的缓动（--cn-ease-sheet）一样。
fn ease_sheet(t: f64) -> f64 {
    let (x1, y1, x2, y2) = (0.32, 0.72, 0.0, 1.0);
    let bez = |a: f64, b: f64, u: f64| {
        3.0 * (1.0 - u).powi(2) * u * a + 3.0 * (1.0 - u) * u * u * b + u.powi(3)
    };
    // 二分找参数 u，使横坐标等于 t
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if bez(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, (lo + hi) / 2.0)
}

/// 把窗口从现在的位置竖着滑到 `to`（屏幕物理像素），`ms` 毫秒。
pub fn slide_window(window: &WebviewWindow, to: PhysicalRect, ms: u32) {
    use std::sync::atomic::Ordering;
    let Ok(h) = window.hwnd() else { return };
    let mut rc = RECT::default();
    // SAFETY: h 是存活的 Tauri 窗口。
    if unsafe { GetWindowRect(h, &mut rc) }.is_err() {
        return;
    }
    let (x, from_y, to_y) = (to.x, rc.top, to.y);
    let (w, hh) = (to.width as i32, to.height as i32);
    // 两头里靠下的那个位置的顶边 = 面板"长出来"的那条线，线以下的部分裁掉
    let line = from_y.max(to_y);
    let handle = h.0 as isize;
    let gen = SLIDE_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let step = move |y: i32, last: bool| {
        let h = HWND(handle as *mut _);
        // SAFETY: 句柄来自本进程的窗口；窗口没了这些调用只是失败。区域设置成功后归系统所有。
        unsafe {
            if last && to_y < line {
                // 停稳在上面：整个窗口都要显示，去掉裁剪
                let _ = SetWindowRgn(h, None, false);
            } else {
                let rgn = CreateRectRgn(0, 0, w, (line - y).clamp(0, hh));
                if SetWindowRgn(h, Some(rgn), false) == 0 {
                    let _ = DeleteObject(rgn.into());
                }
            }
            let _ = SetWindowPos(
                h,
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    };
    step(from_y, false);
    let _ = std::thread::Builder::new()
        .name("panel-slide".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let total = f64::from(ms.max(1));
            loop {
                if SLIDE_GEN.load(Ordering::SeqCst) != gen {
                    return;
                }
                let t = (started.elapsed().as_secs_f64() * 1000.0 / total).min(1.0);
                let y = from_y + (f64::from(to_y - from_y) * ease_sheet(t)).round() as i32;
                step(y, t >= 1.0);
                if t >= 1.0 {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(6));
            }
        });
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
    if !rounded {
        suppress_nc_frame(h);
    }
}

/// 窗口激活 / 失去激活时（`WM_NCACTIVATE`），系统默认处理会在窗口外沿画一圈老式的 1px 窗框，
/// 直接画进窗口内容里：无边框窗口的客户区就是整个窗口，这圈线就压在面板边上，被窗口区域裁剪后
/// 剩下左、右、下三条白线和圆角（顶上那条落在裁掉的预留区里）。只有窗口改大小重画时才会消失，
/// 所以"有时有一圈白边、拖一下大小就没了"。毛玻璃面板没有窗框，这条消息直接吞掉：
/// 返回 TRUE 让激活照常进行，不交给默认处理去画框。
fn suppress_nc_frame(h: HWND) {
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::WM_NCACTIVATE;

    unsafe extern "system" fn proc(
        h: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if msg == WM_NCACTIVATE {
            return LRESULT(1);
        }
        // SAFETY: 原样交给下一层窗口过程。
        unsafe { DefSubclassProc(h, msg, wparam, lparam) }
    }
    // SAFETY: 在窗口所在的 UI 线程调用；同一个 id 重复挂只是更新，不会挂两层。
    if !unsafe { SetWindowSubclass(h, Some(proc), 0x4E43, 0) }.as_bool() {
        tracing::debug!("挂窗口子类失败，面板激活时可能画出系统窗框");
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
/// 挂了毛玻璃背板的窗口，这一块按背板的圆角裁（见上面"窗口区域"）。
pub fn set_rect_region(window: &WebviewWindow, rect: Option<(i32, i32, i32, i32)>) {
    let Ok(h) = window.hwnd() else { return };
    {
        let mut map = REGION_RECT.lock();
        let map = map.get_or_insert_with(HashMap::new);
        match rect {
            Some(r) => map.insert(h.0 as isize, r),
            None => map.remove(&(h.0 as isize)),
        };
    }
    apply_region(h);
}

pub fn hide_window(window: &WebviewWindow) {
    let Ok(h) = window.hwnd() else { return };
    // SAFETY: h 是存活的 Tauri 窗口。
    unsafe {
        let _ = ShowWindow(h, SW_HIDE);
    }
}

/// 截图收工：藏起来、缩成 1×1、网页内存目标降到 Low。
///
/// 只藏不缩的话 WebView2 还按整屏留着画面缓冲（4K 屏一个遮罩一百多 MB 显存），待机时一直占着；
/// 缩了之后上一张截图的纹理、像素数组还留在 GPU 缓存和 JS 堆里（又是一百多 MB），Low 让它们
/// 马上清掉。网页照常运行（不像 `IsVisible = false` 那样停帧），下次截图前就能先画好。
/// 下次截图 `overlay::arrange` 摆窗口时铺满整屏、`unpark_overlay` 把内存目标调回来。
pub fn park_overlay(window: &WebviewWindow) {
    let _ = window.hide();
    let _ = window.set_size(tauri::PhysicalSize::new(1, 1));
    super::webview_idle::set_memory_low(window, true);
}

pub fn unpark_overlay(window: &WebviewWindow) {
    super::webview_idle::set_memory_low(window, false);
}

/// 加上 `WS_EX_NOACTIVATE`：点击、显示都不会把它变成前台窗口。
/// WebView2 的页面本来就按屏幕刷新率刷新。
pub fn unlock_frame_rate(_window: &WebviewWindow) {}

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

/// 见 `platform::set_overlay_capturable`。
pub fn set_overlay_capturable(window: &WebviewWindow, visible: bool) {
    set_exclude_from_capture(window, !visible);
}

/// 同上，给截图底图窗口（原生窗口，只有句柄）用。
pub fn set_hwnd_capturable(handle: u64, visible: bool) {
    let affinity = if visible {
        WDA_NONE
    } else {
        WDA_EXCLUDEFROMCAPTURE
    };
    // SAFETY: 句柄来自本进程创建的窗口。
    let _ = unsafe { SetWindowDisplayAffinity(hwnd(handle), affinity) };
}

/// 诊断模式下，窗口显示时放开抓屏、隐藏时恢复排除。非诊断模式什么都不做。
pub fn reveal_for_tests(window: &WebviewWindow, visible: bool) {
    if self_capture_allowed() {
        set_exclude_from_capture(window, !visible);
    }
}
