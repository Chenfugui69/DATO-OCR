// 总纲 §6.2 禁止 unwrap/expect。靠人工 review 守不住，交给 clippy 拦。
//
// 测试代码整体放行：断言失败就该 panic，那正是测试要的行为，改成 `?` 只会让失败
// 信息变差。注意 `cfg_attr(test, allow(..))` 是**整个 crate** 放行，所以生产代码的
// 拦截实际发生在非 test 那次编译上 —— `cargo clippy --all-targets` 两次都编，
// 覆盖是完整的。
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod capture;
mod commands;
mod error;
mod logging;
mod paths;
mod platform;
mod state;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconEvent;
use tauri::{AppHandle, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

use crate::capture::CaptureMode;
use crate::error::AppResult;
use crate::paths::AppPaths;
use crate::state::AppState;

/// M0 只挂截图热键。完整的热键表（F2 长截图 / F3 识字 / Alt+V 剪贴板 …）
/// 见规格 07 §8，等设置模块（M6）能读写配置之后再接。
fn capture_hotkey() -> Shortcut {
    Shortcut::new(None, Code::F1)
}

pub fn run() {
    // DPI 必须在任何窗口、任何抓屏 API 之前声明（规格 07 §4.1）。
    platform::init_process();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(on_shortcut)
                .build(),
        )
        // 底图走自定义协议而不是 IPC。原因见 capture::protocol 的文件头
        // —— 33MB 过 IPC 要 900ms，直接把整个延迟预算吃光。
        .register_asynchronous_uri_scheme_protocol(
            capture::protocol::SCHEME,
            capture::protocol::handle,
        )
        .setup(setup)
        .on_window_event(on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::capture::capture_prepare,
            commands::capture::capture_overlay_boot,
            commands::capture::capture_overlay_ready,
            commands::capture::capture_overlay_pixels_ready,
            commands::capture::capture_finish,
            commands::capture::capture_cancel,
            commands::capture::capture_trigger,
            commands::system::get_system_diagnostics,
            commands::system::open_data_folder,
            commands::system::report_error,
        ])
        .build(tauri::generate_context!());

    // 到这一步失败说明应用根本起不来，没有可恢复路径。但也不能 expect 掉：
    // 这是 `windows_subsystem = "windows"` 的 GUI 进程，没有控制台，panic 信息
    // 直接进虚空 —— 用户看到的是"双击了没反应"，我们手里一条线索都没有。
    // 所以先尽量往日志里写一笔（`setup` 已经跑过的话 tracing 就是活的），
    // 再以非零码退出。
    let app = match app {
        Ok(app) => app,
        Err(err) => {
            tracing::error!("Tauri 应用初始化失败: {err}");
            eprintln!("CHENOCR 初始化失败: {err}");
            std::process::exit(1);
        }
    };

    app.run(|app, event| {
            // 遮罩窗口必须等到 `Ready` 之后再建。在 `setup()` 里建出来的窗口，
            // WebView 拿不到 Tauri 注入的 IPC 初始化脚本，页面里的 JS 根本不会跑
            // —— 表现是按 F1 之后遮罩永远不出现，且没有任何报错。
            if matches!(event, tauri::RunEvent::Ready) {
                capture::overlay::prewarm(app);

                // 启动热一次，并订阅"热身状态失效"的时机（插拔屏、睡眠唤醒等）。
                // 热身本身在后台线程跑，不占 UI 线程。策略见 capture::warmup。
                capture::warmup::install();
            }
        });
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();

    let paths = AppPaths::resolve(&handle)?;
    let log_guard = logging::init(&paths.logs());

    tracing::info!(
        version = %app.package_info().version,
        data_dir = %paths.root().display(),
        "CHENOCR 启动"
    );

    if let Err(err) = paths.reset_temp() {
        tracing::warn!("重置 temp 目录失败: {err}");
    }

    app.manage(AppState::new(paths, log_guard));

    register_hotkeys(&handle);
    build_tray(&handle)?;

    // 主窗口按规格 01 §2 建好但隐藏，由托盘唤起。开发时直接显示出来，
    // 否则每次调试都得先去点托盘。
    if cfg!(debug_assertions) {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }

    Ok(())
}

fn register_hotkeys(app: &AppHandle) {
    match app.global_shortcut().register(capture_hotkey()) {
        Ok(()) => tracing::info!("已注册截图热键 F1"),
        // 热键被别的程序占了不该让应用起不来，托盘菜单里仍然能触发截图。
        Err(err) => tracing::error!("注册截图热键 F1 失败（可能被其他程序占用）: {err}"),
    }
}

fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, event: tauri_plugin_global_shortcut::ShortcutEvent) {
    // 只认按下，不认松开，否则一次按键会触发两遍。
    if event.state() != ShortcutState::Pressed || *shortcut != capture_hotkey() {
        return;
    }

    trigger_capture(app.clone(), CaptureMode::Normal);
}

/// 抓屏是 CPU/GDI 密集的活，放到工作线程上做，别卡住事件循环。
///
/// 遮罩窗口的创建必须回到主线程 —— 这一点由 `capture::start` 内部通过
/// Tauri 的 API 保证调用位置，这里只负责别在主线程上做像素搬运。
fn trigger_capture(app: AppHandle, mode: CaptureMode) {
    std::thread::spawn(move || {
        if let Err(err) = capture::start(&app, mode) {
            tracing::error!("启动截图失败: {err}");
        }
    });
}

fn build_tray(app: &AppHandle) -> AppResult<()> {
    let capture_item = MenuItem::with_id(app, "capture", "截图  F1", true, None::<&str>)?;
    let show_item = MenuItem::with_id(app, "show", "打开主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出 CHENOCR", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&capture_item, &show_item, &quit_item])?;

    let tray = app
        .tray_by_id("main-tray")
        .ok_or_else(|| error::AppError::Internal("找不到 tauri.conf.json 里声明的托盘图标".into()))?;

    tray.set_menu(Some(menu))?;
    tray.on_menu_event(|app, event| match event.id.as_ref() {
        "capture" => trigger_capture(app.clone(), CaptureMode::Normal),
        "show" => show_main_window(app),
        "quit" => app.exit(0),
        other => tracing::debug!(id = other, "未处理的托盘菜单项"),
    });

    tray.on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::DoubleClick { .. } = event {
            show_main_window(tray.app_handle());
        }
    });

    Ok(())
}

fn show_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        tracing::error!("主窗口不存在");
        return;
    };

    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// 遮罩窗口这里刻意什么都不做 —— 尤其是不响应失焦：多屏时用户在屏幕之间移动鼠标
/// 会不断切换焦点，靠失焦来退出会导致截图刚开始就被自己关掉。
fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    // 主窗口的关闭按钮只是收进托盘，不退进程（规格 07 §8 的 closeToTray）。
    if window.label() == "main" {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window.hide();
        }
    }
}
