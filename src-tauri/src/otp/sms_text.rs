//! 从一条短信里认出验证码。纯文字处理，和平台无关。
//!
//! 这里的中文是用来匹配短信内容的关键词，不是界面文案（`scripts/backend-strings.mjs` 跳过这个文件）。

/// 中文关键词直接按子串找
const KEYWORDS_ZH: &[&str] = &[
    "验证码",
    "校验码",
    "确认码",
    "动态码",
    "动态密码",
    "安全码",
    "安全代码",
    "登录码",
    "授权码",
    "验证代码",
    "认证码",
    "驗證碼",
    "認證碼",
    "確認碼",
    "安全碼",
];
/// 英文关键词要前后不挨着字母数字（`otp` 不能匹配 `hotpot`）。单独一个 `code` 太泛
/// （"Use code SAVE20"），只认这些搭配
const KEYWORDS_EN: &[&str] = &[
    "verification code",
    "verification",
    "security code",
    "login code",
    "sign-in code",
    "one-time",
    "otp",
    "passcode",
    "auth code",
    "authentication code",
    "your code",
    "code is",
    "code:",
    "2fa",
];
/// 数字前面是这些的不是验证码（"尾号 1234"）
const PREFIX_EXCLUDE: &[&str] = &[
    "尾号",
    "尾數",
    "卡号",
    "账号",
    "帳號",
    "手机",
    "手机号",
    "电话",
    "單號",
    "单号",
    "编号",
    "ending in",
    "ending",
    "account",
    "order",
    "no.",
    "tel",
];
/// 数字后面紧跟这些的是数量 / 日期（"2026年"、"10分钟"）
const SUFFIX_EXCLUDE: &[char] = &[
    '年', '月', '日', '号', '時', '时', '分', '秒', '元', '%', '个', '次', '天', '位', '条', '件',
    '岁',
];
/// 候选离关键词最远多少个字
const MAX_DISTANCE: usize = 60;

/// 从一条短信里认出验证码。没有验证码相关的关键词就不认，免得把普通数字当成验证码。
///
/// 候选是 4–8 位的纯数字，或 4–8 位、字母数字都有的串；`123-456` / `123 456` 这种拆开写的
/// 合成一个。纯数字优先，同类里离关键词最近的胜出（在关键词后面的比前面的略占优）。
pub fn extract(text: &str) -> Option<String> {
    let orig: Vec<char> = text.chars().collect();
    let lower: Vec<char> = orig.iter().map(char::to_ascii_lowercase).collect();
    let keywords = find_keywords(&lower);
    if keywords.is_empty() {
        return None;
    }

    // (纯数字? 0 : 1, 距离, 验证码)
    let mut best: Option<(u8, usize, String)> = None;
    let mut i = 0;
    while i < lower.len() {
        if !lower[i].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }
        let start = i;
        while i < lower.len() && lower[i].is_ascii_alphanumeric() {
            i += 1;
        }
        let mut end = i;
        let mut token: String = orig[start..end].iter().collect();

        // 123-456 / 123 456
        if is_digits(&token, 3) {
            if let Some(next_end) = digit_group_after(&lower, end) {
                token.extend(orig[end + 1..next_end].iter());
                end = next_end;
                i = end;
            }
        }

        let tier =
            if token.len() >= 4 && token.len() <= 8 && token.bytes().all(|b| b.is_ascii_digit()) {
                0
            } else if (4..=8).contains(&token.len())
                && token.bytes().any(|b| b.is_ascii_digit())
                && token.bytes().any(|b| b.is_ascii_alphabetic())
            {
                1
            } else {
                continue;
            };
        if excluded(&lower, start, end) {
            continue;
        }
        let distance = keywords
            .iter()
            .filter_map(|&(ks, ke)| {
                if start >= ke {
                    Some(start - ke)
                } else if end <= ks {
                    Some(ks - end + 2)
                } else {
                    None
                }
            })
            .min();
        let Some(distance) = distance.filter(|d| *d <= MAX_DISTANCE) else {
            continue;
        };
        let better = best
            .as_ref()
            .is_none_or(|(t, d, _)| (tier, distance) < (*t, *d));
        if better {
            best = Some((tier, distance, token));
        }
    }
    best.map(|(_, _, code)| code)
}

fn is_digits(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit())
}

/// `end` 处是 `-` 或空格、后面紧跟正好 3 位数字：返回那 3 位的结尾
fn digit_group_after(chars: &[char], end: usize) -> Option<usize> {
    if !matches!(chars.get(end), Some('-' | ' ')) {
        return None;
    }
    let s = end + 1;
    let mut e = s;
    while e < chars.len() && chars[e].is_ascii_alphanumeric() {
        e += 1;
    }
    let group: String = chars[s..e].iter().collect();
    is_digits(&group, 3).then_some(e)
}

