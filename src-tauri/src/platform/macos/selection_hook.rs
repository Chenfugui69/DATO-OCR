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
use crate::platform::{PhysicalRect, SelectionEvent, TextBounds};

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
    /// 选中了这段字（问得到的话，还有它在屏幕上的范围）
    Text(String, Option<TextBounds>),
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

const AX_VALUE_CGRECT: u32 = 3;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXValueGetValue(value: ffi::CFTypeRef, kind: u32, out: *mut std::ffi::c_void) -> bool;
    fn AXValueCreate(kind: u32, value: *const std::ffi::c_void) -> ffi::CFTypeRef;
    fn AXTextMarkerRangeCopyStartMarker(range: ffi::CFTypeRef) -> ffi::CFTypeRef;
    fn CFNumberGetTypeID() -> usize;
    fn AXTextMarkerRangeCopyEndMarker(range: ffi::CFTypeRef) -> ffi::CFTypeRef;
}

/// 带参数地问一个属性（拿到的要自己释放）
unsafe fn param_attr(element: ffi::CFTypeRef, name: &str, parameter: ffi::CFTypeRef) -> Option<ffi::CFTypeRef> {
    let name = objc2_foundation::NSString::from_str(name);
    let mut value: ffi::CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyParameterizedAttributeValue(
        element,
        std::ptr::from_ref::<objc2_foundation::NSString>(&name).cast(),
        parameter,
        &mut value,
    );
    (err == 0 && !value.is_null()).then_some(value)
}

/// 屏幕范围（AXValue 里的 CGRect）读出来并释放；读不出、是空的给 None
unsafe fn take_rect(value: ffi::CFTypeRef) -> Option<ffi::CGRect> {
    let mut rect = geometry::rect(0.0, 0.0, 0.0, 0.0);
    let ok = AXValueGetValue(value, AX_VALUE_CGRECT, std::ptr::from_mut(&mut rect).cast());
    ffi::CFRelease(value);
    (ok && rect.size.width > 0.0 && rect.size.height > 0.0).then_some(rect)
}

/// 两个文字标记围出来的范围在屏幕上的框
unsafe fn marker_pair_rect(element: ffi::CFTypeRef, a: ffi::CFTypeRef, b: ffi::CFTypeRef) -> Option<ffi::CGRect> {
    use objc2::runtime::AnyObject;
    let items: [&AnyObject; 2] = [&*a.cast::<AnyObject>(), &*b.cast::<AnyObject>()];
    let list = objc2_foundation::NSArray::from_slice(&items);
    let Some(range) = param_attr(
        element,
        "AXTextMarkerRangeForUnorderedTextMarkers",
        std::ptr::from_ref::<objc2_foundation::NSArray<AnyObject>>(&list).cast(),
    ) else {
        tracing::debug!("划词：两个标记围不成范围");
        return None;
    };
    let rect = param_attr(element, "AXBoundsForTextMarkerRange", range).and_then(|v| take_rect(v));
    if rect.is_none() {
        tracing::debug!("划词：单字范围问不到屏幕位置");
    }
    ffi::CFRelease(range);
    rect
}

/// 看着像一个字的框：不能比两三个字宽、也不能高过两行（选区拖出段落时，末尾的"字"是段落之间的分隔，
/// 报回来的是整块的框）
fn char_like(r: &ffi::CGRect) -> bool {
    let (w, h) = (r.size.width, r.size.height);
    w > 0.0 && h > 0.0 && w <= h * 3.0 && h <= 120.0
}

/// 选区一头的那个字：从 `from` 起往里挪一个字围成范围问位置；问到的不像一个字（换行、段落分隔）
/// 就再往里挪一个，最多挪几次。`backward`：从结尾往前找（最后一个字），否则从开头往后找（第一个字）
unsafe fn edge_char(
    element: ffi::CFTypeRef,
    from: ffi::CFTypeRef,
    step: &str,
    backward: bool,
) -> Option<ffi::CGRect> {
    let mut outer = ffi::CFRetain(from);
    let mut found = None;
    for _ in 0..6 {
        let Some(inner) = param_attr(element, step, outer) else {
            break;
        };
        let rect = if backward {
            marker_pair_rect(element, inner, outer)
        } else {
            marker_pair_rect(element, outer, inner)
        };
        ffi::CFRelease(outer);
        outer = inner;
        if let Some(r) = rect.filter(char_like) {
            found = Some(r);
            break;
        }
    }
    ffi::CFRelease(outer);
    found
}

/// 文字标记在整页文字里的序号（问不到给 None）
unsafe fn marker_index(element: ffi::CFTypeRef, marker: ffi::CFTypeRef) -> Option<isize> {
    let v = param_attr(element, "AXIndexForTextMarker", marker)?;
    let n = (ffi::CFGetTypeID(v) == CFNumberGetTypeID())
        .then(|| (*v.cast::<objc2_foundation::NSNumber>()).as_isize());
    ffi::CFRelease(v);
    n
}

