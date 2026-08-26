# Autonomous Goal Mode and Continuous Progress Convergence

Status: implemented

## Context

Long-running autonomous tasks operate unattended over many model steps. When tasks involve complex multi-step debugging or refactoring, models risk entering repetitive loops (calling identical tools with identical failing arguments), premature termination, or wandering off-course after context compactions.

## Decision

Implemented autonomous goal convergence and repetition detection:

1. **Repetitive Tool Call Loop Detection**:
   - In `crates/mini-agent-core/src/harness.rs`, `Harness::run` tracks consecutive tool call signatures across turn steps.
   - When the agent produces identical tool calls and arguments across consecutive turns without progress (threshold $\ge 2$), the harness injects an explicit advisory warning context:
     `[Loop warning: identical tool calls were repeated without progress. Please adjust arguments or try an alternate strategy.]`
2. **Unattended Copilot Execution (`mini-agent auto`)**:
   - Runs with automatic approval (`ApprovalMode::Automatic`), uncapped step capacity (`max_steps: 0`), and dynamic prefix compaction preserving recent tool tail context verbatim.
3. **Deterministic Persistence & Recovery**:
   - Checkpoints settled turn history to `~/.mini-agent/sessions/` when `--persist` is enabled.

## Consequences

- Prevents runaway token waste caused by repetitive tool invocation loops in long-running agent tasks.
- Gives the model clear, actionable context signals to pivot strategy when a tool action stalls.
- Preserves the lightweight boundary of `mini-agent-core` with deterministic test coverage.
