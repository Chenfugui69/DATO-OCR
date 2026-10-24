//! 录屏时的键盘和滚轮：系统级事件监听（和划词监听一样的做法），按下的键显示在画面角落、滚动时画箭头。
//! 别的程序的按键要有「辅助功能」权限才收得到，没有就什么都不来（不报错）。
//! 鼠标键不在这里：录制每一拍直接问系统按着没有（`mouse_buttons`），不用监听。

use std::ptr::NonNull;
use std::sync::mpsc::Sender;

use block2::RcBlock;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use parking_lot::Mutex;

use super::ffi;
use super::hook::Monitors;
use super::util::on_main;
use crate::error::AppResult;
use crate::platform::types::RecordInput;

static TX: Mutex<Option<Sender<RecordInput>>> = Mutex::new(None);

pub struct Guard {
    monitors: Option<Monitors>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(m) = self.monitors.take() {
            m.remove();
        }
        *TX.lock() = None;
    }
}

/// 现在按着哪几个鼠标键：第 0 位左键、第 1 位右键、第 2 位中键
pub fn mouse_buttons() -> u8 {
    // SAFETY: 只读系统的按键状态
    unsafe {
        u8::from(ffi::CGEventSourceButtonState(
            ffi::kCGEventSourceStateCombinedSessionState,
            ffi::kCGMouseButtonLeft,
        )) | u8::from(ffi::CGEventSourceButtonState(
            ffi::kCGEventSourceStateCombinedSessionState,
            ffi::kCGMouseButtonRight,
        )) << 1
            | u8::from(ffi::CGEventSourceButtonState(
                ffi::kCGEventSourceStateCombinedSessionState,
                ffi::kCGMouseButtonCenter,
            )) << 2
    }
}

/// 不是普通字符的键：按键码 → 显示的写法
fn special(code: u16) -> Option<&'static str> {
    Some(match code {
        36 | 76 => "⏎",
        48 => "⇥",
        49 => "Space",
        51 => "⌫",
        117 => "⌦",
        53 => "Esc",
        123 => "←",
        124 => "→",
        125 => "↓",
        126 => "↑",
        115 => "Home",
        119 => "End",
        116 => "PgUp",
        121 => "PgDn",
        122 => "F1",
        120 => "F2",
        99 => "F3",
        118 => "F4",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        100 => "F8",
        101 => "F9",
        109 => "F10",
        103 => "F11",
        111 => "F12",
        _ => return None,
    })
}

fn label(event: &NSEvent) -> Option<String> {
    let flags = event.modifierFlags();
    let mut out = String::new();
    for (flag, sym) in [
        (NSEventModifierFlags::Control, "⌃"),
        (NSEventModifierFlags::Option, "⌥"),
        (NSEventModifierFlags::Shift, "⇧"),
        (NSEventModifierFlags::Command, "⌘"),
    ] {
        if flags.contains(flag) {
            out.push_str(sym);
        }
    }
    let key = match special(event.keyCode()) {
        Some(s) => s.to_string(),
        None => event
            .charactersIgnoringModifiers()
            .map(|s| s.to_string().to_uppercase())
            .filter(|s| !s.trim().is_empty())?,
    };
    out.push_str(&key);
    Some(out)
}

fn handle(event: &NSEvent) {
    let Some(tx) = TX.lock().clone() else { return };
    let input = match event.r#type() {
        // 按住不放的自动重复不算
        NSEventType::KeyDown if !event.isARepeat() => label(event).map(RecordInput::Key),
        NSEventType::ScrollWheel => {
            let dy = event.scrollingDeltaY();
            (dy.abs() > 0.1).then_some(RecordInput::Scroll(-dy))
        }
        _ => None,
    };
    if let Some(i) = input {
        let _ = tx.send(i);
    }
}

pub fn install(tx: Sender<RecordInput>) -> AppResult<Guard> {
    *TX.lock() = Some(tx);
    let mask = NSEventMask::KeyDown | NSEventMask::ScrollWheel;
    let monitors = on_main(move |_| {
        let mut list = Vec::new();
        let global = RcBlock::new(|event: NonNull<NSEvent>| {
            // SAFETY: 回调期间事件对象有效。
            handle(unsafe { event.as_ref() });
        });
        if let Some(m) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global) {
            list.push(m);
        }
        Monitors(list)
    });
    Ok(Guard {
        monitors: Some(monitors),
    })
}
