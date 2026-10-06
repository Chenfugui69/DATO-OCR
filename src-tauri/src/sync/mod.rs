//! 多端同步（规格 09）。
//!
//! 两条路，可以同时开：
//! - **局域网 + 设备码**（`lan`）：一台电脑当主机，其他设备输入它的设备码、主机点同意后加入。
//!   电脑之间端到端加密；手机用浏览器打开主机的同步网页，或者用快捷指令收发。
//! - **WebDAV 网盘**（`webdav`）：同一个网盘账号 + 同一个同步密码的设备自动同步，不在一个网络也行。
//!
//! 本机复制的文字 / 链接 / 颜色 / 图片发给其他设备；收到的记录进历史，刚复制的还会直接放进
//! 本机剪贴板（Ctrl+V 就能粘）。同一条记录按 `sync_id` 去重；每次发送带发出时间，
//! `(sync_id, 发出时间)` 处理过的不再处理，所以经过几条路转发也不会打转。

pub mod crypto;
pub mod lan;
pub mod record;
pub mod webdav;

#[cfg(test)]
mod net_tests;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Listener};

use crate::error::{AppError, AppResult};
use crate::platform;
use crate::state::state;
use crate::storage::sync::{self as peers, Peer};
use crate::storage::{clipboard as repo, now_ms, secrets};
use crate::{events, net, wm};

use lan::discovery::{self, Advertiser};
use lan::host::{Backend, Content, HostInfo, PairRequestView, Server, WebItem};
use lan::member::{Credentials, Member, MemberStatus};
use record::SyncRecord;
use webdav::{Account, Runner};

/// 处理过的 (记录, 发出时间) 记多久
const SEEN_TTL: Duration = Duration::from_secs(1800);
/// 刚收到的记录，这么久之内本机又"复制"了它（多半是写剪贴板的回声）就不再发出去
const ECHO_GUARD: Duration = Duration::from_secs(15);
/// 同步的文字上限
const MAX_TEXT: usize = 2 * 1024 * 1024;

/// 测试开关 `CHENOCR_TEST_SYNC_LOOPBACK=1`：局域网服务只绑 127.0.0.1（不触发防火墙询问）、
/// 不做 mDNS 广播、加入申请自动同意且不弹主窗口。只绑回环时只有本机进程连得上，自动同意不会放外人进来。
fn test_loopback() -> bool {
    std::env::var("CHENOCR_TEST_SYNC_LOOPBACK").is_ok_and(|v| v == "1")
}

const KV_DEVICE_ID: &str = "sync.device_id";
const KV_CODE: &str = "sync.code";
const KV_DAV_ACCOUNT: &str = "sync.webdav.account";
const KV_DAV_SEEN: &str = "sync.webdav.seen";
const SECRET_DAV_PASSWORD: &str = "sync.webdav.password";
const SECRET_DAV_KEY: &str = "sync.webdav.key";

/// 一条记录是从哪来的，决定还要转发到哪
#[derive(Clone, Debug, PartialEq)]
pub enum Via {
    /// 本机复制的
    Local,
    /// 本机是主机，某台成员推上来的
    Member(String),
    /// 本机是成员，主机推下来的
    Host,
    Webdav,
}

#[derive(Default)]
pub struct SyncService {
    device_id: OnceLock<String>,
    seen: Mutex<HashMap<(String, i64), Instant>>,
    inbound: Mutex<HashMap<String, Instant>>,
    lan: Mutex<LanRuntime>,
    member: Mutex<Option<(String, Member)>>,
    join: Mutex<JoinState>,
    join_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    webdav: Mutex<Option<(String, Runner)>>,
    last_seen_write: Mutex<HashMap<String, Instant>>,
    reconfiguring: Mutex<()>,
}

