//! 藏起来的窗口让网页省内存。
//!
//! WebView2 不会自己察觉宿主窗口被藏起来了：Tauri 的 `hide()` 和我们自己的 `ShowWindow` 只藏窗口，
//! 网页照样当自己在前台，渲染进程按"可见"留着画面缓冲和各种缓存。这里给窗口挂一层子类，
//! 窗口藏够 [`IDLE_AFTER`] 就告诉 WebView2"不可见"（`IsVisible = false`）并把内存目标降到 Low，
//! 让它丢掉画面、压缩缓存；窗口要显示时（`WM_WINDOWPOSCHANGING` 带 `SWP_SHOWWINDOW`，
//! 比真正上屏早一步）立刻恢复。
//!
//! 不马上降：刚藏起来的页面常常还有收尾（关闭动画之后复位状态、等一两帧），网页不可见时
//! `requestAnimationFrame` 停、定时器被压到一秒一次，收尾会拖到下次显示才跑完。

use std::time::Duration;

use tauri::WebviewWindow;
use webview2_com::GetProcessExtendedInfosCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller, ICoreWebView2Environment13, ICoreWebView2FrameInfo2,
    ICoreWebView2ProcessExtendedInfoCollection, ICoreWebView2_19, ICoreWebView2_2,
    ICoreWebView2_20, COREWEBVIEW2_FRAME_KIND, COREWEBVIEW2_FRAME_KIND_MAIN_FRAME,
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    COREWEBVIEW2_PROCESS_KIND, COREWEBVIEW2_PROCESS_KIND_RENDERER,
};
use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
use windows::Win32::UI::Shell::{
    DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindowVisible, KillTimer, SetTimer, SWP_HIDEWINDOW, SWP_SHOWWINDOW, WINDOWPOS, WM_NCDESTROY,
    WM_TIMER, WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING,
};

/// 子类 id，也拿来当定时器 id（定时器 id 只在这个窗口里唯一就行）
const ID: usize = 0x5744_4C45; // "WDLE"
/// 藏了这么久还没再显示，才让网页进入省内存状态
const IDLE_AFTER: Duration = Duration::from_secs(8);

struct State {
    controller: ICoreWebView2Controller,
    webview: Option<ICoreWebView2_19>,
    idle: bool,
}

impl State {
    fn set_idle(&mut self, idle: bool) {
        if self.idle == idle {
            return;
        }
        self.idle = idle;
        let level = if idle {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
        } else {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
        };
        // SAFETY: 都在窗口所在的 UI 线程上调用，COM 对象由 State 持有着。
        unsafe {
            // 先恢复可见再提内存目标，反过来先降目标再藏：两种顺序都让网页少做一次无用功
            if idle {
                let _ = self.controller.SetIsVisible(false);
            }
            if let Some(webview) = &self.webview {
                let _ = webview.SetMemoryUsageTargetLevel(level);
            }
            if !idle {
                let _ = self.controller.SetIsVisible(true);
            }
        }
    }
}

/// 只调网页的内存目标，不动可见性（截图遮罩收起时用，见 `effects::park_overlay`）。
pub fn set_memory_low(window: &WebviewWindow, low: bool) {
    let level = if low {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    };
    let _ = window.with_webview(move |webview| {
        // SAFETY: 在事件循环线程上，controller 是这个窗口存活的 WebView2 控制器。
        unsafe {
            if let Ok(w) = webview
                .controller()
                .CoreWebView2()
                .and_then(|w| w.cast::<ICoreWebView2_19>())
            {
                let _ = w.SetMemoryUsageTargetLevel(level);
            }
        }
    });
}

/// 给窗口挂上（页面每次开始加载都会调，已经挂过就什么都不做）。
pub fn track(window: &WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else { return };
    let hwnd = hwnd.0 as isize;
    let _ = window.with_webview(move |webview| {
        let hwnd = HWND(hwnd as *mut _);
        let mut existing = 0usize;
        // SAFETY: 在窗口所在的 UI 线程（with_webview 的回调就在事件循环线程上）查询本窗口的子类。
        if unsafe { GetWindowSubclass(hwnd, Some(proc), ID, Some(&mut existing)) }.as_bool() {
            return;
        }
        let controller = webview.controller();
        // SAFETY: controller 是这个窗口存活的 WebView2 控制器。
        let webview = unsafe { controller.CoreWebView2() }
            .ok()
            .and_then(|w| w.cast::<ICoreWebView2_19>().ok());
        let state = Box::into_raw(Box::new(State {
            controller,
            webview,
            idle: false,
        }));
        // SAFETY: 同一线程；state 的所有权交给子类，WM_NCDESTROY 时收回。
        if !unsafe { SetWindowSubclass(hwnd, Some(proc), ID, state as usize) }.as_bool() {
            // SAFETY: 没挂上，所有权还在这里
            drop(unsafe { Box::from_raw(state) });
            tracing::debug!("挂窗口子类失败，藏起来的窗口不会省内存");
        }
    });
}

