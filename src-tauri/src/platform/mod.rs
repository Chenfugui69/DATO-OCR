//! 跨平台抽象层（规格 07）。
//!
//! **铁律**：`platform/` 之外禁止出现 `#[cfg(windows)]` / `use windows::`，
//! 签名里禁止出现 HWND 之类的平台类型（一律包成 u64）。
//!
//! 规格草案里是 8 个 trait；这里落地成一组模块级函数，由编译期选择 `sys` 实现。
//! 效果相同 —— macOS 移植时只需在 `macos/` 里补齐同名函数，上层零改动 ——
//! 但省掉了 trait object 和一堆只有一个实现的样板。
//!
//! | 能力 | Windows | macOS |
//! |---|---|---|
//! | 抓屏 | xcap + WGC | CGWindowListCreateImage，需「屏幕录制」权限 |
//! | 窗口枚举 | EnumWindows + DWM 扩展边框 | CGWindowListCopyWindowInfo |
//! | 冻结底图层 | 分层窗口 + UpdateLayeredWindow | NSWindow + CALayer |
//! | 浮层窗口 | 置顶窗口 | 不激活应用的 NSPanel |
//! | 剪贴板 | AddClipboardFormatListener | NSPasteboard changeCount 轮询 |
//! | 输入模拟 | SendInput | CGEvent，需「辅助功能」权限 |
//! | 全局钩子 | WH_MOUSE_LL / WH_KEYBOARD_LL | NSEvent 监听 + CGEventTap |
//! | 密钥 | DPAPI | 钥匙串里的主密钥 + AES-GCM |
//! | 系统 OCR | Windows.Media.Ocr | Vision.framework |

mod generic;
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
use tauri::utils::config::WindowEffectsConfig;
use tauri::{AppHandle, Manager, Runtime, WebviewWindow, WebviewWindowBuilder};

use crate::error::AppResult;

/// 进程启动最早期调用：声明 Per-Monitor V2 DPI 感知等。
pub fn init_process() {
    sys::init_process()
}

/// 静默启动（不显示主窗口）完成后调用：别让一个没有窗口的应用占着前台。
pub fn after_silent_start() {
    sys::after_silent_start()
}

/// 用户点了程序坞图标要求重新打开应用（只有 macOS 有这个事件）。
pub fn is_reopen_event(event: &tauri::RunEvent) -> bool {
    sys::is_reopen_event(event)
}

/// 这个平台的默认热键。
pub fn default_hotkeys() -> &'static DefaultHotkeys {
    &sys::DEFAULT_HOTKEYS
}

/// 托盘菜单项的文字：功能名后面带上快捷键，按平台的习惯写。
pub fn menu_label(text: &str, accelerator: &str) -> String {
    sys::menu_label(text, accelerator)
}

/// 左键点托盘图标是不是直接弹菜单（macOS 的习惯）。false = 左键打开主窗口、右键弹菜单。
pub fn tray_menu_on_left_click() -> bool {
    sys::TRAY_MENU_ON_LEFT_CLICK
}

/// 托盘图标是不是单色的"模板图"，由系统按菜单栏深浅自动上色（macOS）。
pub fn tray_icon_is_template() -> bool {
    sys::TRAY_ICON_IS_TEMPLATE
}

/// 要用户手动授予的系统权限的当前状态（macOS 的屏幕录制、辅助功能）。
pub fn permissions() -> Permissions {
    sys::permissions()
}