#[derive(Default)]
struct LanRuntime {
    server: Option<Server>,
    advertiser: Option<Advertiser>,
    /// 当前服务的 (端口, 名字, 设备码)，变了才重启
    config: Option<(u16, String, String)>,
    error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinState {
    /// idle | searching | waiting | done | error
    pub phase: String,
    pub sas: Option<String>,
    pub host_name: Option<String>,
    pub error: Option<String>,
}

// ───────────────────────── 本机身份 ─────────────────────────

pub fn device_id(app: &AppHandle) -> String {
    let st = state(app);
    let svc = &st.sync;
    svc.device_id
        .get_or_init(|| {
            let db = &state(app).db;
            if let Ok(Some(id)) = db.kv_get(KV_DEVICE_ID) {
                return id;
            }
            let id = uuid::Uuid::new_v4().simple().to_string();
            let _ = db.kv_set(KV_DEVICE_ID, &id);
            id
        })
        .clone()
}

pub fn default_device_name() -> String {
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(windows))]
    let name = std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());
    name.filter(|n| !n.is_empty())
        .unwrap_or_else(|| "DATO OCR".into())
}

pub fn device_name(app: &AppHandle) -> String {
    let name = state(app).settings.read().sync.device_name.clone();
    if name.is_empty() {
        default_device_name()
    } else {
        name
    }
}

pub fn platform_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn new_code() -> String {
    let bytes = crypto::random_bytes(8);
    let mut code = String::new();
    for (i, b) in bytes.iter().enumerate() {
        // 首位不出 0，读起来不像少了一位
        let d = if i == 0 { 1 + b % 9 } else { b % 10 };
        code.push(char::from(b'0' + d));
    }
    code
}

pub fn device_code(app: &AppHandle) -> String {
    let db = &state(app).db;
    if let Ok(Some(code)) = db.kv_get(KV_CODE) {
        if code.len() == 8 {
            return code;
        }
    }
    let code = new_code();
    let _ = db.kv_set(KV_CODE, &code);
    code
}

pub fn regenerate_code(app: &AppHandle) -> AppResult<String> {
    let code = new_code();
    state(app).db.kv_set(KV_CODE, &code)?;
    reconfigure(app);
    Ok(code)
}

fn changed(app: &AppHandle) {
    let _ = app.emit(events::SYNC_CHANGED, ());
}

// ───────────────────────── 启动 / 重新配置 ─────────────────────────

pub fn start(app: &AppHandle) {
    // 历史变了，让打开着的手机网页刷新
    let ui = app.clone();
    app.listen_any(events::CLIPBOARD_CHANGED, move |_| {
        let st = state(&ui);
        let lan = st.sync.lan.lock();
        if let Some(server) = &lan.server {
            server.host.notify_changed();
        }
    });
    let app = app.clone();
    std::thread::spawn(move || reconfigure(&app));
}

/// 按设置把各条路开起来 / 关掉。设置、配对关系变了都调一次，没变的部分不动。
pub fn reconfigure(app: &AppHandle) {
    let st = state(app);
    let svc = &st.sync;
    let _guard = svc.reconfiguring.lock();
    let settings = state(app).settings.read().sync.clone();

    // 局域网
    let host_peer = state(app).db.with(|c| peers::host(c)).ok().flatten();
    match (settings.lan_enabled, host_peer) {
        (false, _) => {
            stop_server(app);
            *svc.member.lock() = None;
        }
        (true, Some(peer)) => {
            stop_server(app);
            ensure_member(app, peer);
        }
        (true, None) => {
            *svc.member.lock() = None;
            ensure_server(app, settings.lan_port);
        }
    }

    // 网盘
    let wanted = settings
        .webdav_enabled
        .then(|| webdav_credentials(app))
        .flatten();
    match wanted {
        None => *svc.webdav.lock() = None,
        Some((account, key)) => {
            let fingerprint = format!(
                "{}|{}|{}|{}|{}",
                account.url,
                account.user,
                account.folder,
                settings.webdav_interval,
                device_name(app)
            );
            let mut slot = svc.webdav.lock();
            if slot.as_ref().map(|(f, _)| f) != Some(&fingerprint) {
                *slot = None;
                match start_webdav(app, account, key, settings.webdav_interval) {
                    Ok(runner) => *slot = Some((fingerprint, runner)),
                    Err(err) => tracing::warn!("WebDAV 同步启动失败：{err}"),
                }
            }
        }
    }
    changed(app);
}

fn stop_server(app: &AppHandle) {
    let st = state(app);
    let mut lan = st.sync.lan.lock();
    lan.advertiser = None;
    lan.server = None;
    lan.config = None;
    lan.error = None;
}

