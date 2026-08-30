# Runtime Control-Plane Ownership

Status: implemented

## Decision

The local CLI and JSON-RPC transport share one App Server control plane.
`LocalAppServerClient` owns the local turn batch, checkpoint, and persistence
operations used by the CLI; `AppServerRuntime` remains only the host-backed
composition object and exposes the client for frontend calls.

An `AppServerConnection` receives one `RuntimeServices` value containing the
management and workflow services for the same runtime identity. It can no
longer attach those services independently. The workflow implementation
module is private to App Server; local frontend parsing and prompt helpers are
exposed through `frontend::workflow` instead of the Host workflow module.

`RuntimeManagementService` is the owner of mutable management state. The
active thread ID is derived from the durable Session when one exists, or from
the App Server's initial thread otherwise. It is not copied into the local
client or a second management-state field. MCP status is grouped under one
runtime sub-state, while the shared approval controller is held at the
service boundary.

The durable Session remains the source of settled history, and Core remains
the source of live Thread checkpoints. The management service stores only the
bounded control-plane state required to coordinate those authorities.

## Consequences

- CLI, local clients, and JSON-RPC use the same App Server turn and management
  path.
- A runtime's workflow and management state are bound as one service unit.
- Host persistence and workflow state-machine code remain below the App Server
  boundary without leaking the Host workflow module through the public module
  tree.
- The remaining cached world/MCP state is explicit and bounded; it is not
  presented as a replacement for Core or Session state.

## Verification

```text
cargo fmt --all PASS
cargo test -p mini-agent-app-server PASS
cargo test -p mini-agent-cli PASS (14 unit + 11 integration tests)
cargo clippy -p mini-agent-app-server -p mini-agent-cli --all-targets -- -D warnings PASS
```
