# Rust 有效行数硬上限调整为 7,000 / 45,000 / 65,000

- status: implemented
- date: 2026-10-01

## 决策

按当前用户要求，将 Core + Protocol 硬上限从 `6,500` 调整为 `7,000`，Control Plane 从
`38,000` 调整为 `45,000`，Release Rust source 从 `55,000` 调整为 `65,000`。Release Rust
每 PR `1,000` 行仍是审查参考值，不是硬门禁。

## 实施

- `scripts/line_budget.py` 是执行值来源，`scripts/test_line_budget.py` 覆盖报告和越界边界。
- 同步更新 `AGENTS.md`、`.github/pull_request_template.md` 和 `docs/limits.md`；历史的
  0.9.0 发布门槛留在 release note 中，不回写当前门槛。
- 预算分类、归属边界和统计口径不变。

## 验证

- `python3 scripts/test_line_budget.py`：17 项通过。
- `python3 scripts/line_budget.py --base HEAD --check-delta --json`：硬门槛为
  `7,000 / 45,000 / 65,000`；本次 Core + Protocol、Control Plane、Release Rust 增量为
  `0 / 12 / 12`，`violations` 为空。
- 当前报告：`6,170/7,000`、`37,847/45,000`、`54,909/65,000`，通过。
