//! RapidOCR-json 子进程（规格 04 §2）。
//!
//! 协议：启动后打印若干行，最后一行 `OCR init completed.`；之后每行输入一个 JSON
//! 任务，每行输出一个 JSON 结果。`code` 100 成功、101 无文字（**不是错误**）、其他失败。
//!
//! 读 stdout 放在独立线程转发到 channel，这样单次识别可以加超时（卡死就杀进程重启）。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use base64::Engine as _;
use image::RgbaImage;
use serde::Deserialize;

use super::reflow::OcrBlock;
use crate::error::{AppError, AppResult};
use crate::{imaging, platform};

const INIT_TIMEOUT: Duration = Duration::from_secs(15);
pub const RECOGNIZE_TIMEOUT: Duration = Duration::from_secs(20);

pub struct RapidEngine {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

#[derive(Deserialize)]
struct RawResult {
    code: i32,
    data: serde_json::Value,
}

impl RapidEngine {
    pub fn spawn(dir: &Path) -> AppResult<Self> {
        let exe = dir.join("RapidOCR-json.exe");
        // 模型目录用相对路径（工作目录设为引擎目录）：绝对路径里有中文或长路径前缀时引擎读不到
        let mut child = platform::hidden_command(&exe)
            .current_dir(dir)
            .arg("--models=models")
            .arg("--det=ch_PP-OCRv4_det_infer.onnx")
            .arg("--cls=ch_ppocr_mobile_v2.0_cls_infer.onnx")
            .arg("--rec=rec_ch_PP-OCRv4_infer.onnx")
            .arg("--keys=dict_chinese.txt")
            .arg("--ensureAscii=1")
            .arg("--doAngle=1")
            .arg("--mostAngle=1")
            // 默认 1024 会把 4K 截图里的小字缩没；2048 是速度和精度的折中
            .arg("--maxSideLen=2048")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AppError::msg(format!("启动识字引擎失败：{e}")))?;
        platform::tie_to_current_process(&child);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::msg("识字引擎没有 stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::msg("识字引擎没有 stdout"))?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("rapidocr-stdout".into())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })?;
        let mut engine = Self {
            child,
            stdin,
            lines: rx,
        };
        let deadline = std::time::Instant::now() + INIT_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match engine.lines.recv_timeout(left) {
                Ok(line) if line.contains("init completed") => break,
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => {
                    engine.kill();
                    return Err(AppError::msg("识字引擎初始化超时"));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    engine.kill();
                    return Err(AppError::msg(
                        "识字引擎启动后立即退出（模型文件缺失或损坏）",
                    ));
                }
            }
        }
        Ok(engine)
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn recognize(&mut self, image: &RgbaImage, temp: &Path) -> AppResult<Vec<OcrBlock>> {
        let request = request_json(image, temp)?;
        self.stdin.write_all(request.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        let line = match self.lines.recv_timeout(RECOGNIZE_TIMEOUT) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => return Err(AppError::msg("识字超时")),
            Err(RecvTimeoutError::Disconnected) => return Err(AppError::msg("识字引擎已退出")),
        };
        let _ = std::fs::remove_file(temp);
        let raw: RawResult = serde_json::from_str(&line)?;
        match raw.code {
            100 => Ok(serde_json::from_value(raw.data)?),
            101 => Ok(Vec::new()),
            _ => Err(AppError::msg(format!(
                "识字引擎报错：{}",
                raw.data.as_str().unwrap_or("未知错误")
            ))),
        }
    }
}

impl Drop for RapidEngine {
    fn drop(&mut self) {
        self.kill();
    }
}

/// 路径全是 ASCII 时写 BMP 临时文件传路径（最快）；否则传 base64 PNG ——
/// 引擎内部用 OpenCV 读文件，Windows 上非 ASCII 路径会读不到。
fn request_json(image: &RgbaImage, temp: &Path) -> AppResult<String> {
    let path_str = temp.to_string_lossy();
    if path_str.is_ascii() {
        std::fs::write(temp, imaging::encode_bmp24(image))?;
        return Ok(serde_json::json!({ "image_path": path_str }).to_string());
    }
    let png = imaging::encode_png(image)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    Ok(serde_json::json!({ "image_base64": b64 }).to_string())
}

/// 引擎目录：打包资源里的 `ocr/`。开发时也会被 tauri-build 拷到 target 目录。
pub fn engine_dir(paths: &crate::paths::AppPaths) -> Option<PathBuf> {
    paths
        .resource("ocr")
        .filter(|d| d.join("RapidOCR-json.exe").exists())
}
