# 薄 Agent Loop、厚 Control Plane：把 Harness 做成可交付的系统工程

Status: proposed  
Date: 2026-09-08  
Scope: `mini-agent` 的 Core、Protocol、Capabilities、Host、App Server，以及通过 Python SDK、FastAPI Gateway 和 Web Studio 使用它们的客户端边界

## 提案摘要

本提案建议把 Agent Harness 明确为一个系统工程问题：模型和模型框架是可替换的执行引擎，真正需要长期稳定的是 Control Plane 对状态、权限与沙箱、恢复、验证和审计的边界。

目标形态是：

```text
Outcome / Goal / Boundary / Invariant
                ↓
        Control Plane
  State · Permission · Recovery
  Verification · Audit · Control
                ↓
       Thin Agent Loop
  Model Step · Tool Batch · Limits
                ↓
        Model + Capabilities
```

这不是把 Agent 变成更复杂的 Planner，也不是把数据库系统的术语机械搬进代码。它将复用数据库团队已经验证过的工程方法：可重复测试、故障注入、状态验证、检查点、恢复、补偿、审计和明确的失败边界。

第一阶段不会新增 Planner、Scheduler、Memory、Plugin 或 Policy Framework，也不会把这些概念塞进 Core。优先使用现有的 `Thread`、`Turn`、`Goal`、`Session`、`ThreadItem`、Approval、World 和 observation events，验证这些边界是否足以支撑可交付结果。

## 实施进度（2026-09-08）

Batch 1 已完成第一条控制面不变量切片：active Goal 的 Loop 配置只由 Goal
Runtime 持有。App Server 公共 `thread/settings/update` 在 Goal 为
`Running` 时拒绝 `continuationMode` 改写；Gateway 启动恢复会延后已保存的
`continuous` 偏好，并在 Goal 结束后恢复，普通工具设置不会再把该偏好覆盖成
`manual`。新增 Rust 公共 JSON-RPC 场景和 Gateway 单测，现有 50 个 App Server
库测试全部通过，新增切片使 runtime/release Rust 基线从 `17,674/27,094`
增至 `17,736/27,156`（`+62/+62`，仍为 green）。

随后补充 AC-02 的高风险反例：Capabilities 测试验证 `trusted + project` 下的
普通 patch 可以直接准入，但删除操作和 `shell` 的 `git reset --hard HEAD` 仍然
要求 approval；该证据只增加 `+0/+10` release lines，当前基线为
`17,736/27,166`，仍为 green。

Batch 3 已完成 canonical continuation persistence 的第一条切片：SessionStore
以带 `thread_id` 的原子 `thread_settings.json` sidecar 保存显式
`manual`/`continuous` 偏好，App Server 在 runtime bind 时恢复它；Gateway 的
continuation shadow cache 已删除，SessionCatalog 只读该 bounded projection。
Capabilities resume、App Server idle shutdown/restart、Gateway catalog、Goal
ownership、partial tool batch 与普通设置不覆盖偏好的测试均通过。为完成这条证据，App Server
增加了显式 idle-only `shutdown()` seam；活动 turn 不允许直接 shutdown，必须先
按既有 cancel/settlement 路径收敛。本切片相对 `24ae42e` 增加 runtime/release
`+98/+98`，没有修改 Cargo manifest，预算仍为 green。

同一 App Server 后端的两个连接 subscriber 也已验证收到同一个
`thread/settings/updated` `stateRevision`；这证明控制面事件不会因连接投影而
分叉。跨 SDK/Gateway/Web 的端到端 revision 收敛，以及跨 Project/Thread attach
的 fork/并发组合仍需单独覆盖，不能由单连接测试代替。

Gateway 的显式 Project attach 选择也已补证：在没有现有本地绑定时，canonical
Session lookup、resume 参数和返回的 Project ID 保持一致；已有 live binding 的
同 ID 冲突现在 fail closed 为 `409`，不会静默复用错误 workspace。未授权的跨
Project 隐式猜测、fork/并发组合和跨协议 revision 收敛继续留在后续矩阵中。

