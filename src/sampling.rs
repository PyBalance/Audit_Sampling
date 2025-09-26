use crate::config::TransactionType;
use crate::journal::{find_acct_code_col, find_credit_col, find_date_col, find_debit_col, find_direction_col, find_report_subject_col, find_signed_amount_col, find_voucher_line_col, parse_amount, parse_date_flex, JournalData, Record};
use anyhow::{bail, Context, Result};
use rand::seq::SliceRandom;
use rand::{rng, Rng};
use rust_xlsxwriter::{Workbook, Worksheet};
use std::collections::HashSet;
use std::path::Path;
use std::env;
use libc;

fn truncate_chars(s: &str, max_chars: usize) -> String { s.chars().take(max_chars).collect() }
fn truncate_to_bytes(s: &str, max_bytes: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let b = ch.len_utf8();
        if used + b > max_bytes { break; }
        out.push(ch);
        used += b;
    }
    out
}

fn sanitize_sheet_name(name: &str) -> String {
    let mut s = name.replace(['\\', '/', '*', '?', ':', '[', ']'], "");
    while s.starts_with('\'') { s.remove(0); }
    while s.ends_with('\'') { s.pop(); }
    let s = s.trim();
    if s.is_empty() { return "Sheet".to_string(); }
    // 优先按字符截断，再保证字节数≤31（兼容部分库内部按字节处理的场景）
    let mut t = truncate_chars(s, 31);
    if t.as_bytes().len() > 31 { t = truncate_to_bytes(&t, 31); }
    if t.is_empty() { "Sheet".to_string() } else { t }
}

