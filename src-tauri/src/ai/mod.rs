//! AI 对话。
//!
//! - 服务商、模型都由用户配置（设置 → AI 对话）：填接口地址和密钥，点"检测模型"拉出
//!   服务商的模型列表，再挑要用的加进来。密钥用 DPAPI 加密存在数据库里，不进 settings.json
//! - 两种接口格式：OpenAI 兼容（市面上绝大多数服务和本地 Ollama）、Anthropic Messages
//! - 回答是流式的，通过 Tauri Channel 一段段推给前端；前端随时可以停（`cancel`）
//! - 上下文可以带图片（截图）：从内存图库或文件读出，长边超过 2000 像素先缩小，编 PNG 发出去
//!
//! 入口：划词翻译面板里展开、识字窗口、截图工具条、独立的 AI 窗口。各入口只是给对话组件
//! 不同的初始上下文（`AiContext`），对话本身是同一套。

pub mod anthropic;
pub mod openai;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use base64::Engine as _;
use image::RgbaImage;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, AppResult};
use crate::net::{self, Purpose};
use crate::settings::AiProvider;
use crate::state::state;
use crate::storage::secrets;
use crate::{events, imaging, wm};

pub const WINDOW: &str = "ai";
/// 发给模型的图片长边上限
const MAX_IMAGE_SIDE: u32 = 2000;

fn secret_key(provider_id: &str) -> String {
    format!("ai_key:{provider_id}")
}

pub fn key_of(app: &AppHandle, provider_id: &str) -> Option<String> {
    state(app)
        .db
        .with(|c| secrets::get(c, &secret_key(provider_id)))
        .ok()
        .flatten()
        .filter(|k| !k.trim().is_empty())
}

pub fn set_key(app: &AppHandle, provider_id: &str, key: Option<&str>) -> AppResult<()> {
    let key = key.map(str::trim).filter(|k| !k.is_empty());
    state(app)
        .db
        .with(|c| secrets::set(c, &secret_key(provider_id), key))
}

/// 每个服务商的密钥（打码后的），没设置的不在表里。
pub fn masked_keys(app: &AppHandle) -> HashMap<String, String> {
    let providers = state(app).settings.read().ai.providers.clone();
    providers
        .iter()
        .filter_map(|p| key_of(app, &p.id).map(|k| (p.id.clone(), secrets::masked(&k))))
        .collect()
}

// ───────────────────────── 请求 / 事件 ─────────────────────────

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ChatImage {
    /// 内存图库里的图（截图）
    Store { id: String },
    /// 磁盘上的图（识字记录、截图库）
    File { path: String },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    /// user | assistant
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub images: Vec<ChatImage>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    /// 前端生成的请求 id，停止时用
    pub id: String,
    /// `服务商id/模型名`；空 = 默认模型
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ChatEvent {
    /// 开始回答了（告诉前端实际用的是哪个模型）
    Start {
        model: String,
    },
    Delta {
        text: String,
    },
    Done,
    Error {
        message: String,
    },
}

/// 发给接口适配层的参数。
pub struct Params {
    pub model: String,
    pub system: String,
    pub temperature: f64,
    pub max_tokens: u32,
}

/// 编码好的一条消息：图片已转成 base64 PNG。
pub struct Prepared {
    pub role: String,
    pub text: String,
    pub images: Vec<String>,
}

/// 把服务的错误回复整理成一句人话。
pub(crate) async fn api_error(resp: reqwest::Response) -> AppError {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .or_else(|| v["message"].as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| text.chars().take(200).collect());
    let hint = match status.as_u16() {
        401 | 403 => "密钥无效或没有权限",
        404 => "接口地址或模型名不对",
        429 => "请求太频繁或额度用完了",
        _ => "服务返回错误",
    };
    AppError::Network(format!("{hint}（{status}）：{detail}"))
}

/// Server-Sent Events 的 `data:` 行。按字节攒，凑够一整行再解码（多字节字符可能被拆在两块里）。
#[derive(Default)]
pub(crate) struct Sse {
    buf: Vec<u8>,
}

impl Sse {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(data) = line.trim_end().strip_prefix("data:") {
                out.push(data.trim_start().to_string());
            }
        }
        out
    }
}

// ───────────────────────── 模型 ─────────────────────────

