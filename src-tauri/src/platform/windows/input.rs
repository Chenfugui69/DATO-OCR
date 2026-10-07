//! 输入模拟与焦点还原（规格 05 §3.4、07 §4.5）。

use std::time::Duration;

use windows::Win32::Foundation::POINT;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, SetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_C, VK_CONTROL, VK_LCONTROL, VK_LMENU,
    VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, IsIconic,
    IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

use super::util::hwnd;
use crate::error::{AppError, AppResult};
use crate::platform::WindowHandle;

pub fn cursor_position() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY: 输出到栈上的 POINT。
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

/// 把焦点还给目标窗口。非前台进程直接 `SetForegroundWindow` 会被前台锁拦下，
/// 所以先 `AttachThreadInput` 挂到当前前台线程的输入队列上。
pub fn focus_window(window: WindowHandle) -> AppResult<()> {
    let target = hwnd(window.0);
    // SAFETY: 所有调用都容忍目标窗口已经销毁（返回失败而不是 UB）。
    unsafe {
        if !IsWindow(Some(target)).as_bool() {
            return Err(AppError::msg("目标窗口已关闭"));
        }
        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        }
        let current = GetCurrentThreadId();
        let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let target_thread = GetWindowThreadProcessId(target, None);

        let attach_fg = fg_thread != 0 && fg_thread != current;
        let attach_target =
            target_thread != 0 && target_thread != current && target_thread != fg_thread;
        if attach_fg {
            let _ = AttachThreadInput(current, fg_thread, true);
        }
        if attach_target {
            let _ = AttachThreadInput(current, target_thread, true);
        }
        let _ = BringWindowToTop(target);
        let ok = SetForegroundWindow(target).as_bool();
        let _ = SetFocus(Some(target));
        if attach_target {
            let _ = AttachThreadInput(current, target_thread, false);
        }
        if attach_fg {
            let _ = AttachThreadInput(current, fg_thread, false);
        }
        if !ok {
            tracing::debug!("SetForegroundWindow 未成功，目标窗口可能拿不到焦点");
        }
    }
    Ok(())
}

/// 长截图：滚轮发给有焦点的窗口（"滚动非活动窗口"关闭的系统上尤其如此），所以把焦点交过去。
pub fn route_scroll_to(window: WindowHandle) -> AppResult<()> {
    focus_window(window)
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// 给所有当前按着的修饰键发 keyup。用户按 Alt+V 唤出面板后可能还没松开 Alt，
/// 这时直接发 Ctrl+V 会变成 Ctrl+Alt+V。
const MENU_MASK_KEY: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);

fn release_modifiers() {
    let modifiers = [
        VK_MENU,
        VK_LMENU,
        VK_RMENU,
        VK_CONTROL,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_SHIFT,
        VK_LSHIFT,
        VK_RSHIFT,
        VK_LWIN,
        VK_RWIN,
    ];
    let held: Vec<VIRTUAL_KEY> = modifiers
        .iter()
        // SAFETY: 纯查询。最高位为 1 表示按下。
        .filter(|vk| unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000 != 0)
        .copied()
        .collect();
    let mut pressed: Vec<INPUT> = Vec::new();
    if held
        .iter()
        .any(|vk| matches!(*vk, VK_MENU | VK_LMENU | VK_RMENU))
    {
        // 单独按下又松开 Alt 会激活传统程序的菜单栏。先按一下一个没人用的键（0xE8，
        // 同 AutoHotkey 的 MenuMaskKey）把这次 Alt 变成"组合键"，再松开 Alt
        pressed.push(key(MENU_MASK_KEY, false));
        pressed.push(key(MENU_MASK_KEY, true));
    }
    pressed.extend(held.iter().map(|vk| key(*vk, true)));
    if !pressed.is_empty() {
        // SAFETY: 输入数组和结构体大小都如实传入。
        unsafe { SendInput(&pressed, std::mem::size_of::<INPUT>() as i32) };
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn send_ctrl(letter: VIRTUAL_KEY) -> AppResult<()> {
    release_modifiers();
    let inputs = [
        key(VK_CONTROL, false),
        key(letter, false),
        key(letter, true),
        key(VK_CONTROL, true),
    ];
    // SAFETY: 同上。
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(AppError::msg(
            "模拟按键被系统拦截（目标可能是管理员权限窗口）",
        ));
    }
    Ok(())
}

pub fn send_paste() -> AppResult<()> {
    send_ctrl(VK_V)
}

pub fn send_copy() -> AppResult<()> {
    send_ctrl(VK_C)
}
