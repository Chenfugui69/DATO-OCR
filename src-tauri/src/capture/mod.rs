//! 截图流程编排（规格 02）。
//!
//! 整条链路由 Rust 驱动，不经主窗口中转：
//!
//! 1. 热键回调 → 工作线程 `start`：记前台窗口 → 抓全部显示器 → 枚举窗口 → 建会话
//! 2. UI 线程：摆好预建的遮罩窗口，把冻结画面装进隐藏的原生底图层，给每个遮罩发
//!    `capture-session-start`
//! 3. 遮罩前端拉会话信息、画好压暗层，调 `capture_overlay_ready` → Rust 把底图和遮罩
//!    原子上屏（实测 4K 单屏热键到可见 ~70ms）
//! 4. 原始像素随后经 `shot:` 协议异步进 WebView（放大镜/取色/马赛克用），不在关键路径
//! 5. 用户完成 → `capture_finish`：裁剪 + 叠标注 → 复制/保存/贴图/识字/翻译/长截图

pub mod overlay;
pub mod record_prep;
pub mod region_translate;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::RgbaImage;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tauri_plugin_dialog::DialogExt;

use crate::error::{AppError, AppResult};
use crate::platform::{
    self, ClipboardPayload, MonitorId, MonitorInfo, PhysicalRect, WindowHandle, WindowInfo,
};
use crate::settings::CaptureSettings;
use crate::state::state;
use crate::{ai, events, imaging, library, longshot, ocr, pin, wm};

/// 每个窗口最多记这么多子控件（有的程序几千个，再多对框选没帮助，只拖慢）
const MAX_CHILDREN_PER_WINDOW: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureIntent {
    Normal,
    Ocr,
    Longshot,
    Translate,
}

pub struct MonitorShot {
    pub info: MonitorInfo,
    pub image: Arc<RgbaImage>,
}

pub struct Session {
    pub id: u64,
    pub intent: CaptureIntent,
    pub monitors: Vec<MonitorShot>,
    pub windows: Vec<WindowInfo>,
    pub previous: Option<WindowHandle>,
    pub source_app: Option<String>,
    pub cursor: Option<(i32, i32)>,
    pub started: Instant,
    shown: Mutex<HashSet<u64>>,
}

impl Session {
    pub fn monitor(&self, id: MonitorId) -> Option<&MonitorShot> {
        self.monitors.iter().find(|m| m.info.id == id)
    }

    /// 键盘焦点给鼠标所在的屏；取不到鼠标就给主屏。
    fn focus_monitor(&self) -> Option<MonitorId> {
        self.cursor
            .and_then(|(x, y)| {
                self.monitors
                    .iter()
                    .find(|m| m.info.bounds.contains_point(x, y))
            })
            .or_else(|| self.monitors.iter().find(|m| m.info.is_primary))
            .or_else(|| self.monitors.first())
            .map(|m| m.info.id)
    }
}

#[derive(Default)]
pub struct CaptureState {
    current: Mutex<Option<Arc<Session>>>,
    /// 录制中又开的一次截图（录 DATO 自己的操作）：录制那个会话先放在这里，截图结束再放回 `current`
    outer: Mutex<Option<Arc<Session>>>,
    /// 截图/长截图进行中。热键再按一次直接忽略（规格 02 §3.9.1）
    busy: AtomicBool,
    next_id: AtomicU64,
    /// 从主窗口按钮开始的截图：主窗口被收起来了（见 `stash_main`），截图结束（含录完 GIF / 长截图、
    /// 取消、失败）要放回来
    main_stashed: AtomicBool,
}

impl CaptureState {
    pub fn current(&self) -> Option<Arc<Session>> {
        self.current.lock().clone()
    }

    /// 截图 / 长截图进行中
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    pub(crate) fn release_busy(&self) {
        self.busy.store(false, Ordering::SeqCst);
    }

    /// 现在这次截图是录制中开的
    pub fn is_nested(&self) -> bool {
        self.outer.lock().is_some()
    }

