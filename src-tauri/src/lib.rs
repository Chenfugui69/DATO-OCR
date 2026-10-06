//! DATO OCR —— 截图 · 识字 · 翻译 · 剪贴板 · AI。（内部代号 chenocr）
//!
//! 启动顺序：
//! 1. `platform::init_process()`：声明 PMv2 DPI 感知（必须在任何窗口之前）
//! 2. `setup`：数据目录 → 日志 → 配置 → 数据库 → 全局状态 → 托盘 → 主窗口
//! 3. `RunEvent::Ready`：注册热键、预建遮罩/面板/toast 窗口、抓屏热身、剪贴板监听
//!    （窗口必须等 Ready 之后再建：`setup()` 里建的窗口拿不到 IPC 初始化脚本）

// 总纲 §6.2 禁止运行时路径上的 unwrap/expect，交给 clippy 拦。测试代码放行。
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod ai;
mod capture;
mod clipboard;
mod commands;
mod editor;
mod error;
mod events;
mod gif_record;
mod hotkeys;
mod image_store;
mod imaging;
mod library;
mod logging;
mod longshot;
mod maintenance;
mod net;
mod ocr;
mod paths;
mod pin;
mod platform;
mod settings;
mod state;
mod storage;
mod sync;
mod translate;
mod tray;
mod update;
mod wm;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

use crate::paths::AppPaths;
use crate::platform::SystemEvent;
use crate::settings::Settings;
use crate::state::{state, AppState};
use crate::storage::Db;

