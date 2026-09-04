# mini-agent 与 Codex App Server 异同比较

Status: archived
Archived: 2026-09-04

> 本文是历史比较记录，不是当前协议规范。当前行为以 `docs/` 主题文档和各目录 README 为准。

## 结论摘要

两者共享同一个外部骨架：

```text
initialize → thread → turn → streamed events → control
```

但定位不同：`mini-agent` 是有界、可验证的 Harness 加精简 App Server；
Codex App Server 是面向丰富客户端集成的完整运行时接口，不是协议兼容的缩水实现。

## 相同点

| 方面 | 共同点 |
| --- | --- |
| 通信模型 | 双向 JSON-RPC 风格通信，客户端发请求，服务端异步推送事件 |
| 握手 | 都要求先 `initialize`，再使用其他方法 |
| 核心对象 | 都有 `Thread`、`Turn`、`Item` |
| 主流程 | 都支持创建或恢复 Thread、启动 Turn，并通过事件流观察执行 |
| 控制 | 都提供 steer 和中断/取消能力 |
| 工具执行 | 都把模型消息、工具调用和工具结果作为执行过程的一部分进行投影 |
| 审批 | 都支持客户端参与工具或文件操作审批 |
| 持久化 | 都保存对话或执行历史，使 Thread 能够恢复 |

## 主要差异

| 维度 | 当前 mini-agent | Codex App Server |
| --- | --- | --- |
| 定位 | Harness 研究和最小可验证实现 | Codex 完整运行时的产品集成接口 |
| 核心目标 | 显式 loop、bounded input、stop 分类、安全检查点和确定性测试 | 支撑丰富客户端、长任务、恢复、审批、MCP、环境和 UI |
| 协议 | 自有 `mini-agent-app-server-protocol`，方法名有意接近 Codex | Codex 官方 App Server protocol |
| JSON-RPC 细节 | 协议模型显式处理 JSON-RPC 版本字段，协议版本为 `1` | 官方 wire 上省略 `"jsonrpc":"2.0"` 字段 |
| Transport | 主要是 stdio JSONL；另有进程内 `LocalAppServerClient` | stdio、WebSocket、Unix socket，以及关闭 transport 的模式 |
| Thread 状态 | `Session JSONL + settled checkpoint + Actor/CAS/stateRevision` | Codex thread/rollout/session 体系，支持更丰富的 archive、fork、resume 和 metadata |
| Turn 控制 | 强调在安全检查点结束当前工作单元，再处理 queued steer/follow-up | steer 向当前 in-flight Turn 追加输入，保持同一个 Turn |
| 事件 | 稳定的 `turn/event`，加 bounded 的 `item/started` 和 `item/completed` | 大量 `turn/*`、`item/*`、delta、diff、plan、tool progress 和 warning 事件 |
| Item | 主要用于有限的模型/工具生命周期投影，参数和结果严格截断、脱敏 | 类型更丰富，包括消息、命令执行、文件变更、MCP 调用和计划等 |
| 工具面 | 默认 `read_file`、`apply_patch`、`shell`、`read_image`；MCP 等是显式扩展 | 工具、技能、MCP、Apps、动态工具、文件系统和命令执行等能力更广 |
| 权限/沙箱 | Host 分离 admission、approval、runtime 和 sandbox，重点是边界清晰 | 更完整的 approval policy、sandbox policy、网络权限和文件变更审批 |
| 配置 | 只能选择有限的本地 runtime 组合，不允许任意替换 system prompt | 支持按 Thread/Turn 覆盖 model、cwd、sandbox、personality、output schema 等 |

## 关键语义差异

### 1. mini-agent 优先保证可验证性

mini-agent 把控制流程显式化：

```text
Core loop
  → safe checkpoint
  → durable checkpoint
  → queued steer/follow-up
```

因此 cancel、steer、timeout 的顺序和落盘时机可以通过 deterministic scenario 验证。

Codex 更偏向持续运行的客户端体验：一个 Turn 可以持续接收 steer，继续后续
sampling，并持续产生事件。

### 2. Codex App Server 是完整运行时门面

Codex App Server 不只是 loop 的 RPC 包装，还承载：

```text
Thread/session/rollout
模型与配置
工具和 MCP
审批与沙箱
文件变更
计划与 diff
流式事件
环境与账号能力
```

mini-agent 则刻意将这些责任拆开：Core 不拥有文件、进程、Provider、审批 UI
和持久化；这些由 Capabilities、Host 和 App Server 组合提供。

### 3. 方法名相似不代表协议兼容

两边都可能出现：

```text
initialize
thread/start
thread/resume
turn/start
turn/steer
thread/goal/set
```

但参数、事件名称、Item 结构、错误语义、ID 生命周期和控制边界不同。因此更准确的
说法是：mini-agent 借鉴了 Codex App Server 的对象模型和客户端交互形态，但实现
的是自己的 bounded protocol，而不是 Codex protocol 的兼容实现。

## 关系判断

```text
Codex App Server：完整、丰富，适合产品集成，但状态和事件面较大

mini-agent App Server：较小、显式、可测试，适合研究 Harness 边界
```

mini-agent 更像是从 Codex App Server 的思想中抽取出的“可验证最小闭环”，
而不是 Codex App Server 的替代品。

## 依据

- 当前 mini-agent：[docs/app-server.md](../../../../docs/app-server.md)
- 当前 Harness 边界：[docs/harness-boundaries.md](../../../../docs/harness-boundaries.md)
- 当前 Harness 分层：[docs/harness-framework.md](../../../../docs/harness-framework.md)
- Codex：[Codex App Server 官方文档](https://developers.openai.com/codex/app-server)
