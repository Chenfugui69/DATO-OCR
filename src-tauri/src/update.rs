//! 检查更新、下载、校验、安装。
//!
//! - 两个渠道：国内（Gitee）、国外（GitHub）。按选的渠道先试，连不上再试另一个
//! - 更新清单是 Tauri updater 兼容的 JSON，多带一个 `changes`（新增 / 修复 / 改进），给设置页的更新简介用
//! - 安装包下载完先用 minisign 公钥验签（私钥只在发版的电脑上，见 `scripts/release.mjs`），
//!   签名里的可信注释带着文件名、文件名里有版本号，对不上就拒绝（防止有人把旧版的安装包配上新版本号骗你"降级"）
//! - 自动检查：启动 30 秒后一次，之后每 6 小时一次，可在设置里关掉；手动检查随时可以
//!
//! 联网走 `net::client`，跟着设置里的代理走（国内用户常开代理，GitHub 也常要代理）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};
use crate::state::state;
use crate::storage::now_ms;
use crate::{events, net, wm};

/// 发版签名的公钥（`tauri signer generate` 生成，minisign 格式再 base64）。
/// 换钥匙的话，已经装着旧版的用户就验不过新版了，只能手动下载一次——别随便换。
const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEI5QTk0MjAzODkyN0FCM0UKUldRK3F5ZUpBMEtwdVRhYVJjcU01ZnBjQng5ZnlrUFM0dHF6clhoM2xjN1NjVG4zSldQWVdwYUUK";

/// 国内渠道：Gitee 仓库里的清单，安装包在 Gitee 的发行版里
const CN_MANIFEST: &str = "https://gitee.com/Chenfugui69/DATO-OCR/raw/main/update/latest-cn.json";
/// 国外渠道：GitHub 仓库里的清单，安装包在 GitHub Releases 里
const GLOBAL_MANIFEST: &str =
    "https://raw.githubusercontent.com/Chenfugui69/DATO-OCR/main/update/latest.json";

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);
const MAX_INSTALLER: u64 = 300 * 1024 * 1024;

fn platform_key() -> &'static str {
    if cfg!(all(windows, target_arch = "aarch64")) {
        "windows-aarch64"
    } else if cfg!(windows) {
        "windows-x86_64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-aarch64"
    } else if cfg!(target_os = "macos") {
        "darwin-x86_64"
    } else {
        "linux-x86_64"
    }
}

/// 测试开关 `CHENOCR_TEST_UPDATE_URL=http://127.0.0.1:端口/latest.json`：只查这一个地址（允许 http）。
fn test_url() -> Option<String> {
    std::env::var("CHENOCR_TEST_UPDATE_URL")
        .ok()
        .filter(|u| !u.is_empty())
}

/// 测试开关 `CHENOCR_TEST_UPDATE_NO_INSTALL=1`：下载并验签，但不运行安装包、不退出。
fn test_no_install() -> bool {
    std::env::var("CHENOCR_TEST_UPDATE_NO_INSTALL").is_ok_and(|v| v == "1")
}

pub fn endpoints(channel: &str) -> Vec<String> {
    if let Some(url) = test_url() {
        return vec![url];
    }
    let (first, second) = if channel == "global" {
        (GLOBAL_MANIFEST, CN_MANIFEST)
    } else {
        (CN_MANIFEST, GLOBAL_MANIFEST)
    };
    vec![first.to_string(), second.to_string()]
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Changes {
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub fixed: Vec<String>,
    #[serde(default)]
    pub improved: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Platform {
    pub url: String,
    pub signature: String,
    #[serde(default)]
    pub size: Option<u64>,
}

/// 更新清单。字段名和 Tauri updater 的静态 JSON 一致，以后换官方插件也能直接用。
#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    #[serde(default)]
    pub changes: Changes,
    /// 英文界面用的简介（可选）
    #[serde(default, rename = "changesEn")]
    pub changes_en: Option<Changes>,
    pub platforms: std::collections::HashMap<String, Platform>,
}

/// 设置页显示的更新简介。
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub date: Option<String>,
    pub changes: Changes,
    pub changes_en: Option<Changes>,
    pub notes: Option<String>,
    pub size: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current: String,
    pub checking: bool,
    pub last_check: Option<i64>,
    pub error: Option<String>,
    pub available: Option<UpdateInfo>,
    /// downloading | verifying | installing | ready（测试模式下载好了） | None
    pub stage: Option<String>,
    /// 下载进度 0–1（不知道总大小时为 None）
    pub progress: Option<f32>,
}

