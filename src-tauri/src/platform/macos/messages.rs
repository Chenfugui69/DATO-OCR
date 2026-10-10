//! 读「信息」App 的本地数据库（`~/Library/Messages/chat.db`），拿 iPhone 转发过来的短信和 iMessage。
//!
//! iPhone 上打开 设置 → 信息 → 短信转发 之后，短信会同步到同一个 Apple ID 登录的 Mac。
//! 这个库受「完全磁盘访问」保护：没授权时连打开都会被系统拒绝（EPERM）。只读打开，不改任何东西。
//!
//! 没有"来新消息了"的通知可等，只能轮询：先看库文件和 WAL 的修改时间 / 大小有没有变，
//! 变了才查一次 `ROWID` 比上次大的消息。开关打开的那一刻记下当时最大的 `ROWID`，之前的消息一概不管。

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};

use crate::platform::SmsMessage;

/// 多久看一次库文件有没有变
const POLL: Duration = Duration::from_millis(700);
/// 打不开库（多半是没授权）之后隔多久再试
const RETRY: Duration = Duration::from_secs(5);
/// 比这更早收到的不管：睡眠醒来一下子补进来的旧短信，验证码早过期了
const MAX_AGE_MS: i64 = 5 * 60 * 1000;
/// 2001-01-01（苹果的时间起点）的 Unix 秒数
const APPLE_EPOCH: i64 = 978_307_200;

const MESSAGES_APP: &str = "/System/Applications/Messages.app";

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// 测试开关 `CHENOCR_TEST_MESSAGES_DB=某个 .db`：读这个库（结构和 chat.db 一样），不用完全磁盘访问权限。
fn db_path() -> PathBuf {
    match std::env::var_os("CHENOCR_TEST_MESSAGES_DB") {
        Some(p) => PathBuf::from(p),
        None => home().join("Library/Messages/chat.db"),
    }
}

/// 有没有「完全磁盘访问」：试着打开 chat.db；这台 Mac 从没用过「信息」（没有这个文件）的话，
/// 改看同样受保护的 Safari 目录。
pub fn full_disk_access_granted() -> bool {
    match std::fs::File::open(db_path()) {
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            std::fs::read_dir(home().join("Library/Safari")).is_ok()
        }
        Err(_) => false,
    }
}

pub fn start(tx: Sender<SmsMessage>, enabled: Box<dyn Fn() -> bool + Send>) {
    let _ = std::thread::Builder::new()
        .name("sms-watch".into())
        .spawn(move || run(&db_path(), &tx, enabled.as_ref()));
}

/// 库文件和 WAL 的 (修改时间, 大小)。新消息先写进 WAL，所以两个都要看。
fn stamp(db: &Path) -> Vec<Option<(SystemTime, u64)>> {
    let wal = db.with_extension("db-wal");
    [db, wal.as_path()]
        .iter()
        .map(|p| {
            let m = std::fs::metadata(p).ok()?;
            Some((m.modified().ok()?, m.len()))
        })
        .collect()
}

fn run(db: &Path, tx: &Sender<SmsMessage>, enabled: &dyn Fn() -> bool) {
    let mut conn: Option<Connection> = None;
    // 已经看过的最大 ROWID；None = 刚打开开关，还没定起点
    let mut cursor: Option<i64> = None;
    let mut last_stamp = Vec::new();
    let mut warned = false;
    loop {
        std::thread::sleep(POLL);
        if !enabled() {
            conn = None;
            cursor = None;
            last_stamp.clear();
            continue;
        }
        let now_stamp = stamp(db);
        if cursor.is_some() && now_stamp == last_stamp {
            continue;
        }
        if conn.is_none() {
            match open(db) {
                Ok(c) => {
                    conn = Some(c);
                    warned = false;
                }
                Err(err) => {
                    if !warned {
                        tracing::warn!("打不开「信息」的数据库（没有完全磁盘访问权限？）：{err}");
                        warned = true;
                    }
                    std::thread::sleep(RETRY);
                    continue;
                }
            }
        }
        let Some(c) = conn.as_ref() else { continue };
        let result = match cursor {
            None => max_rowid(c).map(|max| {
                cursor = Some(max);
                tracing::info!(max, "开始读取短信");
            }),
            Some(after) => fetch(c, after).map(|rows| {
                let now = now_ms();
                for (rowid, msg) in rows {
                    cursor = Some(rowid);
                    if now - msg.at_ms <= MAX_AGE_MS {
                        let _ = tx.send(msg);
                    }
                }
            }),
        };
        match result {
            Ok(()) => last_stamp = now_stamp,
            Err(err) => {
                tracing::warn!("读取短信失败：{err}");
                conn = None;
            }
        }
    }
}

