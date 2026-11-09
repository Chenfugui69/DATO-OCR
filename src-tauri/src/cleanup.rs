//! 存储清理：设置 → 存储里的"清理"（一键全清 / 展开后按类单独清）和定时自动清理。
//!
//! 四类：剪贴板历史、截图库、识字记录、日志。用户特意留下的不删：剪贴板置顶 / 收藏 / 有备注 /
//! 已分组的，截图库收藏 / 有备注的（和剪贴板面板里"清空"的规则一样）。

use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};
use crate::events;
use crate::state::state;
use crate::storage::{now_ms, ocr, screenshots};

pub const TARGETS: &[&str] = &["clipboard", "screenshots", "ocr", "logs"];

/// 定时清理多久跑一次（启动后先等一会儿，之后每 6 小时）
const SCHEDULE_INTERVAL: Duration = Duration::from_secs(6 * 3600);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub key: String,
    pub bytes: u64,
    /// 记录条数（日志是文件数）
    pub count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanResult {
    /// 删掉的记录 / 文件数
    pub removed: usize,
    pub freed_bytes: u64,
}

pub fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn file_count(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .map(|d| d.flatten().filter(|e| e.path().is_file()).count() as u64)
        .unwrap_or(0)
}

fn table_count(app: &AppHandle, table: &str) -> u64 {
    state(app)
        .db
        .with(|c| {
            Ok(c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
                r.get::<_, i64>(0)
            })?)
        })
        .map(|n| n.max(0) as u64)
        .unwrap_or(0)
}

pub fn usage(app: &AppHandle) -> Vec<Usage> {
    let root = state(app).paths.root().to_path_buf();
    TARGETS
        .iter()
        .map(|&key| {
            let (bytes, count) = match key {
                "clipboard" => (dir_size(&root.join("clipboard")), table_count(app, "clipboard_items")),
                "screenshots" => (dir_size(&root.join("screenshots")), table_count(app, "screenshots")),
                "ocr" => (dir_size(&root.join("ocr")), table_count(app, "ocr_records")),
                _ => (dir_size(&root.join("logs")), file_count(&root.join("logs"))),
            };
            Usage {
                key: key.into(),
                bytes,
                count,
            }
        })
        .collect()
}

fn ids(app: &AppHandle, sql: &str, cutoff: i64) -> AppResult<Vec<i64>> {
    state(app).db.with(|c| {
        let mut stmt = c.prepare(sql)?;
        let ids = stmt
            .query_map([cutoff], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(ids)
    })
}

fn clean_clipboard(app: &AppHandle, cutoff: i64) -> AppResult<usize> {
    let ids = ids(
        app,
        "SELECT id FROM clipboard_items
          WHERE pinned = 0 AND favorite = 0 AND note IS NULL AND group_id IS NULL
            AND last_used_at < ?1",
        cutoff,
    )?;
    if !ids.is_empty() {
        crate::clipboard::delete(app, &ids)?;
    }
    Ok(ids.len())
}

fn clean_screenshots(app: &AppHandle, cutoff: i64) -> AppResult<usize> {
    let ids = ids(
        app,
        "SELECT id FROM screenshots WHERE favorite = 0 AND note IS NULL AND created_at < ?1",
        cutoff,
    )?;
    if ids.is_empty() {
        return Ok(0);
    }
    let st = state(app);
    let files = st.db.with(|c| {
        let mut files = Vec::new();
        for id in &ids {
            files.extend(screenshots::delete(c, *id)?);
        }
        Ok(files)
    })?;
    for f in files {
        let _ = std::fs::remove_file(st.paths.abs(&f));
    }
    let _ = app.emit(events::LIBRARY_CHANGED, ());
    Ok(ids.len())
}

fn clean_ocr(app: &AppHandle, cutoff: i64) -> AppResult<usize> {
    let ids = ids(app, "SELECT id FROM ocr_records WHERE created_at < ?1", cutoff)?;
    if ids.is_empty() {
        return Ok(0);
    }
    let st = state(app);
    let files = st.db.with(|c| {
        let mut files = Vec::new();
        for id in &ids {
            files.extend(ocr::delete(c, *id)?);
        }
        Ok(files)
    })?;
    for f in files {
        let _ = std::fs::remove_file(st.paths.abs(&f));
    }
    let _ = app.emit(events::OCR_HISTORY_CHANGED, ());
    Ok(ids.len())
}

/// 日志：最新的那个文件正在写，留着；其余早于截止时间的删掉
fn clean_logs(app: &AppHandle, cutoff: SystemTime) -> usize {
    let dir = state(app).paths.logs();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut files: Vec<(SystemTime, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), e.path()))
        })
        .collect();
    files.sort_by_key(|(t, _)| *t);
    files.pop();
    files
        .into_iter()
        .filter(|(t, _)| *t < cutoff)
        .filter(|(_, p)| std::fs::remove_file(p).is_ok())
        .count()
}

/// 清理 `targets` 里的几类；`older_than_days` = 0 表示不论新旧全清（受保护的除外）
pub fn clean(app: &AppHandle, targets: &[String], older_than_days: u32) -> AppResult<CleanResult> {
    if let Some(bad) = targets.iter().find(|t| !TARGETS.contains(&t.as_str())) {
        return Err(AppError::msg(format!("未知的清理项 {bad}")));
    }
    let span_ms = i64::from(older_than_days) * 86_400_000;
    let cutoff = if older_than_days == 0 { i64::MAX } else { now_ms() - span_ms };
    let cutoff_time = if older_than_days == 0 {
        SystemTime::now() + Duration::from_secs(3600)
    } else {
        SystemTime::now() - Duration::from_secs(u64::from(older_than_days) * 86_400)
    };
    let before: u64 = usage(app).iter().map(|u| u.bytes).sum();
    let mut removed = 0usize;
    for t in targets {
        removed += match t.as_str() {
            "clipboard" => clean_clipboard(app, cutoff)?,
            "screenshots" => clean_screenshots(app, cutoff)?,
            "ocr" => clean_ocr(app, cutoff)?,
            _ => clean_logs(app, cutoff_time),
        };
    }
    // 删了一大批记录后把 WAL 收回来（数据库文件本身的空洞留给每月一次的 VACUUM）
    let _ = state(app).db.with(|c| {
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    });
    let after: u64 = usage(app).iter().map(|u| u.bytes).sum();
    let freed_bytes = before.saturating_sub(after);
    tracing::info!(removed, freed_bytes, ?targets, older_than_days, "存储清理");
    Ok(CleanResult {
        removed,
        freed_bytes,
    })
}

/// 定时清理：按设置里的天数和勾选的类别，删掉早于那个天数的
fn run_scheduled(app: &AppHandle) {
    let s = state(app).settings.read().storage.clone();
    if s.auto_clean_days == 0 || s.auto_clean_targets.is_empty() {
        return;
    }
    if let Err(err) = clean(app, &s.auto_clean_targets, s.auto_clean_days) {
        tracing::warn!("定时清理失败：{err}");
    }
}

pub fn spawn_schedule(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        // 让出启动高峰（后台维护在 15 秒，错开）
        std::thread::sleep(Duration::from_secs(60));
        loop {
            run_scheduled(&app);
            std::thread::sleep(SCHEDULE_INTERVAL);
        }
    });
}