#[derive(Default)]
pub struct UpdateService {
    status: Mutex<UpdateStatus>,
    pending: Mutex<Option<(Manifest, Platform)>>,
    installing: AtomicBool,
}

fn emit(app: &AppHandle) {
    let _ = app.emit(events::UPDATE_STATUS, status(app));
}

fn set(app: &AppHandle, f: impl FnOnce(&mut UpdateStatus)) {
    f(&mut state(app).update.status.lock());
    emit(app);
}

pub fn current_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

pub fn status(app: &AppHandle) -> UpdateStatus {
    let mut s = state(app).update.status.lock().clone();
    s.current = current_version(app);
    s
}

/// `remote` 比 `current` 新才算更新。版本号解析不了的一律不算。
pub fn is_newer(remote: &str, current: &str) -> bool {
    match (
        semver::Version::parse(remote.trim().trim_start_matches('v')),
        semver::Version::parse(current.trim().trim_start_matches('v')),
    ) {
        (Ok(r), Ok(c)) => r > c,
        _ => false,
    }
}

pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        // 测试时不用等半分钟
        let delay = if test_url().is_some() {
            Duration::from_secs(3)
        } else {
            FIRST_CHECK_DELAY
        };
        std::thread::sleep(delay);
        loop {
            if state(&app).settings.read().update.auto_check {
                let _ = tauri::async_runtime::block_on(check(&app, false));
            }
            std::thread::sleep(CHECK_INTERVAL);
        }
    });
}

async fn fetch_manifest(app: &AppHandle, url: &str) -> AppResult<Manifest> {
    let res = net::client(app, net::Purpose::Update)
        .get(url)
        .header("Cache-Control", "no-cache")
        .send()
        .await?;
    if !res.status().is_success() {
        return Err(AppError::Network(format!("HTTP {}", res.status().as_u16())));
    }
    let text = res.text().await?;
    serde_json::from_str(&text).map_err(|e| AppError::msg(format!("更新信息格式不对：{e}")))
}

/// 查一次。`manual` = 用户点了"检查更新"：不弹提示（设置页上直接显示结果）。
pub async fn check(app: &AppHandle, manual: bool) -> AppResult<UpdateStatus> {
    if state(app).update.status.lock().checking {
        return Ok(status(app));
    }
    set(app, |s| {
        s.checking = true;
        s.error = None;
    });
    let channel = state(app).settings.read().update.channel.clone();
    let mut last_err = None;
    let mut found = None;
    for url in endpoints(&channel) {
        match fetch_manifest(app, &url).await {
            Ok(m) => {
                found = Some(m);
                break;
            }
            Err(err) => {
                tracing::info!(url, "检查更新失败：{err}");
                last_err = Some(err);
            }
        }
    }
    let current = current_version(app);
    let result = match found {
        None => Err(last_err.unwrap_or_else(|| AppError::msg("没有可用的更新地址"))),
        Some(m) => {
            let platform = m.platforms.get(platform_key()).cloned();
            match platform {
                Some(p) if is_newer(&m.version, &current) => {
                    let info = UpdateInfo {
                        version: m.version.trim_start_matches('v').to_string(),
                        date: m.pub_date.clone(),
                        changes: m.changes.clone(),
                        changes_en: m.changes_en.clone(),
                        notes: m.notes.clone(),
                        size: p.size,
                    };
                    *state(app).update.pending.lock() = Some((m, p));
                    Ok(Some(info))
                }
                _ => {
                    *state(app).update.pending.lock() = None;
                    Ok(None)
                }
            }
        }
    };
    let now = now_ms();
    match result {
        Ok(available) => {
            set(app, |s| {
                s.checking = false;
                s.last_check = Some(now);
                s.error = None;
                s.available = available.clone();
            });
            if let Some(info) = available {
                tracing::info!(version = %info.version, "发现新版本");
                if !manual {
                    notify_once(app, &info.version);
                }
            }
        }
        Err(err) => {
            set(app, |s| {
                s.checking = false;
                s.last_check = Some(now);
                s.error = Some(friendly(&err));
            });
        }
    }
    Ok(status(app))
}

