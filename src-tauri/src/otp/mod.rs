//! 短信验证码。
//!
//! Mac 上读「信息」里 iPhone 转发来的短信（`platform::start_sms_watcher`），认出验证码后：
//! 放进本机剪贴板、进剪贴板历史、右下角提示；开着多端同步的话带上 `otp` 标记发给其他设备，
//! 对方（比如 Windows 电脑）收到后同样提示并写进剪贴板（`sync::receive`）。
//!
//! 验证码在历史里是单独一类（`type = otp`）：卡片上显示验证码和短信原文，可以打码；
//! 过了设置的时间（默认 1 分钟）自动从历史里删掉，置顶、收藏的除外。

mod sms_text;

use std::sync::mpsc;
use std::time::Duration;

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

pub fn start(app: &AppHandle) {
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
                    deliver(&app, &code, &msg);
                }
            }
        });
}

fn deliver(app: &AppHandle, code: &str, msg: &SmsMessage) {
    let st = state(app);
    let source = crate::i18n::text("信息");
    let id = if st.settings.read().clipboard.enabled {
        crate::clipboard::store_otp(app, code, &msg.text, &source, msg.app_path.as_deref())
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
