# Goal Runtime 与 `thread/goal/*` 实施附录

Status: implemented — canonical execution appendix
Date: 2026-09-01
Scope: mini-agent App Server / Host workflow control plane

本附录承接 [Codex-aligned capabilities and ThreadItems](2026-09-01-codex-aligned-capabilities-thread-items.md) 与 [Harness framework and next-iteration note](../../implemented/architecture/2026-08-31-vscode-harness-lessons-next-iteration.md)，只记录 Goal Runtime 的落地边界、自动续跑和通知。前者是总架构记录；本文件不是第二套 workflow 设计。每个阶段仍须经过六项变更准入和独立提交；本轮 Batch 1–5 已在同一 breaking checkpoint 收敛完成。

## 1. 当前结论

原先的推荐顺序是：

1. `CollaborationMode` 和线程级设置；
2. 保持 `workflow/plan/set` 已移除，不提供兼容适配器；
3. typed `WorkflowPolicy`；
4. `thread/goal/set/get/clear`；
5. `GoalRuntime` 收回 verifier、advance、continuation；
6. `thread/goal/updated` 通知；
7. 最后废弃旧的 `criteria`、`record_verdict`、`advance`。

其中前 1 项已完成，前 2 项被当前公共协议决策有意改写，前 3 项已通过现有 typed mode 与 Actor policy seam 部分实现，前 4–7 项已在本轮收敛完成。Goal Runtime 直接复用最新 Codex-shaped Thread/Turn 边界，不再在 `WorkflowService` 上暴露手工推进接口。

| 原步骤 | 当前状态 | 当前事实与计划处理 |
|---|---|---|
| 1. `CollaborationMode` | 已完成 | `thread/settings/update` 使用 typed `collaborationMode`；Runtime Actor 应用 bounded prompt、Plan approval lock 和持久化恢复。 |
| 2. `workflow/plan/set` | 已明确不保留 | 旧方法已删除，不注册、不暴露，也不提供兼容适配器。这是有意的 breaking change；后续不得重新引入。 |
| 3. typed `WorkflowPolicy` | 部分完成 | 当前由 `CollaborationMode`、Host `PlanModeState`、`ApprovalController` 和 Runtime Actor 共同形成 typed policy seam；不为形式上的 `WorkflowPolicy` 再增加一层。只有出现第二种 workflow policy 时才考虑提取独立类型。 |
| 4. `thread/goal/set/get/clear` | 第一批已完成 | 已补齐 Codex-shaped protocol、public JSON-RPC、local client、bounded Host state 和 set/get/clear 公共场景。 |
| 5. `GoalRuntime` | 已完成 | `GoalRuntime` 已成为 Runtime Actor 内的串行状态 owner；settled checkpoint 才启动 tool-free verifier，approved/rejected/error 统一推进、重试或失败，续跑仍复用现有 Thread worker。 |
| 6. Goal/settings notifications | 已完成 | Goal 与 settings 各自只有一个 App Server broadcast source；`thread/settings/updated` 与 mutation 使用同一 `stateRevision`，Goal turn 的 `goalId + turnId + checkpointSeq` 负责 stale-result 防护。 |
| 7. 旧手工 Goal API 退役 | 已完成 | 旧 `workflow/goal/*` constants、DTO、handlers、Local client facade 和 frontend re-export 已删除；`workflow/state` 保留为只读 aggregate，Goal 写操作只有 `thread/goal/*`。 |

## 2. 目标边界

目标调用路径如下：

```text
App Server client
  -> thread/goal/set | get | clear
    -> Runtime Actor / GoalRuntime
      -> HostWorkflowStore (durable goal state and bounded artifacts)
    -> Core Thread (one ordinary turn at a time)
      -> tool-free verifier (after a settled checkpoint)
      -> state transition + next turn scheduling
      -> thread/goal/updated + turn/event
```

职责必须保持单一：

- Core 继续拥有 Thread、turn loop、模型输入、工具结果、事件和 Session conversation writeback；Core 不认识 GoalRuntime。
- Host `HostWorkflowStore` 继续拥有 `goal/state.json`、`goal/plan.md` 和 verifier artifact 的受限持久化，不负责调度下一轮 turn。
- App Server 内部 `GoalRuntime` 拥有 Goal 生命周期、verifier 调用、milestone advance/retry、continuation scheduling 和通知时序。它是 Runtime Actor 内的一个串行状态组件，不是另一个独立执行线程或第二个 Runtime；durable state、public set/get/clear、Goal/settings notifications、settled checkpoint verifier、resume/clear stale-result guard 和自动续跑均已收回。
- JSON-RPC / local client 只负责 decode、调用 canonical GoalRuntime action 和投影 DTO；不在 transport 层做 advance、retry 或重复通知。
- Host/Capabilities 继续决定审批、sandbox 和工具实际副作用；GoalRuntime 不复制 ToolRouter、ToolOrchestrator 或 approval authority。

