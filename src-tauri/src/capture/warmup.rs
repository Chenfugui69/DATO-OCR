//! 抓屏热身策略。
//!
//! 首次抓屏要建 D3D 设备、开 WGC 会话，实测 137ms，而热态只要 49ms —— 差的这
//! 90ms 刚好让第一次 F1 从 79ms 冲到 163ms，突破规格 00 §6.4 的 150ms 预算。
//! 所以启动后空跑一次抓屏，把这笔钱在用户按键之前付掉。
//!
//! 但这份 GPU 侧状态**会失效**（插拔屏、改分辨率、睡眠唤醒、显卡驱动 TDR），
//! 失效之后 137ms 会悄悄回来，且日志里除了 `capture_ms` 变大没有别的线索。
//! 所以除了启动那次，还要在失效时机各补一次 —— 由
//! [`platform::on_capture_state_lost`] 提供这些时机，具体覆盖到哪些事件、
//! 以及为什么 TDR 只能间接覆盖，见 `platform/windows/system_events.rs`。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::platform;

/// 超过这个耗时就当作"热身状态已经丢了"。
///
/// 实测（3840×2160 单屏）热态 48–53ms、冷态 137ms，中间空档很宽，取 100ms 两边
/// 都不贴边。多屏是逐块抓的，阈值要按屏数放大，见 [`note_capture_cost`]。
const COLD_CAPTURE_HINT_MS: u128 = 100;

/// 正在热身。用来吞掉抖动：插拔一次屏可能连发好几个 `WM_DISPLAYCHANGE`，
/// 没必要为每一个都空跑一次 137ms 的抓屏。
static RUNNING: AtomicBool = AtomicBool::new(false);

/// 装好热身策略：立刻热一次，并订阅之后的失效时机。
///
/// 幂等由 [`platform::on_capture_state_lost`] 那边保证（重复订阅会被忽略）。
pub fn install() {
    schedule("启动");
    platform::on_capture_state_lost(|| schedule("状态失效"));
}

/// 丢一个热身到后台线程。
///
/// 必须是后台线程：热身要上百毫秒，而调用方可能是 UI 线程（系统事件的窗口过程就在
/// UI 线程上），在那里同步跑会卡住整个界面。
fn schedule(reason: &'static str) {
    // compare_exchange 而不是 load+store：两个 WM_DISPLAYCHANGE 可能同时进来。
    if RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        tracing::debug!(reason, "已有热身在跑，跳过这次");
        return;
    }

    std::thread::spawn(move || {
        tracing::debug!(reason, "开始抓屏热身");
        platform::screen_capture().warm_up();
        RUNNING.store(false, Ordering::Release);
    });
}

/// 记一次真实抓屏的耗时，慢得像冷抓就喊一声。
///
/// 这条是**观测**，不是补救：那次慢抓屏本身已经把状态重新热起来了，事后再热身没有
/// 意义。它存在的唯一目的是别让"热身失效"继续沉默 —— 尤其是 TDR，我们没法直接
/// 检测（原因见 `platform/windows/system_events.rs`），只能靠这条日志暴露出来。
///
/// 阈值按屏数放大：多屏是逐块抓的，两块 4K 热态本来就要 100ms 上下。屏数由调用方
/// 传进来 —— 它手上已经有了，这里再问一次 `list_monitors()` 是白花的热路径开销。
pub fn note_capture_cost(elapsed: Duration, monitors: usize) {
    let budget = COLD_CAPTURE_HINT_MS * monitors.max(1) as u128;

    if elapsed.as_millis() > budget {
        tracing::warn!(
            capture_ms = elapsed.as_millis(),
            budget_ms = budget,
            monitors,
            "抓屏耗时像是冷启动，热身状态可能在某个没覆盖到的时机丢了（显卡驱动 TDR？）"
        );
    }
}
