# Runtime State Actor Queue

Status: implemented

## Decision

阶段三把 App Server worker 作为运行时状态 actor。这里的 actor 指“独占状态的异步执行者”：状态只由 worker 持有，其他入口通过命令队列发送请求，并通过一次性回复通道取得结果。

以下状态已经由同一个 worker 队列统一接纳和处理：

| 状态 | 队列内的所有者 | 说明 |
| --- | --- | --- |
| Session | `RuntimeActorState.management.session` | 会话元数据、checkpoint 序号和持久化提交 |
| World | `RuntimeActorState.management.world` | workspace、审批、copilot、sandbox 及上下文更新 |
| MCP | `RuntimeActorState.management.mcp` | 已启用服务、工具数量和待重试服务 |
| Workflow | `RuntimeActorState.workflow` | Plan、Goal、verifier 状态及其文件持久化 |

`RuntimeManagementService` 和 `WorkflowService` 在绑定后只保留 App Server
命令发送端，不再各自持有一份可变运行时状态。CLI、JSON-RPC 和内部
workflow 调用因此共享同一条状态命令队列。原有 JSON-RPC wire API 不变。

状态命令在 Core turn 空闲时和运行时都经过同一个 actor。运行期间，查询及
不触碰当前 `Thread` 的 workflow 状态变更可以继续处理；会读取或修改
`Thread`、checkpoint、MCP tools 的命令返回 `Busy`，直到 Core turn 结束，
避免把暂时从 actor 状态表中取出的运行中 Thread 暴露给并发修改。

## 为什么采用 actor

阶段三解决的不是“有没有锁”，而是多个入口对同一运行时状态的操作顺序和
所有权不清楚。单独的锁只能保护一次读写，不能自动保证“读取 Thread、更新
上下文、写入 Session、更新内存 World”是同一个有序动作。actor 把这些命令
排入一个明确的服务器接纳序列，使并发请求先变成顺序，再由一个执行者更新
状态和持久化结果。

Actor 不替代 Core agent loop，也不意味着所有 capability 都变成 actor。Core
仍拥有模型推理和当前 Thread 的执行语义；Capabilities 仍提供工具、MCP、
审批和文件能力；App Server actor 只负责统一运行时状态的变更入口和顺序。

## Consequences

- Session、World、MCP、Workflow 的管理服务不再形成平行的可变状态来源。
- Workflow 超时、暂停和 verifier 更新可以在 Core turn 仍运行时通过同一队列
  进入，避免状态卡在 `running`。
- `ApprovalController` 仍是 capability 回调通道，不作为运行时状态树的一部分；
  它服务于工具执行中的请求响应，不负责决定状态命令顺序。
- `thread_ids` 仍是 worker 维护的生命周期索引；成功的生命周期变更现在会
  推进同一个 runtime revision，但索引本身仍由 worker 维护，尚未存入
  `RuntimeActorState`。
- 启动阶段仍允许在 worker 绑定前构造 Session 和 Workflow store；绑定完成后
  所有运行时访问都走 actor。

## Verification

- App Server 和 CLI 的全部专项测试通过，包括 session、world、MCP、workflow、
  超时、暂停、恢复和并发命令路径。
- actor 命令协议单独放在 `runtime_command.rs`，处理逻辑保留在
  `runtime_actor.rs`，避免继续扩大单一模块。
- settled turn 的 Session 提交已经收拢到 worker action 内。当前的
  revision/CAS、context 事务和 turn 结算边界见
  `2026-08-30-runtime-revision-cas-and-transaction.md`。
