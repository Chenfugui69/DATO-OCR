//! 本地公式识别（可选下载）：不经过 AI，几百毫秒把公式图认成 LaTeX。
//!
//! 模型是 RapidLaTeXOCR（LaTeX-OCR / pix2tex 的 ONNX 版，MIT 许可）：缩放模型（挑最合适的输入宽度）、
//! 编码器（看图）、解码器（一个记号一个记号写出 LaTeX），外加分词表；推理用微软的 ONNX Runtime（MIT），
//! 动态库跟模型一起由用户在设置里下载（Mac 约 210MB，Windows 约 185MB），程序本身不带。
//! 印刷体公式认得准，手写的、很长的多行公式还是交给 AI 好（操作条上的 AI 按钮）。
//!
//! 下载：
//! - 网慢的时候单个连接常常只有每秒几 KB（GitHub 按连接限速），所以按 1MB 一段、十几个连接同时下，
//!   每段失败了重试；下完核对大小和 SHA-256
//! - Windows 的 ONNX Runtime 官方包是 79MB 的 zip，里面真正要的 onnxruntime.dll 只占一小块（其余是调试符号）：
//!   先取 zip 尾部的目录，只下 dll 那一段解压，核对 CRC 和 SHA-256
//! - Mac 的是 33MB 的 tgz，整包下完核对后解出 libonnxruntime.dylib
//! - 全部放进 `engines/latex.tmp`，都成功了才换成 `engines/latex`，中途失败不留半截
//!
//! 推理照着 RapidLaTeXOCR 的 Python 实现（rapid_latex_ocr/main.py、utils.py、models.py）：
//! 1. 灰度、拉伸到 0~255，按平均亮度判断是不是深底浅字（是就反相），裁到有字的范围，补成 32 的倍数
//! 2. 缩到 672×192 以内；缩放模型给出最合适的宽度（32 的倍数），按它缩放再来，最多 10 轮
//! 3. 归一化（均值 0.7931、标准差 0.1738）送进编码器；解码器从 [BOS] 开始每步取概率最大的记号，到 [EOS] 停
//! 4. 记号拼起来、Ġ 换成空格，再去掉多余的空格（\mathrm { d } → \mathrm{d}，x = 1 → x=1）

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use image::{GrayImage, Luma, RgbaImage};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

use super::paddle::DownloadStatus;
use crate::error::{AppError, AppResult};
use crate::state::state;
use crate::{events, net};

/// 要下载的一个文件
struct Asset {
    name: &'static str,
    url: &'static str,
    size: u64,
    sha256: &'static str,
}

const MODELS: [Asset; 4] = [
    Asset {
        name: "tokenizer.json",
        url: "https://github.com/RapidAI/RapidLaTeXOCR/releases/download/v0.0.0/tokenizer.json",
        size: 24_174,
        sha256: "1dc27b18d6a518d0d5ff3f4bb7bd98521fe80ad39e5b2a246d4109f1bb9d5019",
    },
    Asset {
        name: "image_resizer.onnx",
        url: "https://github.com/RapidAI/RapidLaTeXOCR/releases/download/v0.0.0/image_resizer.onnx",
        size: 38_967_751,
        sha256: "e0b075c39700f64d50400f39c8fc186bbb3b5d84d31864008313f376603aca9d",
    },
    Asset {
        name: "encoder.onnx",
        url: "https://github.com/RapidAI/RapidLaTeXOCR/releases/download/v0.0.0/encoder.onnx",
        size: 89_008_136,
        sha256: "01bf5dc25539ca0cd5b1bd29296ea495977a6ba5f629dc4178277809d26e5e7d",
    },
    Asset {
        name: "decoder.onnx",
        url: "https://github.com/RapidAI/RapidLaTeXOCR/releases/download/v0.0.0/decoder.onnx",
        size: 50_952_726,
        sha256: "bd695497bf1b22279b7626f5916c79226e1e244c84355f8da7edfd2d921d0072",
    },
];

/// ONNX Runtime 的动态库：Mac 是整个 tgz 下来解出 dylib，Windows 是从 zip 里只取 dll 那一段
#[allow(dead_code)]
enum Runtime {
    TarGz {
        url: &'static str,
        size: u64,
        sha256: &'static str,
    },
    ZipMember {
        url: &'static str,
        /// zip 里的路径
        member: &'static str,
        /// 解出来的 dll
        size: u64,
        sha256: &'static str,
    },
}

const MAC_RUNTIME: Runtime = Runtime::TarGz {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-osx-arm64-1.28.3.tgz",
    size: 33_057_743,
    sha256: "c436bd9f47dbce6f6311ccc21de97f829b3c862c09d24aa321958cb72f0a1d32",
};

const WINDOWS_RUNTIME: Runtime = Runtime::ZipMember {
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.3/onnxruntime-win-x64-1.28.3.zip",
    member: "onnxruntime-win-x64-1.28.3/lib/onnxruntime.dll",
    size: 15_828_832,
    sha256: "4d2774a5f64e4a230b16b74b167171e92f65b0683c19680c1d04fb284b92a6e2",
};

/// 装好后动态库的文件名
#[cfg(target_os = "macos")]
const LIB: &str = "libonnxruntime.dylib";
#[cfg(not(target_os = "macos"))]
const LIB: &str = "onnxruntime.dll";

