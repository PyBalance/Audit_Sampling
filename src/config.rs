use anyhow::{Context, Result};
use serde::Deserialize;
use std::{collections::HashMap, fs, path::Path};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionType {
    Debit,
    Credit,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub population_name: Option<String>,
    #[serde(default)]
    pub account_codes: Option<Vec<String>>, // None/empty => 不过滤科目编码
    #[serde(default)]
    pub transaction_type: Option<TransactionType>, // None => 借/贷各一条
    #[serde(default)]
    pub value_column: Option<String>, // None => 不使用自定义金额列，仅按借/贷列
}

pub type ConfigMap = HashMap<String, Vec<Rule>>;

pub fn load_config(path: &Path) -> Result<ConfigMap> {
    let text = fs::read_to_string(path).with_context(|| format!("读取配置失败: {}", path.display()))?;
    let cfg: ConfigMap = serde_json::from_str(&text).context("配置 JSON 解析失败")?;
    Ok(cfg)
}
