# Web Studio 独立 Session 派生与上下文压缩

状态：implemented
日期：2026-09-14
范围：`mini-codex`、Python SDK、`mini-agent-web` Gateway 和 Web Studio

## 结论

Web Studio 的“派生聊天分支”将创建一个新的 `session_id`、新的持久化目录和
独立的 App Server client。父 Session 保持不变。派生只复制父 Thread 最近一次
已结算的 checkpoint，不复制整份历史 JSONL。

派生默认使用 `exact` 策略：

- 只复制最近一次已结算 checkpoint，不在 fork 时调用模型，也不触发压缩；
- 源 Session 的正常 Turn 继续按自身 `ContextLimitBehavior` 自动压缩，并把结果
  写回源 Session；child 首次运行时再按 child 自己的配置判断是否需要压缩；
- “派生并压缩”只能通过显式 `contextPolicy=compact` 请求，压缩失败时父 Session
  保持不变；
- 两种路径都不执行工具、不产生工具审批，页面显示实际的压缩前后大小和结果。

这项能力解决两个问题。20 MB 的父 `session.jsonl` 不会被完整复制到分支，父文件
也不会因为分支继续增长。派生分支会得到一个新的、可重启恢复的 Session。新文件
的初始大小取决于复制后的 checkpoint，而不是父 JSONL 的历史总大小。

用户流程如下：

1. 用户在已结算的会话上选择“派生聊天分支”。
2. Studio 显示“正在准备分支”，同时锁定父会话的派生操作。
3. App Server 检查当前 Thread 没有活跃 Turn、审批等待或停止结算。
4. Core 为分支生成有界 checkpoint。必要时只压缩分支副本。
5. SessionStore 原子创建新 Session，并记录父子关系。
6. Gateway 启动独立的子 App Server，Studio 切换到新的 Thread 和 Session。
7. Header、侧栏和运行详情显示新的 `session_id`，详情中保留父 Session 和压缩结果。

## 实现结果

本决策已经落地，实际边界如下：

- Core 的 `Harness::prepare_fork_checkpoint` 只在已结算 Thread 上准备副本；`exact`
  保留原 checkpoint，不检查 fork 专属的上下文软阈值、不调用模型；显式 `compact`
  才复用既有模型摘要/机械压缩规则，且不会修改父 Harness、执行工具或产生审批。
- Capabilities 的 `SessionStore::fork_from_checkpoint` 原子创建新的 Session 目录、
  lock、thread index 和 `forked_from` 记录，仅写入整理后的 checkpoint，并复制附件；
  父 `session.jsonl` 不会被重写。
- App Server 新增 `session/fork`，只接受空闲运行时和有界的
  `sourceThreadId/newThreadId/contextPolicy`，返回 Session ID、父 lineage、checkpoint
  序号、Session/上下文大小和压缩方式。
- Gateway 为 child 启动独立 App Server client；父子 Thread 不共享 client、Session lock、
  审批上下文或事件接收器。child 启动失败时保留可 attach 的 Session 目录，不留下永久
  运行时绑定。
- Web Studio 的“派生独立分支”默认使用 `exact`，在 Header 和运行详情中显示实际
  Session ID、父 Session、checkpoint、大小及压缩结果；侧栏另提供“派生并压缩”，
  通过显式 `compact` 选择。原有 `thread/fork` 继续保留为嵌入式逻辑 Thread 分支。

实现后的尺寸基线为 runtime `19,703 -> 20,102`（`+399`）、release Rust
`29,483 -> 30,045`（`+562`）；总量仍处于 green。仓库的 Green/Amber 单 PR
增量门禁现为 runtime `+200`、release `+300`，本批次增量检查因此仍会明确报告超额，
后续应拆分或以删除冗余概念抵消，不以降低边界质量换取通过。

所有者链路为：

```text
Core context normalization
  -> App Server fork coordination
  -> Capabilities SessionStore persistence
  -> Python SDK transport
  -> Gateway process binding
  -> Web Studio state and display
```

