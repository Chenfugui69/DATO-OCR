//! 系统事件监听：显示器变化、睡眠唤醒、主题/透明度变化。
//!
//! **监听窗口必须是真正的顶层窗口，不能用 `HWND_MESSAGE`**：message-only 窗口收不到
//! 广播消息，而 `WM_DISPLAYCHANGE` / `WM_POWERBROADCAST` / `WM_SETTINGCHANGE` 正是广播
//! 给顶层窗口的。用了 HWND_MESSAGE 的话代码看着完全正常、永远不触发。

use std::sync::OnceLock;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassExW,
    TranslateMessage, MSG, WM_DISPLAYCHANGE, WM_POWERBROADCAST, WM_SETTINGCHANGE, WNDCLASSEXW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::platform::SystemEvent;

static HANDLER: OnceLock<fn(SystemEvent)> = OnceLock::new();

const PBT_APMRESUMESUSPEND: usize = 0x7;
const WM_DWMCOLORIZATIONCHANGED: u32 = 0x0320;
const PBT_APMRESUMEAUTOMATIC: usize = 0x12;

pub fn install(handler: fn(SystemEvent)) {
    if HANDLER.set(handler).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("system-events".into())
        .spawn(|| {
            // SAFETY: 标准窗口类注册 + 隐藏顶层窗口 + 消息循环。
            unsafe {
                let Ok(module) = GetModuleHandleW(None) else {
                    return;
                };
                let class_name = w!("ChenocrSystemEvents");
                let class = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(proc),
                    hInstance: module.into(),
                    lpszClassName: class_name,
                    ..Default::default()
                };
                RegisterClassExW(&class);
                // 不可见、不进任务栏的顶层窗口
                if CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class_name,
                    w!("DATO COR System Events"),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    Some(module.into()),
                    None,
                )
                .is_err()
                {
                    tracing::warn!("创建系统事件监听窗口失败");
                    return;
                }
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("启动系统事件线程失败：{err}");
    }
}

fn fire(event: SystemEvent) {
    if let Some(handler) = HANDLER.get() {
        handler(event);
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DISPLAYCHANGE => fire(SystemEvent::DisplayChanged),
        WM_POWERBROADCAST if matches!(wparam.0, PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC) => {
            fire(SystemEvent::Resumed)
        }
        WM_DWMCOLORIZATIONCHANGED => fire(SystemEvent::ThemeChanged),
        WM_SETTINGCHANGE if lparam.0 != 0 => {
            // SAFETY: WM_SETTINGCHANGE 的 lparam 是以 NUL 结尾的宽字符串（或 0）。
            let area = unsafe { PCWSTR(lparam.0 as *const u16).to_string() }.unwrap_or_default();
            if area == "ImmersiveColorSet" {
                fire(SystemEvent::ThemeChanged);
            }
        }
        _ => {}
    }
    // SAFETY: 原样转发。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
