//! 录屏时的键盘和滚轮：全局低级钩子只旁听（全部放行），按下的键显示在画面角落、滚动时画箭头。
//! 和长截图的钩子一样装在有消息循环的专用线程上，回调里只发一次 channel。
//! 鼠标键不在这里：录制每一拍直接问系统按着没有（`mouse_buttons`），不用监听。

use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, MAPVK_VK_TO_CHAR, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetSystemMetrics, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    SM_SWAPBUTTON, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_MOUSEWHEEL, WM_QUIT,
    WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::error::{AppError, AppResult};
use crate::platform::types::RecordInput;

struct State {
    tx: Sender<RecordInput>,
    /// 按着没松的键：低级钩子不区分自动重复，按住不放时重复来的按下不算
    down: HashSet<u32>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub struct Guard {
    thread_id: u32,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        // SAFETY: 向钩子线程投递 WM_QUIT，线程收到后卸载钩子并退出。
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        *STATE.lock() = None;
    }
}

/// 现在按着哪几个鼠标键：第 0 位左键、第 1 位右键、第 2 位中键（左右手习惯换过键的按换过的算）
pub fn mouse_buttons() -> u8 {
    // SAFETY: 只读系统的按键状态。
    let pressed = |vk: u16| unsafe { GetAsyncKeyState(i32::from(vk)) } as u16 & 0x8000 != 0;
    // GetAsyncKeyState 看的是物理键
    // SAFETY: 只读系统设置。
    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    let (left, right) = if swapped {
        (VK_RBUTTON, VK_LBUTTON)
    } else {
        (VK_LBUTTON, VK_RBUTTON)
    };
    u8::from(pressed(left.0))
        | u8::from(pressed(right.0)) << 1
        | u8::from(pressed(VK_MBUTTON.0)) << 2
}

pub fn install(tx: Sender<RecordInput>) -> AppResult<Guard> {
    *STATE.lock() = Some(State {
        tx,
        down: HashSet::new(),
    });
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<u32, String>>();
    let handle = std::thread::Builder::new()
        .name("record-input".into())
        .spawn(move || {
            // SAFETY: 钩子在本线程安装和卸载，消息循环保证回调能被调度。
            unsafe {
                let module = GetModuleHandleW(None).ok().map(|m| m.into());
                let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
                let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
                let _ = ready_tx.send(match (&mouse, &keyboard) {
                    (Ok(_), Ok(_)) => Ok(GetCurrentThreadId()),
                    _ => Err("安装录屏的键盘监听失败".into()),
                });
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                if let Ok(h) = mouse {
                    let _ = UnhookWindowsHookEx(h);
                }
                if let Ok(h) = keyboard {
                    let _ = UnhookWindowsHookEx(h);
                }
            }
        })?;
    let thread_id = ready_rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| AppError::msg("钩子线程没有响应"))?
        .map_err(AppError::Msg)?;
    Ok(Guard {
        thread_id,
        handle: Some(handle),
    })
}

fn send(input: RecordInput) {
    if let Some(st) = STATE.lock().as_ref() {
        let _ = st.tx.send(input);
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WM_MOUSEWHEEL {
        // SAFETY: WH_MOUSE_LL 的 lparam 指向 MSLLHOOKSTRUCT。
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        // 往上滚（离开用户）是正的；这里要的是往下滚为正
        let delta = i32::from((info.mouseData >> 16) as u16 as i16);
        if delta != 0 {
            send(RecordInput::Scroll(-f64::from(delta) / 120.0));
        }
    }
    // SAFETY: 原样交给下一个钩子。
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: WH_KEYBOARD_LL 的 lparam 指向 KBDLLHOOKSTRUCT。
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let vk = info.vkCode;
        match wparam.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let label = STATE.lock().as_mut().and_then(|st| {
                    // 按住不放的自动重复不算
                    st.down.insert(vk).then(|| label(vk, &st.down)).flatten()
                });
                if let Some(label) = label {
                    send(RecordInput::Key(label));
                }
            }
            WM_KEYUP | WM_SYSKEYUP => {
                if let Some(st) = STATE.lock().as_mut() {
                    st.down.remove(&vk);
                }
            }
            _ => {}
        }
    }
    // SAFETY: 原样交给下一个钩子（只旁听，不吞键）。
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 修饰键：VK 码 → 属于哪一个（Ctrl / Alt / Shift / Win）
fn modifier(vk: u32) -> Option<usize> {
    match vk {
        0x11 | 0xA2 | 0xA3 => Some(0),
        0x12 | 0xA4 | 0xA5 => Some(1),
        0x10 | 0xA0 | 0xA1 => Some(2),
        0x5B | 0x5C => Some(3),
        _ => None,
    }
}

const MODIFIER_NAMES: [&str; 4] = ["Ctrl", "Alt", "Shift", "Win"];

/// 不是普通字符的键：VK 码 → 显示的写法
fn special(vk: u32) -> Option<String> {
    Some(
        match vk {
            0x0D => "Enter",
            0x09 => "Tab",
            0x20 => "Space",
            0x08 => "Backspace",
            0x2E => "Delete",
            0x2D => "Insert",
            0x1B => "Esc",
            0x25 => "←",
            0x27 => "→",
            0x28 => "↓",
            0x26 => "↑",
            0x24 => "Home",
            0x23 => "End",
            0x21 => "PgUp",
            0x22 => "PgDn",
            0x2C => "PrtSc",
            0x14 => "CapsLock",
            0x70..=0x87 => return Some(format!("F{}", vk - 0x6F)),
            0x60..=0x69 => return Some(format!("Num {}", vk - 0x60)),
            _ => return None,
        }
        .to_string(),
    )
}

/// 一次按键显示成什么：按着的修饰键 + 这个键，例如 Ctrl+Shift+S。只按修饰键不显示
fn label(vk: u32, down: &HashSet<u32>) -> Option<String> {
    if modifier(vk).is_some() {
        return None;
    }
    let key = match special(vk) {
        Some(s) => s,
        None => {
            // SAFETY: 只查键盘布局里这个键对应的字符。
            let ch = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_CHAR) } & 0x7FFF;
            let ch = char::from_u32(ch).filter(|c| !c.is_control() && !c.is_whitespace())?;
            ch.to_uppercase().to_string()
        }
    };
    let mut held = [false; 4];
    for &d in down {
        if let Some(i) = modifier(d) {
            held[i] = true;
        }
    }
    let mut out: Vec<&str> = MODIFIER_NAMES
        .iter()
        .zip(held)
        .filter(|(_, h)| *h)
        .map(|(n, _)| *n)
        .collect();
    out.push(&key);
    Some(out.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_combine_held_modifiers() {
        let down: HashSet<u32> = [0xA2, 0xA0, 0x53].into_iter().collect();
        assert_eq!(label(0x53, &down).as_deref(), Some("Ctrl+Shift+S"));
        assert_eq!(label(0xA2, &down), None);
        let none = HashSet::new();
        assert_eq!(label(0x0D, &none).as_deref(), Some("Enter"));
        assert_eq!(label(0x71, &none).as_deref(), Some("F2"));
    }
}