fn friendly(err: &AppError) -> String {
    match err {
        AppError::Network(m) if m.contains("HTTP 404") => "更新服务器上还没有发布信息".into(),
        AppError::Network(_) => "连不上更新服务器，检查一下网络或代理，或者换个更新渠道".into(),
        other => other.to_string(),
    }
}

/// 自动检查发现新版本：右下角轻提示一次（每个版本只提示一次；用户选了"不显示更新提示"的不提示）。
fn notify_once(app: &AppHandle, version: &str) {
    let st = state(app);
    if st.settings.read().update.skipped_version == version {
        return;
    }
    if st.db.kv_get("update.notified").ok().flatten().as_deref() == Some(version) {
        return;
    }
    let _ = st.db.kv_set("update.notified", version);
    wm::toast(
        app,
        "info",
        format!("DATO COR 有新版本 {version}，可以在「设置 → 更新」里查看"),
    );
}

/// "不显示更新提示"：这个版本不再提示，有更新的版本时再提示。
pub fn skip(app: &AppHandle, version: &str) -> AppResult<()> {
    let version = version.to_string();
    crate::commands::system::update_settings_with(app, move |s| {
        s.update.skipped_version = version;
    })?;
    emit(app);
    Ok(())
}

/// 签名可信注释里的文件名要带着这个版本号（`DATO-COR_0.3.0_x64-setup.exe`）。
fn signed_for_version(trusted_comment: &str, version: &str) -> bool {
    trusted_comment
        .split('\t')
        .find_map(|part| part.strip_prefix("file:"))
        .is_some_and(|file| file.contains(&format!("_{version}_")))
}

pub fn verify(data: &[u8], signature_b64: &str, version: &str) -> AppResult<()> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let decode = |s: &str| -> AppResult<String> {
        let bytes = b64
            .decode(s.trim())
            .map_err(|_| AppError::msg("签名格式不对"))?;
        String::from_utf8(bytes).map_err(|_| AppError::msg("签名格式不对"))
    };
    let pk = minisign_verify::PublicKey::decode(&decode(PUBKEY)?)
        .map_err(|e| AppError::msg(format!("公钥无效：{e}")))?;
    let sig = minisign_verify::Signature::decode(&decode(signature_b64)?)
        .map_err(|e| AppError::msg(format!("签名无效：{e}")))?;
    pk.verify(data, &sig, true)
        .map_err(|_| AppError::msg("安装包签名校验失败，已停止更新（文件可能被改过）"))?;
    if !signed_for_version(sig.trusted_comment(), version) {
        return Err(AppError::msg("安装包和版本号对不上，已停止更新"));
    }
    Ok(())
}

/// 下载安装包并验签，返回文件内容。`on_progress` 拿到 0–1 的进度（不知道总大小时是 None）。
pub async fn download_verified(
    client: &reqwest::Client,
    platform: &Platform,
    version: &str,
    mut on_progress: impl FnMut(Option<f32>),
) -> AppResult<Vec<u8>> {
    let res = client.get(&platform.url).send().await?;
    if !res.status().is_success() {
        return Err(AppError::Network(format!(
            "下载安装包失败：HTTP {}",
            res.status().as_u16()
        )));
    }
    let total = res.content_length().or(platform.size);
    if total.is_some_and(|t| t > MAX_INSTALLER) {
        return Err(AppError::msg("安装包大小不对，已停止"));
    }
    let mut data: Vec<u8> = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        data.extend_from_slice(&chunk?);
        if data.len() as u64 > MAX_INSTALLER {
            return Err(AppError::msg("安装包大小不对，已停止"));
        }
        on_progress(total.map(|t| (data.len() as f32 / t as f32).min(1.0)));
    }
    verify(&data, &platform.signature, version)?;
    Ok(data)
}

