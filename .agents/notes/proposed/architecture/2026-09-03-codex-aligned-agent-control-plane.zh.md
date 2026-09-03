# 面向 Codex 的 Agent 控制平面：批准、计划、目标与 Web Studio

状态：提案中 — 等待评审和分批实现
日期：2026-09-03
范围：mini-codex Protocol/Host/App Server，以及 mini-agent-web SDK/FastAPI/Web Studio

## 待决策事项

将 Mini Agent 的执行路径和 Web Studio 控制面统一到一条明确的控制链：

```text
Web Studio
    → FastAPI 网关
    → Python SDK
    → App Server JSON-RPC
    → Host/Capabilities 准入与批准
    → mini-agent-core Turn Loop
```

控制平面必须明确用户控制和执行所有权，不引入独立的 Profile 层：

```text
Thread Mode（Session）：chat | plan | goal
访问与批准（运行时）：访问范围 + 策略 + 有界生命周期
执行所有权：Core Turn | Plan 生命周期 | Goal 生命周期
```

此外，每个 Thread/Session 都必须绑定到一个不可变的工作区清单：

```text
Workspace Binding（Session）：primaryRoot + associatedRoots + 每个目录的访问策略
```

面向用户的操作流程应当刻意保持简单：

1. 在 Web Studio 创建 Project 并开始任务。
2. 任务需要编辑时，只需选择一次“完全访问”。它表示当前 Runtime/Session
   的整机访问范围，并在启用前要求明确的高风险确认；同时仍保留不可绕过的
   Host Deny、Plan 锁和安全策略规定的 Shell 确认。若需要最小权限，另有“项目访问”。
3. 如果任务涉及更多目录，在 Project 编辑中将目录添加为“可编辑工作区”或“仅参考目录”。
   Agent 接收到的是由此生成的不可变清单。
4. 如果目录只是在消息中被提到，默认将其视为一次性参考请求。要求同步或更新时，必须
   形成明确的路径范围写入意图。如果整机范围的“完全访问”已经启用，Host 可以在该范围
   和剩余保护下准入；否则请求明确批准，或提示将它加入 Project 作为可编辑目录。模型消息
   中的路径不能静默扩展 Project。
5. 通过 `/plan` 进入计划模式，通过 `/goal` 进入 Goal 模式。Goal 对当前 Thread
   会话持续显示在对话上方，并提供开始/恢复、暂停、更新和删除操作。

Plan 和 Goal 继续沿用现有的 Thread 边界。本提案不创建第二个 Workflow
Service、第二个 Turn Loop，也不引入通用策略/插件框架。

### 直接切换：不提供迁移兼容层

本提案只定义一套新的控制契约和一套规范存储布局。新 Runtime 不读取、导入、翻译、写入
或删除旧的 Web `~/.mini-agent/state.json`、旧 Web checkpoint、`profile`、
`profile=auto`、`interactive`、`ask`、`auto`、旧的 `remember` 或布尔/字符串批准格式。
这些旧产物和输入不属于新 Runtime 的范围。唯一狭窄的迁移例外是输入边界收到旧的
`turbomode`/`Turbomode`：可以一次性映射为“完全访问”/`SecurityPreset::FullMachine`，
随后立即丢弃。它不能持久化，不能作为 Runtime/Profile 身份暴露，也不能被新的公开协议接受。
新实现只从规范 Session Store 和新的 `~/.mini-agent/web/state.json` 开始，不增加通用迁移解析器、
导入器、回退路径或兼容层。

## 当前证据与问题

当前实现已经具备所需组件，但它们的权威边界和语义分散在三个仓库中：

1. `ApprovalController` 会把所有批准过的操作缓存到 `ApprovalStore`。因此，
   原本想表达“仅允许一次”的 UI 选择可能表现成“始终允许”。
2. `ApprovalRespondParams` 携带 `remember`，但 App Server 的响应路径目前只消费
   布尔批准结果。用户选择的访问范围和生命周期在 Host 恢复执行前已经丢失。
3. `mini-agent-web` 还维护 `_remembered_approvals`，并将待批准请求广播给所有
   WebSocket 客户端。这造成第二个批准权威，也允许跨 Thread 的展示或响应竞争。
4. `profile` 在 SDK/App Server 初始化时生效。Web 设置更新不会重建运行中的
   Host Profile，因此 UI 中的 Profile 变化可能不会改变实际行为。
5. Web UI 将 Plan 和 Goal 当作类似 Profile 的选择项，而 App Server 正确地将
   Plan 建模为 Thread 设置，将 Goal 建模为独立的 Thread Goal 生命周期。
6. 多个 Web 调用没有携带当前 Thread ID，并回退到 `default`。因此 Plan、Goal、
   Builtin 选择和批准展示可能指向与 Studio 当前显示不同的 Thread。
7. 当前 Web slash 命令目录只覆盖了目标控制面的部分内容。`/goal`、`/compact`、
   `/mcp`、`/review`，以及有界的 Skill/Plugin 发现还没有形成一致的命令契约。
8. Rust `SessionStore` 已经在 `~/.mini-agent/sessions/` 下持久化追加式日志、
   settled checkpoint、summary、signals、prompt context、attachments 和锁；
   Web Studio 另外持久化 `state.json`、每个 Thread 的 checkpoint 和工作区日志，
   因而出现两个可能的历史/恢复权威。
9. Web Studio 的 Project API 接受主目录和多个 `source_folders`，但当前 SDK 启动
   使用进程 cwd，没有将完整的 Project 根目录清单传入 Host 工具构建。UI 展示的
   Project 范围因此大于 agent 实际获得的工作区范围。
10. 当前 Web 元数据没有提供规范的 Project Session 目录，缺少运行状态、暂停原因、
    锁定/只读状态、活动 Turn 和可恢复性。全局 client/active state 不足以支持在
    历史、运行中和暂停 Session 之间切换。
11. Core 默认 `max_steps` 为 8。Web Studio 当前将技术性的 `Turn Settled`/`step_limit`
    结果展示得像 Turn 被中断，却没有说明这是有界 Turn 正常结束但结果可能不完整。

相关的现有责任方包括：

