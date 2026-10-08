//! Tauri 窗口上的原生小动作。
//!
//! AppKit 的窗口只能在主线程碰，而上层的 command 多半跑在工作线程上，所以每个函数都先
//! 切到主线程（已经在主线程就直接执行）。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CStr;
use std::ptr::NonNull;
use std::sync::OnceLock;

use block2::RcBlock;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Imp, Sel};
use objc2::{sel, ClassType, MainThreadMarker};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSApplication, NSAutoresizingMaskOptions,
    NSEvent, NSPanel, NSUserInterfaceItemIdentification, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowOrderingMode,
    NSWindowSharingType, NSWindowStyleMask,
};
use objc2_foundation::{NSObjectProtocol, NSString, NSTimer};
use objc2_quartz_core::CAMediaTimingFunction;
use tauri::utils::config::WindowEffectsConfig;
use tauri::window::{Effect, EffectState, EffectsBuilder};
use tauri::{
    AppHandle, LogicalPosition, Manager, Runtime, TitleBarStyle, WebviewWindow,
    WebviewWindowBuilder,
};

use super::ffi::CGRect;
use super::geometry;
use super::util::{encode_handle, ns_window, on_main, own_pid};
use crate::error::{AppError, AppResult};
use crate::platform::{FloatingKind, PhysicalRect};

/// 截图遮罩的窗口层级：盖住菜单栏（24）、程序坞（20）和别的应用的置顶窗口。
/// 用屏保那一档（1000）；输入法候选窗在更高的层，标注打字时还能看到。
pub const OVERLAY_LEVEL: isize = 1000;
/// 普通浮层（面板、气泡、贴图、提示）：和系统的浮动面板同一档
const PANEL_LEVEL: isize = 3;
/// 要盖住程序坞（20）的浮层：和菜单栏上的状态项同一档
const ABOVE_DOCK_LEVEL: isize = 25;

// ───────────────────────── 浮层 = 不激活应用的面板 ─────────────────────────
//
// Tauri（tao）建出来的是普通窗口：要让它收键盘就得激活整个应用，于是用户正在用的程序被切走、
// 别的应用全屏时窗口出不来、事后还得把焦点还回去。NSPanel 加上"不激活"样式就没有这些问题 ——
// 它能拿键盘焦点而不让应用变成前台，收起时焦点自己回到原来的程序（系统的聚焦搜索就是这样）。
//
// 窗口是不是面板在分配对象的那一刻就定了，而 tao 只会分配它自己的 `TaoWindow`。办法：给
// `TaoWindow` 补一个类方法 `+alloc`，平时照常分配；建浮层之前打个标记，标记在的那一次改成分配
// 我们的面板类。对象从出生起就是面板，之后 tao 该怎么初始化、怎么用都不受影响。
//
// **不要改成"建好之后把对象的类换掉"**（`object_setClass`，tauri-nspanel 插件的做法）。试过：
// WKWebView 和 AppKit 自己都会对窗口挂属性监听（KVO），监听是按对象当时的类登记的，类一换，
// 之后注销监听就抛异常 —— 实测拔掉显示器（销毁那块屏的遮罩）、关掉贴图都因此崩溃。

/// 面板类：父类是 NSPanel，其余照着 `TaoWindow` 来 —— tao 会按名字读写成员变量 `focusable`。
fn panel_class() -> Option<&'static AnyClass> {
    static CLASS: OnceLock<Option<&'static AnyClass>> = OnceLock::new();
    *CLASS.get_or_init(|| {
        let mut builder = ClassBuilder::new(c"DatoOcrFloatingPanel", NSPanel::class())?;
        builder.add_ivar::<Bool>(FOCUSABLE);
        // SAFETY: 两个方法的签名都是 `- (BOOL)xxx`，和这里的函数一致。
        unsafe {
            builder.add_method(
                sel!(canBecomeKeyWindow),
                can_become_key as extern "C-unwind" fn(_, _) -> _,
            );
            builder.add_method(
                sel!(canBecomeMainWindow),
                never as extern "C-unwind" fn(_, _) -> _,
            );
        }
        Some(builder.register())
    })
}

