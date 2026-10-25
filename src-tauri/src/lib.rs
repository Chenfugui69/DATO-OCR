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
mod apng;
mod capture;
mod clipboard;
mod commands;
mod editor;
mod error;
mod events;
mod gif_record;
mod hotkeys;
mod i18n;
mod i18n_en;
mod i18n_tw;
mod image_store;
mod imaging;
mod library;
mod lifecycle;
mod logging;
mod longshot;
mod maintenance;
mod mcp;
mod net;
mod ocr;
mod otp;
mod paths;
mod pin;
mod popmenu;
mod record_edit;
mod record_fx;
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
    // `--mcp`：给 AI 工具用的命令行桥接，不启动界面（见 mcp::bridge）
    if std::env::args().any(|a| a == "--mcp") {
        mcp::bridge::run();
        return;
    }
    platform::init_process();

    // 测试实例（`CHENOCR_TEST_DATA_DIR`，数据放在别处）不参与单实例：可以和用户正在用的那个同时开着，
    // 拍说明用的截图、做自动化测试都不用先让用户退出。它也就收不到 `--action=` 这类转发。
    // 调试版再加 `CHENOCR_TEST_SINGLE=1` 就照样参与（用户的实例已经退出时）：用空数据跑自动化，
    // `--eval=` / `--action=` 也收得到，不碰用户的设置和钥匙串
    let single = std::env::var_os("CHENOCR_TEST_DATA_DIR").is_none()
        || (cfg!(debug_assertions) && std::env::var_os("CHENOCR_TEST_SINGLE").is_some());
    let builder = tauri::Builder::default();
    let builder = if single {
        builder
            // 单实例必须第一个注册：第二次启动只是把已有实例的主窗口叫出来。
            // `--page=settings:translate` 这样的参数可以直接跳到某一页（快捷方式、测试用）；
            // `--translate=文字` 在鼠标旁边弹出划词面板翻译这段文字（脚本、快捷指令、测试用）；
            // `--action=capture` 直接触发一个功能、`--action=cancel` 取消正在进行的截图，
            // 都不叫主窗口（快捷方式、脚本、自动化测试用）
            .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
                // 开机自启 / 静默启动撞上已经在跑的实例：什么都不做（不能把主窗口弹出来）
                if args.iter().any(|a| a == "--autostart" || a == "--hidden") {
                    return;
                }
                if cfg!(debug_assertions) {
                    if let Some(spec) = args.iter().find_map(|a| a.strip_prefix("--eval=")) {
                        debug_eval(app, spec);
                        return;
                    }
                    // `--demo=ocr:图片路径`：拿一张图片文件开一个识字任务（表格 / 公式识别的自动化测试用）
                    #[cfg(debug_assertions)]
                    if let Some(path) = args.iter().find_map(|a| a.strip_prefix("--demo=ocr:")) {
                        match imaging::load(std::path::Path::new(path)) {
                            Ok(image) => {
                                if let Err(err) = ocr::open_job(app, image, None, false) {
                                    tracing::warn!("--demo=ocr 打开失败：{err}");
                                }
                            }
                            Err(err) => tracing::warn!("--demo=ocr 读图失败：{err}"),
                        }
                        return;
                    }
                    // `--demo=popup:x,y`（屏幕物理像素）：在指定位置弹出划词面板
                    #[cfg(debug_assertions)]
                    if let Some(p) = args.iter().find_map(|a| a.strip_prefix("--demo=popup:")) {
                        if let Some((x, y)) = p.split_once(',') {
                            if let (Ok(x), Ok(y)) = (x.parse(), y.parse()) {
                                translate::selection::show_popup(
                                    app,
                                    "Apple chip".into(),
                                    Some((x, y)),
                                );
                            }
                        }
                        return;
                    }
                    // `--demo=selection`：一秒半后问前台程序选中了什么，写进日志（调划词判断用）
                    #[cfg(debug_assertions)]
                    if args.iter().any(|a| a == "--demo=selection") {
                        std::thread::spawn(|| {
                            std::thread::sleep(std::time::Duration::from_millis(1500));
                            tracing::info!(result = %platform::probe_selection(), "前台程序的选中文字");
                        });
                        return;
                    }
                    // `--demo=selbtn` 或 `--demo=selbtn:x,y`（屏幕物理像素）：不用真的选字，把划词按钮亮出来看样子
                    #[cfg(debug_assertions)]
                    if let Some(spec) = args.iter().find_map(|a| a.strip_prefix("--demo=selbtn")) {
                        let at = spec.strip_prefix(':').and_then(|p| {
                            let (x, y) = p.split_once(',')?;
                            Some((x.parse().ok()?, y.parse().ok()?))
                        });
                        translate::selection::demo_button(app, at);
                        return;
                    }
                }
                if let Some(action) = args.iter().find_map(|a| a.strip_prefix("--action=")) {
                    match hotkeys::HotkeyAction::parse(action) {
                        Some(action) => hotkeys::dispatch(app, action),
                        None if action == "cancel" => capture::cancel_current(app),
                        None => tracing::warn!(action, "不认识的 --action"),
                    }
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
    } else {
        builder
    };
    let builder = builder
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
        // 每个窗口的页面都按屏幕刷新率刷新（截图库滚动、拖贴图、划词面板，不止截图遮罩）。
        // 放在这里统一做，新加的窗口也不会漏
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Started {
                if let Some(window) = webview.get_webview_window(webview.label()) {
                    platform::unlock_frame_rate(&window);
                    // 截图遮罩藏着时也要先画好下一次截图的画面，不能停；它藏起来时缩成 1×1 省内存
                    if !window.label().starts_with(capture::overlay::LABEL_PREFIX) {
                        platform::track_webview_idle(&window);
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::capture::capture_start,
            commands::capture::capture_session_info,
            commands::capture::capture_overlay_ready,
            commands::capture::capture_window_children,
            commands::capture::capture_finish,
            commands::capture::capture_cancel,
            commands::capture::capture_translate,
            commands::capture::clipboard_write_text,
            commands::capture::clipboard_read_text,
            commands::capture::longshot_set_regions,
            commands::capture::longshot_finish,
            commands::capture::longshot_abort,
            commands::capture::gif_set_regions,
            commands::capture::gif_set_capturable,
            commands::capture::capture_record_prep,
            commands::capture::gif_finish,
            commands::capture::gif_move_to,
            commands::capture::record_glyphs,
            commands::capture::gif_cancel,
            commands::capture::record_edit_info,
            commands::capture::record_edit_save,
            commands::capture::record_edit_discard,
            commands::capture::media_access,
            commands::capture::media_request,
            commands::capture::longshot_undo,
            commands::capture::pin_info,
            commands::capture::pin_ready,
            commands::capture::pin_close,
            commands::capture::pin_close_all,
            commands::capture::pin_copy,
            commands::capture::pin_save,
            commands::capture::pin_ocr,
            commands::capture::pin_text,
            commands::capture::pin_scan_codes,
            commands::capture::pin_detect_structure,
            commands::capture::pin_table,
            commands::capture::pin_structure,
            commands::capture::pin_formula_regions,
            commands::capture::pin_region_style,
            commands::capture::popmenu_open,
            commands::capture::popmenu_items,
            commands::capture::popmenu_place,
            commands::capture::popmenu_pick,
            commands::capture::popmenu_close,
            commands::capture::pin_transform,
            commands::capture::pin_edit,
            commands::capture::pin_restore_last,
            commands::library::ocr_structure,
            commands::library::ocr_export_xlsx,
            commands::capture::clipboard_write_rich,
            commands::clipboard::clipboard_pin_image,
            commands::capture::capture_scan_codes,
            commands::capture::capture_structure,
            commands::capture::capture_elements_at,
            commands::capture::capture_elements_near,
            commands::capture::capture_detect_structure,
            commands::capture::capture_table,
            commands::capture::capture_formula_regions,
            commands::capture::capture_region_style,
            commands::capture::capture_overlay_stray,
            commands::capture::capture_pin_color,
            commands::capture::editor_current,
            commands::capture::editor_finish,
            commands::capture::editor_close,
            commands::capture::editor_open_shot,
            commands::clipboard::clipboard_query,
            commands::clipboard::clipboard_get,
            commands::clipboard::clipboard_stats,
            commands::clipboard::clipboard_gif_preview,
            commands::clipboard::clipboard_icon_colors,
            commands::clipboard::clipboard_file_icon,
            commands::clipboard::clipboard_smart_suggest,
            commands::clipboard::mail_codes_status,
            commands::clipboard::mail_codes_set_password,
            commands::clipboard::mail_codes_test,
            commands::clipboard::clipboard_preview_open,
            commands::clipboard::clipboard_preview_current,
            commands::clipboard::clipboard_preview_close,
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
            commands::clipboard::clipboard_set_group_color,
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
            commands::library::ocr_paddle_install,
            commands::library::ocr_paddle_remove,
            commands::library::ocr_latex_install,
            commands::library::ocr_latex_remove,
            commands::library::ocr_history,
            commands::library::ocr_open_record,
            commands::library::ocr_delete_record,
            commands::library::ocr_from_clip,
            commands::tools::translate_text,
            commands::tools::translate_providers,
            commands::tools::translate_detect,
            commands::tools::translate_popup_text,
            commands::tools::translate_popup_input,
            commands::tools::selection_button_pressed,
            commands::tools::system_language,
            commands::tools::translate_popup_reserve,
            commands::tools::translate_popup_preview,
            commands::tools::translate_set_field,
            commands::ai::ai_models,
            commands::ai::ai_chat,
            commands::ai::ai_cancel,
            commands::ai::ai_keys,
            commands::ai::ai_set_key,
            commands::ai::ai_open,
            commands::ai::ai_context,
            commands::ai::ai_attach_image,
            commands::ai::ai_release_images,
            commands::system::settings_get,
            commands::system::settings_set,
            commands::system::settings_reset,
            commands::system::visuals_get,
            commands::system::hotkeys_status,
            commands::system::hotkeys_suspend,
            commands::system::hotkeys_resume,
            commands::system::hotkeys_defaults,
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
            commands::system::toast_show,
            commands::system::speak,
            commands::system::speak_stop,
            commands::system::speak_status,
            commands::system::speak_voices,
            commands::system::mcp_status,
            commands::system::mcp_regenerate_token,
            commands::system::window_set_bounds,
            commands::system::window_backdrop,
            commands::system::window_backdrop_fade,
            commands::system::window_glass_snapshot_ready,
            commands::system::window_is_foreground,
            commands::system::window_set_region,
            commands::system::permissions_status,
            commands::system::ui_language,
            commands::system::window_ready,
            commands::system::window_placement,
            commands::system::window_take_focus,
            commands::system::window_start_resize,
            commands::system::permission_request,
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
        // macOS：主窗口关到菜单栏之后，点程序坞图标把它叫回来
        event if platform::is_reopen_event(&event) => wm::show_main(app, None),
        _ => {}
    });
}

/// 调试版专用：`--eval=窗口标签:脚本文件`，在指定窗口的页面里执行一段脚本。
///
/// 自动化测试用：macOS 上没有「辅助功能」权限就模拟不了鼠标键盘，只能在页面里合成指针事件；
/// 量拖动帧率也靠它。正式版里没有这个入口。
fn debug_eval(app: &AppHandle, spec: &str) {
    let Some((label, path)) = spec.split_once(':') else {
        tracing::warn!(spec, "--eval 的写法是 窗口标签:脚本文件");
        return;
    };
    let result = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|js| {
            app.get_webview_window(label)
                .ok_or_else(|| format!("没有窗口 {label}"))?
                .eval(js)
                .map_err(|e| e.to_string())
        });
    match result {
        Ok(()) => tracing::info!(label, path, "已执行调试脚本"),
        Err(err) => tracing::warn!(label, path, "执行调试脚本失败：{err}"),
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let _ = APP.set(handle.clone());

    let paths = AppPaths::resolve(&handle)?;
    // 先按系统语言定一次：第一次运行时设置的默认内容（AI 预设提示词）要按它生成
    i18n::set_language("system");
    let mut settings = Settings::load(&paths.settings_file());
    i18n::set_language(&settings.general.language);
    // 没改过的 AI 默认提示词跟着界面语言走（系统语言变了、或者上次是另一种语言）
    settings.ai.follow_language(i18n::english());
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
    // 预建按下快捷键 / 选中文字就要立刻出来的：截图遮罩、剪贴板面板（划词小圆点在
    // `reconfigure` 里按设置建）。别的窗口用到才建、闲置一阵就释放，见 lifecycle.rs
    capture::overlay::prewarm(app);
    clipboard::panel::prewarm(app);
    lifecycle::start(app);
    translate::selection::reconfigure(app);
    clipboard::start(app);
    sync::start(app);
    otp::start(app);
    let ui = app.clone();
    std::thread::spawn(move || mcp::reconfigure(&ui));
    update::start(app);
    platform::watch_system_events(on_system_event);
    std::thread::spawn(platform::warm_up_capture);
    maintenance::spawn(app);
    let ui = app.clone();
    std::thread::spawn(move || after_rename(&ui));

    platform::apply_theme(app, &state(app).settings.read().appearance.theme);
    // 开机自启时安静地待在托盘里（主窗口不建，用到才建）；手动启动时显示主窗口
    let silent = std::env::args().any(|a| a == "--autostart" || a == "--hidden");
    if silent {
        platform::after_silent_start();
    } else {
        wm::show_main(app, None);
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
            if event == SystemEvent::Resumed {
                // 睡了一觉，邮箱那条连接多半已经断了：关掉让它马上重连，别等到超时
                otp::mail::kick();
            }
            if event == SystemEvent::DisplayChanged {
                schedule_overlay_prewarm(app);
            }
        }
        SystemEvent::ThemeChanged => {
            let ui = app.clone();
            let _ = app.run_on_main_thread(move || wm::refresh_visuals(&ui));
        }
    }
}

/// 屏幕参数一变就重建遮罩太勤了：菜单栏 / 程序坞出现消失、切全屏空间时，系统一秒连发几十次。
/// 停下 400ms 后才重建一次，而且只在显示器的数量、位置、大小、缩放真的变了时才做。
fn schedule_overlay_prewarm(app: &AppHandle) {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static LAST: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);
    let id = SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        if SEQ.load(Ordering::SeqCst) != id {
            return;
        }
        let layout = platform::list_monitors()
            .map(|ms| {
                ms.iter()
                    .map(|m| format!("{}:{:?}:{}", m.id, m.bounds, m.scale_factor))
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .unwrap_or_default();
        {
            let mut last = LAST.lock();
            if last.as_deref() == Some(layout.as_str()) {
                return;
            }
            *last = Some(layout);
        }
        let ui = app.clone();
        let _ = app.run_on_main_thread(move || capture::overlay::prewarm(&ui));
    });
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
        lifecycle::forget(label);
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
        // 删掉 mcp.json：命令行桥接就知道 DATO OCR 没在运行，不会去连一个没人听的端口
        let _ = std::fs::remove_file(st.paths.mcp_file());
    }
    tracing::info!("DATO OCR 退出");
}