非目标：

- 不恢复 `workflow/plan/set`；
- 不把 Goal loop 放进 Core；
- 不让客户端提交任意 verifier prompt、system prompt 或未 bounded 的 continuation policy；
- 不允许手工 `advance` 与自动续跑同时修改同一个 Goal；
- 不在没有可测量预算余量或公共边界证据时扩展 GoalRuntime。

## 3. Public API 目标形状

### 3.1 Goal lifecycle

新增 v2 方法，使用现有 `ActionResult<T>` 和 action revision 语义：

```text
thread/goal/set
  request:  { threadId, objective }
  response: ActionResult<ThreadGoal>
  effect:   create/replace a durable Goal and schedule its first ordinary Thread Turn

thread/goal/get
  request:  { threadId }
  response: ActionResult<ThreadGoal | null>
  effect:   read only; never schedules work

thread/goal/clear
  request:  { threadId }
  response: ActionResult<{ cleared: boolean }>
  effect:   stop at a safe checkpoint, persist terminal/cleared state, and remove
            the active Goal association; never silently abandon a running turn
```

具体字段仍以 Rust/TypeScript schema 生成结果为准，所有 client-to-server optional field 遵守 v2 `#[ts(optional = nullable)]` 规则。`objective`、verifier summary 和状态投影必须沿用现有 bounded limits；不新增 raw plan 或 raw prompt 字段。

`set` 的并发语义先固定为：

- 没有 active Goal：创建并启动第一个 milestone；
- Goal 为 `UserPaused`、`Failed` 或 `Converged`：显式 set 才创建新的 Goal ID；
- Goal 正在 `Running`、`Verifying` 或等待下一轮：返回 deterministic conflict，不隐式替换；
- 需要替换目标时先 `clear`，再 `set`，使旧 Goal 的 durable 终态和通知可审计。

`get` 是纯查询，不能触发 verifier 或续跑。`clear` 必须经过 Runtime Actor 的 safe-checkpoint 顺序，不能直接删除 `state.json` 来绕过正在运行的 turn。

### 3.2 Settings and Goal notifications

新增通知只在 Runtime Actor 完成 durable mutation 和 `stateRevision` 更新后发布：

```text
thread/settings/updated
  { threadId, collaborationMode, stateRevision }

thread/goal/updated
  { threadId, goal, reason, stateRevision, turnId? }
```

`goal` 为 nullable，以表达 clear；`reason` 使用有限的 string enum，例如 `set`、`restored`、`turnStarted`、`verifying`、`advanced`、`retrying`、`paused`、`failed`、`converged`、`cleared`。通知 payload 不复制完整 plan、verifier prompt、tool arguments 或 Session history。

通知时序固定为：

```text
validate command
  -> Runtime Actor mutation
    -> Core turn/checkpoint settles (if applicable)
      -> Host durable state write
        -> stateRevision/action receipt advances
          -> one server-owned notification
            -> command response / next scheduled action
```

同一状态变化只能由 App Server event bus 发布一次；JSON-RPC transport、local client 和 SDK projection 不得各自再发一遍。若通知发布失败，持久化状态不能回滚；下次 `get` 或 resume 必须能恢复事实状态。

## 4. GoalRuntime 设计

### 4.1 内部状态机

```text
Idle
  --set--> Running(milestone)
  --restore--> Running | UserPaused | Failed | Converged

Running
  --start turn--> AwaitingTurn
  --clear / pause--> Cancelling
  --timeout / interrupt--> Failed

AwaitingTurn
  --settled--> Verifying
  --cancelled--> Failed or UserPaused (deterministic command reason)

Verifying
  --approved--> Advance
  --rejected / needs_clarification--> Retrying
  --verifier error / timeout--> Failed

Advance
  --more milestones--> Running(next milestone)
  --last milestone--> Converged

Retrying
  --loop budget available--> Running(same milestone)
  --loop budget exhausted--> Failed

Cancelling
  --safe checkpoint--> UserPaused or cleared terminal state
```

