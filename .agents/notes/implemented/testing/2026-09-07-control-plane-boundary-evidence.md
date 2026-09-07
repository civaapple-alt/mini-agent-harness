# Control Plane Boundary Evidence

* **日期**: 2026-09-07
* **状态**: implemented
* **Class**: testing
* **范围**: Permission、Sandbox、Recovery、Audit 的确定性边界证据

## 结论

line gate 落地后的第一轮证据复核复用了现有 Capabilities 和 App Server 公共边界，
没有新增 benchmark framework、付费 Provider 或运行时兼容层。成功路径和失败反例都
保持在已有 hard limit 内；结果已同步到 [`docs/harness-evidence.md`](../../../../docs/harness-evidence.md)。

## Evidence matrix

| 边界 | 成功证据 | 失败反例 | 结果 |
| --- | --- | --- | --- |
| Permission / grant | `security::tests::scoped_approvals_require_an_exact_owner_and_workspace_revision`、`workspace::tests::read_image_accepts_absolute_path_outside_workspace_after_approval` | `security::tests::matches_human_formatted_actions`、`workspace::tests::read_image_outside_workspace_can_be_denied`、`workspace::tests::shell_denial_is_explicit_before_sandbox_execution`、`tests::projects_structured_approval_denial_through_public_app_server` | covered |
| Sandbox | `sandbox::tests::creates_and_attaches_sandbox_guard`、`workspace::tests::docker_sandbox_mounts_workspace_and_keeps_container_tmp_ephemeral` | `workspace::tests::shell_process_has_a_timeout`、`workspace::tests::read_only_shell_subset_rejects_side_effect_flags` | covered with platform/daemon caveat |
| Recovery | `goal_runtime::tests::goal_runtime_supports_pause_and_resume`、`tests::exposes_a_restored_core_checkpoint_without_replaying_the_first_turn` | `goal_runtime::tests::ignores_verifier_result_after_goal_clear`、`goal_runtime::tests::ignores_verifier_result_for_changed_checkpoint`、`goal_runtime::tests::handles_rejected_and_failed_verifier_results` | covered |
| Audit | `trace::tests::trace_redacts_payloads_and_carries_round_metadata`、`trace::tests::trace_records_bounded_output_without_copying_tool_arguments` | `trace::tests::trace_rejects_unbounded_trace_ids`、`trace::tests::trace_refuses_to_exceed_total_artifact_limit`、`trace::tests::diagnostic_metadata_stays_out_of_the_wire_event` | covered |

## Verification

在 Windows 工作区执行：

```text
cargo test -p mini-agent-capabilities --lib security::tests
cargo test -p mini-agent-capabilities --lib sandbox::tests
cargo test -p mini-agent-capabilities --lib workspace::tests::
cargo test -p mini-agent-app-server --lib trace::tests
cargo test -p mini-agent-app-server --lib goal_runtime::tests
cargo test -p mini-agent-app-server --lib projects_structured_approval_denial_through_public_app_server
cargo test -p mini-agent-app-server --lib exposes_a_restored_core_checkpoint_without_replaying_the_first_turn
```

结果：`5 + 2 + 24 + 5 + 6 + 1 + 1 = 44` 个测试通过。Docker 场景在 Docker 不可用时
按测试契约显式跳过；本次本机运行通过了实际挂载和容器临时目录断言。

## 未证明事项

- 这些确定性测试证明边界行为和失败分类，不等价于完整的安全证明；恶意内核、容器
  daemon 配置和跨平台隔离仍需独立环境证据。
- Audit 覆盖 bounded、redacted trace 和 wire separation；跨进程崩溃后的审计完整性
  仍未纳入本批。
- 没有调用真实或付费 Provider；模型质量、跨 Provider 一致性和生产延迟不在本批结论内。
