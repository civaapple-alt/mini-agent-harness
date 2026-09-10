# 行数门禁上调的系统工程背景

Status: implemented  
Date: 2026-09-10  
Scope: Runtime、Release Rust、Control Plane 的行数预算治理

## 决策背景

2026-09-09 的 `d487d77`（`build: raise source line budgets`）将 Runtime 硬上限从
`20,000` 调整为 `25,000`，Release Rust 硬上限从 `30,000` 调整为 `35,000`，并
增加 `25,000` 的 Control Plane 硬上限。此前的直接工程事实是：Runtime 基线为
`19,887/20,000`，Release Rust 基线为 `29,997/30,000`，继续在原门槛内维护会把
必要的安全、恢复和边界证据挤出预算。

这次调整还有一个此前没有写入 note 的方向性背景：Agent Harness 最终是系统工程，
不是 Planner、Coder 或 MCP 功能数量的竞赛。近期对外分享的思考将这一点概括为：

> 薄 Agent Loop，厚 Control Plane：模型和 Framework 可以替换，但 State、Permission、
> Sandbox、Recovery 等边界要稳定。Harness 的差距在于能否把测试、故障注入、验证、
> Rollback、Audit 等能力组合成真正可交付的系统。

这也解释了为什么数据库系统中已经长期存在的测试、故障注入、验证、检查点、恢复、
补偿和审计方法，适合成为 Harness 的工程参照。模型能力增强后，Orchestration 不应
无限增厚；应优先让 Agent 理解 Goal、Boundary、Invariant，而不是用更多步骤掩盖
控制面边界不稳定的问题。

## 为什么不是继续压缩到旧门槛

旧门槛与当前系统职责之间出现了错配：

1. State、Permission/Sandbox、Recovery、Verification、Audit 是交付可靠性的结构性
   边界，不是可随意删除的装饰代码；
2. 为了挤进 `20,000/30,000` 而删除 Core、Actor/CAS/Session authority、公共协议或
   边界测试，会让行数指标变好，却降低系统真实性和安全性；
3. 把所有 Rust 行数压成一个数字，会掩盖 Control Plane 的实际复杂度，也无法区分
   可替换的 Provider 增长与必须稳定的控制面增长；
4. 因此应保留足够但有限的硬上限，并单独观察 Control Plane，而不是用破坏边界的
   重构或 Cargo 胶水层换取统计上的通过。

## 调整的边界

这不是放弃行数治理，也不是为未来堆叠 Framework 预留无限空间：

- `25,000` Runtime、`35,000` Release Rust 和 `25,000` Control Plane 仍是硬上限；
- operating/red 区间同步上调为 Runtime `24,000/24,500`、Release Rust
  `34,000/34,500`；
- Green/Amber 的单 PR 增量仍分别受 `+100/+150` 行约束，进入 Red 后冻结正增长；
- 新代码默认净零增长，优先删除废弃概念、重复状态和冗余分支；
- 行数门禁只是准入证据之一，仍必须同时检查职责边界、权限、Sandbox、Recovery、
  Audit、协议和测试证据。

## 记录缺口与补记

当时把调整视为构建门禁和发布基线校准，直接同步到了 `AGENTS.md`、
`CHANGELOG.md`、`docs/releasing.md`、`scripts/line_budget.py` 及其测试，没有另立
一份记录“为什么必须给系统工程留出控制面空间”。因此，2026-09-07 的
[`Line Gate：Control Plane 统计与增量门禁`](2026-09-07-line-gate-control-plane-and-delta.md)
仍保留调整前的基线和阈值，不能单独作为当前上调决定的完整依据。

本 note 补齐的是决策背景和护栏，不替代当前配置。当前数值以 `AGENTS.md`、
`scripts/line_budget.py` 和发布文档为准；架构方向与实现边界见
[`薄 Agent Loop、厚 Control Plane：系统工程决策`](../architecture/2026-09-08-thin-loop-thick-control-plane-system-engineering.zh.md)。

## 后续约束

后续如果再次调整预算，必须同时记录：实际基线、硬上限、operating/red 区间、
调整原因、是否发生职责边界变化，以及为什么不能通过删除/重构旧概念解决。预算
变化不能只出现在构建配置中，也不能把一次门禁校准误解为降低系统工程约束。
