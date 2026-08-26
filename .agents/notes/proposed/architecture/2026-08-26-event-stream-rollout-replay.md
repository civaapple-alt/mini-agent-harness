# Deterministic Rollout Event Trace Replay and Offline Evaluator

Status: proposed

## Context

The harness writes detailed structured JSONL traces when `--trace PATH` is enabled. Currently, analyzing these traces requires manual inspection or custom parsing scripts. Offline debugging of regressions, token bottlenecks, and prompt effectiveness benefits from a native replay mechanism.

## Proposal

Implement a trace replay and inspection tool `mini-agent trace replay <TRACE_JSONL>` in `mini-agent-cli`:

1. **Deterministic Offline Replay**:
   - Step through recorded model reasoning, tool invocations, and tool outputs in real-time or step-by-step mode without invoking live model providers or executing external tools.
2. **Trace Diffing**:
   - Support comparing two trace files (`mini-agent trace diff trace1.jsonl trace2.jsonl`) to highlight divergences in tool choices, token consumption, and step latencies.
3. **Structured Metrics Summary**:
   - Print a terminal summary table of input/cached/output tokens, tool error counts, and truncation events.

## Acceptance Criteria

- Ability to replay any `--trace` output offline with accurate terminal tag formatting.
- Clear diff visualization for benchmark runs under different harness configurations.

## Risks

- Replay logic should stay confined to `mini-agent-cli` without increasing `mini-agent-core` complexity.