随后补充的 active-turn shutdown guard 与双 subscriber revision 场景相对
`2327e8f` 增加 runtime/release `+26/+26`；当前累计基线为
`17,942/27,426`，仍处于 green。

这不是 Batch 1 的全部故障矩阵；approval denial、timeout、MCP refusal、Goal
恢复和 revision 的既有证据仍需在同一报告中统一记录；跨客户端 revision 收敛与
跨 Project/Thread attach 仍是后续工作。

## 背景与当前证据

当前 `mini-agent` 已经形成了一个适合验证 Harness 假设的最小闭环：

| 层 | 当前责任 |
| --- | --- |
| Core | 有界上下文、Model/Tool step、Compaction、限制、Stop classification、控制检查点和历史写回 |
| Protocol | Model、Tool、Message、Event、Approval 和 Stop 等可移植契约 |
| Capabilities | Provider、Workspace、Process、Sandbox、MCP、工具准入和具体副作用 |
| Host | Prompt、Rule、World、Workflow、ToolOrchestrator 和 Runtime composition |
| App Server | Thread、Turn、Goal、Actor/CAS、revision、事件、Session 和 JSON-RPC 控制面 |
| SDK/Gateway/Studio | 进程连接、协议投影、HTTP/WebSocket、用户操作和只读状态展示 |

最近的 `approval_policy` 与 `continuation` 实践说明了问题的本质。它们看似只是控制屏上的两个选择，实际分别改变工具准入和 Agent loop 的继续方式，因此需要经过协议、App Server runtime、Capabilities、SDK、Gateway、Session 恢复和 UI。这是跨边界契约的证据，不是应该由 Core 吞下更多职责的理由。

当前基线如下：

```text
Core:              3,317 effective lines
Protocol:            769 effective lines
Capabilities:      9,484 effective lines
Host:              3,012 effective lines
App Server:       10,910 effective lines
Control Plane:    18,527 effective lines
Runtime:           18,008 / 20,000
Release Rust:     27,492 / 30,000
```

`python scripts/cargo_boundary.py --json` 当前通过；唯一显式 review edge 是 `mini-agent-app-server → mini-agent-capabilities`，原因是 App Server 仍参与 Provider、Session 和 Approval 的 runtime assembly。两个相关提交没有修改 Cargo manifest，因此本提案不会为了减少文件数而改变依赖方向。

## Harness 假设

### H1：模型能力增强会减少显式编排，但不会减少控制面责任

未来模型可以在给定 Goal、Boundary 和 Invariant 后自行选择局部步骤。Harness 仍必须独立决定工具是否准入、何时停止、怎样恢复、如何验证结果，以及哪些证据可以交付给人。

可证伪条件：如果模型必须依赖固定的逐步 Planner 才能在边界内完成任务，或者模型可以通过改变提示词绕过状态、权限或恢复规则，则 H1 不成立。

### H2：可交付性主要来自稳定边界，而不是能力清单

Planner、Coder、MCP、模型数量和工具数量不是成熟度本身。成熟度应表现为：结果有明确验收，副作用可追踪，失败可分类，运行可恢复，权限不会因组合方式而意外扩大。

可证伪条件：增加某个能力后，若不增加新的编排代码，仅依靠既有 Control Plane 就能稳定完成相同场景，则“能力数量决定 Harness 差距”的判断不成立。

### H3：数据库式工程证据适合 Agent Harness

Scenario、故障注入、验证器、检查点、补偿和审计可以区分“模型偶然完成”与“系统在失败和重启后仍可交付”。这些机制应首先作为 Host/App Server 的边界测试与观察证据，而不是 Core 中新的通用框架。

可证伪条件：同一场景在超时、拒绝、revision 冲突、进程锁和部分副作用后无法得到可解释的状态，且现有边界无法增加有界故障注入 seam，则需要先重新审视当前分层。

