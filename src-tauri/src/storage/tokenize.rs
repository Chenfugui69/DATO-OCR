//! 全文索引用的中文分词（规格 05 §2.1 方案 1：bigram，零依赖）。
//!
//! FTS5 的 unicode61 不切分中文：`这是一份文档` 会被当成一个词，搜"文档"搜不到。
//! 写入索引前把连续的 CJK 字符展开成"单字 + 相邻二字"，用空格隔开：
//!
//! ```text
//! 你好世界 → 你 好 世 界 你好 好世 世界
//! ```
//!
//! 查询时：单个汉字匹配单字 token，两个及以上汉字拆成 bigram 全部 AND。
//! 非 CJK 文本原样交给 unicode61（它会按空白/标点切词、大小写折叠），查询时做前缀匹配。

/// 索引文本上限。剪贴板单条最大 5MB，全量展开会让索引膨胀到不可接受。
const MAX_INDEX_CHARS: usize = 200_000;

pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF   // CJK 扩展 A
        | 0x4E00..=0x9FFF // CJK 基本
        | 0xF900..=0xFAFF // 兼容汉字
        | 0x3040..=0x30FF // 平假名 / 片假名
        | 0xAC00..=0xD7AF // 谚文
        | 0x20000..=0x2FA1F)
}

/// 把任意文本转成写入 FTS 的 token 串。
pub fn index_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_INDEX_CHARS * 4) * 2);
    let mut run: Vec<char> = Vec::new();

    let flush = |run: &mut Vec<char>, out: &mut String| {
        if run.is_empty() {
            return;
        }
        for c in run.iter() {
            out.push(' ');
            out.push(*c);
        }
        for pair in run.windows(2) {
            out.push(' ');
            out.push(pair[0]);
            out.push(pair[1]);
        }
        out.push(' ');
        run.clear();
    };

    for c in text.chars().take(MAX_INDEX_CHARS) {
        if is_cjk(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// 用户输入的搜索词 → FTS5 MATCH 表达式。没有可搜的内容时返回 None。
///
/// 只产出由字母数字和 CJK 字符组成的带引号短语，不会把用户输入里的 FTS 语法
/// （`AND`、`*`、`"`、`NEAR` 等）原样透传，所以不存在注入或语法错误。
pub fn match_query(input: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut cjk: Vec<char> = Vec::new();
    let mut word = String::new();

    fn flush_cjk(cjk: &mut Vec<char>, terms: &mut Vec<String>) {
        match cjk.len() {
            0 => {}
            1 => terms.push(format!("\"{}\"", cjk[0])),
            _ => {
                for pair in cjk.windows(2) {
                    terms.push(format!("\"{}{}\"", pair[0], pair[1]));
                }
            }
        }
        cjk.clear();
    }
    fn flush_word(word: &mut String, terms: &mut Vec<String>) {
        if !word.is_empty() {
            terms.push(format!("\"{}\"*", word));
            word.clear();
        }
    }

    for c in input.chars() {
        if is_cjk(c) {
            flush_word(&mut word, &mut terms);
            cjk.push(c);
        } else if c.is_alphanumeric() {
            flush_cjk(&mut cjk, &mut terms);
            word.push(c);
        } else {
            flush_cjk(&mut cjk, &mut terms);
            flush_word(&mut word, &mut terms);
        }
    }
    flush_cjk(&mut cjk, &mut terms);
    flush_word(&mut word, &mut terms);

    terms.dedup();
    (!terms.is_empty()).then(|| terms.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn search(docs: &[&str], query: &str) -> Vec<i64> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(tokens, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 2');",
        )
        .unwrap();
        for (i, doc) in docs.iter().enumerate() {
            conn.execute(
                "INSERT INTO t(rowid, tokens) VALUES (?1, ?2)",
                rusqlite::params![i as i64 + 1, index_tokens(doc)],
            )
            .unwrap();
        }
        let Some(q) = match_query(query) else {
            return vec![];
        };
        let mut stmt = conn
            .prepare("SELECT rowid FROM t WHERE t MATCH ?1 ORDER BY rowid")
            .unwrap();
        stmt.query_map([q], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn bigram_expansion() {
        assert_eq!(
            index_tokens("你好世界")
                .split_whitespace()
                .collect::<Vec<_>>(),
            vec!["你", "好", "世", "界", "你好", "好世", "世界"]
        );
    }

    #[test]
    fn finds_chinese_substring() {
        let docs = ["这是一份重要文档", "完全无关的内容", "文件档案"];
        assert_eq!(search(&docs, "文档"), vec![1]);
        assert_eq!(search(&docs, "重要文档"), vec![1]);
        assert_eq!(search(&docs, "档"), vec![1, 3]);
    }

    #[test]
    fn mixed_and_latin_prefix() {
        let docs = ["Hello World 你好", "help me", "nothing"];
        assert_eq!(search(&docs, "hel"), vec![1, 2]);
        assert_eq!(search(&docs, "hello 你好"), vec![1]);
        assert_eq!(search(&docs, "WORLD"), vec![1]);
    }

    #[test]
    fn fts_syntax_is_neutralised() {
        assert_eq!(
            match_query("\"* AND NEAR("),
            Some("\"AND\"* \"NEAR\"*".to_string())
        );
        assert_eq!(match_query("  ** "), None);
        let docs = ["a\"b"];
        assert!(search(&docs, "\"\"\"").is_empty());
    }
}
