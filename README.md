# MUS 审计抽样（Rust 版）使用说明（审计用户视角）

本仓库实现了 MUS（Monetary Unit Sampling，货币单元抽样）的两个核心步骤：
- 计划（Planning）：根据置信水平、可容忍错报等参数计算样本量 n 与抽样间隔；
- 提取（Extraction）：按固定间隔从总体中抽取样本，并识别“高值项目”（个别重大项目）。

本文聚焦“如何提取样本”，并简要说明必须的前置“计划”步骤。

## 工作流程总览
- 准备数据：每条记录至少包含一列“账面金额”（建议以最小货币单位记录，如“分”）。
- 计划（Planning）：设定置信水平、可容忍错报（TE）、预期错报（EE），计算样本量 n 和“高值阈值”。
- 提取（Extraction）：按固定间隔从非高值总体中抽样，同时所有高值项目（金额≥高值阈值）全部入样。
- 评价（Evaluation）：对样本进行审定、计算上限误差等（本仓库暂未实现评价函数，仅说明流程）。

## 数据准备（审计口径）
- 必需字段：账面金额列，默认列名为 `book.value`（Rust 接口允许自定义列名）。
- 金额单位：建议使用整数最小货币单位（如“分”），避免小数舍入影响。
- 负数与零：
  - 负值在规划时按 0 计入抽样基数（需要单独分析）。
  - 金额为 0 的记录不会被抽中（需要另行关注）。
- 缺失/无穷：存在 NA/Inf 会收到警告，这些记录不参与抽样。

## 关键参数解释（Planning）
- 置信水平 `confidence.level`：介于 (0,1)，常用 0.95。
- 可容忍错报 `tolerable.error`（TE）：整个总体层面的可容忍错报额。
- 预期错报 `expected.error`（EE）：预计总体中可能存在的错报额。
- 最小样本量 `n.min`：若算法计算 n 小于该值，则用该最小值。
- 百分比模式 `errors.as.pct`：若为 `true`，则 TE/EE 以“占比”传入（算法内部乘以总体金额）。
- 保守法 `conservative`：按 AICPA 兼容的 γ 分布因子取较大样本量，偏保守。
- 合并 `combined`：标记来自多分层合并的总体（对提取逻辑无直接影响）。

规划输出的两个重要结果：
- 样本量 `n`；
- 高值阈值 `High.value.threshold`：等于抽样间隔。账面金额 ≥ 阈值的记录被视为“高值项目”，全部入样。

## 如何提取样本（Extraction）
提取阶段需要“规划结果”作为输入，并可配置以下选项：
- `start.point`（起始点）：固定间隔抽样的起点，取 [0, 抽样间隔]。不指定时随机产生。
- `seed`（随机种子）：给定后可复现随机起点（便于重现样本）。
- `obey.n.as.min`：
  - `false`（默认）：抽样间隔使用“高值阈值”，可能导致实际样本数略小于 n（与多数商用统计软件一致）。
  - `true`：按“完美间隔”重新计算抽样间隔，保证实际样本数等于 n（在重新计算后，若产生新高值项目，会迭代直至稳定）。

提取结果包含：
- `high.values`：所有高值项目；
- `sample.population`：用于抽样的非高值总体，并给出累计金额（便于溯源命中区间）；
- `sampling.interval`：提取完成后用于评价的“重新评估抽样间隔”（非高值总额 ÷ 实际样本数）；
- `sample`：最终抽取的样本清单，每条含：
  - `book_value`：该记录的账面金额；
  - `MUS.hit`：命中的货币单元位置；
  - `cum_before`/`cum_after`：该记录在累计金额序列中的区间（便于核对）。

## 面向审计的操作步骤
1) 明确总体与账面金额列（建议整数最小货币单位）。
2) 选择参数：置信水平、TE、EE、是否保守法、最小样本量等；
3) 执行“计划”：得到样本量 n 与高值阈值；
4) 执行“提取”：
   - 若希望每次复现相同样本，设定 `seed`；
   - 若强制抽满 n，设定 `obey.n.as.min = true`；
   - 可指定 `start.point`（典型用于与 R/其他系统对齐）。
5) 保存样本清单：高值项目 + 抽样样本即为需审定的记录集合。

