-- 短信验证码（type = 'otp'）：content_text 是验证码本身（粘贴出去的就是它），这里存短信原文。
ALTER TABLE clipboard_items ADD COLUMN origin_text TEXT;
