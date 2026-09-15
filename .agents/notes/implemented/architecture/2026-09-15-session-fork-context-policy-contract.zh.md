# Session fork 上下文策略与结果元数据契约

状态：implemented
日期：2026-09-15
批次：Iteration 3，Session fork 的持久化结果与策略冲突
范围：`mini-codex` Capabilities/App Server 与 `mini-agent-web` Gateway

## Decision

child Session 的 header 在已有 lineage 旁持久化一组有界 fork 元数据：
`context_policy`、压缩前后字节数、`compacted` 和 `method`。Capabilities
`SessionStore` 负责读取、校验和匹配这些字段；App Server 在 Core 准备上下文前
先按父 Session、checkpoint、child Thread ID 和策略查找已有 fork。

相同请求直接返回持久化结果，不再次准备上下文或调用模型。相同 child Thread ID
若已经使用另一种策略，则拒绝请求。返回给协议层的压缩结果来自持久化元数据，避免
重试时用本次请求重新计算出的临时值覆盖历史事实。

## Harness hypothesis

如果 fork 的策略和压缩结果随 child Session 一起落盘，并且重试在 Core 介入前完成
查找，那么进程重启、网络重试或 Gateway 重连都能观察到同一个结果；策略冲突也会
在持久化边界被拒绝，而不是创建第二个 child 或返回不一致的指标。

反例包括：同一 child 的重复请求再次触发模型、重试返回不同的压缩 method/字节数、
策略被 Gateway 本地状态静默改写，或 header 中出现未校验的任意元数据。

## Ownership and boundaries

| 对象 | 唯一权威 | 本批变更 | 禁止行为 |
| --- | --- | --- | --- |
| fork 元数据和 child 身份 | Capabilities `SessionStore` | 在 Session header 持久化并校验 | Gateway 创建第二套 Session 真相 |
| 上下文准备与模型调用 | Core Harness，经 App Server worker 调度 | 仅首次 fork 进入准备路径 | 重试绕过 worker 或重新调用模型 |
| RPC 结果映射 | App Server | 从持久化元数据构造 `SessionForkResult` | 用当前请求的临时值覆盖历史结果 |
| child 路由与项目绑定 | Web Gateway `ClientPool` | 保存并检查 `context_policy`，复用 child client | 把 Gateway metadata 当 Session 权威 |

## Cross-repository contract

| 语义 | mini-codex | mini-agent-web |
| --- | --- | --- |
| 策略 | 协议 `exact`/`compact` 映射到有界持久化字段 | REST 请求继续使用 `context_policy` |
| 相同重试 | `find_fork` 在 Core 准备前返回已有 child 和原始指标 | 已绑定 child 返回保存的结果，不重复 fork |
| 策略冲突 | SessionStore 返回错误，不覆盖 index 或 child | Gateway 在已有绑定上返回 HTTP 409 |
| 旧 Session | 无新元数据的旧 header 仍可按 lineage 发现，并由首次兼容路径补齐返回值 | 没有 `context_policy` 的旧 Gateway metadata 不强行推断策略 |
| 权威关系 | SessionStore 文件、锁和 header | Gateway 只保存路由派生状态，不写 Session 文件 |

## Verification

- Capabilities 测试覆盖 header 元数据、相同策略重试和不同策略冲突。
- App Server 的 JSON-RPC 场景验证相同 fork 重试在 Core 准备前返回持久化结果，模型
  调用次数不增加，并验证策略冲突被拒绝。
- Gateway 测试覆盖同一 fork 的重复请求只创建一个 child，以及策略冲突返回 `409`。

验证命令与结果：

| 仓库 | 命令 | 结果 |
| --- | --- | --- |
| mini-codex | `cargo fmt --all` | passed |
| mini-codex | `cargo test -p mini-agent-capabilities --lib` | 77 passed |
| mini-codex | `cargo test -p mini-agent-app-server --lib` | 57 passed |
| mini-codex | `cargo clippy -p mini-agent-capabilities --all-targets -- -D warnings` | passed |
| mini-codex | `cargo clippy -p mini-agent-app-server --all-targets -- -D warnings` | passed |
| mini-codex | `python scripts/line_budget.py --base 4ecd041 --check-delta --json` | runtime `+96`，release `+248`，control plane `+247`，无 violation |
| mini-agent-web | `uv run pytest -q tests/gateway/test_gateway_goals_and_items.py tests/gateway/test_session_manager.py` | 56 passed，2 个既有 Windows asyncio 资源析构 warning |
| mini-agent-web | `uv run ruff check server/control/client_pool.py tests/gateway/test_gateway_goals_and_items.py` | passed |
| mini-codex | `python scripts/check_iteration_note.py .agents/notes/implemented/architecture/2026-09-15-session-fork-context-policy-contract.zh.md` | passed |

## Consequences

- fork 结果在进程边界和网络重试中具有稳定来源，App Server 不需要临时结果缓存。
- `SessionForkInfo` 增加可选 metadata，以读取旧格式；新格式由创建路径强制写入。
- 每次新 fork 增加少量 header 字段和一次受锁保护的 preflight 查找，换取可恢复的
  结果一致性。本批相对 `4ecd041` 增加 runtime `96` 行、release Rust `248` 行、
  control plane `247` 行，仍处于 green 区间。

## Remaining risks

- 旧版本已创建且没有 metadata 的 Session 无法恢复历史压缩 method；当前兼容路径只能
  使用本次准备结果补齐响应，不能把推断值当作历史事实。
- Rust App Server 的策略冲突目前通过 checkpoint 错误返回，尚未在公共协议中建模为
  独立 conflict code；Web Gateway 能将本地已绑定冲突映射为 `409`，跨进程冲突仍需
  后续统一错误契约。
- 本批不运行真实 provider、完整 workspace 测试或前端构建。