- `crates/mini-agent-capabilities/src/workspace/approval.rs`：Host 侧批准执行和缓存批准；
- `crates/mini-agent-app-server/src/lib.rs`：批准 Broker 和生命周期；
- `crates/mini-agent-app-server-protocol/src/lib.rs`：批准、Thread 设置和 Goal 的线协议类型；
- `server/session_manager.py`：SDK 生命周期、Web 传输和当前重复的批准状态；
- `sdk/python/src/mini_agent/client.py`：Stdio JSON-RPC 和通知适配；
- `frontend/src/App.jsx`、`frontend/src/utils/slashCommands.js`：Studio 控制状态和本地命令分发。

## 目标语义

### 不引入独立的 Profile 层

产品和协议不应将 `Profile` 建模为独立的控制平面层。Runtime 启动时只接收实际需要的
有界输入：Provider、Prompt/规则来源、扩展、工具、Sandbox/安全、访问范围和不可变的
WorkspaceSpec。如果实现上需要将这些值放进一个内部结构，它也只是私有 Runtime 配置对象，
不是面向用户的 Profile、可选择的工作流或持久化的 Session 轴。

Studio 应显示当前 Mode 和 Goal 状态，但 Mode 的启动应遵循用户明确输入的 slash
命令，不再要求额外的 Mode 选择器：

| Studio 标签 | 规范操作 | 运行时含义 |
| --- | --- | --- |
| Interactive / Chat | 普通输入，或使用 `/plan off` | 普通用户驱动的 Turn |
| Plan | `/plan` 或 `/plan on\|off` → `thread/settings/update` | 读多写少的探索 Runtime；允许 scratch 脚本/输出和 `plan.md`，正式项目修改延后 |
| Goal | `/goal ...` → `thread/goal/set|get|clear` | Thread 所属的自主生命周期和验证器 |

当前代码不足以支持三个面向用户的不同预设：Host 的 `interactive`、`ask` 和 `auto`
内置配置使用相同的 named 基线，除非被工作区配置覆盖。在 stdio App Server 路径中，
选择 `auto` 也不会移除 `HarnessConfig` 默认的 8 步限制。旧的内嵌/本地
`automatic` 路径改变的是循环预算和上下文压缩，这属于执行实现细节，不是用户人格。

因此，Web Studio 和 REPL 应移除 Profile 选择器以及 `profile=auto` Runtime 选项。新的公开
契约直接拒绝 Profile 形态的输入，不做翻译；任何 Runtime 或 Session 都不保留 Profile 身份。
面向用户的整机访问名称只有“完全访问”，内部由 `SecurityPreset::FullMachine` 支持。旧的
`turbomode`/`Turbomode` 只允许在迁移输入边界作为指向该访问预设的一次性别名；它不能残留为
Runtime 或 Profile 身份。面向用户只独立暴露真正的决定：

- 访问：`project` 或 `machine`；
- 批准策略：`per_action`、`auto_approve` 或 `strict`；
- Mode：通过 `/plan` 控制 Chat/Plan；
- 所选 Thread 的 Plan Runtime 状态和 Goal 生命周期：通过 `/goal` 和持久顶部栏控制。

启动输入变化必须通过 Runtime 重建或明确的下一个 Runtime 操作生效，不能只更新 Python
偏好字段而不改变实际 Runtime 行为。

### Approval Policy、访问范围与生命周期不同

`per_action` 表示何时必须询问，不表示每个操作都会自动获准。工具暴露范围、Host
安全策略、Plan 锁和人工批准仍然是相互独立的门槛。

公开决策词汇为：

```text
policy: per_action | auto_approve | strict
access:  project | machine
lifetime: once | session
```

规则：

- `per_action` 表示每个需要批准的操作都可以询问；不表示每个操作都会自动获准；
- `once` 只批准当前 `requestId`；
- `session` 在当前 Thread 或 App Server Session 内，对相同的有界操作类别生效，具体生命周期按选定范围执行；
- `project` 将文件/进程准入限制在不可变的 Project WorkspaceSpec 内；
- `machine` 表示当前 Runtime/Session 的整机访问范围；
- 明确的安全 Deny 始终优先于 UI 批准；
- Plan Mode 的修改锁不能被批准绕过；
- `auto_approve` 可以跳过可准入操作的交互等待，但不能覆盖明确 Deny 或 Plan 限制；
- `strict` 在不打开用户批准请求的情况下拒绝需要批准的操作。

批准键必须包含有界的所有者和操作身份，例如：

```text
(access scope, thread/session scope, tool name, normalized action class)
```

不能将原始的无限制命令、密钥或任意 Prompt 文本变成批准规则。

### 产品预设：“项目访问”与整机范围的“完全访问”

普通用户路径不应要求为每次工具调用重新选择生命周期或范围。Studio 面向用户只保留少量预设。
默认策略仍为 `per_action`；出现批准请求时，用户可以为当前 Runtime/Session 启用一次整机
范围的“完全访问”，不需要配置其他 Runtime 预设，也不需要反复回答同一类请求：

```text
项目访问：
  访问范围：当前 Project 的 WorkspaceSpec
  覆盖范围：primaryRoot + 标记为 editable/reference 的 associatedRoots

完全访问：
  访问范围：当前 Runtime/Session 的整机范围
  启用条件：启用前明确确认高风险访问
```

“完全访问”是面向用户的整机访问名称，不能重新命名或文档化为 Project 范围权限。当前
实现基线是 `SecurityPreset::FullMachine`：文件操作可以访问 Project 工作区以外的路径，
但硬性安全 Deny 仍然有效，Shell 操作仍可能要求确认。如果未来要提供连 Shell 询问也移除的
产品选项，必须使用单独名称并明确确认为高风险预设，不能藏在 `per_action` 或 `profile=auto` 后面。

“项目访问”仍受当前 Project 不可变 WorkspaceSpec、明确的安全 Deny、Plan 锁和已配置工具范围
约束。批准策略独立于这两个访问预设。UI 详情必须明确写出“当前 Project 工作区”或“整台机器”，
并说明仍然存在的 Deny/确认保护。

