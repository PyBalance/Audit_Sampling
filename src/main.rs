mod config;
mod journal;
mod sampling;

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use clap::{Parser, ValueEnum};
use config::ConfigMap;
use journal::{load_journal, JournalData};
use sampling::{build_population, perform_mus_sampling_with_rules, perform_random_sampling_with_rules, ResolvedRule};
use std::path::PathBuf;

#[derive(Debug, Clone, ValueEnum)]
enum Method {
    Mus,
    Random,
}

#[derive(Parser, Debug)]
#[command(
    name = "audit-sampler",
    version,
    about = "命令行审计抽样工具（支持 MUS 与随机抽样）",
    long_about = "\
一个用于从会计序时账进行审计抽样的命令行工具：\n\
- 两种方法：MUS（货币单元抽样）与随机抽样；\n\
- 可按期间、报表科目与方向（借/贷）构建总体；\n\
- 配置文件可选，所有字段均可选；不提供配置时将严格使用‘报表科目’列枚举科目；\n\
- 输出一个 Excel，每个总体一个工作表（表名按规则或自动命名）；\n\
- MUS 参数（低风险默认最低样本）：--confidence 0.90（90%置信，允许较高 RIA=10%，常见于低风险项目）；--risk-factor 0.0（零预期错报，显著降低样本；0.25 为典型 25% 预期错报）。"
)] 
struct Args {
    /// 序时账文件路径（Excel .xlsx/.xls 或 CSV）
    /// 注意：会自动识别中文/英文常见列，如 日期/科目编码/借方金额/贷方金额
    #[arg(long, value_name = "FILE")] 
    journal: PathBuf,

    /// 期间开始日期，格式：YYYY-MM-DD（含边界）
    #[arg(long, value_name = "YYYY-MM-DD")] 
    start: String,

    /// 期间结束日期，格式：YYYY-MM-DD（含边界）
    #[arg(long, value_name = "YYYY-MM-DD")] 
    end: String,

    /// 抽样方法：mus 或 random
    #[arg(long, value_enum)]
    method: Method,

    /// 重要性水平（MUS）：如未给出 --tolerable-misstatement，则以此作为 TE；
    /// 仅 MUS 需要二者之一（materiality 或 tolerable-misstatement）。
    #[arg(long, value_name = "AMOUNT")]
    materiality: Option<f64>,

    /// 可容忍错报 TE（MUS）：优先级高于 materiality；
    /// 若 TE ≥ 总体金额，则样本量 n=0（默认 n.min=0 不强制抽样）。
    #[arg(long, value_name = "AMOUNT")]
    tolerable_misstatement: Option<f64>,

    /// 风险系数（MUS）：预期错报 EE = TE × 风险系数；默认 0.0（零预期，最低样本）；0.25 为典型值（25% 预期错报）。
    #[arg(long, default_value_t = 0.0)]
    risk_factor: f64,

    /// 置信水平（MUS）：默认 0.90（90%，允许 RIA=10%，样本更少）；0.95 适用于高可靠性要求。
    #[arg(long, default_value_t = 0.90)]
    confidence: f64,

    /// 抽样数量（随机）：仅 random 方法需要；size>0
    #[arg(long, value_name = "N")] 
    size: Option<usize>,

    /// 需要抽样的报表科目名列表：可省略或指定为 all（默认 all）。
    /// 多个名称用空格分隔：--accounts A B C
    /// 无配置文件时，严格使用序时账“报表科目”列作为名称来源（无该列则报错）。
    #[arg(long, num_args = 0.., value_name = "NAME")] 
    accounts: Vec<String>,

    /// JSON 配置文件路径（可选）：不提供时按自动识别模式运行；
    /// 配置中各字段均可选：缺失时将按默认规则（借/贷拆分；不按编码过滤；自动命名工作表；金额取借/贷列）。
    #[arg(long, value_name = "FILE")] 
    config: Option<PathBuf>,

    /// 输出 Excel 路径：所有总体写入同一文件，不存在则创建。
    #[arg(long, value_name = "FILE")] 
    output: PathBuf,

    /// 选择输出列：
    /// - 默认：不传或传入以+开头的列名时，在默认列基础上追加（默认列：凭证唯一号, 凭证行号, 日期, 摘要, 科目编码, 科目全称, 借方金额, 贷方金额）；
    /// - 显式列表：传入不带+的列名列表时，仅输出这些列；
    /// - all：输出所有列，按输入表头顺序。
    #[arg(long, num_args = 0.., value_name = "NAME")] 
    columns: Vec<String>,