## 常见问题（FAQ）
- 为什么实际样本数比 n 少？
  - 默认使用“高值阈值=抽样间隔”的做法时，可能出现少量偏差；如需强制抽满 n，请将 `obey.n.as.min = true`。
- 什么是“高值项目”？
  - 账面金额 ≥ 高值阈值（即抽样间隔）的记录。其重要性足以直接全检，不参与间隔抽样。
- 能否复现完全相同的样本？
  - 可以。设定固定的 `seed` 与 `start.point`（若需要），即可保证可重复性。
- TE/EE 是否可用百分比？
  - 可以。将 `errors.as.pct = true`，并把 TE/EE 传入为比例（如 0.05 表示 5%）。
- 单位不是“分”可以吗？
  - 建议使用最小货币单位以避免小数舍入差异。若使用小数金额，请注意可能造成与 R 的边界取整差异。

## 与 R 版对齐（可选）
若需与 R 包 MUS 的结果一致：
- 在 R 中使用 `MUS.planning` 与 `MUS.extraction`，传入相同的 TE/EE/置信水平/n.min/保守法设置；
- 对于提取：使用相同的 `start.point` 与 `seed`；如需严格抽满 n，选择 `obey.n.as.min=TRUE`；
- 账面金额应使用一致的单位与取整方式（建议整数最小货币单位）。

## 开发者补充（如需自助运行）
本仓库为 Rust 库（非命令行工具）。可调用以下公开 API：
- 计划：`mus_planning(book_values, PlanningOptions) -> Plan`
- 提取：`mus_extraction(&Plan, ExtractionOptions) -> Extraction`

### 推荐默认值（便于开箱即用）
```rust
// Planning
let planning_defaults = PlanningOptions {
    confidence_level: 0.95,          // 常用置信水平
    // tolerable_error / expected_error 由项目决定
    n_min: 0,                        // 不强制最小样本量
    errors_as_pct: false,            // 默认金额口径
    conservative: false,             // 默认非保守法
    combined: false,                 // 默认非合并层
    col_name_book_values: "book_value".to_string(),
};

// Extraction
let extraction_defaults = ExtractionOptions {
    start_point: None,      // 随机起点
    seed: None,             // 如需复现，项目上显式设定
    obey_n_as_min: true,    // 强制抽满 n
    combined: false,
};
```
提示：若不传这些字段，库内部也有 `Default` 实现；此处为审计项目常见的“建议默认值”。

快速示例（Rust）：
```rust
use audit_sampling::{PlanningOptions, ExtractionOptions, mus_planning, mus_extraction};

// 1) 准备数据：500 条记录的账面金额（示例）
let data: Vec<f64> = (0..500).map(|i| ((i % 1000) + 1) as f64).collect();

// 2) 计划
let plan = mus_planning(&data, PlanningOptions {
    confidence_level: 0.95,
    tolerable_error: 100_000.0,
    expected_error: 20_000.0,
    n_min: 0,
    errors_as_pct: false,
    conservative: false,
    combined: false,
    col_name_book_values: "book.value".to_string(),
}).expect("planning");

// 3) 提取（可指定 seed 以复现）
let extract = mus_extraction(&plan, ExtractionOptions {
    start_point: Some(5.0),
    seed: Some(0),
    obey_n_as_min: true,
    combined: false,
}).expect("extraction");

println!("高值项目: {} 条", extract.high_values.len());
println!("抽样样本: {} 条", extract.sample.len());
```

运行测试（包含一个示例用例）：
```
cargo test
```

——以上内容旨在帮助审计人员理解 MUS 在“计划—提取”阶段的实际使用方法。若需在贵司的审计作业平台中落地执行，可将本库集成到内部工具或编写简单的 CLI 包装导入/导出 CSV。

## 命令行工具 audit-sampler（本仓库新增）

根据《命令行审计抽样工具 - 设计文档》实现了一个 CLI：从序时账（Excel/CSV）按“报表科目”构建总体并进行 MUS 或随机抽样，输出到一个 Excel。

安装/构建：
- 在仓库根目录执行 `cargo build --release`，可执行文件位于 `target/release/audit-sampler`。