fn ensure_server(app: &AppHandle, port: u16) {
    let config = (port, device_name(app), device_code(app));
    let st = state(app);
    let mut lan = st.sync.lan.lock();
    if lan.config.as_ref() == Some(&config) && lan.server.is_some() {
        return;
    }
    lan.advertiser = None;
    lan.server = None;
    let backend: Arc<dyn Backend> = Arc::new(AppBackend { app: app.clone() });
    let bind = if test_loopback() {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    } else {
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
    };
    match Server::start(backend, bind, port) {
        Ok(server) => {
            if !test_loopback() {
                match Advertiser::start(&device_id(app), &config.1, &config.2, server.port) {
                    Ok(adv) => lan.advertiser = Some(adv),
                    // 广播不了也不要紧，对方可以手填地址
                    Err(err) => tracing::warn!("mDNS 广播失败：{err}"),
                }
            }
            lan.server = Some(server);
            lan.error = None;
            lan.config = Some(config);
        }
        Err(err) => {
            tracing::warn!("局域网同步服务启动失败：{err}");
            lan.error = Some(err.to_string());
            lan.config = None;
        }
    }
}

fn unprotect_key(secret: &[u8]) -> Option<crypto::SecretKey> {
    platform::unprotect(secret).ok()?.try_into().ok()
}

fn ensure_member(app: &AppHandle, peer: Peer) {
    let st = state(app);
    let mut slot = st.sync.member.lock();
    if slot.as_ref().map(|(id, _)| id) == Some(&peer.device_id) {
        return;
    }
    *slot = None;
    let Some(key) = unprotect_key(&peer.secret) else {
        tracing::warn!("加入主机的密钥解不开（换了电脑或用户？），需要重新加入");
        return;
    };
    let creds = Credentials {
        my_id: device_id(app),
        host_id: peer.device_id.clone(),
        host_name: peer.name.clone(),
        key,
        address: peer.address.as_deref().and_then(|a| a.parse().ok()),
    };
    let on_record = {
        let app = app.clone();
        Arc::new(move |record: SyncRecord| receive(&app, record, Via::Host, true))
    };
    let on_address = {
        let app = app.clone();
        let host_id = peer.device_id.clone();
        Arc::new(move |addr: SocketAddr| {
            let _ = state(&app)
                .db
                .with(|c| peers::seen(c, &host_id, Some(&addr.to_string())));
        })
    };
    *slot = Some((peer.device_id, Member::start(creds, on_record, on_address)));
}

// ───────────────────────── 发出 / 收到 ─────────────────────────

fn active(app: &AppHandle) -> bool {
    let st = state(app);
    let svc = &st.sync;
    svc.lan.lock().server.is_some() || svc.member.lock().is_some() || svc.webdav.lock().is_some()
}

/// 本机复制了一条（新的，或者重新复制了旧的）：发给其他设备。
pub fn on_local(app: &AppHandle, id: i64) {
    if !active(app) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(err) = send_local(&app, id) {
            tracing::debug!(id, "这条不同步：{err}");
        }
    });
}

fn send_local(app: &AppHandle, id: i64) -> AppResult<()> {
    let st = state(app);
    let settings = st.settings.read().sync.clone();
    let src = st.db.with(|c| repo::sync_source(c, id))?;
    if let Some(sid) = &src.sync_id {
        let recent = st
            .sync
            .inbound
            .lock()
            .get(sid)
            .is_some_and(|at| at.elapsed() < ECHO_GUARD);
        if recent {
            return Ok(());
        }
    }
    let mut record = SyncRecord {
        kind: src.kind.clone(),
        created_at: src.created_at,
        sent_at: now_ms(),
        width: src.width.map(|w| w as u32),
        height: src.height.map(|h| h as u32),
        ..Default::default()
    };
    match src.kind.as_str() {
        "text" | "link" | "color" => {
            let text = src.text.clone().unwrap_or_default();
            if text.len() > MAX_TEXT {
                return Err(AppError::msg("文字太长"));
            }
            record.text = Some(text);
        }
        "image" if settings.send_images => {
            let path = src
                .file_path
                .as_deref()
                .ok_or_else(|| AppError::msg("图片文件缺失"))?;
            let bytes = std::fs::read(st.paths.abs(path))?;
            if bytes.len() as u64 > u64::from(settings.max_image_mb) * 1024 * 1024 {
                return Err(AppError::msg("图片超过同步大小上限"));
            }
            record.image = Some(bytes);
        }
        _ => return Err(AppError::msg("这种类型不同步")),
    }
    // 别的设备同步来的、又在本机复制了一次：还算它原来那台的
    let me = device_id(app);
    match &src.device_id {
        Some(origin) => {
            record.origin = origin.clone();
            record.origin_name = src.source_app.clone().unwrap_or_default();
        }
        None => {
            record.origin = me.clone();
            record.origin_name = device_name(app);
            record.app = src.source_app.clone();
        }
    }
    record.id = match src.sync_id {
        Some(sid) => sid,
        None => {
            let sid = uuid::Uuid::new_v4().to_string();
            st.db.with(|c| repo::set_sync_id(c, id, &sid))?;
            sid
        }
    };
    mark_seen(app, &record);
    tracing::debug!(id, kind = %record.kind, "发出同步记录");
    dispatch(app, &record, &Via::Local);
    Ok(())
}

