# Deterministic Rollout Event Trace Replay and Offline Evaluator

Status: rejected — Detailed external trace replay was prompt-weight-specific; session.jsonl is the mainline durable record.

## Context

The harness writes detailed structured JSONL traces when `--trace PATH` is enabled. Analyzing and debugging regressions, token bottlenecks, and tool usage offline requires an inspection and replay mechanism that does not trigger paid model API calls or execute destructive external tools.

## Decision

Implemented trace playback and inspection in `crates/mini-agent-cli/src/trace.rs`:

1. **Deterministic Offline Playback (`mini-agent trace replay <TRACE_JSONL>`)**:
   - Parses the JSONL events recorded by `--trace PATH` (`RunStarted`, `ModelStarted`, `AssistantReasoningDelta`, `AssistantTextDelta`, `ToolStarted`, `ToolFinished`, `ContextCompactionStarted`, `ContextCompactionFinished`, `RunFinished`).
   - Streams formatted playback with matching terminal color tags (`thinking>`, `assistant>`, `tool>`, `tool[ok]>`, `tool[error]>`, `context>`) without executing real tools or calling LLM endpoints.
2. **Structured Execution Summary (`mini-agent trace summary <TRACE_JSONL> [--json]`)**:
   - Calculates total model requests, input tokens, cached tokens, output tokens, tool calls breakdown (success, error, truncated), compactions, and stop reasons.
   - Emits either a clean human-readable table or structured JSON for machine ingestion.

## Consequences

- Completely offline debugging of model reasoning traces and agent trajectories.
- Zero extra complexity added to the microkernel core `mini-agent-core`.
- Automated test coverage in `crates/mini-agent-cli/src/trace.rs` and CLI integration.
