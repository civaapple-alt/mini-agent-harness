# Real LLM scenario checks

The repository has two different test layers:

- cargo test --workspace uses deterministic local provider fixtures. It is safe
  for CI and does not spend provider capacity.
- This runner makes real Responses API requests. It is a manual integration
  check and never runs as part of cargo test or GitHub Actions by default.

## Safety and budget

The runner refuses to contact a provider unless --allow-paid is present. Every
invocation also has:

| Control | Default | Hard maximum |
| --- | ---: | ---: |
| provider requests | scenario budget | 12 |
| output tokens per request | 256 | 512 |
| wall time per scenario | 120 seconds | 120 seconds |

The request budget is reserved before each provider call, including the
auxiliary model request used by compaction. The output token limit is sent to
the provider (max_output_tokens for Responses and max_tokens for Chat
Completions). A failed or timed-out call still consumes any request already
started. In the JSONL, `model_steps` counts normal harness steps;
`requests_used` counts actual provider calls, including compaction.

## Scenarios

| Scenario | Maximum requests | What it checks |
| --- | ---: | --- |
| text | 1 | provider authentication, streaming text, response parsing, and exact final output |
| tool | 2 | function schema, tool-call argument parsing, local tool execution, and follow-up response |
| conversation | 2 | context retained across two harness runs |
| compaction | 2 | auxiliary summarization, bounded context compaction, and continuation |

The scenarios use short fixed prompts and deterministic verifiers. A passing
scenario demonstrates that one provider/model combination completed that
contract; it is not a general quality benchmark.

## Running

Export credentials in the process environment or put them in the workspace
.env file. Do not commit the file.

Run one cheap smoke check first:

    cargo run --release -p mini-agent-cli --example real_llm -- \
      --allow-paid --scenario text --max-requests 1 --max-output-tokens 64

Run the tool path separately:

    cargo run --release -p mini-agent-cli --example real_llm -- \
      --allow-paid --scenario tool --max-requests 2 --max-output-tokens 128

Run the full provider check, with a maximum of seven requests:

    cargo run --release -p mini-agent-cli --example real_llm -- \
      --allow-paid --scenario all --max-requests 7 --max-output-tokens 512

Use 512 output tokens for the compaction scenario: some Responses-compatible
models use a longer summary even when the final continuation is short.

Use --output /tmp/mini-agent-real-llm.jsonl to keep machine-readable evidence.
The path must not already exist. The output contains model results and provider
usage but never the API key; review it before sharing because model output may
still contain sensitive text.

The runner reads OPENAI_BASE_URL and OPENAI_MODEL using the same Responses
adapter as the CLI. It disables built-in web search so the scenario budget
covers only the requested model calls. Use a provider/model that accepts the
Responses API and its output-token parameter.

## Evidence review

For each run, retain the JSONL file together with:

- the git revision;
- the operating system and architecture;
- the provider endpoint family and model name;
- the selected scenarios and request/output budgets;
- pass/fail results and reported usage.

Do not treat a single pass as evidence of model quality. Compare repeated runs
only when the task prompts, model settings, scenario order, and budget are held
constant. The existing prompt_weight example is a separate paid experiment and
normally issues 12 requests per repetition; use it only when that experiment
is explicitly intended.

Vision, mentor, persistent CLI sessions, Goal Mode, MCP, and provider-specific
GLM image routing are intentionally separate test blocks. Add one scenario at a
time with an explicit request budget and a deterministic verifier; do not make
real calls a default release or CI gate.