const FOCUSABLE: &CStr = c"focusable";

/// 无边框窗口默认当不了键盘焦点窗口，遮罩和面板要收键盘。和 tao 一样听 `focusable` 的。
extern "C-unwind" fn can_become_key(this: &AnyObject, _: Sel) -> Bool {
    match this.class().instance_variable(FOCUSABLE) {
        // SAFETY: 这个成员变量是上面按 Bool 类型声明的。
        Some(ivar) => unsafe { *ivar.load::<Bool>(this) },
        None => Bool::YES,
    }
}

extern "C-unwind" fn never(_: &AnyObject, _: Sel) -> Bool {
    Bool::NO
}

thread_local! {
    /// 本线程（主线程）下一次分配 tao 窗口时改成分配面板
    static PANEL_NEXT: Cell<bool> = const { Cell::new(false) };
}

type AllocFn = unsafe extern "C-unwind" fn(&AnyClass, Sel) -> *mut AnyObject;
/// `TaoWindow` 原来的（从 NSObject 继承来的）`+alloc`
static ORIGINAL_ALLOC: OnceLock<AllocFn> = OnceLock::new();

extern "C-unwind" fn alloc_window(class: &AnyClass, sel: Sel) -> *mut AnyObject {
    let class = if PANEL_NEXT.replace(false) {
        panel_class().unwrap_or(class)
    } else {
        class
    };
    match ORIGINAL_ALLOC.get() {
        // SAFETY: 原实现就是 `+[NSObject alloc]`，按传进去的类分配对象。
        Some(original) => unsafe { original(class, sel) },
        None => std::ptr::null_mut(),
    }
}

/// 给 `TaoWindow` 装上 `+alloc`（只在主线程调用）。tao 的窗口类要等它建过第一个窗口才存在
/// （主窗口在启动时就建了，轮到浮层时一定已经有了）；万一还没有，这次不装，下次再试。
fn alloc_hook_installed() -> bool {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.get().is_some() {
        return true;
    }
    let Some(tao) = AnyClass::get(c"TaoWindow") else {
        return false;
    };
    let (Some(method), Some(_)) = (tao.class_method(sel!(alloc)), panel_class()) else {
        return false;
    };
    // SAFETY: `+alloc` 的签名就是 AllocFn；类方法加在元类上，签名串沿用原方法的。
    let added = unsafe {
        let original: AllocFn = std::mem::transmute::<Imp, AllocFn>(method.implementation());
        let _ = ORIGINAL_ALLOC.set(original);
        let hook: Imp = std::mem::transmute::<AllocFn, Imp>(alloc_window);
        objc2::ffi::class_addMethod(
            std::ptr::from_ref(tao.metaclass()).cast_mut(),
            sel!(alloc),
            hook,
            objc2::ffi::method_getTypeEncoding(method),
        )
        .as_bool()
    };
    if added {
        let _ = INSTALLED.set(());
    }
    added
}

fn with_window<R: Send>(
    window: &WebviewWindow,
    f: impl FnOnce(&NSWindow, MainThreadMarker) -> R + Send,
) -> Option<R> {
    let window = window.clone();
    on_main(move |mtm| ns_window(&window, mtm).map(|ns| f(&ns, mtm)))
}

pub fn native_handle(window: &WebviewWindow) -> AppResult<u64> {
    with_window(window, |ns, _| {
        encode_handle(own_pid(), ns.windowNumber() as u32)
    })
    .ok_or_else(|| AppError::msg("窗口已关闭"))
}

/// 句柄 → 本进程的 NSWindow。
pub fn window_of_handle(handle: u64, mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    NSApplication::sharedApplication(mtm)
        .windowWithWindowNumber(super::util::handle_window(handle) as isize)
}

