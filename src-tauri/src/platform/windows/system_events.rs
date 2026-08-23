//! 监听那些会让抓屏热身状态失效的系统事件（Windows）。
//!
//! # 为什么需要这个
//!
//! `ScreenCapture::warm_up` 在启动时空跑一次抓屏，把 D3D 设备创建和 WGC 会话建立
//! 的开销（实测 137ms vs 热态 49ms）挪到启动期。但这份 GPU 侧状态不是永久的，
//! 下面三件事之后可能已经失效，而**失效是沉默的** —— 用户只会偶然遇到一次慢 F1，
//! 日志里除了 `capture_ms` 变大之外没有任何线索。
//!
//! | 时机 | 消息 | 说明 |
//! |---|---|---|
//! | 显示器拓扑/分辨率变化 | `WM_DISPLAYCHANGE` | 插拔屏、改分辨率、改缩放、切换投影 |
//! | 睡眠 / 休眠唤醒 | `WM_POWERBROADCAST` + `PBT_APMRESUME*` | 唤醒后 GPU 侧对象通常已经没了 |
//! | 显卡驱动 TDR（超时重置） | —— 见下 | 驱动被重启，所有 D3D 设备失效 |
//!
//! # TDR 没有专门的窗口消息
//!
//! TDR 的正规检测方式是自己持有 D3D 设备、拿 `DXGI_ERROR_DEVICE_REMOVED` 之后查
//! `GetDeviceRemovedReason`。但设备在 xcap 内部，我们碰不到，所以**这里做不到直接
//! 检测 TDR**，不假装能。
//!
//! 兜底靠两点：
//!
//! 1. 驱动重置会重建桌面，实践中几乎总会跟一个 `WM_DISPLAYCHANGE`，所以多数 TDR
//!    被上面那条覆盖到了。
//! 2. 真漏掉的话，代价上限是**一次**慢抓屏 —— 那次慢抓屏本身就把状态重新热起来了，
//!    所以事后再热身没有意义。为了不让这种情况继续沉默，`capture` 模块会在抓屏
//!    耗时异常时打一条 warn（见 `capture::mod` 里的 `COLD_CAPTURE_HINT_MS`）。
//!
//! # 为什么是一个隐藏的顶层窗口，而不是 message-only 窗口
//!
//! `HWND_MESSAGE` 窗口**收不到广播消息**，而 `WM_DISPLAYCHANGE` 和
//! `WM_POWERBROADCAST` 正是广播给顶层窗口的。所以这里必须是真正的顶层窗口，
//! 只是永远不显示（不调 `ShowWindow`）。
//!
//! 也没有复用底图窗口：底图是每块屏一个，广播会来 N 次，还得去重；而且那个模块的
//! 职责是显示画面，不该顺手兼职系统事件总线。

use std::sync::OnceLock;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassExW, PBT_APMRESUMEAUTOMATIC,
    PBT_APMRESUMESUSPEND, WM_DISPLAYCHANGE, WM_POWERBROADCAST, WNDCLASSEXW, WS_OVERLAPPED,
};

const CLASS_NAME: PCWSTR = w!("ChenocrSystemEvents");

/// 失效通知的去处。只装一次，之后不可变。
static HANDLER: OnceLock<fn()> = OnceLock::new();

/// 监听窗口。只建一次；重复调用是幂等的。
static INSTALLED: OnceLock<()> = OnceLock::new();

pub fn on_capture_state_lost(handler: fn()) {
    if HANDLER.set(handler).is_err() {
        tracing::warn!("抓屏失效回调已经装过了，忽略这次");
        return;
    }

    if INSTALLED.get().is_some() {
        return;
    }

    match create_window() {
        Ok(_) => {
            let _ = INSTALLED.set(());
            tracing::debug!("系统事件监听窗口已建立（显示变化 / 电源唤醒）");
        }
        // 装不上只是失去"重新热身"这个优化，不影响任何功能，所以不往上抛。
        Err(err) => tracing::warn!("系统事件监听窗口建立失败，热身状态失效后不会自动补：{err}"),
    }
}

fn create_window() -> windows::core::Result<HWND> {
    // SAFETY: 传 None 取当前进程模块句柄，这个调用不会失败。
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }?.into();

    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };

    // 重复注册会返回 0 并把 last error 设成 ERROR_CLASS_ALREADY_EXISTS。
    // 这里不检查返回值：真正会失败的是下面的 CreateWindowExW，查它就够了。
    // SAFETY: class 里的字段都有效，lpszClassName 是静态宽字符串。
    unsafe { RegisterClassExW(&class) };

    // 父窗口传 None，**不能**传 HWND_MESSAGE —— message-only 窗口收不到
    // WM_DISPLAYCHANGE / WM_POWERBROADCAST 这类广播消息。这里要的是真正的顶层
    // 窗口，只是永远不调 ShowWindow，所以用户看不到。
    //
    // SAFETY: 类已注册；窗口不显示，尺寸给 0 即可。
    unsafe {
        CreateWindowExW(
            Default::default(),
            CLASS_NAME,
            w!("CHENOCR System Events"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    }
}

/// 只关心"热身状态可能没了"这一件事，其余一律交回系统。
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let reason = match msg {
        WM_DISPLAYCHANGE => Some("显示配置变化"),
        WM_POWERBROADCAST => {
            let event = u32::try_from(wparam.0).unwrap_or(u32::MAX);
            if event == PBT_APMRESUMEAUTOMATIC || event == PBT_APMRESUMESUSPEND {
                Some("从睡眠唤醒")
            } else {
                None
            }
        }
        _ => None,
    };

    if let Some(reason) = reason {
        if let Some(handler) = HANDLER.get() {
            tracing::info!(reason, "抓屏热身状态可能已失效，重新热身");
            handler();
        }
    }

    // SAFETY: 转发给系统默认处理，参数原样传递。
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