状态转换必须由一个 Runtime Actor 串行化。`turn/start`、`turn/interrupt`、Goal set/clear、verifier result 和 resume race 时，以 action revision 和明确的 command outcome 判定，不以 wall-clock 先后猜测优先级。

### 4.2 自动续跑

每个 milestone 只允许一个 Core turn：

1. `set` 持久化 Goal 初始状态并发出 `set` 通知；
2. GoalRuntime 通过现有 App Server turn control 启动第一轮；
3. 监听 settled `turn/event` / checkpoint，不在 turn 尚未 settle 时调用 verifier；
4. 用现有 tool-free verifier 对 bounded checkpoint 做验证；
5. 持久化 verdict 和状态转换，再发布 `verifying`、`advanced`、`retrying` 或终态通知；
6. 只有状态已持久化并且没有并发 clear/pause 时，才调度下一轮。

verifier、advance 和 continuation 由 GoalRuntime 统一编排，但 verifier 仍使用隔离的 tool-free provider/Thread；不得把 verifier 的模型输入或输出写回主 Thread 的用户历史。Goal prompt 继续使用 Host 的 bounded `goal_turn_prompt`，只注入 objective、当前 milestone 和有限的上轮 verdict artifact。

### 4.3 Resume、clear 和失败

- resume 先从 `goal/state.json` 恢复 GoalRuntime 状态，再决定是继续当前 milestone、等待已存在 checkpoint，还是进入 terminal state；不得因为重新绑定 Runtime 而重复执行已经 settled 的 turn；
- clear 若有运行中的 turn，按现有 Runtime Actor busy/safe-checkpoint 规则拒绝并保留当前状态；空闲但 verifier pending 时先清理 pending association，再移除 durable Goal；
- verifier rejected 只在 loop budget 未耗尽时 retry 当前 milestone；不得自动推进 milestone；
- timeout/cancel/worker disconnect 必须保留非空、bounded 的失败原因，并使下一轮模型输入（若存在）看到结构化 Tool/Goal result，而不是空字符串；
- 所有 terminal state 都必须幂等，重复 get/clear/resume 不得再次调用 verifier、advance 或工具；迟到 verifier 结果必须被 `goalId`、`turnId` 和 settled checkpoint 校验丢弃。

## 5. 分阶段落地顺序

本计划启动时全 Rust 余量只有 `248` 行；该前置条件已由后续 P1 清理批次释放。每个实现批次控制在几百行以内，并记录 runtime 与 all-Rust 两个 delta。

### Batch 0：预算与冻结检查

- 确认 `python scripts/line_budget.py` 基线；
- 继续禁止 Core、Actor/CAS/Session 测试删除；
- 先完成能明确证明公共路径仍覆盖的 P1 test/wrapper 清理；
- 不添加 `thread/goal/*` 常量、DTO 或 handler，避免先出现未接线的公共 API。

### Batch 1：Goal canonical action 与 public contract — 已完成

- 在 app-server protocol v2 增加 `thread/goal/set/get/clear` 的 params/result/notification DTO；
- 在 App Server 增加 typed internal Goal action，并由 `GoalRuntime` 复用 Host store；
- local client 与 JSON-RPC 共用同一 action；
- 旧 `workflow/goal/*` 在本阶段曾作为迁移面，现已由 Batch 5 移除；
- public tests 已覆盖 set/get/clear、无 Goal get、running Goal 的 deterministic conflict、无 Host path 泄露。

本批实际变更：新增 `ThreadGoal`、`ThreadGoalSet/Get/Clear*` DTO 与方法常量；Host Goal state 增加 bounded objective、token budget 和时间字段，并为旧 state 提供 serde 默认值；Runtime Actor 内新增 `GoalRuntime` owner。运行时自动首轮、verifier、continuation 和通知不在本批宣称完成。

预期只允许小幅净增；如果 protocol + handler 超过当前预算，应先拆出 offset commit，不得压缩 DTO 或绕过 schema。

### Batch 2：抽出 serialized GoalRuntime — 首轮/通知接缝已完成

- 新增 app-server 内部 `goal_runtime.rs`，不新建独立 crate/线程；
- 将 durable Goal state、public set/get/clear action 和通知出口收回一个
  serialized GoalRuntime owner；Host 只保留文件/状态原语；
