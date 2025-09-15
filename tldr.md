# audit-sampler TL;DR

面向一线使用者的快速上手与案例。详细原理与完整说明见 README。

## 我有一个序时账 Excel，想快速抽样

无配置模式（严格使用“报表科目”列，按借/贷各建总体，默认不过滤科目编码）：

```
audit-sampler \
  --journal ./test-data/删除模式序时账.xlsx \
  --start 2024-01-01 \
  --end   2024-12-31 \
  --method random \
  --size 50 \
  --accounts all \
  --output ./output/random_all.xlsx
```

只做“应付账款”（名称来自序时账“报表科目”列）：

```
audit-sampler \
  --journal ./test-data/删除模式序时账.xlsx \
  --start 2024-01-01 \
  --end   2024-12-31 \
  --method random \
  --size 30 \
  --accounts 应付账款 \
  --output ./output/random_ap.xlsx
```

## 我有抽样口径定义，想用配置文件

最常见的配置（示例）：应付账款按借/贷分别抽样，编码以 2202 为前缀。

```json
{
  "应付账款": [
    { "population_name": "应付账款_贷方", "account_codes": ["2202"], "transaction_type": "credit" },
    { "population_name": "应付账款_借方", "account_codes": ["2202"], "transaction_type": "debit" }
  ]
}
```

运行：

```
audit-sampler \
  --journal ./test-data/删除模式序时账.xlsx \
  --start 2024-01-01 \
  --end   2024-12-31 \
  --method mus \
  --tolerable-misstatement 100000 \
  --accounts 应付账款 \
  --config ./config/config.json \
  --output ./output/mus_ap.xlsx
```

要点：
- 配置文件所有字段都是可选：
  - 不写 `population_name` → 自动命名 `<科目>_借方`/`<科目>_贷方`
  - 不写 `account_codes` 或设为 `[]` → 不按编码过滤
  - 不写 `transaction_type` → 自动生成借/贷两条规则
  - 不写 `value_column` → 金额按借/贷列取值
- 不提供 `--config` 也能运行：自动识别名称列与借/贷列。

## MUS 与随机：我该怎么选？

- MUS（货币单元抽样）：金额越大越容易中样，适用于金额集中、重大发生额业务。需要给出 TE（或 materiality）。
  - 若 TE ≥ 总体金额 → n=0（不抽样，默认不强制最小样本量）。
  - EE = TE × 风险系数（默认 0.25）。
- 随机：等概率抽样，适用于均匀样本或流程合规测试。给出 `--size` 即可。

## 常见问题（QA）

1) 没有配置文件可以跑吗？
- 可以。按“名称列+借/贷列+期间”自动构建总体。

2) 工作表名里有中文会报错吗？
- 工具会自动清洗和截断工作表名以满足 Excel 限制。

3) 只想按期间和方向，不想过滤科目编码怎么办？
- 配置里把 `account_codes` 留空或不写即可，或无配置模式下默认就不过滤。

4) 想看筛选过程是否正确？
- 运行时加环境变量 `AS_DEBUG=1` 可输出筛选计数（期间/编码/方向）。

## 一键准备（可选）

Release 构建并运行：

```
cargo run --release -- --journal ./test-data/删除模式序时账.xlsx --start 2024-01-01 --end 2024-12-31 --method random --size 20 --accounts all --output ./output/random_all.xlsx
```

安装后直接使用：

```
cargo install --path . --force
audit-sampler --help
```