/// 记一笔"处理过"；已经处理过返回 false。
fn mark_seen(app: &AppHandle, record: &SyncRecord) -> bool {
    let st = state(app);
    let mut seen = st.sync.seen.lock();
    seen.retain(|_, at| at.elapsed() < SEEN_TTL);
    seen.insert((record.id.clone(), record.sent_at), Instant::now())
        .is_none()
}

/// 转发：从哪来的就不回哪去。
fn dispatch(app: &AppHandle, record: &SyncRecord, via: &Via) {
    let st = state(app);
    let svc = &st.sync;
    if let Some(server) = &svc.lan.lock().server {
        let exclude = match via {
            Via::Member(id) => Some(id.as_str()),
            _ => None,
        };
        server.host.broadcast(record, exclude);
    }
    if let Some((_, member)) = svc.member.lock().as_ref() {
        if *via != Via::Host {
            member.push(record.clone());
        }
    }
    let webdav = svc.webdav.lock();
    if let Some((_, runner)) = webdav.as_ref() {
        if *via != Via::Webdav {
            runner.push(record.clone());
        }
    }
}

/// 收到其他设备的一条记录。`live` = 刚复制的，可以直接放进本机剪贴板。
pub fn receive(app: &AppHandle, record: SyncRecord, via: Via, live: bool) {
    if record.origin == device_id(app) || !mark_seen(app, &record) {
        return;
    }
    let st = state(app);
    if !st.settings.read().clipboard.enabled {
        return;
    }
    st.sync
        .inbound
        .lock()
        .insert(record.id.clone(), Instant::now());
    let (id, payload) = match crate::clipboard::store_remote(app, &record) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(from = %record.origin_name, "收到的同步记录存不下：{err}");
            return;
        }
    };
    tracing::info!(id, from = %record.origin_name, kind = %record.kind, "收到同步记录");
    if live && st.settings.read().sync.auto_write {
        if let Some(payload) = payload {
            if let Err(err) = crate::clipboard::write_own(app, &payload, Some(id)) {
                tracing::warn!("写入剪贴板失败：{err}");
            }
        }
    }
    dispatch(app, &record, &via);
}

// ───────────────────────── 主机的后端 ─────────────────────────

struct AppBackend {
    app: AppHandle,
}

impl Backend for AppBackend {
    fn info(&self) -> HostInfo {
        let s = state(&self.app).settings.read().sync.clone();
        HostInfo {
            device_id: device_id(&self.app),
            name: device_name(&self.app),
            platform: platform_name().into(),
            code: device_code(&self.app),
            web_enabled: s.web_enabled,
            max_upload: s.max_image_mb as usize * 1024 * 1024,
        }
    }

