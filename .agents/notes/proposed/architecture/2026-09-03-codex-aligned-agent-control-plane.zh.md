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

控制平面必须保持以下三个维度相互独立：

```text
Host Profile（启动时）：interactive | ask | auto
Thread Mode（Session）：chat | plan | goal
Approval（运行时）：policy + 有界决策范围
```

此外，每个 Thread/Session 都必须绑定到一个不可变的工作区清单：

```text
Workspace Binding（Session）：primaryRoot + associatedReadRoots + 写入策略
```

Plan 和 Goal 继续沿用现有的 Thread 边界。本提案不创建第二个 Workflow
Service、第二个 Turn Loop，也不引入通用策略/插件框架。

## 当前证据与问题

当前实现已经具备所需组件，但它们的权威边界和语义分散在三个仓库中：

1. `ApprovalController` 会把所有批准过的操作缓存到 `ApprovalStore`。因此，
   原本想表达“仅允许一次”的 UI 选择可能表现成“始终允许”。
2. `ApprovalRespondParams` 携带 `remember`，但 App Server 的响应路径目前只消费
   布尔批准结果。用户选择的范围在 Host 恢复执行前已经丢失。
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

相关的现有责任方包括：

- `crates/mini-agent-capabilities/src/workspace/approval.rs`：Host 侧批准执行和缓存批准；
- `crates/mini-agent-app-server/src/lib.rs`：批准 Broker 和生命周期；
- `crates/mini-agent-app-server-protocol/src/lib.rs`：批准、Thread 设置和 Goal 的线协议类型；
- `server/session_manager.py`：SDK 生命周期、Web 传输和当前重复的批准状态；
- `sdk/python/src/mini_agent/client.py`：Stdio JSON-RPC 和通知适配；
- `frontend/src/App.jsx`、`frontend/src/utils/slashCommands.js`：Studio 控制状态和本地命令分发。

## 目标语义

### Profile 不等于 Mode

Host Profile 仍然是在初始化阶段组合 Provider、Prompt/规则来源、扩展深度、工具
范围、Sandbox 和安全预设。它不是 Plan 或 Goal 的在线替代品。

因此，Studio 主控制项应呈现为：

| Studio 标签 | 规范操作 | 运行时含义 |
| --- | --- | --- |
| Interactive / Chat | 使用 `collaborationMode=default` 调用 `thread/settings/update` | 普通用户驱动的 Turn |
| Plan | 使用 `collaborationMode=plan` 调用 `thread/settings/update` | 只读项目修改；仍允许写入当前计划 |
| Goal | 调用 `thread/goal/set|get|clear` | Thread 所属的自主生命周期和验证器 |

当前的 Host Profile `interactive`、`ask` 和 `auto` 继续作为启动配置或高级配置
保留。运行中选择不同 Host Profile 必须重启/重建 App Server Runtime，或明确标注为
“下一个 Runtime 生效”。不能只更新 Python 偏好字段而不改变实际 Runtime 行为。

### Approval Policy 与 Approval Scope 不同

`per_action` 表示何时必须询问，不表示每个操作都会自动获准。工具暴露范围、Host
安全策略、Plan 锁和人工批准仍然是相互独立的门槛。

公开决策词汇为：

```text
policy: per_action | auto_approve | strict
scope:  once | session | workspace
```

规则：

- `once` 只批准当前 `requestId`；
- `session` 在当前 Thread 或 App Server Session 内，对相同的有界操作类别生效，具体生命周期按选定范围执行；
- `workspace` 只对当前工作区、工具和规范化操作类别生效，不是无限制的机器级权限；
- 明确的安全 Deny 始终优先于 UI 批准；
- Plan Mode 的修改锁不能被批准绕过；
- `auto_approve` 可以跳过可准入操作的交互等待，但不能覆盖明确 Deny 或 Plan 限制；
- `strict` 在不打开用户批准请求的情况下拒绝需要批准的操作。

批准键必须包含有界的所有者和操作身份，例如：

```text
(workspace root, thread/session scope, tool name, normalized action class)
```

不能将原始的无限制命令、密钥或任意 Prompt 文本变成批准规则。

## 所有权模型

| 层 | 负责 | 不负责 |
| --- | --- | --- |
| Core | Turn Loop、限制、停止分类、事件、历史写回 | UI、批准对话框、持久化、Profile 组合 |
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
  "scopes": ["once", "session", "workspace"]
}
```

```json
{
  "requestId": "approval-1",
  "approved": true,
  "scope": "once",
  "reason": ""
}
```

`approval/resolved` 必须回显最终范围和关联字段。为保持线协议兼容，接受旧的
`remember`：`false` 映射为 `once`，`true` 映射为 `session`；新客户端使用 `scope`。
App Server 不得再静默地记住每次成功的布尔批准。

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
│           └── 启用时的 plan/goal 文件
├── web/
│   ├── state.json                   # 项目、UI 偏好、选择状态
│   └── session-index.json           # 可选，只能是派生/缓存索引
└── logs/
    └── <workspaceId>/<sessionId>/   # 诊断日志，不是对话状态
```

