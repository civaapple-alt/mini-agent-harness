# Codex-Aligned Thread and Goal Runtime Architecture

Status: proposed
Date: 2026-09-03
Scope: App Server、Host 和 Thread 运行时边界

## Proposal

mini-codex 将不再以 `WorkflowService` 作为 Plan、Goal 和 Runtime 的统一领域入口。公共请求按 Codex 的 Thread 边界拆分，Goal 行为按每个 Thread 拆分，Runtime Actor 只负责串行执行和状态提交。

目标结构：

```text
App Server
├── ThreadSettingsRequestProcessor
│   └── thread/settings/update
├── ThreadGoalRequestProcessor
│   └── thread/goal/set|get|clear
├── ThreadManager
│   └── ThreadHandle -> mini-agent-core::Thread
├── ThreadState / ThreadListener
│   └── 设置、Turn、Goal 事件排序
└── GoalService
    └── GoalRuntimeHandle（每个 Thread 一个）
        └── HostWorkflowStore（持久化 primitive）
```

这不是复制 Codex 的实现，而是采用同一组所有权边界：

```text
mini ThreadGoalRequestProcessor
    ≈ Codex ThreadGoalRequestProcessor

mini GoalService
    ≈ Codex GoalService

mini GoalRuntimeHandle
    ≈ Codex GoalRuntimeHandle

mini HostWorkflowStore
    ≈ Codex StateRuntime / thread_goals

mini ThreadManager + ThreadHandle
    ≈ Codex ThreadManager + CodexThread

mini ThreadState / ThreadListener
    ≈ Codex ThreadListenerCommand
```

## 所有权边界

### ThreadSettingsRequestProcessor

只负责协议解析和参数边界检查：

- 接收 `thread/settings/update`；
- 转换为有类型的 `ThreadSettingsUpdate`；
- 调用 ThreadState；
- 返回设置更新结果；
- 不处理 Goal 生命周期；
- 不直接替换原始 system prompt。

`collaborationMode` 和受限的 `builtinTools` 属于 Thread 设置。Plan 是一种 collaboration mode，不再建立独立的 Plan workflow 服务。

### ThreadGoalRequestProcessor

只负责 Goal 公共协议：

- `thread/goal/set`；
- `thread/goal/get`；
- `thread/goal/clear`；
- 参数验证和协议投影；
- 将结果和通知交给 ThreadListener 排序。

它不暴露 verifier criteria、verdict、advance 或 continuation 控制。

### GoalService

GoalService 是 Goal 的领域操作入口，负责：

- 创建、更新、暂停、恢复和清理 Goal；
- 校验 objective、token budget 和状态迁移；
- 以 `goal_id` 或等价的预期值保护并发更新；
- 写入持久 Goal 状态；
- 返回包含 previous/current 状态的 `GoalSetOutcome`；
- 在持久化成功后应用运行时效果。

GoalService 不实现第二套 Turn loop，也不负责具体工具执行。

### GoalRuntimeHandle

每个 Thread 拥有一个 GoalRuntimeHandle，负责 Goal 的运行时行为：

- 当前 active Goal 和 active turn；
- 已安排但尚未启动的 turn；
- settled checkpoint 后的 verifier preparation；
- verifier 结果应用；
- token accounting；
- `continue_if_idle`；
- active turn 上的 bounded steering；
- turn error、usage limit 和预算耗尽的状态投影。

它可以调用 ThreadHandle 启动普通 turn，但不创建新的执行循环。

### HostWorkflowStore

HostWorkflowStore 只作为持久化 primitive，保存 Goal workspace 中的状态、criteria、verifier 结果和 checkpoint 关联信息。

Goal 的业务规则应由 GoalService 和 GoalRuntimeHandle 决定。Store 不应成为 App Server 外部控制协议的入口，也不应自行启动 turn。

### ThreadManager 和 ThreadHandle

ThreadManager 管理 Thread 标识和查找，ThreadHandle 只提供 Turn 执行边界：

```text
start_turn
start_turn_if_idle
steer_running_turn
recover_turn_if_idle
```

它不判断 Goal 是否收敛、不读取 verifier criteria，也不决定是否继续 Goal。

mini-agent-core 的 Thread 和 Harness loop 保持现有职责：执行一次 Turn、产生事件、写回 conversation history。App Server 只通过 ThreadHandle 调用这些能力。

### ThreadState 和 ThreadListener

ThreadState 保存 Thread 级别的 live 状态，例如：

- 当前 settings snapshot；
- active collaboration mode；
- builtin tool selection；
- 当前 turn 和 checkpoint 关联；
- Runtime revision。

ThreadListener 是所有外部通知的排序边界。设置、Goal、Turn 和审批相关事件必须经过同一条 Thread 事件序列，避免不同 broadcast stream 产生不可确定的客户端观察顺序。

Runtime Actor 继续作为实现串行化的机制，但不再作为 Workflow 领域对象。Actor 负责接收命令、校验 revision、调用对应服务、提交状态并转发事件。

## 关键流程

### 设置 collaboration mode

```text
thread/settings/update
    → ThreadSettingsRequestProcessor
    → ThreadState.apply_settings()
    → ThreadHandle 应用到下一次 Turn
    → ThreadListener.emit(settings updated)
```

Plan 的稳定提示词仍由 Host 组装。App Server 只选择 allowlisted mode，并让 Thread 在安全边界应用 bounded overlay。

### 设置 Goal

