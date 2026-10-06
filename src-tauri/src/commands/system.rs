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

/// 设置一次只改一个：拖滑杆、取色时前端会连着发好几次，同时写同一个临时文件会互相
/// 踩掉（"系统找不到指定的文件"），前后比较也会错乱。
static SETTINGS_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Rust 这边改设置（和界面保存走同一把锁，不会互相覆盖）。
pub(crate) fn update_settings_with(
    app: &AppHandle,
    f: impl FnOnce(&mut Settings),
) -> AppResult<Settings> {
    let st = state(app);
    let _guard = SETTINGS_LOCK.lock();
    let mut next = st.settings();
    f(&mut next);
    st.update_settings(app, next)
}

#[tauri::command]
pub async fn settings_set(app: AppHandle, settings: Settings) -> AppResult<Settings> {
    let st = state(&app);
    let (before, next) = {
        let _guard = SETTINGS_LOCK.lock();
        let before = st.settings();
        let next = st.update_settings(&app, settings)?;
        (before, next)
    };

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
    if before.clipboard.panel_blur != next.clipboard.panel_blur
        || before.clipboard.panel_docked != next.clipboard.panel_docked
        || before.clipboard.panel_style != next.clipboard.panel_style
    {
        let ui = app.clone();
        app.run_on_main_thread(move || crate::clipboard::panel::apply_material(&ui))?;
    }
    if before.translate.selection != next.translate.selection {
        translate::selection::reconfigure(&app);
    }
    if before.sync != next.sync {
        // 开关局域网、换端口、改设备名、网盘间隔…：按新设置重开（起服务、mDNS 都可能慢一点，别卡住保存）
        let ui = app.clone();
        std::thread::spawn(move || crate::sync::reconfigure(&ui));
    }
    if before.translate.popup.blur != next.translate.popup.blur
        || before.translate.popup.radius != next.translate.popup.radius
    {
        let ui = app.clone();
        app.run_on_main_thread(move || translate::selection::apply_popup_material(&ui))?;
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
/// 显示时走的是原生"显示但不激活"，隐藏也要走原生的，只调 Tauri 的 `hide()` 窗口会一直挂着。
#[tauri::command]
pub async fn toast_hide(app: AppHandle) -> AppResult<()> {
    if let Some(w) = tauri::Manager::get_webview_window(&app, wm::TOAST) {
        crate::platform::hide_window(&w);
        crate::platform::reveal_for_tests(&w, false);
    }
    Ok(())
}

/// 调用方窗口一次同时改位置和大小（逻辑像素）。划词面板展开 / 收起 AI 时用，
/// 让窗口一步到位，动画全在页面里做。
#[tauri::command]
pub async fn window_set_bounds(
    window: tauri::WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    backdrop: Option<BackdropRect>,
) -> AppResult<()> {
    let s = window.scale_factor()?;
    let (px, py) = ((x * s).round() as i32, (y * s).round() as i32);
    let (pw, ph) = (
        (width * s).round().max(1.0) as u32,
        (height * s).round().max(1.0) as u32,
    );
    let Some(b) = backdrop else {
        return crate::platform::set_bounds(&window, px, py, pw, ph);
    };
    // 毛玻璃面板展开 / 收起：窗口挪动和背板范围在 UI 线程上一起改，系统合成时是同一帧
    let (tx, rx) = tokio::sync::oneshot::channel();
    let w = window.clone();
    window.run_on_main_thread(move || {
        crate::platform::set_backdrop_rect(&w, Some(b.physical(s)), 0);
        let _ = tx.send(crate::platform::set_bounds(&w, px, py, pw, ph));
    })?;
    rx.await
        .map_err(|_| crate::error::AppError::msg("窗口操作被取消"))?
}

/// 窗口里的一块（逻辑像素）。
#[derive(serde::Deserialize, Clone, Copy)]
pub struct BackdropRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl BackdropRect {
    fn physical(self, s: f64) -> (f32, f32, f32, f32) {
        (
            (self.x * s) as f32,
            (self.y * s) as f32,
            (self.width * s) as f32,
            (self.height * s) as f32,
        )
    }
}

/// 窗口只保留一块（逻辑像素）：外面不显示、点击穿透；`rect` 为空 = 整个窗口。
#[tauri::command]
pub async fn window_set_region(
    window: tauri::WebviewWindow,
    rect: Option<BackdropRect>,
) -> AppResult<()> {
    let s = window.scale_factor()?;
    let rect = rect.map(|r| {
        let (x, y, w, h) = r.physical(s);
        (
            x.round() as i32,
            y.round() as i32,
            w.round() as i32,
            h.round() as i32,
        )
    });
    crate::platform::set_rect_region(&window, rect);
    Ok(())
}

/// 毛玻璃背板只铺窗口里的一块，`ms` > 0 时动画过去（和页面同一条缓动曲线）；`rect` 为空 = 铺满窗口。
#[tauri::command]
pub async fn window_backdrop(
    window: tauri::WebviewWindow,
    rect: Option<BackdropRect>,
    ms: u32,
) -> AppResult<()> {
    let s = window.scale_factor()?;
    let w = window.clone();
    window.run_on_main_thread(move || {
        crate::platform::set_backdrop_rect(&w, rect.map(|r| r.physical(s)), ms);
    })?;
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

/// 调用方窗口现在是不是前台窗口（剪贴板面板关掉右键菜单后用它判断要不要收起）。
#[tauri::command]
pub async fn window_is_foreground(window: tauri::WebviewWindow) -> AppResult<bool> {
    let own = crate::platform::native_handle(&window)?;
    Ok(crate::platform::foreground_window().is_some_and(|h| h.0 == own))
}
