# Session 状态、Notebook 与 Subagent 执行策略

- status: proposed
- date: 2026-09-17

## 摘要

Checkpoint 和 Notebook 将继续保持两个独立的数据层：

- Checkpoint 保存会话执行状态，用于恢复 Thread、继续 Turn 和创建 Child
  Session。
- Notebook 保存用户或 Agent 明确选择的稳定事实，用于跨 Turn、压缩和 runtime
  重启保留记忆。

Notebook 不会自动收集全部对话，也不会替代 Checkpoint。它将增加更新、遗忘、
重要性和降权机制，但这些机制只影响未来的 Notebook 读取和摘要，不会修改已经
持久化的会话历史。

Child 的 `operation` 记录仍然是第三个独立层。它记录 `queued`、`running`、
`awaiting_approval`、`completed`、`failed` 和 `cancelled` 等任务状态，用于
恢复和控制 Child。它不属于 Notebook，也不应因为任务状态变化而写入长期记忆。

## 当前问题

当前 Notebook 只有 `key`、`content` 和更新时间。它支持覆盖和追加，但不支持：

- 删除或遗忘条目；
- 标记重要性；
- 让低价值内容降低摘要优先级；
- 让用户在 WebStudio 中管理记忆；
- 区分长期事实和临时上下文。

当前最多 32 个条目是实现保护值，不是 Notebook 的语义要求。只依赖条目数也
不能准确表达内容大小。一个 4 KiB 条目和一个很短的条目不应消耗相同的上下文
预算。

## 提案

### 1. 使用稳定的 Notebook 条目模型

每个条目增加离散的 `importance`。不使用浮点权重，避免模型或 UI 产生无法
解释的分数。

```json
{
  "key": "architecture.child_session",
  "content": "Child Session 通过独立 Session/runtime 实现结构并发，不进入 mini-agent-core。",
  "importance": "high",
  "createdAtMs": 1789639000000,
  "updatedAtMs": 1789639000000,
  "lastUsedAtMs": 1789639000000
}
```

级别含义固定如下：

| Level | 语义 | 摘要行为 |
| --- | --- | --- |
| `critical` | 不应丢失的用户约束或核心决策 | 始终优先保留，不自动降权 |
| `high` | 重要架构、偏好和项目事实 | 高优先级，长期保留 |
| `normal` | 一般的跨 Turn 事实 | 按最近使用和更新时间排序 |
| `temporary` | 只在近期有用的工作上下文 | 可以过期或被低优先级淘汰 |

缺少 `importance` 时按 `normal` 读取，保证旧 `notebook.json` 兼容。

### 2. 增加显式的更新和遗忘操作

`notebook_write` 将支持同一 key 的覆盖、追加和 importance 更新：

```json
{
  "key": "architecture.child_session",
  "content": "Child Session 不进入 Core。",
  "importance": "critical",
  "append": false
}
```

新增 `notebook_forget`：

```json
{
  "key": "temporary.current_debug_hint"
}
```

遗忘将从当前 Notebook 快照中删除条目，并增加 `revision`。它不会删除
Checkpoint 中已经存在的历史消息，也不会删除过去的 `notebook_write` 工具
调用记录。用户看到的是“未来不再作为 Notebook 记忆使用”，而不是历史擦除。

新增 `notebook_list` 或扩展现有 WebStudio Notebook API，返回 key、importance、
更新时间和有界内容。返回值不包含物理路径、完整 Checkpoint 或模型隐藏上下文。

### 3. 用重要性和时间生成摘要

摘要排序将使用确定性规则：

```text
critical > high > normal > temporary
同一 level：lastUsedAtMs > updatedAtMs
仍相同：key 升序
```

`temporary` 条目可以带 `expiresAtMs`。过期条目不进入摘要和默认读取，但在
用户明确查看前仍保留，便于用户决定是否遗忘。若实现复杂度需要收敛，第一版
可以只实现 importance 排序，不实现自动过期。

