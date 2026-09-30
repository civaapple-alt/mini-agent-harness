# Core + Protocol 预算与 Release 增量参考值

- status: implemented
- date: 2026-09-30

## 决策

Core + Protocol Rust 有效行硬上限调整为 `6,500`。Control Plane 与 Release Rust
硬上限分别为 `38,000` 和 `55,000`。每次变更 `1,000` 行 Release Rust 是审查参考值，
不是硬门禁；超出时报告增量并审查依据，仍只由三个绝对上限决定预算是否通过。

## 实施

- `scripts/line_budget.py` 使用三个绝对硬上限；`--check-delta` 报告 Core + Protocol、
  Control Plane 和 Release Rust 的增量，并继续检查绝对上限。
- 文本报告会提示 Release 增量高于参考值；JSON 明确标示该参考值不是硬门禁。
- `AGENTS.md`、`.github/pull_request_template.md`、`docs/limits.md` 和
  `docs/releasing.md` 使用相同门槛和措辞。

## 验证

- `python3 -m unittest scripts.test_line_budget -v`：17 项通过，包含 `+1,001` 行仍通过
  硬门禁的回归验证。
- `python3 scripts/line_budget.py`：Core + Protocol `6,101/6,500`、Control Plane
  `35,614/38,000`、Release Rust `51,809/55,000`，通过。
- `python3 scripts/line_budget.py --base HEAD --check-delta --json`：分别增加
  `513`、`828`、`1,385` 行；Release 超过 `1,000` 参考值只产生提示，`violations` 为空。
