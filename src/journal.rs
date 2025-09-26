use anyhow::{Context, Result};
use calamine::{open_workbook_auto, Reader};
use chrono::NaiveDate;
use csv::ReaderBuilder;
use std::{collections::HashMap, fs::File, path::Path};

#[derive(Debug, Clone)]
pub struct Record {
    pub values: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct JournalData {
    pub headers: Vec<String>,
    pub rows: Vec<Record>,
}

fn normalize_header(h: &str) -> String { h.trim().to_string() }

fn xlsx_to_string<T: calamine::DataType>(cell: &T) -> String {
    // Prefer semantic date rendering only if the cell is marked as datetime
    // or contains ISO8601 datetime text. Avoid misinterpreting numeric amounts as dates.
    if cell.is_datetime() || cell.is_datetime_iso() {
        if let Some(dt) = cell.as_date() { return dt.format("%Y-%m-%d").to_string(); }
    }
    if let Some(s) = cell.as_string() { return s; }
    if let Some(i) = cell.as_i64() { return i.to_string(); }
    if let Some(f) = cell.as_f64() {
        if (f.fract()).abs() < f64::EPSILON { return format!("{}", f as i64); }
        return f.to_string();
    }
    if let Some(b) = cell.get_bool() { return b.to_string(); }
    String::new()
}

fn load_excel(path: &Path) -> Result<JournalData> {
    let mut wb = open_workbook_auto(path).with_context(|| format!("打开 Excel 失败: {}", path.display()))?;
    // Pick first visible sheet
    let sheet_names = wb.sheet_names().to_owned();
    let name = sheet_names
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Excel 无可读工作表"))?;
    let range = wb.worksheet_range(&name)?;

    let mut rows_iter = range.rows();
    let headers_row = rows_iter.next().ok_or_else(|| anyhow::anyhow!("缺少表头行"))?;
    let headers: Vec<String> = headers_row
        .iter()
        .map(xlsx_to_string)
        .map(|s| normalize_header(&s))
        .collect();

    let mut rows: Vec<Record> = Vec::new();
    for r in rows_iter {
        let mut map: HashMap<String, String> = HashMap::new();
        for (i, cell) in r.iter().enumerate() {
            if let Some(h) = headers.get(i) {
                map.insert(h.clone(), xlsx_to_string(cell));
            }
        }
        // Skip completely empty rows
        if map.values().all(|v| v.trim().is_empty()) { continue; }
        rows.push(Record { values: map });
    }
    Ok(JournalData { headers, rows })
}

fn load_csv(path: &Path) -> Result<JournalData> {
    let file = File::open(path).with_context(|| format!("打开 CSV 失败: {}", path.display()))?;
    let mut rdr = ReaderBuilder::new().flexible(true).has_headers(true).from_reader(file);
    let headers = rdr.headers()?.iter().map(|s| normalize_header(s)).collect::<Vec<_>>();
    let mut rows: Vec<Record> = Vec::new();
    for rec in rdr.records() {
        let rec = rec?;
        let mut map: HashMap<String, String> = HashMap::new();
        for (i, v) in rec.iter().enumerate() {
            if let Some(h) = headers.get(i) { map.insert(h.clone(), v.trim().to_string()); }
        }
        if map.values().all(|v| v.trim().is_empty()) { continue; }
        rows.push(Record { values: map });
    }
    Ok(JournalData { headers, rows })
}

pub fn load_journal(path: &Path) -> Result<JournalData> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "xlsx" | "xlsm" | "xls" => load_excel(path),
        "csv" => load_csv(path),
        _ => {
            if path.is_file() {
                // Try Excel first
                load_excel(path).or_else(|_| load_csv(path))
            } else {
                Err(anyhow::anyhow!("不支持的文件类型: {}", path.display()))
            }
        }
    }
}

// Helpers to detect column names present in Chinese/English ledgers
pub fn find_date_col(headers: &[String]) -> Option<String> {
    const CANDS: &[&str] = &["date", "日期", "记账日期", "凭证日期"]; 
    headers.iter().find(|h| {
        let l = h.to_lowercase();
        CANDS.iter().any(|c| l.contains(&c.to_lowercase()))
    }).cloned()
}

pub fn find_acct_code_col(headers: &[String]) -> Option<String> {
    const CANDS: &[&str] = &["account_code", "科目编码", "科目代码", "会计科目代码", "会计科目编号", "科目号"]; 
    headers.iter().find(|h| {
        let l = h.to_lowercase();
        CANDS.iter().any(|c| l.contains(&c.to_lowercase()))
    }).cloned()
}

pub fn find_debit_col(headers: &[String]) -> Option<String> {
    const CANDS: &[&str] = &["debit", "debit_amount", "借方", "借方发生额", "借方金额", "借方发生"]; 
    headers.iter().find(|h| {
        let l = h.to_lowercase();
        CANDS.iter().any(|c| l.contains(&c.to_lowercase()))
    }).cloned()
}

pub fn find_credit_col(headers: &[String]) -> Option<String> {
    const CANDS: &[&str] = &["credit", "credit_amount", "贷方", "贷方发生额", "贷方金额", "贷方发生"]; 
    headers.iter().find(|h| {
        let l = h.to_lowercase();
        CANDS.iter().any(|c| l.contains(&c.to_lowercase()))
    }).cloned()
}

pub fn find_direction_col(headers: &[String]) -> Option<String> {
    const CANDS: &[&str] = &["方向", "借贷方向", "direction"]; 
    headers.iter().find(|h| {
        let l = h.to_lowercase();
        CANDS.iter().any(|c| l.contains(&c.to_lowercase()))
    }).cloned()
}

pub fn find_report_subject_col(headers: &[String]) -> Option<String> {
    // 严格仅支持“报表科目”列，不做别名/模糊匹配
    headers
        .iter()
        .find(|h| h.trim() == "报表科目")
        .cloned()
}

pub fn find_signed_amount_col(headers: &[String]) -> Option<String> {
    // 一些系统把借/贷净额放在“借正贷负”列（>0 借，<0 贷）
    const CANDS: &[&str] = &["借正贷负", "借贷净额", "signed", "net"]; 
    headers.iter().find(|h| {
        let l = h.to_lowercase();
        CANDS.iter().any(|c| l.contains(&c.to_lowercase()))
    }).cloned()
}

pub fn find_voucher_line_col(headers: &[String]) -> Option<String> {
    headers.iter().find(|h| h.trim() == "凭证行号").cloned()
}

pub fn parse_amount(s: &str) -> f64 {
    let mut t = s.trim().replace(",", "");
    let has_paren = (t.starts_with('(') && t.ends_with(')'))
        || (t.starts_with('（') && t.ends_with('）'));
    if has_paren {
        t = t
            .trim_matches(|c: char| c == '(' || c == ')' || c == '（' || c == '）')
            .to_string();
    }
    t = t
        .trim_start_matches(|c: char| c == '¥' || c == '￥' || c == '$')
        .to_string();
    let v = t.parse::<f64>().unwrap_or(0.0);
    if has_paren { -v } else { v }
}

pub fn parse_date_flex(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    if s.is_empty() { return None; }
    const FORMATS: &[&str] = &[
        "%Y-%m-%d", "%Y/%m/%d", "%Y.%m.%d", "%Y%m%d",
        "%Y-%m-%d %H:%M:%S", "%Y/%m/%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M:%S%.f",
        "%Y年%m月%d日",
    ];
    for f in FORMATS {
        if let Ok(d) = NaiveDate::parse_from_str(s, f) { return Some(d); }
    }
    None
}
