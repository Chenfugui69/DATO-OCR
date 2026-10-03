//! 启动后的后台维护（规格 07 §7.4）：孤儿文件清理、定期 VACUUM。不阻塞 UI。

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use tauri::AppHandle;

use crate::error::AppResult;
use crate::state::state;
use crate::storage::{clipboard, now_ms, ocr, screenshots};

const VACUUM_INTERVAL_MS: i64 = 30 * 86_400_000;

pub fn spawn(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        // 让出启动高峰
        std::thread::sleep(Duration::from_secs(15));
        if let Err(err) = run(&app) {
            tracing::warn!("后台维护失败：{err}");
        }
    });
}

fn run(app: &AppHandle) -> AppResult<()> {
    let st = state(app);
    let mut referenced: HashSet<String> = HashSet::new();
    st.db.with(|c| {
        referenced.extend(clipboard::referenced_files(c)?);
        referenced.extend(screenshots::referenced_files(c)?);
        referenced.extend(ocr::referenced_files(c)?);
        Ok(())
    })?;
    let mut removed = 0usize;
    for category in ["clipboard", "screenshots", "ocr"] {
        removed += sweep(
            st.paths.root(),
            &st.paths.root().join(category),
            &referenced,
        );
    }
    if removed > 0 {
        tracing::info!(removed, "清理了孤儿文件");
    }

    let last: i64 = st
        .db
        .kv_get("last_vacuum_at")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if now_ms() - last > VACUUM_INTERVAL_MS {
        st.db.with(|c| {
            c.execute_batch("VACUUM;")?;
            Ok(())
        })?;
        st.db.kv_set("last_vacuum_at", &now_ms().to_string())?;
        tracing::info!("数据库 VACUUM 完成");
    }
    Ok(())
}

/// 删掉数据库里没有记录的文件。只动一小时前的文件，避免和正在写入的记录抢跑。
fn sweep(root: &Path, dir: &Path, referenced: &HashSet<String>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let cutoff = SystemTime::now() - Duration::from_secs(3600);
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            removed += sweep(root, &path, referenced);
            continue;
        }
        if meta.modified().is_ok_and(|m| m > cutoff) {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if !referenced.contains(&rel) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}
