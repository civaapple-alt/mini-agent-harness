# 面向交付的 Agent 运行系统行数门禁

- status: implemented
- date: 2026-09-20

## 决策

Mini Agent Harness 是一个面向交付的 Agent 运行系统。它需要持续建设执行控制、
状态持久化、权限、恢复和验证能力。行数门禁应为这些职责留出有限空间，同时保留
可检查的硬上限和单次变更限制。

| 指标 | 原硬上限 | 新硬上限 | 单次增量 |
| --- | ---: | ---: | ---: |
| Core + Protocol | 6,000 | 6,000 | 仅执行硬上限 |
| Control Plane | 28,000 | 30,000 | 仅执行硬上限 |
| Release Rust source | 40,000 | 45,000 | +1,000 |

Release Rust source 包含受支持运行时 crate 和测试，不包含实验性 CLI/REPL。
取消 Release Rust 的 `39,000` operating limit、`39,500` red band 和红区冻结规则。
每次变更只要 Release Rust 总量不超过 `45,000`，且相对基线的净增长不超过
`1,000` 行，就通过行数门禁。Core + Protocol 和 Control Plane 仍受各自硬上限约束。

## 背景

旧门禁把 Release Rust 硬上限设为 `40,000`，并在 `39,500` 行后冻结正增长。
调整前的工作区有 `39,592` 行，只剩 408 行旧硬上限空间。它同时包含 Session 恢复、
子代理协调和 Harness 验证等交付能力。旧冻结线会在剩余硬空间尚未用完时拒绝
每项新增实现，无法反映这些职责的实际成本。

本次调整扩大 Control Plane 和 Release Rust 的硬上限，并把单次增长限制统一为
`1,000` 行。它给有证据的功能批次留出空间，仍限制每次改动的大小，也不改变
Core、Host、Capabilities、App Server 的职责边界。Core + Protocol 上限保持
`6,000`，以保留执行内核和协议的独立约束。

## 实施位置

- `scripts/line_budget.py` 执行三个硬上限和 Release Rust `+1,000` 增量检查。
- `scripts/test_line_budget.py` 覆盖硬上限、允许的最大增量、超限增量和旧红区范围内的增长。
- `AGENTS.md`、`docs/limits.md`、`docs/releasing.md`、`.github/pull_request_template.md`
  和提案证据指南使用相同数值。
- 子代理动态协作提案记录本次工作区实际行数和准入结果。

## 后果

- 工作区超过硬上限时，`python scripts/line_budget.py` 失败。
- PR 的 Release Rust 增量超过 `1,000` 行时，带 `--check-delta` 的检查失败。
- 达到原来的 `39,000` 或 `39,500` 数值不再改变门禁状态。
- 新代码仍应控制增长；行数通过不能替代所有权、安全和边界证据审查。

## 验证

- `python -m unittest scripts.test_line_budget`：17 项通过。
- `python scripts/line_budget.py`：Core + Protocol `4,663/6,000`、Control Plane
  `26,775/30,000`、Release Rust `39,592/45,000`，通过。
- `python scripts/line_budget.py --base HEAD --check-delta --json`：Release Rust
  `+758`、Control Plane `+509`，通过。
