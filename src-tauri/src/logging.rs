//! 日志：`%APPDATA%/DATO COR/logs/`，按天滚动，保留 7 天（规格 00 §6.3）。
//!
//! 级别默认 info；`CHENOCR_LOG` 环境变量可覆盖（EnvFilter 语法）。
//! **禁止**往日志里写剪贴板原文、OCR 结果原文、任何密钥 —— 只记长度和类型。

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

pub fn init(dir: &Path, debug: bool) -> Option<WorkerGuard> {
    let default_level = if debug || cfg!(debug_assertions) {
        "chenocr_lib=debug,chenocr=debug,warn"
    } else {
        "chenocr_lib=info,chenocr=info,warn"
    };
    let filter =
        EnvFilter::try_from_env("CHENOCR_LOG").unwrap_or_else(|_| EnvFilter::new(default_level));

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("chenocr")
        .filename_suffix("log")
        .max_log_files(7)
        .build(dir);

    let (writer, guard) = match appender {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            (Some(writer), Some(guard))
        }
        Err(err) => {
            eprintln!("日志文件初始化失败：{err}");
            (None, None)
        }
    };

    let file_layer = writer.map(|w| {
        fmt::layer()
            .with_ansi(false)
            .with_target(false)
            .with_writer(w)
    });
    let console_layer = cfg!(debug_assertions).then(|| fmt::layer().with_target(false));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .try_init();

    guard
}
