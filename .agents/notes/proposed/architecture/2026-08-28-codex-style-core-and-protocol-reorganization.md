# Codex-Style Core and Protocol Reorganization

Status: proposed

## Context

The current split establishes a useful first boundary:

```text
mini-agent-protocol  -> portable in-process contracts
mini-agent-core      -> Harness execution loop
mini-agent-cli       -> provider, tools, sessions, UI, policy, and storage
```

This is smaller and easier to test than the Codex workspace, but it leaves
several concepts that belong to the agent runtime distributed across the CLI.
In particular, Session state, active Turn state, pending input, context
preparation, and event delivery are not represented by one core-owned service.
The CLI therefore performs part of the work that Codex places behind
`CodexThread` and `session::run_turn`.

The goal is not to copy Codex's app-server wholesale. Codex's app-server is a
control-plane adapter that exposes harness capabilities to another process.
The immediate goal is to make the local execution core stable enough that an
app-server-like adapter can be added later without moving execution semantics
back into the CLI.

## Proposal

Reorganize in four layers, delivered in order:

```text
future wire adapter (JSON-RPC / ACP / another host)
                    |
mini-agent-protocol | portable request, state, and event contracts
                    |
mini-agent-core     | Thread/Session/Turn + Context + agent loop
                    |
host adapters       | provider, concrete tools, approval, sandbox, storage, UI
```

The first two implementation stages stabilize `mini-agent-core`. Protocol
changes should describe the core state machine, not prematurely become a
network protocol. A later adapter may serialize the same contracts or define a
versioned wire DTO layer without coupling the execution kernel to JSON-RPC.

### 1. Core owns runtime state, not external storage

Move the semantic state needed to execute and resume a conversation into core:

- `ThreadId`, `TurnId`, and explicit thread/turn status;
- ordered conversation items and bounded context preparation;
- context compaction decisions and resulting context revisions;
- active Turn input mode (`Start`, `StartIfIdle`, `Steer`, `FollowUp`);
- pending input queue and the safe boundaries at which it is consumed;
- model step accounting, stop reasons, cancellation, and failure state;
- tool invocation records and their settled results;
- event ordering and the event sequence number for one running Thread.

Keep these host concerns outside core:

- JSONL layout, files, attachments, and session directories;
- provider HTTP clients and credentials;
- filesystem, process, web, and MCP implementations;
- TTY prompts, approval UI, and terminal rendering;
- sandbox implementation and operating-system process containment.

Core may expose a narrow checkpoint/restore representation, but it must not
open files or replay uncertain external effects. The CLI remains responsible
for durable storage and converts stored records into core state.

### 2. Core modules

Add private modules behind a small public API. The expected shape is:

```text
mini-agent-core/src/
  thread.rs       Thread runtime and lifecycle
  session.rs      conversation state and checkpoint projection
  context.rs      bounded model context and compaction
  turn.rs         start/steer/follow-up input and run loop
  tool_router.rs  model ToolCall validation and dispatch selection
  tool_result.rs  settled tool result and retry classification
  event_bus.rs    ordered observer fan-out
  harness.rs      compatibility facade during migration
```

The exact file split is not mandatory. The ownership rules are mandatory:

- `Thread` owns a long-lived runtime and its current Turn.
- `Session` owns ordered conversation state, but not storage I/O.
- `Context` is the only component that decides what is model-visible and how
  hard limits or compaction are applied.
- `Turn` owns the event-driven loop and input queue.
- `ToolRouter` resolves a model call to a registered capability.
- A host-facing tool executor reports approval, sandbox, retry, and settled
  result outcomes without putting those implementations in core.
- `Harness::run` remains as a compatibility convenience that creates or uses
  one Thread/Turn; new code should use the explicit runtime API.

### 3. Protocol improvements

Extend `mini-agent-protocol` only with contracts required by the runtime:

```rust
ThreadId(String)
TurnId(String)
TurnInput { mode, text }
TurnMode { Start, StartIfIdle, Steer, FollowUp }
ThreadStatus { Idle, Running, AwaitingInput, Failed, Closed }
TurnStatus { InProgress, Completed, Steered, Cancelled, Failed }
```

Add typed contracts for:

- `ThreadStart`, `TurnStart`, `TurnInput`, and `TurnCancel` operations;
- a `TurnSubmission` result distinguishing `Started`, `Steered`, and
  `NotSubmitted`;
- event envelopes containing `thread_id`, optional `turn_id`, sequence, and
  event payload;
- tool execution outcomes such as `Completed`, `Failed`, `NeedsApproval`,
  `Deferred`, and `Retryable`;
- bounded, serializable error values and capability/version information.

The existing `Model`, `Tool`, `Message`, `Event`, and `Observer` contracts stay
source-compatible where practical. Do not add JSON-RPC method names or server
transport code to this crate yet. A future `mini-agent-app-server` or wire
protocol crate should map these runtime contracts to JSON-RPC/ACP and own
transport compatibility.

### 4. Event-driven closure

The core runtime should produce one ordered event stream and allow multiple
consumers:

```text
Thread/Turn runtime
       |
       +--> live Observer / REPL
       +--> trace and diagnostics
       +--> host checkpoint writer
       +--> future app-server notification adapter
```

The runtime must emit lifecycle events for Thread and Turn start, model steps,
model deltas, ToolCall dispatch, tool outcome, input consumption, compaction,
and Turn completion/failure. The host may persist those events or derive a
checkpoint at a settled boundary, but the event order and identity come from
core.

