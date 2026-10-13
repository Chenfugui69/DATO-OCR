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

/// 设置页调划词面板外观时的预览：在主窗口旁边亮出真的面板 / 收起。
#[tauri::command]
pub async fn translate_popup_preview(app: AppHandle, show: bool) -> AppResult<()> {
    crate::translate::selection::preview_popup(&app, show);
    Ok(())
}

/// 存 / 清掉某个翻译源要填的一项（密钥、应用 ID、区域……）。
#[tauri::command]
pub async fn translate_set_field(
    app: AppHandle,
    provider: String,
    field: String,
    value: Option<String>,
) -> AppResult<()> {
    super::blocking(move || translate::set_field(&app, &provider, &field, value.as_deref())).await
}

/// 划词面板上方预留了多高（逻辑像素）
#[tauri::command]
pub async fn translate_popup_reserve() -> AppResult<f64> {
    Ok(crate::translate::selection::current_reserve())
}
