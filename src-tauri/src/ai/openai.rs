//! OpenAI 兼容接口（OpenAI / DeepSeek / 通义 / Kimi / 智谱 / 硅基流动 / OpenRouter / Ollama …）。
//!
//! `base_url` 填到版本号为止，例如 `https://api.openai.com/v1`，后面拼 `/models`、`/chat/completions`。

use serde_json::{json, Value};

use super::{
    api_error, dedup_bodies, fixed_sampling, send_first_ok, Params, Piece, Prepared, Sse,
    NOTICE_NO_THINKING,
};
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

/// 拼请求体。按从激进到保守排好几份：服务不认某个参数时退到下一份（见 `send_first_ok`）。
fn bodies(params: &Params, messages: &[Prepared]) -> Vec<Value> {
    let mut list = Vec::with_capacity(messages.len() + 1);
    if !params.system.trim().is_empty() {
        list.push(json!({ "role": "system", "content": params.system }));
    }
    list.extend(messages.iter().map(message));
    let minimal = json!({
        "model": params.model,
        "messages": list,
        "stream": true,
    });
    let mut full = minimal.clone();
    if params.max_tokens > 0 {
        full["max_tokens"] = json!(params.max_tokens);
    }
    let mut out = Vec::with_capacity(3);
    match params.effort {
        Some(effort) => {
            // 推理模型不收温度；最深一档 xhigh 只有较新的模型认，不认就退回 high
            let level = if effort == "max" { "xhigh" } else { effort };
            full["reasoning_effort"] = json!(level);
            out.push(full.clone());
            if level == "xhigh" {
                full["reasoning_effort"] = json!("high");
                out.push(full);
            }
        }
        None => {
            if !fixed_sampling(&params.model) {
                full["temperature"] = json!(params.temperature);
            }
            out.push(full);
        }
    }
    out.push(minimal);
    dedup_bodies(out)
}

pub async fn stream(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    params: &Params,
    messages: &[Prepared],
    mut on_piece: impl FnMut(Piece),
) -> AppResult<()> {
    let url = format!("{base}/chat/completions");
    let bodies = bodies(params, messages);
    let (mut resp, used) = send_first_ok(&bodies, |body| {
        let req = client.post(&url).json(body);
        match key {
            Some(key) => req.bearer_auth(key),
            None => req,
        }
    })
    .await?;
    if params.effort.is_some() && bodies[used].get("reasoning_effort").is_none() {
        on_piece(Piece::Notice(NOTICE_NO_THINKING));
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
            let delta = &event["choices"][0]["delta"];
            // 思考过程：DeepSeek / 通义 / 硅基流动叫 reasoning_content，OpenRouter / Ollama 叫 reasoning
            if let Some(text) = delta["reasoning_content"]
                .as_str()
                .or_else(|| delta["reasoning"].as_str())
                .filter(|t| !t.is_empty())
            {
                on_piece(Piece::Reasoning(text));
            }
            if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
                on_piece(Piece::Text(text));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(model: &str, effort: Option<&'static str>) -> Params {
        Params {
            model: model.into(),
            system: "sys".into(),
            temperature: 0.7,
            max_tokens: 0,
            effort,
        }
    }

    #[test]
    fn effort_falls_back_from_xhigh_to_high_to_nothing() {
        let b = bodies(&params("gpt-5.2", Some("max")), &[]);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0]["reasoning_effort"], "xhigh");
        assert_eq!(b[1]["reasoning_effort"], "high");
        assert!(b[2].get("reasoning_effort").is_none());
        assert!(b.iter().all(|x| x.get("temperature").is_none()));
        assert_eq!(b[2]["messages"][0]["role"], "system");
    }

    #[test]
    fn plain_chat_keeps_temperature_except_reasoning_models() {
        let b = bodies(&params("deepseek-chat", None), &[]);
        assert_eq!(b[0]["temperature"], 0.7);
        assert!(b[1].get("temperature").is_none());
        let b = bodies(&params("openai/o3-mini", None), &[]);
        assert_eq!(b.len(), 1);
    }
}