## 关键设计原则

### 1. 薄 Loop

Core 只负责可移植的 Model/Tool contract、显式运行循环、context/step limits、stop classification、observation events 和历史写回。Provider、文件系统、进程、审批 UI、Session 存储和终端输出继续留在 Core 外。

模型可以规划局部步骤，但不能拥有取消、超时、审批、沙箱、上下文预算或最终结算的权威。

### 2. 厚 Control Plane

Control Plane 负责把一次模型运行变成可恢复、可验证、可审计的工作单元：

```text
接收 Goal
  → 绑定 Thread / Session / World
  → 生成有界 Context
  → 检查 Deny / Plan lock / Workspace / Sandbox
  → 请求或复用 Approval
  → 执行 ToolRuntime
  → 写入 Receipt / Event / Checkpoint
  → 验证结果
  → 完成、暂停、恢复或交给人工
```

固定顺序仍为：

```text
Deny → Plan lock → workspace/sandbox → approval → execution → receipt/event
```

任何 UI 或 SDK 都不能改变 Host/Capabilities 的安全结论。

### 3. 正交控制维度

以下维度必须保持独立：

```text
Thread / Collaboration mode: default | plan
Access scope:               project | full_machine
Approval policy:            interactive | automatic | trusted
Action grant scope:         once | session | project
Continuation:               manual | continuous
Recovery state:             running | paused | settled | failed | blocked
```

`full_machine` 只是路径范围；`trusted` 只是受约束的普通工作区动作准入；`continuous` 只是 loop 的继续方式。它们都不能绕过 Deny、Plan lock、高风险确认、工具可用性或上下文/时间/步数限制。

### 4. 状态、恢复和审计优先于隐式自动化

- 每个可见 Turn 必须在 `TurnFinished` 前形成 settled checkpoint，不能把竞争中的 steer 或未完成工具组伪装成已完成状态。
- `cancel`、`interrupt`、`resume` 和 `fork` 是有记录的控制动作；停止不撤销已经产生的副作用。
- 没有通用补偿能力的副作用不得宣传为“自动 rollback”。第一阶段只记录部分完成、失败原因和下一步；只有具体工具具备可证明的补偿语义时，才增加工具级 rollback/compensation。
- 审计沿用 bounded observation events 和 `JsonlTrace`，只记录状态、counts、receipt、版本和 hashes，不记录 raw prompt、凭证、完整 arguments 或 results。

## 所有权与负空间

| 对象 | 唯一权威 | 允许的其他层 | 明确禁止 |
| --- | --- | --- | --- |
| Loop、Limits、Stop、Core events | Core | App Server 驱动，客户端消费 | Web、SDK 或 Host 自建第二个 loop |
| Tool admission、Approval、Sandbox、具体副作用 | Host/Capabilities | App Server 编排，UI 展示 pending request | UI/SDK 自行放行或缓存 grant |
| Thread、Turn、Goal、Session history、revision、checkpoint | App Server/SessionStore | SDK/Gateway 提供投影 | Web 维护第二套历史或结算状态 |
| World、Prompt、Rule、Workflow、Tool composition | Host | App Server 选择 allowlisted runtime | 公共协议接收任意 raw system prompt |
| Goal 验证与结果状态 | App Server 管生命周期，Host 提供 bounded verifier/world 输入 | Web 展示 evidence | 模型自行宣布完成而跳过验证 |
| 进程连接、JSON-RPC、类型解析 | Python SDK | Gateway 转 REST/WebSocket | SDK 重新解释运行时语义 |
| Project 清单和页面交互状态 | Gateway/Web | 发起控制请求、展示事件 | 伪造 Session、Approval 或 Recovery authority |

本提案已验证 `mini-agent-web/server/session_manager.py` 中 Thread continuation
与 App Server canonical state 的关系：偏好由 SessionStore sidecar 持久化，Gateway
只读 SessionCatalog 投影并在受控时机发起恢复请求，重复缓存和写入路径已删除。
后续重点转为跨客户端 revision 收敛、跨 Project/Thread attach，以及 active turn
取消后再 shutdown 的生命周期组合；任何新适配都不能静默覆盖 canonical state。