基本用法：
```
audit-sampler \
  --journal "./test-data/删除模式序时账.xlsx" \
  --start "2024-01-01" \
  --end   "2024-12-31" \
  --method mus \
  --materiality 1000000 \
  --tolerable-misstatement 500000 \
  --accounts "主营业务收入" "存货" \
  --config ./config/config.json \
  --output ./output/mus_sampling_result.xlsx

# 随机抽样（每科目 50 个样本）
audit-sampler \
  --journal "./test-data/删除模式序时账.xlsx" \
  --start "2024-01-01" \
  --end   "2024-12-31" \
  --method random \
  --size 50 \
  --accounts all \
  --config ./config/config.json \
  --output ./output/random_sampling_result.xlsx
```

说明：
- `--accounts` 省略或为 `all` 时处理配置中的所有科目；
- MUS：若仅提供 `--materiality`，工具将默认将其作为 `--tolerable-misstatement`；`--risk-factor`（默认 0.25）作为“预期错报 = TE × 风险系数”；
- Excel 解析使用 `calamine`，识别常见列：日期（如“日期/凭证日期”）、科目编码（如“科目编码/会计科目代码”）、借贷金额（如“借方/借方发生额、贷方/贷方发生额”）。总体名称严格使用“报表科目”列（无该列则报错）。
- 配置示例：`config/config.json`（与设计文档一致）。

## 配置文件使用说明（表格映射与处理流程）

本工具通过一个 JSON 配置文件（如 `config/config.json`）把“报表科目/抽样总体”的业务定义，映射到序时账中的筛选规则。运行时，CLI 会按此配置逐个“总体”构建数据（总体单位=报表科目+方向），再根据选择的方法（MUS/随机）产生样本，并把每个总体写入一个独立工作表。

### 配置文件结构

```json
{
  "报表科目名称": [
    {
      "population_name": "在 Excel 中显示的工作表名",
      "account_codes": ["科目编码前缀", "..."],
      "transaction_type": "debit|credit",
      "value_column": "金额列名（可留用'金额'或按需指定）"
    }
    // 可以为同一报表科目配置多条规则（如按借/贷分两个总体）
  ]
}
```

字段解释（现在全部“可选”）：
- Key（最外层键）：“报表科目名称”。用于指定一个业务口径的总体集合；是否处理该名称由 `--accounts` 控制。
- `population_name`（可选）：单条规则的工作表名。缺失时按方向自动命名：`<报表科目名称>_借方` 或 `<报表科目名称>_贷方`。
- `account_codes`（可选）：与“科目编码”列做“前缀匹配”。如 `"2202"` 将匹配 `2202/220201/22020101` 等；多个前缀为“或”关系。
  - 缺失或为空（`[]`）：不按科目编码过滤（仅按期间与方向筛选）。
  - 如需精确匹配某编码，请写完整编码为一个前缀。
- `transaction_type`（可选）：为 `debit` 或 `credit`。
  - 缺失时，将自动生成两条规则（借/贷各一条），对应两个工作表。
- `value_column`（可选）：金额列名。
  - 缺失时，不使用自定义金额列；按规则方向自动回退到“借方金额/贷方金额”列。

示例（与仓库内 `config/config.json` 一致，节选）：

```json
{
  "应付账款": [
    {
      "population_name": "应付账款_贷方",
      "account_codes": ["2202"],
      "transaction_type": "credit",
      "value_column": "金额"
    },
    {
      "population_name": "应付账款_借方",
      "account_codes": ["2202"],
      "transaction_type": "debit",
      "value_column": "金额"
    }
  ]
}
```

### 表格列的自动映射规则

为适配不同来源的序时账，本工具会在表头中自动识别常见列名（不区分大小写）：
- 日期列：包含“date/日期/记账日期/凭证日期”等字样。
- 科目编码列：包含“account_code/科目编码/会计科目代码/会计科目编号/科目号”等字样。
- 借方金额列：包含“debit/借方/借方发生额/借方金额”等字样。
- 贷方金额列：包含“credit/贷方/贷方发生额/贷方金额”等字样。
- 借贷方向列（可选）：包含“方向/借贷方向/direction”等字样；若存在，优先据此判定方向；否则根据“借方金额/贷方金额”的大于 0 值判定。

