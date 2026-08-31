# Multi-Branch Tree Conversations and Speculative Lanes

Status: implemented

## Context

Durable sessions are linear append-only sequences of turns per thread. In complex debugging or architectural tasks, users or agents often want to explore alternative hypotheses from a prior checkpoint without mutating or losing the main line of exploration.

## Decision

Implemented explicit session branching via `mini-agent fork <SESSION_ID>`:
1. **Lineage-Preserving Forking**: In [`crates/mini-agent-capabilities/src/session.rs`](../../../../crates/mini-agent-capabilities/src/session.rs), `SessionStore::open(..., SessionRequest::Fork(parent_id))` reads the parent session's latest settled checkpoint and initializes a new discrete session directory and file.
2. **Session Header Provenance**: Writes a `session_created` record with `forked_from: { parent_session_id, parent_checkpoint_seq }` and seeds the new thread with the parent checkpoint messages.
3. **Core Run Loop Unchanged**: [`mini-agent-core`](../../../../crates/mini-agent-core/src/harness.rs) remains completely unaware of session graph topology, receiving only the linear checkpoint history path.

## Consequences

- Users can safely branch and explore speculative code solutions or alternate agent prompts.
- Zero state pollution or file corruption in the parent session.
- Verified by automated unit tests in [`crates/mini-agent-core/src/session_tests.rs`](../../../../crates/mini-agent-core/src/session_tests.rs) and CLI dispatch tests in [`crates/mini-agent-cli/src/main.rs`](../../../../crates/mini-agent-cli/src/main.rs).