/// 建一个浮层窗口：能浮在别的程序（包括全屏的）上面，拿键盘焦点时不把应用切到前台。
pub fn build_floating(
    builder: WebviewWindowBuilder<'_, tauri::Wry, AppHandle>,
    kind: FloatingKind,
) -> AppResult<WebviewWindow> {
    // 建窗在主线程上是同步完成的，标记只对紧接着的这一次分配有效
    let armed = MainThreadMarker::new().is_some() && alloc_hook_installed();
    PANEL_NEXT.set(armed);
    let built = builder.build();
    PANEL_NEXT.set(false);
    let window = built?;

    let label = window.label().to_string();
    with_window(&window, move |ns, _| {
        if is_panel(ns) {
            ns.setStyleMask(ns.styleMask() | NSWindowStyleMask::NonactivatingPanel);
            ns.setHidesOnDeactivate(false);
            tracing::debug!(label, "浮层已建成面板");
        } else {
            tracing::warn!(label, "浮层没能建成面板，收键盘时会把应用切到前台");
        }
        ns.setAnimationBehavior(NSWindowAnimationBehavior::None);
        let mut behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle;
        match kind {
            FloatingKind::Overlay => {
                behavior |= NSWindowCollectionBehavior::Stationary;
                ns.setLevel(OVERLAY_LEVEL);
            }
            FloatingKind::Panel => ns.setLevel(PANEL_LEVEL),
        }
        ns.setCollectionBehavior(behavior);
    });
    Ok(window)
}

/// 窗口上有属性监听时对象的类是 KVO 的动态子类，所以要问"是不是这个类或它的子类"，不能比类名。
fn is_panel(ns: &NSWindow) -> bool {
    panel_class().is_some_and(|class| ns.isKindOfClass(class))
}

/// 浮层盖不盖在程序坞上面：程序坞的窗口层级比普通浮层高，要盖住它得再往上提一档。
pub fn set_above_dock(window: &WebviewWindow, above: bool) {
    with_window(window, move |ns, _| {
        ns.setLevel(if above { ABOVE_DOCK_LEVEL } else { PANEL_LEVEL });
    });
}

/// 浮层拿键盘焦点时应用并不在前台（系统的"前台窗口"还是别的程序的），所以看的是它是不是焦点窗口。
pub fn is_foreground(window: &WebviewWindow) -> bool {
    with_window(window, |ns, _| ns.isKeyWindow()).unwrap_or(false)
}

/// 把键盘焦点交给窗口。面板不激活应用；普通窗口照常把应用切到前台。
pub fn take_focus(window: &WebviewWindow) -> AppResult<()> {
    let label = window.label().to_string();
    let panel = with_window(window, move |ns, mtm| {
        let panel = is_panel(ns);
        if panel {
            ns.makeKeyAndOrderFront(None);
            let app_active = NSApplication::sharedApplication(mtm).isActive();
            if ns.isKeyWindow() {
                tracing::debug!(label, app_active, "浮层拿到键盘焦点");
            } else {
                tracing::warn!(label, app_active, "浮层没拿到键盘焦点，按键可能没反应");
            }
        }
        panel
    })
    .ok_or_else(|| AppError::msg("窗口已关闭"))?;
    if !panel {
        window.set_focus()?;
    }
    Ok(())
}

pub fn set_exclude_from_capture(window: &WebviewWindow, exclude: bool) {
    with_window(window, move |ns, _| {
        ns.setSharingType(if exclude {
            NSWindowSharingType::None
        } else {
            NSWindowSharingType::ReadOnly
        });
    });
}

