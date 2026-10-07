//! 托盘图标与菜单。

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

use crate::error::AppResult;
use crate::hotkeys::{self, HotkeyAction};
use crate::platform;
use crate::state::state;
use crate::wm;

pub fn build(app: &AppHandle) -> AppResult<()> {
    let hk = state(app).settings.read().hotkeys.clone();
    let label = platform::menu_label;
    let capture = MenuItem::with_id(
        app,
        "capture",
        label("截图", &hk.capture),
        true,
        None::<&str>,
    )?;
    let instant = MenuItem::with_id(
        app,
        "instant",
        label("瞬间截屏", &hk.instant),
        true,
        None::<&str>,
    )?;
    let longshot = MenuItem::with_id(
        app,
        "longshot",
        label("长截图", &hk.longshot),
        true,
        None::<&str>,
    )?;
    let ocr = MenuItem::with_id(app, "ocr", label("识字", &hk.ocr), true, None::<&str>)?;
    let clip = MenuItem::with_id(
        app,
        "clipboard",
        label("剪贴板", &hk.clipboard),
        true,
        None::<&str>,
    )?;
    let ai = MenuItem::with_id(app, "ai", label("AI 对话", &hk.ai), true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "打开主窗口", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 DATO OCR", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &capture, &instant, &longshot, &ocr, &clip, &ai, &sep1, &show, &settings, &sep2, &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("DATO OCR")
        .menu(&menu)
        .show_menu_on_left_click(platform::tray_menu_on_left_click())
        .on_menu_event(|app, event| match event.id.as_ref() {
            "capture" => hotkeys::dispatch(app, HotkeyAction::Capture),
            "instant" => hotkeys::dispatch(app, HotkeyAction::Instant),
            "longshot" => hotkeys::dispatch(app, HotkeyAction::Longshot),
            "ocr" => hotkeys::dispatch(app, HotkeyAction::Ocr),
            "clipboard" => hotkeys::dispatch(app, HotkeyAction::Clipboard),
            "ai" => hotkeys::dispatch(app, HotkeyAction::Ai),
            "show" => wm::show_main(app, None),
            "settings" => wm::show_main(app, Some("settings")),
            "quit" => crate::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if platform::tray_menu_on_left_click() {
                return;
            }
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                wm::show_main(tray.app_handle(), None);
            }
        });
    if platform::tray_icon_is_template() {
        // 菜单栏里用单色图标（只看透明度），系统按菜单栏的深浅自动上色
        builder = builder
            .icon(tauri::include_image!("icons/tray.png"))
            .icon_as_template(true);
    } else if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// 热键改了之后重建菜单，让菜单上显示的快捷键跟着变。
pub fn rebuild(app: &AppHandle) {
    if let Some(tray) = app.remove_tray_by_id("main-tray") {
        drop(tray);
    }
    if let Err(err) = build(app) {
        tracing::warn!("重建托盘失败：{err}");
    }
}
