//! 翻译源。**接口地址和签名逻辑全部收敛在这个文件**，失效时只改这里（规格 04 §6.1）。
//!
//! 内置免费源都是非官方接口，随时可能失效 —— 上层负责失败自动降级。
//!
//! | id | 说明 |
//! |---|---|
//! | bing | 必应翻译网页版接口，国内可直连 |
//! | transmart | 腾讯交互翻译的浏览器插件接口，国内可直连 |
//! | youdao | 有道翻译网页版接口（先取一次密钥，返回内容是加密的），国内可直连 |
//! | google | Chrome 划词扩展用的 clients5 接口；不行再退到 gtx 客户端。国内直连不通 |
//! | deepl | 用户自填 API Key（Free 版 key 以 `:fx` 结尾） |
//! | openai | 任意 OpenAI 兼容接口（OpenAI / DeepSeek / 通义 / Kimi …） |

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};

pub struct Ctx<'a> {
    pub client: &'a reqwest::Client,
    pub text: &'a str,
    /// 内部语言码：zh / zh-TW / en / ja / ko / fr / de / es / ru / pt / it / ar / th / vi；None = 自动
    pub from: Option<&'a str>,
    pub to: &'a str,
}

pub struct Output {
    pub text: String,
    pub detected: Option<String>,
}

// ───────────────────────── 必应（网页版） ─────────────────────────
//
// 先 GET 翻译页拿 IG / IID / key / token（页面里的 params_AbusePreventionHelper，
// 有效期约 1 小时），再 POST ttranslatev3。原来 Edge 内置翻译用的
// edge.microsoft.com/translate/auth 已经下线（404）。

struct BingToken {
    ig: String,
    iid: String,
    key: String,
    token: String,
    fetched: Instant,
    ttl: Duration,
}

static BING: Mutex<Option<BingToken>> = Mutex::new(None);

fn bing_lang(code: &str) -> &str {
    match code {
        "zh" => "zh-Hans",
        "zh-TW" => "zh-Hant",
        other => other,
    }
}

fn between<'a>(html: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = html.find(start)? + start.len();
    let j = html[i..].find(end)? + i;
    Some(&html[i..j])
}

async fn bing_token(
    client: &reqwest::Client,
    force: bool,
) -> AppResult<(String, String, String, String)> {
    if !force {
        if let Some(t) = BING.lock().as_ref() {
            if t.fetched.elapsed() < t.ttl {
                return Ok((t.ig.clone(), t.iid.clone(), t.key.clone(), t.token.clone()));
            }
        }
    }
    let html = client
        .get("https://www.bing.com/translator")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let bad = || AppError::Network("必应翻译页面格式已变化".into());
    let ig = between(&html, "IG:\"", "\"").ok_or_else(bad)?.to_string();
    let iid = between(&html, "data-iid=\"", "\"")
        .unwrap_or("translator.5028")
        .to_string();
    let params = between(&html, "params_AbusePreventionHelper = [", "]").ok_or_else(bad)?;
    let mut parts = params.split(',');
    let key = parts.next().ok_or_else(bad)?.trim().to_string();
    let token = parts
        .next()
        .ok_or_else(bad)?
        .trim()
        .trim_matches('"')
        .to_string();
    let ttl_ms: u64 = parts
        .next()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(3_600_000);
    *BING.lock() = Some(BingToken {
        ig: ig.clone(),
        iid: iid.clone(),
        key: key.clone(),
        token: token.clone(),
        fetched: Instant::now(),
        // 提前 5 分钟换新
        ttl: Duration::from_millis(ttl_ms.saturating_sub(300_000)),
    });
    Ok((ig, iid, key, token))
}

pub async fn bing(ctx: &Ctx<'_>) -> AppResult<Output> {
    for attempt in 0..2 {
        let (ig, iid, key, token) = bing_token(ctx.client, attempt > 0).await?;
        let resp = ctx
            .client
            .post(format!(
                "https://www.bing.com/ttranslatev3?isVertical=1&IG={ig}&IID={iid}"
            ))
            .form(&[
                ("fromLang", ctx.from.map(bing_lang).unwrap_or("auto-detect")),
                ("to", bing_lang(ctx.to)),
                ("text", ctx.text),
                ("token", token.as_str()),
                ("key", key.as_str()),
            ])
            .send()
            .await?;
        let body: Value = match resp.error_for_status() {
            Ok(r) => r.json().await.unwrap_or(Value::Null),
            Err(err) if attempt == 0 => {
                tracing::debug!("必应翻译令牌可能过期，刷新重试：{err}");
                continue;
            }
            Err(err) => return Err(err.into()),
        };
        let item = &body[0];
        match item["translations"][0]["text"].as_str() {
            Some(text) => {
                return Ok(Output {
                    text: text.to_string(),
                    detected: item["detectedLanguage"]["language"]
                        .as_str()
                        .map(normalize_detected),
                })
            }
            // 令牌失效时返回的是 {"statusCode":205} 之类，刷新一次再试
            None if attempt == 0 => continue,
            None => return Err(AppError::Network("必应翻译返回格式异常".into())),
        }
    }
    Err(AppError::Network("必应翻译不可用".into()))
}