/// 弹出系统的授权引导 / 打开对应的系统设置页。
pub fn request_permission(which: Permission) {
    sys::request_permission(which)
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

/// 连续录一块屏里的一个区域（GIF 用）。`start(显示器, 区域)`：区域是这块屏内的物理像素；
/// `next` 取这个区域的新画面（不透明 RGBA），没有新画面时等到超时返回 None；drop 时停止。
pub use sys::capture::ScreenRecorder;

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

/// 这个窗口是不是 DATO OCR 自己的。
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

/// 长截图：让滚轮发给这个窗口。Windows 上要把焦点交给它；macOS 的滚轮跟着鼠标走，不用动。
pub fn route_scroll_to(window: WindowHandle) -> AppResult<()> {
    sys::input::route_scroll_to(window)
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

/// 应用设置里的深浅色（system | light | dark）同步给原生层：系统材质（毛玻璃）、原生菜单按它画。
pub fn apply_theme(app: &AppHandle, theme: &str) {
    sys::effects::apply_theme(app, theme);
}

/// 面板的滑入滑出是不是由平台挪窗口来做（见 `slide_window`）。不是的话由页面自己做动画。
/// `glass`：面板开着毛玻璃（窗口就是面板本身，页面在窗口里没有地方可滑）。
pub fn slides_windows(glass: bool) -> bool {
    sys::effects::slides_windows(glass)
}

/// 面板滑入前 / 滑出后待的位置。`rect` 是面板停稳时的位置。
pub fn slide_start(rect: PhysicalRect, monitor: &MonitorInfo) -> PhysicalRect {
    sys::effects::slide_start(rect, monitor)
}

/// 把窗口从现在的位置滑到目标位置（屏幕物理像素），`ms` 毫秒。
pub fn slide_window(window: &WebviewWindow, to: PhysicalRect, ms: u32) {
    sys::effects::slide_window(window, to, ms);
}

/// 悬浮的底部面板盖住程序坞 / 任务栏时，离屏幕底边留多少（逻辑像素）。
pub fn over_dock_gap() -> f64 {
    sys::effects::OVER_DOCK_GAP
}

/// 浮层要不要盖在程序坞上面（macOS：程序坞比普通浮层高一层；别的平台不用管）。
pub fn set_above_dock(window: &WebviewWindow, above: bool) {
    sys::effects::set_above_dock(window, above);
}

/// 窗口不抢焦点地置顶显示（toast 之类）。
/// 让窗口永远不被激活（点它、显示它都不抢焦点）。悬浮按钮这类"浮在别人上面"的窗口用。
pub fn set_no_activate(window: &WebviewWindow) {
    sys::effects::set_no_activate(window);
}

/// 建一个浮层窗口：能浮在别的程序（包括全屏的）上面，收键盘时不把用户正在用的程序切走。
/// 浮层一律用它建，不要直接 `builder.build()`（macOS 上窗口的种类在创建那一刻就定了）。
pub fn build_floating(
    builder: WebviewWindowBuilder<'_, tauri::Wry, AppHandle>,
    kind: FloatingKind,
) -> AppResult<WebviewWindow> {
    sys::effects::build_floating(builder, kind)
}

/// 这个窗口现在是不是用户正在用的那个（收键盘的那个）。
pub fn window_is_foreground(window: &WebviewWindow) -> bool {
    sys::effects::is_foreground(window)
}

/// 把键盘焦点交给窗口（窗口要已经显示）。浮层拿焦点不会把整个应用切到前台。
pub fn take_focus(window: &WebviewWindow) -> AppResult<()> {
    sys::effects::take_focus(window)
}

/// 摆放窗口：外框左上角的屏幕物理坐标 + 内容区的物理尺寸。
pub fn place_window(
    window: &WebviewWindow,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> AppResult<()> {
    sys::effects::place_window(window, x, y, width, height)
}

/// 带标题栏的"正经窗口"（识字、编辑、AI）用什么外框，在建窗时套上。
pub fn frame_window<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    sys::effects::frame_window(builder)
}

/// 主窗口这类窗口的系统材质；`glass` 为 false 时不用材质。
pub fn window_effects(glass: bool) -> Option<WindowEffectsConfig> {
    sys::effects::window_effects(glass)
}

/// 一次同时改窗口位置和大小（屏幕物理像素，和 `MonitorInfo` 同一套坐标）。
pub fn set_bounds(
    window: &WebviewWindow,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> AppResult<()> {
    sys::effects::set_bounds(window, x, y, width, height)
}

/// 同上，单位是逻辑像素（和前端 `window.screenX` 同一套坐标）。
pub fn set_bounds_logical(
    window: &WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> AppResult<()> {
    sys::effects::set_bounds_logical(window, x, y, width, height)
}

/// 把窗口裁成圆角矩形（物理像素半径，0 = 不裁）。
pub fn set_round_region(window: &WebviewWindow, radius: u32) {
    sys::effects::set_round_region(window, radius);
}

/// 毛玻璃背板：`Some(物理像素半径)` 挂上 / 更新，`None` 去掉（见 windows/effects.rs）。只能在 UI 线程调用。
pub fn set_backdrop(window: &WebviewWindow, radius: Option<u32>) -> bool {
    sys::effects::set_backdrop(window, radius)
}

/// 背板只铺窗口里的一块（物理像素），`ms` > 0 时动画过去；`None` = 铺满窗口。只能在 UI 线程调用。
pub fn set_backdrop_rect(window: &WebviewWindow, rect: Option<(f32, f32, f32, f32)>, ms: u32) {
    sys::effects::set_backdrop_rect(window, rect, ms);
}

/// 窗口大小变了，背板的圆角裁剪跟上。
pub fn resize_backdrop(window: &WebviewWindow) -> bool {
    sys::effects::resize_backdrop(window)
}

/// 窗口只保留一个矩形（物理像素）：外面不显示、点击穿透。`None` = 整个窗口。
pub fn set_rect_region(window: &WebviewWindow, rect: Option<(i32, i32, i32, i32)>) {
    sys::effects::set_rect_region(window, rect);
}

/// 系统圆角（Win11）。
pub fn set_rounded(window: &WebviewWindow, rounded: bool) {
    sys::effects::set_rounded(window, rounded);
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

    /// 底图和会话共用同一份像素（`Arc`），不拷贝。
    pub fn load(
        monitor: MonitorId,
        image: &std::sync::Arc<RgbaImage>,
        at: PhysicalRect,
    ) -> AppResult<()> {
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

/// 删掉当前用户"开机自启"里叫 `name` 的那一项（品牌改名后清旧名字用）。删掉了返回 true。
pub fn remove_autostart_entry(name: &str) -> bool {
    sys::system_info::remove_autostart_entry(name)
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

/// 系统 OCR 引擎的名字（识字结果里显示）。
pub fn system_ocr_name() -> &'static str {
    sys::SYSTEM_OCR_NAME
}

/// 这个系统上有没有可下载的 PaddleOCR 引擎。
pub fn paddle_ocr_supported() -> bool {
    sys::PADDLE_OCR_SUPPORTED
}

pub fn system_ocr(image: &RgbaImage) -> AppResult<Vec<SysOcrLine>> {
    sys::ocr::recognize(image)
}
