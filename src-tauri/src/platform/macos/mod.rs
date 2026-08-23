//! macOS 平台桩。
//!
//! 第一版**不实现** macOS，但签名必须完整，且每个 `unimplemented!` 都要写清楚
//! 移植时该用哪个 API（规格 07 §5）。这是留给未来的地图。
//!
//! 移植时的权限清单（这是真正的工作量，不是 API 本身）：
//! - 抓屏 → 需要「屏幕录制」权限
//! - 窗口子控件枚举 / 模拟按键 / 滚轮监听 → 需要「辅助功能」权限
//! 首次运行必须有引导流程，不能等到用户点了功能才失败。

use image::RgbaImage;

use crate::error::AppResult;
use crate::platform::{
    BackdropLayer, MonitorId, MonitorInfo, PhysicalRect, ScreenCapture, SystemInfo, WindowEffects,
};

pub struct MacosScreenCapture;
pub struct MacosWindowEffects;
pub struct MacosSystemInfo;
pub struct MacosBackdropLayer;

static SCREEN_CAPTURE: MacosScreenCapture = MacosScreenCapture;
static WINDOW_EFFECTS: MacosWindowEffects = MacosWindowEffects;
static SYSTEM_INFO: MacosSystemInfo = MacosSystemInfo;
static BACKDROP_LAYER: MacosBackdropLayer = MacosBackdropLayer;

/// macOS 没有 Windows 那种进程级 DPI 声明，AppKit 天生按 backing scale 工作。
pub fn init_process() {}

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

/// macOS 上对应的通知是 `NSApplicationDidChangeScreenParametersNotification`
/// （显示配置变化）和 `NSWorkspaceDidWakeNotification`（睡眠唤醒）。
/// 留空只是失去"重新热身"这个优化，不影响功能。
pub fn on_capture_state_lost(_handler: fn()) {}

impl ScreenCapture for MacosScreenCapture {
    fn list_monitors(&self) -> AppResult<Vec<MonitorInfo>> {
        unimplemented!(
            "macOS: 用 NSScreen.screens 拿 frame 与 backingScaleFactor；\
             visibleFrame 对应 work_area。注意 NSScreen 的原点在左下角，\
             需要翻转成左上角原点才能对上 PhysicalRect 的约定"
        )
    }

    fn capture_all(&self) -> AppResult<Vec<(MonitorInfo, RgbaImage)>> {
        unimplemented!(
            "macOS: 优先 ScreenCaptureKit（12.3+，SCScreenshotManager）；\
             回退用 CGDisplayCreateImage。两者都需要「屏幕录制」权限。\
             对 list_monitors 的结果逐块抓即可"
        )
    }

    fn warm_up(&self) {
        // macOS 上等价的开销是 SCStream / SCShareableContent 的首次建立。
        // 留空不影响正确性，只是首次截图会慢一些。
    }
}

impl WindowEffects for MacosWindowEffects {
    fn native_handle(&self, _window: &tauri::WebviewWindow) -> AppResult<u64> {
        unimplemented!(
            "macOS: window.ns_window() 拿到的是 NSWindow 指针。注意 BackdropLayer \
             那边要的是 windowNumber 而不是指针，转换要在 platform 内部做完"
        )
    }

    fn exclude_from_capture(
        &self,
        _window: &tauri::WebviewWindow,
        _enabled: bool,
    ) -> AppResult<()> {
        unimplemented!(
            "macOS: 设 NSWindow.sharingType = .none。注意它只影响屏幕共享/录制，\
             ScreenCaptureKit 侧还需要用 SCContentFilter 的 excludingWindows 排除"
        )
    }
}

impl BackdropLayer for MacosBackdropLayer {
    fn ensure(&self, _monitor: MonitorId) -> AppResult<()> {
        unimplemented!(
            "macOS: 每块屏一个 NSWindow，level = .screenSaver - 1（要在遮罩窗口之下），\
             ignoresMouseEvents = true，isOpaque = true，collectionBehavior 加 \
             .canJoinAllSpaces | .stationary。sharingType = .none 对应 \
             WDA_EXCLUDEFROMCAPTURE"
        )
    }

    fn load(&self, _monitor: MonitorId, _image: &RgbaImage, _at: PhysicalRect) -> AppResult<()> {
        unimplemented!(
            "macOS: 把 RgbaImage 包成 CGImage（CGDataProvider + CGImageCreate），\
             挂到 CALayer.contents 上。别走 NSImageView —— 它会做插值。\
             图层要设 contentsScale = backingScaleFactor 并关掉 magnificationFilter，\
             否则 Retina 上底图会被重采样，破坏像素级一致"
        )
    }

    fn show_above(&self, _monitor: MonitorId, _overlay: u64) -> AppResult<()> {
        unimplemented!(
            "macOS: orderWindow(.below, relativeTo: overlay.windowNumber)。\
             AppKit 没有 DeferWindowPos 那种批次，但两个窗口的 order 变更在同一次 \
             CATransaction 内提交即可保证同帧；必要时用 NSDisableScreenUpdates 包一层"
        )
    }

    fn hide_all(&self) {
        unimplemented!("macOS: 逐个 orderOut(nil)")
    }

    fn retain(&self, _keep: &[MonitorId]) {
        unimplemented!(
            "macOS: 监听 NSApplication.didChangeScreenParametersNotification，\
             把不在列表里的窗口 close()"
        )
    }
}

impl SystemInfo for MacosSystemInfo {
    fn is_dark_mode(&self) -> bool {
        unimplemented!(
            "macOS: NSApp.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua"
        )
    }

    fn is_transparency_enabled(&self) -> bool {
        unimplemented!(
            "macOS: NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency 取反"
        )
    }
}
