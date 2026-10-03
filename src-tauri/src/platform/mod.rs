//! 跨平台抽象层（规格 07）。
//!
//! **铁律**：`platform/` 之外禁止出现 `#[cfg(windows)]` / `use windows::`，
//! 签名里禁止出现 HWND 之类的平台类型（一律包成 u64）。
//!
//! 规格草案里是 8 个 trait；这里落地成一组模块级函数，由编译期选择 `sys` 实现。
//! 效果相同 —— macOS 移植时只需在 `macos/` 里补齐同名函数，上层零改动 ——
//! 但省掉了 trait object 和一堆只有一个实现的样板。
//!
//! | 能力 | Windows | macOS（移植时） |
//! |---|---|---|
//! | 抓屏 | xcap + WGC | ScreenCaptureKit，需「屏幕录制」权限 |
//! | 窗口枚举 | EnumWindows + DWM 扩展边框 | CGWindowListCopyWindowInfo |
//! | 冻结底图层 | 分层窗口 + UpdateLayeredWindow | NSWindow + CALayer |
//! | 剪贴板 | AddClipboardFormatListener | NSPasteboard changeCount 轮询 |
//! | 输入模拟 | SendInput | CGEvent，需「辅助功能」权限 |
//! | 全局钩子 | WH_MOUSE_LL / WH_KEYBOARD_LL | CGEventTap |
//! | 密钥 | DPAPI | Keychain |
//! | 系统 OCR | Windows.Media.Ocr | Vision.framework |

pub mod types;

pub use types::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows as sys;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use self::macos as sys;

use std::path::Path;
use std::sync::mpsc::Sender;

use image::RgbaImage;
use tauri::WebviewWindow;

use crate::error::AppResult;

/// 进程启动最早期调用：声明 Per-Monitor V2 DPI 感知等。
pub fn init_process() {
    sys::init_process()
}

// ───────────────────────── 抓屏 ─────────────────────────

pub fn list_monitors() -> AppResult<Vec<MonitorInfo>> {
    sys::capture::list_monitors()
}

/// 抓取全部显示器。顺序：主屏在前，其余按 x、y 排。
pub fn capture_all() -> AppResult<Vec<(MonitorInfo, RgbaImage)>> {
    sys::capture::capture_all()
}

pub fn capture_monitor(id: MonitorId) -> AppResult<RgbaImage> {
    sys::capture::capture_monitor(id)
}

/// 预先支付首次抓屏的设备初始化开销（冷 137ms vs 热 49ms），结果丢弃。
pub fn warm_up_capture() {
    sys::capture::warm_up()
}