Project 编辑是持久扩展工作区的正式方式，应提供两种明确的目录意图： “添加为参考”和
“添加为可编辑工作区”。普通消息中提到的路径只是临时上下文，不是工作区变更。
`reference <path>` 可以授予有界的一次性读取；`sync/update <path>` 必须形成明确的路径范围
写入意图。如果整机范围的“完全访问”已经启用，Host 可以在 machine 范围和剩余保护下准入
该意图；否则必须请求明确批准，或提示将该路径添加为可编辑 Project 根目录。模型不能仅通过
在自己的消息中输出路径来持久扩展 Project。

### Thread 所属 Runtime 与内部安全保护

当前 `HarnessConfig::default()` 将 `max_steps` 设置为 8。这只是实现层的循环保护，
不是有价值的用户任务设置。应将 `max_steps`/`step_limit` 从 Web Studio、SDK 和 REPL
的普通控制契约中移除；不要把“提高步数预算”作为常规恢复操作。

Core 仍然需要不可由用户配置的安全保护，以防 Provider/Tool 无限循环。该保护属于
Core 的 limits，并且必须通过新的类型化 `runtime_guard` 诊断暴露，而不是保留旧的兼容
分类。UI 不能把它展示成“8 步”“Turn 被中断”或“任务失败”，却不说明是安全保护触发。

Plan 和 Goal 各自管理自己的 Runtime 生命周期：

- Chat 运行一次用户请求的 Core Turn，在 Provider 完成、用户明确取消或异常 Runtime
  保护触发时结束；
- Plan 管理所选 Thread 的 planning Turn、Plan 锁和状态，不依赖全局的、用户可调的步数预算；
- Goal 管理所选 Thread 的 continuation、验证、暂停/恢复和完成。它可以有内部循环保护，
  但不把 `max_steps` 作为 Goal 进度语义暴露；
- App Server 只负责路由和串行化这些 Thread 所属的 Runtime，不增加全局 Profile 或步数预算层。

“settled”仍然只是内部持久化/生命周期术语，不等于“completed”。App Server 和 SDK
应保留最终语义状态以及用于调试的诊断字段。Web Studio 只在确实发生时显示“已完成”、
“已取消”、“等待批准”，或“运行保护已触发，请检查或重试”。原始实现计数不属于公开
状态契约，内部诊断可以为调试记录它们。

`turn_finished` 通知或其 SDK 投影必须携带最终的语义状态，并在可用时携带原始诊断，
这样 Web Studio 就不会从一个普通的流关闭事件推断“中断”。继续必须是明确的下一次
Turn 或 Goal 操作，不能变成无限制自动循环。

### Goal 是围绕目标运行的自主 Runtime

Goal 应被理解为一个长期存在、围绕目标推进的 Agent/Copilot Runtime，而不是
`profile=auto` 开关，也不是一个变大的单次 Turn。它的进度单位是同一 Thread 内跨多个
Turn 的“执行—验证—继续”循环：

```text
/goal <objective>
    → 执行一个 Goal Turn
    → 持久化 settled checkpoint 和证据
    → 执行独立的 Goal verifier
    ├─ approved / 目标完成 → Goal complete
    ├─ rejected 或证据不足 → 在同一 Thread 调度下一个 Turn
    ├─ 用户暂停 → paused
    └─ blocked / usage 或安全保护触发 → 以可行动状态停止
```

层次之间的责任必须保持清晰：

- Core 负责一个有界 Turn，以及该 Turn 的工具、事件和历史语义；
- App Server 负责 Goal 调度、串行化、verifier 分发、过期 checkpoint 拒绝和按 Thread
  路由的通知；
- Host 负责 Goal 状态以及有界的证据/计划持久化；
- SDK 和 Web Studio 投影同一份 Goal 状态，并提供开始/恢复、暂停、更新和清除操作。

每次 continuation 都创建新的 `turnId`，但保留相同的 `threadId`、`sessionId`、工作区绑定、
批准范围和 Goal 身份。验证被拒绝不应被展示为用户可见的 Turn 失败；它是下一次尝试的证据，
Goal Prompt 必须要求 Agent 处理 verifier 的发现。只有 verifier 证据可以证明完成，不能只接受
模型声称“已经完成”。

Goal 可以使用 milestone 作为内部进度/证据 checkpoint，但 milestone 不是第二套用户工作流。
`max_steps`、单个 Turn 的步数预算和 loop 次数不能成为用户理解 Goal 的任务模型。只保留
不可由用户配置的 runaway/安全保护，以及必要的有界资源/用量停止条件。这些停止必须产生
`blocked`、`usage_limited` 或清晰的运行保护诊断，并且可以恢复或检查；不能无解释地显示为
“Turn 被中断”。

Plan 和 Goal 有意不同：Plan 是用户通过 slash 启动的、读多写少的探索 Runtime，产出计划文件；
Goal 是用户通过 slash 启动的自主 Runtime，可以在获得批准后执行项目修改、验证结果，并跨 Turn
持续推进，直到完成、暂停或触发终止保护。两者都不需要 Profile 选择器。

## 所有权模型

| 层 | 负责 | 不负责 |
| --- | --- | --- |
| Core | Turn Loop、内部安全保护、停止分类、事件、历史写回 | UI、批准对话框、持久化、Runtime 启动组合 |
| Protocol | 类型化 Thread 设置、Goal、批准请求/响应/解析 | 传输特定的 UI 状态 |
| Capabilities/Host | 工具准入、安全 Deny/Ask/Allow、Plan 锁、有范围的批准执行 | WebSocket、React、JSON-RPC 解析 |
| App Server | Thread 状态、Runtime Actor 排序、批准 Broker、公开通知 | 第二个执行循环或任意 Prompt 替换 |
| Python SDK | JSON-RPC 传输、类型解析、批准回调适配 | 策略权威或持久批准决定 |
| FastAPI | HTTP/WebSocket 映射和连接路由 | 独立批准缓存或 Runtime 语义 |
| Web Studio | 渲染状态和提交明确的用户决定 | 猜测批准状态或应用隐藏策略 |

权威关联链为：

```text
projectId → workspaceId → sessionId → threadId → turnId → callId/itemId → requestId → resolved decision
```

所有批准通知和响应都必须依据这条链进行校验。

## 建议的 Protocol 变更

### Thread 设置