## Harness hypothesis

如果派生操作复制的是最近一次已结算 checkpoint，而不是整份 Session 日志，且默认
不改变源 Session 的正常压缩节奏，那么：

- 父 Session 不会被分支操作改写；
- 分支可以拥有独立的 Session lock、生命周期、事件序列和恢复路径；
- 一个因重复 checkpoint 变大的 20 MB Session 可以生成远小于 20 MB 的新 Session；
- 分支首次运行按 child 自身的上下文配置处理，不会因为 fork 隐式改变了 checkpoint
  而产生不可解释的压缩差异。

反例是：父 Thread 正在运行、父 Session 只有未结算状态、压缩后仍超过上下文或
记录限制，或者子 Session 在 Gateway 重启后无法从 catalog 恢复。任一反例成立，
本提案都不能进入 `implemented`。

## 当前证据

| ID | 观察 | 代码证据 | 影响 | 根因 | 反例 |
| --- | --- | --- | --- | --- | --- |
| E-01 | Web Gateway 的 fork 调用 source client 的 `thread/fork`，然后把 child 绑定到同一个 client。 | [`server/control/client_pool.py`](../../../../mini-agent-web/server/control/client_pool.py) 的 `fork_thread` | 父子 Thread 共用 App Server 和 SessionStore，不能形成独立 Session。 | Gateway 复用了进程内 Thread 分叉，而不是 Session 分叉。 | 若 App Server 为每个 child 自动创建新 Session，E-01 不成立；当前代码没有这一步。 |
| E-02 | App Server `ThreadManager::fork` 读取 source checkpoint，并在内存中恢复到新 Thread。 | [`thread_manager.rs`](../../../../crates/mini-agent-app-server/src/thread_manager.rs) 的 `fork` | 分支建立时没有新的持久化 Session 记录。 | Core Thread 拓扑和 SessionStore 生命周期是两个不同概念。 | 如果 fork action 同时写入 SessionStore，需补充代码证据；当前 action 只操作 ThreadManager。 |
| E-03 | Capabilities 已有 `SessionRequest::Fork`，会读取最新 settled checkpoint，创建新目录，写入 `forked_from`，并复制附件。 | [`session.rs`](../../../../crates/mini-agent-capabilities/src/session.rs) 的 `SessionStore::fork` | 可复用持久化和 lineage 基础，但 Web Gateway 尚未使用。 | CLI 嵌入式运行路径先实现了 Session fork，Web 路径仍停留在 Thread fork。 | 如果新目录复制完整 JSONL，E-03 的“只写 checkpoint”描述不成立；当前实现写入初始化 header、Thread 和 checkpoint。 |
| E-04 | SessionStore 的单文件上限是 32 MiB，单条 JSONL 记录上限是 512 KiB。 | [`session.rs`](../../../../crates/mini-agent-capabilities/src/session.rs) 的 `MAX_SESSION_BYTES`、`MAX_RECORD_BYTES`；[`docs/limits.md`](../../../../docs/limits.md) | 父 Session 接近上限时，整文件复制会放大失败概率；子 checkpoint 也必须满足单条记录上限。 | 存储大小和模型上下文大小是不同边界。 | 如果未来改了限制，契约测试必须从常量读取，而不能依赖本提案中的数字。 |
| E-05 | Core 的正常压缩在运行 Turn 的 `prepare_context` 中按源配置触发；`session/fork` 默认只复制最近 settled checkpoint，显式 `compact` 才调用同一套整理规则。 | [`harness.rs`](../../../../crates/mini-agent-core/src/harness.rs) 的 `prepare_context`、`prepare_fork_checkpoint`、`compact_context`；[`docs/limits.md`](../../../../docs/limits.md) | 源 Session 的压缩节奏与 fork 的快照语义分离；用户可明确选择是否为 child 预压缩。 | fork 不应猜测 source/child 的上下文阈值，也不应在默认操作中额外调用模型。 | 如果默认 fork 仍在软阈值处调用 provider，E-05 重新成立。 |
| E-06 | App Server `thread/fork` 返回的结果只有新的 Thread ID，Gateway 的 fork 响应没有新的 Session ID。 | [`json_rpc/thread.rs`](../../../../crates/mini-agent-app-server/src/json_rpc/thread.rs)；[`server/control/client_pool.py`](../../../../mini-agent-web/server/control/client_pool.py) | Studio 无法准确显示新分支的实际 Session，也无法在重启后独立 attach。 | 协议只表达 Thread 分叉，没有表达 Session 分叉结果。 | 如果现有响应能稳定返回 child Session ID，需把该响应 fixture 加入证据。 |

