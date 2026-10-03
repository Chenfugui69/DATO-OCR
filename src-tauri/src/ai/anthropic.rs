//! Anthropic Messages 接口（Claude）。`base_url` 填 `https://api.anthropic.com`（带不带 `/v1` 都行）。

use serde_json::{json, Value};

use super::{api_error, Params, Prepared, Sse};
use crate::error::{AppError, AppResult};

const VERSION: &str = "2023-06-01";
/// Messages 接口必须给 max_tokens；设置里填 0（用默认）时用这个
const DEFAULT_MAX_TOKENS: u32 = 8192;

fn root(base: &str) -> &str {
    base.trim_end_matches('/').trim_end_matches("/v1")
}

fn headers(req: reqwest::RequestBuilder, key: Option<&str>) -> reqwest::RequestBuilder {
    let req = req.header("anthropic-version", VERSION);
    match key {
        Some(key) => req.header("x-api-key", key),
        None => req,
    }
}

pub async fn list_models(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
) -> AppResult<Vec<String>> {
    let resp = headers(
        client.get(format!("{}/v1/models?limit=1000", root(base))),
        key,
    )
    .send()
    .await?;
    if !resp.status().is_success() {
        return Err(api_error(resp).await);
    }
    let body: Value = resp.json().await?;
    let ids = body["data"]
        .as_array()
        .ok_or_else(|| AppError::msg("模型列表格式无法识别"))?
        .iter()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect();
    Ok(ids)
}

fn message(m: &Prepared) -> Value {
    let mut parts: Vec<Value> = m
        .images
        .iter()
        .map(|b64| json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64 } }))
        .collect();
    parts.push(json!({ "type": "text", "text": m.text }));
    json!({ "role": m.role, "content": parts })
}

pub async fn stream(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    params: &Params,
    messages: &[Prepared],
    mut on_delta: impl FnMut(&str),
) -> AppResult<()> {
    let mut body = json!({
        "model": params.model,
        "max_tokens": if params.max_tokens > 0 { params.max_tokens } else { DEFAULT_MAX_TOKENS },
        "messages": messages.iter().map(message).collect::<Vec<_>>(),
        "temperature": params.temperature.min(1.0),
        "stream": true,
    });
    if !params.system.trim().is_empty() {
        body["system"] = json!(params.system);
    }
    let mut resp = headers(client.post(format!("{}/v1/messages", root(base))), key)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(api_error(resp).await);
    }
    let mut sse = Sse::default();
    while let Some(chunk) = resp.chunk().await? {
        for data in sse.feed(&chunk) {
            let Ok(event) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            match event["type"].as_str() {
                Some("content_block_delta") => {
                    if let Some(text) = event["delta"]["text"].as_str() {
                        on_delta(text);
                    }
                }
                Some("message_stop") => return Ok(()),
                Some("error") => {
                    let msg = event["error"]["message"].as_str().unwrap_or("服务返回错误");
                    return Err(AppError::msg(msg.to_string()));
                }
                _ => {}
            }
        }
    }
    Ok(())
}
