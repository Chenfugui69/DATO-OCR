//! 翻译（规格 04 第二部分）：多源自动降级 + 内存 LRU 缓存。
//!
//! 优先级就是设置里翻译源列表的顺序（用户可拖动排序），排第一的可用源是默认源。
//! 翻译面板可以同时请求多个源（前端逐个指定 provider 并发调用）。
//! 一个源失败或超时（5 秒）立刻换下一个；全部失败时给出明确提示，引导去设置里填密钥。
//! 内置免费源都是非官方网页接口，同一个源两次请求至少隔 3 秒，别把接口打爆。

pub mod detect;
pub mod more;
pub mod providers;
pub mod selection;
pub mod worth;

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
    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
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
    /// 用户自己添加的（不是内置的）；`kind` 是它的类型：ai | deeplx | http
    pub custom: bool,
    pub kind: Option<String>,
    /// 能不能用：免费源总是能用；自填源要把必填的几项填了
    pub configured: bool,
    /// 用户在设置里有没有打开它
    pub enabled: bool,
    /// 要用户填的几项（密钥、应用 ID、区域……）和现在填没填
    pub fields: Vec<FieldInfo>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldInfo {
    pub key: String,
    pub label: String,
    pub secret: bool,
    pub required: bool,
    pub hint: String,
    pub set: bool,
    /// 填的值：密钥类的是打码后的样子，其余的是原样
    pub value: Option<String>,
}

fn is_free(id: &str) -> bool {
    more::builtin(id).is_some_and(|b| b.free)
}

/// 某个源的某一项存在密钥表里的键。DeepL、AI 大模型沿用老版本的键，别的按 `translate.<源>.<项>`。
fn field_key(provider: &str, field: &str) -> String {
    match (provider, field) {
        ("deepl", "key") => SECRET_DEEPL.into(),
        ("openai", "key") => SECRET_OPENAI.into(),
        _ => format!("translate.{provider}.{field}"),
    }
}

fn field(app: &AppHandle, provider: &str, name: &str) -> String {
    secret(app, &field_key(provider, name)).unwrap_or_default()
}

/// 存 / 清掉某个源的某一项（密钥、应用 ID、区域……都加密存在密钥表里，不进设置文件、不参与同步）。
pub fn set_field(
    app: &AppHandle,
    provider: &str,
    name: &str,
    value: Option<&str>,
) -> AppResult<()> {
    let known = match more::builtin(provider) {
        Some(b) => b.fields,
        None => more::CUSTOM_FIELDS,
    };
    if !known.iter().any(|f| f.key == name) {
        return Err(AppError::msg("这个翻译源没有这一项"));
    }
    state(app)
        .db
        .with(|c| secrets::set(c, &field_key(provider, name), value))?;
    // 换了密钥：以前失败 / 成功的结果都不算数了
    state(app).translate.cache.lock().clear();
    Ok(())
}

fn fields_of(app: &AppHandle, provider: &str, specs: &[more::FieldSpec]) -> Vec<FieldInfo> {
    specs
        .iter()
        .map(|f| {
            let value = secret(app, &field_key(provider, f.key));
            FieldInfo {
                key: f.key.into(),
                label: crate::i18n::text(f.label),
                secret: f.secret,
                required: f.required,
                hint: crate::i18n::text(f.hint),
                set: value.is_some(),
                value: value.map(|v| if f.secret { secrets::masked(&v) } else { v }),
            }
        })
        .collect()
}

/// 全部翻译源，按设置里的顺序。
pub fn list_providers(app: &AppHandle) -> Vec<ProviderInfo> {
    let st = state(app);
    let settings = st.settings.read().translate.clone();
    settings
        .providers
        .iter()
        .filter_map(|p| {
            if let Some(b) = more::builtin(&p.id) {
                let fields = fields_of(app, b.id, b.fields);
                return Some(ProviderInfo {
                    name: crate::i18n::text(b.name),
                    free: b.free,
                    custom: false,
                    kind: None,
                    configured: fields.iter().all(|f| f.set || !f.required),
                    enabled: p.enabled,
                    fields,
                    id: p.id.clone(),
                });
            }
            let c = settings.custom.iter().find(|c| c.id == p.id)?;
            let ready = match c.kind.as_str() {
                "ai" => !c.url.trim().is_empty() && !c.model.trim().is_empty(),
                _ => !c.url.trim().is_empty(),
            };
            Some(ProviderInfo {
                name: c.name.clone(),
                free: false,
                custom: true,
                kind: Some(c.kind.clone()),
                configured: ready,
                enabled: p.enabled,
                fields: fields_of(app, &c.id, more::CUSTOM_FIELDS),
                id: p.id.clone(),
            })
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
            "youdao" => providers::youdao(&ctx).await,
            "google" => providers::google(&ctx).await,
            "deepl" => match secret(app, SECRET_DEEPL) {
                Some(k) => providers::deepl(&ctx, &k).await,
                None => Err(AppError::msg("未填写 DeepL 密钥")),
            },
            "openai" => match secret(app, SECRET_OPENAI) {
                Some(k) => {
                    providers::openai(
                        &ctx,
                        &settings.custom_base_url,
                        &settings.custom_model,
                        &k,
                        "",
                    )
                    .await
                }
                None => Err(AppError::msg("未填写 AI 接口密钥")),
            },
            id @ ("baidu" | "caiyun" | "niutrans" | "azure" | "tencentcloud" | "volcengine") => {
                let get = |name: &str| field(app, id, name);
                let missing = more::builtin(id)
                    .is_some_and(|b| b.fields.iter().any(|f| f.required && get(f.key).is_empty()));
                if missing {
                    Err(AppError::msg("这个翻译源的密钥还没填全"))
                } else {
                    match id {
                        "baidu" => more::baidu(&ctx, &get("appid"), &get("key")).await,
                        "caiyun" => more::caiyun(&ctx, &get("token")).await,
                        "niutrans" => more::niutrans(&ctx, &get("apikey")).await,
                        "azure" => more::azure(&ctx, &get("key"), &get("region")).await,
                        "tencentcloud" => {
                            more::tencent_cloud(
                                &ctx,
                                &get("secretId"),
                                &get("secretKey"),
                                &get("region"),
                            )
                            .await
                        }
                        _ => more::volcengine(&ctx, &get("accessKey"), &get("secretKey")).await,
                    }
                }
            }
            other => match settings.custom.iter().find(|c| c.id == other) {
                Some(c) => {
                    let key = field(app, &c.id, "key");
                    match c.kind.as_str() {
                        "ai" => providers::openai(&ctx, &c.url, &c.model, &key, &c.prompt).await,
                        "deeplx" => more::deeplx(&ctx, &c.url, &key).await,
                        _ => {
                            more::http_template(&ctx, &c.url, &c.headers, &c.body, &c.path, &key)
                                .await
                        }
                    }
                }
                None => Err(AppError::msg(format!("未知翻译源 {other}"))),
            },
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
