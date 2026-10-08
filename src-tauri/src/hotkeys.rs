//! 全局热键：F1 截图 / F2 长截图 / F3 识字 / Alt+V 剪贴板 / Ctrl+Alt+T 划词翻译 / Shift+F1 瞬间截屏。
//!
//! 加速键字符串用 global-hotkey 的语法（`Ctrl+Alt+T`、`Super+Shift+S`、`F1`）。
//! 被别的程序占用时注册会失败 —— 不让应用起不来，状态报给设置页显示冲突。

use std::collections::HashMap;
use std::str::FromStr;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::capture::{self, CaptureIntent};
use crate::state::state;
use crate::{clipboard, translate};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HotkeyAction {
    Capture,
    Longshot,
    Ocr,
    Clipboard,
    Translate,
    Instant,
    Ai,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyStatus {
    pub action: HotkeyAction,
    pub accelerator: String,
    pub ok: bool,
    pub error: Option<String>,
}

impl HotkeyAction {
    /// 命令行里的名字（`--action=capture`）。
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "capture" => Self::Capture,
            "longshot" => Self::Longshot,
            "ocr" => Self::Ocr,
            "clipboard" => Self::Clipboard,
            "translate" => Self::Translate,
            "instant" => Self::Instant,
            "ai" => Self::Ai,
            _ => return None,
        })
    }
}

#[derive(Default)]
pub struct HotkeyRegistry {
    map: Mutex<HashMap<u32, HotkeyAction>>,
    status: Mutex<Vec<HotkeyStatus>>,
}

impl HotkeyRegistry {
    pub fn status(&self) -> Vec<HotkeyStatus> {
        self.status.lock().clone()
    }
}

pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    // 只认按下，否则一次按键触发两遍
    if event.state() != ShortcutState::Pressed {
        return;
    }
    let action = state(app).hotkeys.map.lock().get(&shortcut.id()).copied();
    if action.is_none() {
        tracing::debug!(id = shortcut.id(), "收到未登记的热键");
    }
    if let Some(action) = action {
        tracing::debug!(?action, "热键触发");
        dispatch(app, action);
    }
}

pub fn dispatch(app: &AppHandle, action: HotkeyAction) {
    match action {
        HotkeyAction::Capture => capture::trigger(app, CaptureIntent::Normal),
        HotkeyAction::Longshot => capture::trigger(app, CaptureIntent::Longshot),
        HotkeyAction::Ocr => capture::trigger(app, CaptureIntent::Ocr),
        HotkeyAction::Clipboard => clipboard::panel::toggle(app),
        HotkeyAction::Translate => translate::selection::trigger(app),
        HotkeyAction::Instant => capture::trigger_instant(app),
        HotkeyAction::Ai => crate::ai::open_window(
            app,
            crate::ai::AiContext {
                source: "free".into(),
                ..Default::default()
            },
        ),
    }
}

/// 按当前设置重新注册全部热键。
pub fn register_all(app: &AppHandle) -> Vec<HotkeyStatus> {
    let st = state(app);
    let hk = st.settings.read().hotkeys.clone();
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let mut map = HashMap::new();
    let mut statuses = Vec::new();
    let wanted = [
        (HotkeyAction::Capture, hk.capture),
        (HotkeyAction::Longshot, hk.longshot),
        (HotkeyAction::Ocr, hk.ocr),
        (HotkeyAction::Clipboard, hk.clipboard),
        (HotkeyAction::Translate, hk.translate),
        (HotkeyAction::Instant, hk.instant),
        (HotkeyAction::Ai, hk.ai),
    ];
    for (action, accel) in wanted {
        let accel = accel.trim().to_string();
        if accel.is_empty() {
            continue;
        }
        let result = Shortcut::from_str(&accel)
            .map_err(|e| format!("无法识别的快捷键：{e}"))
            .and_then(|sc| {
                if map.contains_key(&sc.id()) {
                    return Err("与其他功能的快捷键重复".to_string());
                }
                gs.register(sc)
                    .map(|_| sc)
                    .map_err(|_| "已被其他程序占用".to_string())
            });
        match result {
            Ok(sc) => {
                map.insert(sc.id(), action);
                statuses.push(HotkeyStatus {
                    action,
                    accelerator: accel,
                    ok: true,
                    error: None,
                });
            }
            Err(err) => {
                tracing::warn!(?action, %accel, "注册热键失败：{err}");
                statuses.push(HotkeyStatus {
                    action,
                    accelerator: accel,
                    ok: false,
                    error: Some(crate::i18n::text(err)),
                });
            }
        }
    }
    tracing::info!(registered = map.len(), "全局热键已注册");
    *st.hotkeys.map.lock() = map;
    *st.hotkeys.status.lock() = statuses.clone();
    statuses
}

/// 校验一个加速键字符串能否被解析（设置页录入时用）。
pub fn validate(accel: &str) -> Result<(), String> {
    Shortcut::from_str(accel)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 暂停全部热键（录入新快捷键时，避免按下的组合直接触发功能）。
pub fn suspend(app: &AppHandle) {
    let _ = app.global_shortcut().unregister_all();
}