摘要仍受现有总大小限制。当前 64 KiB Notebook 文件上限和单条 4 KiB 上限继续
保留。建议把 32 条改为 64 条硬上限，并把字节上限作为主要预算。条目数仍然
有界，64 KiB 仍然是实际容量上限。

### 4. 保持 Checkpoint 和 Notebook 的边界

Checkpoint 将继续由 Turn 持久化流程自动产生。它包含恢复 Core 所需的消息、
工具结果和上下文状态，不增加重要性级别，也不因为 Notebook 的遗忘而被改写。

Notebook 将继续由 SessionStore 单独保存。恢复时只注入 Notebook 摘要。完整
内容通过 `notebook_read` 显式读取。Notebook 写入不会直接修改 Core 的消息
列表。

创建 Child Session 时只复制父 Checkpoint 的允许内容，不复制父 Notebook。这样
Child 不会把父记忆写入自己的 Notebook，也不会与父 Session 共享写入。Child
可以通过 Host 校验后的只读范围按需读取父 Notebook。读取的是父 Notebook 的
最新完整快照，不是复制出来的第二份文件。

Child 读取父 Notebook 时使用显式范围：

```json
{
  "scope": "parent",
  "key": "architecture.child_session"
}
```

Host 根据 Child Session 已持久化的 `parent_session_id` 校验关系。客户端不能
传入任意 Session ID，也不能传入物理路径。父 Notebook 的写入、更新重要性和
遗忘仍由父 Session 的 SessionStore 负责。父 Notebook 后续更新后，Child 的
下一次读取看到最新版本。

Child 不会默认把完整父 Notebook 注入自己的模型上下文。模型需要相关事实时
才调用 `notebook_read(scope: "parent")`。如果父 Notebook 缺失或损坏，Child
继续使用自己的 Checkpoint 和 Notebook，并收到有界的读取错误。

Child 可以写自己的 Notebook。Child 的结果也不会自动合并到父 Notebook。父
模型通过 `task_read` 获取有界结果后，可以自行判断是否把结论写入父 Notebook。

### 5. 观测 Subagent 并保持审批独立

在本提案中，Subagent 指通过独立 Child Session/runtime 执行的子任务。父
Session 只接收有界的状态、operation 和结果，不接收 Child 的完整历史。

观测链路将保持两条 Thread 事件流：

```text
Parent Thread
  └─ tool_started: delegate_task
       └─ operation_id
            └─ Child Thread events
                 ├─ model/tool progress
                 ├─ approval request
                 └─ turn finished
```

`delegate_task` 的 `tool_started` 事件会在父 Thread 中留下委派证据。Child 的
模型、工具、审批和结束事件继续使用自己的 `thread_id`、`turn_id`、sequence
和 replay cursor。WebStudio 可以在父 Thread 中显示紧凑的 Child 状态行，并
提供进入 Child Thread 查看完整事件的入口。聚合视图只能按 `operation_id`
组合两个投影，不能伪造一条跨 Thread 的 sequence。

WebStudio 重连后先从 SessionStore 的 child catalog 恢复 operation 状态，再
分别按父、Child 的 cursor 读取事件。`queued`、`running`、
`awaiting_approval`、`completed`、`failed` 和 `cancelled` 都必须可见。没有
在线 Child runtime 时，界面显示需要恢复或重新 attach，不把缺少进程误报为
成功。

Child 的实际工具调用会重新经过 Child 自己的 Host admission、Approval 和
ToolRuntime。Parent 已批准的 action 不会自动授权 Child。Child 仍使用当前
项目的工具策略、工作区边界和 sandbox，但每次 approval 都带有 Child 的
`session_id`、`thread_id`、`turn_id`、`call_id` 和 action identity。

`delegate_task` 只创建有界的 Child 请求，不为排队动作增加审批。Child 执行
`shell`、`apply_patch`、MCP 或其他受控工具时，WebStudio 将审批决定路由到
Child App Server。Child 等待审批时，Parent 可以继续运行。取消 Parent 不会
自动取消 Child，用户或控制面可以单独调用 Child 的 `turn/interrupt`。