/// 这台电脑能不能用（有对应的 ONNX Runtime）：Apple 芯片的 Mac、64 位 Windows
pub fn supported() -> bool {
    runtime().is_some()
}

fn runtime() -> Option<&'static Runtime> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(&MAC_RUNTIME)
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some(&WINDOWS_RUNTIME)
    } else {
        None
    }
}

/// 一共要下载多少字节（设置页显示大小）
pub fn download_size() -> u64 {
    let rt = match runtime() {
        Some(Runtime::TarGz { size, .. }) => *size,
        // 只下 dll 那一段，压缩后大约这么大
        Some(Runtime::ZipMember { .. }) => 5_700_000,
        None => 0,
    };
    MODELS.iter().map(|a| a.size).sum::<u64>() + rt
}

fn root(paths: &crate::paths::AppPaths) -> PathBuf {
    paths.root().join("engines")
}

/// 装好的目录（模型和动态库都在）
pub fn installed_dir(paths: &crate::paths::AppPaths) -> Option<PathBuf> {
    let dir = root(paths).join("latex");
    let complete = dir.join(LIB).exists() && MODELS.iter().all(|a| dir.join(a.name).exists());
    complete.then_some(dir)
}

pub fn installed(app: &AppHandle) -> bool {
    installed_dir(&state(app).paths).is_some()
}

// ───────────────────────── 下载 ─────────────────────────

static DOWNLOAD: Mutex<Option<DownloadStatus>> = Mutex::new(None);

pub fn download_status() -> DownloadStatus {
    DOWNLOAD.lock().clone().unwrap_or_default()
}

fn set(app: &AppHandle, f: impl FnOnce(&mut DownloadStatus)) {
    let snapshot = {
        let mut s = DOWNLOAD.lock();
        let s = s.get_or_insert_with(DownloadStatus::default);
        f(s);
        s.clone()
    };
    let _ = app.emit(events::OCR_LATEX_DOWNLOAD, snapshot);
}

/// 下载并安装。已经在下载就直接返回
pub async fn install(app: &AppHandle) -> AppResult<()> {
    let Some(rt) = runtime() else {
        return Err(AppError::msg(
            "本地公式识别暂时只支持 Apple 芯片的 Mac 和 64 位 Windows",
        ));
    };
    {
        let mut s = DOWNLOAD.lock();
        if s.as_ref().is_some_and(|s| s.stage.is_some()) {
            return Ok(());
        }
        *s = Some(DownloadStatus {
            stage: Some("downloading".into()),
            progress: Some(0.0),
            error: None,
        });
    }
    set(app, |_| {});
    let result = install_inner(app, rt).await;
    match &result {
        Ok(()) => {
            set(app, |s| *s = DownloadStatus::default());
            tracing::info!("本地公式识别模型已安装");
        }
        Err(err) => {
            tracing::warn!("本地公式识别模型安装失败：{err}");
            set(app, |s| {
                *s = DownloadStatus {
                    error: Some(crate::i18n::text(err)),
                    ..Default::default()
                }
            });
        }
    }
    result
}

/// 下载进度：已经下了多少字节（几个文件、几个连接一起加），隔一会儿报一次（0~1）
struct Progress {
    done: AtomicU64,
    total: u64,
    last: Mutex<Instant>,
    report: Box<dyn Fn(f32) + Send + Sync>,
}

impl Progress {
    fn add(&self, n: u64) {
        let done = self.done.fetch_add(n, Ordering::Relaxed) + n;
        let mut last = self.last.lock();
        if last.elapsed() > Duration::from_millis(200) {
            *last = Instant::now();
            (self.report)((done as f64 / self.total.max(1) as f64).min(1.0) as f32);
        }
    }
}

