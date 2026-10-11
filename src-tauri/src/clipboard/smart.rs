//! 智能推荐：点剪贴板面板上的 ✨，根据用户正在做的事挑出接下来最可能要粘贴的几条（参照 Paste 的"智能剪贴板"）。
//!
//! "正在做的事"= 打开面板之前在用的那个窗口：应用名、窗口标题，再把这个窗口截下来用系统识字认出上面的字
//! （在本机做；面板盖住的那块涂掉，不算进去）。用过一次智能推荐之后，每次打开面板就在后台先读好，
//! 点 ✨ 时直接拿来用，几乎不用等。然后：
//! - 默认在本机按词重合打分（中文按两个字一组、英文按单词），再看上下文里有没有"链接""图片"这类提示；
//! - 设置里打开"用 AI 模型推荐"后，把这些上下文和最近的剪贴板摘要交给设置好的 AI 模型挑，失败了退回本机打分。
//!
//! 验证码不参与推荐。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use serde::Serialize;
use tauri::AppHandle;

use crate::error::{AppError, AppResult};
use crate::imaging;
use crate::platform::{self, PhysicalRect, WindowHandle};
use crate::state::state;
use crate::storage::clipboard::{self as repo, ClipItem, ClipQuery};

use super::smart_words::wanted_kind;

/// 从最近这么多条里挑
const CANDIDATES: u32 = 80;
/// 最多推荐几条
const PICKS: usize = 5;
/// 本机打分用的窗口文字上限
const MAX_CONTEXT_CHARS: usize = 3000;
/// 截下来的窗口长边超过这么多像素就先缩小再识字（Retina 屏上字是两倍大，缩一半照样认得出，识字快好几倍）
const OCR_MAX_SIDE: u32 = 1800;
/// 交给 AI 模型的：条数、窗口文字（越短越快）
const AI_CANDIDATES: usize = 40;
const AI_CONTEXT_CHARS: usize = 1500;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// 推荐的条目，最可能的在前；可能是空的
    pub items: Vec<ClipItem>,
    /// 模型给的一句理由（本机打分没有）
    pub reason: Option<String>,
    /// 是 AI 模型挑的（false = 本机按词打分）
    pub by_ai: bool,
    /// 看的是哪个应用
    pub app: Option<String>,
}

#[derive(Clone)]
struct Context {
    app: Option<String>,
    title: String,
    text: String,
}

/// 打开面板之前的那个窗口：应用名、标题、识字认出的文字。截不到 / 认不出就只有标题。
fn context(target: Option<WindowHandle>) -> Context {
    let Some(target) = target else {
        return Context {
            app: None,
            title: String::new(),
            text: String::new(),
        };
    };
    let info = platform::enumerate_windows()
        .ok()
        .and_then(|ws| ws.into_iter().find(|w| w.handle == target));
    let app = platform::app_info_of(target)
        .map(|a| a.name)
        .or_else(|| info.as_ref().map(|w| w.app_name.clone()))
        .filter(|n| !n.is_empty());
    let title = info.as_ref().map(|w| w.title.clone()).unwrap_or_default();
    let text = info
        .and_then(|w| {
            window_text(w.bounds)
                .map_err(|e| tracing::debug!("智能推荐：读窗口文字失败：{e}"))
                .ok()
        })
        .unwrap_or_default();
    Context { app, title, text }
}