在现有 `thread/settings/update` 契约中增加可选的类型化 `approvalPolicy`；将
`collaborationMode` 和有界的 `builtinTools` 保持在同一个 Thread 边界中。结果和
`thread/settings/updated` 通知必须返回生效后的值以及现有的 `stateRevision`。

不要重新引入聚合的 `workflow/state` 线协议权威。SDK 可以提供组合 Settings 和
Goal 的展示辅助方法，但 App Server 仍通过各自独立的规范方法保持权威。

### 不使用 Profile 的 Runtime 启动输入

App Server 初始化和 Runtime 创建应直接接收类型化输入：Provider/Model 选择、有界的工具和
扩展选择、Prompt/规则来源、Sandbox/安全、访问范围和 WorkspaceSpec。新的公开启动契约中
不再有 `profile` 字段。Runtime/Session 创建后，这些启动输入对该身份不可变；变更必须明确
重建 Runtime 或新建 Session。Profile 形态的启动输入直接拒绝，不解析也不翻译。

### Approval 生命周期

保留现有方法，通过增加有界字段来扩展，不创建第二套批准传输：

```json
{
  "requestId": "approval-1",
  "threadId": "thread-1",
  "turnId": "turn-1",
  "callId": "call-1",
  "toolName": "shell",
  "action": "shell command `cargo test`",
  "accessScopes": ["project", "machine"],
  "lifetimes": ["once", "session"]
}
```

```json
{
  "requestId": "approval-1",
  "approved": true,
  "access": "machine",
  "lifetime": "once",
  "reason": ""
}
```

`approval/resolved` 必须回显最终访问范围、生命周期和关联字段。新的线协议只接受类型化的
`access` 和 `lifetime` 字段；旧的 `remember` 和只有布尔值的批准响应直接拒绝。App Server
不得再静默地记住每次成功的布尔批准。

### 显式压缩

通过诸如 `thread/compact` 的显式 Thread 控制方法暴露已有的、有界的 Core/App Server
压缩路径。它必须持久化生成的 checkpoint，并发出正常的、有界的
`ContextCompaction`/Turn 投影；不得创建第二个历史存储或可向模型暴露的无限制输入路径。

## 规范存储、工作区范围与 Session 生命周期

### 决策：统一权威，不统一所有文件

存储模型应围绕 Rust `SessionStore` 统一，但 Web Studio 的展示状态仍应保持为更小的
独立关注点。关键规则是：持久化对话状态只有一个权威，而不是把 Runtime、UI 和项目
数据塞进一个巨大的 JSON 文件。

Rust Session Store 已经负责追加式 Session 日志、settled checkpoint、summary、signals、
prompt context、attachments 和 Session 锁。Web Studio 当前另外创建
`~/.mini-agent/state.json` 和 `~/.mini-agent/checkpoints/<threadId>.json`。后者不能继续
作为第二个恢复权威。Web 元数据可以缓存引用和展示字段，但 transcript、items、checkpoint、
resume 和锁状态必须通过 App Server/SDK 适配器从规范 Session Store 获取。

目标目录结构如下：

```text
~/.mini-agent/
├── sessions/
│   └── <workspaceId>/
│       └── <sessionId>/
│           ├── workspace.json       # 不可变 WorkspaceSpec 快照
│           ├── session.jsonl        # 持久化追加式权威
│           ├── summary.json         # 有界的 list/read 投影
│           ├── signals.json         # 有界的生命周期信号
│           ├── prompt_context.json
│           ├── session.lock         # 单写入者所有权
│           ├── attachments/
│           └── plan/
│               ├── plan.md             # 保留的明确 Plan 产物
│               ├── scratch/            # 可清理的探索脚本/输出
│               └── cleanup.json        # 有界的清理清单
├── web/
│   ├── state.json                   # 项目、UI 偏好、选择状态
│   └── session-index.json           # 可选，只能是派生/缓存索引
└── logs/
    └── <workspaceId>/<sessionId>/   # 诊断日志，不是对话状态
```

这是面向新实现的直接切换。目标 `sessions/` 布局下的规范 Session Store 记录是唯一的
持久化对话和恢复权威。旧的根目录 Web `state.json` 和旧 Web checkpoint 目录不会被读取、
导入、翻译、写入或删除；它们不属于新 Runtime 的范围，也不参与 Project 或 Session 发现。

新的 `~/.mini-agent/web/state.json` 是有版本的 Web manifest，只保存项目注册表、UI 偏好和
`projectId`、`workspaceId`、`sessionId`、`threadId` 等引用。它不是旧的根目录 `state.json`，
也不能保存 transcript 或重复的 Thread checkpoint。

Plan scratch 必须放在规范 Session 目录下，而不是 Project 根目录或 Web 自有状态中。
它的路径由 Session/Plan Runtime 生成并保持有界。`plan.md` 是保留的 Plan 产物；scratch
内容按照 `cleanup.json` 清单清理。

新契约不包含 `~/.mini-agent/profile/` 目录、每个 Session 的 Profile 文件或项目级
`.agents/profile.json` 输入。Profile 文件不会被读取或翻译，Profile 身份也不会写入规范
Session 历史或 Web 状态。

### Project、WorkspaceSpec 与 Session 绑定

Web Studio 的 Project 是面向用户的身份；它必须解析成明确的 Runtime `WorkspaceSpec`：

```text
Project
└── WorkspaceSpec
    ├── primaryRoot
    ├── associatedRoots[] (reference | editable)
    └── 有界的写入/执行策略
```

`workspaceId` 根据规范化、canonical 化且有序的根目录清单和其 schema 版本生成稳定
标识，不能基于展示名称生成。每个 Session 都在 `workspace.json` 中保存不可变副本，
因此 Project 后续修改关联目录不会静默改变旧对话的含义。重新绑定已有 Session 必须
通过明确的 fork/新建 Session 完成，不能原地修改其工作区。

当前 Project API 已经接受主目录和多个 `source_folders`，但 Web Runtime 当前用进程
cwd 启动 SDK，并没有将完整清单传入 Host 工具构建。必须打通以下链路：

```text
Project source_folders
    → FastAPI 中的 WorkspaceSpec
    → SDK/App Server 可信 Runtime 配置
    → Host ToolBuildRequest.extra_read_roots + 有界的写入根目录
    → Capabilities Workspace 路径准入
```