/// 下载、验签、运行安装包（Windows 上随后退出，安装完自动重新打开）。
pub async fn install(app: &AppHandle) -> AppResult<()> {
    let svc = &state(app).update;
    if svc.installing.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let result = install_inner(app).await;
    state(app).update.installing.store(false, Ordering::SeqCst);
    if let Err(err) = &result {
        set(app, |s| {
            s.stage = None;
            s.progress = None;
            s.error = Some(err.to_string());
        });
    }
    result
}

async fn install_inner(app: &AppHandle) -> AppResult<()> {
    let (manifest, platform) = state(app)
        .update
        .pending
        .lock()
        .clone()
        .ok_or_else(|| AppError::msg("没有可安装的更新，先检查一下更新"))?;
    let version = manifest.version.trim_start_matches('v').to_string();
    set(app, |s| {
        s.error = None;
        s.stage = Some("downloading".into());
        s.progress = Some(0.0);
    });

    let client = net::client(app, net::Purpose::Download);
    let mut last_emit = Instant::now();
    let data = download_verified(&client, &platform, &version, |progress| {
        if last_emit.elapsed() > Duration::from_millis(150) {
            last_emit = Instant::now();
            set(app, |s| s.progress = progress);
        }
    })
    .await?;
    set(app, |s| {
        s.stage = Some("verifying".into());
        s.progress = Some(1.0);
    });

    let dir = std::env::temp_dir().join("DATO-COR-update");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("DATO-COR_{version}_setup.exe"));
    std::fs::write(&file, &data)?;
    tracing::info!(version, path = %file.display(), "更新包已下载并校验");

    if test_no_install() {
        set(app, |s| s.stage = Some("ready".into()));
        return Ok(());
    }
    set(app, |s| s.stage = Some("installing".into()));
    launch_installer(app, &file)
}

#[cfg(windows)]
fn launch_installer(app: &AppHandle, file: &std::path::Path) -> AppResult<()> {
    // 和 Tauri 官方更新插件一样的参数：/P 只显示进度，/UPDATE 走更新流程，/R 装完重新打开
    std::process::Command::new(file)
        .args(["/P", "/UPDATE", "/R"])
        .spawn()
        .map_err(|e| AppError::msg(format!("打不开安装包：{e}")))?;
    // 安装包要替换正在运行的程序文件，这边得马上退出
    crate::quit(app);
    Ok(())
}