/// 诊断开关：`CHENOCR_ALLOW_SELF_CAPTURE=1` 时，浮动窗口**显示期间**不排除抓屏，
/// 好让 `screencapture` 之类的外部工具拍到它们做视觉验证。
pub fn self_capture_allowed() -> bool {
    std::env::var("CHENOCR_ALLOW_SELF_CAPTURE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn reveal_for_tests(window: &WebviewWindow, visible: bool) {
    if self_capture_allowed() {
        set_exclude_from_capture(window, !visible);
    }
}

/// 点它、显示它都不抢键盘焦点：面板只在里面有输入框需要时才成为焦点窗口。
/// 悬浮按钮上的点击另由划词监听吞掉（`selection_hook`）。
pub fn set_no_activate(window: &WebviewWindow) {
    with_window(window, |ns, _| {
        if is_panel(ns) {
            // SAFETY: 已确认对象的类是 NSPanel 的子类。
            let panel: &NSPanel = unsafe { &*std::ptr::from_ref(ns).cast() };
            panel.setBecomesKeyOnlyIfNeeded(true);
        }
    });
}

/// 系统圆角、窗口裁剪都是 Windows 的做法；macOS 上面板的圆角由毛玻璃背板自己裁（见 `set_backdrop`）。
pub fn set_rounded(_window: &WebviewWindow, _rounded: bool) {}

pub fn set_round_region(_window: &WebviewWindow, _radius: u32) {}

// ───────────────────────── 浮动面板的毛玻璃背板 ─────────────────────────
//
// 面板窗口本身全透明，毛玻璃是垫在网页下面的一块 NSVisualEffectView。圆角半径用户可调；面板在窗口里
// 展开 / 收起时背板只铺窗口的一部分，还要跟着页面一起做动画 —— Tauri 的 `set_effects` 建的那块视图
// 永远铺满窗口，所以这块视图自己管。

/// 背板视图的标识，靠它在内容视图的子视图里把背板找出来
const BACKDROP_ID: &str = "DatoOcrBackdrop";

fn find_backdrop(content: &NSView) -> Option<Retained<NSVisualEffectView>> {
    let id = NSString::from_str(BACKDROP_ID);
    content.subviews().iter().find_map(|view| {
        let ours = view.identifier().is_some_and(|i| i.isEqualToString(&id));
        ours.then(|| view.downcast::<NSVisualEffectView>().ok())
            .flatten()
    })
}

fn fill_mask() -> NSAutoresizingMaskOptions {
    NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable
}

/// 毛玻璃背板：`Some(半径)`（物理像素）挂上或者更新半径，`None` 去掉。
pub fn set_backdrop(window: &WebviewWindow, radius: Option<u32>) -> bool {
    with_window(window, move |ns, mtm| {
        let Some(content) = ns.contentView() else {
            return false;
        };
        let existing = find_backdrop(&content);
        let Some(radius) = radius else {
            if let Some(view) = existing {
                view.removeFromSuperview();
            }
            return true;
        };
        let view = existing.unwrap_or_else(|| {
            let view = NSVisualEffectView::initWithFrame(mtm.alloc(), content.bounds());
            view.setIdentifier(Some(&NSString::from_str(BACKDROP_ID)));
            view.setMaterial(NSVisualEffectMaterial::Popover);
            view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            // 面板不是应用的主窗口，跟着窗口激活状态走的话材质一直是"没激活"的灰
            view.setState(NSVisualEffectState::Active);
            view.setAutoresizingMask(fill_mask());
            view.setWantsLayer(true);
            content.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, None);
            view
        });
        if let Some(layer) = view.layer() {
            layer.setCornerRadius(f64::from(radius) / ns.backingScaleFactor().max(1.0));
            layer.setMasksToBounds(true);
        }
        true
    })
    .unwrap_or(false)
}

/// 窗口大小变了：背板靠自动布局跟着变，不用做事。
pub fn resize_backdrop(_window: &WebviewWindow) -> bool {
    true
}

