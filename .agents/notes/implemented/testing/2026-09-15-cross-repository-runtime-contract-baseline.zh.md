# Runtime 子系统跨仓契约基线

状态：implemented
日期：2026-09-15
批次：Iteration 0，基线与契约固化
范围：`mini-codex`、`mini-agent-web` 的 Core、Capabilities、App Server、Python SDK、Gateway 和协议兼容 fixture

## Decision

本批次不改变运行时权限、Session fork 默认策略或停止语义。将当前已落地的行为
固定为后续迭代的可复验基线，并把 `mini-agent-web` 作为 `session/fork`、Stopping、
item identity 和审批生命周期的契约消费者纳入验证范围。

当前采用的跨层顺序为：

```text
Core checkpoint / event
  -> App Server worker / JSON-RPC
  -> Capabilities SessionStore / approval authority
  -> Python SDK types
  -> Gateway client binding / approval bridge
  -> Web Studio state
```

## Harness hypothesis

如果父 Thread 只从已结算 checkpoint 派生，Exact fork 不调用模型，停止请求在
`turn_finished` 前保持 Stopping，并且 Web 只消费结构化 RPC 和事件，那么父子 Session、
停止中的 Turn、审批请求和 UI 状态可以在重启、重连和多客户端场景中保持一致。

反例包括：Web 复用父 client 或父 Session、停止响应被当成终态、迟到审批重新打开
已停止 Turn、`session/fork` 的字段被 SDK 静默丢弃，或协议新增 phase 使旧 SDK 解析失败。

## Ownership and boundaries

| 对象 | 唯一权威 | Web 允许做什么 | Web 禁止做什么 |
| --- | --- | --- | --- |
| Turn loop、停止结算、事件顺序 | Core 与 App Server worker | 展示 phase，等待 `turn_finished` | 用本地状态伪造终态或重排事件 |
| 工具准入、批准、ActionGrantKey | Host/Capabilities | 转发审批请求和结果 | 保存第二份 grant 或自行放行 |
| Session、checkpoint、父子 lineage | Capabilities `SessionStore` | 使用 RPC 返回的 Session ID attach/resume | 读取、复制或修改 `session.jsonl` |
| RPC 类型和事件投影 | App Server Protocol / Python SDK | 保留未知事件并投影有界字段 | 用布尔值替代结构化 phase 或 identity |
| child client 与 Project 绑定 | Gateway `ClientPool` / `SessionManager` | 建立独立 client、项目映射和重连 | 让父子 Session 共享 writer 或审批缓存 |

## Cross-repository contract

| 语义 | mini-codex | mini-agent-web | 基线结论 |
| --- | --- | --- | --- |
| 独立派生 | `session/fork` 返回 child/parent Session、checkpoint 和大小指标 | `ClientPool.fork_thread` 调用 `fork_session`，再以 `resume` 启动 child client | 已对齐，`thread/fork` 仍保留为进程内逻辑 fork |
| 默认压缩策略 | `ForkContextPolicy::Exact`，Exact 不调用模型 | REST、SDK 默认 `context_policy="exact"` | 已对齐，Compact 必须显式传入 |
| 停止生命周期 | Runtime phase 使用 `Stopping`，终态由 `turn_finished` 收口 | Gateway 保留 active Turn，不把 interrupt accepted 当成完成 | 已对齐 |
| item identity | model item 使用 `item_id`，工具调用使用 `call_id` | SDK/Gateway 保留 item ID 和 turn identity | 已有 fixture，后续补更强的跨重连断言 |
| 审批 evidence | App Server 旁路写入有界 JSONL，不参与授权 | Gateway 只维护 pending bridge，不缓存 grant | 已对齐 |

## Verification

代码基线：

- mini-codex：`6f299a4`，`git status --short --branch` 干净。
- mini-agent-web：`1bbac60`，`git status --short --branch` 干净。
- mini-codex line budget：runtime `20,217`，release Rust `30,160`，control plane
  `20,995`，runtime/release 均为 green。

已运行：

| 仓库 | 命令 | 结果 |
| --- | --- | --- |
| mini-codex | `cargo test -p mini-agent-core --lib` | 42 passed |
| mini-codex | `cargo test -p mini-agent-capabilities --lib` | 75 passed |
| mini-codex | `cargo test -p mini-agent-app-server --lib` | 56 passed |
| mini-agent-web | `uv run pytest -q tests/gateway/test_gateway_goals_and_items.py tests/gateway/test_session_manager.py` | 56 passed，1 个既有 asyncio 子进程析构 warning |
| mini-agent-web | `uv run python cookbook/python-demo/06_protocol_compatibility.py` | 14 个 protocol event fixture passed |
| mini-codex | `python scripts/test_iteration_note.py` | passed |

本批新增的可复验工具是 `scripts/check_iteration_note.py`。它检查 implemented note
是否包含假设、所有权、跨仓契约、验证和剩余风险，避免批次完成后只留下“已完成”描述。

## Budget

本批没有修改 Rust 运行时代码，runtime/release 生产行为 delta 为 `0`。新增内容是
notes、notes 检查脚本和 Web 协议 fixture 的验证面；下一批如修改 Rust，必须用
`python scripts/line_budget.py --base <merge-base> --check-delta --json` 记录实际 delta，
并遵守当前非红区 runtime `+200`、release `+300` 的增量门禁。

## Remaining risks

- `SessionStore::fork_from_checkpoint` 尚未证明相同父 checkpoint 与相同
  `newThreadId` 的重复请求是幂等的；下一批优先验证并决定冲突语义。
- Web 的 Gateway 测试覆盖了独立 child client 和并发 attach，但尚未覆盖 App Server
  在 fork 写入成功后、child client 启动前崩溃时的跨进程恢复场景。
- Gateway 测试存在一个既有的 Windows asyncio 子进程析构 warning；它不影响本批断言，
  但应在后续 runtime EOF/reconnect 批次中清理。
- 本批未运行真实 provider、完整 workspace 测试或前端构建；后续只在影响对应层时运行。

## Next batch

Iteration 1 聚焦停止与身份契约，Iteration 2 聚焦 Session fork 的重复请求、部分失败
和 attach/retry。每批完成后更新本目录新的主题记录，不在本文件末尾追加工作日志。