- `thread/goal/set` 已通过既有 worker 的 `StartIfIdle` 提交首轮普通 Turn；
  不新建第二个 turn loop；
- Goal update/clear 通过单一 App Server notification source 发布，local client
  的通用 notification 入口与 `next_event()` 的 turn-event 过滤已接通；
- verifier invocation、advance/retry、settled continuation、loop budget 和
  verifier failure 后续已由 Batch 3 收回 GoalRuntime；旧 manual path 已由
  Batch 5 退役。

### Batch 3：自动续跑与恢复 — 已完成

- 在现有 turn settled/checkpoint path 上接 GoalRuntime，不新增 Core loop；
- 实现 verifier 后下一轮、resume 不重放、clear/interrupt safe checkpoint；
- 明确 turn/interrupt 与 Goal clear、steer、follow-up 的确定性冲突结果；
- 加入 bounded scenario：多 milestone、verifier reject/retry、timeout、cancel、restart/resume；
- 测试必须断言 durable state、下一轮 model input 中的 bounded Goal result 和 turn/event 顺序。

### Batch 4：settings/Goal notifications — 已完成

- 在 Runtime Actor 的唯一 mutation commit 点发布
  `thread/settings/updated`，并保留 Goal notification 的 `turnId` settled-turn
  关联；首轮 `thread/goal/updated|cleared` 已完成；
- 验证 notification 只发一次、先于后续 continuation、且 stateRevision 与 action response 一致；
- clear 的 nullable goal、resume 的 settled checkpoint 分支、verifier/advance 的
  `turnId` 关联都要有 public JSON-RPC evidence；
- local client、stdio JSON-RPC 和 SDK projection 使用同一 notification model，不增加 transport-specific state machine。

### Batch 5：退役旧手工 Goal API — 已完成

- 在一个 breaking batch 中移除旧 protocol constants、DTO、handlers、client facade 和重复测试；
- 将 verifier、advance、retry 和 failure 统一收回 GoalRuntime，客户端不再提交 verdict/advance；
- 旧方法现在收到 deterministic method-not-found，且不改变 `thread/goal/*` 的状态或通知语义。

`workflow/state` 可暂时保留为聚合只读视图；是否最终退役另行记录，不与本计划混合。

### Batch 6：接入 Goal step、timeout、token budget 执行 — 已完成

本批仍复用唯一的 Core/App Server turn 生命周期：

- `milestone_step_budget` 在 Goal turn 开始时临时映射为 Core
  `HarnessConfig.max_steps`，普通客户端 turn 的配置不变；Core 的
  `StepLimit` 以 `usageLimited` 结束 Goal。
- `milestone_timeout_secs` 由 App Server worker 在同一 turn future 外部计时；
  到期只调用现有 `RunControl::request_cancel()`，等待 Core 在模型步/完整工具
  batch 的安全边界结算。同步工具副作用不会被异步强杀，最终以
  `usageLimited` 和 bounded reason 持久化。
- `ModelResponded` 与 `ContextCompactionFinished` 的 provider usage 在
  `BroadcastSink` 汇总为 input + output tokens，写入 `GoalState.tokens_used`。
  达到正的 `token_budget` 后持久化 `BudgetLimited`，公开映射为
  `budgetLimited`，不再启动下一次 milestone/verifier。
- Goal state schema 升为 3；`tokens_used` 对旧 state 使用 serde 默认值 0。
  非正 token budget 在 Host 创建边界拒绝；没有 provider usage metadata 时不
  进行猜测性估算。

六项准入记录：

1. Layer：Core 仅提供既有 `max_steps`、`RunControl` 与 usage events；执行
   决策和持久化在 App Server worker/GoalRuntime/Host store。
2. Duplicate responsibility：没有新增 Goal loop、timer thread 或 verifier
   loop；普通 `turn/start` 仍沿用原 worker path。
3. Replace vs add：替换此前“step/timeout 只持久化、不执行”的空档；新增
   Goal limit 状态和 durable usage counter，不复制 Session authority。
4. Net line delta：本批保持在几百行内，运行受影响 crate 测试、Clippy 和
   line-budget 后记录实际数值。
5. Visible surface：新增 `tokensUsed` 的真实投影、`usageLimited`/
   `budgetLimited` 状态及 bounded `last_error`；Goal turn 的既有事件顺序和
   checkpoint 写回保留，未引入无界输入。