## 设计决定

### 1. 保留两个概念，但不再混用

`thread/fork` 继续表示 App Server 进程内的逻辑 Thread 分叉，供嵌入式调用者使用。
Web Studio 的“派生聊天分支”不再调用它，而是调用新的 Session 分叉管理操作。

产品语义定义如下：

```text
Thread ID   = 逻辑会话路由身份
Session ID  = 持久化、锁和恢复身份
Fork        = 从一个已结算 Thread checkpoint 创建独立聊天分支
```

独立 Session 不代表独立 Project 权限。子 Session 仍使用目标 Project 的访问范围、
批准策略和工作区边界。权限 grant、父 Goal 的活跃调度和父 Plan 的清理状态不从父
Session 直接复制。

### 2. 使用单一的分支准备结果

App Server 生成一个内部结果，避免 Gateway 自己拼装 checkpoint：

```text
ForkPreparation {
    messages: Vec<Message>,
    context_before_bytes: usize,
    context_after_bytes: usize,
    method: Exact | ModelSummary | Mechanical,
}
```

`messages` 只在 App Server 到 Capabilities 的内部调用中传递。它不能进入 Gateway
状态缓存，也不能进入 Web 响应。响应只返回有界的大小、布尔结果和 lineage。

### 3. 在 Core 增加“为分支准备 checkpoint”的边界方法

Core 将复用现有 `compact_context` 的保留规则，不新增第二套压缩算法。新的方法
只允许在 Thread 已结算时调用。`exact` 依赖已恢复的 settled checkpoint 已通过
逐条消息限制校验；`compact` 才在 fork 边界执行模型上下文限制校验：

```text
prepare_fork_checkpoint(policy)
    -> settled messages + compaction report
```

方法在临时 SessionState 上工作。它不会向父 Session 写入 checkpoint，不改变父
Thread 的下一轮计数、最后 Turn、事件序列或状态。压缩请求使用空工具列表，不能
触发工具、副作用或审批。

显式 `compact` 方法必须同时验证：

- 模型请求上下文不超过 `HarnessConfig.max_context_bytes`；
- 每个 Context、模型响应和工具结果仍符合 Core 限制；
- 压缩确实减少上下文，或者机械裁剪后满足硬限制；
- 无法满足限制时返回结构化错误，不创建半成品子 Session。

初始化 checkpoint 的 JSONL 单记录上限由 Capabilities `SessionStore` 在写入边界
校验；Core 不复制这个存储常量，也不在内部重复实现 Session 文件限制。

### 4. Capabilities 写入独立 Session

在现有 `SessionStore::fork` 的基础上抽出一个只写给定 checkpoint 的存储方法：

```text
fork_from_checkpoint(
    workspace,
    parent_session_id,
    parent_checkpoint_seq,
    child_thread_id,
    messages,
) -> OpenedSessionMetadata
```

该方法负责：

- 读取并校验父 Session 的身份和来源；
- 创建新的 Session ID 和临时目录；
- 写入 `session_created`、`thread_started` 和一个初始化 checkpoint；
- 写入 `forked_from.parent_session_id` 和 `parent_checkpoint_seq`；
- 复制当前 Thread 的附件目录；
- 先完成文件 flush、sync 和目录提交，再更新 child 的 Thread index；
- 失败时删除或标记不可见的临时目录，不让 catalog 暴露半成品。

