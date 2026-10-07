//! PaddleOCR 高精度引擎（可选下载，规格 04 §2.3）。
//!
//! 用的是 PaddleOCR-json（Apache-2.0，和 RapidOCR-json 同一个作者、同一套 JSON 协议），整包约 93MB 的 7z，
//! 解开约 300MB，放在数据目录的 `engines/paddle`。要求 CPU 支持 AVX。
//!
//! 下载完先核对大小和 SHA-256 再解压；解压到临时目录，全部成功才换到正式位置，中途失败不留半截。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};
use crate::state::state;
use crate::{events, net};

pub const EXE: &str = "PaddleOCR-json.exe";
const VERSION: &str = "v1.4.1";
const URL: &str = "https://github.com/hiroi-sora/PaddleOCR-json/releases/download/v1.4.1/PaddleOCR-json_v1.4.1_windows_x64.7z";
const SIZE: u64 = 92_736_768;
const SHA256: &str = "c0912a70acb1f8f18fafe1f438a2935292a6ec7e2859156fa48a33e91358d71d";

/// 下载状态（设置页显示进度）。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStatus {
    /// downloading | extracting | None（没在下载）
    pub stage: Option<String>,
    /// 0–1
    pub progress: Option<f32>,
    pub error: Option<String>,
}

fn engines_root(paths: &crate::paths::AppPaths) -> PathBuf {
    paths.root().join("engines")
}

/// 已经装好的引擎目录。
pub fn engine_dir(paths: &crate::paths::AppPaths) -> Option<PathBuf> {
    let dir = engines_root(paths).join("paddle");
    dir.join(EXE).exists().then_some(dir)
}

fn set(app: &AppHandle, f: impl FnOnce(&mut DownloadStatus)) {
    let snapshot = {
        let st = state(app);
        let mut s = st.ocr.paddle_download.lock();
        f(&mut s);
        s.clone()
    };
    let _ = app.emit(events::OCR_ENGINE_DOWNLOAD, snapshot);
}

/// 下载并安装。重复调用（已经在下载）直接返回。
pub async fn install(app: &AppHandle) -> AppResult<()> {
    if !crate::platform::paddle_ocr_supported() {
        return Err(AppError::msg("PaddleOCR 引擎目前只有 Windows 版"));
    }
    if !super::supports_avx() {
        return Err(AppError::msg(
            "这台电脑的处理器不支持 AVX 指令集，用不了 PaddleOCR",
        ));
    }
    {
        let st = state(app);
        let mut s = st.ocr.paddle_download.lock();
        if s.stage.is_some() {
            return Ok(());
        }
        *s = DownloadStatus {
            stage: Some("downloading".into()),
            progress: Some(0.0),
            error: None,
        };
    }
    set(app, |_| {});
    let result = install_inner(app).await;
    match &result {
        Ok(()) => {
            set(app, |s| *s = DownloadStatus::default());
            tracing::info!(version = VERSION, "PaddleOCR 引擎已安装");
        }
        Err(err) => {
            tracing::warn!("PaddleOCR 引擎安装失败：{err}");
            set(app, |s| {
                *s = DownloadStatus {
                    error: Some(err.to_string()),
                    ..Default::default()
                }
            });
        }
    }
    result
}

async fn install_inner(app: &AppHandle) -> AppResult<()> {
    let root = engines_root(&state(app).paths);
    std::fs::create_dir_all(&root)?;
    let archive = root.join("paddle.7z.part");

    let client = net::client(app, net::Purpose::Download);
    let res = client.get(URL).send().await?;
    if !res.status().is_success() {
        return Err(AppError::Network(format!(
            "下载失败：HTTP {}",
            res.status().as_u16()
        )));
    }
    let mut data: Vec<u8> = Vec::with_capacity(SIZE as usize);
    let mut hasher = Sha256::new();
    let mut stream = res.bytes_stream();
    let mut last_emit = Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        data.extend_from_slice(&chunk);
        if data.len() as u64 > SIZE {
            return Err(AppError::msg("下载的文件大小不对，已停止"));
        }
        if last_emit.elapsed() > Duration::from_millis(200) {
            last_emit = Instant::now();
            let p = data.len() as f32 / SIZE as f32;
            set(app, |s| s.progress = Some(p.min(1.0)));
        }
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if data.len() as u64 != SIZE || digest != SHA256 {
        return Err(AppError::msg("下载的文件不完整或被改动过，请重试"));
    }
    std::fs::write(&archive, &data)?;
    drop(data);

    set(app, |s| {
        s.stage = Some("extracting".into());
        s.progress = None;
    });
    let staging = root.join("paddle.tmp");
    let target = root.join("paddle");
    let result = tokio::task::spawn_blocking({
        let (archive, staging, target) = (archive.clone(), staging.clone(), target.clone());
        move || extract(&archive, &staging, &target)
    })
    .await
    .map_err(|e| AppError::msg(format!("解压失败：{e}")))?;
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// 解压到临时目录，包里只有一个顶层文件夹，把它整个挪成正式目录。
fn extract(archive: &Path, staging: &Path, target: &Path) -> AppResult<()> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging)?;
    sevenz_rust2::decompress_file(archive, staging)
        .map_err(|e| AppError::msg(format!("解压失败：{e}")))?;
    let inner = std::fs::read_dir(staging)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.join(EXE).exists())
        .or_else(|| staging.join(EXE).exists().then(|| staging.to_path_buf()))
        .ok_or_else(|| AppError::msg("引擎包里没有找到 PaddleOCR-json.exe"))?;
    if target.exists() {
        std::fs::remove_dir_all(target)?;
    }
    std::fs::rename(&inner, target)?;
    Ok(())
}

/// 删掉已安装的引擎（先停掉正在跑的进程）。
pub fn remove(app: &AppHandle) -> AppResult<()> {
    let st = state(app);
    if let Some(mut engine) = st.ocr.paddle.lock().take() {
        engine.kill();
    }
    let dir = engines_root(&st.paths).join("paddle");
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用本地已经下好的引擎包测解压 + 识别：
    /// `CHENOCR_TEST_PADDLE_7Z=包路径 CHENOCR_TEST_OCR_IMAGE=图片 cargo test paddle_extract_and_run -- --ignored`
    #[test]
    #[ignore]
    fn paddle_extract_and_run() {
        let archive = PathBuf::from(std::env::var("CHENOCR_TEST_PADDLE_7Z").unwrap());
        let image = std::env::var("CHENOCR_TEST_OCR_IMAGE").unwrap();
        let root = std::env::temp_dir().join(format!("chenocr-paddle-{}", std::process::id()));
        let (staging, target) = (root.join("paddle.tmp"), root.join("paddle"));
        let started = Instant::now();
        extract(&archive, &staging, &target).unwrap();
        println!("解压 {:?}", started.elapsed());
        assert!(target.join(EXE).exists());
        let img = image::open(&image).unwrap().to_rgba8();
        let mut engine = super::super::rapid::RapidEngine::spawn_paddle(&target).unwrap();
        let blocks = engine.recognize(&img, &root.join("t.bmp")).unwrap();
        engine.kill();
        let text: Vec<_> = blocks.iter().map(|b| b.text.as_str()).collect();
        println!("{text:?}");
        assert!(!blocks.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