#[cfg(not(windows))]
fn launch_installer(_app: &AppHandle, file: &std::path::Path) -> AppResult<()> {
    Err(AppError::msg(format!(
        "这个系统上还不能自动安装，安装包已下载到 {}",
        file.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("0.3.0", "0.2.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("0.3.0-beta.1", "0.3.0"));
        assert!(!is_newer("garbage", "0.2.0"));
    }

    #[test]
    fn manifest_parses_tauri_format_with_changes() {
        let json = r#"{
          "version": "0.3.0",
          "notes": "多端同步",
          "pub_date": "2026-10-06T00:00:00Z",
          "changes": { "added": ["多设备同步剪贴板"], "fixed": ["取色保存失败"] },
          "platforms": { "windows-x86_64": { "signature": "c2ln", "url": "https://x/a.exe", "size": 123 } }
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.version, "0.3.0");
        assert_eq!(m.changes.added, vec!["多设备同步剪贴板"]);
        assert!(m.changes.improved.is_empty());
        assert_eq!(m.platforms["windows-x86_64"].size, Some(123));
    }

    #[test]
    fn channel_order_and_fallback() {
        let cn = endpoints("cn");
        assert!(cn[0].contains("gitee.com"));
        assert!(cn[1].contains("github"));
        let global = endpoints("global");
        assert!(global[0].contains("github"));
        assert!(global[1].contains("gitee.com"));
    }

    #[test]
    fn signed_file_name_must_carry_version() {
        assert!(signed_for_version(
            "timestamp:1791270708\tfile:DATO-COR_0.3.0_x64-setup.exe",
            "0.3.0"
        ));
        assert!(!signed_for_version(
            "timestamp:1791270708\tfile:DATO-COR_0.2.0_x64-setup.exe",
            "0.3.0"
        ));
        assert!(!signed_for_version("timestamp:1791270708", "0.3.0"));
    }

    /// 用发版私钥签过的一个小文件（文件名 `DATO-COR_9.9.9_x64-setup.exe`）：
    /// 证明内置公钥和发版私钥是一对、验签和版本号检查都走得通。
    const FIXTURE: &[u8] = b"DATO COR updater test fixture\n";
    const FIXTURE_SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRK3F5ZUpBMEtwdVNEaEhyblhwYkRuZkhRUXNDWGhDQUxXTjJqcVVRNXl3aUVIU0NBWndldlZtOEp5S3NrNTVnV0pDZXdSaERNZmFkanZEdkR5OFh2bHNpcS9YWmgvQndzPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkxMjcwODE1CWZpbGU6REFUTy1DT1JfOS45LjlfeDY0LXNldHVwLmV4ZQpRYUltcmIvU3NXUllqR0EwTEZMY0EwTlJ6NHc3enJlR2pkYW5ObTN0Z2VRZjgwNFI5OUhFLzEzaldNVmhIVTlvZ3BBeUNlcVluaURLdFJmMEU3NHlCQT09Cg==";

    #[test]
    fn real_signature_verifies() {
        verify(FIXTURE, FIXTURE_SIG, "9.9.9").unwrap();
        // 内容改一个字节、或者拿它冒充别的版本，都不行
        assert!(verify(b"DATO COR updater test fixturE\n", FIXTURE_SIG, "9.9.9").is_err());
        assert!(verify(FIXTURE, FIXTURE_SIG, "9.9.10").is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn download_checks_signature() {
        use axum::routing::get;
        let app = axum::Router::new()
            .route("/good.exe", get(|| async { FIXTURE.to_vec() }))
            .route(
                "/bad.exe",
                get(|| async { b"DATO COR updater test fixturE\n".to_vec() }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let platform = |path: &str| Platform {
            url: format!("http://127.0.0.1:{port}{path}"),
            signature: FIXTURE_SIG.into(),
            size: None,
        };
        let mut last = None;
        let data = download_verified(&client, &platform("/good.exe"), "9.9.9", |p| last = p)
            .await
            .unwrap();
        assert_eq!(data, FIXTURE);
        assert_eq!(last, Some(1.0));
        let err = download_verified(&client, &platform("/bad.exe"), "9.9.9", |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("签名校验失败"), "{err}");
        let err = download_verified(&client, &platform("/missing.exe"), "9.9.9", |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
    }

    #[test]
    fn rejects_bad_signatures() {
        assert!(verify(b"data", "not-base64!!", "0.3.0").is_err());
        // 格式对但不是这把钥匙签的：用一个随便的 minisign 签名
        let foreign = base64::engine::general_purpose::STANDARD.encode(
            "untrusted comment: x\nRUQ+qyeJA0KpuR3a0WbHDYhfWd2aVOhAwq9ZbNt+K8L9Z5DK7Xw4gKbLbuGiY+M+DcJ4G6r1pLzlzJwKHYhaN9cS7WNd1Ql9Ewo=\ntrusted comment: timestamp:1\tfile:DATO-COR_0.3.0_x64-setup.exe\nAAAA\n",
        );
        assert!(verify(b"data", &foreign, "0.3.0").is_err());
    }
}