static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn run() {
    platform::init_process();

    let builder = tauri::Builder::default()
        // 单实例必须第一个注册：第二次启动只是把已有实例的主窗口叫出来。
        // `--page=settings:translate` 这样的参数可以直接跳到某一页（快捷方式、测试用）；
        // `--translate=文字` 在鼠标旁边弹出划词面板翻译这段文字（脚本、快捷指令、测试用）
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // 开机自启 / 静默启动撞上已经在跑的实例：什么都不做（不能把主窗口弹出来）
            if args.iter().any(|a| a == "--autostart" || a == "--hidden") {
                return;
            }
            if let Some(text) = args.iter().find_map(|a| a.strip_prefix("--translate=")) {
                if !text.trim().is_empty() {
                    translate::selection::show_popup(app, text.to_string(), None);
                }
                return;
            }
            let page = args.iter().find_map(|a| a.strip_prefix("--page="));
            wm::show_main(app, page);
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(hotkeys::on_shortcut)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .register_asynchronous_uri_scheme_protocol(image_store::SCHEME, image_store::handle)
        .setup(setup)
        .on_window_event(on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::capture::capture_start,
            commands::capture::capture_session_info,
            commands::capture::capture_overlay_ready,
            commands::capture::capture_window_children,
            commands::capture::capture_finish,
            commands::capture::capture_cancel,
            commands::capture::capture_translate,
            commands::capture::clipboard_write_text,
            commands::capture::longshot_set_regions,
            commands::capture::longshot_finish,
            commands::capture::longshot_abort,
            commands::capture::gif_set_regions,
            commands::capture::gif_finish,
            commands::capture::gif_cancel,
            commands::capture::longshot_undo,
            commands::capture::pin_info,
            commands::capture::pin_close,
            commands::capture::pin_close_all,
            commands::capture::pin_copy,
            commands::capture::pin_save,
            commands::capture::pin_ocr,
            commands::capture::editor_current,
            commands::capture::editor_finish,
            commands::capture::editor_close,
            commands::capture::editor_open_shot,
            commands::clipboard::clipboard_query,
            commands::clipboard::clipboard_get,
            commands::clipboard::clipboard_stats,
            commands::clipboard::clipboard_paste,
            commands::clipboard::clipboard_copy,
            commands::clipboard::clipboard_delete,
            commands::clipboard::clipboard_set_pinned,
            commands::clipboard::clipboard_set_favorite,
            commands::clipboard::clipboard_set_note,
            commands::clipboard::clipboard_set_group,
            commands::clipboard::clipboard_groups,
            commands::clipboard::clipboard_create_group,
            commands::clipboard::clipboard_rename_group,
            commands::clipboard::clipboard_delete_group,
            commands::clipboard::clipboard_clear,
            commands::clipboard::clipboard_panel_hide,
            commands::clipboard::clipboard_open,
            commands::sync::sync_status,
            commands::sync::sync_approve,
            commands::sync::sync_remove_peer,
            commands::sync::sync_join,
            commands::sync::sync_cancel_join,
            commands::sync::sync_leave,
            commands::sync::sync_regenerate_code,
            commands::sync::sync_qr,
            commands::sync::sync_webdav_connect,
            commands::sync::sync_webdav_disconnect,
            commands::sync::sync_webdav_now,
            commands::sync::sync_webdav_devices,
            commands::update::update_status,
            commands::update::update_check,
            commands::update::update_install,
            commands::update::update_skip,
            commands::library::library_query,
            commands::library::library_delete,
            commands::library::library_set_favorite,
            commands::library::library_set_note,
            commands::library::library_copy,
            commands::library::library_pin,
            commands::library::library_save_as,
            commands::library::library_reveal,
            commands::library::library_ocr,
            commands::library::ocr_current_job,
            commands::library::ocr_rerun,
            commands::library::ocr_save_text,
            commands::library::ocr_status,
            commands::library::ocr_history,
            commands::library::ocr_open_record,
            commands::library::ocr_delete_record,
            commands::library::ocr_from_clip,
            commands::tools::translate_text,
            commands::tools::translate_providers,
            commands::tools::translate_detect,
            commands::tools::translate_popup_text,
            commands::tools::translate_popup_reserve,
            commands::ai::ai_models,
            commands::ai::ai_chat,
            commands::ai::ai_cancel,
            commands::ai::ai_keys,
            commands::ai::ai_set_key,
            commands::ai::ai_open,
            commands::ai::ai_context,
            commands::system::settings_get,
            commands::system::settings_set,
            commands::system::settings_reset,
            commands::system::visuals_get,
            commands::system::hotkeys_status,
            commands::system::hotkeys_suspend,
            commands::system::hotkeys_resume,
            commands::system::hotkey_validate,
            commands::system::secrets_status,
            commands::system::secret_set,
            commands::system::data_dir,
            commands::system::app_info,
            commands::system::open_data_folder,
            commands::system::open_url,
            commands::system::pick_directory,
            commands::system::show_main,
            commands::system::toast_hide,
            commands::system::window_set_bounds,
            commands::system::window_backdrop,
            commands::system::window_set_region,
            commands::system::quit_app,
            commands::system::report_error,
        ]);

    // 这是 windows_subsystem = "windows" 的 GUI 进程，没有控制台，panic 信息进虚空。
    // 初始化失败时至少往日志里留一笔，再以非零码退出。
    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(err) => {
            tracing::error!("应用初始化失败：{err}");
            eprintln!("DATO OCR 初始化失败：{err}");
            std::process::exit(1);
        }
    };

    app.run(|app, event| match event {
        RunEvent::Ready => on_ready(app),
        // 所有窗口都关了也不退出：常驻托盘
        RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
        RunEvent::Exit => shutdown(app),
        _ => {}
    });
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let _ = APP.set(handle.clone());

    let paths = AppPaths::resolve(&handle)?;
    let settings = Settings::load(&paths.settings_file());
    let log_guard = logging::init(&paths.logs(), false);
    tracing::info!(version = %app.package_info().version, data = %paths.root().display(), "DATO OCR 启动");
    if let Some(old) = &paths.migrated_from {
        tracing::info!(from = %old.display(), "已把旧数据目录搬到新位置");
    }
    if let Err(err) = paths.reset_temp() {
        tracing::warn!("清空 temp 目录失败：{err}");
    }

    let db = match Db::open(&paths.db_file()) {
        Ok(db) => db,
        Err(err) => {
            // 库坏了不能让应用起不来：挪到一边，建个新的
            tracing::error!("打开数据库失败，改名保留后重建：{err}");
            let broken = paths.db_file().with_extension(format!(
                "db.broken.{}",
                chrono::Local::now().format("%Y%m%d%H%M%S")
            ));
            let _ = std::fs::rename(paths.db_file(), broken);
            Db::open(&paths.db_file())?
        }
    };

    app.manage(AppState::new(paths, settings, db, log_guard));
    tray::build(&handle)?;
    Ok(())
}

