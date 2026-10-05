//! 设备之间传的一条剪贴板记录。和传输方式无关：局域网、WebDAV 都传它（加密后）。

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

const MAGIC: &[u8; 4] = b"DCR1";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncRecord {
    /// 记录的全局 ID（`clipboard_items.sync_id`），各设备靠它去重
    pub id: String,
    /// 最早复制它的设备
    pub origin: String,
    pub origin_name: String,
    /// text | link | color | image
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 在原设备上是从哪个应用复制的
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub created_at: i64,
    /// 这次发出的时间。同一条记录再复制一次会再发一次，接收方据此判断"是不是刚复制的"
    pub sent_at: i64,
    /// 图片原图（PNG / JPEG 字节），编码时放在头部后面，不进 JSON
    #[serde(skip)]
    pub image: Option<Vec<u8>>,
}

impl SyncRecord {
    /// `DCR1 ‖ 头长度(u32 LE) ‖ 头(JSON) ‖ 图片字节`
    pub fn encode(&self) -> Vec<u8> {
        let head = serde_json::to_vec(self).unwrap_or_default();
        let image = self.image.as_deref().unwrap_or_default();
        let mut out = Vec::with_capacity(8 + head.len() + image.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(head.len() as u32).to_le_bytes());
        out.extend_from_slice(&head);
        out.extend_from_slice(image);
        out
    }

    pub fn decode(bytes: &[u8]) -> AppResult<Self> {
        let bad = || AppError::msg("同步记录格式不对");
        if bytes.len() < 8 || &bytes[..4] != MAGIC {
            return Err(bad());
        }
        let len = u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| bad())?) as usize;
        let head = bytes.get(8..8 + len).ok_or_else(bad)?;
        let mut record: SyncRecord = serde_json::from_slice(head)?;
        let rest = &bytes[8 + len..];
        record.image = (!rest.is_empty()).then(|| rest.to_vec());
        if record.id.is_empty() || record.origin.is_empty() {
            return Err(bad());
        }
        match record.kind.as_str() {
            "text" | "link" | "color" if record.text.is_some() => Ok(record),
            "image" if record.image.is_some() => Ok(record),
            _ => Err(bad()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_text_and_image() {
        let text = SyncRecord {
            id: "a".into(),
            origin: "dev".into(),
            origin_name: "PC".into(),
            kind: "text".into(),
            text: Some("你好".into()),
            created_at: 1,
            sent_at: 2,
            ..Default::default()
        };
        assert_eq!(SyncRecord::decode(&text.encode()).unwrap(), text);

        let image = SyncRecord {
            kind: "image".into(),
            text: None,
            width: Some(2),
            height: Some(3),
            image: Some(vec![1, 2, 3, 4]),
            ..text.clone()
        };
        assert_eq!(SyncRecord::decode(&image.encode()).unwrap(), image);
    }

    #[test]
    fn rejects_garbage_and_empty_payloads() {
        assert!(SyncRecord::decode(b"nope").is_err());
        let empty = SyncRecord {
            id: "a".into(),
            origin: "dev".into(),
            kind: "image".into(),
            ..Default::default()
        };
        assert!(SyncRecord::decode(&empty.encode()).is_err());
    }
}
