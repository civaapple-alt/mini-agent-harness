# Historical Real LLM Scenario Runner

Status: historical — runner removed from the current mainline

## Current status update (2026-08-31)

The manual runner described below was removed when non-mainline experiments were
retired. The current workspace has no `crates/mini-agent-cli/examples/real_llm.rs`
entry point, so this note preserves the former bounded real-provider contract and
its evidence expectations only. No paid-provider run is implied by this historical
record; the current `docs/harness-evidence.md` guide tracks real-provider evidence
as open.

## Decision

At the time of this design, real-provider checks lived in the manual example
`crates/mini-agent-cli/examples/real_llm.rs` rather than in the default test
suite. That runner required an explicit `--allow-paid` acknowledgement and
supports nine bounded scenarios:

- text: one request for authentication, streaming, and final-text parsing;
- tool: two requests for function schema, local execution, and settlement;
- conversation: two requests for context carried across harness runs;
- persistence: two requests for a settled checkpoint, reopen, and restored context;
- vision: up to three requests including DeepSeek Files upload, image projection, and response;
- compaction: two requests including the auxiliary summarization request.
- verifier: one request for an independent review and a persisted derived item.
- goal: one request for verifier parsing, verdict persistence, and milestone advancement.
- mcp: two requests for a production MCP stdio path plus model tool settlement.

The selected scenarios must fit the invocation-wide request budget. The hard
maximum is 20 requests. Each provider request also receives an explicit output
token ceiling, defaulting to 256 and capped at 1024, and each scenario has a
bounded wall-clock timeout. Results are emitted as JSONL without the API key.

## Boundary

This runner tests the real provider adapter plus the portable core harness. It
does not pretend to prove general model quality. The verifier and Goal use production
state/verdict code; MCP uses the production loader with a local stdio fixture.
Persistent CLI sessions and provider image projection remain scenario-specific
and budgeted; every provider request uses the single Responses protocol.

Real calls are never made by cargo test, CI, or the normal release workflow.
The prompt-weight experiment remains separate because it intentionally uses a
larger repeated-call budget.

## Verification

The runner has deterministic argument parsing tests and can be invoked with
--help without credentials. Its live evidence includes selected scenarios,
request usage, provider-reported token usage, compaction count, and verifier
results. Review the JSONL together with the revision and host environment.
