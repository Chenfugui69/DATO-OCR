//! 截图遮罩窗口的生命周期。
//!
//! 每块显示器一个窗口，标签固定为 `capture-{monitor_id}`（规格 01 §2）。
//! 前端靠自己的窗口标签知道"我负责哪块屏"，不用往 URL 上挂查询串。
//!
//! 遮罩窗口是**透明的，只画压暗层和选区**；冻结的屏幕画面由它下面的原生底图窗口
//! 负责（`platform::BackdropLayer`）。两者是一对，显示时必须同帧，见
//! [`show_with_backdrop`]。
//!
//! # 为什么窗口在启动时就建好、之后只 show/hide
//!
//! 规格 01 §2 的表格把截图窗口的"创建时机"写成"按下截图热键"。M0 实测证明这条
//! 做不到：WebView2 建一个窗口要 400–1000ms，光这一步就把 150ms 的预算超了 5 倍
//! （首次 1941ms，之后 ~730ms）。所以改成和同一份规格里 `main`、`clipboard`
//! 两个窗口一样的做法 —— 启动时建好并隐藏，热键只做 show()。规格 05 §5 对剪贴板
//! 面板的原话是"常驻但隐藏，不要每次打开都创建，打开只是 show() + 刷新数据"，
//! 这里是同一个道理，只是 01 的表格没同步。
//!
//! 代价是每块屏常驻一个 WebView2（约 40–60MB）。这和规格 00 的 80MB 空闲内存
//! 目标有冲突，M7 调优时可能要改成"用完延迟若干秒再销毁"的折中。

use std::time::Instant;

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::error::{AppError, AppResult};
use crate::platform::{self, MonitorId, MonitorInfo};

const LABEL_PREFIX: &str = "capture-";

pub fn label_for(monitor: MonitorId) -> String {
    format!("{LABEL_PREFIX}{}", monitor.0)
}

/// 启动时预建所有遮罩窗口和底图窗口。
///
/// 失败不致命 —— 热键那条路会兜底补建，只是会慢。
///
/// **必须在 UI 线程调用**（底图窗口的所有权要求，见 `platform::BackdropLayer`）。
/// 现在的调用点是 `RunEvent::Ready`，满足这个条件。
pub fn prewarm(app: &AppHandle) {
    let started = Instant::now();

    let monitors = match platform::screen_capture().list_monitors() {
        Ok(monitors) => monitors,
        Err(err) => {
            tracing::warn!("预建遮罩窗口时枚举显示器失败: {err}");
            return;
        }
    };

    let mut ok = 0usize;
    for info in &monitors {
        match ensure_one(app, info) {
            Ok(_) => ok += 1,
            Err(err) => tracing::warn!(monitor = %info.id, "预建遮罩窗口失败: {err}"),
        }
        // 底图窗口是纯 Win32 窗口，建起来比 WebView 便宜得多，但同样先建好
        // —— 热键路径上不该出现任何建窗动作。
        if let Err(err) = platform::backdrop_layer().ensure(info.id) {
            tracing::warn!(monitor = %info.id, "预建底图窗口失败: {err}");
        }
    }

    tracing::info!(
        monitors = monitors.len(),
        created = ok,
        elapsed_ms = started.elapsed().as_millis(),
        "遮罩窗口预建完成"
    );
}

/// 热键路径：把窗口摆到当前显示器几何上，并清理已拔掉的显示器留下的窗口。
///
/// 正常情况下这里只有 set_position/set_size 两次系统调用，几毫秒。只有在预建
/// 之后显示器配置变过（插拔、改分辨率）时才会走到建窗的慢路径。
pub fn arrange(app: &AppHandle, monitors: &[MonitorInfo]) -> AppResult<()> {
    for (label, window) in app.webview_windows() {
        if !label.starts_with(LABEL_PREFIX) {
            continue;
        }
        let still_there = monitors.iter().any(|info| label_for(info.id) == label);
        if !still_there {
            tracing::info!(%label, "显示器已移除，销毁对应遮罩窗口");
            let _ = window.close();
        }
    }

    let mut usable = 0usize;
    for info in monitors {
        match ensure_one(app, info) {
            Ok(_) => usable += 1,
            // 一块屏建不起来不该让整次截图作废，其余屏照常用。
            Err(err) => tracing::error!(monitor = %info.id, "准备遮罩窗口失败: {err}"),
        }
    }

    if usable == 0 {
        return Err(AppError::Window("没有任何可用的遮罩窗口".into()));
    }

    Ok(())
}

/// 通知各遮罩"新会话开始了，去取底图"。
///
/// 窗口是复用的，所以必须靠事件让前端重置选区状态并重新拉图；靠 onMount 是不行的，
/// mount 只在启动预建时发生一次。
pub fn notify_session(app: &AppHandle, monitors: &[MonitorInfo], session_id: i64) {
    for info in monitors {
        notify_one(app, info.id, session_id);
    }
}

/// 只通知一块屏。前端按 `sessionId` 去重，所以重复发是安全的。
pub fn notify_one(app: &AppHandle, monitor: MonitorId, session_id: i64) {
    let label = label_for(monitor);
    let payload = SessionStartPayload {
        session_id,
        monitor: monitor.0,
    };
    if let Err(err) = app.emit_to(label.as_str(), "capture-session-start", payload) {
        tracing::error!(%label, "广播 capture-session-start 失败: {err}");
    }
}

