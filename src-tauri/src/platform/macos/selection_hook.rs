//! 划词监听：认出"像是刚选了一段文字"的鼠标手势。
//!
//! 判定和 Windows 一样：左键拖动超过一小段距离，或双击 / 三击；并且按下或松开时光标是
//! 文本光标（I 形）。按住 ⌥ / ⌃ 选字是明确的意图，不要求 I 形光标。
//!
//! 用 `NSEvent` 的全局监听（别的应用里的鼠标事件，只读，不需要权限）加本地监听（落在我们
//! 自己窗口上的）。悬浮按钮上的点击在本地监听里直接吞掉：按钮是不抢焦点的面板，被吞的点击
//! 也到不了页面，原程序的焦点和选区都还在，上层随后模拟 ⌘C 就能取到文字。

use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// 每按下一次左键加一：松开后隔一小会儿才上报"选了字"，这期间又按下了（双击接三击）就不报了，
/// 免得上一次的按钮在新的一次选字中途冒出来。
static PRESS: AtomicU64 = AtomicU64::new(0);

/// 问系统"选中了什么字"的结果
#[derive(Debug)]
enum Selection {
    /// 选中了这段字
    Text(String),
    /// 对方回答了：没选中（或者只选中了空白）
    Empty,
    /// 问不到：没权限、对方程序不回答（这时才看光标的样子）
    Unknown,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> ffi::CFTypeRef;
    fn AXUIElementCopyParameterizedAttributeValue(
        element: ffi::CFTypeRef,
        attribute: ffi::CFTypeRef,
        parameter: ffi::CFTypeRef,
        value: *mut ffi::CFTypeRef,
    ) -> i32;
}

/// 读一个属性（拿到的要自己释放）。不支持、没值都是 None
unsafe fn copy_attr(element: ffi::CFTypeRef, name: &str) -> Option<ffi::CFTypeRef> {
    let name = objc2_foundation::NSString::from_str(name);
    let mut value: ffi::CFTypeRef = std::ptr::null();
    let err = ffi::AXUIElementCopyAttributeValue(
        element,
        std::ptr::from_ref::<objc2_foundation::NSString>(&name).cast(),
        &mut value,
    );
    if err == 0 && !value.is_null() {
        Some(value)
    } else {
        None
    }
}

/// CFString 转成 Rust 的字符串并释放；不是字符串给 None
unsafe fn take_string(value: ffi::CFTypeRef) -> Option<String> {
    let text = (ffi::CFGetTypeID(value) == ffi::CFStringGetTypeID())
        .then(|| (*value.cast::<objc2_foundation::NSString>()).to_string());
    ffi::CFRelease(value);
    text
}

/// 一个元素里选中的字：先看普通控件的 AXSelectedText，再看网页的选区（AXSelectedTextMarkerRange，
/// 网页正文里选的字普通的那个问不到）。None = 这个元素两样都不支持
unsafe fn selection_of(element: ffi::CFTypeRef) -> Option<String> {
    if let Some(text) = copy_attr(element, "AXSelectedText").and_then(|v| take_string(v)) {
        if !text.trim().is_empty() {
            return Some(text);
        }
        // 普通控件说"没选中"，网页那条路再问一下（网页的输入框两样都支持，正文只支持后一样）
        return Some(web_selection(element).unwrap_or(text));
    }
    web_selection(element)
}

unsafe fn web_selection(element: ffi::CFTypeRef) -> Option<String> {
    let range = copy_attr(element, "AXSelectedTextMarkerRange")?;
    let name = objc2_foundation::NSString::from_str("AXStringForTextMarkerRange");
    let mut value: ffi::CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyParameterizedAttributeValue(
        element,
        std::ptr::from_ref::<objc2_foundation::NSString>(&name).cast(),
        range,
        &mut value,
    );
    ffi::CFRelease(range);
    if err != 0 || value.is_null() {
        return Some(String::new());
    }
    take_string(value).or(Some(String::new()))
}

