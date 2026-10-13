//! 验证码：短信里的、邮件里的。
//!
//! 邮件：用 IMAP 连邮箱看新邮件（`mail`，Windows、macOS 都一样）。
//! 短信：Mac 上读「信息」里 iPhone 转发来的短信（`platform::start_sms_watcher`）。认出验证码后：
//! 放进本机剪贴板、进剪贴板历史、右下角提示；开着多端同步的话带上 `otp` 标记发给其他设备，
//! 对方（比如 Windows 电脑）收到后同样提示并写进剪贴板（`sync::receive`）。
//!
//! 验证码在历史里是单独一类（`type = otp`）：卡片上显示验证码和短信原文，可以打码；
//! 过了设置的时间（默认 1 分钟）自动从历史里删掉，置顶、收藏的除外。

pub mod mail;
mod mail_text;
mod sms_text;

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::AppHandle;

use crate::platform::{self, ClipboardPayload, SmsMessage};
use crate::state::state;
use crate::storage::{clipboard as repo, now_ms};
use crate::wm;

pub use sms_text::extract;

/// 同步记录里标记"这是验证码"
pub const TAG: &str = "otp";

/// 多久查一次有没有到期的验证码
const EXPIRE_CHECK: Duration = Duration::from_secs(5);
/// 同一个验证码这么久之内只算一次
const SEEN_WINDOW: Duration = Duration::from_secs(180);

/// 最近见过的验证码。同一个验证码可能从几条路来：这台设备自己连邮箱读到一次，别的设备读到后又同步过来一次；
/// 邮件在两个文件夹里各出现一次……只提示第一次。
static SEEN: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

/// 这个验证码是不是刚才没见过（顺手记下）。返回 false = 刚处理过，别再提示了。
pub fn first_sight(code: &str) -> bool {
    let mut seen = SEEN.lock();
    seen.retain(|(_, at)| at.elapsed() < SEEN_WINDOW);
    if seen.iter().any(|(c, _)| c == code) {
        return false;
    }
    seen.push((code.to_string(), Instant::now()));
    true
}

pub fn start(app: &AppHandle) {
    mail::reconfigure(app);

    // 收到的验证码（别的设备同步来的）到期也要删，所以哪个平台都开
    let sweeper = app.clone();
    let _ = std::thread::Builder::new()
        .name("otp-expire".into())
        .spawn(move || loop {
            std::thread::sleep(EXPIRE_CHECK);
            remove_expired(&sweeper);
        });

    if !platform::sms_supported() {
        return;
    }
    let (tx, rx) = mpsc::channel::<SmsMessage>();
    let check = app.clone();
    platform::start_sms_watcher(
        tx,
        Box::new(move || state(&check).settings.read().clipboard.sms_codes),
    );
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("otp".into())
        .spawn(move || {
            for msg in rx {
                if let Some(code) = extract(&msg.text) {
                    tracing::info!(from = ?msg.sender, "短信里认出验证码");
                    // 短信只有 iPhone 收得到，转发到 Mac 的也是从 iPhone 来的
                    let source = crate::i18n::text("信息");
                    let from = Found {
                        origin: &msg.text,
                        source: &source,
                        app_path: msg.app_path.as_deref(),
                        device_kind: Some("iphone"),
                    };
                    deliver(&app, &code, &from);
                }
            }
        });
}

/// 验证码是从哪认出来的
struct Found<'a> {
    /// 原文（短信全文 / 邮件的主题和开头一段）
    origin: &'a str,
    /// 历史里显示的来源（「信息」/ 发件人）
    source: &'a str,
    /// 来源应用，取图标用
    app_path: Option<&'a Path>,
    /// 来自哪种设备（短信是 iphone）；None = 就是这台设备
    device_kind: Option<&'a str>,
}

fn deliver(app: &AppHandle, code: &str, from: &Found) {
    if !first_sight(code) {
        tracing::debug!("这个验证码刚处理过，不再提示");
        return;
    }
    let st = state(app);
    let id = if st.settings.read().clipboard.enabled {
        crate::clipboard::store_otp(
            app,
            code,
            from.origin,
            from.source,
            from.app_path,
            from.device_kind,
        )
        .map_err(|err| tracing::warn!("验证码存进历史失败：{err}"))
        .ok()
    } else {
        None
    };
    let payload = ClipboardPayload::Text {
        text: code.to_string(),
        html: None,
        rtf: None,
    };
    if let Err(err) = crate::clipboard::write_own(app, &payload, id) {
        tracing::warn!("验证码写入剪贴板失败：{err}");
        return;
    }
    wm::toast(app, "success", format!("验证码 {code} 已复制"));
    if let Some(id) = id {
        crate::sync::on_local(app, id);
    }
}

fn remove_expired(app: &AppHandle) {
    let secs = state(app).settings.read().clipboard.otp_expire_secs;
    if secs == 0 {
        return;
    }
    let before = now_ms() - i64::from(secs) * 1000;
    match state(app).db.with(|c| repo::expired_otp_ids(c, before)) {
        Ok(ids) if !ids.is_empty() => {
            tracing::debug!(count = ids.len(), "删除到期的验证码");
            if let Err(err) = crate::clipboard::delete(app, &ids) {
                tracing::warn!("删除到期的验证码失败：{err}");
            }
        }
        Ok(_) => {}
        Err(err) => tracing::warn!("查到期的验证码失败：{err}"),
    }
}