父 Session 的完整 JSONL 不复制。子 Session 的首个文件大小由初始化记录和给定
checkpoint 决定。

### 5. App Server 和 Gateway 分开持有父子运行时

增加 App Server 管理操作 `session/fork`：

```json
{
  "sourceThreadId": "thread-a",
  "newThreadId": "thread-a_fork_7k2m",
  "contextPolicy": "exact"
}
```

需要显式压缩时，调用方传入 `contextPolicy=compact`；结果只返回：

```json
{
  "threadId": "thread-a_fork_7k2m",
  "sessionId": "s-child",
  "parentSessionId": "s-parent",
  "parentCheckpointSeq": 42,
  "compacted": false,
  "contextBeforeBytes": 812000,
  "contextAfterBytes": 812000,
  "method": "exact"
}
```

字段是有界数字、标识和枚举。响应不携带完整对话、工具参数或模型摘要。

Gateway 在同一个 `SessionManager` 锁内执行以下顺序：

1. 解析 source 的 Project、Thread 和当前 Session 身份。
2. 拒绝活跃 Turn、审批等待、停止结算、只读锁和无法确认的运行状态。
3. 调用 source client 的 `session/fork`。
4. 使用返回的 `session_id` 启动新的 App Server client，模式为 `resume`。
5. 确认 child client 已 attach 并能读取 child Thread。
6. 写入 child 的 Gateway metadata 和 lineage 摘要。
7. 绑定 child 到独立 client 后返回 Gateway 响应。

Gateway 不读取 `session.jsonl`，不复制 checkpoint，也不把父 client 复用给 child。
如果 child client 启动失败，child Session 保留为可诊断、可重试的历史项，Gateway
不能把它伪装成已经运行成功的分支。

### 6. Settings、Goal、Plan 和附件的继承边界

| 对象 | 子分支行为 |
| --- | --- |
| Project 访问范围和批准策略 | 继续读取目标 Project 的权威配置，不复制 grant 缓存 |
| continuation mode | 复制为 child Thread 的初始设置，并改写 Thread ID |
| builtin tool 可见性 | 由 child client 按 Project 和 Thread 配置重新计算 |
| Goal | 不自动激活父 Goal；父 Goal 的状态不驱动 child |
| Plan | 不复制活跃 Plan 控制状态；已完成 Plan 内容只能作为 checkpoint 文本存在 |
| attachments | 复制父 Session 的附件目录，保持 child 私有副本 |
| approval evidence | 不复制父审批 pending；新的工具请求使用 child 的身份 |

## 所有权和边界

| 对象 | 唯一权威 | 允许的消费者 | 禁止的重复实现 |
| --- | --- | --- | --- |
| checkpoint 内容和上下文整理 | Core Thread/Harness | App Server 调用并消费结果 | Gateway、SDK 和 Web 自己截断消息 |
| Session ID、文件、lock、lineage | Capabilities `SessionStore` | App Server、SDK、Gateway 读取结果 | Gateway 复制或修改 `session.jsonl` |
| fork admission、忙碌检查和操作顺序 | App Server Runtime Actor | SDK 和 Gateway 提交请求 | Web 根据本地 `isGenerating` 自己放行 |
| 父子 client binding | Gateway `SessionManager` | Web 通过 API attach 和切换 | 多个 client pool 建立影子绑定 |
| Project/Thread 页面状态 | Web Studio | 展示状态和错误 | Web 保存第二份 checkpoint 或 Session 状态 |
| 工具准入和副作用 | Host/Capabilities | 分支继承 Project 配置并重新准入 | Fork 过程执行工具或复制批准结果 |

独立 Session 解决的是持久化和运行时隔离，不改变 Host 对权限和副作用的权威。

## 跨仓契约矩阵