这是语义上的统一，不要求在一次变更中移动所有诊断文件。迁移期间必须继续读取已有的
单根目录 Session。Web checkpoint 目录转为兼容读取/导入来源，之后停止写入；它不能覆盖
更新的规范 checkpoint。迁移必须幂等，不能覆盖规范 Session；如果发现无法关联到正确项目
的旧 checkpoint，应报告为孤立数据，而不是静默绑定。

首轮迁移期间，`state.json` 可以作为有版本的 Web manifest 保留，但应位于
`~/.mini-agent/web/` 下，并且只保存项目注册表、UI 偏好和 `projectId`、`workspaceId`、
`sessionId`、`threadId` 等引用。不能保存 transcript 或重复的 Thread checkpoint。

### Project、WorkspaceSpec 与 Session 绑定

Web Studio 的 Project 是面向用户的身份；它必须解析成明确的 Runtime `WorkspaceSpec`：

```text
Project
└── WorkspaceSpec
    ├── primaryRoot
    ├── associatedReadRoots[]
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
    → Host ToolBuildRequest.extra_read_roots
    → Capabilities Workspace 路径准入
```

根目录清单属于控制面配置，不是模型生成的输入。创建 Runtime 前必须对其校验、限制、
canonical 化，并将其纳入 Runtime/Session 身份。

### 工作区范围规则

初始策略有意采用非对称设计：

| 根目录类型 | 读取工具 | `apply_patch`/创建 | Shell cwd | 默认写入权 |
| --- | --- | --- | --- | --- |
| `primaryRoot` | 允许 | 受 Plan/安全/批准约束后允许 | `primaryRoot` | 有 |
| `associatedReadRoots[]` | 通过 canonical 路径检查后允许 | 拒绝 | 不改变 cwd | 无 |

关联目录是读取根，不是“所有项目目录都可写”的隐式授权。它们必须传递到现有的有界
`extra_read_roots`/`Workspace::with_read_roots` 路径，而不是只显示在 UI 中。嵌套或重复的
根目录必须规范化；符号链接和逃逸情况必须在 Runtime 创建前拒绝，或归并到 canonical
根目录清单中。

Native Shell 始终从 `primaryRoot` 启动。由于 Native 子进程可以发起任意文件系统写入，
文件工具的检查无法观察这些写入，因此 Shell 访问关联目录不能被视为写入授权。严格的
执行模式必须使用配置好的进程 Sandbox，或要求一个明确描述受影响路径的批准；UI 不能
声称 `per_action` 单独就能保证 Native Shell 的路径写入安全。Docker/Sandbox 挂载必须
只包含声明的根目录，并在 Sandbox 支持时保持主目录与只读关联目录的区别。

根目录清单需要小而明确的 Protocol 限制，包括关联目录数量、路径长度和序列化后的总字节数。
具体常量属于 Protocol 实现批次；目录列表绝不能作为无限制的模型可见上下文。

### Session 目录与生命周期

产品必须区分 Runtime 实时状态和 Goal 状态：