fn provider(app: &AppHandle, id: &str) -> AppResult<AiProvider> {
    state(app)
        .settings
        .read()
        .ai
        .providers
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| AppError::msg("找不到这个服务商"))
}

/// 向服务商要模型列表（"检测模型"按钮）。
pub async fn list_models(app: &AppHandle, provider_id: &str) -> AppResult<Vec<String>> {
    let p = provider(app, provider_id)?;
    if p.base_url.is_empty() {
        return Err(AppError::msg("先填写接口地址"));
    }
    let key = key_of(app, &p.id);
    let client = net::client(app, Purpose::Ai);
    match p.kind.as_str() {
        "anthropic" => anthropic::list_models(&client, &p.base_url, key.as_deref()).await,
        _ => openai::list_models(&client, &p.base_url, key.as_deref()).await,
    }
}

/// 按 `服务商id/模型名` 找模型；找不到就用默认模型，再找不到用第一个可用的。
fn resolve(app: &AppHandle, wanted: Option<&str>) -> AppResult<(AiProvider, String)> {
    let ai = state(app).settings.read().ai.clone();
    let pick = |spec: &str| {
        let (pid, model) = spec.split_once('/')?;
        ai.providers
            .iter()
            .find(|p| p.enabled && p.id == pid && p.models.iter().any(|m| m == model))
            .map(|p| (p.clone(), model.to_string()))
    };
    wanted
        .filter(|s| !s.is_empty())
        .and_then(pick)
        .or_else(|| pick(&ai.default_model))
        .or_else(|| {
            ai.providers
                .iter()
                .find(|p| p.enabled && !p.models.is_empty())
                .and_then(|p| p.models.first().map(|m| (p.clone(), m.clone())))
        })
        .ok_or_else(|| {
            AppError::msg("还没有可用的 AI 模型：请先在 设置 → AI 对话 里添加服务商和模型")
        })
}

// ───────────────────────── 对话 ─────────────────────────

static RUNNING: Mutex<Option<HashMap<String, tauri::async_runtime::JoinHandle<()>>>> =
    Mutex::new(None);

fn load_image(app: &AppHandle, image: &ChatImage) -> AppResult<Arc<RgbaImage>> {
    match image {
        ChatImage::Store { id } => state(app)
            .images
            .get(id)
            .ok_or_else(|| AppError::msg("图片已经释放，请重新截图")),
        ChatImage::File { path } => {
            let path = std::path::Path::new(path);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                state(app).paths.abs(&path.to_string_lossy())
            };
            Ok(Arc::new(image::open(path)?.to_rgba8()))
        }
    }
}

