//! 截图流程编排。
//!
//! # 为什么整条链路由 Rust 驱动
//!
//! 规格 01 §3.2 描述的是"热键 → 发 `capture-hotkey` 事件 → 主窗口调
//! `capture_prepare` → 打开遮罩"。这条链路要在隐藏的主窗口 WebView 里绕一圈，
//! 白白多两次 IPC 往返，而从按下热键到遮罩可交互的预算只有 150ms
//! （规格 00 §6.4）。所以这里改成：
//!
//! 1. Rust 侧热键回调直接抓屏，并把**启动时就建好的**遮罩窗口摆到位
//! 2. `capture-hotkey` 事件照旧广播，主窗口需要联动时仍然收得到
//! 3. 向各遮罩窗口发 `capture-session-start`，前端据此调 `capture_prepare()`
//!    拿会话元信息（幂等的读操作）
//! 4. 前端画完底图调 `capture_overlay_ready()`，Rust 才 `show()` 窗口
//!
//! 对前端而言 `capture_prepare` 的返回结构与规格一致，只是不再由它触发抓屏。
//! 遮罩窗口为什么要预建、而不是按规格 01 §2 的表格在热键时创建，见
//! [`overlay`] 的文件头注释 —— 一句话：WebView2 建窗要 400–1000ms，做不到 150ms。
//!
//! # 底图不在 WebView 里
//!
//! 冻结画面由原生底图窗口显示（[`platform::BackdropLayer`]），遮罩 WebView 只负责
//! 压暗和选区。理由和实测数据见那个 trait 的文档。这里要记住的后果是：
//!
//! - 第 4 步不再等底图传输完成，前端拿到会话元信息就可以让窗口显示了
//! - 底图像素**仍然**通过 `shot:` 协议异步进 WebView，供放大镜/取色/马赛克使用，
//!   只是挪出了关键路径
//! - 所有底图窗口操作必须回 UI 线程，见 [`with_ui_thread`]

pub mod backdrop;
pub mod bmp;
pub mod overlay;
pub mod protocol;
pub mod session;
pub mod warmup;

use std::time::Instant;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, AppResult};
use crate::platform;
use crate::state::AppState;

