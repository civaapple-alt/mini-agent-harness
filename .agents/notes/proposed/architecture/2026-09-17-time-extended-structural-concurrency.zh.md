# 时间上延展、结构上并发：Agent Harness Charter

- status: proposed
- revision: 2
- confirmedAt:
- arrivedAt:

## Spark

当前项目后续的推进方向（2026-09-17 意图工作坊）。需要补齐三个相互关联、但不能混为一谈的能力：子 agent 委派、会话内记事本、后台长任务跟踪。

## Problem

当前 harness 主要建模“单个前台、短视界”的运行。Session、Thread、Plan 和 Goal 已经提供了持久化地基，但仍缺少：

- 父 agent 将有界子任务委派给 child agent，并接收结构化结果；
- 事实跨压缩、跨 Turn 保留的会话记事本；
- WebSocket 断开后仍可查询、取消和恢复判断的后台长任务。

因此真正缺少的不是一个更大的执行循环，而是两个正交维度：

- 时间：上下文和任务生命周期可以跨越当前 Turn、压缩和连接；
- 结构：父子 Session 可以在统一协议和权限边界内并发运行。

历史上曾实现过子进程 CLI 会话形式的 subagent 委派，但该路径已退出当前主线；更早的 Core 内多租户调度器方案也已否决。历史实现和否决理由只能作为边界证据，不能直接视为可复用实现。

## Desired Change

### 1. Child Session 委派

主 agent 可以提交一个有界的 `delegate_task` 请求。App Server 创建 child Session/runtime，child agent 使用与主 agent 相同的 Protocol、Host、Capabilities、Approval 和 Replay 契约，并返回有界结构化结果，不把完整 child 历史内联到父上下文。

子 agent 在 UI 中表现为 child Thread，在持久化和生命周期上是 child Session-backed runtime。第一阶段只允许树形父子关系、深度 1，不支持孙 agent。

### 2. 会话记事本

主会话获得 Session-owned 的有界记事本。记事本可以由 agent 写入、由用户查看，并跨压缩、Turn 切换和 runtime 重启存活。默认只注入受控摘要，完整内容通过显式的有界读取按需获取。

第一阶段不允许 child agent 写入主会话记事本，避免在并发写入策略尚未确定时引入共享状态。

### 3. 后台长任务

后台任务是可持久化的生命周期投影，而不是新的执行引擎。至少需要表达：`queued`、`running`、`awaiting_approval`、`completed`、`failed`、`cancelled`，并支持状态查询、取消、有限重试和断线重连后的恢复判断。

首批将后台任务限定为独立 Session/runtime 或其明确的 child execution；不把任意 shell 进程自动纳入该模型。

## Chosen Frame

采用“统一协议，同一权限边界，独立 Session/runtime”的模型：

```text
Parent Thread / Turn
  -> delegate_task(operation_id)
  -> App Server child-session control seam
      -> Child Session / Thread / Turn
      -> Child Host / Capabilities
  -> bounded result and status projection
  -> Parent Thread item / checkpoint
```

“单执行路径”特指统一的协议、授权、审批、事件和 replay 路径，不特指一个进程或一个共享调度器。Core 仍然只负责显式 Turn Loop、工具契约、限制和事件；不在 Core/Host 内引入多租户调度器，也不建立 Gateway 的第二状态机。

当前 `session/fork` 要求源 Session idle，不能直接覆盖“父 Turn 运行期间委派 child”的场景。因此需要新增明确的 child-session 生命周期控制 seam，而不是把现有 fork 语义隐式扩展为并发调度。

## Constraints

- 每个 Thread 继续最多一个 active Turn；并发通过独立的 child Session/runtime 表达。
- child 的工具访问最终由 Host/Capabilities 授权；Gateway 和前端不保存授权或执行状态副本。
- 父子关系、`operation_id`、child 状态和 bounded result 必须持久化或可重建，并通过 sequence/replay 对外观察。
- 新增消息、工具、结果和事件都必须有界；不将 child 完整历史、记事本全文或资源正文默认注入模型上下文。
- 首批建议 `depth = 1`、每个父任务最多两个 active children；具体上限必须进入协议和 Harness Scenario，而不是只写在 UI 中。
- 父取消、child 取消、审批等待、超时、失败和重试必须具有明确的级联语义和幂等规则。
- 每个批次继续回答 pull request 模板的六个 admission 问题，并补充 prompt/tool/schema/loop/context/events 变化的 Harness Scenario/Eval 证据。

