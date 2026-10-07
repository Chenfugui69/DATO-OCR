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

/// 系统当前的光标是不是 I 形。拿不到别的应用的光标对象本身，只能比热点和图片大小。
fn is_ibeam_cursor() -> bool {
    #[allow(deprecated)]
    let Some(current) = NSCursor::currentSystemCursor() else {
        return false;
    };
    let ibeam = NSCursor::IBeamCursor();
    let (a, b) = (current.hotSpot(), ibeam.hotSpot());
    let (sa, sb) = (current.image().size(), ibeam.image().size());
    (a.x - b.x).abs() < 0.5
        && (a.y - b.y).abs() < 0.5
        && (sa.width - sb.width).abs() < 0.5
        && (sa.height - sb.height).abs() < 0.5
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
        if down.ibeam || alt || ctrl || is_ibeam_cursor() {
            let (x0, x1) = (down.at.0.min(pt.0), down.at.0.max(pt.0));
            let (y0, y1) = (down.at.1.min(pt.1), down.at.1.max(pt.1));
            send(
                &st,
                SelectionEvent::Selected {
                    anchor: PhysicalRect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32),
                    end: pt,
                    alt,
                    ctrl,
                },
            );
        }
    } else {
        // 右键、中键、滚轮：收起按钮
        st.down = None;
        if st.button.is_some() {
            send(&st, SelectionEvent::Dismiss);
        }
    }
    false
}
