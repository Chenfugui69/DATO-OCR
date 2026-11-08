//! 主线程看门狗：主线程卡住时留下证据。
//!
//! 热键回调、窗口显示、托盘菜单全在主线程上跑。主线程一旦被堵住（等锁、等一个永远不回来的系统调用），
//! 表现就是"进程还在，快捷键和界面都没反应"，而日志里什么都没有 —— 卡住的线程不会再打日志。
//!
//! 后台线程每隔几秒往主线程投一个空任务，记下它什么时候被执行。超过 `STALL_AFTER` 没被执行就记一条
//! 警告；macOS 上再用系统自带的 `sample` 抓一份全部线程的调用栈存进日志目录（每次进程最多几份），
//! 下次用户说"又卡死了"时能直接看到主线程停在哪里。主线程缓过来了也记一笔卡了多久。
//!
//! 时间用 `Instant`：macOS 上它不算睡眠的时间，合盖睡一晚不会被当成卡住。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tauri::AppHandle;

/// 多久投一次
const BEAT_EVERY: Duration = Duration::from_secs(5);
/// 投出去的任务这么久还没被执行，就算主线程卡住了
const STALL_AFTER: Duration = Duration::from_secs(15);
/// 卡住这么久就抓一次调用栈（再久也只抓一次，等它缓过来再算下一次）
#[cfg(target_os = "macos")]
const SAMPLE_AFTER: Duration = Duration::from_secs(20);
/// 每次进程最多抓几份调用栈（一份几百 KB）
#[cfg(target_os = "macos")]
const MAX_SAMPLES: u32 = 3;

/// 主线程最后一次执行心跳任务的时刻（距 `origin()` 的毫秒数）
static LAST_BEAT: AtomicU64 = AtomicU64::new(0);

fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    origin().elapsed().as_millis() as u64
}

pub fn start(app: &AppHandle, logs: PathBuf) {
    LAST_BEAT.store(now_ms(), Ordering::SeqCst);
    remove_old_samples(&logs);
    let app = app.clone();
    std::thread::Builder::new()
        .name("main-watchdog".into())
        .spawn(move || run(&app, &logs))
        .ok();
}

fn run(app: &AppHandle, logs: &std::path::Path) {
    // 正在卡着：从什么时候开始（毫秒），这次抓过栈没有
    let mut stalled_since: Option<u64> = None;
    #[cfg(target_os = "macos")]
    let mut sampled = false;
    #[cfg(target_os = "macos")]
    let mut samples = 0u32;
    #[cfg(not(target_os = "macos"))]
    let _ = logs;
    loop {
        // 投一个心跳；主线程闲着时立刻就执行。事件循环已经没了（正在退出）就不用再看了
        if app
            .run_on_main_thread(|| LAST_BEAT.store(now_ms(), Ordering::SeqCst))
            .is_err()
        {
            return;
        }
        std::thread::sleep(BEAT_EVERY);
        let last = LAST_BEAT.load(Ordering::SeqCst);
        let now = now_ms();
        // 心跳之间本身隔着 BEAT_EVERY，没卡的话 last 最多落后这么多
        let behind = Duration::from_millis(now.saturating_sub(last));
        if behind < STALL_AFTER {
            if let Some(since) = stalled_since.take() {
                tracing::warn!(stalled_ms = now.saturating_sub(since), "主线程缓过来了");
            }
            #[cfg(target_os = "macos")]
            {
                sampled = false;
            }
            continue;
        }
        if stalled_since.is_none() {
            stalled_since = Some(last);
            tracing::error!(
                stalled_ms = behind.as_millis() as u64,
                "主线程卡住了：快捷键、窗口、托盘这时都不会响应"
            );
        }
        #[cfg(target_os = "macos")]
        if !sampled && behind >= SAMPLE_AFTER && samples < MAX_SAMPLES {
            sampled = true;
            samples += 1;
            sample_stacks(logs);
        }
    }
}

/// 调用栈文件和日志一样留 7 天
fn remove_old_samples(logs: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(logs) else {
        return;
    };
    let keep = Duration::from_secs(7 * 24 * 3600);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("hang-") && name.ends_with(".txt")) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > keep);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 用系统的 `sample` 抓本进程全部线程的调用栈（2 秒），存进日志目录。
#[cfg(target_os = "macos")]
fn sample_stacks(logs: &std::path::Path) {
    let file = logs.join(format!(
        "hang-{}.txt",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    let pid = std::process::id().to_string();
    let result = std::process::Command::new("/usr/bin/sample")
        .args([pid.as_str(), "2", "-mayDie", "-file"])
        .arg(&file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match result {
        Ok(status) if status.success() => {
            tracing::error!(file = %file.display(), "已抓下主线程卡住时的调用栈");
        }
        Ok(status) => tracing::warn!(?status, "抓调用栈失败"),
        Err(err) => tracing::warn!("抓调用栈失败：{err}"),
    }
}

/// 调试版：`CHENOCR_TEST_STUCK=场景` 启动后自己造一次"卡住"，看能不能留下证据、能不能自己恢复。
/// 不用按键、不用别的实例转发命令（测试实例收不到 `--action=`）：
///
/// - `stall`：主线程卡 30 秒 → 日志里应有"主线程卡住了""已抓下……调用栈""主线程缓过来了"，日志目录多一个 hang-*.txt
/// - `grab`：抓屏卡 30 秒 → 8 秒后"系统抓屏迟迟没有返回"；再按一次截图应照常"截图会话开始"
/// - `busy`：占着"截图中"却没有会话 → 16 秒后按截图应先"收回"再"截图会话开始"
/// - `orphan`：会话还在、遮罩不在屏幕上 → 6 秒后按截图应先"收回"再"截图会话开始"
#[cfg(debug_assertions)]
pub fn run_demo(app: &AppHandle) {
    use crate::capture::{self, CaptureIntent};
    let Ok(which) = std::env::var("CHENOCR_TEST_STUCK") else {
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
        let press = |app: &AppHandle| {
            tracing::info!("测试：按一次截图");
            let ui = app.clone();
            let _ = app.run_on_main_thread(move || capture::trigger(&ui, CaptureIntent::Normal));
        };
        wait(3000);
        tracing::info!(%which, "测试：开始模拟卡住");
        match which.as_str() {
            "stall" => {
                let _ = app.run_on_main_thread(|| std::thread::sleep(Duration::from_secs(30)));
            }
            "grab" => {
                capture::TEST_GRAB_HANG_MS.store(30_000, Ordering::SeqCst);
                press(&app);
                wait(10_000);
                press(&app);
                wait(1500);
                capture::cancel_current(&app);
            }
            "busy" => {
                capture::test_simulate(&app, "busy");
                press(&app);
                wait(16_000);
                press(&app);
                wait(1500);
                capture::cancel_current(&app);
            }
            "orphan" => {
                press(&app);
                wait(1500);
                capture::test_simulate(&app, "shown");
                wait(6000);
                press(&app);
                wait(1500);
                capture::cancel_current(&app);
            }
            _ => tracing::warn!(%which, "不认识的 CHENOCR_TEST_STUCK"),
        }
        tracing::info!(%which, "测试：模拟结束");
    });
}