| 语义 | Core/Host | App Server RPC/Event | SDK | Gateway/Web Studio |
| --- | --- | --- | --- | --- |
| 派生请求 | `ForkContextPolicy`、有界准备结果 | `session/fork` 请求和 `SessionForkResult` | `fork_session(...)`，解析结构化错误 | `POST /api/threads/fork` 使用独立分支语义 |
| Session 身份 | `SessionStore` 返回 child ID | result 含 `sessionId`、parent ID、checkpoint seq | DTO 保留完整字段 | Header、侧栏和详情显示 child ID |
| 压缩结果 | Core `CompactionMethod`、前后 bytes | bounded result，不发送摘要正文 | 保留枚举和数字 | 显示“已压缩/未压缩”和大小 |
| 忙碌状态 | Thread checkpoint 拒绝 Running | `Busy`、`ReadOnly`、`ApprovalPending`、`Stopping` | 保留错误类别 | 禁用按钮并说明下一步 |
| lineage | Session header `forked_from` | 返回 parent Session 和 checkpoint seq | 解析但不重写 | 详情抽屉显示来源 |
| 子运行时 | Host/SessionStore 新 lock | child 以 `resume` 启动 | 新 client 生命周期 | 父子会话可独立切换和重连 |

`thread/fork` 的现有响应保持原义，仍只表示进程内 Thread 分叉。Web Gateway 的
`POST /api/threads/fork` 将切换到独立 Session 语义，并在 API 文档和测试 fixture
中明确，不增加一个含义模糊的布尔开关。

## 上下文和大小规则

父文件大小不能直接推导子文件大小。SessionStore 是追加日志，每个 settled Turn
会写入消息和 checkpoint。父文件可能因为重复 checkpoint 达到 20 MB，而最新
checkpoint 仍只有几百 KiB。

派生结果应返回实际测量值。对当前默认限制，初始 child 文件必须满足：

```text
checkpoint JSONL record <= 512 KiB
model-visible context <= 1 MiB
child Session file <= 32 MiB
```

因此，20 MB 父 Session 的典型结果是：

- 父文件仍约 20 MB；
- child 只写一个 checkpoint，初始文件通常是几百 KiB 量级；
- 默认 `exact` 始终不产生模型压缩请求；显式 `compact` 是否需要请求取决于可压缩
  的历史前缀；
- 默认不会因为最新上下文接近软阈值而改变 checkpoint；只有显式 `compact` 才使用
  压缩后的大小；
- 不能用“20 MB 的固定比例”承诺结果，必须以返回的 `sessionBytes` 和磁盘测量值为准。

## 失败、重启和并发处理

- source Turn 运行中：返回 `Busy`，不读取不稳定 checkpoint。
- source 等待审批：返回 `ApprovalPending`，不让 fork 绕过审批或复制 pending。
- source 正在停止：返回 `Stopping`，等待权威终态后由用户重试。
- source 是其他进程的只读 Session：允许历史读取，但拒绝 fork 控制操作。
- 显式 `compact` 的 provider 压缩请求失败：按现有压缩错误路径返回，父不变；默认
  `exact` 不发起 provider 请求，因此不会因 fork 增加一次模型失败点。
- 子文件写入中断：child 目录不可进入 catalog；重启清理未提交临时目录。
- 子 client 启动失败：保留 child Session 的失败原因和 Session ID，允许 attach 重试。
- Gateway 在返回前崩溃：下次 catalog 扫描通过 `forked_from` 和 child Session 文件恢复；
  不重复创建同一 child。请求需要使用有界幂等键，重试只能得到同一个 child 结果。
- 父子同时运行：两个 Session 各自持有 lock、Turn、事件序列和审批身份；两者仍受
  同一 Project 的权限边界约束。

## 备选方案

### A. 保留当前同一 App Server 的 `thread/fork`

不采用。它的实现成本最低，但父子共享 SessionStore、lock 和事件空间，不能保证
独立恢复，也不能解决父 Session 继续增长的问题。