根目录清单属于控制面配置，不是模型生成的输入。创建 Runtime 前必须对其校验、限制、
canonical 化，并将其纳入 Runtime/Session 身份。

### 工作区范围规则

初始策略有意采用非对称设计：

| 根目录类型 | 读取工具 | `apply_patch`/创建 | Shell cwd | 默认写入权 |
| --- | --- | --- | --- | --- |
| `primaryRoot` | 允许 | 受 Plan/安全/批准约束后允许 | `primaryRoot` | 有 |
| `associatedRoots[]: reference` | 通过 canonical 路径检查后允许 | 拒绝 | 不改变 cwd | 无 |
| `associatedRoots[]: editable` | 通过 canonical 路径检查后允许 | 通过明确的 Project 或 machine 访问，并受 Plan/安全/批准约束 | 不改变 cwd | 仅该目录有 |
| Session 所属的 `planScratchRoot` | 有界的 Plan 探索 | 只允许临时脚本/输出和清理 | Plan 中为 `planScratchRoot` | 仅临时 |

关联目录不是“所有项目目录都可写”的隐式授权。Project 编辑必须通过将目录添加为
`reference` 或 `editable` 来明确表达意图。Reference 根目录传递到现有的有界
`extra_read_roots`/`Workspace::with_read_roots` 路径；editable 根目录还需要明确且有界的
写入根目录准入路径。两种目录都必须传递到 Runtime 创建，而不是只显示在 UI 中。嵌套或
重复的根目录必须规范化；符号链接和逃逸情况必须在 Runtime 创建前拒绝，或归并到 canonical
根目录清单中。

普通 Chat 和 Goal 的 Shell 从 `primaryRoot` 启动；Plan 探索从 `planScratchRoot` 启动。
由于 Native 子进程可以发起任意文件系统写入，文件工具的检查无法观察这些写入，因此
Shell 访问关联目录不能被视为写入授权。在 Plan 中向 scratch 之外写入属于正式 Project
修改，不属于探索。严格的执行模式必须使用配置好的进程 Sandbox，或要求一个明确描述
受影响路径的批准；UI 不能声称 `per_action` 单独就能保证 Native Shell 的路径写入安全。
Docker/Sandbox 挂载必须包含声明的 Project 根目录和有界的 scratch 根目录，并在 Sandbox
支持时保持参考目录与可编辑目录的区别。

根目录清单需要小而明确的 Protocol 限制，包括关联目录数量、路径长度和序列化后的总字节数。
具体常量属于 Protocol 实现批次；目录列表绝不能作为无限制的模型可见上下文。

### Session 目录与生命周期

产品必须区分 Runtime 实时状态和 Goal 状态：

| 维度 | 值 | 权威 |
| --- | --- | --- |
| Runtime | `running`、`idle`、`closed` | App Server Actor/Turn 状态 |
| Session UI 投影 | `running`、`paused`、`historical`、`locked` | Web 对 Runtime、锁和 summary 的投影 |
| Plan | `none`、`exploring`、`settling`、`cleanup_pending` | 所选 Thread 的 Plan Runtime |
| Goal | `none`、`active`、`paused`、`completed`、`failed` | Thread Goal 生命周期 |

“暂停 Session”表示一个可恢复的 Session，其活动 Turn 已被中断或明确挂起；它不能与
`Goal.status=paused` 混淆。没有明确暂停原因的 idle Session 可以显示为 historical/idle；
只要锁可获得，其持久历史仍然可恢复。closed Session 保留用于历史查看，只能通过明确的
attach/resume 操作恢复。

单写入者规则按 Session 生效：`session.lock` 和 App Server 所有者阻止两个 Runtime 同时
修改同一 Session。多个 Session 可以并发运行。Web 标签切换只改变当前选中的 Session 和
事件订阅，不能取消另一个 Session 的活动 Turn。批准请求、活动 Turn ID、Goal 控制和
待处理 UI 状态必须按 `sessionId`/`threadId` 建立，不能由一个全局 Web 值承载。

权威读取路径应当有界且明确：

```text
App Server:  session/list(workspaceId, cursor, limit)
            session/inspect(sessionId)
            thread/read + thread/items/list 读取 settled 内容
SDK:        list_sessions / inspect_session / resume_thread 适配器
FastAPI:    project session list/read/resume/interrupt 路由
Studio:     按 Project 展示 Running / Paused / History 分组
```

具体方法名可以遵循现有命名约定，但契约必须提供：

- 有界、支持 cursor 的 Session 列表，包含 `sessionId`、`threadId`、`projectId`、
  `workspaceId`、标题/摘要、`updatedAt`、Runtime 状态、Goal 状态、活动 Turn ID、
  checkpoint 序号、锁所有者/只读标识和 `resumable`；
- 从规范 Session Store 读取历史，绝不能从 Web checkpoint 副本读取；
- 运行中的 Session 通过通知读取实时内容；只有 settled checkpoint 才能作为恢复权威；
- 明确的 resume/attach 行为：如果另一个所有者持有锁，则以只读失败，不强行合并或窃取 Session；
- interrupt/pause 和 resume 的状态转换始终关联同一个 Thread/Session，并在刷新后可见。

对于不同的 WorkspaceSpec，Web 后端必须维护相互独立的 Runtime handle，或维护独立的
App Server Runtime 实例，因为一个可变的进程 cwd 不能表达多个 primary root。在同一个
WorkspaceSpec 内，只有当 App Server 路由和 SessionStore 锁仍然按 Thread/Session 隔离时，
才允许复用同一个 Runtime 来复用多个 Thread/Session。当前 Web 的 singleton client 和
全局 `_active_turns`/批准展示只是待移除的实现约束，不是产品语义。

### 历史读取与会话切换行为

预期用户流程如下：

1. 选择 Project 时加载该 Project 的不可变 WorkspaceSpec 和有界 Session 目录。
2. 选择历史 Session 时读取规范 summary 和 settled items，然后附加对应 Thread，不创建 Web 副本。
3. 选择运行中的 Session 时订阅其 Thread 事件并展示活动 Turn；不能把部分 Turn 重放成 settled 历史项。
4. 选择暂停 Session 时展示最近的 settled checkpoint、暂停原因和 Resume 操作。Resume 必须
   重新取得锁并从规范 checkpoint 继续，不能重复最后一条用户消息。