async fn install_inner(app: &AppHandle, rt: &Runtime) -> AppResult<()> {
    let base = root(&state(app).paths);
    let staging = base.join("latex.tmp");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let client = net::client(app, net::Purpose::Download);
    let progress = Arc::new(Progress {
        done: AtomicU64::new(0),
        total: download_size(),
        last: Mutex::new(Instant::now()),
        report: Box::new({
            let app = app.clone();
            move |p| set(&app, |s| s.progress = Some(p))
        }),
    });
    let result = async {
        for asset in &MODELS {
            let path = staging.join(asset.name);
            fetch_ranges(&client, asset.url, asset.size, &path, &progress).await?;
            verify(&path, asset.size, asset.sha256)?;
        }
        match rt {
            Runtime::TarGz { url, size, sha256 } => {
                let archive = staging.join("runtime.tgz");
                fetch_ranges(&client, url, *size, &archive, &progress).await?;
                verify(&archive, *size, sha256)?;
                set(app, |s| {
                    s.stage = Some("extracting".into());
                    s.progress = None;
                });
                let (archive, out) = (archive.clone(), staging.join(LIB));
                tokio::task::spawn_blocking(move || {
                    let r = extract_dylib(&archive, &out);
                    let _ = std::fs::remove_file(&archive);
                    r
                })
                .await
                .map_err(|e| AppError::msg(format!("解压失败：{e}")))??;
            }
            Runtime::ZipMember {
                url,
                member,
                size,
                sha256,
            } => {
                let out = staging.join(LIB);
                fetch_zip_member(&client, url, member, &out, &progress).await?;
                verify(&out, *size, sha256)?;
            }
        }
        let target = base.join("latex");
        if target.exists() {
            unload();
            clear(&target);
        }
        if target.exists() {
            // 旧目录里只剩删不掉的动态库（Windows 上加载过就删不了）：新文件一个个挪进去，
            // 动态库是同一个版本，挪不进去就用旧的
            for entry in std::fs::read_dir(&staging)? {
                let path = entry?.path();
                let Some(name) = path.file_name() else {
                    continue;
                };
                let dest = target.join(name);
                if std::fs::rename(&path, &dest).is_err() && !(name == LIB && dest.exists()) {
                    return Err(AppError::msg(
                        "安装失败：旧的模型文件删不掉，重启程序后再试",
                    ));
                }
            }
            let _ = std::fs::remove_dir_all(&staging);
        } else {
            std::fs::rename(&staging, &target)?;
        }
        AppResult::Ok(())
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// 核对大小和 SHA-256（`size` 为 0 / `sha256` 为空的不核对那一项）
fn verify(path: &Path, size: u64, sha256: &str) -> AppResult<()> {
    let len = std::fs::metadata(path)?.len();
    if size > 0 && len != size {
        return Err(AppError::msg("下载的文件不完整，请重试"));
    }
    if sha256.is_empty() {
        return Ok(());
    }
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if digest != sha256 {
        return Err(AppError::msg("下载的文件不完整或被改动过，请重试"));
    }
    Ok(())
}

/// 每段多大、同时开几个连接
const PART: u64 = 1 << 20;
const CONNECTIONS: usize = 12;

/// 整个文件按段并行下到 `path`
async fn fetch_ranges(
    client: &reqwest::Client,
    url: &str,
    size: u64,
    path: &Path,
    progress: &Progress,
) -> AppResult<()> {
    fetch_span(client, url, 0, size, path, progress).await
}

/// 连接多久收不到数据算卡住了（慢网下有的连接会停住不动，比等读超时快得多）
const STALL: Duration = Duration::from_secs(20);

/// 下一段（`[a, b)`）。连接断了、卡住了就重连，从已经收到的地方接着下；连着好几次一点都没下到才算失败
async fn fetch_part(client: &reqwest::Client, url: &str, a: u64, b: u64) -> AppResult<Vec<u8>> {
    let mut data: Vec<u8> = Vec::with_capacity((b - a) as usize);
    let mut last = None;
    let mut fruitless = 0;
    while fruitless < 6 {
        if fruitless > 0 {
            tokio::time::sleep(Duration::from_millis(500 * fruitless)).await;
        }
        let from = a + data.len() as u64;
        let before = data.len();
        let res = client
            .get(url)
            .header(reqwest::header::RANGE, format!("bytes={from}-{}", b - 1))
            .send()
            .await;
        match res {
            Ok(res) if res.status().as_u16() == 206 => {
                let mut stream = res.bytes_stream();
                loop {
                    match tokio::time::timeout(STALL, stream.next()).await {
                        Ok(Some(Ok(chunk))) => {
                            data.extend_from_slice(&chunk);
                            if data.len() as u64 > b - a {
                                return Err(AppError::msg("下载的文件大小不对，已停止"));
                            }
                        }
                        Ok(Some(Err(e))) => {
                            last = Some(AppError::from(e));
                            break;
                        }
                        Ok(None) => break,
                        Err(_) => {
                            last = Some(AppError::msg("下载卡住了，请重试"));
                            break;
                        }
                    }
                }
            }
            // 服务器不认分段（或者出错了）：不重试，报出去
            Ok(res) => {
                return Err(AppError::Network(format!(
                    "下载失败：HTTP {}",
                    res.status().as_u16()
                )))
            }
            Err(e) => last = Some(AppError::from(e)),
        }
        if data.len() as u64 == b - a {
            return Ok(data);
        }
        fruitless = if data.len() > before {
            0
        } else {
            fruitless + 1
        };
    }
    Err(last.unwrap_or_else(|| AppError::msg("下载失败")))
}

/// 从 zip 里只下一个文件：先取尾部的目录找到它的位置，再只下它那一段、解压
async fn fetch_zip_member(
    client: &reqwest::Client,
    url: &str,
    member: &str,
    out: &Path,
    progress: &Progress,
) -> AppResult<()> {
    // zip 的总大小：先要第一个字节，响应头里带着总长（GitHub 的下载服务器不认 "最后 N 字节" 的写法）。
    // 网络抖一下就再问一次
    let mut total = None;
    let mut last = None;
    for attempt in 0..4u64 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(800 * attempt)).await;
        }
        match client
            .get(url)
            .header(reqwest::header::RANGE, "bytes=0-0")
            .send()
            .await
        {
            Ok(res) if res.status().as_u16() == 206 => {
                total = res
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.rsplit('/').next())
                    .and_then(|v| v.parse::<u64>().ok());
                if total.is_some() {
                    break;
                }
                last = Some(AppError::msg("下载失败：服务器没有给出文件大小"));
            }
            Ok(res) => {
                return Err(AppError::Network(format!(
                    "下载失败：HTTP {}",
                    res.status().as_u16()
                )))
            }
            Err(e) => last = Some(AppError::from(e)),
        }
    }
    let Some(total) = total else {
        return Err(last.unwrap_or_else(|| AppError::msg("下载失败")));
    };
    // 最后 64KB 里有目录结尾记录和目录本身
    let tail_start = total.saturating_sub(65_536);
    let tail = fetch_part(client, url, tail_start, total).await?;
    let entry = zip_entry(&tail, tail_start, member)?;
    // 本地文件头（30 字节 + 文件名 + 扩展字段）之后才是数据
    let header = fetch_part(client, url, entry.local_offset, entry.local_offset + 30).await?;
    let le16 = |b: &[u8], i: usize| u64::from(u16::from_le_bytes([b[i], b[i + 1]]));
    if header[..4] != [0x50, 0x4b, 0x03, 0x04] {
        return Err(AppError::msg("运行库压缩包格式不对"));
    }
    let start = entry.local_offset + 30 + le16(&header, 26) + le16(&header, 28);
    let packed = out.with_extension("part");
    fetch_span(client, url, start, entry.compressed, &packed, progress).await?;
    let data = std::fs::read(&packed)?;
    let _ = std::fs::remove_file(&packed);
    let bytes = match entry.method {
        0 => data,
        8 => {
            let mut v = Vec::with_capacity(entry.size as usize);
            flate2::read::DeflateDecoder::new(&data[..]).read_to_end(&mut v)?;
            v
        }
        m => {
            return Err(AppError::msg(format!(
                "运行库压缩包用了不认识的压缩方式（{m}）"
            )))
        }
    };
    if bytes.len() as u64 != entry.size || crc32fast::hash(&bytes) != entry.crc {
        return Err(AppError::msg("下载的文件不完整或被改动过，请重试"));
    }
    std::fs::write(out, bytes)?;
    Ok(())
}

