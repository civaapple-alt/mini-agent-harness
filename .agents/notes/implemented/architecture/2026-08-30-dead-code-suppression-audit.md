# Dead-Code Suppression Audit

Status: implemented

## Decision

The remaining `#[allow(dead_code)]` attributes were audited by call site and
removed. None represented an intentionally unused compatibility surface.

| Location | Classification | Result |
|---|---|---|
| `mini-agent-cli/src/args.rs:Invocation::session_id` | 主线需要 | Used by session resume and auto mode dispatch |
| `mini-agent-host/src/goal.rs` goal/plan functions | 主线需要 | Used by Goal Mode, Plan Mode, and App Server workflow calls |
| `mini-agent-host/src/harness_builder.rs:harness_config` | 主线需要 | Used by the App Server local runtime path |
| `mini-agent-capabilities/src/workspace/shell.rs:CommandOutput` | 主线内部需要 | The shell path consumes `text/source_*`; tests assert the formatted text |
| `mini-agent-capabilities/src/workspace/shell.rs` unused output fields | 确实遗留 | Removed because formatted text already carries the observable result |
| `mini-agent-capabilities/src/workspace/approval.rs` constructors | 外部嵌入和测试需要 | Used by the external provider example and capability tests |
| `mini-agent-host/src/env_file.rs:ValueSource::UserEnv` | 主线需要 | Used by user-level configuration resolution |

There are no retained `dead_code` suppressions in the current Rust sources.
If a future embedding-only API is intentionally kept unused by this workspace,
it should be documented at its public boundary rather than silenced globally.

## Verification

The workspace check and strict Clippy build are the acceptance checks for this
audit; a new unused item now fails compilation instead of being hidden by an
attribute.
