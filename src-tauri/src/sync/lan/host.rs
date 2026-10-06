//! 局域网主机：本机开一个 HTTP 服务，其他设备用设备码申请加入，本机点"同意"后发给它凭据。
//!
//! - 电脑（跑着 DATO OCR 的）：配对时双方交换一次性 X25519 公钥，两边屏幕显示同一个 4 位核对数字；
//!   同意后主机给它一把专属密钥（用配对会话密钥加密传过去）。之后每条记录都用这把密钥加密，
//!   主机通过 SSE 长连接推给它，它用 POST 推上来。
//! - 手机浏览器 / 快捷指令：同意后发一个令牌（浏览器存在 Cookie 里，快捷指令放在请求头里）。
//!   浏览器在 http 页面上用不了 WebCrypto，这条路的内容是明文，靠 Wi-Fi 本身的加密；
//!   所以只接受局域网地址来的连接，设置里也可以关掉网页访问。
//!
//! 和应用的其他部分（数据库、剪贴板）隔着 `Backend` 这个接口，方便单独测试。

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot, Notify};

use super::discovery;
use crate::error::{AppError, AppResult};
use crate::storage::now_ms;
use crate::storage::sync::Peer;
use crate::sync::crypto::{self, Handshake, SecretKey};
use crate::sync::record::SyncRecord;

const WEB_PAGE: &str = include_str!("../../../assets/sync-web.html");
/// 认证时间戳允许的偏差：两台电脑的时钟差不多就行
const AUTH_SKEW_MS: i64 = 5 * 60 * 1000;
const PAIR_TTL: Duration = Duration::from_secs(180);
const MAX_PENDING: usize = 8;
const COOKIE: &str = "dato_token";

pub const AAD_RECORD: &[u8] = b"dato-cor/record";
pub const AAD_MEMBER_KEY: &[u8] = b"dato-cor/pair/member-key";

pub fn auth_aad(device_id: &str) -> Vec<u8> {
    format!("dato-cor/auth/{device_id}").into_bytes()
}

/// 主机这边的设备信息。
#[derive(Clone, Debug)]
pub struct HostInfo {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub code: String,
    pub web_enabled: bool,
    pub max_upload: usize,
}

/// 网页上显示的一条历史。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebItem {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub preview: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub source: Option<String>,
    pub at: i64,
    pub remote: bool,
}

pub enum Content {
    Text(String),
    Image(Vec<u8>, &'static str),
}

/// 主机需要应用提供的东西。
pub trait Backend: Send + Sync + 'static {
    fn info(&self) -> HostInfo;
    /// 有新的加入申请（界面上弹出来让用户同意）
    fn pair_requested(&self);
    /// 同意后保存设备。电脑带着密钥原文（由应用加密保存），网页的 `peer.secret` 已经是令牌哈希
    fn save_member(&self, peer: Peer, key: Option<SecretKey>) -> AppResult<()>;
    fn member_key(&self, device_id: &str) -> Option<SecretKey>;
    fn web_member(&self, token_hash: &[u8]) -> Option<Peer>;
    fn remove_member(&self, device_id: &str);
    fn seen(&self, device_id: &str, addr: SocketAddr);
    /// 收到一条记录（应用负责入库、写剪贴板、转发给其他设备）
    fn received(&self, record: SyncRecord, from: &str);
    fn recent(&self, limit: u32) -> Vec<WebItem>;
    fn content(&self, id: i64, thumb: bool) -> Option<Content>;
    fn latest(&self) -> Option<Content>;
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRequestView {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub kind: String,
    pub sas: Option<String>,
    pub address: String,
}

enum PendState {
    Waiting,
    Approved(serde_json::Value, Option<String>),
    Denied,
}

struct Pending {
    view: PairRequestView,
    device_id: String,
    session_key: Option<SecretKey>,
    state: PendState,
    notify: Arc<Notify>,
    created: Instant,
}

pub struct Host {
    backend: Arc<dyn Backend>,
    pending: Mutex<HashMap<String, Pending>>,
    keys: Mutex<HashMap<String, SecretKey>>,
    desktop: Mutex<HashMap<String, Vec<mpsc::Sender<Event>>>>,
    web: Mutex<Vec<(String, mpsc::Sender<Event>)>>,
    strikes: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

impl Host {
    pub fn new(backend: Arc<dyn Backend>) -> Arc<Self> {
        Arc::new(Self {
            backend,
            pending: Mutex::new(HashMap::new()),
            keys: Mutex::new(HashMap::new()),
            desktop: Mutex::new(HashMap::new()),
            web: Mutex::new(Vec::new()),
            strikes: Mutex::new(HashMap::new()),
        })
    }