5. 切换离开某个 Session 时，独立的运行中 Session 继续运行；但批准决定只有在携带准确的
   关联字段并且所有者有权时才能提交。返回时从 App Server 状态/事件和规范 summary
   重新协调 UI，而不是依赖过期的 React 状态。

如果 Session 被其他进程锁定，Studio 可以浏览其持久历史，但必须标记为只读并禁用
resume/批准控制。如果 Runtime 断开连接，状态变为 `unknown/reconnecting`，直到 App
Server 完成协调；UI 不能仅因为 WebSocket 断开就猜测它是 `paused`。

## Studio 与命令契约

Web UI 应为每个活动 Thread 保持一个状态对象，并按 `requestId` 保存待批准请求映射。
通知必须按 `threadId` 路由；其他 Thread 的待批准请求不能出现在当前 Thread 的输入框中。

批准面板应提供：

```text
请求批准
允许一次
本会话允许
项目访问（当前 Project 工作区）
完全访问（整机范围，高风险）
拒绝并说明原因
```

“完全访问”必须显示为整机范围，并显示高风险确认和仍然存在的 Deny/确认保护；“项目访问”
必须标出当前 Project 工作区以及 reference/editable 目录。两个访问范围不能合并成模糊的
“允许全部”标签。

加号菜单和 slash 命令应调用类型化 API：

| 命令 | 操作 |
| --- | --- |
| `/status` | 读取 World、当前 Thread 设置、Goal 和 MCP 状态 |
| `/compact` | 调用显式 Thread 压缩 |
| `/plan on\|off` | 启动/停止当前 Thread 的读多写少探索 Runtime |
| `/goal <objective>` | 创建并启动有界的 Thread Goal |
| `/goal start\|resume` | 开始或恢复当前 Thread Goal |
| `/goal pause` | 暂停当前 Thread Goal，但不删除目标 |
| `/goal update <objective>` | 更新当前 Goal 目标 |
| `/goal clear` | 清除当前 Goal |
| `/mcp` / `/mcp retry` | 读取或重试 MCP 状态 |
| `/review` | 启动 allowlist 中的 Review Workflow |
| `/skill` / `/plugin` | 只列出或选择已发现并批准的条目 |

控制命令不能变成普通模型 Prompt。Review、Skill 和 Plugin 激活必须保持 allowlist 化
并且有界。任意扩展指令的动态热加载不在本提案范围内；如需新的 Runtime 激活契约，
必须单独记录变更准入。

### 持久 Goal 顶部栏

通过 `/goal <objective>` 设置 Thread Goal 后，Studio 必须在当前选中 Thread 的对话上方
渲染持久的 Goal 顶部栏。顶部栏是规范 Goal 状态的投影，不是由 UI 自己维护的第二份
Workflow 记录；在刷新和 Session 切换后仍应保留。

顶部栏必须提供：

- 有界目标和当前状态（`active`、`paused`、`blocked`、`usage-limited`、
  `budget-limited` 或 `complete`）；
- Goal 未运行时的开始/恢复操作；
- Goal 运行时的暂停操作；
- Update/Edit，通过明确的 Thread Goal 更新提交；
- Delete/Clear，在正常确认后清除当前 Goal；
- 可用时显示最近的 verifier/continuation 摘要，但不能在顶部栏中嵌入无限制 transcript。

`/goal <objective>` 创建并启动 Goal。`/goal pause` 保留目标，供之后的 `/goal start` 或
`/goal resume` 使用；`/goal clear` 清除当前 Goal 配置，但 Session 历史仍然保留。切换
Thread 时，顶部栏必须切换到所选 Thread 的 Goal，不能展示其他 Thread 的目标或控制项。

## 实施批次

### Batch 0 — 契约与 Trace Fixture

记录用户控制轴、访问/批准词汇、Thread 所属 Plan/Goal Runtime 生命周期、内部安全保护
语义和关联规则。定义替代 Profile 的直接启动输入，以及替代常规 `max_steps` 的不可配置
Core 安全保护。在改变执行逻辑前增加离线 Protocol Fixture。不需要 Provider 调用。

### Batch 1 — 规范存储、WorkspaceSpec 与 Session 目录

- 定义有版本的 `WorkspaceSpec`、稳定的 `workspaceId`、每个 Session 不可变的
  `workspace.json` 和有界的根目录清单限制；
- 将 Web Studio 的 Project 主目录和关联目录，以及每个目录的 `reference`/`editable` 意图，
  一路传递到 Host 工具构建；
- 将主目录设为 agent cwd/写入根，并执行明确的 reference/editable 根目录准入；增加路径、
  符号链接、Sandbox 和 Native Shell 边界测试；
- 实现相互独立的“项目访问”和整机范围“完全访问”预设；保留
  `SecurityPreset::FullMachine` 的保护，并区分已配置根目录与用户消息中临时提到的路径；
- 增加基于规范 `SessionStore` summary 和锁的有界 App Server/SDK/Web Session list/inspect 路径；
- 将新的 Web 状态放到 Web 自有子目录，停止写入重复的 Web checkpoint，并确保旧 Web 状态/
  checkpoint 路径永远不会被读取、导入或写入；
- 暴露按 Session 的 Runtime/Goal 状态、活动 Turn、锁和可恢复字段；移除一个全局 Web client
  或一份全局 active state 表示所有 Project 的假设；
- 在启用并发运行/暂停 Session 切换前，实现按 Project 的历史选择和 attach/只读行为。

这一批是批准范围所依赖的存储和身份前置条件。同时必须决定：一个 App Server 进程是否
复用不可变 WorkspaceSpec，或者 Web 是否为每个 WorkspaceSpec 保留一个 Runtime handle；
一个可变的共享 cwd 不能作为第三种状态。

### Batch 2 — 批准正确性与路由

- 让 `once` 真正只生效一次；
- 让 `session` 生命周期以及 `project`/`machine` 访问范围成为明确的有界决策条目；
- 保持 Deny 和 Plan 锁的优先级；
- 将访问范围和批准生命周期从 App Server → SDK → FastAPI → Studio 传递完整；
- 删除 Web 的重复批准权威；
- 按 Thread 和 request ID 路由待批准请求；
- 增加 Core/Host/App Server 边界测试和一个 SDK/Web 批准场景。

