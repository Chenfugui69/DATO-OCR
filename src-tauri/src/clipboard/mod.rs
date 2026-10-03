//! 剪贴板历史（规格 05）。
//!
//! 采集链路：平台监听线程（同步取来源应用 + 读全部格式）→ channel → 本模块的入库线程
//! （分类、去重、存图、缩略图、来源图标）→ 数据库 → `clipboard-changed` 事件。
//!
//! 默认**全量记录**（铁律 6）：隐私标记、黑名单两道开关都默认关闭。

pub mod panel;

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

use crate::error::AppResult;
use crate::paths::thumb_rel;
use crate::platform::{self, ClipboardBackup, ClipboardPayload, ClipboardSnapshot};
use crate::settings::ClipboardSettings;
use crate::state::state;
use crate::storage::{clipboard as repo, clipboard::NewClip};
use crate::{events, imaging};

const PREVIEW_CHARS: usize = 300;
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub struct ClipboardService {
    started: AtomicBool,
    ignore: Mutex<IgnoreState>,
    inserts: AtomicU32,
}

/// 自己造成的剪贴板变化：序列号区间 `(start, end]` 内的都不进历史。自己写一次剪贴板，
/// 系统序列号可能跳不止 1（延迟渲染、程序分步写入），所以记区间而不是单个号。
///
/// 区间要能**同时存在好几段**：划词翻译是"模拟复制 → 恢复备份"两次连着做，入库线程
/// 处理第一次变化时第二段可能已经开始了，只记一段的话第一次就会漏判。
#[derive(Default)]
struct IgnoreState {
    ranges: VecDeque<IgnoreRange>,
}

struct IgnoreRange {
    start: u32,
    /// None：正在写，终点还没拿到，入库线程遇到这段之后的变化要等
    end: Option<u32>,
    /// 这次变化对应哪条历史：让它冒泡到最前，而不是新增一条
    touch: Option<i64>,
    at: Instant,
}

/// 区间保留多久。入库线程最多晚几百毫秒，留足余量。
const IGNORE_TTL: Duration = Duration::from_secs(10);

/// 划词翻译取完字、恢复了原剪贴板之后，有的输入法（实测微信输入法）还会把刚才复制的
/// 文字（换行统一成 `\n`）再写一遍剪贴板，把恢复的原内容顶掉。短时间内再出现同一段文字：
/// 不记历史，并再恢复一次原内容。
struct Echo {
    text: String,
    until: Instant,
    backup: Option<ClipboardBackup>,
}

static ECHO: Mutex<Option<Echo>> = Mutex::new(None);
const ECHO_WINDOW: Duration = Duration::from_millis(1500);

fn normalize_text(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_string()
}

/// 划词翻译恢复剪贴板后调用：接下来一小会儿里出现的同一段文字是"回声"。
pub fn expect_echo(text: &str, backup: Option<ClipboardBackup>) {
    *ECHO.lock() = Some(Echo {
        text: normalize_text(text),
        until: Instant::now() + ECHO_WINDOW,
        backup,
    });
}

