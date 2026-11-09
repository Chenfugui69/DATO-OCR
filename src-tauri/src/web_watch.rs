//! 网页卡死自动恢复。
//!
//! 每个窗口的界面是一个独立的网页进程。网页自己陷进死循环时（#2：原位翻译排版死循环，CPU 跑满、
//! 内存涨到 4GB），主进程一切正常、日志也静悄悄，但网页画的按钮全都点不动，用户只能强退。
//! 主线程看门狗（`watchdog.rs`）管不到这种情况。
//!
//! 这里每隔几秒给**显示着、已经报到过**的窗口发一下"在吗"（往页面里执行一句脚本，页面回一个
//! `web_pong`）。页面卡在死循环里时这句脚本根本排不上，自然回不来；连着 `HUNG_AFTER` 没回，就当它
//! 卡死：记一笔日志，结束这个网页进程（macOS）或重新加载页面（其他平台）。网页进程没了会走
//! `lib.rs` 的 `on_web_content_terminated` 重新加载，页面加载完再报到，之后接着照看。
//! 截图遮罩卡死时，正在进行的截图 / 长截图先收掉：页面重新加载后不认识旧会话，留着只会挡在屏幕上。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::{capture, lifecycle, platform};

/// 多久问一次
const PING_EVERY: Duration = Duration::from_secs(3);
/// 多久没回就算卡死。正常几毫秒就回；页面偶尔忙一两秒（解码 5K 大图、排一张大表）不能误伤
const HUNG_AFTER: Duration = Duration::from_secs(12);

static SEQ: AtomicU64 = AtomicU64::new(0);
/// 每个窗口：发出去还没回的那一下是什么时候发的
static PENDING: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::Builder::new()
        .name("web-watch".into())
        .spawn(move || run(&app))
        .ok();
}

/// 页面回话了（`web_pong`）
pub fn pong(label: &str) {
    if let Some(map) = PENDING.lock().as_mut() {
        map.remove(label);
    }
}

fn run(app: &AppHandle) {
    let mut last_tick = Instant::now();
    loop {
        std::thread::sleep(PING_EVERY);
        // 隔了远不止一个间隔才醒（系统睡眠、整机卡顿）：之前发出去的都不算数，重新开始问
        if last_tick.elapsed() > PING_EVERY * 3 {
            PENDING.lock().take();
        }
        last_tick = Instant::now();
        tick(app);
    }
}

fn tick(app: &AppHandle) {
    // 先在锁外问好哪些窗口要照看：问"显示着没有"要到主线程上走一趟，主线程上的 `web_pong`
    // 又要拿这把锁，拿着锁去问会互相等死
    let windows: Vec<(String, WebviewWindow)> = app
        .webview_windows()
        .into_iter()
        .filter(|(_, w)| watched(w))
        .collect();
    let mut hung = Vec::new();
    let mut ping = Vec::new();
    {
        let mut guard = PENDING.lock();
        let map = guard.get_or_insert_with(HashMap::new);
        // 不显示、没报到（还在加载、刚重新加载）的窗口不问：藏着的网页会被系统挂起，回不了是正常的
        map.retain(|label, _| windows.iter().any(|(l, _)| l == label));
        for (label, window) in &windows {
            match map.get(label) {
                Some(sent) if sent.elapsed() >= HUNG_AFTER => {
                    hung.push((window.clone(), sent.elapsed()));
                    map.remove(label);
                }
                Some(_) => {}
                None => {
                    map.insert(label.clone(), Instant::now());
                    ping.push(window.clone());
                }
            }
        }
    }
    for window in ping {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        if window
            .eval(format!("window.__datoPing&&window.__datoPing({seq})"))
            .is_err()
        {
            pong(window.label());
        }
    }
    for (window, waited) in hung {
        recover(app, &window, waited);
    }
}

fn watched(window: &WebviewWindow) -> bool {
    lifecycle::is_ready(window.label()) && window.is_visible().unwrap_or(false)
}

fn recover(app: &AppHandle, window: &WebviewWindow, waited: Duration) {
    let label = window.label().to_string();
    tracing::error!(
        %label,
        waited_ms = waited.as_millis() as u64,
        "窗口的网页没有响应（可能陷进了死循环），结束它重新加载"
    );
    if label.starts_with(capture::overlay::LABEL_PREFIX) {
        capture::cancel_current(app);
    }
    restart(window);
}

/// 结束窗口的网页进程重新加载（macOS），结束不了就直接重新加载。页面加载完会再报到。
/// 卡死的网页、一直没加载起来的网页（预建的遮罩页面没报到）都走这里。**别在 UI 线程调**：
/// 结束网页进程要到 UI 线程上走一趟、等它回话。
pub fn restart(window: &WebviewWindow) {
    let label = window.label().to_string();
    // 页面重新加载完会再报到；这之前不再问它
    lifecycle::forget(&label);
    if let Some(map) = PENDING.lock().as_mut() {
        map.remove(&label);
    }
    if platform::kill_web_content(window) {
        // 网页进程没了会走 `on_web_content_terminated` 重新加载
        return;
    }
    if let Err(err) = window.reload() {
        tracing::error!(%label, "重新加载页面失败：{err}");
    }
}
