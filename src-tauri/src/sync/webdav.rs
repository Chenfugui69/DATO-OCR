//! WebDAV 网盘同步（"账号同步"）：坚果云，或者用 Alist / CloudDrive2 之类挂成 WebDAV 的 115 等网盘。
//!
//! 网盘上的布局（`{文件夹}` 默认 `DATO-OCR`）：
//!
//! ```text
//! {文件夹}/group.json                      盐、Argon2 参数、校验块（盐不是秘密）
//! {文件夹}/items/{发出时间}-{设备}-{记录}.dcr  一条记录，用同步密码派生的密钥加密
//! {文件夹}/devices/{设备ID}.dcr             设备名、平台、最近在线时间（加密）
//! ```
//!
//! 网盘只看得到密文。各设备定时列 `items/`，没处理过的文件下载下来解密入库；
//! 自己发的文件只留最近 24 小时 / 100 个，删掉更早的，免得目录越来越大、列一次越来越慢。

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use parking_lot::Mutex;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Notify};

use crate::error::{AppError, AppResult};
use crate::storage::now_ms;
use crate::sync::crypto::{self, KdfParams, SecretKey};
use crate::sync::record::SyncRecord;

const AAD_RECORD: &[u8] = b"dato-cor/webdav/record";
const AAD_CHECK: &[u8] = b"dato-cor/webdav/check";
const AAD_DEVICE: &[u8] = b"dato-cor/webdav/device";
/// 自己发的文件保留多久、最多几个
const KEEP_OWN_MS: i64 = 24 * 3600 * 1000;
const KEEP_OWN_MAX: usize = 100;
/// 别的设备不在了，它留下的文件最多留多久
const KEEP_ANY_MS: i64 = 7 * 24 * 3600 * 1000;
/// 新设备第一次同步，最多导入多少条、多久以内的
const FIRST_IMPORT: usize = 10;
const FIRST_IMPORT_MS: i64 = 24 * 3600 * 1000;
/// 发出时间和本机时间差多少以内算"刚复制的"（各设备时钟可能有偏差）
const FRESH_MS: i64 = 5 * 60 * 1000;

