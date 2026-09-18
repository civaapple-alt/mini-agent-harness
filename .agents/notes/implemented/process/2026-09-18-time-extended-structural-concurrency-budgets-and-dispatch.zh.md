# 时间上延展、结构上并发：预算与 Child 分派意图

- status: implemented
- date: 2026-09-18
- origin: 时间上延展、结构上并发的 Agent Harness 改造

## 当前决策

Child Session 的并发容量和单次任务的调度意图是两个不同维度：

- 全局或项目配置只保存 `subagent.max_concurrent_children`，范围为 `1..=8`，
  默认值为 `2`；
- Main Thread 在每次 `delegate_task` 或 Child 创建请求中选择
  `execution_mode: "parallel" | "sequential"`；
- 有依赖的任务可附带 `group_id` 和 `sequence`，由 Host 持久化到 operation；
- `parallel` 表示任务独立时可以占用可用槽位，`sequential` 表示同一 operation
  group 按顺序等待；
- Host 负责校验和执行约束，不根据项目配置猜测当前任务的调度方式；
- 旧客户端不传 `execution_mode` 时兼容回退为 `parallel`，但不再把它写入全局或
  项目配置。

这保持了“时间上延展”的 operation、Session 和恢复状态，也保持了“结构上并发”
的独立 Child runtime；Core 不新增 Scheduler，Gateway 不建立第二份生命周期权威。

## 预算门禁

为支持该架构持续演进，同时保留硬约束，预算调整为：

| 指标 | 硬上限 | operating | red band |
| --- | ---: | ---: | ---: |
| Core + Protocol | 6,000 | — | — |
| Control Plane | 28,000 | — | — |
| Release Rust source | 40,000 | 39,000 | 39,500 |

Control Plane 仍然单独统计；Release Rust source 包含受支持 Runtime 包的生产代码和
测试，不包含实验性 CLI/REPL。Runtime 聚合值继续报告，但不再设置 `25,000` 的
聚合硬门禁；Core + Protocol 的 `6,000` 硬门禁保持执行内核和公共协议的最小边界。
放宽上限不是放宽边界：新增实现仍需删除优先、保持 Core/Host/Capabilities 所有权
清晰，并通过增量预算检查。

## 实现位置

- `mini-agent-capabilities` 的 `delegate_task` schema 暴露本次分派的
  `execution_mode`、`group_id` 和 `sequence`；
- WebStudio Gateway 将模型事件中的调度意图传给 `SessionManager.start_child_task`；
- `SessionManager` 只从全局/项目设置读取并发上限，并将实际模式写入 Child operation；
- `scripts/line_budget.py`、贡献者门禁和发布文档同步使用新的预算值。

## 取舍与剩余风险

- 没有增加项目级 `default_execution_mode`，避免把任务依赖关系固化成静态策略；
- `parallel` 兼容回退只服务旧调用方，新 Main Thread 应显式表达有依赖的顺序任务；
- 当前仍限制 Child 深度为一层；跨父 Thread 的全局资源预算不是本次变更的一部分。

## 验证

- `cargo fmt --all`：通过；
- `cargo test -p mini-agent-capabilities`：99 passed；
- `cargo test -p mini-agent-protocol`：10 passed；
- `cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings`：通过；
- `python scripts/test_line_budget.py`：15 passed；
- `python scripts/line_budget.py`：Core + Protocol `4518/6000`、Control Plane
  `23603/28000`、Release Rust `34809/40000`，无 violation；
- `mini-agent-web` `uv run pytest -q`：159 passed；Ruff changed-file check 通过。
