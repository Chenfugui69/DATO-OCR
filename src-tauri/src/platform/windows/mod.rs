//! Windows 平台实现。
//!
//! 这是**唯一**允许 `use windows::...` 的目录（规格 07 §1）。

mod backdrop;
mod capture;
mod dpi;
mod self_capture;
mod system_events;
mod system_info;
mod window_effects;

use super::{BackdropLayer, ScreenCapture, SystemInfo, WindowEffects};

static SCREEN_CAPTURE: capture::WindowsScreenCapture = capture::WindowsScreenCapture;
static WINDOW_EFFECTS: window_effects::WindowsWindowEffects = window_effects::WindowsWindowEffects;
static SYSTEM_INFO: system_info::WindowsSystemInfo = system_info::WindowsSystemInfo;
static BACKDROP_LAYER: backdrop::WindowsBackdropLayer = backdrop::WindowsBackdropLayer;

pub fn init_process() {
    dpi::declare_per_monitor_v2();
}

pub fn screen_capture() -> &'static dyn ScreenCapture {
    &SCREEN_CAPTURE
}

pub fn window_effects() -> &'static dyn WindowEffects {
    &WINDOW_EFFECTS
}

pub fn system_info() -> &'static dyn SystemInfo {
    &SYSTEM_INFO
}

pub fn backdrop_layer() -> &'static dyn BackdropLayer {
    &BACKDROP_LAYER
}

pub fn on_capture_state_lost(handler: fn()) {
    system_events::on_capture_state_lost(handler);
}
