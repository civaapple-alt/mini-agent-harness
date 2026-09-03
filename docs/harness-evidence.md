# Harness Scenario 与 Evidence 基线

Status: current evidence guide

## 目的与范围

不先添加通用 benchmark framework，使用现有 CLI/App Server 公共路径建立小型、
可复现、临时 workspace 隔离的 harness scenario 集。评估同时观察 resolution、
round/step 数、tool-call 数、token/byte 使用、latency、失败分类和边界违规，
不把单一模型分数当作门槛，也不调用付费 provider。

每个场景至少记录：输入、允许的工具面、事件或 turn trace 摘要、最终文件或
Session 状态、是否越界和耗时。模型输出、工具结果、fixture 和 trace 均使用
已有 hard limit；公共边界测试仍是最低证据。

## 第一版 bounded scenario baseline

第一版复用现有 CLI 公共集成测试，在临时 workspace 中运行本地 TCP mock provider。
以下耗时是 Windows 本地单测墙钟时间，包含 Cargo 增量检查和启动开销，只用于
发现异常，不是模型质量指标。

| 场景 | 公共测试 | 可审计结果 |
| :--- | :--- | :--- |
| 上下文组装与受限 skill 摘要 | `ask_reads_stdin_and_keeps_machine_output_clean` | 请求含 user/world/instructions，未泄漏完整 skill body，JSON 输出保持机器可读 |
| 无工具运行时退化 | `ask_no_tools_uses_model_only_scope_without_extension_tools` | `tools=[]`，运行时组合为无工具，disabled 原因可见 |
| durable Session resume | `durable_session_resumes_settled_history_after_restart` | 重启后请求同时包含旧问题、旧答案和新问题，两个 turn 均落盘 |
| Goal tool turn 与 verifier | `goal_mode_runs_a_tool_turn_and_verifies_the_settled_history` | verifier 输入含 tool evidence，Goal 状态为 `converged` |
| timeout → interrupt → failed | `goal_mode_timeout_is_deterministic_and_keeps_repl_alive` | timeout 后 turn settled、Goal 为 `failed`，REPL 无 `Busy` 残留 |
| restart recovery | `running_goal_is_paused_when_a_session_restarts` | 不重放活动 turn，恢复后状态为 `user_paused` |
| steer 安全检查点 | `steer_interrupts_a_running_turn_at_a_checkpoint` | 第一 turn 保存 `steered`，第二 request 使用新消息 |
| follow-up 排队 | `follow_up_is_queued_until_the_running_turn_finishes` | 第一 request 结束后才发送第二 request |

命令：

```text
cargo test -p mini-agent-cli --test interactive <scenario> -- --exact
```

本批 8/8 通过。该基线证明公共路径可承载基础 harness evidence，但不把当时
尚未覆盖的跨文件重构、CLI 工具失败恢复、MCP/approval/sandbox 拒绝或独立
provider 对比倒填为基线；后续补充结果应以新的 dated note 记录。

## Failure / timeout / retry 矩阵

| Fault class | 证据 | 状态 | 剩余缺口 |
| :--- | :--- | :--- | :--- |
| 缺少必要工具参数 | Core `missing_required_tool_argument_is_projected_for_model_recovery` | covered | 保持下一轮可见的 bounded Tool result |
| 未知工具恢复 | CLI `ask_recovers_from_unknown_tool_on_public_path` | covered | 更广泛 provider 脏参数未穷举 |
| 模型部分流失败 | Core `partial_model_stream_is_failed_without_fabricating_completion` | covered | 真实 provider 截断仍需矩阵 |
| Retryable 工具结果 | Core `retryable_tool_result_is_preserved_until_model_recovers` | covered | 无隐式重试，策略层 deferred |
| HTTP 429 | Capabilities `maps_http_429_to_bounded_api_error_without_retrying` | covered: bounded fail-fast | provider-specific retry/backoff deferred |
| shell timeout | Capabilities `shell_process_has_a_timeout` | capability boundary covered | CLI/App Server 公共路径未独立覆盖 |
| turn/Goal timeout | App Server/CLI Goal timeout scenario | public path covered | 与 tool timeout 的组合矩阵未覆盖 |
| approval/MCP refusal | App Server `NeedsApproval`、Capabilities MCP denial | covered | 沿用既有 event projection |
| MCP call timeout | Capabilities controlled slow call、App Server public projection | boundary covered | CLI actual MCP transport deferred |
| MCP circuit breaker | Capabilities `circuit_breaker_trips_after_failures_and_recovers` | unit-only | 真实失败到 model round 的公共路径未覆盖 |
| Docker sandbox | availability、mount、ephemeral filesystem probe | host runtime covered | 更强隔离仍需 policy 和跨平台证据 |

## 不变量与证据门槛

- Approval 或 sandbox 拒绝必须产生非空、结构化、下一步模型可见的拒绝结果；
  不能用缺失工具或空字符串冒充 denial evidence。
- timeout、cancel、steer 竞态必须记录确定性顺序；保持 safe-checkpoint settlement
  和 durable checkpoint 顺序，不从偶然墙钟结果推导优先级。
- 超过五秒不是自动忽略理由；只有确定性慢场景可显式 `#[ignore]`，并提供
  定时或手动命令。
- 已退役的外部 `--trace` 路径不得恢复；trace 复用现有 observation events 和
  Session records，保持 bounded、redacted，并明确是内部 artifact 还是公共协议。
- Docker evidence 必须区分 daemon、workspace mount、容器临时文件和完整安全隔离；
  network、capability、privilege、read-only 或 resource policy 需要单独契约和边界测试。

## 维护规则

本文件维护当前 baseline、矩阵和证据门槛。新的实验结果先写入
`.agents/notes/` 的日期变更记录；确认改变当前基线后，再直接更新本文件，避免
用另一个索引文档串联历史。
