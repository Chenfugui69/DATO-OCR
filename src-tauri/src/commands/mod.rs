//! 所有 `#[tauri::command]` 的集合。
//!
//! 命名规范（规格 00 §6.1）：`snake_case`，动词开头，域名前缀。
//! 前端一律通过 `src/lib/ipc.ts` 的封装调用，不直接 `invoke`。

pub mod capture;
pub mod system;
