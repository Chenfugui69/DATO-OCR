//! MCP 工具：AI 能对 DATO OCR 做的事。结果都是给模型看的文字（JSON），图片给缩小过的图。
//! 验证码（type = otp）一律不出现在结果里，也不能按 ID 读到。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};
use crate::platform::ClipboardPayload;
use crate::state::state;
use crate::storage::clipboard::{self as repo, ClipItem, ClipQuery};
use crate::storage::ocr;
use crate::{events, imaging};

pub const INSTRUCTIONS: &str = "DATO OCR keeps the user's clipboard history (text, links, images, files copied on this \
computer and synced from their other devices), pinboards (named groups of saved clips) and the text recognized from \
screenshots (OCR). Use search_clipboard to find things the user copied, get_clipboard_item to read one in full, and the \
pinboard tools to organize clips. Clip ids are stable numbers. Verification codes are never exposed.";

/// 交给模型的图片最长边（再大就是浪费上下文）
const MAX_IMAGE_SIDE: u32 = 1568;
/// 一条文字最多给这么多字
const MAX_TEXT_CHARS: usize = 100_000;

fn types_enum() -> Value {
    json!(["text", "link", "image", "files", "color"])
}

pub fn definitions() -> Value {
    json!([
        {
            "name": "search_clipboard",
            "title": "Search clipboard history",
            "description": "Search the user's clipboard history (newest first). Without a query it lists the most recent clips. \
    Returns id, type, a short preview, the app or device it came from, its pinboard and when it was copied.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Words to search for (Chinese and English both work). Omit to list recent clips." },
                    "type": { "type": "string", "enum": types_enum(), "description": "Only this kind of clip" },
                    "pinboard": { "type": "string", "description": "Only clips in the pinboard with this name" },
                    "copied_after": { "type": "string", "description": "Only clips copied at or after this time, e.g. 2026-10-10 or 2026-10-10T09:00" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 20 }
                }
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_clipboard_item",
            "title": "Read a clip",
            "description": "Read one clip in full by id: the whole text, the image itself (downscaled), or the file paths.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "integer" } },
                "required": ["id"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "add_to_clipboard",
            "title": "Save text to the clipboard history",
            "description": "Save a piece of text into the clipboard history (e.g. a draft you wrote for the user), optionally \
    putting it on the system clipboard so the user can paste it right away, and optionally into a pinboard.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "copy": { "type": "boolean", "default": false, "description": "Also put it on the system clipboard" },
                    "pinboard": { "type": "string", "description": "Also add it to this pinboard (created if missing)" },
                    "title": { "type": "string", "description": "Name shown on the card in the pinboard" }
                },
                "required": ["text"]
            }
        },
        {
            "name": "list_pinboards",
            "title": "List pinboards",
            "description": "List the user's pinboards (named groups of saved clips) with how many clips each has.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "create_pinboard",
            "title": "Create a pinboard",
            "description": "Create a new pinboard.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "color": { "type": "string", "enum": ["red", "orange", "yellow", "green", "blue", "purple", "pink", "gray"] }
                },
                "required": ["name"]
            }
        },
        {
            "name": "add_to_pinboard",
            "title": "Add clips to a pinboard",
            "description": "Put clips (by id) into a pinboard. The pinboard is created if it doesn't exist. Clips in a pinboard \
    are kept when old history is cleaned up.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pinboard": { "type": "string" },
                    "ids": { "type": "array", "items": { "type": "integer" }, "minItems": 1 }
                },
                "required": ["pinboard", "ids"]
            }
        },
        {
            "name": "remove_from_pinboard",
            "title": "Remove clips from their pinboard",
            "description": "Take clips (by id) out of whatever pinboard they are in. The clips stay in the history.",
            "inputSchema": {
                "type": "object",
                "properties": { "ids": { "type": "array", "items": { "type": "integer" }, "minItems": 1 } },
                "required": ["ids"]
            }
        },
        {
            "name": "search_ocr_history",
            "title": "Search recognized text",
            "description": "Search the text DATO OCR recognized from the user's screenshots (newest first).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 20, "default": 5 }
                }
            },
            "annotations": { "readOnlyHint": true }
        }
    ])
}

fn text(v: Value) -> Vec<Value> {
    vec![json!({ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() })]
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn ids_arg(args: &Value) -> AppResult<Vec<i64>> {
    let ids: Vec<i64> = args
        .get("ids")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    if ids.is_empty() {
        return Err(AppError::msg("ids is required"));
    }
    Ok(ids)
}

fn time_str(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

/// `2026-10-10`、`2026-10-10T09:00`、`2026-10-10 09:00:00` → 本地时间的毫秒
fn parse_time(s: &str) -> Option<i64> {
    use chrono::{NaiveDate, NaiveDateTime, TimeZone};
    let s = s.trim().replace('T', " ");
    let dt = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M"))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
        })?;
    chrono::Local
        .from_local_datetime(&dt)
        .earliest()
        .map(|t| t.timestamp_millis())
}