模型通过 `task_read` 获取 Child 的有界结果。结果不会自动写入 Parent
Notebook。Parent 模型必须先判断结果是否适合长期保存，再显式写入自己的
Notebook。

### 6. 配置 Subagent 并发数量和执行方式

当前每个父 Thread 最多运行两个 active Child。这是资源保护值，不是 Core
的限制。提案将它改为 Host/App Server 的有界配置：

```json
{
  "subagent": {
    "maxConcurrentChildren": 2,
    "defaultExecutionMode": "parallel"
  }
}
```

`maxConcurrentChildren` 的范围建议为 `1..=8`，作用域是一个父 Thread。默认
值保持为 `2`。这个配置只影响新的 Child 启动，不会强行停止已经运行的 Child。
如果多个父 Thread 同时运行 Child，项目还需要单独的资源预算；第一版至少
必须在 Host 侧拒绝超出进程、模型和审批资源的请求，不能把项目级总量误认为
单个父 Thread 的并发数。

`defaultExecutionMode` 支持 `parallel` 和 `sequential`。它只是默认值，单个
Child 组可以显式覆盖。执行方式属于 Child operation group，不属于 Core Turn
Loop。

`parallel` 的规则如下：

- 同一组内的独立 Child 在并发额度内立即启动；
- 超过额度的 Child 保持 `queued`，前一个 Child 结束后再启动；
- 一个 Child 等待 approval 时只占用自己的并发额度，其他 Child 可以继续；
- Child 的实际执行时间可以重叠，WebStudio 显示每个 Child 的独立状态。

`sequential` 的规则如下：

- 同一组按照提交顺序排队；
- 前一个 Child 到达 `completed`、`failed`、`cancelled` 或明确的终止状态后，
  才启动下一个 Child；
- Child 等待 approval 期间不会启动下一个 Child；
- 前一个 Child 失败或取消后，默认停止该组，用户可以单独重试或继续；
- 顺序关系只限制 Child 启动，不改变父 Thread 的执行。

为了让两种方式可恢复，operation 记录将增加有界的 group metadata：

```json
{
  "operation_id": "child:review-a",
  "operation_kind": "child_task",
  "status": "queued",
  "operation_group_id": "review",
  "execution_mode": "sequential",
  "sequence": 1,
  "attempt": 1
}
```

同一 group 的调度状态仍以 SessionStore 中的 operation 记录为准。Host/App
Server 可以维护短期的启动协调器，但它只能执行调度动作，不能成为第二份
历史或授权状态。runtime 重启后，协调器根据 `queued` 和终止记录重建未启动
的 Child，不从 Gateway 内存 map 猜测结果。

协议层可以为 `delegate_task` 增加可选字段：

```json
{
  "child_thread_id": "review-a",
  "prompt": "检查 auth 模块",
  "group_id": "review",
  "execution_mode": "parallel",
  "sequence": 1
}
```

没有 `group_id` 时，Child 使用默认执行方式并独立占用并发额度。客户端不能
传入任意并发数绕过项目配置。Host 以当前项目的有效配置重新校验 group、
mode、sequence 和 Child lineage。

并发和顺序都不改变工具审批边界。每个 Child 仍使用自己的 Host admission、
Approval、ToolRuntime 和 sandbox。`parallel` 可能同时产生多个 approval request，
WebStudio 必须显示对应的 Child 和 operation。`sequential` 只保证启动顺序，
不会合并审批，也不会把 Parent 的 approval grant 传给 Child。

### 7. 提供 WebStudio 记忆管理界面

在当前 Session 的控制面板增加 Notebook 区域，显示：

- key；
- 内容摘要；
- importance；
- 最近更新时间；
- 最近使用时间；
- 更新、调整级别和遗忘操作。

面板应与 Checkpoint 查看分开。Checkpoint 查看器显示执行历史，Notebook 面板
显示可编辑的长期事实。两个面板不能共用“删除历史”的按钮，避免用户误以为
遗忘 Notebook 会擦除会话历史。

