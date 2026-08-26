# System-Prompt Weight Experiment Protocol

Status: implemented
Archived: 2026-08-24

## Status

The repeatable evaluation entry point is implemented. No model-backed result has been recorded yet because running it consumes provider capacity and must be an explicit operator action.

## Question

Does an expanded operational system prompt improve tool-use correctness enough to justify its permanent context and token cost over the minimal default?

## Fixed WHAT

- the exact model named by `OPENAI_MODEL`;
- three deterministic lookup tasks and verifiers;
- one tool schema and implementation;
- three harness steps and the same tool-output budget;
- treatment order alternated across tasks and repetitions;
- full ordered events, latency, and reported token usage in JSONL.

Only the system prompt changes. The prompt fixtures are [minimal](docs/experiments/fixtures/prompt-weight-minimal.txt) and [expanded](docs/experiments/fixtures/prompt-weight-expanded.txt). They have aligned operational intent but are not claimed to be logically identical.

## Run

One repetition attempts six runs and normally makes twelve Responses API calls because each successful task has a tool-call step and a final-answer step.

```sh
OPENAI_API_KEY=... OPENAI_MODEL=... \
  cargo run -p mini-agent-cli --example prompt_weight -- \
  --runs 3 --output prompt-weight.jsonl
```

The output path must not already exist. The example defaults to one repetition, caps repetitions at twenty, never prints the API key, and does not alter the normal `mini-agent` command surface.

## Decision Rule

Compare exact verifier pass rate first, then tool argument correctness, model steps, latency, input tokens, cached input tokens, and output tokens. Prefer the minimal prompt unless the expanded prompt produces a repeatable task-quality improvement that justifies its extra context. A single run is a smoke test, not evidence for changing the default.