pub use session::{CaptureSession, MonitorShot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureMode {
    Normal,
    Longshot,
    Ocr,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 结束截图时要执行的动作。M0 只实现 `Copy`，其余在各自里程碑落地。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureAction {
    Copy,
    Save,
    Pin,
    Ocr,
    Translate,
    Longshot,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    pub id: u64,
    pub name: String,
    /// 虚拟桌面物理坐标，可能为负。
    pub x: i32,
    pub y: i32,
    /// 抓到的图的真实尺寸（物理像素）。
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
    /// 规格 01 §3.1 里的字段。M0 不落盘 —— 像素通过 `capture_get_image`
    /// 以原始字节走 IPC 直送前端，写 4K PNG 到磁盘会直接吃掉整个延迟预算。
    /// 等 M6 做截图库、真的要持久化时再填。
    pub image_path: Option<String>,
}

/// 窗口矩形。M1 接入窗口枚举后才会有内容，M0 恒为空数组。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRect {
    pub handle: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub title: String,
    pub app_name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturePrepareResult {
    pub monitors: Vec<MonitorSnapshot>,
    pub windows: Vec<WindowRect>,
    pub captured_at: i64,
    /// 底图编码格式的扩展名（`bmp` / `png`），前端拼 `shot:` URL 时要用。
    /// 由环境变量决定，见 [`backdrop::configured`]。
    pub backdrop_format: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureFinishResult {
    pub action: CaptureAction,
    pub width: u32,
    pub height: u32,
}

/// 热键入口。抓屏 + 摆好遮罩窗口，但不显示 —— 等前端画完底图再 show。
pub fn start(app: &AppHandle, mode: CaptureMode) -> AppResult<()> {
    let started_at = Instant::now();
    let state = app.state::<AppState>();

    // 已经在截图中就忽略重复触发。用户连按 F1 不该叠出两层遮罩。
    if state.capture.lock().is_some() {
        tracing::debug!("已有进行中的截图会话，忽略本次热键");
        return Ok(());
    }

    let shots = platform::screen_capture().capture_all()?;
    let captured = started_at.elapsed();
    warmup::note_capture_cost(captured, shots.len());

    let monitors: Vec<_> = shots.iter().map(|(info, _)| info.clone()).collect();
    let shots = shots
        .into_iter()
        .map(|(info, image)| MonitorShot {
            bounds: crate::platform::PhysicalRect::new(
                info.bounds.x,
                info.bounds.y,
                image.width(),
                image.height(),
            ),
            info,
            image,
        })
        .collect();

    let captured_at = Utc::now().timestamp_millis();
    *state.capture.lock() = Some(CaptureSession::new(shots, captured_at, started_at));

    // 先把底图推进原生层。这是"能看见画面"的关键一步，越早排队越好。
    load_backdrops(app, captured_at);

    if let Err(err) = overlay::arrange(app, &monitors) {
        // 备窗失败要把会话清掉，否则下次热键会被"已在截图中"挡住。
        state.capture.lock().take();
        return Err(err);
    }

    let arranged = started_at.elapsed();
    overlay::notify_session(app, &monitors, captured_at);

    tracing::info!(
        monitors = monitors.len(),
        capture_ms = captured.as_millis(),
        arrange_ms = arranged.saturating_sub(captured).as_millis(),
        total_ms = started_at.elapsed().as_millis(),
        ?mode,
        "截图会话已就绪，等待遮罩前端上屏"
    );

    // 主窗口/托盘等需要联动的地方仍然按规格 01 §3.2 收这个事件。
    if let Err(err) = app.emit("capture-hotkey", CaptureHotkeyPayload { mode }) {
        tracing::warn!("广播 capture-hotkey 失败: {err}");
    }

    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureHotkeyPayload {
    mode: CaptureMode,
}

/// 把一段活儿投到 UI 线程上执行。
///
/// 原生底图窗口归创建它的线程所有，所以每一次 ensure/load/show/hide 都必须在同一个
/// 线程上跑（细节见 [`platform::BackdropLayer`]）。而热键回调是在工作线程上
/// （`lib.rs` 的 `trigger_capture` 特意把抓屏挪出了事件循环），命令处理器所在的
/// 线程也不由我们决定，所以一律显式投递，不去猜"这次是不是正好在主线程"。
///
/// 投递是异步的，但主线程队列是 FIFO —— 这一点是"装载一定发生在显示之前"的
/// 唯一保证，别把其中任何一步改成直接调用。
fn with_ui_thread(app: &AppHandle, what: &'static str, job: impl FnOnce(&AppHandle) + Send + 'static) {
    let handle = app.clone();
    let result = app.run_on_main_thread(move || job(&handle));

    if let Err(err) = result {
        tracing::error!(what, "投递到 UI 线程失败: {err}");
    }
}

/// 把各屏底图装进原生底图窗口，并清理已拔掉的显示器。
///
/// `session_id` 用来确认"装的还是当前这次会话" —— 投递到主线程和真正执行之间
/// 用户完全可能已经按了 Esc 又按了一次 F1。
fn load_backdrops(app: &AppHandle, session_id: i64) {
    with_ui_thread(app, "装载底图", move |app| {
        let started = Instant::now();
        let layer = platform::backdrop_layer();

        let state = app.state::<AppState>();
        let guard = state.capture.lock();
        let Some(session) = guard.as_ref() else {
            tracing::debug!("底图装载时会话已结束，跳过");
            return;
        };
        if session.captured_at != session_id {
            tracing::debug!("底图装载时会话已被替换，跳过");
            return;
        }

        let ids: Vec<_> = session.shots.iter().map(|shot| shot.info.id).collect();
        layer.retain(&ids);

        let mut ok = 0usize;
        for shot in &session.shots {
            match layer.load(shot.info.id, &shot.image, shot.bounds) {
                Ok(()) => ok += 1,
                // 一块屏装不上不该让整次截图作废，其余屏照常。
                Err(err) => tracing::error!(monitor = %shot.info.id, "装载底图失败: {err}"),
            }
        }

        tracing::info!(
            loaded = ok,
            of = session.shots.len(),
            load_ms = started.elapsed().as_millis(),
            "底图已推进原生层"
        );
    });
}

pub fn prepare_result(app: &AppHandle) -> AppResult<CapturePrepareResult> {
    let state = app.state::<AppState>();
    let guard = state.capture.lock();
    let session = guard.as_ref().ok_or(AppError::NoCaptureSession)?;

    Ok(CapturePrepareResult {
        monitors: session
            .shots
            .iter()
            .map(|shot| MonitorSnapshot {
                id: shot.info.id.0,
                name: shot.info.name.clone(),
                x: shot.bounds.x,
                y: shot.bounds.y,
                width: shot.bounds.width,
                height: shot.bounds.height,
                scale_factor: shot.info.scale_factor,
                is_primary: shot.info.is_primary,
                image_path: None,
            })
            .collect(),
        // 窗口枚举是 M1 的活（见 02-截图模块.md）。
        windows: Vec::new(),
        captured_at: session.captured_at,
        backdrop_format: backdrop::configured().extension().to_owned(),
    })
}

/// 把某块屏的底图编码交出去。字节经 `shot:` 协议直送 WebView。
///
/// 编码在协议线程上做，但取像素要拿会话锁 —— 所以先把图 clone 出来再编？不。
/// 4K 的 `RgbaImage` 是 33MB，clone 一次就是一次多余的内存搬运。这里选择在
/// 锁内编码：截图会话期间没有别的路径会争这把锁（前端唯一的并发动作是拉底图，
/// 而每块屏只拉一次）。
pub fn image_bytes(
    app: &AppHandle,
    monitor: platform::MonitorId,
    format: backdrop::Format,
) -> AppResult<Vec<u8>> {
    let state = app.state::<AppState>();
    let guard = state.capture.lock();
    let session = guard.as_ref().ok_or(AppError::NoCaptureSession)?;
    let shot = session.shot(monitor)?;

    backdrop::encode(&shot.image, format)
}

/// 遮罩前端挂载完成。
///
/// 预建的窗口里 WebView 何时加载完是不确定的，热键完全可能跑在前端注册监听之前，
/// 那条 `capture-session-start` 就白发了 —— 表现是按下 F1 之后永远没有遮罩，
/// 而且因为会话还挂着，之后每次 F1 都被"已在截图中"挡掉。所以前端就绪时反过来
/// 问一次：现在有没有等着我的会话。
pub fn overlay_boot(app: &AppHandle, monitor: platform::MonitorId) {
    let pending = {
        let state = app.state::<AppState>();
        let guard = state.capture.lock();
        guard.as_ref().map(|session| session.captured_at)
    };

    tracing::debug!(monitor = %monitor, pending = pending.is_some(), "遮罩前端已就绪");

    if let Some(session_id) = pending {
        overlay::notify_one(app, monitor, session_id);
    }
}

/// 遮罩前端准备就绪，可以让这块屏的画面出来了。
///
/// 只等前端确认"压暗层已经在 DOM 里"，**不**等底图 —— 底图在原生层，走的是
/// 另一条路。所以这里的 `prepare_ms` 是唯一还在关键路径上的前端耗时。
pub fn overlay_ready(
    app: &AppHandle,
    monitor: platform::MonitorId,
    prepare_ms: Option<f64>,
) -> AppResult<()> {
    // 会话在这里就查一次，好让"没有会话"能作为错误返回给前端；
    // 真正的显示动作在 UI 线程上做。
    {
        let state = app.state::<AppState>();
        let guard = state.capture.lock();
        guard.as_ref().ok_or(AppError::NoCaptureSession)?;
    }

    tracing::debug!(monitor = %monitor, prepare_ms, "遮罩前端就绪，投递显示");

    with_ui_thread(app, "显示遮罩", move |app| show_pair(app, monitor));

    Ok(())
}

/// 原子地显示某块屏的底图窗口和遮罩窗口。**只能在 UI 线程调用。**
///
/// 两个窗口必须同帧出现：先显示底图会露出一帧没压暗的原始画面（一道闪光），
/// 先显示遮罩则会看到一层压暗盖在真实桌面上。
fn show_pair(app: &AppHandle, monitor: platform::MonitorId) {
    let started = Instant::now();

    let (elapsed, is_primary) = {
        let state = app.state::<AppState>();
        let guard = state.capture.lock();
        let Some(session) = guard.as_ref() else {
            // 投递期间用户按了 Esc。窗口就该保持隐藏。
            tracing::debug!(monitor = %monitor, "显示遮罩时会话已结束，跳过");
            return;
        };
        let is_primary = session
            .shot(monitor)
            .map(|shot| shot.info.is_primary)
            .unwrap_or(false);
        (session.started_at.elapsed(), is_primary)
    };

    if let Err(err) = overlay::show_with_backdrop(app, monitor, is_primary) {
        tracing::error!(monitor = %monitor, "显示遮罩失败: {err}");
        return;
    }

    tracing::info!(
        monitor = %monitor,
        hotkey_to_visible_ms = elapsed.as_millis(),
        show_ms = started.elapsed().as_micros() as f64 / 1000.0,
        "画面已上屏"
    );
}

/// 底图像素送达 WebView 的耗时，单位毫秒。纯埋点。
///
/// 这条路已经不在关键路径上了（画面由原生层负责），但它决定了放大镜、取色、
/// 马赛克这些要读背景像素的功能多久之后可用，所以照量。
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixelTimings {
    /// 取字节：`fetch` 到拿到 Blob。纯传输。
    pub fetch_ms: f64,
    /// 解码：`createImageBitmap`。
    pub decode_ms: f64,
}

/// 前端已经拿到并解好了底图像素。
pub fn overlay_pixels_ready(
    app: &AppHandle,
    monitor: platform::MonitorId,
    timings: PixelTimings,
) {
    let state = app.state::<AppState>();
    let guard = state.capture.lock();
    let Some(session) = guard.as_ref() else {
        // 会话结束得比像素到达更快。不是错误。
        return;
    };

    tracing::info!(
        monitor = %monitor,
        hotkey_to_pixels_ms = session.started_at.elapsed().as_millis(),
        fetch_ms = timings.fetch_ms,
        decode_ms = timings.decode_ms,
        "底图像素已进 WebView（放大镜/取色可用）"
    );
}

pub fn cancel(app: &AppHandle) {
    if teardown(app) {
        tracing::info!("截图已取消");
    }
}

/// 收工：藏窗口、丢会话、让前端把像素放掉。返回是否真的有会话被收掉。
fn teardown(app: &AppHandle) -> bool {
    // 底图窗口和遮罩窗口一起藏。这里不必原子 —— 藏窗口的中间态是"看到真实桌面"，
    // 而那正是收工后应该看到的东西。
    with_ui_thread(app, "隐藏底图", |_| {
        platform::backdrop_layer().hide_all();
    });
    overlay::hide_all(app);
    overlay::notify_session_end(app);

    let state = app.state::<AppState>();
    // 这个中间变量是必需的，不是啰嗦：直接把表达式当返回值写，`state` 的借用
    // 会活到函数返回之后，借用检查器不放行。
    let had_session = state.capture.lock().take().is_some();
    had_session
}

pub fn finish(
    app: &AppHandle,
    bounds: Bounds,
    action: CaptureAction,
) -> AppResult<CaptureFinishResult> {
    let rect = platform::PhysicalRect::new(bounds.x, bounds.y, bounds.width, bounds.height);

    // 只在锁内做裁剪，拿到图就放锁 —— 后面写剪贴板可能耗时。
    let image = {
        let state = app.state::<AppState>();
        let guard = state.capture.lock();
        let session = guard.as_ref().ok_or(AppError::NoCaptureSession)?;
        session.crop(rect)?
    };

    let (width, height) = image.dimensions();

    match action {
        CaptureAction::Copy => write_to_clipboard(app, &image)?,
        other => {
            return Err(AppError::Internal(format!(
                "动作 {other:?} 还未实现（M0 只做复制到剪贴板）"
            )))
        }
    }

    // 图已经交付，会话可以收了。
    teardown(app);

    tracing::info!(width, height, ?action, "截图完成");

    Ok(CaptureFinishResult {
        action,
        width,
        height,
    })
}

fn write_to_clipboard(app: &AppHandle, image: &image::RgbaImage) -> AppResult<()> {
    use tauri_plugin_clipboard_manager::ClipboardExt;

    let (width, height) = image.dimensions();
    let payload = tauri::image::Image::new(image.as_raw(), width, height);

    app.clipboard()
        .write_image(&payload)
        .map_err(|err| AppError::Clipboard(err.to_string()))
}