unsafe extern "system" fn proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: data 是 `track` 里交过来的 Box<State>，只在这个窗口的 UI 线程上访问，NCDESTROY 前一直有效。
    let state = unsafe { &mut *(data as *mut State) };
    match msg {
        WM_WINDOWPOSCHANGING => {
            // SAFETY: 这两条消息的 lparam 是系统给的 WINDOWPOS
            let flags = unsafe { (*(lparam.0 as *const WINDOWPOS)).flags };
            if flags.contains(SWP_SHOWWINDOW) {
                // SAFETY: 本窗口的定时器
                let _ = unsafe { KillTimer(Some(hwnd), ID) };
                state.set_idle(false);
            }
        }
        WM_WINDOWPOSCHANGED => {
            // SAFETY: 同上
            let flags = unsafe { (*(lparam.0 as *const WINDOWPOS)).flags };
            if flags.contains(SWP_HIDEWINDOW) {
                // SAFETY: 本窗口的定时器；同一个 id 再设只是重新计时
                unsafe { SetTimer(Some(hwnd), ID, IDLE_AFTER.as_millis() as u32, None) };
            }
        }
        WM_TIMER if wparam.0 == ID => {
            // SAFETY: 本窗口
            unsafe {
                let _ = KillTimer(Some(hwnd), ID);
                if !IsWindowVisible(hwnd).as_bool() {
                    state.set_idle(true);
                }
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            // SAFETY: 窗口要没了：摘掉子类、收回 State（之后不会再有消息进来）
            unsafe {
                let _ = KillTimer(Some(hwnd), ID);
                let _ = RemoveWindowSubclass(hwnd, Some(proc), ID);
                drop(Box::from_raw(data as *mut State));
                return DefSubclassProc(hwnd, msg, wparam, lparam);
            }
        }
        _ => {}
    }
    // SAFETY: 原样交给下一层窗口过程。
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// 结束这个窗口的网页（渲染）进程。页面卡在死循环里时光重新加载没用：WebView2 要先等旧页面卸载，
/// 卡住的页面永远轮不到，实测重新加载后 30 秒还卡着。每个窗口各有一个渲染进程（同源的几个窗口也是
/// 分开的，实测），按窗口主框架的编号找到它再结束。结束后页面停在"崩溃"状态，由调用方重新加载。
///
/// **别在 UI 线程调**：要到 UI 线程上问 WebView2、等它回话，最多等 2 秒。
pub fn kill_renderer(window: &WebviewWindow) -> bool {
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    let sent = window.with_webview(move |webview| {
        let reply = tx.clone();
        // SAFETY: 在事件循环线程上，controller 是这个窗口存活的 WebView2 控制器。
        let started = unsafe {
            (|| -> windows::core::Result<()> {
                let core = webview.controller().CoreWebView2()?;
                let mut frame = 0u32;
                core.cast::<ICoreWebView2_20>()?.FrameId(&mut frame)?;
                let env = core
                    .cast::<ICoreWebView2_2>()?
                    .Environment()?
                    .cast::<ICoreWebView2Environment13>()?;
                let handler = GetProcessExtendedInfosCompletedHandler::create(Box::new(
                    move |result, list| {
                        let pid = result.ok().and(list).and_then(|l| renderer_of(&l, frame));
                        tracing::debug!(frame, ?pid, "找卡死页面的网页进程");
                        let _ = reply.send(pid.is_some_and(terminate));
                        Ok(())
                    },
                ));
                env.GetProcessExtendedInfos(&handler)
            })()
        };
        if started.is_err() {
            let _ = tx.send(false);
        }
    });
    if sent.is_err() {
        return false;
    }
    rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false)
}

/// 哪个渲染进程装着这个主框架。找到不止一个（理论上编号不该重）就不认，免得结束错了别的窗口。
fn renderer_of(list: &ICoreWebView2ProcessExtendedInfoCollection, frame: u32) -> Option<u32> {
    let mut found = None;
    // SAFETY: 在 WebView2 回调里（UI 线程），集合和里面的对象都有效。
    unsafe {
        let mut n = 0u32;
        list.Count(&mut n).ok()?;
        for i in 0..n {
            let Ok(info) = list.GetValueAtIndex(i) else { continue };
            let Ok(process) = info.ProcessInfo() else { continue };
            let mut kind = COREWEBVIEW2_PROCESS_KIND::default();
            if process.Kind(&mut kind).is_err() || kind != COREWEBVIEW2_PROCESS_KIND_RENDERER {
                continue;
            }
            // 集合要一直拿着：WebView2 的迭代器不替集合续命，集合一放掉迭代器就指向释放了的内存（实测直接崩）
            let Ok(frames) = info.AssociatedFrameInfos() else { continue };
            let Ok(it) = frames.GetIterator() else { continue };
            let mut has = BOOL(0);
            while it.HasCurrent(&mut has).is_ok() && has.as_bool() {
                if let Ok(f) = it.GetCurrent().and_then(|f| f.cast::<ICoreWebView2FrameInfo2>()) {
                    let mut id = 0u32;
                    let mut fk = COREWEBVIEW2_FRAME_KIND::default();
                    if f.FrameId(&mut id).is_ok()
                        && id == frame
                        && f.FrameKind(&mut fk).is_ok()
                        && fk == COREWEBVIEW2_FRAME_KIND_MAIN_FRAME
                    {
                        let mut pid = 0i32;
                        process.ProcessId(&mut pid).ok()?;
                        if found.is_some() {
                            return None;
                        }
                        found = u32::try_from(pid).ok();
                        break;
                    }
                }
                if it.MoveNext(&mut has).is_err() {
                    break;
                }
            }
        }
    }
    found
}

fn terminate(pid: u32) -> bool {
    // SAFETY: 打开、结束、关掉句柄，句柄只在这里用。
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) else { return false };
        let ok = TerminateProcess(handle, 1).is_ok();
        let _ = CloseHandle(handle);
        ok
    }
}