/// 背板只铺在窗口里的一块（物理像素 x, y, w, h，原点在窗口左上角），`ms` > 0 时用和页面一样的
/// 缓动曲线动画过去；`None` = 恢复铺满整个窗口。
pub fn set_backdrop_rect(window: &WebviewWindow, rect: Option<(f32, f32, f32, f32)>, ms: u32) {
    with_window(window, move |ns, _| {
        let Some(content) = ns.contentView() else {
            return;
        };
        let Some(view) = find_backdrop(&content) else {
            return;
        };
        let bounds = content.bounds();
        let flipped = content.isFlipped();
        let (frame, mask) = match rect {
            Some((x, y, w, h)) => {
                let s = ns.backingScaleFactor().max(1.0);
                let (x, y) = (f64::from(x) / s, f64::from(y) / s);
                let (w, h) = ((f64::from(w) / s).max(1.0), (f64::from(h) / s).max(1.0));
                // 视图的原点默认在左下角
                let y = if flipped {
                    y
                } else {
                    bounds.size.height - y - h
                };
                // 窗口变大小时这一块离窗口左上角的距离和大小都不变（范围由页面管）
                let below = if flipped {
                    NSAutoresizingMaskOptions::ViewMaxYMargin
                } else {
                    NSAutoresizingMaskOptions::ViewMinYMargin
                };
                (
                    geometry::rect(x, y, w, h),
                    NSAutoresizingMaskOptions::ViewMaxXMargin | below,
                )
            }
            None => (bounds, fill_mask()),
        };
        view.setAutoresizingMask(mask);
        if ms == 0 {
            if let Some(layer) = view.layer() {
                layer.removeAllAnimations();
            }
            view.setFrame(frame);
            return;
        }
        NSAnimationContext::beginGrouping();
        let ctx = NSAnimationContext::currentContext();
        ctx.setDuration(f64::from(ms) / 1000.0);
        // 和页面里的 cubic-bezier(0.32, 0.72, 0, 1) 一样
        ctx.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
            0.32, 0.72, 0.0, 1.0,
        )));
        view.animator().setFrame(frame);
        NSAnimationContext::endGrouping();
    });
}

// ───────────────────────── 窗口只留一块 ─────────────────────────
//
// macOS 没有"窗口区域"。窗口本来就是全透明的，留出来的那一截页面不画东西、背板也不铺，看不见；
// 要补的只有"点击穿过去"。做法：盯着鼠标 —— 鼠标在窗口里但不在保留的那一块上，就让整个窗口
// 不收鼠标事件，回到那一块上再恢复。

/// 保留的那一块：离窗口左上角的距离 + 大小（点）
type Region = (f64, f64, f64, f64);

thread_local! {
    /// 窗口号 → 保留的那一块
    static REGIONS: RefCell<HashMap<isize, Region>> = RefCell::new(HashMap::new());
    static REGION_TIMER: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
    /// 连续多少拍没有一个带区域的窗口是显示着的
    static REGION_IDLE: Cell<u32> = const { Cell::new(0) };
}

const REGION_INTERVAL: f64 = 1.0 / 30.0;
/// 带区域的窗口都藏起来这么多拍之后停表；下次设区域（面板每次弹出都会设）再开
const REGION_IDLE_TICKS: u32 = 60;

/// 把窗口限制在一个矩形里（物理像素，相对窗口左上角）：外面的部分点击直接穿过去。`None` = 整个窗口。
pub fn set_rect_region(window: &WebviewWindow, rect: Option<(i32, i32, i32, i32)>) {
    with_window(window, move |ns, mtm| {
        let number = ns.windowNumber();
        match rect {
            Some((x, y, w, h)) => {
                let s = ns.backingScaleFactor().max(1.0);
                let region = (
                    f64::from(x) / s,
                    f64::from(y) / s,
                    f64::from(w) / s,
                    f64::from(h) / s,
                );
                REGIONS.with_borrow_mut(|r| r.insert(number, region));
                REGION_IDLE.set(0);
                watch_regions();
                region_tick(mtm);
            }
            None => {
                if REGIONS.with_borrow_mut(|r| r.remove(&number)).is_some() {
                    ns.setIgnoresMouseEvents(false);
                }
            }
        }
    });
}