日期解析支持：`YYYY-MM-DD`、`YYYY/MM/DD`、`YYYY.MM.DD`、`YYYYMMDD`、`YYYY-MM-DD HH:MM:SS`、以及 ISO8601 `YYYY-MM-DDTHH:MM:SS`（可带小数秒）。如某行“日期”存在但无法解析，该行视为“无效日期”，会被期间筛选直接排除。

### 构建总体与抽样的处理流程

针对每个需要处理的“报表科目名称”，按其下每条规则执行：
1. 读取序时账（Excel/CSV），自动识别关键列名。
2. 期间筛选：仅保留 `--start` 至 `--end`（含边界）的记录；日期无法解析的行会被丢弃。
3. 科目筛选（是否按编码由配置决定）：
   - 默认（无配置或未提供 `account_codes`）：不按“科目编码”拆分或过滤，总体以“报表科目”为单位（再区分借/贷）。
   - 有配置且提供了 `account_codes`：把其作为“筛选标准”的并集使用（任一前缀命中即保留），用于收敛该“报表科目”的总体范围；不会按单个编码拆成多张表。
4. 方向筛选：
   - 如存在“借贷方向”列，按其内容识别为借/贷；
   - 否则根据“借方金额/贷方金额”两列的正数判定（借方>0 视为借，贷方>0 视为贷）。
5. 金额列：优先取 `value_column` 指定列；该列缺失或为 0 时，自动回退到“借方金额/贷方金额”对应方向的发生额。
6. 形成“总体”（population）：满足 1–5 条件的记录集合。
7. 抽样：
   - MUS：
     - 先按总体金额计算 `n`（用内置 `mus_planning`，参数由 `--materiality`、`--risk-factor` 等确定），
     - 再对总体金额做 PPS 等距抽样（保证金额越大，被抽中概率越高）。
   - 随机：从总体等概率不放回抽取 `--size` 条；若总体不足 `size`，则全取。
8. 输出：把该规则的样本写入 Excel 的一个工作表，表名为 `population_name`。

备注与建议：
- 总体单位是“报表科目名称 + 借/贷方向”，不会按科目编码自动拆分；`account_codes` 仅作为过滤条件。
- 如确有“按口径拆分成多张表”的需要，请在配置中为同一报表科目增加多条规则，并分别命名 `population_name`（例如：应付账款_已开票、应付账款_暂估）。
- `account_codes` 为“前缀匹配”而非完全相等；如需精确匹配，可写完整编码；不填或空数组表示不过滤。
- MUS 仅用“金额>0”的记录参与样本量与 PPS 计算；随机抽样不做此限制。
- 如果你的序时账没有统一的“金额”列，建议在配置中把 `value_column` 留为“金额”，工具会自动回退到借/贷列，仍可正常工作。
- 若需问题定位，可开启调试：`AS_DEBUG=1` 环境变量将输出每条规则的筛选计数（期间/科目/方向等）。

### 无配置模式与缺省行为

配置文件可以省略：
- 当 `--config` 未提供时，工具会从序时账自动识别“报表科目/科目名称/科目全称/一级科目”等列，并把其中的唯一值视为“报表科目名称”集合。
- `--accounts`：
  - 未提供或为 `all` 时，处理上一步识别出的所有“报表科目名称”；
  - 如提供若干名称，则仅处理这些名称。
- 对每个“报表科目名称”，默认生成两条规则（借/贷各一条），工作表名分别为 `<名称>_贷方` 与 `<名称>_借方`；不按科目编码过滤；金额列不指定，直接使用“借方金额/贷方金额”。

如果无法在表头中识别到“报表科目/科目名称/科目全称/一级科目”等任何一列，而又没有提供 `--config`，工具将报错提示无法自动识别总体名称来源。

### 最小可用示例

```
audit-sampler \
  --journal "./test-data/删除模式序时账.xlsx" \
  --start "2024-01-01" \
  --end   "2024-12-31" \
  --method mus \
  --materiality 1000000 \
  --accounts 应付账款 \
  --config ./config/config.json \
  --output ./output/mus_ap_2024.xlsx
```

运行后，`./output/mus_ap_2024.xlsx` 中会出现两个工作表：`应付账款_贷方`、`应付账款_借方`，分别对应配置中的两条规则。
