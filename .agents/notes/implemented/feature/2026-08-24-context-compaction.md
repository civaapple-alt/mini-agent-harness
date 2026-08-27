# Context Compaction with Tail and World State Retention

Status: implemented

## Context

Long-running autonomous tasks (copilot/auto loops) inevitably exceed raw context windows. Naive conversation summarization erases recent tool outputs, active execution authority, and current environmental facts, causing models to lose track of recent changes and repeat already executed operations.

## Decision

In `auto` mode (or when `ContextLimitBehavior::Compact` is enabled):

1. When settled history reaches half of the context limit (512 KiB / 1 MiB):
   - The latest typed [`Message::Context`](../../../../crates/mini-agent-core/src/model.rs) (e.g. World State) is preserved verbatim.
   - The last two model-step groups (capped at 128 KiB serialized) are preserved verbatim as the active tail.
   - Only the older prefix is sent to the model for structured summarization using a deterministic compaction prompt and an **empty tool catalog** (compact cannot call tools or attach `read_image` payloads).
2. If the compaction request itself is oversized, older prefix messages are mechanically dropped until it fits.
3. If the model returns an empty, invalid, or tool-calling summary, the harness falls back to mechanical prefix trimming instead of failing the run.

## Consequences

- Prevents context explosion while maintaining the model's awareness of the latest tool execution outcomes and system environment.
- Compaction runs as an auxiliary request without consuming the user's primary agent step budget.
- Dedicated trace events (`ContextCompactionStarted`, `ContextCompactionFinished`) ensure observability of context mutations.
