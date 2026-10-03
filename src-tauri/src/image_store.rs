//! 内存图片仓库 + `shot:` 自定义协议。
//!
//! 冻结的屏幕画面、贴图、长图编辑器里的图都放这里，WebView 用
//! `http://shot.localhost/<id>` 取（Windows 上自定义协议的形式）。
//!
//! 为什么不走 IPC：4K 一张 33MB，base64 过 IPC 要近一秒；自定义协议直接回二进制。
//! 为什么不落盘：用完即丢的东西没必要写文件再读回来。

use std::collections::HashMap;
use std::sync::Arc;

use image::RgbaImage;
use parking_lot::RwLock;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::imaging;
use crate::state::AppState;

pub const SCHEME: &str = "shot";

#[derive(Default)]
pub struct ImageStore {
    images: RwLock<HashMap<String, Arc<RgbaImage>>>,
}

impl ImageStore {
    pub fn put(&self, id: impl Into<String>, image: Arc<RgbaImage>) {
        self.images.write().insert(id.into(), image);
    }

    pub fn get(&self, id: &str) -> Option<Arc<RgbaImage>> {
        self.images.read().get(id).cloned()
    }

    pub fn remove(&self, id: &str) {
        self.images.write().remove(id);
    }

    pub fn remove_prefix(&self, prefix: &str) {
        self.images.write().retain(|k, _| !k.starts_with(prefix));
    }
}

fn respond(status: StatusCode, mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    if let Ok(v) = mime.parse() {
        headers.insert(header::CONTENT_TYPE, v);
    }
    // 自定义协议对 WebView 而言是跨源的，没有这个头 fetch 会被静默拒绝
    if let Ok(v) = "*".parse() {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
    if let Ok(v) = "no-store".parse() {
        headers.insert(header::CACHE_CONTROL, v);
    }
    response
}

pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let id = request.uri().path().trim_matches('/').to_string();
    // 编码放到工作线程，别占着 WebView 的协议回调线程
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let response = match state.images.get(&id) {
            Some(img) => match imaging::encode_for_webview(&img) {
                Ok((bytes, mime)) => respond(StatusCode::OK, mime, bytes),
                Err(err) => respond(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "text/plain",
                    err.to_string().into_bytes(),
                ),
            },
            None => respond(StatusCode::NOT_FOUND, "text/plain", b"not found".to_vec()),
        };
        responder.respond(response);
    });
}
