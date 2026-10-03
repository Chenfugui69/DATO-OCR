//! 翻译（规格 04 第二部分）：多源自动降级 + 内存 LRU 缓存。
//!
//! 优先级就是设置里翻译源列表的顺序（用户可拖动排序），排第一的可用源是默认源。
//! 翻译面板可以同时请求多个源（前端逐个指定 provider 并发调用）。
//! 一个源失败或超时（5 秒）立刻换下一个；全部失败时给出明确提示，引导去设置里填密钥。
//! 内置免费源都是非官方网页接口，同一个源两次请求至少隔 3 秒，别把接口打爆。

pub mod detect;
pub mod providers;
pub mod selection;

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::AppHandle;

use crate::error::{AppError, AppResult};
use crate::state::state;
use crate::storage::secrets;
use providers::Ctx;

const CACHE_CAPACITY: usize = 200;
/// 内置免费源的最小请求间隔（规格 04 §6.1）
const FREE_MIN_INTERVAL: Duration = Duration::from_secs(3);
const MAX_TEXT_CHARS: usize = 20_000;

pub const SECRET_DEEPL: &str = "deepl_api_key";
pub const SECRET_OPENAI: &str = "openai_api_key";

#[derive(Default)]
pub struct TranslateService {
    cache: Mutex<Cache>,
    /// 每个免费源下一次允许发请求的时刻
    next_slot: Mutex<HashMap<String, Instant>>,
}

#[derive(Default)]
struct Cache {
    map: HashMap<String, TranslateResult>,
    order: VecDeque<String>,
}