### B. Gateway 直接复制 `session.jsonl`

不采用。Gateway 会绕过 SessionStore 对 checkpoint、锁、序列、附件和 lineage 的
所有权。复制 20 MB 日志还会把重复 checkpoint 原样带入 child，无法解决大小问题。

### C. Gateway 创建空 Session，再调用 `thread/resume`

不采用。`thread/resume` 只安装内存 checkpoint，首次 Turn 之前没有可靠的 child
历史持久化。Gateway 还需要理解 Core checkpoint，导致跨层重复上下文逻辑。

### D. App Server 准备 checkpoint，Capabilities 写新 Session，Gateway 启动 child

采用。Core 保持模型上下文规则，Capabilities 保持 Session 文件权威，App Server
负责运行时顺序，Gateway 只负责独立进程绑定。该方案重用现有 SessionStore fork、
checkpoint、compaction 和 attach 边界，删除 Web 侧共享 client 的旧路径。

## 可证伪验收标准

| ID | 前置条件 | 操作 | 可观察结果 | 失败反例 | 证据 |
| --- | --- | --- | --- | --- | --- |
| AC-01 | source 有 settled checkpoint | 点击“派生聊天分支” | 返回新的 `thread_id` 和 `session_id`，父 ID 不变 | child 使用父 Session ID 或复用父 client | App Server fixture、Gateway 集成测试 |
| AC-02 | source `session.jsonl` 为 20 MB，最新 checkpoint 远小于全日志 | 派生并检查 child 文件 | child 不包含父历史全量日志，报告真实 `sessionBytes` | child 复制出接近 20 MB 的重复 JSONL | bounded SessionStore scenario |
| AC-03 | source 上下文达到 512 KiB 软阈值 | 使用默认策略派生 | child 标记 `compacted=false`，前后大小相同且 provider 请求数不增加 | fork 因软阈值隐式压缩或父文件出现新的 compaction 记录 | Core scenario + protocol fixture |
| AC-04 | source checkpoint 可由 child 自身的正常 Turn 压缩处理 | 默认派生后启动 child | fork 不先代替 child 压缩；child 首次 Turn 按自身配置产生正常 compaction | fork 使用 source 的阈值猜测 child 行为 | Core/App Server scenario |
| AC-05 | 显式 `compact` 的 provider 请求失败 | 派生 | 返回结构化失败且不创建可见 child；仅对不适合的模型摘要响应沿用既有机械裁剪路径 | 静默丢历史或创建半成品 Session | Mock provider scenario |
| AC-06 | source 有活跃 Turn、审批或停止结算 | 派生 | 返回对应 busy 状态，不创建 child，不改变 source | child 从未结算状态产生 | App Server/Gateway tests |
| AC-07 | parent and child 已创建 | 重启 Gateway 和两个 App Server | 两个 Session 均从独立目录恢复，ID、lineage、Thread 可读 | child 只存在内存，重启后消失 | restart scenario |
| AC-08 | parent 和 child 同时运行 | 分别提交 Turn | 两个 Turn、事件序列和审批身份互不污染 | 一个 Session 收到另一个 Session 的事件或 approval | multi-session integration test |
| AC-09 | 父有附件 | 派生并读取附件 | child 有私有附件副本，父附件不受修改 | child 依赖父目录或修改父文件 | Capabilities test |
| AC-10 | child 启动在写入后失败 | 重试 attach | catalog 显示可诊断失败，重试不创建第二个 child | 返回成功但无法 attach，或重复创建 | Gateway failure-injection test |
| AC-11 | UI 已切换到 child | 查看 Header、侧栏和详情 | 显示 child `session_id`、父 Session ID、压缩结果 | 只显示标题或继续显示父 Session ID | Web component/integration test |
| AC-12 | 继承 Project policy，父有 pending approval 或 Goal | 派生并运行 child | child 重新按 Project policy 准入，不继承 pending approval 或父 Goal 调度 | child 复用父 approval grant 或被父 Goal 驱动 | Host/Gateway/Web scenario |

