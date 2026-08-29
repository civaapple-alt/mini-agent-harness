# App Server Session, World, and MCP Management

Status: implemented

## Decision

The App Server is now the management boundary for runtime state needed by a
frontend. Versioned protocol methods expose session metadata, workspace state,
execution policy, and MCP status/retry:

```text
session/info
world/state
world/refresh
world/set_execution
mcp/status
mcp/retry
```

`RuntimeManagementService` owns the mutable management snapshot and is
attached to JSON-RPC connections and `LocalAppServerClient`. Both transports
therefore share the same implementation for world and MCP transitions.

`AppServerRuntime` delegates session metadata, world refresh, execution policy,
MCP retry, and persistence updates to that service. REPL status, `/world`,
`/session`, `/mcp`, session checks, and JSON session output use the local
client protocol. The CLI no longer imports the capability session module or
Host `WorldState` for these operations. Session selection uses the App Server's
`SessionRequest` type; capability conversion happens inside the runtime.

The standalone `mini-agent-app-server` binary attaches the management service
during startup and advertises `capabilities.runtimeManagement` when available.

## Consequences

- Session/world/MCP management has one service implementation and two thin
  transports (local and JSON-RPC).
- Workspace paths and provider-backed state remain server-side; protocol
  results contain bounded display metadata and snapshots.
- CLI still owns frontend-only safety flags, approval prompting, profile
  selection, and rendering.
- ACP remains a side adapter and can map these methods later without changing
  Core execution contracts.

## Verification

```text
cargo check -p mini-agent-app-server -p mini-agent-cli --all-targets --locked PASS
cargo test -p mini-agent-app-server -p mini-agent-app-server-protocol -p mini-agent-cli --all-targets --locked PASS (24 + 4 + 54 tests)
cargo test -p mini-agent-acp --all-targets --locked PASS (5 tests)
cargo clippy -p mini-agent-app-server -p mini-agent-app-server-protocol -p mini-agent-cli --all-targets --locked -- -D warnings PASS
python scripts/line_budget.py runtime gate PASS (15,857/20,000); overall command reports the workspace gate at 38,782/30,000
```

These are local Windows results; macOS/Linux CI and a real-provider run remain
separate evidence.
