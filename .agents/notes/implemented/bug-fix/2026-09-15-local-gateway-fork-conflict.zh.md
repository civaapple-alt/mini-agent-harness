# Gateway 本地 Session fork 冲突统一契约

状态：implemented
日期：2026-09-15
批次：Iteration 5，统一本地与跨进程策略冲突
范围：`mini-agent-web` Gateway 的本地 child binding 与 `session/fork` HTTP 边界

## Decision

Gateway 在发现已绑定 child 使用不同 `context_policy` 时，使用独立的
`SessionForkConflictError`，不再把本地错误伪装成来自 App Server 的
`AppServerError`。HTTP 路由将本地异常与跨进程 JSON-RPC 异常映射到相同的
`-32001`、`contextPolicy` 数据形状。

Project binding 冲突仍是 Gateway 自身的另一类绑定错误，不扩展 Session fork 的
跨仓协议枚举。

## Harness hypothesis

如果本地和跨进程的 child 策略冲突共享稳定的 code/data 契约，同时保留各自的内部
错误所有权，调用方就不需要区分冲突来源，维护者也不会误以为 Gateway 能代表
App Server 生成内部错误。

反例包括：本地冲突仍返回字符串、不同来源产生不同 code，或把 Project binding
冲突错误标成 Session context policy 冲突。

## Ownership and boundaries

| 对象 | 唯一权威 | 本批变更 | 禁止行为 |
| --- | --- | --- | --- |
| 本地 child policy 检查 | Gateway `ClientPool` | 使用 typed `SessionForkConflictError` | 构造伪造的 App Server transport 状态 |
| 跨进程 policy 检查 | App Server SessionStore | 继续消费 `AppServerError` 的 `-32001` | Gateway 重复扫描或修改 Session 文件 |
| HTTP 语义 | `server/routes/threads.py` | 两条路径都输出结构化 HTTP 409 | 让客户端解析自然语言判断冲突 |
| Project binding | Gateway binding registry | 保持独立的 409 字符串错误 | 塞进 `contextPolicy` 数据枚举 |

## Cross-repository contract

本批不改变 `mini-codex` wire contract：`-32001` 表示 Session child 身份或策略冲突，
策略冲突的 `data.kind` 为 `contextPolicy`，并包含 bounded child 与策略字段。
`mini-agent-web` 的本地和跨进程路径均遵守这一 HTTP 409 适配契约。

## Verification

- Gateway fork 路由测试：10 passed。
- 本批新增的 Gateway typed error、路由、测试通过 Ruff 与格式检查。
- `git diff --check` 通过。
- 前一批 Rust、SDK、Gateway 全量目标测试与协议 smoke test 已通过；本批未改 Rust。

## Consequences

- 本地重试与跨进程重试现在返回同样的机器可读冲突信息。
- Gateway 内部错误类型不再混淆 SDK 的 App Server transport error 语义。
- 新增一个很小的 Gateway 控制面错误类型，但没有扩展公共 JSON-RPC schema。

## Remaining risks

- 同一 child ID 绑定到不同 Project 的冲突仍是独立字符串 detail，后续若需要统一，
  应先定义 Project binding 的独立契约，不能复用 `contextPolicy`。
- 旧 Session 缺少策略 metadata 时仍无法证明历史策略，只能保持兼容。
