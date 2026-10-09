//! 划词监听：认出"像是刚选了一段文字"的鼠标手势。
//!
//! 判定和 Windows 一样：左键拖动超过一小段距离，或双击 / 三击；并且按下或松开时光标是
//! 文本光标（I 形）。按住 ⌥ / ⌃ 选字是明确的意图，不要求 I 形光标。
//!
//! 用 `NSEvent` 的全局监听（别的应用里的鼠标事件，只读，不需要权限）加本地监听（落在我们
//! 自己窗口上的）。悬浮按钮上的点击在本地监听里直接吞掉：按钮是不抢焦点的面板，被吞的点击
//! 也到不了页面，原程序的焦点和选区都还在，上层随后模拟 ⌘C 就能取到文字。

use std::ptr::NonNull;
use std::sync::mpsc::Sender;

use block2::RcBlock;
use objc2_app_kit::{NSCursor, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use parking_lot::Mutex;

use super::ffi;
use super::geometry;
use super::hook::Monitors;
use super::util::on_main;
use crate::error::AppResult;
use crate::platform::{PhysicalRect, SelectionEvent};

/// 拖动距离达到这么多逻辑像素才算"拖选"
const DRAG_MIN_POINTS: f64 = 6.0;

struct Down {
    at: (i32, i32),
    ibeam: bool,
    alt: bool,
    ctrl: bool,
    clicks: isize,
    /// 这块屏上"拖选"的最小物理像素距离
    drag_min: i32,
}

struct HookState {
    tx: Option<Sender<SelectionEvent>>,
    down: Option<Down>,
    /// 悬浮按钮当前的屏幕矩形；None = 没显示
    button: Option<PhysicalRect>,
    /// 吞掉了按钮上的按下，对应的抬起也要吞
    swallow_up: bool,
}

static STATE: Mutex<HookState> = Mutex::new(HookState {
    tx: None,
    down: None,
    button: None,
    swallow_up: false,
});

pub struct Guard {
    monitors: Option<Monitors>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(monitors) = self.monitors.take() {
            monitors.remove();
        }
        let mut st = STATE.lock();
        st.tx = None;
        st.down = None;
        st.button = None;
    }
}

pub fn install(tx: Sender<SelectionEvent>) -> AppResult<Guard> {
    STATE.lock().tx = Some(tx);
    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseUp
        | NSEventMask::RightMouseDown
        | NSEventMask::OtherMouseDown
        | NSEventMask::ScrollWheel;
    let monitors = on_main(move |_| {
        let mut list = Vec::new();
        let global = RcBlock::new(|event: NonNull<NSEvent>| {
            // SAFETY: 回调期间事件对象有效。
            handle(unsafe { event.as_ref() });
        });
        if let Some(m) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global) {
            list.push(m);
        }
        let local = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: 同上。
            if handle(unsafe { event.as_ref() }) {
                std::ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
        // SAFETY: 回调只在主线程被调用，返回的要么是原事件要么是空。
        if let Some(m) =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local) }
        {
            list.push(m);
        }
        Monitors(list)
    });
    Ok(Guard {
        monitors: Some(monitors),
    })
}

/// 悬浮按钮显示 / 隐藏时告诉监听它在哪，好吞掉按钮上的点击。
pub fn set_button_rect(rect: Option<PhysicalRect>) {
    let mut st = STATE.lock();
    st.button = rect;
    if rect.is_none() {
        st.swallow_up = false;
    }
}

/// 系统当前的光标像不像 I 形：热点在正中间，而且不是方方正正的大十字。只在问不到系统"有没有选中文字"时用。
///
/// 拿不到别的应用的光标对象本身，只能看图片的样子。以前是和 `NSCursor.IBeamCursor` 比热点和大小，
/// macOS 26 起系统光标不再带图片（大小、热点都是 0），那个比较永远不成立 —— 表现就是选了字按钮
/// 不出来，只有按住修饰键（不查光标）才出来。macOS 26 的 I 形光标是 23×22、热点 12,11；箭头的热点在左上角。
fn is_ibeam_cursor() -> bool {
    #[allow(deprecated)]
    let Some(current) = NSCursor::currentSystemCursor() else {
        return false;
    };
    let (size, hot) = (current.image().size(), current.hotSpot());
    if size.width <= 0.0 || size.height <= 0.0 {
        return false;
    }
    (hot.x / size.width - 0.5).abs() < 0.2 && (hot.y / size.height - 0.5).abs() < 0.2
}

/// 当前光标的大小和热点，写日志用（按钮该出不出时看是哪一步没认出来）。
fn cursor_shape() -> String {
    #[allow(deprecated)]
    match NSCursor::currentSystemCursor() {
        Some(c) => {
            let (size, hot) = (c.image().size(), c.hotSpot());
            format!(
                "{:.0}x{:.0} 热点 {:.0},{:.0}",
                size.width, size.height, hot.x, hot.y
            )
        }
        None => "拿不到".into(),
    }
}

