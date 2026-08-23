//! 数据目录布局，见规格 07 §6。
//!
//! 根目录刻意用 Tauri 的 `config_dir()` 再拼 `CHENOCR`，而不是 `app_data_dir()`：
//! - Windows: `%APPDATA%\CHENOCR`
//! - macOS:   `~/Library/Application Support/CHENOCR`
//!
//! `app_data_dir()` 会带上 identifier（`com.chenocr.app`），和规格里写的路径不一致。
//! 这样绕一下也就不需要在 `platform/` 之外做平台分支。

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    pub fn resolve(app: &AppHandle) -> AppResult<Self> {
        let root = app
            .path()
            .config_dir()
            .map_err(|err| AppError::Io(format!("定位配置目录失败: {err}")))?
            .join("CHENOCR");

        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn temp(&self) -> PathBuf {
        self.root.join("temp")
    }

    pub fn ensure(&self, dir: PathBuf) -> AppResult<PathBuf> {
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// 启动时清空 `temp/`（规格 07 §7.4）。删不掉的文件跳过就好 —— 可能正被
    /// 上一个还没退干净的实例占着，不值得为此让启动失败。
    pub fn reset_temp(&self) -> AppResult<()> {
        let temp = self.temp();
        if temp.exists() {
            if let Err(err) = std::fs::remove_dir_all(&temp) {
                tracing::warn!("清空 temp 目录失败，跳过: {err}");
            }
        }
        self.ensure(temp)?;
        Ok(())
    }
}