/// 网盘连接参数。
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub url: String,
    pub user: String,
    pub password: String,
    pub folder: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Group {
    v: u32,
    salt: String,
    kdf: KdfParams,
    check: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DavDevice {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub last_seen: i64,
}

pub struct Dav {
    client: reqwest::Client,
    /// 账号的 WebDAV 根地址
    root: url::Url,
    /// 同步文件夹，各级目录名
    folder: Vec<String>,
    base: url::Url,
    user: String,
    password: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

impl Dav {
    pub fn new(client: reqwest::Client, account: &Account) -> AppResult<Self> {
        let mut base = url::Url::parse(account.url.trim()).map_err(|_| {
            AppError::msg("WebDAV 地址格式不对，应该像 https://dav.jianguoyun.com/dav/")
        })?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(AppError::msg("WebDAV 地址要以 http:// 或 https:// 开头"));
        }
        // 统一成以 / 结尾，后面按路径段往上拼
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        let root = base.clone();
        let folder: Vec<String> = account
            .folder
            .split(['/', '\\'])
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if let Ok(mut segs) = base.path_segments_mut() {
            segs.pop_if_empty();
            for s in &folder {
                segs.push(s);
            }
            segs.push("");
        }
        Ok(Self {
            client,
            root,
            folder,
            base,
            user: account.user.clone(),
            password: account.password.clone(),
        })
    }

    /// `path` 相对同步文件夹，比如 `items/` 或 `group.json`。
    fn url(&self, path: &str) -> url::Url {
        let mut u = self.base.clone();
        if let Ok(mut segs) = u.path_segments_mut() {
            segs.pop_if_empty();
            for s in path.split('/') {
                segs.push(s);
            }
        }
        u
    }

    fn req(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, self.url(path))
            .basic_auth(&self.user, Some(&self.password))
    }

    fn check(status: StatusCode, what: &str) -> AppResult<()> {
        match status {
            s if s.is_success() => Ok(()),
            StatusCode::UNAUTHORIZED => Err(AppError::Network(
                "WebDAV 账号或密码不对（坚果云要用「第三方应用管理」里生成的应用密码）".into(),
            )),
            StatusCode::FORBIDDEN => Err(AppError::Network(format!("{what}：没有权限"))),
            StatusCode::INSUFFICIENT_STORAGE => Err(AppError::Network("网盘空间不够了".into())),
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => Err(
                AppError::Network("网盘请求太频繁被限流了，把同步间隔调大一点".into()),
            ),
            s => Err(AppError::Network(format!(
                "{what}失败：HTTP {}",
                s.as_u16()
            ))),
        }
    }

    pub async fn list(&self, path: &str) -> AppResult<Option<Vec<Entry>>> {
        let res = self
            .req(dav_method(b"PROPFIND")?, path)
            .header("Depth", "1")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(
                r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#,
            )
            .send()
            .await?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::check(res.status(), "列目录")?;
        let xml = res.text().await?;
        let own = self.url(path);
        let own = own.path().trim_end_matches('/');
        Ok(Some(
            parse_multistatus(&xml)
                .into_iter()
                .filter(|(href, _)| {
                    let p = href_path(href);
                    p.trim_end_matches('/') != own
                })
                .filter_map(|(href, is_dir)| {
                    let name = last_segment(&href)?;
                    Some(Entry { name, is_dir })
                })
                .collect(),
        ))
    }

    pub async fn mkcol(&self, path: &str) -> AppResult<()> {
        let res = self.req(dav_method(b"MKCOL")?, path).send().await?;
        // 405 = 已经存在
        if res.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        Self::check(res.status(), "建文件夹")
    }

    pub async fn put(&self, path: &str, body: Vec<u8>) -> AppResult<()> {
        let res = self.req(Method::PUT, path).body(body).send().await?;
        Self::check(res.status(), "上传")
    }

    pub async fn get(&self, path: &str) -> AppResult<Option<Vec<u8>>> {
        let res = self.req(Method::GET, path).send().await?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::check(res.status(), "下载")?;
        Ok(Some(res.bytes().await?.to_vec()))
    }

    pub async fn delete(&self, path: &str) -> AppResult<()> {
        let res = self.req(Method::DELETE, path).send().await?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Self::check(res.status(), "删除")
    }

    /// 连上网盘：检查账号、建好文件夹、用同步密码拿到内容密钥。
    /// 网盘上还没有同步数据就新建一组（第一台设备），有的话校验同步密码。
    pub async fn open_group(&self, sync_password: &str, kdf: KdfParams) -> AppResult<SecretKey> {
        if sync_password.chars().count() < 6 {
            return Err(AppError::msg("同步密码至少 6 位"));
        }
        // 先确认根地址能访问（账号密码对不对）
        let probe = Dav {
            client: self.client.clone(),
            root: self.root.clone(),
            folder: Vec::new(),
            base: self.root.clone(),
            user: self.user.clone(),
            password: self.password.clone(),
        };
        if probe.list("").await?.is_none() {
            return Err(AppError::Network("WebDAV 地址不存在，检查一下地址".into()));
        }
        // 同步文件夹一级一级建（MKCOL 不会自动建上级）
        for depth in 1..=self.folder.len() {
            probe.mkcol(&self.folder[..depth].join("/")).await?;
        }
        self.mkcol("items").await?;
        self.mkcol("devices").await?;
        if let Some(bytes) = self.get("group.json").await? {
            let group: Group = serde_json::from_slice(&bytes)
                .map_err(|_| AppError::msg("网盘上的 group.json 格式不对"))?;
            let salt = b64()
                .decode(&group.salt)
                .map_err(|_| AppError::msg("盐格式不对"))?;
            let password = sync_password.to_string();
            let key = tokio::task::spawn_blocking(move || {
                crypto::password_key(&password, &salt, group.kdf)
            })
            .await
            .map_err(|e| AppError::msg(e.to_string()))??;
            let check = b64()
                .decode(&group.check)
                .map_err(|_| AppError::msg("校验块格式不对"))?;
            crypto::open(&key, &check, AAD_CHECK)
                .map_err(|_| AppError::msg("同步密码不对（要和其他设备上设的一样）"))?;
            return Ok(key);
        }
        let salt = crypto::random_bytes(16);
        let password = sync_password.to_string();
        let salt2 = salt.clone();
        let key = tokio::task::spawn_blocking(move || crypto::password_key(&password, &salt2, kdf))
            .await
            .map_err(|e| AppError::msg(e.to_string()))??;
        let group = Group {
            v: 1,
            salt: b64().encode(&salt),
            kdf,
            check: b64().encode(crypto::seal(&key, b"dato-cor", AAD_CHECK)),
        };
        self.put("group.json", serde_json::to_vec_pretty(&group)?)
            .await?;
        Ok(key)
    }
}