/// 只截窗口所在的那一块屏（不是全部屏），面板盖住的地方涂黑，太大的先缩小，再识字
fn window_text(bounds: PhysicalRect) -> AppResult<String> {
    if !platform::system_ocr_available() {
        return Err(AppError::msg("没有系统识字"));
    }
    let cx = bounds.x + bounds.width as i32 / 2;
    let cy = bounds.y + bounds.height as i32 / 2;
    let monitors = platform::list_monitors()?;
    let monitor = monitors
        .iter()
        .find(|m| m.bounds.contains_point(cx, cy))
        .or_else(|| monitors.first())
        .ok_or_else(|| AppError::msg("没有抓到屏幕"))?;
    let img = platform::capture_monitor(monitor.id)?;
    let visible = bounds
        .intersect(&monitor.bounds)
        .ok_or_else(|| AppError::msg("窗口不在屏幕上"))?;
    let rel = PhysicalRect::new(
        visible.x - monitor.bounds.x,
        visible.y - monitor.bounds.y,
        visible.width,
        visible.height,
    );
    let mut crop = imaging::crop(&img, rel)?;
    drop(img);
    // 面板开着时允许被截屏：它盖住的那块是剪贴板内容，认进来就"什么都像"了
    if let Some(panel) = super::panel::shown_rect().and_then(|p| p.intersect(&visible)) {
        let (x0, y0) = ((panel.x - visible.x) as u32, (panel.y - visible.y) as u32);
        for y in y0..(y0 + panel.height).min(crop.height()) {
            for x in x0..(x0 + panel.width).min(crop.width()) {
                crop.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
    }
    let long = crop.width().max(crop.height());
    if long > OCR_MAX_SIDE {
        let k = f64::from(OCR_MAX_SIDE) / f64::from(long);
        crop = image::imageops::resize(
            &crop,
            ((f64::from(crop.width()) * k) as u32).max(1),
            ((f64::from(crop.height()) * k) as u32).max(1),
            image::imageops::FilterType::Triangle,
        );
    }
    let mut lines = platform::system_ocr(&crop)?;
    lines.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let text: Vec<String> = lines.into_iter().map(|l| l.text).collect();
    Ok(clip_middle(&text.join("\n"), MAX_CONTEXT_CHARS))
}

/// 太长的话留头（标题、收件人之类）和尾（光标多半在后面），去掉中间
fn clip_middle(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let head = max / 3;
    let tail = max - head;
    let mut out: String = chars[..head].iter().collect();
    out.push_str("\n…\n");
    out.extend(chars[chars.len() - tail..].iter());
    out
}

fn candidates(app: &AppHandle) -> AppResult<Vec<ClipItem>> {
    let q = ClipQuery {
        kinds: ["text", "link", "image", "files", "color"]
            .map(String::from)
            .to_vec(),
        limit: CANDIDATES,
        ..Default::default()
    };
    Ok(state(app).db.with(|c| repo::query(c, &q))?.items)
}

/// 读好的"你在做什么"：什么时候读的、是哪个窗口、内容。打开面板时后台读一份，点 ✨ 和接着的 AI 细化都用它
static LAST: Mutex<Option<(Instant, Option<WindowHandle>, Context)>> = Mutex::new(None);
/// 同一个窗口、这么久以内读的算数
const CONTEXT_TTL: Duration = Duration::from_secs(60);
/// 同一时间只读一份：点 ✨ 时后台那份还没读完，就等它读完直接用，不重复截屏识字
static READING: Mutex<()> = Mutex::new(());
/// 用过智能推荐没有（0 = 还没查，1 = 没用过，2 = 用过）。没用过的人打开面板时不预读，不白白截屏识字
static USED: AtomicU8 = AtomicU8::new(0);
const USED_KEY: &str = "smart.used";

fn used(app: &AppHandle) -> bool {
    match USED.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let yes = matches!(state(app).db.kv_get(USED_KEY), Ok(Some(_)));
            USED.store(if yes { 2 } else { 1 }, Ordering::Relaxed);
            yes
        }
    }
}

fn mark_used(app: &AppHandle) {
    if USED.swap(2, Ordering::Relaxed) != 2 {
        let _ = state(app).db.kv_set(USED_KEY, "1");
    }
}

/// 打开面板时调：在后台把"你在做什么"读好（截屏 + 识字，零点几秒），点 ✨ 时直接用
pub fn prefetch(app: &AppHandle) {
    if !used(app) {
        return;
    }
    std::thread::spawn(|| {
        read_context(true);
    });
}

/// `fresh`：不管读没读过都重读（打开面板时：窗口里的内容可能变了）；否则同一个窗口、没过期就用读好的
fn read_context(fresh: bool) -> Context {
    let _one = READING.lock();
    let target = super::panel::previous_window();
    if !fresh {
        if let Some((_, _, ctx)) = LAST
            .lock()
            .as_ref()
            .filter(|(at, w, _)| *w == target && at.elapsed() < CONTEXT_TTL)
        {
            return ctx.clone();
        }
    }
    let ctx = context(target);
    *LAST.lock() = Some((Instant::now(), target, ctx.clone()));
    ctx
}

