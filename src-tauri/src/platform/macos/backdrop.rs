//! 冻结底图的原生显示层。
//!
//! 每块屏一个无边框窗口，画面是内容视图图层上的一张 CGImage，不经过 WebView
//! （4K 画面进 WebView 要一两百毫秒，还会被色彩管理改像素）。
//!
//! | 设置 | 为什么 |
//! |---|---|
//! | 层级 = 遮罩 - 1 | 紧贴在遮罩下面，别的应用的置顶窗口夹不进来 |
//! | `ignoresMouseEvents` | 鼠标穿透，绝不能吃掉本该给遮罩的点击 |
//! | 普通窗口（不是面板）且不设为焦点 | 永不拿键盘焦点 |
//! | `sharingType = none` | 长截图采集时不能把上一帧的底图拍进去 |
//! | 不带出现动画 | 要第一帧就是最终画面 |
//!
//! 窗口归主线程所有，状态放 `thread_local`，**只能在主线程调用**。

use std::cell::RefCell;
use std::collections::HashMap;

use image::RgbaImage;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSWindow, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
    NSWindowSharingType, NSWindowStyleMask,
};
use objc2_quartz_core::CATransaction;

use super::effects::{self, OVERLAY_LEVEL};
use super::{geometry, util};
use crate::error::{AppError, AppResult};
use crate::platform::{MonitorId, PhysicalRect};

struct Surface {
    window: Retained<NSWindow>,
}

thread_local! {
    static SURFACES: RefCell<HashMap<u64, Surface>> = RefCell::new(HashMap::new());
}

fn main_thread(what: &str) -> AppResult<MainThreadMarker> {
    MainThreadMarker::new().ok_or_else(|| {
        tracing::error!(what, "底图层被非主线程调用，这次调用不会生效");
        AppError::msg("底图层只能在主线程使用")
    })
}

fn set_shared(window: &NSWindow, shared: bool) {
    window.setSharingType(if shared {
        NSWindowSharingType::ReadOnly
    } else {
        NSWindowSharingType::None
    });
}

fn create_window(mtm: MainThreadMarker) -> Retained<NSWindow> {
    // SAFETY: 标准的窗口创建；不随关闭释放，生命周期由 Retained 管。
    let window = unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            geometry::rect(0.0, 0.0, 1.0, 1.0),
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window
    };
    window.setOpaque(true);
    window.setHasShadow(false);
    window.setIgnoresMouseEvents(true);
    window.setLevel(OVERLAY_LEVEL - 1);
    window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    set_shared(&window, false);
    if let Some(view) = window.contentView() {
        view.setWantsLayer(true);
    }
    window
}

pub fn ensure(monitor: MonitorId) -> AppResult<()> {
    let mtm = main_thread("ensure")?;
    SURFACES.with_borrow_mut(|surfaces| {
        surfaces.entry(monitor.0).or_insert_with(|| Surface {
            window: create_window(mtm),
        });
    });
    Ok(())
}

pub fn load(monitor: MonitorId, image: &RgbaImage, at: PhysicalRect) -> AppResult<()> {
    main_thread("load")?;
    ensure(monitor)?;
    let cg = util::cgimage_from_rgba(image, true).ok_or_else(|| AppError::msg("底图转换失败"))?;
    SURFACES.with_borrow(|surfaces| {
        let surface = surfaces
            .get(&monitor.0)
            .ok_or_else(|| AppError::msg(format!("底图窗口 {monitor} 不存在")))?;
        let frame = geometry::flip(geometry::rect_to_points(at));
        surface.window.setFrame_display(frame, false);
        let layer = surface
            .window
            .contentView()
            .and_then(|v| v.layer())
            .ok_or_else(|| AppError::msg("底图窗口没有图层"))?;
        // 关掉隐式动画：换图不能淡入淡出
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        // SAFETY: CGImage 是 CoreFoundation 对象，可以当 id 交给图层；图层自己会持有它。
        unsafe { layer.setContents(Some(&*cg.0.cast::<AnyObject>())) };
        CATransaction::commit();
        Ok(())
    })
}

/// 底图和遮罩在同一轮事件循环里先后排到前面，同一帧上屏。
pub fn show_below(monitor: MonitorId, overlay: u64) -> AppResult<()> {
    let mtm = main_thread("show_below")?;
    SURFACES.with_borrow(|surfaces| {
        let surface = surfaces
            .get(&monitor.0)
            .ok_or_else(|| AppError::msg(format!("底图窗口 {monitor} 不存在")))?;
        surface.window.orderFrontRegardless();
        if effects::self_capture_allowed() {
            set_shared(&surface.window, true);
        }
        if let Some(overlay) = effects::window_of_handle(overlay, mtm) {
            overlay.orderFrontRegardless();
        }
        Ok(())
    })
}

fn hide(surface: &Surface) {
    surface.window.orderOut(None);
    set_shared(&surface.window, false);
}

pub fn hide_all() {
    if main_thread("hide_all").is_err() {
        return;
    }
    SURFACES.with_borrow(|surfaces| surfaces.values().for_each(hide));
}

/// 隐藏并释放画面（每块 4K 屏 33MB），窗口本身保留复用。
pub fn release_all() {
    if main_thread("release_all").is_err() {
        return;
    }
    SURFACES.with_borrow(|surfaces| {
        for surface in surfaces.values() {
            hide(surface);
            if let Some(layer) = surface.window.contentView().and_then(|v| v.layer()) {
                // SAFETY: 清空图层内容。
                unsafe { layer.setContents(None) };
            }
        }
    });
}

pub fn retain(keep: &[MonitorId]) {
    if main_thread("retain").is_err() {
        return;
    }
    SURFACES.with_borrow_mut(|surfaces| {
        surfaces.retain(|id, surface| {
            let alive = keep.iter().any(|m| m.0 == *id);
            if !alive {
                surface.window.orderOut(None);
            }
            alive
        });
    });
}