fn open(db: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // 「信息」正在写的时候别立刻报"库被锁住"
    conn.busy_timeout(Duration::from_millis(500))?;
    Ok(conn)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

fn max_rowid(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT IFNULL(MAX(ROWID), 0) FROM message", [], |r| {
        r.get(0)
    })
}

/// `ROWID` 比 `after` 大的收到的消息（自己发出去的不要），按先后排。
fn fetch(conn: &Connection, after: i64) -> rusqlite::Result<Vec<(i64, SmsMessage)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.ROWID, m.text, m.attributedBody, h.id, m.date
           FROM message m LEFT JOIN handle h ON h.ROWID = m.handle_id
          WHERE m.ROWID > ?1 AND m.is_from_me = 0
          ORDER BY m.ROWID LIMIT 200",
    )?;
    let rows = stmt.query_map([after], |r| {
        let rowid: i64 = r.get(0)?;
        let text: Option<String> = r.get(1)?;
        let body: Option<Vec<u8>> = r.get(2)?;
        let sender: Option<String> = r.get(3)?;
        let date: Option<i64> = r.get(4)?;
        Ok((rowid, text, body, sender, date))
    })?;
    let app_path = Some(PathBuf::from(MESSAGES_APP)).filter(|p| p.exists());
    let mut out = Vec::new();
    for row in rows {
        let (rowid, text, body, sender, date) = row?;
        // 新系统上 `text` 常常是空的，正文只在 `attributedBody` 里
        let text = text
            .filter(|t| !t.trim().is_empty())
            .or_else(|| body.as_deref().and_then(attributed_text));
        let Some(text) = text else { continue };
        out.push((
            rowid,
            SmsMessage {
                text,
                sender,
                at_ms: apple_time_to_ms(date.unwrap_or_default()),
                app_path: app_path.clone(),
            },
        ));
    }
    Ok(out)
}

/// `message.date`：High Sierra 起是 2001 年以来的纳秒，更早是秒。
fn apple_time_to_ms(date: i64) -> i64 {
    let since_2001_ms = if date > 1_000_000_000_000 {
        date / 1_000_000
    } else {
        date * 1000
    };
    since_2001_ms + APPLE_EPOCH * 1000
}