    pub fn pending(&self) -> Vec<PairRequestView> {
        let mut map = self.pending.lock();
        map.retain(|_, p| p.created.elapsed() < PAIR_TTL);
        let mut list: Vec<_> = map
            .values()
            .filter(|p| matches!(p.state, PendState::Waiting))
            .map(|p| (p.created, p.view.clone()))
            .collect();
        list.sort_by_key(|(at, _)| *at);
        list.into_iter().map(|(_, v)| v).collect()
    }

    /// 用户点了同意 / 拒绝。
    pub fn decide(&self, id: &str, approve: bool) -> AppResult<()> {
        let mut map = self.pending.lock();
        let p = map
            .get_mut(id)
            .filter(|p| matches!(p.state, PendState::Waiting))
            .ok_or_else(|| AppError::msg("这个申请已经过期了"))?;
        if !approve {
            p.state = PendState::Denied;
            p.notify.notify_waiters();
            return Ok(());
        }
        let info = self.backend.info();
        let addr = p.view.address.clone();
        let mut peer = Peer {
            device_id: p.device_id.clone(),
            name: p.view.name.clone(),
            platform: p.view.platform.clone(),
            kind: p.view.kind.clone(),
            role: "member".into(),
            address: Some(addr),
            last_seen: Some(now_ms()),
            ..Default::default()
        };
        if p.view.kind == "desktop" {
            let session = p.session_key.ok_or_else(|| AppError::msg("配对会话缺失"))?;
            let member_key = crypto::random_key();
            self.backend.save_member(peer, Some(member_key))?;
            self.keys.lock().insert(p.device_id.clone(), member_key);
            let sealed = crypto::seal(&session, &member_key, AAD_MEMBER_KEY);
            p.state = PendState::Approved(
                serde_json::json!({
                    "hostId": info.device_id,
                    "hostName": info.name,
                    "sealedKey": B64.encode(sealed),
                }),
                None,
            );
        } else {
            let token = B64URL.encode(crypto::random_bytes(32));
            peer.secret = Sha256::digest(token.as_bytes()).to_vec();
            self.backend.save_member(peer, None)?;
            p.state = PendState::Approved(
                serde_json::json!({
                    "token": token,
                    "deviceId": p.device_id,
                    "hostName": info.name,
                }),
                Some(token),
            );
        }
        p.notify.notify_waiters();
        Ok(())
    }

    /// 移除设备：断开它的连接，忘掉它的密钥。
    pub fn forget(&self, device_id: &str) {
        self.keys.lock().remove(device_id);
        self.desktop.lock().remove(device_id);
        self.web.lock().retain(|(id, _)| id != device_id);
    }

    /// 在线的设备（有长连接开着）。
    pub fn online(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .desktop
            .lock()
            .iter()
            .filter(|(_, txs)| txs.iter().any(|t| !t.is_closed()))
            .map(|(id, _)| id.clone())
            .collect();
        ids.extend(
            self.web
                .lock()
                .iter()
                .filter(|(_, t)| !t.is_closed())
                .map(|(id, _)| id.clone()),
        );
        ids.sort();
        ids.dedup();
        ids
    }

    /// 把一条记录推给在线的电脑（`exclude` 是它的来源，不回推）。
    pub fn broadcast(&self, record: &SyncRecord, exclude: Option<&str>) {
        let encoded = record.encode();
        let mut desktop = self.desktop.lock();
        for (device_id, txs) in desktop.iter_mut() {
            txs.retain(|t| !t.is_closed());
            if Some(device_id.as_str()) == exclude || txs.is_empty() {
                continue;
            }
            let Some(key) = self.key_for(device_id) else {
                continue;
            };
            let data = B64.encode(crypto::seal(&key, &encoded, AAD_RECORD));
            for tx in txs.iter() {
                let _ = tx.try_send(Event::default().event("record").data(data.clone()));
            }
        }
    }