fn watch_regions() {
    if REGION_TIMER.with_borrow(Option::is_some) {
        return;
    }
    let tick = RcBlock::new(|_: NonNull<NSTimer>| {
        // SAFETY: 定时器挂在主线程的运行循环上，回调在主线程。
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        if !region_tick(mtm) {
            if let Some(timer) = REGION_TIMER.take() {
                timer.invalidate();
            }
        }
    });
    // SAFETY: 回调只碰主线程的数据。
    let timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(REGION_INTERVAL, true, &tick)
    };
    REGION_TIMER.set(Some(timer));
}

/// 按鼠标位置更新每个带区域的窗口收不收鼠标事件。返回 false = 没什么可盯的了，可以停表。
fn region_tick(mtm: MainThreadMarker) -> bool {
    let mouse = NSEvent::mouseLocation();
    // 按着鼠标（拖窗口、拖边改大小）的时候不动，免得把正在进行的拖动掐断
    let dragging = NSEvent::pressedMouseButtons() != 0;
    let app = NSApplication::sharedApplication(mtm);
    let mut visible = false;
    let watching = REGIONS.with_borrow_mut(|regions| {
        regions.retain(|&number, &mut (x, y, w, h)| {
            let Some(ns) = app.windowWithWindowNumber(number) else {
                return false;
            };
            if !ns.isVisible() {
                return true;
            }
            visible = true;
            if dragging {
                return true;
            }
            let frame = ns.frame();
            let inside = |left: f64, bottom: f64, w: f64, h: f64| {
                mouse.x >= left && mouse.x < left + w && mouse.y >= bottom && mouse.y < bottom + h
            };
            let in_window = inside(
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            );
            // 屏幕坐标的原点在左下角
            let top = frame.origin.y + frame.size.height;
            let in_region = inside(frame.origin.x + x, top - y - h, w, h);
            let ignore = in_window && !in_region;
            if ns.ignoresMouseEvents() != ignore {
                ns.setIgnoresMouseEvents(ignore);
            }
            true
        });
        !regions.is_empty()
    });
    let idle = if visible { 0 } else { REGION_IDLE.get() + 1 };
    REGION_IDLE.set(idle);
    watching && idle < REGION_IDLE_TICKS
}

thread_local! {
    /// 刚建好的窗口：下一轮事件循环还要再摆一次的位置（见 `place_window`）
    static PENDING_FRAME: RefCell<HashMap<isize, CGRect>> = RefCell::new(HashMap::new());
}

/// 外框左上角 + 内容区大小（全局点坐标）→ 系统要的窗口外框。
fn frame_for(ns: &NSWindow, points: CGRect) -> CGRect {
    let content = geometry::flip(points);
    let frame = ns.frameRectForContentRect(content);
    // 标题栏叠在内容上的窗口，内容区就是整个外框；按左上角对齐
    geometry::rect(
        frame.origin.x,
        content.origin.y + content.size.height - frame.size.height,
        frame.size.width,
        frame.size.height,
    )
}

/// 面板的滑入滑出在 macOS 上是挪窗口本身：毛玻璃背板是窗口的一部分，页面里的动画带不动它。
pub const SLIDES_WINDOWS: bool = true;

/// 把窗口从现在的位置滑到 `to`（屏幕物理像素）。
pub fn slide_window(window: &WebviewWindow, to: PhysicalRect, ms: u32) {
    let points = geometry::rect_to_points(to);
    with_window(window, move |ns, _| {
        let frame = frame_for(ns, points);
        NSAnimationContext::beginGrouping();
        let ctx = NSAnimationContext::currentContext();
        ctx.setDuration(f64::from(ms) / 1000.0);
        // 和页面里面板用的缓动（--cn-ease-sheet）一样
        ctx.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
            0.32, 0.72, 0.0, 1.0,
        )));
        ns.animator().setFrame_display(frame, true);
        NSAnimationContext::endGrouping();
    });
}

/// 应用里选了浅色 / 深色时，原生层（毛玻璃材质、菜单）也得是那个外观，不然系统是深色、应用选了浅色，
/// 毛玻璃还是深色的，上面压着浅色主题的深色字，看不清。选"跟随系统"就交还给系统。
pub fn apply_theme(app: &AppHandle, theme: &str) {
    app.set_theme(match theme {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => None,
    });
}