## 证据化问题清单

| ID | 观察与代码证据 | 影响 | 拟处理方式 | 反例 |
| --- | --- | --- | --- | --- |
| E-01 | `ContinuationMode` 从 `mini-agent-app-server-protocol` 经 Runtime、SDK、Gateway 到 Studio；`Trusted` 在 Protocol/Capabilities 中参与工具准入 | 一个 UI 字段可能改变 loop、权限和恢复行为 | 继续使用现有跨层契约矩阵，不新增 Core 依赖 | 若字段只影响展示且不会改变任何运行结果，可退回 UI-only 方案 |
| E-02 | Cargo boundary 通过，但标记 `App Server → Capabilities` review edge | runtime assembly 的具体能力仍暴露在 App Server | 仅在出现重复 authority 或具体迁移实验时下沉到 Host | 若 App Server 不再直接构造 Provider/Session/Approval，可进入依赖收敛批次 |
| E-03 | Batch 3 已将 Thread continuation 写入 SessionStore 的带 `thread_id` sidecar，Gateway 可从 SessionCatalog 只读投影；App Server startup bind 负责恢复 | worker 生命周期必须先显式 idle shutdown，活动 turn 不能绕过 settlement 释放 lock | App Server 是唯一写 authority；Gateway cache 已删除；继续沿用 shutdown/restart 与跨 Thread/Project 场景验证 | 已有 App Server shutdown/restart 场景证明新 worker 返回相同 mode 且旧 worker 已释放 lock，E-03 covered |
| E-04 | 当前 runtime 仅剩约 992 行 operating budget，release Rust 仅剩约 1,508 行 operating budget | Control Plane 很容易用新抽象掩盖复杂度增长 | 新增概念默认必须有删除项或净零抵消 | 若批次可以删除旧状态、兼容分支或重复测试并形成净减少，可放宽 |
| E-05 | 现有边界已有 Approval、Goal、Session、lock、timeout 和事件测试，但跨故障组合仍需形成统一 scenario | 单元测试通过不等于跨层可交付 | 建立 bounded Scenario/Eval 矩阵，不调用付费 Provider | 若既有公共场景已区分所有目标结果和失败反例，则不新增重复 scenario |

## 分批实施方案

### Batch 0：建立控制面契约和证据基线

- Scope: `docs/`、`.agents/notes/`、Core/Host/Capabilities/App Server/Web 的现有类型和事件。
- Delete/replace: 删除重复的“总开关”或把 `access + policy + grant + continuation` 混成一个 profile 的说明；不新增运行时代码。
- Contract delta: 仅整理已有 canonical 字段和兼容边界。
- Expected budget: runtime `+0`，release Rust `+0`。
- Evidence: `rg` 符号检索、`python scripts/line_budget.py`、`python scripts/cargo_boundary.py --json`、协议 fixture 清单。
- Stop condition: 无法为某个状态指定唯一权威，或发现 Web/Gateway 已经改变安全结论。

### Batch 1：控制面故障矩阵

- Scope: Host/App Server 的 Mock Provider、Approval、Session lock、revision 和 timeout seam。
- Delete/replace: 优先复用现有 `Thread`、`GoalRuntime`、`ApprovalStore`、checkpoint 和 event，不引入新的总编排器。
- Contract delta: 如现有事件足够，不新增事件；不足时只增加有界的 failure/receipt metadata。
- Expected budget: Core `+0`；runtime 与 release Rust 必须净零，新增测试应由删除旧分支或重复 fixture 抵消。
- Evidence: denied high-risk action、Plan lock、stale revision、approval wait、timeout、process lock、partial tool batch、provider failure。
- Stop condition: 失败后没有 durable checkpoint，或事件无法区分“未执行”“部分执行”“已结算”。

