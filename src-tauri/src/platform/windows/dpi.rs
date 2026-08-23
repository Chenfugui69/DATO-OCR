//! 进程级 DPI 声明。
//!
//! 规格 07 §4.1：没有这一句，混合 DPI 环境下所有坐标都是错的。
//! 另外 xcap 的 `scale_factor()` 内部会先检查进程是否 DPI-aware，不 aware 时
//! 会退化成 `GetDeviceCaps` 的估算值。

use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

pub fn declare_per_monitor_v2() {
    // SAFETY: 纯进程级开关，无指针参数、无资源所有权转移。
    let result = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    // Tauri/tao 自己也会声明一次。谁先谁后都行，后来者会拿到
    // ERROR_ACCESS_DENIED，这不是故障，最终的感知级别是一样的。
    if let Err(err) = result {
        tracing::debug!(
            "SetProcessDpiAwarenessContext 未生效（通常是已被声明过）: {err}"
        );
    }
}