fn on_ready(app: &AppHandle) {
    let statuses = hotkeys::register_all(app);
    if statuses.iter().any(|s| !s.ok) {
        // 刚退出的旧实例可能还没释放热键（开发时重启、自动更新后重启都会遇到），过一会儿再试一次
        let retry = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let ui = retry.clone();
            let _ = retry.run_on_main_thread(move || {
                hotkeys::register_all(&ui);
            });
        });
    }
    capture::overlay::prewarm(app);
    wm::prewarm_toast(app);
    clipboard::panel::prewarm(app);
    translate::selection::prewarm(app);
    translate::selection::reconfigure(app);
    clipboard::start(app);
    sync::start(app);
    update::start(app);
    platform::watch_system_events(on_system_event);
    std::thread::spawn(platform::warm_up_capture);
    maintenance::spawn(app);
    let ui = app.clone();
    std::thread::spawn(move || after_rename(&ui));

    if let Some(main) = app.get_webview_window(wm::MAIN) {
        wm::apply_window_effects(app, &main);
        // 开机自启时安静地待在托盘里；手动启动时显示主窗口
        let silent = std::env::args().any(|a| a == "--autostart" || a == "--hidden");
        if !silent {
            let _ = main.show();
            let _ = main.set_focus();
        }
    }
}

/// 品牌从 DATO COR 改名为 DATO OCR 之后要补的事（每次启动都检查，做过了就什么都不动）：
/// - 用默认保存位置的：图片\DATO COR 改名成 图片\DATO OCR
/// - 开机自启项按产品名登记，旧名字那一项删掉；设置里开着自启的，用新名字重新登记
fn after_rename(app: &AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    // 测试实例（数据放在别处）不碰用户真正的图片文件夹和开机自启项
    if std::env::var_os("CHENOCR_TEST_DATA_DIR").is_some() {
        return;
    }
    let settings = state(app).settings();
    capture::migrate_save_directory(&settings.capture);
    if platform::remove_autostart_entry("DATO COR") && settings.general.auto_start {
        if let Err(err) = app.autolaunch().enable() {
            tracing::warn!("改名后重新登记开机自启失败：{err}");
        }
    }
}

/// 系统事件回调（在系统事件线程上，必须立刻返回）。
fn on_system_event(event: SystemEvent) {
    static WARMING: AtomicBool = AtomicBool::new(false);
    let Some(app) = APP.get() else { return };
    match event {
        SystemEvent::DisplayChanged | SystemEvent::Resumed => {
            // 插拔屏 / 改分辨率 / 睡眠唤醒后 GPU 侧的抓屏热身状态可能已经没了，
            // 首次 F1 会悄悄慢回 137ms。重新热一次；多个连续事件只热一次。
            if WARMING
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(800));
                    platform::warm_up_capture();
                    WARMING.store(false, Ordering::SeqCst);
                });
            }
            if event == SystemEvent::DisplayChanged {
                let ui = app.clone();
                let _ = app.run_on_main_thread(move || capture::overlay::prewarm(&ui));
            }
        }
        SystemEvent::ThemeChanged => {
            let ui = app.clone();
            let _ = app.run_on_main_thread(move || wm::refresh_visuals(&ui));
        }
    }
}

fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    let label = window.label();
    if let WindowEvent::CloseRequested { api, .. } = event {
        match label {
            wm::MAIN => {
                let app = window.app_handle();
                if state(app).settings.read().general.close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    quit(app);
                }
            }
            // 识字窗口、AI 窗口复用，关闭 = 隐藏（AI 窗口里的对话还留着）
            ocr::WINDOW | ai::WINDOW => {
                api.prevent_close();
                let _ = window.hide();
            }
            editor::WINDOW => {
                api.prevent_close();
                editor::close(window.app_handle());
            }
            _ => {}
        }
    }
    if let WindowEvent::Destroyed = event {
        if label.starts_with(pin::LABEL_PREFIX) {
            pin::close(window.app_handle(), label);
        }
    }
}

pub(crate) fn quit(app: &AppHandle) {
    shutdown(app);
    app.exit(0);
}

fn shutdown(app: &AppHandle) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(st) = app.try_state::<AppState>() {
        // 退出时必须杀掉识字子进程，防止残留
        st.ocr.shutdown();
    }
    tracing::info!("DATO OCR 退出");
}
