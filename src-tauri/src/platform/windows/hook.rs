//! 长截图用的全局低级钩子（规格 03 §3.1、07 §4.6）。
//!
//! 三条硬性要求：
//! 1. 钩子装在**有消息循环的专用线程**上（不能是 UI 线程，会被 UI 卡住）
//! 2. 回调必须极快返回（系统 LowLevelHooksTimeout 默认 300ms，超时会被静默摘掉），
//!    回调里只做一次 channel 发送
//! 3. 会话结束必须 `UnhookWindowsHookEx`，全局钩子不卸会拖慢整个系统的输入
//!
//! 鼠标事件全部放行（用户要滚动下面的页面）；键盘只吞 Enter / Esc / Backspace ——
//! 长截图期间遮罩是鼠标穿透的、拿不到焦点，这三个键必须由我们截获。

use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

use crate::error::{AppError, AppResult};
use crate::platform::{HookEvent, HookKey};

static TX: Mutex<Option<Sender<HookEvent>>> = Mutex::new(None);

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
        *TX.lock() = None;
    }
}

pub fn install(tx: Sender<HookEvent>) -> AppResult<Guard> {
    *TX.lock() = Some(tx);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<u32, String>>();
    let handle = std::thread::Builder::new()
        .name("input-hook".into())
        .spawn(move || {
            // SAFETY: 钩子在本线程安装和卸载，消息循环保证回调能被调度。
            unsafe {
                let module = GetModuleHandleW(None).ok().map(|m| m.into());
                let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
                let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
                match (&mouse, &keyboard) {
                    (Ok(_), Ok(_)) => {
                        let _ = ready_tx.send(Ok(GetCurrentThreadId()));
                    }
                    _ => {
                        let _ = ready_tx.send(Err("安装全局钩子失败".into()));
                    }
                }
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

fn emit(event: HookEvent) {
    if let Some(tx) = TX.lock().as_ref() {
        let _ = tx.send(event);
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: WH_MOUSE_LL 的 lparam 指向 MSLLHOOKSTRUCT。
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        match wparam.0 as u32 {
            WM_MOUSEWHEEL => emit(HookEvent::Wheel {
                delta: i32::from((info.mouseData >> 16) as u16 as i16),
                x: info.pt.x,
                y: info.pt.y,
            }),
            WM_MOUSEMOVE => emit(HookEvent::MouseMove {
                x: info.pt.x,
                y: info.pt.y,
            }),
            _ => {}
        }
    }
    // SAFETY: 原样交给下一个钩子。
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: WH_KEYBOARD_LL 的 lparam 指向 KBDLLHOOKSTRUCT。
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let key = match info.vkCode {
            0x0D => Some(HookKey::Enter),
            0x1B => Some(HookKey::Escape),
            0x08 => Some(HookKey::Backspace),
            _ => None,
        };
        if let Some(key) = key {
            let msg = wparam.0 as u32;
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                emit(HookEvent::Key(key));
                return LRESULT(1);
            }
            if msg == WM_KEYUP || msg == WM_SYSKEYUP {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: 原样交给下一个钩子。
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
