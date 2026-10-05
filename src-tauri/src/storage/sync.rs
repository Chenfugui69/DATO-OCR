//! 同步过的设备（规格 09）。密钥在这里只是一段字节，加解密由 `sync` 模块负责。

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::now_ms;
use crate::error::AppResult;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub device_id: String,
    pub name: String,
    /// windows | macos | ios | android | web
    pub platform: String,
    /// desktop：跑着 DATO COR 的电脑；web：浏览器 / 快捷指令
    pub kind: String,
    /// member：加入了本机；host：本机加入的主机
    pub role: String,
    pub address: Option<String>,
    pub code: Option<String>,
    pub last_seen: Option<i64>,
    pub created_at: i64,
    #[serde(skip)]
    pub secret: Vec<u8>,
}

const COLUMNS: &str =
    "device_id, name, platform, kind, role, secret, address, code, last_seen, created_at";

fn map(row: &Row<'_>) -> rusqlite::Result<Peer> {
    Ok(Peer {
        device_id: row.get(0)?,
        name: row.get(1)?,
        platform: row.get(2)?,
        kind: row.get(3)?,
        role: row.get(4)?,
        secret: row.get(5)?,
        address: row.get(6)?,
        code: row.get(7)?,
        last_seen: row.get(8)?,
        created_at: row.get(9)?,
    })
}

pub fn upsert(conn: &Connection, peer: &Peer) -> AppResult<()> {
    conn.execute(
        "INSERT INTO sync_peers (device_id, name, platform, kind, role, secret, address, code, last_seen, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(device_id) DO UPDATE SET name = excluded.name, platform = excluded.platform,
           kind = excluded.kind, role = excluded.role, secret = excluded.secret,
           address = excluded.address, code = excluded.code, last_seen = excluded.last_seen",
        params![
            peer.device_id,
            peer.name,
            peer.platform,
            peer.kind,
            peer.role,
            peer.secret,
            peer.address,
            peer.code,
            peer.last_seen,
            if peer.created_at > 0 { peer.created_at } else { now_ms() },
        ],
    )?;
    Ok(())
}

pub fn list(conn: &Connection) -> AppResult<Vec<Peer>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM sync_peers ORDER BY created_at"
    ))?;
    let rows = stmt.query_map([], map)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn get(conn: &Connection, device_id: &str) -> AppResult<Option<Peer>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM sync_peers WHERE device_id = ?1"),
            [device_id],
            map,
        )
        .optional()?)
}

/// 网页 / 快捷指令按令牌哈希找设备。
pub fn find_web(conn: &Connection, token_hash: &[u8]) -> AppResult<Option<Peer>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM sync_peers WHERE kind = 'web' AND role = 'member' AND secret = ?1"
            ),
            [token_hash],
            map,
        )
        .optional()?)
}

pub fn host(conn: &Connection) -> AppResult<Option<Peer>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM sync_peers WHERE role = 'host' LIMIT 1"),
            [],
            map,
        )
        .optional()?)
}

pub fn remove(conn: &Connection, device_id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM sync_peers WHERE device_id = ?1", [device_id])?;
    Ok(())
}

pub fn remove_role(conn: &Connection, role: &str) -> AppResult<()> {
    conn.execute("DELETE FROM sync_peers WHERE role = ?1", [role])?;
    Ok(())
}

pub fn seen(conn: &Connection, device_id: &str, address: Option<&str>) -> AppResult<()> {
    conn.execute(
        "UPDATE sync_peers SET last_seen = ?1, address = COALESCE(?2, address) WHERE device_id = ?3",
        params![now_ms(), address, device_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    #[test]
    fn peers_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            upsert(
                c,
                &Peer {
                    device_id: "web1".into(),
                    name: "iPhone".into(),
                    kind: "web".into(),
                    role: "member".into(),
                    secret: vec![7; 32],
                    ..Default::default()
                },
            )?;
            upsert(
                c,
                &Peer {
                    device_id: "mac".into(),
                    name: "MacBook".into(),
                    kind: "desktop".into(),
                    role: "host".into(),
                    secret: vec![1; 32],
                    code: Some("12345678".into()),
                    ..Default::default()
                },
            )?;
            assert_eq!(list(c)?.len(), 2);
            assert_eq!(find_web(c, &[7; 32])?.unwrap().name, "iPhone");
            assert!(find_web(c, &[1; 32])?.is_none());
            assert_eq!(host(c)?.unwrap().device_id, "mac");
            seen(c, "mac", Some("192.168.1.9:47380"))?;
            assert_eq!(
                get(c, "mac")?.unwrap().address.as_deref(),
                Some("192.168.1.9:47380")
            );
            remove_role(c, "host")?;
            assert!(host(c)?.is_none());
            Ok(())
        })
        .unwrap();
    }
}
