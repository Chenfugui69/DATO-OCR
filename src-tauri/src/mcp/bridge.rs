//! `DATO OCR --mcp`：给只会起子进程的 MCP 客户端（Claude Desktop 之类）用的命令行桥接。
//!
//! 一行一条 JSON-RPC 从标准输入读进来，转给正在运行的 DATO OCR 里的 MCP 服务，回复写到标准输出。
//! 端口和令牌每次都从数据目录的 `mcp.json` 现读（DATO OCR 重启、换了令牌也不用改客户端配置）。
//! 不启动界面；DATO OCR 没开或者没打开 MCP 时，对请求回一个说明原因的错误（英文：看的是 AI 客户端，它会转述）。

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::paths::AppPaths;

struct Target {
    url: String,
    token: String,
}

fn target() -> Result<Target, String> {
    let file = AppPaths::guess_root()
        .ok_or("cannot find the DATO OCR data folder")?
        .join("mcp.json");
    let raw = std::fs::read_to_string(&file).map_err(|_| {
        "DATO OCR is not running, or Settings → AI Tools → \"Allow AI tools to connect\" is off"
            .to_string()
    })?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let port = v["port"].as_u64().ok_or("mcp.json is malformed")?;
    Ok(Target {
        url: format!(
            "http://127.0.0.1:{port}{}",
            v["path"].as_str().unwrap_or("/mcp")
        ),
        token: v["token"].as_str().unwrap_or_default().to_string(),
    })
}

fn error_reply(id: Value, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } })
}

pub fn run() {
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap_or_default();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let id = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|v| v.get("id").cloned());
        let reply: Option<String> = match target() {
            Ok(t) => {
                let sent = tauri::async_runtime::block_on(async {
                    let resp = client
                        .post(&t.url)
                        .bearer_auth(&t.token)
                        .header("content-type", "application/json")
                        .header("accept", "application/json, text/event-stream")
                        .body(line.clone())
                        .send()
                        .await?;
                    let status = resp.status();
                    let body = resp.text().await?;
                    Ok::<_, reqwest::Error>((status, body))
                });
                match sent {
                    Ok((status, body)) if status.is_success() => {
                        Some(body).filter(|b| !b.trim().is_empty())
                    }
                    Ok((status, body)) => id.map(|id| {
                        error_reply(id, &format!("DATO OCR rejected the request ({status}): {body}"))
                            .to_string()
                    }),
                    Err(_) => id.map(|id| {
                        error_reply(id, "cannot reach DATO OCR; make sure it is running and AI tools are allowed in its settings")
                            .to_string()
                    }),
                }
            }
            Err(msg) => id.map(|id| error_reply(id, &msg).to_string()),
        };
        if let Some(reply) = reply {
            // 一条消息一行（stdio 传输的规矩：消息里不能有换行）
            let one_line = reply.replace('\n', " ");
            if writeln!(stdout, "{one_line}")
                .and_then(|_| stdout.flush())
                .is_err()
            {
                break;
            }
        }
    }
}
