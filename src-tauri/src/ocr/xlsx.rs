//! 识别出来的表格导出成 Excel（.xlsx）：合并单元格照原样合并、表头加粗；
//! 看着是数字的写成真数字（保留千分位、小数位、百分号、括号负数的显示方式），
//! 太长的数字（12 位及以上：卡号、证件号、订单号）一律按文本写，免得 Excel 变成科学计数法、丢精度。

use rust_xlsxwriter::{Format, FormatAlign, Workbook};
use serde::Deserialize;

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableCell {
    pub text: String,
    pub header: bool,
    pub row_span: u32,
    pub col_span: u32,
}

/// 一个格子里的数字：值和 Excel 的数字格式
#[derive(Debug, PartialEq)]
pub struct ParsedNumber {
    pub value: f64,
    pub format: String,
}

/// 认数字：`1,234.50`、`-12`、`(1,200)`（会计写法的负数）、`12.5%`、前后可带 ¥ $ € £ 符号。
/// 认不出、或者数字位数 ≥ 12（按文本保留）返回 None。
pub fn parse_number(text: &str) -> Option<ParsedNumber> {
    let mut t = text.trim();
    if t.is_empty() {
        return None;
    }
    let mut currency = "";
    for sym in ["¥", "￥", "$", "€", "£"] {
        if let Some(rest) = t.strip_prefix(sym) {
            currency = sym;
            t = rest.trim_start();
            break;
        }
    }
    let (negative_paren, inner) = match t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        Some(inner) => (true, inner),
        None => (false, t),
    };
    let (percent, inner) = match inner.strip_suffix('%') {
        Some(rest) => (true, rest),
        None => (false, inner),
    };
    let (sign, digits) = match inner.strip_prefix('-').or_else(|| inner.strip_prefix('−')) {
        Some(rest) => (-1.0, rest),
        None => (1.0, inner.strip_prefix('+').unwrap_or(inner)),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '.') {
        return None;
    }
    let grouped = digits.contains(',');
    let (int_part, frac_part) = digits.split_once('.').unwrap_or((digits, ""));
    if frac_part.contains(',') || frac_part.contains('.') || int_part.is_empty() {
        return None;
    }
    // 千分位必须是规整的三位一组
    if grouped {
        let groups: Vec<&str> = int_part.split(',').collect();
        if groups[0].is_empty() || groups[0].len() > 3 || groups[1..].iter().any(|g| g.len() != 3) {
            return None;
        }
    }
    let plain: String = digits.chars().filter(|c| *c != ',').collect();
    if plain.chars().filter(char::is_ascii_digit).count() >= 12 {
        return None;
    }
    let mut value: f64 = plain.parse().ok()?;
    value *= sign;
    if negative_paren {
        value = -value.abs();
    }
    let decimals = frac_part.len();
    let mut base = if grouped { "#,##0".to_string() } else { "0".to_string() };
    if decimals > 0 {
        base.push('.');
        base.push_str(&"0".repeat(decimals));
    }
    let format = if percent {
        value /= 100.0;
        format!("{base}%")
    } else {
        let positive = format!("{currency}{base}");
        if negative_paren {
            format!("{positive};({positive})")
        } else {
            positive
        }
    };
    Some(ParsedNumber { value, format })
}

/// 写成 xlsx 文件。`rows` 是每行的格子（合并单元格只出现在左上角那一格）。
pub fn write(rows: &[Vec<TableCell>], path: &std::path::Path) -> AppResult<()> {
    let err = |e: rust_xlsxwriter::XlsxError| AppError::msg(format!("写 Excel 文件失败：{e}"));
    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    let header = Format::new().set_bold().set_align(FormatAlign::Top);
    let normal = Format::new().set_align(FormatAlign::Top);
    // 被合并单元格占掉的位置（行, 列）
    let mut taken = std::collections::HashSet::<(u32, u16)>::new();
    let mut widths: Vec<usize> = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let r = r as u32;
        let mut c: u16 = 0;
        for cell in row {
            while taken.contains(&(r, c)) {
                c += 1;
            }
            let rs = cell.row_span.clamp(1, 1000);
            let cs = cell.col_span.clamp(1, 1000) as u16;
            let base = if cell.header { &header } else { &normal };
            let number = if cell.header { None } else { parse_number(&cell.text) };
            let fmt = match &number {
                Some(n) => base.clone().set_num_format(&n.format),
                None => base.clone(),
            };
            if rs > 1 || cs > 1 {
                sheet
                    .merge_range(r, c, r + rs - 1, c + cs - 1, "", &fmt)
                    .map_err(err)?;
            }
            match &number {
                Some(n) => sheet.write_number_with_format(r, c, n.value, &fmt).map_err(err)?,
                None => sheet.write_string_with_format(r, c, &cell.text, &fmt).map_err(err)?,
            };
            for dr in 0..rs {
                for dc in 0..cs {
                    taken.insert((r + dr, c + dc));
                }
            }
            // 列宽按这一列里最长的字估（中文算两个字宽），合并的格子不算
            if cs == 1 {
                let w: usize = cell.text.chars().map(|ch| if ch.is_ascii() { 1 } else { 2 }).sum();
                let col = c as usize;
                if widths.len() <= col {
                    widths.resize(col + 1, 0);
                }
                widths[col] = widths[col].max(w);
            }
            c += cs;
        }
    }
    for (i, w) in widths.iter().enumerate() {
        sheet
            .set_column_width(i as u16, (*w as f64 + 2.0).clamp(6.0, 60.0))
            .map_err(err)?;
    }
    book.save(path).map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: f64, f: &str) -> Option<ParsedNumber> {
        Some(ParsedNumber { value: v, format: f.into() })
    }

    #[test]
    fn parses_financial_numbers() {
        assert_eq!(parse_number("1,234.50"), n(1234.5, "#,##0.00"));
        assert_eq!(parse_number("-12"), n(-12.0, "0"));
        assert_eq!(parse_number("(1,200)"), n(-1200.0, "#,##0;(#,##0)"));
        assert_eq!(parse_number("12.5%"), n(0.125, "0.0%"));
        assert_eq!(parse_number("¥3,000"), n(3000.0, "¥#,##0"));
    }

    #[test]
    fn keeps_long_numbers_and_text_as_text() {
        assert_eq!(parse_number("6222021234567890"), None);
        assert_eq!(parse_number("123456789012"), None);
        assert_eq!(parse_number("12,34"), None);
        assert_eq!(parse_number("收入"), None);
        assert_eq!(parse_number("1.2.3"), None);
        assert_eq!(parse_number(""), None);
    }

    #[test]
    fn writes_merged_cells() {
        let cell = |t: &str, h: bool, rs: u32, cs: u32| TableCell { text: t.into(), header: h, row_span: rs, col_span: cs };
        let rows = vec![
            vec![cell("项目", true, 2, 1), cell("2025 年", true, 1, 2)],
            vec![cell("上半年", true, 1, 1), cell("下半年", true, 1, 1)],
            vec![cell("收入", false, 1, 1), cell("1,200.00", false, 1, 1), cell("(300)", false, 1, 1)],
        ];
        let path = std::env::temp_dir().join(format!("chenocr-xlsx-test-{}.xlsx", std::process::id()));
        write(&rows, &path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1000);
        let _ = std::fs::remove_file(&path);
    }
}
