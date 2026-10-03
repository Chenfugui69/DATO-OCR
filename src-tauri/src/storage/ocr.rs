//! 识字记录表。

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::{fts_delete, fts_upsert, now_ms, tokenize};
use crate::error::{AppError, AppResult};

#[derive(Clone, Debug)]
pub struct NewOcrRecord {
    pub screenshot_id: Option<i64>,
    pub image_path: Option<String>,
    pub width: u32,
    pub height: u32,
    pub engine: String,
    pub raw_blocks: String,
    pub plain_text: String,
    pub lang: Option<String>,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrRecordSummary {
    pub id: i64,
    pub image_path: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub engine: String,
    pub preview: String,
    pub char_count: i64,
    pub elapsed_ms: Option<i64>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrRecord {
    pub id: i64,
    pub screenshot_id: Option<i64>,
    pub image_path: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub engine: String,
    pub raw_blocks: String,
    pub plain_text: String,
    pub lang: Option<String>,
    pub elapsed_ms: Option<i64>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OcrQuery {
    pub keyword: Option<String>,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrPage {
    pub items: Vec<OcrRecordSummary>,
    pub total: i64,
}

pub fn insert(conn: &Connection, rec: &NewOcrRecord) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO ocr_records(screenshot_id, image_path, width, height, engine, raw_blocks, plain_text,
            lang, elapsed_ms, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            rec.screenshot_id,
            rec.image_path,
            rec.width,
            rec.height,
            rec.engine,
            rec.raw_blocks,
            rec.plain_text,
            rec.lang,
            rec.elapsed_ms as i64,
            now_ms()
        ],
    )?;
    let id = conn.last_insert_rowid();
    fts_upsert(conn, "ocr_fts", id, &rec.plain_text)?;
    Ok(id)
}

fn map_full(row: &Row<'_>) -> rusqlite::Result<OcrRecord> {
    Ok(OcrRecord {
        id: row.get(0)?,
        screenshot_id: row.get(1)?,
        image_path: row.get(2)?,
        width: row.get(3)?,
        height: row.get(4)?,
        engine: row.get(5)?,
        raw_blocks: row.get(6)?,
        plain_text: row.get(7)?,
        lang: row.get(8)?,
        elapsed_ms: row.get(9)?,
        created_at: row.get(10)?,
    })
}

pub fn get(conn: &Connection, id: i64) -> AppResult<OcrRecord> {
    conn.query_row(
        "SELECT id, screenshot_id, image_path, width, height, engine, raw_blocks, plain_text, lang,
            elapsed_ms, created_at FROM ocr_records WHERE id = ?1",
        [id],
        map_full,
    )
    .optional()?
    .ok_or_else(|| AppError::NotFound(format!("识字记录 {id}")))
}

pub fn update_text(conn: &Connection, id: i64, text: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE ocr_records SET plain_text = ?1 WHERE id = ?2",
        params![text, id],
    )?;
    fts_upsert(conn, "ocr_fts", id, text)
}

pub fn query(conn: &Connection, q: &OcrQuery) -> AppResult<OcrPage> {
    let limit = q.limit.clamp(1, 200);
    let (where_sql, arg) = match q.keyword.as_deref().and_then(tokenize::match_query) {
        Some(m) => (
            "WHERE id IN (SELECT rowid FROM ocr_fts WHERE ocr_fts MATCH ?1)".to_string(),
            Some(m),
        ),
        None => (String::new(), None),
    };
    let total: i64 = match &arg {
        Some(a) => conn.query_row(
            &format!("SELECT COUNT(*) FROM ocr_records {where_sql}"),
            [a],
            |r| r.get(0),
        )?,
        None => conn.query_row("SELECT COUNT(*) FROM ocr_records", [], |r| r.get(0))?,
    };
    let sql = format!(
        "SELECT id, image_path, width, height, engine, substr(plain_text, 1, 240), length(plain_text),
            elapsed_ms, created_at
         FROM ocr_records {where_sql} ORDER BY created_at DESC, id DESC LIMIT {limit} OFFSET {}",
        q.offset
    );
    let mut stmt = conn.prepare(&sql)?;
    let mapper = |r: &Row<'_>| {
        Ok(OcrRecordSummary {
            id: r.get(0)?,
            image_path: r.get(1)?,
            width: r.get(2)?,
            height: r.get(3)?,
            engine: r.get(4)?,
            preview: r.get(5)?,
            char_count: r.get(6)?,
            elapsed_ms: r.get(7)?,
            created_at: r.get(8)?,
        })
    };
    let items = match &arg {
        Some(a) => stmt.query_map([a], mapper)?.collect::<Result<_, _>>()?,
        None => stmt.query_map([], mapper)?.collect::<Result<_, _>>()?,
    };
    Ok(OcrPage { items, total })
}

pub fn delete(conn: &Connection, id: i64) -> AppResult<Option<String>> {
    let path: Option<Option<String>> = conn
        .query_row(
            "SELECT image_path FROM ocr_records WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    conn.execute("DELETE FROM ocr_records WHERE id = ?1", [id])?;
    fts_delete(conn, "ocr_fts", id)?;
    // 图片可能和截图库共用，只有 ocr/ 目录下的才归识字记录所有
    Ok(path.flatten().filter(|p| p.starts_with("ocr/")))
}

pub fn referenced_files(conn: &Connection) -> AppResult<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT image_path FROM ocr_records WHERE image_path IS NOT NULL")?;
    let files = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(files)
}