/// 这次变化是不是划词翻译的回声；是的话顺手把原内容恢复回去。
fn swallow_echo(app: &AppHandle, snap: &ClipboardSnapshot) -> bool {
    let mut echo = ECHO.lock();
    let Some(e) = echo.as_ref() else { return false };
    if Instant::now() > e.until {
        *echo = None;
        return false;
    }
    let same = snap
        .text
        .as_deref()
        .is_some_and(|t| normalize_text(t) == e.text);
    if !same {
        return false;
    }
    let backup = echo.take().and_then(|e| e.backup);
    drop(echo);
    tracing::debug!("划词翻译的剪贴板回声，不记录并恢复原内容");
    if let Some(backup) = backup {
        begin_ignore(app);
        platform::clipboard_restore(backup);
        end_ignore(app);
    }
    true
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipChanged {
    pub id: i64,
    pub is_new: bool,
}

pub fn start(app: &AppHandle) {
    let st = state(app);
    if st.clipboard.started.swap(true, Ordering::SeqCst) {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel::<ClipboardSnapshot>();
    if let Err(err) = platform::start_clipboard_listener(tx) {
        tracing::error!("剪贴板监听启动失败：{err}");
        return;
    }
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("clipboard-ingest".into())
        .spawn(move || {
            for snapshot in rx {
                if let Err(err) = ingest(&app, snapshot) {
                    tracing::warn!("剪贴板入库失败：{err}");
                }
            }
        });
}

/// DATO COR 自己写剪贴板（粘贴回填、划词翻译恢复…）。这次变化不进历史；
/// `touch` 给了的话让那条已有记录冒泡到最前（规格 05 §3.4 决定）。
pub fn write_own(app: &AppHandle, payload: &ClipboardPayload, touch: Option<i64>) -> AppResult<()> {
    begin_ignore(app);
    let result = platform::clipboard_write(payload);
    finish_ignore(app, touch);
    result
}

/// 模拟 Ctrl+C / 恢复备份这类"拿不到确切写入时机"的操作：先 begin（入库线程遇到新
/// 变化会等待），操作完成后 end（把当前序列号记为这段的终点）。两者成对、不嵌套。
pub fn begin_ignore(app: &AppHandle) {
    let st = state(app);
    let mut ig = st.clipboard.ignore.lock();
    ig.ranges
        .retain(|r| r.end.is_none() || r.at.elapsed() < IGNORE_TTL);
    ig.ranges.push_back(IgnoreRange {
        start: platform::clipboard_sequence(),
        end: None,
        touch: None,
        at: Instant::now(),
    });
}

pub fn end_ignore(app: &AppHandle) {
    finish_ignore(app, None);
}

fn finish_ignore(app: &AppHandle, touch: Option<i64>) {
    let st = state(app);
    let mut ig = st.clipboard.ignore.lock();
    if let Some(range) = ig.ranges.iter_mut().rev().find(|r| r.end.is_none()) {
        range.end = Some(platform::clipboard_sequence());
        range.touch = touch;
    }
}

/// 这次变化是不是自己造成的；是的话返回要冒泡的那条历史（可能没有）。
fn should_ignore(app: &AppHandle, sequence: u32) -> Option<Option<i64>> {
    let st = state(app);
    // 最多等 ~1 秒：划词翻译的模拟复制要等目标程序响应
    for _ in 0..200 {
        {
            let ig = st.clipboard.ignore.lock();
            // 有一段还没拿到终点、而这次变化在它起点之后：可能就是它，等它写完再判断
            let waiting = ig
                .ranges
                .iter()
                .any(|r| r.end.is_none() && sequence > r.start);
            if !waiting {
                return ig
                    .ranges
                    .iter()
                    .find(|r| {
                        r.end
                            .is_some_and(|end| sequence > r.start && sequence <= end)
                    })
                    .map(|r| r.touch);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

fn sha(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn preview(text: &str) -> String {
    let collapsed: String = text.chars().take(PREVIEW_CHARS * 2).collect();
    let trimmed = collapsed.trim();
    trimmed.chars().take(PREVIEW_CHARS).collect()
}

pub fn is_url(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with("http://") || t.starts_with("https://"))
        && t.len() < 4096
        && !t.chars().any(char::is_whitespace)
        && url::Url::parse(t).is_ok()
}

pub fn is_color(text: &str) -> bool {
    let t = text.trim();
    if let Some(hex) = t.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    let lower = t.to_ascii_lowercase();
    ["rgb(", "rgba(", "hsl(", "hsla("]
        .iter()
        .any(|p| lower.starts_with(p) && lower.ends_with(')') && lower.len() < 40)
}

fn blacklisted(settings: &ClipboardSettings, snapshot: &ClipboardSnapshot) -> bool {
    if !settings.use_blacklist {
        return false;
    }
    let Some(exe) = snapshot
        .source
        .as_ref()
        .and_then(|s| s.exe_path.as_ref())
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_lowercase())
    else {
        return false;
    };
    settings.blacklist.contains(&exe)
}

fn ingest(app: &AppHandle, snap: ClipboardSnapshot) -> AppResult<()> {
    if let Some(touch) = should_ignore(app, snap.sequence) {
        if let Some(id) = touch {
            let st = state(app);
            st.db.with(|c| repo::touch(c, id))?;
            let _ = app.emit(events::CLIPBOARD_CHANGED, ClipChanged { id, is_new: false });
        }
        return Ok(());
    }
    if swallow_echo(app, &snap) {
        return Ok(());
    }
    let st = state(app);
    let settings = st.settings.read().clipboard.clone();
    if !settings.enabled || snap.is_empty() {
        return Ok(());
    }
    if settings.respect_privacy_flag && snap.privacy_flagged {
        tracing::debug!("应用标记了不记录，跳过");
        return Ok(());
    }
    if blacklisted(&settings, &snap) {
        tracing::debug!("来源应用在黑名单中，跳过");
        return Ok(());
    }

    let Some(mut item) = classify(app, &snap, &settings)? else {
        return Ok(());
    };
    if let Some(id) = st.db.with(|c| repo::find_duplicate(c, &item.hash))? {
        // 去重：不新增，只让已有那条冒到最前（不改 created_at）
        st.db.with(|c| repo::touch(c, id))?;
        let _ = app.emit(events::CLIPBOARD_CHANGED, ClipChanged { id, is_new: false });
        cleanup_unused_files(app, &item);
        return Ok(());
    }
    if let Some(source) = &snap.source {
        item.source_app = Some(source.name.clone()).filter(|n| !n.is_empty());
        if let Some(path) = &source.exe_path {
            item.source_app_path = Some(path.to_string_lossy().into_owned());
            item.source_icon = app_icon(app, path);
        }
    }
    let id = st.db.with(|c| repo::insert(c, &item))?;
    tracing::debug!(id, kind = %item.kind, chars = item.char_count, "剪贴板新记录");
    let _ = app.emit(events::CLIPBOARD_CHANGED, ClipChanged { id, is_new: true });

    if st.clipboard.inserts.fetch_add(1, Ordering::Relaxed) % 50 == 0 {
        apply_retention(app);
    }
    Ok(())
}

/// 去重命中时，classify 里已经写盘的图片要删掉。
fn cleanup_unused_files(app: &AppHandle, item: &NewClip) {
    let st = state(app);
    for f in [&item.file_path, &item.thumb_path].into_iter().flatten() {
        let _ = std::fs::remove_file(st.paths.abs(f));
    }
}

fn classify(
    app: &AppHandle,
    snap: &ClipboardSnapshot,
    settings: &ClipboardSettings,
) -> AppResult<Option<NewClip>> {
    let st = state(app);
    if !snap.files.is_empty() {
        let files: Vec<String> = snap
            .files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let joined = files.join("\n");
        let names: Vec<String> = snap
            .files
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        return Ok(Some(NewClip {
            kind: "files".into(),
            content_text: Some(joined.clone()),
            preview: Some(preview(&names.join("\n"))),
            hash: sha(joined.as_bytes()),
            char_count: Some(files.len() as i64),
            files,
            ..Default::default()
        }));
    }

    let text = snap
        .text
        .as_deref()
        .map(str::to_string)
        .filter(|t| !t.trim().is_empty());
    if let (Some(img), None) = (&snap.image, &text) {
        let mut hasher = Sha256::new();
        hasher.update(img.width().to_le_bytes());
        hasher.update(img.height().to_le_bytes());
        hasher.update(img.as_raw());
        let hash: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        // 去重先查，命中就不必编码写盘
        if st.db.with(|c| repo::find_duplicate(c, &hash))?.is_some() {
            return Ok(Some(NewClip {
                kind: "image".into(),
                hash,
                ..Default::default()
            }));
        }
        let bytes = match &snap.image_png {
            Some(png) => png.clone(),
            None => imaging::encode_png(img)?,
        };
        if bytes.len() as u64 > u64::from(settings.max_image_mb) * 1024 * 1024 {
            tracing::info!(bytes = bytes.len(), "剪贴板图片超过大小上限，不保存");
            return Ok(None);
        }
        let rel = st.paths.new_rel_file("clipboard", "png")?;
        std::fs::write(st.paths.abs(&rel), &bytes)?;
        let thumb = thumb_rel(&rel);
        let thumb = imaging::write_thumbnail(img, 320, &st.paths.abs(&thumb))
            .ok()
            .map(|_| thumb);
        return Ok(Some(NewClip {
            kind: "image".into(),
            file_path: Some(rel),
            thumb_path: thumb,
            hash,
            size_bytes: Some(bytes.len() as i64),
            width: Some(i64::from(img.width())),
            height: Some(i64::from(img.height())),
            ..Default::default()
        }));
    }

    let Some(mut text) = text else {
        return Ok(None);
    };
    let limit = settings.max_text_mb as usize * 1024 * 1024;
    let mut truncated = false;
    if text.len() > limit {
        let mut cut = limit;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        truncated = true;
    }
    let kind = if is_url(&text) {
        "link"
    } else if is_color(&text) {
        "color"
    } else {
        "text"
    };
    let html = snap
        .html
        .clone()
        .filter(|h| h.len() <= MAX_HTML_BYTES && kind == "text");
    let rtf = snap
        .rtf
        .clone()
        .filter(|r| r.len() <= MAX_HTML_BYTES && kind == "text");
    Ok(Some(NewClip {
        kind: kind.into(),
        preview: Some(preview(&text)),
        // 换行统一后再算：输入法（如微信输入法）会把刚复制的文字改成 \n 换行重写一遍，
        // 不统一的话同一段文字会出现两条
        hash: sha(text.replace("\r\n", "\n").as_bytes()),
        char_count: Some(text.chars().count() as i64),
        size_bytes: Some(text.len() as i64),
        content_text: Some(text),
        content_html: html,
        content_rtf: rtf,
        truncated,
        ..Default::default()
    }))
}

/// 来源应用图标缓存在 `app-icons/{路径哈希}.png`，每个应用只提取一次。
fn app_icon(app: &AppHandle, exe: &Path) -> Option<String> {
    let st = state(app);
    let key = sha(exe.to_string_lossy().to_lowercase().as_bytes());
    let rel = format!("app-icons/{}.png", &key[..16]);
    let abs = st.paths.abs(&rel);
    if abs.exists() {
        return Some(rel);
    }
    let icon = platform::extract_app_icon(exe)?;
    let bytes = imaging::encode_png(&icon).ok()?;
    std::fs::write(&abs, bytes).ok()?;
    Some(rel)
}

pub fn delete(app: &AppHandle, ids: &[i64]) -> AppResult<()> {
    let st = state(app);
    let files = st.db.with(|c| repo::delete(c, ids))?;
    for f in files {
        let _ = std::fs::remove_file(st.paths.abs(&f));
    }
    let _ = app.emit(
        events::CLIPBOARD_CHANGED,
        ClipChanged {
            id: 0,
            is_new: false,
        },
    );
    Ok(())
}

fn apply_retention(app: &AppHandle) {
    let st = state(app);
    let s = st.settings.read().clipboard.clone();
    if s.retention_days == 0 && s.retention_max_items == 0 {
        return;
    }
    match st
        .db
        .with(|c| repo::ids_for_retention(c, s.retention_days, s.retention_max_items))
    {
        Ok(ids) if !ids.is_empty() => {
            tracing::info!(count = ids.len(), "按保留策略清理剪贴板历史");
            let _ = delete(app, &ids);
        }
        Ok(_) => {}
        Err(err) => tracing::warn!("保留策略清理失败：{err}"),
    }
}

/// 把一条历史转成可写入剪贴板的内容。
pub fn payload_of(app: &AppHandle, id: i64, plain: bool) -> AppResult<ClipboardPayload> {
    let st = state(app);
    let detail = st.db.with(|c| repo::get_detail(c, id))?;
    Ok(match detail.item.kind.as_str() {
        "image" => {
            let rel = detail
                .item
                .file_path
                .ok_or_else(|| crate::error::AppError::msg("图片文件丢失"))?;
            let bytes = std::fs::read(st.paths.abs(&rel))?;
            let image = imaging::decode(&bytes)?;
            let png = imaging::has_alpha(&image).then_some(bytes);
            ClipboardPayload::Image { image, png }
        }
        "files" => ClipboardPayload::Files(detail.item.files.iter().map(Into::into).collect()),
        _ => ClipboardPayload::Text {
            text: detail.content_text.unwrap_or_default(),
            html: if plain { None } else { detail.content_html },
            rtf: if plain { None } else { detail.content_rtf },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_links_and_colors() {
        assert!(is_url("https://example.com/a?b=1"));
        assert!(!is_url("see https://example.com"));
        assert!(!is_url("ftp://x"));
        assert!(is_color("#FF5733"));
        assert!(is_color("#fff"));
        assert!(is_color("rgb(1, 2, 3)"));
        assert!(!is_color("#GGGGGG"));
        assert!(!is_color("FF5733"));
    }

    #[test]
    fn preview_is_bounded() {
        let long = "字".repeat(1000);
        assert_eq!(preview(&long).chars().count(), PREVIEW_CHARS);
        assert_eq!(preview("  hi  "), "hi");
    }
}