Child Session 的 Notebook 面板应分为两个区域：Child 自己的可编辑 Notebook，
以及父 Session Notebook 的只读投影。父 Notebook 区域不显示写入、调整级别或
遗忘按钮，只显示来源 Session 和读取时间。

模型写入 Notebook 后，WebStudio 通过有界事件或重新读取 Notebook projection
更新面板。事件只发送 key、revision、importance 和操作类型，不发送完整内容。

### 8. 控制自动写入

第一版不把每条用户输入自动写入 Notebook。模型只能在判断事实适合长期保存时
调用 `notebook_write`，并且必须选择或使用默认的 importance。WebStudio 可以在
后续增加“记住这条”确认操作，让用户在写入前看到内容和重要性。

以下内容适合从本次讨论中写入 Notebook：

```text
Child Session 不进入 mini-agent-core。
WebStudio 新项目默认开启 pstack。
+ pstack 是 Turn 级工作流激活，$pstack:skill 是 Skill 级显式加载。
启用 Skill 根目录只允许 read_file 读取，脚本执行仍需审批和沙箱。
```

以下内容仍应留在 Checkpoint、Item 历史或 operation 中：

```text
完整对话、截图、shell 日志、plan.md 全文、Child 的 running 状态、工具原始输出。
```

## 故障和安全边界

- Notebook 缺失时，Checkpoint 仍然可以恢复，Runtime 使用空 Notebook。
- Notebook 损坏时，Runtime 不把损坏内容注入模型，并向控制面返回有界诊断。
- 遗忘只影响 Notebook 读取、摘要和面板，不擦除历史 Checkpoint。
- Notebook 不授予工作区读写、Shell 执行或审批权限。
- Child 可以读取自己的 Notebook，也可以按父子 lineage 读取父 Notebook。
  Child 不能写入、更新或遗忘父 Notebook。
- 所有内容、条目数、摘要和事件继续使用现有有界限制。

## 实施批次

1. 扩展数据结构，兼容旧文件并增加 `importance`、`lastUsedAtMs`。
2. 实现 `notebook_forget` 和更新语义，补充原子写入与并发测试。
3. 实现确定性的摘要排序和 `temporary` 条目策略。
4. 增加 WebStudio Notebook projection、编辑、级别调整和遗忘操作。
5. 增加 checkpoint、Child fork、runtime restart 和 replay 场景证据。

## 验收标准

- 同一 key 可以覆盖、追加和修改 importance。
- `notebook_forget` 后条目不再出现在摘要、默认读取和 WebStudio projection。
- `critical` 和 `high` 条目在摘要空间不足时优先保留。
- 旧 Notebook 文件缺少 importance 时仍能读取。
- Notebook 遗忘不会改变 Checkpoint 消息、Turn 历史或 Child lineage。
- Child fork 默认不复制父 Notebook。
- Child 可以按已验证的 parent lineage 读取父 Notebook 的最新快照。
- Child 无法通过父范围执行写入、追加、重要性修改或遗忘。
- 父 Notebook 的更新和遗忘在 Child 的下一次读取中生效。
- Parent 和 Child 的事件可以分别按 cursor replay，聚合视图不会伪造跨 Thread sequence。
- Child 的工具审批不会继承 Parent 的 approval grant，审批请求能定位到 Child。
- Parent 取消不会误取消 Child，Child 可以单独取消并保持 operation 状态一致。
- Runtime 重启后 Notebook 内容、revision 和 importance 保留。
- Notebook 损坏不会破坏 Checkpoint 恢复。
- 面板可以区分“遗忘记忆”和“删除会话历史”。
- 所有更新、遗忘和摘要行为都有有界测试。

## 非目标

- 不把 Notebook 变成完整对话数据库。
- 不自动保存所有用户输入。
- 不使用无法解释的连续浮点权重。
- 不因遗忘 Notebook 擦除 Checkpoint 历史。
- 不在父子 Session 之间复制 Notebook，也不允许共享写入。
- 不自动把 Child 结果或 Child Notebook 合并到父 Session。
- 不在第一版引入向量数据库、语义搜索或跨 Session 全局记忆。
