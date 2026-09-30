# Rust 行数预算上限校准：Control Plane 38,000 / Release 55,000

- status: implemented
- date: 2026-09-30

## 决策

将 Control Plane 有效 Rust 行硬上限从 `35,000` 调整为 `38,000`，将 Release Rust
source 硬上限从 `50,000` 调整为 `55,000`。Core + Protocol 仍为 `6,000`，Release
Rust 单次 PR 净增量仍为 `1,000`。预算分类、职责边界和统计口径均不变；门禁仍只使用
硬上限和单次增量检查，不设 operating 或 red 区间。

## 背景

调整前工作区为 Core + Protocol `5,588/6,000`、Control Plane `34,365/35,000`、
Release Rust `49,985/50,000`。Control Plane 余量为 `635` 行，Release Rust 余量为
`15` 行。当前交付需要维持既定 Host、Capabilities 与 App Server 边界；将硬上限校准
到 `38,000` 和 `55,000` 为后续有证据的工作留出有限空间，不改变任何层的所有权。

本次属于门禁校准，不扩展运行时功能。Core + Protocol 上限和每次 Release Rust
`1,000` 行增量门禁保持不变；新上限仍是约束，不是增长目标。超限时应先检查是否能
删除过时概念或简化实现，再决定是否需要新的预算调整。

## 实施位置

- `scripts/line_budget.py` 是可执行门槛的唯一来源。
- `scripts/test_line_budget.py` 覆盖报告数值与 Control Plane、Release 超限边界。
- `AGENTS.md`、`.github/pull_request_template.md`、`docs/limits.md` 和
  `docs/releasing.md` 同步公布当前门槛。
- 本记录进入 `.agents/notes/README.md` 的 implemented/process 索引。

## 验证

- `python3 scripts/test_line_budget.py`：17 项通过。
- `python3 scripts/line_budget.py`：Core + Protocol `5,588/6,000`、Control Plane
  `34,786/38,000`、Release Rust `50,424/55,000`，通过。最终统计包含同批完成的
  Skill 运行时发现刷新修复。
- 受影响 Rust 包的 fmt、Clippy 与测试通过；详见 Skill 发现刷新记录。