## 批次卡

### Batch 1：删除 Web 侧共享 Session 的 fork 路径

- Scope：`mini-agent-web/server/control/client_pool.py`、Gateway route、现有 fork tests。
- Delete/replace：删除 Web Studio 使用的 `bind_thread_client(child, parent_client)` 路径。
- Contract delta：Gateway fork 语义改为独立 Session；保留 App Server `thread/fork` 原义。
- Expected budget：Python 代码净减少或持平；Rust runtime `+0`。
- Evidence：先完成 AC-01、AC-06、AC-08 的 Gateway binding fixture。
- Stop conditions：无法在同一 Gateway lock 内获得唯一 parent Session 身份时停止。
- Commit：Gateway 不再把 Web 分支绑定到父 client。

### Batch 2：稳定 Core 的分支 checkpoint 整理边界

- Scope：`mini-agent-core` 的 Harness/Thread、Core bounded scenario。
- Delete/replace：复用现有 compaction 保留规则，不新增第二套裁剪器。
- Contract delta：增加 `ForkContextPolicy`、`ForkPreparation` 和结构化压缩结果；默认
  策略为 `Exact`，`Compact` 仅由显式调用选择。
- Expected budget：runtime `+180..+300` effective lines；release `+180..+320`。
- Evidence：AC-03、AC-04、AC-05，包含 parent unchanged 和 empty tool catalog。
- Stop conditions：无法保证压缩失败时 source checkpoint 不变时停止。
- Commit：Core 可以生成不修改父 Thread 的有界分支 checkpoint。

### Batch 3：Capabilities 和 App Server 持久化独立 Session

- Scope：SessionStore、App Server management RPC/protocol、SDK DTO/API。
- Delete/replace：将现有 `SessionStore::fork` 的重复初始化逻辑收敛到给定 checkpoint
  的写入方法；不新增 Gateway 文件写入。
- Contract delta：增加 `session/fork` 和 `SessionForkResult`，含 lineage 与 bounded metrics。
- Expected budget：runtime `+300..+500`；release `+550..+850` effective lines，需提供
  同批次删除重复初始化和无效测试胶水的实际抵消。
- Evidence：AC-01、AC-02、AC-07、AC-09、AC-10。
- Stop conditions：父 lock、child temp cleanup 或 thread index 无法形成原子边界时停止。
- Commit：App Server 能提交一个可重启恢复的独立 child Session。

### Batch 4：Web Studio 状态、文档和全链路故障注入

- Scope：SDK/Gateway/Web Studio、现有 Session ID 展示、docs 和 notes。
- Delete/replace：Web 不再显示共享 Session 的隐含语义，删掉旧 fork 响应假设。
- Contract delta：显示新的 Session ID、lineage、压缩结果和可恢复失败状态。
- Expected budget：Web/SDK/Python 净增长需保持小范围；Rust 不新增公共运行时概念。
- Evidence：AC-08、AC-11、AC-12，桌面宽屏、多窗口、重启和失败注入验收。
- Stop conditions：出现跨 Project/Thread/Session 的事件或审批污染时停止发布。
- Commit：用户看到的是独立、可恢复且可解释的派生聊天分支。

## 六问准入

1. **所属层。** Context normalization 属于 Core，因为 Core 拥有模型可见上下文、
   hard limit 和 compaction 规则。Session 文件和 lineage 属于 Capabilities，因为
   SessionStore 是唯一持久化权威。fork 顺序属于 App Server，client 进程绑定属于
   Gateway，展示属于 Web。Gateway 不能直接实现其中任何一个规则。
2. **已有职责。** 已检查 `ThreadManager::fork`、`Harness::compact_context`、
   `SessionStore::fork`、`RuntimeManagementService`、SDK `MiniAgentClient` 和
   Web `ClientPool.fork_thread`。方案复用前四者的边界，只替换 Web 侧错误的共享
   client 调用，不再增加 Gateway checkpoint 缓存。
