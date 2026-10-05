//! 全部 `#[tauri::command]`。前端对应的类型安全封装在 `src/lib/ipc.ts`，
//! 组件里禁止直接调 `invoke`。
//!
//! 约定：凡是可能碰磁盘/网络/大图的命令都写成 `async`（跑在异步运行时线程池上）；
//! 同步命令会跑在主线程上，慢了会卡住全部窗口。

pub mod ai;
pub mod capture;
pub mod clipboard;
pub mod library;
pub mod sync;
pub mod system;
pub mod tools;

use serde::de::DeserializeOwned;
use tauri::ipc::{InvokeBody, Request};

use crate::error::{AppError, AppResult};

/// 二进制请求：元数据放在 `x-meta` 头里（纯 ASCII 的 JSON），正文是原始字节。
pub(crate) fn raw_request<T: DeserializeOwned>(request: &Request<'_>) -> AppResult<(T, Vec<u8>)> {
    let meta = request
        .headers()
        .get("x-meta")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::msg("缺少请求元数据"))?;
    let meta: T = serde_json::from_str(meta)?;
    let body = match request.body() {
        InvokeBody::Raw(bytes) => bytes.clone(),
        InvokeBody::Json(_) => Vec::new(),
    };
    Ok((meta, body))
}

/// 把阻塞工作挪到专用线程池，别占异步运行时的工作线程。
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::msg(format!("后台任务失败：{e}")))?
}
