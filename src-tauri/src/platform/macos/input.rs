//! 输入模拟与焦点还原。
//!
//! 浮层在 macOS 上都是"不激活应用"的面板（见 `effects::build_floating`）：面板出现时原来的
//! 应用一直是激活状态，面板一收起键盘焦点自己回到它那里。所以这里的"还原焦点"多数时候
//! 什么都不用做，只有目标应用确实不在前台时才去激活它。

use std::time::Duration;

use objc2_app_kit::{NSApplication, NSApplicationActivationOptions, NSWorkspace};

use super::ffi;
use super::util::{handle_pid, handle_window, on_main, own_pid};
use super::{geometry, permissions, window_enum};
use crate::error::{AppError, AppResult};
use crate::platform::WindowHandle;

pub fn cursor_position() -> Option<(i32, i32)> {
    window_enum::cursor_point().map(geometry::point_to_physical)
}

pub fn focus_window(window: WindowHandle) -> AppResult<()> {
    let pid = handle_pid(window.0);
    if pid == own_pid() {
        let number = handle_window(window.0) as isize;
        on_main(move |mtm| {
            let app = NSApplication::sharedApplication(mtm);
            // 记下的是"我们自己、但没有具体窗口"（比如用户刚把主窗口收进程序坞，前台还是我们）：什么都不做。
            // 激活整个应用的话，开着"把窗口最小化为应用程序图标"时系统会把收起的窗口一起还原出来；
            // 收进程序坞的窗口也不去"放到最前"（那等于把它还原）
            let Some(w) = app
                .windowWithWindowNumber(number)
                .filter(|w| !w.isMiniaturized())
            else {
                return;
            };
            w.makeKeyAndOrderFront(None);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        });
        return Ok(());
    }
    let front = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|a| a.processIdentifier());
    if front == Some(pid) {
        return Ok(());
    }
    let app = window_enum::running_app(pid).ok_or_else(|| AppError::msg("目标应用已退出"))?;
    #[allow(deprecated)]
    let ok = app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    if !ok {
        tracing::debug!("激活目标应用未成功，它可能拿不到焦点");
    }
    Ok(())
}

/// 长截图时滚轮该发给谁。macOS 的滚轮永远发给鼠标底下的窗口，和焦点无关，不用做任何事；
/// 遮罩面板还握着键盘焦点，正好用来收 Enter / Esc / Backspace。
pub fn route_scroll_to(_window: WindowHandle) -> AppResult<()> {
    Ok(())
}

/// 发 ⌘+某键。事件上只带 ⌘ 这一个修饰键标志，用户手上还按着的 ⌥（刚按完 ⌥V）不会混进去。
///
/// ⌘ 本身也要按下、松开各发一次：只在那个键上打"带着 ⌘"的标志的话，系统记的合成按键状态里 ⌘ 就一直
/// 是按着的（`CGEventSourceFlagsState(CombinedSessionState)` 一直带 ⌘），之后程序模拟的点击全变成
/// ⌘+点击 —— 拖放粘贴在 Claude 这类程序里点不进输入框，粘贴时好时坏
fn send_command(key: u16) -> AppResult<()> {
    permissions::ensure_accessibility()?;
    // SAFETY: 事件来源和事件都在本函数内创建并释放。
    unsafe {
        let source = ffi::CGEventSourceCreate(ffi::kCGEventSourceStateCombinedSessionState);
        if !source.is_null() {
            // 合成事件之后的一小段时间里系统默认会压住真实键盘事件；只放行鼠标和系统事件，
            // 免得用户手上的修饰键在这期间搅进来
            ffi::CGEventSourceSetLocalEventsFilterDuringSuppressionState(
                source,
                ffi::kCGEventFilterMaskPermitLocalMouseEvents
                    | ffi::kCGEventFilterMaskPermitSystemDefinedEvents,
                ffi::kCGEventSuppressionStateSuppressionInterval,
            );
        }
        let cmd_down = ffi::CGEventCreateKeyboardEvent(source, ffi::kVK_Command, true);
        let down = ffi::CGEventCreateKeyboardEvent(source, key, true);
        let up = ffi::CGEventCreateKeyboardEvent(source, key, false);
        let cmd_up = ffi::CGEventCreateKeyboardEvent(source, ffi::kVK_Command, false);
        let events = [cmd_down, down, up, cmd_up];
        let ok = events.iter().all(|e| !e.is_null());
        if ok {
            ffi::CGEventSetFlags(cmd_down, ffi::kCGEventFlagMaskCommand);
            ffi::CGEventSetFlags(down, ffi::kCGEventFlagMaskCommand);
            ffi::CGEventSetFlags(up, ffi::kCGEventFlagMaskCommand);
            ffi::CGEventSetFlags(cmd_up, 0);
            for (i, e) in events.iter().enumerate() {
                if i > 0 {
                    std::thread::sleep(Duration::from_millis(if i == 2 { 8 } else { 4 }));
                }
                ffi::CGEventPost(ffi::kCGSessionEventTap, *e);
            }
        }
        for obj in events.into_iter().chain([source]) {
            if !obj.is_null() {
                ffi::CFRelease(obj.cast_const());
            }
        }
        if !ok {
            return Err(AppError::msg("创建按键事件失败"));
        }
    }
    Ok(())
}

