//! 剪贴板历史表（规格 05 §2）。

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row, ToSql};
use serde::{Deserialize, Serialize};

use super::{fts_delete, fts_upsert, now_ms, tokenize};
use crate::error::{AppError, AppResult};

/// 去重回看的条数（规格 05 §1.3）
const DEDUP_WINDOW: i64 = 200;

#[derive(Clone, Debug, Default)]
pub struct NewClip {
    pub kind: String,
    pub content_text: Option<String>,
    pub content_html: Option<String>,
    pub content_rtf: Option<String>,
    pub file_path: Option<String>,
    pub thumb_path: Option<String>,
    pub files: Vec<String>,
    pub preview: Option<String>,
    pub hash: String,
    pub char_count: Option<i64>,
    pub size_bytes: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub source_app: Option<String>,
    pub source_app_path: Option<String>,
    pub source_icon: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipItem {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub preview: Option<String>,
    pub has_html: bool,
    pub file_path: Option<String>,
    pub thumb_path: Option<String>,
    pub files: Vec<String>,
    pub char_count: Option<i64>,
    pub size_bytes: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub source_app: Option<String>,
    pub source_icon: Option<String>,
    pub truncated: bool,
    pub pinned: bool,
    pub favorite: bool,
    pub note: Option<String>,
    pub group_id: Option<i64>,
    pub created_at: i64,
    pub last_used_at: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipDetail {
    #[serde(flatten)]
    pub item: ClipItem,
    pub content_text: Option<String>,
    pub content_html: Option<String>,
    pub content_rtf: Option<String>,
    pub source_app_path: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClipQuery {
    pub keyword: Option<String>,
    pub kinds: Vec<String>,
    pub pinned_only: bool,
    pub favorite_only: bool,
    pub group_id: Option<i64>,
    /// 上一页最后一条的游标，`pinned:last_used_at:id`
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipPage {
    pub items: Vec<ClipItem>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipStats {
    pub total: i64,
    pub pinned: i64,
    pub favorite: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipGroup {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
    pub count: i64,
}

const ITEM_COLUMNS: &str =
    "id, type, preview, content_html IS NOT NULL, file_path, thumb_path, file_list,
    char_count, size_bytes, width, height, source_app, source_icon, truncated, pinned, favorite,
    note, group_id, created_at, last_used_at";

fn map_item(row: &Row<'_>) -> rusqlite::Result<ClipItem> {
    let files: Option<String> = row.get(6)?;
    Ok(ClipItem {
        id: row.get(0)?,
        kind: row.get(1)?,
        preview: row.get(2)?,
        has_html: row.get(3)?,
        file_path: row.get(4)?,
        thumb_path: row.get(5)?,
        files: files
            .and_then(|f| serde_json::from_str(&f).ok())
            .unwrap_or_default(),
        char_count: row.get(7)?,
        size_bytes: row.get(8)?,
        width: row.get(9)?,
        height: row.get(10)?,
        source_app: row.get(11)?,
        source_icon: row.get(12)?,
        truncated: row.get(13)?,
        pinned: row.get(14)?,
        favorite: row.get(15)?,
        note: row.get(16)?,
        group_id: row.get(17)?,
        created_at: row.get(18)?,
        last_used_at: row.get(19)?,
    })
}

fn search_text(item: &NewClip) -> String {
    let mut text = String::new();
    if let Some(t) = &item.content_text {
        text.push_str(t);
    }
    for f in &item.files {
        text.push(' ');
        text.push_str(f.rsplit(['\\', '/']).next().unwrap_or(f));
    }
    if let Some(app) = &item.source_app {
        text.push(' ');
        text.push_str(app);
    }
    text
}

/// 最近 200 条里找相同哈希。
pub fn find_duplicate(conn: &Connection, hash: &str) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM (SELECT id, hash FROM clipboard_items ORDER BY last_used_at DESC LIMIT ?1)
             WHERE hash = ?2 LIMIT 1",
            params![DEDUP_WINDOW, hash],
            |r| r.get(0),
        )
        .optional()?)
}

/// 已有记录"冒泡"到最前：只改 last_used_at，不动 created_at。
pub fn touch(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE clipboard_items SET last_used_at = ?1 WHERE id = ?2",
        params![now_ms(), id],
    )?;
    Ok(())
}

pub fn insert(conn: &mut Connection, item: &NewClip) -> AppResult<i64> {
    let now = now_ms();
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO clipboard_items (type, content_text, content_html, content_rtf, file_path, thumb_path,
            file_list, preview, hash, char_count, size_bytes, width, height, source_app, source_app_path,
            source_icon, truncated, created_at, last_used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18)",
        params![
            item.kind,
            item.content_text,
            item.content_html,
            item.content_rtf,
            item.file_path,
            item.thumb_path,
            (!item.files.is_empty()).then(|| serde_json::to_string(&item.files).unwrap_or_default()),
            item.preview,
            item.hash,
            item.char_count,
            item.size_bytes,
            item.width,
            item.height,
            item.source_app,
            item.source_app_path,
            item.source_icon,
            item.truncated,
            now,
        ],
    )?;
    let id = tx.last_insert_rowid();
    fts_upsert(&tx, "clipboard_fts", id, &search_text(item))?;
    tx.commit()?;
    Ok(id)
}

pub fn get_item(conn: &Connection, id: i64) -> AppResult<ClipItem> {
    conn.query_row(
        &format!("SELECT {ITEM_COLUMNS} FROM clipboard_items WHERE id = ?1"),
        [id],
        map_item,
    )
    .optional()?
    .ok_or_else(|| AppError::NotFound(format!("剪贴板记录 {id}")))
}

pub fn get_detail(conn: &Connection, id: i64) -> AppResult<ClipDetail> {
    let item = get_item(conn, id)?;
    let (content_text, content_html, content_rtf, source_app_path) = conn.query_row(
        "SELECT content_text, content_html, content_rtf, source_app_path FROM clipboard_items WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    Ok(ClipDetail {
        item,
        content_text,
        content_html,
        content_rtf,
        source_app_path,
    })
}

