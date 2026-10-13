//! 从一封邮件里认出验证码：拆出主题、发件人、正文的文字，再交给认短信的那套规则（`sms_text`）。
//! 纯文字处理，不连网。

use mailparse::{MailHeaderMap, ParsedMail};

use super::sms_text;

/// 正文只看开头这么多字：验证码邮件都把验证码放在前面，后面是页脚、免责声明、一堆链接
const BODY_CHARS: usize = 3000;
/// 存进历史的"原文"里，正文留这么多字
const SNIPPET_CHARS: usize = 240;

pub struct MailCode {
    pub code: String,
    /// 发件人显示的名字，没有就是邮箱地址
    pub sender: String,
    /// 主题 + 正文开头一段，存进历史当"原文"
    pub origin: String,
}

/// 一封原始邮件（RFC 822）里的验证码；不是验证码邮件返回 None。
pub fn extract(raw: &[u8]) -> Option<MailCode> {
    let mail = mailparse::parse_mail(raw).ok()?;
    let subject = mail.headers.get_first_value("Subject").unwrap_or_default();
    let subject = collapse(&subject, false);
    let (mut plain, mut html) = (String::new(), String::new());
    collect(&mail, &mut plain, &mut html, 0);
    let plain = clip(&collapse(&plain, true));
    let html = clip(&collapse(&html_to_text(&html), true));

    // 主题里就有的最可信（"123456 是你的验证码"）；再看纯文本，最后看网页版的正文。
    // 主题里有关键词、验证码在正文里的也很常见，所以正文是连着主题一起认的
    let code = sms_text::extract(&subject).or_else(|| {
        [&plain, &html]
            .into_iter()
            .find_map(|body| in_body(&subject, body))
    })?;
    let body = if plain.is_empty() { &html } else { &plain };
    let snippet: String = body.chars().take(SNIPPET_CHARS).collect();
    let snippet = snippet.replace('\n', " ");
    Some(MailCode {
        code,
        sender: sender(&mail),
        origin: [subject.as_str(), snippet.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
    })
}

fn in_body(subject: &str, body: &str) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    sms_text::extract(body).or_else(|| {
        // 关键词只在主题里（"你的登录验证码"），正文开头就是那串数字
        let head: String = body.chars().take(80).collect();
        sms_text::extract(&format!("{subject} {head}"))
    })
}

/// 把邮件各部分里的文字收起来：纯文本的和网页版的分开放。附件不看。
fn collect(part: &ParsedMail, plain: &mut String, html: &mut String, depth: usize) {
    if depth > 8 {
        return;
    }
    if !part.subparts.is_empty() {
        for sub in &part.subparts {
            collect(sub, plain, html, depth + 1);
        }
        return;
    }
    if part.get_content_disposition().disposition == mailparse::DispositionType::Attachment {
        return;
    }
    let target = match part.ctype.mimetype.to_ascii_lowercase().as_str() {
        "text/plain" => plain,
        "text/html" => html,
        _ => return,
    };
    if let Ok(body) = part.get_body() {
        target.push_str(&body);
        target.push('\n');
    }
}

fn sender(mail: &ParsedMail) -> String {
    let from = mail.headers.get_first_value("From").unwrap_or_default();
    let parsed = mailparse::addrparse(&from).ok();
    let first = parsed.as_ref().and_then(|list| list.iter().next());
    match first {
        Some(mailparse::MailAddr::Single(one)) => one
            .display_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| one.addr.clone()),
        Some(mailparse::MailAddr::Group(group)) => group.group_name.clone(),
        None => collapse(&from, false),
    }
}

/// 连着的空白压成一个空格；零宽字符直接去掉（有的邮件在验证码中间塞这个）。
/// `keep_lines`：中间换过行的压成一个换行而不是空格 —— 单独占一行的验证码和下一行的"5 分钟内有效"
/// 不能连成 `739102 5`，那样像是分段写的电话号码。
fn collapse(text: &str, keep_lines: bool) -> String {
    let mut out = String::with_capacity(text.len());
    // 0 = 没有空白，1 = 有空白，2 = 空白里有换行
    let mut gap = 0;
    for c in text.chars() {
        if matches!(
            c,
            '\u{200b}'..='\u{200f}' | '\u{feff}' | '\u{34f}' | '\u{2060}'
        ) {
            continue;
        }
        if c.is_whitespace() {
            gap = gap.max(if c == '\n' && keep_lines { 2 } else { 1 });
        } else {
            if gap > 0 && !out.is_empty() {
                out.push(if gap == 2 { '\n' } else { ' ' });
            }
            gap = 0;
            out.push(c);
        }
    }
    out
}

fn clip(text: &str) -> String {
    text.chars().take(BODY_CHARS).collect()
}

/// 分段的标签：换成换行
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "br",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "ul",
    "ol",
    "tr",
    "td",
    "th",
    "table",
    "section",
    "center",
    "blockquote",
    "hr",
];

