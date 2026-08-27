# Deterministic Hard Limits System

Status: implemented

## Context

Unbounded context growth, unchecked tool loops, and massive tool outputs frequently lead to out-of-memory crashes, runaway API expenses, and degraded reasoning quality. Instead of dynamic fuzzy limits or silent truncation of arbitrary data, the harness requires explicit, deterministic byte and step boundaries.

## Decision

All model-visible inputs, outputs, tool invocations, and conversation context sizes are bounded by hard limits specified in [`HarnessConfig`](crates/mini-agent-core/src/harness.rs):

| Boundary | Default Limit | Limit Behavior |
| :--- | :--- | :--- |
| Single typed context item | 8 KiB | Rejected before appending |
| User input | 32 KiB | Rejected before execution |
| Model response (reasoning + text) | 64 KiB | Rejected before executing tool calls |
| Tool calls in one step | 8 | Entire step proposal rejected |
| Single tool output | 16 KiB | Retains UTF-8 safe head and tail with `[truncated]` marker |
| Total request context | 1 MiB | Rejected before sampling (or triggers compaction if configured) |
| Model steps in one run | 8 (or 0 for unlimited) | Halts with `StopReason::StepLimit` |

Context size is calculated as serialized byte length (provider-neutral safety ceiling), scaled to match modern large-window models (e.g. DeepSeek V4).

## Consequences

- Tool output retains both initial context (head) and final verdict/error (tail) within a strict 16 KiB ceiling.
- Violations of safety boundaries immediately emit structured `RunFailed { reason: Limit }` events rather than silently dropping information.
- Protects downstream provider token windows and prevents unbudgeted token consumption.
