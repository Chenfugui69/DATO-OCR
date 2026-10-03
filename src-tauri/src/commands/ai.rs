//! AI 对话命令。

use std::collections::HashMap;

use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::ai::{self, AiContext, ChatEvent, ChatRequest};
use crate::error::AppResult;

#[tauri::command]
pub async fn ai_models(app: AppHandle, provider_id: String) -> AppResult<Vec<String>> {
    ai::list_models(&app, &provider_id).await
}

#[tauri::command]
pub async fn ai_chat(
    app: AppHandle,
    request: ChatRequest,
    on_event: Channel<ChatEvent>,
) -> AppResult<()> {
    ai::chat(app, request, on_event).await
}

#[tauri::command]
pub async fn ai_cancel(id: String) -> AppResult<()> {
    ai::cancel(&id);
    Ok(())
}

#[tauri::command]
pub async fn ai_keys(app: AppHandle) -> AppResult<HashMap<String, String>> {
    Ok(ai::masked_keys(&app))
}

#[tauri::command]
pub async fn ai_set_key(app: AppHandle, provider_id: String, key: Option<String>) -> AppResult<()> {
    ai::set_key(&app, &provider_id, key.as_deref())
}

#[tauri::command]
pub async fn ai_open(app: AppHandle, context: AiContext) -> AppResult<()> {
    ai::open_window(&app, context);
    Ok(())
}

#[tauri::command]
pub async fn ai_context() -> AppResult<Option<AiContext>> {
    Ok(ai::current_context())
}
