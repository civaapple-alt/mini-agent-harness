# Goal Revision 与 Web 重连游标证据

* **日期**: 2026-09-08
* **状态**: implemented
* **Class**: testing
* **范围**: Goal 生命周期通知、SDK/Gateway 投影、Web Studio 重连恢复

## 决策

Goal 与 Thread settings 共用 App Server 的 `RuntimeRevision`，不增加第二个
Goal 版本计数器。Goal set/clear 在 mutation 完成后发送当前 revision；Goal turn
在本轮最终 revision 提交前发送 next revision，使通知描述的 Goal 快照不会落后
于其对应的控制面 mutation。Goal action result、JSON-RPC notification、SDK typed
result、Gateway REST response 和 Web Studio 都保留 `stateRevision`。

Web Studio 对每个 Thread 使用同一单调 cursor 消费 settings 与 Goal notification。
历史读取不再直接覆盖 Plan/Goal 控制面状态；WebSocket reconnect 会先清除旧的
进程内 cursor，再读取 canonical workflow projection，处理 App Server 重启后
revision 序列重置。

## Evidence matrix

| 边界 | 成功证据 | 失败反例 | 结果 |
| --- | --- | --- | --- |
| App Server Goal revision | `json_rpc::tests::exposes_codex_shaped_thread_goal_lifecycle` 验证 Goal action result、updated/cleared notification 的 `stateRevision`，并验证 clear notification 与 response 一致 | Goal event 在 mutation 前使用旧 revision | covered |
| SDK/Gateway projection | `tests/sdk/test_sdk_apis.py::test_thread_goal_api_mapping_without_starting_goal_runtime`、`tests/gateway/test_gateway_goals_and_items.py::test_goal_lifecycle_full_state_machine` | Goal action response 丢失 revision，Gateway 退回自行生成状态 | covered |
| Web projection | `revision_state.test.js`；App 与 SidePanel 均按 Thread 单调应用 Goal state | stale Goal notification 覆盖较新的 Goal/Plan state；history read 覆盖 notification | covered |
| Reconnect recovery | WebSocket `onopen` 重新读取 `get_workflow_state()` 并重建 cursor | App Server restart 后旧 Web cursor 永久拒绝新 revision | covered for browser reconnect |

## Verification

mini-codex:

```text
cargo fmt --all
cargo test -p mini-agent-app-server-protocol -p mini-agent-app-server
cargo clippy -p mini-agent-app-server -p mini-agent-app-server-protocol --all-targets -- -D warnings
python scripts/line_budget.py
python scripts/cargo_boundary.py --json
```

mini-agent-web:

```text
.venv\Scripts\python.exe -m pytest tests/sdk/test_sdk_apis.py tests/sdk/test_sdk_events.py tests/gateway/test_gateway_goals_and_items.py -q
.venv\Scripts\ruff.exe check sdk/python/src/mini_agent server tests/sdk/test_sdk_apis.py tests/gateway/test_gateway_goals_and_items.py
cd frontend
npm run test
npm run lint
```

结果：App Server `51`、App Server Protocol `14`、Python SDK/Gateway 目标集成
`26`、前端 Node `17`、Vitest `8` 全部通过；line budget 为 runtime
`18,044/20,000`、release Rust `27,528/30,000`，Cargo boundary 无 violation。

## 未证明事项

- Runtime revision 仍是 App Server 进程内序列；如果 Gateway 内部重启但浏览器
  WebSocket 没有断开，当前没有跨进程 generation notification，仍需后续 seam。
- 本批没有完成 fork 与并发 attach 的组合场景。
- reconnect 证明的是浏览器 cursor 重建，不等价于跨进程事件 replay；恢复仍以
  canonical workflow read 为准。