    fn take_main_stashed(&self) -> bool {
        self.main_stashed.swap(false, Ordering::SeqCst)
    }
}

/// 从主窗口的按钮开始截图：主窗口不能被拍进冻结的画面里。截图结束时 `restore_main` 放回来。
///
/// Windows 上不藏它，只是排除出抓屏：窗口原地不动，截完撤掉排除就行。藏起来再显示的话，碰上
/// 识字 / AI 窗口第一次建出来，主窗口的 Mica 会被画成浅色（侧边栏一片发白，属性却都对），
/// 要等别的事情触发重绘才恢复。macOS 上还是藏起来（新系统的抓屏不一定认"排除"）。
pub fn stash_main(app: &AppHandle) {
    let st = state(app);
    let Some(main) = tauri::Manager::get_webview_window(app, wm::MAIN) else {
        return;
    };
    if st.capture.is_busy() || !main.is_visible().unwrap_or(false) {
        return;
    }
    if cfg!(windows) {
        platform::set_exclude_from_capture(&main, true);
        // 排除在 DWM 下一帧合成时才生效
        std::thread::sleep(Duration::from_millis(50));
    } else {
        let _ = main.hide();
        // 等系统的收起动画走完
        std::thread::sleep(Duration::from_millis(220));
    }
    st.capture.main_stashed.store(true, Ordering::SeqCst);
}

/// 把 `stash_main` 收起的主窗口放回来。只能在 UI 线程调用。
/// macOS 上是重新显示：`focus` 时让它拿焦点（取消、复制、保存这类回到原处的结束方式），
/// 否则焦点还给这时的前台窗口（刚弹出来的贴图之类）。
fn restore_main(app: &AppHandle, focus: bool) {
    if !state(app).capture.take_main_stashed() {
        return;
    }
    // 这期间用户把主窗口收进了程序坞：别给它还原出来
    if tauri::Manager::get_webview_window(app, wm::MAIN)
        .is_some_and(|w| w.is_minimized().unwrap_or(false))
    {
        return;
    }
    if cfg!(windows) {
        if let Some(main) = tauri::Manager::get_webview_window(app, wm::MAIN) {
            platform::set_exclude_from_capture(&main, false);
        }
        return;
    }
    let mut front = (!focus).then(platform::foreground_window).flatten();
    // 前台还是刚藏起来的遮罩（识字、翻译这些，新窗口稍后才弹出来）：不还，焦点给一个看不见的窗口键盘就没地方去了
    overlay::for_each(app, |w| {
        if front.is_some_and(|f| platform::native_handle(w).is_ok_and(|h| h == f.0)) {
            front = None;
        }
    });
    wm::show_main(app, None);
    if let Some(front) = front {
        let _ = platform::focus_window(front);
    }
}

pub fn image_id(session: u64, monitor: MonitorId) -> String {
    format!("cap-{session}-{}", monitor.0)
}

/// 热键 / 托盘入口。抓屏是重活，放工作线程。
pub fn trigger(app: &AppHandle, intent: CaptureIntent) {
    let st = state(app);
    if st.capture.busy.load(Ordering::SeqCst) {
        // 录制中按截图热键：照常开一次截图，录制不停 —— 用 DATO 截图的过程也录进去。
        // 录制的控制条先让位，截完再回来（见 `end_session`）
        if crate::gif_record::is_active() && !st.capture.is_nested() {
            let app = app.clone();
            std::thread::spawn(move || {
                if let Err(err) = start(&app, intent, false, true) {
                    tracing::error!("录制中截图没开起来：{err}");
                    wm::toast(&app, "error", format!("截图失败：{err}"));
                }
            });
            return;
        }
        if st.capture.current().is_some() {
            let _ = app.emit(events::CAPTURE_HOTKEY, intent);
        }
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(err) = start(&app, intent, false, false) {
            report_start_failure(&app, &err);
        }
    });
}

