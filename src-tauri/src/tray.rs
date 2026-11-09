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
    // 菜单上的字跟着界面语言（改语言后会重建菜单）
    let tr = crate::i18n::localize;
    let capture = MenuItem::with_id(
        app,
        "capture",
        label(&tr("截图"), &hk.capture),
        true,
        None::<&str>,
    )?;
    let instant = MenuItem::with_id(
        app,
        "instant",
        label(&tr("瞬间截屏"), &hk.instant),
        true,
        None::<&str>,
    )?;
    let longshot = MenuItem::with_id(
        app,
        "longshot",
        label(&tr("长截图"), &hk.longshot),
        true,
        None::<&str>,
    )?;
    let delayed = MenuItem::with_id(app, "capture-delayed", tr("延时 3 秒截图"), true, None::<&str>)?;
    let ocr = MenuItem::with_id(app, "ocr", label(&tr("识字"), &hk.ocr), true, None::<&str>)?;
    let clip = MenuItem::with_id(
        app,
        "clipboard",
        label(&tr("剪贴板"), &hk.clipboard),
        true,
        None::<&str>,
    )?;
    let ai = MenuItem::with_id(app, "ai", label(&tr("AI 对话"), &hk.ai), true, None::<&str>)?;
    let input_tr = MenuItem::with_id(
        app,
        "input-translate",
        label(&tr("输入翻译"), &hk.input_translate),
        true,
        None::<&str>,
    )?;
    let pin_clip = MenuItem::with_id(
        app,
        "pin-clipboard",
        label(&tr("贴出剪贴板图片"), &hk.pin_clipboard),
        true,
        None::<&str>,
    )?;
    let restore_pin = MenuItem::with_id(app, "restore-pin", tr("恢复刚关闭的贴图"), true, None::<&str>)?;
    let release_pins = MenuItem::with_id(
        app,
        "release-pins",
        tr("取消贴图的鼠标穿透"),
        true,
        None::<&str>,
    )?;
    let sep_pins = PredefinedMenuItem::separator(app)?;
    let show = MenuItem::with_id(app, "show", tr("打开主窗口"), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", tr("设置…"), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", tr("退出 DATO OCR"), true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &capture,
            &delayed,
            &instant,
            &longshot,
            &ocr,
            &clip,
            &input_tr,
            &ai,
            &sep_pins,
            &pin_clip,
            &restore_pin,
            &release_pins,
            &sep1,
            &show,
            &settings,
            &sep2,
            &quit,
        ],
    )?;

    let builder = TrayIconBuilder::with_id("main-tray").tooltip("DATO OCR");
    // Windows 右键弹的是自己画的菜单（open_menu），不挂系统菜单
    #[cfg(not(windows))]
    let builder = builder.menu(&menu);
    #[cfg(windows)]
    let _ = &menu;
    let mut builder = builder
        .show_menu_on_left_click(platform::tray_menu_on_left_click())
        .on_menu_event(|app, event| run(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            if platform::tray_menu_on_left_click() {
                return;
            }
            // Windows：右键弹自己画的菜单（系统菜单是老式样子，图标、圆角、深浅色都跟程序别处不一样）
            #[cfg(windows)]
            if let TrayIconEvent::Click {
                button: MouseButton::Right,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_menu(tray.app_handle());
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

/// 菜单项选了哪个（系统菜单、自己画的菜单都走这里）
pub fn run(app: &AppHandle, id: &str) {
    match id {
        "capture" => hotkeys::dispatch(app, HotkeyAction::Capture),
        // 延时截图：先去把菜单、悬停提示之类点出来，3 秒后再冻结画面
        "capture-delayed" => {
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                hotkeys::dispatch(&app, HotkeyAction::Capture);
            });
        }
        "instant" => hotkeys::dispatch(app, HotkeyAction::Instant),
        "longshot" => hotkeys::dispatch(app, HotkeyAction::Longshot),
        "ocr" => hotkeys::dispatch(app, HotkeyAction::Ocr),
        "clipboard" => hotkeys::dispatch(app, HotkeyAction::Clipboard),
        "ai" => hotkeys::dispatch(app, HotkeyAction::Ai),
        "pin-clipboard" => hotkeys::pin_clipboard(app),
        "input-translate" => hotkeys::dispatch(app, HotkeyAction::InputTranslate),
        "restore-pin" => {
            if !matches!(crate::pin::restore_last(app), Ok(true)) {
                wm::toast(app, "info", "没有刚关闭的贴图");
            }
        }
        "release-pins" => crate::pin::release_click_through(app),
        "show" => wm::show_main(app, None),
        "settings" => wm::show_main(app, Some("settings")),
        "quit" => crate::quit(app),
        _ => {}
    }
}

/// Windows 托盘右键：在鼠标那里弹自己画的菜单（popmenu 窗口，和贴图的右键菜单同一个），选了哪项回到 `run`
#[cfg(windows)]
fn open_menu(app: &AppHandle) {
    use serde_json::json;
    let hk = state(app).settings.read().hotkeys.clone();
    let tr = crate::i18n::localize;
    let item = |id: &str, icon: &str, text: &str, key: &str| {
        json!({ "kind": "item", "id": id, "icon": icon, "label": tr(text), "shortcut": if key.is_empty() { None } else { Some(key) } })
    };
    let sep = || json!({ "kind": "separator" });
    let items = json!([
        item("capture", "capture", "截图", &hk.capture),
        item("capture-delayed", "timer", "延时 3 秒截图", ""),
        item("instant", "instant", "瞬间截屏", &hk.instant),
        item("longshot", "longshot", "长截图", &hk.longshot),
        item("ocr", "ocr", "识字", &hk.ocr),
        item("clipboard", "clipboard", "剪贴板", &hk.clipboard),
        item("input-translate", "translate", "输入翻译", &hk.input_translate),
        item("ai", "ai", "AI 对话", &hk.ai),
        sep(),
        item("pin-clipboard", "pinImage", "贴出剪贴板图片", &hk.pin_clipboard),
        item("restore-pin", "restore", "恢复刚关闭的贴图", ""),
        item("release-pins", "clickThrough", "取消贴图的鼠标穿透", ""),
        sep(),
        item("show", "window", "打开主窗口", ""),
        item("settings", "settings", "设置…", ""),
        sep(),
        json!({ "kind": "item", "id": "quit", "icon": "quit", "label": tr("退出 DATO OCR"), "danger": true }),
    ]);
    if let Err(err) = crate::popmenu::open(app, crate::popmenu::TRAY_OWNER.into(), items) {
        tracing::warn!("托盘菜单弹不出来：{err}");
    }
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
