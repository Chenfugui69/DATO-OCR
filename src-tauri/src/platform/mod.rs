//! 跨平台抽象层。
//!
//! # 铁律（`docs/spec/07-跨平台抽象层与数据存储.md` 第 1 节）
//!
//! 1. `platform/` 之外的任何 Rust 代码，禁止出现 `#[cfg(windows)]`
//! 2. `platform/` 之外，禁止 `use windows::...`
//! 3. 前端代码里禁止出现平台判断
//! 4. trait 方法签名必须平台中立 —— 不出现 `HWND` / `HMONITOR`，句柄用 `u64` 包装
//!
//! # 完整契约与实现进度
//!
//! 规格 07 第 2 节定义了七个 trait。这里只声明**当前里程碑已经落地**的部分，
//! 其余在进入对应里程碑、读过对应规格文档之后按原文补齐 —— 不预先编造
//! `ClipboardPayload` / `ScrollEvent` 这类还没定义清楚的类型。
//!
//! | trait | 状态 | 落地里程碑 | 规格 |
//! |---|---|---|---|
//! | [`ScreenCapture`]     | 部分（`list_monitors` / `capture_all`） | M0；`capture_monitor` / `capture_region` 待 M5，`capture_window` 待 M1 | 07 §2 |
//! | [`WindowEffects`]     | 部分（`exclude_from_capture`） | M0；`probe_capabilities` / `apply_backdrop` / `set_click_through` 待 M6 | 07 §2、01 §2.3 |
//! | [`SystemInfo`]        | 部分（`is_transparency_enabled` / `is_dark_mode`） | M0；其余待 M6 | 07 §2 |
//! | [`BackdropLayer`]     | 完整 | M0 | **规格外新增**，见下 |
//! | `WindowEnumerator`    | 未实现 | M1 | 02-截图模块.md |
//! | `ClipboardMonitor`    | 未实现 | M4 | 05-剪贴板模块.md |
//! | `InputSimulator`      | 未实现 | M4 | 05-剪贴板模块.md |
//! | `SecretStore`         | 未实现 | M3 | 07 §4.7 |
//! | `ScrollListener`      | 未实现 | M5 | 03-长截图与拼接.md |
//!
//! [`BackdropLayer`] 是规格 07 那七个 trait 之外新加的第八个，因为截图底图不再
//! 进 WebView（原因见该 trait 的文档）。它同样遵守上面那四条铁律。

pub mod types;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
use self::macos as sys;
#[cfg(windows)]
use self::windows as sys;

pub use types::{MonitorId, MonitorInfo, PhysicalRect};

use crate::error::AppResult;
use image::RgbaImage;
use tauri::WebviewWindow;

/// 抓屏。所有返回的图像都是**物理像素**，不做任何缩放。
pub trait ScreenCapture: Send + Sync {
    /// 列出所有显示器，按主屏优先、其余按 x 坐标排序。
    fn list_monitors(&self) -> AppResult<Vec<MonitorInfo>>;

    /// 抓取所有显示器。返回顺序与 [`Self::list_monitors`] 一致。
    ///
    /// 多屏时逐块抓取无法做到严格同一时刻，但相邻几毫秒的差异肉眼不可见。
    ///
    /// 规格 07 §2 的原始签名返回 `(MonitorId, RgbaImage)`。这里改成带回完整的
    /// [`MonitorInfo`]：底层（xcap）的每个属性访问器都要重新查一次显示配置，
    /// 调用方拿到 id 后还得再枚举一遍才能知道几何信息，在 150ms 的预算里这笔
    /// 开销毫无必要。返回类型依旧是平台中立的。
    fn capture_all(&self) -> AppResult<Vec<(MonitorInfo, RgbaImage)>>;

    /// 先付一次抓屏的初始化开销，把结果丢掉。
    ///
    /// 首次抓屏要建 D3D 设备、开 WGC 会话，实测比后续的热抓屏贵 90ms
    /// （137ms vs 48ms），刚好让第一次 F1 冲破 150ms 预算（规格 00 §6.4）。
    /// 启动时空跑一次，用户按下的第一次 F1 就已经是热的。
    ///
    /// 失败只记日志不上报：这是纯优化，此时也还没有窗口可以弹提示。
    fn warm_up(&self);
}

/// 订阅"抓屏热身状态可能已经失效"的时机。
///
/// [`ScreenCapture::warm_up`] 省下来的是进程级的 GPU 侧初始化（Windows 上是 D3D
/// 设备 + WGC 会话）。这份状态**不是永久的**，下面这些事件之后它可能已经没了，
/// 而症状是沉默的：用户某次按 F1 又慢回 137ms，日志里除了 `capture_ms` 变大之外
/// 什么都看不出来。所以要在这些时机重新热身一次 —— 它们都发生在空闲时刻，
/// 这笔开销正好可以在用户按键之前付掉。
///
/// `handler` 会在**平台的 UI 线程**上被调用，所以它必须立刻返回：热身要上百毫秒，
/// 实现里请自己丢给后台线程。
pub fn on_capture_state_lost(handler: fn()) {
    sys::on_capture_state_lost(handler);
}