/// 网页（选区标记）：整段、第一个字、最后一个字
unsafe fn marker_bounds(
    element: ffi::CFTypeRef,
) -> Result<(ffi::CGRect, Option<ffi::CGRect>, Option<ffi::CGRect>), String> {
    let range = copy_attr(element, "AXSelectedTextMarkerRange").ok_or("标记:无")?;
    let whole = param_attr(element, "AXBoundsForTextMarkerRange", range).and_then(|v| take_rect(v));
    let Some(whole) = whole else {
        ffi::CFRelease(range);
        return Err("标记:问不到范围".into());
    };
    // 起点、终点：系统的公开函数直接从选区标记里拆（Chrome、Edge 不支持用带参数的查询取）
    let start = param_attr(element, "AXStartTextMarkerForTextMarkerRange", range).or_else(|| {
        let m = AXTextMarkerRangeCopyStartMarker(range);
        (!m.is_null()).then_some(m)
    });
    let end = param_attr(element, "AXEndTextMarkerForTextMarkerRange", range).or_else(|| {
        let m = AXTextMarkerRangeCopyEndMarker(range);
        (!m.is_null()).then_some(m)
    });
    ffi::CFRelease(range);
    // 从下往上选时 Chrome、Edge 报的起点是按下鼠标的地方（在后面）：按文字里的序号排一下，
    // 不然"最后一个字"会从选区上面那一段里找
    let (start, end) = match (start, end) {
        (Some(a), Some(b)) if marker_index(element, a).zip(marker_index(element, b)).is_some_and(|(ia, ib)| ia > ib) => {
            (Some(b), Some(a))
        }
        other => other,
    };
    let first = start.and_then(|s| edge_char(element, s, "AXNextTextMarkerForTextMarker", false));
    let last = end.and_then(|e| edge_char(element, e, "AXPreviousTextMarkerForTextMarker", true));
    tracing::debug!(
        start = start.is_some(),
        end = end.is_some(),
        first = first.is_some(),
        last = last.is_some(),
        "划词：网页选区逐步问的结果"
    );
    // Chrome、Edge、Electron 不支持取起点 / 终点标记，按序号问：选中范围（序号）→ 单个字的位置
    let (first, last) = if first.is_some() && last.is_some() {
        (first, last)
    } else {
        let (f, l) = index_char_bounds(element, whole);
        (first.or(f), last.or(l))
    };
    for m in [start, end].into_iter().flatten() {
        ffi::CFRelease(m);
    }
    Ok((whole, first, last))
}

/// 普通控件（选中范围）：整段、第一个字、最后一个字
unsafe fn range_bounds(
    element: ffi::CFTypeRef,
) -> Result<(ffi::CGRect, Option<ffi::CGRect>, Option<ffi::CGRect>), String> {
    let value = copy_attr(element, "AXSelectedTextRange").ok_or("范围:无")?;
    let mut range = CfRange { location: 0, length: 0 };
    let ok = AXValueGetValue(value, AX_VALUE_CFRANGE, std::ptr::from_mut(&mut range).cast());
    let whole = param_attr(element, "AXBoundsForRange", value).and_then(|v| take_rect(v));
    ffi::CFRelease(value);
    let whole = whole.ok_or("范围:问不到范围")?;
    let one = |location: isize| -> Option<ffi::CGRect> {
        let r = CfRange { location, length: 1 };
        let v = AXValueCreate(AX_VALUE_CFRANGE, std::ptr::from_ref(&r).cast());
        if v.is_null() {
            return None;
        }
        let rect = param_attr(element, "AXBoundsForRange", v).and_then(|b| take_rect(b)).filter(char_like);
        ffi::CFRelease(v);
        rect
    };
    if !ok || range.length <= 0 {
        return Ok((whole, None, None));
    }
    Ok((whole, one(range.location), one(range.location + range.length - 1)))
}

/// 按序号问选区第一个字、最后一个字的位置：AXSelectedTextRange 给出起点和长度，
/// 单个字先用 AXBoundsForRange 问，不行再换成文字标记（AXTextMarkerForIndex）问
unsafe fn index_char_bounds(element: ffi::CFTypeRef, whole: ffi::CGRect) -> (Option<ffi::CGRect>, Option<ffi::CGRect>) {
    // 单个字的框得落在整段的框里（Edge 用 AXBoundsForRange 问单个字，回的是整个网页的框）
    let inside = |r: &ffi::CGRect| {
        let t = 4.0;
        r.origin.x >= whole.origin.x - t
            && r.origin.y >= whole.origin.y - t
            && r.origin.x + r.size.width <= whole.origin.x + whole.size.width + t
            && r.origin.y + r.size.height <= whole.origin.y + whole.size.height + t
            && r.size.height < whole.size.height + t
            && char_like(r)
    };
    let Some(value) = copy_attr(element, "AXSelectedTextRange") else {
        tracing::debug!("划词：没有选中范围（序号）");
        return (None, None);
    };
    let mut range = CfRange { location: 0, length: 0 };
    let ok = AXValueGetValue(value, AX_VALUE_CFRANGE, std::ptr::from_mut(&mut range).cast());
    ffi::CFRelease(value);
    if !ok || range.length <= 0 {
        return (None, None);
    }
    let one = |at: isize| -> Option<ffi::CGRect> {
        let r = CfRange { location: at, length: 1 };
        let v = AXValueCreate(AX_VALUE_CFRANGE, std::ptr::from_ref(&r).cast());
        if !v.is_null() {
            let rect = param_attr(element, "AXBoundsForRange", v).and_then(|b| take_rect(b)).filter(inside);
            ffi::CFRelease(v);
            if rect.is_some() {
                return rect;
            }
        }
        let marker = |i: isize| {
            let n = objc2_foundation::NSNumber::new_isize(i);
            param_attr(element, "AXTextMarkerForIndex", std::ptr::from_ref::<objc2_foundation::NSNumber>(&n).cast())
        };
        let a = marker(at)?;
        let Some(b) = marker(at + 1) else {
            ffi::CFRelease(a);
            return None;
        };
        let rect = marker_pair_rect(element, a, b).filter(inside);
        ffi::CFRelease(a);
        ffi::CFRelease(b);
        rect
    };
    let first = one(range.location);
    let last = one(range.location + range.length - 1);
    tracing::debug!(
        location = range.location,
        length = range.length,
        first = first.is_some(),
        last = last.is_some(),
        "划词：按序号问单个字的位置"
    );
    (first, last)
}

