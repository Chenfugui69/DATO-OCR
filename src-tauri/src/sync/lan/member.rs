//! 本机加入了另一台电脑（主机）：配对、保持一条 SSE 长连接收记录、把本机的记录推上去。
//!
//! 主机的地址会变（DHCP），连不上时用 mDNS 按主机的设备 ID 重新找。

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Notify};

use super::discovery;
use super::host::{auth_header, AAD_MEMBER_KEY, AAD_RECORD};
use crate::error::{AppError, AppResult};
use crate::sync::crypto::{self, Handshake, SecretKey};
use crate::sync::record::SyncRecord;

/// 本机作为成员保存的凭据。
#[derive(Clone)]
pub struct Credentials {
    pub my_id: String,
    pub host_id: String,
    pub host_name: String,
    pub key: SecretKey,
    pub address: Option<SocketAddr>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberStatus {
    pub online: bool,
    pub address: Option<String>,
    pub error: Option<String>,
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        // 局域网直连，绝不能走代理
        .no_proxy()
        .connect_timeout(Duration::from_secs(3))
        .build()
        .unwrap_or_default()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairResponse {
    id: String,
    host_id: String,
    host_name: String,
    host_public_key: Option<String>,
    sas: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WaitResponse {
    status: String,
    sealed_key: Option<String>,
}

/// 在局域网里按设备码找主机，返回能连上的地址。
pub async fn find_host(code: &str) -> AppResult<(SocketAddr, String)> {
    let code = code.to_string();
    let found = tokio::task::spawn_blocking(move || {
        discovery::browse(Duration::from_secs(5), |f| f.code == code)
    })
    .await
    .map_err(|e| AppError::msg(e.to_string()))??;
    let client = http_client();
    for f in found {
        if let Some(addr) = reachable(&client, &f).await {
            return Ok((addr, f.device_id));
        }
    }
    Err(AppError::msg(
        "局域网里没找到这个设备码。确认两台设备连着同一个网络、主机打开了局域网同步；也可以手动填主机地址",
    ))
}

/// 按设备 ID 重新找主机的地址。
async fn rediscover(host_id: &str) -> Option<SocketAddr> {
    let id = host_id.to_string();
    let found = tokio::task::spawn_blocking(move || {
        discovery::browse(Duration::from_secs(4), |f| f.device_id == id)
    })
    .await
    .ok()?
    .ok()?;
    let client = http_client();
    for f in found.into_iter().filter(|f| f.device_id == host_id) {
        if let Some(addr) = reachable(&client, &f).await {
            return Some(addr);
        }
    }
    None
}

/// 广播里的地址挨个试，返回第一个连得上、而且应答的确实是这台设备的。
///
/// 广播里会带上代理软件 TUN 模式的虚拟地址（198.18.0.1 之类）。本机也开着同样的代理时，
/// 这个地址在本机指向的是**本机自己**：连上去应答的是本机的同步服务，配对时就报"设备码不对"。
/// 所以本机自己的地址直接跳过，应答的设备 ID 也要核对。
async fn reachable(client: &reqwest::Client, found: &discovery::Found) -> Option<SocketAddr> {
    let mine = discovery::own_ips();
    for &addr in &found.addrs {
        if mine.contains(&addr.ip()) {
            continue;
        }
        if probe(client, addr).await.as_deref() == Some(found.device_id.as_str()) {
            return Some(addr);
        }
    }
    None
}

/// 问一下这个地址上的 DATO OCR 是哪台设备（设备 ID）；连不上返回 None。
pub async fn probe(client: &reqwest::Client, addr: SocketAddr) -> Option<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Info {
        device_id: String,
    }
    let resp = client
        .get(format!("http://{addr}/api/info"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<Info>().await.ok().map(|i| i.device_id)
}

/// 配对：申请加入、等主机同意。`on_sas` 拿到核对数字时调用（界面显示出来让用户和主机比对）。
pub async fn pair(
    addr: SocketAddr,
    code: &str,
    my_id: &str,
    my_name: &str,
    platform: &str,
    on_sas: impl Fn(&str),
) -> AppResult<Credentials> {
    let client = http_client();
    let code = super::host::normalize_code(code);
    let hs = Handshake::new();
    let my_pub = hs.public();
    let res = client
        .post(format!("http://{addr}/api/pair/request"))
        .timeout(Duration::from_secs(10))
        .json(&serde_json::json!({
            "code": code,
            "name": my_name,
            "platform": platform,
            "kind": "desktop",
            "deviceId": my_id,
            "publicKey": B64.encode(my_pub),
        }))
        .send()
        .await?;
    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(AppError::msg(if text.is_empty() {
            "主机拒绝了申请".to_string()
        } else {
            text
        }));
    }
    let resp: PairResponse = res.json().await?;
    let host_pub = resp
        .host_public_key
        .and_then(|k| B64.decode(k).ok())
        .ok_or_else(|| AppError::msg("主机没有返回公钥"))?;
    let session = hs.finish(
        &host_pub,
        &crypto::pair_transcript(&my_pub, &host_pub, &code),
    )?;
    if resp.sas.as_deref() != Some(session.sas.as_str()) {
        return Err(AppError::msg(
            "两边的核对数字对不上，可能有人在冒充主机，已停止",
        ));
    }
    on_sas(&session.sas);

    for _ in 0..12 {
        let wait: WaitResponse = client
            .get(format!("http://{addr}/api/pair/wait"))
            .query(&[("id", resp.id.as_str())])
            .timeout(Duration::from_secs(40))
            .send()
            .await?
            .json()
            .await?;
        match wait.status.as_str() {
            "pending" => continue,
            "approved" => {
                let sealed = wait
                    .sealed_key
                    .and_then(|k| B64.decode(k).ok())
                    .ok_or_else(|| AppError::msg("主机没有返回密钥"))?;
                let key: SecretKey = crypto::open(&session.key, &sealed, AAD_MEMBER_KEY)?
                    .try_into()
                    .map_err(|_| AppError::msg("密钥长度不对"))?;
                return Ok(Credentials {
                    my_id: my_id.to_string(),
                    host_id: resp.host_id,
                    host_name: resp.host_name,
                    key,
                    address: Some(addr),
                });
            }
            "denied" => return Err(AppError::msg("主机拒绝了加入")),
            _ => return Err(AppError::msg("申请已过期，请重新输入设备码")),
        }
    }
    Err(AppError::msg("等太久了，主机一直没有处理申请"))
}

type OnRecord = Arc<dyn Fn(SyncRecord) + Send + Sync>;
type OnAddress = Arc<dyn Fn(SocketAddr) + Send + Sync>;

/// 跑着的成员连接。drop 时断开。
pub struct Member {
    status: Arc<Mutex<MemberStatus>>,
    outbox: Arc<Mutex<VecDeque<SyncRecord>>>,
    wake: Arc<Notify>,
    stop: watch::Sender<bool>,
}

const OUTBOX_MAX: usize = 20;

impl Member {
    pub fn start(creds: Credentials, on_record: OnRecord, on_address: OnAddress) -> Self {
        let status = Arc::new(Mutex::new(MemberStatus::default()));
        let outbox = Arc::new(Mutex::new(VecDeque::new()));
        let wake = Arc::new(Notify::new());
        let (stop, stop_rx) = watch::channel(false);
        let ctx = Ctx {
            creds,
            status: status.clone(),
            outbox: outbox.clone(),
            wake: wake.clone(),
            on_record,
            on_address,
            client: http_client(),
        };
        tauri::async_runtime::spawn(run(ctx, stop_rx));
        Self {
            status,
            outbox,
            wake,
            stop,
        }
    }

    pub fn status(&self) -> MemberStatus {
        self.status.lock().clone()
    }

    /// 推一条记录给主机（连着就马上发，断着就攒着，连上再发）。
    pub fn push(&self, record: SyncRecord) {
        let mut q = self.outbox.lock();
        if q.len() >= OUTBOX_MAX {
            q.pop_front();
        }
        q.push_back(record);
        drop(q);
        self.wake.notify_one();
    }
}

impl Drop for Member {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

struct Ctx {
    creds: Credentials,
    status: Arc<Mutex<MemberStatus>>,
    outbox: Arc<Mutex<VecDeque<SyncRecord>>>,
    wake: Arc<Notify>,
    on_record: OnRecord,
    on_address: OnAddress,
    client: reqwest::Client,
}

impl Ctx {
    fn set_status(&self, online: bool, address: Option<SocketAddr>, error: Option<String>) {
        *self.status.lock() = MemberStatus {
            online,
            address: address.map(|a| a.to_string()),
            error,
        };
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header("x-dato-device", &self.creds.my_id).header(
            "x-dato-auth",
            auth_header(&self.creds.my_id, &self.creds.key),
        )
    }

    async fn send(&self, addr: SocketAddr, record: &SyncRecord) -> AppResult<()> {
        let body = crypto::seal(&self.creds.key, &record.encode(), AAD_RECORD);
        let res = self
            .auth(self.client.post(format!("http://{addr}/api/m/push")))
            .timeout(Duration::from_secs(30))
            .body(body)
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(AppError::Network(format!("主机返回 {}", res.status())));
        }
        Ok(())
    }

    async fn flush(&self, addr: SocketAddr) -> AppResult<()> {
        loop {
            let Some(record) = self.outbox.lock().front().cloned() else {
                return Ok(());
            };
            self.send(addr, &record).await?;
            self.outbox.lock().pop_front();
        }
    }

    fn handle_event(&self, event: &str, data: &str) {
        if event != "record" {
            return;
        }
        let record = B64
            .decode(data.trim())
            .map_err(|e| AppError::msg(e.to_string()))
            .and_then(|b| crypto::open(&self.creds.key, &b, AAD_RECORD))
            .and_then(|b| SyncRecord::decode(&b));
        match record {
            Ok(r) => {
                let cb = self.on_record.clone();
                tauri::async_runtime::spawn_blocking(move || cb(r));
            }
            Err(err) => tracing::warn!("主机推来的记录解不开：{err}"),
        }
    }
}

async fn run(ctx: Ctx, mut stop: watch::Receiver<bool>) {
    let mut addr = ctx.creds.address;
    let mut failures = 0u32;
    loop {
        if *stop.borrow() {
            break;
        }
        if addr.is_none() || failures >= 2 {
            if let Some(found) = rediscover(&ctx.creds.host_id).await {
                if addr != Some(found) {
                    (ctx.on_address)(found);
                }
                addr = Some(found);
                failures = 0;
            }
        }
        let result = match addr {
            Some(a) => session(&ctx, a, &mut stop).await,
            None => Err(AppError::msg("局域网里找不到主机")),
        };
        if *stop.borrow() {
            break;
        }
        let (error, wait) = match result {
            Ok(()) => (None, 1),
            Err(AppError::Msg(m)) if m == "unauthorized" => {
                (Some("主机已经移除了本机，请重新加入".to_string()), 60)
            }
            Err(err) => {
                failures += 1;
                (Some(err.to_string()), (2u64 << failures.min(4)).min(30))
            }
        };
        ctx.set_status(false, addr, error);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
            _ = stop.changed() => {}
        }
    }
    ctx.set_status(false, None, None);
}

/// 一次连接：开 SSE，边收边把攒着的记录推上去，断了就返回。
async fn session(ctx: &Ctx, addr: SocketAddr, stop: &mut watch::Receiver<bool>) -> AppResult<()> {
    let res = ctx
        .auth(ctx.client.get(format!("http://{addr}/api/m/events")))
        .send()
        .await?;
    if res.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::msg("unauthorized"));
    }
    if !res.status().is_success() {
        return Err(AppError::Network(format!("主机返回 {}", res.status())));
    }
    ctx.set_status(true, Some(addr), None);
    tracing::info!(%addr, "已连上同步主机");
    if let Err(err) = ctx.flush(addr).await {
        tracing::warn!("推送攒着的记录失败：{err}");
    }
    let mut stream = res.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { return Ok(()) };
                buf.extend_from_slice(&chunk?);
                while let Some(end) = find_event_end(&buf) {
                    let raw: Vec<u8> = buf.drain(..end).collect();
                    let text = String::from_utf8_lossy(&raw);
                    let (event, data) = parse_event(&text);
                    ctx.handle_event(&event, &data);
                }
            }
            _ = ctx.wake.notified() => {
                if let Err(err) = ctx.flush(addr).await {
                    tracing::warn!("推送记录失败：{err}");
                    return Err(err);
                }
            }
            _ = stop.changed() => return Ok(()),
        }
    }
}

/// SSE 事件之间是一个空行。返回这个事件（含分隔）结束的位置。
fn find_event_end(buf: &[u8]) -> Option<usize> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn parse_event(text: &str) -> (String, String) {
    let mut event = "message".to_string();
    let mut data = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(v.strip_prefix(' ').unwrap_or(v));
        }
    }
    (event, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_framing() {
        let buf = b"event: record\ndata: abc\n\n: keep-alive\n\nevent: x".to_vec();
        let end = find_event_end(&buf).unwrap();
        let (e, d) = parse_event(std::str::from_utf8(&buf[..end]).unwrap());
        assert_eq!((e.as_str(), d.as_str()), ("record", "abc"));
        let rest = &buf[end..];
        let end2 = find_event_end(rest).unwrap();
        assert_eq!(
            parse_event(std::str::from_utf8(&rest[..end2]).unwrap()).1,
            ""
        );
        assert!(find_event_end(&rest[end2..]).is_none());
    }
}
