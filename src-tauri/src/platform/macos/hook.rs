//! 长截图期间的全局输入监听（规格 03 §3.1）。
//!
//! 和 Windows 的低级钩子不同，这里分三路，尽量不依赖权限：
//!
//! - **滚轮**：`NSEvent` 的全局监听（别的应用收到的滚轮）加本地监听（我们自己窗口收到的）。
//!   只看鼠标事件不需要任何权限。
//! - **鼠标位置**：定时读（进出提示条区域时上层要开关鼠标穿透），比监听移动事件省事也可靠。
//! - **Enter / Esc / Backspace**：遮罩面板在长截图期间一直握着键盘焦点（macOS 的滚轮跟着鼠标走，
//!   不用把焦点让给被滚动的窗口），按键由本地监听收下并吞掉。用户中途点了别的窗口、焦点
//!   跑掉之后，本地监听就收不到了 —— 有「辅助功能」权限时另开一个事件拦截（CGEventTap）
//!   在全局兜住，没有权限就只有本地这一路。

use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
use parking_lot::Mutex;

use super::util::{on_main, on_main_async};
use super::{ffi, geometry, input, permissions};
use crate::error::AppResult;
use crate::platform::{HookEvent, HookKey};

static TX: Mutex<Option<Sender<HookEvent>>> = Mutex::new(None);
/// 事件拦截的端口，回调里被系统停用后要靠它重新启用
static TAP_PORT: AtomicUsize = AtomicUsize::new(0);

/// `NSEvent` 监听句柄。只在主线程上创建和移除，这里只是带着它跨线程。
pub(super) struct Monitors(pub Vec<Retained<AnyObject>>);

// SAFETY: 句柄只在主线程上被使用（见 `remove`）。
unsafe impl Send for Monitors {}

impl Monitors {
    pub fn remove(self) {
        on_main_async(move |_| {
            // 整个结构体一起带进闭包（它才是 Send 的，里面的句柄不是）
            let this = self;
            for monitor in this.0 {
                // SAFETY: 句柄是 addMonitor 返回的，移除一次。
                unsafe { NSEvent::removeMonitor(&monitor) };
            }
        });
    }
}

pub struct Guard {
    monitors: Option<Monitors>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(monitors) = self.monitors.take() {
            monitors.remove();
        }
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
        *TX.lock() = None;
    }
}

fn emit(event: HookEvent) {
    if let Some(tx) = TX.lock().as_ref() {
        let _ = tx.send(event);
    }
}

fn key_of(code: i64) -> Option<HookKey> {
    match code {
        ffi::kVK_Return | ffi::kVK_ANSI_KeypadEnter => Some(HookKey::Enter),
        ffi::kVK_Escape => Some(HookKey::Escape),
        ffi::kVK_Delete => Some(HookKey::Backspace),
        _ => None,
    }
}

fn emit_wheel(event: &NSEvent) {
    let (x, y) = geometry::point_to_physical(geometry::flip_point(NSEvent::mouseLocation()));
    emit(HookEvent::Wheel {
        delta: (event.scrollingDeltaY() * 10.0) as i32,
        x,
        y,
    });
}

