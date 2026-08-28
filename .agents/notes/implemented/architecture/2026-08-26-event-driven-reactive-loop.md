# Event-Driven Reactive Loop and Passive Observers

Status: implemented

## Context

Agent turns need one event-driven loop for live client progress and durable restart evidence without making observers part of execution control.

## Decision

The harness uses a passive `Observer::observe(&Event)` contract. Observers cannot rewrite model output, tool arguments, or control flow.

The runtime has two related outputs:

- **Live event stream**: reasoning, assistant text, tool status, approvals, turn lifecycle, and failures are rendered by the CLI or broadcast by App Server.
- **Durable session log**: settled turns, context checkpoints, derived mentor items, and stored result handles append to the session's `session.jsonl`. This is the single durable source of truth and is reloaded on resume.

External trace files and trace replay are not part of the mainline runtime. Detailed trace replay was prompt-weight specific and is retained only as a rejected historical experiment.

Tool results reactively trigger the next model step until tool calls are empty or a hard limit is reached. `/steer` settles at a model-step or complete-tool-batch checkpoint with `StopReason::Steered`; queued follow-up input starts only after settlement. Cancellation follows the same cooperative boundary.

## Consequences

- Terminal presentation and App Server transport stay outside the execution kernel.
- Restart evidence is deterministic and append-only.
- Session result handles survive process restart without maintaining a second result or trace database.

## Event and state closure

The kernel emits `Event` values. `Thread` adds `thread_id`, `turn_id`, and monotonic `sequence` through `EventEnvelope`. CLI observers use this stream for terminal rendering; App Server broadcasts it to subscribers.

Successful stop reasons are `Completed`, `StepLimit`, `Steered`, and `Cancelled`. Model, compaction, limit, and Thread errors emit failure events and settle the Thread as `Failed`.

`SessionState` and `Context` are owned by core and remain storage-neutral. A `ThreadCheckpoint` contains only settled core values and can be restored after a restart; it never replays an uncertain external tool effect. Session files, approval state, sandbox/process state, and terminal UI remain host or edge responsibilities.

## Source references

- `mini-agent-core/src/harness.rs`: model/tool loop, limits, compaction, and stop reasons.
- `mini-agent-core/src/thread.rs`: Thread lifecycle, turn identity, envelopes, and checkpoints.
- `mini-agent-core/src/input.rs`: bounded steer/follow-up queue.
- `mini-agent-protocol/src/event.rs`: event and envelope contracts.
- `mini-agent-protocol/src/turn.rs`: turn input, status, and submission contracts.
- `mini-agent-app-server/src/lib.rs`: serialized control-plane worker and event broadcast.
- `mini-agent-host/src/session.rs`: append-only session and checkpoint storage.
- `mini-agent-host/src/result_store.rs`: session-backed result handles.
- `mini-agent-cli/src/repl.rs`: interactive worker, input routing, and settled persistence.