/// 把 `url` 的 `[start, start + len)` 下到 `path`（从文件开头写）
async fn fetch_span(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    len: u64,
    path: &Path,
    progress: &Progress,
) -> AppResult<()> {
    let file = std::fs::File::create(path)?;
    file.set_len(len)?;
    let file = Mutex::new(file);
    let parts: Vec<(u64, u64)> = (0..len.div_ceil(PART))
        .map(|i| (i * PART, ((i + 1) * PART).min(len)))
        .collect();
    let mut results = futures_util::stream::iter(parts)
        .map(|(a, b)| {
            let file = &file;
            async move {
                let data = fetch_part(client, url, start + a, start + b).await?;
                let mut f = file.lock();
                f.seek(SeekFrom::Start(a))?;
                f.write_all(&data)?;
                drop(f);
                progress.add(data.len() as u64);
                AppResult::Ok(())
            }
        })
        .buffer_unordered(CONNECTIONS);
    while let Some(r) = results.next().await {
        r?;
    }
    Ok(())
}

/// zip 目录里一个文件的位置和大小
#[derive(Debug, PartialEq)]
struct ZipEntry {
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    local_offset: u64,
}

/// 在 zip 的尾部（从 `tail_start` 开始的一段）里找目录结尾记录，再在目录里找 `member`
fn zip_entry(tail: &[u8], tail_start: u64, member: &str) -> AppResult<ZipEntry> {
    let bad = || AppError::msg("运行库压缩包格式不对");
    let le16 = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let le32 = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(bad)?;
    let cd_size = u64::from(le32(tail, eocd + 12));
    let cd_offset = u64::from(le32(tail, eocd + 16));
    if cd_offset < tail_start || cd_offset + cd_size > tail_start + tail.len() as u64 {
        return Err(AppError::msg("运行库压缩包的目录太大，没法只下一部分"));
    }
    let cd = &tail[(cd_offset - tail_start) as usize..(cd_offset - tail_start + cd_size) as usize];
    let mut i = 0;
    while i + 46 <= cd.len() && cd[i..i + 4] == [0x50, 0x4b, 0x01, 0x02] {
        let name_len = usize::from(le16(cd, i + 28));
        let extra_len = usize::from(le16(cd, i + 30));
        let comment_len = usize::from(le16(cd, i + 32));
        let name = cd.get(i + 46..i + 46 + name_len).ok_or_else(bad)?;
        if name == member.as_bytes() {
            return Ok(ZipEntry {
                method: le16(cd, i + 10),
                crc: le32(cd, i + 16),
                compressed: u64::from(le32(cd, i + 20)),
                size: u64::from(le32(cd, i + 24)),
                local_offset: u64::from(le32(cd, i + 42)),
            });
        }
        i += 46 + name_len + extra_len + comment_len;
    }
    Err(AppError::msg("运行库压缩包里没有找到需要的文件"))
}

