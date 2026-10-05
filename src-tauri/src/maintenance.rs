//! 启动后的后台维护（规格 07 §7.4）：孤儿文件清理、定期 VACUUM、一次性升级旧缩略图。不阻塞 UI。

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
    if let Err(err) = upgrade_thumbnails(app) {
        tracing::warn!("升级缩略图失败：{err}");
    }
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

/// 旧版缩略图按长边 320 / 480 缩，宽截图在卡片里糊（见 `imaging::CARD_THUMB_SHORT`）。
/// 从原图重新生成一份，存成新文件名（界面按地址缓存图片，同名覆盖的话已经打开的面板还显示旧的），
/// 旧文件不再被引用，由下面的孤儿清理删掉。只做一次，做完记一笔。
fn upgrade_thumbnails(app: &AppHandle) -> AppResult<()> {
    const KEY: &str = "card_thumbs_v2";
    let st = state(app);
    if st.db.kv_get(KEY)?.is_some() {
        return Ok(());
    }
    let mut rows: Vec<(bool, i64, String, String)> = Vec::new();
    st.db.with(|c| {
        rows.extend(
            clipboard::image_thumbs(c)?
                .into_iter()
                .map(|(id, f, t)| (true, id, f, t)),
        );
        rows.extend(
            screenshots::image_thumbs(c)?
                .into_iter()
                .map(|(id, f, t)| (false, id, f, t)),
        );
        Ok(())
    })?;
    let mut upgraded = 0usize;
    for (is_clip, id, file, thumb) in rows {
        let thumb_abs = st.paths.abs(&thumb);
        let Ok((tw, th)) = image::image_dimensions(&thumb_abs) else {
            continue;
        };
        if tw.min(th) >= crate::imaging::CARD_THUMB_SHORT {
            continue;
        }
        let Ok(original) = image::open(st.paths.abs(&file)) else {
            continue;
        };
        // 原图的短边也就这么点：缩略图已经是它能给的最清楚的了
        if original.width().min(original.height()) <= tw.min(th) {
            continue;
        }
        let new_rel = match file.rfind('.') {
            Some(dot) => format!("{}.card.jpg", &file[..dot]),
            None => format!("{file}.card.jpg"),
        };
        if crate::imaging::write_card_thumbnail(&original.to_rgba8(), &st.paths.abs(&new_rel))
            .is_err()
        {
            continue;
        }
        st.db.with(|c| {
            if is_clip {
                clipboard::set_thumb(c, id, &new_rel)
            } else {
                screenshots::set_thumb(c, id, &new_rel)
            }
        })?;
        upgraded += 1;
        // 慢慢来，别和用户抢 CPU
        std::thread::sleep(Duration::from_millis(30));
    }
    st.db.kv_set(KEY, "1")?;
    if upgraded > 0 {
        tracing::info!(upgraded, "重新生成了旧的卡片缩略图");
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
