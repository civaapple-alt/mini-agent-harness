# Tool outcome 公共投影与 Web 消费边界

状态：implemented
日期：2026-09-15
批次：Iteration 7，保留主链工具结果的公共语义
范围：App Server Protocol `ThreadItem`、Python SDK、Gateway Session catalog、TUI

## Decision

`ThreadItem.ToolCall` 增加可选的 `outcome` 字段，使用 Core 的
`ToolExecutionStatus` 序列化值：`completed`、`failed`、`needs_approval`、
`deferred`、`retryable`。已有 `status` 字段继续表示 Item 生命周期，仍只使用
`inProgress`、`completed`、`failed`。两个字段保持正交，不把工具策略结果塞进
生命周期枚举。

Live `turn/event` 的 ToolFinished、App Server 的 Item lifecycle notification、
`thread/items/list` 和 Session catalog 都保留该字段。旧 Session 记录缺少
`outcome` 时继续生成原有 projection。SDK 用 `ToolOutcome` Literal 表达当前
有界值；TUI 只按 outcome 改变展示文案，不据此创建重试或授权状态。

## Harness hypothesis

如果 App Server 在 Core event 到 public Item 的投影中保留完整 outcome，SDK、
Gateway 和 TUI 就能区分“失败”“等待审批”“暂缓执行”和“可重试”，而不必从
`content` 或 `is_error` 反推；旧客户端只读取 `status` 时仍可正常工作。

反例包括：只给 live event 加字段而丢掉 Session catalog，使用一个新的 ItemStatus
变体替代两个维度，或让 Web 根据 `MCP tool call timed out` 等诊断文本决定重试。

## Ownership and boundaries

| 对象 | 唯一权威 | 本批变更 | 禁止行为 |
| --- | --- | --- | --- |
| 工具策略/执行结果 | Core event 中的 `ToolExecutionStatus` | App Server Item 复制为可选 `outcome` | App Server 重新分类或读取错误文本 |
| Item 生命周期 | App Server Protocol `ItemStatus` | 保持原有 `status` 语义与值 | 把 outcome 变体添加到生命周期枚举 |
| 历史 Item | SessionStore 记录，经 Gateway catalog 投影 | 新记录透传 outcome，旧记录缺省兼容 | Gateway 用本地状态补写 Session 权威 |
| SDK | Python `ThreadItem`/`ToolOutcome` | 解析并保留 outcome | 丢弃未知字段或把它改写成布尔值 |
| TUI | `stream_renderer` 展示层 | 用结构化 outcome 选择提示文案 | 由展示逻辑触发 retry/approval |

## Cross-repository contract

| 语义 | mini-codex | mini-agent-web |
| --- | --- | --- |
| lifecycle | `ThreadItem.ToolCall.status`，camelCase ItemStatus | `ThreadItem.status` 原样保留 |
| tool outcome | 可选 `ToolExecutionStatus`，snake_case | `ThreadItem.outcome: ToolOutcome \| None` |
| 兼容性 | 老 Session 的 Message::Tool 可没有 outcome | SDK/catalog 缺省为 `None`，旧响应字段不被强制添加 |
| 诊断文本 | `content`/`output` 仍为有界显示和模型输入内容 | 只展示，不用于分类 |

本批是一个向后兼容的公共字段增加。未来如果新增 outcome 值，必须同步更新
Rust Protocol、Python Literal、Gateway/TUI fixture 和 protocol compatibility
smoke test，再决定客户端对未知值的处理策略。

## Alternatives considered

- 将 `needs_approval`、`deferred`、`retryable` 添加到 `ItemStatus`。否决，因为
  生命周期和工具结果是两个维度，会让 `inProgress` 与 settled outcome 无法独立表达。
- 只保留 `turn/event.event.outcome`，不扩展 Item。否决，因为 `thread/items/list`、
  `turn/read` 和 Gateway 历史投影会继续丢失语义。

## Verification

| 仓库 | 命令 | 结果 |
| --- | --- | --- |
| mini-codex | `cargo fmt --all` | passed |
| mini-codex | `cargo test -p mini-agent-app-server-protocol --lib` | 19 passed |
| mini-codex | `cargo test -p mini-agent-app-server --lib` | 57 passed |
| mini-agent-web | `uv run pytest -q tests/sdk/test_sdk_events.py tests/gateway/test_session_manager.py tests/tui/test_tui_rendering.py` | 79 passed；2 个既有 Windows asyncio 资源析构 warning |
| mini-agent-web | `uv run python cookbook/python-demo/06_protocol_compatibility.py` | passed |
| mini-agent-web | Ruff changed-file check | passed |
| mini-codex | `cargo clippy -p mini-agent-app-server-protocol --all-targets -- -D warnings`、`cargo clippy -p mini-agent-app-server --all-targets -- -D warnings` | passed |
| mini-codex | `python scripts/line_budget.py --base f6db381 --check-delta --json` | 无 violation；runtime `+47`、release Rust `+47`、control plane `+47`，当前 green |
| mini-codex | `python scripts/check_iteration_note.py .agents/notes/implemented/architecture/2026-09-15-tool-outcome-public-projection.zh.md` | passed |

## Consequences

- `Core → Host → App Server → SDK/Gateway` 的 tool outcome 不再在 Item projection
  处被折叠，Web 可以显示准确的等待审批、延迟和可重试状态。
- `status` 的既有消费者无需迁移；需要执行策略的消费者改读 `outcome`，而不是
  解析 `output`。
- 新增一个有界公共字段，并补上历史 Session 缺省处理、SDK 类型和 TUI 展示路径。

## Remaining risks

- Python `Literal` 只提供静态约束，运行时仍保留服务器传来的字符串；未来未知
  outcome 的降级策略尚未单独定义。
- TUI 目前只改变提示文案，不实现自动 retry 或 approval continuation；这些动作
  仍属于 SDK/App Server 控制面。
- 本批未运行真实 provider、真实 MCP transport 或完整 workspace suite；前端构建
  也不在当前 Web 仓库的可用验证范围内。
