# 父 Session 子任务身份与事件恢复一致性

- status: implemented
- date: 2026-09-26

## 决策

子任务只属于创建它的 main Session。`delegate_task` 接受父 Session 内唯一的
`child_key`，Capabilities 根据父 Session ID 和 key 派生 canonical
`child_thread_id`。显示标题可以重复，不参与身份判断。Gateway 复用该 canonical ID，
并验证已存在 Thread 的 parent Session lineage；不把另一个父 Session 的同名任务
误认作当前任务。

Gateway 将 `tool_started` 中的有界提示词和调度参数，按父 Thread、父 Turn、工具调用
ID 保存到既有委派 receipt；只有配对的 `delegate_task` 成功结果确认 canonical ID 后，
才创建 Child Session。结果不回显提示词，避免 32 KiB 的提示词超过 Core 16 KiB 工具
输出上限而被截断。Receipt 只是崩溃恢复和幂等派发凭据，不是第二份任务状态账本。

事件回放检测到缺口时，Web 丢弃无法证明连续的事件后缀，从持久 Thread history 和
`runtime/status` 重建展示。已结算 Turn 会清除过期的生成状态；消息块自身残留的
streaming 标记不能继续驱动“思考中”计时器。缺少 `tool_started` 参数的成功委派回放
会生成可见失败诊断，不会静默漏派。

## 变更准入

1. **所属边界：**Capabilities 生成并返回身份；Gateway 关联工具调用、管理派发和
   恢复；Web 重建事件投影并根据权威 Turn 状态收敛活动指示器。
2. **已有职责：**复用 `DelegateTaskTool`、SessionStore 的 parent lineage、Gateway
   既有委派 receipt、Web 的 history/runtime 查询。没有增加 Core 任务循环或 Gateway
   任务账本。
3. **替代内容：**不再把模型给出的 Thread ID 或显示名称当作子任务身份；不再把
   不完整事件后缀叠加到恢复快照，也不再让过期 block 标记单独控制运行计时。
4. **行数：**有效 Rust Release 源码由 44,381 增至 44,466（净增 85）；Control Plane
   为 30,904（净增 0）；Core + Protocol 为 4,767（净增 0）。均低于当前硬上限，
   本变更低于 1,000 行增量门禁。
5. **协议与持久化：**`delegate_task` 入参使用 `child_key`；成功结果返回 canonical
   `child_thread_id`、operation 和调度元数据，但不重复返回 prompt。Gateway 在既有
   receipt 中暂存有界参数。没有改变 Session operation 格式或跨 Session 共享行为。
6. **边界证据：**Capabilities 129 项测试、App Server 72 项测试、Gateway 129 项测试、
   前端 Node 78 项与 Vitest 131 项测试均通过；相关 Rust clippy、Web lint/build、
   Ruff 和行数门禁通过。使用 mock 与确定性 fixture，没有调用付费模型。

## 验证

- 同一个 `child_key` 在不同父 Session 下生成不同 canonical ID；相同 Session 重放保持
  稳定。Gateway 拒绝将其他父 Session 拥有的 Thread 当作当前 Child。
- 最大 32 KiB prompt 从 `tool_started` receipt 恢复，且不会进入成功工具结果；只有
  成功 `tool_finished` 才触发 materialize。
- 缺少对应 start 事件的 finish 回放显示失败状态和诊断。
- 事件缺口后不消费不完整后缀；runtime 已结算后，旧 reasoning block 不再维持
  “思考中”或递增耗时。
