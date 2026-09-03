# Harness 框架比较与分层结论

Date: 2026-08-31
Status: implemented, frozen topic note
Source: [The Coding Harness Behind GitHub Copilot in VS Code](https://code.visualstudio.com/blogs/2026/05/15/agent-harnesses-github-copilot-vscode)

## 结论

模型只是引擎，harness 才是把上下文、工具、执行循环和结果反馈组织成产品
体验的系统。harness 至少负责：

1. 组装模型可见上下文；
2. 声明当前允许的工具能力；
3. 校验并执行工具调用；
4. 决定继续、停止、取消或进入下一轮。

一次用户可见的 `turn` 可以包含多个内部 `round`/`step`。每一轮都重新组装
有界上下文，执行工具并判断是否继续。工具数量、取消、stop hook、上下文
压缩和持久化结果共同构成 loop-control，而不是交给模型自行决定。

VS Code 的经验是设计假设和验证方法，不是 mini-agent-harness 的功能对齐
目标。下一迭代应投资可观察、可评估、可取消的边界，而不是用 provider、工具
或配置数量代替成熟度。

## 定位与分层

两者共享的最小闭环是：

```text
用户输入 → 构造上下文 → 模型生成 → 判断工具调用
         → 执行工具并写回结果 → 再次请求模型 → 最终回复或停止
```

```text
mini-agent-harness = model + bounded tool loop
Codex native        = durable Session/Task/Turn/Item + tool/event runtime
```

当前 mini-codex 的分层边界：

```text
CLI / 客户端
    ↓
App Server（Actor、CAS/revision、事件与控制面）
    ↓
Host（runtime/profile/workflow 组合）
    ↓
Capabilities（provider、workspace、process、sandbox、MCP、approval）
    ↓
Core（Thread、Harness、Turn/Step、limits、control）
    ↓
Protocol（消息、工具、事件、停止原因和限制契约）
```

| 概念 | mini-agent-harness | Codex 原生框架 |
| :--- | :--- | :--- |
| 会话 | `Thread`、`SessionState` 与 settled checkpoint | 持久化 `CodexThread`、`Session` 与 rollout |
| 工作单元 | `Thread::run_turn`；Turn 内多个 Core Step | `RegularTask` 驱动的持久化 Turn |
| 模型步骤 | `Model::respond` 后执行有界工具 batch | Responses stream、多个 output Item 与异步工具任务 |
| 工具路由 | Capabilities 执行，Host/App Server 组合 policy、MCP、approval | `ToolRouter`、沙箱、审批、MCP 与 `StepContext` 协同 |
| 上下文 | bounded `Message`、压缩和 UTF-8 截断 | Turn/Step context、response item、rollout 与模型相关压缩 |
| 事件 | 稳定的 `Event`/`EventEnvelope` | Turn/Item lifecycle、delta、diff、MCP 和 raw response 事件 |
| 持久化 | settled checkpoint、Session JSONL、Result Store handle | thread store、rollout、response item 与恢复/分叉 |
| 控制 | safe checkpoint 上的 cancel、steer、follow-up | cancellation token、interrupt、mailbox 和 task 生命周期 |

## Turn/Step 与控制边界

mini 的一轮流程是：

```text
turn/start
  ↓
App Server Actor 分配 identity/revision
  ↓
Thread.begin_turn → Harness.run_with_control_mode
  ↓
追加 User Message，检查/压缩 context
  ↓
Model.respond
  ├─ 无工具调用 → 最终回复并结束
  └─ 有工具调用 → 校验数量 → 完整执行 bounded batch
                         ↓
                    追加 Tool Message / result handle
                         ↓
                    下一次 Model.respond
  ↓
TurnFinished → settled checkpoint / Session 持久化
```

响应和工具调用先验证，失败时不执行副作用；工具结果进入模型上下文前截断；
cancel 和 steer 只在模型步骤之间或完整工具 batch 后观察。App Server 的默认
外部语义是安全停止当前工作单元、持久化 checkpoint，再启动 queued steer 或
follow-up；一次 `turn/start` 仍可能连续 settle 多个 Core Turn。

Codex 原生通常把 `turn/steer` 放入 Session input queue/mailbox，在同一个外部
Turn 内继续下一次 sampling。两者不是正确性高低差异，而是边界选择：mini
优先显式 checkpoint 和确定性测试，原生框架优先长任务的连续工作语义。

## 成熟度判断

| 维度 | mini-agent-harness 当前优势 | Codex 原生框架的优势与代价 |
| :--- | :--- | :--- |
| 执行内核 | Loop、Step、limits、stop 分类和安全检查点显式，容易做确定性 fixture | 生命周期覆盖完整，但 Task、Item、stream 和异步工具协调更复杂 |
| 状态与恢复 | App Server Actor/CAS、Session settled checkpoint 和单一路径边界清楚 | 持久化粒度、恢复/分叉和长任务能力更丰富，状态面更大 |
| 能力面 | Provider、workspace、sandbox、MCP、approval 在 Core 外组合 | 工具、hooks、skills、MCP、沙箱和环境集成更成熟，兼容矩阵更宽 |
| 可验证性 | bounded 输入/输出、被动事件和本地 mock 场景便于隔离验证 | 更接近生产工作流，需要更大的跨平台、真实 provider 和长期运行证据 |
| 主要风险 | 追求小而漏掉真实 provider、平台和安全策略证据 | 功能面扩大后增加隐式状态、异步竞态和上下文成本 |

因此，mini 不复制原生 Codex 的全部对象或工具生态；继续保持
`CLI → App Server → Host → Core` 主路径，用 bounded scenario 验证每次变更
对 Turn、Tool、Context、State 和 Boundary 的影响。只有真实场景和证据成立，
才扩大 provider、retry 或 Docker policy。

## 维护规则

本主题的原始合并记录见
[`2026-08-31-harness-lessons-archive.md`](2026-08-31-harness-lessons-archive.md)。
新的框架比较应创建新日期 note，不能修改本文件的历史结论。