pub fn install(tx: Sender<HookEvent>) -> AppResult<Guard> {
    *TX.lock() = Some(tx);
    let monitors = on_main(|_| {
        let mut list = Vec::new();
        let global = RcBlock::new(|event: NonNull<NSEvent>| {
            // SAFETY: 回调期间事件对象有效。
            emit_wheel(unsafe { event.as_ref() });
        });
        if let Some(m) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::ScrollWheel,
            &global,
        ) {
            list.push(m);
        }
        let local = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: 同上。
            let e = unsafe { event.as_ref() };
            let kind = e.r#type();
            if kind == NSEventType::ScrollWheel {
                emit_wheel(e);
            } else if let Some(key) = key_of(i64::from(e.keyCode())) {
                if kind == NSEventType::KeyDown && !e.isARepeat() {
                    emit(HookEvent::Key(key));
                }
                // 按下、松开都吞掉，别让页面再处理一遍
                return std::ptr::null_mut();
            }
            event.as_ptr()
        });
        // SAFETY: 回调只在主线程被调用，返回的要么是原事件要么是空。
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::ScrollWheel | NSEventMask::KeyDown | NSEventMask::KeyUp,
                &local,
            )
        };
        if let Some(m) = monitor {
            list.push(m);
        }
        Monitors(list)
    });

    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();

    let poll_stop = stop.clone();
    threads.push(
        std::thread::Builder::new()
            .name("input-hook-mouse".into())
            .spawn(move || {
                let mut last = None;
                while !poll_stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(33));
                    let now = input::cursor_position();
                    if now != last {
                        last = now;
                        if let Some((x, y)) = now {
                            emit(HookEvent::MouseMove { x, y });
                        }
                    }
                }
            })?,
    );

    if permissions::accessibility_granted() {
        let tap_stop = stop.clone();
        threads.push(
            std::thread::Builder::new()
                .name("input-hook-keys".into())
                .spawn(move || key_tap_thread(&tap_stop))?,
        );
    } else {
        tracing::info!("没有「辅助功能」权限：长截图的按键只在遮罩握着焦点时有效");
    }

    Ok(Guard {
        monitors: Some(monitors),
        stop,
        threads,
    })
}

/// 全局吞掉 Enter / Esc / Backspace。事件拦截挂在本线程的运行循环上；回调必须很快返回，
/// 否则系统会把拦截停用（收到停用通知时重新启用）。
fn key_tap_thread(stop: &AtomicBool) {
    let mask = (1u64 << ffi::kCGEventKeyDown) | (1u64 << ffi::kCGEventKeyUp);
    // SAFETY: 端口和运行循环源都在本线程创建、使用、释放。
    unsafe {
        let port = ffi::CGEventTapCreate(
            ffi::kCGSessionEventTap,
            ffi::kCGHeadInsertEventTap,
            ffi::kCGEventTapOptionDefault,
            mask,
            key_tap_callback,
            std::ptr::null_mut(),
        );
        if port.is_null() {
            tracing::warn!("创建按键拦截失败，长截图的按键只在遮罩握着焦点时有效");
            return;
        }
        let source = ffi::CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
        ffi::CFRunLoopAddSource(
            ffi::CFRunLoopGetCurrent(),
            source,
            ffi::kCFRunLoopCommonModes,
        );
        ffi::CGEventTapEnable(port, true);
        TAP_PORT.store(port as usize, Ordering::SeqCst);
        while !stop.load(Ordering::SeqCst) {
            ffi::CFRunLoopRunInMode(ffi::kCFRunLoopDefaultMode, 0.1, false);
        }
        TAP_PORT.store(0, Ordering::SeqCst);
        ffi::CGEventTapEnable(port, false);
        ffi::CFMachPortInvalidate(port);
        ffi::CFRelease(source.cast_const());
        ffi::CFRelease(port.cast_const());
    }
}

unsafe extern "C" fn key_tap_callback(
    _proxy: ffi::CGEventTapProxy,
    kind: u32,
    event: ffi::CGEventRef,
    _user_info: *mut std::ffi::c_void,
) -> ffi::CGEventRef {
    if kind == ffi::kCGEventTapDisabledByTimeout || kind == ffi::kCGEventTapDisabledByUserInput {
        let port = TAP_PORT.load(Ordering::SeqCst);
        if port != 0 {
            // SAFETY: 端口在拦截线程退出前一直有效。
            unsafe { ffi::CGEventTapEnable(port as ffi::CFMachPortRef, true) };
        }
        return event;
    }
    // SAFETY: 回调期间事件有效。
    let code = unsafe { ffi::CGEventGetIntegerValueField(event, ffi::kCGKeyboardEventKeycode) };
    match key_of(code) {
        Some(key) => {
            if kind == ffi::kCGEventKeyDown {
                emit(HookEvent::Key(key));
            }
            std::ptr::null_mut()
        }
        None => event,
    }
}
