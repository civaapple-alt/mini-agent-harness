# Circuit Breaking and Graceful Degradation for Remote HTTP MCP Tools

Status: implemented

## Context

Streamable HTTP MCP tools depend on external network stability. When a remote server encounters high latency, intermittent 500 errors, or connection drops during a multi-turn run, consecutive failing tool calls degrade the model's performance and burn step budgets.

## Decision

Implemented a lightweight fail-fast circuit breaker in [`crates/mini-agent-capabilities/src/mcp.rs`](../../../../crates/mini-agent-capabilities/src/mcp.rs):
1. **Failure Threshold & Cooldown**: Tracks consecutive transport errors and timeouts (threshold: 3).
2. **Fail-Fast Open State**: Upon reaching the threshold, trips into an `Open` state for a 30-second cooldown window, immediately failing subsequent calls with a clear diagnostic error:
   `"MCP server circuit breaker is open (failing fast after 3 consecutive errors)"`
3. **Automatic Half-Open Probe & Reset**: After the cooldown expires, the next call serves as a trial probe. A successful execution immediately resets the failure counter to 0.

## Consequences

- Prevents hanging the agent loop and burning step limits on down or lagging remote MCP servers.
- Fully isolated in the CLI MCP transport layer without adding any complexity to `mini-agent-core`.
- Automated test coverage in [`crates/mini-agent-capabilities/src/mcp_tests.rs`](../../../../crates/mini-agent-capabilities/src/mcp_tests.rs).