/// 通知各遮罩"会话结束，把像素放掉"。
///
/// 窗口不销毁，所以前端为放大镜/取色留的那张 4K 底图（约 33MB）不会随窗口一起
/// 消失。不主动放掉的话，多屏时空闲内存会被几张底图长期占着，撑破规格 00 的
/// 80MB 目标。
pub fn notify_session_end(app: &AppHandle) {
    if let Err(err) = app.emit("capture-session-end", ()) {
        tracing::error!("广播 capture-session-end 失败: {err}");
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionStartPayload {
    session_id: i64,
    monitor: u64,
}

fn ensure_one(app: &AppHandle, info: &MonitorInfo) -> AppResult<WebviewWindow> {
    let label = label_for(info.id);

    if let Some(existing) = app.get_webview_window(&label) {
        place(&existing, info)?;
        return Ok(existing);
    }

    // 构建器的 position/inner_size 只接受逻辑单位，混合 DPI 下换算不可靠。
    // 这里先给个粗略的逻辑值把窗口放到目标显示器上（这样它能解析到正确的
    // scale factor），随后再用物理像素精确定位。
    let logical_scale = if info.scale_factor > 0.0 {
        info.scale_factor
    } else {
        1.0
    };

    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("capture.html".into()))
        .title("CHENOCR")
        .position(
            f64::from(info.bounds.x) / logical_scale,
            f64::from(info.bounds.y) / logical_scale,
        )
        .inner_size(
            f64::from(info.bounds.width) / logical_scale,
            f64::from(info.bounds.height) / logical_scale,
        )
        .decorations(false)
        // 遮罩窗口**必须**真透明才能做镂空效果（规格 01 §2.2）。
        // 这和主窗口的处理正好相反。
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .shadow(false)
        .visible(false)
        // 焦点在 show 的时候再抢。建窗阶段就抢焦点会在启动时把用户正在打字的
        // 窗口顶掉 —— 这些窗口是开机预建的，不是用户主动打开的。
        .focused(false)
        // 留着这条日志：遮罩是隐藏窗口，页面加载失败的话前端一声不响，只表现为
        // 按 F1 没反应。排查时第一眼要看的就是这里的 url 对不对
        // —— 用 `cargo build` 直接编出来的包会指向 devUrl，而不是内嵌资源。
        .on_page_load(|window, payload| {
            tracing::debug!(
                label = window.label(),
                url = %payload.url(),
                event = ?payload.event(),
                "遮罩页面加载事件"
            );
        })
        .build()?;

    place(&window, info)?;

    // 别把自己拍进别人的抓屏结果（规格 07 §4.4）。
    if let Err(err) = platform::window_effects().exclude_from_capture(&window, true) {
        tracing::debug!(monitor = %info.id, "遮罩窗口排除抓屏失败: {err}");
    }

    Ok(window)
}

/// 用物理像素精确摆放，避免逻辑像素换算带来的 1px 错位和模糊。
fn place(window: &WebviewWindow, info: &MonitorInfo) -> AppResult<()> {
    window.set_position(PhysicalPosition::new(info.bounds.x, info.bounds.y))?;
    window.set_size(PhysicalSize::new(info.bounds.width, info.bounds.height))?;
    // Windows 上先设尺寸再设位置有时会被 DWM 挪回去，再确认一次位置。
    window.set_position(PhysicalPosition::new(info.bounds.x, info.bounds.y))?;
    Ok(())
}

/// 把底图窗口和遮罩窗口在同一帧里显示出来。**只能在 UI 线程调用。**
///
/// 底图在下、遮罩在上，由 `DeferWindowPos` 一个批次提交。分两次 show 会露出
/// 一帧没压暗的原始画面 —— 那是一道很显眼的闪光。
///
/// # 为什么批次之后还要再调一次 Tauri 的 `show()`
///
/// 因为 tao 自己记着一份可见性状态（`WindowFlags::VISIBLE`），而它只在
/// `Window::set_visible` 里更新。我们用原生 `SWP_SHOWWINDOW` 把窗口显示出来，
/// 绕过了那条路，tao 那份状态就还是"隐藏"。后果不止是状态对不上：
/// `set_focus()` 会先检查这个标志位，发现"窗口是隐藏的"就直接什么都不做
/// —— 表现是遮罩明明在屏幕上，Esc 和 Enter 却全都没反应。
///
/// 所以这里补一次 `show()`：此刻窗口在系统层面已经可见了，那次 ShowWindow
/// 是空操作，唯一的作用就是把 tao 的状态同步过来。顺序不能反 —— 放在批次之前的话
/// 它会真的把遮罩提前显示出来，原子性就白做了。
pub fn show_with_backdrop(app: &AppHandle, monitor: MonitorId, focus: bool) -> AppResult<()> {
    let label = label_for(monitor);
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| AppError::Window(format!("遮罩窗口 {label} 不存在")))?;

    let handle = platform::window_effects().native_handle(&window)?;
    platform::backdrop_layer().show_above(monitor, handle)?;

    window.show()?;

    // 焦点要单独抢：批次里带的是 `SWP_NOACTIVATE`（底图窗口绝不能被激活）。
    if focus {
        window.set_focus()?;
    }

    Ok(())
}

/// 收工时藏起所有遮罩窗口。
///
/// 刻意用 hide 而不是 close —— 窗口要留着给下一次热键复用，见文件头的说明。
/// 按标签前缀全量清扫而不是只处理本次会话记录的那几个，这样即使会话状态和
/// 实际窗口对不上（建窗中途失败之类），也不会留下一块盖住屏幕的遮罩。
pub fn hide_all(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if !label.starts_with(LABEL_PREFIX) {
            continue;
        }
        if let Err(err) = window.hide() {
            tracing::error!(%label, "隐藏遮罩窗口失败: {err}");
        }
    }
}