这是第一道实现批次，也是安全正确性门槛。

### Batch 3 — 活动 Thread 控制

- 所有 Plan、Goal、Builtin 和 status 调用传递活动 `threadId`；
- Thread 选择后独立恢复 Settings 和 Goal；
- 发出并消费权威的 Settings/Goal 通知；
- 将 Plan Runtime 状态和 Goal 生命周期绑定到所选 Thread；完全移除在线控制路径中的 Profile。

### Batch 4 — SDK 与 FastAPI 收敛

- 增加类型化的批准请求/决定模型；
- 只使用类型化的批准回调和决定；删除布尔/字符串批准回调路径；
- 确保重连、超时、取消和延迟响应行为是确定性的；
- 删除过时的 Web 侧 remembered approval 状态和宽泛广播。

### Batch 5 — Studio 控制面

- 显示当前 Mode 和 Goal 状态，并将 `/plan`、`/goal` 作为明确的启动控制；
- 将 Security/Approval 保持为独立设置和批准面板，并明确区分“项目访问”和整机范围的“完全访问”；
- 从普通路径和 Runtime 控制路径移除 `interactive`/`ask`/`auto` 及 `profile=auto`；高级 status
  只展示有效的有界 Runtime 输入；
- 将异常的 Runtime 安全保护展示为保护诊断，不能展示为“8 步”“达到步骤上限”或“中断”；
- 实现加号菜单和批准面板；
- 增加当前 Thread 的 Mode、Goal、批准策略、MCP 和 Runtime revision 状态指示器。

### Batch 6 — Slash 与 Review Workflow

优先实现 `/status`、`/compact`、`/plan`、`/goal` 和 `/mcp`。只有在对应能力证据被接受后，
才通过现有 allowlist 增加 `/review` 和 Skill/Plugin 选择。

## 验收证据

提案只有在以下场景全部满足后才能移动到 `implemented/`：

1. `allow once` 使第二次相同操作再次请求批准。
2. `session` 批准只影响目标 Thread/Session。
3. `project` 访问/批准不影响其他 Project 或无关的操作类别。
4. 任何 UI 范围都不能覆盖安全 Deny。
5. Plan Mode 允许读取声明的根目录、执行有界探索命令、在 `planScratchRoot` 下创建临时
   脚本/输出并保留 `plan.md`；正式 Project 源码/配置修改仍然受 Plan 门槛约束。
6. Goal 的 set、update、clear、resume、verifier 和 continuation 事件保持有序并限定于 Thread。
   一个 settled Goal Turn 之后必须有 verifier 证据；不完整/拒绝的结果在相同
   `threadId` 上调度新的 Turn，而 approved 结果结束 Goal。
7. 两个活动 Thread 不能展示或解析彼此的批准请求。
8. Web Studio 和 REPL 没有 Profile 控制，也不保留 Profile 身份。Profile 形态的输入，
   包括 `profile=auto`，直接拒绝。唯一的迁移例外是 `turbomode`/`Turbomode`：一次性映射
   为“完全访问”/`SecurityPreset::FullMachine` 后丢弃，不能改变 Mode 或循环语义。
9. `/status`、`/plan`、`/goal`、`/mcp` 和 `/compact` 使用控制 API，而不是意外变成模型 Prompt。
10. `item/started`、批准事件、`approval/resolved`、`item/completed` 和 `turn/read` 保持相同的有界 call identity 和最终结果。
11. 一个包含主目录和多个关联目录的 Project，可以从每个声明的目录读取文件；`reference`
    目录只读，`editable` 目录只有通过明确的 Project 或 machine 访问，且继续受
    Plan/security/approval 门槛约束时才可写；Plan 临时写入隔离在 `planScratchRoot` 中。
12. 面向用户的“项目访问”预设只覆盖声明的 Project 工作区；“完全访问”明确覆盖当前
    Runtime/Session 的整机范围。两者都保留硬性安全 Deny，Shell 确认和其他保护必须在
    详情中可见。
13. 修改 Project 的关联目录不会改变已有 Session 的 `workspace.json`；新 Session 或明确 fork
    才能获取新清单。
14. 只在消息中提到的路径可以作为有界参考上下文使用；要求同步/更新时必须形成明确的
    路径范围意图；已启用的整机“完全访问”可以在其保护下准入，否则必须明确请求批准或
    将它加入 Project 根目录。
15. 旧 Web `state.json` 和 checkpoint 路径永远不会被读取、导入、翻译、写入或删除。Project
    历史和恢复只使用规范 `SessionStore` 以及新的派生 Web manifest/index。
16. Project 历史可以列出有界的 historical、running、paused 和 locked Session，并保持稳定的
    `sessionId`/`threadId`/`workspaceId` 关联。
17. 切换到运行中的 Session 会连接其事件流而不取消另一个运行中的 Session；切换到暂停 Session
    只有在重新取得规范锁后才恢复，且不会重复 Turn。
18. 被其他所有者锁定的 Session 仍可作为历史读取，但不能恢复或批准待处理操作；WebSocket
    断开时显示 reconnecting/unknown，直到服务器完成协调。
19. `/plan` 启动/停止所选 Thread 的 Plan 探索 Runtime；`/goal <objective>` 创建并启动 Goal，
    持久顶部栏支持开始/恢复、暂停、更新和删除，且不会展示其他 Thread 的 Goal。
20. Plan 临时脚本和输出在正常结算、取消或明确退出时会被清理；清理失败显示为
    `cleanup_pending`，而 `plan.md` 及其有界摘要会保留。
21. 不存在面向用户的 `max_steps` 或常规 `step_limit` 控制。Plan 和 Goal Runtime 状态
    负责继续与完成；Core 异常的安全保护结果作为内部诊断保留，并展示为“运行保护已触发，
    请检查或重试”；只有真正的取消才展示为“中断/已取消”。

Provider 支持的验证保持为可选，默认不能使用付费调用。正常证据路径使用 Mock Provider、
Protocol Fixture 和 Harness Scenario。

## 变更准入