/// 窗口特效与抓屏可见性。
pub trait WindowEffects: Send + Sync {
    /// 把窗口从"被别人抓屏"的结果里排除掉。
    ///
    /// 用途见规格 07 §4.4：长截图采集时不能把自己的提示条拍进去。
    /// Windows 10 2004 以下不支持，此时返回 `Ok(())` 静默降级。
    fn exclude_from_capture(&self, window: &WebviewWindow, enabled: bool) -> AppResult<()>;

    /// 取 Tauri 窗口的原生句柄（Windows 上是 HWND）。
    ///
    /// 包成 `u64` 是为了守住规格 07 §1 第 4 条铁律：平台句柄不能出现在
    /// `platform/` 之外的签名里。目前唯一的用途是把遮罩窗口交给
    /// [`BackdropLayer::show_above`] 去排 z 序。
    fn native_handle(&self, window: &WebviewWindow) -> AppResult<u64>;
}

/// 冻结底图的原生显示层。
///
/// # 为什么底图不进 WebView
///
/// M0 实测（3840×2160 @150% 单屏）：把底图当图片喂给 WebView，传输那一段是
/// 「约 74ms 固定开销 + 2.4ms/MB」（24.9MB 共 134ms，明细见 `capture/protocol.rs`）。
/// 加上解码和抓屏就顶到 244ms，而预算是 150ms（规格 00 §6.4）。改走原生层后
/// 同一台机器降到 70ms。三条理由：
///
/// 1. **代价随像素数线性涨**。单块 4K 就要 134ms，双 4K 直接翻倍。压缩省不下来
///    ——「像素级一致」是硬要求（规格 08 M0 验收项），只能无损，而桌面上摊着照片
///    壁纸时 PNG 基本压不动（实测比 BMP 还大）。
/// 2. **Chromium 会做色彩管理转换**。广色域屏上底图经过 WebView 的色彩管线就不再
///    和真实桌面逐字节相同，同样违反「像素级一致」。原生位图拷贝没有这一层。
/// 3. 顺带省掉 WebView 里那份 33MB 的解码结果。
///
/// # 底图窗口和遮罩窗口的关系
///
/// 每块屏两个顶层窗口：底图窗口在下，只负责把冻结画面画出来，永不接收输入；
/// 透明的遮罩 WebView 在上，负责压暗、选区、工具条。两者由 DWM 合成，是成熟场景
/// ——注意这**不是**"在 WebView2 同一个窗口内叠原生内容"那种难题。
///
/// 显示必须原子：见 [`Self::show_above`]。
///
/// # 像素照样要送进 WebView，只是不在关键路径上
///
/// 放大镜（6 倍实时取色）、取色快捷键、马赛克都需要背景原始像素，按需 IPC 在
/// 74ms 往返下撑不住 60fps。所以底图像素仍然通过 `shot:` 协议异步流进 WebView，
/// 只是遮罩不再等它才显示：画面 ~70ms 出来，像素 230–330ms 到位，在此之前放大镜
/// 不渲染。用户从看到画面到手开始动至少 200–400ms，感知不到这个差。
pub trait BackdropLayer: Send + Sync {
    /// 为某块屏准备底图窗口。幂等，重复调用只是确认窗口还在。
    ///
    /// 必须在 UI 线程调用（Windows 上窗口归创建它的线程所有）。
    fn ensure(&self, monitor: MonitorId) -> AppResult<()>;

    /// 把这块屏的冻结画面装进底图窗口并摆到 `at`，但**保持隐藏**。
    ///
    /// 装载和显示分开，是为了让显示那一步能和遮罩窗口凑成一次原子操作。
    fn load(&self, monitor: MonitorId, image: &RgbaImage, at: PhysicalRect) -> AppResult<()>;

    /// 把底图窗口显示在 `overlay` 正下方，且两者在同一帧里出现。
    ///
    /// `overlay` 是遮罩窗口的原生句柄（Windows 上是 HWND）。用 `u64` 而不是
    /// 平台句柄类型，是为了守住规格 07 §1 第 4 条铁律。
    ///
    /// 分两次 show 是不行的：中间那一帧会露出没有压暗的原始画面，表现为一道闪光。
    fn show_above(&self, monitor: MonitorId, overlay: u64) -> AppResult<()>;

    /// 藏起所有底图窗口。窗口和像素缓冲都留着复用。
    fn hide_all(&self);

    /// 销毁不在 `keep` 里的底图窗口（显示器被拔掉了）。
    fn retain(&self, keep: &[MonitorId]);
}

/// 系统外观与能力探测。
pub trait SystemInfo: Send + Sync {
    fn is_dark_mode(&self) -> bool;

    /// 系统"透明效果"开关。关闭时必须放弃玻璃材质（规格 00 §6.5）。
    fn is_transparency_enabled(&self) -> bool;
}

/// 进程级初始化。必须在创建任何窗口、调用任何抓屏 API **之前**执行。
///
/// Windows 上这里声明 Per-Monitor-DPI-Aware-V2；少了这一句，混合 DPI 环境下
/// 所有坐标都是错的（规格 07 §4.1）。
pub fn init_process() {
    sys::init_process();
}

pub fn screen_capture() -> &'static dyn ScreenCapture {
    sys::screen_capture()
}

pub fn window_effects() -> &'static dyn WindowEffects {
    sys::window_effects()
}

pub fn backdrop_layer() -> &'static dyn BackdropLayer {
    sys::backdrop_layer()
}

pub fn system_info() -> &'static dyn SystemInfo {
    sys::system_info()
}