/// 问系统：当前有焦点的那个控件里是不是选中了文字。要「辅助功能」权限（取选中的字本来也要）；
/// 没权限、对方程序不支持（有的浏览器、自绘界面）时返回 None，由光标的样子说了算。
fn has_selected_text() -> Option<bool> {
    use objc2_foundation::NSString;
    // SAFETY: 都是 CoreFoundation / 辅助功能的只读查询；拿到的对象逐个释放。
    unsafe {
        if !ffi::AXIsProcessTrusted() {
            return None;
        }
        let attr = |name: &str| NSString::from_str(name);
        let system = ffi::AXUIElementCreateSystemWide();
        if system.is_null() {
            return None;
        }
        // 对方程序卡住时别跟着卡
        ffi::AXUIElementSetMessagingTimeout(system, 0.25);
        let mut focused: ffi::CFTypeRef = std::ptr::null();
        let name = attr("AXFocusedUIElement");
        let err = ffi::AXUIElementCopyAttributeValue(
            system,
            std::ptr::from_ref::<NSString>(&name).cast(),
            &mut focused,
        );
        ffi::CFRelease(system);
        if err != 0 || focused.is_null() {
            return None;
        }
        let mut text: ffi::CFTypeRef = std::ptr::null();
        let name = attr("AXSelectedText");
        let err = ffi::AXUIElementCopyAttributeValue(
            focused,
            std::ptr::from_ref::<NSString>(&name).cast(),
            &mut text,
        );
        ffi::CFRelease(focused);
        if err != 0 || text.is_null() {
            return None;
        }
        let selected = (ffi::CFGetTypeID(text) == ffi::CFStringGetTypeID())
            .then(|| ffi::CFStringGetLength(text) > 0);
        ffi::CFRelease(text);
        selected
    }
}

fn send(st: &HookState, event: SelectionEvent) {
    if let Some(tx) = st.tx.as_ref() {
        let _ = tx.send(event);
    }
}

/// 返回 true = 吞掉这个事件（只对落在我们自己窗口上的事件有效）。
fn handle(event: &NSEvent) -> bool {
    let kind = event.r#type();
    let screens = geometry::screens();
    let location = geometry::flip_point(NSEvent::mouseLocation());
    let pt = geometry::point_to_physical_in(&screens, location);
    let flags = event.modifierFlags();
    let alt = flags.contains(NSEventModifierFlags::Option);
    let ctrl = flags.contains(NSEventModifierFlags::Control);

    let mut st = STATE.lock();
    if kind == NSEventType::LeftMouseDown {
        if st.button.is_some_and(|r| r.contains_point(pt.0, pt.1)) {
            st.swallow_up = true;
            st.down = None;
            send(&st, SelectionEvent::ButtonClicked);
            return true;
        }
        if st.button.is_some() {
            send(&st, SelectionEvent::Dismiss);
        }
        send(&st, SelectionEvent::PointerDown);
        let scale = screens
            .iter()
            .find(|s| s.physical.contains_point(pt.0, pt.1))
            .map_or(1.0, |s| s.scale);
        st.down = Some(Down {
            at: pt,
            ibeam: is_ibeam_cursor(),
            alt,
            ctrl,
            clicks: event.clickCount(),
            drag_min: (DRAG_MIN_POINTS * scale).round() as i32,
        });
    } else if kind == NSEventType::LeftMouseUp {
        if st.swallow_up {
            st.swallow_up = false;
            return true;
        }
        let Some(down) = st.down.take() else {
            return false;
        };
        let dragged = (down.at.0 - pt.0).abs().max((down.at.1 - pt.1).abs()) >= down.drag_min;
        if !dragged && down.clicks < 2 {
            return false;
        }
        let (alt, ctrl) = (down.alt || alt, down.ctrl || ctrl);
        let (x0, x1) = (down.at.0.min(pt.0), down.at.0.max(pt.0));
        let (y0, y1) = (down.at.1.min(pt.1), down.at.1.max(pt.1));
        let selected = SelectionEvent::Selected {
            anchor: PhysicalRect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32),
            end: pt,
            alt,
            ctrl,
        };
        let ibeam = down.ibeam || is_ibeam_cursor();
        if alt || ctrl {
            send(&st, selected);
        } else if let Some(tx) = st.tx.clone() {
            // 直接问系统有没有选中文字，比猜光标准。要跨进程问，放到别的线程，稍等一下让对方把选区更新完。
            // 问不到（没有「辅助功能」权限、对方程序不支持）才看光标像不像 I 形
            let cursor = cursor_shape();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(80));
                let asked = has_selected_text();
                let show = asked.unwrap_or(ibeam);
                tracing::debug!(?asked, ibeam, %cursor, show, "划词手势");
                if show {
                    let _ = tx.send(selected);
                }
            });
        }
    } else {
        // 右键、中键、滚轮：收起按钮
        st.down = None;
        if kind != NSEventType::ScrollWheel {
            send(&st, SelectionEvent::PointerDown);
        }
        if st.button.is_some() {
            send(&st, SelectionEvent::Dismiss);
        }
    }
    false
}