1. **所属层：** Capabilities/Host 负责 WorkspaceSpec 校验和工具范围；SessionStore 仍是
   持久化的 Capabilities/Host 边界；App Server 暴露有界的 Session 管理和实时状态；Python
   SDK/FastAPI/Web 适配 Project/Session 列表和切换。只有现有有界 checkpoint 接缝需要适配器
   时才允许修改 Core；Core 不负责 UI、项目注册表、文件系统持久化策略或批准对话框。
2. **已有所有者：** 复用 `SessionStore`、`session_directory`、settled checkpoint 规则、
   `Workspace::with_read_roots`、`ToolBuildRequest`、`ApprovalController`、`ApprovalStore`、
   `ApprovalBroker`、Thread Settings、Goal Runtime、`ThreadListener`、SDK 通知处理和已有
   WebSocket 路由。不允许第二个 transcript/checkpoint 存储、第二个 Router、Workflow Service、
   Turn Loop 或策略框架。
3. **替换还是增加：** 删除旧的 Web checkpoint 权威，使用从规范 SessionStore 派生的引用/缓存
   索引，不增加迁移/导入路径；用每个 Runtime/Session 的不可变 WorkspaceSpec 替换可变进程 cwd
   假设；扩展已有 Session、Thread
   和工具构建接缝，增加有界根目录/状态元数据，然后再加入有范围的批准；不引入通用存储框架。
4. **净行数变化：** 2026-09-03 基线为 Runtime `19,554/20,000`、Release Rust `29,328/30,000`，
   剩余空间分别为 446 和 672 行。提案不构成一个无限制实现批次。每个 Rust 批次都必须记录
   可测量的抵消或保持净零，运行 `python scripts/line_budget.py`，并记录前后实际计数。
5. **可见表面：** 只增加有界的 WorkspaceSpec 根目录元数据、每个根目录的访问模式、Session
   目录/状态字段、访问范围与批准生命周期元数据、类型化控制字段和 Thread 范围通知。
   不公开任意 Prompt 替换、无限制根目录列表/路径、无限制扩展激活或无界事件内容。
   Reference 根目录只读；Editable 根目录必须由 Project 或 machine 访问决定明确授权，
   并继续受批准/Plan 约束。Plan scratch、`plan.md` 和清理状态是有界的 Thread 所属产物。
   不要增加独立的 Profile 层，也不要把 Profile 名称作为用户控制。
6. **边界证据：** 现有 Session、Workspace、Protocol、Host、App Server、SDK 和 Web 测试只覆盖
   部分路径。新的多根路径准入、规范存储、Session 列表/锁、历史/运行中/暂停切换、批准范围、
   Thread 路由、Plan/Goal、压缩和 slash 场景都是必需的，因为单元测试不足以证明端到端控制链。

## 变更测试

- **假设：** 独立的 Project/machine 访问范围、有界的批准生命周期和 Thread 所属的 Plan/Goal
  Runtime，可以在不削弱 Host 安全、不丢失多根工作区边界、不重复执行状态的前提下，为客户端
  提供类似 Codex 的控制。移除 Profile 和常规步数预算控制，可以让所有权更容易理解。
- **区分结果的 Trace：** `projectId → workspaceId → sessionId → threadId → turnId → callId →
  requestId`，随后是根目录准入、Session 锁/状态、访问范围/生命周期、解析结果、ToolItem 完成、
  Plan scratch 清理状态、保留的 `plan.md`、Stop Reason 和规范 readback。同一条 Trace 必须证明
  历史选择和暂停 Session 恢复不会生成重复 checkpoint 或 Turn，并区分 Project 配置的 Editable
  根目录、只作参考的一次性消息路径，以及明确批准的同步/更新。
- **为什么不能只放在 Host 适配器：** 根目录准入和 Session 身份必须跨过 App Server/SDK 边界，
  同时规范历史和锁所有权必须能被 Web Studio 观察到。UI 适配器无法单独维持这两个不变量；
  只放 Host 的根目录列表也无法解决当前 Web 进程 cwd 的缺口。
- **永久复杂度：** 一个类型化、有界的 WorkspaceSpec、一条规范 SessionStore 所有权路径、
  一个有界的 Session 目录投影、一份有范围的批准契约、一个有界的 Plan scratch/清理路径和
  一个 Core 内部防止无限循环的保护。
  明确排除通用 Hook、Profile/策略引擎、存储框架和扩展框架。

## 非目标

- 不修改 `D:/gh-ws/codex`，也不复制官方 Codex 仓库。
- 不把整机范围的“完全访问”等同于无限制的“允许全部”；硬性安全 Deny 以及文档化的
  Shell/确认保护仍然有效。
- 不增加第二个 Core 执行循环、Goal verifier 历史或 Web 侧持久化权威。
- 不读取、导入、翻译、写入或删除旧 Web `state.json` 或 checkpoint 目录。它们不属于新
  Runtime；新的 Web manifest/index 只是派生的展示状态，不是第二个 Session 数据库。
- 不让关联 Project 目录隐式可写，也不声称仅靠 Native Shell cwd 就能实现多根文件系统隔离。
- 不支持同一 Session 的两个并发写入者，也不静默地将已有 Session 重新绑定到修改后的 Project 工作区清单。
- 不把 Plan 做成严格只读，也不允许它的 scratch 根目录变成未声明的持久 Project 写入区。
- 不公开任意原始系统 Prompt 替换。
- 不在首个批准批次中加入 Skill/Plugin 的动态热加载。
- 不增加独立的 `Profile` 层，也不保留 `interactive`/`ask`/`auto`、`profile=auto` 或
  `turbomode`/`Turbomode` 作为 Runtime/Session 身份。唯一的迁移输入别名是将 `turbomode`
  映射为 `FullMachine` 后立即丢弃；不提供其他迁移解析器或兼容别名。
- 不暴露 `max_steps` 或常规 `step_limit` 作为任务控制；Core 安全保护保持内部，继续行为归
  Plan/Goal Runtime 状态管理。

外部对齐原则是明确表达自主性和批准边界，明确安全的本地操作，并要求对破坏性、外部或
扩大范围的操作进行确认；这与 [OpenAI 官方指导](https://developers.openai.com/api/docs/guides/latest-model)
一致。