fn summary(item: &ClipItem, group_names: &[(i64, String)]) -> Value {
    let preview: String = item
        .preview
        .clone()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    json!({
        "id": item.id,
        "type": item.kind,
        "preview": if item.kind == "files" { item.files.join("\n") } else { preview },
        "title": item.note,
        "source": item.source_app,
        "device": item.device_kind,
        "from_other_device": item.remote,
        "pinboard": item.group_id.and_then(|g| group_names.iter().find(|(id, _)| *id == g).map(|(_, n)| n.clone())),
        "size": match (item.width, item.height) { (Some(w), Some(h)) => Some(format!("{w}x{h}")), _ => None },
        "copied_at": time_str(item.last_used_at),
    })
}

fn group_names(app: &AppHandle) -> AppResult<Vec<(i64, String)>> {
    Ok(state(app)
        .db
        .with(|c| repo::list_groups(c))?
        .into_iter()
        .map(|g| (g.id, g.name))
        .collect())
}

/// 按名字找分组（不分大小写），`create` 时没有就建一个
fn find_group(app: &AppHandle, name: &str, create: bool) -> AppResult<i64> {
    let db = &state(app).db;
    let groups = db.with(|c| repo::list_groups(c))?;
    if let Some(g) = groups.iter().find(|g| g.name.eq_ignore_ascii_case(name)) {
        return Ok(g.id);
    }
    if !create {
        return Err(AppError::msg(format!("no pinboard named \"{name}\"")));
    }
    db.with(|c| repo::create_group(c, name, None))
}

fn changed(app: &AppHandle) {
    let _ = app.emit(
        events::CLIPBOARD_CHANGED,
        crate::clipboard::ClipChanged {
            id: 0,
            is_new: false,
        },
    );
}

fn require_write(app: &AppHandle) -> AppResult<()> {
    if state(app).settings.read().mcp.allow_write {
        Ok(())
    } else {
        Err(AppError::msg(
            "The user only allows reading. Ask them to turn on \"Allow changes\" in DATO OCR settings → AI tools.",
        ))
    }
}