// ───────────────────────── 腾讯交互翻译（TranSmart） ─────────────────────────
//
// 浏览器插件用的公开接口，不需要密钥。必须给出源语言（用本地检测结果）。
// 按行拆成 text_list 发送，译文逐行对应，原文的换行结构得以保留。

pub async fn transmart(ctx: &Ctx<'_>, detected: &str) -> AppResult<Output> {
    let lines: Vec<&str> = ctx.text.split('\n').collect();
    let source = ctx.from.unwrap_or(detected);
    let body = json!({
        "header": {
            "fn": "auto_translation",
            "client_key": format!("browser-chrome-130.0-Windows_10-{}", uuid::Uuid::new_v4().simple()),
        },
        "type": "plain",
        "model_category": "normal",
        "source": { "lang": source, "text_list": lines },
        "target": { "lang": ctx.to },
    });
    let resp: Value = ctx
        .client
        .post("https://transmart.qq.com/api/imt")
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if resp["header"]["ret_code"].as_str() != Some("succ") {
        return Err(AppError::Network(format!(
            "腾讯翻译返回错误：{}",
            resp["header"]["ret_code"].as_str().unwrap_or("未知")
        )));
    }
    let out = resp["auto_translation"]
        .as_array()
        .ok_or_else(|| AppError::Network("腾讯翻译返回格式异常".into()))?
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Output {
        text: out,
        detected: resp["src_lang"].as_str().map(normalize_detected),
    })
}

// ───────────────────────── 有道（网页版） ─────────────────────────
//
// 有道翻译网页（fanyi.youdao.com）用的接口：先 GET webtranslate/key 拿 secretKey / aesKey / aesIv，
// 翻译请求按 secretKey 签名（MD5），返回的是 URL 安全 base64 的 AES-128-CBC 密文，
// 密钥和 IV 分别是 aesKey、aesIv 的 MD5。密钥一般不变，缓存一小时，失败时刷新一次再试。

struct YoudaoKeys {
    secret: String,
    aes_key: [u8; 16],
    aes_iv: [u8; 16],
    fetched: Instant,
}

static YOUDAO: Mutex<Option<YoudaoKeys>> = Mutex::new(None);
const YOUDAO_KEY_TTL: Duration = Duration::from_secs(3600);
const YOUDAO_KEY_SIGN: &str = "asdjnjfenknafdfsdfsd";