fn dav_method(name: &'static [u8]) -> AppResult<Method> {
    Method::from_bytes(name).map_err(|e| AppError::msg(e.to_string()))
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// 从 PROPFIND 的 multistatus 里取出 (href, 是不是文件夹)。不同服务器的命名空间前缀各不相同
/// （`d:` `D:` `lp1:` 或者没有前缀），这里只认标签的本地名。
pub fn parse_multistatus(xml: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = find_open(rest, "response") {
        let after = &rest[start..];
        let end = find_close(after, "response").unwrap_or(after.len());
        let block = &after[..end];
        if let Some(href) = tag_text(block, "href") {
            let is_dir = find_open(block, "collection").is_some();
            out.push((href, is_dir));
        }
        rest = &after[end.max(1)..];
    }
    out
}

fn tag_name(s: &str) -> &str {
    let end = s
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(s.len());
    &s[..end]
}

fn local_is(name: &str, local: &str) -> bool {
    name == local || name.rsplit(':').next() == Some(local) && name.contains(':')
}

fn find_open(s: &str, local: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = s[from..].find('<') {
        let at = from + i;
        let name = tag_name(&s[at + 1..]);
        if !name.starts_with('/') && local_is(name, local) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

fn find_close(s: &str, local: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = s[from..].find("</") {
        let at = from + i;
        let name = tag_name(&s[at + 2..]);
        if local_is(name, local) {
            return s[at..].find('>').map(|e| at + e + 1);
        }
        from = at + 2;
    }
    None
}

fn tag_text(block: &str, local: &str) -> Option<String> {
    let start = find_open(block, local)?;
    let open_end = start + block[start..].find('>')? + 1;
    let close = open_end + block[open_end..].find('<')?;
    Some(
        block[open_end..close]
            .trim()
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'"),
    )
}

fn href_path(href: &str) -> String {
    // href 可能是完整 URL，也可能只是路径
    url::Url::parse(href)
        .map(|u| u.path().to_string())
        .unwrap_or_else(|_| href.to_string())
}

fn last_segment(href: &str) -> Option<String> {
    let path = href_path(href);
    let seg = path.trim_end_matches('/').rsplit('/').next()?;
    if seg.is_empty() {
        return None;
    }
    Some(percent_decode(seg))
}

fn percent_decode(s: &str) -> String {
    let hex = |b: u8| (b as char).to_digit(16).map(|v| v as u8);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 文件名：`{发出时间 13 位}-{设备前 12 位}-{记录前 12 位}.dcr`
pub fn item_name(record: &SyncRecord, sender: &str) -> String {
    let short = |s: &str| -> String {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect()
    };
    format!(
        "{:013}-{}-{}.dcr",
        record.sent_at.max(0),
        short(sender),
        short(&record.id)
    )
}

pub fn parse_item_name(name: &str) -> Option<(i64, String)> {
    let stem = name.strip_suffix(".dcr")?;
    let mut parts = stem.splitn(3, '-');
    let ts = parts.next()?.parse().ok()?;
    let sender = parts.next()?.to_string();
    parts.next()?;
    Some((ts, sender))
}

fn short_id(id: &str) -> String {
    id.chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect()
}

/// 一轮轮询要做的事：哪些文件是新的（按时间排好），哪些自己的旧文件该删。
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub fetch: Vec<String>,
    pub delete: Vec<String>,
}

pub fn plan(names: &[String], seen: &HashSet<String>, my_id: &str, now: i64, first: bool) -> Plan {
    let me = short_id(my_id);
    let mut items: Vec<(i64, String, &String)> = names
        .iter()
        .filter_map(|n| parse_item_name(n).map(|(ts, sender)| (ts, sender, n)))
        .collect();
    items.sort();
    let mut plan = Plan::default();
    let own: Vec<&(i64, String, &String)> = items.iter().filter(|(_, s, _)| *s == me).collect();
    let own_overflow = own.len().saturating_sub(KEEP_OWN_MAX);
    for (i, (ts, _, name)) in own.iter().enumerate() {
        if i < own_overflow || now - ts > KEEP_OWN_MS {
            plan.delete.push((*name).clone());
        }
    }
    for (ts, sender, name) in &items {
        if *sender != me && now - ts > KEEP_ANY_MS {
            plan.delete.push((*name).clone());
        }
    }
    let fresh: Vec<&String> = items
        .iter()
        .filter(|(ts, sender, name)| {
            *sender != me && !seen.contains(*name) && now - ts <= KEEP_ANY_MS
        })
        .map(|(_, _, n)| *n)
        .collect();
    plan.fetch = if first {
        // 新设备：只要最近一天里的最后几条，别把一周的历史全倒进来
        items
            .iter()
            .filter(|(ts, sender, _)| *sender != me && now - ts <= FIRST_IMPORT_MS)
            .map(|(_, _, n)| (*n).clone())
            .rev()
            .take(FIRST_IMPORT)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        fresh.into_iter().cloned().collect()
    };
    plan
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebdavStatus {
    pub running: bool,
    pub last_sync: Option<i64>,
    pub error: Option<String>,
}

/// 收到的记录交给应用：`live` = 这是刚复制的，可以直接放进剪贴板。
pub type OnRecord = Arc<dyn Fn(SyncRecord, bool) + Send + Sync>;
/// 已处理文件名的读写（存在数据库 kv 里，重启后不重复处理）
pub type SeenStore = Arc<dyn Fn(Option<&HashSet<String>>) -> Option<HashSet<String>> + Send + Sync>;

pub struct Config {
    pub account: Account,
    pub key: SecretKey,
    pub my_id: String,
    pub my_name: String,
    pub platform: String,
    pub interval: Duration,
}

/// 跑着的网盘同步。drop 时停。
pub struct Runner {
    outbox: Arc<Mutex<VecDeque<SyncRecord>>>,
    wake: Arc<Notify>,
    stop: watch::Sender<bool>,
    pub status: Arc<Mutex<WebdavStatus>>,
}

impl Runner {
    pub fn start(
        client: reqwest::Client,
        config: Config,
        on_record: OnRecord,
        seen: SeenStore,
    ) -> AppResult<Self> {
        let dav = Dav::new(client, &config.account)?;
        let outbox = Arc::new(Mutex::new(VecDeque::new()));
        let wake = Arc::new(Notify::new());
        let (stop, stop_rx) = watch::channel(false);
        let status = Arc::new(Mutex::new(WebdavStatus {
            running: true,
            ..Default::default()
        }));
        let worker = Worker {
            dav,
            config,
            outbox: outbox.clone(),
            wake: wake.clone(),
            status: status.clone(),
            on_record,
            seen_store: seen,
        };
        tauri::async_runtime::spawn(worker.run(stop_rx));
        Ok(Self {
            outbox,
            wake,
            stop,
            status,
        })
    }

    pub fn push(&self, record: SyncRecord) {
        let mut q = self.outbox.lock();
        if q.len() >= 50 {
            q.pop_front();
        }
        q.push_back(record);
        drop(q);
        self.wake.notify_one();
    }

    /// 马上同步一次
    pub fn poke(&self) {
        self.wake.notify_one();
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

struct Worker {
    dav: Dav,
    config: Config,
    outbox: Arc<Mutex<VecDeque<SyncRecord>>>,
    wake: Arc<Notify>,
    status: Arc<Mutex<WebdavStatus>>,
    on_record: OnRecord,
    seen_store: SeenStore,
}

impl Worker {
    async fn run(self, mut stop: watch::Receiver<bool>) {
        let mut seen = (self.seen_store)(None);
        let mut last_device_note = 0i64;
        loop {
            if *stop.borrow() {
                break;
            }
            let result = self.tick(&mut seen, &mut last_device_note).await;
            {
                let mut s = self.status.lock();
                match result {
                    Ok(()) => {
                        s.last_sync = Some(now_ms());
                        s.error = None;
                    }
                    Err(err) => {
                        tracing::warn!("WebDAV 同步失败：{err}");
                        s.error = Some(err.to_string());
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(self.config.interval) => {}
                _ = self.wake.notified() => {}
                _ = stop.changed() => {}
            }
        }
        self.status.lock().running = false;
    }

    async fn tick(&self, seen: &mut Option<HashSet<String>>, last_note: &mut i64) -> AppResult<()> {
        // 先把本机的发出去
        loop {
            let Some(record) = self.outbox.lock().front().cloned() else {
                break;
            };
            let name = item_name(&record, &self.config.my_id);
            let body = crypto::seal(&self.config.key, &record.encode(), AAD_RECORD);
            self.dav.put(&format!("items/{name}"), body).await?;
            self.outbox.lock().pop_front();
            seen.get_or_insert_with(HashSet::new).insert(name);
        }

        let now = now_ms();
        if now - *last_note > 3600 * 1000 {
            let note = DavDevice {
                device_id: self.config.my_id.clone(),
                name: self.config.my_name.clone(),
                platform: self.config.platform.clone(),
                last_seen: now,
            };
            let body = crypto::seal(&self.config.key, &serde_json::to_vec(&note)?, AAD_DEVICE);
            self.dav
                .put(
                    &format!("devices/{}.dcr", short_id(&self.config.my_id)),
                    body,
                )
                .await?;
            *last_note = now;
        }

        let names: Vec<String> = self
            .dav
            .list("items")
            .await?
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.name)
            .collect();
        let first = seen.is_none();
        let known = seen.clone().unwrap_or_default();
        let plan = plan(&names, &known, &self.config.my_id, now, first);
        let mut next: HashSet<String> = known;
        let last = plan.fetch.last().cloned();
        for name in &plan.fetch {
            next.insert(name.clone());
            let Some(bytes) = self.dav.get(&format!("items/{name}")).await? else {
                continue;
            };
            let record = match crypto::open(&self.config.key, &bytes, AAD_RECORD)
                .and_then(|b| SyncRecord::decode(&b))
            {
                Ok(r) => r,
                Err(err) => {
                    tracing::warn!(name, "网盘上的记录解不开：{err}");
                    continue;
                }
            };
            let live =
                !first && Some(name) == last.as_ref() && (now - record.sent_at).abs() <= FRESH_MS;
            let cb = self.on_record.clone();
            let _ = tokio::task::spawn_blocking(move || cb(record, live)).await;
        }
        for name in &plan.delete {
            if let Err(err) = self.dav.delete(&format!("items/{name}")).await {
                tracing::debug!(name, "删旧文件失败：{err}");
            }
        }
        // 只记还在网盘上的，集合不会越长越大
        let present: HashSet<&String> = names.iter().collect();
        next.retain(|n| present.contains(n));
        if first || next != *seen.as_ref().unwrap_or(&HashSet::new()) {
            (self.seen_store)(Some(&next));
        }
        *seen = Some(next);
        Ok(())
    }
}

/// 网盘上登记过的设备（设置页显示用）。
pub async fn devices(
    client: reqwest::Client,
    account: &Account,
    key: &SecretKey,
) -> AppResult<Vec<DavDevice>> {
    let dav = Dav::new(client, account)?;
    let mut out = Vec::new();
    for entry in dav.list("devices").await?.unwrap_or_default() {
        if entry.is_dir || !entry.name.ends_with(".dcr") {
            continue;
        }
        let Some(bytes) = dav.get(&format!("devices/{}", entry.name)).await? else {
            continue;
        };
        if let Ok(plain) = crypto::open(key, &bytes, AAD_DEVICE) {
            if let Ok(d) = serde_json::from_slice::<DavDevice>(&plain) {
                out.push(d);
            }
        }
    }
    out.sort_by_key(|d| -d.last_seen);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multistatus_with_various_prefixes() {
        let jianguo = r#"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/DATO-COR/items/</d:href>
<d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>
<d:response><d:href>/dav/DATO-COR/items/0001700000000000-abc-def.dcr</d:href>
<d:propstat><d:prop><d:resourcetype/></d:prop></d:propstat></d:response></d:multistatus>"#;
        let r = parse_multistatus(jianguo);
        assert_eq!(r.len(), 2);
        assert!(r[0].1);
        assert!(!r[1].1);
        assert_eq!(
            last_segment(&r[1].0).as_deref(),
            Some("0001700000000000-abc-def.dcr")
        );

        let apache = r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:lp1="DAV:">
<D:response><D:href>http://nas.local/dav/%E5%89%AA%E8%B4%B4/</D:href><D:propstat><D:prop>
<lp1:resourcetype><D:collection/></lp1:resourcetype></D:prop></D:propstat></D:response></D:multistatus>"#;
        let r = parse_multistatus(apache);
        assert_eq!(r.len(), 1);
        assert!(r[0].1);
        assert_eq!(last_segment(&r[0].0).as_deref(), Some("剪贴"));

        let bare = r#"<multistatus xmlns="DAV:"><response><href>/a/b.dcr</href><propstat><prop><resourcetype/></prop></propstat></response></multistatus>"#;
        assert_eq!(
            parse_multistatus(bare),
            vec![("/a/b.dcr".to_string(), false)]
        );
    }

    #[test]
    fn item_names_roundtrip() {
        let r = SyncRecord {
            id: "1b2c3d4e-5f60-7182-93a4-b5c6d7e8f900".into(),
            sent_at: 1_760_000_000_123,
            ..Default::default()
        };
        let name = item_name(&r, "0123456789abcdef0123");
        assert_eq!(name, "1760000000123-0123456789ab-1b2c3d4e5f60.dcr");
        assert_eq!(
            parse_item_name(&name),
            Some((1_760_000_000_123, "0123456789ab".to_string()))
        );
        assert_eq!(parse_item_name("group.json"), None);
    }

    #[test]
    fn plan_fetches_new_others_and_prunes_own() {
        let me = "aaaaaaaaaaaa";
        let now = 10 * 24 * 3600 * 1000;
        let n = |ts: i64, who: &str, id: &str| format!("{ts:013}-{who}-{id}.dcr");
        let names = vec![
            n(now - 1000, "bbbbbbbbbbbb", "x1"),
            n(now - 2000, "bbbbbbbbbbbb", "x0"),
            n(now - 500, me, "m1"),
            n(now - KEEP_OWN_MS - 1, me, "m0"),
            n(now - KEEP_ANY_MS - 1, "cccccccccccc", "old"),
        ];
        let seen: HashSet<String> = [names[1].clone()].into_iter().collect();
        let p = plan(&names, &seen, me, now, false);
        assert_eq!(p.fetch, vec![names[0].clone()]);
        assert!(p.delete.contains(&names[3]));
        assert!(p.delete.contains(&names[4]));
        assert!(!p.delete.contains(&names[2]));

        // 第一次：只拿最近一天里别人的，按时间顺序
        let p = plan(&names, &HashSet::new(), me, now, true);
        assert_eq!(p.fetch, vec![names[1].clone(), names[0].clone()]);
    }

    #[test]
    fn own_files_capped() {
        let me = "aaaaaaaaaaaa";
        let now = 1_000_000_000;
        let names: Vec<String> = (0..KEEP_OWN_MAX + 3)
            .map(|i| format!("{:013}-{me}-{i}.dcr", now - 1000 + i as i64))
            .collect();
        let p = plan(&names, &HashSet::new(), me, now, false);
        assert_eq!(p.delete.len(), 3);
        assert_eq!(p.delete[0], names[0]);
        assert!(p.fetch.is_empty());
    }
}
