# Stopping、EOF 与跨仓恢复契约

状态：implemented
日期：2026-09-15
批次：Iteration 9，完成 Gateway/Web 的停止与运行时断线恢复边界
范围：`mini-agent-web` Gateway、Session catalog、Web Studio；依赖本仓库既有 App Server/SessionStore 权威状态

## Decision

传输任务的取消和 App Server EOF 都不是 Turn 已完成或失败的证据。Gateway
不再合成 `turn_finished`，也不再把 `ServerProcessError` 广播为终止性 Turn
错误；最终结算继续只来自 App Server 的权威事件或持久化状态。

fork 的 SessionStore 结果在 child client 启动前写入 Gateway catalog。子进程
启动失败时，已持久化的 `projectId + threadId + sessionId` 仍可被下一次请求
发现并重试，不重复创建 Session。

Web Studio 将 WebSocket 断线和 runtime 错误显式投影为 `reconnecting`，使用
既有 `attachThread`、`runtime/status`、`thread/items/list` 和事件 replay 对账。
恢复链不新增公共恢复接口、不复制 Gateway 执行状态机，也不取得授权副本。

## Harness hypothesis

如果停止、EOF、child 启动失败和重连都保留“未结算”事实，并由 SessionStore、
App Server runtime/status 和 Thread Item history 共同对账，那么恢复不会伪造
完成/失败，不会漂移 `projectId + threadId + turnId`，也不会因迟到或重复事件
生成第二份执行状态。

## Ownership and boundaries

| 边界 | 权威 | 本批行为 | 禁止行为 |
| --- | --- | --- | --- |
| App Server Turn 结算 | App Server/Core | 继续发布真实 `turn_finished` 或持久化状态 | Gateway 以 EOF/取消推断结果 |
| Session 身份 | SessionStore | 先持久化 fork，再启动 child client | Gateway 重新生成 Session |
| Gateway | SessionManager/ClientPool | 保留可 attach catalog，转发可恢复 runtime 错误 | 保存授权副本或新增恢复状态机 |
| Web Studio | 既有请求与 replay 投影 | 展示 reconnecting 并重新对账 | 前端执行 retry、approval 或本地恢复 |

## Cross-repository contract

`ThreadItem.status` 继续表示生命周期，`ThreadItem.outcome` 继续表示工具结果。
Turn 的 `turn_finished` 是结算证据；WebSocket 取消、EOF 和运行时错误只是
传输/可恢复性信号。身份始终由 `projectId + threadId + turnId` 绑定，重连不得
用当前选中会话替换事件自身的身份。

## Verification

| 仓库 | 命令 | 结果 |
| --- | --- | --- |
| mini-agent-web | `uv run pytest -q tests/gateway/test_gateway_agent.py tests/gateway/test_session_manager.py tests/sdk/test_sdk_events.py` | 85 passed；2 个既有 Windows asyncio 资源析构 warning |
| mini-agent-web | `uv run ruff check server/control/client_pool.py server/routes/agent_ws.py tests/gateway/test_gateway_agent.py tests/gateway/test_session_manager.py` | passed |
| mini-agent-web | `npm test` | Node 53 passed；Vitest 36 passed |
| mini-agent-web | `npm run build` | passed；保留既有 Vite chunk size warning |
| mini-agent-web | `npm run lint` | passed |

## Consequences

- interrupt 已接受但进程在 `turn_finished` 前退出时，不会被 Gateway 伪造为 interrupted。
- child client 启动失败后，fork catalog 记录仍可 attach/retry。
- Web Studio 能区分连接恢复中与 Turn 已完成，恢复完成后再接受 replay 事件。

## Remaining risks

- 本批没有调用真实 provider，也没有执行完整受控子进程崩溃注入；现有证据由边界 fixture 覆盖。
- 若 SessionStore 本身不可读，Web 只能展示恢复失败并要求重新 attach，不能凭 Gateway 缓存重建权威状态。
- Batch 10/11 仍需继续迁移 Legacy 工具和补齐 Plan Mode、resolver、context compaction 的 Harness Scenario/Eval 证据。
