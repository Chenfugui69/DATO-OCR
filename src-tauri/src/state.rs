//! 全局状态。
//!
//! 规格 01 §4.1 列出了完整的 `AppState`（含 db / settings / ocr / pinned_windows
//! 等）。这里只放**已经落地的模块**用到的字段，其余随各自里程碑补上。
//!
//! 锁一律用 `parking_lot`，不用 std 的。**禁止在持锁状态下 await。**

use parking_lot::Mutex;
use tracing_appender::non_blocking::WorkerGuard;

use crate::capture::CaptureSession;
use crate::paths::AppPaths;

pub struct AppState {
    pub paths: AppPaths,
    pub capture: Mutex<Option<CaptureSession>>,
    /// 非阻塞日志的写线程句柄，必须活到进程结束，否则日志会被吞掉。
    _log_guard: Option<WorkerGuard>,
}

impl AppState {
    pub fn new(paths: AppPaths, log_guard: Option<WorkerGuard>) -> Self {
        Self {
            paths,
            capture: Mutex::new(None),
            _log_guard: log_guard,
        }
    }
}
