# Session fork 结构化冲突契约

状态：implemented
日期：2026-09-15
批次：Iteration 4，跨进程 conflict code 与错误数据
范围：`mini-codex` Capabilities/App Server/Protocol 与 `mini-agent-web` SDK/Gateway

## Decision

Capabilities 将 child Thread 冲突建模为 typed `SessionForkError`，区分父 lineage
冲突、上下文策略冲突和普通存储错误。App Server 不再通过错误文本判断冲突，而是
将前两类映射为 `AppServerError::SessionForkConflict`。

JSON-RPC 使用固定 code `-32001`。`error.data.kind` 是 `parentLineage` 或
`contextPolicy`，字段只包含有界的 child Thread 和策略信息。已准入 action 的
`actionId`、`actionSequence`、`stateRevision` 会与 conflict data 合并，避免错误
详情覆盖已有的 action 追踪字段。

Python SDK 导出 `SESSION_FORK_CONFLICT_CODE` 并原样保留 `AppServerError.data`。
Gateway 对该 code 返回 HTTP 409 和结构化 detail，其他 App Server 错误仍返回 400。

## Harness hypothesis

如果持久化层产生 typed conflict，App Server 在协议边界映射固定 code，并且错误
元数据在 action envelope 中合并，那么跨进程重试和 Gateway 重连都能区分“child
身份冲突”和普通 checkpoint 故障，调用方也能稳定决定是否更换 child Thread ID。

反例包括：Gateway 依赖错误文本、冲突被当作 400 或 500、action metadata 被覆盖，
或父 lineage 冲突被错误标记为策略冲突。

## Ownership and boundaries

| 对象 | 唯一权威 | 本批变更 | 禁止行为 |
| --- | --- | --- | --- |
| 冲突分类 | Capabilities `SessionStore` | 用 `SessionForkConflict` 表达两类身份冲突 | App Server 匹配存储错误文本 |
| JSON-RPC code/data | App Server Protocol + App Server adapter | 固定 `-32001`，合并 action metadata | 用通用 `-32000` 淹没冲突原因 |
| SDK 错误保留 | Python SDK `AppServerError` | 导出 code 常量并保留 data | 解析时丢弃结构化 data |
| HTTP 状态映射 | Web Gateway route | conflict code 转 409，其余错误仍为 400 | Gateway 修改 Session 文件或伪造 Session 权威 |

## Cross-repository contract

| 语义 | mini-codex | mini-agent-web |
| --- | --- | --- |
| code | `SESSION_FORK_CONFLICT_CODE = -32001` | SDK 导出同名常量 |
| conflict data | tagged `kind`，支持 `parentLineage`、`contextPolicy` | `AppServerError.data` 原样进入 HTTP 409 detail |
| action metadata | 与冲突字段合并保留 | 不解释 action 顺序，只保留给调用方 |
| 普通错误 | 继续使用 `-32000` 或既有 JSON-RPC code | 继续映射为 HTTP 400 |
| 重试建议 | 冲突时不能复用同一 child ID 改策略或父 lineage | 调用方读取 409 detail 后更换 child ID |

## Verification

- Capabilities 测试：77 passed。
- App Server Protocol 测试：18 passed。
- App Server 测试：57 passed，包含公共 JSON-RPC fork conflict 场景。
- Gateway、SessionManager、SDK 测试：63 passed，保留 2 个既有 Windows asyncio
  资源析构 warning。
- 协议兼容 smoke test 通过，包含结构化 fork conflict fixture。
- Rust affected-package clippy、Python Ruff 和 `cargo fmt --all` 通过。
- `python scripts/line_budget.py --base 233c94a --check-delta --json` 无违规，
  runtime `+81`、release Rust `+165`、control plane `+163`，当前仍为 green。

## Consequences

- SDK、Gateway 和其他客户端可以按 code/data 处理 conflict，不必依赖自然语言。
- action 追踪信息和错误原因同时可见，便于日志、重试和 UI 展示。
- Capabilities 错误边界更明确，但 `SessionStore::fork_from_checkpoint` 的错误
  返回类型从字符串变为 enum，嵌入方需要按 variant 处理。

## Remaining risks

- Gateway 内部已绑定 child 的本地冲突仍由 `RuntimeError` 产生，状态码是 409 但
  detail 还是字符串；后续可让本地和跨进程路径共用同一 Gateway conflict 类型。
- 旧 Session 没有策略 metadata 时仍只能兼容 lineage，无法证明历史压缩策略。
- 本批不运行真实 provider、完整 workspace 测试或前端构建。