6. Boundary evidence：App Server public JSON-RPC tests 覆盖 Core step limit、
   cooperative timeout 和 provider usage/token budget；普通 turn 与已有
   verifier/checkpoint tests 继续通过。

### Batch 7：补齐 verifier fault-injection 与 Goal 通知证据 — 已完成

- `goal_runtime_tests.rs` 直接注入 bounded verifier result，覆盖 rejected
  不推进 milestone、verifier execution error 持久化为 failed，以及清理
  pending association 后迟到结果被丢弃；不增加生产 provider 或第二个
  verifier loop。
- App Server public JSON-RPC Goal 场景现在记录并断言同一 `turnId` 下的
  `active -> blocked` Goal notification 顺序。Core `turn/event` 与 Goal
  notification 之间仍没有共享的 wire sequence，因此跨流全序不在本批
  声称已证明。

六项准入记录：

1. Layer：测试只位于 App Server GoalRuntime 与 JSON-RPC 公共边界；生产
   Core、Host、protocol 和 verifier provider wiring 不变。
2. Duplicate responsibility：复用现有 `complete_verification`、Goal event
   bus 和 `next_notification`，没有新增 verifier、通知或状态 owner。
3. Replace vs add：把此前“有实现但缺少 rejected/error evidence”的空档补成
   可回归证据；不恢复手工 Goal API，也不新增 restart 专用路径。
4. Net line delta：仅增加小批测试和文档；以本提交后的 line-budget 报告为
   准，runtime 仍需保持在 20,000 行硬门禁内。
5. Visible surface：只验证既有 bounded Goal status、`turnId` 和 last error；
   不新增模型上下文、持久化字段或公共协议字段。
6. Boundary evidence：覆盖 rejected、verifier error、late result、公开
   preparation failure、timeout/limit，以及 Goal notification 的同流顺序；
   provider-backed verifier、跨流全序和 restart/resume 的专门 public case
   继续作为后续证据项。

### Batch 8：restart/resume settled Goal 公共证据 — 已完成

- App Server public JSON-RPC 测试先写入一个 `active_turn_settled` 的 durable
  Goal，再通过 fresh connection/rebind 恢复 Runtime；收到的 blocked 通知仍
  关联原 `turn-1`，且 test model 调用计数为 0，证明没有重放主 turn。
- 该测试复用现有 Runtime Actor `InstallRuntime -> resume_goal` 路径；没有
  增加 restart 专用状态、第二个 turn loop 或新的协议字段。由于测试没有
  配置真实 verifier，它验证的是恢复分支和 preparation failure 的公共行为，
  不是 provider 质量。

六项准入记录：

1. Layer：测试位于 App Server public JSON-RPC/runtime rebind 边界；生产
   resume 路径不变。
2. Duplicate responsibility：复用现有 durable Host Goal state、
   `resume_goal` 和 `InstallRuntime`，没有新增恢复协调器。
3. Replace vs add：补齐原计划缺失的 restart/resume public evidence；不增加
   兼容 API、手工 advance 或新的 Goal owner。
4. Net line delta：本批增加 81 行测试 Rust，runtime 从 `19,454` 到
   `19,535`，仍低于 20,000 行硬门禁。
5. Visible surface：只验证原有 `blocked`、`turnId` 和 durable status；不新增
   模型可见内容、事件类型、持久化字段或公共协议字段。
6. Boundary evidence：fresh rebind、settled resume、preparation failure 和
   no-main-turn-replay 均有公共证据；跨流全序和真实 provider verifier 仍开放。

### Batch 9：统一跨流通知、ThreadItem 生命周期和 verifier 上下文边界 — 已完成

- Core `turn/event`、Goal 更新和 settings 更新现在进入同一个 App Server
  runtime notification bus；JSON-RPC connection 与 stdio transport 不再用
  `tokio::select!` 在多个 ready stream 之间竞争。Goal/settings 的既有内部
  broadcast 仍保留给 Runtime Actor 所有权边界，公共消费只使用统一流。
- `ToolFinished` 保留原始 `callId` 对应的 arguments，ThreadItem 对参数做
  深度、条目数、字符串和总字节限制，并对敏感 key 脱敏。started/completed
  Item 可用同一 `callId` 合并；不新增 Item history、list API 或 Artifact API。
- Goal verifier 只恢复最近 24 条 settled message，再由 Core 的现有字节级
  context guard 做最终限制；裁剪结果不会写回主 Thread Session。

六项准入记录：

