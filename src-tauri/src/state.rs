//! 全局状态。用 parking_lot 的锁，**禁止持锁 await**。

use parking_lot::RwLock;
use tauri::{AppHandle, Emitter, Manager};
use tracing_appender::non_blocking::WorkerGuard;

use crate::capture::CaptureState;
use crate::clipboard::ClipboardService;
use crate::error::AppResult;
use crate::hotkeys::HotkeyRegistry;
use crate::image_store::ImageStore;
use crate::longshot::LongshotState;
use crate::ocr::OcrService;
use crate::paths::AppPaths;
use crate::pin::PinRegistry;
use crate::platform::SystemVisuals;
use crate::settings::Settings;
use crate::storage::Db;
use crate::translate::TranslateService;
use crate::{events, platform};

pub struct AppState {
    pub paths: AppPaths,
    pub settings: RwLock<Settings>,
    pub db: Db,
    pub images: ImageStore,
    pub capture: CaptureState,
    pub longshot: LongshotState,
    pub clipboard: ClipboardService,
    pub ocr: OcrService,
    pub translate: TranslateService,
    pub pins: PinRegistry,
    pub hotkeys: HotkeyRegistry,
    pub visuals: RwLock<SystemVisuals>,
    _log_guard: Option<WorkerGuard>,
}

impl AppState {
    pub fn new(
        paths: AppPaths,
        settings: Settings,
        db: Db,
        log_guard: Option<WorkerGuard>,
    ) -> Self {
        Self {
            paths,
            settings: RwLock::new(settings),
            db,
            images: ImageStore::default(),
            capture: CaptureState::default(),
            longshot: LongshotState::default(),
            clipboard: ClipboardService::default(),
            ocr: OcrService::default(),
            translate: TranslateService::default(),
            pins: PinRegistry::default(),
            hotkeys: HotkeyRegistry::default(),
            visuals: RwLock::new(platform::system_visuals()),
            _log_guard: log_guard,
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    /// 保存设置并广播给所有窗口。
    pub fn update_settings(&self, app: &AppHandle, next: Settings) -> AppResult<Settings> {
        let next = next.sanitized();
        next.save(&self.paths.settings_file())?;
        *self.settings.write() = next.clone();
        let _ = app.emit(events::SETTINGS_CHANGED, &next);
        Ok(next)
    }
}

pub fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}
