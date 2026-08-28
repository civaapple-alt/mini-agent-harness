# Interactive Steer and Follow-up Run Control

Status: implemented
Date: 2026-08-28
Commit: `5cca173`

## Context

The interactive REPL needs two different ways to submit a message while work
is running. A normal message should remain a follow-up and wait in the input
queue. A correction for a turn that has already drifted must reach the worker
without waiting for all queued work to finish.

Hard cancellation is unsafe because a tool may already have performed part of
an external side effect. The control path therefore needs a cooperative stop
boundary and a durable settled checkpoint.

## Decision

The core harness exposes `RunControl` and `Harness::run_with_control`.

- The REPL keeps ordinary input as `WorkerCommand::Prompt`, preserving FIFO
  follow-up behavior.
- `/steer <message>` places the message in a bounded shared steering queue and
  raises the current `RunControl` signal when a turn is active.
- The harness checks the signal before a model step, after a model response is
  appended, and after the complete tool batch finishes.
- A requested stop returns `StopReason::Steered`. The CLI records the whole
  settled prefix in the session checkpoint with `status: "steered"`, then runs
  the queued steering message immediately.
- If steering interrupts Goal Mode, the active Goal is paused and the new
  message runs as a regular turn. This avoids silently continuing an objective
  that the user has just corrected.

The boundary is cooperative: it does not terminate a tool halfway through.
An in-flight provider response is allowed to reach the next safe observation
point; steering is not implemented as an unsafe future drop or process kill.

## User contract

```text
ordinary message       follow-up; waits in the FIFO queue
/steer <message>       request immediate safe checkpoint, then run message
```

When no turn is active, `/steer` is treated as a prompt to execute rather than
as a cancellation request. The steering queue is bounded by the same
`MAX_QUEUED_INPUTS` ceiling as normal REPL input.

## Verification

- `cargo test -p mini-agent-core`: 29 tests passed, including the complete
  tool-batch steering boundary.
- `cargo test -p mini-agent-cli`: 174 unit tests and 28 interactive tests
  passed.
- The interactive integration test confirms that `/steer` arrives while the
  first provider request is held, the first turn is persisted as `steered`,
  and the correction is sent in the next provider request.
- `cargo clippy -p mini-agent-core -p mini-agent-cli --all-targets -- -D warnings`
  passed.
- `cargo fmt --all`, `git diff --check`, and the line budget check passed.

## Consequences and limits

- Users can correct a running REPL turn without losing the settled history or
  killing the process.
- Ordinary follow-up queue semantics remain unchanged.
- This control path applies to interactive REPL execution. `mini-agent ask`
  remains a one-shot command and does not expose `/steer`.
- Provider cancellation during a request is intentionally not claimed; the
  guarantee is a safe checkpoint after the current response/tool boundary.
