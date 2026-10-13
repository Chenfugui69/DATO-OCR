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

/// 用户在对话里自己加的图片（点加号选的、拖进来的、粘贴的）：页面把图转成 PNG / JPEG 的 base64 送过来，
/// 放进内存图库，返回图库里的 id（之后和截图一样随提问发给模型）。只在内存里，不落盘。
#[tauri::command]
pub async fn ai_attach_image(app: AppHandle, data: String) -> AppResult<String> {
    super::blocking(move || {
        use base64::Engine;
        let raw = data.split_once(',').map_or(data.as_str(), |(_, b)| b);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .map_err(|_| crate::error::AppError::msg("图片读不出来"))?;
        let image = image::load_from_memory(&bytes)?.to_rgba8();
        let id = format!("{ATTACH_PREFIX}{}", uuid::Uuid::new_v4().simple());
        crate::state::state(&app)
            .images
            .put(id.clone(), std::sync::Arc::new(image));
        Ok(id)
    })
    .await
}

/// 不要了的图（从输入框里删掉了、对话关了）从内存图库里放掉。只放用户自己加的那些。
#[tauri::command]
pub async fn ai_release_images(app: AppHandle, ids: Vec<String>) -> AppResult<()> {
    let st = crate::state::state(&app);
    for id in ids.iter().filter(|id| id.starts_with(ATTACH_PREFIX)) {
        st.images.remove(id);
    }
    Ok(())
}

const ATTACH_PREFIX: &str = "ai-attach-";

#[tauri::command]
pub async fn ai_context() -> AppResult<Option<AiContext>> {
    Ok(ai::current_context())
}
