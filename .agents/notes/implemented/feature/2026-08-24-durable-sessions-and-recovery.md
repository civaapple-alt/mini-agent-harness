# Append-Only Durable Sessions and Checkpoint Recovery

Status: implemented

## Context

Interactive coding agent sessions require crash resilience and resumption without storing ambiguous or incomplete state. Naive database storage or in-place file mutations risk data corruption during process aborts (`kill -9`, power failure).

## Decision

The CLI provides opt-in session persistence via `--persist`, `sessions`, and `resume SESSION_ID`:

1. **Storage Layout & Concurrency**:
   - Sessions are stored under `~/.mini-agent/sessions/<workspace>/<session-id>/session.jsonl`.
   - File-based mutex locking (`SessionLock`) prevents concurrent writes to the same session directory.
2. **Append-Only Event Records**:
   - All session headers, threads, turns, checkpoints, and derived mentor items are written as distinct JSONL records with strict sequence numbers (`seq`).
3. **Settled Checkpoint Rule**:
   - Checkpoints are committed only after a turn fully settles (`TurnStatus::Completed` or `TurnStatus::StepLimit`).
   - Resumption always restores from the latest valid checkpoint record.
4. **Torn-Tail Auto-Recovery**:
   - If a crash occurs during a disk write, the store detects unparseable or incomplete trailing bytes on next open and truncates back to the last clean record boundary.

## Consequences

- In-flight interrupted tool executions or provider calls are never replayed as partial turns.
- Crash recovery is robust, deterministic, and self-healing.
- Sessions can be audited and inspected independently without executing runtime code.