    /// 剪贴板历史变了：让打开着的网页刷新。
    pub fn notify_changed(&self) {
        let mut web = self.web.lock();
        web.retain(|(_, t)| !t.is_closed());
        for (_, tx) in web.iter() {
            let _ = tx.try_send(Event::default().event("changed").data("1"));
        }
    }

    /// 断开所有长连接（停服务时用，否则优雅关闭会一直等它们）。
    pub fn disconnect_all(&self) {
        self.desktop.lock().clear();
        self.web.lock().clear();
    }

    fn key_for(&self, device_id: &str) -> Option<SecretKey> {
        if let Some(k) = self.keys.lock().get(device_id) {
            return Some(*k);
        }
        let key = self.backend.member_key(device_id)?;
        self.keys.lock().insert(device_id.to_string(), key);
        Some(key)
    }

    fn strike(&self, ip: IpAddr) -> bool {
        let mut s = self.strikes.lock();
        s.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(600));
        let e = s.entry(ip).or_insert((0, Instant::now()));
        e.0 += 1;
        e.0 > 5
    }

    fn blocked(&self, ip: IpAddr) -> bool {
        self.strikes
            .lock()
            .get(&ip)
            .is_some_and(|(n, at)| *n > 5 && at.elapsed() < Duration::from_secs(600))
    }

