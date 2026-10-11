//! MCP 的 HTTP 端点（Streamable HTTP 传输：POST JSON-RPC，回 JSON；不开 SSE 推流）。

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::AppHandle;
use tokio::sync::oneshot;

use super::tools;
use crate::error::{AppError, AppResult};

/// 支持的协议版本：客户端要哪个就回哪个，不认识的回最新的
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

struct Shared {
    app: AppHandle,
    token: RwLock<String>,
}

pub struct Server {
    shared: Arc<Shared>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Server {
    pub fn start(app: AppHandle, port: u16, token: &str) -> AppResult<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
            AppError::msg(format!("端口 {port} 打不开（可能被别的程序占用了）：{e}"))
        })?;
        listener.set_nonblocking(true)?;
        let shared = Arc::new(Shared {
            app,
            token: RwLock::new(token.to_string()),
        });
        let router = Router::new()
            .route("/mcp", post(handle).get(no_stream).delete(no_stream))
            .with_state(shared.clone());
        let (tx, rx) = oneshot::channel::<()>();
        tauri::async_runtime::spawn(async move {
            let listener = match tokio::net::TcpListener::from_std(listener) {
                Ok(l) => l,
                Err(err) => {
                    tracing::error!("MCP 服务启动失败：{err}");
                    return;
                }
            };
            let serve = axum::serve(listener, router).with_graceful_shutdown(async {
                let _ = rx.await;
            });
            if let Err(err) = serve.await {
                tracing::warn!("MCP 服务退出：{err}");
            }
        });
        Ok(Self {
            shared,
            shutdown: Some(tx),
        })
    }

    pub fn set_token(&self, token: &str) {
        *self.shared.token.write() = token.to_string();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// 不开服务器推流（GET 打开 SSE）、也不管会话（DELETE）
async fn no_stream() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

async fn handle(State(shared): State<Arc<Shared>>, headers: HeaderMap, body: Bytes) -> Response {
    // 网页里的脚本（DNS 重绑定之类）不许连：带了 Origin 的只认本机
    if let Some(origin) = header(&headers, "origin") {
        let local = [
            "http://127.0.0.1",
            "http://localhost",
            "https://127.0.0.1",
            "https://localhost",
        ]
        .iter()
        .any(|p| origin == *p || origin.starts_with(&format!("{p}:")));
        if !local {
            return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
        }
    }
    let expected = format!("Bearer {}", shared.token.read());
    if header(&headers, "authorization") != Some(expected.as_str()) {
        return (StatusCode::UNAUTHORIZED, "missing or wrong token").into_response();
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return json_response(rpc_error(Value::Null, -32700, "Parse error"));
    };
    let responses: Vec<Value> = match msg {
        Value::Array(batch) => {
            let mut out = Vec::new();
            for m in batch {
                if let Some(r) = dispatch(&shared.app, m).await {
                    out.push(r);
                }
            }
            out
        }
        m => dispatch(&shared.app, m).await.into_iter().collect(),
    };
    match responses.len() {
        // 只有通知（initialized 之类）：不用回
        0 => StatusCode::ACCEPTED.into_response(),
        1 => json_response(responses.into_iter().next().unwrap_or(Value::Null)),
        _ => json_response(Value::Array(responses)),
    }
}

fn json_response(v: Value) -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        v.to_string(),
    )
        .into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// 处理一条消息；通知（没有 id）返回 None。
pub(super) async fn dispatch(app: &AppHandle, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let wanted = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let version = VERSIONS
                .iter()
                .find(|v| **v == wanted)
                .unwrap_or(&VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": {
                    "name": "dato-ocr",
                    "title": "DATO OCR",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": tools::INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools::definitions() }),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let app = app.clone();
            let outcome =
                tauri::async_runtime::spawn_blocking(move || tools::call(&app, &name, &args)).await;
            match outcome {
                Ok(Ok(content)) => json!({ "content": content, "isError": false }),
                Ok(Err(err)) => json!({
                    "content": [{ "type": "text", "text": crate::i18n::text(&err) }],
                    "isError": true,
                }),
                Err(err) => json!({
                    "content": [{ "type": "text", "text": err.to_string() }],
                    "isError": true,
                }),
            }
        }
        _ => return Some(rpc_error(id, -32601, "Method not found")),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}
