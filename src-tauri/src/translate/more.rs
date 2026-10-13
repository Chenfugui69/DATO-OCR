//! 更多翻译源：要自己填密钥的几家官方接口，和用户自己添加的翻译（AI 模型、DeepLX、任意 HTTP 接口）。
//!
//! 官方接口都是照各家文档写的。没有密钥的话没法真的调一次，所以除了签名算法有单元测试之外，
//! 这些源是"按文档实现、没有用真账号验证过"的；设置页每个源都有"检测"按钮，填完密钥点一下就知道通不通。

use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::providers::{md5_hex, normalize_detected, Ctx, Output};
use crate::error::{AppError, AppResult};

/// 一个翻译源要用户填的一项（密钥、应用 ID、区域……）。
pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    /// 密钥类的：界面上只显示打码后的样子
    pub secret: bool,
    pub required: bool,
    pub hint: &'static str,
}

const fn secret(key: &'static str, label: &'static str, hint: &'static str) -> FieldSpec {
    FieldSpec {
        key,
        label,
        secret: true,
        required: true,
        hint,
    }
}

pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    pub free: bool,
    pub fields: &'static [FieldSpec],
}

/// 全部内置翻译源。前四个是免费的网页接口，后面的要自己去各家申请密钥。
pub const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "bing",
        name: "必应翻译",
        free: true,
        fields: &[],
    },
    Builtin {
        id: "transmart",
        name: "腾讯翻译",
        free: true,
        fields: &[],
    },
    Builtin {
        id: "youdao",
        name: "有道翻译",
        free: true,
        fields: &[],
    },
    Builtin {
        id: "google",
        name: "谷歌翻译",
        free: true,
        fields: &[],
    },
    Builtin {
        id: "deepl",
        name: "DeepL",
        free: false,
        fields: &[secret("key", "密钥", "xxxxxxxx-xxxx-...:fx")],
    },
    Builtin {
        id: "openai",
        name: "AI 大模型",
        free: false,
        fields: &[secret("key", "密钥", "sk-...")],
    },
    Builtin {
        id: "baidu",
        name: "百度翻译",
        free: false,
        fields: &[
            FieldSpec {
                key: "appid",
                label: "APP ID",
                secret: false,
                required: true,
                hint: "2015063000000001",
            },
            secret("key", "密钥", ""),
        ],
    },
    Builtin {
        id: "caiyun",
        name: "彩云小译",
        free: false,
        fields: &[secret("token", "令牌", "")],
    },
    Builtin {
        id: "niutrans",
        name: "小牛翻译",
        free: false,
        fields: &[secret("apikey", "API Key", "")],
    },
    Builtin {
        id: "azure",
        name: "微软 Azure 翻译",
        free: false,
        fields: &[
            secret("key", "密钥", ""),
            FieldSpec {
                key: "region",
                label: "区域",
                secret: false,
                required: false,
                hint: "eastasia（全球资源可不填）",
            },
        ],
    },
    Builtin {
        id: "tencentcloud",
        name: "腾讯云机器翻译",
        free: false,
        fields: &[
            secret("secretId", "SecretId", "AKID..."),
            secret("secretKey", "SecretKey", ""),
            FieldSpec {
                key: "region",
                label: "地域",
                secret: false,
                required: false,
                hint: "ap-guangzhou",
            },
        ],
    },
    Builtin {
        id: "volcengine",
        name: "火山翻译",
        free: false,
        fields: &[
            secret("accessKey", "Access Key ID", "AK..."),
            secret("secretKey", "Secret Access Key", ""),
        ],
    },
];

pub fn builtin(id: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.id == id)
}

