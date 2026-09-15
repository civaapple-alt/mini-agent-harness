# 主执行链 typed Tool outcome 边界

状态：implemented
日期：2026-09-15
批次：Iteration 6，主执行链的工具结果状态收敛
范围：`mini-agent-core`、Protocol、Host、Capabilities、App Server 与 `mini-agent-web` 消费边界

## Decision

主执行链采用 `Core → Host → Capabilities → App Server` 的责任顺序：Core
解析模型请求并记录结果，Host 编排工具准入、审批和执行，Capabilities 负责具体
副作用及其可重试/延迟分类，App Server 将 Core event 和 Item 有序投影到 RPC。

工具状态通过已有的 `ToolExecutionOutcome` 传递，使用
`ToolExecutionStatus::{Completed, Failed, NeedsApproval, Deferred, Retryable}`；
Host 不再从错误内容匹配字符串来猜测状态。`ToolAdmission::Deferred` 表达尚未
执行的 typed 准入结果；审批拒绝通过 `ApprovalFailure` 在 Host 边界区分用户拒绝、
策略拒绝和内部错误。旧 `Legacy` 工具仍使用自己的 `execute_outcome`，以保持增量
迁移兼容，但不再被 Host 的全局字符串分类器改写。

## Harness hypothesis

如果工具状态在 Capabilities/Host 交界处以结构化 outcome 产生，并由 Core 原样
写入 `ToolFinished`、history 和下一轮模型输入，那么主链上的审批、Plan-mode
deferred、Shell timeout 与 MCP retryable 结果会保持语义一致；诊断文本变化不会
意外改变 loop control 或 App Server 对外投影。

反例包括：Host 再次匹配 `timed out`、`user denied` 等文本；Plan-mode lock 被
误报为普通失败；MCP timeout 被当成永久失败；或 Gateway/SDK 为了展示结果复制一
份 status 权威。

## Ownership and boundaries

| 对象 | 唯一权威 | 本批变更 | 禁止行为 |
| --- | --- | --- | --- |
| turn loop、history、Core event | Core `ToolRouter`/run loop | 记录 Host 返回的完整 outcome | Core 自己解析 provider、审批或 sandbox 错误文本 |
| 准入、审批顺序和结果映射 | Host `ToolOrchestrator` | 消费 typed `ToolAdmission` 与 `ApprovalFailure` | 用 `content` 反推 status，或维护第二份 grant |
| 具体副作用和局部分类 | Capabilities `ToolRuntime` | MCP/Shell 返回显式 `Retryable`，Plan lock 返回 `Deferred` | 把运行时诊断字符串当成跨层分类协议 |
| 对外顺序与投影 | App Server worker/protocol | 保持既有 `turn/event`、Item 和 outcome 字段 | 重新执行工具或在协议层发明另一套状态 |
| Web 消费 | Python SDK/Gateway/Studio | 继续透传和展示 bounded `outcome` | 自行决定 retry/approval/deferred 权威或缓存授权 |

## Cross-repository contract

本批不改变 JSON-RPC、SDK 或 Gateway 的 wire shape。App Server 继续在既有
`ToolFinished`/Item 投影中携带 `outcome`；`mini-agent-web` 的 SDK 保留状态字符串，
Gateway 和前端只消费该投影。实际变化只发生在 Rust 主链内部：新的 built-in
Capabilities 路径不再依赖 Host 的错误文本分类，Legacy fixture 的显式失败文本
仍保持 `Failed`。

因此 Web 不需要新增 API、状态枚举或本地缓存；它受益于状态来源更稳定，但必须继续
把 `content` 当作诊断信息，不能据此决定自动重试、重新发起审批或改变 Thread 状态。

## Verification

| 仓库 | 命令 | 结果 |
| --- | --- | --- |
| mini-codex | `cargo fmt --all -- --check` | passed |
| mini-codex | `cargo test -p mini-agent-protocol --lib` | 7 passed |
| mini-codex | `cargo test -p mini-agent-capabilities --lib` | 77 passed |
| mini-codex | `cargo test -p mini-agent-host --lib` | 34 passed |
| mini-codex | `cargo test -p mini-agent-app-server --lib` | 57 passed |
| mini-codex | affected-package Clippy (`protocol`, `capabilities`, `host`, `app-server`) | passed |
| mini-codex | `python scripts/line_budget.py --base ae68147 --check-delta --json` | 无 violation；runtime `+96`、release Rust `+200`、control plane `+136`，当前 green |
| mini-agent-web | `uv run pytest -q tests/sdk/test_sdk_events.py tests/sdk/test_sdk_apis.py tests/gateway/test_gateway_goals_and_items.py tests/gateway/test_session_manager.py` | 83 passed；2 个既有 Windows asyncio 资源析构 warning；无 Web 代码改动 |
| mini-codex | `python scripts/check_iteration_note.py .agents/notes/implemented/architecture/2026-09-15-typed-tool-outcome-boundary.zh.md` | passed |

## Consequences

- `Core → Host → App Server` 的状态流现在有清晰的 typed boundary，主链维护者不必
  在多个 crate 之间追踪相同的错误前缀。
- Host 的 `ToolOrchestrator` 更短且只处理 admission/approval 的正交责任；旧工具
  可逐步迁移到显式 outcome，不需要先改造全部 `ToolError` 构造点。
- MCP timeout、Shell timeout 和 Plan-mode lock 的模型可见结果更稳定；App Server
  和 Web 的公共协议保持兼容。
- 本批新增少量协议/Host/Capabilities 代码，runtime 与 release 仍在增量门禁内。

## Remaining risks

- `ToolError` 仍是字符串兼容类型；未迁移的 provider/Legacy 工具仍可能在自身内部
  选择错误 status，后续应按具体实验逐个迁移，不能恢复全局文本分类器。
- 本批覆盖了 Host、Capabilities 和 App Server 的 bounded/unit/public projection
  路径，但没有调用真实 provider、真实 MCP transport 或完整 workspace suite。
- Web 未改代码，因此没有新增前端 build 证据；若未来扩展 public outcome 枚举，必须
  先补 SDK、Gateway、前端和 protocol fixture 的兼容测试。