## Non-goals

- 不恢复已删除的 subprocess CLI bypass、ACP 旁路或旧的 parent-side trace 实现。
- 不在 Core 中增加通用调度框架、多租户执行器或分布式集群。
- 不实现 P2P、跨机器或网络集群式 agent 拓扑。
- 不允许 child agent 在第一阶段递归委派或写入父会话记事本。
- 不把 Goal Runtime 直接改造成所有后台任务的通用调度器；可复用其持久化和状态投影经验，但保留 Goal 的里程碑与验证语义。
- 不默认注入全部 notebook、child history 或所有关联资源。

## Proposed Batches

1. **Batch 0：契约和证据 fixture**
   固定 lineage、operation、状态、取消、结果上限和 replay 形状；先用确定性 Harness fixture 验证，不调用付费 provider。
2. **Batch 1：后台任务观察面**
   建立任务 ID、生命周期投影、断线重连、取消和 replay；暂不开放模型自主委派。
3. **Batch 2：Session Notebook**
   实现有界写入、读取、压缩后恢复和用户可见投影；验证父/child 并发写入策略前，保持 child 只读。
4. **Batch 3：单 child 委派**
   支持父 Turn 创建一个 child Session、等待状态、接收 bounded result；限制深度 1 和并发数，不允许递归委派。
5. **Batch 4：组合场景**
   验证父 Turn、child Turn、notebook、后台任务和 WebSocket 重连的组合行为。

## Arrival Evidence

- 父 Turn 尚未结束时，两个 child 可以同时处于 `running`，并能用开始/结束时间证明实际重叠；
- child 使用独立历史和上下文边界，父 agent 只收到有界结果，不内联完整 child 历史；
- 父、child、operation 和状态在断线及 App Server/runtime 重启后仍可恢复，事件可按 cursor replay；
- child 等待审批、失败、取消、超时和重试都有可观察且幂等的结果；
- notebook 内容在压缩、Turn 切换和重启后保留，前端看到的是持久投影而不是 Gateway 缓存；
- 确定性 Harness Scenario 强制父 agent 写入 notebook、委派 child 并启动后台任务；
- 全部实现批次保持 line budget 在 green/amber 范围内，不进入 red band。

## Falsification Signals

- 任一批次要求 Core/Host 引入共享多租户调度器或第二执行路径；
- Gateway/frontend 出现 parent、child 或 workflow 的影子历史和授权状态；
- child 或后台任务事件不能 replay，或结果、历史、记事本注入无上限；
- 父子取消、审批、重试或重启恢复语义无法形成确定性测试；
- 无法证明两个 child 的实际时间重叠；
- line budget 进入 red band；
- 仅靠 UI 显示“后台运行”，但 SessionStore 中没有权威状态。

## Historical Evidence

- 当前 [docs/app-server.md](../../../../docs/app-server.md) 规定了单 Thread active Turn、idle-only session fork、事件 replay 和 Session 持久化边界。
- [mini-agent-core/src/thread.rs](../../../../crates/mini-agent-core/src/thread.rs) 的 Turn Loop 仍以单 Thread 的 active Turn 为边界。
- [mini-agent-app-server/src/goal_runtime.rs](../../../../crates/mini-agent-app-server/src/goal_runtime.rs) 将 Goal Runtime 定义为生命周期和验证决策所有者，而不是第二条 Turn Loop。
- 2026-08-27 的历史记录显示，子 Session 方案曾实现，Core 内多租户调度器方案被否决；这些记录支持“复用 Session 边界、避免 Core scheduler”的约束，但不证明并发委派已经存在。

## Open Questions

- 第一版采用 `delegate_task` + `task_read/wait`，还是显式的 `spawn/wait/list/cancel` 工具组？建议先采用最小的创建、读取/等待接口。
- child 是由同一 App Server 控制的独立 runtime，还是由本地 runtime factory 启动的独立 App Server client？需要在不引入 Gateway 影子状态的前提下确定控制 seam。
- notebook 的完整内容采用显式工具读取，还是每 Turn 注入有限摘要？建议摘要优先、正文按需读取。
- 后台任务的第一种具体执行类型是 child agent Turn，还是 detached Goal continuation？建议先实现一种，避免抽象出无语义的通用 scheduler。
