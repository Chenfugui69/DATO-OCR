//! SQLite 存储（规格 07 §7）。
//!
//! 桌面应用的写入量很小，一个连接 + 互斥锁足够；查询都走索引，持锁时间是毫秒级。
//! **禁止在持锁时做 IO 以外的慢操作**（图片编码、网络请求之类先做完再进锁）。

pub mod clipboard;
pub mod ocr;
pub mod screenshots;
pub mod secrets;
pub mod sync;
pub mod tokenize;

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppResult;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../migrations/001_init.sql")),
    (2, include_str!("../../migrations/002_sync.sql")),
];

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        let existed = path.exists();
        let conn = Connection::open(path)?;
        apply_pragmas(&conn)?;
        migrate(&conn, existed.then_some(path))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> AppResult<Self> {
        let conn = Connection::open_in_memory()?;
        apply_pragmas(&conn)?;
        migrate(&conn, None)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> AppResult<T>) -> AppResult<T> {
        let mut conn = self.conn.lock();
        f(&mut conn)
    }

    pub fn kv_get(&self, key: &str) -> AppResult<Option<String>> {
        self.with(|c| {
            Ok(
                c.query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
                    .optional()?,
            )
        })
    }

    pub fn kv_set(&self, key: &str, value: &str) -> AppResult<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO kv(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
    }
}

fn apply_pragmas(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA cache_size = -20000;
         PRAGMA temp_store = MEMORY;",
    )?;
    Ok(())
}

/// 逐个应用未执行的迁移。升级 schema 前先把旧库备份一份（只保留最近一份）。
fn migrate(conn: &Connection, existing_file: Option<&Path>) -> AppResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
           version INTEGER PRIMARY KEY,
           applied_at INTEGER NOT NULL
         );",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;

    let pending: Vec<_> = MIGRATIONS.iter().filter(|(v, _)| *v > current).collect();
    if pending.is_empty() {
        return Ok(());
    }

    if let (Some(file), true) = (existing_file, current > 0) {
        let backup = file.with_extension(format!("db.bak.v{current}"));
        // VACUUM INTO 得到的是一致的快照，比直接拷文件（还有 WAL）可靠
        let _ = std::fs::remove_file(&backup);
        if let Err(err) = conn.execute("VACUUM INTO ?1", [backup.to_string_lossy()]) {
            tracing::warn!("迁移前备份数据库失败：{err}");
        }
    }

    for (version, sql) in pending {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES (?1, ?2)",
            params![version, now_ms()],
        )?;
        tx.commit()?;
        tracing::info!(version, "已应用数据库迁移");
    }
    Ok(())
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 往 contentless FTS 表写一行（先删后插，等价于 upsert）。
pub(crate) fn fts_upsert(conn: &Connection, table: &str, rowid: i64, text: &str) -> AppResult<()> {
    conn.execute(&format!("DELETE FROM {table} WHERE rowid = ?1"), [rowid])?;
    let tokens = tokenize::index_tokens(text);
    if !tokens.trim().is_empty() {
        conn.execute(
            &format!("INSERT INTO {table}(rowid, tokens) VALUES (?1, ?2)"),
            params![rowid, tokens],
        )?;
    }
    Ok(())
}

pub(crate) fn fts_delete(conn: &Connection, table: &str, rowid: i64) -> AppResult<()> {
    conn.execute(&format!("DELETE FROM {table} WHERE rowid = ?1"), [rowid])?;
    Ok(())
}