/// `ai = false`：本机打分，很快（打开面板时已经读好了窗口，几十毫秒），先把结果给用户；
/// `ai = true`：拿同一份上下文交给 AI 模型挑（几秒），结果出来再替换。
pub async fn suggest(app: AppHandle, ai: bool) -> AppResult<Suggestion> {
    mark_used(&app);
    let worker = app.clone();
    let (ctx, items) = tauri::async_runtime::spawn_blocking(move || {
        let ctx = read_context(false);
        candidates(&worker).map(|items| (ctx, items))
    })
    .await
    .map_err(|e| AppError::msg(e.to_string()))??;
    if items.is_empty() {
        return Ok(Suggestion {
            items: Vec::new(),
            reason: None,
            by_ai: false,
            app: ctx.app,
        });
    }
    if ai {
        if state(&app).settings.read().general.offline_mode {
            return Err(AppError::msg("已开启离线模式，AI 对话不可用"));
        }
        let (ids, reason) = ask_model(&app, &ctx, &items).await?;
        return Ok(Suggestion {
            items: pick(&items, &ids),
            reason,
            by_ai: true,
            app: ctx.app,
        });
    }
    let ids = local_rank(&ctx, &items);
    Ok(Suggestion {
        items: pick(&items, &ids),
        reason: None,
        by_ai: false,
        app: ctx.app,
    })
}

fn pick(items: &[ClipItem], ids: &[i64]) -> Vec<ClipItem> {
    ids.iter()
        .filter_map(|id| items.iter().find(|i| i.id == *id).cloned())
        .take(PICKS)
        .collect()
}

// ───────────────────────── AI 模型 ─────────────────────────

fn item_line(i: &ClipItem) -> String {
    let body = match i.kind.as_str() {
        "image" => format!(
            "image {}x{}",
            i.width.unwrap_or_default(),
            i.height.unwrap_or_default()
        ),
        "files" => i.files.join(", "),
        _ => i
            .preview
            .clone()
            .unwrap_or_default()
            .replace('\n', " ")
            .chars()
            .take(160)
            .collect(),
    };
    let title = i
        .note
        .as_deref()
        .map(|n| format!(" \"{n}\""))
        .unwrap_or_default();
    let from = i.source_app.as_deref().unwrap_or("?");
    format!("#{} [{}]{title} from {from}: {body}", i.id, i.kind)
}

async fn ask_model(
    app: &AppHandle,
    ctx: &Context,
    items: &[ClipItem],
) -> AppResult<(Vec<i64>, Option<String>)> {
    let lang = if crate::i18n::english() {
        "English"
    } else {
        "Simplified Chinese"
    };
    let system = format!(
        "You pick which item from the user's clipboard history they most likely want to paste next, based on what \
they are doing right now. Reply with JSON only, no prose: {{\"ids\": [up to {PICKS} ids, best first], \"reason\": \
\"at most 15 words in {lang}\"}}. Only use ids from the list. If nothing clearly fits, reply {{\"ids\": []}}."
    );
    // 给模型的越少越快：最近 40 条、窗口文字 1500 字
    let list: Vec<String> = items.iter().take(AI_CANDIDATES).map(item_line).collect();
    let user = format!(
        "App: {}\nWindow title: {}\nText visible in the window (OCR, may be noisy):\n<<<\n{}\n>>>\n\nClipboard history (newest first):\n{}",
        ctx.app.as_deref().unwrap_or("unknown"),
        ctx.title,
        clip_middle(&ctx.text, AI_CONTEXT_CHARS),
        list.join("\n")
    );
    // 设置里可以单独给推荐挑个快的小模型；没挑就用 AI 对话的默认模型
    let model = state(app).settings.read().clipboard.smart_model.clone();
    let reply = crate::ai::complete(app, Some(&model), &system, &user, 120).await?;
    let start = reply
        .find('{')
        .ok_or_else(|| AppError::msg("模型没有回 JSON"))?;
    let end = reply
        .rfind('}')
        .ok_or_else(|| AppError::msg("模型没有回 JSON"))?;
    let v: serde_json::Value = serde_json::from_str(&reply[start..=end])?;
    let ids = v["ids"]
        .as_array()
        .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
        .unwrap_or_default();
    let reason = v["reason"]
        .as_str()
        .map(str::to_string)
        .filter(|r| !r.is_empty());
    Ok((ids, reason))
}

// ───────────────────────── 本机打分 ─────────────────────────

const STOP: &[&str] = &[
    "the", "and", "for", "you", "your", "with", "that", "this", "are", "was", "from", "have",
    "has", "not", "but", "all", "can", "will", "our", "out", "http", "https", "www", "com",
];