| 维度 | 值 | 权威 |
| --- | --- | --- |
| Runtime | `running`、`idle`、`closed` | App Server Actor/Turn 状态 |
| Session UI 投影 | `running`、`paused`、`historical`、`locked` | Web 对 Runtime、锁和 summary 的投影 |
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
当前工作区范围允许
拒绝并说明原因
```

“完全范围”必须显示实际的有界范围，例如“当前工作区 + 当前工具类别”，而不能显示成
无限制的“允许全部”。

加号菜单和 slash 命令应调用类型化 API：

| 命令 | 操作 |
| --- | --- |
| `/status` | 读取 World、当前 Thread 设置、Goal 和 MCP 状态 |
| `/compact` | 调用显式 Thread 压缩 |
| `/plan on\|off` | 更新当前 Thread 的 collaboration mode |
| `/goal <objective>` | 设置有界的 Thread Goal |
| `/goal clear` | 清除当前 Goal |
| `/mcp` / `/mcp retry` | 读取或重试 MCP 状态 |
| `/review` | 启动 allowlist 中的 Review Workflow |
| `/skill` / `/plugin` | 只列出或选择已发现并批准的条目 |

控制命令不能变成普通模型 Prompt。Review、Skill 和 Plugin 激活必须保持 allowlist 化
并且有界。任意扩展指令的动态热加载不在本提案范围内；如需新的 Runtime 激活契约，
必须单独记录变更准入。

## 实施批次

### Batch 0 — 契约与 Trace Fixture

记录状态轴、批准范围词汇和关联规则。在改变执行逻辑前增加离线 Protocol Fixture。
不需要 Provider 调用。

### Batch 1 — 规范存储、WorkspaceSpec 与 Session 目录

- 定义有版本的 `WorkspaceSpec`、稳定的 `workspaceId`、每个 Session 不可变的
  `workspace.json` 和有界的根目录清单限制；
- 将 Web Studio 的 Project 主目录和关联读取目录一路传递到 Host 工具构建；
- 将主目录设为 agent cwd/写入根，将关联目录设为明确的读取根；增加路径、符号链接、
  Sandbox 和 Native Shell 边界测试；
- 增加基于规范 `SessionStore` summary 和锁的有界 App Server/SDK/Web Session list/inspect 路径；
- 将 Web 状态移动到 Web 自有子目录，停止写入重复的 Web checkpoint，并增加幂等的旧 checkpoint
  迁移或兼容读取路径；
- 暴露按 Session 的 Runtime/Goal 状态、活动 Turn、锁和可恢复字段；移除一个全局 Web client
  或一份全局 active state 表示所有 Project 的假设；
- 在启用并发运行/暂停 Session 切换前，实现按 Project 的历史选择和 attach/只读行为。

这一批是批准范围所依赖的存储和身份前置条件。同时必须决定：一个 App Server 进程是否
复用不可变 WorkspaceSpec，或者 Web 是否为每个 WorkspaceSpec 保留一个 Runtime handle；
一个可变的共享 cwd 不能作为第三种状态。

### Batch 2 — 批准正确性与路由

- 让 `once` 真正只生效一次；
- 让 `session`/`workspace` 成为明确的有界缓存条目；
- 保持 Deny 和 Plan 锁的优先级；
- 将范围从 App Server → SDK → FastAPI → Studio 传递完整；
- 删除 Web 的重复批准权威；
- 按 Thread 和 request ID 路由待批准请求；
- 增加 Core/Host/App Server 边界测试和一个 SDK/Web 批准场景。

这是第一道实现批次，也是安全正确性门槛。

### Batch 3 — 活动 Thread 控制

- 所有 Plan、Goal、Builtin 和 status 调用传递活动 `threadId`；
- Thread 选择后独立恢复 Settings 和 Goal；
- 发出并消费权威的 Settings/Goal 通知；
- 除非明确执行重启/重建操作，否则不要把 Profile 偏好变化当作在线 Runtime 变化。

### Batch 4 — SDK 与 FastAPI 收敛

- 增加类型化的批准请求/决定模型；
- 仅将布尔/字符串批准回调保留为兼容适配器；
- 确保重连、超时、取消和延迟响应行为是确定性的；
- 删除过时的 Web 侧 remembered approval 状态和宽泛广播。

### Batch 5 — Studio 控制面

- 将主选择器改名为 Mode：Interactive、Plan、Goal；
- 将 Security/Approval 保持为独立选择器；
- 实现加号菜单和批准面板；
- 增加当前 Thread 的 Mode、Goal、批准策略、MCP 和 Runtime revision 状态指示器。

### Batch 6 — Slash 与 Review Workflow

优先实现 `/status`、`/compact`、`/plan`、`/goal` 和 `/mcp`。只有在对应能力证据被接受后，
才通过现有 allowlist 增加 `/review` 和 Skill/Plugin 选择。

## 验收证据

提案只有在以下场景全部满足后才能移动到 `implemented/`：

1. `allow once` 使第二次相同操作再次请求批准。
2. `session` 批准只影响目标 Thread/Session。
3. `workspace` 批准不影响其他工作区或无关的操作类别。
4. 任何 UI 范围都不能覆盖安全 Deny。
5. Plan Mode 延迟项目修改，但允许修改当前计划。
6. Goal 的 set、update、clear、resume、verifier 和 continuation 事件保持有序并限定于 Thread。
7. 两个活动 Thread 不能展示或解析彼此的批准请求。
8. Profile 变更要么在 Runtime 重建后生效，要么明确报告为下一个 Runtime 的设置。
9. `/status`、`/plan`、`/goal`、`/mcp` 和 `/compact` 使用控制 API，而不是意外变成模型 Prompt。
10. `item/started`、批准事件、`approval/resolved`、`item/completed` 和 `turn/read` 保持相同的有界 call identity 和最终结果。
11. 一个包含主目录和多个关联目录的 Project，可以从每个声明的关联目录读取文件，而
    `apply_patch` 和创建操作默认仍限制在主目录。
12. Project 清单之外的根目录、符号链接逃逸和未声明的 Shell 写入必须被拒绝或进入文档化
    的 Sandbox/批准路径；UI 不能将关联目录显示成无限制的写入范围。
13. 修改 Project 的关联目录不会改变已有 Session 的 `workspace.json`；新 Session 或明确 fork
    才能获取新清单。
14. 旧 Web checkpoint 可以被幂等地发现并迁移/报告；规范 `SessionStore` 历史始终优先，且迁移
    后不再写入第二份 checkpoint。
15. Project 历史可以列出有界的 historical、running、paused 和 locked Session，并保持稳定的
    `sessionId`/`threadId`/`workspaceId` 关联。
16. 切换到运行中的 Session 会连接其事件流而不取消另一个运行中的 Session；切换到暂停 Session
    只有在重新取得规范锁后才恢复，且不会重复 Turn。
17. 被其他所有者锁定的 Session 仍可作为历史读取，但不能恢复或批准待处理操作；WebSocket
    断开时显示 reconnecting/unknown，直到服务器完成协调。

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
3. **替换还是增加：** 将 Web checkpoint 权威替换为引用/缓存索引和幂等迁移路径；用每个
   Runtime/Session 的不可变 WorkspaceSpec 替换可变进程 cwd 假设；扩展已有 Session、Thread
   和工具构建接缝，增加有界根目录/状态元数据，然后再加入有范围的批准；不引入通用存储框架。
4. **净行数变化：** 2026-09-03 基线为 Runtime `19,554/20,000`、Release Rust `29,328/30,000`，
   剩余空间分别为 446 和 672 行。提案不构成一个无限制实现批次。每个 Rust 批次都必须记录
   可测量的抵消或保持净零，运行 `python scripts/line_budget.py`，并记录前后实际计数。
5. **可见表面：** 只增加有界的 WorkspaceSpec 根目录元数据、Session 目录/状态字段、批准
   范围元数据、类型化控制字段和 Thread 范围通知。不公开任意 Prompt 替换、无限制根目录
   列表/路径、无限制扩展激活或无界事件内容。关联目录默认只读。
6. **边界证据：** 现有 Session、Workspace、Protocol、Host、App Server、SDK 和 Web 测试只覆盖
   部分路径。新的多根路径准入、存储迁移、Session 列表/锁、历史/运行中/暂停切换、批准范围、
   Thread 路由、Plan/Goal、压缩和 slash 场景都是必需的，因为单元测试不足以证明端到端控制链。

## 变更测试

- **假设：** 明确的批准范围和 Thread 所有权，可以在不削弱 Host 安全、不丢失多根工作区边界、
  不重复执行状态的前提下，为客户端提供类似 Codex 的控制。
- **区分结果的 Trace：** `projectId → workspaceId → sessionId → threadId → turnId → callId →
  requestId`，随后是根目录准入、Session 锁/状态、批准范围、解析结果、ToolItem 完成和规范
  readback。同一条 Trace 必须证明历史选择和暂停 Session 恢复不会生成重复 checkpoint 或 Turn。
- **为什么不能只放在 Host 适配器：** 根目录准入和 Session 身份必须跨过 App Server/SDK 边界，
  同时规范历史和锁所有权必须能被 Web Studio 观察到。UI 适配器无法单独维持这两个不变量；
  只放 Host 的根目录列表也无法解决当前 Web 进程 cwd 的缺口。
- **永久复杂度：** 一个类型化、有界的 WorkspaceSpec、一条规范 SessionStore 所有权路径、
  一个有界的 Session 目录投影和一份有范围的批准契约。明确排除通用 Hook、策略引擎、存储
  框架和扩展框架。

## 非目标

- 不修改 `D:/gh-ws/codex`，也不复制官方 Codex 仓库。
- 不实现无限制的机器级“允许全部”批准。
- 不增加第二个 Core 执行循环、Goal verifier 历史或 Web 侧持久化权威。
- 不让 Web `state.json` 或 checkpoint 目录成为第二个 Session 数据库；迁移只是兼容工作，
  不是长期并行存储。
- 不让关联 Project 目录隐式可写，也不声称仅靠 Native Shell cwd 就能实现多根文件系统隔离。
- 不支持同一 Session 的两个并发写入者，也不静默地将已有 Session 重新绑定到修改后的 Project 工作区清单。
- 不公开任意原始系统 Prompt 替换。
- 不在首个批准批次中加入 Skill/Plugin 的动态热加载。

外部对齐原则是明确表达自主性和批准边界，明确安全的本地操作，并要求对破坏性、外部或
扩大范围的操作进行确认；这与 [OpenAI 官方指导](https://developers.openai.com/api/docs/guides/latest-model)
一致。
