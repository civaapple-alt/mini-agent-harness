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
| 上下文组装与受限 skill 摘要 | `run_reads_stdin_and_keeps_machine_output_clean` | 请求含 user/world/instructions，未泄漏完整 skill body，JSON 输出保持机器可读 |
| 无工具运行时退化 | `run_no_tools_uses_model_only_scope_without_extension_tools` | `tools=[]`，运行时组合为无工具，disabled 原因可见 |
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

## Control Plane boundary evidence（2026-09-07）

line gate 的后续计划要求 Permission、Sandbox、Recovery、Audit 不能只停留在
分类统计，而要有可观察的成功路径和失败反例。本轮复用现有确定性测试，不增加
新的 benchmark framework、Provider 调用或运行时兼容层。

| 边界 | 成功路径 | 失败反例 | 状态 |
| :--- | :--- | :--- | :--- |
| Permission / grant | scoped approval 精确匹配 owner/revision；`trusted + project` 普通 patch 直接准入；批准后的 workspace 外部图片读取 | security deny 优先级；外部读取拒绝；Shell 拒绝先于 sandbox；trusted 下删除和高风险 Shell 仍需 approval；App Server 公共 approval denial | covered |
| Sandbox | Native guard；Docker workspace mount 与 ephemeral `/tmp` | Shell timeout；只读 Shell 拒绝副作用参数 | covered with platform/daemon caveat |
| Recovery | Goal pause/resume；恢复 checkpoint 不重放首轮 | Goal 清除后忽略旧 verifier；checkpoint 改变后忽略旧结果；拒绝/失败 verdict | covered |
| Audit | bounded、redacted trace；round metadata 与输出记录 | unbounded trace id；artifact 总量超限；diagnostic metadata 不进入 wire event | covered |
| Loop ownership | active Goal 临时拥有 milestone loop；settle 后可恢复 Thread 的 continuous 偏好 | public settings 覆盖 Goal loop；普通工具设置把持久偏好误写成 manual | covered: App Server RPC + Gateway tests |
| Revision / admission | `runtime_mutations_reject_stale_revision_tokens` 保留 actor/CAS 顺序 | stale mutation 在 runtime owner 变化后被拒绝，不进入 settings 或 execution | covered |

可复现命令和逐项测试名见
[`2026-09-07-control-plane-boundary-evidence.md`](../.agents/notes/implemented/testing/2026-09-07-control-plane-boundary-evidence.md)。
本轮共 44 个确定性测试通过；2026-09-08 新增 Goal loop ownership 的 App Server
与 Gateway 场景证据。Docker 不可用时按既有契约显式跳过；本次 Windows
运行实际通过了挂载和容器临时目录断言。未覆盖恶意内核、跨平台隔离、崩溃后的审计
完整性和真实 Provider 质量。

## Scenario report template

每个新增或更新的场景都应留下下面这组最小记录；测试通过本身不能替代
边界证据：

```text
Scenario: <stable name>
Hypothesis: <harness behavior being tested>
Public path: <CLI command or App Server method>
Workspace: <temporary fixture and cleanup rule>
Allowed tools / policy: <tool selection, access, approval, sandbox>
Stimulus: <input, fault injection, or race schedule>
Observed trace: <event/turn/item ordering and bounded counts>
Settled state: <turn/thread/session/goal/files>
Boundary result: <no violation, or exact violation>
Command: <copyable local command>
Evidence revision: <commit or dated report>
Known gap: <what this scenario does not prove>
```

报告应同时给出成功和失败路径的可观察结果。若场景依赖 Windows、Docker、
真实 Provider 或付费服务，必须显式标注平台/凭证条件，不能把本地 Mock
结果扩展解释成跨平台或真实 Provider 证据。

## Failure / timeout / retry 矩阵

| Fault class | 证据 | 状态 | 剩余缺口 |
| :--- | :--- | :--- | :--- |
| 缺少必要工具参数 | Core `missing_required_tool_argument_is_projected_for_model_recovery` | covered | 保持下一轮可见的 bounded Tool result |
| 未知工具恢复 | CLI `run_recovers_from_unknown_tool_on_public_path` | covered | 更广泛 provider 脏参数未穷举 |
| 模型部分流失败 | Core `partial_model_stream_is_failed_without_fabricating_completion` | covered | 真实 provider 截断仍需矩阵 |
| Partial tool batch | Core `harness::tests::recovers_after_partial_tool_batch_without_erasing_completed_action` | 同一 batch 中第一个 action 保持 `Completed`、第二个明确为 `Failed`，有序写入 history/event，模型随后恢复 | App Server/Capabilities 具体副作用 batch 仍需公共路径 scenario |
| Retryable 工具结果 | Core `retryable_tool_result_is_preserved_until_model_recovers` | covered | 无隐式重试，策略层 deferred |
| HTTP 429 | Capabilities `maps_http_429_to_bounded_api_error_without_retrying` | covered: bounded fail-fast | provider-specific retry/backoff deferred |
| shell timeout | Capabilities `shell_process_has_a_timeout` | capability boundary covered | CLI/App Server 公共路径未独立覆盖 |
| turn/Goal timeout | App Server/CLI Goal timeout scenario | public path covered | 与 tool timeout 的组合矩阵未覆盖 |
| approval/MCP refusal | App Server `NeedsApproval`、Capabilities MCP denial | covered | 沿用既有 event projection |
| MCP call timeout | Capabilities controlled slow call、App Server public projection | boundary covered | CLI actual MCP transport deferred |
| MCP circuit breaker | Capabilities `circuit_breaker_trips_after_failures_and_recovers` | unit-only | 真实失败到 model round 的公共路径未覆盖 |
| Docker sandbox | availability、mount、ephemeral filesystem probe | host runtime covered | 更强隔离仍需 policy 和跨平台证据 |
| Goal-owned continuation | `json_rpc::tests::rejects_thread_continuation_updates_while_goal_runtime_is_active`、Gateway preference/settlement tests | active Goal 不接受 Thread continuation 改写；settlement 后恢复显式偏好 | covered |
| Continuation persistence | Capabilities `session::tests::continuation_preference_survives_session_resume`；App Server `json_rpc::tests::persists_and_restores_thread_continuation_through_app_server_restart`、`json_rpc::tests::broadcasts_thread_settings_updates_with_action_revision`、`json_rpc::tests::enforces_goal_timeout_with_cooperative_cancellation`；Gateway `test_session_catalog_reads_bounded_history_without_web_state`、`test_attach_thread_honors_explicit_project_canonical_session`、`test_attach_thread_rejects_project_switch_for_live_binding` | `thread_settings.json` 按 Thread ID 原子保存；显式 idle shutdown 释放旧 worker lock；active turn 返回 `Busy`，settlement 后可 shutdown；两个 App Server subscriber 收到同一 revision；explicit Project attach 选择同一 canonical Session，live 同 ID 冲突返回 `409`；新 bind、SessionStore resume 与 Gateway read-only projection 均返回同一 bounded mode | covered; Gateway fork/并发组合与 SDK/Gateway/Web 跨进程 revision 收敛 remain |
| Trusted admission | Capabilities `workspace::tests::trusted_policy_directly_admits_non_destructive_patches_but_not_deletes` | trusted 不能升级为 allow-all：删除和高风险 Shell 仍进入 approval | covered |

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