#[repr(C)]
struct CfRange {
    location: isize,
    length: isize,
}
const AX_VALUE_CFRANGE: u32 = 4;

/// 选区在屏幕上的位置（左上原点的点坐标）：整段外框，和第一个字、最后一个字的框（按钮贴在选区的头、尾上）。
/// 网页（Chrome、Edge、Electron 程序）用选区标记问，普通控件用选中范围问；这个元素问不到就往父元素上找几层
/// （网页里回答选中文字的常是某一段，选区标记挂在整个网页上）。问不到给 None，日志里记下每一步
unsafe fn bounds_of(element: ffi::CFTypeRef) -> Option<(ffi::CGRect, Option<ffi::CGRect>, Option<ffi::CGRect>)> {
    let mut tried = Vec::new();
    let mut current = ffi::CFRetain(element);
    let mut found = None;
    for _ in 0..6 {
        match marker_bounds(current).or_else(|e| {
            tried.push(e);
            range_bounds(current)
        }) {
            Ok(b) => {
                found = Some(b);
                break;
            }
            Err(e) => tried.push(e),
        }
        let Some(parent) = copy_attr(current, "AXParent") else {
            break;
        };
        ffi::CFRelease(current);
        current = parent;
    }
    ffi::CFRelease(current);
    match &found {
        None => tracing::debug!(?tried, "划词：问不到选中文字在屏幕上的位置"),
        Some((_, first, last)) => {
            tracing::debug!(first = first.is_some(), last = last.is_some(), "划词：问到了选中文字的位置");
        }
    }
    found
}

/// 问前台程序：当前有焦点的那个控件里选中的文字。要「辅助功能」权限（取选中的字本来也要）。
/// 焦点元素自己不支持的话，顺着父元素往上找几层（网页里焦点常落在正文里的某一段上，选区挂在整个网页上）。
fn selected_text(screens: &[geometry::Screen]) -> Selection {
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
        let mut bounds = None;
        for _ in 0..6 {
            if let Some(text) = selection_of(current) {
                if !text.trim().is_empty() {
                    bounds = bounds_of(current).map(|(whole, first, last)| {
                        let to_px = |r| geometry::rect_to_physical_in(screens, r);
                        TextBounds { whole: to_px(whole), first: first.map(to_px), last: last.map(to_px) }
                    });
                }
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
            Some(t) if !t.trim().is_empty() => Selection::Text(t, bounds.filter(|b| b.whole.width > 0 && b.whole.height > 0)),
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
        let selected = move |text, text_rect| SelectionEvent::Selected {
            anchor: PhysicalRect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32),
            end: pt,
            alt,
            ctrl,
            text,
            text_rect,
        };
        let ibeam = down.ibeam || is_ibeam_cursor();
        if alt || ctrl {
            send(&st, selected(None, None));
        } else if let Some(tx) = st.tx.clone() {
            // 直接问系统有没有选中文字，比猜光标准。要跨进程问，放到别的线程，稍等一下让对方把选区更新完。
            // 问不到（没有「辅助功能」权限、对方程序不支持）才看光标像不像 I 形
            let cursor = cursor_shape();
            let press = PRESS.load(Ordering::SeqCst);
            let screens = screens.clone();
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
                    result = selected_text(&screens);
                    if matches!(result, Selection::Text(..)) {
                        break;
                    }
                }
                // 问到了就听它的；问不到（没权限、程序不回答）才看光标像不像 I 形
                let (show, text, text_rect) = match result {
                    Selection::Text(t, r) => (true, Some(t), r),
                    Selection::Empty => (false, None, None),
                    Selection::Unknown => (ibeam, None, None),
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
                    let _ = tx.send(selected(text, text_rect));
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
    format!("{:?}", selected_text(&geometry::screens()))
}