/// 从 ONNX Runtime 的 tgz 里解出 `lib/libonnxruntime.<版本>.dylib`（tar 格式自己读：512 字节一块）
fn extract_dylib(archive: &Path, out: &Path) -> AppResult<()> {
    let mut tar =
        flate2::read::GzDecoder::new(std::io::BufReader::new(std::fs::File::open(archive)?));
    let mut block = [0u8; 512];
    let mut long_name: Option<String> = None;
    loop {
        if tar.read_exact(&mut block).is_err() || block.iter().all(|&b| b == 0) {
            break;
        }
        let field = |a: usize, b: usize| {
            let raw = &block[a..b];
            let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).into_owned()
        };
        let size = u64::from_str_radix(field(124, 136).trim(), 8).unwrap_or(0);
        let kind = block[156];
        let mut name = match long_name.take() {
            Some(n) => n,
            None => {
                let prefix = field(345, 500);
                let base = field(0, 100);
                if prefix.is_empty() {
                    base
                } else {
                    format!("{prefix}/{base}")
                }
            }
        };
        let padded = size.div_ceil(512) * 512;
        let mut body = vec![0u8; padded as usize];
        tar.read_exact(&mut body)?;
        body.truncate(size as usize);
        match kind {
            // GNU 长文件名：内容是下一个条目的名字
            b'L' => {
                long_name = Some(
                    String::from_utf8_lossy(&body)
                        .trim_end_matches('\0')
                        .to_string(),
                )
            }
            // pax 扩展头：里面可能有 path=...
            b'x' => {
                let text = String::from_utf8_lossy(&body).into_owned();
                long_name = text
                    .lines()
                    .find_map(|l| l.split_once(" path=").map(|(_, p)| p.to_string()));
            }
            b'0' | 0 => {
                name = name.trim_start_matches("./").to_string();
                // 只要 lib/ 下面那个：调试符号目录（lib/xxx.dSYM/Contents/Resources/DWARF/）里也有个同名文件
                let mut parts = name.rsplit('/');
                let file = parts.next().unwrap_or("");
                let parent = parts.next().unwrap_or("");
                if parent == "lib" && file.starts_with("libonnxruntime") && file.ends_with(".dylib")
                {
                    std::fs::write(out, &body)?;
                    return Ok(());
                }
            }
            _ => {}
        }
    }
    Err(AppError::msg("运行库压缩包里没有找到 libonnxruntime.dylib"))
}

/// 删掉已安装的模型
pub fn remove(app: &AppHandle) -> AppResult<()> {
    unload();
    let dir = root(&state(app).paths).join("latex");
    if dir.exists() {
        clear(&dir);
    }
    if MODELS.iter().any(|a| dir.join(a.name).exists()) {
        return Err(AppError::msg("模型文件删不掉，重启程序后再试"));
    }
    Ok(())
}

/// 删掉整个目录；删不掉（Windows 上加载过的动态库在用着）就把能删的都删掉。
/// 模型文件没了就算没装，留下的动态库下次安装时接着用
fn clear(dir: &Path) {
    if std::fs::remove_dir_all(dir).is_ok() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let _ = std::fs::remove_dir(dir);
}

// ───────────────────────── 推理 ─────────────────────────

const MAX_DIMS: (u32, u32) = (672, 192);
const MIN_DIMS: (u32, u32) = (32, 32);
const BOS: i64 = 1;
const EOS: i64 = 2;
const MAX_SEQ: usize = 512;
const MEAN: f32 = 0.7931 * 255.0;
const STD: f32 = 0.1738 * 255.0;
/// 用完多久不再用就把模型从内存里卸掉（三个模型加起来两百多 MB）
const IDLE_UNLOAD: Duration = Duration::from_secs(180);

struct Model {
    resizer: Session,
    encoder: Session,
    decoder: Session,
    /// 记号 → 文字
    vocab: Vec<String>,
}

static MODEL: Mutex<Option<(Model, Instant)>> = Mutex::new(None);
/// ONNX Runtime 的动态库只加载一次（加载成功之后才记下）
static RUNTIME_READY: Mutex<bool> = Mutex::new(false);

fn ort_err(e: impl std::fmt::Display) -> AppError {
    AppError::msg(format!("本地公式识别出错：{e}"))
}

fn session(path: &Path) -> AppResult<Session> {
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(4));
    Session::builder()
        .map_err(ort_err)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort_err)?
        .with_intra_threads(threads)
        .map_err(ort_err)?
        .commit_from_file(path)
        .map_err(ort_err)
}

fn load(dir: &Path) -> AppResult<Model> {
    {
        let mut ready = RUNTIME_READY.lock();
        if !*ready {
            let _ = ort::init_from(dir.join(LIB))
                .map_err(|e| AppError::msg(format!("ONNX Runtime 加载失败：{e}")))?
                .with_name("dato-latex")
                .commit();
            *ready = true;
        }
    }
    let tokenizer: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("tokenizer.json"))?)?;
    let map = tokenizer["model"]["vocab"]
        .as_object()
        .ok_or_else(|| AppError::msg("分词表格式不对"))?;
    let mut vocab = vec![String::new(); map.len()];
    for (token, id) in map {
        if let Some(slot) = id.as_u64().and_then(|i| vocab.get_mut(i as usize)) {
            *slot = token.clone();
        }
    }
    Ok(Model {
        resizer: session(&dir.join("image_resizer.onnx"))?,
        encoder: session(&dir.join("encoder.onnx"))?,
        decoder: session(&dir.join("decoder.onnx"))?,
        vocab,
    })
}

fn unload() {
    *MODEL.lock() = None;
}

/// 闲置一段时间后卸掉模型（只起一个看门的线程）
fn watch_idle() {
    static WATCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("latex-idle".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_secs(30));
            let mut model = MODEL.lock();
            if model
                .as_ref()
                .is_some_and(|(_, used)| used.elapsed() > IDLE_UNLOAD)
            {
                *model = None;
                tracing::debug!("本地公式识别模型闲置，已卸载");
            }
        });
}