/// 用户自己加的翻译都有一项可选的密钥
pub const CUSTOM_FIELDS: &[FieldSpec] = &[FieldSpec {
    key: "key",
    label: "密钥",
    secret: true,
    required: false,
    hint: "",
}];

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn hmac_sha256(key: &[u8], data: &str) -> Vec<u8> {
    // HMAC 接受任意长度的密钥，这里不会失败
    let Ok(mut mac) = <Hmac<Sha256> as Mac>::new_from_slice(key) else {
        return Vec::new();
    };
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 接口返回了报错：带上源的名字和它给的说明。
fn refused(name: &str, code: impl std::fmt::Display, message: &str) -> AppError {
    AppError::msg(format!("{name}返回错误 {code}：{message}"))
}

// ───────────────────────── 百度翻译 ─────────────────────────

fn baidu_lang(code: &str) -> &str {
    match code {
        "zh-TW" => "cht",
        "ja" => "jp",
        "ko" => "kor",
        "fr" => "fra",
        "es" => "spa",
        "ar" => "ara",
        "vi" => "vie",
        other => other,
    }
}

pub async fn baidu(ctx: &Ctx<'_>, appid: &str, key: &str) -> AppResult<Output> {
    let salt = uuid::Uuid::new_v4().simple().to_string();
    let sign = md5_hex(&format!("{appid}{}{salt}{key}", ctx.text));
    let resp: Value = ctx
        .client
        .post("https://fanyi-api.baidu.com/api/trans/vip/translate")
        .form(&[
            ("q", ctx.text),
            ("from", ctx.from.map_or("auto", baidu_lang)),
            ("to", baidu_lang(ctx.to)),
            ("appid", appid),
            ("salt", &salt),
            ("sign", &sign),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(code) = resp["error_code"].as_str().filter(|c| *c != "52000") {
        return Err(refused(
            "百度翻译",
            code,
            resp["error_msg"].as_str().unwrap_or_default(),
        ));
    }
    let lines: Vec<&str> = resp["trans_result"]
        .as_array()
        .map(|list| list.iter().filter_map(|r| r["dst"].as_str()).collect())
        .unwrap_or_default();
    Ok(Output {
        text: lines.join("\n"),
        detected: resp["from"].as_str().map(|f| match f {
            "cht" => "zh-TW".to_string(),
            "jp" => "ja".into(),
            "kor" => "ko".into(),
            "fra" => "fr".into(),
            "spa" => "es".into(),
            "ara" => "ar".into(),
            "vie" => "vi".into(),
            other => normalize_detected(other),
        }),
    })
}

// ───────────────────────── 彩云小译 ─────────────────────────

pub async fn caiyun(ctx: &Ctx<'_>, token: &str) -> AppResult<Output> {
    let lang = |code: &str| {
        if code == "zh-TW" {
            "zh-Hant".to_string()
        } else {
            code.to_string()
        }
    };
    // 一行一段地送，回来再按行拼上，段落不会被揉成一团
    let source: Vec<&str> = ctx.text.split('\n').collect();
    let resp: Value = ctx
        .client
        .post("https://api.interpreter.caiyunai.com/v1/translator")
        .header("x-authorization", format!("token {token}"))
        .json(&json!({
            "source": source,
            "trans_type": format!("{}2{}", ctx.from.map_or("auto".into(), lang), lang(ctx.to)),
            "request_id": "dato-ocr",
            "detect": true,
        }))
        .send()
        .await?
        .json()
        .await?;
    let Some(target) = resp["target"].as_array() else {
        return Err(refused(
            "彩云小译",
            "",
            resp["message"].as_str().unwrap_or("令牌无效或额度用完"),
        ));
    };
    let lines: Vec<&str> = target.iter().filter_map(Value::as_str).collect();
    Ok(Output {
        text: lines.join("\n"),
        detected: None,
    })
}

// ───────────────────────── 小牛翻译 ─────────────────────────

pub async fn niutrans(ctx: &Ctx<'_>, apikey: &str) -> AppResult<Output> {
    let lang = |code: &str| {
        if code == "zh-TW" {
            "cht".to_string()
        } else {
            code.to_string()
        }
    };
    let resp: Value = ctx
        .client
        .post("https://api.niutrans.com/NiuTransServer/translation")
        .json(&json!({
            "from": ctx.from.map_or("auto".into(), lang),
            "to": lang(ctx.to),
            "apikey": apikey,
            "src_text": ctx.text,
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    match resp["tgt_text"].as_str() {
        Some(text) => Ok(Output {
            text: text.to_string(),
            detected: None,
        }),
        None => Err(refused(
            "小牛翻译",
            resp["error_code"].as_str().unwrap_or_default(),
            resp["error_msg"].as_str().unwrap_or_default(),
        )),
    }
}

// ───────────────────────── 微软 Azure ─────────────────────────

fn azure_lang(code: &str) -> &str {
    match code {
        "zh" => "zh-Hans",
        "zh-TW" => "zh-Hant",
        other => other,
    }
}

pub async fn azure(ctx: &Ctx<'_>, key: &str, region: &str) -> AppResult<Output> {
    let mut query = vec![("api-version", "3.0"), ("to", azure_lang(ctx.to))];
    if let Some(from) = ctx.from {
        query.push(("from", azure_lang(from)));
    }
    let mut request = ctx
        .client
        .post("https://api.cognitive.microsofttranslator.com/translate")
        .query(&query)
        .header("Ocp-Apim-Subscription-Key", key);
    if !region.trim().is_empty() {
        request = request.header("Ocp-Apim-Subscription-Region", region.trim());
    }
    let resp: Value = request
        .json(&json!([{ "Text": ctx.text }]))
        .send()
        .await?
        .json()
        .await?;
    if let Some(err) = resp.get("error") {
        return Err(refused(
            "Azure 翻译",
            &err["code"],
            err["message"].as_str().unwrap_or_default(),
        ));
    }
    let first = &resp[0];
    Ok(Output {
        text: first["translations"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        detected: first["detectedLanguage"]["language"]
            .as_str()
            .map(normalize_detected),
    })
}

// ───────────────────────── 腾讯云机器翻译（TC3 签名） ─────────────────────────

const TENCENT_HOST: &str = "tmt.tencentcloudapi.com";

/// TC3-HMAC-SHA256 的 Authorization 头。`timestamp` 是 Unix 秒，`date` 是它对应的 UTC 日期（YYYY-MM-DD）。
fn tencent_authorization(
    secret_id: &str,
    secret_key: &str,
    payload: &str,
    timestamp: i64,
    date: &str,
) -> String {
    let canonical = format!(
        "POST\n/\n\ncontent-type:application/json; charset=utf-8\nhost:{TENCENT_HOST}\nx-tc-action:texttranslate\n\n\
         content-type;host;x-tc-action\n{}",
        sha256_hex(payload.as_bytes())
    );
    let scope = format!("{date}/tmt/tc3_request");
    let to_sign = format!(
        "TC3-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        sha256_hex(canonical.as_bytes())
    );
    let k_date = hmac_sha256(format!("TC3{secret_key}").as_bytes(), date);
    let k_service = hmac_sha256(&k_date, "tmt");
    let k_signing = hmac_sha256(&k_service, "tc3_request");
    let signature = hex(&hmac_sha256(&k_signing, &to_sign));
    format!(
        "TC3-HMAC-SHA256 Credential={secret_id}/{scope}, SignedHeaders=content-type;host;x-tc-action, Signature={signature}"
    )
}

pub async fn tencent_cloud(
    ctx: &Ctx<'_>,
    secret_id: &str,
    secret_key: &str,
    region: &str,
) -> AppResult<Output> {
    let payload = json!({
        "SourceText": ctx.text,
        "Source": ctx.from.unwrap_or("auto"),
        "Target": ctx.to,
        "ProjectId": 0,
    })
    .to_string();
    let now = chrono::Utc::now();
    let authorization = tencent_authorization(
        secret_id,
        secret_key,
        &payload,
        now.timestamp(),
        &now.format("%Y-%m-%d").to_string(),
    );
    let region = if region.trim().is_empty() {
        "ap-guangzhou"
    } else {
        region.trim()
    };
    let resp: Value = ctx
        .client
        .post(format!("https://{TENCENT_HOST}"))
        .header("Authorization", authorization)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("X-TC-Action", "TextTranslate")
        .header("X-TC-Timestamp", now.timestamp().to_string())
        .header("X-TC-Version", "2018-03-21")
        .header("X-TC-Region", region)
        .body(payload)
        .send()
        .await?
        .json()
        .await?;
    let body = &resp["Response"];
    if let Some(err) = body.get("Error") {
        return Err(refused(
            "腾讯云翻译",
            &err["Code"],
            err["Message"].as_str().unwrap_or_default(),
        ));
    }
    Ok(Output {
        text: body["TargetText"].as_str().unwrap_or_default().to_string(),
        detected: body["Source"].as_str().map(normalize_detected),
    })
}

// ───────────────────────── 火山翻译（V4 签名） ─────────────────────────

const VOLC_HOST: &str = "translate.volcengineapi.com";
const VOLC_QUERY: &str = "Action=TranslateText&Version=2020-06-01";

/// 火山引擎的 Authorization 头。`x_date` 是 `YYYYMMDDTHHMMSSZ`。
fn volc_authorization(access_key: &str, secret_key: &str, body: &str, x_date: &str) -> String {
    let (region, service) = ("cn-north-1", "translate");
    let short = &x_date[..8];
    let hash = sha256_hex(body.as_bytes());
    let canonical = format!(
        "POST\n/\n{VOLC_QUERY}\ncontent-type:application/json\nhost:{VOLC_HOST}\nx-content-sha256:{hash}\nx-date:{x_date}\n\n\
         content-type;host;x-content-sha256;x-date\n{hash}"
    );
    let scope = format!("{short}/{region}/{service}/request");
    let to_sign = format!(
        "HMAC-SHA256\n{x_date}\n{scope}\n{}",
        sha256_hex(canonical.as_bytes())
    );
    let k_date = hmac_sha256(secret_key.as_bytes(), short);
    let k_region = hmac_sha256(&k_date, region);
    let k_service = hmac_sha256(&k_region, service);
    let k_signing = hmac_sha256(&k_service, "request");
    let signature = hex(&hmac_sha256(&k_signing, &to_sign));
    format!(
        "HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders=content-type;host;x-content-sha256;x-date, Signature={signature}"
    )
}

pub async fn volcengine(ctx: &Ctx<'_>, access_key: &str, secret_key: &str) -> AppResult<Output> {
    let lang = |code: &str| {
        if code == "zh-TW" {
            "zh-Hant".to_string()
        } else {
            code.to_string()
        }
    };
    let mut body = json!({ "TargetLanguage": lang(ctx.to), "TextList": [ctx.text] });
    if let Some(from) = ctx.from {
        body["SourceLanguage"] = json!(lang(from));
    }
    let body = body.to_string();
    let x_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let resp: Value = ctx
        .client
        .post(format!("https://{VOLC_HOST}/?{VOLC_QUERY}"))
        .header(
            "Authorization",
            volc_authorization(access_key, secret_key, &body, &x_date),
        )
        .header("Content-Type", "application/json")
        .header("X-Date", &x_date)
        .header("X-Content-Sha256", sha256_hex(body.as_bytes()))
        .body(body)
        .send()
        .await?
        .json()
        .await?;
    if let Some(err) = resp["ResponseMetadata"].get("Error") {
        return Err(refused(
            "火山翻译",
            &err["Code"],
            err["Message"].as_str().unwrap_or_default(),
        ));
    }
    let first = &resp["TranslationList"][0];
    Ok(Output {
        text: first["Translation"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        detected: first["DetectedSourceLanguage"]
            .as_str()
            .map(normalize_detected),
    })
}

// ───────────────────────── 自己添加的：DeepLX ─────────────────────────

pub async fn deeplx(ctx: &Ctx<'_>, url: &str, key: &str) -> AppResult<Output> {
    let up = |code: &str| {
        if code.starts_with("zh") {
            "ZH".to_string()
        } else {
            code.to_uppercase()
        }
    };
    let mut request = ctx.client.post(url.trim()).json(&json!({
        "text": ctx.text,
        "source_lang": ctx.from.map_or("auto".into(), up),
        "target_lang": up(ctx.to),
    }));
    if !key.is_empty() {
        request = request.bearer_auth(key);
    }
    let resp: Value = request.send().await?.error_for_status()?.json().await?;
    match resp["data"].as_str() {
        Some(text) => Ok(Output {
            text: text.to_string(),
            detected: resp["source_lang"].as_str().map(normalize_detected),
        }),
        None => Err(AppError::msg(format!(
            "DeepLX 没有返回译文：{}",
            resp["message"].as_str().unwrap_or("接口返回的格式不对")
        ))),
    }
}

// ───────────────────────── 自己添加的：任意 HTTP 接口 ─────────────────────────

/// 把模板里的 `{text}` `{from}` `{to}` `{key}` 换成实际的值。`json` = 值要放进 JSON 字符串里（转义引号、换行）。
fn fill(template: &str, ctx: &Ctx<'_>, key: &str, json: bool) -> String {
    let esc = |v: &str| {
        if json {
            let quoted = Value::String(v.to_string()).to_string();
            quoted[1..quoted.len() - 1].to_string()
        } else {
            v.to_string()
        }
    };
    template
        .replace("{text}", &esc(ctx.text))
        .replace("{from}", &esc(ctx.from.unwrap_or("auto")))
        .replace("{to}", &esc(ctx.to))
        .replace("{key}", &esc(key))
}

/// 按 `a.b.0.c` 这样的路径从返回的 JSON 里取译文。
fn pick<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .filter(|p| !p.is_empty())
        .try_fold(value, |v, part| match part.parse::<usize>() {
            Ok(index) if v.is_array() => v.get(index),
            _ => v.get(part),
        })
}

/// 用户自己描述的接口：地址、请求头（一行一个 `名字: 值`）、请求体模板、译文在返回结果里的路径。
/// 请求体留空就发 GET（参数写在地址里）。
pub async fn http_template(
    ctx: &Ctx<'_>,
    url: &str,
    headers: &str,
    body: &str,
    path: &str,
    key: &str,
) -> AppResult<Output> {
    let url = fill(url.trim(), ctx, key, false);
    let mut request = if body.trim().is_empty() {
        ctx.client.get(url)
    } else {
        let filled = fill(body, ctx, key, true);
        let json_body = filled.trim_start().starts_with(['{', '[']);
        let request = ctx.client.post(url).body(filled);
        if json_body {
            request.header("Content-Type", "application/json")
        } else {
            request.header("Content-Type", "application/x-www-form-urlencoded")
        }
    };
    for line in headers.lines() {
        if let Some((name, value)) = line.split_once(':') {
            let (name, value) = (name.trim(), fill(value.trim(), ctx, key, false));
            if !name.is_empty() {
                request = request.header(name, value);
            }
        }
    }
    let response = request.send().await?;
    let status = response.status();
    let raw = response.text().await?;
    if !status.is_success() {
        let brief: String = raw.chars().take(160).collect();
        return Err(AppError::msg(format!("接口返回 {status}：{brief}")));
    }
    // 没写路径：整个返回内容就是译文
    if path.trim().is_empty() {
        return Ok(Output {
            text: raw.trim().to_string(),
            detected: None,
        });
    }
    let value: Value =
        serde_json::from_str(&raw).map_err(|_| AppError::msg("接口返回的不是 JSON，取不出译文"))?;
    let found = pick(&value, path.trim())
        .ok_or_else(|| AppError::msg(format!("返回结果里没有 {path} 这一项")))?;
    let text = match found {
        Value::String(s) => s.clone(),
        // 一段一条的数组：拼起来
        Value::Array(list) => list
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    };
    Ok(Output {
        text,
        detected: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(client: &'a reqwest::Client, text: &'a str) -> Ctx<'a> {
        Ctx {
            client,
            text,
            from: None,
            to: "zh",
        }
    }

    /// 腾讯云文档里的签名算法：密钥派生和最终签名的格式（64 位十六进制，改一个字就变）。
    #[test]
    fn tencent_signature_is_stable() {
        let a = tencent_authorization(
            "AKIDEXAMPLE",
            "secret",
            "{\"a\":1}",
            1_551_113_065,
            "2019-02-25",
        );
        assert!(a.starts_with("TC3-HMAC-SHA256 Credential=AKIDEXAMPLE/2019-02-25/tmt/tc3_request, SignedHeaders=content-type;host;x-tc-action, Signature="));
        let sig = a.rsplit('=').next().unwrap();
        assert_eq!(sig.len(), 64);
        assert_eq!(
            a,
            tencent_authorization(
                "AKIDEXAMPLE",
                "secret",
                "{\"a\":1}",
                1_551_113_065,
                "2019-02-25"
            )
        );
        assert_ne!(
            a,
            tencent_authorization(
                "AKIDEXAMPLE",
                "secret",
                "{\"a\":2}",
                1_551_113_065,
                "2019-02-25"
            )
        );
        assert_ne!(
            a,
            tencent_authorization(
                "AKIDEXAMPLE",
                "other",
                "{\"a\":1}",
                1_551_113_065,
                "2019-02-25"
            )
        );
    }

    #[test]
    fn volc_signature_shape() {
        let a = volc_authorization("AKLT", "secret", "{}", "20240101T000000Z");
        assert!(a.starts_with("HMAC-SHA256 Credential=AKLT/20240101/cn-north-1/translate/request, SignedHeaders=content-type;host;x-content-sha256;x-date, Signature="));
        assert_eq!(a.rsplit('=').next().unwrap().len(), 64);
    }

    /// HMAC-SHA256 的标准测试向量（RFC 4231 第 2 例）
    #[test]
    fn hmac_matches_rfc4231() {
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", "what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    /// 真的发一次请求（要联网，默认忽略）：`cargo test -- --ignored smoke_http_template --nocapture`。
    /// 用回显接口验证"填模板 → 发请求 → 按路径取结果"这一整条路。
    #[tokio::test]
    #[ignore]
    async fn smoke_http_template() {
        let client = reqwest::Client::new();
        let c = ctx(&client, "say \"hi\"\nplease");
        let out = http_template(
            &c,
            "https://httpbin.org/post?to={to}",
            "X-Token: {key}",
            r#"{"text": "{text}", "target": "{to}"}"#,
            "json.text",
            "K1",
        )
        .await
        .unwrap();
        assert_eq!(out.text, "say \"hi\"\nplease");
        let echoed = http_template(
            &c,
            "https://httpbin.org/post",
            "X-Token: {key}",
            "{}",
            "headers.X-Token",
            "K1",
        )
        .await
        .unwrap();
        assert_eq!(echoed.text, "K1");
        println!("http_template 联网验证通过");
    }

    #[test]
    fn template_fill_and_pick() {
        let client = reqwest::Client::new();
        let c = ctx(&client, "say \"hi\"\nplease");
        assert_eq!(
            fill(
                r#"{"q":"{text}","to":"{to}","from":"{from}","k":"{key}"}"#,
                &c,
                "K1",
                true
            ),
            r#"{"q":"say \"hi\"\nplease","to":"zh","from":"auto","k":"K1"}"#
        );
        assert_eq!(
            fill("https://x/?to={to}&k={key}", &c, "K1", false),
            "https://x/?to=zh&k=K1"
        );
        let v: Value =
            serde_json::from_str(r#"{"data":{"translations":[{"text":"你好"}]},"list":["a","b"]}"#)
                .unwrap();
        assert_eq!(
            pick(&v, "data.translations.0.text").and_then(Value::as_str),
            Some("你好")
        );
        assert!(pick(&v, "data.missing").is_none());
        assert!(pick(&v, "list").is_some_and(Value::is_array));
    }
}
