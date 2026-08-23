//! 统一错误类型。
//!
//! 规格 00 §6.2：command 一律返回 `Result<T, AppError>`，前端拿到结构化的
//! `{ code, message, detail }`。`code` 是稳定的机器可读标识，前端据此决定文案；
//! `message` 已经是可直接展示的中文；`detail` 只用于日志和「复制详情」。

use serde::{ser::SerializeStruct, Serialize, Serializer};

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("抓屏失败")]
    Capture(String),

    #[error("找不到显示器 {0}")]
    MonitorNotFound(u64),

    #[error("当前没有进行中的截图会话")]
    NoCaptureSession,

    #[error("选区无效")]
    InvalidSelection(String),

    #[error("窗口操作失败")]
    Window(String),

    #[error("剪贴板操作失败")]
    Clipboard(String),

    #[error("图像处理失败")]
    Image(String),

    #[error("文件读写失败")]
    Io(String),

    #[error("内部错误")]
    Internal(String),
}

impl AppError {
    /// 稳定的机器可读错误码，前端用它做分支判断。改动等于破坏 API。
    pub fn code(&self) -> &'static str {
        match self {
            Self::Capture(_) => "capture_failed",
            Self::MonitorNotFound(_) => "monitor_not_found",
            Self::NoCaptureSession => "no_capture_session",
            Self::InvalidSelection(_) => "invalid_selection",
            Self::Window(_) => "window_failed",
            Self::Clipboard(_) => "clipboard_failed",
            Self::Image(_) => "image_failed",
            Self::Io(_) => "io_failed",
            Self::Internal(_) => "internal",
        }
    }

    fn detail(&self) -> Option<&str> {
        match self {
            Self::Capture(d)
            | Self::InvalidSelection(d)
            | Self::Window(d)
            | Self::Clipboard(d)
            | Self::Image(d)
            | Self::Io(d)
            | Self::Internal(d) => Some(d.as_str()),
            Self::MonitorNotFound(_) | Self::NoCaptureSession => None,
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("AppError", 3)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.to_string())?;
        state.serialize_field("detail", &self.detail())?;
        state.end()
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<xcap::XCapError> for AppError {
    fn from(err: xcap::XCapError) -> Self {
        Self::Capture(err.to_string())
    }
}

impl From<image::ImageError> for AppError {
    fn from(err: image::ImageError) -> Self {
        Self::Image(err.to_string())
    }
}

impl From<tauri::Error> for AppError {
    fn from(err: tauri::Error) -> Self {
        Self::Window(err.to_string())
    }
}