/// 把一张公式图认成 LaTeX（模型没装返回错误）。要在后台线程调
pub fn recognize(app: &AppHandle, image: &RgbaImage) -> AppResult<String> {
    let dir = installed_dir(&state(app).paths)
        .ok_or_else(|| AppError::msg("还没有下载本地公式识别模型"))?;
    let started = Instant::now();
    let mut slot = MODEL.lock();
    if slot.is_none() {
        let t = Instant::now();
        *slot = Some((load(&dir)?, Instant::now()));
        tracing::info!(
            elapsed_ms = t.elapsed().as_millis() as u64,
            "本地公式识别模型已加载"
        );
        watch_idle();
    }
    let Some((model, used)) = slot.as_mut() else {
        return Err(AppError::msg("本地公式识别模型加载失败"));
    };
    *used = Instant::now();
    let input = fit_width(&mut model.resizer, image)?;
    let tokens = decode(model, &input)?;
    let latex = post_process(&detokenize(&model.vocab, &tokens));
    tracing::info!(
        tokens = tokens.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "本地公式识别"
    );
    if latex.trim().is_empty() {
        return Err(AppError::msg("图里没有认出公式"));
    }
    Ok(latex)
}

/// 送进编码器的图：1×1×高×宽，已归一化
struct Input {
    width: u32,
    height: u32,
    data: Vec<f32>,
}

/// 第 2 步：缩放模型挑最合适的宽度，按它缩放再试，最多 10 轮
fn fit_width(resizer: &mut Session, image: &RgbaImage) -> AppResult<Input> {
    let base = minmax_size(&pad(&luma(image))?);
    let (w0, mut h) = base.dimensions();
    let mut w = w0;
    let mut r = 1.0f64;
    let mut last = None;
    for _ in 0..10 {
        h = ((f64::from(h) * r) as u32).max(1);
        let (input, padded_w) = prepare(&base, r, w, h)?;
        let tensor = Tensor::from_array((
            vec![1i64, 1, i64::from(input.height), i64::from(input.width)],
            input.data.clone(),
        ))
        .map_err(ort_err)?;
        let out = resizer.run(ort::inputs![tensor]).map_err(ort_err)?;
        let (_, scores) = out[0].try_extract_tensor::<f32>().map_err(ort_err)?;
        let best = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(i, _)| i);
        drop(out);
        last = Some(input);
        w = (best as u32 + 1) * 32;
        if w == padded_w {
            break;
        }
        r = f64::from(w) / f64::from(padded_w);
    }
    last.ok_or_else(|| AppError::msg("图里没有认出公式"))
}

/// 按 `r` 缩放到 `w × h`（放大用双线性、缩小用 Lanczos），再整理一遍、归一化。返回图和整理后的宽
fn prepare(base: &GrayImage, r: f64, w: u32, h: u32) -> AppResult<(Input, u32)> {
    let filter = if r > 1.0 {
        image::imageops::FilterType::Triangle
    } else {
        image::imageops::FilterType::Lanczos3
    };
    let resized = image::imageops::resize(base, w.max(1), h.max(1), filter);
    let padded = pad(&minmax_size(&resized))?;
    let (width, height) = padded.dimensions();
    let data = padded
        .pixels()
        .map(|p| (f32::from(p[0]) - MEAN) / STD)
        .collect();
    Ok((
        Input {
            width,
            height,
            data,
        },
        width,
    ))
}

/// 亮度（和 PIL 的 L 模式一样的算法）
fn luma(image: &RgbaImage) -> GrayImage {
    GrayImage::from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y).0;
        let l =
            (u32::from(p[0]) * 19595 + u32::from(p[1]) * 38470 + u32::from(p[2]) * 7471 + 0x8000)
                >> 16;
        Luma([l.min(255) as u8])
    })
}

/// 第 1 步：拉伸到 0~255，深底浅字的反过来（字总是深色），裁到有字的范围，宽高补到 32 的倍数（补白）
fn pad(gray: &GrayImage) -> AppResult<GrayImage> {
    let (w, h) = gray.dimensions();
    let (min, max) = gray
        .pixels()
        .fold((255u8, 0u8), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
    if max <= min {
        return Err(AppError::msg("图里没有认出公式"));
    }
    let scale = 255.0 / f64::from(max - min);
    let mut data: Vec<f64> = gray
        .pixels()
        .map(|p| f64::from(p[0] - min) * scale)
        .collect();
    let mean = data.iter().sum::<f64>() / data.len() as f64;
    let ink: Vec<bool> = if mean > 128.0 {
        data.iter().map(|&v| v < 128.0).collect()
    } else {
        let ink = data.iter().map(|&v| v > 128.0).collect();
        for v in &mut data {
            *v = 255.0 - *v;
        }
        ink
    };
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if ink[(y * w + x) as usize] {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return Err(AppError::msg("图里没有认出公式"));
    }
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut out = GrayImage::from_pixel(cw.div_ceil(32) * 32, ch.div_ceil(32) * 32, Luma([255]));
    for y in 0..ch {
        for x in 0..cw {
            let v = data[((y0 + y) * w + x0 + x) as usize];
            out.put_pixel(x, y, Luma([v.clamp(0.0, 255.0) as u8]));
        }
    }
    Ok(out)
}

/// 缩到 672×192 以内（保持比例），小于 32×32 的补白
fn minmax_size(img: &GrayImage) -> GrayImage {
    let (w, h) = img.dimensions();
    let ratio = (f64::from(w) / f64::from(MAX_DIMS.0)).max(f64::from(h) / f64::from(MAX_DIMS.1));
    let img = if ratio > 1.0 {
        let nw = ((f64::from(w) / ratio).floor() as u32).max(1);
        let nh = ((f64::from(h) / ratio).floor() as u32).max(1);
        image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle)
    } else {
        img.clone()
    };
    let (w, h) = img.dimensions();
    if w >= MIN_DIMS.0 && h >= MIN_DIMS.1 {
        return img;
    }
    let mut out = GrayImage::from_pixel(w.max(MIN_DIMS.0), h.max(MIN_DIMS.1), Luma([255]));
    image::imageops::replace(&mut out, &img, 0, 0);
    out
}

