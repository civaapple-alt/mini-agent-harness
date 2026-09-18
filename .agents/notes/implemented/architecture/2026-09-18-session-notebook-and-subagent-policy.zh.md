# Session Notebook 与 Subagent 执行策略落地记录

- status: implemented
- date: 2026-09-18
- proposal: [Session 状态、Notebook 与 Subagent 执行策略](../../proposed/architecture/2026-09-17-session-state-notebook-and-subagent-execution-policy.zh.md)

## 已落地边界

- Checkpoint、Notebook、Child operation 仍是三个独立的持久化层；Notebook 不进入
  `mini-agent-core`，也不会因为遗忘而改写历史消息。
- Notebook 支持 `critical`、`high`、`normal`、`temporary` 四级重要性，最多 64
  个条目；摘要按重要性、更新时间和 key 稳定排序。`notebook_forget` 删除条目并
  增加 revision。
- Child 不复制父级 Notebook。Host 通过已持久化的 `parent_session_id` 校验
  `scope: "parent"`，允许 Child 只读父级快照；写入和遗忘始终只作用于当前 Session。
- App Server 新增 `session/notebook/read|write|forget`，WebStudio 提供对应 REST
  API 和“记忆”侧栏；当前 Session 可编辑，父级快照只读。
- Child operation 记录保存 prompt、group、execution mode 和 sequence。默认每个
  父 Thread 最多 2 个 active Child，Host 配置范围为 `1..=8`；并发组超额进入
  `queued`，顺序组按 sequence 启动，队列状态来自 SessionStore 而不是内存猜测。
- Child 的 runtime、工具调用和 approval 仍使用独立 Session/Thread；父级授权不会
  自动授予 Child，父级取消也不会隐式取消 Child。

## 保留的非目标

- 没有在 Core 增加 Notebook、调度器、审批继承或跨 Thread 事件序列。
- 没有把父级 Notebook 自动注入 Child，也没有把 Child 结果自动写回父级 Notebook。
- 没有把 Notebook 条目变成无限上下文；所有读取仍受既有有界输出与 Session 权限约束。