    fn pair_requested(&self) {
        changed(&self.app);
        if test_loopback() {
            let app = self.app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                let ids: Vec<String> = {
                    let st = state(&app);
                    let lan = st.sync.lan.lock();
                    lan.server
                        .as_ref()
                        .map(|s| s.host.pending().into_iter().map(|p| p.id).collect())
                        .unwrap_or_default()
                };
                for id in ids {
                    let _ = approve(&app, &id, true);
                }
            });
            return;
        }
        let app = self.app.clone();
        let _ = self
            .app
            .run_on_main_thread(move || wm::show_main(&app, Some("sync")));
    }

    fn save_member(&self, mut peer: Peer, key: Option<crypto::SecretKey>) -> AppResult<()> {
        if let Some(key) = key {
            peer.secret = platform::protect(&key)?;
        }
        state(&self.app).db.with(|c| peers::upsert(c, &peer))?;
        tracing::info!(name = %peer.name, kind = %peer.kind, "新设备加入同步");
        changed(&self.app);
        Ok(())
    }

    fn member_key(&self, device_id: &str) -> Option<crypto::SecretKey> {
        let peer = state(&self.app)
            .db
            .with(|c| peers::get(c, device_id))
            .ok()
            .flatten()
            .filter(|p| p.role == "member" && p.kind == "desktop")?;
        unprotect_key(&peer.secret)
    }

    fn web_member(&self, token_hash: &[u8]) -> Option<Peer> {
        state(&self.app)
            .db
            .with(|c| peers::find_web(c, token_hash))
            .ok()
            .flatten()
    }

    fn remove_member(&self, device_id: &str) {
        let _ = state(&self.app).db.with(|c| peers::remove(c, device_id));
        changed(&self.app);
    }

    fn seen(&self, device_id: &str, addr: SocketAddr) {
        // 网页每次刷新都会走到这，写库限个流
        {
            let st = state(&self.app);
            let mut last = st.sync.last_seen_write.lock();
            if last
                .get(device_id)
                .is_some_and(|at| at.elapsed() < Duration::from_secs(30))
            {
                return;
            }
            last.insert(device_id.to_string(), Instant::now());
        }
        let _ = state(&self.app)
            .db
            .with(|c| peers::seen(c, device_id, Some(&addr.ip().to_string())));
        changed(&self.app);
    }

    fn received(&self, record: SyncRecord, from: &str) {
        receive(&self.app, record, Via::Member(from.to_string()), true);
    }

    fn recent(&self, limit: u32) -> Vec<WebItem> {
        let q = repo::ClipQuery {
            kinds: ["text", "link", "color", "image"]
                .map(String::from)
                .to_vec(),
            limit,
            ..Default::default()
        };
        state(&self.app)
            .db
            .with(|c| repo::query(c, &q))
            .map(|page| {
                page.items
                    .into_iter()
                    .map(|i| WebItem {
                        id: i.id,
                        kind: i.kind,
                        preview: i.preview,
                        width: i.width,
                        height: i.height,
                        source: i.source_app,
                        at: i.last_used_at,
                        remote: i.remote,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn content(&self, id: i64, thumb: bool) -> Option<Content> {
        let st = state(&self.app);
        let detail = st.db.with(|c| repo::get_detail(c, id)).ok()?;
        if detail.item.kind == "image" {
            let path = if thumb {
                detail.item.thumb_path.or(detail.item.file_path)
            } else {
                detail.item.file_path
            }?;
            let bytes = std::fs::read(st.paths.abs(&path)).ok()?;
            let ct = if path.ends_with(".png") {
                "image/png"
            } else {
                "image/jpeg"
            };
            return Some(Content::Image(bytes, ct));
        }
        if matches!(detail.item.kind.as_str(), "text" | "link" | "color") {
            return detail.content_text.map(Content::Text);
        }
        None
    }

    fn latest(&self) -> Option<Content> {
        let id = self.recent(1).first()?.id;
        self.content(id, false)
    }
}

// ───────────────────────── 网盘 ─────────────────────────

fn stored_account(app: &AppHandle) -> Option<(String, String, String)> {
    let raw = state(app).db.kv_get(KV_DAV_ACCOUNT).ok()??;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some((
        v["url"].as_str()?.to_string(),
        v["user"].as_str()?.to_string(),
        v["folder"].as_str()?.to_string(),
    ))
}

/// 设置里的网盘账号和连接时验证过的一致，才算配置好了。
fn webdav_credentials(app: &AppHandle) -> Option<(Account, crypto::SecretKey)> {
    let st = state(app);
    let s = st.settings.read().sync.clone();
    let (url, user, folder) = stored_account(app)?;
    if (url.as_str(), user.as_str(), folder.as_str())
        != (
            s.webdav_url.as_str(),
            s.webdav_user.as_str(),
            s.webdav_folder.as_str(),
        )
    {
        return None;
    }
    let (password, key) = st
        .db
        .with(|c| {
            Ok((
                secrets::get(c, SECRET_DAV_PASSWORD)?,
                secrets::get(c, SECRET_DAV_KEY)?,
            ))
        })
        .ok()?;
    let key: crypto::SecretKey = B64.decode(key?).ok()?.try_into().ok()?;
    Some((
        Account {
            url,
            user,
            password: password?,
            folder,
        },
        key,
    ))
}

fn start_webdav(
    app: &AppHandle,
    account: Account,
    key: crypto::SecretKey,
    interval: u32,
) -> AppResult<Runner> {
    let on_record = {
        let app = app.clone();
        Arc::new(move |record: SyncRecord, live: bool| receive(&app, record, Via::Webdav, live))
    };
    let seen_store = {
        let app = app.clone();
        Arc::new(
            move |next: Option<&HashSet<String>>| -> Option<HashSet<String>> {
                let db = &state(&app).db;
                match next {
                    Some(set) => {
                        let list: Vec<&String> = set.iter().collect();
                        let _ = db.kv_set(
                            KV_DAV_SEEN,
                            &serde_json::to_string(&list).unwrap_or_default(),
                        );
                        None
                    }
                    None => db
                        .kv_get(KV_DAV_SEEN)
                        .ok()
                        .flatten()
                        .and_then(|raw| serde_json::from_str::<HashSet<String>>(&raw).ok()),
                }
            },
        )
    };
    Runner::start(
        net::client(app, net::Purpose::Sync),
        webdav::Config {
            account,
            key,
            my_id: device_id(app),
            my_name: device_name(app),
            platform: platform_name().into(),
            interval: Duration::from_secs(u64::from(interval)),
        },
        on_record,
        seen_store,
    )
}

/// 连上网盘：验证账号、用同步密码拿到密钥，保存后开始同步。`password` 留空 = 沿用已保存的。
pub async fn webdav_connect(
    app: &AppHandle,
    url: String,
    user: String,
    password: String,
    folder: String,
    sync_password: String,
) -> AppResult<()> {
    let st = state(app);
    let folder = folder.trim().trim_matches(['/', '\\']).to_string();
    let folder = if folder.is_empty() {
        "DATO-OCR".to_string()
    } else {
        folder
    };
    let password = if password.is_empty() {
        st.db
            .with(|c| secrets::get(c, SECRET_DAV_PASSWORD))?
            .ok_or_else(|| AppError::msg("请填写 WebDAV 密码"))?
    } else {
        password
    };
    let account = Account {
        url: url.trim().to_string(),
        user: user.trim().to_string(),
        password,
        folder,
    };
    if account.user.is_empty() {
        return Err(AppError::msg("请填写 WebDAV 账号"));
    }
    let dav = webdav::Dav::new(net::client(app, net::Purpose::Sync), &account)?;
    let key = dav
        .open_group(&sync_password, crypto::KdfParams::default())
        .await?;
    st.db.with(|c| {
        secrets::set(c, SECRET_DAV_PASSWORD, Some(&account.password))?;
        secrets::set(c, SECRET_DAV_KEY, Some(&B64.encode(key)))?;
        Ok(())
    })?;
    st.db.kv_set(
        KV_DAV_ACCOUNT,
        &serde_json::json!({ "url": account.url, "user": account.user, "folder": account.folder })
            .to_string(),
    )?;
    // 换了账号或文件夹：处理过哪些文件要重新算
    st.db.with(|c| {
        c.execute("DELETE FROM kv WHERE key = ?1", [KV_DAV_SEEN])?;
        Ok(())
    })?;
    crate::commands::system::update_settings_with(app, |s| {
        s.sync.webdav_enabled = true;
        s.sync.webdav_url = account.url.clone();
        s.sync.webdav_user = account.user.clone();
        s.sync.webdav_folder = account.folder.clone();
    })?;
    *st.sync.webdav.lock() = None;
    reconfigure(app);
    Ok(())
}

pub fn webdav_disconnect(app: &AppHandle) -> AppResult<()> {
    let st = state(app);
    st.db.with(|c| {
        secrets::set(c, SECRET_DAV_PASSWORD, None)?;
        secrets::set(c, SECRET_DAV_KEY, None)?;
        c.execute(
            "DELETE FROM kv WHERE key IN (?1, ?2)",
            [KV_DAV_ACCOUNT, KV_DAV_SEEN],
        )?;
        Ok(())
    })?;
    crate::commands::system::update_settings_with(app, |s| s.sync.webdav_enabled = false)?;
    reconfigure(app);
    Ok(())
}

pub fn webdav_now(app: &AppHandle) {
    let st = state(app);
    let webdav = st.sync.webdav.lock();
    if let Some((_, runner)) = webdav.as_ref() {
        runner.poke();
    }
}

pub async fn webdav_devices(app: &AppHandle) -> AppResult<Vec<webdav::DavDevice>> {
    let (account, key) = webdav_credentials(app).ok_or_else(|| AppError::msg("还没有连上网盘"))?;
    webdav::devices(net::client(app, net::Purpose::Sync), &account, &key).await
}

// ───────────────────────── 配对 / 设备管理 ─────────────────────────

pub fn approve(app: &AppHandle, id: &str, ok: bool) -> AppResult<()> {
    let st = state(app);
    let lan = st.sync.lan.lock();
    let server = lan
        .server
        .as_ref()
        .ok_or_else(|| AppError::msg("局域网同步没有开"))?;
    server.host.decide(id, ok)?;
    drop(lan);
    changed(app);
    Ok(())
}

pub fn remove_peer(app: &AppHandle, device_id: &str) -> AppResult<()> {
    let st = state(app);
    if let Some(server) = &st.sync.lan.lock().server {
        server.host.forget(device_id);
    }
    st.db.with(|c| peers::remove(c, device_id))?;
    reconfigure(app);
    Ok(())
}

fn set_join(app: &AppHandle, f: impl FnOnce(&mut JoinState)) {
    {
        let st = state(app);
        f(&mut st.sync.join.lock());
    }
    changed(app);
}

/// 输入设备码加入另一台电脑。`address` 填了就直接连（局域网组播被挡时用）。
pub fn join(app: &AppHandle, code: String, address: Option<String>) -> AppResult<()> {
    let code = lan::host::normalize_code(&code);
    if code.len() != 8 {
        return Err(AppError::msg("设备码是 8 位数字"));
    }
    if code == device_code(app) {
        return Err(AppError::msg("这是本机自己的设备码"));
    }
    let manual: Option<SocketAddr> = match address
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
    {
        None => None,
        Some(a) => {
            let with_port = if a.contains(':') {
                a
            } else {
                format!("{a}:{}", state(app).settings.read().sync.lan_port)
            };
            Some(with_port.parse().map_err(|_| {
                AppError::msg("地址格式不对，应该像 192.168.1.8 或 192.168.1.8:47380")
            })?)
        }
    };
    cancel_join(app);
    set_join(app, |j| {
        *j = JoinState {
            phase: "searching".into(),
            ..Default::default()
        }
    });
    let task_app = app.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let app = task_app;
        let result = run_join(&app, &code, manual).await;
        set_join(&app, |j| match result {
            Ok(host_name) => {
                j.phase = "done".into();
                j.host_name = Some(host_name);
                j.error = None;
            }
            Err(err) => {
                j.phase = "error".into();
                j.error = Some(err.to_string());
            }
        });
    });
    *state(app).sync.join_task.lock() = Some(handle);
    Ok(())
}

async fn run_join(app: &AppHandle, code: &str, manual: Option<SocketAddr>) -> AppResult<String> {
    let addr = match manual {
        Some(a) => a,
        None => lan::member::find_host(code).await?.0,
    };
    let my_id = device_id(app);
    let my_name = device_name(app);
    let creds = lan::member::pair(addr, code, &my_id, &my_name, platform_name(), |sas| {
        let sas = sas.to_string();
        set_join(app, |j| {
            j.phase = "waiting".into();
            j.sas = Some(sas);
        });
    })
    .await?;
    let secret = platform::protect(&creds.key)?;
    let st = state(app);
    st.db.with(|c| {
        peers::remove_role(c, "host")?;
        peers::upsert(
            c,
            &Peer {
                device_id: creds.host_id.clone(),
                name: creds.host_name.clone(),
                platform: String::new(),
                kind: "desktop".into(),
                role: "host".into(),
                secret,
                address: creds.address.map(|a| a.to_string()),
                code: Some(code.to_string()),
                last_seen: Some(now_ms()),
                created_at: 0,
            },
        )
    })?;
    tracing::info!(host = %creds.host_name, "已加入同步主机");
    if !st.settings.read().sync.lan_enabled {
        crate::commands::system::update_settings_with(app, |s| s.sync.lan_enabled = true)?;
    }
    reconfigure(app);
    Ok(creds.host_name)
}

pub fn cancel_join(app: &AppHandle) {
    let task = state(app).sync.join_task.lock().take();
    if let Some(task) = task {
        task.abort();
    }
    set_join(app, |j| *j = JoinState::default());
}

/// 退出加入的主机，本机回到主机模式。
pub fn leave(app: &AppHandle) -> AppResult<()> {
    state(app).db.with(|c| peers::remove_role(c, "host"))?;
    *state(app).sync.member.lock() = None;
    reconfigure(app);
    Ok(())
}

pub fn qr_svg(text: &str) -> AppResult<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| AppError::msg(e.to_string()))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(180, 180)
        .quiet_zone(true)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

// ───────────────────────── 状态 ─────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerView {
    #[serde(flatten)]
    pub peer: Peer,
    pub online: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebdavView {
    pub configured: bool,
    pub running: bool,
    pub last_sync: Option<i64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub device_id: String,
    pub device_name: String,
    pub default_name: String,
    pub platform: String,
    pub code: String,
    /// off | host | member
    pub lan_mode: String,
    pub lan_error: Option<String>,
    pub port: Option<u16>,
    /// 手机扫码 / 浏览器打开的地址（好用的排前面）
    pub urls: Vec<String>,
    pub members: Vec<PeerView>,
    pub host: Option<PeerView>,
    pub host_status: Option<MemberStatus>,
    pub pending: Vec<PairRequestView>,
    pub join: JoinState,
    pub webdav: WebdavView,
}

pub fn status(app: &AppHandle) -> AppResult<SyncStatus> {
    let st = state(app);
    let svc = &st.sync;
    let settings = st.settings.read().sync.clone();
    let all = st.db.with(|c| peers::list(c))?;
    let code = device_code(app);

    let (port, lan_error, online, pending) = {
        let lan = svc.lan.lock();
        match &lan.server {
            Some(s) => (Some(s.port), None, s.host.online(), s.host.pending()),
            None => (None, lan.error.clone(), Vec::new(), Vec::new()),
        }
    };
    let host_status = svc.member.lock().as_ref().map(|(_, m)| m.status());
    let lan_mode = if !settings.lan_enabled {
        "off"
    } else if host_status.is_some() || all.iter().any(|p| p.role == "host") {
        "member"
    } else {
        "host"
    };
    let urls = match port {
        Some(p) if settings.web_enabled => discovery::local_ips()
            .into_iter()
            .map(|ip| format!("http://{ip}:{p}/#c={code}"))
            .collect(),
        _ => Vec::new(),
    };
    let mut members = Vec::new();
    let mut host = None;
    for peer in all {
        let is_online = if peer.role == "host" {
            host_status.as_ref().is_some_and(|s| s.online)
        } else {
            online.contains(&peer.device_id)
        };
        let view = PeerView {
            peer,
            online: is_online,
        };
        if view.peer.role == "host" {
            host = Some(view);
        } else {
            members.push(view);
        }
    }
    let configured = webdav_credentials(app).is_some();
    let join = svc.join.lock().clone();
    let webdav = match svc.webdav.lock().as_ref() {
        Some((_, r)) => {
            let s = r.status.lock().clone();
            WebdavView {
                configured,
                running: s.running,
                last_sync: s.last_sync,
                error: s.error,
            }
        }
        None => WebdavView {
            configured,
            running: false,
            last_sync: None,
            error: None,
        },
    };
    Ok(SyncStatus {
        device_id: device_id(app),
        device_name: device_name(app),
        default_name: default_device_name(),
        platform: platform_name().into(),
        code,
        lan_mode: lan_mode.into(),
        lan_error,
        port,
        urls,
        members,
        host,
        host_status,
        pending,
        join,
        webdav,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_eight_digits_without_leading_zero() {
        for _ in 0..50 {
            let c = new_code();
            assert_eq!(c.len(), 8);
            assert!(c.chars().all(|ch| ch.is_ascii_digit()));
            assert_ne!(c.as_bytes()[0], b'0');
        }
    }
}