/// 第 3 步：编码器看一遍图，解码器从 [BOS] 开始每步取概率最大的记号，到 [EOS] 或 512 个为止
fn decode(model: &mut Model, input: &Input) -> AppResult<Vec<i64>> {
    let image = Tensor::from_array((
        vec![1i64, 1, i64::from(input.height), i64::from(input.width)],
        input.data.clone(),
    ))
    .map_err(ort_err)?;
    let encoded = model.encoder.run(ort::inputs![image]).map_err(ort_err)?;
    let (shape, data) = encoded[0].try_extract_tensor::<f32>().map_err(ort_err)?;
    let context = Tensor::from_array((shape.to_vec(), data.to_vec())).map_err(ort_err)?;
    drop(encoded);
    let mut tokens = vec![BOS];
    for _ in 0..MAX_SEQ {
        let window = &tokens[tokens.len().saturating_sub(MAX_SEQ)..];
        let n = window.len() as i64;
        let x = Tensor::from_array((vec![1i64, n], window.to_vec())).map_err(ort_err)?;
        let mask =
            Tensor::from_array((vec![1i64, n], vec![true; window.len()])).map_err(ort_err)?;
        let out = model
            .decoder
            .run(ort::inputs![&x, &mask, &context])
            .map_err(ort_err)?;
        let (shape, logits) = out[0].try_extract_tensor::<f32>().map_err(ort_err)?;
        let vocab = shape.last().copied().unwrap_or(0) as usize;
        if vocab == 0 || logits.len() < vocab {
            return Err(AppError::msg("本地公式识别出错：解码器输出不对"));
        }
        let last = &logits[logits.len() - vocab..];
        let next = last
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(EOS, |(i, _)| i as i64);
        if next == EOS {
            break;
        }
        tokens.push(next);
    }
    Ok(tokens[1..].to_vec())
}

/// 第 4 步：记号拼回文字（分词表没有解码规则：记号直接连起来，Ġ 是空格）
fn detokenize(vocab: &[String], tokens: &[i64]) -> String {
    let text: String = tokens
        .iter()
        .filter(|&&t| t > EOS)
        .filter_map(|&t| vocab.get(t as usize))
        .map(String::as_str)
        .collect();
    text.replace('Ġ', " ").trim().to_string()
}

