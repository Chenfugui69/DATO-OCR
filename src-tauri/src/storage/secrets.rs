//! 密钥表：存的是平台加密后的密文（Windows DPAPI），绝不存明文（规格 04 §6.3）。

use rusqlite::{params, Connection, OptionalExtension};

use super::now_ms;
use crate::error::AppResult;
use crate::platform;

pub fn set(conn: &Connection, key: &str, plain: Option<&str>) -> AppResult<()> {
    match plain.map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) => {
            let cipher = platform::protect(value.as_bytes())?;
            conn.execute(
                "INSERT INTO secrets(key, cipher, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET cipher = excluded.cipher, updated_at = excluded.updated_at",
                params![key, cipher, now_ms()],
            )?;
        }
        None => {
            conn.execute("DELETE FROM secrets WHERE key = ?1", [key])?;
        }
    }
    Ok(())
}

pub fn get(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    let cipher: Option<Vec<u8>> = conn
        .query_row("SELECT cipher FROM secrets WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(cipher) = cipher else {
        return Ok(None);
    };
    match platform::unprotect(&cipher) {
        Ok(plain) => Ok(String::from_utf8(plain).ok()),
        Err(err) => {
            // 换了机器/用户后 DPAPI 解不开，视为未设置
            tracing::warn!(key, "密钥解密失败：{err}");
            Ok(None)
        }
    }
}

/// 脱敏显示：`sk-••••••••1234`（前 3 后 4）
pub fn masked(plain: &str) -> String {
    let chars: Vec<char> = plain.chars().collect();
    if chars.len() <= 8 {
        return "•".repeat(chars.len().max(4));
    }
    let head: String = chars[..3].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}••••••••{tail}")
}

#[cfg(test)]
mod tests {
    use super::masked;

    #[test]
    fn mask_keeps_head_and_tail() {
        assert_eq!(masked("sk-abcdefghijkl1234"), "sk-••••••••1234");
        assert_eq!(masked("short"), "•••••");
    }
}