    fn desktop_auth(&self, headers: &HeaderMap) -> Result<(String, SecretKey), StatusCode> {
        let device = header_str(headers, "x-dato-device").ok_or(StatusCode::UNAUTHORIZED)?;
        let auth = header_str(headers, "x-dato-auth").ok_or(StatusCode::UNAUTHORIZED)?;
        let key = self.key_for(&device).ok_or(StatusCode::UNAUTHORIZED)?;
        let sealed = B64.decode(auth).map_err(|_| StatusCode::UNAUTHORIZED)?;
        let plain = crypto::open(&key, &sealed, &auth_aad(&device))
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let ts = i64::from_le_bytes(plain.try_into().map_err(|_| StatusCode::UNAUTHORIZED)?);
        if (now_ms() - ts).abs() > AUTH_SKEW_MS {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok((device, key))
    }

    fn web_auth(&self, headers: &HeaderMap) -> Result<Peer, StatusCode> {
        if !self.backend.info().web_enabled {
            return Err(StatusCode::FORBIDDEN);
        }
        let token = header_str(headers, "authorization")
            .and_then(|v| v.strip_prefix("Bearer ").map(str::to_string))
            .or_else(|| cookie(headers, COOKIE))
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let hash = Sha256::digest(token.trim().as_bytes());
        self.backend
            .web_member(&hash)
            .ok_or(StatusCode::UNAUTHORIZED)
    }
}

/// 设备凭据：电脑之间用的认证头（专属密钥加密的当前时间）。
pub fn auth_header(device_id: &str, key: &SecretKey) -> String {
    B64.encode(crypto::seal(
        key,
        &now_ms().to_le_bytes(),
        &auth_aad(device_id),
    ))
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

type Shared = Arc<Host>;

pub fn router(host: Shared) -> Router {
    let max_upload = host.backend.info().max_upload.max(1024 * 1024) + 64 * 1024;
    Router::new()
        .route("/", get(page))
        .route("/api/info", get(info))
        .route("/api/pair/request", post(pair_request))
        .route("/api/pair/wait", get(pair_wait))
        .route("/api/me", get(me))
        .route("/api/items", get(items))
        .route("/api/items/{id}/text", get(item_text))
        .route("/api/items/{id}/image", get(item_image))
        .route("/api/latest", get(latest))
        .route("/api/push", post(web_push))
        .route("/api/events", get(web_events))
        .route("/api/forget", post(web_forget))
        .route("/api/m/push", post(member_push))
        .route("/api/m/events", get(member_events))
        .layer(DefaultBodyLimit::max(max_upload))
        .layer(middleware::from_fn(local_only))
        .with_state(host)
}

/// 只接受局域网地址来的连接。
async fn local_only(req: Request, next: Next) -> Response {
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    match ip {
        Some(ip) if discovery::is_local(ip) => next.run(req).await,
        _ => StatusCode::FORBIDDEN.into_response(),
    }
}

async fn page(State(host): State<Shared>) -> Response {
    if !host.backend.info().web_enabled {
        return (StatusCode::FORBIDDEN, "网页访问已关闭").into_response();
    }
    let mut res = Html(WEB_PAGE).into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

async fn info(State(host): State<Shared>) -> Json<serde_json::Value> {
    let i = host.backend.info();
    Json(serde_json::json!({
        "deviceId": i.device_id,
        "name": i.name,
        "platform": i.platform,
        "web": i.web_enabled,
        "v": 1,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairRequest {
    code: String,
    name: String,
    #[serde(default)]
    platform: String,
    /// desktop | web
    kind: String,
    #[serde(default)]
    device_id: Option<String>,
    /// 电脑：一次性 X25519 公钥（base64）
    #[serde(default)]
    public_key: Option<String>,
}

pub fn normalize_code(code: &str) -> String {
    code.chars().filter(char::is_ascii_digit).collect()
}

async fn pair_request(
    State(host): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<PairRequest>,
) -> Response {
    let info = host.backend.info();
    if host.blocked(addr.ip()) {
        return (StatusCode::TOO_MANY_REQUESTS, "尝试次数太多，稍后再试").into_response();
    }
    if normalize_code(&req.code) != info.code {
        host.strike(addr.ip());
        return (StatusCode::FORBIDDEN, "设备码不对").into_response();
    }
    let kind = if req.kind == "desktop" {
        "desktop"
    } else {
        "web"
    };
    if kind == "web" && !info.web_enabled {
        return (StatusCode::FORBIDDEN, "主机关闭了网页访问").into_response();
    }
    let name: String = req.name.trim().chars().take(40).collect();
    let name = if name.is_empty() {
        "未命名设备".to_string()
    } else {
        name
    };
    let device_id = req
        .device_id
        .filter(|id| kind == "desktop" && (8..=64).contains(&id.len()) && id != &info.device_id)
        .unwrap_or_else(|| format!("web-{}", uuid::Uuid::new_v4().simple()));

    let (session_key, sas, host_pub) = if kind == "desktop" {
        let Some(member_pub) = req.public_key.and_then(|k| B64.decode(k).ok()) else {
            return (StatusCode::BAD_REQUEST, "缺少公钥").into_response();
        };
        let hs = Handshake::new();
        let host_pub = hs.public();
        let transcript = crypto::pair_transcript(&member_pub, &host_pub, &info.code);
        match hs.finish(&member_pub, &transcript) {
            Ok(s) => (Some(s.key), Some(s.sas), Some(B64.encode(host_pub))),
            Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        }
    } else {
        (None, None, None)
    };

    let id = uuid::Uuid::new_v4().simple().to_string();
    {
        let mut map = host.pending.lock();
        map.retain(|_, p| p.created.elapsed() < PAIR_TTL);
        if map.len() >= MAX_PENDING {
            return (StatusCode::TOO_MANY_REQUESTS, "申请太多，稍后再试").into_response();
        }
        map.insert(
            id.clone(),
            Pending {
                view: PairRequestView {
                    id: id.clone(),
                    name,
                    platform: req.platform.chars().take(16).collect(),
                    kind: kind.into(),
                    sas: sas.clone(),
                    address: addr.ip().to_string(),
                },
                device_id,
                session_key,
                state: PendState::Waiting,
                notify: Arc::new(Notify::new()),
                created: Instant::now(),
            },
        );
    }
    host.backend.pair_requested();
    Json(serde_json::json!({
        "id": id,
        "hostId": info.device_id,
        "hostName": info.name,
        "hostPublicKey": host_pub,
        "sas": sas,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct WaitQuery {
    id: String,
}

/// 长轮询等主机同意：最多挂 25 秒，没结果就返回 pending 让对方再来。
async fn pair_wait(State(host): State<Shared>, Query(q): Query<WaitQuery>) -> Response {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let notify = {
            let mut map = host.pending.lock();
            let Some(p) = map.get(&q.id) else {
                return Json(serde_json::json!({ "status": "expired" })).into_response();
            };
            match &p.state {
                PendState::Waiting if p.created.elapsed() >= PAIR_TTL => {
                    map.remove(&q.id);
                    return Json(serde_json::json!({ "status": "expired" })).into_response();
                }
                PendState::Waiting => p.notify.clone(),
                PendState::Denied => {
                    map.remove(&q.id);
                    return Json(serde_json::json!({ "status": "denied" })).into_response();
                }
                PendState::Approved(payload, token) => {
                    let mut body = payload.clone();
                    body["status"] = "approved".into();
                    let token = token.clone();
                    map.remove(&q.id);
                    let mut res = Json(body).into_response();
                    if let Some(token) = token {
                        if let Ok(v) = HeaderValue::from_str(&format!(
                            "{COOKIE}={token}; Path=/; Max-Age=315360000; HttpOnly; SameSite=Strict"
                        )) {
                            res.headers_mut().insert(header::SET_COOKIE, v);
                        }
                    }
                    return res;
                }
            }
        };
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            return Json(serde_json::json!({ "status": "pending" })).into_response();
        };
        let _ = tokio::time::timeout(left, notify.notified()).await;
    }
}

async fn me(State(host): State<Shared>, headers: HeaderMap) -> Response {
    match host.web_auth(&headers) {
        Ok(peer) => Json(serde_json::json!({
            "deviceId": peer.device_id,
            "name": peer.name,
            "hostName": host.backend.info().name,
        }))
        .into_response(),
        Err(code) => code.into_response(),
    }
}

#[derive(Deserialize)]
struct ItemsQuery {
    limit: Option<u32>,
}

async fn items(
    State(host): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<ItemsQuery>,
) -> Response {
    let peer = match host.web_auth(&headers) {
        Ok(p) => p,
        Err(code) => return code.into_response(),
    };
    host.backend.seen(&peer.device_id, addr);
    let limit = q.limit.unwrap_or(60).clamp(1, 200);
    Json(host.backend.recent(limit)).into_response()
}

fn content_response(content: Option<Content>) -> Response {
    match content {
        Some(Content::Text(text)) => {
            ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
        }
        Some(Content::Image(bytes, ct)) => (
            [
                (header::CONTENT_TYPE, ct),
                (header::CACHE_CONTROL, "private, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn item_text(
    State(host): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    if let Err(code) = host.web_auth(&headers) {
        return code.into_response();
    }
    match host.backend.content(id, false) {
        Some(Content::Text(t)) => content_response(Some(Content::Text(t))),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct ImageQuery {
    thumb: Option<u8>,
}

async fn item_image(
    State(host): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(q): Query<ImageQuery>,
) -> Response {
    if let Err(code) = host.web_auth(&headers) {
        return code.into_response();
    }
    match host.backend.content(id, q.thumb == Some(1)) {
        Some(Content::Image(b, ct)) => content_response(Some(Content::Image(b, ct))),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// 快捷指令"从电脑取剪贴板"：最新一条，文字直接返回文字，图片返回图片。
async fn latest(State(host): State<Shared>, headers: HeaderMap) -> Response {
    if let Err(code) = host.web_auth(&headers) {
        return code.into_response();
    }
    content_response(host.backend.latest())
}

fn sniff_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G'])
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"GIF8")
        || (bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
        || bytes.starts_with(b"BM")
}

/// 手机发上来：文字（text/plain）或图片（image/*）。快捷指令把剪贴板当"文件"发，
/// 类型不一定标对，按内容再认一次。
async fn web_push(
    State(host): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let peer = match host.web_auth(&headers) {
        Ok(p) => p,
        Err(code) => return code.into_response(),
    };
    host.backend.seen(&peer.device_id, addr);
    if body.is_empty() {
        return (StatusCode::BAD_REQUEST, "内容是空的").into_response();
    }
    let ct = header_str(&headers, "content-type").unwrap_or_default();
    let now = now_ms();
    let mut record = SyncRecord {
        id: uuid::Uuid::new_v4().to_string(),
        origin: peer.device_id.clone(),
        origin_name: peer.name.clone(),
        created_at: now,
        sent_at: now,
        ..Default::default()
    };
    if ct.starts_with("image/") || sniff_image(&body) {
        if body.len() > host.backend.info().max_upload {
            return (StatusCode::PAYLOAD_TOO_LARGE, "图片太大").into_response();
        }
        record.kind = "image".into();
        record.image = Some(body.to_vec());
    } else {
        let Ok(text) = String::from_utf8(body.to_vec()) else {
            return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "只支持文字和图片").into_response();
        };
        if text.trim().is_empty() {
            return (StatusCode::BAD_REQUEST, "内容是空的").into_response();
        }
        record.kind = "text".into();
        record.text = Some(text);
    }
    let backend = host.backend.clone();
    let from = peer.device_id.clone();
    // 入库、解码图片、写剪贴板都是同步的慢活，别占着网络线程
    let _ = tokio::task::spawn_blocking(move || backend.received(record, &from)).await;
    StatusCode::NO_CONTENT.into_response()
}

fn sse_stream(
    rx: mpsc::Receiver<Event>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|ev| (Ok(ev), rx))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
}

async fn web_events(
    State(host): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let peer = match host.web_auth(&headers) {
        Ok(p) => p,
        Err(code) => return code.into_response(),
    };
    host.backend.seen(&peer.device_id, addr);
    let (tx, rx) = mpsc::channel(16);
    let _ = tx.try_send(Event::default().event("hello").data("1"));
    host.web.lock().push((peer.device_id, tx));
    sse_stream(rx).into_response()
}

async fn web_forget(State(host): State<Shared>, headers: HeaderMap) -> Response {
    let peer = match host.web_auth(&headers) {
        Ok(p) => p,
        Err(code) => return code.into_response(),
    };
    host.forget(&peer.device_id);
    host.backend.remove_member(&peer.device_id);
    let mut res = StatusCode::NO_CONTENT.into_response();
    res.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("dato_token=; Path=/; Max-Age=0"),
    );
    res
}

async fn member_push(
    State(host): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (device, key) = match host.desktop_auth(&headers) {
        Ok(v) => v,
        Err(code) => return code.into_response(),
    };
    host.backend.seen(&device, addr);
    let record = match crypto::open(&key, &body, AAD_RECORD).and_then(|b| SyncRecord::decode(&b)) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let backend = host.backend.clone();
    let _ = tokio::task::spawn_blocking(move || backend.received(record, &device)).await;
    StatusCode::NO_CONTENT.into_response()
}

async fn member_events(
    State(host): State<Shared>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let (device, _) = match host.desktop_auth(&headers) {
        Ok(v) => v,
        Err(code) => return code.into_response(),
    };
    host.backend.seen(&device, addr);
    let (tx, rx) = mpsc::channel(32);
    let _ = tx.try_send(
        Event::default()
            .event("hello")
            .data(host.backend.info().name),
    );
    host.desktop.lock().entry(device).or_default().push(tx);
    sse_stream(rx).into_response()
}

/// 跑起来的主机服务。drop 时停服务。
pub struct Server {
    pub host: Arc<Host>,
    pub port: u16,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Server {
    /// 在端口上起服务。先同步地绑端口，端口被占用时立刻报错。
    /// `bind` 平时是 0.0.0.0；测试用 127.0.0.1（只绑回环不会触发 Windows 防火墙的询问）。
    pub fn start(backend: Arc<dyn Backend>, bind: IpAddr, port: u16) -> AppResult<Self> {
        let listener = std::net::TcpListener::bind((bind, port)).map_err(|e| {
            AppError::msg(format!("端口 {port} 打不开（可能被别的程序占用了）：{e}"))
        })?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let host = Host::new(backend);
        let app = router(host.clone());
        let (tx, rx) = oneshot::channel::<()>();
        tauri::async_runtime::spawn(async move {
            let listener = match tokio::net::TcpListener::from_std(listener) {
                Ok(l) => l,
                Err(err) => {
                    tracing::error!("同步服务启动失败：{err}");
                    return;
                }
            };
            let serve = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = rx.await;
            });
            if let Err(err) = serve.await {
                tracing::warn!("同步服务退出：{err}");
            }
        });
        tracing::info!(port, "局域网同步服务已启动");
        Ok(Self {
            host,
            port,
            shutdown: Some(tx),
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.host.disconnect_all();
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        tracing::info!("局域网同步服务已停止");
    }
}
