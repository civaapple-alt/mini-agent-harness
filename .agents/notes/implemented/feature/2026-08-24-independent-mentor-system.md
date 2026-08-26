# Independent Mentor Session Analysis and Verification

Status: implemented

## Context

Evaluating an agent's task outcomes within the same conversation loop risks confirmation bias, prompt contamination, and token explosion. An objective review requires an independent evaluator operating on settled artifacts.

## Decision

The CLI provides two dedicated mentor commands:
- `mini-agent mentor insight SESSION_ID` (analyzes decisions and friction points).
- `mini-agent mentor verify SESSION_ID <CRITERIA>` (evaluates outcomes against explicit success criteria).

1. **Isolation**:
   - The mentor uses an independently configured model (`MENTOR_OPENAI_MODEL`, `MENTOR_OPENAI_API_KEY`, `MENTOR_OPENAI_BASE_URL`).
   - The mentor runs strictly **tool-free** with a dedicated evaluation system prompt.
2. **Derived Storage**:
   - Mentor outputs are recorded into `session.jsonl` as `type: "derived"` items linked to the source checkpoint sequence and fingerprint.
   - Derived items are **never replayed** into the primary agent conversation history on session resume.

## Consequences

- Completely isolates primary agent execution from post-hoc quality assurance.
- Provides non-interactive, verifiable compliance checking without risking unintended side effects or tool executions during evaluation.
