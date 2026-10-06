//! 翻译命令。

use tauri::AppHandle;

use crate::error::AppResult;
use crate::translate::{self, detect, selection, ProviderInfo, TranslateRequest, TranslateResult};

#[tauri::command]
pub async fn translate_text(
    app: AppHandle,
    request: TranslateRequest,
) -> AppResult<TranslateResult> {
    translate::translate(&app, request).await
}

#[tauri::command]
pub async fn translate_providers(app: AppHandle) -> AppResult<Vec<ProviderInfo>> {
    Ok(translate::list_providers(&app))
}

#[tauri::command]
pub async fn translate_detect(text: String) -> AppResult<String> {
    Ok(detect::detect(&text).to_string())
}

/// 划词翻译气泡当前要翻译的文字。
#[tauri::command]
pub async fn translate_popup_text() -> AppResult<Option<String>> {
    Ok(selection::current())
}

/// 划词面板上方预留了多高（逻辑像素）
#[tauri::command]
pub async fn translate_popup_reserve() -> AppResult<f64> {
    Ok(crate::translate::selection::current_reserve())
}