fn unique_sheet_name(base: &str, used: &mut std::collections::HashSet<String>) -> String {
    const MAX_CHARS: usize = 31;
    let mut candidate = sanitize_sheet_name(base);
    candidate = truncate_chars(&candidate, MAX_CHARS);
    if !used.contains(&candidate) {
        used.insert(candidate.clone());
        return candidate;
    }
    // Append numeric suffixes: " (2)", " (3)" ... while keeping ≤31 chars
    let mut idx: u32 = 2;
    loop {
        let suffix = format!(" ({idx})");
        let suffix_len_chars = suffix.chars().count();
        let max_base_chars = MAX_CHARS.saturating_sub(suffix_len_chars);
        let base0 = truncate_chars(&sanitize_sheet_name(base), max_base_chars);
        // 同样保证字节≤31
        let max_base_bytes = 31usize.saturating_sub(suffix.as_bytes().len());
        let base_trimmed = truncate_to_bytes(&base0, max_base_bytes);
        let cand = format!("{base_trimmed}{suffix}");
        if !used.contains(&cand) {
            used.insert(cand.clone());
            return cand;
        }
        idx += 1;
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedRule {
    pub population_name: String,
    pub account_codes: Option<Vec<String>>, // None/empty => 不过滤
    pub transaction_type: TransactionType,
    pub value_column: Option<String>, // None => 按借/贷列
}

fn effective_amount_for_rule(
    r: &Record,
    value_col: Option<&str>,
    debit_col: Option<&str>,
    credit_col: Option<&str>,
    direction: &TransactionType,
) -> f64 {
    if let Some(col) = value_col {
        return r
            .values
            .get(col)
            .map(|s| parse_amount(s))
            .unwrap_or(0.0);
    }
    match direction {
        TransactionType::Debit => debit_col
            .and_then(|c| r.values.get(c))
            .map(|s| parse_amount(s))
            .unwrap_or(0.0),
        TransactionType::Credit => credit_col
            .and_then(|c| r.values.get(c))
            .map(|s| parse_amount(s))
            .unwrap_or(0.0),
    }
}

pub fn build_population(
    data: &JournalData,
    period: (chrono::NaiveDate, chrono::NaiveDate),
    account_name: &str,
    rule: &ResolvedRule,
) -> Vec<Record> {
    let (start, end) = period;
    let date_col = find_date_col(&data.headers);
    let acct_col = find_acct_code_col(&data.headers);
    let debit_col = find_debit_col(&data.headers);
    let credit_col = find_credit_col(&data.headers);
    let dir_col = find_direction_col(&data.headers);
    let subject_col = find_report_subject_col(&data.headers);
    let signed_col = find_signed_amount_col(&data.headers);
    if env::var("AS_DEBUG").is_ok() {
        eprintln!(
            "[debug] headers: date={:?}, acct={:?}, debit={:?}, credit={:?}, dir={:?}",
            date_col, acct_col, debit_col, credit_col, dir_col
        );
    }

    let codes: HashSet<String> = rule
        .account_codes
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut out = Vec::new();
    let mut dbg_total = 0usize;
    let mut dbg_in_period = 0usize;
    let mut dbg_code_match = 0usize;
    let mut dbg_debit = 0usize;
    let mut dbg_credit = 0usize;
    let mut dbg_printed = 0usize;
    'rows: for r in &data.rows {
        // Period filter
        dbg_total += 1;
        if let Some(dc) = &date_col {
            match r.values.get(dc).and_then(|v| parse_date_flex(v)) {
                Some(d) => {
                    if d < start || d > end { continue 'rows; } else { dbg_in_period += 1; }
                }
                None => {
                    // 日期列存在但无法解析，按“无效日期”处理：跳过该行
                    continue 'rows;
                }
            }
        }
        // 限定“报表科目/科目名称”等（若存在该列）
        if let Some(sc) = &subject_col {
            if let Some(v) = r.values.get(sc) {
                if v.trim() != account_name { continue 'rows; }
            }
        }
        // Account code filter (prefix match allowed). None/empty => 不过滤
        if !codes.is_empty() {
            if let Some(ac) = &acct_col {
                if let Some(code) = r.values.get(ac) {
                    let c = code.trim();
                    if !codes.iter().any(|cfg| c.starts_with(cfg)) { continue 'rows; } else { dbg_code_match += 1; }
                }
            }
        }

        // Direction filter
        let mut is_debit = None;
        if let Some(dc) = &dir_col {
            if let Some(v) = r.values.get(dc) {
                let v = v.trim();
                if v.contains('借') || v.eq_ignore_ascii_case("debit") { is_debit = Some(true); }
                if v.contains('贷') || v.eq_ignore_ascii_case("credit") { is_debit = Some(false); }
            }
        }
        if is_debit.is_none() {
            let d_amt = debit_col.as_ref().and_then(|c| r.values.get(c)).map(|s| parse_amount(s)).unwrap_or(0.0);
            let c_amt = credit_col.as_ref().and_then(|c| r.values.get(c)).map(|s| parse_amount(s)).unwrap_or(0.0);
            if d_amt > 0.0 { is_debit = Some(true); }
            else if c_amt > 0.0 { is_debit = Some(false); }
            else if d_amt < 0.0 { is_debit = Some(true); }
            else if c_amt < 0.0 { is_debit = Some(false); }
            else if let Some(sc) = &signed_col {
                let s_amt = r.values.get(sc).map(|s| parse_amount(s)).unwrap_or(0.0);
                if s_amt > 0.0 { is_debit = Some(true); } else if s_amt < 0.0 { is_debit = Some(false); }
            }
            if env::var("AS_DEBUG").is_ok() && dbg_printed < 5 {
                let raw_d = debit_col.as_ref().and_then(|c| r.values.get(c)).cloned();
                let raw_c = credit_col.as_ref().and_then(|c| r.values.get(c)).cloned();
                eprintln!(
                    "[debug] sample row: code={:?}, date={:?}, raw_d={:?}, raw_c={:?}, d_amt={}, c_amt={}",
                    acct_col.as_ref().and_then(|c| r.values.get(c)),
                    date_col.as_ref().and_then(|c| r.values.get(c)),
                    raw_d,
                    raw_c,
                    d_amt, c_amt
                );
                dbg_printed += 1;
            }
        }
        match (rule.transaction_type.clone(), is_debit) {
            (TransactionType::Debit, Some(true)) => { dbg_debit += 1; }
            (TransactionType::Credit, Some(false)) => { dbg_credit += 1; }
            // Unknown direction: conservatively skip
            _ => continue 'rows,
        }

        let eff = effective_amount_for_rule(
            r,
            rule.value_column.as_deref(),
            debit_col.as_deref(),
            credit_col.as_deref(),
            &rule.transaction_type,
        );
        if eff <= 0.0 {
            continue 'rows;
        }

        out.push(r.clone());
    }
    if env::var("AS_DEBUG").is_ok() {
        eprintln!(
            "[debug] {}: total={}, in_period={}, code_match={}, debit={}, credit={} -> selected={}",
            rule.population_name,
            dbg_total, dbg_in_period, dbg_code_match, dbg_debit, dbg_credit, out.len()
        );
    }
    out
}

fn amounts_from_population(
    population: &[Record],
    value_col: Option<&str>,
    debit_col: Option<&str>,
    credit_col: Option<&str>,
    direction: &TransactionType,
) -> Vec<f64> {
    population
        .iter()
        .map(|r| effective_amount_for_rule(r, value_col, debit_col, credit_col, direction))
        .collect()
}

fn pps_systematic_indices(amounts: &[f64], n: usize) -> Vec<usize> {
    if n == 0 || amounts.is_empty() { return Vec::new(); }
    let total: f64 = amounts.iter().sum();
    if total <= 0.0 { return Vec::new(); }
    let interval = total / n as f64;
    let mut rng = rng();
    let start: f64 = rng.random_range(0.0..interval);
    let mut thresholds: Vec<f64> = (0..n).map(|i| start + i as f64 * interval).collect();
    thresholds.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let mut indices = Vec::new();
    let mut cum = 0.0_f64;
    let mut t_idx = 0;
    for (i, &v) in amounts.iter().enumerate() {
        let prev = cum;
        cum += v.max(0.0);
        while t_idx < thresholds.len() && thresholds[t_idx] <= cum {
            if thresholds[t_idx] > prev {
                indices.push(i);
            }
            t_idx += 1;
        }
        if t_idx >= thresholds.len() { break; }
    }
    indices
}

pub fn perform_random_sampling_with_rules(population: Vec<Record>, size: usize) -> Vec<Record> {
    if size >= population.len() { return population; }
    let mut idxs: Vec<usize> = (0..population.len()).collect();
    idxs.shuffle(&mut rng());
    idxs.truncate(size);
    idxs.sort_unstable();
    idxs.into_iter().map(|i| population[i].clone()).collect()
}

fn with_silenced_stderr<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    unsafe {
        let stderr_fd = libc::STDERR_FILENO;
        let saved = libc::dup(stderr_fd);
        let devnull_c = b"/dev/null\0";
        let dn = libc::open(devnull_c.as_ptr() as *const i8, libc::O_WRONLY);
        if dn >= 0 {
            libc::dup2(dn, stderr_fd);
            libc::close(dn);
        }
        let res = f();
        if saved >= 0 {
            libc::dup2(saved, stderr_fd);
            libc::close(saved);
        }
        res
    }
}

pub fn perform_mus_sampling_with_rules(
    population: Vec<Record>,
    rule: &ResolvedRule,
    tolerable_error: f64,
    expected_error: f64,
    confidence: f64,
    verbose: bool,
) -> Result<Vec<Record>> {
    // Amounts (with fallback to debit/credit columns if value_column not present)
    let header_probe: Vec<String> = population
        .get(0)
        .map(|r| r.values.keys().cloned().collect())
        .unwrap_or_else(|| Vec::new());
    let debit_col = crate::journal::find_debit_col(&header_probe);
    let credit_col = crate::journal::find_credit_col(&header_probe);
    let amounts = amounts_from_population(&population, rule.value_column.as_deref(), debit_col.as_deref(), credit_col.as_deref(), &rule.transaction_type);
    let total: f64 = amounts.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        bail!("总体金额为空或非正，已跳过（可能被负数或零值剔除后为空）");
    }
    if verbose {
        let ratio = if tolerable_error > 0.0 { total / tolerable_error } else { f64::INFINITY };
        eprintln!(
            "[MUS] population='{}' BV={:.2} TE={:.2} EE={:.2} Conf={:.2} BV/TE={:.2}",
            rule.population_name,
            total,
            tolerable_error,
            expected_error,
            confidence,
            ratio
        );
    }

    // Use library planning to derive n
    use audit_sampling::{mus_planning, PlanningOptions};
    let opts = PlanningOptions {
        col_name_book_values: rule.value_column.clone().unwrap_or_else(|| "book.value".to_string()),
        confidence_level: confidence,
        tolerable_error,
        expected_error,
        ..Default::default()
    };
    let plan = if verbose {
        mus_planning(&amounts, opts)
    } else {
        with_silenced_stderr(|| mus_planning(&amounts, opts))
    }.context("MUS 规划失败")?;
    let n = plan.n;
    if n == 0 { return Ok(Vec::new()); }

    // Select by PPS systematic sampling using computed n
    let idxs = pps_systematic_indices(&amounts, n);
    if idxs.is_empty() { return Ok(Vec::new()); }

    // 去重处理：对于被多个抽样阈值命中的大额记录，只保留首次命中的索引
    use std::collections::HashSet;
    let mut seen: HashSet<usize> = HashSet::with_capacity(idxs.len());
    let mut unique_idxs: Vec<usize> = Vec::with_capacity(idxs.len());
    for i in idxs {
        if seen.insert(i) {
            unique_idxs.push(i);
        }
    }
    if verbose && unique_idxs.len() < n {
        eprintln!(
            "[MUS] 去重后样本: 计划 n={} -> 实际 {}（存在大额记录被多次命中，已合并去重）",
            n,
            unique_idxs.len()
        );
    }

    let mut out = Vec::with_capacity(unique_idxs.len());
    for i in unique_idxs { out.push(population[i].clone()); }
    Ok(out)
}

pub struct SummaryCtx {
    pub method: String,
    pub start: String,
    pub end: String,
    pub note: String,
}

pub fn write_results_to_excel(
    results: &[(String, Vec<Record>, usize)],
    summary_rows: &[(String, usize, usize)],
    output: &Path,
    display_headers: &[String],
    summary_ctx: &SummaryCtx,
) -> Result<()> {
    let mut wb = Workbook::new();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (sheet_name, rows, _pop_len) in results {
        let sname = unique_sheet_name(sheet_name, &mut used);
        let mut ws = wb.add_worksheet().set_name(&sname)?;
        write_sheet(&mut ws, rows, display_headers)?;
    }
    // Summary sheet (always add)
    let sname = unique_sheet_name("抽样统计", &mut used);
    let mut ws = wb.add_worksheet().set_name(&sname)?;
    write_summary(&mut ws, summary_rows, summary_ctx)?;
    wb.save(output).with_context(|| format!("保存 Excel 失败: {}", output.display()))?;
    Ok(())
}

fn write_sheet(ws: &mut Worksheet, rows: &[Record], display_headers: &[String]) -> Result<()> {
    // Use the selected headers order
    let headers: Vec<String> = display_headers.to_vec();

    // Optional: sort rows by 凭证行号（若存在该列）
    use std::cmp::Ordering;
    let voucher_col = find_voucher_line_col(&headers).or_else(|| Some("凭证行号".to_string()));
    let mut rows_sorted: Vec<Record> = rows.to_vec();
    if let Some(vc) = &voucher_col {
        fn parse_int_like(s: &str) -> Option<i64> {
            let t = s.trim().replace(",", "");
            if t.is_empty() { return None; }
            if t.chars().all(|ch| ch.is_ascii_digit() || ch == '-') {
                return t.parse::<i64>().ok();
            }
            None
        }
        rows_sorted.sort_by(|a, b| {
            let av = a.values.get(vc).map(|s| s.as_str()).unwrap_or("");
            let bv = b.values.get(vc).map(|s| s.as_str()).unwrap_or("");
            match (parse_int_like(av), parse_int_like(bv)) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => av.cmp(bv),
            }
        });
    }

    // Write header row
    for (c, h) in headers.iter().enumerate() { ws.write_string(0, c as u16, h)?; }
    // Rows
    for (i, r) in rows_sorted.iter().enumerate() {
        for (c, h) in headers.iter().enumerate() {
            let v = r.values.get(h).cloned().unwrap_or_default();
            // Excel 单元格字符串上限 32767 个字符；再保证字节安全
            let mut safe = truncate_chars(&v, 32767);
            if safe.as_bytes().len() > 32767 { safe = truncate_to_bytes(&safe, 32767); }
            ws.write_string((i + 1) as u32, c as u16, &safe)?;
        }
    }
    Ok(())
}

fn write_summary(ws: &mut Worksheet, rows: &[(String, usize, usize)], ctx: &SummaryCtx) -> Result<()> {
    let headers = vec![
        "总体名称".to_string(),
        "总体条数".to_string(),
        "样本条数".to_string(),
        "方法".to_string(),
        "开始日期".to_string(),
        "结束日期".to_string(),
        "参数".to_string(),
    ];
    for (c, h) in headers.iter().enumerate() { ws.write_string(0, c as u16, h)?; }
    for (i, (name, pop, sam)) in rows.iter().enumerate() {
        ws.write_string((i + 1) as u32, 0, name)?;
        ws.write_string((i + 1) as u32, 1, &pop.to_string())?;
        ws.write_string((i + 1) as u32, 2, &sam.to_string())?;
        ws.write_string((i + 1) as u32, 3, &ctx.method)?;
        ws.write_string((i + 1) as u32, 4, &ctx.start)?;
        ws.write_string((i + 1) as u32, 5, &ctx.end)?;
        ws.write_string((i + 1) as u32, 6, &ctx.note)?;
    }
    Ok(())
}
