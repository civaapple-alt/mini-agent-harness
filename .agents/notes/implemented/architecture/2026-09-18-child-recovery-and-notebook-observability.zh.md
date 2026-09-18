# Child 恢复与 Notebook 可观测性补全

## 状态

已实现。保留独立 Child Runtime、Main Thread 按次决定执行模式，以及 Notebook
独立持久化；本批次补齐 Gateway 恢复、委派幂等、父任务状态投影和 Notebook
更新失效通知。

## Child operation 恢复

- Runtime 启动、重新 attach、Session 恢复和项目 Runtime 重启后，Gateway 扫描
  `session.jsonl` 的最新 `operation` 投影。
- 没有 `turn_id` 的 `queued` 操作重新进入既有队列并按 slot/group 规则 drain。
- 已有活动 `turn_id` 的 Child 只重新绑定等待任务，不创建重复 Turn。
- Child 结束后继续 drain 同一父 Thread 的队列；配置变更只影响后续调度和可恢复
  队列，不改写已有 Child 的执行模式。

## delegation receipt

`delegate_task` 在 Child materialization 前写入有界 receipt。receipt 以父 Turn、
事件 item 或调用 ID 组成幂等键，记录父子 Thread、prompt、项目、执行模式和
operation group 等最小调度元数据。Gateway 重启后：

- `pending` receipt 补建 Child operation；
- `materialized` receipt 重新绑定已有 Child；
- `completed`/`failed` receipt 不重复创建 Child。

receipt 不复制 Child checkpoint、工具输出或正文；SessionStore 的 operation
projection 仍是 Child 状态权威。

## WebStudio 观测

父 Thread 接收有界的 `child_operation_updated` 状态事件，也可以通过现有
`listChildTasks()` 读取 canonical projection。事件只包含 operation、Child
Thread、状态、执行模式、group/sequence 和有限错误码，不内联 Child transcript。
刷新页面后由 projection 恢复状态，点击状态行可以进入独立 Child Thread。

## Notebook

- `notebook_write` 和 `notebook_forget` 发送 `session/notebook/updated`，只携带
  `threadId`、`revision` 和 `changedKeys`；WebStudio 收到后重新读取 Notebook。
- 全局默认与项目覆盖统一使用 `max_entries`、`max_entry_bytes`。不存在旧字段
  别名或旧环境变量双写；Runtime 只读取
  `MINI_AGENT_NOTEBOOK_MAX_ENTRY_BYTES`。
- 有效总大小由条数、单条上限和固定元数据预算计算，并封顶 64 KiB。
- `evidence` 是调用方声明的来源元数据，不宣称已由 Git 或文件系统验证。

`lastUsedAtMs`、自动过期、权重衰减和 Git 自动 evidence 校验不属于本批次，继续
作为后续提案，避免把 Notebook 变成新的运行时调度系统。

Child delegation 同样不保留隐式模式：`execution_mode` 由 Main Thread 每次
显式选择，缺少该字段的请求直接拒绝，不回退到 `parallel`。

## 验证

覆盖 Gateway receipt reload、Child queue reconciliation、Notebook 配置继承与
覆盖、协议通知序列化、Rust 单元测试、Gateway 测试和 WebStudio 前端测试/构建。
