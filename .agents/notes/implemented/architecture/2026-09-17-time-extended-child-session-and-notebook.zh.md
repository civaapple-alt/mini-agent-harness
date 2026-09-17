# 时间上延展、结构上并发：Child Session、operation 与 Session notebook

- status: implemented
- date: 2026-09-17

## 当前决策

Child Session 不进入 `mini-agent-core`。Core 继续运行一个 Thread 的一个
显式 Turn，负责工具契约、限制、停止分类和观察事件。Host/App Server 通过
独立 Session、独立 runtime、独立锁和独立 replay 表达结构并发。

创建 child 时，Host 从父 Session 读取最近一次完整持久化 checkpoint。它不
复制父 Core 的可变上下文、in-flight prompt、工具调用或审批。Child 随后按
相同的 Protocol、Host、Capabilities 和 Approval 路径运行。

## 已落地的控制接缝

- `delegate_task` 是 Host capability。它只返回有界的排队请求，不在 Core 中
  创建 scheduler。
- WebStudio 观察父 runtime 的真实 `delegate_task` `tool_started` 事件，调用
  相同的 child control seam。Gateway 只编排和转发，不维护第二份历史或授权
  状态。
- `task_read` 从 child Session 的 canonical projection 读取状态、attempt、
  result 和 error。完整 child history 不内联到父上下文。
- 每个父 Thread 最多两个 active child，child 深度限制为一层。Retry 使用
  相同 `operation_id` 和递增的 `operation_attempt`，并启动新的 child Turn。

`session.jsonl` 中的 operation 记录是可恢复状态的来源。记录支持
`queued`、`running`、`awaiting_approval`、`completed`、`failed` 和
`cancelled`。Gateway 重启后通过 Session catalog 重建 child 投影。取消仍
然是标准 cooperative `turn/interrupt`，不会删除 child Session。

## Session notebook

`notebook.json` 由 SessionStore 所有。`notebook_read` 和 `notebook_write`
通过 Host/Capabilities 访问。Runtime resume 只注入有界摘要，完整条目仍需
显式读取。Notebook 跨 Turn、压缩和 runtime 重启保留，但第一版不允许 child
写入父 notebook，也不把 notebook 变成 Core 的共享可变状态。

## 为什么不会破坏 Core

Core 看见的仍是普通 Turn 和可选的有界 operation correlation 字段。Child
创建、排队、启动、取消、重试、Session 持久化和恢复都发生在 Core 外部。
因此现有单 Thread active Turn 不变，旧客户端缺少 operation 字段时也保持原
行为。

## 当前非目标

第一版不做 Core 内多租户 scheduler、递归 child、跨机器执行、任意 shell
后台任务、父上下文自动拼接完整 child history，或 child 与父 notebook 的
共享写入。模型需要结果时使用 `task_read`。

## 验证

- `cargo test -p mini-agent-protocol`
- `cargo test -p mini-agent-capabilities`
- `cargo test -p mini-agent-app-server`
- `cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings`
- `python scripts/cargo_boundary.py --json`
- `python scripts/line_budget.py`
