# Harness 实现维护历史索引

Date: 2026-09-03
Status: implemented history index

本文件只记录已经落地的维护批次和当前缺口，不承载下一批实现细节。每个新批次
应另建 dated note；原始逐批记录仍保存在
[`2026-08-31-harness-lessons-archive.md`](2026-08-31-harness-lessons-archive.md)。

## 已完成批次

| 日期 | 批次 | 结果 |
| :--- | :--- | :--- |
| 2026-09-01 | Plan → `collaborationMode` | 公共入口切换到 `thread/settings/update`；旧 `workflow/plan/set` 和 facade wrapper 移除 |
| 2026-09-01 | P0 预算释放 | 移除死 facade 与 REPL 管理展示，保留 Core/Actor/CAS/Session 权威 |
| 2026-09-01 | P1 重复测试清理 | 保留公共边界证据，删除只重复低层实现的测试 |
| 2026-09-01 | REPL 收缩 | 移除重复启动摘要、Session 管理展示和交互管理胶水 |
| 2026-09-01 | REPL 30% 压缩 | 固定启动模式，避免把实验性 CLI 管理面提升为运行时概念 |
| 2026-09-03 | Host 测试清理 | 收敛重复 Host 测试，保留真实公共路径 |
| 2026-09-03 | Host catalog 与 Capabilities fixture | 复用 catalog/fixture，减少重复样板 |
| 2026-09-03 | Capabilities helper 清理 | 收敛 blocking helper 与低层重复断言 |
| 2026-09-03 | App Server fixture 清理 | 复用公共测试 fixture，保留跨层断言 |
| 2026-09-03 | GoalRuntime fixture 收敛 | 复用 settled-checkpoint fixture，保持 Goal 状态权威 |
| 2026-09-03 | post-admission outcome projection | 收敛 Capabilities 结果投影，保持 structured denial contract |
| 2026-09-03 | Builtin tool surface upgrade | 默认 4 个工具、分页 `read_file`、预校验 `apply_patch`，App Server/Web 状态对齐 |

## 当前预算与质量门

最近一次实现批次后：

| 指标 | 当前值 | 硬上限 |
| :--- | ---: | ---: |
| Runtime Rust（Core/Protocol/Host/App Server） | 18,614 | 20,000 |
| Release Rust（Core/Protocol/Capabilities/Host/App Server） | 28,470 | 30,000 |
| CLI/实验 REPL | 3,244 | 单独报告 |

文档整理不改变 Rust 预算。Rust 改动仍须运行受影响测试、fmt、Clippy 和
`python scripts/line_budget.py`；跨 package 改动才运行 workspace Clippy，完整
workspace test 需要显式批准。

## 后续缺口

- CLI public MCP transport 的 bounded timeout projection；
- HTTP 429 的 provider-specific retry/backoff contract；
- Docker 更强 network/capability/resource isolation 的明确政策与跨平台证据；
- 独立 model/provider comparison scenario；
- 更完整的工具失败、超时、重试组合矩阵。

这些是 evidence 或政策缺口，不是自动扩张 runtime 的理由。除非先删除等量冗余
或提供明确 offset，否则不增加 Rust 生产面。