/// 网页版正文变成文字：丢掉 `<style>` / `<script>` / `<head>` 整段和所有标签，常见的实体换回字符。
/// 分段的标签换成换行，其余的换成空格 —— 一个数字一个格子的验证码认不出来就算了，
/// 不能把相邻两段里的数字粘成一个。
fn html_to_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len() / 2);
    let mut i = 0;
    while i < html.len() {
        let rest = &html[i..];
        if rest.starts_with('<') {
            let skipped = ["style", "script", "head", "title"].iter().find_map(|tag| {
                let after = lower[i..].strip_prefix('<')?.strip_prefix(tag)?;
                if !after.starts_with([' ', '>', '\t', '\n', '\r']) {
                    return None;
                }
                let close = format!("</{tag}");
                Some(lower[i..].find(&close).map_or(html.len(), |at| i + at))
            });
            if let Some(to) = skipped {
                i = to;
                // 落在结束标签上，下一轮把它当普通标签跳过
                if i >= html.len() {
                    break;
                }
            }
            let Some(end) = html[i..].find('>') else {
                break;
            };
            let name = lower[i + 1..i + end].trim_start_matches('/');
            let name = name
                .split(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .unwrap_or_default();
            out.push(if BLOCK_TAGS.contains(&name) {
                '\n'
            } else {
                ' '
            });
            i += end + 1;
        } else if rest.starts_with('&') {
            let end = rest.find(';').filter(|e| *e <= 10);
            let decoded = end.and_then(|e| entity(&rest[1..e]));
            match (end, decoded) {
                (Some(e), Some(c)) => {
                    out.push(c);
                    i += e + 1;
                }
                _ => {
                    out.push('&');
                    i += 1;
                }
            }
        } else {
            let c = rest.chars().next().unwrap_or(' ');
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "nbsp" | "ensp" | "emsp" | "thinsp" => ' ',
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(subject: &str, ctype: &str, body: &str) -> Vec<u8> {
        format!(
            "From: \"GitHub\" <noreply@github.com>\r\nTo: me@example.com\r\nSubject: {subject}\r\n\
             MIME-Version: 1.0\r\nContent-Type: {ctype}; charset=utf-8\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn plain_text_mail() {
        let raw = mail(
            "[GitHub] Please verify your device",
            "text/plain",
            "Hey there!\r\n\r\nVerification code: 731464\r\n\r\nIf you did not attempt to sign in, visit https://github.com/settings/security",
        );
        let found = extract(&raw).unwrap();
        assert_eq!(found.code, "731464");
        assert_eq!(found.sender, "GitHub");
        assert!(found
            .origin
            .starts_with("[GitHub] Please verify your device\nHey there!"));
    }

    #[test]
    fn code_in_subject() {
        let raw = mail(
            "482913 is your verification code",
            "text/plain",
            "Thanks for signing up.",
        );
        assert_eq!(extract(&raw).unwrap().code, "482913");
    }

    #[test]
    fn html_only_mail() {
        let raw = mail(
            "Your login code",
            "text/html",
            "<html><head><style>.a{width:600px;color:#123456}</style><title>x</title></head><body>\
             <p>Hi,</p><p>Your one-time code is</p><div style=\"font-size:32px\"><b>905&#8203;117</b></div>\
             <p>It expires in 10&nbsp;minutes.</p><script>var n = 20240101;</script></body></html>",
        );
        assert_eq!(extract(&raw).unwrap().code, "905117");
    }

    #[test]
    fn keyword_in_subject_code_in_body() {
        // 主题里的中文按邮件的规矩是编码过的（RFC 2047）：你的登录验证码
        let raw = mail(
            "=?UTF-8?B?5L2g55qE55m75b2V6aqM6K+B56CB?=",
            "text/html",
            "<div><h1>739102</h1><p>5 分钟内有效，请勿告诉他人。</p></div>",
        );
        assert_eq!(extract(&raw).unwrap().code, "739102");
    }

    #[test]
    fn multipart_prefers_plain() {
        let raw = b"From: Proton <no-reply@proton.me>\r\nSubject: Proton verification code\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\r\nYour Proton verification code is: 204=\r\n851\r\n--b1\r\n\
Content-Type: text/html; charset=utf-8\r\n\r\n<p>Your Proton verification code is: <b>204851</b></p>\r\n--b1--\r\n";
        let found = extract(raw).unwrap();
        assert_eq!(found.code, "204851");
        assert_eq!(found.sender, "Proton");
    }

    #[test]
    fn ordinary_mail_is_ignored() {
        for (subject, body) in [
            (
                "Your order #20458812 has shipped",
                "Order 20458812 will arrive on 2026-10-14. Total: 1299 USD.",
            ),
            (
                "Weekly newsletter",
                "Use code SAVE2026 for 20% off before 2026.",
            ),
            (
                "会议纪要",
                "明天 1030 在 3 号会议室开会，请带上 2025 年的报表。",
            ),
        ] {
            assert!(
                extract(&mail(subject, "text/plain", body)).is_none(),
                "{subject}"
            );
        }
    }
}
