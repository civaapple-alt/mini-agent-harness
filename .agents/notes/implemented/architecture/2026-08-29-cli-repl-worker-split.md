# CLI REPL Worker Split

Status: implemented

## Decision

The interactive frontend is split into two focused modules:

- `repl.rs` owns terminal input, command parsing, event rendering, approval
  presentation, and work queueing.
- `repl_worker.rs` owns the App Server worker thread, turn execution, workflow
  transitions, verifier handling, persistence calls, and worker-side errors.

Both modules share the same bounded `ReplEvent` and `WorkerCommand` channel
contract. No second Harness or workflow state machine was introduced.

## Consequences

The main REPL module is now 527 lines and the worker module is 926 lines. The
large execution loop is isolated from terminal presentation, making later
workflow or provider changes reviewable without growing the input/rendering
surface.

## Verification

```text
cargo fmt --all PASS
cargo check -p mini-agent-cli --all-targets --locked PASS
```
