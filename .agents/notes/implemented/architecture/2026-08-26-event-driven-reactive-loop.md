# Event-Driven Reactive Loop and Passive Observers

Status: implemented

## Context

In complex agent architectures like Codex, tasks progress through an event-driven loop that simultaneously drives client-side live streaming (thinking deltas, assistant responses, tool execution status) and durable audit history (rollout event traces). Integrating intercepting middleware into the core loop often introduces hidden scheduling logic, side effects, and non-deterministic behavior.

## Decision

The harness adopts a pure, passive event-driven architecture based on [`Observer`](crates/mini-agent-core/src/event.rs):

1. **Passive Immutable Observers**:
   - `Observer::observe(&mut self, event: &Event)` accepts strictly immutable references.
   - Observers **cannot** alter model outputs, rewrite tool arguments, or redirect the control flow.
2. **Dual-Path Presentation & Audit**:
   - **Client Live Streaming**: Real-time rendering of `thinking>` (reasoning deltas), `assistant>` (text deltas), and single-line tool previews (`tool[ok]>`) in interactive terminals.
   - **Rollout Trace Logging**: When `--trace PATH` is supplied, every lifecycle event (`RunStarted`, `ModelStarted`, `ModelResponded`, `ToolStarted`, `ToolFinished`, `RunFinished`) is appended to a structured JSONL trace with exact token counts, latencies, and truncation flags.
3. **Reactive Turn Progression**:
   - Tool results are emitted via `Event::ToolFinished` and converted into `Message::Tool` items, reactively triggering the next model turn step until `tool_calls` is empty or a hard limit is reached.

## Consequences

- Completely decouples terminal presentation, metrics collection, and logging from the core execution engine.
- Guarantees 100% deterministic, audit-grade event logs for benchmarking and post-mortem review.