## Delivery plan

### Stage 0 — Freeze the current boundary

- Keep `mini-agent-protocol` free of provider, storage, and host policy code.
- Document `Harness` as the compatibility facade, not the long-term Thread API.
- Add contract tests for current stop reasons, event ordering, context limits,
  and tool failure projection.
- Keep the existing CLI behavior unchanged.

### Stage 1 — Move Context and Session semantics into core

- Extract `Context` from `Harness` without changing compaction behavior.
- Extract a storage-neutral `SessionState` containing ordered messages,
  context revision, and settled Turn metadata.
- Add explicit restore/checkpoint conversion functions that operate on values,
  not paths or files.
- Make CLI session code an adapter that loads JSONL and feeds `SessionState`.
- Preserve existing session files and migration compatibility.

Acceptance evidence:

- existing context compaction tests pass unchanged or with equivalent coverage;
- restore, fork, resume, and torn-tail recovery produce the same messages;
- core tests prove storage is not required to execute or restore state;
- CLI session tests still pass through the built binary.

### Stage 2 — Introduce Thread and Turn state

- Add a core-owned Thread with one active Turn at a time.
- Replace the boolean-only control path with typed input submission and a
  bounded pending-input queue.
- Implement safe consumption after model sampling and after a complete tool
  batch; never interrupt an in-flight external effect.
- Distinguish `Steer` from FIFO `FollowUp` in core rather than only in REPL.
- Add cancellation as a separate operation from steering.

Acceptance evidence:

- deterministic integration tests cover Start, Steer, FollowUp, cancellation,
  idle rejection, and duplicate/late Turn IDs;
- a steer submitted during model work is consumed at the next safe boundary;
- a follow-up remains queued until the active Turn settles;
- restart and resume preserve settled history and do not replay uncertain tools.

### Stage 3 — Separate routing from orchestration

- Evolve `ToolRegistry` into a model-facing `ToolRouter` with validated tool
  lookup and bounded call batches.
- Define a core-neutral `ToolExecutionRequest` and `ToolExecutionOutcome`.
- Keep approval, sandbox, network admission, and process handling in a host
  executor implementing that contract.
- Add explicit retry/deferred/approval outcomes rather than encoding all
  failures as plain tool text.

Acceptance evidence:

- unknown tools, rejected approval, retryable failure, and settled success have
  distinct events and history records;
- the CLI's current security and sandbox tests remain authoritative for policy;
- core remains deterministic with an in-memory executor.

### Stage 4 — Add an external protocol adapter

Only after Stages 1–3 are stable, add an app-server-like crate or binary:

```text
thread/start
turn/start
turn/steer
turn/cancel
thread/events
thread/read or resume
```

This adapter will translate wire requests into core operations and translate
core events into notifications. It must not implement a second agent loop,
context builder, or tool policy engine.

## Non-goals

- Full source-level parity with Codex.
- Moving JSONL persistence or uncertain effect replay into core.
- A generic dependency-injection or multi-tenant scheduler.
- Making every current CLI tool portable.
- Promising remote protocol compatibility before a versioned wire contract is
  deliberately designed.

## Risks and guardrails

| Risk | Guardrail |
| --- | --- |
| Core becomes another monolith | Keep modules under the existing line budget and require a clear owner for each state transition. |
| Storage semantics leak into core | Core accepts values and repositories only through narrow, storage-neutral conversions; no filesystem paths or JSONL writes. |
| Protocol grows before behavior stabilizes | Add a protocol type only when a core test or future adapter needs it. |
| Steer interrupts an external effect | Consume pending input only after sampling or a complete tool batch. |
| Tool policy becomes duplicated | Core reports capability outcomes; the host remains the authority for approval and sandbox decisions. |
| Compatibility breaks current CLI users | Retain `Harness` and core re-exports during at least one migration stage. |

## Exit criteria

This proposal can move to `implemented/architecture/` only when:

- Context and Session semantics are owned and tested by core;
- Thread and Turn identity, status, and input modes are explicit;
- event ordering is deterministic and supports live and durable projections;
- CLI storage and policy remain host adapters;
- the current CLI, core, and workspace test suites pass;
- the new state machine has deterministic integration coverage for success,
  failure, steer, follow-up, restart, and cancellation;
- any external protocol adapter is a thin translation layer over core rather
  than a second execution implementation.

## Implementation progress

Stage 1 has started in the current working tree:

- `mini-agent-core` now exposes storage-neutral `Context` and `SessionState`;
- `Harness` owns `SessionState` instead of a raw message vector;
- `Harness::restore_session` accepts the core state value and validates it
  against the active model/tool limits;
- CLI `SessionStore` still owns JSONL, locks, paths, and torn-tail recovery,
  but hands a `SessionState` value to the harness;
- `mini-agent-protocol` now names `TurnInputMode` and `TurnInput`, and core
  `RunControl` owns a bounded pending input queue with Steer priority;
- interactive follow-up text submitted while a Turn is running now enters the
  same core queue and is dispatched only after the current command settles;
- existing core and CLI session, compaction, restart, and interactive tests
  pass after the migration.

The proposal remains `proposed`: Thread/Turn identity, active Turn status,
safe input consumption inside the run loop, and a protocol adapter are not
implemented yet. The current queue is the first migration seam for those
future semantics; it does not claim Codex-equivalent same-Turn steering.
