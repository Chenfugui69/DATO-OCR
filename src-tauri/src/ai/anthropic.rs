//! Anthropic Messages 接口（Claude）。`base_url` 填 `https://api.anthropic.com`（带不带 `/v1` 都行）。

use serde_json::{json, Value};

use super::{
    api_error, dedup_bodies, fixed_sampling, legacy_claude, send_first_ok, Params, Piece, Prepared,
    Sse, NOTICE_NO_THINKING,
};
use crate::error::{AppError, AppResult};

const VERSION: &str = "2023-06-01";
/// Messages 接口必须给 max_tokens；设置里填 0（用默认）时用这个。
/// 新一代模型都支持 6.4 万以上；老模型（Claude 3.5 只有 8192）用小的
const MAX_TOKENS: u32 = 32_000;
const LEGACY_MAX_TOKENS: u32 = 8192;

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

/// 老模型思考预算（`budget_tokens`），按深度给
fn budget(effort: &str) -> u32 {
    match effort {
        "low" => 2048,
        "medium" => 6000,
        "high" => 12_000,
        _ => 24_000,
    }
}

/// 拼请求体。按从激进到保守排好几份：服务不认某个参数时退到下一份（见 `send_first_ok`）。
fn bodies(params: &Params, messages: &[Prepared]) -> Vec<Value> {
    let legacy = legacy_claude(&params.model);
    let user_max = (params.max_tokens > 0).then_some(params.max_tokens);
    let base = |max_tokens: u32| {
        let mut body = json!({
            "model": params.model,
            "max_tokens": max_tokens,
            "messages": messages.iter().map(message).collect::<Vec<_>>(),
            "stream": true,
        });
        if !params.system.trim().is_empty() {
            body["system"] = json!(params.system);
        }
        body
    };
    // 新一代模型一直在想（思考 token 也算进 max_tokens），给足余量
    let mut full = base(user_max.unwrap_or(if legacy {
        LEGACY_MAX_TOKENS
    } else {
        MAX_TOKENS
    }));
    match params.effort {
        Some(effort) if legacy => {
            let budget = budget(effort);
            full["max_tokens"] = json!(user_max
                .filter(|m| *m > budget + 1024)
                .unwrap_or(budget + 8192));
            full["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
        }
        Some(effort) => {
            full["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
            full["output_config"] = json!({ "effort": effort });
        }
        // 开思考时温度只能是默认值，新一代模型干脆不收温度
        None if !fixed_sampling(&params.model) => {
            full["temperature"] = json!(params.temperature.min(1.0));
        }
        None => {}
    }
    // 兜底：只留必填项，max_tokens 取所有模型都接受的值
    let minimal = base(user_max.unwrap_or(LEGACY_MAX_TOKENS));
    dedup_bodies(vec![full, minimal])
}

pub async fn stream(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    params: &Params,
    messages: &[Prepared],
    mut on_piece: impl FnMut(Piece),
) -> AppResult<()> {
    let url = format!("{}/v1/messages", root(base));
    let bodies = bodies(params, messages);
    let (mut resp, used) =
        send_first_ok(&bodies, |body| headers(client.post(&url), key).json(body)).await?;
    if used > 0 && params.effort.is_some() {
        on_piece(Piece::Notice(NOTICE_NO_THINKING));
    }
    let mut sse = Sse::default();
    while let Some(chunk) = resp.chunk().await? {
        for data in sse.feed(&chunk) {
            let Ok(event) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            match event["type"].as_str() {
                Some("content_block_delta") => {
                    let delta = &event["delta"];
                    match delta["type"].as_str() {
                        Some("thinking_delta") => {
                            if let Some(text) = delta["thinking"].as_str().filter(|t| !t.is_empty())
                            {
                                on_piece(Piece::Reasoning(text));
                            }
                        }
                        _ => {
                            if let Some(text) = delta["text"].as_str() {
                                on_piece(Piece::Text(text));
                            }
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn params(model: &str, effort: Option<&'static str>) -> Params {
        Params {
            model: model.into(),
            system: String::new(),
            temperature: 0.7,
            max_tokens: 0,
            effort,
        }
    }

    #[test]
    fn modern_claude_uses_adaptive_thinking_without_temperature() {
        let b = bodies(&params("claude-opus-5-5", Some("high")), &[]);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0]["thinking"]["type"], "adaptive");
        assert_eq!(b[0]["output_config"]["effort"], "high");
        assert!(b[0].get("temperature").is_none());
        assert!(b[1].get("thinking").is_none());
        // 不开思考也不发温度（新一代模型不收）
        let b = bodies(&params("claude-opus-5-5", None), &[]);
        assert!(b[0].get("temperature").is_none());
    }

    #[test]
    fn legacy_claude_uses_budget_below_max_tokens() {
        let b = bodies(&params("claude-sonnet-4-5-20250929", Some("max")), &[]);
        let budget = b[0]["thinking"]["budget_tokens"].as_u64().unwrap();
        assert!(b[0]["max_tokens"].as_u64().unwrap() > budget);
        let b = bodies(&params("claude-3-5-haiku-latest", None), &[]);
        assert_eq!(b.len(), 2);
        assert!(b[0].get("temperature").is_some());
        assert!(b[1].get("temperature").is_none());
    }
}
