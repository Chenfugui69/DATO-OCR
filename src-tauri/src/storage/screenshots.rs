//! 截图库表。

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row, ToSql};
use serde::{Deserialize, Serialize};

use super::{fts_delete, fts_upsert, now_ms, tokenize};
use crate::error::{AppError, AppResult};

#[derive(Clone, Debug)]
pub struct NewShot {
    pub file_path: String,
    pub thumb_path: Option<String>,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub kind: String,
    pub source_app: Option<String>,
    pub has_annotations: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shot {
    pub id: i64,
    pub file_path: String,
    pub thumb_path: Option<String>,
    pub width: i64,
    pub height: i64,
    pub size_bytes: i64,
    pub kind: String,
    pub source_app: Option<String>,
    pub ocr_text: Option<String>,
    pub has_annotations: bool,
    pub favorite: bool,
    pub note: Option<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShotQuery {
    pub keyword: Option<String>,
    pub favorite_only: bool,
    pub kind: Option<String>,
    /// `created_at:id`
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotPage {
    pub items: Vec<Shot>,
    pub next_cursor: Option<String>,
    pub total: i64,
}

const COLUMNS: &str =
    "id, file_path, thumb_path, width, height, size_bytes, kind, source_app, ocr_text,
    has_annotations, favorite, note, created_at";

fn map(row: &Row<'_>) -> rusqlite::Result<Shot> {
    Ok(Shot {
        id: row.get(0)?,
        file_path: row.get(1)?,
        thumb_path: row.get(2)?,
        width: row.get(3)?,
        height: row.get(4)?,
        size_bytes: row.get(5)?,
        kind: row.get(6)?,
        source_app: row.get(7)?,
        ocr_text: row.get(8)?,
        has_annotations: row.get(9)?,
        favorite: row.get(10)?,
        note: row.get(11)?,
        created_at: row.get(12)?,
    })
}

pub fn insert(conn: &Connection, shot: &NewShot) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO screenshots(file_path, thumb_path, width, height, size_bytes, kind, source_app,
            has_annotations, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            shot.file_path,
            shot.thumb_path,
            shot.width,
            shot.height,
            shot.size_bytes as i64,
            shot.kind,
            shot.source_app,
            shot.has_annotations,
            now_ms()
        ],
    )?;
    let id = conn.last_insert_rowid();
    if let Some(app) = &shot.source_app {
        fts_upsert(conn, "screenshots_fts", id, app)?;
    }
    Ok(id)
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Shot> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM screenshots WHERE id = ?1"),
        [id],
        map,
    )
    .optional()?
    .ok_or_else(|| AppError::NotFound(format!("截图 {id}")))
}

pub fn query(conn: &Connection, q: &ShotQuery) -> AppResult<ShotPage> {
    let limit = q.limit.clamp(1, 500) as i64;
    let mut wheres: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(m) = q.keyword.as_deref().and_then(tokenize::match_query) {
        wheres
            .push("id IN (SELECT rowid FROM screenshots_fts WHERE screenshots_fts MATCH ?)".into());
        args.push(Box::new(m));
    }
    if q.favorite_only {
        wheres.push("favorite = 1".into());
    }
    if let Some(kind) = &q.kind {
        wheres.push("kind = ?".into());
        args.push(Box::new(kind.clone()));
    }
    let base_where = if wheres.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", wheres.join(" AND "))
    };
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM screenshots {base_where}"),
        params_from_iter(args.iter().map(|a| a.as_ref())),
        |r| r.get(0),
    )?;

    if let Some((t, id)) = q.cursor.as_deref().and_then(|c| {
        let (a, b) = c.split_once(':')?;
        Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?))
    }) {
        wheres.push("(created_at < ? OR (created_at = ? AND id < ?))".into());
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
        "SELECT {COLUMNS} FROM screenshots {where_sql} ORDER BY created_at DESC, id DESC LIMIT {}",
        limit + 1
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut items: Vec<Shot> = stmt
        .query_map(params_from_iter(args.iter().map(|a| a.as_ref())), map)?
        .collect::<Result<_, _>>()?;
    let next_cursor = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|s| format!("{}:{}", s.created_at, s.id))
    } else {
        None
    };
    Ok(ShotPage {
        items,
        next_cursor,
        total,
    })
}

pub fn delete(conn: &Connection, id: i64) -> AppResult<Vec<String>> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT file_path, thumb_path FROM screenshots WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    conn.execute("DELETE FROM screenshots WHERE id = ?1", [id])?;
    fts_delete(conn, "screenshots_fts", id)?;
    Ok(row
        .map(|(f, t)| std::iter::once(f).chain(t).collect())
        .unwrap_or_default())
}

pub fn set_favorite(conn: &Connection, id: i64, favorite: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE screenshots SET favorite = ?1 WHERE id = ?2",
        params![favorite, id],
    )?;
    Ok(())
}

pub fn set_note(conn: &Connection, id: i64, note: Option<&str>) -> AppResult<()> {
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    conn.execute(
        "UPDATE screenshots SET note = ?1 WHERE id = ?2",
        params![note, id],
    )?;
    reindex(conn, id)
}

pub fn set_ocr_text(conn: &Connection, id: i64, text: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE screenshots SET ocr_text = ?1 WHERE id = ?2",
        params![text, id],
    )?;
    reindex(conn, id)
}

fn reindex(conn: &Connection, id: i64) -> AppResult<()> {
    let (ocr, note, app): (Option<String>, Option<String>, Option<String>) = conn.query_row(
        "SELECT ocr_text, note, source_app FROM screenshots WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let text = [ocr, note, app]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    fts_upsert(conn, "screenshots_fts", id, &text)
}

pub fn referenced_files(conn: &Connection) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT file_path FROM screenshots UNION ALL SELECT thumb_path FROM screenshots WHERE thumb_path IS NOT NULL",
    )?;
    let files = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(files)
}