    /// 输出详细日志（默认关闭）。不加 --verbose 时，仅在完成时打印输出文件路径。
    #[arg(long, default_value_t = false)]
    verbose: bool,
}

fn parse_date(s: &str) -> Result<NaiveDate> {
    const FORMATS: &[&str] = &[
        "%Y-%m-%d", "%Y/%m/%d", "%Y.%m.%d", "%Y%m%d", "%Y-%m-%d %H:%M:%S",
        "%Y/%m/%d %H:%M:%S",
    ];
    for f in FORMATS {
        if let Ok(d) = NaiveDate::parse_from_str(s, f) { return Ok(d); }
    }
    bail!("无法解析日期: {s}");
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Validate method-specific args
    match args.method {
        Method::Mus => {
            if args.materiality.is_none() && args.tolerable_misstatement.is_none() {
                bail!("MUS 方法需要提供 --materiality 或 --tolerable-misstatement 之一");
            }
            if args.confidence <= 0.0 || args.confidence >= 1.0 {
                bail!("MUS 方法要求 --confidence 介于 0 与 1 之间（例如 0.90 或 0.95）");
            }
        }
        Method::Random => {
            if args.size.unwrap_or(0) == 0 { bail!("随机抽样需要提供 --size > 0"); }
        }
    }

    let start = parse_date(&args.start).context("解析开始日期失败")?;
    let end = parse_date(&args.end).context("解析结束日期失败")?;
    if end < start { bail!("结束日期早于开始日期"); }

    // Load journal（无论是否有配置，都要求存在“报表科目”列）
    let data: JournalData = load_journal(&args.journal).with_context(|| format!("读取序时账失败: {}", args.journal.display()))?;
    let period = (start, end);
    let subject_col = journal::find_report_subject_col(&data.headers)
        .ok_or_else(|| anyhow::anyhow!("未找到‘报表科目’列。请在序时账中提供该列，或调整导出字段。"))?;

    // 计算最终输出列（保持输入表头顺序）
    let selected_headers: Vec<String> = {
        fn split_tokens(items: &[String]) -> Vec<String> {
            let mut out = Vec::new();
            for it in items {
                if it.contains(',') {
                    out.extend(it.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
                } else {
                    out.push(it.trim().to_string());
                }
            }
            out
        }
        let defaults = vec![
            "凭证唯一号", "凭证行号", "日期", "摘要", "科目编码", "科目全称", "借方金额", "贷方金额",
        ];
        let tokens = split_tokens(&args.columns);
        let is_all = tokens.iter().any(|t| t.eq_ignore_ascii_case("all"));
        let has_explicit = tokens.iter().any(|t| !t.starts_with('+') && !t.eq_ignore_ascii_case("all"));
        let mut want: Vec<String> = if is_all {
            data.headers.clone()
        } else if has_explicit {
            tokens
                .into_iter()
                .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("all"))
                .map(|t| t.trim_start_matches('+').to_string())
                .collect()
        } else {
            let mut set: std::collections::BTreeSet<String> = defaults.iter().map(|s| (*s).to_string()).collect();
            for t in tokens { if t.starts_with('+') { set.insert(t.trim_start_matches('+').to_string()); } }
            set.into_iter().collect()
        };
        // 保持输入表头顺序
        let set: std::collections::HashSet<String> = want.drain(..).collect();
        data.headers.iter().filter(|h| set.contains(*h)).cloned().collect()
    };

    // Load config (可选)
    let cfg_opt: Option<ConfigMap> = match &args.config {
        Some(p) => Some(config::load_config(p).context("读取配置文件失败")?),
        None => None,
    };

    // 解析目标 accounts
    let target_accounts: Vec<String> = match &cfg_opt {
        Some(cfg) => {
            if args.accounts.is_empty() || args.accounts.iter().any(|a| a.eq_ignore_ascii_case("all")) {
                cfg.keys().cloned().collect()
            } else {
                args.accounts.clone()
            }
        }
        None => {
            // 无配置：严格使用“报表科目”列做分组
            let mut set = std::collections::BTreeSet::new();
            for r in &data.rows {
                if let Some(v) = r.values.get(&subject_col) { if !v.trim().is_empty() { set.insert(v.trim().to_string()); } }
            }
            let mut all: Vec<String> = set.into_iter().collect();
            if args.accounts.is_empty() || args.accounts.iter().any(|a| a.eq_ignore_ascii_case("all")) {
                all
            } else {
                let req: std::collections::HashSet<_> = args.accounts.iter().collect();
                all.into_iter().filter(|s| req.contains(s)).collect()
            }
        }
    };

    // Execute per account/rule and collect results
    let mut results_nonempty: Vec<(String, Vec<journal::Record>, usize)> = Vec::new();
    let mut summary_rows: Vec<(String, usize, usize)> = Vec::new();

    for account in target_accounts {
        // 组装规则（配置中的字段均可选；若未配置该 account，则按默认：借/贷各一条规则）
        let resolved_rules: Vec<ResolvedRule> = match &cfg_opt {
            Some(cfg) => {
                if let Some(rules) = cfg.get(&account) {
                    let mut out = Vec::new();
                    for rule in rules {
                        let types: Vec<config::TransactionType> = match &rule.transaction_type {
                            Some(t) => vec![t.clone()],
                            None => vec![config::TransactionType::Credit, config::TransactionType::Debit],
                        };
                        for t in types {
                            let pname = match (&rule.population_name, &t) {
                                (Some(n), _) => n.clone(),
                                (None, config::TransactionType::Debit) => format!("{}_借方", account),
                                (None, config::TransactionType::Credit) => format!("{}_贷方", account),
                            };
                            let codes = rule.account_codes.clone().filter(|v| !v.is_empty());
                            let vcol = rule.value_column.clone();
                            out.push(ResolvedRule { population_name: pname, account_codes: codes, transaction_type: t.clone(), value_column: vcol });
                        }
                    }
                    if out.is_empty() {
                        vec![
                            ResolvedRule { population_name: format!("{}_贷方", account), account_codes: None, transaction_type: crate::config::TransactionType::Credit, value_column: None },
                            ResolvedRule { population_name: format!("{}_借方", account), account_codes: None, transaction_type: crate::config::TransactionType::Debit, value_column: None },
                        ]
                    } else { out }
                } else {
                    vec![
                        ResolvedRule { population_name: format!("{}_贷方", account), account_codes: None, transaction_type: crate::config::TransactionType::Credit, value_column: None },
                        ResolvedRule { population_name: format!("{}_借方", account), account_codes: None, transaction_type: crate::config::TransactionType::Debit, value_column: None },
                    ]
                }
            }
            None => vec![
                ResolvedRule { population_name: format!("{}_贷方", account), account_codes: None, transaction_type: crate::config::TransactionType::Credit, value_column: None },
                ResolvedRule { population_name: format!("{}_借方", account), account_codes: None, transaction_type: crate::config::TransactionType::Debit, value_column: None },
            ],
        };

        for rrule in resolved_rules {
            let population = build_population(&data, period, &account, &rrule);
            let population_len = population.len();
            if population_len == 0 {
                if args.verbose { eprintln!("警告: {} 的总体为空，已跳过。", rrule.population_name); }
                summary_rows.push((rrule.population_name.clone(), 0usize, 0usize));
                continue;
            }
            let sampled = match args.method {
                Method::Mus => {
                    let te = args.tolerable_misstatement.or(args.materiality).expect("validated");
                    let ee = te * args.risk_factor;
                    perform_mus_sampling_with_rules(population, &rrule, te, ee, args.confidence, args.verbose)
                        .with_context(|| format!("MUS 抽样失败: {}", rrule.population_name))?
                }
                Method::Random => {
                    perform_random_sampling_with_rules(population, args.size.unwrap())
                }
            };
            let sample_len = sampled.len();
            summary_rows.push((rrule.population_name.clone(), population_len, sample_len));
            if sample_len > 0 {
                results_nonempty.push((rrule.population_name.clone(), sampled, population_len));
            }
        }
    }

    // Write to Excel：仅写有样本的表，另附“抽样统计”工作表
    let method_str = match args.method { Method::Mus => "mus", Method::Random => "random" }.to_string();
    let note = match args.method {
        Method::Mus => {
            let te = args.tolerable_misstatement.or(args.materiality).unwrap_or(0.0);
            format!("TE={:.2}, risk={:.2}, conf={:.2}", te, args.risk_factor, args.confidence)
        }
        Method::Random => {
            format!("size={}", args.size.unwrap_or(0))
        }
    };
    let ctx = sampling::SummaryCtx { method: method_str, start: args.start.clone(), end: args.end.clone(), note };

    sampling::write_results_to_excel(&results_nonempty, &summary_rows, &args.output, &selected_headers, &ctx)
        .with_context(|| format!("写出结果失败: {}", args.output.display()))?;

    println!("{}", args.output.display());
    Ok(())
}
