# Web Cookbook 恢复与投影契约

状态：implemented  
Date: 2026-09-15  
Batch: Cookbook 与 SDK 契约同步，Batch 2  
Scope: 离线恢复、Session fork 投影、EOF 处理和执行策略

## Decision

Cookbook 新增无 Provider 的恢复示例，按既有 SDK 读取顺序调用
`get_runtime_status`、`replay_events`、`list_thread_items` 和 `read_thread`。
fixture 将 `projectId/threadId/turnId` 保持在同一身份中，过滤过期和跨 Turn
事件，并把重复 Item 投影收敛到最后一条记录。

示例将 `stopping`、运行中的 checkpoint 和当前 Turn 缺少 `turn_finished` 视为
未 settlement。示例不推断完成、不重试 Turn、不审批工具，也不执行本地副作用。
Session fork 结果和 context-policy 冲突使用既有 SDK 类型。

Steer 和 interrupt 示例在控制调用与 stream 操作处捕获 `ServerProcessError`，
EOF 后尝试读取权威 runtime、Item 和 checkpoint。Workflow 示例展示独立的
Project access 与 approval policy，并说明 `trusted` 不是 allow-all。

## Harness hypothesis

如果 Cookbook 恢复示例通过 App Server 既有读取接口对账，那么 EOF 或迟到事件
不会被误判为 Turn 完成，投影身份不会漂移，客户端示例也不会形成第二套执行状态机。

## Ownership and boundaries

- App Server 与 mini-codex 负责 settlement、SessionStore 权威状态和公共 runtime 投影。
- SDK 负责 runtime、event、Item、checkpoint、fork 和 conflict 的传输与 typed parsing。
- Cookbook 只展示调用顺序和结果，不重试 Turn、不审批 action、不复制授权逻辑。

## Cross-repository contract

恢复流程使用既有的 `runtime/status`、`turn/events`、`thread/items/list` 和
`thread/read`。Stopping runtime 不等于完成 Turn。`ThreadItem.status` 继续表示
生命周期，`ThreadItem.outcome` 继续表示工具结果。Gateway 和 Web Studio 继续消费
这些字段，不建立本地权威状态。

## Verification

- `uv run python cookbook/python-demo/07_recovery_and_projection.py`：通过，覆盖
  stopping、重复数据、迟到事件、身份和 fork conflict fixture。
- `uv run python cookbook/python-demo/06_protocol_compatibility.py`：通过。
- `uv run pytest -q tests/cookbook tests/sdk`：43 passed。
- `uv run ruff check cookbook/python-demo sdk/python tests/cookbook tests/sdk`：通过。
- `uv run ruff format --check cookbook/python-demo sdk/python tests/cookbook tests/sdk`：
  19 个文件已格式化。
- `git diff --check`：通过，仅有 Windows 换行提示。
- 未启动 App Server、未调用 Provider、未修改前端生产代码。

## Remaining risks

- 恢复示例使用确定性 fixture，未注入真实 child process 崩溃或恶意传输故障。
- Live Steer 和 interrupt 仍是 Provider 示例，只做编译检查，不进入默认 CI 场景。
- 重连权威仍依赖 App Server 与 SessionStore 可用；Cookbook 不增加 cache fallback。