/// 中文两个字一组、英文和数字按词（两个字符以上），都转小写
fn tokens(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let lower = text.to_lowercase();
    let mut word = String::new();
    let mut prev_cjk: Option<char> = None;
    for c in lower.chars() {
        let cjk = ('\u{4e00}'..='\u{9fff}').contains(&c);
        if c.is_ascii_alphanumeric() {
            word.push(c);
        } else if !word.is_empty() {
            if word.len() >= 2 && !STOP.contains(&word.as_str()) {
                out.insert(std::mem::take(&mut word));
            }
            word.clear();
        }
        if cjk {
            if let Some(p) = prev_cjk {
                out.insert(format!("{p}{c}"));
            }
            prev_cjk = Some(c);
        } else {
            prev_cjk = None;
        }
    }
    if word.len() >= 2 && !STOP.contains(&word.as_str()) {
        out.insert(word);
    }
    out
}

fn item_text(i: &ClipItem) -> String {
    let mut s = i.preview.clone().unwrap_or_default();
    for extra in [i.note.as_deref(), i.source_app.as_deref()]
        .into_iter()
        .flatten()
    {
        s.push(' ');
        s.push_str(extra);
    }
    for f in &i.files {
        s.push(' ');
        s.push_str(f.rsplit(['/', '\\']).next().unwrap_or(f));
    }
    s
}

fn local_rank(ctx: &Context, items: &[ClipItem]) -> Vec<i64> {
    let context = format!("{}\n{}", ctx.title, ctx.text);
    let ctx_tokens = tokens(&context);
    let item_tokens: Vec<HashSet<String>> = items.iter().map(|i| tokens(&item_text(i))).collect();
    // 很多条都有的词（"的""http"之类漏网的）不算数
    let mut df: HashMap<&str, usize> = HashMap::new();
    for t in &item_tokens {
        for w in t {
            *df.entry(w.as_str()).or_default() += 1;
        }
    }
    let wanted = wanted_kind(&context.to_lowercase());
    let n = items.len().max(1) as f64;
    let mut scored: Vec<(f64, i64)> = items
        .iter()
        .zip(&item_tokens)
        .enumerate()
        .filter_map(|(rank, (item, toks))| {
            let overlap: f64 = toks
                .intersection(&ctx_tokens)
                .map(|w| {
                    let rare = 1.0 / (1.0 + *df.get(w.as_str()).unwrap_or(&0) as f64);
                    // 长词更说明问题
                    rare * (w.chars().count().min(8) as f64 / 2.0)
                })
                .sum();
            let kind_bonus = if wanted == Some(item.kind.as_str()) {
                0.6
            } else {
                0.0
            };
            if overlap <= 0.0 && kind_bonus <= 0.0 {
                return None;
            }
            // 越新越可能（最多加 0.3）
            let recency = 0.3 * (1.0 - rank as f64 / n);
            Some((overlap + kind_bonus + recency, item.id))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(PICKS).map(|(_, id)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, kind: &str, preview: &str) -> ClipItem {
        ClipItem {
            id,
            kind: kind.into(),
            preview: Some(preview.into()),
            has_html: false,
            file_path: None,
            thumb_path: None,
            files: Vec::new(),
            char_count: None,
            size_bytes: None,
            width: None,
            height: None,
            source_app: None,
            source_icon: None,
            truncated: false,
            remote: false,
            pinned: false,
            favorite: false,
            note: None,
            group_id: None,
            created_at: 0,
            last_used_at: 0,
            origin_text: None,
            device_kind: None,
        }
    }

    #[test]
    fn tokenizes_chinese_and_english() {
        let t = tokens("周五发布 DATO-OCR release notes");
        assert!(t.contains("周五") && t.contains("五发") && t.contains("发布"));
        assert!(t.contains("dato") && t.contains("ocr") && t.contains("release"));
        assert!(!t.contains("the"));
    }

    #[test]
    fn ranks_overlap_and_requested_kind() {
        let items = vec![
            item(1, "text", "今天中午吃什么"),
            item(2, "link", "https://airbnb.com/rooms/298838"),
            item(3, "text", "Cabin near Muir Woods, three nights"),
        ];
        let ctx = Context {
            app: Some("Mail".into()),
            title: "San Francisco weekend".into(),
            text: "We're doing the coast weekend, one cabin. Here's the cabin:".into(),
        };
        let ranked = local_rank(&ctx, &items);
        assert_eq!(ranked.first(), Some(&3));
        assert!(ranked.contains(&2), "{ranked:?}");
        assert!(!ranked.contains(&1));
    }

    #[test]
    fn keeps_head_and_tail_of_long_text() {
        let long = format!("HEAD{}TAIL", "x".repeat(5000));
        let c = clip_middle(&long, 300);
        assert!(c.starts_with("HEAD") && c.ends_with("TAIL"));
        assert!(c.chars().count() < 320);
    }
}