1. Layer：App Server notification/Item projection、Core completion event 和
   Goal verifier input adapter；不移动 Core、Host 或 Session authority。
2. Duplicate responsibility：统一替换 transport 层多流 select merge，复用
   现有 Event/Goal/settings source、ThreadItem projection 和 Core context guard。
3. Replace vs add：替换跨流 ready-race；仅补齐 completed ToolCall 参数投影、
   bounded/redacted arguments 和 verifier history window，不增加第二个存储或 loop。
4. Net line delta：runtime `19,535 → 19,799`（`+264`）；release Rust
   `28,701 → 28,965`（`+264`），实验 CLI/REPL 不计入 release 门禁。
5. Visible surface：公共 Item 参数 bounded/redacted，Goal/Core/settings 通知
   共享发送顺序；verifier 只接收最近 24 条历史；无新的 Item listing 或 Artifact
   公共协议。
6. Boundary evidence：App Server public test 覆盖 settings-before-Goal 顺序，
   Protocol test 覆盖 ToolCall 参数安全投影与 completed identity，verifier test
   覆盖历史窗口；Protocol 11、Core 33、App Server 46 个定向测试通过。

## 6. 每个实现批次的六项准入模板

```text
1. Layer:
   App Server protocol/Runtime Actor/GoalRuntime, Host store, or client projection;
   Core only participates through existing turn/checkpoint contracts.

2. Duplicate responsibility:
   List the current WorkflowService, RuntimeCommand, verifier, and JSON-RPC paths
   touched by the batch. Prove which one is canonical and which old path is removed
   or delegated.

3. Replace vs add:
   State whether the batch replaces manual criteria/record/advance wiring, replaces
   a duplicate notification path, or adds a genuinely missing lifecycle state.
   No second Goal store, turn loop, verifier thread, or approval authority.

4. Net line delta:
   Record runtime and all-Rust before/after values. Default to net-zero or name an
   explicit P1 offset. Keep each commit reviewable and below the repository change-size
   guidance.

5. Visible surface:
   List model-visible prompt fragments, turn/event notifications, persistence files,
   action/revision metadata, and public protocol changes. Every injected fragment is
   bounded; no raw prompt or unbounded plan is exposed.

6. Boundary evidence:
   Add/maintain App Server public JSON-RPC and local-client evidence for the changed
   behavior, plus deterministic timeout/cancel/steer/resume race coverage where relevant.
```

## 7. 完成判定与否决信号

完成判定：

1. `thread/goal/set/get/clear` 是唯一新的 canonical Goal lifecycle control；
2. 每个 milestone 至多一个主 Thread turn，verifier 在 settled checkpoint 后运行；
3. approved/rejected/timeout/cancel/resume 都有 durable state、bounded reason、turn/event 顺序证据；
4. `thread/goal/updated` 与 `thread/settings/updated` 由单一 App Server commit 点发布且不重复；
5. 旧 manual criteria/record/advance 已在 breaking batch 中消失，不能与自动续跑并发写入；
6. runtime 与 all-Rust hard ceilings 均通过，并留下可继续开发的显式余量；
7. README、CHANGELOG、Agent Notes、schema fixtures、SDK/cookbook（若 API 已公开）同步更新。

以下任一信号出现就暂停当前阶段并回到设计审计：

- `set` 返回 running 但没有可观察的首轮 turn；
- resume 重新执行已 settled 的 milestone；
- notification 先于 durable write，或同一状态变化出现两条通知；
- verifier output、plan 全文或 tool arguments 无界进入主 Thread/model context；
- manual `advance` 能绕过 verifier 或在 Goal clear 后继续调度；
- 为了凑行数删除 Core turn、Actor/CAS/Session 或 approval boundary evidence；
- 为了兼容而恢复已明确移除的 `workflow/plan/set`。

## 8. 下一步

Batch 1–9 的 Goal contract、serialized owner、settled-checkpoint verifier、
自动 continuation、settings notification、resume/clear stale-result guard、
旧手工 API 退役、step/timeout/token budget 执行，以及 rejected/error
fault-injection evidence、restart/resume public evidence、跨流通知顺序、
bounded ThreadItem lifecycle 和 verifier history bound 均已完成，并通过
Protocol、Core、App Server 定向测试。后续只补有明确边界的真实 provider/
跨平台证据、专用 Item notification/listing 和 Codex ThreadItem/Artifact
扩展，不再恢复第二个 Goal loop 或手工 advance API。