fn encode_image(img: &RgbaImage) -> AppResult<String> {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    let png = if long > MAX_IMAGE_SIDE {
        let s = f64::from(MAX_IMAGE_SIDE) / f64::from(long);
        let small = image::imageops::resize(
            img,
            ((f64::from(w) * s).round() as u32).max(1),
            ((f64::from(h) * s).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        );
        imaging::encode_png(&small)?
    } else {
        imaging::encode_png(img)?
    };
    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

fn prepare(app: &AppHandle, messages: &[ChatMessage]) -> AppResult<Vec<Prepared>> {
    messages
        .iter()
        .filter(|m| matches!(m.role.as_str(), "user" | "assistant"))
        .map(|m| {
            let images = m
                .images
                .iter()
                .map(|i| load_image(app, i).and_then(|img| encode_image(&img)))
                .collect::<AppResult<Vec<_>>>()?;
            Ok(Prepared {
                role: m.role.clone(),
                text: m.content.clone(),
                images,
            })
        })
        .collect()
}

/// 开始一次对话请求。回答通过 `channel` 推回去，函数本身立即返回。
pub async fn chat(app: AppHandle, req: ChatRequest, channel: Channel<ChatEvent>) -> AppResult<()> {
    if state(&app).settings.read().general.offline_mode {
        return Err(AppError::msg("已开启离线模式，AI 对话不可用"));
    }
    let (provider, model) = resolve(&app, req.model.as_deref())?;
    let ai = state(&app).settings.read().ai.clone();
    let messages = req.messages.clone();
    let prep_app = app.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || prepare(&prep_app, &messages))
        .await
        .map_err(|e| AppError::msg(e.to_string()))??;
    let key = key_of(&app, &provider.id);
    let client = net::client(&app, Purpose::Ai);
    let params = Params {
        model: model.clone(),
        system: ai.system_prompt.clone(),
        temperature: ai.temperature,
        max_tokens: ai.max_tokens,
    };
    let id = req.id.clone();
    let _ = channel.send(ChatEvent::Start {
        model: format!("{} · {model}", provider.name),
    });
    let handle = tauri::async_runtime::spawn(async move {
        let delta = |text: &str| {
            let _ = channel.send(ChatEvent::Delta {
                text: text.to_string(),
            });
        };
        let result = match provider.kind.as_str() {
            "anthropic" => {
                anthropic::stream(
                    &client,
                    &provider.base_url,
                    key.as_deref(),
                    &params,
                    &prepared,
                    delta,
                )
                .await
            }
            _ => {
                openai::stream(
                    &client,
                    &provider.base_url,
                    key.as_deref(),
                    &params,
                    &prepared,
                    delta,
                )
                .await
            }
        };
        let _ = channel.send(match result {
            Ok(()) => ChatEvent::Done,
            Err(err) => {
                // 日志只记模型和错误，不记对话内容
                tracing::warn!(model = %params.model, "AI 对话失败：{err}");
                ChatEvent::Error {
                    message: err.to_string(),
                }
            }
        });
        if let Some(map) = RUNNING.lock().as_mut() {
            map.remove(&id);
        }
    });
    RUNNING
        .lock()
        .get_or_insert_with(HashMap::new)
        .insert(req.id, handle);
    Ok(())
}

/// 停止一次进行中的回答。
pub fn cancel(id: &str) {
    if let Some(handle) = RUNNING.lock().as_mut().and_then(|m| m.remove(id)) {
        handle.abort();
    }
}

// ───────────────────────── 独立窗口 ─────────────────────────

/// 打开对话时带进去的东西：一段文字（选中的字 / 识字结果）、几张图（截图）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiContext {
    pub text: Option<String>,
    pub images: Vec<ChatImage>,
    /// selection | ocr | capture | free
    pub source: String,
    /// 已有的对话（从划词面板挪到独立窗口时带过去接着聊）。前端的格式，这里原样转交
    pub history: Vec<serde_json::Value>,
    /// 每次打开加一，前端据此开新对话
    pub seq: u64,
}

static CONTEXT: Mutex<Option<AiContext>> = Mutex::new(None);
static SEQ: AtomicU64 = AtomicU64::new(0);

pub fn current_context() -> Option<AiContext> {
    CONTEXT.lock().clone()
}

/// 截图带进 AI：图放进内存图库，只留最近几张。
pub fn store_image(app: &AppHandle, image: Arc<RgbaImage>) -> ChatImage {
    static KEPT: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let id = format!("ai-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let st = state(app);
    st.images.put(id.clone(), image);
    let mut kept = KEPT.lock();
    kept.push(id.clone());
    while kept.len() > 6 {
        let old = kept.remove(0);
        st.images.remove(&old);
    }
    ChatImage::Store { id }
}

/// 打开（或换上新上下文）独立的 AI 对话窗口。
pub fn open_window(app: &AppHandle, mut context: AiContext) {
    context.seq = SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    *CONTEXT.lock() = Some(context.clone());
    let panel = state(app).settings.read().ai.panel.clone();
    let ui = app.clone();
    let _ = app.run_on_main_thread(move || {
        let window = match ui.get_webview_window(WINDOW) {
            Some(w) => w,
            None => match wm::builder(&ui, WINDOW)
                .title("AI 对话 - DATO COR")
                .resizable(true)
                .transparent(true)
                .shadow(true)
                .min_inner_size(380.0, 420.0)
                .inner_size(f64::from(panel.width), f64::from(panel.height))
                .build()
            {
                Ok(w) => {
                    wm::apply_window_effects(&ui, &w);
                    if let Some(monitor) = wm::monitor_under_cursor() {
                        let _ = wm::place_on_monitor(
                            &w,
                            &monitor,
                            f64::from(panel.width),
                            f64::from(panel.height),
                            wm::Anchor::Center,
                        );
                    }
                    w
                }
                Err(err) => {
                    tracing::error!("创建 AI 窗口失败：{err}");
                    return;
                }
            },
        };
        let _ = ui.emit_to(WINDOW, events::AI_CONTEXT, &context);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    });
}
