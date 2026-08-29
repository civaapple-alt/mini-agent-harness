# Core Protocol Boundary and Harness Module Ownership

Status: implemented

## Decision

`mini-agent-core` no longer re-exports the wire and provider contracts from
`mini-agent-protocol`. Consumers import protocol types from
`mini-agent-protocol` directly. The Core root exports only execution-owned
types such as `Harness`, `Thread`, `SessionState`, `RunControl`, and
`ToolRegistry`.

The Harness execution loop remains the stable public entry point, while its
high-churn responsibilities are kept in private modules:

| Module | Responsibility |
| --- | --- |
| `run_control.rs` | Cooperative cancel/steer flags and queued input semantics |
| `context_controller.rs` | Context compaction partitioning, trimming, and bounded summary assembly |
| `tool_batch_executor.rs` | Complete tool-batch execution, bounded output, events, and session recording |
| `turn_engine.rs` | Model response sizing and model event forwarding |
| `harness.rs` | Public Harness state, turn orchestration, lifecycle events, and error mapping |

These modules are private implementation details. The existing Core public
API and turn semantics are unchanged; protocol ownership is now explicit at
crate boundaries without introducing a second wire model.

## Consequences

- Core no longer acts as a convenience re-export barrel for Protocol.
- Host, Capabilities, App Server, ACP, and experiments declare their direct
  Protocol dependency where they consume wire contracts.
- Context, tool-batch, model-event, and run-control changes can evolve without
  adding more unrelated code to `harness.rs`.
- `harness.rs` is approximately 696 lines instead of 994; the module split is
  structural, not a behavior rewrite.

## Verification

The following checks passed on 2026-08-29:

```text
cargo check -p mini-agent-core -p mini-agent-capabilities -p mini-agent-host \
  -p mini-agent-app-server -p mini-agent-acp -p mini-agent-experiments \
  --all-targets --locked
cargo test -p mini-agent-core --all-targets --locked -- --test-threads=1
cargo test -p mini-agent-host --all-targets --locked -- --test-threads=1
cargo test -p mini-agent-app-server --all-targets --locked -- --test-threads=1
cargo test -p mini-agent-acp --all-targets --locked -- --test-threads=1
cargo clippy -p mini-agent-core --all-targets --locked -- -D warnings
```

The workspace-wide line-budget script still reports the pre-existing total
workspace budget overage; the runtime layer budget remains a separate gate.