/// 去掉多余的空格：`\mathrm { d }` 这类里面的空格全去掉；别处的空格只留两边都是字母的（`\alpha b`），
/// 和 `\ `（LaTeX 里明写的空格）
fn post_process(s: &str) -> String {
    // \operatorname / \mathrm / \text / \mathbf 后面的 {...} 里不要空格
    let mut s = s.to_string();
    for cmd in ["\\operatorname", "\\mathrm", "\\text", "\\mathbf"] {
        let mut out = String::new();
        let mut rest = s.as_str();
        while let Some(i) = rest.find(cmd) {
            out.push_str(&rest[..i + cmd.len()]);
            rest = &rest[i + cmd.len()..];
            let trimmed = rest.trim_start().trim_start_matches('*').trim_start();
            if let Some(body) = trimmed.strip_prefix('{') {
                if let Some(end) = body.find('}') {
                    out.push('{');
                    out.extend(body[..end].chars().filter(|c| !c.is_whitespace()));
                    out.push('}');
                    rest = &body[end + 1..];
                }
            }
        }
        out.push_str(rest);
        s = out;
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !c.is_whitespace() {
            out.push(c);
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        let prev = out.chars().last();
        let next = chars.get(j).copied();
        let keep = match (prev, next) {
            (Some(p), Some(n)) => (p.is_ascii_alphabetic() && n.is_ascii_alphabetic()) || p == '\\',
            _ => false,
        };
        if keep {
            out.push(' ');
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_are_removed_except_between_letters() {
        assert_eq!(post_process("x = \\frac { a } { b }"), "x=\\frac{a}{b}");
        assert_eq!(post_process("\\alpha b + c"), "\\alpha b+c");
        assert_eq!(post_process("\\mathrm { d } x"), "\\mathrm{d}x");
        assert_eq!(post_process("a \\ b"), "a\\ b");
    }

    #[test]
    fn tokens_join_and_g_becomes_space() {
        let vocab: Vec<String> = ["[PAD]", "[BOS]", "[EOS]", "\\alpha", "Ġb", "=", "1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(detokenize(&vocab, &[3, 4, 5, 6, 2]), "\\alpha b=1");
    }

    #[test]
    fn pad_crops_to_ink_and_rounds_up_to_32() {
        // 白底上一块 40×10 的深色：裁到这块、补成 64×32；深底浅字反相后结果一样
        let mut light = GrayImage::from_pixel(100, 60, Luma([250]));
        let mut dark = GrayImage::from_pixel(100, 60, Luma([10]));
        for y in 20..30 {
            for x in 30..70 {
                light.put_pixel(x, y, Luma([20]));
                dark.put_pixel(x, y, Luma([240]));
            }
        }
        for img in [light, dark] {
            let out = pad(&img).unwrap();
            assert_eq!(out.dimensions(), (64, 32));
            assert!(out.get_pixel(5, 5)[0] < 20, "字是深色");
            assert_eq!(out.get_pixel(50, 20)[0], 255, "补的是白色");
        }
        assert!(pad(&GrayImage::from_pixel(10, 10, Luma([200]))).is_err());
    }

    #[test]
    fn minmax_shrinks_large_and_pads_small() {
        assert_eq!(
            minmax_size(&GrayImage::new(1344, 96)).dimensions(),
            (672, 48)
        );
        assert_eq!(minmax_size(&GrayImage::new(20, 10)).dimensions(), (32, 32));
    }

    #[test]
    fn finds_a_member_in_the_zip_directory() {
        // 一个只有目录的小 zip：两个条目，第二个是要找的
        let mut cd = Vec::new();
        for (name, offset) in [("a/readme.txt", 0u32), ("a/lib/onnxruntime.dll", 1234)] {
            cd.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            cd.extend_from_slice(&[0; 6]);
            cd.extend_from_slice(&8u16.to_le_bytes());
            cd.extend_from_slice(&[0; 4]);
            cd.extend_from_slice(&0xdead_beefu32.to_le_bytes());
            cd.extend_from_slice(&500u32.to_le_bytes());
            cd.extend_from_slice(&900u32.to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0; 12]);
            cd.extend_from_slice(&offset.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let mut tail = vec![0u8; 100];
        let cd_offset = 10_000 + tail.len() as u32;
        tail.extend_from_slice(&cd);
        tail.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        tail.extend_from_slice(&[0; 8]);
        tail.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        tail.extend_from_slice(&cd_offset.to_le_bytes());
        tail.extend_from_slice(&[0; 2]);
        let e = zip_entry(&tail, 10_000, "a/lib/onnxruntime.dll").unwrap();
        assert_eq!(
            e,
            ZipEntry {
                method: 8,
                crc: 0xdead_beef,
                compressed: 500,
                size: 900,
                local_offset: 1234
            }
        );
        assert!(zip_entry(&tail, 10_000, "nope").is_err());
    }
}

#[cfg(test)]
mod smoke {
    /// 真模型：`CHENOCR_LATEX_DIR=模型目录 CHENOCR_LATEX_IMAGE=公式图 cargo test --lib -- --ignored smoke_latex --nocapture`
    #[test]
    #[ignore]
    fn smoke_extract() {
        // `CHENOCR_LATEX_TGZ=onnxruntime-osx-arm64.tgz CHENOCR_LATEX_OUT=libonnxruntime.dylib cargo test --lib -- --ignored smoke_extract`
        let archive = std::path::PathBuf::from(std::env::var("CHENOCR_LATEX_TGZ").unwrap());
        let out = std::path::PathBuf::from(std::env::var("CHENOCR_LATEX_OUT").unwrap());
        super::extract_dylib(&archive, &out).unwrap();
        println!("解出 {} 字节", std::fs::metadata(&out).unwrap().len());
    }

    /// 只从 Windows 的 ONNX Runtime zip 里下 dll 那一段（真联网）：
    /// `CHENOCR_LATEX_OUT=onnxruntime.dll cargo test --lib -- --ignored smoke_zip_member --nocapture`
    #[test]
    #[ignore]
    fn smoke_zip_member() {
        let out = std::path::PathBuf::from(std::env::var("CHENOCR_LATEX_OUT").unwrap());
        let super::Runtime::ZipMember { url, member, .. } = super::WINDOWS_RUNTIME else {
            unreachable!()
        };
        let progress = super::Progress {
            done: Default::default(),
            total: 7_000_000,
            last: parking_lot::Mutex::new(std::time::Instant::now()),
            report: Box::new(|p| println!("进度 {:.0}%", p * 100.0)),
        };
        let client = reqwest::Client::builder()
            .read_timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap();
        let t = std::time::Instant::now();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(super::fetch_zip_member(
                &client, url, member, &out, &progress,
            ))
            .unwrap();
        println!(
            "下载 {} 字节，{}ms",
            progress.done.load(std::sync::atomic::Ordering::Relaxed),
            t.elapsed().as_millis()
        );
    }

    #[test]
    #[ignore]
    fn smoke_latex() {
        let dir = std::path::PathBuf::from(std::env::var("CHENOCR_LATEX_DIR").unwrap());
        let img = image::open(std::env::var("CHENOCR_LATEX_IMAGE").unwrap())
            .unwrap()
            .to_rgba8();
        let t = std::time::Instant::now();
        let mut model = super::load(&dir).unwrap();
        println!("加载 {}ms", t.elapsed().as_millis());
        for _ in 0..2 {
            let t = std::time::Instant::now();
            let input = super::fit_width(&mut model.resizer, &img).unwrap();
            let tokens = super::decode(&mut model, &input).unwrap();
            let latex = super::post_process(&super::detokenize(&model.vocab, &tokens));
            println!(
                "{}×{} {} 个记号 {}ms：{latex}",
                input.width,
                input.height,
                tokens.len(),
                t.elapsed().as_millis()
            );
        }
    }
}