/// 问前台程序：当前有焦点的那个控件里选中的文字。要「辅助功能」权限（取选中的字本来也要）。
/// 焦点元素自己不支持的话，顺着父元素往上找几层（网页里焦点常落在正文里的某一段上，选区挂在整个网页上）。
fn selected_text() -> Selection {
    // SAFETY: 都是辅助功能的只读查询（第一次问到浏览器时打开它的辅助功能树除外）；拿到的对象逐个释放。
    unsafe {
        if !ffi::AXIsProcessTrusted() {
            return Selection::Unknown;
        }
        let Some(pid) = objc2_app_kit::NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier())
        else {
            return Selection::Unknown;
        };
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Selection::Unknown;
        }
        // 对方程序卡住时别跟着卡
        ffi::AXUIElementSetMessagingTimeout(app, 0.25);
        // 浏览器、Electron 程序默认不建辅助功能树，问不出选区；第一次问到时打开（和截图框选元素共用）
        super::elements::enable_tree(pid, app);
        let focused = copy_attr(app, "AXFocusedUIElement");
        ffi::CFRelease(app);
        let Some(mut current) = focused else {
            return Selection::Unknown;
        };
        let mut answer = None;
        for _ in 0..6 {
            if let Some(text) = selection_of(current) {
                answer = Some(text);
                break;
            }
            let Some(parent) = copy_attr(current, "AXParent") else {
                break;
            };
            ffi::CFRelease(current);
            current = parent;
        }
        ffi::CFRelease(current);
        match answer {
            Some(t) if !t.trim().is_empty() => Selection::Text(t),
            Some(_) => Selection::Empty,
            None => Selection::Unknown,
        }
    }
}

/// 选完以后隔多久问一次（毫秒，从松开鼠标算）：程序更新选区有快有慢，问到有字就停
const ASK_AT: [u64; 4] = [25, 90, 220, 400];

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
        PRESS.fetch_add(1, Ordering::SeqCst);
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
        let selected = move |text| SelectionEvent::Selected {
            anchor: PhysicalRect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32),
            end: pt,
            alt,
            ctrl,
            text,
        };
        let ibeam = down.ibeam || is_ibeam_cursor();
        if alt || ctrl {
            send(&st, selected(None));
        } else if let Some(tx) = st.tx.clone() {
            // 直接问系统有没有选中文字，比猜光标准。要跨进程问，放到别的线程，稍等一下让对方把选区更新完。
            // 问不到（没有「辅助功能」权限、对方程序不支持）才看光标像不像 I 形
            let cursor = cursor_shape();
            let press = PRESS.load(Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut waited = 0;
                let mut result = Selection::Unknown;
                for at in ASK_AT {
                    std::thread::sleep(std::time::Duration::from_millis(at - waited));
                    waited = at;
                    // 中途又按下了鼠标（双击接三击、或者点别处了）：这次不算
                    if PRESS.load(Ordering::SeqCst) != press {
                        return;
                    }
                    result = selected_text();
                    if matches!(result, Selection::Text(_)) {
                        break;
                    }
                }
                // 问到了就听它的；问不到（没权限、程序不回答）才看光标像不像 I 形
                let (show, text) = match result {
                    Selection::Text(t) => (true, Some(t)),
                    Selection::Empty => (false, None),
                    Selection::Unknown => (ibeam, None),
                };
                tracing::debug!(
                    asked = ?text.as_ref().map(|t| t.chars().count()),
                    ibeam,
                    %cursor,
                    show,
                    waited,
                    "划词手势"
                );
                if show && PRESS.load(Ordering::SeqCst) == press {
                    let _ = tx.send(selected(text));
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

/// 调试版：现在问前台程序"选中了什么"的结果
#[cfg(debug_assertions)]
pub fn probe() -> String {
    format!("{:?}", selected_text())
}