pub fn query(conn: &Connection, q: &ClipQuery) -> AppResult<ClipPage> {
    let limit = q.limit.clamp(1, 200) as i64;
    let mut wheres: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(m) = q.keyword.as_deref().and_then(tokenize::match_query) {
        wheres.push("id IN (SELECT rowid FROM clipboard_fts WHERE clipboard_fts MATCH ?)".into());
        args.push(Box::new(m));
    }
    if !q.kinds.is_empty() {
        let marks = vec!["?"; q.kinds.len()].join(",");
        wheres.push(format!("type IN ({marks})"));
        for k in &q.kinds {
            args.push(Box::new(k.clone()));
        }
    }
    if q.pinned_only {
        wheres.push("pinned = 1".into());
    }
    if q.favorite_only {
        wheres.push("favorite = 1".into());
    }
    if let Some(g) = q.group_id {
        wheres.push("group_id = ?".into());
        args.push(Box::new(g));
    }
    if let Some(cursor) = q.cursor.as_deref().and_then(parse_cursor) {
        let (p, t, id) = cursor;
        wheres.push(
            "(pinned < ? OR (pinned = ? AND (last_used_at < ? OR (last_used_at = ? AND id < ?))))"
                .into(),
        );
        args.push(Box::new(p));
        args.push(Box::new(p));
        args.push(Box::new(t));
        args.push(Box::new(t));
        args.push(Box::new(id));
    }

    let where_sql = if wheres.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", wheres.join(" AND "))
    };
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM clipboard_items {where_sql}
         ORDER BY pinned DESC, last_used_at DESC, id DESC LIMIT {}",
        limit + 1
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut items: Vec<ClipItem> = stmt
        .query_map(params_from_iter(args.iter().map(|a| a.as_ref())), map_item)?
        .collect::<Result<_, _>>()?;

    let next_cursor = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items
            .last()
            .map(|it| format!("{}:{}:{}", it.pinned as i64, it.last_used_at, it.id))
    } else {
        None
    };
    Ok(ClipPage { items, next_cursor })
}

fn parse_cursor(s: &str) -> Option<(i64, i64, i64)> {
    let mut parts = s.split(':').map(|p| p.parse::<i64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

pub fn stats(conn: &Connection) -> AppResult<ClipStats> {
    Ok(conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(pinned), 0), COALESCE(SUM(favorite), 0) FROM clipboard_items",
        [],
        |r| {
            Ok(ClipStats {
                total: r.get(0)?,
                pinned: r.get(1)?,
                favorite: r.get(2)?,
            })
        },
    )?)
}