/// 显示器配置变化 / 睡眠唤醒 / 系统主题变化时回调（回调必须立刻返回）。
pub fn watch_system_events(handler: fn(SystemEvent)) {
    sys::system_events::install(handler)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemEvent {
    DisplayChanged,
    Resumed,
    ThemeChanged,
}

// ───────────────────────── 窗口 ─────────────────────────

/// 按 Z 序从上到下枚举可见顶层窗口（已过滤最小化、隐藏、工具窗、本进程窗口）。
pub fn enumerate_windows() -> AppResult<Vec<WindowInfo>> {
    sys::window_enum::enumerate_top_level()
}

/// 指定窗口内的可见子控件矩形（截图时的区域细分检测）。
pub fn enumerate_children(window: WindowHandle) -> AppResult<Vec<PhysicalRect>> {
    sys::window_enum::enumerate_children(window)
}

pub fn foreground_window() -> Option<WindowHandle> {
    sys::window_enum::foreground_window()
}

/// 这个窗口是不是 DATO COR 自己的。
pub fn is_own_window(window: WindowHandle) -> bool {
    sys::window_enum::is_own_window(window)
}

/// 某个屏幕点上实际可见的顶层窗口（跳过鼠标穿透的窗口）。
pub fn window_at(x: i32, y: i32) -> Option<WindowHandle> {
    sys::window_enum::window_at(x, y)
}

pub fn app_info_of(window: WindowHandle) -> Option<AppInfo> {
    sys::window_enum::app_info_of(window)
}

/// 把焦点还给指定窗口（绕过前台锁）。
pub fn focus_window(window: WindowHandle) -> AppResult<()> {
    sys::input::focus_window(window)
}

pub fn cursor_position() -> Option<(i32, i32)> {
    sys::input::cursor_position()
}

/// Tauri 窗口的原生句柄（不透明 u64）。
pub fn native_handle(window: &WebviewWindow) -> AppResult<u64> {
    sys::effects::native_handle(window)
}

/// 窗口是否从抓屏结果里排除（Win10 2004+）。
pub fn set_exclude_from_capture(window: &WebviewWindow, exclude: bool) {
    sys::effects::set_exclude_from_capture(window, exclude)
}

/// 诊断模式（`CHENOCR_ALLOW_SELF_CAPTURE=1`）下显示时放开抓屏、隐藏时恢复排除。
pub fn reveal_for_tests(window: &WebviewWindow, visible: bool) {
    sys::effects::reveal_for_tests(window, visible)
}

/// 窗口不抢焦点地置顶显示（toast 之类）。
/// 让窗口永远不被激活（点它、显示它都不抢焦点）。悬浮按钮这类"浮在别人上面"的窗口用。
pub fn set_no_activate(window: &WebviewWindow) {
    sys::effects::set_no_activate(window);
}

pub fn show_without_activate(window: &WebviewWindow) -> AppResult<()> {
    sys::effects::show_without_activate(window)
}

/// 把当前鼠标指针画进截图（`origin` = 图左上角的虚拟桌面物理坐标）。瞬间截屏用。
pub fn draw_cursor(image: &mut image::RgbaImage, origin: (i32, i32)) {
    sys::cursor::draw_cursor(image, origin);
}

/// 原生隐藏（配合 `show_without_activate` 用）。
pub fn hide_window(window: &WebviewWindow) {
    sys::effects::hide_window(window);
}

// ───────────────────────── 冻结底图层 ─────────────────────────
//
// 截图时冻结的屏幕画面不进 WebView：4K 传输解码要 170ms 且 Chromium 色彩管理会改
// 像素值。每块屏一个原生分层窗口，画面在隐藏状态就交给 DWM，和透明遮罩原子上屏。
// **以下函数只能在 UI 线程调用**（窗口归创建线程所有）。

pub mod backdrop {
    use super::*;

    pub fn ensure(monitor: MonitorId) -> AppResult<()> {
        sys::backdrop::ensure(monitor)
    }

    pub fn load(monitor: MonitorId, image: &RgbaImage, at: PhysicalRect) -> AppResult<()> {
        sys::backdrop::load(monitor, image, at)
    }

    /// 底图摆到遮罩正下方，两者同一批次显示，杜绝先亮一帧未压暗的画面。
    pub fn show_below(monitor: MonitorId, overlay: u64) -> AppResult<()> {
        sys::backdrop::show_below(monitor, overlay)
    }

    pub fn hide_all() {
        sys::backdrop::hide_all()
    }

    pub fn release_all() {
        sys::backdrop::release_all()
    }

    pub fn retain(keep: &[MonitorId]) {
        sys::backdrop::retain(keep)
    }
}

// ───────────────────────── 剪贴板 ─────────────────────────

/// 启动剪贴板监听线程，每次变化推送一份快照。
pub fn start_clipboard_listener(tx: Sender<ClipboardSnapshot>) -> AppResult<()> {
    sys::clipboard::start_listener(tx)
}

pub fn clipboard_write(payload: &ClipboardPayload) -> AppResult<()> {
    sys::clipboard::write(payload)
}

pub fn clipboard_read_text() -> Option<String> {
    sys::clipboard::read_text()
}

/// 整份备份当前剪贴板（划词翻译模拟复制前用，事后原样恢复）。
pub fn clipboard_backup() -> Option<ClipboardBackup> {
    sys::clipboard::backup().map(ClipboardBackup)
}

pub fn clipboard_restore(backup: ClipboardBackup) {
    sys::clipboard::restore(backup.0)
}

pub fn clipboard_sequence() -> u32 {
    sys::clipboard::sequence_number()
}

#[derive(Clone)]
pub struct ClipboardBackup(sys::clipboard::Backup);

// ───────────────────────── 输入模拟 / 钩子 ─────────────────────────

/// 先释放所有修饰键（用户可能还按着 Alt+V 的 Alt），再发 Ctrl+V。
pub fn send_paste() -> AppResult<()> {
    sys::input::send_paste()
}

pub fn send_copy() -> AppResult<()> {
    sys::input::send_copy()
}

/// 安装全局鼠标/键盘钩子（长截图用）。Enter/Esc/Backspace 会被吞掉，其余放行。
/// 返回的守卫 drop 时卸载钩子。
pub fn install_input_hook(tx: Sender<HookEvent>) -> AppResult<InputHookGuard> {
    sys::hook::install(tx).map(InputHookGuard)
}

pub struct InputHookGuard(#[allow(dead_code)] sys::hook::Guard);

/// 安装常驻的划词监听钩子（只看鼠标，只吞悬浮按钮上的点击）。守卫 drop 时卸载。
pub fn start_selection_watch(tx: Sender<SelectionEvent>) -> AppResult<SelectionWatchGuard> {
    sys::selection_hook::install(tx).map(SelectionWatchGuard)
}

pub struct SelectionWatchGuard(#[allow(dead_code)] sys::selection_hook::Guard);

/// 告诉划词钩子悬浮按钮的位置（屏幕物理坐标）；None = 按钮已隐藏。
pub fn set_selection_button_rect(rect: Option<PhysicalRect>) {
    sys::selection_hook::set_button_rect(rect);
}

// ───────────────────────── 密钥 ─────────────────────────

pub fn protect(plain: &[u8]) -> AppResult<Vec<u8>> {
    sys::secret::protect(plain)
}

pub fn unprotect(cipher: &[u8]) -> AppResult<Vec<u8>> {
    sys::secret::unprotect(cipher)
}

// ───────────────────────── 系统信息 ─────────────────────────

pub fn system_visuals() -> SystemVisuals {
    sys::system_info::visuals()
}

/// 可执行文件图标（32×32 RGBA）。
pub fn extract_app_icon(exe: &Path) -> Option<RgbaImage> {
    sys::app_icon::extract(exe)
}

/// 默认的"图片"文件夹。
pub fn pictures_dir() -> Option<std::path::PathBuf> {
    sys::system_info::pictures_dir()
}

/// 启动子进程时不弹控制台窗口（Windows 的 CREATE_NO_WINDOW）。
pub fn hidden_command(program: &Path) -> std::process::Command {
    sys::process::hidden_command(program)
}

/// 让子进程随本进程退出（包括崩溃、被强制结束）而结束，不留孤儿进程。
pub fn tie_to_current_process(child: &std::process::Child) {
    sys::process::tie_to_current_process(child);
}

// ───────────────────────── 系统 OCR ─────────────────────────

pub fn system_ocr_available() -> bool {
    sys::ocr::available()
}

pub fn system_ocr(image: &RgbaImage) -> AppResult<Vec<SysOcrLine>> {
    sys::ocr::recognize(image)
}