/// 从 `attributedBody`（NSAttributedString 的 typedstream 归档）里抠出正文。
///
/// 不解整个归档：正文紧跟在类名 `NSString` 后面，中间隔几个固定字节、以 `+` 结尾，
/// 然后是长度（一个字节；`0x81` 后跟 2 字节、`0x82` 后跟 4 字节，小端）和 UTF-8 正文。
fn attributed_text(blob: &[u8]) -> Option<String> {
    const CLASS: &[u8] = b"NSString";
    let at = blob.windows(CLASS.len()).position(|w| w == CLASS)? + CLASS.len();
    let rest = &blob[at..];
    let plus = rest.iter().take(8).position(|&b| b == b'+')?;
    let rest = &rest[plus + 1..];
    let (&tag, rest) = rest.split_first()?;
    let (len, rest) = match tag {
        0x81 => (
            usize::from(u16::from_le_bytes(rest.get(..2)?.try_into().ok()?)),
            &rest[2..],
        ),
        0x82 => (
            u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize,
            &rest[4..],
        ),
        n => (usize::from(n), rest),
    };
    let text = String::from_utf8_lossy(rest.get(..len)?).into_owned();
    Some(text).filter(|t| !t.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 照真实归档的样子拼一段：前面一堆别的字节、类名、固定的几个字节、长度、正文、后面还有别的
    fn archive(text: &str) -> Vec<u8> {
        let mut blob = b"\x04\x0bstreamtyped\x81\xe8\x03\x84\x01@\x84\x84\x84\x12NSAttributedString\x00\x84\x84\x08NSObject\x00\x85\x92\x84\x84\x84\x08".to_vec();
        blob.extend_from_slice(b"NSString\x01\x94\x84\x01+");
        let bytes = text.as_bytes();
        match bytes.len() {
            n if n < 0x80 => blob.push(n as u8),
            n if n <= 0xFFFF => {
                blob.push(0x81);
                blob.extend_from_slice(&(n as u16).to_le_bytes());
            }
            n => {
                blob.push(0x82);
                blob.extend_from_slice(&(n as u32).to_le_bytes());
            }
        }
        blob.extend_from_slice(bytes);
        blob.extend_from_slice(b"\x86\x84\x02iI\x01\x05\x92\x84\x84\x84\x0cNSDictionary");
        blob
    }

    #[test]
    fn decodes_attributed_body() {
        let short = "【淘宝】验证码 482913，5 分钟内有效";
        assert_eq!(attributed_text(&archive(short)).as_deref(), Some(short));
        let long = "验证码 123456。".repeat(30);
        assert!(long.len() > 0x80);
        assert_eq!(
            attributed_text(&archive(&long)).as_deref(),
            Some(long.as_str())
        );
        assert_eq!(attributed_text(b"no class name here"), None);
        assert_eq!(attributed_text(b"NSString\x01\x94\x84\x01+\x40ab"), None);
    }

    #[test]
    fn converts_apple_time() {
        // 2026-10-10 00:00:00 UTC
        let unix_ms = 1_791_590_400_000_i64;
        let ns = (unix_ms / 1000 - APPLE_EPOCH) * 1_000_000_000;
        assert_eq!(apple_time_to_ms(ns), unix_ms);
        assert_eq!(apple_time_to_ms(ns / 1_000_000_000), unix_ms);
    }

    /// 和「信息」的库结构一样的一个小库：只取收到的、`text` 空了从 `attributedBody` 拿
    #[test]
    fn fetches_incoming_messages_after_cursor() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
             CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT, attributedBody BLOB,
                                   handle_id INTEGER, date INTEGER, is_from_me INTEGER);
             INSERT INTO handle VALUES (1, '+8610690000');",
        )
        .unwrap();
        let add = |text: Option<&str>, body: Option<Vec<u8>>, from_me: i64| {
            conn.execute(
                "INSERT INTO message (text, attributedBody, handle_id, date, is_from_me)
                 VALUES (?1, ?2, 1, 800000000000000000, ?3)",
                rusqlite::params![text, body, from_me],
            )
            .unwrap();
        };
        add(Some("旧消息 111111"), None, 0);
        let start = max_rowid(&conn).unwrap();
        add(Some("验证码 222222"), None, 0);
        add(Some("我发出去的 333333"), None, 1);
        add(None, Some(archive("验证码 444444")), 0);
        add(None, None, 0);

        let rows = fetch(&conn, start).unwrap();
        let texts: Vec<&str> = rows.iter().map(|(_, m)| m.text.as_str()).collect();
        assert_eq!(texts, ["验证码 222222", "验证码 444444"]);
        assert_eq!(rows[0].1.sender.as_deref(), Some("+8610690000"));
        assert!(rows[0].0 > start);
    }
}
