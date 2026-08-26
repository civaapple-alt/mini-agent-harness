# Real-Model Evaluation of Minimalist vs Heavy System Prompts

Status: proposed

## Context

Agent frameworks often inject extensive persona, guideline, and formatting instructions into system prompts. It remains unproven whether dense system prompt instructions measurably improve task completion over a minimalist prompt (`You are a coding agent. Use tools when needed and report the result plainly.`).

## Proposal

Execute the prompt weight evaluation benchmark ([docs/experiments/prompt-weight.md](file:///D:/gh-ws/codex-ws/mini-codex/docs/experiments/prompt-weight.md)) against real authorized provider models (e.g. DeepSeek V4 / OpenAI models):
1. Fix all WHAT parameters (task fixture, verifiers, tool catalog).
2. Compare Treatment A (minimal prompt: ~15 words) against Treatment B (verbose prompt: ~400 words with formatting rules).
3. Record latency, token consumption, model step counts, and deterministic verification pass rates in JSONL traces.

## Acceptance Criteria

- Publish trace comparison showing whether verbose prompt rules improve pass rate or merely increase token cost and response latency.
- Incorporate findings into default system prompt decisions without increasing line complexity.

## Risks

- Requires paid model provider authorization before execution.