/// 关键词出现的位置 `[start, end)`（按字符算）
fn find_keywords(lower: &[char]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut scan = |kw: &str, word: bool| {
        let kw: Vec<char> = kw.chars().collect();
        if kw.len() > lower.len() {
            return;
        }
        for s in 0..=lower.len() - kw.len() {
            if lower[s..s + kw.len()] != kw[..] {
                continue;
            }
            let e = s + kw.len();
            let edge = |c: Option<&char>| c.is_none_or(|c| !c.is_ascii_alphanumeric());
            let first_alnum = kw.first().is_some_and(char::is_ascii_alphanumeric);
            let last_alnum = kw.last().is_some_and(char::is_ascii_alphanumeric);
            if word
                && ((first_alnum && !edge(s.checked_sub(1).and_then(|p| lower.get(p))))
                    || (last_alnum && !edge(lower.get(e))))
            {
                continue;
            }
            found.push((s, e));
        }
    };
    for kw in KEYWORDS_ZH {
        scan(kw, false);
    }
    for kw in KEYWORDS_EN {
        scan(kw, true);
    }
    found
}

/// 候选 `[start, end)` 是不是电话号码的一段、网址的一部分、数量日期、"尾号 xxxx"
fn excluded(lower: &[char], start: usize, end: usize) -> bool {
    let before = start.checked_sub(1).and_then(|p| lower.get(p)).copied();
    let before2 = start.checked_sub(2).and_then(|p| lower.get(p)).copied();
    let after = lower.get(end).copied();
    let after2 = lower.get(end + 1).copied();
    let alnum = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    let digit = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());

    // 138 0013 8000 这种分段写的号码
    if (matches!(before, Some(' ' | '-')) && digit(before2))
        || (matches!(after, Some(' ' | '-')) && digit(after2))
    {
        return true;
    }
    // 网址、邮箱、小数
    if matches!(before, Some('/' | '@' | '_' | '=' | '&' | '+'))
        || matches!(after, Some('/' | '@' | '_' | '=' | '&'))
        || (before == Some('.') && alnum(before2))
        || (after == Some('.') && alnum(after2))
    {
        return true;
    }
    // 2026年、10分钟
    let next = lower[end..].iter().find(|c| !c.is_whitespace()).copied();
    if next.is_some_and(|c| SUFFIX_EXCLUDE.contains(&c)) {
        return true;
    }
    // 尾号 1234、ending in 1234
    let head: String = lower[start.saturating_sub(12)..start].iter().collect();
    let head =
        head.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '：' | '为' | '是'));
    PREFIX_EXCLUDE.iter().any(|p| head.ends_with(p))
}

#[cfg(test)]
mod tests {
    use super::extract;

    #[test]
    fn finds_codes() {
        let cases = [
            (
                "【淘宝】您的验证码为 482913，5分钟内有效，请勿泄露。",
                "482913",
            ),
            ("您的验证码是8264。如非本人操作请忽略", "8264"),
            ("【京东】482913是您的登录验证码，请勿告诉他人", "482913"),
            ("G-582014 is your Google verification code.", "582014"),
            (
                "Your Apple Account code is: 529183. Don't share it with anyone.",
                "529183",
            ),
            ("验证码：AB12CD，10分钟内有效", "AB12CD"),
            ("动态密码 652341，请于2分钟内输入。客服电话 95555", "652341"),
            (
                "Your WhatsApp code: 123-456. Don't share this code.",
                "123456",
            ),
            (
                "【招商银行】验证码 739102，您尾号6789的卡正在进行网上支付",
                "739102",
            ),
            (
                "[Microsoft] 使用安全代码 7319 验证你的 Microsoft 帐户",
                "7319",
            ),
            ("Your verification code is 004821", "004821"),
            ("您正在修改密码，验证码 3388，2026年10月10日前有效", "3388"),
        ];
        for (sms, code) in cases {
            assert_eq!(extract(sms).as_deref(), Some(code), "{sms}");
        }
    }

    #[test]
    fn ignores_messages_without_codes() {
        let cases = [
            "【招商银行】您尾号6789的账户于2026年10月10日支出 1280.00 元",
            "Use code SAVE20 for 20% off your next order",
            "明天下午 3 点开会，地址见 https://example.com/room/4821",
            "验证码已发送到尾号 1234 的手机",
            "你的快递到了，取件请联系 138 0013 8000",
            "Your one-time setup is complete",
            "hotpot 8848 tonight?",
        ];
        for sms in cases {
            assert_eq!(extract(sms), None, "{sms}");
        }
    }
}