fn md5_hex(text: &str) -> String {
    use md5::{Digest, Md5};
    Md5::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn md5_bytes(text: &str) -> [u8; 16] {
    use md5::{Digest, Md5};
    Md5::digest(text.as_bytes()).into()
}

fn youdao_lang(code: &str) -> &str {
    match code {
        "zh" => "zh-CHS",
        "zh-TW" => "zh-CHT",
        other => other,
    }
}

/// 公共参数 + 按 `key` 算的签名
fn youdao_params(key: &str) -> Vec<(&'static str, String)> {
    let t = chrono::Utc::now().timestamp_millis().to_string();
    let sign = md5_hex(&format!(
        "client=fanyideskweb&mysticTime={t}&product=webfanyi&key={key}"
    ));
    vec![
        ("sign", sign),
        ("client", "fanyideskweb".into()),
        ("product", "webfanyi".into()),
        ("appVersion", "1.0.0".into()),
        ("vendor", "web".into()),
        ("pointParam", "client,mysticTime,product".into()),
        ("mysticTime", t),
        ("keyfrom", "fanyi.web".into()),
    ]
}

fn youdao_request(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    // 不带 Referer 和这个 cookie 时接口会拒绝
    let user = format!(
        "OUTFOX_SEARCH_USER_ID={}@10.110.96.157",
        -(i64::from(uuid::Uuid::new_v4().as_fields().0 % 1_000_000_000))
    );
    builder
        .header("Referer", "https://fanyi.youdao.com/")
        .header("Origin", "https://fanyi.youdao.com")
        .header("Cookie", user)
}

async fn youdao_keys(
    client: &reqwest::Client,
    force: bool,
) -> AppResult<(String, [u8; 16], [u8; 16])> {
    if !force {
        if let Some(k) = YOUDAO.lock().as_ref() {
            if k.fetched.elapsed() < YOUDAO_KEY_TTL {
                return Ok((k.secret.clone(), k.aes_key, k.aes_iv));
            }
        }
    }
    let mut query = youdao_params(YOUDAO_KEY_SIGN);
    query.push(("keyid", "webfanyi-key-getter".into()));
    let resp: Value = youdao_request(client.get("https://dict.youdao.com/webtranslate/key"))
        .query(&query)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let data = &resp["data"];
    let (Some(secret), Some(key), Some(iv)) = (
        data["secretKey"].as_str(),
        data["aesKey"].as_str(),
        data["aesIv"].as_str(),
    ) else {
        return Err(AppError::Network("有道翻译密钥接口格式已变化".into()));
    };
    let keys = (secret.to_string(), md5_bytes(key), md5_bytes(iv));
    *YOUDAO.lock() = Some(YoudaoKeys {
        secret: keys.0.clone(),
        aes_key: keys.1,
        aes_iv: keys.2,
        fetched: Instant::now(),
    });
    Ok(keys)
}

fn youdao_decrypt(body: &str, key: &[u8; 16], iv: &[u8; 16]) -> AppResult<Value> {
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use base64::Engine as _;
    let bad = || AppError::Network("有道翻译返回内容无法解密".into());
    let mut raw = base64::engine::general_purpose::URL_SAFE
        .decode(body.trim())
        .map_err(|_| bad())?;
    let plain = cbc::Decryptor::<aes::Aes128>::new(key.into(), iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut raw)
        .map_err(|_| bad())?;
    serde_json::from_slice(plain).map_err(|_| bad())
}

/// 有道的结果：每段一个数组，段里每句一个 `tgt`；段尾的换行在 tgt 里，统一去掉再按段换行拼回去。
fn youdao_text(resp: &Value) -> Option<String> {
    let paragraphs = resp["translateResult"].as_array()?;
    let text = paragraphs
        .iter()
        .map(|p| {
            p.as_array()
                .map(|sentences| {
                    sentences
                        .iter()
                        .filter_map(|s| s["tgt"].as_str())
                        .collect::<String>()
                })
                .unwrap_or_default()
                .trim_end_matches(['\r', '\n'])
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(text)
}

pub async fn youdao(ctx: &Ctx<'_>) -> AppResult<Output> {
    for attempt in 0..2 {
        let (secret, key, iv) = youdao_keys(ctx.client, attempt > 0).await?;
        let mut form = youdao_params(&secret);
        form.extend([
            ("i", ctx.text.to_string()),
            (
                "from",
                ctx.from.map(youdao_lang).unwrap_or("auto").to_string(),
            ),
            ("to", youdao_lang(ctx.to).to_string()),
            ("useTerm", "false".into()),
            ("dictResult", "false".into()),
            ("keyid", "webfanyi".into()),
        ]);
        let body = youdao_request(ctx.client.post("https://dict.youdao.com/webtranslate"))
            .form(&form)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let resp = match youdao_decrypt(&body, &key, &iv) {
            Ok(v) => v,
            // 密钥换了：刷新一次再试
            Err(_) if attempt == 0 => continue,
            Err(err) => return Err(err),
        };
        if resp["code"].as_i64() != Some(0) {
            if attempt == 0 {
                continue;
            }
            return Err(AppError::Network(format!(
                "有道翻译返回错误：{}",
                resp["code"]
            )));
        }
        let text =
            youdao_text(&resp).ok_or_else(|| AppError::Network("有道翻译返回格式异常".into()))?;
        // type 形如 "en2zh-CHS"
        let detected = resp["type"]
            .as_str()
            .and_then(|t| t.split('2').next())
            .map(normalize_detected);
        return Ok(Output { text, detected });
    }
    Err(AppError::Network("有道翻译不可用".into()))
}

// ───────────────────────── Google (gtx) ─────────────────────────

fn google_lang(code: &str) -> &str {
    match code {
        "zh" => "zh-CN",
        other => other,
    }
}

// 优先用 Chrome 划词扩展的接口：老的 gtx 客户端对很多代理出口 IP 直接回 429（实测 2026-10），
// 这个接口同一出口仍正常。它不行再退回 gtx。

pub async fn google(ctx: &Ctx<'_>) -> AppResult<Output> {
    match google_dict(ctx).await {
        Ok(out) if !out.text.trim().is_empty() => Ok(out),
        Ok(_) => google_gtx(ctx).await,
        Err(err) => {
            tracing::debug!("谷歌 clients5 接口失败，改用 gtx：{err}");
            google_gtx(ctx).await
        }
    }
}

async fn google_dict(ctx: &Ctx<'_>) -> AppResult<Output> {
    let url = format!(
        "https://clients5.google.com/translate_a/t?client=dict-chrome-ex&sl={}&tl={}",
        ctx.from.map(google_lang).unwrap_or("auto"),
        google_lang(ctx.to)
    );
    let resp: Value = ctx
        .client
        .post(url)
        .form(&[("q", ctx.text)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    // 自动检测时是 [["译文","en"]]，指定源语言时是 ["译文"]
    let first = &resp[0];
    let (text, detected) = match first.as_array() {
        Some(pair) => (
            pair.first().and_then(Value::as_str),
            pair.get(1).and_then(Value::as_str),
        ),
        None => (first.as_str(), None),
    };
    let text = text.ok_or_else(|| AppError::Network("谷歌翻译返回格式异常".into()))?;
    Ok(Output {
        text: text.to_string(),
        detected: detected.map(normalize_detected),
    })
}

async fn google_gtx(ctx: &Ctx<'_>) -> AppResult<Output> {
    let url = format!(
        "https://translate.googleapis.com/translate_a/single?client=gtx&dt=t&sl={}&tl={}",
        ctx.from.map(google_lang).unwrap_or("auto"),
        google_lang(ctx.to)
    );
    let resp: Value = ctx
        .client
        .post(url)
        .form(&[("q", ctx.text)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let segments = resp[0]
        .as_array()
        .ok_or_else(|| AppError::Network("谷歌翻译返回格式异常".into()))?;
    let text: String = segments.iter().filter_map(|s| s[0].as_str()).collect();
    Ok(Output {
        text,
        detected: resp[2].as_str().map(normalize_detected),
    })
}

// ───────────────────────── DeepL ─────────────────────────

fn deepl_lang(code: &str) -> String {
    match code {
        "zh" | "zh-TW" => "ZH".into(),
        "en" => "EN".into(),
        "pt" => "PT-PT".into(),
        other => other.to_uppercase(),
    }
}

pub async fn deepl(ctx: &Ctx<'_>, key: &str) -> AppResult<Output> {
    let host = if key.ends_with(":fx") {
        "https://api-free.deepl.com"
    } else {
        "https://api.deepl.com"
    };
    let mut body = json!({ "text": [ctx.text], "target_lang": deepl_lang(ctx.to) });
    if let Some(from) = ctx.from {
        let src = deepl_lang(from);
        body["source_lang"] = json!(src.split('-').next().unwrap_or(&src));
    }
    let resp: Value = ctx
        .client
        .post(format!("{host}/v2/translate"))
        .header("Authorization", format!("DeepL-Auth-Key {key}"))
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let t = &resp["translations"][0];
    Ok(Output {
        text: t["text"].as_str().unwrap_or_default().to_string(),
        detected: t["detected_source_language"]
            .as_str()
            .map(normalize_detected),
    })
}

// ───────────────────────── OpenAI 兼容 ─────────────────────────

fn language_name(code: &str) -> &str {
    match code {
        "zh" => "Simplified Chinese",
        "zh-TW" => "Traditional Chinese",
        "en" => "English",
        "ja" => "Japanese",
        "ko" => "Korean",
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "ru" => "Russian",
        "pt" => "Portuguese",
        "it" => "Italian",
        "ar" => "Arabic",
        "th" => "Thai",
        "vi" => "Vietnamese",
        other => other,
    }
}

pub async fn openai(ctx: &Ctx<'_>, base_url: &str, model: &str, key: &str) -> AppResult<Output> {
    // 提示词写死，不暴露给用户（规格 04 §6.2）
    let system = format!(
        "You are a translation engine. Translate the user's text into {}.\n\
         Output ONLY the translation, no explanations, no quotes, no markdown.\n\
         Preserve the original line breaks and paragraph structure.\n\
         If the text is already in {}, translate it into English instead.",
        language_name(ctx.to),
        language_name(ctx.to)
    );
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let resp: Value = ctx
        .client
        .post(url)
        .bearer_auth(key)
        .json(&json!({
            "model": model,
            "temperature": 0.2,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": ctx.text }
            ]
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let text = resp["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| AppError::Network("模型返回格式异常".into()))?;
    Ok(Output {
        text: text.trim().to_string(),
        detected: None,
    })
}

/// 各家返回的检测语言码 → 内部语言码。
fn normalize_detected(code: &str) -> String {
    let lower = code.to_ascii_lowercase();
    match lower.as_str() {
        "zh-hans" | "zh-cn" | "zh-chs" | "zh" => "zh".into(),
        "zh-hant" | "zh-tw" | "zh-hk" | "zh-cht" => "zh-TW".into(),
        other => other.split('-').next().unwrap_or(other).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真的联网请求有道：`cargo test youdao_live -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn youdao_live() {
        let client = reqwest::Client::new();
        for (text, from, to) in [
            ("今天天气不错\n第二行文字。", None, "en"),
            ("Hello world", Some("en"), "ja"),
            ("Bonjour le monde", None, "zh-TW"),
        ] {
            let out = youdao(&Ctx {
                client: &client,
                text,
                from,
                to,
            })
            .await
            .unwrap();
            println!("{text:?} -> {:?} ({:?})", out.text, out.detected);
            assert!(!out.text.trim().is_empty());
        }
    }
}