/// 截图没能开始。缺系统权限时光弹个提示不够：提示又小又短，而从主窗口点的截图，主窗口这时
/// 已经藏起来了，用户看到的就是"点了没反应"。把主窗口叫回来，停在设置页的"系统权限"上。
fn report_start_failure(app: &AppHandle, err: &AppError) {
    tracing::error!("启动截图失败：{err}");
    let ui_app = app.clone();
    let _ = app.run_on_main_thread(move || restore_main(&ui_app, true));
    if platform::permissions().screen_capture == Some(false) {
        wm::show_main(app, Some("settings:permissions"));
        wm::toast(
            app,
            "error",
            "还没有「屏幕录制」权限，请按设置页里的说明授权",
        );
    } else {
        wm::toast(app, "error", format!("截图失败：{err}"));
    }
}

/// 瞬间截屏：按下热键就抓整个桌面（带鼠标指针），让一碰就消失的弹窗、悬停提示来不及反应。
/// 抓完按设置：照常框选 / 直接复制整屏 / 直接保存整屏。
pub fn trigger_instant(app: &AppHandle) {
    if state(app).capture.busy.load(Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(err) = start(&app, CaptureIntent::Normal, true, false) {
            report_start_failure(&app, &err);
        }
    });
}

/// `nested`：录制中开的截图（录制占着截图的"忙"，不再抢；录制的会话先收到 `outer`）
fn start(app: &AppHandle, intent: CaptureIntent, instant: bool, nested: bool) -> AppResult<()> {
    let st = state(app);
    if !nested && st.capture.busy.swap(true, Ordering::SeqCst) {
        tracing::debug!("已在截图中，忽略这次热键");
        return Ok(());
    }
    let started = Instant::now();
    // 提示条、划词按钮平时能被抓屏看到（给远程控制的对面看），抓屏之前先排除
    wm::conceal_all_floating(app);
    let previous = platform::foreground_window();
    let cursor = platform::cursor_position();
    let settings = st.settings.read().capture.clone();

    // 窗口和子控件跟抓屏并行枚举，而且必须赶在遮罩出现之前：Chromium 系浏览器发现自己
    // 被完全遮住会把网页内容区那个子窗口藏起来，遮罩一盖上就枚举不到了
    let (shots, windows, grab_ms, enum_ms) = std::thread::scope(|scope| {
        let windows = scope.spawn(|| {
            let t = Instant::now();
            let w = enumerate_for_detection(&settings);
            (w, t.elapsed().as_millis() as u64)
        });
        let t = Instant::now();
        let shots = platform::capture_all();
        let grab_ms = t.elapsed().as_millis() as u64;
        let (windows, enum_ms) = windows.join().unwrap_or_default();
        (shots, windows, grab_ms, enum_ms)
    });
    let mut shots = match shots {
        Ok(shots) => shots,
        Err(err) => {
            if !nested {
                st.capture.release_busy();
            }
            return Err(err);
        }
    };
    if instant && settings.instant_cursor {
        for (info, image) in &mut shots {
            platform::draw_cursor(image, (info.bounds.x, info.bounds.y));
        }
    }
    if instant && settings.instant_action != "select" {
        // 不进遮罩：鼠标所在那块屏的整屏图直接复制 / 保存
        st.capture.release_busy();
        let (cx, cy) = cursor.unwrap_or((0, 0));
        let pick = shots
            .iter()
            .position(|(info, _)| info.bounds.contains_point(cx, cy))
            .unwrap_or(0);
        if pick >= shots.len() {
            return Err(AppError::msg("没有抓到屏幕"));
        }
        let (info, image) = shots.swap_remove(pick);
        let action = if settings.instant_action == "save" {
            FinishAction::Save
        } else {
            FinishAction::Copy
        };
        let source_app = previous.and_then(platform::app_info_of).map(|a| a.name);
        tracing::info!(action = %settings.instant_action, "瞬间截屏（整屏）");
        return perform(
            app,
            action,
            Arc::new(image),
            info.bounds,
            info.scale_factor,
            source_app,
            false,
        );
    }
    let capture_ms = started.elapsed().as_millis() as u64;
    if capture_ms > 100 * shots.len() as u64 {
        // 正常热态单屏 ~50ms。显著变慢通常是 GPU 状态丢了（驱动重置之类），留个痕迹
        // 抓屏和枚举窗口是并行的，分开记，看是哪个慢
        tracing::warn!(capture_ms, grab_ms, enum_ms, windows = windows.len(), "抓屏明显变慢");
    }

    let source_app = previous.and_then(platform::app_info_of).map(|a| a.name);

    let id = st.capture.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    let monitors: Vec<MonitorShot> = shots
        .into_iter()
        .map(|(info, image)| {
            let image = Arc::new(image);
            st.images.put(image_id(id, info.id), image.clone());
            MonitorShot { info, image }
        })
        .collect();
    let session = Arc::new(Session {
        id,
        intent,
        monitors,
        windows,
        previous,
        source_app,
        cursor,
        started,
        shown: Mutex::new(HashSet::new()),
    });
    {
        let mut current = st.capture.current.lock();
        if nested {
            *st.capture.outer.lock() = current.take();
            crate::gif_record::suspend_ui();
        }
        *current = Some(session.clone());
    }
    tracing::info!(session = id, ?intent, capture_ms, nested, "截图会话开始");

    let ui_app = app.clone();
    let ui_session = session.clone();
    app.run_on_main_thread(move || {
        if nested {
            // 录制时遮罩是鼠标穿透的，截图要收鼠标
            overlay::for_each(&ui_app, |w| {
                let _ = w.set_ignore_cursor_events(false);
            });
        }
        let infos: Vec<MonitorInfo> = ui_session.monitors.iter().map(|m| m.info.clone()).collect();
        if let Err(err) = overlay::arrange(&ui_app, &infos) {
            tracing::error!("摆放遮罩失败：{err}");
            end_session(&ui_app, ui_session.id, true);
            return;
        }
        for m in &ui_session.monitors {
            if let Err(err) = platform::backdrop::load(m.info.id, &m.image, m.info.bounds) {
                tracing::error!(monitor = %m.info.id, "装载底图失败：{err}");
            }
            let label = overlay::label_for(m.info.id);
            let _ = ui_app.emit_to(
                label.as_str(),
                events::CAPTURE_SESSION_START,
                SessionStart {
                    session_id: ui_session.id,
                },
            );
        }
    })?;

    // 看门狗：遮罩迟迟不报到（页面没加载、前端异常）就收回会话，
    // 否则之后每次热键都会被"已在截图中"挡掉，用户看到的是截图彻底失灵。
    let wd_app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(3000));
        let st = state(&wd_app);
        let stuck = st
            .capture
            .current()
            .is_some_and(|s| s.id == id && s.shown.lock().is_empty());
        if stuck {
            tracing::error!(session = id, "遮罩 3 秒内未就绪，取消本次截图");
            end_session(&wd_app, id, true);
        }
    });
    Ok(())
}