3. **旧概念。** 删除 Web Studio 对同一 App Server `thread/fork` 的依赖。保留
   App Server `thread/fork`，因为它是嵌入式调用者的逻辑 Thread API，且不承担独立
   Session 语义。新的 `session/fork` 是明确的独立 Session 操作。
4. **预算。** 基线为 runtime `19,703`、release Rust `29,483` effective lines；实际
   增长为 runtime `+399`、release Rust `+562`，总量仍在 green。当前 Green/Amber
   单 PR 增量门禁为 runtime `+200`、release `+300`；本批次超出该门禁，后续需
   删除重复路径或拆批，不降低边界。
5. **可见面。** 新增一个 App Server `session/fork` 方法和一个 bounded result；
   不新增完整历史、工具参数或模型摘要的 Gateway/Web 传输。新增一个 child Session
   目录、`forked_from` header 和 bounded metrics。模型只接收现有 compaction prompt
   及有界 checkpoint，不接收新的无限输入。
6. **边界测试。** 需要 Core Mock Provider scenario、Capabilities SessionStore
   fixture、App Server JSON-RPC fixture、SDK 错误映射、Gateway 双 client/restart/
   cleanup 测试和 Web UI 组件测试。AC-04、AC-06、AC-07、AC-10 是必须失败的反例，
   不能只用成功路径证明方案。

## 验证结果

实现按权威层向外验证：

```text
cargo fmt --all
cargo clippy -p mini-agent-core --all-targets -- -D warnings
cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings
cargo clippy -p mini-agent-app-server --all-targets -- -D warnings
cargo test -p mini-agent-core
cargo test -p mini-agent-capabilities
cargo test -p mini-agent-app-server
python scripts/line_budget.py
python scripts/cargo_boundary.py --json
```

然后在 `mini-agent-web` 运行其 Gateway、SDK 和前端检查：

```text
uv run ruff check .
uv run ruff format --check .
uv run pytest -q
npm run lint
npm test
npm run build
```

Core、Capabilities、App Server、SDK、Gateway 和 Web 已覆盖独立 Session、checkpoint
边界、子 client 绑定、重启恢复和展示链路。现有测试不使用付费 Provider；模型与审批
验证使用本地/确定性 fixture。跨浏览器实时审批协调、真实 provider 压缩耗时和完整多
窗口故障注入仍保留为后续场景验收。

本轮语义优化的验证重点是：默认 `exact` fork 不增加 provider 请求、不写回父
Session，也不因 source 的软阈值触发 fork 压缩；显式 `compact` 仍复用原有压缩路径。

提案阶段已记录的基线：`python scripts/line_budget.py` 于 2026-09-14 输出
runtime `19,703/25,000`、release Rust `29,483/35,000`，两个预算均为 green。
实际 before、after 和 delta 已记录在“实现结果”：runtime `19,703 -> 20,102`
（`+399`），release Rust `29,483 -> 30,045`（`+562`）。

## 已处理风险与当前限制

- 显式 `compact` 仍会消耗一次 provider 请求时间和额度；Gateway 当前在 fork API
  返回前等待 checkpoint 准备完成，UI 以已有请求状态和 Toast 呈现结果。默认
  `exact` 不增加 provider 请求，child 的正常 Turn 才按自身配置决定是否压缩。
- 子 client 的启动失败会保留 child catalog 记录，后续 attach/retry 仍需由用户触发；
  SessionStore 创建阶段的临时目录和 lock 会清理。
- 本批次已覆盖 Core、Capabilities、App Server、SDK、Gateway 和 Web 的有界成功路径、
  独立绑定、重启恢复和 UI 展示；跨浏览器实时审批协调、真实 provider 压缩耗时和完整
  多窗口故障注入仍属于后续场景验收，不改变独立 Session 的持久化边界。