impl Cache {
    fn get(&self, key: &str) -> Option<TranslateResult> {
        self.map.get(key).cloned()
    }
    fn put(&mut self, key: String, value: TranslateResult) {
        if self.map.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
            while self.order.len() > CACHE_CAPACITY {
                if let Some(old) = self.order.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TranslateRequest {
    pub text: String,
    /// None / "auto" = 自动检测
    pub from: Option<String>,
    /// None / "auto" = 按设置（中文 ↔ 英文）
    pub to: Option<String>,
    /// None / "auto" = 按优先级自动降级
    pub provider: Option<String>,
    /// 不读缓存（设置页"检测"按钮用，要真的发一次请求）
    pub fresh: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateResult {
    pub text: String,
    pub from: String,
    pub to: String,
    pub provider: String,
    pub cached: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub free: bool,
    /// 能不能用：免费源总是能用；自填源要填了密钥
    pub configured: bool,
    /// 用户在设置里有没有打开它
    pub enabled: bool,
}

fn provider_name(id: &str) -> &'static str {
    match id {
        "bing" => "必应翻译",
        "transmart" => "腾讯翻译",
        "google" => "谷歌翻译",
        "deepl" => "DeepL",
        "openai" => "AI 大模型",
        _ => "未知",
    }
}

fn is_free(id: &str) -> bool {
    matches!(id, "bing" | "transmart" | "google")
}

/// 全部翻译源，按设置里的顺序。
pub fn list_providers(app: &AppHandle) -> Vec<ProviderInfo> {
    let st = state(app);
    let entries = st.settings.read().translate.providers.clone();
    entries
        .into_iter()
        .map(|p| {
            let configured = match p.id.as_str() {
                "deepl" => secret(app, SECRET_DEEPL).is_some(),
                "openai" => secret(app, SECRET_OPENAI).is_some(),
                _ => true,
            };
            ProviderInfo {
                name: provider_name(&p.id).into(),
                free: is_free(&p.id),
                configured,
                enabled: p.enabled,
                id: p.id,
            }
        })
        .collect()
}

/// 限流：间隔不够就等到够了再发，而不是直接失败（命中缓存的不走这里）。
async fn throttle(app: &AppHandle, provider: &str) {
    let wait = {
        let st = state(app);
        let mut slots = st.translate.next_slot.lock();
        let now = Instant::now();
        let start = slots
            .get(provider)
            .copied()
            .filter(|t| *t > now)
            .unwrap_or(now);
        // 先占位，并发的请求会排到后面
        slots.insert(provider.to_string(), start + FREE_MIN_INTERVAL);
        start - now
    };
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
}

fn cache_key(text: &str, to: &str, provider: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{hex}:{to}:{provider}")
}

pub async fn translate(app: &AppHandle, req: TranslateRequest) -> AppResult<TranslateResult> {
    let text = req.text.trim();
    if text.is_empty() {
        return Err(AppError::msg("没有要翻译的内容"));
    }
    let text: String = text.chars().take(MAX_TEXT_CHARS).collect();
    let (settings, offline) = {
        let st = state(app);
        let s = st.settings.read();
        (s.translate.clone(), s.general.offline_mode)
    };
    if offline {
        return Err(AppError::msg("已开启离线模式，翻译不可用"));
    }

    let from = req.from.filter(|f| f != "auto");
    let detected = from
        .clone()
        .unwrap_or_else(|| detect::detect(&text).to_string());
    let to = req
        .to
        .filter(|t| t != "auto")
        .or_else(|| Some(settings.target_language.clone()).filter(|t| t != "auto"))
        .unwrap_or_else(|| detect::default_target(&detected).to_string());

    let order: Vec<String> = match req.provider.filter(|p| p != "auto") {
        Some(p) => vec![p],
        None => list_providers(app)
            .into_iter()
            .filter(|p| p.enabled && p.configured)
            .map(|p| p.id)
            .collect(),
    };
    if order.is_empty() {
        return Err(AppError::msg("没有可用的翻译源，请在设置中启用"));
    }
    let http = crate::net::client(app, crate::net::Purpose::Translate);

    let mut last_err: Option<AppError> = None;
    for provider in order {
        let key = cache_key(&text, &to, &provider);
        if !req.fresh {
            if let Some(mut hit) = state(app).translate.cache.lock().get(&key) {
                hit.cached = true;
                return Ok(hit);
            }
        }
        if is_free(&provider) {
            throttle(app, &provider).await;
        }
        let ctx = Ctx {
            client: &http,
            text: &text,
            from: from.as_deref(),
            to: &to,
        };
        let result = match provider.as_str() {
            "bing" => providers::bing(&ctx).await,
            "transmart" => providers::transmart(&ctx, &detected).await,
            "google" => providers::google(&ctx).await,
            "deepl" => match secret(app, SECRET_DEEPL) {
                Some(k) => providers::deepl(&ctx, &k).await,
                None => Err(AppError::msg("未填写 DeepL 密钥")),
            },
            "openai" => match secret(app, SECRET_OPENAI) {
                Some(k) => {
                    providers::openai(&ctx, &settings.custom_base_url, &settings.custom_model, &k)
                        .await
                }
                None => Err(AppError::msg("未填写 AI 接口密钥")),
            },
            other => Err(AppError::msg(format!("未知翻译源 {other}"))),
        };
        match result {
            Ok(out) if !out.text.trim().is_empty() => {
                let value = TranslateResult {
                    text: out.text,
                    from: out.detected.unwrap_or(detected.clone()),
                    to: to.clone(),
                    provider: provider.clone(),
                    cached: false,
                };
                state(app).translate.cache.lock().put(key, value.clone());
                return Ok(value);
            }
            Ok(_) => {
                tracing::warn!(%provider, "翻译源返回空结果，换下一个");
                last_err = Some(AppError::msg("翻译结果为空"));
            }
            Err(err) => {
                // 日志里只记源和错误，不记原文（规格 00 §6.3）
                tracing::warn!(%provider, chars = text.chars().count(), "翻译失败，换下一个：{err}");
                last_err = Some(err);
            }
        }
    }
    Err(match last_err {
        Some(AppError::Network(_)) | None => {
            AppError::msg("内置翻译暂时不可用，请检查网络，或在设置中填写自己的翻译密钥")
        }
        Some(err) => err,
    })
}

fn secret(app: &AppHandle, key: &str) -> Option<String> {
    state(app).db.with(|c| secrets::get(c, key)).ok().flatten()
}