/// 自动框选用的窗口列表（按 Z 序）及各自的子控件。
fn enumerate_for_detection(settings: &CaptureSettings) -> Vec<WindowInfo> {
    if !settings.detect_windows {
        return Vec::new();
    }
    let mut windows = platform::enumerate_windows().unwrap_or_else(|err| {
        tracing::warn!("枚举窗口失败：{err}");
        Vec::new()
    });
    if settings.detect_child_windows {
        for w in &mut windows {
            let mut children = platform::enumerate_children(w.handle).unwrap_or_default();
            // 和窗口一样大（或更大）的子窗口没有区分意义，比如 Chromium 的 D3D 中间层
            children.retain(|c| c.area() < w.bounds.area());
            children.truncate(MAX_CHILDREN_PER_WINDOW);
            w.children = children;
        }
    }
    windows
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionStart {
    session_id: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: u64,
    pub intent: CaptureIntent,
    pub monitor: MonitorInfo,
    pub image_url_id: String,
    pub windows: Vec<WindowInfo>,
    /// 鼠标位置（本屏局部物理坐标）；不在本屏为 None
    pub cursor: Option<(i32, i32)>,
    pub has_focus: bool,
    /// 录制中开的截图：工具条上不放录制、长截图
    pub recording: bool,
    pub settings: CaptureSettings,
}

pub fn session_info(app: &AppHandle, monitor: MonitorId) -> AppResult<Option<SessionInfo>> {
    let st = state(app);
    let Some(session) = st.capture.current() else {
        return Ok(None);
    };
    let Some(shot) = session.monitor(monitor) else {
        return Ok(None);
    };
    let b = shot.info.bounds;
    let windows = session
        .windows
        .iter()
        .filter(|w| w.bounds.intersect(&b).is_some())
        .cloned()
        .collect();
    let settings = st.settings.read().capture.clone();
    Ok(Some(SessionInfo {
        session_id: session.id,
        intent: session.intent,
        monitor: shot.info.clone(),
        image_url_id: image_id(session.id, monitor),
        windows,
        cursor: session
            .cursor
            .filter(|(x, y)| b.contains_point(*x, *y))
            .map(|(x, y)| (x - b.x, y - b.y)),
        has_focus: session.focus_monitor() == Some(monitor),
        recording: st.capture.is_nested(),
        settings,
    }))
}

/// 遮罩已画好压暗层 → 和底图一起上屏。
pub fn overlay_ready(app: &AppHandle, session_id: u64, monitor: MonitorId) -> AppResult<()> {
    let st = state(app);
    let Some(session) = st.capture.current().filter(|s| s.id == session_id) else {
        return Ok(());
    };
    if !session.shown.lock().insert(monitor.0) {
        return Ok(());
    }
    let focus_monitor = session.focus_monitor();
    let focus = focus_monitor == Some(monitor);
    // 鼠标所在那块屏的遮罩是不是已经显示了（它先好、这块后好）
    let focused_shown = focus_monitor.filter(|m| !focus && session.shown.lock().contains(&m.0));
    let elapsed = session.started.elapsed().as_millis() as u64;
    let ui_app = app.clone();
    app.run_on_main_thread(move || {
        if let Err(err) = overlay::show_with_backdrop(&ui_app, monitor, focus) {
            tracing::error!(%monitor, "遮罩上屏失败：{err}");
        }
        // 多块屏：别的屏的遮罩后显示时会把键盘焦点带走（显示窗口就会抢），鼠标所在那块的遮罩
        // 就不是焦点窗口了 —— 第一下点击被系统拿去"激活窗口"，要再点一下才能拖。把焦点还回去
        if let Some(m) = focused_shown {
            overlay::refocus(&ui_app, m);
        }
        tracing::info!(session = session_id, %monitor, hotkey_to_visible_ms = elapsed, "遮罩可见");
    })?;
    Ok(())
}

/// 结束会话：藏遮罩、释放像素、可选地把焦点还给截图前的窗口。
pub fn end_session(app: &AppHandle, session_id: u64, restore_focus: bool) {
    let st = state(app);
    // 录制中开的截图还没结束，录制先结束了：录制的会话收掉就行，遮罩还归这次截图用
    {
        let mut outer = st.capture.outer.lock();
        if outer.as_ref().is_some_and(|s| s.id == session_id) {
            *outer = None;
            st.images.remove_prefix(&format!("cap-{session_id}-"));
            return;
        }
    }
    let session = {
        let mut current = st.capture.current.lock();
        match current.as_ref() {
            Some(s) if s.id == session_id => current.take(),
            _ => None,
        }
    };
    let Some(session) = session else { return };
    record_prep::leave(app, false);
    st.images.remove_prefix(&format!("cap-{session_id}-"));
    // 录制中开的截图结束了：会话、遮罩都还给录制，录制的控制条回来
    if let Some(outer) = st.capture.outer.lock().take() {
        if crate::gif_record::is_active() {
            *st.capture.current.lock() = Some(outer);
            let ui_app = app.clone();
            let previous = session.previous;
            let _ = app.run_on_main_thread(move || {
                let _ = ui_app.emit(events::CAPTURE_SESSION_END, ());
                crate::gif_record::resume_ui(&ui_app);
                if restore_focus {
                    if let Some(prev) = previous {
                        let _ = platform::focus_window(prev);
                    }
                }
            });
            return;
        }
    }
    let ui_app = app.clone();
    let previous = session.previous;
    let _ = app.run_on_main_thread(move || {
        overlay::hide_all(&ui_app);
        let _ = ui_app.emit(events::CAPTURE_SESSION_END, ());
        if restore_focus {
            if let Some(prev) = previous {
                let _ = platform::focus_window(prev);
            }
        }
        restore_main(&ui_app, restore_focus);
        state(&ui_app).capture.release_busy();
    });
}

/// 取消正在进行的截图 / 长截图（命令行 `--action=cancel`，脚本和自动化测试用）。
pub fn cancel_current(app: &AppHandle) {
    if state(app).longshot.is_active() {
        longshot::abort(app);
    } else if let Some(session) = state(app).capture.current() {
        end_session(app, session.id, true);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FinishAction {
    Copy,
    Save,
    Pin,
    Ocr,
    Translate,
    Longshot,
    /// 带着截图问 AI
    Ai,
    /// 录制选区为 GIF
    Gif,
    /// 录制选区为视频（MP4）
    Video,
    /// 录制选区，格式按设置（录制栏里选的 MP4 / GIF / APNG）
    Record,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishMeta {
    pub session_id: u64,
    pub monitor_id: MonitorId,
    /// 本屏局部物理坐标
    pub rect: PhysicalRect,
    pub action: FinishAction,
    /// 标注层 PNG 左上角（本屏局部物理坐标）；没有标注时为 None
    pub annotation_at: Option<(i32, i32)>,
    /// 自由选区的轮廓（本屏局部物理坐标）；`rect` 是它的外接矩形。轮廓以外截出来是透明的
    #[serde(default)]
    pub shape: Option<Vec<[f64; 2]>>,
}

pub fn finish(app: &AppHandle, meta: FinishMeta, annotation_png: Vec<u8>) -> AppResult<()> {
    let st = state(app);
    let session = st
        .capture
        .current()
        .filter(|s| s.id == meta.session_id)
        .ok_or_else(|| AppError::msg("截图会话已结束"))?;
    let shot = session
        .monitor(meta.monitor_id)
        .ok_or_else(|| AppError::msg("显示器不存在"))?;
    if meta.rect.is_empty() {
        return Err(AppError::msg("选区为空"));
    }
    let monitor = shot.info.clone();
    let global = PhysicalRect::new(
        monitor.bounds.x + meta.rect.x,
        monitor.bounds.y + meta.rect.y,
        meta.rect.width,
        meta.rect.height,
    );

    // 录制中开的截图：不能再开长截图、再录一段（遮罩、抓屏都占着）
    if st.capture.is_nested()
        && matches!(
            meta.action,
            FinishAction::Longshot | FinishAction::Gif | FinishAction::Video | FinishAction::Record
        )
    {
        return Err(AppError::msg("正在录制，先停下录制再用这个"));
    }
    if meta.action == FinishAction::Longshot {
        return longshot::start(app, session.clone(), monitor, global);
    }
    if matches!(
        meta.action,
        FinishAction::Gif | FinishAction::Video | FinishAction::Record
    ) {
        let format = match meta.action {
            FinishAction::Gif => crate::gif_record::RecordFormat::Gif,
            FinishAction::Video => crate::gif_record::RecordFormat::Mp4,
            _ => crate::gif_record::RecordFormat::parse(&st.settings.read().record.format),
        };
        return crate::gif_record::start(app, session.clone(), monitor, global, format);
    }

    let mut image = imaging::crop_opaque(&shot.image, meta.rect)?;
    let annotated = meta.annotation_at.is_some() && !annotation_png.is_empty();
    if let Some((ax, ay)) = meta.annotation_at.filter(|_| annotated) {
        imaging::composite_png(
            &mut image,
            &annotation_png,
            i64::from(ax - meta.rect.x),
            i64::from(ay - meta.rect.y),
        )?;
    }
    if let Some(shape) = meta.shape.as_ref().filter(|s| s.len() >= 3) {
        let (ox, oy) = (f64::from(meta.rect.x), f64::from(meta.rect.y));
        let local: Vec<[f64; 2]> = shape.iter().map(|[x, y]| [x - ox, y - oy]).collect();
        imaging::mask_polygon(&mut image, &local);
    }
    let source_app = session.source_app.clone();
    if meta.action == FinishAction::Pin {
        // 贴图：遮罩先留着，等贴图窗口把图画出来再收（见 `pin::create`）。先收的话选区那块画面
        // 会消失两三百毫秒再冒出来，看着像跳了一下
        let image = Arc::new(image);
        let (end_app, session_id) = (app.clone(), session.id);
        pin::create(
            app,
            image.clone(),
            (global.x, global.y),
            monitor.scale_factor,
            Some(Box::new(move || end_session(&end_app, session_id, false))),
        )?;
        if st.settings.read().capture.save_to_library {
            let app = app.clone();
            std::thread::spawn(move || {
                if let Err(err) = library::add(&app, &image, "normal", source_app, annotated) {
                    tracing::warn!("写入截图库失败：{err}");
                }
            });
        }
        return Ok(());
    }
    let restore = matches!(meta.action, FinishAction::Copy | FinishAction::Save);
    end_session(app, session.id, restore);

    let image = Arc::new(image);
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(err) = perform(
            &app,
            meta.action,
            image,
            global,
            monitor.scale_factor,
            source_app,
            annotated,
        ) {
            tracing::error!("截图完成动作失败：{err}");
            wm::toast(&app, "error", err.to_string());
        }
    });
    Ok(())
}

pub(crate) fn perform(
    app: &AppHandle,
    action: FinishAction,
    image: Arc<RgbaImage>,
    at: PhysicalRect,
    scale: f64,
    source_app: Option<String>,
    annotated: bool,
) -> AppResult<()> {
    let st = state(app);
    let settings = st.settings.read().capture.clone();
    // 截图库写入（PNG 编码 + 缩略图）放在用户可感知的动作之后，别拖慢"复制"
    let add_to_library = || -> Option<i64> {
        if !settings.save_to_library {
            return None;
        }
        library::add(app, &image, "normal", source_app.clone(), annotated)
            .map_err(|err| tracing::warn!("写入截图库失败：{err}"))
            .ok()
    };
    match action {
        FinishAction::Copy => {
            // 自由选区有透明的部分：另外写一份 PNG，粘贴到支持透明的程序里还是那个形状
            let png = if imaging::has_alpha(&image) {
                Some(imaging::encode_png(&image)?)
            } else {
                None
            };
            platform::clipboard_write(&ClipboardPayload::Image {
                image: (*image).clone(),
                png,
            })?;
            if settings.finish_action == "copyAndSave" {
                let path = auto_save_path(&settings)?;
                write_image_file(&image, &path, &settings)?;
                wm::toast(app, "success", "已复制并保存");
            } else {
                wm::toast(app, "success", "已复制到剪贴板");
            }
            add_to_library();
        }
        FinishAction::Save => {
            add_to_library();
            if let Some(path) = ask_save_path(app, &settings)? {
                write_image_file(&image, &path, &settings)?;
                wm::toast(app, "success", "已保存");
            }
        }
        FinishAction::Pin => {
            pin::create(app, image.clone(), (at.x, at.y), scale, None)?;
            add_to_library();
        }
        FinishAction::Ocr | FinishAction::Translate => {
            let shot_id = add_to_library();
            ocr::open_job(
                app,
                (*image).clone(),
                shot_id,
                action == FinishAction::Translate,
            )?;
        }
        FinishAction::Ai => {
            let attached = ai::store_image(app, image.clone());
            ai::open_window(
                app,
                ai::AiContext {
                    images: vec![attached],
                    source: "capture".into(),
                    ..Default::default()
                },
            );
            add_to_library();
        }
        FinishAction::Longshot | FinishAction::Gif | FinishAction::Video | FinishAction::Record => {
        }
    }
    Ok(())
}

pub fn save_directory(settings: &CaptureSettings) -> std::path::PathBuf {
    settings
        .save_directory
        .as_ref()
        .filter(|d| !d.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| platform::pictures_dir().map(|p| p.join(SAVE_DIR_NAME)))
        .unwrap_or_else(|| std::env::temp_dir().join(SAVE_DIR_NAME))
}

/// 默认保存位置：图片\DATO OCR
const SAVE_DIR_NAME: &str = "DATO OCR";

/// 品牌改名后，用默认保存位置的用户：把旧的"图片\DATO COR"文件夹改名过去，截图还在原来那些文件旁边。
/// 改不了名（文件被占用之类）就算了，新截图存到新文件夹，旧的不动。启动时调一次。
pub fn migrate_save_directory(settings: &CaptureSettings) {
    if settings
        .save_directory
        .as_ref()
        .is_some_and(|d| !d.trim().is_empty())
    {
        return;
    }
    let Some(pictures) = platform::pictures_dir() else {
        return;
    };
    let (old, new) = (pictures.join("DATO COR"), pictures.join(SAVE_DIR_NAME));
    if old.is_dir() && !new.exists() {
        match std::fs::rename(&old, &new) {
            Ok(()) => {
                tracing::info!(from = %old.display(), to = %new.display(), "默认保存文件夹跟着品牌改名")
            }
            Err(err) => tracing::warn!("默认保存文件夹改名失败，旧截图留在原处：{err}"),
        }
    }
}

pub fn file_name(settings: &CaptureSettings) -> String {
    let now = chrono::Local::now();
    let name = settings
        .file_name_template
        .replace("{yyyy}", &now.format("%Y").to_string())
        .replace("{MM}", &now.format("%m").to_string())
        .replace("{dd}", &now.format("%d").to_string())
        .replace("{HH}", &now.format("%H").to_string())
        .replace("{mm}", &now.format("%M").to_string())
        .replace("{ss}", &now.format("%S").to_string());
    let clean: String = name
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let ext = if settings.image_format == "jpg" {
        "jpg"
    } else {
        "png"
    };
    format!("{clean}.{ext}")
}

fn auto_save_path(settings: &CaptureSettings) -> AppResult<std::path::PathBuf> {
    let dir = save_directory(settings);
    std::fs::create_dir_all(&dir)?;
    let mut path = dir.join(file_name(settings));
    let mut n = 1;
    while path.exists() {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = path
            .extension()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        path = dir.join(format!(
            "{}_{n}.{ext}",
            stem.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_')
        ));
        n += 1;
    }
    Ok(path)
}

pub fn ask_save_path(
    app: &AppHandle,
    settings: &CaptureSettings,
) -> AppResult<Option<std::path::PathBuf>> {
    let dir = save_directory(settings);
    let _ = std::fs::create_dir_all(&dir);
    let picked = app
        .dialog()
        .file()
        .set_title("保存截图")
        .set_directory(&dir)
        .set_file_name(file_name(settings))
        .add_filter("PNG 图片", &["png"])
        .add_filter("JPEG 图片", &["jpg", "jpeg"])
        .blocking_save_file();
    Ok(picked.and_then(|p| p.into_path().ok()))
}

pub fn write_image_file(
    image: &RgbaImage,
    path: &std::path::Path,
    settings: &CaptureSettings,
) -> AppResult<()> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let bytes = if ext == "jpg" || ext == "jpeg" {
        imaging::encode_jpeg(image, settings.jpg_quality)?
    } else {
        imaging::encode_png(image)?
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_template_and_sanitizing() {
        let mut s = CaptureSettings {
            file_name_template: "shot:{yyyy}".into(),
            ..Default::default()
        };
        let name = file_name(&s);
        assert!(name.starts_with("shot_20"));
        assert!(name.ends_with(".png"));
        s.image_format = "jpg".into();
        assert!(file_name(&s).ends_with(".jpg"));
    }
}
