# Multi-Branch Tree Conversations and Speculative Lanes

Status: proposed

## Context

Current durable sessions are linear append-only sequences of turns per thread. In complex debugging or architectural tasks, users or agents often want to explore alternative hypotheses from a prior checkpoint without losing the main line of exploration.

## Proposal

Extend session storage to support explicit tree-structured branching:
1. Allow `mini-agent fork <CHECKPOINT_ID>` to initialize a new lane referencing a parent checkpoint fingerprint.
2. Store branches as discrete thread IDs within the same workspace session file.
3. Keep the core run loop unchanged by feeding only the linear path from root to active branch tip into [`Harness::restore_history`](crates/mini-agent-core/src/harness.rs).

## Acceptance Criteria

- Ability to switch and resume different branches cleanly from CLI.
- No leakage of speculative branch messages into the main thread.
- Entire implementation stays strictly inside `mini-agent-cli` to preserve core simplicity.

## Risks

- Tree reconciliation UI and branch management cognitive load.
- File size growth in long branching sessions.
