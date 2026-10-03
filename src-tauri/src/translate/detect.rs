//! 本地语言检测（规格 04 §7）：先本地判断，省一次网络请求。结果只是默认值，界面上可改。

pub fn detect(text: &str) -> &'static str {
    let mut total = 0usize;
    let (mut han, mut kana, mut hangul, mut cyrillic, mut arabic, mut thai) = (0, 0, 0, 0, 0, 0);
    for c in text.chars().filter(|c| c.is_alphanumeric()) {
        total += 1;
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF => kana += 1,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => han += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul += 1,
            0x0400..=0x04FF => cyrillic += 1,
            0x0600..=0x06FF => arabic += 1,
            0x0E00..=0x0E7F => thai += 1,
            _ => {}
        }
    }
    if total == 0 {
        return "en";
    }
    let ratio = |n: usize| n as f64 / total as f64;
    let cjk = han + kana + hangul;
    if ratio(cjk) > 0.2 {
        // 有假名就是日文（日文里也大量用汉字）；有谚文就是韩文
        if kana > 0 && ratio(kana) > 0.05 {
            return "ja";
        }
        if hangul > han {
            return "ko";
        }
        return "zh";
    }
    if ratio(cyrillic) > 0.2 {
        return "ru";
    }
    if arabic > 0 && ratio(arabic) > 0.2 {
        return "ar";
    }
    if thai > 0 && ratio(thai) > 0.2 {
        return "th";
    }
    "en"
}

/// 默认目标语言：中文 → 英文，其他 → 中文。
pub fn default_target(source: &str) -> &'static str {
    if source == "zh" || source == "zh-TW" {
        "en"
    } else {
        "zh"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_languages() {
        assert_eq!(detect("这是一段中文 with some English"), "zh");
        assert_eq!(detect("The quick brown fox"), "en");
        assert_eq!(detect("これは日本語の文章です"), "ja");
        assert_eq!(detect("안녕하세요 반갑습니다"), "ko");
        assert_eq!(detect("Привет, как дела?"), "ru");
        assert_eq!(detect("12345 !!!"), "en");
        assert_eq!(default_target("zh"), "en");
        assert_eq!(default_target("ja"), "zh");
    }
}
