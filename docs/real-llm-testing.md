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
| provider requests | scenario budget | 20 |
| output tokens per request | 256 | 1024 |
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
| persistence | 2 | settled session checkpoint, process-style reopen, and restored context |
| vision | 3 | image tool result, provider image projection, and vision-model response |
| compaction | 2 | auxiliary summarization, bounded context compaction, and continuation |
| mentor | 1 | independent mentor response, source checkpoint, and persisted derived insight |
| goal | 1 | independent verifier verdict parsing, verdict persistence, and milestone advancement |
| mcp | 2 | production MCP stdio handshake, tool discovery/call, and model tool settlement |

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

Run the full provider check, with a maximum of sixteen provider requests:

    cargo run --release -p mini-agent-cli --example real_llm -- \
      --allow-paid --scenario all --max-requests 16 --max-output-tokens 512

Use 1024 output tokens for the compaction scenario: some Responses-compatible
models use a longer summary even when the final continuation is short. Some
provider-compatible endpoints also reject very low `max_output_tokens` values;
if the response reports a `max_output_tokens` validation error, retry with
1024 and record that provider limitation in the evidence.

Use --output /tmp/mini-agent-real-llm.jsonl to keep machine-readable evidence.
The path must not already exist. The output contains model results and provider
usage but never the API key; review it before sharing because model output may
still contain sensitive text.

The runner reads OPENAI_BASE_URL and OPENAI_MODEL using the same Responses
adapter as the CLI. Mentor and Goal use MENTOR_OPENAI_MODEL, with
MENTOR_OPENAI_API_KEY and MENTOR_OPENAI_BASE_URL optional fallbacks to the
primary provider settings. It disables built-in web search so the scenario
budget covers only the requested model calls. Use a provider/model that accepts
the Responses API and its output-token parameter.

On macOS and Linux, use `python3` when a local Python command is needed; on
Windows, use `python`. The runner probes `python3` first and then `python` for
its local MCP fixture, so the same scenario command works on both host
families.

The mentor block creates a short settled session, invokes the independent
mentor prompt, and checks that the production derived-item record points at the
source checkpoint. The Goal block creates a bounded temporary Goal workspace,
uses the production verdict parser, records the verdict, and advances one
milestone. The MCP block starts a local Python stdio fixture through the
production MCP loader, checks discovery and a preflight call, then lets the
real model call the exposed MCP tool. The fixture is local and does not add a
network service or provider charge.

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

Persistent CLI sessions and provider-specific image handling remain covered by
separate paths inside this runner. Add one scenario at a time with an
explicit request budget and a deterministic verifier; do not make real calls a
default release or CI gate.
