//! 设置、外观、热键、密钥、应用信息。

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use super::blocking;
use crate::error::{AppError, AppResult};
use crate::hotkeys::{self, HotkeyStatus};
use crate::settings::Settings;
use crate::state::state;
use crate::storage::secrets;
use crate::translate::{SECRET_DEEPL, SECRET_OPENAI};
use crate::wm::{self, VisualCapabilities};
use crate::{ocr, translate, tray};

#[tauri::command]
pub async fn settings_get(app: AppHandle) -> AppResult<Settings> {
    Ok(state(&app).settings())
}

#[tauri::command]
pub async fn settings_set(app: AppHandle, settings: Settings) -> AppResult<Settings> {
    let st = state(&app);
    let before = st.settings();
    let next = st.update_settings(&app, settings)?;

    if before.hotkeys != next.hotkeys {
        let ui = app.clone();
        app.run_on_main_thread(move || {
            hotkeys::register_all(&ui);
            tray::rebuild(&ui);
        })?;
    }
    if before.general.auto_start != next.general.auto_start {
        let manager = app.autolaunch();
        let result = if next.general.auto_start {
            manager.enable()
        } else {
            manager.disable()
        };
        if let Err(err) = result {
            tracing::warn!("设置开机自启失败：{err}");
        }
    }
    if before.translate.selection != next.translate.selection {
        translate::selection::reconfigure(&app);
    }
    if before.appearance != next.appearance {
        let ui = app.clone();
        app.run_on_main_thread(move || wm::refresh_visuals(&ui))?;
    }
    Ok(next)
}

#[tauri::command]
pub async fn settings_reset(app: AppHandle) -> AppResult<Settings> {
    settings_set(app, Settings::default()).await
}

#[tauri::command]
pub async fn visuals_get(app: AppHandle) -> AppResult<VisualCapabilities> {
    Ok(wm::visual_capabilities(&app))
}

#[tauri::command]
pub async fn hotkeys_status(app: AppHandle) -> AppResult<Vec<HotkeyStatus>> {
    Ok(state(&app).hotkeys.status())
}

/// 录入快捷键期间暂停全部热键，避免按下的组合直接触发功能。
#[tauri::command]
pub async fn hotkeys_suspend(app: AppHandle) -> AppResult<()> {
    let ui = app.clone();
    app.run_on_main_thread(move || hotkeys::suspend(&ui))?;
    Ok(())
}

#[tauri::command]
pub async fn hotkeys_resume(app: AppHandle) -> AppResult<Vec<HotkeyStatus>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let ui = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(hotkeys::register_all(&ui));
    })?;
    rx.await.map_err(|_| AppError::msg("注册热键失败"))
}

#[tauri::command]
pub async fn hotkey_validate(accelerator: String) -> AppResult<()> {
    hotkeys::validate(&accelerator).map_err(AppError::Msg)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsStatus {
    pub deepl: Option<String>,
    pub openai: Option<String>,
}

#[tauri::command]
pub async fn secrets_status(app: AppHandle) -> AppResult<SecretsStatus> {
    let st = state(&app);
    let get = |k: &str| -> Option<String> {
        st.db
            .with(|c| secrets::get(c, k))
            .ok()
            .flatten()
            .map(|s| secrets::masked(&s))
    };
    Ok(SecretsStatus {
        deepl: get(SECRET_DEEPL),
        openai: get(SECRET_OPENAI),
    })
}

#[tauri::command]
pub async fn secret_set(app: AppHandle, key: String, value: Option<String>) -> AppResult<()> {
    let key = match key.as_str() {
        "deepl" => SECRET_DEEPL,
        "openai" => SECRET_OPENAI,
        _ => return Err(AppError::msg("未知密钥")),
    };
    state(&app)
        .db
        .with(|c| secrets::set(c, key, value.as_deref()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub save_dir: String,
    pub ocr: ocr::EngineStatus,
    pub data_bytes: u64,
}

fn dir_size(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// 前端拼 asset 协议地址要用的数据目录绝对路径（廉价，启动时调一次）。
#[tauri::command]
pub async fn data_dir(app: AppHandle) -> AppResult<String> {
    Ok(state(&app).paths.root().display().to_string())
}

#[tauri::command]
pub async fn app_info(app: AppHandle) -> AppResult<AppInfo> {
    blocking(move || {
        let st = state(&app);
        let settings = st.settings.read().capture.clone();
        Ok(AppInfo {
            version: app.package_info().version.to_string(),
            data_dir: st.paths.root().display().to_string(),
            save_dir: crate::capture::save_directory(&settings)
                .display()
                .to_string(),
            ocr: ocr::status(&app),
            data_bytes: dir_size(st.paths.root()),
        })
    })
    .await
}

#[tauri::command]
pub async fn open_data_folder(app: AppHandle, which: String) -> AppResult<()> {
    let st = state(&app);
    let path = match which.as_str() {
        "logs" => st.paths.logs(),
        "save" => {
            let settings = st.settings.read().capture.clone();
            let dir = crate::capture::save_directory(&settings);
            std::fs::create_dir_all(&dir)?;
            dir
        }
        _ => st.paths.root().to_path_buf(),
    };
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| AppError::msg(e.to_string()))
}

#[tauri::command]
pub async fn open_url(app: AppHandle, url: String) -> AppResult<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(AppError::msg("只能打开网页链接"));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::msg(e.to_string()))
}

#[tauri::command]
pub async fn pick_directory(app: AppHandle) -> AppResult<Option<String>> {
    blocking(move || {
        let picked = app
            .dialog()
            .file()
            .set_title("选择文件夹")
            .blocking_pick_folder();
        Ok(picked
            .and_then(|p| p.into_path().ok())
            .map(|p| p.display().to_string()))
    })
    .await
}

#[tauri::command]
pub async fn show_main(app: AppHandle, page: Option<String>) -> AppResult<()> {
    let ui = app.clone();
    app.run_on_main_thread(move || wm::show_main(&ui, page.as_deref()))?;
    Ok(())
}

/// toast 全部消失后由前端调用：藏窗口（诊断模式下顺带恢复抓屏排除）。
#[tauri::command]
pub async fn toast_hide(app: AppHandle) -> AppResult<()> {
    if let Some(w) = tauri::Manager::get_webview_window(&app, wm::TOAST) {
        let _ = w.hide();
        crate::platform::reveal_for_tests(&w, false);
    }
    Ok(())
}

#[tauri::command]
pub async fn quit_app(app: AppHandle) -> AppResult<()> {
    crate::quit(&app);
    Ok(())
}

/// 前端异常写进 Rust 日志。截图遮罩是隐藏窗口且没有 devtools，是全项目最深的调试
/// 盲区，里面的异常在界面上完全不可见，只表现为"按 F1 没反应"。
#[tauri::command]
pub fn report_error(window: tauri::Window, scope: String, message: String) {
    tracing::error!(window = window.label(), %scope, "前端错误：{message}");
}