```text
thread/goal/set
    → ThreadGoalRequestProcessor
    → GoalService.set_thread_goal()
    → HostWorkflowStore 持久化
    → GoalSetOutcome
    → GoalRuntimeHandle.apply_external_goal_set()
    → ThreadHandle.start_turn_if_idle()
    → ThreadListener.emit(goal updated)
```

当 active Goal 的 objective 在运行中的 Thread 上发生变化时，GoalRuntimeHandle 可以注入 bounded steering；当 Thread 空闲时，使用 `start_turn_if_idle` 启动下一轮。

### Goal turn 完成

```text
Thread turn settled
    → GoalRuntimeHandle.prepare_verification()
    → tool-free verifier
    → GoalRuntimeHandle.apply_verdict()
    → GoalService.advance / fail
    → ThreadHandle.start_turn_if_idle()
```

verifier 只消费受限的 checkpoint 和 bounded message window。`criteria`、`record_verdict` 和 `advance` 都是内部动作，不进入公共协议。

## Goal 状态语义

公共状态和 Host 内部状态保持明确投影：

| 公共状态 | Host 状态 | 运行时效果 |
|---|---|---|
| `active` | `Running` | 标记 active；空闲时继续启动 turn |
| `paused` | `UserPaused` | 停止 continuation，等待显式恢复 |
| `blocked` | `Failed` | 清除 active runtime，保留失败原因 |
| `usageLimited` | `UsageLimited` | 停止 Goal continuation |
| `budgetLimited` | `BudgetLimited` | 停止 Goal continuation |
| `complete` | `Converged` | 清除 active runtime |

状态规则：

- 新 Goal 必须从 `active` 开始；
- `active → paused` 和 `paused → active` 是明确的外部状态迁移；
- turn error 只能由内部运行时投影为 `blocked`；
- verifier 只能在 settled checkpoint 上运行；
- 旧 checkpoint 的 verifier 结果必须被丢弃；
- pause、clear、cancel 和 verifier 结果之间的竞态由 Runtime Actor 的确定性命令顺序解决。

## 公共协议

公共 Thread 面只保留：

```text
thread/settings/update
thread/settings/updated

thread/goal/set
thread/goal/get
thread/goal/clear
thread/goal/updated
thread/goal/cleared
```

`workflow/state` 如果仍需要，只作为只读聚合投影，不拥有任何状态，也不能触发 workflow 动作。

以下能力只能是内部方法：

```text
prepare_verification
record_verifier_result
advance_goal
fail_goal
continue_if_idle
```

## 实施顺序

### 第一阶段：替换 WorkflowService 边界

- 移除 `WorkflowService` 的 App Server 领域定位；
- 将 JSON-RPC 入口拆成 Thread Settings 和 Thread Goal 两个处理器；
- Runtime Actor 只接收 typed settings/goal command；
- 保持现有 Thread、Session、Approval 和 Tool 边界不变；
- 不增加新的公共方法。

### 第二阶段：提取 GoalService 和 GoalRuntimeHandle

- 从当前 GoalRuntime 提取 GoalService 的外部 Goal 操作；
- 将 verifier、continuation、accounting 和 active turn 留在 per-thread handle；
- 将 HostWorkflowStore 收敛为持久化操作；
- 完成 pause/resume 的真实状态迁移；
- 所有 Goal 更新返回明确 outcome。

### 第三阶段：收敛 Thread 执行边界

- 将当前 Thread worker 的启动、steer、recover 操作收敛到 ThreadHandle；
- 引入最小 ThreadManager 查找 Thread；
- 统一 ThreadListener 的事件排序；
- GoalRuntimeHandle 只能通过 ThreadHandle 调度普通 Turn。

### 第四阶段：删除 Workflow 领域概念

- Runtime Actor 中不再保留 workflow-specific 聚合分支；
- Plan 归入 Thread settings；
- Goal 归入 GoalService；
- verifier 和 continuation 归入 GoalRuntimeHandle；
- `workflow/state` 仅保留为只读状态投影。

## 不在本提案中的内容

- 不修改 Core Harness 的 turn loop；
- 不在 Core 引入 Goal、Plan、Store 或 Runtime Actor；
- 不建立通用 workflow framework、plugin framework、policy framework 或依赖注入容器；
- 不新增客户端可手动驱动的 verifier/advance API；
- 不复制 Codex 的 SQLite、analytics、metrics 和完整多线程基础设施；
- 不把 `WorkflowService` 扩展成更大的管理服务。

## 验收标准

1. 公共请求只通过 Thread settings 和 Thread Goal 边界进入运行时。
2. `WorkflowService` 不再拥有任何 Goal 或 Plan 领域行为。
3. Goal 的持久状态只有一个权威来源，运行时 active 状态只有一个 per-thread owner。
4. Goal continuation 复用 Thread 的普通 turn 入口，不存在第二套 loop。
5. 设置、Goal、Turn 和审批通知在同一个 Thread listener 序列中可确定地观察。
6. `paused`、`blocked`、`usageLimited`、`budgetLimited` 和 `complete` 都有明确的持久化和运行时效果。
7. 现有公共 boundary tests 覆盖 set/get/clear、settings、settled verifier、pause/resume、stale verdict 和通知顺序。
8. 新增代码默认保持 runtime 和 release source 的净增长为零；每个实现批次都必须先完成六项 change-admission 问题并运行受影响测试、Clippy、格式化和 line budget 检查。
