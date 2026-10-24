//! 录制准备（截图框变成录制框，还没点开始）：不压暗、露出真实桌面，遮罩除了录制的控制条、录制栏、
//! 把手、选区边框这几块以外全部鼠标穿透 —— 底下的程序照常点、照常用，点「打开文件夹」弹出来的
//! 资源管理器也看得见、点得到。
//!
//! 能点的区域由页面报上来（和录制中同一套，见 `gif_set_regions`），这里每 16ms 看一眼鼠标在不在里面，
//! 切遮罩的穿透。鼠标穿过去了页面就收不到鼠标、按键，鼠标效果和按键显示的预览改由这里转给页面：
//! 光标位置、按着的键、滚轮（`record-prep-pointer`），按键（`record-key`，和录制中一样）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use super::overlay;
use crate::error::{AppError, AppResult};
use crate::events;
use crate::platform::{self, MonitorId, MonitorInfo, PhysicalRect, RecordInput};
use crate::state::state;

struct Prep {
    monitor: MonitorInfo,
    interactive: bool,
    stop: Arc<AtomicBool>,
    /// 全局键盘 / 滚轮监听。进程里只能有一份：开始录制前必须当场卸掉（见 `leave`），
    /// 不然录制装上的那份会被它晚一步清掉
    _input: Option<platform::RecordInputGuard>,
}

static PREP: Mutex<Option<Prep>> = Mutex::new(None);
/// 页面报上来能点的区域（本屏局部物理坐标）。单独放：页面进准备阶段时先画出控制条、报区域，
/// 和通知进准备阶段是两个并发的调用，区域可能先到
static REGIONS: Mutex<Vec<PhysicalRect>> = Mutex::new(Vec::new());

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrepEvent {
    active: bool,
    monitor_id: u64,
}

/// 准备时转给页面的鼠标：位置（本屏局部物理坐标，不在这块屏上是 None）、按着的键、这一拍滚了多少
#[derive(Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Pointer {
    at: Option<(i32, i32)>,
    buttons: u8,
    scroll: f64,
}

/// 进入录制准备。`monitor`：录制框所在的屏
pub fn enter(app: &AppHandle, session_id: u64, monitor: MonitorId) -> AppResult<()> {
    let session = state(app)
        .capture
        .current()
        .filter(|s| s.id == session_id)
        .ok_or_else(|| AppError::msg("截图会话已结束"))?;
    let info = session
        .monitor(monitor)
        .ok_or_else(|| AppError::msg("显示器不存在"))?
        .info
        .clone();
    // 上一次的先停掉、监听先卸掉，再装这次的（监听进程里只有一份）
    if let Some(old) = PREP.lock().take() {
        old.stop.store(true, Ordering::SeqCst);
    }
    let (tx, rx) = channel::<RecordInput>();
    let input = platform::start_record_input(tx)
        .inspect_err(|err| tracing::warn!("录制准备：键盘监听没装上，按键预览看不到：{err}"))
        .ok();
    let stop = Arc::new(AtomicBool::new(false));
    *PREP.lock() = Some(Prep {
        monitor: info.clone(),
        interactive: false,
        stop: stop.clone(),
        _input: input,
    });

    let ui = app.clone();
    app.run_on_main_thread(move || {
        // 冻结的画面撤掉露出真实桌面；遮罩全部穿透，鼠标移到能点的地方再打开（`poll`）
        platform::backdrop::hide_all();
        overlay::for_each(&ui, |w| {
            let _ = w.set_ignore_cursor_events(true);
        });
        let _ = ui.emit(
            events::RECORD_PREP,
            PrepEvent {
                active: true,
                monitor_id: monitor.0,
            },
        );
    })?;

    let poll_app = app.clone();
    std::thread::Builder::new()
        .name("record-prep".into())
        .spawn(move || poll(&poll_app, &stop, &rx, &info))?;
    tracing::info!(session = session_id, %monitor, "录制准备：遮罩改成穿透");
    Ok(())
}

/// 页面报上来能点的区域（本屏局部物理坐标）
pub fn set_regions(regions: Vec<PhysicalRect>) {
    *REGIONS.lock() = regions;
}

/// 离开录制准备。`restore`：回到截图编辑（冻结的画面、压暗、遮罩收鼠标都还原）；
/// 开始录制、截图结束时不用还原
pub fn leave(app: &AppHandle, restore: bool) {
    let Some(prep) = PREP.lock().take() else {
        return;
    };
    prep.stop.store(true, Ordering::SeqCst);
    REGIONS.lock().clear();
    let monitor = prep.monitor.id;
    // 这里就卸掉键盘监听（drop），见 `Prep::_input`
    drop(prep);
    if !restore {
        return;
    }
    let Some(session) = state(app).capture.current() else {
        return;
    };
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        for m in &session.monitors {
            let Some(w) = tauri::Manager::get_webview_window(&ui, &overlay::label_for(m.info.id))
            else {
                continue;
            };
            let _ = w.set_ignore_cursor_events(false);
            match platform::native_handle(&w) {
                Ok(handle) => {
                    if let Err(err) = platform::backdrop::show_below(m.info.id, handle) {
                        tracing::warn!(monitor = %m.info.id, "录制准备退回截图：底图没显示出来：{err}");
                    }
                }
                Err(err) => tracing::warn!("录制准备退回截图：拿不到遮罩窗口：{err}"),
            }
        }
        let _ = ui.emit(
            events::RECORD_PREP,
            PrepEvent {
                active: false,
                monitor_id: monitor.0,
            },
        );
        overlay::refocus(&ui, monitor);
    });
}

fn poll(app: &AppHandle, stop: &AtomicBool, rx: &Receiver<RecordInput>, info: &MonitorInfo) {
    let label = overlay::label_for(info.id);
    let b = info.bounds;
    let mut last: Option<Pointer> = None;
    while !stop.load(Ordering::SeqCst) {
        let cursor = platform::cursor_position();
        let local = cursor.map(|(x, y)| (x - b.x, y - b.y));
        let inside_monitor = local
            .is_some_and(|(x, y)| x >= 0 && y >= 0 && x < b.width as i32 && y < b.height as i32);
        // 能不能点
        {
            let mut guard = PREP.lock();
            let Some(p) = guard.as_mut() else { break };
            let inside = inside_monitor
                && local.is_some_and(|(x, y)| {
                    REGIONS.lock().iter().any(|r| r.contains_point(x, y))
                });
            if inside != p.interactive {
                p.interactive = inside;
                if let Some(w) = tauri::Manager::get_webview_window(app, &label) {
                    let _ = w.set_ignore_cursor_events(!inside);
                }
            }
        }
        // 预览用的鼠标、按键
        let mut scroll = 0.0;
        for input in rx.try_iter() {
            match input {
                RecordInput::Key(key) => {
                    let _ = app.emit_to(label.as_str(), events::RECORD_KEY, key);
                }
                RecordInput::Scroll(dy) => scroll += dy,
            }
        }
        let pointer = Pointer {
            at: local.filter(|_| inside_monitor),
            buttons: platform::mouse_buttons(),
            scroll,
        };
        if last.as_ref() != Some(&pointer) {
            let _ = app.emit_to(label.as_str(), events::RECORD_PREP_POINTER, pointer.clone());
            last = Some(Pointer {
                scroll: 0.0,
                ..pointer
            });
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}