/// 删除记录，返回需要一并删掉的文件（相对路径）。
pub fn delete(conn: &mut Connection, ids: &[i64]) -> AppResult<Vec<String>> {
    let tx = conn.transaction()?;
    let mut files = Vec::new();
    for id in ids {
        let row: Option<(Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT file_path, thumb_path FROM clipboard_items WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((f, t)) = row {
            files.extend(f);
            files.extend(t);
        }
        tx.execute("DELETE FROM clipboard_items WHERE id = ?1", [id])?;
        fts_delete(&tx, "clipboard_fts", *id)?;
    }
    tx.commit()?;
    Ok(files)
}

pub fn set_flag(conn: &Connection, id: i64, column: &str, value: bool) -> AppResult<()> {
    let column = match column {
        "pinned" => "pinned",
        "favorite" => "favorite",
        other => return Err(AppError::msg(format!("未知字段 {other}"))),
    };
    conn.execute(
        &format!("UPDATE clipboard_items SET {column} = ?1 WHERE id = ?2"),
        params![value, id],
    )?;
    Ok(())
}

pub fn set_note(conn: &Connection, id: i64, note: Option<&str>) -> AppResult<()> {
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    conn.execute(
        "UPDATE clipboard_items SET note = ?1 WHERE id = ?2",
        params![note, id],
    )?;
    // 备注也要能搜到：重建这一条的索引
    let (text, files, app): (Option<String>, Option<String>, Option<String>) = conn.query_row(
        "SELECT content_text, file_list, source_app FROM clipboard_items WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let mut all = format!(
        "{} {} {}",
        text.unwrap_or_default(),
        files.unwrap_or_default(),
        app.unwrap_or_default()
    );
    if let Some(n) = note {
        all.push(' ');
        all.push_str(n);
    }
    fts_upsert(conn, "clipboard_fts", id, &all)?;
    Ok(())
}

pub fn set_group(conn: &Connection, ids: &[i64], group: Option<i64>) -> AppResult<()> {
    for id in ids {
        conn.execute(
            "UPDATE clipboard_items SET group_id = ?1 WHERE id = ?2",
            params![group, id],
        )?;
    }
    Ok(())
}

pub fn list_groups(conn: &Connection) -> AppResult<Vec<ClipGroup>> {
    let mut stmt = conn.prepare(
        "SELECT g.id, g.name, g.color, (SELECT COUNT(*) FROM clipboard_items i WHERE i.group_id = g.id)
         FROM clipboard_groups g ORDER BY g.sort_order, g.id",
    )?;
    let groups = stmt
        .query_map([], |r| {
            Ok(ClipGroup {
                id: r.get(0)?,
                name: r.get(1)?,
                color: r.get(2)?,
                count: r.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(groups)
}

pub fn create_group(conn: &Connection, name: &str) -> AppResult<i64> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::msg("分组名不能为空"));
    }
    conn.execute(
        "INSERT INTO clipboard_groups(name, sort_order, created_at)
         VALUES (?1, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM clipboard_groups), ?2)",
        params![name, now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn rename_group(conn: &Connection, id: i64, name: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE clipboard_groups SET name = ?1 WHERE id = ?2",
        params![name.trim(), id],
    )?;
    Ok(())
}

pub fn delete_group(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM clipboard_groups WHERE id = ?1", [id])?;
    Ok(())
}

/// 清理候选：`scope` 为 all / unpinned。永不删除置顶、收藏、有备注、已分组的条目
/// （"all" 也一样遵守，除非显式 force）。
pub fn ids_for_clear(conn: &Connection, scope: &str) -> AppResult<Vec<i64>> {
    let sql = match scope {
        "everything" => "SELECT id FROM clipboard_items",
        _ => {
            "SELECT id FROM clipboard_items
              WHERE pinned = 0 AND favorite = 0 AND note IS NULL AND group_id IS NULL"
        }
    };
    let mut stmt = conn.prepare(sql)?;
    let ids = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

/// 保留策略（默认关闭）：超出天数或条数的普通条目。
pub fn ids_for_retention(conn: &Connection, days: u32, max_items: u32) -> AppResult<Vec<i64>> {
    let protected = "pinned = 0 AND favorite = 0 AND note IS NULL AND group_id IS NULL";
    let mut ids: Vec<i64> = Vec::new();
    if days > 0 {
        let cutoff = now_ms() - i64::from(days) * 86_400_000;
        let mut stmt = conn.prepare(&format!(
            "SELECT id FROM clipboard_items WHERE {protected} AND last_used_at < ?1"
        ))?;
        ids.extend(stmt.query_map([cutoff], |r| r.get::<_, i64>(0))?.flatten());
    }
    if max_items > 0 {
        let mut stmt = conn.prepare(&format!(
            "SELECT id FROM clipboard_items WHERE {protected}
             ORDER BY last_used_at DESC LIMIT -1 OFFSET ?1"
        ))?;
        ids.extend(
            stmt.query_map([i64::from(max_items)], |r| r.get::<_, i64>(0))?
                .flatten(),
        );
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// 数据库里引用到的全部文件（用于孤儿文件清理）。
/// 图片条目的 (id, 原图, 缩略图)，升级缩略图用。
pub fn image_thumbs(conn: &Connection) -> AppResult<Vec<(i64, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_path, thumb_path FROM clipboard_items
         WHERE type = 'image' AND file_path IS NOT NULL AND thumb_path IS NOT NULL",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn set_thumb(conn: &Connection, id: i64, thumb: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE clipboard_items SET thumb_path = ?1 WHERE id = ?2",
        params![thumb, id],
    )?;
    Ok(())
}

pub fn referenced_files(conn: &Connection) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT file_path FROM clipboard_items WHERE file_path IS NOT NULL
         UNION ALL SELECT thumb_path FROM clipboard_items WHERE thumb_path IS NOT NULL",
    )?;
    let files = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    fn text_clip(text: &str) -> NewClip {
        NewClip {
            kind: "text".into(),
            content_text: Some(text.into()),
            preview: Some(text.into()),
            hash: format!("h-{text}"),
            ..Default::default()
        }
    }

    #[test]
    fn insert_query_search_and_paginate() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            for i in 0..30 {
                insert(c, &text_clip(&format!("第{i}条 内容")))?;
            }
            insert(c, &text_clip("这是一份重要文档"))?;
            Ok(())
        })
        .unwrap();

        let found = db
            .with(|c| {
                query(
                    c,
                    &ClipQuery {
                        keyword: Some("文档".into()),
                        limit: 10,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        assert_eq!(found.items.len(), 1);
        assert_eq!(found.items[0].preview.as_deref(), Some("这是一份重要文档"));

        let mut seen = 0;
        let mut cursor = None;
        loop {
            let page = db
                .with(|c| {
                    query(
                        c,
                        &ClipQuery {
                            cursor: cursor.clone(),
                            limit: 7,
                            ..Default::default()
                        },
                    )
                })
                .unwrap();
            seen += page.items.len();
            if page.next_cursor.is_none() {
                break;
            }
            cursor = page.next_cursor;
        }
        assert_eq!(seen, 31);
    }

    #[test]
    fn pinned_first_and_dedup_touch() {
        let db = Db::open_in_memory().unwrap();
        let (a, b) = db
            .with(|c| {
                let a = insert(c, &text_clip("a"))?;
                let b = insert(c, &text_clip("b"))?;
                Ok((a, b))
            })
            .unwrap();
        db.with(|c| set_flag(c, a, "pinned", true)).unwrap();
        let page = db
            .with(|c| {
                query(
                    c,
                    &ClipQuery {
                        limit: 10,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        assert_eq!(page.items[0].id, a);
        assert_eq!(db.with(|c| find_duplicate(c, "h-b")).unwrap(), Some(b));
        assert_eq!(db.with(|c| find_duplicate(c, "h-zzz")).unwrap(), None);
    }

    #[test]
    fn delete_removes_from_index() {
        let db = Db::open_in_memory().unwrap();
        let id = db.with(|c| insert(c, &text_clip("独一无二"))).unwrap();
        db.with(|c| delete(c, &[id])).unwrap();
        let found = db
            .with(|c| {
                query(
                    c,
                    &ClipQuery {
                        keyword: Some("独一".into()),
                        limit: 10,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        assert!(found.items.is_empty());
    }
}
