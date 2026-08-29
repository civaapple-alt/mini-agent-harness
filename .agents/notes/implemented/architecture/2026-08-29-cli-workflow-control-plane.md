# CLI Workflow Control Plane Through App Server

Status: implemented

## Decision

The interactive CLI now uses `LocalAppServerClient` for the complete workflow
control path. Goal start, pause, fail, criteria lookup, verifier evidence
recording, milestone advance, plan mode changes, and restart pause handling
all cross the same App Server management contract used by JSON-RPC clients.

The CLI still owns presentation and local approval policy. It may derive the
session-relative `plan.md` and `goal/` paths needed by that policy, but it no
longer invokes `WorkflowService` or Host workflow persistence directly.

The verifier evidence operation is available as
`workflow/goal/record_verdict` and carries only a checkpoint sequence and
bounded verifier output. The wire response does not expose Host paths.

## Consequences

- CLI and future ACP clients share identical workflow state transitions.
- Workflow errors cross the local client boundary as `JsonRpcError` values.
- The App Server remains the single workflow control plane while CLI-specific
  approval and status rendering stay at the frontend edge.
- `WorkflowService` remains an App Server implementation detail.

## Verification

```text
cargo check -p mini-agent-app-server -p mini-agent-cli --all-targets --locked PASS
cargo test -p mini-agent-app-server --all-targets --locked -- --test-threads=1 PASS (25 tests)
cargo test -p mini-agent-cli --all-targets --locked -- --test-threads=1
  unit tests PASS; interactive suite reached the deterministic workflow tests
  before the long-running timeout scenario exceeded the local command window
rg workflow_service crates/mini-agent-cli/src/repl.rs
  no matches
```

The CLI still intentionally imports approval, sandbox, security, skills, and
observer contracts from the App Server frontend facade because those are
startup and interaction presentation concerns, not workflow management calls.
