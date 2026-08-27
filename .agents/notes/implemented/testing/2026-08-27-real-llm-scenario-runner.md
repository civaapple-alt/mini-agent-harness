# Real LLM Scenario Runner

Status: implemented

## Decision

Real-provider checks live in the manual example
crates/mini-agent-cli/examples/real_llm.rs rather than in the default test
suite. The runner requires an explicit --allow-paid acknowledgement and
supports six bounded scenarios:

- text: one request for authentication, streaming, and final-text parsing;
- tool: two requests for function schema, local execution, and settlement;
- conversation: two requests for context carried across harness runs;
- persistence: two requests for a settled checkpoint, reopen, and restored context;
- vision: up to three requests including DeepSeek Files upload, image projection, and response;
- compaction: two requests including the auxiliary summarization request.

The selected scenarios must fit the invocation-wide request budget. The hard
maximum is 12 requests. Each provider request also receives an explicit output
token ceiling, defaulting to 256 and capped at 1024, and each scenario has a
bounded wall-clock timeout. Results are emitted as JSONL without the API key.

## Boundary

This runner tests the real provider adapter plus the portable core harness. It
does not pretend to prove general model quality, and it does not automatically
test vision, mentor, MCP, Goal Mode, persistent CLI sessions, or provider
specific GLM image routing. Those paths need separate fixtures and budgets.

Real calls are never made by cargo test, CI, or the normal release workflow.
The prompt-weight experiment remains separate because it intentionally uses a
larger repeated-call budget.

## Verification

The runner has deterministic argument parsing tests and can be invoked with
--help without credentials. Its live evidence includes selected scenarios,
request usage, provider-reported token usage, compaction count, and verifier
results. Review the JSONL together with the revision and host environment.