### Batch 2：恢复与验证闭环

- Scope: App Server checkpoint/resume/fork、Goal verifier、ResultStore/ThreadItem 投影和 bounded trace。
- Delete/replace: 删除任何把“模型最终文本”当作完成证据的路径；不承诺无补偿能力工具的自动 rollback。
- Contract delta: 保持现有 Thread/Turn/Goal/ThreadItem 语义，必要时补充有界 verification result。
- Evidence: 重启后恢复 settled state；Goal 在验证失败时暂停或失败；已完成副作用不被重复执行；审计不含 raw secret/prompt。
- Stop condition: resume 会重放不可重放工具，或者 verifier 只能依赖模型自报成功。

### Batch 3：清理重复状态与依赖边界

- Scope: `mini-agent-web/server/session_manager.py` 的 continuation 适配，以及 `mini-agent-app-server` 的 runtime assembly 边界。
- Delete/replace: 如果 canonical persistence 成立，删除 Gateway continuation cache；如果 Host 能完整接管 concrete Provider/Session/Approval assembly，才评估删除 `App Server → Capabilities`。
- Contract delta: 不为减少 Cargo edge 添加隐藏 facade；所有权迁移必须保留结构化 RPC 意图。
- Evidence: 重启、并发 attach、跨 Thread、跨 Project、revision 变化和 `cargo_boundary.py --json`。
- Stop condition: 新 seam 需要 App Server、Host、Gateway 各自保存同一份 authority，或引入循环依赖。

### Batch 4：结果导向的用户契约

- Scope: Goal/Thread 输入、Verifier、ThreadItem/Artifact 投影和 Web Studio 的有限入口。
- Delete/replace: 删除要求模型按固定步骤执行的 UI 文案；优先把输入收敛为 `Outcome`、`Boundary`、`Invariant`、`Deliverable` 和 `Review point`。
- Contract delta: 先复用 Goal、ThreadItem 和 ResultStore；只有 Scenario 证明现有结构无法表达结果合同，才设计新类型。
- Evidence: 同一 Goal 使用不同模型或不同局部步骤仍能得到相同验收结论；缺少 Boundary 或 Deliverable 时明确询问或停止，不默默放宽权限。
- Stop condition: 新输入导致模型可见上下文无界，或结果合同变成另一套执行循环。

## 验收标准

| ID | 给定条件与操作 | 预期 trace / 结果 | 失败反例 | 证据 |
| --- | --- | --- | --- | --- |
| AC-01 | 给定同一 Goal、Boundary、Invariant，替换 Mock Model 的局部计划 | Thread/Turn/Tool/Verifier 的最终状态一致；局部步骤可以不同 | 必须依赖特定 Planner 顺序才能通过 | bounded Harness Scenario |
| AC-02 | `trusted + project` 请求普通已校验 patch | Host/Capabilities 允许普通 patch，但高风险、删除、越界或外部动作仍产生 Approval | `trusted` 变成 allow-all | Capabilities admission tests + RPC fixture |
| AC-03 | `continuous` Turn 遇到 cancel、timeout、context limit 或 approval wait | 先按规定顺序观察控制信号，产生可解释 stop reason 和 durable checkpoint | 连续模式无限运行或丢失 pending action | Core/App Server control scenario |
| AC-04 | Tool batch 中途失败或进程重启 | 已完成副作用、未执行动作和可恢复下一步均可区分，不重放不可重放调用 | 恢复后重复写文件或把部分完成报告成成功 | fault-injection scenario |
| AC-05 | Goal verifier 对最终结果返回失败 | Goal 进入 paused/blocked/failed 等明确状态，用户看到缺口和下一步 | 模型文本声称完成就结束 | Goal verifier integration test |
| AC-06 | 两个客户端观察同一 Thread，并在一个客户端更新控制项 | App Server event/revision 是唯一状态源，另一个客户端最终收敛 | Web 各自显示不同 continuation/approval 状态 | SDK/Gateway/Web integration test |
| AC-07 | 审计和 trace 在成功、拒绝、超时、恢复场景中生成 | 只含 bounded metadata、counts、hashes 和状态，不含 raw prompt、secret、完整参数/结果 | 为了排障把敏感上下文写入 trace | trace redaction test |
| AC-08 | 运行依赖边界检查和预算门禁 | `cargo_boundary.py --json` 无 violation；runtime/release 不进入 red band | 通过新增 facade 或放宽安全规则解决行数/依赖问题 | Cargo boundary + line budget |

