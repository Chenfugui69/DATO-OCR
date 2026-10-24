//! 窗口按需创建、闲置释放。
//!
//! 每个窗口背后是一个独立的 WebView 进程，光是空着就要几十到一百多 MB（macOS 实测：主窗口藏着 105MB、
//! 截图遮罩 107MB、剪贴板面板 60MB、划词面板 26MB、提示条和划词按钮各 19MB）。以前启动时全部预建、
//! 一直挂着，空闲时整个应用 600 多 MB。现在：
//!
//! - 只有截图遮罩常驻（热键到出现要在 100ms 内，等不起建窗口）；
//! - 其余窗口用到时才建（`when_ready`：建好、等页面报到，再显示），藏起来超过一段时间就销毁（`start` 里的巡检）。
//!
//! 页面加载完会调 `window_ready` 报到（`src/main.tsx`），这之前发给它的事件它收不到，所以显示之前要等。

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::{AppHandle, Manager, WebviewWindow};

/// 藏起来多久就销毁。没列出来的窗口不管：截图遮罩、剪贴板面板、划词小圆点常驻（按下快捷键、
/// 选中文字就要立刻出来）；贴图、编辑窗口、AI 窗口这些各管各的。
const RELEASE_AFTER: &[(&str, Duration)] = &[
    (crate::wm::MAIN, Duration::from_secs(120)),
    (crate::wm::TOAST, Duration::from_secs(30)),
    (
        crate::translate::selection::WINDOW,
        Duration::from_secs(300),
    ),
    (crate::ocr::WINDOW, Duration::from_secs(120)),
];

/// 巡检间隔
const SWEEP_EVERY: Duration = Duration::from_secs(10);
/// 等页面报到最多等多久（正常两三百毫秒）
const READY_TIMEOUT: Duration = Duration::from_secs(4);

static READY: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static WAITERS: Mutex<Option<HashMap<String, Vec<SyncSender<()>>>>> = Mutex::new(None);
/// 窗口从什么时候开始一直藏着
static HIDDEN_SINCE: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// 页面加载完了（前端调 `window_ready`）。
pub fn mark_ready(label: &str) {
    READY
        .lock()
        .get_or_insert_with(HashSet::new)
        .insert(label.to_string());
    let waiters = WAITERS
        .lock()
        .get_or_insert_with(HashMap::new)
        .remove(label)
        .unwrap_or_default();
    for tx in waiters {
        let _ = tx.try_send(());
    }
}

/// 窗口没了（销毁事件里调）。下次再建要重新等页面报到。
pub fn forget(label: &str) {
    if let Some(ready) = READY.lock().as_mut() {
        ready.remove(label);
    }
    if let Some(hidden) = HIDDEN_SINCE.lock().as_mut() {
        hidden.remove(label);
    }
}

fn is_ready(label: &str) -> bool {
    READY.lock().as_ref().is_some_and(|r| r.contains(label))
}

/// 页面报到时收到通知。已经报到过的话马上就有。
fn wait(label: &str) -> Receiver<()> {
    let (tx, rx) = sync_channel(1);
    // 先拿着等待表的锁再看报没报到，免得"刚看完没报到、它就报到了"这一下漏掉
    let mut waiters = WAITERS.lock();
    if is_ready(label) {
        let _ = tx.try_send(());
    } else {
        waiters
            .get_or_insert_with(HashMap::new)
            .entry(label.to_string())
            .or_default()
            .push(tx);
    }
    rx
}

/// 用一个按需创建的窗口：在、而且页面加载好了，就直接 `then`；不在就先 `create`，等页面报到
/// （最多几秒）再 `then`。**只能在 UI 线程调用**，`then` 也在 UI 线程上跑。
pub fn when_ready<F>(app: &AppHandle, label: &str, create: fn(&AppHandle), then: F)
where
    F: FnOnce(&AppHandle, WebviewWindow) + Send + 'static,
{
    if let Some(hidden) = HIDDEN_SINCE.lock().as_mut() {
        hidden.remove(label);
    }
    match app.get_webview_window(label) {
        Some(window) if is_ready(label) => {
            then(app, window);
            return;
        }
        Some(_) => {}
        None => create(app),
    }
    let started = Instant::now();
    let rx = wait(label);
    let (app, label) = (app.clone(), label.to_string());
    std::thread::spawn(move || {
        if rx.recv_timeout(READY_TIMEOUT).is_err() {
            tracing::warn!(label, "窗口页面迟迟没报到，照样显示");
        } else {
            tracing::debug!(
                label,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "窗口按需建好"
            );
        }
        let ui = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(window) = ui.get_webview_window(&label) {
                then(&ui, window);
            }
        });
    });
}

/// 后台巡检：藏着超过时限的窗口销毁掉，把它的 WebView 进程连同内存一起还给系统。
pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::Builder::new()
        .name("window-release".into())
        .spawn(move || loop {
            std::thread::sleep(SWEEP_EVERY);
            let ui = app.clone();
            let _ = app.run_on_main_thread(move || sweep(&ui));
        })
        .ok();
}

fn sweep(app: &AppHandle) {
    let now = Instant::now();
    let mut hidden = HIDDEN_SINCE.lock();
    let hidden = hidden.get_or_insert_with(HashMap::new);
    for &(label, after) in RELEASE_AFTER {
        let Some(window) = app.get_webview_window(label) else {
            hidden.remove(label);
            continue;
        };
        // 收进程序坞的窗口不算藏着：那是用户放着待会儿要用的。销毁的话程序坞里会留下一个没有页面的空窗口
        if window.is_visible().unwrap_or(true) || window.is_minimized().unwrap_or(false) {
            hidden.remove(label);
            continue;
        }
        let since = *hidden.entry(label.to_string()).or_insert(now);
        if now.duration_since(since) >= after {
            hidden.remove(label);
            tracing::debug!(label, "窗口闲置，释放");
            let _ = window.destroy();
        }
    }
}