pub fn call(app: &AppHandle, name: &str, args: &Value) -> AppResult<Vec<Value>> {
    let st = state(app);
    match name {
        "search_clipboard" => {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .clamp(1, 50) as u32;
            let group_id = match str_arg(args, "pinboard") {
                Some(n) => Some(find_group(app, n, false)?),
                None => None,
            };
            let after = str_arg(args, "copied_after").and_then(parse_time);
            let kinds = match str_arg(args, "type") {
                Some(t) => vec![t.to_string()],
                None => ["text", "link", "image", "files", "color"]
                    .map(String::from)
                    .to_vec(),
            };
            let q = ClipQuery {
                keyword: str_arg(args, "query").map(String::from),
                kinds,
                group_id,
                // 按时间筛选时多取一些再滤
                limit: if after.is_some() { 200 } else { limit },
                ..Default::default()
            };
            let page = st.db.with(|c| repo::query(c, &q))?;
            let names = group_names(app)?;
            let items: Vec<Value> = page
                .items
                .iter()
                .filter(|i| i.kind != "otp")
                .filter(|i| after.is_none_or(|t| i.last_used_at >= t))
                .take(limit as usize)
                .map(|i| summary(i, &names))
                .collect();
            Ok(text(json!({ "count": items.len(), "items": items })))
        }
        "get_clipboard_item" => {
            let id = args
                .get("id")
                .and_then(Value::as_i64)
                .ok_or_else(|| AppError::msg("id is required"))?;
            let detail = st.db.with(|c| repo::get_detail(c, id))?;
            if detail.item.kind == "otp" {
                return Err(AppError::msg("this clip is not available"));
            }
            let names = group_names(app)?;
            let mut meta = summary(&detail.item, &names);
            match detail.item.kind.as_str() {
                "image" => {
                    let rel = detail
                        .item
                        .file_path
                        .clone()
                        .ok_or_else(|| AppError::msg("image file is missing"))?;
                    let img = imaging::load(&st.paths.abs(&rel))?;
                    let (w, h) = img.dimensions();
                    let scale = (MAX_IMAGE_SIDE as f64 / w.max(h) as f64).min(1.0);
                    let small = if scale < 1.0 {
                        image::imageops::thumbnail(
                            &img,
                            ((w as f64 * scale) as u32).max(1),
                            ((h as f64 * scale) as u32).max(1),
                        )
                    } else {
                        img
                    };
                    let jpeg = imaging::encode_jpeg(&small, 85)?;
                    let mut out = text(meta);
                    out.push(json!({ "type": "image", "data": B64.encode(jpeg), "mimeType": "image/jpeg" }));
                    Ok(out)
                }
                "files" => {
                    meta["files"] = json!(detail.item.files);
                    Ok(text(meta))
                }
                _ => {
                    let full = detail.content_text.unwrap_or_default();
                    let truncated = full.chars().count() > MAX_TEXT_CHARS;
                    meta["text"] = json!(full.chars().take(MAX_TEXT_CHARS).collect::<String>());
                    meta["truncated"] = json!(truncated || detail.item.truncated);
                    meta.as_object_mut().map(|m| m.remove("preview"));
                    Ok(text(meta))
                }
            }
        }
        "add_to_clipboard" => {
            require_write(app)?;
            let body = args
                .get("text")
                .and_then(Value::as_str)
                .filter(|t| !t.trim().is_empty())
                .ok_or_else(|| AppError::msg("text is required"))?;
            let id = crate::clipboard::store_external_text(app, body, "AI")?;
            if let Some(board) = str_arg(args, "pinboard") {
                let g = find_group(app, board, true)?;
                st.db.with(|c| repo::set_group(c, &[id], Some(g)))?;
            }
            if let Some(title) = str_arg(args, "title") {
                st.db.with(|c| repo::set_note(c, id, Some(title)))?;
            }
            let copied = args.get("copy").and_then(Value::as_bool).unwrap_or(false);
            if copied {
                let payload = ClipboardPayload::Text {
                    text: body.to_string(),
                    html: None,
                    rtf: None,
                };
                crate::clipboard::write_own(app, &payload, Some(id))?;
            }
            crate::sync::on_local(app, id);
            changed(app);
            Ok(text(json!({ "id": id, "on_system_clipboard": copied })))
        }
        "list_pinboards" => {
            let groups = st.db.with(|c| repo::list_groups(c))?;
            let list: Vec<Value> = groups
                .iter()
                .map(|g| json!({ "name": g.name, "clips": g.count, "color": g.color }))
                .collect();
            Ok(text(json!({ "pinboards": list })))
        }
        "create_pinboard" => {
            require_write(app)?;
            let name = str_arg(args, "name").ok_or_else(|| AppError::msg("name is required"))?;
            let color = str_arg(args, "color").map(|c| match c {
                "red" => "#ff3b30",
                "orange" => "#ff9500",
                "yellow" => "#f7c600",
                "green" => "#34c759",
                "blue" => "#007aff",
                "purple" => "#af52de",
                "pink" => "#ff2d55",
                _ => "#8e8e93",
            });
            let id = st.db.with(|c| repo::create_group(c, name, color))?;
            changed(app);
            Ok(text(json!({ "created": name, "id": id })))
        }
        "add_to_pinboard" => {
            require_write(app)?;
            let name =
                str_arg(args, "pinboard").ok_or_else(|| AppError::msg("pinboard is required"))?;
            let ids = ids_arg(args)?;
            let g = find_group(app, name, true)?;
            st.db.with(|c| repo::set_group(c, &ids, Some(g)))?;
            changed(app);
            Ok(text(json!({ "pinboard": name, "added": ids })))
        }
        "remove_from_pinboard" => {
            require_write(app)?;
            let ids = ids_arg(args)?;
            st.db.with(|c| repo::set_group(c, &ids, None))?;
            changed(app);
            Ok(text(json!({ "removed": ids })))
        }
        "search_ocr_history" => {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .clamp(1, 20) as u32;
            let page = st.db.with(|c| {
                ocr::query(
                    c,
                    &ocr::OcrQuery {
                        keyword: str_arg(args, "query").map(String::from),
                        offset: 0,
                        limit,
                    },
                )
            })?;
            let mut out = Vec::new();
            for s in page.items {
                let rec = st.db.with(|c| ocr::get(c, s.id))?;
                out.push(json!({
                    "id": s.id,
                    "recognized_at": time_str(s.created_at),
                    "text": rec.plain_text.chars().take(4000).collect::<String>(),
                }));
            }
            Ok(text(json!({ "count": out.len(), "results": out })))
        }
        _ => Err(AppError::msg(format!("unknown tool {name}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times() {
        assert!(parse_time("2026-10-10").is_some());
        assert!(parse_time("2026-10-10T09:30").is_some());
        assert!(parse_time("2026-10-10 09:30:15").is_some());
        assert_eq!(parse_time("yesterday"), None);
        assert!(parse_time("2026-10-10T09:00").unwrap() > parse_time("2026-10-10").unwrap());
    }

    #[test]
    fn every_tool_has_a_schema() {
        let defs = definitions();
        let defs = defs.as_array().unwrap();
        assert_eq!(defs.len(), 8);
        for d in defs {
            assert!(d["name"].is_string());
            assert_eq!(d["inputSchema"]["type"], "object");
        }
    }
}
