//! 给 AI 工具用的 MCP 服务（Model Context Protocol，参照 Paste 的 MCP）。
//!
//! 接上之后 Claude、Codex、Cursor 之类可以搜剪贴板历史、读某一条、往剪贴板里加内容、建分组、把内容放进分组、
//! 查识字记录（工具见 `tools`）。
//!
//! - 只在本机：服务绑 127.0.0.1，要带令牌（`Authorization: Bearer …`），网页来的请求（带别的 Origin）一律拒绝
//! - 能填网址的客户端（Claude Code、Cursor）直接连 `http://127.0.0.1:端口/mcp`（Streamable HTTP，只回 JSON，不推流）
//! - 只会起子进程的客户端（Claude Desktop）用 `DATO OCR --mcp`：那是个命令行桥接（`bridge`），把标准输入的
//!   JSON-RPC 转给这个服务。它靠数据目录里的 `mcp.json`（端口 + 令牌）找到服务，所以换令牌不用改客户端配置
//! - 验证码一律不给 AI 看

pub mod bridge;
mod server;
mod tools;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::AppHandle;

use crate::state::state;

const KV_TOKEN: &str = "mcp.token";

struct Running {
    server: server::Server,
    port: u16,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

pub fn token(app: &AppHandle) -> String {
    let db = &state(app).db;
    if let Ok(Some(t)) = db.kv_get(KV_TOKEN) {
        if t.len() >= 32 {
            return t;
        }
    }
    regenerate_token(app)
}

/// 换一个令牌：之前配好的客户端（填了旧令牌的）要重新配置，命令行桥接不受影响。
pub fn regenerate_token(app: &AppHandle) -> String {
    let t = uuid::Uuid::new_v4().simple().to_string() + &uuid::Uuid::new_v4().simple().to_string();
    let _ = state(app).db.kv_set(KV_TOKEN, &t);
    if let Some(r) = RUNNING.lock().as_ref() {
        r.server.set_token(&t);
        write_discovery(app, r.port, &t);
    }
    t
}

fn write_discovery(app: &AppHandle, port: u16, token: &str) {
    let body = serde_json::json!({ "port": port, "token": token, "path": "/mcp" });
    if let Err(err) = std::fs::write(state(app).paths.mcp_file(), body.to_string()) {
        tracing::warn!("写 mcp.json 失败：{err}");
    }
}

/// 按设置开 / 关。启动时和改了 MCP 设置时调用。
pub fn reconfigure(app: &AppHandle) {
    let s = state(app).settings.read().mcp.clone();
    // 先拿令牌：第一次要生成，生成时会锁 RUNNING，不能在下面锁住之后再拿
    let token = token(app);
    let mut running = RUNNING.lock();
    let same = running.as_ref().is_some_and(|r| r.port == s.port);
    if s.enabled && same {
        return;
    }
    *running = None;
    *LAST_ERROR.lock() = None;
    let _ = std::fs::remove_file(state(app).paths.mcp_file());
    if !s.enabled {
        return;
    }
    match server::Server::start(app.clone(), s.port, &token) {
        Ok(server) => {
            write_discovery(app, s.port, &token);
            tracing::info!(port = s.port, "MCP 服务已启动");
            *running = Some(Running {
                server,
                port: s.port,
            });
        }
        Err(err) => {
            tracing::warn!("MCP 服务启动失败：{err}");
            *LAST_ERROR.lock() = Some(crate::i18n::text(&err));
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub running: bool,
    pub url: String,
    pub token: String,
    pub error: Option<String>,
    /// 命令行桥接要用的程序路径（Claude Desktop 的配置里填它）
    pub command: String,
}

pub fn status(app: &AppHandle) -> McpStatus {
    let port = state(app).settings.read().mcp.port;
    McpStatus {
        running: RUNNING.lock().is_some(),
        url: format!("http://127.0.0.1:{port}/mcp"),
        token: token(app),
        error: LAST_ERROR.lock().clone(),
        command: std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}
