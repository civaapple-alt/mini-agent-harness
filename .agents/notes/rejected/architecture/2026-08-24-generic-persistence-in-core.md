# Generic Persistence and Transaction Journaling in Core

Status: rejected — Core should not own persistence mechanisms or generic effect journals

## Context

Proposals suggested embedding an undo/redo journal, effect intent transaction log, or generic storage adapter directly inside `mini-agent-core` to manage session state and crash recovery.

## Rejected Proposal

Add persistent database traits, transaction logs, and execution checkpointing inside the microkernel core loop.

## Rationale for Rejection

1. **Inability to Settle Non-Idempotent Effects**: Crash-boundary simulations ([Effect Recovery Boundary](../../archived/experiments/2026-08-24-effect-recovery.md)) demonstrated that while intent logs detect uncertainty, they cannot settle or safely rollback non-idempotent real-world side effects (such as deleted files or external HTTP requests).
2. **Microkernel Violation**: Adding persistence violates core separation of concerns. Persistence is a host-level integration requirement that belongs at the CLI/edge layer ([`crates/mini-agent-cli/src/session.rs`](../../../../crates/mini-agent-cli/src/session.rs)).
