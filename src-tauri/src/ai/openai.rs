//! OpenAI 兼容接口（OpenAI / DeepSeek / 通义 / Kimi / 智谱 / 硅基流动 / OpenRouter / Ollama …）。
//!
//! `base_url` 填到版本号为止，例如 `https://api.openai.com/v1`，后面拼 `/models`、`/chat/completions`。

use serde_json::{json, Value};

use super::{api_error, Params, Prepared, Sse};
use crate::error::{AppError, AppResult};

pub async fn list_models(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
) -> AppResult<Vec<String>> {
    let mut req = client.get(format!("{base}/models"));
    if let Some(key) = key {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(api_error(resp).await);
    }
    let body: Value = resp.json().await?;
    let mut ids: Vec<String> = body["data"]
        .as_array()
        .or_else(|| body["models"].as_array())
        .ok_or_else(|| AppError::msg("模型列表格式无法识别"))?
        .iter()
        .filter_map(|m| {
            m["id"]
                .as_str()
                .or_else(|| m["name"].as_str())
                .map(String::from)
        })
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn message(m: &Prepared) -> Value {
    if m.images.is_empty() {
        return json!({ "role": m.role, "content": m.text });
    }
    let mut parts: Vec<Value> = m
        .images
        .iter()
        .map(|b64| json!({ "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{b64}") } }))
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
    let mut list = Vec::with_capacity(messages.len() + 1);
    if !params.system.trim().is_empty() {
        list.push(json!({ "role": "system", "content": params.system }));
    }
    list.extend(messages.iter().map(message));
    let mut body = json!({
        "model": params.model,
        "messages": list,
        "stream": true,
        "temperature": params.temperature,
    });
    if params.max_tokens > 0 {
        body["max_tokens"] = json!(params.max_tokens);
    }
    let mut req = client.post(format!("{base}/chat/completions")).json(&body);
    if let Some(key) = key {
        req = req.bearer_auth(key);
    }
    let mut resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(api_error(resp).await);
    }
    let mut sse = Sse::default();
    while let Some(chunk) = resp.chunk().await? {
        for data in sse.feed(&chunk) {
            if data == "[DONE]" {
                return Ok(());
            }
            let Ok(event) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            if let Some(msg) = event["error"]["message"].as_str() {
                return Err(AppError::msg(msg.to_string()));
            }
            if let Some(text) = event["choices"][0]["delta"]["content"].as_str() {
                if !text.is_empty() {
                    on_delta(text);
                }
            }
        }
    }
    Ok(())
}
