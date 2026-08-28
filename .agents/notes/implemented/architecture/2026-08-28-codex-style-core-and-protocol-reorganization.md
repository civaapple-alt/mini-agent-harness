# Codex-Style Core and Protocol Reorganization

Status: implemented

## Decision

The harness uses a core execution kernel, a transport-neutral protocol crate,
optional control-plane adapters, and host adapters:

```text
future wire transport
        |
mini-agent-app-server (optional control plane)
        |                         \
mini-agent-core (execution) ----> mini-agent-protocol (contracts)
        ^
        |
host adapters (providers, tools, storage, policy, UI)
```

`mini-agent-core` owns the semantic runtime state and execution loop. It is
usable directly by a host or through `mini-agent-app-server`. The protocol
crate owns the shared model, tool, message, turn, event, stop, limit, and
structured outcome contracts. Neither crate depends on the CLI.

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

Legacy payload-only event and session records remain readable because the new
status fields are optional. The protocol does not contain JSON-RPC method
names, provider clients, storage formats, or policy implementations.

### Control-plane adapter

`mini-agent-app-server` is a thin in-process adapter around one core `Thread`.
It serializes start/cancel commands, routes running `Steer` and `FollowUp`
inputs through `RunControl`, and broadcasts `EventEnvelope` values. It owns no
second model loop, context builder, tool router, or security policy. A future
wire transport can map this surface to JSON-RPC or ACP without changing core.

### Host policy boundary

The CLI assembles provider and concrete tool implementations, approval and
Plan Mode policy, sandbox/process containment, MCP lifecycle, session JSONL,
trace persistence, and terminal rendering. A host-only outcome adapter maps
legacy policy errors into structured statuses while preserving existing user
visible content and fail-closed behavior.

## Consequences

- CLI session storage remains an adapter over core values rather than a second
  conversation state machine.
- Live UI, trace persistence, replay, and future notifications consume one
  ordered core event stream.
- Tool routing and tool policy can evolve independently; approval and sandbox
  decisions cannot silently move into the kernel.
- `ToolRegistry` remains as a compatibility alias for `ToolRouter`.
- The CLI outcome classifier is transitional: host tools should eventually
  return typed policy errors directly instead of relying on legacy error text.
- A versioned external transport is intentionally not part of this decision.

## Verification

The current workspace verifies the boundary with:

- `cargo test --workspace --quiet`;
- `cargo clippy --workspace --all-targets --quiet -- -D warnings`;
- `git diff --check`;
- core tests for context/session limits, success/failure, steer/follow-up,
  cancellation, structured tool status, checkpoints, and turn identity;
- protocol tests for typed control DTOs, legacy event/session decoding, and
  structured outcome serialization;
- app-server tests for lifecycle events, queued/steered/cancelled turns, and
  starting from a restored checkpoint without replaying the first turn;
- CLI tests for approval/sandbox behavior, session restart, trace replay,
  provider projection, and host outcome classification.

The architecture note is implemented. Remaining work is additive: replace the
CLI compatibility classifier with typed policy errors in individual host tools,
then design a deliberately versioned JSON-RPC/ACP transport.