/// `points`：外框左上角 + 内容区大小，全局点坐标。
fn set_frame(ns: &NSWindow, points: CGRect) {
    // 还有一次没执行的"再摆一次"：让它摆到最新要的位置，别把这次的盖回去
    PENDING_FRAME.with_borrow_mut(|pending| {
        if let Some(slot) = pending.get_mut(&ns.windowNumber()) {
            *slot = points;
        }
    });
    ns.setFrame_display(frame_for(ns, points), true);
}

/// 一次同时改位置和大小（屏幕物理像素），同步生效。
/// Tauri 自己的 `set_position` / `set_size` 在 macOS 上是排到下一轮事件循环才执行的，
/// 紧跟着 `show()` 的话窗口会先在旧位置闪一下。
pub fn set_bounds(
    window: &WebviewWindow,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> AppResult<()> {
    let points = geometry::rect_to_points(PhysicalRect::new(x, y, width, height));
    with_window(window, move |ns, _| set_frame(ns, points))
        .ok_or_else(|| AppError::msg("窗口已关闭"))
}

pub fn place_window(
    window: &WebviewWindow,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> AppResult<()> {
    // 刚建好的窗口：tao 把建窗时给的位置排在下一轮事件循环才应用，会盖掉这里刚摆好的。
    // 上层给它的位置是按"物理 ÷ 本屏缩放"算的，在缩放不同的副屏上并不对，所以下一轮再摆一次
    // （这中间要是又有人挪了窗口，摆的是最新要的那个位置，见 `set_frame`）
    let points = geometry::rect_to_points(PhysicalRect::new(x, y, width, height));
    with_window(window, move |ns, _| {
        set_frame(ns, points);
        PENDING_FRAME.with_borrow_mut(|pending| pending.insert(ns.windowNumber(), points));
    })
    .ok_or_else(|| AppError::msg("窗口已关闭"))?;
    let window = window.clone();
    super::util::on_main_async(move |mtm| {
        let Some(ns) = ns_window(&window, mtm) else {
            return;
        };
        let latest = PENDING_FRAME.with_borrow_mut(|pending| pending.remove(&ns.windowNumber()));
        if let Some(points) = latest {
            set_frame(&ns, points);
        }
    });
    Ok(())
}

/// 同上，单位是逻辑像素（前端 `window.screenX` 那一套，在 macOS 上就是全局点坐标）。
pub fn set_bounds_logical(
    window: &WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> AppResult<()> {
    let points = geometry::rect(x, y, width.max(1.0), height.max(1.0));
    with_window(window, move |ns, _| set_frame(ns, points))
        .ok_or_else(|| AppError::msg("窗口已关闭"))
}

pub fn hide_window(window: &WebviewWindow) {
    with_window(window, |ns, _| ns.orderOut(None));
}

/// 显示在最前但不拿键盘焦点（toast、划词悬浮按钮）。
pub fn show_without_activate(window: &WebviewWindow) -> AppResult<()> {
    with_window(window, |ns, _| ns.orderFrontRegardless())
        .ok_or_else(|| AppError::msg("窗口已关闭"))
}

/// "正经窗口"（主窗口之外的识字、编辑、AI 窗口）的外框：用系统标题栏，红黄绿灯叠在页面上。
pub fn frame_window<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    builder
        .decorations(true)
        .title_bar_style(TitleBarStyle::Overlay)
        .hidden_title(true)
        // 页面自己画的标题栏高 44，红黄绿灯在里面垂直居中
        .traffic_light_position(LogicalPosition::new(16.0, 24.0))
}

/// 主窗口这类窗口的系统材质：侧边栏那种半透明模糊。
pub fn window_effects(glass: bool) -> Option<WindowEffectsConfig> {
    glass.then(|| {
        EffectsBuilder::new()
            .effect(Effect::Sidebar)
            .state(EffectState::FollowsWindowActiveState)
            .build()
    })
}
