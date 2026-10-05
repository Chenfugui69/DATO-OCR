//! 同步的联网测试：本机回环上起真的服务，走一遍配对、收发、网页接口、WebDAV。

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;

use super::crypto::{KdfParams, SecretKey};
use super::lan::host::{Backend, Content, HostInfo, Server, WebItem};
use super::lan::member::{self, Member};
use super::record::SyncRecord;
use super::webdav::{self, Account, Dav, Runner};
use crate::error::AppResult;
use crate::storage::sync::Peer;

const CODE: &str = "48312290";
const LOOPBACK: std::net::IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

#[derive(Default)]
struct MockHost {
    members: Mutex<HashMap<String, (Peer, Option<SecretKey>)>>,
    received: Mutex<Vec<(SyncRecord, String)>>,
}

impl Backend for MockHost {
    fn info(&self) -> HostInfo {
        HostInfo {
            device_id: "host-device-0001".into(),
            name: "PC".into(),
            platform: "windows".into(),
            code: CODE.into(),
            web_enabled: true,
            max_upload: 4 * 1024 * 1024,
        }
    }
    fn pair_requested(&self) {}
    fn save_member(&self, peer: Peer, key: Option<SecretKey>) -> AppResult<()> {
        self.members
            .lock()
            .insert(peer.device_id.clone(), (peer, key));
        Ok(())
    }
    fn member_key(&self, device_id: &str) -> Option<SecretKey> {
        self.members.lock().get(device_id).and_then(|(_, k)| *k)
    }
    fn web_member(&self, token_hash: &[u8]) -> Option<Peer> {
        self.members
            .lock()
            .values()
            .find(|(p, _)| p.kind == "web" && p.secret == token_hash)
            .map(|(p, _)| p.clone())
    }
    fn remove_member(&self, device_id: &str) {
        self.members.lock().remove(device_id);
    }
    fn seen(&self, _: &str, _: SocketAddr) {}
    fn received(&self, record: SyncRecord, from: &str) {
        self.received.lock().push((record, from.to_string()));
    }
    fn recent(&self, _: u32) -> Vec<WebItem> {
        vec![WebItem {
            id: 7,
            kind: "text".into(),
            preview: Some("最近一条".into()),
            width: None,
            height: None,
            source: None,
            at: 1,
            remote: false,
        }]
    }
    fn content(&self, id: i64, _: bool) -> Option<Content> {
        (id == 7).then(|| Content::Text("最近一条".into()))
    }
    fn latest(&self) -> Option<Content> {
        self.content(7, false)
    }
}

fn text_record(id: &str, origin: &str, text: &str) -> SyncRecord {
    SyncRecord {
        id: id.into(),
        origin: origin.into(),
        origin_name: origin.into(),
        kind: "text".into(),
        text: Some(text.into()),
        created_at: 1,
        sent_at: crate::storage::now_ms(),
        ..Default::default()
    }
}

async fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
    for _ in 0..200 {
        if let Some(v) = f() {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("等太久了");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_pairing_and_records_both_ways() {
    let backend = Arc::new(MockHost::default());
    let server = Server::start(backend.clone(), LOOPBACK, 0).unwrap();
    let addr: SocketAddr = format!("127.0.0.1:{}", server.port).parse().unwrap();
    let host = server.host.clone();

    // 主机这边：等申请出现，核对数字后同意
    let approver = {
        let host = host.clone();
        tokio::spawn(async move {
            let req = wait_for(|| host.pending().into_iter().next()).await;
            assert_eq!(req.name, "MacBook");
            assert_eq!(req.kind, "desktop");
            host.decide(&req.id, true).unwrap();
            req.sas.unwrap()
        })
    };
    let member_sas = Arc::new(Mutex::new(String::new()));
    let sas_slot = member_sas.clone();
    let creds = member::pair(
        addr,
        "4831 2290",
        "member-device-01",
        "MacBook",
        "macos",
        move |s| {
            *sas_slot.lock() = s.to_string();
        },
    )
    .await
    .unwrap();
    let host_sas = approver.await.unwrap();
    assert_eq!(*member_sas.lock(), host_sas, "两边核对数字一致");
    assert_eq!(creds.host_id, "host-device-0001");
    assert_eq!(creds.host_name, "PC");
    assert_eq!(backend.member_key("member-device-01"), Some(creds.key));

    // 成员连上，主机推一条下去
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SyncRecord>();
    let m = Member::start(
        creds,
        Arc::new(move |r| {
            let _ = tx.send(r);
        }),
        Arc::new(|_| {}),
    );
    wait_for(|| {
        host.online()
            .contains(&"member-device-01".to_string())
            .then_some(())
    })
    .await;
    assert!(m.status().online);
    host.broadcast(&text_record("r1", "host-device-0001", "电脑上复制的"), None);
    let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.text.as_deref(), Some("电脑上复制的"));

    // 来源就是它自己的不回推
    host.broadcast(
        &text_record("r2", "x", "不该收到"),
        Some("member-device-01"),
    );

    // 成员推一条上来
    m.push(text_record("r3", "member-device-01", "Mac 上复制的"));
    wait_for(|| (!backend.received.lock().is_empty()).then_some(())).await;
    let (rec, from) = backend.received.lock()[0].clone();
    assert_eq!(rec.text.as_deref(), Some("Mac 上复制的"));
    assert_eq!(from, "member-device-01");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), rx.recv())
            .await
            .is_err(),
        "排除的那台不应该收到"
    );

    // 移除后连不上
    host.forget("member-device-01");
    backend.remove_member("member-device-01");
    wait_for(|| (!m.status().online).then_some(())).await;
    drop(m);
    drop(server);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_pairing_items_and_push() {
    let backend = Arc::new(MockHost::default());
    let server = Server::start(backend.clone(), LOOPBACK, 0).unwrap();
    let base = format!("http://127.0.0.1:{}", server.port);
    let client = member::http_client();

    // 页面本身能打开
    let page = client.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);

    // 没令牌不给看
    let r = client
        .get(format!("{base}/api/items"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = client
        .get(format!("{base}/api/m/events"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // 设备码不对
    let r = client
        .post(format!("{base}/api/pair/request"))
        .json(&serde_json::json!({ "code": "11112222", "name": "x", "kind": "web" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    let r: serde_json::Value = client
        .post(format!("{base}/api/pair/request"))
        .json(&serde_json::json!({ "code": CODE, "name": "iPhone", "platform": "ios", "kind": "web" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = r["id"].as_str().unwrap().to_string();
    assert!(r["sas"].is_null());
    let host = server.host.clone();
    let req_id = id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        host.decide(&req_id, true).unwrap();
    });
    let res = client
        .get(format!("{base}/api/pair/wait"))
        .query(&[("id", id.as_str())])
        .send()
        .await
        .unwrap();
    let cookie = res
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cookie.starts_with("dato_token="));
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["status"], "approved");
    let token = body["token"].as_str().unwrap().to_string();

    // 令牌放请求头（快捷指令）或 Cookie（浏览器）都行
    let items: serde_json::Value = client
        .get(format!("{base}/api/items"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(items[0]["preview"], "最近一条");
    let latest = client
        .get(format!("{base}/api/latest"))
        .header("cookie", format!("dato_token={token}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(latest, "最近一条");

    let r = client
        .post(format!("{base}/api/push"))
        .bearer_auth(&token)
        .header("content-type", "text/plain; charset=utf-8")
        .body("手机上复制的")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let png = crate::imaging::encode_png(&image::RgbaImage::new(3, 2)).unwrap();
    let r = client
        .post(format!("{base}/api/push"))
        .bearer_auth(&token)
        // 快捷指令有时不标类型，按内容认
        .header("content-type", "application/octet-stream")
        .body(png.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let received = backend.received.lock().clone();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].0.text.as_deref(), Some("手机上复制的"));
    assert_eq!(received[0].0.origin_name, "iPhone");
    assert_eq!(received[1].0.kind, "image");
    assert_eq!(received[1].0.image.as_deref(), Some(png.as_slice()));

    // 连续输错设备码会被限流
    for _ in 0..6 {
        let _ = client
            .post(format!("{base}/api/pair/request"))
            .json(&serde_json::json!({ "code": "00000000", "name": "x", "kind": "web" }))
            .send()
            .await;
    }
    let r = client
        .post(format!("{base}/api/pair/request"))
        .json(&serde_json::json!({ "code": CODE, "name": "x", "kind": "web" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
}

// ───────────── 内存里的 WebDAV 服务（够测试用的最小子集） ─────────────

#[derive(Default)]
struct MemDav {
    files: Mutex<HashMap<String, Vec<u8>>>,
    dirs: Mutex<HashSet<String>>,
}

async fn dav_handler(
    axum::extract::State(fs): axum::extract::State<Arc<MemDav>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    use base64::Engine;
    let auth = base64::engine::general_purpose::STANDARD.encode("me:app-pass");
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Basic {auth}"))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let path = uri.path().trim_end_matches('/').to_string();
    let parent_ok = |p: &str| {
        let parent = p.rsplit_once('/').map(|(a, _)| a).unwrap_or("");
        parent == "/dav" || fs.dirs.lock().contains(parent)
    };
    match method.as_str() {
        "MKCOL" => {
            if fs.dirs.lock().contains(&path) {
                return StatusCode::METHOD_NOT_ALLOWED.into_response();
            }
            if !parent_ok(&path) {
                return StatusCode::CONFLICT.into_response();
            }
            fs.dirs.lock().insert(path);
            StatusCode::CREATED.into_response()
        }
        "PUT" => {
            if !parent_ok(&path) {
                return StatusCode::CONFLICT.into_response();
            }
            fs.files.lock().insert(path, body.to_vec());
            StatusCode::CREATED.into_response()
        }
        "GET" => match fs.files.lock().get(&path) {
            Some(b) => b.clone().into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
        "DELETE" => {
            fs.files.lock().remove(&path);
            StatusCode::NO_CONTENT.into_response()
        }
        "PROPFIND" => {
            if path != "/dav" && !fs.dirs.lock().contains(&path) {
                return StatusCode::NOT_FOUND.into_response();
            }
            let mut xml = format!(
                r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:"><d:response><d:href>{path}/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>"#
            );
            let prefix = format!("{path}/");
            for d in fs.dirs.lock().iter() {
                if let Some(rest) = d.strip_prefix(&prefix) {
                    if !rest.contains('/') {
                        xml.push_str(&format!(r#"<d:response><d:href>{d}/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>"#));
                    }
                }
            }
            for f in fs.files.lock().keys() {
                if let Some(rest) = f.strip_prefix(&prefix) {
                    if !rest.contains('/') {
                        xml.push_str(&format!(r#"<d:response><d:href>{f}</d:href><d:propstat><d:prop><d:resourcetype/></d:prop></d:propstat></d:response>"#));
                    }
                }
            }
            xml.push_str("</d:multistatus>");
            (StatusCode::MULTI_STATUS, xml).into_response()
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn start_mem_dav() -> (String, Arc<MemDav>) {
    let fs = Arc::new(MemDav::default());
    let app = axum::Router::new()
        .fallback(dav_handler)
        .with_state(fs.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{port}/dav/"), fs)
}

const FAST: KdfParams = KdfParams { m: 256, t: 1, p: 1 };

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn webdav_group_and_sync_between_two_devices() {
    let (url, fs) = start_mem_dav().await;
    let account = |password: &str| Account {
        url: url.clone(),
        user: "me".into(),
        password: password.into(),
        folder: "同步/DATO-COR".into(),
    };
    let client = member::http_client();

    let wrong = Dav::new(client.clone(), &account("nope")).unwrap();
    let err = wrong
        .open_group("sync-pass", FAST)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("应用密码"), "{err}");

    let dav = Dav::new(client.clone(), &account("app-pass")).unwrap();
    let key = dav.open_group("sync-pass", FAST).await.unwrap();
    // 第二台设备：同一个同步密码拿到同一把钥匙，错的密码被拒
    assert_eq!(dav.open_group("sync-pass", FAST).await.unwrap(), key);
    assert!(dav.open_group("other-pass", FAST).await.is_err());
    // 中文文件夹名按 URL 编码传过去，一级一级建好了
    assert!(fs
        .files
        .lock()
        .contains_key("/dav/%E5%90%8C%E6%AD%A5/DATO-COR/group.json"));

    let start = |my_id: &str, tx: tokio::sync::mpsc::UnboundedSender<(SyncRecord, bool)>| {
        let seen: Arc<Mutex<Option<HashSet<String>>>> = Arc::new(Mutex::new(None));
        Runner::start(
            client.clone(),
            webdav::Config {
                account: account("app-pass"),
                key,
                my_id: my_id.into(),
                my_name: my_id.into(),
                platform: "windows".into(),
                interval: Duration::from_millis(200),
            },
            Arc::new(move |r, live| {
                let _ = tx.send((r, live));
            }),
            Arc::new(move |next: Option<&HashSet<String>>| match next {
                Some(s) => {
                    *seen.lock() = Some(s.clone());
                    None
                }
                None => seen.lock().clone(),
            }),
        )
        .unwrap()
    };
    let (tx_a, mut rx_a) = tokio::sync::mpsc::unbounded_channel();
    let (tx_b, mut rx_b) = tokio::sync::mpsc::unbounded_channel();
    let a = start("aaaaaaaaaaaaaaaa", tx_a);
    let b = start("bbbbbbbbbbbbbbbb", tx_b);
    // 等两边都跑完第一轮（第一轮不算"刚复制的"）
    tokio::time::sleep(Duration::from_millis(600)).await;

    a.push(text_record("w1", "aaaaaaaaaaaaaaaa", "A 复制的"));
    let (got, live) = tokio::time::timeout(Duration::from_secs(5), rx_b.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.text.as_deref(), Some("A 复制的"));
    assert!(live, "刚复制的可以直接写剪贴板");
    // 网盘上是密文
    let stored: Vec<Vec<u8>> = fs
        .files
        .lock()
        .iter()
        .filter(|(k, _)| k.contains("/items/"))
        .map(|(_, v)| v.clone())
        .collect();
    assert_eq!(stored.len(), 1);
    assert!(!String::from_utf8_lossy(&stored[0]).contains("A 复制的"));
    // 自己发的不会自己收到
    assert!(
        tokio::time::timeout(Duration::from_millis(500), rx_a.recv())
            .await
            .is_err()
    );
    // 设备登记
    let devices = webdav::devices(client.clone(), &account("app-pass"), &key)
        .await
        .unwrap();
    assert_eq!(devices.len(), 2);
    drop(a);
    drop(b);
}
