//! 全局错误类型。
//!
//! 所有 command 返回 `AppResult<T>`。前端拿到的是结构化的 `{ code, message }`，
//! 由 `src/lib/ipc.ts` 统一转成 toast。

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Msg(String),
    #[error("文件读写失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("数据库错误：{0}")]
    Db(#[from] rusqlite::Error),
    #[error("窗口系统错误：{0}")]
    Tauri(#[from] tauri::Error),
    #[error("图像处理失败：{0}")]
    Image(#[from] image::ImageError),
    #[error("数据格式错误：{0}")]
    Json(#[from] serde_json::Error),
    #[error("抓屏失败：{0}")]
    Capture(String),
    #[error("网络请求失败：{0}")]
    Network(String),
    #[error("找不到：{0}")]
    NotFound(String),
}

impl AppError {
    pub fn msg(text: impl Into<String>) -> Self {
        Self::Msg(text.into())
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Msg(_) => "error",
            Self::Io(_) => "io",
            Self::Db(_) => "db",
            Self::Tauri(_) => "tauri",
            Self::Image(_) => "image",
            Self::Json(_) => "json",
            Self::Capture(_) => "capture",
            Self::Network(_) => "network",
            Self::NotFound(_) => "not_found",
        }
    }
}

#[cfg(windows)]
impl From<xcap::XCapError> for AppError {
    fn from(err: xcap::XCapError) -> Self {
        Self::Capture(err.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Network("请求超时".into())
        } else if err.is_connect() {
            Self::Network("无法连接服务器".into())
        } else {
            Self::Network(err.to_string())
        }
    }
}

#[cfg(windows)]
impl From<windows::core::Error> for AppError {
    fn from(err: windows::core::Error) -> Self {
        Self::Msg(format!("系统调用失败：{}", err.message()))
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("code", self.code())?;
        // 英文界面下换成英文（见 i18n.rs）
        s.serialize_field("message", &crate::i18n::text(self))?;
        s.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;