pub fn send_paste() -> AppResult<()> {
    send_command(ffi::kVK_ANSI_V)
}

/// 在鼠标现在的位置点一下左键（拖卡片粘贴：松手的地方先点一下，那个窗口拿到焦点、光标落到那儿）
pub fn click_here() -> AppResult<()> {
    let at = window_enum::cursor_point().ok_or_else(|| AppError::msg("拿不到鼠标位置"))?;
    click_point(at)
}

/// 调试用（`--demo=drag:`）：用真的鼠标事件在 `points[0]` 按下，依次拖过中间各点，在倒数第二个点松开，
/// 再马上把鼠标挪到最后一个点（模拟用户松手后顺手一挪）。坐标是全局的点。
#[cfg(debug_assertions)]
pub fn debug_drag(points: &[(f64, f64)]) -> AppResult<()> {
    permissions::ensure_accessibility()?;
    if points.len() < 3 {
        return Err(AppError::msg("至少要三个点"));
    }
    let post = |kind: u32, p: (f64, f64)| unsafe {
        let e = ffi::CGEventCreateMouseEvent(std::ptr::null_mut(), kind, ffi::CGPoint::new(p.0, p.1), ffi::kCGMouseButtonLeft);
        if !e.is_null() {
            if kind != ffi::kCGEventMouseMoved {
                ffi::CGEventSetIntegerValueField(e, ffi::kCGMouseEventClickState, 1);
            }
            ffi::CGEventSetFlags(e, 0);
            ffi::CGEventPost(ffi::kCGSessionEventTap, e);
            ffi::CFRelease(e.cast_const());
        }
    };
    let glide = |kind: u32, a: (f64, f64), b: (f64, f64), steps: u32, ms: u64| {
        for i in 1..=steps {
            let t = f64::from(i) / f64::from(steps);
            post(kind, (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            std::thread::sleep(Duration::from_millis(ms));
        }
    };
    let n = points.len();
    post(ffi::kCGEventMouseMoved, points[0]);
    std::thread::sleep(Duration::from_millis(80));
    post(ffi::kCGEventLeftMouseDown, points[0]);
    std::thread::sleep(Duration::from_millis(120));
    for w in points[..n - 1].windows(2) {
        glide(ffi::kCGEventLeftMouseDragged, w[0], w[1], 25, 16);
        std::thread::sleep(Duration::from_millis(120));
    }
    post(ffi::kCGEventLeftMouseUp, points[n - 2]);
    std::thread::sleep(Duration::from_millis(30));
    glide(ffi::kCGEventMouseMoved, points[n - 2], points[n - 1], 6, 12);
    Ok(())
}

/// 调试用（`--demo=wheel:x,y,次数,每次的量,单位,间隔毫秒`）：指针挪到 (x, y)（全局点），连着发滚轮事件。
/// 单位 0 = 像素（触控板那种）、1 = 行（鼠标滚轮那种）
#[cfg(debug_assertions)]
pub fn debug_wheel(at: (f64, f64), count: u32, delta: i32, units: u32, every_ms: u64) -> AppResult<()> {
    permissions::ensure_accessibility()?;
    unsafe {
        let e = ffi::CGEventCreateMouseEvent(std::ptr::null_mut(), ffi::kCGEventMouseMoved, ffi::CGPoint::new(at.0, at.1), ffi::kCGMouseButtonLeft);
        if !e.is_null() {
            ffi::CGEventSetFlags(e, 0);
            ffi::CGEventPost(ffi::kCGSessionEventTap, e);
            ffi::CFRelease(e.cast_const());
        }
        std::thread::sleep(Duration::from_millis(120));
        for k in 0..count {
            // 滚的同时鼠标左右晃一点（像真人滚的时候手在动），悬停会跟着换卡片
            if let Ok(jiggle) = std::env::var("CHENOCR_WHEEL_JIGGLE").map(|v| v.parse::<f64>().unwrap_or(0.0)) {
                let dx = if k % 2 == 0 { jiggle } else { -jiggle };
                let m = ffi::CGEventCreateMouseEvent(std::ptr::null_mut(), ffi::kCGEventMouseMoved, ffi::CGPoint::new(at.0 + dx, at.1), ffi::kCGMouseButtonLeft);
                if !m.is_null() {
                    ffi::CGEventSetFlags(m, 0);
                    ffi::CGEventPost(ffi::kCGSessionEventTap, m);
                    ffi::CFRelease(m.cast_const());
                }
            }
            let delta = if std::env::var_os("CHENOCR_WHEEL_VARY").is_some() { delta * (1 + (k as i32 * 7) % 12) } else { delta };
            let e = ffi::CGEventCreateScrollWheelEvent2(std::ptr::null_mut(), units, 1, delta, 0, 0);
            if e.is_null() {
                return Err(AppError::msg("建不出滚轮事件"));
            }
            ffi::CGEventSetFlags(e, 0);
            ffi::CGEventPost(ffi::kCGSessionEventTap, e);
            ffi::CFRelease(e.cast_const());
            std::thread::sleep(Duration::from_millis(every_ms));
        }
    }
    Ok(())
}

/// 调试用（`--demo=key:键码,修饰`）：真的按一下某个键（修饰是 CGEventFlags 的值，比如 ⌥ = 524288），
/// 用来唤出别的程序（对照它们的界面）
#[cfg(debug_assertions)]
pub fn debug_key(key: u16, flags: u64) -> AppResult<()> {
    permissions::ensure_accessibility()?;
    unsafe {
        for down in [true, false] {
            let e = ffi::CGEventCreateKeyboardEvent(std::ptr::null_mut(), key, down);
            if e.is_null() {
                return Err(AppError::msg("建不出按键事件"));
            }
            ffi::CGEventSetFlags(e, flags);
            ffi::CGEventPost(ffi::kCGSessionEventTap, e);
            ffi::CFRelease(e.cast_const());
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    Ok(())
}

/// 调试用（`--demo=click:x,y,修饰`）：在全局点坐标 `(x, y)` 带着修饰键点一下左键（⌘ 点选多张卡片对照用）
#[cfg(debug_assertions)]
pub fn debug_click(at: (f64, f64), flags: u64) -> AppResult<()> {
    permissions::ensure_accessibility()?;
    let p = ffi::CGPoint::new(at.0, at.1);
    unsafe {
        for kind in [ffi::kCGEventMouseMoved, ffi::kCGEventLeftMouseDown, ffi::kCGEventLeftMouseUp] {
            let e = ffi::CGEventCreateMouseEvent(std::ptr::null_mut(), kind, p, ffi::kCGMouseButtonLeft);
            if e.is_null() {
                return Err(AppError::msg("建不出鼠标事件"));
            }
            if kind != ffi::kCGEventMouseMoved {
                ffi::CGEventSetIntegerValueField(e, ffi::kCGMouseEventClickState, 1);
            }
            ffi::CGEventSetFlags(e, flags);
            ffi::CGEventPost(ffi::kCGSessionEventTap, e);
            ffi::CFRelease(e.cast_const());
            std::thread::sleep(Duration::from_millis(60));
        }
    }
    Ok(())
}

/// 在屏幕物理坐标 `(x, y)` 点一下左键（指针会跳到那儿）
pub fn click_at(x: i32, y: i32) -> AppResult<()> {
    let screens = geometry::screens();
    let at = geometry::rect_to_points_in(&screens, crate::platform::PhysicalRect::new(x, y, 1, 1)).origin;
    click_point(at)
}

fn click_point(at: ffi::CGPoint) -> AppResult<()> {
    permissions::ensure_accessibility()?;
    // SAFETY: 事件在本函数内创建并释放。
    unsafe {
        let down = ffi::CGEventCreateMouseEvent(std::ptr::null_mut(), ffi::kCGEventLeftMouseDown, at, ffi::kCGMouseButtonLeft);
        let up = ffi::CGEventCreateMouseEvent(std::ptr::null_mut(), ffi::kCGEventLeftMouseUp, at, ffi::kCGMouseButtonLeft);
        let ok = !down.is_null() && !up.is_null();
        if ok {
            // 标成"第一下点击"：不带的话是 0，有的程序（网页内核）不当它是点击
            ffi::CGEventSetIntegerValueField(down, ffi::kCGMouseEventClickState, 1);
            ffi::CGEventSetIntegerValueField(up, ffi::kCGMouseEventClickState, 1);
            // 不带修饰键：事件默认沿用系统记的按键状态，用户刚好按着 ⌘ 时会变成 ⌘+点击
            ffi::CGEventSetFlags(down, 0);
            ffi::CGEventSetFlags(up, 0);
            ffi::CGEventPost(ffi::kCGSessionEventTap, down);
            std::thread::sleep(Duration::from_millis(12));
            ffi::CGEventPost(ffi::kCGSessionEventTap, up);
        }
        for obj in [down, up] {
            if !obj.is_null() {
                ffi::CFRelease(obj.cast_const());
            }
        }
        if !ok {
            return Err(AppError::msg("创建鼠标事件失败"));
        }
    }
    Ok(())
}

pub fn send_copy() -> AppResult<()> {
    send_command(ffi::kVK_ANSI_C)
}
