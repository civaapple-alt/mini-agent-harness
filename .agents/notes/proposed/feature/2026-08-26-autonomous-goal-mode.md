# Autonomous Goal Mode and Continuous Progress Convergence

Status: proposed

## Context

Long-running autonomous tasks (such as `/goal` or overnight code refactorings) require more than just an uncapped step loop. When operating unattended over dozens or hundreds of steps, models risk entering repetitive loops, premature termination, or wandering off-course after repeated context compactions.

## Proposal

Introduce an explicit **Goal Mode** in `mini-agent-cli`:

1. **Self-Audit Convergence Gate**:
   - Before the agent completes a long-running goal, the harness injects an autonomous verification turn where the agent must verify test suites, diffs, and criteria before outputting `<!-- GOAL_COMPLETE -->`.
2. **Repetition & Loop Detection**:
   - Track consecutive tool call signatures across turns. If the agent repeatedly calls identical commands with identical arguments and outcomes, trigger an automated progress warning in context.
3. **Periodic Durable Checkpoints**:
   - Automatically persist settled turns to `~/.mini-agent/sessions/` every $N$ steps to guarantee resume points for overnight executions.

## Acceptance Criteria

- Unattended long-running tasks can autonomously self-correct and verify results without human intervention.
- Repetitive loops are caught early, avoiding token waste.

## Risks

- Adding loop detection must remain lightweight and avoid injecting non-deterministic prompts that confuse model reasoning.
