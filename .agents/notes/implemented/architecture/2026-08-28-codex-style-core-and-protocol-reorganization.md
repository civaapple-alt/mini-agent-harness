# Codex-Style Core and Protocol Reorganization

Status: implemented

## Decision

The harness uses four conceptual layers while keeping the crate graph shallow:

```text
CLI client
    ↓
mini-agent-app-server (service boundary and wire dispatch)
    ↓
mini-agent-host + workflows (application host)
    ↓
mini-agent-core + mini-agent-protocol (execution foundation)
```

`mini-agent-core` owns the semantic runtime state and execution loop. It is
usable directly by a host or through `mini-agent-app-server`. The protocol
crate owns the shared model, tool, message, turn, event, stop, limit, and
structured outcome contracts. `mini-agent-host` owns provider and product
composition. Neither foundation crate depends on the CLI.

### Core ownership

Core owns:

- `Context` and storage-neutral `SessionState`;
- `ThreadId`, `TurnId`, `ThreadStatus`, `TurnStatus`, and turn sequencing;
- start, steer, follow-up, cancellation, safe checkpoints, and event ordering;
- model/tool step limits, compaction, stop reasons, and failure projection;
- `ToolRouter`, structured tool outcomes, and `ThreadCheckpoint`.

`Harness` remains a compatibility facade over the kernel. `ThreadCheckpoint`
contains only settled values and restores through Harness validation. It never
replays an uncertain external effect.

Core does not open files, manage credentials, render a TTY, approve actions, or
implement a sandbox. Those responsibilities remain host adapters.

### Protocol ownership

`mini-agent-protocol` is transport-neutral and defines:

- typed `ThreadStart`, `TurnStart`, `TurnInput`, and `TurnCancel` payloads;
- `TurnSubmission` with typed `TurnId` results;
- `EventEnvelope` and `EventSink` for ordered live and durable projections;
- `ToolExecutionRequest`, `ToolExecutionOutcome`, and
  `ToolExecutionStatus` (`completed`, `failed`, `needs_approval`, `deferred`,
  and `retryable`);
- optional outcome status on `ToolFinished` and `Message::Tool`.

Provider-facing tool messages and live events keep outcome status optional at
the protocol boundary because those payloads are projected into different
wire shapes. Persisted session checkpoints accept only the current record
shape; the session loader does not migrate payload-only historical records.
The protocol does not contain JSON-RPC method names, provider clients, storage
formats, or policy implementations.

### Service boundary

`mini-agent-app-server` is an in-process service around one core `Thread`. It
serializes commands, routes running `Steer` and `FollowUp` inputs through
`RunControl`, and broadcasts `EventEnvelope` values. The separate
`mini-agent-app-server-protocol` crate defines versioned JSON-RPC envelopes,
initialization, thread/turn methods, and event notifications. `serve_stdio`
provides newline-delimited JSON framing for subprocess clients. The service
owns no second model loop, context builder, tool router, or security policy.

### Host policy boundary

`mini-agent-host` assembles provider and concrete tool implementations,
approval and Plan Mode policy, sandbox/process containment, MCP lifecycle,
session JSONL, result handles, and world context through `RuntimeBuilder`.
The CLI owns only frontend input, output, and command routing. A host-only
outcome adapter maps legacy policy errors into structured statuses while
preserving existing user visible content and fail-closed behavior.

## Consequences

- CLI session storage remains an adapter over core values rather than a second
  conversation state machine.
- Live UI and future notifications consume one ordered core event stream; settled
  history and result handles use the session JSONL record.
- Tool routing and tool policy can evolve independently; approval and sandbox
  decisions cannot silently move into the kernel.
- `ToolRegistry` remains as a compatibility alias for `ToolRouter`.
- The CLI outcome classifier is transitional: host tools should eventually
  return typed policy errors directly instead of relying on legacy error text.
- The app-server wire surface is versioned separately from the kernel; ACP
  remains an adapter concern and is not a dependency of core.

## Verification

The current workspace verifies the boundary with:

- `cargo test --workspace --quiet`;
- `cargo clippy --workspace --all-targets --quiet -- -D warnings`;
- `git diff --check`;
- core tests for context/session limits, success/failure, steer/follow-up,
  cancellation, structured tool status, checkpoints, and turn identity;
- protocol tests for typed control DTOs, event projection, and structured
  outcome serialization;
- app-server tests for lifecycle events, queued/steered/cancelled turns,
  restored checkpoints, JSON-RPC initialization, and event projection;
- app-server-protocol tests for JSON-RPC framing DTOs, camelCase payloads, and
  event identity/sequence preservation;
- CLI tests for approval/sandbox behavior and session restart,
  provider projection, and host outcome classification.

The architecture note is implemented. Remaining additive work is multi-thread
service management, approval request messages, and an ACP mapping adapter after
the app-server semantics stabilize.