## 明确非目标

- 不追求 Codex 的 Planner、Coder、MCP、模型、插件或 UI 数量 parity。
- 不在 Core 中加入 Scheduler、Memory、Plugin、Policy、Rollback 或 Orchestration framework。
- 不把连续执行、Full Machine 或 Trusted 组合成隐式超级权限。
- 不把 Web、SDK 或 Gateway 变成 Session、Approval、Recovery 的权威持有者。
- 不承诺所有文件、Shell、MCP 或外部动作都能自动 rollback；补偿语义必须逐工具证明。
- 不以真实 Provider 或付费请求作为默认证据。
- 不为了局部行数目标删除 Core、Actor/CAS/Session authority、公共协议或边界测试。

## 六项变更准入回答

1. **所属层**：本提案属于跨层架构，但执行面仍由 Core 保持最小；控制面由 App Server、Host 和 Capabilities 按现有所有权承载，SDK/Gateway/Web 只做协议和交互适配。
2. **重复职责**：已有 `Thread`、`Turn`、`Goal`、`SessionStore`、`ApprovalStore`、`ToolOrchestrator`、checkpoint、events 和 `JsonlTrace` 已覆盖大部分责任；实施前必须检索并证明新类型不能替代旧类型。
3. **替换优先**：优先删除 Gateway shadow state、隐式总开关、模型自报完成路径和重复兼容分支；只有 Scenario 证明现有边界无法表达时才新增概念。
4. **净行数**：当前 runtime 为 `18,008/20,000`，release Rust 为 `27,492/30,000`；相对 `cea7a04` 累计为 `+272/+326`，相对 `82d8906` 本批为 `+66/+66`，仍低于 operating/red-band 门槛。Core production 预期净增为 `0`；本批只增加 bounded fault evidence 测试，未改变 Core production 责任；每个实现批次默认 runtime/release 净零或提供明确删除抵消，进入 red band 即停止扩张。
5. **可见表面**：新增的 Goal/Boundary/Invariant、控制字段、事件和结果投影都必须有 hard limit；未知权限输入 fail closed；不得把 raw prompt、凭证或无界工具结果写入模型上下文、事件或持久化。
6. **边界证据**：使用 Core/Capabilities/App Server 的单测与协议 fixture，再用 Mock Provider 的 bounded Scenario 覆盖拒绝、Plan lock、超时、取消、锁竞争、部分副作用、恢复、验证失败和审计脱敏；跨仓验证 SDK、Gateway 和 Studio 收敛到同一 revision。

## 决策请求与完成定义

本提案请求评审是否同意“薄 Loop、厚 Control Plane、结果优先、证据驱动”的方向，而不是立即批准新增一套长期 Agent 框架。

提案只有在以下条件全部满足后，才应移入 `implemented/`：

1. 至少一个不依赖固定 Planner 步骤的结果导向 Scenario 通过；
2. 至少一个故障注入 Scenario 证明拒绝、超时、部分完成和恢复边界；
3. State、Permission/Sandbox、Recovery、Verification、Audit 各有唯一权威和反例证据；
4. Gateway continuation 状态是否为重复 authority 已经决定并验证；
5. Cargo boundary、受影响包测试、lint、line budget 和跨层协议证据全部通过；
6. 文档、协议、SDK、Gateway、Studio 与实际行为一致，且剩余风险被明确记录。
