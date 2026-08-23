//! 日志。规格 00 §6.3：`tracing` + 按天滚动 + 保留 7 天。
//!
//! **日志里禁止出现剪贴板内容原文、OCR 结果原文、任何 API Key。**
//! 只记长度、类型、耗时这类元信息。

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// 返回的 guard 必须活到进程结束，否则非阻塞写线程会提前退出、丢日志。
#[must_use]
pub fn init(logs_dir: &Path) -> Option<WorkerGuard> {
    let default_level = if cfg!(debug_assertions) {
        "debug"
    } else {
        "info"
    };

    let filter = EnvFilter::try_from_env("CHENOCR_LOG").unwrap_or_else(|_| {
        EnvFilter::new(format!("chenocr_lib={default_level},chenocr={default_level},warn"))
    });

    let (file_layer, guard) = match build_file_appender(logs_dir) {
        Some((appender, guard)) => (
            Some(fmt::layer().with_ansi(false).with_writer(appender)),
            Some(guard),
        ),
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        // 控制台只在开发时有人看，release 下省掉它的格式化开销。
        .with(cfg!(debug_assertions).then(|| fmt::layer().with_writer(std::io::stderr)))
        .init();

    guard
}

fn build_file_appender(
    logs_dir: &Path,
) -> Option<(tracing_appender::non_blocking::NonBlocking, WorkerGuard)> {
    if let Err(err) = std::fs::create_dir_all(logs_dir) {
        eprintln!("CHENOCR: 创建日志目录失败，本次运行只输出到控制台: {err}");
        return None;
    }

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("chenocr")
        .filename_suffix("log")
        .max_log_files(7)
        .build(logs_dir);

    match appender {
        Ok(appender) => Some(tracing_appender::non_blocking(appender)),
        Err(err) => {
            eprintln!("CHENOCR: 初始化日志文件失败，本次运行只输出到控制台: {err}");
            None
        }
    }
}
