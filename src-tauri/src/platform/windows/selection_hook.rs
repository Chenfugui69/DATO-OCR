//! 划词监听：常驻的 `WH_MOUSE_LL` 钩子，认出"像是刚选了一段文字"的鼠标手势。
//!
//! 判定：左键拖动超过一小段距离，或双击 / 三击；并且按下或松开时光标是文本光标（I 形）。
//! 拖窗口、拖文件、点桌面图标时光标不是 I 形，不会误触发。按住 Alt / Ctrl 选字是明确
//! 的意图，不要求 I 形光标。
//!
//! 悬浮按钮的点击在这里直接吞掉：按钮窗口不抢焦点，被吞的点击也到不了任何窗口，
//! 所以原程序的焦点和选区都还在，上层随后模拟 Ctrl+C 就能取到文字。
//!
//! 和长截图的钩子一样：专用线程 + 消息循环，回调里只做轻量判断和一次 channel 发送。

use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetDoubleClickTime, VK_CONTROL, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetCursorInfo, GetMessageW, GetSystemMetrics, LoadCursorW,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, CURSORINFO,
    IDC_IBEAM, MSG, MSLLHOOKSTRUCT, SM_CXDOUBLECLK, SM_CYDOUBLECLK, WH_MOUSE_LL, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN,
};

use crate::error::{AppError, AppResult};
use crate::platform::{PhysicalRect, SelectionEvent};

/// 拖动距离达到这么多物理像素才算"拖选"（150% 缩放下约 6 个逻辑像素）
const DRAG_MIN: i32 = 9;

struct Down {
    at: POINT,
    ibeam: bool,
    alt: bool,
    ctrl: bool,
    clicks: u32,
}

#[derive(Default)]
struct HookState {
    tx: Option<Sender<SelectionEvent>>,
    down: Option<Down>,
    last_click: Option<(POINT, Instant, u32)>,
    /// 悬浮按钮当前的屏幕矩形；None = 没显示
    button: Option<PhysicalRect>,
    /// 吞掉了按钮上的按下，对应的抬起也要吞
    swallow_up: bool,
}

static STATE: Mutex<HookState> = Mutex::new(HookState {
    tx: None,
    down: None,
    last_click: None,
    button: None,
    swallow_up: false,
});

pub struct Guard {
    thread_id: u32,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        // SAFETY: 让钩子线程退出消息循环，线程里卸载钩子。
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let mut st = STATE.lock();
        st.tx = None;
        st.down = None;
        st.button = None;
    }
}

pub fn install(tx: Sender<SelectionEvent>) -> AppResult<Guard> {
    STATE.lock().tx = Some(tx);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<u32, String>>();
    let handle = std::thread::Builder::new()
        .name("selection-hook".into())
        .spawn(move || {
            // SAFETY: 钩子在本线程安装和卸载，消息循环保证回调能被调度。
            unsafe {
                let module = GetModuleHandleW(None).ok().map(|m| m.into());
                let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
                match &hook {
                    Ok(_) => {
                        let _ = ready_tx.send(Ok(GetCurrentThreadId()));
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(format!("安装划词钩子失败：{err}")));
                        return;
                    }
                }
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                if let Ok(h) = hook {
                    let _ = UnhookWindowsHookEx(h);
                }
            }
        })?;
    let thread_id = ready_rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| AppError::msg("划词钩子线程没有响应"))?
        .map_err(AppError::Msg)?;
    Ok(Guard {
        thread_id,
        handle: Some(handle),
    })
}

/// 悬浮按钮显示 / 隐藏时告诉钩子它在哪，好吞掉按钮上的点击。
pub fn set_button_rect(rect: Option<PhysicalRect>) {
    let mut st = STATE.lock();
    st.button = rect;
    if rect.is_none() {
        st.swallow_up = false;
    }
}

fn is_ibeam_cursor() -> bool {
    // SAFETY: 只读查询；系统共享光标句柄不需要释放。
    unsafe {
        let mut info = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        if GetCursorInfo(&mut info).is_err() {
            return false;
        }
        LoadCursorW(None, IDC_IBEAM).is_ok_and(|ibeam| ibeam == info.hCursor)
    }
}

fn key_down(vk: u16) -> bool {
    // SAFETY: 纯查询。最高位为 1 表示按下。
    (unsafe { GetAsyncKeyState(i32::from(vk)) } as u16) & 0x8000 != 0
}

fn send(st: &HookState, event: SelectionEvent) {
    if let Some(tx) = st.tx.as_ref() {
        let _ = tx.send(event);
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: WH_MOUSE_LL 的 lparam 指向 MSLLHOOKSTRUCT。
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if handle(wparam.0 as u32, info.pt) {
            return LRESULT(1);
        }
    }
    // SAFETY: 原样交给下一个钩子。
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 返回 true = 吞掉这个事件。
fn handle(msg: u32, pt: POINT) -> bool {
    let mut st = STATE.lock();
    match msg {
        WM_LBUTTONDOWN => {
            if st.button.is_some_and(|r| r.contains_point(pt.x, pt.y)) {
                st.swallow_up = true;
                st.down = None;
                send(&st, SelectionEvent::ButtonClicked);
                return true;
            }
            if st.button.is_some() {
                send(&st, SelectionEvent::Dismiss);
            }
            // SAFETY: 纯查询。
            let (dbl_ms, dbl_w, dbl_h) = unsafe {
                (
                    GetDoubleClickTime(),
                    GetSystemMetrics(SM_CXDOUBLECLK),
                    GetSystemMetrics(SM_CYDOUBLECLK),
                )
            };
            let now = Instant::now();
            let clicks = match st.last_click {
                Some((p, t, n))
                    if now.duration_since(t) <= Duration::from_millis(u64::from(dbl_ms))
                        && (p.x - pt.x).abs() <= dbl_w / 2
                        && (p.y - pt.y).abs() <= dbl_h / 2 =>
                {
                    n + 1
                }
                _ => 1,
            };
            st.last_click = Some((pt, now, clicks));
            st.down = Some(Down {
                at: pt,
                ibeam: is_ibeam_cursor(),
                alt: key_down(VK_MENU.0),
                ctrl: key_down(VK_CONTROL.0),
                clicks,
            });
        }
        WM_LBUTTONUP => {
            if st.swallow_up {
                st.swallow_up = false;
                return true;
            }
            let Some(down) = st.down.take() else {
                return false;
            };
            let dragged = (down.at.x - pt.x).abs().max((down.at.y - pt.y).abs()) >= DRAG_MIN;
            let multi = down.clicks >= 2;
            if !dragged && !multi {
                return false;
            }
            let alt = down.alt || key_down(VK_MENU.0);
            let ctrl = down.ctrl || key_down(VK_CONTROL.0);
            if down.ibeam || alt || ctrl || is_ibeam_cursor() {
                let (x0, x1) = (down.at.x.min(pt.x), down.at.x.max(pt.x));
                let (y0, y1) = (down.at.y.min(pt.y), down.at.y.max(pt.y));
                let anchor = PhysicalRect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32);
                send(
                    &st,
                    SelectionEvent::Selected {
                        anchor,
                        end: (pt.x, pt.y),
                        alt,
                        ctrl,
                    },
                );
            }
        }
        WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            st.down = None;
            if st.button.is_some() {
                send(&st, SelectionEvent::Dismiss);
            }
        }
        _ => {}
    }
    false
}
