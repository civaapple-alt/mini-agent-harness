# Automatic Replay of Interrupted Unsettled Turns

Status: rejected — Replaying interrupted effects produces duplicate non-idempotent side-effects

## Context

When resuming an interrupted or crashed session, a common proposal is to re-execute the interrupted turn from the exact point of failure.

## Rejected Proposal

Automatically replay or resume execution of a turn that was interrupted mid-stream during an active provider request or tool execution.

## Rationale for Rejection

1. **Duplicate Side Effects**: If a shell command or file modification succeeded right before the process was killed, replaying that turn blindly executes the command a second time, risking data loss, duplicate billing, or corrupted repository state.
2. **Determinism**: The only clean, predictable resumption state is the latest fully settled checkpoint (`TurnCommit`). Interrupted turns are discarded from replay history.
