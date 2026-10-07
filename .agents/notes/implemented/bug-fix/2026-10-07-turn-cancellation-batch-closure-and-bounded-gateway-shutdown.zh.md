# Turn 取消结算、工具批次闭合与 Gateway 有界停机

状态：implemented
日期：2026-10-07
范围：Core、App Server、Python SDK、Gateway、Web Studio

## 决策

停止信号到达 Core 后，立即取消本地等待中的模型请求 future，不等待模型生成自然结束。该行为只关闭本地等待；供应商是否继续远端计算不在本地保证范围内。

工具批次在每个调用启动前检查取消状态。已经启动的工具保留实际结果；未启动的调用记录为 `cancelled`，并为其 `callId` 写入匹配的工具结果。这样既闭合模型调用历史，也不会把未知副作用伪装成未执行。工具运行时不能取消或结果未持久化时，恢复仍要求用户核对实际状态。

Gateway 关闭时先拒绝新的 Turn，并向活动的父、子 Session Turn 并发发出中断。它保留流观察任务，在共享的最多 10 秒窗口内等待终态事件或权威状态/检查点；窗口到期仍未确认的执行状态保持未知。随后 SDK 优雅关闭 App Server，并在最多 5 秒的清理阶段向独立 POSIX 进程组发送终止信号；Uvicorn 的优雅退出上限为 15 秒。超时清理不代表 Turn 已结算。

重连和重启后的历史仍以 App Server 持久化 Session、Turn 检查点和工具执行 journal 为准。事件回放只补生命周期线索，不触发 Turn 或工具重放。副作用未知的调用先核对，之后由用户显式恢复同一 Turn。

## 边界

- Core `RunControl` 与工具批次执行器负责立即取消模型 future、传播工具取消信号，并为未启动调用闭合 `callId`。
- Host/Capabilities 的现有取消与执行 journal 继续负责工具副作用及恢复核对；Gateway 不建立第二份执行状态。
- Python SDK 管理隔离的 App Server 子进程及其进程组。Gateway 管理新 Turn admission、活动 Turn 中断、终态观察和整体退出期限。
- Web Studio 仅区分取消结果与工具失败，并依据权威恢复状态显示 Turn；它不会因超时自动重发或续跑。
- steer 的持久化、请求 ID 去重和 `accepted` / `applied` / `unapplied` 状态仍由 [长 Turn 纠偏与 Session 恢复可信性](../architecture/2026-10-06-long-turn-steer-and-session-recovery.zh.md) 记录；停止先于应用时，未应用纠偏不会越过取消状态继续执行。

## 验证证据

- 受影响 Rust 包测试、Clippy 和 `cargo fmt --all` 通过；覆盖模型 future 取消、工具批次中断及未启动 `callId` 的结果闭合。
- Python SDK 全量测试 108 项通过，Ruff 检查通过；Web Gateway 测试 261 项、离线 smoke 5 项、UI 测试 47 项通过，ESLint、Ruff 与格式检查通过。
- `python scripts/line_budget.py` 通过：Core + Protocol `7,312 / 7,500`，Control Plane `43,318 / 45,000`，Release Rust `62,330 / 65,000`。
- 两仓 `git diff --check` 通过。没有调用真实模型供应商。

## 剩余限制

没有在浏览器中手动执行真实长 Turn 的 Ctrl+C、断线和重启走查；退出时序由本地 Gateway/SDK 测试与离线 smoke 覆盖。强制终止后，无法确认的工具副作用仍必须由用户核对，10 秒结算窗口也不承诺供应商停止远端计算。
